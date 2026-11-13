//! Diagnostic partageable, construit exclusivement à partir de champs autorisés.
//!
//! Aucun texte libre du daemon, nom de jeu, chemin, journal ou jeton n'est
//! exporté : masquer certains motifs dans un journal ne suffit pas à garantir
//! sa confidentialité.

use crate::model::{DoctorReport, Job, JobKind, JobState};
use serde_json::{json, Value};

/// Rapport structuré sans journal ni données de compte. Les sections absentes
/// sont nulles : une erreur de lecture ne doit pas passer pour une réussite.
pub fn report(doctor: Option<&DoctorReport>, jobs: Option<&[Job]>, daemon: Option<bool>) -> Value {
    let jobs = jobs.map(|jobs| {
        let mut counts = [[0u64; 7]; 3];
        for job in jobs {
            let kind = match job.kind { JobKind::InstallGame => 0, JobKind::InstallProton => 1, JobKind::InstallGraphics => 2 };
            let state = match job.state { JobState::Queued => 0, JobState::Running => 1, JobState::Paused => 2, JobState::Done => 3, JobState::Failed => 4, JobState::Cancelled => 5, JobState::Interrupted => 6 };
            counts[kind][state] += 1;
        }
        let states = |c: &[u64; 7]| json!({"queued":c[0],"running":c[1],"paused":c[2],"done":c[3],"failed":c[4],"cancelled":c[5],"interrupted":c[6]});
        json!({"games":states(&counts[0]),"proton":states(&counts[1]),"graphics":states(&counts[2])})
    });
    json!({
        "format": "lapinux-diagnostic-v1",
        "lapinux_version": crate::VERSION,
        "architecture": std::env::consts::ARCH,
        "operating_system": std::env::consts::OS,
        "environment": environment(),
        "daemon_responding": daemon,
        "checks": doctor.map(DoctorReport::private_summary),
        "downloads": jobs,
        "privacy": {"raw_logs_included": false, "account_data_included": false, "game_names_included": false, "paths_included": false, "hostname_included": false, "network_addresses_included": false},
    })
}

/// Faits techniques de la machine, tous anonymes : noyau, bureau, session,
/// langue, mémoire, processeur et cartes graphiques. Ni nom d'hôte, ni compte,
/// ni chemin, ni adresse réseau n'y entrent.
pub fn environment() -> Value {
    let kernel = read_trimmed("/proc/sys/kernel/osrelease");
    let memory_mib = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|text| {
            text.lines()
                .find_map(|line| line.strip_prefix("MemTotal:"))
                .and_then(|rest| rest.split_whitespace().next())
                .and_then(|kib| kib.parse::<u64>().ok())
                .map(|kib| kib / 1024)
        });
    let cpu = std::fs::read_to_string("/proc/cpuinfo").ok().and_then(|text| {
        text.lines().find_map(|line| {
            line.strip_prefix("model name").map(|rest| {
                rest.trim_start_matches([':', ' ', '\t'])
                    .trim()
                    .to_string()
            })
        })
    });
    let desktop = env_text("XDG_CURRENT_DESKTOP");
    let session = env_text("XDG_SESSION_TYPE").or_else(|| {
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            Some("wayland".to_string())
        } else if std::env::var_os("DISPLAY").is_some() {
            Some("x11".to_string())
        } else {
            None
        }
    });
    let locale = env_text("LC_ALL")
        .or_else(|| env_text("LC_MESSAGES"))
        .or_else(|| env_text("LANG"));
    json!({
        "kernel": kernel,
        "desktop": desktop,
        "session": session,
        "locale": locale,
        "memory_mib": memory_mib,
        "cpu": cpu,
        "gpus": gpus(),
        "packaged_appimage": std::env::var_os("APPIMAGE").is_some(),
    })
}

fn env_text(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn read_trimmed(path: impl AsRef<std::path::Path>) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Cartes graphiques lues dans sysfs : identifiants et pilote, sans numéro de
/// série, adresse PCI complète ni chemin de périphérique.
fn gpus() -> Value {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir("/sys/class/drm") else {
        return json!([]);
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("card") || name.contains('-') {
            continue;
        }
        let device = entry.path().join("device");
        if !device.is_dir() {
            continue;
        }
        let vendor = read_trimmed(device.join("vendor"));
        let model = read_trimmed(device.join("device"));
        let driver = std::fs::read_link(device.join("driver"))
            .ok()
            .and_then(|path| path.file_name().map(|name| name.to_string_lossy().to_string()));
        let key = format!("{vendor:?}/{model:?}/{driver:?}");
        if !seen.insert(key) {
            continue;
        }
        out.push(json!({"vendor_id": vendor, "device_id": model, "driver": driver}));
    }
    json!(out)
}

/// English/French UI copy is kept here so the export does not depend on free
/// text from the diagnostic or interfere with catalog edits in progress.
pub fn label(key: &str) -> &'static str {
    let english = crate::i18n::active() == "en";
    match (key, english) {
        ("export", false) => "Exporter le diagnostic privé",
        ("export", true) => "Export private diagnostic",
        ("copy", false) => "Copier le diagnostic privé",
        ("copy", true) => "Copy private diagnostic",
        ("hint", false) => "Sans journaux, comptes, noms de jeux ni chemins. Ce rapport ne couvre pas tous les problèmes ; vérifie-le avant de le partager.",
        ("hint", true) => "No logs, accounts, game names or paths. This report cannot cover every issue; review it before sharing.",
        ("choose", false) => "Choisir le dossier du diagnostic",
        ("choose", true) => "Choose a diagnostic folder",
        ("success", false) => "Diagnostic enregistré dans le dossier choisi.",
        ("success", true) => "Diagnostic saved to the selected folder.",
        ("failure", false) => "Impossible d'enregistrer le diagnostic.",
        ("failure", true) => "Could not save the diagnostic.",
        ("copied", false) => "Diagnostic copié.",
        ("copied", true) => "Diagnostic copied.",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Check, CheckStatus};

    #[test]
    fn export_does_not_include_free_text_or_unrecognised_checks() {
        let doctor = DoctorReport { checks: vec![
            Check { name: "Steam".into(), status: CheckStatus::Warn, detail: "/home/alice/token=secret".into() },
            Check { name: "alice@example.org".into(), status: CheckStatus::Fail, detail: "private".into() },
            Check { name: "Connexion Steam".into(), status: CheckStatus::Ok, detail: "account".into() },
        ] };
        let value = report(Some(&doctor), None, Some(true));
        let text = value.to_string();
        for secret in ["alice", "token", "/home/", "private", "example.org"] { assert!(!text.contains(secret), "{secret}"); }
        assert_eq!(value["checks"]["steam_library"], "warning");
        assert_eq!(value["checks"]["library"], "unknown");
        assert!(value["checks"].get("steam_session").is_none());
        assert!(value["downloads"].is_null());
    }

    #[test]
    fn environment_has_no_identity_or_paths() {
        let facts = environment().to_string();
        assert!(facts.contains("kernel"));
        assert!(!facts.contains("/home/"));
        assert!(!facts.contains("/sys/"));
        assert!(!facts.contains("/proc/"));
    }
}
