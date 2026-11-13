//! Configurations de jeux : correctifs, traductions et mods déclaratifs,
//! publiés dans le dépôt `lapinux-configurations` de Pissenlit Vert.
//!
//! Une configuration ne contient aucun script : Lapinux applique des réglages
//! au jeu (variables d'environnement, valeurs DXVK, arguments) et/ou copie des
//! fichiers dans son dossier. Tout est réversible : l'installateur garde la
//! trace des fichiers posés et une copie de ceux qu'il remplace.
//!
//! Le catalogue publié ne porte que la description de ces actions ; la copie
//! compilée dans le binaire garantit qu'un dépôt injoignable ne prive pas d'un
//! correctif déjà connu.

use crate::model::{DxvkOverride, Game, GameSettings};
use crate::stores::Store;
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

/// Catalogue publié par Pissenlit Vert.
pub const CATALOG_URL: &str =
    "https://raw.githubusercontent.com/PissenlitVert/lapinux-configurations/main/catalog.json";

/// Dépôt qui porte le catalogue et sa documentation.
pub const REPOSITORY_URL: &str = "https://github.com/PissenlitVert/lapinux-configurations";

/// Adresse du catalogue, remplaçable par `LAPINUX_CONFIGURATIONS_CATALOG`
/// (fork, miroir ou fichier servi localement pour tester).
pub fn catalog_url() -> String {
    crate::env_var("CONFIGURATIONS_CATALOG")
        .map(|url| url.trim().to_string())
        .filter(|url| !url.is_empty())
        .unwrap_or_else(|| CATALOG_URL.to_string())
}

/// Dossier d'où partent les fichiers d'un catalogue : tout ce qui précède le
/// nom du fichier JSON.
pub fn catalog_base(url: &str) -> String {
    url.rsplit_once('/')
        .map(|(base, _)| base.to_string())
        .unwrap_or_default()
}

/// Auteur d'une configuration, ou curateur du catalogue.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Author {
    pub name: String,
    /// Compte GitHub, sans l'arobase.
    pub github: String,
}

impl Author {
    pub fn profile(&self) -> Option<String> {
        let login = self.github.trim().trim_start_matches('@');
        (!login.is_empty()).then(|| format!("https://github.com/{login}"))
    }
}

/// Conditions du test qui a validé une configuration : ce qui permet de
/// savoir si elle s'applique encore après un changement de Proton.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Tested {
    pub proton: String,
    pub gpu: String,
    pub ram: String,
    pub distro: String,
    /// Date du test, `AAAA-MM-JJ`.
    pub date: String,
}

/// Jeu visé : noms lisibles et identifiants de boutique.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct GameRef {
    pub names: Vec<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub steam_appids: Vec<u32>,
    /// `epic:<appName>`, `gog:<id>`, `itch:<slug>`.
    #[serde(default)]
    pub store_ids: Vec<String>,
}

impl GameRef {
    /// Vrai quand cette description désigne le jeu. L'identifiant Steam ne
    /// vaut que pour Steam : ailleurs, `appid` est un identifiant interne à
    /// Lapinux et ne doit pas se confondre avec celui d'un jeu Steam.
    pub fn matches(&self, game: &Game) -> bool {
        if game.store == Store::Steam && self.steam_appids.iter().any(|id| *id == game.appid) {
            return true;
        }
        if !game.store_id.trim().is_empty() {
            let wanted = format!("{}:{}", store_key(game.store), game.store_id.trim());
            if self
                .store_ids
                .iter()
                .any(|id| id.trim().eq_ignore_ascii_case(&wanted))
            {
                return true;
            }
        }
        let game_name = normalized(&game.name);
        if game_name.is_empty() {
            return false;
        }
        self.names
            .iter()
            .chain(self.aliases.iter())
            .any(|name| normalized(name) == game_name)
    }
}

fn store_key(store: Store) -> &'static str {
    match store {
        Store::Steam => "steam",
        Store::Epic => "epic",
        Store::Gog => "gog",
        Store::Itch => "itch",
    }
}

/// Réglages appliqués au jeu par une configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SettingsPatch {
    /// Variables d'environnement, `(nom, valeur)`.
    pub env: Vec<(String, String)>,
    pub dxvk: DxvkOverride,
    /// Arguments ajoutés après l'exécutable.
    pub args: Vec<String>,
}

impl SettingsPatch {
    pub fn is_empty(&self) -> bool {
        self.env.is_empty() && self.dxvk.is_empty() && self.args.is_empty()
    }

