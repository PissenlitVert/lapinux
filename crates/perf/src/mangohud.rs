//! Configuration MangoHud par jeu.

use crate::{Error, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

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
    /// Nom complet de la carte graphique, en tête du bloc GPU. MangoApp ne
    /// le remplit pas ; `gpu_text` prend le relais sous GameScope.
    pub gpu_name: bool,
    /// Libellé court de la carte graphique, affiché à la place de « GPU ».
    pub gpu_text: Option<String>,
    /// Nom du processeur, écrit dans `cpu_text` (MangoHud n'a pas de ligne
    /// dédiée pour le modèle). Raccourci pour ne pas étirer le HUD.
    pub cpu_text: Option<String>,
    /// Une seule ligne au lieu du bloc vertical.
    pub horizontal: bool,
    /// Cadence affichée en clair.
    pub fps: bool,
    /// Courbe de frametime, activée séparément du profil par l'overlay.
    pub frame_timing: bool,
    /// Charge de chaque cœur du processeur, demandée par l'overlay.
    pub cpu_cores: bool,
    /// Fréquences et puissance : le supplément du profil détaillé.
    pub clocks: bool,
    /// Taille de l'image du jeu, quand elle est connue. Elle sert à centrer
    /// le bloc verticalement ; elle n'est jamais écrite telle quelle.
    pub screen: Option<(u32, u32)>,
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
            gpu_name: true,
            gpu_text: None,
            cpu_text: None,
            horizontal: false,
            fps: true,
            frame_timing: false,
            cpu_cores: false,
            clocks: false,
            screen: None,
        }
    }
}

/// Positions acceptées par MangoHud : les neuf ancrages de l'écran.
pub const POSITIONS: [&str; 9] = [
    "top-left",
    "top-center",
    "top-right",
    "middle-left",
    "middle-center",
    "middle-right",
    "bottom-left",
    "bottom-center",
    "bottom-right",
];

/// Ramène une position à l'une des neuf valeurs connues ; une valeur vide ou
/// inconnue retombe en haut à gauche comme MangoHud.
pub fn normalize_position(position: &str) -> String {
    let candidate = position.trim().to_ascii_lowercase().replace('_', "-");
    let candidate = match candidate.as_str() {
        "center" | "centre" => "middle-center".to_string(),
        other => other.to_string(),
    };
    if POSITIONS.contains(&candidate.as_str()) {
        candidate
    } else {
        "top-left".into()
    }
}

/// Modèle du processeur, pour `cpu_text`. MangoHud n'affiche pas le nom du CPU
/// de lui-même ; lire `/proc/cpuinfo` évite une dépendance et reste valable
/// sous Proton. Le nom est raccourci : « AMD Ryzen 7 5800X3D 8-Core
/// Processor » devient « Ryzen 7 5800X3D ».
pub fn cpu_label() -> Option<String> {
    static CPU: OnceLock<Option<String>> = OnceLock::new();
    CPU.get_or_init(detect_cpu_label).clone()
}

fn detect_cpu_label() -> Option<String> {
    let text = std::fs::read_to_string("/proc/cpuinfo").ok()?;
    let model = text.lines().find_map(|line| {
        line.strip_prefix("model name")
            .and_then(|rest| rest.split_once(':'))
            .map(|(_, value)| value.trim())
    })?;
    short_cpu_name(model)
}

/// Nom public de la carte graphique, raccourci pour le HUD. Le résultat est
/// mis en cache : sous GameScope, MangoApp ne renseigne pas `gpu_name`, et un
/// `gpu_text` explicite est le seul moyen fiable d'afficher le modèle.
pub fn gpu_label() -> Option<String> {
    static GPU: OnceLock<Option<String>> = OnceLock::new();
    GPU.get_or_init(detect_gpu_label).clone()
}

