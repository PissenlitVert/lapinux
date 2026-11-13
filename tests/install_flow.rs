//! Parcours réel d'une installation Lapinux : bibliothèque, daemon et CLI.
//!
//! Le test vérifie que `general.library_root` est une vraie bibliothèque :
//! le jeu installé est visible, lancé en dry-run avec un préfixe Lapinux, et
//! survit à un rescan. Un manifeste Steam sans fichiers ne doit pas être
//! confondu avec une installation Lapinux.

use lapinux_core::client::Client;
use lapinux_core::ipc::Req;
use lapinux_core::model::{Game, GameOrigin};
use lapinux_core::paths::LapinuxPaths;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

const LAPINUX_APPID: u32 = 990_001;
const STEAM_AUTHORED_APPID: u32 = 990_999;

#[test]
fn diagnostic_export_without_daemon_is_private_and_does_not_overwrite() {
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir().unwrap();
    let output = home.path().join("report.json");
    let run = || Command::new(env!("CARGO_BIN_EXE_lapinux"))
        .args(["cli", "diagnostic-export"])
        .arg(&output)
        .env("LAPINUX_HOME", home.path())
        .output().unwrap();
    let first = run();
    assert!(first.status.success(), "{}", String::from_utf8_lossy(&first.stderr));
    let report: serde_json::Value = serde_json::from_slice(&fs::read(&output).unwrap()).unwrap();
    assert_eq!(report["format"], "lapinux-diagnostic-v1");
    assert_eq!(report["daemon_responding"], false);
    assert!(report["checks"].is_null());
    assert!(report["downloads"].is_null());
    assert_eq!(fs::metadata(&output).unwrap().permissions().mode() & 0o777, 0o600);
    fs::write(&output, b"keep this file").unwrap();
    assert!(!run().status.success());
    assert_eq!(fs::read(&output).unwrap(), b"keep this file");
}

struct Scenario {
    home: TempDir,
    paths: LapinuxPaths,
    library: PathBuf,
}

impl Scenario {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let root = home.path();
        let library = root.join("library");
        fs::create_dir_all(library.join("Steam")).unwrap();
        fs::create_dir_all(library.join("steamapps")).unwrap();
        fs::create_dir_all(root.join("config")).unwrap();
        fs::write(
            root.join("config/config.toml"),
            format!(
                "[general]\nlibrary_root = \"{}\"\n\n\
                 [steam]\nusername = \"tester\"\n\n\
                 [launch]\nproton = \"Lapinux-Test-Proton\"\ngamescope = false\n",
                library.display()
            ),
        )
        .unwrap();
        // Manifeste écrit par Steam : aucun fichier installé.
        fs::write(
            library.join(format!("Steam/appmanifest_{STEAM_AUTHORED_APPID}.acf")),
            format!(
                "\"AppState\" {{\n  \"appid\" \"{STEAM_AUTHORED_APPID}\"\n  \"name\" \"Steam Authored\"\n  \
                 \"StateFlags\" \"4\"\n  \"installdir\" \"SteamAuthored\"\n}}\n"
            ),
        )
        .unwrap();
        // Compatdata piège dans la bibliothèque : un jeu Lapinux ne doit pas l'utiliser.
        for appid in [LAPINUX_APPID] {
            fs::create_dir_all(library.join(format!("steamapps/compatdata/{appid}/pfx"))).unwrap();
        }
        // Proton factice : un vrai script Python qui note l'exécutable reçu,
        // pour vérifier qu'un lancement passe bien par Proton.
        let proton = root.join("data/compatibility/protons/Lapinux-Test-Proton/proton");
        fs::create_dir_all(proton.parent().unwrap()).unwrap();
        write_exec(
            &proton,
            r#"#!/usr/bin/env python3
import os, sys
here = os.path.dirname(os.path.abspath(sys.argv[0]))
with open(os.path.join(here, "calls.log"), "a") as log:
    log.write("\n".join(sys.argv[1:]) + "\n")
"#,
        );
        // Installation déjà posée par Lapinux : manifeste géré et exécutable.
        let installdir = format!("Jeu {LAPINUX_APPID}");
        let target = library.join("Steam").join(&installdir);
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join(format!("jeu{LAPINUX_APPID}.exe")), b"MZfake").unwrap();
        lapinux_steam::manifest::write_manifest(
            &library,
            LAPINUX_APPID,
            &format!("Jeu {LAPINUX_APPID}"),
            &installdir,
            6,
            true,
        )
        .unwrap();

        let paths = LapinuxPaths::under(root);
        Self {
            home,
            paths,
            library,
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        let output = Command::new(env!("CARGO_BIN_EXE_lapinux"))
            .args(args)
            .env("LAPINUX_HOME", self.home.path())
            .env("LAPINUX_LOG", "error")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "lapinux {:?} a échoué ({:?})\nstdout:\n{}\nstderr:\n{}",
            args,
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    fn shutdown(&self) {
        let _ = Client::new(&self.paths)
            .without_spawn()
            .request(&Req::Shutdown);
        std::thread::sleep(std::time::Duration::from_millis(400));
    }

    fn games(&self) -> Vec<Game> {
        let output = self.run(&["cli", "scan", "--json"]);
        serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
            panic!(
                "sortie scan illisible ({err})\n{}",
                String::from_utf8_lossy(&output.stdout)
            )
        })
    }
}

