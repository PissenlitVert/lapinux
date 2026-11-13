//! Modèles partagés entre le daemon, l'interface, l'overlay et la CLI.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Drm {
    /// Aucun DRM Steam détecté.
    None,
    /// Utilise steam_api (succès, cloud, statuts) : le jeu démarre sans
    /// client, mais Steamworks ne s'initialise qu'avec lui.
    Steamworks,
    /// SteamStub/CEG : nécessite le client Steam.
    SteamStub,
    /// Non encore analysé.
    Unknown,
}

impl Drm {
    pub fn label(self) -> &'static str {
        match self {
            Drm::None => "sans DRM",
            Drm::Steamworks => "Steamworks",
            Drm::SteamStub => "DRM Steam",
            Drm::Unknown => "inconnu",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct GameSettings {
    /// Nom du Proton à utiliser, sinon celui de la config.
    pub proton: Option<String>,
    pub gamescope: Option<bool>,
    /// None suit le choix global ; Some(Off) désactive pour ce jeu.
    pub frame_generation: Option<crate::config::FrameGeneration>,
    /// None suit la version globale.
    pub afmf_version: Option<String>,
    pub afmf_performance_mode: Option<crate::config::AfmfPerformanceMode>,
    pub afmf_search_mode: Option<crate::config::AfmfSearchMode>,
    /// Multiplicateur de génération d'images de ce jeu (0, 2, 3 ou 4) ;
    /// None suit le réglage global.
    #[serde(default)]
    pub afmf_multiplier: Option<u32>,
    pub scaling_filter: Option<crate::config::ScalingFilter>,
    /// Netteté GameScope de ce jeu ; sinon celle des réglages globaux.
    #[serde(default)]
    pub sharpness: Option<u32>,
    pub vkbasalt: Option<bool>,
    pub vkbasalt_effect: Option<crate::config::VkBasaltEffect>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps_cap: Option<u32>,
    /// Lève la limite que le jeu s'impose dans ses propres fichiers de
    /// réglages (Unreal Engine : `GameUserSettings.ini`). Lapinux les réécrit
    /// avant chaque lancement et garde l'original pour revenir en arrière.
    #[serde(default)]
    pub unlock_fps: Option<bool>,
    /// HUD nommé de ce jeu (`hud_custom`), ou preset intégré. Vide suit le
    /// HUD actif des réglages globaux.
    #[serde(default)]
    pub hud_style: Option<String>,
    pub hdr: Option<bool>,
    pub adaptive_sync: Option<bool>,
    /// Plaque de performance pour ce jeu ; sinon le réglage global.
    #[serde(default)]
    pub hud_on_screen: Option<bool>,
    /// Badge d'enregistrement pour ce jeu ; sinon le réglage global.
    #[serde(default)]
    pub record_badge: Option<bool>,
    /// Enregistrement automatique au lancement de ce jeu.
    #[serde(default)]
    pub record_on_launch: Option<bool>,
    /// Arrêt de l'enregistrement à la fermeture de ce jeu.
    #[serde(default)]
    pub stop_on_exit: Option<bool>,
    /// Cadence vidéo propre à ce jeu.
    #[serde(default)]
    pub video_fps: Option<u32>,
    /// Profil d'encodage propre à ce jeu (`performance`, `balanced`, `quality`).
    #[serde(default)]
    pub video_profile: Option<String>,
    /// Hauteur vidéo propre à ce jeu (`720p`, `1080p`, `1440p`, `native`).
    #[serde(default)]
    pub video_resolution: Option<String>,
    /// Encodeur vidéo propre à ce jeu (`auto`, `nvenc`, `software`).
    #[serde(default)]
    pub video_encoder: Option<String>,
    /// Codec vidéo propre à ce jeu (`auto`, `h264`, `hevc`, `av1`).
    #[serde(default)]
    pub video_codec: Option<String>,
    /// Son du jeu dans les captures de ce jeu.
    #[serde(default)]
    pub record_audio: Option<bool>,
    /// Microphone dans les captures de ce jeu.
    #[serde(default)]
    pub record_microphone: Option<bool>,
    /// Copie locale automatique des sauvegardes de ce jeu.
    #[serde(default)]
    pub saves_auto: Option<bool>,
    /// Saute la copie locale quand le cloud de la boutique garde déjà ce jeu.
    #[serde(default)]
    pub saves_skip_cloud: Option<bool>,
    /// Relance ce jeu après le chargement d'un emplacement de sauvegarde.
    #[serde(default)]
    pub saves_relaunch: Option<bool>,
    /// Variables d'environnement additionnelles.
    pub env: Vec<(String, String)>,
    /// Arguments ajoutés après le chemin de l'exécutable.
    pub args: Vec<String>,
    /// Profil manettes associé.
    pub input_profile: Option<String>,
    /// Capture des manettes pour ce jeu ; sinon le réglage global.
    #[serde(default)]
    pub grab_devices: Option<bool>,
    /// Langue des textes et du doublage (`french`, `schinese`…), quand le jeu
    /// la propose ; `None` suit le réglage général.
    pub language: Option<String>,
    /// Dossier local des sauvegardes d'un jeu de boutique, synchronisé avec le
    /// cloud de sa boutique. Sans valeur, Lapinux prend `sauvegardes/` dans le
    /// dossier du jeu. Un jeu Steam n'en a pas besoin : son chemin vient de son
    /// manifeste.
    pub save_dir: Option<String>,
    /// Synchronisation cloud automatique de ce jeu : reprise après
    /// installation ou avant le lancement, puis envoi après la partie. Sans
    /// valeur, elle est active — le réglage est né activé pour tous les jeux
    /// existants. Les boutons manuels de la fiche restent disponibles quand
    /// elle est coupée.
    #[serde(default)]
    pub cloud_sync: Option<bool>,
    /// Ce que DXVK annonce de la carte au moteur du jeu : certains moteurs
    /// lisent mal la mémoire de l'adaptateur et se bridinent eux-mêmes.
    pub dxvk: DxvkOverride,
    /// Options de compatibilité explicites, sans modification permanente du préfixe.
    pub compatibility: CompatibilityOptions,
    /// Mode performance : coupe la scène de lancement, l'enregistrement
    /// automatique et les HUD qui dessinent par-dessus le jeu. GameScope, la
    /// résolution et la limite FPS restent ceux du jeu ; l'enregistrement
    /// manuel reste disponible.
    pub performance: bool,
    /// Pont Steamworks : `Some(true)` l'active, `Some(false)` le désactive.
    /// `None` est désactivé en natif ; sous Windows, le défaut dépend du jeu.
    #[serde(default)]
    pub steam_shim: Option<bool>,
    /// Volume du jeu retenu entre deux parties (0 à 1). Le menu rapide
    /// l'écrit ; le lancement suivant le réapplique au flux du jeu, sans
    /// toucher au son du bureau.
    #[serde(default)]
    pub volume: Option<f32>,
    /// Injecteurs graphiques et DLL DLSS maintenus par Lapinux malgré le
    /// verdict d'un anti-triche. Un jeu protégé se lance sans eux tant que ce
    /// choix explicite manque.
    #[serde(default)]
    pub allow_protected_injections: Option<bool>,
}

/// Réglages expérimentaux par jeu, laissés au comportement du Proton par défaut.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct CompatibilityOptions {
    pub wined3d: bool,
    pub dxvk_d3d8: bool,
    pub old_gl_string: bool,
    pub no_fsync: bool,
    pub no_esync: bool,
    pub no_ntsync: bool,
    pub d3d9_mode_count: bool,
    pub d3d9_cube_depth: bool,
    pub d3d9_sampler_type: bool,
    pub vkd3d_no_dxr: bool,
}

impl CompatibilityOptions {
    pub fn dxvk_legacy_line(&self) -> Option<String> {
        let mut flags = Vec::new();
        if self.d3d9_mode_count {
            flags.push("d3d9.modeCountCompatibility = True");
        }
        if self.d3d9_cube_depth {
            flags.push("d3d9.supportCubeDepthFormats = True");
        }
        if self.d3d9_sampler_type {
            flags.push("d3d9.forceSamplerTypeSpecConstants = True");
        }
        (!flags.is_empty()).then(|| flags.join("; "))
    }
}

/// Jeux que le pont Steamworks écarte par défaut. La liste est vide : le pont
/// s'installe pour tous les jeux, sauf refus explicite du joueur. Le crash de
/// SMS Returns venait du retour des `CSteamID` par valeur ; l'ABI à pointeur
/// caché est corrigée, donc plus aucun jeu n'est écarté d'office.
pub const STEAM_SHIM_EXCLUDED: &[u32] = &[];

/// Le pont est-il écarté par défaut pour ce jeu ?
pub fn steam_shim_excluded(appid: u32) -> bool {
    STEAM_SHIM_EXCLUDED.contains(&appid)
}

impl GameSettings {
    /// La synchronisation cloud automatique est active par défaut : un jeu
    /// enregistré avant l'existence du réglage n'a pas de valeur explicite.
    pub fn cloud_sync_enabled(&self) -> bool {
        self.cloud_sync.unwrap_or(true)
    }

    /// Un jeu protégé refuse les injecteurs installés et les DLL DLSS
    /// remplacées tant que le joueur ne les a pas autorisés explicitement.
    /// Seule règle du refus, partagée par le lancement, les états et la fiche.
    pub fn injections_blocked(&self, protected: bool) -> bool {
        protected && self.allow_protected_injections != Some(true)
    }

    /// Ce qu'un anti-triche impose : le pont Steamworks, les injecteurs
    /// (effets vkBasalt, génération d'images, plaque de performance) et
    /// l'enregistrement automatique restent coupés tant que le joueur ne les
    /// réclame pas explicitement. Un champ déjà décidé par le joueur garde sa
    /// valeur : cette fonction ne remplit que les défauts.
    pub fn apply_protected_defaults(&mut self, native: bool) {
        self.vkbasalt.get_or_insert(false);
        self.hud_on_screen.get_or_insert(false);
        self.record_on_launch.get_or_insert(false);
        if self.frame_generation.is_none() {
            self.frame_generation = Some(crate::config::FrameGeneration::Off);
        }
        // Un jeu natif n'a pas de pont Windows à couper.
        if !native {
            self.steam_shim.get_or_insert(false);
        }
    }

    /// Capture effective de ce jeu : chaque champ laissé vide reprend la
    /// valeur globale, les autres la remplacent.
    pub fn effective_capture(
        &self,
        global: &crate::config::CaptureConfig,
    ) -> crate::config::CaptureConfig {
        let mut capture = global.clone();
        if let Some(value) = self.record_on_launch {
            capture.record_on_launch = value;
        }
        if let Some(value) = self.stop_on_exit {
            capture.stop_on_exit = value;
        }
        if let Some(value) = self.video_fps {
            capture.video_fps = value.clamp(5, 240);
        }
        if let Some(value) = &self.video_profile {
            capture.video_profile = value.clone();
        }
        if let Some(value) = &self.video_resolution {
            capture.video_resolution = value.clone();
        }
        if let Some(value) = &self.video_encoder {
            capture.video_encoder = value.clone();
        }
        if let Some(value) = &self.video_codec {
            capture.video_codec = value.clone();
        }
        if let Some(value) = self.record_audio {
            capture.record_audio = value;
        }
        if let Some(value) = self.record_microphone {
            capture.record_microphone = value;
        }
        if let Some(value) = self.hud_on_screen {
            capture.hud_on_screen = value;
        }
        if let Some(value) = self.record_badge {
            capture.record_badge = value;
        }
        capture
    }

