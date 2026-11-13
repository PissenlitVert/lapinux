//! Modèles de la partie sociale Steam : présence, amis, notifications,
//! famille et commentaires.
//!
//! Ils vivent dans le noyau parce que le daemon les produit (protocole du
//! client Steam) et que l'interface les affiche ; `lapinux-core` ne connaît
//! ni le réseau ni le protocole, seulement la forme des données.

use crate::stores::Store;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// État de présence publié sur le réseau Steam.
///
/// L'ordre des valeurs est celui du protocole (`EPersonaState`) : une
/// correspondance directe avec les octets envoyés au CM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SteamPresence {
    Offline,
    Online,
    Busy,
    Away,
    Snooze,
    Trade,
    Play,
    Invisible,
}

impl Default for SteamPresence {
    /// Préférence de départ d'un compte : en ligne. Un ami sans état connu
    /// n'est jamais décrit par ce défaut, il reçoit son état explicite.
    fn default() -> Self {
        SteamPresence::Online
    }
}

impl SteamPresence {
    /// Valeur `EPersonaState` envoyée au CM.
    pub fn code(self) -> u32 {
        match self {
            SteamPresence::Offline => 0,
            SteamPresence::Online => 1,
            SteamPresence::Busy => 2,
            SteamPresence::Away => 3,
            SteamPresence::Snooze => 4,
            SteamPresence::Trade => 5,
            SteamPresence::Play => 6,
            SteamPresence::Invisible => 7,
        }
    }

    /// Lecture de la valeur réseau ; un code inconnu retombe hors ligne.
    pub fn from_code(code: u32) -> Self {
        match code {
            1 => SteamPresence::Online,
            2 => SteamPresence::Busy,
            3 => SteamPresence::Away,
            4 => SteamPresence::Snooze,
            5 => SteamPresence::Trade,
            6 => SteamPresence::Play,
            7 => SteamPresence::Invisible,
            _ => SteamPresence::Offline,
        }
    }

    /// Libellé court, dans la langue de l'interface.
    pub fn label(self) -> &'static str {
        match self {
            SteamPresence::Offline => crate::i18n::t("profile.offline"),
            SteamPresence::Online => crate::i18n::t("profile.online"),
            SteamPresence::Busy => "Occupé",
            SteamPresence::Away => "Absent",
            SteamPresence::Snooze => "Absent (répétition)",
            SteamPresence::Trade => "Échange",
            SteamPresence::Play => "Prêt à jouer",
            SteamPresence::Invisible => "Invisible",
        }
    }

    /// Vrai quand le compte est visible comme disponible par ses amis.
    pub fn online(self) -> bool {
        !matches!(self, SteamPresence::Offline | SteamPresence::Invisible)
    }

    /// Choix proposés dans l'interface : les quatre états qu'un joueur
    /// attend d'un lanceur, sans les états d'échange.
    pub fn choices() -> [SteamPresence; 4] {
        [
            SteamPresence::Online,
            SteamPresence::Away,
            SteamPresence::Invisible,
            SteamPresence::Offline,
        ]
    }
}

/// État d'un ami, tel que le CM le pousse (amis en ligne) ou le renvoie après
/// une demande explicite (amis hors ligne).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SteamFriend {
    pub steam_id: u64,
    pub name: String,
    pub presence: SteamPresence,
    /// Nom du jeu en cours, publié par Steam (riche présence).
    pub game: String,
    /// Appid du jeu en cours, 0 sinon.
    pub game_appid: u32,
    /// Dernière connexion, en secondes Unix (0 = inconnu).
    pub last_logon: i64,
    /// Dernière déconnexion, en secondes Unix (0 = inconnu).
    pub last_logoff: i64,
    /// URL d'avatar, vide si Steam n'en a pas donné.
    pub avatar: String,
}

impl Default for SteamFriend {
    /// Un ami fraîchement listé n'a pas encore livré son état : il part hors
    /// ligne. Le décrire « en ligne » rangeait toute la liste dans les amis
    /// connectés tant que Steam n'avait rien publié.
    fn default() -> Self {
        Self {
            steam_id: 0,
            name: String::new(),
            presence: SteamPresence::Offline,
            game: String::new(),
            game_appid: 0,
            last_logon: 0,
            last_logoff: 0,
            avatar: String::new(),
        }
    }
}

