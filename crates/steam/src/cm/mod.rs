//! Client du protocole CM de Steam — la couche d'authentification de Lapinux.
//!
//! Le moteur de Valve n'expose aucune interface de connexion (voir
//! [`crate::probe`]). Ce module parle donc directement aux serveurs CM :
//! découverte de la liste par la WebAPI, trame TCP `VT01`, échange de clé
//! RSA-OAEP/SHA-1, canal AES-256-CBC, puis `ClientLogon`.
//!
//! L'accès anonyme (`steamcmd +login anonymous`) est implémenté et vérifié de
//! bout en bout. La connexion nominative suit le chemin des clients actuels
//! (SteamCMD, SteamKit2, JavaSteam — le moteur de GameNative) : le service
//! `Authentication` rend un jeton de refresh, puis `ClientLogon` s'authentifie
//! avec ce jeton, sans jamais renvoyer le mot de passe.

pub mod auth;
pub mod crypto;
pub mod proto;
pub mod wire;

use serde_json::Value;
use std::io::ErrorKind;
use std::net::TcpStream;
use std::time::{Duration, Instant};
use wire::Endpoint;

/// `k_EMsgChannelEncryptRequest`.
const CHANNEL_ENCRYPT_REQUEST: u32 = 1303;
/// `k_EMsgChannelEncryptResponse`.
const CHANNEL_ENCRYPT_RESPONSE: u32 = 1304;
/// `k_EMsgChannelEncryptResult`.
const CHANNEL_ENCRYPT_RESULT: u32 = 1305;
/// `k_EMsgClientLogon`.
const CLIENT_LOGON: u32 = 5514;
/// `k_EMsgClientLogOnResponse`.
const CLIENT_LOGON_RESPONSE: u32 = 751;
/// `k_EMsgClientLoggedOff`.
const CLIENT_LOGGED_OFF: u32 = 757;
/// `k_EMsgClientLicenseList`.
const CLIENT_LICENSE_LIST: u32 = 780;
/// `k_EMsgMulti`.
const MULTI: u32 = 1;
/// Nombre de CM essayés avant d'abandonner.
const MAX_ATTEMPTS: usize = 4;
/// Nombre de codes Steam Guard demandés avant d'abandonner.
const MAX_GUARD_ATTEMPTS: u8 = 3;
/// Attente maximale d'une confirmation Steam Guard (application ou e-mail).
const DEFAULT_AUTH_TIMEOUT: Duration = Duration::from_secs(180);
/// Durée de vie d'un défi QR : Steam le fait expirer vers deux minutes, un
/// nouveau défi est donc affiché avant, pour que le code reste scannable.
const QR_LIFETIME: Duration = Duration::from_secs(95);
/// Attente totale d'une approbation QR, renouvellements compris.
const QR_BUDGET: Duration = Duration::from_secs(300);
/// Défis QR successifs autorisés avant d'abandonner.
const MAX_QR_RENEWALS: u32 = 8;
/// Protocole de chiffrement du canal (le seul accepté par les CM).
const CHANNEL_PROTOCOL: u32 = 1;
/// Univers Public.
const UNIVERSE_PUBLIC: u32 = 1;
/// `EResult::OK`.
const RESULT_OK: u32 = 1;

/// Liste des CM « netfilter » (TCP 27017) publiée par Steam.
pub const CM_LIST_URL: &str =
    "https://api.steampowered.com/ISteamDirectory/GetCMListForConnect/v1/?cellid=0&cmtype=netfilter";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("E/S : {0}")]
    Io(#[from] std::io::Error),
    #[error("réseau : {0}")]
    Http(String),
    #[error("connexion : {0}")]
    Connect(String),
    #[error("protocole : {0}")]
    Protocol(String),
    #[error("chiffrement : {0}")]
    Crypto(String),
    #[error("aucun serveur CM disponible")]
    NoServer,
    #[error("délai dépassé")]
    Timeout,
    #[error("authentification : {0}")]
    Auth(String),
}

impl Error {
    /// Vrai si Steam a refusé pour une limite temporaire (EResult 84). Ce
    /// refus ne dit rien du jeton : il faut attendre, pas rescanner.
    pub fn is_rate_limited(&self) -> bool {
        let text = self.to_string().to_lowercase();
        text.contains("eresult 84")
            || text.contains("ratelimitexceeded")
            || text.contains("rate limit exceeded")
            || text.contains("trop de tentatives")
            || text.contains("limite les tentatives")
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    /// Hôtes `host:port` imposés ; vide = découverte par la WebAPI.
    pub endpoints: Vec<String>,
    /// Délai de connexion et de lecture de chaque étape.
    pub timeout: Duration,
    /// Attente de `ClientLicenseList` après une connexion réussie.
    pub license_wait: Duration,
    /// Attente maximale d'une authentification et de ses confirmations.
    pub auth_timeout: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            endpoints: Vec::new(),
            timeout: Duration::from_secs(6),
            license_wait: Duration::from_millis(750),
            auth_timeout: DEFAULT_AUTH_TIMEOUT,
        }
    }
}

impl Config {
    /// Reprend `LAPINUX_STEAM_CM`, liste de serveurs séparée par des
    /// virgules, si présent : pratique pour épingler un CM en test.
    pub fn from_env() -> Self {
        let mut config = Self::default();
        if let Some(value) = lapinux_core::env_var("STEAM_CM") {
            config.endpoints = value
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_string)
                .collect();
        }
        config
    }
}