    pub fn sharpness(&self, global: u32) -> u32 {
        self.sharpness.unwrap_or(global)
    }

    pub fn hud_on_screen(&self, global: bool) -> bool {
        self.hud_on_screen.unwrap_or(global)
    }

    pub fn record_badge(&self, global: bool) -> bool {
        self.record_badge.unwrap_or(global)
    }

    pub fn saves_auto(&self, global: bool) -> bool {
        self.saves_auto.unwrap_or(global)
    }

    pub fn saves_skip_cloud(&self, global: bool) -> bool {
        self.saves_skip_cloud.unwrap_or(global)
    }

    pub fn saves_relaunch(&self, global: bool) -> bool {
        self.saves_relaunch.unwrap_or(global)
    }

    pub fn grab_devices(&self, global: bool) -> bool {
        self.grab_devices.unwrap_or(global)
    }
}

/// Réglages DXVK par jeu, passés au lancement par `DXVK_CONFIG`.
///
/// DXVK ne peut que **baisser** les valeurs qu'il rapporte : ce sont des
/// plafonds, pas une description libre de la carte. Le nom, lui, est remplacé
/// tel quel.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct DxvkOverride {
    /// `dxgi.maxSharedMemory` : plafond de la mémoire partagée annoncée (Mo).
    pub shared_mb: Option<u32>,
    /// `dxgi.maxDeviceMemory` : plafond de la mémoire dédiée annoncée (Mo).
    pub device_mb: Option<u32>,
    /// `dxgi.customDeviceDesc` : nom de carte annoncé au moteur.
    pub device: Option<String>,
}

impl DxvkOverride {
    pub fn is_empty(&self) -> bool {
        self.shared_mb.is_none() && self.device_mb.is_none() && self.device.is_none()
    }

    /// Contenu de `DXVK_CONFIG`, une option par segment séparé par `;`.
    ///
    /// Le nom de carte est cité : DXVK s'arrête au premier espace d'une
    /// valeur non citée. Les guillemets, points-virgules et retours à la
    /// ligne sont retirés, sans quoi un nom saisi à la main casserait la
    /// ligne entière.
    pub fn config_line(&self) -> Option<String> {
        let mut options = Vec::new();
        if let Some(mb) = self.shared_mb.filter(|mb| *mb > 0) {
            options.push(format!("dxgi.maxSharedMemory = {mb}"));
        }
        if let Some(mb) = self.device_mb.filter(|mb| *mb > 0) {
            options.push(format!("dxgi.maxDeviceMemory = {mb}"));
        }
        if let Some(device) = self.device.as_deref().map(sanitized_device) {
            if !device.is_empty() {
                options.push(format!("dxgi.customDeviceDesc = \"{device}\""));
            }
        }
        (!options.is_empty()).then(|| options.join("; "))
    }
}

/// Nom de carte tel qu'il peut vivre dans une ligne de configuration DXVK :
/// une seule ligne, sans guillemet ni séparateur, 64 caractères au plus.
fn sanitized_device(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '"' | ';' | '\n' | '\r' => ' ',
            c if c.is_control() => ' ',
            c => c,
        })
        .take(64)
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Un fichier gardé dans Steam Cloud.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct CloudFile {
    /// Chemin relatif tel que Steam le nomme (`saves/slot1.sav`).
    pub name: String,
    /// Taille brute, en octets.
    pub size: u64,
    /// Dernière modification, en secondes Unix (0 si Steam ne la donne pas).
    pub modified: i64,
    /// Empreinte SHA-1 du fichier gardé par Steam : elle dit si la copie
    /// locale a vraiment changé, une date seule pouvant bouger sans contenu.
    #[serde(default)]
    pub sha: Vec<u8>,
    /// Index du dossier déclaré par le jeu dans la réponse du cloud.
    pub prefix_index: usize,
    /// Dossier déclaré par le jeu (`%WinAppDataLocal%MotorSlice/Saved/SaveGames/`),
    /// résolu en index au chargement.
    pub prefix: String,
    /// Copie présente sur le disque : sa date (0 si elle n'existe pas). Rempli
    /// par le daemon quand il sait où le jeu écrit.
    pub local: Option<i64>,
    /// Chemin local résolu et vérifié ; faux si son état est inconnu.
    #[serde(default)]
    pub local_resolved: bool,
    /// Empreinte du fichier local, seulement lorsqu'il a pu être lu.
    #[serde(default)]
    pub local_sha: Option<Vec<u8>>,
    /// Pour Epic/GOG, indique si ce fichier existe dans le cloud.
    /// `None` garde le comportement des relevés Steam antérieurs.
    #[serde(default)]
    pub cloud_present: Option<bool>,
}

/// Sauvegardes cloud d'un jeu : ce que le compte garde, et ce que le jeu
/// déclare comme dossiers.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct CloudSaves {
    pub files: Vec<CloudFile>,
    /// Entrées écartées à la lecture : Steam rend l'historique complet, donc
    /// plusieurs versions d'un même chemin arrivent ensemble. La plus récente
    /// reste dans `files` ; les autres sont gardées ici pour que la copie de
    /// récupération les conserve avant tout remplacement.
    #[serde(default)]
    pub superseded: Vec<CloudFile>,
    /// Numéro de changement du cloud : Steam le compare avant de synchroniser.
    pub change_number: u64,
    /// Dossiers de sauvegarde déclarés par le jeu.
    pub prefixes: Vec<String>,
    pub used_files: Option<u32>,
    pub used_bytes: Option<u64>,
    pub max_files: Option<u32>,
    pub max_bytes: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SteamCloudState {
    UpToDate,
    Conflict,
    MissingLocal,
    Unknown,
}

impl CloudSaves {
    /// Le cloud connaît-il cette empreinte pour ce chemin ? Steam garde
    /// plusieurs versions d'un même fichier (une par machine) : une copie
    /// locale qui correspond à l'une d'elles est déjà une version du cloud,
    /// il n'y a donc rien à protéger ni à choisir.
    pub fn knows(&self, file: &CloudFile, sha: &[u8]) -> bool {
        file.sha == sha
            || self
                .superseded
                .iter()
                .any(|old| old.prefix == file.prefix && old.name == file.name && old.sha == sha)
    }

    /// Une date identique ne prouve jamais que les contenus sont identiques.
    pub fn steam_state(&self) -> SteamCloudState {
        if self.files.is_empty() {
            return SteamCloudState::Unknown;
        }
        let mut missing = false;
        let mut unknown = false;
        for file in &self.files {
            if !file.local_resolved {
                unknown = true;
            } else if file.cloud_present == Some(false) {
                if file.local_sha.is_some() {
                    return SteamCloudState::Conflict;
                }
                unknown = true;
            } else if let Some(local) = &file.local_sha {
                if local.len() != 20 {
                    unknown = true;
                } else if !self.knows(file, local) {
                    return SteamCloudState::Conflict;
                }
            } else {
                missing = true;
            }
        }
        if unknown {
            SteamCloudState::Unknown
        } else if missing {
            SteamCloudState::MissingLocal
        } else {
            SteamCloudState::UpToDate
        }
    }

    /// Poids total des sauvegardes gardées dans le cloud.
    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|file| file.size).sum()
    }

    /// Date de la sauvegarde la plus récente, en secondes Unix.
    pub fn last_modified(&self) -> Option<i64> {
        self.files
            .iter()
            .map(|file| file.modified)
            .filter(|stamp| *stamp > 0)
            .max()
    }

    /// Fichier du cloud le plus récemment modifié, avec sa date si Steam la
    /// publie. La fiche nomme ainsi la sauvegarde derrière la date.
    pub fn latest_file(&self) -> Option<&CloudFile> {
        self.files
            .iter()
            .filter(|file| file.modified > 0)
            .max_by_key(|file| file.modified)
    }

    /// Fichier présent sur le disque dont la copie locale est la plus récente.
    /// Il peut différer du fichier cloud le plus récent : c'est justement ce
    /// que la fiche doit montrer.
    pub fn latest_local_file(&self) -> Option<(&CloudFile, i64)> {
        self.files
            .iter()
            .filter_map(|file| file.local.map(|local| (file, local)))
            .max_by_key(|(_, local)| *local)
    }

    /// Le jeu garde-t-il des sauvegardes ?
    pub fn has_saves(&self) -> bool {
        !self.files.is_empty()
    }
}

/// Ce qu'une partie a changé dans le cloud : les fichiers apparus, ceux qui ont
/// bougé et ceux qui ont disparu entre le lancement et la sortie du jeu.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct CloudDiff {
    pub added: Vec<CloudFile>,
    pub changed: Vec<CloudFile>,
    pub removed: Vec<String>,
}

/// Sens d'une synchronisation cloud : ce que Lapinux a reçu ou envoyé.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CloudSyncDirection {
    /// Le cloud a fourni des fichiers au disque.
    #[default]
    Download,
    /// Le disque a fourni des fichiers au cloud.
    Upload,
}

/// Une ligne du journal local des synchronisations cloud d'un jeu.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct CloudSyncEntry {
    /// Date de l'opération, en secondes Unix.
    pub at: i64,
    pub direction: CloudSyncDirection,
    /// Machine d'origine pour un envoi ; nom du service pour une réception.
    pub machine: String,
    pub files: usize,
    pub bytes: u64,
}

impl CloudDiff {
    /// Rien n'a bougé : la partie n'a rien envoyé.
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.changed.is_empty() && self.removed.is_empty()
    }

    /// Ce que la comparaison a trouvé, en une phrase.
    pub fn summary(&self) -> String {
        if self.is_empty() {
            return "aucun changement dans le cloud".to_string();
        }
        let mut parts = Vec::new();
        if !self.added.is_empty() {
            parts.push(format!("{} ajouté(s)", self.added.len()));
        }
        if !self.changed.is_empty() {
            parts.push(format!("{} modifié(s)", self.changed.len()));
        }
        if !self.removed.is_empty() {
            parts.push(format!("{} retiré(s)", self.removed.len()));
        }
        parts.join(" · ")
    }
}

/// Compare deux relevés du cloud d'un même jeu : ce que la session a changé.
///
/// La comparaison se fait sur le nom, la taille et la date — Steam ne publie pas
/// d'empreinte exploitable dans la liste, et une date qui bouge suffit à dire
/// qu'un fichier a été réécrit.
pub fn cloud_diff(before: &CloudSaves, after: &CloudSaves) -> CloudDiff {
    let mut diff = CloudDiff::default();
    for file in &after.files {
        match before.files.iter().find(|old| old.name == file.name) {
            None => diff.added.push(file.clone()),
            Some(old) if old.size != file.size || old.modified != file.modified => {
                diff.changed.push(file.clone())
            }
            Some(_) => {}
        }
    }
    for old in &before.files {
        if !after.files.iter().any(|file| file.name == old.name) {
            diff.removed.push(old.name.clone());
        }
    }
    diff
}