    /// Pose les réglages et rend de quoi les retirer. Les valeurs précédentes
    /// de chaque variable touchée sont gardées : le retrait remet exactement
    /// l'état d'avant.
    pub fn apply(&self, settings: &mut GameSettings) -> SettingsUndo {
        let mut undo = SettingsUndo::default();
        for (key, value) in &self.env {
            let previous = settings
                .env
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.clone());
            undo.env.push(EnvUndo {
                key: key.clone(),
                previous,
                applied: value.clone(),
            });
            match settings.env.iter_mut().find(|(name, _)| name == key) {
                Some(entry) => entry.1 = value.clone(),
                None => settings.env.push((key.clone(), value.clone())),
            }
        }
        if !self.dxvk.is_empty() {
            undo.dxvk_before = settings.dxvk.clone();
            undo.dxvk_shared = self.dxvk.shared_mb.is_some();
            undo.dxvk_device_mb = self.dxvk.device_mb.is_some();
            undo.dxvk_device = self.dxvk.device.is_some();
            if let Some(mb) = self.dxvk.shared_mb {
                settings.dxvk.shared_mb = Some(mb);
            }
            if let Some(mb) = self.dxvk.device_mb {
                settings.dxvk.device_mb = Some(mb);
            }
            if let Some(name) = self.dxvk.device.clone() {
                settings.dxvk.device = Some(name);
            }
            undo.dxvk_after = settings.dxvk.clone();
        }
        if !self.args.is_empty() {
            undo.args_patched = true;
            undo.args_before = settings.args.clone();
            for arg in &self.args {
                if !settings.args.contains(arg) {
                    settings.args.push(arg.clone());
                }
            }
            undo.args_after = settings.args.clone();
        }
        undo
    }
}

/// Retire des réglages l'effet d'une configuration. Une valeur modifiée depuis
/// n'est pas écrasée : elle appartient à l'utilisateur.
pub fn undo_settings(settings: &mut GameSettings, undo: &SettingsUndo) {
    for entry in &undo.env {
        let current = settings
            .env
            .iter()
            .find(|(name, _)| name == &entry.key)
            .map(|(_, value)| value.clone());
        if current.as_deref() != Some(entry.applied.as_str()) {
            continue;
        }
        match &entry.previous {
            Some(value) => {
                if let Some(found) = settings.env.iter_mut().find(|(name, _)| name == &entry.key) {
                    found.1 = value.clone();
                }
            }
            None => settings.env.retain(|(name, _)| name != &entry.key),
        }
    }
    if undo.dxvk_shared && settings.dxvk.shared_mb == undo.dxvk_after.shared_mb {
        settings.dxvk.shared_mb = undo.dxvk_before.shared_mb;
    }
    if undo.dxvk_device_mb && settings.dxvk.device_mb == undo.dxvk_after.device_mb {
        settings.dxvk.device_mb = undo.dxvk_before.device_mb;
    }
    if undo.dxvk_device && settings.dxvk.device == undo.dxvk_after.device {
        settings.dxvk.device = undo.dxvk_before.device.clone();
    }
    if undo.args_patched && settings.args == undo.args_after {
        settings.args = undo.args_before.clone();
    }
}

/// Une variable d'environnement posée, avec ce qu'il y avait avant.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct EnvUndo {
    pub key: String,
    /// `None` : la variable n'existait pas.
    pub previous: Option<String>,
    /// Valeur posée par la configuration.
    pub applied: String,
}

/// Ce qu'il faut pour revenir en arrière après une application de réglages.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SettingsUndo {
    pub env: Vec<EnvUndo>,
    pub dxvk_before: DxvkOverride,
    pub dxvk_after: DxvkOverride,
    pub dxvk_shared: bool,
    pub dxvk_device_mb: bool,
    pub dxvk_device: bool,
    pub args_patched: bool,
    pub args_before: Vec<String>,
    pub args_after: Vec<String>,
}

/// Un fichier à poser dans le dossier du jeu.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct FileSpec {
    /// Chemin dans le dépôt, relatif au dossier de la configuration.
    pub source: String,
    /// Chemin dans le jeu, relatif à son dossier d'installation.
    pub target: String,
    /// Empreinte SHA-256 du fichier, en hexadécimal.
    pub sha256: String,
    /// Copie l'ancien fichier avant de le remplacer (vrai par défaut).
    pub backup: bool,
}

/// Actions déclaratives d'une configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Actions {
    pub settings: SettingsPatch,
    pub files: Vec<FileSpec>,
}

impl Actions {
    pub fn has_work(&self) -> bool {
        !self.settings.is_empty() || !self.files.is_empty()
    }
}