impl SteamFriend {
    /// Vrai pour un ami dont Steam dit qu'il joue actuellement.
    pub fn in_game(&self) -> bool {
        self.game_appid != 0 || !self.game.is_empty()
    }

    /// Ligne d'état : jeu en cours, sinon présence.
    pub fn state_line(&self) -> String {
        if self.game.trim().is_empty() {
            self.presence.label().to_string()
        } else {
            self.game.clone()
        }
    }
}

/// Type d'activité observée chez un ami Steam.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SteamFriendEventKind {
    #[default]
    Online,
    Offline,
    GameStarted,
    GameStopped,
}

/// Transition récente d'un ami : connexion, déconnexion, jeu lancé ou quitté.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SteamFriendEvent {
    pub kind: SteamFriendEventKind,
    pub steam_id: u64,
    pub name: String,
    /// Avatar de l'ami au moment de l'événement, vide si Steam ne l'a pas
    /// fourni. Les anciennes entrées n'en ont pas encore.
    pub avatar: String,
    /// Jeu concerné, vide pour une connexion ou une déconnexion.
    pub game: String,
    pub appid: u32,
    /// Heure de l'événement, en secondes Unix.
    pub at: i64,
}

/// Notification Steam poussée par le CM (nouveaux objets, cadeaux, etc.).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SteamNotification {
    /// Code `EClientNotificationType` ; le libellé est dérivé à l'affichage.
    pub kind: u32,
    pub count: u32,
    /// Heure de réception ou du dernier changement du compteur, en secondes
    /// Unix (0 = inconnue). Le CM ne publie pas d'horodatage avec le compteur.
    #[serde(default)]
    pub at: i64,
}

impl SteamNotification {
    pub fn label(&self) -> &'static str {
        match self.kind {
            1 => "Objets",
            2 => "Cadeaux",
            3 => "Messages hors ligne",
            4 => "Demandes d'ami",
            5 => "Échanges",
            _ => "Notifications",
        }
    }
}

/// Jeu en promotion cité par une notification de liste de souhaits.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SteamWishlistDeal {
    pub appid: u32,
    pub name: String,
    /// Prix final déjà formaté par Steam, vide quand il est inconnu.
    pub price: String,
    /// Remise en pourcentage, 0 sans promotion.
    pub discount: u32,
}

/// Notification lue sur le service Web Steam. Contrairement aux compteurs
/// poussés par le CM, elle porte le texte et l'adresse de l'événement :
/// wishlist, promotion, échange, préchargement, tour de jeu, etc.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SteamWebNotification {
    /// Code `ESteamNotificationType` du service Web.
    pub kind: u32,
    pub title: String,
    pub body: String,
    /// Adresse à ouvrir, vide quand Steam n'en donne pas.
    pub url: String,
    /// Appids cités par la notification (wishlist), pour résoudre les noms.
    #[serde(default)]
    pub appids: Vec<u32>,
    /// Détails des jeux wishlist : nom, prix et remise pour l'affichage.
    #[serde(default)]
    pub deals: Vec<SteamWishlistDeal>,
    /// Heure de publication, en secondes Unix (0 = inconnue).
    pub at: i64,
}

impl SteamWebNotification {
    /// Libellé court du type, dans la langue de l'interface.
    pub fn label(&self) -> &'static str {
        match self.kind {
            2 => "Cadeau",
            3 => "Commentaire",
            4 => "Objet",
            5 => "Demande d'ami",
            6 => "Grande promotion",
            7 => "Préchargement",
            8 => "Wishlist",
            9 => "Échange",
            10 => "Annonce",
            11 => "Aide",
            12 => "Tour de jeu",
            13 => "Message",
            14 => "Modération",
            16 => "Famille",
            22 => "Jeu ajouté",
            28 => "Playtest",
            29 => "Échange annulé",
            30 => "Modération",
            _ => "Notification",
        }
    }
}

