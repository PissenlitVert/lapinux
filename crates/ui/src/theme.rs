//! Thèmes et plugins de Lapinux.
//!
//! Un thème est un manifeste TOML : une palette, une disposition (`classic`
//! ou `nx`) et quelques réglages de forme. Le thème classique est compilé dans
//! le binaire, NX et les autres thèmes natifs viennent de plugins `.so`, et
//! les thèmes créés par l'utilisateur vivent dans `~/.config/lapinux/themes`.
//!
//! Un plugin est aussi un manifeste TOML. Il peut surcharger les couleurs et
//! la disposition du thème actif, changer le libellé de l'accueil, ajouter des
//! raccourcis au dock NX et déclarer des hooks backend exécutés par le daemon
//! au lancement et à la fin d'une partie.

use egui::Color32;
use lapinux_plugin_api::ThemeSpec;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Identifiant du thème historique, celui qui préserve l'interface actuelle.
pub const CLASSIC_ID: &str = "classic";
/// Identifiant du thème inspiré de la Nintendo Switch.
#[cfg(test)]
pub const NX_ID: &str = "nx";
/// Anciennes valeurs du champ `general.theme` : elles désignent le thème
/// classique, la palette claire n'ayant jamais eu de rendu distinct.
pub const CLASSIC_ALIASES: [&str; 6] = ["classic", "default", "dark", "light", "sombre", "clair"];
#[cfg(test)]
const RESERVED_IDS: [&str; 2] = [CLASSIC_ID, NX_ID];

/// Disposition de l'interface portée par un thème.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeLayout {
    /// L'interface Lapinux telle qu'elle existe depuis le début.
    #[default]
    Classic,
    /// L'accueil console : carrousel horizontal et dock circulaire.
    Nx,
}

impl ThemeLayout {
    pub fn from_key(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "classic" | "default" | "dark" | "light" | "sombre" | "clair" => Some(Self::Classic),
            "nx" | "switch" | "console" => Some(Self::Nx),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Classic => lapinux_core::i18n::t("theme.layout_classic"),
            Self::Nx => "NX",
        }
    }
}

/// Palette appliquée à la coque de l'interface et aux écrans NX. Les couleurs
/// s'écrivent `#RRGGBB` ou `#RRGGBBAA` dans les manifestes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub background: Color32,
    pub panel: Color32,
    pub card: Color32,
    pub card_hover: Color32,
    pub card_active: Color32,
    pub border: Color32,
    pub accent: Color32,
    pub text: Color32,
    pub text_dim: Color32,
}

impl Palette {
    /// La palette exacte de l'interface historique.
    pub const fn classic() -> Self {
        Self {
            background: Color32::from_rgb(20, 21, 22),
            panel: Color32::from_rgb(28, 29, 31),
            card: Color32::from_rgb(39, 40, 43),
            card_hover: Color32::from_rgb(48, 49, 52),
            card_active: Color32::from_rgb(58, 59, 62),
            border: Color32::from_rgb(64, 65, 68),
            accent: Color32::from_rgb(206, 208, 211),
            text: Color32::from_rgb(235, 236, 238),
            text_dim: Color32::from_rgb(160, 162, 166),
        }
    }

    /// Gris anthracite, cyan et blanc de l'accueil Switch.
    pub const fn nx() -> Self {
        Self {
            background: Color32::from_rgb(28, 28, 30),
            panel: Color32::from_rgb(43, 43, 46),
            card: Color32::from_rgb(57, 57, 61),
            card_hover: Color32::from_rgb(70, 70, 75),
            card_active: Color32::from_rgb(82, 83, 89),
            border: Color32::from_rgb(73, 74, 79),
            accent: Color32::from_rgb(0, 200, 224),
            text: Color32::from_rgb(244, 245, 247),
            text_dim: Color32::from_rgb(166, 169, 176),
        }
    }

    pub fn is_light(self) -> bool {
        let value =
            self.background.r() as u32 + self.background.g() as u32 + self.background.b() as u32;
        value > 390
    }
}

impl Default for Palette {
    fn default() -> Self {
        Self::classic()
    }
}

/// Couleurs et réglages d'un thème sérialisé.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ThemeManifest {
    pub id: String,
    pub name: String,
    pub author: String,
    pub description: String,
    /// `classic` ou `nx`.
    pub layout: String,
    pub accent: String,
    pub background: String,
    pub panel: String,
    pub card: String,
    pub card_hover: String,
    pub card_active: String,
    pub border: String,
    pub text: String,
    pub text_dim: String,
    /// Rayon des surfaces du thème, en points.
    pub rounding: f32,
    /// Échelle des tailles de texte (1.0 = taille normale).
    pub font_scale: f32,
    pub home_label: String,
    pub home_subtitle: String,
}

impl Default for ThemeManifest {
    fn default() -> Self {
        let palette = Palette::classic();
        Self {
            id: CLASSIC_ID.to_string(),
            name: lapinux_core::i18n::t("theme.layout_classic").to_string(),
            author: "Lapinux".to_string(),
            description: lapinux_core::i18n::t("theme.classic_layout_description").to_string(),
            layout: "classic".to_string(),
            accent: hex(palette.accent),
            background: hex(palette.background),
            panel: hex(palette.panel),
            card: hex(palette.card),
            card_hover: hex(palette.card_hover),
            card_active: hex(palette.card_active),
            border: hex(palette.border),
            text: hex(palette.text),
            text_dim: hex(palette.text_dim),
            rounding: 12.0,
            font_scale: 1.0,
            home_label: "Lapinux".to_string(),
            home_subtitle: lapinux_core::i18n::t("theme.classic_description").to_string(),
        }
    }
}

impl ThemeManifest {
    /// Convertit le thème fourni par un `.so` en manifeste interne. Les
    /// libellés viennent du manifeste du plugin, dans la langue demandée ;
    /// le `.so` ne fournit que la palette et la géométrie. Les valeurs
    /// absentes ou invalides retombent sur le thème classique.
    pub fn from_native(
        id: &str,
        spec: &ThemeSpec,
        plugin: &PluginManifest,
        language: &str,
    ) -> Self {
        let mut manifest = Self::classic();
        manifest.id = id.to_string();
        let name = plugin.localized_name(language).trim();
        manifest.name = if name.is_empty() {
            id.to_string()
        } else {
            name.to_string()
        };
        let description = plugin.localized_description(language).trim();
        manifest.description = if description.is_empty() {
            lapinux_core::i18n::t("theme.native_description").to_string()
        } else {
            description.to_string()
        };
        manifest.author = plugin.author.clone();
        manifest.accent = hex(spec_color(spec.colors.accent));
        manifest.background = hex(spec_color(spec.colors.background));
        manifest.panel = hex(spec_color(spec.colors.panel));
        manifest.card = hex(spec_color(spec.colors.card));
        manifest.card_hover = hex(spec_color(spec.colors.card_hover));
        manifest.card_active = hex(spec_color(spec.colors.card_active));
        manifest.border = hex(spec_color(spec.colors.border));
        manifest.text = hex(spec_color(spec.colors.text));
        manifest.text_dim = hex(spec_color(spec.colors.text_dim));
        if spec.rounding > 0.0 {
            manifest.rounding = spec.rounding;
        }
        if spec.font_scale > 0.0 {
            manifest.font_scale = spec.font_scale;
        }
        manifest.home_label = plugin
            .localized_home_label(language)
            .map(str::to_string)
            .unwrap_or_else(|| lapinux_core::i18n::t("theme.home_label_default").to_string());
        manifest.home_subtitle = plugin
            .localized_home_subtitle(language)
            .map(str::to_string)
            .unwrap_or_default();
        manifest
    }

