//! Daemon Lapinux : bibliothèque, téléchargements, sessions de jeu et overlay.

pub mod captures;
mod cloud_journal;
mod configurations;
pub mod discord;
pub mod discord_apps;
pub mod doctor;
mod fps;
mod frames;
mod kwin_shot;
pub mod jobs;
pub mod launch;
pub mod logs;
mod saves;
pub mod social;
pub mod steam;
mod steam_headless;
pub mod stores;
mod toasts;
mod tray;

use lapinux_core::config::Config;
use lapinux_core::ipc::{self, Envelope, Req};
use lapinux_core::model::{
    Game, Job, JobReport, JobState, ProtonInfo, SaveEvent, SaveEventKind, SessionInfo,
    SteamLoginState,
};
use lapinux_core::paths::LapinuxPaths;
use lapinux_core::store::Store;
use std::collections::{HashMap, HashSet};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub type Result<T> = std::result::Result<T, lapinux_core::Error>;

/// Plafonds de débit partagés par tous les téléchargements du daemon.
///
/// Le global est commun à tous les stores ; chaque store a son propre
/// limiteur, réutilisé par ses tâches successives. Les arcs ne changent
/// jamais : un réglage modifié s'applique donc en direct, sans relancer le
/// téléchargement en cours.
pub struct Throttle {
    global: Arc<lapinux_steam::throttle::RateLimiter>,
    stores: Mutex<HashMap<String, Arc<lapinux_steam::throttle::RateLimiter>>>,
}

impl Throttle {
    fn new(config: &Config) -> Self {
        let global = lapinux_steam::throttle::RateLimiter::new(mbit_per_sec_to_bytes(
            config.downloads.global_mbit_per_sec,
        ));
        let mut stores = HashMap::new();
        for (store, mbit) in &config.downloads.store_mbit_per_sec {
            stores.insert(
                store.clone(),
                lapinux_steam::throttle::RateLimiter::new(mbit_per_sec_to_bytes(*mbit)),
            );
        }
        Self {
            global,
            stores: Mutex::new(stores),
        }
    }

    fn apply(&self, config: &Config) {
        self.global.set_rate(mbit_per_sec_to_bytes(
            config.downloads.global_mbit_per_sec,
        ));
        if let Ok(mut stores) = self.stores.lock() {
            for (store, mbit) in &config.downloads.store_mbit_per_sec {
                stores
                    .entry(store.clone())
                    .or_insert_with(|| lapinux_steam::throttle::RateLimiter::new(0))
                    .set_rate(mbit_per_sec_to_bytes(*mbit));
            }
        }
    }

    /// Limiteur d'un store, créé à la première utilisation. Un store sans
    /// plafond garde un limiteur à zéro : la valeur pourra changer en cours
    /// de téléchargement.
    pub fn limit(&self, store: &str) -> lapinux_steam::throttle::DownloadLimit {
        let store = self.stores.lock().ok().map(|mut stores| {
            stores
                .entry(store.to_string())
                .or_insert_with(|| lapinux_steam::throttle::RateLimiter::new(0))
                .clone()
        });
        lapinux_steam::throttle::DownloadLimit {
            global: Some(Arc::clone(&self.global)),
            store,
        }
    }
}

