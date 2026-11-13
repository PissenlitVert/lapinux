//! Interface en ligne de commande de Lapinux.

use lapinux_core::client::Client;
use lapinux_core::config::Config;
use lapinux_core::ipc::{Envelope, Req};
use lapinux_core::model::{Drm, Game, JobState};
use lapinux_core::paths::LapinuxPaths;
use lapinux_core::store::Store;
use lapinux_core::util::format_bytes;
use std::time::Duration;

pub fn run(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        Some("doctor") => doctor(),
        Some("diagnostic-export") => diagnostic_export(args.get(1)),
        Some("scan") => scan(args.iter().any(|a| a == "--json")),
        Some("list") => list(args.iter().any(|a| a == "--json")),
        Some("protons") => protons(args.iter().any(|a| a == "--json")),
        Some("configurations") => configurations(args.get(1), args.iter().any(|a| a == "--json")),
        Some("releases") => releases(),
        Some("devices") => devices(),
        Some("access") => access(),
        Some("input-test") => input_test(arg_value(args, "--seconds"), false),
        Some("virtual-pad") => input_test(None, true),
        Some("login") => login(args.get(1).map(String::as_str)),
        Some("install") => install(args.get(1)),
        Some("launch") => launch(args.get(1), args.iter().any(|a| a == "--dry-run")),
        Some("jobs") => jobs(),
        Some("session") => session(),
        Some("stop") => stop(),
        Some("config") => print_config(),
        // La sonde vit dans `lapinux-steam` ; le binaire dédié est
        // `lapinux-probe`, ce point d'entrée reste un raccourci.
        Some("steam-probe") => lapinux_steam::probe::cli(&args[1..]),
        Some("overlay-demo") => overlay_demo(args),
        Some("help") | None => {
            help();
            0
        }
        Some(other) => {
            eprintln!("commande inconnue : {other}");
            help();
            2
        }
    }
}

fn help() {
    let name = lapinux_core::APP_NAME;
    let bin = lapinux_core::APP_BIN;
    println!(
        "{name} — gestionnaire de jeux\n\
         \n\
         Utilisation :\n\
         \x20 {bin}                        interface graphique\n\
         \x20 {bin} --daemon               daemon (bibliothèque, téléchargements, overlay)\n\
         \x20 {bin} overlay [options]      quick menu en jeu\n\
         \x20 {bin} scene [options] -- CMD  prototype de scène persistante GameScope\n\
         \x20 {bin} cli <commande>         commandes ci-dessous\n\
         \n\
         Commandes :\n\
         \x20 doctor                       diagnostic de l'environnement\n\
         \x20 diagnostic-export <fichier>   export privé JSON sans journaux ni comptes\n\
         \x20 scan [--json]                scanne les bibliothèques Steam\n\
         \x20 list [--json]                liste les jeux connus\n\
         \x20 protons [--json]             liste les Proton installés\n\
         \x20 configurations <appid> [--json]\n\
         \x20                              correctifs et traductions du jeu\n\
         \x20 releases                     Proton téléchargeables (GitHub)\n\
         \x20 devices                      manettes et périphériques evdev\n\
         \x20 access                       accès réels à /dev/input (clavier, manettes)\n\
         \x20 input-test [--seconds N]     lit les manettes et affiche les actions\n\
         \x20 virtual-pad                  crée le pad virtuel uinput (test)\n\
         \x20 login <compte>               connexion Lapinux (2FA, jeton réutilisable)\n\
         \x20 install <appid>              télécharge un jeu possédé\n\
         \x20 launch <appid> [--dry-run]   lance un jeu\n\
         \x20 jobs                         tâches de téléchargement\n\
         \x20 session                      session en cours\n\
         \x20 stop                         arrête la session\n\
         \x20 config                       configuration effective\n\
         \x20 steam-probe [--session]      sonde de l'hôte natif Steam (voir {bin}-probe)",
    );
}

fn arg_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn paths() -> LapinuxPaths {
    LapinuxPaths::discover()
}

fn store(paths: &LapinuxPaths) -> Result<Store, String> {
    Store::open(&paths.db_file).map_err(|e| e.to_string())
}

fn daemon_up() -> bool {
    Client::new(&paths()).without_spawn().daemon_alive()
}

fn doctor() -> i32 {
    let paths = paths();
    if daemon_up() {
        if let Ok(envelope) = Client::new(&paths).without_spawn().request(&Req::Doctor) {
            if let Ok(report) = envelope.into_data::<lapinux_core::model::DoctorReport>() {
                return print_doctor(&report);
            }
        }
    }
    match lapinux_daemon::Shared::new(&paths) {
        Ok(shared) => print_doctor(&lapinux_daemon::doctor::report(&shared)),
        Err(err) => {
            eprintln!("diagnostic impossible : {err}");
            1
        }
    }
}