/// Membre de la famille Steam.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SteamFamilyMember {
    pub steam_id: u64,
    pub name: String,
    /// 1 = adulte, 2 = enfant, tel que Steam le publie.
    pub role: u32,
    /// Vrai pour le compte connecté.
    pub is_self: bool,
    /// URL de la photo de profil, vide tant qu'elle n'est pas connue.
    pub avatar: String,
    /// Nombre de jeux que ce membre apporte à la bibliothèque partagée.
    #[serde(default)]
    pub games: u32,
    /// Appids de ces jeux, tels que Steam les publie pour ce membre. Ils
    /// permettent de nommer la liste partagée sans relancer de requête.
    #[serde(default)]
    pub appids: Vec<u32>,
}

/// Groupe familial Steam, avec ses membres nommés quand Steam les publie.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SteamFamily {
    pub group_id: u64,
    pub name: String,
    pub members: Vec<SteamFamilyMember>,
}

/// Commentaire publié sur le profil public.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SteamComment {
    pub author: String,
    pub author_id: u64,
    /// Date de publication, telle que Steam l'affiche (déjà localisée).
    pub posted: String,
    pub text: String,
}

/// Tout ce qu'une session sociale rapporte, publié en bloc à l'interface.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SteamSocial {
    /// "idle", "connecting", "live" ou "error".
    pub state: String,
    /// Dernière erreur, si la session n'est pas en vie.
    pub error: Option<String>,
    pub persona_name: String,
    pub steam_id: u64,
    /// Présence que Lapinux publie.
    pub presence: SteamPresence,
    /// Présence que Steam applique réellement (renvoyée par le CM).
    pub applied_presence: SteamPresence,
    pub friends: Vec<SteamFriend>,
    pub notifications: Vec<SteamNotification>,
    /// Notifications du service Web Steam (wishlist, promotions, échanges,
    /// préchargements…), avec leur texte, en plus des compteurs du CM.
    #[serde(default)]
    pub web_notifications: Vec<SteamWebNotification>,
    /// Activité récente des amis, de la plus récente à la plus ancienne.
    #[serde(default)]
    pub friend_events: Vec<SteamFriendEvent>,
    /// Nouveaux commentaires sur le profil, comptés par Steam.
    pub new_comments: u32,
    pub family: Option<SteamFamily>,
    /// Messages par ami : ceux de la session et l'historique récent demandé
    /// à l'ouverture d'une conversation.
    pub chats: BTreeMap<u64, Vec<SteamChatMessage>>,
    /// État de chargement et pagination de chaque historique Steam.
    #[serde(default)]
    pub history: BTreeMap<u64, SteamChatHistoryState>,
    /// Emotes possédées par le compte connecté, telles que
    /// `Player.GetEmoticonList` les rend.
    #[serde(default)]
    pub emoticons: Vec<SteamEmoticon>,
    /// Autocollants possédés, publiés par `CMsgClientEmoticonList`.
    #[serde(default)]
    pub stickers: Vec<SteamSticker>,
    /// Dernière activation de clé demandée depuis l'interface.
    #[serde(default)]
    pub redeem: Option<SteamKeyRedeem>,
    /// Invitations de lobby reçues pendant la session. `connect` vient de
    /// l'adresse enregistrée à la création du lobby ; sans elle, le bouton
    /// « Rejoindre » reste désactivé.
    #[serde(default)]
    pub invites: Vec<SteamLobbyInvite>,
    /// Dernière mise à jour, en secondes Unix.
    pub updated: i64,
}

/// Invitation de lobby reçue d'un ami (message CM 6621).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SteamLobbyInvite {
    pub appid: u32,
    pub lobby: u64,
    /// Ami qui invite.
    pub inviter: u64,
    /// Nom de l'ami, résolu par le daemon quand il est connu.
    pub inviter_name: Option<String>,
    /// Adresse `ip:port` de la partie, si le lobby en a une.
    pub connect: Option<String>,
}

