//! Steam sans client Steam : bibliothèques, manifests, DRM, catalogue et
//! téléchargements natifs Lapinux.

pub mod account;
pub mod achievements;
pub mod catalog;
pub mod cdn_stream;
pub mod cloud;
pub mod cm;
pub mod depot;
pub mod download_log;
pub mod drm;
pub mod gamedata;
pub mod library;
pub mod lobby;
pub mod manifest;
pub mod native;
pub mod playtime;
pub mod probe;
pub mod session;
pub mod shared;
pub mod social;
pub mod stats;
pub mod stats_store;
pub mod steam_mapping;
pub mod steaminput;
mod storecache;
mod storegate;
pub mod throttle;
pub mod tools;
pub mod vdf;
pub mod web;
pub mod workshop;

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
    Core(#[from] lapinux_core::Error),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// Vrai si Steam a refusé la connexion parce que le compte ou l'adresse
    /// est temporairement bloqué (EResult 84). Ce refus ne dit rien du jeton.
    pub fn is_rate_limited(&self) -> bool {
        let text = self.to_string().to_lowercase();
        text.contains("eresult 84")
            || text.contains("ratelimitexceeded")
            || text.contains("rate limit exceeded")
            || text.contains("trop de tentatives")
            || text.contains("limite les tentatives")
    }

    /// Vrai si Steam a refusé le jeton (EResult 5, « mot de passe invalide »).
    /// Le jeton reste valable : c'est une session concurrente ou une rafale de
    /// logons qui l'a rendu momentanément refusé, et chaque tentative de plus
    /// prolonge la pénalité. Il faut donc attendre au lieu de réessayer.
    pub fn is_token_refused(&self) -> bool {
        let text = self.to_string().to_lowercase();
        text.contains("invalidpassword")
            || text.contains("mot de passe invalide")
            || text.contains("eresult 5)")
            || text.contains("eresult 5,")
    }
}

/// Installe le fournisseur cryptographique de rustls, une fois pour tout le
/// processus.
///
/// Deux fournisseurs (`ring` et `aws-lc-rs`) sont compilés par nos dépendances :
/// sans choix explicite, rustls panique à la première connexion TLS, ce qui tuait
/// le fil social (amis, présence, profil) dès qu'il ouvrait son WebSocket. À
/// appeler avant toute connexion réseau.
pub fn install_crypto_provider() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // Un fournisseur déjà choisi par l'application reste prioritaire.
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

pub use library::{
    discover_libraries, discover_libraries_with_managed, find_launch_exe, find_native_executable,
    fingerprint_with_managed, merge_existing, scan, scan_library, scan_with_managed, Library,
    LibraryFingerprint, LibraryStamp, ScanReport,
};
pub use manifest::{write_manifest, AppManifest};
