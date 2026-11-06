//! Mode jeu léger : GameScope, MangoHud et supervision de session.

pub mod caps;
pub mod gamescope;
pub mod mangohud;
pub mod session;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("E/S : {0}")]
    Io(#[from] std::io::Error),
    #[error("core : {0}")]
    Core(#[from] lapinux_core::Error),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;

pub use caps::PerfCaps;
pub use gamescope::GamescopeSettings;
pub use mangohud::HudSettings;
pub use session::{
    layers_summary, prepare, render_prepared, PerfConfig, PreparedLaunch, Session, SessionOutcome,
};

#[cfg(test)]
mod tests {
    use super::*;
    use lapinux_proton::LaunchPlan;
    use std::path::{Path, PathBuf};

    fn plan() -> LaunchPlan {
        LaunchPlan {
            program: PathBuf::from("/usr/bin/umu-run"),
            args: vec!["/jeux/TF2/hl2.exe".into()],
            env: vec![("GAMEID".into(), "lapinux-440".into())],
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
            mangohud: Some(bin("mangohud")),
            power_profiles: None,
        }
    }

    #[test]
    fn gamescope_keeps_mangohud_in_the_game() {
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
            ..PerfConfig::default()
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
        let argv: Vec<String> = prepared
            .command
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert_eq!(
            prepared.command.get_program(),
            caps.gamescope.as_ref().unwrap().as_os_str()
        );
        // `--mangoapp` n'affiche rien sans le processus lancé par Steam : le
        // HUD doit vivre dans le jeu, où il relit sa configuration.
        assert!(!argv.contains(&"--mangoapp".to_string()));
        assert!(argv.contains(&"/usr/bin/umu-run".to_string()));
        assert!(argv.contains(&"/jeux/TF2/hl2.exe".to_string()));
        let envs: Vec<String> = prepared
            .command
            .get_envs()
            .filter_map(|(k, v)| {
                v.map(|v| format!("{}={}", k.to_string_lossy(), v.to_string_lossy()))
            })
            .collect();
        assert!(envs.contains(&"GAMEID=lapinux-440".to_string()));
        assert!(envs.contains(&"MANGOHUD=1".to_string()));
        assert!(dir.path().join("MangoHud/440.conf").is_file());
    }

    #[test]
    fn gamescope_hud_reads_the_selected_profile() {
        let dir = tempfile::tempdir().unwrap();
        let caps = fake_caps(dir.path());
        let config = PerfConfig {
            gamescope: GamescopeSettings {
                mangoapp: true,
                ..Default::default()
            },
            hud: mangohud::preset_settings_at("simple", Some(60), "top-right"),
            ..PerfConfig::default()
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
        assert!(prepared.gamescope && prepared.mangohud);
        let config_file = dir.path().join("MangoHud/440.conf");
        let text = std::fs::read_to_string(&config_file).unwrap();
        assert!(text.contains("position=top-right"));
        // Le modèle du GPU peut être écrit en libellé (`gpu_text`, que MangoApp
        // affiche) ou en ligne dédiée (`gpu_name`) : au moins l'un des deux.
        assert!(
            text.contains("gpu_text=") || text.contains("gpu_name"),
            "conf : {text}"
        );
        assert!(text.contains("gpu_temp"));
        assert!(text.contains("cpu_temp"));
        assert!(text.contains("cpu_text="));
        let envs: Vec<(String, String)> = prepared
            .command
            .get_envs()
            .filter_map(|(k, v)| {
                v.map(|v| {
                    (
                        k.to_string_lossy().to_string(),
                        v.to_string_lossy().to_string(),
                    )
                })
            })
            .collect();
        assert!(envs.contains(&(
            "MANGOHUD_CONFIGFILE".to_string(),
            config_file.to_string_lossy().to_string()
        )));
        // La couche MangoHud est chargée dans le jeu : c'est elle qui dessine
        // le HUD, GameScope ne fait que composer l'image.
        assert!(envs.iter().any(|(key, _)| key == "MANGOHUD"));
    }

    #[test]
    fn hidden_hud_is_still_loaded_for_the_overlay_switch() {
        let dir = tempfile::tempdir().unwrap();
        let caps = fake_caps(dir.path());
        let config = PerfConfig {
            gamescope: GamescopeSettings::default(),
            hud: HudSettings {
                enabled: false,
                ..Default::default()
            },
            ..PerfConfig::default()
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
        // Le HUD est masqué mais présent : l'interrupteur de l'overlay peut
        // l'allumer sans relancer le jeu.
        assert!(prepared.mangohud);
        let text = std::fs::read_to_string(dir.path().join("MangoHud/440.conf")).unwrap();
        assert!(text.contains("no_display=1"));
        let envs: Vec<String> = prepared
            .command
            .get_envs()
            .filter_map(|(k, v)| {
                v.map(|v| format!("{}={}", k.to_string_lossy(), v.to_string_lossy()))
            })
            .collect();
        assert!(envs.contains(&"MANGOHUD=1".to_string()));
    }

    #[test]
    fn performance_mode_does_not_inject_mangohud() {
        let dir = tempfile::tempdir().unwrap();
        let caps = fake_caps(dir.path());
        let config = PerfConfig {
            inject_mangohud: false,
            hud: HudSettings {
                enabled: true,
                ..Default::default()
            },
            ..PerfConfig::default()
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
        // Le mode performance ne charge même pas la couche masquée : aucune
        // variable MangoHud ne doit descendre jusqu'au jeu.
        assert!(!prepared.mangohud);
        let envs: Vec<String> = prepared
            .command
            .get_envs()
            .filter_map(|(k, v)| {
                v.map(|v| format!("{}={}", k.to_string_lossy(), v.to_string_lossy()))
            })
            .collect();
        assert!(!envs.iter().any(|env| env.starts_with("MANGOHUD")));
        assert!(!dir.path().join("MangoHud/440.conf").exists());
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
            ..PerfConfig::default()
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
            scene: false,
            scene_exit_path: None,
            stats_path: None,
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
            scene: false,
            scene_exit_path: None,
            stats_path: None,
        };
        let mut session = Session::spawn(1, prepared).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));
        session.terminate();
        let outcome = session.wait().unwrap();
        assert!(outcome.code != Some(0));
    }

    #[test]
    fn summary_lists_layers() {
        assert_eq!(layers_summary(true, true), "GameScope + MangoHud");
        assert_eq!(layers_summary(false, false), "lancement direct");
    }
}
