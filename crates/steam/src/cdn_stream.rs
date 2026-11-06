//! Téléchargement des chunks CDN en flux, pour que le plafond de débit
//! s'applique aux octets eux-mêmes et non à des réservations par chunk.
//!
//! Le corps de la réponse est lu morceau par morceau ; après chaque morceau,
//! le limiteur attend le budget correspondant. Tant qu'il attend, la fenêtre
//! TCP ralentit le CDN : le débit est lisse au lieu d'arriver par salves d'un
//! mégaoctet entrecoupées de longues pauses.

use crate::throttle::DownloadLimit;
use bytes::{Bytes, BytesMut};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use steamroom::cdn::{CdnServer, CdnServerPool};
use steamroom::depot::{ChunkId, DepotId};
use steamroom_client::download::{BoxError, ChunkFetcher};

/// Délai maximal sans un seul octet reçu avant de considérer la connexion
/// morte. Un CDN qui accepte la requête puis se tait bloquait un chunk pour
/// toujours : le moteur gardait alors une seule requête en vol et le
/// téléchargement semblait s'arrêter, avant de repartir au hasard d'un
/// délai TCP. Vingt secondes laissent le temps à un chunk lent d'arriver.
const CHUNK_IDLE_TIMEOUT: Duration = Duration::from_secs(20);

/// Origine des chunks : le pool de serveurs Steam, ou une base directe dans
/// les tests (un [`CdnServer`] ne peut pas être construit hors de steamroom).
enum Source {
    Pool {
        pool: CdnServerPool,
        cdn_auth_token: Option<String>,
    },
    #[cfg(test)]
    Direct(String),
}

/// Fetcher CDN qui lit le corps de la réponse au rythme du plafond.
pub struct StreamingFetcher {
    client: reqwest::Client,
    source: Source,
    limit: DownloadLimit,
    /// Octets réellement reçus du réseau, mis à jour au fil de la lecture :
    /// la jauge de débit avance sans attendre la fin d'un chunk entier.
    received: Arc<AtomicU64>,
    /// Silence maximal entre deux morceaux du corps de la réponse.
    idle_timeout: Duration,
}

impl StreamingFetcher {
    pub fn new(
        pool: CdnServerPool,
        cdn_auth_token: Option<String>,
        limit: DownloadLimit,
        received: Arc<AtomicU64>,
    ) -> Self {
        Self {
            // Une coupure de connexion ne doit pas non plus laisser une
            // requête suspendue indéfiniment. Le délai global est large
            // (le bridage ralentit volontairement la lecture), le vrai
            // garde-fou est le silence entre deux morceaux.
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(15))
                .timeout(Duration::from_secs(120))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            source: Source::Pool {
                pool,
                cdn_auth_token,
            },
            limit,
            received,
            idle_timeout: CHUNK_IDLE_TIMEOUT,
        }
    }

    #[cfg(test)]
    fn direct(base: String, limit: DownloadLimit, received: Arc<AtomicU64>) -> Self {
        Self {
            client: reqwest::Client::new(),
            source: Source::Direct(base),
            limit,
            received,
            idle_timeout: CHUNK_IDLE_TIMEOUT,
        }
    }

    #[cfg(test)]
    fn with_idle_timeout(mut self, idle_timeout: Duration) -> Self {
        self.idle_timeout = idle_timeout;
        self
    }

    fn report_failure(&self, server: Option<&CdnServer>, retry_after: Option<Duration>) {
        if let (Source::Pool { pool, .. }, Some(server)) = (&self.source, server) {
            pool.report_failure(server, retry_after);
        }
    }

    fn report_success(&self, server: Option<&CdnServer>) {
        if let (Source::Pool { pool, .. }, Some(server)) = (&self.source, server) {
            pool.report_success(server);
        }
    }
}

