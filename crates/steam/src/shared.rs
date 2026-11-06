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
    if let Ok(mut slot) = rate_limit().lock() {
        *slot = Some(RateLimited {
            account: account.to_string(),
            until: Instant::now() + RATE_LIMIT_HOLD,
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

/// Session empruntée : elle tient le verrou du pool jusqu'à sa libération.
pub struct Session {
    client: Arc<SteamClient<LoggedIn>>,
    _guard: OwnedMutexGuard<Option<Cached>>,
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
                    }
                    return Err(error);
                }
            };
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
    client.pics_get_access_tokens(&[AppId(PING_APP)]).await.is_ok()
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
