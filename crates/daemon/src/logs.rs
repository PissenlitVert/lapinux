//! Journaux de l'application exposés à l'écran Réglages → Logs.
//!
//! Trois journaux stables sont écrits : `daemon.log`, `ui.log` (interface et
//! WebKit) et `overlay.log` (menu rapide, HUD et bandeaux). Chaque partie a en
//! plus son `game-<appid>-<horodatage>.log`, que la page Activité ouvre et que
//! la suppression de la partie emporte. Tous tournent au-delà de `LOG_CAP`,
//! ce qui garde le dossier borné et lisible. Le nom demandé par l'interface est
//! toujours validé contre cette liste, jamais utilisé comme chemin : un client
//! IPC ne peut pas lire hors du dossier.
//!
//! Les fichiers de session peuvent grossir sans limite ; seule la fin est lue,
//! sur les derniers 256 Kio, pour garder l'interface réactive.

use lapinux_core::model::{LogFile, LogsView};
use lapinux_core::paths::LapinuxPaths;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::UNIX_EPOCH;

/// Bytes relus en fin de fichier.
const TAIL_BYTES: u64 = 256 * 1024;
/// Bornes de sécurité pour le nombre de lignes rendues.
pub const MIN_LINES: usize = 1;
pub const MAX_LINES: usize = 4000;
pub const DEFAULT_LINES: usize = 400;

/// Fichiers `.log` du dossier, `daemon.log` d'abord puis les plus récents.
pub fn list(paths: &LapinuxPaths) -> Vec<LogFile> {
    let Ok(entries) = std::fs::read_dir(&paths.log_dir) else {
        return Vec::new();
    };
    let mut files: Vec<LogFile> = entries
        .flatten()
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            if !meta.is_file() {
                return None;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.ends_with(".log") {
                return None;
            }
            Some(LogFile {
                name,
                size: meta.len(),
                modified: meta
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                    .map(|age| age.as_secs() as i64)
                    .unwrap_or(0),
            })
        })
        .collect();
    // Les quatre journaux de l'application d'abord, dans l'ordre où on les
    // consulte ; les archives et le reste suivent par date.
    let rank = |name: &str| match name {
        "daemon.log" => 0,
        "ui.log" => 1,
        "games.log" => 2,
        "overlay.log" => 3,
        _ => 4,
    };
    files.sort_by(|a, b| {
        rank(&a.name)
            .cmp(&rank(&b.name))
            .then(b.modified.cmp(&a.modified))
            .then(a.name.cmp(&b.name))
    });
    files
}

/// Liste les journaux et rend les dernières lignes du fichier demandé. Un nom
/// inconnu (fichier supprimé, chemin déguisé) retombe sur `daemon.log`.
pub fn view(paths: &LapinuxPaths, file: Option<&str>, lines: usize) -> LogsView {
    let files = list(paths);
    let chosen = file
        .filter(|wanted| files.iter().any(|entry| entry.name == *wanted))
        .map(str::to_string)
        .or_else(|| {
            files
                .iter()
                .find(|entry| entry.name == "daemon.log")
                .map(|entry| entry.name.clone())
        })
        .or_else(|| files.first().map(|entry| entry.name.clone()))
        .unwrap_or_default();

    let mut view = LogsView {
        file: chosen.clone(),
        files,
        lines: Vec::new(),
    };
    if chosen.is_empty() {
        return view;
    }
    let limit = lines.clamp(MIN_LINES, MAX_LINES);
    match tail(&paths.log_dir.join(&chosen), limit) {
        Ok(read) => view.lines = read,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            view.lines = vec!["journal supprimé".into()];
        }
        Err(error) => view.lines = vec![format!("lecture impossible : {error}")],
    }
    view
}

/// Ouvre un journal partagé en ajout, après rotation, et y date l'entrée :
/// menu rapide, HUD et bandeaux tiennent dans un seul `overlay.log` au lieu
/// d'un fichier par jeu et par processus.
pub fn open_shared(
    paths: &LapinuxPaths,
    name: &str,
    header: &str,
) -> std::io::Result<std::fs::File> {
    let path = paths.log_dir.join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    lapinux_core::util::rotate_log(&path);
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    use std::io::Write;
    let _ = writeln!(
        file,
        "\n=== {header} · {} ===",
        lapinux_core::util::format_datetime(lapinux_core::util::now_unix())
    );
    Ok(file)
}

/// Efface les journaux et tubes que les versions précédentes créaient par
/// partie ou par overlay. Ils s'accumulaient sans nettoyage — un fichier par
/// lancement — et noyaient la liste des journaux.
pub fn prune_legacy(paths: &LapinuxPaths) {
    let Ok(entries) = std::fs::read_dir(&paths.log_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !legacy_name(&name) {
            continue;
        }
        if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(true) {
            continue;
        }
        let _ = std::fs::remove_file(entry.path());
    }
}

/// Nom d'un fichier de l'ancien schéma : `<appid>-<horodatage>.log`,
/// `overlay*` par jeu, ou tube laissé par une session tuée.
fn legacy_name(name: &str) -> bool {
    if let Some(stem) = name.strip_suffix(".log") {
        let digits = |text: &str| !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
        let mut parts = stem.split('-');
        if let (Some(appid), Some(stamp), None) = (parts.next(), parts.next(), parts.next()) {
            if digits(appid) && digits(stamp) {
                return true;
            }
        }
        for prefix in ["overlay-hud-", "overlay-record-", "overlay-"] {
            if let Some(rest) = stem.strip_prefix(prefix) {
                if digits(rest) {
                    return true;
                }
            }
        }
        return matches!(stem, "overlay-toast" | "webview");
    }
    (name.starts_with("fps-") && name.ends_with(".fifo"))
        || (name.starts_with("nav-") && name.ends_with(".fifo"))
        || (name.starts_with("scene-") && name.ends_with(".exit"))
}