/// Activation d'une clé produit Steam : l'état, la phrase à lire et les jeux
/// que la clé a apportés quand Steam les nomme.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteamKeyRedeem {
    /// « running », « ok » ou « error ».
    pub state: String,
    pub message: String,
    /// Noms de jeux lus dans le reçu Steam, quand il les donne.
    #[serde(default)]
    pub games: Vec<String>,
    /// Appids des jeux apportés par la clé, lus dans le même reçu.
    #[serde(default)]
    pub appids: Vec<u32>,
}

impl SteamSocial {
    pub fn connecting() -> Self {
        Self {
            state: "connecting".into(),
            ..Self::default()
        }
    }

    pub fn failed(message: impl Into<String>) -> Self {
        Self {
            state: "error".into(),
            error: Some(message.into()),
            ..Self::default()
        }
    }

    /// Vrai quand le CM est connecté et que la liste d'amis est arrivée.
    pub fn live(&self) -> bool {
        self.state == "live"
    }

    /// Amis en jeu ou en ligne, du plus récent au plus ancien.
    pub fn online_friends(&self) -> Vec<&SteamFriend> {
        let mut friends: Vec<&SteamFriend> = self
            .friends
            .iter()
            .filter(|friend| friend.presence.online())
            .collect();
        friends.sort_by(|a, b| {
            b.in_game()
                .cmp(&a.in_game())
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        friends
    }

    /// Amis hors ligne (ou invisibles), triés par dernière connexion.
    pub fn offline_friends(&self) -> Vec<&SteamFriend> {
        let mut friends: Vec<&SteamFriend> = self
            .friends
            .iter()
            .filter(|friend| !friend.presence.online())
            .collect();
        friends.sort_by(|a, b| {
            b.last_logon
                .cmp(&a.last_logon)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        friends
    }

    /// Nombre total de notifications Steam en attente. Les commentaires du
    /// profil ne sont pas des notifications du lanceur : ils ne comptent pas
    /// dans la pastille de la cloche. Les notifications du service Web, elles,
    /// sont déjà filtrées sur les événements non lus.
    pub fn badge_count(&self) -> u32 {
        self.notifications
            .iter()
            .map(|notification| notification.count)
            .sum::<u32>()
            + self.web_notifications.len() as u32
    }

    /// Pastille filtrée selon les catégories activées dans les réglages et
    /// la durée de conservation choisie.
    pub fn badge_count_filtered(
        &self,
        steam_other: bool,
        wishlist_promotions: bool,
        since: i64,
    ) -> u32 {
        let counters = if steam_other {
            self.notifications
                .iter()
                .filter(|notification| notification.at <= 0 || notification.at >= since)
                .map(|notification| notification.count)
                .sum::<u32>()
        } else {
            0
        };
        let web = self
            .web_notifications
            .iter()
            .filter(|notification| notification.at <= 0 || notification.at >= since)
            .filter(|notification| {
                if notification.kind == 8 {
                    wishlist_promotions
                } else {
                    steam_other
                }
            })
            .count() as u32;
        counters + web
    }
}

/// Un message échangé avec un ami Steam.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SteamChatMessage {
    /// Auteur du message : le compte connecté ou l'ami.
    pub from: u64,
    pub text: String,
    /// Heure du message, en secondes Unix ; 0 quand Steam ne la donne pas.
    pub at: i64,
    /// Vrai pour un message écrit depuis Lapinux.
    pub outgoing: bool,
    /// Vrai pour un message reçu en direct pendant cette session, plutôt que
    /// relu dans un historique Steam. Seuls ceux-là peuvent déclencher une
    /// notification : ouvrir une vieille conversation ne doit pas faire
    /// apparaître une rafale de toasts.
    #[serde(default)]
    pub received_live: bool,
}

/// Une emote Steam possédée par le compte connecté.
///
/// Steam l'identifie par son nom, celui des shortcodes `:nom:` du chat ;
/// l'image vient du CDN communautaire, sans jeton.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SteamEmoticon {
    pub name: String,
    /// Nombre d'exemplaires possédés.
    pub count: u32,
    /// Appid du jeu qui l'apporte, 0 quand Steam ne le dit pas.
    pub appid: u32,
}

/// Adresse CDN de l'image d'une emote, à partir de son nom de shortcode.
/// Le nom est nu : Steam le rend parfois entouré de deux-points. La variante
/// `emoticonlarge` garde une image nette quand le chat l'agrandit.
pub fn steam_emoticon_url(name: &str) -> String {
    format!(
        "https://steamcommunity-a.akamaihd.net/economy/emoticonlarge/{}",
        name.trim_matches(':')
    )
}

impl SteamEmoticon {
    /// Adresse CDN de l'image de l'emote.
    pub fn url(&self) -> String {
        steam_emoticon_url(&self.name)
    }
}

/// Un autocollant Steam possédé par le compte connecté.
///
/// Steam l'envoie avec `/sticker nom` et le rend dans le chat sous la forme
/// `[sticker type="nom"][/sticker]`. L'image vient du CDN communautaire.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SteamSticker {
    pub name: String,
    /// Nombre d'exemplaires possédés.
    pub count: u32,
    /// Appid du jeu qui l'apporte, 0 quand Steam ne le dit pas.
    pub appid: u32,
}

/// Échappe un segment de chemin : les noms d'autocollants Steam contiennent
/// des espaces et parfois des caractères non latins (`Three Piece`,
/// `霞露零贴纸`). Sans cet encodage, l'URL est refusée par le CDN.
fn encode_path_segment(value: &str) -> String {
    use std::fmt::Write;
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

/// Adresse CDN de l'image d'un autocollant, à partir de son nom Steam.
pub fn steam_sticker_url(name: &str) -> String {
    format!(
        "https://steamcommunity-a.akamaihd.net/economy/sticker/{}",
        encode_path_segment(name.trim_matches(':'))
    )
}

impl SteamSticker {
    /// Adresse CDN de l'image de l'autocollant.
    pub fn url(&self) -> String {
        steam_sticker_url(&self.name)
    }
}

/// État de la lecture de l'historique d'une conversation Steam.
///
/// Steam ne rend qu'une fenêtre de messages récents ; `more_available` dit
/// s'il reste des messages avant la page déjà chargée. Le curseur interne
/// n'est pas envoyé à l'interface : il sert seulement à demander la page
/// suivante au worker social.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SteamChatHistoryState {
    /// Une requête est en cours pour cette conversation.
    pub loading: bool,
    /// Steam a indiqué d'autres messages avant ceux déjà chargés.
    pub more_available: bool,
    /// Dernière erreur, quand Steam n'a pas rendu l'historique.
    pub error: Option<String>,
    /// Curseur `time_last`/`ordinal_last` de la page suivante.
    #[serde(skip)]
    pub cursor: Option<(u32, u32)>,
}

impl SteamChatHistoryState {
    /// Vrai quand la dernière page est arrivée sans erreur et que Steam
    /// n'annonce plus de page précédente. C'est alors seulement que l'écran
    /// peut annoncer le début de l'historique disponible.
    pub fn complete(&self) -> bool {
        !self.loading && !self.more_available && self.error.is_none()
    }
}

/// État de la fenêtre d'amis : liste, avatars et discussions.
///
/// Les messages sont ceux de la session et l'historique récent demandé à
/// l'ouverture d'une conversation. Steam ne fournit pas d'historique complet.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SteamFriendsState {
    /// Vrai quand la session sociale est connectée.
    pub live: bool,
    /// Dernière erreur de la session, si elle n'est pas en vie.
    pub error: Option<String>,
    /// Amis proposés, jeux en cours et joueurs en ligne d'abord.
    pub friends: Vec<SteamFriend>,
    /// Avatars déjà téléchargés par le daemon, indexés par SteamID.
    /// L'overlay lit ces fichiers locaux : il ne contacte jamais le réseau.
    pub avatars: BTreeMap<u64, PathBuf>,
    /// Messages par ami : ceux de la session et l'historique récent demandé
    /// à l'ouverture d'une conversation.
    pub chats: BTreeMap<u64, Vec<SteamChatMessage>>,
    /// Dernier lobby Steam créé par le jeu, quand il y en a un. L'overlay
    /// n'affiche le bouton « Inviter » que dans ce cas.
    pub lobby: Option<u64>,
}

