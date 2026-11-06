//! Authentification moderne de Steam, sur le canal CM déjà chiffré.
//!
//! Le logon historique `ClientLogon` avec un mot de passe n'est plus accepté
//! par Steam. Les clients actuels (SteamCMD, SteamKit2, JavaSteam — le moteur
//! de GameNative) passent par le service unifié `Authentication` :
//!
//! 1. `GetPasswordRSAPublicKey` rend une clé RSA propre au compte ;
//! 2. le mot de passe y est chiffré en PKCS#1 v1.5 puis envoyé à
//!    `BeginAuthSessionViaCredentials` ;
//! 3. Steam Guard (e-mail, application ou confirmation) est traité pendant le
//!    `PollAuthSessionStatus` ;
//! 4. le jeton de refresh obtenu sert de `access_token` au `ClientLogon`.
//!
//! Aucun mot de passe n'est envoyé dans `ClientLogon`, et le jeton de refresh
//! permet les connexions suivantes sans mot de passe.

use super::crypto::{self, Channel};
use super::{proto, wire, Config, Error, Guard};
use rand_core::OsRng;
use rsa::{BigUint, Pkcs1v15Encrypt, RsaPublicKey};
use std::io::ErrorKind;
use std::net::TcpStream;
use std::time::{Duration, Instant};

/// Résultat d'une authentification réussie.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tokens {
    pub account_name: String,
    pub steam_id: u64,
    /// Jeton de refresh : c'est lui qui sert de `access_token` au CM.
    pub refresh_token: String,
    /// Jeton d'accès de courte durée (inutilisé par ce client, conservé pour
    /// les appels WebAPI futurs).
    pub access_token: Option<String>,
    /// Donnée Steam Guard à renvoyer au prochain `BeginAuthSessionViaCredentials`.
    pub guard_data: Option<String>,
}

/// Réponse d'un appel de service unifié.
struct Reply {
    eresult: i32,
    body: Vec<u8>,
}

/// Authentifie un compte et rend les jetons nécessaires au logon CM.
///
/// `prompt` est appelé pour les codes Steam Guard (`retry` indique qu'un code
/// précédent a été refusé) et pour les confirmations à effectuer dans
/// l'application Steam ; pour une confirmation, la valeur rendue est ignorée.
pub fn authenticate(
    stream: &mut TcpStream,
    channel: &Channel,
    config: &Config,
    name: &str,
    password: &str,
    guard_data: Option<&str>,
    prompt: &mut dyn FnMut(Guard, bool) -> Result<String, Error>,
) -> Result<Tokens, Error> {
    let key = get_password_key(stream, channel, config, name)?;

    let encrypted_password = encrypt_password(&key.modulus_hex, &key.exponent_hex, password)?;
    let begin = proto::begin_auth_request(&proto::BeginAuthRequest {
        account_name: name,
        encrypted_password: &encrypted_password,
        timestamp: key.timestamp,
        persistent: true,
        guard_data,
        device: proto::AuthDevice {
            friendly_name: lapinux_core::APP_NAME,
            os_type: proto::LINUX_OS_TYPE as i32,
        },
    });
    let reply = call(
        stream,
        channel,
        config,
        proto::AUTH_BEGIN_CREDENTIALS,
        &begin,
    )?;
    if reply.eresult != super::RESULT_OK as i32 {
        return Err(Error::Auth(format!(
            "connexion refusée : {} (eresult {})",
            super::result_label(reply.eresult),
            reply.eresult
        )));
    }
    let session = proto::parse_begin_auth(&reply.body);
    if session.client_id == 0 || session.request_id.is_empty() {
        return Err(Error::Protocol(
            "BeginAuthSessionViaCredentials sans session".into(),
        ));
    }
    if let Some(detail) = session.extended_error.as_deref().filter(|d| !d.is_empty()) {
        return Err(Error::Auth(detail.to_string()));
    }

    handle_guard(stream, channel, config, &session, prompt)?;
    poll_tokens(stream, channel, config, &session, name)
}

fn get_password_key(
    stream: &mut TcpStream,
    channel: &Channel,
    config: &Config,
    name: &str,
) -> Result<proto::PasswordRsaKey, Error> {
    let request = proto::get_password_rsa_key_request(name);
    let reply = call(
        stream,
        channel,
        config,
        proto::AUTH_GET_PASSWORD_KEY,
        &request,
    )?;
    if reply.eresult != super::RESULT_OK as i32 {
        return Err(Error::Auth(format!(
            "clé du mot de passe refusée : {} (eresult {})",
            super::result_label(reply.eresult),
            reply.eresult
        )));
    }
    let key = proto::parse_password_rsa_key(&reply.body);
    if key.modulus_hex.is_empty() || key.exponent_hex.is_empty() {
        return Err(Error::Protocol("clé RSA du compte incomplète".into()));
    }
    Ok(key)
}

