//! Types partagés, configuration, persistance et IPC de Ludus.

pub mod client;
pub mod config;
pub mod error;
pub mod ipc;
pub mod model;
pub mod paths;
pub mod proc;
pub mod store;
pub mod util;

pub use config::Config;
pub use error::{Error, Result};
pub use model::*;
pub use paths::LudusPaths;

pub const APP_NAME: &str = "Ludus";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