/// Un ami d'une boutique hors Steam.
///
/// Epic Games et GOG publient chacun la liste d'amis de leur compte ; leurs
/// formes diffèrent, ce type les réunit pour l'interface. La présence vient
/// d'un second service quand la boutique en a un : `status` reste vide quand
/// elle n'a rien pu dire, au lieu de faire passer l'ami pour absent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StoreFriend {
    pub store: Store,
    /// Identifiant du compte chez la boutique.
    pub id: String,
    /// Nom publié par la boutique.
    pub name: String,
    /// Nom d'amitié choisi par le joueur, quand la boutique le publie.
    pub alias: String,
    /// Adresse de la photo de profil, vide si la boutique n'en donne pas.
    pub avatar: String,
    /// Jeu en cours, quand la boutique le publie.
    pub game: String,
    /// `online`, `away`, `in-game`, `offline` ou vide quand la présence n'est
    /// pas publiée.
    pub status: String,
    /// Dernière connexion, en secondes Unix (0 = inconnue).
    pub last_seen: i64,
}

impl StoreFriend {
    /// Vrai quand la boutique voit l'ami connecté, même absent.
    pub fn online(&self) -> bool {
        matches!(self.status.as_str(), "online" | "in-game" | "away")
    }

    /// Ligne d'état : le jeu en cours prime sur la présence, et la présence
    /// est traduite plutôt que d'afficher le code du service.
    pub fn state_line(&self) -> String {
        if !self.game.trim().is_empty() {
            return self.game.clone();
        }
        match self.status.as_str() {
            "in-game" => "En jeu".into(),
            "online" => crate::i18n::t("profile.online").into(),
            "away" => crate::i18n::t("profile.away").into(),
            "offline" => crate::i18n::t("profile.offline").into(),
            _ => String::new(),
        }
    }
}

