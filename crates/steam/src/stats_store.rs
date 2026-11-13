//! Écriture des succès d'un jeu, sans client Steam.
//!
//! Valve n'accepte plus `CMsgClientStoreUserStats` (EMsg 820) d'une session
//! nue : le CM coupe la connexion (`ClientLoggedOff`, `EResult` 7). Le chemin
//! qui répond, vérifié sur Spacewar et Team Fortress 2, est
//! `CMsgClientStoreUserStats2` (EMsg 5466), à condition de joindre le
//! `crc_stats` du schéma : sans lui le CM refuse en `EResult` 8
//! (`InvalidParam`) et signale toutes les statistiques comme périmées.
//!
//! Ce module ne débloque rien de lui-même : il reçoit les noms des succès que
//! le jeu a réellement accordés (voir le pont Steamworks
//! `crates/steamshim`), les traduit en bits du schéma et n'écrit que les
//! statistiques qui changent. Un succès déjà débloqué n'est jamais réécrit.

use crate::cm::proto::{field_bytes, field_fixed64, field_varint, Field, Reader};
use crate::{Error, Result};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};
use steamroom::client::msg::ClientMsg;
use steamroom::client::multi::unpack_multi;
use steamroom::client::{LoggedIn, SteamClient};
use steamroom::messages::header::PacketHeader;
use steamroom::messages::EMsg;

/// Schéma et valeurs du compte pour un jeu.
const STATS_METHOD: &str = "Player.GetUserStats#1";
/// `k_EMsgClientStoreUserStats2`.
const CLIENT_STORE_USER_STATS2: u32 = 5466;
/// `k_EMsgClientStoreUserStatsResponse`.
const CLIENT_STORE_USER_STATS_RESPONSE: u32 = 821;
/// Un CM qui ne répond pas en un quart de minute est perdu.
const REPLY_TIMEOUT: Duration = Duration::from_secs(20);

/// Statistiques du compte lues par `Player.GetUserStats#1`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GameStats {
    /// Schéma KV du jeu, tel que le publie Steam.
    pub schema: Vec<u8>,
    /// Valeurs du compte, par identifiant de statistique.
    pub values: BTreeMap<u32, u32>,
    /// Empreinte du schéma, exigée par le CM pour accepter l'écriture.
    pub crc: Option<u32>,
}

/// Position d'un succès dans le schéma : une statistique et un bit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AchievementSlot {
    pub stat_id: u32,
    pub bit: u32,
}

/// Résultat d'une écriture.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoreReport {
    pub eresult: i32,
    /// Vrai quand le CM a répondu `EResult::OK`.
    pub accepted: bool,
    pub out_of_date: bool,
    /// Succès déjà débloqués : rien n'a été renvoyé pour eux.
    pub already: Vec<String>,
    /// Succès inconnus du schéma du jeu.
    pub unknown: Vec<String>,
    /// Statistiques réellement envoyées.
    pub written: Vec<u32>,
    /// Statistiques refusées par le CM, avec la valeur qu'il a rétablie.
    pub failed: Vec<(u32, u32)>,
}

impl StoreReport {
    fn unchanged() -> Self {
        Self {
            eresult: 1,
            accepted: true,
            ..Default::default()
        }
    }
}

/// Lit le schéma, les valeurs et l'empreinte d'une réponse `GetUserStats`.
pub fn parse_user_stats(body: &[u8]) -> GameStats {
    let mut stats = GameStats::default();
    let mut reader = Reader::new(body);
    while let Some((number, field)) = reader.next() {
        match (number, field) {
            (2, Field::Varint(value)) => stats.crc = Some(value as u32),
            (3, Field::Bytes(schema)) => stats.schema = schema.to_vec(),
            (4, Field::Bytes(entry)) => {
                let mut stat_id = 0u32;
                let mut value = 0u32;
                let mut inner = Reader::new(entry);
                while let Some((number, field)) = inner.next() {
                    if let Field::Varint(raw) = field {
                        match number {
                            1 => stat_id = u32::try_from(raw).unwrap_or_default(),
                            2 => value = u32::try_from(raw).unwrap_or_default(),
                            _ => {}
                        }
                    }
                }
                stats.values.insert(stat_id, value);
            }
            _ => {}
        }
    }
    stats
}

