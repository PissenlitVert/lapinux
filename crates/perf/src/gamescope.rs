//! Construction des arguments gamescope (validation sur la version installée).

#[derive(Debug, Clone, PartialEq)]
pub struct GamescopeSettings {
    pub enabled: bool,
    /// Connecteur de l'écran cible (vide = écran principal).
    pub output: String,
    pub width: u32,
    pub height: u32,
    /// Résolution demandée au jeu ; zéro conserve la résolution de sortie.
    pub game_width: u32,
    pub game_height: u32,
    pub filter: lapinux_core::config::ScalingFilter,
    pub sharpness: u32,
    /// Limite de FPS (0 = désactivée).
    pub fps_cap: u32,
    /// Fréquence de l'écran visé, pour que l'overlay connaisse la cadence
    /// physique. La cadence du compositeur (`-r`) est calculée séparément :
    /// elle reste au-dessus de l'écran pour permettre de couper la limite
    /// en cours de partie.
    pub refresh_hz: u32,
    pub hdr: bool,
    pub adaptive_sync: bool,
    pub fullscreen: bool,
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
            game_width: 0,
            game_height: 0,
            filter: lapinux_core::config::ScalingFilter::Auto,
            sharpness: 2,
            fps_cap: 120,
            refresh_hz: 0,
            hdr: false,
            adaptive_sync: true,
            fullscreen: true,
            stats_path: None,
        }
    }
}

/// Plancher de cadence du compositeur imbriqué quand la limite est coupée.
///
/// `-r` n'est pas un simple indice : pour un jeu synchronisé, c'est un vrai
/// plafond (vérifié avec GameScope 3.17 : `-r 60` bloque à 60 i/s). « Illimité »
/// doit donc lancer le compositeur au-dessus de l'écran, sinon le jeu reste
/// bridé à sa fréquence. Cette valeur couvre aussi les paliers personnalisés
/// de l'interface, qui montent jusqu'à 1000.
const UNLIMITED_COMPOSITOR_HZ: u32 = 1000;