    pub fn classic() -> Self {
        Self::default()
    }

    #[cfg(test)]
    pub fn nx() -> Self {
        let palette = Palette::nx();
        Self {
            id: NX_ID.to_string(),
            name: "NX".to_string(),
            author: "Lapinux".to_string(),
            description: lapinux_core::i18n::t("theme.nx_description").to_string(),
            layout: "nx".to_string(),
            accent: hex(palette.accent),
            background: hex(palette.background),
            panel: hex(palette.panel),
            card: hex(palette.card),
            card_hover: hex(palette.card_hover),
            card_active: hex(palette.card_active),
            border: hex(palette.border),
            text: hex(palette.text),
            text_dim: hex(palette.text_dim),
            rounding: 12.0,
            font_scale: 1.0,
            home_label: lapinux_core::i18n::t("theme.home_label_default").to_string(),
            home_subtitle: lapinux_core::i18n::t("theme.nx_hint").to_string(),
        }
    }

    pub fn layout(&self) -> ThemeLayout {
        ThemeLayout::from_key(&self.layout).unwrap_or_default()
    }

    pub fn palette(&self) -> Palette {
        let fallback = if self.layout() == ThemeLayout::Nx {
            Palette::nx()
        } else {
            Palette::classic()
        };
        Palette {
            background: parse_color(&self.background).unwrap_or(fallback.background),
            panel: parse_color(&self.panel).unwrap_or(fallback.panel),
            card: parse_color(&self.card).unwrap_or(fallback.card),
            card_hover: parse_color(&self.card_hover).unwrap_or(fallback.card_hover),
            card_active: parse_color(&self.card_active).unwrap_or(fallback.card_active),
            border: parse_color(&self.border).unwrap_or(fallback.border),
            accent: parse_color(&self.accent).unwrap_or(fallback.accent),
            text: parse_color(&self.text).unwrap_or(fallback.text),
            text_dim: parse_color(&self.text_dim).unwrap_or(fallback.text_dim),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        validate_id(&self.id)?;
        if self.name.trim().is_empty() {
            return Err(lapinux_core::i18n::t("theme.empty_name").to_string());
        }
        if !(self.layout.trim().is_empty() || ThemeLayout::from_key(&self.layout).is_some()) {
            return Err(lapinux_core::i18n::t("theme.unknown_layout").to_string());
        }
        for (label, value) in [
            ("accent", &self.accent),
            ("background", &self.background),
            ("panel", &self.panel),
            ("card", &self.card),
            ("card_hover", &self.card_hover),
            ("card_active", &self.card_active),
            ("border", &self.border),
            ("text", &self.text),
            ("text_dim", &self.text_dim),
        ] {
            if parse_color(value).is_none() {
                return Err(lapinux_core::i18n::t_args(
                    "theme.invalid_color",
                    &[("label", &label.to_string()), ("value", &value.to_string())],
                ));
            }
        }
        if !(0.0..=36.0).contains(&self.rounding) {
            return Err(lapinux_core::i18n::t("theme.rounding_range").to_string());
        }
        if !(0.75..=1.6).contains(&self.font_scale) {
            return Err(lapinux_core::i18n::t("theme.font_scale_range").to_string());
        }
        Ok(())
    }
}

fn normalized_language(code: &str) -> String {
    code.trim()
        .to_ascii_lowercase()
        .replace('_', "-")
        .split('-')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Entrée ajoutée au dock NX par un plugin.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct DockEntry {
    pub label: String,
    /// Un caractère ou un glyphe affiché dans le dock.
    pub icon: String,
    /// Page interne : `library`, `collections`, `downloads`, `controllers`,
    /// `profile`, `boutiques` ou `captures`.
    pub page: Option<String>,
    /// Lien ouvert par le navigateur du système.
    pub url: Option<String>,
    /// Couleur du raccourci, `#RRGGBB`.
    pub color: Option<String>,
}

/// Contributions d'un plugin. Tous les champs d'apparence sont facultatifs :
/// un plugin qui ne change que le dock reste valable.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BackendHook {
    /// `game_launch` ou `game_stop`.
    pub event: String,
    /// Ligne de commande exécutée avec le shell de l'utilisateur.
    pub command: String,
}

/// Page interactive déclarée par un plugin. L'apparence vient des commandes
/// de dessin renvoyées par sa bibliothèque native ; le manifeste ne fournit
/// que l'entrée de navigation et un repli textuel.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PluginPage {
    pub title: String,
    pub icon: String,
    pub subtitle: String,
    /// Mot ou contenu court propre au plugin, traduit avec la page.
    pub word: String,
    pub footer: String,
    /// Rempli par `resolve`, pas par le manifeste : identifiant du plugin dont
    /// la bibliothèque native fournit le rendu.
    #[serde(skip)]
    pub plugin: String,
}

/// Textes d'un plugin dans une langue. Tous les champs sont facultatifs : un
/// champ absent retombe sur la valeur de base du manifeste.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PluginTranslation {
    pub name: Option<String>,
    pub description: Option<String>,
    pub home_label: Option<String>,
    pub home_subtitle: Option<String>,
    pub page_title: Option<String>,
    pub page_subtitle: Option<String>,
    pub page_word: Option<String>,
    pub page_footer: Option<String>,
    /// Libellés du dock, indexés par libellé d'origine, clé de page ou URL.
    pub dock: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub accent: Option<String>,
    pub background: Option<String>,
    pub panel: Option<String>,
    pub card: Option<String>,
    pub card_hover: Option<String>,
    pub card_active: Option<String>,
    pub border: Option<String>,
    pub text: Option<String>,
    pub text_dim: Option<String>,
    pub layout: Option<String>,
    pub rounding: Option<f32>,
    pub font_scale: Option<f32>,
    pub home_label: Option<String>,
    pub home_subtitle: Option<String>,
    /// Page interactive fournie par le plugin, quand il en déclare une.
    pub page: Option<PluginPage>,
    /// Bibliothèque native chargée par Lapinux, quand le plugin en fournit
    /// une. Le chemin est relatif au manifeste, ou absolu.
    pub native: Option<NativePlugin>,
    /// Textes traduits, indexés par code de langue (`fr`, `en`…).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub translations: BTreeMap<String, PluginTranslation>,
    /// Sections cachées de la barre du haut, par clé de page.
    pub hide_pages: Vec<String>,
    pub dock: Vec<DockEntry>,
    /// Commandes lancées par le daemon aux événements de partie.
    pub backend_hooks: Vec<BackendHook>,
}

