//! Native bridge integration: build first with `cargo build -p lapinux-steamshim`,
//! then run `cargo test -p lapinux-daemon --test native_steamshim`.
//! The daemon Shared launch path is exercised in a subprocess with an isolated
//! HOME and D-Bus, so the real desktop and Steam account are never contacted.

#![cfg(all(target_os = "linux", target_arch = "x86_64"))]

use lapinux_core::model::{Game, GameOrigin, GameSettings, SessionInfo};
use lapinux_core::paths::LapinuxPaths;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

const APPID: u32 = 424242;

/// Ensures that a failed assertion never leaves the sandboxed daemon running.
struct Daemon(std::process::Child);
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn ipc(socket: &Path, req: &lapinux_core::ipc::Req) -> lapinux_core::ipc::Envelope {
    lapinux_core::ipc::send_request_timeout(socket, req, Duration::from_secs(20)).unwrap()
}

fn ipc_launch(socket: &Path, dry_run: bool) -> lapinux_core::ipc::Envelope {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let reply = ipc(
            socket,
            &lapinux_core::ipc::Req::Launch {
                appid: APPID,
                dry_run,
            },
        );
        if reply.error.as_deref().is_some_and(|error| {
            error.contains("opération de sauvegarde ou de lancement est déjà en cours")
        }) && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(50));
        } else {
            return reply;
        }
    }
}

fn cc(args: &[&str]) {
    let output = Command::new("cc")
        .args(args)
        .output()
        .expect("C compiler required");
    assert!(
        output.status.success(),
        "cc: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn wait_idle(shared: &lapinux_daemon::Shared) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while shared.session_info().is_some() {
        assert!(Instant::now() < deadline, "previous session did not stop");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn launch_when_ready(
    shared: &Arc<lapinux_daemon::Shared>,
    dry_run: bool,
) -> lapinux_core::Result<lapinux_core::ipc::Envelope> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match shared.launch(APPID, dry_run) {
            Err(error)
                if error
                    .to_string()
                    .contains("opération de sauvegarde ou de lancement est déjà en cours")
                    && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(50));
            }
            result => return result,
        }
    }
}

fn run_game(
    shared: &Arc<lapinux_daemon::Shared>,
    game: &mut Game,
    enabled: bool,
    marker: &Path,
    dry_run_bridge: bool,
    mode: &str,
) -> String {
    wait_idle(shared);
    game.settings.steam_shim = Some(enabled);
    game.settings.args = vec![mode.into()];
    game.settings.env = vec![
        ("GAME_MARKER".into(), "preserved".into()),
        ("LD_PRELOAD".into(), marker.to_str().unwrap().into()),
    ];
    if dry_run_bridge {
        game.settings
            .env
            .push(("LAPINUX_STEAMSHIM_DRY_RUN".into(), "1".into()));
    }
    shared.save_settings(APPID, game.settings.clone()).unwrap();
    let dry: serde_json::Value = launch_when_ready(shared, true)
        .expect("dry run")
        .into_data()
        .unwrap();
    let plan = dry["command"].as_str().unwrap();
    assert_eq!(
        plan.contains("libsteam_api.so"),
        enabled,
        "unexpected bridge in command: {plan}"
    );
    let data: serde_json::Value = launch_when_ready(shared, false)
        .expect("daemon launch")
        .into_data()
        .unwrap();
    let info: SessionInfo = serde_json::from_value(data["session"].clone()).unwrap();
    let session = shared
        .session
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .session
        .clone();
    let deadline = Instant::now() + Duration::from_secs(10);
    let outcome = loop {
        if let Some(outcome) = session.lock().unwrap().try_wait().unwrap() {
            break outcome;
        }
        assert!(Instant::now() < deadline, "client did not exit");
        std::thread::sleep(Duration::from_millis(20));
    };
    let log = fs::read_to_string(&info.log_path).unwrap();
    assert_eq!(outcome.code, Some(0), "client failed:\n{log}");
    log
}

