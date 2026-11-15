//! Session sociale Steam : une connexion CM permanente qui publie notre
//! présence, reçoit la liste d'amis, les notifications, et garde le jeton de
//! session en vie.
//!
//! Le catalogue et les téléchargements ouvrent des connexions courtes ; celle-ci
//! reste ouverte tant que l'application tourne, comme le client de Valve. C'est
//! elle qui rend le compte visible (« en ligne », « absent », « invisible ») et
//! qui apporte ce que les pages publiques ne montrent pas : les amis en ligne,
//! leurs jeux, la famille Steam et les notifications.
//!
//! L'entretien du jeton passe par `Authentication.GenerateAccessTokenForApp` :
//! Steam rend un jeton de refresh renouvelé, écrit aussitôt dans le fichier de
//! session — la session n'a donc plus à être refaite à la main.

use crate::cm::proto;
use crate::session::SavedSession;
use lapinux_core::social::{
    SteamChatHistoryState, SteamChatMessage, SteamEmoticon, SteamFamily, SteamFamilyMember,
    SteamFriend, SteamFriendEvent, SteamFriendEventKind, SteamKeyRedeem, SteamNotification,
    SteamPresence, SteamSocial, SteamSticker, SteamWebNotification, SteamWishlistDeal,
};
use lapinux_core::util::now_unix;
use prost::Message;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use steamroom::client::msg::ClientMsg;
use steamroom::client::multi;
use steamroom::client::IncomingMsg;
use steamroom::client::{LoggedIn, SteamClient};
use steamroom::generated::c_msg_client_games_played::GamePlayed;
use steamroom::generated::c_msg_client_persona_state::Friend as PersonaFriend;
use steamroom::generated::{
    CMsgClientAccountInfo, CMsgClientChangeStatus, CMsgClientCommentNotifications,
    CMsgClientFriendsList, CMsgClientGamesPlayed, CMsgClientPersonaState,
    CMsgClientRequestCommentNotifications, CMsgClientRequestFriendData,
    CMsgClientUserNotifications, CMsgProtoBufHeader,
};
use steamroom::messages::header::PacketHeader;
use steamroom::messages::EMsg;
use steamroom_client::login::LoginBuilder;

/// `k_EMsgClientRegisterKey` : activation d'une clé produit.
const CLIENT_REGISTER_KEY: u32 = 743;
/// `k_EMsgClientPurchaseResponse` : verdict de l'activation.
const CLIENT_PURCHASE_RESPONSE: u32 = 763;
/// `k_EMsgClientAccountInfo` : notre pseudo, publié après le logon.
const CLIENT_ACCOUNT_INFO: u32 = 768;
/// `k_EMsgClientChangeStatus` : publie notre présence.
const CLIENT_CHANGE_STATUS: u32 = 716;
/// `k_EMsgClientGamesPlayed` : publie le jeu en cours, ou l'efface.
const CLIENT_GAMES_PLAYED: u32 = 742;
/// `k_EMsgClientFriendMsg` : message envoyé à un ami. 773 est
/// `ClientGameConnectDeny` : l'ancienne valeur ne pouvait pas atteindre Steam.
const CLIENT_FRIEND_MSG: u32 = 718;
/// `k_EMsgClientFriendMsgIncoming` : message reçu d'un ami, poussé par le CM.
const CLIENT_FRIEND_MSG_INCOMING: u32 = 5427;
/// `FriendMessages.GetRecentMessages#1` : historique récent d'une conversation.
const FRIEND_MESSAGES_RECENT: &str = "FriendMessages.GetRecentMessages#1";
/// `FriendMessages.SendMessage#1` : envoi moderne, seul chemin qui fait
/// interpréter le BBCode des emotes par Steam.
const FRIEND_MESSAGES_SEND: &str = "FriendMessages.SendMessage#1";
/// `Player.GetEmoticonList#1` : emotes possédées par le compte.
const PLAYER_EMOTICON_LIST: &str = "Player.GetEmoticonList#1";
/// `k_EMsgClientGetEmoticonList` : demande la collection complète, emotes
/// et autocollants. Le service `Player.GetEmoticonList` ne rend que les
/// emotes ; les autocollants vivent dans l'ancien message 9331.
const CLIENT_GET_EMOTICON_LIST: u32 = 9330;
/// `k_EMsgClientEmoticonList` : réponse 9330, avec `emoticons` (1) et
/// `stickers` (2).
const CLIENT_EMOTICON_LIST: u32 = 9331;
/// Nombre de messages demandés à Steam par page d'historique.
const HISTORY_MESSAGES: u32 = 100;
/// Début de la fenêtre demandée à Steam. Zéro laisse Steam choisir sa fenêtre
/// récente ; une valeur non nulle demande l'historique depuis ce point. Un
/// timestamp minimal récupère tout ce que Steam accepte encore de rendre.
const HISTORY_START_TIME: u32 = 1;
/// Messages conservés en mémoire, toutes pages d'historique confondues.
/// La limite est assez haute pour laisser l'archive locale récupérer les
/// pages anciennes avant qu'elles ne soient élaguées de l'instantané.
const CHAT_MESSAGES: usize = 5000;
/// `k_EMsgClientFriendsList` : la liste d'amis poussée par le CM.
const CLIENT_FRIENDS_LIST: u32 = 767;
/// `k_EMsgClientPersonaState` : les états de présence des amis.
const CLIENT_PERSONA_STATE: u32 = 766;
/// `k_EMsgClientRequestFriendData` : demande les états des amis.
const CLIENT_REQUEST_FRIEND_DATA: u32 = 815;
/// `k_EMsgClientUserNotifications` : objets, cadeaux, demandes.
const CLIENT_USER_NOTIFICATIONS: u32 = 5599;
/// `k_EMsgClientCommentNotifications` : nouveaux commentaires.
const CLIENT_COMMENT_NOTIFICATIONS: u32 = 5582;
/// `k_EMsgClientRequestCommentNotifications`.
const CLIENT_REQUEST_COMMENT_NOTIFICATIONS: u32 = 5583;
/// `k_EMsgClientLoggedOff`.
const CLIENT_LOGGED_OFF: u32 = 757;
/// `k_EMsgClientSetHeartbeatRate`.
const CLIENT_SET_HEARTBEAT_RATE: u32 = 755;
/// `k_EMsgClientPing` : le CM attend un battement en retour.
const CLIENT_PING: u32 = 781;
/// `k_EMsgClientMMSCreateLobby` : création d'un lobby.
const CLIENT_MMS_CREATE_LOBBY: u32 = 6601;
/// `k_EMsgClientMMSCreateLobbyResponse` : réponse, corrélée par `app_id`.
const CLIENT_MMS_CREATE_LOBBY_RESPONSE: u32 = 6602;
/// `k_EMsgClientMMSJoinLobby` : rejointe du lobby d'un ami.
const CLIENT_MMS_JOIN_LOBBY: u32 = 6603;
/// `k_EMsgClientMMSJoinLobbyResponse` : verdict, lobby et membres.
const CLIENT_MMS_JOIN_LOBBY_RESPONSE: u32 = 6604;
/// `k_EMsgClientMMSUserJoinedLobby` : un membre est entré dans le lobby.
const CLIENT_MMS_USER_JOINED_LOBBY: u32 = 6619;
/// `k_EMsgClientMMSUserLeftLobby` : un membre en est sorti.
const CLIENT_MMS_USER_LEFT_LOBBY: u32 = 6620;
/// `k_EMsgClientMMSInviteToLobby` : invitation d'un ami dans un lobby.
const CLIENT_MMS_INVITE_TO_LOBBY: u32 = 6621;
/// `k_EMsgClientMMSSetLobbyData` : publication des métadonnées du lobby.
const CLIENT_MMS_SET_LOBBY_DATA: u32 = 6609;
/// `k_EMsgClientMMSSetLobbyDataResponse` : verdict de la publication.
const CLIENT_MMS_SET_LOBBY_DATA_RESPONSE: u32 = 6610;
/// `k_EMsgClientMMSGetLobbyData` : lecture des métadonnées d'un lobby.
const CLIENT_MMS_GET_LOBBY_DATA: u32 = 6611;
/// `k_EMsgClientMMSLobbyData` : métadonnées poussées par le CM.
const CLIENT_MMS_LOBBY_DATA: u32 = 6612;

/// Invitations reçues depuis l'ouverture de la session. Elles vivent hors de
/// l'instantané : rejoindre est une action ponctuelle, pas un état affiché.
static INVITES: Mutex<Vec<crate::lobby::LobbyInvite>> = Mutex::new(Vec::new());

/// Retient une invitation reçue. La session s'en sert à l'arrivée du message
/// 6621 ; les tests s'en servent pour vérifier la chaîne sans passer par le CM.
pub fn remember_invite(invite: crate::lobby::LobbyInvite) {
    if let Ok(mut slot) = INVITES.lock() {
        if !slot.iter().any(|known| known.lobby == invite.lobby) {
            slot.push(invite);
        }
    }
}

/// Invitations reçues, de la plus ancienne à la plus récente.
pub fn received_invites() -> Vec<crate::lobby::LobbyInvite> {
    INVITES.lock().map(|slot| slot.clone()).unwrap_or_default()
}

/// Traite une invitation reçue du CM (EMsg 6621) : décode `app_id` et
/// `steam_id_lobby`, puis retient l'invitation. C'est le seul chemin de
/// réception — la session et les tests passent par lui.
pub fn receive_invite(body: &[u8]) -> Option<crate::lobby::LobbyInvite> {
    let invite = crate::lobby::parse_invite(body)?;
    remember_invite(invite);
    Some(invite)
}

/// Invitation retenue pour ce lobby, si elle existe : c'est elle qui nomme
/// l'appid et le lobby d'une rejointe acceptée.
pub fn received_invite(lobby: u64) -> Option<crate::lobby::LobbyInvite> {
    received_invites()
        .into_iter()
        .find(|invite| invite.lobby == lobby)
}

/// Demande de join prête à partir d'une invitation reçue : l'EMsg, l'appid,
/// le lobby et le corps encodé. L'appid et le lobby viennent de l'invitation,
/// jamais d'une valeur par défaut ni du jeu lancé.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinRequest {
    pub emsg: u32,
    pub appid: u32,
    pub lobby: u64,
    pub body: Vec<u8>,
}

/// Accepte une invitation retenue : sans invitation pour ce lobby, il n'y a
/// rien à rejoindre et aucune demande n'est fabriquée.
pub fn invite_join_request(lobby: u64, persona: &str, cell_id: u32) -> Option<JoinRequest> {
    let invite = received_invite(lobby)?;
    let body = crate::lobby::join_lobby_body(invite.app_id, invite.lobby, persona, cell_id);
    Some(JoinRequest {
        emsg: CLIENT_MMS_JOIN_LOBBY,
        appid: invite.app_id,
        lobby: invite.lobby,
        body,
    })
}

/// Appid et corps de la demande, quand seul l'encodage compte.
pub fn invite_join_body(lobby: u64, persona: &str, cell_id: u32) -> Option<(u32, Vec<u8>)> {
    invite_join_request(lobby, persona, cell_id).map(|request| (request.appid, request.body))
}

/// Message de rejointe prêt pour la sortie : l'EMsg et le corps exactement
/// tels que le worker les remet au transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JoinMessage {
    pub emsg: u32,
    pub appid: u32,
    pub body: Vec<u8>,
}

/// Sortie des rejointes : la session y branche le client Steam, le test un
/// enregistreur. C'est le seul point par lequel un 6603 quitte le worker.
#[allow(async_fn_in_trait)]
pub(crate) trait JoinSink {
    async fn send_join(&mut self, message: JoinMessage) -> Result<(), String>;
}

/// Sortie réelle : estampille le message (job, session, appid) et le remet au
/// CM par le transport Steam.
pub(crate) struct ClientJoinSink<'a> {
    pub client: &'a SteamClient<LoggedIn>,
    pub steam_id: u64,
    pub session_id: Option<i32>,
    pub job: Option<u64>,
}

impl JoinSink for ClientJoinSink<'_> {
    async fn send_join(&mut self, message: JoinMessage) -> Result<(), String> {
        send_lobby(
            self.client,
            self.steam_id,
            self.session_id,
            message.appid,
            self.job,
            message.emsg,
            &message.body,
        )
        .await
    }
}

/// Prépare le message de join que le worker envoie : invitation retenue pour
/// ce lobby quand elle existe, sinon le lobby du jeu avec l'appid demandé.
/// Le test du parcours vérifie ainsi exactement ce qui part sur le réseau.
pub fn prepare_join_request(appid: u32, lobby: u64, persona: &str, cell_id: u32) -> JoinRequest {
    match invite_join_request(lobby, persona, cell_id) {
        Some(request) => request,
        None => JoinRequest {
            emsg: CLIENT_MMS_JOIN_LOBBY,
            appid,
            lobby,
            body: crate::lobby::join_lobby_body(appid, lobby, persona, cell_id),
        },
    }
}

/// Émission d'une rejointe : prépare la demande, puis la remet au sink. Le
/// worker et la preuve de parcours passent par cette même fonction ; seule
/// l'implémentation du sink diffère (client Steam ou enregistreur de test).
pub(crate) async fn emit_join<S: JoinSink>(
    sink: &mut S,
    appid: u32,
    lobby: u64,
    persona: &str,
    cell_id: u32,
) -> Result<(), String> {
    let request = prepare_join_request(appid, lobby, persona, cell_id);
    tracing::info!(
        "Steam : rejointe du lobby {} demandée (appid {})",
        request.lobby,
        request.appid
    );
    sink.send_join(JoinMessage {
        emsg: request.emsg,
        appid: request.appid,
        body: request.body,
    })
    .await
}

/// Adresse `ip:port` publiée dans les métadonnées d'un lobby, telle que le CM
/// l'a rendue (6612). C'est ce que voit un invité, même sur une autre machine.
static LOBBY_CONNECTS: Mutex<std::collections::BTreeMap<u64, String>> =
    Mutex::new(std::collections::BTreeMap::new());

/// Retient l'adresse lue dans les métadonnées d'un lobby.
pub fn remember_lobby_connect(lobby: u64, connect: &str) {
    let connect = connect.trim();
    if lobby == 0 || connect.is_empty() {
        return;
    }
    if let Ok(mut map) = LOBBY_CONNECTS.lock() {
        map.insert(lobby, connect.to_string());
    }
}

/// Adresse enregistrée pour ce lobby, si le CM l'a déjà rendue.
pub fn lobby_connect(lobby: u64) -> Option<String> {
    LOBBY_CONNECTS
        .lock()
        .ok()
        .and_then(|map| map.get(&lobby).cloned())
}

/// Membres connus d'un lobby rejoint : posés par la réponse 6604, tenus à
/// jour par les notifications 6619 (entrée) et 6620 (sortie).
static LOBBY_MEMBERS: Mutex<std::collections::BTreeMap<u64, Vec<crate::lobby::LobbyMember>>> =
    Mutex::new(std::collections::BTreeMap::new());

/// Membres actuellement connus pour ce lobby, dans l'ordre d'arrivée.
pub fn lobby_members(lobby: u64) -> Vec<crate::lobby::LobbyMember> {
    LOBBY_MEMBERS
        .lock()
        .ok()
        .and_then(|map| map.get(&lobby).cloned())
        .unwrap_or_default()
}

/// Remplace la liste des membres d'un lobby rejoint.
fn remember_lobby_members(lobby: u64, members: &[crate::lobby::LobbyMember]) {
    if let Ok(mut map) = LOBBY_MEMBERS.lock() {
        map.insert(lobby, members.to_vec());
    }
}

/// Ajoute un membre entré (6619), ou rafraîchit son nom. Une notification
/// reçue prouve l'appartenance au lobby : sans 6604 préalable, la liste naît
/// de cette première annonce au lieu de rester vide.
fn apply_member_joined(lobby: u64, member: crate::lobby::LobbyMember) -> bool {
    let Ok(mut map) = LOBBY_MEMBERS.lock() else {
        return false;
    };
    let members = map.entry(lobby).or_default();
    match members
        .iter_mut()
        .find(|known| known.steam_id == member.steam_id)
    {
        Some(known) => *known = member,
        None => members.push(member),
    }
    true
}

/// Retire un membre sorti (6620). Sans 6604 préalable, la liste du lobby est
/// créée vide : elle est correcte, quitte à ce qu'une entrée suivante la
/// remplisse.
fn apply_member_left(lobby: u64, steam_id: u64) -> bool {
    let Ok(mut map) = LOBBY_MEMBERS.lock() else {
        return false;
    };
    let members = map.entry(lobby).or_default();
    members.retain(|known| known.steam_id != steam_id);
    true
}

/// Applique une notification de membre déjà lue : entrée (6619) ou sortie
/// (6620). C'est le seul chemin de mise à jour de la liste.
pub fn apply_member_event(event: &crate::lobby::LobbyMemberEvent, enters: bool) -> bool {
    if enters {
        apply_member_joined(event.lobby, event.member.clone())
    } else {
        apply_member_left(event.lobby, event.member.steam_id)
    }
}

/// Dernier lobby créé (appid, lobby), pour que le daemon puisse le relire et
/// y inviter des amis.
static CREATED_LOBBY: std::sync::Mutex<Option<(u32, u64)>> = std::sync::Mutex::new(None);

/// Informations demandées pour chaque ami : nom, état, présence, dernière
/// connexion, jeu en cours et données de riche présence
/// (`EClientPersonaStateFlag`).
const PERSONA_FLAGS: u32 = 1 | 2 | 16 | 64 | 256 | 512 | 4096;

/// Attente maximale du logon auprès d'un CM.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(25);
/// Attente maximale d'une requête à l'API communautaire.
const HTTP_TIMEOUT: Duration = Duration::from_secs(12);
/// Battement par défaut si le CM ne dit rien de mieux.
const DEFAULT_HEARTBEAT: Duration = Duration::from_secs(15);
/// Pause avant de retenter après une connexion perdue.
const RETRY_DELAY: Duration = Duration::from_secs(8);
/// Attente pendant que le pool de lectures occupe la seule session Steam.
const SESSION_PAUSE: Duration = Duration::from_secs(2);
/// Délai initial : le scan de démarrage lit la bibliothèque avant la présence.
const STARTUP_DELAY: Duration = Duration::from_secs(20);

/// Commandes reçues par la session depuis le daemon.
#[derive(Debug)]
pub enum SocialCommand {
    /// Change la présence publiée.
    Presence(SteamPresence),
    /// Publie le jeu Steam en cours ; `None` efface l'état « en jeu ».
    /// Le nom accompagne l'appid parce que Steam l'affiche dans l'invitation
    /// native, et parce qu'un appid seul ne suffit pas toujours à remplir la
    /// vignette chez le destinataire.
    Playing { appid: Option<u32>, name: String },
    /// Redemande la liste d'amis et leurs états.
    Refresh,
    /// Active une clé produit Steam sur le compte connecté.
    Redeem(String),
    /// Envoie un message texte à un ami.
    Message { steam_id: u64, text: String },
    /// Envoie un autocollant Steam à un ami.
    Sticker { steam_id: u64, name: String },
    /// Demande les derniers messages échangés avec un ami. `older` remonte
    /// d'une page quand Steam a signalé d'autres messages.
    History { steam_id: u64, older: bool },
    /// Crée un lobby Steam pour ce jeu.
    CreateLobby {
        appid: u32,
        max_members: u32,
        lobby_type: i32,
        lobby_flags: i32,
        /// Cellule CM explicitement demandée ; `None` = celle de la session.
        cell_id: Option<u32>,
        /// Adresse `ip:port` publiée dans les métadonnées du lobby.
        connect: Option<String>,
        reply: std::sync::mpsc::Sender<Result<u64, i32>>,
    },
    /// Publie l'adresse de la partie dans les métadonnées du lobby.
    SetLobbyData {
        appid: u32,
        lobby: u64,
        connect: String,
    },
    /// Demande les métadonnées d'un lobby (réponse 6612).
    GetLobbyData { appid: u32, lobby: u64 },
    /// Rejoint le lobby reçu dans une invitation.
    JoinLobby {
        appid: u32,
        lobby: u64,
        reply: std::sync::mpsc::Sender<Result<crate::lobby::JoinedLobby, i32>>,
    },
    /// Invite un ami dans le dernier lobby créé.
    InviteToLobby {
        appid: u32,
        lobby: u64,
        steam_id: u64,
        reply: std::sync::mpsc::Sender<Result<(), i32>>,
    },
}

/// Demande la création d'un lobby pour ce jeu. La session envoie
/// `CMsgClientMMSCreateLobby` ; la réponse 6602 est journalisée et son
/// identifiant conservé pour le daemon.
pub fn create_lobby(
    social: &Social,
    appid: u32,
    max_members: u32,
    lobby_type: Option<i32>,
    lobby_flags: Option<i32>,
    cell_id: Option<u32>,
    connect: Option<&str>,
) -> Option<std::sync::mpsc::Receiver<Result<u64, i32>>> {
    // Le jeu peut avoir déjà créé son lobby (ou le pont l'avoir fait pour
    // lui) : on le réutilise au lieu d'en ouvrir un second pour le même
    // appid. `forget_lobby` le jette dès que la session change de jeu.
    if let Some((known_appid, lobby)) = last_lobby_info() {
        if known_appid == appid && lobby != 0 {
            tracing::info!("Steam : lobby {lobby} récupéré pour appid {appid}");
            let (reply, answer) = std::sync::mpsc::channel();
            let _ = reply.send(Ok(lobby));
            return Some(answer);
        }
    }
    let (reply, answer) = std::sync::mpsc::channel();
    social
        .commands
        .send(SocialCommand::CreateLobby {
            appid,
            max_members,
            lobby_type: lobby_type.unwrap_or(2),
            lobby_flags: lobby_flags.unwrap_or(1),
            cell_id,
            connect: connect.map(str::to_string),
            reply,
        })
        .ok()?;
    Some(answer)
}

/// Publie l'adresse de la partie dans les métadonnées du lobby : un invité
/// distant la relit par `CMsgClientMMSGetLobbyData`.
pub fn set_lobby_data(social: &Social, appid: u32, lobby: u64, connect: &str) -> bool {
    social
        .commands
        .send(SocialCommand::SetLobbyData {
            appid,
            lobby,
            connect: connect.to_string(),
        })
        .is_ok()
}

/// Demande les métadonnées d'un lobby ; la réponse remplit `lobby_connect`.
pub fn request_lobby_data(social: &Social, appid: u32, lobby: u64) -> bool {
    social
        .commands
        .send(SocialCommand::GetLobbyData { appid, lobby })
        .is_ok()
}

/// Réponse de rejointe en attente : une seule à la fois, la session la prend
/// à l'arrivée du 6604.
static PENDING_JOIN: Mutex<
    Option<std::sync::mpsc::Sender<Result<crate::lobby::JoinedLobby, i32>>>,
> = Mutex::new(None);

/// Rejoint un lobby. La réponse porte le verdict (`eresult` 1 = accepté), le
/// lobby effectif et les membres déjà présents.
pub fn join_lobby(
    social: &Social,
    appid: u32,
    lobby: u64,
) -> Option<std::sync::mpsc::Receiver<Result<crate::lobby::JoinedLobby, i32>>> {
    let (reply, answer) = std::sync::mpsc::channel();
    social
        .commands
        .send(SocialCommand::JoinLobby {
            appid,
            lobby,
            reply,
        })
        .ok()?;
    Some(answer)
}

/// Verdict d'une réponse de join : `Ok` seulement si le CM accepte et donne
/// un lobby. Le refus garde son eresult, que l'appelant remonte.
fn join_verdict(joined: &crate::lobby::JoinedLobby) -> Result<(), i32> {
    if joined.eresult == 1 && joined.lobby != 0 {
        Ok(())
    } else {
        Err(joined.eresult)
    }
}

