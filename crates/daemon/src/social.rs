//! Partie sociale Steam du daemon : une session permanente qui publie la
//! présence, apporte les amis, la famille et les notifications, et entretient
//! le jeton de refresh.
//!
//! L'interface ne parle jamais directement au réseau Steam : elle demande
//! [`Req::SteamSocial`](lapinux_core::ipc::Req::SteamSocial) et change l'état
//! publié par `SteamSocialPresence`. La session vit tant que le daemon tourne,
//! comme le client de Valve : fermer la fenêtre ne fait pas disparaître le
//! compte de la liste d'amis.

use crate::{Result, Shared};
use lapinux_core::ipc::Envelope;
use lapinux_core::social::{
    SteamChatMessage, SteamFriend, SteamFriendEvent, SteamFriendsState, SteamLobbyInvite,
    SteamPresence, SteamSocial,
};

use lapinux_steam::session;
use lapinux_steam::social::Social;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Avatars téléchargés simultanément au maximum : une grande liste se remplit
/// en quelques secondes sans lancer cent requêtes d'un coup.
const AVATAR_CONCURRENCY: usize = 12;
static AVATAR_ACTIVE: AtomicUsize = AtomicUsize::new(0);
/// Un avatar Steam fait quelques kilo-octets ; cette borne protège le cache
/// d'une réponse inattendue.
const AVATAR_MAX_BYTES: u64 = 2 * 1024 * 1024;

/// Noms des jeux absents du catalogue, résolus par la boutique en tâche de
/// fond. La réponse sociale ne doit jamais attendre le réseau : le nom arrive
/// au snapshot suivant, et l'interface le complète aussi à l'affichage.
///
/// Un échec n'est pas définitif : la boutique peut être momentanément
/// injoignable au démarrage. Un nom absent est donc retenté après un délai,
/// au lieu de laisser « En jeu » sans titre pour toute la session.
struct CachedAppName {
    name: Option<String>,
    at: Instant,
}

const APP_NAME_RETRY: Duration = Duration::from_secs(60);

fn app_name_cache() -> &'static Mutex<HashMap<u32, CachedAppName>> {
    static CACHE: OnceLock<Mutex<HashMap<u32, CachedAppName>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn app_name_requests() -> &'static Mutex<HashSet<u32>> {
    static REQUESTS: OnceLock<Mutex<HashSet<u32>>> = OnceLock::new();
    REQUESTS.get_or_init(|| Mutex::new(HashSet::new()))
}

fn cached_store_name(appid: u32) -> Option<String> {
    if appid == 0 {
        return None;
    }
    if let Ok(names) = app_name_cache().lock() {
        if let Some(entry) = names.get(&appid) {
            if entry.name.is_some() || entry.at.elapsed() < APP_NAME_RETRY {
                return entry.name.clone();
            }
        }
    }
    let mut start = false;
    if let Ok(mut pending) = app_name_requests().lock() {
        if pending.insert(appid) {
            start = true;
        }
    }
    if start {
        std::thread::spawn(move || {
            let name = lapinux_steam::catalog::Catalog::store_name(appid);
            if let Ok(mut names) = app_name_cache().lock() {
                names.insert(
                    appid,
                    CachedAppName {
                        name,
                        at: Instant::now(),
                    },
                );
            }
            if let Ok(mut pending) = app_name_requests().lock() {
                pending.remove(&appid);
            }
        });
    }
    None
}

/// Archive locale des messages déjà vus : Steam supprime son historique après
/// une fenêtre récente, l'archive garde au moins ce que Lapinux a chargé.
const CHAT_ARCHIVE_FILE: &str = "steam-chats.json";
const CHAT_ARCHIVE_MAX: usize = 5000;

pub(crate) type ChatArchive = BTreeMap<u64, Vec<SteamChatMessage>>;

fn chat_archive_path(paths: &lapinux_core::paths::LapinuxPaths) -> PathBuf {
    paths.data_dir.join(CHAT_ARCHIVE_FILE)
}

pub(crate) fn load_chat_archive(paths: &lapinux_core::paths::LapinuxPaths) -> ChatArchive {
    let Ok(text) = std::fs::read_to_string(chat_archive_path(paths)) else {
        return ChatArchive::new();
    };
    let Ok(entries) = serde_json::from_str::<Vec<(u64, Vec<SteamChatMessage>)>>(&text) else {
        return ChatArchive::new();
    };
    entries.into_iter().collect()
}

fn save_chat_archive(paths: &lapinux_core::paths::LapinuxPaths, archive: &ChatArchive) {
    let entries: Vec<(u64, Vec<SteamChatMessage>)> = archive
        .iter()
        .map(|(friend, messages)| (*friend, messages.clone()))
        .collect();
    let Ok(text) = serde_json::to_string(&entries) else {
        return;
    };
    let path = chat_archive_path(paths);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let temporary = path.with_extension("json.tmp");
    if std::fs::write(&temporary, text).is_ok() {
        let _ = std::fs::rename(&temporary, &path);
    }
}

fn merge_chat_messages(target: &mut Vec<SteamChatMessage>, incoming: &[SteamChatMessage]) -> bool {
    let mut changed = false;
    for message in incoming {
        if message.text.trim().is_empty() {
            continue;
        }
        if target.iter().any(|known| {
            known.from == message.from
                && known.outgoing == message.outgoing
                && known.text == message.text
                && known.at == message.at
        }) {
            continue;
        }
        target.push(message.clone());
        changed = true;
    }
    if changed {
        target.sort_by_key(|message| message.at);
        if target.len() > CHAT_ARCHIVE_MAX {
            target.drain(0..target.len() - CHAT_ARCHIVE_MAX);
        }
    }
    changed
}

/// Message d'un export JSON, avec les noms de champs des outils courants.
#[derive(serde::Deserialize)]
struct ImportedChatMessage {
    #[serde(default, alias = "timestamp", alias = "time")]
    at: i64,
    #[serde(default, alias = "sender_id", alias = "steam_id")]
    from: u64,
    #[serde(default, alias = "message", alias = "content")]
    text: String,
    #[serde(default)]
    outgoing: bool,
}

fn convert_imported(
    messages: Vec<ImportedChatMessage>,
    friend_id: u64,
    self_id: u64,
) -> Vec<SteamChatMessage> {
    messages
        .into_iter()
        .filter_map(|message| {
            let text = message.text.trim();
            if text.is_empty() {
                return None;
            }
            let outgoing = message.outgoing || (self_id != 0 && message.from == self_id);
            let from = if message.from != 0 {
                message.from
            } else if outgoing && self_id != 0 {
                self_id
            } else {
                friend_id
            };
            (from != 0).then(|| SteamChatMessage {
                from,
                text: text.to_string(),
                at: message.at.max(0),
                outgoing,
                received_live: false,
            })
        })
        .collect()
}