/// Identifiants d'une connexion CM.
#[derive(Clone)]
pub enum Auth {
    Anonymous,
    /// Authentification complète : mot de passe, puis jeton de refresh.
    Account {
        name: String,
        password: String,
        /// Donnée Steam Guard rendue par une connexion précédente.
        guard_data: Option<String>,
    },
    /// Reconnexion sans mot de passe avec le jeton de refresh enregistré.
    Token { name: String, refresh_token: String },
}

/// Code Steam Guard demandé en cours de connexion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Guard {
    EmailCode,
    TwoFactor,
    /// Approbation dans l'application Steam (ou lien e-mail).
    DeviceConfirmation,
    EmailConfirmation,
}

/// Licence rendue par Steam : le paquet et le compte qui le possède.
/// Un `owner_id` différent du compte connecté signale un jeu partagé par la
/// famille Steam.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct License {
    pub package_id: u32,
    pub owner_id: u32,
    /// Date d'ajout de la licence au compte, en secondes Unix (0 = inconnue).
    pub time_created: i64,
    /// Moyen de paiement publié par Steam (0 = gratuit ou non renseigné).
    pub payment_method: u32,
}

/// Résultat d'une connexion CM.
#[derive(Debug, Clone)]
pub struct Session {
    pub endpoint: String,
    /// Vrai pour `+login anonymous`, faux pour un compte nominatif.
    pub anonymous: bool,
    /// Taille du nonce envoyé par le CM (le canal reste en mode historique).
    pub challenge_len: usize,
    pub steam_id: u64,
    pub session_id: i32,
    pub eresult: i32,
    pub heartbeat_seconds: Option<u32>,
    pub cell_id: Option<u32>,
    pub country: Option<String>,
    /// `None` si le CM n'a pas envoyé de liste dans le délai imparti.
    pub licenses: Option<Vec<License>>,
    /// Jeton de refresh obtenu par le service d'authentification.
    pub refresh_token: Option<String>,
    /// Donnée Steam Guard à réutiliser au prochain logon.
    pub guard_data: Option<String>,
    /// Nom rendu par Steam (peut différer de la casse saisie).
    pub account_name: Option<String>,
}

/// Découvre les CM par la WebAPI ; les moins chargés d'abord.
pub fn discover(timeout: Duration) -> Result<Vec<Endpoint>, Error> {
    let agent = ureq::AgentBuilder::new().timeout(timeout).build();
    let response = agent
        .get(CM_LIST_URL)
        .call()
        .map_err(|e| Error::Http(e.to_string()))?;
    let text = response
        .into_string()
        .map_err(|e| Error::Http(format!("liste CM illisible : {e}")))?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|e| Error::Http(format!("liste CM illisible : {e}")))?;
    parse_server_list(&value)
}

/// Extrait et trie `response.serverlist[].endpoint`.
pub fn parse_server_list(value: &Value) -> Result<Vec<Endpoint>, Error> {
    let list = value
        .get("response")
        .and_then(|response| response.get("serverlist"))
        .and_then(Value::as_array)
        .ok_or_else(|| Error::Http("réponse sans serverlist".into()))?;
    let mut servers: Vec<(u64, Endpoint)> = list
        .iter()
        .filter_map(|server| {
            let endpoint = server.get("endpoint")?.as_str()?.parse().ok()?;
            let load = server
                .get("load")
                .and_then(Value::as_u64)
                .unwrap_or(u64::MAX);
            Some((load, endpoint))
        })
        .collect();
    servers.sort_by_key(|(load, _)| *load);
    servers.dedup_by(|a, b| a.1 == b.1);
    if servers.is_empty() {
        return Err(Error::NoServer);
    }
    Ok(servers.into_iter().map(|(_, endpoint)| endpoint).collect())
}

fn endpoints(config: &Config) -> Result<Vec<Endpoint>, Error> {
    if !config.endpoints.is_empty() {
        return config
            .endpoints
            .iter()
            .map(|item| item.parse())
            .collect();
    }
    match discover(config.timeout) {
        Ok(servers) if !servers.is_empty() => Ok(servers),
        Ok(_) => Err(Error::NoServer),
        Err(error) => {
            tracing::debug!("découverte CM indisponible : {error}");
            Ok(vec![Endpoint {
                host: "cm0.steampowered.com".into(),
                port: 27017,
            }])
        }
    }
}

/// Durée pendant laquelle un refus de connexion par jeton est retenu.
///
/// Steam ne garde qu'une session par compte. Multiplier les tentatives après
/// un refus (`mot de passe invalide`, eresult 5) ne fait qu'allonger le
/// blocage qu'il applique alors : la réponse est donc gardée quelques minutes
/// au lieu d'être redemandée à chaque clic ou à chaque sondage.
const AUTH_REFUSAL_HOLD: Duration = Duration::from_secs(300);

/// Refus récent, avec le compte concerné et sa date.
static AUTH_REFUSAL: std::sync::Mutex<Option<(String, Instant)>> = std::sync::Mutex::new(None);

