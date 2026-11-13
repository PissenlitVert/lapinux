//! Protocole de lobby Steam (messages MMS) : encodage de la demande et
//! lecture de la réponse.
//!
//! `CMsgClientMMSCreateLobby` (EMsg 6601) porte `app_id` (1), `max_members`
//! (2), `lobby_type` (3), `lobby_flags` (4), `metadata` (11, répété de
//! `MMSKeyValuePair`) et `persona_name_owner` (8). La réponse 6602 rend
//! `app_id` (1), `steam_id_lobby` (2, fixed64) et `eresult` (3).
//!
//! `CMsgClientMMSSetLobbyData` (6609) écrit les mêmes paires au champ 9, et
//! `CMsgClientMMSLobbyData` (6612) les relit au champ 12 : c'est par là que
//! l'adresse de la partie circule jusqu'à l'invité. Le champ `bytes` 7/8 est
//! l'ancien blob binaire, que les clients modernes n'écrivent plus.
//!
//! L'encodage est fait ici sans dépendance : le protocole est petit et
//! stable, et cela évite de coupler ce module au client CM.

/// Réponse à une création de lobby.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LobbyCreated {
    pub app_id: u32,
    pub lobby: u64,
    pub eresult: i32,
}

fn varint(mut value: u64, out: &mut Vec<u8>) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn key(field: u32, wire: u8, out: &mut Vec<u8>) {
    varint(((field as u64) << 3) | u64::from(wire), out);
}

fn field_varint(field: u32, value: u64, out: &mut Vec<u8>) {
    key(field, 0, out);
    varint(value, out);
}

fn field_fixed32(field: u32, value: u32, out: &mut Vec<u8>) {
    key(field, 5, out);
    out.extend_from_slice(&value.to_le_bytes());
}

fn field_fixed64(field: u32, value: u64, out: &mut Vec<u8>) {
    key(field, 1, out);
    out.extend_from_slice(&value.to_le_bytes());
}

fn field_bytes(field: u32, value: &[u8], out: &mut Vec<u8>) {
    key(field, 2, out);
    varint(value.len() as u64, out);
    out.extend_from_slice(value);
}

/// Corps de `CMsgClientMMSCreateLobby`. `persona` est le nom affiché du
/// propriétaire ; vide, le champ est omis. `metadata` accompagne le lobby dès
/// sa création : c'est le champ 11 (`repeated MMSKeyValuePair`).
pub fn create_lobby_body(
    app_id: u32,
    max_members: u32,
    lobby_type: i32,
    lobby_flags: i32,
    cell_id: u32,
    public_ip: Option<[u8; 4]>,
    persona: &str,
    metadata: &[(&str, &str)],
) -> Vec<u8> {
    let mut out = Vec::new();
    field_varint(1, u64::from(app_id), &mut out);
    field_varint(2, u64::from(max_members), &mut out);
    field_varint(3, lobby_type as u64, &mut out);
    field_varint(4, lobby_flags as u64, &mut out);
    field_varint(5, u64::from(cell_id), &mut out);
    for (name, value) in metadata {
        field_bytes(11, &metadata_pair(name, value), &mut out);
    }
    // `CMsgIPAddress` (champ 9) : sous-message `v4` (fixed32) comme le
    // client Steam, pas l'ancien `deprecated_public_ip`.
    if let Some(ip) = public_ip {
        let mut inner = Vec::new();
        field_fixed32(1, u32::from_le_bytes(ip), &mut inner);
        field_bytes(9, &inner, &mut out);
    }
    if !persona.is_empty() {
        field_bytes(8, persona.as_bytes(), &mut out);
    }
    out
}

/// Invitation reçue d'un ami : le lobby à rejoindre et qui l'a envoyée.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LobbyInvite {
    pub app_id: u32,
    pub lobby: u64,
    pub inviter: u64,
}

