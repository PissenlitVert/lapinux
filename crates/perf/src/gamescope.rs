//! Construction des arguments gamescope (validation sur la version installée).

#[derive(Debug, Clone, PartialEq)]
pub struct GamescopeSettings {
    pub enabled: bool,
    /// Connecteur de l'écran cible (vide = écran principal).
    pub output: String,
    pub width: u32,
    pub height: u32,
    /// Limite de FPS (0 = désactivée).
    pub fps_cap: u32,
    /// Fréquence de l'écran visé. Le compositeur imbriqué tourne à cette
    /// cadence, indépendamment de la limite : sans cela, relever la limite
    /// en cours de partie ne peut pas dépasser la cadence du lancement.
    pub refresh_hz: u32,
    pub hdr: bool,
    pub adaptive_sync: bool,
    pub fullscreen: bool,
    /// Utilise l'overlay MangoHud intégré à gamescope (`--mangoapp`).
    pub mangoapp: bool,
    /// Tube où gamescope écrit sa cadence réelle (`-T`) : une ligne `fps=`
    /// toutes les quelques centaines d'images. Renseigné par la session.
    pub stats_path: Option<String>,
}

impl Default for GamescopeSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            output: String::new(),
            width: 2560,
            height: 1440,
            fps_cap: 120,
            refresh_hz: 0,
            hdr: false,
            adaptive_sync: true,
            fullscreen: true,
            mangoapp: false,
            stats_path: None,
        }
    }
}

/// Cadence minimale du compositeur imbriqué quand ni l'écran ni la limite
/// ne sont connus : un `-r 0` serait refusé par GameScope.
const COMPOSITOR_FLOOR: u32 = 60;