/// Une configuration publiée.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Configuration {
    /// Identifiant stable : une correction publiée remplace l'ancienne.
    pub id: String,
    pub title: String,
    /// Description du problème constaté.
    pub description: String,
    /// Ce que la configuration change, en une ou deux phrases.
    pub solution: String,
    /// `graphics`, `performance`, `display`, `language`, `compatibility`,
    /// `input`, `content`.
    #[serde(rename = "type")]
    pub kind: String,
    /// `windows`, `linux` ou `both`.
    pub platform: String,
    /// `dxvk`, `vkd3d`, `wine`, `proton`, `gamescope`, `game`.
    pub layers: Vec<String>,
    pub author: Author,
    pub version: String,
    pub license: String,
    /// `proposed`, `verified` ou `broken`.
    pub status: String,
    /// Langue ajoutée ou corrigée (`fr`, `de`…), pour une traduction.
    pub language: Option<String>,
    pub tested: Tested,
    pub game: GameRef,
    /// Autres configurations dont celle-ci a besoin.
    pub requires: Vec<String>,
    pub conflicts: Vec<String>,
    /// Entrée retirée du catalogue : plus proposée, mais toujours
    /// désinstallable par ceux qui l'ont posée.
    pub retired: bool,
    pub actions: Actions,
    pub links: Vec<String>,
    /// Poids total annoncé, en octets (0 si inconnu).
    pub size: u64,
    /// Dossier de la configuration dans le dépôt, ajouté par l'assembleur du
    /// catalogue. Vide dans un manifeste source.
    pub base: String,
}

impl Configuration {
    /// Vrai quand la configuration désigne ce jeu.
    pub fn applies_to(&self, game: &Game) -> bool {
        self.game.matches(game)
    }

    /// Vrai quand elle est encore proposée.
    pub fn available(&self) -> bool {
        !self.retired
    }

    /// Vérifie ce qui peut l'être sans réseau ni disque. Rend la liste des
    /// problèmes, vide quand tout va bien.
    pub fn validate(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if !is_slug(&self.id) {
            problems.push(
                "id : minuscules, chiffres et tirets uniquement (ex. bento-blocks-shared-memory)"
                    .to_string(),
            );
        }
        if self.title.trim().is_empty() {
            problems.push("title : obligatoire".into());
        }
        if self.description.trim().is_empty() {
            problems.push("description : le problème constaté est obligatoire".into());
        }
        if self.kind.trim().is_empty() {
            problems.push("type : obligatoire".into());
        } else if !KNOWN_TYPES.contains(&self.kind.as_str()) {
            problems.push(format!(
                "type : « {} » inconnu (attendu : {})",
                self.kind,
                KNOWN_TYPES.join(", ")
            ));
        }
        if !KNOWN_PLATFORMS.contains(&self.platform.as_str()) {
            problems.push(format!(
                "platform : « {} » inconnu (attendu : {})",
                self.platform,
                KNOWN_PLATFORMS.join(", ")
            ));
        }
        for layer in &self.layers {
            if !KNOWN_LAYERS.contains(&layer.as_str()) {
                problems.push(format!("layers : « {layer} » inconnu"));
            }
        }
        if self.author.name.trim().is_empty() {
            problems.push("author.name : obligatoire".into());
        }
        if self.version.trim().is_empty() {
            problems.push("version : obligatoire".into());
        }
        if self.license.trim().is_empty() {
            problems.push("license : obligatoire (sans quoi rien ne peut être publié)".into());
        }
        if !self.status.is_empty() && !KNOWN_STATUS.contains(&self.status.as_str()) {
            problems.push(format!("status : « {} » inconnu", self.status));
        }
        if self.game.names.iter().all(|name| name.trim().is_empty()) {
            problems.push("game.names : au moins un nom est obligatoire".into());
        }
        // Une entrée retirée peut être une simple pierre tombale : elle sert à
        // retirer du catalogue une configuration publiée qui ne doit plus
        // apparaître, sans rien appliquer.
        if !self.retired && !self.actions.has_work() {
            problems.push("actions : rien à appliquer (settings ou files)".into());
        }
        let mut targets = std::collections::HashSet::new();
        for (index, file) in self.actions.files.iter().enumerate() {
            // Windows traite les noms de DLL sans distinguer la casse.
            if !targets.insert(file.target.to_ascii_lowercase()) {
                problems.push(format!("files[{index}].target : cible dupliquée"));
            }
            if let Err(err) = relative_path(&file.source) {
                problems.push(format!("files[{index}].source : {err}"));
            }
            if let Err(err) = relative_path(&file.target) {
                problems.push(format!("files[{index}].target : {err}"));
            }
            if !is_sha256(&file.sha256) {
                problems.push(format!(
                    "files[{index}].sha256 : 64 caractères hexadécimaux attendus"
                ));
            }
        }
        if self.requires.contains(&self.id) || self.conflicts.contains(&self.id) {
            problems.push(
                "requires/conflicts : une configuration ne se référence pas elle-même".into(),
            );
        }
        problems
    }

    /// Poids total des fichiers, quand le catalogue ne l'annonce pas.
    pub fn declares_files(&self) -> bool {
        !self.actions.files.is_empty()
    }
}

pub const KNOWN_TYPES: [&str; 7] = [
    "graphics",
    "performance",
    "display",
    "language",
    "compatibility",
    "input",
    "content",
];
pub const KNOWN_PLATFORMS: [&str; 3] = ["windows", "linux", "both"];
pub const KNOWN_LAYERS: [&str; 6] = ["dxvk", "vkd3d", "wine", "proton", "gamescope", "game"];
pub const KNOWN_STATUS: [&str; 3] = ["proposed", "verified", "broken"];

