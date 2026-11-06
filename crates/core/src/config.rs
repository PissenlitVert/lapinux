//! Configuration utilisateur (TOML).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::captures::{self, DEFAULT_NAME_FORMAT};
use crate::paths::{expand_tilde, home_dir, LapinuxPaths};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub general: GeneralConfig,
    pub library: LibraryConfig,
    /// Collections de jeux : catégories manuelles et regroupements dynamiques.
    pub collections: CollectionsConfig,
    pub steam: SteamConfig,
    pub launch: LaunchConfig,
    pub overlay: OverlayConfig,
    pub input: InputConfig,
    pub artwork: ArtworkConfig,
    pub downloads: DownloadsConfig,
    /// Captures : photo de la fenêtre du jeu et enregistrement des parties.
    pub capture: CaptureConfig,
    /// Rich Presence Discord : ce que Lapinux publie sur le profil.
    pub discord: DiscordConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct GeneralConfig {
    /// Langue de l'interface : "fr" ou "en".
    pub lang: String,
    /// Langue des jeux (nom Steam : `french`, `schinese`…) ; vide = celle du
    /// client Steam, sinon celle du bureau. C'est elle qui choisit les dépôts
    /// de doublage téléchargés.
    pub game_language: String,
    /// Racine des bibliothèques de jeux gérées par Lapinux.
    pub library_root: String,
    /// Thème actif : `classic`, `nx` ou l'identifiant d'un thème créé dans
    /// `~/.config/lapinux/themes`. `sombre` et `clair` restent lus comme
    /// l'ancien thème classique.
    pub theme: String,
    /// Plugins actifs, par identifiant. Chaque plugin vit dans
    /// `~/.config/lapinux/plugins` et ne contient que des contributions
    /// déclaratives (couleurs, disposition, dock).
    #[serde(default)]
    pub active_plugins: Vec<String>,
    /// Ordre des sections de la barre du haut, par clé stable : `library`,
    /// `downloads`, `controllers`, `profile`, `boutiques`, `captures`. Les
    /// sections absentes sont ajoutées à la suite, dans l'ordre par défaut.
    pub topbar_order: Vec<String>,
    /// Animations de l'interface.
    pub animations: bool,
    /// Démarre directement en mode plein écran façon console.
    pub big_picture: bool,
    /// Largeur des pochettes dans la grille de la bibliothèque.
    pub grid_width: u32,
    /// Écran d'attente qui couvre la fenêtre pendant qu'un jeu démarre :
    /// `capsule` (jaquette du jeu et logo Lapinux qui glisse en diagonale),
    /// `lapin` (l'animation du lapin) ou `aucun`. Vide vaut `capsule`.
    pub launch_screen: String,
    /// Racine des copies locales des sauvegardes ; vide = données de Lapinux.
    pub saves_root: String,
    /// Ancien réglage du nombre de copies gardées : les sauvegardes vivent
    /// désormais dans trois emplacements fixes. Le champ reste lu pour ne pas
    /// casser les configurations existantes.
    pub saves_keep: u32,
    /// Copie automatique des sauvegardes à la fermeture d'un jeu.
    pub saves_auto: bool,
    /// Saute la copie automatique quand la boutique garde déjà les sauvegardes
    /// dans son cloud : la version cloud suffit, et une copie locale de plus
    /// ne ferait que doubler l'espace occupé.
    pub saves_skip_cloud: bool,
    /// Relance le jeu juste après avoir chargé un emplacement de sauvegarde :
    /// un jeu ne relit ses fichiers qu'au démarrage. Désactivé, le chargement
    /// remplace les fichiers et le jeu démarrera dessus plus tard.
    pub saves_relaunch: bool,
    /// Minutes sans que la fenêtre soit active avant que Lapinux se range dans
    /// la zone de notification. `0` désactive le rangement : la fenêtre reste
    /// ouverte. Hors partie, ce repos ne coûte alors ni processeur ni carte
    /// graphique.
    pub tray_idle_minutes: u32,
}

/// Ordre d'affichage de la bibliothèque, conservé d'un lancement à l'autre.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LibrarySort {
    #[default]
    Name,
    Playtime,
    Acquired,
}

/// Filtre d'installation de la bibliothèque.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LibraryInstallFilter {
    #[default]
    All,
    Installed,
    Missing,
}

/// Sources affichées dans la bibliothèque : la grille garde la même
/// sélection au redémarrage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LibrarySources {
    pub disk: bool,
    pub steam: bool,
    pub family: bool,
    pub epic: bool,
    pub gog: bool,
    pub itch: bool,
}

impl Default for LibrarySources {
    fn default() -> Self {
        Self {
            disk: true,
            steam: true,
            family: true,
            epic: true,
            gog: true,
            itch: true,
        }
    }
}

/// Axe de regroupement d'une collection dynamique.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollectionField {
    #[default]
    Publisher,
    Developer,
    Store,
}

impl CollectionField {
    /// Les trois axes proposés dans l'interface, dans l'ordre d'affichage.
    pub const ALL: [Self; 3] = [Self::Publisher, Self::Developer, Self::Store];
}

/// Collection de jeux : catégorie manuelle ou regroupement automatique.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct GameCollection {
    /// Identifiant stable, indépendant du nom.
    pub id: String,
    pub name: String,
    /// `None` pour une catégorie manuelle ; `Some` pour un regroupement
    /// automatique par éditeur, développeur ou boutique.
    pub field: Option<CollectionField>,
    /// Appartenance d'une collection manuelle. Ignoré pour une collection
    /// dynamique, qui recalcule ses jeux à chaque affichage.
    pub appids: Vec<u32>,
}

impl Default for GameCollection {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            field: None,
            appids: Vec::new(),
        }
    }
}

