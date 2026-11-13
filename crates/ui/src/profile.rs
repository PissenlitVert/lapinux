//! Section **Profil** : le compte Steam tel qu'il se montre, et ce que la
//! session sociale apporte — présence publiée, amis en ligne, famille,
//! notifications et commentaires.
//!
//! Tout est lu sans client Steam : la session CM du daemon donne les amis, la
//! famille et les notifications ; les pages publiques de Steam donnent
//! l'identité, la description, les noms et les photos
//! des membres de la famille, et les commentaires du profil.

use super::*;
use lapinux_core::social::SteamFriend;
use std::time::Instant;

/// Largeur de la liste des plateformes, quand elle tient sur le côté.
const PLATFORM_SIDE: f32 = 216.0;
/// Place minimale laissée à la fiche : en dessous, la liste passe au-dessus.
const STEAM_PAGE: f32 = 620.0;

const USER_AGENT: &str = "Lapinux/0.1";
/// Agent d'un navigateur de bureau, pour les pages communautaires : Steam
/// réserve les siens aux profiles publics et limite durement les autres.
const BROWSER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0";
const FETCH_TIMEOUT: Duration = Duration::from_secs(20);
/// Avatars d'amis téléchargés au plus par passage : assez pour la liste, sans
/// inonder le réseau.
const FRIEND_AVATARS: usize = 24;
/// Pages communautaires : une requête à la fois, un peu d'air entre elles.
/// Lancées en rafale — fiche, commentaires, famille —, Steam finit par répondre
/// 429 et la fiche du profil tombe.
const COMMUNITY_SPACING: Duration = Duration::from_millis(600);

/// Durée de fraîcheur du profil lu sur Steam : la page bouge peu, et Steam
/// ferme la porte aux lectures répétées.
const PROFILE_CACHE_TTL: Duration = Duration::from_secs(600);
/// Après un 429, la pause est plus longue : la fenêtre de limitation de Steam
/// dépasse largement l'intervalle normal entre deux requêtes.
const COMMUNITY_PENALTY: Duration = Duration::from_secs(20);
/// Attente avant de relire les vitrines quand Steam a refusé : la fenêtre de
/// limitation dure plus longtemps que la pause ordinaire.
const SHOWCASE_RETRY: Duration = Duration::from_secs(60);
/// Délai du nouvel essai automatique de la fiche après une limite de débit.
const RELOAD_DELAY: Duration = Duration::from_secs(45);
/// Essais automatiques avant de laisser la main au bouton « Réessayer ».
const RELOAD_ATTEMPTS: u32 = 3;

/// Saut de ligne voulu par Steam (`<br>`, `</p>`), distinct des retours à la
/// ligne de mise en forme du HTML : sans cette distinction, l'indentation du
/// code source devenait des lignes vides dans la fiche.
const LINE_BREAK: char = '\u{b}';

/// Fragment de la description publique : du texte, un lien, ou une émoticône
/// Steam (`:nom:`, rendue par Steam sous forme d'image).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum DescriptionPart {
    Text(String),
    /// Lien du profil : le libellé affiché et l'adresse qu'il ouvre.
    Link {
        text: String,
        url: String,
    },
    Emote {
        name: String,
        url: String,
    },
}

/// Identité publique d'un compte Steam, telle que la renvoie le profil XML.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct SteamProfile {
    pub steam_id: String,
    pub name: String,
    pub avatar_url: String,
    /// `online`, `offline`, `in-game`…
    pub online_state: String,
    pub state_message: String,
    /// `public`, `friendsonly`, `private`.
    pub privacy_state: String,
    pub member_since: String,
    pub location: String,
    pub real_name: String,
    pub headline: String,
    /// Description du profil : le texte et les émoticônes Steam, dans l'ordre.
    pub description: Vec<DescriptionPart>,
    /// Heures jouées sur les deux dernières semaines.
    pub hours_2wk: String,
    /// Nom personnalisé de l'adresse du profil.
    pub custom_url: String,
    pub vac_banned: bool,
    pub trade_ban: String,
}

/// Entrées de la liste du Profil : les plateformes, puis deux pages qui ne
/// dépendent d'aucune boutique — la présence Discord et l'activité de la
/// session. Chacune a sa ligne et sa page, comme Steam.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Platform {
    Steam,
    Epic,
    Gog,
    Itch,
    Discord,
    Activity,
}

impl Platform {
    fn label(self) -> &'static str {
        match self {
            Platform::Steam => "Steam",
            Platform::Epic => "Epic Games",
            Platform::Gog => "GOG",
            Platform::Itch => "itch.io",
            Platform::Discord => "Discord",
            Platform::Activity => lapinux_core::i18n::t("profile.activity"),
        }
    }

    /// Boutique correspondante, Steam comprise ; les pages Discord et Activité
    /// n'appartiennent à aucune boutique.
    fn store(self) -> Option<lapinux_core::stores::Store> {
        match self {
            Platform::Steam => Some(lapinux_core::stores::Store::Steam),
            Platform::Epic => Some(lapinux_core::stores::Store::Epic),
            Platform::Gog => Some(lapinux_core::stores::Store::Gog),
            Platform::Itch => Some(lapinux_core::stores::Store::Itch),
            Platform::Discord | Platform::Activity => None,
        }
    }
}

/// Une ligne de la liste des plateformes : la boutique, le compte qui y est
/// connecté et son état.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlatformEntry {
    pub platform: Platform,
    /// Nom du compte, ou le réglage à faire quand il n'y en a pas.
    pub account: String,
    pub connected: bool,
}

/// Onglets de la fiche Steam : le profil public, ou les achats et les clés.
/// Les deux options vivent au-dessus de tous les encarts, comme dans Steam.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProfileTab {
    Profil,
    Achats,
}

/// Ce que la page demande à l'application quand on agit dessus.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ProfileAction {
    /// Active une clé produit sur le compte Steam.
    RedeemKey(String),
    /// Joue la célébration d'activation sans toucher au compte : c'est le
    /// bouton de test, aucune clé n'est envoyée à Steam.
    TestRedeem,
    /// Publier une nouvelle présence sur Steam.
    SetPresence(SteamPresence),
    /// Redemander la liste d'amis au daemon.
    RefreshSocial,
    /// Rejoindre le lobby d'un ami invitant, à l'adresse enregistrée.
    JoinInvite {
        appid: u32,
        lobby: u64,
        connect: String,
    },
    /// Refaire une lecture complète (profil, commentaires).
    Reload,
    /// Ouvrir les réglages du compte d'une boutique.
    Accounts(lapinux_core::stores::Store),
    Copy(String),
    Open(String),
    /// Ouvrir la fiche du jeu dans la boutique Steam, dans sa fenêtre.
    Game {
        appid: u32,
        name: String,
    },
}

/// Un commentaire tel que la page publique le rend : son auteur, sa photo,
/// puis le texte avec ses liens, ses émoticônes et ses images.
#[derive(Debug, Clone, Default, PartialEq)]
struct CommentView {
    author: String,
    author_id: u64,
    avatar: String,
    posted: String,
    parts: Vec<DescriptionPart>,
    images: Vec<String>,
}

/// Un jeu de la vitrine « Activité récente » : sa capsule, le temps passé
/// dessus et son dernier lancement, tels que la page publique les publie.
#[derive(Debug, Clone, Default, PartialEq)]
struct ShowcaseGame {
    name: String,
    /// « 1 776 h en tout »
    hours: String,
    /// « dernière utilisation le 20 oct. »
    last_used: String,
    /// « 18 succès sur 29 »
    achievements: String,
    /// Capsule du jeu, à télécharger comme les autres visuels.
    capsule: String,
}

/// Un jeu de la bibliothèque familiale d'un membre : son appid garde la
/// vignette associée, son nom vient du catalogue quand il y est.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FamilyGame {
    appid: u32,
    name: String,
}

/// Une vitrine du profil Steam : « Jeux que j'aime », « Activité récente »…
/// Ce que le joueur a choisi de montrer, tel que sa page publique le rend.
#[derive(Debug, Clone, Default, PartialEq)]
struct SteamShowcase {
    title: String,
    /// Texte de la vitrine, liens et émoticônes compris, comme la description
    /// du profil : un lien s'ouvre, une émoticône se dessine.
    parts: Vec<DescriptionPart>,
    /// Noms des jeux d'une vitrine d'activité.
    lines: Vec<String>,
    /// Détail des jeux d'une vitrine d'activité, quand Steam le donne.
    games: Vec<ShowcaseGame>,
    /// Sous-titre de la vitrine : « 9,5 h les 15 derniers jours ».
    subtitle: String,
    /// Visuels de la vitrine, tels que Steam les publie.
    images: Vec<String>,
}

enum ProfileMsg {
    Loaded(SteamProfile),
    /// Total des quinze derniers jours lu sur la page publique : le profil
    /// XML renvoie encore `hoursPlayed2Wk` à 0 alors que la page l'affiche.
    RecentHours(String),
    Avatar(Vec<u8>),
    Comments {
        total: u32,
        comments: Vec<CommentView>,
        error: Option<String>,
    },
    FriendAvatar {
        steam_id: u64,
        bytes: Vec<u8>,
    },
    /// Photo d'un auteur de commentaire.
    CommentAvatar {
        url: String,
        bytes: Vec<u8>,
    },
    /// Emoji Unicode téléchargé : il remplace le glyphe monochrome.
    Emoji {
        text: String,
        bytes: Vec<u8>,
    },
    /// Identité publique d'un membre de la famille, lue sans session.
    FamilyProfile {
        steam_id: u64,
        name: String,
        avatar: Option<Vec<u8>>,
    },
    /// Image d'une émoticône Steam de la description.
    Emote {
        url: String,
        bytes: Vec<u8>,
    },
    /// Vitrines du profil, lues sur sa page publique.
    Showcases(Vec<SteamShowcase>),
    /// Image d'une vitrine.
    ShowcaseImage {
        url: String,
        bytes: Vec<u8>,
    },
    /// Capsule du jeu auquel un ami joue.
    GameCapsule {
        appid: u32,
        bytes: Vec<u8>,
    },
    Failed(FetchError),
}

/// Pourquoi une page n'a pas pu être lue. La distinction compte : une limite de
/// débit de Steam se referme d'elle-même, une panne de réseau non.
#[derive(Debug, Clone)]
struct FetchError {
    message: String,
    /// Réponse 429 : un nouvel essai a du sens.
    rate_limited: bool,
}

impl FetchError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            rate_limited: false,
        }
    }

    /// Refus de débit de Steam (429) : la page revient d'elle-même.
    fn rate_limited() -> Self {
        Self {
            message: lapinux_core::i18n::t("ui.profile_rate_limited").into(),
            rate_limited: true,
        }
    }
}

/// État de la section entre deux images : dernier profil lu, textures déjà
/// téléversées et requête en cours. Aucune donnée du daemon n'est dupliquée
/// ici, seulement ce que le réseau a rapporté.
pub(crate) struct Profile {
    profile: Option<SteamProfile>,
    error: Option<String>,
    loading: bool,
    /// Compte déjà chargé : le réseau n'est pas relancé à chaque image.
    loaded_for: Option<u64>,
    rx: Option<mpsc::Receiver<ProfileMsg>>,
    avatar: Option<egui::TextureHandle>,
    /// Session sociale publiée par le daemon (amis, famille, notifications).
    social: Option<SteamSocial>,
    comments: Option<(u32, Vec<CommentView>)>,
    comments_error: Option<String>,
    /// Émoticônes Steam de la description, par URL.
    emotes: HashMap<String, egui::TextureHandle>,
    /// Émoticônes déjà demandées, pour ne pas les redemander à chaque image.
    emote_pending: HashSet<String>,
    /// Vitrines du profil et leurs visuels.
    showcases: Option<Vec<SteamShowcase>>,
    showcase_images: HashMap<String, egui::TextureHandle>,
    showcase_pending: HashSet<String>,
    /// Catégories d'amis repliées, par nom de section.
    collapsed_friends: HashSet<String>,
    /// Capsules des jeux joués par les amis, par appid.
    game_capsules: HashMap<u32, egui::TextureHandle>,
    game_capsule_pending: HashSet<u32>,
    /// Photos des amis et des membres de la famille, par SteamID.
    friend_avatars: HashMap<u64, egui::TextureHandle>,
    /// Photos déjà demandées, pour ne pas les redemander à chaque image.
    friend_avatar_pending: HashSet<u64>,
    /// Noms des membres de la famille, quand la page publique les donne.
    family_names: HashMap<u64, String>,
    /// Membres de la famille dont la page publique est déjà demandée (ou lue).
    family_profiles_asked: HashSet<u64>,
    /// Membre dont le modal des jeux partagés est ouvert.
    family_games: Option<u64>,
    /// La dernière panne était une limite de débit de Steam (429).
    rate_limited: bool,
    /// Fiche à relire toute seule, après une limite de débit.
    retry_at: Option<Instant>,
    /// Essais automatiques déjà faits depuis la dernière fiche lue.
    retries: u32,
    friend_tx: mpsc::Sender<ProfileMsg>,
    friend_rx: mpsc::Receiver<ProfileMsg>,
    /// Emoji Unicode rendus en images : la police de l'application n'a que
    /// des glyphes monochromes.
    emojis: HashMap<String, egui::TextureHandle>,
    emoji_pending: HashSet<String>,
    /// Photos des auteurs de commentaires, par adresse : un profil peut
    /// n'avoir qu'une adresse personnalisée, sans identifiant numérique.
    comment_avatars: HashMap<String, egui::TextureHandle>,
    comment_avatar_pending: HashSet<String>,
    /// L'identifiant du compte reste masqué tant qu'on ne le demande pas.
    id_revealed: bool,
    /// Pseudos dévoilés dans la liste des plateformes : ils sont masqués par
    /// défaut, l'œil de chaque ligne les montre.
    revealed_accounts: HashSet<Platform>,
    /// Plateforme dont la fiche est affichée.
    platform: Platform,
    /// Onglet choisi : la fiche du profil, ou les achats.
    tab: ProfileTab,
    /// Clé produit en cours de saisie dans l'onglet Achats.
    key_input: String,
}

impl Profile {
    pub(crate) fn new() -> Self {
        let (friend_tx, friend_rx) = mpsc::channel();
        Self {
            profile: None,
            error: None,
            loading: false,
            loaded_for: None,
            rx: None,
            avatar: None,
            social: None,
            comments: None,
            comments_error: None,
            emotes: HashMap::new(),
            emote_pending: HashSet::new(),
            showcases: None,
            showcase_images: HashMap::new(),
            showcase_pending: HashSet::new(),
            // Comme Steam : la liste des absents reste fermée.
            collapsed_friends: HashSet::from(
                [lapinux_core::i18n::t("profile.offline").to_string()],
            ),
            game_capsules: HashMap::new(),
            game_capsule_pending: HashSet::new(),
            friend_avatars: HashMap::new(),
            friend_avatar_pending: HashSet::new(),
            family_names: HashMap::new(),
            family_profiles_asked: HashSet::new(),
            family_games: None,
            rate_limited: false,
            retry_at: None,
            retries: 0,
            friend_tx,
            friend_rx,
            emojis: HashMap::new(),
            emoji_pending: HashSet::new(),
            comment_avatars: HashMap::new(),
            comment_avatar_pending: HashSet::new(),
            id_revealed: false,
            revealed_accounts: HashSet::new(),
            platform: Platform::Steam,
            tab: ProfileTab::Profil,
            key_input: String::new(),
        }
    }

    /// Publie l'état social lu par l'application.
    pub(crate) fn set_social(&mut self, social: Option<SteamSocial>) {
        self.social = social;
        self.fetch_friend_avatars();
        self.fetch_game_capsules();
        self.fetch_family_profiles();
    }

    /// Nom public d'un membre de la famille, quand sa fiche a répondu.
    pub(crate) fn family_name(&self, steam_id: u64) -> Option<String> {
        self.family_names.get(&steam_id).cloned()
    }

    /// Photo d'un membre de la famille, déjà décodée par la fiche publique.
    pub(crate) fn family_avatar(&self, steam_id: u64) -> Option<egui::TextureHandle> {
        self.friend_avatars.get(&steam_id).cloned()
    }

    /// SteamID du profil lu, s'il est connu.
    pub(crate) fn steam_id(&self) -> Option<u64> {
        self.profile
            .as_ref()
            .and_then(|profile| profile.steam_id.parse::<u64>().ok())
    }

    /// Photos des amis et des membres de la famille : celles qui manquent sont
    /// téléchargées en tâche de fond.
    fn fetch_friend_avatars(&mut self) {
        let Some(social) = self.social.as_ref() else {
            return;
        };
        // La famille passe en premier : elle est petite, et la file est bornée
        // (un compte avec beaucoup d'amis ne doit pas la repousser à jamais).
        // Membres et amis partagent la même clé : leur SteamID.
        let mut offered: Vec<(u64, String)> = Vec::new();
        if let Some(family) = social.family.as_ref() {
            offered.extend(
                family
                    .members
                    .iter()
                    .map(|member| (member.steam_id, member.avatar.clone())),
            );
        }
        offered.extend(
            social
                .friends
                .iter()
                .map(|friend| (friend.steam_id, friend.avatar.clone())),
        );
        let pending: Vec<(u64, String)> = offered
            .into_iter()
            .filter(|(_, avatar)| !avatar.is_empty())
            .filter(|(steam_id, _)| !self.friend_avatars.contains_key(steam_id))
            .filter(|(steam_id, _)| !self.friend_avatar_pending.contains(steam_id))
            .take(FRIEND_AVATARS.saturating_sub(self.friend_avatars.len()))
            .collect();
        if pending.is_empty() {
            return;
        }
        for (steam_id, _) in &pending {
            self.friend_avatar_pending.insert(*steam_id);
        }
        let tx = self.friend_tx.clone();
        std::thread::spawn(move || {
            for (steam_id, url) in pending {
                if let Some(bytes) = http_bytes(&url) {
                    let _ = tx.send(ProfileMsg::FriendAvatar { steam_id, bytes });
                }
            }
        });
    }

    /// Télécharge les émoticônes Steam de la description, une fois chacune :
    /// le profil les publie comme des images (`:nom:`).
    fn fetch_emotes(&mut self) {
        let Some(profile) = self.profile.as_ref() else {
            return;
        };
        let mut pending: Vec<String> = Vec::new();
        // Description, vitrines et commentaires : tous portent des `:nom:`,
        // et chacun arrive à son heure.
        let showcase_parts: Vec<&DescriptionPart> = self
            .showcases
            .iter()
            .flatten()
            .flat_map(|showcase| showcase.parts.iter())
            .chain(
                self.comments
                    .iter()
                    .flat_map(|(_, list)| list.iter())
                    .flat_map(|comment| comment.parts.iter()),
            )
            .collect();
        let mut collect = |part: &DescriptionPart| {
            let DescriptionPart::Emote { url, .. } = part else {
                return;
            };
            if self.emotes.contains_key(url) || self.emote_pending.contains(url) {
                return;
            }
            self.emote_pending.insert(url.clone());
            pending.push(url.clone());
        };
        for part in profile.description.iter().chain(showcase_parts) {
            collect(part);
        }
        if pending.is_empty() {
            return;
        }
        let tx = self.friend_tx.clone();
        std::thread::spawn(move || {
            for url in pending {
                if let Some(bytes) = http_bytes(&url) {
                    let _ = tx.send(ProfileMsg::Emote { url, bytes });
                }
            }
        });
    }

    /// Télécharge les visuels des vitrines, une fois chacun : ce sont les
    /// capsules des jeux montrés, pas les émoticônes du texte.
    fn fetch_static_images(&mut self) {
        let mut urls: Vec<String> = Vec::new();
        for showcase in self.showcases.iter().flatten() {
            urls.extend(showcase.images.iter().cloned());
            urls.extend(
                showcase
                    .games
                    .iter()
                    .map(|game| game.capsule.clone())
                    .filter(|url| !url.is_empty()),
            );
        }
        for comment in self.comments.iter().flat_map(|(_, list)| list.iter()) {
            urls.extend(comment.images.iter().cloned());
        }
        let mut pending: Vec<String> = Vec::new();
        for url in urls {
            if self.showcase_images.contains_key(&url) || self.showcase_pending.contains(&url) {
                continue;
            }
            self.showcase_pending.insert(url.clone());
            pending.push(url);
        }
        if pending.is_empty() {
            return;
        }
        let tx = self.friend_tx.clone();
        std::thread::spawn(move || {
            for url in pending {
                if let Some(bytes) = http_bytes(&url) {
                    let _ = tx.send(ProfileMsg::ShowcaseImage { url, bytes });
                }
            }
        });
    }

    /// Photos des auteurs de commentaires : téléchargées une fois chacune.
    fn fetch_comment_avatars(&mut self) {
        let mut pending: Vec<String> = Vec::new();
        for comment in self.comments.iter().flat_map(|(_, list)| list.iter()) {
            let url = &comment.avatar;
            if url.is_empty()
                || self.comment_avatars.contains_key(url)
                || self.comment_avatar_pending.contains(url)
            {
                continue;
            }
            self.comment_avatar_pending.insert(url.clone());
            pending.push(url.clone());
        }
        if pending.is_empty() {
            return;
        }
        let tx = self.friend_tx.clone();
        std::thread::spawn(move || {
            for url in pending {
                if let Some(bytes) = http_bytes(&url) {
                    let _ = tx.send(ProfileMsg::CommentAvatar { url, bytes });
                }
            }
        });
    }

    /// Demande les emoji Unicode des textes affichés : la police de
    /// l'application ne dessine que des glyphes monochromes, une petite image
    /// leur rend leurs couleurs.
    fn queue_text_emojis(&mut self) {
        let mut texts: Vec<&str> = Vec::new();
        for part in self
            .profile
            .iter()
            .flat_map(|profile| profile.description.iter())
        {
            if let DescriptionPart::Text(text) = part {
                texts.push(text);
            }
        }
        for showcase in self.showcases.iter().flatten() {
            for part in &showcase.parts {
                if let DescriptionPart::Text(text) = part {
                    texts.push(text);
                }
            }
        }
        for comment in self.comments.iter().flat_map(|(_, list)| list.iter()) {
            for part in &comment.parts {
                if let DescriptionPart::Text(text) = part {
                    texts.push(text);
                }
            }
        }
        let pending: Vec<String> = texts
            .into_iter()
            .flat_map(split_emoji)
            .filter(|(emoji, _)| *emoji)
            .map(|(_, text)| text.to_string())
            .filter(|text| !self.emojis.contains_key(text) && !self.emoji_pending.contains(text))
            .collect();
        for text in pending {
            if !self.emoji_pending.insert(text.clone()) {
                continue;
            }
            let urls = emoji_urls(&text);
            let tx = self.friend_tx.clone();
            std::thread::spawn(move || {
                let bytes = urls
                    .iter()
                    .find_map(|url| http_bytes(url))
                    .unwrap_or_default();
                let _ = tx.send(ProfileMsg::Emoji { text, bytes });
            });
        }
    }

    /// Télécharge la capsule du jeu auquel chaque ami joue : la vignette du
    /// magasin, à la taille d'une ligne.
    fn fetch_game_capsules(&mut self) {
        let Some(social) = self.social.as_ref() else {
            return;
        };
        let appids: Vec<u32> = social
            .friends
            .iter()
            .map(|friend| friend.game_appid)
            .collect();
        self.request_game_capsules(&appids);
    }

    /// Demande les capsules manquantes d'une poignée d'appids : le modal
    /// familial n'appelle que les lignes visibles, pour ne pas télécharger
    /// toute une bibliothèque partagée d'un coup.
    fn request_game_capsules(&mut self, appids: &[u32]) {
        // Les tests ne doivent pas ouvrir de connexion réseau : ils dessinent
        // le modal avec les vignettes déjà en cache, ou leur repli.
        if cfg!(test) {
            return;
        }
        let mut pending: Vec<u32> = Vec::new();
        for &appid in appids {
            if appid == 0
                || self.game_capsules.contains_key(&appid)
                || self.game_capsule_pending.contains(&appid)
            {
                continue;
            }
            self.game_capsule_pending.insert(appid);
            pending.push(appid);
        }
        if pending.is_empty() {
            return;
        }
        let tx = self.friend_tx.clone();
        std::thread::spawn(move || {
            for (appid, bytes) in fetch_game_capsules(&pending) {
                let _ = tx.send(ProfileMsg::GameCapsule { appid, bytes });
            }
        });
    }

    /// Prépare les capsules des amis qui jouent, y compris depuis l'Accueil.
    /// La fiche du profil et l'Accueil lisent ainsi exactement le même visuel.
    pub(super) fn ensure_game_capsules(&mut self, ctx: &egui::Context, appids: &[u32]) {
        self.request_game_capsules(appids);
        self.poll_friend_avatars(ctx);
    }

    /// Capsule déjà chargée, sans nouvelle requête réseau.
    pub(super) fn game_capsule(&self, appid: u32) -> Option<egui::TextureHandle> {
        self.game_capsules.get(&appid).cloned()
    }

    /// Complète les membres de la famille que la session ne nomme pas : la
    /// page publique d'un profil donne son nom et sa photo sans compte Steam.
    fn fetch_family_profiles(&mut self) {
        let Some(social) = self.social.as_ref() else {
            return;
        };
        let Some(family) = social.family.as_ref() else {
            return;
        };
        let missing: Vec<u64> = family
            .members
            .iter()
            .filter(|member| member.steam_id != 0)
            .filter(|member| {
                (member.name.trim().is_empty() || member.avatar.trim().is_empty())
                    && !self.family_profiles_asked.contains(&member.steam_id)
            })
            .map(|member| member.steam_id)
            .collect();
        if missing.is_empty() {
            return;
        }
        for steam_id in &missing {
            self.family_profiles_asked.insert(*steam_id);
        }
        let tx = self.friend_tx.clone();
        std::thread::spawn(move || {
            for steam_id in missing {
                let Some(identity) = fetch_public_identity(steam_id) else {
                    continue;
                };
                let avatar = (!identity.avatar_url.is_empty())
                    .then(|| http_bytes(&identity.avatar_url))
                    .flatten();
                let _ = tx.send(ProfileMsg::FamilyProfile {
                    steam_id,
                    name: identity.name,
                    avatar,
                });
            }
        });
    }