#[test]
fn native_bridge_launch_end_to_end() {
    if std::env::var_os("LAPINUX_TEST_INSTALLED_DIST").is_some() {
        installed_dist_bridge_launch();
        return;
    }
    if std::env::var_os("LAPINUX_BRIDGE_TEST_CHILD").is_none() {
        // The test executable lives in target/<profile>/deps, including when
        // Cargo uses CARGO_TARGET_DIR or a non-default profile.
        let test_exe = std::env::current_exe().unwrap();
        let shim = test_exe
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .join("libsteam_api.so");
        assert!(shim.is_file(), "build the bridge in the same profile first, e.g. `cargo build -p lapinux-steamshim`: {}", shim.display());
        let root = tempfile::Builder::new()
            .prefix("lapinux-bridge-")
            .tempdir_in(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"))
            .unwrap();
        let test_binary = root.path().join("bridge-integration-test");
        fs::copy(std::env::current_exe().unwrap(), &test_binary).unwrap();
        let output = Command::new(test_binary)
            .args(["--exact", "native_bridge_launch_end_to_end", "--nocapture"])
            .env("LAPINUX_BRIDGE_TEST_CHILD", "1")
            .env("HOME", root.path())
            .env("LAPINUX_HOME", root.path())
            .env("XDG_RUNTIME_DIR", root.path().join("run"))
            .env("XDG_CONFIG_HOME", root.path().join("xdg-config"))
            .env("XDG_DATA_HOME", root.path().join("xdg-data"))
            .env("XDG_CACHE_HOME", root.path().join("xdg-cache"))
            .env_remove("LAPINUX_STEAMSHIM_DRY_RUN")
            .env("LAPINUX_BRIDGE_TEST_SO", shim.canonicalize().unwrap())
            .env(
                "DBUS_SESSION_BUS_ADDRESS",
                "unix:path=/nonexistent-lapinux-test-bus",
            )
            .env_remove("LAPINUX_STEAMSHIM")
            .env_remove("LD_PRELOAD")
            // Cargo ajoute target/debug à LD_LIBRARY_PATH, ce qui sélectionnerait
            // le pont par son SONAME même sans injection par Lapinux.
            .env_remove("LD_LIBRARY_PATH")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("DISPLAY")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "integration child failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }

    let root = std::path::PathBuf::from(std::env::var_os("LAPINUX_HOME").unwrap());
    let paths = LapinuxPaths::under(&root);
    paths.ensure().unwrap();
    let mut config = lapinux_core::Config::default();
    config.launch.gamescope = false;
    config.launch.scene = false;
    config.general.saves_auto = false;
    config.capture.record_on_launch = false;
    config.capture.hud_on_screen = false;
    config.capture.record_badge = false;
    config.input.grab_devices = false;
    config.overlay.pad_hotkey.clear();
    config.overlay.hotkey.clear();
    config.save(&paths.config_file).unwrap();

    let game_dir = root.join("library/steamapps/common/test");
    fs::create_dir_all(&game_dir).unwrap();
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/native_steamshim");
    let original = game_dir.join("libsteam_api.so");
    cc(&[
        "-shared",
        "-fPIC",
        "-Wl,-soname,libsteam_api.so",
        fixtures.join("steam_stub.c").to_str().unwrap(),
        "-o",
        original.to_str().unwrap(),
    ]);
    let exe = game_dir.join("client");
    cc(&[
        fixtures.join("client.c").to_str().unwrap(),
        "-L",
        game_dir.to_str().unwrap(),
        "-Wl,-rpath,$ORIGIN",
        "-lsteam_api",
        "-ldl",
        "-o",
        exe.to_str().unwrap(),
    ]);
    let marker_source = root.join("marker.c");
    fs::write(
        &marker_source,
        "int lapinux_preload_marker(void) { return 1; }\n",
    )
    .unwrap();
    let marker = root.join("marker.so");
    cc(&[
        "-shared",
        "-fPIC",
        marker_source.to_str().unwrap(),
        "-o",
        marker.to_str().unwrap(),
    ]);
    let before = fs::read(&original).unwrap();
    let shim = paths.data_dir.join("libsteam_api.so");
    fs::copy(std::env::var_os("LAPINUX_BRIDGE_TEST_SO").unwrap(), &shim).unwrap();

    let shared = Arc::new(lapinux_daemon::Shared::new(&paths).unwrap());
    let mut game = Game {
        appid: APPID,
        name: "Native test".into(),
        installdir: "test".into(),
        library: root.join("library"),
        origin: GameOrigin::Steam,
        installed: true,
        native: true,
        exe: Some(exe.clone()),
        settings: GameSettings {
            cloud_sync: Some(false),
            performance: true,
            ..Default::default()
        },
        ..Default::default()
    };
    shared.store.save_game(&game).unwrap();
    assert!(run_game(&shared, &mut game, true, &marker, true, "enabled")
        .contains("enabled: native bridge"));
    assert!(
        run_game(&shared, &mut game, false, &marker, false, "disabled")
            .contains("disabled: original library")
    );
    wait_idle(&shared);
    assert_eq!(
        fs::read(&original).unwrap(),
        before,
        "game library must remain untouched"
    );

    // Fake gamescope executes the real child, without loading the bridge itself.
    use std::os::unix::fs::PermissionsExt;
    let bin = root.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let wrapper = bin.join("gamescope");
    fs::write(&wrapper, "#!/bin/sh\nprintf '%s' \"${LD_PRELOAD-unset}\" > \"$LAPINUX_HOME/wrapper-preload\"\nwhile [ \"$1\" != -- ]; do shift; done\nshift\nexec \"$@\"\n").unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::set_var(
        "PATH",
        std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(&path))).unwrap(),
    );
    game.settings.gamescope = Some(true);
    assert!(run_game(&shared, &mut game, true, &marker, true, "enabled")
        .contains("enabled: native bridge"));
    assert_eq!(
        fs::read_to_string(root.join("wrapper-preload")).unwrap(),
        marker.to_str().unwrap()
    );
    wait_idle(&shared);
    game.settings.gamescope = Some(false);

    assert!(
        run_game(&shared, &mut game, true, &marker, false, "no-session")
            .contains("missing session rejected")
    );
    // Fake saved session: no real token or network. Failed persistence is
    // reported to the game, rather than falsely acknowledging the success.
    fs::write(
        paths.data_dir.join("steam-session.json"),
        r#"{"account_name":"test","steam_id":123,"refresh_token":"fake-test-token"}"#,
    )
    .unwrap();
    // A per-game HOME override must not redirect the bridge away from the daemon.
    let game_home = root.join("different-game-home");
    fs::create_dir_all(&game_home).unwrap();
    std::env::set_var("HOME", &game_home);
    std::env::set_var("LAPINUX_HOME", &game_home);
    let spool = paths.data_dir.join("achievements");
    fs::write(&spool, b"not a directory").unwrap();
    assert!(
        run_game(&shared, &mut game, true, &marker, false, "spool-failure")
            .contains("spool failure reported")
    );
    fs::remove_file(&spool).unwrap();
    // StoreStats persists the request even if the native process exits immediately.
    assert!(
        run_game(&shared, &mut game, true, &marker, false, "enabled")
            .contains("enabled: native bridge")
    );
    let files: Vec<_> = fs::read_dir(paths.data_dir.join("achievements"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "json"))
        .collect();
    assert_eq!(files.len(), 1);
    let job: serde_json::Value = serde_json::from_slice(&fs::read(&files[0]).unwrap()).unwrap();
    assert_eq!(job["appid"], APPID);
    assert_eq!(job["steam_id"], 123);
    assert_eq!(job["names"], serde_json::json!(["ACH_TEST"]));
    wait_idle(&shared);

    game.settings.steam_shim = Some(true);
    shared.save_settings(APPID, game.settings.clone()).unwrap();
    fs::remove_file(&shim).unwrap();
    let missing = launch_when_ready(&shared, false)
        .err()
        .expect("missing shim must be explicit")
        .to_string();
    assert!(missing.contains("introuvable"), "{missing}");
    let mut elf32 = [0u8; 20];
    elf32[..4].copy_from_slice(b"\x7fELF");
    elf32[4] = 1;
    elf32[5] = 1;
    elf32[18] = 3;
    fs::write(&shim, elf32).unwrap();
    let incompatible = launch_when_ready(&shared, false)
        .err()
        .expect("wrong shim ABI must fail")
        .to_string();
    assert!(incompatible.contains("incompatible"), "{incompatible}");
    let mut truncated = elf32;
    truncated[4] = 2;
    truncated[18] = 62;
    fs::write(&shim, truncated).unwrap();
    let invalid = launch_when_ready(&shared, false)
        .err()
        .expect("truncated bridge must fail")
        .to_string();
    assert!(invalid.contains("incompatible ou corrompu"), "{invalid}");
    fs::copy(std::env::var_os("LAPINUX_BRIDGE_TEST_SO").unwrap(), &shim).unwrap();
    let unsupported = root.join("unsupported");
    fs::write(&unsupported, elf32).unwrap();
    game.exe = Some(unsupported);
    shared.store.save_game(&game).unwrap();
    let incompatible = launch_when_ready(&shared, false)
        .err()
        .expect("wrong game ABI must fail")
        .to_string();
    assert!(
        incompatible.contains("architecture du jeu"),
        "{incompatible}"
    );
}