/// Bibliothèque native d'un plugin : un `.so` chargé par Lapinux au démarrage.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct NativePlugin {
    /// Fichier de la bibliothèque, relatif au manifeste ou absolu.
    pub library: String,
    /// Le plugin fournit un thème natif (`theme` dans son descripteur).
    /// Il est chargé même s'il n'est pas dans `active_plugins` pour pouvoir
    /// apparaître dans la liste des thèmes.
    pub theme: bool,
}

impl Default for PluginManifest {
    fn default() -> Self {
        Self {
            id: "mon-plugin".to_string(),
            name: lapinux_core::i18n::t("theme.plugin_default_name").to_string(),
            version: "0.1.0".to_string(),
            author: String::new(),
            description: lapinux_core::i18n::t("theme.plugin_description").to_string(),
            accent: None,
            background: None,
            panel: None,
            card: None,
            card_hover: None,
            card_active: None,
            border: None,
            text: None,
            text_dim: None,
            layout: None,
            rounding: None,
            font_scale: None,
            home_label: None,
            home_subtitle: None,
            page: None,
            native: None,
            translations: BTreeMap::new(),
            hide_pages: Vec::new(),
            dock: Vec::new(),
            backend_hooks: Vec::new(),
        }
    }
}

impl PluginManifest {
    pub fn validate(&self) -> Result<(), String> {
        validate_id(&self.id)?;
        if self.name.trim().is_empty() {
            return Err(lapinux_core::i18n::t("theme.empty_plugin_name").to_string());
        }
        for (label, value) in [
            ("accent", &self.accent),
            ("background", &self.background),
            ("panel", &self.panel),
            ("card", &self.card),
            ("card_hover", &self.card_hover),
            ("card_active", &self.card_active),
            ("border", &self.border),
            ("text", &self.text),
            ("text_dim", &self.text_dim),
        ] {
            if let Some(value) = value {
                if !value.trim().is_empty() && parse_color(value).is_none() {
                    return Err(lapinux_core::i18n::t_args(
                        "theme.invalid_color",
                        &[("label", &label.to_string()), ("value", &value.to_string())],
                    ));
                }
            }
        }
        if let Some(layout) = &self.layout {
            if !layout.trim().is_empty() && ThemeLayout::from_key(layout).is_none() {
                return Err(lapinux_core::i18n::t("theme.unknown_layout").to_string());
            }
        }
        if let Some(rounding) = self.rounding {
            if !(0.0..=36.0).contains(&rounding) {
                return Err(lapinux_core::i18n::t("theme.rounding_range").to_string());
            }
        }
        if let Some(scale) = self.font_scale {
            if !(0.75..=1.6).contains(&scale) {
                return Err(lapinux_core::i18n::t("theme.font_scale_range").to_string());
            }
        }
        for page in &self.hide_pages {
            if !valid_page_key(page) {
                return Err(lapinux_core::i18n::t_args(
                    "theme.unknown_hidden_page",
                    &[("page", &page.to_string())],
                ));
            }
        }
        for entry in &self.dock {
            if entry.label.trim().is_empty() {
                return Err(lapinux_core::i18n::t("theme.dock_missing_label").to_string());
            }
            if let Some(page) = &entry.page {
                if !valid_page_key(page) {
                    return Err(lapinux_core::i18n::t_args(
                        "theme.unknown_dock_page",
                        &[("page", &page.to_string())],
                    ));
                }
            }
            if let Some(url) = &entry.url {
                if !(url.starts_with("http://") || url.starts_with("https://")) {
                    return Err(lapinux_core::i18n::t_args(
                        "theme.invalid_dock_link",
                        &[("url", &url.to_string())],
                    ));
                }
            }
            if entry.page.is_none() && entry.url.is_none() {
                return Err(lapinux_core::i18n::t_fmt(
                    "theme.dock_target",
                    &[entry.label.clone()],
                ));
            }
            if let Some(color) = &entry.color {
                if !color.trim().is_empty() && parse_color(color).is_none() {
                    return Err(lapinux_core::i18n::t_args(
                        "theme.invalid_dock_color",
                        &[("color", &color.to_string())],
                    ));
                }
            }
        }
        for hook in &self.backend_hooks {
            if !matches!(hook.event.as_str(), "game_launch" | "game_stop") {
                return Err(lapinux_core::i18n::t_args(
                    "theme.unknown_backend_event",
                    &[("event", &hook.event)],
                ));
            }
            if hook.command.trim().is_empty() {
                return Err(lapinux_core::i18n::t("theme.empty_backend_command").to_string());
            }
        }
        if let Some(native) = &self.native {
            if native.library.trim().is_empty() {
                return Err(lapinux_core::i18n::t("theme.empty_native_library").to_string());
            }
        }
        Ok(())
    }

    /// Traduction exacte, puis variante régionale (`fr-CA` → `fr`).
    pub fn translation_for(&self, language: &str) -> Option<&PluginTranslation> {
        let raw = language.trim().to_ascii_lowercase().replace('_', "-");
        if raw.is_empty() {
            return None;
        }
        if let Some(translation) = self.translations.get(&raw) {
            return Some(translation);
        }
        let base = raw.split('-').next().unwrap_or_default();
        self.translations.get(base).or_else(|| {
            self.translations
                .iter()
                .find(|(code, _)| normalized_language(code) == base)
                .map(|(_, translation)| translation)
        })
    }

    pub fn localized_name(&self, language: &str) -> &str {
        self.translation_for(language)
            .and_then(|translation| translation.name.as_deref())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or(&self.name)
    }

    pub fn localized_description(&self, language: &str) -> &str {
        self.translation_for(language)
            .and_then(|translation| translation.description.as_deref())
            .filter(|description| !description.trim().is_empty())
            .unwrap_or(&self.description)
    }

    pub fn localized_home_label(&self, language: &str) -> Option<&str> {
        self.translation_for(language)
            .and_then(|translation| translation.home_label.as_deref())
            .or(self.home_label.as_deref())
            .filter(|label| !label.trim().is_empty())
    }

    pub fn localized_home_subtitle(&self, language: &str) -> Option<&str> {
        self.translation_for(language)
            .and_then(|translation| translation.home_subtitle.as_deref())
            .or(self.home_subtitle.as_deref())
            .filter(|subtitle| !subtitle.trim().is_empty())
    }

