//! Types partagés, configuration, persistance et IPC de Lapinux.

pub mod captures;
pub mod client;
pub mod config;
pub mod configurations;
pub mod display;
pub mod error;
pub mod ipc;
pub mod i18n;
pub mod lang;
pub mod model;
pub mod paths;
pub mod proc;
pub mod social;
pub mod store;
pub mod stores;
pub mod system;
pub mod util;

pub use config::Config;
pub use error::{Error, Result};
pub use model::*;
pub use paths::LapinuxPaths;

/// Nom du produit, imprimé partout où l'application se présente à
/// l'utilisateur : fenêtre, aide en ligne de commande, diagnostic.
pub const APP_NAME: &str = "Lapinux";
/// Titre de la fenêtre plein écran d'attente. Il est unique pour que KWin
/// puisse la reconnaître et la maintenir au-dessus de la fenêtre du jeu.
pub const SPLASH_TITLE: &str = "Lapinux — Attente";
/// Nom du binaire installé : cargo, lanceur du bureau, services.
pub const APP_BIN: &str = "lapinux";
/// Identifiant d'application : app_id Wayland, WM_CLASS X11 et nom du
/// lanceur du bureau, pour que la fenêtre et le menu se reconnaissent.
pub const APP_ID: &str = "lapinux";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Variable d'environnement de l'application : `LAPINUX_<suffix>`.
///
/// Toutes les variables sont lues par ce point unique, pour qu'un renommage
/// futur n'ait qu'un endroit à toucher.
pub fn env_var(suffix: &str) -> Option<String> {
    env_var_os(suffix).map(|value| value.to_string_lossy().into_owned())
}

pub fn env_var_os(suffix: &str) -> Option<std::ffi::OsString> {
    std::env::var_os(format!("LAPINUX_{suffix}"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn les_variables_d_environnement_sont_prefixees_lapinux() {
        // Nom unique au test : aucun autre test ne touche cette variable.
        std::env::set_var("LAPINUX_TEST_HELLO", "bonjour");
        assert_eq!(super::env_var("TEST_HELLO").as_deref(), Some("bonjour"));
        std::env::remove_var("LAPINUX_TEST_HELLO");
        assert_eq!(super::env_var("TEST_HELLO"), None);
    }
}
