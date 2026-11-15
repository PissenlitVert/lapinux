//! Diagnostic partageable, construit exclusivement à partir de champs autorisés.
//!
//! Aucun texte libre du daemon, nom de jeu, chemin, journal ou jeton n'est
//! exporté : masquer certains motifs dans un journal ne suffit pas à garantir
//! sa confidentialité.

use crate::model::{DoctorReport, Job, JobKind, JobState};
use serde_json::{json, Value};

/// Résumé de l'application tel que l'utilisateur la voit : réglages choisis,
/// taille de la bibliothèque et comptes connectés. Aucun nom de jeu, chemin ou
/// identifiant de compte n'y figure.
#[derive(Debug, Clone, Default)]
pub struct AppSummary {
    pub language: String,
    /// `classic`, `native` ou `custom` : le nom libre d'un thème n'est pas exporté.
    pub theme_kind: String,
    pub page: String,
    pub startup_page: String,
    pub big_picture: bool,
    pub gamescope: bool,
    pub end_screen: bool,
    pub capture_on_launch: bool,
    pub discord: bool,
    pub plugins: u32,
    pub library_total: u32,
    pub library_installed: u32,
    pub steam_connected: bool,
    pub steam_token_saved: bool,
}

impl AppSummary {
    fn json(&self) -> Value {
        json!({
            "language": self.language,
            "theme_kind": self.theme_kind,
            "page": self.page,
            "startup_page": self.startup_page,
            "big_picture": self.big_picture,
            "gamescope": self.gamescope,
            "end_screen": self.end_screen,
            "capture_on_launch": self.capture_on_launch,
            "discord": self.discord,
            "plugins": self.plugins,
            "library_total": self.library_total,
            "library_installed": self.library_installed,
            "steam_connected": self.steam_connected,
            "steam_token_saved": self.steam_token_saved,
        })
    }
}

/// Rapport structuré sans journal ni données de compte. Les sections absentes
/// sont nulles : une erreur de lecture ne doit pas passer pour une réussite.
pub fn report(
    doctor: Option<&DoctorReport>,
    jobs: Option<&[Job]>,
    daemon: Option<bool>,
    app: Option<&AppSummary>,
) -> Value {
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
        "application": app.map(AppSummary::json),
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
    let cpu = std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|text| {
            text.lines().find_map(|line| {
                line.strip_prefix("model name")
                    .map(|rest| rest.trim_start_matches([':', ' ', '\t']).trim().to_string())
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
            .and_then(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().to_string())
            });
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
        ("hint", false) => "Sans journaux, comptes, noms de jeux ni chemins. Inclut un résumé de l'application : réglages, compteurs de bibliothèque et comptes connectés. Vérifie-le avant de le partager.",
        ("hint", true) => "No logs, accounts, game names or paths. Includes an application summary: settings, library counters and connected accounts. Review it before sharing.",
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
        let doctor = DoctorReport {
            checks: vec![
                Check {
                    name: "Steam".into(),
                    status: CheckStatus::Warn,
                    detail: "/home/alice/token=secret".into(),
                },
                Check {
                    name: "alice@example.org".into(),
                    status: CheckStatus::Fail,
                    detail: "private".into(),
                },
                Check {
                    name: "Connexion Steam".into(),
                    status: CheckStatus::Ok,
                    detail: "account".into(),
                },
            ],
        };
        let value = report(Some(&doctor), None, Some(true), None);
        let text = value.to_string();
        for secret in ["alice", "token", "/home/", "private", "example.org"] {
            assert!(!text.contains(secret), "{secret}");
        }
        assert_eq!(value["checks"]["steam_library"], "warning");
        assert_eq!(value["checks"]["library"], "unknown");
        assert!(value["checks"].get("steam_session").is_none());
        assert!(value["downloads"].is_null());
    }

    #[test]
    fn application_summary_adds_counters_without_names() {
        let app = AppSummary {
            language: "fr".into(),
            theme_kind: "custom".into(),
            page: "library".into(),
            startup_page: "default".into(),
            big_picture: false,
            gamescope: true,
            end_screen: true,
            capture_on_launch: true,
            discord: false,
            plugins: 2,
            library_total: 42,
            library_installed: 7,
            steam_connected: true,
            steam_token_saved: true,
        };
        let value = report(None, None, Some(true), Some(&app));
        assert_eq!(value["application"]["library_total"], 42);
        assert_eq!(value["application"]["library_installed"], 7);
        assert_eq!(value["application"]["theme_kind"], "custom");
        assert_eq!(value["application"]["gamescope"], true);
        let text = value.to_string();
        for private in ["alice", "MyGame", "/home/"] {
            assert!(!text.contains(private), "{private}");
        }
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