/// Lit un export JSON : liste plate pour la conversation ouverte, objet
/// `{\"messages\": [...]}`, ou archive Lapinux multi-conversations.
fn parse_chat_export(
    text: &str,
    friend_id: u64,
    self_id: u64,
) -> std::result::Result<Vec<(u64, Vec<SteamChatMessage>)>, String> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|error| format!("JSON illisible : {error}"))?;
    match value {
        serde_json::Value::Object(mut object) => {
            let messages = object
                .remove("messages")
                .ok_or_else(|| "le fichier ne contient pas de liste `messages`".to_string())?;
            let messages: Vec<ImportedChatMessage> = serde_json::from_value(messages)
                .map_err(|error| format!("messages illisibles : {error}"))?;
            let messages = convert_imported(messages, friend_id, self_id);
            if messages.is_empty() {
                return Err("aucun message reconnu dans ce fichier".into());
            }
            Ok(vec![(friend_id, messages)])
        }
        serde_json::Value::Array(items) => {
            let is_archive = items.first().is_some_and(|item| {
                item.as_array()
                    .is_some_and(|pair| pair.len() == 2 && pair[0].is_u64() && pair[1].is_array())
            });
            if is_archive {
                let archive: Vec<(u64, Vec<ImportedChatMessage>)> =
                    serde_json::from_value(serde_json::Value::Array(items))
                        .map_err(|error| format!("archive illisible : {error}"))?;
                let mut chats = Vec::new();
                for (id, messages) in archive {
                    if id == 0 {
                        continue;
                    }
                    let messages = convert_imported(messages, id, self_id);
                    if !messages.is_empty() {
                        chats.push((id, messages));
                    }
                }
                if chats.is_empty() {
                    return Err("aucun message reconnu dans cette archive".into());
                }
                Ok(chats)
            } else {
                let messages: Vec<ImportedChatMessage> =
                    serde_json::from_value(serde_json::Value::Array(items))
                        .map_err(|error| format!("messages illisibles : {error}"))?;
                let messages = convert_imported(messages, friend_id, self_id);
                if messages.is_empty() {
                    return Err("aucun message reconnu dans ce fichier".into());
                }
                Ok(vec![(friend_id, messages)])
            }
        }
        _ => Err("le fichier doit contenir une liste de messages".into()),
    }
}

#[cfg(test)]
mod chat_import_tests {
    use super::*;

    #[test]
    fn un_export_json_plat_remplit_la_conversation() {
        let text = r#"[
            {"from": 1, "text": "bonjour", "timestamp": 100, "outgoing": false},
            {"from": 2, "message": "salut", "time": 200, "outgoing": true}
        ]"#;
        let chats = parse_chat_export(text, 1, 2).unwrap();
        assert_eq!(chats.len(), 1);
        assert_eq!(chats[0].0, 1);
        assert_eq!(chats[0].1.len(), 2);
        assert_eq!(chats[0].1[1].text, "salut");
        assert!(chats[0].1[1].outgoing);
    }

    #[test]
    fn une_archive_lapinux_reste_multi_conversations() {
        let text = r#"[
            [1, [{"from": 1, "text": "a", "at": 1, "outgoing": false}]],
            [2, [{"from": 2, "text": "b", "at": 2, "outgoing": true}]]
        ]"#;
        let chats = parse_chat_export(text, 1, 2).unwrap();
        assert_eq!(chats.len(), 2);
        assert_eq!(chats[0].1[0].text, "a");
        assert_eq!(chats[1].1[0].text, "b");
    }
}

/// Adresse `ip:port` du serveur d'écoute, par lobby créé par le daemon.
///
/// Steam ne relaie pas cette valeur : la métadonnée de lobby
/// (`CMsgClientMMSCreateLobby`, champ 7) n'est pas encodée ici faute de forme
/// protobuf vérifiée. Le registre fournit l'adresse que `SteamLobbyJoin`
/// transmet au lancement, pour un lobby créé par ce daemon ; une rejointe
/// entre deux machines dépend encore de l'accessibilité du port d'écoute.
static LOBBY_ADDRESSES: OnceLock<Mutex<BTreeMap<u64, String>>> = OnceLock::new();

fn lobby_addresses() -> &'static Mutex<BTreeMap<u64, String>> {
    LOBBY_ADDRESSES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// Retient l'adresse de la partie pour un lobby créé par le daemon.
pub(crate) fn remember_lobby_address(lobby: u64, connect: &str) {
    let connect = connect.trim();
    if lobby == 0 || connect.is_empty() {
        return;
    }
    if let Ok(mut map) = lobby_addresses().lock() {
        map.insert(lobby, connect.to_string());
    }
}

/// Adresse de la partie enregistrée pour ce lobby, si elle est connue.
pub(crate) fn lobby_address(lobby: u64) -> Option<String> {
    // Les métadonnées du lobby viennent du CM : elles valent aussi pour un
    // invité sur une autre machine, et priment sur le registre local.
    lapinux_steam::social::lobby_connect(lobby).or_else(|| {
        lobby_addresses()
            .lock()
            .ok()
            .and_then(|map| map.get(&lobby).cloned())
    })
}

/// Adresse retenue pour une rejointe : celle que le joueur ou l'interface
/// connaissait déjà, sinon celle que les métadonnées du lobby viennent de
/// rendre. Une valeur sans port ne passe pas.
pub(crate) fn join_address(known: &str, published: Option<&str>) -> Option<String> {
    let candidate = if known.contains(':') {
        known
    } else {
        published.unwrap_or_default()
    };
    let candidate = candidate.trim();
    (!candidate.is_empty() && candidate.contains(':')).then(|| candidate.to_string())
}

/// Adresse `ip:port` du serveur d'écoute du jeu hôte : `LAPINUX_LOBBY_CONNECT`
/// si le joueur la fixe, sinon l'IP de la machine sur le réseau local et le
/// port d'écoute du jeu (`LAPINUX_LOBBY_PORT`, 7777 par défaut, celui d'Unreal).
pub(crate) fn listen_address() -> Option<String> {
    if let Ok(value) = std::env::var("LAPINUX_LOBBY_CONNECT") {
        let value = value.trim();
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    let port = std::env::var("LAPINUX_LOBBY_PORT")
        .ok()
        .and_then(|port| port.trim().parse::<u16>().ok())
        .filter(|port| *port != 0)
        .unwrap_or(7777);
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    // Un `connect` UDP n'envoie rien : il ne fait que choisir la route, donc
    // l'adresse de l'interface qui porte le trafic par défaut.
    socket.connect("1.1.1.1:80").ok()?;
    let ip = socket.local_addr().ok()?.ip();
    if ip.is_loopback() || ip.is_unspecified() {
        return None;
    }
    Some(format!("{ip}:{port}"))
}

/// Lobby à rejoindre au prochain lancement du jeu visé.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingJoin {
    pub appid: u32,
    pub lobby: u64,
    pub connect: String,
}

static PENDING_JOIN: OnceLock<Mutex<Option<PendingJoin>>> = OnceLock::new();

fn pending_join() -> &'static Mutex<Option<PendingJoin>> {
    PENDING_JOIN.get_or_init(|| Mutex::new(None))
}

/// Pose la rejointe que le prochain lancement du jeu visé consommera.
pub(crate) fn queue_join(join: PendingJoin) {
    if let Ok(mut slot) = pending_join().lock() {
        *slot = Some(join);
    }
}

