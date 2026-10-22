//! Petits utilitaires partagés.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["o", "Kio", "Mio", "Gio", "Tio"];
    if bytes < 1024 {
        return format!("{bytes} o");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

pub fn format_duration(seconds: u64) -> String {
    let h = seconds / 3600;
    let m = (seconds % 3600) / 60;
    if h > 0 {
        format!("{h} h {m:02}")
    } else if m > 0 {
        format!("{m} min")
    } else {
        format!("{seconds} s")
    }
}

pub fn ellipsize(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Slug utilisé pour les noms de dossier (protons, profils).
pub fn slugify(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_dash = false;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "sans-nom".into()
    } else {
        trimmed
    }
}

pub fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_units() {
        assert_eq!(format_bytes(512), "512 o");
        assert_eq!(format_bytes(2048), "2.0 Kio");
        assert_eq!(format_bytes(50 * 1024 * 1024), "50.0 Mio");
        assert_eq!(format_bytes(700 * 1024 * 1024 * 1024), "700 Gio");
    }

    #[test]
    fn durations() {
        assert_eq!(format_duration(42), "42 s");
        assert_eq!(format_duration(125), "2 min");
        assert_eq!(format_duration(7300), "2 h 01");
    }

    #[test]
    fn slugs() {
        assert_eq!(slugify("GE-Proton 11.7"), "ge-proton-11-7");
        assert_eq!(slugify("!!!"), "sans-nom");
    }

    #[test]
    fn ellipsis() {
        assert_eq!(ellipsize("abcdef", 4), "abc…");
        assert_eq!(ellipsize("abc", 10), "abc");
    }
}
