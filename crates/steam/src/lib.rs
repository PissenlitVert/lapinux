//! Steam sans client Steam : bibliothèques, manifests, DRM, SteamCMD, DepotDownloader.

pub mod depotdownloader;
pub mod drm;
pub mod library;
pub mod manifest;
pub mod meta;
pub mod steamcmd;
pub mod tools;
pub mod vdf;

use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("E/S : {0}")]
    Io(#[from] std::io::Error),
    #[error("VDF : {0}")]
    Vdf(String),
    #[error("JSON : {0}")]
    Json(#[from] serde_json::Error),
    #[error("réseau : {0}")]
    Http(String),
    #[error("core : {0}")]
    Core(#[from] ludus_core::Error),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;

pub use library::{
    discover_libraries, find_launch_exe, find_native_executable, merge_existing, scan, scan_library,
    Library, ScanReport,
};
pub use manifest::{write_manifest, AppManifest};
pub use meta::{parse_app_info, AppInfo, LaunchEntry};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    SteamCmd,
    DepotDownloader,
}

impl Backend {
    pub fn label(self) -> &'static str {
        match self {
            Backend::SteamCmd => "SteamCMD",
            Backend::DepotDownloader => "DepotDownloader",
        }
    }
}

/// Choisit un backend de téléchargement selon la préférence et les outils présents.
pub fn choose_backend(
    preference: &str,
    allow_depotdownloader: bool,
    steamcmd_ok: bool,
    depotdownloader_ok: bool,
) -> Option<Backend> {
    match preference {
        "steamcmd" => steamcmd_ok.then_some(Backend::SteamCmd),
        "depotdownloader" => {
            (allow_depotdownloader && depotdownloader_ok).then_some(Backend::DepotDownloader)
        }
        _ => {
            if steamcmd_ok {
                Some(Backend::SteamCmd)
            } else if allow_depotdownloader && depotdownloader_ok {
                Some(Backend::DepotDownloader)
            } else {
                None
            }
        }
    }
}

/// Vrai si SteamCMD possède déjà un jeton de connexion (SSFN).
pub fn has_steam_token(steamcmd_root: &Path) -> bool {
    let homes = [steamcmd_root.to_path_buf(), ludus_core::paths::home_dir().join(".steam")];
    homes.iter().any(|dir| {
        std::fs::read_dir(dir)
            .map(|entries| {
                entries.flatten().any(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .to_ascii_lowercase()
                        .starts_with("ssfn")
                })
            })
            .unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_preference() {
        assert_eq!(
            choose_backend("auto", true, true, true),
            Some(Backend::SteamCmd)
        );
        assert_eq!(
            choose_backend("auto", true, false, true),
            Some(Backend::DepotDownloader)
        );
        assert_eq!(choose_backend("auto", false, false, true), None);
        assert_eq!(choose_backend("steamcmd", true, false, true), None);
        assert_eq!(
            choose_backend("depotdownloader", true, true, true),
            Some(Backend::DepotDownloader)
        );
    }

    #[test]
    fn token_detection() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!has_steam_token(dir.path()));
        std::fs::write(dir.path().join("ssfn1234"), b"x").unwrap();
        assert!(has_steam_token(dir.path()));
    }
}
