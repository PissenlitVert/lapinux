//! Parcours Steam sans interface : compte Lapinux, catalogue, installation et
//! mise à jour natives, puis préparation du lancement Proton. Ni SteamCMD ni le
//! client Steam de bureau n'interviennent pour ces opérations : Lapinux ne
//! démarre jamais Steam, même pour un jeu Steamworks. La session
//! qui en découle (entrées, raccourci, overlay, arrêt) vit dans
//! [`crate::launch`].

#[path = "steam_cloud.rs"]
mod cloud;
#[path = "steam_auth_state.rs"]
mod steam_auth_state;
#[path = "steam_catalog.rs"]
mod steam_catalog;
#[path = "steam_install.rs"]
mod steam_install;
#[path = "steam_launch.rs"]
pub(crate) mod steam_launch;
#[path = "steam_login.rs"]
mod steam_login;

pub(crate) use cloud::{read_cloud_local, save_cloud_recovery, write_cloud_file};

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
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
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

/// Chemins des manettes physiques que le service d'entrée remplace par son
/// pad virtuel. Le pad du pont Steam et le hidraw du Steam Controller ne
/// doivent pas rester visibles du jeu : Wine/SDL les énumère avant le pad
/// virtuel et peut choisir une source que Lapinux a grabée, donc muette.
fn input_sources_to_hide(sources: &[PathBuf]) -> Vec<PathBuf> {
    if sources.is_empty() {
        return Vec::new();
    }
    let mut paths = sources.to_vec();
    // Les nœuds associés doivent appartenir au même périphérique sysfs,
    // jamais simplement porter le même nom qu'une manette prise en charge.
    let devices: Vec<PathBuf> = sources
        .iter()
        .filter_map(|path| {
            let event = path.file_name()?;
            std::fs::canonicalize(Path::new("/sys/class/input").join(event).join("device")).ok()
        })
        .collect();
    // Ne masquer un hidraw Steam que s'il est rattaché à une source ouverte.
    // Le récepteur et ses autres interfaces restent visibles sinon.
    if let Ok(entries) = std::fs::read_dir("/sys/class/hidraw") {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if !name.starts_with("hidraw") {
                continue;
            }
            let Ok(uevent) = std::fs::read_to_string(entry.path().join("device/uevent")) else {
                continue;
            };
            let Some(id) = uevent.lines().find_map(|line| line.strip_prefix("HID_ID=")) else {
                continue;
            };
            let mut parts = id.split(':');
            let bus = parts
                .next()
                .and_then(|part| u32::from_str_radix(part, 16).ok());
            let vendor = parts
                .next()
                .and_then(|part| u32::from_str_radix(part, 16).ok());
            let product = parts
                .next()
                .and_then(|part| u32::from_str_radix(part, 16).ok());
            if bus == Some(3)
                && vendor == Some(0x28de)
                && matches!(product, Some(0x1302..=0x1305 | 0x1142))
                && std::fs::canonicalize(entry.path().join("device"))
                    .ok()
                    .is_some_and(|hid| devices.iter().any(|device| device.starts_with(&hid)))
            {
                paths.push(PathBuf::from("/dev").join(name));
            }
        }
    }
    // Les jeux qui passent encore par l'ancienne interface joystick doivent
    // suivre le meme sort que leur evdev.
    if let Ok(entries) = std::fs::read_dir("/sys/class/input") {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if !name.starts_with("js") {
                continue;
            }
            if std::fs::canonicalize(entry.path().join("device"))
                .ok()
                .is_some_and(|node| devices.contains(&node))
            {
                paths.push(PathBuf::from("/dev/input").join(name));
            }
        }
    }
    paths.sort();
    paths.dedup();
    paths
}