/// Enregistre un lobby rejoint quand le CM l'accepte (`chat_room_enter_response`
/// vaut 1) : l'adresse publiée devient lisible par le daemon et le lobby est
/// celui des invitations suivantes. Un refus ne laisse rien derrière.
pub fn record_join_result(joined: &crate::lobby::JoinedLobby) -> bool {
    if joined.eresult != 1 || joined.lobby == 0 {
        return false;
    }
    if let Ok(mut slot) = CREATED_LOBBY.lock() {
        *slot = Some((joined.app_id, joined.lobby));
    }
    if let Some(connect) = joined.connect() {
        remember_lobby_connect(joined.lobby, &connect);
    }
    remember_lobby_members(joined.lobby, &joined.members);
    true
}

/// Cellule CM annoncée par le client. Steam l'assigne au logon ; faute de
/// pouvoir la relire dans la bibliothèque, elle se surcharge par
/// `LAPINUX_LOBBY_CELL` (voir la sonde `exemples/cellule.rs`).
fn lobby_cell() -> u32 {
    std::env::var("LAPINUX_LOBBY_CELL")
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(0)
}

/// IP publique annoncée dans la demande, comme le fait le client Steam.
fn lobby_public_ip() -> Option<[u8; 4]> {
    std::env::var("LAPINUX_LOBBY_IP")
        .ok()?
        .trim()
        .parse::<std::net::Ipv4Addr>()
        .ok()
        .map(|ip| ip.octets())
}

/// Identifiant du dernier lobby créé, s'il y en a un.
pub fn last_lobby() -> Option<u64> {
    last_lobby_info().map(|(_, lobby)| lobby)
}

/// Appid et identifiant du dernier lobby créé, s'il y en a un.
pub fn last_lobby_info() -> Option<(u32, u64)> {
    CREATED_LOBBY.lock().ok().and_then(|slot| *slot)
}

/// Oublie le lobby quand la session change de jeu : un lobby de la partie
/// précédente ne doit pas être réutilisé pour la suivante.
pub fn forget_lobby() {
    if let Ok(mut slot) = CREATED_LOBBY.lock() {
        if let Some((appid, lobby)) = slot.take() {
            tracing::info!("Steam : lobby {lobby} oublié (appid {appid})");
        }
    }
}

/// Retient l'appid publié et jette le lobby quand le jeu change. Les
/// republications du même appid gardent le lobby courant.
fn set_playing_appid(playing: &AtomicU64, appid: Option<u32>) {
    let next = u64::from(appid.unwrap_or(0));
    let previous = playing.swap(next, Ordering::SeqCst);
    if previous != next {
        forget_lobby();
    }
}

/// Invite un ami dans le dernier lobby créé. Rend `None` quand aucun lobby
/// n'est connu : le daemon le dit à l'interface au lieu d'envoyer un message
/// sans cible.
pub fn invite_to_lobby(
    social: &Social,
    steam_id: u64,
) -> Option<std::sync::mpsc::Receiver<Result<(), i32>>> {
    let (appid, lobby) = last_lobby_info()?;
    let (reply, answer) = std::sync::mpsc::channel();
    social
        .commands
        .send(SocialCommand::InviteToLobby {
            appid,
            lobby,
            steam_id,
            reply,
        })
        .ok()?;
    Some(answer)
}

/// Poignée de la session sociale : un instantané partagé et un canal de
/// commandes. La session vit dans son propre fil, avec son runtime tokio.
pub struct Social {
    snapshot: Arc<Mutex<SteamSocial>>,
    commands: tokio::sync::mpsc::UnboundedSender<SocialCommand>,
    /// Appid publié, zéro quand aucun jeu n'est en cours. La session le relit
    /// après une reconnexion : l'état survit à une coupure du CM.
    playing: Arc<AtomicU64>,
    /// Nom du jeu publié, pour que la présence Steam ne reste pas vide.
    playing_name: Arc<Mutex<String>>,
    stop: Arc<AtomicBool>,
}

impl Social {
    /// Démarre la session. Elle se reconnecte toute seule : l'appel ne peut
    /// donc pas échouer sur le réseau, seulement sur l'absence de jeton.
    pub fn start(
        session: SavedSession,
        presence: SteamPresence,
    ) -> Result<Self, lapinux_core::Error> {
        Self::start_with_playing(session, presence, None, None)
    }

