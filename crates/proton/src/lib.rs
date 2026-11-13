//! Gestion des Proton : découverte, téléchargement, préfixes et lancement.

pub mod install;
pub mod launch;
pub mod prefix;
pub mod releases;
pub mod scan;
pub mod tools;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("E/S : {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON : {0}")]
    Json(#[from] serde_json::Error),
    #[error("réseau : {0}")]
    Http(String),
    #[error("core : {0}")]
    Core(#[from] lapinux_core::Error),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;

pub use install::{extract, install_from_url, verify_sha512};
pub use launch::{build, render_plan, umu_run_path, LaunchOptions, LaunchPlan};
pub use prefix::{
    ensure as ensure_prefix, initialize as initialize_prefix, is_initialized, prefix_for,
    reset as reset_prefix,
};
pub use prefix::{prefix_name, prefix_name_for, prefix_path};
pub use releases::{all_releases, fetch_releases, parse_releases, ProtonSource, SOURCES};
pub use scan::{find, recommended, scan};