/// Lit `CMsgClientMMSInviteToLobby` reçu d'un ami : `app_id` (1),
/// `steam_id_lobby` (2, fixed64) et `steam_id_inviter` (3, fixed64).
/// Sans lobby ni appid, il n'y a rien à rejoindre.
pub fn parse_invite(body: &[u8]) -> Option<LobbyInvite> {
    let mut reader = Reader::new(body);
    let mut invite = LobbyInvite {
        app_id: 0,
        lobby: 0,
        inviter: 0,
    };
    while reader.at < body.len() {
        let header = reader.varint()?;
        let field = (header >> 3) as u32;
        let wire = (header & 7) as u8;
        match (field, wire) {
            (1, 0) => invite.app_id = reader.varint()? as u32,
            (2, 1) => invite.lobby = reader.fixed64()?,
            (3, 1) => invite.inviter = reader.fixed64()?,
            _ => reader.skip(wire)?,
        }
    }
    (invite.app_id != 0 && invite.lobby != 0).then_some(invite)
}

/// Une paire de métadonnées de lobby, encodée comme `MMSKeyValuePair`
/// (`name` 1, `value` 2).
fn metadata_pair(name: &str, value: &str) -> Vec<u8> {
    let mut pair = Vec::new();
    field_bytes(1, name.as_bytes(), &mut pair);
    field_bytes(2, value.as_bytes(), &mut pair);
    pair
}

/// Corps de `CMsgClientMMSSetLobbyData` (6609) : l'adresse de la partie est
/// publiée dans les métadonnées (`metadata`, champ 9). Les champs de capacité
/// et de type sont omis, pour ne pas écraser ceux du lobby.
pub fn set_lobby_data_body(app_id: u32, lobby: u64, metadata: &[(&str, &str)]) -> Vec<u8> {
    let mut out = Vec::new();
    field_varint(1, u64::from(app_id), &mut out);
    field_fixed64(2, lobby, &mut out);
    field_fixed64(3, 0, &mut out);
    for (name, value) in metadata {
        field_bytes(9, &metadata_pair(name, value), &mut out);
    }
    out
}

/// Corps de `CMsgClientMMSGetLobbyData` (6611).
pub fn get_lobby_data_body(app_id: u32, lobby: u64) -> Vec<u8> {
    let mut out = Vec::new();
    field_varint(1, u64::from(app_id), &mut out);
    field_fixed64(2, lobby, &mut out);
    out
}

/// Métadonnées d'un lobby, lues dans `CMsgClientMMSLobbyData` (6612).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LobbyData {
    pub app_id: u32,
    pub lobby: u64,
    pub metadata: Vec<(String, String)>,
}

impl LobbyData {
    /// Adresse `ip:port` de la partie, sous la clé que Steamworks lit
    /// (`connect`), sinon les clés équivalentes de Source et d'Unreal.
    pub fn connect(&self) -> Option<String> {
        connect_from(&self.metadata)
    }
}

/// Adresse `ip:port` dans une liste de paires, sous `connect` ou l'une des
/// clés équivalentes des jeux Source et Unreal.
pub fn connect_from(metadata: &[(String, String)]) -> Option<String> {
    for wanted in ["connect", "connectstring", "server", "gameserver"] {
        if let Some((_, value)) = metadata
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(wanted))
        {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

/// Corps de `CMsgClientMMSJoinLobby` (6603) : `app_id` (1),
/// `steam_id_lobby` (2, fixed64), `persona_name` (3) et `cell_id` (5).
pub fn join_lobby_body(app_id: u32, lobby: u64, persona: &str, cell_id: u32) -> Vec<u8> {
    let mut out = Vec::new();
    field_varint(1, u64::from(app_id), &mut out);
    field_fixed64(2, lobby, &mut out);
    if !persona.is_empty() {
        field_bytes(3, persona.as_bytes(), &mut out);
    }
    if cell_id != 0 {
        field_varint(5, u64::from(cell_id), &mut out);
    }
    out
}

/// Un membre présent dans le lobby au moment de la rejointe.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LobbyMember {
    pub steam_id: u64,
    pub persona_name: String,
}

/// Réponse `CMsgClientMMSJoinLobbyResponse` (6604) : le verdict
/// (`chat_room_enter_response`, 1 = accepté), le lobby effectif et les
/// membres déjà présents.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JoinedLobby {
    pub app_id: u32,
    pub lobby: u64,
    pub eresult: i32,
    pub max_members: i32,
    pub lobby_type: i32,
    pub lobby_flags: i32,
    pub owner: u64,
    pub metadata: Vec<(String, String)>,
    pub members: Vec<LobbyMember>,
}

impl JoinedLobby {
    /// Adresse de la partie publiée par l'hôte, quand elle est arrivée avec
    /// la réponse de join.
    pub fn connect(&self) -> Option<String> {
        connect_from(&self.metadata)
    }
}