/// Blocage temporaire de Steam (EResult 84) : 30 minutes sans nouvelle
/// tentative, sinon le compteur repart.
const RATE_LIMIT_HOLD: Duration = Duration::from_secs(30 * 60);

static RATE_LIMIT: std::sync::Mutex<Option<(String, Instant)>> = std::sync::Mutex::new(None);

fn recent_rate_limit(account: &str) -> Option<u64> {
    let limited = RATE_LIMIT.lock().ok()?;
    let (name, at) = limited.as_ref()?;
    if name != account {
        return None;
    }
    let elapsed = at.elapsed();
    (elapsed < RATE_LIMIT_HOLD).then(|| (RATE_LIMIT_HOLD - elapsed).as_secs().max(1))
}

fn remember_rate_limit(account: Option<&str>) {
    if let Ok(mut slot) = RATE_LIMIT.lock() {
        *slot = account.map(|name| (name.to_string(), Instant::now()));
    }
}

fn clear_rate_limit(account: &str) {
    if let Ok(mut slot) = RATE_LIMIT.lock() {
        if slot
            .as_ref()
            .map(|(name, _)| name.as_str() == account)
            .unwrap_or(false)
        {
            *slot = None;
        }
    }
}

/// Compte auquel un refus récent s'applique, `None` si le dernier essai est
/// assez ancien. Ne concerne que le jeton (mot de passe : un refus dit alors
/// simplement que le mot de passe est faux, et la saisie doit pouvoir être
/// rejouée tout de suite).
fn recent_refusal(account: &str) -> Option<u64> {
    let refusal = AUTH_REFUSAL.lock().ok()?;
    let (name, at) = refusal.as_ref()?;
    if name != account {
        return None;
    }
    let elapsed = at.elapsed();
    (elapsed < AUTH_REFUSAL_HOLD).then(|| (AUTH_REFUSAL_HOLD - elapsed).as_secs())
}

/// Retient un refus pour ce compte, ou l'oublie après une réussite.
fn remember_refusal(account: Option<&str>) {
    if let Ok(mut slot) = AUTH_REFUSAL.lock() {
        *slot = account.map(|name| (name.to_string(), Instant::now()));
    }
}

/// Verdict d'un échec, quand le compte est connu.
fn refusal_account(auth: &Auth) -> Option<&str> {
    match auth {
        Auth::Token { name, .. } | Auth::Account { name, .. } => Some(name.as_str()),
        Auth::Anonymous => None,
    }
}

/// Un refus d'authentification pendant une connexion par jeton.
fn is_auth_refusal(auth: &Auth, error: &Error) -> bool {
    matches!(auth, Auth::Token { .. }) && matches!(error, Error::Auth(_))
}

/// Ouvre une session sur le premier CM qui répond.
///
/// `prompt` est appelé si le CM exige un code Steam Guard : le CLI le lit au
/// terminal, les tests peuvent fournir une réponse fixe.
pub fn sign_in(
    config: &Config,
    auth: &Auth,
    prompt: &mut dyn FnMut(Guard, bool) -> Result<String, Error>,
) -> Result<Session, Error> {
    if let Some(name) = refusal_account(auth) {
        if let Some(seconds) = recent_rate_limit(name) {
            return Err(Error::Auth(format!(
                "Steam limite les tentatives pour ce compte ; nouvel essai dans {seconds} s"
            )));
        }
        if let Some(seconds) = recent_refusal(name) {
            return Err(Error::Auth(format!(
                "Steam refuse encore les connexions pour ce compte ; nouvel essai dans {seconds} s"
            )));
        }
    }
    let servers = endpoints(config)?;
    let mut last = Error::NoServer;
    for endpoint in servers.iter().take(MAX_ATTEMPTS) {
        tracing::debug!("CM : essai sur {endpoint}");
        match try_endpoint(endpoint, config, auth, prompt) {
            Ok(session) => {
                remember_refusal(None);
                if let Some(name) = refusal_account(auth) {
                    clear_rate_limit(name);
                }
                return Ok(session);
            }
            Err(error) => {
                tracing::debug!("CM {endpoint} : {error}");
                last = error;
            }
        }
    }
    if last.is_rate_limited() {
        remember_rate_limit(refusal_account(auth));
    }
    if is_auth_refusal(auth, &last) {
        remember_refusal(refusal_account(auth));
    }
    Err(last)
}

/// Ouvre une session anonyme (`steamcmd +login anonymous`).
pub fn sign_in_anonymous(config: &Config) -> Result<Session, Error> {
    let mut prompt = |_, _| Err(Error::Auth("aucun code Steam Guard attendu".into()));
    sign_in(config, &Auth::Anonymous, &mut prompt)
}

/// Connexion par QR code : Steam rend une URL, le téléphone l'approuve, le CM
/// rend le jeton, puis la même connexion ouvre la session.
///
/// `on_url` reçoit l'URL à afficher (et ses renouvellements) ; `cancelled` est
/// consulté entre deux sondages pour qu'une annulation rende la main vite.
pub fn sign_in_qr(
    config: &Config,
    on_url: &mut dyn FnMut(&str),
    cancelled: &dyn Fn() -> bool,
) -> Result<Session, Error> {
    sign_in_qr_with_tokens(config, on_url, cancelled, &mut |_| {})
}