/// Dernières `max_lines` lignes, sans charger tout un gros journal.
fn tail(path: &Path, max_lines: usize) -> std::io::Result<Vec<String>> {
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    let start = len.saturating_sub(TAIL_BYTES);
    if start > 0 {
        file.seek(SeekFrom::Start(start))?;
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<&str> = text.lines().collect();
    // En coupant au milieu d'un fichier, la première ligne est incomplète.
    if start > 0 && !lines.is_empty() {
        lines.remove(0);
    }
    let skip = lines.len().saturating_sub(max_lines);
    Ok(lines[skip..]
        .iter()
        .map(|line| strip_ansi(line.trim_end_matches('\r')))
        .collect())
}

/// Retire les séquences ANSI : le journal est affiché dans l'interface, pas
/// dans un terminal, et les couleurs de `tracing` n'y ont plus de sens.
fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            out.push(ch);
            continue;
        }
        match chars.peek().copied() {
            Some('[') => {
                chars.next();
                for code in chars.by_ref() {
                    if code.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            Some(']') => {
                chars.next();
                while let Some(code) = chars.next() {
                    if code == '\u{7}' {
                        break;
                    }
                    if code == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths_with_logs(files: &[(&str, &str)]) -> (tempfile::TempDir, LapinuxPaths) {
        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        std::fs::create_dir_all(&paths.log_dir).unwrap();
        for (name, content) in files {
            std::fs::write(paths.log_dir.join(name), content).unwrap();
        }
        (dir, paths)
    }

    #[test]
    fn daemon_log_comes_first_then_the_most_recent() {
        let (_dir, paths) = paths_with_logs(&[
            ("2830030-1.log", "session"),
            ("overlay-1.log", "overlay"),
            ("daemon.log", "daemon"),
            ("notes.txt", "pas un journal"),
        ]);
        let files = list(&paths);
        let names: Vec<&str> = files.iter().map(|file| file.name.as_str()).collect();
        assert_eq!(names[0], "daemon.log");
        assert!(names.contains(&"2830030-1.log"));
        assert!(!names.contains(&"notes.txt"));
    }

    #[test]
    fn only_the_last_lines_are_returned() {
        let content: String = (1..=50).map(|n| format!("ligne {n}\n")).collect();
        let (_dir, paths) = paths_with_logs(&[("daemon.log", &content)]);
        let view = view(&paths, Some("daemon.log"), 3);
        assert_eq!(view.file, "daemon.log");
        assert_eq!(view.lines, vec!["ligne 48", "ligne 49", "ligne 50"]);
    }

    #[test]
    fn unknown_names_and_paths_fall_back_on_the_daemon_log() {
        let (_dir, paths) =
            paths_with_logs(&[("daemon.log", "daemon"), ("2830030-1.log", "session")]);
        let view = view(&paths, Some("../../etc/passwd"), 10);
        assert_eq!(view.file, "daemon.log");
        assert_eq!(view.lines, vec!["daemon"]);
    }

    #[test]
    fn ansi_colours_are_removed() {
        assert_eq!(
            strip_ansi("\u{1b}[2m2026-10-23\u{1b}[0m \u{1b}[32mINFO\u{1b}[0m daemon"),
            "2026-10-23 INFO daemon"
        );
        assert_eq!(strip_ansi("sans couleur"), "sans couleur");
    }

    #[test]
    fn legacy_logs_are_recognised() {
        assert!(legacy_name("2830030-1793745565.log"));
        assert!(legacy_name("overlay-2830030.log"));
        assert!(legacy_name("overlay-hud-2830030.log"));
        assert!(legacy_name("overlay-record-2830030.log"));
        assert!(legacy_name("overlay-toast.log"));
        assert!(legacy_name("webview.log"));
        assert!(legacy_name("fps-2830030-1793745565.fifo"));
        assert!(legacy_name("nav-2830030.fifo"));
        assert!(!legacy_name("daemon.log"));
        assert!(!legacy_name("game-2830030-1793745565.log"));
        assert!(!legacy_name("ui.log"));
        assert!(!legacy_name("games.log"));
        assert!(!legacy_name("overlay.log"));
        assert!(!legacy_name("downloads.log"));
        assert!(!legacy_name("daemon.log.1"));
    }

    #[test]
    fn invalid_utf8_is_shown_lossily() {
        let (_dir, paths) = paths_with_logs(&[("daemon.log", "")]);
        std::fs::write(paths.log_dir.join("daemon.log"), b"ok\n\xff\xfe fin\n").unwrap();
        let view = view(&paths, Some("daemon.log"), 10);
        assert_eq!(view.lines, vec!["ok", "�\u{fffd} fin"]);
    }

    #[test]
    fn missing_directory_yields_an_empty_view() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        let view = view(&paths, None, 30);
        assert!(view.file.is_empty());
        assert!(view.files.is_empty());
        assert!(view.lines.is_empty());
    }
}