/// Mégabits par seconde → octets par seconde (réseau : 1 Mbit = 1 000 000
/// bits, l'unité des débits affichés partout dans l'application).
/// Impose à toutes les lectures Steam la langue de jeu choisie dans les
/// réglages : c'est elle qui décide des dépôts de doublage téléchargés et du
/// poids annoncé. Vide = la langue du client Steam, sinon celle du bureau.
/// Déplace un dossier sur place, ou recopie puis remplace si les deux
/// extrémités sont sur des disques différents. Une coupure laisse l'ancien
/// dossier intact : la copie se fait à côté puis prend la place d'un coup.
fn move_directory(from: &Path, to: &Path) {
    if std::fs::rename(from, to).is_ok() {
        return;
    }
    let Some(parent) = to.parent() else {
        return;
    };
    let Some(name) = to.file_name() else {
        return;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let staging = parent.join(format!(".{}.moving", name.to_string_lossy()));
    let _ = std::fs::remove_dir_all(&staging);
    if lapinux_core::util::copy_tree(from, &staging).is_err() {
        let _ = std::fs::remove_dir_all(&staging);
        return;
    }
    if std::fs::rename(&staging, to).is_ok() {
        let _ = std::fs::remove_dir_all(from);
    } else {
        let _ = std::fs::remove_dir_all(&staging);
    }
}

fn apply_game_language(config: &Config) {
    let code = lapinux_core::lang::find(&config.general.game_language)
        .map(|language| language.code);
    lapinux_steam::native::set_language_override(code);
}

fn mbit_per_sec_to_bytes(mbit: u32) -> u64 {
    mbit as u64 * 1_000_000 / 8
}

/// Échantillonneur de débit d'une tâche. Une fenêtre d'environ une seconde
/// lisse les rafales de chunks : un pic de 150 ms donnerait un maximum
/// artificiel et un minimum proche de zéro.
#[derive(Default)]
struct RateTracker {
    last_at: Option<Instant>,
    last_network: u64,
    low: Option<f64>,
    high: Option<f64>,
    active_secs: f64,
    weighted: f64,
    samples: u32,
}

impl RateTracker {
    fn sample(&mut self, now: Instant, network: u64) {
        let Some(last) = self.last_at else {
            self.last_at = Some(now);
            self.last_network = network;
            return;
        };
        let elapsed = now.duration_since(last).as_secs_f64();
        if network > self.last_network && elapsed >= 0.9 {
            let rate = (network - self.last_network) as f64 / elapsed;
            self.low = Some(self.low.map_or(rate, |low| low.min(rate)));
            self.high = Some(self.high.map_or(rate, |high| high.max(rate)));
            self.active_secs += elapsed;
            self.weighted += rate * elapsed;
            self.samples += 1;
            self.last_at = Some(now);
            self.last_network = network;
        } else if elapsed >= 2.5 {
            // Aucun octet depuis un moment (vérification locale, reprise) :
            // on repart d'une fenêtre neuve plutôt que de compter un débit nul.
            self.last_at = Some(now);
            self.last_network = network;
        }
    }

    /// Fenêtre remise à zéro quand la tâche s'arrête : le temps de pause ne
    /// doit pas entrer dans le débit moyen.
    fn reset_window(&mut self, network: u64) {
        self.last_at = None;
        self.last_network = network;
    }

    fn finish(&mut self, now: Instant, network: u64) {
        if let Some(last) = self.last_at {
            let elapsed = now.duration_since(last).as_secs_f64();
            if network > self.last_network && elapsed >= 0.2 {
                let rate = (network - self.last_network) as f64 / elapsed;
                self.low = Some(self.low.map_or(rate, |low| low.min(rate)));
                self.high = Some(self.high.map_or(rate, |high| high.max(rate)));
                self.active_secs += elapsed;
                self.weighted += rate * elapsed;
                self.samples += 1;
            }
        }
        self.last_at = None;
    }

    fn report(&self, network: u64, written: u64, elapsed_secs: u64) -> JobReport {
        let average = if self.active_secs > 0.0 {
            (self.weighted / self.active_secs) as u64
        } else if elapsed_secs > 0 {
            network / elapsed_secs
        } else {
            0
        };
        JobReport {
            elapsed_secs,
            active_secs: self.active_secs.round() as u64,
            average_bps: average,
            peak_bps: self.high.unwrap_or(0.0) as u64,
            low_bps: self.low.unwrap_or(0.0) as u64,
            network,
            written,
            samples: self.samples,
        }
    }
}

/// Clé du store pour la liste des tâches, et taille maximale conservée.
const JOBS_KEY: &str = "jobs";
const JOBS_KEEP: usize = 80;

/// Recharge les tâches du dernier daemon. Une tâche qui était en cours ou en
/// pause ne peut pas reprendre seule après un arrêt : elle devient une tâche
/// *interrompue*, à reprendre d'un clic, avec ses compteurs et les fichiers
/// déjà reçus. Elle ne doit pas être confondue avec une annulation, dont la
/// carte disparaît définitivement.
fn restore_jobs(store: &Store) -> Vec<Job> {
    let mut jobs: Vec<Job> = match store.get_json(JOBS_KEY) {
        Ok(jobs) => jobs.unwrap_or_default(),
        Err(err) => {
            tracing::warn!("tâches enregistrées illisibles : {err}");
            Vec::new()
        }
    };
    let now = lapinux_core::util::now_unix();
    for job in &mut jobs {
        if matches!(
            job.state,
            JobState::Running | JobState::Queued | JobState::Paused
        ) {
            job.state = JobState::Interrupted;
            job.finished = Some(now);
            job.message =
                "interrompu par le redémarrage ; les fichiers déjà reçus sont conservés".into();
        } else if job.state == JobState::Cancelled
            && job.message.contains("redémarrage")
        {
            // Fichier écrit par une version qui marquait les interruptions du
            // démarrage comme des annulations : ces tâches sont à reprendre,
            // elles n'ont pas été annulées par l'utilisateur.
            job.state = JobState::Interrupted;
        }
    }
    // Une annulation ne se survit pas : sa carte a déjà disparu de la liste
    // au moment du clic, seul un fichier d'une ancienne version peut encore
    // en contenir.
    jobs.retain(|job| job.state != JobState::Cancelled);
    jobs.retain(|job| !job.id.trim().is_empty());
    jobs.sort_by_key(|job| job.started);
    if jobs.len() > JOBS_KEEP {
        jobs.drain(0..jobs.len() - JOBS_KEEP);
    }
    jobs
}

pub struct Shared {
    pub paths: LapinuxPaths,
    pub store: Arc<Store>,
    pub config: Mutex<Config>,
    pub jobs: Mutex<Vec<Job>>,
    /// Dernière écriture des tâches sur disque, pour limiter les écritures
    /// pendant qu'un téléchargement avance de quelques mégaoctets.
    jobs_saved: Mutex<Option<Instant>>,
    pub cancel: Mutex<HashMap<String, Arc<AtomicBool>>>,
    /// Drapeaux de pause des téléchargements, par identifiant de tâche.
    pub pause: Mutex<HashMap<String, Arc<AtomicBool>>>,
    pub protons: Mutex<Vec<ProtonInfo>>,
    pub session: Mutex<Option<launch::SessionState>>,
    /// Enregistrement vidéo de la partie en cours, quand il tourne.
    pub recorder: Mutex<Option<captures::Recorder>>,
    /// Sérialise le démarrage d'un enregistrement : deux appels rapprochés
    /// (fenêtre du jeu, minuteur de session) ne doivent pas lancer deux
    /// encodeurs sur la même partie.
    pub recording_start: Mutex<()>,
    /// Échantillonneur système du HUD de performance : il garde les compteurs
    /// précédents, donc il doit vivre avec le daemon.
    system: Mutex<lapinux_core::system::SystemSampler>,
    pub overlay: Mutex<Option<std::process::Child>>,
    /// Plaque de performance de la partie : un overlay sans menu, qui vit du
    /// lancement à la fin de la session.
    pub perf_hud: Mutex<Option<std::process::Child>>,
    /// HUD d'enregistrement : un processus et une surface à part, pour que
    /// couper la plaque de performance ne retire jamais le badge REC.
    pub record_hud: Mutex<Option<std::process::Child>>,
    /// Bandeaux de succès : file d'attente et fil qui les affiche un à un.
    pub(crate) toasts: Mutex<crate::toasts::ToastQueue>,
    /// Tube de navigation de l'overlay : le service d'entrée y écrit les
    /// appuis de la manette pendant que le panneau est ouvert.
    pub nav_out: Mutex<Option<std::fs::File>>,
    /// Synchronisation du catalogue Steam en arrière-plan.
    pub catalog_running: AtomicBool,
    /// Les fiches détaillées ont déjà été demandées une fois.
    pub catalog_details_checked: AtomicBool,
    pub catalog_error: Mutex<Option<String>>,
    /// Dernière connexion Steam demandée par l'écran de connexion.
    pub login: Mutex<SteamLoginState>,
    /// Dernière vérification du jeton enregistré (verdict, date, réponse).
    pub token_check: Mutex<crate::steam::TokenCheck>,
    /// Lectures Steam par jeu (fiche, succès) gardées quelques minutes : chaque
    /// lecture ouvrait une session, et Steam n'en garde qu'une par compte.
    pub game_reads: Mutex<crate::steam::GameReadCache>,
    /// Relevé du cloud pris au lancement d'un jeu, gardé jusqu'à sa sortie :
    /// la comparaison dit ce que la partie a envoyé sur le cloud Steam.
    pub cloud_before: Mutex<Option<(u32, lapinux_core::model::CloudSaves)>>,
    /// Comparaison cloud déjà faite pour un jeu, gardée jusqu'au lancement
    /// suivant : la fiche l'affiche sans relire le cloud à chaque repeint.
    pub cloud_last: Mutex<Option<(u32, lapinux_core::model::CloudDiff)>>,
    /// Session sociale Steam : présence publiée, amis, famille. Née au premier
    /// besoin et conservée tant que le daemon vit.
    pub social: Mutex<Option<Arc<lapinux_steam::social::Social>>>,
    /// Jeu Steam dont la partie est en cours. Il est publié sur Steam quand le
    /// réglage l'autorise, et repris par une session qui se reconnecte.
    pub steam_playing: Mutex<Option<u32>>,
    /// Ce que l'interface affiche, publié par Discord Rich Presence.
    pub discord_view: Mutex<discord::Viewing>,
    /// Rich Presence autorisée : l'interface la coupe en se fermant, sans
    /// toucher au réglage enregistré.
    pub discord_active: AtomicBool,
    /// Application Discord officielle de chaque jeu, lue dans le catalogue
    /// public de Discord : elle donne au Rich Presence le vrai nom du jeu.
    pub discord_apps: Mutex<discord_apps::Index>,
    /// Configurations connues des jeux, lues dans `configurations/catalog.json`
    /// du dépôt : la fiche propose de les installer sur le jeu ouvert.
    pub configurations: Mutex<configurations::State>,
    /// Session sociale en pause : une synchronisation du catalogue tient la
    /// seule session du compte, et rien ne doit la recréer avant la fin.
    pub social_paused: AtomicBool,
    /// Code Steam Guard à transmettre à la connexion en cours.
    pub login_guard: Mutex<Option<std::sync::mpsc::Sender<String>>>,
    /// Demande d'annulation de la connexion en cours.
    pub login_cancel: AtomicBool,
    /// Plafonds de débit des téléchargements, réglés depuis la page
    /// Téléchargements.
    pub throttle: Throttle,
    /// Débits observés par tâche, uniquement pendant le transfert.
    rates: Mutex<HashMap<String, RateTracker>>,
    /// Date du binaire qui a lancé ce daemon : l'interface s'en sert pour
    /// remplacer un daemon resté sur une version antérieure.
    pub build: lapinux_core::ipc::DaemonBuild,
}

impl Shared {
    pub fn new(paths: &LapinuxPaths) -> Result<Self> {
        paths.ensure().map_err(lapinux_core::Error::Io)?;
        // La permission de capture doit exister avant le démarrage de KWin :
        // elle est écrite dès que Lapinux tourne, pas à la première vidéo.
        captures::ensure_permission_file();
        // Les captures d'une version antérieure vivaient dans `~/Images` :
        // elles rejoignent les dossiers Vidéos et Images du bureau.
        crate::launch::migrate_legacy_captures();
        let config = Config::load_or_create(paths)?;
        // La langue de la configuration vaut pour tout le processus : sans
        // cet appel, le tray restait en anglais alors que l'interface était
        // bien en français (le défaut de `i18n` est l'anglais).
        lapinux_core::i18n::set(&config.general.lang);
        apply_game_language(&config);
        // Les sauvegardes passent à trois emplacements : les anciennes séries
        // sont réduites avant même la première ouverture d'une fiche.
        crate::saves::settle_all(paths, &config);
        let throttle = Throttle::new(&config);
        // Le prédécesseur peut tenir la base quelques instants après son
        // signal d'arrêt : une relance immédiate échouait sur le verrou
        // (« Database already open »). On lui laisse le temps de rendre.
        let mut opened = None;
        let mut last = None;
        for attempt in 0..20 {
            match Store::open(&paths.db_file) {
                Ok(store) => {
                    opened = Some(store);
                    break;
                }
                Err(error) => {
                    last = Some(error);
                    if attempt < 19 {
                        std::thread::sleep(std::time::Duration::from_millis(250));
                    }
                }
            }
        }
        let store = Arc::new(match opened {
            Some(store) => store,
            None => return Err(last.expect("Store::open a échoué sans erreur")),
        });
        let jobs = restore_jobs(&store);
        Ok(Self {
            paths: paths.clone(),
            store,
            config: Mutex::new(config),
            jobs: Mutex::new(jobs),
            jobs_saved: Mutex::new(None),
            cancel: Mutex::new(HashMap::new()),
            pause: Mutex::new(HashMap::new()),
            protons: Mutex::new(Vec::new()),
            session: Mutex::new(None),
            recorder: Mutex::new(None),
            recording_start: Mutex::new(()),
            system: Mutex::new(lapinux_core::system::SystemSampler::new()),
            overlay: Mutex::new(None),
            perf_hud: Mutex::new(None),
            record_hud: Mutex::new(None),
            toasts: Mutex::new(crate::toasts::ToastQueue::default()),
            nav_out: Mutex::new(None),
            catalog_running: AtomicBool::new(false),
            catalog_details_checked: AtomicBool::new(false),
            catalog_error: Mutex::new(None),
            login: Mutex::new(SteamLoginState::default()),
            token_check: Mutex::new(crate::steam::TokenCheck::default()),
            game_reads: Mutex::new(crate::steam::GameReadCache::default()),
            cloud_before: Mutex::new(None),
            cloud_last: Mutex::new(None),
            social: Mutex::new(None),
            steam_playing: Mutex::new(None),
            discord_view: Mutex::new(discord::Viewing::default()),
            discord_active: AtomicBool::new(true),
            discord_apps: Mutex::new(discord_apps::Index::load(paths)),
            configurations: Mutex::new(configurations::State::new()),
            social_paused: AtomicBool::new(false),
            login_guard: Mutex::new(None),
            login_cancel: AtomicBool::new(false),
            throttle,
            rates: Mutex::new(HashMap::new()),
            build: lapinux_core::ipc::DaemonBuild {
                stamp: lapinux_core::paths::current_binary_stamp(),
                version: lapinux_core::VERSION.to_string(),
                started: lapinux_core::util::now_unix().max(0) as u64,
            },
        })
    }

    /// Enregistre les tâches sur disque. Les mises à jour de progression sont
    /// limitées à une écriture toutes les deux secondes ; un changement
    /// d'état force l'écriture immédiate.
    pub fn persist_jobs(&self, force: bool) {
        if !force {
            let recent = self
                .jobs_saved
                .lock()
                .ok()
                .and_then(|last| *last)
                .map(|at| at.elapsed() < Duration::from_secs(2))
                .unwrap_or(false);
            if recent {
                return;
            }
        }
        let mut jobs = self
            .jobs
            .lock()
            .map(|jobs| jobs.clone())
            .unwrap_or_default();
        if jobs.len() > JOBS_KEEP {
            jobs.drain(0..jobs.len() - JOBS_KEEP);
        }
        match self.store.set_json(JOBS_KEY, &jobs) {
            Ok(()) => {
                if let Ok(mut last) = self.jobs_saved.lock() {
                    *last = Some(Instant::now());
                }
            }
            Err(err) => tracing::warn!("persistance des tâches : {err}"),
        }
    }

    pub fn config_snapshot(&self) -> Config {
        self.config
            .lock()
            .map(|c| c.clone())
            .unwrap_or_default()
    }

    /// Applique une modification du fichier sans redémarrer le daemon.
    pub fn reload_config(&self) {
        let Ok(config) = Config::load(&self.paths.config_file) else {
            return;
        };
        lapinux_core::i18n::set(&config.general.lang);
        apply_game_language(&config);
        if let Ok(mut guard) = self.config.lock() {
            *guard = config;
        }
    }

    /// Scanner les bibliothèques Steam et fusionner avec les réglages connus.
    pub fn rescan_library(&self) -> Result<Vec<Game>> {
        let managed = self.config_snapshot().library_root();
        self.rescan_library_at(&managed)
    }

    /// Scan en utilisant la racine indiquée : un téléchargement doit voir le
    /// manifeste qu'il vient d'écrire même si les réglages ont changé entre-temps.
    pub(crate) fn rescan_library_at(&self, managed: &std::path::Path) -> Result<Vec<Game>> {
        self.migrate_managed_layout(managed);
        let roots = lapinux_core::paths::steam_roots();
        let report = lapinux_steam::scan_with_managed(&roots, Some(managed));
        for warning in &report.warnings {
            tracing::warn!("scan : {warning}");
        }
        let stored = self.store.load_games().unwrap_or_default();
        let merged = lapinux_steam::merge_existing(report.games, &stored);
        self.store
            .save_games(&merged)
            .map_err(|e| lapinux_core::Error::Other(e.to_string()))?;
        Ok(merged)
    }

    /// Range une bibliothèque Lapinux dans sa forme lisible : les jeux Steam
    /// quittent `steamapps/common` pour `Steam`, leurs manifestes suivent, et
    /// les préfixes passent de `compatdata/<id>` à `CompatData/<jeu> (<id>)`.
    /// Ce ne sont que des renommages : aucun octet n'est recopié sur le même
    /// disque. Un marqueur évite de relire ces dossiers à chaque scan.
    fn migrate_managed_layout(&self, managed: &Path) {
        let marker = managed.join(".lapinux-layout");
        if marker.is_file() || !managed.is_dir() {
            return;
        }
        // Une vraie bibliothèque Steam garde son organisation : si la racine
        // est déclarée par Steam, ses jeux ne sont pas à Lapinux.
        let roots = lapinux_core::paths::steam_roots();
        let canon = std::fs::canonicalize(managed).unwrap_or_else(|_| managed.to_path_buf());
        let real_steam = lapinux_steam::discover_libraries_with_managed(&roots, Some(managed))
            .into_iter()
            .find(|library| library.root == canon)
            .map(|library| !library.managed)
            .unwrap_or(false);
        if real_steam {
            return;
        }
        let legacy = managed.join("steamapps");
        let steam = managed.join("Steam");
        let compat = managed.join("CompatData");
        let stored = self.store.load_games().unwrap_or_default();
        let mut names: HashMap<u32, String> = stored
            .iter()
            .filter(|game| !game.name.trim().is_empty())
            .map(|game| (game.appid, game.name.clone()))
            .collect();
        // Seuls les préfixes des jeux que Lapinux possède suivent : ceux d'un
        // vrai Steam restent dans le `compatdata` de Steam.
        let mut managed_ids: HashSet<u32> = stored
            .iter()
            .filter(|game| game.origin != lapinux_core::model::GameOrigin::Steam)
            .map(|game| game.appid)
            .collect();

        // Un manifeste écrit par Lapinux dit quels dossiers de `steamapps`
        // lui appartiennent. Ceux d'un vrai Steam restent en place.
        let mut manifests: Vec<(u32, String, PathBuf)> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&legacy) {
            for entry in entries.flatten() {
                let path = entry.path();
                let is_manifest = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(|name| name.starts_with("appmanifest_") && name.ends_with(".acf"))
                    .unwrap_or(false);
                if !is_manifest {
                    continue;
                }
                let Some(manifest) = lapinux_steam::AppManifest::read(&path).ok() else {
                    continue;
                };
                if !manifest.lapinux_managed {
                    continue;
                }
                names
                    .entry(manifest.appid)
                    .or_insert_with(|| manifest.name.clone());
                managed_ids.insert(manifest.appid);
                manifests.push((manifest.appid, manifest.name, path));
            }
        }

        // 1. Les dossiers des jeux. Tout ce qui reste sous `common` rejoint
        //    `Steam` : les jeux Lapinux comme les dossiers orphelins d'une
        //    ancienne installation, sans jamais écraser une destination.
        let legacy_common = legacy.join("common");
        if let Ok(entries) = std::fs::read_dir(&legacy_common) {
            for entry in entries.flatten() {
                let from = entry.path();
                if !from.is_dir() {
                    continue;
                }
                let Some(name) = from.file_name() else {
                    continue;
                };
                let to = steam.join(name);
                if !to.exists() {
                    move_directory(&from, &to);
                }
            }
        }

        // 2. Les manifestes, une fois les jeux en place.
        if !manifests.is_empty() {
            let _ = std::fs::create_dir_all(&steam);
        }
        for (appid, _, path) in &manifests {
            let to = steam.join(format!("appmanifest_{appid}.acf"));
            if !to.exists() {
                let _ = std::fs::rename(path, &to);
            }
        }

        // 3. Les préfixes : ceux de la bibliothèque sont renommés sur place ;
        //    ceux des données Lapinux suivent une seule fois, avec une copie
        //    seulement s'ils vivent sur un autre disque.
        for source in [
            legacy.join("compatdata"),
            managed.join("compatdata"),
            self.paths.data_dir.join("prefixes"),
        ] {
            let Ok(entries) = std::fs::read_dir(&source) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let Some(appid) = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .and_then(|name| name.parse::<u32>().ok())
                else {
                    continue;
                };
                if !managed_ids.contains(&appid) {
                    continue;
                }
                let Some(name) = names.get(&appid) else {
                    continue;
                };
                let to = compat.join(lapinux_proton::prefix_name(name, appid));
                if !to.exists() {
                    move_directory(&path, &to);
                }
            }
        }

        // 4. Le dossier `steamapps` vidé de ce que Lapinux y avait mis ne
        //    reste pas : la bibliothèque ne montre plus que `Steam`,
        //    `CompatData` et les rayons des boutiques.
        let _ = std::fs::remove_dir(&legacy_common);
        let _ = std::fs::remove_dir(legacy.join("compatdata"));
        let _ = std::fs::remove_dir(managed.join("compatdata"));
        let _ = std::fs::remove_dir(&legacy);
        let _ = std::fs::create_dir_all(&compat);
        let _ = std::fs::write(&marker, b"1");
    }

    pub fn scan_protons(&self) -> Result<Vec<ProtonInfo>> {
        let roots = lapinux_core::paths::steam_roots();
        let extra = vec![
            self.paths.proton_dir.clone(),
            lapinux_core::paths::home_dir().join(".local/share/umu/compatibilitytools"),
        ];
        let protons = lapinux_proton::scan(&roots, &extra);
        if let Ok(mut cache) = self.protons.lock() {
            *cache = protons.clone();
        }
        Ok(protons)
    }

    pub fn protons_cached(&self) -> Vec<ProtonInfo> {
        if let Ok(cache) = self.protons.lock() {
            if !cache.is_empty() {
                return cache.clone();
            }
        }
        self.scan_protons().unwrap_or_default()
    }

    pub fn session_info(&self) -> Option<SessionInfo> {
        self.session
            .lock()
            .ok()
            .and_then(|s| s.as_ref().map(|state| state.info.clone()))
    }

    /// Parties terminées, de la plus récente à la plus ancienne : une lecture
    /// locale, sans réseau.
    pub fn session_history(&self) -> Result<Envelope> {
        let mut sessions = self
            .store
            .get_json::<Vec<lapinux_core::model::PlaySession>>("sessions")
            .map_err(|error| lapinux_core::Error::Other(error.to_string()))?
            .unwrap_or_default();
        sessions.reverse();
        Ok(Envelope::ok(&sessions))
    }

    /// Retire une partie de l'historique. Le temps de jeu du jeu et le total
    /// de la session diminuent d'autant : sans cela, la même partie
    /// continuerait de peser dans « Les plus joués » après sa suppression.
    pub fn delete_session(&self, appid: u32, started: i64) -> Result<Envelope> {
        let mut sessions = self
            .store
            .get_json::<Vec<lapinux_core::model::PlaySession>>("sessions")
            .map_err(|error| lapinux_core::Error::Other(error.to_string()))?
            .unwrap_or_default();
        let before = sessions.len();
        let mut seconds = 0u64;
        sessions.retain(|session| {
            if session.appid == appid && session.started == started {
                seconds += session.seconds;
                false
            } else {
                true
            }
        });
        let deleted = before - sessions.len();
        if deleted > 0 {
            self.store
                .set_json("sessions", &sessions)
                .map_err(|error| lapinux_core::Error::Other(error.to_string()))?;
            self.forget_playtime(&[(appid, seconds)]);
        }
        Ok(Envelope::ok(&serde_json::json!({ "deleted": deleted })))
    }

    /// Vide une journée entière de l'historique : toutes les parties dont la
    /// fin tombe dans ce jour.
    pub fn clear_session_day(&self, day: i64) -> Result<Envelope> {
        let mut sessions = self
            .store
            .get_json::<Vec<lapinux_core::model::PlaySession>>("sessions")
            .map_err(|error| lapinux_core::Error::Other(error.to_string()))?
            .unwrap_or_default();
        let before = sessions.len();
        let mut removed: HashMap<u32, u64> = HashMap::new();
        sessions.retain(|session| {
            let ended = if session.ended > 0 {
                session.ended
            } else {
                session.started
            };
            if lapinux_core::util::day_start(ended) == day {
                *removed.entry(session.appid).or_default() += session.seconds;
                false
            } else {
                true
            }
        });
        let deleted = before - sessions.len();
        if deleted > 0 {
            self.store
                .set_json("sessions", &sessions)
                .map_err(|error| lapinux_core::Error::Other(error.to_string()))?;
            let removed: Vec<(u32, u64)> = removed.into_iter().collect();
            self.forget_playtime(&removed);
        }
        Ok(Envelope::ok(&serde_json::json!({ "deleted": deleted })))
    }

    /// Retire des secondes au temps de jeu cumulé des jeux et au total, sans
    /// jamais passer sous zéro.
    fn forget_playtime(&self, removed: &[(u32, u64)]) {
        for (appid, seconds) in removed {
            if *seconds == 0 {
                continue;
            }
            if let Ok(Some(mut game)) = self.store.game(*appid) {
                game.playtime_secs = game.playtime_secs.saturating_sub(*seconds);
                let _ = self.store.save_game(&game);
            }
        }
        if let Ok(mut stored) = self
            .store
            .get_json::<lapinux_core::model::Stats>("stats")
        {
            let mut stats = stored.take().unwrap_or_default();
            let total: u64 = removed.iter().map(|(_, seconds)| *seconds).sum();
            stats.total_seconds = stats.total_seconds.saturating_sub(total);
            let _ = self.store.set_json("stats", &stats);
        }
    }

    /// Remet la fenêtre de mesure à zéro après une pause : le temps d'arrêt
    /// ne doit pas devenir le « débit le plus bas » de la reprise.
    pub(crate) fn reset_rate_window(&self, id: &str, network: u64) {
        if let Ok(mut rates) = self.rates.lock() {
            rates
                .entry(id.to_string())
                .or_default()
                .reset_window(network);
        }
    }

    pub fn update_job(&self, id: &str, update: impl FnOnce(&mut Job)) {
        let mut terminal = false;
        if let Ok(mut jobs) = self.jobs.lock() {
            if let Some(job) = jobs.iter_mut().find(|j| j.id == id) {
                update(job);
                terminal = matches!(
                    job.state,
                    JobState::Done
                        | JobState::Failed
                        | JobState::Cancelled
                        | JobState::Interrupted
                );
                if let Ok(mut trackers) = self.rates.lock() {
                    let now = Instant::now();
                    let tracker = trackers.entry(id.to_string()).or_default();
                    if job.state == JobState::Running {
                        tracker.sample(now, job.network);
                    } else if !terminal {
                        tracker.reset_window(job.network);
                    }
                    if terminal {
                        tracker.finish(now, job.network);
                        let finished = job
                            .finished
                            .unwrap_or_else(lapinux_core::util::now_unix);
                        let elapsed = finished.saturating_sub(job.started).max(0) as u64;
                        job.report = Some(tracker.report(job.network, job.written, elapsed));
                        trackers.remove(id);
                    }
                }
            }
        }
        if terminal {
            self.persist_jobs(true);
        } else {
            self.persist_jobs(false);
        }
    }
}

