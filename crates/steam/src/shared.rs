//! Session Steam partagée par toutes les lectures.
//!
//! Steam ne garde **qu'une session par compte** : deux logons rapprochés se
//! ferment l'un l'autre, et une rafale finit par un refus temporaire (`mot de
//! passe invalide`, eresult 5) même avec un jeton valide. Chaque lecture
//! ouvrait pourtant la sienne — une par fiche ouverte, une par synchronisation,
//! une par demande de succès.
//!
//! Une seule session est donc gardée ici, réutilisée tant que le canal répond
//! et que la même identité est demandée. Le verrou du pool est tenu pendant
//! toute la lecture : les réponses du CM arrivent sur un seul canal, deux
//! lectures simultanées se mêleraient.

use std::ops::Deref;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use steamroom::client::{LoggedIn, SteamClient};
use steamroom::depot::AppId;
use tokio::sync::{Mutex, OwnedMutexGuard};

use crate::Result;

/// Âge au-delà duquel la session est rouverte sans attendre une panne : un CM
/// peut fermer un canal resté longtemps sans trafic.
const SESSION_TTL: Duration = Duration::from_secs(20 * 60);

/// Application témoin d'un canal encore vivant (Team Fortress 2 : son jeton
/// PICS est public, la réponse ne dépend d'aucune licence).
const PING_APP: u32 = 220;

/// Un refus 84 est une limite temporaire de Steam, pas un jeton mort. Chaque
/// nouvelle tentative la prolonge : toutes les lectures partagent donc la même
/// pause au lieu de rouvrir une session à chaque requête.
const RATE_LIMIT_HOLD: Duration = Duration::from_secs(30 * 60);

/// Pause après un jeton refusé (EResult 5) : le jeton reste bon, mais Steam
/// pénalise les logons répétés. Relancer tout de suite prolonge le refus.
const TOKEN_REFUSED_HOLD: Duration = Duration::from_secs(90);

/// Compte actuellement bloqué par Steam, avec la fin de la pause.
struct RateLimited {
    account: String,
    until: Instant,
}

fn rate_limit() -> &'static std::sync::Mutex<Option<RateLimited>> {
    static LIMITED: OnceLock<std::sync::Mutex<Option<RateLimited>>> = OnceLock::new();
    LIMITED.get_or_init(|| std::sync::Mutex::new(None))
}

/// Secondes restantes avant de pouvoir retenter ce compte.
fn rate_limit_wait(account: &str) -> Option<u64> {
    let mut slot = rate_limit().lock().ok()?;
    let limited = slot.as_ref()?;
    if limited.account != account {
        return None;
    }
    let left = limited.until.saturating_duration_since(Instant::now());
    if left.is_zero() {
        *slot = None;
        return None;
    }
    Some(left.as_secs().max(1))
}

fn remember_rate_limit(account: &str) {
    remember_hold(account, RATE_LIMIT_HOLD);
}

fn remember_hold(account: &str, hold: Duration) {
    if let Ok(mut slot) = rate_limit().lock() {
        *slot = Some(RateLimited {
            account: account.to_string(),
            until: Instant::now() + hold,
        });
    }
}

fn clear_rate_limit(account: &str) {
    if let Ok(mut slot) = rate_limit().lock() {
        if slot
            .as_ref()
            .map(|limited| limited.account == account)
            .unwrap_or(false)
        {
            *slot = None;
        }
    }
}

/// Session gardée, avec l'identité qui l'a ouverte.
struct Cached {
    key: String,
    client: Arc<SteamClient<LoggedIn>>,
    opened: Instant,
}

fn pool() -> &'static Arc<Mutex<Option<Cached>>> {
    static POOL: OnceLock<Arc<Mutex<Option<Cached>>>> = OnceLock::new();
    POOL.get_or_init(|| Arc::new(Mutex::new(None)))
}

/// Un téléchargement tient une session à lui seul : `install` ne peut pas
/// partager le canal du pool, et Steam ferme l'une des deux connexions quand
/// deux logons du même compte se suivent. Les lectures ordinaires s'effacent
/// donc pendant l'installation au lieu de rouvrir une session qui la tuerait.
static INSTALLING: AtomicBool = AtomicBool::new(false);

fn install_pool() -> &'static Arc<Mutex<()>> {
    static POOL: OnceLock<Arc<Mutex<()>>> = OnceLock::new();
    POOL.get_or_init(|| Arc::new(Mutex::new(())))
}

/// Garde d'installation : elle sérialise les téléchargements et empêche les
/// autres lectures Steam d'ouvrir une connexion concurrente.
pub struct InstallGuard {
    _slot: OwnedMutexGuard<()>,
}

impl Drop for InstallGuard {
    fn drop(&mut self) {
        INSTALLING.store(false, Ordering::SeqCst);
    }
}

