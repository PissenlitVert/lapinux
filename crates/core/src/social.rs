//! Modèles de la partie sociale Steam : présence, amis, notifications,
//! famille et commentaires.
//!
//! Ils vivent dans le noyau parce que le daemon les produit (protocole du
//! client Steam) et que l'interface les affiche ; `lapinux-core` ne connaît
//! ni le réseau ni le protocole, seulement la forme des données.

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
            SteamPresence::Offline => "Hors ligne",
            SteamPresence::Online => "En ligne",
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

/// Notification Steam poussée par le CM (nouveaux objets, cadeaux, etc.).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SteamNotification {
    /// Code `EClientNotificationType` ; le libellé est dérivé à l'affichage.
    pub kind: u32,
    pub count: u32,
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
    /// Nouveaux commentaires sur le profil, comptés par Steam.
    pub new_comments: u32,
    pub family: Option<SteamFamily>,
    /// Messages échangés depuis l'ouverture de la session, par ami.
    pub chats: BTreeMap<u64, Vec<SteamChatMessage>>,
    /// Dernière activation de clé demandée depuis l'interface.
    #[serde(default)]
    pub redeem: Option<SteamKeyRedeem>,
    /// Dernière mise à jour, en secondes Unix.
    pub updated: i64,
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

    /// Nombre total de notifications en attente, commentaires compris.
    pub fn badge_count(&self) -> u32 {
        self.notifications
            .iter()
            .map(|notification| notification.count)
            .sum::<u32>()
            + self.new_comments
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
}

/// État de la fenêtre d'amis de l'overlay : liste, avatars et discussions.
///
/// Les messages sont ceux de la session courante. Steam ne fournit pas
/// d'historique complet sans une requête dédiée ; cette fenêtre sert d'abord
/// à lire les nouveaux messages et à répondre.
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
    /// Messages échangés depuis l'ouverture de la session, par ami.
    pub chats: BTreeMap<u64, Vec<SteamChatMessage>>,
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
    fn notifications_and_comments_share_one_badge() {
        let social = SteamSocial {
            notifications: vec![
                SteamNotification { kind: 2, count: 3 },
                SteamNotification { kind: 5, count: 1 },
            ],
            new_comments: 4,
            ..SteamSocial::default()
        };
        assert_eq!(social.badge_count(), 8);
        assert_eq!(social.notifications[0].label(), "Cadeaux");
        assert_eq!(social.notifications[1].label(), "Échanges");
    }

}