    pub fn localized_page_title(&self, language: &str) -> Option<&str> {
        self.translation_for(language)
            .and_then(|translation| translation.page_title.as_deref())
            .filter(|title| !title.trim().is_empty())
            .or_else(|| {
                self.page
                    .as_ref()
                    .map(|page| page.title.as_str())
                    .filter(|title| !title.trim().is_empty())
            })
    }

    pub fn localized_page_subtitle(&self, language: &str) -> Option<&str> {
        self.translation_for(language)
            .and_then(|translation| translation.page_subtitle.as_deref())
            .filter(|subtitle| !subtitle.trim().is_empty())
            .or_else(|| {
                self.page
                    .as_ref()
                    .map(|page| page.subtitle.as_str())
                    .filter(|subtitle| !subtitle.trim().is_empty())
            })
    }

    pub fn localized_page_word(&self, language: &str) -> Option<&str> {
        self.translation_for(language)
            .and_then(|translation| translation.page_word.as_deref())
            .filter(|word| !word.trim().is_empty())
            .or_else(|| {
                self.page
                    .as_ref()
                    .map(|page| page.word.as_str())
                    .filter(|word| !word.trim().is_empty())
            })
    }

    pub fn localized_page_footer(&self, language: &str) -> Option<&str> {
        self.translation_for(language)
            .and_then(|translation| translation.page_footer.as_deref())
            .filter(|footer| !footer.trim().is_empty())
            .or_else(|| {
                self.page
                    .as_ref()
                    .map(|page| page.footer.as_str())
                    .filter(|footer| !footer.trim().is_empty())
            })
    }

    pub fn localized_dock_label<'a>(
        &'a self,
        label: &str,
        entry: &DockEntry,
        language: &str,
    ) -> Option<&'a str> {
        let translation = self.translation_for(language)?;
        translation
            .dock
            .get(label)
            .or_else(|| {
                entry
                    .page
                    .as_ref()
                    .and_then(|page| translation.dock.get(page))
            })
            .or_else(|| entry.url.as_ref().and_then(|url| translation.dock.get(url)))
            .map(String::as_str)
            .filter(|label| !label.trim().is_empty())
    }
}

/// Raccourci résolu, prêt à être dessiné dans le dock.
#[derive(Debug, Clone, PartialEq)]
pub struct DockShortcut {
    pub label: String,
    pub icon: String,
    pub page: Option<String>,
    pub url: Option<String>,
    pub color: Color32,
}

/// Thème effectif : le manifeste actif, éventuellement surchargé par les
/// plugins cochés.
#[derive(Debug, Clone, PartialEq)]
pub struct ActiveTheme {
    pub id: String,
    pub name: String,
    /// Le thème actif vient d'un `.so` et peut fournir une frame de fond.
    pub native: bool,
    pub colors: Palette,
    pub rounding: f32,
    pub font_scale: f32,
    pub home_label: String,
    pub home_subtitle: String,
    /// Page fournie par un plugin actif.
    pub plugin_page: Option<PluginPage>,
    pub hidden_pages: Vec<String>,
    pub dock: Vec<DockShortcut>,
}

impl Default for ActiveTheme {
    fn default() -> Self {
        Self::from_manifest(&ThemeManifest::classic())
    }
}

impl ActiveTheme {
    pub fn from_manifest(manifest: &ThemeManifest) -> Self {
        Self {
            id: manifest.id.clone(),
            name: manifest.name.clone(),
            native: false,
            colors: manifest.palette(),
            rounding: manifest.rounding,
            font_scale: manifest.font_scale,
            home_label: manifest.home_label.clone(),
            home_subtitle: manifest.home_subtitle.clone(),
            plugin_page: None,
            hidden_pages: Vec::new(),
            dock: Vec::new(),
        }
    }

    pub fn hides(&self, page: &str) -> bool {
        self.hidden_pages.iter().any(|hidden| hidden == page)
    }
}

#[derive(Debug, Clone)]
pub struct Theme {
    pub manifest: ThemeManifest,
    pub builtin: bool,
    /// Thème fourni par une bibliothèque native : sa palette et sa frame
    /// viennent du `.so`, pas d'un bloc TOML.
    pub native: bool,
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct Plugin {
    pub manifest: PluginManifest,
    pub path: Option<PathBuf>,
}

/// Thèmes disponibles : les deux thèmes fournis, puis les fichiers de
/// `~/.config/lapinux/themes/*.toml`.
#[derive(Debug, Clone)]
pub struct ThemeLibrary {
    pub themes: Vec<Theme>,
    pub dir: PathBuf,
    pub error: Option<String>,
}

impl ThemeLibrary {
    pub fn load(dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        let mut library = Self {
            // Seul le thème classique est compilé : NX vit dans le catalogue
            // communautaire et n'existe qu'une fois installé.
            themes: vec![Theme {
                manifest: ThemeManifest::classic(),
                builtin: true,
                native: false,
                path: None,
            }],
            dir,
            error: None,
        };
        library.reload_user();
        library
    }

