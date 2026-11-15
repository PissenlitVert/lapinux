//! Client Steam en arrière-plan.
//!
//! Certains jeux n'interrogent pas seulement `steam_api64.dll` : leur
//! protection passe par `steamclient`, qui exige un client Steam vivant et
//! **connecté** à un compte. Le client est lancé sans fenêtre (`-silent`) et
//! sans préchargement (`LD_AUDIT`, `LD_PRELOAD`) : Lapinux ne demande pas
//! lui-même d'instrumenter le client.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// Temps laissé au client pour se connecter avant de rendre la main.
const READY_TIMEOUT: Duration = Duration::from_secs(120);

/// Le client Steam tourne-t-il déjà ?
fn client_running() -> bool {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return false;
    };
    entries.flatten().any(|entry| {
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .filter(|pid| pid.chars().all(|digit| digit.is_ascii_digit()))
        else {
            return false;
        };
        std::fs::read_to_string(format!("/proc/{pid}/comm"))
            .map(|comm| comm.trim() == "steam")
            .unwrap_or(false)
    })
}

/// Journal où le client inscrit ses changements d'état de connexion.
fn connection_log() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    [
        home.join(".local/share/Steam"),
        home.join(".steam/steam"),
        home.join(".steam/root"),
    ]
    .into_iter()
    .map(|base| base.join("logs/connection_log.txt"))
    .find(|path| path.is_file())
}

/// L'état final du journal est-il « connecté » ?
///
/// `Logged On` puis `Logged Off` s'écrivent dans le même fichier d'une session
/// à l'autre : seul le dernier événement compte.
fn ends_logged_on(text: &str) -> bool {
    let mut logged_on = false;
    for line in text.lines() {
        if line.contains("[Logged On") {
            logged_on = true;
        } else if line.contains("[Logged Off") {
            logged_on = false;
        }
    }
    logged_on
}

/// Le compte est-il connecté depuis le lancement du client ?
///
/// Le fichier doit avoir été écrit après le démarrage : sans cette borne, un
/// `Logged On` d'une session précédente ferait partir le jeu devant un client
/// encore à l'écran de connexion.
fn logged_on_since(path: &Path, since: SystemTime) -> bool {
    let Ok(modified) = std::fs::metadata(path).and_then(|meta| meta.modified()) else {
        return false;
    };
    if modified < since {
        return false;
    }
    std::fs::read_to_string(path)
        .map(|text| ends_logged_on(&text))
        .unwrap_or(false)
}

/// Démarre le client sans fenêtre et attend la connexion au compte.
pub fn ensure_running() {
    if client_running() {
        return;
    }
    let started = SystemTime::now();
    let log = connection_log();
    let mut command = std::process::Command::new("steam");
    command.arg("-silent");
    for key in ["LD_AUDIT", "LD_PRELOAD"] {
        command.env_remove(key);
    }
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if let Err(error) = command.spawn() {
        tracing::warn!("Steam en arrière-plan : lancement impossible ({error})");
        return;
    }
    tracing::info!("Steam en arrière-plan : client démarré sans fenêtre (-silent)");
    let deadline = Instant::now() + READY_TIMEOUT;
    while Instant::now() < deadline {
        let ready = match log.as_deref() {
            Some(path) => logged_on_since(path, started),
            // Sans journal lisible, on ne peut vérifier que le processus :
            // mieux vaut attendre franchement qu'un client en cours de
            // connexion, plutôt que de lancer le jeu devant un écran de login.
            None => client_running(),
        };
        if ready {
            tracing::info!("Steam en arrière-plan : connecté");
            return;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    tracing::warn!("Steam en arrière-plan : toujours pas connecté après 120 s");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_dernier_evenement_decide_de_l_etat() {
        let connecte = "[2026-11-14 22:16:03] [Logged On, 4, 7] [U:1:31] Recv…\n";
        assert!(ends_logged_on(connecte));

        let deconnecte =
            format!("{connecte}[2026-11-14 22:19:32] [Logged Off, 0, 0] [U:1:31] LogOff()\n");
        assert!(!ends_logged_on(&deconnecte));

        let reconnexion =
            format!("{deconnecte}[2026-11-14 22:25:00] [Logged On, 4, 7] [U:1:31] Recv…\n");
        assert!(ends_logged_on(&reconnexion));
        assert!(!ends_logged_on(""));
    }

    #[test]
    fn un_journal_anterieur_au_lancement_ne_compte_pas() {
        let dir = std::env::temp_dir().join(format!("lapinux-steam-log-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dossier de test");
        let path = dir.join("connection_log.txt");
        std::fs::write(&path, "[2026-11-14 21:39:57] [Logged On, 4, 7] [U:1:31]\n").unwrap();
        let previous = SystemTime::now();
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(&path, "[2026-11-14 22:16:03] [Logged On, 4, 7] [U:1:31]\n").unwrap();
        assert!(!logged_on_since(&path, previous + Duration::from_secs(60)));
        assert!(logged_on_since(&path, previous));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
