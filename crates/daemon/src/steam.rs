//! Parcours Steam sans interface : compte Lapinux, catalogue, installation et
//! mise à jour natives, puis préparation du lancement Proton. Ni SteamCMD ni le
//! client Steam de bureau n'interviennent pour ces opérations ; le client
//! officiel peut être démarré en headless pour les jeux Steamworks. La session
//! qui en découle (entrées, raccourci, overlay, arrêt) vit dans
//! [`crate::launch`].

use crate::{Result, Shared};
use lapinux_core::ipc::Envelope;
use lapinux_core::model::{
    Game, Job, JobKind, JobState, SessionInfo, SteamGuardKind, SteamLoginState, SteamStatus,
    SteamTokenState, SteamTokenVerdict,
};
use lapinux_perf::{PerfCaps, PerfConfig};
use lapinux_proton::LaunchOptions;
use lapinux_steam::cm;
use lapinux_steam::native::{self, InstallEvent};
use lapinux_steam::session::{self, SavedSession};
use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

/// Message d'erreur prêt pour l'écran de connexion : le préfixe de catégorie
/// (« authentification : », « connexion : ») n'apprend rien à l'utilisateur.
fn message_utilisateur(error: &cm::Error) -> String {
    let text = error.to_string();
    let text = ["authentification : ", "connexion : ", "protocole : "]
        .iter()
        .find_map(|prefix| text.strip_prefix(prefix))
        .unwrap_or(&text);
    if error.is_rate_limited() || text.to_lowercase().contains("limite les tentatives") {
        return "Steam limite les tentatives de connexion pour ce compte. Attends au moins 30 minutes (idéalement 1 heure) sans réessayer : ton jeton est déjà enregistré, aucun nouveau QR n'est nécessaire. Clique sur « Se connecter » après l'attente.".into();
    }
    text.to_string()
}

/// Date de dernière écriture d'un fichier, en secondes Unix.
fn file_stamp(path: &Path) -> Option<i64> {
    std::fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|elapsed| elapsed.as_secs() as i64)
}

/// Faut-il lancer la vérification du jeton ?
///
/// Inutile si une autre tourne déjà, ou si le verdict rendu porte sur ce même
/// jeton — la date du jeton décide : un fichier réécrit rend le verdict caduc.
fn needs_check(check: &TokenCheck, stamp: Option<i64>) -> bool {
    if check.running {
        return false;
    }
    let known = check.token_saved_at == stamp
        && matches!(
            check.verdict,
            SteamTokenVerdict::Valid
                | SteamTokenVerdict::Expired
                | SteamTokenVerdict::RateLimited
        );
    !known
}

/// Durée de validité d'une lecture Steam par jeu.
///
/// L'interface redemande la fiche et les succès à chaque ouverture, et chaque
/// lecture ouvrait une session Steam entière. Steam n'en garde qu'une par
/// compte : à force, il refuse les connexions (`mot de passe invalide`,
/// eresult 5) pendant un moment. Une réponse fraîche est donc servie sans
/// toucher au réseau.
const GAME_READ_TTL: Duration = Duration::from_secs(300);

/// Préchargement des fiches au démarrage : combien de jeux récemment joués,
/// après quel délai (l'interface démarre en même temps), et avec quel espace
/// entre deux lectures (Steam ne garde qu'une session par compte).
const INFO_PRELOAD_GAMES: usize = 5;
const INFO_PRELOAD_DELAY: Duration = Duration::from_secs(15);
const INFO_PRELOAD_GAP: Duration = Duration::from_secs(2);
/// Une fiche sur disque plus jeune que ça n'a pas besoin d'être relue au
/// démarrage : elle est déjà à jour pour un usage courant.
const INFO_PRELOAD_MAX_AGE: Duration = Duration::from_secs(1800);

/// Ménage du dossier de fiches : une fiche par jeu ouvert y est écrite, le
/// dossier suivrait donc la taille de la bibliothèque. Les fiches plus vieilles
/// que ça partent, et au-delà du plafond les plus anciennes aussi.
const INFO_DISK_MAX_AGE: Duration = Duration::from_secs(30 * 24 * 3600);
const INFO_DISK_MAX_FILES: usize = 400;

/// Réponses Steam gardées par jeu, et lectures en cours.
#[derive(Debug, Default)]
pub struct GameReadCache {
    entries: std::collections::HashMap<(&'static str, u32), (Instant, serde_json::Value)>,
    running: std::collections::HashSet<(&'static str, u32)>,
}

impl GameReadCache {
    /// Valeur encore valable, s'il y en a une.
    fn fresh(&self, kind: &'static str, appid: u32) -> Option<serde_json::Value> {
        let (at, value) = self.entries.get(&(kind, appid))?;
        (at.elapsed() < GAME_READ_TTL).then(|| value.clone())
    }

    /// Valeur connue, même périmée : mieux qu'une attente quand une lecture
    /// est déjà en cours pour le même jeu.
    fn any(&self, kind: &'static str, appid: u32) -> Option<serde_json::Value> {
        self.entries
            .get(&(kind, appid))
            .map(|(_, value)| value.clone())
    }

    /// Prend la lecture à son compte ; `false` si une autre la tient déjà.
    pub(crate) fn begin(&mut self, kind: &'static str, appid: u32) -> bool {
        self.running.insert((kind, appid))
    }

    pub(crate) fn finish(&mut self, kind: &'static str, appid: u32, value: serde_json::Value) {
        self.running.remove(&(kind, appid));
        self.entries
            .insert((kind, appid), (Instant::now(), value));
    }

    pub(crate) fn abandon(&mut self, kind: &'static str, appid: u32) {
        self.running.remove(&(kind, appid));
    }

    /// Oublie toutes les lectures gardées : vider les caches doit se voir
    /// dans la session en cours, pas seulement au prochain démarrage.
    fn clear(&mut self) {
        self.entries.clear();
    }

    /// Installe une valeur relue sur le disque : elle est servie tout de
    /// suite mais marquée périmée, pour que la relecture réseau la remplace.
    fn seed(&mut self, kind: &'static str, appid: u32, value: serde_json::Value) {
        let at = Instant::now()
            .checked_sub(GAME_READ_TTL)
            .unwrap_or_else(Instant::now);
        self.entries.insert((kind, appid), (at, value));
    }

    /// La valeur connue est-elle encore dans sa fenêtre de fraîcheur ?
    fn is_fresh(&self, kind: &'static str, appid: u32) -> bool {
        self.entries
            .get(&(kind, appid))
            .map_or(false, |(at, _)| at.elapsed() < GAME_READ_TTL)
    }

    /// Oublie une lecture : la prochaine demande la refait. La sortie d'un jeu
    /// rend caduque celle du cloud, qui datait d'avant la partie.
    pub fn forget(&mut self, kind: &'static str, appid: u32) {
        self.entries.remove(&(kind, appid));
    }

    /// Oublie toutes les lectures d'une sorte (les fiches de boutique quand la
    /// langue change : leur texte est écrit dans celle qui était choisie).
    fn forget_kind(&mut self, kind: &'static str) {
        self.entries.retain(|(saved, _), _| *saved != kind);
    }
}

/// Résultat de la dernière vérification du jeton enregistré.
#[derive(Debug, Clone, Default)]
pub struct TokenCheck {
    pub verdict: SteamTokenVerdict,
    pub checked_at: Option<i64>,
    pub detail: Option<String>,
    /// Une vérification tient déjà la ligne réseau.
    pub running: bool,
    /// Date du jeton vérifié : un autre jeton (fichier réécrit, nouvelle
    /// connexion) rend ce verdict caduc.
    pub token_saved_at: Option<i64>,
}

impl Shared {
    pub fn steam_status(&self) -> SteamStatus {
        let config = self.config_snapshot();
        let login = self
            .login
            .lock()
            .map(|state| state.clone())
            .unwrap_or_default();
        let saved = session::load_from(&self.session_path());
        let token_saved = saved
            .as_ref()
            .map(|saved| !saved.refresh_token.trim().is_empty())
            .unwrap_or(false);
        // Les jetons écrits avant l'horodatage n'en ont pas : la date du
        // fichier dit alors à peu près quand il a été obtenu.
        let stamp = saved.as_ref().and_then(|saved| {
            saved
                .saved_at
                .or_else(|| file_stamp(&self.session_path()))
        });
        let check = self.token_check_for(stamp);
        let token = saved.as_ref().map(|saved| SteamTokenState {
            account_name: saved.account_name.clone(),
            saved_at: stamp,
            // Un jeton absent ne peut pas être valide, même sans vérification.
            verdict: if saved.refresh_token.trim().is_empty() {
                SteamTokenVerdict::Expired
            } else {
                check.verdict
            },
            checked_at: check.checked_at,
            detail: check.detail.clone(),
        });
        // Un jeton expiré ou temporairement bloqué ne peut pas ouvrir de
        // session : l'écran ne doit pas afficher « connecté », même si la
        // session du démarrage avait abouti.
        let unavailable = token
            .as_ref()
            .map(|token| {
                matches!(
                    token.verdict,
                    SteamTokenVerdict::Expired | SteamTokenVerdict::RateLimited
                )
            })
            .unwrap_or(false);
        // L'identifiant sert à ouvrir le profil public : la session le garde
        // d'une connexion à l'autre, même sans jeton.
        let steam_id = saved
            .map(|saved| saved.steam_id)
            .filter(|steam_id| *steam_id > 0);
        SteamStatus {
            username: config.steam.username.clone(),
            connected: !unavailable && (login.connected || token_saved),
            token_saved,
            token,
            steam_id,
            library_root: config.library_root(),
            login,
            default_proton: lapinux_proton::recommended(&self.protons_cached())
                .map(|proton| proton.name.clone()),
            default_language: lapinux_steam::native::default_language_label(),
        }
    }

    /// Verdict applicable au jeton enregistré. Un verdict rendu pour un autre
    /// jeton ne vaut plus rien : le fichier a été remplacé ou réécrit.
    fn token_check_for(&self, stamp: Option<i64>) -> TokenCheck {
        let check = self.token_check.lock().map(|c| c.clone()).unwrap_or_default();
        if stamp.is_none() || check.token_saved_at != stamp {
            return TokenCheck::default();
        }
        check
    }

    /// Vérifie le jeton enregistré auprès de Steam, en arrière-plan : le logon
    /// par jeton dure une seconde et ne doit pas retenir l'interface.
    ///
    /// Une seule vérification à la fois ; un verdict déjà connu (valide ou
    /// jeton refusé) n'est pas redemandé, un échec réseau ne condamne pas le
    /// jeton.
    pub fn steam_token_check(self: &Arc<Self>) -> Result<Envelope> {
        let saved = session::load_from(&self.session_path())
            .filter(|saved| !saved.refresh_token.trim().is_empty());
        let Some(saved) = saved else {
            return Ok(Envelope::ok(&self.steam_status()));
        };
        let stamp = saved
            .saved_at
            .or_else(|| file_stamp(&self.session_path()));
        // Le verrou est relâché avant toute lecture d'état : `steam_status`
        // relit le même verrou, et un return sous garde le bloquerait pour de bon.
        let start = {
            let mut check = self.token_check.lock().map_err(|_| {
                lapinux_core::Error::Other("état du jeton indisponible".into())
            })?;
            let start = needs_check(&check, stamp);
            if start {
                *check = TokenCheck {
                    verdict: SteamTokenVerdict::Checking,
                    running: true,
                    token_saved_at: stamp,
                    ..Default::default()
                };
            }
            start
        };
        if !start {
            return Ok(Envelope::ok(&self.steam_status()));
        }

        let shared = Arc::clone(self);
        std::thread::spawn(move || {
            // Première tentative par la session partagée : si le jeton est bon,
            // la vérification réutilise (ou ouvre) la connexion que toutes les
            // lectures emploieront — aucun logon de plus, donc aucune session
            // éjectée. C'est le cas courant.
            let verdict = match lapinux_steam::shared::verify_blocking(
                &saved.account_name,
                &saved.refresh_token,
            ) {
                Ok(()) => {
                    tracing::info!(
                        "jeton Lapinux vérifié : valide (steamid {})",
                        saved.steam_id
                    );
                    (SteamTokenVerdict::Valid, None)
                }
                Err(error) if error.is_rate_limited() => {
                    let message = message_utilisateur(&cm::Error::Auth(error.to_string()));
                    tracing::info!("jeton Lapinux non vérifié : {message}");
                    (SteamTokenVerdict::RateLimited, Some(message))
                }
                Err(error) => {
                    // La session partagée ne dit pas pourquoi elle a échoué :
                    // `cm::sign_in` tranche entre un jeton mort et une panne
                    // réseau, et c'est le seul cas où une connexion de plus se
                    // justifie.
                    tracing::debug!("session partagée : {error}");
                    let config = cm::Config::from_env();
                    let auth = cm::Auth::Token {
                        name: saved.account_name.clone(),
                        refresh_token: saved.refresh_token.clone(),
                    };
                    let mut prompt =
                        |_, _| Err(cm::Error::Auth("aucun code Steam Guard attendu".into()));
                    match cm::sign_in(&config, &auth, &mut prompt) {
                        Ok(session) => {
                            tracing::info!(
                                "jeton Lapinux vérifié : valide (steamid {})",
                                session.steam_id
                            );
                            (SteamTokenVerdict::Valid, None)
                        }
                        // Un refus d'authentification condamne le jeton ; une
                        // panne réseau ou un CM muet ne dit rien de lui.
                        Err(error @ cm::Error::Auth(_)) => {
                            let message = message_utilisateur(&error);
                            if error.is_rate_limited() {
                                tracing::info!("jeton Lapinux limité ({message})");
                                (SteamTokenVerdict::RateLimited, Some(message))
                            } else {
                                tracing::info!("jeton Lapinux vérifié : refusé ({message})");
                                (SteamTokenVerdict::Expired, Some(message))
                            }
                        }
                        Err(other) => {
                            let message = message_utilisateur(&other);
                            tracing::info!("jeton Lapinux non vérifié : {message}");
                            (SteamTokenVerdict::Unknown, Some(message))
                        }
                    }
                }
            };
            if let Ok(mut check) = shared.token_check.lock() {
                *check = TokenCheck {
                    verdict: verdict.0,
                    checked_at: Some(session::now_unix()),
                    detail: verdict.1,
                    running: false,
                    token_saved_at: stamp,
                };
            }
        });
        Ok(Envelope::ok(&self.steam_status()))
    }

    /// Le jeton vient d'être obtenu par une connexion : il est valide.
    fn mark_token_valid(&self) {
        // La date du jeton écrit décide de la validité de ce verdict.
        let stamp = session::load_from(&self.session_path())
            .and_then(|saved| saved.saved_at)
            .or_else(|| file_stamp(&self.session_path()));
        if let Ok(mut check) = self.token_check.lock() {
            *check = TokenCheck {
                verdict: SteamTokenVerdict::Valid,
                checked_at: Some(session::now_unix()),
                detail: None,
                running: false,
                token_saved_at: stamp,
            };
        }
    }

    /// Oublie le jeton Lapinux enregistré ; aucun mot de passe n'est conservé.
    pub fn steam_forget_token(&self) -> Result<Envelope> {
        if self.login.lock().map(|state| state.running).unwrap_or(false) {
            return Err(lapinux_core::Error::Other(
                "attendez la fin de la connexion Steam en cours".into(),
            ));
        }
        let path = self.session_path();
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(lapinux_core::Error::Other(format!(
                    "jeton non supprimé ({}): {error}",
                    path.display()
                )))
            }
        }
        // Plus de jeton : plus de verdict à afficher.
        if let Ok(mut check) = self.token_check.lock() {
            *check = TokenCheck::default();
        }
        // La session sociale vivait de ce jeton : elle s'arrête avec lui.
        self.stop_social();
        // L'état « connecté » ne survit pas au jeton : `steam_status`
        // recompose la réponse à partir de la session disque.
        self.finish_login(SteamLoginState {
            stage: "jeton oublié".into(),
            ..Default::default()
        });
        Ok(Envelope::ok(&self.steam_status()))
    }

    /// Déconnecte le compte : jeton, identité et session sociale. C'est le
    /// pendant Steam du bouton « Déconnecter » des autres boutiques ;
    /// « oublier le jeton » garde, lui, le nom du compte pour se reconnecter.
    pub fn steam_disconnect(&self) -> Result<Envelope> {
        let _ = self.steam_forget_token()?;
        let mut config = self.config_snapshot();
        if !config.steam.username.trim().is_empty() {
            config.steam.username.clear();
            config
                .save(&self.paths.config_file)
                .map_err(|error| lapinux_core::Error::Other(error.to_string()))?;
            if let Ok(mut guard) = self.config.lock() {
                *guard = config;
            }
        }
        // La session partagée vivait du jeton supprimé : la prochaine lecture
        // en ouvrira une neuve avec le compte reconnecté.
        lapinux_steam::shared::block_on(lapinux_steam::shared::forget());
        Ok(Envelope::ok(&self.steam_status()))
    }

    pub fn session_path(&self) -> std::path::PathBuf {
        session::path_in(&self.paths.data_dir)
    }

    fn saved_session(&self, username: &str) -> Option<SavedSession> {
        session::load_from(&self.session_path())
            .filter(|saved| saved.account_name.eq_ignore_ascii_case(username))
    }

    fn update_login(&self, update: impl FnOnce(&mut SteamLoginState)) {
        if let Ok(mut state) = self.login.lock() {
            update(&mut state);
        }
    }

    fn finish_login(&self, state: SteamLoginState) {
        if let Ok(mut slot) = self.login.lock() {
            *slot = state;
        }
    }

    /// Catalogue des jeux possédés. `refresh` lance une synchronisation en
    /// arrière-plan (licences Steam, puis noms) et rend tout de suite le cache :
    /// une grosse bibliothèque ne bloque ni le daemon ni l'interface, qui sonde.
    pub fn steam_catalog(self: &Arc<Self>, refresh: bool) -> Result<Envelope> {
        let config = self.config_snapshot();
        let account = config.steam.username.trim().to_string();

        if refresh {
            if account.is_empty() {
                return Err(lapinux_core::Error::Other(
                    "connectez d'abord votre compte Steam : la bibliothèque en dépend".into(),
                ));
            }
            self.start_catalog_refresh(&account);
        }

        let catalog = lapinux_steam::catalog::Catalog::load(&self.catalog_path());
        // Les fiches détaillées évoluent : un cache d'une version précédente
        // est complété une fois en arrière-plan sans attendre une action de
        // l'utilisateur. Les quelques appids dont Steam ne publie rien ne
        // doivent pas relancer une synchronisation à chaque démarrage.
        let stale = !catalog.entries.is_empty() && !catalog.is_current();
        if !refresh && stale && !self.catalog_details_checked.swap(true, Ordering::SeqCst) {
            self.start_catalog_refresh(&account);
        }
        // Jeux installés et leur bibliothèque : c'est là que se trouvent les
        // manifestes, donc là que se lit l'état d'un contenu additionnel.
        let installed: std::collections::HashMap<u32, std::path::PathBuf> = self
            .store
            .load_games()
            .unwrap_or_default()
            .into_iter()
            .filter(|game| game.installed)
            .map(|game| (game.appid, game.library))
            .collect();
        let mut entries: Vec<lapinux_core::model::SteamCatalogGame> = catalog
            .entries
            .iter()
            .filter(|entry| entry.is_game() || entry.is_demo())
            .map(|entry| lapinux_core::model::SteamCatalogGame {
                appid: entry.appid,
                name: entry.display_name(),
                installed: installed.contains_key(&entry.appid),
                family: entry.family,
                details: entry.details.clone(),
                acquired: entry.acquired,
                // La raison est mesurée ici, une fois toutes les sources lues :
                // prêté, gratuit, ou simplement non publié par Steam.
                undated_reason: entry.acquired.is_none().then(|| {
                    if entry.family {
                        "family"
                    } else if entry.details.free {
                        "free"
                    } else {
                        "unpublished"
                    }
                    .to_string()
                }),
                minutes: entry.minutes,
                dlc: catalog
                    .dlcs_of(entry)
                    .into_iter()
                    .map(|dlc| lapinux_core::model::SteamDlc {
                        installed: installed
                            .get(&entry.appid)
                            .map(|library| extra_content_installed(library, dlc.appid))
                            .unwrap_or(false),
                        appid: dlc.appid,
                        name: dlc.name,
                        download: dlc.download,
                        install: dlc.install,
                    })
                    .collect(),
            })
            .collect();
        entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

        Ok(Envelope::ok(&lapinux_core::model::SteamCatalog {
            account: catalog.account,
            updated: catalog.updated,
            entries,
            syncing: self.catalog_running.load(Ordering::SeqCst),
            error: self.catalog_error.lock().ok().and_then(|slot| slot.clone()),
        }))
    }