/// Réponse de la fiche sur les sauvegardes cloud d'un jeu : ce que le compte
/// garde maintenant, et ce que la dernière partie y a changé.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct CloudReport {
    pub saves: CloudSaves,
    /// Comparaison avec le relevé pris au lancement : présent une seule fois,
    /// quand un jeu vient de se terminer.
    pub diff: Option<CloudDiff>,
    /// Dossier du préfixe où le jeu lit ses sauvegardes, quand Lapinux sait
    /// le résoudre (chemin Windows déclaré par Steam).
    pub local_dir: Option<String>,
    /// Fichiers du cloud déjà présents à cet endroit.
    pub local_present: usize,
    /// Pourquoi la lecture n'a rien donné (session coupée, compte absent) :
    /// la carte des réglages le dit, plutôt que « aucune sauvegarde ».
    pub error: Option<String>,
    /// Journal local des synchronisations, la plus récente en premier. Il
    /// survit aux lectures : la fiche peut dire d'où vient le dernier envoi,
    /// même quand le cloud ne publie pas le nom de la machine d'origine.
    pub syncs: Vec<CloudSyncEntry>,
}

/// Une copie locale datée des sauvegardes d'un jeu : le filet quand la
/// boutique n'offre pas de cloud, et une version de plus quand elle en offre.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct SaveSnapshot {
    /// Nom du dossier de la copie (secondes Unix, en texte).
    pub id: String,
    /// Emplacement occupé, de 1 à 3. Zéro pour une copie qui précède le
    /// découpage en emplacements (elle est rangée à la première lecture).
    pub slot: u8,
    /// Date de la copie.
    pub created: i64,
    pub files: usize,
    pub bytes: u64,
    /// Dossier sauvegardé au moment de la copie.
    pub source: String,
    /// Dossier de la copie sur le disque : celui que la fiche ouvre dans le
    /// gestionnaire de fichiers.
    pub path: String,
}

/// Nature d'une ligne du journal des sauvegardes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SaveEventKind {
    /// Une copie datée est partie dans les emplacements du jeu.
    #[default]
    Backup,
    /// Un emplacement a remplacé les sauvegardes du jeu.
    Restore,
    /// Les copies ont été recopiées vers un dossier choisi.
    Export,
    /// Une copie a été retirée des emplacements.
    Delete,
}

/// Une opération du journal des sauvegardes d'un jeu : ce que la fiche
/// affiche sous les emplacements, du plus récent au plus ancien.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct SaveEvent {
    /// Date de l'opération (secondes Unix).
    pub at: i64,
    pub kind: SaveEventKind,
    /// Copie concernée, vide pour un export.
    pub snapshot: String,
    pub files: usize,
    pub bytes: u64,
    /// Dossier sauvegardé, ou destination de l'export.
    pub detail: String,
    /// Vrai quand la copie vient de la fermeture d'une partie, et non d'un
    /// clic dans la fiche.
    pub auto: bool,
    /// Vrai pour une copie antérieure au journal, reconstituée à l'affichage :
    /// aucune opération ne l'a enregistrée, mais elle existe toujours.
    pub imported: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum GameOrigin {
    /// Jeu découvert dans une bibliothèque gérée par Steam.
    #[default]
    Steam,
    /// Jeu installé par Lapinux dans `general.library_root`.
    Lapinux,
    /// Jeu installé par Lapinux depuis une boutique (Epic, GOG, itch.io).
    Store,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Game {
    pub appid: u32,
    pub name: String,
    pub installdir: String,
    /// Bibliothèque d'origine (Steam ou `general.library_root`).
    pub library: PathBuf,
    /// Auteur de l'installation : Steam, Lapinux ou une boutique.
    pub origin: GameOrigin,
    /// Boutique d'où vient le jeu ; Steam pour un jeu Steam.
    pub store: crate::stores::Store,
    /// Identifiant chez la boutique (`appName`, `product_id`, `game_id`).
    /// Vide pour un jeu Steam, dont `appid` suffit.
    pub store_id: String,
    pub size_on_disk: u64,
    pub state_flags: u32,
    pub last_updated: i64,
    pub build_id: String,
    pub installed: bool,
    pub prefix: Option<PathBuf>,
    pub exe: Option<PathBuf>,
    /// Jeu Linux natif (pas besoin de Proton).
    pub native: bool,
    pub drm: Drm,
    pub settings: GameSettings,
    pub playtime_secs: u64,
    pub last_played: Option<i64>,
    pub favorite: bool,
}

impl Default for Game {
    fn default() -> Self {
        Self {
            appid: 0,
            name: String::new(),
            installdir: String::new(),
            library: PathBuf::new(),
            origin: GameOrigin::Steam,
            store: crate::stores::Store::Steam,
            store_id: String::new(),
            size_on_disk: 0,
            state_flags: 0,
            last_updated: 0,
            build_id: String::new(),
            installed: false,
            prefix: None,
            exe: None,
            native: false,
            drm: Drm::Unknown,
            settings: GameSettings::default(),
            playtime_secs: 0,
            last_played: None,
            favorite: false,
        }
    }
}

impl Game {
    pub fn install_dir(&self) -> PathBuf {
        // Un jeu de boutique n'appartient pas à l'arborescence de Steam : son
        // dossier se lit depuis la racine de la bibliothèque, rangé sous le
        // nom de son launcher (`Epic Games/<jeu>`). `steamapps` reste le
        // domaine des jeux Steam.
        if self.origin == GameOrigin::Store && self.store != crate::stores::Store::Steam {
            return self.library.join(&self.installdir);
        }
        // Un jeu Steam possédé par Lapinux n'est pas dans `steamapps` : la
        // bibliothèque gérée ne montre qu'un dossier `Steam`, avec les dossiers
        // des jeux. Seule une vraie installation Steam garde `steamapps/common`.
        if self.origin == GameOrigin::Steam {
            return self
                .library
                .join("steamapps")
                .join("common")
                .join(&self.installdir);
        }
        self.library.join("Steam").join(&self.installdir)
    }

    /// Ce jeu utilise Steamworks ou SteamStub. Certaines protections peuvent
    /// empêcher le lancement direct ; Lapinux ne démarre pas le client Steam.
    pub fn requires_steam(&self) -> bool {
        matches!(self.drm, Drm::Steamworks | Drm::SteamStub)
    }
}

/// Code Steam Guard attendu par une connexion Lapinux en cours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SteamGuardKind {
    EmailCode,
    TwoFactor,
    DeviceConfirmation,
    EmailConfirmation,
}

/// Progression d'une connexion Steam ouverte par l'écran de connexion.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SteamLoginState {
    /// Une tentative est en cours dans le daemon.
    pub running: bool,
    pub connected: bool,
    /// « Lapinux » une fois la connexion réussie.
    pub backend: Option<String>,
    /// Étape lisible : connexion, code Steam Guard, confirmation, terminé.
    pub stage: String,
    /// Code attendu, si l'utilisateur doit en saisir un.
    pub guard: Option<SteamGuardKind>,
    pub guard_message: Option<String>,
    /// Le code précédent a été refusé : l'interface peut le signaler.
    pub guard_retry: bool,
    /// URL du QR code à scanner quand la connexion passe par l'application
    /// Steam ; remplacée à chaque renouvellement du défi.
    #[serde(default)]
    pub qr: Option<String>,
    pub error: Option<String>,
    pub message: Option<String>,
}

impl SteamLoginState {
    /// Vrai si l'utilisateur doit taper un code dans l'écran de connexion.
    pub fn waits_for_code(&self) -> bool {
        matches!(
            self.guard,
            Some(SteamGuardKind::EmailCode | SteamGuardKind::TwoFactor)
        )
    }

    /// Vrai si Steam attend une confirmation dans l'application ou par e-mail.
    pub fn waits_for_confirmation(&self) -> bool {
        matches!(
            self.guard,
            Some(SteamGuardKind::DeviceConfirmation | SteamGuardKind::EmailConfirmation)
        )
    }
}

/// Verdict de la vérification du jeton enregistré.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SteamTokenVerdict {
    /// Pas encore vérifié depuis le démarrage du daemon.
    #[default]
    Unknown,
    /// Steam a accepté le jeton : la reconnexion se fera seule.
    Valid,
    /// Steam a refusé le jeton : un nouveau QR code est nécessaire.
    Expired,
    /// Vérification en cours auprès de Steam.
    Checking,
    /// Steam a temporairement bloqué les tentatives (EResult 84) : le jeton
    /// n'est pas mort, il faut attendre avant de réessayer.
    RateLimited,
}

/// État du jeton de session enregistré, tel que l'écran de connexion l'affiche.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SteamTokenState {
    pub account_name: String,
    /// Date d'écriture du jeton, en secondes Unix.
    #[serde(default)]
    pub saved_at: Option<i64>,
    #[serde(default)]
    pub verdict: SteamTokenVerdict,
    /// Dernière vérification, en secondes Unix.
    #[serde(default)]
    pub checked_at: Option<i64>,
    /// Réponse de Steam quand le jeton a été refusé.
    #[serde(default)]
    pub detail: Option<String>,
}

/// État du compte Steam, sans jamais exposer de secret.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SteamStatus {
    pub username: String,
    pub connected: bool,
    /// Jeton enregistré : âge et verdict de la dernière vérification.
    #[serde(default)]
    pub token: Option<SteamTokenState>,
    /// Un jeton Lapinux est enregistré : la connexion se fera sans mot de passe.
    #[serde(default)]
    pub token_saved: bool,
    /// Identifiant Steam (SteamID64) de la session enregistrée : il ouvre le
    /// profil public et ses jeux les plus joués sans clé d'API.
    #[serde(default)]
    pub steam_id: Option<u64>,
    pub library_root: PathBuf,
    /// Dernière connexion ou tentative, telle que la voit l'écran de connexion.
    #[serde(default)]
    pub login: SteamLoginState,
    /// Proton que « Automatique » prendrait : les listes le nomment au lieu de
    /// laisser croire qu'elles ne choisissent rien.
    #[serde(default)]
    pub default_proton: Option<String>,
    /// Langue que le moteur prendrait sans choix explicite (celle du client
    /// Steam, sinon celle du bureau), en libellé.
    #[serde(default)]
    pub default_language: Option<String>,
}

/// Fichier de journal proposé dans Réglages → Logs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LogFile {
    pub name: String,
    pub size: u64,
    /// Dernière écriture, en secondes Unix.
    pub modified: i64,
}

/// Journaux disponibles et dernières lignes du fichier choisi.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LogsView {
    pub file: String,
    pub files: Vec<LogFile>,
    pub lines: Vec<String>,
}

/// Poids d'une plateforme, lu dans les manifestes publics des dépôts PICS :
/// `install` est la taille une fois décompressée, `download` celle du
/// téléchargement.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SteamSize {
    pub install: u64,
    pub download: u64,
}

impl SteamSize {
    pub fn is_zero(&self) -> bool {
        self.install == 0 && self.download == 0
    }
}

