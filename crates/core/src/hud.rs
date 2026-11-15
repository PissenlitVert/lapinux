//! HUD de performance entièrement personnalisable.
//!
//! Un [`HudStyle`] décrit tout ce que la plaque native dessine : les éléments
//! affichés et leur ordre, leur précision et leurs unités, les couleurs de
//! chaque famille de mesures, le fond, les libellés, l'accent et le badge
//! d'enregistrement. Les styles créés par l'utilisateur vivent dans la
//! configuration ; les trois presets historiques restent disponibles pour la
//! migration des anciennes configurations.
//!
//! Le format est versionné (`HUD_STYLE_VERSION`). Une configuration écrite
//! avant l'arrivée des styles est convertie en un HUD nommé qui reprend
//! exactement l'ancien préréglage, l'ancrage, la disposition, l'échelle, la
//! courbe et les cœurs : aucun réglage existant n'est perdu.

use crate::i18n::t;
use serde::{Deserialize, Serialize};

/// Version du format des styles de HUD.
pub const HUD_STYLE_VERSION: u32 = 1;

/// Élément de la plaque native. L'ordre du vecteur `HudStyle::elements` est
/// l'ordre réel d'affichage ; un élément désactivé garde sa place pour
/// pouvoir être rallumé sans le rechercher.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HudElement {
    Fps,
    Frametime,
    Gpu,
    GpuTemp,
    Vram,
    Cpu,
    CpuTemp,
    Ram,
    Cores,
    Graph,
}

impl HudElement {
    pub const ALL: [HudElement; 10] = [
        HudElement::Fps,
        HudElement::Frametime,
        HudElement::Gpu,
        HudElement::GpuTemp,
        HudElement::Vram,
        HudElement::Cpu,
        HudElement::CpuTemp,
        HudElement::Ram,
        HudElement::Cores,
        HudElement::Graph,
    ];

    /// Identifiant stable utilisé dans la configuration et par l'overlay.
    pub fn wire(self) -> &'static str {
        match self {
            HudElement::Fps => "fps",
            HudElement::Frametime => "frametime",
            HudElement::Gpu => "gpu",
            HudElement::GpuTemp => "gpu_temp",
            HudElement::Vram => "vram",
            HudElement::Cpu => "cpu",
            HudElement::CpuTemp => "cpu_temp",
            HudElement::Ram => "ram",
            HudElement::Cores => "cores",
            HudElement::Graph => "graph",
        }
    }

    /// Libellé traduit, utilisé par l'overlay, les réglages et les aperçus.
    pub fn label(self) -> &'static str {
        match self {
            HudElement::Fps => t("hud.elem_fps"),
            HudElement::Frametime => t("hud.elem_frametime"),
            HudElement::Gpu => t("hud.elem_gpu"),
            HudElement::GpuTemp => t("hud.elem_gpu_temp"),
            HudElement::Vram => t("hud.elem_vram"),
            HudElement::Cpu => t("hud.elem_cpu"),
            HudElement::CpuTemp => t("hud.elem_cpu_temp"),
            HudElement::Ram => t("hud.elem_ram"),
            HudElement::Cores => t("hud.elem_cores"),
            HudElement::Graph => t("hud.elem_graph"),
        }
    }

    /// Une valeur numérique : la précision se règle de 0 à 2 décimales.
    pub fn has_precision(self) -> bool {
        matches!(
            self,
            HudElement::Fps
                | HudElement::Frametime
                | HudElement::Gpu
                | HudElement::GpuTemp
                | HudElement::Vram
                | HudElement::Cpu
                | HudElement::CpuTemp
                | HudElement::Ram
        )
    }

    /// Unités proposées, la première étant celle par défaut.
    pub fn units(self) -> &'static [&'static str] {
        match self {
            HudElement::GpuTemp | HudElement::CpuTemp => &["celsius", "fahrenheit"],
            HudElement::Vram | HudElement::Ram => &["gib", "percent"],
            _ => &[],
        }
    }

    /// Une courbe n'a ni précision ni unité : elle occupe une bande.
    pub fn is_chart(self) -> bool {
        matches!(self, HudElement::Cores | HudElement::Graph)
    }
}

