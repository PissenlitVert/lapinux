//! Thèmes et plugins de Lapinux.
//!
//! Un thème est un manifeste TOML : une palette, une disposition (`classic`
//! ou `nx`) et quelques réglages de forme. Les thèmes fournis (classique et
//! NX) sont compilés dans le binaire ; les thèmes créés par l'utilisateur
//! vivent dans `~/.config/lapinux/themes`.
//!
//! Un plugin est aussi un manifeste TOML, sans code exécutable. Il peut
//! surcharger les couleurs et la disposition du thème actif, changer le
//! libellé de l'accueil et ajouter des raccourcis au dock NX. Un plugin ne
//! peut donc pas lire de fichiers ni lancer de commandes arbitraires : ses
//! contributions restent dans le cadre de l'interface.

use egui::Color32;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Identifiant du thème historique, celui qui préserve l'interface actuelle.
pub const CLASSIC_ID: &str = "classic";
/// Identifiant du thème inspiré de la Nintendo Switch.
pub const NX_ID: &str = "nx";
/// Anciennes valeurs du champ `general.theme` : elles désignent le thème
/// classique, la palette claire n'ayant jamais eu de rendu distinct.
pub const CLASSIC_ALIASES: [&str; 4] = ["classic", "sombre", "clair", "default"];
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
            "classic" | "default" | "sombre" | "clair" => Some(Self::Classic),
            "nx" | "switch" | "console" => Some(Self::Nx),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Classic => "Classique",
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
        let value = self.background.r() as u32
            + self.background.g() as u32
            + self.background.b() as u32;
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
            name: "Classique".to_string(),
            author: "Lapinux".to_string(),
            description: "L'interface Lapinux historique, en gris neutre.".to_string(),
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
            home_subtitle: "Votre bibliothèque, comme une console.".to_string(),
        }
    }
}

impl ThemeManifest {
    pub fn classic() -> Self {
        Self::default()
    }

    pub fn nx() -> Self {
        let palette = Palette::nx();
        Self {
            id: NX_ID.to_string(),
            name: "NX".to_string(),
            author: "Lapinux".to_string(),
            description: "Accueil horizontal, carrousel de jeux et dock inspirés de la Nintendo Switch."
                .to_string(),
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
            home_label: "Jeux".to_string(),
            home_subtitle: "Choisissez un jeu et appuyez sur Entrée.".to_string(),
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
            return Err("Le nom du thème est vide.".to_string());
        }
        if !(self.layout.trim().is_empty() || ThemeLayout::from_key(&self.layout).is_some()) {
            return Err("Disposition inconnue : utilisez `classic` ou `nx`.".to_string());
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
                return Err(format!(
                    "Couleur `{label}` invalide : `{value}` (attendu #RRGGBB)."
                ));
            }
        }
        if !(0.0..=36.0).contains(&self.rounding) {
            return Err("`rounding` doit être compris entre 0 et 36.".to_string());
        }
        if !(0.75..=1.6).contains(&self.font_scale) {
            return Err("`font_scale` doit être compris entre 0.75 et 1.6.".to_string());
        }
        Ok(())
    }
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
    /// Sections cachées de la barre du haut, par clé de page.
    pub hide_pages: Vec<String>,
    pub dock: Vec<DockEntry>,
}

impl Default for PluginManifest {
    fn default() -> Self {
        Self {
            id: "mon-plugin".to_string(),
            name: "Mon plugin".to_string(),
            version: "0.1.0".to_string(),
            author: String::new(),
            description: "Décrivez ce que ce plugin change.".to_string(),
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
            hide_pages: Vec::new(),
            dock: Vec::new(),
        }
    }
}