/// Fiche d'un jeu telle que Steam la publie (PICS) : tout ce que Lapinux
/// rapporte sans client Steam ni service tiers.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct SteamDetails {
    pub developers: Vec<String>,
    pub publishers: Vec<String>,
    pub genres: Vec<String>,
    pub categories: Vec<String>,
    /// Windows, macOS, Linux.
    pub platforms: Vec<String>,
    /// Date de sortie Steam, en secondes Unix.
    pub release_date: Option<i64>,
    /// Première sortie du jeu, si Steam la connaît.
    pub original_release_date: Option<i64>,
    /// `Complète`, `Partielle` ou vide.
    pub controller: String,
    pub metacritic: Option<u32>,
    pub metacritic_name: String,
    pub metacritic_url: String,
    /// Pourcentage d'avis positifs et note globale sur 10.
    pub review_percent: Option<u32>,
    pub review_score: Option<u32>,
    pub languages: Vec<String>,
    /// Langues avec doublage complet.
    pub full_audio: Vec<String>,
    /// Langue que Steam prendrait pour ce jeu (celle du client, sinon celle du
    /// bureau), en libellé : c'est elle qui choisit le dépôt de doublage et
    /// pèse dans le poids annoncé.
    #[serde(default)]
    pub default_language: String,
    pub free: bool,
    /// `Vérifié`, `Jouable`, `Non pris en charge` ou vide.
    pub deck: String,
    pub dlc_count: usize,
    /// Appids des DLC publiés pour le jeu.
    pub dlc_ids: Vec<u32>,
    /// Quota de sauvegardes cloud, en octets, et nombre de fichiers autorisés.
    pub cloud_quota: Option<u64>,
    pub cloud_files: Option<u32>,
    pub homepage: String,
    pub app_type: String,
    /// `64` pour les jeux 64 bits.
    pub os_arch: String,
    pub primary_genre: String,
    pub workshop: bool,
    /// Descripteurs de contenu mature (nudité, violence…).
    pub descriptors: Vec<String>,
    /// Build de la branche publique et date de publication, en secondes Unix.
    pub build_id: Option<u64>,
    pub build_time: Option<i64>,
    /// Branches de dépôts publiées (`public`, `beta`…).
    pub branches: Vec<String>,
    pub depot_count: usize,
    /// Poids par plateforme, sans les DLC ni les dépôts partagés.
    pub size_windows: Option<SteamSize>,
    pub size_linux: Option<SteamSize>,
    pub size_macos: Option<SteamSize>,
    /// Illustrations officielles servies par le CDN Steam.
    pub header_image: String,
    pub library_image: String,
    pub hero_image: String,
    pub logo_image: String,
    /// Bannière étroite, icône de bibliothèque et fond flouté.
    pub capsule_image: String,
    pub icon_image: String,
    pub background_image: String,
}

/// Contenu additionnel possédé, tel que PICS le publie : de quoi l'afficher
/// et calculer le poids d'une installation qui l'inclut.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SteamDlc {
    pub appid: u32,
    pub name: String,
    /// Octets à télécharger pour ses dépôts, nuls si Steam ne les publie pas.
    pub download: u64,
    /// Octets une fois installé.
    pub install: u64,
    /// Déjà sur le disque : son manifeste est présent dans la bibliothèque du
    /// jeu. Un contenu absent se télécharge, un contenu présent se vérifie.
    #[serde(default)]
    pub installed: bool,
}

impl SteamDetails {
    pub fn is_empty(&self) -> bool {
        self.developers.is_empty() && self.genres.is_empty() && self.release_date.is_none()
    }
}

/// Libellé lisible d'un type de manette Steam Input.
pub fn steam_input_label(kind: &str) -> &str {
    match kind {
        "controller_ps5" => "DualSense",
        "controller_ps5_edge" => "DualSense Edge",
        "controller_ps4" => "DualShock 4",
        "controller_xboxone" => "Manette Xbox",
        "controller_xbox360" => "Manette Xbox 360",
        "controller_switch_pro" => "Nintendo Switch Pro",
        "controller_switch_joycon_pair" => "Joy-Con",
        "controller_steamcontroller" => "Steam Controller",
        "controller_steamcontroller_gordon" => "Steam Controller",
        "controller_triton" => "Steam Controller 2026",
        "controller_neptune" => "Steam Deck",
        "controller_rog_ally" => "ROG Ally",
        "controller_mobile_touch" => "Tactile",
        "controller_generic" => "Manette générique",
        other => other,
    }
}

/// Identifiants matériels de la Steam Controller 2026 (USB, Bluetooth et récepteurs).
pub fn is_steam_controller_2026(vendor: u16, product: u16) -> bool {
    vendor == 0x28de && matches!(product, 0x1302..=0x1305)
}

/// Type Steam Input d'une manette, d'après son identifiant matériel.
///
/// Le pont de la Steam Controller 2026 annonce un pad Xbox 360 : l'appelant
/// remplace ce type par `controller_triton` quand `steam_bridge` est actif.
pub fn steam_input_kind(vendor: u16, product: u16) -> &'static str {
    match (vendor, product) {
        (0x054c, 0x0ce6) => "controller_ps5",
        (0x054c, 0x0df2) => "controller_ps5_edge",
        (0x054c, _) => "controller_ps4",
        (0x057e, 0x2009) => "controller_switch_pro",
        (0x057e, 0x2006 | 0x2007) => "controller_switch_joycon_pair",
        (0x28de, 0x1205 | 0x1206) => "controller_neptune",
        (vendor, product) if is_steam_controller_2026(vendor, product) => "controller_triton",
        (0x045e, 0x0b12 | 0x0b13 | 0x0b20 | 0x02fd | 0x02e0) => "controller_xboxone",
        (0x045e, 0x028e | 0x0291 | 0x02d1) => "controller_xbox360",
        (0x28de, _) => "controller_steamcontroller",
        _ => "controller_generic",
    }
}

/// Manette reconnue par Steam Input : le type Steam qu'elle prend et le nom
/// que le pilote lui donne.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SteamInputController {
    /// Type Steam Input (`controller_ps5`, `controller_xboxone`…).
    pub kind: String,
    /// Libellé lisible du type, tel que Steam le nomme.
    pub label: String,
    pub name: String,
    pub vendor: u16,
    pub product: u16,
    pub connected: bool,
}

/// Une ligne de réglage lue dans un profil Steam Input : zone morte d'un
/// stick, réglage de pavé, etc. La clé et la valeur restent celles du VDF.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SteamInputSetting {
    pub key: String,
    pub value: String,
}

/// Un binding d'une entrée Steam Input, avec l'activateur qui le déclenche.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SteamInputAction {
    /// Nom Steam de l'entrée (`button_a`, `click`, `dpad_north`…).
    pub input: String,
    /// Nom de l'activateur (`Full_Press`, `Soft_Press`, `Release_Press`…).
    pub activator: String,
    /// Sortie déclarée par Steam (`xinput_button A, , `, `key_press ENTER`…).
    pub binding: String,
}

/// Groupe d'actions d'un profil Steam Input, par mode de contrôle
/// (`four_buttons`, `joystick`, `trigger`, `absolute_mouse`…).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SteamInputActionGroup {
    /// Source Steam du groupe (`button_diamond`, `joystick`, `switch`…).
    pub source: String,
    /// Mode Steam du groupe.
    pub mode: String,
    pub actions: Vec<SteamInputAction>,
    /// Réglages propres au groupe (zones mortes, pavé tactile…).
    pub settings: Vec<SteamInputSetting>,
}

/// Profil Steam Input trouvé dans les dossiers du compte ou d'un jeu.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SteamInputProfile {
    /// Jeu auquel le profil est rattaché ; 0 quand le chemin n'en porte pas.
    pub appid: u32,
    pub title: String,
    pub description: String,
    /// Type de manette du profil (`controller_ps5`…).
    pub controller_type: String,
    pub creator: String,
    pub path: PathBuf,
    /// Identifiant de la configuration publiée sur Steam, quand elle vient de
    /// l'Atelier. Zéro pour une configuration purement locale.
    #[serde(default)]
    pub workshop_id: u64,
    /// Dernière écriture, en secondes Unix.
    pub modified: i64,
    /// « jeu », « compte » ou « partagé » : d'où vient le profil.
    pub origin: String,
    /// Sélectionné pour ce jeu dans Lapinux ou dans les `configset` importés de Steam.
    #[serde(default)]
    pub active: bool,
    /// Nom du preset Steam appliqué (`Default`, `Menu`…).
    #[serde(default)]
    pub preset: String,
    /// Le fichier a été lu et les actions ont été extraites.
    #[serde(default)]
    pub details_loaded: bool,
    /// Erreur de lecture du détail ; vide quand le fichier n'a simplement pas
    /// été chargé pour ce profil.
    #[serde(default)]
    pub details_error: String,
    #[serde(default)]
    pub action_groups: Vec<SteamInputActionGroup>,
    /// Réglages globaux du profil (zones mortes, options de pavé…).
    #[serde(default)]
    pub settings: Vec<SteamInputSetting>,
}

/// Profils Steam Input proposés pour les manettes branchées.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SteamInputReport {
    pub controllers: Vec<SteamInputController>,
    pub profiles: Vec<SteamInputProfile>,
    /// Des dossiers Steam Input ont été trouvés.
    pub available: bool,
    pub notes: Vec<String>,
}

impl SteamInputReport {
    /// Profils qui visent l'une des manettes présentes. Sans manette
    /// reconnue, tous les profils restent proposés.
    pub fn for_connected(&self) -> Vec<&SteamInputProfile> {
        if self.controllers.is_empty() {
            return self.profiles.iter().collect();
        }
        self.profiles
            .iter()
            .filter(|profile| {
                self.controllers
                    .iter()
                    .any(|controller| controller.kind == profile.controller_type)
            })
            .collect()
    }
}

/// Configuration Steam Input publiée sur Steam, mais pas nécessairement
/// présente sur le disque de l'utilisateur.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SteamInputCommunityProfile {
    /// Identifiant du fichier publié (Atelier/CDN Steam).
    pub id: u64,
    pub appid: u32,
    pub title: String,
    pub description: String,
    pub controller_type: String,
    /// Identifiant Steam de l'auteur, tel que publié par Steam.
    pub creator: String,
    /// URL du `.vdf` sur le CDN de Steam.
    pub download_url: String,
    pub file_size: u64,
    /// Dernière mise à jour, en secondes Unix.
    pub modified: i64,
    /// Note Steam, de 0 à 1.
    pub rating: f32,
    pub votes_up: u64,
    pub votes_down: u64,
    pub subscriptions: u64,
    /// Une copie a déjà été téléchargée par Lapinux.
    pub installed: bool,
}

/// Copie d'une configuration Steam Input que Lapinux garde pour lui-même.
///
/// Le fichier `.vdf` est recopié sous `steaminput/saved/<appid>/`, avec ses
/// métadonnées : la fiche Steam peut être relue ou le jeu désinstallé, la
/// configuration reste consultable et testable dans Manettes.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SteamInputSaved {
    /// Identifiant de la configuration publiée d'origine.
    pub id: u64,
    /// Jeu auquel Lapinux l'a appliquée.
    pub appid: u32,
    /// Jeu d'où vient la configuration (différent en cas de repli).
    pub source_appid: u32,
    pub title: String,
    pub description: String,
    pub controller_type: String,
    pub creator: String,
    pub modified: i64,
    /// Date de la copie dans Lapinux, en secondes Unix.
    pub saved_at: i64,
    /// Chemin de la copie `.vdf` dans le dossier de données de Lapinux.
    pub path: PathBuf,
    /// Nom du preset Steam appliqué (`Default`, `Menu`…).
    #[serde(default)]
    pub preset: String,
    #[serde(default)]
    pub details_loaded: bool,
    #[serde(default)]
    pub details_error: String,
    #[serde(default)]
    pub action_groups: Vec<SteamInputActionGroup>,
    #[serde(default)]
    pub settings: Vec<SteamInputSetting>,
}

/// Résultat d'une recherche dans le catalogue communautaire Steam.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SteamInputCommunityReport {
    pub profiles: Vec<SteamInputCommunityProfile>,
    /// Nombre total de configurations trouvées par Steam.
    pub total: usize,
    pub page: usize,
    pub per_page: usize,
    /// Aucun résultat pour ce jeu : la liste vient d'autres jeux et reste
    /// applicable à celui-ci, comme le permet Steam Input.
    pub fallback: bool,
    pub notes: Vec<String>,
}