/// Collections enregistrées par l'utilisateur.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct CollectionsConfig {
    pub items: Vec<GameCollection>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct LibraryConfig {
    /// Masque les outils Steam (Proton, Steam Linux Runtime, redistribuables)
    /// dans la bibliothèque : ce ne sont pas des jeux jouables.
    pub hide_tools: bool,
    /// Affiche les démos Steam dans la bibliothèque. Les démos sont des
    /// applications distinctes de leur jeu, mais restent jouables : elles sont
    /// donc visibles par défaut.
    #[serde(default = "shown_by_default")]
    pub show_demos: bool,
    /// Masque les outils et assets de **tous** les comptes d'un coup. Ce
    /// réglage prioritaire complète les cases de chaque boutique dans
    /// Réglages › Comptes.
    #[serde(default)]
    pub hide_all_tools: bool,
    /// Outils et assets propres à chaque boutique (marketplace Epic : assets,
    /// greffons, moteurs, projets). Cachés par défaut : la grille ne garde
    /// que les jeux.
    #[serde(default = "hidden_by_default")]
    pub hide_epic_tools: bool,
    #[serde(default = "hidden_by_default")]
    pub hide_gog_tools: bool,
    #[serde(default = "hidden_by_default")]
    pub hide_itch_tools: bool,
    /// Affiche les pastilles de provenance (Steam perso, famille, disque) sur
    /// les jaquettes.
    pub show_badges: bool,
    /// Taille des pastilles de provenance, en pourcentage de leur taille
    /// nominale. Au-delà de 100 %, une petite tuile ramène l'échelle à la place
    /// réelle : deux pastilles ne débordent jamais de la jaquette.
    pub badge_scale: u32,
    /// Arrondi des jaquettes de la grille, en pixels.
    pub cover_radius: u32,
    /// Espace entre deux jaquettes, en pixels.
    pub grid_spacing: u32,
    /// Largeur de l'aperçu de survol, en pixels.
    pub hover_width: u32,
    /// Tri retenu dans la bibliothèque.
    pub sort: LibrarySort,
    /// Sources affichées : la grille rouvre avec la même sélection.
    pub sources: LibrarySources,
    /// Filtre d'installation retenu (tous, installés, à installer).
    pub filter: LibraryInstallFilter,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct DownloadsConfig {
    /// Plafond commun à tous les stores, en Mbit/s (0 = illimité).
    pub global_mbit_per_sec: u32,
    /// Plafond propre à chaque store, en Mbit/s (0 = illimité).
    pub store_mbit_per_sec: HashMap<String, u32>,
    /// Télécharge plusieurs jeux à la fois. Sinon les demandes suivantes
    /// attendent dans la file et partent une par une.
    pub parallel_installs: bool,
    /// Reprend les sauvegardes cloud juste après l'installation, avant le
    /// premier lancement. Steam n'a pas besoin d'un dossier configuré ; les
    /// boutiques demandent leur dossier de sauvegarde habituel.
    #[serde(default = "cloud_saves_after_install_default")]
    pub cloud_saves_after_install: bool,
    /// Serveur CDN Steam préféré (hôte de la liste des serveurs de contenu).
    /// Vide : Steam choisit, comme son client.
    pub steam_server: String,
    /// Famille de CDN Epic préférée : `cloudfront`, `akamai`, `fastly`, ou
    /// vide pour garder l'ordre publié par Epic.
    pub epic_cdn: String,
}

impl DownloadsConfig {
    /// Limite effective d'un store : la plus stricte entre le plafond global
    /// et celui du store. 0 des deux côtés signifie « sans limite ».
    pub fn effective_mbit(&self, store: &str) -> u32 {
        let store_limit = self
            .store_mbit_per_sec
            .get(store)
            .copied()
            .unwrap_or(0);
        match (self.global_mbit_per_sec, store_limit) {
            (0, 0) => 0,
            (0, store) => store,
            (global, 0) => global,
            (global, store) => global.min(store),
        }
    }
}

/// Valeur d'un booléen qui reste vrai quand la configuration a été écrite
/// avant son apparition : les champs ajoutés ensuite gardent le comportement
/// attendu sans réécriture du fichier.
fn default_true() -> bool {
    true
}

/// Nom de fichier par défaut des vidéos : date ISO, heure locale, jeu et
/// cadence. Les anciennes configurations gardent ce modèle sans réécriture.
fn default_video_name_format() -> String {
    DEFAULT_NAME_FORMAT.into()
}

fn default_video_fps() -> u32 {
    30
}

fn default_video_profile() -> String {
    "balanced".into()
}

fn default_video_resolution() -> String {
    "1080p".into()
}

fn default_video_encoder() -> String {
    "auto".into()
}

/// Ancrage par défaut du badge d'enregistrement : en bas à gauche, loin des
/// chiffres de performance qui occupent souvent le haut de l'écran.
fn default_record_badge_position() -> String {
    "bottom-left".into()
}

fn default_record_badge_scale() -> f32 {
    1.0
}

/// Taille de la plaque de performance native. Un peu plus grande que la
/// plaque d'origine : à 1080p, son texte restait minuscule.
fn default_hud_scale() -> f32 {
    1.15
}

/// Captures : ce que fait l'enregistreur tout seul pendant une partie.
///
/// La photo et la vidéo ne prennent que **la fenêtre du jeu**, jamais le
/// bureau : c'est le portail du bureau qui choisit la fenêtre, et il garde son
/// jeton pour reprendre la même au lancement suivant.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CaptureConfig {
    /// Démarre l'enregistrement dès que le jeu affiche sa première image.
    pub record_on_launch: bool,
    /// Arrête l'enregistrement et garde le fichier quand le jeu se ferme.
    pub stop_on_exit: bool,
    /// Cadence de la vidéo, en images par seconde. 30 i/s est le compromis
    /// qui garde le jeu fluide ; 60 i/s coûte deux fois plus de captures KWin
    /// et de données brutes à encoder.
    #[serde(default = "default_video_fps")]
    pub video_fps: u32,
    /// Effort de l'encodeur : `performance`, `balanced` ou `quality`. Le
    /// profil performance réduit le preset et la qualité ; la résolution se
    /// règle séparément avec `video_resolution`.
    #[serde(default = "default_video_profile")]
    pub video_profile: String,
    /// Hauteur maximale de la vidéo : `720p`, `1080p`, `1440p` ou `native`.
    /// Une résolution plus basse réduit le travail d'encodage et le débit.
    #[serde(default = "default_video_resolution")]
    pub video_resolution: String,
    /// Encodeur préféré : `auto` (GPU si disponible), `nvenc` ou `software`.
    #[serde(default = "default_video_encoder")]
    pub video_encoder: String,
    /// Codec demandé à l'enregistreur : `auto`, `h264`, `hevc` ou `av1`.
    pub video_codec: String,
    /// Enregistre le son du jeu, et lui seul : ni le reste du bureau, ni le
    /// micro.
    pub record_audio: bool,
    /// Ajoute le microphone au son de la vidéo. Décoché par défaut : une
    /// capture de partie n'a pas à enregistrer la pièce.
    #[serde(default)]
    pub record_microphone: bool,
    /// Affiche un HUD de performance à l'écran pendant la partie. Il vit dans
    /// la fenêtre de l'overlay, donc il n'entre jamais dans la vidéo : c'est
    /// MangoHud, qui dessine dans l'image du jeu, qui est masqué pendant
    /// l'enregistrement.
    #[serde(default = "default_true")]
    pub hud_on_screen: bool,
    /// Affiche un badge « REC » séparé de la plaque de performance : il donne
    /// le statut et le temps enregistré, à l'endroit choisi. Allumé par défaut :
    /// le statut reste visible, mais il ne fait plus partie des chiffres.
    pub record_badge: bool,
    /// Ancrage du badge : `bottom-left`, `top-center`…
    #[serde(default = "default_record_badge_position")]
    pub record_badge_position: String,
    /// Taille du badge, en multiples de sa taille de base.
    #[serde(default = "default_record_badge_scale")]
    pub record_badge_scale: f32,
    /// Taille de la plaque de performance native (texte, barres et courbe),
    /// en multiples de sa taille de base.
    #[serde(default = "default_hud_scale")]
    pub hud_scale: f32,
    /// Modèle du nom des vidéos : `{date}`, `{time}`, `{jeu}`, `{fps}`,
    /// `{appid}` et `{unix}` sont remplacés au démarrage de l'enregistrement.
    #[serde(default = "default_video_name_format")]
    pub video_name_format: String,
    /// Dossier des vidéos. Vide : le dossier XDG des vidéos, sous `Lapinux`.
    #[serde(default)]
    pub video_dir: String,
    /// Dossier des photos. Vide : le dossier XDG des images, sous `Lapinux`.
    #[serde(default)]
    pub image_dir: String,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            record_on_launch: true,
            stop_on_exit: true,
            video_fps: default_video_fps(),
            video_profile: default_video_profile(),
            video_resolution: default_video_resolution(),
            video_encoder: default_video_encoder(),
            video_codec: "auto".into(),
            record_audio: true,
            hud_on_screen: true,
            record_microphone: false,
            record_badge: true,
            record_badge_position: default_record_badge_position(),
            record_badge_scale: default_record_badge_scale(),
            hud_scale: default_hud_scale(),
            video_name_format: default_video_name_format(),
            video_dir: String::new(),
            image_dir: String::new(),
        }
    }
}