/// Traite la confirmation Steam Guard la plus prioritaire, comme JavaSteam :
/// confirmation mobile, puis code d'application, puis code e-mail.
fn handle_guard(
    stream: &mut TcpStream,
    channel: &Channel,
    config: &Config,
    session: &proto::BeginAuthResponse,
    prompt: &mut dyn FnMut(Guard, bool) -> Result<String, Error>,
) -> Result<(), Error> {
    let Some(confirmation) = preferred_confirmation(&session.confirmations) else {
        return Ok(());
    };
    let guard = match confirmation.kind {
        proto::GUARD_NONE => return Ok(()),
        proto::GUARD_EMAIL_CODE => Guard::EmailCode,
        proto::GUARD_DEVICE_CODE => Guard::TwoFactor,
        proto::GUARD_DEVICE_CONFIRMATION => Guard::DeviceConfirmation,
        proto::GUARD_EMAIL_CONFIRMATION => Guard::EmailConfirmation,
        other => {
            return Err(Error::Auth(format!(
                "confirmation Steam Guard non gérée : type {other}"
            )))
        }
    };
    if matches!(guard, Guard::DeviceConfirmation | Guard::EmailConfirmation) {
        prompt(guard, false)?;
        return Ok(());
    }

    let mut retry = false;
    for attempt in 0..super::MAX_GUARD_ATTEMPTS {
        let code = prompt(guard, retry)?;
        let request =
            proto::update_guard_code_request(session.client_id, session.steam_id, &code, confirmation.kind);
        let reply = call(
            stream,
            channel,
            config,
            proto::AUTH_UPDATE_GUARD_CODE,
            &request,
        )?;
        // DuplicateRequest (29) arrive quand la connexion a déjà été confirmée
        // dans l'application Steam : le prochain poll rendra les jetons.
        if matches!(reply.eresult, 1 | 29) {
            return Ok(());
        }
        // Un code faux laisse la session ouverte : Steam demande simplement un
        // autre code (InvalidLoginAuthCode ou TwoFactorCodeMismatch).
        if matches!(reply.eresult, 65 | 88) && attempt + 1 < super::MAX_GUARD_ATTEMPTS {
            retry = true;
            continue;
        }
        return Err(Error::Auth(format!(
            "code Steam Guard refusé : {} (eresult {})",
            super::result_label(reply.eresult),
            reply.eresult
        )));
    }
    Err(Error::Auth("trop de codes Steam Guard refusés".into()))
}

/// Ordre de préférence de JavaSteam : pas de garde, confirmation de
/// l'application, code de l'application, code e-mail, confirmation e-mail.
fn preferred_confirmation(
    confirmations: &[proto::AuthConfirmation],
) -> Option<&proto::AuthConfirmation> {
    fn rank(kind: u32) -> u8 {
        match kind {
            proto::GUARD_NONE => 0,
            proto::GUARD_DEVICE_CONFIRMATION => 1,
            proto::GUARD_DEVICE_CODE => 2,
            proto::GUARD_EMAIL_CODE => 3,
            proto::GUARD_EMAIL_CONFIRMATION => 4,
            _ => 5,
        }
    }
    confirmations.iter().min_by_key(|item| rank(item.kind))
}

fn poll_tokens(
    stream: &mut TcpStream,
    channel: &Channel,
    config: &Config,
    session: &proto::BeginAuthResponse,
    fallback_name: &str,
) -> Result<Tokens, Error> {
    let interval = if session.interval > 0.0 {
        session.interval.clamp(1.0, 10.0)
    } else {
        5.0
    };
    let interval = Duration::from_secs_f32(interval);
    let deadline = Instant::now() + config.auth_timeout;
    let mut client_id = session.client_id;
    let request_id = session.request_id.clone();

    loop {
        let request = proto::poll_auth_request(client_id, &request_id);
        let reply = call(
            stream,
            channel,
            config,
            proto::AUTH_POLL_SESSION,
            &request,
        )?;
        if reply.eresult != super::RESULT_OK as i32 {
            return Err(Error::Auth(format!(
                "session d'authentification perdue : {} (eresult {})",
                super::result_label(reply.eresult),
                reply.eresult
            )));
        }
        let status = proto::parse_poll_auth(&reply.body);
        if status.new_client_id != 0 {
            client_id = status.new_client_id;
        }
        if let Some(refresh) = status.refresh_token.filter(|token| !token.is_empty()) {
            return Ok(Tokens {
                account_name: status
                    .account_name
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| fallback_name.to_string()),
                steam_id: session.steam_id,
                refresh_token: refresh,
                access_token: status.access_token.filter(|token| !token.is_empty()),
                guard_data: status.new_guard_data.filter(|data| !data.is_empty()),
            });
        }
        if Instant::now() >= deadline {
            return Err(Error::Timeout);
        }
        std::thread::sleep(interval);
    }
}