/// Abonnement à l'Atelier tel qu'il est installé sur le disque.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct WorkshopItem {
    pub published_file_id: u64,
    pub title: String,
    /// Vignette officielle publiée par Steam, vide si Steam ne la fournit pas.
    #[serde(default)]
    pub preview_url: String,
    /// Poids publié par Steam pour ce contenu, en octets.
    pub size: u64,
    /// Dernière mise à jour, en secondes Unix.
    pub time_updated: i64,
    pub manifest: String,
    /// Contenu téléchargé, quand il est présent sur le disque.
    pub path: PathBuf,
    pub installed: bool,
    /// Désactivé dans le rangement Lapinux (`Mods`) : le contenu reste sur le
    /// disque, mais aucun lien actif ne le présente au jeu.
    pub disabled: bool,
    pub url: String,
}

/// Atelier d'un jeu : nos abonnements, leur poids et le dossier de rangement.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct WorkshopReport {
    pub appid: u32,
    pub name: String,
    /// Le jeu publie un atelier : un manifeste ou du contenu existe.
    pub supported: bool,
    pub items: Vec<WorkshopItem>,
    /// Poids de tous les abonnements.
    pub total_size: u64,
    /// Poids des abonnements présents sur le disque.
    pub installed_size: u64,
    pub workshop_url: String,
    /// Dossier `LapinuxLibrary/Mods/<jeu> (<appid>)/SteamWorkshop`.
    pub mods_dir: PathBuf,
    /// Dossiers de fichiers réels présents dans ce dossier.
    pub links: usize,
    pub notes: Vec<String>,
}

/// Résultat du rangement de l'Atelier dans `Mods`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct WorkshopArrangeReport {
    pub dir: PathBuf,
    /// Dossiers matérialisés après le rangement.
    pub links: usize,
    pub removed: usize,
    /// Octets couverts par les liens.
    pub bytes: u64,
}

/// Action demandée sur un ou plusieurs contenus de l'Atelier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkshopAction {
    Enable,
    Disable,
    Uninstall,
}

/// Résultat d'une activation, d'une désactivation ou d'une suppression.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct WorkshopManageReport {
    /// Dossiers actifs restants dans `SteamWorkshop`.
    pub links: usize,
    /// Entrées retirées par l'opération.
    pub removed_links: usize,
    /// Contenus téléchargés supprimés.
    pub removed_items: usize,
    /// Octets libérés par les suppressions.
    pub freed_bytes: u64,
    /// Contenus qui n'ont pas pu être supprimés.
    pub errors: Vec<String>,
}

/// Résultat d'un téléchargement de l'Atelier effectué sans le client Steam,
/// par DepotDownloader.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct WorkshopDownloadReport {
    pub appid: u32,
    /// Contenus effectivement récupérés et placés dans la bibliothèque.
    pub downloaded: Vec<u64>,
    /// Poids de ces contenus sur le disque.
    pub bytes: u64,
    /// Dossiers actifs après le rangement automatique.
    pub links: usize,
    /// Journal complet de DepotDownloader.
    pub log: PathBuf,
    pub errors: Vec<String>,
}

/// Fiche complète d'un jeu, relue à la demande : ce que le **compte** y a joué
/// (session Steam), ce que le **protocole** publie (PICS : build, dépôts,
/// branches) et ce que la **boutique** affiche (description, captures,
/// exigences).
///
/// Chaque source est optionnelle : une source muette laisse une note dans
/// [`notes`](Self::notes) et n'empêche pas les autres d'être lues.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SteamGameInfo {
    pub appid: u32,
    pub name: String,
    /// Relecture de la fiche, en secondes Unix.
    pub fetched: i64,
    /// Sources qui ont répondu : « compte », « protocole », « boutique ».
    pub sources: Vec<String>,
    /// Ce qui n'a pas pu être lu, et pourquoi.
    pub notes: Vec<String>,
    pub account: SteamAccountPlay,
    pub update: SteamUpdateInfo,
    pub store: SteamStorePage,
    /// Statistiques communautaires publiées par le jeu, avec la valeur du compte.
    pub stats: Vec<SteamStat>,
}

impl SteamGameInfo {
    /// La fiche a-t-elle au moins une source lue ?
    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }
}

/// Ce que le compte a fait du jeu : temps de jeu, succès, badge, joueurs.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SteamAccountPlay {
    /// Le compte possède le jeu (licence, jeu gratuit ou famille).
    pub owned: bool,
    /// Minutes jouées, telles que Steam les compte.
    pub minutes_total: Option<u64>,
    /// Minutes des deux dernières semaines.
    pub minutes_2weeks: Option<u64>,
    pub minutes_windows: Option<u64>,
    pub minutes_linux: Option<u64>,
    pub minutes_macos: Option<u64>,
    /// Minutes jouées sur un Steam Deck.
    pub minutes_deck: Option<u64>,
    /// Dernière partie, en secondes Unix.
    pub last_played: Option<i64>,
    /// Niveau du compte Steam et niveau du badge du jeu, lus par le protocole.
    pub level: Option<u32>,
    pub badge_level: Option<u32>,
    /// Joueurs connectés au moment de la lecture.
    pub players_now: Option<u64>,
    pub achievements_unlocked: Option<u32>,
    pub achievements_total: Option<u32>,
    /// Dernier succès débloqué, avec sa date.
    pub last_achievement: String,
    pub last_achievement_time: Option<i64>,
    /// Succès les plus rares, les moins obtenus d'abord.
    pub rarest: Vec<SteamRarity>,
}

/// Un succès et la part de joueurs qui l'ont obtenu.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SteamRarity {
    pub name: String,
    /// Pourcentage de joueurs qui le possèdent.
    pub percent: f32,
    pub unlocked: bool,
}

/// Ce que le protocole publie du build : mises à jour, dépôts, branches.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SteamUpdateInfo {
    /// `game`, `dlc`, `demo`, `tool`, `music`…
    pub app_type: String,
    /// `released`, `prerelease`…
    pub release_state: String,
    /// Build de la branche publique, et sa date de publication.
    pub public_build: Option<u64>,
    pub public_build_time: Option<i64>,
    /// Build réellement installé sur le disque, et sa date.
    pub installed_build: Option<u64>,
    pub installed_build_time: Option<i64>,
    /// Branches publiées, `public` comprise.
    pub branches: Vec<SteamBranch>,
    /// Dépôts du build public.
    pub depots: Vec<SteamDepot>,
    /// Poids du build public : à télécharger, puis une fois installé.
    pub download: u64,
    pub install: u64,
    /// Sauvegardes cloud : quota en octets et nombre de fichiers.
    pub cloud_quota: Option<u64>,
    pub cloud_files: Option<u32>,
    /// Classifications officielles (PEGI, ESRB, USK…).
    pub ratings: Vec<String>,
    /// Contenus additionnels publiés par le jeu.
    pub dlc: Vec<SteamDlcRef>,
}

/// Une branche publiée d'un jeu (`public`, `beta`, `previous`…).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct SteamBranch {
    pub name: String,
    pub build: Option<u64>,
    pub time: Option<i64>,
    pub description: String,
}

/// Un dépôt publié : ce qu'il pèse et à qui il sert.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct SteamDepot {
    pub id: u32,
    /// Systèmes desservis (`windows`, `linux`, `macos`), vide pour tous.
    pub os: Vec<String>,
    /// Langue du dépôt quand il est propre à un doublage.
    pub language: String,
    pub download: u64,
    pub install: u64,
    /// Dépôt partagé par plusieurs applications.
    pub shared: bool,
    /// Contenu additionnel dont ce dépôt dépend, s'il n'appartient pas au jeu.
    pub dlc_appid: Option<u32>,
}

/// Un contenu additionnel publié, avec son nom quand Steam le donne.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct SteamDlcRef {
    pub appid: u32,
    pub name: String,
    /// Le compte possède ce contenu.
    pub owned: bool,
    /// Octets à télécharger pour ses dépôts, nuls si Steam ne les publie pas.
    pub download: u64,
    /// Octets une fois installé.
    pub install: u64,
    /// Chemin de sa bannière dans les visuels de la boutique
    /// (`<hash>/header.jpg`), vide quand Steam n'en publie pas.
    pub image: String,
}

/// Un morceau de la description d'une boutique : le texte ou un visuel, dans
/// l'ordre où la page les publie.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum DescriptionBlock {
    Text(String),
    Media(String),
}

/// Ce que la page publique de la boutique raconte du jeu.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct SteamStorePage {
    pub short_description: String,
    /// Description longue, débarrassée de son balisage.
    pub description: String,
    /// La description dans son ordre de lecture : texte, image, texte…
    pub description_blocks: Vec<DescriptionBlock>,
    pub header_image: String,
    pub screenshots: Vec<String>,
    /// Bande-annonce publiée par la boutique (mp4 ou webm), vide s'il n'y en a pas.
    pub trailer: String,
    /// Affiche de la bande-annonce, quand la boutique en publie une.
    pub trailer_poster: String,
    /// Visuels que la boutique intercale dans la description : ils
    /// accompagnent le texte du jeu, et sont rendus avec lui.
    pub description_images: Vec<String>,
    /// Prix affiché (`9,99 €`, `Gratuit`…).
    pub price: String,
    /// Remise en pourcentage.
    pub discount: u32,
    /// Nombre d'avis publiés.
    pub recommendations: Option<u64>,
    /// Résumé des avis (`Très positive`, `Mitigées`…).
    pub review_summary: String,
    pub review_count: Option<u64>,
    pub website: String,
    pub support_url: String,
    pub support_email: String,
    pub legal_notice: String,
    pub drm_notice: String,
    /// Configuration minimale et recommandée, telles que la boutique les publie.
    pub requirements_min: String,
    pub requirements_recommended: String,
    pub genres: Vec<String>,
    pub categories: Vec<String>,
    /// Les mêmes fonctionnalités, avec l'identifiant que Steam leur donne : la
    /// fiche leur associe un pictogramme.
    pub features: Vec<SteamFeature>,
    /// Annonces publiées par l'éditeur : notes de mise à jour et actualités.
    pub news: Vec<SteamNews>,
    /// Descripteurs de contenu de la boutique.
    pub descriptors: Vec<String>,
    /// Date de sortie telle qu'elle est écrite (`12 mars 2024`).
    pub release: String,
}

/// Une fonctionnalité annoncée par la boutique : solo, cloud, partage
/// familial, manettes, atelier…
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct SteamFeature {
    /// Identifiant de la catégorie Steam (`2` = solo, `23` = Steam Cloud…).
    pub id: u32,
    /// Libellé tel que la boutique le publie, en français.
    pub label: String,
}

/// Une annonce de l'éditeur, telle que le flux public de Steam la sert.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct SteamNews {
    pub title: String,
    /// Début du texte, balisage retiré.
    pub excerpt: String,
    /// Texte entier de l'annonce, balisage retiré : la fiche le lit sans
    /// ouvrir le navigateur.
    pub body: String,
    /// Publication, en secondes Unix.
    pub date: i64,
    /// Auteur de la publication, quand Steam le donne.
    pub author: String,
    /// Adresse publique de l'annonce complète.
    pub url: String,
    /// Visuel de l'annonce (`og:image` de sa page communautaire), vide quand
    /// Steam n'en publie pas.
    pub image: String,
}