/// Test opt-in : utilise les vrais binaire et pont du profil dist, installés
/// ensemble. Le daemon tourne sous bubblewrap afin de ne pas accéder au bureau,
/// au réseau, aux périphériques ou aux processus de l'utilisateur.
fn installed_dist_bridge_launch() {
    use lapinux_core::ipc::Req;
    use std::os::unix::fs::PermissionsExt;

    let project = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let target = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let build = target.join("dist");
    for name in ["lapinux", "lapinux-probe", "libsteam_api.so"] {
        assert!(
            build.join(name).is_file(),
            "build first: cargo build --profile dist -p lapinux -p lapinux-steamshim (missing {})",
            build.join(name).display()
        );
    }
    let root = tempfile::Builder::new()
        .prefix("lapinux-installed-")
        .tempdir_in(&target)
        .unwrap();
    let root = root.path().canonicalize().unwrap();
    let install = root.join("installed");
    let output = Command::new("bash")
        .arg(project.join("scripts/install-artifacts.sh"))
        .arg(&build)
        .arg(&install)
        .output()
        .expect("install script");
    assert!(
        output.status.success(),
        "installation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let bridge = install.join("libsteam_api.so");
    let binary = install.join("lapinux");
    // Une seconde installation ne doit pas écraser un répertoire existant.
    let installed_before = fs::read(&bridge).unwrap();
    let retry = Command::new("bash")
        .arg(project.join("scripts/install-artifacts.sh"))
        .arg(&build)
        .arg(&install)
        .output()
        .unwrap();
    assert!(!retry.status.success());
    assert_eq!(fs::read(&bridge).unwrap(), installed_before);
    let incomplete = root.join("incomplete");
    fs::create_dir(&incomplete).unwrap();
    let empty_install = root.join("should-not-install");
    let missing = Command::new("bash")
        .arg(project.join("scripts/install-artifacts.sh"))
        .arg(&incomplete)
        .arg(&empty_install)
        .output()
        .unwrap();
    assert!(!missing.status.success() && !empty_install.exists());
    let state = root.join("state");
    let home = root.join("home");
    fs::create_dir_all(&home).unwrap();
    let paths = LapinuxPaths::under(&state);
    paths.ensure().unwrap();
    let mut config = lapinux_core::Config::default();
    config.general.library_root = root.join("managed").to_string_lossy().into_owned();
    config.launch.gamescope = false;
    config.launch.scene = false;
    config.general.saves_auto = false;
    config.capture.record_on_launch = false;
    config.capture.hud_on_screen = false;
    config.capture.record_badge = false;
    config.input.grab_devices = false;
    config.overlay.pad_hotkey.clear();
    config.overlay.hotkey.clear();
    config.save(&paths.config_file).unwrap();

    let library = root.join("library");
    let game_dir = library.join("steamapps/common/test");
    fs::create_dir_all(&game_dir).unwrap();
    let fixtures = project.join("crates/daemon/tests/fixtures/native_steamshim");
    let original = game_dir.join("libsteam_api.so");
    cc(&[
        "-shared",
        "-fPIC",
        "-Wl,-soname,libsteam_api.so",
        fixtures.join("steam_stub.c").to_str().unwrap(),
        "-o",
        original.to_str().unwrap(),
    ]);
    let client = game_dir.join("client");
    cc(&[
        fixtures.join("client.c").to_str().unwrap(),
        "-L",
        game_dir.to_str().unwrap(),
        "-Wl,-rpath,$ORIGIN",
        "-lsteam_api",
        "-ldl",
        "-o",
        client.to_str().unwrap(),
    ]);
    let marker_source = root.join("marker.c");
    let marker = root.join("marker.so");
    fs::write(
        &marker_source,
        "int lapinux_preload_marker(void) { return 1; }\n",
    )
    .unwrap();
    cc(&[
        "-shared",
        "-fPIC",
        marker_source.to_str().unwrap(),
        "-o",
        marker.to_str().unwrap(),
    ]);
    let wrapper = game_dir.join("launch.sh");
    fs::write(&wrapper, "#!/bin/sh\n\"$1\" \"$2\"\nstatus=$?\nprintf '%s' \"$status\" > \"$TEST_RESULT\"\nexit \"$status\"\n").unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    let original_before = fs::read(&original).unwrap();
    let client_before = fs::read(&client).unwrap();
    let wrapper_before = fs::read(&wrapper).unwrap();
    let result_file = root.join("game-result");
    let unsupported = root.join("unsupported-32");
    let mut elf32 = [0u8; 20];
    elf32[..4].copy_from_slice(b"\x7fELF");
    elf32[4] = 1;
    elf32[5] = 1;
    elf32[18] = 3;
    fs::write(&unsupported, elf32).unwrap();
    let settings = |enabled: bool, dry_run: bool| {
        let mut env = vec![
            ("GAME_MARKER".into(), "preserved".into()),
            ("LD_PRELOAD".into(), marker.to_string_lossy().into_owned()),
            (
                "EXPECTED_STEAM_PROVIDER".into(),
                if enabled { &bridge } else { &original }
                    .to_string_lossy()
                    .into_owned(),
            ),
            (
                "TEST_RESULT".into(),
                result_file.to_string_lossy().into_owned(),
            ),
        ];
        if dry_run {
            env.push(("LAPINUX_STEAMSHIM_DRY_RUN".into(), "1".into()));
        }
        GameSettings {
            steam_shim: Some(enabled),
            cloud_sync: Some(false),
            performance: true,
            args: vec![
                client.to_string_lossy().into_owned(),
                if enabled { "enabled" } else { "disabled" }.into(),
            ],
            env,
            ..Default::default()
        }
    };
    {
        let store = lapinux_core::store::Store::open(&paths.db_file).unwrap();
        store
            .save_game(&Game {
                appid: APPID,
                name: "Native test".into(),
                installdir: "test".into(),
                library,
                origin: GameOrigin::Steam,
                installed: true,
                native: true,
                exe: Some(wrapper.clone()),
                settings: settings(true, true),
                ..Default::default()
            })
            .unwrap();
        store
            .save_game(&Game {
                appid: APPID + 1,
                name: "Unsupported architecture".into(),
                installdir: "test".into(),
                library: root.join("library"),
                origin: GameOrigin::Steam,
                installed: true,
                native: true,
                exe: Some(unsupported.clone()),
                settings: settings(true, true),
                ..Default::default()
            })
            .unwrap();
    }
    assert!(!paths.data_dir.join("libsteam_api.so").exists());

    let log = fs::File::create(root.join("daemon.log")).unwrap();
    let child = Command::new("bwrap")
        .args([
            "--unshare-user",
            "--unshare-pid",
            "--unshare-net",
            "--die-with-parent",
            "--ro-bind",
            "/",
            "/",
            "--proc",
            "/proc",
            "--dev",
            "/dev",
            "--tmpfs",
            "/tmp",
            "--tmpfs",
            "/run",
            "--bind",
        ])
        .arg(&root)
        .arg(&root)
        .args([
            "--clearenv",
            "--setenv",
            "PATH",
            "/usr/bin:/bin",
            "--setenv",
            "DBUS_SESSION_BUS_ADDRESS",
            "unix:path=/nonexistent-lapinux-test-bus",
            "--setenv",
            "HOME",
        ])
        .arg(&home)
        .args(["--setenv", "LAPINUX_HOME"])
        .arg(&state)
        .args(["--setenv", "XDG_RUNTIME_DIR"])
        .arg(root.join("runtime"))
        .args(["--chdir"])
        .arg(&root)
        .arg(&binary)
        .arg("--daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone().unwrap()))
        .stderr(Stdio::from(log))
        .spawn()
        .expect("bubblewrap is required for isolated production test");
    let mut daemon = Daemon(child);
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if lapinux_core::ipc::send_request_timeout(
            &paths.socket,
            &Req::Ping,
            Duration::from_millis(300),
        )
        .is_ok()
        {
            break;
        }
        if let Some(status) = daemon.0.try_wait().unwrap() {
            panic!(
                "isolated daemon exited {status}: {}",
                fs::read_to_string(root.join("daemon.log")).unwrap_or_default()
            );
        }
        assert!(
            Instant::now() < deadline,
            "daemon not ready: {}",
            fs::read_to_string(root.join("daemon.log")).unwrap_or_default()
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    for (enabled, dry_run) in [(true, true), (false, true), (true, false)] {
        if !dry_run {
            // Session fictive : le réseau est isolé, aucun compte réel n'est utilisé.
            fs::write(
                paths.data_dir.join("steam-session.json"),
                r#"{"account_name":"test","steam_id":123,"refresh_token":"fake-test-token"}"#,
            )
            .unwrap();
        }
        let saved = ipc(
            &paths.socket,
            &Req::SaveSettings {
                appid: APPID,
                settings: settings(enabled, dry_run),
            },
        );
        assert!(saved.ok, "save settings: {:?}", saved.error);
        let dry = ipc_launch(&paths.socket, true);
        assert!(dry.ok, "dry launch: {:?}", dry.error);
        let plan = dry.data["command"].as_str().unwrap();
        assert_eq!(plan.contains(bridge.to_str().unwrap()), enabled, "{plan}");
        let launched = ipc_launch(&paths.socket, false);
        assert!(launched.ok, "launch: {:?}", launched.error);
        let game_log = PathBuf::from(launched.data["session"]["log_path"].as_str().unwrap());
        let deadline = Instant::now() + Duration::from_secs(20);
        while !result_file.is_file() {
            assert!(
                Instant::now() < deadline,
                "game did not exit; log: {}",
                fs::read_to_string(&game_log).unwrap_or_default()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        let log = fs::read_to_string(&game_log).unwrap();
        assert_eq!(
            fs::read_to_string(&result_file).unwrap(),
            "0",
            "client failure: {log}"
        );
        let expected = if enabled {
            "enabled: native bridge"
        } else {
            "disabled: original library"
        };
        assert!(log.contains(expected), "{log}");
        assert!(log.contains("verified SteamAPI_Init provider="), "{log}");
        fs::remove_file(&result_file).unwrap();
        while ipc(&paths.socket, &Req::Session).data != serde_json::Value::Null {
            assert!(Instant::now() < deadline, "session did not stop");
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(fs::read(&original).unwrap(), original_before);
        assert_eq!(fs::read(&client).unwrap(), client_before);
        assert_eq!(fs::read(&wrapper).unwrap(), wrapper_before);
        if !dry_run {
            let spool = paths.data_dir.join("achievements");
            let files: Vec<_> = fs::read_dir(&spool)
                .unwrap()
                .map(|e| e.unwrap().path())
                .filter(|p| p.extension().is_some_and(|ext| ext == "json"))
                .collect();
            assert_eq!(files.len(), 1, "achievement must survive process exit");
            let lot: serde_json::Value =
                serde_json::from_slice(&fs::read(&files[0]).unwrap()).unwrap();
            assert_eq!(lot["appid"], APPID);
            assert_eq!(lot["steam_id"], 123);
            assert_eq!(lot["names"], serde_json::json!(["ACH_TEST"]));
            println!("installed production bridge: achievement queued without network");
        } else {
            println!(
                "installed production bridge: {}",
                if enabled { "enabled" } else { "disabled" }
            );
        }
    }

    let saved = ipc(
        &paths.socket,
        &Req::SaveSettings {
            appid: APPID,
            settings: settings(true, true),
        },
    );
    assert!(saved.ok);
    let missing_path = install.join("bridge-saved");
    fs::rename(&bridge, &missing_path).unwrap();
    let missing = ipc_launch(&paths.socket, false);
    assert!(!missing.ok && missing.error.unwrap_or_default().contains("introuvable"));
    fs::write(&bridge, elf32).unwrap();
    let incompatible = ipc_launch(&paths.socket, false);
    assert!(
        !incompatible.ok
            && incompatible
                .error
                .unwrap_or_default()
                .contains("incompatible")
    );
    fs::remove_file(&bridge).unwrap();
    fs::rename(&missing_path, &bridge).unwrap();
    let game_arch = ipc(
        &paths.socket,
        &Req::Launch {
            appid: APPID + 1,
            dry_run: false,
        },
    );
    assert!(
        !game_arch.ok
            && game_arch
                .error
                .unwrap_or_default()
                .contains("architecture du jeu")
    );
    assert_eq!(fs::read(&wrapper).unwrap(), wrapper_before);
    assert_eq!(fs::read(&original).unwrap(), original_before);
    assert_eq!(fs::read(&client).unwrap(), client_before);
    let stopped = ipc(&paths.socket, &Req::Shutdown);
    assert!(stopped.ok);
    let status = daemon.0.wait().unwrap();
    assert!(status.success(), "daemon shutdown: {status}");
    println!("installed production bridge: absent and incompatible diagnostics verified");
}
