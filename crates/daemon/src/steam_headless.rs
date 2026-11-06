//! Démarrage du client Steam sans interface pour les jeux Steamworks.
//!
//! Steam n'est pas un simple lanceur : `SteamAPI_Init` a besoin d'un client
//! authentifié. On démarre donc le client officiel avec `-silent`, qui vit
//! dans la zone de notification sans ouvrir sa fenêtre principale, puis on
//! attend `steamwebhelper` avant de rendre la main au lancement du jeu.
//!
//! Aucun client tiers n'est utilisé : le client Steam officiel reste seul
//! responsable de la session et du DRM. Lapinux ne stocke pas de mot de passe ;
//! le compte doit avoir une connexion enregistrée pour démarrer sans fenêtre.

use lapinux_core::paths::which;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const READY_TIMEOUT: Duration = Duration::from_secs(60);
const POLL_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Debug)]
enum SteamClient {
    Native(PathBuf),
    Flatpak(PathBuf),
}

impl SteamClient {
    fn command(&self) -> Command {
        match self {
            Self::Native(path) => Command::new(path),
            Self::Flatpak(path) => {
                let mut command = Command::new(path);
                command.args(["run", "com.valvesoftware.Steam"]);
                command
            }
        }
    }
}

/// Trouve le client Steam natif, ou le Flatpak quand c'est la seule version
/// installée.
fn find_client() -> Option<SteamClient> {
    if let Some(path) = which("steam") {
        return Some(SteamClient::Native(path));
    }
    let flatpak = which("flatpak")?;
    let installed = Command::new(&flatpak)
        .args(["info", "com.valvesoftware.Steam"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    installed.then_some(SteamClient::Flatpak(flatpak))
}

fn process_comms() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let pid = entry.file_name().to_str()?.parse::<u32>().ok()?;
            std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()
        })
        .map(|comm| comm.trim().to_string())
        .filter(|comm| !comm.is_empty())
        .collect()
}

/// Le processus principal et son navigateur interne. Le pont manette
/// `steam-puck-bridge` porte « steam » dans son nom mais ne compte pas.
fn is_client_process(comm: &str) -> bool {
    comm == "steam" || comm.starts_with("steamwebhelper")
}

fn is_ready_process(comm: &str) -> bool {
    comm.starts_with("steamwebhelper")
}

/// Vrai tant qu'un client Steam (ou son navigateur interne) est vivant.
pub fn client_alive() -> bool {
    process_comms().iter().any(|comm| is_client_process(comm))
}

fn client_ready() -> bool {
    process_comms().iter().any(|comm| is_ready_process(comm))
}

fn main_client_alive() -> bool {
    process_comms().iter().any(|comm| comm == "steam")
}

fn steam_log(log_dir: &Path) -> Option<std::fs::File> {
    let _ = std::fs::create_dir_all(log_dir);
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_dir.join("steam-headless.log"))
        .ok()
}

/// Démarre le client Steam si nécessaire, puis attend qu'il soit prêt.
///
/// Le client est lancé avec `-silent` : la fenêtre principale ne s'ouvre pas.
/// Si Steam est déjà lancé, on ne crée pas un second client : on attend
/// simplement son navigateur interne.
pub fn ensure(log_dir: &Path) -> Result<(), String> {
    if client_ready() {
        return Ok(());
    }

    if !main_client_alive() {
        let client = find_client().ok_or_else(|| {
            "client Steam introuvable ; installe Steam ou son Flatpak".to_string()
        })?;
        tracing::info!("Steam headless : démarrage du client");
        let mut command = client.command();
        command.arg("-silent");
        command.stdin(Stdio::null());
        if let Some(log) = steam_log(log_dir) {
            match log.try_clone() {
                Ok(errors) => {
                    command.stdout(Stdio::from(log));
                    command.stderr(Stdio::from(errors));
                }
                Err(_) => {
                    command.stdout(Stdio::from(log));
                    command.stderr(Stdio::null());
                }
            }
        } else {
            command.stdout(Stdio::null());
            command.stderr(Stdio::null());
        }
        let mut child = command
            .spawn()
            .map_err(|error| format!("impossible de démarrer Steam : {error}"))?;
        // Steam se détache parfois de son lanceur : ce fil évite un zombie
        // quand le script se termine, sans bloquer le daemon.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    } else {
        tracing::info!("Steam headless : client déjà lancé, attente de l'interface interne");
    }

    let started = Instant::now();
    let mut next_note = 10u64;
    while started.elapsed() < READY_TIMEOUT {
        if client_ready() {
            tracing::info!("Steam headless : client prêt");
            return Ok(());
        }
        if started.elapsed().as_secs() >= next_note {
            tracing::info!(
                "Steam headless : toujours en cours de démarrage ({} s)",
                started.elapsed().as_secs()
            );
            next_note += 10;
        }
        std::thread::sleep(POLL_INTERVAL);
    }

    Err(format!(
        "Steam n'a pas fini de démarrer après {} s ; vérifie qu'il est à jour et connecté, puis réessaie",
        READY_TIMEOUT.as_secs()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_pont_manette_n_est_pas_un_client_steam() {
        assert!(is_client_process("steam"));
        assert!(is_client_process("steamwebhelper"));
        assert!(is_ready_process("steamwebhelper"));
        assert!(!is_client_process("steam-puck-bridge"));
        assert!(!is_ready_process("steam"));
    }
}
