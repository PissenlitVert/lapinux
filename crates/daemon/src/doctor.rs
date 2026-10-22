//! Diagnostic de l'environnement, affiché dans l'UI et via `ludus cli doctor`.

use crate::Shared;
use ludus_core::model::{Check, CheckStatus, DoctorReport};
use ludus_core::paths;

fn check(name: &str, status: CheckStatus, detail: impl Into<String>) -> Check {
    Check {
        name: name.to_string(),
        status,
        detail: detail.into(),
    }
}

pub fn report(shared: &Shared) -> DoctorReport {
    let mut checks = Vec::new();

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

    let library_root = shared.config_snapshot().library_root();
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
            format!("{} sera créé au premier téléchargement", library_root.display()),
        )
    });

    checks.push(if paths::which("umu-run").is_some() {
        check("umu-run", CheckStatus::Ok, "lancement Proton via umu disponible")
    } else {
        check(
            "umu-run",
            CheckStatus::Warn,
            "absent : repli sur `proton run` direct",
        )
    });

    checks.push(if paths::which("gamescope").is_some() {
        check("gamescope", CheckStatus::Ok, "compositeur imbriqué disponible")
    } else {
        check("gamescope", CheckStatus::Warn, "absent : lancement sans gamescope")
    });

    checks.push(if paths::which("mangohud").is_some() {
        check("MangoHud", CheckStatus::Ok, "overlay de performance disponible")
    } else {
        check("MangoHud", CheckStatus::Warn, "absent : HUD désactivé")
    });

    checks.push(match paths::which("gamemoderun") {
        Some(_) => check("GameMode", CheckStatus::Ok, "gamemoderun détecté"),
        None => check(
            "GameMode",
            CheckStatus::Warn,
            "absent (rpm-ostree install gamemode, redémarrage requis) : nice/gamescope restent actifs",
        ),
    });

    let recorder = shared.config_snapshot().overlay.recorder;
    checks.push(match paths::which(&recorder) {
        Some(path) => check("Enregistrement", CheckStatus::Ok, path.display().to_string()),
        None => check(
            "Enregistrement",
            CheckStatus::Warn,
            "gpu-screen-recorder absent : bouton d'enregistrement désactivé",
        ),
    });

    checks.push(match paths::which("steamcmd") {
        Some(path) => check("SteamCMD", CheckStatus::Ok, path.display().to_string()),
        None => {
            if shared.steamcmd.available() {
                check(
                    "SteamCMD",
                    CheckStatus::Ok,
                    shared.steamcmd.binary().display().to_string(),
                )
            } else {
                check(
                    "SteamCMD",
                    CheckStatus::Warn,
                    "non installé : nécessaire pour télécharger de nouveaux jeux",
                )
            }
        }
    });

    checks.push(if ludus_steam::has_steam_token(&shared.steamcmd.root) {
        check("Connexion Steam", CheckStatus::Ok, "jeton SteamCMD présent")
    } else {
        check(
            "Connexion Steam",
            CheckStatus::Warn,
            "connectez-vous avec « ludus cli login <compte> »",
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

    let devices = ludus_input::device::list();
    let pads = devices.iter().filter(|d| d.gamepad).count();
    checks.push(check(
        "Manettes",
        if pads > 0 {
            CheckStatus::Ok
        } else {
            CheckStatus::Warn
        },
        format!("{pads} manette(s) détectée(s) sur {} périphérique(s) evdev", devices.len()),
    ));

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
