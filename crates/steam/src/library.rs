//! Découverte des bibliothèques Steam et construction de la liste de jeux.

use crate::drm;
use crate::manifest::{appid_from_filename, AppManifest};
use crate::vdf;
use crate::Result;
use ludus_core::model::{Drm, Game, GameSettings};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct Library {
    pub root: PathBuf,
}

impl Library {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn steamapps(&self) -> PathBuf {
        self.root.join("steamapps")
    }

    pub fn common_dir(&self) -> PathBuf {
        self.steamapps().join("common")
    }

    pub fn compatdata_dir(&self) -> PathBuf {
        self.steamapps().join("compatdata")
    }

    pub fn manifest_path(&self, appid: u32) -> PathBuf {
        self.steamapps().join(format!("appmanifest_{appid}.acf"))
    }

    pub fn prefix(&self, appid: u32) -> PathBuf {
        self.compatdata_dir().join(appid.to_string())
    }

    pub fn install_dir(&self, installdir: &str) -> PathBuf {
        self.common_dir().join(installdir)
    }
}

#[derive(Debug, Clone, Default)]
pub struct ScanReport {
    pub games: Vec<Game>,
    pub libraries: Vec<PathBuf>,
    pub warnings: Vec<String>,
}

/// Toutes les bibliothèques déclarées par les racines Steam fournies.
pub fn discover_libraries(steam_roots: &[PathBuf]) -> Vec<Library> {
    let mut libraries: Vec<Library> = Vec::new();
    let mut push = |path: PathBuf| {
        if !path.join("steamapps").is_dir() {
            return;
        }
        let canon = std::fs::canonicalize(&path).unwrap_or(path);
        if !libraries.iter().any(|l| l.root == canon) {
            libraries.push(Library::new(canon));
        }
    };

    for root in steam_roots {
        push(root.clone());
        let libraryfolders = root.join("steamapps/libraryfolders.vdf");
        let Ok(text) = std::fs::read_to_string(&libraryfolders) else {
            continue;
        };
        let Ok(obj) = vdf::parse_root(&text) else {
            continue;
        };
        for (_key, entry) in obj.objects() {
            if let Some(path) = entry.str("path") {
                push(PathBuf::from(path));
            }
        }
    }
    libraries
}

pub fn scan(steam_roots: &[PathBuf]) -> ScanReport {
    let libraries = discover_libraries(steam_roots);
    let mut report = ScanReport {
        libraries: libraries.iter().map(|l| l.root.clone()).collect(),
        ..Default::default()
    };
    for library in &libraries {
        match scan_library(library) {
            Ok(mut games) => report.games.append(&mut games),
            Err(err) => report
                .warnings
                .push(format!("{} : {err}", library.root.display())),
        }
    }
    report.games.sort_by(|a, b| b.installed.cmp(&a.installed).then(a.name.cmp(&b.name)));
    report
}

pub fn scan_library(library: &Library) -> Result<Vec<Game>> {
    let steamapps = library.steamapps();
    let entries = std::fs::read_dir(&steamapps)?;
    let mut games = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let is_manifest = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.starts_with("appmanifest_") && n.ends_with(".acf"))
            .unwrap_or(false);
        if !is_manifest {
            continue;
        }
        let Some(appid) = appid_from_filename(&path) else {
            continue;
        };
        let manifest = match AppManifest::read(&path) {
            Ok(m) => m,
            Err(err) => {
                tracing::warn!("manifeste illisible {} : {err}", path.display());
                continue;
            }
        };
        games.push(build_game(library, manifest));
        let _ = appid;
    }
    Ok(games)
}

fn build_game(library: &Library, manifest: AppManifest) -> Game {
    let install_dir = library.install_dir(&manifest.installdir);
    let prefix = library.prefix(manifest.appid);
    let native_exe = find_native_executable(&install_dir, &manifest.name);
    let exe = native_exe
        .clone()
        .or_else(|| find_launch_exe(&install_dir, &manifest.name));
    let drm = if native_exe.is_some() {
        Drm::None
    } else {
        drm::detect(&install_dir, exe.as_deref())
    };
    let installed = manifest.installed() && install_dir.is_dir();
    Game {
        appid: manifest.appid,
        name: manifest.name,
        installdir: manifest.installdir,
        library: library.root.clone(),
        size_on_disk: manifest.size_on_disk,
        state_flags: manifest.state_flags,
        last_updated: manifest.last_updated,
        last_played: (manifest.last_played > 0).then_some(manifest.last_played),
        build_id: manifest.build_id,
        installed,
        prefix: prefix.is_dir().then_some(prefix),
        exe,
        native: native_exe.is_some(),
        drm,
        settings: GameSettings::default(),
        playtime_secs: 0,
        favorite: false,
    }
}