    fn reload_user(&mut self) {
        let mut errors = Vec::new();
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            self.error = None;
            return;
        };
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("toml"))
            .collect();
        paths.sort();
        for path in paths {
            let text = match std::fs::read_to_string(&path) {
                Ok(text) => text,
                Err(error) => {
                    errors.push(format!("{} : {error}", path.display()));
                    continue;
                }
            };
            let manifest = match toml::from_str::<ThemeManifest>(&text) {
                Ok(manifest) => manifest,
                Err(error) => {
                    errors.push(format!("{} : {error}", path.display()));
                    continue;
                }
            };
            if let Err(error) = manifest.validate() {
                errors.push(format!("{} : {error}", path.display()));
                continue;
            }
            if self
                .themes
                .iter()
                .any(|theme| theme.manifest.id == manifest.id)
            {
                errors.push(lapinux_core::i18n::t_fmt(
                    "theme.id_taken",
                    &[path.display().to_string(), manifest.id.to_string()],
                ));
                continue;
            }
            self.themes.push(Theme {
                manifest,
                builtin: false,
                native: false,
                path: Some(path),
            });
        }
        self.error = (!errors.is_empty()).then(|| errors.join("\n"));
    }

    pub fn reload(&mut self) {
        self.themes.retain(|theme| theme.builtin || theme.native);
        self.reload_user();
    }

    /// Retire le thème fourni par un plugin qui vient d'être supprimé, puis
    /// relit les thèmes TOML du dossier : un thème déclaratif du même
    /// identifiant doit réapparaître immédiatement, sans redémarrage. Les
    /// autres thèmes natifs restent disponibles.
    pub fn drop_native(&mut self, id: &str) {
        self.themes
            .retain(|theme| !(theme.native && theme.manifest.id.eq_ignore_ascii_case(id)));
        self.reload();
    }

    /// Enregistre ou rafraîchit un thème fourni par un `.so`.
    pub fn register_native_theme(&mut self, manifest: ThemeManifest) {
        if manifest.validate().is_err() {
            return;
        }
        if let Some(existing) = self
            .themes
            .iter_mut()
            .find(|theme| theme.manifest.id == manifest.id)
        {
            // Un `.so` installé remplace le manifeste TOML du même identifiant :
            // c'est ainsi que `nx` passe du thème déclaratif au thème natif.
            if existing.native || !existing.builtin {
                existing.manifest = manifest;
                existing.native = true;
                existing.path = None;
            }
            return;
        }
        self.themes.push(Theme {
            manifest,
            builtin: false,
            native: true,
            path: None,
        });
    }

    pub fn active(&self, id: &str) -> &Theme {
        let id = id.trim();
        let id = if CLASSIC_ALIASES
            .iter()
            .any(|alias| id.eq_ignore_ascii_case(alias))
        {
            CLASSIC_ID
        } else {
            id
        };
        self.themes
            .iter()
            .find(|theme| theme.manifest.id.eq_ignore_ascii_case(id))
            .unwrap_or(&self.themes[0])
    }

    pub fn find(&self, id: &str) -> Option<&Theme> {
        self.themes
            .iter()
            .find(|theme| theme.manifest.id.eq_ignore_ascii_case(id))
    }

    /// Résout le thème actif et applique les plugins dans leur ordre de
    /// configuration. Le dernier plugin qui touche une couleur gagne.
    pub fn resolve(&self, id: &str, plugins: &[PluginManifest]) -> ActiveTheme {
        let theme = self.active(id);
        let mut active = ActiveTheme::from_manifest(&theme.manifest);
        active.native = theme.native;
        let language = lapinux_core::i18n::active();
        for plugin in plugins {
            if let Some(value) = colour_override(&plugin.accent) {
                active.colors.accent = value;
            }
            if let Some(value) = colour_override(&plugin.background) {
                active.colors.background = value;
            }
            if let Some(value) = colour_override(&plugin.panel) {
                active.colors.panel = value;
            }
            if let Some(value) = colour_override(&plugin.card) {
                active.colors.card = value;
            }
            if let Some(value) = colour_override(&plugin.card_hover) {
                active.colors.card_hover = value;
            }
            if let Some(value) = colour_override(&plugin.card_active) {
                active.colors.card_active = value;
            }
            if let Some(value) = colour_override(&plugin.border) {
                active.colors.border = value;
            }
            if let Some(value) = colour_override(&plugin.text) {
                active.colors.text = value;
            }
            if let Some(value) = colour_override(&plugin.text_dim) {
                active.colors.text_dim = value;
            }
            if let Some(rounding) = plugin.rounding {
                active.rounding = rounding;
            }
            if let Some(scale) = plugin.font_scale {
                active.font_scale = scale;
            }
            if let Some(label) = plugin.localized_home_label(language) {
                active.home_label = label.to_string();
            }
            if let Some(subtitle) = plugin.localized_home_subtitle(language) {
                active.home_subtitle = subtitle.to_string();
            }
            if let Some(page) = plugin.page.as_ref() {
                let mut page = page.clone();
                if let Some(title) = plugin.localized_page_title(language) {
                    page.title = title.to_string();
                }
                if let Some(subtitle) = plugin.localized_page_subtitle(language) {
                    page.subtitle = subtitle.to_string();
                }
                if let Some(word) = plugin.localized_page_word(language) {
                    page.word = word.to_string();
                }
                if let Some(footer) = plugin.localized_page_footer(language) {
                    page.footer = footer.to_string();
                }
                page.plugin = plugin.id.clone();
                active.plugin_page = Some(page);
            }
            for page in &plugin.hide_pages {
                if !active.hides(page) {
                    active.hidden_pages.push(page.clone());
                }
            }
            for entry in &plugin.dock {
                active.dock.push(DockShortcut {
                    label: plugin
                        .localized_dock_label(&entry.label, entry, language)
                        .unwrap_or(&entry.label)
                        .to_string(),
                    icon: entry.icon.clone(),
                    page: entry.page.clone(),
                    url: entry.url.clone(),
                    color: entry
                        .color
                        .as_deref()
                        .and_then(parse_color)
                        .unwrap_or(active.colors.accent),
                });
            }
        }
        active
    }

    #[cfg(test)]
    pub fn save(
        &mut self,
        manifest: &ThemeManifest,
        previous: Option<&Path>,
    ) -> Result<PathBuf, String> {
        manifest.validate()?;
        if RESERVED_IDS
            .iter()
            .any(|id| manifest.id.eq_ignore_ascii_case(id))
        {
            return Err(lapinux_core::i18n::t_fmt(
                "theme.reserved_id",
                &[manifest.id.to_string()],
            ));
        }
        if self.themes.iter().any(|theme| {
            !theme.builtin && theme.manifest.id == manifest.id && theme.path.as_deref() != previous
        }) {
            return Err(lapinux_core::i18n::t_fmt(
                "theme.theme_exists",
                &[manifest.id.to_string()],
            ));
        }
        std::fs::create_dir_all(&self.dir).map_err(|error| error.to_string())?;
        let path = self.dir.join(format!("{}.toml", manifest.id));
        let body = toml::to_string_pretty(manifest).map_err(|error| error.to_string())?;
        std::fs::write(
            &path,
            format!("# {}\n{body}", lapinux_core::i18n::t("theme.file_header")),
        )
        .map_err(|error| error.to_string())?;
        if let Some(previous) = previous {
            if previous != path {
                let _ = std::fs::remove_file(previous);
            }
        }
        self.reload();
        Ok(path)
    }

    pub fn delete(&mut self, id: &str) -> Result<(), String> {
        let Some(theme) = self.find(id) else {
            return Err(lapinux_core::i18n::t("theme.not_found").to_string());
        };
        if theme.builtin {
            return Err(lapinux_core::i18n::t("theme.builtin_undeletable").to_string());
        }
        let Some(path) = theme.path.clone() else {
            return Err(lapinux_core::i18n::t("theme.no_file").to_string());
        };
        std::fs::remove_file(path).map_err(|error| error.to_string())?;
        self.reload();
        Ok(())
    }
}

/// Plugins disponibles dans `~/.config/lapinux/plugins/*.toml`. Aucun plugin
/// n'est actif par défaut.
#[derive(Debug, Clone)]
pub struct PluginLibrary {
    pub plugins: Vec<Plugin>,
    pub dir: PathBuf,
    pub error: Option<String>,
}

impl PluginLibrary {
    pub fn load(dir: impl Into<PathBuf>) -> Self {
        let mut library = Self {
            plugins: Vec::new(),
            dir: dir.into(),
            error: None,
        };
        library.reload();
        library
    }

