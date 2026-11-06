//! Outils de performance réellement installés sur la machine.

use std::path::PathBuf;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PerfCaps {
    pub gamescope: Option<PathBuf>,
    pub mangohud: Option<PathBuf>,
    pub power_profiles: Option<PathBuf>,
}

impl PerfCaps {
    pub fn detect() -> Self {
        Self {
            gamescope: lapinux_core::paths::which("gamescope"),
            mangohud: lapinux_core::paths::which("mangohud"),
            power_profiles: lapinux_core::paths::which("powerprofilesctl"),
        }
    }
}