fn detect_gpu_label() -> Option<String> {
    // NVIDIA expose un nom marketing propre, sans dépendre de pciutils.
    if let Ok(output) = Command::new("nvidia-smi")
        .args(["--query-gpu=name", "--format=csv,noheader"])
        .output()
    {
        if output.status.success() {
            if let Some(line) = String::from_utf8_lossy(&output.stdout).lines().next() {
                if let Some(name) = short_gpu_name(line) {
                    return Some(name);
                }
            }
        }
    }
    // AMD et Intel : `lspci -mm` fournit le nom du modèle entre guillemets.
    let output = Command::new("lspci").arg("-mm").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        let class = line.to_ascii_lowercase();
        if !(class.contains("vga compatible controller")
            || class.contains("3d controller")
            || class.contains("display controller"))
        {
            continue;
        }
        // `lspci -mm` : "classe" "vendeur" "modèle".
        let quoted: Vec<&str> = line.split('"').collect();
        if let Some(device) = quoted.get(5) {
            if let Some(name) = short_gpu_name(device) {
                return Some(name);
            }
        }
    }
    None
}

fn short_cpu_name(raw: &str) -> Option<String> {
    let mut name = raw
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect::<String>();
    // « … @ 3.60GHz » ne sert à rien dans le HUD.
    if let Some((head, _)) = name.split_once('@') {
        name = head.to_string();
    }
    name = name.trim().to_string();
    for suffix in [" CPU", " Processor"] {
        if let Some(stripped) = name.strip_suffix(suffix) {
            name = stripped.to_string();
        }
    }
    // « 8-Core » décrit le nombre de cœurs, pas le modèle.
    let mut words: Vec<&str> = name.split_whitespace().collect();
    if words
        .last()
        .is_some_and(|word| word.ends_with("-Core"))
    {
        words.pop();
    }
    name = words.join(" ");
    for prefix in [
        "AMD ",
        "Intel(R) ",
        "Intel ",
        "Genuine Intel(R) ",
        "Genuine Intel ",
    ] {
        if let Some(stripped) = name.strip_prefix(prefix) {
            name = stripped.to_string();
        }
    }
    name = name.replace("(R)", "").replace("(TM)", "");
    name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() {
        None
    } else {
        Some(name.chars().take(28).collect())
    }
}

fn short_gpu_name(raw: &str) -> Option<String> {
    let mut name = raw.trim().to_string();
    // « AD104 [GeForce RTX 4070] » : le nom commercial est entre crochets.
    if let Some(open) = name.rfind('[') {
        if let Some(close) = name[open..].find(']') {
            name = name[open + 1..open + close].to_string();
        }
    }
    for prefix in [
        "NVIDIA ",
        "GeForce ",
        "AMD ",
        "ATI ",
        "Advanced Micro Devices, Inc. ",
        "Intel(R) ",
        "Intel ",
    ] {
        if let Some(stripped) = name.strip_prefix(prefix) {
            name = stripped.to_string();
        }
    }
    name = name
        .replace("(R)", "")
        .replace("(TM)", "")
        .replace(" Graphics", "");
    name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() {
        None
    } else {
        Some(name.chars().take(28).collect())
    }
}

/// Réglages d'un préréglage nommé, tel que l'overlay le propose : `simple`
/// (cadence, GPU, CPU, températures), `essential` (la mémoire en plus) et
/// `full` (VRAM, fréquences et puissance). Les trois portent le nom du GPU et
/// du CPU ; tout le reste vaut arrêté.
pub fn preset_settings_at(preset: &str, fps_limit: Option<u32>, position: &str) -> HudSettings {
    let gpu = gpu_label();
    let base = HudSettings {
        position: normalize_position(position),
        // Un libellé explicite traverse MangoApp ; `gpu_name` seul reste vide
        // sous GameScope et n'affiche donc aucun modèle.
        gpu_text: gpu.clone(),
        gpu_name: gpu.is_none(),
        cpu_text: cpu_label(),
        ..Default::default()
    };
    match preset.trim().to_ascii_lowercase().as_str() {
        "simple" => HudSettings {
            enabled: true,
            fps_limit,
            font_size: 16,
            gpu_stats: true,
            cpu_stats: true,
            temps: true,
            ram: false,
            vram: false,
            fps: true,
            clocks: false,
            ..base
        },
        "essential" | "basic" => HudSettings {
            enabled: true,
            fps_limit,
            font_size: 18,
            gpu_stats: true,
            cpu_stats: true,
            temps: true,
            ram: true,
            vram: false,
            fps: true,
            clocks: false,
            ..base
        },
        "full" | "detailed" => HudSettings {
            enabled: true,
            fps_limit,
            font_size: 20,
            gpu_stats: true,
            cpu_stats: true,
            temps: true,
            ram: true,
            vram: true,
            fps: true,
            clocks: true,
            ..base
        },
        _ => HudSettings {
            enabled: false,
            fps_limit,
            ..base
        },
    }
}