/// Connexion par QR qui livre les jetons dès l'approbation, avant le logon CM.
///
/// Un scan approuvé est une ressource rare : si le logon échoue ensuite
/// (rate-limit, coupure), l'appelant peut enregistrer le jeton et le réutiliser
/// sans faire rescanner l'utilisateur.
pub fn sign_in_qr_with_tokens(
    config: &Config,
    on_url: &mut dyn FnMut(&str),
    cancelled: &dyn Fn() -> bool,
    on_tokens: &mut dyn FnMut(&auth::Tokens),
) -> Result<Session, Error> {
    // Un QR fraîchement approuvé qui échoue au logon dit la même chose qu'un
    // jeton refusé : le compte est bloqué un moment, inutile de rescaner.
    if let Some(seconds) = recent_rate_limit(REFUSAL_QR) {
        return Err(Error::Auth(format!(
            "Steam limite les tentatives ; nouvel essai dans {seconds} s"
        )));
    }
    if let Some(seconds) = recent_refusal(REFUSAL_QR) {
        return Err(Error::Auth(format!(
            "Steam refuse encore les connexions ; nouvel essai dans {seconds} s"
        )));
    }
    let result = sign_in_qr_work(config, on_url, cancelled, on_tokens);
    match &result {
        Ok(_) => {
            remember_refusal(None);
            remember_rate_limit(None);
        }
        Err(error) if error.is_rate_limited() => {
            remember_rate_limit(Some(REFUSAL_QR));
            remember_refusal(Some(REFUSAL_QR));
        }
        Err(error) if matches!(error, Error::Auth(_)) => remember_refusal(Some(REFUSAL_QR)),
        Err(_) => {}
    }
    result
}

/// Compte fictif sous lequel un refus de QR est retenu : le compte n'est pas
/// encore connu à ce stade.
const REFUSAL_QR: &str = "qr";

fn sign_in_qr_work(
    config: &Config,
    on_url: &mut dyn FnMut(&str),
    cancelled: &dyn Fn() -> bool,
    on_tokens: &mut dyn FnMut(&auth::Tokens),
) -> Result<Session, Error> {
    let servers = endpoints(config)?;
    if cancelled() {
        return Err(Error::Auth("connexion annulée".into()));
    }
    // Le budget vaut pour toute la connexion par QR, pas par serveur.
    let started = Instant::now();
    let deadline = started + config.auth_timeout.max(QR_BUDGET);
    let (endpoint, link) = open_first(&servers, config)?;
    let result = try_qr_endpoint(
        &endpoint,
        config,
        link,
        on_url,
        cancelled,
        on_tokens,
        deadline,
    );
    // Plus de temps ou plus de serveur : la seule action utile est de relancer.
    match result {
        Err(error) if matches!(error, Error::NoServer | Error::Timeout | Error::Io(_)) => {
            Err(Error::Auth(format!(
                "le défi QR n'a pas abouti en {} s ; relancez la connexion",
                started.elapsed().as_secs()
            )))
        }
        other => other,
    }
}

/// Ouvre le premier canal disponible parmi les CM proposés.
///
/// Les essais partent ensemble : la liste est triée par charge, mais un CM
/// injoignable coûtait sinon tout son délai de connexion avant que le suivant
/// soit seulement tenté — c'est ce qui retardait l'affichage du QR.
fn open_first(servers: &[Endpoint], config: &Config) -> Result<(Endpoint, Link), Error> {
    let candidates: Vec<Endpoint> = servers.iter().take(MAX_ATTEMPTS).cloned().collect();
    if candidates.is_empty() {
        return Err(Error::NoServer);
    }
    let (sender, receiver) = std::sync::mpsc::channel();
    for endpoint in candidates {
        let sender = sender.clone();
        let config = config.clone();
        std::thread::spawn(move || {
            let result = open(&endpoint, &config);
            let _ = sender.send((endpoint, result));
        });
    }
    let mut last = Error::NoServer;
    for (endpoint, result) in receiver.iter() {
        match result {
            Ok(link) => {
                tracing::debug!("CM retenu : {endpoint}");
                return Ok((endpoint, link));
            }
            Err(error) => {
                tracing::debug!("CM {endpoint} injoignable : {error}");
                last = error;
            }
        }
    }
    Err(last)
}

