//! Bridage du débit des téléchargements.
//!
//! Le plafond s'applique aux octets **au fil de l'eau** : le fetcher CDN lit
//! le corps de la réponse par petits morceaux et attend le budget de chacun
//! avant de demander le suivant. Réserver un chunk entier avant la requête
//! produisait des salves d'un mégaoctet suivies de longues pauses — impression
//! que le téléchargement s'arrête — et laissait trente-deux requêtes en vol,
//! bien au-dessus du plafond. Ici, le seau à jetons est partagé par toutes les
//! lectures : le débit total est lissé et respecte le plafond à quelques
//! pour cent près.
//!
//! Un limiteur est partagé par tous les chunks d'un même groupe : le plafond
//! global est commun à tous les stores, celui du store à toutes ses tâches.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// Débit maximal partagé, en octets par seconde. 0 = illimité.
pub struct RateLimiter {
    bytes_per_sec: AtomicU64,
    /// Jetons disponibles, ou dette négative des lectures déjà réservées, et
    /// instant du dernier remplissage. La réservation est brève : l'attente
    /// se fait hors du verrou, en parallèle entre les connexions.
    state: Mutex<TokenBucket>,
}

struct TokenBucket {
    tokens: f64,
    last_refill: Instant,
}

impl RateLimiter {
    pub fn new(bytes_per_sec: u64) -> Arc<Self> {
        Arc::new(Self {
            bytes_per_sec: AtomicU64::new(bytes_per_sec),
            state: Mutex::new(TokenBucket {
                tokens: 0.0,
                last_refill: Instant::now(),
            }),
        })
    }

    /// Change le plafond sans remplacer le limiteur : les téléchargements en
    /// cours voient la nouvelle valeur à la lecture suivante.
    pub fn set_rate(&self, bytes_per_sec: u64) {
        self.bytes_per_sec.store(bytes_per_sec, Ordering::Relaxed);
    }

    pub fn rate(&self) -> u64 {
        self.bytes_per_sec.load(Ordering::Relaxed)
    }

    /// Consomme le budget de `bytes` octets, en attendant le remplissage du
    /// seau. Appelée après chaque morceau reçu : tant qu'elle attend, le
    /// fetcher ne redemande rien, la fenêtre TCP ralentit l'émetteur et le
    /// débit se lisse au lieu d'arriver par paquets.
    ///
    /// Le budget est **réservé** sous le verrou, puis l'attente se fait hors
    /// du verrou. Garder le verrou pendant le sommeil sérialisait tous les
    /// fetchers : un seul pouvait consommer son budget à la fois, et les
    /// trente-deux connexions avançaient par à-coups. Ici chacune réserve sa
    /// part et attend la sienne en parallèle, ce qui garde un débit régulier.
    pub async fn wait(&self, bytes: u64) {
        if bytes == 0 {
            return;
        }
        let want = bytes as f64;
        let (mut owed, mut rate) = {
            let rate = self.bytes_per_sec.load(Ordering::Relaxed);
            if rate == 0 {
                return;
            }
            let rate = rate as f64;
            // Une rafale de 100 ms au plus, jamais moins que l'unité
            // demandée : une lecture plus grosse que le seau ne doit pas
            // attendre indéfiniment.
            let burst = (rate * 0.1).max(want);
            let mut bucket = self.state.lock().await;
            let now = Instant::now();
            let elapsed = now.duration_since(bucket.last_refill).as_secs_f64();
            bucket.last_refill = now;
            bucket.tokens = (bucket.tokens.min(burst) + elapsed * rate).min(burst);
            bucket.tokens -= want;
            // Le seau peut descendre sous zéro : la dette représente les
            // octets déjà réservés par les autres fetchers, pas un dépassement.
            let owed = if bucket.tokens < 0.0 {
                -bucket.tokens / rate
            } else {
                0.0
            };
            (owed, rate)
        };
        // Découpé en tranches courtes pour qu'un changement de plafond soit
        // pris en compte sans attendre la fin d'une longue dette.
        while owed > 0.0 {
            let current = self.bytes_per_sec.load(Ordering::Relaxed);
            if current == 0 {
                return;
            }
            let current = current as f64;
            if (current - rate).abs() > f64::EPSILON {
                owed *= rate / current;
                rate = current;
            }
            let slice = owed.min(0.05);
            tokio::time::sleep(Duration::from_secs_f64(slice)).await;
            owed -= slice;
        }
    }
}