/// Réglage d'un élément : présence, taille, précision et unité.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HudElementStyle {
    pub kind: HudElement,
    pub enabled: bool,
    /// Taille relative de cet élément dans la disposition en bloc. La
    /// disposition en ligne garde une taille commune pour rester sur une
    /// seule ligne lisible.
    pub scale: f32,
    pub precision: u8,
    /// `celsius`/`fahrenheit` pour les températures, `gib`/`percent` pour la
    /// mémoire. Vide pour les autres éléments.
    pub unit: String,
}

impl Default for HudElementStyle {
    fn default() -> Self {
        Self {
            kind: HudElement::Fps,
            enabled: true,
            scale: 1.0,
            precision: 0,
            unit: String::new(),
        }
    }
}

impl HudElementStyle {
    pub fn new(kind: HudElement) -> Self {
        Self {
            kind,
            enabled: true,
            scale: 1.0,
            precision: default_precision(kind),
            unit: default_unit(kind).to_string(),
        }
    }

    fn normalize(&mut self) {
        if !self.scale.is_finite() {
            self.scale = 1.0;
        }
        self.scale = self.scale.clamp(0.75, 1.5);
        self.precision = self.precision.min(2);
        let units = self.kind.units();
        if units.is_empty() {
            self.unit.clear();
        } else {
            let wanted = self.unit.trim().to_ascii_lowercase();
            if !units.contains(&wanted.as_str()) {
                self.unit = units[0].to_string();
            } else {
                self.unit = wanted;
            }
        }
    }
}

fn default_precision(kind: HudElement) -> u8 {
    match kind {
        HudElement::Frametime => 1,
        HudElement::Ram | HudElement::Vram => 1,
        _ => 0,
    }
}

fn default_unit(kind: HudElement) -> &'static str {
    kind.units().first().copied().unwrap_or("")
}

/// Toutes les couleurs de la plaque, chacune réglable séparément.
///
/// Les valeurs sont des chaînes hexadécimales `#RRGGBB` ou `#RRGGBBAA` :
/// lisibles dans le fichier TOML, elles ne dépendent d'aucune bibliothèque
/// graphique et se valident sans peindre.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HudColors {
    /// Valeurs des mesures (quand un élément n'a pas sa propre couleur).
    pub text: String,
    /// Libellés, unités, séparateurs, titre secondaire.
    pub label: String,
    /// Accent de la plaque : titre, soulignement, points de focus.
    pub accent: String,
    /// Fond du panneau, alpha compris.
    pub panel: String,
    /// Bordure du panneau.
    pub border: String,
    pub fps: String,
    pub frametime: String,
    /// Courbe de frametime.
    pub graph: String,
    pub cpu: String,
    pub gpu: String,
    /// Températures CPU et GPU.
    pub temp: String,
    pub ram: String,
    pub vram: String,
    /// Barres de charge par cœur.
    pub cores: String,
    /// Pastille et liseré de l'icône du jeu.
    pub icon: String,
    pub badge_text: String,
    pub badge_panel: String,
    pub badge_accent: String,
}

impl Default for HudColors {
    fn default() -> Self {
        Self {
            text: "#F1F5F9".into(),
            label: "#94A3B8".into(),
            accent: "#CED0D3".into(),
            panel: "#000000D0".into(),
            border: "#FFFFFF1A".into(),
            fps: "#7DD3FC".into(),
            frametime: "#F1F5F9".into(),
            graph: "#38BDF8".into(),
            cpu: "#60A5FA".into(),
            gpu: "#34D399".into(),
            temp: "#FBBF24".into(),
            ram: "#F472B6".into(),
            vram: "#A78BFA".into(),
            cores: "#60A5FA".into(),
            icon: "#CED0D3".into(),
            badge_text: "#FEE2E2".into(),
            badge_panel: "#080A0EDC".into(),
            badge_accent: "#F87171".into(),
        }
    }
}

impl HudColors {
    /// Applique une couleur à un emplacement désigné par son nom de champ.
    /// Renvoie false pour un emplacement inconnu : l'UI ne peut pas peindre
    /// silencieusement une couleur qui ne sert à rien.
    pub fn set(&mut self, slot: HudColorSlot, value: &str) -> bool {
        let Some(normalized) = normalize_hex(value) else {
            return false;
        };
        let field = self.field_mut(slot);
        *field = normalized;
        true
    }