fn is_slug(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !text.starts_with('-')
        && !text.ends_with('-')
}

fn is_sha256(text: &str) -> bool {
    text.len() == 64 && text.chars().all(|c| c.is_ascii_hexdigit())
}

/// Chemin relatif sûr : pas d'absolu, pas de `..`, pas de séparateur Windows
/// (Wine interpréterait `..\\..\\` là où Linux voit un simple nom de fichier).
pub fn relative_path(text: &str) -> Result<PathBuf, String> {
    let raw = text.trim();
    if raw.is_empty() {
        return Err("chemin vide".into());
    }
    if raw.starts_with('/') {
        return Err("chemin absolu refusé".into());
    }
    if raw.contains('\\') {
        return Err("séparateurs Windows refusés".into());
    }
    if raw.contains('\0') {
        return Err("caractère nul refusé".into());
    }
    let mut out = PathBuf::new();
    for component in Path::new(raw).components() {
        match component {
            Component::Normal(name) => {
                let name = name
                    .to_str()
                    .ok_or_else(|| "nom de fichier illisible".to_string())?;
                if name.is_empty() {
                    continue;
                }
                if name.contains(':') {
                    return Err("deux-points refusé (lecteur Windows)".into());
                }
                out.push(name);
            }
            _ => return Err("chemin relatif attendu, sans « .. »".into()),
        }
    }
    if out.as_os_str().is_empty() {
        return Err("chemin vide".into());
    }
    Ok(out)
}

/// Chemin de téléchargement d'un fichier, sur le dépôt du catalogue.
pub fn file_url(base: &str, configuration: &Configuration, file: &FileSpec) -> Option<String> {
    if base.trim().is_empty() || configuration.base.trim().is_empty() {
        return None;
    }
    let source = relative_path(&file.source).ok()?;
    let mut path = String::new();
    for component in configuration
        .base
        .split('/')
        .chain(source.components().filter_map(|component| match component {
            Component::Normal(name) => name.to_str(),
            _ => None,
        }))
    {
        let component = component.trim();
        if component.is_empty() {
            continue;
        }
        if !path.is_empty() {
            path.push('/');
        }
        path.push_str(&percent_encode(component));
    }
    Some(format!("{}/{path}", base.trim_end_matches('/')))
}

/// Échappe ce qui ne peut pas vivre tel quel dans une URL de chemin.
fn percent_encode(segment: &str) -> String {
    let mut out = String::new();
    for byte in segment.bytes() {
        let plain = byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~');
        if plain {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Thème déclaratif proposé par le catalogue : un manifeste TOML et son
/// empreinte SHA-256. Aucun code n'est exécuté ; l'interface lit le fichier et
/// valide le thème comme un thème créé à la main.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ThemeEntry {
    pub id: String,
    pub title: String,
    pub description: String,
    pub author: Author,
    pub version: String,
    pub license: String,
    /// `proposed`, `verified`, `experimental` ou `broken`.
    pub status: String,
    /// Chemin du manifeste TOML, relatif à la racine du catalogue.
    pub file: String,
    pub sha256: String,
    pub size: u64,
}

impl ThemeEntry {
    pub fn available(&self) -> bool {
        self.status != "broken"
    }

    pub fn validate(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if self.id.is_empty()
            || !self
                .id
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_')
        {
            problems.push("identifiant de thème invalide".to_string());
        }
        if self.title.trim().is_empty() {
            problems.push("thème sans titre".to_string());
        }
        if relative_path(&self.file).is_err() {
            problems.push("fichier de thème invalide".to_string());
        }
        if !is_sha256(&self.sha256) {
            problems.push("empreinte de thème invalide".to_string());
        }
        if self.size > 256 * 1024 {
            problems.push("thème trop gros".to_string());
        }
        problems
    }
}

/// Adresse du manifeste d'un thème dans le catalogue.
pub fn theme_url(base: &str, entry: &ThemeEntry) -> Option<String> {
    let path = relative_path(&entry.file).ok()?;
    Some(format!(
        "{}/{}",
        base.trim_end_matches('/'),
        path.to_string_lossy().replace('\\', "/")
    ))
}

/// Valide un manifeste de thème reçu du réseau sans dépendre de l'interface :
/// identifiant attendu, disposition connue et couleurs au format attendu.
pub fn validate_theme_manifest(text: &str, expected_id: &str) -> Result<(), String> {
    let value: toml::Value = toml::from_str(text).map_err(|error| format!("thème illisible : {error}"))?;
    let id = value
        .get("id")
        .and_then(|id| id.as_str())
        .unwrap_or_default();
    if !id.eq_ignore_ascii_case(expected_id) {
        return Err(format!("thème {expected_id} : identifiant « {id} » inattendu"));
    }
    let layout = value
        .get("layout")
        .and_then(|layout| layout.as_str())
        .unwrap_or("classic")
        .trim()
        .to_ascii_lowercase();
    if !matches!(layout.as_str(), "classic" | "default" | "nx" | "switch" | "console") {
        return Err(format!("thème {expected_id} : disposition inconnue « {layout} »"));
    }
    for key in [
        "accent", "background", "panel", "card", "card_hover", "card_active", "border",
        "text", "text_dim",
    ] {
        if let Some(color) = value.get(key).and_then(|color| color.as_str()) {
            if !valid_color(color) {
                return Err(format!("thème {expected_id} : couleur {key} invalide"));
            }
        }
    }
    Ok(())
}

fn valid_color(value: &str) -> bool {
    let value = value.trim();
    let Some(hex) = value.strip_prefix('#') else {
        return false;
    };
    (hex.len() == 6 || hex.len() == 8) && hex.chars().all(|ch| ch.is_ascii_hexdigit())
}

/// Thèmes du catalogue, ceux déjà installés et l'état de la dernière lecture.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ThemeCatalogSet {
    pub themes: Vec<ThemeEntry>,
    /// Identifiants des fichiers `themes/*.toml` déjà présents.
    pub installed: Vec<String>,
    pub remote: bool,
    pub fetched: i64,
    pub catalog_url: String,
}

/// Le catalogue publié : version 2, plus les correctifs de la version 1.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Catalog {
    pub version: u32,
    pub maintainer: Author,
    #[serde(default)]
    pub configurations: Vec<Configuration>,
    /// Thèmes proposés : ils vivent dans le même catalogue que les correctifs
    /// de jeux, mais s'installent dans `themes/` et ne touchent aucun jeu.
    #[serde(default)]
    pub themes: Vec<ThemeEntry>,
    /// Ancien format : des correctifs de réglages seulement.
    #[serde(default)]
    pub fixes: Vec<LegacyFix>,
}