    /// Charge le profil dès que le SteamID est connu, une seule fois.
    pub(crate) fn ensure_loaded(&mut self, steam_id: Option<u64>) {
        let Some(steam_id) = steam_id.filter(|id| *id > 0) else {
            return;
        };
        if self.loaded_for == Some(steam_id) {
            return;
        }
        self.loaded_for = Some(steam_id);
        self.start(steam_id, false);
    }

    /// Relance la lecture, même si le profil est déjà affiché. Le geste remet
    /// les compteurs à zéro : un clic vaut un nouvel essai complet.
    pub(crate) fn reload(&mut self, steam_id: u64) {
        self.loaded_for = Some(steam_id);
        self.retries = 0;
        self.retry_at = None;
        // Le bouton force une lecture neuve : sans cela, le cache répondrait
        // et le geste ne changerait rien.
        self.start(steam_id, true);
    }

    fn start(&mut self, steam_id: u64, fresh: bool) {
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        self.loading = true;
        self.error = None;
        self.rate_limited = false;
        self.comments_error = None;
        self.friend_avatars.clear();
        self.friend_avatar_pending.clear();
        self.emotes.clear();
        self.emote_pending.clear();
        self.showcases = None;
        self.comments = None;
        self.showcase_images.clear();
        self.showcase_pending.clear();
        self.family_names.clear();
        self.family_profiles_asked.clear();
        std::thread::spawn(move || fetch(steam_id, &tx, fresh));
    }

    /// Relit ce que les fils réseau ont publié : profil, avatar, commentaires,
    /// avatars et noms des amis et de la famille.
    pub(crate) fn poll(&mut self, ctx: &egui::Context) {
        self.retry_when_due(ctx);
        self.poll_profile(ctx);
        self.poll_friend_avatars(ctx);
    }

    /// Relit la fiche toute seule après une limite de débit : Steam rouvre la
    /// porte en quelques dizaines de secondes et l'utilisateur n'a rien à faire.
    fn retry_when_due(&mut self, ctx: &egui::Context) {
        let Some(at) = self.retry_at else {
            return;
        };
        let now = Instant::now();
        if now >= at {
            self.retry_at = None;
            if let Some(steam_id) = self.loaded_for {
                self.start(steam_id, false);
            }
        } else {
            // Le compte à rebours de la bannière avance à la seconde.
            ctx.request_repaint_after(at - now);
        }
    }

    /// Ce que la bannière annonce après un refus de débit : la cause, et le
    /// prochain essai.
    fn rate_limit_note(&self) -> String {
        match self.retry_at {
            Some(at) => {
                let left = at.saturating_duration_since(Instant::now()).as_secs() + 1;
                lapinux_core::i18n::t_args(
                    "profile.rate_limited_left",
                    &[("left", &left.to_string())],
                )
            }
            None => lapinux_core::i18n::t("profile.rate_limited").into(),
        }
    }

    /// Photos et noms arrivés depuis la dernière image.
    fn poll_friend_avatars(&mut self, ctx: &egui::Context) {
        loop {
            match self.friend_rx.try_recv() {
                Ok(ProfileMsg::FriendAvatar { steam_id, bytes }) => {
                    self.friend_avatar_pending.remove(&steam_id);
                    if let Ok(image) = image::load_from_memory(&bytes) {
                        self.friend_avatars.insert(
                            steam_id,
                            texture_from_image(ctx, image, &format!("friend-{steam_id}")),
                        );
                    }
                    ctx.request_repaint();
                }
                Ok(ProfileMsg::Emote { url, bytes }) => {
                    self.emote_pending.remove(&url);
                    if let Ok(image) = image::load_from_memory(&bytes) {
                        self.emotes
                            .insert(url, texture_from_image(ctx, image, "emote"));
                    }
                    ctx.request_repaint();
                }
                Ok(ProfileMsg::CommentAvatar { url, bytes }) => {
                    self.comment_avatar_pending.remove(&url);
                    if let Ok(image) = image::load_from_memory(&bytes) {
                        self.comment_avatars
                            .insert(url, texture_from_image(ctx, image, "comment-avatar"));
                    }
                    ctx.request_repaint();
                }
                // Les capsules passent par le canal des amis : c'est
                // `fetch_game_capsules` qui les publie, pas `fetch` du profil.
                Ok(ProfileMsg::GameCapsule { appid, bytes }) => {
                    self.game_capsule_pending.remove(&appid);
                    if let Ok(image) = image::load_from_memory(&bytes) {
                        self.game_capsules
                            .insert(appid, texture_from_image(ctx, image, "game-capsule"));
                    }
                    ctx.request_repaint();
                }
                // Captures de vitrine, capsules de jeux récents et images des
                // commentaires : `fetch_static_images` les publie ici.
                Ok(ProfileMsg::ShowcaseImage { url, bytes }) => {
                    self.showcase_pending.remove(&url);
                    if let Ok(image) = image::load_from_memory(&bytes) {
                        self.showcase_images
                            .insert(url, texture_from_image(ctx, image, "showcase"));
                    }
                    ctx.request_repaint();
                }
                Ok(ProfileMsg::Emoji { text, bytes }) => {
                    // Une image reçue libère la place ; une absence garde la
                    // demande en attente, sinon chaque image relancerait le
                    // téléchargement.
                    if let Ok(image) = image::load_from_memory(&bytes) {
                        self.emoji_pending.remove(&text);
                        self.emojis
                            .insert(text, texture_from_image(ctx, image, "emoji"));
                    }
                    ctx.request_repaint();
                }
                Ok(ProfileMsg::FamilyProfile {
                    steam_id,
                    name,
                    avatar,
                }) => {
                    if !name.trim().is_empty() {
                        self.family_names.insert(steam_id, name);
                    }
                    if let Some(bytes) = avatar {
                        if let Ok(image) = image::load_from_memory(&bytes) {
                            self.friend_avatars.insert(
                                steam_id,
                                texture_from_image(ctx, image, &format!("friend-{steam_id}")),
                            );
                        }
                    }
                    ctx.request_repaint();
                }
                Ok(_) => {}
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => break,
            }
        }
    }

    fn poll_profile(&mut self, ctx: &egui::Context) {
        let Some(rx) = self.rx.take() else {
            return;
        };
        let mut finished = false;
        loop {
            match rx.try_recv() {
                Ok(ProfileMsg::Loaded(profile)) => {
                    self.profile = Some(profile);
                    self.rate_limited = false;
                    self.retries = 0;
                    self.retry_at = None;
                    self.avatar = None;
                    self.fetch_emotes();
                    ctx.request_repaint();
                }
                Ok(ProfileMsg::RecentHours(hours)) => {
                    if let Some(profile) = self.profile.as_mut() {
                        // Un vrai total XML garde la priorité : la page ne
                        // sert que de repli quand Steam publie 0.0.
                        if hours_missing(&profile.hours_2wk) {
                            profile.hours_2wk = hours;
                            ctx.request_repaint();
                        }
                    }
                }
                Ok(ProfileMsg::Avatar(bytes)) => {
                    if let Ok(image) = image::load_from_memory(&bytes) {
                        self.avatar = Some(texture_from_image(ctx, image, "profile-avatar"));
                    }
                }
                Ok(ProfileMsg::Comments {
                    total,
                    comments,
                    error,
                }) => {
                    self.comments = Some((total, comments));
                    self.comments_error = error;
                    // Photos et visuels des commentaires, puis leurs emoji.
                    self.fetch_comment_avatars();
                    self.fetch_static_images();
                    self.fetch_emotes();
                    ctx.request_repaint();
                }
                Ok(ProfileMsg::Showcases(showcases)) => {
                    self.showcases = Some(showcases);
                    self.fetch_static_images();
                    // Les émoticônes des vitrines ne sont connues qu'ici : la
                    // première lecture du profil les cherchait avant elles.
                    self.fetch_emotes();
                    ctx.request_repaint();
                }
                Ok(ProfileMsg::ShowcaseImage { url, bytes }) => {
                    self.showcase_pending.remove(&url);
                    if let Ok(image) = image::load_from_memory(&bytes) {
                        self.showcase_images
                            .insert(url, texture_from_image(ctx, image, "showcase"));
                    }
                    ctx.request_repaint();
                }
                Ok(ProfileMsg::Failed(error)) => {
                    self.rate_limited = error.rate_limited;
                    self.error = Some(error.message);
                    // Une limite de débit se referme d'elle-même : la fiche
                    // revient sans qu'on ait à cliquer sur « Réessayer ».
                    if error.rate_limited && self.retries < RELOAD_ATTEMPTS {
                        self.retries += 1;
                        self.retry_at = Some(Instant::now() + RELOAD_DELAY);
                    }
                    finished = true;
                    break;
                }
                // Rien d'autre n'arrive par ce fil : les photos et les noms
                // des amis et de la famille ont le leur.
                Ok(_) => {}
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    finished = true;
                    break;
                }
            }
        }
        if finished {
            self.loading = false;
        } else {
            self.rx = Some(rx);
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }

    /// Page entière : la liste des plateformes à gauche, la fiche de celle qui
    /// est choisie à droite — aujourd'hui Steam, avec son identité, ses amis,
    /// sa famille, ses commentaires et ses jeux.
    pub(crate) fn page(
        &mut self,
        ui: &mut egui::Ui,
        account: &str,
        platforms: &[PlatformEntry],
        discord: &mut dyn FnMut(&mut egui::Ui),
        activity: &mut dyn FnMut(&mut egui::Ui),
    ) -> Vec<ProfileAction> {
        let mut actions = Vec::new();
        let account = account.trim().to_string();
        let steam_id = self
            .profile
            .as_ref()
            .and_then(|profile| profile.steam_id.parse::<u64>().ok());
        ui.spacing_mut().item_spacing.y = 10.0;
        // Les emoji des textes affichés partent en images : la police n'a que
        // des glyphes monochromes.
        self.queue_text_emojis();
        let mut open_accounts = None;
        if self.beside_list(ui) {
            // La liste reste en place pendant que la fiche défile : c'est une
            // colonne, pas la première carte de la page.
            let height = ui.available_height().max(240.0);
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.y = 10.0;
                ui.allocate_ui_with_layout(
                    egui::vec2(PLATFORM_SIDE, height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(PLATFORM_SIDE);
                        open_accounts = self.platform_list(ui, platforms, PLATFORM_SIDE);
                    },
                );
                let page = ui.available_width().max(320.0);
                // La colonne garde sa hauteur propre : la fiche défile à
                // l'intérieur, sans emporter la liste.
                ui.allocate_ui_with_layout(
                    egui::vec2(page, height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(page);
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .max_height(height)
                            .show(ui, |ui| {
                                ui.set_width(page - 14.0);
                                // La liste choisit la page : la fiche Steam, ou
                                // l'une des deux pages de la session.
                                match self.platform {
                                    Platform::Discord => discord(ui),
                                    Platform::Activity => activity(ui),
                                    _ => self.steam_page(ui, &account, steam_id, &mut actions),
                                }
                            });
                    },
                );
            });
        } else {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.spacing_mut().item_spacing.y = 10.0;
                    // Étroite : la même liste, au-dessus de la fiche, sans
                    // s'étirer d'un bord à l'autre.
                    open_accounts =
                        self.platform_list(ui, platforms, ui.available_width().min(320.0));
                    match self.platform {
                        Platform::Discord => discord(ui),
                        Platform::Activity => activity(ui),
                        _ => self.steam_page(ui, &account, steam_id, &mut actions),
                    }
                    ui.add_space(12.0);
                });
        }
        if let Some(store) = open_accounts {
            actions.push(ProfileAction::Accounts(store));
        }
        actions
    }

    /// La liste tient-elle sur le côté ? Il faut de la place pour elle, et
    /// assez pour que la fiche Steam garde sa mise en page.
    fn beside_list(&self, ui: &egui::Ui) -> bool {
        let width = ui.available_width();
        let gap = ui.spacing().item_spacing.x;
        width - PLATFORM_SIDE - gap >= STEAM_PAGE
    }

    /// Liste des plateformes : un titre, puis une ligne par plateforme — son
    /// logo dans une tuile, son nom, et le compte qui y est connecté. Pas de
    /// cadre : la liste vit sur le fond de la page, comme un sommaire, pour ne
    /// pas faire un troisième encart à côté de la fiche et de la famille.
    ///
    /// Rend la boutique dont il faut ouvrir les réglages : les pages des autres
    /// boutiques viendront s'ajouter ici, leur compte se règle en attendant
    /// dans Réglages › Comptes.
    fn platform_list(
        &mut self,
        ui: &mut egui::Ui,
        platforms: &[PlatformEntry],
        width: f32,
    ) -> Option<lapinux_core::stores::Store> {
        let mut open = None;
        ui.set_width(width);
        // L'ordre vient de l'appelant : Activité, Discord, puis les
        // plateformes. Activité occupe donc le sommet de la colonne, au-dessus
        // du titre « Plateformes » qui n'annonce que Steam et les boutiques.
        ui.add_space(5.0);
        let mut platforms_titled = false;
        for entry in platforms {
            if !platforms_titled
                && !matches!(entry.platform, Platform::Activity | Platform::Discord)
            {
                platforms_titled = true;
                ui.add_space(2.0);
                // Le titre s'aligne sur la tuile des lignes plutôt que sur le
                // bord de la page.
                ui.horizontal(|ui| {
                    ui.add_space(9.0);
                    ui.label(
                        egui::RichText::new(lapinux_core::i18n::t("ui.profile_platforms"))
                            .size(11.5)
                            .strong()
                            .color(TEXT_DIM()),
                    );
                });
                ui.add_space(8.0);
            }
            let active = entry.platform == self.platform;
            if self.platform_row(ui, entry, width, active) {
                match entry.platform {
                    // Steam, Discord et Activité ouvrent leur page.
                    Platform::Steam | Platform::Discord | Platform::Activity => {
                        self.platform = entry.platform
                    }
                    // Les autres boutiques n'ont pas encore leur propre page :
                    // leur ligne mène au compte qui les branche.
                    other => open = other.store(),
                }
            }
            ui.add_space(2.0);
        }
        open
    }

    /// Une ligne de la liste : tuile au logo, nom, et compte connecté sur cette
    /// plateforme. Rend `true` si la ligne a été cliquée.
    ///
    /// Le pseudo est masqué par défaut — il ne s'affiche qu'en cliquant l'œil
    /// de la ligne, et retombe masqué quand on le reclique.
    fn platform_row(
        &mut self,
        ui: &mut egui::Ui,
        entry: &PlatformEntry,
        width: f32,
        active: bool,
    ) -> bool {
        let platform = entry.platform;
        const HEIGHT: f32 = 52.0;
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(width, HEIGHT), egui::Sense::click());
        let revealed = self.revealed_accounts.contains(&platform);
        // Activité et Discord ne sont pas des comptes : leur ligne dit ce
        // qu'elles montrent, sans cadenas ni « Non connecté ».
        let session_page = matches!(platform, Platform::Activity | Platform::Discord);
        // L'œil occupe le bord droit de la ligne, hors du texte.
        let eye_rect = egui::Rect::from_center_size(
            egui::pos2(rect.right() - 20.0, rect.center().y),
            egui::vec2(26.0, 26.0),
        );
        let eye = (entry.connected && !session_page).then(|| {
            ui.interact(
                eye_rect,
                ui.id().with(("account-eye", platform)),
                egui::Sense::click(),
            )
        });
        let painter = ui.painter();
        if active || response.hovered() {
            painter.rect_filled(
                rect,
                egui::Rounding::same(8.0),
                if active { CARD_ACTIVE() } else { CARD_HOVER() },
            );
        }
        let tile = egui::Rect::from_center_size(
            egui::pos2(rect.left() + 9.0 + 18.0, rect.center().y),
            egui::vec2(36.0, 36.0),
        );
        painter.rect_filled(
            tile,
            egui::Rounding::same(7.0),
            egui::Color32::from_black_alpha(70),
        );
        paint_platform_logo(painter, platform, tile);
        // Le compte s'écrit à côté du nom : ce qui dépasse est coupé par le
        // bord de la ligne plutôt que de déborder sur la fiche. Le texte
        // s'arrête avant l'œil, qui occupe le bord droit.
        let mut clip = rect.shrink(2.0);
        if eye.is_some() {
            clip.max.x -= 32.0;
        }
        let painter = painter.with_clip_rect(clip);
        let left = tile.right() + 9.0;
        painter.text(
            egui::pos2(left, rect.center().y - 7.0),
            egui::Align2::LEFT_CENTER,
            platform.label(),
            egui::FontId::proportional(12.5),
            if active { TEXT() } else { TEXT_DIM() },
        );
        // Le compte connecté, ou ce qu'il reste à faire : une ligne muette
        // « connecté » ne dirait pas à quoi se rattacher. Le pseudo se cache
        // derrière des points tant que l'œil n'a pas été cliqué.
        let (line, color) = if session_page {
            (entry.account.clone(), TEXT_DIM())
        } else if entry.connected {
            if revealed {
                (entry.account.clone(), TEXT_DIM())
            } else {
                ("••••••".to_string(), TEXT_DIM())
            }
        } else {
            (
                lapinux_core::i18n::t("profile.not_connected").to_string(),
                TEXT_DIM(),
            )
        };
        painter.text(
            egui::pos2(left, rect.center().y + 7.5),
            egui::Align2::LEFT_CENTER,
            line,
            egui::FontId::proportional(10.5),
            color,
        );
        if let Some(eye) = eye {
            let hovered = eye.hovered();
            if hovered {
                painter.rect_filled(eye_rect, egui::Rounding::same(6.0), CARD_HOVER());
            }
            paint_eye(
                &painter,
                eye_rect.center(),
                revealed,
                if hovered { TEXT() } else { TEXT_DIM() },
            );
            let _ = eye.clone().on_hover_text(if revealed {
                lapinux_core::i18n::t("profile.hide_nickname")
            } else {
                lapinux_core::i18n::t("profile.show_nickname")
            });
            let _ = eye.clone().on_hover_cursor(egui::CursorIcon::PointingHand);
            if eye.clicked() {
                if revealed {
                    self.revealed_accounts.remove(&platform);
                } else {
                    self.revealed_accounts.insert(platform);
                }
                return false;
            }
        }
        response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
    }

    /// Fiche de la plateforme choisie : celle de Steam aujourd'hui.
    fn steam_page(
        &mut self,
        ui: &mut egui::Ui,
        account: &str,
        steam_id: Option<u64>,
        actions: &mut Vec<ProfileAction>,
    ) {
        // Les deux onglets n'appartiennent qu'au compte Steam : les boutiques
        // ont chacune leur page, sans achat à activer.
        let steam = self.platform == Platform::Steam;
        if steam {
            self.tabs(ui);
        }
        match (steam, self.tab) {
            (true, ProfileTab::Achats) => self.purchases_board(ui, actions),
            _ => {
                self.board(ui, account, steam_id, actions);
                if let Some(profile) = self.profile.clone() {
                    self.private_note(ui, &profile);
                }
            }
        }
        ui.add_space(12.0);
    }

    /// Les deux onglets de la fiche, côte à côte sur toute la largeur : deux
    /// rectangles, celui qui est choisi se distinguant par son fond.
    fn tabs(&mut self, ui: &mut egui::Ui) {
        let width = ui.available_width();
        let segment = (width / 2.0).max(110.0);
        let widths = [segment, segment];
        let active = [
            self.tab == ProfileTab::Profil,
            self.tab == ProfileTab::Achats,
        ];
        let (rect, responses) = crate::segmented_pill_with(
            ui,
            &widths,
            &active,
            "profile-tabs",
            egui::Rounding::same(8.0),
        );
        let painter = ui.painter().clone();
        for (index, label) in [
            lapinux_core::i18n::t("profile.tab_profile"),
            lapinux_core::i18n::t("profile.tab_purchases"),
        ]
        .iter()
        .enumerate()
        {
            let slot = crate::segment_rect(rect, &widths, index);
            let ink = if active[index] { TEXT() } else { TEXT_DIM() };
            painter.text(
                slot.center(),
                egui::Align2::CENTER_CENTER,
                *label,
                egui::FontId::proportional(13.0),
                ink,
            );
            if responses[index].clicked() {
                self.tab = if index == 0 {
                    ProfileTab::Profil
                } else {
                    ProfileTab::Achats
                };
            }
        }
        ui.add_space(10.0);
    }

    /// Onglet Achats : l'activation d'une clé produit sur le compte.
    fn purchases_board(&mut self, ui: &mut egui::Ui, actions: &mut Vec<ProfileAction>) {
        let redeem = self
            .social
            .as_ref()
            .and_then(|social| social.redeem.clone());
        let running = redeem
            .as_ref()
            .is_some_and(|redeem| redeem.state == "running");
        egui::Frame::none()
            .fill(CARD())
            .stroke(egui::Stroke::new(1.0_f32, BORDER()))
            .rounding(egui::Rounding::same(12.0))
            .inner_margin(16.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    egui::RichText::new(lapinux_core::i18n::t("profile.activate_game"))
                        .size(13.0)
                        .strong()
                        .color(TEXT()),
                );
                ui.add_space(3.0);
                muted(ui, lapinux_core::i18n::t("profile.product_key_hint"));
                ui.add_space(10.0);
                let mut submit = false;
                let mut test = false;
                ui.horizontal(|ui| {
                    let field = ui.available_width() - 196.0;
                    let response = ui.add_sized(
                        [field.max(160.0), 30.0],
                        egui::TextEdit::singleline(&mut self.key_input)
                            .hint_text("XXXXX-XXXXX-XXXXX"),
                    );
                    // Une clé se colle puis s'active d'une touche.
                    if response.lost_focus()
                        && ui.input(|input| input.key_pressed(egui::Key::Enter))
                    {
                        submit = true;
                    }
                    ui.add_space(8.0);
                    let button = egui::Button::new(
                        egui::RichText::new(lapinux_core::i18n::t("profile.activate"))
                            .size(12.0)
                            .strong(),
                    );
                    if ui
                        .add_enabled(!running && !self.key_input.trim().is_empty(), button)
                        .clicked()
                    {
                        submit = true;
                    }
                    ui.add_space(8.0);
                    // Le bouton de test ne touche pas au compte : il joue
                    // l'animation seule, avec des jeux de démonstration.
                    if ui
                        .add(egui::Button::new(
                            egui::RichText::new(lapinux_core::i18n::t("profile.test")).size(12.0),
                        ))
                        .on_hover_text(lapinux_core::i18n::t("profile.play_activation"))
                        .clicked()
                    {
                        test = true;
                    }
                });
                if test {
                    actions.push(ProfileAction::TestRedeem);
                }
                if submit && !running {
                    let key = self.key_input.trim().to_string();
                    if !key.is_empty() {
                        self.key_input.clear();
                        actions.push(ProfileAction::RedeemKey(key));
                    }
                }
                if let Some(redeem) = redeem {
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if redeem.state == "running" {
                            crate::section_icons::loading(ui, 18.0);
                        }
                        let color = match redeem.state.as_str() {
                            "ok" => OK,
                            "error" => DANGER,
                            _ => TEXT_DIM(),
                        };
                        ui.label(egui::RichText::new(&redeem.message).size(12.0).color(color));
                    });
                }
            });
    }

    /// La fiche se range en deux colonnes : à gauche l'identité puis les
    /// commentaires, à droite la famille puis les amis. Les deux colonnes
    /// partagent la même largeur à droite, pour que les encarts s'alignent.
    /// Sur une fenêtre étroite, tout s'empile dans cet ordre.
    fn board(
        &mut self,
        ui: &mut egui::Ui,
        account: &str,
        steam_id: Option<u64>,
        actions: &mut Vec<ProfileAction>,
    ) {
        let family_present = self
            .social
            .as_ref()
            .and_then(|social| social.family.as_ref())
            .is_some();
        let width = ui.available_width();
        let gap = ui.spacing().item_spacing.x;
        // La colonne de droite se contente d'une part étroite ; la fiche garde
        // la place dont sa grille de faits a besoin.
        let right = (width * 0.34).clamp(280.0, 420.0);
        let left = width - gap - right;
        if !family_present || left < 560.0 {
            self.identity_card(ui, account, actions);
            self.showcases_card(ui, actions);
            self.family_card(ui);
            self.comments_card(ui, steam_id, actions);
            self.friends_card(ui, steam_id, actions);
            return;
        }
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(left, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(left);
                    ui.spacing_mut().item_spacing.y = 10.0;
                    self.identity_card(ui, account, actions);
                    self.showcases_card(ui, actions);
                    self.comments_card(ui, steam_id, actions);
                },
            );
            ui.allocate_ui_with_layout(
                egui::vec2(right, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(right);
                    ui.spacing_mut().item_spacing.y = 10.0;
                    self.family_card(ui);
                    self.friends_card(ui, steam_id, actions);
                },
            );
        });
    }

    /// Description du profil : le texte, les liens et les émoticônes Steam
    /// (les `:nom:` du profil) rendues à la taille du texte, repliées sur la
    /// largeur. Un lien ouvre le navigateur, comme dans Steam.
    fn description(
        &self,
        ui: &mut egui::Ui,
        parts: &[DescriptionPart],
        actions: &mut Vec<ProfileAction>,
    ) {
        let line_height = ui.fonts(|fonts| fonts.row_height(&egui::FontId::proportional(12.0)));
        // Un seul bloc : les lignes d'un même texte se suivent de près, sans
        // l'espacement des cartes entre chacune.
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            for line in description_lines(parts) {
                ui.horizontal_wrapped(|ui| {
                    // L'espacement vient des mots eux-mêmes.
                    ui.spacing_mut().item_spacing.x = 0.0;
                    for piece in line {
                        match piece {
                            DescriptionPiece::Word(word) => {
                                for (emoji, chunk) in split_emoji(word) {
                                    if !emoji {
                                        ui.label(
                                            egui::RichText::new(chunk).size(12.0).color(TEXT_DIM()),
                                        );
                                        continue;
                                    }
                                    match self.emojis.get(chunk) {
                                        Some(texture) => {
                                            let side = line_height * 1.15;
                                            ui.add(
                                                egui::Image::new((
                                                    texture.id(),
                                                    egui::vec2(side, side),
                                                ))
                                                .fit_to_exact_size(egui::vec2(side, side)),
                                            );
                                        }
                                        None => {
                                            ui.label(
                                                egui::RichText::new(chunk)
                                                    .size(12.0)
                                                    .color(TEXT_DIM()),
                                            );
                                        }
                                    }
                                }
                            }
                            DescriptionPiece::Link { text, url } => {
                                let response = ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(text)
                                            .size(12.0)
                                            .color(BLUETOOTH_BLUE)
                                            .underline(),
                                    )
                                    .sense(egui::Sense::click()),
                                );
                                let response = response
                                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                                    .on_hover_text(url);
                                if response.clicked() {
                                    actions.push(ProfileAction::Open(url.to_string()));
                                }
                            }
                            DescriptionPiece::Emote { name, url } => {
                                match self.emotes.get(url) {
                                    Some(texture) => {
                                        let side = line_height * 1.1;
                                        let size = egui::vec2(side, side);
                                        let response = ui.add(
                                            egui::Image::new((texture.id(), size))
                                                .fit_to_exact_size(size),
                                        );
                                        response.on_hover_text(format!(":{name}:"));
                                    }
                                    // En attendant l'image (ou si Steam ne la
                                    // donne pas), le code reste lisible.
                                    None => {
                                        ui.label(
                                            egui::RichText::new(format!(":{name}:"))
                                                .size(12.0)
                                                .color(TEXT_DIM()),
                                        );
                                    }
                                }
                            }
                        }
                    }
                });
            }
        });
    }

    /// Ligne du haut de la fiche : l'identifiant, sa copie et la relecture,
    /// à la hauteur de la photo.
    fn header_tools(
        &mut self,
        ui: &mut egui::Ui,
        profile: &SteamProfile,
        actions: &mut Vec<ProfileAction>,
    ) {
        if refresh_button(ui, lapinux_core::i18n::t("profile.reload_profile")).clicked() {
            actions.push(ProfileAction::Reload);
        }
        let steam_id = profile.steam_id.clone();
        let (rect, response) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::click());
        paint_copy_glyph(ui.painter(), rect, TEXT_DIM());
        if response
            .clone()
            .on_hover_text(lapinux_core::i18n::t("profile.copy_steamid"))
            .clicked()
            && !steam_id.is_empty()
        {
            actions.push(ProfileAction::Copy(steam_id));
        }
        // L'identifiant est peint dans son rectangle : un libellé aligné à
        // droite se décalerait sur le glyphe de copie.
        let label = if self.id_revealed {
            format!("ID {}", profile.steam_id)
        } else {
            masked_steam_id(&profile.steam_id)
        };
        let size = text_size(ui, &label, 10.5);
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
        painted_text_at(ui, rect.min, &label, 10.5, TEXT_DIM());
        if response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(if self.id_revealed {
                lapinux_core::i18n::t("profile.hide_id")
            } else {
                lapinux_core::i18n::t("profile.show_id")
            })
            .clicked()
        {
            self.id_revealed = !self.id_revealed;
        }
    }

    fn identity_card(
        &mut self,
        ui: &mut egui::Ui,
        account: &str,
        actions: &mut Vec<ProfileAction>,
    ) {
        egui::Frame::none()
            .fill(CARD())
            .stroke(egui::Stroke::new(1.0_f32, BORDER()))
            .rounding(egui::Rounding::same(12.0))
            .inner_margin(16.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let Some(profile) = self.profile.clone() else {
                    self.identity_placeholder(ui, account);
                    return;
                };
                // La photo, le nom et l'identifiant partagent la même ligne :
                // l'identifiant se cale au coin haut droit, à la hauteur de la
                // photo, sans laisser de bande vide au-dessus.
                ui.horizontal_top(|ui| {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(76.0, 76.0), egui::Sense::hover());
                    ui.painter()
                        .rect_filled(rect, egui::Rounding::same(12.0), CARD_HOVER());
                    match &self.avatar {
                        Some(texture) => {
                            egui::Image::new((texture.id(), rect.size()))
                                .rounding(egui::Rounding::same(12.0))
                                .paint_at(ui, rect);
                        }
                        None => {
                            ui.painter().text(
                                rect.center(),
                                egui::Align2::CENTER_CENTER,
                                "?",
                                display_font(26.0),
                                TEXT_DIM(),
                            );
                        }
                    }
                    ui.add_space(12.0);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 5.0;
                        // Le nom, puis son menu d'état collé à lui : le point
                        // de présence est peint dans le bouton du menu.
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(&profile.name)
                                    .size(20.0)
                                    .strong()
                                    .color(TEXT()),
                            );
                            ui.add_space(6.0);
                            self.presence_dropdown(ui, self.status_color(&profile), actions);
                        });
                        // Le nom réel garde la couleur du texte : un émoji
                        // monochrome y est bien plus lisible que dans le gris
                        // des libellés ; la visibilité le suit.
                        if !profile.real_name.is_empty() || !profile.privacy_state.is_empty() {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                if !profile.real_name.is_empty() {
                                    muted(ui, lapinux_core::i18n::t("profile.real_name"));
                                    ui.label(
                                        egui::RichText::new(&profile.real_name)
                                            .size(11.0)
                                            .color(TEXT()),
                                    );
                                }
                                if !profile.privacy_state.is_empty() {
                                    let privacy = privacy_label(&profile.privacy_state);
                                    muted(
                                        ui,
                                        if profile.real_name.is_empty() {
                                            privacy
                                        } else {
                                            format!("· {privacy}")
                                        },
                                    );
                                }
                            });
                        }
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                        self.header_tools(ui, &profile, actions);
                    });
                });
                if !profile.description.is_empty() {
                    ui.add_space(10.0);
                    self.description(ui, &profile.description, actions);
                }
                ui.add_space(12.0);
                egui::Grid::new("profile-facts")
                    .num_columns(4)
                    .spacing([26.0, 6.0])
                    .show(ui, |ui| {
                        fact(
                            ui,
                            lapinux_core::i18n::t("profile.member_since"),
                            &profile.member_since,
                        );
                        fact(
                            ui,
                            lapinux_core::i18n::t("profile.location_label"),
                            &profile.location,
                        );
                        fact(
                            ui,
                            lapinux_core::i18n::t("profile.playtime_2weeks"),
                            &format_hours(&profile.hours_2wk),
                        );
                        fact(ui, "Adresse", &profile.custom_url);
                    });
            });
    }

    /// Pastille d'identité : la présence publiée quand la session sociale est
    /// vivante, sinon l'état public du profil.
    fn status_color(&self, profile: &SteamProfile) -> egui::Color32 {
        if let Some(social) = self.social.as_ref().filter(|social| social.live()) {
            return presence_color(social.presence);
        }
        online_label(profile).0
    }

    /// Sélecteur de présence, à la place du libellé d'état : le choix part au
    /// daemon, qui le publie sur Steam sans redémarrer la session. Le point
    /// d'état est peint dans le bouton même du menu, devant son libellé : il
    /// n'occupe pas de place à côté du nom.
    fn presence_dropdown(
        &self,
        ui: &mut egui::Ui,
        color: egui::Color32,
        actions: &mut Vec<ProfileAction>,
    ) {
        let current = self
            .social
            .as_ref()
            .map(|social| social.presence)
            .unwrap_or_default();
        let mut choice = current;
        // Les espaces réservent la place du point : le bouton reste un menu
        // déroulant ordinaire, avec sa liste, son clavier et sa fermeture.
        let response = egui::ComboBox::from_id_salt("profile-presence")
            .width(118.0)
            .selected_text(
                egui::RichText::new(format!("     {}", current.label()))
                    .size(11.5)
                    .color(TEXT_DIM()),
            )
            .show_ui(ui, |ui| {
                for option in SteamPresence::choices() {
                    ui.selectable_value(&mut choice, option, option.label());
                }
            });
        let rect = response.response.rect;
        ui.painter()
            .circle_filled(egui::pos2(rect.left() + 11.0, rect.center().y), 3.2, color);
        if choice != current {
            actions.push(ProfileAction::SetPresence(choice));
        }
    }

    fn identity_placeholder(&mut self, ui: &mut egui::Ui, account: &str) {
        if self.loading {
            ui.horizontal(|ui| {
                crate::section_icons::loading(ui, 18.0);
                muted(ui, lapinux_core::i18n::t("profile.reading_public"));
            });
            return;
        }
        if self.error.is_some() {
            if let Some(error) = self.error.as_deref() {
                ui.horizontal(|ui| {
                    status_dot(ui, DANGER);
                    ui.label(egui::RichText::new(error).size(12.5).color(TEXT()));
                });
            }
            ui.add_space(4.0);
            // Une limite de débit n'est pas une configuration manquante : la
            // bannière le dit, au lieu de renvoyer vers les réglages du compte.
            if self.rate_limited {
                muted(ui, self.rate_limit_note());
            } else {
                muted(ui, lapinux_core::i18n::t("ui.profile_steamid_required"));
            }
            ui.add_space(4.0);
            if link_text(ui, lapinux_core::i18n::t("configurations.retry")).clicked() {
                if let Some(steam_id) = self.loaded_for {
                    self.reload(steam_id);
                }
            }
            return;
        }
        if account.is_empty() {
            ui.label(
                egui::RichText::new(lapinux_core::i18n::t(
                    "settings.connection.no_steam_account",
                ))
                .size(15.0)
                .strong()
                .color(TEXT()),
            );
            muted(ui, lapinux_core::i18n::t("ui.profile_connect_hint"));
        } else {
            muted(ui, lapinux_core::i18n::t("profile.not_read"));
        }
    }

    /// Amis poussés par la session sociale : en jeu, en ligne, puis hors ligne.
    fn friends_card(
        &mut self,
        ui: &mut egui::Ui,
        steam_id: Option<u64>,
        actions: &mut Vec<ProfileAction>,
    ) {
        let Some(social) = self.social.clone() else {
            return;
        };
        egui::Frame::none()
            .fill(CARD())
            .stroke(egui::Stroke::new(1.0_f32, BORDER()))
            .rounding(egui::Rounding::same(12.0))
            .inner_margin(16.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(lapinux_core::i18n::t("friends.title"))
                            .size(13.0)
                            .strong()
                            .color(TEXT()),
                    );
                    if social.live() {
                        let online = social.online_friends().len();
                        muted(
                            ui,
                            format!("{} en ligne · {} au total", online, social.friends.len()),
                        );
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if refresh_button(ui, lapinux_core::i18n::t("profile.refresh_friends"))
                            .clicked()
                        {
                            actions.push(ProfileAction::RefreshSocial);
                        }
                    });
                });
                if !social.invites.is_empty() {
                    ui.add_space(8.0);
                    for invite in &social.invites {
                        ui.horizontal(|ui| {
                            let who = invite.inviter_name.clone().unwrap_or_else(|| {
                                lapinux_core::i18n::t_fmt(
                                    "profile.friend_number",
                                    &[invite.inviter.to_string()],
                                )
                            });
                            muted(
                                ui,
                                lapinux_core::i18n::t_args(
                                    "profile.invite_lobby",
                                    &[("who", &who)],
                                ),
                            );
                            if ui.button(lapinux_core::i18n::t("profile.join")).clicked() {
                                actions.push(ProfileAction::JoinInvite {
                                    appid: invite.appid,
                                    lobby: invite.lobby,
                                    connect: invite.connect.clone().unwrap_or_default(),
                                });
                            }
                            if invite.connect.is_none() {
                                muted(ui, lapinux_core::i18n::t("profile.lobby_address"));
                            }
                        });
                    }
                }
                if !social.live() {
                    ui.add_space(6.0);
                    muted(
                        ui,
                        social.error.clone().unwrap_or_else(|| {
                            lapinux_core::i18n::t("profile.social_connecting").into()
                        }),
                    );
                    return;
                }
                if social.friends.is_empty() {
                    ui.add_space(6.0);
                    muted(ui, lapinux_core::i18n::t("profile.no_friends"));
                    return;
                }
                // Quatre catégories, comme dans Steam : ceux qui jouent,
                // ceux qui sont disponibles, les absents, puis les hors ligne.
                // Steam considère un absent comme « en ligne » ; il ne doit
                // pourtant jamais passer avant un ami réellement disponible.
                let mut playing: Vec<&SteamFriend> = Vec::new();
                let mut online: Vec<&SteamFriend> = Vec::new();
                let mut away: Vec<&SteamFriend> = Vec::new();
                let mut offline: Vec<&SteamFriend> = Vec::new();
                for friend in &social.friends {
                    if friend.in_game() {
                        playing.push(friend);
                    } else if matches!(
                        friend.presence,
                        lapinux_core::social::SteamPresence::Away
                            | lapinux_core::social::SteamPresence::Snooze
                    ) {
                        away.push(friend);
                    } else if friend.presence.online() {
                        online.push(friend);
                    } else {
                        offline.push(friend);
                    }
                }
                for group in [&mut playing, &mut online, &mut away, &mut offline] {
                    group.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
                }
                for (label, group) in [
                    (lapinux_core::i18n::t("profile.in_game"), &playing),
                    (lapinux_core::i18n::t("profile.online"), &online),
                    (lapinux_core::i18n::t("profile.away"), &away),
                ] {
                    self.friend_category(ui, label, group, actions);
                }
                if !offline.is_empty() {
                    self.friend_category(
                        ui,
                        lapinux_core::i18n::t("profile.offline"),
                        &offline,
                        actions,
                    );
                }
                let _ = steam_id;
            });
    }

    /// Une catégorie de la liste d'amis, dans le style des catégories de
    /// Téléchargements : un titre, son compte, un chevron, et un clic qui la
    /// replie. « Hors ligne » reste fermée tant qu'on ne l'ouvre pas.
    fn friend_category(
        &mut self,
        ui: &mut egui::Ui,
        label: &str,
        group: &[&SteamFriend],
        actions: &mut Vec<ProfileAction>,
    ) {
        if group.is_empty() {
            return;
        }
        let mut open = !self.collapsed_friends.contains(label);
        let header =
            crate::LapinuxApp::section_heading(ui, label, &group.len().to_string(), None, open);
        if header.toggle {
            open = !open;
            if open {
                self.collapsed_friends.remove(label);
            } else {
                self.collapsed_friends.insert(label.to_string());
            }
        }
        if !open {
            // Repliée, la catégorie montre les photos de ses amis, comme les
            // catégories de Téléchargements montrent leurs jeux.
            self.collapsed_friend_faces(ui, header.free, group);
            return;
        }
        for friend in group {
            self.friend_row(ui, friend, actions);
        }
    }

    /// Photos des amis d'une catégorie repliée, dans la place libre de son
    /// en-tête : les premières, puis « +N ».
    fn collapsed_friend_faces(&self, ui: &mut egui::Ui, free: egui::Rect, group: &[&SteamFriend]) {
        if group.is_empty() || free.width() < 24.0 {
            return;
        }
        const SIZE: f32 = 20.0;
        const GAP: f32 = 5.0;
        let (shown, more, offset) = crate::downloads::collapsed_strip(free.width(), group.len());
        let painter = ui.painter().clone();
        let mut x = free.left() + offset;
        let top = free.center().y - SIZE / 2.0;
        for friend in group.iter().take(shown) {
            let rect = egui::Rect::from_min_size(egui::pos2(x, top), egui::vec2(SIZE, SIZE));
            let rounding = egui::Rounding::same(SIZE / 2.0);
            painter.rect_filled(rect, rounding, egui::Color32::from_black_alpha(70));
            match self.friend_avatars.get(&friend.steam_id) {
                Some(texture) => {
                    egui::Image::new((texture.id(), rect.size()))
                        .rounding(rounding)
                        .paint_at(ui, rect);
                }
                None => {
                    let initial = friend
                        .name
                        .trim()
                        .chars()
                        .next()
                        .map(|letter| letter.to_uppercase().to_string())
                        .unwrap_or_else(|| "?".into());
                    painter.text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        initial,
                        egui::FontId::proportional(10.5),
                        TEXT_DIM(),
                    );
                }
            }
            x += SIZE + GAP;
        }
        if more > 0 {
            painter.text(
                egui::pos2(x + 1.0, free.center().y),
                egui::Align2::LEFT_CENTER,
                format!("+{more}"),
                egui::FontId::proportional(10.5),
                TEXT_DIM(),
            );
        }
    }

    /// Photo de l'auteur d'un commentaire, ou son initiale le temps qu'elle
    /// arrive.
    fn comment_avatar_cell(&self, ui: &mut egui::Ui, comment: &CommentView, size: f32) {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
        let rounding = egui::Rounding::same(size * 0.3);
        ui.painter().rect_filled(rect, rounding, CARD_HOVER());
        if let Some(texture) = self.comment_avatars.get(&comment.avatar) {
            egui::Image::new((texture.id(), rect.size()))
                .rounding(rounding)
                .paint_at(ui, rect);
            return;
        }
        let initial = comment
            .author
            .trim()
            .chars()
            .next()
            .map(|letter| letter.to_uppercase().to_string())
            .unwrap_or_else(|| "?".into());
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            initial,
            egui::FontId::proportional(size * 0.46),
            TEXT_DIM(),
        );
    }

    /// Photo de profil d'un compte : l'image téléchargée, sinon l'initiale du
    /// nom, le temps qu'elle arrive.
    fn avatar_cell(&self, ui: &mut egui::Ui, steam_id: u64, name: &str, size: f32) {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
        let rounding = egui::Rounding::same(size * 0.25);
        ui.painter().rect_filled(rect, rounding, CARD_HOVER());
        if let Some(texture) = self.friend_avatars.get(&steam_id) {
            egui::Image::new((texture.id(), rect.size()))
                .rounding(rounding)
                .paint_at(ui, rect);
            return;
        }
        let initial = name
            .trim()
            .chars()
            .next()
            .map(|letter| letter.to_uppercase().to_string())
            .unwrap_or_else(|| "?".into());
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            initial,
            egui::FontId::proportional(size * 0.46),
            TEXT_DIM(),
        );
    }

    fn friend_row(
        &self,
        ui: &mut egui::Ui,
        friend: &SteamFriend,
        actions: &mut Vec<ProfileAction>,
    ) {
        ui.horizontal(|ui| {
            self.avatar_cell(ui, friend.steam_id, &friend.name, 28.0);
            ui.add_space(8.0);
            let name = if friend.name.trim().is_empty() {
                "Ami Steam".to_string()
            } else {
                friend.name.clone()
            };
            let playing = friend.in_game();
            // Chaque bout de la ligne réserve sa place : la capsule du jeu se
            // glisse entre le pseudo et « Joue à … », la pastille ferme la
            // ligne.
            let name_width = (ui.available_width() * 0.4).max(56.0);
            ui.allocate_ui_with_layout(
                egui::vec2(name_width, 18.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.add(
                        egui::Label::new(egui::RichText::new(name).size(12.5).color(TEXT()))
                            .truncate(),
                    );
                },
            );
            if playing {
                // Le jeu mène à sa boutique, comme un lien de la fiche.
                let mut open_game = false;
                if let Some(capsule) = self.game_capsules.get(&friend.game_appid) {
                    let response = ui
                        .add(
                            egui::Image::new((capsule.id(), egui::vec2(48.0, 18.0)))
                                .rounding(egui::Rounding::same(3.0)),
                        )
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_text(lapinux_core::i18n::t("profile.open_game_page"));
                    open_game |= response.clicked();
                    ui.add_space(6.0);
                }
                // Le nom du jeu vient de la présence riche de Steam ; quand
                // elle ne le donne pas, l'appid suffit à dire qu'il joue.
                // La ligne se lit « Joue à … », comme dans Steam, au lieu d'un
                // titre poussé contre le bord droit.
                let title = if friend.game.trim().is_empty() {
                    lapinux_core::i18n::t("profile.a_game").to_string()
                } else {
                    friend.game.clone()
                };
                let title_width = (ui.available_width() - 18.0).max(24.0);
                let response = ui
                    .add_sized(
                        [title_width, 18.0],
                        egui::Label::new(
                            egui::RichText::new(lapinux_core::i18n::t_args(
                                "profile.playing",
                                &[("title", &title.to_string())],
                            ))
                            .size(11.5)
                            .color(TEXT_DIM()),
                        )
                        .truncate()
                        .sense(egui::Sense::click()),
                    )
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text(lapinux_core::i18n::t("profile.open_game_page"));
                open_game |= response.clicked();
                if open_game && friend.game_appid != 0 {
                    actions.push(ProfileAction::Game {
                        appid: friend.game_appid,
                        name: friend.game.clone(),
                    });
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    status_dot(ui, BLUETOOTH_BLUE);
                });
            } else {
                let color = presence_color(friend.presence);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    status_dot(ui, color);
                    muted(ui, last_seen_line(friend));
                });
            }
        });
        ui.add_space(4.0);
    }

    /// Famille Steam : photo et nom de chaque membre, comme la liste d'amis.
    /// Aucun rôle ni identifiant à l'écran : le nom vient de la fiche du joueur
    /// (le groupe familial ne publie que des identifiants).
    fn family_card(&mut self, ui: &mut egui::Ui) {
        let Some(family) = self
            .social
            .as_ref()
            .and_then(|social| social.family.clone())
        else {
            self.family_games = None;
            return;
        };
        // Les noms lus sur les pages publiques complètent ceux de la session.
        let names: HashMap<u64, String> = self.family_names.clone();
        egui::Frame::none()
            .fill(CARD())
            .stroke(egui::Stroke::new(1.0_f32, BORDER()))
            .rounding(egui::Rounding::same(12.0))
            .inner_margin(16.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(lapinux_core::i18n::t("ui.profile_family"))
                            .size(13.0)
                            .strong()
                            .color(TEXT()),
                    );
                    if !family.name.is_empty() {
                        muted(ui, format!("· {}", family.name));
                    }
                });
                ui.add_space(8.0);
                for member in &family.members {
                    let name = family_member_name(member, &names);
                    ui.horizontal(|ui| {
                        self.avatar_cell(ui, member.steam_id, &name, 28.0);
                        ui.add_space(8.0);
                        if !name.trim().is_empty() {
                            ui.label(egui::RichText::new(name).size(12.5).color(TEXT()));
                        }
                        // Le nombre de jeux que ce membre apporte à la
                        // bibliothèque partagée : Steam le publie jeu par jeu,
                        // un jeu appartenant à plusieurs membres.
                        if member.games > 0 {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let response = ui
                                        .add(
                                            egui::Button::new(
                                                egui::RichText::new(lapinux_core::i18n::t_fmt(
                                                    "profile.games_count",
                                                    &[member.games.to_string()],
                                                ))
                                                .size(11.0)
                                                .color(TEXT_DIM()),
                                            )
                                            .frame(false),
                                        )
                                        .on_hover_text(lapinux_core::i18n::t(
                                            "profile.family_games_hint",
                                        ));
                                    if response.clicked() {
                                        self.family_games = Some(member.steam_id);
                                    }
                                },
                            );
                        }
                    });
                    ui.add_space(4.0);
                }
            });
    }

    /// Liste des jeux partagés par un membre, ouverte depuis le compteur de
    /// la famille. Le panneau est centré au-dessus d'un voile : les vignettes
    /// des lignes visibles se chargent au fur et à mesure du défilement.
    pub(crate) fn family_games_modal(
        &mut self,
        ctx: &egui::Context,
        catalog: Option<&lapinux_core::model::SteamCatalog>,
    ) {
        if !matches!(self.platform, Platform::Steam) {
            self.family_games = None;
            return;
        }
        let Some(steam_id) = self.family_games else {
            return;
        };
        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.family_games = None;
            return;
        }
        let Some(family) = self
            .social
            .as_ref()
            .and_then(|social| social.family.clone())
        else {
            self.family_games = None;
            return;
        };
        let Some(member) = family
            .members
            .iter()
            .find(|member| member.steam_id == steam_id)
            .cloned()
        else {
            self.family_games = None;
            return;
        };
        let mut name = family_member_name(&member, &self.family_names);
        if name.trim().is_empty() {
            name = lapinux_core::i18n::t("profile.family_member").to_string();
        }
        let title =
            lapinux_core::i18n::t_args("profile.family_games_title", &[("name", name.as_str())]);
        let games = family_games(&member.appids, catalog);
        let subtitle = (!games.is_empty()).then(|| {
            lapinux_core::i18n::t_fmt("profile.family_games_count", &[games.len().to_string()])
        });
        const HEADER_HEIGHT: f32 = 82.0;
        const ROW_HEIGHT: f32 = 36.0;
        let screen = ctx.screen_rect();
        let visible_rows = games.len().clamp(1, 10) as f32;
        let panel_width = (screen.width() - 32.0).clamp(300.0, 480.0);
        let panel_height = (HEADER_HEIGHT + 30.0 + visible_rows * ROW_HEIGHT)
            .clamp(220.0, 620.0)
            .min((screen.height() - 32.0).max(200.0));
        let panel = egui::vec2(panel_width, panel_height);
        let rect = egui::Rect::from_center_size(screen.center(), panel);
        let header_rect = egui::Rect::from_min_max(
            rect.min,
            egui::pos2(rect.right(), rect.top() + HEADER_HEIGHT),
        );
        let body_rect =
            egui::Rect::from_min_max(egui::pos2(rect.left(), header_rect.bottom()), rect.max);
        let mut close = false;
        egui::Window::new("family-games")
            .id(egui::Id::new(("family-games", steam_id)))
            .title_bar(false)
            .resizable(false)
            .collapsible(false)
            .movable(false)
            .fixed_pos(screen.min)
            .fixed_size(screen.size())
            .frame(egui::Frame::none())
            .show(ctx, |ui| {
                ui.painter()
                    .rect_filled(screen, 0.0, egui::Color32::from_black_alpha(190));
                // Le voile absorbe les clics hors du panneau : la page en
                // dessous ne reçoit rien pendant que le modal est ouvert.
                let outside = [
                    egui::Rect::from_min_max(screen.min, egui::pos2(rect.left(), screen.max.y)),
                    egui::Rect::from_min_max(egui::pos2(rect.right(), screen.min.y), screen.max),
                    egui::Rect::from_min_max(
                        egui::pos2(rect.left(), screen.min.y),
                        egui::pos2(rect.right(), rect.top()),
                    ),
                    egui::Rect::from_min_max(
                        egui::pos2(rect.left(), rect.bottom()),
                        egui::pos2(rect.right(), screen.max.y),
                    ),
                ];
                for (index, area) in outside.into_iter().enumerate() {
                    if area.width() > 0.0 && area.height() > 0.0 {
                        let _ = ui.interact(
                            area,
                            egui::Id::new(("family-games-outside", steam_id, index)),
                            egui::Sense::click(),
                        );
                    }
                }
                ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                    egui::Frame::none()
                        .fill(CARD())
                        .stroke(egui::Stroke::new(1.0_f32, BORDER()))
                        .rounding(egui::Rounding::same(16.0))
                        .shadow(egui::epaint::Shadow {
                            offset: egui::vec2(0.0, 18.0),
                            blur: 44.0,
                            spread: 0.0,
                            color: egui::Color32::from_black_alpha(170),
                        })
                        .show(ui, |ui| {
                            ui.set_min_size(panel);
                            ui.set_max_size(panel);
                            let painter = ui.painter();
                            painter.rect_filled(
                                header_rect,
                                egui::Rounding {
                                    nw: 16.0,
                                    ne: 16.0,
                                    sw: 0.0,
                                    se: 0.0,
                                },
                                BACKGROUND(),
                            );
                            painter.line_segment(
                                [
                                    egui::pos2(rect.left(), header_rect.bottom()),
                                    egui::pos2(rect.right(), header_rect.bottom()),
                                ],
                                egui::Stroke::new(1.0_f32, BORDER()),
                            );
                            ui.scope_builder(
                                egui::UiBuilder::new()
                                    .max_rect(header_rect.shrink2(egui::vec2(16.0, 0.0))),
                                |ui| {
                                    ui.add_space(15.0);
                                    ui.horizontal(|ui| {
                                        self.avatar_cell(ui, steam_id, &name, 40.0);
                                        ui.add_space(11.0);
                                        let text_width = (ui.available_width() - 34.0).max(120.0);
                                        ui.allocate_ui_with_layout(
                                            egui::vec2(text_width, 42.0),
                                            egui::Layout::top_down(egui::Align::Min),
                                            |ui| {
                                                ui.add(
                                                    egui::Label::new(
                                                        egui::RichText::new(&title)
                                                            .size(15.5)
                                                            .strong()
                                                            .color(TEXT()),
                                                    )
                                                    .truncate(),
                                                );
                                                ui.add_space(3.0);
                                                if let Some(subtitle) = &subtitle {
                                                    muted(ui, subtitle.clone());
                                                }
                                            },
                                        );
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                let close_button = ui.add(
                                                    egui::Button::new(
                                                        egui::RichText::new("×")
                                                            .size(19.0)
                                                            .color(TEXT_DIM()),
                                                    )
                                                    .frame(false),
                                                );
                                                if close_button.clicked() {
                                                    close = true;
                                                }
                                            },
                                        );
                                    });
                                },
                            );
                            let body_inner = body_rect.shrink2(egui::vec2(14.0, 9.0));
                            ui.scope_builder(egui::UiBuilder::new().max_rect(body_inner), |ui| {
                                ui.set_clip_rect(body_inner);
                                if games.is_empty() {
                                    ui.add_space(24.0);
                                    ui.horizontal(|ui| {
                                        ui.add_space(8.0);
                                        crate::section_icons::loading(ui, 18.0);
                                        ui.add_space(8.0);
                                        muted(
                                            ui,
                                            lapinux_core::i18n::t("profile.family_games_pending"),
                                        );
                                    });
                                    return;
                                }
                                ui.spacing_mut().item_spacing.y = 0.0;
                                let list_height = (body_inner.height() - 2.0).max(ROW_HEIGHT);
                                egui::ScrollArea::vertical()
                                    .id_salt(("family-games-list", steam_id))
                                    .auto_shrink([false, false])
                                    .max_height(list_height)
                                    .show_rows(ui, ROW_HEIGHT, games.len(), |ui, range| {
                                        let start = range.start.saturating_sub(3);
                                        let end = (range.end + 6).min(games.len());
                                        let appids: Vec<u32> = games[start..end]
                                            .iter()
                                            .map(|game| game.appid)
                                            .collect();
                                        self.request_game_capsules(&appids);
                                        for index in range {
                                            let game = &games[index];
                                            let row_rect = egui::Rect::from_min_size(
                                                ui.cursor().min,
                                                egui::vec2(ui.available_width(), ROW_HEIGHT),
                                            );
                                            if ui
                                                .ctx()
                                                .pointer_hover_pos()
                                                .map_or(false, |pos| row_rect.contains(pos))
                                            {
                                                ui.painter().rect_filled(
                                                    row_rect.shrink2(egui::vec2(4.0, 2.0)),
                                                    egui::Rounding::same(7.0),
                                                    CARD_HOVER(),
                                                );
                                            }
                                            ui.allocate_ui_with_layout(
                                                row_rect.size(),
                                                egui::Layout::left_to_right(egui::Align::Center),
                                                |ui| {
                                                    ui.add_space(8.0);
                                                    let (icon_rect, _) = ui.allocate_exact_size(
                                                        egui::vec2(56.0, 22.0),
                                                        egui::Sense::hover(),
                                                    );
                                                    ui.painter().rect_filled(
                                                        icon_rect,
                                                        egui::Rounding::same(4.0),
                                                        CARD_HOVER(),
                                                    );
                                                    if let Some(texture) =
                                                        self.game_capsules.get(&game.appid)
                                                    {
                                                        egui::Image::new((
                                                            texture.id(),
                                                            icon_rect.size(),
                                                        ))
                                                        .rounding(egui::Rounding::same(4.0))
                                                        .paint_at(ui, icon_rect);
                                                    }
                                                    ui.painter().rect_stroke(
                                                        icon_rect,
                                                        egui::Rounding::same(4.0),
                                                        egui::Stroke::new(1.0_f32, BORDER()),
                                                    );
                                                    ui.add_space(11.0);
                                                    let label_width =
                                                        (ui.available_width() - 10.0).max(80.0);
                                                    ui.allocate_ui_with_layout(
                                                        egui::vec2(label_width, 20.0),
                                                        egui::Layout::left_to_right(
                                                            egui::Align::Center,
                                                        ),
                                                        |ui| {
                                                            ui.add(
                                                                egui::Label::new(
                                                                    egui::RichText::new(&game.name)
                                                                        .size(12.5)
                                                                        .color(TEXT()),
                                                                )
                                                                .truncate(),
                                                            )
                                                            .on_hover_text(&game.name);
                                                        },
                                                    );
                                                    ui.add_space(8.0);
                                                },
                                            );
                                        }
                                    });
                            });
                        });
                });
            });
        if close {
            self.family_games = None;
        }
    }

    /// Vitrines du profil, un encart chacune, sous la fiche : le joueur y
    /// montre ce qu'il aime, ses jeux récents ou quelques objets. Le texte
    /// garde ses liens et ses émoticônes, comme la description.
    fn showcases_card(&mut self, ui: &mut egui::Ui, actions: &mut Vec<ProfileAction>) {
        let Some(showcases) = self.showcases.clone() else {
            // La page de Steam met un instant : la carte dit ce qu'elle
            // attend, au lieu de laisser un trou dans la fiche.
            let message = if self.rate_limited {
                lapinux_core::i18n::t("profile.showcases_rate_limited")
            } else {
                lapinux_core::i18n::t("profile.showcases_loading")
            };
            egui::Frame::none()
                .fill(CARD())
                .stroke(egui::Stroke::new(1.0_f32, BORDER()))
                .rounding(egui::Rounding::same(12.0))
                .inner_margin(16.0)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(
                        egui::RichText::new(lapinux_core::i18n::t("ui.profile_showcases"))
                            .size(13.0)
                            .strong()
                            .color(TEXT()),
                    );
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if !self.rate_limited {
                            crate::section_icons::loading(ui, 18.0);
                        }
                        muted(ui, message);
                    });
                });
            return;
        };
        if showcases.is_empty() {
            return;
        }
        // Textures copiées avant la boucle : la carte se dessine sans retenir
        // l'état du profil.
        let images = self.showcase_images.clone();
        egui::Frame::none()
            .fill(CARD())
            .stroke(egui::Stroke::new(1.0_f32, BORDER()))
            .rounding(egui::Rounding::same(12.0))
            .inner_margin(16.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    egui::RichText::new(lapinux_core::i18n::t("ui.profile_showcases"))
                        .size(13.0)
                        .strong()
                        .color(TEXT()),
                );
                ui.add_space(10.0);
                for (index, showcase) in showcases.iter().enumerate() {
                    if index > 0 {
                        ui.add_space(14.0);
                    }
                    // Titre et, pour l'activité récente, le temps des
                    // quinze derniers jours.
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(&showcase.title)
                                .size(12.0)
                                .strong()
                                .color(TEXT_DIM()),
                        );
                        if !showcase.subtitle.is_empty() {
                            muted(ui, format!("· {}", showcase.subtitle));
                        }
                    });
                    ui.add_space(6.0);
                    if showcase.games.is_empty() {
                        for line in &showcase.lines {
                            ui.label(egui::RichText::new(line).size(12.0).color(TEXT()));
                        }
                    }
                    // Chaque jeu récent : sa capsule, son nom, ses heures et
                    // son dernier lancement, puis ses succès.
                    for game in &showcase.games {
                        ui.horizontal_top(|ui| {
                            let (rect, _) = ui
                                .allocate_exact_size(egui::vec2(112.0, 42.0), egui::Sense::hover());
                            match images.get(&game.capsule) {
                                Some(texture) => {
                                    egui::Image::new((texture.id(), rect.size()))
                                        .rounding(egui::Rounding::same(4.0))
                                        .paint_at(ui, rect);
                                }
                                None => {
                                    ui.painter().rect_filled(
                                        rect,
                                        egui::Rounding::same(4.0),
                                        CARD_HOVER(),
                                    );
                                }
                            }
                            ui.add_space(10.0);
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                ui.label(
                                    egui::RichText::new(&game.name)
                                        .size(12.5)
                                        .strong()
                                        .color(TEXT()),
                                );
                                ui.horizontal(|ui| {
                                    if !game.hours.is_empty() {
                                        ui.label(
                                            egui::RichText::new(&game.hours)
                                                .size(11.5)
                                                .color(TEXT_DIM()),
                                        );
                                    }
                                    if !game.achievements.is_empty() {
                                        muted(ui, format!("· {}", game.achievements));
                                    }
                                });
                                if !game.last_used.is_empty() {
                                    muted(ui, game.last_used.clone());
                                } else {
                                    // La ligne garde sa place : les rangées
                                    // s'alignent d'un jeu à l'autre.
                                    ui.add_space(14.0);
                                }
                            });
                        });
                        ui.add_space(8.0);
                    }
                    if !showcase.parts.is_empty() {
                        self.description(ui, &showcase.parts, actions);
                    }
                    if showcase.images.is_empty() {
                        continue;
                    }
                    ui.add_space(6.0);
                    // Une capture seule (vitrine de créations) se montre en
                    // grand ; les visuels d'une vitrine d'activité restent des
                    // vignettes.
                    let single = showcase.images.len() == 1;
                    ui.horizontal_wrapped(|ui| {
                        for url in &showcase.images {
                            let size = if single {
                                let width = ui.available_width().min(360.0);
                                egui::vec2(width, width / 2.1)
                            } else {
                                egui::vec2(112.0, 63.0)
                            };
                            match images.get(url) {
                                Some(texture) => {
                                    ui.add(
                                        egui::Image::new((texture.id(), size))
                                            .rounding(egui::Rounding::same(6.0)),
                                    );
                                }
                                None => {
                                    ui.allocate_space(size);
                                }
                            }
                        }
                    });
                }
            });
    }

    /// Commentaires du profil public, lus sans compte.
    fn comments_card(
        &mut self,
        ui: &mut egui::Ui,
        steam_id: Option<u64>,
        actions: &mut Vec<ProfileAction>,
    ) {
        let Some(steam_id) = steam_id else {
            return;
        };
        let Some((total, comments)) = self.comments.clone() else {
            let loading = self.comments_error.is_none() && !self.rate_limited;
            let message = if let Some(error) = self.comments_error.clone() {
                error
            } else if self.rate_limited {
                lapinux_core::i18n::t("profile.comments_rate_limited").to_string()
            } else {
                lapinux_core::i18n::t("profile.comments_loading").to_string()
            };
            egui::Frame::none()
                .fill(CARD())
                .stroke(egui::Stroke::new(1.0_f32, BORDER()))
                .rounding(egui::Rounding::same(12.0))
                .inner_margin(16.0)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(
                        egui::RichText::new(lapinux_core::i18n::t("ui.profile_comments"))
                            .size(13.0)
                            .strong()
                            .color(TEXT()),
                    );
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if loading {
                            crate::section_icons::loading(ui, 18.0);
                        }
                        muted(ui, message);
                    });
                });
            return;
        };
        egui::Frame::none()
            .fill(CARD())
            .stroke(egui::Stroke::new(1.0_f32, BORDER()))
            .rounding(egui::Rounding::same(12.0))
            .inner_margin(16.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(lapinux_core::i18n::t("ui.profile_comments"))
                            .size(13.0)
                            .strong()
                            .color(TEXT()),
                    );
                    muted(ui, format!("· {total} au total"));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if link_text(ui, lapinux_core::i18n::t("profile.open_profile")).clicked() {
                            actions.push(ProfileAction::Open(format!(
                                "https://steamcommunity.com/profiles/{steam_id}"
                            )));
                        }
                    });
                });
                if comments.is_empty() {
                    ui.add_space(6.0);
                    muted(ui, lapinux_core::i18n::t("profile.no_comments"));
                    return;
                }
                ui.add_space(8.0);
                let images = self.showcase_images.clone();
                for comment in &comments {
                    ui.horizontal_top(|ui| {
                        self.comment_avatar_cell(ui, comment, 26.0);
                        ui.add_space(8.0);
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(comment.author.clone())
                                        .size(12.0)
                                        .strong()
                                        .color(TEXT()),
                                );
                                muted(ui, format!("· {}", comment.posted));
                            });
                            ui.add_space(2.0);
                            // Le texte garde ses liens et ses émoticônes, comme
                            // la description du profil.
                            self.description(ui, &comment.parts, actions);
                            if comment.images.is_empty() {
                                return;
                            }
                            ui.add_space(4.0);
                            ui.horizontal_wrapped(|ui| {
                                for url in &comment.images {
                                    if let Some(texture) = images.get(url) {
                                        ui.add(
                                            egui::Image::new((
                                                texture.id(),
                                                egui::vec2(128.0, 72.0),
                                            ))
                                            .rounding(egui::Rounding::same(6.0)),
                                        );
                                    }
                                }
                            });
                        });
                    });
                    ui.add_space(8.0);
                }
            });
    }

    /// Profil privé : la page le dit, plutôt que d'afficher des cases vides.
    fn private_note(&mut self, ui: &mut egui::Ui, profile: &SteamProfile) {
        if profile.privacy_state == "public" {
            return;
        }
        egui::Frame::none()
            .fill(CARD())
            .stroke(egui::Stroke::new(1.0_f32, BORDER()))
            .rounding(egui::Rounding::same(12.0))
            .inner_margin(14.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    status_dot(ui, WARN);
                    ui.label(
                        egui::RichText::new(lapinux_core::i18n::t_fmt(
                            "profile.partial_privacy",
                            &[privacy_label(&profile.privacy_state).to_lowercase()],
                        ))
                        .size(12.0)
                        .color(TEXT()),
                    );
                });
            });
    }
}