pub fn run(paths: LapinuxPaths, ready: Option<std::sync::mpsc::Sender<()>>) -> Result<()> {
    // Une session précédente a pu mourir avec l'overlay ouvert : les panneaux
    // Plasma sont alors restés en autohide. Les rendre avant d'ouvrir quoi
    // que ce soit, sans bloquer si Plasma n'est pas encore joignable.
    lapinux_input::panels::restore_stale(&paths.data_dir);
    let shared = Arc::new(Shared::new(&paths)?);
    shared.persist_jobs(true);
    // Les versions précédentes écrivaient un journal par partie et par
    // overlay : ils sont retirés pour que la liste des journaux reste courte.
    crate::logs::prune_legacy(&paths);

    if paths.socket.exists() {
        let alive = ipc::send_request_timeout(&paths.socket, &Req::Ping, Duration::from_millis(300))
            .is_ok();
        if alive {
            return Err(lapinux_core::Error::Ipc(
                "un daemon écoute déjà sur ce socket".into(),
            ));
        }
        let _ = std::fs::remove_file(&paths.socket);
    }

    // Les jeux de boutique posés avant le rangement par launcher rejoignent
    // leur rayon : un simple renommage de dossier, sans relire ni recopier un
    // octet, et la fiche du jeu suit.
    shared.migrate_store_shelves();
    let _ = shared.rescan_library();
    let _ = shared.scan_protons();
    // Une manette branchée doit l'emporter sur sa liaison sans fil, sur
    // toutes les pages : le veilleur vit avec le daemon.
    lapinux_input::watch_wired_takeover();
    // Le compte reste visible de ses amis tant que l'application tourne : la
    // session sociale naît avec le daemon, sans bloquer son démarrage.
    shared.start_social_in_background();
    // La présence Discord suit ce que l'interface affiche ; elle attend un
    // client Discord et un identifiant, sans retarder le démarrage.
    shared.start_discord_in_background();
    // Le catalogue qui associe un jeu à son application Discord officielle
    // pèse quelques mégaoctets : il se télécharge derrière, jamais dans le
    // chemin d'un lancement.
    shared.start_discord_apps_in_background();
    // Le catalogue de configurations tient en quelques kilo-octets : sa
    // relecture suit le démarrage sans le retarder.
    shared.start_configurations_in_background();
    // Les fiches des jeux récemment joués se préparent derrière, et le dossier
    // de fiches se nettoie : la première ouverture relira le disque au lieu
    // d'attendre Steam, et le cache ne grossit plus sans fin.
    shared.start_info_preload();
    // Et ceux des jeux de boutique ouverts récemment : leur fiche est déjà sur
    // disque, la relire derrière garde le premier clic instantané et à jour.
    shared.start_store_preload();

    // Une surface HUD laissée par un daemon précédent resterait dessinée sur
    // le bureau, sans jeu pour la refermer ni overlay pour la couper : on
    // l'arrête avant d'ouvrir le socket.
    Shared::reap_stale_hud();

    let listener = bind_socket(&paths)?;
    // Seul le daemon qui possède le socket sert le menu : un second daemon
    // (autre LAPINUX_HOME, test…) ne doit pas prendre le nom D-Bus du premier.
    serve_quick_menu(Arc::clone(&shared));
    // Le tray appartient au daemon : il survit à la fermeture de l'interface,
    // la relance, et lance les jeux récents sans elle.
    tray::start(Arc::clone(&shared));
    if let Some(ready) = ready {
        let _ = ready.send(());
    }
    tracing::info!("daemon à l'écoute sur {}", paths.socket.display());

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let Some(slot) = CONNECTIONS.take() else {
                    // Trop de requêtes à la fois : répondre tout de suite coûte
                    // une écriture, là où ouvrir un fil de plus en aurait laissé
                    // un de plus pour chaque appelant impatient.
                    tracing::warn!(
                        "connexion IPC refusée : {} requêtes en cours",
                        CONNECTIONS.live()
                    );
                    refuse(stream);
                    continue;
                };
                let shared = Arc::clone(&shared);
                std::thread::spawn(move || {
                    // Le jeton rend sa place même si le traitement panique.
                    let _slot = slot;
                    match handle_connection(shared, stream) {
                        Ok(()) => {}
                        // Un client qui disparaît avant d'avoir fini sa requête
                        // (délai dépassé, fenêtre fermée) n'est pas une erreur :
                        // le fil et le socket sont déjà rendus.
                        Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => {}
                        Err(err) => tracing::warn!("connexion IPC : {err}"),
                    }
                });
            }
            Err(err) => tracing::warn!("accept : {err}"),
        }
    }
    Ok(())
}