/// Préréglage à la position par défaut, pour les appelants qui n'en choisissent
/// pas (tests, anciens chemins).
pub fn preset_settings(preset: &str, fps_limit: Option<u32>) -> HudSettings {
    preset_settings_at(preset, fps_limit, "top-left")
}

pub fn config_text(settings: &HudSettings) -> String {
    let mut out = String::new();
    out.push_str(&format!("# Généré par {}\n", lapinux_core::APP_NAME));
    let position = normalize_position(&settings.position);
    // La couche MangoHud vendue par Lapinux (`packaging/mangohud`) ancre la
    // ligne sur la vraie taille de la fenêtre du HUD : les neuf positions
    // tombent juste, y compris la droite et le centre. Le repli en bloc ne
    // sert plus qu'aux configurations sans cette couche, où MangoHud 0.8 sort
    // de l'écran — le daemon l'évite alors au lancement.
    let inline = settings.horizontal;
    if inline {
        out.push_str(&format!("position={position}\n"));
    } else if position == "middle-center" {
        // Pas de position « centre » dans MangoHud : on centre horizontalement
        // avec `top-center`, puis on descend le bloc de la moitié de l'écran.
        out.push_str("position=top-center\n");
        if let Some((_, screen_h)) = settings.screen {
            let offset = (screen_h.saturating_sub(block_height(settings)) / 2).saturating_sub(10);
            out.push_str(&format!("offset_y={offset}\n"));
        }
    } else {
        out.push_str(&format!("position={position}\n"));
    }
    out.push_str(&format!("font_size={}\n", settings.font_size.clamp(10, 48)));
    // MangoHud relit ce fichier à chaud : `no_display` suit l'interrupteur de
    // l'overlay, qui peut donc masquer puis réafficher le HUD sans relancer
    // le jeu. La valeur 0 doit être explicite : retirer la ligne ne remet pas
    // le paramètre à zéro lors d'un rechargement.
    out.push_str(if settings.enabled {
        "no_display=0\n"
    } else {
        "no_display=1\n"
    });
    if inline {
        // Par défaut MangoHud étire la ligne sur toute la largeur de l'écran.
        // `horizontal_stretch=0` laisse la fenêtre prendre la largeur de son
        // contenu : elle s'arrête après la dernière statistique.
        out.push_str("horizontal\nhorizontal_stretch=0\n");
    } else {
        // Sans largeur explicite, MangoHud dimensionne le bloc sur trois
        // colonnes de texte : un nom de GPU ou de CPU s'y entasse. La hauteur
        // sert aussi à centrer le bloc verticalement.
        out.push_str(&format!("width={}\n", block_width(settings)));
        out.push_str(&format!("height={}\n", block_height(settings)));
        // Les lignes se touchent avec la valeur par défaut (-0,085).
        out.push_str("cellpadding_y=0.4\n");
    }
    out.push_str("background_alpha=0.35\n");
    out.push_str("round_corners=8\n");
    out.push_str("toggle_hud=F12\n");
    if settings.fps {
        out.push_str("fps\n");
    }
    if settings.gpu_stats {
        out.push_str("gpu_stats\n");
    }
    if let Some(label) = settings
        .gpu_text
        .as_deref()
        .map(str::trim)
        .filter(|label| !label.is_empty())
    {
        out.push_str(&format!("gpu_text={label}\n"));
    }
    if settings.gpu_name {
        out.push_str("gpu_name\n");
    }
    if settings.cpu_stats {
        out.push_str("cpu_stats\n");
    }
    if let Some(label) = settings
        .cpu_text
        .as_deref()
        .map(str::trim)
        .filter(|label| !label.is_empty())
    {
        out.push_str(&format!("cpu_text={label}\n"));
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
    // La valeur 0 est explicite : MangoHud relit ce fichier à chaud, et
    // retirer la ligne laisserait le graphique allumé. L'interrupteur de
    // l'overlay peut donc l'éteindre en cours de partie.
    out.push_str(if settings.frame_timing {
        "frame_timing=1\n"
    } else {
        "frame_timing=0\n"
    });
    // `core_load` dessine une barre par cœur ; il se relit à chaud comme le
    // reste du fichier. La ligne explicite à 0 coupe le retour en arrière.
    out.push_str(if settings.cpu_cores {
        "core_load=1\n"
    } else {
        "core_load=0\n"
    });
    if settings.clocks {
        out.push_str("gpu_core_clock\ngpu_mem_clock\ngpu_power\ncpu_mhz\n");
    }
    // La limite de FPS appartient à GameScope, pas à MangoHud. La couche
    // MangoHud acceptait `fps_limit` mais bridait ici un jeu à 24 i/s alors
    // que la limite demandée était de 165, même sans GameScope. Le HUD ne
    // doit donc jamais poser de plafond : il ne fait qu'afficher les mesures.
    out
}

/// Largeur du bloc : de quoi afficher un nom de GPU ou de CPU sans le
/// tronquer, pour la taille de police choisie.
fn block_width(settings: &HudSettings) -> u32 {
    (settings.font_size as f32 * 20.0)
        .round()
        .clamp(260.0, 520.0) as u32
}

/// Hauteur estimée du bloc, une ligne par statistique affichée : MangoHud
/// s'en sert pour son ancrage avant la première image.
fn block_height(settings: &HudSettings) -> u32 {
    let mut rows = 0;
    if settings.fps {
        rows += 1;
    }
    if settings.gpu_stats {
        rows += 1;
    }
    if settings.cpu_stats {
        rows += 1;
    }
    if settings.ram {
        rows += 1;
    }
    if settings.vram {
        rows += 1;
    }
    if settings.clocks {
        rows += 4;
    }
    rows.max(1) * (settings.font_size + 4) + 24
}

/// Nom du manifeste vendu par Lapinux. Distinct de celui du système : le
/// loader peut ainsi désactiver la couche d'origine par son nom exact sans
/// couper celle qui corrige les ancrages.
pub const VENDORED_LAYER_NAME: &str = "VK_LAYER_LAPINUX_MANGOHUD_overlay_x86_64";

/// Couche MangoHud installée par la distribution.
const SYSTEM_LAYER_NAME: &str = "VK_LAYER_MANGOHUD_overlay_x86_64";

/// Dossier où Lapinux range la couche MangoHud corrigée.
pub fn vendor_layer_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("mangohud")
}

