//! Gestion des préfixes Wine/Proton, par jeu.

use crate::{Error, Result};
use ludus_core::model::Game;
use std::path::{Path, PathBuf};

/// Préfixe existant du jeu (compatdata Steam) sinon préfixe Ludus dédié.
pub fn prefix_for(game: &Game, data_dir: &Path) -> PathBuf {
    if let Some(existing) = &game.prefix {
        if existing.is_dir() {
            return existing.clone();
        }
    }
    data_dir.join("prefixes").join(game.appid.to_string())
}

pub fn marker_path(prefix: &Path) -> PathBuf {
    prefix.join("ludus-prefix.json")
}

pub fn is_initialized(prefix: &Path) -> bool {
    prefix.join("pfx").is_dir() || prefix.join("drive_c").is_dir()
}

/// Crée le répertoire de préfixe et son marqueur.
pub fn ensure(prefix: &Path, appid: u32) -> Result<()> {
    std::fs::create_dir_all(prefix)?;
    let marker = marker_path(prefix);
    if !marker.exists() {
        let payload = serde_json::json!({
            "appid": appid,
            "created_by": "ludus",
            "version": env!("CARGO_PKG_VERSION"),
        });
        std::fs::write(&marker, serde_json::to_vec_pretty(&payload)?)?;
    }
    Ok(())
}

/// Supprime un préfixe (opération destructive, confirmée par l'appelant).
pub fn reset(prefix: &Path) -> Result<bool> {
    if !prefix.exists() {
        return Ok(false);
    }
    let text = prefix.to_string_lossy();
    let looks_safe = prefix
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.chars().all(|c| c.is_ascii_digit()))
        .unwrap_or(false)
        && (text.contains("/compatdata/") || text.contains("/prefixes/"))
        && prefix.components().count() >= 4;
    if !looks_safe {
        return Err(Error::Other(format!(
            "refus de supprimer un chemin inattendu : {}",
            prefix.display()
        )));
    }
    std::fs::remove_dir_all(prefix)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game_with_prefix(prefix: Option<PathBuf>) -> Game {
        Game {
            appid: 440,
            prefix,
            ..Default::default()
        }
    }

    #[test]
    fn prefers_existing_steam_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let compat = dir.path().join("compatdata/440");
        std::fs::create_dir_all(&compat).unwrap();
        let chosen = prefix_for(&game_with_prefix(Some(compat.clone())), dir.path());
        assert_eq!(chosen, compat);
    }

    #[test]
    fn falls_back_to_ludus_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let chosen = prefix_for(&game_with_prefix(None), dir.path());
        assert!(chosen.ends_with("prefixes/440"));
    }

    #[test]
    fn ensure_creates_marker() {
        let dir = tempfile::tempdir().unwrap();
        let prefix = dir.path().join("prefixes/440");
        ensure(&prefix, 440).unwrap();
        assert!(marker_path(&prefix).is_file());
        assert!(!is_initialized(&prefix));
        std::fs::create_dir_all(prefix.join("pfx")).unwrap();
        assert!(is_initialized(&prefix));
    }

    #[test]
    fn reset_refuses_unexpected_path() {
        let dir = tempfile::tempdir().unwrap();
        let weird = dir.path().join("home/prefix");
        std::fs::create_dir_all(&weird).unwrap();
        assert!(reset(&weird).is_err());
    }

    #[test]
    fn reset_removes_compatdata_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let prefix = dir.path().join("steamapps/compatdata/440");
        std::fs::create_dir_all(prefix.join("pfx")).unwrap();
        assert!(reset(&prefix).unwrap());
        assert!(!prefix.exists());
        assert!(!reset(&prefix).unwrap());
    }
}