    pub fn get(&self, slot: HudColorSlot) -> &str {
        match slot {
            HudColorSlot::Text => &self.text,
            HudColorSlot::Label => &self.label,
            HudColorSlot::Accent => &self.accent,
            HudColorSlot::Panel => &self.panel,
            HudColorSlot::Border => &self.border,
            HudColorSlot::Fps => &self.fps,
            HudColorSlot::Frametime => &self.frametime,
            HudColorSlot::Graph => &self.graph,
            HudColorSlot::Cpu => &self.cpu,
            HudColorSlot::Gpu => &self.gpu,
            HudColorSlot::Temp => &self.temp,
            HudColorSlot::Ram => &self.ram,
            HudColorSlot::Vram => &self.vram,
            HudColorSlot::Cores => &self.cores,
            HudColorSlot::Icon => &self.icon,
            HudColorSlot::BadgeText => &self.badge_text,
            HudColorSlot::BadgePanel => &self.badge_panel,
            HudColorSlot::BadgeAccent => &self.badge_accent,
        }
    }

    fn field_mut(&mut self, slot: HudColorSlot) -> &mut String {
        match slot {
            HudColorSlot::Text => &mut self.text,
            HudColorSlot::Label => &mut self.label,
            HudColorSlot::Accent => &mut self.accent,
            HudColorSlot::Panel => &mut self.panel,
            HudColorSlot::Border => &mut self.border,
            HudColorSlot::Fps => &mut self.fps,
            HudColorSlot::Frametime => &mut self.frametime,
            HudColorSlot::Graph => &mut self.graph,
            HudColorSlot::Cpu => &mut self.cpu,
            HudColorSlot::Gpu => &mut self.gpu,
            HudColorSlot::Temp => &mut self.temp,
            HudColorSlot::Ram => &mut self.ram,
            HudColorSlot::Vram => &mut self.vram,
            HudColorSlot::Cores => &mut self.cores,
            HudColorSlot::Icon => &mut self.icon,
            HudColorSlot::BadgeText => &mut self.badge_text,
            HudColorSlot::BadgePanel => &mut self.badge_panel,
            HudColorSlot::BadgeAccent => &mut self.badge_accent,
        }
    }

    pub fn normalize(&mut self) {
        for slot in HudColorSlot::ALL {
            let fallback = HudColors::default();
            let fallback = fallback.get(slot);
            let current = self.get(slot).to_string();
            let value = normalize_hex(&current).unwrap_or_else(|| fallback.to_string());
            *self.field_mut(slot) = value;
        }
    }
}

/// Emplacement de couleur exposé par l'éditeur. L'ordre est celui de l'UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HudColorSlot {
    Text,
    Label,
    Accent,
    Fps,
    Frametime,
    Graph,
    Gpu,
    Cpu,
    Temp,
    Vram,
    Ram,
    Cores,
    Panel,
    Border,
    Icon,
    BadgeText,
    BadgePanel,
    BadgeAccent,
}

impl HudColorSlot {
    /// Couleurs réellement visibles dans l'éditeur. `Text` reste dans le
    /// format pour les anciennes configurations, mais chaque valeur a sa
    /// propre couleur : l'exposer ferait un réglage sans effet.
    pub const ALL: [HudColorSlot; 17] = [
        HudColorSlot::Label,
        HudColorSlot::Accent,
        HudColorSlot::Fps,
        HudColorSlot::Frametime,
        HudColorSlot::Graph,
        HudColorSlot::Gpu,
        HudColorSlot::Cpu,
        HudColorSlot::Temp,
        HudColorSlot::Vram,
        HudColorSlot::Ram,
        HudColorSlot::Cores,
        HudColorSlot::Panel,
        HudColorSlot::Border,
        HudColorSlot::Icon,
        HudColorSlot::BadgeText,
        HudColorSlot::BadgePanel,
        HudColorSlot::BadgeAccent,
    ];

