//! Ce qu'un dossier de jeu installé dit de sa protection.
//!
//! Le scan ne lit que des noms de fichiers et l'en-tête PE de l'exécutable :
//! de quoi répondre « rien trouvé » sans charger un jeu de 150 Go. C'est ce
//! que le disque montre, pas une garantie de la boutique : l'absence de
//! marqueur connu vaut « sans DRM », et c'est dit ainsi dans la fiche.

use std::path::Path;

/// Profondeur et nombre de fichiers bornés : un dossier de jeu peut contenir
/// des dizaines de milliers de fichiers, une fiche ne doit pas attendre.
const MAX_DEPTH: usize = 4;
const MAX_FILES: usize = 8_000;
/// Payloads d'installation : leurs DLL ne disent rien du jeu lui-même.
const SKIP_DIRS: [&str; 6] = [
    "redist",
    "redistributables",
    "_commonredist",
    "directx",
    "vcredist",
    "dotnet",
];
/// Exécutables auxiliaires : jamais ceux qui portent la protection.
const SKIP_EXES: [&str; 5] = ["crashhandler", "unins", "setup", "vcredist", "dxsetup"];

/// Fichiers dont le nom seul annonce la protection.
const FILE_MARKERS: [(&str, &str, bool); 9] = [
    ("steam_api", "Steam", false),
    ("steamclient", "Steam", true),
    ("steamstub", "Steam", true),
    ("denuvo", "Denuvo", false),
    ("securom", "SecuROM", false),
    ("arxan", "Arxan", false),
    ("solidshield", "SolidShield", false),
    ("themida", "Themida", false),
    ("winlicense", "WinLicense", false),
];

/// Sections PE qui trahissent un emballage : SteamStub, Denuvo, VMProtect,
/// Themida, Enigma.
const SECTION_MARKERS: [(&str, &str, bool); 7] = [
    (".bind", "Steam", true),
    ("steamstub", "Steam", true),
    (".ceg", "Steam", true),
    (".denuvo", "Denuvo", false),
    (".vmp", "VMProtect", false),
    (".themida", "Themida", false),
    (".enigma", "Enigma", false),
];

/// Protection et client trouvés dans un dossier installé.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Report {
    /// Protection nommée par le dossier (`Steam`, `Denuvo`…).
    pub(crate) drm: Option<&'static str>,
    /// Client dont le build dépend, quand le dossier le nomme.
    pub(crate) client: Option<&'static str>,
    /// Le build exige-t-il le client Steam (SteamStub, CEG, steamclient) ?
    pub(crate) requires_steam: bool,
}