/// Démarre une connexion par QR code : Steam rend l'URL à afficher et la
/// session à interroger jusqu'à ce que le téléphone l'approuve.
pub(super) fn begin_qr(
    stream: &mut TcpStream,
    channel: &Channel,
    config: &Config,
) -> Result<proto::BeginQrResponse, Error> {
    let request = proto::begin_qr_request(lapinux_core::APP_NAME);
    let reply = call(stream, channel, config, proto::AUTH_BEGIN_QR, &request)?;
    if reply.eresult != super::RESULT_OK as i32 {
        return Err(Error::Auth(format!(
            "QR refusé : {} (eresult {})",
            super::result_label(reply.eresult),
            reply.eresult
        )));
    }
    let response = proto::parse_begin_qr(&reply.body);
    if response.client_id == 0 || response.request_id.is_empty() || response.challenge_url.is_empty()
    {
        return Err(Error::Protocol("QR sans session".into()));
    }
    Ok(response)
}

/// Résultat d'une attente de QR.
pub(super) enum QrPoll {
    /// Le téléphone a approuvé : les jetons sont là.
    Approved(Tokens),
    /// Steam laisse expirer un défi ; `until` est passé et l'appelant doit en
    /// afficher un nouveau plutôt que d'abandonner.
    Renew,
}

