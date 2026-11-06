//! État du quick menu overlay, indépendant du rendu et testable.
//!
//! Le panneau n'a plus d'onglets : la capsule occupe le haut du menu, et
//! « Performances » ou les « Enregistrements » ouvrent un panneau dédié depuis
//! leur ligne. Ces panneaux portent un bouton Retour.

use lapinux_core::social::{SteamChatMessage, SteamFriend, SteamFriendsState};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Menu,
    Performance,
    Recordings,
    /// Liste complète des profils de manettes, ouverte depuis le menu.
    Profiles,
    /// Fenêtre compacte des amis Steam, ouverte depuis le carré du pied de
    /// page. Chaque ami ouvre une conversation de la session courante.
    Invite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    Resume,
    Performance,
    Recordings,
    /// Volume du flux audio du jeu, réglé dans le menu rapide.
    Volume,
    Profile,
    Proton,
    Screenshot,
    QuitGame,
}

pub const ITEMS: [Item; 8] = [
    Item::Resume,
    Item::Performance,
    Item::Recordings,
    Item::Volume,
    Item::Profile,
    Item::Proton,
    Item::Screenshot,
    Item::QuitGame,
];

/// Profils MangoHud proposés en jeu : le premier est le plus léger, le dernier
/// ajoute VRAM, fréquences et puissance. Les trois portent le nom du GPU et du
/// CPU ainsi que les températures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HudPreset {
    Simple,
    #[default]
    Basic,
    Detailed,
}

impl HudPreset {
    pub const ALL: [HudPreset; 3] = [HudPreset::Simple, HudPreset::Basic, HudPreset::Detailed];

    pub fn label(self) -> &'static str {
        match self {
            HudPreset::Simple => "Simple",
            HudPreset::Basic => "Basique",
            HudPreset::Detailed => "Détaillée",
        }
    }

    /// Forme envoyée au daemon, et lue dans les réglages du jeu. `essential`
    /// et `full` restent les noms historiques des deux profils complets.
    pub fn wire(self) -> &'static str {
        match self {
            HudPreset::Simple => "simple",
            HudPreset::Basic => "essential",
            HudPreset::Detailed => "full",
        }
    }

    pub fn from_wire(text: &str) -> Self {
        match text.trim().to_ascii_lowercase().as_str() {
            "simple" => HudPreset::Simple,
            "essential" | "basic" => HudPreset::Basic,
            "full" | "detailed" => HudPreset::Detailed,
            _ => HudPreset::Basic,
        }
    }

    pub fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|preset| *preset == self)
            .unwrap_or(1)
    }

    pub fn cycle(self, delta: i32) -> Self {
        let next = (self.index() as i32 + delta).rem_euclid(Self::ALL.len() as i32) as usize;
        Self::ALL[next]
    }
}

/// Les neuf ancrages proposés par MangoHud, dans l'ordre de lecture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HudPosition {
    #[default]
    TopLeft,
    TopCenter,
    TopRight,
    MiddleLeft,
    MiddleCenter,
    MiddleRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
}

impl HudPosition {
    pub const ALL: [HudPosition; 9] = [
        HudPosition::TopLeft,
        HudPosition::TopCenter,
        HudPosition::TopRight,
        HudPosition::MiddleLeft,
        HudPosition::MiddleCenter,
        HudPosition::MiddleRight,
        HudPosition::BottomLeft,
        HudPosition::BottomCenter,
        HudPosition::BottomRight,
    ];

    pub fn label(self) -> &'static str {
        match self {
            HudPosition::TopLeft => "Haut gauche",
            HudPosition::TopCenter => "Haut centre",
            HudPosition::TopRight => "Haut droite",
            HudPosition::MiddleLeft => "Milieu gauche",
            HudPosition::MiddleCenter => "Centre",
            HudPosition::MiddleRight => "Milieu droite",
            HudPosition::BottomLeft => "Bas gauche",
            HudPosition::BottomCenter => "Bas centre",
            HudPosition::BottomRight => "Bas droite",
        }
    }

    pub fn wire(self) -> &'static str {
        match self {
            HudPosition::TopLeft => "top-left",
            HudPosition::TopCenter => "top-center",
            HudPosition::TopRight => "top-right",
            HudPosition::MiddleLeft => "middle-left",
            HudPosition::MiddleCenter => "middle-center",
            HudPosition::MiddleRight => "middle-right",
            HudPosition::BottomLeft => "bottom-left",
            HudPosition::BottomCenter => "bottom-center",
            HudPosition::BottomRight => "bottom-right",
        }
    }

    pub fn from_wire(text: &str) -> Self {
        let wanted = text.trim().to_ascii_lowercase();
        Self::ALL
            .iter()
            .copied()
            .find(|position| position.wire() == wanted)
            .unwrap_or(HudPosition::TopLeft)
    }

    pub fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|position| *position == self)
            .unwrap_or(0)
    }

    pub fn cycle(self, delta: i32) -> Self {
        let next = (self.index() as i32 + delta).rem_euclid(Self::ALL.len() as i32) as usize;
        Self::ALL[next]
    }
}

/// Paliers de la limite de FPS. Le premier palier utile est 15 i/s ; le
/// dernier (0) coupe le limiteur et laisse la session monter jusqu'à la
/// cadence de l'écran.
///
/// GameScope divise la limite dans la cadence de son compositeur, qui prend le
/// plus petit multiple de la limite strictement supérieur à l'écran (au moins
/// 2×) : chaque palier tombe sur un diviseur entier, donc la limite affichée
/// est celle qui tourne vraiment.
pub const FPS_STEPS: [u32; 11] = [15, 30, 45, 60, 90, 120, 144, 165, 240, 360, 0];

/// Marge de la piste de cadence, depuis le bord gauche de la carte (le
/// pictogramme occupe les 44 premiers pixels).
pub const FPS_TRACK_LEFT: f32 = 44.0;
/// Marge de la piste depuis le bord droit de la carte.
pub const FPS_TRACK_RIGHT: f32 = 14.0;

/// Moteur du HUD : la plaque dessinée par Lapinux ou le HUD de MangoHud.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HudEngine {
    /// Plaque dessinée par Lapinux, dans une fenêtre séparée : elle n'entre
    /// jamais dans la vidéo.
    #[default]
    Native,
    /// HUD dessiné par MangoHud dans l'image du jeu.
    MangoHud,
}

impl HudEngine {
    pub fn from_wire(value: &str) -> Self {
        match value {
            "mangohud" | "mango" => Self::MangoHud,
            _ => Self::Native,
        }
    }
}

/// Disposition du HUD MangoHud : bloc vertical classique, ou une seule ligne
/// horizontale quand l'écran est chargé.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HudLayout {
    #[default]
    Block,
    Inline,
}

impl HudLayout {
    pub const ALL: [HudLayout; 2] = [HudLayout::Block, HudLayout::Inline];

    pub fn label(self) -> &'static str {
        match self {
            HudLayout::Block => "En bloc",
            HudLayout::Inline => "En ligne",
        }
    }

    pub fn wire(self) -> &'static str {
        match self {
            HudLayout::Block => "block",
            HudLayout::Inline => "inline",
        }
    }

    pub fn from_wire(text: &str) -> Self {
        match text.trim().to_ascii_lowercase().as_str() {
            "inline" | "line" | "horizontal" | "ligne" => HudLayout::Inline,
            _ => HudLayout::Block,
        }
    }

    pub fn cycle(self, delta: i32) -> Self {
        match (self, delta.signum()) {
            (HudLayout::Block, 1) | (HudLayout::Inline, -1) => HudLayout::Inline,
            (HudLayout::Inline, 1) | (HudLayout::Block, -1) => HudLayout::Block,
            _ => self,
        }
    }
}

/// Pictogramme porté par une ligne du menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Play,
    Gauge,
    Screen,
    Chip,
    Gamepad,
    Flask,
    Camera,
    Film,
    Speaker,
    Cross,
    Friend,
}

pub fn icon(item: Item) -> Icon {
    match item {
        Item::Resume => Icon::Play,
        Item::Performance => Icon::Screen,
        Item::Recordings => Icon::Film,
        Item::Volume => Icon::Speaker,
        Item::Profile => Icon::Gamepad,
        Item::Proton => Icon::Flask,
        Item::Screenshot => Icon::Camera,
        Item::QuitGame => Icon::Cross,
    }
}

/// Hauteur de l'en-tête du menu (capsule et bandeau des statistiques).
pub const HEADER_HEIGHT: f32 = LIVE_TOP + LIVE_HEIGHT + 10.0;
pub const ROW_HEIGHT: f32 = 50.0;
pub const FOOTER_HEIGHT: f32 = 50.0;
/// Espace du titre au-dessus de la liste des profils.
pub const PROFILE_TOP: f32 = 42.0;
/// Nombre d'amis visibles dans la page d'amis ; le reste défile.
pub const INVITE_ROWS: usize = 7;
/// Haut du titre de la page d'amis, sous le bouton Retour.
pub const INVITE_TITLE_TOP: f32 = 72.0;
/// Haut de la première ligne d'amis, après l'en-tête compact.
pub const INVITE_TOP: f32 = 136.0;
/// Hauteur du champ de saisie, au-dessus du pied de page de la conversation.
pub const CHAT_COMPOSER_HEIGHT: f32 = 52.0;
/// Longueur maximale d'un message, comme le protocole Steam.
pub const CHAT_MESSAGE_MAX: usize = 1024;
/// Fenêtre compacte des amis : elle flotte au-dessus du jeu au lieu d'occuper
/// tout l'écran.
pub const FRIENDS_WIDTH: f32 = 680.0;
pub const FRIENDS_HEIGHT: f32 = 620.0;
/// Carré d'accès aux amis, dans le pied du panneau principal.
pub const FRIENDS_BUTTON_SIZE: f32 = 34.0;
/// Durée de l'ouverture : le panneau entre depuis le bord gauche.
pub const OPEN_SECONDS: f32 = 0.28;
/// Durée de la fermeture, plus courte pour ne pas retarder le retour au jeu.
pub const CLOSE_SECONDS: f32 = 0.18;
/// Marge intérieure du panneau.
pub const PADDING: f32 = 20.0;
/// Hauteur de la carte capsule : elle occupe tout le haut du menu, dans le
/// rapport 16:9 de la largeur utile.
pub const CAPSULE_HEIGHT: f32 = 270.0;
/// Plus d'onglets : la capsule commence directement sous la marge.
pub const TABS_HEIGHT: f32 = 0.0;
pub const TAB_GAP: f32 = 0.0;
/// Haut de la carte capsule, au sommet du panneau.
pub const CAPSULE_TOP: f32 = PADDING;
/// Bandeau des statistiques, sous la capsule.
pub const LIVE_TOP: f32 = CAPSULE_TOP + CAPSULE_HEIGHT + 8.0;
pub const LIVE_HEIGHT: f32 = 26.0;

/// Bouton Retour du panneau Performances, en haut.
pub const PERF_BACK_TOP: f32 = PADDING;
pub const PERF_BACK_HEIGHT: f32 = 38.0;
/// Haut de la première ligne réglable du panneau Performances, sous Retour.
pub const PERF_TOP: f32 = PERF_BACK_TOP + PERF_BACK_HEIGHT + 8.0;
/// Hauteur d'une ligne du panneau Performances.
pub const PERF_ROW: f32 = 44.0;
/// Titre de section (« Profil MangoHud », « Position »).
pub const PERF_SECTION: f32 = 22.0;
/// Grille des neuf ancrages.
pub const PERF_GRID: f32 = 96.0;
/// Bandeau du détail DLSS, en bas du panneau Performances.
pub const PERF_INFO: f32 = 34.0;
/// Lignes réglables du panneau Performances.
pub const PERF_ROWS: usize = 11;
/// Largeur d'un bouton du sélecteur Natif / MangoHud, et l'écart entre eux.
pub const PERF_ENGINE_W: f32 = 92.0;
pub const PERF_ENGINE_GAP: f32 = 6.0;
/// Bandeau d'état de la page « Enregistrements », au-dessus des vidéos.
pub const RECORDING_STATUS_HEIGHT: f32 = 46.0;
/// Haut de la première ligne du HUD d'enregistrement, sous son bandeau d'état.
pub const RECORDINGS_TOP: f32 = PERF_TOP + RECORDING_STATUS_HEIGHT;
/// Lignes de la page « Enregistrements » : pause, arrêt, puis les trois
/// réglages du badge.
pub const RECORD_ROWS: usize = 5;