/// Installe le manifeste à côté de la bibliothèque, quand celle-ci existe, et
/// renvoie le dossier à ajouter aux chemins du loader.
///
/// La bibliothèque est un build de MangoHud 0.8.4 avec la position du HUD en
/// ligne corrigée (`packaging/mangohud`) ; sans elle, rien n'est déclaré et
/// MangoHud du système reste seul en jeu.
pub fn ensure_vendor_layer(data_dir: &Path) -> Option<PathBuf> {
    let dir = vendor_layer_dir(data_dir);
    let library = dir.join("libMangoHud.so");
    if !library.is_file() {
        return None;
    }
    let manifest = dir.join("lapinux-mangohud.json");
    let library = json_escape(&library.to_string_lossy());
    let text = format!(
        r#"{{
  "file_format_version": "1.0.0",
  "layer": {{
    "name": "{VENDORED_LAYER_NAME}",
    "type": "GLOBAL",
    "api_version": "1.3.0",
    "library_path": "{library}",
    "implementation_version": "1",
    "description": "MangoHud (Lapinux) : ancrages en ligne corrigés",
    "functions": {{
      "vkNegotiateLoaderLayerInterfaceVersion": "vkNegotiateLoaderLayerInterfaceVersion"
    }},
    "enable_environment": {{ "MANGOHUD": "1" }},
    "disable_environment": {{ "DISABLE_MANGOHUD": "1" }}
  }}
}}
"#
    );
    if std::fs::read_to_string(&manifest).ok().as_deref() != Some(text.as_str()) {
        let _ = std::fs::write(&manifest, text);
    }
    Some(dir)
}