fn try_qr_endpoint(
    endpoint: &Endpoint,
    config: &Config,
    mut link: Link,
    on_url: &mut dyn FnMut(&str),
    cancelled: &dyn Fn() -> bool,
    on_tokens: &mut dyn FnMut(&auth::Tokens),
    deadline: Instant,
) -> Result<Session, Error> {
    // Steam fait tourner l'URL du défi toutes les vingt secondes environ : la
    // date du dernier défi publié dit si le QR affiché est encore vivant. Une
    // approbation du téléphone vaut pour le défi en cours, donc le renouveler
    // sans raison risquerait de la perdre.
    let fresh = std::cell::Cell::new(Instant::now());
    let mut publish = |url: &str| {
        fresh.set(Instant::now());
        on_url(url);
    };
    let mut challenge = auth::begin_qr(&mut link.stream, &link.channel, config)?;
    publish(&challenge.challenge_url);

    // Si le défi cesse d'être renouvelé (connexion perdue, session expirée), un
    // nouveau défi vaut mieux qu'un abandon : le téléphone n'a qu'à scanner
    // celui qui est affiché.
    // Un CM qui répond une erreur instantanément ne doit pas faire tourner la
    // boucle à vide jusqu'au budget : quelques défis suffisent à conclure.
    let mut renewals = 0u32;
    let tokens = loop {
        let until = (fresh.get() + QR_LIFETIME).min(deadline);
        match auth::poll_qr(
            &mut link.stream,
            &link.channel,
            config,
            &challenge,
            &mut publish,
            cancelled,
            until,
        ) {
            Ok(auth::QrPoll::Approved(tokens)) => break tokens,
            Ok(auth::QrPoll::Renew) => {
                if Instant::now() >= deadline || renewals >= MAX_QR_RENEWALS {
                    return Err(Error::Timeout);
                }
                renewals += 1;
            }
            Err(error) if cancelled() => return Err(error),
            Err(error) => {
                if Instant::now() >= deadline || renewals >= MAX_QR_RENEWALS {
                    return Err(error);
                }
                renewals += 1;
                // Canal coupé ou session QR perdue : un CM neuf et un défi
                // neuf repartent proprement, l'URL affichée suit.
                tracing::debug!("QR : {error}, nouveau défi");
                if matches!(error, Error::Io(_) | Error::Timeout) {
                    link = open(endpoint, config)?;
                }
            }
        }
        challenge = auth::begin_qr(&mut link.stream, &link.channel, config)?;
        publish(&challenge.challenge_url);
    };

    // Le jeton est valable avant même le logon : on le livre maintenant pour
    // qu'il survive à un refus du CM (rate-limit, coupure).
    on_tokens(&tokens);

    let auth = Auth::Token {
        name: tokens.account_name.clone(),
        refresh_token: tokens.refresh_token.clone(),
    };
    let mut prompt = |_, _| Err(Error::Auth("aucun code Steam Guard attendu".into()));
    let mut session = logon(
        &mut link.stream,
        &link.channel,
        endpoint,
        config,
        &auth,
        &mut prompt,
        link.challenge_len,
    )?;
    // La donnée Steam Guard du téléphone vaut pour les prochaines connexions.
    session.guard_data = tokens.guard_data.or(session.guard_data);
    // Une approbation par QR ne rend pas toujours le nom de compte : sans lui
    // l'application ne saurait pas sous quel compte enregistrer le jeton.
    if session
        .account_name
        .as_deref()
        .map(str::trim)
        .unwrap_or("")
        .is_empty()
    {
        session.account_name = account_name_for(session.steam_id)
            .or_else(|| Some(session.steam_id.to_string()).filter(|_| session.steam_id != 0));
    }
    Ok(session)
}

/// Nom de compte d'un SteamID, lu dans les `loginusers.vdf` du client Steam.
///
/// Un logon par jeton n'exige pas ce nom, mais Lapinux s'en sert pour nommer
/// la session enregistrée et remplir les réglages.
fn account_name_for(steam_id: u64) -> Option<String> {
    if steam_id == 0 {
        return None;
    }
    for root in lapinux_core::paths::steam_roots() {
        let path = root.join("config/loginusers.vdf");
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Some(name) = account_name_in(&text, steam_id) {
            return Some(name);
        }
    }
    None
}