/// Cadence et durée lues sur la session en cours, rafraîchies par l'overlay.
/// Une vidéo de la page « Enregistrements » : son nom, sa date et son poids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recording {
    pub name: String,
    pub modified: i64,
    pub bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Live {
    pub fps: Option<u32>,
    pub session_secs: Option<u64>,
    /// Cadence des dernières secondes, de la plus ancienne à la plus récente.
    pub series: Vec<u32>,
    /// Bibliothèques DLSS du jeu en cours, quand il en a.
    pub dlss: Option<lapinux_core::model::DlssInfo>,
    /// Un HUD est actif : les réglages MangoHud s'appliquent à chaud.
    pub hud_live: bool,
    /// La session tourne sous GameScope : la limite de FPS peut changer à chaud.
    pub gamescope: bool,
    /// Enregistrement en cours : pause et durée déjà captée.
    pub recording: Option<lapinux_core::model::RecordingState>,
    /// Volume actuel du flux audio du jeu, quand le daemon le connaît.
    pub volume: Option<f32>,
}

/// La cadence tient-elle la limite fixée pour la session ?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FpsStatus {
    Good,
    Low,
    Unknown,
}

impl Live {
    pub fn fps_status(&self, cap: u32) -> FpsStatus {
        match self.fps {
            None => FpsStatus::Unknown,
            // Aucune limite : la cadence ne peut pas être « en retard ».
            Some(_) if cap == 0 => FpsStatus::Good,
            // 10 % de marge : une mesure instantanée oscille autour de la
            // limite, et GameScope arrondit la cadence au diviseur inférieur.
            Some(fps) if fps as f32 >= cap as f32 * 0.90 => FpsStatus::Good,
            Some(_) => FpsStatus::Low,
        }
    }

    pub fn fps_label(&self) -> String {
        match self.fps {
            Some(fps) => format!("{fps} FPS"),
            None => "— FPS".into(),
        }
    }

    pub fn session_label(&self) -> String {
        match self.session_secs {
            Some(seconds) => format!("Session {}", lapinux_core::util::format_duration(seconds)),
            None => "Session —".into(),
        }
    }

    /// Bandeau de la capture. Le temps affiché est celui qui est réellement
    /// dans la vidéo : le menu ouvert, lui, n'enregistre rien.
    pub fn recording_label(&self) -> Option<String> {
        let state = self.recording?;
        let clock = format_clock(state.secs);
        Some(if state.paused {
            format!("REC pause · {clock}")
        } else {
            format!("REC {clock}")
        })
    }
}

/// Chronomètre de la capture : `mm:ss`, ou `h:mm:ss` au-delà d'une heure.
fn format_clock(seconds: u64) -> String {
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let rest = seconds % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{rest:02}")
    } else {
        format!("{minutes:02}:{rest:02}")
    }
}

