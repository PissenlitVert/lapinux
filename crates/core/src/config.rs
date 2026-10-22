//! Configuration utilisateur (TOML).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::paths::{expand_tilde, home_dir, LudusPaths};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub general: GeneralConfig,
    pub steam: SteamConfig,
    pub launch: LaunchConfig,
    pub overlay: OverlayConfig,
    pub input: InputConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct GeneralConfig {
    /// Langue de l'interface : "fr" ou "en".
    pub lang: String,
    /// Racine des bibliothèques de jeux gérées par Ludus.
    pub library_root: String,
    /// Thème : "sombre" ou "clair".
    pub theme: String,
    /// Animations de l'interface.
    pub animations: bool,
    /// Démarre directement en mode plein écran façon console.
    pub big_picture: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SteamConfig {
    /// "auto", "steamcmd" ou "depotdownloader".
    pub backend: String,
    /// Nom de compte Steam (jamais le mot de passe).
    pub username: String,
    /// Accepte l'usage de DepotDownloader (outil tiers) comme repli.
    pub allow_depotdownloader: bool,
    /// Autorise le lancement des outils réseau en mode simulation.
    pub offline_tools: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct LaunchConfig {
    /// Proton par défaut ("auto" = meilleur candidat détecté).
    pub proton: String,
    /// Lance le jeu dans gamescope.
    pub gamescope: bool,
    pub width: u32,
    pub height: u32,
    pub fps_cap: u32,
    pub hdr: bool,
    pub adaptive_sync: bool,
    pub fullscreen: bool,
    /// MangoHud par défaut.
    pub mangohud: bool,
    /// Utilise gamemoderun s'il est disponible.
    pub gamemode: bool,
    /// Lance Steam en arrière-plan pour les jeux à DRM Steam.
    pub steam_for_drm: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct OverlayConfig {
    /// Raccourci clavier global (chord evdev) : "Meta+F1".
    pub hotkey: String,
    /// Raccourci manette : "L1+R1+Start".
    pub pad_hotkey: String,
    pub fps_max: u32,
    /// Enregistreur utilisé si présent.
    pub recorder: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct InputConfig {
    pub grab_devices: bool,
    pub deadzone: f32,
    pub stick_curve: f32,
    /// Profil appliqué par défaut.
    pub default_profile: String,
    /// Gyro activé par défaut.
    pub gyro: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            general: GeneralConfig::default(),
            steam: SteamConfig::default(),
            launch: LaunchConfig::default(),
            overlay: OverlayConfig::default(),
            input: InputConfig::default(),
        }
    }
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            lang: "fr".into(),
            library_root: default_library_root().to_string_lossy().to_string(),
            theme: "sombre".into(),
            animations: true,
            big_picture: false,
        }
    }
}

impl Default for SteamConfig {
    fn default() -> Self {
        Self {
            backend: "auto".into(),
            username: String::new(),
            allow_depotdownloader: true,
            offline_tools: false,
        }
    }
}

impl Default for LaunchConfig {
    fn default() -> Self {
        Self {
            proton: "auto".into(),
            gamescope: true,
            width: 2560,
            height: 1440,
            fps_cap: 120,
            hdr: false,
            adaptive_sync: true,
            fullscreen: true,
            mangohud: false,
            gamemode: true,
            steam_for_drm: true,
        }
    }
}

impl Default for OverlayConfig {
    fn default() -> Self {
        Self {
            hotkey: "Meta+F1".into(),
            pad_hotkey: "L1+R1+Start".into(),
            fps_max: 60,
            recorder: "gpu-screen-recorder".into(),
        }
    }
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            grab_devices: true,
            deadzone: 0.12,
            stick_curve: 1.6,
            default_profile: "defaut".into(),
            gyro: false,
        }
    }
}

/// `/var/mnt/Jeux` si le disque de jeux est monté et assez libre, sinon `~/Jeux`.
pub fn default_library_root() -> PathBuf {
    for candidate in ["/var/mnt/Jeux", &format!("{}/Jeux", home_dir().display())] {
        let path = PathBuf::from(candidate);
        if path.parent().map(|p| p.exists()).unwrap_or(false) && candidate.starts_with("/var/mnt") {
            return path.join("LudusLibrary");
        }
    }
    home_dir().join("Jeux/LudusLibrary")
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, crate::error::Error> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(path)?;
        let cfg: Config = toml::from_str(&raw)?;
        Ok(cfg.normalized())
    }

    pub fn load_or_create(paths: &LudusPaths) -> Result<Self, crate::error::Error> {
        let cfg = Self::load(&paths.config_file)?;
        if !paths.config_file.exists() {
            cfg.save(&paths.config_file)?;
        }
        Ok(cfg)
    }

    pub fn save(&self, path: &Path) -> Result<(), crate::error::Error> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let raw = toml::to_string_pretty(self)?;
        std::fs::write(path, raw)?;
        Ok(())
    }

    /// Normalise les valeurs relatives (`~`) et les bornes numériques.
    pub fn normalized(mut self) -> Self {
        if self.general.library_root.starts_with('~') {
            self.general.library_root = expand_tilde(&self.general.library_root)
                .to_string_lossy()
                .to_string();
        }
        self.launch.fps_cap = self.launch.fps_cap.clamp(30, 480);
        self.overlay.fps_max = self.overlay.fps_max.clamp(30, 240);
        self.input.deadzone = self.input.deadzone.clamp(0.0, 0.6);
        self.input.stick_curve = self.input.stick_curve.clamp(0.5, 4.0);
        if self.general.lang != "en" {
            self.general.lang = "fr".into();
        }
        self
    }

    pub fn library_root(&self) -> PathBuf {
        expand_tilde(&self.general.library_root)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let cfg = Config::default().normalized();
        assert!(cfg.launch.width >= 640);
        assert!(cfg.overlay.fps_max >= 30);
        assert!(cfg.launch.proton == "auto");
    }

    #[test]
    fn roundtrip_toml() {
        let cfg = Config::default();
        let raw = toml::to_string_pretty(&cfg).unwrap();
        let back: Config = toml::from_str(&raw).unwrap();
        assert_eq!(cfg, back);
    }

    #[test]
    fn clamps_weird_values() {
        let mut cfg = Config::default();
        cfg.launch.fps_cap = 5;
        cfg.input.deadzone = 5.0;
        let cfg = cfg.normalized();
        assert_eq!(cfg.launch.fps_cap, 30);
        assert_eq!(cfg.input.deadzone, 0.6);
    }

    #[test]
    fn missing_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = Config::load(&dir.path().join("absent.toml")).unwrap();
        assert_eq!(cfg.general.lang, "fr");
    }
}