    /// Démarre la session en reprenant un jeu déjà lancé, par exemple après
    /// une pause du catalogue ou une reconnexion du daemon.
    pub fn start_with_playing(
        session: SavedSession,
        presence: SteamPresence,
        playing: Option<u32>,
        playing_name: Option<String>,
    ) -> Result<Self, lapinux_core::Error> {
        if session.refresh_token.trim().is_empty() {
            return Err(lapinux_core::Error::Other(
                "session Steam sans jeton : reconnectez-vous".into(),
            ));
        }
        let snapshot = Arc::new(Mutex::new(SteamSocial {
            state: "connecting".into(),
            steam_id: session.steam_id,
            persona_name: session.account_name.clone(),
            presence,
            ..SteamSocial::default()
        }));
        let (commands, receiver) = tokio::sync::mpsc::unbounded_channel();
        let stop = Arc::new(AtomicBool::new(false));
        let playing = Arc::new(AtomicU64::new(u64::from(playing.unwrap_or(0))));
        let playing_name = Arc::new(Mutex::new(playing_name.unwrap_or_default()));
        let chats = Arc::new(Mutex::new(BTreeMap::new()));
        let worker_snapshot = Arc::clone(&snapshot);
        let worker_stop = Arc::clone(&stop);
        let worker_playing = Arc::clone(&playing);
        let worker_playing_name = Arc::clone(&playing_name);
        let worker_chats = Arc::clone(&chats);
        std::thread::Builder::new()
            .name("lapinux-social".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build();
                let Ok(runtime) = runtime else {
                    if let Ok(mut slot) = worker_snapshot.lock() {
                        *slot = SteamSocial::failed("runtime réseau indisponible");
                    }
                    return;
                };
                runtime.block_on(worker(
                    session,
                    worker_snapshot,
                    receiver,
                    worker_stop,
                    worker_playing,
                    worker_playing_name,
                    worker_chats,
                    presence,
                ));
            })
            .map_err(|error| lapinux_core::Error::Other(format!("fil social : {error}")))?;
        Ok(Self {
            snapshot,
            commands,
            playing,
            playing_name,
            stop,
        })
    }

    /// Dernier état publié.
    pub fn snapshot(&self) -> SteamSocial {
        self.snapshot
            .lock()
            .map(|slot| slot.clone())
            .unwrap_or_default()
    }

    /// Change la présence publiée. L'instantané prend le choix tout de suite :
    /// l'interface qui répond au clic ne doit pas réafficher l'ancien état en
    /// attendant que le CM confirme (`applied_presence`).
    pub fn set_presence(&self, presence: SteamPresence) -> bool {
        if self
            .commands
            .send(SocialCommand::Presence(presence))
            .is_err()
        {
            return false;
        }
        if let Ok(mut slot) = self.snapshot.lock() {
            slot.presence = presence;
            slot.updated = now_unix();
        }
        true
    }

    /// Publie le jeu Steam en cours, ou l'efface avec `None`. Le nom est
    /// transmis à Steam avec l'appid : sans lui, l'invitation native peut
    /// s'afficher sans nom ni vignette chez le destinataire.
    pub fn set_playing(&self, appid: Option<u32>, name: Option<&str>) -> bool {
        let name = name.unwrap_or_default().trim().to_string();
        if self
            .commands
            .send(SocialCommand::Playing {
                appid,
                name: name.clone(),
            })
            .is_err()
        {
            return false;
        }
        self.playing
            .store(u64::from(appid.unwrap_or(0)), Ordering::SeqCst);
        if let Ok(mut slot) = self.playing_name.lock() {
            *slot = name;
        }
        true
    }

    /// Jeu Steam actuellement publié, s'il y en a un.
    pub fn playing_appid(&self) -> Option<u32> {
        match self.playing.load(Ordering::SeqCst) as u32 {
            0 => None,
            appid => Some(appid),
        }
    }

    /// Republie l'état « en jeu » avec le nom conservé. Une invitation part
    /// toujours après un état frais : sans cela, Steam peut perdre le jeu
    /// associé et afficher « Inviter à jouer à … » sans vignette ni bouton.
    pub fn republish_playing(&self) -> bool {
        let appid = self.playing_appid();
        let name = self
            .playing_name
            .lock()
            .map(|name| name.clone())
            .unwrap_or_default();
        self.commands
            .send(SocialCommand::Playing { appid, name })
            .is_ok()
    }

    /// Redemande la liste d'amis et les états.
    pub fn refresh(&self) -> bool {
        self.commands.send(SocialCommand::Refresh).is_ok()
    }

    /// Active une clé produit sur le compte connecté.
    pub fn redeem(&self, key: &str) -> bool {
        self.commands
            .send(SocialCommand::Redeem(key.to_string()))
            .is_ok()
    }

    /// Envoie un message texte à un ami. Steam ne répond pas à l'envoi : le
    /// message local est ajouté à l'instantané par le worker après remise au
    /// CM, puis les réponses arrivent par `ClientFriendMsgIncoming`.
    pub fn message(&self, steam_id: u64, text: &str) -> bool {
        let text = text.trim();
        if steam_id == 0 || text.is_empty() || text.len() > 1024 {
            return false;
        }
        self.commands
            .send(SocialCommand::Message {
                steam_id,
                text: text.to_string(),
            })
            .is_ok()
    }

    /// Envoie un autocollant à un ami. Les noms Steam peuvent contenir des
    /// espaces (`Three Piece`) ; seuls les caractères de contrôle et ceux qui
    /// casseraient la commande `/sticker` sont refusés.
    pub fn sticker(&self, steam_id: u64, name: &str) -> bool {
        let name = name.trim();
        if steam_id == 0
            || name.is_empty()
            || name.len() > 100
            || name.chars().any(|character| {
                character.is_control() || matches!(character, '[' | ']' | '"' | '\\')
            })
        {
            return false;
        }
        self.commands
            .send(SocialCommand::Sticker {
                steam_id,
                name: name.to_string(),
            })
            .is_ok()
    }

    /// Demande l'historique récent d'une conversation. La réponse est
    /// fusionnée dans l'instantané par le worker, sans bloquer l'interface.
    pub fn history(&self, steam_id: u64, older: bool) -> bool {
        if steam_id == 0 {
            return false;
        }
        self.commands
            .send(SocialCommand::History { steam_id, older })
            .is_ok()
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl Drop for Social {
    fn drop(&mut self) {
        self.stop();
    }
}

/// État interne du fil : ce que Steam a poussé, prêt à être publié.
struct Live {
    persona_name: String,
    steam_id: u64,
    cell_id: u32,
    public_ip: Option<[u8; 4]>,
    friends: HashMap<u64, SteamFriend>,
    notifications: Vec<SteamNotification>,
    web_notifications: Vec<SteamWebNotification>,
    /// Détails des jeux déjà résolus pour les notifications wishlist : l'API
    /// du magasin n'est interrogée que pour les appids encore inconnus.
    wishlist_deals: HashMap<u32, SteamWishlistDeal>,
    /// Activité récente des amis, de la plus récente à la plus ancienne.
    friend_events: Vec<SteamFriendEvent>,
    /// Amis dont le premier état sert de référence : leur apparition initiale
    /// ne déclenche pas de notification.
    friends_seen: std::collections::HashSet<u64>,
    new_comments: u32,
    family: Option<SteamFamily>,
    /// Présence demandée par l'utilisateur, publiée sur le réseau.
    presence: SteamPresence,
    /// Présence confirmée par Steam (`CMsgPersonaChangeResponse`).
    applied: SteamPresence,
    /// Dernière activation de clé, telle qu'elle s'affiche.
    redeem: Option<SteamKeyRedeem>,
    /// État de la lecture de l'historique, par ami.
    history: BTreeMap<u64, SteamChatHistoryState>,
    /// Emotes possédées, rendues par `Player.GetEmoticonList`.
    emoticons: Vec<SteamEmoticon>,
    /// Autocollants possédés, rendus par `CMsgClientEmoticonList`.
    stickers: Vec<SteamSticker>,
}

async fn worker(
    session: SavedSession,
    snapshot: Arc<Mutex<SteamSocial>>,
    mut commands: tokio::sync::mpsc::UnboundedReceiver<SocialCommand>,
    stop: Arc<AtomicBool>,
    playing: Arc<AtomicU64>,
    playing_name: Arc<Mutex<String>>,
    chats: Arc<Mutex<BTreeMap<u64, Vec<SteamChatMessage>>>>,
    presence: SteamPresence,
) {
    let mut presence = presence;
    // Une activation demandée pendant une reconnexion part dès que le canal
    // est rouvert : une clé ne se perd pas en silence.
    let mut pending_redeem: Option<String> = None;
    // Le démarrage (scan de la bibliothèque) occupe la session Steam : s'y
    // connecter tout de suite la ferait éjecter, puis refuser le jeton.
    tokio::time::sleep(STARTUP_DELAY).await;
    loop {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        // Steam ne garde qu'une session par compte : ouvrir celle-ci pendant
        // qu'une lecture du pool travaille ferme la connexion de la lecture,
        // et la lecture suivante ferme la nôtre — une rafale de logons qui
        // faisait échouer les copies de sauvegarde en cours. On laisse donc le
        // pool finir avant de se reconnecter, sans publier d'échec : le dernier
        // état connu reste affiché pendant l'attente.
        if crate::shared::pool_wants_session() || !crate::shared::release_if_idle().await {
            tokio::time::sleep(SESSION_PAUSE).await;
            continue;
        }
        publish(
            &snapshot,
            SteamSocial::connecting(),
            presence,
            session.steam_id,
        );
        match connect(
            &session,
            &snapshot,
            &mut commands,
            &stop,
            &playing,
            &playing_name,
            &chats,
            &mut presence,
            &mut pending_redeem,
        )
        .await
        {
            Ok(()) => {
                // Déconnexion propre demandée ou fermeture du CM : on boucle.
            }
            Err(error) => {
                if stop.load(Ordering::SeqCst) {
                    return;
                }
                if crate::shared::pool_busy() {
                    // Coupure causée par une lecture du pool : le dernier état
                    // connu reste affiché, le temps que la session soit rendue.
                    tracing::debug!("session sociale interrompue par une lecture : {error}");
                } else {
                    publish(
                        &snapshot,
                        SteamSocial::failed(error),
                        presence,
                        session.steam_id,
                    );
                }
            }
        }
        // Attente avant reconnexion, interrompue par une commande.
        let deadline = tokio::time::Instant::now() + RETRY_DELAY;
        loop {
            tokio::select! {
                _ = tokio::time::sleep_until(deadline) => break,
                command = commands.recv() => match command {
                    Some(SocialCommand::Presence(next)) => presence = next,
                    Some(SocialCommand::Playing { appid, name }) => {
                        set_playing_appid(&playing, appid);
                        if let Ok(mut slot) = playing_name.lock() {
                            *slot = name;
                        }
                        break;
                    }
                    Some(SocialCommand::Refresh) => break,
                    // Sans session, l'activation ne peut pas aboutir : la clé
                    // attend la reconnexion au lieu d'être perdue en silence.
                    Some(SocialCommand::Redeem(key)) => pending_redeem = Some(key),
                    Some(SocialCommand::Message { steam_id, .. }) => {
                        tracing::debug!(
                            "message Steam à {steam_id} ignoré : session absente"
                        );
                    }
                    Some(SocialCommand::Sticker { steam_id, .. }) => {
                        tracing::debug!(
                            "autocollant Steam à {steam_id} ignoré : session absente"
                        );
                    }
                    Some(SocialCommand::History { steam_id, older }) => {
                        tracing::debug!(
                            "historique Steam de {steam_id} ignoré : session absente ({older})"
                        );
                    }
                    Some(SocialCommand::CreateLobby { reply, .. }) => {
                        tracing::debug!("création de lobby impossible : session absente");
                        let _ = reply.send(Err(2));
                    }
                    Some(SocialCommand::JoinLobby { lobby, reply, .. }) => {
                        tracing::debug!("rejointe du lobby {lobby} impossible : session absente");
                        let _ = reply.send(Err(2));
                    }
                    Some(SocialCommand::SetLobbyData { lobby, .. }) => {
                        tracing::debug!(
                            "métadonnées du lobby {lobby} non publiées : session absente"
                        );
                    }
                    Some(SocialCommand::GetLobbyData { lobby, .. }) => {
                        tracing::debug!(
                            "métadonnées du lobby {lobby} non demandées : session absente"
                        );
                    }
                    Some(SocialCommand::InviteToLobby { reply, .. }) => {
                        tracing::debug!("invitation de lobby impossible : session absente");
                        let _ = reply.send(Err(2));
                    }
                    None => return,
                },
            }
            if stop.load(Ordering::SeqCst) {
                return;
            }
        }
    }
}

/// Publie l'instantané en gardant la présence demandée et le SteamID connus.
fn publish(
    snapshot: &Arc<Mutex<SteamSocial>>,
    mut social: SteamSocial,
    presence: SteamPresence,
    steam_id: u64,
) {
    social.presence = presence;
    if social.steam_id == 0 {
        social.steam_id = steam_id;
    }
    social.updated = now_unix();
    if let Ok(mut slot) = snapshot.lock() {
        *slot = social;
    }
}

/// Une vie de la session : connexion, publication de la présence, puis boucle
/// d'écoute. Rend la main quand la connexion est perdue.
async fn connect(
    session: &SavedSession,
    snapshot: &Arc<Mutex<SteamSocial>>,
    commands: &mut tokio::sync::mpsc::UnboundedReceiver<SocialCommand>,
    stop: &Arc<AtomicBool>,
    playing: &Arc<AtomicU64>,
    playing_name: &Arc<Mutex<String>>,
    chats: &Arc<Mutex<BTreeMap<u64, Vec<SteamChatMessage>>>>,
    presence: &mut SteamPresence,
    pending_redeem: &mut Option<String>,
) -> Result<(), String> {
    // Le WebSocket du CM est en TLS : sans fournisseur choisi, rustls panique
    // ici et emporte le fil social avec lui.
    crate::install_crypto_provider();
    if let Some(seconds) = crate::shared::logon_hold_seconds(&session.account_name) {
        return Err(format!(
            "Steam limite les tentatives pour ce compte ; nouvel essai dans {seconds} s"
        ));
    }
    let client = tokio::time::timeout(
        LOGIN_TIMEOUT,
        LoginBuilder::new()
            .device_name(lapinux_core::APP_NAME)
            .cell_id(lobby_cell())
            .with_refresh_token(session.account_name.clone(), session.refresh_token.clone())
            .login(),
    )
    .await
    .map_err(|_| "connexion au réseau Steam expirée".to_string())?
    .map_err(|error| {
        let text = error.to_string();
        if text.to_lowercase().contains("invalidpassword") {
            crate::shared::note_logon_refused(&session.account_name);
        }
        format!("connexion au réseau Steam : {text}")
    })?;
    connect_with(
        client,
        session,
        snapshot,
        commands,
        stop,
        playing,
        playing_name,
        chats,
        presence,
        pending_redeem,
    )
    .await
}

/// Suite d'une session CM déjà authentifiée : publication de la présence,
/// puis boucle d'écoute et de commandes. Séparée de [`connect`] pour que le
/// test d'intégration puisse brancher le vrai client sur un faux CM local.
async fn connect_with(
    client: SteamClient<LoggedIn>,
    session: &SavedSession,
    snapshot: &Arc<Mutex<SteamSocial>>,
    commands: &mut tokio::sync::mpsc::UnboundedReceiver<SocialCommand>,
    stop: &Arc<AtomicBool>,
    playing: &Arc<AtomicU64>,
    playing_name: &Arc<Mutex<String>>,
    chats: &Arc<Mutex<BTreeMap<u64, Vec<SteamChatMessage>>>>,
    presence: &mut SteamPresence,
    pending_redeem: &mut Option<String>,
) -> Result<(), String> {
    let mut live = Live {
        persona_name: session.account_name.clone(),
        steam_id: session.steam_id,
        cell_id: lobby_cell(),
        public_ip: lobby_public_ip(),
        friends: HashMap::new(),
        notifications: Vec::new(),
        web_notifications: Vec::new(),
        wishlist_deals: HashMap::new(),
        friend_events: Vec::new(),
        friends_seen: std::collections::HashSet::new(),
        new_comments: 0,
        family: None,
        presence: *presence,
        applied: *presence,
        redeem: None,
        history: BTreeMap::new(),
        emoticons: Vec::new(),
        stickers: Vec::new(),
    };

    // 1. Publier la présence et, si une partie tourne déjà, le jeu en cours.
    send_change_status(&client, session.steam_id, live.presence).await?;
    let appid = playing.load(Ordering::SeqCst) as u32;
    if appid != 0 {
        let name = playing_name
            .lock()
            .map(|name| name.clone())
            .unwrap_or_default();
        if let Err(error) = send_games_played(&client, session.steam_id, appid, &name).await {
            tracing::warn!("Steam : jeu {appid} non publié ({error})");
        }
    }
    send(
        &client,
        session.steam_id,
        CLIENT_REQUEST_COMMENT_NOTIFICATIONS,
        &CMsgClientRequestCommentNotifications::default().encode_to_vec(),
    )
    .await?;
    let mut calls = Calls {
        next: 0,
        renew: None,
        family: None,
        shared: None,
        history: None,
        history_queue: VecDeque::new(),
        emoticons: None,
    };
    // Les emotes possédées ne changent qu'avec la collection : une demande
    // par session suffit. La réponse rejoint l'instantané publié.
    let job = calls.next_job();
    match start_service(
        &client,
        session.steam_id,
        None,
        job,
        PLAYER_EMOTICON_LIST,
        &[],
    )
    .await
    {
        Ok(()) => calls.emoticons = Some(job),
        // Une collection d'emotes indisponible ne doit pas couper la session
        // sociale : le chat et la présence continuent sans elles.
        Err(error) => tracing::warn!("Steam : liste d'emotes non demandée ({error})"),
    }
    // La collection complète porte aussi les autocollants : elle est
    // demandée séparément, sans bloquer la session si Steam l'ignore.
    match send(&client, session.steam_id, CLIENT_GET_EMOTICON_LIST, &[]).await {
        Ok(()) => tracing::info!("Steam : collection d'emotes et d'autocollants demandée"),
        Err(error) => tracing::warn!("Steam : collection d'autocollants non demandée ({error})"),
    }
    // 2. Le jeton est renouvelé dès la première connexion réussie : Steam rend
    //    un jeton tourné, écrit dans le fichier de session. La réponse est
    //    reconnue par la boucle d'écoute — attendre ici consommerait les
    //    poussées qui accompagnent le logon. Une sonde peut laisser
    //    l'entretien de côté avec `LAPINUX_SOCIAL_NO_RENEW`.
    if renewal_enabled() {
        let job = calls.next_job();
        let mut request = Vec::new();
        proto::field_bytes(1, session.refresh_token.as_bytes(), &mut request);
        proto::field_fixed64(2, session.steam_id, &mut request);
        // `k_ETokenRenewalType_Allow` : le jeton rendu doit rester utilisable
        // par un client de bureau.
        proto::field_varint(3, 1, &mut request);
        start_service(
            &client,
            session.steam_id,
            None,
            job,
            "Authentication.GenerateAccessTokenForApp#1",
            &request,
        )
        .await?;
        calls.renew = Some(job);
    }

    let mut heartbeat = tokio::time::interval(DEFAULT_HEARTBEAT);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // Les notifications du service Web ne sont pas poussées par le CM : elles
    // sont relues à intervalle régulier avec le jeton de session.
    let mut notifications_tick = tokio::time::interval(Duration::from_secs(60));
    notifications_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut family_done = false;
    let mut friends_asked = false;
    // La collection est redemandée une fois la liste d'amis reçue : le
    // message est alors sûr d'être lu par la boucle, même si la première
    // demande a précédé l'ouverture de l'écoute.
    let mut collection_asked = false;
    // Jeton d'accès de la session : il sert à l'API communautaire, jamais
    // écrit sur disque et jamais publié dans l'instantané.
    let mut access: Option<String> = None;

    // Une clé demandée pendant la reconnexion part maintenant, une fois la
    // session en place : envoyée juste après le logon, le CM fermait la
    // connexion et Steam ne répondait jamais à l'activation.
    if let Some(key) = pending_redeem.take() {
        live.redeem = Some(SteamKeyRedeem {
            state: "running".into(),
            message: "Activation demandée à Steam…".into(),
            games: Vec::new(),
            appids: Vec::new(),
        });
        publish(snapshot, live.snapshot(chats), live.presence, live.steam_id);
        let mut body = Vec::new();
        proto::field_bytes(1, key.trim().as_bytes(), &mut body);
        if let Err(error) = send(&client, live.steam_id, CLIENT_REGISTER_KEY, &body).await {
            tracing::warn!("Steam : clé non envoyée ({error})");
            *pending_redeem = Some(key);
        }
    }

    let mut pending_lobby: Option<(u32, std::sync::mpsc::Sender<Result<u64, i32>>)> = None;
    // Le CM attribue un identifiant de session au logon. Les messages de
    // matchmaking et les appels de service portent les mêmes champs d'en-tête
    // que le client Steam : sans `client_sessionid`, le CM ne rattache pas le
    // lobby ni le message à la session.
    let mut session_id: Option<i32> = None;
    let mut lobby_job: u64 = 1;
    // Guette la réclamation de la session unique par le pool, même quand les
    // messages du CM arrivent sans interruption.
    let mut handoff = tokio::time::interval(SESSION_PAUSE);
    handoff.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        if stop.load(Ordering::SeqCst) {
            return Ok(());
        }
        tokio::select! {
            command = commands.recv() => match command {
                Some(SocialCommand::Presence(next)) => {
                    live.presence = next;
                    *presence = next;
                    send_change_status(&client, live.steam_id, next).await?;
                    publish(snapshot, live.snapshot(chats), next, live.steam_id);
                }
                Some(SocialCommand::Playing { appid, name }) => {
                    let appid = appid.unwrap_or(0);
                    set_playing_appid(&playing, Some(appid));
                    if let Ok(mut slot) = playing_name.lock() {
                        *slot = name.clone();
                    }
                    if let Err(error) =
                        send_games_played(&client, live.steam_id, appid, &name).await
                    {
                        tracing::warn!("Steam : état de jeu non publié ({error})");
                    }
                }
                Some(SocialCommand::Redeem(key)) => {
                    live.redeem = Some(SteamKeyRedeem {
                        state: "running".into(),
                        message: "Activation demandée à Steam…".into(),
                        games: Vec::new(),
                        appids: Vec::new(),
                    });
                    publish(snapshot, live.snapshot(chats), live.presence, live.steam_id);
                    let mut body = Vec::new();
                    proto::field_bytes(1, key.trim().as_bytes(), &mut body);
                    send(&client, live.steam_id, CLIENT_REGISTER_KEY, &body).await?;
                    continue;
                }
                Some(SocialCommand::CreateLobby {
                    appid,
                    max_members,
                    lobby_type,
                    lobby_flags,
                    cell_id,
                    connect,
                    reply,
                }) => {
                    pending_lobby = Some((appid, reply));
                    let metadata: Vec<(&str, &str)> = connect
                        .as_deref()
                        .map(|address| vec![("connect", address)])
                        .unwrap_or_default();
                    let body = crate::lobby::create_lobby_body(
                        appid,
                        max_members,
                        lobby_type,
                        lobby_flags,
                        cell_id.unwrap_or(live.cell_id),
                        live.public_ip,
                        &live.persona_name,
                        &metadata,
                    );
                    let job = lobby_job;
                    lobby_job = lobby_job.wrapping_add(1);
                    tracing::info!(
                        "Steam : création de lobby demandée pour {appid} (job {job}, session {:?}, cellule {})",
                        session_id,
                        live.cell_id
                    );
                    tracing::debug!("Steam : corps création lobby = {:02x?}", body);
                    if let Err(error) = send_lobby(
                        &client,
                        live.steam_id,
                        session_id,
                        appid,
                        Some(job),
                        CLIENT_MMS_CREATE_LOBBY,
                        &body,
                    )
                    .await
                    {
                        tracing::warn!("Steam : lobby non demandé ({error})");
                    }
                }
                Some(SocialCommand::SetLobbyData {
                    appid,
                    lobby,
                    connect,
                }) => {
                    let body = crate::lobby::set_lobby_data_body(
                        appid,
                        lobby,
                        &[("connect", connect.as_str())],
                    );
                    let job = lobby_job;
                    lobby_job = lobby_job.wrapping_add(1);
                    tracing::info!(
                        "Steam : adresse {connect} publiée dans le lobby {lobby} (appid {appid})"
                    );
                    if let Err(error) = send_lobby(
                        &client,
                        live.steam_id,
                        session_id,
                        appid,
                        Some(job),
                        CLIENT_MMS_SET_LOBBY_DATA,
                        &body,
                    )
                    .await
                    {
                        tracing::warn!("Steam : métadonnées du lobby non publiées ({error})");
                    }
                }
                Some(SocialCommand::GetLobbyData { appid, lobby }) => {
                    let body = crate::lobby::get_lobby_data_body(appid, lobby);
                    let job = lobby_job;
                    lobby_job = lobby_job.wrapping_add(1);
                    if let Err(error) = send_lobby(
                        &client,
                        live.steam_id,
                        session_id,
                        appid,
                        Some(job),
                        CLIENT_MMS_GET_LOBBY_DATA,
                        &body,
                    )
                    .await
                    {
                        tracing::warn!(
                            "Steam : métadonnées du lobby {lobby} non demandées ({error})"
                        );
                    }
                }
                Some(SocialCommand::JoinLobby {
                    appid,
                    lobby,
                    reply,
                }) => {
                    if let Ok(mut slot) = PENDING_JOIN.lock() {
                        *slot = Some(reply);
                    }
                    // Une invitation reçue fait foi pour l'appid, le lobby et
                    // l'EMsg : c'est elle que le joueur accepte. Un lobby du
                    // jeu passe par le même chemin, avec l'appid demandé.
                    let job = lobby_job;
                    lobby_job = lobby_job.wrapping_add(1);
                    let mut sink = ClientJoinSink {
                        client: &client,
                        steam_id: live.steam_id,
                        session_id,
                        job: Some(job),
                    };
                    if let Err(error) =
                        emit_join(&mut sink, appid, lobby, &live.persona_name, live.cell_id).await
                    {
                        tracing::warn!("Steam : rejointe du lobby non demandée ({error})");
                        if let Ok(mut slot) = PENDING_JOIN.lock() {
                            if let Some(reply) = slot.take() {
                                let _ = reply.send(Err(2));
                            }
                        }
                    }
                }
                Some(SocialCommand::InviteToLobby {
                    appid,
                    lobby,
                    steam_id,
                    reply,
                }) => {
                    let body = crate::lobby::invite_to_lobby_body(appid, lobby, steam_id);
                    tracing::info!(
                        "Steam : invitation 6621 vers {steam_id} (appid {appid}, lobby {lobby}, corps={:02x?})",
                        body
                    );
                    match send_lobby(
                        &client,
                        live.steam_id,
                        session_id,
                        appid,
                        None,
                        CLIENT_MMS_INVITE_TO_LOBBY,
                        &body,
                    )
                    .await
                    {
                        Ok(()) => {
                            tracing::info!("Steam : ami {steam_id} invité dans {lobby}");
                            let _ = reply.send(Ok(()));
                        }
                        Err(error) => {
                            tracing::warn!("Steam : invitation non envoyée ({error})");
                            let _ = reply.send(Err(2));
                        }
                    }
                }
                Some(SocialCommand::Sticker { steam_id, name }) => {
                    let job = calls.next_job();
                    match send_friend_sticker(
                        &client,
                        live.steam_id,
                        session_id,
                        steam_id,
                        &name,
                        job,
                    )
                    .await
                    {
                        Err(error) => {
                            tracing::warn!("autocollant Steam non envoyé à {steam_id} : {error}");
                        }
                        Ok(()) => {
                            // Le serveur transforme la commande en balise
                            // `[sticker]` ; la copie locale emploie déjà cette
                            // forme pour que la bulle affiche l'image.
                            remember_chat(
                                chats,
                                steam_id,
                                SteamChatMessage {
                                    from: live.steam_id,
                                    text: sticker_bbcode(&name),
                                    at: now_unix(),
                                    outgoing: true,
                                    received_live: false,
                                },
                            );
                            publish(
                                snapshot,
                                live.snapshot(chats),
                                live.presence,
                                live.steam_id,
                            );
                        }
                    }
                }
                Some(SocialCommand::Message { steam_id, text }) => {
                    let job = calls.next_job();
                    match send_friend_message(
                        &client,
                        live.steam_id,
                        session_id,
                        steam_id,
                        &text,
                        &live.emoticons,
                        job,
                    )
                    .await
                    {
                        Err(error) => {
                            tracing::warn!("message Steam non envoyé à {steam_id} : {error}");
                        }
                        Ok(()) => {
                            remember_chat(
                                chats,
                                steam_id,
                                SteamChatMessage {
                                    from: live.steam_id,
                                    text,
                                    at: now_unix(),
                                    outgoing: true,
                                    received_live: false,
                                },
                            );
                            publish(
                                snapshot,
                                live.snapshot(chats),
                                live.presence,
                                live.steam_id,
                            );
                        }
                    }
                }
                Some(SocialCommand::History { steam_id, older }) => {
                    let already = calls
                        .history
                        .is_some_and(|(_, friend)| friend == steam_id)
                        || calls
                            .history_queue
                            .iter()
                            .any(|(friend, _)| *friend == steam_id);
                    if already {
                        continue;
                    }
                    if older
                        && !live
                            .history
                            .get(&steam_id)
                            .is_some_and(|state| state.more_available)
                    {
                        continue;
                    }
                    let state = live.history.entry(steam_id).or_default();
                    if older && state.cursor.is_none() {
                        state.loading = false;
                        state.more_available = false;
                        state.error =
                            Some("Steam n'a pas indiqué de page plus ancienne.".into());
                        publish(snapshot, live.snapshot(chats), live.presence, live.steam_id);
                        continue;
                    }
                    state.loading = true;
                    state.error = None;
                    publish(snapshot, live.snapshot(chats), live.presence, live.steam_id);
                    if calls.history.is_some() {
                        calls.history_queue.push_back((steam_id, older));
                    } else {
                        let cursor = if older {
                            live.history.get(&steam_id).and_then(|state| state.cursor)
                        } else {
                            None
                        };
                        let job = calls.next_job();
                        match request_history(
                        &client,
                        live.steam_id,
                        session_id,
                        job,
                        steam_id,
                        cursor,
                    )
                    .await
                    {
                            Ok(()) => calls.history = Some((job, steam_id)),
                            Err(error) => {
                                let state = live.history.entry(steam_id).or_default();
                                state.loading = false;
                                state.error = Some(error);
                                publish(snapshot, live.snapshot(chats), live.presence, live.steam_id);
                            }
                        }
                    }
                }
                Some(SocialCommand::Refresh) => {
                    // La liste est redemandée à l'API communautaire quand la
                    // session en a le jeton ; les états suivent par le CM.
                    if let Some(token) = access.clone() {
                        apply_remote_friend_list(&token, &mut live).await;
                    }
                    request_friend_data(&client, live.steam_id, live.friends.keys().copied())
                        .await?;
                    publish(snapshot, live.snapshot(chats), live.presence, live.steam_id);
                }
                None => return Ok(()),
            },
            // Le pool réclame la session unique : la rendre sans attendre,
            // sinon il se fait refuser (`mot de passe invalide`) et les copies
            // de sauvegarde échouent.
            _ = handoff.tick() => {
                if crate::shared::pool_wants_session() {
                    return Ok(());
                }
            },
            message = client.recv_msg() => {
                let message = message.map_err(|error| format!("session Steam interrompue : {error}"))?;
                for message in expand(message) {
                    if let Some(sid) = message.header.client_sessionid {
                        session_id = Some(sid);
                    }
                    // La réponse de création de lobby arrive avec son propre
                    // EMsg, corrélée par `app_id` (pas de jobid).
                    if message.emsg == EMsg(CLIENT_MMS_CREATE_LOBBY_RESPONSE) {
                        tracing::info!(
                            "Steam : en-tête réponse lobby session={:?} job_cible={:?} eresult={:?} msg={:?} corps={:02x?}",
                            message.header.client_sessionid,
                            message.header.jobid_target,
                            message.header.eresult,
                            message.header.error_message,
                            message.body
                        );
                        if let Some(created) = crate::lobby::parse_lobby_created(&message.body) {
                            tracing::info!(
                                "Steam : lobby {} créé pour appid {} (eresult {})",
                                created.lobby,
                                created.app_id,
                                created.eresult
                            );
                            if created.eresult == 1 && created.lobby != 0 {
                                if let Ok(mut slot) = CREATED_LOBBY.lock() {
                                    *slot = Some((created.app_id, created.lobby));
                                }
                            }
                            if let Some((appid, reply)) = pending_lobby.take() {
                                if appid == created.app_id {
                                    let verdict = if created.eresult == 1 && created.lobby != 0 {
                                        Ok(created.lobby)
                                    } else {
                                        tracing::warn!(
                                            "Steam : lobby refusé (eresult {}, lobby {})",
                                            created.eresult,
                                            created.lobby
                                        );
                                        Err(created.eresult)
                                    };
                                    let _ = reply.send(verdict);
                                } else {
                                    pending_lobby = Some((appid, reply));
                                }
                            }
                        } else {
                            tracing::warn!("Steam : réponse de lobby illisible");
                        }
                        continue;
                    }
                    // Invitation d'un ami : Steam relaie le message 6621 à
                    // l'invité. On la retient pour que l'interface puisse la
                    // proposer, au lieu de la laisser tomber.
                    if message.emsg == EMsg(CLIENT_MMS_INVITE_TO_LOBBY) {
                        match receive_invite(&message.body) {
                            Some(invite) => {
                                tracing::info!(
                                    "Steam : invitation reçue pour appid {} (lobby {}, de {})",
                                    invite.app_id,
                                    invite.lobby,
                                    invite.inviter
                                );
                                // L'adresse de la partie vit dans les métadonnées
                                // du lobby : sans cette lecture, l'invité n'a
                                // rien à transmettre au lancement.
                                let body =
                                    crate::lobby::get_lobby_data_body(invite.app_id, invite.lobby);
                                let job = lobby_job;
                                lobby_job = lobby_job.wrapping_add(1);
                                if let Err(error) = send_lobby(
                                    &client,
                                    live.steam_id,
                                    session_id,
                                    invite.app_id,
                                    Some(job),
                                    CLIENT_MMS_GET_LOBBY_DATA,
                                    &body,
                                )
                                .await
                                {
                                    tracing::warn!(
                                        "Steam : adresse du lobby {} non demandée ({error})",
                                        invite.lobby
                                    );
                                }
                            }
                            None => tracing::warn!("Steam : invitation illisible"),
                        }
                        continue;
                    }
                    // Entrée ou sortie d'un membre : la liste du lobby rejoint
                    // suit l'état réel de la session.
                    if message.emsg == EMsg(CLIENT_MMS_USER_JOINED_LOBBY)
                        || message.emsg == EMsg(CLIENT_MMS_USER_LEFT_LOBBY)
                    {
                        let enters = message.emsg == EMsg(CLIENT_MMS_USER_JOINED_LOBBY);
                        match crate::lobby::parse_member_event(&message.body) {
                            Some(event) => {
                                let applied = apply_member_event(&event, enters);
                                if applied {
                                    tracing::info!(
                                        "Steam : {} {} le lobby {} ({} membre(s))",
                                        event.member.persona_name,
                                        if enters { "rejoint" } else { "quitte" },
                                        event.lobby,
                                        lobby_members(event.lobby).len()
                                    );
                                } else {
                                    tracing::debug!(
                                        "Steam : membre annoncé pour le lobby {} non rejoint",
                                        event.lobby
                                    );
                                }
                            }
                            None => tracing::warn!("Steam : notification de membre illisible"),
                        }
                        continue;
                    }
                    // Réponse de rejointe : le verdict, le lobby effectif et
                    // les membres. Un refus ne doit rien enregistrer.
                    if message.emsg == EMsg(CLIENT_MMS_JOIN_LOBBY_RESPONSE) {
                        let answer = PENDING_JOIN.lock().ok().and_then(|mut slot| slot.take());
                        match crate::lobby::parse_join_lobby_response(&message.body) {
                            Some(joined) => {
                                let accepted = join_verdict(&joined).is_ok();
                                if accepted {
                                    record_join_result(&joined);
                                    tracing::info!(
                                        "Steam : lobby {} rejoint ({} membre(s), {} place(s))",
                                        joined.lobby,
                                        joined.members.len(),
                                        joined.max_members
                                    );
                                } else {
                                    tracing::warn!(
                                        "Steam : rejointe du lobby refusée (eresult {}, lobby {})",
                                        joined.eresult,
                                        joined.lobby
                                    );
                                }
                                if let Some(reply) = answer {
                                    let _ = if accepted {
                                        reply.send(Ok(joined))
                                    } else {
                                        reply.send(Err(joined.eresult))
                                    };
                                }
                            }
                            None => {
                                tracing::warn!("Steam : réponse de rejointe illisible");
                                if let Some(reply) = answer {
                                    let _ = reply.send(Err(2));
                                }
                            }
                        }
                        continue;
                    }
                    // Métadonnées d'un lobby : le CM rend les paires posées à
                    // la création ou par SetLobbyData. L'adresse `connect`
                    // rejoint le registre lu par le daemon.
                    if message.emsg == EMsg(CLIENT_MMS_LOBBY_DATA) {
                        match crate::lobby::parse_lobby_data(&message.body) {
                            Some(data) => match data.connect() {
                                Some(connect) => {
                                    tracing::info!(
                                        "Steam : lobby {} annonce la partie à {connect}",
                                        data.lobby
                                    );
                                    remember_lobby_connect(data.lobby, &connect);
                                }
                                None => tracing::info!(
                                    "Steam : lobby {} sans adresse de partie",
                                    data.lobby
                                ),
                            },
                            None => tracing::warn!("Steam : métadonnées de lobby illisibles"),
                        }
                        continue;
                    }
                    // Verdict d'une publication de métadonnées.
                    if message.emsg == EMsg(CLIENT_MMS_SET_LOBBY_DATA_RESPONSE) {
                        tracing::info!(
                            "Steam : métadonnées de lobby publiées (eresult {:?})",
                            message.header.eresult
                        );
                        continue;
                    }
                    // Réponse à un appel de service : elle est reconnue par son
                    // identifiant de job, jamais confondue avec une poussée.
                    if message.emsg == EMsg::SERVICE_METHOD_RESPONSE {
                        match message.header.jobid_target {
                            Some(job) if Some(job) == calls.renew => {
                                calls.renew = None;
                                if eresult_ok(&message) {
                                    access = apply_tokens(session, &message.body);
                                    // Le CM ne pousse la liste d'amis qu'une
                                    // fois, groupée avec la réponse au logon :
                                    // l'API communautaire la rend à la demande,
                                    // avec le jeton d'accès de la session.
                                    if let Some(token) = access.clone() {
                                        if apply_remote_friend_list(&token, &mut live).await {
                                            friends_asked = true;
                                            request_friend_data(
                                                &client,
                                                live.steam_id,
                                                live.friends.keys().copied(),
                                            )
                                            .await?;
                                        }
                                        if let Some(web) =
                                            fetch_web_notifications(&token, live.steam_id, &mut live.wishlist_deals).await
                                        {
                                            live.web_notifications = web;
                                        }
                                        publish(
                                            snapshot,
                                            live.snapshot(chats),
                                            live.presence,
                                            live.steam_id,
                                        );
                                    }
                                } else {
                                    tracing::debug!(
                                        "entretien du jeton refusé ({:?}) : la session enregistrée reste valable",
                                        message.header.eresult
                                    );
                                }
                                continue;
                            }
                            Some(job) if Some(job) == calls.emoticons => {
                                calls.emoticons = None;
                                if eresult_ok(&message) {
                                    live.emoticons = parse_emoticons(&message.body);
                                    tracing::info!(
                                        "Steam : {} emote(s) possédée(s)",
                                        live.emoticons.len()
                                    );
                                } else {
                                    tracing::debug!(
                                        "Steam : liste d'emotes refusée ({:?})",
                                        message.header.eresult
                                    );
                                }
                                publish(snapshot, live.snapshot(chats), live.presence, live.steam_id);
                                continue;
                            }
                            Some(job) if Some(job) == calls.family => {
                                calls.family = None;
                                live.family = eresult_ok(&message)
                                    .then(|| parse_family(&message.body, live.steam_id))
                                    .flatten();
                                // La réponse ne porte que des identifiants : les
                                // noms et les photos se demandent aux fiches de
                                // joueur, avec les amis comme pour tout le monde.
                                let members: Vec<u64> = live
                                    .family
                                    .as_ref()
                                    .map(|family| {
                                        family
                                            .members
                                            .iter()
                                            .map(|member| member.steam_id)
                                            .collect()
                                    })
                                    .unwrap_or_default();
                                if !members.is_empty() {
                                    request_friend_data(&client, live.steam_id, members.into_iter())
                                        .await?;
                                }
                                // La bibliothèque partagée dit, pour chaque
                                // jeu, qui le possède : c'est de là que vient
                                // le nombre de jeux de chaque membre.
                                if let Some(group) = live
                                    .family
                                    .as_ref()
                                    .map(|family| family.group_id)
                                    .filter(|group| *group != 0)
                                {
                                    let job = calls.next_job();
                                    let mut request = Vec::new();
                                    proto::field_fixed64(1, group, &mut request);
                                    // `include_own` : les jeux du compte
                                    // connecté comptent aussi.
                                    proto::field_varint(2, 1, &mut request);
                                    start_service(
                                        &client,
                                        live.steam_id,
                                        session_id,
                                        job,
                                        "FamilyGroups.GetSharedLibraryApps#1",
                                        &request,
                                    )
                                    .await?;
                                    calls.shared = Some(job);
                                }
                                continue;
                            }
                            Some(job) if Some(job) == calls.shared => {
                                calls.shared = None;
                                if eresult_ok(&message) {
                                    if let Some(family) = live.family.as_mut() {
                                        apply_shared_apps(&message.body, family);
                                    }
                                }
                                publish(snapshot, live.snapshot(chats), live.presence, live.steam_id);
                                continue;
                            }
                            Some(job)
                                if calls
                                    .history
                                    .is_some_and(|(pending, _)| pending == job) =>
                            {
                                let friend = calls.history.take().map(|(_, friend)| friend);
                                if let Some(friend) = friend {
                                    if eresult_ok(&message) {
                                        let HistoryPage {
                                            messages,
                                            more_available,
                                            cursor,
                                        } = parse_history(&message.body, live.steam_id, friend);
                                        let added = merge_history(chats, friend, messages);
                                        let state = live.history.entry(friend).or_default();
                                        state.loading = false;
                                        state.error = None;
                                        state.cursor = cursor;
                                        state.more_available =
                                            more_available && cursor.is_some();
                                        tracing::info!(
                                            "Steam : historique de {friend} : {added} message(s) ajouté(s), page suivante {}",
                                            state.more_available
                                        );
                                    } else {
                                        let state = live.history.entry(friend).or_default();
                                        state.loading = false;
                                        state.more_available = false;
                                        state.error = Some(format!(
                                            "Steam a refusé l'historique (eresult {:?})",
                                            message.header.eresult.unwrap_or(0)
                                        ));
                                    }
                                }
                                publish(snapshot, live.snapshot(chats), live.presence, live.steam_id);
                                if let Some((friend, older)) = calls.history_queue.pop_front() {
                                    let cursor = if older {
                                        live.history.get(&friend).and_then(|state| state.cursor)
                                    } else {
                                        None
                                    };
                                    let state = live.history.entry(friend).or_default();
                                    state.loading = true;
                                    state.error = None;
                                    if older && cursor.is_none() {
                                        state.loading = false;
                                        state.more_available = false;
                                        state.error = Some(
                                            "Steam n'a pas indiqué de page plus ancienne.".into(),
                                        );
                                        publish(
                                            snapshot,
                                            live.snapshot(chats),
                                            live.presence,
                                            live.steam_id,
                                        );
                                        continue;
                                    }
                                    let job = calls.next_job();
                                    match request_history(
                                        &client,
                                        live.steam_id,
                                        session_id,
                                        job,
                                        friend,
                                        cursor,
                                    )
                                    .await
                                    {
                                        Ok(()) => calls.history = Some((job, friend)),
                                        Err(error) => {
                                            let state = live.history.entry(friend).or_default();
                                            state.loading = false;
                                            state.error = Some(error);
                                            publish(
                                                snapshot,
                                                live.snapshot(chats),
                                                live.presence,
                                                live.steam_id,
                                            );
                                        }
                                    }
                                }
                                continue;
                            }
                            _ => {}
                        }
                    }
                    tracing::debug!(
                        "CM → {} ({} octets)",
                        emsg_name(message.emsg.0),
                        message.body.len()
                    );
                    match message.emsg.0 {
                        CLIENT_ACCOUNT_INFO => {
                            if let Ok(info) = CMsgClientAccountInfo::decode(&*message.body) {
                                if let Some(name) = info.persona_name.filter(|n| !n.is_empty()) {
                                    live.persona_name = name;
                                }
                            }
                        }
                        CLIENT_FRIENDS_LIST => {
                            if let Ok(list) = CMsgClientFriendsList::decode(&*message.body) {
                                tracing::debug!(
                                    "liste d'amis : {} relations, incrémentale {}",
                                    list.friends.len(),
                                    list.bincremental.unwrap_or(false)
                                );
                                apply_friends_list(&mut live, &list);
                                tracing::debug!("amis retenus : {}", live.friends.len());
                                // Steam ne pousse l'état que des amis en ligne :
                                // il faut demander les autres explicitement,
                                // une fois la liste reçue.
                                if !friends_asked {
                                    friends_asked = true;
                                    request_friend_data(
                                        &client,
                                        live.steam_id,
                                        live.friends.keys().copied(),
                                    )
                                    .await?;
                                }
                                if !collection_asked {
                                    collection_asked = true;
                                    if let Err(error) = send(
                                        &client,
                                        live.steam_id,
                                        CLIENT_GET_EMOTICON_LIST,
                                        &[],
                                    )
                                    .await
                                    {
                                        tracing::warn!(
                                            "Steam : collection redemandée sans succès ({error})"
                                        );
                                    }
                                }
                            }
                        }
                        CLIENT_PERSONA_STATE => {
                            if let Ok(state) = CMsgClientPersonaState::decode(&*message.body) {
                                if apply_persona_state(&mut live, &state) > 0 {
                                    publish(
                                        snapshot,
                                        live.snapshot(chats),
                                        live.presence,
                                        live.steam_id,
                                    );
                                }
                            }
                        }
                        CLIENT_USER_NOTIFICATIONS => {
                            if let Ok(notifications) = CMsgClientUserNotifications::decode(&*message.body) {
                                let previous: HashMap<u32, (u32, i64)> = live
                                    .notifications
                                    .iter()
                                    .map(|notification| {
                                        (notification.kind, (notification.count, notification.at))
                                    })
                                    .collect();
                                let now = unix_now();
                                live.notifications = notifications
                                    .notifications
                                    .iter()
                                    .filter(|notification| notification.count.unwrap_or(0) > 0)
                                    .map(|notification| {
                                        let kind = notification.user_notification_type.unwrap_or(0);
                                        let count = notification.count.unwrap_or(0);
                                        // Le compteur n'a pas d'heure Steam : garder celle
                                        // de son apparition tant que le nombre ne change pas.
                                        let at = previous
                                            .get(&kind)
                                            .filter(|(old_count, at)| *old_count == count && *at > 0)
                                            .map(|(_, at)| *at)
                                            .unwrap_or(now);
                                        SteamNotification { kind, count, at }
                                    })
                                    .collect();
                            }
                        }
                        CLIENT_COMMENT_NOTIFICATIONS => {
                            if let Ok(comments) = CMsgClientCommentNotifications::decode(&*message.body) {
                                live.new_comments = comments.count_new_comments_owner.unwrap_or(0)
                                    .max(comments.count_new_comments.unwrap_or(0));
                            }
                        }
                        CLIENT_FRIEND_MSG_INCOMING => {
                            if let Some(message) = receive_friend_message(chats, &message.body) {
                                tracing::debug!("Steam : message reçu de {}", message.from);
                                publish(
                                    snapshot,
                                    live.snapshot(chats),
                                    live.presence,
                                    live.steam_id,
                                );
                            }
                        }
                        CLIENT_EMOTICON_LIST => {
                            let (emoticons, stickers) = parse_client_emoticon_list(&message.body);
                            // La réponse 9331 remplace la liste du service : elle
                            // porte la collection complète du compte.
                            if !emoticons.is_empty() {
                                live.emoticons = emoticons;
                            }
                            live.stickers = stickers;
                            tracing::info!(
                                "Steam : {} emote(s) et {} autocollant(s) possédé(s)",
                                live.emoticons.len(),
                                live.stickers.len()
                            );
                        }
                        CLIENT_SET_HEARTBEAT_RATE => {
                            // Le corps donne l'intervalle en secondes ; Steam
                            // ferme la session si on ne le respecte pas.
                            let seconds = first_varint(&message.body).unwrap_or(15).clamp(5, 120);
                            heartbeat = tokio::time::interval(Duration::from_secs(seconds));
                            heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                        }
                        CLIENT_PURCHASE_RESPONSE => {
                            live.redeem = Some(parse_purchase(&message.body));
                            publish(snapshot, live.snapshot(chats), live.presence, live.steam_id);
                        }
                        CLIENT_PING => {
                            client.send_heartbeat().await.map_err(|error| error.to_string())?;
                        }
                        CLIENT_LOGGED_OFF => {
                            return Err("session fermée par Steam".into());
                        }
                        _ => {}
                    }
                }
                publish(snapshot, live.snapshot(chats), live.presence, live.steam_id);

                // La famille n'est demandée qu'une fois la liste d'amis
                // connue : la réponse arrive comme les autres, sans rien
                // interrompre.
                if !family_done && friends_asked {
                    family_done = true;
                    let job = calls.next_job();
                    let mut request = Vec::new();
                    proto::field_varint(1, live.steam_id, &mut request);
                    proto::field_varint(2, 1, &mut request);
                    start_service(
                        &client,
                        live.steam_id,
                        session_id,
                        job,
                        "FamilyGroups.GetFamilyGroupForUser#1",
                        &request,
                    )
                    .await?;
                    calls.family = Some(job);
                }
            },
            _ = heartbeat.tick() => {
                client.send_heartbeat().await.map_err(|error| format!("battement : {error}"))?;
            }
            _ = notifications_tick.tick() => {
                if let Some(token) = access.as_deref() {
                    if let Some(web) =
                        fetch_web_notifications(token, live.steam_id, &mut live.wishlist_deals).await
                    {
                        if web != live.web_notifications {
                            live.web_notifications = web;
                            publish(snapshot, live.snapshot(chats), live.presence, live.steam_id);
                        }
                    }
                }
            }
        }
    }
}