fn json_escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Ajoute un élément à une liste séparée, sans doublon.
fn merge_list(existing: &str, item: &str, separator: char) -> String {
    let already = existing
        .split(separator)
        .any(|entry| entry.trim() == item);
    if already {
        return existing.to_string();
    }
    if existing.trim().is_empty() {
        item.to_string()
    } else {
        format!("{existing}{separator}{item}")
    }
}

/// Variables qui font charger la couche corrigée à la place de celle du
/// système : `VK_ADD_IMPLICIT_LAYER_PATH` est le seul chemin honoré par le
/// loader pour une couche implicite, et `VK_LOADER_LAYERS_DISABLE` coupe
/// MangoHud d'origine par son nom. Rien n'est renvoyé sans bibliothèque.
pub fn vendor_env(data_dir: &Path) -> Vec<(String, String)> {
    let Some(dir) = ensure_vendor_layer(data_dir) else {
        return Vec::new();
    };
    let dir = dir.to_string_lossy().to_string();
    let existing_path = std::env::var("VK_ADD_IMPLICIT_LAYER_PATH").unwrap_or_default();
    let existing_disable = std::env::var("VK_LOADER_LAYERS_DISABLE").unwrap_or_default();
    vec![
        (
            "VK_ADD_IMPLICIT_LAYER_PATH".to_string(),
            merge_list(&existing_path, &dir, ':'),
        ),
        (
            "VK_LOADER_LAYERS_DISABLE".to_string(),
            merge_list(&existing_disable, SYSTEM_LAYER_NAME, ','),
        ),
    ]
}

/// Écrit `~/.config/MangoHud/<appid>.conf` et renvoie le chemin.
pub fn write_config(config_dir: &Path, appid: u32, settings: &HudSettings) -> Result<PathBuf> {
    let dir = config_dir.join("MangoHud");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{appid}.conf"));
    std::fs::write(&path, config_text(settings)).map_err(Error::Io)?;
    Ok(path)
}