    /// Une lecture par jeu, servie depuis le cache tant qu'elle est fraîche.
    ///
    /// `kind` distingue les lectures (fiche, succès), `produce` fait le vrai
    /// travail : une seule lecture à la fois par jeu, les autres recevant la
    /// valeur connue plutôt que d'ouvrir une session Steam de plus.
    fn cached_game_read<F>(&self, kind: &'static str, appid: u32, produce: F) -> Result<Envelope>
    where
        F: FnOnce() -> Result<serde_json::Value>,
    {
        for _ in 0..50 {
            let (fresh, known) = {
                let cache = self
                    .game_reads
                    .lock()
                    .map_err(|_| lapinux_core::Error::Other("cache Steam indisponible".into()))?;
                (cache.fresh(kind, appid), cache.any(kind, appid))
            };
            if let Some(value) = fresh {
                return Ok(Envelope::ok(&value));
            }
            let started = {
                let mut cache = self
                    .game_reads
                    .lock()
                    .map_err(|_| lapinux_core::Error::Other("cache Steam indisponible".into()))?;
                if cache.running.contains(&(kind, appid)) {
                    false
                } else {
                    cache.begin(kind, appid)
                }
            };
            if started {
                let value = produce();
                let mut cache = self
                    .game_reads
                    .lock()
                    .map_err(|_| lapinux_core::Error::Other("cache Steam indisponible".into()))?;
                match &value {
                    Ok(value) => cache.finish(kind, appid, value.clone()),
                    Err(_) => cache.abandon(kind, appid),
                }
                return value.map(|value| Envelope::ok(&value));
            }
            // Une lecture tourne déjà : sa valeur, même ancienne, vaut mieux
            // qu'une seconde session Steam pour le même jeu.
            if let Some(value) = known {
                return Ok(Envelope::ok(&value));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        produce().map(|value| Envelope::ok(&value))
    }

    /// Fiche complète d'un jeu, relue à chaque ouverture : le compte par la Web
    /// API, le protocole par PICS, la boutique par sa page publique. Chaque
    /// source est tentée séparément — une source muette laisse une note et
    /// n'empêche pas les autres d'être lues.
    /// La fiche connue du jeu part tout de suite, même périmée, et sa
    /// relecture réseau se fait derrière : ouvrir un jeu ne doit plus coûter
    /// plusieurs secondes de protocole à chaque fois, seulement la première.
    pub fn game_info(self: &Arc<Self>, appid: u32) -> Result<Envelope> {
        let shared = Arc::clone(self);
        self.cached_remote_read("info", appid, move || shared.read_game_info(appid))
    }

    /// Fichier d'une fiche gardée sur disque : `fiches-v8` pour Steam,
    /// `fiches-boutique-v3` pour Epic, GOG et itch.
    fn sheet_path(&self, dir: &str, appid: u32) -> std::path::PathBuf {
        self.paths
            .cache_dir
            .join(dir)
            .join(format!("{appid}.json"))
    }

    /// Version 8 : la présentation du jeu passe avant les DLC et la
    /// description est découpée en blocs texte/média dans l'ordre de la page.
    /// Les anciens dossiers restent ignorés, ce qui périme les copies où
    /// « À propos » arrivait sous les contenus additionnels.
    fn game_info_path(&self, appid: u32) -> std::path::PathBuf {
        self.sheet_path("fiches-v8", appid)
    }

    pub(crate) fn store_sheet_path(&self, appid: u32) -> std::path::PathBuf {
        self.sheet_path("fiches-boutique-v3", appid)
    }

    /// Fichier de cache disque d'une lecture Steam. La fiche et les pages de
    /// boutique ont leur dossier historique ; les succès et le cloud gagnent
    /// le leur pour survivre à un redémarrage de l'interface.
    fn remote_sheet_path(&self, kind: &str, appid: u32) -> Option<std::path::PathBuf> {
        let dir = match kind {
            "info" => "fiches-v8".to_string(),
            "boutique" => "fiches-boutique-v3".to_string(),
            // Les succès et le cloud sont propres au compte : un second
            // compte ne doit jamais voir le relevé du premier.
            "succès" => format!("succes/{}", self.cache_account()),
            "cloud" => format!("clouds/{}", self.cache_account()),
            _ => return None,
        };
        Some(self.sheet_path(&dir, appid))
    }

    /// Compte Steam sous lequel les lectures personnelles sont rangées.
    /// Un nom vide garde un dossier commun, sans exposer de chemin arbitraire.
    fn cache_account(&self) -> String {
        let account = self.config_snapshot().steam.username;
        let clean: String = account
            .chars()
            .filter(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
            })
            .collect();
        if clean.is_empty() {
            "local".into()
        } else {
            clean.to_ascii_lowercase()
        }
    }

    /// Supprime la copie disque d'une lecture qui vient de changer (cloud
    /// restauré, partie terminée…). La mémoire seule serait insuffisante : au
    /// prochain redémarrage, l'ancienne copie reviendrait.
    pub(crate) fn forget_remote_sheet(&self, kind: &str, appid: u32) {
        if let Some(path) = self.remote_sheet_path(kind, appid) {
            let _ = std::fs::remove_file(path);
        }
    }

    /// Fiche écrite par un passage précédent, si elle se relit.
    fn read_sheet(path: &Path) -> Option<serde_json::Value> {
        let bytes = std::fs::read(path).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// Garde une fiche sur disque : ouvrir un jeu juste après un redémarrage
    /// ne repaie plus la lecture réseau. L'écriture passe par un fichier
    /// temporaire pour qu'une fiche à moitié écrite ne soit jamais relue.
    pub(crate) fn save_sheet(path: &Path, value: &serde_json::Value) {
        let Some(dir) = path.parent() else { return };
        if std::fs::create_dir_all(dir).is_err() {
            return;
        }
        let temp = path.with_extension("tmp");
        let Ok(bytes) = serde_json::to_vec(value) else {
            return;
        };
        if std::fs::write(&temp, bytes).is_ok() {
            let _ = std::fs::rename(&temp, path);
        }
    }

    /// Réponse connue servie tout de suite — mémoire, puis disque — et relecture
    /// réseau derrière quand elle a dépassé sa fenêtre. Les fiches, les succès
    /// et le cloud sont tenus sur disque ; les lectures personnelles sont
    /// rangées sous le compte Steam qui les a lues.
    pub(crate) fn cached_remote_read<F>(
        self: &Arc<Self>,
        kind: &'static str,
        appid: u32,
        produce: F,
    ) -> Result<Envelope>
    where
        F: Fn() -> Result<serde_json::Value> + Send + Sync + 'static,
    {
        let disk = self.remote_sheet_path(kind, appid);
        let (known, fresh) = {
            let cache = self.game_reads.lock().map_err(|_| {
                lapinux_core::Error::Other("cache Steam indisponible".into())
            })?;
            (cache.any(kind, appid), cache.is_fresh(kind, appid))
        };
        if let Some(known) = known {
            if !fresh {
                self.refresh_remote_read(kind, appid, Arc::new(produce), disk);
            }
            return Ok(Envelope::ok(&known));
        }
        // Rien en mémoire (premier lancement, ou jeu jamais ouvert depuis ce
        // démarrage) : la fiche écrite au dernier passage s'affiche aussitôt.
        if let Some(known) = disk.as_deref().and_then(Self::read_sheet) {
            if let Ok(mut cache) = self.game_reads.lock() {
                cache.seed(kind, appid, known.clone());
            }
            self.refresh_remote_read(kind, appid, Arc::new(produce), disk);
            return Ok(Envelope::ok(&known));
        }
        let envelope = self.cached_game_read(kind, appid, || produce())?;
        if let Some(path) = &disk {
            Self::save_sheet(path, &envelope.data);
        }
        Ok(envelope)
    }

    /// Relit en tâche de fond, une seule fois à la fois : la réponse suivante
    /// sera fraîche sans que l'écran ait attendu.
    fn refresh_remote_read<F>(
        self: &Arc<Self>,
        kind: &'static str,
        appid: u32,
        produce: Arc<F>,
        disk: Option<std::path::PathBuf>,
    ) where
        F: Fn() -> Result<serde_json::Value> + Send + Sync + 'static,
    {
        let started = self
            .game_reads
            .lock()
            .map(|mut cache| cache.begin(kind, appid))
            .unwrap_or(false);
        if !started {
            return;
        }
        let shared = Arc::clone(self);
        std::thread::spawn(move || {
            let value = produce();
            if let Ok(mut cache) = shared.game_reads.lock() {
                match &value {
                    Ok(value) => cache.finish(kind, appid, value.clone()),
                    Err(_) => cache.abandon(kind, appid),
                }
            }
            if let (Ok(value), Some(path)) = (&value, disk) {
                Self::save_sheet(&path, value);
            }
        });
    }

    /// Ménage du dossier de fiches : restes d'écriture, fiches trop vieilles,
    /// puis les plus anciennes au-delà du plafond. Appelé au démarrage ; sans
    /// lui, le dossier grossissait d'un fichier par jeu ouvert, sans fin.
    pub fn prune_game_info_cache(&self) {
        let mut removed = 0usize;
        for dir in [
            "fiches-v8",
            "fiches-v7",
            "fiches-v6",
            "fiches-v5",
            "fiches-v4",
            "fiches-v3",
            "fiches-v2",
            "fiches",
            "fiches-boutique-v3",
            "fiches-boutique",
        ] {
            removed += Self::prune_sheet_dir(&self.paths.cache_dir.join(dir));
        }
        if removed > 0 {
            tracing::debug!("fiches : {removed} fichier(s) de cache retiré(s)");
        }
    }

    /// Vide les caches de Lapinux : jaquettes, visuels, fiches, pages de
    /// boutique et lectures en mémoire. Configuration, journaux, Proton et jeux
    /// restent en place. Rend l'espace libéré, en octets.
    pub fn clear_caches(&self) -> u64 {
        let mut freed = 0u64;
        for dir in [
            "covers",
            "art",
            "stores",
            "webview",
            "fiches-v8",
            "fiches-v7",
            "fiches-v6",
            "fiches-v5",
            "fiches-v4",
            "fiches-v3",
            "fiches-v2",
            "fiches",
            "fiches-boutique-v3",
            "fiches-boutique",
            "succes",
            "clouds",
        ] {
            freed += Self::remove_tree(&self.paths.cache_dir.join(dir));
        }
        // Le cache des pages de boutique vit dans le profil WebKit, hors des
        // caches : c'est souvent le plus gros dossier de la maison. Les
        // cookies restent — vider les caches ne déconnecte personne.
        freed += Self::remove_tree(&self.paths.store_web_dir().join("cache"));
        if let Ok(mut cache) = self.game_reads.lock() {
            cache.clear();
        }
        freed
    }

    /// Oublie les fiches de boutique : description et annonces sont écrites
    /// dans la langue choisie, un changement de langue les périme.
    pub fn forget_store_sheets(&self) {
        if let Ok(mut cache) = self.game_reads.lock() {
            cache.forget_kind("boutique");
        }
        let _ = std::fs::remove_dir_all(self.paths.cache_dir.join("fiches-boutique-v3"));
        let _ = std::fs::remove_dir_all(self.paths.cache_dir.join("fiches-boutique"));
    }

    /// Vide un dossier et ses sous-dossiers ; rend le poids des fichiers partis.
    fn remove_tree(dir: &Path) -> u64 {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return 0;
        };
        let mut freed = 0u64;
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                freed += Self::remove_tree(&path);
                let _ = std::fs::remove_dir(&path);
            } else {
                let size = meta.len();
                if std::fs::remove_file(&path).is_ok() {
                    freed += size;
                }
            }
        }
        freed
    }