impl CaptureConfig {
    /// Dossier effectif des vidéos, dossier XDG inclus.
    pub fn videos_dir(&self) -> PathBuf {
        captures::resolve_dir(&self.video_dir, || crate::paths::videos_dir().join("Lapinux"))
    }

    /// Dossier effectif des photos de jeu.
    pub fn images_dir(&self) -> PathBuf {
        captures::resolve_dir(&self.image_dir, || crate::paths::pictures_dir().join("Lapinux"))
    }

    /// Nom sans extension de la prochaine vidéo.
    pub fn render_stem(&self, game: &str, appid: u32, fps: u32, at: i64) -> String {
        captures::render_stem(&self.video_name_format, game, appid, fps, at)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SteamConfig {
    /// Nom de compte Steam (jamais le mot de passe).
    pub username: String,
    /// Autorise le lancement des outils réseau en mode simulation.
    pub offline_tools: bool,
    /// Présence publiée sur Steam quand Lapinux est lancé : en ligne, absent,
    /// occupé, invisible ou hors ligne.
    pub presence: crate::social::SteamPresence,
    /// Garde la session sociale en vie : le compte reste visible de ses amis
    /// tant que l'application tourne, et le jeton est entretenu.
    pub social: bool,
    /// Publie le jeu Steam en cours sur le profil, sans lancer le client
    /// Steam : l'état « en jeu » apparaît chez les amis comme avec Steam.
    #[serde(default = "default_true")]
    pub show_playing: bool,
}

/// Plus haute limite de FPS proposée : le palier du dessus de l'overlay,
/// au-delà duquel il n'y a que « illimité ».
pub const MAX_FPS_CAP: u32 = 360;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct LaunchConfig {
    /// Proton par défaut ("auto" = meilleur candidat détecté).
    pub proton: String,
    /// Lance le jeu dans GameScope.
    pub gamescope: bool,
    /// Enveloppe le jeu dans `lapinux scene` : la scène garde la fenêtre de
    /// lancement et l'écran de fin dans GameScope, sans les recréer côté
    /// interface. Sans GameScope, le lancement direct reste utilisé.
    #[serde(default = "default_true")]
    pub scene: bool,
    /// Connecteur de l'écran de jeu (vide = écran principal, laissé à GameScope).
    pub output: String,
    pub width: u32,
    pub height: u32,
    /// Limite de FPS demandée à GameScope (0 = illimitée, aucun argument posé).
    pub fps_cap: u32,
    pub hdr: bool,
    pub adaptive_sync: bool,
    pub fullscreen: bool,
    /// MangoHud par défaut.
    pub mangohud: bool,
    /// Démarre le client Steam sans interface avant un jeu Steamworks.
    #[serde(default = "default_true")]
    pub steam_headless: bool,
}

impl LaunchConfig {
    /// Aligne la résolution et la limite de FPS sur le mode courant d'un
    /// écran, et retient son connecteur pour GameScope.
    pub fn apply_display(&mut self, display: &crate::display::DisplayInfo) {
        self.output = display.name.clone();
        self.width = display.width;
        self.height = display.height;
        if display.refresh_hz >= 30 {
            self.fps_cap = display.refresh_hz.min(MAX_FPS_CAP);
        }
    }

}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct OverlayConfig {
    /// Raccourci clavier global (chord evdev) : "Shift+Tab".
    pub hotkey: String,
    /// Raccourci manette : "Guide" (ou "L1+R1+Start").
    pub pad_hotkey: String,
    pub fps_max: u32,
}

/// Disposition des lettres affichées sur les quatre boutons de façade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FaceButtonProfile {
    Xbox,
    Nintendo,
}

impl Default for FaceButtonProfile {
    fn default() -> Self {
        Self::Xbox
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct InputConfig {
    pub grab_devices: bool,
    pub deadzone: f32,
    pub stick_curve: f32,
    /// Profil appliqué par défaut.
    pub default_profile: String,
    /// Disposition des lettres des boutons de façade. Xbox reste le défaut,
    /// même pour une manette Nintendo, tant que le profil Nintendo n'est pas
    /// choisi dans la page Manettes.
    pub face_profile: FaceButtonProfile,
    /// Gyro activé par défaut.
    pub gyro: bool,
    /// Nom personnalisé par manette, indexé par son identifiant stable.
    pub pad_names: HashMap<String, String>,
    /// Certains dongles 2,4 GHz annoncent sticks, gâchettes et vibrations que
    /// la manette n'a pas. Quand ce drapeau est vrai pour une manette, ces
    /// fonctions sont masquées dans sa fiche et ses tests.
    pub pad_hide_dongle_axes: HashMap<String, bool>,
}

/// Source d'illustrations, classée par préférence dans `ArtworkConfig`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtSource {
    /// Fiches PICS, CDN Steam et cache local du client Steam.
    Steam,
    /// Illustrations communautaires SteamGridDB (clé personnelle).
    Steamgriddb,
}

impl ArtSource {
    pub fn label(self) -> &'static str {
        match self {
            ArtSource::Steam => "Lapinux",
            ArtSource::Steamgriddb => "SteamGridDB",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ArtworkConfig {
    /// Clé personnelle SteamGridDB (envoyée en `Authorization: Bearer`).
    pub steamgriddb_key: String,
    /// Autorise SteamGridDB ; l'ordre de `sources` décide de sa priorité.
    pub steamgriddb: bool,
    /// Ordre de préférence pour les jaquettes, bannières, logos et icônes.
    pub sources: Vec<ArtSource>,
}

/// Application Discord de Lapinux : les pages de l'application se publient
/// sous son nom, Discord ne laissant pas une activité choisir son titre.
pub const DEFAULT_DISCORD_CLIENT_ID: &str = "1565408468138065972";

/// Rich Presence Discord.
///
/// Le titre d'une activité est le nom de l'application Discord qui la publie.
/// En partie et sur une fiche, Lapinux se présente donc sous l'application
/// officielle du jeu, trouvée dans le catalogue détectable : le titre est le
/// jeu. Les pages de l'application, elles, se publient sous l'application
/// Lapinux, faute de pouvoir renommer une activité.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct DiscordConfig {
    /// Publie l'activité de Lapinux sur Discord.
    pub enabled: bool,
    /// La page ouverte quand aucune fiche ne l'est : sa phrase et son
    /// contenu (jeux installés, téléchargements, manettes, captures…).
    pub show_library: bool,
    /// Le jeu dont la fiche est ouverte.
    pub show_viewing: bool,
    /// Le téléchargement en cours (jeu, progression).
    pub show_downloads: bool,
    /// La partie en cours, avec son temps de session.
    pub show_game: bool,
}

impl Default for DiscordConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            show_library: true,
            show_viewing: true,
            show_downloads: true,
            show_game: true,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            general: GeneralConfig::default(),
            library: LibraryConfig::default(),
            collections: CollectionsConfig::default(),
            steam: SteamConfig::default(),
            launch: LaunchConfig::default(),
            overlay: OverlayConfig::default(),
            input: InputConfig::default(),
            artwork: ArtworkConfig::default(),
            downloads: DownloadsConfig::default(),
            capture: CaptureConfig::default(),
            discord: DiscordConfig::default(),
        }
    }
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            lang: crate::i18n::system_language(),
            game_language: String::new(),
            library_root: default_library_root().to_string_lossy().to_string(),
            theme: "sombre".into(),
            active_plugins: Vec::new(),
            topbar_order: Vec::new(),
            animations: true,
            big_picture: false,
            grid_width: 168,
            launch_screen: "capsule".into(),
            saves_root: String::new(),
            saves_keep: 3,
            saves_auto: true,
            saves_skip_cloud: true,
            saves_relaunch: false,
            tray_idle_minutes: 15,
        }
    }
}

