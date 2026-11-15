//! Encodage protobuf minimal pour les quelques messages du client CM.
//!
//! Générer les `.proto` de Steam (et dépendre de `protoc`) serait
//! disproportionné pour quatre champs. Ce module couvre les types réellement
//! utilisés : varint, 64 bits, 32 bits et longueur.

use super::{Error, License};
use flate2::read::GzDecoder;
use sha1::{Digest, Sha1};
use std::io::Read;

/// Version de protocole annoncée par `ClientLogon` (`MsgClientLogon.CurrentProtocol`).
pub const PROTOCOL_VERSION: u32 = 65580;
/// Version de paquet client utilisée par les clients actuels.
pub const CLIENT_PACKAGE_VERSION: u32 = 1_561_159_470;
/// Version annoncée avec un logon par jeton, comme les clients récents.
pub const AUTH_CLIENT_PACKAGE_VERSION: u32 = 1771;
/// `k_EMsgClientHello` : annonce du protocole juste après le canal chiffré.
pub const CLIENT_HELLO: u32 = 9805;
/// `k_EMsgServiceMethodCallFromClientNonAuthed` : appel de service avant logon.
pub const SERVICE_METHOD_CALL_FROM_CLIENT_NON_AUTHED: u32 = 9804;
/// `k_EMsgServiceMethodResponse`.
pub const SERVICE_METHOD_RESPONSE: u32 = 147;

/// Méthodes du service `Authentication` (le suffixe est la version du service).
pub const AUTH_GET_PASSWORD_KEY: &str = "Authentication.GetPasswordRSAPublicKey#1";
pub const AUTH_BEGIN_CREDENTIALS: &str = "Authentication.BeginAuthSessionViaCredentials#1";
pub const AUTH_UPDATE_GUARD_CODE: &str = "Authentication.UpdateAuthSessionWithSteamGuardCode#1";
pub const AUTH_POLL_SESSION: &str = "Authentication.PollAuthSessionStatus#1";
pub const AUTH_BEGIN_QR: &str = "Authentication.BeginAuthSessionViaQR#1";

/// `EAuthTokenPlatformType::k_EAuthTokenPlatformType_SteamClient`.
pub const AUTH_PLATFORM_STEAM_CLIENT: u32 = 1;
/// `ESessionPersistence::k_ESessionPersistence_Persistent`.
pub const AUTH_PERSISTENT: u32 = 1;
/// `EAuthSessionGuardType`.
pub const GUARD_NONE: u32 = 1;
pub const GUARD_EMAIL_CODE: u32 = 2;
pub const GUARD_DEVICE_CODE: u32 = 3;
pub const GUARD_DEVICE_CONFIRMATION: u32 = 4;
pub const GUARD_EMAIL_CONFIRMATION: u32 = 5;
/// Taille maximale d'un `Multi` décompressé : un CM ne l'approche jamais.
const MAX_MULTI: usize = 4 << 20;

/// Identifiant anonyme (`EAccountType::AnonUser`, univers Public, compte 0).
pub const ANON_STEAM_ID: u64 = (1 << 56) | (10 << 52);
/// Identifiant individuel de remplacement avant que le CM ne rende le vrai
/// (`EAccountType::Individual`, instance Desktop, compte 0).
pub const INDIVIDUAL_STEAM_ID: u64 = (1 << 56) | (1 << 52) | (1 << 32);
/// `EOSType::LinuxUnknown`, la valeur qu'annonce un client Linux.
pub const LINUX_OS_TYPE: u32 = -203i32 as u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field<'a> {
    Varint(u64),
    Fixed64(u64),
    Fixed32(u32),
    Bytes(&'a [u8]),
}

/// Lecteur séquentiel de champs protobuf.
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn varint(&mut self) -> Option<u64> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = *self.data.get(self.pos)?;
            self.pos += 1;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Some(value);
            }
        }
        None
    }

    /// Champ suivant, ou `None` en fin de message (ou si le message est
    /// tronqué : un client ne doit rien interpréter au-delà).
    pub fn next(&mut self) -> Option<(u32, Field<'a>)> {
        if self.pos >= self.data.len() {
            return None;
        }
        let key = self.varint()?;
        let number = (key >> 3) as u32;
        let field = match key & 7 {
            0 => Field::Varint(self.varint()?),
            1 => {
                let end = self.pos.checked_add(8)?;
                let raw = self.data.get(self.pos..end)?;
                self.pos = end;
                Field::Fixed64(u64::from_le_bytes(raw.try_into().ok()?))
            }
            2 => {
                let len = usize::try_from(self.varint()?).ok()?;
                let end = self.pos.checked_add(len)?;
                let raw = self.data.get(self.pos..end)?;
                self.pos = end;
                Field::Bytes(raw)
            }
            5 => {
                let end = self.pos.checked_add(4)?;
                let raw = self.data.get(self.pos..end)?;
                self.pos = end;
                Field::Fixed32(u32::from_le_bytes(raw.try_into().ok()?))
            }
            _ => return None,
        };
        Some((number, field))
    }
}