/// Appels de service en attente : jamais plus d'un renouvellement et d'une
/// demande de famille à la fois. L'historique d'une conversation peut attendre
/// son tour dans `history_queue`.
struct Calls {
    next: u64,
    renew: Option<u64>,
    family: Option<u64>,
    /// Bibliothèque partagée de la famille, demandée après le groupe.
    shared: Option<u64>,
    /// Appel d'historique en cours : (job, ami).
    history: Option<(u64, u64)>,
    /// Conversations et page demandée dont l'historique attend la fin de
    /// l'appel courant.
    history_queue: VecDeque<(u64, bool)>,
    /// Appel de `Player.GetEmoticonList` en cours.
    emoticons: Option<u64>,
}

impl Calls {
    fn next_job(&mut self) -> u64 {
        self.next += 1;
        self.next
    }
}

/// Envoie un appel de service sans attendre sa réponse : la boucle d'écoute
/// reconnaîtra `SERVICE_METHOD_RESPONSE` par son identifiant de job. Attendre
/// sur place avalerait les poussées reçues entre-temps.
async fn start_service(
    client: &SteamClient<LoggedIn>,
    steam_id: u64,
    session_id: Option<i32>,
    job: u64,
    method: &str,
    request: &[u8],
) -> Result<(), String> {
    let mut message = ClientMsg::with_body(EMsg::SERVICE_METHOD_CALL_FROM_CLIENT, request);
    message.header.target_job_name = Some(method.to_string());
    message.header.jobid_source = Some(job);
    // Un appel de service porte l'identité de la session : le CM refuse ce
    // qu'il ne peut pas rattacher à un compte connecté. Le client Steam pose
    // aussi `client_sessionid`, indispensable aux méthodes de chat.
    message.header.steamid = Some(steam_id);
    message.header.client_sessionid = session_id;
    client
        .send_msg(&message)
        .await
        .map_err(|error| format!("appel à {method} refusé : {error}"))
}

/// Corps de `FriendMessages.GetRecentMessages#1` : les deux interlocuteurs, la
/// fenêtre complète et le nombre de messages demandés. Steam répond du plus
/// récent au plus ancien.
///
/// `rtime32_start_time` est indispensable pour dépasser la fenêtre récente que
/// Steam applique quand le champ vaut zéro.
fn history_request(steam_id: u64, friend: u64, cursor: Option<(u32, u32)>) -> Vec<u8> {
    let mut request = Vec::new();
    proto::field_fixed64(1, steam_id, &mut request);
    proto::field_fixed64(2, friend, &mut request);
    proto::field_varint(3, u64::from(HISTORY_MESSAGES), &mut request);
    proto::field_fixed32(5, HISTORY_START_TIME, &mut request);
    if let Some((time_last, ordinal_last)) = cursor {
        proto::field_varint(8, u64::from(time_last), &mut request);
        proto::field_varint(9, u64::from(ordinal_last), &mut request);
    }
    request
}

/// Demande l'historique sans attendre : la boucle d'écoute reconnaîtra la
/// réponse par le job, comme pour la famille et le renouvellement du jeton.
async fn request_history(
    client: &SteamClient<LoggedIn>,
    steam_id: u64,
    session_id: Option<i32>,
    job: u64,
    friend: u64,
    cursor: Option<(u32, u32)>,
) -> Result<(), String> {
    start_service(
        client,
        steam_id,
        session_id,
        job,
        FRIEND_MESSAGES_RECENT,
        &history_request(steam_id, friend, cursor),
    )
    .await
}

/// Développe une trame reçue : Steam groupe souvent plusieurs messages dans un
/// `Multi`, en particulier la première poussée qui suit le logon (compte,
/// liste d'amis, états). Chaque sous-message redevient une trame ordinaire.
fn expand(message: IncomingMsg) -> Vec<IncomingMsg> {
    if message.emsg != EMsg::MULTI {
        return vec![message];
    }
    let payloads = match multi::unpack_multi(&message.body) {
        Ok(payloads) => payloads,
        Err(error) => {
            tracing::warn!("lot de messages illisible : {error}");
            return Vec::new();
        }
    };
    let mut messages = Vec::with_capacity(payloads.len());
    for payload in payloads {
        match parse_payload(&payload) {
            Some(inner) => messages.push(inner),
            None => tracing::warn!("message de lot illisible ({} octets)", payload.len()),
        }
    }
    if messages.len() > 1 {
        tracing::debug!("lot reçu : {} messages", messages.len());
    }
    messages
}

/// Trame d'un sous-message : même forme qu'une trame directe, la magie et la
/// longueur en moins.
fn parse_payload(payload: &[u8]) -> Option<IncomingMsg> {
    match PacketHeader::parse(payload).ok()? {
        PacketHeader::Protobuf { header, body } => Some(IncomingMsg {
            emsg: header.emsg,
            is_protobuf: true,
            header: header.decode_header().unwrap_or_default(),
            body,
        }),
        PacketHeader::Simple { header, body } => Some(IncomingMsg {
            emsg: header.emsg,
            is_protobuf: false,
            header: CMsgProtoBufHeader::default(),
            body,
        }),
        PacketHeader::Extended { header, body } => Some(IncomingMsg {
            emsg: header.emsg,
            is_protobuf: false,
            header: CMsgProtoBufHeader {
                steamid: Some(header.steam_id),
                client_sessionid: Some(header.session_id),
                ..CMsgProtoBufHeader::default()
            },
            body,
        }),
    }
}

/// Un `eresult` absent vaut succès dans le protocole Steam (`1` = OK).
fn eresult_ok(message: &IncomingMsg) -> bool {
    matches!(message.header.eresult, None | Some(1))
}

impl Live {
    fn snapshot(&self, chats: &Arc<Mutex<BTreeMap<u64, Vec<SteamChatMessage>>>>) -> SteamSocial {
        SteamSocial {
            // Le daemon remplit les invitations à la lecture : la session les
            // garde hors de son instantané.
            invites: Vec::new(),
            state: "live".into(),
            error: None,
            persona_name: self.persona_name.clone(),
            steam_id: self.steam_id,
            presence: self.presence,
            applied_presence: self.applied,
            friends: self.friends.values().cloned().collect(),
            notifications: self.notifications.clone(),
            web_notifications: self.web_notifications.clone(),
            friend_events: self.friend_events.clone(),
            chats: chats.lock().map(|chats| chats.clone()).unwrap_or_default(),
            history: self.history.clone(),
            emoticons: self.emoticons.clone(),
            stickers: self.stickers.clone(),
            new_comments: self.new_comments,
            redeem: self.redeem.clone(),
            // L'historique des achats n'appartient pas à la session : le
            // daemon le joint à l'instantané qu'il publie.
            family: self.family.clone().map(|mut family| {
                // Le groupe familial ne publie ni nom ni photo : ils viennent
                // de la fiche de joueur de chaque membre, connue dès que
                // Steam l'a poussée.
                for member in &mut family.members {
                    let Some(friend) = self.friends.get(&member.steam_id) else {
                        continue;
                    };
                    if member.name.trim().is_empty() {
                        member.name = friend.name.clone();
                    }
                    if member.avatar.trim().is_empty() {
                        member.avatar = friend.avatar.clone();
                    }
                }
                family
            }),
            updated: now_unix(),
        }
    }
}

/// Nom lisible des messages qui comptent pour le diagnostic.
fn emsg_name(emsg: u32) -> String {
    let name = match emsg {
        CLIENT_ACCOUNT_INFO => "ClientAccountInfo",
        CLIENT_GAMES_PLAYED => "ClientGamesPlayed",
        CLIENT_FRIEND_MSG => "ClientFriendMsg",
        CLIENT_FRIEND_MSG_INCOMING => "ClientFriendMsgIncoming",
        CLIENT_GET_EMOTICON_LIST => "ClientGetEmoticonList",
        CLIENT_EMOTICON_LIST => "ClientEmoticonList",
        CLIENT_CHANGE_STATUS => "ClientChangeStatus",
        CLIENT_FRIENDS_LIST => "ClientFriendsList",
        CLIENT_PERSONA_STATE => "ClientPersonaState",
        CLIENT_REQUEST_FRIEND_DATA => "ClientRequestFriendData",
        CLIENT_USER_NOTIFICATIONS => "ClientUserNotifications",
        CLIENT_COMMENT_NOTIFICATIONS => "ClientCommentNotifications",
        CLIENT_LOGGED_OFF => "ClientLoggedOff",
        CLIENT_SET_HEARTBEAT_RATE => "ClientSetHeartbeatRate",
        CLIENT_PING => "ClientPing",
        _ => return format!("emsg {emsg}"),
    };
    name.to_string()
}

/// Envoie un message protobuf au CM, estampillé du compte connecté.
///
/// Sans le `steamid` dans l'en-tête, le CM ne rattache pas le message à la
/// session : les poussées attendues (états des amis, présence appliquée) ne
/// viennent jamais.
async fn send(
    client: &SteamClient<LoggedIn>,
    steam_id: u64,
    emsg: u32,
    body: &[u8],
) -> Result<(), String> {
    let mut message = ClientMsg::with_body(EMsg(emsg), body);
    message.header.steamid = Some(steam_id);
    client
        .send_msg(&message)
        .await
        .map_err(|error| format!("envoi Steam refusé : {error}"))
}

/// Envoie un message de matchmaking avec les en-têtes du client Steam.
///
/// `routing_appid` rattache la requête au jeu ; `jobid_source` permet au CM
/// de corréler sa réponse ; `client_sessionid` vient du logon. SteamKit les
/// pose pour chaque message MMS ; sans eux le CM répond `eresult 2`.
fn lobby_message(
    steam_id: u64,
    session_id: Option<i32>,
    appid: u32,
    job: Option<u64>,
    emsg: u32,
    body: &[u8],
) -> ClientMsg<'_> {
    let mut message = ClientMsg::with_body(EMsg(emsg), body);
    message.header.steamid = Some(steam_id);
    message.header.routing_appid = Some(appid);
    if let Some(session_id) = session_id {
        message.header.client_sessionid = Some(session_id);
    }
    if let Some(job) = job {
        message.header.jobid_source = Some(job);
    }
    message
}

async fn send_lobby(
    client: &SteamClient<LoggedIn>,
    steam_id: u64,
    session_id: Option<i32>,
    appid: u32,
    job: Option<u64>,
    emsg: u32,
    body: &[u8],
) -> Result<(), String> {
    let message = lobby_message(steam_id, session_id, appid, job, emsg, body);
    client
        .send_msg(&message)
        .await
        .map_err(|error| format!("envoi Steam refusé : {error}"))
}

/// Corps de `CMsgClientFriendMsg` : le destinataire, le type texte et le
/// message. Les champs 4 (`rtime32_server_timestamp`, fixed32) et 5
/// (`echo_to_sender`, bool) restent absents : Steam estampille le message, et
/// Lapinux ajoute lui-même sa copie locale sans attendre d'écho. Écrire un
/// varint au champ 4 produisait un message que le décodeur protobuf refusait.
fn friend_msg_message(destination: u64, text: &str) -> Vec<u8> {
    let mut body = Vec::new();
    proto::field_fixed64(1, destination, &mut body);
    // `EChatEntryType::ChatMsg`.
    proto::field_varint(2, 1, &mut body);
    proto::field_bytes(3, text.as_bytes(), &mut body);
    body
}

/// Lit `CMsgClientFriendMsgIncoming` : l'auteur (1), le type (2), le texte
/// (4) et l'horodatage (5). Le champ 3 est `from_limited_account`, pas du
/// texte : le lire comme message laissait passer les vrais envois.
fn parse_incoming_friend_msg(body: &[u8]) -> Option<(u64, String, i64)> {
    let mut from = 0u64;
    let mut kind = 1i32;
    let mut text: Option<Vec<u8>> = None;
    let mut at = 0i64;
    let mut reader = proto::Reader::new(body);
    while let Some((number, field)) = reader.next() {
        match (number, field) {
            (1, proto::Field::Fixed64(steam_id)) => from = steam_id,
            (2, proto::Field::Varint(entry)) => kind = entry as i32,
            (4, proto::Field::Bytes(message)) => {
                text = Some(message.to_vec());
            }
            (5, proto::Field::Fixed32(timestamp)) => at = i64::from(timestamp),
            _ => {}
        }
    }
    if from == 0 || !matches!(kind, 0 | 1) {
        return None;
    }
    // Le client Steam peut laisser un octet NUL final ; il ne fait pas
    // partie du message affiché.
    let text = String::from_utf8_lossy(&text?)
        .trim_matches('\0')
        .trim()
        .to_string();
    if text.is_empty() {
        return None;
    }
    Some((from, text, at))
}

/// Corps de `FriendMessages.SendMessage#1` : destinataire (1), type texte
/// (2), message (3) et indicateur BBCode (4). L'indicateur vrai demande à
/// Steam d'interpréter les shortcodes et les commandes du message ; les
/// crochets littéraux doivent donc avoir été échappés par l'appelant.
fn friend_messages_send(destination: u64, text: &str, contains_bbcode: bool) -> Vec<u8> {
    let mut body = Vec::new();
    proto::field_fixed64(1, destination, &mut body);
    // `EChatEntryType::ChatMsg`.
    proto::field_varint(2, 1, &mut body);
    proto::field_bytes(3, text.as_bytes(), &mut body);
    if contains_bbcode {
        proto::field_varint(4, 1, &mut body);
    }
    body
}

/// Un shortcode complet, dans la forme ASCII ou avec le deux-points
/// modificatif U+02D0 que certains outils Steam emploient.
fn emote_shortcode(word: &str) -> Option<&str> {
    let name = word
        .strip_prefix(':')
        .or_else(|| word.strip_prefix('\u{2d0}'))?;
    let name = name
        .strip_suffix(':')
        .or_else(|| name.strip_suffix('\u{2d0}'))?;
    if name.is_empty()
        || name.len() > 48
        || !name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        return None;
    }
    Some(name)
}

/// Échappe les crochets littéraux : le serveur les lirait sinon comme du
/// BBCode. Même règle que le client Steam, qui préfixe `[` d'un antislash.
fn escape_bbcode(text: &str) -> String {
    text.replace('[', "\\[")
}

/// Vrai si le message contient le shortcode d'une emote possédée. Le
/// shortcode reste intact : Steam le traduit lui-même en balise au moment de
/// l'envoi, contrairement aux balises précomposées qu'il retire.
fn has_owned_emote(text: &str, emoticons: &[SteamEmoticon]) -> bool {
    let names: HashSet<&str> = emoticons
        .iter()
        .map(|emote| emote.name.as_str())
        .filter(|name| !name.is_empty())
        .collect();
    if names.is_empty() {
        return false;
    }
    let punctuation = [',', '.', '!', '?', ';', ')', '(', '"', '\''];
    text.split_whitespace().any(|word| {
        let trimmed = word.trim_matches(|character: char| punctuation.contains(&character));
        emote_shortcode(trimmed).is_some_and(|name| names.contains(name))
    })
}

/// Prépare un message contenant une emote : les crochets littéraux sont
/// échappés, mais les shortcodes `:nom:` sont laissés au serveur. `None`
/// signifie qu'aucune emote possédée n'est présente, et le message garde
/// l'ancien chemin sans exposer ses crochets au parseur BBCode.
fn bbcoded_message(text: &str, emoticons: &[SteamEmoticon]) -> Option<String> {
    has_owned_emote(text, emoticons).then(|| escape_bbcode(text))
}

/// Forme locale d'un autocollant envoyé : la même balise que celle que Steam
/// diffuse aux destinataires.
fn sticker_bbcode(name: &str) -> String {
    format!("[sticker type=\"{name}\"][/sticker]")
}

/// Envoie `/sticker nom` par le chemin moderne, avec BBCode déclaré : c'est
/// ce qui fait convertir la commande en autocollant par Steam.
async fn send_friend_sticker(
    client: &SteamClient<LoggedIn>,
    source: u64,
    session_id: Option<i32>,
    destination: u64,
    name: &str,
    job: u64,
) -> Result<(), String> {
    start_service(
        client,
        source,
        session_id,
        job,
        FRIEND_MESSAGES_SEND,
        &friend_messages_send(destination, &format!("/sticker {name}"), true),
    )
    .await
}

