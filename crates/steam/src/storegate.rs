//! Porte d'entrée des lectures publiques de la boutique Steam.
//!
//! Akamai ferme la boutique quand les fiches partent en rafale : une
//! synchronisation pouvait envoyer 271 appels de paquets d'affilée, puis
//! insister malgré les 403. Cette porte impose un espacement entre les
//! requêtes, coupe la boutique après trois refus consécutifs et retient la
//! coupure sur disque — un redémarrage ne repart pas au galop, et une
//! synchronisation qui tombe sur un refus s'arrête au lieu de marteler.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Espacement minimal entre deux requêtes, toutes tâches confondues. Quatre
/// requêtes de front sans espacement, c'est une rafale aux yeux d'Akamai.
const SPACING: Duration = Duration::from_millis(350);
/// Refus consécutifs avant de fermer la boutique pour un moment.
const TRIP_AFTER: u32 = 3;
/// Repos appliqué quand Steam ne donne pas de `Retry-After`.
const COOLDOWN: Duration = Duration::from_secs(900);
/// Repos minimal, même si Steam demande moins : un 403 d'Akamai se rouvre
/// rarement en quelques secondes.
const MIN_REST: Duration = Duration::from_secs(120);

/// Ce que la porte répond à une requête.
pub(crate) enum Verdict {
    /// La requête peut partir.
    Go,
    /// La boutique se repose : ne rien envoyer, servir le cache.
    Resting,
}

struct Gate {
    /// Instant du prochain départ autorisé.
    next: Mutex<Instant>,
    /// Refus consécutifs depuis le dernier succès.
    refusals: AtomicU32,
    /// Fin du repos imposé, si la boutique a fermé la porte.
    rest_until: Mutex<Option<Instant>>,
}

fn gate() -> &'static Gate {
    static GATE: OnceLock<Gate> = OnceLock::new();
    GATE.get_or_init(|| Gate {
        next: Mutex::new(Instant::now()),
        refusals: AtomicU32::new(0),
        rest_until: Mutex::new(read_rest()),
    })
}

/// Demande le droit de partir. Attendre fait partie du contrat : la tâche
/// appelante est mise en sommeil jusqu'à son tour.
pub(crate) async fn permit() -> Verdict {
    let gate = gate();
    {
        let mut rest = gate.rest_until.lock().unwrap_or_else(|err| err.into_inner());
        if let Some(until) = *rest {
            if Instant::now() < until {
                return Verdict::Resting;
            }
            *rest = None;
            clear_rest();
        }
    }
    let wait = {
        let mut next = gate.next.lock().unwrap_or_else(|err| err.into_inner());
        let now = Instant::now();
        let start = (*next).max(now);
        *next = start + SPACING;
        start.saturating_duration_since(now)
    };
    if wait > Duration::ZERO {
        tokio::time::sleep(wait).await;
    }
    Verdict::Go
}

/// La boutique a répondu : le compteur de refus repart de zéro.
pub(crate) fn accepted() {
    gate().refusals.store(0, Ordering::Relaxed);
}

/// La boutique a refusé (403, 429). Trois refus d'affilée la mettent au repos,
/// avec le délai qu'elle annonce quand elle en annonce un.
pub(crate) fn refused(retry_after: Option<Duration>) {
    let gate = gate();
    let count = gate.refusals.fetch_add(1, Ordering::Relaxed) + 1;
    tracing::debug!("boutique : refus {count}/{TRIP_AFTER}");
    if count < TRIP_AFTER {
        return;
    }
    let rest = retry_after.unwrap_or(COOLDOWN).max(MIN_REST);
    let until = Instant::now() + rest;
    *gate.rest_until.lock().unwrap_or_else(|err| err.into_inner()) = Some(until);
    gate.refusals.store(0, Ordering::Relaxed);
    write_rest(rest);
    tracing::warn!(
        "boutique : {TRIP_AFTER} refus d'affilée, pause de {} s — les fiches déjà lues restent servies",
        rest.as_secs()
    );
}

/// La boutique se repose-t-elle en ce moment ?
pub(crate) fn resting() -> bool {
    let gate = gate();
    match *gate.rest_until.lock().unwrap_or_else(|err| err.into_inner()) {
        Some(until) => Instant::now() < until,
        None => false,
    }
}

/// `Retry-After` d'une réponse, en secondes (la forme date est ignorée : Steam
/// n'envoie que des secondes).
pub(crate) fn retry_after(response: &reqwest::Response) -> Option<Duration> {
    let value = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()?;
    Some(Duration::from_secs(value))
}

fn rest_path() -> PathBuf {
    lapinux_core::paths::LapinuxPaths::discover()
        .cache_dir
        .join("steam-store")
        .join("repos")
}

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

fn write_rest(rest: Duration) {
    let path = rest_path();
    let Some(parent) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let _ = std::fs::write(path, (now_epoch() + rest.as_secs()).to_string());
}

fn read_rest() -> Option<Instant> {
    let text = std::fs::read_to_string(rest_path()).ok()?;
    let seconds = remaining(&text, now_epoch())?;
    Some(Instant::now() + Duration::from_secs(seconds))
}

fn clear_rest() {
    let _ = std::fs::remove_file(rest_path());
}

/// Secondes restantes d'une coupure écrite sur disque. `None` quand elle est
/// passée — le fichier n'est alors qu'un reste à ignorer.
fn remaining(text: &str, now: u64) -> Option<u64> {
    let until: u64 = text.trim().parse().ok()?;
    (until > now).then(|| until - now)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn une_coupure_ecrite_expire_d_elle_meme() {
        assert_eq!(remaining("1000", 900), Some(100));
        assert_eq!(remaining("1000", 1000), None);
        assert_eq!(remaining("1000", 1200), None);
        assert_eq!(remaining("illisible", 0), None);
    }

    #[test]
    fn le_repos_annonce_ne_descend_pas_sous_le_minimum() {
        // Un `Retry-After: 1` d'Akamai ne rouvre pas la boutique une seconde
        // plus tard : le minimum de deux minutes s'applique.
        assert!(MIN_REST >= Duration::from_secs(120));
        assert!(TRIP_AFTER >= 2);
    }
}
