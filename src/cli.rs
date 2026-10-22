//! Interface en ligne de commande de Ludus.

use ludus_core::client::Client;
use ludus_core::ipc::{Envelope, Req};
use ludus_core::model::{Drm, Game, JobState};
use ludus_core::paths::LudusPaths;
use ludus_core::store::Store;
use std::time::Duration;

pub fn run(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        Some("doctor") => doctor(),
        Some("scan") => scan(args.iter().any(|a| a == "--json")),
        Some("list") => list(args.iter().any(|a| a == "--json")),
        Some("protons") => protons(args.iter().any(|a| a == "--json")),
        Some("releases") => releases(),
        Some("devices") => devices(),
        Some("input-test") => input_test(arg_value(args, "--seconds"), false),
        Some("virtual-pad") => input_test(None, true),
        Some("login") => login(args.get(1).map(String::as_str)),
        Some("tools") => tools(args),
        Some("install") => install(args.get(1)),
        Some("launch") => launch(args.get(1), args.iter().any(|a| a == "--dry-run")),
        Some("jobs") => jobs(),
        Some("session") => session(),
        Some("stop") => stop(),
        Some("config") => print_config(),
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
    println!(
        "Ludus — gestionnaire de jeux\n\
         \n\
         Utilisation :\n\
         \x20 ludus                        interface graphique\n\
         \x20 ludus --daemon               daemon (bibliothèque, téléchargements, overlay)\n\
         \x20 ludus overlay [options]      quick menu en jeu\n\
         \x20 ludus cli <commande>         commandes ci-dessous\n\
         \n\
         Commandes :\n\
         \x20 doctor                       diagnostic de l'environnement\n\
         \x20 scan [--json]                scanne les bibliothèques Steam\n\
         \x20 list [--json]                liste les jeux connus\n\
         \x20 protons [--json]             liste les Proton installés\n\
         \x20 releases                     Proton téléchargeables (GitHub)\n\
         \x20 devices                      manettes et périphériques evdev\n\
         \x20 input-test [--seconds N]     lit les manettes et affiche les actions\n\
         \x20 virtual-pad                  crée le pad virtuel uinput (test)\n\
         \x20 login <compte>               connexion Steam via SteamCMD (2FA)\n\
         \x20 tools                        état des outils (SteamCMD, DepotDownloader)\n\
         \x20 tools install <outil>        télécharge un outil\n\
         \x20 install <appid>              télécharge un jeu possédé\n\
         \x20 launch <appid> [--dry-run]   lance un jeu\n\
         \x20 jobs                         tâches de téléchargement\n\
         \x20 session                      session en cours\n\
         \x20 stop                         arrête la session\n\
         \x20 config                       configuration effective"
    );
}

fn arg_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn paths() -> LudusPaths {
    LudusPaths::discover()
}

fn store(paths: &LudusPaths) -> Result<Store, String> {
    Store::open(&paths.db_file).map_err(|e| e.to_string())
}

fn daemon_up() -> bool {
    Client::new(&paths()).without_spawn().daemon_alive()
}

fn doctor() -> i32 {
    let paths = paths();
    if daemon_up() {
        if let Ok(envelope) = Client::new(&paths).without_spawn().request(&Req::Doctor) {
            if let Ok(report) = envelope.into_data::<ludus_core::model::DoctorReport>() {
                return print_doctor(&report);
            }
        }
    }
    match ludus_daemon::Shared::new(&paths) {
        Ok(shared) => print_doctor(&ludus_daemon::doctor::report(&shared)),
        Err(err) => {
            eprintln!("diagnostic impossible : {err}");
            1
        }
    }
}

