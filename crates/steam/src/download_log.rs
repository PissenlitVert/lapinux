//! Journal détaillé des téléchargements de jeux.
//!
//! Un fichier par machine, en ajout, dans `logs/downloads.log` : chaque chunk
//! demandé au CDN, chaque seconde de progression, chaque changement de pause
//! ou d'annulation, avec le trafic réseau mesuré sur la machine. Il sert à
//! répondre à une question précise : quand un téléchargement est arrêté,
//! reste-t-il quelque chose qui consomme la connexion ?

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

struct Inner {
    path: PathBuf,
    file: Mutex<Option<File>>,
    start: Instant,
}

impl Inner {
    fn write(&self, line: &str) {
        let Ok(mut guard) = self.file.lock() else {
            return;
        };
        let Some(file) = guard.as_mut() else {
            return;
        };
        let wall = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let _ = writeln!(
            file,
            "[{wall} +{:>8} ms] {line}",
            self.start.elapsed().as_millis()
        );
        // Chaque ligne part sur le disque : un plantage ne doit pas emporter
        // les preuves du dernier téléchargement.
        let _ = file.flush();
    }
}

/// Journal partageable entre le moteur et le daemon ; un journal sans fichier
/// reste utilisable (tests).
#[derive(Clone)]
pub struct DownloadLog {
    inner: Arc<Inner>,
}

impl DownloadLog {
    /// Ouvre (ou crée) le journal, en ajoutant une ligne de séparation par
    /// lancement. Un chemin impossible à ouvrir désactive seulement le journal.
    pub fn open(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // Un journal en ajout seul grossit sans fin : au-delà de la limite, il
        // part en `.1` et un fichier neuf commence.
        lapinux_core::util::rotate_log(&path);
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok();
        let log = Self {
            inner: Arc::new(Inner {
                path,
                file: Mutex::new(file),
                start: Instant::now(),
            }),
        };
        log.write("──────── nouveau téléchargement ────────");
        log
    }

    /// Journal sans destination, pour les tests et les appels sans contexte.
    pub fn sink() -> Self {
        Self {
            inner: Arc::new(Inner {
                path: PathBuf::new(),
                file: Mutex::new(None),
                start: Instant::now(),
            }),
        }
    }

    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    /// Écrit une ligne ; ne panique jamais et n'interrompt jamais un transfert.
    pub fn write(&self, line: impl AsRef<str>) {
        self.inner.write(line.as_ref());
    }
}

/// Écrit une ligne isolée dans un journal, sans en-tête ni ouverture
/// persistante : utilisé par les requêtes IPC (pause, annulation) qui ne
/// partagent pas l'objet du téléchargement.
///
/// Un échec d'écriture est ignoré : le journal ne doit jamais empêcher
/// d'arrêter un transfert.
pub fn note(path: &Path, line: impl AsRef<str>) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let wall = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let _ = writeln!(file, "[{wall}         ipc] {}", line.as_ref());
    let _ = file.flush();
}

/// Octets reçus par la machine depuis le démarrage, toutes interfaces
/// physiques confondues (loopback et ponts locaux exclus).
///
/// C'est la mesure qui distingue « le moteur télécharge encore » de « autre
/// chose sur la machine utilise la connexion » : le compteur du moteur ne voit
/// que les chunks qu'il a lui-même demandés.
pub fn system_rx_bytes() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/net/dev").ok()?;
    let mut total = 0u64;
    for line in text.lines().skip(2) {
        let Some((iface, rest)) = line.split_once(':') else {
            continue;
        };
        let iface = iface.trim();
        if iface == "lo"
            || iface.starts_with("veth")
            || iface.starts_with("docker")
            || iface.starts_with("virbr")
            || iface.starts_with("br-")
            || iface.starts_with("tap")
        {
            continue;
        }
        if let Some(value) = rest
            .split_whitespace()
            .next()
            .and_then(|value| value.parse::<u64>().ok())
        {
            total = total.saturating_add(value);
        }
    }
    Some(total)
}

/// Débit lisible en Mio/s.
pub fn rate_mib(bytes: u64, elapsed_ms: u64) -> f64 {
    if elapsed_ms == 0 {
        return 0.0;
    }
    bytes as f64 / 1_048_576.0 * 1000.0 / elapsed_ms as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_one_line_per_event() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logs/downloads.log");
        let log = DownloadLog::open(&path);
        log.write("première");
        log.write("deuxième");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("nouveau téléchargement"));
        assert!(text.contains("première"));
        assert!(text.contains("deuxième"));
        assert_eq!(text.lines().count(), 3);
    }

    #[test]
    fn sink_never_panics() {
        let log = DownloadLog::sink();
        log.write("sans fichier");
        assert!(log.path().as_os_str().is_empty());
    }

    #[test]
    fn rate_is_in_mib_per_second() {
        let rate = rate_mib(2 * 1_048_576, 1000);
        assert!((rate - 2.0).abs() < 0.001);
        assert_eq!(rate_mib(10, 0), 0.0);
    }

    #[test]
    fn note_appends_without_header() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("downloads.log");
        note(&path, "annulation demandée");
        note(&path, "pause demandée");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("nouveau téléchargement"));
        assert!(text.contains("annulation demandée"));
        assert_eq!(text.lines().count(), 2);
    }

    #[test]
    fn system_rx_reads_a_positive_counter() {
        let rx = system_rx_bytes();
        if let Some(rx) = rx {
            assert!(rx > 0, "la machine a forcément reçu des octets");
        }
    }
}