/// Cherche le nom de compte `steam_id` dans le contenu d'un `loginusers.vdf`.
fn account_name_in(text: &str, steam_id: u64) -> Option<String> {
    if steam_id == 0 {
        return None;
    }
    let wanted = steam_id.to_string();
    let users = crate::vdf::parse_root(text).ok()?;
    let entry = users.objects().find(|(id, _)| *id == wanted)?.1;
    let name = entry.str("AccountName")?.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// Canal chiffré établi avec un CM, `ClientHello` déjà envoyé : tout appel de
/// service unifié ou logon part de là.
struct Link {
    stream: TcpStream,
    channel: crypto::Channel,
    challenge_len: usize,
}

fn open(endpoint: &Endpoint, config: &Config) -> Result<Link, Error> {
    let address = endpoint.socket_addr()?;
    let mut stream = TcpStream::connect_timeout(&address, config.timeout)
        .map_err(|e| Error::Connect(format!("{endpoint} : {e}")))?;
    stream.set_nodelay(true).ok();
    stream.set_write_timeout(Some(config.timeout))?;
    stream.set_read_timeout(Some(config.timeout))?;

    // 1. Le CM impose son nonce et sa version de protocole.
    let request = wire::read_packet(&mut stream)?;
    let (emsg, body) = wire::parse_struct(&request)
        .ok_or_else(|| Error::Protocol("trame d'initialisation illisible".into()))?;
    if emsg != CHANNEL_ENCRYPT_REQUEST {
        return Err(Error::Protocol(format!(
            "le CM ouvre par {emsg}, pas par ChannelEncryptRequest"
        )));
    }
    if body.len() < 8 {
        return Err(Error::Protocol("ChannelEncryptRequest tronqué".into()));
    }
    let protocol = u32::from_le_bytes(body[..4].try_into().expect("4 octets"));
    let universe = u32::from_le_bytes(body[4..8].try_into().expect("4 octets"));
    if protocol != CHANNEL_PROTOCOL || universe != UNIVERSE_PUBLIC {
        return Err(Error::Protocol(format!(
            "canal refusé : protocole {protocol}, univers {universe}"
        )));
    }
    let challenge = &body[8..];
    let challenge_len = challenge.len();

    // 2. Clé de session chiffrée RSA-OAEP/SHA-1, CRC sur le chiffré. Le nonce
    // du CM est joint à la clé : il sert de graine HMAC au canal.
    let session_key: [u8; 32] = crypto::random_bytes::<32>();
    let mut secret = Vec::with_capacity(32 + challenge.len());
    secret.extend_from_slice(&session_key);
    secret.extend_from_slice(challenge);
    let encrypted = crypto::encrypt_session_key(&secret)?;
    let mut response = Vec::with_capacity(8 + encrypted.len() + 8);
    response.extend_from_slice(&CHANNEL_PROTOCOL.to_le_bytes());
    response.extend_from_slice(&(encrypted.len() as u32).to_le_bytes());
    response.extend_from_slice(&encrypted);
    response.extend_from_slice(&crypto::crc32(&encrypted).to_le_bytes());
    response.extend_from_slice(&0u32.to_le_bytes());
    wire::write_packet(
        &mut stream,
        &wire::struct_message(CHANNEL_ENCRYPT_RESPONSE, &response),
    )?;

    let result = wire::read_packet(&mut stream)?;
    let (emsg, body) = wire::parse_struct(&result)
        .ok_or_else(|| Error::Protocol("ChannelEncryptResult illisible".into()))?;
    if emsg != CHANNEL_ENCRYPT_RESULT {
        return Err(Error::Protocol(format!(
            "réponse au canal : {emsg} au lieu de ChannelEncryptResult"
        )));
    }
    let eresult = body
        .get(..4)
        .map(|bytes| u32::from_le_bytes(bytes.try_into().expect("4 octets")))
        .ok_or_else(|| Error::Protocol("ChannelEncryptResult sans résultat".into()))?;
    if eresult != RESULT_OK {
        return Err(Error::Protocol(format!(
            "canal refusé (eresult {eresult})"
        )));
    }
    // Les CM actuels envoient un nonce : le canal ajoute alors un HMAC-SHA1,
    // dont le secret est la première moitié de la clé de session.
    let hmac = if challenge.is_empty() {
        None
    } else {
        let mut secret = [0u8; 16];
        secret.copy_from_slice(&session_key[..16]);
        Some(secret)
    };
    let channel = crypto::Channel::new(session_key, hmac);

    // 3. ClientHello : les clients l'annoncent dès le canal chiffré. Sans lui,
    // le CM accepte encore `ClientLogon` mais ferme la connexion sur un appel
    // de service unifié (`Authentication`).
    wire::write_packet(
        &mut stream,
        &channel.encrypt(&wire::proto_message(
            proto::CLIENT_HELLO,
            &proto::proto_header(0, 0),
            &proto::client_hello(),
        )),
    )?;

    Ok(Link {
        stream,
        channel,
        challenge_len,
    })
}

fn try_endpoint(
    endpoint: &Endpoint,
    config: &Config,
    auth: &Auth,
    prompt: &mut dyn FnMut(Guard, bool) -> Result<String, Error>,
) -> Result<Session, Error> {
    let Link {
        mut stream,
        channel,
        challenge_len,
    } = open(endpoint, config)?;
    logon(
        &mut stream,
        &channel,
        endpoint,
        config,
        auth,
        prompt,
        challenge_len,
    )
}

/// Authentifie puis ouvre la session sur un canal déjà chiffré.
fn logon(
    stream: &mut TcpStream,
    channel: &crypto::Channel,
    endpoint: &Endpoint,
    config: &Config,
    auth: &Auth,
    prompt: &mut dyn FnMut(Guard, bool) -> Result<String, Error>,
    challenge_len: usize,
) -> Result<Session, Error> {
    // 4. Authentification puis ClientLogon. Un compte nominatif obtient
    // d'abord un jeton de refresh du service `Authentication` (mot de passe
    // chiffré en RSA, Steam Guard), puis le CM ne voit que ce jeton.
    let mut refresh_token = None;
    let mut guard_data = None;
    let mut account_name = None;
    let (header_id, body) = match auth {
        Auth::Anonymous => (proto::ANON_STEAM_ID, proto::anonymous_logon()),
        Auth::Account {
            name,
            password,
            guard_data: saved_guard,
        } => {
            let tokens = auth::authenticate(
                stream,
                channel,
                config,
                name,
                password,
                saved_guard.as_deref(),
                prompt,
            )?;
            if tokens.steam_id == 0 {
                return Err(Error::Protocol("authentification sans SteamID".into()));
            }
            refresh_token = Some(tokens.refresh_token.clone());
            guard_data = tokens.guard_data.clone();
            account_name = Some(tokens.account_name.clone());
            (
                tokens.steam_id,
                proto::token_logon(&proto::TokenLogon {
                    refresh_token: &tokens.refresh_token,
                }),
            )
        }
        Auth::Token { name, refresh_token: token } => {
            // Le CM compare l'en-tête au `sub` du jeton : l'identifiant
            // factice d'avant-login est refusé (eresult 5).
            let steam_id = proto::refresh_token_steam_id(token)
                .ok_or_else(|| Error::Auth("jeton de refresh sans SteamID lisible".into()))?;
            tracing::debug!("logon par jeton : compte {name:?}, steamid {steam_id}");
            refresh_token = Some(token.clone());
            account_name = Some(name.clone());
            (
                steam_id,
                proto::token_logon(&proto::TokenLogon {
                    refresh_token: token,
                }),
            )
        }
    };
    let header = proto::proto_header(header_id, 0);
    wire::write_packet(
        stream,
        &channel.encrypt(&wire::proto_message(CLIENT_LOGON, &header, &body)),
    )?;

    let mut session = Session {
        endpoint: endpoint.to_string(),
        anonymous: matches!(auth, Auth::Anonymous),
        challenge_len,
        steam_id: 0,
        session_id: 0,
        eresult: 0,
        heartbeat_seconds: None,
        cell_id: None,
        country: None,
        licenses: None,
        refresh_token,
        guard_data,
        account_name,
    };
    let mut deadline = Instant::now() + config.timeout;
    let mut closed = false;
    loop {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        stream.set_read_timeout(Some((deadline - now).max(Duration::from_millis(1))))?;
        let raw = match wire::read_packet(stream) {
            Ok(raw) => raw,
            Err(Error::Io(error))
                if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) =>
            {
                break;
            }
            // Le CM peut fermer juste après avoir répondu : le SteamID suffit.
            Err(Error::Io(error)) if session.steam_id != 0 && is_disconnect(&error) => break,
            Err(error) => return Err(error),
        };
        let plaintext = channel.decrypt(&raw)?;
        for message in split_incoming(&plaintext)? {
            let Some((emsg, header, body)) = wire::parse_proto(&message) else {
                continue;
            };
            match emsg {
                CLIENT_LOGON_RESPONSE => {
                    let response = proto::parse_logon_response(header, body);
                    session.eresult = response.eresult;
                    session.session_id = response.session_id;
                    session.heartbeat_seconds = response.heartbeat_seconds;
                    session.cell_id = response.cell_id;
                    session.country = response.country;
                    if u32::try_from(response.eresult).ok() == Some(RESULT_OK) {
                        if response.steam_id == 0 {
                            return Err(Error::Protocol("réponse sans SteamID".into()));
                        }
                        session.steam_id = response.steam_id;
                        // Un compte nominatif reçoit sa liste de licences juste
                        // après le logon ; on lui laisse un peu plus de temps
                        // qu'à l'anonyme, qui n'en a aucune.
                        let wait = if session.anonymous {
                            config.license_wait
                        } else {
                            config.license_wait.max(Duration::from_secs(2))
                        };
                        deadline = Instant::now() + wait;
                    } else {
                        let detail = response
                            .eresult_extended
                            .map(|code| format!(", détail {code}"))
                            .unwrap_or_default();
                        return Err(Error::Auth(format!(
                            "connexion refusée : {} (eresult {}){detail}",
                            result_label(response.eresult),
                            response.eresult
                        )));
                    }
                }
                CLIENT_LICENSE_LIST => {
                    session.licenses = Some(proto::parse_license_list(body));
                }
                CLIENT_LOGGED_OFF => {
                    let eresult = proto::parse_logon_response(&[], body).eresult;
                    if session.steam_id == 0 {
                        return Err(Error::Protocol(format!(
                            "session fermée : {} (eresult {eresult})",
                            result_label(eresult)
                        )));
                    }
                    closed = true;
                }
                _ => {}
            }
        }
        if closed || (session.steam_id != 0 && session.licenses.is_some()) {
            break;
        }
    }

    if session.steam_id == 0 {
        return Err(Error::Timeout);
    }
    Ok(session)
}