pub fn varint(mut value: u64, out: &mut Vec<u8>) {
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

pub fn field_varint(field: u32, value: u64, out: &mut Vec<u8>) {
    varint(u64::from(field) << 3, out);
    varint(value, out);
}

pub fn field_bytes(field: u32, bytes: &[u8], out: &mut Vec<u8>) {
    varint((u64::from(field) << 3) | 2, out);
    varint(bytes.len() as u64, out);
    out.extend_from_slice(bytes);
}

pub fn field_fixed64(field: u32, value: u64, out: &mut Vec<u8>) {
    varint((u64::from(field) << 3) | 1, out);
    out.extend_from_slice(&value.to_le_bytes());
}

pub fn field_fixed32(field: u32, value: u32, out: &mut Vec<u8>) {
    varint((u64::from(field) << 3) | 5, out);
    out.extend_from_slice(&value.to_le_bytes());
}

/// En-tête `CMsgProtoBufHeader` : SteamID, puis identifiant de session.
pub fn proto_header(steam_id: u64, session_id: i32) -> Vec<u8> {
    let mut out = Vec::with_capacity(16);
    field_fixed64(1, steam_id, &mut out);
    if session_id != 0 {
        field_varint(2, session_id as u64, &mut out);
    }
    out
}

/// Corps de `ClientHello` : la version de protocole annoncée au CM.
pub fn client_hello() -> Vec<u8> {
    let mut out = Vec::new();
    field_varint(1, u64::from(PROTOCOL_VERSION), &mut out);
    out
}

/// Corps de `ClientLogon` pour l'accès anonyme (`steamcmd +login anonymous`).
pub fn anonymous_logon() -> Vec<u8> {
    let mut out = Vec::with_capacity(16);
    field_varint(1, u64::from(PROTOCOL_VERSION), &mut out);
    field_varint(5, u64::from(CLIENT_PACKAGE_VERSION), &mut out);
    out
}

/// Identifiant machine au format attendu par Steam : un objet KeyValues
/// binaire `MessageObject` avec les empreintes `BB3`, `FF2` et `3B3`.
///
/// Le client officiel et steam-user envoient ce format ; un simple hachage
/// brut n'est pas un objet machine valide.
pub fn machine_id() -> Vec<u8> {
    let identity = std::fs::read_to_string("/etc/machine-id")
        .map(|value| value.trim().to_string())
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_else(|| "lapinux".to_string());
    let host = std::env::var("HOSTNAME").unwrap_or_else(|_| "lapinux".to_string());
    let disk = format!("{identity}/{host}");
    let mut out = Vec::with_capacity(155);
    out.push(0);
    push_cstr(&mut out, "MessageObject");
    for (key, value) in [
        ("BB3", identity.as_str()),
        ("FF2", host.as_str()),
        ("3B3", disk.as_str()),
    ] {
        out.push(1);
        push_cstr(&mut out, key);
        push_cstr(&mut out, &hex_sha1(value));
    }
    out.extend_from_slice(&[8, 8]);
    out
}

fn push_cstr(out: &mut Vec<u8>, value: &str) {
    out.extend_from_slice(value.as_bytes());
    out.push(0);
}

fn hex_sha1(value: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(value.as_bytes());
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(40);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

/// Identifiants d'un logon par jeton (`access_token` = jeton de refresh obtenu
/// par le service d'authentification).
pub struct TokenLogon<'a> {
    pub refresh_token: &'a str,
}

/// Corps de `ClientLogon` pour un compte nominatif connecté par jeton.
///
/// Les numéros suivent `CMsgClientLogon` : 30 identifiant machine,
/// 96 nom de machine, 102 réponse de rate-limit, 108 jeton d'accès.
/// Le champ 50 (`account_name`) est volontairement absent :
/// les clients qui se connectent par jeton de refresh l'interdisent, le CM
/// retrouve le compte dans le jeton. L'en-tête du message porte le SteamID du
/// jeton (voir [`refresh_token_steam_id`]).
pub fn token_logon(request: &TokenLogon<'_>) -> Vec<u8> {
    let mut out = Vec::new();
    field_varint(1, u64::from(PROTOCOL_VERSION), &mut out);
    field_varint(5, u64::from(AUTH_CLIENT_PACKAGE_VERSION), &mut out);
    field_bytes(6, b"english", &mut out);
    field_varint(7, u64::from(LINUX_OS_TYPE), &mut out);
    field_varint(8, 1, &mut out);
    field_bytes(30, &machine_id(), &mut out);
    field_bytes(96, b"lapinux", &mut out);
    field_varint(102, 1, &mut out);
    field_bytes(108, request.refresh_token.as_bytes(), &mut out);
    out
}

/// SteamID porté par le champ `sub` d'un jeton de refresh.
///
/// Le jeton est un JWT `header.payload.signature` ; seul le `sub` du payload
/// est lu. Le CM compare l'en-tête du `ClientLogon` à ce SteamID : envoyer
/// l'identifiant factice d'avant-login fait refuser le jeton (`eresult` 5).
pub fn refresh_token_steam_id(token: &str) -> Option<u64> {
    let payload = token.split('.').nth(1)?;
    let bytes = base64_url_decode(payload)?;
    let claims: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    match claims.get("sub")? {
        serde_json::Value::String(text) => text.parse().ok(),
        serde_json::Value::Number(number) => number.as_u64(),
        _ => None,
    }
}

/// Décode un base64 URL sans remplissage (format des segments JWT).
fn base64_url_decode(input: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut buffer = 0u32;
    let mut bits = 0u32;
    for byte in input.bytes() {
        let digit = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => continue,
            _ => return None,
        } as u32;
        buffer = (buffer << 6) | digit;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// En-tête d'un appel de service unifié : `jobid_source` et nom de méthode.
pub fn service_header(job_id: u64, method: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(32 + method.len());
    field_fixed64(10, job_id, &mut out);
    field_bytes(12, method.as_bytes(), &mut out);
    out
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ServiceHeader {
    pub target_job_id: u64,
    pub eresult: Option<i32>,
    pub error_message: Option<String>,
    pub target_job_name: Option<String>,
}

/// Décode les champs utiles de `CMsgProtoBufHeader` dans une réponse.
pub fn parse_service_header(header: &[u8]) -> ServiceHeader {
    let mut parsed = ServiceHeader::default();
    let mut reader = Reader::new(header);
    while let Some((number, field)) = reader.next() {
        match (number, field) {
            (11, Field::Fixed64(job)) => parsed.target_job_id = job,
            (12, Field::Bytes(name)) => {
                parsed.target_job_name = String::from_utf8(name.to_vec()).ok()
            }
            (13, Field::Varint(result)) => parsed.eresult = Some(result as i32),
            (14, Field::Bytes(message)) => {
                parsed.error_message = String::from_utf8(message.to_vec()).ok()
            }
            _ => {}
        }
    }
    parsed
}

pub fn get_password_rsa_key_request(account_name: &str) -> Vec<u8> {
    let mut out = Vec::new();
    field_bytes(1, account_name.as_bytes(), &mut out);
    out
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PasswordRsaKey {
    pub modulus_hex: String,
    pub exponent_hex: String,
    pub timestamp: u64,
}

pub fn parse_password_rsa_key(body: &[u8]) -> PasswordRsaKey {
    let mut key = PasswordRsaKey::default();
    let mut reader = Reader::new(body);
    while let Some((number, field)) = reader.next() {
        match (number, field) {
            (1, Field::Bytes(modulus)) => {
                key.modulus_hex = String::from_utf8(modulus.to_vec()).unwrap_or_default()
            }
            (2, Field::Bytes(exponent)) => {
                key.exponent_hex = String::from_utf8(exponent.to_vec()).unwrap_or_default()
            }
            (3, Field::Varint(timestamp)) => key.timestamp = timestamp,
            _ => {}
        }
    }
    key
}

/// Détails de l'appareil joints à `BeginAuthSessionViaCredentials`.
pub struct AuthDevice<'a> {
    pub friendly_name: &'a str,
    pub os_type: i32,
}

pub struct BeginAuthRequest<'a> {
    pub account_name: &'a str,
    pub encrypted_password: &'a str,
    pub timestamp: u64,
    pub persistent: bool,
    pub guard_data: Option<&'a str>,
    pub device: AuthDevice<'a>,
}

/// Corps de `BeginAuthSessionViaCredentials`.
pub fn begin_auth_request(request: &BeginAuthRequest<'_>) -> Vec<u8> {
    let mut device = Vec::new();
    field_bytes(1, request.device.friendly_name.as_bytes(), &mut device);
    field_varint(2, u64::from(AUTH_PLATFORM_STEAM_CLIENT), &mut device);
    field_varint(3, u64::from(request.device.os_type as u32), &mut device);

    let mut out = Vec::new();
    field_bytes(2, request.account_name.as_bytes(), &mut out);
    field_bytes(3, request.encrypted_password.as_bytes(), &mut out);
    field_varint(4, request.timestamp, &mut out);
    field_varint(5, 1, &mut out);
    field_varint(6, u64::from(AUTH_PLATFORM_STEAM_CLIENT), &mut out);
    field_varint(7, if request.persistent { 1 } else { 0 }, &mut out);
    field_bytes(8, b"Client", &mut out);
    field_bytes(9, &device, &mut out);
    if let Some(guard) = request.guard_data.filter(|value| !value.is_empty()) {
        field_bytes(10, guard.as_bytes(), &mut out);
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthConfirmation {
    pub kind: u32,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BeginAuthResponse {
    pub client_id: u64,
    pub request_id: Vec<u8>,
    pub interval: f32,
    pub confirmations: Vec<AuthConfirmation>,
    pub steam_id: u64,
    pub extended_error: Option<String>,
}

impl Default for BeginAuthResponse {
    fn default() -> Self {
        Self {
            client_id: 0,
            request_id: Vec::new(),
            interval: 0.0,
            confirmations: Vec::new(),
            steam_id: 0,
            extended_error: None,
        }
    }
}

pub fn parse_begin_auth(body: &[u8]) -> BeginAuthResponse {
    let mut response = BeginAuthResponse::default();
    let mut reader = Reader::new(body);
    while let Some((number, field)) = reader.next() {
        match (number, field) {
            (1, Field::Varint(client_id)) => response.client_id = client_id,
            (2, Field::Bytes(request_id)) => response.request_id = request_id.to_vec(),
            (3, Field::Fixed32(bits)) => response.interval = f32::from_bits(bits),
            (4, Field::Bytes(entry)) => {
                let mut confirmation = AuthConfirmation {
                    kind: 0,
                    message: None,
                };
                let mut entry = Reader::new(entry);
                while let Some((number, field)) = entry.next() {
                    match (number, field) {
                        (1, Field::Varint(kind)) => confirmation.kind = kind as u32,
                        (2, Field::Bytes(message)) => {
                            confirmation.message = String::from_utf8(message.to_vec()).ok()
                        }
                        _ => {}
                    }
                }
                response.confirmations.push(confirmation);
            }
            (5, Field::Fixed64(steam_id)) => response.steam_id = steam_id,
            (8, Field::Bytes(message)) => {
                response.extended_error = String::from_utf8(message.to_vec()).ok()
            }
            _ => {}
        }
    }
    response
}

/// Corps de `BeginAuthSessionViaQR` : le nom de l'appareil et ses détails,
/// comme pour une connexion par mot de passe.
pub fn begin_qr_request(device_friendly_name: &str) -> Vec<u8> {
    let mut device = Vec::new();
    field_bytes(1, device_friendly_name.as_bytes(), &mut device);
    field_varint(2, u64::from(AUTH_PLATFORM_STEAM_CLIENT), &mut device);
    field_varint(3, u64::from(LINUX_OS_TYPE), &mut device);

    let mut out = Vec::new();
    field_bytes(1, device_friendly_name.as_bytes(), &mut out);
    field_varint(2, u64::from(AUTH_PLATFORM_STEAM_CLIENT), &mut out);
    field_bytes(3, &device, &mut out);
    out
}

/// Réponse de `BeginAuthSessionViaQR` : l'URL à afficher et la session à
/// interroger jusqu'à ce que le téléphone l'approuve.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BeginQrResponse {
    pub client_id: u64,
    pub challenge_url: String,
    pub request_id: Vec<u8>,
    pub interval: f32,
}

pub fn parse_begin_qr(body: &[u8]) -> BeginQrResponse {
    let mut response = BeginQrResponse::default();
    let mut reader = Reader::new(body);
    while let Some((number, field)) = reader.next() {
        match (number, field) {
            (1, Field::Varint(client_id)) => response.client_id = client_id,
            (2, Field::Bytes(url)) => {
                response.challenge_url = String::from_utf8_lossy(url).into_owned()
            }
            (3, Field::Bytes(request_id)) => response.request_id = request_id.to_vec(),
            (4, Field::Fixed32(bits)) => response.interval = f32::from_bits(bits),
            _ => {}
        }
    }
    response
}

pub fn update_guard_code_request(
    client_id: u64,
    steam_id: u64,
    code: &str,
    code_type: u32,
) -> Vec<u8> {
    let mut out = Vec::new();
    field_varint(1, client_id, &mut out);
    field_fixed64(2, steam_id, &mut out);
    field_bytes(3, code.as_bytes(), &mut out);
    field_varint(4, u64::from(code_type), &mut out);
    out
}

pub fn poll_auth_request(client_id: u64, request_id: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    field_varint(1, client_id, &mut out);
    field_bytes(2, request_id, &mut out);
    out
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PollAuthResponse {
    pub new_client_id: u64,
    /// Nouvelle URL de QR quand Steam fait tourner le défi.
    pub new_challenge_url: Option<String>,
    pub refresh_token: Option<String>,
    pub access_token: Option<String>,
    pub account_name: Option<String>,
    pub new_guard_data: Option<String>,
}

pub fn parse_poll_auth(body: &[u8]) -> PollAuthResponse {
    let mut response = PollAuthResponse::default();
    let mut reader = Reader::new(body);
    while let Some((number, field)) = reader.next() {
        match (number, field) {
            (1, Field::Varint(client_id)) => response.new_client_id = client_id,
            (2, Field::Bytes(url)) => {
                response.new_challenge_url = String::from_utf8(url.to_vec()).ok()
            }
            (3, Field::Bytes(token)) => {
                response.refresh_token = String::from_utf8(token.to_vec()).ok()
            }
            (4, Field::Bytes(token)) => {
                response.access_token = String::from_utf8(token.to_vec()).ok()
            }
            (6, Field::Bytes(name)) => {
                response.account_name = String::from_utf8(name.to_vec()).ok()
            }
            (7, Field::Bytes(guard)) => {
                response.new_guard_data = String::from_utf8(guard.to_vec()).ok()
            }
            _ => {}
        }
    }
    response
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogOnResponse {
    pub steam_id: u64,
    pub session_id: i32,
    pub eresult: i32,
    pub heartbeat_seconds: Option<u32>,
    pub cell_id: Option<u32>,
    pub country: Option<String>,
    /// `eresult_extended` : motif détaillé quand Steam en fournit un.
    pub eresult_extended: Option<i32>,
}

/// Décode un `ClientLogOnResponse` : SteamID et session dans l'en-tête,
/// résultat et cellules dans le corps.
pub fn parse_logon_response(header: &[u8], body: &[u8]) -> LogOnResponse {
    let mut response = LogOnResponse::default();
    let mut reader = Reader::new(header);
    while let Some((number, field)) = reader.next() {
        match (number, field) {
            (1, Field::Fixed64(id)) => response.steam_id = id,
            (2, Field::Varint(session)) => response.session_id = session as i32,
            _ => {}
        }
    }
    let mut reader = Reader::new(body);
    while let Some((number, field)) = reader.next() {
        match (number, field) {
            (1, Field::Varint(result)) => response.eresult = result as i32,
            (10, Field::Varint(code)) => response.eresult_extended = Some(code as i32),
            (3, Field::Varint(seconds)) => {
                response.heartbeat_seconds = Some(seconds as u32);
            }
            (7, Field::Varint(cell)) => response.cell_id = Some(cell as u32),
            (16, Field::Bytes(country)) => {
                response.country = String::from_utf8(country.to_vec()).ok();
            }
            _ => {}
        }
    }
    response
}

/// Extrait les licences de `CMsgClientLicenseList` : paquet et propriétaire.
pub fn parse_license_list(body: &[u8]) -> Vec<License> {
    let mut licenses = Vec::new();
    let mut reader = Reader::new(body);
    while let Some((number, field)) = reader.next() {
        if number != 2 {
            continue;
        }
        let Field::Bytes(entry) = field else {
            continue;
        };
        let mut entry = Reader::new(entry);
        let mut package_id = None;
        let mut owner_id = 0;
        let mut time_created = 0;
        let mut payment_method = 0;
        while let Some((number, field)) = entry.next() {
            match (number, field) {
                (1, Field::Varint(id)) => package_id = Some(id as u32),
                // Date d'ajout de la licence : c'est elle qui date un achat.
                // Le CM l'écrit en `fixed32`, pas en varint.
                (2, Field::Fixed32(created)) => time_created = created as i64,
                (2, Field::Varint(created)) => time_created = created as i64,
                (6, Field::Varint(method)) => payment_method = method as u32,
                (12, Field::Varint(owner)) => owner_id = owner as u32,
                _ => {}
            }
        }
        if let Some(package_id) = package_id {
            licenses.push(License {
                package_id,
                owner_id,
                time_created,
                payment_method,
            });
        }
    }
    licenses
}

/// Déballe `CMsgMulti`, éventuellement compressé en gzip.
pub fn split_multi(body: &[u8]) -> Result<Vec<Vec<u8>>, Error> {
    let mut declared = None;
    let mut payload = None;
    let mut reader = Reader::new(body);
    while let Some((number, field)) = reader.next() {
        match (number, field) {
            (1, Field::Varint(size)) => declared = Some(size as usize),
            (2, Field::Bytes(bytes)) => payload = Some(bytes),
            _ => {}
        }
    }
    let payload = payload.ok_or_else(|| Error::Protocol("Multi sans charge utile".into()))?;
    let data = match declared.unwrap_or(0) {
        0 => payload.to_vec(),
        size if size > MAX_MULTI => {
            return Err(Error::Protocol(format!(
                "Multi de {size} octets refusé (limite {MAX_MULTI})"
            )))
        }
        size => {
            let decoder = GzDecoder::new(payload);
            let mut data = Vec::with_capacity(size.min(MAX_MULTI));
            decoder
                .take(MAX_MULTI as u64 + 1)
                .read_to_end(&mut data)
                .map_err(|e| Error::Protocol(format!("Multi illisible : {e}")))?;
            if data.len() != size {
                return Err(Error::Protocol(format!(
                    "Multi décompressé : {} octets au lieu de {size}",
                    data.len()
                )));
            }
            data
        }
    };
    let mut messages = Vec::new();
    let mut offset = 0;
    while offset < data.len() {
        let Some(head) = data.get(offset..offset + 4) else {
            return Err(Error::Protocol("Multi tronqué".into()));
        };
        let len = u32::from_le_bytes(
            head.try_into()
                .map_err(|_| Error::Protocol("Multi tronqué".into()))?,
        ) as usize;
        let start = offset + 4;
        let end = start + len;
        let message = data
            .get(start..end)
            .ok_or_else(|| Error::Protocol("Multi tronqué".into()))?;
        messages.push(message.to_vec());
        offset = end;
    }
    Ok(messages)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varint_round_trip() {
        for value in [0, 1, 127, 128, 300, 65_580, u64::from(u32::MAX), u64::MAX] {
            let mut buffer = Vec::new();
            varint(value, &mut buffer);
            let mut reader = Reader::new(&buffer);
            assert_eq!(reader.varint(), Some(value));
        }
    }

    #[test]
    fn header_carries_steam_id_and_session() {
        let header = proto_header(ANON_STEAM_ID, 42);
        let mut reader = Reader::new(&header);
        let mut steam_id = None;
        let mut session = None;
        while let Some((number, field)) = reader.next() {
            match (number, field) {
                (1, Field::Fixed64(value)) => steam_id = Some(value),
                (2, Field::Varint(value)) => session = Some(value as i32),
                _ => {}
            }
        }
        assert_eq!(steam_id, Some(ANON_STEAM_ID));
        assert_eq!(session, Some(42));
    }

    #[test]
    fn logon_body_is_minimal_and_stable() {
        let body = anonymous_logon();
        let mut reader = Reader::new(&body);
        let mut fields = Vec::new();
        while let Some((number, field)) = reader.next() {
            fields.push((number, field));
        }
        assert_eq!(
            fields,
            vec![
                (1, Field::Varint(u64::from(PROTOCOL_VERSION))),
                (5, Field::Varint(u64::from(CLIENT_PACKAGE_VERSION))),
            ]
        );
    }

    fn fields(body: &[u8]) -> Vec<(u32, Field<'_>)> {
        let mut reader = Reader::new(body);
        let mut out = Vec::new();
        while let Some((number, field)) = reader.next() {
            out.push((number, field));
        }
        out
    }

    #[test]
    fn hello_announces_the_protocol_version() {
        assert_eq!(
            fields(&client_hello()),
            vec![(1, Field::Varint(u64::from(PROTOCOL_VERSION)))]
        );
    }

    #[test]
    fn token_logon_carries_only_the_refresh_token_as_credential() {
        // Les clients qui se connectent par jeton de refresh n'envoient ni
        // compte ni mot de passe : le CM lit l'identité dans le jeton, et
        // l'en-tête porte son SteamID (voir `refresh_token_steam_id`).
        let body = token_logon(&TokenLogon {
            refresh_token: "refresh-token",
        });
        let fields = fields(&body);
        assert!(!fields.iter().any(|(number, _)| *number == 50));
        assert!(fields.contains(&(108, Field::Bytes(b"refresh-token"))));
        assert!(fields.iter().any(|(number, _)| *number == 30));
        assert!(!fields.iter().any(|(number, _)| *number == 84));
        assert!(!fields.iter().any(|(number, _)| *number == 101));
    }

    #[test]
    fn refresh_token_steam_id_comes_from_the_payload() {
        // JWT header.payload.signature ; payload :
        // {"iss":"steam","sub":"76561198000000000","aud":["client","web"]}
        let token = "e30.eyJpc3MiOiJzdGVhbSIsInN1YiI6Ijc2NTYxMTk4MDAwMDAwMDAwIiwiYXVkIjpbImNsaWVudCIsIndlYiJdfQ.c2ln";
        assert_eq!(refresh_token_steam_id(token), Some(76_561_198_000_000_000));
        assert_eq!(refresh_token_steam_id("pas-un-jeton"), None);
        assert_eq!(refresh_token_steam_id("e30.e30.c2ln"), None);
    }

    #[test]
    fn auth_requests_carry_the_expected_fields() {
        assert_eq!(
            fields(&get_password_rsa_key_request("alice")),
            vec![(1, Field::Bytes(b"alice"))]
        );

        let begin = begin_auth_request(&BeginAuthRequest {
            account_name: "alice",
            encrypted_password: "c2VjcmV0",
            timestamp: 1_700_000_000,
            persistent: true,
            guard_data: Some("guard"),
            device: AuthDevice {
                friendly_name: lapinux_core::APP_NAME,
                os_type: LINUX_OS_TYPE as i32,
            },
        });
        let parsed = fields(&begin);
        assert!(parsed.contains(&(2, Field::Bytes(b"alice"))));
        assert!(parsed.contains(&(3, Field::Bytes(b"c2VjcmV0"))));
        assert!(parsed.contains(&(4, Field::Varint(1_700_000_000))));
        assert!(parsed.contains(&(7, Field::Varint(1))));
        assert!(parsed.contains(&(8, Field::Bytes(b"Client"))));
        assert!(parsed.contains(&(10, Field::Bytes(b"guard"))));

        let code = update_guard_code_request(7, 42, "ABC123", GUARD_EMAIL_CODE);
        assert!(fields(&code).contains(&(3, Field::Bytes(b"ABC123"))));
        assert!(fields(&code).contains(&(4, Field::Varint(2))));

        let poll = poll_auth_request(7, b"request");
        assert!(fields(&poll).contains(&(2, Field::Bytes(b"request"))));
    }

    #[test]
    fn parses_the_auth_responses() {
        let key = PasswordRsaKey {
            modulus_hex: "abcdef".into(),
            exponent_hex: "010001".into(),
            timestamp: 99,
        };
        let mut body = Vec::new();
        field_bytes(1, key.modulus_hex.as_bytes(), &mut body);
        field_bytes(2, key.exponent_hex.as_bytes(), &mut body);
        field_varint(3, key.timestamp, &mut body);
        assert_eq!(parse_password_rsa_key(&body), key);

        let mut confirmation = Vec::new();
        field_varint(1, u64::from(GUARD_EMAIL_CODE), &mut confirmation);
        field_bytes(2, b"code envoye", &mut confirmation);
        let mut begin = Vec::new();
        field_varint(1, 1234, &mut begin);
        field_bytes(2, b"request", &mut begin);
        field_fixed32(3, 5.0f32.to_bits(), &mut begin);
        field_bytes(4, &confirmation, &mut begin);
        field_fixed64(5, 7_653_000_000_000_000_000, &mut begin);
        let parsed = parse_begin_auth(&begin);
        assert_eq!(parsed.client_id, 1234);
        assert_eq!(parsed.request_id, b"request");
        assert_eq!(parsed.interval, 5.0);
        assert_eq!(
            parsed.confirmations,
            vec![AuthConfirmation {
                kind: GUARD_EMAIL_CODE,
                message: Some("code envoye".into()),
            }]
        );
        assert_eq!(parsed.steam_id, 7_653_000_000_000_000_000);

        let mut poll = Vec::new();
        field_bytes(2, b"https://s.team/q/1/next", &mut poll);
        field_bytes(3, b"refresh", &mut poll);
        field_bytes(4, b"access", &mut poll);
        field_bytes(6, b"alice", &mut poll);
        field_bytes(7, b"guard", &mut poll);
        let parsed = parse_poll_auth(&poll);
        assert_eq!(
            parsed.new_challenge_url.as_deref(),
            Some("https://s.team/q/1/next")
        );
        assert_eq!(parsed.refresh_token.as_deref(), Some("refresh"));
        assert_eq!(parsed.access_token.as_deref(), Some("access"));
        assert_eq!(parsed.account_name.as_deref(), Some("alice"));
        assert_eq!(parsed.new_guard_data.as_deref(), Some("guard"));
    }

    #[test]
    fn qr_messages_round_trip() {
        let qr = begin_qr_request(lapinux_core::APP_NAME);
        let parsed = fields(&qr);
        assert!(parsed.contains(&(1, Field::Bytes(lapinux_core::APP_NAME.as_bytes()))));
        assert!(parsed.contains(&(2, Field::Varint(u64::from(AUTH_PLATFORM_STEAM_CLIENT)))));

        let mut body = Vec::new();
        field_varint(1, 42, &mut body);
        field_bytes(2, b"https://s.team/q/1/42", &mut body);
        field_bytes(3, b"request", &mut body);
        field_fixed32(4, 3.0f32.to_bits(), &mut body);
        let begin = parse_begin_qr(&body);
        assert_eq!(begin.client_id, 42);
        assert_eq!(begin.challenge_url, "https://s.team/q/1/42");
        assert_eq!(begin.request_id, b"request");
        assert_eq!(begin.interval, 3.0);
    }

    #[test]
    fn service_header_round_trip() {
        let header = service_header(42, AUTH_GET_PASSWORD_KEY);
        let parsed = parse_service_header(&header);
        assert_eq!(parsed.target_job_id, 0);
        assert!(header.len() > 20);
        let mut response = Vec::new();
        field_fixed64(11, 42, &mut response);
        field_bytes(12, AUTH_GET_PASSWORD_KEY.as_bytes(), &mut response);
        field_varint(13, 1, &mut response);
        let parsed = parse_service_header(&response);
        assert_eq!(parsed.target_job_id, 42);
        assert_eq!(parsed.eresult, Some(1));
        assert_eq!(
            parsed.target_job_name.as_deref(),
            Some(AUTH_GET_PASSWORD_KEY)
        );
    }

    #[test]
    fn individual_steam_id_has_the_individual_type() {
        assert_eq!(INDIVIDUAL_STEAM_ID, 76_561_197_960_265_728);
        assert_eq!(INDIVIDUAL_STEAM_ID >> 52 & 0xf, 1);
        assert_eq!(INDIVIDUAL_STEAM_ID >> 32 & 0xfffff, 1);
    }

    #[test]
    fn machine_id_is_a_stable_valve_keyvalues_object() {
        let id = machine_id();
        assert_eq!(id.len(), 155);
        assert!(id.starts_with(b"\0MessageObject\0"));
        assert!(id.ends_with(&[8, 8]));
        assert_eq!(id, machine_id());
    }

    #[test]
    fn parses_a_logon_response() {
        let header = proto_header(7_653_000_000_000_000_000, 7);
        let mut body = Vec::new();
        field_varint(1, 1, &mut body);
        field_varint(10, 42, &mut body);
        field_varint(3, 30, &mut body);
        field_varint(7, 14, &mut body);
        field_bytes(16, b"fr", &mut body);
        let response = parse_logon_response(&header, &body);
        assert_eq!(response.steam_id, 7_653_000_000_000_000_000);
        assert_eq!(response.session_id, 7);
        assert_eq!(response.eresult_extended, Some(42));
        assert_eq!(response.eresult, 1);
        assert_eq!(response.heartbeat_seconds, Some(30));
        assert_eq!(response.cell_id, Some(14));
        assert_eq!(response.country.as_deref(), Some("fr"));
    }

    #[test]
    fn truncated_header_has_no_steam_id() {
        let response = parse_logon_response(&[0x09, 0x01], &[]);
        assert_eq!(response, LogOnResponse::default());
    }

    #[test]
    fn license_list_extracts_packages_and_owners() {
        let mut first = Vec::new();
        field_varint(1, 228_980, &mut first);
        // La date arrive en `fixed32` : c'est ainsi que le CM l'écrit.
        first.extend([0x15]);
        first.extend(1_700_000_000u32.to_le_bytes());
        field_varint(6, 2, &mut first);
        let mut second = Vec::new();
        field_varint(1, 228_990, &mut second);
        field_varint(12, 4242, &mut second);
        let mut body = Vec::new();
        field_varint(1, 1, &mut body);
        field_bytes(2, &first, &mut body);
        field_bytes(2, &second, &mut body);
        assert_eq!(
            parse_license_list(&body),
            vec![
                License {
                    package_id: 228_980,
                    owner_id: 0,
                    time_created: 1_700_000_000,
                    payment_method: 2,
                },
                License {
                    package_id: 228_990,
                    owner_id: 4242,
                    time_created: 0,
                    payment_method: 0,
                },
            ]
        );
    }

    #[test]
    fn multi_splits_length_prefixed_messages() {
        let mut inner = Vec::new();
        proto_header(ANON_STEAM_ID, 0);
        field_varint(1, 1, &mut inner);
        let mut payload = Vec::new();
        payload.extend_from_slice(&(inner.len() as u32).to_le_bytes());
        payload.extend_from_slice(&inner);
        let mut body = Vec::new();
        field_varint(1, 0, &mut body);
        field_bytes(2, &payload, &mut body);
        let messages = split_multi(&body).unwrap();
        assert_eq!(messages, vec![inner]);
    }

    #[test]
    fn multi_rejects_truncated_payload() {
        let mut body = Vec::new();
        field_varint(1, 0, &mut body);
        field_bytes(2, &[8, 0, 0, 0, 1], &mut body);
        assert!(split_multi(&body).is_err());
    }

    #[test]
    fn multi_rejects_oversized_declaration() {
        let mut body = Vec::new();
        field_varint(1, (MAX_MULTI + 1) as u64, &mut body);
        field_bytes(2, b"x", &mut body);
        assert!(split_multi(&body).is_err());
    }
}
