//! Configuration MangoHud par jeu.

use crate::{Error, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct HudSettings {
    pub enabled: bool,
    pub fps_limit: Option<u32>,
    pub position: String,
    pub font_size: u32,
    pub gpu_stats: bool,
    pub cpu_stats: bool,
    pub temps: bool,
    pub ram: bool,
    pub vram: bool,
}

impl Default for HudSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            fps_limit: None,
            position: "top-left".into(),
            font_size: 20,
            gpu_stats: true,
            cpu_stats: true,
            temps: true,
            ram: true,
            vram: true,
        }
    }
}

pub fn config_text(settings: &HudSettings) -> String {
    let mut out = String::new();
    out.push_str("# Généré par Ludus\n");
    out.push_str(&format!("position={}\n", settings.position));
    out.push_str(&format!("font_size={}\n", settings.font_size.clamp(10, 48)));
    out.push_str("background_alpha=0.35\n");
    out.push_str("round_corners=8\n");
    out.push_str("toggle_hud=F12\n");
    if settings.gpu_stats {
        out.push_str("gpu_stats\n");
    }
    if settings.cpu_stats {
        out.push_str("cpu_stats\n");
    }
    if settings.temps {
        out.push_str("gpu_temp\ncpu_temp\n");
    }
    if settings.ram {
        out.push_str("ram\n");
    }
    if settings.vram {
        out.push_str("vram\n");
    }
    if let Some(limit) = settings.fps_limit.filter(|l| *l >= 5) {
        out.push_str("toggle_fps_limit=F11\n");
        out.push_str(&format!("fps_limit={limit}\n"));
    }
    out
}

/// Écrit `~/.config/MangoHud/<appid>.conf` et renvoie le chemin.
pub fn write_config(config_dir: &Path, appid: u32, settings: &HudSettings) -> Result<PathBuf> {
    let dir = config_dir.join("MangoHud");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{appid}.conf"));
    std::fs::write(&path, config_text(settings)).map_err(Error::Io)?;
    Ok(path)
}

pub fn env(config_file: &Path) -> Vec<(String, String)> {
    vec![
        ("MANGOHUD".to_string(), "1".to_string()),
        (
            "MANGOHUD_CONFIGFILE".to_string(),
            config_file.to_string_lossy().to_string(),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_contains_settings() {
        let settings = HudSettings {
            fps_limit: Some(60),
            font_size: 200,
            ..Default::default()
        };
        let text = config_text(&settings);
        assert!(text.contains("fps_limit=60"));
        assert!(text.contains("font_size=48"));
        assert!(text.contains("gpu_stats"));
        assert!(text.contains("toggle_hud=F12"));
    }

    #[test]
    fn disabled_stats_are_omitted() {
        let settings = HudSettings {
            gpu_stats: false,
            cpu_stats: false,
            ram: false,
            vram: false,
            temps: false,
            ..Default::default()
        };
        let text = config_text(&settings);
        assert!(!text.contains("gpu_stats"));
        assert!(!text.contains("cpu_stats"));
        assert!(!text.contains("ram"));
        assert!(!text.contains("fps_limit"));
    }

    #[test]
    fn writes_config_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(dir.path(), 440, &HudSettings::default()).unwrap();
        assert_eq!(path.file_name().unwrap(), "440.conf");
        assert!(std::fs::read_to_string(&path).unwrap().contains("position=top-left"));
        let env = env(&path);
        assert!(env.contains(&("MANGOHUD".into(), "1".into())));
    }
}