/// Cache ou remontre le HUD pendant une capture. MangoHud dessine dans
/// l'image même du jeu : le masquer est le seul moyen d'obtenir une vidéo
/// propre, et `mangohudctl` parle au HUD en cours par son socket, sans
/// relancer le jeu.
pub fn set_display(visible: bool) -> std::result::Result<(), String> {
    let Some(ctl) = lapinux_core::paths::which("mangohudctl") else {
        return Err("mangohudctl absent : le HUD restera dans la vidéo".into());
    };
    let output = std::process::Command::new(ctl)
        .arg("set")
        .arg("no_display")
        .arg(if visible { "false" } else { "true" })
        .output()
        .map_err(|err| format!("mangohudctl : {err}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "mangohudctl : {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

pub fn env(config_file: &Path) -> Vec<(String, String)> {
    vec![
        ("MANGOHUD".to_string(), "1".to_string()),
        config_env(config_file),
    ]
}

/// Seule la configuration, sans activer la couche MangoHud : MangoApp de
/// GameScope lit ce chemin sans dessiner un second HUD dans le jeu.
pub fn config_env(config_file: &Path) -> (String, String) {
    (
        "MANGOHUD_CONFIGFILE".to_string(),
        config_file.to_string_lossy().to_string(),
    )
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
        // Le HUD ne borne jamais la cadence : GameScope s'en charge, et le
        // limiteur de MangoHud bridait certains jeux à 24 i/s.
        assert!(!text.contains("fps_limit"));
        assert!(text.contains("font_size=48"));
        assert!(text.contains("gpu_stats"));
        assert!(text.contains("toggle_hud=F12"));
        assert!(text.contains("gpu_name"));
    }

    #[test]
    fn presets_differ_in_what_they_show() {
        let simple = preset_settings("simple", Some(60));
        assert!(simple.enabled && simple.gpu_stats && simple.cpu_stats && !simple.vram);
        assert!(!simple.ram && !simple.clocks);
        let basic = preset_settings("essential", Some(60));
        assert!(basic.enabled && basic.gpu_stats && !basic.vram);
        assert!(basic.ram);
        let detailed = preset_settings("full", Some(60));
        assert!(detailed.enabled && detailed.vram && detailed.clocks);
        let text = config_text(&detailed);
        assert!(text.contains("vram"));
        assert!(text.contains("gpu_core_clock"));
        // La courbe de frametime reste un réglage séparé, coupé par défaut —
        // écrit explicitement pour que MangoHud puisse l'éteindre à chaud.
        assert!(text.contains("frame_timing=0"));
        assert!(!text.contains("frame_timing=1"));
        let off = preset_settings("off", Some(60));
        assert!(!off.enabled);
    }

    #[test]
    fn every_profile_is_placed_and_names_its_hardware() {
        for preset in ["simple", "essential", "full"] {
            let settings = preset_settings_at(preset, Some(60), "bottom-center");
            let text = config_text(&settings);
            assert!(text.contains("position=bottom-center"), "{preset}");
            assert!(
                text.contains("gpu_text=") || text.contains("gpu_name"),
                "{preset} : aucun nom de GPU"
            );
            assert!(text.contains("cpu_text="), "{preset}");
            assert!(text.contains("gpu_temp"), "{preset}");
            assert!(text.contains("cpu_temp"), "{preset}");
        }
    }

    #[test]
    fn unknown_positions_fall_back_to_top_left() {
        assert_eq!(normalize_position("somewhere"), "top-left");
        assert_eq!(normalize_position(""), "top-left");
        assert_eq!(normalize_position("middle_center"), "middle-center");
        assert_eq!(normalize_position("centre"), "middle-center");
    }

    #[test]
    fn hud_never_limits_the_frame_rate() {
        let settings = preset_settings_at("full", Some(165), "top-left");
        let text = config_text(&settings);
        assert!(
            !text.contains("fps_limit") && !text.contains("toggle_fps_limit"),
            "MangoHud ne doit pas doubler la limite de GameScope : {text}"
        );
    }

    #[test]
    fn disabled_stats_are_omitted() {
        let settings = HudSettings {
            gpu_stats: false,
            cpu_stats: false,
            ram: false,
            vram: false,
            temps: false,
            gpu_name: false,
            fps: false,
            ..Default::default()
        };
        let text = config_text(&settings);
        assert!(!text.contains("gpu_stats"));
        assert!(!text.contains("cpu_stats"));
        // « ram » seul : « frame_timing » contient les mêmes lettres.
        assert!(!text.contains("ram\n"));
        assert!(!text.contains("fps\n"));
        assert!(!text.contains("fps_limit"));
    }

    #[test]
    fn horizontal_layout_is_written() {
        let mut settings = HudSettings::default();
        assert!(!config_text(&settings).contains("horizontal"));
        settings.horizontal = true;
        let text = config_text(&settings);
        assert!(text.contains("horizontal\n"));
        // La ligne ne doit pas être étirée sur toute la largeur de l'écran.
        assert!(text.contains("horizontal_stretch=0"));
    }

    #[test]
    fn block_is_sized_for_hardware_names() {
        let settings = preset_settings_at("full", Some(60), "top-left");
        let text = config_text(&settings);
        let width: u32 = text
            .lines()
            .find_map(|line| line.strip_prefix("width="))
            .and_then(|value| value.parse().ok())
            .expect("largeur absente");
        // Trois colonnes de texte feraient 248 px : trop étroit pour un nom
        // de processeur suivi de sa charge et de sa température.
        assert!(width >= 320, "largeur {width}");
        assert!(text.contains("height="), "hauteur absente");
        assert!(text.contains("cellpadding_y=0.4"));
    }

    #[test]
    fn centre_uses_a_top_center_anchor_and_an_offset() {
        let mut settings = preset_settings_at("essential", Some(60), "middle-center");
        settings.screen = Some((1920, 1080));
        let text = config_text(&settings);
        assert!(text.contains("position=top-center"));
        let offset: u32 = text
            .lines()
            .find_map(|line| line.strip_prefix("offset_y="))
            .and_then(|value| value.parse().ok())
            .expect("décalage vertical absent pour le centre");
        assert!(offset > 300, "décalage {offset}");
        // Sans taille d'écran connue, le bloc reste centré en haut.
        assert!(!config_text(&preset_settings_at("essential", Some(60), "middle-center"))
            .contains("offset_y"));
    }

    #[test]
    fn inline_hud_keeps_every_anchor() {
        for position in [
            "top-left",
            "top-center",
            "top-right",
            "middle-left",
            "middle-center",
            "middle-right",
            "bottom-left",
            "bottom-center",
            "bottom-right",
        ] {
            let mut settings = preset_settings_at("simple", Some(60), position);
            settings.horizontal = true;
            let text = config_text(&settings);
            assert!(text.contains(&format!("position={position}")), "{position}");
            assert!(text.contains("horizontal\n"), "{position}");
            // Pas de dimensions de bloc en ligne : elles n'ont pas de sens.
            assert!(!text.contains("width="), "{position}");
        }
    }

    #[test]
    fn graph_is_independent_from_the_profile() {
        let mut settings = preset_settings("simple", Some(60));
        assert!(config_text(&settings).contains("frame_timing=0"));
        settings.frame_timing = true;
        assert!(config_text(&settings).contains("frame_timing=1"));
    }

    #[test]
    fn visibility_follows_the_enabled_switch() {
        let mut settings = preset_settings("simple", Some(60));
        assert!(config_text(&settings).contains("no_display=0"));
        settings.enabled = false;
        assert!(config_text(&settings).contains("no_display=1"));
    }

    #[test]
    fn hardware_labels_are_short() {
        assert_eq!(
            short_cpu_name("AMD Ryzen 7 5800X3D 8-Core Processor").as_deref(),
            Some("Ryzen 7 5800X3D")
        );
        assert_eq!(
            short_cpu_name("Intel(R) Core(TM) i7-12700K CPU @ 3.60GHz").as_deref(),
            Some("Core i7-12700K")
        );
        assert_eq!(
            short_gpu_name("NVIDIA GeForce RTX 4070").as_deref(),
            Some("RTX 4070")
        );
        assert_eq!(
            short_gpu_name("AD104 [GeForce RTX 4070]").as_deref(),
            Some("RTX 4070")
        );
        assert_eq!(
            short_gpu_name("Navi 31 [Radeon RX 7900 XTX]").as_deref(),
            Some("Radeon RX 7900 XTX")
        );
    }

    #[test]
    fn vendored_layer_declares_itself_and_replaces_the_system_one() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let layer = vendor_layer_dir(&data);
        std::fs::create_dir_all(&layer).unwrap();
        // Sans bibliothèque, rien n'est déclaré : pas de HUD fantôme.
        assert_eq!(ensure_vendor_layer(&data), None);
        std::fs::write(layer.join("libMangoHud.so"), b"ELF").unwrap();
        let dir = ensure_vendor_layer(&data).expect("couche installée");
        let manifest = std::fs::read_to_string(dir.join("lapinux-mangohud.json")).unwrap();
        assert!(manifest.contains(VENDORED_LAYER_NAME));
        assert!(manifest.contains("libMangoHud.so"));
        assert!(manifest.contains("\"MANGOHUD\": \"1\""));
        // Le manifeste est réécrit quand le chemin change, et pas avant.
        let first = std::fs::metadata(dir.join("lapinux-mangohud.json"))
            .unwrap()
            .modified()
            .unwrap();
        ensure_vendor_layer(&data);
        let second = std::fs::metadata(dir.join("lapinux-mangohud.json"))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn merged_lists_keep_existing_entries() {
        assert_eq!(merge_list("", "a", ':'), "a");
        assert_eq!(merge_list("/x", "/y", ':'), "/x:/y");
        assert_eq!(merge_list("/x:/y", "/y", ':'), "/x:/y");
        assert_eq!(merge_list("*,~implicit~", "VK_LAYER_X", ','), "*,~implicit~,VK_LAYER_X");
    }

    #[test]
    fn writes_config_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(dir.path(), 440, &HudSettings::default()).unwrap();
        assert_eq!(path.file_name().unwrap(), "440.conf");
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("position=top-left"));
        let env = env(&path);
        assert!(env.contains(&("MANGOHUD".into(), "1".into())));
    }
}
