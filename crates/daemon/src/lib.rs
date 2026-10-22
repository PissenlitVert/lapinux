//! Daemon Ludus : bibliothèque, téléchargements, sessions de jeu et overlay.

pub mod doctor;
pub mod jobs;
pub mod launch;

use ludus_core::config::Config;
use ludus_core::ipc::{self, Envelope, Req};
use ludus_core::model::{Game, Job, ProtonInfo, SessionInfo};
use ludus_core::paths::LudusPaths;
use ludus_core::store::Store;
use ludus_steam::depotdownloader::DepotDownloader;
use ludus_steam::steamcmd::SteamCmd;
use std::collections::HashMap;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub type Result<T> = std::result::Result<T, ludus_core::Error>;

pub struct Shared {
    pub paths: LudusPaths,
    pub store: Arc<Store>,
    pub config: Mutex<Config>,
    pub jobs: Mutex<Vec<Job>>,
    pub cancel: Mutex<HashMap<String, Arc<AtomicBool>>>,
    pub protons: Mutex<Vec<ProtonInfo>>,
    pub session: Mutex<Option<launch::SessionState>>,
    pub overlay: Mutex<Option<std::process::Child>>,
    pub recorder: Mutex<Option<std::process::Child>>,
    pub steamcmd: SteamCmd,
    pub depot: DepotDownloader,
}

impl Shared {
    pub fn new(paths: &LudusPaths) -> Result<Self> {
        paths.ensure().map_err(ludus_core::Error::Io)?;
        let config = Config::load_or_create(paths)?;
        let store = Arc::new(Store::open(&paths.db_file)?);
        Ok(Self {
            paths: paths.clone(),
            store,
            config: Mutex::new(config),
            jobs: Mutex::new(Vec::new()),
            cancel: Mutex::new(HashMap::new()),
            protons: Mutex::new(Vec::new()),
            session: Mutex::new(None),
            overlay: Mutex::new(None),
            recorder: Mutex::new(None),
            steamcmd: SteamCmd::new(&paths.data_dir),
            depot: DepotDownloader::new(&paths.data_dir),
        })
    }

    pub fn config_snapshot(&self) -> Config {
        self.config
            .lock()
            .map(|c| c.clone())
            .unwrap_or_default()
    }

    /// Scanner les bibliothèques Steam et fusionner avec les réglages connus.
    pub fn rescan_library(&self) -> Result<Vec<Game>> {
        let roots = ludus_core::paths::steam_roots();
        let report = ludus_steam::scan(&roots);
        for warning in &report.warnings {
            tracing::warn!("scan : {warning}");
        }
        let stored = self.store.load_games().unwrap_or_default();
        let merged = ludus_steam::merge_existing(report.games, &stored);
        self.store
            .save_games(&merged)
            .map_err(|e| ludus_core::Error::Other(e.to_string()))?;
        Ok(merged)
    }

    pub fn scan_protons(&self) -> Result<Vec<ProtonInfo>> {
        let roots = ludus_core::paths::steam_roots();
        let extra = vec![
            self.paths.proton_dir.clone(),
            ludus_core::paths::home_dir().join(".local/share/umu/compatibilitytools"),
        ];
        let protons = ludus_proton::scan(&roots, &extra);
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

    pub fn update_job(&self, id: &str, update: impl FnOnce(&mut Job)) {
        if let Ok(mut jobs) = self.jobs.lock() {
            if let Some(job) = jobs.iter_mut().find(|j| j.id == id) {
                update(job);
            }
        }
    }
}

pub fn run(paths: LudusPaths, ready: Option<std::sync::mpsc::Sender<()>>) -> Result<()> {
    let shared = Arc::new(Shared::new(&paths)?);

    if paths.socket.exists() {
        let alive = ipc::send_request_timeout(&paths.socket, &Req::Ping, Duration::from_millis(300))
            .is_ok();
        if alive {
            return Err(ludus_core::Error::Ipc(
                "un daemon écoute déjà sur ce socket".into(),
            ));
        }
        let _ = std::fs::remove_file(&paths.socket);
    }

    let _ = shared.rescan_library();
    let _ = shared.scan_protons();

    let listener = bind_socket(&paths)?;
    if let Some(ready) = ready {
        let _ = ready.send(());
    }
    tracing::info!("daemon à l'écoute sur {}", paths.socket.display());

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let shared = Arc::clone(&shared);
                std::thread::spawn(move || {
                    if let Err(err) = handle_connection(shared, stream) {
                        tracing::warn!("connexion IPC : {err}");
                    }
                });
            }
            Err(err) => tracing::warn!("accept : {err}"),
        }
    }
    Ok(())
}