/// Envoie un message texte au CM. Dès qu'il contient une emote possédée, il
/// passe par `FriendMessages.SendMessage#1` en déclarant le BBCode : Steam
/// transforme alors le shortcode en emoticon pour le destinataire.
async fn send_friend_message(
    client: &SteamClient<LoggedIn>,
    source: u64,
    session_id: Option<i32>,
    destination: u64,
    text: &str,
    emoticons: &[SteamEmoticon],
    job: u64,
) -> Result<(), String> {
    if let Some(prepared) = bbcoded_message(text, emoticons) {
        match start_service(
            client,
            source,
            session_id,
            job,
            FRIEND_MESSAGES_SEND,
            // Le shortcode reste dans le texte : avec `contains_bbcode`,
            // Steam le convertit lui-même en emoticon. Les balises
            // précomposées, elles, finissent réduites au nom de l'emote.
            &friend_messages_send(destination, &prepared, true),
        )
        .await
        {
            Ok(()) => return Ok(()),
            // Le service moderne peut être indisponible : le message part
            // quand même en texte simple plutôt que de rester dans le champ.
            Err(error) => {
                tracing::warn!("Steam : envoi BBCode refusé ({error}), repli sur un message texte")
            }
        }
    }
    send(
        client,
        source,
        CLIENT_FRIEND_MSG,
        &friend_msg_message(destination, text),
    )
    .await
}

/// Ajoute un message à l'historique de la session et borne la conversation.
fn remember_chat(
    chats: &Arc<Mutex<BTreeMap<u64, Vec<SteamChatMessage>>>>,
    friend: u64,
    message: SteamChatMessage,
) {
    let Ok(mut chats) = chats.lock() else {
        return;
    };
    let conversation = chats.entry(friend).or_default();
    conversation.push(message);
    if conversation.len() > CHAT_MESSAGES {
        let excess = conversation.len() - CHAT_MESSAGES;
        conversation.drain(0..excess);
    }
}

/// Traite un message reçu d'un ami (5427) : le décode et l'ajoute à la
/// conversation. C'est le seul chemin d'entrée des vrais messages Steam.
fn receive_friend_message(
    chats: &Arc<Mutex<BTreeMap<u64, Vec<SteamChatMessage>>>>,
    body: &[u8],
) -> Option<SteamChatMessage> {
    let (from, text, at) = parse_incoming_friend_msg(body)?;
    let message = SteamChatMessage {
        from,
        text,
        at: if at > 0 { at } else { now_unix() },
        outgoing: false,
        received_live: true,
    };
    remember_chat(chats, from, message.clone());
    Some(message)
}

/// Une page de `CFriendMessages_GetRecentMessages_Response`.
///
/// Le curseur reprend l'heure et l'ordinal du plus ancien message de la page :
/// Steam les attend dans `time_last` et `ordinal_last` pour rendre la page
/// précédente. Il n'est pas exposé à l'interface.
struct HistoryPage {
    messages: Vec<SteamChatMessage>,
    more_available: bool,
    cursor: Option<(u32, u32)>,
}

/// Lit `CFriendMessages_GetRecentMessages_Response` : les messages reviennent
/// du plus récent au plus ancien, avec l'accountid de leur auteur. Les deux
/// interlocuteurs sont ramenés à leurs SteamID64 et au sens de la conversation.
fn parse_history(body: &[u8], steam_id: u64, friend: u64) -> HistoryPage {
    let self_account = (steam_id & 0xFFFF_FFFF) as u32;
    let friend_account = (friend & 0xFFFF_FFFF) as u32;
    let mut messages = Vec::new();
    let mut more_available = false;
    let mut reader = proto::Reader::new(body);
    while let Some((number, field)) = reader.next() {
        match (number, field) {
            (4, proto::Field::Varint(value)) => more_available = value != 0,
            (1, proto::Field::Bytes(entry)) => {
                let mut account = 0u32;
                let mut timestamp = 0u32;
                let mut ordinal = 0u32;
                let mut text = None;
                let mut inner = proto::Reader::new(entry);
                while let Some((number, field)) = inner.next() {
                    match (number, field) {
                        (1, proto::Field::Varint(value)) => account = value as u32,
                        (2, proto::Field::Varint(value)) => timestamp = value as u32,
                        (3, proto::Field::Bytes(value)) => text = Some(value),
                        (4, proto::Field::Varint(value)) => ordinal = value as u32,
                        _ => {}
                    }
                }
                let Some(text) = text else {
                    continue;
                };
                let text = String::from_utf8_lossy(text)
                    .trim_matches('\0')
                    .trim()
                    .to_string();
                if text.is_empty() || account == 0 {
                    continue;
                }
                let outgoing = account == self_account;
                if !outgoing && account != friend_account {
                    continue;
                }
                messages.push((
                    SteamChatMessage {
                        from: if outgoing { steam_id } else { friend },
                        text,
                        at: i64::from(timestamp),
                        outgoing,
                        received_live: false,
                    },
                    ordinal,
                ));
            }
            _ => {}
        }
    }
    // Steam rend du plus récent au plus ancien : la page se lit dans l'autre
    // sens et son premier message devient le curseur de la page précédente.
    messages.reverse();
    let cursor = messages
        .first()
        .map(|(message, ordinal)| (message.at as u32, *ordinal));
    HistoryPage {
        messages: messages.into_iter().map(|(message, _)| message).collect(),
        more_available,
        cursor,
    }
}

/// Lit les entrées d'une liste d'objets Steam : nom (1), nombre possédé (2)
/// et appid (`appid_field`, 6 pour une emote et 4 pour un autocollant).
fn parse_named_items(body: &[u8], list_field: u32, appid_field: u32) -> Vec<SteamEmoticon> {
    let mut items: Vec<SteamEmoticon> = Vec::new();
    let mut reader = proto::Reader::new(body);
    while let Some((number, field)) = reader.next() {
        let proto::Field::Bytes(entry) = field else {
            continue;
        };
        if number != list_field {
            continue;
        }
        let mut name = None;
        let mut count = 0u32;
        let mut appid = 0u32;
        let mut inner = proto::Reader::new(entry);
        while let Some((number, field)) = inner.next() {
            match (number, field) {
                (1, proto::Field::Bytes(value)) => {
                    name = Some(
                        String::from_utf8_lossy(value)
                            .trim_matches('\0')
                            .to_string(),
                    )
                }
                (2, proto::Field::Varint(value)) => count = value as u32,
                (number, proto::Field::Varint(value)) if number == appid_field => {
                    appid = value as u32
                }
                _ => {}
            }
        }
        // Steam rend le nom des emotes déjà entouré de deux-points
        // (`:steamhappy:`) ; le shortcode et l'URL CDN le veulent nu.
        let name = name
            .map(|name| name.trim().trim_matches(':').trim().to_string())
            .filter(|name| !name.is_empty());
        let Some(name) = name else {
            continue;
        };
        if items.iter().any(|known| known.name == name) {
            continue;
        }
        items.push(SteamEmoticon { name, count, appid });
    }
    items
}

/// Lit `CPlayer_GetEmoticonList_Response` : une entrée par emote (champ 1),
/// avec son nom (1), le nombre possédé (2) et l'appid (6).
fn parse_emoticons(body: &[u8]) -> Vec<SteamEmoticon> {
    parse_named_items(body, 1, 6)
}

/// Lit `CMsgClientEmoticonList` : emotes (champ 1) et autocollants (champ 2).
/// L'appid d'un autocollant est le champ 4, celui d'une emote le champ 6.
fn parse_client_emoticon_list(body: &[u8]) -> (Vec<SteamEmoticon>, Vec<SteamSticker>) {
    let emoticons = parse_named_items(body, 1, 6);
    let stickers = parse_named_items(body, 2, 4)
        .into_iter()
        .map(|item| SteamSticker {
            name: item.name,
            count: item.count,
            appid: item.appid,
        })
        .collect();
    (emoticons, stickers)
}

/// Fusionne l'historique Steam avec les messages déjà reçus dans la session.
/// Steam peut renvoyer les mêmes messages que le CM : chaque message existant
/// ne consomme qu'une seule copie, et une répétition réelle reste présente.
fn merge_history(
    chats: &Arc<Mutex<BTreeMap<u64, Vec<SteamChatMessage>>>>,
    friend: u64,
    history: Vec<SteamChatMessage>,
) -> usize {
    if history.is_empty() {
        return 0;
    }
    let Ok(mut chats) = chats.lock() else {
        return 0;
    };
    let conversation = chats.entry(friend).or_default();
    let mut matched = vec![false; conversation.len()];
    let mut additions = Vec::new();
    for message in history {
        let duplicate = conversation.iter().enumerate().position(|(index, known)| {
            !matched[index]
                && known.from == message.from
                && known.outgoing == message.outgoing
                && known.text == message.text
                && (known.at - message.at).abs() <= 10
        });
        match duplicate {
            Some(index) => matched[index] = true,
            None => additions.push(message),
        }
    }
    let added = additions.len();
    conversation.extend(additions);
    conversation.sort_by_key(|message| message.at);
    if conversation.len() > CHAT_MESSAGES {
        let excess = conversation.len() - CHAT_MESSAGES;
        conversation.drain(0..excess);
    }
    added
}

/// Corps du message « jeu en cours ». Un appid nul envoie une liste vide,
/// qui efface l'état chez Steam.
///
/// `game_extra_info` est réservé aux raccourcis non-Steam : le renseigner
/// pour un vrai appid fait perdre à Steam l'identité du jeu (nom, vignette
/// et action « Rejoindre »). Le nom est résolu par Steam à partir de l'appid.
fn games_played_message(appid: u32, _name: &str) -> CMsgClientGamesPlayed {
    CMsgClientGamesPlayed {
        games_played: if appid == 0 {
            Vec::new()
        } else {
            vec![GamePlayed {
                game_id: Some(u64::from(appid)),
                ..GamePlayed::default()
            }]
        },
        ..CMsgClientGamesPlayed::default()
    }
}

/// Publie notre présence.
async fn send_games_played(
    client: &SteamClient<LoggedIn>,
    steam_id: u64,
    appid: u32,
    name: &str,
) -> Result<(), String> {
    let message = games_played_message(appid, name);
    send(
        client,
        steam_id,
        CLIENT_GAMES_PLAYED,
        &message.encode_to_vec(),
    )
    .await
}

/// Publie notre présence.
async fn send_change_status(
    client: &SteamClient<LoggedIn>,
    steam_id: u64,
    presence: SteamPresence,
) -> Result<(), String> {
    let message = CMsgClientChangeStatus {
        persona_state: Some(presence.code()),
        // L'état est choisi par l'utilisateur : Steam ne doit pas le déduire
        // d'une inactivité de la fenêtre.
        persona_set_by_user: Some(true),
        ..CMsgClientChangeStatus::default()
    };
    send(
        client,
        steam_id,
        CLIENT_CHANGE_STATUS,
        &message.encode_to_vec(),
    )
    .await
}

/// Demande le détail des amis listés.
async fn request_friend_data(
    client: &SteamClient<LoggedIn>,
    steam_id: u64,
    friends: impl Iterator<Item = u64>,
) -> Result<(), String> {
    let message = CMsgClientRequestFriendData {
        persona_state_requested: Some(PERSONA_FLAGS),
        friends: friends.filter(|id| *id != 0).collect(),
    };
    send(
        client,
        steam_id,
        CLIENT_REQUEST_FRIEND_DATA,
        &message.encode_to_vec(),
    )
    .await
}

/// Fusionne la liste d'amis poussée par le CM : relations retirées comprises.
fn apply_friends_list(live: &mut Live, list: &CMsgClientFriendsList) {
    if !list.bincremental.unwrap_or(false) {
        live.friends.clear();
        live.friends_seen.clear();
    }
    for friend in &list.friends {
        let Some(steam_id) = friend.ulfriendid.filter(|id| *id != 0) else {
            continue;
        };
        let relationship = friend.efriendrelationship.unwrap_or(0);
        // 3 = ami ; les autres relations (demandes, ignorés) ne sont pas des
        // amis et disparaissent de la liste. Les groupes et salons Steam
        // figurent parfois avec la même relation : leur identifiant n'est pas
        // un compte individuel, ils n'ont rien à faire dans la liste d'amis.
        if relationship == 3 && is_person(steam_id) {
            let entry = live.friends.entry(steam_id).or_insert(SteamFriend {
                steam_id,
                ..SteamFriend::default()
            });
            // Un ami hors ligne n'a pas encore d'état : Steam le publiera.
            entry.steam_id = steam_id;
        } else {
            live.friends.remove(&steam_id);
            live.friends_seen.remove(&steam_id);
        }
    }
}

/// Un compte individuel : le type se lit aux bits 52 à 55 du SteamID (1 pour
/// une personne, 7 pour un groupe, 8 pour un salon). Les groupes ne sont pas
/// des amis, même quand Steam les range avec eux.
fn is_person(steam_id: u64) -> bool {
    ((steam_id >> 52) & 0xf) == 1
}

/// Fusionne un état de présence poussé par le CM et retourne le nombre
/// d'événements d'activité produits. Le premier état d'un ami sert de
/// référence : il ne notifie pas une connexion déjà en cours au démarrage.
fn apply_persona_state(live: &mut Live, state: &CMsgClientPersonaState) -> usize {
    let at = unix_now();
    let mut events = Vec::new();
    for friend in &state.friends {
        let Some(steam_id) = friend.friendid.filter(|id| *id != 0) else {
            continue;
        };
        if steam_id == live.steam_id {
            // Notre propre ligne : elle confirme la présence appliquée.
            if let Some(code) = friend.persona_state {
                live.applied = SteamPresence::from_code(code);
            }
            if let Some(name) = friend.player_name.as_ref().filter(|name| !name.is_empty()) {
                live.persona_name = name.clone();
            }
            continue;
        }
        let Some(entry) = live.friends.get_mut(&steam_id) else {
            // Steam publie aussi les comptes de groupe et les profils inconnus.
            continue;
        };
        let first_state = live.friends_seen.insert(steam_id);
        let previous = entry.clone();
        apply_persona(entry, friend);
        if !first_state {
            events.extend(friend_events_between(&previous, entry, at));
        }
    }
    let count = events.len();
    // `friend_events` est publié du plus récent au plus ancien. Les deux
    // transitions d'un même instant (connexion puis lancement) sont produites
    // dans l'ordre chronologique : les insérer une à une en tête donne donc
    // bien [GameStarted, Online], et l'interface les rejoue à l'envers pour
    // notifier la connexion avant le jeu.
    for event in events {
        live.friend_events.insert(0, event);
    }
    live.friend_events.truncate(100);
    count
}

/// Compare deux états d'un ami et rend les transitions qui méritent une
/// notification. Un changement de jeu produit la fin de l'ancien et le début
/// du nouveau ; un simple passage en ligne/absent n'en produit aucune.
fn friend_events_between(
    previous: &SteamFriend,
    next: &SteamFriend,
    at: i64,
) -> Vec<SteamFriendEvent> {
    let mut events = Vec::new();
    let event = |kind: SteamFriendEventKind, game: &str, appid: u32| SteamFriendEvent {
        kind,
        steam_id: next.steam_id,
        name: next.name.clone(),
        avatar: next.avatar.clone(),
        game: game.to_string(),
        appid,
        at,
    };
    let was_online = previous.presence.online();
    let is_online = next.presence.online();
    if !was_online && is_online {
        events.push(event(SteamFriendEventKind::Online, "", 0));
    } else if was_online && !is_online {
        events.push(event(SteamFriendEventKind::Offline, "", 0));
    }
    if previous.game_appid != next.game_appid {
        if previous.game_appid != 0 {
            events.push(event(
                SteamFriendEventKind::GameStopped,
                &previous.game,
                previous.game_appid,
            ));
        }
        if next.game_appid != 0 {
            events.push(event(
                SteamFriendEventKind::GameStarted,
                &next.game,
                next.game_appid,
            ));
        }
    }
    events
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn apply_persona(entry: &mut SteamFriend, friend: &PersonaFriend) {
    if let Some(name) = friend.player_name.as_ref().filter(|name| !name.is_empty()) {
        entry.name = name.clone();
    }
    if let Some(code) = friend.persona_state {
        entry.presence = SteamPresence::from_code(code);
    }
    let incoming_game = friend.game_name.as_ref().filter(|game| !game.is_empty());
    if let Some(game) = incoming_game {
        entry.game = game.clone();
    }
    if let Some(appid) = friend.game_played_app_id {
        if appid == 0 {
            // Steam signale explicitement la fin du jeu par un appid nul.
            entry.game_appid = 0;
            entry.game.clear();
        } else {
            // Un appid différent désigne un autre jeu : sans nouveau nom dans
            // cette mise à jour, l'ancien titre ne doit pas rester attaché à la
            // nouvelle capsule. Le daemon le résoudra depuis le catalogue ou
            // la boutique.
            if entry.game_appid != appid && incoming_game.is_none() {
                entry.game.clear();
            }
            entry.game_appid = appid;
            // Un jeu joué vaut présence « en jeu » : Steam publie parfois un
            // état en ligne sans le drapeau de riche présence.
            if entry.presence == SteamPresence::Online {
                entry.presence = SteamPresence::Play;
            }
        }
    }
    if let Some(logon) = friend.last_logon.filter(|time| *time != 0) {
        entry.last_logon = i64::from(logon);
    }
    if let Some(logoff) = friend.last_logoff.filter(|time| *time != 0) {
        entry.last_logoff = i64::from(logoff);
    }
    if let Some(hash) = friend.avatar_hash.as_ref().filter(|hash| hash.len() >= 20) {
        entry.avatar = avatar_url(hash);
    }
}

/// URL publique d'un avatar à partir de son empreinte SHA-1.
fn avatar_url(hash: &[u8]) -> String {
    let mut hex = String::with_capacity(hash.len() * 2);
    for byte in hash {
        hex.push_str(&format!("{byte:02x}"));
    }
    format!("https://avatars.fastly.steamstatic.com/{hex}_full.jpg")
}

/// Premier varint d'un corps protobuf sans schéma (utilisé pour le battement).
fn first_varint(body: &[u8]) -> Option<u64> {
    let mut reader = proto::Reader::new(body);
    match reader.next() {
        Some((_, proto::Field::Varint(value))) => Some(value),
        _ => None,
    }
}

/// Entretien du jeton : `LAPINUX_SOCIAL_NO_RENEW` le désactive, comme les
/// sondes désactivent le réseau.
fn renewal_enabled() -> bool {
    lapinux_core::env_var("SOCIAL_NO_RENEW").is_none()
}

/// Réponse à l'entretien du jeton : Steam y rend un jeton de refresh tourné,
/// aussitôt enregistré. Le jeton d'accès n'est pas conservé — il expire en
/// quelques heures et sert au réseau, pas au disque.
fn apply_tokens(session: &SavedSession, body: &[u8]) -> Option<String> {
    let mut access = None;
    let mut refresh = None;
    let mut reader = proto::Reader::new(body);
    while let Some((number, field)) = reader.next() {
        if let proto::Field::Bytes(value) = field {
            match number {
                1 => access = String::from_utf8(value.to_vec()).ok(),
                2 => refresh = String::from_utf8(value.to_vec()).ok(),
                _ => {}
            }
        }
    }
    // Steam ne rend pas toujours un jeton de refresh : le jeton d'accès rendu
    // s'ajoute à la session sans la remplacer, et le jeton enregistré reste
    // celui qui reconnecte le compte.
    if let Some(refresh) = refresh.filter(|token| !token.trim().is_empty()) {
        if refresh != session.refresh_token {
            let renewed = SavedSession {
                account_name: session.account_name.clone(),
                steam_id: session.steam_id,
                refresh_token: refresh,
                guard_data: session.guard_data.clone(),
                saved_at: None,
            };
            match crate::session::save(&renewed) {
                Ok(path) => tracing::info!("jeton Steam entretenu ({})", path.display()),
                Err(error) => tracing::warn!("jeton Steam non enregistré : {error}"),
            }
        }
    } else {
        tracing::debug!("session confirmée : le jeton enregistré reste valable");
    }
    access.filter(|token| !token.trim().is_empty())
}

/// Demande la liste d'amis à l'API communautaire et la fusionne dans l'état.
///
/// Rend `true` si la liste a été obtenue : le CM ne pousse la sienne qu'au
/// logon, groupée avec la réponse — cette requête la rend à tout moment, et
/// les noms et les états suivent par `ClientRequestFriendData`.
async fn apply_remote_friend_list(access_token: &str, live: &mut Live) -> bool {
    let url = format!(
        "https://api.steampowered.com/IFriendsListService/GetFriendsList/v1/?access_token={access_token}"
    );
    let response = match reqwest::Client::new()
        .get(&url)
        .timeout(HTTP_TIMEOUT)
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            tracing::debug!("liste d'amis indisponible : {error}");
            return false;
        }
    };
    let status = response.status();
    let body = match response.text().await {
        Ok(body) => body,
        Err(error) => {
            tracing::debug!("liste d'amis illisible : {error}");
            return false;
        }
    };
    if !status.is_success() {
        tracing::debug!("liste d'amis refusée ({status}, {} octets)", body.len());
        return false;
    }
    let Some((friends, total)) = parse_friend_list(&body) else {
        tracing::debug!(
            "liste d'amis inattendue ({status}, {} octets) : {}",
            body.len(),
            body.chars().take(300).collect::<String>()
        );
        return false;
    };
    tracing::debug!(
        "liste d'amis HTTP : {total} relations, {} amis",
        friends.len()
    );
    for (steam_id, relationship) in friends {
        if relationship == 3 && is_person(steam_id) {
            live.friends.entry(steam_id).or_insert(SteamFriend {
                steam_id,
                ..SteamFriend::default()
            });
        } else {
            live.friends.remove(&steam_id);
        }
    }
    true
}

/// Relit les notifications du service Web Steam avec le jeton de session.
/// Le CM ne pousse que des compteurs ; ce service rend les événements complets
/// (wishlist, promotion, échange, préchargement…) et leur `body_data`.
async fn fetch_web_notifications(
    access_token: &str,
    steam_id: u64,
    wishlist_deals: &mut HashMap<u32, SteamWishlistDeal>,
) -> Option<Vec<SteamWebNotification>> {
    let url = format!(
        "https://api.steampowered.com/ISteamNotificationService/GetSteamNotifications/v1/?access_token={access_token}"
    );
    let response = match reqwest::Client::new()
        .get(&url)
        .header("accept-encoding", "identity")
        .timeout(HTTP_TIMEOUT)
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            tracing::debug!("notifications Steam indisponibles : {error}");
            return None;
        }
    };
    let status = response.status();
    let body = match response.text().await {
        Ok(body) => body,
        Err(error) => {
            tracing::debug!("notifications Steam illisibles : {error}");
            return None;
        }
    };
    if !status.is_success() {
        tracing::debug!(
            "notifications Steam refusées ({status}, {} octets)",
            body.len()
        );
        return None;
    }
    let mut notifications = parse_web_notifications(&body);
    resolve_wishlist_deals(&mut notifications, wishlist_deals).await;
    apply_wishlist_urls(&mut notifications, steam_id);
    Some(notifications)
}

/// Remplace le compteur d'une wishlist par les noms des jeux. Une seule
/// requête au magasin pour tous les appids absents du cache, puis les cartes
/// suivantes réutilisent les noms déjà connus.
async fn resolve_wishlist_deals(
    notifications: &mut [SteamWebNotification],
    known: &mut HashMap<u32, SteamWishlistDeal>,
) {
    let mut missing = Vec::new();
    for notification in notifications.iter().filter(|item| item.kind == 8) {
        for appid in &notification.appids {
            if !known.contains_key(appid) && !missing.contains(appid) {
                missing.push(*appid);
            }
        }
    }
    for chunk in missing.chunks(50) {
        known.extend(fetch_wishlist_deals(chunk).await);
    }
    for notification in notifications.iter_mut().filter(|item| item.kind == 8) {
        notification.deals = notification
            .appids
            .iter()
            .filter_map(|appid| known.get(appid).cloned())
            .collect();
        let names: Vec<&str> = notification
            .deals
            .iter()
            .map(|deal| deal.name.as_str())
            .filter(|name| !name.trim().is_empty())
            .collect();
        if names.is_empty() {
            continue;
        }
        notification.body = if names.len() == notification.appids.len() {
            names.join(" · ")
        } else {
            format!(
                "{} · {} autre(s)",
                names.join(" · "),
                notification.appids.len() - names.len()
            )
        };
    }
}

