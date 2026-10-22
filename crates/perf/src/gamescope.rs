//! Construction des arguments gamescope (validation sur la version installée).

#[derive(Debug, Clone, PartialEq)]
pub struct GamescopeSettings {
    pub enabled: bool,
    pub width: u32,
    pub height: u32,
    /// Limite de FPS (0 = désactivée).
    pub fps_cap: u32,
    pub hdr: bool,
    pub adaptive_sync: bool,
    pub fullscreen: bool,
    /// Utilise l'overlay MangoHud intégré à gamescope (`--mangoapp`).
    pub mangoapp: bool,
}

impl Default for GamescopeSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            width: 2560,
            height: 1440,
            fps_cap: 120,
            hdr: false,
            adaptive_sync: true,
            fullscreen: true,
            mangoapp: false,
        }
    }
}

/// Arguments passés à gamescope, sans le `--` ni la commande.
pub fn args(settings: &GamescopeSettings) -> Vec<String> {
    let mut args = Vec::new();
    let width = settings.width.max(320);
    let height = settings.height.max(240);
    args.extend(["-W".into(), width.to_string()]);
    args.extend(["-H".into(), height.to_string()]);
    args.extend(["-w".into(), width.to_string()]);
    args.extend(["-h".into(), height.to_string()]);
    if settings.fps_cap >= 30 {
        // `-r` cale la fréquence du compositeur imbriqué.
        args.extend(["-r".into(), settings.fps_cap.to_string()]);
        args.extend(["--framerate-limit".into(), settings.fps_cap.to_string()]);
    }
    if settings.fullscreen {
        args.push("-f".into());
    }
    if settings.adaptive_sync {
        args.push("--adaptive-sync".into());
    }
    if settings.hdr {
        args.push("--hdr-enabled".into());
    }
    if settings.mangoapp {
        args.push("--mangoapp".into());
    }
    args.push("--force-grab-cursor".into());
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_args_include_resolution_and_cap() {
        let args = args(&GamescopeSettings::default());
        assert_eq!(args[0], "-W");
        assert_eq!(args[1], "2560");
        assert!(args.contains(&"--framerate-limit".to_string()));
        assert!(args.contains(&"120".to_string()));
        assert!(args.contains(&"-f".to_string()));
        assert!(args.contains(&"--adaptive-sync".to_string()));
        assert!(!args.contains(&"--hdr-enabled".to_string()));
    }

    #[test]
    fn hdr_and_mangoapp_flags() {
        let settings = GamescopeSettings {
            hdr: true,
            mangoapp: true,
            fps_cap: 0,
            fullscreen: false,
            ..Default::default()
        };
        let args = args(&settings);
        assert!(args.contains(&"--hdr-enabled".to_string()));
        assert!(args.contains(&"--mangoapp".to_string()));
        assert!(!args.contains(&"--framerate-limit".to_string()));
        assert!(!args.contains(&"-f".to_string()));
    }

    #[test]
    fn tiny_dimensions_are_clamped() {
        let settings = GamescopeSettings {
            width: 10,
            height: 10,
            ..Default::default()
        };
        let args = args(&settings);
        assert_eq!(args[1], "320");
        assert_eq!(args[3], "240");
    }
}