/// Connexions servies en parallèle.
///
/// Chaque requête occupe un fil et un socket jusqu'à sa réponse. Steam peut
/// traîner plusieurs minutes sur une fiche : sans cette borne, une interface
/// qui redemande pendant ce temps empilait un fil et un socket de plus à chaque
/// essai, et le daemon en gardait des milliers.
const MAX_CONNECTIONS: usize = 64;

static CONNECTIONS: ConnectionBudget = ConnectionBudget::new(MAX_CONNECTIONS);

/// Quota de connexions simultanées. Une place se prend avec `take` et se rend
/// quand le jeton tombe — donc à la fin du fil, ou pendant un déroulement de
/// pile si le traitement panique.
struct ConnectionBudget {
    live: AtomicUsize,
    limit: usize,
}

/// Place réservée dans le quota, rendue au `Drop`.
struct ConnectionSlot<'a> {
    budget: &'a ConnectionBudget,
}

impl ConnectionBudget {
    const fn new(limit: usize) -> Self {
        Self {
            live: AtomicUsize::new(0),
            limit,
        }
    }

    /// `None` quand toutes les places sont prises.
    fn take(&self) -> Option<ConnectionSlot<'_>> {
        if self.live.fetch_add(1, Ordering::AcqRel) >= self.limit {
            self.live.fetch_sub(1, Ordering::AcqRel);
            return None;
        }
        Some(ConnectionSlot { budget: self })
    }

    fn live(&self) -> usize {
        self.live.load(Ordering::Acquire)
    }
}

impl Drop for ConnectionSlot<'_> {
    fn drop(&mut self) {
        self.budget.live.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Répond « occupé » puis referme : l'appelant reçoit une erreur claire sans
/// attendre son propre délai, et aucun socket ne reste ouvert.
fn refuse(mut stream: UnixStream) {
    // La requête est lue avant de refuser : le client reçoit le message au lieu
    // d'une écriture coupée en pleine trame. Le délai reste court parce que ce
    // refus s'exécute sur le fil d'acceptation : un client muet ne doit pas
    // retenir les connexions suivantes.
    stream.set_read_timeout(Some(Duration::from_millis(300))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(2))).ok();
    let _ = ipc::read_frame(&mut stream);
    let envelope = Envelope::err("daemon occupé : trop de requêtes à la fois, réessaie dans un instant");
    let body = serde_json::to_vec(&envelope).unwrap_or_default();
    let _ = ipc::write_frame(&mut stream, &body);
    let _ = stream.shutdown(std::net::Shutdown::Both);
}