/// Noms publics des jeux demandés. L'API du magasin répond un objet indexé par
/// appid ; `success: false` signale une fiche retirée, simplement ignorée.
async fn fetch_wishlist_deals(appids: &[u32]) -> HashMap<u32, SteamWishlistDeal> {
    // Le magasin refuse plusieurs appids dans une même requête (HTTP 400),
    // mais répond correctement à l'unité. Quatre fiches à la fois suffisent à
    // couvrir une wishlist sans ouvrir une rafale de connexions.
    let mut deals = HashMap::new();
    let mut pending = appids.iter().copied();
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..4 {
        let Some(appid) = pending.next() else {
            break;
        };
        tasks.spawn(async move { (appid, fetch_wishlist_deal(appid).await) });
    }
    while let Some(result) = tasks.join_next().await {
        if let Ok((appid, Some(deal))) = result {
            deals.insert(appid, deal);
        }
        if let Some(appid) = pending.next() {
            tasks.spawn(async move { (appid, fetch_wishlist_deal(appid).await) });
        }
    }
    deals
}

async fn fetch_wishlist_deal(appid: u32) -> Option<SteamWishlistDeal> {
    let url = format!(
        "https://store.steampowered.com/api/appdetails?appids={appid}&filters=basic,price_overview&l=french"
    );
    let response = match reqwest::Client::new()
        .get(&url)
        .header("accept-encoding", "identity")
        .timeout(HTTP_TIMEOUT)
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            tracing::debug!("fiche wishlist indisponible pour {appid} : {error}");
            return None;
        }
    };
    let status = response.status();
    let body = match response.text().await {
        Ok(body) => body,
        Err(error) => {
            tracing::debug!("fiche wishlist illisible pour {appid} : {error}");
            return None;
        }
    };
    if !status.is_success() {
        tracing::debug!(
            "fiche wishlist refusée pour {appid} ({status}, {} octets)",
            body.len()
        );
        return None;
    }
    parse_wishlist_deals(&body, &[appid]).remove(&appid)
}

fn parse_wishlist_deals(body: &str, appids: &[u32]) -> HashMap<u32, SteamWishlistDeal> {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(body) else {
        return HashMap::new();
    };
    let mut deals = HashMap::new();
    for appid in appids {
        let Some(data) = json
            .get(appid.to_string())
            .and_then(|entry| entry.get("data"))
        else {
            continue;
        };
        let Some(name) = data
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
        else {
            continue;
        };
        let price = data
            .pointer("/price_overview/final_formatted")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .unwrap_or_default()
            .to_string();
        let discount = data
            .pointer("/price_overview/discount_percent")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
            .min(100) as u32;
        deals.insert(
            *appid,
            SteamWishlistDeal {
                appid: *appid,
                name: name.to_string(),
                price,
                discount,
            },
        );
    }
    deals
}

/// `ISteamNotificationService/GetSteamNotifications` : chaque entrée porte un
/// type, un horodatage et un `body_data` JSON propre au type. Les entrées déjà
/// lues ou masquées sont écartées, ainsi que les commentaires de profil, qui
/// ne sont pas des notifications du lanceur.
fn parse_web_notifications(body: &str) -> Vec<SteamWebNotification> {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(body) else {
        return Vec::new();
    };
    let entries = json
        .get("response")
        .and_then(|response| response.get("notifications"))
        .or_else(|| json.get("notifications"))
        .and_then(serde_json::Value::as_array);
    let Some(entries) = entries else {
        return Vec::new();
    };
    let mut notifications: Vec<SteamWebNotification> = entries
        .iter()
        .filter_map(|entry| {
            if entry
                .get("hidden")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
            {
                return None;
            }
            if entry
                .get("read")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
            {
                return None;
            }
            let kind = entry
                .get("notification_type")
                .and_then(|value| number(value.clone()))
                .unwrap_or(0) as u32;
            if kind == 3 {
                return None;
            }
            let raw = entry
                .get("body_data")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let (title, body, url, appids) = parse_web_body(kind, raw);
            Some(SteamWebNotification {
                kind,
                title,
                body,
                url,
                appids,
                deals: Vec::new(),
                at: entry
                    .get("timestamp")
                    .and_then(|value| number(value.clone()))
                    .unwrap_or(0) as i64,
            })
        })
        .collect();
    notifications.sort_by(|left, right| right.at.cmp(&left.at));
    notifications
}

/// Page groupée des promotions de la liste de souhaits, triée par remise.
const WISHLIST_PROMOTIONS_URL: &str = "https://store.steampowered.com/wishlist/?sort=discount";

/// Page de la liste de souhaits du compte, filtrée sur les jeux cités : Steam
/// y regroupe leurs promotions. Sans compte connu, la page générique suffit.
fn wishlist_promotions_url(steam_id: u64, appids: &[u32]) -> String {
    if steam_id == 0 {
        return WISHLIST_PROMOTIONS_URL.to_string();
    }
    let mut url =
        format!("https://store.steampowered.com/wishlist/profiles/{steam_id}/?sort=discount");
    if !appids.is_empty() {
        let ids = appids
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join("%2C");
        url.push_str("&wng=");
        url.push_str(&ids);
    }
    url
}

/// Remplace le lien générique des notifications wishlist par la page groupée
/// du compte, avec les appids que la notification cite.
fn apply_wishlist_urls(notifications: &mut [SteamWebNotification], steam_id: u64) {
    for notification in notifications.iter_mut().filter(|item| item.kind == 8) {
        notification.url = wishlist_promotions_url(steam_id, &notification.appids);
    }
}

/// Décode le `body_data` d'une notification Web. Les clés varient selon le
/// type ; les plus courantes sont lues, et un appid seul donne au moins le
/// lien vers la fiche du jeu. Un contenu non JSON reste affichable tel quel.
fn parse_web_body(kind: u32, raw: &str) -> (String, String, String, Vec<u32>) {
    let raw = raw.trim();
    if raw.is_empty() {
        return (String::new(), String::new(), String::new(), Vec::new());
    }
    let Ok(json) = serde_json::from_str::<serde_json::Value>(raw) else {
        return (String::new(), raw.to_string(), String::new(), Vec::new());
    };
    let text = |keys: &[&str]| -> Option<String> {
        keys.iter().find_map(|key| {
            json.get(*key)
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        })
    };
    let title = text(&[
        "title",
        "name",
        "game_name",
        "app_name",
        "subject",
        "display_name",
    ])
    .unwrap_or_default();
    let mut body = text(&[
        "body",
        "message",
        "text",
        "description",
        "comment",
        "content",
        "status",
    ])
    .unwrap_or_default();
    let mut url = text(&["url", "link", "store_url", "community_url"]).unwrap_or_default();
    let mut appids: Vec<u32> = json
        .get("appids")
        .and_then(serde_json::Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(|value| number(value.clone()))
                .filter_map(|appid| u32::try_from(appid).ok())
                .collect()
        })
        .unwrap_or_default();
    let appid = json
        .get("appid")
        .and_then(|value| number(value.clone()))
        .or_else(|| json.get("app_id").and_then(|value| number(value.clone())))
        .or_else(|| appids.first().copied().map(u64::from))
        .unwrap_or(0);
    if let Ok(appid) = u32::try_from(appid) {
        if appid != 0 && !appids.contains(&appid) {
            appids.push(appid);
        }
    }
    if url.is_empty() {
        if kind == 8 {
            url = WISHLIST_PROMOTIONS_URL.to_string();
        } else if appid > 0 {
            url = format!("https://store.steampowered.com/app/{appid}");
        }
    }
    if title.is_empty() && body.is_empty() && kind == 8 && !appids.is_empty() {
        let count = json
            .get("count")
            .and_then(|value| number(value.clone()))
            .unwrap_or(appids.len() as u64)
            .max(1);
        body = if count == 1 {
            "Un jeu de votre liste de souhaits".into()
        } else {
            format!("{count} jeux de votre liste de souhaits")
        };
    }
    if title.is_empty() && body.is_empty() {
        (String::new(), String::new(), url, appids)
    } else {
        (title, body, url, appids)
    }
}

/// `CMsgGetFriendsListResponse` en JSON : la liste imbriquée porte les mêmes
/// relations que la poussée du CM. Les identifiants arrivent en texte.
fn parse_friend_list(body: &str) -> Option<(Vec<(u64, u32)>, usize)> {
    let json: serde_json::Value = serde_json::from_str(body).ok()?;
    let response = json.get("response")?;
    // L'API écrit `friendslist`, la même structure porte `friends_list`
    // ailleurs : les deux formes sont acceptées.
    let list = response
        .get("friendslist")
        .or_else(|| response.get("friends_list"))?;
    let total = list
        .get("max_friend_count")
        .cloned()
        .and_then(number)
        .unwrap_or(0) as usize;
    let entries = list.get("friends")?.as_array()?;
    let mut friends = Vec::with_capacity(entries.len());
    for entry in entries {
        let id = entry
            .get("ulfriendid")
            .or_else(|| entry.get("steamid"))
            .and_then(|value| number(value.clone()));
        let Some(id) = id.filter(|id| *id != 0) else {
            continue;
        };
        let relationship = entry
            .get("efriendrelationship")
            .and_then(|value| number(value.clone()))
            .unwrap_or(0) as u32;
        friends.push((id, relationship));
    }
    Some((friends, total))
}

/// Entier JSON, qu'il arrive en nombre ou en texte (Steam écrit les
/// identifiants 64 bits entre guillemets).
fn number(value: serde_json::Value) -> Option<u64> {
    match value {
        serde_json::Value::Number(number) => number.as_u64(),
        serde_json::Value::String(text) => text.parse().ok(),
        _ => None,
    }
}

/// `CFamilyGroups_GetSharedLibraryApps_Response` : chaque jeu nomme ses
/// propriétaires. Le nombre de jeux d'un membre est celui des applications
/// qu'il apporte à la bibliothèque partagée.
fn apply_shared_apps(body: &[u8], family: &mut SteamFamily) {
    use std::collections::{HashMap, HashSet};
    let mut counts: HashMap<u64, HashSet<u32>> = HashMap::new();
    let mut reader = proto::Reader::new(body);
    while let Some((number, field)) = reader.next() {
        let proto::Field::Bytes(app) = field else {
            continue;
        };
        if number != 1 {
            continue;
        }
        let mut appid = 0u32;
        let mut owners: Vec<u64> = Vec::new();
        let mut reader = proto::Reader::new(app);
        while let Some((number, field)) = reader.next() {
            match (number, field) {
                (1, proto::Field::Varint(value)) => appid = value as u32,
                // Liste compactée : huit octets par identifiant.
                (2, proto::Field::Bytes(packed)) => {
                    for chunk in packed.chunks_exact(8) {
                        let mut bytes = [0u8; 8];
                        bytes.copy_from_slice(chunk);
                        owners.push(u64::from_le_bytes(bytes));
                    }
                }
                (2, proto::Field::Fixed64(owner)) => owners.push(owner),
                _ => {}
            }
        }
        if appid == 0 {
            continue;
        }
        for owner in owners {
            counts.entry(owner).or_default().insert(appid);
        }
    }
    for member in &mut family.members {
        let mut appids: Vec<u32> = counts
            .get(&member.steam_id)
            .map(|set| set.iter().copied().collect())
            .unwrap_or_default();
        appids.sort_unstable();
        member.games = appids.len() as u32;
        member.appids = appids;
    }
}

/// `CMsgClientPurchaseResponse` : `eresult` dit si la clé est passée, le
/// détail explique un refus.
fn parse_purchase(body: &[u8]) -> SteamKeyRedeem {
    let mut eresult: Option<u64> = None;
    let mut detail: Option<u64> = None;
    let mut receipt: Option<&[u8]> = None;
    let mut reader = proto::Reader::new(body);
    while let Some((number, field)) = reader.next() {
        match (number, field) {
            (1, proto::Field::Varint(value)) => eresult = Some(value),
            (2, proto::Field::Varint(value)) => detail = Some(value),
            // `purchase_receipt_info` : un `CStore_PurchaseReceiptInfo`
            // encodé à part, avec ses lignes de commande.
            (3, proto::Field::Bytes(value)) => receipt = Some(value),
            _ => {}
        }
    }
    let (games, appids) = receipt.and_then(parse_receipt).unwrap_or_default();
    if matches!(eresult, None | Some(1)) {
        let message = match games.as_slice() {
            [] => "Clé activée : le jeu rejoint votre bibliothèque.".to_string(),
            [name] => format!("Clé activée : {name} rejoint votre bibliothèque."),
            names => format!("Clé activée : {}.", names.join(", ")),
        };
        SteamKeyRedeem {
            state: "ok".into(),
            message,
            games,
            appids,
        }
    } else {
        SteamKeyRedeem {
            state: "error".into(),
            message: match detail {
                Some(detail) => format!("Steam a refusé la clé (code {detail})."),
                None => "Steam a refusé la clé.".into(),
            },
            games: Vec::new(),
            appids: Vec::new(),
        }
    }
}

/// Lignes de commande du reçu (`line_items`, champ 18) : un appid et la
/// description du jeu. Steam ne les donne pas toujours ; l'historique garde
/// alors la clé masquée et se complète à la prochaine activation.
fn parse_receipt(body: &[u8]) -> Option<(Vec<String>, Vec<u32>)> {
    let mut games = Vec::new();
    let mut appids = Vec::new();
    let mut reader = proto::Reader::new(body);
    while let Some((number, field)) = reader.next() {
        let proto::Field::Bytes(item) = field else {
            continue;
        };
        if number != 18 {
            continue;
        }
        let mut appid = 0u32;
        let mut description = String::new();
        let mut item_reader = proto::Reader::new(item);
        while let Some((field_number, field)) = item_reader.next() {
            match (field_number, field) {
                (2, proto::Field::Varint(value)) => appid = value as u32,
                (3, proto::Field::Bytes(text)) => {
                    description = String::from_utf8_lossy(text).trim().to_string();
                }
                _ => {}
            }
        }
        if appid != 0 && !appids.contains(&appid) {
            appids.push(appid);
        }
        if !description.is_empty() {
            games.push(description);
        }
    }
    if games.is_empty() && appids.is_empty() {
        None
    } else {
        Some((games, appids))
    }
}

/// Réponse à « FamilyGroups.GetFamilyGroupForUser » : nom du groupe et membres.
fn parse_family(body: &[u8], steam_id: u64) -> Option<SteamFamily> {
    let mut group_id = 0u64;
    let mut is_member = false;
    let mut group = None;
    let mut reader = proto::Reader::new(body);
    while let Some((number, field)) = reader.next() {
        match (number, field) {
            (1, proto::Field::Varint(value)) => group_id = value,
            (2, proto::Field::Varint(value)) => is_member = value == 0,
            (8, proto::Field::Bytes(bytes)) => group = Some(parse_family_group(bytes)),
            _ => {}
        }
    }
    let group = group?;
    if group.members.is_empty() {
        return None;
    }
    let mut family = group;
    family.group_id = group_id;
    family
        .members
        .iter_mut()
        .for_each(|member| member.is_self = member.steam_id == steam_id);
    if is_member && family.name.trim().is_empty() {
        // Le groupe existe même sans nom : on garde « Famille Steam ».
        family.name = "Famille Steam".into();
    }
    Some(family)
}