/// Prend la main sur Steam pour un téléchargement : attend la fin des lectures
/// en cours, ferme la session partagée, puis interdit les nouvelles lectures
/// jusqu'à la fin de l'installation.
pub async fn begin_install() -> InstallGuard {
    let slot = Arc::clone(install_pool()).lock_owned().await;
    INSTALLING.store(true, Ordering::SeqCst);
    // `forget` attend que la session empruntée soit rendue : le téléchargement
    // ne peut pas être coupé par une lecture encore en vol.
    forget().await;
    InstallGuard { _slot: slot }
}

fn install_in_progress() -> bool {
    INSTALLING.load(Ordering::SeqCst)
}

/// Moteur réseau des lectures, vivant pour tout le processus.
///
/// Une connexion Steam appartient au runtime qui l'a ouverte : la session
/// gardée dans une lecture était donc morte avec le runtime jetable de cette
/// lecture, et la « session réutilisée » était en réalité déconnectée — d'où
/// un logon à chaque fiche. Toutes les lectures tournent désormais ici.
fn engine() -> &'static tokio::runtime::Runtime {
    static ENGINE: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    ENGINE.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(3)
            .enable_all()
            .build()
            .expect("moteur réseau Steam")
    })
}

/// Exécute une lecture Steam sur le moteur commun. Sans lui, chaque appel
/// ouvrait son propre runtime et la session gardée mourait avec lui.
pub fn block_on<F: std::future::Future>(future: F) -> F::Output {
    engine().block_on(future)
}

/// Inactivité au-delà de laquelle la session sociale peut reprendre la main.
///
/// Steam ne garde qu'une session par compte : une connexion ouverte pendant
/// qu'une lecture travaille ferme celle de la lecture (et réciproquement), ce
/// qui faisait échouer les copies de sauvegarde en pleine opération. Le pool
/// annonce donc son usage, et la session sociale attend qu'il soit retombé.
const POOL_IDLE: Duration = Duration::from_secs(60);

/// Temps laissé à la session sociale pour rendre la main après une réclamation.
const SESSION_HANDOFF: Duration = Duration::from_secs(3);

/// Durée pendant laquelle une réclamation reste valable si le pool échoue.
const WANT_WINDOW: Duration = Duration::from_secs(30);

fn last_use() -> &'static std::sync::Mutex<Option<Instant>> {
    static LAST: OnceLock<std::sync::Mutex<Option<Instant>>> = OnceLock::new();
    LAST.get_or_init(|| std::sync::Mutex::new(None))
}

fn touch() {
    if let Ok(mut slot) = last_use().lock() {
        *slot = Some(Instant::now());
    }
}

/// Note un jeton refusé venu d'un autre client (la session sociale) : la pause
/// s'applique alors à toutes les lectures.
pub fn note_logon_refused(account: &str) {
    remember_hold(account, TOKEN_REFUSED_HOLD);
}

/// Secondes de pause restantes pour ce compte, s'il est en pénalité. La
/// session sociale s'y plie aussi : un logon de plus la prolongerait.
pub fn logon_hold_seconds(account: &str) -> Option<u64> {
    rate_limit_wait(account)
}

/// Le pool a-t-il lu quelque chose récemment ? La session sociale ne se
/// connecte que lorsque la réponse est non : ouvrir une connexion maintenant
/// fermerait celle d'une lecture en cours.
/// Réclamation de la session unique par le pool, avec son échéance.
fn wanted() -> &'static std::sync::Mutex<Option<Instant>> {
    static WANTED: OnceLock<std::sync::Mutex<Option<Instant>>> = OnceLock::new();
    WANTED.get_or_init(|| std::sync::Mutex::new(None))
}

fn claim_session() {
    if let Ok(mut slot) = wanted().lock() {
        *slot = Some(Instant::now() + WANT_WINDOW);
    }
}

fn session_claimed() {
    if let Ok(mut slot) = wanted().lock() {
        *slot = None;
    }
}

/// Le pool réclame-t-il la session ? La session sociale la rend alors d'elle-même
/// au lieu d'attendre une coupure : Steam refuse sinon son logon
/// (`mot de passe invalide`) tant qu'une autre session tient le compte.
pub fn pool_wants_session() -> bool {
    wanted()
        .lock()
        .ok()
        .and_then(|slot| *slot)
        .map(|until| Instant::now() < until)
        .unwrap_or(false)
}

pub fn pool_busy() -> bool {
    last_use()
        .lock()
        .ok()
        .and_then(|slot| *slot)
        .map(|at| at.elapsed() < POOL_IDLE)
        .unwrap_or(false)
}