/// Valeur par défaut de la reprise cloud après installation : activée, comme
/// la synchronisation cloud avant et après une partie.
fn cloud_saves_after_install_default() -> bool {
    true
}

/// Valeur par défaut des outils de boutique : cachés, la grille ne garde que
/// les jeux.
fn hidden_by_default() -> bool {
    true
}

/// Valeur par défaut des démos : visibles, comme les jeux qu'elles montrent.
fn shown_by_default() -> bool {
    true
}

impl LibraryConfig {
    /// Les outils de cette boutique sont-ils masqués dans la bibliothèque ?
    /// Le réglage global (`hide_all_tools`) l'emporte sur la case de la
    /// boutique, comme le slide « tous les comptes » des réglages.
    pub fn store_tools_hidden(&self, store: crate::stores::Store) -> bool {
        if self.hide_all_tools {
            return true;
        }
        match store {
            crate::stores::Store::Steam => self.hide_tools,
            crate::stores::Store::Epic => self.hide_epic_tools,
            crate::stores::Store::Gog => self.hide_gog_tools,
            crate::stores::Store::Itch => self.hide_itch_tools,
        }
    }
}

impl Default for LibraryConfig {
    fn default() -> Self {
        Self {
            hide_tools: false,
            show_demos: true,
            hide_all_tools: false,
            hide_epic_tools: true,
            hide_gog_tools: true,
            hide_itch_tools: true,
            show_badges: true,
            badge_scale: 100,
            cover_radius: 10,
            grid_spacing: 14,
            hover_width: 280,
            sort: LibrarySort::Name,
            sources: LibrarySources::default(),
            filter: LibraryInstallFilter::All,
        }
    }
}