fn diagnostic_export(destination: Option<&String>) -> i32 {
    use std::io::Write;
    let Some(destination) = destination else {
        eprintln!("usage : lapinux cli diagnostic-export <fichier>");
        return 2;
    };
    let client = Client::new(&paths()).without_spawn();
    let alive = match client.request_with_timeout(&Req::Ping, Duration::from_millis(600)) {
        Ok(envelope) => Some(envelope.ok),
        Err(lapinux_core::Error::IpcTimeout(_)) => None,
        Err(_) => Some(false),
    };
    let doctor = if alive == Some(true) { client.request_with_timeout(&Req::Doctor, Duration::from_secs(3)).ok().and_then(|e| e.into_data().ok()) } else { None };
    let jobs: Option<Vec<lapinux_core::model::Job>> = if alive == Some(true) { client.request_with_timeout(&Req::Jobs, Duration::from_secs(2)).ok().and_then(|e| e.into_data().ok()) } else { None };
    let report = lapinux_core::diagnostic::report(doctor.as_ref(), jobs.as_deref(), alive);
    let result = serde_json::to_vec_pretty(&report).map_err(std::io::Error::other).and_then(|data| {
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(destination)?;
        file.write_all(&data)
    });
    match result { Ok(()) => { println!("Diagnostic enregistré : {destination}"); 0 }, Err(error) => { eprintln!("Export impossible : {error}"); 1 } }
}

fn print_doctor(report: &lapinux_core::model::DoctorReport) -> i32 {
    println!("{} — diagnostic\n", lapinux_core::APP_NAME);
    for check in &report.checks {
        let symbol = match check.status {
            lapinux_core::model::CheckStatus::Ok => "OK  ",
            lapinux_core::model::CheckStatus::Warn => "ATTN",
            lapinux_core::model::CheckStatus::Fail => "ERR ",
        };
        println!("[{symbol}] {:<16} {}", check.name, check.detail);
    }
    0
}

fn scan(json: bool) -> i32 {
    let paths = paths();
    // Un daemon détient le verrou de la base redb : on passe par l'IPC.
    if daemon_up() {
        let client = Client::new(&paths).without_spawn();
        if !matches!(client.request(&Req::Scan), Ok(envelope) if envelope.ok) {
            eprintln!("scan via daemon impossible");
            return 1;
        }
        let games: Vec<Game> = match client.request(&Req::Games).and_then(Envelope::into_data) {
            Ok(games) => games,
            Err(err) => {
                eprintln!("{err}");
                return 1;
            }
        };
        let libraries = scan_libraries(&paths);
        return print_scan(json, &games, &libraries, &[]);
    }
    let store = match store(&paths) {
        Ok(store) => store,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };
    let roots = lapinux_core::paths::steam_roots();
    let config = Config::load(&paths.config_file).unwrap_or_default();
    let report = lapinux_steam::scan_with_managed(&roots, Some(&config.library_root()));
    let libraries = report.libraries.clone();
    let warnings = report.warnings.clone();
    let stored = store.load_games().unwrap_or_default();
    let games = lapinux_steam::merge_existing(report.games, &stored);
    if let Err(err) = store.save_games(&games) {
        eprintln!("sauvegarde : {err}");
        return 1;
    }
    print_scan(json, &games, &libraries, &warnings)
}

/// Racines Steam plus la bibliothèque gérée par Lapinux, sans doublon.
fn scan_libraries(paths: &LapinuxPaths) -> Vec<std::path::PathBuf> {
    let mut libraries = lapinux_core::paths::steam_roots();
    let managed = Config::load(&paths.config_file)
        .unwrap_or_default()
        .library_root();
    if !libraries.iter().any(|root| root == &managed) {
        libraries.push(managed);
    }
    libraries
}

fn print_scan(
    json: bool,
    games: &[Game],
    libraries: &[std::path::PathBuf],
    warnings: &[String],
) -> i32 {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(games).unwrap_or_default()
        );
    } else {
        println!(
            "{} bibliothèque(s), {} jeu(x) — {} installé(s)",
            libraries.len(),
            games.len(),
            games.iter().filter(|g| g.installed).count()
        );
        for library in libraries {
            println!("  · {}", library.display());
        }
        for warning in warnings {
            println!("  ! {warning}");
        }
        for game in games {
            let exe = game
                .exe
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "exe inconnu".into());
            println!(
                "  {:>8}  {:<42} {:<9} {}",
                game.appid,
                lapinux_core::util::ellipsize(&game.name, 42),
                if game.installed {
                    "installé"
                } else {
                    "absent"
                },
                if game.installed {
                    format!("{} · {}", drm_label(game.drm), exe)
                } else {
                    String::new()
                }
            );
        }
    }
    0
}

fn drm_label(drm: Drm) -> &'static str {
    match drm {
        Drm::None => "sans DRM",
        Drm::Steamworks => "Steamworks",
        Drm::SteamStub => "DRM Steam",
        Drm::Unknown => "DRM ?",
    }
}

