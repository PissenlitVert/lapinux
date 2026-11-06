//! Installation de bout en bout d'une configuration : le catalogue est servi
//! par un vrai serveur HTTP local, le jeu existe sur disque, et le daemon
//! installe puis retire les fichiers et les réglages.

use lapinux_core::configurations::{ConfigurationSet, InstallReport};
use lapinux_core::model::Game;
use lapinux_core::paths::LapinuxPaths;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// Sert le catalogue et un fichier sur une adresse de boucle locale, jusqu'à
/// ce que le test ait fini.
fn serve(catalog: String, file: Vec<u8>) -> (String, Arc<AtomicBool>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let stop = Arc::new(AtomicBool::new(false));
    let halted = Arc::clone(&stop);
    std::thread::spawn(move || {
        while !halted.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let _ = answer(&mut stream, &catalog, &file);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
    });
    (format!("http://127.0.0.1:{port}"), stop)
}

fn answer(stream: &mut TcpStream, catalog: &str, file: &[u8]) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    let mut buffer = [0u8; 2048];
    let read = stream.read(&mut buffer)?;
    let request = String::from_utf8_lossy(&buffer[..read]);
    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/");
    let (status, body, kind): (&str, &[u8], &str) = if path == "/catalog.json" {
        ("200 OK", catalog.as_bytes(), "application/json")
    } else if path == "/configurations/test-game/hello/hello.txt" {
        ("200 OK", file, "text/plain")
    } else {
        ("404 Not Found", b"not found", "text/plain")
    };
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    stream.flush()
}

fn catalog(file: &[u8]) -> String {
    serde_json::json!({
        "version": 2,
        "maintainer": { "name": "Pissenlit Vert", "github": "PissenlitVert" },
        "configurations": [
            {
                "id": "test-game-hello",
                "title": "Test Game — hello file",
                "description": "The test game has no greeting.",
                "solution": "Writes one, and raises shared memory for the test.",
                "type": "content",
                "platform": "linux",
                "layers": ["game"],
                "author": { "name": "Test", "github": "PissenlitVert" },
                "version": "1.0.0",
                "license": "CC0-1.0",
                "status": "verified",
                "game": { "names": ["Test Game"], "steam_appids": [4242] },
                "actions": {
                    "settings": { "dxvk": { "shared_mb": 1024 } },
                    "files": [{
                        "source": "hello.txt",
                        "target": "data/hello.txt",
                        "sha256": sha256(file)
                    }]
                },
                "base": "configurations/test-game/hello"
            },
            {
                "id": "test-game-bad-hash",
                "title": "Test Game — broken digest",
                "description": "The published file does not match its digest.",
                "solution": "Must be refused before writing anything.",
                "type": "content",
                "platform": "linux",
                "layers": ["game"],
                "author": { "name": "Test" },
                "version": "1.0.0",
                "license": "CC0-1.0",
                "status": "proposed",
                "game": { "names": ["Test Game"], "steam_appids": [4242] },
                "actions": {
                    "settings": { "args": ["--should-not-run"] },
                    "files": [{
                        "source": "hello.txt",
                        "target": "data/hello.txt",
                        "sha256": "0000000000000000000000000000000000000000000000000000000000000000"
                    }]
                },
                "base": "configurations/test-game/hello"
            }
        ]
    })
    .to_string()
}

fn await_catalog(shared: &lapinux_daemon::Shared) -> ConfigurationSet {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(envelope) = shared.game_configurations(4242) {
            if let Ok(set) = envelope.into_data::<ConfigurationSet>() {
                if set.remote {
                    return set;
                }
            }
        }
        assert!(
            Instant::now() < deadline,
            "le catalogue local n'a jamais été lu"
        );
        std::thread::sleep(Duration::from_millis(40));
    }
}

#[test]
fn installe_et_retire_une_configuration() {
    let dir = tempfile::tempdir().unwrap();
    let paths = LapinuxPaths::under(dir.path());
    paths.ensure().unwrap();
    let game_dir = dir
        .path()
        .join("library/steamapps/common/Test Game/data");
    std::fs::create_dir_all(&game_dir).unwrap();
    std::fs::write(game_dir.join("hello.txt"), b"original").unwrap();

    let content = b"bonjour".to_vec();
    let (url, stop) = serve(catalog(&content), content.clone());
    std::env::set_var(
        "LAPINUX_CONFIGURATIONS_CATALOG",
        format!("{url}/catalog.json"),
    );

    let game = Game {
        appid: 4242,
        name: "Test Game".into(),
        installdir: "Test Game".into(),
        library: dir.path().join("library"),
        installed: true,
        native: true,
        ..Default::default()
    };
    let shared = Arc::new(lapinux_daemon::Shared::new(&paths).unwrap());
    shared.store.save_game(&game).unwrap();
    shared.start_configurations_in_background();

    let set = await_catalog(&shared);
    assert!(set.configurations.iter().any(|entry| entry.id == "test-game-hello"));
    assert!(set.installed.is_empty());

    let envelope = shared
        .install_configuration(4242, "test-game-hello")
        .expect("installation");
    assert!(envelope.ok, "{:?}", envelope.error);
    let report: InstallReport = envelope.into_data().unwrap();
    assert_eq!(report.files, 1);
    assert_eq!(
        std::fs::read_to_string(game_dir.join("hello.txt")).unwrap(),
        "bonjour"
    );
    let updated = shared.store.game(4242).unwrap().unwrap();
    assert_eq!(updated.settings.dxvk.shared_mb, Some(1024));
    let backup = paths
        .data_dir
        .join("configurations/4242/test-game-hello/backup/data/hello.txt");
    assert_eq!(std::fs::read_to_string(&backup).unwrap(), "original");
    let set = shared
        .game_configurations(4242)
        .unwrap()
        .into_data::<ConfigurationSet>()
        .unwrap();
    assert_eq!(set.installed.len(), 1);

    // Un digest faux est refusé avant toute écriture : ni fichier, ni réglage.
    let refused = shared.install_configuration(4242, "test-game-bad-hash");
    assert!(refused.is_err(), "un digest faux doit être refusé");
    assert_eq!(
        std::fs::read_to_string(game_dir.join("hello.txt")).unwrap(),
        "bonjour"
    );
    let untouched = shared.store.game(4242).unwrap().unwrap();
    assert!(untouched.settings.args.is_empty());

    let envelope = shared
        .remove_configuration(4242, "test-game-hello")
        .expect("retrait");
    assert!(envelope.ok, "{:?}", envelope.error);
    assert_eq!(
        std::fs::read_to_string(game_dir.join("hello.txt")).unwrap(),
        "original"
    );
    let restored = shared.store.game(4242).unwrap().unwrap();
    assert_eq!(restored.settings.dxvk.shared_mb, None);
    let set = shared
        .game_configurations(4242)
        .unwrap()
        .into_data::<ConfigurationSet>()
        .unwrap();
    assert!(set.installed.is_empty());
    assert!(!Path::new(&backup).exists());

    stop.store(true, Ordering::Relaxed);
    std::env::remove_var("LAPINUX_CONFIGURATIONS_CATALOG");
}
