//! Diagnostic de l'environnement, affiché dans l'UI et via `lapinux cli doctor`.

use crate::Shared;
use lapinux_core::model::{Check, CheckStatus, DoctorReport};
use lapinux_core::paths;
use lapinux_input::shortcut;

fn daemon_alive(socket: &std::path::Path) -> bool {
    lapinux_core::ipc::send_request_timeout(
        socket,
        &lapinux_core::ipc::Req::Ping,
        std::time::Duration::from_millis(200),
    )
    .is_ok()
}

fn check(name: &str, status: CheckStatus, detail: impl Into<String>) -> Check {
    Check {
        name: name.to_string(),
        status,
        detail: detail.into(),
    }
}

pub fn report(shared: &Shared) -> DoctorReport {
    let mut checks = Vec::new();
    // Le fichier fait foi : une modification à la main doit être diagnostiquée
    // telle qu'elle sera relue au prochain lancement, pas telle qu'elle était
    // en mémoire au démarrage du daemon.
    let config = lapinux_core::Config::load(&shared.paths.config_file)
        .unwrap_or_else(|_| shared.config_snapshot());

    let roots = paths::steam_roots();
    checks.push(if roots.is_empty() {
        check(
            "Steam",
            CheckStatus::Warn,
            "aucune installation Steam détectée : les jeux déjà installés ne seront pas listés",
        )
    } else {
        check(
            "Steam",
            CheckStatus::Ok,
            format!(
                "{} racine(s) : {}",
                roots.len(),
                roots
                    .iter()
                    .map(|r| r.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )
    });

    let library_root = config.library_root();
    checks.push(if library_root.exists() {
        check(
            "Bibliothèque",
            CheckStatus::Ok,
            library_root.display().to_string(),
        )
    } else {
        check(
            "Bibliothèque",
            CheckStatus::Warn,
            format!(
                "{} sera créé au premier téléchargement",
                library_root.display()
            ),
        )
    });

    checks.push(if paths::which("umu-run").is_some() {
        check(
            "umu-run",
            CheckStatus::Ok,
            "lancement Proton via umu disponible",
        )
    } else {
        check(
            "umu-run",
            CheckStatus::Warn,
            "absent : repli sur `proton run` direct",
        )
    });

    checks.push(if paths::which("gamescope").is_some() {
        check(
            "GameScope",
            CheckStatus::Ok,
            "compositeur imbriqué disponible",
        )
    } else {
        check(
            "GameScope",
            CheckStatus::Warn,
            "absent : lancement sans GameScope",
        )
    });

    let steam = shared.steam_status();
    checks.push(if steam.token_saved {
        check(
            "Connexion Steam",
            CheckStatus::Ok,
            "session Lapinux enregistrée : aucun client Steam n'est lancé",
        )
    } else {
        check(
            "Connexion Steam",
            CheckStatus::Warn,
            "connectez votre compte dans Réglages → Compte Steam (le mot de passe n'est jamais stocké)",
        )
    });

    let uinput = std::path::Path::new("/dev/uinput");
    checks.push(if uinput.exists() {
        let writable = std::fs::OpenOptions::new().write(true).open(uinput).is_ok();
        if writable {
            check("Pad virtuel", CheckStatus::Ok, "/dev/uinput accessible")
        } else {
            check(
                "Pad virtuel",
                CheckStatus::Warn,
                "/dev/uinput présent mais non accessible en écriture",
            )
        }
    } else {
        check("Pad virtuel", CheckStatus::Warn, "/dev/uinput absent")
    });

    let access = lapinux_input::access::audit();
    let chord = config.overlay.hotkey.trim();
    checks.push(match shortcut::state() {
        Some(shortcut::State::Active(chord)) => check(
            "Raccourci clavier",
            CheckStatus::Ok,
            format!("{chord} actif via KWin (aucune permission /dev/input requise)"),
        ),
        Some(shortcut::State::Ready) if shortcut::kwin_available() => check(
            "Raccourci clavier",
            CheckStatus::Ok,
            format!(
                "KWin répond : « {chord} » sera enregistré au lancement d'un jeu ; aucune permission /dev/input nécessaire"
            ),
        ),
        Some(shortcut::State::Ready) => check(
            "Raccourci clavier",
            CheckStatus::Fail,
            format!(
                "KWin ne répond pas : « {chord} » ne peut pas être capturé. Le raccourci passe par un script KWin, disponible sous Plasma Wayland."
            ),
        ),
        None if daemon_alive(&shared.paths.socket) => check(
            "Raccourci clavier",
            CheckStatus::Fail,
            format!(
                "le daemon ne sert pas le raccourci (version antérieure ?) : « {chord} » ne s'ouvrira pas"
            ),
        ),
        None => check(
            "Raccourci clavier",
            CheckStatus::Warn,
            format!("daemon arrêté : « {chord} » sera servi par KWin au démarrage"),
        ),
    });

    let devices = lapinux_input::device::list();
    let pads = devices.iter().filter(|d| d.gamepad).count();
    let denied_pads: Vec<_> = access.iter().filter(|d| !d.readable && d.gamepad).collect();
    checks.push(if let Some(device) = denied_pads.first() {
        check(
            "Manettes",
            CheckStatus::Warn,
            format!(
                "{} ({}) est illisible : {}",
                device.name,
                device.path.display(),
                device.error.as_deref().unwrap_or("permission refusée")
            ),
        )
    } else if pads > 0 {
        check(
            "Manettes",
            CheckStatus::Ok,
            format!(
                "{pads} manette(s) lisible(s) sur {} périphérique(s) evdev",
                devices.len()
            ),
        )
    } else {
        check("Manettes", CheckStatus::Warn, "aucune manette détectée")
    });

    let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_default();
    checks.push(if session == "wayland" {
        check("Session", CheckStatus::Ok, "Wayland (overlay layer-shell)")
    } else {
        check(
            "Session",
            CheckStatus::Warn,
            format!("{session} : l'overlay Wayland peut être indisponible"),
        )
    });

    let mut report = DoctorReport { checks };
    report.checks.sort_by_key(|c| match c.status {
        CheckStatus::Fail => 0,
        CheckStatus::Warn => 1,
        CheckStatus::Ok => 2,
    });
    report
}