fn is_disconnect(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        ErrorKind::UnexpectedEof | ErrorKind::ConnectionReset | ErrorKind::BrokenPipe
    )
}

/// Déballe un `Multi` (le CM y regroupe la réponse de connexion).
fn split_incoming(message: &[u8]) -> Result<Vec<Vec<u8>>, Error> {
    if wire::message_id(message) != Some(MULTI) {
        return Ok(vec![message.to_vec()]);
    }
    let (_, _, body) =
        wire::parse_proto(message).ok_or_else(|| Error::Protocol("Multi illisible".into()))?;
    proto::split_multi(body)
}

/// Libellé court d'un `EResult` fréquent.
pub fn result_label(eresult: i32) -> &'static str {
    match eresult {
        1 => "OK",
        5 => "mot de passe invalide",
        9 => "session d'authentification inconnue",
        15 => "jeton expiré ou révoqué",
        27 => "session d'authentification expirée",
        63 => "code Steam Guard requis",
        65 => "code Steam Guard invalide",
        74 => "adresse e-mail à vérifier",
        84 => "trop de tentatives",
        85 => "code Steam Guard (2FA) requis",
        88 => "code 2FA invalide",
        _ => "refus",
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::net::TcpListener;

    fn no_prompt() -> impl FnMut(Guard, bool) -> Result<String, Error> {
        |_, _| Err(Error::Auth("aucun code attendu".into()))
    }

    #[test]
    fn a_refusal_is_held_for_its_account_only() {
        // Le refus d'un compte ne doit pas bloquer un autre nom.
        remember_refusal(Some("bob"));
        assert!(recent_refusal("bob").is_some());
        assert!(recent_refusal("alice").is_none());
        // Une réussite efface le refus.
        remember_refusal(None);
        assert!(recent_refusal("bob").is_none());
    }


    #[test]
    fn account_name_is_read_from_login_users() {
        let text = "\"users\"\n{\n\t\"76561198272259682\"\n\t{\n\t\t\"AccountName\"\t\t\"nitioulol\"\n\t\t\"PersonaName\"\t\t\"Nathounette\"\n\t}\n}\n";
        assert_eq!(
            account_name_in(text, 76561198272259682),
            Some("nitioulol".into())
        );
        assert_eq!(account_name_in(text, 1), None);
        assert_eq!(account_name_in(text, 0), None);
    }

    #[test]
    fn server_list_is_sorted_by_load_and_deduplicated() {
        let value = json!({
            "response": {
                "serverlist": [
                    { "endpoint": "10.0.0.2:27017", "load": 40 },
                    { "endpoint": "10.0.0.1:27017", "load": 3 },
                    { "endpoint": "10.0.0.2:27017", "load": 4 },
                    { "endpoint": "sans-port" },
                    { "load": 1 }
                ]
            }
        });
        let servers = parse_server_list(&value).unwrap();
        assert_eq!(
            servers,
            vec![
                Endpoint {
                    host: "10.0.0.1".into(),
                    port: 27017
                },
                Endpoint {
                    host: "10.0.0.2".into(),
                    port: 27017
                },
            ]
        );
        assert_eq!(servers[0].to_string(), "10.0.0.1:27017");
    }

    #[test]
    fn server_list_rejects_an_empty_response() {
        assert!(matches!(
            parse_server_list(&json!({ "response": { "serverlist": [] } })),
            Err(Error::NoServer)
        ));
        assert!(parse_server_list(&json!({})).is_err());
    }

    #[test]
    fn multi_is_unwrapped() {
        let inner = wire::proto_message(
            CLIENT_LOGON_RESPONSE,
            &proto::proto_header(42, 0),
            &[],
        );
        let mut payload = Vec::new();
        payload.extend_from_slice(&(inner.len() as u32).to_le_bytes());
        payload.extend_from_slice(&inner);
        let mut body = Vec::new();
        proto::field_bytes(2, &payload, &mut body);
        let outer = wire::proto_message(MULTI, &[], &body);
        assert_eq!(split_incoming(&outer).unwrap(), vec![inner.clone()]);
        assert_eq!(split_incoming(&inner).unwrap(), vec![inner]);
    }

    #[test]
    fn labels_cover_the_common_failures() {
        assert_eq!(result_label(1), "OK");
        assert_eq!(result_label(5), "mot de passe invalide");
        assert_eq!(result_label(15), "jeton expiré ou révoqué");
        assert_eq!(result_label(63), "code Steam Guard requis");
        assert_eq!(result_label(85), "code Steam Guard (2FA) requis");
        assert_eq!(result_label(999), "refus");
    }

    #[test]
    fn handshake_response_has_the_expected_shape() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            request.extend_from_slice(&CHANNEL_PROTOCOL.to_le_bytes());
            request.extend_from_slice(&UNIVERSE_PUBLIC.to_le_bytes());
            request.extend_from_slice(&[0xaa; 12]);
            wire::write_packet(
                &mut socket,
                &wire::struct_message(CHANNEL_ENCRYPT_REQUEST, &request),
            )
            .unwrap();

            let message = wire::read_packet(&mut socket).unwrap();
            let (emsg, body) = wire::parse_struct(&message).unwrap();
            assert_eq!(emsg, CHANNEL_ENCRYPT_RESPONSE);
            assert_eq!(body.len(), 8 + 128 + 4 + 4);
            let protocol = u32::from_le_bytes(body[..4].try_into().unwrap());
            let key_size = u32::from_le_bytes(body[4..8].try_into().unwrap());
            assert_eq!((protocol, key_size), (1, 128));
            let crc = u32::from_le_bytes(body[8 + 128..8 + 128 + 4].try_into().unwrap());
            assert_eq!(crc, crypto::crc32(&body[8..8 + 128]));
            // Pas de ChannelEncryptResult : le client doit s'arrêter proprement.
        });

        let endpoint = Endpoint {
            host: "127.0.0.1".into(),
            port,
        };
        let config = Config {
            timeout: Duration::from_secs(2),
            license_wait: Duration::from_millis(10),
            ..Config::default()
        };
        assert!(try_endpoint(&endpoint, &config, &Auth::Anonymous, &mut no_prompt()).is_err());
        server.join().unwrap();
    }

    #[test]
    fn rejects_a_server_that_speaks_another_protocol() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let body = [9u32.to_le_bytes(), UNIVERSE_PUBLIC.to_le_bytes()].concat();
            wire::write_packet(
                &mut socket,
                &wire::struct_message(CHANNEL_ENCRYPT_REQUEST, &body),
            )
            .unwrap();
        });
        let endpoint = Endpoint {
            host: "127.0.0.1".into(),
            port,
        };
        let config = Config {
            timeout: Duration::from_secs(2),
            ..Config::default()
        };
        match try_endpoint(&endpoint, &config, &Auth::Anonymous, &mut no_prompt()) {
            Err(Error::Protocol(message)) => assert!(message.contains("protocole 9")),
            other => panic!("attendu Protocol, obtenu {other:?}"),
        }
        server.join().unwrap();
    }
}