fn bind_socket(paths: &LudusPaths) -> Result<UnixListener> {
    if let Some(parent) = paths.socket.parent() {
        std::fs::create_dir_all(parent).map_err(ludus_core::Error::Io)?;
    }
    let listener = UnixListener::bind(&paths.socket).map_err(|e| {
        ludus_core::Error::Ipc(format!("bind {} : {e}", paths.socket.display()))
    })?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&paths.socket, std::fs::Permissions::from_mode(0o600)).ok();
    Ok(listener)
}

fn handle_connection(shared: Arc<Shared>, mut stream: UnixStream) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(30))).ok();
    let payload = ipc::read_frame(&mut stream)?;
    let request: Req = match serde_json::from_slice(&payload) {
        Ok(request) => request,
        Err(err) => {
            let envelope = Envelope::err(format!("requête invalide : {err}"));
            let body = serde_json::to_vec(&envelope).unwrap_or_default();
            ipc::write_frame(&mut stream, &body)?;
            return Ok(());
        }
    };
    let envelope = dispatch(&shared, request);
    let body = serde_json::to_vec(&envelope).unwrap_or_default();
    ipc::write_frame(&mut stream, &body)
}

fn dispatch(shared: &Arc<Shared>, request: Req) -> Envelope {
    let result: Result<Envelope> = match request {
        Req::Ping => Ok(Envelope::unit()),
        Req::Scan => shared.rescan_library().map(|games| Envelope::ok(&games)),
        Req::Games => shared
            .store
            .load_games()
            .map(|games| Envelope::ok(&games))
            .map_err(|e| ludus_core::Error::Other(e.to_string())),
        Req::Game { appid } => shared
            .store
            .game(appid)
            .map(|game| Envelope::ok(&game))
            .map_err(|e| ludus_core::Error::Other(e.to_string())),
        Req::SaveSettings { appid, settings } => shared.save_settings(appid, settings),
        Req::Launch { appid, dry_run } => shared.launch(appid, dry_run),
        Req::Stop => shared.stop_session(),
        Req::Session => Ok(Envelope::ok(&shared.session_info())),
        Req::Jobs => Ok(Envelope::ok(
            &shared.jobs.lock().map(|j| j.clone()).unwrap_or_default(),
        )),
        Req::InstallGame { appid } => shared.install_game(appid),
        Req::CancelJob { id } => shared.cancel_job(&id),
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
        Req::Devices => Ok(Envelope::ok(&ludus_input::device::list())),
        Req::Profiles => Ok(Envelope::ok(&ludus_input::list_profiles(
            &shared.paths.data_dir,
        ))),
        Req::Config => Ok(Envelope::ok(&shared.config_snapshot())),
        Req::SetConfig { config } => shared.save_config(config),
        Req::Tools => Ok(Envelope::ok(&serde_json::json!({
            "steamcmd": shared.steamcmd.available(),
            "depotdownloader": shared.depot.available(),
            "steamcmd_path": shared.steamcmd.binary(),
            "depotdownloader_path": shared.depot.binary(),
        }))),
        Req::InstallTools { which } => shared.install_tools(which),
        Req::SteamLoginStart { .. } | Req::SteamLoginSubmit { .. } => Ok(Envelope::ok(
            &serde_json::json!({
                "interactive": false,
                "message": "La connexion Steam se fait dans un terminal : « ludus cli login <compte> » (mot de passe et code 2FA saisis par SteamCMD)."
            }),
        )),
        Req::Stats => Ok(Envelope::ok(
            &shared.store.get_json::<ludus_core::model::Stats>("stats").ok().flatten().unwrap_or_default(),
        )),
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
    use ludus_core::model::Game;

    #[test]
    fn ipc_dispatch_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LudusPaths::under(dir.path());
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
    fn settings_are_persisted() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LudusPaths::under(dir.path());
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
    let repos: Vec<&str> = ludus_proton::SOURCES.iter().map(|s| s.repo).collect();
    let mut releases = Vec::new();
    for repo in repos {
        match ludus_proton::fetch_releases(repo) {
            Ok(mut list) => releases.append(&mut list),
            Err(err) => tracing::warn!("releases {repo} : {err}"),
        }
    }
    Ok(Envelope::ok(&releases))
}