/// Attend l'approbation du QR. `on_url` reçoit chaque nouvelle URL quand Steam
/// fait tourner le défi ; `cancelled` est consulté entre deux sondages et
/// `until` borne l'attente de ce défi précis.
pub(super) fn poll_qr(
    stream: &mut TcpStream,
    channel: &Channel,
    config: &Config,
    challenge: &proto::BeginQrResponse,
    on_url: &mut dyn FnMut(&str),
    cancelled: &dyn Fn() -> bool,
    until: Instant,
) -> Result<QrPoll, Error> {
    let interval = if challenge.interval > 0.0 {
        challenge.interval.clamp(1.0, 10.0)
    } else {
        5.0
    };
    let interval = Duration::from_secs_f32(interval);
    let mut client_id = challenge.client_id;
    let request_id = challenge.request_id.clone();

    loop {
        if cancelled() {
            return Err(Error::Auth("connexion annulée".into()));
        }
        let request = proto::poll_auth_request(client_id, &request_id);
        let reply = call(
            stream,
            channel,
            config,
            proto::AUTH_POLL_SESSION,
            &request,
        )?;
        if reply.eresult != super::RESULT_OK as i32 {
            return Err(Error::Auth(format!(
                "session QR perdue : {} (eresult {})",
                super::result_label(reply.eresult),
                reply.eresult
            )));
        }
        let status = proto::parse_poll_auth(&reply.body);
        if status.new_client_id != 0 {
            client_id = status.new_client_id;
        }
        if let Some(url) = status
            .new_challenge_url
            .as_deref()
            .filter(|url| !url.is_empty())
        {
            on_url(url);
        }
        if let Some(refresh) = status.refresh_token.filter(|token| !token.is_empty()) {
            tracing::debug!(
                "QR approuvé : compte {:?}, jeton de rafraîchissement {} octet(s), jeton d'accès {}, donnée Guard {}",
                status.account_name,
                refresh.len(),
                status
                    .access_token
                    .as_deref()
                    .map(|token| token.len())
                    .unwrap_or(0),
                if status.new_guard_data.is_some() {
                    "reçue"
                } else {
                    "absente"
                }
            );
            // Le JWT nomme le compte : l'en-tête du logon doit porter ce
            // SteamID, même quand le poll ne rend pas l'account_name.
            let steam_id = proto::refresh_token_steam_id(&refresh).unwrap_or(0);
            return Ok(QrPoll::Approved(Tokens {
                account_name: status.account_name.unwrap_or_default(),
                steam_id,
                refresh_token: refresh,
                access_token: status.access_token.filter(|token| !token.is_empty()),
                guard_data: status.new_guard_data.filter(|data| !data.is_empty()),
            }));
        }
        if Instant::now() >= until {
            return Ok(QrPoll::Renew);
        }
        // Rendre la main souvent : une annulation ne doit pas attendre l'intervalle.
        let next = (Instant::now() + interval).min(until);
        while Instant::now() < next {
            if cancelled() {
                return Err(Error::Auth("connexion annulée".into()));
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

/// Appelle une méthode du service unifié et rend son corps de réponse.
fn call(
    stream: &mut TcpStream,
    channel: &Channel,
    config: &Config,
    method: &str,
    request: &[u8],
) -> Result<Reply, Error> {
    let job_id = job_id();
    let header = proto::service_header(job_id, method);
    let message = wire::proto_message(
        proto::SERVICE_METHOD_CALL_FROM_CLIENT_NON_AUTHED,
        &header,
        request,
    );
    wire::write_packet(stream, &channel.encrypt(&message))?;

    let deadline = Instant::now() + config.timeout;
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Err(Error::Timeout);
        }
        stream.set_read_timeout(Some((deadline - now).max(Duration::from_millis(1))))?;
        let raw = match wire::read_packet(stream) {
            Ok(raw) => raw,
            Err(Error::Io(error))
                if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) =>
            {
                return Err(Error::Timeout)
            }
            Err(error) => return Err(error),
        };
        let plaintext = channel.decrypt(&raw)?;
        for message in super::split_incoming(&plaintext)? {
            let Some((emsg, header, body)) = wire::parse_proto(&message) else {
                continue;
            };
            if emsg != proto::SERVICE_METHOD_RESPONSE {
                continue;
            }
            let parsed = proto::parse_service_header(header);
            if parsed.target_job_id != job_id
                && parsed.target_job_name.as_deref() != Some(method)
            {
                continue;
            }
            return Ok(Reply {
                eresult: parsed.eresult.unwrap_or(super::RESULT_OK as i32),
                body: body.to_vec(),
            });
        }
    }
}

fn job_id() -> u64 {
    let job = u64::from_le_bytes(crypto::random_bytes::<8>());
    match job {
        0 | u64::MAX => 0x1122_3344_5566_7788,
        job => job,
    }
}

/// Chiffre le mot de passe en RSA PKCS#1 v1.5, puis l'encode en base64.
fn encrypt_password(
    modulus_hex: &str,
    exponent_hex: &str,
    password: &str,
) -> Result<String, Error> {
    let modulus = BigUint::parse_bytes(
        modulus_hex.trim().trim_start_matches("0x").as_bytes(),
        16,
    )
    .ok_or_else(|| Error::Crypto("module RSA invalide".into()))?;
    let exponent = BigUint::parse_bytes(
        exponent_hex.trim().trim_start_matches("0x").as_bytes(),
        16,
    )
    .ok_or_else(|| Error::Crypto("exposant RSA invalide".into()))?;
    let key = RsaPublicKey::new(modulus, exponent)
        .map_err(|e| Error::Crypto(format!("clé RSA du compte : {e}")))?;
    let encrypted = key
        .encrypt(&mut OsRng, Pkcs1v15Encrypt, password.as_bytes())
        .map_err(|e| Error::Crypto(format!("RSA PKCS#1 : {e}")))?;
    Ok(base64_encode(&encrypted))
}

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(input: &[u8]) -> String {
    let mut out = String::with_capacity((input.len() + 2) / 3 * 4);
    for chunk in input.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        out.push(BASE64[(first >> 2) as usize] as char);
        out.push(BASE64[(((first & 0x03) << 4) | (second >> 4)) as usize] as char);
        if chunk.len() > 1 {
            out.push(BASE64[(((second & 0x0f) << 2) | (third >> 6)) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(BASE64[(third & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsa::traits::PublicKeyParts;
    use rsa::{RsaPrivateKey, RsaPublicKey};

    #[test]
    fn base64_matches_reference_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"hello"), "aGVsbG8=");
    }

    #[test]
    fn password_is_encrypted_in_pkcs1_and_decrypts() {
        let private = RsaPrivateKey::new(&mut OsRng, 1024).unwrap();
        let public = RsaPublicKey::from(&private);
        let modulus = hex(&public.n().to_bytes_be());
        let exponent = hex(&public.e().to_bytes_be());
        let encoded = encrypt_password(&modulus, &exponent, "s3cret").unwrap();
        let raw = base64_decode(&encoded);
        assert_eq!(raw.len(), 128);
        let plaintext = private.decrypt(Pkcs1v15Encrypt, &raw).unwrap();
        assert_eq!(plaintext, b"s3cret");
    }

    #[test]
    fn a_malformed_key_is_rejected() {
        assert!(matches!(
            encrypt_password("zz", "010001", "x"),
            Err(Error::Crypto(_))
        ));
        assert!(matches!(
            encrypt_password("", "010001", "x"),
            Err(Error::Crypto(_))
        ));
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn base64_decode(value: &str) -> Vec<u8> {
        let mut out = Vec::new();
        let mut buffer = 0u32;
        let mut bits = 0u32;
        for byte in value.bytes().filter(|byte| *byte != b'=') {
            let digit = BASE64
                .iter()
                .position(|candidate| *candidate == byte)
                .expect("base64 valide") as u32;
            buffer = (buffer << 6) | digit;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push((buffer >> bits) as u8);
                buffer &= (1 << bits) - 1;
            }
        }
        out
    }
}