/// `CFamilyGroups_GetFamilyGroup_Response` : nom et membres.
fn parse_family_group(body: &[u8]) -> SteamFamily {
    let mut family = SteamFamily::default();
    let mut reader = proto::Reader::new(body);
    while let Some((number, field)) = reader.next() {
        match (number, field) {
            (1, proto::Field::Bytes(name)) => {
                family.name = String::from_utf8_lossy(name).trim().to_string();
            }
            (2, proto::Field::Bytes(member)) => {
                let mut entry = SteamFamilyMember::default();
                let mut reader = proto::Reader::new(member);
                while let Some((number, field)) = reader.next() {
                    match (number, field) {
                        (1, proto::Field::Fixed64(steam_id)) => entry.steam_id = steam_id,
                        (2, proto::Field::Varint(role)) => entry.role = role as u32,
                        _ => {}
                    }
                }
                if entry.steam_id != 0 {
                    family.members.push(entry);
                }
            }
            _ => {}
        }
    }
    family
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use steamroom::generated::c_msg_client_friends_list::Friend as ListFriend;
    use steamroom::generated::{
        CMsgClientFriendMsg, CMsgClientFriendMsgIncoming, CMsgClientLogonResponse,
    };
    use steamroom::transport::Transport;
    use steamroom_client::login::PreparedLoginBuilder;
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::Message as WsMessage;

    /// Une rejointe acceptée enregistre le lobby et l'adresse publiée ; un
    /// refus ne laisse ni lobby ni adresse.
    #[test]
    fn un_lobby_rejoint_est_enregistre_et_un_refus_non() {
        let mut joined = crate::lobby::JoinedLobby {
            app_id: 480,
            lobby: 777_001,
            eresult: 1,
            max_members: 4,
            ..crate::lobby::JoinedLobby::default()
        };
        joined.metadata = vec![("connect".to_string(), "10.4.3.2:7777".to_string())];
        assert!(join_verdict(&joined).is_ok(), "un eresult 1 est accepté");
        assert!(record_join_result(&joined), "un succès s'enregistre");
        assert_eq!(lobby_connect(777_001).as_deref(), Some("10.4.3.2:7777"));
        let refused = crate::lobby::JoinedLobby {
            app_id: 480,
            lobby: 777_002,
            eresult: 2,
            ..crate::lobby::JoinedLobby::default()
        };
        assert_eq!(
            join_verdict(&refused),
            Err(2),
            "le refus remonte son eresult"
        );
        assert!(
            !record_join_result(&refused),
            "un refus ne s'enregistre pas"
        );
        assert!(lobby_connect(777_002).is_none());
    }

    /// La réponse de join pose la liste des membres, et les notifications
    /// 6619/6620 la tiennent à jour.
    #[test]
    fn les_notifications_de_membres_tiennent_la_liste_du_lobby() {
        let mut joined = crate::lobby::JoinedLobby {
            app_id: 480,
            lobby: 888_001,
            eresult: 1,
            ..crate::lobby::JoinedLobby::default()
        };
        joined.members = vec![crate::lobby::LobbyMember {
            steam_id: 1,
            persona_name: "JoueurExemple".to_string(),
        }];
        assert!(record_join_result(&joined));
        assert_eq!(lobby_members(888_001).len(), 1);
        assert!(apply_member_joined(
            888_001,
            crate::lobby::LobbyMember {
                steam_id: 2,
                persona_name: "Ami".to_string(),
            }
        ));
        assert_eq!(lobby_members(888_001).len(), 2);
        // Le même membre annoncé deux fois ne compte qu'une fois.
        assert!(apply_member_joined(
            888_001,
            crate::lobby::LobbyMember {
                steam_id: 2,
                persona_name: "Ami renommé".to_string(),
            }
        ));
        assert_eq!(lobby_members(888_001).len(), 2);
        assert_eq!(lobby_members(888_001)[1].persona_name, "Ami renommé");
        assert!(apply_member_left(888_001, 1));
        assert_eq!(lobby_members(888_001).len(), 1);
        assert!(
            apply_member_left(999_999, 3),
            "une notification suffit à connaître le lobby"
        );
        assert!(
            lobby_members(999_999).is_empty(),
            "une sortie seule laisse une liste vide, pas inventée"
        );
    }

    /// Accepter une invitation nomme le lobby de la demande : ni vide, ni le
    /// lobby du jeu, ni une valeur par défaut.
    #[test]
    fn une_invitation_fournit_le_lobby_de_la_demande() {
        remember_invite(crate::lobby::LobbyInvite {
            app_id: 900,
            lobby: 901,
            inviter: 902,
        });
        let (appid, body) = invite_join_body(901, "test-user", 0).expect("invitation retrouvée");
        assert_eq!(appid, 900);
        assert!(
            body.windows(9)
                .any(|window| window == [0x11, 0x85, 0x03, 0, 0, 0, 0, 0, 0]),
            "le lobby 901 part en fixed64 au champ 2"
        );
        assert!(
            invite_join_body(999, "test-user", 0).is_none(),
            "sans invitation, aucune demande n'est fabriquée"
        );
    }

    /// Le CM refuse un message MMS sans les en-têtes du client Steam :
    /// l'appid doit être routé, la session reprise et le job annoncé.
    #[test]
    fn lobby_header_carries_appid_session_and_job() {
        let message = lobby_message(
            76_561_198_000_000_000,
            Some(1_248_222_867),
            480,
            Some(7),
            CLIENT_MMS_CREATE_LOBBY,
            &[1, 2, 3],
        );
        assert_eq!(message.header.steamid, Some(76_561_198_000_000_000));
        assert_eq!(message.header.client_sessionid, Some(1_248_222_867));
        assert_eq!(message.header.routing_appid, Some(480));
        assert_eq!(message.header.jobid_source, Some(7));
        assert_eq!(&message.body[..], &[1, 2, 3]);
    }

    /// Le message « jeu en cours » porte l'appid Steam dans `game_id` ; un
    /// appid nul envoie une liste vide, qui efface l'état. Le nom ne doit pas
    /// être envoyé dans `game_extra_info`, qui est réservé aux raccourcis.
    #[test]
    fn games_played_carries_the_steam_appid() {
        let playing = games_played_message(620, "Portal 2");
        assert_eq!(playing.games_played.len(), 1);
        assert_eq!(playing.games_played[0].game_id, Some(620));
        assert_eq!(playing.games_played[0].game_extra_info, None);
        assert!(games_played_message(0, "").games_played.is_empty());
    }

    /// Un message porte le destinataire, le type texte et le contenu.
    #[test]
    fn friend_message_carries_destination_kind_and_text() {
        let body = friend_msg_message(222, "salut");
        let mut fields = proto::Reader::new(&body);
        assert_eq!(fields.next(), Some((1, proto::Field::Fixed64(222))));
        assert_eq!(fields.next(), Some((2, proto::Field::Varint(1))));
        assert_eq!(fields.next(), Some((3, proto::Field::Bytes(b"salut"))));
        // Le champ 4 est un fixed32 (`rtime32_server_timestamp`), pas un
        // varint : le message s'arrête après le texte, Steam l'estampille.
        assert_eq!(fields.next(), None);
    }

    /// Le BBCode déjà composé part avec l'indicateur : c'est lui qui demande
    /// à Steam d'interpréter les balises au lieu de les diffuser en texte.
    #[test]
    fn friend_messages_send_declare_le_bbcode() {
        let body = friend_messages_send(222, "[emoticon]steamhappy[/emoticon]", true);
        let mut fields = proto::Reader::new(&body);
        assert_eq!(fields.next(), Some((1, proto::Field::Fixed64(222))));
        assert_eq!(fields.next(), Some((2, proto::Field::Varint(1))));
        assert_eq!(
            fields.next(),
            Some((3, proto::Field::Bytes(b"[emoticon]steamhappy[/emoticon]")))
        );
        // Champ 4 : `contains_bbcode` vrai.
        assert_eq!(fields.next(), Some((4, proto::Field::Varint(1))));
        assert_eq!(fields.next(), None);
    }

    #[test]
    fn les_shortcodes_d_emotes_partent_tels_quels() {
        let emoticons = vec![SteamEmoticon {
            name: "steamhappy".into(),
            count: 1,
            appid: 753,
        }];
        // Steam traduit le shortcode en emoticon : on ne précompose pas la
        // balise, sinon il la retire et le destinataire lit le nom seul.
        assert_eq!(
            bbcoded_message("salut :steamhappy: !", &emoticons).as_deref(),
            Some("salut :steamhappy: !")
        );
        // La forme U+02D0 que le sélecteur employait reste acceptée.
        assert_eq!(
            bbcoded_message("ːsteamhappyː", &emoticons).as_deref(),
            Some("ːsteamhappyː")
        );
        // Un shortcode inconnu, et un message sans emote, gardent l'ancien
        // chemin : les crochets littéraux ne sont jamais exposés au BBCode.
        assert!(bbcoded_message(":inconnue:", &emoticons).is_none());
        assert!(bbcoded_message("salut [toi]", &emoticons).is_none());
        assert_eq!(
            bbcoded_message("[salut] :steamhappy:", &emoticons).as_deref(),
            Some("\\[salut] :steamhappy:")
        );
    }

    #[test]
    fn incoming_friend_message_is_decoded() {
        let mut body = Vec::new();
        proto::field_fixed64(1, 222, &mut body);
        proto::field_varint(2, 1, &mut body);
        proto::field_varint(3, 0, &mut body);
        proto::field_bytes(4, b"bonjour", &mut body);
        proto::field_fixed32(5, 1_700_000_000, &mut body);
        assert_eq!(
            parse_incoming_friend_msg(&body),
            Some((222, "bonjour".into(), 1_700_000_000))
        );
        // Les messages d'un autre type (saisie en cours, invitation) sont
        // ignorés.
        let mut other = Vec::new();
        proto::field_fixed64(1, 222, &mut other);
        proto::field_varint(2, 2, &mut other);
        proto::field_bytes(4, b"x", &mut other);
        assert_eq!(parse_incoming_friend_msg(&other), None);
    }

    #[test]
    fn les_emsg_de_message_ami_suivent_le_protocole() {
        // 773 est ClientGameConnectDeny et 776 n'existe pas : avec ces
        // valeurs, aucun message d'ami ne partait ni n'arrivait.
        assert_eq!(CLIENT_FRIEND_MSG, 718);
        assert_eq!(CLIENT_FRIEND_MSG_INCOMING, 5427);
    }

    #[test]
    fn un_message_recu_du_cm_arrive_dans_la_conversation() {
        let chats = Arc::new(Mutex::new(BTreeMap::new()));
        let mut body = Vec::new();
        proto::field_fixed64(1, 222, &mut body);
        proto::field_varint(2, 1, &mut body);
        proto::field_varint(3, 0, &mut body);
        proto::field_bytes(4, b"bonjour\0", &mut body);
        proto::field_fixed32(5, 1_700_000_000, &mut body);
        let message = receive_friend_message(&chats, &body).expect("message reçu");
        assert_eq!(message.from, 222);
        assert_eq!(message.text, "bonjour");
        assert!(!message.outgoing);
        assert!(message.received_live);
        assert_eq!(message.at, 1_700_000_000);
        let stored = chats
            .lock()
            .map(|chats| chats.get(&222).cloned().unwrap_or_default())
            .unwrap_or_default();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].text, "bonjour");
        // Une saisie en cours (type 2) ne crée aucun message.
        let mut typing = Vec::new();
        proto::field_fixed64(1, 222, &mut typing);
        proto::field_varint(2, 2, &mut typing);
        proto::field_bytes(4, b"...", &mut typing);
        assert!(receive_friend_message(&chats, &typing).is_none());
        assert_eq!(
            chats
                .lock()
                .ok()
                .and_then(|chats| chats.get(&222).map(Vec::len)),
            Some(1)
        );
    }

    fn history_message(account: u32, at: u32, text: &str) -> Vec<u8> {
        let mut entry = Vec::new();
        proto::field_varint(1, u64::from(account), &mut entry);
        proto::field_varint(2, u64::from(at), &mut entry);
        proto::field_bytes(3, text.as_bytes(), &mut entry);
        entry
    }

    #[test]
    fn l_historique_steam_rend_les_deux_auteurs_dans_l_ordre() {
        let steam_id = 76_561_197_960_265_729;
        let friend = 76_561_197_960_265_730;
        let mut body = Vec::new();
        // La réponse de Steam va du plus récent au plus ancien.
        proto::field_bytes(1, &history_message(2, 200, "récent"), &mut body);
        proto::field_bytes(1, &history_message(1, 100, "ancien"), &mut body);
        let page = parse_history(&body, steam_id, friend);
        assert!(!page.more_available);
        assert_eq!(page.cursor, Some((100, 0)));
        let messages = page.messages;
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].text, "ancien");
        assert!(messages[0].outgoing);
        assert_eq!(messages[0].from, steam_id);
        assert_eq!(messages[0].at, 100);
        assert_eq!(messages[1].text, "récent");
        assert!(!messages[1].outgoing);
        assert!(!messages[1].received_live, "l'historique ne notifie pas");
        assert_eq!(messages[1].from, friend);
    }

    #[test]
    fn l_historique_ne_double_pas_les_messages_de_la_session() {
        let steam_id = 76_561_197_960_265_729;
        let friend = 76_561_197_960_265_730;
        let chats = Arc::new(Mutex::new(BTreeMap::from([(
            friend,
            vec![SteamChatMessage {
                from: friend,
                text: "bonjour".into(),
                at: 200,
                outgoing: false,
                received_live: true,
            }],
        )])));
        let mut body = Vec::new();
        proto::field_bytes(1, &history_message(2, 200, "bonjour"), &mut body);
        proto::field_bytes(1, &history_message(1, 100, "avant"), &mut body);
        let page = parse_history(&body, steam_id, friend);
        assert_eq!(merge_history(&chats, friend, page.messages), 1);
        let stored = chats
            .lock()
            .map(|chats| chats.get(&friend).cloned().unwrap_or_default())
            .unwrap_or_default();
        assert_eq!(stored.len(), 2);
        assert_eq!(stored[0].text, "avant");
        assert!(stored[0].outgoing);
        assert_eq!(stored[1].text, "bonjour");
    }

    fn history_page(entries: &[(u32, u32, &str)], more: bool) -> Vec<u8> {
        let mut body = Vec::new();
        for (account, at, text) in entries {
            proto::field_bytes(1, &history_message(*account, *at, text), &mut body);
        }
        if more {
            proto::field_varint(4, 1, &mut body);
        }
        body
    }

    #[test]
    fn les_pages_d_historique_forment_une_chronologie_unique() {
        use lapinux_core::social::SteamChatHistoryState;

        let steam_id = 76_561_197_960_265_729;
        let friend = 76_561_197_960_265_730;
        let chats = Arc::new(Mutex::new(BTreeMap::new()));
        let mut state = SteamChatHistoryState::default();
        // Trois pages du plus récent au plus ancien. Les pages 2 et 3
        // chevauchent la précédente : quatre (400) puis deux (200).
        let pages = [
            (
                history_page(&[(2, 500, "cinq"), (1, 400, "quatre")], true),
                true,
            ),
            (
                history_page(
                    &[(1, 400, "quatre"), (2, 300, "trois"), (1, 200, "deux")],
                    true,
                ),
                true,
            ),
            (
                history_page(&[(1, 200, "deux"), (2, 100, "un")], false),
                false,
            ),
        ];
        for (index, (body, more)) in pages.iter().enumerate() {
            state.loading = true;
            assert!(
                !state.complete(),
                "page {} : fin annoncée pendant le chargement",
                index + 1
            );
            let page = parse_history(body, steam_id, friend);
            merge_history(&chats, friend, page.messages);
            state.loading = false;
            state.error = None;
            state.cursor = page.cursor;
            state.more_available = page.more_available && page.cursor.is_some();
            assert_eq!(state.more_available, *more, "page {}", index + 1);
            assert_eq!(state.complete(), !*more, "page {}", index + 1);
        }
        let stored = chats
            .lock()
            .map(|chats| chats.get(&friend).cloned().unwrap_or_default())
            .unwrap_or_default();
        let texts: Vec<&str> = stored.iter().map(|message| message.text.as_str()).collect();
        assert_eq!(texts, vec!["un", "deux", "trois", "quatre", "cinq"]);
        assert_eq!(stored.len(), 5);
        assert_eq!(
            stored
                .iter()
                .filter(|message| message.text == "quatre")
                .count(),
            1
        );
        assert_eq!(
            stored
                .iter()
                .filter(|message| message.text == "deux")
                .count(),
            1
        );
        assert!(stored.windows(2).all(|pair| pair[0].at <= pair[1].at));
    }

    #[test]
    fn l_historique_demande_les_deux_comptes_et_une_borne() {
        let body = history_request(100, 200, None);
        let mut fields = proto::Reader::new(&body);
        assert_eq!(fields.next(), Some((1, proto::Field::Fixed64(100))));
        assert_eq!(fields.next(), Some((2, proto::Field::Fixed64(200))));
        assert_eq!(
            fields.next(),
            Some((3, proto::Field::Varint(u64::from(HISTORY_MESSAGES))))
        );
        assert_eq!(
            fields.next(),
            Some((5, proto::Field::Fixed32(HISTORY_START_TIME)))
        );
        assert_eq!(fields.next(), None);
    }

    #[test]
    fn l_historique_demande_la_page_precedente_avec_son_curseur() {
        let mut entry = Vec::new();
        proto::field_varint(1, 2, &mut entry);
        proto::field_varint(2, 100, &mut entry);
        proto::field_bytes(3, b"ancien", &mut entry);
        proto::field_varint(4, 7, &mut entry);
        let mut body = Vec::new();
        proto::field_bytes(1, &entry, &mut body);
        proto::field_varint(4, 1, &mut body);
        let page = parse_history(&body, 1, 2);
        assert!(page.more_available);
        assert_eq!(page.cursor, Some((100, 7)));
        assert_eq!(page.messages[0].text, "ancien");

        let request = history_request(100, 200, Some((100, 7)));
        let mut fields = proto::Reader::new(&request);
        assert_eq!(fields.next(), Some((1, proto::Field::Fixed64(100))));
        assert_eq!(fields.next(), Some((2, proto::Field::Fixed64(200))));
        assert_eq!(
            fields.next(),
            Some((3, proto::Field::Varint(u64::from(HISTORY_MESSAGES))))
        );
        assert_eq!(
            fields.next(),
            Some((5, proto::Field::Fixed32(HISTORY_START_TIME)))
        );
        assert_eq!(fields.next(), Some((8, proto::Field::Varint(100))));
        assert_eq!(fields.next(), Some((9, proto::Field::Varint(7))));
        assert_eq!(fields.next(), None);
    }

    fn friend(state: u32, game: Option<&str>, appid: Option<u32>) -> PersonaFriend {
        PersonaFriend {
            friendid: Some(42),
            persona_state: Some(state),
            player_name: Some("Alice".into()),
            game_name: game.map(str::to_string),
            game_played_app_id: appid,
            avatar_hash: Some(vec![0xab; 20]),
            ..PersonaFriend::default()
        }
    }

    /// Le propriétaire de chaque jeu donne son nombre de jeux partagés.
    #[test]
    fn shared_library_counts_games_per_owner() {
        fn app(appid: u64, owners: &[u64]) -> Vec<u8> {
            let mut body = Vec::new();
            proto::field_varint(1, appid, &mut body);
            let mut packed = Vec::new();
            for owner in owners {
                packed.extend_from_slice(&owner.to_le_bytes());
            }
            proto::field_bytes(2, &packed, &mut body);
            body
        }
        let mut response = Vec::new();
        proto::field_bytes(1, &app(10, &[11, 22]), &mut response);
        proto::field_bytes(1, &app(20, &[11]), &mut response);
        proto::field_bytes(1, &app(30, &[33]), &mut response);
        let mut family = SteamFamily {
            members: [11u64, 22, 33, 44]
                .into_iter()
                .map(|steam_id| SteamFamilyMember {
                    steam_id,
                    ..SteamFamilyMember::default()
                })
                .collect(),
            ..SteamFamily::default()
        };
        apply_shared_apps(&response, &mut family);
        assert_eq!(family.members[0].games, 2);
        assert_eq!(family.members[0].appids, vec![10, 20]);
        assert_eq!(family.members[1].games, 1);
        assert_eq!(family.members[1].appids, vec![10]);
        assert_eq!(family.members[2].games, 1);
        assert_eq!(family.members[2].appids, vec![30]);
        assert_eq!(family.members[3].games, 0);
        assert!(family.members[3].appids.is_empty());
    }

    /// Le verdict d'une activation se lit dans l'eresult et son détail.
    #[test]
    fn purchase_response_says_whether_the_key_worked() {
        let mut body = Vec::new();
        proto::field_varint(1, 1, &mut body);
        let ok = parse_purchase(&body);
        assert_eq!(ok.state, "ok", "{ok:?}");
        let mut body = Vec::new();
        proto::field_varint(1, 2, &mut body);
        proto::field_varint(2, 15, &mut body);
        let refused = parse_purchase(&body);
        assert_eq!(refused.state, "error");
        assert!(refused.message.contains("15"), "{}", refused.message);
    }

    #[test]
    fn avatar_url_is_the_sha1_in_hex() {
        let url = avatar_url(&[0x01, 0x23, 0xab]);
        assert_eq!(
            url,
            "https://avatars.fastly.steamstatic.com/0123ab_full.jpg"
        );
    }

    /// SteamID d'un compte individuel, comme Steam en publie (univers 1,
    /// type 1).
    fn person(account: u64) -> u64 {
        (1u64 << 56) | (1 << 52) | account
    }

    /// SteamID d'un groupe : même univers, type 7.
    fn group(account: u64) -> u64 {
        (1u64 << 56) | (7u64 << 52) | account
    }

    #[test]
    fn friends_list_keeps_only_real_friends() {
        let mut live = Live {
            persona_name: "me".into(),
            steam_id: 1,
            cell_id: 0,
            public_ip: None,
            friends: HashMap::new(),
            notifications: Vec::new(),
            web_notifications: Vec::new(),
            wishlist_deals: HashMap::new(),
            friend_events: Vec::new(),
            friends_seen: std::collections::HashSet::new(),
            new_comments: 0,
            family: None,
            presence: SteamPresence::Online,
            applied: SteamPresence::Online,
            redeem: None,
            history: BTreeMap::new(),
            emoticons: Vec::new(),
            stickers: Vec::new(),
        };
        let list = CMsgClientFriendsList {
            bincremental: Some(false),
            friends: vec![
                ListFriend {
                    ulfriendid: Some(person(100)),
                    efriendrelationship: Some(3),
                },
                ListFriend {
                    ulfriendid: Some(person(200)),
                    efriendrelationship: Some(2),
                },
                // Un groupe rangé avec les amis : la liste le laisse dehors.
                ListFriend {
                    ulfriendid: Some(group(300)),
                    efriendrelationship: Some(3),
                },
            ],
            ..CMsgClientFriendsList::default()
        };
        apply_friends_list(&mut live, &list);
        assert_eq!(live.friends.len(), 1);
        assert!(live.friends.contains_key(&person(100)));
    }

    #[test]
    fn an_incremental_removal_drops_the_friend() {
        let mut live = Live {
            persona_name: "me".into(),
            steam_id: 1,
            cell_id: 0,
            public_ip: None,
            friends: HashMap::new(),
            notifications: Vec::new(),
            web_notifications: Vec::new(),
            wishlist_deals: HashMap::new(),
            friend_events: Vec::new(),
            friends_seen: std::collections::HashSet::new(),
            new_comments: 0,
            family: None,
            presence: SteamPresence::Online,
            applied: SteamPresence::Online,
            redeem: None,
            history: BTreeMap::new(),
            emoticons: Vec::new(),
            stickers: Vec::new(),
        };
        live.friends.insert(
            person(100),
            SteamFriend {
                steam_id: person(100),
                ..SteamFriend::default()
            },
        );
        let list = CMsgClientFriendsList {
            bincremental: Some(true),
            friends: vec![ListFriend {
                ulfriendid: Some(person(100)),
                efriendrelationship: Some(0),
            }],
            ..CMsgClientFriendsList::default()
        };
        apply_friends_list(&mut live, &list);
        assert!(live.friends.is_empty());
    }

    #[test]
    fn persona_state_carries_name_state_game_and_avatar() {
        let mut entry = SteamFriend {
            steam_id: 42,
            ..SteamFriend::default()
        };
        apply_persona(&mut entry, &friend(1, Some("Portal 2"), Some(620)));
        assert_eq!(entry.name, "Alice");
        assert_eq!(entry.game, "Portal 2");
        assert_eq!(entry.game_appid, 620);
        assert_eq!(entry.presence, SteamPresence::Play);
        assert!(entry.avatar.ends_with("_full.jpg"));

        apply_persona(&mut entry, &friend(1, None, Some(0)));
        assert_eq!(entry.game_appid, 0);
        assert!(entry.game.is_empty());
    }

    #[test]
    fn changing_appid_drops_the_previous_game_name() {
        let mut entry = SteamFriend {
            steam_id: 42,
            ..SteamFriend::default()
        };
        apply_persona(&mut entry, &friend(1, Some("Portal 2"), Some(620)));
        // Steam peut annoncer le nouveau jeu sans renvoyer son nom. L'ancien
        // titre ne doit pas rester collé au nouvel appid.
        apply_persona(&mut entry, &friend(1, None, Some(730)));
        assert_eq!(entry.game_appid, 730);
        assert!(entry.game.is_empty());

        apply_persona(&mut entry, &friend(1, Some("Counter-Strike 2"), Some(730)));
        assert_eq!(entry.game, "Counter-Strike 2");
    }

    #[test]
    fn le_premier_etat_d_ami_ne_notifie_pas() {
        let mut live = Live {
            persona_name: "moi".into(),
            steam_id: 1,
            cell_id: 0,
            public_ip: None,
            friends: HashMap::new(),
            notifications: Vec::new(),
            web_notifications: Vec::new(),
            wishlist_deals: HashMap::new(),
            friend_events: Vec::new(),
            friends_seen: std::collections::HashSet::new(),
            new_comments: 0,
            family: None,
            presence: SteamPresence::Online,
            applied: SteamPresence::Online,
            redeem: None,
            history: BTreeMap::new(),
            emoticons: Vec::new(),
            stickers: Vec::new(),
        };
        live.friends.insert(
            42,
            SteamFriend {
                steam_id: 42,
                name: "Alice".into(),
                ..SteamFriend::default()
            },
        );
        let online = CMsgClientPersonaState {
            friends: vec![friend(1, None, None)],
            ..CMsgClientPersonaState::default()
        };
        assert_eq!(apply_persona_state(&mut live, &online), 0);
        assert!(live.friend_events.is_empty(), "premier état = référence");

        assert_eq!(apply_persona_state(&mut live, &online), 0);
        let playing = CMsgClientPersonaState {
            friends: vec![friend(1, Some("Portal 2"), Some(620))],
            ..CMsgClientPersonaState::default()
        };
        assert_eq!(apply_persona_state(&mut live, &playing), 1);
        assert_eq!(live.friend_events.len(), 1);
        assert_eq!(
            live.friend_events[0].kind,
            SteamFriendEventKind::GameStarted
        );
        assert_eq!(live.friend_events[0].game, "Portal 2");
    }

    #[test]
    fn connexion_et_lancement_du_meme_instant_sont_publies_du_plus_recent_au_plus_ancien() {
        let mut live = Live {
            persona_name: "moi".into(),
            steam_id: 1,
            cell_id: 0,
            public_ip: None,
            friends: HashMap::new(),
            notifications: Vec::new(),
            web_notifications: Vec::new(),
            wishlist_deals: HashMap::new(),
            friend_events: Vec::new(),
            friends_seen: std::collections::HashSet::new(),
            new_comments: 0,
            family: None,
            presence: SteamPresence::Online,
            applied: SteamPresence::Online,
            redeem: None,
            history: BTreeMap::new(),
            emoticons: Vec::new(),
            stickers: Vec::new(),
        };
        live.friends.insert(
            42,
            SteamFriend {
                steam_id: 42,
                name: "Alice".into(),
                ..SteamFriend::default()
            },
        );
        let offline = CMsgClientPersonaState {
            friends: vec![friend(0, None, None)],
            ..CMsgClientPersonaState::default()
        };
        assert_eq!(apply_persona_state(&mut live, &offline), 0);
        let playing = CMsgClientPersonaState {
            friends: vec![friend(6, Some("Garry's Mod"), Some(4000))],
            ..CMsgClientPersonaState::default()
        };
        assert_eq!(apply_persona_state(&mut live, &playing), 2);
        // La liste est du plus récent au plus ancien : le lancement précède
        // la connexion. L'interface la rejoue à l'envers pour notifier dans
        // l'ordre réel.
        assert_eq!(
            live.friend_events
                .iter()
                .map(|event| event.kind)
                .collect::<Vec<_>>(),
            vec![
                SteamFriendEventKind::GameStarted,
                SteamFriendEventKind::Online
            ]
        );
    }

    #[test]
    fn les_transitions_d_ami_produisent_des_evenements() {
        let offline = SteamFriend {
            steam_id: 42,
            name: "Alice".into(),
            avatar: "https://avatars.steamstatic.com/alice.jpg".into(),
            ..SteamFriend::default()
        };
        let online = SteamFriend {
            presence: SteamPresence::Online,
            ..offline.clone()
        };
        let events = friend_events_between(&offline, &online, 1);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, SteamFriendEventKind::Online);
        assert_eq!(events[0].name, "Alice");
        assert_eq!(
            events[0].avatar,
            "https://avatars.steamstatic.com/alice.jpg"
        );

        let playing = SteamFriend {
            presence: SteamPresence::Play,
            game: "Portal 2".into(),
            game_appid: 620,
            ..online.clone()
        };
        let events = friend_events_between(&online, &playing, 2);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, SteamFriendEventKind::GameStarted);
        assert_eq!(events[0].game, "Portal 2");
        assert_eq!(events[0].appid, 620);

        let events = friend_events_between(&playing, &online, 3);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, SteamFriendEventKind::GameStopped);
        assert_eq!(events[0].game, "Portal 2");

        let events = friend_events_between(&online, &offline, 4);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, SteamFriendEventKind::Offline);

        let away = SteamFriend {
            presence: SteamPresence::Away,
            ..online.clone()
        };
        assert!(friend_events_between(&online, &away, 5).is_empty());
    }

    #[test]
    fn our_own_persona_confirms_the_applied_state() {
        let mut live = Live {
            persona_name: "moi".into(),
            steam_id: 7,
            cell_id: 0,
            public_ip: None,
            friends: HashMap::new(),
            notifications: Vec::new(),
            web_notifications: Vec::new(),
            wishlist_deals: HashMap::new(),
            friend_events: Vec::new(),
            friends_seen: std::collections::HashSet::new(),
            new_comments: 0,
            family: None,
            presence: SteamPresence::Invisible,
            applied: SteamPresence::Online,
            redeem: None,
            history: BTreeMap::new(),
            emoticons: Vec::new(),
            stickers: Vec::new(),
        };
        let state = CMsgClientPersonaState {
            status_flags: Some(PERSONA_FLAGS),
            friends: vec![PersonaFriend {
                friendid: Some(7),
                persona_state: Some(SteamPresence::Invisible.code()),
                player_name: Some("moi".into()),
                ..PersonaFriend::default()
            }],
        };
        apply_persona_state(&mut live, &state);
        assert_eq!(live.applied, SteamPresence::Invisible);
        assert!(live.friends.is_empty());
    }

    /// Trame telle que le CM l'écrit : EMsg protobuf, longueur d'en-tête,
    /// en-tête, corps — sans la magie ni la longueur du transport.
    fn frame(emsg: u32, header: CMsgProtoBufHeader, body: &[u8]) -> Vec<u8> {
        let header = header.encode_to_vec();
        let mut out = Vec::new();
        out.extend_from_slice(&(emsg | 0x8000_0000).to_le_bytes());
        out.extend_from_slice(&(header.len() as u32).to_le_bytes());
        out.extend_from_slice(&header);
        out.extend_from_slice(body);
        out
    }

    fn multi(inner: &[Vec<u8>]) -> IncomingMsg {
        let mut payload = Vec::new();
        for message in inner {
            payload.extend_from_slice(&(message.len() as u32).to_le_bytes());
            payload.extend_from_slice(message);
        }
        let batch = steamroom::generated::CMsgMulti {
            message_body: Some(payload.into()),
            size_unzipped: None,
        };
        IncomingMsg {
            emsg: EMsg::MULTI,
            is_protobuf: true,
            header: CMsgProtoBufHeader::default(),
            body: batch.encode_to_vec().into(),
        }
    }

    #[test]
    fn a_plain_message_is_its_own_batch() {
        let state = CMsgClientPersonaState {
            friends: vec![friend(1, None, None)],
            ..CMsgClientPersonaState::default()
        };
        let message = IncomingMsg {
            emsg: EMsg(CLIENT_PERSONA_STATE),
            is_protobuf: true,
            header: CMsgProtoBufHeader::default(),
            body: state.encode_to_vec().into(),
        };
        let expanded = expand(message);
        assert_eq!(expanded.len(), 1);
        assert_eq!(expanded[0].emsg.0, CLIENT_PERSONA_STATE);
    }

    #[test]
    fn a_batch_gives_back_every_message_it_carried() {
        // Ce que Steam envoie après un logon : la liste d'amis et les états
        // voyagent avec d'autres poussées dans un seul `Multi`.
        let list = CMsgClientFriendsList {
            bincremental: Some(false),
            friends: vec![ListFriend {
                ulfriendid: Some(100),
                efriendrelationship: Some(3),
            }],
            ..CMsgClientFriendsList::default()
        };
        let state = CMsgClientPersonaState {
            friends: vec![friend(1, Some("Portal 2"), Some(620))],
            ..CMsgClientPersonaState::default()
        };
        let batch = multi(&[
            frame(
                CLIENT_FRIENDS_LIST,
                CMsgProtoBufHeader {
                    steamid: Some(7),
                    ..Default::default()
                },
                &list.encode_to_vec(),
            ),
            frame(
                CLIENT_PERSONA_STATE,
                CMsgProtoBufHeader::default(),
                &state.encode_to_vec(),
            ),
        ]);
        let expanded = expand(batch);
        assert_eq!(expanded.len(), 2);
        assert_eq!(expanded[0].emsg.0, CLIENT_FRIENDS_LIST);
        assert_eq!(expanded[0].header.steamid, Some(7));
        assert_eq!(expanded[1].emsg.0, CLIENT_PERSONA_STATE);
        let decoded = CMsgClientPersonaState::decode(&*expanded[1].body).unwrap();
        assert_eq!(decoded.friends.len(), 1);
        assert_eq!(decoded.friends[0].game_name.as_deref(), Some("Portal 2"));
    }

    #[test]
    fn an_unreadable_batch_yields_nothing() {
        let message = IncomingMsg {
            emsg: EMsg::MULTI,
            is_protobuf: true,
            header: CMsgProtoBufHeader::default(),
            body: bytes::Bytes::from_static(b"pas un protobuf"),
        };
        assert!(expand(message).is_empty());
    }

    #[test]
    fn the_api_friend_list_is_read_as_sent() {
        // Forme réelle de `IFriendsListService/GetFriendsList` : les
        // identifiants arrivent en texte, sous `friendslist`.
        let body = r#"{"response":{"friendslist":{"bincremental":false,"friends":[
            {"ulfriendid":"76561198000000001","efriendrelationship":3},
            {"ulfriendid":"76561198000000002","efriendrelationship":5}],
            "max_friend_count":2}}}"#;
        let (friends, total) = parse_friend_list(body).unwrap();
        assert_eq!(total, 2);
        assert_eq!(
            friends,
            vec![(76_561_198_000_000_001, 3), (76_561_198_000_000_002, 5)]
        );
    }

    #[test]
    fn an_api_error_is_not_a_friend_list() {
        assert!(parse_friend_list(r#"{"response":{}}"#).is_none());
        assert!(parse_friend_list("pas du json").is_none());
    }

    #[test]
    fn a_family_group_is_read_field_by_field() {
        let mut member = Vec::new();
        proto::field_fixed64(1, 111, &mut member);
        proto::field_varint(2, 1, &mut member);
        let mut body = Vec::new();
        proto::field_bytes(1, "Maison".as_bytes(), &mut body);
        proto::field_bytes(2, &member, &mut body);
        let family = parse_family_group(&body);
        assert_eq!(family.name, "Maison");
        assert_eq!(family.members.len(), 1);
        assert_eq!(family.members[0].steam_id, 111);
        // Le rôle vient tel quel : rien n'est nommé ici, le nom et la photo
        // suivent par la fiche de joueur.
        assert_eq!(family.members[0].role, 1);
        assert!(family.members[0].name.is_empty());
        assert!(family.members[0].avatar.is_empty());
    }

    #[test]
    fn family_members_borrow_the_name_and_photo_of_their_player_card() {
        let mut live = Live {
            persona_name: "moi".into(),
            steam_id: 1,
            cell_id: 0,
            public_ip: None,
            friends: HashMap::new(),
            notifications: Vec::new(),
            web_notifications: Vec::new(),
            wishlist_deals: HashMap::new(),
            friend_events: Vec::new(),
            friends_seen: std::collections::HashSet::new(),
            new_comments: 0,
            family: Some(SteamFamily {
                name: "Maison".into(),
                members: vec![
                    SteamFamilyMember {
                        steam_id: 111,
                        role: 1,
                        ..Default::default()
                    },
                    SteamFamilyMember {
                        steam_id: 222,
                        role: 2,
                        ..Default::default()
                    },
                ],
                ..SteamFamily::default()
            }),
            presence: SteamPresence::Online,
            applied: SteamPresence::Online,
            redeem: None,
            history: BTreeMap::new(),
            emoticons: Vec::new(),
            stickers: Vec::new(),
        };
        live.friends.insert(
            111,
            SteamFriend {
                steam_id: 111,
                name: "JoueurExemple".into(),
                avatar: "https://avatars.example/joueur_full.jpg".into(),
                ..SteamFriend::default()
            },
        );
        let chats = std::sync::Arc::new(std::sync::Mutex::new(std::collections::BTreeMap::new()));
        let social = live.snapshot(&chats);
        let family = social.family.expect("famille publiée");
        assert_eq!(family.members[0].name, "JoueurExemple");
        assert_eq!(
            family.members[0].avatar,
            "https://avatars.example/joueur_full.jpg"
        );
        // Un membre sans fiche connue reste sans nom : rien n'est inventé.
        assert!(family.members[1].name.is_empty());
        assert!(family.members[1].avatar.is_empty());
    }

    #[test]
    fn heartbeat_interval_is_read_from_the_first_varint() {
        let mut body = Vec::new();
        proto::field_varint(1, 30, &mut body);
        assert_eq!(first_varint(&body), Some(30));
        assert_eq!(first_varint(&[]), None);
    }

    const COMPTE_TEST: u64 = 76_561_198_000_000_000;
    const AMI_TEST: u64 = 76_561_198_000_000_001;
    // Écrits en clair exprès : le test doit échouer si on réintroduit 776 ou
    // 773 dans les constantes du client. 5427 est `ClientFriendMsgIncoming`,
    // 718 `ClientFriendMsg` (SteamKit `emsg.h`, SteamTracking
    // `steammessages_clientserver_friends.proto`).
    const EMSG_AMI_ENTRANT: u32 = 5427;
    const EMSG_AMI_SORTANT: u32 = 718;

    /// Un faux CM local : WebSocket, trames protobuf et logon minimal. Il ne
    /// simule que ce que le client utilise pour le chat ami.
    struct FauxCm {
        url: String,
        recus: tokio::sync::mpsc::UnboundedReceiver<(u32, CMsgClientFriendMsg)>,
        pousser: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
    }

    async fn demarrer_cm() -> FauxCm {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("port de test");
        let port = listener.local_addr().expect("adresse locale").port();
        let (pousser, mut a_pousser): (
            tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
            tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
        ) = tokio::sync::mpsc::unbounded_channel();
        let (recus_tx, recus) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            let Ok((socket, _)) = listener.accept().await else {
                return;
            };
            let Ok(ws) = tokio_tungstenite::accept_async(socket).await else {
                return;
            };
            let (mut sink, mut source) = ws.split();
            loop {
                tokio::select! {
                    envoye = a_pousser.recv() => {
                        let Some(trame) = envoye else { break };
                        if sink
                            .send(WsMessage::Binary(trame.into()))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    message = source.next() => {
                        let Some(Ok(WsMessage::Binary(data))) = message else {
                            break;
                        };
                        let Ok(PacketHeader::Protobuf { header, body }) =
                            PacketHeader::parse(&data)
                        else {
                            continue;
                        };
                        if header.emsg == EMsg::CLIENT_LOGON {
                            let reponse = CMsgClientLogonResponse {
                                eresult: Some(1),
                                heartbeat_seconds: Some(15),
                                ..CMsgClientLogonResponse::default()
                            };
                            let entete = CMsgProtoBufHeader {
                                steamid: Some(COMPTE_TEST),
                                client_sessionid: Some(4242),
                                ..CMsgProtoBufHeader::default()
                            };
                            let trame = frame(
                                EMsg::CLIENT_LOG_ON_RESPONSE.0,
                                entete,
                                &reponse.encode_to_vec(),
                            );
                            if sink
                                .send(WsMessage::Binary(trame.into()))
                                .await
                                .is_err()
                            {
                                break;
                            }
                        } else if header.emsg.0 == EMSG_AMI_SORTANT {
                            if let Ok(message) = CMsgClientFriendMsg::decode(&*body) {
                                let _ = recus_tx.send((header.emsg.0, message));
                            }
                        }
                    }
                }
            }
        });
        FauxCm {
            url: format!("ws://127.0.0.1:{port}/cmsocket/"),
            recus,
            pousser,
        }
    }

    type WsStream = tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >;

    /// Le transport que `SteamClient::connect_ws` attend. En production le
    /// WebSocket est en `wss://` ; ici il est local et en clair, mais les
    /// octets — trames, en-têtes protobuf, corps — sont les mêmes.
    struct TransportLocal {
        sink: tokio::sync::Mutex<futures_util::stream::SplitSink<WsStream, WsMessage>>,
        source: tokio::sync::Mutex<futures_util::stream::SplitStream<WsStream>>,
    }

    fn io_error(error: impl std::fmt::Display) -> steamroom::Error {
        steamroom::Error::Io(std::io::Error::new(
            std::io::ErrorKind::Other,
            error.to_string(),
        ))
    }

    impl TransportLocal {
        async fn connecter(url: &str) -> Self {
            let (ws, _) = tokio::time::timeout(
                Duration::from_secs(5),
                tokio_tungstenite::connect_async(url),
            )
            .await
            .expect("WebSocket de test")
            .expect("connexion au faux CM");
            let (sink, source) = ws.split();
            Self {
                sink: tokio::sync::Mutex::new(sink),
                source: tokio::sync::Mutex::new(source),
            }
        }
    }

    impl Transport for TransportLocal {
        fn send(
            &self,
            payload: &[u8],
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<(), steamroom::Error>> + Send + '_>,
        > {
            let payload = payload.to_vec();
            Box::pin(async move {
                self.sink
                    .lock()
                    .await
                    .send(WsMessage::Binary(payload.into()))
                    .await
                    .map_err(io_error)
            })
        }

        fn recv(
            &self,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = Result<bytes::Bytes, steamroom::Error>>
                    + Send
                    + '_,
            >,
        > {
            Box::pin(async move {
                loop {
                    let message = self.source.lock().await.next().await;
                    match message {
                        Some(Ok(WsMessage::Binary(data))) => return Ok(data),
                        Some(Ok(WsMessage::Text(text))) => {
                            return Ok(bytes::Bytes::from(text.to_string()))
                        }
                        Some(Ok(WsMessage::Close(_))) | None => {
                            return Err(steamroom::Error::Io(std::io::Error::new(
                                std::io::ErrorKind::UnexpectedEof,
                                "faux CM fermé",
                            )))
                        }
                        Some(Ok(_)) => continue,
                        Some(Err(error)) => return Err(io_error(error)),
                    }
                }
            })
        }
    }

    fn entete_ami() -> CMsgProtoBufHeader {
        CMsgProtoBufHeader {
            steamid: Some(AMI_TEST),
            client_sessionid: Some(4242),
            ..CMsgProtoBufHeader::default()
        }
    }

    async fn attendre<T>(mut condition: impl FnMut() -> Option<T>) -> T {
        for _ in 0..500 {
            if let Some(value) = condition() {
                return value;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("le faux CM n'a rien livré en 5 s");
    }

    /// Le chemin réel « CM → chat de l'overlay » : un vrai client `steamroom`
    /// se connecte à un faux CM local, fait le logon, reçoit 5427 au format
    /// protobuf, publie la conversation dans l'instantané, puis envoie 718.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn le_vrai_client_recoit_et_envoie_les_messages_d_ami() {
        let mut cm = demarrer_cm().await;
        let transport = TransportLocal::connecter(&cm.url).await;
        let (client, _reception) = SteamClient::connect_ws(transport)
            .await
            .expect("client WebSocket");
        let client = client.prepare().await.expect("ClientHello");
        let client = tokio::time::timeout(
            Duration::from_secs(5),
            PreparedLoginBuilder::new(client)
                .device_name("lapinux-test")
                .with_refresh_token("compte-de-test", "jeton-de-test")
                .login(),
        )
        .await
        .expect("logon du faux CM")
        .expect("logon accepté");

        let session = SavedSession {
            account_name: "compte-de-test".into(),
            steam_id: COMPTE_TEST,
            refresh_token: "jeton-de-test".into(),
            guard_data: None,
            saved_at: None,
        };
        let snapshot = Arc::new(Mutex::new(SteamSocial {
            state: "connecting".into(),
            steam_id: COMPTE_TEST,
            persona_name: session.account_name.clone(),
            ..SteamSocial::default()
        }));
        let chats = Arc::new(Mutex::new(BTreeMap::new()));
        let (commands, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let stop = Arc::new(AtomicBool::new(false));
        let playing = Arc::new(AtomicU64::new(0));
        let playing_name = Arc::new(Mutex::new(String::new()));
        let mut presence = SteamPresence::Online;
        let mut pending_redeem = None;
        let tache = tokio::spawn({
            let snapshot = Arc::clone(&snapshot);
            let chats = Arc::clone(&chats);
            let stop = Arc::clone(&stop);
            let playing = Arc::clone(&playing);
            let playing_name = Arc::clone(&playing_name);
            let session = session.clone();
            async move {
                connect_with(
                    client,
                    &session,
                    &snapshot,
                    &mut receiver,
                    &stop,
                    &playing,
                    &playing_name,
                    &chats,
                    &mut presence,
                    &mut pending_redeem,
                )
                .await
            }
        });

        assert!(
            chats.lock().map(|chats| chats.is_empty()).unwrap_or(false),
            "aucune conversation ne préexiste"
        );

        // 1. Un ami inconnu de l'état : le CM pousse un vrai 5427, texte au
        //    champ 4, avec l'octet NUL que le client Steam laisse parfois.
        let premier = CMsgClientFriendMsgIncoming {
            steamid_from: Some(AMI_TEST),
            chat_entry_type: Some(1),
            from_limited_account: Some(false),
            message: Some(b"salut du vrai CM\0".to_vec()),
            rtime32_server_timestamp: Some(1_700_000_123),
        };
        cm.pousser
            .send(frame(
                EMSG_AMI_ENTRANT,
                entete_ami(),
                &premier.encode_to_vec(),
            ))
            .expect("poussée 5427");
        let conversation = attendre(|| {
            snapshot
                .lock()
                .ok()
                .and_then(|social| social.chats.get(&AMI_TEST).cloned())
                .filter(|messages| !messages.is_empty())
        })
        .await;
        assert_eq!(conversation.len(), 1);
        assert_eq!(conversation[0].from, AMI_TEST);
        assert_eq!(conversation[0].text, "salut du vrai CM");
        assert!(!conversation[0].text.contains('\0'));
        assert_eq!(conversation[0].at, 1_700_000_123);
        assert!(!conversation[0].outgoing);
        assert_eq!(
            snapshot.lock().expect("instantané").state,
            "live",
            "l'instantané a été republié après réception"
        );

        // 3. Un second message s'ajoute à la même conversation, sans doublon
        //    ni écrasement du premier.
        let second = CMsgClientFriendMsgIncoming {
            steamid_from: Some(AMI_TEST),
            chat_entry_type: Some(1),
            from_limited_account: Some(false),
            message: Some(b"deuxieme message".to_vec()),
            rtime32_server_timestamp: Some(1_700_000_456),
        };
        cm.pousser
            .send(frame(
                EMSG_AMI_ENTRANT,
                entete_ami(),
                &second.encode_to_vec(),
            ))
            .expect("seconde poussée 5427");
        let conversation = attendre(|| {
            snapshot
                .lock()
                .ok()
                .and_then(|social| social.chats.get(&AMI_TEST).cloned())
                .filter(|messages| messages.len() == 2)
        })
        .await;
        assert_eq!(conversation[0].text, "salut du vrai CM");
        assert_eq!(conversation[0].at, 1_700_000_123);
        assert_eq!(conversation[1].text, "deuxieme message");
        assert_eq!(conversation[1].at, 1_700_000_456);
        assert_eq!(
            snapshot.lock().expect("instantané").chats.len(),
            1,
            "une seule conversation pour cet ami"
        );

        // 2. L'overlay écrit : le faux CM reçoit 718 et le décode avec le
        //    protobuf généré, destinataire et texte au bon champ.
        let social = Social {
            snapshot: Arc::clone(&snapshot),
            commands,
            playing: Arc::clone(&playing),
            playing_name: Arc::clone(&playing_name),
            stop: Arc::clone(&stop),
        };
        assert!(social.message(AMI_TEST, "réponse de l'overlay"));
        let (emsg, envoye) = tokio::time::timeout(Duration::from_secs(5), cm.recus.recv())
            .await
            .expect("718 attendu")
            .expect("message décodé");
        assert_eq!(emsg, EMSG_AMI_SORTANT);
        assert_eq!(envoye.steamid, Some(AMI_TEST));
        assert_eq!(envoye.chat_entry_type, Some(1));
        assert_eq!(
            envoye.message.as_deref(),
            Some("réponse de l'overlay".as_bytes())
        );

        // La copie locale du message envoyé rejoint l'instantané, sans
        // attendre un écho du CM.
        let conversation = attendre(|| {
            snapshot
                .lock()
                .ok()
                .and_then(|social| social.chats.get(&AMI_TEST).cloned())
                .filter(|messages| messages.len() == 3)
        })
        .await;
        assert!(conversation[2].outgoing);
        assert_eq!(conversation[2].text, "réponse de l'overlay");

        // Fermer le canal de commandes rend la main à la boucle d'écoute.
        drop(social);
        let fin = tokio::time::timeout(Duration::from_secs(5), tache)
            .await
            .expect("la session doit rendre la main")
            .expect("tâche de session");
        fin.expect("session sans erreur");
    }

    #[test]
    fn la_liste_d_emotes_lit_les_noms_et_les_compteurs() {
        let mut entry = Vec::new();
        proto::field_bytes(1, b":steamhappy:", &mut entry);
        proto::field_varint(2, 3, &mut entry);
        proto::field_varint(6, 753, &mut entry);
        let mut body = Vec::new();
        proto::field_bytes(1, &entry, &mut body);

        let emoticons = parse_emoticons(&body);

        assert_eq!(
            emoticons,
            vec![SteamEmoticon {
                name: "steamhappy".to_string(),
                count: 3,
                appid: 753,
            }]
        );
    }

    #[test]
    fn la_collection_lit_les_emotes_et_les_autocollants() {
        let mut emote = Vec::new();
        proto::field_bytes(1, b":steamhappy:", &mut emote);
        proto::field_varint(2, 1, &mut emote);
        proto::field_varint(6, 753, &mut emote);
        let mut sticker = Vec::new();
        proto::field_bytes(1, b"sticker_1", &mut sticker);
        proto::field_varint(2, 2, &mut sticker);
        proto::field_varint(4, 730, &mut sticker);
        let mut body = Vec::new();
        proto::field_bytes(1, &emote, &mut body);
        proto::field_bytes(2, &sticker, &mut body);

        let (emoticons, stickers) = parse_client_emoticon_list(&body);

        assert_eq!(emoticons.len(), 1);
        assert_eq!(emoticons[0].name, "steamhappy");
        assert_eq!(
            stickers,
            vec![SteamSticker {
                name: "sticker_1".to_string(),
                count: 2,
                appid: 730,
            }]
        );
    }

    #[test]
    fn les_notifications_web_lisent_le_contenu_et_ecartent_les_commentaires() {
        let body = r#"{
            "response": {
                "notifications": [
                    {
                        "notification_type": 8,
                        "body_data": "{\"title\":\"Un jeu\",\"body\":\"en promotion\",\"appid\":123}",
                        "read": false,
                        "hidden": false,
                        "timestamp": 1700000000
                    },
                    {
                        "notification_type": 3,
                        "body_data": "{\"comment\":\"un commentaire\"}",
                        "read": false,
                        "timestamp": 1700000001
                    },
                    {
                        "notification_type": 2,
                        "body_data": "{}",
                        "read": true,
                        "timestamp": 1700000002
                    }
                ]
            }
        }"#;

        let notifications = parse_web_notifications(body);

        assert_eq!(
            notifications.len(),
            1,
            "commentaire et notification lue écartés"
        );
        assert_eq!(notifications[0].kind, 8);
        assert_eq!(notifications[0].title, "Un jeu");
        assert_eq!(notifications[0].body, "en promotion");
        assert_eq!(notifications[0].appids, vec![123]);
        assert_eq!(notifications[0].url, WISHLIST_PROMOTIONS_URL);
        assert_eq!(notifications[0].at, 1_700_000_000);
    }

    #[test]
    fn une_wishlist_sans_texte_decrit_ses_jeux() {
        let (title, body, url, appids) = parse_web_body(
            8,
            r#"{"appid":3905450,"count":2,"appids":[3905450,3278310]}"#,
        );
        assert!(title.is_empty());
        assert_eq!(body, "2 jeux de votre liste de souhaits");
        assert_eq!(url, WISHLIST_PROMOTIONS_URL);
        assert_eq!(appids, vec![3905450, 3278310]);
    }

    #[test]
    fn les_liens_wishlist_groupent_les_appids_du_compte() {
        assert_eq!(
            wishlist_promotions_url(76561198272259682, &[3905450, 3278310]),
            "https://store.steampowered.com/wishlist/profiles/76561198272259682/?sort=discount&wng=3905450%2C3278310"
        );
        assert_eq!(wishlist_promotions_url(0, &[1]), WISHLIST_PROMOTIONS_URL);

        let mut notification = SteamWebNotification {
            kind: 8,
            appids: vec![3905450, 3278310],
            url: WISHLIST_PROMOTIONS_URL.to_string(),
            ..SteamWebNotification::default()
        };
        apply_wishlist_urls(std::slice::from_mut(&mut notification), 76561198272259682);
        assert_eq!(
            notification.url,
            "https://store.steampowered.com/wishlist/profiles/76561198272259682/?sort=discount&wng=3905450%2C3278310"
        );
    }

    #[test]
    fn les_wishlists_lisent_les_noms_et_les_prix() {
        let body = r#"{
            "3905450": {"success": true, "data": {
                "type": "game",
                "name": "Jeu A",
                "price_overview": {"final_formatted": "4,99€", "discount_percent": 75}
            }},
            "3278310": {"success": false}
        }"#;
        let deals = parse_wishlist_deals(body, &[3905450, 3278310, 999]);
        let deal = deals.get(&3905450).expect("fiche lue");
        assert_eq!(deal.name, "Jeu A");
        assert_eq!(deal.price, "4,99€");
        assert_eq!(deal.discount, 75);
        assert!(!deals.contains_key(&3278310));
        assert!(!deals.contains_key(&999));
    }

    #[test]
    fn un_body_data_sans_json_reste_affichable() {
        let (title, body, url, appids) = parse_web_body(7, "préchargement disponible");
        assert!(title.is_empty());
        assert_eq!(body, "préchargement disponible");
        assert!(url.is_empty());
        assert!(appids.is_empty());
    }
}