/// Cache les sources de manette au processus du jeu. Le service d'entrée,
/// lui, tourne hors de cet espace de montage et continue de les lire.
fn hide_input_sources(prepared: &mut lapinux_perf::PreparedLaunch, sources: &[PathBuf]) {
    let Some(bwrap) = lapinux_core::paths::which("bwrap") else {
        tracing::warn!("entrée : bwrap absent, les manettes physiques restent visibles du jeu");
        return;
    };
    let paths = input_sources_to_hide(sources);
    if paths.is_empty() {
        return;
    }
    let original = std::mem::replace(&mut prepared.command, Command::new("/bin/true"));
    let mut command = Command::new(&bwrap);
    command.arg("--dev-bind").arg("/").arg("/");
    // Le nouveau namespace change le propriétaire apparent de /tmp : Xwayland
    // refuse alors de créer son serveur. Un tmpfs interne garde un dossier
    // appartenant à l'utilisateur, sans exposer les sockets X du bureau.
    command.arg("--tmpfs").arg("/tmp/.X11-unix");
    for path in &paths {
        if path.exists() {
            command.arg("--bind").arg("/dev/null").arg(path);
        }
    }
    command.arg("--");
    command.arg(original.get_program());
    command.args(original.get_args());
    for (key, value) in original.get_envs() {
        match value {
            Some(value) => {
                command.env(key, value);
            }
            None => {
                command.env_remove(key);
            }
        }
    }
    if let Some(cwd) = original.get_current_dir() {
        command.current_dir(cwd);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    tracing::info!(
        "entrée : {} source(s) de manette masquée(s) au jeu, pad virtuel Lapinux utilisé",
        paths.len()
    );
    prepared.command = command;
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
            SteamTokenVerdict::Valid | SteamTokenVerdict::Expired | SteamTokenVerdict::RateLimited
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
///
/// Le démarrage doit rester silencieux : deux fiches suffisent à rendre le
/// premier clic agréable, et elles partent après deux minutes. Les autres
/// seront lues à l'ouverture du jeu, depuis un cache déjà chaud la plupart du
/// temps. Cinq fiches complètes demandaient plusieurs dizaines d'appels
/// Steam et se mêlaient à la synchronisation du compte.
const INFO_PRELOAD_GAMES: usize = 2;
const INFO_PRELOAD_DELAY: Duration = Duration::from_secs(120);
const INFO_PRELOAD_GAP: Duration = Duration::from_secs(10);
/// Une fiche sur disque plus jeune que ça n'a pas besoin d'être relue au
/// démarrage : elle est déjà à jour pour un usage courant.
const INFO_PRELOAD_MAX_AGE: Duration = Duration::from_secs(6 * 3600);

/// Ménage du dossier de fiches : une fiche par jeu ouvert y est écrite, le
/// dossier suivrait donc la taille de la bibliothèque. Les fiches plus vieilles
/// que ça partent, et au-delà du plafond les plus anciennes aussi.
const INFO_DISK_MAX_AGE: Duration = Duration::from_secs(30 * 24 * 3600);
const INFO_DISK_MAX_FILES: usize = 400;

/// Les fiches gardent un nom de dossier stable. Un changement de structure
/// est signalé par le fichier `.format` placé à côté, qui vide le dossier une
/// fois : l'utilisateur ne voit pas une nouvelle génération de dossiers.
const STEAM_SHEET_DIR: &str = "fiches";
const STORE_SHEET_DIR: &str = "fiches-boutique";
const SHEET_CACHE_FORMAT: &str = "2";
const LEGACY_SHEET_DIRS: &[&str] = &[
    "fiches-v1",
    "fiches-v2",
    "fiches-v3",
    "fiches-v4",
    "fiches-v5",
    "fiches-v6",
    "fiches-v7",
    "fiches-v8",
    "fiches-boutique-v1",
    "fiches-boutique-v2",
    "fiches-boutique-v3",
    "fiches-boutique-v4",
];

/// Seules les réponses déjà enregistrées sur disque peuvent être évincées :
/// les autres n'ont pas de repli si la prochaine lecture réseau échoue.
const GAME_READ_DISK_BACKED_LIMIT: usize = 128;

#[derive(Debug)]
struct GameReadEntry {
    at: Instant,
    value: serde_json::Value,
    last_used: Instant,
    disk_backed: bool,
}

/// Réponses gardées par jeu, et lectures en cours.
#[derive(Debug, Default)]
pub struct GameReadCache {
    entries: std::collections::HashMap<(&'static str, u32), GameReadEntry>,
    running: std::collections::HashSet<(&'static str, u32)>,
}

impl GameReadCache {
    /// Valeur connue, même périmée : mieux qu'une attente quand une lecture
    /// est déjà en cours pour le même jeu.
    fn any(&mut self, kind: &'static str, appid: u32) -> Option<serde_json::Value> {
        let entry = self.entries.get_mut(&(kind, appid))?;
        entry.last_used = Instant::now();
        Some(entry.value.clone())
    }

    /// Prend la lecture à son compte ; `false` si une autre la tient déjà.
    pub(crate) fn begin(&mut self, kind: &'static str, appid: u32) -> bool {
        self.running.insert((kind, appid))
    }

    pub(crate) fn finish(
        &mut self,
        kind: &'static str,
        appid: u32,
        value: serde_json::Value,
        disk_backed: bool,
    ) {
        self.running.remove(&(kind, appid));
        self.insert(kind, appid, value, Instant::now(), disk_backed);
    }

    pub(crate) fn abandon(&mut self, kind: &'static str, appid: u32) {
        self.running.remove(&(kind, appid));
        self.trim_disk_backed();
    }

    fn insert(
        &mut self,
        kind: &'static str,
        appid: u32,
        value: serde_json::Value,
        at: Instant,
        disk_backed: bool,
    ) {
        self.entries.insert(
            (kind, appid),
            GameReadEntry {
                at,
                value,
                last_used: Instant::now(),
                disk_backed,
            },
        );
        self.trim_disk_backed();
    }

    /// Les lectures en cours gardent leur ancienne valeur jusqu'à leur réponse.
    fn trim_disk_backed(&mut self) {
        while self
            .entries
            .values()
            .filter(|entry| entry.disk_backed)
            .count()
            > GAME_READ_DISK_BACKED_LIMIT
        {
            let oldest = self
                .entries
                .iter()
                .filter(|(key, entry)| entry.disk_backed && !self.running.contains(key))
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| *key);
            match oldest {
                Some(key) => {
                    self.entries.remove(&key);
                }
                None => break,
            }
        }
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
        self.insert(kind, appid, value, at, true);
    }

    /// La valeur connue est-elle encore dans sa fenêtre de fraîcheur ?
    fn is_fresh(&self, kind: &'static str, appid: u32) -> bool {
        self.entries
            .get(&(kind, appid))
            .map_or(false, |entry| entry.at.elapsed() < GAME_READ_TTL)
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

impl Shared {}

fn perf_config(
    config: &lapinux_core::Config,
    game: &Game,
    scene_cover: Option<PathBuf>,
    scene_backdrop: Option<PathBuf>,
) -> PerfConfig {
    let settings = &game.settings;
    let mut gamescope = lapinux_perf::GamescopeSettings {
        enabled: settings.gamescope.unwrap_or(config.launch.gamescope),
        output: config.launch.output.clone(),
        width: settings.width.unwrap_or(config.launch.width),
        height: settings.height.unwrap_or(config.launch.height),
        game_width: config.graphics.game_width,
        game_height: config.graphics.game_height,
        filter: settings
            .scaling_filter
            .unwrap_or(config.graphics.scaling_filter),
        sharpness: settings.sharpness(config.graphics.sharpness),
        fps_cap: settings.fps_cap.unwrap_or(config.launch.fps_cap),
        refresh_hz: 0,
        hdr: settings.hdr.unwrap_or(config.launch.hdr),
        adaptive_sync: settings
            .adaptive_sync
            .unwrap_or(config.launch.adaptive_sync),
        fullscreen: config.launch.fullscreen,
        // Le tube est créé par la session, avec son dossier de journaux.
        stats_path: None,
    };
    // La fréquence de l'écran reste connue pour l'overlay ; la cadence du
    // compositeur (`-r`) est calculée dans les arguments GameScope, au-dessus
    // de l'écran pour que « illimité » puisse être appliqué à chaud.
    gamescope.refresh_hz = session_refresh_hz(&gamescope);
    PerfConfig {
        gamescope: gamescope.clone(),
        game_env: Vec::new(),
        scene: !settings.performance && config.launch.scene && gamescope.enabled,
        scene_name: game.name.clone(),
        scene_cover,
        scene_backdrop,
    }
}

/// Jaquette déjà téléchargée pour la scène de lancement. Aucune requête
/// réseau : un jeu sans visuel garde le lapin de la scène.
fn scene_cover(paths: &lapinux_core::paths::LapinuxPaths, appid: u32) -> Option<PathBuf> {
    let covers = paths.covers_dir();
    [
        paths.existing_custom_cover_path(appid),
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
fn scene_backdrop(paths: &lapinux_core::paths::LapinuxPaths, appid: u32) -> Option<PathBuf> {
    let art = paths.art_dir();
    let saved = art.join(format!("steam-shot-{appid}.jpg"));
    if saved.is_file() {
        return Some(saved);
    }
    let sheet = paths
        .cache_dir
        .join(STEAM_SHEET_DIR)
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
    let response = ureq::get(url).timeout(Duration::from_secs(3)).call().ok()?;
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
/// peut installer, et ceux qui viennent d'un autre membre de la famille Steam,
/// avec le SteamID de chaque propriétaire.
///
/// Un paquet apparaît parfois dans les deux listes — le compte possède le jeu
/// **et** sa famille le partage. Il est alors possédé : le compte personnel
/// l'emporte, sinon la fiche annoncerait une version prêtée pour un jeu qu'on
/// a acheté, et le catalogue refuserait de s'en servir comme du sien.
///
/// Un même paquet peut appartenir à deux membres (deux achats du même jeu) :
/// la liste garde les deux propriétaires, et le contenu est proposé pour l'un
/// comme pour l'autre.
fn split_license_packages(
    licenses: &[cm::License],
    own_account_id: u32,
) -> (Vec<u32>, HashMap<u32, Vec<u64>>) {
    // Base des SteamID individuels : l'identifiant de compte tient dans les
    // 32 bits de poids faible.
    const INDIVIDUAL_STEAM_ID: u64 = 76_561_197_960_265_728;
    let personal: HashSet<u32> = licenses
        .iter()
        .filter(|licence| licence.owner_id == 0 || licence.owner_id == own_account_id)
        .map(|licence| licence.package_id)
        .collect();
    let packages: Vec<u32> = licenses.iter().map(|licence| licence.package_id).collect();
    let mut family: HashMap<u32, Vec<u64>> = HashMap::new();
    for license in licenses {
        if license.owner_id == 0 || license.owner_id == own_account_id {
            continue;
        }
        if personal.contains(&license.package_id) {
            continue;
        }
        let owners = family.entry(license.package_id).or_default();
        let steam_id = INDIVIDUAL_STEAM_ID + license.owner_id as u64;
        if !owners.contains(&steam_id) {
            owners.push(steam_id);
        }
    }
    (packages, family)
}

#[cfg(test)]
mod tests {
    use super::cloud::{cloud_action, same_cloud_snapshot, CloudAction};
    use super::*;
    use lapinux_core::paths::LapinuxPaths;

    #[test]
    fn game_display_overrides_reach_the_gamescope_launch() {
        let mut config = lapinux_core::Config::default();
        config.graphics.sharpness = 3;
        config.graphics.scaling_filter = lapinux_core::config::ScalingFilter::Fsr;
        let mut game = Game::default();
        game.settings.sharpness = Some(11);
        game.settings.scaling_filter = Some(lapinux_core::config::ScalingFilter::Nearest);
        game.settings.gamescope = Some(true);
        let perf = perf_config(&config, &game, None, None);
        assert_eq!(
            perf.gamescope.sharpness, 11,
            "le jeu prime sur la netteté globale"
        );
        assert_eq!(
            perf.gamescope.filter,
            lapinux_core::config::ScalingFilter::Nearest
        );
        assert!(perf.gamescope.enabled);

        game.settings.sharpness = None;
        game.settings.scaling_filter = None;
        game.settings.gamescope = None;
        let inherited = perf_config(&config, &game, None, None);
        assert_eq!(inherited.gamescope.sharpness, 3);
        assert_eq!(
            inherited.gamescope.filter,
            lapinux_core::config::ScalingFilter::Fsr
        );
    }

    #[test]
    fn no_input_source_means_nothing_can_be_hidden() {
        assert!(input_sources_to_hide(&[]).is_empty());
    }

    #[test]
    fn clearing_cache_preserves_custom_covers_and_store_installers() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        let shared = Shared::new(&paths).unwrap();
        std::fs::create_dir_all(paths.covers_dir()).unwrap();
        std::fs::create_dir_all(paths.stores_cache_dir()).unwrap();
        std::fs::write(paths.covers_dir().join("custom-42.jpg"), b"chosen").unwrap();
        std::fs::write(paths.covers_dir().join("42.jpg"), b"cache").unwrap();
        std::fs::write(paths.stores_cache_dir().join("installer.part"), b"download").unwrap();
        std::fs::create_dir_all(paths.data_dir.join("artwork")).unwrap();
        std::fs::write(paths.custom_cover_path(43), b"chosen").unwrap();
        shared.clear_caches().unwrap();
        assert!(paths.covers_dir().join("custom-42.jpg").is_file());
        assert!(!paths.covers_dir().join("42.jpg").exists());
        assert!(paths.stores_cache_dir().join("installer.part").is_file());
        assert!(paths.custom_cover_path(43).is_file());
    }

    #[test]
    fn clearing_cache_does_not_follow_symlinks() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        let shared = Shared::new(&paths).unwrap();
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("keep"), b"important").unwrap();
        std::fs::create_dir_all(paths.covers_dir()).unwrap();
        symlink(&outside, paths.covers_dir().join("link")).unwrap();
        shared.clear_caches().unwrap();
        assert!(outside.join("keep").is_file());
        symlink(&outside, paths.art_dir()).unwrap();
        assert!(shared.clear_caches().is_err());
        assert!(outside.join("keep").is_file());
    }

    #[test]
    fn clearing_cache_rejects_a_save_root_inside_it() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        let shared = Shared::new(&paths).unwrap();
        let saves = paths.covers_dir().join("saved-games");
        std::fs::create_dir_all(&saves).unwrap();
        std::fs::write(saves.join("save"), b"important").unwrap();
        shared.config.lock().unwrap().general.saves_root = saves.to_string_lossy().into();
        assert!(shared.clear_caches().is_err());
        assert!(saves.join("save").is_file());
    }

    #[test]
    fn clearing_cache_rejects_a_library_containing_the_cache() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        let shared = Shared::new(&paths).unwrap();
        std::fs::create_dir_all(paths.covers_dir()).unwrap();
        std::fs::write(paths.covers_dir().join("image.jpg"), b"keep").unwrap();
        shared.config.lock().unwrap().general.library_root = dir.path().to_string_lossy().into();
        assert!(shared.clear_caches().is_err());
        assert!(paths.covers_dir().join("image.jpg").is_file());
    }

    #[test]
    fn a_game_read_is_served_from_cache_until_it_expires() {
        let mut cache = GameReadCache::default();
        assert!(!cache.is_fresh("succès", 10));
        // Une seule lecture à la fois par jeu : la seconde ne démarre pas.
        assert!(cache.begin("succès", 10));
        assert!(!cache.begin("succès", 10));
        cache.finish("succès", 10, serde_json::json!({ "ok": true }), false);
        assert!(cache.is_fresh("succès", 10));
        assert_eq!(
            cache.any("succès", 10),
            Some(serde_json::json!({ "ok": true }))
        );
        // La fiche et les succès ne se mélangent pas.
        assert!(!cache.is_fresh("info", 10));
        // Passé le délai, la valeur reste connue sans être fraîche.
        cache.entries.get_mut(&("succès", 10)).unwrap().at =
            Instant::now() - GAME_READ_TTL - Duration::from_secs(1);
        assert!(!cache.is_fresh("succès", 10));
        assert_eq!(
            cache.any("succès", 10),
            Some(serde_json::json!({ "ok": true }))
        );
    }

    #[test]
    fn le_cache_evince_les_lectures_sur_disque_les_moins_utilisees() {
        let mut cache = GameReadCache::default();
        for appid in 0..GAME_READ_DISK_BACKED_LIMIT as u32 {
            cache.finish("info", appid, serde_json::json!(appid), true);
        }
        // Fixe les dates pour tester le choix sans dépendre de l'horloge.
        cache.entries.get_mut(&("info", 0)).unwrap().last_used =
            Instant::now() - Duration::from_secs(2);
        cache.entries.get_mut(&("info", 1)).unwrap().last_used =
            Instant::now() - Duration::from_secs(1);
        assert_eq!(cache.any("info", 0), Some(serde_json::json!(0)));
        cache.finish("info", 999, serde_json::json!(999), true);
        assert_eq!(cache.entries.len(), GAME_READ_DISK_BACKED_LIMIT);
        assert!(cache.any("info", 1).is_none());
        assert_eq!(cache.any("info", 0), Some(serde_json::json!(0)));
    }

    #[test]
    fn le_cache_garde_les_replis_sans_copie_et_les_lectures_en_cours() {
        let mut cache = GameReadCache::default();
        cache.finish(
            "taille-boutique",
            1,
            serde_json::json!("sans disque"),
            false,
        );
        for appid in 0..GAME_READ_DISK_BACKED_LIMIT as u32 {
            cache.finish("info", appid, serde_json::json!(appid), true);
        }
        cache.entries.get_mut(&("info", 0)).unwrap().last_used =
            Instant::now() - Duration::from_secs(2);
        assert!(cache.begin("info", 0));
        cache.finish("info", 999, serde_json::json!(999), true);
        assert_eq!(cache.any("info", 0), Some(serde_json::json!(0)));
        assert_eq!(
            cache.any("taille-boutique", 1),
            Some(serde_json::json!("sans disque"))
        );
        assert_eq!(
            cache
                .entries
                .values()
                .filter(|entry| entry.disk_backed)
                .count(),
            GAME_READ_DISK_BACKED_LIMIT
        );
        cache.abandon("info", 0);
    }

    #[test]
    fn une_fiche_evincee_reste_disponible_sur_disque_sans_reseau() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        let shared = std::sync::Arc::new(Shared::new(&paths).unwrap());
        let expected = serde_json::json!({"titre": "repli"});
        for appid in 0..=GAME_READ_DISK_BACKED_LIMIT as u32 {
            let value = expected.clone();
            let reply = shared
                .cached_remote_read("info", appid, move || Ok(value.clone()))
                .unwrap();
            assert_eq!(reply.data, expected);
            if appid == 0 {
                shared
                    .game_reads
                    .lock()
                    .unwrap()
                    .entries
                    .get_mut(&("info", 0))
                    .unwrap()
                    .last_used = Instant::now() - Duration::from_secs(2);
            }
        }
        assert!(shared.game_reads.lock().unwrap().any("info", 0).is_none());
        let (tx, rx) = std::sync::mpsc::channel();
        let reply = shared
            .cached_remote_read("info", 0, move || {
                let _ = tx.send(());
                Err(lapinux_core::Error::Other("réseau indisponible".into()))
            })
            .unwrap();
        assert_eq!(reply.data, expected);
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let reply = shared
            .cached_remote_read("info", 0, || {
                Err(lapinux_core::Error::Other("réseau indisponible".into()))
            })
            .unwrap();
        assert_eq!(reply.data, expected);
    }

    #[test]
    fn un_echec_d_ecriture_conserve_la_fiche_en_memoire() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        let shared = std::sync::Arc::new(Shared::new(&paths).unwrap());
        let fiches = paths.cache_dir.join(STEAM_SHEET_DIR);
        std::fs::remove_dir_all(&fiches).unwrap();
        std::fs::write(&fiches, b"pas un dossier").unwrap();
        let reply = shared
            .cached_remote_read("info", 42, || Ok(serde_json::json!("repli")))
            .unwrap();
        assert_eq!(reply.data, serde_json::json!("repli"));
        let mut cache = shared.game_reads.lock().unwrap();
        assert!(!cache.entries.get(&("info", 42)).unwrap().disk_backed);
        for appid in 0..=GAME_READ_DISK_BACKED_LIMIT as u32 {
            cache.finish("boutique", appid, serde_json::json!(appid), true);
        }
        assert_eq!(cache.any("info", 42), Some(serde_json::json!("repli")));
    }

    /// Le ménage du dossier de fiches retire les restes d'écriture et borne
    /// le nombre de fichiers gardés.
    #[test]
    fn le_menage_du_cache_de_fiches_borne_le_dossier() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        let fiches = paths.cache_dir.join(STEAM_SHEET_DIR);
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

    /// Les versions de cache de l'ancienne disposition ne doivent pas rester
    /// à côté des dossiers stables.
    #[test]
    fn les_anciens_dossiers_de_fiches_sont_supprimes() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        for old in ["fiches-v8", "fiches-boutique-v4", "fiches-boutique-v2"] {
            let path = paths.cache_dir.join(old);
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join("old.json"), b"{}").unwrap();
        }
        let stable = paths.cache_dir.join(STEAM_SHEET_DIR);
        std::fs::create_dir_all(&stable).unwrap();
        std::fs::write(stable.join("before.json"), b"{}").unwrap();
        let shared = Shared::new(&paths).unwrap();
        shared.prune_game_info_cache();
        for old in ["fiches-v8", "fiches-boutique-v4", "fiches-boutique-v2"] {
            assert!(!paths.cache_dir.join(old).exists(), "{old} doit partir");
        }
        assert!(stable.is_dir());
        assert!(!stable.join("before.json").exists());
        assert!(paths.cache_dir.join(STORE_SHEET_DIR).is_dir());
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
            finished: matches!(
                state,
                JobState::Done | JobState::Failed | JobState::Cancelled
            )
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
            assert_eq!(
                jobs.len(),
                3,
                "seule la carte terminée disparaît : {jobs:?}"
            );
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
        assert_eq!(
            normalize_platform(Some(" Linux ".into())).as_deref(),
            Some("linux")
        );
        assert_eq!(
            normalize_platform(Some("windows".into())).as_deref(),
            Some("windows")
        );
        assert_eq!(
            normalize_platform(Some("macOS".into())).as_deref(),
            Some("macos")
        );
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
            // Le même paquet partagé par un second membre : les deux
            // propriétaires comptent, comme pour deux achats du même jeu.
            cm::License {
                package_id: 300,
                owner_id: 8,
                ..cm::License::default()
            },
        ];
        let (packages, family) = split_license_packages(&licences, 999);
        assert_eq!(packages, vec![100, 100, 200, 300, 400, 500, 500, 300]);
        assert!(
            !family.contains_key(&100),
            "un jeu possédé n'est pas familial"
        );
        assert!(!family.contains_key(&200));
        assert_eq!(
            family.get(&300),
            Some(&vec![
                76_561_197_960_265_728 + 7,
                76_561_197_960_265_728 + 8,
            ]),
            "un jeu prêté garde tous ses propriétaires"
        );
        assert!(!family.contains_key(&400));
        assert!(!family.contains_key(&500));
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
    fn pe_version_resource_is_checked_and_extracted() {
        fn p16(d: &mut [u8], p: usize, n: u16) {
            d[p..p + 2].copy_from_slice(&n.to_le_bytes());
        }
        fn p32(d: &mut [u8], p: usize, n: u32) {
            d[p..p + 4].copy_from_slice(&n.to_le_bytes());
        }
        let mut d = vec![0u8; 1024];
        d[..2].copy_from_slice(b"MZ");
        p32(&mut d, 0x3c, 0x80);
        d[0x80..0x84].copy_from_slice(b"PE\0\0");
        p16(&mut d, 0x86, 1);
        p16(&mut d, 0x94, 0xf0);
        let opt = 0x98;
        p16(&mut d, opt, 0x20b);
        p32(&mut d, opt + 108, 16);
        p32(&mut d, opt + 128, 0x1000);
        p32(&mut d, opt + 132, 0x180);
        let section = opt + 0xf0;
        p32(&mut d, section + 12, 0x1000);
        p32(&mut d, section + 16, 0x200);
        p32(&mut d, section + 20, 0x200);
        let r = 0x200;
        p16(&mut d, r + 14, 1);
        p32(&mut d, r + 16, 16);
        p32(&mut d, r + 20, 0x80000020);
        p16(&mut d, r + 0x20 + 14, 1);
        p32(&mut d, r + 0x20 + 20, 0x80000040);
        p16(&mut d, r + 0x40 + 14, 1);
        p32(&mut d, r + 0x40 + 20, 0x60);
        p32(&mut d, r + 0x60, 0x1080);
        p32(&mut d, r + 0x64, 92);
        let v = r + 0x80;
        p16(&mut d, v, 92);
        p16(&mut d, v + 2, 52);
        for (i, unit) in "VS_VERSION_INFO"
            .encode_utf16()
            .chain(std::iter::once(0))
            .enumerate()
        {
            p16(&mut d, v + 6 + i * 2, unit);
        }
        p32(&mut d, v + 40, 0xfeef04bd);
        p32(&mut d, v + 48, (310 << 16) | 2);
        p32(&mut d, v + 52, (1 << 16) | 7);
        assert_eq!(pe_file_version(&d), Some([310, 2, 1, 7]));
        assert_eq!(pe_file_version(&d[..300]), None);
        p32(&mut d, v + 40, 0);
        assert_eq!(pe_file_version(&d), None);
    }

    #[test]
    fn dlss_libraries_are_recognised() {
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("Binaries/Win64");
        std::fs::create_dir_all(&game).unwrap();
        assert!(detect_tech(dir.path()).dlss.is_empty());

        std::fs::write(game.join("nvngx_dlss.dll"), b"x").unwrap();
        let info = detect_tech(dir.path()).dlss;
        assert!(info.dlss);
        assert!(!info.frame_generation);
        assert_eq!(info.summary(), "DLSS");
        assert_eq!(info.short(), "DLSS");

        std::fs::write(game.join("nvngx_dlssg.dll"), b"x").unwrap();
        std::fs::write(game.join("nvngx_dlssd.dll"), b"x").unwrap();
        let info = detect_tech(dir.path()).dlss;
        assert_eq!(info.summary(), "DLSS · images générées · rayons");
        assert_eq!(info.short(), "DLSS · FG · RR");
    }

    #[test]
    fn engine_upscalers_and_anti_cheat_are_recognised() {
        use lapinux_core::model::{AntiCheat, GameEngine, Upscaler};
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("Jeu/Binaries/Win64");
        std::fs::create_dir_all(&game).unwrap();
        std::fs::create_dir_all(dir.path().join("Jeu/Content")).unwrap();
        std::fs::write(game.join("Jeu-Win64-Shipping.exe"), b"x").unwrap();
        std::fs::write(game.join("libxess.dll"), b"x").unwrap();
        std::fs::write(game.join("amd_fidelityfx_dx12.dll"), b"x").unwrap();
        std::fs::create_dir_all(dir.path().join("Jeu/BattlEye")).unwrap();

        let tech = detect_tech(dir.path());
        assert_eq!(tech.engine, Some(GameEngine::Unreal));
        assert_eq!(tech.upscalers, vec![Upscaler::Fsr, Upscaler::Xess]);
        assert_eq!(tech.anti_cheat, vec![AntiCheat::BattlEye]);
        assert_eq!(tech.upscaling(), "FSR · XeSS");
        assert_eq!(tech.anti_cheat(), "BattlEye");

        // Un jeu Unity n'annonce pas Unreal, et le vide reste vide.
        std::fs::remove_file(game.join("Jeu-Win64-Shipping.exe")).unwrap();
        std::fs::write(dir.path().join("Jeu/UnityPlayer.dll"), b"x").unwrap();
        assert_eq!(detect_tech(dir.path()).engine, Some(GameEngine::Unity));
        let empty = detect_tech(&dir.path().join("absent"));
        assert_eq!(empty, lapinux_core::model::GameTech::default());
        assert_eq!(empty.upscaling(), "—");
    }

    #[test]
    fn un_anti_triche_coupe_pont_injecteurs_et_enregistrement() {
        use lapinux_core::config::FrameGeneration;
        use lapinux_core::model::GameSettings;
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("Jeu/EasyAntiCheat")).unwrap();
        let tech = detect_tech(dir.path());
        assert!(tech.protected());
        assert_eq!(tech.anti_cheat(), "Easy Anti-Cheat");

        // Les défauts du jeu protégé coupent tout ce qui s'injecte.
        let mut settings = GameSettings::default();
        settings.apply_protected_defaults(false);
        assert_eq!(settings.steam_shim, Some(false));
        assert_eq!(settings.vkbasalt, Some(false));
        assert_eq!(settings.hud_on_screen, Some(false));
        assert_eq!(settings.record_on_launch, Some(false));
        assert_eq!(settings.frame_generation, Some(FrameGeneration::Off));

        // Un choix explicite du joueur reste : la protection ne remplit que
        // les champs qu'il n'a jamais décidés.
        let mut chosen = GameSettings::default();
        chosen.vkbasalt = Some(true);
        chosen.apply_protected_defaults(false);
        assert_eq!(chosen.vkbasalt, Some(true));

        // Un jeu natif n'a pas de pont Windows à couper.
        let mut native = GameSettings::default();
        native.apply_protected_defaults(true);
        assert_eq!(native.steam_shim, None);
        assert_eq!(native.record_on_launch, Some(false));
    }

    #[test]
    fn cloud_comparison_and_recovery_refuse_stale_or_incomplete_data() {
        use lapinux_core::model::{CloudFile, CloudSaves, SteamCloudState};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("save.sav");
        std::fs::write(&path, b"local").unwrap();
        let remote = b"remote".to_vec();
        let file = CloudFile {
            name: "save.sav".into(),
            sha: lapinux_steam::cloud::file_sha(&remote),
            local_resolved: true,
            local_sha: Some(lapinux_steam::cloud::file_sha(b"local")),
            ..Default::default()
        };
        let cloud = CloudSaves {
            files: vec![file],
            change_number: 1,
            ..Default::default()
        };
        assert_eq!(cloud.steam_state(), SteamCloudState::Conflict);
        let mut stale = cloud.clone();
        stale.change_number += 1;
        assert!(!same_cloud_snapshot(&cloud, &stale));
        let mut unknown = cloud.clone();
        unknown.files[0].local_resolved = false;
        assert_eq!(unknown.steam_state(), SteamCloudState::Unknown);
        let mut identical = cloud.clone();
        identical.files[0].local_sha = Some(identical.files[0].sha.clone());
        assert_eq!(identical.steam_state(), SteamCloudState::UpToDate);
        // Une copie locale qui correspond à une version écartée du même chemin
        // (celle d'une autre machine) est déjà une version du cloud : il n'y a
        // pas de conflit à trancher.
        let mut variant = cloud.clone();
        let mut old = variant.files[0].clone();
        old.sha = variant.files[0].local_sha.clone().unwrap();
        old.modified -= 10;
        variant.superseded = vec![old];
        assert_eq!(variant.steam_state(), SteamCloudState::UpToDate);
        let game = Game {
            appid: 42,
            ..Default::default()
        };
        let local = vec![(path.clone(), Some(b"local".to_vec()))];
        let backup =
            save_cloud_recovery(
                dir.path(),
                &game,
                &cloud,
                &local,
                &[Some(remote.clone())],
                &[],
            )
                .unwrap();
        assert_eq!(std::fs::read(backup.join("0-local")).unwrap(), b"local");
        assert_eq!(std::fs::read(backup.join("0-cloud")).unwrap(), remote);
        assert!(save_cloud_recovery(dir.path(), &game, &cloud, &local, &[], &[]).is_err());
        std::fs::write(&path, b"newer").unwrap();
        assert!(write_cloud_file(&path, b"remote", Some(b"local")).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"newer");
        assert!(write_cloud_file(&path, b"remote", None).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"newer");
    }

    #[test]
    fn cloud_sync_uploads_changes_and_deletes_files_gone_since_launch() {
        use lapinux_core::model::{CloudFile, CloudSaves};
        let data = b"progression";
        let file = CloudFile {
            prefix: "saves/".into(),
            name: "slot.sav".into(),
            sha: lapinux_steam::cloud::file_sha(data),
            ..Default::default()
        };
        assert_eq!(cloud_action(&file, Some(data), None), CloudAction::Skip);
        assert_eq!(
            cloud_action(&file, Some(b"autre"), None),
            CloudAction::Upload
        );
        let before = CloudSaves {
            files: vec![file.clone()],
            ..Default::default()
        };
        assert_eq!(
            cloud_action(&file, None, Some(&before)),
            CloudAction::Delete
        );
        assert_eq!(cloud_action(&file, None, None), CloudAction::IgnoreMissing);
        assert_eq!(
            cloud_action(&file, None, Some(&CloudSaves::default())),
            CloudAction::IgnoreMissing
        );
        let mut unknown = file;
        unknown.sha.clear();
        assert_eq!(
            cloud_action(&unknown, Some(b"autre"), None),
            CloudAction::Conflict
        );
    }

    #[test]
    fn uninstall_with_backup_refuses_unknown_and_preserves_game() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        let shared = Shared::new(&paths).unwrap();
        let library = dir.path().join("library");
        let install = library.join("Steam/Test");
        std::fs::create_dir_all(&install).unwrap();
        std::fs::write(install.join("game.exe"), b"game").unwrap();
        let game = Game {
            appid: 42,
            name: "Test".into(),
            installdir: "Test".into(),
            library,
            origin: lapinux_core::model::GameOrigin::Lapinux,
            installed: true,
            // Un jeu déjà lancé garde la règle stricte : sans dossier de
            // sauvegarde détecté, on ne supprime pas ses fichiers.
            playtime_secs: 60,
            ..Default::default()
        };
        shared.store.save_game(&game).unwrap();
        assert!(shared.uninstall_game_with_backup(42).is_err());
        assert!(install.join("game.exe").exists());
        let saves = install.join("saves");
        std::fs::create_dir_all(&saves).unwrap();
        std::fs::write(saves.join("save"), b"progress").unwrap();
        shared.uninstall_game_with_backup(42).unwrap();
        assert!(!install.exists());
        let copies = crate::saves::list(&game, &paths, &shared.config_snapshot());
        assert_eq!(
            std::fs::read(Path::new(&copies[0].path).join("save")).unwrap(),
            b"progress"
        );
    }

    #[test]
    fn a_never_launched_game_uninstalls_without_a_save_copy() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        let shared = Shared::new(&paths).unwrap();
        let library = dir.path().join("library");
        let install = library.join("Steam/Test");
        std::fs::create_dir_all(&install).unwrap();
        std::fs::write(install.join("game.exe"), b"game").unwrap();
        let game = Game {
            appid: 43,
            name: "Test jamais lance".into(),
            installdir: "Test".into(),
            library,
            origin: lapinux_core::model::GameOrigin::Lapinux,
            installed: true,
            ..Default::default()
        };
        shared.store.save_game(&game).unwrap();
        let result = shared.uninstall_game_with_backup(43).unwrap();
        assert!(result
            .data
            .get("backup")
            .is_some_and(|value| value.is_null()));
        assert!(!install.exists());
        assert!(shared.store.game(43).unwrap().is_none());
    }

    #[test]
    fn uninstall_with_backup_covers_all_four_stores() {
        use lapinux_core::model::GameOrigin;
        use lapinux_core::stores::Store;
        let temp = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(temp.path());
        let shared = Shared::new(&paths).unwrap();
        for (index, store) in [Store::Steam, Store::Epic, Store::Gog, Store::Itch]
            .into_iter()
            .enumerate()
        {
            let appid = 700 + index as u32;
            let origin = if store == Store::Steam {
                GameOrigin::Steam
            } else {
                GameOrigin::Store
            };
            let mut game = Game {
                appid,
                store,
                origin,
                name: format!("Test {appid}"),
                installdir: format!("test-{appid}"),
                library: temp.path().join("library"),
                installed: true,
                ..Default::default()
            };
            game.settings.save_dir = Some("saves".into());
            let install = game.install_dir();
            std::fs::create_dir_all(&install).unwrap();
            std::fs::write(install.join("game.exe"), b"game").unwrap();
            shared.store.save_game(&game).unwrap();
            // L'ancienne commande ne doit pas contourner la copie vérifiée.
            assert!(shared.uninstall_game(appid).is_err(), "{}", store.label());
            assert!(install.join("game.exe").exists());
            std::fs::create_dir_all(install.join("saves")).unwrap();
            std::fs::write(install.join("saves/progress"), b"progress").unwrap();
            std::os::unix::fs::symlink("progress", install.join("saves/link")).unwrap();
            assert!(shared.uninstall_game_with_backup(appid).is_err());
            assert!(install.join("game.exe").exists());
            std::fs::remove_file(install.join("saves/link")).unwrap();
            shared.uninstall_game_with_backup(appid).unwrap();
            assert!(!install.exists());
            let copies = crate::saves::list(&game, &paths, &shared.config_snapshot());
            assert_eq!(
                std::fs::read(Path::new(&copies[0].path).join("progress")).unwrap(),
                b"progress"
            );
        }
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

        shared.uninstall_game_locked(42).unwrap();
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
        assert!(shared.uninstall_game_locked(43).is_err());
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

/// Lit uniquement la ressource RT_VERSION d'une DLL PE, avec des bornes
/// strictes ; une bibliothèque inconnue ou corrompue ne donne aucune version.
/// Version d'une DLL, pour les modules qui comparent des bibliothèques.
pub(crate) fn dll_version(path: &Path) -> Option<[u16; 4]> {
    dll_file_version(path)
}

fn dll_file_version(path: &Path) -> Option<[u16; 4]> {
    use std::io::Read;
    let file = std::fs::File::open(path).ok()?;
    if file.metadata().ok()?.len() > 64 * 1024 * 1024 {
        return None;
    }
    let mut data = Vec::new();
    file.take(64 * 1024 * 1024 + 1)
        .read_to_end(&mut data)
        .ok()?;
    if data.len() > 64 * 1024 * 1024 {
        return None;
    }
    pe_file_version(&data)
}

fn pe_file_version(data: &[u8]) -> Option<[u16; 4]> {
    fn u16_at(d: &[u8], p: usize) -> Option<u16> {
        Some(u16::from_le_bytes(
            d.get(p..p.checked_add(2)?)?.try_into().ok()?,
        ))
    }
    fn u32_at(d: &[u8], p: usize) -> Option<u32> {
        Some(u32::from_le_bytes(
            d.get(p..p.checked_add(4)?)?.try_into().ok()?,
        ))
    }
    if data.get(..2)? != b"MZ" {
        return None;
    }
    let pe = u32_at(data, 0x3c)? as usize;
    if data.get(pe..pe.checked_add(4)?)? != b"PE\0\0" {
        return None;
    }
    let sections = u16_at(data, pe.checked_add(6)?)? as usize;
    let optional_size = u16_at(data, pe.checked_add(20)?)? as usize;
    let opt = pe.checked_add(24)?;
    let opt_end = opt.checked_add(optional_size)?;
    if opt_end > data.len() || sections > 96 {
        return None;
    }
    let (directories, count_offset) = match u16_at(data, opt)? {
        0x10b => (96usize, 92usize),
        0x20b => (112usize, 108usize),
        _ => return None,
    };
    if count_offset.checked_add(4)? > optional_size
        || u32_at(data, opt.checked_add(count_offset)?)? < 3
        || directories.checked_add(24)? > optional_size
    {
        return None;
    }
    let resource_rva = u32_at(data, opt.checked_add(directories + 16)?)?;
    let resource_size = u32_at(data, opt.checked_add(directories + 20)?)? as usize;
    if resource_rva == 0 || resource_size < 16 {
        return None;
    }
    let table = opt_end;
    let rva_to_file = |rva: u32, len: usize| -> Option<usize> {
        for i in 0..sections {
            let section = table.checked_add(i.checked_mul(40)?)?;
            if section.checked_add(40)? > data.len() {
                return None;
            }
            let address = u32_at(data, section + 12)?;
            let size = u32_at(data, section + 16)? as usize;
            let offset = u32_at(data, section + 20)? as usize;
            if let Some(delta) = rva.checked_sub(address).map(|v| v as usize) {
                if delta.checked_add(len)? <= size {
                    let pos = offset.checked_add(delta)?;
                    if pos.checked_add(len)? <= data.len() {
                        return Some(pos);
                    }
                }
            }
        }
        None
    };
    let base = rva_to_file(resource_rva, resource_size)?;
    let resources = data.get(base..base.checked_add(resource_size)?)?;
    fn child(d: &[u8], dir: usize, wanted: Option<u32>, directory: bool) -> Option<usize> {
        let count =
            u16_at(d, dir.checked_add(12)?)? as usize + u16_at(d, dir.checked_add(14)?)? as usize;
        if count > 4096 {
            return None;
        }
        for i in 0..count {
            let pos = dir.checked_add(16)?.checked_add(i.checked_mul(8)?)?;
            let name = u32_at(d, pos)?;
            let target = u32_at(d, pos.checked_add(4)?)?;
            if wanted.is_none_or(|id| name == id) && (target & 0x8000_0000 != 0) == directory {
                return Some((target & 0x7fff_ffff) as usize);
            }
        }
        None
    }
    let kind = child(resources, 0, Some(16), true)?;
    let name = child(resources, kind, None, true)?;
    let entry = child(resources, name, None, false)?;
    let value_rva = u32_at(resources, entry)?;
    let size = u32_at(resources, entry.checked_add(4)?)? as usize;
    if size < 40 || size > 1024 * 1024 {
        return None;
    }
    let pos = rva_to_file(value_rva, size)?;
    let version = data.get(pos..pos.checked_add(size)?)?;
    let total = u16_at(version, 0)? as usize;
    let fixed_len = u16_at(version, 2)? as usize;
    if total > size || fixed_len < 52 {
        return None;
    }
    let expected: Vec<u16> = "VS_VERSION_INFO"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    for (i, expected) in expected.iter().enumerate() {
        if u16_at(version, 6 + i * 2)? != *expected {
            return None;
        }
    }
    let fixed = (6 + expected.len() * 2 + 3) & !3;
    if fixed.checked_add(fixed_len)? > total || u32_at(version, fixed)? != 0xFEEF04BD {
        return None;
    }
    let ms = u32_at(version, fixed + 8)?;
    let ls = u32_at(version, fixed + 12)?;
    Some([(ms >> 16) as u16, ms as u16, (ls >> 16) as u16, ls as u16])
}

/// Repère le moteur, les suréchantillonneurs et l'anti-triche dans le dossier
/// du jeu. Les noms de fichiers et de dossiers suffisent : les jeux rangent
/// leurs bibliothèques à côté de leur exécutable (Unreal dans le dossier du
/// moteur), et le jeu n'est jamais lancé pour autant.
pub(crate) fn detect_tech(root: &Path) -> lapinux_core::model::GameTech {
    use lapinux_core::model::{AntiCheat, GameEngine, GameTech, Upscaler};
    let mut tech = GameTech::default();
    if !root.is_dir() {
        return tech;
    }
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    let mut seen = 0usize;
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            seen += 1;
            if seen > 40_000 {
                break;
            }
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if kind.is_dir() {
                match name.as_str() {
                    "easyanticheat" | "easyanticheat_eos" => {
                        push(&mut tech.anti_cheat, AntiCheat::EasyAntiCheat)
                    }
                    "battleye" => push(&mut tech.anti_cheat, AntiCheat::BattlEye),
                    _ => {}
                }
                // Les préfixes Proton rangent des milliers de fichiers qui ne
                // disent rien du jeu lui-même.
                if depth < 3 && !name.starts_with("prefix") && name != "drive_c" && name != "pfx" {
                    stack.push((path, depth + 1));
                }
                continue;
            }
            let engine = match name.as_str() {
                "unityplayer.dll" => Some(GameEngine::Unity),
                "data.win" => Some(GameEngine::GameMaker),
                "gameinfo.txt" | "hl2.exe" => Some(GameEngine::Source),
                _ if name.ends_with("-win64-shipping.exe") || name.ends_with("-win32-shipping.exe") => {
                    Some(GameEngine::Unreal)
                }
                _ if name.ends_with(".pck") => Some(GameEngine::Godot),
                _ if name.ends_with(".rpa") => Some(GameEngine::RenPy),
                _ if name.starts_with("game.rgss") => Some(GameEngine::RpgMaker),
                _ => None,
            };
            if let Some(engine) = engine {
                tech.engine.get_or_insert(engine);
            }
            match name.as_str() {
                "nvngx_dlss.dll" => {
                    tech.dlss.dlss = true;
                    tech.dlss.dlss_version = tech.dlss.dlss_version.or_else(|| dll_file_version(&path));
                }
                "nvngx_dlssg.dll" => {
                    tech.dlss.frame_generation = true;
                    tech.dlss.frame_generation_version = tech
                        .dlss
                        .frame_generation_version
                        .or_else(|| dll_file_version(&path));
                }
                "nvngx_dlssd.dll" => {
                    tech.dlss.ray_reconstruction = true;
                    tech.dlss.ray_reconstruction_version = tech
                        .dlss
                        .ray_reconstruction_version
                        .or_else(|| dll_file_version(&path));
                }
                "amd_fidelityfx_upscaler_dx12.dll"
                | "amd_fidelityfx_dx12.dll"
                | "ffx_fsr2_api_x64.dll" => push(&mut tech.upscalers, Upscaler::Fsr),
                "libxess.dll" | "libxess_dx11.dll" => push(&mut tech.upscalers, Upscaler::Xess),
                "easyanticheat.exe" | "easyanticheat_eos.dll" => {
                    push(&mut tech.anti_cheat, AntiCheat::EasyAntiCheat)
                }
                "beservice.exe" => push(&mut tech.anti_cheat, AntiCheat::BattlEye),
                _ => {}
            }
        }
    }
    // L'ordre de découverte dépend du disque : la fiche doit être stable.
    tech.upscalers.sort();
    tech.anti_cheat.sort();
    tech
}

fn push<T: PartialEq>(list: &mut Vec<T>, value: T) {
    if !list.contains(&value) {
        list.push(value);
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
