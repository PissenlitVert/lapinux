//! Recherche d'une police système pour le rendu de l'overlay.

use crate::{Error, Result};
use std::path::{Path, PathBuf};

const CANDIDATES: [&str; 8] = [
    "/usr/share/fonts/google-noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/google-noto-sans-fonts/NotoSans-Regular.ttf",
    "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
    "/usr/share/fonts/liberation-sans/LiberationSans-Regular.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
];

/// Trouve un TTF sans-serif lisible.
pub fn find_font() -> Result<PathBuf> {
    for candidate in CANDIDATES {
        let path = Path::new(candidate);
        if path.is_file() {
            return Ok(path.to_path_buf());
        }
    }
    // Balayage limité de /usr/share/fonts.
    if let Some(found) = scan(Path::new("/usr/share/fonts"), 0) {
        return Ok(found);
    }
    Err(Error::Other(
        "aucune police TTF trouvée dans /usr/share/fonts".into(),
    ))
}

fn scan(dir: &Path, depth: usize) -> Option<PathBuf> {
    if depth > 3 {
        return None;
    }
    let entries = std::fs::read_dir(dir).ok()?;
    let mut dirs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            dirs.push(path);
            continue;
        }
        let name = path.file_name()?.to_string_lossy().to_ascii_lowercase();
        if name.ends_with(".ttf")
            && (name.contains("sans") || name.contains("dejavu") || name.contains("noto"))
            && !name.contains("bold")
            && !name.contains("italic")
        {
            return Some(path);
        }
    }
    for dir in dirs {
        if let Some(found) = scan(&dir, depth + 1) {
            return Some(found);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_a_system_font() {
        // La machine de développement possède Noto Sans ; sur un système nu,
        // on vérifie au moins que l'erreur est explicite.
        match find_font() {
            Ok(path) => assert!(path.is_file()),
            Err(err) => assert!(err.to_string().contains("police")),
        }
    }
}