/// Lit `CMsgClientMMSJoinLobbyResponse` : `app_id` (1), `steam_id_lobby`
/// (2, fixed64), `chat_room_enter_response` (3), capacités et type (4 à 6),
/// propriétaire (7, fixed64), membres (9) et métadonnées (10).
pub fn parse_join_lobby_response(body: &[u8]) -> Option<JoinedLobby> {
    let mut reader = Reader::new(body);
    let mut joined = JoinedLobby::default();
    while reader.at < body.len() {
        let header = reader.varint()?;
        let field = (header >> 3) as u32;
        let wire = (header & 7) as u8;
        match (field, wire) {
            (1, 0) => joined.app_id = reader.varint()? as u32,
            (2, 1) => joined.lobby = reader.fixed64()?,
            (3, 0) => joined.eresult = reader.varint()? as i32,
            (4, 0) => joined.max_members = reader.varint()? as i32,
            (5, 0) => joined.lobby_type = reader.varint()? as i32,
            (6, 0) => joined.lobby_flags = reader.varint()? as i32,
            (7, 1) => joined.owner = reader.fixed64()?,
            (9, 2) => joined.members.extend(parse_member(reader.bytes()?)),
            (10, 2) => joined
                .metadata
                .extend(parse_metadata_pairs(reader.bytes()?)),
            _ => reader.skip(wire)?,
        }
    }
    // Une réponse sans appid n'est pas exploitable ; un refus, lui, peut
    // n'avoir ni lobby ni membre et doit tout de même remonter.
    (joined.app_id != 0).then_some(joined)
}

fn parse_member(body: &[u8]) -> Option<LobbyMember> {
    let mut reader = Reader::new(body);
    let mut member = LobbyMember::default();
    while reader.at < body.len() {
        let header = reader.varint()?;
        let field = (header >> 3) as u32;
        let wire = (header & 7) as u8;
        match (field, wire) {
            (1, 1) => member.steam_id = reader.fixed64()?,
            (2, 2) => member.persona_name = reader.string()?,
            _ => reader.skip(wire)?,
        }
    }
    (member.steam_id != 0).then_some(member)
}

/// Lit `CMsgClientMMSLobbyData` : `app_id` (1), `steam_id_lobby` (2, fixed64)
/// et `metadata` (12, répété de `MMSKeyValuePair`).
pub fn parse_lobby_data(body: &[u8]) -> Option<LobbyData> {
    let mut reader = Reader::new(body);
    let mut data = LobbyData::default();
    while reader.at < body.len() {
        let header = reader.varint()?;
        let field = (header >> 3) as u32;
        let wire = (header & 7) as u8;
        match (field, wire) {
            (1, 0) => data.app_id = reader.varint()? as u32,
            (2, 1) => data.lobby = reader.fixed64()?,
            (12, 2) => data.metadata.extend(parse_metadata_pairs(reader.bytes()?)),
            _ => reader.skip(wire)?,
        }
    }
    (data.app_id != 0 && data.lobby != 0).then_some(data)
}

fn parse_metadata_pairs(body: &[u8]) -> Vec<(String, String)> {
    let mut reader = Reader::new(body);
    let mut name = None;
    let mut value = None;
    while reader.at < body.len() {
        let Some(header) = reader.varint() else {
            return Vec::new();
        };
        let field = (header >> 3) as u32;
        let wire = (header & 7) as u8;
        match (field, wire) {
            (1, 2) => name = reader.string(),
            (2, 2) => value = reader.string(),
            _ => {
                if reader.skip(wire).is_none() {
                    return Vec::new();
                }
            }
        }
    }
    match (name, value) {
        (Some(name), Some(value)) => vec![(name, value)],
        _ => Vec::new(),
    }
}

/// Entrée ou sortie d'un membre dans un lobby (6619/6620).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LobbyMemberEvent {
    pub app_id: u32,
    pub lobby: u64,
    pub member: LobbyMember,
}