impl Action {
    /// Forme envoyée au daemon par l'overlay.
    pub fn wire(&self) -> String {
        match self {
            Action::Resume => "resume",
            Action::SetFpsCap(value) => return format!("fps_cap:{value}"),
            Action::Hud {
                enabled,
                profile,
                position,
                layout,
                graph,
                cores,
            } => {
                return format!(
                    "hud:{}:{}:{}:{}:{}:{}",
                    u8::from(*enabled),
                    profile.wire(),
                    position.wire(),
                    layout.wire(),
                    u8::from(*graph),
                    u8::from(*cores)
                )
            }
            Action::HudEngine { mangohud } => {
                return format!(
                    "hud_engine:{}",
                    if *mangohud { "mangohud" } else { "native" }
                )
            }
            Action::HudScale(scale) => return format!("hud_scale:{scale:.2}"),
            Action::HudVisible(show) => return format!("hud_visible:{}", u8::from(*show)),
            Action::RecordBadge {
                enabled,
                position,
                scale,
            } => {
                return format!(
                    "record_badge:{}:{}:{scale:.2}",
                    u8::from(*enabled),
                    position.wire()
                )
            }
            Action::RecordingStart => "recording_start",
            Action::RecordingPause { paused } => {
                return format!("recording_pause:{}", u8::from(*paused))
            }
            Action::RecordingStop => "recording_stop",
            Action::GameVolume(volume) => return format!("game_volume:{volume:.2}"),
            Action::SelectProfile(name) => return format!("profile_select:{name}"),
            Action::SendMessage { steam_id, text } => {
                return format!("message:{steam_id}:{text}")
            }
            Action::CycleProfile => "profile_next",
            Action::CycleProton => "proton_next",
            Action::Screenshot => "screenshot",
            Action::CutHud => "hud_cut",
            Action::QuitGame => "quit_game",
        }
        .to_string()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Resume,
    SetFpsCap(u32),
    Hud {
        enabled: bool,
        profile: HudPreset,
        position: HudPosition,
        layout: HudLayout,
        graph: bool,
        /// Charge de chaque cœur du processeur (`core_load`).
        cores: bool,
    },
    /// Change le moteur du HUD pour les prochaines parties.
    HudEngine { mangohud: bool },
    /// Taille de la plaque native, appliquée à chaud.
    HudScale(f32),
    /// Affiche ou retire la plaque native de performance.
    HudVisible(bool),
    /// Réglages du badge d'enregistrement, indépendants de la plaque.
    RecordBadge {
        enabled: bool,
        position: HudPosition,
        scale: f32,
    },
    /// Démarre une capture depuis la page Enregistrements.
    RecordingStart,
    /// Suspend ou reprend la capture en cours, sans la terminer.
    RecordingPause { paused: bool },
    /// Termine la capture et garde la vidéo.
    RecordingStop,
    /// Volume du flux audio du jeu, entre 0 et 1.
    GameVolume(f32),
    CycleProfile,
    /// Choix exact d'un profil dans la liste : la partie adopte ce profil
    /// tout de suite, et le jeu le garde pour la prochaine fois.
    SelectProfile(String),
    /// Message Steam envoyé à un ami depuis sa conversation.
    SendMessage { steam_id: u64, text: String },
    CycleProton,
    Screenshot,
    /// Coupe tous les HUD de performance : MangoHud et la plaque Lapinux.
    CutHud,
    QuitGame,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MenuState {
    pub game_name: String,
    pub proton: String,
    pub profile: String,
    /// Tous les profils enregistrés, donnés par le daemon (`--profiles`).
    pub profiles: Vec<String>,
    /// Ligne sélectionnée dans la page « Profils manettes ».
    pub profile_selected: usize,
    pub fps_cap: u32,
    /// Profil MangoHud retenu, même quand le HUD est coupé.
    pub hud: HudPreset,
    /// MangoHud affiché ou non.
    pub hud_on: bool,
    /// Moteur choisi pour le HUD : plaque native ou MangoHud.
    pub hud_engine: HudEngine,
    /// Taille de la plaque native, en multiples de sa taille de base.
    pub perf_scale: f32,
    /// Ancrage du HUD à l'écran.
    pub hud_position: HudPosition,
    /// Bloc vertical ou une seule ligne.
    pub hud_layout: HudLayout,
    /// Courbe de frametime MangoHud, indépendante du profil.
    pub hud_graph: bool,
    /// Utilisation de chaque cœur du processeur.
    pub hud_cores: bool,
    /// Volume du flux audio du jeu, entre 0 et 1.
    pub volume: f32,
    /// Curseur de volume en cours de glissement : un relevé PipeWire plus
    /// ancien ne doit pas le faire reculer sous le pointeur.
    pub volume_dragging: bool,
    /// Fin de la courte période où un relevé en retard est ignoré, le temps
    /// que la dernière valeur envoyée atteigne le flux.
    pub volume_hold_until: Option<Instant>,
    /// Dossier des vidéos, donné par le dæmon (`--captures`).
    pub recordings_dir: Option<std::path::PathBuf>,
    /// Jeu dont la page « Enregistrements » liste les vidéos.
    pub appid: u32,
    /// Vidéos du jeu, de la plus récente à la plus ancienne.
    pub recordings: Vec<Recording>,
    pub recording_selected: usize,
    /// Amis, avatars et conversations fournis par le daemon.
    pub invite: SteamFriendsState,
    /// Ami sélectionné dans la page d'amis.
    pub invite_selected: usize,
    /// Conversation ouverte, par SteamID ; `None` sur la liste d'amis.
    pub chat_friend: Option<u64>,
    /// Texte en cours de saisie dans la conversation.
    pub chat_input: String,
    pub page: Page,
    pub selected: usize,
    /// Ligne sélectionnée dans le panneau Performances.
    pub perf_selected: usize,
    /// Progression d'ouverture 0..1 (animation).
    pub anim: f32,
    /// Impulsion de sélection 0..1 (animation courte).
    pub pulse: f32,
    /// Cadence et durée de la partie, lues sur le daemon.
    pub live: Live,
    /// Plaque de performance affichée hors du panneau, pendant la partie.
    pub perf_hud: bool,
    /// Surface dédiée au HUD seul. Le processus du menu connaît l'état de la
    /// plaque, mais ne la redessine jamais : sinon elle apparaîtrait deux fois
    /// à la fermeture du panneau.
    pub hud_only: bool,
    /// Badge « REC » séparé de la plaque de performance : le statut de
    /// l'enregistrement ne dépend pas du HUD de chiffres.
    pub record_badge: bool,
    /// Ancrage du badge d'enregistrement.
    pub record_position: HudPosition,
    /// Taille du badge, en multiples de sa taille de base.
    pub record_scale: f32,
    /// Relevé CPU / mémoire / carte graphique de la plaque.
    pub system: Option<lapinux_core::system::SystemInfo>,
    pub exit: bool,
}

impl MenuState {
    pub fn new(game_name: impl Into<String>) -> Self {
        Self {
            game_name: game_name.into(),
            proton: "auto".into(),
            profile: "xbox".into(),
            profiles: Vec::new(),
            profile_selected: 0,
            fps_cap: 60,
            hud: HudPreset::Basic,
            hud_on: false,
            hud_engine: HudEngine::Native,
            perf_scale: 1.15,
            hud_position: HudPosition::TopLeft,
            hud_layout: HudLayout::Block,
            hud_graph: false,
            hud_cores: false,
            volume: 1.0,
            volume_dragging: false,
            volume_hold_until: None,
            recordings_dir: None,
            appid: 0,
            recordings: Vec::new(),
            recording_selected: 0,
            invite: SteamFriendsState::default(),
            invite_selected: 0,
            chat_friend: None,
            chat_input: String::new(),
            page: Page::Menu,
            selected: 0,
            perf_selected: 0,
            anim: 0.0,
            pulse: 0.0,
            live: Live::default(),
            perf_hud: false,
            hud_only: false,
            record_badge: false,
            record_position: HudPosition::BottomLeft,
            record_scale: 1.0,
            system: None,
            exit: false,
        }
    }

    /// Remplace les statistiques ; renvoie true si l'affichage doit être
    /// rafraîchi (rien à redessiner tant que la cadence ne bouge pas).
    pub fn set_live(&mut self, live: Live) -> bool {
        if self.live == live {
            return false;
        }
        // Le volume vient du vrai flux PipeWire : le curseur ne doit pas
        // afficher 100 % quand le jeu est déjà baissé. Pendant un glissement
        // (et juste après), le relevé peut avoir un cycle de retard : il ne
        // doit pas ramener le curseur en arrière.
        let settled = self
            .volume_hold_until
            .map_or(true, |until| Instant::now() >= until);
        if !self.volume_dragging && settled {
            if let Some(volume) = live.volume {
                self.volume = volume.clamp(0.0, 1.0);
            }
        }
        self.live = live;
        true
    }

    /// Lignes proposées dans le menu. Les amis ont leur propre carré dans le
    /// pied du panneau : ils ne changent donc plus le nombre de lignes.
    pub fn items(&self) -> Vec<Item> {
        ITEMS.to_vec()
    }

    /// Rang de la ligne « Volume du jeu » dans le menu.
    pub fn volume_row(&self) -> usize {
        self.items()
            .iter()
            .position(|item| *item == Item::Volume)
            .unwrap_or(0)
    }

    pub fn current(&self) -> Item {
        let items = self.items();
        items[self.selected.min(items.len().saturating_sub(1))]
    }

    /// Met à jour la liste d'amis, les avatars et les messages. Une lecture
    /// ratée ne ferme pas la conversation : l'état précédent reste affiché.
    pub fn set_invite(&mut self, mut invite: SteamFriendsState) -> bool {
        // Une session qui se reconnecte rend une liste vide : elle ne doit pas
        // effacer celle qu'on affichait déjà, sinon la fenêtre d'amis se vide
        // le temps que le compte revienne.
        if !invite.live && invite.friends.is_empty() && !self.invite.friends.is_empty() {
            invite.friends = std::mem::take(&mut self.invite.friends);
            let mut avatars = std::mem::take(&mut self.invite.avatars);
            avatars.extend(invite.avatars);
            invite.avatars = avatars;
            if invite.chats.is_empty() {
                invite.chats = std::mem::take(&mut self.invite.chats);
            }
        }
        if self.invite == invite {
            return false;
        }
        self.invite = invite;
        if self
            .chat_friend
            .is_some_and(|id| !self.invite.friends.iter().any(|friend| friend.steam_id == id))
        {
            self.close_chat();
        }
        if self.invite_selected >= self.invite.friends.len() {
            self.invite_selected = self.invite.friends.len().saturating_sub(1);
        }
        true
    }

    pub fn next(&mut self) {
        match self.page {
            Page::Performance => self.perf_selected = (self.perf_selected + 1) % PERF_ROWS,
            Page::Recordings => self.recording_selected = (self.recording_selected + 1) % RECORD_ROWS,
            Page::Profiles => {
                if !self.profiles.is_empty() {
                    self.profile_selected = (self.profile_selected + 1) % self.profiles.len();
                }
            }
            Page::Invite => {
                if self.chat_friend.is_none() && !self.invite.friends.is_empty() {
                    self.invite_selected =
                        (self.invite_selected + 1) % self.invite.friends.len();
                }
            }
            Page::Menu => {
                let len = self.items().len().max(1);
                self.selected = (self.selected + 1) % len;
            }
        }
        self.pulse = 1.0;
    }

    pub fn prev(&mut self) {
        match self.page {
            Page::Performance => {
                self.perf_selected = (self.perf_selected + PERF_ROWS - 1) % PERF_ROWS
            }
            Page::Recordings => {
                self.recording_selected = (self.recording_selected + RECORD_ROWS - 1) % RECORD_ROWS
            }
            Page::Profiles => {
                if !self.profiles.is_empty() {
                    self.profile_selected =
                        (self.profile_selected + self.profiles.len() - 1) % self.profiles.len();
                }
            }
            Page::Invite => {
                if self.chat_friend.is_none() && !self.invite.friends.is_empty() {
                    self.invite_selected = (self.invite_selected + self.invite.friends.len() - 1)
                        % self.invite.friends.len();
                }
            }
            Page::Menu => {
                let len = self.items().len().max(1);
                self.selected = (self.selected + len - 1) % len;
            }
        }
        self.pulse = 1.0;
    }

    /// Change d'onglet. Le panneau Performances repart de sa première ligne ;
    /// le menu retrouve « Performances » pour revenir d'un coup d'œil.
    pub fn set_page(&mut self, page: Page) {
        if self.page == page {
            return;
        }
        self.page = page;
        self.pulse = 1.0;
        match page {
            Page::Performance => self.perf_selected = 0,
            Page::Recordings => self.recording_selected = 0,
            Page::Profiles => {
                self.profile_selected = self
                    .profiles
                    .iter()
                    .position(|name| *name == self.profile)
                    .unwrap_or(0);
            }
            Page::Invite => {
                self.invite_selected = 0;
                self.chat_friend = None;
                self.chat_input.clear();
            }
            Page::Menu => {
                self.selected = self
                    .items()
                    .iter()
                    .position(|item| *item == Item::Performance)
                    .unwrap_or(0);
            }
        }
    }

    /// Ajuste la valeur de la ligne courante (gauche/droite).
    pub fn adjust(&mut self, delta: i32) -> Option<Action> {
        if self.page == Page::Performance {
            return self.adjust_perf(delta);
        }
        if self.page == Page::Recordings {
            return self.adjust_record(delta);
        }
        if self.page == Page::Profiles || self.page == Page::Invite {
            return None;
        }
        match self.current() {
            Item::Performance => {
                self.set_page(Page::Performance);
                None
            }
            Item::Volume => self.adjust_volume(delta),
            Item::Profile => Some(Action::CycleProfile),
            Item::Proton => Some(Action::CycleProton),
            _ => None,
        }
    }

    /// Pas de 5 % au clavier ou à la manette, comme les autres réglages
    /// continus. Le daemon applique la valeur au flux audio du jeu.
    fn adjust_volume(&mut self, delta: i32) -> Option<Action> {
        self.set_volume(self.volume + delta.signum() as f32 * 0.05)
    }

    /// Position du curseur de volume, de 0 à 1.
    pub fn volume_ratio(&self) -> f32 {
        self.volume.clamp(0.0, 1.0)
    }

    /// Change le volume local. Une valeur identique ne renvoie rien : une
    /// touche maintenue en butée ne relance pas le daemon en boucle.
    pub fn set_volume(&mut self, volume: f32) -> Option<Action> {
        let next = (volume.clamp(0.0, 1.0) * 100.0).round() / 100.0;
        if (next - self.volume).abs() < 0.001 {
            return None;
        }
        self.volume = next;
        self.live.volume = Some(next);
        // La valeur met un court instant à atteindre PipeWire ; les relevés
        // de la seconde en cours ne doivent pas annuler le geste.
        self.volume_hold_until = Some(Instant::now() + Duration::from_millis(450));
        self.pulse = 1.0;
        Some(Action::GameVolume(next))
    }

    /// Début d'un glissement sur la piste de volume.
    pub fn begin_volume_drag(&mut self) {
        self.volume_dragging = true;
        self.volume_hold_until = None;
    }

    /// Fin du glissement : on laisse un court délai au flux pour rejoindre la
    /// dernière valeur avant de réaccepter ses relevés.
    pub fn end_volume_drag(&mut self) {
        self.volume_dragging = false;
        self.live.volume = Some(self.volume);
        self.volume_hold_until = Some(Instant::now() + Duration::from_millis(450));
    }

    /// Volume visé par un clic sur la piste de la ligne « Volume du jeu ».
    pub fn volume_at(&self, x: f32, panel_x: f32, panel_w: f32) -> Option<f32> {
        let left = panel_x + PADDING + 58.0;
        let right = panel_x + panel_w - PADDING - 16.0;
        if right <= left || x < left - 14.0 || x > right + 14.0 {
            return None;
        }
        let ratio = ((x - left) / (right - left)).clamp(0.0, 1.0);
        Some((ratio * 100.0).round() / 100.0)
    }

    /// Cran de la piste correspondant à la limite courante. `0` (illimité)
    /// ferme la liste : il s'obtient en poussant le curseur à fond à droite.
    pub fn fps_index(&self) -> usize {
        if self.fps_cap == 0 {
            return FPS_STEPS.len() - 1;
        }
        FPS_STEPS
            .iter()
            .position(|value| *value != 0 && *value >= self.fps_cap)
            .unwrap_or(FPS_STEPS.len() - 2)
    }

    /// Position du curseur sur la piste, de 0 (le plus bas) à 1 (illimité).
    pub fn fps_ratio(&self) -> f32 {
        self.fps_index() as f32 / (FPS_STEPS.len() - 1) as f32
    }

    /// Choisit un cran de la piste. Renvoie toujours l'action : le daemon
    /// décide de la valeur réellement applicable.
    pub fn set_fps_index(&mut self, index: usize) -> Option<Action> {
        let index = index.min(FPS_STEPS.len() - 1);
        self.fps_cap = FPS_STEPS[index];
        self.pulse = 1.0;
        Some(Action::SetFpsCap(self.fps_cap))
    }

    /// Cran visé par un clic à l'abscisse `x`, dans la piste de la carte
    /// « Limite de FPS ». `None` hors de la piste : le clic reste alors un
    /// simple appui sur la ligne.
    pub fn fps_index_at(&self, x: f32, panel_x: f32, panel_w: f32) -> Option<usize> {
        let left = panel_x + PADDING + FPS_TRACK_LEFT;
        let right = panel_x + panel_w - PADDING - FPS_TRACK_RIGHT;
        if right <= left || x < left - 14.0 || x > right + 14.0 {
            return None;
        }
        let ratio = ((x - left) / (right - left)).clamp(0.0, 1.0);
        Some((ratio * (FPS_STEPS.len() - 1) as f32).round() as usize)
    }

    fn adjust_perf(&mut self, delta: i32) -> Option<Action> {
        match self.perf_selected {
            // L'interrupteur est la première ligne : il reste sous les yeux.
            0 => {
                let show = !(self.perf_hud || self.hud_on);
                self.perf_hud = show;
                self.hud_on = false;
                if show {
                    self.hud_engine = HudEngine::Native;
                }
                self.pulse = 1.0;
                Some(Action::HudVisible(show))
            }
            1 => {
                let next = (self.fps_index() as i32 + delta)
                    .clamp(0, FPS_STEPS.len() as i32 - 1) as usize;
                self.set_fps_index(next)
            }
            2 => self.set_engine(!matches!(self.hud_engine, HudEngine::MangoHud)),
            3..=5 => {
                self.hud = self.hud.cycle(delta.signum());
                self.perf_selected = 3 + self.hud.index();
                self.pulse = 1.0;
                Some(self.hud_action())
            }
            6 => {
                self.place_hud(self.hud_position.cycle(delta.signum()));
                Some(self.hud_action())
            }
            7 => {
                self.hud_layout = self.hud_layout.cycle(delta.signum());
                self.pulse = 1.0;
                Some(self.hud_action())
            }
            8 => {
                self.hud_graph = !self.hud_graph;
                self.pulse = 1.0;
                Some(self.hud_action())
            }
            9 => {
                self.hud_cores = !self.hud_cores;
                self.pulse = 1.0;
                Some(self.hud_action())
            }
            10 => self.set_scale(self.perf_scale + delta as f32 * 0.05),
            _ => None,
        }
    }

    fn adjust_record(&mut self, delta: i32) -> Option<Action> {
        // Les deux premières lignes sont des commandes : elles ne se règlent
        // pas avec gauche/droite, seule la validation les déclenche.
        match self.recording_selected {
            0 | 1 => None,
            2 => {
                self.record_badge = !self.record_badge;
                self.pulse = 1.0;
                Some(self.record_badge_action())
            }
            3 => {
                self.record_position = self.record_position.cycle(delta.signum());
                self.pulse = 1.0;
                Some(self.record_badge_action())
            }
            4 => self.set_record_scale(self.record_scale + delta as f32 * 0.05),
            _ => None,
        }
    }

    fn activate_record(&mut self) -> Option<Action> {
        match self.recording_selected {
            0 => {
                let Some(state) = self.live.recording else {
                    // Aucun encodeur en cours : la première ligne démarre
                    // directement la capture au lieu de rester sans effet.
                    self.pulse = 1.0;
                    return Some(Action::RecordingStart);
                };
                let paused = !state.paused;
                if let Some(recording) = self.live.recording.as_mut() {
                    recording.paused = paused;
                }
                self.pulse = 1.0;
                Some(Action::RecordingPause { paused })
            }
            1 => {
                // Sans capture, la ligne reste visible mais sans effet.
                self.live.recording?;
                self.pulse = 1.0;
                Some(Action::RecordingStop)
            }
            2 => {
                self.record_badge = !self.record_badge;
                self.pulse = 1.0;
                Some(self.record_badge_action())
            }
            3 => {
                self.record_position = self.record_position.cycle(1);
                self.pulse = 1.0;
                Some(self.record_badge_action())
            }
            4 => self.set_record_scale(self.record_scale + 0.05),
            _ => None,
        }
    }

    /// Action complète du badge : le daemon réécrit ses trois réglages d'un
    /// bloc et relance son processus séparé.
    pub fn record_badge_action(&self) -> Action {
        Action::RecordBadge {
            enabled: self.record_badge,
            position: self.record_position,
            scale: self.record_scale,
        }
    }

    pub fn set_record_scale(&mut self, scale: f32) -> Option<Action> {
        self.record_scale = scale.clamp(0.65, 2.0);
        self.pulse = 1.0;
        Some(self.record_badge_action())
    }

    /// Taille du badge visée par un clic sur la piste de la ligne « Taille ».
    pub fn record_scale_at(&self, x: f32, panel_x: f32, panel_w: f32) -> Option<f32> {
        let left = panel_x + PADDING + FPS_TRACK_LEFT;
        let right = panel_x + panel_w - PADDING - FPS_TRACK_RIGHT;
        if right <= left || x < left - 14.0 || x > right + 14.0 {
            return None;
        }
        let ratio = ((x - left) / (right - left)).clamp(0.0, 1.0);
        Some(((0.65 + ratio * (2.0 - 0.65)) * 20.0).round() / 20.0)
    }

    /// Valide la ligne courante (Entrée / A).
    pub fn activate(&mut self) -> Option<Action> {
        if self.page == Page::Performance {
            return self.activate_perf();
        }
        if self.page == Page::Recordings {
            return self.activate_record();
        }
        if self.page == Page::Profiles {
            return self.activate_profile();
        }
        if self.page == Page::Invite {
            return self.activate_invite();
        }
        if self.page != Page::Menu {
            return None;
        }
        match self.current() {
            Item::Resume => {
                self.exit = true;
                Some(Action::Resume)
            }
            Item::Performance => {
                self.set_page(Page::Performance);
                None
            }
            Item::Recordings => {
                self.set_page(Page::Recordings);
                None
            }
            Item::Profile => {
                self.set_page(Page::Profiles);
                None
            }
            Item::Proton => Some(Action::CycleProton),
            Item::Screenshot => Some(Action::Screenshot),
            // Le volume se règle à la piste, au clavier ou à la manette ;
            // valider la ligne ne doit pas changer le son par surprise.
            Item::Volume => None,
            Item::QuitGame => {
                self.exit = true;
                Some(Action::QuitGame)
            }
        }
    }

    /// Valide le profil sélectionné dans la liste. Un profil déjà actif ne
    /// renvoie rien : le daemon n'a pas à relancer le service d'entrée pour
    /// un choix identique.
    fn activate_profile(&mut self) -> Option<Action> {
        let name = self.profiles.get(self.profile_selected)?.clone();
        if name == self.profile {
            return None;
        }
        self.profile = name.clone();
        self.pulse = 1.0;
        Some(Action::SelectProfile(name))
    }

    /// Ouvre la conversation de l'ami sélectionné, ou envoie le texte saisi
    /// quand une conversation est déjà ouverte.
    fn activate_invite(&mut self) -> Option<Action> {
        if self.chat_friend.is_some() {
            self.send_chat()
        } else {
            self.open_chat();
            None
        }
    }

    /// Ouvre la conversation de l'ami sélectionné.
    pub fn open_chat(&mut self) -> bool {
        if self.page != Page::Invite || self.chat_friend.is_some() {
            return false;
        }
        let Some(friend) = self.invite.friends.get(self.invite_selected) else {
            return false;
        };
        self.chat_friend = Some(friend.steam_id);
        self.chat_input.clear();
        self.pulse = 1.0;
        true
    }

    /// Ouvre la conversation d'un ami précis, depuis un clic sur sa carte.
    pub fn open_chat_at(&mut self, index: usize) -> bool {
        if self.page != Page::Invite || index >= self.invite.friends.len() {
            return false;
        }
        self.invite_selected = index;
        self.open_chat()
    }

    pub fn chat_open(&self) -> bool {
        self.chat_friend.is_some()
    }

    pub fn chat_friend_id(&self) -> Option<u64> {
        self.chat_friend
    }

    /// Nom de l'ami dont la conversation est ouverte.
    pub fn chat_friend_name(&self) -> String {
        self.chat_friend
            .and_then(|id| self.invite.friends.iter().find(|friend| friend.steam_id == id))
            .map(|friend| {
                if friend.name.trim().is_empty() {
                    format!("SteamID {}", friend.steam_id)
                } else {
                    friend.name.clone()
                }
            })
            .unwrap_or_default()
    }

    /// Ligne d'état de l'ami dont la conversation est ouverte.
    pub fn chat_friend_status(&self) -> String {
        self.chat_friend
            .and_then(|id| self.invite.friends.iter().find(|friend| friend.steam_id == id))
            .map(SteamFriend::state_line)
            .unwrap_or_default()
    }

    /// Messages de la conversation ouverte, du plus ancien au plus récent.
    pub fn chat_messages(&self) -> &[SteamChatMessage] {
        self.chat_friend
            .and_then(|id| self.invite.chats.get(&id))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Dernier message échangé avec un ami, pour l'aperçu de la liste.
    pub fn last_message(&self, steam_id: u64) -> Option<&SteamChatMessage> {
        self.invite.chats.get(&steam_id)?.last()
    }

    /// Referme la conversation et revient à la liste d'amis.
    pub fn close_chat(&mut self) {
        self.chat_friend = None;
        self.chat_input.clear();
        self.pulse = 1.0;
    }

    /// Ajoute un caractère au message en cours. Les caractères de contrôle
    /// (Entrée, Échap, flèches) ne sont jamais ajoutés au texte.
    pub fn push_chat_char(&mut self, ch: char) -> bool {
        if self.chat_friend.is_none() || ch.is_control() {
            return false;
        }
        if self.chat_input.chars().count() >= CHAT_MESSAGE_MAX {
            return false;
        }
        self.chat_input.push(ch);
        self.pulse = 1.0;
        true
    }

    pub fn pop_chat_char(&mut self) -> bool {
        if self.chat_friend.is_none() {
            return false;
        }
        if self.chat_input.pop().is_some() {
            self.pulse = 1.0;
            true
        } else {
            false
        }
    }

    /// Prépare l'envoi du texte saisi. Le daemon ajoute la copie locale une
    /// fois le message accepté par Steam.
    pub fn send_chat(&mut self) -> Option<Action> {
        let steam_id = self.chat_friend?;
        let text = self.chat_input.trim();
        if text.is_empty() {
            return None;
        }
        let text = text.chars().take(CHAT_MESSAGE_MAX).collect::<String>();
        self.chat_input.clear();
        self.pulse = 1.0;
        Some(Action::SendMessage { steam_id, text })
    }

    /// Nom affiché d'un ami, avec son SteamID en secours.
    pub fn invite_friend_label(&self, index: usize) -> String {
        self.invite
            .friends
            .get(index)
            .map(|friend| {
                if friend.name.trim().is_empty() {
                    format!("SteamID {}", friend.steam_id)
                } else {
                    friend.name.clone()
                }
            })
            .unwrap_or_default()
    }

    /// Amis visibles dans la fenêtre courante de la liste.
    pub fn invite_visible(&self) -> &[SteamFriend] {
        if self.invite.friends.is_empty() {
            return &[];
        }
        let start = self.invite_offset();
        let end = (start + INVITE_ROWS).min(self.invite.friends.len());
        &self.invite.friends[start..end]
    }

    /// Décalage de la liste pour garder l'ami sélectionné à l'écran.
    pub fn invite_offset(&self) -> usize {
        let max = self.invite.friends.len().saturating_sub(INVITE_ROWS);
        self.invite_selected
            .saturating_sub(INVITE_ROWS.saturating_sub(1))
            .min(max)
    }

    /// La liste dépasse-t-elle la fenêtre visible ?
    pub fn invite_needs_scroll(&self) -> bool {
        self.invite.friends.len() > INVITE_ROWS
    }

    /// Molette : avance ou recule la sélection sans sortir de la liste. La
    /// sélection reste visible, donc la barre de défilement la suit.
    pub fn scroll_invite(&mut self, delta: i32) -> bool {
        if self.page != Page::Invite
            || self.chat_friend.is_some()
            || delta == 0
            || self.invite.friends.is_empty()
        {
            return false;
        }
        let last = self.invite.friends.len() - 1;
        let next = (self.invite_selected as i64 + delta as i64).clamp(0, last as i64) as usize;
        if next == self.invite_selected {
            return false;
        }
        self.invite_selected = next;
        self.pulse = 1.0;
        true
    }

    /// Largeur de la liste d'amis : toute la largeur intérieure de la
    /// fenêtre, avec la barre de défilement sur le bord droit.
    pub fn invite_list_width(&self, panel_w: f32) -> f32 {
        (panel_w - 2.0 * PADDING).max(1.0)
    }

    /// Bord droit de la liste d'amis, en coordonnées écran.
    pub fn invite_list_right(&self, panel_x: f32, panel_w: f32) -> f32 {
        panel_x + PADDING + self.invite_list_width(panel_w)
    }

    /// Carré « Amis » du pied du panneau principal.
    pub fn friends_button_rect(
        &self,
        panel_x: f32,
        panel_y: f32,
        panel_h: f32,
    ) -> (f32, f32, f32, f32) {
        (
            panel_x + PADDING + 4.0,
            panel_y + panel_h - FOOTER_HEIGHT + 8.0,
            FRIENDS_BUTTON_SIZE,
            FRIENDS_BUTTON_SIZE,
        )
    }

    pub fn friends_button_at(
        &self,
        panel_x: f32,
        panel_y: f32,
        panel_h: f32,
        x: f32,
        y: f32,
    ) -> bool {
        let (left, top, width, height) = self.friends_button_rect(panel_x, panel_y, panel_h);
        x >= left && x <= left + width && y >= top && y <= top + height
    }

    /// Hauteur d'une ligne d'ami, calculée pour remplir la page sans laisser
    /// un grand vide sous la liste.
    pub fn invite_row_height(&self, panel_h: f32) -> f32 {
        let available = panel_h - INVITE_TOP - FOOTER_HEIGHT - 24.0;
        (available / INVITE_ROWS as f32).clamp(44.0, 70.0)
    }

    /// Piste de la barre de défilement, en coordonnées écran.
    pub fn invite_scrollbar(
        &self,
        panel_x: f32,
        panel_y: f32,
        panel_w: f32,
        panel_h: f32,
    ) -> Option<(f32, f32, f32, f32)> {
        if !self.invite_needs_scroll() {
            return None;
        }
        let left = panel_x + PADDING;
        let width = self.invite_list_width(panel_w);
        let height = self.invite_visible().len() as f32 * self.invite_row_height(panel_h) - 8.0;
        Some((
            left + width - 4.0,
            panel_y + self.invite_row_y(0, panel_h) + 4.0,
            3.0,
            height,
        ))
    }

    /// Pouce de la barre, proportionnel au nombre d'amis visibles.
    pub fn invite_scrollbar_thumb(
        &self,
        panel_x: f32,
        panel_y: f32,
        panel_w: f32,
        panel_h: f32,
    ) -> Option<(f32, f32, f32, f32)> {
        let (x, y, width, height) = self.invite_scrollbar(panel_x, panel_y, panel_w, panel_h)?;
        let visible = self.invite_visible().len();
        let total = self.invite.friends.len();
        let thumb_height = (height * visible as f32 / total as f32)
            .max(24.0)
            .min(height);
        let max_offset = total.saturating_sub(visible);
        let progress = if max_offset == 0 {
            0.0
        } else {
            self.invite_offset() as f32 / max_offset as f32
        };
        Some((
            x,
            y + (height - thumb_height) * progress,
            width,
            thumb_height,
        ))
    }

    /// Ami visé par un clic sur la barre de défilement.
    pub fn invite_scroll_index_at(
        &self,
        panel_x: f32,
        panel_y: f32,
        panel_w: f32,
        panel_h: f32,
        x: f32,
        y: f32,
    ) -> Option<usize> {
        let (track_x, track_y, track_w, track_h) =
            self.invite_scrollbar(panel_x, panel_y, panel_w, panel_h)?;
        if x < track_x - 6.0
            || x > track_x + track_w + 6.0
            || y < track_y
            || y > track_y + track_h
            || track_h <= 0.0
        {
            return None;
        }
        let last = self.invite.friends.len().saturating_sub(1);
        let ratio = ((y - track_y) / track_h).clamp(0.0, 1.0);
        Some((ratio * last as f32).round() as usize)
    }

    /// Ligne de la page d'invitation visée par `y` (relatif au panneau).
    pub fn invite_hit_row(&self, y: f32, panel_h: f32) -> Option<usize> {
        if self.page != Page::Invite {
            return None;
        }
        let row_h = self.invite_row_height(panel_h);
        for row in 0..self.invite_visible().len() {
            let top = self.invite_row_y(row, panel_h);
            if y >= top && y < top + row_h {
                return Some(self.invite_offset() + row);
            }
        }
        None
    }

    /// Haut d'une ligne d'ami, relatif au panneau.
    pub fn invite_row_y(&self, row: usize, panel_h: f32) -> f32 {
        INVITE_TOP + row as f32 * self.invite_row_height(panel_h)
    }

    /// Hauteur de la fenêtre des amis.
    pub fn invite_height(&self) -> f32 {
        FRIENDS_HEIGHT
    }

    fn activate_perf(&mut self) -> Option<Action> {
        match self.perf_selected {
            0 => self.adjust_perf(1),
            1 => self.adjust_perf(1),
            2 => self.set_engine(!matches!(self.hud_engine, HudEngine::MangoHud)),
            3..=5 => {
                self.hud = HudPreset::ALL[self.perf_selected - 3];
                self.pulse = 1.0;
                Some(self.hud_action())
            }
            6 => {
                self.place_hud(self.hud_position.cycle(1));
                Some(self.hud_action())
            }
            7 => {
                self.hud_layout = self.hud_layout.cycle(1);
                self.pulse = 1.0;
                Some(self.hud_action())
            }
            8 => {
                self.hud_graph = !self.hud_graph;
                self.pulse = 1.0;
                Some(self.hud_action())
            }
            9 => {
                self.hud_cores = !self.hud_cores;
                self.pulse = 1.0;
                Some(self.hud_action())
            }
            10 => self.set_scale(self.perf_scale + 0.05),
            _ => None,
        }
    }

    /// Applique un ancrage MangoHud. La couche vendue par Lapinux corrige la
    /// position du HUD en ligne : les neuf ancrages tiennent dans les deux
    /// dispositions.
    pub fn place_hud(&mut self, position: HudPosition) {
        self.hud_position = position;
        self.pulse = 1.0;
    }

    /// Change le moteur du HUD. La plaque native s'applique tout de suite ;
    /// MangoHud, lui, est injecté au lancement du jeu.
    pub fn set_engine(&mut self, mangohud: bool) -> Option<Action> {
        self.hud_engine = if mangohud {
            HudEngine::MangoHud
        } else {
            HudEngine::Native
        };
        self.hud_on = mangohud;
        self.perf_hud = !mangohud;
        self.pulse = 1.0;
        Some(Action::HudEngine { mangohud })
    }

    /// Taille de la plaque native, bornée comme le réglage des Captures.
    pub fn set_scale(&mut self, scale: f32) -> Option<Action> {
        let next = scale.clamp(0.75, 2.0);
        // En butée, la touche maintenue ne doit pas relancer le HUD en boucle.
        if (next - self.perf_scale).abs() < 0.001 {
            return None;
        }
        self.perf_scale = next;
        self.pulse = 1.0;
        Some(Action::HudScale(self.perf_scale))
    }

    /// Taille visée par un clic sur la piste de la ligne « Taille du HUD ».
    pub fn scale_at(&self, x: f32, panel_x: f32, panel_w: f32) -> Option<f32> {
        let left = panel_x + PADDING + FPS_TRACK_LEFT;
        let right = panel_x + panel_w - PADDING - FPS_TRACK_RIGHT;
        if right <= left || x < left - 14.0 || x > right + 14.0 {
            return None;
        }
        let ratio = ((x - left) / (right - left)).clamp(0.0, 1.0);
        Some(((0.75 + ratio * (2.0 - 0.75)) * 20.0).round() / 20.0)
    }

    /// Bouton Natif / MangoHud visé par un clic, s'il y en a un.
    pub fn engine_at(
        &self,
        panel_x: f32,
        panel_y: f32,
        panel_w: f32,
        x: f32,
        y: f32,
    ) -> Option<bool> {
        if self.page != Page::Performance {
            return None;
        }
        let card_y = panel_y + self.perf_row_y(2) + 2.0;
        let card_h = PERF_ROW - 4.0;
        if y < card_y || y >= card_y + card_h {
            return None;
        }
        let right = panel_x + panel_w - PADDING - 16.0;
        let left = right - (2.0 * PERF_ENGINE_W + PERF_ENGINE_GAP);
        if x < left || x > right {
            return None;
        }
        Some(x >= left + PERF_ENGINE_W + PERF_ENGINE_GAP)
    }

    /// Action MangoHud avec l'état complet du panneau : le daemon réécrit le
    /// fichier du jeu d'un bloc, sans deviner le profil ou l'ancrage.
    pub fn hud_action(&self) -> Action {
        Action::Hud {
            enabled: self.hud_on,
            profile: self.hud,
            position: self.hud_position,
            layout: self.hud_layout,
            graph: self.hud_graph,
            cores: self.hud_cores,
        }
    }

    /// Referme le panneau ouvert (Performances ou Enregistrements), sinon
    /// l'overlay. Dans une conversation, Échap revient d'abord à la liste.
    pub fn close(&mut self) {
        if self.page == Page::Invite && self.chat_friend.is_some() {
            self.close_chat();
        } else if self.page != Page::Menu {
            self.set_page(Page::Menu);
        } else {
            self.exit = true;
        }
    }

    /// Avance les animations ; renvoie true si un repaint est nécessaire.
    pub fn tick(&mut self, dt: f32) -> bool {
        let mut repaint = false;
        if self.anim < 1.0 {
            self.anim = (self.anim + dt / OPEN_SECONDS).min(1.0);
            repaint = true;
        }
        if self.pulse > 0.0 {
            self.pulse = (self.pulse - dt / 0.16).max(0.0);
            repaint = true;
        }
        repaint
    }

    /// Ramène l'animation vers zéro avant de détruire la surface : la
    /// fermeture glisse vers la gauche au lieu de disparaître d'un coup.
    pub fn tick_out(&mut self, dt: f32) -> bool {
        if self.anim <= 0.0 {
            self.anim = 0.0;
            return false;
        }
        self.anim = (self.anim - dt / CLOSE_SECONDS).max(0.0);
        if self.pulse > 0.0 {
            self.pulse = (self.pulse - dt / 0.16).max(0.0);
        }
        true
    }

    pub fn label(&self, item: Item) -> String {
        match item {
            Item::Resume => "Reprendre".into(),
            Item::Performance => "Performances".into(),
            Item::Recordings => "Enregistrements".into(),
            Item::Volume => "Volume du jeu".into(),
            Item::Profile => format!("Profil manettes : {}", self.profile),
            Item::Proton => format!("Proton : {}", self.proton),
            Item::Screenshot => "Capture d'écran".into(),
            Item::QuitGame => format!("Quitter {}", self.game_name),
        }
    }

    /// Valeur affichée à droite de la ligne, quand il y en a une.
    pub fn value(&self, item: Item) -> Option<String> {
        match item {
            Item::Resume => None,
            Item::Performance => Some(match self.hud_engine {
                HudEngine::Native => "Natif · plaque Lapinux".into(),
                HudEngine::MangoHud => format!("MangoHud · {}", self.hud.label()),
            }),
            Item::Recordings => Some(if self.record_badge {
                "Affiché".into()
            } else {
                "Coupé".into()
            }),
            Item::Volume => Some(format!("{:.0} %", self.volume * 100.0)),
            Item::Profile => Some(self.profile.clone()),
            Item::Proton => Some(self.proton.clone()),
            Item::Screenshot => Some("PNG".into()),
            Item::QuitGame => Some("Arrêter".into()),
        }
    }

    pub fn hint(&self, item: Item) -> &'static str {
        match item {
            Item::Resume => "Entrée",
            Item::Performance => "Entrée",
            Item::Recordings => "Entrée",
            Item::Volume => "← →",
            Item::Profile => "Entrée · liste",
            Item::Proton => "← → (prochain lancement)",
            Item::Screenshot => "Entrée",
            Item::QuitGame => "Entrée",
        }
    }

    /// Nom de la ligne du panneau Performances.
    pub fn perf_label(&self, row: usize) -> String {
        if self.page == Page::Recordings {
            return match row {
                0 => match self.live.recording {
                    Some(state) if state.paused => "Reprendre".into(),
                    Some(_) => "Pause".into(),
                    None => "Démarrer l'enregistrement".into(),
                },
                1 => "Arrêter l'enregistrement".into(),
                2 => "HUD d'enregistrement".into(),
                3 => "Emplacement".into(),
                4 => "Taille".into(),
                _ => String::new(),
            };
        }
        match row {
            0 => "HUD de performance".into(),
            1 => "Limite de FPS".into(),
            2 => "Moteur du HUD".into(),
            3..=5 => HudPreset::ALL[row - 3].label().into(),
            6 => "Emplacement".into(),
            7 => "Disposition".into(),
            8 => "Graphique".into(),
            9 => "Cœurs du CPU".into(),
            10 => "Taille du HUD".into(),
            _ => String::new(),
        }
    }

    /// Valeur de la ligne du panneau Performances.
    pub fn perf_value(&self, row: usize) -> String {
        if self.page == Page::Recordings {
            return match row {
                0 | 1 => String::new(),
                3 => self.record_position.label().into(),
                4 => format!("{:.2} ×", self.record_scale),
                _ => String::new(),
            };
        }
        match row {
            0 => String::new(),
            1 => {
                if self.fps_cap == 0 {
                    "Illimité".into()
                } else {
                    format!("{} FPS", self.fps_cap)
                }
            }
            2 => String::new(),
            3..=5 => {
                if self.hud_on && self.hud == HudPreset::ALL[row - 3] {
                    "Actif".into()
                } else {
                    String::new()
                }
            }
            6 => self.hud_position.label().into(),
            7 => self.hud_layout.label().into(),
            8 => {
                if self.hud_graph {
                    "Activé".into()
                } else {
                    "Désactivé".into()
                }
            }
            9 => {
                if self.hud_cores {
                    "Activé".into()
                } else {
                    "Désactivé".into()
                }
            }
            10 => format!("{:.2} ×", self.perf_scale),
            _ => String::new(),
        }
    }

    /// Sous-titre du panneau Performances.
    pub fn perf_subtitle(&self, row: usize) -> String {
        if self.page == Page::Recordings {
            return match row {
                0 => match self.live.recording {
                    Some(state) if state.paused => "Reprendre la capture".into(),
                    Some(_) => "Suspendre sans couper la vidéo".into(),
                    None => "Lancer une capture de la partie".into(),
                },
                1 => {
                    if self.live.recording.is_some() {
                        "Terminer et garder la vidéo".into()
                    } else {
                        "Aucun enregistrement en cours".into()
                    }
                }
                2 => "Afficher ou retirer le badge REC".into(),
                3 => "Ancrage du badge à l'écran".into(),
                4 => String::new(),
                _ => String::new(),
            };
        }
        match row {
            0 => "Afficher ou retirer la plaque".into(),
            2 => "Natif ou MangoHud".into(),
            3 => "GPU · CPU · températures".into(),
            4 => "Mémoire et températures".into(),
            5 => "VRAM · fréquences · puissance".into(),
            6 => "Ancrage du HUD à l'écran".into(),
            7 => "Une ligne ou un bloc".into(),
            8 => "Courbe de frametime".into(),
            9 => "Utilisation de chaque cœur".into(),
            10 => String::new(),
            _ => String::new(),
        }
    }

    /// Ordonnée du haut de la ligne `index` du menu, relative au panneau.
    pub fn row_y(&self, index: usize) -> f32 {
        HEADER_HEIGHT + index as f32 * ROW_HEIGHT
    }

    /// Ordonnée du haut d'une ligne réglable du panneau Performances.
    pub fn perf_row_y(&self, index: usize) -> f32 {
        if self.page == Page::Recordings {
            return match index {
                0 => RECORDINGS_TOP,
                1 => RECORDINGS_TOP + PERF_ROW,
                2 => RECORDINGS_TOP + 2.0 * PERF_ROW + PERF_SECTION,
                3 => RECORDINGS_TOP + 3.0 * PERF_ROW + 2.0 * PERF_SECTION,
                4 => RECORDINGS_TOP + 4.0 * PERF_ROW + 3.0 * PERF_SECTION,
                _ => RECORDINGS_TOP + 4.0 * PERF_ROW + 3.0 * PERF_SECTION,
            };
        }
        match index {
            0 => PERF_TOP,
            1 => PERF_TOP + PERF_ROW,
            2 => PERF_TOP + 2.0 * PERF_ROW,
            3..=5 => PERF_TOP + 3.0 * PERF_ROW + PERF_SECTION + (index - 3) as f32 * PERF_ROW,
            6 => PERF_TOP + 3.0 * PERF_ROW + PERF_SECTION + 3.0 * PERF_ROW + PERF_SECTION,
            7..=10 => {
                PERF_TOP
                    + 3.0 * PERF_ROW
                    + PERF_SECTION
                    + 3.0 * PERF_ROW
                    + PERF_SECTION
                    + PERF_ROW
                    + (index as f32 - 7.0) * PERF_ROW
            }
            _ => PERF_TOP,
        }
    }

    /// Haut de la grille des positions, sous la ligne « Cœurs du CPU ».
    pub fn perf_grid_top(&self) -> f32 {
        if self.page == Page::Recordings {
            return self.perf_row_y(RECORD_ROWS - 1) + PERF_ROW + 6.0;
        }
        self.perf_row_y(PERF_ROWS - 1) + PERF_ROW + 6.0
    }

    /// Rectangle du bouton Retour du panneau Performances : (x, y, largeur,
    /// hauteur), relatif à l'écran.
    pub fn perf_back_rect(
        &self,
        panel_x: f32,
        panel_y: f32,
        panel_w: f32,
    ) -> (f32, f32, f32, f32) {
        (
            panel_x + PADDING,
            panel_y + PERF_BACK_TOP,
            panel_w - 2.0 * PADDING,
            PERF_BACK_HEIGHT,
        )
    }

    /// Hauteur du panneau Performances : il couvre la grille et le détail DLSS.
    pub fn perf_height(&self) -> f32 {
        self.perf_grid_top() + PERF_GRID + 8.0 + PERF_INFO + 10.0 + FOOTER_HEIGHT
    }

    pub fn panel_height(&self) -> f32 {
        match self.page {
            Page::Menu => {
                HEADER_HEIGHT
                    + self.items().len() as f32 * ROW_HEIGHT
                    + 6.0
                    + FOOTER_HEIGHT
            }
            Page::Performance => self.perf_height(),
            Page::Recordings => self.recordings_height(),
            Page::Profiles => self.profiles_height(),
            Page::Invite => self.invite_height(),
        }
    }

    /// Haut de la ligne d'un profil dans la page « Profils manettes ».
    pub fn profile_row_y(&self, index: usize) -> f32 {
        HEADER_HEIGHT + PROFILE_TOP + index as f32 * ROW_HEIGHT
    }

    /// Ligne de profil visée par `y` (relatif au panneau).
    pub fn profile_hit_row(&self, y: f32) -> Option<usize> {
        if self.page != Page::Profiles {
            return None;
        }
        for index in 0..self.profiles.len() {
            let top = self.profile_row_y(index);
            if y >= top && y < top + ROW_HEIGHT {
                return Some(index);
            }
        }
        None
    }

    /// Hauteur de la page « Profils manettes » : titre, une ligne par profil
    /// et le pied de page.
    pub fn profiles_height(&self) -> f32 {
        HEADER_HEIGHT
            + PROFILE_TOP
            + self.profiles.len() as f32 * ROW_HEIGHT
            + 8.0
            + FOOTER_HEIGHT
    }

    /// Hauteur de la page « Enregistrements » : retour, statut, cinq lignes,
    /// la grille des neuf ancrages et le pied de page.
    pub fn recordings_height(&self) -> f32 {
        self.perf_grid_top() + PERF_GRID + 8.0 + FOOTER_HEIGHT
    }

    /// Ligne de réglage visée par `y` (relatif au panneau).
    pub fn hit_recording_row(&self, y: f32) -> Option<usize> {
        if self.page != Page::Recordings {
            return None;
        }
        for index in 0..RECORD_ROWS {
            let top = self.perf_row_y(index);
            if y >= top && y < top + PERF_ROW {
                return Some(index);
            }
        }
        None
    }

    /// Case de la grille des ancrages du badge visée par un clic.
    pub fn record_position_cell_at(
        &self,
        panel_x: f32,
        panel_y: f32,
        panel_w: f32,
        x: f32,
        y: f32,
    ) -> Option<HudPosition> {
        if self.page != Page::Recordings {
            return None;
        }
        let top = panel_y + self.perf_grid_top();
        if y < top || y >= top + PERF_GRID {
            return None;
        }
        let left = panel_x + PADDING;
        let inner = (panel_w - 2.0 * PADDING).max(0.0);
        const GAP: f32 = 4.0;
        let cell_w = (inner - 2.0 * GAP) / 3.0;
        let cell_h = (PERF_GRID - 2.0 * GAP) / 3.0;
        let col = ((x - left) / (cell_w + GAP)).floor() as i32;
        let row = ((y - top) / (cell_h + GAP)).floor() as i32;
        if !(0..3).contains(&col) || !(0..3).contains(&row) {
            return None;
        }
        HudPosition::ALL.get((row * 3 + col) as usize).copied()
    }

    /// Relit les vidéos de la page « Enregistrements ». Le dossier vient du
    /// dæmon ; les noms personnalisés ne portent plus forcément l'appid, donc
    /// la page liste les vidéos récentes du dossier plutôt que de filtrer sur
    /// un préfixe technique.
    pub fn load_recordings(&mut self) {
        self.recordings.clear();
        let Some(dir) = self.recordings_dir.as_ref() else {
            return;
        };
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("mp4") {
                continue;
            }
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let meta = entry.metadata().ok();
            self.recordings.push(Recording {
                name: name.to_string(),
                bytes: meta.as_ref().map(|meta| meta.len()).unwrap_or(0),
                modified: meta
                    .as_ref()
                    .and_then(|meta| meta.modified().ok())
                    .and_then(|stamp| stamp.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|age| age.as_secs() as i64)
                    .unwrap_or(0),
            });
        }
        self.recordings.sort_by(|a, b| b.modified.cmp(&a.modified));
        self.recordings.truncate(8);
        if self.recording_selected >= self.recordings.len() {
            self.recording_selected = 0;
        }
    }

    /// Reprend les réglages HUD de la session : la plaque de performance vit
    /// dans un processus à part, et les changements faits dans le panneau
    /// rapide (profil, ancrage, disposition, courbe, cœurs) doivent l'atteindre
    /// sans relancer la partie.
    pub fn apply_session_hud(&mut self, info: &lapinux_core::model::SessionInfo) -> bool {
        let mut changed = self.apply_session_identity(info);
        let hud = HudPreset::from_wire(&info.hud_preset);
        let position = HudPosition::from_wire(&info.hud_position);
        let layout = HudLayout::from_wire(&info.hud_layout);
        if self.hud != hud {
            self.hud = hud;
            changed = true;
        }
        if self.hud_position != position {
            self.hud_position = position;
            changed = true;
        }
        if self.hud_layout != layout {
            self.hud_layout = layout;
            changed = true;
        }
        if self.hud_graph != info.hud_graph {
            self.hud_graph = info.hud_graph;
            changed = true;
        }
        if self.hud_cores != info.hud_cores {
            self.hud_cores = info.hud_cores;
            changed = true;
        }
        changed
    }

    /// Reprend l'identité de la session : le profil de manettes change à
    /// chaud dans le daemon, et le panneau doit afficher le nom réellement
    /// actif sans attendre une réouverture.
    pub fn apply_session_identity(&mut self, info: &lapinux_core::model::SessionInfo) -> bool {
        let mut changed = false;
        if !info.input_profile.is_empty() && self.profile != info.input_profile {
            self.profile = info.input_profile.clone();
            changed = true;
        }
        let proton = if info.proton.is_empty() {
            "auto".to_string()
        } else {
            info.proton.clone()
        };
        if self.proton != proton {
            self.proton = proton;
            changed = true;
        }
        if self.fps_cap != info.fps_cap {
            self.fps_cap = info.fps_cap;
            changed = true;
        }
        changed
    }

    /// Ligne du menu visée par `y` (relatif au panneau).
    pub fn hit_row(&self, y: f32) -> Option<usize> {
        if self.page != Page::Menu {
            return None;
        }
        for index in 0..self.items().len() {
            let top = self.row_y(index);
            if y >= top && y < top + ROW_HEIGHT {
                return Some(index);
            }
        }
        None
    }

    /// Ligne réglable du panneau Performances visée par `y`.
    pub fn hit_perf_row(&self, y: f32) -> Option<usize> {
        if self.page != Page::Performance {
            return None;
        }
        for index in 0..PERF_ROWS {
            let top = self.perf_row_y(index);
            if y >= top && y < top + PERF_ROW {
                return Some(index);
            }
        }
        None
    }

    /// Le clic tombe-t-il sur le bouton Retour du panneau Performances ?
    pub fn back_at(&self, panel_x: f32, panel_y: f32, panel_w: f32, x: f32, y: f32) -> bool {
        if self.page == Page::Menu {
            return false;
        }
        let (left, top, width, height) = self.perf_back_rect(panel_x, panel_y, panel_w);
        x >= left && x <= left + width && y >= top && y <= top + height
    }

    /// Case de la grille des positions visée par un clic.
    pub fn position_cell_at(
        &self,
        panel_x: f32,
        panel_y: f32,
        panel_w: f32,
        x: f32,
        y: f32,
    ) -> Option<HudPosition> {
        if self.page != Page::Performance {
            return None;
        }
        let top = panel_y + self.perf_grid_top();
        if y < top || y >= top + PERF_GRID {
            return None;
        }
        let left = panel_x + PADDING;
        let inner = (panel_w - 2.0 * PADDING).max(0.0);
        const GAP: f32 = 4.0;
        let cell_w = (inner - 2.0 * GAP) / 3.0;
        let cell_h = (PERF_GRID - 2.0 * GAP) / 3.0;
        let col = ((x - left) / (cell_w + GAP)).floor() as i32;
        let row = ((y - top) / (cell_h + GAP)).floor() as i32;
        if !(0..3).contains(&col) || !(0..3).contains(&row) {
            return None;
        }
        HudPosition::ALL.get((row * 3 + col) as usize).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_wraps() {
        let mut menu = MenuState::new("Jeu");
        menu.prev();
        assert_eq!(menu.selected, menu.items().len() - 1);
        menu.next();
        assert_eq!(menu.selected, 0);
        assert_eq!(menu.current(), Item::Resume);
    }

    #[test]
    fn performance_tab_opens_with_enter() {
        let mut menu = MenuState::new("Jeu");
        menu.selected = 1;
        assert_eq!(menu.activate(), None);
        assert_eq!(menu.page, Page::Performance);
        assert_eq!(menu.perf_selected, 0);
        menu.close();
        assert_eq!(menu.page, Page::Menu);
        assert_eq!(menu.current(), Item::Performance);
        menu.close();
        assert!(menu.exit);
    }

    #[test]
    fn fps_cap_steps_in_the_performance_panel() {
        let mut menu = MenuState::new("Jeu");
        menu.set_page(Page::Performance);
        menu.perf_selected = 1;
        assert_eq!(menu.adjust(1), Some(Action::SetFpsCap(90)));
        assert_eq!(menu.adjust(-1), Some(Action::SetFpsCap(60)));
        // Le palier le plus bas est 15 i/s : on peut descendre jusque-là.
        assert_eq!(menu.adjust(-5), Some(Action::SetFpsCap(15)));
        // Le dernier cran coupe la limite.
        assert_eq!(menu.adjust(1), Some(Action::SetFpsCap(30)));
        menu.set_fps_index(FPS_STEPS.len() - 1);
        assert_eq!(menu.fps_cap, 0);
        assert_eq!(menu.perf_value(1), "Illimité");
        assert_eq!(menu.adjust(1), Some(Action::SetFpsCap(0)));
    }

    #[test]
    fn fps_track_maps_clicks_to_steps() {
        let menu = MenuState::new("Jeu");
        let panel_x = 40.0;
        let panel_w = 396.0;
        let left = panel_x + PADDING + FPS_TRACK_LEFT;
        let right = panel_x + panel_w - PADDING - FPS_TRACK_RIGHT;
        assert_eq!(menu.fps_index_at(left, panel_x, panel_w), Some(0));
        assert_eq!(
            menu.fps_index_at(right, panel_x, panel_w),
            Some(FPS_STEPS.len() - 1)
        );
        // Le milieu de la piste tombe sur le palier du milieu : l'arrondi
        // part de la moitié, pas d'un entier tronqué.
        assert_eq!(
            menu.fps_index_at((left + right) / 2.0, panel_x, panel_w),
            Some(((FPS_STEPS.len() - 1) as f32 / 2.0).round() as usize)
        );
        // À gauche de la piste, le clic reste un appui sur la ligne.
        assert_eq!(menu.fps_index_at(panel_x + 10.0, panel_x, panel_w), None);
    }

    #[test]
    fn engine_and_scale_live_in_the_performance_panel() {
        let mut menu = MenuState::new("Jeu");
        menu.set_page(Page::Performance);
        menu.perf_selected = 2;
        assert_eq!(
            menu.activate(),
            Some(Action::HudEngine { mangohud: true })
        );
        assert!(menu.hud_on);
        assert!(!menu.perf_hud);
        assert_eq!(
            menu.activate(),
            Some(Action::HudEngine { mangohud: false })
        );
        assert!(!menu.hud_on);
        assert!(menu.perf_hud);
        // L'échelle se règle sur la dernière ligne et reste bornée.
        assert_eq!(menu.perf_label(10), "Taille du HUD");
        menu.set_scale(1.5);
        assert_eq!(menu.perf_value(10), "1.50 ×");
        assert_eq!(menu.set_scale(9.0), Some(Action::HudScale(2.0)));
        assert_eq!(menu.perf_value(10), "2.00 ×");
    }

    #[test]
    fn hud_scale_track_reaches_both_ends() {
        let mut menu = MenuState::new("Jeu");
        menu.set_page(Page::Performance);
        let panel_x = 40.0;
        let panel_w = 520.0;
        let left = panel_x + PADDING + FPS_TRACK_LEFT;
        let right = panel_x + panel_w - PADDING - FPS_TRACK_RIGHT;
        assert_eq!(menu.scale_at(left, panel_x, panel_w), Some(0.75));
        assert_eq!(menu.scale_at(right, panel_x, panel_w), Some(2.0));
        assert_eq!(
            menu.set_scale(menu.scale_at(right, panel_x, panel_w).unwrap()),
            Some(Action::HudScale(2.0))
        );
    }

    #[test]
    fn activate_resume_exits() {
        let mut menu = MenuState::new("Jeu");
        assert_eq!(menu.activate(), Some(Action::Resume));
        assert!(menu.exit);
    }

    #[test]
    fn quit_game_exits() {
        let mut menu = MenuState::new("Jeu");
        menu.selected = menu.items().len() - 1;
        assert_eq!(menu.activate(), Some(Action::QuitGame));
        assert!(menu.exit);
    }

    #[test]
    fn animation_progresses() {
        let mut menu = MenuState::new("Jeu");
        assert!(menu.tick(0.05));
        assert!(menu.anim > 0.0);
        menu.anim = 1.0;
        menu.pulse = 0.0;
        assert!(!menu.tick(0.05));
    }

    #[test]
    fn labels_and_hints_are_stable() {
        let mut menu = MenuState::new("Team Fortress 2");
        menu.fps_cap = 120;
        menu.hud_on = true;
        menu.hud_engine = HudEngine::MangoHud;
        menu.hud = HudPreset::Detailed;
        assert_eq!(menu.perf_label(0), "HUD de performance");
        assert_eq!(menu.perf_label(1), "Limite de FPS");
        assert_eq!(menu.perf_value(1), "120 FPS");
        assert!(menu.label(Item::QuitGame).contains("Team Fortress 2"));
        assert_eq!(menu.hint(Item::Proton), "← → (prochain lancement)");
        assert_eq!(
            menu.value(Item::Performance).as_deref(),
            Some("MangoHud · Détaillée")
        );
    }

    #[test]
    fn hit_testing_finds_rows() {
        let menu = MenuState::new("Jeu");
        let first = menu.row_y(0) + 10.0;
        assert_eq!(menu.hit_row(first), Some(0));
        let second = menu.row_y(1) + 10.0;
        assert_eq!(menu.hit_row(second), Some(1));
        let last = menu.row_y(menu.items().len() - 1) + 10.0;
        assert_eq!(menu.hit_row(last), Some(menu.items().len() - 1));
        assert_eq!(menu.hit_row(0.0), None);
        // Sur l'onglet Performances, les lignes du menu ne répondent plus.
        let mut menu = menu;
        menu.set_page(Page::Performance);
        assert_eq!(menu.hit_row(first), None);
        assert_eq!(menu.hit_perf_row(PERF_TOP + 10.0), Some(0));
    }

    #[test]
    fn rows_are_evenly_spaced_under_the_capsule() {
        let menu = MenuState::new("Jeu");
        assert_eq!(menu.row_y(0), HEADER_HEIGHT);
        for index in 1..menu.items().len() {
            assert_eq!(menu.row_y(index) - menu.row_y(index - 1), ROW_HEIGHT);
        }
        // La dernière ligne tient dans le panneau, pied de page compris.
        let last_bottom = menu.row_y(menu.items().len() - 1) + ROW_HEIGHT;
        assert!(last_bottom + FOOTER_HEIGHT <= menu.panel_height());
        // Huit lignes tiennent encore dans un écran 1080p, avec la capsule.
        assert!(menu.panel_height() < 840.0);
    }

    #[test]
    fn performance_panel_is_tall_and_fits() {
        let mut menu = MenuState::new("Jeu");
        menu.set_page(Page::Performance);
        let height = menu.panel_height();
        assert!(height > 600.0, "panneau Performances trop bas : {height}");
        assert!(height < 840.0, "panneau Performances trop haut : {height}");
        // Les sept lignes et la grille ne se chevauchent pas.
        for index in 1..PERF_ROWS {
            assert!(menu.perf_row_y(index) >= menu.perf_row_y(index - 1) + PERF_ROW);
        }
        assert!(menu.perf_grid_top() >= menu.perf_row_y(PERF_ROWS - 1) + PERF_ROW);
        assert!(menu.perf_grid_top() + PERF_GRID < height);
    }

    #[test]
    fn every_item_has_its_own_icon() {
        let mut seen = Vec::new();
        for item in ITEMS {
            let glyph = icon(item);
            assert!(!seen.contains(&glyph), "pictogramme en double : {glyph:?}");
            seen.push(glyph);
        }
        assert_eq!(icon(Item::QuitGame), Icon::Cross);
        assert_eq!(icon(Item::Resume), Icon::Play);
        assert_eq!(icon(Item::Performance), Icon::Screen);
    }

    #[test]
    fn volume_slider_drives_the_game_stream() {
        let mut menu = MenuState::new("Jeu");
        menu.selected = menu.volume_row();
        assert_eq!(menu.volume, 1.0);
        assert_eq!(menu.adjust(-1), Some(Action::GameVolume(0.95)));
        assert_eq!(menu.value(Item::Volume).as_deref(), Some("95 %"));
        assert_eq!(menu.set_volume(0.4), Some(Action::GameVolume(0.4)));
        assert_eq!(menu.set_volume(0.4), None);
        // Un relevé PipeWire antérieur au geste ne fait pas reculer le curseur.
        menu.begin_volume_drag();
        assert!(menu.set_live(Live {
            volume: Some(0.2),
            ..Default::default()
        }));
        assert_eq!(menu.volume, 0.4);
        menu.end_volume_drag();
        assert_eq!(menu.volume, 0.4);
        assert_eq!(Action::GameVolume(0.4).wire(), "game_volume:0.40");
        assert_eq!(menu.volume_at(500.0, 0.0, 440.0), None);
        assert_eq!(menu.volume_at(241.0, 0.0, 440.0), Some(0.5));
    }

    #[test]
    fn live_labels_fall_back_gracefully() {
        let live = Live::default();
        assert_eq!(live.fps_label(), "— FPS");
        assert_eq!(live.session_label(), "Session —");
        let live = Live {
            fps: Some(142),
            session_secs: Some(4_380),
            series: Vec::new(),
            dlss: None,
            hud_live: false,
            gamescope: false,
            recording: None,
            volume: None,
        };
        assert_eq!(live.fps_label(), "142 FPS");
        assert_eq!(live.session_label(), "Session 1 h 13");
    }

    #[test]
    fn la_page_enregistrements_commande_la_capture() {
        let mut menu = MenuState::new("Jeu");
        menu.set_page(Page::Recordings);
        assert_eq!(menu.perf_label(0), "Démarrer l'enregistrement");
        assert_eq!(menu.activate(), Some(Action::RecordingStart));
        assert_eq!(Action::RecordingStart.wire(), "recording_start");
        menu.live.recording = Some(lapinux_core::model::RecordingState {
            paused: false,
            secs: 83,
        });
        assert_eq!(
            menu.activate(),
            Some(Action::RecordingPause { paused: true })
        );
        assert!(menu.live.recording.is_some_and(|state| state.paused));
        assert_eq!(menu.perf_label(0), "Reprendre");
        menu.recording_selected = 1;
        assert_eq!(menu.activate(), Some(Action::RecordingStop));
        assert_eq!(
            Action::RecordingPause { paused: true }.wire(),
            "recording_pause:1"
        );
        assert_eq!(Action::RecordingStop.wire(), "recording_stop");
    }

    #[test]
    fn la_page_enregistrements_garde_ses_reglages_sous_les_commandes() {
        let mut menu = MenuState::new("Jeu");
        menu.set_page(Page::Recordings);
        menu.recording_selected = 2;
        assert!(matches!(
            menu.activate(),
            Some(Action::RecordBadge { enabled: true, .. })
        ));
        menu.recording_selected = 3;
        assert!(matches!(
            menu.activate(),
            Some(Action::RecordBadge { .. })
        ));
        menu.recording_selected = 4;
        assert!(matches!(
            menu.activate(),
            Some(Action::RecordBadge { .. })
        ));
        assert!(menu.perf_row_y(4) + PERF_ROW <= menu.perf_grid_top());
        assert!(menu.perf_grid_top() + PERF_GRID < menu.recordings_height());
        assert!(menu.recordings_height() < 800.0);
    }

    #[test]
    fn le_bandeau_de_capture_affiche_le_temps_enregistre() {
        let mut menu = MenuState::new("Jeu");
        assert_eq!(menu.live.recording_label(), None);
        menu.live.recording = Some(lapinux_core::model::RecordingState {
            paused: false,
            secs: 83,
        });
        assert_eq!(menu.live.recording_label().as_deref(), Some("REC 01:23"));
        menu.live.recording = Some(lapinux_core::model::RecordingState {
            paused: true,
            secs: 3_725,
        });
        assert_eq!(
            menu.live.recording_label().as_deref(),
            Some("REC pause · 1:02:05")
        );
    }

    #[test]
    fn fps_status_compares_with_the_cap() {
        let live = Live {
            fps: Some(142),
            session_secs: None,
            series: Vec::new(),
            dlss: None,
            hud_live: false,
            gamescope: false,
            recording: None,
            volume: None,
        };
        assert_eq!(live.fps_status(144), FpsStatus::Good);
        assert_eq!(live.fps_status(165), FpsStatus::Low);
        assert_eq!(live.fps_status(0), FpsStatus::Good);
        assert_eq!(Live::default().fps_status(60), FpsStatus::Unknown);
    }

    #[test]
    fn live_updates_only_when_values_change() {
        let mut menu = MenuState::new("Jeu");
        let live = Live {
            fps: Some(60),
            session_secs: Some(30),
            series: vec![60, 61, 60],
            dlss: None,
            hud_live: false,
            gamescope: false,
            recording: None,
            volume: None,
        };
        assert!(menu.set_live(live.clone()));
        assert!(!menu.set_live(live));
        assert!(menu.set_live(Live {
            fps: Some(61),
            session_secs: Some(31),
            series: vec![60, 61, 60, 61],
            dlss: None,
            hud_live: false,
            gamescope: false,
            recording: None,
            volume: None,
        }));
    }

    #[test]
    fn values_track_the_state() {
        let mut menu = MenuState::new("Jeu");
        menu.fps_cap = 144;
        menu.hud = HudPreset::Detailed;
        menu.hud_on = false;
        assert_eq!(menu.perf_value(1), "144 FPS");
        assert_eq!(
            menu.value(Item::Performance).as_deref(),
            Some("Natif · plaque Lapinux")
        );
        assert_eq!(menu.value(Item::Resume), None);
        menu.hud_engine = HudEngine::MangoHud;
        menu.hud_on = true;
        assert_eq!(
            menu.value(Item::Performance).as_deref(),
            Some("MangoHud · Détaillée")
        );
        assert_eq!(menu.perf_value(5), "Actif");
    }

    #[test]
    fn la_ligne_profil_ouvre_la_liste_complete() {
        let mut menu = MenuState::new("Jeu");
        menu.profiles = vec!["xbox".into(), "nintendo".into()];
        menu.selected = menu.items().iter().position(|item| *item == Item::Profile).unwrap();

        assert_eq!(menu.activate(), None);
        assert_eq!(menu.page, Page::Profiles);
        assert!(menu.panel_height() > HEADER_HEIGHT + FOOTER_HEIGHT);
    }

    fn invite_state() -> SteamFriendsState {
        SteamFriendsState {
            live: true,
            friends: vec![
                SteamFriend {
                    steam_id: 111,
                    name: "Alice".into(),
                    presence: lapinux_core::social::SteamPresence::Online,
                    ..SteamFriend::default()
                },
                SteamFriend {
                    steam_id: 222,
                    name: "Bob".into(),
                    presence: lapinux_core::social::SteamPresence::Offline,
                    ..SteamFriend::default()
                },
            ],
            ..SteamFriendsState::default()
        }
    }

    #[test]
    fn le_carre_amis_ouvre_une_conversation_et_envoie_un_message() {
        let mut menu = MenuState::new("Portal 2");
        assert_eq!(menu.items().len(), 8);
        let (x, y, width, height) = menu.friends_button_rect(40.0, 100.0, 700.0);
        assert!(menu.friends_button_at(40.0, 100.0, 700.0, x + width / 2.0, y + height / 2.0));
        assert!(menu.set_invite(invite_state()));
        menu.set_page(Page::Invite);
        assert_eq!(menu.invite_visible().len(), 2);
        menu.invite_selected = 1;
        assert_eq!(menu.activate(), None);
        assert!(menu.chat_open());
        assert_eq!(menu.chat_friend_id(), Some(222));
        assert!(menu.send_chat().is_none());
        for ch in "Salut Bob".chars() {
            assert!(menu.push_chat_char(ch));
        }
        assert_eq!(
            menu.activate(),
            Some(Action::SendMessage {
                steam_id: 222,
                text: "Salut Bob".into()
            })
        );
        assert_eq!(
            Action::SendMessage {
                steam_id: 222,
                text: "Salut Bob".into()
            }
            .wire(),
            "message:222:Salut Bob"
        );
        assert!(menu.chat_input.is_empty());
        menu.close();
        assert_eq!(menu.page, Page::Invite);
        assert!(!menu.chat_open());
    }

    #[test]
    fn la_conversation_montre_les_messages_recus_et_envoyes() {
        let mut menu = MenuState::new("Portal 2");
        let mut state = invite_state();
        state.chats.insert(
            222,
            vec![
                SteamChatMessage {
                    from: 222,
                    text: "Salut !".into(),
                    at: 0,
                    outgoing: false,
                },
                SteamChatMessage {
                    from: 7,
                    text: "Ça va ?".into(),
                    at: 0,
                    outgoing: true,
                },
            ],
        );
        assert!(menu.set_invite(state));
        menu.set_page(Page::Invite);
        menu.invite_selected = 1;
        assert!(menu.open_chat());
        assert_eq!(menu.chat_friend_name(), "Bob");
        assert_eq!(menu.chat_messages().len(), 2);
        assert_eq!(menu.last_message(222).unwrap().text, "Ça va ?");
    }

    #[test]
    fn la_liste_d_amis_defile_sans_sortir_du_panneau() {
        let mut menu = MenuState::new("Portal 2");
        let mut state = invite_state();
        state.friends = (0..12)
            .map(|index| SteamFriend {
                steam_id: 1000 + index as u64,
                name: format!("Ami {index}"),
                ..SteamFriend::default()
            })
            .collect();
        menu.set_invite(state);
        menu.set_page(Page::Invite);
        menu.invite_selected = 10;
        assert_eq!(menu.invite_offset(), 4);
        assert_eq!(menu.invite_visible().len(), INVITE_ROWS);
        assert_eq!(menu.invite_visible()[0].steam_id, 1004);
        assert!(menu.invite_height() < 900.0);
        assert!(menu.invite_needs_scroll());
        let (_, top, _, height) = menu.invite_scrollbar(0.0, 0.0, 520.0, 800.0).unwrap();
        assert!(menu.invite_scrollbar_thumb(0.0, 0.0, 520.0, 800.0).is_some());
        let list_right = menu.invite_list_right(0.0, 520.0);
        assert_eq!(
            menu.invite_scroll_index_at(
                0.0,
                0.0,
                520.0,
                800.0,
                list_right - 1.0,
                top + height
            ),
            Some(11)
        );
        assert!(menu.scroll_invite(-3));
        assert_eq!(menu.invite_selected, 7);
    }

    #[test]
    fn la_page_d_amis_commence_sous_un_entete_compact() {
        let mut menu = MenuState::new("Portal 2");
        menu.set_invite(invite_state());
        menu.set_page(Page::Invite);
        // Le titre et la liste ne doivent plus être repoussés par la grande
        // capsule du menu principal.
        assert!(menu.invite_row_y(0, 800.0) < 160.0);
        assert!(menu.invite_row_height(800.0) >= 44.0);
        let list = menu.invite_list_width(1280.0);
        assert!(list > 500.0);
        assert!(list <= 1280.0 - 2.0 * PADDING);
    }

    #[test]
    fn choisir_un_profil_envoie_le_nom_exact() {
        let mut menu = MenuState::new("Jeu");
        menu.profiles = vec!["xbox".into(), "nintendo".into()];
        menu.set_page(Page::Profiles);
        menu.profile_selected = 1;

        assert_eq!(
            menu.activate(),
            Some(Action::SelectProfile("nintendo".into()))
        );
        assert_eq!(menu.profile, "nintendo");
        // Le choix déjà actif ne relance pas le service d'entrée.
        assert_eq!(menu.activate(), None);
    }

    #[test]
    fn hud_profiles_cycle_and_have_wire_names() {
        assert_eq!(HudPreset::Simple.cycle(1), HudPreset::Basic);
        assert_eq!(HudPreset::Detailed.cycle(1), HudPreset::Simple);
        assert_eq!(HudPreset::Simple.cycle(-1), HudPreset::Detailed);
        assert_eq!(HudPreset::from_wire("essential"), HudPreset::Basic);
        assert_eq!(HudPreset::from_wire("full"), HudPreset::Detailed);
        assert_eq!(HudPreset::from_wire("simple"), HudPreset::Simple);
        assert_eq!(HudPreset::from_wire("inconnu"), HudPreset::Basic);
        let action = Action::Hud {
            enabled: true,
            profile: HudPreset::Detailed,
            position: HudPosition::TopCenter,
            layout: HudLayout::Block,
            graph: false,
            cores: false,
        };
        assert_eq!(action.wire(), "hud:1:full:top-center:block:0:0");
        assert_eq!(HudLayout::from_wire("inline"), HudLayout::Inline);
        assert_eq!(HudLayout::from_wire("bloc"), HudLayout::Block);
        assert_eq!(HudLayout::Block.cycle(1), HudLayout::Inline);
        assert_eq!(HudLayout::Inline.cycle(1), HudLayout::Block);
    }

    #[test]
    fn hud_positions_cover_the_nine_anchors() {
        assert_eq!(HudPosition::ALL.len(), 9);
        assert_eq!(HudPosition::TopLeft.cycle(1), HudPosition::TopCenter);
        assert_eq!(HudPosition::TopLeft.cycle(-1), HudPosition::BottomRight);
        assert_eq!(
            HudPosition::from_wire("middle-right"),
            HudPosition::MiddleRight
        );
        assert_eq!(HudPosition::from_wire(""), HudPosition::TopLeft);
        assert_eq!(HudPosition::BottomCenter.wire(), "bottom-center");
    }

    #[test]
    fn hud_rows_drive_the_full_action() {
        let mut menu = MenuState::new("Jeu");
        menu.set_page(Page::Performance);
        menu.perf_selected = 2;
        assert_eq!(
            menu.activate(),
            Some(Action::HudEngine { mangohud: true })
        );
        menu.perf_selected = 3;
        assert_eq!(
            menu.activate(),
            Some(Action::Hud {
                enabled: true,
                profile: HudPreset::Simple,
                position: HudPosition::TopLeft,
                layout: HudLayout::Block,
                graph: false,
                cores: false,
            })
        );
        menu.perf_selected = 6;
        assert_eq!(
            menu.adjust(1),
            Some(Action::Hud {
                enabled: true,
                profile: HudPreset::Simple,
                position: HudPosition::TopCenter,
                layout: HudLayout::Block,
                graph: false,
                cores: false,
            })
        );
        menu.perf_selected = 7;
        assert_eq!(
            menu.activate(),
            Some(Action::Hud {
                enabled: true,
                profile: HudPreset::Simple,
                position: HudPosition::TopCenter,
                layout: HudLayout::Inline,
                graph: false,
                cores: false,
            })
        );
        menu.perf_selected = 8;
        assert_eq!(
            menu.activate(),
            Some(Action::Hud {
                enabled: true,
                profile: HudPreset::Simple,
                position: HudPosition::TopCenter,
                layout: HudLayout::Inline,
                graph: true,
                cores: false,
            })
        );
        // L'avant-dernière ligne allume la charge de chaque cœur.
        menu.perf_selected = 9;
        let Some(Action::Hud { cores, .. }) = menu.activate() else {
            panic!("ligne Cœurs du CPU inactive");
        };
        assert!(cores);
    }

    #[test]
    fn inline_accepts_every_anchor() {
        for position in HudPosition::ALL {
            let mut menu = MenuState::new("Jeu");
            menu.set_page(Page::Performance);
            menu.perf_selected = 7;
            menu.activate();
            assert_eq!(menu.hud_layout, HudLayout::Inline);
            menu.place_hud(position);
            assert_eq!(menu.hud_layout, HudLayout::Inline, "{position:?}");
            assert_eq!(menu.hud_position, position);
        }
    }

    #[test]
    fn clicks_find_back_and_position_cells() {
        let mut menu = MenuState::new("Jeu");
        menu.set_page(Page::Performance);
        let panel_x = 40.0;
        let panel_y = 100.0;
        let panel_w = 396.0;
        assert!(menu.back_at(
            panel_x,
            panel_y,
            panel_w,
            panel_x + 60.0,
            panel_y + PERF_BACK_TOP + 10.0
        ));
        assert!(!menu.back_at(
            panel_x,
            panel_y,
            panel_w,
            panel_x + 60.0,
            panel_y + PERF_TOP + 10.0
        ));
        let grid_top = panel_y + menu.perf_grid_top();
        let cell = menu.position_cell_at(
            panel_x,
            panel_y,
            panel_w,
            panel_x + PADDING + 10.0,
            grid_top + 10.0,
        );
        assert_eq!(cell, Some(HudPosition::TopLeft));
        let cell = menu.position_cell_at(
            panel_x,
            panel_y,
            panel_w,
            panel_x + panel_w / 2.0,
            grid_top + PERF_GRID / 2.0,
        );
        assert_eq!(cell, Some(HudPosition::MiddleCenter));
        assert_eq!(
            menu.position_cell_at(panel_x, panel_y, panel_w, panel_x, panel_y),
            None
        );
    }
}