impl Default for DownloadsConfig {
    fn default() -> Self {
        Self {
            global_mbit_per_sec: 0,
            store_mbit_per_sec: HashMap::new(),
            parallel_installs: true,
            cloud_saves_after_install: true,
            steam_server: String::new(),
            epic_cdn: String::new(),
        }
    }
}

impl Default for SteamConfig {
    fn default() -> Self {
        Self {
            username: String::new(),
            offline_tools: false,
            presence: crate::social::SteamPresence::default(),
            social: true,
            show_playing: true,
        }
    }
}

impl Default for LaunchConfig {
    fn default() -> Self {
        Self {
            proton: "auto".into(),
            gamescope: true,
            scene: true,
            output: String::new(),
            width: 2560,
            height: 1440,
            fps_cap: 120,
            hdr: false,
            adaptive_sync: true,
            fullscreen: true,
            mangohud: false,
            steam_headless: true,
        }
    }
}

impl Default for OverlayConfig {
    fn default() -> Self {
        Self {
            hotkey: "Shift+Tab".into(),
            pad_hotkey: "Guide".into(),
            fps_max: 60,
        }
    }
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            grab_devices: true,
            deadzone: 0.12,
            stick_curve: 1.6,
            default_profile: "xbox".into(),
            face_profile: FaceButtonProfile::Xbox,
            gyro: false,
            pad_names: HashMap::new(),
            pad_hide_dongle_axes: HashMap::new(),
        }
    }
}

impl Default for ArtworkConfig {
    fn default() -> Self {
        Self {
            steamgriddb_key: String::new(),
            steamgriddb: true,
            sources: vec![ArtSource::Steam, ArtSource::Steamgriddb],
        }
    }
}

