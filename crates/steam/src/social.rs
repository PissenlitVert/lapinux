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
    SteamChatMessage, SteamKeyRedeem,
    SteamFamily, SteamFamilyMember, SteamFriend, SteamNotification, SteamPresence, SteamSocial,
};
use lapinux_core::util::now_unix;
use prost::Message;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use steamroom::client::msg::ClientMsg;
use steamroom::client::{LoggedIn, SteamClient};
use steamroom::client::multi;
use steamroom::client::IncomingMsg;
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
/// `k_EMsgClientFriendMsg` : message envoyé à un ami.
const CLIENT_FRIEND_MSG: u32 = 773;
/// `k_EMsgClientFriendMsgIncoming` : message reçu d'un ami.
const CLIENT_FRIEND_MSG_INCOMING: u32 = 776;
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

/// Commandes reçues par la session depuis le daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    pub fn start(session: SavedSession, presence: SteamPresence) -> Result<Self, lapinux_core::Error> {
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
        if self.commands.send(SocialCommand::Presence(presence)).is_err() {
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
    friends: HashMap<u64, SteamFriend>,
    notifications: Vec<SteamNotification>,
    new_comments: u32,
    family: Option<SteamFamily>,
    /// Présence demandée par l'utilisateur, publiée sur le réseau.
    presence: SteamPresence,
    /// Présence confirmée par Steam (`CMsgPersonaChangeResponse`).
    applied: SteamPresence,
    /// Dernière activation de clé, telle qu'elle s'affiche.
    redeem: Option<SteamKeyRedeem>,
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
    loop {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        publish(&snapshot, SteamSocial::connecting(), presence, session.steam_id);
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
                publish(
                    &snapshot,
                    SteamSocial::failed(error),
                    presence,
                    session.steam_id,
                );
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
                        playing.store(u64::from(appid.unwrap_or(0)), Ordering::SeqCst);
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
    let client = tokio::time::timeout(
        LOGIN_TIMEOUT,
        LoginBuilder::new()
            .device_name(lapinux_core::APP_NAME)
            .with_refresh_token(session.account_name.clone(), session.refresh_token.clone())
            .login(),
    )
    .await
    .map_err(|_| "connexion au réseau Steam expirée".to_string())?
    .map_err(|error| format!("connexion au réseau Steam : {error}"))?;

    let mut live = Live {
        persona_name: session.account_name.clone(),
        steam_id: session.steam_id,
        friends: HashMap::new(),
        notifications: Vec::new(),
        new_comments: 0,
        family: None,
        presence: *presence,
        applied: *presence,
        redeem: None,
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
    let mut calls = Calls { next: 0, renew: None, family: None, shared: None };
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
            job,
            "Authentication.GenerateAccessTokenForApp#1",
            &request,
        )
        .await?;
        calls.renew = Some(job);
    }

    let mut heartbeat = tokio::time::interval(DEFAULT_HEARTBEAT);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut family_done = false;
    let mut friends_asked = false;
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
                    playing.store(u64::from(appid), Ordering::SeqCst);
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
                Some(SocialCommand::Message { steam_id, text }) => {
                    match send_friend_message(&client, live.steam_id, steam_id, &text).await {
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
            message = client.recv_msg() => {
                let message = message.map_err(|error| format!("session Steam interrompue : {error}"))?;
                for message in expand(message) {
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
                                    }
                                } else {
                                    tracing::debug!(
                                        "entretien du jeton refusé ({:?}) : la session enregistrée reste valable",
                                        message.header.eresult
                                    );
                                }
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
                            }
                        }
                        CLIENT_PERSONA_STATE => {
                            if let Ok(state) = CMsgClientPersonaState::decode(&*message.body) {
                                apply_persona_state(&mut live, &state);
                            }
                        }
                        CLIENT_USER_NOTIFICATIONS => {
                            if let Ok(notifications) = CMsgClientUserNotifications::decode(&*message.body) {
                                live.notifications = notifications
                                    .notifications
                                    .iter()
                                    .filter(|notification| notification.count.unwrap_or(0) > 0)
                                    .map(|notification| SteamNotification {
                                        kind: notification.user_notification_type.unwrap_or(0),
                                        count: notification.count.unwrap_or(0),
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
                            if let Some((from, text, at)) =
                                parse_incoming_friend_msg(&message.body)
                            {
                                remember_chat(
                                    chats,
                                    from,
                                    SteamChatMessage {
                                        from,
                                        text,
                                        at: if at > 0 { at } else { now_unix() },
                                        outgoing: false,
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
        }
    }
}

/// Appels de service en attente : jamais plus d'un renouvellement et d'une
/// demande de famille à la fois.
struct Calls {
    next: u64,
    renew: Option<u64>,
    family: Option<u64>,
    /// Bibliothèque partagée de la famille, demandée après le groupe.
    shared: Option<u64>,
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
    job: u64,
    method: &str,
    request: &[u8],
) -> Result<(), String> {
    let mut message = ClientMsg::with_body(EMsg::SERVICE_METHOD_CALL_FROM_CLIENT, request);
    message.header.target_job_name = Some(method.to_string());
    message.header.jobid_source = Some(job);
    // Un appel de service porte l'identité de la session : le CM refuse ce
    // qu'il ne peut pas rattacher à un compte connecté.
    message.header.steamid = Some(steam_id);
    client
        .send_msg(&message)
        .await
        .map_err(|error| format!("appel à {method} refusé : {error}"))
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
            state: "live".into(),
            error: None,
            persona_name: self.persona_name.clone(),
            steam_id: self.steam_id,
            presence: self.presence,
            applied_presence: self.applied,
            friends: self.friends.values().cloned().collect(),
            notifications: self.notifications.clone(),
            chats: chats.lock().map(|chats| chats.clone()).unwrap_or_default(),
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

/// Corps de `CMsgClientFriendMsg` : le destinataire, le type texte et le
/// message. `echo_to_sender` est faux : Lapinux ajoute lui-même la copie
/// locale, sans attendre un écho du CM.
fn friend_msg_message(destination: u64, text: &str) -> Vec<u8> {
    let mut body = Vec::new();
    proto::field_fixed64(1, destination, &mut body);
    // `EChatEntryType::ChatMsg`.
    proto::field_varint(2, 1, &mut body);
    proto::field_bytes(3, text.as_bytes(), &mut body);
    proto::field_varint(4, 0, &mut body);
    body
}

/// Lit `CMsgClientFriendMsgIncoming`. Les champs 3 et 4 sont acceptés pour le
/// texte : selon la version du CM, le message arrive dans l'un ou l'autre.
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
            (3, proto::Field::Bytes(message)) | (4, proto::Field::Bytes(message)) => {
                text = Some(message.to_vec());
            }
            (5, proto::Field::Fixed32(timestamp)) => at = i64::from(timestamp),
            _ => {}
        }
    }
    if from == 0 || !matches!(kind, 0 | 1) {
        return None;
    }
    let text = String::from_utf8_lossy(&text?).trim().to_string();
    if text.is_empty() {
        return None;
    }
    Some((from, text, at))
}

/// Envoie un message texte au CM.
async fn send_friend_message(
    client: &SteamClient<LoggedIn>,
    source: u64,
    destination: u64,
    text: &str,
) -> Result<(), String> {
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
    if conversation.len() > 200 {
        let excess = conversation.len() - 200;
        conversation.drain(0..excess);
    }
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
        }
    }
}

/// Un compte individuel : le type se lit aux bits 52 à 55 du SteamID (1 pour
/// une personne, 7 pour un groupe, 8 pour un salon). Les groupes ne sont pas
/// des amis, même quand Steam les range avec eux.
fn is_person(steam_id: u64) -> bool {
    ((steam_id >> 52) & 0xf) == 1
}

/// Fusionne un état de présence poussé par le CM.
fn apply_persona_state(live: &mut Live, state: &CMsgClientPersonaState) {
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
        apply_persona(entry, friend);
    }
}