/// Travail demandé par KWin, exécuté hors du fil D-Bus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuickTask {
    /// Le raccourci du menu rapide a été pressé.
    Toggle,
    /// Une fenêtre vient d'apparaître, avec le pid qui la porte.
    GameWindow(u32),
    /// Une fenêtre vient de disparaître, avec le pid qui la portait.
    GameWindowGone(u32),
}

/// Sert le service D-Bus que le script KWin appelle pendant une session.
///
/// KWin appelle ces méthodes **en synchrone** : il attend notre réponse avant
/// de reprendre la main. Or ouvrir le menu rapide ou cadrer le jeu remonte à
/// KWin (chargement d'un script, relevé de géométrie, capture). Appeler KWin
/// depuis le fil D-Bus qui lui répond l'interbloque pour de bon — le
/// compositeur attend notre retour, nous attendons le sien — et plus rien ne
/// fonctionne jusqu'au redémarrage du daemon. Les deux méthodes ne font donc
/// ici que déposer une tâche, qu'un fil unique exécute dans l'ordre.
fn serve_quick_menu(shared: Arc<Shared>) {
    let (tasks, queue) = std::sync::mpsc::channel::<QuickTask>();
    {
        let shared = Arc::clone(&shared);
        std::thread::spawn(move || {
            while let Ok(task) = queue.recv() {
                match task {
                    QuickTask::Toggle => {
                        let _ = shared.toggle_overlay();
                    }
                    QuickTask::GameWindow(pid) => shared.note_game_window(pid),
                    QuickTask::GameWindowGone(pid) => shared.note_game_window_gone(pid),
                }
            }
        });
    }
    let toggle = {
        let tasks = tasks.clone();
        move || {
            // Jamais bloquant : KWin ne doit pas attendre que le travail
            // précédent soit fini pour récupérer sa réponse.
            let _ = tasks.send(QuickTask::Toggle);
        }
    };
    let game_window = {
        let tasks = tasks.clone();
        move |pid: u32| {
            let _ = tasks.send(QuickTask::GameWindow(pid));
        }
    };
    let game_window_gone = move |pid: u32| {
        let _ = tasks.send(QuickTask::GameWindowGone(pid));
    };
    if let Err(err) =
        lapinux_input::shortcut::serve(toggle, game_window, game_window_gone)
    {
        tracing::warn!("menu rapide : {err}");
    }
}

fn bind_socket(paths: &LapinuxPaths) -> Result<UnixListener> {
    if let Some(parent) = paths.socket.parent() {
        std::fs::create_dir_all(parent).map_err(lapinux_core::Error::Io)?;
    }
    let listener = UnixListener::bind(&paths.socket).map_err(|e| {
        lapinux_core::Error::Ipc(format!("bind {} : {e}", paths.socket.display()))
    })?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&paths.socket, std::fs::Permissions::from_mode(0o600)).ok();
    Ok(listener)
}

fn handle_connection(shared: Arc<Shared>, mut stream: UnixStream) -> std::io::Result<()> {
    // Un pair qui n'envoie rien, ou qui disparaît pendant qu'on lui répond, ne
    // doit pas retenir le fil : les deux délais bornent l'attente.
    stream.set_read_timeout(Some(Duration::from_secs(30))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(30))).ok();
    let result = serve_connection(&shared, &mut stream);
    // Fermeture explicite : le socket ne survit pas à la requête, même quand
    // Steam a refusé ou traîné.
    let _ = stream.shutdown(std::net::Shutdown::Both);
    result
}

fn serve_connection(shared: &Arc<Shared>, stream: &mut UnixStream) -> std::io::Result<()> {
    let payload = ipc::read_frame(stream)?;
    let request: Req = match serde_json::from_slice(&payload) {
        Ok(request) => request,
        Err(err) => {
            let envelope = Envelope::err(format!("requête invalide : {err}"));
            let body = serde_json::to_vec(&envelope).unwrap_or_default();
            ipc::write_frame(stream, &body)?;
            return Ok(());
        }
    };
    let envelope = dispatch(shared, request);
    let body = serde_json::to_vec(&envelope).unwrap_or_default();
    ipc::write_frame(stream, &body)
}

/// Trace les actions dans le journal du daemon. Les lectures répétées (statut,
/// listes, sondages) restent silencieuses pour ne pas noyer le journal, et
/// aucun secret — mot de passe, jeton, code Steam Guard — n'y est écrit.
fn log_action(request: &Req) {
    match request {
        Req::Scan => tracing::info!("scan de la bibliothèque demandé"),
        Req::SaveSettings { appid, .. } => tracing::info!("réglages du jeu {appid} enregistrés"),
        Req::Launch { appid, dry_run } => {
            if *dry_run {
                tracing::info!("commande de lancement calculée pour {appid}");
            } else {
                tracing::info!("lancement du jeu {appid}");
            }
        }
        Req::Stop => tracing::info!("arrêt de la session demandé"),
        Req::InstallGame {
            appid,
            dlc,
            platform,
        } => tracing::info!(
            "installation de {appid} demandée ({} contenu(s), version {})",
            dlc.len(),
            platform.as_deref().unwrap_or("automatique")
        ),
        Req::UninstallGame { appid } => tracing::info!("désinstallation de {appid} demandée"),
        Req::CancelJob { id } => tracing::info!("tâche {id} annulée"),
        Req::PauseJob { id, paused } => {
            tracing::info!("tâche {id} {}", if *paused { "mise en pause" } else { "reprise" })
        }
        Req::ClearHistory => tracing::info!("historique des téléchargements effacé"),
        Req::DeleteJob { id } => tracing::info!("tâche {id} retirée de l'historique"),
        Req::ReorderJobs { ids } => {
            tracing::info!("ordre des téléchargements modifié ({} tâches)", ids.len())
        }
        Req::InstallProton { asset, .. } => tracing::info!("installation de Proton {asset}"),
        // Journalisé avant la réponse : le refus éventuel vient du daemon, et
        // la ligne ne doit pas annoncer une suppression qui n'a pas eu lieu.
        Req::RemoveProton { name } => tracing::info!("suppression du Proton {name} demandée"),
        Req::OverlayToggle => tracing::info!("menu rapide basculé"),
        Req::OverlayAction { action } => tracing::info!("action du menu rapide : {action}"),
        Req::RecordingPause { paused } => {
            tracing::info!("enregistrement {}", if *paused { "en pause" } else { "repris" })
        }
        Req::SetConfig { .. } => tracing::info!("configuration enregistrée"),
        Req::SteamCatalog { refresh: true } => tracing::info!("synchronisation du catalogue Steam"),
        Req::SteamCatalogPatch { .. } => tracing::info!("jeu d'une clé ajouté au catalogue"),
        Req::GameInfo { appid } => tracing::info!("fiche Steam du jeu {appid} demandée"),
        Req::GameConfigurations { appid } => {
            tracing::debug!("configurations du jeu {appid} demandées")
        }
        Req::InstallConfiguration { appid, id } => {
            tracing::info!("installation de la configuration {id} sur {appid}")
        }
        Req::RemoveConfiguration { appid, id } => {
            tracing::info!("retrait de la configuration {id} de {appid}")
        }
        Req::GameAchievements { appid } => {
            tracing::info!("succès du jeu {appid} demandés (session Steam)")
        }
        Req::AchievementToast { appid, .. } => {
            tracing::info!("bandeau de succès du jeu {appid} demandé")
        }
        Req::SteamCloud { appid } => {
            tracing::info!("sauvegardes cloud du jeu {appid} demandées")
        }
        Req::CloudRestore { appid } => {
            tracing::info!("récupération des sauvegardes cloud du jeu {appid}")
        }
        Req::CloudUpload { appid } => {
            tracing::info!("envoi des sauvegardes cloud du jeu {appid}")
        }
        Req::SaveSnapshots { appid } => {
            tracing::info!("copies locales des sauvegardes du jeu {appid} demandées")
        }
        Req::BackupSaves { appid, slot } => tracing::info!(
            "copie locale des sauvegardes du jeu {appid} demandée (emplacement {})",
            (*slot)
                .map(|slot| slot.to_string())
                .unwrap_or_else(|| "libre".into())
        ),
        Req::RestoreSaves { appid, snapshot } => {
            tracing::info!("restauration de la copie {snapshot} du jeu {appid}")
        }
        Req::ExportSaves { appid, dest } => {
            tracing::info!("export des sauvegardes du jeu {appid} vers {dest}")
        }
        Req::DeleteSaveSnapshot { appid, snapshot } => {
            tracing::info!("suppression de la copie {snapshot} du jeu {appid}")
        }
        Req::SavesOverview => tracing::info!("bilan des copies locales demandé"),
        Req::SteamLogin { username, .. } => {
            tracing::info!("connexion Steam demandée pour {username}")
        }
        Req::SteamLoginQr => tracing::info!("connexion Steam par QR demandée"),
        Req::SteamLoginGuard { .. } => tracing::info!("code Steam Guard transmis"),
        Req::SteamLoginCancel => tracing::info!("connexion Steam annulée"),
        Req::SteamForgetToken => tracing::info!("jeton Lapinux oublié"),
        Req::SteamDisconnect => tracing::info!("déconnexion du compte Steam demandée"),
        Req::SteamTokenCheck => tracing::info!("vérification du jeton Lapinux demandée"),
        Req::Stores { refresh, .. } => tracing::info!(
            "comptes des boutiques demandés{}",
            if *refresh { " (relecture)" } else { "" }
        ),
        Req::StoreLogin { store, .. } => tracing::info!("connexion {} demandée", store.label()),
        Req::StoreLogout { store } => tracing::info!("compte {} retiré", store.label()),
        Req::StoreInstall { store, store_id, .. } => {
            tracing::info!("installation {store_id} demandée depuis {}", store.label())
        }
        Req::StoreDetails { appid } => {
            tracing::info!("fiche de boutique du jeu {appid} demandée")
        }
        Req::StoreFreeGames => tracing::info!("jeux gratuits demandés"),
        Req::StoreClaimFreeGames { ids } => {
            tracing::info!("récupération de {} offre(s) gratuite(s) demandée", ids.len())
        }
        Req::DiscordView { section, .. } => {
            tracing::debug!("vue publiée sur Discord : {section}")
        }
        Req::DiscordPresence { active } => tracing::info!(
            "présence Discord {}",
            if *active { "réactivée" } else { "coupée" }
        ),
        Req::SteamSocial { refresh: true } => tracing::info!("liste d'amis redemandée"),
        Req::SteamSocialPresence { presence } => {
            tracing::info!("présence Steam : {}", presence.label())
        }
        Req::SteamSocialRedeemKey { .. } => tracing::info!("activation d'une clé produit Steam"),
        Req::SteamSocialMessage { steam_id, .. } => {
            tracing::info!("message Steam demandé pour {steam_id}")
        }
        Req::StoreSize { appid } => {
            tracing::info!("poids du jeu de boutique {appid} demandé")
        }
        Req::StoreDlcSizes { appid } => {
            tracing::info!("poids des contenus du jeu de boutique {appid} demandé")
        }
        Req::StoreUpdates => tracing::info!("recherche des mises à jour de boutique"),
        Req::ClearCaches => tracing::info!("caches vidés depuis les réglages"),
        Req::Shutdown => tracing::info!("arrêt du daemon demandé"),
        _ => {}
    }
}