    pub fn label(self) -> &'static str {
        match self {
            HudColorSlot::Text => t("hud.color_text"),
            HudColorSlot::Label => t("hud.color_label"),
            HudColorSlot::Accent => t("hud.color_accent"),
            HudColorSlot::Fps => t("hud.color_fps"),
            HudColorSlot::Frametime => t("hud.color_frametime"),
            HudColorSlot::Graph => t("hud.color_graph"),
            HudColorSlot::Gpu => t("hud.color_gpu"),
            HudColorSlot::Cpu => t("hud.color_cpu"),
            HudColorSlot::Temp => t("hud.color_temp"),
            HudColorSlot::Vram => t("hud.color_vram"),
            HudColorSlot::Ram => t("hud.color_ram"),
            HudColorSlot::Cores => t("hud.color_cores"),
            HudColorSlot::Panel => t("hud.color_panel"),
            HudColorSlot::Border => t("hud.color_border"),
            HudColorSlot::Icon => t("hud.color_icon"),
            HudColorSlot::BadgeText => t("hud.color_badge_text"),
            HudColorSlot::BadgePanel => t("hud.color_badge_panel"),
            HudColorSlot::BadgeAccent => t("hud.color_badge_accent"),
        }
    }

    /// Forme utilisée par l'overlay pour désigner une couleur sans JSON.
    pub fn wire(self) -> &'static str {
        match self {
            HudColorSlot::Text => "text",
            HudColorSlot::Label => "label",
            HudColorSlot::Accent => "accent",
            HudColorSlot::Fps => "fps",
            HudColorSlot::Frametime => "frametime",
            HudColorSlot::Graph => "graph",
            HudColorSlot::Gpu => "gpu",
            HudColorSlot::Cpu => "cpu",
            HudColorSlot::Temp => "temp",
            HudColorSlot::Vram => "vram",
            HudColorSlot::Ram => "ram",
            HudColorSlot::Cores => "cores",
            HudColorSlot::Panel => "panel",
            HudColorSlot::Border => "border",
            HudColorSlot::Icon => "icon",
            HudColorSlot::BadgeText => "badge_text",
            HudColorSlot::BadgePanel => "badge_panel",
            HudColorSlot::BadgeAccent => "badge_accent",
        }
    }
}

/// Badge d'enregistrement, rattaché au style : sa couleur, sa place et sa
/// taille suivent le HUD choisi, sans dépendre d'un second réglage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HudBadge {
    pub enabled: bool,
    /// Mot affiché avant le chronomètre, `REC` par défaut.
    #[serde(default = "default_badge_label")]
    pub label: String,
    pub position: String,
    pub scale: f32,
}

fn default_badge_label() -> String {
    "REC".into()
}

impl Default for HudBadge {
    fn default() -> Self {
        Self {
            enabled: true,
            label: default_badge_label(),
            position: "bottom-left".into(),
            scale: 1.0,
        }
    }
}

/// Un HUD nommé, complet et réutilisable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HudStyle {
    /// Version du format, pour migrer sans deviner.
    pub version: u32,
    /// Identifiant stable (slug) : le renommer ne casse pas les jeux qui le
    /// référencent.
    pub id: String,
    pub name: String,
    /// Nom affiché pour le processeur. Vide : le modèle détecté est utilisé.
    #[serde(default)]
    pub cpu_label: String,
    /// Nom affiché pour la carte graphique. Vide : le modèle détecté est utilisé.
    #[serde(default)]
    pub gpu_label: String,
    /// `block` ou `inline`.
    pub layout: String,
    pub position: String,
    pub scale: f32,
    pub elements: Vec<HudElementStyle>,
    pub colors: HudColors,
    pub badge: HudBadge,
}

impl Default for HudStyle {
    fn default() -> Self {
        Self::builtin("essential")
    }
}

impl HudStyle {
    /// Style intégré correspondant à un preset historique. `essential` est le
    /// défaut quand le nom est vide ou inconnu.
    pub fn builtin(preset: &str) -> Self {
        let preset = canonical_preset(preset);
        let mut elements = Vec::new();
        for kind in HudElement::ALL {
            let enabled = match preset {
                "simple" => matches!(kind, HudElement::Fps | HudElement::Gpu | HudElement::Cpu),
                "full" => !matches!(kind, HudElement::Cores | HudElement::Graph),
                _ => !matches!(
                    kind,
                    HudElement::GpuTemp
                        | HudElement::CpuTemp
                        | HudElement::Vram
                        | HudElement::Cores
                        | HudElement::Graph
                ),
            };
            let mut element = HudElementStyle::new(kind);
            element.enabled = enabled;
            elements.push(element);
        }
        Self {
            version: HUD_STYLE_VERSION,
            id: preset.to_string(),
            name: preset_label(preset).to_string(),
            cpu_label: String::new(),
            gpu_label: String::new(),
            layout: "block".into(),
            position: "top-left".into(),
            scale: 1.15,
            elements,
            colors: HudColors::default(),
            badge: HudBadge::default(),
        }
    }

