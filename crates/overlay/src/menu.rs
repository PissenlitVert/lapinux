//! État du quick menu overlay, indépendant du rendu et testable.
//!
//! Le menu compact ouvre les performances, enregistrements, profils et amis
//! depuis leurs lignes. Les réglages avancés restent accessibles à la manette.

use lapinux_core::i18n::{t, t_args};
use lapinux_core::social::{SteamChatMessage, SteamFriend, SteamFriendsState};
use std::collections::BTreeSet;
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
    Screenshot,
    Friends,
    QuitGame,
}

/// Met une majuscule au début du nom affiché sans changer l'identifiant du profil.
pub fn profile_display_name(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

pub const ITEMS: [Item; 8] = [
    Item::Resume,
    Item::Screenshot,
    Item::Recordings,
    Item::Volume,
    Item::Performance,
    Item::Profile,
    Item::Friends,
    Item::QuitGame,
];

/// Les neuf ancrages proposés par le HUD natif, dans l'ordre de lecture.
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
            HudPosition::TopLeft => t("overlay.pos_top_left"),
            HudPosition::TopCenter => t("overlay.pos_top_center"),
            HudPosition::TopRight => t("overlay.pos_top_right"),
            HudPosition::MiddleLeft => t("overlay.pos_middle_left"),
            HudPosition::MiddleCenter => t("overlay.pos_center"),
            HudPosition::MiddleRight => t("overlay.pos_middle_right"),
            HudPosition::BottomLeft => t("overlay.pos_bottom_left"),
            HudPosition::BottomCenter => t("overlay.pos_bottom_center"),
            HudPosition::BottomRight => t("overlay.pos_bottom_right"),
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
/// Retrait du rail de génération d'images : il laisse la place aux deux
/// flèches dessinées aux extrémités.
pub const AFMF_RAIL_INSET: f32 = 16.0;

/// Bornes du rail de génération d'images, dans les mêmes marges que la piste
/// de cadence.
fn afmf_rail_bounds(panel_x: f32, panel_w: f32) -> (f32, f32) {
    (
        panel_x + PADDING + FPS_TRACK_LEFT + AFMF_RAIL_INSET,
        panel_x + panel_w - PADDING - FPS_TRACK_RIGHT - AFMF_RAIL_INSET,
    )
}

/// Disposition du HUD : bloc vertical classique, ou une seule ligne
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
            HudLayout::Block => t("overlay.layout_block"),
            HudLayout::Inline => t("overlay.layout_inline"),
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
        Item::Screenshot => Icon::Camera,
        Item::Friends => Icon::Friend,
        Item::QuitGame => Icon::Cross,
    }
}

/// Hauteur de l'en-tête du menu (capsule et bandeau des statistiques).
pub const HEADER_HEIGHT: f32 = LIVE_TOP + LIVE_HEIGHT + 10.0;
pub const ROW_HEIGHT: f32 = 46.0;
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
/// Hauteur de la capsule panoramique en haut du menu.
pub const CAPSULE_HEIGHT: f32 = 150.0;
/// Plus d'onglets dans l'en-tête.
pub const TABS_HEIGHT: f32 = 0.0;
pub const TAB_GAP: f32 = 0.0;
/// Haut de la carte capsule, au sommet du panneau.
pub const CAPSULE_TOP: f32 = 0.0;
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
/// Titre de section (« Profil du HUD », « Position »).
pub const PERF_SECTION: f32 = 22.0;
/// Grille des neuf ancrages.
pub const PERF_GRID: f32 = 96.0;
/// Bandeau du détail DLSS, en bas du panneau Performances.
pub const PERF_INFO: f32 = 34.0;
/// Lignes réglables du panneau Performances.
pub const PERF_ROWS: usize = 22;

