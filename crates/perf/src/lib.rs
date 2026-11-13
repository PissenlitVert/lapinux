//! Mode jeu léger : GameScope, labels matériels et supervision de session.

pub mod caps;
pub mod gamescope;
pub mod gamescope_check;
pub mod labels;
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
pub use gamescope_check::{GamescopeProbe, GamescopeVersion};
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

    /// Faux GameScope : il répond à `--version` et `--help` comme le vrai,
    /// sinon le contrôle de version écarterait le compositeur dans les tests
    /// qui vérifient justement la composition de la commande.
    fn fake_caps(dir: &Path) -> PerfCaps {
        fake_caps_version(dir, "3.17.0", &gamescope_check::REQUIRED.join(" "))
    }

    fn fake_caps_version(dir: &Path, version: &str, help: &str) -> PerfCaps {
        let path = dir.join("gamescope");
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\ncase \"$1\" in\n--version) echo \"gamescope version {version}\" ;;\n\
                 --help) echo \"{help}\" ;;\nesac\n"
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        PerfCaps {
            gamescope: Some(path),
            power_profiles: None,
        }
    }

    #[test]
    fn gamescope_wraps_the_game_without_a_hud_layer() {
        let dir = tempfile::tempdir().unwrap();
        let caps = fake_caps(dir.path());
        let config = PerfConfig::default();
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
    }

    #[test]
    fn native_preload_is_applied_to_game_not_scene_or_gamescope() {
        let dir = tempfile::tempdir().unwrap();
        let caps = fake_caps(dir.path());
        let config = PerfConfig {
            game_env: vec![("LD_PRELOAD".into(), Some("/pont.so:/existing.so".into()))],
            scene: true,
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
        let args: Vec<_> = prepared
            .command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        let scene = args.iter().position(|arg| arg == "scene").unwrap();
        let preload = args
            .iter()
            .position(|arg| arg == "LD_PRELOAD=/pont.so:/existing.so")
            .unwrap();
        let game = args
            .iter()
            .position(|arg| arg == "/usr/bin/umu-run")
            .unwrap();
        assert!(scene < preload && preload < game);
        assert!(prepared
            .command
            .get_envs()
            .any(|(key, value)| key == "LD_PRELOAD" && value.is_none()));
    }

    #[test]
    fn game_only_graphics_environment_is_applied_after_scene_and_gamescope() {
        let dir = tempfile::tempdir().unwrap();
        let caps = fake_caps(dir.path());
        let config = PerfConfig {
            game_env: vec![
                ("DISABLE_LSFGVK".into(), None),
                ("LSFGVK_ENV".into(), Some("1".into())),
                ("LSFGVK_MULTIPLIER".into(), Some("2".into())),
            ],
            scene: true,
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
        let args: Vec<_> = prepared
            .command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let scene = args.iter().position(|a| a == "scene").unwrap();
        let var = args.iter().position(|a| a == "LSFGVK_ENV=1").unwrap();
        let game = args.iter().position(|a| a == "/usr/bin/umu-run").unwrap();
        assert!(scene < var && var < game);
        assert!(args.windows(2).any(|a| a == ["-u", "DISABLE_LSFGVK"]));
        assert!(prepared
            .command
            .get_envs()
            .any(|(k, v)| k == "DISABLE_LSFGVK" && v == Some(std::ffi::OsStr::new("1"))));
        assert!(!prepared.command.get_envs().any(|(k, _)| k == "LSFGVK_ENV"));
    }

    #[test]
    fn game_only_effects_are_not_enabled_in_gamescope() {
        let dir = tempfile::tempdir().unwrap();
        let caps = fake_caps(dir.path());
        let config = PerfConfig {
            game_env: vec![
                ("NVPRESENT_ENABLE_SMOOTH_MOTION".into(), Some("1".into())),
                ("ENABLE_VKBASALT".into(), Some("1".into())),
                ("DISABLE_LSFGVK".into(), Some("1".into())),
            ],
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
        let args: Vec<_> = prepared
            .command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let separator = args.iter().position(|a| a == "--").unwrap();
        let effect = args.iter().position(|a| a == "ENABLE_VKBASALT=1").unwrap();
        let game = args.iter().position(|a| a == "/usr/bin/umu-run").unwrap();
        assert!(separator < effect && effect < game);
        assert!(prepared
            .command
            .get_envs()
            .any(|(k, v)| k == "ENABLE_VKBASALT" && v.is_none()));
        assert!(prepared
            .command
            .get_envs()
            .any(|(k, v)| k == "NVPRESENT_ENABLE_SMOOTH_MOTION" && v.is_none()));
    }

    #[test]
    fn afmf_is_limited_to_game_with_or_without_gamescope_and_scene() {
        let dir = tempfile::tempdir().unwrap();
        let caps = fake_caps(dir.path());
        for native in [false, true] {
            for (scope, scene) in [(false, false), (true, false), (true, true)] {
                let mut launch = plan();
                if native {
                    launch.program = "/games/native".into();
                }
                launch.env.push(("AFMF_ENABLE".into(), "manual".into()));
                launch.env.push(("DISABLE_LSFGVK".into(), "0".into()));
                let mut config = PerfConfig::default();
                config.gamescope.enabled = scope;
                config.scene = scene;
                config.game_env = vec![
                    ("AFMF_ENABLE".into(), Some("1".into())),
                    ("AFMF_PERFORMANCE_MODE".into(), Some("quality".into())),
                    ("AFMF_SEARCH_MODE".into(), Some("high".into())),
                    ("DISABLE_AFMF".into(), None),
                    ("DISABLE_LSFGVK".into(), Some("1".into())),
                    (
                        "VK_ADD_IMPLICIT_LAYER_PATH".into(),
                        Some("/afmf:/existing".into()),
                    ),
                ];
                let prepared = prepare(
                    &launch,
                    440,
                    &config,
                    &caps,
                    dir.path(),
                    &dir.path().join("logs"),
                )
                .unwrap();
                let args: Vec<_> = prepared
                    .command
                    .get_args()
                    .map(|a| a.to_string_lossy().into_owned())
                    .collect();
                let game = args
                    .iter()
                    .position(|a| a == &launch.program.to_string_lossy())
                    .unwrap();
                let afmf = args.iter().position(|a| a == "AFMF_ENABLE=1").unwrap();
                assert!(afmf < game);
                if scene {
                    assert!(args.iter().position(|a| a == "scene").unwrap() < afmf);
                }
                assert!(args.contains(&"VK_ADD_IMPLICIT_LAYER_PATH=/afmf:/existing".to_string()));
                assert!(args.contains(&"AFMF_PERFORMANCE_MODE=quality".to_string()));
                assert!(args.contains(&"AFMF_SEARCH_MODE=high".to_string()));
                let envs: Vec<_> = prepared.command.get_envs().collect();
                for key in ["AFMF_PERFORMANCE_MODE", "AFMF_SEARCH_MODE"] {
                    assert!(envs.iter().any(|(k, v)| *k == key && v.is_none()));
                }
                assert!(envs.iter().any(|(k, v)| *k == "AFMF_ENABLE" && v.is_none()));
                assert!(envs
                    .iter()
                    .any(|(k, v)| *k == "DISABLE_AFMF" && *v == Some(std::ffi::OsStr::new("1"))));
                assert!(args.windows(2).any(|pair| pair == ["-u", "DISABLE_AFMF"]));
                assert!(envs
                    .iter()
                    .any(|(k, v)| *k == "DISABLE_LSFGVK" && *v == Some(std::ffi::OsStr::new("1"))));
            }
        }
    }

    #[test]
    fn competing_frame_generators_are_disabled_in_game_and_parent_processes() {
        let dir = tempfile::tempdir().unwrap();
        let caps = fake_caps(dir.path());
        for (scope, scene) in [(false, false), (true, false), (true, true)] {
            let mut launch = plan();
            for key in ["AFMF_ENABLE", "NVPRESENT_ENABLE_SMOOTH_MOTION", "PROTON_USE_OPTISCALER"] {
                launch.env.push((key.into(), "1".into()));
            }
            launch.env.push(("DISABLE_LSFGVK".into(), "0".into()));
            let mut config = PerfConfig::default();
            config.gamescope.enabled = scope;
            config.scene = scene;
            config.game_env = vec![
                ("DISABLE_AFMF".into(), Some("1".into())),
                ("AFMF_ENABLE".into(), None),
                ("DISABLE_LSFGVK".into(), Some("1".into())),
                ("NVPRESENT_ENABLE_SMOOTH_MOTION".into(), None),
                ("PROTON_USE_OPTISCALER".into(), None),
            ];
            let prepared = prepare(&launch, 440, &config, &caps, dir.path(), &dir.path().join("logs")).unwrap();
            let args: Vec<_> = prepared.command.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
            for key in ["AFMF_ENABLE", "NVPRESENT_ENABLE_SMOOTH_MOTION", "PROTON_USE_OPTISCALER"] {
                assert!(args.windows(2).any(|pair| pair == ["-u", key]));
                assert!(prepared.command.get_envs().any(|(k, v)| k == key && v.is_none()));
            }
            assert!(args.contains(&"DISABLE_AFMF=1".to_string()));
            assert!(args.contains(&"DISABLE_LSFGVK=1".to_string()));
            for key in ["DISABLE_AFMF", "DISABLE_LSFGVK"] {
                assert!(prepared.command.get_envs().any(|(k, v)| k == key && v == Some(std::ffi::OsStr::new("1"))));
            }
        }
    }

    #[test]
    fn injector_overrides_are_confined_to_game_with_gamescope_and_scene() {
        let dir = tempfile::tempdir().unwrap();
        let caps = fake_caps(dir.path());
        for (scope, scene) in [(false, false), (true, false), (true, true)] {
            let mut config = PerfConfig::default();
            config.gamescope.enabled = scope;
            config.scene = scene;
            config
                .game_env
                .push(("WINEDLLOVERRIDES".into(), Some("foo=n,b;dxgi=n,b".into())));
            let prepared = prepare(
                &plan(),
                440,
                &config,
                &caps,
                dir.path(),
                &dir.path().join("logs"),
            )
            .unwrap();
            let args: Vec<_> = prepared
                .command
                .get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            let override_pos = args
                .iter()
                .position(|a| a == "WINEDLLOVERRIDES=foo=n,b;dxgi=n,b")
                .unwrap();
            assert!(override_pos < args.iter().position(|a| a == "/usr/bin/umu-run").unwrap());
            if scene {
                assert!(args.iter().position(|a| a == "scene").unwrap() < override_pos);
            }
            assert!(prepared
                .command
                .get_envs()
                .any(|(k, v)| k == "WINEDLLOVERRIDES" && v.is_none()));
        }
    }

    #[test]
    fn proton_compatibility_is_applied_to_game_not_gamescope_or_scene() {
        let dir = tempfile::tempdir().unwrap();
        let caps = fake_caps(dir.path());
        for (scope, scene) in [(false, false), (true, false), (true, true)] {
            let mut config = PerfConfig::default();
            config.gamescope.enabled = scope;
            config.scene = scene;
            config.game_env = vec![
                ("PROTON_USE_WINED3D".into(), Some("1".into())),
                ("PROTON_DXVK_D3D8".into(), Some("0".into())),
                (
                    "DXVK_CONFIG".into(),
                    Some("d3d9.modeCountCompatibility = True".into()),
                ),
                ("VKD3D_CONFIG".into(), Some("foo,nodxr".into())),
            ];
            let prepared = prepare(
                &plan(),
                440,
                &config,
                &caps,
                dir.path(),
                &dir.path().join("logs"),
            )
            .unwrap();
            let args: Vec<_> = prepared
                .command
                .get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            let game = args.iter().position(|a| a == "/usr/bin/umu-run").unwrap();
            for key in [
                "PROTON_USE_WINED3D",
                "PROTON_DXVK_D3D8",
                "DXVK_CONFIG",
                "VKD3D_CONFIG",
            ] {
                let position = args
                    .iter()
                    .position(|a| a.starts_with(&format!("{key}=")))
                    .unwrap();
                assert!(position < game);
                if scene {
                    assert!(args.iter().position(|a| a == "scene").unwrap() < position);
                }
                assert!(prepared
                    .command
                    .get_envs()
                    .any(|(k, v)| k == key && v.is_none()));
            }
        }
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
            scene: false,
            scene_exit_path: None,
            scene_start_path: None,
            stats_path: None,
            gamescope_warning: None,
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
            scene: false,
            scene_exit_path: None,
            scene_start_path: None,
            stats_path: None,
            gamescope_warning: None,
        };
        let mut session = Session::spawn(1, prepared).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));
        session.terminate();
        let outcome = session.wait().unwrap();
        assert!(outcome.code != Some(0));
    }

    #[test]
    fn un_gamescope_trop_ancien_retombe_sur_le_lancement_direct() {
        let dir = tempfile::tempdir().unwrap();
        let caps = fake_caps_version(dir.path(), "3.14.2", &gamescope_check::REQUIRED.join(" "));
        let prepared = prepare(
            &plan(),
            440,
            &PerfConfig::default(),
            &caps,
            dir.path(),
            &dir.path().join("logs"),
        )
        .unwrap();
        assert!(!prepared.gamescope);
        assert_eq!(prepared.command.get_program(), "/usr/bin/umu-run");
        let warning = prepared.gamescope_warning.expect("motif");
        assert!(warning.contains("3.14.2"), "{warning}");
        assert!(warning.contains("lancement direct"), "{warning}");
    }

    #[test]
    fn une_option_inconnue_ecarte_gamescope_sans_casser_le_lancement() {
        let dir = tempfile::tempdir().unwrap();
        // `--hdr-enabled` absent : GameScope sortirait sur cet argument.
        let caps = fake_caps_version(dir.path(), "3.17.0", "--framerate-limit --adaptive-sync");
        let prepared = prepare(
            &plan(),
            440,
            &PerfConfig::default(),
            &caps,
            dir.path(),
            &dir.path().join("logs"),
        )
        .unwrap();
        assert!(!prepared.gamescope);
        let warning = prepared.gamescope_warning.expect("motif");
        assert!(warning.contains("--hdr-enabled"), "{warning}");
    }

    #[test]
    fn un_gamescope_absent_est_signale() {
        let dir = tempfile::tempdir().unwrap();
        let caps = PerfCaps {
            gamescope: None,
            power_profiles: None,
        };
        let prepared = prepare(
            &plan(),
            440,
            &PerfConfig::default(),
            &caps,
            dir.path(),
            &dir.path().join("logs"),
        )
        .unwrap();
        assert!(!prepared.gamescope);
        let warning = prepared.gamescope_warning.expect("motif");
        assert!(warning.contains("introuvable"), "{warning}");
    }

    #[test]
    fn summary_lists_layers() {
        assert_eq!(layers_summary(true), "GameScope");
        assert_eq!(layers_summary(false), "lancement direct");
    }
}