    /// Convertit les anciens réglages globaux en HUD nommé. C'est la migration
    /// qui garantit qu'une configuration existante garde son apparence.
    #[allow(clippy::too_many_arguments)]
    pub fn from_legacy(
        preset: &str,
        position: &str,
        layout: &str,
        scale: f32,
        graph: bool,
        cores: bool,
        badge_enabled: bool,
        badge_position: &str,
        badge_scale: f32,
    ) -> Self {
        let preset = if preset.trim().is_empty() {
            "essential"
        } else {
            preset
        };
        let mut style = Self::builtin(preset);
        style.id = "hud-migre".into();
        style.name = "Mon HUD".into();
        if !position.trim().is_empty() {
            style.position = position.to_string();
        }
        if !layout.trim().is_empty() {
            style.layout = layout.to_string();
        }
        style.scale = scale;
        if let Some(element) = style.element_mut(HudElement::Graph) {
            element.enabled = graph;
        }
        if let Some(element) = style.element_mut(HudElement::Cores) {
            element.enabled = cores;
        }
        style.badge = HudBadge {
            enabled: badge_enabled,
            label: default_badge_label(),
            position: if badge_position.trim().is_empty() {
                "bottom-left".into()
            } else {
                badge_position.to_string()
            },
            scale: badge_scale,
        };
        style.normalize();
        style
    }

    /// Nouveau HUD vierge, dérivé du style courant : l'utilisateur part de ce
    /// qu'il voit déjà au lieu d'un écran vide.
    pub fn duplicate_of(&self, name: &str) -> Self {
        let mut style = self.clone();
        style.id.clear();
        style.name = name.to_string();
        style.normalize();
        style
    }

    pub fn element_mut(&mut self, kind: HudElement) -> Option<&mut HudElementStyle> {
        self.elements
            .iter_mut()
            .find(|element| element.kind == kind)
    }

    pub fn element(&self, kind: HudElement) -> Option<&HudElementStyle> {
        self.elements.iter().find(|element| element.kind == kind)
    }

    pub fn enabled(&self, kind: HudElement) -> bool {
        self.element(kind).is_some_and(|element| element.enabled)
    }

    /// Nombre de lignes de mesures (hors courbes) réellement dessinées.
    pub fn row_count(&self) -> usize {
        self.elements
            .iter()
            .filter(|element| element.enabled && !element.kind.is_chart())
            .count()
    }

    /// Valide et complète le style : identifiant, nom, couleurs, ancrage,
    /// disposition, échelle, unités et présence de chaque élément.
    pub fn normalize(&mut self) {
        self.version = HUD_STYLE_VERSION;
        self.name = self.name.trim().to_string();
        if self.name.is_empty() {
            self.name = "HUD".into();
        }
        self.cpu_label = self.cpu_label.trim().chars().take(40).collect();
        self.gpu_label = self.gpu_label.trim().chars().take(40).collect();
        if self.id.trim().is_empty() {
            self.id = slugify(&self.name);
        } else {
            self.id = slugify(&self.id);
        }
        if self.id.is_empty() {
            self.id = "hud".into();
        }
        self.layout = if self.layout.trim().eq_ignore_ascii_case("inline") {
            "inline".into()
        } else {
            "block".into()
        };
        self.position = self.position.trim().to_ascii_lowercase();
        if !HUD_POSITIONS.contains(&self.position.as_str()) {
            self.position = "top-left".into();
        }
        self.scale = self.scale.clamp(0.75, 2.0);
        self.colors.normalize();
        // Chaque élément existe une seule fois ; l'ordre du vecteur est
        // conservé, les éléments découverts sont ajoutés à la fin.
        let mut seen = Vec::new();
        self.elements.retain(|element| {
            if seen.contains(&element.kind) {
                return false;
            }
            seen.push(element.kind);
            true
        });
        for kind in HudElement::ALL {
            if !seen.contains(&kind) {
                let mut element = HudElementStyle::new(kind);
                element.enabled = false;
                self.elements.push(element);
            }
        }
        for element in &mut self.elements {
            element.normalize();
        }
        self.badge.position = self.badge.position.trim().to_ascii_lowercase();
        if !HUD_POSITIONS.contains(&self.badge.position.as_str()) {
            self.badge.position = "bottom-left".into();
        }
        self.badge.scale = self.badge.scale.clamp(0.65, 2.0);
        self.badge.label = self.badge.label.trim().to_string();
        self.badge.label = self.badge.label.chars().take(16).collect();
    }