/// Une statistique que le jeu publie, et sa valeur pour ce compte.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct SteamStat {
    /// Nom interne du protocole (`total_kills`).
    pub name: String,
    /// Nom lisible (`Éliminations totales`).
    pub display: String,
    /// Valeur du compte, telle que Steam la rend.
    pub value: String,
}

/// Succès d'un jeu tels que la **session** Steam les publie.
///
/// Le schéma du jeu (`Player.GetGameAchievements`) donne le nom, la
/// description, l'icône et le caractère caché ; `ClientGetUserStats` donne la
/// date de déblocage de chaque succès pour le compte connecté.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SteamAchievements {
    pub appid: u32,
    /// Lecture, en secondes Unix.
    pub fetched: i64,
    /// Succès dans l'ordre du jeu.
    pub items: Vec<SteamAchievement>,
    /// Minutes jouées par le compte, lues dans la même session (l'embed du
    /// jeu les affiche sans attendre la fiche complète).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub play_minutes: Option<u64>,
    /// Ce qui n'a pas pu être lu, et pourquoi.
    pub notes: Vec<String>,
}

impl SteamAchievements {
    pub fn unlocked(&self) -> usize {
        self.items.iter().filter(|item| item.achieved).count()
    }
}

/// Un succès du jeu et l'état du compte pour celui-ci.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SteamAchievement {
    /// Nom interne du protocole (`GMA_PLAY_SINGLEPLAYER`).
    pub api_name: String,
    pub name: String,
    pub description: String,
    /// Icône officielle, servie par le CDN Steam.
    pub icon: String,
    /// Succès caché par le jeu : son nom ne se dévoile qu'une fois obtenu.
    pub hidden: bool,
    pub achieved: bool,
    /// Date de déblocage, en secondes Unix. Nulle pour un succès verrouillé.
    pub unlock_time: Option<i64>,
    /// Part des joueurs qui l'ont obtenu, en pourcentage. Le schéma du
    /// protocole la publie avec le succès.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percent: Option<f32>,
}

/// Un succès débloqué, prêt pour le bandeau qui se pose en haut de l'écran :
/// ce que le daemon annonce et ce que le processus de l'overlay peint.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AchievementToast {
    pub title: String,
    /// Nom du jeu, rappelé au bord droit de l'en-tête.
    pub game: String,
    /// Boutique d'où vient le succès : sa marque colore le bandeau.
    pub store: crate::stores::Store,
    /// Part des joueurs qui l'ont obtenu, en pourcentage. Absente quand la
    /// boutique ne la publie pas : le bandeau ne montre alors aucune rareté.
    pub percent: Option<f32>,
    /// Icône déjà téléchargée dans le cache : le processus du bandeau n'a pas
    /// de réseau, il ne lit que des fichiers.
    pub icon: Option<PathBuf>,
    /// Icône du jeu, posée à gauche de son nom dans l'en-tête.
    pub game_icon: Option<PathBuf>,
}

/// Un jeu possédé sur Steam, installé ou non dans la bibliothèque Lapinux.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SteamCatalogGame {
    pub appid: u32,
    pub name: String,
    pub installed: bool,
    /// Jeu prêté par un autre membre de la famille Steam.
    #[serde(default)]
    pub family: bool,
    /// Détails lus dans PICS lors de la synchronisation du catalogue.
    #[serde(default)]
    pub details: SteamDetails,
    /// Contenus additionnels **possédés** que le jeu déclare, avec leur poids.
    #[serde(default)]
    pub dlc: Vec<SteamDlc>,
    /// Date d'ajout de la licence du jeu au compte, en secondes Unix.
    #[serde(default)]
    pub acquired: Option<i64>,
    /// Pourquoi le jeu reste sans date, quand elle manque : `family` (prêté
    /// par la famille), `free` (gratuit) ou `unpublished` (aucune licence
    /// datée publiée par Steam). Rempli par la synchronisation.
    #[serde(default)]
    pub undated_reason: Option<String>,
    /// Minutes jouées selon Steam, tous systèmes confondus.
    #[serde(default)]
    pub minutes: Option<u64>,
}

/// Contenu additionnel du groupe familial, avec les membres qui le possèdent.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SteamFamilyDlc {
    pub appid: u32,
    pub name: String,
    #[serde(default)]
    pub owners: Vec<u64>,
}

/// Catalogue du compte Steam connecté (issu de `licenses_print`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SteamCatalog {
    pub account: String,
    pub updated: i64,
    pub entries: Vec<SteamCatalogGame>,
    /// Contenus additionnels partagés par le groupe, avec leur propriétaire.
    #[serde(default)]
    pub family_dlcs: Vec<SteamFamilyDlc>,
    /// Une synchronisation tourne en arrière-plan : l'interface peut sonder.
    #[serde(default)]
    pub syncing: bool,
    /// Dernière erreur de synchronisation, effacée au succès suivant.
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProtonInfo {
    pub name: String,
    pub path: PathBuf,
    /// "steam", "compatibilitytools", "lapinux" ou "umu".
    pub source: String,
    pub version: String,
    /// Exécutable `proton` présent.
    pub runnable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProtonRelease {
    pub repo: String,
    pub tag: String,
    pub asset: String,
    pub asset_url: String,
    pub size: u64,
    pub checksum_url: Option<String>,
    pub published: String,
    pub prerelease: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobKind {
    InstallGame,
    InstallProton,
    InstallGraphics,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobState {
    Queued,
    Running,
    Paused,
    Done,
    Failed,
    /// Arrêtée par l'utilisateur : la carte est retirée de la liste et les
    /// fichiers partiels d'une installation neuve sont supprimés.
    Cancelled,
    /// Arrêtée par un redémarrage de l'application : rien n'a été effacé, les
    /// fichiers reçus restent sur le disque et la carte propose de reprendre.
    /// C'est le seul état « en attente de reprise » qui s'affiche dans la
    /// section « En pause », jamais dans « Terminées ».
    Interrupted,
}

impl JobState {
    pub fn label(self) -> &'static str {
        match self {
            JobState::Queued => "en attente",
            JobState::Running => "en cours",
            JobState::Paused => "en pause",
            JobState::Done => "terminé",
            JobState::Failed => "échec",
            JobState::Cancelled => "annulé",
            JobState::Interrupted => "interrompu",
        }
    }
}

/// Bilan d'un téléchargement terminé, calculé pendant le transfert.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct JobReport {
    /// Durée murale, pauses comprises.
    pub elapsed_secs: u64,
    /// Temps réellement passé à recevoir des octets, pauses exclues.
    pub active_secs: u64,
    /// Débit moyen pondéré par le temps actif.
    pub average_bps: u64,
    pub peak_bps: u64,
    pub low_bps: u64,
    pub network: u64,
    pub written: u64,
    /// Nombre de fenêtres de mesure (environ une par seconde).
    pub samples: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Job {
    pub id: String,
    pub kind: JobKind,
    /// Étiquette complète : « Nom du jeu (appid) ».
    pub label: String,
    /// Nom seul, sans appid : titre de la carte de téléchargement.
    #[serde(default)]
    pub title: String,
    pub appid: Option<u32>,
    pub state: JobState,
    /// 0.0 à 1.0 (peut rester à 0 si la progression est inconnue).
    pub progress: f32,
    pub downloaded: u64,
    pub total: u64,
    /// Octets réellement reçus du réseau (chunks CDN).
    #[serde(default)]
    pub network: u64,
    /// Octets réellement écrits sur le disque.
    #[serde(default)]
    pub written: u64,
    /// Hôte CDN ou serveur qui sert le téléchargement.
    #[serde(default)]
    pub server: String,
    /// Dépôt Steam en cours (1-based) et nombre de dépôts retenus.
    #[serde(default)]
    pub depot: usize,
    #[serde(default)]
    pub depots: usize,
    /// Remplace une installation existante au lieu d'en créer une.
    #[serde(default)]
    pub updating: bool,
    /// Contenus additionnels demandés avec le jeu : la file d'attente les
    /// conserve pour que la reprise installe la même chose.
    #[serde(default)]
    pub dlc: Vec<u32>,
    /// Version retenue dans le menu du bouton (`linux`, `windows`, `macos`) :
    /// la file d'attente la conserve, sinon une tâche relancée depuis la file
    /// reprendrait la préférence du moteur au lieu du choix affiché.
    #[serde(default)]
    pub platform: Option<String>,
    pub message: String,
    pub backend: String,
    pub started: i64,
    pub finished: Option<i64>,
    /// Présent une fois la tâche terminée : rapport de débit et de durée.
    #[serde(default)]
    pub report: Option<JobReport>,
}

/// Cadence réelle des images pendant une partie, relevée sur la fenêtre du
/// jeu. `current` ne vaut que pour la session en cours ; l'historique garde
/// les trois bornes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FpsStats {
    pub current: u32,
    pub min: u32,
    pub avg: u32,
    pub max: u32,
}

impl FpsStats {
    /// Rien de mesuré : la carte n'affiche pas une ligne vide.
    pub fn is_empty(&self) -> bool {
        self.max == 0
    }

    /// « 45 min · 78 moy · 120 max », dans l'ordre où on les lit.
    pub fn summary(&self) -> String {
        if self.is_empty() {
            return String::new();
        }
        format!("{} min · {} moy · {} max", self.min, self.avg, self.max)
    }
}

/// Une partie terminée : ce que la page Activité raconte, une carte par
/// partie. Le démarrage et la fin viennent de la session elle-même, la durée
/// en découle — une partie interrompue garde ce qu'elle a duré.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PlaySession {
    pub appid: u32,
    pub name: String,
    /// Secondes Unix de début et de fin.
    pub started: i64,
    pub ended: i64,
    pub seconds: u64,
    /// Journal de la session, sur le disque : la page Activité peut le
    /// déplier et le copier après coup.
    #[serde(default)]
    pub log: String,
    /// FPS relevés pendant la partie ; absent pour les parties antérieures à
    /// la mesure, ou quand GameScope n'a rien publié.
    #[serde(default)]
    pub fps: Option<FpsStats>,
    /// Courbe de la cadence, réduite à 120 points : de quoi dessiner le
    /// graphique de la carte sans garder une mesure par seconde.
    #[serde(default)]
    pub fps_series: Vec<u32>,
    /// Instant du premier et du dernier point de la courbe : la carte peut
    /// donc nommer l'heure sous le curseur.
    #[serde(default)]
    pub fps_series_start: i64,
    #[serde(default)]
    pub fps_series_end: i64,
}

