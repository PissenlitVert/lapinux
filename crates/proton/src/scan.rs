//! Découverte des Proton installés (Steam, compatibilitytools.d, Ludus, umu).

use ludus_core::model::ProtonInfo;
use std::path::{Path, PathBuf};

/// Répertoires à examiner, dans l'ordre de priorité d'affichage.
pub fn candidate_dirs(steam_roots: &[PathBuf], extra: &[PathBuf]) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for root in steam_roots {
        dirs.push(root.join("steamapps/common"));
        dirs.push(root.join("compatibilitytools.d"));
    }
    dirs.push(PathBuf::from("/usr/share/steam/compatibilitytools.d"));
    dirs.extend(extra.iter().cloned());
    dirs
}

pub fn scan(steam_roots: &[PathBuf], extra: &[PathBuf]) -> Vec<ProtonInfo> {
    let mut found: Vec<ProtonInfo> = Vec::new();
    for dir in candidate_dirs(steam_roots, extra) {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if !looks_like_proton(&name, &path) {
                continue;
            }
            let info = describe(&path, &name, source_label(&dir));
            if found.iter().any(|p| p.path == info.path) {
                continue;
            }
            found.push(info);
        }
    }
    found.sort_by(|a, b| {
        version_key(&b.name)
            .cmp(&version_key(&a.name))
            .then(a.name.cmp(&b.name))
    });
    found
}

fn source_label(dir: &Path) -> String {
    if dir.ends_with("compatibilitytools.d") {
        "compatibilitytools".into()
    } else if dir.ends_with("umu/compatibilitytools") {
        "umu".into()
    } else if dir.to_string_lossy().contains("/ludus/") {
        "ludus".into()
    } else if dir.ends_with("common") {
        "steam".into()
    } else {
        "autre".into()
    }
}

fn looks_like_proton(name: &str, path: &Path) -> bool {
    let lower = name.to_ascii_lowercase();
    if !(lower.contains("proton") || lower.contains("ge-")) {
        return false;
    }
    path.join("proton").is_file()
        || path.join("files/bin/wine").is_file()
        || path.join("files/bin/wine64").is_file()
}

fn describe(path: &Path, name: &str, source: String) -> ProtonInfo {
    let version = std::fs::read_to_string(path.join("version"))
        .or_else(|_| std::fs::read_to_string(path.join("version.txt")))
        .map(|v| v.trim().to_string())
        .unwrap_or_default();
    ProtonInfo {
        name: name.to_string(),
        path: path.to_path_buf(),
        source,
        version,
        runnable: path.join("proton").is_file() || path.join("files/bin/wine").is_file(),
    }
}

/// Clé de tri numérique : "GE-Proton11-7" → [11, 7].
pub fn version_key(name: &str) -> Vec<u32> {
    let mut numbers = Vec::new();
    let mut current = String::new();
    for ch in name.chars() {
        if ch.is_ascii_digit() {
            current.push(ch);
        } else if !current.is_empty() {
            if let Ok(value) = current.parse::<u32>() {
                numbers.push(value);
            }
            current.clear();
        }
    }
    if !current.is_empty() {
        if let Ok(value) = current.parse::<u32>() {
            numbers.push(value);
        }
    }
    numbers
}

/// Meilleur Proton disponible : GE le plus récent, sinon CachyOS, sinon le plus récent.
pub fn recommended(protons: &[ProtonInfo]) -> Option<&ProtonInfo> {
    let runnable: Vec<&ProtonInfo> = protons.iter().filter(|p| p.runnable).collect();
    if runnable.is_empty() {
        return None;
    }
    let rank = |p: &ProtonInfo| -> u8 {
        let name = p.name.to_ascii_lowercase();
        if name.contains("ge-proton") || name.contains("dw-proton") {
            0
        } else if name.contains("cachyos") {
            1
        } else if name.contains("experimental") {
            3
        } else {
            2
        }
    };
    let mut sorted = runnable;
    sorted.sort_by(|a, b| {
        rank(a)
            .cmp(&rank(b))
            .then(version_key(&b.name).cmp(&version_key(&a.name)))
            .then(a.name.cmp(&b.name))
    });
    sorted.first().copied()
}

pub fn find<'a>(protons: &'a [ProtonInfo], name: &str) -> Option<&'a ProtonInfo> {
    protons.iter().find(|p| p.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch_file(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"x").unwrap();
    }

    #[test]
    fn scans_compatibility_tools() {
        let dir = tempfile::tempdir().unwrap();
        let tool = dir.path().join("compatibilitytools.d/GE-Proton11-7-x86_64");
        touch_file(&tool.join("proton"));
        std::fs::write(tool.join("version"), "GE-Proton11-7\n").unwrap();
        std::fs::create_dir_all(dir.path().join("compatibilitytools.d/Autre/")).unwrap();

        let protons = scan(&[], &[dir.path().join("compatibilitytools.d")]);
        assert_eq!(protons.len(), 1);
        assert_eq!(protons[0].name, "GE-Proton11-7-x86_64");
        assert_eq!(protons[0].version, "GE-Proton11-7");
        assert!(protons[0].runnable);
    }

    #[test]
    fn version_keys_sort_numerically() {
        assert_eq!(version_key("GE-Proton11-7"), vec![11, 7]);
        assert!(version_key("GE-Proton11-10") > version_key("GE-Proton11-7"));
        assert_eq!(version_key("Proton - Experimental"), Vec::<u32>::new());
    }

    #[test]
    fn recommendation_prefers_ge() {
        let mk = |name: &str| ProtonInfo {
            name: name.into(),
            path: PathBuf::from("/x"),
            source: "test".into(),
            version: String::new(),
            runnable: true,
        };
        let list = vec![
            mk("Proton 11.0"),
            mk("Proton - Experimental"),
            mk("GE-Proton10-4"),
            mk("GE-Proton11-2"),
        ];
        assert_eq!(recommended(&list).unwrap().name, "GE-Proton11-2");
        assert_eq!(find(&list, "Proton 11.0").unwrap().name, "Proton 11.0");
        assert!(recommended(&[]).is_none());
    }
}
