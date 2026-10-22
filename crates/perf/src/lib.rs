//! Mode jeu léger : gamescope, MangoHud, GameMode et supervision de session.

pub mod gamemode;
pub mod gamescope;
pub mod mangohud;
pub mod session;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("E/S : {0}")]
    Io(#[from] std::io::Error),
    #[error("core : {0}")]
    Core(#[from] ludus_core::Error),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;

pub use gamemode::{wrap, PerfCaps, Wrapped};
pub use gamescope::GamescopeSettings;
pub use mangohud::HudSettings;
pub use session::{
    layers_summary, prepare, render_prepared, PerfConfig, PreparedLaunch, Session, SessionOutcome,
};

#[cfg(test)]
mod tests {
    use super::*;
    use ludus_proton::LaunchPlan;
    use std::path::{Path, PathBuf};

    fn plan() -> LaunchPlan {
        LaunchPlan {
            program: PathBuf::from("/usr/bin/umu-run"),
            args: vec!["/jeux/TF2/hl2.exe".into()],
            env: vec![("GAMEID".into(), "ludus-440".into())],
            cwd: None,
        }
    }

    fn fake_caps(dir: &Path) -> PerfCaps {
        let bin = |name: &str| {
            let path = dir.join(name);
            std::fs::write(&path, "#!/bin/sh\n").unwrap();
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        };
        PerfCaps {
            gamescope: Some(bin("gamescope")),
            gamemoderun: Some(bin("gamemoderun")),
            mangohud: Some(bin("mangohud")),
            recorder: None,
            power_profiles: None,
        }
    }

    #[test]
    fn gamescope_is_wrapped_by_gamemode_and_uses_mangoapp() {
        let dir = tempfile::tempdir().unwrap();
        let caps = fake_caps(dir.path());
        let config = PerfConfig {
            gamescope: GamescopeSettings {
                mangoapp: true,
                ..Default::default()
            },
            hud: HudSettings {
                enabled: true,
                ..Default::default()
            },
            gamemode: true,
        };
        let prepared = prepare(
            &plan(),
            440,
            &config,
            &caps,
            dir.path(),
            &dir.path().join("logs"),
        )
        .unwrap();
        assert!(prepared.gamescope);
        assert!(prepared.gamemode);
        let argv: Vec<String> = prepared
            .command
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert_eq!(argv[0], caps.gamescope.as_ref().unwrap().to_string_lossy());
        assert!(argv.contains(&"--mangoapp".to_string()));
        assert!(argv.contains(&"/usr/bin/umu-run".to_string()));
        assert!(argv.contains(&"/jeux/TF2/hl2.exe".to_string()));
        let envs: Vec<String> = prepared
            .command
            .get_envs()
            .filter_map(|(k, v)| {
                v.map(|v| format!("{}={}", k.to_string_lossy(), v.to_string_lossy()))
            })
            .collect();
        assert!(envs.contains(&"GAMEID=ludus-440".to_string()));
        assert!(!envs.iter().any(|e| e.starts_with("MANGOHUD=")));
    }

    #[test]
    fn without_gamescope_mangohud_env_is_set() {
        let dir = tempfile::tempdir().unwrap();
        let mut caps = fake_caps(dir.path());
        caps.gamescope = None;
        let config = PerfConfig {
            gamescope: GamescopeSettings {
                enabled: true,
                ..Default::default()
            },
            hud: HudSettings {
                enabled: true,
                ..Default::default()
            },
            gamemode: false,
        };
        let prepared = prepare(
            &plan(),
            440,
            &config,
            &caps,
            dir.path(),
            &dir.path().join("logs"),
        )
        .unwrap();
        assert!(!prepared.gamescope);
        assert!(prepared.mangohud);
        assert!(!prepared.gamemode);
        let argv: Vec<String> = prepared
            .command
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert_eq!(argv, vec!["/jeux/TF2/hl2.exe"]);
        let config_file = dir.path().join("MangoHud/440.conf");
        assert!(config_file.is_file());
        let envs: Vec<String> = prepared
            .command
            .get_envs()
            .filter_map(|(k, v)| {
                v.map(|v| format!("{}={}", k.to_string_lossy(), v.to_string_lossy()))
            })
            .collect();
        assert!(envs.iter().any(|e| e.starts_with("MANGOHUD_CONFIGFILE=")));
    }

    #[test]
    fn session_runs_and_reports_exit_code() {
        let dir = tempfile::tempdir().unwrap();
        let mut command = std::process::Command::new("/bin/sh");
        command.arg("-c").arg("exit 3");
        let prepared = PreparedLaunch {
            command,
            log_path: dir.path().join("session.log"),
            gamescope: false,
            mangohud: false,
            gamemode: false,
        };
        let mut session = Session::spawn(1, prepared).unwrap();
        let outcome = session.wait().unwrap();
        assert_eq!(outcome.code, Some(3));
        assert!(dir.path().join("session.log").is_file());
    }

    #[test]
    fn session_terminate_kills_process_group() {
        let dir = tempfile::tempdir().unwrap();
        let mut command = std::process::Command::new("/bin/sh");
        command.arg("-c").arg("sleep 30");
        let prepared = PreparedLaunch {
            command,
            log_path: dir.path().join("session.log"),
            gamescope: false,
            mangohud: false,
            gamemode: false,
        };
        let mut session = Session::spawn(1, prepared).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));
        session.terminate();
        let outcome = session.wait().unwrap();
        assert!(outcome.code != Some(0));
    }

    #[test]
    fn summary_lists_layers() {
        assert_eq!(layers_summary(true, true, true), "gamescope + MangoHud + GameMode");
        assert_eq!(layers_summary(false, false, false), "lancement direct");
    }
}