/// État de l'enregistrement en cours, tel que l'overlay l'affiche.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecordingState {
    /// Pause demandée par l'utilisateur. Le menu rapide suspend aussi la
    /// capture, mais cette suspension temporaire n'est pas comptée ici.
    pub paused: bool,
    /// Secondes réellement enregistrées, pauses déduites.
    pub secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionInfo {
    pub appid: u32,
    pub name: String,
    pub pid: u32,
    pub started: i64,
    pub proton: String,
    pub fps_cap: u32,
    /// Fréquence de l'écran visé, et non l'argument `-r` de GameScope :
    /// celui-ci tourne au-dessus de l'écran pour laisser la place à
    /// « illimité » et aux changements de limite à chaud.
    #[serde(default)]
    pub refresh_hz: u32,
    pub overlay_open: bool,
    pub input_profile: String,
    pub log_path: PathBuf,
    /// Le jeu a affiché sa première fenêtre : l'écran d'attente peut
    /// s'effacer. Sans suivi disponible (pas de KWin), il vaut `true` pour
    /// garder l'ancien comportement plutôt que de couvrir l'écran pour rien.
    #[serde(default)]
    pub window_shown: bool,
    /// Le jeu a réellement commencé à dessiner. Sous gamescope, sa fenêtre
    /// existe avant la première image : c'est le journal de gamescope qui
    /// donne le vrai départ du rendu.
    #[serde(default)]
    pub render_started: bool,
    /// La fenêtre du jeu s'est fermée : l'écran de fin peut se poser tout
    /// de suite, même si Proton met encore quelques secondes à sortir. La
    /// session, elle, attend la sortie réelle du processus pour sauvegarder.
    #[serde(default)]
    pub window_gone: bool,
    /// La session passe par gamescope : sa fenêtre n'est qu'une enveloppe
    /// noire tant que le jeu n'a rien présenté.
    #[serde(default)]
    pub gamescope: bool,
    /// La session est enveloppée dans `lapinux scene` : le lancement et
    /// l'écran de fin sont dessinés dans GameScope, pas par l'interface.
    #[serde(default)]
    pub scene: bool,
    /// Mode performance du jeu : la session a coupé la scène, l'enregistrement
    /// automatique et les HUD. L'enregistrement manuel reste possible.
    #[serde(default)]
    pub performance: bool,
    /// Image publique du jeu, pour l'icône de l'activité Discord.
    #[serde(default)]
    pub cover: String,
    /// Application Discord officielle du jeu : Discord prend son nom et son
    /// icône pour l'activité, au lieu de ceux de Lapinux. Vide quand le jeu
    /// n'est pas reconnu par le catalogue Discord.
    #[serde(default)]
    pub discord_app: String,
    /// Cadence des images relevée pendant la partie, mise à jour chaque
    /// seconde. Absente tant que rien n'a été mesuré.
    #[serde(default)]
    pub fps: Option<FpsStats>,
    /// Courbe de la cadence en cours de partie, pour la carte d'activité.
    #[serde(default)]
    pub fps_series: Vec<u32>,
    /// Bornes de la courbe, en secondes Unix : l'infobulle date chaque point.
    #[serde(default)]
    pub fps_series_start: i64,
    #[serde(default)]
    pub fps_series_end: i64,
    /// Boutique d'où vient le jeu : la petite vignette Discord porte sa
    /// marque.
    #[serde(default)]
    pub store: crate::stores::Store,
    /// Ce que le jeu embarqué sait faire en DLSS, détecté dans ses fichiers.
    #[serde(default)]
    pub dlss: Option<DlssInfo>,
    /// Multiplicateur de génération d'images de la session, quand la couche
    /// afmf-linux est chargée : 0 coupe la génération, ×2, ×3 et ×4
    /// intercalent des images.
    #[serde(default)]
    pub afmf_multiplier: Option<u32>,
    /// Fichier de contrôle à chaud de cette session, écrit par le daemon et
    /// lu par la couche : le multiplicateur y change sans relancer le jeu.
    #[serde(default)]
    pub afmf_control: Option<PathBuf>,
}

/// Ce que le dossier d'un jeu révèle de son moteur et de ses technologies :
/// une lecture de ses fichiers, jamais un lancement du jeu.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameTech {
    #[serde(default)]
    pub engine: Option<GameEngine>,
    /// Suréchantillonneurs embarqués, hors DLSS (voir `dlss`).
    #[serde(default)]
    pub upscalers: Vec<Upscaler>,
    /// Un jeu peut embarquer deux anti-triches (campagne et multijoueur).
    #[serde(default)]
    pub anti_cheat: Vec<AntiCheat>,
    #[serde(default)]
    pub dlss: DlssInfo,
}

impl GameTech {
    /// « DLSS · images générées · FSR · XeSS », ou « — » sans suréchantillonneur.
    pub fn upscaling(&self) -> String {
        let mut parts = Vec::new();
        if !self.dlss.is_empty() {
            parts.push(self.dlss.short());
        }
        parts.extend(
            self.upscalers
                .iter()
                .map(|upscaler| upscaler.label().to_string()),
        );
        if parts.is_empty() {
            "—".into()
        } else {
            parts.join(" · ")
        }
    }

    /// Vrai quand un anti-triche protège le jeu : son processus refuse les
    /// bibliothèques et les outils qui s'y injectent.
    pub fn protected(&self) -> bool {
        !self.anti_cheat.is_empty()
    }

    /// « Easy Anti-Cheat · BattlEye », ou « — » sans anti-triche.
    pub fn anti_cheat(&self) -> String {
        if self.anti_cheat.is_empty() {
            return "—".into();
        }
        self.anti_cheat
            .iter()
            .map(|anti_cheat| anti_cheat.label())
            .collect::<Vec<_>>()
            .join(" · ")
    }
}

/// Moteur reconnu d'après les fichiers du jeu : les noms propres ne se
/// traduisent pas, et un moteur inconnu reste muet plutôt qu'inventé.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum GameEngine {
    Unreal,
    Unity,
    Godot,
    Source,
    GameMaker,
    RenPy,
    RpgMaker,
}

impl GameEngine {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Unreal => "Unreal Engine",
            Self::Unity => "Unity",
            Self::Godot => "Godot",
            Self::Source => "Source",
            Self::GameMaker => "GameMaker",
            Self::RenPy => "Ren'Py",
            Self::RpgMaker => "RPG Maker",
        }
    }
}

/// Suréchantillonneurs concurrents de DLSS, repérés par leurs bibliothèques.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Upscaler {
    Fsr,
    Xess,
}

impl Upscaler {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Fsr => "FSR",
            Self::Xess => "XeSS",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum AntiCheat {
    EasyAntiCheat,
    BattlEye,
}

impl AntiCheat {
    pub fn label(&self) -> &'static str {
        match self {
            Self::EasyAntiCheat => "Easy Anti-Cheat",
            Self::BattlEye => "BattlEye",
        }
    }
}

/// Une bibliothèque DLSS trouvée sur le disque, avec sa version lue dans la
/// ressource de la DLL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DlssLibrary {
    /// `nvngx_dlss.dll`, `nvngx_dlssg.dll` ou `nvngx_dlssd.dll`.
    pub name: String,
    pub version: Option<[u16; 4]>,
    pub path: std::path::PathBuf,
    /// Jeu d'où vient la bibliothèque, quand elle vient d'ailleurs.
    #[serde(default)]
    pub source: String,
}

impl DlssLibrary {
    /// « 3.7.10 », ou « version inconnue » quand la ressource manque.
    pub fn version_label(&self) -> String {
        let Some(version) = self.version else {
            return "version inconnue".into();
        };
        if version[3] == 0 {
            format!("{}.{}.{}", version[0], version[1], version[2])
        } else {
            format!(
                "{}.{}.{}.{}",
                version[0], version[1], version[2], version[3]
            )
        }
    }
}

/// Bibliothèques DLSS d'un jeu, et versions plus récentes trouvées ailleurs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DlssState {
    /// Une entrée par nom de DLL, la plus ancienne copie du jeu.
    pub current: Vec<DlssLibrary>,
    /// Une entrée par nom de DLL qui peut recevoir une version plus récente.
    pub available: Vec<DlssLibrary>,
    /// Vrai quand une sauvegarde existe : le jeu tourne sur d'autres DLL.
    pub updated: bool,
    /// Vrai quand un anti-triche interdit de remplacer les DLL tant que le
    /// joueur n'a pas autorisé les injections de ce jeu.
    #[serde(default)]
    pub blocked: bool,
}

/// Bibliothèques DLSS trouvées dans le dossier du jeu. Lapinux ne les active
/// pas lui-même : Proton s'en charge quand le jeu les charge.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DlssInfo {
    /// `nvngx_dlss.dll` : le jeu propose le suréchantillonnage DLSS.
    pub dlss: bool,
    /// `nvngx_dlssg.dll` : génération d'images (DLSS 3).
    pub frame_generation: bool,
    /// `nvngx_dlssd.dll` : reconstruction de rayons (DLSS 3.5).
    pub ray_reconstruction: bool,
    /// Versions PE des DLL, lorsqu'une ressource de version est lisible.
    #[serde(default)]
    pub dlss_version: Option<[u16; 4]>,
    #[serde(default)]
    pub frame_generation_version: Option<[u16; 4]>,
    #[serde(default)]
    pub ray_reconstruction_version: Option<[u16; 4]>,
}

impl DlssInfo {
    pub fn is_empty(&self) -> bool {
        !self.dlss && !self.frame_generation && !self.ray_reconstruction
    }

    fn version(version: Option<[u16; 4]>) -> String {
        version
            .map(|v| format!(" {}.{}.{}.{}", v[0], v[1], v[2], v[3]))
            .unwrap_or_default()
    }

    /// Indique les fonctions repérées, sans déduire une version des noms des DLL.
    pub fn short(&self) -> String {
        if self.is_empty() {
            return "—".into();
        }
        if self.frame_generation || self.ray_reconstruction {
            let mut parts = vec![format!("DLSS{}", Self::version(self.dlss_version))];
            if self.frame_generation {
                parts.push(format!(
                    "FG{}",
                    Self::version(self.frame_generation_version)
                ));
            }
            if self.ray_reconstruction {
                parts.push(format!(
                    "RR{}",
                    Self::version(self.ray_reconstruction_version)
                ));
            }
            parts.join(" · ")
        } else {
            format!("DLSS{}", Self::version(self.dlss_version))
        }
    }