impl ChunkFetcher for StreamingFetcher {
    async fn fetch_chunk(
        &self,
        depot_id: DepotId,
        chunk_id: &ChunkId,
    ) -> std::result::Result<Bytes, BoxError> {
        let path = format!("/depot/{}/chunk/{}", depot_id.0, chunk_id);
        let (url, server) = match &self.source {
            Source::Pool {
                pool,
                cdn_auth_token,
            } => {
                let (server, wait) = pool.pick();
                if !wait.is_zero() {
                    tokio::time::sleep(wait).await;
                }
                (
                    server.build_url(&path, cdn_auth_token.as_deref()),
                    Some(server),
                )
            }
            #[cfg(test)]
            Source::Direct(base) => (format!("{base}{path}"), None),
        };

        let response = match self.client.get(&url).send().await {
            Ok(response) => response,
            Err(error) => {
                self.report_failure(server, None);
                return Err(format!("CDN : {error}").into());
            }
        };
        if response.status() != reqwest::StatusCode::OK {
            let status = response.status();
            let retry_after = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok())
                .map(Duration::from_secs);
            self.report_failure(server, retry_after);
            return Err(format!("CDN : HTTP {status}").into());
        }

        let expected = response.content_length();
        let mut response = response;
        let mut data = BytesMut::with_capacity(expected.unwrap_or(0) as usize);
        loop {
            let piece = match tokio::time::timeout(self.idle_timeout, response.chunk()).await {
                Ok(Ok(Some(piece))) => piece,
                Ok(Ok(None)) => break,
                Ok(Err(error)) => {
                    self.report_failure(server, None);
                    return Err(format!("CDN : {error}").into());
                }
                Err(_) => {
                    // Le serveur ne répond plus : on libère le chunk et le
                    // pool le met en quarantaine, plutôt que de bloquer le
                    // dépôt entier sur une connexion morte.
                    self.report_failure(server, None);
                    return Err(format!(
                        "CDN : aucun octet reçu pendant {} s",
                        self.idle_timeout.as_secs().max(1)
                    )
                    .into());
                }
            };
            // Le compteur réseau avance dès la réception : sans cela, un
            // chunk bridé et long à recevoir laissait la jauge à zéro.
            self.received
                .fetch_add(piece.len() as u64, Ordering::Relaxed);
            // Le morceau vient d'être lu : on attend son budget avant de
            // demander le suivant. Le CDN reste ainsi freiné par TCP au lieu
            // de vider sa réponse d'un coup.
            self.limit.wait(piece.len() as u64).await;
            data.extend_from_slice(&piece);
        }
        if let Some(expected) = expected {
            if data.len() as u64 != expected {
                self.report_failure(server, None);
                return Err(format!(
                    "CDN : {} octets reçus sur {}",
                    data.len(),
                    expected
                )
                .into());
            }
        }
        self.report_success(server);
        Ok(data.freeze())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Instant;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Serveur HTTP minimal : une requête, une réponse complète, puis fermeture.
    async fn serve_once(body: Vec<u8>) -> (u16, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("port de test");
        let port = listener.local_addr().expect("adresse locale").port();
        let handle = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("connexion");
            let mut buf = [0u8; 4096];
            let _ = socket.read(&mut buf).await;
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            socket.write_all(head.as_bytes()).await.expect("en-tête");
            socket.write_all(&body).await.expect("corps");
            socket.flush().await.expect("envoi");
        });
        (port, handle)
    }

    #[tokio::test]
    async fn streamed_chunks_respect_the_cap_and_arrive_whole() {
        let body = vec![7u8; 512 * 1024];
        let (port, server) = serve_once(body.clone()).await;
        let received = Arc::new(AtomicU64::new(0));
        let fetcher = StreamingFetcher::direct(
            format!("http://127.0.0.1:{port}"),
            DownloadLimit {
                global: None,
                store: Some(crate::throttle::RateLimiter::new(1_000_000)),
            },
            Arc::clone(&received),
        );
        let started = Instant::now();
        let data = fetcher
            .fetch_chunk(DepotId(7), &ChunkId([1; 20]))
            .await
            .expect("chunk complet");
        let elapsed = started.elapsed();
        assert_eq!(data.len(), body.len());
        assert_eq!(data.as_ref(), body.as_slice());
        assert_eq!(received.load(Ordering::Relaxed), body.len() as u64);
        // 512 Kio à 1 Mo/s valent ~524 ms ; on laisse de la marge pour un
        // ordonnanceur chargé, tout en refusant une arrivée instantanée.
        assert!(
            elapsed >= Duration::from_millis(450),
            "le plafond doit s'appliquer au flux : {elapsed:?}"
        );
        server.await.expect("serveur de test");
    }

    /// En-tête puis silence : la connexion reste ouverte sans jamais envoyer
    /// le corps, comme un CDN qui a accepté la requête puis s'est tu.
    async fn serve_stalled() -> (u16, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("port de test");
        let port = listener.local_addr().expect("adresse locale").port();
        let handle = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("connexion");
            let mut buf = [0u8; 4096];
            let _ = socket.read(&mut buf).await;
            let head = "HTTP/1.1 200 OK\r\nContent-Length: 1024\r\nConnection: close\r\n\r\n";
            socket.write_all(head.as_bytes()).await.expect("en-tête");
            socket.flush().await.expect("envoi");
            tokio::time::sleep(Duration::from_secs(5)).await;
        });
        (port, handle)
    }

    #[tokio::test]
    async fn a_silent_cdn_releases_the_chunk_instead_of_hanging() {
        // Un serveur muet ne doit pas immobiliser le dépôt : le chunk échoue
        // après le délai de silence, et le pool pourra choisir un autre hôte.
        let (port, server) = serve_stalled().await;
        let fetcher = StreamingFetcher::direct(
            format!("http://127.0.0.1:{port}"),
            DownloadLimit::unlimited(),
            Arc::new(AtomicU64::new(0)),
        )
        .with_idle_timeout(Duration::from_millis(150));
        let started = Instant::now();
        let error = fetcher
            .fetch_chunk(DepotId(7), &ChunkId([5; 20]))
            .await
            .expect_err("un flux muet doit échouer");
        assert!(error.to_string().contains("aucun octet"));
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "le chunk doit être libéré rapidement : {:?}",
            started.elapsed()
        );
        server.abort();
    }

    /// Plusieurs réponses servies en parallèle, chacune sur sa connexion.
    async fn serve_many(
        body: Vec<u8>,
        count: usize,
    ) -> (u16, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("port de test");
        let port = listener.local_addr().expect("adresse locale").port();
        let handle = tokio::spawn(async move {
            let mut writers = Vec::new();
            for _ in 0..count {
                let (mut socket, _) = listener.accept().await.expect("connexion");
                let body = body.clone();
                writers.push(tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let _ = socket.read(&mut buf).await;
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    socket.write_all(head.as_bytes()).await.expect("en-tête");
                    socket.write_all(&body).await.expect("corps");
                    socket.flush().await.expect("envoi");
                }));
            }
            for writer in writers {
                let _ = writer.await;
            }
        });
        (port, handle)
    }

    #[tokio::test]
    async fn concurrent_fetches_share_the_same_cap() {
        // Huit chunks de 128 Kio à 1 Mo/s valent 1,05 s au total, et non
        // 0,13 s : le limiteur est commun à toutes les connexions. C'était le
        // défaut de l'ancienne réservation par chunk, qui autorisait trente-
        // deux requêtes en vol et dépassait le plafond.
        let body = vec![9u8; 128 * 1024];
        let (port, server) = serve_many(body.clone(), 8).await;
        let limiter = crate::throttle::RateLimiter::new(1_000_000);
        let received = Arc::new(AtomicU64::new(0));
        let started = Instant::now();
        let mut tasks = Vec::new();
        for _ in 0..8 {
            let fetcher = StreamingFetcher::direct(
                format!("http://127.0.0.1:{port}"),
                DownloadLimit {
                    global: Some(Arc::clone(&limiter)),
                    store: None,
                },
                Arc::clone(&received),
            );
            tasks.push(tokio::spawn(async move {
                fetcher
                    .fetch_chunk(DepotId(7), &ChunkId([4; 20]))
                    .await
            }));
        }
        for task in tasks {
            let data = task.await.expect("tâche terminée").expect("chunk complet");
            assert_eq!(data.len(), body.len());
        }
        assert_eq!(received.load(Ordering::Relaxed), 8 * body.len() as u64);
        let elapsed = started.elapsed();
        assert!(
            elapsed >= Duration::from_millis(900),
            "le plafond doit être partagé entre les connexions : {elapsed:?}"
        );
        assert!(
            elapsed < Duration::from_secs(3),
            "le plafond ne doit pas non plus bloquer : {elapsed:?}"
        );
        server.await.expect("serveur de test");
    }

    #[tokio::test]
    async fn an_unlimited_fetch_does_not_wait() {
        let body = vec![3u8; 64 * 1024];
        let (port, server) = serve_once(body.clone()).await;
        let received = Arc::new(AtomicU64::new(0));
        let fetcher = StreamingFetcher::direct(
            format!("http://127.0.0.1:{port}"),
            DownloadLimit::unlimited(),
            Arc::clone(&received),
        );
        let started = Instant::now();
        let data = fetcher
            .fetch_chunk(DepotId(7), &ChunkId([2; 20]))
            .await
            .expect("chunk complet");
        assert_eq!(data.len(), body.len());
        assert_eq!(received.load(Ordering::Relaxed), body.len() as u64);
        assert!(
            started.elapsed() < Duration::from_millis(300),
            "sans plafond, aucune attente : {:?}",
            started.elapsed()
        );
        server.await.expect("serveur de test");
    }

    /// Mesure réelle sur Internet : 20 Mo téléchargés par quatre fetchers
    /// partageant un plafond de 20 Mbit/s, comme le moteur qui ouvre environ
    /// deux secondes de chunks en parallèle. Ignoré par défaut puisqu'il
    /// dépend du réseau ; à lancer avec `--ignored --nocapture`.
    #[tokio::test]
    #[ignore = "télécharge 20 Mo sur Internet"]
    async fn wan_cap_holds_at_twenty_mbit() {
        let bytes = 5_000_000u64;
        let limiter = crate::throttle::RateLimiter::new(20_000_000 / 8);
        let received = Arc::new(AtomicU64::new(0));
        let started = Instant::now();
        let mut tasks = Vec::new();
        for _ in 0..4 {
            let fetcher = StreamingFetcher::direct(
                format!("https://speed.cloudflare.com/__down?bytes={bytes}&x="),
                DownloadLimit {
                    global: Some(Arc::clone(&limiter)),
                    store: None,
                },
                Arc::clone(&received),
            );
            tasks.push(tokio::spawn(async move {
                fetcher
                    .fetch_chunk(DepotId(7), &ChunkId([5; 20]))
                    .await
            }));
        }
        let mut total = 0usize;
        for task in tasks {
            let data = task
                .await
                .expect("tâche terminée")
                .expect("téléchargement de test");
            assert_eq!(data.len() as u64, bytes);
            total += data.len();
        }
        let elapsed = started.elapsed().as_secs_f64();
        let mbit = total as f64 * 8.0 / 1_000_000.0 / elapsed;
        eprintln!("20 Mo à 20 Mbit/s : {elapsed:.2} s, {mbit:.1} Mbit/s mesurés");
        assert_eq!(received.load(Ordering::Relaxed), 20_000_000);
        // La poignée de main TLS et le démarrage lent de TCP coûtent la
        // première seconde : on accepte 14 à 24 Mbit/s sur 8 s théoriques.
        assert!(
            (14.0..=24.0).contains(&mbit),
            "le débit doit rester proche de 20 Mbit/s : {mbit:.2}"
        );
    }

}