/// Les deux plafonds qui encadrent un store : le global et le sien.
#[derive(Clone, Default)]
pub struct DownloadLimit {
    pub global: Option<Arc<RateLimiter>>,
    pub store: Option<Arc<RateLimiter>>,
}

impl DownloadLimit {
    pub fn unlimited() -> Self {
        Self::default()
    }

    /// Les deux limites courent en parallèle : attendre la plus stricte, et
    /// non la somme des deux délais.
    pub async fn wait(&self, bytes: u64) {
        match (&self.global, &self.store) {
            (Some(global), Some(store)) => {
                tokio::join!(global.wait(bytes), store.wait(bytes));
            }
            (Some(limit), None) | (None, Some(limit)) => limit.wait(bytes).await,
            (None, None) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unlimited_is_immediate() {
        let limit = DownloadLimit::unlimited();
        let started = Instant::now();
        limit.wait(50 * 1024 * 1024).await;
        assert!(started.elapsed() < Duration::from_millis(50));
    }

    #[tokio::test]
    async fn a_limited_transfer_takes_the_expected_time() {
        // 64 Kio à 4 Mio/s : 16 ms de budget.
        let limit = DownloadLimit {
            global: None,
            store: Some(RateLimiter::new(4 * 1024 * 1024)),
        };
        let started = Instant::now();
        limit.wait(64 * 1024).await;
        assert!(started.elapsed() >= Duration::from_millis(12));
    }

    #[tokio::test]
    async fn consecutive_reads_share_one_budget() {
        // Deux lectures de 100 Ko à 2 Mo/s valent 50 ms chacune : le budget
        // n'est pas remis à zéro entre les deux, sinon le plafond doublerait.
        let limiter = RateLimiter::new(2_000_000);
        let started = Instant::now();
        limiter.wait(100_000).await;
        limiter.wait(100_000).await;
        assert!(
            started.elapsed() >= Duration::from_millis(90),
            "deux lectures doivent payer leur budget : {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn an_idle_bucket_does_not_bank_more_than_its_burst() {
        // Après une longue pause, le seau ne rend pas plusieurs secondes de
        // budget : la première lecture passe grâce à la rafale (bornée à
        // l'unité demandée), la suivante paie son dû.
        let limiter = RateLimiter::new(1_000_000);
        tokio::time::sleep(Duration::from_millis(400)).await;
        let started = Instant::now();
        limiter.wait(200_000).await;
        assert!(
            started.elapsed() < Duration::from_millis(80),
            "la rafale doit couvrir la première lecture : {:?}",
            started.elapsed()
        );
        limiter.wait(200_000).await;
        assert!(
            started.elapsed() >= Duration::from_millis(150),
            "la seconde lecture doit attendre son budget : {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn two_caps_do_not_add_their_delays() {
        // 64 Kio à 4 Mio/s vaut 16 ms ; deux plafonds identiques doivent
        // donner ~16 ms, pas 32 ms.
        let limit = DownloadLimit {
            global: Some(RateLimiter::new(4 * 1024 * 1024)),
            store: Some(RateLimiter::new(4 * 1024 * 1024)),
        };
        let started = Instant::now();
        limit.wait(64 * 1024).await;
        let elapsed = started.elapsed();
        assert!(elapsed >= Duration::from_millis(12), "{elapsed:?}");
        assert!(elapsed < Duration::from_millis(40), "{elapsed:?}");
    }
}