    /// Ménage d'un dossier de fiches ; rend le nombre de fichiers retirés.
    fn prune_sheet_dir(dir: &Path) -> usize {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return 0;
        };
        let now = std::time::SystemTime::now();
        let mut kept: Vec<(std::time::SystemTime, std::path::PathBuf)> = Vec::new();
        let mut removed = 0usize;
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            let written = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
            let age = now.duration_since(written).unwrap_or_default();
            // Un `.tmp` est le reste d'une écriture interrompue : jamais relu,
            // il part quel que soit son âge.
            let abandoned = path.extension().and_then(|ext| ext.to_str()) == Some("tmp");
            if abandoned || age > INFO_DISK_MAX_AGE {
                if std::fs::remove_file(&path).is_ok() {
                    removed += 1;
                }
                continue;
            }
            kept.push((written, path));
        }
        if kept.len() > INFO_DISK_MAX_FILES {
            kept.sort_by_key(|(written, _)| *written);
            let extra = kept.len() - INFO_DISK_MAX_FILES;
            for (_, path) in kept.into_iter().take(extra) {
                if std::fs::remove_file(&path).is_ok() {
                    removed += 1;
                }
            }
        }
        removed
    }

    /// Prépare en tâche de fond les fiches des jeux les plus récemment joués :
    /// leur première ouverture ne paie plus la lecture réseau, elle relit le
    /// disque. Séquentiel et espacé, parce qu'un second logon Steam ferme la
    /// session courante (sociale comprise) ; le préchargement commence après
    /// le démarrage de l'interface, quand ses propres requêtes sont passées.
    pub fn start_info_preload(self: &Arc<Self>) {
        let shared = Arc::clone(self);
        std::thread::spawn(move || {
            shared.prune_game_info_cache();
            // Sans session enregistrée, il n'y a rien à lire : le jeu de
            // réglages de connexion n'a pas à payer cinq tentatives.
            if session::load_from(&shared.session_path()).is_none() {
                return;
            }
            std::thread::sleep(INFO_PRELOAD_DELAY);
            let recent = shared.recent_steam_games(INFO_PRELOAD_GAMES);
            for (index, appid) in recent.into_iter().enumerate() {
                if index > 0 {
                    std::thread::sleep(INFO_PRELOAD_GAP);
                }
                if shared.info_is_warm(appid) {
                    continue;
                }
                // Une lecture déjà en cours (l'utilisateur vient d'ouvrir la
                // même fiche) n'est pas doublée.
                let started = shared
                    .game_reads
                    .lock()
                    .map(|mut cache| cache.begin("info", appid))
                    .unwrap_or(false);
                if !started {
                    continue;
                }
                let value = shared.read_game_info(appid);
                if let Ok(mut cache) = shared.game_reads.lock() {
                    match &value {
                        Ok(value) => cache.finish("info", appid, value.clone()),
                        Err(_) => cache.abandon("info", appid),
                    }
                }
                if let Ok(value) = value {
                    Self::save_sheet(&shared.game_info_path(appid), &value);
                }
            }
        });
    }

    /// Jeux Steam joués le plus récemment, du plus récent au plus ancien :
    /// c'est l'ordre dans lequel une fiche est rouverte.
    fn recent_steam_games(&self, limit: usize) -> Vec<u32> {
        let mut played: Vec<(i64, u32)> = self
            .store
            .load_games()
            .unwrap_or_default()
            .into_iter()
            .filter(|game| game.store == lapinux_core::stores::Store::Steam)
            .filter_map(|game| game.last_played.map(|at| (at, game.appid)))
            .collect();
        played.sort_by(|a, b| b.cmp(a));
        let mut seen = HashSet::new();
        played
            .into_iter()
            .filter(|(at, appid)| *at > 0 && seen.insert(*appid))
            .map(|(_, appid)| appid)
            .take(limit)
            .collect()
    }

    /// La fiche du jeu est déjà sur disque et assez jeune pour ne pas être
    /// relue au démarrage.
    fn info_is_warm(&self, appid: u32) -> bool {
        let Ok(meta) = std::fs::metadata(self.game_info_path(appid)) else {
            return false;
        };
        meta.modified()
            .ok()
            .and_then(|written| written.elapsed().ok())
            .map_or(false, |age| age < INFO_PRELOAD_MAX_AGE)
    }

    fn read_game_info(&self, appid: u32) -> Result<serde_json::Value> {
        let sources = self.steam_sources();
        // Le build installé se lit dans le manifeste : c'est lui que la fiche
        // compare au build public pour dire si une mise à jour attend.
        let installed = self
            .store
            .load_games()
            .unwrap_or_default()
            .into_iter()
            .find(|game| game.appid == appid);
        let installed_build = installed
            .as_ref()
            .and_then(|game| game.build_id.trim().parse::<u64>().ok());
        let mut info = lapinux_steam::gamedata::game_info(appid, &sources, installed_build);
        // Le manifeste porte aussi la date de ce build : elle date la version
        // qui tourne, comme `timebuildupdated` date celle que Steam publie.
        info.update.installed_build_time = installed
            .as_ref()
            .map(|game| game.last_updated)
            .filter(|time| *time > 0);
        // Contenus possédés : le catalogue tient la liste des licences, la
        // fiche ne la lit pas. Sans cette marque, la liste des contenus ne
        // pouvait pas dire « 4 sur 9 ».
        if !info.update.dlc.is_empty() {
            let catalog = lapinux_steam::catalog::Catalog::load(&self.catalog_path());
            let owned: HashSet<u32> = catalog
                .entries
                .iter()
                .find(|entry| entry.appid == appid)
                .map(|entry| {
                    catalog
                        .dlcs_of(entry)
                        .into_iter()
                        .map(|dlc| dlc.appid)
                        .collect()
                })
                .unwrap_or_default();
            for content in &mut info.update.dlc {
                content.owned = owned.contains(&content.appid);
            }
        }
        serde_json::to_value(&info)
            .map_err(|error| lapinux_core::Error::Other(format!("fiche illisible : {error}")))
    }

    /// Succès d'un jeu, lus par la session Steam seule : le schéma du jeu par
    /// `Player.GetGameAchievements`, l'état du compte par `ClientGetUserStats`.
    /// La session enregistrée suffit. Le résultat est gardé cinq minutes :
    /// la fiche les redemande à chaque repeint.
    pub fn game_achievements(self: &Arc<Self>, appid: u32) -> Result<Envelope> {
        let shared = Arc::clone(self);
        self.cached_remote_read("succès", appid, move || {
            let sources = shared.steam_sources();
            let read = lapinux_steam::achievements::game_achievements(appid, &sources);
            serde_json::to_value(&read)
                .map_err(|error| lapinux_core::Error::Other(format!("succès illisibles : {error}")))
        })
    }

    /// Relevé frais des succès, sans le cache de cinq minutes : la surveillance
    /// d'une partie doit voir un déblocage sans attendre que la fiche le
    /// redemande pour un autre jeu.
    pub(crate) fn read_steam_achievements_now(
        &self,
        appid: u32,
    ) -> lapinux_core::model::SteamAchievements {
        let sources = self.steam_sources();
        lapinux_steam::achievements::game_achievements(appid, &sources)
    }

    /// Sauvegardes Steam Cloud d'un jeu, lues par la session enregistrée : la
    /// liste des fichiers gardés, le quota consommé et — quand une partie vient
    /// de se terminer — ce qu'elle a changé par rapport au lancement.
    pub fn steam_cloud(self: &Arc<Self>, appid: u32) -> Result<Envelope> {
        let shared = Arc::clone(self);
        let envelope = self.cached_remote_read("cloud", appid, move || {
            let mut report = lapinux_core::model::CloudReport::default();
            match shared.read_cloud_result(appid) {
                Ok(saves) => report.saves = saves,
                Err(error) => report.error = Some(error),
            }
            report.diff = shared.cloud_diff_after(appid);
            serde_json::to_value(&report).map_err(|error| {
                lapinux_core::Error::Other(format!("sauvegardes cloud illisibles : {error}"))
            })
        })?;
        // La liste distante peut venir du cache ; l'état local, lui, change
        // dès qu'une reprise écrit un fichier. Il est donc recalculé à chaque
        // lecture au lieu d'être servi avec une ancienne réponse.
        let mut report = envelope
            .into_data::<lapinux_core::model::CloudReport>()
            .unwrap_or_default();
        self.mark_cloud_locals(appid, &mut report);
        report.syncs = crate::cloud_journal::history(&self.paths, appid);
        Ok(Envelope::ok(&report))
    }

    /// Remplit l'état local de chaque fichier cloud : présent ou non dans le
    /// préfixe du jeu, et à quelle date. C'est ce qui distingue « à récupérer »
    /// de « déjà là » dans les réglages du jeu.
    fn mark_cloud_locals(&self, appid: u32, report: &mut lapinux_core::model::CloudReport) {
        let Some(game) = self.store.game(appid).ok().flatten() else {
            return;
        };
        // Le relevé peut avoir été lu dans le cache : ses états locaux sont
        // repartis de zéro avant d'être vérifiés sur le disque.
        report.local_present = 0;
        report.local_dir = None;
        let mut dirs: Vec<std::path::PathBuf> = Vec::new();
        for file in &mut report.saves.files {
            // Une réponse en cache peut porter un ancien état local : un
            // chemin devenu introuvable doit repartir de « absent ».
            file.local = None;
            let Some(path) = cloud_file_path(&game, file, &self.paths.data_dir) else {
                continue;
            };
            file.local = std::fs::metadata(&path)
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|stamp| stamp.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|age| age.as_secs() as i64);
            if file.local.is_some() {
                report.local_present += 1;
            }
            if let Some(dir) = path.parent() {
                if !dirs.iter().any(|known| known == dir) {
                    dirs.push(dir.to_path_buf());
                }
            }
        }
        report.local_dir = dirs.first().map(|dir| dir.display().to_string());
    }

    /// Récupère du cloud les sauvegardes d'un jeu et les écrit dans son
    /// préfixe, à l'endroit où le jeu les lit.
    ///
    /// Steam ne synchronise rien sans son client : un jeu lancé par Lapinux
    /// démarre donc sans les sauvegardes du compte. Cette restauration les
    /// télécharge par le protocole (le CM rend l'adresse CDN), fichier par
    /// fichier, et remplace la copie locale quand elle diffère.
    pub fn cloud_restore(&self, appid: u32) -> Result<Envelope> {
        if self
            .session
            .lock()
            .map(|session| {
                session
                    .as_ref()
                    .map(|state| state.info.appid == appid)
                    .unwrap_or(false)
            })
            .unwrap_or(false)
        {
            return Err(lapinux_core::Error::Other(
                "quittez le jeu avant de récupérer ses sauvegardes".into(),
            ));
        }
        let game = self
            .store
            .game(appid)
            .map_err(|e| lapinux_core::Error::Other(e.to_string()))?
            .ok_or_else(|| lapinux_core::Error::Other("jeu introuvable".into()))?;
        let (written, bytes) = self
            .sync_cloud(&game, appid, false)
            .map_err(|error| lapinux_core::Error::Other(format!("sauvegardes cloud : {error}")))?;
        // La copie locale a changé : la fiche relira plutôt que de garder
        // l'état d'avant.
        if let Ok(mut cache) = self.game_reads.lock() {
            cache.forget("cloud", appid);
        }
        self.forget_remote_sheet("cloud", appid);
        tracing::info!(
            "cloud : {written} sauvegarde(s) récupérée(s) pour {appid} ({} octets)",
            bytes
        );
        Ok(Envelope::ok(&serde_json::json!({
            "files": written,
            "bytes": bytes,
        })))
    }

    /// Envoie vers le cloud les sauvegardes que la partie a écrites.
    ///
    /// Le relevé pris au lancement sert de témoin : si `change_number` a bougé
    /// entre-temps, une autre machine a sauvegardé pendant la partie — rien
    /// n'est envoyé, pour ne pas écraser sa progression. Un fichier dont la
    /// copie locale n'est pas plus récente que celle du cloud est laissé
    /// tranquille lui aussi.
    pub(crate) fn sync_cloud_up(
        &self,
        game: &Game,
        appid: u32,
    ) -> std::result::Result<
        (usize, u64, Option<lapinux_core::model::CloudDiff>, bool),
        String,
    > {
        let config = self.config_snapshot();
        let saved = self.saved_session(&config.steam.username).ok_or_else(|| {
            "connectez votre compte Steam dans Réglages → Compte Steam".to_string()
        })?;
        if saved.refresh_token.trim().is_empty() {
            return Err("la session Lapinux n'a pas de jeton réutilisable".to_string());
        }
        let data_dir = self.paths.data_dir.clone();
        let machine = machine_name();
        let record_machine = machine.clone();
        let before = self
            .cloud_before
            .lock()
            .ok()
            .and_then(|mut slot| slot.take())
            .filter(|(id, _)| *id == appid)
            .map(|(_, saves)| saves);
        let result = lapinux_steam::shared::block_on(async move {
            let session =
                lapinux_steam::shared::session(&saved.account_name, &saved.refresh_token)
                    .await
                    .map_err(|error| error.to_string())?;
            let cloud = lapinux_steam::cloud::game_cloud(&session, appid)
                .await
                .map_err(|error| error.to_string())?;
            let diff = before
                .as_ref()
                .map(|before| lapinux_core::model::cloud_diff(before, &cloud));
            // Le cloud garde-t-il déjà cette partie ? La réponse guide le
            // réglage qui saute la copie locale des jeux sauvegardés en ligne.
            let present = !cloud.files.is_empty();
            if cloud.files.is_empty() {
                return Ok((0, 0, diff, false));
            }
            if let Some(before) = &before {
                // Une autre machine a sauvegardé pendant la partie : chaque
                // fichier est arbitré séparément plus bas. Seul un fichier
                // local plus récent et différent part ; celui du cloud, plus
                // récent, reste intact. Un abandon global perdait les
                // réglages changés en jeu.
                if cloud.change_number != before.change_number {
                    tracing::warn!(
                        "cloud : change_number {} → {} ; envoi fichier par fichier",
                        before.change_number,
                        cloud.change_number
                    );
                }
            }
            let mut pending: Vec<(lapinux_core::model::CloudFile, Vec<u8>)> = Vec::new();
            for file in &cloud.files {
                let Some(path) = cloud_file_path(game, file, &data_dir) else {
                    continue;
                };
                let Some(local) = local_modified(&path) else {
                    continue;
                };
                let data = std::fs::read(&path).map_err(|error| error.to_string())?;
                if data.len() as u64 > lapinux_steam::cloud::MAX_UPLOAD {
                    tracing::warn!("cloud : {} trop gros pour l'envoi", file.name);
                    continue;
                }
                if !cloud_upload_wanted(file, Some(local), &lapinux_steam::cloud::file_sha(&data)) {
                    continue;
                }
                pending.push((file.clone(), data));
            }
            if pending.is_empty() {
                return Ok((0, 0, diff, present));
            }
            let names: Vec<String> = pending
                .iter()
                .map(|(file, _)| format!("{}{}", file.prefix, file.name))
                .collect();
            let batch = lapinux_steam::cloud::begin_batch(&session, appid, &machine, &names)
                .await
                .map_err(|error| error.to_string())?;
            let mut sent = 0usize;
            let mut bytes = 0u64;
            for (file, data) in &pending {
                lapinux_steam::cloud::upload_file(&session, appid, file, data, batch)
                    .await
                    .map_err(|error| error.to_string())?;
                sent += 1;
                bytes += data.len() as u64;
            }
            lapinux_steam::cloud::complete_batch(&session, appid, batch)
                .await
                .map_err(|error| error.to_string())?;
            Ok((sent, bytes, diff, present))
        });
        if let Ok((sent, bytes, _, _)) = &result {
            if *sent > 0 {
                crate::cloud_journal::record(
                    &self.paths,
                    appid,
                    lapinux_core::model::CloudSyncEntry {
                        at: lapinux_core::util::now_unix(),
                        direction: lapinux_core::model::CloudSyncDirection::Upload,
                        machine: record_machine,
                        files: *sent,
                        bytes: *bytes,
                    },
                );
            }
        }
        result
    }

    /// Envoie à la demande les sauvegardes d'un jeu, depuis ses réglages.
    pub fn cloud_upload(&self, appid: u32) -> Result<Envelope> {
        if self
            .session
            .lock()
            .map(|session| {
                session
                    .as_ref()
                    .map(|state| state.info.appid == appid)
                    .unwrap_or(false)
            })
            .unwrap_or(false)
        {
            return Err(lapinux_core::Error::Other(
                "quittez le jeu avant d'envoyer ses sauvegardes".into(),
            ));
        }
        let game = self
            .store
            .game(appid)
            .map_err(|e| lapinux_core::Error::Other(e.to_string()))?
            .ok_or_else(|| lapinux_core::Error::Other("jeu introuvable".into()))?;
        let (sent, bytes, diff, _) = self
            .sync_cloud_up(&game, appid)
            .map_err(|error| lapinux_core::Error::Other(format!("sauvegardes cloud : {error}")))?;
        if let Ok(mut cache) = self.game_reads.lock() {
            cache.forget("cloud", appid);
        }
        self.forget_remote_sheet("cloud", appid);
        if let Some(diff) = diff {
            if let Ok(mut slot) = self.cloud_last.lock() {
                *slot = Some((appid, diff));
            }
        }
        tracing::info!("cloud : {sent} sauvegarde(s) envoyée(s) pour {appid} ({bytes} octets)");
        Ok(Envelope::ok(&serde_json::json!({
            "files": sent,
            "bytes": bytes,
        })))
    }

    /// Prépare le préfixe Proton avant une reprise cloud. Un jeu jamais lancé
    /// n'a pas de `drive_c` : Proton le crée par `wineboot`, sans démarrer le
    /// jeu. En cas d'échec, le chemin est rendu quand même pour les
    /// sauvegardes `%GameInstall%`, qui n'ont pas besoin du préfixe.
    fn cloud_prefix(&self, game: &Game) -> std::path::PathBuf {
        let prefix = lapinux_proton::prefix_for(game, &self.paths.data_dir);
        if lapinux_proton::is_initialized(&prefix) {
            return prefix;
        }
        let config = self.config_snapshot();
        let protons = self.protons_cached();
        let wanted = game
            .settings
            .proton
            .clone()
            .unwrap_or_else(|| config.launch.proton.clone());
        let proton = if wanted == "auto" {
            lapinux_proton::recommended(&protons).cloned()
        } else {
            protons
                .iter()
                .find(|proton| proton.name == wanted)
                .cloned()
        };
        let Some(proton) = proton else {
            tracing::warn!(
                "cloud : aucun Proton utilisable pour préparer le préfixe de {}",
                game.appid
            );
            return prefix;
        };
        tracing::info!(
            "cloud : préparation du préfixe Proton de {} avec {}",
            game.appid,
            proton.name
        );
        if let Err(error) = lapinux_proton::initialize_prefix(
            &prefix,
            game.appid,
            &proton,
            &game.install_dir(),
        ) {
            tracing::warn!(
                "cloud : préfixe Proton de {} non préparé : {error}",
                game.appid
            );
        }
        prefix
    }

    /// Reprend les sauvegardes cloud plus récentes juste après une installation.
    ///
    /// La lecture préalable évite de préparer un préfixe Proton pour un jeu qui
    /// ne publie rien en ligne. Un échec (session, réseau, chemin inconnu) ne
    /// remet pas l'installation en cause : le premier lancement retentera la
    /// reprise de son côté.
    fn pull_cloud_after_install(&self, appid: u32) -> Option<String> {
        if !self.config_snapshot().downloads.cloud_saves_after_install {
            return None;
        }
        let game = self.store.game(appid).ok().flatten()?;
        if !game.settings.cloud_sync_enabled() {
            tracing::debug!("cloud : synchronisation automatique désactivée pour {appid}");
            return None;
        }
        match self.read_cloud_result(appid) {
            Ok(saves) if saves.files.is_empty() => {
                tracing::debug!(
                    "cloud : aucune sauvegarde à reprendre après l'installation de {appid}"
                );
                return None;
            }
            Ok(_) => {}
            Err(error) => {
                tracing::debug!(
                    "cloud : lecture impossible après l'installation de {appid} ({error})"
                );
                return None;
            }
        }
        match self.sync_cloud(&game, appid, true) {
            Ok((0, _)) => None,
            Ok((files, bytes)) => {
                // Le relevé affiché par la fiche a été lu avant l'installation :
                // sans cette invalidation, le tag restait « à récupérer » alors
                // que la reprise venait d'écrire les fichiers.
                if let Ok(mut cache) = self.game_reads.lock() {
                    cache.forget("cloud", appid);
                }
                self.forget_remote_sheet("cloud", appid);
                tracing::info!(
                    "cloud : {files} sauvegarde(s) reprise(s) après l'installation de {appid} ({bytes} octets)"
                );
                Some(format!("{files} sauvegarde(s) cloud"))
            }
            Err(error) => {
                tracing::debug!(
                    "cloud : reprise impossible après l'installation de {appid} ({error})"
                );
                None
            }
        }
    }

    /// Reprend dans le préfixe du jeu les sauvegardes que le compte a sur le
    /// cloud.
    ///
    /// `only_newer` ne ramène que les fichiers que le cloud a plus récents que
    /// le disque : c'est ce qui précède un lancement, où une progression locale
    /// déjà jouée ne doit pas être écrasée. Rendre `(0, 0)` veut dire « rien à
    /// reprendre ». Un fichier dont le chemin local est inconnu est ignoré pour
    /// ne pas abandonner la reprise des autres sauvegardes.
    fn sync_cloud(
        &self,
        game: &Game,
        appid: u32,
        only_newer: bool,
    ) -> std::result::Result<(usize, u64), String> {
        let config = self.config_snapshot();
        let saved = self.saved_session(&config.steam.username).ok_or_else(|| {
            "connectez votre compte Steam dans Réglages → Compte Steam".to_string()
        })?;
        if saved.refresh_token.trim().is_empty() {
            return Err("la session Lapinux n'a pas de jeton réutilisable".to_string());
        }
        // Un jeu jamais lancé n'a pas encore de `pfx/drive_c` : Proton le crée
        // par `wineboot`, sans démarrer le jeu, pour que les sauvegardes
        // puissent être posées au bon endroit.
        let prefix_dir = self.cloud_prefix(game);
        let result = lapinux_steam::shared::block_on(async move {
            let session =
                lapinux_steam::shared::session(&saved.account_name, &saved.refresh_token)
                    .await
                    .map_err(|error| error.to_string())?;
            let saves = lapinux_steam::cloud::game_cloud(&session, appid)
                .await
                .map_err(|error| error.to_string())?;
            if saves.files.is_empty() {
                return Err("ce jeu ne publie aucune sauvegarde cloud".to_string());
            }
            let mut written = 0usize;
            let mut bytes = 0u64;
            // Une reprise automatique ne doit pas retarder le lancement : passé
            // le délai, le reste s'obtient par le bouton des réglages.
            let deadline =
                std::time::Instant::now() + std::time::Duration::from_secs(25);
            for file in &saves.files {
                if only_newer && std::time::Instant::now() > deadline {
                    tracing::warn!("cloud : délai de reprise dépassé, fichiers restants laissés au bouton");
                    break;
                }
                let Some(path) = cloud_file_path_in(game, file, &prefix_dir) else {
                    tracing::warn!(
                        "cloud : {} ignoré, chemin de sauvegarde inconnu (préfixe « {} »)",
                        file.name,
                        file.prefix
                    );
                    continue;
                };
                if only_newer && !cloud_file_wanted(file, local_modified(&path)) {
                    continue;
                }
                let data = lapinux_steam::cloud::download_file(&session, appid, file)
                    .await
                    .map_err(|error| error.to_string())?;
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
                }
                std::fs::write(&path, &data).map_err(|error| error.to_string())?;
                if file.modified > 0 {
                    // La date du cloud guide le jeu quand il compare ses
                    // sauvegardes : elle est reposée sur la copie locale.
                    let stamp = std::time::UNIX_EPOCH
                        + std::time::Duration::from_secs(file.modified as u64);
                    if let Ok(handle) = std::fs::OpenOptions::new().write(true).open(&path) {
                        let _ = handle.set_times(std::fs::FileTimes::new().set_modified(stamp));
                    }
                }
                written += 1;
                bytes += data.len() as u64;
            }
            Ok::<(usize, u64), String>((written, bytes))
        });
        if let Ok((files, bytes)) = &result {
            if *files > 0 {
                crate::cloud_journal::record(
                    &self.paths,
                    appid,
                    lapinux_core::model::CloudSyncEntry {
                        at: lapinux_core::util::now_unix(),
                        direction: lapinux_core::model::CloudSyncDirection::Download,
                        machine: "Steam Cloud".into(),
                        files: *files,
                        bytes: *bytes,
                    },
                );
            }
        }
        result
    }

    /// Liste des sauvegardes cloud d'un jeu, ou `None` quand le compte n'a pas
    /// de session utilisable : la fiche se passe alors de la ligne.
    fn read_cloud(&self, appid: u32) -> Option<lapinux_core::model::CloudSaves> {
        self.read_cloud_result(appid).ok()
    }

    /// Même lecture, avec la raison de l'échec : la carte des réglages doit
    /// distinguer « aucune sauvegarde » de « session coupée ».
    fn read_cloud_result(
        &self,
        appid: u32,
    ) -> std::result::Result<lapinux_core::model::CloudSaves, String> {
        let config = self.config_snapshot();
        let saved = self.saved_session(&config.steam.username).ok_or_else(|| {
            "connectez votre compte Steam dans Réglages → Compte Steam".to_string()
        })?;
        if saved.refresh_token.trim().is_empty() {
            return Err("la session Lapinux n'a pas de jeton réutilisable".to_string());
        }
        lapinux_steam::shared::block_on(async {
            let session =
                lapinux_steam::shared::session(&saved.account_name, &saved.refresh_token).await?;
            lapinux_steam::cloud::game_cloud(&session, appid).await
        })
        .map_err(|error| {
            tracing::warn!("cloud du jeu {appid} illisible : {error}");
            error.to_string()
        })
    }

    /// Différence entre le relevé pris au lancement et l'état actuel du cloud.
    ///
    /// Rien n'est comparé tant que le jeu tourne : la partie est justement en
    /// train d'écrire. La comparaison est faite une fois, gardée pour la fiche,
    /// puis le relevé est oublié — le lancement suivant en prendra un neuf.
    fn cloud_diff_after(&self, appid: u32) -> Option<lapinux_core::model::CloudDiff> {
        if self
            .session_info()
            .map(|session| session.appid == appid)
            .unwrap_or(false)
        {
            return None;
        }
        if let Some((known, diff)) = self.cloud_last.lock().ok().and_then(|slot| slot.clone()) {
            if known == appid {
                return Some(diff);
            }
        }
        let (known, before) = self
            .cloud_before
            .lock()
            .ok()
            .and_then(|slot| slot.clone())?;
        if known != appid {
            return None;
        }
        let after = self.read_cloud(appid)?;
        let diff = lapinux_core::model::cloud_diff(&before, &after);
        tracing::info!("cloud : {}", diff.summary());
        if let Ok(mut slot) = self.cloud_last.lock() {
            *slot = Some((appid, diff.clone()));
        }
        if let Ok(mut slot) = self.cloud_before.lock() {
            *slot = None;
        }
        Some(diff)
    }

    /// Session enregistrée et clé du compte, ce dont les lectures Steam ont
    /// besoin.
    fn steam_sources(&self) -> lapinux_steam::gamedata::Sources {
        let saved = session::load_from(&self.session_path());
        // La langue choisie pour les jeux pilote aussi la langue des pages
        // Steam lues pour la fiche (résumé, à propos, annonces).
        let language = lapinux_core::Config::load(&self.paths.config_file)
            .map(|config| config.general.game_language)
            .unwrap_or_default();
        lapinux_steam::gamedata::Sources {
            language,
            account: saved
                .as_ref()
                .map(|saved| saved.account_name.clone())
                .unwrap_or_default(),
            refresh_token: saved
                .as_ref()
                .map(|saved| saved.refresh_token.clone())
                .unwrap_or_default(),
            steam_id: saved.as_ref().map(|saved| saved.steam_id).unwrap_or(0),
        }
    }

    fn catalog_path(&self) -> std::path::PathBuf {
        self.paths.data_dir.join("steam-catalog.json")
    }

    /// Synchronisation du catalogue en arrière-plan, une seule à la fois.
    pub(crate) fn start_catalog_refresh(self: &Arc<Self>, account: &str) {
        if account.trim().is_empty() || self.catalog_running.swap(true, Ordering::SeqCst) {
            return;
        }
        let shared = Arc::clone(self);
        let account = account.trim().to_string();
        std::thread::spawn(move || {
            let result = shared.refresh_catalog(&account);
            if let Ok(mut slot) = shared.catalog_error.lock() {
                *slot = result.err().map(|error| error.to_string());
            }
            shared.catalog_running.store(false, Ordering::SeqCst);
        });
    }

    pub fn steam_catalog_patch(self: &Arc<Self>, appid: u32, name: String) -> Result<Envelope> {
        let mut appid = appid;
        let mut title = name.trim().to_string();
        if appid == 0 && !title.is_empty() {
            if let Some((found, named)) = lapinux_steam::native::search_appid(&title) {
                appid = found;
                title = named;
            }
        }
        if appid != 0 {
            self.insert_catalog_entry(appid, title)?;
            // La fiche d'un jeu obtenu par une clé n'a pas encore d'images :
            // la boutique les donne tout de suite, sans session.
            let shared = Arc::clone(self);
            std::thread::spawn(move || {
                shared.fill_catalog_details(appid);
                // Description, captures, prérequis, poids : la fiche complète
                // se lit tout de suite, sinon elle restait vide jusqu'à la
                // synchronisation suivante.
                if let Err(error) = shared.game_info(appid) {
                    tracing::info!("fiche du jeu {appid} non lue ({error})");
                }
            });
        } else {
            // Le reçu ne nomme rien : la licence de la clé porte son paquet, et
            // la boutique donne le jeu en une requête. La lecture prend
            // quelques secondes, hors de la réponse.
            let shared = Arc::clone(self);
            let account = self.config_snapshot().steam.username.trim().to_string();
            std::thread::spawn(move || match shared.resolve_key_game() {
                Some((found, named)) => {
                    if let Err(error) = shared.insert_catalog_entry(found, named) {
                        tracing::warn!("catalogue : jeu de la clé non enregistré ({error})");
                    } else {
                        shared.fill_catalog_details(found);
                        if let Err(error) = shared.game_info(found) {
                            tracing::info!("fiche du jeu {found} non lue ({error})");
                        }
                    }
                }
                None => {
                    // Ni reçu ni boutique n'ont nommé le jeu : la relecture
                    // complète reste le filet, en tâche de fond.
                    tracing::warn!("catalogue : jeu de la clé introuvable, relecture complète");
                    shared.start_catalog_refresh(&account);
                }
            });
        }
        self.steam_catalog(false)
    }

    /// Ajoute une fiche minimale au catalogue, sans relire le compte : la
    /// synchronisation complète la complétera.
    /// Complète la fiche d'un jeu ajouté par une clé : images, icône et
    /// présentation lues sur la boutique publique, sans session. Sans cela, sa
    /// fiche restait vide jusqu'à la synchronisation suivante.
    fn fill_catalog_details(&self, appid: u32) {
        let Some(details) = lapinux_steam::native::store_art(appid) else {
            tracing::info!("catalogue : images du jeu {appid} indisponibles");
            return;
        };
        let path = self.catalog_path();
        let mut catalog = lapinux_steam::catalog::Catalog::load(&path);
        let Some(slot) = catalog
            .entries
            .iter_mut()
            .find(|entry| entry.appid == appid)
        else {
            return;
        };
        if !slot.details.header_image.is_empty() {
            return;
        }
        slot.details = details;
        catalog.updated = lapinux_core::util::now_unix();
        if let Err(error) = catalog.save(&path) {
            tracing::warn!("catalogue : fiche du jeu {appid} non enregistrée ({error})");
        } else {
            tracing::info!("catalogue : images du jeu {appid} complétées");
        }
    }

    fn insert_catalog_entry(&self, appid: u32, title: String) -> Result<()> {
        let path = self.catalog_path();
        let mut catalog = lapinux_steam::catalog::Catalog::load(&path);
        let title = title.trim().to_string();
        let stamp = lapinux_core::util::now_unix();
        if catalog.entries.iter().any(|entry| entry.appid == appid) {
            // La fiche existait déjà, sans date : la clé vient de la lui
            // donner, et cette date prime sur celles qu'une relève devine.
            let mut touched = false;
            if let Some(slot) = catalog
                .entries
                .iter_mut()
                .find(|entry| entry.appid == appid)
            {
                if slot.acquired.is_none() {
                    slot.acquired = Some(stamp);
                    touched = true;
                }
                if !title.is_empty() && slot.name.starts_with("Jeu ") {
                    slot.name = title;
                    touched = true;
                }
            }
            catalog.stamped.insert(appid, stamp);
            catalog.updated = stamp;
            catalog
                .save(&path)
                .map_err(|error| lapinux_core::Error::Other(error.to_string()))?;
            if touched {
                tracing::info!("catalogue : jeu {appid} daté après une clé");
            }
            return Ok(());
        }
        let title = if title.is_empty() {
            format!("Jeu {appid}")
        } else {
            title
        };
        catalog.entries.push(lapinux_steam::catalog::Entry {
            appid,
            name: title,
            kind: "Game".to_string(),
            family: false,
            acquired: Some(stamp),
            minutes: None,
            details: Default::default(),
        });
        catalog.stamped.insert(appid, stamp);
        catalog.updated = stamp;
        catalog
            .save(&path)
            .map_err(|error| lapinux_core::Error::Other(error.to_string()))?;
        tracing::info!("catalogue : jeu {appid} ajouté après une clé");
        Ok(())
    }

    /// Jeu de la licence la plus récente du compte : sa date départage la clé
    /// qui vient d'être activée, et son paquet suffit à la boutique pour rendre
    /// l'appid et le nom. Ni PICS ni reconstruction du catalogue.
    fn resolve_key_game(self: &Arc<Self>) -> Option<(u32, String)> {
        let account = self.config_snapshot().steam.username.trim().to_string();
        if account.is_empty() {
            tracing::warn!("catalogue : aucun compte Steam en configuration");
            return None;
        }
        let saved = self.saved_session(&account)?;
        if saved.refresh_token.trim().is_empty() {
            tracing::warn!("catalogue : session Lapinux sans jeton réutilisable");
            return None;
        }
        tracing::info!("catalogue : recherche du jeu de la clé pour {account}");
        self.pause_social();
        let result = (|| {
            let mut session = self.sign_in_with_token(&account, &saved.refresh_token).ok()?;
            tracing::info!("catalogue : session ouverte pour la clé");
            let mut licenses = session.licenses.take().unwrap_or_default();
            drop(session);
            licenses.retain(|license| license.time_created > 0);
            let newest = licenses
                .iter()
                .max_by_key(|license| license.time_created)?;
            tracing::info!(
                "catalogue : paquet {} de la licence la plus récente",
                newest.package_id
            );
            lapinux_steam::native::package_app(newest.package_id)
        })();
        self.resume_social();
        if result.is_none() {
            tracing::warn!("catalogue : la licence de la clé n'a rendu aucun jeu");
        }
        result
    }

    /// Travail lourd de la synchronisation, exécuté hors de la boucle IPC.
    /// La liste de licences vient du canal CM Lapinux ; PICS étend chaque
    /// paquet en appids, puis rend les noms. SteamCMD n'est jamais appelé.
    ///
    /// Steam ne garde qu'une session par compte : deux logons qui se suivent
    /// se ferment l'un l'autre, et la session sociale se reconnecte toute
    /// seule — ce qui coupe la synchronisation en plein vol. Elle attend donc
    /// la fin du catalogue, puis repart.
    fn refresh_catalog(self: &Arc<Self>, account: &str) -> Result<()> {
        self.pause_social();
        let result = self.refresh_catalog_work(account);
        self.resume_social();
        result
    }

    fn refresh_catalog_work(&self, account: &str) -> Result<()> {
        let saved = self.saved_session(account).ok_or_else(|| {
            lapinux_core::Error::Other(
                "connectez votre compte Steam dans Réglages → Compte Steam".into(),
            )
        })?;
        if saved.refresh_token.trim().is_empty() {
            return Err(lapinux_core::Error::Other(
                "la session Lapinux n'a pas de jeton réutilisable : reconnectez-vous".into(),
            ));
        }
        let mut session = self
            .sign_in_with_token(account, &saved.refresh_token)
            .map_err(|error| lapinux_core::Error::Other(format!("session Lapinux : {error}")))?;
        let steam_id = session.steam_id;
        let licenses = session
            .licenses
            .take()
            .filter(|licenses| !licenses.is_empty())
            .ok_or_else(|| {
                lapinux_core::Error::Other(
                    "Steam n'a renvoyé aucune licence pour ce compte".into(),
                )
            })?;
        let own_account_id = (steam_id & 0xffff_ffff) as u32;
        let (packages, family_packages) = split_license_packages(&licenses, own_account_id);
        // Le canal qui a lu les licences se ferme avant que le catalogue
        // n'ouvre sa propre session Steam : un compte n'en garde qu'une, et
        // deux logons qui se suivent se coupent l'un l'autre — c'est ainsi que
        // PICS cessait de rendre les fiches et vidait la bibliothèque.
        drop(session);
        // Les licences du site portent la date d'achat que le protocole ne donne
        // pas. Elles se lisent ici : une session close invalide le jeton qu'elle
        // a émis, et la page rend alors 403. Rien n'est bloquant — sans elle,
        // la synchro continue, les jeux restent simplement sans date.
        // Steam renouvelle le jeton d'actualisation à chaque connexion : celui
        // chargé en tête de synchro est périmé depuis le logon, et un jeton
        // périmé fait rendre 403 à la page. On relit donc celui du disque.
        let current_session = lapinux_steam::session::load_from(
            &lapinux_steam::session::path_in(&self.paths.data_dir),
        );
        let refresh_token = current_session
            .as_ref()
            .map(|session| session.refresh_token.as_str())
            .filter(|token| !token.trim().is_empty())
            .unwrap_or(&saved.refresh_token);
        let site_dates = match lapinux_steam::web::license_dates(account, refresh_token, steam_id)
        {
            Ok(dates) => dates,
            Err(error) => {
                tracing::info!("catalogue : dates du site indisponibles ({error})");
                Vec::new()
            }
        };
        // Une licence datée date le jeu principal de son paquet : c'est la date
        // d'ajout au compte. Les paquets de la famille datent le compte de
        // l'autre membre, pas le nôtre : ils restent sans date.
        let mut package_dates: std::collections::HashMap<u32, i64> =
            std::collections::HashMap::new();
        for license in &licenses {
            if license.time_created <= 0 {
                continue;
            }
            package_dates
                .entry(license.package_id)
                .and_modify(|old| *old = (*old).min(license.time_created))
                .or_insert(license.time_created);
        }
        tracing::info!(
            "catalogue : {} paquets de licences, {} partagés par la famille Steam",
            packages.len(),
            family_packages.len()
        );
        let path = self.catalog_path();
        let previous = lapinux_steam::catalog::Catalog::load(&path);
        let (mut entries, mut dlcs, _package_main, appids) = native::catalog(
            account,
            &saved.refresh_token,
            steam_id,
            &packages,
            &family_packages,
            &package_dates,
            &site_dates,
        )
        .map_err(|e| lapinux_core::Error::Other(e.to_string()))?;
        if entries.is_empty() {
            return Err(lapinux_core::Error::Other(
                "Steam n'a renvoyé aucun jeu pour ce compte".into(),
            ));
        }
        // Une lecture du site qui échoue (403 d'Edge, page vide) ne doit pas
        // effacer les dates qu'une synchronisation précédente avait posées :
        // sans cette reprise, une panne d'une minute faisait reculer la
        // bibliothèque datée.
        // Le report tourne toujours : une lecture partielle du site ne doit
        // pas faire reculer un jeu daté la veille.
        if !previous.entries.is_empty() {
            let earlier: std::collections::HashMap<u32, i64> = previous
                .entries
                .iter()
                .filter_map(|entry| Some((entry.appid, entry.acquired?)))
                .collect();
            let mut kept = 0usize;
            for entry in &mut entries {
                if entry.acquired.is_none() {
                    if let Some(stamp) = earlier.get(&entry.appid) {
                        entry.acquired = Some(*stamp);
                        // Une date reprise vient d'une licence du compte : le
                        // jeu est possédé. Sans cela, une lecture du site
                        // refusée le remarquait familial tout en gardant sa
                        // date — deux états contradictoires pour un même jeu.
                        entry.family = false;
                        kept += 1;
                    }
                }
            }
            if kept > 0 {
                tracing::info!(
                    "catalogue : {kept} date(s) gardées de la synchronisation précédente"
                );
            }
        }
        // Les dates du site comblent les jeux arrivés par la liste des jeux
        // possédés, comme Insurgency. La page n'a pas d'appid, d'où le
        // rapprochement par le nom — et seulement pour les jeux sans date.
        if !site_dates.is_empty() {
            let mut filled = 0usize;
            for entry in &mut entries {
                if entry.acquired.is_some() {
                    continue;
                }
                if let Some((_, stamp)) = site_dates
                    .iter()
                    .find(|(name, _)| lapinux_steam::web::same_title(name, &entry.name))
                {
                    entry.acquired = Some(*stamp);
                    // La page des licences ne liste que les licences *du
                    // compte* : un homonyme daté prouve la possession. Un jeu
                    // acheté (clé revendeur comprise) garde donc sa date même
                    // si un membre de la famille le partage aussi — la
                    // possession l'emporte sur le partage.
                    entry.family = false;
                    filled += 1;
                }
            }
            if filled > 0 {
                tracing::info!(
                    "catalogue : {filled} date(s) d'achat lues sur la page des licences"
                );
            } else {
                tracing::info!(
                    "catalogue : {} licence(s) datées sur le site, aucune ne rapproche un jeu sans date",
                    site_dates.len()
                );
            }
        }
        // L'historique des transactions nomme les jeux achetés par leur appid,
        // là où la page des licences n'a que des noms de paquets : il date les
        // jeux qu'aucun rapprochement de noms n'atteint. Une lecture refusée ne
        // retire aucune date.
        if entries.iter().any(|entry| entry.acquired.is_none()) {
            match lapinux_steam::web::purchase_dates(account, refresh_token, steam_id) {
                Ok(dates) if !dates.is_empty() => {
                    let known: std::collections::HashMap<u32, i64> = dates.into_iter().collect();
                    let mut filled = 0usize;
                    for entry in &mut entries {
                        if entry.acquired.is_some() {
                            continue;
                        }
                        if let Some(stamp) = known.get(&entry.appid) {
                            entry.acquired = Some(*stamp);
                            entry.family = false;
                            filled += 1;
                        }
                    }
                    tracing::info!("catalogue : {filled} jeu(x) datés par l'historique des achats");
                }
                Ok(_) => {
                    tracing::info!("catalogue : historique des achats sans appid exploitable")
                }
                Err(error) => {
                    tracing::info!("catalogue : historique des achats indisponible ({error})")
                }
            }
        }
        // Une synchronisation incomplète ne doit pas effacer une bibliothèque
        // connue : les fiches des jeux toujours licenciés que PICS n'a pas
        // redécrites cette fois restent telles quelles.
        if !previous.entries.is_empty() {
            let licensed: std::collections::HashSet<u32> = appids.iter().copied().collect();
            let known: std::collections::HashSet<u32> = entries
                .iter()
                .map(|entry| entry.appid)
                .chain(dlcs.iter().map(|dlc| dlc.appid))
                .collect();
            let mut kept = 0usize;
            for entry in &previous.entries {
                if licensed.contains(&entry.appid) && !known.contains(&entry.appid) {
                    entries.push(entry.clone());
                    kept += 1;
                }
            }
            for dlc in &previous.dlcs {
                if licensed.contains(&dlc.appid) && !known.contains(&dlc.appid) {
                    dlcs.push(dlc.clone());
                }
            }
            if kept > 0 {
                tracing::info!(
                    "catalogue : {kept} fiche(s) gardées de la synchronisation précédente"
                );
            }
        }
        // Une synchronisation qui ne décrit qu'une poignée des applications
        // licenciées est dégradée, même si l'ancien catalogue est déjà court :
        // le 1er novembre, une session CM muette a fait tomber la bibliothèque
        // de 1 362 à 159 jeux, et chaque relance réécrivait le catalogue déjà
        // amputé au lieu de garder le précédent. Le rapport se mesure donc aux
        // applications licenciées, pas à la taille du catalogue d'avant.
        let named = entries.len() + dlcs.len();
        if !appids.is_empty() && named * 2 < appids.len() {
            return Err(lapinux_core::Error::Other(format!(
                "Steam n'a décrit que {named} applications sur {} : l'ancien catalogue est gardé",
                appids.len()
            )));
        }
        entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        entries.dedup_by_key(|entry| entry.appid);
        dlcs.sort_by_key(|entry| entry.appid);
        dlcs.dedup_by_key(|entry| entry.appid);
        // Les dates posées à l'activation traversent la relève : elles sont
        // plus fiables que celles que les licences et les lots font deviner.
        let stamped = previous.stamped.clone();
        for entry in &mut entries {
            if let Some(date) = stamped.get(&entry.appid) {
                entry.acquired = Some(*date);
            }
        }
        let catalog = lapinux_steam::catalog::Catalog {
            version: lapinux_steam::catalog::VERSION,
            account: account.to_string(),
            updated: lapinux_core::util::now_unix(),
            entries,
            dlcs,
            stamped,
        };
        catalog
            .save(&path)
            .map_err(|e| lapinux_core::Error::Other(format!("catalogue non enregistré : {e}")))?;
        tracing::info!(
            "catalogue : {} jeux et {} contenus additionnels enregistrés",
            catalog.entries.len(),
            catalog.dlcs.len()
        );
        Ok(())
    }

    /// Démarre la connexion Steam en arrière-plan avec Lapinux (mot de passe,
    /// Steam Guard, jeton de refresh). Rendre la main tout de suite permet
    /// d'attendre un code Steam Guard sans bloquer l'interface ; l'état est
    /// relu par `SteamStatus`.
    pub fn steam_login(
        self: &Arc<Self>,
        username: &str,
        password: &str,
        guard: Option<&str>,
        token: Option<&str>,
    ) -> Result<Envelope> {
        let username = username.trim().to_string();
        if username.is_empty() {
            return Err(lapinux_core::Error::Other(
                "renseignez votre compte Steam".into(),
            ));
        }
        {
            let mut state = self.login.lock().map_err(|_| {
                lapinux_core::Error::Other("état de connexion Steam indisponible".into())
            })?;
            if state.running {
                return Err(lapinux_core::Error::Other(
                    "une connexion Steam est déjà en cours".into(),
                ));
            }
            *state = SteamLoginState {
                running: true,
                stage: "connexion".into(),
                ..Default::default()
            };
        }
        self.login_cancel.store(false, Ordering::SeqCst);
        if let Ok(mut slot) = self.login_guard.lock() {
            *slot = None;
        }

        let shared = Arc::clone(self);
        let password = password.to_string();
        let guard = guard.map(str::to_string);
        let token = token
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .map(str::to_string);
        std::thread::spawn(move || shared.run_steam_login(username, password, guard, token));

        Ok(Envelope::ok(&serde_json::json!({ "pending": true })))
    }

    /// Connexion par QR code : aucune saisie de mot de passe, l'application
    /// Steam du téléphone approuve le défi. L'URL est publiée dans l'état dès
    /// que Steam la rend, l'interface n'a qu'à afficher le QR.
    pub fn steam_login_qr(self: &Arc<Self>) -> Result<Envelope> {
        {
            let mut state = self.login.lock().map_err(|_| {
                lapinux_core::Error::Other("état de connexion Steam indisponible".into())
            })?;
            if state.running {
                return Err(lapinux_core::Error::Other(
                    "une connexion Steam est déjà en cours".into(),
                ));
            }
            *state = SteamLoginState {
                running: true,
                stage: "connexion par QR".into(),
                ..Default::default()
            };
        }
        self.login_cancel.store(false, Ordering::SeqCst);
        if let Ok(mut slot) = self.login_guard.lock() {
            *slot = None;
        }
        let shared = Arc::clone(self);
        std::thread::spawn(move || shared.run_qr_login());
        Ok(Envelope::ok(&serde_json::json!({ "pending": true })))
    }

    /// Enregistre le jeton dès l'approbation du téléphone. Si le logon CM
    /// échoue ensuite (rate-limit, coupure), le scan n'est pas perdu : le jeton
    /// reste utilisable par le runtime Steam et les lectures Web.
    fn remember_qr_token(&self, tokens: &cm::auth::Tokens) {
        let name = if tokens.account_name.trim().is_empty() {
            match session::load_from(&self.session_path()) {
                Some(saved) if saved.steam_id == tokens.steam_id => saved.account_name,
                _ => {
                    tracing::warn!(
                        "jeton QR approuvé pour steamid {} sans nom de compte ; non enregistré",
                        tokens.steam_id
                    );
                    return;
                }
            }
        } else {
            tokens.account_name.clone()
        };
        let saved = SavedSession {
            account_name: name.clone(),
            steam_id: tokens.steam_id,
            refresh_token: tokens.refresh_token.clone(),
            guard_data: tokens.guard_data.clone(),
            saved_at: None,
        };
        match session::save_to(&self.session_path(), &saved) {
            Ok(()) => tracing::info!(
                "jeton QR enregistré pour {name} (steamid {}), logon CM en cours",
                tokens.steam_id
            ),
            Err(error) => tracing::warn!("jeton QR non enregistré : {error}"),
        }
    }

    /// Défi QR puis attente de l'approbation ; le logon suit sur la même
    /// connexion, comme pour la connexion par mot de passe.
    fn run_qr_login(self: Arc<Self>) {
        tracing::info!("connexion Steam par QR demandée");
        let config = cm::Config::from_env();
        let mut on_url = |url: &str| {
            self.update_login(|state| {
                state.qr = Some(url.to_string());
                state.stage = "QR à scanner".into();
            });
        };
        let cancelled = || self.login_aborted();
        let mut on_tokens = |tokens: &cm::auth::Tokens| self.remember_qr_token(tokens);
        match cm::sign_in_qr_with_tokens(&config, &mut on_url, &cancelled, &mut on_tokens) {
            Ok(session) => {
                if self.login_aborted() {
                    return;
                }
                let mut name = session.account_name.clone().unwrap_or_default();
                if name.trim().is_empty() {
                    // Le poll QR omet parfois le nom : le jeton, lui, porte le
                    // SteamID, qui suffit à retrouver le compte déjà connu.
                    name = session::load_from(&self.session_path())
                        .filter(|saved| saved.steam_id == session.steam_id)
                        .map(|saved| saved.account_name)
                        .unwrap_or_default();
                }
                if name.trim().is_empty() {
                    self.finish_login(SteamLoginState {
                        running: false,
                        stage: "erreur".into(),
                        error: Some("QR approuvé mais Steam n'a pas rendu le compte".into()),
                        ..Default::default()
                    });
                    return;
                }
                self.finish_steam_session(&name, &session);
            }
            Err(error) => {
                if self.login_aborted() {
                    return;
                }
                tracing::warn!("connexion Steam par QR impossible : {error}");
                self.finish_login(SteamLoginState {
                    running: false,
                    stage: "erreur".into(),
                    error: Some(message_utilisateur(&error)),
                    ..Default::default()
                });
            }
        }
    }

    /// Annule la connexion en cours : le fil réseau s'arrête dès qu'il rend la
    /// main, l'interface est libérée tout de suite et l'état n'est plus écrasé
    /// par une tentative tardive.
    pub fn steam_login_cancel(&self) -> Result<Envelope> {
        let running = self.login.lock().map(|state| state.running).unwrap_or(false);
        if !running {
            return Err(lapinux_core::Error::Other(
                "aucune connexion Steam n'est en cours".into(),
            ));
        }
        self.login_cancel.store(true, Ordering::SeqCst);
        if let Ok(mut slot) = self.login_guard.lock() {
            *slot = None;
        }
        self.finish_login(SteamLoginState {
            stage: "annulé".into(),
            message: Some("connexion Steam annulée".into()),
            ..Default::default()
        });
        Ok(Envelope::ok(&self.steam_status()))
    }

    fn login_aborted(&self) -> bool {
        self.login_cancel.load(Ordering::SeqCst)
    }

    /// Transmet le code Steam Guard demandé par la connexion en cours.
    pub fn steam_login_guard(&self, code: &str) -> Result<Envelope> {
        let code = code.trim();
        if code.is_empty() {
            return Err(lapinux_core::Error::Other(
                "saisissez le code Steam Guard".into(),
            ));
        }
        let sender = self
            .login_guard
            .lock()
            .ok()
            .and_then(|mut slot| slot.take());
        let Some(sender) = sender else {
            return Err(lapinux_core::Error::Other(
                "aucun code Steam Guard n'est attendu".into(),
            ));
        };
        sender.send(code.to_string()).map_err(|_| {
            lapinux_core::Error::Other("la connexion Steam a expiré".into())
        })?;
        // L'état est mis à jour par la connexion elle-même : un code faux peut
        // redemander immédiatement un nouveau code, et l'interface doit voir
        // cette demande plutôt qu'un état « vérification » écrasé après coup.
        Ok(Envelope::unit())
    }

    fn run_steam_login(
        self: Arc<Self>,
        username: String,
        password: String,
        guard: Option<String>,
        token: Option<String>,
    ) {
        tracing::info!("connexion Steam demandée pour {username}");
        let mut initial_guard = guard;
        let mut failures = Vec::new();
        let mut password_error = None;

        // 1. Jeton Lapinux collé dans l'écran de connexion : il remplace le mot
        // de passe et sert de repli à la session enregistrée sur le disque.
        if let Some(token) = token.as_deref() {
            self.update_login(|state| state.stage = "connexion par jeton Lapinux".into());
            match self.sign_in_with_token(&username, token) {
                Ok(session) => {
                    if self.login_aborted() {
                        return;
                    }
                    self.finish_steam_session(&username, &session);
                    return;
                }
                Err(error) => failures.push(format!("jeton fourni : {error}")),
            }
            if self.login_aborted() {
                return;
            }
        }

        // 2. Jeton Lapinux déjà enregistré : reconnexion sans mot de passe.
        if let Some(saved) = self.saved_session(&username) {
            self.update_login(|state| state.stage = "jeton enregistré".into());
            match self.sign_in_with_token(&username, &saved.refresh_token) {
                Ok(session) => {
                    if self.login_aborted() {
                        return;
                    }
                    self.finish_steam_session(&username, &session);
                    return;
                }
                Err(error) => failures.push(format!("jeton Lapinux : {error}")),
            }
            if self.login_aborted() {
                return;
            }
        }

        // 3. Identifiants classiques : le service Authentication rend un jeton.
        if !password.is_empty() {
            let config = cm::Config::from_env();
            self.update_login(|state| state.stage = "connexion Lapinux".into());
            let guard_data = self.saved_session(&username).and_then(|saved| saved.guard_data);
            let auth = cm::Auth::Account {
                name: username.clone(),
                password: password.clone(),
                guard_data,
            };
            let mut prompt = |kind, retry| {
                self.steam_guard_prompt(kind, retry, &mut initial_guard)
            };
            let result = cm::sign_in(&config, &auth, &mut prompt);
            match result {
                Ok(session) => {
                    if self.login_aborted() {
                        return;
                    }
                    self.finish_steam_session(&username, &session);
                    return;
                }
                Err(error) => password_error = Some(message_utilisateur(&error)),
            }
        }
        if self.login_aborted() {
            return;
        }
        // L'écran montre la tentative qui compte : un jeton enregistré refusé
        // ne doit pas enterrer l'erreur du mot de passe saisi, et inversement
        // quand aucun mot de passe n'a été fourni.
        let error = match password_error {
            Some(error) => error,
            None if !failures.is_empty() => failures.join(" · "),
            None => "renseignez votre mot de passe Steam pour vous connecter".into(),
        };
        tracing::warn!("connexion Steam impossible pour {username} : {error}");
        self.finish_login(SteamLoginState {
            running: false,
            stage: "erreur".into(),
            error: Some(error),
            ..Default::default()
        });
    }

    /// Reconnexion sans mot de passe par un jeton de refresh Lapinux. Une
    /// connexion par jeton ne déclenche pas Steam Guard : si le serveur en
    /// demande un, le jeton est refusé et l'appelant essaye autre chose.
    fn sign_in_with_token(
        &self,
        username: &str,
        refresh_token: &str,
    ) -> std::result::Result<cm::Session, cm::Error> {
        let config = cm::Config::from_env();
        let auth = cm::Auth::Token {
            name: username.to_string(),
            refresh_token: refresh_token.to_string(),
        };
        let mut prompt = |_, _| Err(cm::Error::Auth("aucun code Steam Guard attendu".into()));
        match cm::sign_in(&config, &auth, &mut prompt) {
            // Un jeton qui échoue n'est pas forcément mort : Steam répond
            // parfois « mot de passe invalide » quand une autre session du
            // compte vient de s'ouvrir. Le journal garde la réponse exacte et
            // l'appelant passe au mot de passe.
            Err(error) => {
                tracing::warn!("connexion par jeton Lapinux refusée : {error}");
                Err(error)
            }
            Ok(session) => {
                // Steam fait tourner le jeton de refresh quand on ouvre une
                // session : la version rendue remplace celle du disque, sinon
                // l'ancienne meurt à la rotation suivante et le compte finit
                // par refuser toute connexion (« mot de passe invalide »).
                if let Some(renewed) = session
                    .refresh_token
                    .as_deref()
                    .filter(|renewed| !renewed.trim().is_empty() && *renewed != refresh_token)
                {
                    let saved = SavedSession {
                        account_name: username.to_string(),
                        steam_id: session.steam_id,
                        refresh_token: renewed.to_string(),
                        guard_data: session.guard_data.clone(),
                        saved_at: None,
                    };
                    match session::save_to(&self.session_path(), &saved) {
                        Ok(_) => tracing::info!("jeton Steam tourné par la session, enregistré"),
                        Err(error) => tracing::warn!("jeton Steam tourné non enregistré : {error}"),
                    }
                }
                Ok(session)
            }
        }
    }

    /// Demande le code Steam Guard à l'interface et attend sa réponse. Le
    /// premier code fourni dans le formulaire est consommé sans attente ; les
    /// suivants arrivent par `SteamLoginGuard`.
    fn steam_guard_prompt(
        &self,
        kind: cm::Guard,
        retry: bool,
        initial: &mut Option<String>,
    ) -> std::result::Result<String, cm::Error> {
        let (guard, stage, message) = match kind {
            cm::Guard::EmailCode => (
                SteamGuardKind::EmailCode,
                "code Steam Guard (e-mail)",
                "Saisissez le code envoyé à l'adresse e-mail du compte.",
            ),
            cm::Guard::TwoFactor => (
                SteamGuardKind::TwoFactor,
                "code Steam Guard (application)",
                "Saisissez le code affiché par l'application Steam.",
            ),
            cm::Guard::DeviceConfirmation => (
                SteamGuardKind::DeviceConfirmation,
                "confirmation dans l'application Steam",
                "Confirmez la connexion dans l'application Steam ; l'écran attend la réponse.",
            ),
            cm::Guard::EmailConfirmation => (
                SteamGuardKind::EmailConfirmation,
                "confirmation par e-mail",
                "Confirmez la connexion via le lien envoyé par e-mail ; l'écran attend la réponse.",
            ),
        };
        let wants_code = matches!(kind, cm::Guard::EmailCode | cm::Guard::TwoFactor);
        let using_initial = wants_code && initial.is_some();
        // Le canal est installé avant de publier l'état : dès que l'interface
        // voit la demande, elle peut envoyer le code sans course.
        let receiver = if wants_code && !using_initial {
            let (sender, receiver) = mpsc::channel();
            if let Ok(mut slot) = self.login_guard.lock() {
                *slot = Some(sender);
            }
            Some(receiver)
        } else {
            None
        };

        self.update_login(|state| {
            state.stage = if using_initial {
                "vérification du code Steam Guard".into()
            } else {
                stage.into()
            };
            state.guard = if using_initial { None } else { Some(guard) };
            state.guard_message = Some(message.into());
            state.guard_retry = retry;
        });

        if wants_code {
            if let Some(code) = initial.take() {
                return Ok(code);
            }
            let Some(receiver) = receiver else {
                return Err(cm::Error::Auth("code Steam Guard indisponible".into()));
            };
            // Attente par tranches : une annulation doit rendre la main sans
            // attendre les trois minutes du délai Steam Guard.
            let deadline = Instant::now() + Duration::from_secs(180);
            let result = loop {
                if self.login_aborted() {
                    break Err(cm::Error::Auth("connexion annulée".into()));
                }
                match receiver.recv_timeout(Duration::from_millis(200)) {
                    Ok(code) => break Ok(code),
                    Err(mpsc::RecvTimeoutError::Timeout) if Instant::now() < deadline => continue,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        break Err(cm::Error::Auth("délai Steam Guard dépassé".into()))
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        break Err(cm::Error::Auth("connexion annulée".into()))
                    }
                }
            };
            if let Ok(mut slot) = self.login_guard.lock() {
                *slot = None;
            }
            if result.is_ok() {
                self.update_login(|state| {
                    state.guard = None;
                    state.guard_retry = false;
                    state.stage = "vérification du code".into();
                });
            }
            return result;
        }
        Ok(String::new())
    }

    fn finish_steam_session(self: &Arc<Self>, username: &str, session: &cm::Session) {
        // Une annulation pendant le réseau ne doit ni enregistrer de jeton ni
        // écraser l'état « annulé » déjà affiché.
        if self.login_aborted() {
            return;
        }
        let mut notes = Vec::new();
        if let Some(refresh) = session.refresh_token.as_deref() {
            let saved = SavedSession {
                account_name: username.to_string(),
                steam_id: session.steam_id,
                refresh_token: refresh.to_string(),
                guard_data: session.guard_data.clone(),
                saved_at: None,
            };
            if let Err(error) = session::save_to(&self.session_path(), &saved) {
                notes.push(format!("jeton non enregistré : {error}"));
            }
        }
        let mut config = self.config_snapshot();
        config.steam.username = username.to_string();
        let _ = self.save_config(config);
        // Le jeton qui vient d'être obtenu est valide par construction ; la
        // vérification n'a pas à retéléphoner à Steam.
        self.mark_token_valid();

        tracing::info!(
            "connecté via Lapinux ({username}, steamid {})",
            session.steam_id
        );
        let message = if notes.is_empty() {
            "connecté via Lapinux ; jeton réutilisable".to_string()
        } else {
            format!("connecté via Lapinux ; {}", notes.join(" ; "))
        };
        self.finish_login(SteamLoginState {
            running: false,
            connected: true,
            backend: Some("Lapinux".into()),
            stage: "terminé".into(),
            message: Some(message),
            ..Default::default()
        });
        // Le jeton vient d'être remplacé : la session sociale repart avec lui,
        // et le compte devient visible de ses amis.
        self.restart_social_in_background();
    }

    /// Télécharge un jeu possédé ou met à jour une installation existante,
    /// dans la bibliothèque où le jeu se trouve déjà.
    ///
    /// `platform` est la version choisie dans le menu du bouton (`linux`,
    /// `windows`, `macos`) ; `None` garde la préférence du moteur.
    pub fn install_game(
        self: &Arc<Self>,
        appid: u32,
        dlc: Vec<u32>,
        platform: Option<String>,
    ) -> Result<Envelope> {
        let platform = normalize_platform(platform);
        let config = self.config_snapshot();
        if config.steam.username.trim().is_empty() {
            return Err(lapinux_core::Error::Other(
                "connectez d'abord votre compte Steam dans Téléchargements".into(),
            ));
        }
        let saved = self.saved_session(&config.steam.username).ok_or_else(|| {
            lapinux_core::Error::Other(
                "connectez votre compte Steam dans Réglages → Compte Steam".into(),
            )
        })?;
        if saved.refresh_token.trim().is_empty() {
            return Err(lapinux_core::Error::Other(
                "session Lapinux sans jeton : reconnectez-vous".into(),
            ));
        }

        let busy = self
            .jobs
            .lock()
            .map(|jobs| {
                jobs.iter().any(|job| {
                    job.appid == Some(appid)
                        && matches!(
                            job.state,
                            JobState::Running | JobState::Queued | JobState::Paused
                        )
                })
            })
            .unwrap_or(false);
        if busy {
            return Err(lapinux_core::Error::Other(
                "un téléchargement est déjà en cours pour ce jeu".into(),
            ));
        }

        let existing = self.store.game(appid).ok().flatten();
        // Mettre à jour un jeu déjà installé se fait dans SA bibliothèque :
        // un jeu Steam garde son dossier, son préfixe et ses fichiers.
        let installed_here = existing.as_ref().filter(|game| game.installed);
        let updating = installed_here.is_some();
        // Même un manifeste incomplet indique la bibliothèque à réparer.
        // Une bibliothèque gérée par Lapinux n'a plus de `steamapps` : ses
        // jeux vivent sous `Steam`, à côté des manifestes. Seule une vraie
        // bibliothèque Steam garde `steamapps/common`.
        let managed_layout = existing
            .as_ref()
            .map(|game| game.origin != lapinux_core::model::GameOrigin::Steam)
            .unwrap_or(true);
        let library_root = existing
            .as_ref()
            .map(|game| game.library.clone())
            .filter(|library| {
                library.join("steamapps").is_dir() || library.join("Steam").is_dir()
            })
            .unwrap_or_else(|| config.library_root());
        let games_root = if managed_layout {
            library_root.join("Steam")
        } else {
            library_root.join("steamapps").join("common")
        };
        let catalog = lapinux_steam::catalog::Catalog::load(&self.catalog_path());
        let catalog_entry = catalog.get(appid);
        let catalog_build = catalog_entry.and_then(|entry| entry.details.build_id);
        // Noms et poids des contenus demandés, pour leur écrire un manifeste
        // d'installation une fois les fichiers en place : c'est ce manifeste
        // qui dira ensuite, dans la fiche du jeu, que le contenu est déjà là.
        let extra_names: Vec<(u32, String, u64)> = dlc
            .iter()
            .filter(|id| **id != appid)
            .map(|id| {
                let entry = catalog.dlc(*id);
                (
                    *id,
                    entry
                        .map(|dlc| dlc.name.clone())
                        .filter(|name| !name.trim().is_empty())
                        .unwrap_or_else(|| format!("Contenu {id}")),
                    entry.map(|dlc| dlc.install).unwrap_or(0),
                )
            })
            .collect();
        let name = existing
            .as_ref()
            .map(|game| game.name.clone())
            .filter(|name| !name.is_empty())
            .or_else(|| catalog_entry.map(|entry| entry.display_name()))
            .unwrap_or_else(|| format!("App {appid}"));
        // En mode « un seul jeu à la fois », une demande qui arrive pendant
        // qu'une autre installation tourne rejoint la file. Le pompage la
        // reprend dès que la place se libère.
        let parallel = config.downloads.parallel_installs;
        // En mode « un seul jeu à la fois », seul un téléchargement qui avance
        // occupe le créneau : une tâche mise en pause par l'utilisateur ne
        // bloque pas la suivante.
        let active = self
            .jobs
            .lock()
            .map(|jobs| {
                jobs.iter().any(|job| {
                    job.kind == JobKind::InstallGame && job.state == JobState::Running
                })
            })
            .unwrap_or(false);
        let queued = !parallel && active;
        let job_id = format!("game-{appid}-{}", lapinux_core::util::now_unix());
        let job = Job {
            id: job_id.clone(),
            kind: JobKind::InstallGame,
            label: format!("{name} ({appid})"),
            title: name.clone(),
            appid: Some(appid),
            state: if queued {
                JobState::Queued
            } else {
                JobState::Running
            },
            progress: 0.0,
            downloaded: 0,
            total: 0,
            network: 0,
            written: 0,
            server: String::new(),
            depot: 0,
            depots: 0,
            updating,
            dlc: dlc.clone(),
            platform: platform.clone(),
            message: if queued {
                "en attente".to_string()
            } else if updating {
                "mise à jour".to_string()
            } else {
                "téléchargement".to_string()
            },
            backend: "Lapinux".to_string(),
            started: lapinux_core::util::now_unix(),
            finished: None,
            report: None,
        };
        if let Ok(mut jobs) = self.jobs.lock() {
            // Une carte arrêtée du même jeu est remplacée par celle qui
            // repart : sans cela, la reprise laissait l'ancienne en
            // « interrompu » à côté de la nouvelle, et la section « En pause »
            // ne se vidait jamais.
            jobs.retain(|old| {
                old.appid != Some(appid)
                    || !matches!(
                        old.state,
                        JobState::Interrupted | JobState::Failed | JobState::Cancelled
                    )
            });
            jobs.push(job);
        }
        self.persist_jobs(true);
        if queued {
            return Ok(Envelope::ok(&serde_json::json!({
                "job": job_id,
                "queued": true,
            })));
        }
        let cancel = Arc::new(AtomicBool::new(false));
        if let Ok(mut map) = self.cancel.lock() {
            map.insert(job_id.clone(), Arc::clone(&cancel));
        }
        let pause = Arc::new(AtomicBool::new(false));
        if let Ok(mut map) = self.pause.lock() {
            map.insert(job_id.clone(), Arc::clone(&pause));
        }

        let shared = Arc::clone(self);
        let download_limit = self.throttle.limit("steam");
        // Serveur CDN choisi dans les réglages de téléchargement : il passe en
        // tête de liste, sans bannir les autres.
        let preferred_server = self
            .config_snapshot()
            .downloads
            .steam_server
            .trim()
            .to_string();
        let preferred_server = if preferred_server.is_empty() {
            None
        } else {
            Some(preferred_server)
        };
        let job_for_thread = job_id.clone();
        let job_dlc = dlc;
        let job_platform = platform;
        // Langue retenue pour ce jeu : la sienne, sinon celle des réglages
        // généraux. Elle choisit les dépôts de doublage téléchargés.
        let language = self
            .store
            .game(appid)
            .ok()
            .flatten()
            .and_then(|game| game.settings.language)
            .unwrap_or_default();
        let language = lapinux_core::lang::resolve(
            &language,
            &self.config_snapshot().general.game_language,
        )
        .map(|language| language.code);
        let account = saved.account_name.clone();
        let refresh_token = saved.refresh_token.clone();
        std::thread::spawn(move || {
            // Journal détaillé du transfert : chaque chunk, chaque pause, et le
            // débit réellement reçu par la machine. Il répond à la question
            // « est-ce que le téléchargement continue après l'arrêt ? ».
            let log = lapinux_steam::download_log::DownloadLog::open(
                &shared.paths.log_dir.join("downloads.log"),
            );
            log.write(format!(
                "tâche {job_for_thread} · appid={appid} · « {name} » · version {} · {} · bibliothèque={} · mise à jour={updating}",
                job_platform.as_deref().unwrap_or("automatique"),
                if cancel.load(Ordering::Relaxed) {
                    "annulée avant démarrage"
                } else {
                    "démarrage"
                },
                library_root.display()
            ));
            tracing::info!("journal de téléchargement : {}", log.path().display());
            let mut installed_name = name.clone();
            // Dossier réel du jeu, connu dès les métadonnées : une annulation
            // d'installation neuve le supprimera en entier.
            let mut installed_dir: Option<String> = None;
            let mut on_event = |event: InstallEvent| match event {
                InstallEvent::Metadata { meta } => {
                    installed_name = meta.name.clone();
                    installed_dir = Some(meta.installdir.clone());
                    shared.update_job(&job_for_thread, |job| {
                        job.title = meta.name.clone();
                        job.label = format!("{} ({appid})", meta.name);
                        job.message = "préparation".to_string();
                    });
                }
                InstallEvent::Status { message } => {
                    shared.update_job(&job_for_thread, |job| {
                        job.message = message;
                    });
                }
                InstallEvent::Progress {
                    depot,
                    depots,
                    fraction,
                    downloaded,
                    total,
                    network,
                    written,
                    server,
                    label,
                } => {
                    // La fraction est déjà globale au build : l'afficher
                    // telle quelle évite un pourcentage qui repart à zéro à
                    // chaque dépôt.
                    let overall = fraction.clamp(0.0, 1.0);
                    shared.update_job(&job_for_thread, |job| {
                        job.progress = overall;
                        job.downloaded = downloaded;
                        job.total = total;
                        job.network = network;
                        job.written = written;
                        job.server = server;
                        job.depot = depot + 1;
                        job.depots = depots;
                        job.message = label;
                    });
                }
            };

            let result = if cancel.load(Ordering::Relaxed) {
                Err(lapinux_steam::Error::Other("annulé".into()))
            } else {
                native::install(
                    &account,
                    &refresh_token,
                    appid,
                    &job_dlc,
                    job_platform.as_deref(),
                    language,
                    &library_root,
                    &games_root,
                    Arc::clone(&cancel),
                    Arc::clone(&pause),
                    &log,
                    download_limit,
                    preferred_server.as_deref(),
                    &mut on_event,
                )
            };

            let cancelled = cancel.load(Ordering::Relaxed);
            log.write(format!(
                "tâche {job_for_thread} terminée · état={} · {}",
                if cancelled {
                    "annulée"
                } else if result.is_ok() {
                    "réussie"
                } else {
                    "échouée"
                },
                match &result {
                    Ok(outcome) => format!("{} o réseau", outcome.bytes),
                    Err(error) => error.to_string(),
                }
            ));
            match result {
                Ok(outcome) => {
                    let name = if outcome.meta.name.trim().is_empty() {
                        installed_name
                    } else {
                        outcome.meta.name.clone()
                    };
                    // Les contenus additionnels installés reçoivent eux aussi
                    // leur manifeste, comme le fait le client Steam : sans lui,
                    // la fiche du jeu les croirait absents du disque.
                    for (dlc_appid, dlc_name, dlc_size) in &extra_names {
                        if !outcome.apps.contains(dlc_appid) {
                            continue;
                        }
                        if let Err(err) = lapinux_steam::write_manifest(
                            &library_root,
                            *dlc_appid,
                            dlc_name,
                            &outcome.meta.installdir,
                            *dlc_size,
                            managed_layout,
                        ) {
                            tracing::debug!("manifeste du contenu {dlc_appid} non écrit : {err}");
                        }
                    }
                    match lapinux_steam::manifest::record_build(
                        &library_root,
                        appid,
                        &name,
                        &outcome.meta.installdir,
                        outcome.size,
                        outcome.build_id.or(catalog_build),
                        managed_layout,
                    ) {
                        Ok(_) => match shared.rescan_library_at(&library_root) {
                            Ok(_) => {
                                let cloud = if shared
                                    .config_snapshot()
                                    .downloads
                                    .cloud_saves_after_install
                                {
                                    shared.update_job(&job_for_thread, |job| {
                                        job.message = "sauvegardes cloud".into();
                                    });
                                    shared
                                        .pull_cloud_after_install(appid)
                                        .map(|note| format!(" · {note}"))
                                        .unwrap_or_default()
                                } else {
                                    String::new()
                                };
                                shared.update_job(&job_for_thread, |job| {
                                    job.state = JobState::Done;
                                    job.progress = 1.0;
                                    job.finished = Some(lapinux_core::util::now_unix());
                                    job.message = format!(
                                        "{} ({} o){}",
                                        if updating {
                                            "mis à jour"
                                        } else {
                                            "installé"
                                        },
                                        outcome.size,
                                        cloud
                                    );
                                });
                            }
                            Err(err) => shared.update_job(&job_for_thread, |job| {
                                job.state = JobState::Failed;
                                job.finished = Some(lapinux_core::util::now_unix());
                                job.message = format!(
                                    "fichiers installés mais bibliothèque non actualisée : {err}"
                                );
                            }),
                        },
                        Err(err) => {
                            shared.update_job(&job_for_thread, |job| {
                                job.state = JobState::Failed;
                                job.finished = Some(lapinux_core::util::now_unix());
                                job.message = format!(
                                    "fichiers téléchargés mais manifeste impossible : {err}"
                                );
                            });
                        }
                    }
                }
                Err(err) => {
                    shared.update_job(&job_for_thread, |job| {
                        job.state = if cancelled {
                            JobState::Cancelled
                        } else {
                            JobState::Failed
                        };
                        job.finished = Some(lapinux_core::util::now_unix());
                        job.message = err.to_string();
                    });
                    if cancelled && !updating {
                        if let Some(dir) = installed_dir.as_deref() {
                            remove_partial_install(&games_root, dir);
                        }
                    }
                }
            }
            if let Ok(mut map) = shared.cancel.lock() {
                map.remove(&job_for_thread);
            }
            if let Ok(mut map) = shared.pause.lock() {
                map.remove(&job_for_thread);
            }
            // La place est libre : la file peut lancer la tâche suivante.
            shared.pump_install_queue();
            if cancelled {
                // Preuve horodatée : le moteur est arrêté, le compteur du
                // journal est figé. On mesure six secondes de trafic machine
                // pour montrer s'il reste autre chose sur la connexion.
                log.write("annulation terminée · mesure du trafic machine pendant 6 s");
                let mut previous = lapinux_steam::download_log::system_rx_bytes();
                for second in 1..=6 {
                    std::thread::sleep(Duration::from_secs(1));
                    let now = lapinux_steam::download_log::system_rx_bytes();
                    if let (Some(before), Some(after)) = (previous, now) {
                        let delta = after.saturating_sub(before);
                        log.write(format!(
                            "post-annulation {second} s · machine +{delta} o ({:.2} MiB/s)",
                            lapinux_steam::download_log::rate_mib(delta, 1000)
                        ));
                    }
                    previous = now;
                }
                log.write("fin des mesures post-annulation");
            }
        });

        Ok(Envelope::ok(&serde_json::json!({ "job": job_id })))
    }

    /// Lance les tâches en attente. En mode simultané, toutes partent ; en
    /// mode file d'attente, une seule, les autres gardent leur place.
    pub fn pump_install_queue(self: &Arc<Self>) {
        let parallel = self.config_snapshot().downloads.parallel_installs;
        loop {
            let next = {
                let Ok(jobs) = self.jobs.lock() else {
                    return;
                };
                next_queued_install(&jobs, parallel)
            };
            let Some(job) = next else {
                return;
            };
            let Some(appid) = job.appid else {
                // Une entrée sans jeu ne doit pas bloquer la file.
                if let Ok(mut jobs) = self.jobs.lock() {
                    jobs.retain(|entry| entry.id != job.id);
                }
                self.persist_jobs(true);
                continue;
            };
            // La demande en attente est remplacée par une vraie tâche Running :
            // install_game recrée le job avec son heure de départ réelle.
            if let Ok(mut jobs) = self.jobs.lock() {
                jobs.retain(|entry| entry.id != job.id);
            }
            self.persist_jobs(true);
            if let Err(error) = self.install_game(appid, job.dlc.clone(), job.platform.clone()) {
                let mut failed = job;
                failed.state = JobState::Failed;
                failed.finished = Some(lapinux_core::util::now_unix());
                failed.message = error.to_string();
                if let Ok(mut jobs) = self.jobs.lock() {
                    jobs.push(failed);
                }
                self.persist_jobs(true);
            }
            if !parallel {
                return;
            }
        }
    }

    /// Désinstalle un jeu : fichiers, préfixe Proton et manifeste. Refuse
    /// pendant une session ou un téléchargement, et ne supprime jamais un
    /// chemin qui n'est pas exactement `<bibliothèque>/steamapps/common/<jeu>`.
    pub fn uninstall_game(&self, appid: u32) -> Result<Envelope> {
        let game = self
            .store
            .game(appid)
            .map_err(|e| lapinux_core::Error::Other(e.to_string()))?
            .ok_or_else(|| lapinux_core::Error::Other("jeu introuvable".into()))?;
        if !game.installed {
            return Err(lapinux_core::Error::Other(
                "le jeu n'est pas installé".into(),
            ));
        }
        if self
            .session_info()
            .map(|session| session.appid == appid)
            .unwrap_or(false)
        {
            return Err(lapinux_core::Error::Other(
                "arrêtez le jeu avant de le désinstaller".into(),
            ));
        }
        if self
            .jobs
            .lock()
            .map(|jobs| {
                jobs.iter().any(|job| {
                    job.appid == Some(appid)
                        && matches!(
                            job.state,
                            JobState::Running | JobState::Queued | JobState::Paused
                        )
                })
            })
            .unwrap_or(false)
        {
            return Err(lapinux_core::Error::Other(
                "un téléchargement est en cours pour ce jeu".into(),
            ));
        }

        // Un jeu de boutique vit sous le rayon de sa boutique, à la racine de
        // la bibliothèque ; un jeu Steam dans `steamapps/common` ; un jeu
        // Lapinux sous `Steam`. Le contrôle est le même pour tous : aucun
        // `..`, et le dossier doit rester sous la racine de la bibliothèque.
        let common = if game.origin == lapinux_core::model::GameOrigin::Store
            && game.store != lapinux_core::stores::Store::Steam
        {
            game.library.clone()
        } else if game.origin == lapinux_core::model::GameOrigin::Steam {
            game.library.join("steamapps").join("common")
        } else {
            game.library.join("Steam")
        };
        let dir = game.install_dir();
        let name = game.installdir.trim();
        let safe_name = !name.is_empty()
            && name
                .split('/')
                .all(|part| !part.is_empty() && part != "." && part != "..")
            && !name.contains('\\');
        let inside_library = dir
            .strip_prefix(&common)
            .map(|rest| !rest.as_os_str().is_empty())
            .unwrap_or(false);
        if !safe_name || !inside_library {
            return Err(lapinux_core::Error::Other(format!(
                "dossier d'installation inattendu : {}",
                dir.display()
            )));
        }

        // Un lien symbolique est retiré lui-même, jamais sa cible.
        if let Ok(metadata) = std::fs::symlink_metadata(&dir) {
            if metadata.file_type().is_symlink() {
                std::fs::remove_file(&dir)?;
            } else if metadata.is_dir() {
                std::fs::remove_dir_all(&dir)?;
            }
        }

        // Le rayon d'une boutique vidé de son dernier jeu ne reste pas : un
        // dossier vide à la racine se lirait comme une bibliothèque fantôme.
        if game.origin == lapinux_core::model::GameOrigin::Store
            && game.store != lapinux_core::stores::Store::Steam
        {
            if let Some(shelf) = dir.parent() {
                if shelf != game.library.as_path() {
                    let _ = std::fs::remove_dir(shelf);
                }
            }
        }

        // Préfixe Proton : Steam dans `steamapps/compatdata/`, les jeux de
        // Lapinux et des boutiques dans le `CompatData/` lisible de leur
        // bibliothèque, l'ancien rangement sous les données ou `compatdata/`.
        // Aucun autre chemin n'est supprimé.
        let mut prefixes = Vec::new();
        if let Some(prefix) = &game.prefix {
            prefixes.push(prefix.clone());
        }
        prefixes.push(self.paths.data_dir.join("prefixes").join(appid.to_string()));
        prefixes.push(
            game.library
                .join("steamapps")
                .join("compatdata")
                .join(appid.to_string()),
        );
        prefixes.push(game.library.join("compatdata").join(appid.to_string()));
        if !game.library.as_os_str().is_empty() {
            let compat = game.library.join("CompatData");
            prefixes.push(compat.join(lapinux_proton::prefix_name_for(&game)));
            // Le jeu a pu être renommé : son dossier garde l'ancien nom et
            // son identifiant. Tous les candidats de cet identifiant partent,
            // sinon un préfixe orphelin resterait après la désinstallation.
            let suffix = format!(" ({appid})");
            if let Ok(entries) = std::fs::read_dir(&compat) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let matches = path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .map(|name| name.ends_with(&suffix))
                        .unwrap_or(false);
                    if matches && path.is_dir() {
                        prefixes.push(path);
                    }
                }
            }
        }
        for prefix in prefixes {
            let lapinux_prefix = prefix.starts_with(self.paths.data_dir.join("prefixes"));
            let steam_prefix = prefix.starts_with(
                game.library.join("steamapps").join("compatdata"),
            );
            let library_prefix = prefix.starts_with(game.library.join("compatdata"));
            let readable_prefix = prefix.starts_with(game.library.join("CompatData"));
            if (lapinux_prefix || steam_prefix || library_prefix || readable_prefix)
                && prefix.exists()
            {
                let _ = std::fs::remove_dir_all(&prefix);
            }
        }

        // Le manifeste a suivi les jeux : `Steam` pour la bibliothèque gérée,
        // `steamapps` pour une vraie bibliothèque Steam.
        for manifest in [
            game.library
                .join("Steam")
                .join(format!("appmanifest_{appid}.acf")),
            game.library
                .join("steamapps")
                .join(format!("appmanifest_{appid}.acf")),
        ] {
            if manifest.is_file() {
                let _ = std::fs::remove_file(&manifest);
            }
        }
        self.store
            .remove_game(appid)
            .map_err(|e| lapinux_core::Error::Other(e.to_string()))?;
        // Pas de scan complet ici : seule l'entrée désinstallée doit bouger,
        // les autres jeux de la bibliothèque restent tels quels.
        tracing::info!(
            "jeu {appid} désinstallé : {} · préfixe et manifeste supprimés",
            dir.display()
        );
        Ok(Envelope::ok(&serde_json::json!({
            "appid": appid,
            "removed": dir.display().to_string(),
        })))
    }

    /// Prépare et lance un jeu installé. Pour un jeu Steamworks, le client
    /// Steam est démarré en mode headless si l'option est active, puis le
    /// lancement attend qu'il soit prêt.
    pub fn launch(self: &Arc<Self>, appid: u32, dry_run: bool) -> Result<Envelope> {
        // Relit le fichier : un réglage modifié à la main (Proton, raccourci)
        // doit s'appliquer à cette session, pas rester celui du démarrage.
        self.reload_config();
        let game = self
            .store
            .game(appid)
            .map_err(|e| lapinux_core::Error::Other(e.to_string()))?
            .ok_or_else(|| lapinux_core::Error::Other("jeu introuvable".into()))?;
        if !game.installed {
            return Err(lapinux_core::Error::Other(
                "le jeu n'est pas installé dans une bibliothèque connue".into(),
            ));
        }
        if self
            .session
            .lock()
            .map(|s| s.is_some())
            .unwrap_or(false)
        {
            return Err(lapinux_core::Error::Other("une session est déjà en cours".into()));
        }
        let config = self.config_snapshot();
        let protons = self.protons_cached();
        let wanted = game
            .settings
            .proton
            .clone()
            .unwrap_or_else(|| config.launch.proton.clone());
        let proton = if game.native {
            None
        } else if wanted == "auto" {
            lapinux_proton::recommended(&protons).cloned()
        } else {
            protons.iter().find(|p| p.name == wanted).cloned()
        };
        if !game.native && proton.is_none() {
            return Err(lapinux_core::Error::Other(
                "aucun Proton utilisable : installez GE-Proton depuis l'écran Protons".into(),
            ));
        }
        let exe = game
            .exe
            .clone()
            .ok_or_else(|| lapinux_core::Error::Other("exécutable du jeu introuvable".into()))?;
        let prefix = lapinux_proton::prefix_for(&game, &self.paths.data_dir);
        lapinux_proton::ensure_prefix(&prefix, appid)
            .map_err(|e| lapinux_core::Error::Other(e.to_string()))?;

        // umu-run exige un Proton complet (toolmanifest.vdf) ; un Proton
        // partiel est lancé directement par son script, sans umu.
        let umu_ready = proton
            .as_ref()
            .map(|p| p.path.join("toolmanifest.vdf").is_file())
            .unwrap_or(false);
        // Un jeu Steam suit le chemin de Steam : Proton directement, avec le
        // `compatdata` de sa bibliothèque. umu-run reste aux jeux de boutique.
        // Un jeu Steam — même installé par Lapinux — garde le chemin Steam :
        // umu-run ne transmet ni SteamAppId ni le client Steam, et Steamworks
        // ne peut pas s'initialiser. Les jeux de boutique restent sur umu.
        let umu = game.origin != lapinux_core::model::GameOrigin::Steam
            && game.store != lapinux_core::stores::Store::Steam
            && game.prefix.is_none()
            && umu_ready
            && lapinux_proton::umu_run_path().is_some();
        // Langue du jeu : celle choisie pour lui, sinon celle des réglages
        // généraux. Wine et les jeux lisent la locale de l'environnement.
        let mut extra_env = game.settings.env.clone();
        // Les réglages DXVK du jeu passent par `DXVK_CONFIG` : une ligne
        // d'options, lue par DXVK au chargement de ses bibliothèques. Une
        // variable posée à la main par l'utilisateur l'emporte : elle est
        // dans ses réglages, pas dans les nôtres.
        if let Some(config) = game.settings.dxvk.config_line() {
            let manual = extra_env.iter().any(|(key, _)| {
                key.eq_ignore_ascii_case("DXVK_CONFIG")
                    || key.eq_ignore_ascii_case("DXVK_CONFIG_FILE")
            });
            if manual {
                tracing::info!(
                    "réglages DXVK du jeu {appid} ignorés : DXVK_CONFIG vient des variables du jeu"
                );
            } else {
                tracing::info!("réglages DXVK du jeu {appid} : {config}");
                extra_env.push(("DXVK_CONFIG".to_string(), config));
            }
        }
        if let Some(language) = lapinux_core::lang::resolve(
            game.settings.language.as_deref().unwrap_or_default(),
            &config.general.game_language,
        ) {
            extra_env.push(("LANG".to_string(), language.locale.to_string()));
            extra_env.push(("LANGUAGE".to_string(), language.locale.to_string()));
        }
        // Un jeu Linux natif se lance directement, hors du client Steam :
        // rien ne lui transmet son appid, et `SteamAPI_Init` échouerait même
        // client démarré. Les variables que Steam pose lui-même pour un jeu
        // natif font le même office. L'appid d'un jeu de boutique, lui, reste
        // interne à Lapinux : il n'est jamais transmis à Steamworks.
        if game.native && game.store == lapinux_core::stores::Store::Steam {
            let already = extra_env
                .iter()
                .any(|(key, _)| key.eq_ignore_ascii_case("SteamAppId"));
            if !already {
                extra_env.push(("SteamAppId".to_string(), appid.to_string()));
                extra_env.push(("SteamGameId".to_string(), appid.to_string()));
            }
        }
        let plan = lapinux_proton::build(&LaunchOptions {
            appid,
            exe: exe.clone(),
            native: game.native,
            proton: proton.clone(),
            prefix: prefix.clone(),
            steam_root: lapinux_core::paths::steam_roots().first().cloned(),
            umu_run: lapinux_proton::umu_run_path(),
            use_umu: umu,
            extra_env,
            extra_args: game.settings.args.clone(),
        })
        .map_err(|e| lapinux_core::Error::Other(e.to_string()))?;

        let caps = PerfCaps::detect();
        let scene_visual = if !game.settings.performance
            && config.launch.scene
            && config.launch.gamescope
        {
            scene_backdrop(&self.paths, appid)
        } else {
            None
        };
        let perf_config = perf_config(
            &config,
            &game,
            scene_cover(&self.paths, appid),
            scene_visual,
        );
        let mut prepared = lapinux_perf::prepare(
            &plan,
            appid,
            &perf_config,
            &caps,
            &self.paths.config_dir,
            &self.paths.log_dir,
        )
        .map_err(|e| lapinux_core::Error::Other(e.to_string()))?;
        // La couche MangoHud corrigée de Lapinux prend la place de celle du
        // système : même activation (MANGOHUD=1), mais les ancrages « en
        // ligne » tombent juste, y compris à droite et au centre.
        for (key, value) in lapinux_perf::mangohud::vendor_env(&self.paths.data_dir) {
            prepared.command.env(key, value);
        }
        // Le résumé décrit ce qui sera visible : MangoHud est chargé même
        // masqué, pour que l'overlay puisse l'allumer en cours de partie.
        let summary = lapinux_perf::layers_summary(
            prepared.gamescope,
            prepared.mangohud && perf_config.hud.enabled,
        );

        if dry_run {
            return Ok(Envelope::ok(&serde_json::json!({
                "command": lapinux_perf::render_prepared(&prepared),
                "proton_plan": lapinux_proton::render_plan(&plan),
                "layers": summary,
                "gamescope": prepared.gamescope,
                "mangohud": prepared.mangohud,
                "log": prepared.log_path,
                "drm": game.drm,
                "proton": proton.as_ref().map(|p| p.name.clone()),
                "steam_headless": config.launch.steam_headless && game.requires_steam(),
                "steam_client": false,
            })));
        }

        // Les sauvegardes du compte, avant que le jeu ne démarre : sans client
        // Steam, rien ne les a jamais posées dans le préfixe. Seul ce que le
        // cloud a de plus récent que le disque est repris — une progression
        // déjà jouée reste —, et un échec ne bloque pas le lancement. Un jeu
        // de boutique n'a pas de cloud Steam : son appid Lapinux n'en désigne
        // aucun, la lecture est donc sautée.
        let steam_game = game.store == lapinux_core::stores::Store::Steam;
        if !game.settings.cloud_sync_enabled() {
            tracing::debug!("cloud : synchronisation automatique désactivée pour {appid}");
        } else if steam_game {
            match self.sync_cloud(&game, appid, true) {
                Ok((0, _)) => {}
                Ok((files, bytes)) => {
                    tracing::info!(
                        "cloud : {files} sauvegarde(s) reprise(s) avant la partie de {appid} ({bytes} octets)"
                    );
                    if let Ok(mut cache) = self.game_reads.lock() {
                        cache.forget("cloud", appid);
                    }
                    self.forget_remote_sheet("cloud", appid);
                }
                Err(error) => {
                    tracing::warn!("cloud : rien repris avant la partie de {appid} ({error})");
                }
            }
        } else if game.settings.save_dir.is_some() {
            // Jeu de boutique : le dossier des sauvegardes est celui que
            // l'utilisateur a désigné. Sa boutique a pu changer depuis la
            // dernière partie, on reprend ce qu'elle a de plus récent.
            match self.store_cloud_pull_auto(appid) {
                Ok((0, _)) => {}
                Ok((files, bytes)) => tracing::info!(
                    "cloud {} : {files} sauvegarde(s) reprise(s) avant la partie de {appid} ({bytes} octets)",
                    game.store.label()
                ),
                Err(error) => tracing::warn!(
                    "cloud {} : rien repris avant la partie de {appid} ({error})",
                    game.store.label()
                ),
            }
        }

        // Un jeu Steamworks a besoin d'un client Steam authentifié. Le client
        // headless tient la session du compte : Lapinux met sa propre session
        // sociale en pause pour éviter que les deux connexions se chassent
        // l'une l'autre. Elle ne reprend qu'à la fermeture du client Steam.
        let mut steam_client = false;
        if config.launch.steam_headless && game.requires_steam() {
            let already_paused = self.social_paused.load(Ordering::SeqCst);
            if !already_paused {
                self.pause_social();
            }
            let result = crate::steam_headless::ensure(&self.paths.log_dir);
            if !already_paused {
                if crate::steam_headless::client_alive() {
                    let shared = Arc::clone(self);
                    std::thread::spawn(move || {
                        while crate::steam_headless::client_alive() {
                            std::thread::sleep(Duration::from_secs(2));
                        }
                        shared.resume_social();
                    });
                } else {
                    self.resume_social();
                }
            }
            result.map_err(|error| {
                lapinux_core::Error::Other(format!(
                    "Steam est requis par « {} » : {error}",
                    game.name
                ))
            })?;
            steam_client = true;
        }

        // La fenêtre de gamescope apparaît avant la première image du jeu :
        // sa présence seule ne suffit pas à retirer l'écran d'attente.
        let gamescope = prepared.gamescope;
        let scene = prepared.scene;
        // Image publique pour le Rich Presence : l'en-tête Steam est stable,
        // les boutiques publient leur jaquette dans leur bibliothèque.
        let cover = if game.store == lapinux_core::stores::Store::Steam {
            format!("https://cdn.cloudflare.steamstatic.com/steam/apps/{appid}/header.jpg")
        } else {
            self.cached_store_entry(appid)
                .map(|entry| entry.cover)
                .unwrap_or_default()
        };
        // Application Discord du jeu : avec elle, l'activité porte le nom et
        // l'icône du jeu au lieu de « Lapinux ». Vide si Discord ne connaît
        // pas ce jeu.
        let discord_app = self.discord_app_for(&game);
        // Un HUD déjà lancé relit sa configuration : l'overlay peut le
        // masquer, le réafficher ou changer de profil à chaud.
        let hud_live = prepared.mangohud;
        let session = lapinux_perf::Session::spawn(appid, prepared)
            .map_err(|e| lapinux_core::Error::Other(e.to_string()))?;
        let info = SessionInfo {
            appid,
            name: game.name.clone(),
            pid: session.pid,
            started: lapinux_core::util::now_unix(),
            proton: proton.map(|p| p.name).unwrap_or_else(|| "natif".into()),
            fps_cap: perf_config.gamescope.fps_cap,
            // Fréquence réelle de l'écran, pas la limite choisie : l'overlay
            // s'en sert pour ne pas proposer une cadence que le moniteur ne
            // peut pas afficher.
            refresh_hz: if gamescope {
                perf_config.gamescope.refresh_hz
            } else {
                0
            },
            mangohud: perf_config.hud.enabled,
            overlay_open: false,
            input_profile: game
                .settings
                .input_profile
                .clone()
                .unwrap_or_else(|| config.input.default_profile.clone()),
            log_path: session.log_path.clone(),
            window_shown: false,
            // Hors gamescope, la fenêtre du jeu est celle du jeu lui-même :
            // elle n'apparaît qu'une fois son rendu prêt. Sous gamescope, le
            // guetteur du journal décidera.
            render_started: !gamescope && !scene,
            window_gone: false,
            gamescope,
            scene,
            performance: game.settings.performance,
            hud_live,
            cover,
            discord_app,
            fps: None,
            fps_series: Vec::new(),
            fps_series_start: 0,
            fps_series_end: 0,
            store: game.store,
            // Le lancement écrit la configuration MangoHud complète : c'est le
            // préréglage que l'overlay affichera.
            hud_preset: if game.settings.performance {
                String::new()
            } else {
                game.settings
                    .hud_preset
                    .clone()
                    .filter(|preset| !preset.trim().is_empty())
                    .unwrap_or_else(|| {
                        if perf_config.hud.enabled {
                            "full".to_string()
                        } else {
                            String::new()
                        }
                    })
            },
            hud_position: perf_config.hud.position.clone(),
            hud_layout: if perf_config.hud.horizontal {
                "inline".into()
            } else {
                "block".into()
            },
            hud_graph: perf_config.hud.frame_timing,
            hud_cores: perf_config.hud.cpu_cores,
            dlss: detect_dlss(&game.install_dir()),
        };
        let wants_mapping = config.input.grab_devices || game.settings.input_profile.is_some();
        self.adopt_session(info.clone(), session, wants_mapping);
        // Un appid Lapinux n'est pas un appid Steam : seuls les jeux de Steam
        // peuvent apparaître « en jeu » sur le profil.
        self.set_steam_playing((game.store == lapinux_core::stores::Store::Steam).then_some(appid));

        // Relevé du cloud avant que le jeu n'écrive : à sa sortie, la
        // comparaison dira ce que la partie a envoyé. La lecture passe par la
        // session partagée, dans un fil à part pour ne pas retarder le
        // lancement.
        if let Ok(mut slot) = self.cloud_last.lock() {
            if slot.as_ref().map(|(known, _)| *known) == Some(appid) {
                *slot = None;
            }
        }
        // Le relevé du cloud n'a de sens que pour un jeu Steam.
        let shared = Arc::clone(self);
        if !steam_game {
            return Ok(Envelope::ok(&serde_json::json!({
                "session": info,
                "layers": summary,
                "steam_client": steam_client,
            })));
        }
        std::thread::spawn(move || {
            let Some(saves) = shared.read_cloud(appid) else {
                return;
            };
            tracing::info!(
                "cloud : {} fichier(s) avant la partie de {appid}",
                saves.files.len()
            );
            if let Ok(mut slot) = shared.cloud_before.lock() {
                *slot = Some((appid, saves));
            }
        });

        Ok(Envelope::ok(&serde_json::json!({
            "session": info,
            "layers": summary,
            "steam_client": steam_client,
        })))
    }
}