    /// Préréglage historique équivalent, pour les anciens chemins.
    pub fn preset(&self) -> &'static str {
        if !self.enabled(HudElement::Frametime) && !self.enabled(HudElement::Ram) {
            "simple"
        } else if self.enabled(HudElement::Vram) {
            "full"
        } else {
            "essential"
        }
    }

    /// Applique un thème de couleurs complet, sans toucher aux éléments.
    pub fn apply_theme(&mut self, theme: &str) {
        self.colors = colors_for_theme(theme);
    }
}

/// Ancrages acceptés, identiques entre le noyau et l'overlay.
pub const HUD_POSITIONS: [&str; 9] = [
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

/// Thèmes prêts à l'emploi. Ils changent toutes les couleurs d'un coup et
/// restent modifiables couleur par couleur ensuite.
pub const HUD_THEMES: [(&str, &str); 6] = [
    ("classique", "Classique"),
    ("neon", "Néon"),
    ("mono", "Monochrome"),
    ("chaud", "Chaud"),
    ("froid", "Froid"),
    ("contraste", "Contraste"),
];

pub fn colors_for_theme(theme: &str) -> HudColors {
    let mut colors = HudColors::default();
    match theme.trim().to_ascii_lowercase().as_str() {
        "neon" => {
            colors.fps = "#22D3EE".into();
            colors.frametime = "#F0ABFC".into();
            colors.graph = "#F0ABFC".into();
            colors.cpu = "#A3E635".into();
            colors.gpu = "#2DD4BF".into();
            colors.temp = "#FDE047".into();
            colors.ram = "#F472B6".into();
            colors.vram = "#818CF8".into();
            colors.cores = "#A3E635".into();
            colors.accent = "#67E8F9".into();
            colors.label = "#94A3B8".into();
            colors.panel = "#05010FD8".into();
            colors.border = "#22D3EE40".into();
        }
        "mono" => {
            colors.fps = "#F8FAFC".into();
            colors.frametime = "#E2E8F0".into();
            colors.graph = "#CBD5E1".into();
            colors.cpu = "#E2E8F0".into();
            colors.gpu = "#F8FAFC".into();
            colors.temp = "#CBD5E1".into();
            colors.ram = "#CBD5E1".into();
            colors.vram = "#E2E8F0".into();
            colors.cores = "#94A3B8".into();
            colors.accent = "#F8FAFC".into();
            colors.label = "#94A3B8".into();
            colors.panel = "#000000E0".into();
            colors.border = "#FFFFFF24".into();
            colors.badge_accent = "#F8FAFC".into();
            colors.badge_text = "#F8FAFC".into();
        }
        "chaud" => {
            colors.fps = "#FDBA74".into();
            colors.frametime = "#FED7AA".into();
            colors.graph = "#FB923C".into();
            colors.cpu = "#FCA5A5".into();
            colors.gpu = "#FDBA74".into();
            colors.temp = "#F87171".into();
            colors.ram = "#FDA4AF".into();
            colors.vram = "#FCA5A5".into();
            colors.cores = "#FB923C".into();
            colors.accent = "#FDBA74".into();
            colors.panel = "#1C0A04DC".into();
            colors.border = "#FDBA7440".into();
            colors.badge_accent = "#FB7185".into();
        }
        "froid" => {
            colors.fps = "#93C5FD".into();
            colors.frametime = "#BFDBFE".into();
            colors.graph = "#38BDF8".into();
            colors.cpu = "#67E8F9".into();
            colors.gpu = "#5EEAD4".into();
            colors.temp = "#A5B4FC".into();
            colors.ram = "#93C5FD".into();
            colors.vram = "#818CF8".into();
            colors.cores = "#67E8F9".into();
            colors.accent = "#7DD3FC".into();
            colors.panel = "#020617E0".into();
            colors.border = "#38BDF840".into();
        }
        "contraste" => {
            colors.text = "#FFFFFF".into();
            colors.fps = "#FFFFFF".into();
            colors.frametime = "#FFFFFF".into();
            colors.graph = "#FFFFFF".into();
            colors.cpu = "#00FFCC".into();
            colors.gpu = "#FFFF00".into();
            colors.temp = "#FF9900".into();
            colors.ram = "#FF66FF".into();
            colors.vram = "#66AAFF".into();
            colors.cores = "#00FFCC".into();
            colors.accent = "#FFFFFF".into();
            colors.label = "#FFFFFF".into();
            colors.panel = "#000000F0".into();
            colors.border = "#FFFFFF80".into();
            colors.badge_text = "#FFFFFF".into();
        }
        _ => {}
    }
    colors
}

/// Palette rapide proposée sous chaque couleur, sans ouvrir de roue.
pub const QUICK_COLORS: [&str; 12] = [
    "#F1F5F9", "#94A3B8", "#F87171", "#FB923C", "#FBBF24", "#A3E635", "#34D399", "#22D3EE",
    "#60A5FA", "#818CF8", "#F472B6", "#E879F9",
];

pub fn canonical_preset(preset: &str) -> &'static str {
    match preset.trim().to_ascii_lowercase().as_str() {
        "simple" => "simple",
        "full" | "detailed" => "full",
        _ => "essential",
    }
}