/// Arguments passés à gamescope, sans le `--` ni la commande.
pub fn args(settings: &GamescopeSettings) -> Vec<String> {
    let mut args = Vec::new();
    let width = settings.width.max(320);
    let height = settings.height.max(240);
    args.extend(["-W".into(), width.to_string()]);
    args.extend(["-H".into(), height.to_string()]);
    let (game_width, game_height) = if settings.game_width >= 320 && settings.game_height >= 240 {
        (settings.game_width, settings.game_height)
    } else {
        (width, height)
    };
    args.extend(["-w".into(), game_width.to_string()]);
    args.extend(["-h".into(), game_height.to_string()]);
    use lapinux_core::config::ScalingFilter;
    let filter = match settings.filter {
        ScalingFilter::Auto => None,
        ScalingFilter::Fsr => Some("fsr"),
        ScalingFilter::Nis => Some("nis"),
        ScalingFilter::Linear => Some("linear"),
        ScalingFilter::Nearest => Some("nearest"),
        ScalingFilter::Pixel => Some("pixel"),
    };
    if let Some(filter) = filter {
        args.extend(["-F".into(), filter.into()]);
        if filter == "fsr" || filter == "nis" {
            args.extend(["--sharpness".into(), settings.sharpness.min(20).to_string()]);
        }
    }
    if !settings.output.trim().is_empty() {
        // `--prefer-output` choisit le moniteur en plein écran.
        args.extend(["--prefer-output".into(), settings.output.trim().to_string()]);
    }
    // `-r` cale la fréquence du compositeur imbriqué. C'est aussi un plafond
    // pour un jeu synchronisé : le lancer à la fréquence de l'écran rendrait
    // « illimité » impossible, et empêcherait de relever la limite à chaud.
    // On choisit donc un multiple entier de la limite, au-dessus de
    // `UNLIMITED_COMPOSITOR_HZ`, pour laisser la place de changer la limite
    // en cours de partie. Sans limite, le compositeur monte au plancher.
    let refresh = if settings.fps_cap == 0 {
        UNLIMITED_COMPOSITOR_HZ
    } else {
        let factor = (UNLIMITED_COMPOSITOR_HZ + settings.fps_cap - 1) / settings.fps_cap;
        factor.max(1).saturating_mul(settings.fps_cap)
    };
    args.extend(["-r".into(), refresh.to_string()]);
    // Aucun plafond statique n'est transmis : `--framerate-limit` est une
    // division de la cadence du compositeur, figée pour toute la partie. Le
    // daemon pose la limite avec `gamescopectl debug_set_fps_limit` dès que
    // GameScope répond, puis à chaque changement fait dans l'overlay. C'est ce
    // qui rend la limite réglable en cours de partie, génération d'images
    // comprise, et c'est aussi ce qui permet de la couper (0).
    if settings.fullscreen {
        args.push("-f".into());
    }
    if settings.adaptive_sync {
        args.push("--adaptive-sync".into());
    }
    if settings.hdr {
        args.push("--hdr-enabled".into());
    }
    if let Some(path) = settings
        .stats_path
        .as_deref()
        .filter(|path| !path.is_empty())
    {
        args.extend(["-T".into(), path.to_string()]);
    }
    args.push("--force-grab-cursor".into());
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_args_include_resolution_and_start_unlimited() {
        let args = args(&GamescopeSettings::default());
        assert_eq!(args[0], "-W");
        assert_eq!(args[1], "2560");
        // Le plafond n'est jamais passé au lancement : il est posé à chaud.
        assert!(!args.contains(&"--framerate-limit".to_string()));
        assert!(args.contains(&"-f".to_string()));
        assert!(args.contains(&"--adaptive-sync".to_string()));
        assert!(!args.contains(&"--hdr-enabled".to_string()));
    }

    #[test]
    fn scaling_uses_separate_game_resolution_and_filter() {
        use lapinux_core::config::ScalingFilter;
        let settings = GamescopeSettings {
            width: 1920,
            height: 1080,
            game_width: 1280,
            game_height: 720,
            filter: ScalingFilter::Fsr,
            sharpness: 4,
            ..Default::default()
        };
        let args = args(&settings);
        assert_eq!(
            &args[..8],
            ["-W", "1920", "-H", "1080", "-w", "1280", "-h", "720"]
        );
        assert!(args.windows(2).any(|v| v == ["-F", "fsr"]));
        assert!(args.windows(2).any(|v| v == ["--sharpness", "4"]));
    }

    #[test]
    fn incomplete_game_resolution_keeps_output_resolution() {
        let settings = GamescopeSettings {
            game_width: 1280,
            game_height: 0,
            ..Default::default()
        };
        let args = args(&settings);
        assert_eq!(
            &args[..8],
            ["-W", "2560", "-H", "1440", "-w", "2560", "-h", "1440"]
        );
        assert!(!args.iter().any(|v| v == "-F"));
    }

    #[test]
    fn hdr_flag() {
        let settings = GamescopeSettings {
            hdr: true,
            fps_cap: 0,
            fullscreen: false,
            ..Default::default()
        };
        let args = args(&settings);
        assert!(args.contains(&"--hdr-enabled".to_string()));
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
    fn each_cap_gets_a_compositor_multiple_above_the_unlimited_floor() {
        for cap in [15, 30, 60, 90, 120, 144, 165, 240, 360, 1000] {
            let settings = GamescopeSettings {
                fps_cap: cap,
                refresh_hz: 165,
                ..Default::default()
            };
            let args = args(&settings);
            let refresh = args.iter().position(|arg| arg == "-r").expect("-r absent");
            let refresh: u32 = args[refresh + 1].parse().unwrap();
            assert!(refresh >= UNLIMITED_COMPOSITOR_HZ, "{cap} : {refresh}");
            assert_eq!(refresh % cap, 0, "le multiple doit être entier pour {cap}");
            assert!(!args.contains(&"--framerate-limit".to_string()));
        }
    }

    #[test]
    fn unlimited_raises_the_compositor_above_the_panel() {
        let settings = GamescopeSettings {
            fps_cap: 0,
            refresh_hz: 165,
            ..Default::default()
        };
        let args = args(&settings);
        let refresh = args.iter().position(|arg| arg == "-r").expect("-r absent");
        // Un jeu synchronisé suit `-r` : le laisser à 165 briderait
        // « illimité » à la fréquence de l'écran.
        assert_eq!(args[refresh + 1], UNLIMITED_COMPOSITOR_HZ.to_string());
        assert!(!args.contains(&"--framerate-limit".to_string()));
    }

    #[test]
    fn no_known_refresh_still_gets_an_unlimited_compositor() {
        let settings = GamescopeSettings {
            fps_cap: 0,
            refresh_hz: 0,
            ..Default::default()
        };
        let args = args(&settings);
        let refresh = args.iter().position(|arg| arg == "-r").expect("-r absent");
        assert_eq!(args[refresh + 1], UNLIMITED_COMPOSITOR_HZ.to_string());
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