/// Ce qu'une boutique hors Steam n'expose pas.
///
/// Le daemon répond franchement plutôt que de laisser la fiche interroger
/// Steam pour un appid Lapinux qui n'y désigne aucun jeu.
fn store_without(shared: &Shared, appid: u32, feature: &str) -> Option<Envelope> {
    let game = shared.store.game(appid).ok().flatten()?;
    if game.store == lapinux_core::stores::Store::Steam {
        return None;
    }
    Some(Envelope::err(format!(
        "{feature} : {} ne l'expose pas pour ses jeux",
        game.store.label()
    )))
}

/// Boutique d'un jeu connu, ou `None` quand la base ne le connaît pas (un jeu
/// du catalogue Steam pas encore installé, par exemple) : l'appelant garde
/// alors le chemin Steam.
fn store_kind(shared: &Shared, appid: u32) -> Option<lapinux_core::stores::Store> {
    if let Ok(Some(game)) = shared.store.game(appid) {
        return Some(game.store);
    }
    // Possédé mais pas installé : sa boutique se lit dans la bibliothèque du
    // compte, que le daemon garde en cache. Sans ce repli, les succès et le
    // cloud d'un jeu de boutique non installé partaient vers Steam.
    shared
        .cached_store_entry(appid)
        .map(|entry| entry.store)
}