/// Session empruntée : elle tient le verrou du pool jusqu'à sa libération.
pub struct Session {
    client: Arc<SteamClient<LoggedIn>>,
    _guard: OwnedMutexGuard<Option<Cached>>,
}

impl Drop for Session {
    fn drop(&mut self) {
        touch();
    }
}

impl Deref for Session {
    type Target = SteamClient<LoggedIn>;

    fn deref(&self) -> &Self::Target {
        self.client.as_ref()
    }
}

/// Session partagée pour ce compte et ce jeton.
///
/// La session existante est rendue si elle a été ouverte pour la même identité,
/// qu'elle a moins de vingt minutes et que son canal répond encore ; sinon une
/// seule connexion est ouverte, et les appels suivants l'attendront.
pub async fn session(account: &str, refresh_token: &str) -> Result<Session> {
    if let Some(seconds) = rate_limit_wait(account) {
        return Err(crate::Error::Other(format!(
            "Steam limite les tentatives pour ce compte ; nouvel essai dans {seconds} s"
        )));
    }
    touch();
    let key = format!("{account}\u{0}{refresh_token}");
    let mut slot = Arc::clone(pool()).lock_owned().await;
    if install_in_progress() {
        return Err(crate::Error::Other(
            "téléchargement Steam en cours : les lectures reprendront à la fin".into(),
        ));
    }
    let reusable = slot
        .as_ref()
        .filter(|cached| cached.key == key && cached.opened.elapsed() < SESSION_TTL)
        .map(|cached| Arc::clone(&cached.client));
    let mut client: Option<Arc<SteamClient<LoggedIn>>> = None;
    if let Some(cached) = reusable {
        if alive(&cached).await {
            tracing::debug!("session Steam réutilisée");
            client = Some(cached);
        }
    }
    let client = match client {
        Some(client) => client,
        None => {
            *slot = None;
            tracing::info!("session Steam ouverte pour {account}");
            let logged = match crate::native::login(account, refresh_token).await {
                Ok(client) => client,
                Err(error) => {
                    if error.is_rate_limited() {
                        remember_rate_limit(account);
                        return Err(error);
                    }
                    if !error.is_token_refused() {
                        return Err(error);
                    }
                    // Une seule session par compte : la présence sociale la
                    // tient peut-être encore. On la réclame, on la laisse se
                    // rendre, puis on retente une fois.
                    claim_session();
                    tokio::time::sleep(SESSION_HANDOFF).await;
                    match crate::native::login(account, refresh_token).await {
                        Ok(client) => client,
                        Err(again) => {
                            // Refus persistant : pénalité de Steam. Chaque
                            // tentative de plus la prolonge, on laisse passer.
                            remember_hold(account, TOKEN_REFUSED_HOLD);
                            session_claimed();
                            return Err(again);
                        }
                    }
                }
            };
            session_claimed();
            clear_rate_limit(account);
            let opened = Arc::new(logged);
            *slot = Some(Cached {
                key,
                client: Arc::clone(&opened),
                opened: Instant::now(),
            });
            opened
        }
    };
    Ok(Session {
        client,
        _guard: slot,
    })
}

/// Le canal répond-il encore ? Une requête minuscule coûte bien moins qu'un
/// logon, et c'est le seul moyen de savoir sans attendre l'échec d'une vraie
/// lecture.
async fn alive(client: &SteamClient<LoggedIn>) -> bool {
    client
        .pics_get_access_tokens(&[AppId(PING_APP)])
        .await
        .is_ok()
}

/// Vérifie le jeton sans ouvrir de connexion de plus : la session partagée est
/// réutilisée si elle existe, sinon ouverte — et c'est alors celle que toutes
/// les lectures emploieront. Un contrôle de jeton ne coûte donc plus un logon
/// séparé, qui éjectait la session des amis.
pub fn verify_blocking(account: &str, refresh_token: &str) -> Result<()> {
    block_on(async { session(account, refresh_token).await.map(|_| ()) })
}

/// Oublie la session gardée : le prochain appel en ouvrira une neuve. À
/// utiliser quand Steam a explicitement refusé le jeton.
pub async fn forget() {
    let mut slot = Arc::clone(pool()).lock_owned().await;
    *slot = None;
}

/// Rend la seule session Steam à la session sociale.
///
/// Steam n'en autorise qu'une par compte : garder la connexion du pool ouverte
/// alors qu'il ne lit plus empêche la présence de se reconnecter (`session
/// fermée par Steam`, puis des trames illisibles). Fermer la connexion oisive
/// laisse donc la place libre ; le pool en rouvrira une à sa prochaine lecture.
/// Rend `false` si une lecture est en cours ou vient d'avoir lieu.
pub async fn release_if_idle() -> bool {
    if pool_busy() {
        return false;
    }
    forget().await;
    true
}
