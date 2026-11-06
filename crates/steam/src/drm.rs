//! Détection du DRM Steam (SteamStub/CEG) par analyse des en-têtes PE.
//!
//! Aucun contournement n'est implémenté : on se contente de prévenir
//! l'utilisateur qu'un client Steam est nécessaire pour démarrer le jeu.

use lapinux_core::model::Drm;
use std::path::Path;

const STUB_SECTIONS: [&str; 4] = [".bind", "steamstub", ".steam", ".ceg"];

/// Analyse un exécutable Windows : section SteamStub ou non.
pub fn analyze_pe(bytes: &[u8]) -> Option<bool> {
    if bytes.len() < 0x40 || &bytes[0..2] != b"MZ" {
        return None;
    }
    let e_lfanew = u32::from_le_bytes(bytes.get(0x3c..0x40)?.try_into().ok()?) as usize;
    let pe = bytes.get(e_lfanew..e_lfanew + 24)?;
    if &pe[0..4] != b"PE\0\0" {
        return None;
    }
    let number_of_sections = u16::from_le_bytes(pe.get(6..8)?.try_into().ok()?) as usize;
    let optional_header_size = u16::from_le_bytes(pe.get(20..22)?.try_into().ok()?) as usize;
    let section_table = e_lfanew + 24 + optional_header_size;
    for index in 0..number_of_sections.min(96) {
        let offset = section_table + index * 40;
        let name_bytes = bytes.get(offset..offset + 8)?;
        let name = String::from_utf8_lossy(name_bytes)
            .trim_end_matches('\0')
            .to_ascii_lowercase();
        if STUB_SECTIONS.iter().any(|stub| name.contains(stub)) {
            return Some(true);
        }
    }
    Some(false)
}

/// Détecte le type de DRM d'une installation.
pub fn detect(install_dir: &Path, exe: Option<&Path>) -> Drm {
    let steam_api = find_steam_api(install_dir);
    // Seul l'en-tête PE nous intéresse : on évite de charger un exécutable
    // complet (certains pèsent plusieurs centaines de Mio).
    let stub = exe
        .and_then(|path| read_prefix(path, 256 * 1024))
        .and_then(|bytes| analyze_pe(&bytes))
        .unwrap_or(false);
    match (stub, steam_api) {
        (true, _) => Drm::SteamStub,
        (false, true) => Drm::Steamworks,
        (false, false) => Drm::None,
    }
}

fn read_prefix(path: &Path, max: usize) -> Option<Vec<u8>> {
    use std::io::Read;
    let file = std::fs::File::open(path).ok()?;
    let mut buffer = Vec::with_capacity(max.min(4096));
    file.take(max as u64).read_to_end(&mut buffer).ok()?;
    Some(buffer)
}

/// Bibliothèques Steamworks : les DLL Windows, et celles que portent les
/// builds Linux natifs (`libsteam_api.so`) et macOS.
fn is_steam_api_library(name: &str) -> bool {
    matches!(
        name,
        "steam_api.dll"
            | "steam_api64.dll"
            | "libsteam_api.so"
            | "steam_api.so"
            | "libsteam_api.dylib"
            | "steam_api.dylib"
    )
}

fn find_steam_api(dir: &Path) -> bool {
    let mut stack = vec![(dir.to_path_buf(), 0usize)];
    while let Some((path, depth)) = stack.pop() {
        if depth > 3 {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.flatten() {
            let entry_path = entry.path();
            if entry_path.is_dir() {
                stack.push((entry_path, depth + 1));
            } else if let Some(name) = entry_path.file_name().and_then(|n| n.to_str()) {
                if is_steam_api_library(&name.to_ascii_lowercase()) {
                    return true;
                }
            }
        }
    }
    false
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// Construit un PE minimal avec les sections fournies.
    pub fn fake_pe(sections: &[&str], bits: u16) -> Vec<u8> {
        let e_lfanew = 0x80usize;
        let optional_size = 0xF0usize;
        let section_count = sections.len();
        let total = e_lfanew + 24 + optional_size + section_count * 40;
        let mut bytes = vec![0u8; total];
        bytes[0..2].copy_from_slice(b"MZ");
        bytes[0x3c..0x40].copy_from_slice(&(e_lfanew as u32).to_le_bytes());
        bytes[e_lfanew..e_lfanew + 4].copy_from_slice(b"PE\0\0");
        // Machine + nombre de sections + taille de l'en-tête optionnel.
        let machine: u16 = if bits == 64 { 0x8664 } else { 0x014c };
        bytes[e_lfanew + 4..e_lfanew + 6].copy_from_slice(&machine.to_le_bytes());
        bytes[e_lfanew + 6..e_lfanew + 8].copy_from_slice(&(section_count as u16).to_le_bytes());
        bytes[e_lfanew + 20..e_lfanew + 22].copy_from_slice(&(optional_size as u16).to_le_bytes());
        for (i, name) in sections.iter().enumerate() {
            let offset = e_lfanew + 24 + optional_size + i * 40;
            let raw = name.as_bytes();
            let len = raw.len().min(8);
            bytes[offset..offset + len].copy_from_slice(&raw[..len]);
        }
        bytes
    }

    #[test]
    fn detects_steam_stub_section() {
        assert_eq!(analyze_pe(&fake_pe(&[".text", ".bind"], 64)), Some(true));
        assert_eq!(analyze_pe(&fake_pe(&[".text", ".steam"], 32)), Some(true));
    }

    #[test]
    fn clean_binary_is_not_stub() {
        assert_eq!(analyze_pe(&fake_pe(&[".text", ".rdata"], 64)), Some(false));
    }

    #[test]
    fn rejects_non_pe() {
        assert_eq!(analyze_pe(b"not a pe"), None);
        assert_eq!(analyze_pe(&[]), None);
    }

    #[test]
    fn a_native_linux_build_names_steamworks() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("bin/x86_64")).unwrap();
        std::fs::write(dir.path().join("bin/x86_64/libsteam_api.so"), b"\x7fELF").unwrap();
        assert_eq!(detect(dir.path(), None), Drm::Steamworks);
    }

    #[test]
    fn detect_uses_steam_api_presence() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("steam_api64.dll"), b"x").unwrap();
        assert_eq!(detect(dir.path(), None), Drm::Steamworks);

        let dir2 = tempfile::tempdir().unwrap();
        let exe = dir2.path().join("game.exe");
        std::fs::write(&exe, fake_pe(&[".text", ".bind"], 64)).unwrap();
        assert_eq!(detect(dir2.path(), Some(&exe)), Drm::SteamStub);

        let dir3 = tempfile::tempdir().unwrap();
        assert_eq!(detect(dir3.path(), None), Drm::None);
    }
}