impl Drop for Scenario {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn write_exec(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn game<'a>(games: &'a [Game], appid: u32) -> &'a Game {
    games
        .iter()
        .find(|game| game.appid == appid)
        .unwrap_or_else(|| {
            panic!(
                "jeu {appid} absent de {:?}",
                games.iter().map(|g| g.appid).collect::<Vec<_>>()
            )
        })
}

fn installed_game_still_launches(scenario: &Scenario, appid: u32) {
    let games = scenario.games();
    let installed = game(&games, appid);
    assert!(installed.installed, "le jeu {appid} doit être installé");
    assert_eq!(
        installed.library, scenario.library,
        "le jeu doit être rattaché à library_root"
    );
    assert_eq!(installed.origin, GameOrigin::Lapinux);
    assert!(
        installed.prefix.is_none(),
        "pas de compatdata Steam pour Lapinux"
    );
    assert!(
        installed
            .exe
            .as_ref()
            .map(|exe| exe.starts_with(&scenario.library))
            .unwrap_or(false),
        "l'exécutable doit venir de la bibliothèque Lapinux"
    );

    let dry_run = scenario.run(&["cli", "launch", &appid.to_string(), "--dry-run"]);
    let plan = String::from_utf8_lossy(&dry_run.stdout);
    // Un jeu Lapinux range son préfixe dans `CompatData`, sous le nom du jeu,
    // pas dans le `compatdata` de `steamapps`.
    assert!(
        plan.contains(&format!("CompatData/Jeu {appid} ({appid})")),
        "le préfixe lisible de la bibliothèque doit être utilisé :\n{plan}"
    );
    assert!(
        !plan.contains(&format!("steamapps/compatdata/{appid}")),
        "le compatdata Steam ne doit pas fuiter :\n{plan}"
    );

    // Lancement réel : Proton exécute le jeu, aucun outil externe n'est appelé.
    let proton_log = scenario
        .home
        .path()
        .join("data/compatibility/protons/Lapinux-Test-Proton/calls.log");
    let exe = installed.exe.clone().unwrap();
    let launch = scenario.run(&["cli", "launch", &appid.to_string()]);
    assert!(
        String::from_utf8_lossy(&launch.stdout).contains("\"steam_client\": false"),
        "le lancement ne doit jamais démarrer le client Steam"
    );
    let mut proton_calls = String::new();
    for _ in 0..40 {
        proton_calls = fs::read_to_string(&proton_log).unwrap_or_default();
        if proton_calls.contains(exe.to_string_lossy().as_ref()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(
        proton_calls.contains(exe.to_string_lossy().as_ref()),
        "Proton doit recevoir l'exécutable du jeu, vu : {proton_calls}\n{exe:?}\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&launch.stdout),
        String::from_utf8_lossy(&launch.stderr),
    );
    assert!(
        proton_calls.contains("run\n"),
        "Proton doit être appelé en mode run : {proton_calls}"
    );
    scenario.run(&["cli", "stop"]);

    // Le jeu doit survivre à un rescan alors que son manifeste a disparu.
    fs::remove_file(
        scenario
            .library
            .join(format!("Steam/appmanifest_{appid}.acf")),
    )
    .unwrap();
    let games = scenario.games();
    let kept = game(&games, appid);
    assert!(kept.installed, "le jeu doit rester lançable après rescan");
    assert_eq!(kept.origin, GameOrigin::Lapinux);
    assert_eq!(kept.name, installed.name);

    // L'origine et l'installation survivent aussi à l'arrêt du daemon
    // (scan direct par la CLI, sans IPC).
    scenario.shutdown();
    let restarted = scenario.games();
    let persisted = game(&restarted, appid);
    assert!(persisted.installed);
    assert_eq!(persisted.origin, GameOrigin::Lapinux);

    // Un manifeste Steam sans fichiers ne doit jamais être « installé »,
    // même placé dans la bibliothèque Lapinux.
    let steam_game = game(&games, STEAM_AUTHORED_APPID);
    assert_eq!(steam_game.origin, GameOrigin::Steam);
    assert!(!steam_game.installed);
}

#[test]
fn lapinux_install_keeps_the_game_across_rescan() {
    let scenario = Scenario::new();
    installed_game_still_launches(&scenario, LAPINUX_APPID);
}

/// Une bibliothèque de l'ancienne disposition : `steamapps/common` pour le
/// jeu, `steamapps/compatdata` pour son préfixe. Le premier scan la range sous
/// `Steam` et `CompatData`, sans recopier les fichiers.
#[test]
fn an_old_managed_library_becomes_readable() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path();
    let library = root.join("library");
    let game_dir = library.join("steamapps/common/Old Game");
    fs::create_dir_all(&game_dir).unwrap();
    fs::write(game_dir.join("old.exe"), b"MZ").unwrap();
    fs::create_dir_all(library.join("steamapps/compatdata/990002/pfx")).unwrap();
    fs::write(library.join("steamapps/compatdata/990002/pfx/state"), b"x").unwrap();
    fs::create_dir_all(root.join("config")).unwrap();
    fs::write(
        root.join("config/config.toml"),
        format!("[general]\nlibrary_root = \"{}\"\n", library.display()),
    )
    .unwrap();
    fs::write(
        library.join("steamapps/appmanifest_990002.acf"),
        "\"AppState\"\n{\n\t\"appid\"\t\t\"990002\"\n\t\"name\"\t\t\"Old Game\"\n\t\"StateFlags\"\t\t\"4\"\n\t\"installdir\"\t\t\"Old Game\"\n\t\"LapinuxManaged\"\t\t\"1\"\n}\n",
    )
    .unwrap();

    let paths = LapinuxPaths::under(root);
    let shared = lapinux_daemon::Shared::new(&paths).unwrap();
    let games = shared.rescan_library().unwrap();
    let moved = game(&games, 990002);
    assert_eq!(moved.origin, GameOrigin::Lapinux);
    assert!(moved.installed);
    assert!(library.join("Steam/Old Game/old.exe").is_file());
    assert!(library.join("Steam/appmanifest_990002.acf").is_file());
    assert!(library
        .join("CompatData/Old Game (990002)/pfx/state")
        .is_file());
    assert!(!library.join("steamapps").exists());
    assert!(!library.join("compatdata").exists());
    drop(home);
}