fn apply_persona(entry: &mut SteamFriend, friend: &PersonaFriend) {
    if let Some(name) = friend.player_name.as_ref().filter(|name| !name.is_empty()) {
        entry.name = name.clone();
    }
    if let Some(code) = friend.persona_state {
        entry.presence = SteamPresence::from_code(code);
    }
    if let Some(game) = friend.game_name.as_ref().filter(|game| !game.is_empty()) {
        entry.game = game.clone();
    }
    if let Some(appid) = friend.game_played_app_id.filter(|appid| *appid != 0) {
        entry.game_appid = appid;
        // Un jeu joué vaut présence « en jeu » : Steam publie parfois un état
        // en ligne sans le drapeau de riche présence.
        if entry.presence == SteamPresence::Online {
            entry.presence = SteamPresence::Play;
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
        tracing::debug!(
            "liste d'amis refusée ({status}, {} octets)",
            body.len()
        );
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
    tracing::debug!("liste d'amis HTTP : {total} relations, {} amis", friends.len());
    for (steam_id, relationship) in friends {
        if relationship == 3 && is_person(steam_id) {
            live.friends
                .entry(steam_id)
                .or_insert(SteamFriend { steam_id, ..SteamFriend::default() });
        } else {
            live.friends.remove(&steam_id);
        }
    }
    true
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
        member.games = counts
            .get(&member.steam_id)
            .map(|set| set.len() as u32)
            .unwrap_or(0);
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
    use steamroom::generated::c_msg_client_friends_list::Friend as ListFriend;

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
        assert_eq!(fields.next(), Some((4, proto::Field::Varint(0))));
        assert_eq!(fields.next(), None);
    }

    #[test]
    fn incoming_friend_message_is_decoded() {
        let mut body = Vec::new();
        proto::field_fixed64(1, 222, &mut body);
        proto::field_varint(2, 1, &mut body);
        proto::field_bytes(3, b"bonjour", &mut body);
        proto::field_fixed32(5, 1_700_000_000, &mut body);
        assert_eq!(
            parse_incoming_friend_msg(&body),
            Some((222, "bonjour".into(), 1_700_000_000))
        );
        // Les messages d'un autre type (émoticône, invitation) sont ignorés.
        let mut other = Vec::new();
        proto::field_fixed64(1, 222, &mut other);
        proto::field_varint(2, 2, &mut other);
        proto::field_bytes(3, b"x", &mut other);
        assert_eq!(parse_incoming_friend_msg(&other), None);
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
        assert_eq!(family.members[1].games, 1);
        assert_eq!(family.members[2].games, 1);
        assert_eq!(family.members[3].games, 0);
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
        assert_eq!(url, "https://avatars.fastly.steamstatic.com/0123ab_full.jpg");
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
            friends: HashMap::new(),
            notifications: Vec::new(),
            new_comments: 0,
            family: None,
            presence: SteamPresence::Online,
            applied: SteamPresence::Online,
            redeem: None,
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
            friends: HashMap::new(),
            notifications: Vec::new(),
            new_comments: 0,
            family: None,
            presence: SteamPresence::Online,
            applied: SteamPresence::Online,
            redeem: None,
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
        apply_persona(
            &mut entry,
            &friend(1, Some("Portal 2"), Some(620)),
        );
        assert_eq!(entry.name, "Alice");
        assert_eq!(entry.game, "Portal 2");
        assert_eq!(entry.game_appid, 620);
        assert_eq!(entry.presence, SteamPresence::Play);
        assert!(entry.avatar.ends_with("_full.jpg"));
    }

    #[test]
    fn our_own_persona_confirms_the_applied_state() {
        let mut live = Live {
            persona_name: "moi".into(),
            steam_id: 7,
            friends: HashMap::new(),
            notifications: Vec::new(),
            new_comments: 0,
            family: None,
            presence: SteamPresence::Invisible,
            applied: SteamPresence::Online,
            redeem: None,
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
                CMsgProtoBufHeader { steamid: Some(7), ..Default::default() },
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
            {"ulfriendid":"76561198008748126","efriendrelationship":3},
            {"ulfriendid":"76561197980667544","efriendrelationship":5}],
            "max_friend_count":2}}}"#;
        let (friends, total) = parse_friend_list(body).unwrap();
        assert_eq!(total, 2);
        assert_eq!(
            friends,
            vec![(76_561_198_008_748_126, 3), (76_561_197_980_667_544, 5)]
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
            friends: HashMap::new(),
            notifications: Vec::new(),
            new_comments: 0,
            family: Some(SteamFamily {
                name: "Maison".into(),
                members: vec![
                    SteamFamilyMember { steam_id: 111, role: 1, ..Default::default() },
                    SteamFamilyMember { steam_id: 222, role: 2, ..Default::default() },
                ],
                ..SteamFamily::default()
            }),
            presence: SteamPresence::Online,
            applied: SteamPresence::Online,
            redeem: None,
        };
        live.friends.insert(
            111,
            SteamFriend {
                steam_id: 111,
                name: "Nathan".into(),
                avatar: "https://avatars.example/nathan_full.jpg".into(),
                ..SteamFriend::default()
            },
        );
        let chats = std::sync::Arc::new(std::sync::Mutex::new(std::collections::BTreeMap::new()));
        let social = live.snapshot(&chats);
        let family = social.family.expect("famille publiée");
        assert_eq!(family.members[0].name, "Nathan");
        assert_eq!(
            family.members[0].avatar,
            "https://avatars.example/nathan_full.jpg"
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
}