/// Conserve les réglages et statistiques des jeux déjà connus.
pub fn merge_existing(fresh: Vec<Game>, stored: &[Game]) -> Vec<Game> {
    fresh
        .into_iter()
        .map(|mut game| {
            if let Some(old) = stored.iter().find(|g| g.appid == game.appid) {
                game.settings = old.settings.clone();
                game.playtime_secs = old.playtime_secs;
                game.favorite = old.favorite;
                game.last_played = game.last_played.or(old.last_played);
            }
            game
        })
        .collect()
}

fn normalize(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

const JUNK: [&str; 14] = [
    "unins",
    "uninstall",
    "setup",
    "install",
    "dxsetup",
    "vcredist",
    "vc_redist",
    "dotnetfx",
    "oalinst",
    "crashhandler",
    "ue4prereq",
    "ue5prereq",
    "prereq",
    "report",
];

fn is_junk(stem: &str) -> bool {
    let lower = stem.to_ascii_lowercase();
    JUNK.iter().any(|junk| lower.contains(junk))
}

/// Trouve l'exécutable Windows le plus plausible d'une installation.
pub fn find_launch_exe(install_dir: &Path, game_name: &str) -> Option<PathBuf> {
    if !install_dir.is_dir() {
        return None;
    }
    let target = normalize(game_name);
    let mut candidates: Vec<(i32, PathBuf)> = Vec::new();
    let mut budget = 40_000usize;
    walk(install_dir, 0, &mut budget, &mut |path, depth| {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            return;
        };
        if !name.to_ascii_lowercase().ends_with(".exe") {
            return;
        }
        let stem = name.trim_end_matches(".exe");
        let normalized = normalize(stem);
        let mut score: i32 = 10;
        if !target.is_empty() && normalized == target {
            score += 1000;
        } else if !target.is_empty() && normalized.starts_with(&target) {
            score += 250;
        } else if !target.is_empty() && target.starts_with(&normalized) && normalized.len() > 3 {
            score += 200;
        } else if !target.is_empty() && normalized.contains(&target) {
            score += 150;
        }
        if is_junk(stem) {
            score -= 800;
        }
        if stem.to_ascii_lowercase().contains("shipping") {
            score += 40;
        }
        let lower_path = path.to_string_lossy().to_ascii_lowercase();
        if lower_path.contains("win64") || lower_path.contains("x64") || lower_path.contains("64") {
            score += 25;
        }
        score += (4i32 - depth as i32) * 10;
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        score += (size / (8 * 1024 * 1024)).min(20) as i32;
        candidates.push((score, path.to_path_buf()));
    });
    candidates.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    candidates
        .into_iter()
        .find(|(score, _)| *score > 0)
        .map(|(_, path)| path)
}

/// Détecte un binaire Linux natif exécutable (ELF) à la racine de l'installation.
pub fn find_native_executable(install_dir: &Path, game_name: &str) -> Option<PathBuf> {
    if !install_dir.is_dir() {
        return None;
    }
    let target = normalize(game_name);
    let mut best: Option<(i32, PathBuf)> = None;
    let entries = std::fs::read_dir(install_dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let lower = name.to_ascii_lowercase();
        if lower.starts_with('.')
            || lower.ends_with(".exe")
            || lower.ends_with(".dll")
            || lower.ends_with(".so")
        {
            continue;
        }
        let exec_bit = {
            use std::os::unix::fs::PermissionsExt;
            meta.permissions().mode() & 0o111 != 0
        };
        let is_elf = {
            use std::io::Read;
            std::fs::File::open(&path)
                .ok()
                .and_then(|mut file| {
                    let mut magic = [0u8; 4];
                    file.read_exact(&mut magic).ok().map(|_| magic)
                })
                .map(|magic| &magic == b"\x7fELF")
                .unwrap_or(false)
        };
        let is_script = lower.ends_with(".sh") && exec_bit;
        if !(is_elf || is_script) {
            continue;
        }
        let normalized = normalize(name.trim_end_matches(".sh"));
        let mut score = 10;
        if normalized == target {
            score += 1000;
        } else if !target.is_empty() && normalized.contains(&target) {
            score += 200;
        }
        if lower.contains("run") || lower.contains("start") {
            score += 40;
        }
        if lower.contains("server") || lower.contains("dedicated") {
            score -= 200;
        }
        if best.as_ref().map(|(s, _)| score > *s).unwrap_or(true) {
            best = Some((score, path));
        }
    }
    best.filter(|(score, _)| *score > 0).map(|(_, p)| p)
}