fn perf_config(
    config: &lapinux_core::Config,
    game: &Game,
    scene_cover: Option<PathBuf>,
    scene_backdrop: Option<PathBuf>,
) -> PerfConfig {
    let settings = &game.settings;
    let mangohud_enabled = !settings.performance
        && settings.mangohud.unwrap_or(config.launch.mangohud);
    // Le préréglage choisi dans l'overlay l'emporte sur la simple case à
    // cocher : il porte les statistiques à afficher. En mode performance,
    // aucun HUD n'est chargé, même masqué.
    let preset = if settings.performance {
        "off".into()
    } else {
        settings
            .hud_preset
            .clone()
            .filter(|preset| !preset.trim().is_empty())
            .unwrap_or_else(|| {
                if mangohud_enabled {
                    "full".into()
                } else {
                    "off".into()
                }
            })
    };
    let position = settings.hud_position.clone().unwrap_or_default();    let mut gamescope = lapinux_perf::GamescopeSettings {
        enabled: settings.gamescope.unwrap_or(config.launch.gamescope),
        output: config.launch.output.clone(),
        width: settings.width.unwrap_or(config.launch.width),
        height: settings.height.unwrap_or(config.launch.height),
        fps_cap: settings.fps_cap.unwrap_or(config.launch.fps_cap),
        refresh_hz: 0,
        hdr: settings.hdr.unwrap_or(config.launch.hdr),
        adaptive_sync: settings
            .adaptive_sync
            .unwrap_or(config.launch.adaptive_sync),
        fullscreen: config.launch.fullscreen,
        mangoapp: mangohud_enabled,
        // Le tube est créé par la session, avec son dossier de journaux.
        stats_path: None,
    };
    // Le compositeur tourne à la cadence de l'écran, pas à la limite choisie.
    gamescope.refresh_hz = session_refresh_hz(&gamescope);
    // Le profil reste enregistré quand MangoHud est coupé : la case à cocher
    // (ou le réglage de l'overlay) décide seule de l'affichage.
    let mut hud = lapinux_perf::mangohud::preset_settings_at(
        &preset,
        Some(gamescope.fps_cap),
        &position,
    );
    hud.enabled = mangohud_enabled && preset != "off";
    // La courbe de frametime se règle séparément : elle survit au changement
    // de profil et reste coupée tant que l'overlay ne l'active pas.
    hud.frame_timing = settings.hud_graph.unwrap_or(false);
    hud.cpu_cores = settings.hud_cores.unwrap_or(false);
    hud.horizontal = matches!(
        settings
            .hud_layout
            .as_deref()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "inline" | "line" | "horizontal"
    );
    PerfConfig {
        gamescope: gamescope.clone(),
        hud,
        inject_mangohud: !settings.performance,
        scene: !settings.performance && config.launch.scene && gamescope.enabled,
        scene_name: game.name.clone(),
        scene_cover,
        scene_backdrop,
    }
}

