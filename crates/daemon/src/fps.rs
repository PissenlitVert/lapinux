//! Cadence réelle des images pendant une partie.
//!
//! GameScope compose le jeu lui-même : c'est lui qui sait à quelle cadence
//! les images sont réellement présentées, y compris pour les jeux Vulkan qui
//! ne passent jamais par X11. Son option `-T` écrit une ligne `fps=…` dans un
//! tube à chaque lot d'images ; il suffit de le lire.
//!
//! Rien n'est mesuré hors GameScope : une cadence devinée serait pire qu'une
//! ligne absente.

use lapinux_core::model::FpsStats;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::time::{Duration, Instant};

/// Début de partie : compilation des shaders, écran de chargement, vidéo…
/// ces premières secondes ne sont pas du jeu, elles ne comptent pas dans le
/// minimum affiché.
const WARMUP: Duration = Duration::from_secs(8);
/// Une mesure presque nulle (menu statique, jeu en pause) ne doit pas devenir
/// le « minimum » de la partie.
const IDLE_FLOOR: u32 = 5;

/// Nombre de points envoyés pour le graphique : au-delà, la courbe est
/// rééchantillonnée à pas de temps constant.
const SERIES: usize = 120;

/// Mesures brutes gardées, horodatées : une partie de plusieurs heures tient
/// dans cette réserve, le moindre détail n'étant conservé que sur les
/// dernières minutes. Les horodatages survivent à la réduction, donc l'heure
/// sous le curseur reste juste.
const RAW: usize = 1440;