/// `/var/mnt/Jeux` si le disque de jeux est monté et assez libre, sinon `~/Jeux`.
pub fn default_library_root() -> PathBuf {
    for candidate in ["/var/mnt/Jeux", &format!("{}/Jeux", home_dir().display())] {
        let path = PathBuf::from(candidate);
        if path.parent().map(|p| p.exists()).unwrap_or(false) && candidate.starts_with("/var/mnt") {
            return path.join("LapinuxLibrary");
        }
    }
    home_dir().join("Jeux/LapinuxLibrary")
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

    pub fn load_or_create(paths: &LapinuxPaths) -> Result<Self, crate::error::Error> {
        let mut cfg = Self::load(&paths.config_file)?;
        cfg.adopt_primary_display();
        let outdated = std::fs::read_to_string(&paths.config_file)
            .ok()
            .and_then(|raw| toml::from_str::<Config>(&raw).ok())
            .map(|raw| raw != cfg)
            .unwrap_or(true);
        if outdated {
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
        if self.general.library_root.trim().is_empty() {
            self.general.library_root = default_library_root().to_string_lossy().to_string();
        }
        if self.general.library_root.starts_with('~') {
            self.general.library_root = expand_tilde(&self.general.library_root)
                .to_string_lossy()
                .to_string();
        }
        // Une langue Steam inconnue retombe sur « comme Steam » plutôt que de
        // bloquer le choix des dépôts.
        self.general.game_language = crate::lang::find(&self.general.game_language)
            .map(|language| language.code.to_string())
            .unwrap_or_default();
        self.general.grid_width = self.general.grid_width.clamp(72, 240);
        self.library.cover_radius = self.library.cover_radius.min(32);
        self.library.grid_spacing = self.library.grid_spacing.clamp(4, 40);
        self.library.hover_width = self.library.hover_width.clamp(220, 420);
        // Zéro veut dire « illimité » et le reste : il ne doit pas être
        // ramené à une limite. Les autres valeurs restent dans les bornes que
        // l'overlay propose.
        self.launch.fps_cap = if self.launch.fps_cap == 0 {
            0
        } else {
            self.launch.fps_cap.clamp(30, MAX_FPS_CAP)
        };
        self.overlay.fps_max = self.overlay.fps_max.clamp(30, 240);
        // Ancienne valeur par défaut (avant l'ajout du bouton Guide).
        if self.overlay.pad_hotkey.trim().is_empty()
            || self.overlay.pad_hotkey == "L1+R1+Start"
        {
            self.overlay.pad_hotkey = "Guide".into();
        }
        // Ancien raccourci clavier par défaut : on passe au combo façon Steam.
        if self.overlay.hotkey.trim().is_empty() || self.overlay.hotkey == "Meta+F1" {
            self.overlay.hotkey = "Shift+Tab".into();
        }
        self.input.deadzone = self.input.deadzone.clamp(0.0, 0.6);
        self.input.stick_curve = self.input.stick_curve.clamp(0.5, 4.0);
        // L'ancien profil « defaut » ne portait aucun remap : il devient le
        // profil Xbox intégré, désormais nommé explicitement.
        if self.input.default_profile.trim().is_empty()
            || self.input.default_profile == "defaut"
        {
            self.input.default_profile = "xbox".into();
        }
        self.downloads.global_mbit_per_sec = self.downloads.global_mbit_per_sec.min(100_000);
        // Une cadence hors bornes ferait refuser le réglage par l'enregistreur.
        self.capture.video_fps = self.capture.video_fps.clamp(15, 240);
        self.capture.video_name_format = self.capture.video_name_format.trim().to_string();
        if self.capture.video_name_format.is_empty() {
            self.capture.video_name_format = default_video_name_format();
        }
        self.capture.video_dir = self.capture.video_dir.trim().to_string();
        self.capture.image_dir = self.capture.image_dir.trim().to_string();
        self.capture.video_codec = match self.capture.video_codec.trim().to_ascii_lowercase().as_str()
        {
            "h264" => "h264".into(),
            "hevc" => "hevc".into(),
            "av1" => "av1".into(),
            _ => "auto".into(),
        };
        self.capture.video_profile = match self
            .capture
            .video_profile
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "performance" => "performance".into(),
            "quality" => "quality".into(),
            _ => "balanced".into(),
        };
        self.capture.video_resolution = match self
            .capture
            .video_resolution
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "720p" => "720p".into(),
            "1440p" | "2k" => "1440p".into(),
            "native" => "native".into(),
            _ => "1080p".into(),
        };
        self.capture.video_encoder = match self
            .capture
            .video_encoder
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "nvenc" => "nvenc".into(),
            "software" => "software".into(),
            _ => "auto".into(),
        };
        for value in self.downloads.store_mbit_per_sec.values_mut() {
            *value = (*value).min(100_000);
        }
        self.artwork.steamgriddb_key = self.artwork.steamgriddb_key.trim().to_string();
        // Chaque source connue apparaît une fois, dans l'ordre choisi ; une
        // source ajoutée par une version future ne fait pas échouer la lecture.
        let mut sources: Vec<ArtSource> = Vec::new();
        for source in &self.artwork.sources {
            if !sources.contains(source) {
                sources.push(*source);
            }
        }
        for source in [ArtSource::Steam, ArtSource::Steamgriddb] {
            if !sources.contains(&source) {
                sources.push(source);
            }
        }
        self.artwork.sources = sources;
        self.general.lang = crate::i18n::normalize(&self.general.lang);
        self
    }

    pub fn library_root(&self) -> PathBuf {
        expand_tilde(&self.general.library_root)
    }

    /// Aucun écran choisi : reprend le mode courant de l'écran principal pour
    /// la résolution et la limite de FPS, et retient son connecteur. Une fois
    /// le connecteur enregistré, les valeurs de l'utilisateur sont respectées.
    pub fn adopt_primary_display(&mut self) -> bool {
        if !self.launch.output.is_empty() {
            return false;
        }
        let Some(display) = crate::display::primary() else {
            return false;
        };
        self.launch.apply_display(&display);
        true
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
        assert_eq!(cfg.overlay.pad_hotkey, "Guide");
        assert_eq!(cfg.overlay.hotkey, "Shift+Tab");
    }

    #[test]
    fn roundtrip_toml() {
        let cfg = Config::default();
        let raw = toml::to_string_pretty(&cfg).unwrap();
        let back: Config = toml::from_str(&raw).unwrap();
        assert_eq!(cfg, back);
    }

    #[test]
    fn old_launch_config_enables_the_headless_steam_client() {
        // Une configuration écrite avant ce réglage doit adopter le nouveau
        // comportement sans être réécrite à la main.
        let cfg: Config = toml::from_str("[launch]\nproton = \"auto\"\n").unwrap();
        assert!(cfg.launch.steam_headless);
    }

    #[test]
    fn old_default_controller_profile_becomes_xbox() {
        let old: Config = toml::from_str("[input]\ndefault_profile = \"defaut\"\n").unwrap();
        assert_eq!(old.normalized().input.default_profile, "xbox");
    }

    #[test]
    fn face_profile_defaults_to_xbox_and_reads_nintendo() {
        let old: Config = toml::from_str("[input]\ndeadzone = 0.2\n").unwrap();
        assert_eq!(old.input.face_profile, FaceButtonProfile::Xbox);
        let nintendo: Config = toml::from_str("[input]\nface_profile = \"nintendo\"\n").unwrap();
        assert_eq!(nintendo.input.face_profile, FaceButtonProfile::Nintendo);
    }

    #[test]
    fn artwork_sources_keep_one_of_each_in_order() {
        let mut cfg = Config::default();
        cfg.artwork.sources = vec![ArtSource::Steamgriddb, ArtSource::Steam];
        let cfg = cfg.normalized();
        assert_eq!(
            cfg.artwork.sources,
            vec![ArtSource::Steamgriddb, ArtSource::Steam]
        );

        let mut cfg = cfg;
        cfg.artwork.sources = vec![ArtSource::Steamgriddb, ArtSource::Steamgriddb];
        let cfg = cfg.normalized();
        assert_eq!(cfg.artwork.sources.len(), 2);
        assert_eq!(cfg.artwork.sources[0], ArtSource::Steamgriddb);
        assert_eq!(cfg.artwork.sources[1], ArtSource::Steam);
    }

    #[test]
    fn an_old_config_announces_steam_games_by_default() {
        // La section existait avant le réglage : un fichier qui ne le contient
        // pas doit garder le comportement attendu, sans réécriture manuelle.
        let cfg: Config = toml::from_str("[steam]\nusername = \"bob\"\n").unwrap();
        assert!(cfg.steam.show_playing);
    }

    #[test]
    fn old_config_without_sources_gets_the_default_order() {
        let raw = "[artwork]\nsteamgriddb = false\nsteamgriddb_key = \"abc\"\n";
        let cfg: Config = toml::from_str(raw).unwrap();
        assert_eq!(
            cfg.normalized().artwork.sources,
            vec![ArtSource::Steam, ArtSource::Steamgriddb]
        );
    }

    #[test]
    fn capture_defaults_record_and_clamp() {
        let cfg = Config::default();
        assert!(cfg.capture.record_on_launch);
        assert!(cfg.capture.stop_on_exit);
        // Le micro reste coupé tant qu'on ne le demande pas.
        assert!(!cfg.capture.record_microphone);
        assert!(cfg.capture.record_audio);
        assert_eq!(cfg.capture.video_fps, 30);
        assert_eq!(cfg.capture.video_profile, "balanced");
        assert_eq!(cfg.capture.video_resolution, "1080p");
        assert_eq!(cfg.capture.video_encoder, "auto");
        assert_eq!(cfg.capture.video_name_format, DEFAULT_NAME_FORMAT);
        assert!(cfg.capture.video_name_format.contains("FPS"));
        // Une configuration écrite avant ce réglage garde le micro coupé.
        let old: Config = toml::from_str("[capture]\nrecord_audio = true\n").unwrap();
        assert!(!old.capture.record_microphone);
        let mut high = Config::default();
        high.capture.video_fps = 1000;
        high.capture.video_codec = "mpeg2".into();
        high.capture.video_profile = "turbo".into();
        high.capture.video_resolution = "4k".into();
        assert_eq!(
            Config {
                capture: CaptureConfig {
                    video_resolution: "2K".into(),
                    ..CaptureConfig::default()
                },
                ..Config::default()
            }
            .normalized()
            .capture
            .video_resolution,
            "1440p"
        );
        high.capture.video_encoder = "magic".into();
        let high = high.normalized();
        assert_eq!(high.capture.video_fps, 240);
        assert_eq!(high.capture.video_codec, "auto");
        assert_eq!(high.capture.video_profile, "balanced");
        assert_eq!(high.capture.video_resolution, "1080p");
        assert_eq!(high.capture.video_encoder, "auto");
        // Ancienne configuration sans section : l'enregistrement suit le jeu.
        let old: Config = toml::from_str("[general]\nlang = \"fr\"\n").unwrap();
        assert!(old.capture.record_on_launch);
        assert!(!old.capture.record_microphone);
        // Les réglages d'encodage absents prennent les valeurs économes.
        assert_eq!(old.capture.video_fps, 30);
        assert_eq!(old.capture.video_profile, "balanced");
        assert_eq!(old.capture.video_resolution, "1080p");
        assert_eq!(old.capture.video_encoder, "auto");
    }

    #[test]
    fn clamps_weird_values() {
        let mut cfg = Config::default();
        cfg.launch.fps_cap = 5;
        cfg.input.deadzone = 5.0;
        let cfg = cfg.normalized();
        assert_eq!(cfg.launch.fps_cap, 30);
        assert_eq!(cfg.input.deadzone, 0.6);
        // Zéro garde son sens « illimité », et une valeur trop haute retombe
        // sur la borne de GameScope.
        let mut unlimited = Config::default();
        unlimited.launch.fps_cap = 0;
        assert_eq!(unlimited.normalized().launch.fps_cap, 0);
        let mut too_high = Config::default();
        too_high.launch.fps_cap = 900;
        assert_eq!(too_high.normalized().launch.fps_cap, MAX_FPS_CAP);
    }

    /// La langue des jeux est un nom Steam : un nom inconnu retombe sur
    /// « comme Steam » plutôt que de bloquer le choix des dépôts.
    #[test]
    fn game_language_is_kept_or_cleared() {
        let mut cfg = Config::default();
        cfg.general.game_language = "french".into();
        assert_eq!(cfg.normalized().general.game_language, "french");

        let mut cfg = Config::default();
        cfg.general.game_language = "klingon".into();
        assert_eq!(cfg.normalized().general.game_language, "");
    }

    #[test]
    fn old_overlay_chord_migrates_to_guide() {
        let mut cfg = Config::default();
        cfg.overlay.pad_hotkey = "L1+R1+Start".into();
        assert_eq!(cfg.normalized().overlay.pad_hotkey, "Guide");
        let mut custom = Config::default();
        custom.overlay.pad_hotkey = "L1+R1+Select".into();
        assert_eq!(custom.normalized().overlay.pad_hotkey, "L1+R1+Select");
    }

    #[test]
    fn old_keyboard_hotkey_migrates_to_shift_tab() {
        let mut cfg = Config::default();
        cfg.overlay.hotkey = "Meta+F1".into();
        assert_eq!(cfg.normalized().overlay.hotkey, "Shift+Tab");
        let mut custom = Config::default();
        custom.overlay.hotkey = "Ctrl+F10".into();
        assert_eq!(custom.normalized().overlay.hotkey, "Ctrl+F10");
    }

    #[test]
    fn load_or_create_persists_migrated_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        paths.ensure().unwrap();
        std::fs::write(
            &paths.config_file,
            "[overlay]\nhotkey = \"Meta+F1\"\npad_hotkey = \"L1+R1+Start\"\n",
        )
        .unwrap();
        let cfg = Config::load_or_create(&paths).unwrap();
        assert_eq!(cfg.overlay.hotkey, "Shift+Tab");
        let saved = std::fs::read_to_string(&paths.config_file).unwrap();
        assert!(saved.contains("Shift+Tab"));
        assert!(!saved.contains("Meta+F1"));
    }

    #[test]
    fn missing_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = Config::load(&dir.path().join("absent.toml")).unwrap();
        assert_eq!(cfg.general.lang, "fr");
        assert_eq!(cfg.general.grid_width, 168);
    }

    #[test]
    fn grid_width_is_clamped() {
        let mut cfg = Config::default();
        cfg.general.grid_width = 10;
        assert_eq!(cfg.clone().normalized().general.grid_width, 72);
        cfg.general.grid_width = 999;
        assert_eq!(cfg.normalized().general.grid_width, 240);
    }

    #[test]
    fn library_visuals_are_clamped() {
        let mut cfg = Config::default();
        cfg.library.cover_radius = 99;
        cfg.library.grid_spacing = 1;
        cfg.library.hover_width = 999;
        let cfg = cfg.normalized();
        assert_eq!(cfg.library.cover_radius, 32);
        assert_eq!(cfg.library.grid_spacing, 4);
        assert_eq!(cfg.library.hover_width, 420);
    }

    #[test]
    fn hover_width_keeps_a_readable_minimum() {
        let mut cfg = Config::default();
        cfg.library.hover_width = 1;
        assert_eq!(cfg.normalized().library.hover_width, 220);
    }

    #[test]
    fn library_view_survives_a_restart() {
        let mut cfg = Config::default();
        cfg.library.sort = LibrarySort::Playtime;
        cfg.library.sources.epic = false;
        cfg.library.filter = LibraryInstallFilter::Installed;
        let text = toml::to_string(&cfg).unwrap();
        let parsed: Config = toml::from_str(&text).unwrap();
        assert_eq!(parsed.library.sort, LibrarySort::Playtime);
        assert!(!parsed.library.sources.epic);
        assert_eq!(parsed.library.filter, LibraryInstallFilter::Installed);
        // Une configuration écrite avant ces champs garde la vue historique :
        // tri alphabétique, toutes les sources, aucun filtre d'installation.
        let old: Config = toml::from_str("[library]\nhover_width = 300\n").unwrap();
        assert_eq!(old.library.sort, LibrarySort::Name);
        assert_eq!(old.library.filter, LibraryInstallFilter::All);
        assert!(old.library.sources.disk && old.library.sources.steam);
        assert!(old.library.sources.epic && old.library.sources.gog);
        assert!(old.library.sources.itch);
    }

    #[test]
    fn collections_survive_a_restart() {
        let mut cfg = Config::default();
        cfg.collections.items.push(GameCollection {
            id: "c1".into(),
            name: "RPG".into(),
            field: None,
            appids: vec![10, 20],
        });
        cfg.collections.items.push(GameCollection {
            id: "c2".into(),
            name: "Par éditeur".into(),
            field: Some(CollectionField::Publisher),
            appids: Vec::new(),
        });
        let text = toml::to_string(&cfg).unwrap();
        let parsed: Config = toml::from_str(&text).unwrap();
        assert_eq!(parsed.collections, cfg.collections);
        let old: Config = toml::from_str("").unwrap();
        assert!(old.collections.items.is_empty());
    }

    #[test]
    fn downloads_start_parallel_and_keep_the_choice() {
        let cfg = Config::default();
        assert!(cfg.downloads.parallel_installs);
        assert!(cfg.downloads.cloud_saves_after_install);
        let text = toml::to_string(&cfg).unwrap();
        let parsed: Config = toml::from_str(&text).unwrap();
        assert!(parsed.downloads.parallel_installs);
        assert!(parsed.downloads.cloud_saves_after_install);
        // Une configuration existante sans ces champs garde le comportement
        // historique : plusieurs téléchargements à la fois, cloud compris.
        let old: Config = toml::from_str("[downloads]\nglobal_mbit_per_sec = 0\n").unwrap();
        assert!(old.downloads.parallel_installs);
        assert!(old.downloads.cloud_saves_after_install);
    }

    #[test]
    fn download_limit_takes_the_strictest_cap() {
        let mut cfg = Config::default();
        assert_eq!(cfg.downloads.effective_mbit("steam"), 0);
        cfg.downloads.global_mbit_per_sec = 400;
        assert_eq!(cfg.downloads.effective_mbit("steam"), 400);
        cfg.downloads.store_mbit_per_sec.insert("steam".into(), 150);
        assert_eq!(cfg.downloads.effective_mbit("steam"), 150);
        cfg.downloads.store_mbit_per_sec.insert("steam".into(), 800);
        assert_eq!(cfg.downloads.effective_mbit("steam"), 400);
        cfg.downloads.global_mbit_per_sec = 0;
        assert_eq!(cfg.downloads.effective_mbit("steam"), 800);
    }

    #[test]
    fn choosing_an_output_stops_the_default_detection() {
        let mut cfg = Config::default();
        cfg.launch.output = "DP-2".into();
        assert!(!cfg.adopt_primary_display());
        assert_eq!(cfg.launch.width, 2560);
    }

    #[test]
    fn apply_display_takes_mode_and_refresh() {
        let mut cfg = Config::default();
        cfg.launch.apply_display(&crate::display::DisplayInfo {
            name: "DP-2".into(),
            width: 1920,
            height: 1080,
            refresh_hz: 165,
            primary: true,
        });
        assert_eq!(cfg.launch.output, "DP-2");
        assert_eq!((cfg.launch.width, cfg.launch.height), (1920, 1080));
        assert_eq!(cfg.launch.fps_cap, 165);
    }

    #[test]
    fn empty_library_root_falls_back_to_default() {
        let mut cfg = Config::default();
        cfg.general.library_root = "   ".into();
        let cfg = cfg.normalized();
        assert_eq!(cfg.library_root(), default_library_root());
    }
}