/// Succès du schéma : nom interne → statistique et bit.
///
/// La racine porte l'appid, puis `stats` ; chaque statistique peut déclarer
/// une section `bits`, une par succès, dont la clé est le rang du bit.
pub fn achievement_slots(schema: &[u8]) -> BTreeMap<String, AchievementSlot> {
    let mut slots = BTreeMap::new();
    let Some(root) = Kv::parse(schema) else {
        return slots;
    };
    let Some(Kv::Children(stats)) = root.children.get("stats") else {
        return slots;
    };
    for (stat_key, stat) in stats {
        let Ok(stat_id) = stat_key.parse::<u32>() else {
            continue;
        };
        let Kv::Children(fields) = stat else {
            continue;
        };
        let Some(Kv::Children(bits)) = fields.get("bits") else {
            continue;
        };
        for (bit_key, bit) in bits {
            let Kv::Children(definition) = bit else {
                continue;
            };
            let Some(Kv::String(name)) = definition.get("name") else {
                continue;
            };
            let bit = bit_key.parse::<u32>().ok().or_else(|| {
                definition.get("bit").and_then(|value| match value {
                    Kv::String(text) => text.parse().ok(),
                    _ => None,
                })
            });
            if let Some(bit) = bit.filter(|bit| *bit < 32) {
                slots.insert(name.clone(), AchievementSlot { stat_id, bit });
            }
        }
    }
    slots
}

/// Corps de `CMsgClientStoreUserStats2` : jeu, auteur, cible, empreinte,
/// statistiques. L'auteur et la cible sont le compte connecté : c'est le
/// chemin qu'emprunte le client quand il enregistre ses propres succès.
pub fn store_body(appid: u32, steam_id: u64, crc: u32, updates: &[(u32, u32)]) -> Vec<u8> {
    let mut body = Vec::new();
    field_fixed64(1, u64::from(appid), &mut body);
    field_fixed64(2, steam_id, &mut body);
    field_fixed64(3, steam_id, &mut body);
    field_varint(4, u64::from(crc), &mut body);
    for (stat_id, value) in updates {
        let mut entry = Vec::new();
        field_varint(1, u64::from(*stat_id), &mut entry);
        field_varint(2, u64::from(*value), &mut entry);
        field_bytes(6, &entry, &mut body);
    }
    body
}

/// Réponse `CMsgClientStoreUserStatsResponse` : code, péremption, échecs.
pub fn parse_store_response(body: &[u8]) -> (i32, bool, Vec<(u32, u32)>) {
    let mut eresult = 0;
    let mut out_of_date = false;
    let mut failed = Vec::new();
    let mut reader = Reader::new(body);
    while let Some((number, field)) = reader.next() {
        match (number, field) {
            (2, Field::Varint(value)) => eresult = value as i32,
            (5, Field::Varint(value)) => out_of_date = value != 0,
            (4, Field::Bytes(entry)) => {
                let mut stat = 0u32;
                let mut reverted = 0u32;
                let mut inner = Reader::new(entry);
                while let Some((number, field)) = inner.next() {
                    if let Field::Varint(value) = field {
                        match number {
                            1 => stat = u32::try_from(value).unwrap_or_default(),
                            2 => reverted = u32::try_from(value).unwrap_or_default(),
                            _ => {}
                        }
                    }
                }
                failed.push((stat, reverted));
            }
            _ => {}
        }
    }
    (eresult, out_of_date, failed)
}