    pub fn reload(&mut self) {
        self.plugins.clear();
        let mut errors = Vec::new();
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            self.error = None;
            return;
        };
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("toml"))
            .collect();
        paths.sort();
        for path in paths {
            let text = match std::fs::read_to_string(&path) {
                Ok(text) => text,
                Err(error) => {
                    errors.push(format!("{} : {error}", path.display()));
                    continue;
                }
            };
            let manifest = match toml::from_str::<PluginManifest>(&text) {
                Ok(manifest) => manifest,
                Err(error) => {
                    errors.push(format!("{} : {error}", path.display()));
                    continue;
                }
            };
            if let Err(error) = manifest.validate() {
                errors.push(format!("{} : {error}", path.display()));
                continue;
            }
            if self
                .plugins
                .iter()
                .any(|plugin| plugin.manifest.id == manifest.id)
            {
                errors.push(lapinux_core::i18n::t_fmt(
                    "theme.id_taken",
                    &[path.display().to_string(), manifest.id.to_string()],
                ));
                continue;
            }
            self.plugins.push(Plugin {
                manifest,
                path: Some(path),
            });
        }
        self.error = (!errors.is_empty()).then(|| errors.join("\n"));
    }

    /// Plugins actifs avec leur manifeste et leur chemin, pour le chargeur
    /// natif. L'ordre de `ids` est conservé.
    pub fn active_plugins(&self, ids: &[String]) -> Vec<Plugin> {
        ids.iter().filter_map(|id| self.find(id).cloned()).collect()
    }

    pub fn find(&self, id: &str) -> Option<&Plugin> {
        self.plugins
            .iter()
            .find(|plugin| plugin.manifest.id.eq_ignore_ascii_case(id))
    }

    pub fn delete(&mut self, id: &str) -> Result<(), String> {
        let Some(plugin) = self.find(id) else {
            return Err(lapinux_core::i18n::t("theme.plugin_not_found").to_string());
        };
        let Some(path) = plugin.path.clone() else {
            return Err(lapinux_core::i18n::t("theme.plugin_without_file").to_string());
        };
        let library = plugin
            .manifest
            .native
            .as_ref()
            .map(|native| native.library.clone());
        std::fs::remove_file(path).map_err(|error| error.to_string())?;
        // Le manifeste seul ne suffisait pas : le `.so` restait dans le dossier
        // et le plugin pouvait réapparaître au prochain chargement.
        if let Some(library) = library {
            let manifest = plugin.path.as_deref().unwrap_or(Path::new(""));
            let candidate = if Path::new(&library).is_absolute() {
                PathBuf::from(&library)
            } else {
                manifest
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .join(&library)
            };
            if let (Ok(root), Ok(candidate)) = (self.dir.canonicalize(), candidate.canonicalize()) {
                if candidate.starts_with(root) {
                    let _ = std::fs::remove_file(candidate);
                }
            }
        }
        self.reload();
        Ok(())
    }
}

#[cfg(test)]
pub fn theme_toml(manifest: &ThemeManifest) -> String {
    toml::to_string_pretty(manifest).unwrap_or_default()
}

#[cfg(test)]
pub fn plugin_toml(manifest: &PluginManifest) -> String {
    toml::to_string_pretty(manifest).unwrap_or_default()
}

#[cfg(test)]
pub fn parse_theme(text: &str) -> Result<ThemeManifest, String> {
    let manifest: ThemeManifest = toml::from_str(text).map_err(|error| {
        lapinux_core::i18n::t_args("theme.invalid_toml", &[("error", &error.to_string())])
    })?;
    manifest.validate()?;
    Ok(manifest)
}

#[cfg(test)]
pub fn parse_plugin(text: &str) -> Result<PluginManifest, String> {
    let manifest: PluginManifest = toml::from_str(text).map_err(|error| {
        lapinux_core::i18n::t_args("theme.invalid_toml", &[("error", &error.to_string())])
    })?;
    manifest.validate()?;
    Ok(manifest)
}

/// Convertit `#RRGGBB` ou `#RRGGBBAA` en couleur egui.
pub fn parse_color(value: &str) -> Option<Color32> {
    let value = value.trim().trim_start_matches('#');
    if !value.is_ascii() || (value.len() != 6 && value.len() != 8) {
        return None;
    }
    let mut channels = [255_u8; 4];
    for (index, chunk) in value.as_bytes().chunks(2).enumerate() {
        let text = std::str::from_utf8(chunk).ok()?;
        channels[index] = u8::from_str_radix(text, 16).ok()?;
    }
    Some(Color32::from_rgba_unmultiplied(
        channels[0],
        channels[1],
        channels[2],
        channels[3],
    ))
}

fn colour_override(value: &Option<String>) -> Option<Color32> {
    value.as_deref().and_then(parse_color)
}

fn hex(color: Color32) -> String {
    format!("#{:02x}{:02x}{:02x}", color.r(), color.g(), color.b())
}

fn spec_color(color: [u8; 4]) -> Color32 {
    Color32::from_rgba_unmultiplied(color[0], color[1], color[2], color[3])
}

fn validate_id(id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err(lapinux_core::i18n::t("theme.empty_id").to_string());
    }
    if !id
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_')
    {
        return Err(lapinux_core::i18n::t("theme.invalid_id").to_string());
    }
    Ok(())
}