impl Catalog {
    /// Toutes les entrées, ancien format converti compris.
    pub fn entries(&self) -> Vec<Configuration> {
        let mut entries = self.configurations.clone();
        for fix in &self.fixes {
            entries.push(fix.to_configuration(&self.maintainer));
        }
        entries
    }

    /// Le catalogue complet, relu et nettoyé.
    pub fn normalized(&self) -> Catalog {
        Catalog {
            version: self.version.max(2),
            maintainer: self.maintainer.clone(),
            configurations: self.entries(),
            themes: self
                .themes
                .iter()
                .filter(|theme| theme.validate().is_empty())
                .cloned()
                .collect(),
            fixes: Vec::new(),
        }
    }
}

/// Entrée du format v1 (`fixes`) : uniquement des réglages.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct LegacyFix {
    pub id: String,
    pub title: String,
    pub name: String,
    pub aliases: Vec<String>,
    pub appids: Vec<u32>,
    pub note: String,
    pub url: String,
    pub dxvk: DxvkOverride,
    pub env: Vec<(String, String)>,
}

impl LegacyFix {
    fn to_configuration(&self, maintainer: &Author) -> Configuration {
        Configuration {
            id: self.id.clone(),
            title: self.title.clone(),
            description: self.note.clone(),
            solution: String::new(),
            kind: "graphics".into(),
            platform: "windows".into(),
            layers: vec!["dxvk".into()],
            author: maintainer.clone(),
            version: "1.0.0".into(),
            license: "CC0-1.0".into(),
            status: "verified".into(),
            language: None,
            tested: Tested::default(),
            game: GameRef {
                names: vec![self.name.clone()],
                aliases: self.aliases.clone(),
                steam_appids: self.appids.clone(),
                store_ids: Vec::new(),
            },
            requires: Vec::new(),
            conflicts: Vec::new(),
            retired: false,
            actions: Actions {
                settings: SettingsPatch {
                    env: self.env.clone(),
                    dxvk: self.dxvk.clone(),
                    args: Vec::new(),
                },
                files: Vec::new(),
            },
            links: (!self.url.trim().is_empty())
                .then(|| vec![self.url.clone()])
                .unwrap_or_default(),
            size: 0,
            base: String::new(),
        }
    }
}

/// Une configuration posée sur un jeu : tout ce qu'il faut pour la retirer.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct InstalledConfiguration {
    pub id: String,
    pub title: String,
    pub version: String,
    pub installed_at: i64,
    pub files: Vec<InstalledFile>,
    pub undo: SettingsUndo,
    /// Adresse du catalogue qui l'a fournie.
    pub source: String,
}

/// Un fichier posé par une configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct InstalledFile {
    pub target: String,
    pub sha256: String,
    /// Une copie de l'ancien fichier attend dans les sauvegardes.
    pub backed_up: bool,
}