pub fn preset_label(preset: &str) -> &'static str {
    match canonical_preset(preset) {
        "simple" => "Simple",
        "full" => "Détaillée",
        _ => "Basique",
    }
}

/// Transforme un nom libre en identifiant stable : minuscules, chiffres et
/// tirets simples. Un nom entièrement non latin garde « hud ».
pub fn slugify(name: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            if dash && !out.is_empty() {
                out.push('-');
            }
            dash = false;
            out.push(ch.to_ascii_lowercase());
        } else {
            dash = true;
        }
    }
    out
}

/// Lit une couleur `#RRGGBB` ou `#RRGGBBAA` (le dièse est facultatif) et la
/// rend normalisée en majuscules avec alpha.
pub fn normalize_hex(value: &str) -> Option<String> {
    let raw = value.trim().trim_start_matches('#');
    let (rgb, alpha) = match raw.len() {
        6 => (raw, "FF"),
        8 => (&raw[..6], &raw[6..]),
        _ => return None,
    };
    if !rgb.chars().all(|ch| ch.is_ascii_hexdigit())
        || !alpha.chars().all(|ch| ch.is_ascii_hexdigit())
    {
        return None;
    }
    Some(format!(
        "#{}{}",
        rgb.to_ascii_uppercase(),
        alpha.to_ascii_uppercase()
    ))
}

/// Décompose une couleur validée en composantes RGBA.
pub fn color_rgba(value: &str) -> Option<[u8; 4]> {
    let raw = normalize_hex(value)?;
    let byte = |index: usize| u8::from_str_radix(&raw[index..index + 2], 16).ok();
    Some([byte(1)?, byte(3)?, byte(5)?, byte(7)?])
}

