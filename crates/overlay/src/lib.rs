//! Quick menu overlay Wayland (wlr-layer-shell) rendu en CPU.

pub mod font;
pub mod menu;
pub mod render;
pub mod shm;
pub mod toast;
pub mod wayland;
pub mod xkb;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("E/S : {0}")]
    Io(#[from] std::io::Error),
    #[error("Wayland : {0}")]
    Wayland(String),
    #[error("core : {0}")]
    Core(#[from] lapinux_core::Error),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;

pub use menu::{Action, Item, MenuState, ITEMS};
pub use toast::{run as run_toast, ToastOptions};
pub use wayland::{run, OverlayOptions};