fn list(json: bool) -> i32 {
    let paths = paths();
    let games: Vec<Game> = if daemon_up() {
        match Client::new(&paths).without_spawn().request(&Req::Games) {
            Ok(envelope) => envelope.into_data::<Vec<Game>>().unwrap_or_default(),
            Err(err) => {
                eprintln!("{err}");
                return 1;
            }
        }
    } else {
        match store(&paths) {
            Ok(store) => store.load_games().unwrap_or_default(),
            Err(err) => {
                eprintln!("{err}");
                return 1;
            }
        }
    };
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&games).unwrap_or_default()
        );
    } else {
        for game in games {
            println!(
                "{:>8}  {}  {}",
                game.appid,
                if game.installed { "[x]" } else { "[ ]" },
                game.name
            );
        }
    }
    0
}

fn protons(json: bool) -> i32 {
    let extra = vec![
        paths().proton_dir.clone(),
        lapinux_core::paths::home_dir().join(".local/share/umu/compatibilitytools"),
    ];
    let list = lapinux_proton::scan(&lapinux_core::paths::steam_roots(), &extra);
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&list).unwrap_or_default()
        );
    } else if list.is_empty() {
        println!("aucun Proton détecté");
    } else {
        let recommended = lapinux_proton::recommended(&list).map(|p| p.name.clone());
        for proton in list {
            println!(
                "{} {:<38} {:<18} {}",
                if Some(&proton.name) == recommended.as_ref() {
                    "*"
                } else {
                    " "
                },
                proton.name,
                proton.source,
                proton.path.display()
            );
        }
    }
    0
}

/// Correctifs, traductions et mods connus pour un jeu, avec ceux déjà posés.
/// La commande ne modifie rien : elle sert à vérifier le catalogue et l'état
/// d'un jeu sans ouvrir l'interface.
fn configurations(appid: Option<&String>, json: bool) -> i32 {
    let Some(appid) = appid.and_then(|s| s.parse::<u32>().ok()) else {
        eprintln!(
            "usage : {} cli configurations <appid> [--json]",
            lapinux_core::APP_BIN
        );
        return 2;
    };
    let set: lapinux_core::configurations::ConfigurationSet = match client()
        .request(&Req::GameConfigurations { appid })
        .and_then(Envelope::into_data)
    {
        Ok(set) => set,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&set).unwrap_or_default());
        return 0;
    }
    if set.configurations.is_empty() && set.installed.is_empty() {
        println!("aucune configuration connue pour le jeu {appid}");
        return 0;
    }
    println!(
        "catalogue {} · {} entrée(s)",
        if set.remote {
            "du dépôt"
        } else {
            "hors ligne"
        },
        set.configurations.len()
    );
    for entry in &set.configurations {
        let installed = set.installed.iter().find(|item| item.id == entry.id);
        println!(
            "{} {:<42} {:<9} {:<12} {}",
            if installed.is_some() { "*" } else { " " },
            entry.id,
            entry.version,
            entry.kind,
            entry.title
        );
    }
    for entry in &set.installed {
        if !set.configurations.iter().any(|known| known.id == entry.id) {
            println!(
                "* {:<42} {:<9} {:<12} {}",
                entry.id, entry.version, "hors catalogue", entry.title
            );
        }
    }
    if !set.maintainer.name.trim().is_empty() {
        println!("mainteneur : {}", set.maintainer.name);
    }
    0
}

fn releases() -> i32 {
    for source in lapinux_proton::SOURCES {
        match lapinux_proton::fetch_releases(source.repo) {
            Ok(list) => {
                println!("\n{} ({})", source.label, source.repo);
                for release in list.iter().take(6) {
                    println!(
                        "  {:<58} {:>8}  {}",
                        release.asset,
                        format_bytes(release.size),
                        if release.checksum_url.is_some() {
                            "sha512"
                        } else {
                            "sans somme"
                        }
                    );
                }
            }
            Err(err) => println!("{} : {err}", source.label),
        }
    }
    0
}

fn devices() -> i32 {
    for device in lapinux_input::device::list() {
        println!(
            "{} {:<44} {:04x}:{:04x} {} boutons {}",
            if device.gamepad {
                "MANETTE"
            } else {
                "PÉRIF  "
            },
            lapinux_core::util::ellipsize(&device.name, 44),
            device.vendor,
            device.product,
            device.buttons,
            device.path.display()
        );
        if !device.axes.is_empty() {
            println!("           axes : {}", device.axes.join(", "));
        }
    }
    0
}

fn access() -> i32 {
    for device in lapinux_input::access::audit() {
        let kind = match (device.keyboard, device.gamepad) {
            (true, _) => "CLAVIER",
            (_, true) => "MANETTE",
            _ => "PÉRIF  ",
        };
        println!(
            "{} {:<40} {:<22} {}",
            if device.readable {
                "LISIBLE"
            } else {
                "REFUSÉ "
            },
            kind,
            lapinux_core::util::ellipsize(&device.name, 38),
            device.path.display()
        );
        if let Some(error) = device.error {
            println!("           {error}");
        }
    }
    0
}