/// Couleur hexadécimale à partir de composantes RGBA.
pub fn hex_rgba(color: [u8; 4]) -> String {
    format!(
        "#{:02X}{:02X}{:02X}{:02X}",
        color[0], color[1], color[2], color[3]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_and_elements_round_trip_through_toml() {
        let mut style = HudStyle::builtin("full");
        style.name = "Mon HUD".into();
        style.id = "mon-hud".into();
        style.colors.fps = "#FF0000".into();
        style.colors.panel = "#102030C0".into();
        style.elements[0].precision = 2;
        style.elements[0].scale = 1.25;
        style.element_mut(HudElement::Ram).unwrap().unit = "percent".into();
        style.normalize();

        let encoded = toml::to_string(&style).unwrap();
        let decoded: HudStyle = toml::from_str(&encoded).unwrap();
        assert_eq!(style, decoded);
        assert_eq!(decoded.colors.fps, "#FF0000FF");
        assert_eq!(decoded.colors.panel, "#102030C0");
        assert_eq!(decoded.element(HudElement::Ram).unwrap().unit, "percent");
        assert!((decoded.element(HudElement::Fps).unwrap().scale - 1.25).abs() < f32::EPSILON);
    }

    #[test]
    fn legacy_style_migrates_every_setting() {
        let style = HudStyle::from_legacy(
            "full",
            "bottom-right",
            "inline",
            1.45,
            true,
            true,
            false,
            "top-center",
            1.6,
        );
        assert_eq!(style.id, "hud-migre");
        assert_eq!(style.name, "Mon HUD");
        assert_eq!(style.position, "bottom-right");
        assert_eq!(style.layout, "inline");
        assert!((style.scale - 1.45).abs() < f32::EPSILON);
        assert!(style.enabled(HudElement::Graph));
        assert!(style.enabled(HudElement::Cores));
        assert!(style.enabled(HudElement::Vram));
        assert!(!style.badge.enabled);
        assert_eq!(style.badge.position, "top-center");
        assert!((style.badge.scale - 1.6).abs() < f32::EPSILON);
    }

    #[test]
    fn normalize_fills_missing_elements_and_validates_colors() {
        let mut style = HudStyle::builtin("simple");
        style
            .elements
            .retain(|element| element.kind == HudElement::Fps);
        style.elements[0].enabled = true;
        style.elements[0].scale = 9.0;
        style.colors.gpu = "not-a-color".into();
        style.position = "everywhere".into();
        style.scale = 9.0;
        style.layout = "diagonal".into();
        style.normalize();

        assert_eq!(style.elements.len(), HudElement::ALL.len());
        assert!(style
            .elements
            .iter()
            .any(|element| element.kind == HudElement::Gpu));
        assert_eq!(style.colors.gpu, HudColors::default().gpu);
        assert_eq!(style.position, "top-left");
        assert_eq!(style.layout, "block");
        assert_eq!(style.scale, 2.0);
        assert_eq!(
            style.elements[0].scale, 1.5,
            "la taille par élément est bornée"
        );
        assert!(style
            .elements
            .iter()
            .filter(|element| element.kind != HudElement::Fps)
            .all(|element| element.scale == 1.0));
        assert_eq!(style.version, HUD_STYLE_VERSION);
    }

    #[test]
    fn an_empty_badge_label_is_kept_empty() {
        let mut style = HudStyle::builtin("essential");
        style.badge.label = "   ".into();
        style.normalize();
        assert_eq!(style.badge.label, "");
    }

    #[test]
    fn custom_cpu_and_gpu_labels_survive_normalization() {
        let mut style = HudStyle::builtin("essential");
        style.cpu_label = "  Mon processeur  ".into();
        style.gpu_label = "Ma carte".into();
        style.normalize();
        assert_eq!(style.cpu_label, "Mon processeur");
        assert_eq!(style.gpu_label, "Ma carte");
    }

    #[test]
    fn hex_colors_accept_short_forms_and_reject_junk() {
        assert_eq!(normalize_hex("#ff0000").as_deref(), Some("#FF0000FF"));
        assert_eq!(normalize_hex("102030c0").as_deref(), Some("#102030C0"));
        assert_eq!(normalize_hex("10203"), None);
        assert_eq!(normalize_hex("#GG0000"), None);
        assert_eq!(color_rgba("#11223344"), Some([0x11, 0x22, 0x33, 0x44]));
        assert_eq!(hex_rgba([0x11, 0x22, 0x33, 0x44]), "#11223344");
    }

    #[test]
    fn themes_change_real_colors() {
        let classic = colors_for_theme("classique");
        let neon = colors_for_theme("neon");
        assert_ne!(classic.fps, neon.fps);
        assert_ne!(classic.graph, neon.graph);
        assert_ne!(classic.badge_accent, colors_for_theme("chaud").badge_accent);
        // Un thème inconnu garde le thème classique complet.
        assert_eq!(colors_for_theme("inconnu"), HudColors::default());
    }

    #[test]
    fn slug_is_stable_and_readable() {
        assert_eq!(slugify("Mon HUD FPS !"), "mon-hud-fps");
        assert_eq!(slugify("  "), "");
        assert_eq!(slugify("HUD_2"), "hud-2");
    }
}