/// Lit `CMsgClientMMSUserJoinedLobby` (6619) et
/// `CMsgClientMMSUserLeftLobby` (6620), de même forme : `app_id` (1),
/// `steam_id_lobby` (2, fixed64), `steam_id_user` (3, fixed64) et
/// `persona_name` (4).
pub fn parse_member_event(body: &[u8]) -> Option<LobbyMemberEvent> {
    let mut reader = Reader::new(body);
    let mut event = LobbyMemberEvent::default();
    while reader.at < body.len() {
        let header = reader.varint()?;
        let field = (header >> 3) as u32;
        let wire = (header & 7) as u8;
        match (field, wire) {
            (1, 0) => event.app_id = reader.varint()? as u32,
            (2, 1) => event.lobby = reader.fixed64()?,
            (3, 1) => event.member.steam_id = reader.fixed64()?,
            (4, 2) => event.member.persona_name = reader.string()?,
            _ => reader.skip(wire)?,
        }
    }
    (event.lobby != 0 && event.member.steam_id != 0).then_some(event)
}

/// Corps de `CMsgClientMMSInviteToLobby` : `app_id` (1), `steam_id_lobby`
/// (2, fixed64), `steam_id_invited` (3, fixed64).
pub fn invite_to_lobby_body(app_id: u32, lobby: u64, invited: u64) -> Vec<u8> {
    let mut out = Vec::new();
    field_varint(1, u64::from(app_id), &mut out);
    field_fixed64(2, lobby, &mut out);
    field_fixed64(3, invited, &mut out);
    out
}

struct Reader<'a> {
    body: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn new(body: &'a [u8]) -> Self {
        Self { body, at: 0 }
    }

    fn varint(&mut self) -> Option<u64> {
        let mut value = 0u64;
        let mut shift = 0;
        loop {
            let byte = *self.body.get(self.at)?;
            self.at += 1;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Some(value);
            }
            shift += 7;
            if shift >= 64 {
                return None;
            }
        }
    }

    fn fixed64(&mut self) -> Option<u64> {
        let bytes = self.body.get(self.at..self.at + 8)?;
        self.at += 8;
        Some(u64::from_le_bytes(bytes.try_into().ok()?))
    }

    fn bytes(&mut self) -> Option<&'a [u8]> {
        let len = self.varint()? as usize;
        let bytes = self.body.get(self.at..self.at.checked_add(len)?)?;
        self.at += len;
        Some(bytes)
    }

    fn string(&mut self) -> Option<String> {
        Some(String::from_utf8_lossy(self.bytes()?).to_string())
    }

    fn skip(&mut self, wire: u8) -> Option<()> {
        match wire {
            0 => {
                self.varint()?;
            }
            1 => {
                self.at = self.at.checked_add(8)?;
            }
            2 => {
                let len = self.varint()? as usize;
                self.at = self.at.checked_add(len)?;
            }
            5 => {
                self.at = self.at.checked_add(4)?;
            }
            _ => return None,
        }
        (self.at <= self.body.len()).then_some(())
    }
}