impl PluginManifest {
    pub fn validate(&self) -> Result<(), String> {
        validate_id(&self.id)?;
        if self.name.trim().is_empty() {
            return Err("Le nom du plugin est vide.".to_string());
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
                    return Err(format!(
                        "Couleur `{label}` invalide : `{value}` (attendu #RRGGBB)."
                    ));
                }
            }
        }
        if let Some(layout) = &self.layout {
            if !layout.trim().is_empty() && ThemeLayout::from_key(layout).is_none() {
                return Err("Disposition inconnue : utilisez `classic` ou `nx`.".to_string());
            }
        }
        if let Some(rounding) = self.rounding {
            if !(0.0..=36.0).contains(&rounding) {
                return Err("`rounding` doit être compris entre 0 et 36.".to_string());
            }
        }
        if let Some(scale) = self.font_scale {
            if !(0.75..=1.6).contains(&scale) {
                return Err("`font_scale` doit être compris entre 0.75 et 1.6.".to_string());
            }
        }
        for page in &self.hide_pages {
            if !valid_page_key(page) {
                return Err(format!("Page inconnue dans `hide_pages` : `{page}`."));
            }
        }
        for entry in &self.dock {
            if entry.label.trim().is_empty() {
                return Err("Une entrée du dock n'a pas de libellé.".to_string());
            }
            if let Some(page) = &entry.page {
                if !valid_page_key(page) {
                    return Err(format!("Page inconnue dans le dock : `{page}`."));
                }
            }
            if let Some(url) = &entry.url {
                if !(url.starts_with("http://") || url.starts_with("https://")) {
                    return Err(format!(
                        "Le lien du dock doit commencer par http:// ou https:// : `{url}`."
                    ));
                }
            }
            if entry.page.is_none() && entry.url.is_none() {
                return Err(format!(
                    "Le raccourci `{}` doit viser une page ou un lien.",
                    entry.label
                ));
            }
            if let Some(color) = &entry.color {
                if !color.trim().is_empty() && parse_color(color).is_none() {
                    return Err(format!("Couleur de dock invalide : `{color}`."));
                }
            }
        }
        Ok(())
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
    pub layout: ThemeLayout,
    pub colors: Palette,
    pub rounding: f32,
    pub font_scale: f32,
    pub home_label: String,
    pub home_subtitle: String,
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
            layout: manifest.layout(),
            colors: manifest.palette(),
            rounding: manifest.rounding,
            font_scale: manifest.font_scale,
            home_label: manifest.home_label.clone(),
            home_subtitle: manifest.home_subtitle.clone(),
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
            themes: vec![
                Theme {
                    manifest: ThemeManifest::classic(),
                    builtin: true,
                    path: None,
                },
                Theme {
                    manifest: ThemeManifest::nx(),
                    builtin: true,
                    path: None,
                },
            ],
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
            if self.themes.iter().any(|theme| theme.manifest.id == manifest.id) {
                errors.push(format!(
                    "{} : l'identifiant `{}` est déjà utilisé",
                    path.display(),
                    manifest.id
                ));
                continue;
            }
            self.themes.push(Theme {
                manifest,
                builtin: false,
                path: Some(path),
            });
        }
        self.error = (!errors.is_empty()).then(|| errors.join("\n"));
    }

    pub fn reload(&mut self) {
        self.themes.retain(|theme| theme.builtin);
        self.reload_user();
    }

    pub fn active(&self, id: &str) -> &Theme {
        let id = id.trim();
        let id = if CLASSIC_ALIASES.iter().any(|alias| id.eq_ignore_ascii_case(alias)) {
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
        let mut active = ActiveTheme::from_manifest(&self.active(id).manifest);
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
            if let Some(layout) = plugin
                .layout
                .as_deref()
                .and_then(ThemeLayout::from_key)
            {
                active.layout = layout;
            }
            if let Some(rounding) = plugin.rounding {
                active.rounding = rounding;
            }
            if let Some(scale) = plugin.font_scale {
                active.font_scale = scale;
            }
            if let Some(label) = plugin.home_label.as_ref().filter(|label| !label.trim().is_empty())
            {
                active.home_label = label.clone();
            }
            if let Some(subtitle) = plugin
                .home_subtitle
                .as_ref()
                .filter(|subtitle| !subtitle.trim().is_empty())
            {
                active.home_subtitle = subtitle.clone();
            }
            for page in &plugin.hide_pages {
                if !active.hides(page) {
                    active.hidden_pages.push(page.clone());
                }
            }
            for entry in &plugin.dock {
                active.dock.push(DockShortcut {
                    label: entry.label.clone(),
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

    pub fn save(&mut self, manifest: &ThemeManifest, previous: Option<&Path>) -> Result<PathBuf, String> {
        manifest.validate()?;
        if RESERVED_IDS
            .iter()
            .any(|id| manifest.id.eq_ignore_ascii_case(id))
        {
            return Err(format!(
                "`{}` est réservé au thème fourni : choisissez un autre identifiant.",
                manifest.id
            ));
        }
        if self
            .themes
            .iter()
            .any(|theme| !theme.builtin
                && theme.manifest.id == manifest.id
                && theme.path.as_deref() != previous)
        {
            return Err(format!("Un thème `{}` existe déjà.", manifest.id));
        }
        std::fs::create_dir_all(&self.dir).map_err(|error| error.to_string())?;
        let path = self.dir.join(format!("{}.toml", manifest.id));
        let body = toml::to_string_pretty(manifest).map_err(|error| error.to_string())?;
        std::fs::write(&path, format!("# Thème Lapinux\n{body}"))
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
            return Err("Thème introuvable.".to_string());
        };
        if theme.builtin {
            return Err("Un thème fourni ne peut pas être supprimé.".to_string());
        }
        let Some(path) = theme.path.clone() else {
            return Err("Thème sans fichier.".to_string());
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
                errors.push(format!(
                    "{} : l'identifiant `{}` est déjà utilisé",
                    path.display(),
                    manifest.id
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

    pub fn active(&self, ids: &[String]) -> Vec<PluginManifest> {
        self.plugins
            .iter()
            .filter(|plugin| ids.iter().any(|id| id == &plugin.manifest.id))
            .map(|plugin| plugin.manifest.clone())
            .collect()
    }

    pub fn find(&self, id: &str) -> Option<&Plugin> {
        self.plugins
            .iter()
            .find(|plugin| plugin.manifest.id.eq_ignore_ascii_case(id))
    }

    pub fn save(
        &mut self,
        manifest: &PluginManifest,
        previous: Option<&Path>,
    ) -> Result<PathBuf, String> {
        manifest.validate()?;
        if self
            .plugins
            .iter()
            .any(|plugin| {
                plugin.manifest.id == manifest.id && plugin.path.as_deref() != previous
            })
        {
            return Err(format!("Un plugin `{}` existe déjà.", manifest.id));
        }
        std::fs::create_dir_all(&self.dir).map_err(|error| error.to_string())?;
        let path = self.dir.join(format!("{}.toml", manifest.id));
        let body = toml::to_string_pretty(manifest).map_err(|error| error.to_string())?;
        std::fs::write(&path, format!("# Plugin Lapinux\n{body}"))
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
        let Some(plugin) = self.find(id) else {
            return Err("Plugin introuvable.".to_string());
        };
        let Some(path) = plugin.path.clone() else {
            return Err("Plugin sans fichier.".to_string());
        };
        std::fs::remove_file(path).map_err(|error| error.to_string())?;
        self.reload();
        Ok(())
    }
}

/// Modèle de thème proposé par le créateur. Il contient tous les champs
/// modifiables, commentés dans l'interface.
pub fn theme_template(layout: ThemeLayout) -> String {
    let mut manifest = match layout {
        ThemeLayout::Classic => ThemeManifest::classic(),
        ThemeLayout::Nx => ThemeManifest::nx(),
    };
    manifest.id = "mon-theme".to_string();
    manifest.name = "Mon thème".to_string();
    manifest.author = String::new();
    manifest.description = "Un thème créé dans Lapinux.".to_string();
    manifest.home_label = "Mes jeux".to_string();
    manifest.home_subtitle = "Choisissez un jeu et appuyez sur Entrée.".to_string();
    let mut body = String::new();
    body.push_str("# Identifiant : minuscules, chiffres et tirets uniquement.\n");
    body.push_str("# Couleurs : #RRGGBB. Disposition : \"classic\" ou \"nx\".\n");
    body.push_str(&toml::to_string_pretty(&manifest).unwrap_or_default());
    body
}

/// Modèle de plugin. Les surcharges sont commentées pour que le fichier reste
/// valide tel quel.
pub fn plugin_template() -> String {
    r##"# Plugin Lapinux : aucune commande n'est exécutée.
# Un plugin peut changer les couleurs et la disposition du thème actif,
# ajuster les formes, renommer l'accueil et ajouter des raccourcis au dock NX.
id = "mon-plugin"
name = "Mon plugin"
version = "0.1.0"
author = ""
description = "Décrivez ce que ce plugin change."

home_label = "Mon espace"
home_subtitle = "Un accueil personnalisé par mon plugin."

# Décommentez les surcharges utiles :
# accent = "#4cc9f0"
# background = "#181a1f"
# panel = "#22252b"
# card = "#2c3038"
# card_hover = "#363b45"
# card_active = "#414754"
# border = "#4a505c"
# text = "#f5f7fa"
# text_dim = "#a8afb9"
# layout = "nx"
# rounding = 12.0
# font_scale = 1.0

# Pages retirées de la barre et du dock (library, collections, downloads,
# controllers, profile, boutiques, captures) :
# hide_pages = ["downloads"]

# Raccourcis ajoutés au dock NX :
# [[dock]]
# label = "Ma boutique"
# icon = "B"
# color = "#4cc9f0"
# url = "https://example.com"
"##
    .to_string()
}

pub fn theme_toml(manifest: &ThemeManifest) -> String {
    toml::to_string_pretty(manifest).unwrap_or_default()
}

pub fn plugin_toml(manifest: &PluginManifest) -> String {
    toml::to_string_pretty(manifest).unwrap_or_default()
}

pub fn parse_theme(text: &str) -> Result<ThemeManifest, String> {
    let manifest: ThemeManifest =
        toml::from_str(text).map_err(|error| format!("TOML invalide : {error}"))?;
    manifest.validate()?;
    Ok(manifest)
}

pub fn parse_plugin(text: &str) -> Result<PluginManifest, String> {
    let manifest: PluginManifest =
        toml::from_str(text).map_err(|error| format!("TOML invalide : {error}"))?;
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

fn validate_id(id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("L'identifiant est vide.".to_string());
    }
    if !id
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_')
    {
        return Err(
            "L'identifiant doit contenir seulement des minuscules, chiffres, `-` ou `_`."
                .to_string(),
        );
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
        assert_eq!(active.layout, ThemeLayout::Nx);
        assert_eq!(active.colors.accent, Color32::from_rgb(255, 0, 0));
        assert_eq!(active.home_label, "Accueil test");
        assert_eq!(active.dock.len(), 1);
        assert_eq!(active.dock[0].color, Color32::from_rgb(0, 255, 0));
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
        manifest.layout = "inconnu".into();
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn templates_parse_cleanly() {
        assert!(parse_theme(&theme_template(ThemeLayout::Nx)).is_ok());
        assert!(parse_plugin(&plugin_template()).is_ok());
    }

    #[test]
    fn old_theme_names_fall_back_to_classic() {
        let library = ThemeLibrary::load("/tmp/lapinux-theme-test-missing");
        assert_eq!(library.active("sombre").manifest.id, CLASSIC_ID);
        assert_eq!(library.active("").manifest.id, CLASSIC_ID);
        assert_eq!(library.active("nx").manifest.id, NX_ID);
    }
}