fn dispatch(shared: &Arc<Shared>, request: Req) -> Envelope {
    log_action(&request);
    let result: Result<Envelope> = match request {
        Req::Ping => Ok(Envelope::ok(&shared.build)),
        Req::Scan => shared.rescan_library().map(|games| Envelope::ok(&games)),
        Req::Games => shared
            .store
            .load_games()
            .map(|games| Envelope::ok(&games))
            .map_err(|e| lapinux_core::Error::Other(e.to_string())),
        Req::Game { appid } => shared
            .store
            .game(appid)
            .map(|game| Envelope::ok(&game))
            .map_err(|e| lapinux_core::Error::Other(e.to_string())),
        Req::SaveSettings { appid, settings } => shared.save_settings(appid, settings),
        Req::Launch { appid, dry_run } => shared.launch(appid, dry_run),
        Req::Stop => shared.stop_session(),
        Req::Session => Ok(Envelope::ok(&shared.session_info())),
        Req::Recording { start } => shared.recording(start),
        Req::RecordingPause { paused } => shared.recording_pause(paused),
        Req::RecordingStatus => Ok(Envelope::ok(&shared.recording_state())),
        Req::GameVolume { volume } => shared.game_volume(volume),
        Req::System => Ok(Envelope::ok(&shared.sample_system())),
        Req::FocusGame { appid } => shared.focus_game(appid),
        Req::SessionHistory => shared.session_history(),
        Req::DeleteSession { appid, started } => shared.delete_session(appid, started),
        Req::ClearSessionDay { day } => shared.clear_session_day(day),
        Req::Jobs => Ok(Envelope::ok(
            &shared.jobs.lock().map(|j| j.clone()).unwrap_or_default(),
        )),
        Req::InstallGame {
            appid,
            dlc,
            platform,
        } => shared.install_game(appid, dlc, platform),
        Req::UninstallGame { appid } => shared.uninstall_game(appid),
        Req::CancelJob { id } => shared.cancel_job(&id),
        Req::PauseJob { id, paused } => shared.pause_job(&id, paused),
        Req::ClearHistory => shared.clear_history(),
        Req::DeleteJob { id } => shared.delete_job(&id),
        Req::ReorderJobs { ids } => match shared.reorder_jobs(ids) {
            Ok(envelope) => {
                // L'ordre décide de la carte qui télécharge : en mode « un seul
                // jeu à la fois », la file applique tout de suite la priorité.
                shared.apply_install_priority();
                Ok(envelope)
            }
            Err(err) => Err(err),
        },
        Req::Protons => shared.scan_protons().map(|p| Envelope::ok(&p)),
        Req::ProtonReleases => single_releases(),
        Req::InstallProton {
            repo,
            asset,
            checksum_url,
        } => shared.install_proton(repo, asset, checksum_url),
        Req::RemoveProton { name } => shared.remove_proton(&name),
        Req::Doctor => Ok(Envelope::ok(&doctor::report(shared))),
        Req::OverlayToggle => shared.toggle_overlay(),
        Req::OverlayAction { action } => shared.overlay_action(&action),
        Req::Devices => Ok(Envelope::ok(&lapinux_input::device::list_cached())),
        Req::Profiles => Ok(Envelope::ok(&lapinux_input::list_profiles(
            &shared.paths.data_dir,
        ))),
        Req::Config => Ok(Envelope::ok(&shared.config_snapshot())),
        Req::SetConfig { config } => match shared.save_config(config) {
            Ok(envelope) => {
                // Passer en mode simultané peut libérer toute la file d'un
                // coup ; le passage en file ne coupe pas les tâches en cours.
                if envelope.ok {
                    shared.pump_install_queue();
                }
                Ok(envelope)
            }
            Err(err) => Err(err),
        },
        Req::SteamStatus => Ok(Envelope::ok(&shared.steam_status())),
        Req::SteamCatalog { refresh } => shared.steam_catalog(refresh),
        Req::SteamCatalogPatch { appid, name } => shared.steam_catalog_patch(appid, name),
        Req::GameInfo { appid } => match store_without(shared, appid, "Fiche Steam") {
            Some(envelope) => Ok(envelope),
            None => shared.game_info(appid),
        },
        Req::GameConfigurations { appid } => {
            // Le catalogue se relit derrière la réponse : la fiche ne dépend
            // jamais du réseau.
            shared.start_configurations_in_background();
            shared.game_configurations(appid)
        }
        Req::InstallConfiguration { appid, id } => shared.install_configuration(appid, &id),
        Req::RemoveConfiguration { appid, id } => shared.remove_configuration(appid, &id),
        // Succès et sauvegardes savent aussi lire chez Epic et GOG : seule la
        // fiche Steam (PICS, boutique) reste propre à Steam.
        Req::GameAchievements { appid } => match store_kind(shared, appid) {
            Some(store) if store != lapinux_core::stores::Store::Steam => {
                shared.store_achievements(appid)
            }
            _ => shared.game_achievements(appid),
        },
        Req::AchievementToast { appid, api_name } => shared
            .test_achievement_toast(appid, &api_name)
            .map(|value| Envelope::ok(&value)),
        Req::SteamCloud { appid } => match store_kind(shared, appid) {
            Some(store) if store != lapinux_core::stores::Store::Steam => {
                shared.store_cloud(appid)
            }
            _ => shared.steam_cloud(appid),
        },
        Req::CloudRestore { appid } => match store_kind(shared, appid) {
            Some(store) if store != lapinux_core::stores::Store::Steam => {
                shared.store_cloud_restore(appid)
            }
            _ => shared.cloud_restore(appid),
        },
        Req::SaveSnapshots { appid } => {
            let config = shared.config_snapshot();
            let copies = match shared.store.game(appid) {
                Ok(Some(game)) => crate::saves::list(&game, &shared.paths, &config),
                _ => Vec::new(),
            };
            Ok(Envelope::ok(&copies))
        }
        Req::SaveHistory { appid } => {
            let config = shared.config_snapshot();
            let copies = match shared.store.game(appid) {
                Ok(Some(game)) => crate::saves::list(&game, &shared.paths, &config),
                _ => Vec::new(),
            };
            Ok(Envelope::ok(&crate::saves::history(
                &shared.store,
                appid,
                &copies,
            )))
        }
        Req::BackupSaves { appid, slot } => match shared.store.game(appid) {
            Ok(Some(game)) if crate::saves::source(&game, &shared.paths).is_none() => Ok(
                Envelope::err("aucun dossier de sauvegarde connu pour ce jeu"),
            ),
            Ok(Some(game)) => {
                let config = shared.config_snapshot();
                match crate::saves::backup_to_slot(&game, &shared.paths, &config, slot) {
                    Ok(Some(snapshot)) => {
                        crate::saves::note(
                            &shared.store,
                            appid,
                            SaveEvent {
                                at: lapinux_core::util::now_unix(),
                                kind: SaveEventKind::Backup,
                                snapshot: snapshot.id.clone(),
                                files: snapshot.files,
                                bytes: snapshot.bytes,
                                detail: snapshot.source.clone(),
                                auto: false,
                                imported: false,
                            },
                        );
                        Ok(Envelope::ok(&serde_json::json!({
                            "snapshot": snapshot
                        })))
                    }
                    Ok(None) => Ok(Envelope::ok(&serde_json::json!({ "unchanged": true }))),
                    Err(error) => Ok(Envelope::err(error)),
                }
            }
            Ok(None) => Ok(Envelope::err("jeu inconnu")),
            Err(err) => Err(err),
        },
        Req::RestoreSaves { appid, snapshot } => match shared.store.game(appid) {
            // Le jeu relit ses sauvegardes au démarrage et les réécrit en
            // quittant : charger pendant une partie serait perdu.
            Ok(Some(_)) if shared.session_info().map(|info| info.appid) == Some(appid) => Ok(
                Envelope::err("fermez le jeu avant de charger une sauvegarde"),
            ),
            Ok(Some(game)) => {
                let config = shared.config_snapshot();
                match crate::saves::restore(&game, &shared.paths, &config, &snapshot) {
                    Ok(files) => {
                        crate::saves::note(
                            &shared.store,
                            appid,
                            SaveEvent {
                                at: lapinux_core::util::now_unix(),
                                kind: SaveEventKind::Restore,
                                snapshot: snapshot.clone(),
                                files,
                                ..Default::default()
                            },
                        );
                        Ok(Envelope::ok(&serde_json::json!({ "files": files })))
                    }
                    Err(error) => Ok(Envelope::err(error)),
                }
            }
            Ok(None) => Ok(Envelope::err("jeu inconnu")),
            Err(err) => Err(err),
        },
        Req::DeleteSaveSnapshot { appid, snapshot } => {
            let config = shared.config_snapshot();
            let removed = match shared.store.game(appid) {
                Ok(Some(game)) => crate::saves::remove(&game, &shared.paths, &config, &snapshot),
                Ok(None) => Err("jeu inconnu".to_string()),
                Err(err) => Err(err.to_string()),
            };
            match removed {
                Ok(copy) => {
                    crate::saves::note(
                        &shared.store,
                        appid,
                        SaveEvent {
                            at: lapinux_core::util::now_unix(),
                            kind: SaveEventKind::Delete,
                            snapshot: copy.id.clone(),
                            files: copy.files,
                            bytes: copy.bytes,
                            detail: copy.source.clone(),
                            ..Default::default()
                        },
                    );
                    Ok(Envelope::ok(&serde_json::json!({ "deleted": copy.id })))
                }
                Err(error) => Ok(Envelope::err(error)),
            }
        }
        Req::ExportSaves { appid, dest } => {
            let config = shared.config_snapshot();
            let exported = match shared.store.game(appid) {
                Ok(Some(game)) => {
                    let result = crate::saves::export(&game, &shared.paths, &config, &dest);
                    result.map(|path| {
                        (
                            path,
                            crate::saves::list(&game, &shared.paths, &config),
                        )
                    })
                }
                Ok(None) => Err("jeu inconnu".to_string()),
                Err(err) => Err(err.to_string()),
            };
            match exported {
                Ok((path, copies)) => {
                    crate::saves::note(
                        &shared.store,
                        appid,
                        SaveEvent {
                            at: lapinux_core::util::now_unix(),
                            kind: SaveEventKind::Export,
                            files: copies.iter().map(|copy| copy.files).sum(),
                            bytes: copies.iter().map(|copy| copy.bytes).sum(),
                            detail: path.display().to_string(),
                            ..Default::default()
                        },
                    );
                    Ok(Envelope::ok(&serde_json::json!({
                        "path": path.display().to_string()
                    })))
                }
                Err(error) => Ok(Envelope::err(error)),
            }
        }
        Req::SavesOverview => {
            let config = shared.config_snapshot();
            Ok(Envelope::ok(&crate::saves::overview(&shared.paths, &config)))
        }
        Req::CloudUpload { appid } => match store_kind(shared, appid) {
            Some(store) if store != lapinux_core::stores::Store::Steam => {
                shared.store_cloud_upload(appid)
            }
            _ => shared.cloud_upload(appid),
        },
        Req::SteamLogin {
            username,
            password,
            guard,
            token,
        } => shared.steam_login(&username, &password, guard.as_deref(), token.as_deref()),
        Req::SteamLoginQr => shared.steam_login_qr(),
        Req::SteamLoginGuard { code } => shared.steam_login_guard(&code),
        Req::SteamLoginCancel => shared.steam_login_cancel(),
        Req::SteamForgetToken => shared.steam_forget_token(),
        Req::SteamDisconnect => shared.steam_disconnect(),
        Req::SteamTokenCheck => shared.steam_token_check(),
        Req::Stores { refresh, only } => shared.stores_view(refresh, &only),
        Req::StoreSize { appid } => shared.store_size(appid),
        Req::StoreDlcSizes { appid } => shared.store_dlc_sizes(appid),
        Req::StoreUpdates => shared.store_updates(),
        Req::StoreLogin {
            store,
            token,
            username,
        } => shared.store_login(store, token, username),
        Req::StoreLogout { store } => shared.store_logout(store),
        Req::StoreInstall {
            store,
            store_id,
            platform,
        } => shared.store_install(store, store_id, platform),
        Req::StoreDetails { appid } => shared.store_details(appid),
        Req::StoreFreeGames => shared.store_free_games(),
        Req::StoreClaimFreeGames { ids } => shared.store_claim_free_games(ids.clone()),
        Req::DiscordView {
            section,
            game,
            note,
            cover,
            store,
            store_id,
            appid,
        } => {
            if let Ok(mut view) = shared.discord_view.lock() {
                *view = crate::discord::Viewing {
                    section,
                    game,
                    note,
                    cover,
                    store,
                    store_id,
                    appid,
                };
            }
            Ok(Envelope::unit())
        }
        Req::DiscordPresence { active } => {
            shared
                .discord_active
                .store(active, std::sync::atomic::Ordering::Relaxed);
            Ok(Envelope::unit())
        }
        Req::SteamSocial { refresh } => shared.steam_social(refresh),
        Req::SteamFriendsState => shared.steam_friends_state(),
        Req::SteamSocialPresence { presence } => shared.steam_social_presence(presence),
        Req::SteamSocialRedeemKey { key } => shared.steam_social_redeem(&key),
        Req::SteamSocialMessage { steam_id, text } => {
            shared.steam_social_message(steam_id, &text)
        }
        Req::Logs { file, lines } => Ok(Envelope::ok(&logs::view(&shared.paths, file.as_deref(), lines))),
        Req::Stats => Ok(Envelope::ok(
            &shared.store.get_json::<lapinux_core::model::Stats>("stats").ok().flatten().unwrap_or_default(),
        )),
        Req::ClearCaches => Ok(Envelope::ok(&serde_json::json!({
            "freed": shared.clear_caches()
        }))),
        Req::Shutdown => {
            let _ = std::fs::remove_file(&shared.paths.socket);
            std::thread::spawn(|| {
                std::thread::sleep(Duration::from_millis(100));
                std::process::exit(0);
            });
            Ok(Envelope::unit())
        }
    };

    match result {
        Ok(envelope) => envelope,
        Err(err) => Envelope::err(err.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lapinux_core::model::{Game, JobKind};

    #[test]
    fn the_connection_budget_is_capped_and_gives_its_places_back() {
        // Deux places, comme un daemon à bout de souffle : la troisième
        // connexion est refusée d'emblée, sans fil.
        let budget = ConnectionBudget::new(2);
        let first = budget.take().expect("première place");
        let second = budget.take().expect("seconde place");
        assert!(budget.take().is_none());
        assert_eq!(budget.live(), 2);
        // Une réponse rendue libère sa place : le refus n'est pas définitif.
        drop(first);
        assert_eq!(budget.live(), 1);
        let third = budget.take().expect("place rendue");
        drop(second);
        drop(third);
        assert_eq!(budget.live(), 0);
    }

    /// Tâche minimale, pour ne tester que l'état et le jeu.
    fn test_job(id: &str, appid: Option<u32>, state: JobState) -> Job {
        Job {
            id: id.into(),
            kind: JobKind::InstallGame,
            label: format!("Jeu {id}"),
            title: format!("Jeu {id}"),
            appid,
            state,
            progress: 0.0,
            downloaded: 0,
            total: 0,
            network: 0,
            written: 0,
            server: String::new(),
            depot: 0,
            depots: 0,
            updating: false,
            dlc: Vec::new(),
            platform: None,
            message: String::new(),
            backend: "Lapinux".into(),
            started: 10,
            finished: None,
            report: None,
        }
    }

    #[test]
    fn ipc_dispatch_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        let shared = Arc::new(Shared::new(&paths).unwrap());
        shared
            .store
            .save_game(&Game {
                appid: 42,
                name: "Jeu test".into(),
                installed: true,
                ..Default::default()
            })
            .unwrap();

        assert!(dispatch(&shared, Req::Ping).ok);
        let games: Vec<Game> = dispatch(&shared, Req::Games).into_data().unwrap();
        assert_eq!(games.len(), 1);
        let game: Option<Game> = dispatch(&shared, Req::Game { appid: 42 })
            .into_data()
            .unwrap();
        assert_eq!(game.unwrap().name, "Jeu test");
        assert!(dispatch(&shared, Req::Config).ok);
        assert!(dispatch(&shared, Req::Doctor).ok);
        assert!(dispatch(&shared, Req::Devices).ok);
        assert!(!dispatch(&shared, Req::Launch { appid: 999, dry_run: true }).ok);
    }

    #[test]
    fn ping_publishes_the_build_of_the_running_daemon() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        let shared = Arc::new(Shared::new(&paths).unwrap());
        let build: lapinux_core::ipc::DaemonBuild = dispatch(&shared, Req::Ping).into_data().unwrap();
        // Le binaire de test existe : son empreinte identifie ce daemon.
        assert!(build.stamp > 0);
        assert_eq!(build.stamp, lapinux_core::paths::current_binary_stamp());
        assert_eq!(build.version, lapinux_core::VERSION);
        assert!(build.started > 1_600_000_000);
    }

    #[test]
    fn reload_config_reads_the_file_again() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        let shared = Arc::new(Shared::new(&paths).unwrap());
        assert_eq!(shared.config_snapshot().overlay.hotkey, "Shift+Tab");

        let mut config = shared.config_snapshot();
        config.overlay.hotkey = "Ctrl+F9".into();
        config.save(&paths.config_file).unwrap();
        shared.reload_config();

        assert_eq!(shared.config_snapshot().overlay.hotkey, "Ctrl+F9");
    }

    #[test]
    fn pause_job_sets_and_clears_the_flag() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        let shared = Arc::new(Shared::new(&paths).unwrap());
        let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        shared
            .pause
            .lock()
            .unwrap()
            .insert("jeu-1".into(), Arc::clone(&flag));

        assert!(
            dispatch(
                &shared,
                Req::PauseJob {
                    id: "jeu-1".into(),
                    paused: true,
                }
            )
            .ok
        );
        assert!(flag.load(std::sync::atomic::Ordering::Relaxed));
        assert!(
            dispatch(
                &shared,
                Req::PauseJob {
                    id: "jeu-1".into(),
                    paused: false,
                }
            )
            .ok
        );
        assert!(!flag.load(std::sync::atomic::Ordering::Relaxed));
        assert!(!dispatch(
            &shared,
            Req::PauseJob {
                id: "inconnue".into(),
                paused: true,
            }
        )
        .ok);
    }

    #[test]
    fn cancel_job_flips_the_job_flag_and_clears_the_card() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        let shared = Arc::new(Shared::new(&paths).unwrap());
        shared.jobs.lock().unwrap().push(test_job("jeu-1", Some(1), JobState::Running));
        let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        shared
            .cancel
            .lock()
            .unwrap()
            .insert("jeu-1".into(), Arc::clone(&flag));

        assert!(!flag.load(std::sync::atomic::Ordering::Relaxed));
        assert!(
            dispatch(&shared, Req::CancelJob { id: "jeu-1".into() }).ok,
            "annuler une tâche connue doit réussir"
        );
        // Le moteur reçoit l'ordre de s'arrêter et la carte n'attend pas :
        // une annulation n'a rien à faire dans « Terminées ».
        assert!(flag.load(std::sync::atomic::Ordering::Relaxed));
        assert!(shared.jobs.lock().unwrap().is_empty());
        assert!(
            !dispatch(&shared, Req::CancelJob { id: "inconnue".into() }).ok,
            "une tâche inconnue doit être refusée"
        );
    }

    #[test]
    fn delete_job_removes_only_finished_history() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        let shared = Arc::new(Shared::new(&paths).unwrap());
        shared
            .jobs
            .lock()
            .unwrap()
            .push(test_job("termine", Some(1), JobState::Done));
        shared
            .jobs
            .lock()
            .unwrap()
            .push(test_job("en-pause", Some(2), JobState::Paused));

        assert!(
            dispatch(
                &shared,
                Req::DeleteJob {
                    id: "termine".into()
                }
            )
            .ok,
            "un téléchargement terminé peut quitter l'historique"
        );
        assert!(
            !dispatch(
                &shared,
                Req::DeleteJob {
                    id: "en-pause".into()
                }
            )
            .ok,
            "un transfert en pause reste disponible pour la reprise"
        );
        let jobs = shared.jobs.lock().unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].id, "en-pause");
    }

    #[test]
    fn cancelling_a_queued_job_removes_it_from_the_queue() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        let shared = Arc::new(Shared::new(&paths).unwrap());
        shared.jobs.lock().unwrap().push(Job {
            id: "game-42-1".into(),
            kind: JobKind::InstallGame,
            label: "Jeu test (42)".into(),
            title: "Jeu test".into(),
            appid: Some(42),
            state: JobState::Queued,
            progress: 0.0,
            downloaded: 0,
            total: 0,
            network: 0,
            written: 0,
            server: String::new(),
            depot: 0,
            depots: 0,
            updating: false,
            dlc: Vec::new(),
            platform: None,
            message: "en attente".into(),
            backend: "Lapinux".into(),
            started: 10,
            finished: None,
            report: None,
        });
        shared.persist_jobs(true);

        assert!(
            dispatch(&shared, Req::CancelJob { id: "game-42-1".into() }).ok,
            "une tâche en attente doit pouvoir être annulée"
        );
        let jobs = shared.jobs.lock().unwrap();
        assert!(
            jobs.iter().all(|job| job.id != "game-42-1"),
            "une tâche annulée quitte la file et la liste"
        );
    }

    #[test]
    fn jobs_survive_a_restart_as_resumable() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        let shared = Arc::new(Shared::new(&paths).unwrap());
        shared.jobs.lock().unwrap().push(Job {
            id: "game-42-1".into(),
            kind: JobKind::InstallGame,
            label: "Jeu test (42)".into(),
            title: "Jeu test".into(),
            appid: Some(42),
            state: JobState::Paused,
            progress: 0.42,
            downloaded: 3_000_000,
            total: 8_000_000,
            network: 3_000_000,
            written: 2_000_000,
            server: "cache.steamcontent.com".into(),
            depot: 1,
            depots: 2,
            updating: false,
            dlc: Vec::new(),
            platform: None,
            message: "en pause".into(),
            backend: "Lapinux".into(),
            started: 10,
            finished: None,
            report: None,
        });
        shared.persist_jobs(true);
        drop(shared);

        let restarted = Arc::new(Shared::new(&paths).unwrap());
        let jobs = restarted.jobs.lock().unwrap().clone();
        assert_eq!(jobs.len(), 1);
        // Un redémarrage n'est pas une annulation : la carte reste, marquée
        // « interrompu », pour que « Reprendre » la relance au même point.
        assert_eq!(jobs[0].state, JobState::Interrupted);
        assert_eq!(jobs[0].downloaded, 3_000_000);
        assert!(jobs[0].message.contains("redémarrage"));
    }

    #[test]
    fn cancellations_do_not_survive_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        let shared = Arc::new(Shared::new(&paths).unwrap());
        let saved = |id: &str, state: JobState, message: &str| Job {
            id: id.into(),
            kind: JobKind::InstallGame,
            label: "Jeu test (7)".into(),
            title: "Jeu test".into(),
            appid: Some(7),
            state,
            progress: 0.2,
            downloaded: 1_000_000,
            total: 8_000_000,
            network: 1_000_000,
            written: 900_000,
            server: String::new(),
            depot: 1,
            depots: 1,
            updating: false,
            dlc: Vec::new(),
            platform: None,
            message: message.into(),
            backend: "Lapinux".into(),
            started: 5,
            finished: Some(6),
            report: None,
        };
        shared.jobs.lock().unwrap().extend([
            // Annulation de l'utilisateur : la carte ne revient pas.
            saved("game-7-1", JobState::Cancelled, "annulé avant démarrage"),
            // Interruption écrite par une ancienne version sous l'état
            // « annulé » : elle reste, avec son bouton Reprendre.
            saved(
                "game-7-2",
                JobState::Cancelled,
                "interrompu par le redémarrage ; les fichiers déjà reçus sont conservés",
            ),
        ]);
        shared.persist_jobs(true);
        drop(shared);

        let restarted = Arc::new(Shared::new(&paths).unwrap());
        let jobs = restarted.jobs.lock().unwrap().clone();
        assert_eq!(jobs.len(), 1, "une annulation ne se recharge pas");
        assert_eq!(jobs[0].id, "game-7-2");
        assert_eq!(jobs[0].state, JobState::Interrupted);
    }

    #[test]
    fn rate_tracker_reports_weighted_rate_and_extremes() {
        let mut tracker = RateTracker::default();
        let start = Instant::now();
        tracker.sample(start, 0);
        tracker.sample(start + Duration::from_secs(1), 5_000_000);
        tracker.sample(start + Duration::from_secs(2), 20_000_000);
        let report = tracker.report(20_000_000, 19_000_000, 5);
        assert_eq!(report.elapsed_secs, 5);
        assert_eq!(report.active_secs, 2);
        assert_eq!(report.samples, 2);
        assert_eq!(report.low_bps, 5_000_000);
        assert_eq!(report.peak_bps, 15_000_000);
        assert_eq!(report.average_bps, 10_000_000);
        assert_eq!(report.written, 19_000_000);
    }

    #[test]
    fn megabits_convert_to_bytes_for_the_throttle() {
        assert_eq!(mbit_per_sec_to_bytes(100), 12_500_000);
        assert_eq!(mbit_per_sec_to_bytes(0), 0);
    }

    #[test]
    fn download_limits_reach_the_shared_throttle() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        let shared = Arc::new(Shared::new(&paths).unwrap());
        let mut config = shared.config_snapshot();
        config.downloads.global_mbit_per_sec = 120;
        config
            .downloads
            .store_mbit_per_sec
            .insert("steam".into(), 80);
        assert!(dispatch(&shared, Req::SetConfig { config }).ok);
        let limit = shared.throttle.limit("steam");
        assert_eq!(
            limit.global.unwrap().rate(),
            15_000_000,
            "le plafond global doit suivre la configuration"
        );
        assert_eq!(
            limit.store.unwrap().rate(),
            10_000_000,
            "le plafond Steam doit suivre la configuration"
        );
    }

    #[test]
    fn settings_are_persisted() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        let shared = Arc::new(Shared::new(&paths).unwrap());
        shared
            .store
            .save_game(&Game {
                appid: 7,
                name: "Sept".into(),
                ..Default::default()
            })
            .unwrap();
        let mut config = shared.config_snapshot();
        config.general.library_root = dir.path().join("jeux").display().to_string();
        assert!(dispatch(&shared, Req::SetConfig { config: config.clone() }).ok);
        let stored: Config = dispatch(&shared, Req::Config).into_data().unwrap();
        assert_eq!(stored.general.library_root, config.general.library_root);
    }
}

fn single_releases() -> Result<Envelope> {
    let repos: Vec<&str> = lapinux_proton::SOURCES.iter().map(|s| s.repo).collect();
    let mut releases = Vec::new();
    for repo in repos {
        match lapinux_proton::fetch_releases(repo) {
            Ok(mut list) => releases.append(&mut list),
            Err(err) => tracing::warn!("releases {repo} : {err}"),
        }
    }
    Ok(Envelope::ok(&releases))
}