/// Découpe un texte en morceaux : les mots, et chaque emoji Unicode à part.
///
/// La police de l'application ne dessine que des glyphes monochromes : chaque
/// emoji part donc séparément, pour être remplacé par une petite image en
/// couleur dès qu'elle est là.
fn split_emoji(text: &str) -> Vec<(bool, &str)> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut out: Vec<(bool, &str)> = Vec::new();
    let mut start = 0;
    let mut index = 0;
    while index < chars.len() {
        let (offset, character) = chars[index];
        if !is_emoji(character) {
            index += 1;
            continue;
        }
        let span = emoji_span(&chars, index);
        if offset > start {
            out.push((false, &text[start..offset]));
        }
        let end = chars
            .get(index + span)
            .map(|(offset, _)| *offset)
            .unwrap_or(text.len());
        out.push((true, &text[offset..end]));
        start = end;
        index += span;
    }
    if start < text.len() {
        out.push((false, &text[start..]));
    }
    out
}

/// Longueur d'un emoji, modificateurs compris : la peau, la variation de
/// présentation et les liaisons qui font un seul signe.
fn emoji_span(chars: &[(usize, char)], start: usize) -> usize {
    let base = chars[start].1;
    let mut span = 1;
    while let Some(&(_, next)) = chars.get(start + span) {
        if next == '\u{200d}' {
            span += 1;
            // Le signe lié par le trait d'union invisible fait partie du même
            // emoji.
            if start + span < chars.len() {
                span += 1;
            }
            continue;
        }
        if next == '\u{fe0f}' || ('\u{1f3fb}'..='\u{1f3ff}').contains(&next) {
            span += 1;
            continue;
        }
        if span == 1 && is_regional(base) && is_regional(next) {
            span += 1;
            continue;
        }
        break;
    }
    span
}

/// Un signe que les polices dessinent en emoji.
fn is_emoji(character: char) -> bool {
    matches!(
        character as u32,
        0x1f000..=0x1faff | 0x2600..=0x27bf | 0x2b00..=0x2bff | 0x2300..=0x23ff | 0x25a0..=0x25ff
    )
}

/// Une lettre d'indicateur régional : deux forment un drapeau.
fn is_regional(character: char) -> bool {
    matches!(character as u32, 0x1f1e6..=0x1f1ff)
}

/// Adresses possibles de l'image d'un emoji, dans le jeu Twemoji.
///
/// Le nom du fichier est la suite de ses points de code, et le jeu ne range
/// pas toujours les séquences de la même façon : certaines gardent le
/// sélecteur de présentation (`fe0f`), d'autres non. On essaie donc les deux.
fn emoji_urls(emoji: &str) -> Vec<String> {
    let codes: Vec<u32> = emoji.chars().map(|character| character as u32).collect();
    let full = codes
        .iter()
        .map(|code| format!("{code:x}"))
        .collect::<Vec<_>>()
        .join("-");
    let bare = codes
        .iter()
        .filter(|code| **code != 0xfe0f)
        .map(|code| format!("{code:x}"))
        .collect::<Vec<_>>()
        .join("-");
    let mut names = vec![full];
    if bare != names[0] {
        names.push(bare);
    }
    names
        .into_iter()
        .map(|name| {
            format!("https://cdn.jsdelivr.net/gh/jdecked/twemoji@15.1.0/assets/72x72/{name}.png")
        })
        .collect()
}

/// Identifiant de compte, masqué : de quoi reconnaître le sien sans l'étaler.
fn masked_steam_id(steam_id: &str) -> String {
    if steam_id.chars().count() <= 5 {
        return format!("ID {steam_id}");
    }
    let head: String = steam_id.chars().take(3).collect();
    let tail: String = steam_id
        .chars()
        .skip(steam_id.chars().count().saturating_sub(2))
        .collect();
    format!("ID {head}••••{tail}")
}

/// Bouton de rafraîchissement : une flèche qui fait le tour, dessinée plutôt
/// qu'écrite — le libellé prenait la place de l'identifiant.
fn refresh_button(ui: &mut egui::Ui, tooltip: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::click());
    let ink = if response.hovered() {
        TEXT()
    } else {
        TEXT_DIM()
    };
    let center = rect.center() + egui::vec2(0.0, -0.4);
    let radius = 5.0;
    let start = -1.2_f32;
    let sweep = std::f32::consts::TAU * 0.78;
    let points: Vec<egui::Pos2> = (0..=22)
        .map(|step| {
            let angle = start + sweep * (step as f32 / 22.0);
            center + egui::vec2(angle.cos(), angle.sin()) * radius
        })
        .collect();
    let stroke = egui::Stroke::new(1.5_f32, ink);
    ui.painter().add(egui::Shape::line(points.clone(), stroke));
    // La pointe ferme la boucle, dans l'axe du dernier segment.
    let tip = points[points.len() - 1];
    let previous = points[points.len() - 2];
    let direction = (tip - previous).normalized();
    let normal = egui::vec2(-direction.y, direction.x);
    let base = tip - direction * 4.0;
    ui.painter()
        .line_segment([tip, base + normal * 2.6], stroke);
    ui.painter()
        .line_segment([tip, base - normal * 2.6], stroke);
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(tooltip)
}

/// Couleur d'une présence : verte en ligne, bleue en jeu, grise hors ligne.
fn presence_color(presence: SteamPresence) -> egui::Color32 {
    match presence {
        SteamPresence::Online | SteamPresence::Play => OK,
        SteamPresence::Away | SteamPresence::Snooze => WARN,
        SteamPresence::Busy => DANGER,
        SteamPresence::Invisible | SteamPresence::Offline | SteamPresence::Trade => TEXT_DIM(),
    }
}

/// « vu il y a … », ou « hors ligne » quand Steam n'a rien daté.
fn last_seen_line(friend: &SteamFriend) -> String {
    let now = lapinux_core::util::now_unix();
    // La présence passe avant les dates : un ami connecté garde une date de
    // déconnexion, et la lire affichait « déconnecté il y a 8 h » sous un point
    // vert.
    if friend.presence.online() {
        return friend.presence.label().to_string();
    }
    if friend.last_logoff > 0 {
        let elapsed = (now - friend.last_logoff).max(0);
        if elapsed < 60 {
            return lapinux_core::i18n::t("profile.offline_now").into();
        }
        return lapinux_core::i18n::t_fmt("profile.offline_ago", &[short_ago(elapsed).to_string()]);
    }
    if friend.last_logon > 0 {
        return lapinux_core::i18n::t_fmt(
            "profile.connected_ago",
            &[short_ago((now - friend.last_logon).max(0)).to_string()],
        );
    }
    friend.presence.label().to_string()
}

fn short_ago(seconds: i64) -> String {
    if seconds < 3600 {
        format!("{} min", (seconds / 60).max(1))
    } else if seconds < 86_400 {
        lapinux_core::i18n::t_args("profile.hours", &[("hours", &(seconds / 3600).to_string())])
    } else {
        lapinux_core::i18n::t_args("profile.days", &[("days", &(seconds / 86_400).to_string())])
    }
}

/// Pastille d'état : verte en ligne, bleue en jeu, grise hors ligne.
fn online_label(profile: &SteamProfile) -> (egui::Color32, String) {
    match profile.online_state.as_str() {
        "online" => (OK, lapinux_core::i18n::t("profile.online").to_string()),
        "in-game" => (
            BLUETOOTH_BLUE,
            lapinux_core::i18n::t("profile.in_game").to_string(),
        ),
        "offline" => (
            TEXT_DIM(),
            lapinux_core::i18n::t("profile.offline").to_string(),
        ),
        _ => {
            if profile.state_message.is_empty() {
                (
                    TEXT_DIM(),
                    lapinux_core::i18n::t("profile.unknown_state").to_string(),
                )
            } else {
                (TEXT_DIM(), profile.state_message.clone())
            }
        }
    }
}

fn privacy_label(state: &str) -> String {
    match state {
        "public" => lapinux_core::i18n::t("profile.public_state").to_string(),
        "friendsonly" => lapinux_core::i18n::t("profile.friends_only").to_string(),
        "private" => lapinux_core::i18n::t("profile.private").to_string(),
        other => match other.trim() {
            "" => lapinux_core::i18n::t("profile.unknown_visibility").to_string(),
            value => value.to_string(),
        },
    }
}

/// Un total absent ou nul : le profil XML de Steam rend souvent `0.0`.
fn hours_missing(value: &str) -> bool {
    let value = value.trim();
    value.is_empty()
        || value
            .replace(',', ".")
            .parse::<f64>()
            .map(|hours| hours <= 0.0)
            .unwrap_or(false)
}

/// Total des quinze derniers jours publié par la page du profil :
/// « 1,6 h les 15 derniers jours » devient `1,6`.
fn recent_two_week_hours(html: &str) -> Option<String> {
    let text = tagged_text(
        html,
        "class=\"recentgame_quicklinks recentgame_recentplaytime\"",
    )?;
    let text = text.trim();
    let start = text.find(|c: char| c.is_ascii_digit())?;
    let rest = &text[start..];
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == ',' || c == '.'))
        .unwrap_or(rest.len());
    let number = rest[..end].trim_end_matches([',', '.']);
    (!number.is_empty()).then(|| number.to_string())
}

/// « 12.4 » → « 12,4 h » ; un champ vide reste un tiret.
fn format_hours(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return "—".to_string();
    }
    lapinux_core::i18n::t_args(
        "profile.hours",
        &[("hours", &lapinux_core::i18n::decimal(trimmed))],
    )
}

/// Fragment d'une ligne de description : un mot, un lien, ou une émoticône.
enum DescriptionPiece<'a> {
    Word(&'a str),
    Link { text: &'a str, url: &'a str },
    Emote { name: &'a str, url: &'a str },
}

