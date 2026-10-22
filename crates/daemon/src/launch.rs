//! Lancement d'un jeu, supervision de session et contrôle de l'overlay.

use crate::{Result, Shared};
use ludus_core::ipc::Envelope;
use ludus_core::model::{Game, SessionInfo};
use ludus_input::Profile;
use ludus_perf::{mangohud, PerfCaps, PerfConfig};
use ludus_proton::LaunchOptions;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub struct SessionState {
    pub info: SessionInfo,
    pub session: Arc<Mutex<ludus_perf::Session>>,
    pub game_name: String,
}

impl Shared {
    pub fn launch(self: &Arc<Self>, appid: u32, dry_run: bool) -> Result<Envelope> {
        let game = self
            .store
            .game(appid)
            .map_err(|e| ludus_core::Error::Other(e.to_string()))?
            .ok_or_else(|| ludus_core::Error::Other("jeu introuvable".into()))?;
        if !game.installed {
            return Err(ludus_core::Error::Other(
                "le jeu n'est pas installé dans une bibliothèque connue".into(),
            ));
        }
        if self
            .session
            .lock()
            .map(|s| s.is_some())
            .unwrap_or(false)
        {
            return Err(ludus_core::Error::Other("une session est déjà en cours".into()));
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
            ludus_proton::recommended(&protons).cloned()
        } else {
            protons.iter().find(|p| p.name == wanted).cloned()
        };

        if !game.native && proton.is_none() {
            return Err(ludus_core::Error::Other(
                "aucun Proton utilisable : installez GE-Proton depuis l'écran Protons".into(),
            ));
        }
        let exe = game
            .exe
            .clone()
            .ok_or_else(|| ludus_core::Error::Other("exécutable du jeu introuvable".into()))?;
        let prefix = ludus_proton::prefix_for(&game, &self.paths.data_dir);
        ludus_proton::ensure_prefix(&prefix, appid)
            .map_err(|e| ludus_core::Error::Other(e.to_string()))?;

        // Les préfixes déjà gérés par Steam ne doivent pas passer par umu :
        // umu réécrit le fichier `version` du préfixe et Steam le considère
        // alors comme invalide. On garde donc Proton direct pour ces jeux.
        let umu = game.prefix.is_none() && ludus_proton::umu_run_path().is_some();
        let plan = ludus_proton::build(&LaunchOptions {
            appid,
            exe: exe.clone(),
            native: game.native,
            proton: proton.clone(),
            prefix: prefix.clone(),
            steam_root: ludus_core::paths::steam_roots().first().cloned(),
            umu_run: ludus_proton::umu_run_path(),
            use_umu: umu,
            extra_env: game.settings.env.clone(),
            extra_args: game.settings.args.clone(),
        })
        .map_err(|e| ludus_core::Error::Other(e.to_string()))?;

        let caps = PerfCaps::detect(&config.overlay.recorder);
        let perf_config = perf_config(&config, &game);
        let prepared = ludus_perf::prepare(
            &plan,
            appid,
            &perf_config,
            &caps,
            &self.paths.config_dir,
            &self.paths.log_dir,
        )
        .map_err(|e| ludus_core::Error::Other(e.to_string()))?;

        let summary = ludus_perf::layers_summary(prepared.gamescope, prepared.mangohud, prepared.gamemode);

        if dry_run {
            return Ok(Envelope::ok(&serde_json::json!({
                "command": ludus_perf::render_prepared(&prepared),
                "proton_plan": ludus_proton::render_plan(&plan),
                "layers": summary,
                "gamescope": prepared.gamescope,
                "mangohud": prepared.mangohud,
                "gamemode": prepared.gamemode,
                "log": prepared.log_path,
                "drm": game.drm,
                "proton": proton.as_ref().map(|p| p.name.clone()),
            })));
        }

        let mut note = String::new();
        if game.blocked_by_drm() {
            if !config.launch.steam_for_drm {
                return Err(ludus_core::Error::Other(
                    "DRM Steam détecté : activez « lancer Steam en arrière-plan » ou lancez le jeu via Steam".into(),
                ));
            }
            let _ = Command::new("steam").arg("-silent").spawn();
            note = "DRM Steam : client Steam démarré en arrière-plan. ".into();
        }

        let session = ludus_perf::Session::spawn(appid, prepared)
            .map_err(|e| ludus_core::Error::Other(e.to_string()))?;
        let info = SessionInfo {
            appid,
            name: game.name.clone(),
            pid: session.pid,
            started: ludus_core::util::now_unix(),
            proton: proton.map(|p| p.name).unwrap_or_else(|| "natif".into()),
            fps_cap: perf_config.gamescope.fps_cap,
            mangohud: perf_config.hud.enabled,
            overlay_open: false,
            input_profile: game
                .settings
                .input_profile
                .clone()
                .unwrap_or_else(|| config.input.default_profile.clone()),
            log_path: session.log_path.clone(),
        };

        if let Ok(mut guard) = self.session.lock() {
            *guard = Some(SessionState {
                info: info.clone(),
                session: Arc::new(Mutex::new(session)),
                game_name: game.name.clone(),
            });
        }

        if config.input.grab_devices || game.settings.input_profile.is_some() {
            self.start_input(&info.input_profile);
        }
        let _ = self.spawn_overlay();

        let shared = Arc::clone(self);
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_millis(500));
                let finished = {
                    let guard = shared.session.lock().ok();
                    match guard.as_ref().and_then(|g| g.as_ref()) {
                        Some(state) => match state.session.lock() {
                            Ok(mut session) => matches!(session.try_wait(), Ok(Some(_))),
                            Err(_) => true,
                        },
                        None => true,
                    }
                };
                if finished {
                    break;
                }
            }
            shared.finish_session();
        });

        Ok(Envelope::ok(&serde_json::json!({
            "note": note,
            "session": info,
            "layers": summary,
        })))
    }

    pub fn stop_session(&self) -> Result<Envelope> {
        let state = self
            .session
            .lock()
            .ok()
            .and_then(|mut guard| guard.take());
        let Some(state) = state else {
            return Ok(Envelope::ok(&serde_json::json!({ "message": "aucune session" })));
        };
        if let Ok(mut session) = state.session.lock() {
            session.terminate();
        }
        self.stop_input();
        self.kill_overlay();
        self.record_playtime(&state.info);
        Ok(Envelope::ok(&serde_json::json!({ "message": "session arrêtée" })))
    }

    fn finish_session(self: &Arc<Self>) {
        let state = self.session.lock().ok().and_then(|mut guard| guard.take());
        if let Some(state) = state {
            self.stop_input();
            self.kill_overlay();
            self.record_playtime(&state.info);
        }
    }

    fn record_playtime(&self, info: &SessionInfo) {
        let played = (ludus_core::util::now_unix() - info.started).max(0) as u64;
        if let Ok(Some(mut game)) = self.store.game(info.appid) {
            game.playtime_secs += played;
            game.last_played = Some(ludus_core::util::now_unix());
            let _ = self.store.save_game(&game);
        }
        if let Ok(mut stats) = self.store.get_json::<ludus_core::model::Stats>("stats") {
            let mut stats = stats.take().unwrap_or_default();
            stats.launches += 1;
            stats.total_seconds += played;
            let _ = self.store.set_json("stats", &stats);
        }
    }

    pub fn spawn_overlay(self: &Arc<Self>) -> Result<()> {
        let Some(info) = self.session_info() else {
            return Ok(());
        };
        if let Ok(mut guard) = self.overlay.lock() {
            if let Some(child) = guard.as_mut() {
                if matches!(child.try_wait(), Ok(None)) {
                    return Ok(());
                }
            }
        }
        let exe = std::env::current_exe()
            .map_err(|e| ludus_core::Error::Other(format!("binaire courant : {e}")))?;
        let config = self.config_snapshot();
        let mut command = Command::new(exe);
        command
            .arg("overlay")
            .arg("--game")
            .arg(&info.name)
            .arg("--fps")
            .arg(info.fps_cap.to_string())
            .arg("--profile")
            .arg(&info.input_profile)
            .arg("--proton")
            .arg(&info.proton)
            .arg("--width")
            .arg(config.launch.width.to_string())
            .arg("--height")
            .arg(config.launch.height.to_string());
        if info.mangohud {
            command.arg("--mangohud");
        }
        if ludus_core::paths::which(&config.overlay.recorder).is_some() {
            command.arg("--recorder");
        }
        let log_path = self
            .paths
            .log_dir
            .join(format!("overlay-{}.log", info.appid));
        if let Some(parent) = log_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .map_err(ludus_core::Error::Io)?;
        let log_err = log.try_clone().map_err(ludus_core::Error::Io)?;
        let child = command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::from(log))
            .stderr(std::process::Stdio::from(log_err))
            .spawn()
            .map_err(|e| ludus_core::Error::Other(format!("overlay : {e}")))?;
        if let Ok(mut guard) = self.overlay.lock() {
            *guard = Some(child);
        }
        if let Ok(mut guard) = self.session.lock() {
            if let Some(state) = guard.as_mut() {
                state.info.overlay_open = true;
            }
        }
        Ok(())
    }

    fn kill_overlay(&self) {
        if let Ok(mut guard) = self.overlay.lock() {
            if let Some(mut child) = guard.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        if let Ok(mut guard) = self.session.lock() {
            if let Some(state) = guard.as_mut() {
                state.info.overlay_open = false;
            }
        }
    }

    pub fn toggle_overlay(self: &Arc<Self>) -> Result<Envelope> {
        let running = {
            let mut guard = self.overlay.lock().map_err(|_| {
                ludus_core::Error::Other("verrou overlay indisponible".into())
            })?;
            match guard.as_mut() {
                Some(child) => match child.try_wait() {
                    Ok(None) => true,
                    _ => {
                        *guard = None;
                        false
                    }
                },
                None => false,
            }
        };
        if running {
            self.kill_overlay();
            Ok(Envelope::ok(&serde_json::json!({ "overlay": "fermé" })))
        } else {
            self.spawn_overlay()?;
            Ok(Envelope::ok(&serde_json::json!({ "overlay": "ouvert" })))
        }
    }

    fn start_input(self: &Arc<Self>, profile_name: &str) {
        let config = self.config_snapshot();
        let mut profile = ludus_input::load_profile(&self.paths.data_dir, profile_name)
            .unwrap_or_default();
        profile.deadzone = config.input.deadzone;
        profile.curve = config.input.stick_curve;
        profile.hotkey_chord = split_chord(&config.overlay.pad_hotkey);
        profile.keyboard_hotkey = split_chord(&config.overlay.hotkey);
        let Ok(mut service) = ludus_input::InputService::start(profile, config.input.grab_devices)
        else {
            return;
        };
        // Le service vit avec la session : le thread s'arrête quand elle se termine
        // et libère immédiatement le grab des manettes.
        let shared = Arc::clone(self);
        std::thread::spawn(move || {
            loop {
                match service.events.recv_timeout(Duration::from_millis(200)) {
                    Ok(ludus_input::InputMsg::Hotkey(_)) => {
                        let _ = shared.toggle_overlay();
                    }
                    Ok(_) => {}
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        if shared.session_info().is_none() {
                            break;
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
            service.stop();
        });
    }

    fn stop_input(&self) {}

    pub fn overlay_action(self: &Arc<Self>, action: &str) -> Result<Envelope> {
        let mut message = String::new();
        if action.starts_with("fps_cap:") {
            if let Ok(value) = action.trim_start_matches("fps_cap:").parse::<u32>() {
                if let Ok(mut guard) = self.session.lock() {
                    if let Some(state) = guard.as_mut() {
                        state.info.fps_cap = value;
                        let _ = mangohud::write_config(
                            &self.paths.config_dir,
                            state.info.appid,
                            &mangohud::HudSettings {
                                enabled: state.info.mangohud,
                                fps_limit: Some(value),
                                ..Default::default()
                            },
                        );
                    }
                }
                message = format!("limite FPS {value} appliquée au prochain lancement");
            }
        } else if action == "mangohud_toggle" {
            let mut enabled = false;
            if let Ok(mut guard) = self.session.lock() {
                if let Some(state) = guard.as_mut() {
                    state.info.mangohud = !state.info.mangohud;
                    enabled = state.info.mangohud;
                    let _ = mangohud::write_config(
                        &self.paths.config_dir,
                        state.info.appid,
                        &mangohud::HudSettings {
                            enabled,
                            fps_limit: Some(state.info.fps_cap),
                            ..Default::default()
                        },
                    );
                }
            }
            message = format!("MangoHud {}", if enabled { "activé" } else { "désactivé" });
        } else if action == "profile_next" {
            let profiles = ludus_input::list_profiles(&self.paths.data_dir);
            if !profiles.is_empty() {
                let current = self.session_info().map(|s| s.input_profile).unwrap_or_default();
                let index = profiles.iter().position(|p| p.name == current).unwrap_or(0);
                let next = profiles[(index + 1) % profiles.len()].name.clone();
                if let Ok(mut guard) = self.session.lock() {
                    if let Some(state) = guard.as_mut() {
                        state.info.input_profile = next.clone();
                    }
                }
                self.start_input(&next);
                message = format!("profil manettes : {next}");
            }
        } else if action == "proton_next" {
            let protons = self.protons_cached();
            if !protons.is_empty() {
                let current = self.session_info().map(|s| s.proton).unwrap_or_default();
                let index = protons.iter().position(|p| p.name == current).unwrap_or(0);
                let next = protons[(index + 1) % protons.len()].name.clone();
                if let Ok(mut guard) = self.session.lock() {
                    if let Some(state) = guard.as_mut() {
                        state.info.proton = next.clone();
                    }
                }
                message = format!("Proton {next} appliqué au prochain lancement");
            }
        } else if action == "quit_game" {
            self.stop_session()?;
            message = "jeu arrêté".into();
        } else if action == "screenshot" {
            message = take_screenshot(&self.paths.data_dir);
        } else if action == "record_toggle" {
            message = self.toggle_recording();
        } else {
            message = "action ignorée".into();
        }
        Ok(Envelope::ok(&serde_json::json!({ "message": message })))
    }

    fn toggle_recording(&self) -> String {
        let config = self.config_snapshot();
        if let Ok(mut guard) = self.recorder.lock() {
            if let Some(mut child) = guard.take() {
                let _ = child.kill();
                let _ = child.wait();
                return "enregistrement arrêté".into();
            }
        }
        let Some(recorder) = ludus_core::paths::which(&config.overlay.recorder) else {
            return "gpu-screen-recorder absent".into();
        };
        let dir = ludus_core::paths::home_dir().join("Videos");
        std::fs::create_dir_all(&dir).ok();
        let path = dir.join(format!(
            "ludus-{}.mp4",
            ludus_core::util::now_unix()
        ));
        let child = Command::new(recorder)
            .arg("-w")
            .arg("screen")
            .arg("-f")
            .arg("60")
            .arg("-o")
            .arg(&path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        match child {
            Ok(child) => {
                if let Ok(mut guard) = self.recorder.lock() {
                    *guard = Some(child);
                }
                format!("enregistrement vers {}", path.display())
            }
            Err(err) => format!("échec enregistrement : {err}"),
        }
    }
}

fn split_chord(text: &str) -> Vec<String> {
    text.split('+')
        .map(|part| part.trim().to_string())
        .filter(|part| !part.is_empty())
        .collect()
}

fn take_screenshot(data_dir: &std::path::Path) -> String {
    let dir = ludus_core::paths::home_dir().join("Images");
    std::fs::create_dir_all(&dir).ok();
    let path = dir.join(format!("ludus-{}.png", ludus_core::util::now_unix()));
    if let Some(spectacle) = ludus_core::paths::which("spectacle") {
        let status = Command::new(spectacle)
            .arg("-b")
            .arg("-n")
            .arg("-o")
            .arg(&path)
            .status();
        if matches!(status, Ok(s) if s.success()) {
            return format!("capture : {}", path.display());
        }
    }
    if let Some(grim) = ludus_core::paths::which("grim") {
        let status = Command::new(grim).arg(&path).status();
        if matches!(status, Ok(s) if s.success()) {
            return format!("capture : {}", path.display());
        }
    }
    let _ = data_dir;
    "aucun outil de capture disponible (installez spectacle ou grim)".into()
}

fn perf_config(config: &ludus_core::Config, game: &Game) -> PerfConfig {
    let settings = &game.settings;
    let mangohud_enabled = settings.mangohud.unwrap_or(config.launch.mangohud);
    let gamescope = ludus_perf::GamescopeSettings {
        enabled: settings.gamescope.unwrap_or(config.launch.gamescope),
        width: settings.width.unwrap_or(config.launch.width),
        height: settings.height.unwrap_or(config.launch.height),
        fps_cap: settings.fps_cap.unwrap_or(config.launch.fps_cap),
        hdr: settings.hdr.unwrap_or(config.launch.hdr),
        adaptive_sync: settings.adaptive_sync.unwrap_or(config.launch.adaptive_sync),
        fullscreen: config.launch.fullscreen,
        mangoapp: mangohud_enabled,
    };
    PerfConfig {
        gamescope: gamescope.clone(),
        hud: ludus_perf::HudSettings {
            enabled: mangohud_enabled,
            fps_limit: Some(gamescope.fps_cap),
            ..Default::default()
        },
        gamemode: config.launch.gamemode,
    }
}

/// Profil par défaut du jeu (utilisé par la CLI).
pub fn default_profile(paths: &ludus_core::LudusPaths, name: &str) -> Profile {
    ludus_input::load_profile(&paths.data_dir, name).unwrap_or_default()
}

pub fn overlay_log_path(paths: &ludus_core::LudusPaths, appid: u32) -> PathBuf {
    paths.log_dir.join(format!("overlay-{appid}.log"))
}