/// Débloque les succès `unlocked` pour ce compte, sur une session ouverte.
///
/// Seuls les bits réellement nouveaux partent : un succès déjà débloqué est
/// laissé tel quel, et un nom absent du schéma est signalé sans écriture.
pub async fn store_achievements(
    client: &SteamClient<LoggedIn>,
    appid: u32,
    steam_id: u64,
    unlocked: &[String],
) -> Result<StoreReport> {
    let stats = read_stats(client, appid, steam_id).await?;
    let slots = achievement_slots(&stats.schema);
    let mut values = stats.values.clone();
    let mut report = StoreReport::default();
    let mut touched = Vec::new();
    for name in unlocked {
        let Some(slot) = slots.get(name) else {
            report.unknown.push(name.clone());
            continue;
        };
        let current = values.get(&slot.stat_id).copied().unwrap_or(0);
        if current & (1u32 << slot.bit) != 0 {
            report.already.push(name.clone());
            continue;
        }
        values.insert(slot.stat_id, current | (1u32 << slot.bit));
        if !touched.contains(&slot.stat_id) {
            touched.push(slot.stat_id);
        }
    }
    let updates: Vec<(u32, u32)> = touched
        .iter()
        .map(|stat_id| (*stat_id, values.get(stat_id).copied().unwrap_or(0)))
        .collect();
    if updates.is_empty() {
        return Ok(StoreReport {
            already: report.already,
            unknown: report.unknown,
            ..StoreReport::unchanged()
        });
    }
    let body = store_body(appid, steam_id, stats.crc.unwrap_or(0), &updates);
    let mut message = ClientMsg::with_body(EMsg(CLIENT_STORE_USER_STATS2), &body);
    message.header.steamid = Some(steam_id);
    client
        .send_msg(&message)
        .await
        .map_err(|error| Error::Other(format!("envoi des succès : {error}")))?;
    let (eresult, out_of_date, failed) = wait_response(client).await?;
    Ok(StoreReport {
        eresult,
        accepted: eresult == 1,
        out_of_date,
        failed,
        written: updates.iter().map(|(stat_id, _)| *stat_id).collect(),
        ..report
    })
}

/// Débloque des succès depuis un contexte synchrone (jeu, shim).
///
/// Ouvre sa propre session avec le jeton enregistré : le shim n'a pas accès au
/// runtime du daemon, et une écriture de fin de partie ne doit pas dépendre de
/// lui.
pub fn store_achievements_blocking(appid: u32, unlocked: &[String]) -> Result<StoreReport> {
    let session = crate::session::load().ok_or_else(|| {
        Error::Other("compte Steam non connecté : aucun succès ne peut être enregistré".into())
    })?;
    if session.steam_id == 0 || session.refresh_token.is_empty() {
        return Err(Error::Other("session Steam incomplète".into()));
    }
    crate::install_crypto_provider();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| Error::Other(format!("runtime réseau : {error}")))?;
    runtime.block_on(async {
        let client = steamroom_client::login::LoginBuilder::new()
            .device_name("lapinux-steamworks")
            .with_refresh_token(session.account_name.clone(), session.refresh_token.clone())
            .login()
            .await
            .map_err(|error| Error::Other(format!("connexion CM : {error}")))?;
        store_achievements(&client, appid, session.steam_id, unlocked).await
    })
}

/// Débloque des succès sur la session partagée d'un processus (le daemon).
///
/// Steam ne garde qu'une session par compte : plutôt que d'en ouvrir une
/// seconde, le pont Steamworks confie l'écriture au daemon, qui réutilise la
/// connexion déjà ouverte pour les lectures et les bandeaux.
pub fn store_achievements_shared(
    account: &str,
    refresh_token: &str,
    steam_id: u64,
    appid: u32,
    unlocked: &[String],
) -> Result<StoreReport> {
    if account.trim().is_empty() || refresh_token.trim().is_empty() || steam_id == 0 {
        return Err(Error::Other("compte Steam non connecté".into()));
    }
    if unlocked.is_empty() {
        return Ok(StoreReport::unchanged());
    }
    crate::install_crypto_provider();
    crate::shared::block_on(async {
        let client = crate::shared::session(account, refresh_token).await?;
        store_achievements(&client, appid, steam_id, unlocked).await
    })
}

async fn read_stats(
    client: &SteamClient<LoggedIn>,
    appid: u32,
    steam_id: u64,
) -> Result<GameStats> {
    let mut body = Vec::new();
    field_varint(1, steam_id, &mut body);
    field_varint(2, u64::from(appid), &mut body);
    let answer = tokio::time::timeout(
        REPLY_TIMEOUT,
        client.call_service_method(STATS_METHOD, &body),
    )
    .await
    .map_err(|_| Error::Other("Steam n'a pas répondu".into()))?
    .map_err(|error| Error::Other(error.to_string()))?;
    Ok(parse_user_stats(&answer.body))
}