/// Découpe la description en lignes de fragments : les sauts de ligne du profil
/// commencent une nouvelle ligne, les mots se replient d'eux-mêmes ensuite.
fn description_lines(parts: &[DescriptionPart]) -> Vec<Vec<DescriptionPiece<'_>>> {
    let mut lines: Vec<Vec<DescriptionPiece<'_>>> = vec![Vec::new()];
    for part in parts {
        match part {
            DescriptionPart::Text(text) => {
                for (index, segment) in text.split('\n').enumerate() {
                    if index > 0 {
                        lines.push(Vec::new());
                    }
                    // Les mots gardent leur espace : la ponctuation colle à
                    // l'émoticône qui précède, comme dans Steam.
                    for word in segment.split_inclusive(' ').filter(|word| !word.is_empty()) {
                        if let Some(line) = lines.last_mut() {
                            line.push(DescriptionPiece::Word(word));
                        }
                    }
                }
            }
            DescriptionPart::Link { text, url } => {
                if let Some(line) = lines.last_mut() {
                    line.push(DescriptionPiece::Link { text, url });
                }
            }
            DescriptionPart::Emote { name, url } => {
                if let Some(line) = lines.last_mut() {
                    line.push(DescriptionPiece::Emote { name, url });
                }
            }
        }
    }
    // Une ligne sans rien à lire (des espaces laissés par le HTML) ne prend
    // pas la place d'une ligne.
    lines.retain(|line| {
        line.iter().any(|piece| match piece {
            DescriptionPiece::Word(word) => !word.trim().is_empty(),
            _ => true,
        })
    });
    lines
}

/// Nom affiché d'un membre de la famille : celui que la session connaît, sinon
/// celui lu sur sa page publique. Jamais d'identifiant à l'écran.
fn family_member_name(
    member: &lapinux_core::social::SteamFamilyMember,
    public_names: &HashMap<u64, String>,
) -> String {
    if !member.name.trim().is_empty() {
        return member.name.clone();
    }
    public_names
        .get(&member.steam_id)
        .cloned()
        .unwrap_or_default()
}

/// Jeux partagés par un membre : l'appid garde la vignette, le catalogue
/// donne le nom des jeux connus. Un appid absent garde un libellé stable
/// plutôt que de disparaître de la liste.
fn family_games(
    appids: &[u32],
    catalog: Option<&lapinux_core::model::SteamCatalog>,
) -> Vec<FamilyGame> {
    let names: HashMap<u32, &str> = catalog
        .map(|catalog| {
            catalog
                .entries
                .iter()
                .filter(|entry| !entry.name.trim().is_empty())
                .map(|entry| (entry.appid, entry.name.as_str()))
                .collect()
        })
        .unwrap_or_default();
    let mut games: Vec<FamilyGame> = appids
        .iter()
        .map(|appid| FamilyGame {
            appid: *appid,
            name: names
                .get(appid)
                .map(|name| (*name).to_string())
                .unwrap_or_else(|| format!("App {appid}")),
        })
        .collect();
    games.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.appid.cmp(&right.appid))
    });
    games
}

/// Un œil : ouvert, il montre ce qu'il couvre ; barré, il le cache.
fn paint_eye(painter: &egui::Painter, center: egui::Pos2, open: bool, color: egui::Color32) {
    const W: f32 = 14.0;
    const H: f32 = 8.0;
    let mut points = Vec::with_capacity(25);
    for step in 0..=24 {
        let angle = step as f32 / 24.0 * std::f32::consts::TAU;
        points.push(center + egui::vec2(W / 2.0 * angle.cos(), H / 2.0 * angle.sin()));
    }
    painter.add(egui::Shape::closed_line(
        points,
        egui::Stroke::new(1.1_f32, color),
    ));
    if open {
        painter.circle_filled(center, 1.9, color);
    } else {
        painter.line_segment(
            [
                center + egui::vec2(-W / 2.0, H / 2.0),
                center + egui::vec2(W / 2.0, -H / 2.0),
            ],
            egui::Stroke::new(1.1_f32, color),
        );
    }
}

/// Logo d'une plateforme dans sa tuile : la marque embarquée, teintée comme
/// les pictogrammes voisins. Les plateformes sans logo prennent leurs
/// initiales, comme les jeux sans jaquette.
fn paint_platform_logo(painter: &egui::Painter, platform: Platform, tile: egui::Rect) {
    let Some(store) = platform.store() else {
        // Pas de marque de boutique : les deux pictogrammes de la session sont
        // dessinés au trait, comme ceux de la navigation.
        match platform {
            Platform::Discord => paint_discord_glyph(painter, tile.center(), 30.0, TEXT()),
            _ => paint_activity_glyph(painter, tile.center(), 30.0, TEXT()),
        }
        return;
    };
    // Une marque plus grande que la tuile d'origine : les logos à lettrage
    // (« EPIC GAMES », « gog.com ») n'étaient qu'une bouillie à 24 px.
    let side = if platform == Platform::Steam {
        32.0
    } else {
        30.0
    };
    let px = crate::mark_pixels(painter.ctx(), side);
    let texture = crate::store_mark_texture(painter.ctx(), store, px);
    // Le logo garde ses proportions : un glyphe large ne s'écrase pas dans la
    // tuile.
    let source = texture.size_vec2();
    let ratio = if source.y > 0.0 {
        source.x / source.y
    } else {
        1.0
    };
    let (width, height) = if ratio >= 1.0 {
        (side, side / ratio)
    } else {
        (side * ratio, side)
    };
    painter.image(
        texture.id(),
        egui::Rect::from_center_size(tile.center(), egui::vec2(width, height)),
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        TEXT(),
    );
}

/// Pictogramme Discord : le visage de la mascotte, au trait, deux yeux et
/// deux pattes — le sous-ensemble Remix n'a pas son logo.
fn paint_discord_glyph(painter: &egui::Painter, center: egui::Pos2, size: f32, ink: egui::Color32) {
    let scale = size / 30.0;
    let stroke = egui::Stroke::new(1.5_f32 * scale, ink);
    let body = egui::Rect::from_center_size(
        center + egui::vec2(0.0, -1.0) * scale,
        egui::vec2(22.0, 15.0) * scale,
    );
    painter.rect_stroke(body, egui::Rounding::same(5.0 * scale), stroke);
    for side in [-1.0_f32, 1.0] {
        painter.circle_filled(
            center + egui::vec2(side * 4.6, -1.0) * scale,
            1.9 * scale,
            ink,
        );
        painter.add(egui::Shape::line(
            vec![
                center + egui::vec2(side * 7.4, 6.4) * scale,
                center + egui::vec2(side * 9.2, 9.6) * scale,
            ],
            stroke,
        ));
    }
}

/// Pictogramme Activité : une ligne de pouls, lisible à la taille de la tuile.
fn paint_activity_glyph(
    painter: &egui::Painter,
    center: egui::Pos2,
    size: f32,
    ink: egui::Color32,
) {
    let scale = size / 30.0;
    let stroke = egui::Stroke::new(1.6_f32 * scale, ink);
    let points = [
        (-11.0, 0.0),
        (-5.5, 0.0),
        (-3.0, -6.5),
        (0.6, 6.0),
        (3.4, -2.5),
        (6.0, 0.0),
        (11.0, 0.0),
    ];
    painter.add(egui::Shape::line(
        points
            .iter()
            .map(|(x, y)| center + egui::vec2(*x, *y) * scale)
            .collect(),
        stroke,
    ));
}

fn status_dot(ui: &mut egui::Ui, color: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(9.0, 9.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 3.5, color);
}

fn fact(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.vertical(|ui| {
        // Un fait tient sur une ligne : dans une grille, le texte ne doit pas
        // se replier mot à mot quand la colonne est encore étroite.
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
        ui.spacing_mut().item_spacing.y = 1.0;
        muted(ui, label);
        let text = if value.trim().is_empty() {
            "—".to_string()
        } else {
            value.trim().to_string()
        };
        ui.label(egui::RichText::new(text).size(12.0).color(TEXT()));
    });
}

/// Icône « copier » : deux feuilles décalées, comme dans les cartes de
/// téléchargement.
fn paint_copy_glyph(painter: &egui::Painter, rect: egui::Rect, ink: egui::Color32) {
    let center = rect.center();
    let back = egui::Rect::from_min_max(
        egui::pos2(center.x - 3.2, center.y - 6.0),
        egui::pos2(center.x + 5.8, center.y + 2.5),
    );
    let front = egui::Rect::from_min_max(
        egui::pos2(center.x - 5.8, center.y - 2.5),
        egui::pos2(center.x + 3.2, center.y + 6.0),
    );
    painter.rect_stroke(back, 1.4_f32, egui::Stroke::new(1.2_f32, ink));
    painter.rect_stroke(front, 1.4_f32, egui::Stroke::new(1.2_f32, ink));
}

/// Requête du fil réseau : profil XML, avatar, pochettes, puis commentaires.
fn fetch(steam_id: u64, tx: &mpsc::Sender<ProfileMsg>, fresh: bool) {
    let url = format!("https://steamcommunity.com/profiles/{steam_id}/?xml=1");
    // Le profil XML suit la même règle que la page : le cache d'abord, Steam
    // ensuite, et le cache périmé en secours quand Steam refuse.
    let cached = if fresh {
        None
    } else {
        cached_text(steam_id, "xml")
    };
    let xml = match cached {
        Some(xml) => xml,
        None => match http_text(&url) {
            Ok(xml) => {
                remember_text(steam_id, "xml", &xml);
                xml
            }
            Err(error) => match stale_text(steam_id, "xml") {
                Some(xml) => xml,
                None => {
                    let _ = tx.send(ProfileMsg::Failed(error));
                    return;
                }
            },
        },
    };
    let profile = profile_from_xml(&xml);
    if profile.steam_id.is_empty() {
        let _ = tx.send(ProfileMsg::Failed(FetchError::new(lapinux_core::i18n::t(
            "profile.not_found",
        ))));
        return;
    }
    let _ = tx.send(ProfileMsg::Loaded(profile.clone()));
    // L'avatar est l'URL donnée par le profil (pleine résolution si Steam la
    // fournit, sinon la moyenne) : le CDN ne compte pas dans le quota.
    if let Some(bytes) = http_bytes(&profile.avatar_url) {
        let _ = tx.send(ProfileMsg::Avatar(bytes));
    }
    // Une seule lecture de la page publique porte les vitrines, leurs visuels
    // et les commentaires. C'est la page la plus bridée de Steam : la demander
    // une fois vaut mieux que trois lectures séparées.
    let mut page = fetch_profile_page(steam_id, fresh);
    // Une limite de débit se rattrape : Steam ferme l'accès par adresse, pour
    // une fenêtre qui dure parfois une minute. On patiente de plus en plus
    // longtemps, puis la carte se remplit sans que personne n'ait à cliquer.
    for backoff in [COMMUNITY_PENALTY, SHOWCASE_RETRY] {
        if !matches!(&page, Err(error) if error.rate_limited) {
            break;
        }
        hold_community_requests(backoff);
        page = fetch_profile_page(steam_id, true);
    }
    match page {
        Ok(html) => {
            // Le profil XML de Steam renvoie `hoursPlayed2Wk` à 0.0 alors que
            // l'en-tête « Activité récente » de la page donne le vrai total
            // des quinze derniers jours. On le reprend sans bloquer la fiche.
            if let Some(hours) = recent_two_week_hours(&html) {
                let _ = tx.send(ProfileMsg::RecentHours(hours));
            }
            let _ = tx.send(ProfileMsg::Showcases(parse_showcases(&html)));
            let (total, comments) = comments_from_page(&html);
            let _ = tx.send(ProfileMsg::Comments {
                total,
                comments,
                error: None,
            });
        }
        Err(error) => {
            let error = error.message;
            let _ = tx.send(ProfileMsg::Comments {
                total: 0,
                comments: Vec::new(),
                error: Some(error),
            });
        }
    }
}

/// Nom et photo d'un profil public, lus sans session : la page XML les donne
/// même quand le reste du profil est privé.
fn fetch_public_identity(steam_id: u64) -> Option<SteamProfile> {
    let url = format!("https://steamcommunity.com/profiles/{steam_id}/?xml=1");
    let xml = http_text(&url).ok()?;
    let profile = profile_from_xml(&xml);
    (!profile.steam_id.is_empty()).then_some(profile)
}

/// Page publique du profil : vitrines, visuels et commentaires y vivent, et
/// nulle part ailleurs. Une seule requête sert les trois.
fn fetch_profile_page(steam_id: u64, fresh: bool) -> Result<String, FetchError> {
    // Une page encore fraîche évite la requête : Steam ferme la porte aux
    // lectures répétées, et la fiche s'affiche alors tout de suite.
    if !fresh {
        if let Some(html) = cached_text(steam_id, "html") {
            return Ok(html);
        }
    }
    let url = format!("https://steamcommunity.com/profiles/{steam_id}/?l=french");
    // La page du profil n'est pas une lecture d'API : présentée sous l'agent de
    // Lapinux, Steam répond 429 presque à chaque fois. Sous un agent de
    // navigateur, elle arrive normalement.
    match http_text_as(&url, BROWSER_AGENT) {
        Ok(html) => {
            remember_text(steam_id, "html", &html);
            Ok(html)
        }
        // Même refusée, la page déjà lue vaut mieux qu'une fiche vide.
        Err(error) => match stale_text(steam_id, "html") {
            Some(html) => Ok(html),
            None => Err(error),
        },
    }
}

/// Fichier de cache d'une lecture communautaire.
fn cache_file(steam_id: u64, extension: &str) -> Option<std::path::PathBuf> {
    let dir = lapinux_core::paths::LapinuxPaths::discover()
        .cache_dir
        .join("profils");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join(format!("{steam_id}.{extension}")))
}

/// Lecture du cache, si elle date de moins de `max_age`.
fn read_cache(path: &std::path::Path, max_age: Option<Duration>) -> Option<String> {
    if let Some(max_age) = max_age {
        let modified = std::fs::metadata(path).ok()?.modified().ok()?;
        if modified.elapsed().unwrap_or_default() > max_age {
            return None;
        }
    }
    std::fs::read_to_string(path).ok()
}

/// Texte encore frais : il évite une requête à Steam.
fn cached_text(steam_id: u64, extension: &str) -> Option<String> {
    read_cache(&cache_file(steam_id, extension)?, Some(PROFILE_CACHE_TTL))
}

/// Texte périmé : il sert de secours quand Steam refuse la lecture.
fn stale_text(steam_id: u64, extension: &str) -> Option<String> {
    read_cache(&cache_file(steam_id, extension)?, None)
}

fn remember_text(steam_id: u64, extension: &str, text: &str) {
    if let Some(path) = cache_file(steam_id, extension) {
        let _ = std::fs::write(path, text);
    }
}

/// Capsules des jeux joués, par appid, téléchargées en une fois.
///
/// L'ancien chemin `steam/apps/<id>/capsule_231x87.jpg` ne sert plus les jeux
/// récents : leur visuel vit sous `store_item_assets`, avec une empreinte que
/// seule la fiche de boutique connaît. Une requête pour tous les amis, puis le
/// CDN ; les jeux anciens gardent l'ancien chemin, qui répond encore.
fn fetch_game_capsules(appids: &[u32]) -> Vec<(u32, Vec<u8>)> {
    let mut out: Vec<(u32, Vec<u8>)> = Vec::new();
    if appids.is_empty() {
        return out;
    }
    let ids: Vec<serde_json::Value> = appids
        .iter()
        .map(|appid| serde_json::json!({ "appid": appid }))
        .collect();
    let input = serde_json::json!({
        "ids": ids,
        "context": { "language": "french", "country_code": "FR" },
        "data_request": { "include_assets": true, "include_basic_info": false },
    });
    let query = percent_encode(&input.to_string());
    let url = format!(
        "https://api.steampowered.com/IStoreBrowseService/GetItems/v1/\
         ?origin=https%3A%2F%2Fstore.steampowered.com&input_json={query}"
    );
    let mut resolved: Vec<u32> = Vec::new();
    if let Ok(text) = http_text(&url) {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(items) = json
                .pointer("/response/store_items")
                .and_then(|items| items.as_array())
            {
                for item in items {
                    let Some(appid) = item.get("appid").and_then(|id| id.as_u64()) else {
                        continue;
                    };
                    let Some(assets) = item.get("assets") else {
                        continue;
                    };
                    let format = assets
                        .get("asset_url_format")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default();
                    // La capsule de liste d'abord, la bannière en secours.
                    let file = assets
                        .get("small_capsule")
                        .and_then(|value| value.as_str())
                        .or_else(|| assets.get("header").and_then(|value| value.as_str()));
                    let Some(file) = file else {
                        continue;
                    };
                    if format.is_empty() {
                        continue;
                    }
                    let url = format!(
                        "https://shared.akamai.steamstatic.com/store_item_assets/{}",
                        format.replace("${FILENAME}", file)
                    );
                    if let Some(bytes) = http_bytes(&url) {
                        out.push((appid as u32, bytes));
                        resolved.push(appid as u32);
                    }
                }
            }
        }
    }
    for appid in appids.iter().filter(|appid| !resolved.contains(appid)) {
        for asset in ["capsule_231x87.jpg", "header.jpg"] {
            let url = format!("https://cdn.akamai.steamstatic.com/steam/apps/{appid}/{asset}");
            if let Some(bytes) = http_bytes(&url) {
                out.push((*appid, bytes));
                break;
            }
        }
    }
    out
}

/// Encodage d'un paramètre de requête : le JSON de la fiche de boutique part
/// tel quel dans l'adresse.
fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 2);
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Coupe une fin de balise restée ouverte : la découpe d'un bloc s'arrête
/// parfois au milieu d'un attribut, et le fragment serait lu comme du texte.
fn trim_partial_tag(html: &str) -> &str {
    match (html.rfind('<'), html.rfind('>')) {
        (Some(open), Some(close)) if open > close => &html[..open],
        _ => html,
    }
}

/// Découpe la page en vitrines : un titre, les lignes de texte et les visuels
/// de chacune. Steam change parfois la forme d'une vitrine (jeux récents,
/// texte libre, objets) ; on garde ce qui est lisible partout.
/// Début d'une vitrine : `class="profile_customization` suivi d'une autre
/// classe, du guillemet ou de la fin de la balise. Sans cette vérification,
/// `profile_customization_header` passerait pour une nouvelle vitrine.
fn next_customization(text: &str) -> Option<usize> {
    const MARK: &str = "class=\"profile_customization";
    let mut from = 0;
    while let Some(index) = text[from..].find(MARK) {
        let at = from + index;
        let after = text[at + MARK.len()..].chars().next();
        if matches!(after, None | Some('"') | Some(' ') | Some('>')) {
            return Some(at);
        }
        from = at + MARK.len();
    }
    None
}

fn parse_showcases(html: &str) -> Vec<SteamShowcase> {
    const MARK: &str = "class=\"profile_customization";
    let mut showcases = Vec::new();
    let mut rest = html;
    while let Some(index) = next_customization(rest) {
        let block = &rest[index..];
        // Le bloc suivant ferme celui-ci.
        let end = next_customization(&block[MARK.len()..])
            .map(|offset| offset + MARK.len())
            .unwrap_or(block.len());
        rest = &block[end..];
        // Le marqueur suivant tombe dans ses propres attributs : le bloc
        // finissait sur une balise ouverte, lue comme du texte.
        let block = trim_partial_tag(&block[..end]);
        let Some(header) = block.find("profile_customization_header") else {
            continue;
        };
        let after_header = &block[header..];
        let Some(open) = after_header.find('>') else {
            continue;
        };
        let Some(close) = after_header.find("</div>") else {
            continue;
        };
        if close <= open {
            continue;
        }
        let title = clean_html(&after_header[open + 1..close])
            .trim()
            .to_string();
        if title.is_empty() {
            continue;
        }
        // `close` désigne la balise fermante du titre : le contenu commence
        // juste après elle, sinon il s'ouvre sur un `</div>` orphelin.
        let content = &block[header + close + 6..];
        // Les jeux d'une vitrine d'activité : capsule, heures et succès.
        let games = parse_showcase_games(content);
        let lines: Vec<String> = games.iter().map(|game| game.name.clone()).collect();
        // Vitrine de texte libre : ses mots, ses liens et ses émoticônes, lus
        // comme la description du profil.
        let parts = if games.is_empty() {
            parse_description(content)
        } else {
            Vec::new()
        };
        let mut images: Vec<String> = Vec::new();
        let mut scan = content;
        while let Some(index) = scan.find("src=\"https://") {
            let tail = &scan[index + 5..];
            let Some(end) = tail.find('"') else {
                break;
            };
            let url = &tail[..end];
            scan = &tail[end..];
            // Les émoticônes ponctuent le texte : ce ne sont pas des visuels.
            if url.contains("/economy/emoticon/") || url.contains("/avatar") {
                continue;
            }
            let url = url.to_string();
            if !images.contains(&url) {
                images.push(url);
            }
            if images.len() >= 3 {
                break;
            }
        }
        // Le temps des quinze derniers jours accompagne le titre.
        let subtitle = tagged_text(block, "recentgame_recentplaytime").unwrap_or_default();
        // Une vitrine de jeux récents n'a pas de visuel à montrer : les images
        // qu'elle contient sont les badges et les succès de chaque jeu.
        if !games.is_empty() {
            images.clear();
        }
        showcases.push(SteamShowcase {
            title,
            subtitle,
            parts,
            lines,
            games,
            images,
        });
    }
    showcases
}

/// Jeux d'une vitrine d'activité : chaque bloc `recent_game` porte sa capsule,
/// ses heures et ses succès.
fn parse_showcase_games(content: &str) -> Vec<ShowcaseGame> {
    const BLOCK: &str = "class=\"recent_game\"";
    let mut games: Vec<ShowcaseGame> = Vec::new();
    let mut rest = content;
    while let Some(index) = rest.find(BLOCK) {
        let block = &rest[index..];
        let end = block[BLOCK.len()..]
            .find(BLOCK)
            .map(|offset| offset + BLOCK.len())
            .unwrap_or(block.len());
        rest = &block[end..];
        let block = &block[..end];
        let Some(name) = tagged_text(block, "class=\"game_name\"").filter(|name| !name.is_empty())
        else {
            continue;
        };
        if games.iter().any(|game| game.name == name) {
            continue;
        }
        // « 1 776 h en tout », puis « dernière utilisation le 20 oct. ».
        let details = tagged_text(block, "class=\"game_info_details\"").unwrap_or_default();
        let mut lines = details
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty());
        games.push(ShowcaseGame {
            name,
            hours: lines.next().unwrap_or_default().to_string(),
            last_used: lines.next().unwrap_or_default().to_string(),
            // « 18 succès sur 29 · 6 rares » : les lignes se suivent.
            achievements: tagged_text(block, "class=\"game_info_achievement_summary\"")
                .unwrap_or_default()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" "),
            capsule: class_src(block, "class=\"game_capsule\"").unwrap_or_default(),
        });
    }
    games
}

/// Texte d'une balise repérée par un attribut, jusqu'à son `</div>`.
fn tagged_text(block: &str, marker: &str) -> Option<String> {
    let index = block.find(marker)?;
    let tail = &block[index..];
    let open = tail.find('>')?;
    let close = tail.find("</div>")?;
    if close <= open {
        return None;
    }
    Some(clean_html(&tail[open + 1..close]).trim().to_string())
}

/// Adresse `src` de la balise qui porte cet attribut.
fn class_src(block: &str, marker: &str) -> Option<String> {
    let index = block.find(marker)?;
    let head = block[..index].rfind('<')?;
    let tail = &block[head..];
    let end = tail.find('>')?;
    attribute(&tail[..end], "src")
}

/// Tour de parole des requêtes communautaires : deux fils qui partent ensemble
/// font la queue au lieu d'arriver ensemble.
fn wait_for_community_turn() {
    let mut free_at = community_gate()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let now = Instant::now();
    if *free_at > now {
        std::thread::sleep(*free_at - now);
    }
    *free_at = Instant::now() + COMMUNITY_SPACING;
}

/// Steam vient de refuser une requête : tout le monde patiente, la tentative en
/// cours comme les suivantes.
fn hold_community_requests(penalty: Duration) {
    let mut free_at = community_gate()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *free_at = (*free_at).max(Instant::now() + penalty);
}

fn community_gate() -> &'static std::sync::Mutex<Instant> {
    static GATE: std::sync::OnceLock<std::sync::Mutex<Instant>> = std::sync::OnceLock::new();
    GATE.get_or_init(|| std::sync::Mutex::new(Instant::now()))
}

/// Une requête HTTP, sans tour de parole ni reprise : un seul appel, traduit
/// en `FetchError`.
#[cfg(test)]
fn get_text(url: &str) -> Result<String, FetchError> {
    get_text_as(url, USER_AGENT)
}

fn get_text_as(url: &str, agent: &str) -> Result<String, FetchError> {
    let response = ureq::get(url)
        .set("User-Agent", agent)
        // Un navigateur annonce ce qu'il accepte ; sans ces en-têtes, Steam
        // traite la lecture comme un robot et ferme plus vite la porte.
        .set(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .set("Accept-Language", "fr-FR,fr;q=0.9,en;q=0.8")
        .timeout(FETCH_TIMEOUT)
        .call()
        .map_err(|error| match error {
            // 429 : Steam demande d'être appelé moins vite. Ce n'est pas une
            // panne, et ça se répare tout seul.
            ureq::Error::Status(429, _) => FetchError::rate_limited(),
            other => FetchError::new(lapinux_core::i18n::t_args(
                "profile.page_unreachable",
                &[("error", &other.to_string())],
            )),
        })?;
    let mut text = String::new();
    std::io::Read::read_to_string(&mut response.into_reader(), &mut text).map_err(|error| {
        FetchError::new(lapinux_core::i18n::t_args(
            "profile.unreadable",
            &[("error", &error.to_string())],
        ))
    })?;
    Ok(text)
}

/// Requête HTTP d'une page texte.
///
/// Les pages communautaires passent par le tour de parole : Steam répond 429
/// dès qu'on les appelle en rafale, et un refus gèle ensuite toutes les
/// requêtes du même genre pendant quelques dizaines de secondes.
fn http_text(url: &str) -> Result<String, FetchError> {
    http_text_as(url, USER_AGENT)
}

/// Même lecture, sous un autre agent : certaines pages communautaires ne
/// répondent qu'à celui d'un navigateur.
fn http_text_as(url: &str, agent: &str) -> Result<String, FetchError> {
    if url.trim().is_empty() {
        return Err(FetchError::new(lapinux_core::i18n::t(
            "profile.empty_address",
        )));
    }
    let community = url.contains("steamcommunity.com");
    if community {
        wait_for_community_turn();
    }
    match get_text_as(url, agent) {
        Ok(text) => Ok(text),
        // Insister tout de suite ne sert à rien : Steam vient de dire non. La
        // pénalité met les pages suivantes à l'abri du même refus, et la fiche
        // se relit d'elle-même quelques dizaines de secondes plus tard.
        Err(error) if error.rate_limited && community => {
            hold_community_requests(COMMUNITY_PENALTY);
            Err(error)
        }
        Err(error) => Err(error),
    }
}

fn http_bytes(url: &str) -> Option<Vec<u8>> {
    if url.trim().is_empty() {
        return None;
    }
    let response = ureq::get(url)
        .set("User-Agent", USER_AGENT)
        .timeout(FETCH_TIMEOUT)
        .call()
        .ok()?;
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut response.into_reader(), &mut bytes).ok()?;
    Some(bytes)
}