/// Lit `CMsgClientMMSCreateLobbyResponse`.
pub fn parse_lobby_created(body: &[u8]) -> Option<LobbyCreated> {
    let mut reader = Reader::new(body);
    let mut created = LobbyCreated {
        app_id: 0,
        lobby: 0,
        eresult: 2,
    };
    while reader.at < body.len() {
        let header = reader.varint()?;
        let field = (header >> 3) as u32;
        let wire = (header & 7) as u8;
        match (field, wire) {
            (1, 0) => created.app_id = reader.varint()? as u32,
            (2, 1) => created.lobby = reader.fixed64()?,
            (3, 0) => created.eresult = reader.varint()? as i32,
            _ => reader.skip(wire)?,
        }
    }
    // Un refus n'a pas de lobby : le verdict (eresult) doit tout de même
    // remonter, sinon l'appelant ne voit qu'un délai dépassé.
    (!body.is_empty()).then_some(created)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_demande_porte_les_champs_du_protocole() {
        let body = create_lobby_body(
            480,
            4,
            2,
            1,
            7,
            Some([1, 2, 3, 4]),
            "test-user",
            &[("connect", "192.168.1.50:7777")],
        );
        // Champ 1 (app_id) en varint : clé 0x08, puis 480 = 0xE0 0x03.
        assert_eq!(&body[..3], &[0x08, 0xe0, 0x03]);
        // Champ 5 (cell_id) : clé 0x28 puis 7.
        assert!(body.windows(2).any(|w| w == [0x28, 0x07]));
        // Champ 9 (CMsgIPAddress.v4) : clé 0x4A, taille 5, clé 0x0D, IP LE.
        assert!(body
            .windows(7)
            .any(|w| w == [0x4a, 0x05, 0x0d, 0x01, 0x02, 0x03, 0x04]));
        assert!(body.windows(9).any(|w| w == b"test-user"));
    }

    #[test]
    fn la_reponse_rend_l_identifiant_du_lobby() {
        let mut body = Vec::new();
        field_varint(1, 480, &mut body);
        field_fixed64(2, 109775240000000001, &mut body);
        field_varint(3, 1, &mut body);
        let created = parse_lobby_created(&body).expect("réponse lisible");
        assert_eq!(created.app_id, 480);
        assert_eq!(created.lobby, 109775240000000001);
        assert_eq!(created.eresult, 1);
    }

    #[test]
    fn un_refus_remonte_son_eresult() {
        let mut body = Vec::new();
        field_varint(1, 480, &mut body);
        field_varint(3, 25, &mut body);
        let created = parse_lobby_created(&body).expect("la réponse doit être lue");
        assert_eq!(created.eresult, 25);
        assert_eq!(created.lobby, 0);
    }

    #[test]
    fn l_adresse_publiee_se_relit_dans_les_metadonnees() {
        // Création : le champ 11 porte la paire `connect`.
        let create = create_lobby_body(
            480,
            4,
            2,
            1,
            7,
            None,
            "test-user",
            &[("connect", "10.0.0.2:7777")],
        );
        let mut pair = Vec::new();
        field_bytes(1, b"connect", &mut pair);
        field_bytes(2, b"10.0.0.2:7777", &mut pair);
        let mut created_pair = Vec::new();
        field_bytes(11, &pair, &mut created_pair);
        assert!(
            create
                .windows(created_pair.len())
                .any(|w| w == created_pair),
            "la paire connect est au champ 11 de la création"
        );
        // Publication après coup : la même paire au champ 9.
        let set = set_lobby_data_body(480, 42, &[("connect", "10.0.0.2:7777")]);
        let mut set_pair = Vec::new();
        field_bytes(9, &pair, &mut set_pair);
        assert!(
            set.windows(set_pair.len()).any(|w| w == set_pair),
            "la paire connect est au champ 9 de la publication"
        );
        // Lecture de la réponse 6612 : la paire revient, et l'adresse aussi.
        let mut inner = Vec::new();
        field_bytes(1, b"connect", &mut inner);
        field_bytes(2, b"10.0.0.2:7777", &mut inner);
        let mut body = Vec::new();
        field_varint(1, 480, &mut body);
        field_fixed64(2, 42, &mut body);
        field_bytes(12, &inner, &mut body);
        let data = parse_lobby_data(&body).expect("métadonnées lisibles");
        assert_eq!(data.app_id, 480);
        assert_eq!(data.lobby, 42);
        assert_eq!(data.connect().as_deref(), Some("10.0.0.2:7777"));
    }

    #[test]
    fn la_rejointe_vise_le_lobby_recu_et_se_relit() {
        // Demande : l'appid, le lobby (fixed64) et le pseudo du joueur.
        let body = join_lobby_body(480, 109_775_240_000_000_001, "test-user", 7);
        assert_eq!(&body[..3], &[0x08, 0xe0, 0x03]);
        assert!(body.windows(1).any(|window| window == [0x11]));
        assert!(body.windows(9).any(|window| window == b"test-user"));
        assert!(body.windows(2).any(|window| window == [0x28, 0x07]));

        // Réponse acceptée : lobby effectif, membres et adresse publiée.
        let mut metadata = Vec::new();
        field_bytes(1, b"connect", &mut metadata);
        field_bytes(2, b"192.168.1.50:7777", &mut metadata);
        let mut member = Vec::new();
        field_fixed64(1, 76_561_198_000_000_001, &mut member);
        field_bytes(2, b"JoueurExemple", &mut member);
        let mut body = Vec::new();
        field_varint(1, 480, &mut body);
        field_fixed64(2, 109_775_240_000_000_001, &mut body);
        field_varint(3, 1, &mut body);
        field_varint(4, 4, &mut body);
        field_varint(5, 2, &mut body);
        field_varint(6, 1, &mut body);
        field_fixed64(7, 76_561_198_000_000_001, &mut body);
        field_bytes(9, &member, &mut body);
        field_bytes(10, &metadata, &mut body);
        let joined = parse_join_lobby_response(&body).expect("réponse lisible");
        assert_eq!(joined.app_id, 480);
        assert_eq!(joined.lobby, 109_775_240_000_000_001);
        assert_eq!(joined.eresult, 1);
        assert_eq!(joined.max_members, 4);
        assert_eq!(joined.owner, 76_561_198_000_000_001);
        assert_eq!(joined.members.len(), 1);
        assert_eq!(joined.members[0].persona_name, "JoueurExemple");
        assert_eq!(joined.connect().as_deref(), Some("192.168.1.50:7777"));

        // Refus : un eresult non nul, sans lobby enregistrable.
        let mut refused = Vec::new();
        field_varint(1, 480, &mut refused);
        field_varint(3, 2, &mut refused);
        let joined = parse_join_lobby_response(&refused).expect("le refus se lit");
        assert_eq!(joined.eresult, 2);
        assert_eq!(joined.lobby, 0);
        assert!(joined.connect().is_none());
    }

    /// Sink de test : il note ce que le worker remet au transport, sans
    /// réseau ni CM.
    #[derive(Default)]
    struct Enregistreur {
        envoyes: Vec<crate::social::JoinMessage>,
    }

    impl crate::social::JoinSink for Enregistreur {
        async fn send_join(&mut self, message: crate::social::JoinMessage) -> Result<(), String> {
            self.envoyes.push(message);
            Ok(())
        }
    }

    /// Le parcours demandé par la mission, sans CM : invitation reçue →
    /// acceptation → JoinLobby 6603 adressé au lobby de l'invitation →
    /// membres initialisés par 6604 → 6619/6620 qui tiennent la liste, y
    /// compris quand aucun 6604 n'a été traité.
    #[tokio::test]
    async fn le_parcours_invitation_join_et_membres_tient() {
        // 1. Octets bruts d'une invitation relayée par le CM (6621), traités
        // par le chemin de réception de production.
        let raw = invite_to_lobby_body(1500, 1501, 76_561_198_000_000_001);
        let received = crate::social::receive_invite(&raw).expect("invitation reçue");
        assert_eq!(received.app_id, 1500);
        assert_eq!(received.lobby, 1501);
        assert!(
            crate::social::received_invites()
                .iter()
                .any(|known| known.lobby == 1501),
            "l'invitation est retenue sous le bon lobby"
        );
        // Une invitation illisible n'est pas retenue.
        assert!(crate::social::receive_invite(&[0x08, 0x01]).is_none());
        // 2. Acceptation : le worker prépare et remet sa demande au transport.
        let mut sink = Enregistreur::default();
        crate::social::emit_join(&mut sink, 0, 1501, "test-user", 0)
            .await
            .expect("rejointe émise");
        assert_eq!(sink.envoyes.len(), 1, "une seule demande émise");
        let sent = &sink.envoyes[0];
        assert_eq!(sent.emsg, 6603);
        assert_eq!(sent.appid, 1500);
        let mut reader = Reader::new(&sent.body);
        assert_eq!(reader.varint(), Some(0x08));
        assert_eq!(reader.varint(), Some(1500));
        assert_eq!(reader.varint(), Some(0x11));
        assert_eq!(
            reader.fixed64(),
            Some(1501),
            "le lobby visé est celui de l'invitation"
        );
        // 3. Réponse 6604 : la liste des membres est initialisée.
        let mut response = Vec::new();
        field_varint(1, 1500, &mut response);
        field_fixed64(2, 1501, &mut response);
        field_varint(3, 1, &mut response);
        let mut member = Vec::new();
        field_fixed64(1, 11, &mut member);
        field_bytes(2, b"JoueurExemple", &mut member);
        field_bytes(9, &member, &mut response);
        let joined = parse_join_lobby_response(&response).expect("réponse de join lue");
        assert!(crate::social::record_join_result(&joined));
        assert_eq!(crate::social::lobby_members(1501).len(), 1);
        // 4. 6619 : un membre entre, la liste suit.
        let mut entered = Vec::new();
        field_varint(1, 1500, &mut entered);
        field_fixed64(2, 1501, &mut entered);
        field_fixed64(3, 12, &mut entered);
        field_bytes(4, b"Ami", &mut entered);
        let event = parse_member_event(&entered).expect("6619 lue");
        assert!(crate::social::apply_member_event(&event, true));
        assert_eq!(crate::social::lobby_members(1501).len(), 2);
        // 5. 6620 : il repart.
        let mut left = Vec::new();
        field_varint(1, 1500, &mut left);
        field_fixed64(2, 1501, &mut left);
        field_fixed64(3, 12, &mut left);
        field_bytes(4, b"Ami", &mut left);
        let event = parse_member_event(&left).expect("6620 lue");
        assert!(crate::social::apply_member_event(&event, false));
        assert_eq!(crate::social::lobby_members(1501).len(), 1);
        assert_eq!(
            crate::social::lobby_members(1501)[0].steam_id,
            11,
            "c'est bien le sortant qui disparaît"
        );
        // 6. Lobby rejoint sans 6604 : la demande part quand même, et la
        // première annonce crée la liste.
        let raw = invite_to_lobby_body(1500, 1503, 76_561_198_000_000_001);
        assert!(crate::social::receive_invite(&raw).is_some());
        let mut sink = Enregistreur::default();
        crate::social::emit_join(&mut sink, 0, 1503, "test-user", 0)
            .await
            .expect("rejointe émise");
        assert_eq!(sink.envoyes.len(), 1);
        assert_eq!(sink.envoyes[0].emsg, 6603);
        assert_eq!(sink.envoyes[0].appid, 1500);
        let mut reader = Reader::new(&sink.envoyes[0].body);
        assert_eq!(reader.varint(), Some(0x08));
        assert_eq!(reader.varint(), Some(1500));
        assert_eq!(reader.varint(), Some(0x11));
        assert_eq!(reader.fixed64(), Some(1503), "le lobby de l'invitation");
        let mut first = Vec::new();
        field_varint(1, 1500, &mut first);
        field_fixed64(2, 1503, &mut first);
        field_fixed64(3, 21, &mut first);
        field_bytes(4, b"Tardif", &mut first);
        let event = parse_member_event(&first).expect("6619 lue");
        assert!(crate::social::apply_member_event(&event, true));
        assert_eq!(crate::social::lobby_members(1503).len(), 1);
        assert_eq!(crate::social::lobby_members(1503)[0].persona_name, "Tardif");
    }

    #[test]
    fn une_notification_de_membre_nomme_le_lobby_et_le_membre() {
        let mut body = Vec::new();
        field_varint(1, 480, &mut body);
        field_fixed64(2, 42, &mut body);
        field_fixed64(3, 76_561_198_000_000_001, &mut body);
        field_bytes(4, b"JoueurExemple", &mut body);
        let event = parse_member_event(&body).expect("notification lisible");
        assert_eq!(event.app_id, 480);
        assert_eq!(event.lobby, 42);
        assert_eq!(event.member.steam_id, 76_561_198_000_000_001);
        assert_eq!(event.member.persona_name, "JoueurExemple");
        // Sans lobby ni membre, il n'y a rien à mettre à jour.
        let mut empty = Vec::new();
        field_varint(1, 480, &mut empty);
        assert!(parse_member_event(&empty).is_none());
    }

    #[test]
    fn une_invitation_recue_nomme_le_lobby_et_l_envoyeur() {
        let mut body = Vec::new();
        field_varint(1, 4670920, &mut body);
        field_fixed64(2, 109775240000000001, &mut body);
        field_fixed64(3, 76561198000000001, &mut body);
        let invite = parse_invite(&body).expect("invitation lisible");
        assert_eq!(invite.app_id, 4670920);
        assert_eq!(invite.lobby, 109775240000000001);
        assert_eq!(invite.inviter, 76561198000000001);
        // Un corps sans lobby n'est pas une invitation.
        assert!(parse_invite(&[0x08, 0xc8, 0x8b, 0x9d, 0x02]).is_none());
    }

    #[test]
    fn l_invitation_nomme_le_lobby_et_l_invite() {
        let body = invite_to_lobby_body(480, 42, 76561198000000000);
        let mut reader = Reader::new(&body);
        assert_eq!(reader.varint(), Some(0x08));
        assert_eq!(reader.varint(), Some(480));
        assert_eq!(reader.varint(), Some(0x11));
        assert_eq!(reader.fixed64(), Some(42));
        assert_eq!(reader.varint(), Some(0x19));
        assert_eq!(reader.fixed64(), Some(76561198000000000));
    }
}
