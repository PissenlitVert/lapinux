//! Couche manettes maison : evdev, remapping, pad virtuel uinput et profils.
//! Le raccourci clavier du menu rapide passe par KWin, jamais par `/dev/input`.

pub mod access;
pub mod battery;
pub mod bluetooth;
pub mod codes;
pub mod control;
pub mod device;
pub mod mapping;
pub mod shortcut;
pub mod nav;
pub mod monitor;
pub mod steam;
pub mod panels;
pub mod service;
pub mod virtual_pad;
pub mod wired;

use lapinux_core::model::ProfileSummary;
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
    Core(#[from] lapinux_core::Error),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;

pub use access::{audit, Access};
pub use battery::{BatteryDesign, BatteryInfo, ChargeState};
pub use bluetooth::{paired_gamepads, watch_wired_takeover};
pub use wired::{usb_hid_for, UsbHid};
pub use mapping::{apply_deadzone_curve, Mapper, Profile, VEvent};
pub use monitor::{
    FaceButtons, MotionState, PadMonitor, PadState, ReportCadence, TouchPoint, TouchpadState,
};
pub use steam::{SteamPad, SteamPads};
pub use nav::{nav_from_event, Nav, NavReader, NavRepeat};
pub use service::{set_overlay_navigation, InputMsg, InputService};
pub use virtual_pad::VirtualPad;

pub fn profiles_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("profiles")
}

pub fn profile_path(data_dir: &Path, name: &str) -> PathBuf {
    profiles_dir(data_dir).join(format!("{name}.toml"))
}

/// Profils fournis avec Lapinux, même quand aucun fichier n'a encore été
/// créé. L'ordre est celui de la liste : Xbox reste le choix par défaut.
pub const BUILTIN_PROFILES: [&str; 2] = ["xbox", "nintendo"];

/// Profil intégré d'une marque. Xbox ne change rien ; Nintendo échange les
/// quatre boutons de façade pour que les lettres imprimées sur la manette
/// correspondent aux boutons attendus par les jeux.
pub fn builtin_profile(name: &str) -> Option<Profile> {
    let mut profile = Profile::default();
    profile.name = name.to_string();
    match name {
        "xbox" => Some(profile),
        "nintendo" => {
            profile.swap_ab = true;
            profile
                .remap
                .insert("BTN_NORTH".into(), "BTN_WEST".into());
            profile
                .remap
                .insert("BTN_WEST".into(), "BTN_NORTH".into());
            Some(profile)
        }
        _ => None,
    }
}

pub fn load_profile(data_dir: &Path, name: &str) -> Result<Profile> {
    let path = profile_path(data_dir, name);
    if !path.exists() {
        if let Some(profile) = builtin_profile(name) {
            return Ok(profile);
        }
        let mut profile = Profile::default();
        profile.name = name.to_string();
        return Ok(profile);
    }
    Profile::load(&path)
}

/// Crée le profil demandé (idempotent) et renvoie le profil actif.
pub fn ensure_profiles(data_dir: &Path, default_name: &str) -> Result<Profile> {
    std::fs::create_dir_all(profiles_dir(data_dir))?;
    if !profile_path(data_dir, default_name).exists() {
        let mut profile = builtin_profile(default_name).unwrap_or_default();
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
            // Le profil historique « defaut » ne portait aucun remap et a
            // été remplacé par le profil Xbox intégré. Son fichier peut
            // encore exister d'une ancienne installation : on ne l'expose
            // plus, mais on ne supprime rien de l'utilisateur.
            if name == "defaut" {
                continue;
            }
            out.push(ProfileSummary {
                is_default: name == "xbox",
                name,
                path,
            });
        }
    }
    // Les profils Xbox et Nintendo sont fournis par Lapinux : l'overlay doit
    // les proposer même si l'utilisateur n'a jamais ouvert la page Manettes.
    for name in BUILTIN_PROFILES {
        if !out.iter().any(|profile| profile.name == name) {
            out.push(ProfileSummary {
                name: name.to_string(),
                path: profile_path(data_dir, name),
                is_default: name == "xbox",
            });
        }
    }
    out.sort_by(|a, b| {
        b.is_default
            .cmp(&a.is_default)
            .then_with(|| builtin_rank(&a.name).cmp(&builtin_rank(&b.name)))
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}

fn builtin_rank(name: &str) -> u8 {
    match name {
        "xbox" => 0,
        "nintendo" => 1,
        _ => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_and_lists_default_profile() {
        let dir = tempfile::tempdir().unwrap();
        let profile = ensure_profiles(dir.path(), "xbox").unwrap();
        assert_eq!(profile.name, "xbox");
        assert!(profile_path(dir.path(), "xbox").is_file());
        let listed = list_profiles(dir.path());
        assert_eq!(
            listed
                .iter()
                .map(|profile| profile.name.as_str())
                .collect::<Vec<_>>(),
            vec!["xbox", "nintendo"]
        );
        assert!(listed[0].is_default);
    }

    #[test]
    fn le_profil_historique_defaut_est_masque() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(profiles_dir(dir.path())).unwrap();
        std::fs::write(profile_path(dir.path(), "defaut"), "").unwrap();
        std::fs::write(profile_path(dir.path(), "perso"), "").unwrap();
        let listed = list_profiles(dir.path());
        assert_eq!(
            listed
                .iter()
                .map(|profile| profile.name.as_str())
                .collect::<Vec<_>>(),
            vec!["xbox", "nintendo", "perso"]
        );
    }

    #[test]
    fn builtins_are_available_without_files() {
        let dir = tempfile::tempdir().unwrap();
        let xbox = load_profile(dir.path(), "xbox").unwrap();
        assert!(!xbox.swap_ab);
        let nintendo = load_profile(dir.path(), "nintendo").unwrap();
        assert!(nintendo.swap_ab);
        assert_eq!(
            nintendo.remap.get("BTN_NORTH").map(String::as_str),
            Some("BTN_WEST")
        );
        assert_eq!(
            nintendo.remap.get("BTN_WEST").map(String::as_str),
            Some("BTN_NORTH")
        );
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
