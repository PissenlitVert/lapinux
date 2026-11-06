//! Audit des accès `/dev/input` : ce que Lapinux peut réellement lire.
//!
//! evdev n'énumère que les périphériques lisibles ; ce module inspecte aussi
//! les nœuds refusés pour que le diagnostic nomme le clavier bloqué.

use crate::codes;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Access {
    pub path: PathBuf,
    pub name: String,
    pub readable: bool,
    pub error: Option<String>,
    pub keyboard: bool,
    pub gamepad: bool,
}

/// Tous les event nodes, lisibles ou non, avec leur type réel via sysfs.
pub fn audit() -> Vec<Access> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir("/dev/input")
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .map(|name| name.starts_with("event"))
                        .unwrap_or(false)
                })
                .collect()
        })
        .unwrap_or_default();
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let name = sysfs_name(&path);
            let keyboard = has_code(&path, "key", codes::KEY_A)
                && has_code(&path, "key", codes::KEY_TAB);
            let gamepad = has_code(&path, "key", codes::BTN_SOUTH)
                && has_code(&path, "abs", codes::ABS_X);
            let error = std::fs::File::open(&path)
                .err()
                .map(|err| err.to_string());
            Access {
                path,
                name,
                readable: error.is_none(),
                error,
                keyboard,
                gamepad,
            }
        })
        .collect()
}

fn event_name(path: &Path) -> Option<&str> {
    path.file_name()?.to_str()
}

fn sysfs_name(path: &Path) -> String {
    event_name(path)
        .and_then(|event| {
            std::fs::read_to_string(format!("/sys/class/input/{event}/device/name")).ok()
        })
        .map(|name| name.trim().to_string())
        .unwrap_or_else(|| "périphérique inconnu".into())
}

fn has_code(path: &Path, capability: &str, code: u16) -> bool {
    let Some(event) = event_name(path) else {
        return false;
    };
    let file = format!("/sys/class/input/{event}/device/capabilities/{capability}");
    let Ok(text) = std::fs::read_to_string(file) else {
        return false;
    };
    bitmap_has(&text, code)
}

/// Le noyau imprime les mots du plus fort au plus faible. On remet le mot 0
/// (codes 0-63) en premier avant de tester le bit demandé.
fn bitmap_has(text: &str, code: u16) -> bool {
    let mut words: Vec<u64> = text
        .split_whitespace()
        .filter_map(|word| u64::from_str_radix(word, 16).ok())
        .collect();
    words.reverse();
    let word = code as usize / 64;
    let bit = code as usize % 64;
    words
        .get(word)
        .map(|value| value & (1u64 << bit) != 0)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitmap_words_are_read_low_first() {
        // Deux mots : le second (affiché en premier) porte KEY_A (30).
        assert!(bitmap_has("0 40000000", codes::KEY_A));
        assert!(!bitmap_has("0 40000000", 31));
        assert!(bitmap_has("2 0", 65));
        assert!(!bitmap_has("0 0", 0));
    }

    #[test]
    fn audit_does_not_panic() {
        let access = audit();
        for device in access {
            assert!(device.path.starts_with("/dev/input"));
            if device.readable {
                assert!(device.error.is_none());
            }
        }
    }
}