fn valid_page_key(page: &str) -> bool {
    matches!(
        page,
        "library"
            | "collections"
            | "downloads"
            | "controllers"
            | "profile"
            | "boutiques"
            | "captures"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn un_manifeste_lit_sa_bibliotheque_native() {
        let manifest: PluginManifest = toml::from_str(
            "id = \"natif\"\nname = \"Natif\"\n[native]\nlibrary = \"libnatif.so\"\n",
        )
        .unwrap();
        assert_eq!(
            manifest
                .native
                .as_ref()
                .map(|native| native.library.as_str()),
            Some("libnatif.so")
        );
        manifest.validate().unwrap();
    }

    #[test]
    fn classic_palette_is_the_historical_one() {
        let manifest = ThemeManifest::classic();
        assert_eq!(manifest.palette(), Palette::classic());
        assert_eq!(manifest.layout(), ThemeLayout::Classic);
    }

    #[test]
    fn nx_is_a_real_layout_with_switch_colors() {
        let manifest = ThemeManifest::nx();
        assert_eq!(manifest.layout(), ThemeLayout::Nx);
        assert_eq!(manifest.palette().accent, Color32::from_rgb(0, 200, 224));
        assert_ne!(manifest.palette(), Palette::classic());
    }

    #[test]
    fn plugin_overrides_are_applied_in_order() {
        let library = ThemeLibrary::load("/tmp/lapinux-theme-test-missing");
        let plugin = PluginManifest {
            id: "test".into(),
            name: "Test".into(),
            accent: Some("#ff0000".into()),
            layout: Some("nx".into()),
            home_label: Some("Accueil test".into()),
            dock: vec![DockEntry {
                label: "Docs".into(),
                icon: "D".into(),
                page: None,
                url: Some("https://example.com".into()),
                color: Some("#00ff00".into()),
            }],
            ..PluginManifest::default()
        };
        let active = library.resolve(CLASSIC_ID, &[plugin]);
        assert_eq!(active.colors.accent, Color32::from_rgb(255, 0, 0));
        assert_eq!(active.home_label, "Accueil test");
        assert_eq!(active.dock.len(), 1);
        assert_eq!(active.dock[0].color, Color32::from_rgb(0, 255, 0));
    }

    #[test]
    fn a_plugin_can_add_its_own_page() {
        let library = ThemeLibrary::load("/tmp/lapinux-theme-test-missing");
        let plugin = PluginManifest {
            id: "chat".into(),
            name: "Miaou".into(),
            page: Some(PluginPage {
                title: "Miaou".into(),
                icon: "cat".into(),
                subtitle: "Une pluie de miaou.".into(),
                footer: "=^.^=".into(),
                ..PluginPage::default()
            }),
            ..PluginManifest::default()
        };
        let active = library.resolve(CLASSIC_ID, &[plugin]);
        let page = active.plugin_page.expect("page du plugin");
        assert_eq!(page.title, "Miaou");
        assert_eq!(page.plugin, "chat");
    }

    #[test]
    fn un_plugin_traduit_ses_textes_et_retombe_sur_la_base() {
        let text = r##"id = "chat"
name = "Miaou"
description = "Une pluie de miaou."
home_label = "Mon espace"

[page]
title = "Miaou"
icon = "cat"
subtitle = "Une pluie de miaou."
word = "miaou"
footer = "=^.^="

[translations.fr]
page_title = "Miaou"

[translations.en]
name = "Meow"
description = "A rain of meows."
home_label = "My space"
page_title = "Meow"
page_subtitle = "A rain of meows."
page_word = "meow"
page_footer = "=^.^="

[translations.en.dock]
"Ma page" = "My page"

[[dock]]
label = "Ma page"
icon = "M"
page = "downloads"
"##;
        let plugin = parse_plugin(text).expect("plugin traduit");
        assert_eq!(plugin.localized_name("en"), "Meow");
        assert_eq!(plugin.localized_name("en-GB"), "Meow");
        assert_eq!(plugin.localized_name("de"), "Miaou");
        assert_eq!(plugin.localized_description("en"), "A rain of meows.");
        assert_eq!(plugin.localized_home_label("en"), Some("My space"));
        assert_eq!(plugin.localized_page_title("en"), Some("Meow"));
        assert_eq!(
            plugin.localized_page_subtitle("en"),
            Some("A rain of meows.")
        );
        assert_eq!(plugin.localized_page_word("en"), Some("meow"));
        assert_eq!(plugin.localized_page_footer("en"), Some("=^.^="));
        assert_eq!(
            plugin.localized_dock_label("Ma page", &plugin.dock[0], "en"),
            Some("My page")
        );

        let roundtrip = parse_plugin(&plugin_toml(&plugin)).expect("traductions relues");
        assert_eq!(roundtrip.localized_name("en"), "Meow");
        assert_eq!(
            roundtrip.localized_dock_label("Ma page", &roundtrip.dock[0], "en"),
            Some("My page")
        );
    }

    #[test]
    fn le_theme_actif_reprend_les_traductions_du_plugin() {
        let library = ThemeLibrary::load("/tmp/lapinux-theme-test-missing");
        let mut plugin = PluginManifest {
            id: "chat".into(),
            name: "Miaou".into(),
            home_label: Some("Mon espace".into()),
            page: Some(PluginPage {
                title: "Miaou".into(),
                icon: "cat".into(),
                subtitle: "Une pluie de miaou.".into(),
                footer: "=^.^=".into(),
                ..PluginPage::default()
            }),
            ..PluginManifest::default()
        };
        let mut english = PluginTranslation::default();
        english.name = Some("Meow".into());
        english.home_label = Some("My space".into());
        english.page_title = Some("Meow".into());
        english.page_subtitle = Some("A rain of meows.".into());
        english.page_word = Some("meow".into());
        english.page_footer = Some("=^.^=".into());
        plugin.translations.insert("en".into(), english);

        let active = library.resolve(CLASSIC_ID, &[plugin]);
        let language = lapinux_core::i18n::active();
        if language == "en" {
            assert_eq!(active.home_label, "My space");
            let page = active.plugin_page.unwrap();
            assert_eq!(page.title, "Meow");
            assert_eq!(page.subtitle, "A rain of meows.");
        } else {
            assert_eq!(active.home_label, "Mon espace");
            let page = active.plugin_page.unwrap();
            assert_eq!(page.title, "Miaou");
            assert_eq!(page.subtitle, "Une pluie de miaou.");
        }
    }

    #[test]
    fn user_theme_round_trips_through_disk() {
        let temp = tempfile::tempdir().unwrap();
        let mut library = ThemeLibrary::load(temp.path());
        let mut manifest = ThemeManifest::classic();
        manifest.id = "perso".into();
        manifest.name = "Perso".into();
        manifest.accent = "#123456".into();
        library.save(&manifest, None).unwrap();
        let loaded = ThemeLibrary::load(temp.path());
        let found = loaded.find("perso").unwrap();
        assert_eq!(found.manifest.accent, "#123456");
        assert!(!found.builtin);
    }

    #[test]
    fn bad_colour_and_unknown_layout_are_rejected() {
        let mut manifest = ThemeManifest::nx();
        manifest.id = "test".into();
        manifest.accent = "rouge".into();
        assert!(manifest.validate().is_err());
        manifest.accent = "#ff0000".into();
        manifest.layout = lapinux_core::i18n::t("controllers.information.unknown_band").into();
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn old_theme_names_fall_back_to_classic() {
        let library = ThemeLibrary::load("/tmp/lapinux-theme-test-missing");
        assert_eq!(library.active("sombre").manifest.id, CLASSIC_ID);
        assert_eq!(library.active("").manifest.id, CLASSIC_ID);
        // NX n'est plus compilé : sans fichier installé, il retombe sur le
        // thème classique au lieu de créer un thème fantôme.
        assert_eq!(library.active("nx").manifest.id, CLASSIC_ID);
    }

    #[test]
    fn nx_installe_depuis_le_catalogue_devient_actif() {
        let temp = tempfile::tempdir().unwrap();
        let manifest = ThemeManifest::nx();
        std::fs::write(temp.path().join("nx.toml"), theme_toml(&manifest)).unwrap();
        let library = ThemeLibrary::load(temp.path());
        let nx = library.find(NX_ID).expect("thème NX installé");
        assert!(!nx.builtin);
        assert_eq!(library.active("nx").manifest.id, NX_ID);
    }

    #[test]
    fn un_theme_natif_remplace_le_manifeste_toml_de_meme_id() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("nx.toml"),
            theme_toml(&ThemeManifest::nx()),
        )
        .unwrap();
        let mut library = ThemeLibrary::load(temp.path());
        assert!(!library.find(NX_ID).unwrap().native);

        let spec = ThemeSpec {
            colors: lapinux_plugin_api::ThemeColors {
                accent: [1, 2, 3, 255],
                background: [4, 5, 6, 255],
                ..Default::default()
            },
            rounding: 14.0,
            font_scale: 1.1,
        };
        let mut plugin = PluginManifest {
            id: NX_ID.into(),
            name: "NX natif".into(),
            home_label: Some("Maison".into()),
            home_subtitle: Some("Sous-titre".into()),
            ..PluginManifest::default()
        };
        let mut english = PluginTranslation::default();
        english.name = Some("NX native".into());
        english.home_label = Some("Home".into());
        english.home_subtitle = Some("Subtitle".into());
        plugin.translations.insert("en".into(), english);

        library.register_native_theme(ThemeManifest::from_native(NX_ID, &spec, &plugin, "fr"));
        let theme = library.find(NX_ID).unwrap();
        assert!(theme.native);
        assert_eq!(theme.manifest.name, "NX natif");
        assert_eq!(theme.manifest.home_label, "Maison");
        assert_eq!(theme.manifest.home_subtitle, "Sous-titre");
        assert_eq!(theme.manifest.palette().accent, Color32::from_rgb(1, 2, 3));
        assert_eq!(library.active(NX_ID).manifest.id, NX_ID);

        let translated = ThemeManifest::from_native(NX_ID, &spec, &plugin, "en");
        assert_eq!(translated.name, "NX native");
        assert_eq!(translated.home_label, "Home");
        assert_eq!(translated.home_subtitle, "Subtitle");
    }

    #[test]
    fn supprimer_un_theme_natif_recharge_le_toml_de_meme_id() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("nx.toml"),
            theme_toml(&ThemeManifest::nx()),
        )
        .unwrap();
        let mut library = ThemeLibrary::load(temp.path());
        let spec = ThemeSpec {
            colors: lapinux_plugin_api::ThemeColors::default(),
            rounding: 0.0,
            font_scale: 0.0,
        };
        let plugin = PluginManifest {
            id: NX_ID.into(),
            name: "NX natif".into(),
            ..PluginManifest::default()
        };
        library.register_native_theme(ThemeManifest::from_native(NX_ID, &spec, &plugin, "fr"));
        assert!(library.find(NX_ID).unwrap().native);

        library.drop_native(NX_ID);

        let restored = library.find(NX_ID).expect("le TOML doit réapparaître");
        assert!(!restored.native);
        assert_eq!(restored.manifest.name, "NX");
        assert_eq!(
            restored.path.as_ref().and_then(|path| path.file_name()),
            Some(std::ffi::OsStr::new("nx.toml"))
        );
        assert_eq!(library.active(NX_ID).manifest.id, NX_ID);
    }

    #[test]
    fn un_plugin_ecrit_avant_la_traduction_reste_lisible() {
        // Fichier tel que l'ancienne version l'écrivait : commentaires et
        // valeurs en français, mêmes clés TOML. Il doit continuer à se lire.
        let ancien = r##"# Plugin Lapinux : aucune commande n'est exécutée.
id = "vieux-plugin"
name = "Vieux plugin"
version = "0.1.0"
author = ""
description = "Décrivez ce que ce plugin change."
home_label = "Mon espace"
home_subtitle = "Un accueil personnalisé par mon plugin."
# accent = "#4cc9f0"
"##;
        let parsed = parse_plugin(ancien).expect("un ancien plugin reste lisible");
        assert_eq!(parsed.id, "vieux-plugin");
        assert_eq!(parsed.home_label.as_deref(), Some("Mon espace"));
        assert_eq!(parsed.description, "Décrivez ce que ce plugin change.");
    }

    #[test]
    fn un_plugin_cree_est_ecrit_puis_relu_avec_ses_hooks() {
        let temp = tempfile::tempdir().unwrap();
        let text = r##"id = "e2e-plugin"
name = "Plugin E2E"
version = "0.1.0"
author = "Lapinux"
description = "Plugin de vérification."

[[backend_hooks]]
event = "game_launch"
command = "printf 'E2E=1\\n'"
"##;
        let manifest = parse_plugin(text).expect("manifeste de plugin valide");
        let path = temp.path().join("e2e-plugin.toml");
        std::fs::write(&path, toml::to_string_pretty(&manifest).unwrap()).unwrap();
        let reloaded = PluginLibrary::load(temp.path());
        let plugin = reloaded.find("e2e-plugin").expect("plugin relu");
        assert_eq!(plugin.manifest.name, "Plugin E2E");
        assert_eq!(plugin.manifest.backend_hooks.len(), 1);
        assert_eq!(plugin.manifest.backend_hooks[0].event, "game_launch");
    }

    #[test]
    fn supprimer_un_plugin_retire_aussi_sa_bibliotheque_native() {
        let temp = tempfile::tempdir().unwrap();
        let text = r#"id = "native-test"
name = "Plugin natif"
version = "0.1.0"
author = "Lapinux"
description = "Plugin natif de test."

[native]
library = "libnative_test.so"
"#;
        let manifest = parse_plugin(text).expect("manifeste natif valide");
        let manifest_path = temp.path().join("native-test.toml");
        let library_path = temp.path().join("libnative_test.so");
        std::fs::write(&manifest_path, toml::to_string_pretty(&manifest).unwrap()).unwrap();
        std::fs::write(&library_path, b"binaire factice").unwrap();
        let mut library = PluginLibrary::load(temp.path());
        library.delete("native-test").expect("plugin supprimé");
        assert!(!manifest_path.exists());
        assert!(!library_path.exists());
        assert!(library.find("native-test").is_none());
    }

    #[test]
    fn les_alias_de_theme_anciens_et_neutres_restent_lus() {
        for key in ["classic", "default", "dark", "light", "sombre", "clair"] {
            assert_eq!(
                ThemeLayout::from_key(key),
                Some(ThemeLayout::Classic),
                "{key}"
            );
        }
        for key in ["nx", "switch", "console"] {
            assert_eq!(ThemeLayout::from_key(key), Some(ThemeLayout::Nx), "{key}");
        }
        // Un fichier écrit avant le changement (layout = "sombre") et sa
        // nouvelle écriture neutre se lisent tous deux.
        for alias in ["sombre", "dark"] {
            let mut manifest = ThemeManifest::classic();
            manifest.layout = alias.into();
            let text = theme_toml(&manifest);
            let parsed = parse_theme(&text).expect("thème relu");
            assert_eq!(parsed.layout(), ThemeLayout::Classic);
        }
        let mut manifest = ThemeManifest::nx();
        manifest.layout = "console".into();
        assert_eq!(
            parse_theme(&theme_toml(&manifest)).unwrap().layout(),
            ThemeLayout::Nx
        );
    }
}