/// Retire la rejointe en attente, et seulement pour le jeu visé : un lancement
/// d'un autre jeu ne la consomme pas et ne l'emporte pas.
pub(crate) fn take_pending_join(appid: u32) -> Option<PendingJoin> {
    let mut slot = pending_join().lock().ok()?;
    match slot.as_ref() {
        Some(join) if join.appid == appid => slot.take(),
        _ => None,
    }
}

/// Abandonne la rejointe quand le lancement a échoué : elle ne doit pas
/// s'appliquer à un essai ultérieur.
pub(crate) fn forget_join(appid: u32) {
    if let Ok(mut slot) = pending_join().lock() {
        if slot.as_ref().map(|join| join.appid) == Some(appid) {
            *slot = None;
        }
    }
}

/// Variable et argument ajoutés au lancement d'une rejointe, à côté des
/// réglages du jeu : le pont Windows lit la première, le jeu lit le second.
pub(crate) fn join_launch_additions(join: &PendingJoin) -> (Vec<(String, String)>, Vec<String>) {
    (
        vec![("LAPINUX_JOIN_CONNECT".to_string(), join.connect.clone())],
        vec!["+connect_lobby".to_string(), join.lobby.to_string()],
    )
}

/// Invitations reçues, avec le nom de l'ami et l'adresse de la partie quand
/// elles sont connues.
fn lobby_invites(friends: &[SteamFriend]) -> Vec<SteamLobbyInvite> {
    lapinux_steam::social::received_invites()
        .into_iter()
        .map(|invite| SteamLobbyInvite {
            appid: invite.app_id,
            lobby: invite.lobby,
            inviter: invite.inviter,
            inviter_name: friends
                .iter()
                .find(|friend| friend.steam_id == invite.inviter)
                .map(|friend| friend.name.clone()),
            connect: lobby_address(invite.lobby),
        })
        .collect()
}

fn avatar_attempts() -> &'static Mutex<HashSet<u64>> {
    static ATTEMPTS: OnceLock<Mutex<HashSet<u64>>> = OnceLock::new();
    ATTEMPTS.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Seules les images servies par Steam sont mises en cache.
fn is_steam_avatar_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let host = rest.split('/').next().unwrap_or_default();
    host == "steamstatic.com" || host.ends_with(".steamstatic.com")
}