/// Contenu d'une balise du profil XML : `<steamID>…</steamID>`, CDATA comprise.
fn xml_tag(xml: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let Some(start) = xml.find(&open) else {
        return String::new();
    };
    let rest = &xml[start + open.len()..];
    let Some(stop) = rest.find(&close) else {
        return String::new();
    };
    let raw = rest[..stop].trim();
    let raw = raw
        .strip_prefix("<![CDATA[")
        .and_then(|text| text.strip_suffix("]]>"))
        .unwrap_or(raw);
    unescape(raw).trim().to_string()
}

/// Lecture du profil public. Un profil privé ne renvoie que l'identifiant, la
/// visibilité et l'avatar : tout le reste reste vide, et la page le dit.
fn profile_from_xml(xml: &str) -> SteamProfile {
    let mut profile = SteamProfile {
        steam_id: xml_tag(xml, "steamID64"),
        name: xml_tag(xml, "steamID"),
        online_state: xml_tag(xml, "onlineState"),
        state_message: xml_tag(xml, "stateMessage"),
        privacy_state: xml_tag(xml, "privacyState"),
        member_since: xml_tag(xml, "memberSince"),
        location: xml_tag(xml, "location"),
        real_name: xml_tag(xml, "realname"),
        headline: xml_tag(xml, "headline"),
        description: parse_description(&xml_tag(xml, "summary")),
        hours_2wk: xml_tag(xml, "hoursPlayed2Wk"),
        custom_url: format_custom_url(&xml_tag(xml, "customURL")),
        vac_banned: xml_tag(xml, "vacBanned") == "1",
        trade_ban: xml_tag(xml, "tradeBanState"),
        avatar_url: String::new(),
    };
    profile.avatar_url = {
        let full = xml_tag(xml, "avatarFull");
        if full.is_empty() {
            let medium = xml_tag(xml, "avatarMedium");
            if medium.is_empty() {
                xml_tag(xml, "avatarIcon")
            } else {
                medium
            }
        } else {
            full
        }
    };
    profile
}

/// Adresse personnalisée : Steam la rend sans le préfixe du site.
fn format_custom_url(value: &str) -> String {
    let value = value.trim();
    if value.is_empty() || value.eq_ignore_ascii_case("none") {
        return String::new();
    }
    format!("steamcommunity.com/id/{value}")
}

/// Description publique : le HTML est réduit au texte, chaque émoticône Steam
/// (`<img class="emoticon">`) devient une image à sa place et chaque lien
/// garde son libellé et son adresse, pour être ouvert d'un clic.
fn parse_description(html: &str) -> Vec<DescriptionPart> {
    let mut parts: Vec<DescriptionPart> = Vec::new();
    let mut text = String::new();
    // Lien ouvert : son adresse et le texte lu jusqu'à `</a>`.
    let mut link: Option<(String, String)> = None;
    let mut rest = html;
    while let Some(index) = rest.find('<') {
        append_description(&mut text, &mut link, &rest[..index]);
        let tail = &rest[index..];
        let Some(end) = tail.find('>') else {
            // Une balise que le flux ne referme pas est du balisage tronqué :
            // elle ne doit pas s'afficher comme du texte (`<div`).
            rest = "";
            break;
        };
        let tag = &tail[..=end];
        let lowered = tag.to_lowercase();
        if lowered.starts_with("<br") || lowered.starts_with("</p") || lowered.starts_with("<p") {
            append_description(&mut text, &mut link, "\u{b}");
        } else if lowered.starts_with("<img") {
            let source = attribute(tag, "src").filter(|src| src.contains("/economy/emoticon/"));
            match source {
                // L'émoticône garde son image même dans un lien : Steam
                // enveloppe parfois `:nom:` et le nom du jeu dans la même
                // balise, et le texte alternatif y perdait le visuel.
                Some(url) => {
                    push_description_text(&mut parts, &mut text);
                    let name = attribute(tag, "alt")
                        .unwrap_or_default()
                        .trim()
                        .trim_matches(':')
                        .to_string();
                    parts.push(DescriptionPart::Emote { name, url });
                }
                // Une autre image (illustration de profil) garde son texte
                // alternatif, faute de mieux.
                None => {
                    if let Some(alt) = attribute(tag, "alt") {
                        append_description(&mut text, &mut link, &alt);
                    }
                }
            }
        } else if lowered.starts_with("<a") && !lowered.starts_with("</a") {
            // Un lien s'ouvre : le texte déjà lu reste du texte.
            if link.is_none() {
                push_description_text(&mut parts, &mut text);
                let url = attribute(tag, "href").unwrap_or_default();
                link = Some((unwrap_linkfilter(&url), String::new()));
            }
        } else if lowered.starts_with("</a") {
            // Un lien se ferme : son libellé devient un fragment cliquable.
            if let Some((url, inner)) = link.take() {
                let label = link_label(&inner);
                if !label.is_empty() && !url.trim().is_empty() {
                    parts.push(DescriptionPart::Link { text: label, url });
                } else if !label.is_empty() {
                    text.push_str(&label);
                }
            }
        }
        rest = &tail[end + 1..];
    }
    append_description(&mut text, &mut link, rest);
    // Un lien que le HTML ne referme pas garde quand même son libellé.
    if let Some((url, inner)) = link.take() {
        let label = link_label(&inner);
        if !label.is_empty() && !url.trim().is_empty() {
            push_description_text(&mut parts, &mut text);
            parts.push(DescriptionPart::Link { text: label, url });
        } else if !label.is_empty() {
            text.push_str(&label);
        }
    }
    push_description_text(&mut parts, &mut text);
    with_emote_codes(parts)
}

/// Émoticônes écrites en clair : certaines vitrines publient `:health:` au
/// lieu de l'image, et le profil ne doit jamais afficher le code.
fn with_emote_codes(parts: Vec<DescriptionPart>) -> Vec<DescriptionPart> {
    let mut out: Vec<DescriptionPart> = Vec::with_capacity(parts.len());
    for part in parts {
        let DescriptionPart::Text(text) = part else {
            out.push(part);
            continue;
        };
        let mut pending = String::new();
        let mut rest = text.as_str();
        while let Some(open) = rest.find(':') {
            pending.push_str(&rest[..open]);
            let after = &rest[open + 1..];
            let Some(close) = after.find(':') else {
                break;
            };
            let name = &after[..close];
            if !is_emote_name(name) {
                pending.push(':');
                rest = after;
                continue;
            }
            if !pending.is_empty() {
                out.push(DescriptionPart::Text(std::mem::take(&mut pending)));
            }
            out.push(DescriptionPart::Emote {
                name: name.to_string(),
                url: format!("https://community.fastly.steamstatic.com/economy/emoticon/{name}"),
            });
            rest = &after[close + 1..];
        }
        pending.push_str(rest);
        if !pending.is_empty() {
            out.push(DescriptionPart::Text(pending));
        }
    }
    out
}

/// Un nom d'émoticône Steam : court, sans espace, et toujours avec une lettre
/// (pour ne pas prendre `12:30:45` pour une image).
fn is_emote_name(name: &str) -> bool {
    (2..=32).contains(&name.len())
        && name
            .chars()
            .any(|character| character.is_ascii_alphabetic())
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
}

/// Ajoute un morceau de HTML au texte en cours : dans un lien, il complète le
/// libellé ; ailleurs, il rejoint le texte de la description.
fn append_description(text: &mut String, link: &mut Option<(String, String)>, chunk: &str) {
    match link {
        Some((_, inner)) => inner.push_str(chunk),
        None => text.push_str(chunk),
    }
}