/// Jaquette déjà téléchargée pour la scène de lancement. Aucune requête
/// réseau : un jeu sans visuel garde le lapin de la scène.
fn scene_cover(
    paths: &lapinux_core::paths::LapinuxPaths,
    appid: u32,
) -> Option<PathBuf> {
    let covers = paths.covers_dir();
    [
        covers.join(format!("custom-{appid}.jpg")),
        covers.join(format!("store-{appid}.jpg")),
        covers.join(format!("sgdb-{appid}.jpg")),
        covers.join(format!("{appid}.jpg")),
    ]
    .into_iter()
    .find(|path| path.is_file())
}

/// Visuel large de la scène. Une capture 16:9 de la fiche Steam est préférée :
/// c'est la seule source qui remplit correctement la surface 16:9 sans
/// recadrer une jaquette portrait. Elle est téléchargée une fois puis gardée
/// dans le cache d'illustrations ; à défaut, une bannière locale est utilisée.
fn scene_backdrop(
    paths: &lapinux_core::paths::LapinuxPaths,
    appid: u32,
) -> Option<PathBuf> {
    let art = paths.art_dir();
    let saved = art.join(format!("steam-shot-{appid}.jpg"));
    if saved.is_file() {
        return Some(saved);
    }
    let sheet = paths
        .cache_dir
        .join("fiches-v8")
        .join(format!("{appid}.json"));
    if let Ok(text) = std::fs::read_to_string(&sheet) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(urls) = value
                .pointer("/store/screenshots")
                .and_then(|value| value.as_array())
            {
                for url in urls.iter().filter_map(|value| value.as_str()).take(3) {
                    let Some(bytes) = fetch_image(url) else {
                        continue;
                    };
                    let Some(image) = image::load_from_memory(&bytes).ok() else {
                        continue;
                    };
                    let ratio = image.width() as f32 / image.height().max(1) as f32;
                    // Une capture 1920x1080 est le cas normal ; on refuse un
                    // portrait ou une bannière trop panoramique.
                    if !(1.6..=1.95).contains(&ratio) {
                        continue;
                    }
                    std::fs::create_dir_all(&art).ok()?;
                    let temporary = saved.with_extension("tmp");
                    if std::fs::write(&temporary, &bytes).is_ok()
                        && std::fs::rename(&temporary, &saved).is_ok()
                    {
                        return Some(saved);
                    }
                }
            }
        }
    }
    ["steam-hero", "sgdb-hero", "steam-capsule", "capsule"]
        .into_iter()
        .map(|name| art.join(format!("{name}-{appid}.jpg")))
        .find(|path| path.is_file())
}