/// Lit un dossier de jeu ; `None` s'il n'est pas sur le disque.
pub(crate) fn scan(dir: &Path) -> Option<Report> {
    if !dir.is_dir() {
        return None;
    }
    let mut report = Report::default();
    let mut exes: Vec<std::path::PathBuf> = Vec::new();
    let mut stack = vec![(dir.to_path_buf(), 0usize)];
    let mut seen = 0usize;
    while let Some((path, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.flatten() {
            let entry_path = entry.path();
            let is_dir = entry
                .file_type()
                .map(|kind| kind.is_dir())
                .unwrap_or(false);
            if is_dir {
                let skipped = entry_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(|name| {
                        let lower = name.to_ascii_lowercase();
                        SKIP_DIRS.iter().any(|skip| lower.contains(skip))
                    })
                    .unwrap_or(false);
                if depth < MAX_DEPTH && !skipped {
                    stack.push((entry_path, depth + 1));
                }
                continue;
            }
            seen += 1;
            if seen > MAX_FILES {
                break;
            }
            let Some(name) = entry_path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let lower = name.to_ascii_lowercase();
            if let Some((_, label, requires_steam)) = FILE_MARKERS
                .iter()
                .find(|(marker, _, _)| lower.contains(marker))
            {
                report.drm = Some(*label);
                report.requires_steam = *requires_steam;
                if *label == "Steam" {
                    report.client = Some("Steam");
                }
                return Some(report);
            }
            if depth <= 2
                && lower.ends_with(".exe")
                && !SKIP_EXES.iter().any(|skip| lower.contains(skip))
                && exes.len() < 8
            {
                exes.push(entry_path);
            }
        }
    }
    // Aucune DLL parlante : l'en-tête des exécutables décide. Un stub Steam
    // ou un emballage Denuvo s'y lit sans lancer le jeu.
    for exe in exes {
        if let Some((label, requires_steam)) = pe_marker(&exe) {
            report.drm = Some(label);
            report.requires_steam = requires_steam;
            if label == "Steam" {
                report.client = Some("Steam");
            }
            break;
        }
    }
    Some(report)
}

/// Nomme la protection lue dans les sections d'un exécutable Windows.
fn pe_marker(path: &Path) -> Option<(&'static str, bool)> {
    use std::io::Read;
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(256 * 1024).read_to_end(&mut bytes).ok()?;
    if bytes.len() < 0x40 || &bytes[0..2] != b"MZ" {
        return None;
    }
    let e_lfanew = u32::from_le_bytes(bytes.get(0x3c..0x40)?.try_into().ok()?) as usize;
    let pe = bytes.get(e_lfanew..e_lfanew + 24)?;
    if &pe[0..4] != b"PE\0\0" {
        return None;
    }
    let sections = u16::from_le_bytes(pe.get(6..8)?.try_into().ok()?) as usize;
    let optional = u16::from_le_bytes(pe.get(20..22)?.try_into().ok()?) as usize;
    let table = e_lfanew + 24 + optional;
    for index in 0..sections.min(96) {
        let offset = table + index * 40;
        let name = String::from_utf8_lossy(bytes.get(offset..offset + 8)?)
            .trim_end_matches('\0')
            .to_ascii_lowercase();
        if let Some((_, label, requires_steam)) = SECTION_MARKERS
            .iter()
            .find(|(marker, _, _)| name.contains(marker))
        {
            return Some((*label, *requires_steam));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("lapinux-drm-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("dossier de test");
        dir
    }

    /// Un PE minimal : une seule section, nommée comme le demande le test.
    fn pe_with_section(name: &str) -> Vec<u8> {
        let e_lfanew = 0x40usize;
        let table = e_lfanew + 24;
        let mut bytes = vec![0u8; table + 40];
        bytes[0..2].copy_from_slice(b"MZ");
        bytes[0x3c..0x40].copy_from_slice(&(e_lfanew as u32).to_le_bytes());
        bytes[e_lfanew..e_lfanew + 4].copy_from_slice(b"PE\0\0");
        bytes[e_lfanew + 6..e_lfanew + 8].copy_from_slice(&1u16.to_le_bytes());
        let take = name.len().min(8);
        bytes[table..table + take].copy_from_slice(&name.as_bytes()[..take]);
        bytes
    }

    #[test]
    fn an_empty_folder_is_free_of_drm_and_client() {
        let dir = temp("vide");
        let report = scan(&dir).expect("dossier lu");
        assert_eq!(report.drm, None);
        assert_eq!(report.client, None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_steam_library_names_both_the_drm_and_the_client() {
        let dir = temp("steam");
        fs::create_dir_all(dir.join("bin")).unwrap();
        fs::write(dir.join("bin/steam_api64.dll"), b"MZ").unwrap();
        let report = scan(&dir).expect("dossier lu");
        assert_eq!(report.drm, Some("Steam"));
        assert_eq!(report.client, Some("Steam"));
        assert!(!report.requires_steam);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_denuvo_file_names_the_protection_without_a_client() {
        let dir = temp("denuvo");
        fs::write(dir.join("Denuvo64.dll"), b"").unwrap();
        let report = scan(&dir).expect("dossier lu");
        assert_eq!(report.drm, Some("Denuvo"));
        assert_eq!(report.client, None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_steamstub_section_is_read_in_the_executable() {
        let dir = temp("stub");
        fs::write(dir.join("jeu.exe"), pe_with_section(".bind")).unwrap();
        let report = scan(&dir).expect("dossier lu");
        assert_eq!(report.drm, Some("Steam"));
        assert_eq!(report.client, Some("Steam"));
        assert!(report.requires_steam);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn installer_payloads_do_not_name_the_game_protection() {
        let dir = temp("redist");
        fs::create_dir_all(dir.join("_CommonRedist/Steam")).unwrap();
        fs::write(dir.join("_CommonRedist/Steam/steam_api64.dll"), b"MZ").unwrap();
        let report = scan(&dir).expect("dossier lu");
        assert_eq!(report.drm, None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_folder_that_is_not_there_is_unknown() {
        let absent = std::env::temp_dir().join("lapinux-drm-absent");
        let _ = fs::remove_dir_all(&absent);
        assert!(scan(&absent).is_none());
    }
}