async fn wait_response(client: &SteamClient<LoggedIn>) -> Result<(i32, bool, Vec<(u32, u32)>)> {
    let deadline = Instant::now() + REPLY_TIMEOUT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(Error::Other(
                "aucune réponse du CM pour l'enregistrement des succès".into(),
            ));
        }
        let message = tokio::time::timeout(left, client.recv_msg())
            .await
            .map_err(|_| Error::Other("réception CM expirée".into()))?
            .map_err(|error| Error::Other(format!("réception CM : {error}")))?;
        if let Some(body) = store_payload(message.emsg.0, &message.body) {
            return Ok(parse_store_response(&body));
        }
    }
}

/// Corps de la réponse 821, y compris quand le CM l'a groupée dans un `Multi`.
fn store_payload(emsg: u32, body: &[u8]) -> Option<Vec<u8>> {
    if emsg == CLIENT_STORE_USER_STATS_RESPONSE {
        return Some(body.to_vec());
    }
    if emsg != 1 {
        return None;
    }
    for part in unpack_multi(body).ok()? {
        if let Ok(PacketHeader::Protobuf { header, body }) = PacketHeader::parse(&part) {
            if header.emsg.0 == CLIENT_STORE_USER_STATS_RESPONSE {
                return Some(body.to_vec());
            }
        }
    }
    None
}

/// Nœud du format KV binaire de Steam, réduit à ce que le schéma emploie.
#[derive(Debug, Clone)]
enum Kv {
    Children(BTreeMap<String, Kv>),
    String(String),
}

impl Kv {
    /// Lit un nœud racine (`tag | clé\0 | …`).
    fn parse(bytes: &[u8]) -> Option<Root> {
        let mut cursor = Cursor {
            data: bytes,
            pos: 0,
        };
        let (_, node) = cursor.node()?;
        let Kv::Children(children) = node else {
            return None;
        };
        Some(Root { children })
    }
}

struct Root {
    children: BTreeMap<String, Kv>,
}