/// Libellé d'un lien : entités rendues, espaces réduits, sur une seule ligne.
fn link_label(raw: &str) -> String {
    unescape(raw)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Steam masque les liens sortants derrière `linkfilter/?u=…` : un clic doit
/// ouvrir la vraie adresse, pas le filtre de Steam.
fn unwrap_linkfilter(url: &str) -> String {
    let url = url.trim();
    let Some(index) = url.find("linkfilter/") else {
        return url.to_string();
    };
    let rest = &url[index..];
    let Some(query) = rest.find("u=") else {
        return url.to_string();
    };
    let encoded = rest[query + 2..].split('&').next().unwrap_or_default();
    match percent_decode(encoded) {
        Some(decoded) if decoded.starts_with("http") => decoded,
        _ => url.to_string(),
    }
}

/// Décode une adresse `%xx` et `+` (forme `application/x-www-form-urlencoded`).
fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' => {
                let hex = std::str::from_utf8(bytes.get(index + 1..index + 3)?).ok()?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                index += 3;
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

/// Ajoute le texte accumulé : entités rendues, espaces doublés réduits, lignes
/// vides supprimées. Les sauts de ligne du profil restent des sauts de ligne.
fn push_description_text(parts: &mut Vec<DescriptionPart>, text: &mut String) {
    let rendered = unescape(text);
    text.clear();
    let mut cleaned = String::with_capacity(rendered.len());
    let mut last_space = false;
    for character in rendered.chars() {
        if character == LINE_BREAK {
            while cleaned.ends_with(' ') {
                cleaned.pop();
            }
            if !cleaned.is_empty() && !cleaned.ends_with('\n') {
                cleaned.push('\n');
            }
            last_space = false;
        } else if character.is_whitespace() {
            if !last_space {
                cleaned.push(' ');
                last_space = true;
            }
        } else {
            cleaned.push(character);
            last_space = false;
        }
    }
    let cleaned = cleaned.trim_matches('\n').to_string();
    if !cleaned.is_empty() {
        parts.push(DescriptionPart::Text(cleaned));
    }
}

/// Texte brut d'un fragment HTML : balises retirées, entités rendues, sauts
/// de ligne conservés.
fn clean_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    let mut tag = String::new();
    while let Some(index) = rest.find('<') {
        out.push_str(&rest[..index]);
        let tail = &rest[index..];
        let Some(end) = tail.find('>') else {
            break;
        };
        tag.clear();
        tag.push_str(&tail[..=end]);
        let lowered = tag.to_lowercase();
        if lowered.starts_with("<br") || lowered.starts_with("</p") || lowered.starts_with("<p") {
            out.push('\n');
        } else if lowered.starts_with("<img") {
            // Une émoticône devient son texte alternatif, quand il existe.
            if let Some(alt) = attribute(&tag, "alt") {
                out.push_str(&alt);
            }
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    let text = unescape(&out);
    // Les lignes vides ne doivent pas se multiplier.
    text.lines()
        .map(str::trim)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// Valeur d'un attribut dans une balise (`alt="…"`).
fn attribute(tag: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=\"");
    let start = tag.find(&needle)? + needle.len();
    let rest = &tag[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Commentaires du profil : la réponse JSON porte le total et le HTML des
/// commentaires, que l'on réduit à auteur, date et texte.
/// Commentaires de la page du profil : elle publie les derniers messages et
/// leur total, ce qui évite une requête communautaire de plus.
fn comments_from_page(html: &str) -> (u32, Vec<CommentView>) {
    let total = html
        .find("_totalcount\"")
        .and_then(|index| {
            let tail = &html[index..];
            let open = tail.find('>')? + 1;
            let close = tail.find('<')?;
            tail.get(open..close)?.trim().parse::<u32>().ok()
        })
        .unwrap_or(0);
    let start = html.find("commentthread_area").unwrap_or(0);
    (total, comments_from_html(&html[start..]))
}

/// Blocs de commentaires d'un HTML Steam : auteur, photo, date et texte.
fn comments_from_html(html: &str) -> Vec<CommentView> {
    let mut comments = Vec::new();
    let mut rest = html;
    while let Some(index) = rest.find("commentthread_comment_text") {
        let head = &rest[..index];
        let tail = &rest[index..];
        let Some(open) = tail.find('>') else {
            break;
        };
        let body = &tail[open + 1..];
        let Some(end) = body.find("</div>") else {
            break;
        };
        // Le corps garde sa mise en forme : liens, émoticônes et images se
        // lisent comme ceux de la description du profil.
        let body = &body[..end];
        rest = &tail[open + 1 + end + 6..];
        let parts = parse_description(body.trim());
        if parts.is_empty() {
            continue;
        }
        comments.push(CommentView {
            author: comment_author(head),
            author_id: comment_author_id(head),
            avatar: comment_avatar(head),
            posted: comment_date(head),
            parts,
            images: comment_images(body),
        });
    }
    comments
}

/// Photo du dernier auteur cité avant le commentaire.
///
/// Le bloc d'avatar porte un cadre animé (une image) puis la photo elle-même,
/// parfois remplacée par son animation : on garde l'adresse de la photo quand
/// elle est là, sinon le dernier visuel du bloc.
fn comment_avatar(head: &str) -> String {
    let Some(start) = head.rfind("commentthread_comment_avatar") else {
        return String::new();
    };
    let block = &head[start..];
    let end = block
        .find("author_name_group")
        .unwrap_or_else(|| block.len().min(2400));
    let block = &block[..end];
    let mut candidates: Vec<String> = Vec::new();
    for marker in ["src=\"https://", "srcset=\"https://"] {
        let mut rest = block;
        while let Some(index) = rest.find(marker) {
            let tail = &rest[index + marker.len()..];
            let Some(end) = tail.find(['"', ' ']) else {
                break;
            };
            // Le marqueur a consommé le début de l'adresse : elle reprend ici.
            let url = format!("https://{}", &tail[..end]);
            rest = &tail[end..];
            let lower = url.to_lowercase();
            if lower.ends_with(".jpg") || lower.ends_with(".png") {
                candidates.push(url);
            }
        }
    }
    // La photo de profil passe devant le cadre ou l'animation.
    if let Some(photo) = candidates.iter().find(|url| url.contains("/avatars.")) {
        return photo.clone();
    }
    candidates.pop().unwrap_or_default()
}

/// Visuels d'un commentaire : les images publiées avec lui, sans les
/// émoticônes ni les avatars qui l'accompagnent.
fn comment_images(html: &str) -> Vec<String> {
    let mut images: Vec<String> = Vec::new();
    let mut rest = html;
    while let Some(index) = rest.find("src=\"https://") {
        let tail = &rest[index + 5..];
        let Some(end) = tail.find('"') else {
            break;
        };
        let url = &tail[..end];
        rest = &tail[end..];
        if url.contains("/economy/emoticon/")
            || url.contains("avatars.")
            || url.contains("/public/images/")
        {
            continue;
        }
        if !images.iter().any(|known| known == url) {
            images.push(url.to_string());
        }
        if images.len() >= 2 {
            break;
        }
    }
    images
}

/// Dernier pseudonyme affiché avant le commentaire.
fn comment_author(head: &str) -> String {
    let Some(index) = head.rfind("<bdi>") else {
        return lapinux_core::i18n::t("profile_page.anonymous").into();
    };
    let rest = &head[index + 5..];
    let Some(end) = rest.find("</bdi>") else {
        return lapinux_core::i18n::t("profile_page.anonymous").into();
    };
    unescape(rest[..end].trim())
}

/// Dernier identifiant de profil cité avant le commentaire.
fn comment_author_id(head: &str) -> u64 {
    let mut last = 0;
    let mut rest = head;
    while let Some(index) = rest.find("steamcommunity.com/profiles/") {
        let digits: String = rest[index + 28..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if let Ok(id) = digits.parse::<u64>() {
            last = id;
        }
        rest = &rest[index + 28..];
    }
    last
}

/// Date affichée par Steam (`data-timestamp`, sinon le titre anglais).
fn comment_date(head: &str) -> String {
    if let Some(index) = head.rfind("data-timestamp=\"") {
        let rest = &head[index + 16..];
        if let Some(end) = rest.find('"') {
            if let Ok(seconds) = rest[..end].parse::<i64>() {
                return lapinux_core::util::format_date(seconds);
            }
        }
    }
    if let Some(index) = head.rfind("commentthread_comment_timestamp\" title=\"") {
        let rest = &head[index + 40..];
        if let Some(end) = rest.find('"') {
            return rest[..end].to_string();
        }
    }
    String::new()
}

/// Entités XML et HTML courantes : les noms de joueurs en contiennent.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(index) = rest.find('&') {
        out.push_str(&rest[..index]);
        let tail = &rest[index..];
        let entity = [
            ("&amp;", '&'),
            ("&lt;", '<'),
            ("&gt;", '>'),
            ("&quot;", '"'),
            ("&#39;", '\''),
            ("&apos;", '\''),
            ("&nbsp;", ' '),
        ]
        .into_iter()
        .find(|(name, _)| tail.starts_with(name));
        match entity {
            Some((name, character)) => {
                out.push(character);
                rest = &tail[name.len()..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Extrait réel d'une page de profil : la vitrine d'activité porte des
    /// capsules et des noms de jeux, la vitrine de texte des émoticônes.
    const SHOWCASE_HTML: &str = r#"
<div class="profile_customization">
  <div class="profile_customization_header ellipsis">Jeux que j'aime</div>
  <div class="profile_customization_block"><div class="customtext_showcase">
    <img src="https://community.fastly.steamstatic.com/economy/emoticon/health" class="emoticon">
    <a href="http://store.steampowered.com/app/500/" > Left 4 Dead </a>
  </div></div>
</div>
<div class="profile_customization">
  <div class="profile_customization_header ellipsis"><div>Activité récente</div></div>
  <div class="profile_customization_block"><div class="recent_games">
    <div class="recent_game"><a href="x"><img
      src="https://shared.fastly.steamstatic.com/store_item_assets/steam/apps/4000/capsule.jpg"></a>
      <div class="game_info_details"><div class="game_name">Garry's Mod</div></div></div>
  </div></div>
</div>
"#;

    /// Un visuel de vitrine téléchargé doit devenir une texture : le message
    /// arrive par le canal des amis, et c'est là qu'il faut le lire.
    #[test]
    fn game_capsules_reach_their_texture_through_the_friend_channel() {
        let ctx = egui::Context::default();
        let mut profile = Profile::new();
        profile.game_capsule_pending.insert(620);
        profile
            .friend_tx
            .send(ProfileMsg::GameCapsule {
                appid: 620,
                bytes: include_bytes!("../../../assets/lapinux-icon.png").to_vec(),
            })
            .expect("canal ouvert");
        profile.poll_friend_avatars(&ctx);
        assert!(
            profile.game_capsules.contains_key(&620),
            "la capsule reçue doit devenir une texture"
        );
        assert!(
            !profile.game_capsule_pending.contains(&620),
            "la capsule reçue ne reste pas en attente"
        );
    }

    #[test]
    fn showcase_images_reach_their_texture() {
        let ctx = egui::Context::default();
        let mut profile = Profile::new();
        let url = "https://example.invalid/capsule.jpg".to_string();
        profile.showcase_pending.insert(url.clone());
        profile
            .friend_tx
            .send(ProfileMsg::ShowcaseImage {
                url: url.clone(),
                bytes: include_bytes!("../../../assets/lapinux-icon.png").to_vec(),
            })
            .expect("canal ouvert");
        profile.poll_friend_avatars(&ctx);
        assert!(
            profile.showcase_images.contains_key(&url),
            "le visuel devient une texture"
        );
        assert!(
            profile.showcase_pending.is_empty(),
            "la demande aboutie ne reste pas en attente"
        );
    }

    #[test]
    fn capsule_and_profile_page_download() {
        let url = "https://cdn.akamai.steamstatic.com/steam/apps/438100/capsule_231x87.jpg";
        let bytes = http_bytes(url);
        println!("capsule: {:?}", bytes.as_ref().map(|b| b.len()));
        if let Some(bytes) = bytes {
            println!("decoded: {:?}", image::load_from_memory(&bytes).is_ok());
        }
        let html = http_text("https://steamcommunity.com/profiles/76561198000000000/?l=french");
        match html {
            Ok(html) => println!(
                "page: {} octets, vitrines: {}",
                html.len(),
                parse_showcases(&html).len()
            ),
            Err(error) => println!("page: ERREUR {}", error.message),
        }
    }

    #[test]
    fn showcases_keep_their_title_text_and_capsule() {
        let showcases = parse_showcases(SHOWCASE_HTML);
        assert_eq!(showcases.len(), 2, "{showcases:?}");
        assert_eq!(showcases[0].title, "Jeux que j'aime");
        // Le texte garde ses liens et ses émoticônes, comme la description.
        assert!(
            showcases[0]
                .parts
                .iter()
                .any(|part| matches!(part, DescriptionPart::Link { text, .. } if text.contains("Left 4 Dead"))),
            "{:?}",
            showcases[0].parts
        );
        assert!(showcases[0]
            .parts
            .iter()
            .any(|part| matches!(part, DescriptionPart::Emote { .. })));
        // Les émoticônes ne sont pas des visuels de vitrine.
        assert!(showcases[0].images.is_empty(), "{:?}", showcases[0].images);
        assert_eq!(showcases[1].title, "Activité récente");
        assert_eq!(showcases[1].lines, vec!["Garry's Mod".to_string()]);
        // Une vitrine de jeux n'ajoute pas de vignettes : les images qu'elle
        // porte sont les badges des jeux, déjà résumés par leurs capsules.
        assert!(showcases[1].images.is_empty(), "{:?}", showcases[1].images);
    }

    /// Extrait réel de la page : une vitrine peut porter une classe de plus
    /// (`myart`), et l'activité récente donne la capsule, les heures et les
    /// succès de chaque jeu.
    const RECENT_SHOWCASES_HTML: &str = r#"
<div class="profile_customization">
  <div class="profile_customization_header ellipsis">Jeux que j'aime</div>
  <div class="profile_customization_block"><div class="customtext_showcase">
    <img src="https://community.fastly.steamstatic.com/economy/emoticon/health" alt=":health:" class="emoticon">
    <a href="http://store.steampowered.com/app/500/"> Left 4 Dead </a>
  </div></div>
</div>
<div class="profile_customization myart">
  <div class="profile_customization_header">Vitrine des créations à la une</div>
  <div class="profile_customization_block"><div class="screenshot_showcase">
    <img style="max-width: 630px;" src="https://images.steamusercontent.com/ugc/13916724478483509441/D036/?imw=630&amp;imh=297">
    <div class="screenshot_showcase_itemname">₍ᐢ. .ᐢ₎ ₊˚⊹♡</div>
  </div></div>
</div>
<div class="profile_customization">
  <div class="profile_recentgame_header profile_customization_header">
    <div>Activité récente</div>
    <div class="recentgame_quicklinks recentgame_recentplaytime"><div>9,5 h les 15 derniers jours</div></div>
  </div>
  <div class="profile_customization_block"><div class="recent_games">
    <div class="recent_game"><div class="recent_game_content"><div class="game_info">
      <div class="game_info_cap "><a href="https://steamcommunity.com/app/4000"><img class="game_capsule" src="https://shared.fastly.steamstatic.com/store_item_assets/steam/apps/4000/capsule_184x69.jpg"></a></div>
      <div class="game_info_details">1 776 h en tout<br>dernière utilisation le 20 oct.</div>
      <div class="game_name"><a class="whiteLink" href="https://steamcommunity.com/app/4000">Garry's Mod</a></div>
    </div>
    <div class="game_info_stats"><div class="game_info_achievements_summary_area">
      <span class="game_info_achievement_summary"><a class="whiteLink" href="https://steamcommunity.com/profiles/1/achievements/4000">18 succès sur 29</a></span>
    </div></div></div></div>
  </div></div>
</div>
"#;

    #[test]
    fn showcases_keep_their_own_card_and_recent_games_their_hours() {
        let showcases = parse_showcases(RECENT_SHOWCASES_HTML);
        assert_eq!(showcases.len(), 3, "{showcases:#?}");
        assert_eq!(showcases[0].title, "Jeux que j'aime");
        // La vitrine de créations a sa propre carte, avec son visuel.
        assert_eq!(showcases[1].title, "Vitrine des créations à la une");
        assert_eq!(showcases[1].images.len(), 1, "{:?}", showcases[1].images);
        assert!(showcases[1].images[0].starts_with("https://images.steamusercontent.com/ugc/"));
        // Les jeux récents portent leur capsule, leurs heures et leurs succès.
        assert_eq!(showcases[2].title, "Activité récente");
        assert_eq!(
            showcases[2].subtitle, "9,5 h les 15 derniers jours",
            "le temps des quinze derniers jours suit le titre"
        );
        let game = showcases[2].games.first().expect("un jeu récent détaillé");
        assert_eq!(game.name, "Garry's Mod");
        assert_eq!(game.hours, "1 776 h en tout");
        assert!(
            game.last_used.contains("dernière utilisation"),
            "{:?}",
            game.last_used
        );
        assert_eq!(game.achievements, "18 succès sur 29");
        assert!(game.capsule.ends_with("capsule_184x69.jpg"));
    }
    use lapinux_core::social::{SteamFamily, SteamFamilyMember, SteamNotification};

    const PROFILE_XML: &str = r#"<?xml version="1.0"?><profile>
        <steamID64>76561198000000000</steamID64>
        <steamID><![CDATA[JoueurExemple]]></steamID>
        <onlineState>offline</onlineState>
        <stateMessage><![CDATA[Offline]]></stateMessage>
        <privacyState>public</privacyState>
        <avatarIcon><![CDATA[https://avatars.example/icon.jpg]]></avatarIcon>
        <avatarMedium><![CDATA[https://avatars.example/medium.jpg]]></avatarMedium>
        <memberSince>December 26, 2015</memberSince>
        <hoursPlayed2Wk>0.0</hoursPlayed2Wk>
        <headline><![CDATA[]]></headline>
        <location><![CDATA[France]]></location>
        <realname><![CDATA[&lt;Joueur&gt;]]></realname>
        <customURL><![CDATA[test-user]]></customURL>
        <vacBanned>0</vacBanned>
        <tradeBanState><![CDATA[None]]></tradeBanState>
        <summary><![CDATA[Fan d'<a class="bb_link" href="https://store.steampowered.com/tags/fr/Horreur/">horreur</a> et de zombies.<br>J'aime aussi les jeux cozy.]]></summary>
        <mostPlayedGames>
            <mostPlayedGame>
                <gameName><![CDATA[Resident Evil 6]]></gameName>
                <gameLink>https://steamcommunity.com/app/221040</gameLink>
                <gameIcon><![CDATA[https://shared.example/221040.jpg]]></gameIcon>
                <hoursPlayed>4.9</hoursPlayed>
                <hoursOnRecord>7.1</hoursOnRecord>
            </mostPlayedGame>
            <mostPlayedGame>
                <gameName><![CDATA[Garry's Mod]]></gameName>
                <gameLink>https://steamcommunity.com/app/4000/</gameLink>
                <hoursPlayed>1.0</hoursPlayed>
            </mostPlayedGame>
        </mostPlayedGames>
    </profile>"#;

    #[test]
    fn a_public_profile_is_read_tag_by_tag() {
        let profile = profile_from_xml(PROFILE_XML);
        assert_eq!(profile.steam_id, "76561198000000000");
        assert_eq!(profile.name, "JoueurExemple");
        assert_eq!(profile.privacy_state, "public");
        assert_eq!(profile.location, "France");
        // Les entités XML sont rendues : un nom ne doit pas s'afficher en
        // `&amp;` ou en `&#39;`.
        assert_eq!(profile.real_name, "<Joueur>");
        assert_eq!(profile.avatar_url, "https://avatars.example/medium.jpg");
        assert_eq!(profile.custom_url, "steamcommunity.com/id/test-user");
    }

    /// Texte de la description, les émoticônes écrites `:nom:`.
    fn description_text(parts: &[DescriptionPart]) -> String {
        parts
            .iter()
            .map(|part| match part {
                DescriptionPart::Text(text) => text.clone(),
                DescriptionPart::Link { text, .. } => text.clone(),
                DescriptionPart::Emote { name, .. } => format!(":{name}:"),
            })
            .collect()
    }

    #[test]
    fn the_description_loses_its_html_but_keeps_its_lines() {
        let profile = profile_from_xml(PROFILE_XML);
        assert_eq!(
            description_text(&profile.description),
            "Fan d'horreur et de zombies.\nJ'aime aussi les jeux cozy."
        );
    }

    #[test]
    fn steam_emoticons_become_images_in_the_description() {
        // Forme réelle de la description publique : les émoticônes sont des
        // images `:nom:` au milieu du texte et des liens.
        let html = concat!(
            "Fan d'<a class=\"bb_link\" href=\"http://store.steampowered.com/tags/fr/Horreur/\">horreur</a>  ",
            "<img src=\"https://community.fastly.steamstatic.com/economy/emoticon/marijacooing\" ",
            "alt=\":marijacooing:\" class=\"emoticon\">, de zombies",
            "<img src=\"https://community.fastly.steamstatic.com/economy/emoticon/burozombie\" ",
            "alt=\":burozombie:\" class=\"emoticon\">.<br>Encore",
        );
        let parts = parse_description(html);
        let emotes: Vec<(&str, &str)> = parts
            .iter()
            .filter_map(|part| match part {
                DescriptionPart::Emote { name, url } => Some((name.as_str(), url.as_str())),
                _ => None,
            })
            .collect();
        assert_eq!(emotes.len(), 2);
        assert_eq!(emotes[0].0, "marijacooing");
        assert!(emotes[0].1.ends_with("/economy/emoticon/marijacooing"));
        assert_eq!(emotes[1].0, "burozombie");
        // Le texte garde ses mots et ses lignes, sans code d'émoticône ; les
        // liens gardent leur libellé à leur place.
        let text: String = parts
            .iter()
            .filter_map(|part| match part {
                DescriptionPart::Text(text) => Some(text.as_str()),
                DescriptionPart::Link { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(text, "Fan d'horreur , de zombies.\nEncore");
        // Les lignes restent séparées pour l'affichage.
        let lines = description_lines(&parts);
        assert_eq!(lines.len(), 2);
        assert!(lines[0]
            .iter()
            .any(|piece| matches!(piece, DescriptionPiece::Emote { .. })));
        assert!(lines[1]
            .iter()
            .all(|piece| matches!(piece, DescriptionPiece::Word(_))));
    }

    #[test]
    fn description_links_keep_their_target() {
        // Forme réelle : les liens sortants passent par le filtre de Steam et
        // l'hôte est rappelé après le lien.
        let html = concat!(
            "J'aime aussi les jeux cozy. ",
            "<a href=\"https://steamcommunity.com/linkfilter/?u=http%3A%2F%2Fstopkillinggames.com%2F\" ",
            "target=\"_blank\">Stop Killing Games</a>",
            "<span class=\"bb_link_host\">[stopkillinggames.com]</span>",
        );
        let parts = parse_description(html);
        let link = parts.iter().find_map(|part| match part {
            DescriptionPart::Link { text, url } => Some((text.as_str(), url.as_str())),
            _ => None,
        });
        assert_eq!(
            link,
            Some(("Stop Killing Games", "http://stopkillinggames.com/"))
        );
        // Le texte autour du lien n'est pas avalé par celui-ci.
        let text: String = parts
            .iter()
            .filter_map(|part| match part {
                DescriptionPart::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(text, "J'aime aussi les jeux cozy. [stopkillinggames.com]");
        // Le lien est un fragment à part, à sa place dans la ligne.
        let lines = description_lines(&parts);
        assert!(lines
            .iter()
            .flatten()
            .any(|piece| matches!(piece, DescriptionPiece::Link { .. })));
    }

    #[test]
    fn plain_links_are_opened_as_is() {
        let parts = parse_description(
            "<a href=\"https://store.steampowered.com/tags/fr/Horreur/\">horreur</a>",
        );
        assert_eq!(
            parts,
            vec![DescriptionPart::Link {
                text: "horreur".into(),
                url: "https://store.steampowered.com/tags/fr/Horreur/".into(),
            }]
        );
    }

    #[test]
    fn a_private_profile_only_gives_the_identity() {
        let xml = "<profile><steamID64>1</steamID64><privacyState>private</privacyState></profile>";
        let profile = profile_from_xml(xml);
        assert_eq!(profile.steam_id, "1");
        assert_eq!(profile.privacy_state, "private");
        assert!(profile.name.is_empty());
        assert_eq!(
            privacy_label(&profile.privacy_state),
            lapinux_core::i18n::t("profile.private")
        );
    }

    #[test]
    fn an_unknown_steam_id_says_so() {
        let xml = "<response><error><![CDATA[The specified profile could not be found.]]></error></response>";
        let profile = profile_from_xml(xml);
        assert!(profile.steam_id.is_empty());
    }

    #[test]
    fn comments_keep_author_date_and_text() {
        // Exemple de page de profil : le total et les commentaires y
        // sont, donc une seule requête suffit.
        let page = r#"
<div class="commentthread_area">
  <a class="commentthread_allcommentslink" href="https://steamcommunity.com/profiles/1/allcomments">Voir les <span id="commentthread_Profile_1_totalcount">30</span> commentaires</a>
  <div class="commentthread_comments">
    <div class="commentthread_comment"><div class="commentthread_comment_avatar"><img src="https://avatars.example/exemple.jpg"></div><a class="commentthread_author_link" href="https://steamcommunity.com/profiles/76561198000000001"><bdi>JoueurExemple</bdi></a><div class="commentthread_comment_timestamp" data-timestamp="1786554596">12 Aug</div><div class="commentthread_comment_text" id="comment_content_1">
        super <b>jeu</b>&nbsp;! <a class="bb_link" href="https://store.steampowered.com/app/620">Portal 2</a>
    </div></div>
  </div>
</div>
"#;
        let (total, comments) = comments_from_page(page);
        assert_eq!(total, 30);
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].author, "JoueurExemple");
        assert_eq!(comments[0].author_id, 7_656_119_800_000_000_1);
        // Le texte garde son lien, et la photo de l'auteur est retenue.
        assert!(
            comments[0].parts.iter().any(|part| matches!(
                part,
                DescriptionPart::Link { text, .. } if text == "Portal 2"
            )),
            "le lien du commentaire est lisible : {:?}",
            comments[0].parts
        );
        assert!(comments[0].avatar.ends_with("exemple.jpg"));
        assert!(!comments[0].posted.is_empty());
    }

    #[test]
    fn an_empty_comment_list_is_still_valid() {
        let (total, comments) = comments_from_page("<html><body>sans commentaire</body></html>");
        assert_eq!(total, 0);
        assert!(comments.is_empty());
    }

    #[test]
    fn a_query_parameter_survives_the_encoding() {
        // Le JSON de la fiche de boutique passe dans l'adresse : sans cet
        // encodage, les accolades et les guillemets cassent la requête.
        let encoded = percent_encode(r#"{"appid":620}"#);
        assert_eq!(encoded, "%7B%22appid%22%3A620%7D");
        assert_eq!(percent_encode("aZ0-_.~"), "aZ0-_.~");
    }

    /// Les catégories d'amis suivent le modèle de Téléchargements : un titre
    /// qui se clique, et « Hors ligne » fermé au départ.
    /// Les photos et les emoji arrivent par le fil des images : un message
    /// posé sur le mauvais fil restait sans texture, donc invisible.
    #[test]
    fn downloaded_pictures_reach_their_textures() {
        let ctx = egui::Context::default();
        crate::install_fonts(&ctx);
        let _ = ctx.run(egui::RawInput::default(), |_| {});
        let png = {
            let pixel = image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 0, 0, 255]));
            let mut bytes = Vec::new();
            image::DynamicImage::ImageRgba8(pixel)
                .write_to(
                    &mut std::io::Cursor::new(&mut bytes),
                    image::ImageFormat::Png,
                )
                .expect("PNG d'essai");
            bytes
        };
        let mut profile = Profile::new();
        profile
            .friend_tx
            .send(ProfileMsg::Emoji {
                text: "🧟‍♀️".into(),
                bytes: png.clone(),
            })
            .unwrap();
        profile
            .friend_tx
            .send(ProfileMsg::CommentAvatar {
                url: "https://avatars.example/a.jpg".into(),
                bytes: png,
            })
            .unwrap();
        profile.poll_friend_avatars(&ctx);
        assert!(profile.emojis.contains_key("🧟‍♀️"));
        assert!(profile
            .comment_avatars
            .contains_key("https://avatars.example/a.jpg"));
        assert!(!profile.emoji_pending.contains("🧟‍♀️"));
    }

    #[test]
    fn friends_categories_start_with_offline_closed() {
        let ctx = egui::Context::default();
        crate::install_fonts(&ctx);
        let mut profile = sample_profile();
        let (output, _) = page_frame(&ctx, &mut profile, Vec::new());
        let playing = painted_text(&output, lapinux_core::i18n::t("profile.in_game"))
            .expect("catégorie en jeu");
        let offline = painted_text(&output, lapinux_core::i18n::t("profile.offline"))
            .expect("catégorie hors ligne");
        // Alice joue : sa ligne est là. Bob est absent : sa catégorie attend.
        assert!(painted_text(&output, "Alice").is_some());
        assert!(painted_text(&output, "Bob").is_none());
        // Le clic sur le titre ouvre la catégorie.
        page_frame(&ctx, &mut profile, press(offline.center()));
        page_frame(&ctx, &mut profile, release(offline.center()));
        let (output, _) = page_frame(&ctx, &mut profile, Vec::new());
        assert!(
            painted_text(&output, "Bob").is_some(),
            "la catégorie hors ligne s'ouvre au clic"
        );
        // Replier « En jeu » retire la ligne du joueur.
        page_frame(&ctx, &mut profile, press(playing.center()));
        page_frame(&ctx, &mut profile, release(playing.center()));
        let (output, _) = page_frame(&ctx, &mut profile, Vec::new());
        assert!(painted_text(&output, "Alice").is_none());
    }

    #[test]
    fn emoji_leave_the_text_and_keep_their_sequence() {
        let parts = split_emoji("Fan d'horreur 🧟‍♀️ et de 📹 jeux");
        assert_eq!(parts[0], (false, "Fan d'horreur "));
        assert_eq!(parts[1], (true, "🧟‍♀️"));
        assert_eq!(parts[2], (false, " et de "));
        assert_eq!(parts[3], (true, "📹"));
        assert_eq!(parts[4], (false, " jeux"));
        // Le fichier Twemoji garde la séquence entière, puis sans le sélecteur
        // de présentation : les deux adresses sont essayées.
        let urls = emoji_urls("🧟‍♀️");
        assert!(urls[0].ends_with("1f9df-200d-2640-fe0f.png"), "{}", urls[0]);
        assert!(urls[1].ends_with("1f9df-200d-2640.png"), "{}", urls[1]);
    }

    #[test]
    fn a_steam_id_is_masked_until_asked() {
        assert_eq!(masked_steam_id("76561198000000000"), "ID 765••••00");
        assert_eq!(masked_steam_id("1234"), "ID 1234");
    }

    #[test]
    fn presence_colors_follow_the_state() {
        assert_eq!(presence_color(SteamPresence::Online), OK);
        assert_eq!(presence_color(SteamPresence::Away), WARN);
        assert_eq!(presence_color(SteamPresence::Busy), DANGER);
        assert_eq!(presence_color(SteamPresence::Invisible), TEXT_DIM());
    }

    #[test]
    fn last_seen_lines_stay_short() {
        let friend = SteamFriend {
            presence: SteamPresence::Offline,
            last_logoff: lapinux_core::util::now_unix() - 7200,
            ..SteamFriend::default()
        };
        assert_eq!(
            last_seen_line(&friend),
            lapinux_core::i18n::t_fmt("profile.offline_ago", &["2 h".to_string()])
        );
        let online = SteamFriend {
            presence: SteamPresence::Online,
            ..SteamFriend::default()
        };
        assert_eq!(
            last_seen_line(&online),
            lapinux_core::i18n::t("profile.online")
        );
    }

    #[test]
    fn online_state_and_hours_are_readable() {
        let mut profile = SteamProfile {
            online_state: "in-game".into(),
            ..SteamProfile::default()
        };
        assert_eq!(
            online_label(&profile).1,
            lapinux_core::i18n::t("profile.in_game")
        );
        profile.online_state = "offline".into();
        assert_eq!(
            online_label(&profile).1,
            lapinux_core::i18n::t("profile.offline")
        );
        assert_eq!(
            format_hours("4.9"),
            format!("{} h", lapinux_core::i18n::decimal("4.9"))
        );
        assert_eq!(
            format_hours(""),
            lapinux_core::i18n::t("controllers.information.dash")
        );
    }

    #[test]
    fn a_family_member_without_a_session_name_borrows_the_public_one() {
        let names = HashMap::from([(7u64, "JoueurExemple".to_string())]);
        let unknown = SteamFamilyMember {
            steam_id: 7,
            ..SteamFamilyMember::default()
        };
        assert_eq!(family_member_name(&unknown, &names), "JoueurExemple");
        // Le nom de la session passe devant, et un membre jamais lu reste vide
        // plutôt que de montrer son identifiant.
        let known = SteamFamilyMember {
            steam_id: 7,
            name: "Alex".into(),
            ..SteamFamilyMember::default()
        };
        assert_eq!(family_member_name(&known, &names), "Alex");
        let stranger = SteamFamilyMember {
            steam_id: 9,
            ..SteamFamilyMember::default()
        };
        assert!(family_member_name(&stranger, &names).is_empty());
    }

    /// État de page complet : identité, amis, famille, commentaires.
    fn sample_profile() -> Profile {
        let mut profile = Profile::new();
        let mut identity = profile_from_xml(PROFILE_XML);
        identity.name = "Alex".into();
        identity.real_name = "<Alex>".into();
        profile.profile = Some(identity);
        profile.comments = Some((
            30,
            vec![CommentView {
                author: "JoueurExemple".into(),
                posted: "12 Aug".into(),
                parts: vec![DescriptionPart::Text("super jeu".into())],
                ..CommentView::default()
            }],
        ));
        // Le réseau n'est pas sollicité : les deux identités sont marquées
        // comme déjà demandées, et le nom du second membre vient de la page
        // publique (déjà lue ici).
        profile
            .family_profiles_asked
            .extend([7_656_119_800_000_000_2, 7_656_119_800_000_000_1]);
        profile
            .friend_avatar_pending
            .extend([7_656_119_800_000_000_2, 7_656_119_800_000_000_1]);
        profile
            .family_names
            .insert(7_656_119_800_000_000_1, "JoueurExemple".into());
        profile.set_social(Some(SteamSocial {
            state: "live".into(),
            persona_name: "Alex".into(),
            steam_id: 7_656_119_800_000_000_2,
            friends: vec![
                SteamFriend {
                    steam_id: 1,
                    name: "Alice".into(),
                    presence: SteamPresence::Online,
                    game: "Portal 2".into(),
                    game_appid: 620,
                    ..SteamFriend::default()
                },
                SteamFriend {
                    steam_id: 2,
                    name: "Bob".into(),
                    presence: SteamPresence::Offline,
                    ..SteamFriend::default()
                },
            ],
            notifications: vec![SteamNotification {
                kind: 2,
                count: 1,
                at: 0,
            }],
            new_comments: 2,
            family: Some(SteamFamily {
                name: "Maison".into(),
                members: vec![
                    SteamFamilyMember {
                        steam_id: 7_656_119_800_000_000_2,
                        name: "Alex".into(),
                        role: 1,
                        is_self: true,
                        avatar: String::new(),
                        games: 42,
                        ..SteamFamilyMember::default()
                    },
                    SteamFamilyMember {
                        steam_id: 7_656_119_800_000_000_1,
                        role: 2,
                        games: 3,
                        ..SteamFamilyMember::default()
                    },
                ],
                ..SteamFamily::default()
            }),
            presence: SteamPresence::Online,
            ..SteamSocial::default()
        }));
        profile
    }

    /// Les quatre plateformes de la liste, comme l'application les fournit :
    /// Steam connecté, les trois boutiques en attente de compte.
    fn sample_platforms() -> Vec<PlatformEntry> {
        vec![
            PlatformEntry {
                platform: Platform::Steam,
                account: "Alex".into(),
                connected: true,
            },
            PlatformEntry {
                platform: Platform::Epic,
                account: String::new(),
                connected: false,
            },
            PlatformEntry {
                platform: Platform::Gog,
                account: String::new(),
                connected: false,
            },
            PlatformEntry {
                platform: Platform::Itch,
                account: String::new(),
                connected: false,
            },
        ]
    }

    /// Dessine la page à la largeur demandée et rend ce qui a été peint.
    fn render_profile_page(profile: &mut Profile, width: f32) -> egui::FullOutput {
        let ctx = egui::Context::default();
        crate::install_fonts(&ctx);
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(width + 40.0, 1400.0),
        ));
        ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.set_width(width);
                let actions =
                    profile.page(ui, "Alex", &sample_platforms(), &mut |_| {}, &mut |_| {});
                assert!(actions.is_empty());
            });
        })
    }

    /// Rend la page et renvoie ses actions. Un libellé fourni est cliqué
    /// (appui puis relâchement) dans la même session egui : c'est le chemin
    /// qu'emprunte un vrai clic dans la page sociale.
    fn render_profile_page_actions(
        profile: &mut Profile,
        width: f32,
        click: Option<&str>,
    ) -> (egui::FullOutput, Vec<ProfileAction>) {
        let ctx = egui::Context::default();
        crate::install_fonts(&ctx);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width + 40.0, 1600.0));
        let mut frame = |input: egui::RawInput| {
            let mut actions = Vec::new();
            let output = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.set_width(width);
                    actions =
                        profile.page(ui, "Alex", &sample_platforms(), &mut |_| {}, &mut |_| {});
                });
            });
            (output, actions)
        };
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(screen);
        let (output, actions) = frame(input);
        let Some(label) = click else {
            return (output, actions);
        };
        let Some(rect) = painted_text(&output, label) else {
            return (output, actions);
        };
        let pos = rect.center();
        let mut press = egui::RawInput::default();
        press.screen_rect = Some(screen);
        press.events = vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            },
        ];
        let _ = frame(press);
        let mut release = egui::RawInput::default();
        release.screen_rect = Some(screen);
        release.events = vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        }];
        frame(release)
    }

    /// Une invitation avec adresse affiche son bouton, et le clic produit la
    /// rejointe avec l'appid, le lobby et l'adresse du registre.
    #[test]
    fn a_lobby_invite_shows_a_join_button_and_its_click_joins() {
        let mut profile = sample_profile();
        let mut social = SteamSocial {
            state: "live".into(),
            ..SteamSocial::default()
        };
        social.invites = vec![lapinux_core::social::SteamLobbyInvite {
            appid: 4_670_920,
            lobby: 109_775_240_000_000_001,
            inviter: 76_561_198_000_000_001,
            inviter_name: Some("JoueurExemple".into()),
            connect: Some("192.168.1.50:7777".into()),
        }];
        profile.social = Some(social);
        let (output, without_click) = render_profile_page_actions(&mut profile, 1200.0, None);
        assert!(without_click.is_empty(), "aucune action sans clic");
        assert!(
            painted_text(&output, lapinux_core::i18n::t("profile.join")).is_some(),
            "le bouton Rejoindre sort du rendu de la page sociale"
        );
        assert!(
            painted_text(
                &output,
                &lapinux_core::i18n::t_args("profile.invite_lobby", &[("who", "JoueurExemple")]),
            )
            .is_some(),
            "l'invitation nomme l'ami qui invite"
        );
        let (_, actions) = render_profile_page_actions(
            &mut profile,
            1200.0,
            Some(lapinux_core::i18n::t("profile.join")),
        );
        assert_eq!(actions.len(), 1, "un clic, une action");
        match &actions[0] {
            ProfileAction::JoinInvite {
                appid,
                lobby,
                connect,
            } => {
                assert_eq!(*appid, 4_670_920);
                assert_eq!(*lobby, 109_775_240_000_000_001);
                assert_eq!(connect, "192.168.1.50:7777");
            }
            _ => panic!("action inattendue : le clic doit rejoindre le lobby"),
        }
    }

    /// Sans adresse, le bouton reste offert : la rejointe la demandera au
    /// lobby, et la requête part sans adresse préalable.
    #[test]
    fn without_an_address_the_join_button_asks_the_lobby_for_one() {
        let mut profile = sample_profile();
        let mut social = SteamSocial {
            state: "live".into(),
            ..SteamSocial::default()
        };
        social.invites = vec![lapinux_core::social::SteamLobbyInvite {
            appid: 4_670_920,
            lobby: 109_775_240_000_000_001,
            inviter: 76_561_198_000_000_001,
            inviter_name: None,
            connect: None,
        }];
        profile.social = Some(social);
        let (output, actions) = render_profile_page_actions(
            &mut profile,
            1200.0,
            Some(lapinux_core::i18n::t("profile.join")),
        );
        assert!(
            painted_text(&output, lapinux_core::i18n::t("profile.lobby_address")).is_some(),
            "la découverte de l'adresse est annoncée"
        );
        assert_eq!(actions.len(), 1, "le clic émet la rejointe");
        match &actions[0] {
            ProfileAction::JoinInvite { lobby, connect, .. } => {
                assert_eq!(*lobby, 109_775_240_000_000_001);
                assert!(connect.is_empty(), "l'adresse sera demandée au lobby");
            }
            _ => panic!("action inattendue : le clic doit rejoindre le lobby"),
        }
    }

    /// Rectangle d'un texte peint : la mise en page se vérifie sans la voir.
    fn painted_text(output: &egui::FullOutput, label: &str) -> Option<egui::Rect> {
        fn find(shape: &egui::Shape, label: &str) -> Option<egui::Rect> {
            match shape {
                egui::Shape::Text(text) if text.galley.text() == label => {
                    Some(egui::Rect::from_min_size(text.pos, text.galley.rect.size()))
                }
                egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| find(shape, label)),
                _ => None,
            }
        }
        output
            .shapes
            .iter()
            .find_map(|clipped| find(&clipped.shape, label))
    }

    /// Rectangle d'un texte peint, comparé sans ses espaces de bord : le menu
    /// d'état réserve la place de son point par des espaces.
    fn painted_text_trimmed(output: &egui::FullOutput, label: &str) -> Option<egui::Rect> {
        fn find(shape: &egui::Shape, label: &str) -> Option<egui::Rect> {
            match shape {
                egui::Shape::Text(text) if text.galley.text().trim() == label => {
                    Some(egui::Rect::from_min_size(text.pos, text.galley.rect.size()))
                }
                egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| find(shape, label)),
                _ => None,
            }
        }
        output
            .shapes
            .iter()
            .find_map(|clipped| find(&clipped.shape, label))
    }

    /// Pastilles pleines peintes : centre et rayon, pour retrouver le point
    /// d'état d'un menu ou d'une ligne.
    fn painted_dots(output: &egui::FullOutput) -> Vec<(egui::Pos2, f32)> {
        fn walk(shape: &egui::Shape, dots: &mut Vec<(egui::Pos2, f32)>) {
            match shape {
                egui::Shape::Circle(circle) if circle.stroke.width == 0.0 => {
                    dots.push((circle.center, circle.radius));
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, dots);
                    }
                }
                _ => {}
            }
        }
        let mut dots = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut dots);
        }
        dots
    }

    /// Même chose, mais dans une bande d'abscisses : un libellé peut apparaître
    /// deux fois — le compte dans la liste des plateformes, le nom dans la
    /// fiche.
    fn painted_text_in(
        output: &egui::FullOutput,
        label: &str,
        left: f32,
        right: f32,
    ) -> Option<egui::Rect> {
        fn find(shape: &egui::Shape, label: &str, left: f32, right: f32) -> Option<egui::Rect> {
            match shape {
                egui::Shape::Text(text)
                    if text.galley.text() == label && text.pos.x >= left && text.pos.x <= right =>
                {
                    Some(egui::Rect::from_min_size(text.pos, text.galley.rect.size()))
                }
                egui::Shape::Vec(shapes) => shapes
                    .iter()
                    .find_map(|shape| find(shape, label, left, right)),
                _ => None,
            }
        }
        output
            .shapes
            .iter()
            .find_map(|clipped| find(&clipped.shape, label, left, right))
    }

    /// Les images peintes (textures) : la marque Steam de la liste en est une.
    fn painted_marks(output: &egui::FullOutput) -> Vec<egui::Rect> {
        fn walk(shape: &egui::Shape, marks: &mut Vec<egui::Rect>) {
            match shape {
                egui::Shape::Mesh(mesh) if mesh.texture_id != egui::TextureId::default() => {
                    marks.push(
                        mesh.vertices
                            .iter()
                            .fold(egui::Rect::NOTHING, |rect, vertex| {
                                rect.union(egui::Rect::from_min_max(vertex.pos, vertex.pos))
                            }),
                    );
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, marks);
                    }
                }
                _ => {}
            }
        }
        let mut marks = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut marks);
        }
        marks
    }

    /// La section Profil se coupe en deux : la liste des plateformes à gauche,
    /// la fiche de celle qui est choisie à droite.
    #[test]
    fn the_platforms_list_sits_on_the_left_of_the_steam_page() {
        let mut profile = sample_profile();
        let output = render_profile_page(&mut profile, 1200.0);
        let list = painted_text(&output, lapinux_core::i18n::t("ui.profile_platforms"))
            .expect("le titre de la liste");
        assert!(
            list.left() < PLATFORM_SIDE,
            "la liste est à gauche (x = {})",
            list.left()
        );
        // L'entrée nomme la plateforme, et le compte connecté juste en dessous.
        assert!(
            painted_text_in(&output, "Steam", 0.0, PLATFORM_SIDE).is_some(),
            "l'entrée Steam est nommée"
        );
        // Le pseudo est masqué par défaut : la ligne montre des points.
        assert!(
            painted_text_in(&output, "••••••", 0.0, PLATFORM_SIDE).is_some(),
            "le pseudo masqué s'écrit sous le nom"
        );
        assert!(
            painted_text_in(&output, "Alex", 0.0, PLATFORM_SIDE).is_none(),
            "le pseudo ne doit pas fuiter"
        );
        // L'œil de la ligne le dévoile.
        profile.revealed_accounts.insert(Platform::Steam);
        let output = render_profile_page(&mut profile, 1200.0);
        assert!(
            painted_text_in(&output, "Alex", 0.0, PLATFORM_SIDE).is_some(),
            "l'œil dévoile le compte"
        );
        // Le logo est dans sa tuile, sous le titre de la liste.
        let mark = painted_marks(&output)
            .into_iter()
            .find(|rect| rect.left() < PLATFORM_SIDE && rect.top() > list.top())
            .expect("le logo Steam de la liste");
        assert!(mark.width() > 12.0, "le logo est peint à taille lisible");
        // La fiche Steam commence à droite de la liste.
        let bio = painted_text(&output, lapinux_core::i18n::t("profile.member_since"))
            .expect("fiche dessinée");
        assert!(
            bio.left() >= PLATFORM_SIDE,
            "la fiche commence à droite de la liste (x = {})",
            bio.left()
        );
    }

    /// Fenêtre étroite : la même liste, au-dessus de la fiche.
    #[test]
    fn a_narrow_window_stacks_the_list_over_the_page() {
        let mut profile = sample_profile();
        let output = render_profile_page(&mut profile, 700.0);
        let list = painted_text(&output, lapinux_core::i18n::t("ui.profile_platforms"))
            .expect("le titre de la liste");
        let bio = painted_text(&output, lapinux_core::i18n::t("profile.member_since"))
            .expect("fiche dessinée");
        assert!(
            list.left() < 200.0,
            "la liste reste à gauche (x = {})",
            list.left()
        );
        assert!(
            list.bottom() <= bio.top(),
            "la liste passe au-dessus de la fiche ({} puis {})",
            list.bottom(),
            bio.top()
        );
    }

    #[test]
    fn a_friend_who_plays_reads_joue_a_then_the_game() {
        let mut profile = sample_profile();
        let output = render_profile_page(&mut profile, 1240.0);
        // Le titre n'est plus poussé contre le bord droit : la ligne se lit
        // « Joue à … », juste après le pseudo et la capsule.
        let line = painted_text(
            &output,
            &lapinux_core::i18n::t_args("profile.playing", &[("title", "Portal 2")]),
        )
        .expect("ligne de jeu dessinée");
        let name = painted_text_in(&output, "Alice", PLATFORM_SIDE, f32::INFINITY)
            .expect("nom de l'ami dessiné");
        assert!(
            line.left() > name.left(),
            "le jeu suit le pseudo (x = {})",
            line.left()
        );
    }

    #[test]
    fn the_family_card_counts_the_games_each_member_shares() {
        let mut profile = sample_profile();
        let output = render_profile_page(&mut profile, 1240.0);
        assert!(
            painted_text(
                &output,
                &lapinux_core::i18n::t_fmt("profile.games_count", &["42".to_string()])
            )
            .is_some(),
            "le compte du compte connecté doit être écrit"
        );
        assert!(
            painted_text(
                &output,
                &lapinux_core::i18n::t_fmt("profile.games_count", &["3".to_string()])
            )
            .is_some(),
            "le compte du second membre doit être écrit"
        );
    }

    #[test]
    fn clicking_a_family_count_selects_the_member_for_the_shared_games_modal() {
        let mut profile = sample_profile();
        let count = lapinux_core::i18n::t_fmt("profile.games_count", &["3".to_string()]);
        let (_, actions) = render_profile_page_actions(&mut profile, 1240.0, Some(&count));
        assert!(actions.is_empty());
        assert_eq!(profile.family_games, Some(7_656_119_800_000_000_1));
    }

    #[test]
    fn shared_games_keep_their_appid_and_use_the_catalog_name() {
        let catalog = lapinux_core::model::SteamCatalog {
            entries: vec![lapinux_core::model::SteamCatalogGame {
                appid: 620,
                name: "Portal 2".into(),
                installed: false,
                family: false,
                details: Default::default(),
                dlc: Vec::new(),
                acquired: None,
                undated_reason: None,
                minutes: None,
            }],
            ..Default::default()
        };
        let games = family_games(&[999, 620], Some(&catalog));
        assert_eq!(games[0].appid, 999);
        assert_eq!(games[0].name, "App 999");
        assert_eq!(games[1].appid, 620);
        assert_eq!(games[1].name, "Portal 2");
    }

    #[test]
    fn the_family_games_modal_lists_the_shared_games_by_name() {
        let mut profile = sample_profile();
        if let Some(family) = profile
            .social
            .as_mut()
            .and_then(|social| social.family.as_mut())
        {
            family.members[1].appids = vec![620, 4000];
        }
        profile.family_games = Some(7_656_119_800_000_000_1);
        let catalog_game = |appid, name: &str| lapinux_core::model::SteamCatalogGame {
            appid,
            name: name.into(),
            installed: false,
            family: false,
            details: Default::default(),
            dlc: Vec::new(),
            acquired: None,
            undated_reason: None,
            minutes: None,
        };
        let catalog = lapinux_core::model::SteamCatalog {
            entries: vec![
                catalog_game(620, "Portal 2"),
                catalog_game(4000, "Garry's Mod"),
            ],
            ..Default::default()
        };
        let ctx = egui::Context::default();
        crate::install_fonts(&ctx);
        // Une fenêtre egui prend sa taille au premier passage ; le second
        // la peint avec son contenu, comme les images suivantes de l'app.
        let mut run = || {
            let mut input = egui::RawInput::default();
            input.screen_rect = Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 700.0),
            ));
            ctx.run(input, |ctx| {
                profile.family_games_modal(ctx, Some(&catalog));
            })
        };
        let _ = run();
        let output = run();
        let portal = painted_text(&output, "Portal 2").expect("Portal 2 absent du modal");
        assert!(
            painted_text(&output, "Garry's Mod").is_some(),
            "Garry's Mod absent du modal"
        );
        assert!(
            portal.left() < 360.0,
            "le titre doit rester à gauche de sa vignette (x = {})",
            portal.left()
        );
        assert!(
            painted_text(
                &output,
                &lapinux_core::i18n::t_fmt("profile.family_games_count", &["2".to_string()]),
            )
            .is_some(),
            "le compte du modal est absent"
        );
        let title =
            lapinux_core::i18n::t_args("profile.family_games_title", &[("name", "JoueurExemple")]);
        assert!(
            painted_text(&output, &title).is_some(),
            "le titre doit nommer le membre"
        );
        assert!(
            painted_text(&output, "{name}").is_none(),
            "le titre ne doit plus garder le gabarit"
        );
    }

    #[test]
    fn the_purchases_tab_only_offers_to_activate_a_key() {
        let mut profile = sample_profile();
        profile.tab = ProfileTab::Achats;
        let output = render_profile_page(&mut profile, 1240.0);
        // Les deux onglets sont là, et celui des achats est ouvert.
        assert!(painted_text(&output, lapinux_core::i18n::t("profile.tab_profile")).is_some());
        assert!(painted_text(&output, lapinux_core::i18n::t("profile.tab_purchases")).is_some());
        assert!(painted_text(&output, lapinux_core::i18n::t("profile.activate_game")).is_some());
        // Plus d'historique : Steam n'aime pas qu'on relise ses licences.
        assert!(
            painted_text(&output, "Historique des achats").is_none(),
            "l'onglet ne montre plus que l'activation"
        );
    }

    #[test]
    fn the_page_draws_identity_friends_family_and_comments() {
        for width in [680.0_f32, 1060.0] {
            let mut profile = sample_profile();
            let output = render_profile_page(&mut profile, width);
            assert!(
                !output.shapes.is_empty(),
                "la fiche du profil doit être dessinée ({width} px)"
            );
        }
    }

    #[test]
    fn the_family_card_sits_beside_the_bio_on_a_wide_page() {
        let mut profile = sample_profile();
        let narrow = render_profile_page(&mut profile, 680.0);
        let family = painted_text(&narrow, lapinux_core::i18n::t("ui.profile_family"))
            .expect("encart famille dessiné");
        // Étroit : les encarts s'empilent, la famille reste dans la colonne.
        assert!(
            family.left() < 680.0 * 0.5,
            "famille empilée (x = {})",
            family.left()
        );

        // Large : la liste des plateformes prend sa colonne, la fiche garde
        // celle qui reste — assez pour la bio et la famille côte à côte.
        let wide = render_profile_page(&mut profile, 1240.0);
        let family = painted_text(&wide, lapinux_core::i18n::t("ui.profile_family"))
            .expect("encart famille dessiné");
        let bio = painted_text(&wide, lapinux_core::i18n::t("profile.member_since"))
            .expect("bio dessinée");
        assert!(
            family.left() > 1240.0 * 0.5,
            "famille à droite (x = {})",
            family.left()
        );
        assert!(
            bio.left() < 1240.0 * 0.5,
            "bio à gauche (x = {})",
            bio.left()
        );
        // La famille commence en haut de page, à côté de la bio : empilée,
        // elle n'apparaîtrait qu'après tout l'encart d'identité.
        assert!(
            family.top() < 120.0,
            "famille en haut de page (y = {})",
            family.top()
        );
    }

    /// Une image de la page sur un contexte qui dure : les clics s'enchaînent
    /// d'une image à l'autre, comme dans la fenêtre.
    fn page_frame(
        ctx: &egui::Context,
        profile: &mut Profile,
        events: Vec<egui::Event>,
    ) -> (egui::FullOutput, Vec<ProfileAction>) {
        let mut actions = Vec::new();
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1200.0, 1400.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.set_width(1100.0);
                    actions =
                        profile.page(ui, "Alex", &sample_platforms(), &mut |_| {}, &mut |_| {});
                });
            },
        );
        (output, actions)
    }

    fn press(pos: egui::Pos2) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            },
        ]
    }

    fn release(pos: egui::Pos2) -> Vec<egui::Event> {
        vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        }]
    }

    #[test]
    fn the_presence_status_is_a_dropdown_beside_the_name() {
        let ctx = egui::Context::default();
        crate::install_fonts(&ctx);
        crate::apply_theme(&ctx);
        let mut profile = sample_profile();
        let (output, _) = page_frame(&ctx, &mut profile, Vec::new());
        let header = painted_text_trimmed(&output, lapinux_core::i18n::t("profile.online"))
            .expect("menu d'état dessiné");
        // Le compte s'écrit aussi dans la liste des plateformes : c'est le nom
        // de la fiche qui compte ici, donc celui à droite de cette liste.
        let name = painted_text_in(&output, "Alex", PLATFORM_SIDE, f32::INFINITY)
            .expect("nom dessiné dans la fiche");
        assert!(
            header.left() > name.right(),
            "menu à droite du nom (x = {})",
            header.left()
        );
        assert!(
            header.center().y > name.top() && header.center().y < name.bottom(),
            "menu sur la ligne du nom (y = {})",
            header.center().y
        );
        // Le point vit dans le bouton, au bord gauche du libellé qu'il précède.
        assert!(
            painted_dots(&output)
                .iter()
                .any(|(center, radius)| *radius <= 4.0
                    && (center.y - header.center().y).abs() < 4.0
                    && center.x >= header.left() - 2.0
                    && center.x < header.left() + 24.0),
            "le point d'état est peint dans le menu"
        );
        assert!(painted_text(&output, "Statut Steam").is_none());
        // Plus d'étiquette de session à côté du menu.
        assert!(painted_text(&output, "· Lapinux").is_none());
        // Le nom réel, puis sa visibilité à sa droite, sur la ligne du dessous.
        let real = painted_text(&output, "<Alex>").expect("nom réel");
        let privacy = painted_text(&output, "· Public").expect("visibilité");
        assert!(
            privacy.left() > real.right(),
            "la visibilité suit le nom réel (x = {})",
            privacy.left()
        );
        assert!(
            privacy.top() > name.top() && real.top() > header.bottom(),
            "nom réel et visibilité sous le pseudo"
        );

        // Un clic ouvre la liste, un second choix publie la présence.
        page_frame(&ctx, &mut profile, press(header.center()));
        page_frame(&ctx, &mut profile, release(header.center()));
        // La liste s'ouvre à l'image suivante, une fois le clic enregistré.
        let (output, _) = page_frame(&ctx, &mut profile, Vec::new());
        let away = painted_text(&output, "Absent").expect("liste des états ouverte");
        page_frame(&ctx, &mut profile, press(away.center()));
        let (_, actions) = page_frame(&ctx, &mut profile, release(away.center()));
        assert_eq!(
            actions,
            vec![ProfileAction::SetPresence(SteamPresence::Away)]
        );
    }

    /// Pendant la lecture de la page, la fiche annonce ce qu'elle attend au
    /// lieu de laisser un vide.
    #[test]
    fn the_page_says_what_it_is_loading() {
        let ctx = egui::Context::default();
        crate::install_fonts(&ctx);
        crate::apply_theme(&ctx);
        let mut profile = Profile::new();
        profile.profile = Some(profile_from_xml(PROFILE_XML));
        let (output, _) = page_frame(&ctx, &mut profile, Vec::new());
        assert!(
            painted_text(&output, lapinux_core::i18n::t("profile.showcases_loading")).is_some(),
            "la carte des vitrines attend son tour"
        );
        assert!(
            painted_text(&output, lapinux_core::i18n::t("profile.comments_loading")).is_some(),
            "la carte des commentaires attend son tour"
        );
    }

    /// L'identifiant se cale au coin haut droit de la fiche, à la hauteur de
    /// la photo : ni bande vide au-dessus, ni chevauchement du nom.
    #[test]
    fn the_header_keeps_the_top_corner() {
        let mut profile = sample_profile();
        let output = render_profile_page(&mut profile, 1240.0);
        // L'identifiant se montre masqué : de quoi le reconnaître, pas de
        // quoi l'étaler.
        let id = painted_text(&output, "ID 765••••00").expect("identifiant masqué");
        assert!(
            painted_text(&output, "76561198000000000").is_none(),
            "l'identifiant complet reste hors de la page"
        );
        // Il partage la ligne du nom, pas une ligne au-dessus.
        let name = painted_text(&output, "Alex").expect("nom du compte");
        assert!(
            id.top() <= name.top() + 1.0,
            "l'identifiant reste à la hauteur du nom ({} vs {})",
            id.top(),
            name.top()
        );
        // Et il occupe bien le bord droit de la fiche.
        let facts = painted_text(&output, lapinux_core::i18n::t("profile.member_since"))
            .expect("fiche dessinée");
        assert!(
            id.left() > facts.center().x,
            "l'identifiant reste dans la moitié droite ({} vs {})",
            id.left(),
            facts.center().x
        );
    }

    /// La fiche se range en deux colonnes : identité puis commentaires à
    /// gauche, famille puis amis à droite.
    #[test]
    fn the_board_splits_identity_comments_from_family_friends() {
        let mut profile = sample_profile();
        let output = render_profile_page(&mut profile, 1240.0);
        let identity = painted_text(&output, lapinux_core::i18n::t("profile.member_since"))
            .expect("fiche dessinée");
        let comments = painted_text(&output, lapinux_core::i18n::t("ui.profile_comments"))
            .expect("commentaires dessinés");
        let family = painted_text(&output, lapinux_core::i18n::t("ui.profile_family"))
            .expect("famille dessinée");
        let friends =
            painted_text(&output, lapinux_core::i18n::t("friends.title")).expect("amis dessinés");
        assert!(
            (comments.left() - identity.left()).abs() < 2.0,
            "les commentaires restent dans la colonne de la fiche (x = {})",
            comments.left()
        );
        assert!(
            comments.top() > identity.bottom(),
            "les commentaires passent sous la fiche"
        );
        assert!(
            (friends.left() - family.left()).abs() < 2.0,
            "les amis partagent la colonne de la famille (x = {})",
            friends.left()
        );
        assert!(
            friends.top() > family.top(),
            "les amis passent sous la famille"
        );
        assert!(
            friends.left() > comments.left(),
            "les amis ne sont pas dans la colonne des commentaires"
        );
    }

    /// La mise en forme du HTML ne crée pas de lignes : seuls les `<br>` de
    /// Steam en font une, et une ligne vide de HTML ne prend pas de place.
    #[test]
    fn only_steam_line_breaks_make_lines() {
        let parts = parse_description("Fan d'horreur,\n   de zombies.\n<br>Deuxième ligne");
        assert_eq!(
            description_lines(&parts).len(),
            2,
            "deux lignes : {parts:?}"
        );
        // Une ligne faite d'espaces entre deux sauts ne s'affiche pas.
        let parts = parse_description("haut<br>   <br>bas");
        assert_eq!(
            description_lines(&parts).len(),
            2,
            "la ligne vide ne compte pas : {parts:?}"
        );
    }

    /// Une émoticône écrite en clair garde son image, et une heure ne devient
    /// pas une émoticône.
    #[test]
    fn written_emote_codes_become_images() {
        let parts = parse_description("Salut :health: toi");
        assert!(
            parts.iter().any(|part| matches!(
                part,
                DescriptionPart::Emote { name, url }
                    if name == "health" && url.ends_with("/economy/emoticon/health")
            )),
            "{parts:?}"
        );
        let parts = parse_description("Il est 12:30:45 ici");
        assert!(
            !parts
                .iter()
                .any(|part| matches!(part, DescriptionPart::Emote { .. })),
            "{parts:?}"
        );
        // Dans un lien, l'émoticône reste une image.
        let parts = parse_description(
            "<a href=\"https://store.example/1\"><img src=\"https://cdn.example/economy/emoticon/health\" alt=\":health:\"> Jeu</a>",
        );
        assert!(
            parts
                .iter()
                .any(|part| matches!(part, DescriptionPart::Emote { .. })),
            "{parts:?}"
        );
    }

    #[test]
    fn description_links_are_clickable() {
        let ctx = egui::Context::default();
        crate::install_fonts(&ctx);
        crate::apply_theme(&ctx);
        let profile = Profile::new();
        let parts = parse_description(
            "Voir <a href=\"https://steamcommunity.com/linkfilter/?u=https%3A%2F%2Fexample.com%2F\">le site</a>.",
        );
        let mut actions: Vec<ProfileAction> = Vec::new();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 200.0));
        let mut draw = |events: Vec<egui::Event>| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        profile.description(ui, &parts, &mut actions);
                    });
                },
            )
        };
        let output = draw(Vec::new());
        let link = painted_text(&output, "le site").expect("lien peint");
        draw(press(link.center()));
        draw(release(link.center()));
        // Le filtre de Steam est retiré : le navigateur ouvre la vraie adresse.
        assert_eq!(
            actions,
            vec![ProfileAction::Open("https://example.com/".to_owned())]
        );
    }

    /// Texte peint, cherché par morceau : les messages de la bannière portent
    /// un compte à rebours, on ne peut pas les comparer à l'identique.
    /// Tout le texte peint, concaténé : une phrase coupée en lignes reste
    /// lisible d'un bloc pour les assertions.
    fn all_text(output: &egui::FullOutput) -> String {
        fn walk(shape: &egui::Shape, out: &mut String) {
            match shape {
                egui::Shape::Text(text) => {
                    out.push_str(text.galley.text());
                    out.push(' ');
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = String::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut out);
        }
        out
    }

    fn any_text(output: &egui::FullOutput, needle: &str) -> bool {
        fn walk(shape: &egui::Shape, needle: &str) -> bool {
            match shape {
                egui::Shape::Text(text) => text.galley.text().contains(needle),
                egui::Shape::Vec(shapes) => shapes.iter().any(|shape| walk(shape, needle)),
                _ => false,
            }
        }
        output
            .shapes
            .iter()
            .any(|clipped| walk(&clipped.shape, needle))
    }

    /// Steam répond 429 quand ses pages communautaires sont appelées en rafale :
    /// la fiche doit revenir d'elle-même, et le dire, plutôt que d'afficher une
    /// panne de configuration.
    #[test]
    fn a_rate_limited_profile_comes_back_on_its_own() {
        let ctx = egui::Context::default();
        crate::install_fonts(&ctx);
        let mut profile = Profile::new();
        profile.loaded_for = Some(7_656_119_800_000_000_2);
        let (tx, rx) = mpsc::channel();
        profile.rx = Some(rx);
        tx.send(ProfileMsg::Failed(FetchError {
            message: "Steam limite les requêtes sur ce profil (réponse 429)".into(),
            rate_limited: true,
        }))
        .expect("fil de lecture ouvert");
        profile.poll(&ctx);
        assert!(!profile.loading, "la lecture est terminée");
        assert!(profile.rate_limited, "la cause est retenue");
        assert_eq!(profile.retries, 1, "un essai automatique est compté");
        let at = profile.retry_at.expect("un nouvel essai est prévu");
        assert!(
            at > Instant::now() + RELOAD_DELAY - Duration::from_secs(5),
            "l'essai laisse la limite se refermer"
        );
        // L'image suivante ne consomme pas l'essai avant l'heure.
        profile.poll(&ctx);
        assert!(profile.retry_at.is_some(), "l'essai attend son tour");
        let output = render_profile_page(&mut profile, 680.0);
        assert!(
            painted_text(&output, lapinux_core::i18n::t("configurations.retry")).is_some(),
            "la bannière laisse relancer tout de suite"
        );
        assert!(
            any_text(
                &output,
                lapinux_core::i18n::t("profile.rate_limited")
                    .split(';')
                    .next()
                    .unwrap(),
            ),
            "la bannière nomme la limite de débit et annonce l'essai"
        );
    }

    /// Une vraie panne, elle, ne promet rien : l'aide sur le compte reste.
    #[test]
    fn a_broken_page_does_not_promise_a_retry() {
        let ctx = egui::Context::default();
        crate::install_fonts(&ctx);
        let mut profile = Profile::new();
        profile.loaded_for = Some(7_656_119_800_000_000_2);
        let (tx, rx) = mpsc::channel();
        profile.rx = Some(rx);
        tx.send(ProfileMsg::Failed(FetchError::new(
            "Page injoignable : erreur réseau",
        )))
        .expect("fil de lecture ouvert");
        profile.poll(&ctx);
        assert!(!profile.rate_limited);
        assert!(profile.retry_at.is_none(), "aucun essai automatique");
        let output = render_profile_page(&mut profile, 680.0);
        let painted = all_text(&output)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let expected = lapinux_core::i18n::t("ui.profile_steamid_required")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            painted.contains(&expected),
            "une panne de réseau garde l'aide sur le compte : {painted}"
        );
    }

    /// Deux pages communautaires ne partent pas ensemble : le tour de parole les
    /// espace, et un refus de Steam met tout le monde en pause.
    #[test]
    fn community_requests_wait_their_turn() {
        hold_community_requests(Duration::from_millis(60));
        let started = Instant::now();
        wait_for_community_turn();
        assert!(
            started.elapsed() >= Duration::from_millis(40),
            "la requête suivante patiente après un refus"
        );
    }

    /// Un 429 de Steam est une limite de débit, pas une panne : c'est ce qui
    /// déclenche la reprise de la fiche. Vérifié contre un vrai serveur, qui
    /// répond exactement ce que répond Steam.
    #[test]
    fn an_http_429_is_read_as_a_rate_limit() {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("port local");
        let address = listener.local_addr().expect("adresse locale");
        let server = std::thread::spawn(move || {
            if let Ok((mut socket, _)) = listener.accept() {
                let _ = socket.write_all(
                    b"HTTP/1.1 429 Too Many Requests\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            }
        });
        let error = get_text(&format!("http://{address}/")).expect_err("un 429 refuse la page");
        assert!(error.rate_limited, "429 = limite de débit, pas une panne");
        server.join().expect("serveur local");
    }
}