/// Ce qu'une boutique a répondu pour sa liste d'amis.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StoreFriends {
    pub store: Store,
    pub connected: bool,
    pub friends: Vec<StoreFriend>,
    /// Ce qu'il faut lire quand la liste manque : compte non branché,
    /// boutique sans liste d'amis, ou refus de son service.
    pub note: String,
}

/// Réponse d'une demande d'ami reçue (nom résolu par le daemon).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SteamFriendRequest {
    pub steam_id: u64,
    pub name: String,
}

/// Erreurs de la partie sociale, indépendantes du transport.
#[derive(Debug, thiserror::Error)]
pub enum SocialError {
    #[error("aucune session Steam enregistrée")]
    NoSession,
    #[error("session sociale déjà arrêtée")]
    Stopped,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presence_codes_follow_the_protocol() {
        assert_eq!(SteamPresence::Online.code(), 1);
        assert_eq!(SteamPresence::Away.code(), 3);
        assert_eq!(SteamPresence::Invisible.code(), 7);
        assert_eq!(SteamPresence::from_code(7), SteamPresence::Invisible);
        assert_eq!(SteamPresence::from_code(99), SteamPresence::Offline);
        assert!(SteamPresence::Online.online());
        assert!(!SteamPresence::Invisible.online());
        assert!(!SteamPresence::Offline.online());
    }

    #[test]
    fn sticker_url_encodes_spaces_and_unicode() {
        assert_eq!(
            steam_sticker_url("Three Piece"),
            "https://steamcommunity-a.akamaihd.net/economy/sticker/Three%20Piece"
        );
        assert_eq!(
            steam_sticker_url("霞露零贴纸"),
            "https://steamcommunity-a.akamaihd.net/economy/sticker/%E9%9C%9E%E9%9C%B2%E9%9B%B6%E8%B4%B4%E7%BA%B8"
        );
    }

    #[test]
    fn choices_cover_the_four_expected_states() {
        let choices = SteamPresence::choices();
        assert_eq!(choices.len(), 4);
        assert!(choices.contains(&SteamPresence::Online));
        assert!(choices.contains(&SteamPresence::Away));
        assert!(choices.contains(&SteamPresence::Invisible));
        assert!(choices.contains(&SteamPresence::Offline));
    }