/// Installation d'un injecteur, suivie indépendamment des configurations du catalogue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphicsInjector {
    pub kind: String,
    pub version: String,
    pub executable: String,
    pub proxy: String,
    pub stage: String,
    pub files: Vec<GraphicsInjectorFile>,
    pub staged: Vec<GraphicsInjectorFile>,
    pub ready: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphicsInjectorFile {
    pub name: String,
    pub sha256: String,
}

/// Diagnostic d'un injecteur, calculé sur les fichiers réels et les réglages
/// du jeu. Aucun champ n'est persisté : il décrit l'instant de la demande.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct GraphicsInjectorStatus {
    pub kind: String,
    pub version: String,
    pub executable: String,
    pub proxy: String,
    /// `ok`, `missing`, `modified`, `incomplete`, `executable_changed`.
    pub install: String,
    /// État de chaque fichier suivi.
    pub files: Vec<GraphicsInjectorFileStatus>,
    /// `applied`, `already`, `blocked`.
    pub launch_override: String,
    /// Cause d'un override bloqué : `wined3d`, `frame_generation`, `vkbasalt`,
    /// `config_file`, `manual_override`, `optiscaler_proton`.
    pub override_reason: String,
    /// Action utile en un clic : `none`, `repair`, `reinstall`, `fix_override`.
    pub action: String,
    /// Dossiers de sauvegarde conservés et réellement présents.
    pub backups: Vec<GraphicsInjectorBackup>,
}

/// Dossier de sauvegarde conservé, proposé à l'ouverture dans la fiche.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct GraphicsInjectorBackup {
    /// `user_modified` ou `stage`.
    pub kind: String,
    pub path: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct GraphicsInjectorFileStatus {
    pub name: String,
    /// `intact`, `missing`, `modified`, `unlinked`, `personalized`.
    pub state: String,
}

/// Ce que le daemon rend pour la fiche d'un jeu.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ConfigurationSet {
    /// Configurations du catalogue qui désignent ce jeu, retirées comprises.
    pub configurations: Vec<Configuration>,
    /// Celles déjà posées sur le jeu.
    pub installed: Vec<InstalledConfiguration>,
    pub maintainer: Author,
    /// Vrai quand le catalogue vient du réseau.
    pub remote: bool,
    /// Date de la dernière lecture réussie, en secondes Unix.
    pub fetched: i64,
    pub catalog_url: String,
}

/// Résultat d'une installation ou d'un retrait.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct InstallReport {
    pub id: String,
    pub title: String,
    pub version: String,
    /// Fichiers posés ou retirés.
    pub files: usize,
    pub bytes: u64,
    /// Ce qui n'a pas pu être fait, sans bloquer le reste.
    pub warnings: Vec<String>,
}

/// La copie compilée du catalogue : les configurations publiées restent
/// connues hors ligne, sans dépendre du réseau.
pub fn bundled() -> Catalog {
    serde_json::from_str::<Catalog>(include_str!("../../../configurations/catalog.json"))
        .map(|catalog| catalog.normalized())
        .unwrap_or_default()
}

/// Associe le catalogue distant à la copie compilée : une correction publiée
/// remplace l'entrée locale du même identifiant, les entrées absentes du
/// réseau restent connues.
pub fn merged(remote: Catalog) -> Catalog {
    let remote = remote.normalized();
    let mut catalog = remote.clone();
    for local in bundled().configurations {
        match catalog
            .configurations
            .iter()
            .position(|entry| entry.id == local.id)
        {
            // Une pierre tombale locale retire l'entrée du catalogue distant :
            // c'est le seul cas où la copie compilée l'emporte sur le réseau.
            Some(position) if local.retired => catalog.configurations[position] = local,
            Some(_) => {}
            None => catalog.configurations.push(local),
        }
    }
    for local in bundled().themes {
        if !catalog.themes.iter().any(|entry| entry.id == local.id) {
            catalog.themes.push(local);
        }
    }
    if catalog.maintainer.name.trim().is_empty() {
        catalog.maintainer = bundled().maintainer;
    }
    catalog
}

