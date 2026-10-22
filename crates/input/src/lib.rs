//! Couche manettes maison : evdev, remapping, pad virtuel uinput et profils.

pub mod codes;
pub mod device;
pub mod mapping;
pub mod nav;
pub mod service;
pub mod virtual_pad;

use ludus_core::model::ProfileSummary;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("E/S : {0}")]
    Io(#[from] std::io::Error),
    #[error("TOML : {0}")]
    Toml(#[from] toml::de::Error),
    #[error("TOML : {0}")]
    TomlSer(#[from] toml::ser::Error),
    #[error("core : {0}")]
    Core(#[from] ludus_core::Error),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;

pub use mapping::{apply_deadzone_curve, Mapper, Profile, VEvent};
pub use nav::{Nav, NavReader};
pub use service::{HotkeySource, InputMsg, InputService};
pub use virtual_pad::VirtualPad;

pub fn profiles_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("profiles")
}

pub fn profile_path(data_dir: &Path, name: &str) -> PathBuf {
    profiles_dir(data_dir).join(format!("{name}.toml"))
}

pub fn load_profile(data_dir: &Path, name: &str) -> Result<Profile> {
    let path = profile_path(data_dir, name);
    if !path.exists() {
        let mut profile = Profile::default();
        profile.name = name.to_string();
        return Ok(profile);
    }
    Profile::load(&path)
}

/// Crée les profils par défaut (idempotent) et renvoie le profil actif.
pub fn ensure_profiles(data_dir: &Path, default_name: &str) -> Result<Profile> {
    std::fs::create_dir_all(profiles_dir(data_dir))?;
    if !profile_path(data_dir, default_name).exists() {
        let mut profile = Profile::default();
        profile.name = default_name.to_string();
        profile.save(&profile_path(data_dir, default_name))?;
    }
    load_profile(data_dir, default_name)
}

pub fn list_profiles(data_dir: &Path) -> Vec<ProfileSummary> {
    let dir = profiles_dir(data_dir);
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }
            let name = path
                .file_stem()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            out.push(ProfileSummary {
                is_default: name == "defaut",
                name,
                path,
            });
        }
    }
    out.sort_by(|a, b| b.is_default.cmp(&a.is_default).then(a.name.cmp(&b.name)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_and_lists_default_profile() {
        let dir = tempfile::tempdir().unwrap();
        let profile = ensure_profiles(dir.path(), "defaut").unwrap();
        assert_eq!(profile.name, "defaut");
        assert!(profile_path(dir.path(), "defaut").is_file());
        let listed = list_profiles(dir.path());
        assert_eq!(listed.len(), 1);
        assert!(listed[0].is_default);
    }

    #[test]
    fn profile_roundtrip_with_remap() {
        let dir = tempfile::tempdir().unwrap();
        let mut profile = Profile::default();
        profile.name = "perso".into();
        profile.remap.insert("BTN_SOUTH".into(), "BTN_EAST".into());
        profile.shift_button = Some("BTN_TL2".into());
        profile.save(&profile_path(dir.path(), "perso")).unwrap();
        let loaded = load_profile(dir.path(), "perso").unwrap();
        assert_eq!(loaded.remap.get("BTN_SOUTH").unwrap(), "BTN_EAST");
        assert_eq!(loaded.shift_button.as_deref(), Some("BTN_TL2"));
    }

    #[test]
    fn out_of_range_values_are_clamped() {
        let dir = tempfile::tempdir().unwrap();
        let path = profile_path(dir.path(), "fou");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "name = \"fou\"\ndeadzone = 9.0\ncurve = 0.01\n").unwrap();
        let loaded = load_profile(dir.path(), "fou").unwrap();
        assert_eq!(loaded.deadzone, 0.6);
        assert_eq!(loaded.curve, 0.5);
    }
}