/// Arguments passés à gamescope, sans le `--` ni la commande.
pub fn args(settings: &GamescopeSettings) -> Vec<String> {
    let mut args = Vec::new();
    let width = settings.width.max(320);
    let height = settings.height.max(240);
    args.extend(["-W".into(), width.to_string()]);
    args.extend(["-H".into(), height.to_string()]);
    args.extend(["-w".into(), width.to_string()]);
    args.extend(["-h".into(), height.to_string()]);
    if !settings.output.trim().is_empty() {
        // `--prefer-output` choisit le moniteur en plein écran.
        args.extend(["--prefer-output".into(), settings.output.trim().to_string()]);
    }
    // `-r` cale la fréquence du compositeur imbriqué. Une limite plus basse
    // que l'écran doit rester un diviseur entier de cette cadence ; une limite
    // égale ou supérieure n'a pas besoin de ce doublement : le compositeur
    // suit simplement l'écran et la limite ne peut de toute façon pas aller
    // au-delà. Sans écran connu, on garde l'ancien calcul pour que la limite
    // reste appliquée. Une marge fixe à 720 avait été utilisée ; elle bridait
    // certains jeux à 20-24 i/s, même sans `--framerate-limit`.
    let display = settings.refresh_hz.max(COMPOSITOR_FLOOR);
    let needs_divisor =
        settings.fps_cap > 0 && (settings.refresh_hz == 0 || settings.fps_cap < display);
    let refresh = if needs_divisor {
        let factor = (display / settings.fps_cap).max(1) + 1;
        factor.saturating_mul(settings.fps_cap).max(display)
    } else {
        display
    };
    args.extend(["-r".into(), refresh.to_string()]);
    // 15 i/s est un palier réel de l'overlay : GameScope sait le diviser comme
    // les autres, il ne faut donc pas le couper à 30.
    if needs_divisor {
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
    if let Some(path) = settings.stats_path.as_deref().filter(|path| !path.is_empty()) {
        args.extend(["-T".into(), path.to_string()]);
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
    fn output_is_forwarded_when_chosen() {
        let settings = GamescopeSettings {
            output: "DP-2".into(),
            ..Default::default()
        };
        let args = args(&settings);
        let index = args
            .iter()
            .position(|arg| arg == "--prefer-output")
            .expect("--prefer-output absent");
        assert_eq!(args[index + 1], "DP-2");
        let automatic = super::args(&GamescopeSettings::default());
        assert!(!automatic.contains(&"--prefer-output".to_string()));
    }

    #[test]
    fn refresh_is_the_smallest_multiple_of_the_cap_above_the_panel() {
        let settings = GamescopeSettings {
            fps_cap: 60,
            refresh_hz: 144,
            ..Default::default()
        };
        let args = args(&settings);
        let refresh = args.iter().position(|arg| arg == "-r").expect("-r absent");
        // 144 / 60 = 2, puis 3 × 60 = 180 : le diviseur reste entier.
        assert_eq!(args[refresh + 1], "180");
        let limit = args
            .iter()
            .position(|arg| arg == "--framerate-limit")
            .expect("--framerate-limit absent");
        assert_eq!(args[limit + 1], "60");
    }

    #[test]
    fn a_cap_equal_to_the_panel_uses_the_panel_rate() {
        let settings = GamescopeSettings {
            fps_cap: 165,
            refresh_hz: 165,
            ..Default::default()
        };
        let args = args(&settings);
        let refresh = args.iter().position(|arg| arg == "-r").expect("-r absent");
        // À cadence égale, le compositeur suit l'écran : inutile de le lancer
        // à 330 pour qu'un diviseur reste entier.
        assert_eq!(args[refresh + 1], "165");
        assert!(!args.contains(&"--framerate-limit".to_string()));
    }

    #[test]
    fn a_cap_above_the_panel_does_not_raise_the_compositor() {
        let settings = GamescopeSettings {
            fps_cap: 360,
            refresh_hz: 165,
            ..Default::default()
        };
        let args = args(&settings);
        let refresh = args.iter().position(|arg| arg == "-r").expect("-r absent");
        // Le moniteur ne peut pas afficher plus de 165 images : une limite
        // supérieure ne justifie pas de composer au-delà de sa cadence.
        assert_eq!(args[refresh + 1], "165");
        assert!(!args.contains(&"--framerate-limit".to_string()));
    }

    #[test]
    fn a_cap_below_the_panel_keeps_its_divisor() {
        let settings = GamescopeSettings {
            fps_cap: 120,
            refresh_hz: 165,
            ..Default::default()
        };
        let args = args(&settings);
        let refresh = args.iter().position(|arg| arg == "-r").expect("-r absent");
        assert_eq!(args[refresh + 1], "240");
        let limit = args
            .iter()
            .position(|arg| arg == "--framerate-limit")
            .expect("--framerate-limit absent");
        assert_eq!(args[limit + 1], "120");
    }

    #[test]
    fn unlimited_follows_the_panel() {
        let settings = GamescopeSettings {
            fps_cap: 0,
            refresh_hz: 165,
            ..Default::default()
        };
        let args = args(&settings);
        let refresh = args.iter().position(|arg| arg == "-r").expect("-r absent");
        assert_eq!(args[refresh + 1], "165");
        // Illimité : aucune limite n'est écrite, c'est le compositeur qui
        // décide de la cadence maximale.
        assert!(!args.contains(&"--framerate-limit".to_string()));
    }

    #[test]
    fn no_known_refresh_keeps_a_usable_compositor() {
        let settings = GamescopeSettings {
            fps_cap: 0,
            refresh_hz: 0,
            ..Default::default()
        };
        let args = args(&settings);
        let refresh = args.iter().position(|arg| arg == "-r").expect("-r absent");
        assert_eq!(args[refresh + 1], COMPOSITOR_FLOOR.to_string());
    }

    #[test]
    fn stats_pipe_is_forwarded() {
        let settings = GamescopeSettings {
            stats_path: Some("/run/user/1000/lapinux/fps.fifo".into()),
            ..Default::default()
        };
        let scope = args(&settings);
        let index = scope.iter().position(|arg| arg == "-T").expect("-T absent");
        assert_eq!(scope[index + 1], "/run/user/1000/lapinux/fps.fifo");
        assert!(!args(&GamescopeSettings::default()).contains(&"-T".to_string()));
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