/// Télécharge une image avec une borne stricte : une réponse HTML ou un
/// fichier trop lourd ne doit pas retarder le lancement ni remplir le cache.
fn fetch_image(url: &str) -> Option<Vec<u8>> {
    let response = ureq::get(url)
        .timeout(Duration::from_secs(3))
        .call()
        .ok()?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(8 * 1024 * 1024)
        .read_to_end(&mut bytes)
        .ok()?;
    (!bytes.is_empty()).then_some(bytes)
}

/// Fréquence de l'écran que GameScope va présenter : celle du connecteur
/// choisi s'il est connu, sinon celle de l'écran principal. Zéro quand aucun
/// écran n'est identifiable, pour ne pas brider le curseur de l'overlay.
fn session_refresh_hz(settings: &lapinux_perf::GamescopeSettings) -> u32 {
    let wanted = settings.output.trim();
    let displays = lapinux_core::display::displays();
    let chosen = displays
        .iter()
        .find(|display| !wanted.is_empty() && display.name == wanted)
        .or_else(|| displays.iter().find(|display| display.primary))
        .or_else(|| displays.first());
    chosen.map(|display| display.refresh_hz).unwrap_or(0)
}

/// Fichier local d'une sauvegarde cloud : le préfixe Windows déclaré par le
/// jeu, ou le jeton porté par le nom (`%GameInstall%save.json`), résolu dans
/// l'installation ou dans le préfixe Proton qui exécute le jeu.
fn cloud_file_path(
    game: &Game,
    file: &lapinux_core::model::CloudFile,
    data_dir: &std::path::Path,
) -> Option<std::path::PathBuf> {
    let prefix_dir = lapinux_proton::prefix_for(game, data_dir);
    cloud_file_path_in(game, file, &prefix_dir)
}

