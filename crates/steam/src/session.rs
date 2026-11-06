//! Jeton de session Steam, conservé d'une connexion à l'autre.
//!
//! SteamCMD garde un jeton SSFN après sa première connexion ; ce module fait
//! l'équivalent pour Lapinux : seul le jeton de refresh rendu par le service
//! d'authentification est écrit, jamais le mot de passe. Le fichier est créé
//! en `0600` et ne contient que ce qu'il faut pour reconnecter le compte.

use lapinux_core::paths::LapinuxPaths;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// Session réutilisable d'un compte Steam.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedSession {
    pub account_name: String,
    pub steam_id: u64,
    pub refresh_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guard_data: Option<String>,
    /// Date d'écriture du jeton, en secondes Unix. Posée par [`save_to`] ;
    /// absente des fichiers écrits avant qu'elle existe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved_at: Option<i64>,
}

/// `~/.local/share/lapinux/steam-session.json` (ou `LAPINUX_HOME`).
pub fn path() -> PathBuf {
    path_in(&LapinuxPaths::discover().data_dir)
}

/// Nom du fichier sous un dossier de données donné (daemon, tests isolés).
pub fn path_in(data_dir: &Path) -> PathBuf {
    data_dir.join("steam-session.json")
}

pub fn load() -> Option<SavedSession> {
    load_from(&path())
}

pub fn load_from(path: &Path) -> Option<SavedSession> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Date courante en secondes Unix (négative impossible en pratique).
pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

pub fn save(session: &SavedSession) -> io::Result<PathBuf> {
    let path = path();
    save_to(&path, session)?;
    Ok(path)
}

/// Écrit la session en propriétaire uniquement, même si le fichier existait
/// déjà avec des permissions plus larges.
///
/// La date d'écriture est posée ici : tout appelant qui enregistre un jeton
/// vient de l'obtenir, et l'écran de connexion s'en sert pour dire son âge.
pub fn save_to(path: &Path, session: &SavedSession) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut session = session.clone();
    session.saved_at = Some(now_unix());
    let data = serde_json::to_vec_pretty(&session)
        .map_err(|error| io::Error::new(io::ErrorKind::Other, error))?;
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    file.write_all(&data)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_keeps_the_token_and_guard_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("steam-session.json");
        let saved = SavedSession {
            account_name: "alice".into(),
            steam_id: 7_653_000_000_000_000_000,
            refresh_token: "refresh".into(),
            guard_data: Some("guard".into()),
            saved_at: None,
        };
        let before = now_unix();
        save_to(&path, &saved).unwrap();
        let stored = load_from(&path).expect("session relue");
        // Le jeton écrit est horodaté par `save_to`, le reste est intact.
        let stamp = stored.saved_at.expect("date d'écriture posée");
        assert!((before..=now_unix()).contains(&stamp), "{stamp}");
        assert_eq!(
            SavedSession {
                saved_at: None,
                ..stored
            },
            saved
        );
    }

    #[test]
    fn file_is_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("steam-session.json");
        fs::write(&path, "{}").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        save_to(
            &path,
            &SavedSession {
                account_name: "alice".into(),
                steam_id: 1,
                refresh_token: "refresh".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn token_written_before_the_date_existed_still_loads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("steam-session.json");
        fs::write(
            &path,
            r#"{"account_name":"alice","steam_id":7,"refresh_token":"refresh"}"#,
        )
        .unwrap();
        let stored = load_from(&path).expect("session relue");
        assert_eq!(stored.account_name, "alice");
        assert_eq!(stored.saved_at, None);
    }

    #[test]
    fn missing_or_broken_files_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load_from(&dir.path().join("absent.json")), None);
        let path = dir.path().join("broken.json");
        fs::write(&path, "{").unwrap();
        assert_eq!(load_from(&path), None);
    }
}