fn input_test(seconds: Option<String>, virtual_pad_only: bool) -> i32 {
    if virtual_pad_only {
        return virtual_pad_check();
    }
    let seconds = seconds.and_then(|s| s.parse::<u64>().ok()).unwrap_or(6);
    let paths = paths();
    let profile = lapinux_input::ensure_profiles(&paths.data_dir, "defaut").unwrap_or_default();
    println!(
        "Manettes détectées : {}",
        lapinux_input::device::list_gamepads().len()
    );
    println!(
        "Lecture pendant {seconds} s (appuyez sur des boutons ; l'overlay est simulé par le raccourci)…"
    );
    let service = match lapinux_input::InputService::start(profile, false) {
        Ok(service) => service,
        Err(err) => {
            eprintln!("service d'entrée : {err}");
            return 1;
        }
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(seconds);
    while std::time::Instant::now() < deadline {
        match service.events.recv_timeout(Duration::from_millis(200)) {
            Ok(lapinux_input::InputMsg::Hotkey) => println!("raccourci overlay (manette)"),
            Ok(lapinux_input::InputMsg::Nav(nav)) => println!("navigation : {nav:?}"),
            Ok(lapinux_input::InputMsg::DevicesChanged) => println!("périphériques mis à jour"),
            Ok(lapinux_input::InputMsg::Error(err)) => println!("erreur : {err}"),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => break,
        }
    }
    drop(service);
    println!("fin du test");
    0
}

fn virtual_pad_check() -> i32 {
    match lapinux_input::VirtualPad::create(&format!(
        "{} Virtual Pad (test)",
        lapinux_core::APP_NAME
    )) {
        Ok(mut pad) => {
            use lapinux_input::mapping::{Kind, VEvent};
            let events = vec![
                VEvent {
                    kind: Kind::Key,
                    code: lapinux_input::codes::BTN_SOUTH,
                    value: 1,
                },
                VEvent {
                    kind: Kind::Abs,
                    code: lapinux_input::codes::ABS_X,
                    value: 20000,
                },
            ];
            if let Err(err) = pad.emit(&events) {
                eprintln!("écriture : {err}");
                return 1;
            }
            std::thread::sleep(Duration::from_millis(300));
            let _ = pad.emit(&[VEvent {
                kind: Kind::Key,
                code: lapinux_input::codes::BTN_SOUTH,
                value: 0,
            }]);
            println!(
                "pad virtuel « {} » créé et alimenté ; il disparaît à la fin du test",
                pad.name()
            );
            0
        }
        Err(err) => {
            eprintln!("pad virtuel indisponible : {err}");
            1
        }
    }
}

fn login(username: Option<&str>) -> i32 {
    let Some(username) = username else {
        eprintln!("usage : {} cli login <compte Steam>", lapinux_core::APP_BIN);
        return 2;
    };
    let paths = paths();
    let session_path = lapinux_steam::session::path_in(&paths.data_dir);
    let saved = lapinux_steam::session::load_from(&session_path)
        .filter(|saved| saved.account_name.eq_ignore_ascii_case(username));

    if let Some(saved) = saved.as_ref() {
        if !saved.refresh_token.trim().is_empty() {
            println!("Reconnexion de « {username} » avec la session Lapinux…");
            if let Some(session) = sign_in_token(username, &saved.refresh_token) {
                return finish_cli_login(&paths, username, session);
            }
            println!("jeton refusé : mot de passe demandé.");
        }
    }

    let password = match read_secret("Mot de passe Steam : ") {
        Ok(value) if !value.is_empty() => value,
        Ok(_) => {
            eprintln!("mot de passe vide");
            return 2;
        }
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };
    let auth = lapinux_steam::cm::Auth::Account {
        name: username.to_string(),
        password,
        guard_data: saved.and_then(|saved| saved.guard_data),
    };
    let config = lapinux_steam::cm::Config::from_env();
    let mut prompt = |kind, retry| {
        let label = match kind {
            lapinux_steam::cm::Guard::EmailCode => "Code Steam Guard reçu par e-mail : ",
            lapinux_steam::cm::Guard::TwoFactor => "Code Steam Guard de l'application : ",
            lapinux_steam::cm::Guard::DeviceConfirmation => {
                println!("Confirmez la connexion dans l'application Steam…");
                return Ok(String::new());
            }
            lapinux_steam::cm::Guard::EmailConfirmation => {
                println!("Confirmez la connexion via le lien envoyé par e-mail…");
                return Ok(String::new());
            }
        };
        if retry {
            println!("Code refusé, réessayez.");
        }
        read_line(label)
    };
    match lapinux_steam::cm::sign_in(&config, &auth, &mut prompt) {
        Ok(session) => finish_cli_login(&paths, username, session),
        Err(err) => {
            eprintln!("connexion Lapinux impossible : {err}");
            1
        }
    }
}

fn sign_in_token(username: &str, token: &str) -> Option<lapinux_steam::cm::Session> {
    let config = lapinux_steam::cm::Config::from_env();
    let auth = lapinux_steam::cm::Auth::Token {
        name: username.to_string(),
        refresh_token: token.to_string(),
    };
    let mut prompt = |_, _| Err(lapinux_steam::cm::Error::Auth("aucun code attendu".into()));
    lapinux_steam::cm::sign_in(&config, &auth, &mut prompt).ok()
}

fn finish_cli_login(
    paths: &LapinuxPaths,
    username: &str,
    session: lapinux_steam::cm::Session,
) -> i32 {
    let Some(refresh_token) = session.refresh_token.clone() else {
        eprintln!("Steam n'a pas rendu de jeton réutilisable");
        return 1;
    };
    let saved = lapinux_steam::session::SavedSession {
        account_name: username.to_string(),
        steam_id: session.steam_id,
        refresh_token,
        guard_data: session.guard_data.clone(),
        // La date d'écriture est posée par `save_to`.
        saved_at: None,
    };
    if let Err(err) =
        lapinux_steam::session::save_to(&lapinux_steam::session::path_in(&paths.data_dir), &saved)
    {
        eprintln!("jeton non enregistré : {err}");
        return 1;
    }
    if let Ok(mut config) = Config::load_or_create(paths) {
        config.steam.username = username.to_string();
        let _ = config.save(&paths.config_file);
    }
    println!("connexion réussie — session Lapinux enregistrée (aucun mot de passe conservé)");
    0
}

fn read_secret(prompt: &str) -> Result<String, String> {
    use std::io::Write;
    print!("{prompt}");
    std::io::stdout().flush().map_err(|err| err.to_string())?;
    let hidden = std::process::Command::new("stty")
        .arg("-echo")
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    let mut line = String::new();
    let read = std::io::stdin().read_line(&mut line);
    if hidden {
        let _ = std::process::Command::new("stty").arg("echo").status();
        println!();
    }
    read.map_err(|err| format!("lecture du mot de passe : {err}"))?;
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

fn read_line(prompt: &str) -> Result<String, lapinux_steam::cm::Error> {
    use std::io::Write;
    print!("{prompt}");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|err| lapinux_steam::cm::Error::Auth(format!("lecture : {err}")))?;
    Ok(line.trim().to_string())
}

fn client() -> Client {
    Client::new(&paths())
}

fn install(appid: Option<&String>) -> i32 {
    let Some(appid) = appid.and_then(|s| s.parse::<u32>().ok()) else {
        eprintln!("usage : {} cli install <appid>", lapinux_core::APP_BIN);
        return 2;
    };
    let client = client();
    let envelope = match client.request(&Req::InstallGame {
        appid,
        dlc: Vec::new(),
        // Sans précision, le moteur applique sa préférence (Windows sous
        // Proton, repli Linux) : la ligne de commande ne choisit pas de
        // version, contrairement au menu de la fiche.
        platform: None,
    }) {
        Ok(envelope) => envelope,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };
    if !envelope.ok {
        eprintln!("{}", envelope.error.unwrap_or_default());
        return 1;
    }
    println!("tâche créée, suivi de la progression (Ctrl+C pour quitter)");
    let mut last = String::new();
    loop {
        std::thread::sleep(Duration::from_millis(600));
        let jobs = match client.request(&Req::Jobs) {
            Ok(envelope) => envelope
                .into_data::<Vec<lapinux_core::model::Job>>()
                .unwrap_or_default(),
            Err(err) => {
                eprintln!("{err}");
                return 1;
            }
        };
        if let Some(job) = jobs.iter().filter(|j| j.appid == Some(appid)).next_back() {
            let line = format!(
                "{:>3.0}%  {:<60}",
                job.progress * 100.0,
                lapinux_core::util::ellipsize(&job.message, 60)
            );
            if line != last {
                println!("{line}");
                last = line;
            }
            match job.state {
                JobState::Done => {
                    println!("terminé : {}", job.message);
                    return 0;
                }
                JobState::Failed => {
                    eprintln!("échec : {}", job.message);
                    return 1;
                }
                JobState::Cancelled => {
                    eprintln!("annulé");
                    return 1;
                }
                _ => {}
            }
        }
    }
}

fn launch(appid: Option<&String>, dry_run: bool) -> i32 {
    let Some(appid) = appid.and_then(|s| s.parse::<u32>().ok()) else {
        eprintln!(
            "usage : {} cli launch <appid> [--dry-run]",
            lapinux_core::APP_BIN
        );
        return 2;
    };
    let client = client();
    let envelope = match client.request(&Req::Launch { appid, dry_run }) {
        Ok(envelope) => envelope,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };
    if !envelope.ok {
        eprintln!("{}", envelope.error.unwrap_or_default());
        return 1;
    }
    if dry_run {
        let data = envelope.data;
        println!("couches : {}", data["layers"].as_str().unwrap_or("?"));
        println!("proton  : {}", data["proton"].as_str().unwrap_or("natif"));
        println!("journal : {}", data["log"].as_str().unwrap_or("?"));
        println!("\ncommande :\n{}", data["command"].as_str().unwrap_or(""));
    } else {
        println!(
            "session démarrée : {}",
            serde_json::to_string_pretty(&envelope.data).unwrap_or_default()
        );
    }
    0
}

fn jobs() -> i32 {
    let client = client();
    match client
        .request(&Req::Jobs)
        .and_then(Envelope::into_data::<Vec<lapinux_core::model::Job>>)
    {
        Ok(jobs) if jobs.is_empty() => println!("aucune tâche"),
        Ok(jobs) => {
            for job in jobs {
                println!(
                    "{:<10} {:>5.1}%  {:<62} {}",
                    job.state.label(),
                    job.progress * 100.0,
                    lapinux_core::util::ellipsize(&job.label, 62),
                    job.message
                );
            }
        }
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    }
    0
}

fn session() -> i32 {
    let client = client();
    match client
        .request(&Req::Session)
        .and_then(Envelope::into_data::<Option<lapinux_core::model::SessionInfo>>)
    {
        Ok(Some(session)) => {
            println!(
                "{} (appid {}) pid {} depuis {} s — Proton {} · overlay {}",
                session.name,
                session.appid,
                session.pid,
                lapinux_core::util::now_unix() - session.started,
                session.proton,
                if session.overlay_open {
                    "ouvert"
                } else {
                    "fermé"
                }
            );
        }
        Ok(None) => println!("aucune session"),
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    }
    0
}

fn stop() -> i32 {
    let client = client();
    match client.request(&Req::Stop) {
        Ok(envelope) if envelope.ok => {
            println!("session arrêtée");
            0
        }
        Ok(envelope) => {
            eprintln!("{}", envelope.error.unwrap_or_default());
            1
        }
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}

/// La clé SteamGridDB ne doit jamais apparaître en clair dans le terminal.
fn masked_config(mut config: lapinux_core::Config) -> lapinux_core::Config {
    if !config.artwork.steamgriddb_key.trim().is_empty() {
        config.artwork.steamgriddb_key = "********".into();
    }
    config
}

fn print_config() -> i32 {
    let paths = paths();
    if daemon_up() {
        if let Ok(envelope) = Client::new(&paths).without_spawn().request(&Req::Config) {
            if let Ok(config) = envelope.into_data::<lapinux_core::Config>() {
                println!("# {}", paths.config_file.display());
                println!(
                    "{}",
                    toml::to_string_pretty(&masked_config(config)).unwrap_or_default()
                );
                return 0;
            }
        }
    }
    match lapinux_core::Config::load(&paths.config_file) {
        Ok(config) => {
            println!("# {}", paths.config_file.display());
            println!(
                "{}",
                toml::to_string_pretty(&masked_config(config)).unwrap_or_default()
            );
            0
        }
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}

fn overlay_demo(args: &[String]) -> i32 {
    apply_overlay_language();
    let seconds = arg_value(args, "--seconds")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(3);
    let mut menu = lapinux_overlay::MenuState::new("Démonstration");
    menu.anim = 1.0;
    menu.fps_cap = 120;
    menu.profile = "defaut".into();
    menu.profiles = vec!["defaut".into(), "Manette Xbox".into(), "Manette PS5".into()];
    menu.proton = "GE-Proton".into();
    let options = lapinux_overlay::OverlayOptions {
        width: arg_value(args, "--width")
            .and_then(|s| s.parse().ok())
            .unwrap_or(1920),
        height: arg_value(args, "--height")
            .and_then(|s| s.parse().ok())
            .unwrap_or(1080),
        duration: Some(Duration::from_secs(seconds)),
        font: None,
        capsule: arg_value(args, "--capsule").map(std::path::PathBuf::from),
        icon: arg_value(args, "--icon").map(std::path::PathBuf::from),
        nav_fifo: None,
        hud_only: false,
        output: None,
        results: None,
    };
    match lapinux_overlay::run(menu, options, |_action| {}) {
        Ok(()) => {
            println!("overlay affiché {seconds} s");
            0
        }
        Err(err) => {
            eprintln!("overlay : {err}");
            1
        }
    }
}

/// Réglages continus dont seul le dernier pas compte : pendant un glissement,
/// le pointeur produit une action par pixel. Les envoyer toutes ferait la
/// queue devant `wpctl` et donnerait un curseur pâteux.
fn coalescible_stream(wire: &str) -> Option<&'static str> {
    ["fps_cap:", "game_volume:", "hud_scale:", "hud_style:"]
        .into_iter()
        .find(|prefix| wire.starts_with(prefix))
}

/// Transmet une action du menu au daemon, depuis le fil des actions.
/// Renvoie le motif du refus pour que l'overlay puisse réarmer son bouton.
fn send_overlay_action(client: &Client, wire: &str) -> Option<String> {
    match client.request(&Req::OverlayAction {
        action: wire.to_string(),
    }) {
        Ok(envelope) if !envelope.ok => {
            let error = envelope
                .error
                .unwrap_or_else(|| "le daemon a refusé l'action".into());
            eprintln!("action refusée : {error}");
            Some(error)
        }
        Ok(_) => None,
        Err(err) => {
            eprintln!("action : {err}");
            Some(err.to_string())
        }
    }
}

/// Bandeau de succès demandé par le daemon : une carte posée en haut de
/// l'écran, sans menu, sans clavier et sans toucher aux panneaux du bureau.
fn achievement_toast(args: &[String]) -> i32 {
    let store = match arg_value(args, "--toast-store").as_deref() {
        Some("epic") => lapinux_core::stores::Store::Epic,
        Some("gog") => lapinux_core::stores::Store::Gog,
        Some("itch") => lapinux_core::stores::Store::Itch,
        _ => lapinux_core::stores::Store::Steam,
    };
    let options = lapinux_overlay::ToastOptions {
        title: arg_value(args, "--toast-title").unwrap_or_else(|| "Succès débloqué".into()),
        game: arg_value(args, "--toast-game").unwrap_or_default(),
        store,
        percent: arg_value(args, "--toast-percent").and_then(|value| value.parse().ok()),
        icon: arg_value(args, "--toast-icon").map(std::path::PathBuf::from),
        game_icon: arg_value(args, "--toast-game-icon").map(std::path::PathBuf::from),
        hold: Duration::from_secs(
            arg_value(args, "--toast-hold")
                .and_then(|value| value.parse().ok())
                .unwrap_or(6),
        ),
        font: None,
        output: arg_value(args, "--output"),
    };
    match lapinux_overlay::run_toast(options) {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("bandeau de succès : {err}");
            1
        }
    }
}

/// Aligne la langue de l'overlay sur celle enregistrée par l'application :
/// ce sous-processus ne lit pas la configuration pour lui-même.
fn apply_overlay_language() {
    let paths = LapinuxPaths::discover();
    if let Ok(config) = lapinux_core::Config::load(&paths.config_file) {
        lapinux_core::i18n::set(&config.general.lang);
    }
}

pub fn overlay(args: &[String]) -> i32 {
    apply_overlay_language();
    // Bandeau de succès : une surface jetable, sans menu ni raccourcis.
    if args.iter().any(|a| a == "--achievement-toast") {
        return achievement_toast(args);
    }
    let game = arg_value(args, "--game").unwrap_or_else(|| "Jeu".into());
    let mut menu = lapinux_overlay::MenuState::new(game);
    menu.anim = 0.0;
    menu.fps_cap = arg_value(args, "--fps")
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);
    // Plaque de performance de Lapinux : elle vit dans la surface de
    // l'overlay, jamais dans celle du jeu. `--hud-only` ne l'allume pas : le
    // démon peut demander un badge d'enregistrement seul.
    menu.perf_hud = args.iter().any(|a| a == "--perf-hud");
    menu.record_badge = args.iter().any(|a| a == "--record-badge");
    // Le style complet arrive en JSON : il porte les couleurs, l'ordre, les
    // unités, l'ancrage et le badge. Sans lui, un style intégré sert de repli.
    menu.hud_style = arg_value(args, "--hud-style")
        .and_then(|body| serde_json::from_str::<lapinux_core::hud::HudStyle>(&body).ok())
        .unwrap_or_else(|| lapinux_core::hud::HudStyle::builtin("essential"));
    menu.hud_style.normalize();
    if let Some(position) = arg_value(args, "--record-badge-position") {
        menu.hud_style.badge.position = position;
    }
    if let Some(scale) =
        arg_value(args, "--record-badge-scale").and_then(|value| value.parse::<f32>().ok())
    {
        menu.hud_style.badge.scale = scale;
    }
    menu.hud_style.badge.enabled = menu.hud_style.badge.enabled || menu.record_badge;
    menu.record_badge = menu.hud_style.badge.enabled;
    menu.record_position =
        lapinux_overlay::menu::HudPosition::from_wire(&menu.hud_style.badge.position);
    menu.record_scale = menu.hud_style.badge.scale;
    menu.perf_scale = menu.hud_style.scale;
    menu.hud_position = lapinux_overlay::menu::HudPosition::from_wire(&menu.hud_style.position);
    menu.hud_layout = lapinux_overlay::menu::HudLayout::from_wire(&menu.hud_style.layout);
    menu.hud_styles = arg_value(args, "--hud-styles")
        .and_then(|body| serde_json::from_str::<Vec<lapinux_core::hud::HudStyle>>(&body).ok())
        .unwrap_or_default();
    for style in &mut menu.hud_styles {
        style.normalize();
    }
    // Vidéos du jeu pour la page « Enregistrements » : le dossier vient du
    // démon, le jeu de son appid.
    menu.appid = arg_value(args, "--appid")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    menu.recordings_dir = arg_value(args, "--captures").map(std::path::PathBuf::from);
    menu.profile = arg_value(args, "--profile").unwrap_or_else(|| "defaut".into());
    // Tous les profils arrivent en une seule valeur, séparés par des sauts de
    // ligne : un nom de profil peut contenir espaces et ponctuation, mais pas
    // de saut de ligne.
    menu.profiles = arg_value(args, "--profiles")
        .map(|list| {
            list.split('\n')
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if menu.profiles.is_empty() {
        menu.profiles.push(menu.profile.clone());
    }
    menu.profile_selected = menu
        .profiles
        .iter()
        .position(|name| *name == menu.profile)
        .unwrap_or(0);
    menu.proton = arg_value(args, "--proton").unwrap_or_else(|| "auto".into());
    let width = arg_value(args, "--width")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1920);
    let height = arg_value(args, "--height")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1080);
    let paths = LapinuxPaths::discover();
    // Les actions partent par un fil dédié : le menu ne doit jamais attendre
    // le daemon. Pendant un glissement de la limite de FPS, seule la dernière
    // valeur demandée compte, les précédentes sont jetées.
    let (sender, actions) = std::sync::mpsc::channel::<String>();
    // Les échecs reviennent au panneau par ce canal : le fil des actions ne
    // bloque jamais le menu, mais l'invitation refusée est réarmée dès que le
    // daemon a répondu.
    let (failures, failed_actions) = std::sync::mpsc::channel::<lapinux_overlay::ActionFailed>();
    {
        let paths = paths.clone();
        std::thread::spawn(move || {
            let client = Client::new(&paths).without_spawn();
            while let Ok(wire) = actions.recv() {
                let mut latest = wire;
                // Tant que la file ne contient que des pas du même réglage,
                // on garde le dernier et on jette les intermédiaires.
                while let Some(stream) = coalescible_stream(&latest) {
                    match actions.try_recv() {
                        Ok(next) if coalescible_stream(&next) == Some(stream) => latest = next,
                        Ok(next) => {
                            if let Some(error) = send_overlay_action(&client, &latest) {
                                let _ = failures.send(lapinux_overlay::ActionFailed {
                                    action: latest.clone(),
                                    error,
                                });
                            }
                            latest = next;
                            break;
                        }
                        Err(_) => break,
                    }
                }
                if let Some(error) = send_overlay_action(&client, &latest) {
                    let _ = failures.send(lapinux_overlay::ActionFailed {
                        action: latest,
                        error,
                    });
                }
            }
        });
    }
    let options = lapinux_overlay::OverlayOptions {
        width,
        height,
        duration: arg_value(args, "--duration")
            .and_then(|s| s.parse().ok())
            .map(Duration::from_secs),
        font: None,
        capsule: arg_value(args, "--capsule").map(std::path::PathBuf::from),
        icon: arg_value(args, "--icon").map(std::path::PathBuf::from),
        nav_fifo: arg_value(args, "--nav-fifo").map(std::path::PathBuf::from),
        hud_only: args.iter().any(|a| a == "--hud-only"),
        output: arg_value(args, "--output"),
        results: Some(failed_actions),
    };
    let quit_paths = paths.clone();
    let result = lapinux_overlay::run(menu, options, move |action| {
        let wire = action.wire();
        if wire == "resume" {
            return;
        }
        // « Quitter » ferme le processus juste après l'animation. Confier
        // l'ordre au fil détaché laissait une course : le processus pouvait
        // se terminer avant que le fil n'ait envoyé la requête, et le jeu
        // restait ouvert jusqu'au clic suivant. L'ordre part donc ici, avant
        // que `run` ne rende la main, avec un délai borné pour ne jamais
        // bloquer la fermeture si le daemon ne répond pas.
        if wire == "quit_game" {
            let client = Client::new(&quit_paths).without_spawn();
            let _ = client
                .request_with_timeout(&Req::OverlayAction { action: wire }, Duration::from_secs(2));
            return;
        }
        let _ = sender.send(wire);
    });
    match result {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("overlay : {err}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn les_glissements_ne_gardent_que_leur_derniere_valeur() {
        assert_eq!(coalescible_stream("game_volume:0.42"), Some("game_volume:"));
        assert_eq!(coalescible_stream("fps_cap:120"), Some("fps_cap:"));
        assert_eq!(coalescible_stream("hud_scale:1.25"), Some("hud_scale:"));
        assert_eq!(
            coalescible_stream("hud_style:{\"name\":\"Course\"}"),
            Some("hud_style:")
        );
        // Une action ponctuelle n'est jamais absorbée par un glissement.
        assert_eq!(coalescible_stream("screenshot"), None);
        assert_eq!(coalescible_stream("game_volume:0.42"), Some("game_volume:"));
    }

    #[test]
    fn la_cle_steamgriddb_est_masquee_a_l_affichage() {
        let mut config = lapinux_core::Config::default();
        config.artwork.steamgriddb_key = "clef-secrete".into();
        let masked = masked_config(config);
        assert_eq!(masked.artwork.steamgriddb_key, "********");
        assert!(toml::to_string_pretty(&masked)
            .unwrap()
            .contains("********"));
        assert!(!toml::to_string_pretty(&masked)
            .unwrap()
            .contains("clef-secrete"));
    }
}