    /// Indique les fonctions repérées, sans affirmer qu'elles sont actives.
    pub fn summary(&self) -> String {
        if self.is_empty() {
            return "non détecté".into();
        }
        let mut parts = Vec::new();
        parts.push(format!("DLSS{}", Self::version(self.dlss_version)));
        if self.frame_generation {
            parts.push(format!(
                "images générées{}",
                Self::version(self.frame_generation_version)
            ));
        }
        if self.ray_reconstruction {
            parts.push(format!(
                "rayons{}",
                Self::version(self.ray_reconstruction_version)
            ));
        }
        parts.join(" · ")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheckStatus {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Check {
    pub name: String,
    pub status: CheckStatus,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct DoctorReport {
    pub checks: Vec<Check>,
}

impl DoctorReport {
    /// Rapport partageable : seules les catégories connues et leur état sont
    /// copiés. Les détails libres peuvent contenir chemins ou données privées.
    pub fn private_summary(&self) -> serde_json::Value {
        const CHECKS: &[(&str, &str)] = &[
            ("Steam", "steam_library"), ("Bibliothèque", "library"),
            ("umu-run", "umu"), ("GameScope", "gamescope"),
            ("Pad virtuel", "virtual_controller"),
            ("Raccourci clavier", "keyboard_shortcut"), ("Manettes", "controllers"),
            ("Session", "desktop_session"),
        ];
        let mut checks = serde_json::Map::new();
        for &(name, key) in CHECKS {
            let state = self.checks.iter().find(|check| check.name == name).map(|check| match check.status {
                CheckStatus::Ok => "ok", CheckStatus::Warn => "warning", CheckStatus::Fail => "error",
            }).unwrap_or("unknown");
            checks.insert(key.to_string(), state.into());
        }
        serde_json::Value::Object(checks)
    }

    pub fn worst(&self) -> CheckStatus {
        if self.checks.iter().any(|c| c.status == CheckStatus::Fail) {
            CheckStatus::Fail
        } else if self.checks.iter().any(|c| c.status == CheckStatus::Warn) {
            CheckStatus::Warn
        } else {
            CheckStatus::Ok
        }
    }
}

/// Bus par lequel une manette est connectée.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DeviceLink {
    #[default]
    Unknown,
    /// Câble USB, ou récepteur USB non identifié.
    Usb,
    Bluetooth,
    /// Récepteur 2,4 GHz branché en USB (dongle propriétaire).
    Dongle,
    /// Pad créé par uinput (Steam Input, test, couche maison).
    Virtual,
}

impl DeviceLink {
    pub fn label(self) -> &'static str {
        match self {
            DeviceLink::Unknown => crate::i18n::t("controllers.link.unknown"),
            DeviceLink::Usb => crate::i18n::t("controllers.link.usb"),
            DeviceLink::Bluetooth => crate::i18n::t("controllers.link.bluetooth"),
            DeviceLink::Dongle => crate::i18n::t("controllers.link.dongle"),
            DeviceLink::Virtual => crate::i18n::t("controllers.link.virtual"),
        }
    }
}

fn connected_by_default() -> bool {
    true
}

/// Champs lus dans la base udev (`/run/udev/data`) : ils portent les vrais
/// noms du matériel, même quand un clone réutilise un identifiant USB connu.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct UdevIdentity {
    pub vendor: Option<String>,
    pub model: Option<String>,
    pub serial: Option<String>,
}

impl UdevIdentity {
    pub fn is_empty(&self) -> bool {
        self.vendor.is_none() && self.model.is_none() && self.serial.is_none()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeviceInfo {
    /// Vide pour une manette appairée mais absente d'evdev.
    pub path: PathBuf,
    pub name: String,
    pub vendor: u16,
    pub product: u16,
    pub gamepad: bool,
    pub buttons: usize,
    /// Libellés evdev de tous les boutons exposés, même au repos.
    #[serde(default)]
    pub button_names: Vec<String>,
    pub axes: Vec<String>,
    /// Le périphérique expose des effets de retour de force.
    #[serde(default)]
    pub force_feedback: bool,
    /// Le périphérique expose un pavé tactile dans ses axes evdev.
    #[serde(default)]
    pub touchpad: bool,
    /// Bus détecté dans sysfs ; inconnu pour les périphériques virtuels.
    #[serde(default)]
    pub connection: DeviceLink,
    /// Pourcentage de batterie, quand le pilote l'expose.
    #[serde(default)]
    pub battery: Option<u8>,
    /// État de charge brut du noyau (`Charging`, `Discharging`, `Full`…).
    #[serde(default)]
    pub battery_status: Option<String>,
    /// Adresse Bluetooth, quand elle est connue.
    #[serde(default)]
    pub address: Option<String>,
    /// Une entrée sans chemin evdev est appairée mais hors ligne.
    #[serde(default = "connected_by_default")]
    pub connected: bool,
    /// Pad virtuel du pont `steam-puck-bridge` : il s'annonce en Xbox 360
    /// (`045e:028e`) alors que la manette est une Steam Controller 2026.
    #[serde(default)]
    pub steam_bridge: bool,
    /// Noms matériels fournis par udev, quand ils existent.
    #[serde(default)]
    pub udev: UdevIdentity,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProfileSummary {
    pub name: String,
    pub path: PathBuf,
    pub is_default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Stats {
    pub launches: u64,
    pub total_seconds: u64,
}

/// Un serveur de téléchargement proposé pour une boutique.
///
/// `id` est ce qui se range dans les réglages (hôte Steam, famille de CDN
/// Epic), `label` ce que la liste affiche et `detail` la précision utile
/// (groupe, charge, pays).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct DownloadServer {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub detail: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_game_capture_overrides_win_and_defaults_restore_the_global() {
        let mut global = crate::config::CaptureConfig::default();
        global.record_on_launch = true;
        global.stop_on_exit = true;
        global.video_fps = 30;
        global.video_profile = "balanced".into();
        global.video_resolution = "1080p".into();
        global.video_encoder = "auto".into();
        global.video_codec = "h264".into();
        global.record_audio = true;
        global.record_microphone = false;
        global.hud_on_screen = true;
        global.record_badge = true;

        let mut game = GameSettings::default();
        game.record_on_launch = Some(false);
        game.stop_on_exit = Some(false);
        game.video_fps = Some(60);
        game.video_profile = Some("quality".into());
        game.video_resolution = Some("1440p".into());
        game.video_encoder = Some("nvenc".into());
        game.video_codec = Some("av1".into());
        game.record_audio = Some(false);
        game.record_microphone = Some(true);
        game.hud_on_screen = Some(false);
        game.record_badge = Some(false);

        let effective = game.effective_capture(&global);
        assert!(!effective.record_on_launch);
        assert!(!effective.stop_on_exit);
        assert_eq!(effective.video_fps, 60);
        assert_eq!(effective.video_profile, "quality");
        assert_eq!(effective.video_resolution, "1440p");
        assert_eq!(effective.video_encoder, "nvenc");
        assert_eq!(effective.video_codec, "av1");
        assert!(!effective.record_audio);
        assert!(effective.record_microphone);
        assert!(!effective.hud_on_screen);
        assert!(!effective.record_badge);

        game.record_on_launch = None;
        game.stop_on_exit = None;
        game.video_fps = None;
        game.video_profile = None;
        game.video_resolution = None;
        game.video_encoder = None;
        game.video_codec = None;
        game.record_audio = None;
        game.record_microphone = None;
        game.hud_on_screen = None;
        game.record_badge = None;
        assert_eq!(
            game.effective_capture(&global),
            global,
            "retirer chaque override rétablit la valeur globale"
        );

        let inherited = GameSettings::default().effective_capture(&global);
        assert_eq!(
            inherited, global,
            "le retour au défaut reprend toute la config globale"
        );
    }

    #[test]
    fn per_game_display_and_save_overrides_are_persistent() {
        let mut game = GameSettings::default();
        game.sharpness = Some(11);
        game.hud_on_screen = Some(false);
        game.record_badge = Some(false);
        game.saves_auto = Some(false);
        game.saves_skip_cloud = Some(true);
        game.saves_relaunch = Some(true);
        game.grab_devices = Some(false);
        assert_eq!(game.sharpness(3), 11);
        assert!(!game.hud_on_screen(true));
        assert!(!game.record_badge(true));
        assert!(!game.saves_auto(true));
        assert!(game.saves_skip_cloud(false));
        assert!(game.saves_relaunch(false));
        assert!(!game.grab_devices(true));

        let wire = serde_json::to_string(&game).unwrap();
        let read: GameSettings = serde_json::from_str(&wire).unwrap();
        assert_eq!(read, game, "les overrides survivent à un redémarrage");
        let default = GameSettings::default();
        assert_eq!(default.sharpness(3), 3);
        assert!(default.saves_auto(true));
        assert!(!default.saves_skip_cloud(false));
    }

    #[test]
    fn steam_controller_2026_ids_are_detected_without_a_bridge() {
        for product in 0x1302..=0x1305 {
            assert!(is_steam_controller_2026(0x28de, product));
            assert_eq!(steam_input_kind(0x28de, product), "controller_triton");
        }
        assert_eq!(
            steam_input_label("controller_triton"),
            "Steam Controller 2026"
        );
        assert!(!is_steam_controller_2026(0x28de, 0x1142));
        assert!(!is_steam_controller_2026(0x045e, 0x1304));
    }

    #[test]
    fn game_install_dir() {
        let mut g = Game::default();
        g.library = PathBuf::from("/mnt/Jeux/Steam");
        g.installdir = "Team Fortress 2".into();
        assert_eq!(
            g.install_dir(),
            PathBuf::from("/mnt/Jeux/Steam/steamapps/common/Team Fortress 2")
        );
    }

    /// Fichier cloud de test : nom, taille et date.
    fn cloud_file(name: &str, size: u64, modified: i64) -> CloudFile {
        CloudFile {
            name: name.into(),
            size,
            modified,
            ..Default::default()
        }
    }

    #[test]
    fn a_session_says_which_cloud_files_it_touched() {
        let before = CloudSaves {
            files: vec![
                cloud_file("slot1.sav", 100, 1_000),
                cloud_file("slot2.sav", 200, 1_000),
                cloud_file("old.sav", 50, 900),
            ],
            ..Default::default()
        };
        let after = CloudSaves {
            files: vec![
                cloud_file("slot1.sav", 100, 1_000),
                cloud_file("slot2.sav", 260, 1_600),
                cloud_file("slot3.sav", 300, 1_600),
            ],
            ..Default::default()
        };
        let diff = cloud_diff(&before, &after);
        assert_eq!(diff.added, vec![cloud_file("slot3.sav", 300, 1_600)]);
        assert_eq!(diff.changed, vec![cloud_file("slot2.sav", 260, 1_600)]);
        assert_eq!(diff.removed, vec!["old.sav".to_string()]);
        assert!(!diff.is_empty());
        assert_eq!(diff.summary(), "1 ajouté(s) · 1 modifié(s) · 1 retiré(s)");
    }

    #[test]
    fn the_latest_cloud_and_local_files_are_found() {
        let mut local = cloud_file("slot2.sav", 200, 1_600);
        local.local = Some(1_700);
        let state = CloudSaves {
            files: vec![cloud_file("slot1.sav", 100, 1_000), local],
            ..Default::default()
        };
        assert_eq!(state.latest_file().unwrap().name, "slot2.sav");
        assert_eq!(state.last_modified(), Some(1_600));
        let (file, local) = state.latest_local_file().unwrap();
        assert_eq!(file.name, "slot2.sav");
        assert_eq!(local, 1_700);
    }

    #[test]
    fn a_cloud_without_dates_has_no_latest_file() {
        let state = CloudSaves {
            files: vec![cloud_file("slot1.sav", 100, 0)],
            ..Default::default()
        };
        assert!(state.latest_file().is_none());
        assert!(state.latest_local_file().is_none());
    }

    #[test]
    fn cloud_sync_is_on_unless_it_is_turned_off() {
        // Les jeux enregistrés avant le réglage n'en ont pas : ils restent
        // synchronisés, comme le réglage global de reprise après installation.
        let settings = GameSettings::default();
        assert!(settings.cloud_sync_enabled());
        let played: GameSettings = serde_json::from_str(r#"{"cloud_sync":false}"#).unwrap();
        assert!(!played.cloud_sync_enabled());
    }

    #[test]
    fn a_session_that_saved_nothing_leaves_the_cloud_still() {
        let state = CloudSaves {
            files: vec![cloud_file("slot1.sav", 100, 1_000)],
            ..Default::default()
        };
        let diff = cloud_diff(&state, &state);
        assert!(diff.is_empty());
        assert_eq!(diff.summary(), "aucun changement dans le cloud");
    }

    #[test]
    fn steamworks_and_steamstub_require_the_client() {
        let mut g = Game::default();
        g.drm = Drm::Steamworks;
        assert!(g.requires_steam());
        g.drm = Drm::SteamStub;
        assert!(g.requires_steam());
        g.drm = Drm::None;
        assert!(!g.requires_steam());
    }

    #[test]
    fn game_serde_defaults() {
        let g: Game = serde_json::from_str("{\"appid\":10,\"name\":\"X\"}").unwrap();
        assert_eq!(g.drm, Drm::Unknown);
        assert_eq!(g.settings.env.len(), 0);
        assert_eq!(g.origin, GameOrigin::Steam);
    }
}