/// Même résolution, avec un préfixe déjà préparé : la reprise cloud initialise
/// Proton avant d'écrire, les autres lectures se contentent du chemin existant.
fn cloud_file_path_in(
    game: &Game,
    file: &lapinux_core::model::CloudFile,
    prefix_dir: &std::path::Path,
) -> Option<std::path::PathBuf> {
    lapinux_steam::cloud::local_file(&file.prefix, &file.name, &game.install_dir(), prefix_dir)
}

/// Ce fichier local doit-il rejoindre le cloud ? Seule une copie plus récente
/// que le cloud, **et** de contenu différent, part : Steam refuse un envoi
/// identique (`DuplicateRequest`), et une date qui bouge sans que le fichier
/// change n'apporte rien.
fn cloud_upload_wanted(
    file: &lapinux_core::model::CloudFile,
    local: Option<i64>,
    sha: &[u8],
) -> bool {
    let Some(local) = local else {
        return false;
    };
    if file.modified > 0 && local <= file.modified {
        return false;
    }
    if !file.sha.is_empty() && file.sha == sha {
        return false;
    }
    true
}

/// Nom de la machine, tel que Steam le range dans un lot d'envoi : le nom
/// d'hôte suffit, c'est une étiquette lisible dans le client.
fn machine_name() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "lapinux".to_string())
}

/// Faut-il reprendre ce fichier du cloud avant une partie ? Un fichier local
/// aussi récent ou plus récent est le travail du joueur : il reste.
fn cloud_file_wanted(file: &lapinux_core::model::CloudFile, local: Option<i64>) -> bool {
    match local {
        None => true,
        Some(local) => file.modified > 0 && local < file.modified,
    }
}

/// Date de modification d'un fichier local, en secondes Unix.
fn local_modified(path: &std::path::Path) -> Option<i64> {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|stamp| stamp.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|age| age.as_secs() as i64)
}

/// Vrai quand ce contenu additionnel est déjà sur le disque.
///
/// Steam installe chaque contenu comme une application à part, avec son propre
/// manifeste `appmanifest_<appid>.acf` dans `steamapps` ; Lapinux écrit le même
/// à l'installation. Un contenu jamais téléchargé n'a pas de manifeste, ou en
/// a un qui n'est pas marqué installé.
fn extra_content_installed(library: &Path, dlc: u32) -> bool {
    // Le manifeste d'un contenu suit celui du jeu : `Steam` dans une
    // bibliothèque gérée, `steamapps` dans une vraie bibliothèque Steam.
    ["Steam", "steamapps"]
        .iter()
        .map(|dir| library.join(dir).join(format!("appmanifest_{dlc}.acf")))
        .any(|path| {
            lapinux_steam::AppManifest::read(&path)
                .map(|manifest| manifest.installed())
                .unwrap_or(false)
        })
}