fn print_doctor(report: &ludus_core::model::DoctorReport) -> i32 {
    println!("Ludus — diagnostic\n");
    for check in &report.checks {
        let symbol = match check.status {
            ludus_core::model::CheckStatus::Ok => "OK  ",
            ludus_core::model::CheckStatus::Warn => "ATTN",
            ludus_core::model::CheckStatus::Fail => "ERR ",
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
        let libraries = ludus_core::paths::steam_roots();
        return print_scan(json, &games, &libraries, &[]);
    }
    let store = match store(&paths) {
        Ok(store) => store,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };
    let roots = ludus_core::paths::steam_roots();
    let report = ludus_steam::scan(&roots);
    let libraries = report.libraries.clone();
    let warnings = report.warnings.clone();
    let stored = store.load_games().unwrap_or_default();
    let games = ludus_steam::merge_existing(report.games, &stored);
    if let Err(err) = store.save_games(&games) {
        eprintln!("sauvegarde : {err}");
        return 1;
    }
    print_scan(json, &games, &libraries, &warnings)
}

fn print_scan(
    json: bool,
    games: &[Game],
    libraries: &[std::path::PathBuf],
    warnings: &[String],
) -> i32 {
    if json {
        println!("{}", serde_json::to_string_pretty(games).unwrap_or_default());
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
                ludus_core::util::ellipsize(&game.name, 42),
                if game.installed { "installé" } else { "absent" },
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
            Ok(envelope) => envelope
                .into_data::<Vec<Game>>()
                .unwrap_or_default(),
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
        println!("{}", serde_json::to_string_pretty(&games).unwrap_or_default());
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
        ludus_core::paths::home_dir().join(".local/share/umu/compatibilitytools"),
    ];
    let list = ludus_proton::scan(&ludus_core::paths::steam_roots(), &extra);
    if json {
        println!("{}", serde_json::to_string_pretty(&list).unwrap_or_default());
    } else if list.is_empty() {
        println!("aucun Proton détecté");
    } else {
        let recommended = ludus_proton::recommended(&list).map(|p| p.name.clone());
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

fn releases() -> i32 {
    for source in ludus_proton::SOURCES {
        match ludus_proton::fetch_releases(source.repo) {
            Ok(list) => {
                println!("\n{} ({})", source.label, source.repo);
                for release in list.iter().take(6) {
                    println!(
                        "  {:<58} {:>6} Mio  {}",
                        release.asset,
                        release.size / 1024 / 1024,
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
    for device in ludus_input::device::list() {
        println!(
            "{} {:<44} {:04x}:{:04x} {} boutons {}",
            if device.gamepad { "MANETTE" } else { "PÉRIF  " },
            ludus_core::util::ellipsize(&device.name, 44),
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

fn input_test(seconds: Option<String>, virtual_pad_only: bool) -> i32 {
    if virtual_pad_only {
        return virtual_pad_check();
    }
    let seconds = seconds
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(6);
    let paths = paths();
    let profile = ludus_input::ensure_profiles(&paths.data_dir, "defaut").unwrap_or_default();
    println!(
        "Manettes détectées : {}",
        ludus_input::device::list_gamepads().len()
    );
    println!(
        "Lecture pendant {seconds} s (appuyez sur des boutons ; l'overlay est simulé par le raccourci)…"
    );
    let service = match ludus_input::InputService::start(profile, false) {
        Ok(service) => service,
        Err(err) => {
            eprintln!("service d'entrée : {err}");
            return 1;
        }
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(seconds);
    while std::time::Instant::now() < deadline {
        match service.events.recv_timeout(Duration::from_millis(200)) {
            Ok(ludus_input::InputMsg::Hotkey(source)) => {
                println!("raccourci overlay ({source:?})");
            }
            Ok(ludus_input::InputMsg::DevicesChanged) => println!("périphériques mis à jour"),
            Ok(ludus_input::InputMsg::Error(err)) => println!("erreur : {err}"),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => break,
        }
    }
    drop(service);
    println!("fin du test");
    0
}

fn virtual_pad_check() -> i32 {
    match ludus_input::VirtualPad::create("Ludus Virtual Pad (test)") {
        Ok(mut pad) => {
            use ludus_input::mapping::{Kind, VEvent};
            let events = vec![
                VEvent {
                    kind: Kind::Key,
                    code: ludus_input::codes::BTN_SOUTH,
                    value: 1,
                },
                VEvent {
                    kind: Kind::Abs,
                    code: ludus_input::codes::ABS_X,
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
                code: ludus_input::codes::BTN_SOUTH,
                value: 0,
            }]);
            println!("pad virtuel « {} » créé et alimenté ; il disparaît à la fin du test", pad.name());
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
        eprintln!("usage : ludus cli login <compte Steam>");
        return 2;
    };
    let paths = paths();
    let steamcmd = ludus_steam::steamcmd::SteamCmd::new(&paths.data_dir);
    if !steamcmd.available() {
        println!("SteamCMD absent, téléchargement…");
        if let Err(err) = steamcmd.ensure(&mut |line| println!("  {line}")) {
            eprintln!("installation de SteamCMD : {err}");
            return 1;
        }
    }
    println!(
        "Connexion Steam pour « {username} » : SteamCMD va demander le mot de passe puis le code 2FA.\n\
         Le mot de passe n'est jamais stocké par Ludus ; seul le jeton SteamCMD le sera."
    );
    match steamcmd.login_interactive(username) {
        Ok(()) => {
            println!("connexion réussie — jeton SteamCMD enregistré");
            0
        }
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}

fn tools(args: &[String]) -> i32 {
    let paths = paths();
    let steamcmd = ludus_steam::steamcmd::SteamCmd::new(&paths.data_dir);
    let depot = ludus_steam::depotdownloader::DepotDownloader::new(&paths.data_dir);
    match args.get(1).map(String::as_str) {
        Some("install") => {
            let Some(tool) = args.get(2).map(String::as_str) else {
                eprintln!("usage : ludus cli tools install <steamcmd|depotdownloader>");
                return 2;
            };
            let mut line = |text: &str| println!("  {text}");
            let result = match tool {
                "steamcmd" => steamcmd.ensure(&mut line),
                "depotdownloader" => depot.ensure(&mut line),
                other => {
                    eprintln!("outil inconnu : {other}");
                    return 2;
                }
            };
            match result {
                Ok(()) => {
                    println!("{tool} prêt");
                    0
                }
                Err(err) => {
                    eprintln!("{tool} : {err}");
                    1
                }
            }
        }
        _ => {
            println!("SteamCMD        : {}", steamcmd.binary().display());
            println!("  installé      : {}", steamcmd.available());
            println!(
                "  jeton Steam   : {}",
                ludus_steam::has_steam_token(&steamcmd.root)
            );
            println!("DepotDownloader : {}", depot.binary().display());
            println!("  installé      : {}", depot.available());
            0
        }
    }
}

fn client() -> Client {
    Client::new(&paths())
}

fn install(appid: Option<&String>) -> i32 {
    let Some(appid) = appid.and_then(|s| s.parse::<u32>().ok()) else {
        eprintln!("usage : ludus cli install <appid>");
        return 2;
    };
    let client = client();
    let envelope = match client.request(&Req::InstallGame { appid }) {
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
            Ok(envelope) => envelope.into_data::<Vec<ludus_core::model::Job>>().unwrap_or_default(),
            Err(err) => {
                eprintln!("{err}");
                return 1;
            }
        };
        if let Some(job) = jobs.iter().filter(|j| j.appid == Some(appid)).next_back() {
            let line = format!(
                "{:>3.0}%  {:<60}",
                job.progress * 100.0,
                ludus_core::util::ellipsize(&job.message, 60)
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
        eprintln!("usage : ludus cli launch <appid> [--dry-run]");
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
    match client.request(&Req::Jobs).and_then(Envelope::into_data::<Vec<ludus_core::model::Job>>) {
        Ok(jobs) if jobs.is_empty() => println!("aucune tâche"),
        Ok(jobs) => {
            for job in jobs {
                println!(
                    "{:<10} {:>5.1}%  {:<62} {}",
                    job.state.label(),
                    job.progress * 100.0,
                    ludus_core::util::ellipsize(&job.label, 62),
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
    match client.request(&Req::Session).and_then(Envelope::into_data::<Option<ludus_core::model::SessionInfo>>) {
        Ok(Some(session)) => {
            println!(
                "{} (appid {}) pid {} depuis {} s — Proton {} · overlay {}",
                session.name,
                session.appid,
                session.pid,
                ludus_core::util::now_unix() - session.started,
                session.proton,
                if session.overlay_open { "ouvert" } else { "fermé" }
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

fn print_config() -> i32 {
    let paths = paths();
    if daemon_up() {
        if let Ok(envelope) = Client::new(&paths).without_spawn().request(&Req::Config) {
            if let Ok(config) = envelope.into_data::<ludus_core::Config>() {
                println!("# {}", paths.config_file.display());
                println!("{}", toml::to_string_pretty(&config).unwrap_or_default());
                return 0;
            }
        }
    }
    match ludus_core::Config::load(&paths.config_file) {
        Ok(config) => {
            println!("# {}", paths.config_file.display());
            println!("{}", toml::to_string_pretty(&config).unwrap_or_default());
            0
        }
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}

fn overlay_demo(args: &[String]) -> i32 {
    let seconds = arg_value(args, "--seconds")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(3);
    let mut menu = ludus_overlay::MenuState::new("Démonstration");
    menu.anim = 1.0;
    menu.fps_cap = 120;
    menu.profile = "defaut".into();
    menu.proton = "GE-Proton".into();
    let options = ludus_overlay::OverlayOptions {
        width: arg_value(args, "--width")
            .and_then(|s| s.parse().ok())
            .unwrap_or(1920),
        height: arg_value(args, "--height")
            .and_then(|s| s.parse().ok())
            .unwrap_or(1080),
        duration: Some(Duration::from_secs(seconds)),
        font: None,
    };
    match ludus_overlay::run(menu, options, |_action| {}) {
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

pub fn overlay(args: &[String]) -> i32 {
    let game = arg_value(args, "--game").unwrap_or_else(|| "Jeu".into());
    let mut menu = ludus_overlay::MenuState::new(game);
    menu.anim = 0.0;
    menu.fps_cap = arg_value(args, "--fps")
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);
    menu.mangohud = args.iter().any(|a| a == "--mangohud");
    menu.profile = arg_value(args, "--profile").unwrap_or_else(|| "defaut".into());
    menu.proton = arg_value(args, "--proton").unwrap_or_else(|| "auto".into());
    menu.recorder_available = args.iter().any(|a| a == "--recorder")
        && ludus_core::paths::which("gpu-screen-recorder").is_some();
    let width = arg_value(args, "--width")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1920);
    let height = arg_value(args, "--height")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1080);
    let paths = LudusPaths::discover();
    let client = Client::new(&paths).without_spawn();
    let options = ludus_overlay::OverlayOptions {
        width,
        height,
        duration: arg_value(args, "--duration").and_then(|s| s.parse().ok()).map(Duration::from_secs),
        font: None,
    };
    let result = ludus_overlay::run(menu, options, move |action| {
        let wire = action.wire();
        if wire == "resume" {
            return;
        }
        match client.request(&Req::OverlayAction { action: wire }) {
            Ok(envelope) if !envelope.ok => {
                eprintln!("action refusée : {}", envelope.error.unwrap_or_default())
            }
            Ok(_) => {}
            Err(err) => eprintln!("action : {err}"),
        }
    });
    match result {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("overlay : {err}");
            1
        }
    }
}