/// Lecteur du KV binaire (`tag | clé\0 | valeur`).
struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn byte(&mut self) -> Option<u8> {
        let byte = *self.data.get(self.pos)?;
        self.pos += 1;
        Some(byte)
    }

    fn cstring(&mut self) -> Option<String> {
        let start = self.pos;
        while *self.data.get(self.pos)? != 0 {
            self.pos += 1;
        }
        let text = String::from_utf8_lossy(&self.data[start..self.pos]).into_owned();
        self.pos += 1;
        Some(text)
    }

    fn skip(&mut self, count: usize) -> Option<()> {
        if self.pos + count > self.data.len() {
            return None;
        }
        self.pos += count;
        Some(())
    }

    fn node(&mut self) -> Option<(String, Kv)> {
        let tag = self.byte()?;
        let key = self.cstring()?;
        let value = match tag {
            0 => Kv::Children(self.children()?),
            1 => Kv::String(self.cstring()?),
            2 | 3 | 4 | 6 => {
                self.skip(4)?;
                Kv::String(String::new())
            }
            5 => Kv::String(self.wide()?),
            7 | 10 => {
                self.skip(8)?;
                Kv::String(String::new())
            }
            _ => return None,
        };
        Some((key, value))
    }

    fn wide(&mut self) -> Option<String> {
        let start = self.pos;
        while self.pos + 1 < self.data.len()
            && !(self.data[self.pos] == 0 && self.data[self.pos + 1] == 0)
        {
            self.pos += 2;
        }
        let slice = self.data.get(start..self.pos)?;
        self.pos += 2;
        Some(String::from_utf16_lossy(
            &slice
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect::<Vec<_>>(),
        ))
    }

    fn children(&mut self) -> Option<BTreeMap<String, Kv>> {
        let mut map = BTreeMap::new();
        loop {
            let tag = *self.data.get(self.pos)?;
            if tag == 8 || tag == 11 {
                self.pos += 1;
                return Some(map);
            }
            let (key, value) = self.node()?;
            map.insert(key, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cm::proto::field_bytes;

    fn node(tag: u8, key: &str, value: &[u8], out: &mut Vec<u8>) {
        out.push(tag);
        out.extend_from_slice(key.as_bytes());
        out.push(0);
        out.extend_from_slice(value);
    }

    fn string(key: &str, value: &str, out: &mut Vec<u8>) {
        node(1, key, format!("{value}\0").as_bytes(), out);
    }

    fn children(key: &str, body: &[u8], out: &mut Vec<u8>) {
        node(0, key, body, out);
        out.push(8);
    }

    /// Schéma minimal : un succès « WIN » sur la statistique 0, bit 0.
    fn schema() -> Vec<u8> {
        let mut bit = Vec::new();
        string("name", "ACH_WIN", &mut bit);
        let mut bits = Vec::new();
        children("0", &bit, &mut bits);
        let mut stat = Vec::new();
        children("bits", &bits, &mut stat);
        let mut stats = Vec::new();
        children("0", &stat, &mut stats);
        let mut app = Vec::new();
        children("stats", &stats, &mut app);
        let mut root = Vec::new();
        children("480", &app, &mut root);
        root
    }

    fn response(values: &[(u32, u32)], crc: u32) -> Vec<u8> {
        let mut body = Vec::new();
        field_varint(2, u64::from(crc), &mut body);
        field_bytes(3, &schema(), &mut body);
        for (id, value) in values {
            let mut entry = Vec::new();
            field_varint(1, u64::from(*id), &mut entry);
            field_varint(2, u64::from(*value), &mut entry);
            field_bytes(4, &entry, &mut body);
        }
        body
    }

    #[test]
    fn the_schema_gives_the_stat_and_bit_of_each_achievement() {
        let slots = achievement_slots(&schema());
        assert_eq!(
            slots.get("ACH_WIN"),
            Some(&AchievementSlot { stat_id: 0, bit: 0 })
        );
    }

    #[test]
    fn the_read_keeps_values_and_the_schema_crc() {
        let stats = parse_user_stats(&response(&[(0, 3)], 4_242));
        assert_eq!(stats.values.get(&0), Some(&3));
        assert_eq!(stats.crc, Some(4_242));
        assert!(!stats.schema.is_empty());
    }

    #[test]
    fn the_store_body_carries_the_crc_and_every_stat() {
        let body = store_body(480, 7, 99, &[(0, 3), (4, 1)]);
        let mut reader = Reader::new(&body);
        let mut stats = 0;
        let mut crc = None;
        while let Some((number, field)) = reader.next() {
            match (number, field) {
                (4, Field::Varint(value)) => crc = Some(value),
                (6, Field::Bytes(_)) => stats += 1,
                _ => {}
            }
        }
        assert_eq!(crc, Some(99));
        assert_eq!(stats, 2);
    }

    #[test]
    fn a_response_is_read_even_when_it_is_grouped_in_a_multi() {
        let mut inner = Vec::new();
        field_varint(2, 1, &mut inner);
        let mut packet = (821u32 | 0x8000_0000).to_le_bytes().to_vec();
        let header = crate::cm::proto::proto_header(7, 0);
        packet.extend_from_slice(&(header.len() as u32).to_le_bytes());
        packet.extend_from_slice(&header);
        packet.extend_from_slice(&inner);
        let mut payload = (packet.len() as u32).to_le_bytes().to_vec();
        payload.extend_from_slice(&packet);
        let mut multi = Vec::new();
        field_bytes(2, &payload, &mut multi);
        assert_eq!(
            store_payload(1, &multi).map(|body| parse_store_response(&body).0),
            Some(1)
        );
        assert_eq!(
            store_payload(821, &inner).map(|body| parse_store_response(&body).0),
            Some(1)
        );
        assert!(store_payload(703, &inner).is_none());
    }

    #[test]
    fn a_refusal_keeps_the_reverted_values() {
        let mut entry = Vec::new();
        field_varint(1, 0, &mut entry);
        field_varint(2, 1, &mut entry);
        let mut body = Vec::new();
        field_varint(2, 8, &mut body);
        field_varint(5, 1, &mut body);
        field_bytes(4, &entry, &mut body);
        let (eresult, out_of_date, failed) = parse_store_response(&body);
        assert_eq!(eresult, 8);
        assert!(out_of_date);
        assert_eq!(failed, vec![(0, 1)]);
    }

    #[test]
    fn a_never_unlocked_achievement_is_not_rewritten() {
        // Le schéma sans `bits` ne promet aucun succès : rien ne doit partir.
        let mut body = Vec::new();
        field_bytes(3, b"\x00\x34\x38\x30\x00\x08", &mut body);
        let stats = parse_user_stats(&body);
        assert!(achievement_slots(&stats.schema).is_empty());
    }
}