/// Sépare les paquets de licences en deux ensembles : tous ceux que le compte
/// peut installer, et ceux qui viennent d'un autre membre de la famille Steam.
///
/// Un paquet apparaît parfois dans les deux listes — le compte possède le jeu
/// **et** sa famille le partage. Il est alors possédé : le compte personnel
/// l'emporte, sinon la fiche annoncerait une version prêtée pour un jeu qu'on
/// a acheté, et le catalogue refuserait de s'en servir comme du sien.
fn split_license_packages(
    licenses: &[cm::License],
    own_account_id: u32,
) -> (Vec<u32>, HashSet<u32>) {
    let personal: HashSet<u32> = licenses
        .iter()
        .filter(|licence| licence.owner_id == 0 || licence.owner_id == own_account_id)
        .map(|licence| licence.package_id)
        .collect();
    let packages: Vec<u32> = licenses.iter().map(|licence| licence.package_id).collect();
    let family: HashSet<u32> = licenses
        .iter()
        .filter(|licence| licence.owner_id != 0 && licence.owner_id != own_account_id)
        .map(|licence| licence.package_id)
        .filter(|package| !personal.contains(package))
        .collect();
    (packages, family)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lapinux_core::paths::LapinuxPaths;

    #[test]
    fn a_game_read_is_served_from_cache_until_it_expires() {
        let mut cache = GameReadCache::default();
        assert!(cache.fresh("succès", 10).is_none());
        // Une seule lecture à la fois par jeu : la seconde ne démarre pas.
        assert!(cache.begin("succès", 10));
        assert!(!cache.begin("succès", 10));
        cache.finish("succès", 10, serde_json::json!({ "ok": true }));
        assert_eq!(
            cache.fresh("succès", 10),
            Some(serde_json::json!({ "ok": true }))
        );
        // La fiche et les succès ne se mélangent pas.
        assert!(cache.fresh("info", 10).is_none());
        // Passé le délai, la valeur reste connue sans être fraîche.
        cache.entries.insert(
            ("succès", 10),
            (
                Instant::now() - GAME_READ_TTL - Duration::from_secs(1),
                serde_json::json!({ "ok": true }),
            ),
        );
        assert!(cache.fresh("succès", 10).is_none());
        assert_eq!(
            cache.any("succès", 10),
            Some(serde_json::json!({ "ok": true }))
        );
    }

    /// Le ménage du dossier de fiches retire les restes d'écriture et borne
    /// le nombre de fichiers gardés.
    #[test]
    fn le_menage_du_cache_de_fiches_borne_le_dossier() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        let fiches = paths.cache_dir.join("fiches-v8");
        std::fs::create_dir_all(&fiches).unwrap();
        let shared = Shared::new(&paths).unwrap();
        // Un `.tmp` est le reste d'une écriture interrompue : il part toujours.
        std::fs::write(fiches.join("1.tmp"), b"{}").unwrap();
        for appid in 0..(INFO_DISK_MAX_FILES + 5) {
            std::fs::write(fiches.join(format!("{appid}.json")), b"{}").unwrap();
        }
        shared.prune_game_info_cache();
        let left = std::fs::read_dir(&fiches).unwrap().flatten().count();
        assert_eq!(left, INFO_DISK_MAX_FILES);
        assert!(!fiches.join("1.tmp").exists());
    }

    /// Seule une copie locale plus récente et de contenu différent part vers
    /// le cloud.
    #[test]
    fn an_upload_carries_only_new_local_content() {
        let file = lapinux_core::model::CloudFile {
            name: "GameSave.sav".into(),
            size: 10,
            modified: 1_700_000_000,
            sha: vec![1, 2, 3],
            ..Default::default()
        };
        // Fichier absent du disque : rien à envoyer.
        assert!(!cloud_upload_wanted(&file, None, &[9]));
        // Copie locale plus ancienne : le cloud est plus récent.
        assert!(!cloud_upload_wanted(&file, Some(1_600_000_000), &[9]));
        // Date plus récente mais contenu identique : Steam refuserait.
        assert!(!cloud_upload_wanted(&file, Some(1_700_000_100), &[1, 2, 3]));
        // Date plus récente et contenu neuf : c'est une sauvegarde à envoyer.
        assert!(cloud_upload_wanted(&file, Some(1_700_000_100), &[9]));
    }

    /// La reprise automatique ne touche qu'aux sauvegardes absentes ou plus
    /// anciennes que le cloud.
    #[test]
    fn an_automatic_restore_keeps_the_fresher_local_save() {
        let file = lapinux_core::model::CloudFile {
            name: "GameSave.sav".into(),
            size: 10,
            modified: 1_700_000_000,
            ..Default::default()
        };
        // Rien sur le disque : le cloud s'applique.
        assert!(cloud_file_wanted(&file, None));
        // Copie locale plus ancienne : le cloud s'applique.
        assert!(cloud_file_wanted(&file, Some(1_600_000_000)));
        // Copie locale aussi récente ou plus récente : elle reste.
        assert!(!cloud_file_wanted(&file, Some(1_700_000_000)));
        assert!(!cloud_file_wanted(&file, Some(1_800_000_000)));
        // Date de cloud inconnue : on ne remplace pas au hasard.
        let undated = lapinux_core::model::CloudFile {
            modified: 0,
            ..file.clone()
        };
        assert!(!cloud_file_wanted(&undated, Some(1_600_000_000)));
    }

    /// Tâche minimale, pour ne tester que ce qui compte : l'état et le jeu.
    fn job(id: &str, appid: u32, state: JobState, started: i64) -> Job {
        Job {
            id: id.into(),
            kind: JobKind::InstallGame,
            label: format!("Jeu {appid}"),
            title: format!("Jeu {appid}"),
            appid: Some(appid),
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
            started,
            finished: matches!(state, JobState::Done | JobState::Failed | JobState::Cancelled)
                .then_some(started + 1),
            report: None,
        }
    }

    #[test]
    fn the_top_of_the_queue_starts_first() {
        // L'ordre du glisser-déposer de l'interface : la carte du haut est la
        // prochaine à partir, même si elle a été mise en file après.
        // La carte du haut a été mise en file après l'autre : c'est pourtant
        // elle qui part la première.
        let waiting = vec![
            job("game-3-3", 3, JobState::Queued, 30),
            job("game-2-2", 2, JobState::Queued, 20),
        ];
        let next = next_queued_install(&waiting, false).expect("un jeu attend de partir");
        assert_eq!(next.appid, Some(3));

        // Un téléchargement en cours occupe la place : rien ne part tant
        // qu'il avance.
        let busy = vec![
            job("game-1-1", 1, JobState::Running, 10),
            job("game-3-3", 3, JobState::Queued, 30),
            job("game-2-2", 2, JobState::Queued, 20),
        ];
        assert!(next_queued_install(&busy, false).is_none());
        // En mode simultané, la file part sans attendre.
        assert_eq!(
            next_queued_install(&busy, true).map(|job| job.appid),
            Some(Some(3))
        );
    }

    #[test]
    fn a_queued_game_above_the_running_one_takes_its_place() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        let shared = Shared::new(&paths).unwrap();
        {
            let mut jobs = shared.jobs.lock().unwrap();
            jobs.push(job("game-2-2", 2, JobState::Queued, 20));
            jobs.push(job("game-1-1", 1, JobState::Running, 10));
        }
        // La carte en attente est passée devant le téléchargement en cours :
        // celui-ci cède le créneau et sera mis en pause, fichiers conservés.
        assert_eq!(
            shared.install_losing_the_slot().as_deref(),
            Some("game-1-1")
        );

        // Le téléchargement remonte au-dessus de la carte en attente : il n'y
        // a plus rien à échanger, il garde son créneau.
        {
            let mut jobs = shared.jobs.lock().unwrap();
            let running = jobs.pop().unwrap();
            jobs.insert(0, running);
        }
        assert!(shared.install_losing_the_slot().is_none());

        // Une tâche en pause ne bloque plus la file : la suivante peut
        // partir sans attendre qu'on la relance.
        {
            let mut jobs = shared.jobs.lock().unwrap();
            jobs[0].state = JobState::Paused;
        }
        let jobs = shared.jobs.lock().unwrap();
        assert!(next_queued_install(&jobs, false).is_some());
    }

    #[test]
    fn cancelling_a_queued_game_removes_its_card_for_good() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        {
            let shared = Arc::new(Shared::new(&paths).unwrap());
            shared
                .jobs
                .lock()
                .unwrap()
                .push(job("game-2-2", 2, JobState::Queued, 20));
            let envelope = shared.cancel_job("game-2-2").unwrap();
            assert!(envelope.ok);
            assert!(
                shared.jobs.lock().unwrap().is_empty(),
                "une tâche annulée ne doit pas atterrir dans « Terminées »"
            );
        }
        // Et elle ne revient pas au redémarrage suivant : c'est la liste
        // enregistrée qui a changé, pas un simple affichage masqué.
        let restarted = Shared::new(&paths).unwrap();
        assert!(restarted.jobs.lock().unwrap().is_empty());
    }

    #[test]
    fn clearing_the_history_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        {
            let shared = Shared::new(&paths).unwrap();
            let mut jobs = shared.jobs.lock().unwrap();
            let mut done = job("game-1-1", 1, JobState::Done, 10);
            done.finished = Some(11);
            jobs.push(done);
            jobs.push(job("game-2-2", 2, JobState::Running, 20));
            // Un transfert interrompu par un redémarrage n'est pas de
            // l'historique : il attend encore une reprise, dans « En pause ».
            jobs.push(job("game-3-3", 3, JobState::Interrupted, 30));
            jobs.push(job("game-4-4", 4, JobState::Paused, 40));
            drop(jobs);

            let envelope = shared.clear_history().unwrap();
            assert!(envelope.ok);
            let jobs = shared.jobs.lock().unwrap();
            assert_eq!(jobs.len(), 3, "seule la carte terminée disparaît : {jobs:?}");
            assert!(jobs.iter().all(|job| job.appid != Some(1)));
            assert!(
                jobs.iter()
                    .any(|job| job.appid == Some(3) && job.state == JobState::Interrupted),
                "une tâche interrompue reste à reprendre"
            );
            assert!(jobs
                .iter()
                .any(|job| job.appid == Some(4) && job.state == JobState::Paused));
        }

        // Le daemon relit le disque : l'historique effacé ne revient pas.
        let restarted = Shared::new(&paths).unwrap();
        let jobs = restarted.jobs.lock().unwrap();
        assert!(
            jobs.iter().all(|job| job.appid != Some(1)),
            "une tâche terminée effacée ne doit pas revenir au redémarrage : {jobs:?}"
        );
    }

    #[test]
    fn catalog_needs_a_saved_lapinux_session() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        let shared = Arc::new(Shared::new(&paths).unwrap());
        let error = shared.refresh_catalog("bob").unwrap_err().to_string();
        assert!(error.contains("connectez votre compte Steam"));
    }

    #[test]
    fn a_second_game_joins_the_queue_when_installs_are_sequential() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        let shared = Arc::new(Shared::new(&paths).unwrap());
        session::save_to(
            &session::path_in(&paths.data_dir),
            &SavedSession {
                account_name: "bob".into(),
                steam_id: 42,
                refresh_token: "refresh".into(),
                guard_data: None,
                saved_at: None,
            },
        )
        .unwrap();
        let mut config = shared.config_snapshot();
        config.steam.username = "bob".into();
        config.downloads.parallel_installs = false;
        shared.save_config(config).unwrap();
        shared.jobs.lock().unwrap().push(Job {
            id: "game-1-1".into(),
            kind: JobKind::InstallGame,
            label: "Autre (1)".into(),
            title: "Autre".into(),
            appid: Some(1),
            state: JobState::Running,
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
            message: "téléchargement".into(),
            backend: "Lapinux".into(),
            started: 10,
            finished: None,
            report: None,
        });

        let envelope = shared
            .install_game(42, Vec::new(), None)
            .expect("la demande doit être acceptée");
        assert!(envelope.ok);
        let jobs = shared.jobs.lock().unwrap();
        let queued = jobs
            .iter()
            .find(|job| job.appid == Some(42))
            .expect("une tâche en attente doit être créée");
        assert_eq!(queued.state, JobState::Queued);
        assert_eq!(queued.message, "en attente");
    }

    #[test]
    fn chosen_extra_content_waits_in_the_job_it_belongs_to() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        let shared = Arc::new(Shared::new(&paths).unwrap());
        session::save_to(
            &session::path_in(&paths.data_dir),
            &SavedSession {
                account_name: "bob".into(),
                steam_id: 42,
                refresh_token: "refresh".into(),
                guard_data: None,
                saved_at: None,
            },
        )
        .unwrap();
        let mut config = shared.config_snapshot();
        config.steam.username = "bob".into();
        // Un seul jeu à la fois : la demande reste en file avant de partir.
        config.downloads.parallel_installs = false;
        shared.save_config(config).unwrap();
        shared.jobs.lock().unwrap().push(Job {
            id: "game-1-1".into(),
            kind: JobKind::InstallGame,
            label: "Autre (1)".into(),
            title: "Autre".into(),
            appid: Some(1),
            state: JobState::Running,
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
            message: "téléchargement".into(),
            backend: "Lapinux".into(),
            started: 10,
            finished: None,
            report: None,
        });

        // Un contenu acheté après coup part avec la demande : la reprise de la
        // file doit installer la même chose, pas seulement le jeu de base.
        // La version choisie voyage avec elle pour la même raison : la file
        // relance `install_game` avec ce que la tâche a retenu.
        let envelope = shared
            .install_game(42, vec![900, 901], Some("Linux".into()))
            .expect("la demande doit être acceptée");
        assert!(envelope.ok);
        let jobs = shared.jobs.lock().unwrap();
        let queued = jobs
            .iter()
            .find(|job| job.appid == Some(42))
            .expect("une tâche en attente doit être créée");
        assert_eq!(queued.dlc, vec![900, 901]);
        assert_eq!(queued.platform.as_deref(), Some("linux"));
    }

    #[test]
    fn an_unknown_version_leaves_the_engine_free() {
        // Seules les trois plateformes que Steam publie sont acceptées : une
        // autre valeur ferait échouer la lecture des dépôts.
        assert_eq!(normalize_platform(Some(" Linux ".into())).as_deref(), Some("linux"));
        assert_eq!(
            normalize_platform(Some("windows".into())).as_deref(),
            Some("windows")
        );
        assert_eq!(normalize_platform(Some("macOS".into())).as_deref(), Some("macos"));
        assert_eq!(normalize_platform(Some("beos".into())), None);
        assert_eq!(normalize_platform(Some(String::new())), None);
        assert_eq!(normalize_platform(None), None);
    }

    #[test]
    fn a_package_owned_and_shared_stays_personal() {
        let licences = vec![
            cm::License {
                package_id: 100,
                owner_id: 0,
                ..cm::License::default()
            },
            // Le même jeu est aussi partagé par la famille : il reste le nôtre.
            cm::License {
                package_id: 100,
                owner_id: 7,
                ..cm::License::default()
            },
            cm::License {
                package_id: 200,
                owner_id: 0,
                ..cm::License::default()
            },
            cm::License {
                package_id: 300,
                owner_id: 7,
                ..cm::License::default()
            },
            cm::License {
                package_id: 400,
                owner_id: 0,
                ..cm::License::default()
            },
            // Une licence nominative du compte lui-même (owner_id = le sien).
            cm::License {
                package_id: 500,
                owner_id: 999,
                ..cm::License::default()
            },
            cm::License {
                package_id: 500,
                owner_id: 7,
                ..cm::License::default()
            },
        ];
        let (packages, family) = split_license_packages(&licences, 999);
        assert_eq!(packages, vec![100, 100, 200, 300, 400, 500, 500]);
        assert!(!family.contains(&100), "un jeu possédé n'est pas familial");
        assert!(!family.contains(&200));
        assert!(family.contains(&300), "un jeu prêté le reste");
        assert!(!family.contains(&400));
        assert!(!family.contains(&500));
    }

    #[test]
    fn a_content_already_on_disk_is_read_from_its_manifest() {
        // Steam installe chaque contenu additionnel comme une application à
        // part : son manifeste dit s'il est là.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let steamapps = root.join("steamapps");
        std::fs::create_dir_all(&steamapps).unwrap();
        lapinux_steam::write_manifest(root, 900, "Contenu", "Jeu", 5, true).unwrap();
        std::fs::write(
            steamapps.join("appmanifest_902.acf"),
            "\"AppState\"\n{\n\t\"appid\"\t\t\"902\"\n\t\"StateFlags\"\t\t\"2\"\n}\n",
        )
        .unwrap();

        assert!(
            extra_content_installed(root, 900),
            "le contenu installé se voit sur le disque"
        );
        assert!(
            !extra_content_installed(root, 901),
            "un contenu jamais téléchargé n'a pas de manifeste"
        );
        assert!(
            !extra_content_installed(root, 902),
            "un manifeste marqué « à mettre à jour » n'est pas installé"
        );
    }

    #[test]
    fn cancelling_a_new_install_removes_only_its_folder() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let games = root.join("Steam");
        let target = games.join("Jeu");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("partiel.bin"), b"x").unwrap();
        let other = games.join("Autre");
        std::fs::create_dir_all(&other).unwrap();

        remove_partial_install(&games, "Jeu");
        assert!(!target.exists(), "le dossier partiel doit disparaître");
        assert!(other.exists(), "les autres jeux restent intacts");

        // Un chemin qui sortirait du dossier des jeux est refusé sans rien
        // supprimer.
        remove_partial_install(&games, "..");
        assert!(root.exists());
    }

    #[test]
    fn saved_token_is_reported_and_can_be_forgotten() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        let shared = Arc::new(Shared::new(&paths).unwrap());
        let path = session::path_in(&paths.data_dir);
        session::save_to(
            &path,
            &SavedSession {
                account_name: "bob".into(),
                steam_id: 42,
                refresh_token: "refresh".into(),
                guard_data: None,
                saved_at: None,
            },
        )
        .unwrap();

        assert!(shared.steam_status().token_saved);
        let envelope = shared.steam_forget_token().unwrap();
        assert!(envelope.ok);
        assert!(!path.exists());
        assert!(!shared.steam_status().token_saved);
        assert!(
            !envelope
                .into_data::<SteamStatus>()
                .expect("état renvoyé")
                .token_saved
        );
        // Oublier un jeton déjà absent ne doit pas échouer.
        shared.steam_forget_token().unwrap();
    }

    #[test]
    fn token_state_reports_age_and_verdict() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        let shared = Arc::new(Shared::new(&paths).unwrap());
        session::save_to(
            &session::path_in(&paths.data_dir),
            &SavedSession {
                account_name: "bob".into(),
                steam_id: 42,
                refresh_token: "refresh".into(),
                guard_data: None,
                saved_at: None,
            },
        )
        .unwrap();

        // Avant toute vérification : jeton présent, âge connu, verdict inconnu.
        let status = shared.steam_status();
        let token = status.token.clone().expect("jeton décrit");
        assert_eq!(token.account_name, "bob");
        assert_eq!(token.verdict, SteamTokenVerdict::Unknown);
        let stamp = token.saved_at.expect("date d'écriture absente");
        assert!(status.connected);

        // Un jeton que Steam a refusé ne compte plus comme une connexion.
        *shared.token_check.lock().unwrap() = TokenCheck {
            verdict: SteamTokenVerdict::Expired,
            checked_at: Some(session::now_unix()),
            detail: Some("jeton révoqué".into()),
            running: false,
            token_saved_at: Some(stamp),
        };
        // Même une session ouverte avant la vérification ne survit pas au refus.
        shared.finish_login(SteamLoginState {
            connected: true,
            ..Default::default()
        });
        let status = shared.steam_status();
        assert!(!status.connected);
        let token = status.token.expect("jeton décrit");
        assert_eq!(token.verdict, SteamTokenVerdict::Expired);
        assert_eq!(token.detail.as_deref(), Some("jeton révoqué"));

        // Une limite temporaire de Steam n'est pas un jeton mort, mais la
        // session reste indisponible pendant la pause.
        *shared.token_check.lock().unwrap() = TokenCheck {
            verdict: SteamTokenVerdict::RateLimited,
            checked_at: Some(session::now_unix()),
            detail: Some("Steam limite les tentatives".into()),
            running: false,
            token_saved_at: Some(stamp),
        };
        let status = shared.steam_status();
        assert!(!status.connected);
        assert_eq!(
            status.token.expect("jeton décrit").verdict,
            SteamTokenVerdict::RateLimited
        );

        // Une vérification réseau réussie rend la connexion.
        *shared.token_check.lock().unwrap() = TokenCheck {
            verdict: SteamTokenVerdict::Valid,
            token_saved_at: Some(stamp),
            ..Default::default()
        };
        assert!(shared.steam_status().connected);

        // Un verdict rendu pour un autre jeton ne s'applique plus.
        *shared.token_check.lock().unwrap() = TokenCheck {
            verdict: SteamTokenVerdict::Expired,
            token_saved_at: Some(stamp - 1),
            ..Default::default()
        };
        let status = shared.steam_status();
        assert_eq!(
            status.token.expect("jeton décrit").verdict,
            SteamTokenVerdict::Unknown
        );
        assert!(status.connected, "un verdict périmé ne déconnecte pas");
    }

    #[test]
    fn token_written_before_the_date_is_dated_by_its_file() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        let shared = Arc::new(Shared::new(&paths).unwrap());
        let path = session::path_in(&paths.data_dir);
        // Ancien format : pas de `saved_at` dans le fichier.
        std::fs::write(
            &path,
            r#"{"account_name":"bob","steam_id":42,"refresh_token":"refresh"}"#,
        )
        .unwrap();
        let token = shared.steam_status().token.expect("jeton décrit");
        let stamp = token.saved_at.expect("date reprise du fichier");
        assert!((session::now_unix() - stamp).abs() < 60, "{stamp}");
    }

    #[test]
    fn a_check_starts_only_once_per_token() {
        let mut check = TokenCheck::default();
        assert!(needs_check(&check, Some(10)), "jeton jamais vérifié");

        check.verdict = SteamTokenVerdict::Checking;
        check.running = true;
        check.token_saved_at = Some(10);
        assert!(!needs_check(&check, Some(10)), "déjà en cours");

        check.running = false;
        check.verdict = SteamTokenVerdict::Valid;
        assert!(!needs_check(&check, Some(10)), "verdict déjà rendu");
        assert!(needs_check(&check, Some(11)), "jeton remplacé");

        check.verdict = SteamTokenVerdict::Expired;
        assert!(!needs_check(&check, Some(10)));

        // Une limite temporaire est déjà connue : inutile de relancer le
        // contrôle et de rallonger le blocage Steam.
        check.verdict = SteamTokenVerdict::RateLimited;
        assert!(!needs_check(&check, Some(10)));

        // Un échec réseau ne condamne pas le jeton : la vérification repart.
        check.verdict = SteamTokenVerdict::Unknown;
        assert!(needs_check(&check, Some(10)));
    }

    #[test]
    fn a_known_verdict_answers_without_deadlocking() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        let shared = Arc::new(Shared::new(&paths).unwrap());
        session::save_to(
            &session::path_in(&paths.data_dir),
            &SavedSession {
                account_name: "bob".into(),
                steam_id: 42,
                refresh_token: "refresh".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let stamp = shared.steam_status().token.expect("jeton décrit").saved_at;
        *shared.token_check.lock().unwrap() = TokenCheck {
            verdict: SteamTokenVerdict::Valid,
            token_saved_at: stamp,
            ..Default::default()
        };
        // Sans jeton à revérifier, la demande rend l'état tout de suite : une
        // garde encore tenue ici bloquerait `steam_status` indéfiniment.
        let envelope = shared.steam_token_check().unwrap();
        assert!(envelope.ok);
        assert!(shared.steam_status().connected);
        assert_eq!(
            shared.token_check.lock().unwrap().verdict,
            SteamTokenVerdict::Valid
        );
    }

    #[test]
    fn checking_a_missing_token_does_not_call_steam() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        let shared = Arc::new(Shared::new(&paths).unwrap());
        // Sans session enregistrée, la demande rend l'état et ne lance rien.
        let envelope = shared.steam_token_check().unwrap();
        assert!(envelope.ok);
        assert!(!shared.token_check.lock().unwrap().running);
        assert_eq!(
            shared.token_check.lock().unwrap().verdict,
            SteamTokenVerdict::Unknown
        );
    }

    #[test]
    fn login_errors_lose_their_category_prefix() {
        // Ce que l'écran affiche pour un mot de passe refusé.
        let error = cm::Error::Auth("connexion refusée : mot de passe invalide (eresult 5)".into());
        assert_eq!(
            message_utilisateur(&error),
            "connexion refusée : mot de passe invalide (eresult 5)"
        );
        // Une erreur sans catégorie connue reste entière.
        let timeout = cm::Error::Timeout;
        assert_eq!(message_utilisateur(&timeout), "délai dépassé");
    }

    #[test]
    fn cancelling_login_stops_the_guard_wait() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        let shared = Arc::new(Shared::new(&paths).unwrap());
        {
            let mut state = shared.login.lock().unwrap();
            state.running = true;
            state.stage = "code Steam Guard".into();
        }
        shared.login_cancel.store(false, Ordering::SeqCst);

        let worker = Arc::clone(&shared);
        let handle = std::thread::spawn(move || {
            let mut initial = None;
            worker.steam_guard_prompt(cm::Guard::EmailCode, false, &mut initial)
        });
        for _ in 0..100 {
            if shared
                .login
                .lock()
                .map(|state| state.guard.is_some())
                .unwrap_or(false)
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }

        shared.steam_login_cancel().unwrap();
        let result = handle.join().unwrap();
        assert!(result.is_err());
        let status = shared.steam_status();
        assert!(!status.login.running);
        assert!(status
            .login
            .message
            .as_deref()
            .unwrap_or_default()
            .contains("annulée"));
    }

    #[test]
    fn guard_code_is_forwarded_to_the_waiting_login() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        let shared = Arc::new(Shared::new(&paths).unwrap());
        let worker = Arc::clone(&shared);
        let handle = std::thread::spawn(move || {
            let mut initial = None;
            worker.steam_guard_prompt(cm::Guard::EmailCode, false, &mut initial)
        });

        for _ in 0..100 {
            if shared
                .login
                .lock()
                .map(|state| state.guard.is_some())
                .unwrap_or(false)
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        shared.steam_login_guard("ABC123").unwrap();
        let code = handle.join().unwrap().unwrap();
        assert_eq!(code, "ABC123");
        assert!(shared
            .login
            .lock()
            .map(|state| state.guard.is_none())
            .unwrap_or(false));
    }

    #[test]
    fn dlss_libraries_are_recognised() {
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("Binaries/Win64");
        std::fs::create_dir_all(&game).unwrap();
        assert_eq!(detect_dlss(dir.path()), None);

        std::fs::write(game.join("nvngx_dlss.dll"), b"x").unwrap();
        let info = detect_dlss(dir.path()).expect("DLSS détecté");
        assert!(info.dlss);
        assert!(!info.frame_generation);
        assert_eq!(info.summary(), "DLSS");
        assert_eq!(info.short(), "DLSS");

        std::fs::write(game.join("nvngx_dlssg.dll"), b"x").unwrap();
        std::fs::write(game.join("nvngx_dlssd.dll"), b"x").unwrap();
        let info = detect_dlss(dir.path()).unwrap();
        assert_eq!(info.summary(), "DLSS 3 · images générées · rayons");
        assert_eq!(info.short(), "DLSS 3 · FG · RR");
    }

    #[test]
    fn uninstall_removes_files_prefix_and_record() {
        use lapinux_core::model::GameOrigin;

        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        let shared = Arc::new(Shared::new(&paths).unwrap());
        let library = dir.path().join("library");
        let install_dir = library.join("Steam/Test Game");
        std::fs::create_dir_all(&install_dir).unwrap();
        std::fs::write(install_dir.join("game.exe"), b"x").unwrap();
        let prefix = paths.data_dir.join("prefixes/42");
        std::fs::create_dir_all(&prefix).unwrap();
        std::fs::write(prefix.join("marker"), b"x").unwrap();
        let game = Game {
            appid: 42,
            name: "Test".into(),
            installdir: "Test Game".into(),
            library: library.clone(),
            origin: GameOrigin::Lapinux,
            installed: true,
            prefix: Some(prefix.clone()),
            ..Default::default()
        };
        shared.store.save_game(&game).unwrap();
        let other = Game {
            appid: 99,
            name: "Autre".into(),
            installdir: "Autre".into(),
            library: library.clone(),
            origin: GameOrigin::Lapinux,
            installed: true,
            ..Default::default()
        };
        shared.store.save_game(&other).unwrap();

        shared.uninstall_game(42).unwrap();
        assert!(!install_dir.exists());
        assert!(!prefix.exists());
        assert!(shared.store.game(42).unwrap().is_none());
        assert!(
            shared.store.game(99).unwrap().is_some(),
            "les autres jeux ne sont pas rescannés"
        );

        // Un dossier qui sortirait de `common` est refusé sans rien supprimer.
        let unsafe_game = Game {
            appid: 43,
            installdir: "..".into(),
            library: library.clone(),
            installed: true,
            ..Default::default()
        };
        shared.store.save_game(&unsafe_game).unwrap();
        assert!(shared.uninstall_game(43).is_err());
        assert!(library.exists());
    }
}

/// Version demandée, réduite aux trois plateformes que Steam publie.
///
/// Une valeur inconnue (interface plus récente, faute de frappe dans un
/// fichier de configuration) vaut `None` : mieux vaut la préférence du moteur
/// qu'un téléchargement qui échouerait faute de dépôt.
fn normalize_platform(platform: Option<String>) -> Option<String> {
    let platform = platform?;
    let platform = platform.trim().to_ascii_lowercase();
    matches!(platform.as_str(), "linux" | "windows" | "macos").then_some(platform)
}

/// Prochaine tâche à lancer : la **première carte en attente**, celle du haut
/// de la liste, puisque l'ordre du vecteur suit le glisser-déposer de
/// l'interface. En mode séquentiel, rien ne part tant qu'un téléchargement
/// occupe la place.
fn next_queued_install(jobs: &[Job], parallel: bool) -> Option<Job> {
    if !parallel
        && jobs
            .iter()
            .any(|job| job.kind == JobKind::InstallGame && job.state == JobState::Running)
    {
        return None;
    }
    jobs.iter()
        .find(|job| job.kind == JobKind::InstallGame && job.state == JobState::Queued)
        .cloned()
}

/// Repère les bibliothèques DLSS dans le dossier du jeu. Un jeu DLSS les
/// range à côté de son exécutable (Unreal les place dans le dossier du
/// moteur) : leur nom suffit à dire ce qu'il sait faire.
fn detect_dlss(root: &Path) -> Option<lapinux_core::model::DlssInfo> {
    if !root.is_dir() {
        return None;
    }
    let mut info = lapinux_core::model::DlssInfo::default();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    let mut seen = 0usize;
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            seen += 1;
            if seen > 40_000 {
                return if info.is_empty() { None } else { Some(info) };
            }
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                // Les préfixes Proton rangent des milliers de fichiers qui ne
                // disent rien du jeu lui-même.
                let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
                if depth < 3 && !name.starts_with("prefix") && name != "drive_c" && name != "pfx" {
                    stack.push((path, depth + 1));
                }
                continue;
            }
            match entry
                .file_name()
                .to_string_lossy()
                .to_ascii_lowercase()
                .as_str()
            {
                "nvngx_dlss.dll" => info.dlss = true,
                "nvngx_dlssg.dll" => info.frame_generation = true,
                "nvngx_dlssd.dll" => info.ray_reconstruction = true,
                _ => {}
            }
        }
    }
    if info.is_empty() {
        None
    } else {
        Some(info)
    }
}

/// Supprime le dossier d'une installation neuve annulée. Refuse tout chemin
/// qui ne serait pas exactement `<dossier des jeux>/<jeu>`.
fn remove_partial_install(games_root: &Path, installdir: &str) {
    let name = Path::new(installdir);
    let safe = !installdir.trim().is_empty()
        && !name.is_absolute()
        && name
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)));
    if !safe {
        return;
    }
    let dir = games_root.join(name);
    if dir.parent() != Some(games_root) {
        return;
    }
    match std::fs::symlink_metadata(&dir) {
        Ok(metadata) if metadata.is_dir() => {
            let _ = std::fs::remove_dir_all(&dir);
        }
        Ok(metadata) if metadata.file_type().is_symlink() => {
            let _ = std::fs::remove_file(&dir);
        }
        _ => {}
    }
}