/// Nom comparable : minuscules, sans ponctuation ni espaces.
fn normalized(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bento() -> Configuration {
        Configuration {
            id: "bento-blocks-shared-memory".into(),
            title: "Bento Blocks — dedicated GPU mistaken for integrated graphics".into(),
            description: "The engine reads shared memory as an integrated GPU.".into(),
            kind: "graphics".into(),
            platform: "windows".into(),
            layers: vec!["dxvk".into()],
            author: Author {
                name: "Pissenlit Vert".into(),
                github: "PissenlitVert".into(),
            },
            version: "1.0.0".into(),
            license: "CC0-1.0".into(),
            status: "verified".into(),
            game: GameRef {
                names: vec!["Bento Blocks".into()],
                aliases: vec!["BentoBlocks".into()],
                steam_appids: vec![3311670],
                store_ids: Vec::new(),
            },
            actions: Actions {
                settings: SettingsPatch {
                    dxvk: DxvkOverride {
                        shared_mb: Some(1024),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                files: Vec::new(),
            },
            ..Default::default()
        }
    }

    fn game(appid: u32, store: Store, name: &str) -> Game {
        Game {
            appid,
            store,
            name: name.into(),
            ..Default::default()
        }
    }

    #[test]
    fn la_configuration_suit_le_nom_et_l_identifiant_steam() {
        assert!(bento().applies_to(&game(3311670, Store::Steam, "Bento Blocks")));
        assert!(bento().applies_to(&game(90, Store::Epic, "BENTO  BLOCKS")));
        assert!(!bento().applies_to(&game(90, Store::Epic, "Bento Block Party")));
    }

    #[test]
    fn l_identifiant_steam_ne_vaut_que_pour_steam() {
        let configuration = Configuration {
            game: GameRef {
                names: vec!["Un autre jeu".into()],
                steam_appids: vec![3311670],
                ..Default::default()
            },
            ..bento()
        };
        assert!(configuration.applies_to(&game(3311670, Store::Steam, "Peu importe")));
        assert!(!configuration.applies_to(&game(3311670, Store::Gog, "Peu importe")));
    }

    #[test]
    fn les_identifiants_de_boutique_sont_reconnus() {
        // Les noms diffèrent : seul l'identifiant de boutique peut décider.
        let configuration = Configuration {
            game: GameRef {
                names: vec!["Catalog Name".into()],
                store_ids: vec!["epic:7349a1".into()],
                ..Default::default()
            },
            ..bento()
        };
        let mut epic = game(12, Store::Epic, "Installed Name");
        epic.store_id = "7349a1".into();
        assert!(configuration.applies_to(&epic));
        epic.store_id = "autre".into();
        assert!(!configuration.applies_to(&epic));
    }

    #[test]
    fn un_manifeste_valide_ne_signale_rien() {
        assert!(bento().validate().is_empty(), "{:?}", bento().validate());
    }

    #[test]
    fn les_chemins_dangereux_sont_refuses() {
        for bad in ["../x", "/etc/passwd", "a/../../b", "C:\\x", "a\\b", "a:b"] {
            let mut configuration = bento();
            configuration.actions.files.push(FileSpec {
                source: "files/fr.pak".into(),
                target: bad.into(),
                sha256: "a".repeat(64),
                backup: true,
            });
            assert!(
                !configuration.validate().is_empty(),
                "{bad} aurait dû être refusé"
            );
        }
    }

    #[test]
    fn les_empreintes_sont_verifiees() {
        let mut configuration = bento();
        configuration.actions.files.push(FileSpec {
            source: "files/fr.pak".into(),
            target: "data/lang/fr.pak".into(),
            sha256: "trop court".into(),
            backup: true,
        });
        assert!(configuration
            .validate()
            .iter()
            .any(|problem| problem.contains("sha256")));
    }

    #[test]
    fn appliquer_puis_retirer_les_reglages() {
        let mut settings = GameSettings {
            env: vec![("WINEDEBUG".into(), "-all".into())],
            args: vec!["-windowed".into()],
            ..Default::default()
        };
        let patch = SettingsPatch {
            env: vec![("DXVK_HUD".into(), "1".into())],
            dxvk: DxvkOverride {
                shared_mb: Some(1024),
                ..Default::default()
            },
            args: vec!["-novid".into()],
        };
        let undo = patch.apply(&mut settings);
        assert_eq!(settings.env.len(), 2);
        assert_eq!(settings.dxvk.shared_mb, Some(1024));
        assert_eq!(settings.args.len(), 2);
        undo_settings(&mut settings, &undo);
        assert_eq!(settings.env.len(), 1);
        assert_eq!(settings.env[0].0, "WINEDEBUG");
        assert_eq!(settings.dxvk.shared_mb, None);
        assert_eq!(settings.args, vec!["-windowed"]);
    }

    #[test]
    fn une_valeur_modifiee_depuis_n_est_pas_ecrasee_au_retrait() {
        let mut settings = GameSettings::default();
        let patch = SettingsPatch {
            dxvk: DxvkOverride {
                shared_mb: Some(1024),
                ..Default::default()
            },
            ..Default::default()
        };
        let undo = patch.apply(&mut settings);
        settings.dxvk.shared_mb = Some(2048);
        undo_settings(&mut settings, &undo);
        assert_eq!(settings.dxvk.shared_mb, Some(2048));
    }

    #[test]
    fn l_ancien_format_est_converti() {
        let catalog = Catalog {
            maintainer: Author {
                name: "Pissenlit Vert".into(),
                github: "PissenlitVert".into(),
            },
            fixes: vec![LegacyFix {
                id: "bento-blocks-shared-memory".into(),
                title: "Bento Blocks".into(),
                name: "Bento Blocks".into(),
                appids: vec![3311670],
                dxvk: DxvkOverride {
                    shared_mb: Some(1024),
                    ..Default::default()
                },
                ..Default::default()
            }],
            ..Default::default()
        };
        let entries = catalog.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].kind, "graphics");
        assert_eq!(entries[0].actions.settings.dxvk.shared_mb, Some(1024));
        assert_eq!(entries[0].author.github, "PissenlitVert");
    }

    #[test]
    fn une_pierre_tombale_retire_l_entree_distante() {
        // Le dépôt distant peut encore publier une configuration retirée : la
        // copie compilée doit la masquer, sans empêcher ses mises à jour.
        let remote = Catalog {
            configurations: vec![bento()],
            ..Default::default()
        };
        let entry = merged(remote)
            .configurations
            .into_iter()
            .find(|entry| entry.id == "bento-blocks-shared-memory")
            .expect("entrée présente");
        assert!(entry.retired);
        assert_eq!(entry.status, "broken");
        assert!(!entry.available());
    }

    #[test]
    fn la_copie_compilee_est_bien_formee() {
        // Le catalogue compilé peut être vide : l'important est qu'il se lise
        // et que chaque entrée publiée passe la validation.
        for entry in bundled().configurations {
            assert!(entry.validate().is_empty(), "{:?}", entry.validate());
            assert!(
                entry.title.chars().all(|c| !"éèêàçù".contains(c)),
                "titre non anglais : {}",
                entry.title
            );
        }
    }

    #[test]
    fn l_adresse_des_fichiers_suit_le_depot() {
        let mut entry = bento();
        entry.base = "configurations/bento-blocks/shared-memory".into();
        let file = FileSpec {
            source: "files/fr.pak".into(),
            ..Default::default()
        };
        assert_eq!(
            file_url("https://example.test/main", &entry, &file).as_deref(),
            Some(
                "https://example.test/main/configurations/bento-blocks/shared-memory/files/fr.pak"
            )
        );
        // Les espaces et accents d'un nom de fichier passent en pourcentage.
        let file = FileSpec {
            source: "files/écran final.pak".into(),
            ..Default::default()
        };
        assert_eq!(
            file_url("https://example.test/main", &entry, &file).as_deref(),
            Some(
                "https://example.test/main/configurations/bento-blocks/shared-memory/files/%C3%A9cran%20final.pak"
            )
        );
        // Sans dossier de base, rien ne peut être téléchargé.
        entry.base.clear();
        assert!(file_url("https://example.test/main", &entry, &file).is_none());
    }

    fn nx_theme() -> ThemeEntry {
        ThemeEntry {
            id: "nx".into(),
            title: "NX".into(),
            description: "Accueil horizontal".into(),
            author: Author {
                name: "Pissenlit Vert".into(),
                github: "PissenlitVert".into(),
            },
            version: "0.1.0".into(),
            license: "CC0-1.0".into(),
            status: "experimental".into(),
            file: "themes/nx.toml".into(),
            sha256: "a".repeat(64),
            size: 512,
        }
    }

    #[test]
    fn un_theme_de_catalogue_se_valide_et_se_localise() {
        let theme = nx_theme();
        assert!(theme.validate().is_empty(), "{:?}", theme.validate());
        assert!(theme.available());
        assert_eq!(
            theme_url("https://example.test/main", &theme).as_deref(),
            Some("https://example.test/main/themes/nx.toml")
        );
        let mut broken = theme.clone();
        broken.id = "../nx".into();
        broken.file = "../nx.toml".into();
        broken.sha256 = "court".into();
        broken.status = "broken".into();
        assert!(!broken.validate().is_empty());
        assert!(!broken.available());
    }

    #[test]
    fn le_catalogue_lit_les_themes_et_ignore_les_invalides() {
        let mut theme = nx_theme();
        let json = serde_json::json!({ "version": 2, "themes": [theme] });
        let catalog: Catalog = serde_json::from_value(json).unwrap();
        assert_eq!(catalog.themes.len(), 1);
        assert_eq!(catalog.normalized().themes.len(), 1);
        theme.sha256 = "pas une empreinte".into();
        let json = serde_json::json!({ "version": 2, "themes": [theme] });
        let catalog: Catalog = serde_json::from_value(json).unwrap();
        assert!(catalog.normalized().themes.is_empty());
    }

    #[test]
    fn un_manifeste_de_theme_est_controle_avant_ecriture() {
        let good = r##"
            id = "nx"
            name = "NX"
            layout = "nx"
            accent = "#00C8E0"
            background = "#1C1C1E"
        "##;
        assert!(validate_theme_manifest(good, "nx").is_ok());
        assert!(validate_theme_manifest(good, "autre").is_err());
        assert!(validate_theme_manifest("id = \"nx\"\nlayout = \"mobile\"", "nx").is_err());
        assert!(
            validate_theme_manifest("id = \"nx\"\naccent = \"rouge\"", "nx").is_err()
        );
    }
}