fn download_avatar(url: &str, path: &Path) {
    let downloaded = (|| -> Option<()> {
        let response = ureq::get(url)
            .set("User-Agent", "Lapinux/0.1 (cache d'avatars Steam)")
            .timeout(Duration::from_secs(8))
            .call()
            .ok()?;
        let mut bytes = Vec::new();
        response
            .into_reader()
            .take(AVATAR_MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        if bytes.is_empty() || bytes.len() as u64 > AVATAR_MAX_BYTES {
            return None;
        }
        let temporary = path.with_extension("part");
        std::fs::write(&temporary, &bytes).ok()?;
        std::fs::rename(&temporary, path).ok()?;
        Some(())
    })();
    if downloaded.is_none() {
        let _ = std::fs::remove_file(path.with_extension("part"));
    }
}

impl Shared {
    /// Fusionne les messages de la session dans l'archive locale, puis remet
    /// l'archive dans l'instantané publié : l'interface voit aussi les messages
    /// que Steam ne rend plus.
    pub(crate) fn sync_chat_archive(&self, chats: &mut BTreeMap<u64, Vec<SteamChatMessage>>) {
        let Ok(mut archive) = self.chat_archive.lock() else {
            return;
        };
        let mut changed = false;
        for (friend, messages) in chats.iter() {
            changed |= merge_chat_messages(archive.entry(*friend).or_default(), messages);
        }
        for (friend, messages) in archive.iter() {
            merge_chat_messages(chats.entry(*friend).or_default(), messages);
        }
        if changed {
            save_chat_archive(&self.paths, &archive);
        }
    }

    /// Chemins des avatars déjà en cache, et téléchargement en tâche de fond
    /// des suivants. Une seule tentative par ami et par vie du daemon évite de
    /// marteler Steam si une image manque.
    fn refresh_friend_avatars(&self, friends: &[SteamFriend]) -> BTreeMap<u64, PathBuf> {
        let directory = self.paths.cache_dir.join("steam-avatars");
        if std::fs::create_dir_all(&directory).is_err() {
            return BTreeMap::new();
        }
        let mut queued = Vec::new();
        let slots = AVATAR_CONCURRENCY.saturating_sub(AVATAR_ACTIVE.load(Ordering::Relaxed));
        if slots > 0 {
            if let Ok(mut attempts) = avatar_attempts().lock() {
                for friend in friends {
                    if !is_steam_avatar_url(&friend.avatar) {
                        continue;
                    }
                    let path = directory.join(format!("{}.jpg", friend.steam_id));
                    if path.is_file() || !attempts.insert(friend.steam_id) {
                        continue;
                    }
                    queued.push((friend.avatar.clone(), path));
                    if queued.len() >= slots {
                        break;
                    }
                }
            }
        }
        if !queued.is_empty() {
            AVATAR_ACTIVE.fetch_add(queued.len(), Ordering::Relaxed);
        }
        for (url, path) in queued {
            std::thread::spawn(move || {
                download_avatar(&url, &path);
                AVATAR_ACTIVE.fetch_sub(1, Ordering::Relaxed);
            });
        }
        friends
            .iter()
            .filter_map(|friend| {
                let path = directory.join(format!("{}.jpg", friend.steam_id));
                path.is_file().then_some((friend.steam_id, path))
            })
            .collect()
    }

    /// Démarre la session sociale si un jeton est enregistré et si la
    /// configuration ne la désactive pas. Idempotent.
    pub fn ensure_social(self: &Arc<Self>) -> Result<Arc<Social>> {
        if let Ok(slot) = self.social.lock() {
            if let Some(social) = slot.as_ref() {
                return Ok(Arc::clone(social));
            }
        }
        if self.social_paused.load(Ordering::SeqCst) {
            return Err(lapinux_core::Error::Other(
                "session sociale en pause : Steam ou synchronisation du catalogue en cours".into(),
            ));
        }
        let config = self.config_snapshot();
        if !config.steam.social {
            return Err(lapinux_core::Error::Other(
                "session sociale désactivée dans les réglages".into(),
            ));
        }
        let saved = session::load_from(&self.session_path())
            .filter(|saved| !saved.refresh_token.trim().is_empty())
            .ok_or_else(|| {
                lapinux_core::Error::Other(
                    "connectez d'abord votre compte Steam pour apparaître en ligne".into(),
                )
            })?;
        // Une session qui naît pendant une partie doit reprendre l'état
        // « en jeu » : sinon l'activité disparaît à chaque reconnexion.
        let playing = if config.steam.show_playing {
            self.steam_playing.lock().ok().and_then(|slot| *slot)
        } else {
            None
        };
        let playing_name = self.session_info().map(|info| info.name);
        let social = Arc::new(Social::start_with_playing(
            saved,
            config.steam.presence,
            playing,
            playing_name,
        )?);
        if let Ok(mut slot) = self.social.lock() {
            *slot = Some(Arc::clone(&social));
        }
        Ok(social)
    }

    /// État social : l'interface y lit les amis, la famille et les
    /// notifications. `refresh` redemande la liste d'amis.
    pub fn steam_social(self: &Arc<Self>, refresh: bool) -> Result<Envelope> {
        if self.social_paused.load(Ordering::SeqCst) {
            // La session sociale reprend dès la fin du catalogue : l'interface
            // voit l'état « connexion », comme pendant une reconnexion.
            return Ok(Envelope::ok(&SteamSocial::connecting()));
        }
        let social = match self.ensure_social() {
            Ok(social) => social,
            Err(error) => {
                return Ok(Envelope::ok(&SteamSocial::failed(error.to_string())));
            }
        };
        if refresh {
            social.refresh();
        }
        let mut snapshot = social.snapshot();
        self.sync_chat_archive(&mut snapshot.chats);
        self.name_friends_games(&mut snapshot.friends);
        self.name_friend_events(&mut snapshot.friend_events);
        snapshot.invites = lobby_invites(&snapshot.friends);
        Ok(Envelope::ok(&snapshot))
    }

    /// Liste d'amis, avatars et discussions de l'overlay. Elle ne dépend ni
    /// d'un jeu lancé ni de ses catégories : discuter avec un ami reste
    /// possible pendant n'importe quelle partie.
    pub fn steam_friends_state(self: &Arc<Self>) -> Result<Envelope> {
        let social = match self.ensure_social() {
            Ok(social) => social,
            Err(error) => {
                let state = SteamFriendsState {
                    error: Some(error.to_string()),
                    ..SteamFriendsState::default()
                };
                return Ok(Envelope::ok(&state));
            }
        };
        let mut snapshot = social.snapshot();
        self.sync_chat_archive(&mut snapshot.chats);
        let live = snapshot.live();
        let self_id = snapshot.steam_id;
        let mut friends: Vec<SteamFriend> = snapshot
            .friends
            .into_iter()
            .filter(|friend| friend.steam_id != 0 && friend.steam_id != self_id)
            .collect();
        friends.sort_by(|a, b| {
            b.in_game()
                .cmp(&a.in_game())
                .then_with(|| b.presence.online().cmp(&a.presence.online()))
                .then_with(|| {
                    a.name
                        .to_lowercase()
                        .cmp(&b.name.to_lowercase())
                        .then(a.steam_id.cmp(&b.steam_id))
                })
        });
        let avatars = self.refresh_friend_avatars(&friends);
        // Le lobby n'est proposé que s'il appartient au jeu lancé : un ancien
        // lobby d'une autre partie ne doit pas faire apparaître un bouton qui
        // enverrait l'ami dans le mauvais salon.
        let session_appid = self.session_info().map(|info| info.appid);
        let lobby = lapinux_steam::social::last_lobby_info()
            .filter(|(appid, _)| Some(*appid) == session_appid)
            .map(|(_, lobby)| lobby);
        let state = SteamFriendsState {
            live,
            error: snapshot.error,
            friends,
            avatars,
            chats: snapshot.chats,
            lobby,
        };
        Ok(Envelope::ok(&state))
    }

    /// Active une clé produit sur le compte connecté.
    ///
    /// La tentative entre dans l'historique tout de suite : le verdict de
    /// Steam arrive dans `redeem`, que l'interface suit à l'image suivante.
    pub fn steam_social_redeem(self: &Arc<Self>, key: &str) -> Result<Envelope> {
        let key = key.trim();
        if key.is_empty() {
            return Err(lapinux_core::Error::Other(
                "saisissez une clé produit Steam".into(),
            ));
        }
        // Une clé arrive souvent juste après la précédente : la relève de
        // celle-là met la session sociale en pause, et la session suivante met
        // une seconde à se connecter. Envoyée dans cet intervalle, la demande
        // tombait sur une session fermée et la seconde activation ne faisait
        // rien. La pause se lève d'elle-même : on lui laisse un demi-seconde,
        // puis on la lève nous-mêmes.
        let started = std::time::Instant::now();
        let deadline = started + Duration::from_secs(2);
        let mut resumed = started;
        loop {
            if self.social_paused.load(Ordering::SeqCst)
                && started.elapsed() > Duration::from_millis(600)
                && resumed.elapsed() > Duration::from_millis(600)
            {
                self.resume_social();
                resumed = std::time::Instant::now();
            }
            // La clé part dans la file du fil social : il l'envoie tout de
            // suite s'il est connecté, sinon il la garde pour la connexion
            // suivante, une fois la session en place.
            let social = self.ensure_social();
            if let Ok(social) = &social {
                if social.redeem(key) {
                    tracing::info!("activation : clé remise au fil social");
                    return Ok(Envelope::ok(&social.snapshot()));
                }
            }
            if std::time::Instant::now() >= deadline {
                return match &social {
                    Ok(social) => {
                        tracing::warn!(
                            "activation : fil social fermé ({})",
                            social.snapshot().state
                        );
                        Ok(Envelope::ok(&SteamSocial::failed(
                            "la session Steam se remet en place : relancez l'activation dans un instant"
                                .to_string(),
                        )))
                    }
                    Err(error) => {
                        tracing::warn!("activation : {error}");
                        Ok(Envelope::ok(&SteamSocial::failed(error.to_string())))
                    }
                };
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// Invite un ami dans le lobby Steam de la partie en cours.
    ///
    /// Le jeu ne crée pas toujours son lobby (certains passent par un autre
    /// réseau, ou n'ouvrent le leur qu'une fois en partie) : quand aucun lobby
    /// n'est connu, le daemon en crée un pour l'appid lancé, puis y invite
    /// l'ami. Sans partie en cours, il n'y a rien à rejoindre et le refus
    /// reste explicite.
    pub fn steam_lobby_invite(self: &Arc<Self>, steam_id: u64) -> Result<Envelope> {
        if steam_id == 0 {
            return Err(lapinux_core::Error::Other("SteamID d'ami invalide".into()));
        }
        let playing = self.steam_playing.lock().ok().and_then(|slot| *slot);
        let social = self.ensure_social().map_err(|error| {
            lapinux_core::Error::Other(format!("session sociale indisponible : {error}"))
        })?;
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        loop {
            if social.snapshot().live() {
                let (appid, lobby) =
                    match lapinux_steam::social::last_lobby_info() {
                        Some(info) => {
                            if let Some(address) = listen_address() {
                                remember_lobby_address(info.1, &address);
                                lapinux_steam::social::set_lobby_data(
                                    &social, info.0, info.1, &address,
                                );
                            }
                            info
                        }
                        None => {
                            let Some(appid) = playing else {
                                return Err(lapinux_core::Error::Other(
                                    "aucun lobby Steam actif : lancez le jeu puis invitez un ami"
                                        .into(),
                                ));
                            };
                            let connect = listen_address();
                            let Some(answer) = lapinux_steam::social::create_lobby(
                                &social,
                                appid,
                                4,
                                None,
                                None,
                                None,
                                connect.as_deref(),
                            ) else {
                                return Err(lapinux_core::Error::Other(
                                    "la session sociale n'accepte plus de commandes".into(),
                                ));
                            };
                            match answer.recv_timeout(Duration::from_secs(12)) {
                                Ok(Ok(lobby)) => {
                                    if let Some(address) = connect.as_deref() {
                                        remember_lobby_address(lobby, address);
                                        lapinux_steam::social::set_lobby_data(
                                            &social, appid, lobby, address,
                                        );
                                    }
                                    tracing::info!(
                                    "Steam : lobby {lobby} créé pour l'invitation (appid {appid})"
                                );
                                    (appid, lobby)
                                }
                                Ok(Err(eresult)) => {
                                    return Err(lapinux_core::Error::Other(format!(
                                        "Steam a refusé la création du lobby (eresult {eresult})"
                                    )))
                                }
                                Err(_) => return Err(lapinux_core::Error::Other(
                                    "Steam n'a pas répondu à la création du lobby (délai dépassé)"
                                        .into(),
                                )),
                            }
                        }
                    };
                let answer =
                    lapinux_steam::social::invite_to_lobby(&social, steam_id).ok_or_else(|| {
                        lapinux_core::Error::Other(
                            "la session sociale n'accepte plus de commandes".into(),
                        )
                    })?;
                return match answer.recv_timeout(Duration::from_secs(5)) {
                    Ok(Ok(())) => {
                        tracing::info!(
                            "Steam : ami {steam_id} invité dans le lobby {lobby} (appid {appid})"
                        );
                        Ok(Envelope::ok(&serde_json::json!({
                            "steam_id": steam_id,
                            "lobby": lobby.to_string(),
                        })))
                    }
                    Ok(Err(eresult)) => Err(lapinux_core::Error::Other(format!(
                        "Steam a refusé l'invitation (eresult {eresult})"
                    ))),
                    Err(_) => Err(lapinux_core::Error::Other(
                        "Steam n'a pas répondu à l'invitation (délai dépassé)".into(),
                    )),
                };
            }
            if std::time::Instant::now() >= deadline {
                return Err(lapinux_core::Error::Other(
                    "la session sociale n'est pas connectée : réessayez dans un instant".into(),
                ));
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// Envoie un message texte à un ami par la session sociale.
    pub fn steam_social_message(self: &Arc<Self>, steam_id: u64, text: &str) -> Result<Envelope> {
        if steam_id == 0 {
            return Err(lapinux_core::Error::Other(
                "SteamID du destinataire manquant".into(),
            ));
        }
        let text = text.trim();
        if text.is_empty() {
            return Err(lapinux_core::Error::Other("message vide".into()));
        }
        if text.len() > 1024 {
            return Err(lapinux_core::Error::Other("message trop long".into()));
        }
        let social = self.ensure_social().map_err(|error| {
            lapinux_core::Error::Other(format!("session sociale indisponible : {error}"))
        })?;
        let snapshot = social.snapshot();
        if !snapshot.live() {
            return Err(lapinux_core::Error::Other(
                "session sociale pas encore connectée ; réessayez dans un instant".into(),
            ));
        }
        if !snapshot
            .friends
            .iter()
            .any(|friend| friend.steam_id == steam_id)
        {
            return Err(lapinux_core::Error::Other(
                "ce compte n'est pas dans votre liste d'amis Steam".into(),
            ));
        }
        if !social.message(steam_id, text) {
            return Err(lapinux_core::Error::Other(
                "la session sociale n'accepte plus de commandes".into(),
            ));
        }
        Ok(Envelope::ok(&serde_json::json!({
            "steam_id": steam_id,
            "transmitted": true,
        })))
    }

    /// Envoie un autocollant Steam à un ami par la session sociale.
    pub fn steam_social_sticker(self: &Arc<Self>, steam_id: u64, name: &str) -> Result<Envelope> {
        if steam_id == 0 {
            return Err(lapinux_core::Error::Other(
                "SteamID du destinataire manquant".into(),
            ));
        }
        let name = name.trim();
        if name.is_empty() {
            return Err(lapinux_core::Error::Other(
                "nom d'autocollant manquant".into(),
            ));
        }
        if name.len() > 100
            || name.chars().any(|character| {
                character.is_control() || matches!(character, '[' | ']' | '"' | '\\')
            })
        {
            return Err(lapinux_core::Error::Other(
                "nom d'autocollant invalide".into(),
            ));
        }
        let social = self.ensure_social().map_err(|error| {
            lapinux_core::Error::Other(format!("session sociale indisponible : {error}"))
        })?;
        let snapshot = social.snapshot();
        if !snapshot.live() {
            return Err(lapinux_core::Error::Other(
                "session sociale pas encore connectée ; réessayez dans un instant".into(),
            ));
        }
        if !snapshot
            .friends
            .iter()
            .any(|friend| friend.steam_id == steam_id)
        {
            return Err(lapinux_core::Error::Other(
                "ce compte n'est pas dans votre liste d'amis Steam".into(),
            ));
        }
        if !social.sticker(steam_id, name) {
            return Err(lapinux_core::Error::Other(
                "la session sociale n'accepte plus de commandes".into(),
            ));
        }
        Ok(Envelope::ok(&serde_json::json!({
            "steam_id": steam_id,
            "sticker": name,
            "transmitted": true,
        })))
    }

    /// Demande l'historique récent d'une conversation Steam. La réponse du
    /// service rejoint l'instantané social, que l'interface relit ensuite.
    pub fn steam_chat_history(self: &Arc<Self>, steam_id: u64, older: bool) -> Result<Envelope> {
        if steam_id == 0 {
            return Err(lapinux_core::Error::Other(
                "SteamID de l'ami manquant".into(),
            ));
        }
        let social = self.ensure_social().map_err(|error| {
            lapinux_core::Error::Other(format!("session sociale indisponible : {error}"))
        })?;
        let snapshot = social.snapshot();
        if !snapshot.live() {
            return Err(lapinux_core::Error::Other(
                "session sociale pas encore connectée ; réessayez dans un instant".into(),
            ));
        }
        if !snapshot
            .friends
            .iter()
            .any(|friend| friend.steam_id == steam_id)
        {
            return Err(lapinux_core::Error::Other(
                "ce compte n'est pas dans votre liste d'amis Steam".into(),
            ));
        }
        if !social.history(steam_id, older) {
            return Err(lapinux_core::Error::Other(
                "la session sociale n'accepte plus de commandes".into(),
            ));
        }
        Ok(Envelope::unit())
    }

    /// Importe un export JSON dans l'archive locale. L'import ne dépend pas
    /// d'une session Steam live : il complète aussi une conversation dont
    /// Steam ne renvoie plus les anciens messages.
    pub fn steam_chat_import(self: &Arc<Self>, steam_id: u64, path: &str) -> Result<Envelope> {
        if steam_id == 0 {
            return Err(lapinux_core::Error::Other(
                "SteamID de l'ami manquant".into(),
            ));
        }
        let path = PathBuf::from(path.trim());
        if path.as_os_str().is_empty() {
            return Err(lapinux_core::Error::Other(
                "chemin du fichier d'import manquant".into(),
            ));
        }
        let metadata = std::fs::metadata(&path).map_err(|error| {
            lapinux_core::Error::Other(format!("fichier inaccessible : {error}"))
        })?;
        if !metadata.is_file() {
            return Err(lapinux_core::Error::Other(
                "le chemin d'import n'est pas un fichier".into(),
            ));
        }
        if metadata.len() > 32 * 1024 * 1024 {
            return Err(lapinux_core::Error::Other(
                "fichier trop volumineux (32 Mo maximum)".into(),
            ));
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|error| lapinux_core::Error::Other(format!("fichier illisible : {error}")))?;
        let self_id = self
            .social
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().map(|social| social.snapshot().steam_id))
            .unwrap_or(0);
        let chats =
            parse_chat_export(&text, steam_id, self_id).map_err(lapinux_core::Error::Other)?;
        let Ok(mut archive) = self.chat_archive.lock() else {
            return Err(lapinux_core::Error::Other(
                "archive locale indisponible".into(),
            ));
        };
        let mut imported = 0usize;
        for (friend, messages) in chats {
            if friend == 0 {
                continue;
            }
            let target = archive.entry(friend).or_default();
            let before = target.len();
            merge_chat_messages(target, &messages);
            imported += target.len().saturating_sub(before);
        }
        save_chat_archive(&self.paths, &archive);
        Ok(Envelope::ok(&serde_json::json!({
            "imported": imported,
        })))
    }

    /// Crée un lobby Steam et attend la réponse du CM.
    ///
    /// Le délai est borné : sans réponse, la requête échoue proprement au
    /// lieu de laisser la session ou l'interface bloquée.
    /// Rejoint le lobby d'un ami : le jeu visé passe par le lancement
    /// habituel, enrichi de `+connect_lobby` et de `LAPINUX_JOIN_CONNECT`.
    pub fn steam_lobby_join(
        self: &Arc<Self>,
        appid: u32,
        lobby: u64,
        connect: &str,
    ) -> Result<Envelope> {
        let known = connect.trim().to_string();
        if lobby == 0 {
            return Err(lapinux_core::Error::Other(
                "invitation sans lobby : il n'y a rien à rejoindre".into(),
            ));
        }
        // L'invitation reçue fait foi : c'est elle que le joueur accepte, avec
        // son appid et son lobby. Un lobby du jeu garde l'appid demandé.
        let appid = lapinux_steam::social::received_invite(lobby)
            .map(|invite| invite.app_id)
            .unwrap_or(appid);
        // Le client rejoint le lobby : la session garde le lobby, suit les
        // membres déjà présents et rend l'adresse publiée lisible.
        let mut published = None;
        if let Ok(social) = self.ensure_social() {
            if let Some(answer) = lapinux_steam::social::join_lobby(&social, appid, lobby) {
                match answer.recv_timeout(Duration::from_secs(12)) {
                    Ok(Ok(joined)) => {
                        tracing::info!(
                            "Steam : lobby {lobby} rejoint ({} membre(s), {} place(s))",
                            joined.members.len(),
                            joined.max_members
                        );
                        published = joined.connect();
                    }
                    Ok(Err(eresult)) => {
                        return Err(lapinux_core::Error::Other(format!(
                            "Steam a refusé la rejointe du lobby (eresult {eresult})"
                        )))
                    }
                    Err(_) => tracing::warn!(
                        "Steam : aucune réponse à la rejointe du lobby {lobby} (délai dépassé)"
                    ),
                }
            }
        }
        let connect = join_address(&known, published.as_deref()).ok_or_else(|| {
            lapinux_core::Error::Other(
                "adresse de la partie inconnue : elle n'est pas publiée dans ce lobby".into(),
            )
        })?;
        queue_join(PendingJoin {
            appid,
            lobby,
            connect: connect.clone(),
        });
        tracing::info!("Steam : rejointe du lobby {lobby} (appid {appid}) à {connect}");
        let launched = self.launch(appid, false);
        if launched.is_err() {
            forget_join(appid);
        }
        launched
    }

    pub fn steam_lobby_create(
        self: &Arc<Self>,
        appid: u32,
        max_members: u32,
        lobby_type: Option<i32>,
        lobby_flags: Option<i32>,
        cell_id: Option<u32>,
    ) -> Result<Envelope> {
        if max_members == 0 || max_members > 250 {
            return Err(lapinux_core::Error::Other(
                "nombre de places invalide (1 à 250)".into(),
            ));
        }
        let social = self.ensure_social().map_err(|error| {
            lapinux_core::Error::Other(format!("session sociale indisponible : {error}"))
        })?;
        // La session peut être en cours de connexion : envoyer tout de suite
        // la ferait tomber sur « session absente ». On attend qu'elle vive,
        // sans dépasser le délai global.
        let connect = listen_address();
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        loop {
            if social.snapshot().live() {
                let answer = lapinux_steam::social::create_lobby(
                    &social,
                    appid,
                    max_members,
                    lobby_type,
                    lobby_flags,
                    cell_id,
                    connect.as_deref(),
                )
                .ok_or_else(|| {
                    lapinux_core::Error::Other(
                        "la session sociale n'accepte plus de commandes".into(),
                    )
                })?;
                return match answer.recv_timeout(Duration::from_secs(12)) {
                    Ok(Ok(lobby)) => {
                        if let Some(address) = connect.as_deref() {
                            remember_lobby_address(lobby, address);
                            lapinux_steam::social::set_lobby_data(&social, appid, lobby, address);
                        }
                        tracing::info!("Steam : lobby {lobby} prêt pour appid {appid}");
                        Ok(Envelope::ok(&serde_json::json!({
                            "appid": appid,
                            "lobby": lobby.to_string(),
                        })))
                    }
                    Ok(Err(eresult)) => Err(lapinux_core::Error::Other(format!(
                        "Steam a refusé la création du lobby (eresult {eresult})"
                    ))),
                    Err(_) => Err(lapinux_core::Error::Other(
                        "Steam n'a pas répondu à la création du lobby (délai dépassé)".into(),
                    )),
                };
            }
            if std::time::Instant::now() >= deadline {
                return Err(lapinux_core::Error::Other(
                    "la session sociale n'est pas connectée : réessayez dans un instant".into(),
                ));
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// Complète le nom du jeu des amis qui jouent.
    ///
    /// Steam ne publie pas toujours ce nom : la présence riche ne le porte que
    /// si le jeu l'écrit. L'appid, lui, est toujours là — le catalogue donne le
    /// nom des jeux possédés, la boutique publique celui des autres.
    fn name_friends_games(self: &Arc<Self>, friends: &mut [lapinux_core::social::SteamFriend]) {
        let playing: Vec<usize> = friends
            .iter()
            .enumerate()
            .filter(|(_, friend)| friend.game.trim().is_empty() && friend.game_appid != 0)
            .map(|(index, _)| index)
            .collect();
        if playing.is_empty() {
            return;
        }
        let catalog =
            lapinux_steam::catalog::Catalog::load(&self.paths.data_dir.join("steam-catalog.json"));
        for index in playing {
            let appid = friends[index].game_appid;
            if let Some(entry) = catalog
                .get(appid)
                .filter(|entry| !entry.name.trim().is_empty())
            {
                friends[index].game = entry.name.clone();
                continue;
            }
            if let Some(name) = cached_store_name(appid) {
                friends[index].game = name;
            }
        }
    }

    /// Complète le nom du jeu des événements d'amis.
    ///
    /// La présence riche ne publie pas toujours `game_name` : sans ce
    /// complément, une notification « jeu lancé/quitté » afficherait
    /// « un jeu ». Le catalogue donne les jeux possédés, la boutique les
    /// autres. Les appids inconnus sont bornés pour ne pas retarder la réponse
    /// sociale ; l'interface complète les plus anciens à l'affichage.
    fn name_friend_events(&self, events: &mut [SteamFriendEvent]) {
        let mut appids = HashSet::new();
        for event in events.iter() {
            if event.game.trim().is_empty() && event.appid != 0 && appids.len() < 10 {
                appids.insert(event.appid);
            }
        }
        if appids.is_empty() {
            return;
        }
        let catalog =
            lapinux_steam::catalog::Catalog::load(&self.paths.data_dir.join("steam-catalog.json"));
        let mut names = BTreeMap::new();
        for appid in appids {
            let name = catalog
                .get(appid)
                .map(|entry| entry.name.clone())
                .filter(|name| !name.trim().is_empty())
                .or_else(|| cached_store_name(appid));
            if let Some(name) = name {
                names.insert(appid, name);
            }
        }
        for event in events.iter_mut() {
            if event.game.trim().is_empty() {
                if let Some(name) = names.get(&event.appid) {
                    event.game = name.clone();
                }
            }
        }
    }

    /// Publie le jeu Steam en cours, ou efface l'état avec `None`.
    ///
    /// L'état est gardé même quand la session sociale n'est pas encore née :
    /// une reconnexion ou une pause du catalogue le reprendra.
    pub fn set_steam_playing(self: &Arc<Self>, appid: Option<u32>) {
        if let Ok(mut slot) = self.steam_playing.lock() {
            // Même sans session sociale vivante, un lobby de la partie
            // précédente ne doit pas resservir pour le jeu suivant.
            if *slot != appid {
                lapinux_steam::social::forget_lobby();
            }
            *slot = appid;
        }
        let config = self.config_snapshot();
        let wanted = if config.steam.show_playing {
            appid
        } else {
            None
        };
        let name = self.session_info().map(|info| info.name);
        if let Ok(slot) = self.social.lock() {
            if let Some(social) = slot.as_ref() {
                social.set_playing(wanted, name.as_deref());
                return;
            }
        }
        // Sans session vivante, inutile d'en ouvrir une uniquement pour
        // effacer un état : la prochaine session repartira proprement.
        let Some(appid) = wanted else {
            return;
        };
        match self.ensure_social() {
            Ok(social) => {
                social.set_playing(Some(appid), name.as_deref());
            }
            Err(error) => tracing::debug!("jeu en cours non publié sur Steam : {error}"),
        }
    }

    /// Change la présence publiée sur Steam et l'enregistre pour la prochaine
    /// ouverture.
    pub fn steam_social_presence(self: &Arc<Self>, presence: SteamPresence) -> Result<Envelope> {
        let mut config = self.config_snapshot();
        if config.steam.presence != presence {
            config.steam.presence = presence;
            self.save_config(config)?;
        }
        let social = match self.ensure_social() {
            Ok(social) => social,
            Err(error) => {
                return Ok(Envelope::ok(&SteamSocial::failed(error.to_string())));
            }
        };
        social.set_presence(presence);
        let snapshot = social.snapshot();
        Ok(Envelope::ok(&snapshot))
    }

    /// Met la session sociale en pause : Steam ne garde qu'une session par
    /// compte, chaque nouveau logon ferme la précédente, et la session sociale
    /// se reconnecte toute seule huit secondes plus tard — de quoi couper une
    /// synchronisation en plein vol. Rien ne la recrée tant que dure la pause.
    pub fn pause_social(&self) {
        self.social_paused.store(true, Ordering::SeqCst);
        self.stop_social();
    }

    /// Sort de la pause : la session sociale repart dans son fil.
    pub fn resume_social(self: &Arc<Self>) {
        self.social_paused.store(false, Ordering::SeqCst);
        self.start_social_in_background();
    }

    /// Arrête la session sociale (jeton oublié, réglage désactivé).
    pub fn stop_social(&self) {
        if let Ok(mut slot) = self.social.lock() {
            if let Some(social) = slot.take() {
                social.stop();
            }
        }
    }

    /// Démarre la session au lancement du daemon, sans bloquer : les erreurs
    /// réseau n'empêchent pas l'application de fonctionner.
    pub fn start_social_in_background(self: &Arc<Self>) {
        let shared = Arc::clone(self);
        std::thread::spawn(move || {
            if let Err(error) = shared.ensure_social() {
                tracing::debug!("session sociale non démarrée : {error}");
            }
        });
    }

    /// Relance la session après une connexion ou une déconnexion : le jeton a
    /// changé, l'ancienne session ne vaut plus rien.
    pub fn restart_social_in_background(self: &Arc<Self>) {
        self.stop_social();
        self.start_social_in_background();
    }

    /// Applique un changement de configuration à la session vivante : présence
    /// publiée, ou arrêt si la session sociale est désactivée.
    pub fn sync_social(&self) {
        let config = self.config_snapshot();
        if !config.steam.social {
            self.stop_social();
            return;
        }
        let playing = if config.steam.show_playing {
            self.steam_playing.lock().ok().and_then(|slot| *slot)
        } else {
            None
        };
        let name = self.session_info().map(|info| info.name);
        if let Ok(slot) = self.social.lock() {
            if let Some(social) = slot.as_ref() {
                social.set_presence(config.steam.presence);
                social.set_playing(playing, name.as_deref());
                return;
            }
        }
        // Aucune session en vie : elle naîtra au prochain besoin.
    }
}

#[cfg(test)]
mod tests {
    use super::Shared;
    use lapinux_core::paths::LapinuxPaths;
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

    #[test]
    fn la_session_sociale_ne_renait_pas_pendant_une_synchronisation() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        let shared = Arc::new(Shared::new(&paths).unwrap());
        shared.pause_social();
        assert!(
            shared.ensure_social().is_err(),
            "aucune session ne doit naître pendant la pause du catalogue"
        );
        shared.resume_social();
        assert!(!shared.social_paused.load(Ordering::SeqCst));
    }

    #[test]
    fn les_avatars_ne_viennent_que_de_steam() {
        assert!(super::is_steam_avatar_url(
            "https://avatars.fastly.steamstatic.com/abc_full.jpg"
        ));
        assert!(super::is_steam_avatar_url(
            "https://avatars.steamstatic.com/abc_full.jpg"
        ));
        assert!(!super::is_steam_avatar_url(
            "http://avatars.fastly.steamstatic.com/abc_full.jpg"
        ));
        assert!(!super::is_steam_avatar_url(
            "https://steamstatic.com.exemple/avatar.jpg"
        ));
        assert!(!super::is_steam_avatar_url(
            "https://exemple.com/abc_full.jpg"
        ));
    }

    #[test]
    fn la_pause_repond_un_etat_social_en_connexion() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        let shared = Arc::new(Shared::new(&paths).unwrap());
        shared.pause_social();
        let social = shared
            .steam_social(false)
            .unwrap()
            .into_data::<lapinux_core::social::SteamSocial>()
            .unwrap();
        assert!(social.error.is_none(), "la pause n'est pas une erreur");
        assert_eq!(social.state, "connecting");
        shared.resume_social();
    }
}

#[cfg(test)]
mod join_tests {
    use super::*;

    const LOBBY: u64 = 109775240000000001;
    const INVITER: u64 = 76561198000000001;

    #[test]
    fn une_invitation_rejointe_produit_variable_et_argument() {
        // 1. Réception du message 6621 : l'invitation est retenue.
        lapinux_steam::social::remember_invite(lapinux_steam::lobby::LobbyInvite {
            app_id: 4670920,
            lobby: LOBBY,
            inviter: INVITER,
        });
        let invite = lapinux_steam::social::received_invites()
            .into_iter()
            .find(|invite| invite.lobby == LOBBY)
            .expect("invitation retenue");
        // 2. L'adresse vient du registre, rempli à la création du lobby.
        remember_lobby_address(invite.lobby, "192.168.1.50:7777");
        let connect = lobby_address(invite.lobby).expect("adresse enregistrée");
        // 3. La requête SteamLobbyJoin pose la rejointe.
        queue_join(PendingJoin {
            appid: invite.app_id,
            lobby: invite.lobby,
            connect,
        });
        // 4. Le lancement du jeu visé la consomme, une seule fois.
        let join = take_pending_join(4670920).expect("rejointe en attente");
        assert!(
            take_pending_join(4670920).is_none(),
            "la rejointe ne sert qu'une fois"
        );
        let (env, args) = join_launch_additions(&join);
        assert_eq!(
            env,
            vec![(
                "LAPINUX_JOIN_CONNECT".to_string(),
                "192.168.1.50:7777".to_string()
            )]
        );
        assert_eq!(args, vec!["+connect_lobby".to_string(), LOBBY.to_string()]);
    }

    #[test]
    fn une_rejointe_ne_fuit_pas_sur_un_autre_jeu() {
        queue_join(PendingJoin {
            appid: 10,
            lobby: 7,
            connect: "10.0.0.2:7777".into(),
        });
        assert!(
            take_pending_join(20).is_none(),
            "autre jeu : rien à prendre"
        );
        assert!(take_pending_join(10).is_some(), "le jeu visé la reçoit");
    }

    #[test]
    fn les_invitations_affichees_portent_l_adresse_du_lobby() {
        lapinux_steam::social::remember_invite(lapinux_steam::lobby::LobbyInvite {
            app_id: 30,
            lobby: 31,
            inviter: 32,
        });
        remember_lobby_address(31, "10.0.0.3:7777");
        let invite = lobby_invites(&[])
            .into_iter()
            .find(|invite| invite.lobby == 31)
            .expect("invitation listée");
        assert_eq!(invite.appid, 30);
        assert_eq!(invite.inviter, 32);
        assert_eq!(invite.connect.as_deref(), Some("10.0.0.3:7777"));
    }

    #[test]
    fn l_adresse_publiee_sert_quand_elle_n_etait_pas_connue() {
        assert_eq!(
            join_address("", Some("10.1.2.3:7777")).as_deref(),
            Some("10.1.2.3:7777")
        );
        assert_eq!(
            join_address("10.0.0.9:7777", Some("10.1.2.3:7777")).as_deref(),
            Some("10.0.0.9:7777"),
            "l'adresse connue prime"
        );
        assert!(join_address("", None).is_none());
        assert!(join_address("pas-une-adresse", None).is_none());
    }

    #[test]
    fn une_invitation_acceptee_rejoint_son_lobby() {
        let raw = lapinux_steam::lobby::invite_to_lobby_body(70, 700, 76_561_198_000_000_001);
        lapinux_steam::social::receive_invite(&raw).expect("invitation reçue");
        // 1. Acceptation : la demande porte le lobby de l'invitation.
        let (appid, body) =
            lapinux_steam::social::invite_join_body(700, "test-user", 0).expect("demande de join");
        assert_eq!(appid, 70);
        let mut key = vec![0x11];
        key.extend_from_slice(&700u64.to_le_bytes());
        assert!(
            body.windows(key.len()).any(|window| window == key),
            "le lobby de l'invitation part au champ 2"
        );
        // 2. Réponse acceptée : lobby et adresse enregistrés.
        let mut joined = lapinux_steam::lobby::JoinedLobby {
            app_id: appid,
            lobby: 700,
            eresult: 1,
            ..Default::default()
        };
        joined.metadata = vec![("connect".to_string(), "10.5.4.3:7777".to_string())];
        assert!(lapinux_steam::social::record_join_result(&joined));
        let address = lobby_address(700).expect("adresse du lobby");
        assert_eq!(address, "10.5.4.3:7777");
        // 3. L'adresse revient à l'invité et part au lancement.
        let connect = join_address("", Some(&address)).expect("adresse retenue");
        let (env, args) = join_launch_additions(&PendingJoin {
            appid,
            lobby: 700,
            connect,
        });
        assert_eq!(
            env,
            vec![(
                "LAPINUX_JOIN_CONNECT".to_string(),
                "10.5.4.3:7777".to_string()
            )]
        );
        assert_eq!(args, vec!["+connect_lobby".to_string(), "700".to_string()]);
        // 4. Refus : rien d'enregistré, aucune adresse.
        let refused = lapinux_steam::lobby::JoinedLobby {
            app_id: appid,
            lobby: 701,
            eresult: 2,
            ..Default::default()
        };
        assert!(!lapinux_steam::social::record_join_result(&refused));
        assert!(lobby_address(701).is_none());
    }

    #[test]
    fn une_adresse_publiee_au_lobby_revient_au_daemon() {
        lapinux_steam::social::remember_lobby_connect(600, "10.9.8.7:7777");
        assert_eq!(lobby_address(600).as_deref(), Some("10.9.8.7:7777"));
        lapinux_steam::social::remember_invite(lapinux_steam::lobby::LobbyInvite {
            app_id: 50,
            lobby: 600,
            inviter: 51,
        });
        let invite = lobby_invites(&[])
            .into_iter()
            .find(|invite| invite.lobby == 600)
            .expect("invitation listée");
        assert_eq!(invite.connect.as_deref(), Some("10.9.8.7:7777"));
    }

    #[test]
    fn sans_adresse_l_invitation_reste_sans_rejointe() {
        lapinux_steam::social::remember_invite(lapinux_steam::lobby::LobbyInvite {
            app_id: 40,
            lobby: 41,
            inviter: 42,
        });
        let invite = lobby_invites(&[])
            .into_iter()
            .find(|invite| invite.lobby == 41)
            .expect("invitation listée");
        assert!(
            invite.connect.is_none(),
            "aucune adresse : bouton désactivé"
        );
    }
}