/// Éléments éditables depuis l'overlay, dans l'ordre des lignes 12 à 19.
/// Les courbes ont déjà leurs lignes dédiées (8 et 9).
pub const HUD_ELEMENT_ROWS: [lapinux_core::hud::HudElement; 8] = [
    lapinux_core::hud::HudElement::Fps,
    lapinux_core::hud::HudElement::Frametime,
    lapinux_core::hud::HudElement::Gpu,
    lapinux_core::hud::HudElement::GpuTemp,
    lapinux_core::hud::HudElement::Vram,
    lapinux_core::hud::HudElement::Cpu,
    lapinux_core::hud::HudElement::CpuTemp,
    lapinux_core::hud::HudElement::Ram,
];
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
            // Un plancher absolu de 60 évite aussi de peindre en rouge une
            // session à 60 ou 120 images sur un écran plus rapide.
            Some(fps) if lapinux_core::util::fps_is_comfortable(fps, cap) => FpsStatus::Good,
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
    pub fn recording_label(&self, label: &str) -> Option<String> {
        let state = self.recording?;
        let clock = format_clock(state.secs);
        let label = label.trim();
        Some(if state.paused {
            if label.is_empty() {
                t_args("overlay.paused_clock", &[("clock", &clock)])
            } else {
                t_args(
                    "overlay.paused_clock_label",
                    &[("label", label), ("clock", &clock)],
                )
            }
        } else if label.is_empty() {
            clock
        } else {
            format!("{label} {clock}")
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
            Action::AfmfMultiplier(value) => return format!("afmf:{value}"),
            Action::HudScale(scale) => return format!("hud_scale:{scale:.2}"),
            Action::HudStyle(style) => {
                let body = serde_json::to_string(style.as_ref()).unwrap_or_default();
                return format!("hud_style:{body}");
            }
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
            Action::SendMessage { steam_id, text } => return format!("message:{steam_id}:{text}"),
            Action::InviteFriend(steam_id) => return format!("invite:{steam_id}"),
            Action::CycleProfile => "profile_next",
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
    /// Multiplicateur de génération d'images de la couche afmf-linux, écrit
    /// dans le fichier de contrôle de la session et appliqué à chaud.
    AfmfMultiplier(u32),
    /// Taille de la plaque native, appliquée à chaud.
    HudScale(f32),
    /// Style complet d'un HUD nommé, appliqué à chaud par le daemon :
    /// couleurs, éléments, unités, ordre, ancrage et badge.
    HudStyle(Box<lapinux_core::hud::HudStyle>),
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
    RecordingPause {
        paused: bool,
    },
    /// Termine la capture et garde la vidéo.
    RecordingStop,
    /// Volume du flux audio du jeu, entre 0 et 1.
    GameVolume(f32),
    CycleProfile,
    /// Choix exact d'un profil dans la liste : la partie adopte ce profil
    /// tout de suite, et le jeu le garde pour la prochaine fois.
    SelectProfile(String),
    /// Message Steam envoyé à un ami depuis sa conversation.
    SendMessage {
        steam_id: u64,
        text: String,
    },
    /// Invitation de l'ami dans le lobby Steam du jeu en cours.
    InviteFriend(u64),
    Screenshot,
    /// Coupe toutes les surfaces de performance : plaque et badge.
    CutHud,
    QuitGame,
}

/// Information de survol, indépendante de la sélection clavier/manette.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hover {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub hint: &'static str,
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
    /// Multiplicateur de génération d'images de la session, quand la couche
    /// afmf-linux la porte : 0 coupe la génération, ×2 à ×4 intercalent des
    /// images. None hors de ce moteur, et la ligne reste absente du panneau.
    pub afmf_multiplier: Option<u32>,
    /// Taille de la plaque native, en multiples de sa taille de base.
    pub perf_scale: f32,
    /// Ancrage du HUD à l'écran.
    pub hud_position: HudPosition,
    /// Bloc vertical ou une seule ligne.
    pub hud_layout: HudLayout,
    /// Volume du flux audio du jeu, entre 0 et 1.
    pub volume: f32,
    /// Curseur de volume en cours de glissement : un relevé PipeWire plus
    /// ancien ne doit pas le faire reculer sous le pointeur.
    pub volume_dragging: bool,
    /// Fin de la courte période où un relevé en retard est ignoré, le temps
    /// que la dernière valeur envoyée atteigne le flux.
    pub volume_hold_until: Option<Instant>,
    /// Fin du délai pendant lequel un réglage de performance vient d'être
    /// touché : le relevé du daemon ne doit pas le ramener en arrière.
    pub adjust_hold_until: Option<Instant>,
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
    /// Amis déjà invités dans le lobby courant. Le bouton affiche alors
    /// « Invitation envoyée » et ignore les clics suivants.
    pub invited: BTreeSet<u64>,
    /// Lobby auquel se rapporte `invited` ; un nouveau lobby réarme les
    /// boutons, un retour temporaire à `None` ne les réarme pas.
    pub invited_lobby: Option<u64>,
    /// Dernière invitation refusée par le daemon : le message reste affiché
    /// sur la page d'amis jusqu'au prochain essai ou au prochain lobby.
    pub invite_failure: Option<String>,
    /// Texte en cours de saisie dans la conversation.
    pub chat_input: String,
    pub page: Page,
    pub hover: Option<Hover>,
    pub selected: usize,
    /// Ligne sélectionnée dans le panneau Performances.
    pub perf_selected: usize,
    /// Les réglages détaillés sont sur une seconde page du panneau.
    pub perf_advanced: bool,
    /// Sous-page des éléments : les huit lignes de couleurs et d'activation
    /// ne tiennent pas avec le reste sur un écran 720p.
    pub perf_elements: bool,
    /// Progression d'ouverture 0..1 (animation).
    pub anim: f32,
    /// Impulsion de sélection 0..1 (animation courte).
    pub pulse: f32,
    /// Cadence et durée de la partie, lues sur le daemon.
    pub live: Live,
    /// Style complet du HUD affiché : source unique du rendu.
    pub hud_style: lapinux_core::hud::HudStyle,
    /// HUD nommés fournis par le daemon, dans l'ordre de l'éditeur.
    pub hud_styles: Vec<lapinux_core::hud::HudStyle>,
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
            afmf_multiplier: None,
            perf_scale: 1.15,
            hud_position: HudPosition::TopLeft,
            hud_layout: HudLayout::Block,
            hud_style: lapinux_core::hud::HudStyle::builtin("essential"),
            hud_styles: Vec::new(),
            volume: 1.0,
            volume_dragging: false,
            volume_hold_until: None,
            adjust_hold_until: None,
            recordings_dir: None,
            appid: 0,
            recordings: Vec::new(),
            recording_selected: 0,
            invite: SteamFriendsState::default(),
            invite_selected: 0,
            chat_friend: None,
            invited: BTreeSet::new(),
            invited_lobby: None,
            invite_failure: None,
            chat_input: String::new(),
            page: Page::Menu,
            hover: None,
            selected: 0,
            perf_selected: 0,
            perf_advanced: false,
            perf_elements: false,
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
        // Un nouveau lobby réarme les invitations ; le même lobby garde les
        // boutons « Invitation envoyée » déjà affichés. Un état temporaire
        // sans lobby ne les efface pas, pour ne pas autoriser un renvoi à la
        // reconnexion suivante.
        if let Some(lobby) = invite.lobby {
            if self.invited_lobby != Some(lobby) {
                self.invited.clear();
                self.invited_lobby = Some(lobby);
                self.invite_failure = None;
            }
        }
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
        if self.chat_friend.is_some_and(|id| {
            !self
                .invite
                .friends
                .iter()
                .any(|friend| friend.steam_id == id)
        }) {
            self.close_chat();
        }
        if self.invite_selected >= self.invite.friends.len() {
            self.invite_selected = self.invite.friends.len().saturating_sub(1);
        }
        true
    }

    pub fn next(&mut self) {
        match self.page {
            Page::Performance => {
                let rows = self.perf_visible_rows();
                self.perf_selected = match rows.iter().position(|row| *row == self.perf_selected) {
                    Some(index) => rows[(index + 1) % rows.len()],
                    None => rows[0],
                };
            }
            Page::Recordings => {
                self.recording_selected = (self.recording_selected + 1) % RECORD_ROWS
            }
            Page::Profiles => {
                if !self.profiles.is_empty() {
                    self.profile_selected = (self.profile_selected + 1) % self.profiles.len();
                }
            }
            Page::Invite => {
                if self.chat_friend.is_none() && !self.invite.friends.is_empty() {
                    self.invite_selected = (self.invite_selected + 1) % self.invite.friends.len();
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
                let rows = self.perf_visible_rows();
                self.perf_selected = match rows.iter().position(|row| *row == self.perf_selected) {
                    Some(index) => rows[(index + rows.len() - 1) % rows.len()],
                    None => rows[rows.len() - 1],
                };
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
        self.hover = None;
        self.pulse = 1.0;
        match page {
            Page::Performance => {
                self.perf_advanced = false;
                self.perf_elements = false;
                self.perf_selected = 0;
            }
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
            Page::Menu => {}
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
        if self.page == Page::Invite {
            if delta > 0 && self.invite_available() {
                let steam_id = self.chat_friend.or_else(|| {
                    self.invite
                        .friends
                        .get(self.invite_selected)
                        .map(|friend| friend.steam_id)
                })?;
                return Some(Action::InviteFriend(steam_id));
            }
            return None;
        }
        if self.page == Page::Profiles {
            return None;
        }
        match self.current() {
            Item::Performance => {
                self.set_page(Page::Performance);
                None
            }
            Item::Volume => self.adjust_volume(delta),
            Item::Profile => Some(Action::CycleProfile),
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

    /// Marque un réglage de performance comme tout juste touché : le relevé du
    /// daemon ne doit pas l'écraser avec une valeur plus ancienne pendant que
    /// le doigt est sur la piste.
    fn hold_adjustment(&mut self) {
        self.adjust_hold_until = Some(Instant::now() + Duration::from_millis(900));
    }

    /// Choisit un cran de la piste. Renvoie toujours l'action : le daemon
    /// décide de la valeur réellement applicable.
    pub fn set_fps_index(&mut self, index: usize) -> Option<Action> {
        let index = index.min(FPS_STEPS.len() - 1);
        self.fps_cap = FPS_STEPS[index];
        self.pulse = 1.0;
        self.hold_adjustment();
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

    /// Cran visé par un clic n'importe où sur la ligne de cadence, à
    /// l'abscisse `x` : au-delà de la piste, le cran extrême est choisi. Un
    /// clic un peu à côté de la piste ne déplaçait sinon rien du tout.
    pub fn fps_index_near(&self, x: f32, panel_x: f32, panel_w: f32) -> usize {
        let left = panel_x + PADDING + FPS_TRACK_LEFT;
        let right = panel_x + panel_w - PADDING - FPS_TRACK_RIGHT;
        if right <= left {
            return self.fps_index();
        }
        let index = ((x.clamp(left, right) - left) / (right - left))
            * (FPS_STEPS.len() - 1) as f32;
        (index.round() as usize).min(FPS_STEPS.len() - 1)
    }

    /// Cran du rail de génération d'images : `0` coupe la génération, puis
    /// viennent les multiplicateurs ×2, ×3 et ×4.
    pub fn afmf_index(&self) -> usize {
        let steps = lapinux_core::config::AFMF_MULTIPLIERS;
        self.afmf_multiplier
            .and_then(|value| steps.iter().position(|step| *step == value))
            .unwrap_or(0)
    }

    /// Position du curseur sur le rail, de 0 à 1.
    pub fn afmf_ratio(&self) -> f32 {
        self.afmf_index() as f32 / (lapinux_core::config::AFMF_MULTIPLIERS.len() - 1) as f32
    }

    /// Cran visé par un clic sur le rail, `None` en dehors de ses marges.
    pub fn afmf_index_at(&self, x: f32, panel_x: f32, panel_w: f32) -> Option<usize> {
        let (left, right) = afmf_rail_bounds(panel_x, panel_w);
        if right <= left || x < left - 14.0 || x > right + 14.0 {
            return None;
        }
        let ratio = ((x - left) / (right - left)).clamp(0.0, 1.0);
        let last = lapinux_core::config::AFMF_MULTIPLIERS.len() - 1;
        Some((ratio * last as f32).round().min(last as f32) as usize)
    }

    /// Cran visé par un clic n'importe où sur la ligne : au-delà du rail, le
    /// cran extrême. C'est aussi la flèche : à gauche elle coupe, à droite
    /// elle monte jusqu'à ×4.
    pub fn afmf_index_near(&self, x: f32, panel_x: f32, panel_w: f32) -> usize {
        let (left, right) = afmf_rail_bounds(panel_x, panel_w);
        if right <= left {
            return self.afmf_index();
        }
        let last = lapinux_core::config::AFMF_MULTIPLIERS.len() - 1;
        let ratio = ((x.clamp(left, right) - left) / (right - left)).clamp(0.0, 1.0);
        (ratio * last as f32).round().min(last as f32) as usize
    }

    /// Place le curseur sur un cran du rail. Hors afmf-linux, la couche n'est
    /// pas chargée : la valeur part quand même au daemon, qui explique
    /// pourquoi elle ne s'applique pas.
    pub fn set_afmf_index(&mut self, index: usize) -> Option<Action> {
        let steps = lapinux_core::config::AFMF_MULTIPLIERS;
        let index = index.min(steps.len() - 1);
        let value = steps[index];
        if self.afmf_multiplier.is_some() {
            self.afmf_multiplier = Some(value);
        }
        self.pulse = 1.0;
        self.hold_adjustment();
        Some(Action::AfmfMultiplier(value))
    }

    fn adjust_perf(&mut self, delta: i32) -> Option<Action> {
        match self.perf_selected {
            // L'interrupteur est la première ligne : il reste sous les yeux.
            0 => {
                let show = !self.perf_hud;
                self.perf_hud = show;
                self.pulse = 1.0;
                Some(Action::HudVisible(show))
            }
            1 => {
                let next =
                    (self.fps_index() as i32 + delta).clamp(0, FPS_STEPS.len() as i32 - 1) as usize;
                self.set_fps_index(next)
            }
            2 => {
                // La couche relit ce fichier à chaque image : le changement
                // s'applique en jeu, sans relancer le jeu ni la session. Le
                // rail bloque à ses extrémités, comme la piste de cadence.
                let last = lapinux_core::config::AFMF_MULTIPLIERS.len() as i32 - 1;
                let next = (self.afmf_index() as i32 + delta.signum()).clamp(0, last) as usize;
                self.set_afmf_index(next)
            }
            3..=5 => {
                self.perf_selected = 3;
                self.cycle_hud_style(delta)
            }
            6 => {
                self.place_hud(self.hud_position.cycle(delta.signum()));
                Some(self.hud_style_action())
            }
            7 => {
                self.set_hud_layout(self.hud_layout.cycle(delta.signum()));
                Some(self.hud_style_action())
            }
            8 => {
                self.toggle_hud_element(lapinux_core::hud::HudElement::Graph);
                Some(self.hud_style_action())
            }
            9 => {
                self.toggle_hud_element(lapinux_core::hud::HudElement::Cores);
                Some(self.hud_style_action())
            }
            10 => self.set_scale(self.perf_scale + delta as f32 * 0.05),
            12..=19 => {
                if !self.perf_elements {
                    return None;
                }
                let element = HUD_ELEMENT_ROWS[self.perf_selected - 12];
                self.toggle_hud_element(element);
                Some(self.hud_style_action())
            }
            20 => {
                self.cycle_hud_theme(delta.signum());
                Some(self.hud_style_action())
            }
            21 => {
                if !self.perf_elements {
                    return None;
                }
                self.perf_elements = false;
                self.perf_selected = 11;
                None
            }
            11 => {
                self.perf_advanced = true;
                // Première ligne du mode détaillé : la sélection reste sur
                // une ligne visible du panneau.
                self.perf_selected = 2;
                None
            }
            _ => None,
        }
    }

    fn cycle_hud_theme(&mut self, delta: i32) {
        let current = lapinux_core::hud::HUD_THEMES
            .iter()
            .position(|(id, _)| lapinux_core::hud::colors_for_theme(id) == self.hud_style.colors);
        let index = current.unwrap_or(0);
        let next = (index as i32 + delta.signum())
            .rem_euclid(lapinux_core::hud::HUD_THEMES.len() as i32) as usize;
        self.hud_style
            .apply_theme(lapinux_core::hud::HUD_THEMES[next].0);
        self.pulse = 1.0;
    }

    /// Fait tourner la couleur d'un élément dans la palette rapide. Entrée
    /// sur une ligne d'élément rend donc le changement de couleur accessible
    /// à la manette, sans clavier.

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
    /// bloc et relance son processus séparé. En natif, le style est la source
    /// unique et part avec les couleurs du badge.
    pub fn record_badge_action(&mut self) -> Action {
        self.hud_style.badge.enabled = self.record_badge;
        self.hud_style.badge.position = self.record_position.wire().to_string();
        self.hud_style.badge.scale = self.record_scale;
        Action::RecordBadge {
            enabled: self.record_badge,
            position: self.record_position,
            scale: self.record_scale,
        }
    }

    pub fn set_record_scale(&mut self, scale: f32) -> Option<Action> {
        self.record_scale = scale.clamp(0.65, 2.0);
        self.hud_style.badge.scale = self.record_scale;
        self.hud_style.badge.enabled = self.record_badge;
        self.hud_style.badge.position = self.record_position.wire().to_string();
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
            Item::Screenshot => Some(Action::Screenshot),
            Item::Friends => {
                self.set_page(Page::Invite);
                None
            }
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
        // Un refus concernant un autre ami ne doit pas s'afficher dans cette
        // conversation.
        self.invite_failure = None;
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
            .and_then(|id| {
                self.invite
                    .friends
                    .iter()
                    .find(|friend| friend.steam_id == id)
            })
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
            .and_then(|id| {
                self.invite
                    .friends
                    .iter()
                    .find(|friend| friend.steam_id == id)
            })
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

    /// Bouton « Inviter » d'une fiche d'ami, en coordonnées écran. Il existe
    /// pendant une partie ; sans lobby connu, le daemon refuse et l'overlay
    /// affiche l'explication au lieu de cacher le bouton.
    pub fn invite_button_rect(
        &self,
        index: usize,
        panel_x: f32,
        panel_y: f32,
        panel_w: f32,
        panel_h: f32,
    ) -> Option<(f32, f32, f32, f32)> {
        if self.page != Page::Invite || !self.invite_available() {
            return None;
        }
        let row = index.checked_sub(self.invite_offset())?;
        if row >= self.invite_visible().len() {
            return None;
        }
        let list_left = panel_x + PADDING;
        let list_width = self.invite_list_width(panel_w);
        let row_h = self.invite_row_height(panel_h);
        let top = panel_y + self.invite_row_y(row, panel_h);
        let width = 136.0;
        let height = 26.0;
        Some((
            list_left + list_width - width - 28.0,
            top + (row_h - height) / 2.0,
            width,
            height,
        ))
    }

    /// Ami dont le bouton « Inviter » est visé par un clic.
    pub fn invite_button_at(
        &self,
        x: f32,
        y: f32,
        panel_x: f32,
        panel_y: f32,
        panel_w: f32,
        panel_h: f32,
    ) -> Option<u64> {
        if self.page != Page::Invite || self.chat_open() {
            return None;
        }
        let index = self.invite_hit_row(y - panel_y, panel_h)?;
        let (left, top, width, height) =
            self.invite_button_rect(index, panel_x, panel_y, panel_w, panel_h)?;
        if x >= left && x <= left + width && y >= top && y <= top + height {
            self.invite.friends.get(index).map(|friend| friend.steam_id)
        } else {
            None
        }
    }

    /// Une invitation a un sens pendant une partie : avec un lobby, elle part
    /// directement ; sans lobby, le daemon la refuse et l'overlay affiche son
    /// explication.
    pub fn invite_available(&self) -> bool {
        self.invite.lobby.is_some() || self.appid != 0
    }

    /// Vrai quand cet ami a déjà reçu une invitation dans le lobby courant.
    pub fn invite_sent(&self, steam_id: u64) -> bool {
        self.invite.lobby.is_some()
            && self.invited_lobby == self.invite.lobby
            && self.invited.contains(&steam_id)
    }

    /// Retour affiché par le bouton : il passe au vert dès l'envoi avec un
    /// lobby, et reste en attente de la réponse du daemon sans lobby.
    pub fn invite_button_label(&self, steam_id: u64) -> &'static str {
        if self.invite_sent(steam_id) {
            t("overlay.invite_sent")
        } else if self.invited.contains(&steam_id) {
            t("overlay.sending")
        } else {
            t("overlay.invite")
        }
    }

    /// Marque l'invitation envoyée, ou en attente quand aucun lobby n'est
    /// encore connu. Rend `false` quand l'ami a déjà une demande en cours :
    /// le clic ne repart pas.
    pub fn mark_invite_sent(&mut self, steam_id: u64) -> bool {
        if let Some(lobby) = self.invite.lobby {
            if self.invited_lobby != Some(lobby) {
                self.invited.clear();
                self.invited_lobby = Some(lobby);
            }
        }
        if !self.invited.insert(steam_id) {
            return false;
        }
        self.invite_failure = None;
        self.pulse = 1.0;
        true
    }

    /// Message d'échec de la dernière invitation refusée par le daemon.
    pub fn invite_failure(&self) -> Option<&str> {
        self.invite_failure.as_deref()
    }

    /// Applique le refus du daemon : l'ami sort des invitations envoyées, le
    /// bouton redevient « Inviter », et le message reste affiché sur la page
    /// d'amis jusqu'au prochain essai ou au prochain lobby.
    pub fn apply_action_failure(&mut self, wire: &str, error: &str) -> bool {
        let Some(steam_id) = wire
            .strip_prefix("invite:")
            .and_then(|id| id.trim().parse::<u64>().ok())
        else {
            return false;
        };
        self.invited.remove(&steam_id);
        self.invite_failure = Some(t_args("overlay.invite_failed", &[("error", error)]));
        self.pulse = 1.0;
        true
    }

    /// Libellé du bouton de la conversation ouverte.
    pub fn chat_invite_button_label(&self) -> &'static str {
        self.chat_friend.map_or(t("overlay.invite"), |steam_id| {
            self.invite_button_label(steam_id)
        })
    }

    /// Bouton « Inviter » dans la conversation ouverte. Il apparaît pendant
    /// une partie, même sans lobby connu ; le clic vise cet ami.
    pub fn chat_invite_button_rect(
        &self,
        panel_x: f32,
        panel_y: f32,
        panel_w: f32,
    ) -> Option<(f32, f32, f32, f32)> {
        if self.page != Page::Invite || !self.chat_open() || !self.invite_available() {
            return None;
        }
        let width = 136.0;
        let height = 26.0;
        Some((
            panel_x + panel_w - PADDING - width,
            panel_y + INVITE_TITLE_TOP - 2.0,
            width,
            height,
        ))
    }

    /// Ami dont le bouton « Inviter » de la conversation est visé.
    pub fn chat_invite_button_at(
        &self,
        x: f32,
        y: f32,
        panel_x: f32,
        panel_y: f32,
        panel_w: f32,
    ) -> Option<u64> {
        let (left, top, width, height) = self.chat_invite_button_rect(panel_x, panel_y, panel_w)?;
        if x >= left && x <= left + width && y >= top && y <= top + height {
            self.chat_friend
        } else {
            None
        }
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
        if (12..=19).contains(&self.perf_selected) {
            // Les couleurs d’éléments se règlent dans l’application : ici,
            // Entrée se contente d’activer ou de couper la mesure, comme
            // les flèches, pour ne pas transformer l’overlay en éditeur.
            let element = HUD_ELEMENT_ROWS[self.perf_selected - 12];
            self.toggle_hud_element(element);
            return Some(self.hud_style_action());
        }
        if self.perf_selected == 20 {
            self.cycle_hud_theme(1);
            return Some(self.hud_style_action());
        }
        if self.perf_selected == 21 {
            self.perf_elements = !self.perf_elements;
            self.perf_selected = if self.perf_elements { 12 } else { 11 };
            return None;
        }
        match self.perf_selected {
            0 | 1 => self.adjust_perf(1),
            // Cliquer la génération d'images la fait avancer d'un cran : sans
            // cette ligne, un clic ne faisait rien du tout et seul le clavier
            // permettait de la couper.
            2 => self.adjust_perf(1),
            3 => self.adjust_perf(1),
            6 => {
                self.place_hud(self.hud_position.cycle(1));
                Some(self.hud_style_action())
            }
            7 => {
                self.set_hud_layout(self.hud_layout.cycle(1));
                Some(self.hud_style_action())
            }
            8 => {
                self.toggle_hud_element(lapinux_core::hud::HudElement::Graph);
                Some(self.hud_style_action())
            }
            9 => {
                self.toggle_hud_element(lapinux_core::hud::HudElement::Cores);
                Some(self.hud_style_action())
            }
            10 => self.set_scale(self.perf_scale + 0.05),
            11 => {
                self.perf_advanced = true;
                // Première ligne du mode détaillé : la sélection reste sur
                // une ligne visible du panneau.
                self.perf_selected = 2;
                None
            }
            _ => None,
        }
    }

    /// Applique un ancrage au HUD natif.

    /// Aligne les champs d'affichage sur le style, source unique du rendu.
    fn sync_style_fields(&mut self) {
        self.hud_position = HudPosition::from_wire(&self.hud_style.position);
        self.hud_layout = HudLayout::from_wire(&self.hud_style.layout);
        self.perf_scale = self.hud_style.scale;
        self.record_badge = self.hud_style.badge.enabled;
        self.record_position = HudPosition::from_wire(&self.hud_style.badge.position);
        self.record_scale = self.hud_style.badge.scale;
    }

    fn hud_style_action(&self) -> Action {
        Action::HudStyle(Box::new(self.hud_style.clone()))
    }

    /// Installe un style complet et renvoie l'action qui l'applique à chaud.
    pub fn set_hud_style(&mut self, mut style: lapinux_core::hud::HudStyle) -> Action {
        style.normalize();
        self.hud_style = style;
        self.sync_style_fields();
        self.pulse = 1.0;
        Action::HudStyle(Box::new(self.hud_style.clone()))
    }

    /// Fait tourner les HUD nommés fournis par le daemon. Sans HUD nommé, les
    /// presets intégrés restent disponibles pour ne pas laisser un menu vide.
    pub fn cycle_hud_style(&mut self, delta: i32) -> Option<Action> {
        let styles: Vec<lapinux_core::hud::HudStyle> = if self.hud_styles.is_empty() {
            vec![
                lapinux_core::hud::HudStyle::builtin("simple"),
                lapinux_core::hud::HudStyle::builtin("essential"),
                lapinux_core::hud::HudStyle::builtin("full"),
            ]
        } else {
            self.hud_styles.clone()
        };
        if styles.is_empty() {
            return None;
        }
        let index = styles
            .iter()
            .position(|style| style.id == self.hud_style.id)
            .unwrap_or(0);
        let next = (index as i32 + delta.signum()).rem_euclid(styles.len() as i32) as usize;
        let mut style = styles[next].clone();
        // Le badge est un réglage de session : changer de HUD ne doit pas le
        // rallumer ni le déplacer sans que l'utilisateur l'ait demandé.
        style.badge = self.hud_style.badge.clone();
        Some(self.set_hud_style(style))
    }

    /// position du HUD en ligne : les neuf ancrages tiennent dans les deux
    /// dispositions.
    pub fn place_hud(&mut self, position: HudPosition) {
        self.hud_position = position;
        self.hud_style.position = position.wire().to_string();
        self.pulse = 1.0;
    }

    pub fn set_hud_layout(&mut self, layout: HudLayout) {
        self.hud_layout = layout;
        self.hud_style.layout = layout.wire().to_string();
        self.pulse = 1.0;
    }

    fn element_color_slot(kind: lapinux_core::hud::HudElement) -> lapinux_core::hud::HudColorSlot {
        use lapinux_core::hud::{HudColorSlot, HudElement};
        match kind {
            HudElement::Fps => HudColorSlot::Fps,
            HudElement::Frametime => HudColorSlot::Frametime,
            HudElement::Gpu => HudColorSlot::Gpu,
            HudElement::GpuTemp | HudElement::CpuTemp => HudColorSlot::Temp,
            HudElement::Vram => HudColorSlot::Vram,
            HudElement::Cpu => HudColorSlot::Cpu,
            HudElement::Ram => HudColorSlot::Ram,
            HudElement::Cores => HudColorSlot::Cores,
            HudElement::Graph => HudColorSlot::Graph,
        }
    }

    pub fn element_color(&self, kind: lapinux_core::hud::HudElement) -> String {
        self.hud_style
            .colors
            .get(Self::element_color_slot(kind))
            .to_string()
    }

    #[allow(dead_code)]
    fn cycle_element_color(&mut self, kind: lapinux_core::hud::HudElement) {
        let slot = Self::element_color_slot(kind);
        let current = self.hud_style.colors.get(slot).to_ascii_uppercase();
        let index = lapinux_core::hud::QUICK_COLORS
            .iter()
            .position(|color| color.eq_ignore_ascii_case(&current))
            .unwrap_or(0);
        let next = (index + 1) % lapinux_core::hud::QUICK_COLORS.len();
        self.hud_style
            .colors
            .set(slot, lapinux_core::hud::QUICK_COLORS[next]);
        self.pulse = 1.0;
    }

    pub fn toggle_hud_element(&mut self, kind: lapinux_core::hud::HudElement) {
        self.hud_style.normalize();
        if let Some(element) = self.hud_style.element_mut(kind) {
            element.enabled = !element.enabled;
        }
        self.sync_style_fields();
        self.pulse = 1.0;
    }

    /// Taille de la plaque native, bornée comme le réglage des Captures.
    pub fn set_scale(&mut self, scale: f32) -> Option<Action> {
        let next = scale.clamp(0.75, 2.0);
        // En butée, la touche maintenue ne doit pas relancer le HUD en boucle.
        if (next - self.perf_scale).abs() < 0.001 {
            return None;
        }
        self.perf_scale = next;
        self.hud_style.scale = next;
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

    /// Referme le panneau ouvert (Performances ou Enregistrements), sinon
    /// l'overlay. Dans une conversation, Échap revient d'abord à la liste.
    pub fn close(&mut self) {
        if self.page == Page::Performance && self.perf_elements {
            self.perf_elements = false;
            self.perf_selected = 21;
        } else if self.page == Page::Performance && self.perf_advanced {
            self.perf_advanced = false;
            self.perf_selected = 11;
        } else if self.page == Page::Invite && self.chat_friend.is_some() {
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

    /// Version réellement utilisée, absente pour un jeu natif ou non renseigné.
    pub fn active_proton(&self) -> Option<&str> {
        let value = self.proton.trim();
        if value.is_empty()
            || value.eq_ignore_ascii_case("auto")
            || value.eq_ignore_ascii_case("natif")
            || value.eq_ignore_ascii_case("native")
        {
            None
        } else {
            Some(value)
        }
    }

    pub fn label(&self, item: Item) -> String {
        match item {
            Item::Resume => t("overlay.resume").into(),
            Item::Performance => t("overlay.performance").into(),
            Item::Recordings => t("overlay.recordings").into(),
            Item::Volume => t("overlay.volume").into(),
            Item::Profile => t_args(
                "overlay.profile",
                &[("name", &profile_display_name(&self.profile))],
            ),
            Item::Screenshot => t("overlay.screenshot").into(),
            Item::Friends => t("overlay.friends").into(),
            Item::QuitGame => t_args("overlay.quit_game", &[("name", &self.game_name)]),
        }
    }

    /// Valeur affichée à droite de la ligne, quand il y en a une.
    pub fn value(&self, item: Item) -> Option<String> {
        match item {
            Item::Resume => None,
            Item::Performance => Some(t_args(
                "overlay.hud_native",
                &[("name", &self.hud_style.name)],
            )),
            Item::Recordings => Some(match self.live.recording {
                Some(state) if state.paused => t("overlay.recording_paused").into(),
                Some(_) => t("overlay.recording_running").into(),
                None => t("overlay.recording_stopped").into(),
            }),
            Item::Volume => Some(format!("{:.0} %", self.volume * 100.0)),
            Item::Profile => Some(profile_display_name(&self.profile)),
            Item::Screenshot => None,
            Item::Friends => Some(t_args(
                "overlay.friends_online",
                &[(
                    "count",
                    &self
                        .invite
                        .friends
                        .iter()
                        .filter(|f| f.presence.online())
                        .count()
                        .to_string(),
                )],
            )),
            Item::QuitGame => Some(t("overlay.quit_short").into()),
        }
    }

    pub fn hint(&self, item: Item) -> &'static str {
        match item {
            Item::Resume => t("overlay.enter"),
            Item::Performance => t("overlay.enter"),
            Item::Recordings => t("overlay.enter"),
            Item::Volume => "← →",
            Item::Profile => t("overlay.enter_list"),
            Item::Screenshot => t("overlay.enter"),
            Item::Friends => t("overlay.enter"),
            Item::QuitGame => t("overlay.enter"),
        }
    }

    /// Nom de la ligne du panneau Performances.
    pub fn perf_label(&self, row: usize) -> String {
        if self.page == Page::Recordings {
            return match row {
                0 => match self.live.recording {
                    Some(state) if state.paused => t("overlay.resume").into(),
                    Some(_) => t("overlay.pause").into(),
                    None => t("overlay.start_recording").into(),
                },
                1 => t("overlay.stop_recording").into(),
                2 => t("overlay.record_hud").into(),
                3 => t("overlay.location").into(),
                4 => t("overlay.size").into(),
                _ => String::new(),
            };
        }
        match row {
            0 => t("overlay.hud_performance").into(),
            1 => t("overlay.fps_limit").into(),
            2 => t("overlay.frame_generation").into(),
            3 => t("overlay.hud_profile").into(),
            6 => t("overlay.location").into(),
            7 => t("overlay.layout").into(),
            8 => t("overlay.graph").into(),
            9 => t("overlay.cpu_cores").into(),
            10 => t("overlay.hud_size").into(),
            11 => t("overlay.advanced").into(),
            12..=19 => HUD_ELEMENT_ROWS[row - 12].label().into(),
            20 => t("overlay.theme").into(),
            21 => {
                if self.perf_elements {
                    t("overlay.back_settings").into()
                } else {
                    t("overlay.hud_elements").into()
                }
            }
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
                    t("overlay.infinite").into()
                } else {
                    format!("{} FPS", self.fps_cap)
                }
            }
            2 => match self.afmf_multiplier {
                Some(0) => t("overlay.disabled_f").into(),
                Some(value) => format!("×{value}"),
                None => t("overlay.inactive").into(),
            },
            3 => self.hud_style.name.clone(),
            6 => HudPosition::from_wire(&self.hud_style.position)
                .label()
                .into(),
            7 => HudLayout::from_wire(&self.hud_style.layout).label().into(),
            8 => {
                if self.hud_style.enabled(lapinux_core::hud::HudElement::Graph) {
                    t("overlay.enabled_f").into()
                } else {
                    t("overlay.disabled_f").into()
                }
            }
            9 => {
                if self.hud_style.enabled(lapinux_core::hud::HudElement::Cores) {
                    t("overlay.enabled_m").into()
                } else {
                    t("overlay.disabled_m").into()
                }
            }
            10 => format!("{:.2} ×", self.hud_style.scale),
            12..=19 => {
                let element = HUD_ELEMENT_ROWS[row - 12];
                if self.hud_style.enabled(element) {
                    t("overlay.enabled").into()
                } else {
                    t("overlay.disabled").into()
                }
            }
            20 => t("overlay.change").into(),
            21 => {
                if self.perf_elements {
                    String::new()
                } else {
                    t("overlay.edit").into()
                }
            }
            _ => String::new(),
        }
    }

    /// Sous-titre du panneau Performances.
    pub fn perf_subtitle(&self, row: usize) -> String {
        if self.page == Page::Recordings {
            return match row {
                0 => match self.live.recording {
                    Some(state) if state.paused => t("overlay.resume_capture").into(),
                    Some(_) => t("overlay.suspend_capture").into(),
                    None => t("overlay.start_capture").into(),
                },
                1 => {
                    if self.live.recording.is_some() {
                        t("overlay.finish_keep").into()
                    } else {
                        t("overlay.no_recording").into()
                    }
                }
                2 => t("overlay.toggle_record_hud").into(),
                3 => t("overlay.badge_anchor").into(),
                4 => String::new(),
                _ => String::new(),
            };
        }
        match row {
            0 => t("overlay.toggle_plate").into(),
            // Le rail occupe le bas de la carte, comme la piste de cadence :
            // pas d'aide en dessous, elle passerait dessous.
            2 => String::new(),
            3 => t("overlay.choose_detail").into(),
            4 => t("overlay.memory_temps").into(),
            5 => t("overlay.vram_detail").into(),
            6 => t("overlay.hud_anchor").into(),
            7 => t("overlay.one_row_block").into(),
            8 => t("overlay.frametime_curve").into(),
            9 => t("overlay.per_core").into(),
            10 => String::new(),
            12..=19 => t("overlay.element_hint").into(),
            20 => t("overlay.apply_theme").into(),
            21 => {
                if self.perf_elements {
                    t("overlay.restore_hud").into()
                } else {
                    t("overlay.customize_elements").into()
                }
            }
            _ => String::new(),
        }
    }

    /// Ordonnée du haut de la ligne `index` du menu, relative au panneau.
    pub fn row_y(&self, index: usize) -> f32 {
        HEADER_HEIGHT + index as f32 * ROW_HEIGHT
    }

    /// Identifiants des lignes visibles et sélectionnables à la manette.
    pub fn perf_visible_rows(&self) -> &'static [usize] {
        if self.perf_elements {
            &[12, 13, 14, 15, 16, 17, 18, 19, 20, 21]
        } else if self.perf_advanced {
            // La génération d'images reste réglable dans le mode détaillé :
            // sans elle, ce mode la faisait disparaître du panneau.
            &[2, 7, 8, 9, 10, 20, 21]
        } else {
            // Toujours visible, même hors afmf-linux : la ligne dit alors
            // « Inactive » au lieu de disparaître sans explication.
            &[0, 1, 2, 3, 6, 11]
        }
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
        let visible = self.perf_visible_rows();
        let slot = visible.iter().position(|row| *row == index).unwrap_or(0);
        PERF_TOP + slot as f32 * PERF_ROW
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
    pub fn perf_back_rect(&self, panel_x: f32, panel_y: f32, panel_w: f32) -> (f32, f32, f32, f32) {
        (
            panel_x + PADDING,
            panel_y + PERF_BACK_TOP,
            panel_w - 2.0 * PADDING,
            PERF_BACK_HEIGHT,
        )
    }

    /// Hauteur du panneau Performances : il couvre la grille et le détail DLSS.
    pub fn perf_height(&self) -> f32 {
        PERF_TOP
            + self.perf_visible_rows().len() as f32 * PERF_ROW
            + PERF_INFO
            + 16.0
            + FOOTER_HEIGHT
    }

    pub fn panel_height(&self) -> f32 {
        match self.page {
            Page::Menu => {
                HEADER_HEIGHT + self.items().len() as f32 * ROW_HEIGHT + 6.0 + FOOTER_HEIGHT
            }
            Page::Performance => self.perf_height(),
            Page::Recordings => self.recordings_height(),
            Page::Profiles => self.profiles_height(),
            Page::Invite => self.invite_height(),
        }
    }

    /// Haut de la ligne d'un profil dans la page « Profils manettes ».
    pub fn profile_row_y(&self, index: usize) -> f32 {
        HEADER_HEIGHT
            + PROFILE_TOP
            + index.saturating_sub(self.profile_offset()) as f32 * ROW_HEIGHT
    }

    /// Ligne de profil visée par `y` (relatif au panneau).
    pub fn profile_hit_row(&self, y: f32) -> Option<usize> {
        if self.page != Page::Profiles {
            return None;
        }
        for index in self.profile_offset()..(self.profile_offset() + 8).min(self.profiles.len()) {
            let top = self.profile_row_y(index);
            if y >= top && y < top + ROW_HEIGHT {
                return Some(index);
            }
        }
        None
    }

    /// Hauteur de la page « Profils manettes » : titre, une ligne par profil
    /// et le pied de page.
    pub fn profile_offset(&self) -> usize {
        self.profile_selected
            .saturating_sub(7)
            .min(self.profiles.len().saturating_sub(8))
    }

    pub fn profiles_height(&self) -> f32 {
        HEADER_HEIGHT
            + PROFILE_TOP
            + self.profiles.len().min(8) as f32 * ROW_HEIGHT
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

    /// Reprend l'identité de la session. Le style du HUD garde sa source
    /// unique dans `--hud-style` : aucun champ de session ne le duplique.
    pub fn apply_session_hud(&mut self, info: &lapinux_core::model::SessionInfo) -> bool {
        self.apply_session_identity(info)
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
        // Pendant et juste après un glissement, la valeur du panneau fait foi :
        // relire un relevé plus ancien ramenait le curseur en arrière sous le
        // doigt, puis à l'ancienne limite.
        let held = self
            .adjust_hold_until
            .is_some_and(|until| Instant::now() < until);
        if !held && self.fps_cap != info.fps_cap {
            self.fps_cap = info.fps_cap;
            changed = true;
        }
        if !held && self.afmf_multiplier != info.afmf_multiplier {
            self.afmf_multiplier = info.afmf_multiplier;
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
        for &index in self.perf_visible_rows() {
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
    fn menu_does_not_offer_changing_proton() {
        let mut menu = MenuState::new("Jeu");
        for _ in 0..menu.items().len() {
            assert!(!menu.label(menu.current()).contains("Proton"));
            menu.next();
        }
        assert_eq!(menu.current(), Item::Resume);
    }

    #[test]
    fn performance_tab_opens_with_enter() {
        let mut menu = MenuState::new("Jeu");
        menu.selected = menu
            .items()
            .iter()
            .position(|item| *item == Item::Performance)
            .unwrap();
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
    fn afmf_multiplier_row_cycles_and_wires_the_live_action() {
        let mut menu = MenuState::new("Jeu");
        menu.set_page(Page::Performance);
        // La ligne est là même hors afmf-linux, avec son état : sinon rien
        // dans l'overlay ne montre ni ne règle la génération d'images.
        assert!(menu.perf_visible_rows().contains(&2));
        assert_eq!(menu.perf_value(2), "Inactive");
        // Le mode détaillé la garde aussi.
        menu.perf_advanced = true;
        assert!(menu.perf_visible_rows().contains(&2));
        menu.perf_advanced = false;
        menu.afmf_multiplier = Some(2);
        assert!(menu.perf_visible_rows().contains(&2));
        assert_eq!(menu.perf_label(2), "Génération d'images");
        assert_eq!(menu.perf_value(2), "×2");
        menu.perf_selected = 2;
        // Un clic sur la ligne avance d'un cran : c'est le seul moyen à la
        // souris de couper la génération.
        assert_eq!(menu.activate(), Some(Action::AfmfMultiplier(3)));
        assert_eq!(menu.adjust(1), Some(Action::AfmfMultiplier(4)));
        // Le rail bloque à son extrémité haute, puis se coupe au premier cran.
        assert_eq!(menu.adjust(1), Some(Action::AfmfMultiplier(4)));
        assert_eq!(menu.set_afmf_index(0), Some(Action::AfmfMultiplier(0)));
        assert_eq!(menu.perf_value(2), "Désactivée");
        assert_eq!(menu.adjust(1), Some(Action::AfmfMultiplier(2)));
        assert_eq!(Action::AfmfMultiplier(4).wire(), "afmf:4");
    }

    #[test]
    fn le_rail_de_generation_choisit_entre_quatre_crans() {
        let mut menu = MenuState::new("Jeu");
        menu.set_page(Page::Performance);
        menu.afmf_multiplier = Some(2);
        let panel_x = 40.0;
        let panel_w = 396.0;
        let left = panel_x + PADDING + FPS_TRACK_LEFT + AFMF_RAIL_INSET;
        let right = panel_x + panel_w - PADDING - FPS_TRACK_RIGHT - AFMF_RAIL_INSET;
        assert_eq!(menu.afmf_index(), 1, "×2 est le deuxième cran");
        assert_eq!(menu.afmf_index_at(left, panel_x, panel_w), Some(0));
        assert_eq!(menu.afmf_index_at(right, panel_x, panel_w), Some(3));
        assert_eq!(menu.afmf_index_at((left + right) / 2.0, panel_x, panel_w), Some(2));
        // Les flèches des extrémités : à gauche on coupe, à droite on monte à ×4.
        assert_eq!(menu.afmf_index_near(panel_x + 20.0, panel_x, panel_w), 0);
        assert_eq!(
            menu.afmf_index_near(panel_x + panel_w - 4.0, panel_x, panel_w),
            3
        );
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
    fn un_clic_hors_de_la_piste_choisit_le_cran_le_plus_proche() {
        let menu = MenuState::new("Jeu");
        let panel_x = 40.0;
        let panel_w = 396.0;
        // Un clic sur le libellé (à gauche) va au minimum, un clic après la
        // piste (à droite) au maximum : le curseur bouge toujours.
        assert_eq!(menu.fps_index_near(panel_x + 20.0, panel_x, panel_w), 0);
        assert_eq!(
            menu.fps_index_near(panel_x + panel_w - 4.0, panel_x, panel_w),
            FPS_STEPS.len() - 1
        );
    }

    #[test]
    fn scale_lives_in_the_performance_panel() {
        let mut menu = MenuState::new("Jeu");
        menu.set_page(Page::Performance);
        // L'échelle se règle sur sa ligne et reste bornée.
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
    fn active_proton_omits_native_or_unknown_sessions() {
        let mut menu = MenuState::new("Jeu");
        for value in ["", "  ", "auto", "natif", "Native"] {
            menu.proton = value.into();
            assert_eq!(menu.active_proton(), None);
        }
        menu.proton = "  GE-Proton9-20  ".into();
        assert_eq!(menu.active_proton(), Some("GE-Proton9-20"));
    }

    #[test]
    fn labels_and_hints_are_stable() {
        let mut menu = MenuState::new("Team Fortress 2");
        menu.fps_cap = 120;
        assert_eq!(menu.perf_label(0), "HUD de performance");
        assert_eq!(menu.perf_label(1), "Limite de FPS");
        assert_eq!(menu.perf_value(1), "120 FPS");
        assert!(menu.label(Item::QuitGame).contains("Team Fortress 2"));
        assert_eq!(
            menu.value(Item::Performance).as_deref(),
            Some("HUD natif · Basique")
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
    fn performance_panels_fit_and_keep_navigation_visible() {
        let mut menu = MenuState::new("Jeu");
        menu.set_page(Page::Performance);
        for advanced in [false, true] {
            menu.perf_advanced = advanced;
            let height = menu.panel_height();
            assert!(height < 692.0, "panneau trop haut pour 720p : {height}");
            let rows = menu.perf_visible_rows();
            for pair in rows.windows(2) {
                assert!(menu.perf_row_y(pair[1]) >= menu.perf_row_y(pair[0]) + PERF_ROW);
            }
            assert!(menu.perf_row_y(*rows.last().unwrap()) + PERF_ROW + FOOTER_HEIGHT < height);
        }
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
        assert!(matches!(menu.activate(), Some(Action::RecordBadge { .. })));
        menu.recording_selected = 4;
        assert!(matches!(menu.activate(), Some(Action::RecordBadge { .. })));
        assert!(menu.perf_row_y(4) + PERF_ROW <= menu.perf_grid_top());
        assert!(menu.perf_grid_top() + PERF_GRID < menu.recordings_height());
        assert!(menu.recordings_height() < 800.0);
    }

    #[test]
    fn le_bandeau_de_capture_affiche_le_temps_enregistre() {
        let mut menu = MenuState::new("Jeu");
        assert_eq!(menu.live.recording_label("REC"), None);
        menu.live.recording = Some(lapinux_core::model::RecordingState {
            paused: false,
            secs: 83,
        });
        assert_eq!(
            menu.live.recording_label("REC").as_deref(),
            Some("REC 01:23")
        );
        assert_eq!(
            menu.live.recording_label("LIVE").as_deref(),
            Some("LIVE 01:23")
        );
        menu.live.recording = Some(lapinux_core::model::RecordingState {
            paused: true,
            secs: 3_725,
        });
        assert_eq!(
            menu.live.recording_label("REC").as_deref(),
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
            gamescope: false,
            recording: None,
            volume: None,
        };
        assert_eq!(live.fps_status(144), FpsStatus::Good);
        assert_eq!(live.fps_status(165), FpsStatus::Good);
        assert_eq!(live.fps_status(0), FpsStatus::Good);
        assert_eq!(Live::default().fps_status(60), FpsStatus::Unknown);
        // Le plancher absolu : 60 images restent confortables sur un écran
        // rapide, alors qu'une vraie chute passe au rouge.
        assert_eq!(
            Live {
                fps: Some(60),
                ..live.clone()
            }
            .fps_status(144),
            FpsStatus::Good
        );
        assert_eq!(
            Live {
                fps: Some(50),
                ..live
            }
            .fps_status(60),
            FpsStatus::Low
        );
    }

    #[test]
    fn live_updates_only_when_values_change() {
        let mut menu = MenuState::new("Jeu");
        let live = Live {
            fps: Some(60),
            session_secs: Some(30),
            series: vec![60, 61, 60],
            dlss: None,
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
            gamescope: false,
            recording: None,
            volume: None,
        }));
    }

    #[test]
    fn values_track_the_state() {
        let mut menu = MenuState::new("Jeu");
        menu.fps_cap = 144;
        assert_eq!(menu.perf_value(1), "144 FPS");
        assert_eq!(
            menu.value(Item::Performance).as_deref(),
            Some("HUD natif · Basique")
        );
        assert_eq!(menu.value(Item::Resume), None);
        menu.set_hud_style(lapinux_core::hud::HudStyle::builtin("full"));
        assert_eq!(
            menu.value(Item::Performance).as_deref(),
            Some("HUD natif · Détaillée")
        );
    }

    #[test]
    fn la_ligne_profil_ouvre_la_liste_complete() {
        let mut menu = MenuState::new("Jeu");
        menu.profiles = vec!["xbox".into(), "nintendo".into()];
        menu.selected = menu
            .items()
            .iter()
            .position(|item| *item == Item::Profile)
            .unwrap();

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
    fn la_ligne_amis_ouvre_une_conversation_et_envoie_un_message() {
        let mut menu = MenuState::new("Portal 2");
        assert!(menu.set_invite(invite_state()));
        menu.selected = menu
            .items()
            .iter()
            .position(|item| *item == Item::Friends)
            .unwrap();
        assert_eq!(menu.activate(), None);
        assert_eq!(menu.page, Page::Invite);
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
                    received_live: false,
                },
                SteamChatMessage {
                    from: 7,
                    text: "Ça va ?".into(),
                    at: 0,
                    outgoing: true,
                    received_live: false,
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
    fn le_bouton_inviter_n_apparait_qu_avec_un_lobby() {
        let mut menu = MenuState::new("Portal 2");
        let mut state = invite_state();
        menu.set_page(Page::Invite);
        menu.set_invite(state.clone());
        assert!(
            menu.invite_button_rect(0, 0.0, 0.0, 520.0, 800.0).is_none(),
            "sans lobby, aucune invitation possible"
        );

        state.lobby = Some(109775240000000001);
        assert!(menu.set_invite(state));
        let (x, y, width, height) = menu
            .invite_button_rect(0, 0.0, 0.0, 520.0, 800.0)
            .expect("bouton visible avec un lobby");
        assert_eq!(
            menu.invite_button_at(x + width / 2.0, y + height / 2.0, 0.0, 0.0, 520.0, 800.0),
            Some(111)
        );
        assert_eq!(
            menu.invite_button_at(x - 20.0, y + height / 2.0, 0.0, 0.0, 520.0, 800.0),
            None
        );
        assert_eq!(Action::InviteFriend(111).wire(), "invite:111");
    }

    #[test]
    fn le_bouton_inviter_apparait_pendant_la_partie_sans_lobby() {
        let mut menu = MenuState::new("SMS Returns");
        menu.set_invite(invite_state());
        menu.set_page(Page::Invite);
        assert!(
            menu.invite_button_rect(0, 0.0, 0.0, 520.0, 800.0).is_none(),
            "hors partie, aucune invitation possible"
        );

        menu.appid = 4670920;
        assert!(
            menu.invite_button_rect(0, 0.0, 0.0, 520.0, 800.0).is_some(),
            "le bouton reste proposé pendant la partie, même sans lobby"
        );
        assert!(menu.open_chat());
        assert!(menu.chat_invite_button_rect(0.0, 0.0, 520.0).is_some());

        // La demande part quand même : le daemon la refusera faute de lobby.
        assert!(menu.mark_invite_sent(111));
        assert_eq!(menu.invite_button_label(111), "Envoi…");
        assert!(!menu.invite_sent(111));
        assert!(menu.apply_action_failure(
            "invite:111",
            "aucun lobby Steam actif : le jeu n'en a pas encore créé"
        ));
        assert_eq!(menu.invite_button_label(111), "Inviter");
        assert!(menu
            .invite_failure()
            .is_some_and(|failure| failure.contains("aucun lobby")));
    }

    #[test]
    fn un_refus_du_daemon_rearme_le_bouton_inviter() {
        let mut menu = MenuState::new("Portal 2");
        let mut state = invite_state();
        state.lobby = Some(109775240000000001);
        menu.set_invite(state);
        menu.set_page(Page::Invite);
        assert!(menu.mark_invite_sent(111));
        assert_eq!(menu.invite_button_label(111), "Invitation envoyée");

        assert!(menu.apply_action_failure("invite:111", "Steam a refusé l'invitation (eresult 2)"));
        assert!(!menu.invite_sent(111));
        assert_eq!(menu.invite_button_label(111), "Inviter");
        let failure = menu.invite_failure().expect("message d'échec affiché");
        assert!(failure.contains("eresult 2"), "{failure}");

        // Un nouvel essai efface le message et reverrouille le bouton.
        assert!(menu.mark_invite_sent(111));
        assert!(menu.invite_failure().is_none());
        assert!(menu.invite_sent(111));
        // Une autre action refusée ne touche pas à l'invitation.
        assert!(!menu.apply_action_failure("fps_cap:60", "refusé"));
        assert!(menu.invite_sent(111));
    }

    #[test]
    fn le_bouton_inviter_apparait_dans_la_conversation_ouverte() {
        let mut menu = MenuState::new("Portal 2");
        let mut state = invite_state();
        menu.set_page(Page::Invite);
        menu.set_invite(state.clone());
        assert!(menu.open_chat());
        assert!(
            menu.chat_invite_button_rect(0.0, 0.0, 520.0).is_none(),
            "sans lobby, la conversation ne propose pas d'invitation"
        );

        state.lobby = Some(109775240000000001);
        assert!(menu.set_invite(state));
        let (x, y, width, height) = menu
            .chat_invite_button_rect(0.0, 0.0, 520.0)
            .expect("bouton visible dans la conversation");
        assert_eq!(
            menu.chat_invite_button_at(x + width / 2.0, y + height / 2.0, 0.0, 0.0, 520.0),
            Some(111)
        );
        assert_eq!(
            menu.chat_invite_button_at(x - 20.0, y + height / 2.0, 0.0, 0.0, 520.0),
            None
        );
        assert_eq!(Action::InviteFriend(111).wire(), "invite:111");
    }

    #[test]
    fn une_invitation_envoyee_ne_se_renvoie_pas() {
        let mut menu = MenuState::new("Portal 2");
        let mut state = invite_state();
        state.lobby = Some(109775240000000001);
        menu.set_page(Page::Invite);
        menu.set_invite(state.clone());
        assert_eq!(menu.invite_button_label(111), "Inviter");

        assert!(menu.mark_invite_sent(111));
        assert!(menu.invite_sent(111));
        assert_eq!(menu.invite_button_label(111), "Invitation envoyée");
        assert!(
            !menu.mark_invite_sent(111),
            "un second clic ne doit pas renvoyer l'invitation"
        );
        assert!(menu.open_chat());
        assert_eq!(menu.chat_invite_button_label(), "Invitation envoyée");

        // Un nouveau lobby réarme le bouton ; le même lobby le garde.
        assert!(!menu.set_invite(state.clone()));
        state.lobby = Some(109775240000000002);
        assert!(menu.set_invite(state));
        assert!(!menu.invite_sent(111));
        assert_eq!(menu.invite_button_label(111), "Inviter");
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
        assert!(menu
            .invite_scrollbar_thumb(0.0, 0.0, 520.0, 800.0)
            .is_some());
        let list_right = menu.invite_list_right(0.0, 520.0);
        assert_eq!(
            menu.invite_scroll_index_at(0.0, 0.0, 520.0, 800.0, list_right - 1.0, top + height),
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
    fn noms_de_profils_affiches_avec_une_majuscule() {
        assert_eq!(profile_display_name("xbox"), "Xbox");
        assert_eq!(profile_display_name("nintendo"), "Nintendo");
        assert_eq!(profile_display_name("équipe perso"), "Équipe perso");
        assert_eq!(profile_display_name("mon Xbox"), "Mon Xbox");
        assert_eq!(profile_display_name(""), "");
        let menu = MenuState::new("Jeu");
        assert_eq!(menu.value(Item::Profile).as_deref(), Some("Xbox"));
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
    fn hud_layouts_have_wire_names() {
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
    fn hud_rows_drive_the_native_style() {
        let mut menu = MenuState::new("Jeu");
        menu.set_page(Page::Performance);
        menu.perf_selected = 3;
        assert!(matches!(menu.adjust(-1), Some(Action::HudStyle(_))));
        menu.perf_selected = 6;
        assert!(matches!(menu.adjust(1), Some(Action::HudStyle(_))));
        assert_eq!(menu.hud_style.position, "top-center");
        menu.perf_selected = 7;
        assert!(matches!(menu.activate(), Some(Action::HudStyle(_))));
        assert_eq!(menu.hud_style.layout, "inline");
        menu.perf_selected = 8;
        assert!(matches!(menu.activate(), Some(Action::HudStyle(_))));
        assert!(menu.hud_style.enabled(lapinux_core::hud::HudElement::Graph));
        menu.perf_selected = 9;
        assert!(matches!(menu.activate(), Some(Action::HudStyle(_))));
        assert!(menu.hud_style.enabled(lapinux_core::hud::HudElement::Cores));
    }

    #[test]
    fn controller_reaches_advanced_and_returns_to_its_entry() {
        let mut menu = MenuState::new("Jeu");
        menu.set_page(Page::Performance);
        for expected in [1, 2, 3, 6, 11] {
            menu.next();
            assert_eq!(menu.perf_selected, expected);
        }
        assert_eq!(menu.activate(), None);
        assert!(menu.perf_advanced);
        // Le mode détaillé commence par la génération d'images, conservée
        // dans cette liste.
        assert_eq!(menu.perf_selected, 2);
        for expected in [7, 8, 9, 10, 20, 21, 2] {
            menu.next();
            assert_eq!(menu.perf_selected, expected);
        }
        menu.close();
        assert!(!menu.perf_advanced);
        assert_eq!(menu.perf_selected, 11);
    }

    #[test]
    fn custom_hud_style_changes_are_sent_hot_and_round_trip() {
        use lapinux_core::hud::{HudElement, HudStyle};
        let mut menu = MenuState::new("Jeu");
        menu.set_page(Page::Performance);
        menu.perf_advanced = true;
        menu.perf_elements = true;
        menu.perf_selected = 12;
        // Même normalisation que le basculement : les couleurs gardent leur
        // alpha, la mesure ne doit pas le changer.
        menu.hud_style.normalize();
        let color_before = menu.element_color(HudElement::Fps);
        let Some(Action::HudStyle(style)) = menu.activate() else {
            panic!("Entrée doit envoyer le style complet");
        };
        // L’overlay n’édite plus les couleurs : Entrée coupe l’élément et
        // laisse sa couleur intacte, elle se règle dans l’application.
        assert!(!style.enabled(HudElement::Fps));
        assert_eq!(menu.element_color(HudElement::Fps), color_before);
        let wire = Action::HudStyle(style.clone()).wire();
        let body = wire.strip_prefix("hud_style:").expect("préfixe du style");
        let parsed: HudStyle = serde_json::from_str(body).expect("style relu");
        assert_eq!(parsed, *style);

        menu.perf_selected = 13;
        let Some(Action::HudStyle(style)) = menu.adjust(-1) else {
            panic!("désactiver un élément doit envoyer le style complet");
        };
        assert!(!style.enabled(HudElement::Frametime));
        assert_eq!(menu.element_color(HudElement::Fps), color_before);
    }

    #[test]
    fn controller_can_invite_selected_friend() {
        let mut menu = MenuState::new("Jeu");
        menu.appid = 1;
        menu.set_invite(invite_state());
        menu.set_page(Page::Invite);
        assert_eq!(menu.adjust(1), Some(Action::InviteFriend(111)));
        assert_eq!(menu.adjust(-1), None);
    }

    #[test]
    fn selected_profile_remains_visible_in_long_list() {
        let mut menu = MenuState::new("Jeu");
        menu.profiles = (0..20).map(|i| format!("Profil {i}")).collect();
        menu.set_page(Page::Profiles);
        for _ in 0..15 {
            menu.next();
        }
        assert_eq!(menu.profile_selected, 15);
        assert_eq!(menu.profile_offset(), 8);
        assert_eq!(menu.profile_hit_row(menu.profile_row_y(15) + 1.0), Some(15));
        assert!(menu.panel_height() < 692.0);
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
    fn clicks_find_back_and_visible_performance_controls() {
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
        for &row in menu.perf_visible_rows() {
            assert_eq!(menu.hit_perf_row(menu.perf_row_y(row) + 10.0), Some(row));
        }
    }
}