fn walk(dir: &Path, depth: usize, budget: &mut usize, visit: &mut dyn FnMut(&Path, usize)) {
    if depth > 3 || *budget == 0 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if *budget == 0 {
            return;
        }
        *budget -= 1;
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            walk(&path, depth + 1, budget, visit);
        } else if file_type.is_file() {
            visit(&path, depth);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch(path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn make_library(dir: &Path, appid: u32, name: &str, installdir: &str) -> Library {
        let lib = Library::new(dir);
        fs::create_dir_all(lib.steamapps()).unwrap();
        fs::create_dir_all(lib.install_dir(installdir)).unwrap();
        fs::write(
            lib.manifest_path(appid),
            format!(
                "\"AppState\" {{ \"appid\" \"{appid}\" \"name\" \"{name}\" \"StateFlags\" \"4\" \"installdir\" \"{installdir}\" \"SizeOnDisk\" \"1000\" }}"
            ),
        )
        .unwrap();
        lib
    }

    #[test]
    fn scans_manifest_and_exe() {
        let dir = tempfile::tempdir().unwrap();
        let lib = make_library(dir.path(), 440, "Team Fortress 2", "Team Fortress 2");
        touch(
            &lib.install_dir("Team Fortress 2").join("hl2.exe"),
            b"MZ\x90\0",
        );
        touch(
            &lib.install_dir("Team Fortress 2").join("bin/win64/hl2.exe"),
            &crate::drm::tests::fake_pe(&[".text"], 64),
        );
        let games = scan_library(&lib).unwrap();
        assert_eq!(games.len(), 1);
        let game = &games[0];
        assert_eq!(game.appid, 440);
        assert_eq!(game.name, "Team Fortress 2");
        assert!(game.installed);
        assert!(game.exe.is_some());
        // Le binaire 64 bits est préféré.
        assert!(game.exe.as_ref().unwrap().to_string_lossy().contains("win64"));
    }

    #[test]
    fn ignores_uninstalled_flag_and_junk_exe() {
        let dir = tempfile::tempdir().unwrap();
        let lib = Library::new(dir.path());
        fs::create_dir_all(lib.steamapps()).unwrap();
        fs::create_dir_all(lib.install_dir("X")).unwrap();
        fs::write(
            lib.manifest_path(7),
            "\"AppState\" { \"appid\" \"7\" \"name\" \"X\" \"StateFlags\" \"1\" \"installdir\" \"X\" }",
        )
        .unwrap();
        touch(&lib.install_dir("X").join("unins000.exe"), b"MZ");
        touch(&lib.install_dir("X").join("X.exe"), b"MZ");
        let games = scan_library(&lib).unwrap();
        assert!(!games[0].installed);
        assert_eq!(
            games[0].exe.as_ref().unwrap().file_name().unwrap(),
            "X.exe"
        );
    }

    #[test]
    fn detects_native_linux_build() {
        let dir = tempfile::tempdir().unwrap();
        let lib = make_library(dir.path(), 9, "Super Jeu", "Super Jeu");
        let exe = lib.install_dir("Super Jeu").join("superjeu");
        std::fs::write(&exe, b"\x7fELF\x02\x01\x01").unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
        let games = scan_library(&lib).unwrap();
        assert!(games[0].native);
    }

    #[test]
    fn library_folders_are_followed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Steam");
        let external = dir.path().join("Jeux/SteamLibrary");
        fs::create_dir_all(root.join("steamapps")).unwrap();
        fs::create_dir_all(external.join("steamapps")).unwrap();
        fs::write(
            root.join("steamapps/libraryfolders.vdf"),
            format!(
                "\"libraryfolders\" {{ \"0\" {{ \"path\" \"{}\" }} \"1\" {{ \"path\" \"{}\" }} }}",
                root.display(),
                external.display()
            ),
        )
        .unwrap();
        let libs = discover_libraries(&[root.clone()]);
        assert_eq!(libs.len(), 2);
    }

    #[test]
    fn merge_keeps_settings() {
        let fresh = Game {
            appid: 1,
            name: "A".into(),
            ..Default::default()
        };
        let stored = vec![Game {
            appid: 1,
            settings: GameSettings {
                mangohud: Some(true),
                ..Default::default()
            },
            playtime_secs: 7200,
            favorite: true,
            ..Default::default()
        }];
        let merged = merge_existing(vec![fresh], &stored);
        assert_eq!(merged[0].settings.mangohud, Some(true));
        assert_eq!(merged[0].playtime_secs, 7200);
        assert!(merged[0].favorite);
    }
}