    #[test]
    fn friends_split_between_online_and_offline() {
        let social = SteamSocial {
            friends: vec![
                SteamFriend {
                    name: "Alice".into(),
                    presence: SteamPresence::Online,
                    ..SteamFriend::default()
                },
                SteamFriend {
                    name: "Bob".into(),
                    presence: SteamPresence::Offline,
                    last_logon: 10,
                    ..SteamFriend::default()
                },
                SteamFriend {
                    name: "Carol".into(),
                    presence: SteamPresence::Online,
                    game: "Portal 2".into(),
                    game_appid: 620,
                    ..SteamFriend::default()
                },
            ],
            ..SteamSocial::default()
        };
        let online = social.online_friends();
        assert_eq!(online.len(), 2);
        // Celui qui joue passe devant.
        assert_eq!(online[0].name, "Carol");
        assert!(online[0].in_game());
        assert_eq!(social.offline_friends().len(), 1);
        assert_eq!(social.offline_friends()[0].name, "Bob");
    }

    #[test]
    fn friends_state_defaults_to_empty() {
        let state = SteamFriendsState::default();
        assert!(!state.live);
        assert!(state.friends.is_empty());
        assert!(state.avatars.is_empty());
        assert!(state.chats.is_empty());
    }

    #[test]
    fn store_friends_speak_french_and_count_an_away_friend_as_connected() {
        // La langue active est globale : sans ce verrou, un test i18n qui la
        // bascule en anglais peut traverser cette assertion.
        let _language = crate::i18n::lock_language();
        let online = StoreFriend {
            status: "online".into(),
            ..StoreFriend::default()
        };
        assert!(online.online());
        assert_eq!(online.state_line(), crate::i18n::t("profile.online"));

        let playing = StoreFriend {
            status: "in-game".into(),
            ..StoreFriend::default()
        };
        assert!(playing.online());
        assert_eq!(playing.state_line(), crate::i18n::t("profile.in_game"));

        let away = StoreFriend {
            status: "away".into(),
            ..StoreFriend::default()
        };
        assert!(away.online());
        assert_eq!(away.state_line(), crate::i18n::t("profile.away"));

        let offline = StoreFriend {
            status: "offline".into(),
            ..StoreFriend::default()
        };
        assert!(!offline.online());
        assert_eq!(offline.state_line(), crate::i18n::t("profile.offline"));

        // Une boutique muette sur la présence ne dit rien, pas « hors ligne ».
        assert_eq!(StoreFriend::default().state_line(), "");

        let game = StoreFriend {
            game: "Fortnite".into(),
            status: "in-game".into(),
            ..StoreFriend::default()
        };
        assert_eq!(game.state_line(), "Fortnite");
    }

    #[test]
    fn only_steam_notifications_count_in_the_badge() {
        let social = SteamSocial {
            notifications: vec![
                SteamNotification {
                    kind: 2,
                    count: 3,
                    at: 0,
                },
                SteamNotification {
                    kind: 5,
                    count: 1,
                    at: 0,
                },
            ],
            new_comments: 4,
            ..SteamSocial::default()
        };
        assert_eq!(social.badge_count(), 4);
        assert_eq!(social.notifications[0].label(), "Cadeaux");
        assert_eq!(social.notifications[1].label(), "Échanges");
    }

    #[test]
    fn web_notifications_add_their_content_to_the_badge() {
        let social = SteamSocial {
            web_notifications: vec![SteamWebNotification {
                kind: 8,
                title: "Un jeu".into(),
                body: "en promotion".into(),
                ..SteamWebNotification::default()
            }],
            ..SteamSocial::default()
        };
        assert_eq!(social.badge_count(), 1);
        assert_eq!(social.badge_count_filtered(true, false, 0), 0);
        assert_eq!(social.badge_count_filtered(false, true, 0), 1);
        assert_eq!(social.web_notifications[0].label(), "Wishlist");
    }
}