/// Lit le tube de statistiques de GameScope et publie les bornes et la courbe
/// au fil de la partie. Rend la main quand la session se termine.
///
/// Le tube est ouvert sans bloquer : GameScope n'ouvre son bout que quelques
/// secondes après le lancement, et son absence n'est pas une erreur.
pub fn watch_stats(
    path: &Path,
    alive: &dyn Fn() -> bool,
    report: &dyn Fn(FpsStats, &[u32], i64, i64),
    fps_cap: u32,
) {
    let mut file: Option<std::fs::File> = None;
    let mut pending = String::new();
    let mut first_sample: Option<Instant> = None;
    let mut samples = 0u32;
    let mut sum = 0u64;
    let mut min = u32::MAX;
    let mut max = 0u32;
    // (instant, cadence) de chaque mesure retenue.
    let mut raw: Vec<(i64, u32)> = Vec::new();
    let mut series: Vec<u32> = Vec::new();
    let mut series_start = 0i64;
    let mut series_end = 0i64;
    loop {
        if !alive() {
            break;
        }
        if file.is_none() {
            match std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(path)
            {
                Ok(opened) => file = Some(opened),
                Err(_) => {
                    std::thread::sleep(Duration::from_millis(300));
                    continue;
                }
            }
        }
        let mut chunk = [0u8; 4096];
        let read = file
            .as_mut()
            .map(|handle| handle.read(&mut chunk))
            .unwrap_or(Ok(0));
        match read {
            Ok(0) => std::thread::sleep(Duration::from_millis(200)),
            Ok(count) => {
                pending.push_str(&String::from_utf8_lossy(&chunk[..count]));
                while let Some(index) = pending.find('\n') {
                    let line = pending[..index].trim().to_string();
                    pending.drain(..=index);
                    let Some(value) = line.strip_prefix("fps=") else {
                        continue;
                    };
                    let Ok(rate) = value.trim().parse::<f32>() else {
                        continue;
                    };
                    let current = measured_rate(rate, fps_cap);
                    let settled_from = *first_sample.get_or_insert_with(Instant::now);
                    let settled = settled_from.elapsed() >= WARMUP && current >= IDLE_FLOOR;
                    if settled {
                        samples += 1;
                        sum += u64::from(current);
                        min = min.min(current);
                        max = max.max(current);
                        raw.push((lapinux_core::util::now_unix(), current));
                        if raw.len() > RAW {
                            raw = raw.iter().step_by(2).copied().collect();
                        }
                        let resampled = resample(&raw, SERIES);
                        series = resampled.0;
                        series_start = resampled.1;
                        series_end = resampled.2;
                    }
                    report(
                        FpsStats {
                            current,
                            min: if min == u32::MAX { 0 } else { min },
                            avg: if samples == 0 {
                                0
                            } else {
                                (sum / u64::from(samples)) as u32
                            },
                            max,
                        },
                        &series,
                        series_start,
                        series_end,
                    );
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(_) => {
                file = None;
                std::thread::sleep(Duration::from_millis(300));
            }
        }
    }
    // Le tube appartient à cette partie : rien ne doit rester derrière elle.
    let _ = std::fs::remove_file(path);
}

/// Convertit une mesure GameScope en cadence affichable. Le tube compte les
/// images du compositeur : déplacer la souris en ajoute, et la valeur peut
/// dépasser la limite du jeu. Cette limite est le plafond réel de la session.
fn measured_rate(rate: f32, fps_cap: u32) -> u32 {
    let measured = rate.round().clamp(0.0, 1000.0) as u32;
    if fps_cap > 0 {
        measured.min(fps_cap)
    } else {
        measured
    }
}

/// Réduit les mesures horodatées à `limit` points **à pas de temps constant**.
///
/// Le graphique place ses points à intervalles réguliers le long de son axe :
/// si la courbe gardait des mesures inégalement espacées (une pause, une
/// reprise), le curseur désignerait le mauvais instant. Chaque point est donc
/// la moyenne des mesures tombant dans sa tranche de temps, et les bornes
/// rendues avec lui donnent l'heure exacte de chaque extrémité.
fn resample(raw: &[(i64, u32)], limit: usize) -> (Vec<u32>, i64, i64) {
    let Some(&(first, _)) = raw.first() else {
        return (Vec::new(), 0, 0);
    };
    let last = raw.last().map(|point| point.0).unwrap_or(first);
    if raw.len() <= limit || last <= first {
        return (raw.iter().map(|point| point.1).collect(), first, last);
    }
    let count = limit.max(2);
    let span = last - first;
    let mut out = Vec::with_capacity(count);
    let mut index = 0usize;
    let mut previous = raw[0].1;
    for bucket in 0..count {
        // Borne de la tranche : la dernière englobe ce qui reste.
        let edge = if bucket + 1 == count {
            last + 1
        } else {
            first + span * (bucket as i64 + 1) / (count as i64 - 1)
        };
        let mut total = 0u64;
        let mut taken = 0u64;
        while index < raw.len() && raw[index].0 < edge {
            total += u64::from(raw[index].1);
            taken += 1;
            index += 1;
        }
        // Une tranche sans mesure (jeu figé) prolonge la précédente : la
        // courbe ne plonge pas à zéro sans preuve.
        previous = if taken > 0 {
            (total / taken) as u32
        } else {
            previous
        };
        out.push(previous);
    }
    (out, first, last)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_capped_session_never_reports_above_the_limit() {
        // GameScope compte aussi les images du curseur : 300 i/s ne doit pas
        // apparaître quand la session est plafonnée à 165.
        assert_eq!(measured_rate(300.0, 165), 165);
        assert_eq!(measured_rate(120.0, 165), 120);
        assert_eq!(measured_rate(300.0, 0), 300);
    }

    /// Le rééchantillonnage garde une mesure par tranche et des bornes qui
    /// correspondent aux extrémités réelles.
    #[test]
    fn resampling_keeps_uniform_time_steps() {
        let raw: Vec<(i64, u32)> = (0..1000).map(|index| (1_700_000_000 + index, 60)).collect();
        let (series, first, last) = resample(&raw, 120);
        assert_eq!(series.len(), 120);
        assert_eq!(first, 1_700_000_000);
        assert_eq!(last, 1_700_000_999);
        assert!(series.iter().all(|value| *value == 60));
    }

    /// Une pause sans mesure ne fait pas tomber la courbe à zéro.
    #[test]
    fn a_gap_holds_the_previous_rate() {
        let mut raw: Vec<(i64, u32)> = (0..100).map(|index| (1_700_000_000 + index, 120)).collect();
        raw.extend((0..100).map(|index| (1_700_000_900 + index, 30)));
        let (series, _, last) = resample(&raw, 10);
        assert_eq!(series.len(), 10);
        assert!(series[0] >= 30 && series[0] <= 120);
        assert_eq!(*series.last().unwrap(), 30);
        assert_eq!(last, 1_700_000_999);
    }

    /// Une partie courte n'est pas étirée : la courbe garde ses mesures.
    #[test]
    fn a_short_series_is_kept_as_is() {
        let raw = [(1_700_000_000, 144), (1_700_000_001, 141)];
        let (series, first, last) = resample(&raw, 120);
        assert_eq!(series, vec![144, 141]);
        assert_eq!((first, last), (1_700_000_000, 1_700_000_001));
    }
}
