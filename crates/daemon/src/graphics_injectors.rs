//! Injecteurs Windows facultatifs, sans exécuter les installateurs téléchargés.
use crate::{Result, Shared};
use lapinux_core::configurations::{
    GraphicsInjector, GraphicsInjectorFile, GraphicsInjectorFileStatus, GraphicsInjectorStatus,
};
use lapinux_core::ipc::Envelope;
use lapinux_core::model::{Game, JobKind, JobState};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

const RECORDS: &str = "graphics_injectors_v1";
const OPTI_URL: &str = "https://github.com/optiscaler/OptiScaler/releases/download/v0.9.4/Optiscaler_0.9.4-final.20260718._MM.7z";
const OPTI_HASH: &str = "575cb4df866116093df75af607e37fd70e10f5163e0f23fd5c804142e80ef0ad";
const RESHADE_URL: &str = "https://reshade.me/downloads/ReShade_Setup_6.8.0.exe";
const RESHADE_HASH: &str = "207aea16205fbf952bc8fe1879966672454cf04002e7ad34237c7990a5b3c0b4";
// Archive officielle 7.1 (empreinte également publiée par le miroir SourceForge).
const CNC_URL: &str =
    "https://github.com/FunkyFr3sh/cnc-ddraw/releases/download/v7.1.0.0/cnc-ddraw.zip";
const CNC_HASH: &str = "0b13ab89a64c9918189b1dadd449ef6ed3cb3b7b19cabd96d8adbd95505bb908";
const CNC_DLL_HASH: &str = "85e0f7d530dfda134793a57cb3e76b0287dcc96892ee57162dd68f47283b03a9";
const CNC_INI_HASH: &str = "2571322a64a767022627ed076f9539179eff322efa336016e8af1f2337cc8fcb";
const CNC_LICENSE: &str = "MIT License\n\nCopyright (c) 2022 github.com/FunkyFr3sh\n\nPermission is hereby granted, free of charge, to any person obtaining a copy\nof this software and associated documentation files (the \"Software\"), to deal\nin the Software without restriction, including without limitation the rights\nto use, copy, modify, merge, publish, distribute, sublicense, and/or sell\ncopies of the Software, and to permit persons to whom the Software is\nfurnished to do so, subject to the following conditions:\n\nThe above copyright notice and this permission notice shall be included in all\ncopies or substantial portions of the Software.\n\nTHE SOFTWARE IS PROVIDED \"AS IS\", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR\nIMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,\nFITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE\nAUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER\nLIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,\nOUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE\nSOFTWARE.\n";

type Records = HashMap<u32, Vec<GraphicsInjector>>;
fn error(e: impl ToString) -> lapinux_core::Error {
    lapinux_core::Error::Other(e.to_string())
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn hash_file(path: &Path) -> std::result::Result<String, String> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !meta.file_type().is_file() || meta.len() > 100 * 1024 * 1024 {
        return Err(format!("fichier invalide : {}", path.display()));
    }
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut h).map_err(|e| e.to_string())?;
    Ok(format!("{:x}", h.finalize()))
}
fn safe_relative(s: &str) -> std::result::Result<&Path, String> {
    let p = Path::new(s);
    if s.contains('\\') || s.chars().any(char::is_control) {
        return Err("chemin relatif invalide".into());
    }
    if s.is_empty() || !p.components().all(|c| matches!(c, Component::Normal(_))) {
        return Err("chemin relatif invalide".into());
    }
    Ok(p)
}
fn checked(root: &Path, relative: &str) -> std::result::Result<PathBuf, String> {
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let mut p = root;
    for c in safe_relative(relative)?.components() {
        p.push(c);
        match std::fs::symlink_metadata(&p) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err(format!("lien symbolique refusé : {}", p.display()))
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(p)
}
fn pe_arch(path: &Path) -> std::result::Result<u16, String> {
    use std::io::{Seek, SeekFrom};
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hdr = [0u8; 64];
    f.read_exact(&mut hdr)
        .map_err(|_| "exécutable PE invalide")?;
    if &hdr[..2] != b"MZ" {
        return Err("exécutable Windows PE requis".into());
    }
    let offset = u32::from_le_bytes(hdr[60..64].try_into().unwrap());
    if offset > 16 * 1024 * 1024 {
        return Err("en-tête PE invalide".into());
    }
    f.seek(SeekFrom::Start(offset as u64))
        .map_err(|e| e.to_string())?;
    let mut pe = [0u8; 6];
    f.read_exact(&mut pe).map_err(|_| "en-tête PE tronqué")?;
    if &pe[..4] != b"PE\0\0" {
        return Err("en-tête PE invalide".into());
    }
    let machine = u16::from_le_bytes([pe[4], pe[5]]);
    if machine != 0x14c && machine != 0x8664 {
        return Err("architecture PE non prise en charge".into());
    }
    Ok(machine)
}
fn download(url: &str, expected: &str, limit: u64) -> std::result::Result<Vec<u8>, String> {
    let response = ureq::get(url)
        .set("User-Agent", "Lapinux")
        .timeout(std::time::Duration::from_secs(180))
        .call()
        .map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit || digest(&bytes) != expected {
        return Err("archive graphique : taille ou SHA-256 incorrect".into());
    }
    Ok(bytes)
}
fn params(kind: &str, version: &str, proxy: &str) -> std::result::Result<(), String> {
    let valid = match kind {
        "optiscaler" => {
            version == "0.9.4"
                && [
                    "dxgi.dll",
                    "d3d12.dll",
                    "dbghelp.dll",
                    "winmm.dll",
                    "version.dll",
                    "wininet.dll",
                    "winhttp.dll",
                ]
                .contains(&proxy)
        }
        "reshade" => {
            version == "6.8.0"
                && [
                    "dxgi.dll",
                    "d3d9.dll",
                    "d3d11.dll",
                    "d3d12.dll",
                    "opengl32.dll",
                ]
                .contains(&proxy)
        }
        "cnc-ddraw" => version == "7.1.0.0" && proxy == "ddraw.dll",
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err("outil, version ou proxy non pris en charge".into())
    }
}
fn files_in(dir: &Path) -> std::result::Result<Vec<GraphicsInjectorFile>, String> {
    fn walk(
        base: &Path,
        current: &Path,
        out: &mut Vec<GraphicsInjectorFile>,
    ) -> std::result::Result<(), String> {
        for entry in std::fs::read_dir(current).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let t = entry.file_type().map_err(|e| e.to_string())?;
            if t.is_dir() {
                walk(base, &entry.path(), out)?;
            } else if t.is_file() {
                let path = entry.path();
                let name = path
                    .strip_prefix(base)
                    .map_err(|e| e.to_string())?
                    .to_str()
                    .ok_or("nom non UTF-8")?
                    .to_string();
                safe_relative(&name)?;
                if out.iter().any(|item| item.name.eq_ignore_ascii_case(&name)) {
                    return Err("archive : noms de fichiers ambigus".into());
                }
                out.push(GraphicsInjectorFile {
                    name,
                    sha256: hash_file(&path)?,
                });
            } else {
                return Err("archive : lien ou fichier spécial refusé".into());
            }
            if out.len() > 100 {
                return Err("archive : trop de fichiers".into());
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out)?;
    Ok(out)
}

/// Déplace un fichier modifié hors du dossier du jeu avant une
/// réinstallation, sans jamais écraser une sauvegarde précédente.
fn move_to_backup(path: &Path, dir: &Path) -> std::result::Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "fichier".into());
    let mut dest = dir.join(&name);
    let mut index = 1;
    while dest.exists() {
        dest = dir.join(format!("{name}.{index}"));
        index += 1;
    }
    if std::fs::rename(path, &dest).is_err() {
        std::fs::copy(path, &dest).map_err(|e| e.to_string())?;
        std::fs::remove_file(path).map_err(|e| e.to_string())?;
    }
    Ok(dest)
}

/// Remet un `ddraw.ini` mis de côté, en remplaçant l'INI officiel s'il existe.
fn restore_ini(aside: Option<&Path>, folder: &Path) {
    let Some(aside) = aside else { return };
    if !aside.exists() {
        return;
    }
    let _ = std::fs::create_dir_all(folder);
    let dest = folder.join("ddraw.ini");
    if std::fs::rename(aside, &dest).is_err() {
        if let Ok(bytes) = std::fs::read(aside) {
            let tmp = folder.join(".lapinux-ddraw.ini.restore");
            if std::fs::write(&tmp, bytes).is_ok() && std::fs::rename(&tmp, &dest).is_ok() {
                let _ = std::fs::remove_file(aside);
            }
        }
    }
}

fn extract(
    kind: &str,
    machine: u16,
    bytes: &[u8],
    stage: &Path,
    reshade_hash: Option<&str>,
    cnc_ini_hash: Option<&str>,
) -> std::result::Result<(), String> {
    if kind == "reshade" {
        let name = if machine == 0x8664 {
            "ReShade64.dll"
        } else {
            "ReShade32.dll"
        };
        let expected = if machine == 0x8664 {
            "b2945c29e7095491a901746b400e58db9b1592ab092bacf2a888ce37f02d08da"
        } else {
            "538998f66c0197adcdeadfe8e5ea19dd6ca3253e40399476d291f385ce518b88"
        };
        let mut zip =
            zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
        let mut entry = zip.by_name(name).map_err(|e| e.to_string())?;
        if entry.size() > 8 * 1024 * 1024 {
            return Err("DLL ReShade trop grosse".into());
        }
        let mut dll = Vec::new();
        entry.read_to_end(&mut dll).map_err(|e| e.to_string())?;
        if digest(&dll) != reshade_hash.unwrap_or(expected) {
            return Err("DLL ReShade : SHA-256 incorrect".into());
        }
        std::fs::write(stage.join(name), dll).map_err(|e| e.to_string())?;
    } else if kind == "cnc-ddraw" {
        if machine != 0x14c {
            return Err("cnc-ddraw 7.1 nécessite un exécutable Windows 32 bits".into());
        }
        let mut zip =
            zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
        for (name, expected, limit) in [
            ("ddraw.dll", CNC_DLL_HASH, 1024 * 1024),
            ("ddraw.ini", CNC_INI_HASH, 128 * 1024),
        ] {
            let mut entry = zip.by_name(name).map_err(|e| e.to_string())?;
            if entry.size() > limit {
                return Err(format!("{name} trop volumineux"));
            }
            let mut data = Vec::new();
            entry.read_to_end(&mut data).map_err(|e| e.to_string())?;
            let hash = if name == "ddraw.dll" {
                reshade_hash.unwrap_or(expected)
            } else {
                cnc_ini_hash.unwrap_or(expected)
            };
            if digest(&data) != hash {
                return Err(format!("{name} : SHA-256 incorrect"));
            }
            std::fs::write(stage.join(name), data).map_err(|e| e.to_string())?;
        }
        if pe_arch(&stage.join("ddraw.dll"))? != 0x14c {
            return Err("DLL cnc-ddraw : architecture invalide".into());
        }
        std::fs::write(stage.join("LICENSE.cnc-ddraw.txt"), CNC_LICENSE)
            .map_err(|e| e.to_string())?;
    } else {
        if machine != 0x8664 {
            return Err("OptiScaler 0.9.4 nécessite un exécutable Windows 64 bits".into());
        }
        let archive = stage.join("archive.7z");
        std::fs::write(&archive, bytes).map_err(|e| e.to_string())?;
        // Archive intégralement épinglée, extraite seulement dans notre dossier neuf.
        let status = std::process::Command::new("7z")
            .arg("x")
            .arg("-y")
            .arg("-bd")
            .arg(format!("-o{}", stage.display()))
            .arg(&archive)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|e| format!("OptiScaler nécessite 7z : {e}"))?;
        if !status.success() {
            return Err("extraction 7z impossible".into());
        }
        std::fs::remove_file(archive).map_err(|e| e.to_string())?;
        if !stage.join("OptiScaler.dll").is_file() || !stage.join("OptiScaler.ini").is_file() {
            return Err("archive OptiScaler incomplète".into());
        }
    }
    Ok(())
}
fn same_file(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::symlink_metadata(a), std::fs::symlink_metadata(b)) {
        (Ok(a), Ok(b)) => {
            a.file_type().is_file()
                && b.file_type().is_file()
                && a.dev() == b.dev()
                && a.ino() == b.ino()
        }
        _ => false,
    }
}
fn link_sources(
    stage: &Path,
    folder: &Path,
    sources: &BTreeMap<String, String>,
) -> std::result::Result<(), String> {
    for (name, source) in sources {
        std::fs::hard_link(stage.join(source), folder.join(name)).map_err(|e| format!("installation interrompue ({e}) : utilise Retirer pour récupérer les fichiers suivis"))?;
    }
    Ok(())
}
fn cnc_conflicts(
    game: &Game,
    frame: lapinux_core::config::FrameGeneration,
    vkbasalt: bool,
) -> std::result::Result<(), String> {
    use lapinux_core::config::FrameGeneration::{LsfgVk, Off};
    let value = |key: &str| {
        game.settings
            .env
            .iter()
            .rev()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.clone())
            .or_else(|| std::env::var(key).ok())
    };
    let active = |key: &str| value(key).is_some_and(|v| !v.is_empty() && v != "0");
    if game.settings.compatibility.wined3d
        || active("PROTON_USE_WINED3D")
        || value("STEAM_COMPAT_CONFIG")
            .unwrap_or_default()
            .split([',', ';', ' '])
            .any(|s| s.eq_ignore_ascii_case("wined3d"))
    {
        return Err("désactive WineD3D avant d'utiliser cnc-ddraw".into());
    }
    if !matches!(frame, Off | LsfgVk)
        || vkbasalt
        || [
            "AFMF_ENABLE",
            "ENABLE_VKBASALT",
            "NVPRESENT_ENABLE_SMOOTH_MOTION",
            "PROTON_USE_OPTISCALER",
        ]
        .iter()
        .any(|key| active(key))
    {
        return Err(
            "cnc-ddraw ne doit pas être combiné avec vkBasalt ou la génération d'images".into(),
        );
    }
    if game
        .settings
        .env
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("CNC_DDRAW_CONFIG_FILE"))
        || std::env::var_os("CNC_DDRAW_CONFIG_FILE").is_some()
    {
        return Err("CNC_DDRAW_CONFIG_FILE est déjà défini : cnc-ddraw pourrait modifier une autre configuration".into());
    }
    if value("WINEDLLOVERRIDES")
        .unwrap_or_default()
        .split(';')
        .any(|rule| {
            rule.split('=').next().unwrap_or("").split(',').any(|n| {
                let n = n.trim().to_ascii_lowercase();
                n.strip_suffix(".dll").unwrap_or(&n) == "ddraw"
            })
        })
    {
        return Err("WINEDLLOVERRIDES contient déjà ddraw".into());
    }
    Ok(())
}
/// Nom du fichier officiel dans le dossier de suivi pour une cible donnée.
fn stage_source_name<'a>(record: &'a GraphicsInjector, file: &'a GraphicsInjectorFile) -> &'a str {
    if file.name != record.proxy {
        return &file.name;
    }
    if record.kind == "optiscaler" {
        "OptiScaler.dll"
    } else if record.kind == "cnc-ddraw" {
        "ddraw.dll"
    } else if record.staged.iter().any(|f| f.name == "ReShade64.dll") {
        "ReShade64.dll"
    } else {
        "ReShade32.dll"
    }
}

fn setting_value(game: &Game, key: &str) -> Option<String> {
    game.settings
        .env
        .iter()
        .rev()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.clone())
        .or_else(|| std::env::var(key).ok())
}

fn setting_active(game: &Game, key: &str) -> bool {
    setting_value(game, key).is_some_and(|v| !v.is_empty() && v != "0")
}

fn effective_frame_generation(
    game: &Game,
    config: &lapinux_core::config::Config,
) -> lapinux_core::config::FrameGeneration {
    game.settings.frame_generation.unwrap_or(config.graphics.frame_generation).supported()
}

/// Ce que le lancement fera réellement de l'override, sans toucher au disque.
fn injector_override_state(
    game: &Game,
    record: &GraphicsInjector,
    frame: lapinux_core::config::FrameGeneration,
    vkbasalt: bool,
) -> (&'static str, &'static str) {
    use lapinux_core::config::FrameGeneration::{LsfgVk, Off};
    let dll = record.proxy.trim_end_matches(".dll");
    let manual = setting_value(game, "WINEDLLOVERRIDES").unwrap_or_default();
    let mut native = false;
    for rule in manual.split(';') {
        let (names, actions) = rule.split_once('=').unwrap_or((rule, ""));
        if names.split(',').any(|name| {
            let name = name.trim().to_ascii_lowercase();
            name.strip_suffix(".dll")
                .unwrap_or(&name)
                .eq_ignore_ascii_case(dll)
        }) {
            if actions.split(',').any(|action| {
                let action = action.trim();
                action.eq_ignore_ascii_case("n") || action.eq_ignore_ascii_case("native")
            }) {
                native = true;
            } else {
                return ("blocked", "manual_override");
            }
        }
    }
    if record.kind == "optiscaler" {
        return ("blocked", "optiscaler_removed");
    }
    // Les causes manuelles priment : une variable du joueur ne peut pas être
    // retirée automatiquement. Les signaler avant les réglages corrigeables
    // évite de proposer « Corriger les réglages » alors qu'un autre blocage
    // resterait après la correction.
    if setting_active(game, "PROTON_USE_OPTISCALER") {
        return ("blocked", "optiscaler_proton");
    }
    if record.kind == "cnc-ddraw" {
        let env_wined3d = setting_active(game, "PROTON_USE_WINED3D")
            || setting_value(game, "STEAM_COMPAT_CONFIG")
                .unwrap_or_default()
                .split([',', ';', ' '])
                .any(|item| item.eq_ignore_ascii_case("wined3d"));
        if env_wined3d {
            return ("blocked", "wined3d_env");
        }
        if setting_active(game, "AFMF_ENABLE")
            || setting_active(game, "NVPRESENT_ENABLE_SMOOTH_MOTION")
        {
            return ("blocked", "frame_generation_env");
        }
        if setting_active(game, "ENABLE_VKBASALT") {
            return ("blocked", "vkbasalt_env");
        }
        if setting_value(game, "CNC_DDRAW_CONFIG_FILE").is_some() {
            return ("blocked", "config_file");
        }
    } else if record.kind == "reshade" && setting_active(game, "ENABLE_VKBASALT") {
        return ("blocked", "vkbasalt_env");
    }
    if record.kind == "cnc-ddraw" {
        if game.settings.compatibility.wined3d {
            return ("blocked", "wined3d");
        }
        if !matches!(frame, Off | LsfgVk) {
            return ("blocked", "frame_generation");
        }
        if vkbasalt {
            return ("blocked", "vkbasalt");
        }
    } else if record.kind == "optiscaler" && !matches!(frame, Off | LsfgVk) {
        return ("blocked", "frame_generation");
    } else if record.kind == "reshade" {
        if matches!(frame, lapinux_core::config::FrameGeneration::OptiScaler) {
            return ("blocked", "frame_generation");
        }
        if vkbasalt {
            return ("blocked", "vkbasalt");
        }
    }
    if native {
        ("already", "")
    } else {
        ("applied", "")
    }
}

/// Un fichier de la copie de suivi est utilisable si son empreinte est celle
/// de l'archive, ou s'il partage son inode avec une cible suivie : le lien
/// physique fait alors apparaître la modification du joueur des deux côtés.
/// `official_only` n'accepte ce partage que pour un `ddraw.ini` personnalisé,
/// dont le contenu officiel n'est pas nécessaire à la réparation des DLL.
fn staged_files_ok(
    record: &GraphicsInjector,
    stage: &Path,
    folder: Option<&Path>,
    official_only: bool,
) -> bool {
    record.staged.iter().all(|staged| {
        let Ok(path) = checked(stage, &staged.name) else {
            return false;
        };
        if hash_file(&path).ok().as_deref() == Some(staged.sha256.as_str()) {
            return true;
        }
        folder.is_some_and(|folder| {
            record.files.iter().any(|file| {
                stage_source_name(record, file) == staged.name
                    && (!official_only || (record.kind == "cnc-ddraw" && file.name == "ddraw.ini"))
                    && std::fs::symlink_metadata(folder.join(&file.name)).is_ok()
                    && same_file(&path, &folder.join(&file.name))
            })
        })
    })
}

fn injector_status(
    game: &Game,
    record: &GraphicsInjector,
    frame: lapinux_core::config::FrameGeneration,
    vkbasalt: bool,
    protected: bool,
) -> GraphicsInjectorStatus {
    let root = game.install_dir();
    let recorded_exe = checked(&root, &record.executable)
        .ok()
        .and_then(|path| path.canonicalize().ok());
    let current_exe = game.exe.as_ref().and_then(|exe| exe.canonicalize().ok());
    let exe_ok = current_exe.is_some() && recorded_exe == current_exe;
    let folder = recorded_exe
        .as_ref()
        .and_then(|exe| exe.parent())
        .map(Path::to_path_buf);
    let stage = checked(&root, &record.stage).ok();
    let stage_ok = stage
        .as_ref()
        .is_some_and(|stage| staged_files_ok(record, stage, folder.as_deref(), false));
    let mut files = Vec::new();
    let mut missing = false;
    let mut modified = false;
    match &folder {
        Some(folder) => {
            for file in &record.files {
                let target = folder.join(&file.name);
                let state = match std::fs::symlink_metadata(&target) {
                    Err(_) => {
                        missing = true;
                        "missing"
                    }
                    Ok(meta) => {
                        let hash = meta
                            .file_type()
                            .is_file()
                            .then(|| hash_file(&target).ok())
                            .flatten();
                        if hash.as_deref() == Some(file.sha256.as_str()) {
                            let linked = stage.as_ref().is_some_and(|stage| {
                                same_file(&stage.join(stage_source_name(record, file)), &target)
                            });
                            if linked || !stage_ok {
                                "intact"
                            } else {
                                "unlinked"
                            }
                        } else if record.kind == "cnc-ddraw" && file.name == "ddraw.ini" {
                            "personalized"
                        } else {
                            modified = true;
                            "modified"
                        }
                    }
                };
                files.push(GraphicsInjectorFileStatus {
                    name: file.name.clone(),
                    state: state.into(),
                });
            }
        }
        None => {
            missing = true;
            files.extend(record.files.iter().map(|file| GraphicsInjectorFileStatus {
                name: file.name.clone(),
                state: "missing".into(),
            }));
        }
    }
    // Un exécutable changé prime : la réinstallation sait désormais sauvegarder
    // les fichiers modifiés et reconstruire une copie de suivi corrompue.
    let install = if !exe_ok {
        "executable_changed"
    } else if !record.ready || !stage_ok {
        "incomplete"
    } else if missing {
        "missing"
    } else if modified {
        "modified"
    } else {
        "ok"
    };
    // Un anti-triche l'emporte sur les réglages qui pourraient l'activer : le
    // lancement saute l'injecteur tant que le joueur ne l'a pas autorisé.
    let (launch_override, override_reason) = if game.settings.injections_blocked(protected) {
        ("blocked", "anticheat")
    } else {
        injector_override_state(game, record, frame, vkbasalt)
    };
    let action = if record.kind == "optiscaler" || override_reason == "anticheat" {
        "none"
    } else if install == "executable_changed" {
        "reinstall"
    } else if install != "ok" {
        "repair"
    } else if launch_override == "blocked"
        && matches!(override_reason, "wined3d" | "frame_generation" | "vkbasalt")
    {
        "fix_override"
    } else {
        "none"
    };
    GraphicsInjectorStatus {
        kind: record.kind.clone(),
        version: record.version.clone(),
        executable: record.executable.clone(),
        proxy: record.proxy.clone(),
        install: install.into(),
        files,
        launch_override: launch_override.into(),
        override_reason: override_reason.into(),
        action: action.into(),
        backups: backup_dirs(game, record),
    }
}

/// Ajoute un dossier de sauvegarde non vide et réellement présent.
fn collect_backup_dir(found: &mut Vec<(String, PathBuf)>, kind: &str, path: &Path) {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return;
    };
    if !meta.file_type().is_dir() {
        return;
    }
    let has_entry = std::fs::read_dir(path)
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(false);
    if !has_entry {
        return;
    }
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if found.iter().any(|(_, existing)| existing == &path) {
        return;
    }
    found.push((kind.to_string(), path));
}

/// Dossiers conservés par une réparation ou une réinstallation, dans le
/// dossier du jeu et dans celui de l'ancien exécutable. Seuls les dossiers
/// présents et non vides sont proposés : jamais un chemin qui n'existe pas.
fn backup_dirs(
    game: &Game,
    record: &GraphicsInjector,
) -> Vec<lapinux_core::configurations::GraphicsInjectorBackup> {
    let root = game.install_dir();
    let mut found = Vec::new();
    let mut folders = vec![root.clone()];
    if let Ok(exe) = checked(&root, &record.executable) {
        if let Some(parent) = exe.parent() {
            folders.push(parent.to_path_buf());
        }
    }
    for folder in &folders {
        collect_backup_dir(
            &mut found,
            "user_modified",
            &folder.join(format!(".lapinux-{}-user-modified", record.kind)),
        );
        let prefix = format!(".lapinux-{}-stage-ancienne", record.kind);
        collect_backup_dir(&mut found, "stage", &folder.join(&prefix));
        // Les copies conservées suivantes portent un suffixe numérique.
        if let Ok(entries) = std::fs::read_dir(folder) {
            for entry in entries.flatten() {
                if entry.file_name().to_string_lossy().starts_with(&prefix) {
                    collect_backup_dir(&mut found, "stage", &entry.path());
                }
            }
        }
    }
    if let Ok(stage) = checked(&root, &record.stage) {
        collect_backup_dir(&mut found, "user_modified", &stage.join("user-modified"));
    }
    found.sort_by(|(left_kind, left), (right_kind, right)| {
        let rank = |kind: &str| usize::from(kind != "user_modified");
        rank(left_kind)
            .cmp(&rank(right_kind))
            .then_with(|| left.cmp(right))
    });
    found
        .into_iter()
        .map(
            |(kind, path)| lapinux_core::configurations::GraphicsInjectorBackup {
                kind,
                path: path.to_string_lossy().into_owned(),
            },
        )
        .collect()
}

fn collision(dir: &Path, name: &str) -> std::result::Result<(), String> {
    for entry in std::fs::read_dir(dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry
            .file_name()
            .to_string_lossy()
            .eq_ignore_ascii_case(name)
        {
            return Err(format!("fichier ou injecteur existant conservé : {name}"));
        }
    }
    Ok(())
}
impl Shared {
    fn injector_records(&self) -> Result<Records> {
        Ok(self.store.get_json(RECORDS)?.unwrap_or_default())
    }
    pub fn graphics_injectors(&self, appid: u32) -> Result<Envelope> {
        Ok(Envelope::ok(
            &self.injector_records()?.remove(&appid).unwrap_or_default(),
        ))
    }
    fn check_injector_game(&self, appid: u32) -> Result<Game> {
        let game = self
            .store
            .game(appid)?
            .ok_or_else(|| error("jeu introuvable"))?;
        if !game.installed || game.native {
            return Err(error("injecteur réservé aux jeux Windows installés"));
        }
        if self.session_info().is_some_and(|s| s.appid == appid) {
            return Err(error(
                "ferme le jeu lancé par Lapinux avant cette opération",
            ));
        }
        if self.jobs.lock().map_err(error)?.iter().any(|j| {
            j.appid == Some(appid)
                && j.kind == JobKind::InstallGame
                && matches!(
                    j.state,
                    JobState::Queued | JobState::Running | JobState::Paused
                )
        }) {
            return Err(error("installation ou mise à jour du jeu en cours"));
        }
        Ok(game)
    }
    pub fn install_graphics_injector(
        &self,
        appid: u32,
        kind: &str,
        version: &str,
        executable: &str,
        proxy: &str,
    ) -> Result<Envelope> {
        let _guard = self.save_operation.try_lock().map_err(error)?;
        if kind == "optiscaler" {
            return Err(error("OptiScaler n'est plus proposé par Lapinux ; utilise afmf-linux"));
        }
        params(kind, version, proxy).map_err(error)?;
        let game = self.check_injector_game(appid)?;
        self.install_graphics_injector_inner(game, kind, version, executable, proxy, None)
    }

    fn install_graphics_injector_inner(
        &self,
        game: Game,
        kind: &str,
        version: &str,
        executable: &str,
        proxy: &str,
        supplied: Option<(&[u8], &str, Option<&str>, Option<&str>)>,
    ) -> Result<Envelope> {
        let appid = game.appid;
        params(kind, version, proxy).map_err(error)?;
        let root = game.install_dir();
        let exe = checked(&root, executable).map_err(error)?;
        if !std::fs::symlink_metadata(&exe)
            .map_err(error)?
            .file_type()
            .is_file()
            || !executable.to_ascii_lowercase().ends_with(".exe")
        {
            return Err(error(
                "sélectionne un exécutable Windows dans le dossier du jeu",
            ));
        }
        let machine = pe_arch(&exe).map_err(error)?;
        if kind == "optiscaler" && machine != 0x8664 {
            return Err(error("OptiScaler nécessite un exécutable 64 bits"));
        }
        if kind == "cnc-ddraw" && machine != 0x14c {
            return Err(error(
                "cnc-ddraw 7.1 nécessite un exécutable Windows 32 bits",
            ));
        }
        let actual_exe = game
            .exe
            .as_ref()
            .ok_or_else(|| error("choisis d'abord l'exécutable de lancement du jeu"))?;
        if exe != actual_exe.canonicalize().map_err(error)? {
            return Err(error(
                "cet exécutable doit être celui configuré pour le lancement du jeu",
            ));
        }
        if kind == "reshade" && supplied.is_none() {
            let prefix = lapinux_proton::prefix_path(&game, &self.paths.data_dir);
            let subdir = if machine == 0x8664 {
                "system32"
            } else {
                "syswow64"
            };
            let found = [
                prefix
                    .join("pfx/drive_c/windows")
                    .join(subdir)
                    .join("d3dcompiler_47.dll"),
                prefix
                    .join("drive_c/windows")
                    .join(subdir)
                    .join("d3dcompiler_47.dll"),
                exe.parent().unwrap().join("d3dcompiler_47.dll"),
            ]
            .iter()
            .any(|p| p.is_file());
            if !found {
                return Err(error("ReShade : d3dcompiler_47.dll introuvable dans le préfixe ou près du jeu ; installe cette dépendance avant de continuer"));
            }
        }
        let mut records = self.injector_records()?;
        if records.get(&appid).into_iter().flatten().next().is_some() {
            return Err(error("outil déjà suivi : retire-le avant de réinstaller"));
        }
        let folder = exe
            .parent()
            .ok_or_else(|| error("dossier de l'exécutable invalide"))?;
        collision(folder, proxy).map_err(error)?;
        let stage_name = format!(".lapinux-{kind}-v1");
        collision(folder, &stage_name).map_err(error)?;
        // Refuse aussi les injecteurs courants non suivis : impossible de déterminer leur chaînage.
        for name in [
            "dxgi.dll",
            "ddraw.dll",
            "d3d9.dll",
            "d3d11.dll",
            "d3d12.dll",
            "opengl32.dll",
            "dbghelp.dll",
            "winmm.dll",
            "version.dll",
            "wininet.dll",
            "winhttp.dll",
        ] {
            collision(folder, name).map_err(error)?;
        }
        if kind == "cnc-ddraw" {
            collision(folder, "ddraw.ini").map_err(error)?;
            let relative_folder = Path::new(executable).parent().unwrap_or(Path::new(""));
            let targets: Vec<String> = ["ddraw.dll", "ddraw.ini"]
                .iter()
                .map(|name| relative_folder.join(name).to_string_lossy().into_owned())
                .collect();
            if self.graphics_files_conflict_with_configurations(appid, &targets)? {
                return Err(error("fichier suivi par une configuration"));
            }
            let graphics = self.config_snapshot().graphics;
            cnc_conflicts(
                &game,
                game.settings.frame_generation.unwrap_or(graphics.frame_generation).supported(),
                game.settings.vkbasalt.unwrap_or(graphics.vkbasalt),
            )
            .map_err(error)?;
        }
        // Vérifier l'outil avant de télécharger ou créer un dossier temporaire.
        if kind == "optiscaler" && lapinux_core::paths::which("7z").is_none() {
            return Err(error("OptiScaler nécessite l'outil 7z installé localement"));
        }
        let bytes = if let Some((bytes, expected, _, _)) = supplied {
            if digest(bytes) != expected {
                return Err(error("archive graphique : SHA-256 incorrect"));
            }
            bytes.to_vec()
        } else if kind == "optiscaler" {
            download(OPTI_URL, OPTI_HASH, 60 * 1024 * 1024).map_err(error)?
        } else if kind == "cnc-ddraw" {
            download(CNC_URL, CNC_HASH, 4 * 1024 * 1024).map_err(error)?
        } else {
            download(RESHADE_URL, RESHADE_HASH, 8 * 1024 * 1024).map_err(error)?
        };
        let stage = folder.join(&stage_name);
        std::fs::create_dir(&stage).map_err(error)?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&stage, std::fs::Permissions::from_mode(0o700)).map_err(error)?;
        // Sans journal, une extraction interrompue ne touche aucune DLL du jeu ;
        // le dossier non suivi est conservé et bloque une nouvelle installation.
        extract(
            kind,
            machine,
            &bytes,
            &stage,
            supplied.and_then(|(_, _, hash, _)| hash),
            supplied.and_then(|(_, _, _, hash)| hash),
        )
        .map_err(|e| {
            error(format!(
                "{e} ; dossier temporaire conservé : {}",
                stage.display()
            ))
        })?;
        let staged = files_in(&stage).map_err(error)?;
        let mut sources = BTreeMap::<String, String>::new();
        if kind == "reshade" {
            sources.insert(
                proxy.into(),
                if machine == 0x8664 {
                    "ReShade64.dll"
                } else {
                    "ReShade32.dll"
                }
                .into(),
            );
        } else if kind == "cnc-ddraw" {
            sources.insert("ddraw.dll".into(), "ddraw.dll".into());
            sources.insert("ddraw.ini".into(), "ddraw.ini".into());
        } else {
            sources.insert(proxy.into(), "OptiScaler.dll".into());
            for file in &staged {
                if !file.name.contains('/')
                    && (file.name.ends_with(".dll") || file.name.ends_with(".ini"))
                    && file.name != "OptiScaler.dll"
                {
                    sources.insert(file.name.clone(), file.name.clone());
                }
            }
            // D3D12Core facultatif n'est pas posé automatiquement dans un dossier préexistant.
        }
        for name in sources.keys() {
            if sources
                .keys()
                .filter(|other| other.eq_ignore_ascii_case(name))
                .count()
                != 1
            {
                return Err(error("fichiers de destination ambigus"));
            }
            collision(folder, name).map_err(|e| {
                error(format!(
                    "{e} ; dossier temporaire conservé : {}",
                    stage.display()
                ))
            })?;
        }
        let relative_folder = Path::new(executable).parent().unwrap_or(Path::new(""));
        let targets: Vec<String> = sources
            .keys()
            .map(|name| relative_folder.join(name).to_string_lossy().into_owned())
            .collect();
        if self.graphics_files_conflict_with_configurations(appid, &targets)? {
            return Err(error(format!(
                "fichier suivi par une configuration ; dossier temporaire conservé : {}",
                stage.display()
            )));
        }
        let stage_rel = relative_folder
            .join(&stage_name)
            .to_str()
            .ok_or_else(|| error("chemin non UTF-8"))?
            .to_string();
        let files = sources
            .iter()
            .map(|(name, source)| {
                let hash = staged
                    .iter()
                    .find(|f| &f.name == source)
                    .ok_or_else(|| error("fichier extrait manquant"))?;
                Ok(GraphicsInjectorFile {
                    name: name.clone(),
                    sha256: hash.sha256.clone(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let record = GraphicsInjector {
            kind: kind.into(),
            version: version.into(),
            executable: executable.into(),
            proxy: proxy.into(),
            stage: stage_rel,
            files,
            staged,
            ready: false,
        };
        records.entry(appid).or_default().push(record.clone());
        self.store.set_json(RECORDS, &records)?;
        link_sources(&stage, folder, &sources).map_err(error)?;
        records
            .get_mut(&appid)
            .unwrap()
            .iter_mut()
            .find(|r| r.kind == kind)
            .unwrap()
            .ready = true;
        if let Err(e) = self.store.set_json(RECORDS, &records) {
            return Err(error(format!(
                "{e} ; installation suivie mais incomplète : utilise Retirer"
            )));
        }
        Ok(Envelope::ok(&record.kind))
    }
    pub fn remove_graphics_injector(&self, appid: u32, kind: &str) -> Result<Envelope> {
        let _guard = self.save_operation.try_lock().map_err(error)?;
        let game = self.check_injector_game(appid)?;
        let root = game.install_dir();
        let mut records = self.injector_records()?;
        let record = records
            .get(&appid)
            .and_then(|r| r.iter().find(|r| r.kind == kind))
            .cloned()
            .ok_or_else(|| error("injecteur non suivi"))?;
        let stage = checked(&root, &record.stage).map_err(error)?;
        let exe = checked(&root, &record.executable).map_err(error)?;
        let folder = exe.parent().ok_or_else(|| error("dossier invalide"))?;
        // Un retrait interrompu ne doit plus laisser l'installation marquée opérationnelle.
        records
            .get_mut(&appid)
            .unwrap()
            .iter_mut()
            .find(|r| r.kind == kind)
            .unwrap()
            .ready = false;
        self.store.set_json(RECORDS, &records)?;
        let mut warnings = Vec::new();
        for file in &record.files {
            safe_relative(&file.name).map_err(error)?;
            let target = folder.join(&file.name);
            match std::fs::symlink_metadata(&target) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => {
                    warnings.push(e.to_string());
                    continue;
                }
                Ok(_) => {}
            }
            let source_name = if file.name == record.proxy {
                if kind == "optiscaler" {
                    "OptiScaler.dll"
                } else if kind == "cnc-ddraw" {
                    "ddraw.dll"
                } else if record.staged.iter().any(|f| f.name == "ReShade64.dll") {
                    "ReShade64.dll"
                } else {
                    "ReShade32.dll"
                }
            } else {
                &file.name
            };
            let source = checked(&stage, source_name).map_err(error)?;
            if !same_file(&source, &target)
                || hash_file(&target).ok().as_deref() != Some(&file.sha256)
            {
                warnings.push(format!(
                    "{} modifié ou remplacé : conservé",
                    target.display()
                ));
                continue;
            }
            if let Err(e) = std::fs::remove_file(&target) {
                warnings.push(format!("{} : {e}", target.display()));
            }
        }
        if warnings.is_empty()
            && !stage.exists()
            && record
                .files
                .iter()
                .any(|f| std::fs::symlink_metadata(folder.join(&f.name)).is_ok())
        {
            warnings.push("dossier de suivi absent ; fichiers conservés".into());
        }
        if warnings.is_empty() && stage.exists() {
            // Ne jamais supprimer une donnée ajoutée ou modifiée dans le dossier de suivi.
            let current = files_in(&stage).map_err(error)?;
            if current.iter().any(|f| {
                !record
                    .staged
                    .iter()
                    .any(|s| s.name == f.name && s.sha256 == f.sha256)
            }) {
                warnings.push(format!("dossier de suivi modifié : {}", stage.display()));
            } else {
                for file in &current {
                    let path = checked(&stage, &file.name).map_err(error)?;
                    if let Err(e) = std::fs::remove_file(path) {
                        warnings.push(e.to_string());
                        break;
                    }
                }
                if warnings.is_empty() {
                    fn remove_empty(p: &Path) -> std::io::Result<()> {
                        for entry in std::fs::read_dir(p)? {
                            let e = entry?;
                            if e.file_type()?.is_dir() {
                                remove_empty(&e.path())?;
                            }
                        }
                        std::fs::remove_dir(p)
                    }
                    if let Err(e) = remove_empty(&stage) {
                        warnings.push(e.to_string());
                    }
                }
            }
        }
        if warnings.is_empty() {
            records.get_mut(&appid).unwrap().retain(|r| r.kind != kind);
            self.store.set_json(RECORDS, &records)?;
        }
        Ok(Envelope::ok(&warnings))
    }

    pub fn graphics_injector_statuses(&self, appid: u32) -> Result<Envelope> {
        let Some(game) = self.store.game(appid)? else {
            return Ok(Envelope::ok(&Vec::<GraphicsInjectorStatus>::new()));
        };
        let config = self.config_snapshot();
        let frame = effective_frame_generation(&game, &config);
        let vkbasalt = game.settings.vkbasalt.unwrap_or(config.graphics.vkbasalt);
        let records = self.injector_records()?;
        let statuses: Vec<GraphicsInjectorStatus> = records
            .get(&appid)
            .map(|list| {
                let protected = self.anticheat_protected(&game);
                list.iter()
                    .map(|record| injector_status(&game, record, frame, vkbasalt, protected))
                    .collect()
            })
            .unwrap_or_default();
        Ok(Envelope::ok(&statuses))
    }

    pub fn repair_graphics_injector(&self, appid: u32, kind: &str) -> Result<Envelope> {
        let _guard = self.save_operation.try_lock().map_err(error)?;
        if kind == "optiscaler" {
            return Err(error("OptiScaler n'est plus pris en charge : utilise Retirer"));
        }
        let game = self.check_injector_game(appid)?;
        self.repair_graphics_injector_inner(game, kind, None)
    }

    fn repair_graphics_injector_inner(
        &self,
        game: Game,
        kind: &str,
        supplied: Option<(&[u8], &str, Option<&str>, Option<&str>)>,
    ) -> Result<Envelope> {
        let root = game.install_dir();
        let mut records = self.injector_records()?;
        let mut record = records
            .get(&game.appid)
            .and_then(|list| list.iter().find(|record| record.kind == kind))
            .cloned()
            .ok_or_else(|| error("injecteur non suivi"))?;
        // La réparation vise l'installation enregistrée, même si le lanceur a
        // changé : ses fichiers restent réparables avant une réinstallation au
        // nouveau chemin. Le diagnostic continue d'annoncer « exécutable
        // changé » tant que la réinstallation n'a pas eu lieu.
        let recorded_exe = checked(&root, &record.executable).map_err(|_| {
            error("exécutable enregistré introuvable : remets l'ancien exécutable ou retire l'injecteur")
        })?;
        let exe = recorded_exe.canonicalize().map_err(error)?;
        let folder = exe
            .parent()
            .ok_or_else(|| error("dossier invalide"))?
            .to_path_buf();
        let stage = checked(&root, &record.stage).map_err(error)?;
        let stage_ok = staged_files_ok(&record, &stage, Some(&folder), true);
        if !stage_ok {
            let machine = pe_arch(&exe).map_err(error)?;
            let (bytes, dll_hash, ini_hash) = if let Some((bytes, expected, dll, ini)) = supplied {
                if digest(bytes) != expected {
                    return Err(error("archive graphique : SHA-256 incorrect"));
                }
                (bytes.to_vec(), dll, ini)
            } else {
                let (url, hash, limit) = match kind {
                    "cnc-ddraw" => (CNC_URL, CNC_HASH, 4 * 1024 * 1024),
                    "optiscaler" => (OPTI_URL, OPTI_HASH, 60 * 1024 * 1024),
                    _ => (RESHADE_URL, RESHADE_HASH, 8 * 1024 * 1024),
                };
                (download(url, hash, limit).map_err(error)?, None, None)
            };
            let parent = stage.parent().unwrap_or(&folder).to_path_buf();
            let temp = parent.join(format!(".lapinux-{kind}-repair"));
            std::fs::create_dir(&temp).map_err(|e| {
                error(format!(
                    "préparation de la réparation impossible ({e}) : examine {}",
                    temp.display()
                ))
            })?;
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o700))
                .map_err(error)?;
            let extracted = extract(kind, machine, &bytes, &temp, dll_hash, ini_hash)
                .and_then(|()| files_in(&temp));
            let fresh = match extracted {
                Ok(fresh) => fresh,
                Err(e) => {
                    return Err(error(format!(
                        "{e} ; dossier de réparation conservé : {}",
                        temp.display()
                    )))
                }
            };
            let same_archive = record.staged.iter().all(|expected| {
                fresh
                    .iter()
                    .any(|file| file.name == expected.name && file.sha256 == expected.sha256)
            }) && fresh.iter().all(|file| {
                record
                    .staged
                    .iter()
                    .any(|expected| file.name == expected.name)
            });
            if !same_archive {
                return Err(error(format!(
                    "archive de réparation différente de l'installation : réinstalle ; dossier conservé : {}",
                    temp.display()
                )));
            }
            std::fs::create_dir_all(&stage).map_err(error)?;
            // Renommer remplace l'entrée de dossier : un lien physique déjà
            // posé n'est jamais réécrit au passage.
            for file in &fresh {
                std::fs::rename(temp.join(&file.name), stage.join(&file.name)).map_err(error)?;
            }
            std::fs::remove_dir_all(&temp).map_err(error)?;
            record.staged = fresh;
            record.ready = false;
        }
        let mut warnings = Vec::new();
        let mut complete = true;
        for file in &record.files {
            safe_relative(&file.name).map_err(error)?;
            let target = folder.join(&file.name);
            // Un ddraw.ini personnalisé n'a pas besoin du contenu officiel :
            // le restaurer perdrait les réglages du wrapper.
            let personalized = record.kind == "cnc-ddraw" && file.name == "ddraw.ini" && {
                match std::fs::symlink_metadata(&target) {
                    Ok(meta) if meta.file_type().is_file() => {
                        hash_file(&target).ok().as_deref() != Some(file.sha256.as_str())
                    }
                    _ => false,
                }
            };
            if personalized {
                continue;
            }
            let source = checked(&stage, stage_source_name(&record, file)).map_err(error)?;
            if hash_file(&source).ok().as_deref() != Some(file.sha256.as_str()) {
                complete = false;
                warnings.push(format!("copie de suivi inattendue : {}", file.name));
                continue;
            }
            match std::fs::symlink_metadata(&target) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    if let Err(e) = std::fs::hard_link(&source, &target) {
                        complete = false;
                        warnings.push(format!("{} : {e}", target.display()));
                    }
                }
                Err(e) => {
                    complete = false;
                    warnings.push(format!("{} : {e}", target.display()));
                }
                Ok(meta) => {
                    let hash = meta
                        .file_type()
                        .is_file()
                        .then(|| hash_file(&target).ok())
                        .flatten();
                    if hash.as_deref() == Some(file.sha256.as_str()) {
                        if same_file(&source, &target) {
                            continue;
                        }
                        // Contenu identique mais copie séparée : relier est sans perte.
                        if let Err(e) = std::fs::remove_file(&target)
                            .and_then(|()| std::fs::hard_link(&source, &target))
                        {
                            complete = false;
                            warnings.push(format!("{} : {e}", target.display()));
                        }
                    } else {
                        let backup = stage.join("user-modified");
                        if let Err(e) = std::fs::create_dir_all(&backup) {
                            complete = false;
                            warnings.push(format!("{} : {e}", backup.display()));
                            continue;
                        }
                        let mut destination = backup.join(&file.name);
                        let mut index = 1;
                        while destination.exists() {
                            destination = backup.join(format!("{}.{index}", file.name));
                            index += 1;
                        }
                        match std::fs::rename(&target, &destination) {
                            Ok(()) => match std::fs::hard_link(&source, &target) {
                                Ok(()) => warnings.push(format!(
                                    "{} modifié : copie conservée dans {}",
                                    target.display(),
                                    destination.display()
                                )),
                                Err(e) => {
                                    let _ = std::fs::rename(&destination, &target);
                                    complete = false;
                                    warnings.push(format!("{} : {e}", target.display()));
                                }
                            },
                            Err(e) => {
                                complete = false;
                                warnings.push(format!("{} : {e}", target.display()));
                            }
                        }
                    }
                }
            }
        }
        record.ready = complete;
        if let Some(list) = records.get_mut(&game.appid) {
            if let Some(slot) = list.iter_mut().find(|record| record.kind == kind) {
                *slot = record;
            }
        }
        self.store.set_json(RECORDS, &records)?;
        Ok(Envelope::ok(&warnings))
    }

    pub fn reinstall_graphics_injector(&self, appid: u32, kind: &str) -> Result<Envelope> {
        if kind == "optiscaler" {
            return Err(error("OptiScaler n'est plus pris en charge : utilise Retirer"));
        }
        self.reinstall_graphics_injector_inner(appid, kind, None)
    }

    fn reinstall_graphics_injector_inner(
        &self,
        appid: u32,
        kind: &str,
        supplied: Option<(&[u8], &str, Option<&str>, Option<&str>)>,
    ) -> Result<Envelope> {
        let game = self.check_injector_game(appid)?;
        let record = self
            .injector_records()?
            .get(&appid)
            .and_then(|list| list.iter().find(|record| record.kind == kind))
            .cloned()
            .ok_or_else(|| error("injecteur non suivi"))?;
        let executable = game
            .exe
            .as_ref()
            .and_then(|exe| exe.strip_prefix(game.install_dir()).ok())
            .map(|path| path.to_string_lossy().into_owned())
            .ok_or_else(|| error("exécutable de lancement introuvable"))?;
        // Le dossier enregistré reste nettoyable même si l'ancien exécutable a
        // été supprimé : seul son chemin relatif permet de retrouver les
        // fichiers posés par Lapinux. La copie de suivi, elle, est reconstruite
        // plus bas depuis l'archive vérifiée : elle n'a pas besoin d'être saine.
        let root = game.install_dir();
        let old_folder = {
            let relative = safe_relative(&record.executable).map_err(error)?;
            let parent = relative.parent().unwrap_or(Path::new(""));
            if parent.as_os_str().is_empty() {
                root.canonicalize().map_err(error)?
            } else {
                checked(&root, &parent.to_string_lossy()).map_err(error)?
            }
        };
        let stage = checked(&root, &record.stage).map_err(error)?;
        let new_folder = game
            .install_dir()
            .join(&executable)
            .parent()
            .ok_or_else(|| error("dossier invalide"))?
            .to_path_buf();
        // Le ddraw.ini personnalisé est mis de côté puis remis en place après
        // l'installation : l'archive ne doit jamais remplacer les réglages du
        // joueur. Les DLL modifiées sont sauvegardées de la même manière.
        let ini_path = old_folder.join("ddraw.ini");
        let official_ini = record
            .files
            .iter()
            .find(|file| file.name == "ddraw.ini")
            .map(|file| file.sha256.as_str());
        let mut ini_aside = None;
        if kind == "cnc-ddraw" {
            if let Ok(meta) = std::fs::symlink_metadata(&ini_path) {
                if meta.file_type().is_file()
                    && hash_file(&ini_path).ok().as_deref() != official_ini
                {
                    let aside = old_folder.join(format!(".lapinux-{kind}-ddraw.ini.pending"));
                    let _ = std::fs::remove_file(&aside);
                    std::fs::rename(&ini_path, &aside).map_err(error)?;
                    ini_aside = Some(aside);
                }
            }
        }
        let backup_dir = root.join(format!(".lapinux-{kind}-user-modified"));
        let mut backups: Vec<PathBuf> = Vec::new();
        for file in &record.files {
            if ini_aside.is_some() && file.name == "ddraw.ini" {
                continue;
            }
            let target = old_folder.join(&file.name);
            let Ok(meta) = std::fs::symlink_metadata(&target) else {
                continue;
            };
            if !meta.file_type().is_file() {
                restore_ini(ini_aside.as_deref(), &old_folder);
                return Err(error(format!(
                    "{} n'est pas un fichier ordinaire : retire-le à la main avant de réinstaller",
                    target.display()
                )));
            }
            if hash_file(&target).ok().as_deref() != Some(file.sha256.as_str()) {
                match move_to_backup(&target, &backup_dir) {
                    Ok(dest) => backups.push(dest),
                    Err(e) => {
                        restore_ini(ini_aside.as_deref(), &old_folder);
                        return Err(error(format!("{} : {e}", target.display())));
                    }
                }
            }
        }
        // Une copie de suivi saine peut être supprimée ; toute copie modifiée
        // ou incomplète est conservée entière sous un autre nom, jamais
        // effacée à l'aveugle. L'installation repart ensuite de l'archive.
        let mut preserved_stage = None;
        if stage.exists() {
            let clean = files_in(&stage)
                .map(|files| {
                    files.len() == record.staged.len()
                        && record.staged.iter().all(|expected| {
                            files.iter().any(|file| {
                                file.name == expected.name && file.sha256 == expected.sha256
                            })
                        })
                })
                .unwrap_or(false);
            if clean {
                if let Err(e) = std::fs::remove_dir_all(&stage) {
                    restore_ini(ini_aside.as_deref(), &old_folder);
                    return Err(error(format!(
                        "copie de suivi non supprimée ({e}) : {}",
                        stage.display()
                    )));
                }
            } else {
                let mut candidate = old_folder.join(format!(".lapinux-{kind}-stage-ancienne"));
                let mut index = 1;
                while candidate.exists() {
                    candidate = old_folder.join(format!(".lapinux-{kind}-stage-ancienne.{index}"));
                    index += 1;
                }
                if let Err(e) = std::fs::rename(&stage, &candidate) {
                    restore_ini(ini_aside.as_deref(), &old_folder);
                    return Err(error(format!(
                        "copie de suivi non conservée ({e}) : {}",
                        stage.display()
                    )));
                }
                preserved_stage = Some(candidate);
            }
        }
        // Les fichiers modifiés ont déjà été sauvegardés et l'INI personnalisé
        // mis de côté : ne reste que les fichiers officiels à retirer.
        for file in &record.files {
            let target = old_folder.join(&file.name);
            let Ok(meta) = std::fs::symlink_metadata(&target) else {
                continue;
            };
            if !meta.file_type().is_file() {
                restore_ini(ini_aside.as_deref(), &old_folder);
                return Err(error(format!(
                    "{} n'est pas un fichier ordinaire : retire-le à la main avant de réinstaller",
                    target.display()
                )));
            }
            if hash_file(&target).ok().as_deref() != Some(file.sha256.as_str()) {
                // Fichier modifié entre le préflight et maintenant : sauvegarde.
                match move_to_backup(&target, &backup_dir) {
                    Ok(dest) => backups.push(dest),
                    Err(e) => {
                        restore_ini(ini_aside.as_deref(), &old_folder);
                        return Err(error(format!("{} : {e}", target.display())));
                    }
                }
                continue;
            }
            if let Err(e) = std::fs::remove_file(&target) {
                restore_ini(ini_aside.as_deref(), &old_folder);
                return Err(error(format!("{} : {e}", target.display())));
            }
        }
        let mut records = self.injector_records()?;
        if let Some(list) = records.get_mut(&appid) {
            list.retain(|record| record.kind != kind);
            if list.is_empty() {
                records.remove(&appid);
            }
        }
        if let Err(e) = self.store.set_json(RECORDS, &records) {
            restore_ini(ini_aside.as_deref(), &old_folder);
            return Err(error(format!(
                "suivi de l'injecteur non mis à jour ({e}) : {}",
                stage.display()
            )));
        }
        if let Err(e) = self.install_graphics_injector_inner(
            game,
            kind,
            &record.version,
            &executable,
            &record.proxy,
            supplied,
        ) {
            restore_ini(ini_aside.as_deref(), &new_folder);
            let mut message = format!("injecteur retiré, installation échouée : {e}");
            if !backups.is_empty() {
                message.push_str(&format!(
                    " ; fichiers modifiés conservés dans {}",
                    backup_dir.display()
                ));
            }
            if let Some(stage) = &preserved_stage {
                message.push_str(&format!(
                    " ; ancienne copie de suivi conservée dans {}",
                    stage.display()
                ));
            }
            return Err(error(message));
        }
        restore_ini(ini_aside.as_deref(), &new_folder);
        let mut warnings = Vec::new();
        if ini_aside.is_some() {
            warnings.push("ddraw.ini personnalisé conservé".to_string());
        }
        if !backups.is_empty() {
            warnings.push(format!(
                "fichiers modifiés sauvegardés dans {}",
                backup_dir.display()
            ));
        }
        if let Some(stage) = &preserved_stage {
            warnings.push(format!(
                "ancienne copie de suivi conservée dans {}",
                stage.display()
            ));
        }
        Ok(Envelope::ok(&warnings))
    }

    pub fn fix_graphics_injector_override(&self, appid: u32, kind: &str) -> Result<Envelope> {
        let _guard = self.save_operation.try_lock().map_err(error)?;
        if kind == "optiscaler" {
            return Err(error("OptiScaler n'est plus pris en charge : retire l'injecteur suivi sans modifier les réglages"));
        }
        let mut game = self.check_injector_game(appid)?;
        let record = self
            .injector_records()?
            .get(&appid)
            .and_then(|list| list.iter().find(|record| record.kind == kind))
            .cloned()
            .ok_or_else(|| error("injecteur non suivi"))?;
        let config = self.config_snapshot();
        if game.settings.compatibility.wined3d {
            game.settings.compatibility.wined3d = false;
        }
        use lapinux_core::config::FrameGeneration;
        if !matches!(
            effective_frame_generation(&game, &config),
            FrameGeneration::Off | FrameGeneration::LsfgVk
        ) {
            game.settings.frame_generation = Some(FrameGeneration::Off);
        }
        if game.settings.vkbasalt.unwrap_or(config.graphics.vkbasalt) {
            game.settings.vkbasalt = Some(false);
        }
        self.store.save_game(&game)?;
        let (state, reason) = injector_override_state(
            &game,
            &record,
            effective_frame_generation(&game, &config),
            game.settings.vkbasalt.unwrap_or(config.graphics.vkbasalt),
        );
        if state == "blocked" {
            let detail = match reason {
                "wined3d" => "WineD3D reste actif (réglage du jeu)",
                "wined3d_env" => {
                    "WineD3D vient d'une variable du jeu (PROTON_USE_WINED3D ou STEAM_COMPAT_CONFIG) : retire-la"
                }
                "frame_generation" => "la génération d'images reste active (réglage du jeu)",
                "frame_generation_env" => {
                    "la génération d'images vient d'une variable du jeu (AFMF_ENABLE ou NVPRESENT_ENABLE_SMOOTH_MOTION) : retire-la"
                }
                "vkbasalt" => "vkBasalt reste actif (réglage du jeu)",
                "vkbasalt_env" => {
                    "vkBasalt vient de la variable ENABLE_VKBASALT : retire-la des variables du jeu"
                }
                "config_file" => {
                    "CNC_DDRAW_CONFIG_FILE est défini dans les variables du jeu : retire cette variable"
                }
                "manual_override" => {
                    "WINEDLLOVERRIDES contient une règle pour ce proxy : retire-la des variables du jeu"
                }
                "optiscaler_proton" => {
                    "PROTON_USE_OPTISCALER est actif : désactive-le pour ce jeu"
                }
                _ => "un conflit inconnu subsiste",
            };
            return Err(error(format!("override toujours bloqué : {detail}")));
        }
        Ok(Envelope::ok(&Vec::<String>::new()))
    }

    pub(crate) fn launch_injector_override(
        &self,
        game: &Game,
        exe: &Path,
        frame: lapinux_core::config::FrameGeneration,
    ) -> Result<Option<String>> {
        let records = self.injector_records()?;
        let installed = records.get(&game.appid).map(Vec::as_slice).unwrap_or(&[]);
        if installed.is_empty() {
            return Ok(None);
        }
        // Jeu protégé par un anti-triche : aucun override, donc l'injecteur
        // n'est pas chargé. Une installation incomplète ne bloque pas non
        // plus le lancement : le jeu doit démarrer proprement.
        if game.settings.injections_blocked(self.anticheat_protected(game)) {
            tracing::info!(
                "injecteur {} du jeu {} ignoré : anti-triche détecté",
                installed[0].kind,
                game.appid
            );
            return Ok(None);
        }
        if installed.len() != 1 || !installed[0].ready {
            return Err(error("installation d'injecteur incomplète ou en conflit : retirer ou réparer avant lancement"));
        }
        let r = &installed[0];
        let chosen = checked(&game.install_dir(), &r.executable).map_err(error)?;
        if chosen != exe.canonicalize().map_err(error)? {
            return Err(error(
                "l'exécutable sélectionné pour l'injecteur diffère de celui lancé par Lapinux",
            ));
        }
        let folder = chosen.parent().ok_or_else(|| error("dossier invalide"))?;
        let stage = checked(&game.install_dir(), &r.stage).map_err(error)?;
        for f in &r.files {
            let target = folder.join(safe_relative(&f.name).map_err(error)?);
            let source = if f.name == r.proxy {
                if r.kind == "optiscaler" {
                    "OptiScaler.dll"
                } else if r.kind == "cnc-ddraw" {
                    "ddraw.dll"
                } else if r.staged.iter().any(|f| f.name == "ReShade64.dll") {
                    "ReShade64.dll"
                } else {
                    "ReShade32.dll"
                }
            } else {
                &f.name
            };
            if !same_file(&target, &checked(&stage, source).map_err(error)?)
                || hash_file(&target).ok().as_deref() != Some(f.sha256.as_str())
            {
                // Les fichiers INI sont modifiables par l'utilisateur ; ils ne sont pas exécutables.
                let rewritten_cnc_ini =
                    r.kind == "cnc-ddraw" && f.name == "ddraw.ini" && hash_file(&target).is_ok();
                if !rewritten_cnc_ini
                    && (!f.name.ends_with(".ini")
                        || !same_file(&target, &checked(&stage, source).map_err(error)?))
                {
                    return Err(error(format!(
                        "injecteur : fichier absent ou modifié : {}",
                        f.name
                    )));
                }
            }
        }
        use lapinux_core::config::FrameGeneration::*;
        if r.kind == "optiscaler" {
            return Err(error(
                "OptiScaler n'est plus pris en charge : retire l'injecteur suivi avant de lancer ce jeu",
            ));
        }
        if r.kind == "cnc-ddraw" {
            cnc_conflicts(
                game,
                frame,
                game.settings
                    .vkbasalt
                    .unwrap_or(self.config_snapshot().graphics.vkbasalt),
            )
            .map_err(error)?;
        }
        if r.kind == "reshade"
            && (matches!(frame, OptiScaler)
                || game
                    .settings
                    .vkbasalt
                    .unwrap_or(self.config_snapshot().graphics.vkbasalt))
        {
            return Err(error(
                "ReShade ne doit pas être combiné automatiquement avec OptiScaler Proton ou vkBasalt",
            ));
        }
        let manual = game
            .settings
            .env
            .iter()
            .rev()
            .find(|(k, _)| k.eq_ignore_ascii_case("WINEDLLOVERRIDES"))
            .map(|(_, v)| v.clone())
            .or_else(|| std::env::var("WINEDLLOVERRIDES").ok())
            .unwrap_or_default();
        let dll = r.proxy.trim_end_matches(".dll");
        let mut already_native = false;
        for rule in manual.split(';') {
            let (names, actions) = rule.split_once('=').unwrap_or((rule, ""));
            if names.split(',').any(|n| {
                let n = n.trim().to_ascii_lowercase();
                n.strip_suffix(".dll")
                    .unwrap_or(&n)
                    .eq_ignore_ascii_case(dll)
            }) {
                if actions.split(',').any(|action| {
                    let action = action.trim();
                    action.eq_ignore_ascii_case("n") || action.eq_ignore_ascii_case("native")
                }) {
                    already_native = true;
                } else {
                    return Err(error(format!(
                        "WINEDLLOVERRIDES force {dll} en builtin : retire cette règle des variables du jeu"
                    )));
                }
            }
        }
        if already_native {
            // La règle du joueur charge déjà la DLL native : ne rien dupliquer.
            return Ok(Some(manual));
        }
        let proton_opti = game
            .settings
            .env
            .iter()
            .rev()
            .find(|(k, _)| k.eq_ignore_ascii_case("PROTON_USE_OPTISCALER"))
            .map(|(_, v)| v.clone())
            .or_else(|| std::env::var("PROTON_USE_OPTISCALER").ok());
        if proton_opti.is_some_and(|v| !v.is_empty() && v != "0") {
            return Err(error(
                "PROTON_USE_OPTISCALER entre en conflit avec l'injecteur autonome",
            ));
        }
        Ok(Some(if manual.is_empty() {
            format!("{dll}=n,b")
        } else {
            format!("{manual};{dll}=n,b")
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lapinux_core::config::FrameGeneration;
    use lapinux_core::paths::LapinuxPaths;
    use std::io::Write;

    fn setup() -> (tempfile::TempDir, Shared, Game) {
        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        paths.ensure().unwrap();
        let root = dir.path().join("library/steamapps/common/Test");
        std::fs::create_dir_all(&root).unwrap();
        let exe = root.join("game.exe");
        let mut pe = vec![0; 134];
        pe[..2].copy_from_slice(b"MZ");
        pe[60..64].copy_from_slice(&128u32.to_le_bytes());
        pe[128..132].copy_from_slice(b"PE\0\0");
        pe[132..134].copy_from_slice(&0x8664u16.to_le_bytes());
        std::fs::write(&exe, pe).unwrap();
        let game = Game {
            appid: 42,
            installdir: "Test".into(),
            library: dir.path().join("library"),
            installed: true,
            exe: Some(exe),
            ..Default::default()
        };
        let shared = Shared::new(&paths).unwrap();
        shared.store.save_game(&game).unwrap();
        (dir, shared, game)
    }
    fn archive() -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        writer
            .start_file("ReShade64.dll", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"fake dll for tests").unwrap();
        writer
            .start_file("ReShade32.dll", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"fake dll for tests").unwrap();
        writer.finish().unwrap().into_inner()
    }
    fn install(shared: &Shared, game: &Game) -> Result<Envelope> {
        let bytes = archive();
        shared.install_graphics_injector_inner(
            game.clone(),
            "reshade",
            "6.8.0",
            "game.exe",
            "dxgi.dll",
            Some((
                &bytes,
                &digest(&bytes),
                Some(&digest(b"fake dll for tests")),
                None,
            )),
        )
    }
    fn warnings(shared: &Shared) -> Vec<String> {
        shared
            .remove_graphics_injector(42, "reshade")
            .unwrap()
            .into_data()
            .unwrap()
    }

    #[test]
    fn removed_optiscaler_is_never_repaired_or_enabled() {
        let (_dir, shared, mut game) = setup();
        install(&shared, &game).unwrap();
        let mut records = shared.injector_records().unwrap();
        records.get_mut(&42).unwrap()[0].kind = "optiscaler".into();
        let record = &records[&42][0];
        let status = injector_status(&game, record, FrameGeneration::Off, false, false);
        assert_eq!(status.launch_override, "blocked");
        assert_eq!(status.action, "none");
        shared.store.set_json(RECORDS, &records).unwrap();
        game.settings.compatibility.wined3d = true;
        game.settings.frame_generation = Some(FrameGeneration::AfmfLinux);
        shared.store.save_game(&game).unwrap();
        assert!(shared.fix_graphics_injector_override(42, "optiscaler").is_err());
        assert!(shared.repair_graphics_injector(42, "optiscaler").is_err());
        assert!(shared.reinstall_graphics_injector(42, "optiscaler").is_err());
        let saved = shared.store.game(42).unwrap().unwrap();
        assert!(saved.settings.compatibility.wined3d);
        assert_eq!(saved.settings.frame_generation, Some(FrameGeneration::AfmfLinux));
        assert!(shared.launch_injector_override(&saved, saved.exe.as_ref().unwrap(), FrameGeneration::Off).is_err());
        assert!(game.install_dir().join("dxgi.dll").exists());
    }

    #[test]
    fn un_jeu_protege_ignore_injecteurs_et_dll_dlss_sauf_accord_explicite() {
        let (_dir, shared, game) = setup();
        install(&shared, &game).unwrap();
        // L'anti-triche est constaté après l'installation, avant le lancement.
        std::fs::create_dir_all(game.install_dir().join("EasyAntiCheat")).unwrap();
        let saved = shared.store.game(42).unwrap().unwrap();
        assert!(shared.anticheat_protected(&saved));

        // Aucun override : la DLL proxy de l'injecteur n'est pas chargée.
        assert_eq!(
            shared
                .launch_injector_override(
                    &saved,
                    saved.exe.as_ref().unwrap(),
                    FrameGeneration::Off
                )
                .unwrap(),
            None
        );
        let records = shared.injector_records().unwrap();
        let status = injector_status(
            &saved,
            &records[&42][0],
            FrameGeneration::Off,
            false,
            true,
        );
        assert_eq!(status.launch_override, "blocked");
        assert_eq!(status.override_reason, "anticheat");
        assert_eq!(status.action, "none");

        // Remplacer les DLL DLSS est refusé tant que rien n'est autorisé.
        assert!(shared.dlss_update(42).is_err());

        // Accord explicite du joueur : l'override revient, les DLL aussi.
        let mut allowed = saved.clone();
        allowed.settings.allow_protected_injections = Some(true);
        shared.store.save_game(&allowed).unwrap();
        let allowed = shared.store.game(42).unwrap().unwrap();
        assert_eq!(
            shared
                .launch_injector_override(
                    &allowed,
                    allowed.exe.as_ref().unwrap(),
                    FrameGeneration::Off
                )
                .unwrap(),
            Some("dxgi=n,b".into())
        );
        assert!(shared.dlss_update(42).is_ok());
    }

    #[test]
    fn injectors_install_launch_remove_and_preserve_manual_overrides() {
        let (_dir, shared, mut game) = setup();
        let root = game.install_dir();
        game.settings
            .env
            .push(("WINEDLLOVERRIDES".into(), "foo=n,b".into()));
        shared.store.save_game(&game).unwrap();
        install(&shared, &game).unwrap();
        assert_eq!(
            shared
                .launch_injector_override(&game, game.exe.as_ref().unwrap(), FrameGeneration::Off)
                .unwrap()
                .as_deref(),
            Some("foo=n,b;dxgi=n,b")
        );
        assert!(shared
            .launch_injector_override(
                &game,
                game.exe.as_ref().unwrap(),
                FrameGeneration::OptiScaler
            )
            .is_err());
        game.settings
            .env
            .push(("WINEDLLOVERRIDES".into(), "dxgi=b".into()));
        assert!(shared
            .launch_injector_override(&game, game.exe.as_ref().unwrap(), FrameGeneration::Off)
            .is_err());
        assert!(warnings(&shared).is_empty());
        assert!(!root.join("dxgi.dll").exists());
        assert!(!root.join(".lapinux-reshade-v1").exists());
        assert!(shared.injector_records().unwrap()[&42].is_empty());
    }
    #[test]
    fn injectors_existing_modified_and_unrelated_files_are_preserved() {
        let (_dir, shared, game) = setup();
        let root = game.install_dir();
        std::fs::write(root.join("DXGI.DLL"), b"personal").unwrap();
        assert!(install(&shared, &game).is_err());
        assert_eq!(std::fs::read(root.join("DXGI.DLL")).unwrap(), b"personal");
        std::fs::remove_file(root.join("DXGI.DLL")).unwrap();
        install(&shared, &game).unwrap();
        std::fs::write(root.join("dxgi.dll"), b"edited").unwrap();
        assert!(!warnings(&shared).is_empty());
        assert_eq!(std::fs::read(root.join("dxgi.dll")).unwrap(), b"edited");
        assert_eq!(shared.injector_records().unwrap()[&42].len(), 1);
        std::fs::write(root.join("dxgi.dll"), b"fake dll for tests").unwrap();
        let stage = root.join(".lapinux-reshade-v1");
        std::fs::write(stage.join("unrelated"), b"keep").unwrap();
        assert!(!warnings(&shared).is_empty());
        assert!(stage.join("unrelated").exists());
        std::fs::remove_file(stage.join("unrelated")).unwrap();
        assert!(warnings(&shared).is_empty());
    }
    #[test]
    fn injectors_recover_partial_operations_and_reject_bad_archives() {
        let (_dir, shared, game) = setup();
        let root = game.install_dir();
        let bytes = archive();
        assert!(shared
            .install_graphics_injector_inner(
                game.clone(),
                "reshade",
                "6.8.0",
                "game.exe",
                "dxgi.dll",
                Some((&bytes, "bad", None, None))
            )
            .is_err());
        assert!(!root.join("dxgi.dll").exists());
        install(&shared, &game).unwrap();
        let mut records = shared.injector_records().unwrap();
        records.get_mut(&42).unwrap()[0].ready = false;
        shared.store.set_json(RECORDS, &records).unwrap();
        assert!(shared
            .launch_injector_override(&game, game.exe.as_ref().unwrap(), FrameGeneration::Off)
            .is_err());
        std::fs::remove_file(root.join("dxgi.dll")).unwrap();
        assert!(warnings(&shared).is_empty());
        assert!(!root.join(".lapinux-reshade-v1").exists());
        assert!(checked(&root, "../game.exe").is_err());
        std::os::unix::fs::symlink("game.exe", root.join("alias.exe")).unwrap();
        assert!(checked(&root, "alias.exe").is_err());
    }
    #[test]
    fn injectors_partial_link_failure_preserves_unowned_file_and_journal() {
        let (_dir, shared, game) = setup();
        let root = game.install_dir();
        let stage = root.join(".lapinux-optiscaler-v1");
        std::fs::create_dir(&stage).unwrap();
        std::fs::write(stage.join("a.dll"), b"a").unwrap();
        std::fs::write(stage.join("b.dll"), b"b").unwrap();
        let staged = files_in(&stage).unwrap();
        let record = GraphicsInjector {
            kind: "optiscaler".into(),
            version: "0.9.4".into(),
            executable: "game.exe".into(),
            proxy: "dxgi.dll".into(),
            stage: ".lapinux-optiscaler-v1".into(),
            files: staged.clone(),
            staged,
            ready: false,
        };
        let mut records = Records::new();
        records.insert(42, vec![record]);
        shared.store.set_json(RECORDS, &records).unwrap();
        std::fs::write(root.join("b.dll"), b"user").unwrap();
        let sources = BTreeMap::from([
            ("a.dll".into(), "a.dll".into()),
            ("b.dll".into(), "b.dll".into()),
        ]);
        assert!(link_sources(&stage, &root, &sources).is_err());
        assert!(root.join("a.dll").exists());
        let issues: Vec<String> = shared
            .remove_graphics_injector(42, "optiscaler")
            .unwrap()
            .into_data()
            .unwrap();
        assert!(!issues.is_empty());
        assert!(!root.join("a.dll").exists());
        assert_eq!(std::fs::read(root.join("b.dll")).unwrap(), b"user");
        assert_eq!(shared.injector_records().unwrap()[&42].len(), 1);
        std::fs::remove_file(root.join("b.dll")).unwrap();
        let issues: Vec<String> = shared
            .remove_graphics_injector(42, "optiscaler")
            .unwrap()
            .into_data()
            .unwrap();
        assert!(issues.is_empty());
        assert!(!stage.exists());
    }

    #[test]
    fn injectors_old_game_settings_and_configuration_collision() {
        let (_dir, shared, game) = setup();
        let old: lapinux_core::model::GameSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(old.frame_generation, None);
        assert_eq!(old.afmf_version, None);
        assert!(shared
            .launch_injector_override(&game, game.exe.as_ref().unwrap(), FrameGeneration::Off)
            .unwrap()
            .is_none());
        let fake = lapinux_core::configurations::InstalledConfiguration {
            id: "old-patch".into(),
            files: vec![lapinux_core::configurations::InstalledFile {
                target: "DXGI.DLL".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut records = HashMap::new();
        records.insert(42u32, vec![fake]);
        shared.store.set_json("configurations", &records).unwrap();
        assert!(install(&shared, &game).is_err());
        assert!(!game.install_dir().join("dxgi.dll").exists());
        assert!(shared.injector_records().unwrap().is_empty());
    }

    fn fake_pe32() -> Vec<u8> {
        let mut pe = vec![0; 134];
        pe[..2].copy_from_slice(b"MZ");
        pe[60..64].copy_from_slice(&128u32.to_le_bytes());
        pe[128..132].copy_from_slice(b"PE\0\0");
        pe[132..134].copy_from_slice(&0x14cu16.to_le_bytes());
        pe
    }
    fn cnc_archive() -> (Vec<u8>, String, String) {
        let dll = fake_pe32();
        let ini = b"[ddraw]\nsavesettings=1\n";
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        w.start_file("ddraw.dll", zip::write::SimpleFileOptions::default())
            .unwrap();
        w.write_all(&dll).unwrap();
        w.start_file("ddraw.ini", zip::write::SimpleFileOptions::default())
            .unwrap();
        w.write_all(ini).unwrap();
        (w.finish().unwrap().into_inner(), digest(&dll), digest(ini))
    }
    fn cnc_install(shared: &Shared, game: &Game) -> Result<Envelope> {
        let (bytes, dll, ini) = cnc_archive();
        shared.install_graphics_injector_inner(
            game.clone(),
            "cnc-ddraw",
            "7.1.0.0",
            "game.exe",
            "ddraw.dll",
            Some((&bytes, &digest(&bytes), Some(&dll), Some(&ini))),
        )
    }
    fn setup32() -> (tempfile::TempDir, Shared, Game) {
        let (dir, shared, game) = setup();
        std::fs::write(game.exe.as_ref().unwrap(), fake_pe32()).unwrap();
        (dir, shared, game)
    }
    fn cnc_warnings(shared: &Shared) -> Vec<String> {
        shared
            .remove_graphics_injector(42, "cnc-ddraw")
            .unwrap()
            .into_data()
            .unwrap()
    }
    #[test]
    fn cnc_install_launch_and_remove_preserve_foreign_overrides() {
        let (_dir, shared, mut game) = setup32();
        game.settings
            .env
            .push(("WINEDLLOVERRIDES".into(), "foo=n,b".into()));
        shared.store.save_game(&game).unwrap();
        cnc_install(&shared, &game).unwrap();
        let dir = game.install_dir();
        assert!(dir.join("ddraw.dll").is_file() && dir.join("ddraw.ini").is_file());
        assert!(dir
            .join(".lapinux-cnc-ddraw-v1/LICENSE.cnc-ddraw.txt")
            .is_file());
        assert_eq!(
            shared
                .launch_injector_override(&game, game.exe.as_ref().unwrap(), FrameGeneration::Off)
                .unwrap()
                .as_deref(),
            Some("foo=n,b;ddraw=n,b")
        );
        assert!(shared
            .launch_injector_override(
                &game,
                game.exe.as_ref().unwrap(),
                FrameGeneration::AfmfLinux
            )
            .is_err());
        game.settings
            .env
            .push(("WINEDLLOVERRIDES".into(), "DDRAW.DLL=b".into()));
        assert!(shared
            .launch_injector_override(&game, game.exe.as_ref().unwrap(), FrameGeneration::Off)
            .is_err());
        game.settings.env.pop();
        game.settings
            .env
            .push(("CNC_DDRAW_CONFIG_FILE".into(), "custom.ini".into()));
        assert!(shared
            .launch_injector_override(&game, game.exe.as_ref().unwrap(), FrameGeneration::Off)
            .is_err());
        assert!(cnc_warnings(&shared).is_empty());
        assert!(!dir.join("ddraw.dll").exists() && !dir.join("ddraw.ini").exists());
    }
    #[test]
    fn cnc_preserves_existing_modified_and_unrelated_files() {
        let (_dir, shared, game) = setup32();
        let dir = game.install_dir();
        std::fs::write(dir.join("DDRAW.DLL"), b"user").unwrap();
        assert!(cnc_install(&shared, &game).is_err());
        assert_eq!(std::fs::read(dir.join("DDRAW.DLL")).unwrap(), b"user");
        std::fs::remove_file(dir.join("DDRAW.DLL")).unwrap();
        std::fs::write(dir.join("DDRAW.INI"), b"old settings").unwrap();
        assert!(cnc_install(&shared, &game).is_err());
        assert_eq!(
            std::fs::read(dir.join("DDRAW.INI")).unwrap(),
            b"old settings"
        );
        std::fs::remove_file(dir.join("DDRAW.INI")).unwrap();
        cnc_install(&shared, &game).unwrap();
        // Un éditeur ou Wine peut remplacer l'INI par renommage, sans garder l'inode.
        std::fs::write(dir.join("replacement.ini"), b"modified").unwrap();
        std::fs::rename(dir.join("replacement.ini"), dir.join("ddraw.ini")).unwrap();
        assert!(shared
            .launch_injector_override(&game, game.exe.as_ref().unwrap(), FrameGeneration::Off)
            .is_ok());
        let warning = cnc_warnings(&shared);
        assert!(!warning.is_empty());
        assert_eq!(std::fs::read(dir.join("ddraw.ini")).unwrap(), b"modified");
        assert!(!dir.join("ddraw.dll").exists());
        assert_eq!(shared.injector_records().unwrap()[&42].len(), 1);
        std::fs::remove_file(dir.join("ddraw.ini")).unwrap();
        let stage = dir.join(".lapinux-cnc-ddraw-v1");
        std::fs::hard_link(stage.join("ddraw.ini"), dir.join("ddraw.ini")).unwrap();
        std::fs::write(stage.join("mine"), b"keep").unwrap();
        assert!(!cnc_warnings(&shared).is_empty());
        assert_eq!(std::fs::read(stage.join("mine")).unwrap(), b"keep");
        std::fs::remove_file(stage.join("mine")).unwrap();
        assert!(cnc_warnings(&shared).is_empty());
    }
    #[test]
    fn cnc_in_place_ini_changes_are_preserved() {
        let (_dir, shared, game) = setup32();
        cnc_install(&shared, &game).unwrap();
        let root = game.install_dir();
        std::fs::write(root.join("ddraw.ini"), b"saved window settings").unwrap();
        assert!(shared
            .launch_injector_override(&game, game.exe.as_ref().unwrap(), FrameGeneration::Off)
            .is_ok());
        assert!(!cnc_warnings(&shared).is_empty());
        assert_eq!(
            std::fs::read(root.join("ddraw.ini")).unwrap(),
            b"saved window settings"
        );
        assert_eq!(
            std::fs::read(root.join(".lapinux-cnc-ddraw-v1/ddraw.ini")).unwrap(),
            b"saved window settings"
        );
        assert_eq!(shared.injector_records().unwrap()[&42].len(), 1);
    }
    #[test]
    fn cnc_untracked_stage_blocks_install() {
        let (_dir, shared, game) = setup32();
        let stage = game.install_dir().join(".lapinux-cnc-ddraw-v1");
        std::fs::create_dir(&stage).unwrap();
        std::fs::write(stage.join("partial"), b"keep").unwrap();
        assert!(cnc_install(&shared, &game).is_err());
        assert_eq!(std::fs::read(stage.join("partial")).unwrap(), b"keep");
        assert!(!game.install_dir().join("ddraw.dll").exists());
    }
    #[test]
    fn cnc_rejects_tracked_configuration_files() {
        for name in ["DDRAW.DLL", "DDRAW.INI"] {
            let (_dir, shared, game) = setup32();
            let config = lapinux_core::configurations::InstalledConfiguration {
                id: "old-patch".into(),
                files: vec![lapinux_core::configurations::InstalledFile {
                    target: name.into(),
                    ..Default::default()
                }],
                ..Default::default()
            };
            let mut records = HashMap::new();
            records.insert(42u32, vec![config]);
            shared.store.set_json("configurations", &records).unwrap();
            assert!(cnc_install(&shared, &game).is_err());
            assert!(!game.install_dir().join("ddraw.dll").exists());
            assert!(!game.install_dir().join("ddraw.ini").exists());
            assert!(!game.install_dir().join(".lapinux-cnc-ddraw-v1").exists());
            assert!(shared.injector_records().unwrap().is_empty());
        }
    }
    #[test]
    fn cnc_rejects_wrong_archives_architecture_and_existing_injector() {
        let (_dir, shared, mut game) = setup();
        assert!(cnc_install(&shared, &game).is_err());
        std::fs::write(game.exe.as_ref().unwrap(), fake_pe32()).unwrap();
        let (bytes, dll, ini) = cnc_archive();
        assert!(shared
            .install_graphics_injector_inner(
                game.clone(),
                "cnc-ddraw",
                "7.1.0.0",
                "game.exe",
                "ddraw.dll",
                Some((&bytes, "wrong", Some(&dll), Some(&ini)))
            )
            .is_err());
        assert!(!game.install_dir().join("ddraw.dll").exists());
        game.settings
            .env
            .push(("CNC_DDRAW_CONFIG_FILE".into(), "personal.ini".into()));
        assert!(cnc_install(&shared, &game).is_err());
        game.settings.env.clear();
        game.settings.compatibility.wined3d = true;
        assert!(cnc_install(&shared, &game).is_err());
        game.settings.compatibility.wined3d = false;
        for (key, value) in [
            ("PROTON_USE_WINED3D", "1"),
            ("STEAM_COMPAT_CONFIG", "foo,wined3d"),
            ("AFMF_ENABLE", "1"),
            ("ENABLE_VKBASALT", "1"),
            ("PROTON_USE_OPTISCALER", "1"),
            ("WINEDLLOVERRIDES", "foo,DDRAW.DLL=b"),
        ] {
            game.settings.env.push((key.into(), value.into()));
            assert!(cnc_install(&shared, &game).is_err(), "{key}");
            game.settings.env.clear();
        }
        install(&shared, &game).unwrap();
        assert!(cnc_install(&shared, &game).is_err());
        assert!(warnings(&shared).is_empty());
    }
    #[test]
    fn cnc_invalid_component_does_not_install() {
        let (bytes, dll, ini) = cnc_archive();
        for (data, dll_hash, ini_hash) in [
            (bytes.as_slice(), "wrong", ini.as_str()),
            (bytes.as_slice(), dll.as_str(), "wrong"),
            (b"not a zip".as_slice(), dll.as_str(), ini.as_str()),
        ] {
            let (_dir, shared, game) = setup32();
            assert!(shared
                .install_graphics_injector_inner(
                    game.clone(),
                    "cnc-ddraw",
                    "7.1.0.0",
                    "game.exe",
                    "ddraw.dll",
                    Some((data, &digest(data), Some(dll_hash), Some(ini_hash)))
                )
                .is_err());
            assert!(!game.install_dir().join("ddraw.dll").exists());
            assert!(!game.install_dir().join("ddraw.ini").exists());
            assert!(shared.injector_records().unwrap().is_empty());
        }
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let mut pe64 = fake_pe32();
        pe64[132..134].copy_from_slice(&0x8664u16.to_le_bytes());
        zip.start_file("ddraw.dll", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(&pe64).unwrap();
        zip.start_file("ddraw.ini", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"ini").unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        let (_dir, shared, game) = setup32();
        assert!(shared
            .install_graphics_injector_inner(
                game.clone(),
                "cnc-ddraw",
                "7.1.0.0",
                "game.exe",
                "ddraw.dll",
                Some((
                    &bytes,
                    &digest(&bytes),
                    Some(&digest(&pe64)),
                    Some(&digest(b"ini"))
                ))
            )
            .is_err());
        assert!(!game.install_dir().join("ddraw.dll").exists());
    }
    #[test]
    fn cnc_modified_dll_is_preserved_and_blocks_launch() {
        let (_dir, shared, game) = setup32();
        cnc_install(&shared, &game).unwrap();
        let root = game.install_dir();
        std::fs::write(root.join("ddraw.dll"), b"modified dll").unwrap();
        assert!(shared
            .launch_injector_override(&game, game.exe.as_ref().unwrap(), FrameGeneration::Off)
            .is_err());
        assert!(!cnc_warnings(&shared).is_empty());
        assert_eq!(
            std::fs::read(root.join("ddraw.dll")).unwrap(),
            b"modified dll"
        );
        assert_eq!(shared.injector_records().unwrap()[&42].len(), 1);
    }
    #[test]
    fn cnc_modified_dll_and_symlink_are_not_removed() {
        let (_dir, shared, game) = setup32();
        cnc_install(&shared, &game).unwrap();
        let root = game.install_dir();
        std::fs::remove_file(root.join("ddraw.dll")).unwrap();
        std::fs::write(root.join("user.dll"), b"personal").unwrap();
        std::os::unix::fs::symlink("user.dll", root.join("ddraw.dll")).unwrap();
        assert!(!cnc_warnings(&shared).is_empty());
        assert_eq!(std::fs::read(root.join("user.dll")).unwrap(), b"personal");
        assert!(root.join("ddraw.dll").is_symlink());
        assert_eq!(shared.injector_records().unwrap()[&42].len(), 1);
        std::fs::remove_file(root.join("ddraw.dll")).unwrap();
        assert!(cnc_warnings(&shared).is_empty());
        assert_eq!(std::fs::read(root.join("user.dll")).unwrap(), b"personal");
    }
    #[test]
    fn cnc_retry_partial_install() {
        let (_dir, shared, game) = setup32();
        cnc_install(&shared, &game).unwrap();
        let root = game.install_dir();
        let mut records = shared.injector_records().unwrap();
        records.get_mut(&42).unwrap()[0].ready = false;
        shared.store.set_json(RECORDS, &records).unwrap();
        std::fs::remove_file(root.join("ddraw.ini")).unwrap();
        assert!(shared
            .launch_injector_override(&game, game.exe.as_ref().unwrap(), FrameGeneration::Off)
            .is_err());
        assert!(cnc_warnings(&shared).is_empty());
        assert!(!root.join("ddraw.dll").exists());
    }
    #[test]
    fn cnc_real_archive_install_remove() {
        let Some(base) = std::env::var_os("LAPINUX_GRAPHICS_ARCHIVE_TEST_DIR") else {
            return;
        };
        let bytes = std::fs::read(PathBuf::from(base).join("cnc-ddraw-7.1.zip")).unwrap();
        assert_eq!(digest(&bytes), CNC_HASH);
        let (_dir, shared, game) = setup32();
        shared
            .install_graphics_injector_inner(
                game.clone(),
                "cnc-ddraw",
                "7.1.0.0",
                "game.exe",
                "ddraw.dll",
                Some((&bytes, CNC_HASH, None, None)),
            )
            .unwrap();
        assert!(shared
            .launch_injector_override(&game, game.exe.as_ref().unwrap(), FrameGeneration::Off)
            .is_ok());
        assert!(cnc_warnings(&shared).is_empty());
        assert!(!game.install_dir().join("ddraw.dll").exists());
    }

    fn cnc_statuses(shared: &Shared) -> Vec<GraphicsInjectorStatus> {
        shared
            .graphics_injector_statuses(42)
            .unwrap()
            .into_data()
            .unwrap()
    }
    fn cnc_repair(shared: &Shared, game: &Game) -> Result<Envelope> {
        shared.repair_graphics_injector_inner(game.clone(), "cnc-ddraw", None)
    }

    #[test]
    fn cnc_diagnostic_intact_missing_and_repair() {
        let (_dir, shared, game) = setup32();
        cnc_install(&shared, &game).unwrap();
        let status = cnc_statuses(&shared);
        assert_eq!(status.len(), 1);
        assert_eq!(status[0].install, "ok");
        assert_eq!(status[0].launch_override, "applied");
        assert_eq!(status[0].action, "none");
        assert!(status[0].files.iter().all(|file| file.state == "intact"));
        let root = game.install_dir();
        std::fs::remove_file(root.join("ddraw.dll")).unwrap();
        let status = cnc_statuses(&shared);
        assert_eq!(status[0].install, "missing");
        assert_eq!(status[0].action, "repair");
        assert!(status[0]
            .files
            .iter()
            .any(|file| file.name == "ddraw.dll" && file.state == "missing"));
        let warnings: Vec<String> = cnc_repair(&shared, &game).unwrap().into_data().unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(root.join("ddraw.dll").is_file());
        assert_eq!(cnc_statuses(&shared)[0].install, "ok");
    }

    #[test]
    fn cnc_diagnostic_modified_is_backed_up_on_repair() {
        let (_dir, shared, game) = setup32();
        cnc_install(&shared, &game).unwrap();
        let root = game.install_dir();
        std::fs::write(root.join("ddraw.dll"), b"personal dll").unwrap();
        let status = cnc_statuses(&shared);
        assert_eq!(status[0].install, "modified");
        assert_eq!(status[0].action, "repair");
        // La DLL modifiée partage son inode avec la copie de suivi : la
        // réparation reprend l'archive officielle pour restaurer l'original.
        let (bytes, dll, ini) = cnc_archive();
        let warnings: Vec<String> = shared
            .repair_graphics_injector_inner(
                game.clone(),
                "cnc-ddraw",
                Some((&bytes, &digest(&bytes), Some(&dll), Some(&ini))),
            )
            .unwrap()
            .into_data()
            .unwrap();
        assert!(!warnings.is_empty());
        assert_eq!(std::fs::read(root.join("ddraw.dll")).unwrap(), fake_pe32());
        let backup = root.join(".lapinux-cnc-ddraw-v1/user-modified/ddraw.dll");
        assert_eq!(std::fs::read(backup).unwrap(), b"personal dll");
        assert_eq!(cnc_statuses(&shared)[0].install, "ok");
        // Une seconde modification ne remplace jamais la première sauvegarde.
        std::fs::write(root.join("ddraw.dll"), b"second dll").unwrap();
        shared
            .repair_graphics_injector_inner(
                game.clone(),
                "cnc-ddraw",
                Some((&bytes, &digest(&bytes), Some(&dll), Some(&ini))),
            )
            .unwrap();
        assert_eq!(
            std::fs::read(root.join(".lapinux-cnc-ddraw-v1/user-modified/ddraw.dll.1")).unwrap(),
            b"second dll"
        );
    }

    #[test]
    fn cnc_personalized_ini_is_never_lost() {
        let (_dir, shared, game) = setup32();
        cnc_install(&shared, &game).unwrap();
        let root = game.install_dir();
        std::fs::write(root.join("ddraw.ini"), b"[ddraw]\nsavesettings=2\n").unwrap();
        let status = cnc_statuses(&shared);
        assert_eq!(status[0].install, "ok");
        assert!(status[0]
            .files
            .iter()
            .any(|file| file.name == "ddraw.ini" && file.state == "personalized"));
        let warnings: Vec<String> = cnc_repair(&shared, &game).unwrap().into_data().unwrap();
        assert!(warnings.is_empty());
        assert_eq!(
            std::fs::read(root.join("ddraw.ini")).unwrap(),
            b"[ddraw]\nsavesettings=2\n"
        );
        let (bytes, dll, ini) = cnc_archive();
        // La réinstallation remet l'INI personnalisé en place après avoir
        // reposé les fichiers officiels.
        let warnings: Vec<String> = shared
            .reinstall_graphics_injector_inner(
                42,
                "cnc-ddraw",
                Some((&bytes, &digest(&bytes), Some(&dll), Some(&ini))),
            )
            .unwrap()
            .into_data()
            .unwrap();
        assert!(warnings
            .iter()
            .any(|warning| warning.contains("ddraw.ini personnalisé")));
        assert_eq!(
            std::fs::read(root.join("ddraw.ini")).unwrap(),
            b"[ddraw]\nsavesettings=2\n"
        );
        assert!(root.join("ddraw.dll").is_file());
        assert_eq!(shared.injector_records().unwrap()[&42].len(), 1);
    }

    #[test]
    fn cnc_diagnostic_override_blocked_then_fixed() {
        let (_dir, shared, game) = setup32();
        cnc_install(&shared, &game).unwrap();
        let mut game = shared.store.game(42).unwrap().unwrap();
        game.settings.compatibility.wined3d = true;
        shared.store.save_game(&game).unwrap();
        let status = cnc_statuses(&shared);
        assert_eq!(status[0].launch_override, "blocked");
        assert_eq!(status[0].override_reason, "wined3d");
        assert_eq!(status[0].action, "fix_override");
        shared
            .fix_graphics_injector_override(42, "cnc-ddraw")
            .unwrap();
        assert!(
            !shared
                .store
                .game(42)
                .unwrap()
                .unwrap()
                .settings
                .compatibility
                .wined3d
        );
        assert_eq!(cnc_statuses(&shared)[0].launch_override, "applied");
        let mut game = shared.store.game(42).unwrap().unwrap();
        game.settings.frame_generation = Some(FrameGeneration::AfmfLinux);
        game.settings.vkbasalt = Some(true);
        shared.store.save_game(&game).unwrap();
        let status = cnc_statuses(&shared);
        assert_eq!(status[0].override_reason, "frame_generation");
        shared
            .fix_graphics_injector_override(42, "cnc-ddraw")
            .unwrap();
        let game = shared.store.game(42).unwrap().unwrap();
        assert_eq!(game.settings.frame_generation, Some(FrameGeneration::Off));
        assert_eq!(game.settings.vkbasalt, Some(false));
        assert_eq!(cnc_statuses(&shared)[0].launch_override, "applied");
    }

    #[test]
    fn cnc_override_blocked_by_user_variables_is_explained_not_deleted() {
        let (_dir, shared, game) = setup32();
        cnc_install(&shared, &game).unwrap();
        let mut game = shared.store.game(42).unwrap().unwrap();
        game.settings
            .env
            .push(("CNC_DDRAW_CONFIG_FILE".into(), "mine.ini".into()));
        shared.store.save_game(&game).unwrap();
        let status = cnc_statuses(&shared);
        assert_eq!(status[0].override_reason, "config_file");
        assert_eq!(status[0].action, "none");
        assert!(shared
            .fix_graphics_injector_override(42, "cnc-ddraw")
            .is_err());
        assert!(shared
            .store
            .game(42)
            .unwrap()
            .unwrap()
            .settings
            .env
            .iter()
            .any(|(key, _)| key == "CNC_DDRAW_CONFIG_FILE"));
    }

    #[test]
    fn cnc_override_env_causes_do_not_offer_a_failing_button() {
        let (_dir, shared, game) = setup32();
        cnc_install(&shared, &game).unwrap();
        for (variable, value, reason) in [
            ("STEAM_COMPAT_CONFIG", "wined3d", "wined3d_env"),
            ("AFMF_ENABLE", "1", "frame_generation_env"),
            ("ENABLE_VKBASALT", "1", "vkbasalt_env"),
            ("PROTON_USE_OPTISCALER", "1", "optiscaler_proton"),
        ] {
            let mut game = shared.store.game(42).unwrap().unwrap();
            game.settings.env = vec![(variable.into(), value.into())];
            shared.store.save_game(&game).unwrap();
            let status = cnc_statuses(&shared);
            assert_eq!(status[0].override_reason, reason, "{variable}");
            assert_eq!(status[0].action, "none", "{variable}");
        }
        // Un réglage corrigeable ne doit pas masquer une variable manuelle :
        // corriger le réglage laisserait le bouton sans issue.
        let mut game = shared.store.game(42).unwrap().unwrap();
        game.settings.compatibility.wined3d = true;
        game.settings.env = vec![("CNC_DDRAW_CONFIG_FILE".into(), "mine.ini".into())];
        shared.store.save_game(&game).unwrap();
        let status = cnc_statuses(&shared);
        assert_eq!(status[0].override_reason, "config_file");
        assert_eq!(status[0].action, "none");
    }

    #[test]
    fn injector_backups_are_exposed_for_each_tool_and_hidden_when_absent() {
        let (_dir, shared, game) = setup32();
        cnc_install(&shared, &game).unwrap();
        let root = game.install_dir();
        let base = shared.injector_records().unwrap()[&42][0].clone();
        for (kind, proxy) in [
            ("cnc-ddraw", "ddraw.dll"),
            ("reshade", "dxgi.dll"),
            ("optiscaler", "dxgi.dll"),
        ] {
            let saved = root.join(format!(".lapinux-{kind}-user-modified"));
            let stage = root.join(format!(".lapinux-{kind}-stage-ancienne"));
            std::fs::create_dir_all(&saved).unwrap();
            std::fs::write(saved.join("old.dll"), b"old").unwrap();
            std::fs::create_dir_all(&stage).unwrap();
            std::fs::write(stage.join("old.dll"), b"old").unwrap();
            let mut record = base.clone();
            record.kind = kind.into();
            record.proxy = proxy.into();
            record.stage = format!(".lapinux-{kind}-v1");
            let status = injector_status(
                &game,
                &record,
                lapinux_core::config::FrameGeneration::Off,
                false,
                false,
            );
            assert_eq!(status.backups.len(), 2, "{kind}");
            assert!(
                status.backups.iter().any(
                    |backup| backup.kind == "user_modified" && Path::new(&backup.path).is_dir()
                ),
                "{kind}"
            );
            assert!(
                status
                    .backups
                    .iter()
                    .any(|backup| backup.kind == "stage" && Path::new(&backup.path).is_dir()),
                "{kind}"
            );
            std::fs::remove_dir_all(&saved).unwrap();
            std::fs::remove_dir_all(&stage).unwrap();
            let status = injector_status(
                &game,
                &record,
                lapinux_core::config::FrameGeneration::Off,
                false,
                false,
            );
            assert!(status.backups.is_empty(), "{kind}");
        }
    }

    #[test]
    fn cnc_diagnostic_incomplete_stage_is_rebuilt() {
        let (_dir, shared, game) = setup32();
        cnc_install(&shared, &game).unwrap();
        let root = game.install_dir();
        std::fs::remove_dir_all(root.join(".lapinux-cnc-ddraw-v1")).unwrap();
        let status = cnc_statuses(&shared);
        assert_eq!(status[0].install, "incomplete");
        assert_eq!(status[0].action, "repair");
        let (bytes, dll, ini) = cnc_archive();
        let warnings: Vec<String> = shared
            .repair_graphics_injector_inner(
                game.clone(),
                "cnc-ddraw",
                Some((&bytes, &digest(&bytes), Some(&dll), Some(&ini))),
            )
            .unwrap()
            .into_data()
            .unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(root.join(".lapinux-cnc-ddraw-v1/ddraw.dll").is_file());
        assert_eq!(cnc_statuses(&shared)[0].install, "ok");
    }

    #[test]
    fn cnc_diagnostic_executable_change_and_reinstall() {
        let (_dir, shared, game) = setup32();
        cnc_install(&shared, &game).unwrap();
        let root = game.install_dir();
        std::fs::create_dir(root.join("sub")).unwrap();
        let other = root.join("sub/other.exe");
        std::fs::write(&other, fake_pe32()).unwrap();
        let mut game = shared.store.game(42).unwrap().unwrap();
        game.exe = Some(other.clone());
        shared.store.save_game(&game).unwrap();
        let status = cnc_statuses(&shared);
        assert_eq!(status[0].install, "executable_changed");
        assert_eq!(status[0].action, "reinstall");
        let (bytes, dll, ini) = cnc_archive();
        shared
            .reinstall_graphics_injector_inner(
                42,
                "cnc-ddraw",
                Some((&bytes, &digest(&bytes), Some(&dll), Some(&ini))),
            )
            .unwrap();
        assert!(root.join("sub/ddraw.dll").is_file());
        assert!(!root.join("ddraw.dll").exists());
        let status = cnc_statuses(&shared);
        assert_eq!(status[0].install, "ok");
        assert_eq!(status[0].executable, "sub/other.exe");
    }

    #[test]
    fn cnc_reinstall_preserves_ini_and_backs_up_modified_dll() {
        let (_dir, shared, game) = setup32();
        cnc_install(&shared, &game).unwrap();
        let root = game.install_dir();
        std::fs::write(root.join("ddraw.ini"), b"[ddraw]\nwindowed=1\n").unwrap();
        std::fs::write(root.join("ddraw.dll"), b"patched dll").unwrap();
        std::fs::create_dir(root.join("sub")).unwrap();
        let other = root.join("sub/other.exe");
        std::fs::write(&other, fake_pe32()).unwrap();
        let mut game = shared.store.game(42).unwrap().unwrap();
        game.exe = Some(other.clone());
        shared.store.save_game(&game).unwrap();
        let status = cnc_statuses(&shared);
        assert_eq!(status[0].install, "executable_changed");
        assert_eq!(status[0].action, "reinstall");
        let (bytes, dll, ini) = cnc_archive();
        let warnings: Vec<String> = shared
            .reinstall_graphics_injector_inner(
                42,
                "cnc-ddraw",
                Some((&bytes, &digest(&bytes), Some(&dll), Some(&ini))),
            )
            .unwrap()
            .into_data()
            .unwrap();
        assert!(warnings
            .iter()
            .any(|warning| warning.contains("ddraw.ini personnalisé")));
        let new_folder = root.join("sub");
        assert_eq!(
            std::fs::read(new_folder.join("ddraw.ini")).unwrap(),
            b"[ddraw]\nwindowed=1\n"
        );
        assert_eq!(
            std::fs::read(new_folder.join("ddraw.dll")).unwrap(),
            fake_pe32()
        );
        assert_eq!(
            std::fs::read(root.join(".lapinux-cnc-ddraw-user-modified/ddraw.dll")).unwrap(),
            b"patched dll"
        );
        assert!(!root.join("ddraw.dll").exists());
        let status = cnc_statuses(&shared);
        assert_eq!(status[0].install, "ok");
        assert_eq!(
            status[0]
                .files
                .iter()
                .find(|file| file.name == "ddraw.ini")
                .unwrap()
                .state,
            "personalized"
        );
        assert_eq!(
            status[0]
                .files
                .iter()
                .find(|file| file.name == "ddraw.dll")
                .unwrap()
                .state,
            "intact"
        );
    }

    #[test]
    fn cnc_reinstall_recovers_corrupt_stage_and_changed_executable() {
        let (_dir, shared, game) = setup32();
        cnc_install(&shared, &game).unwrap();
        let root = game.install_dir();
        let ini = b"[ddraw]\nwindowed=1\nsavesettings=2\n";
        std::fs::write(root.join("ddraw.ini"), ini).unwrap();
        std::fs::write(root.join("ddraw.dll"), b"patched dll").unwrap();
        // Une sauvegarde antérieure ne doit jamais être écrasée par la
        // nouvelle : celle-ci prend un suffixe.
        let backups = root.join(".lapinux-cnc-ddraw-user-modified");
        std::fs::create_dir_all(&backups).unwrap();
        std::fs::write(backups.join("ddraw.dll"), b"older backup").unwrap();
        // La copie de suivi devient réellement incomplète : le fichier retiré
        // n'existe que dans cette copie, aucune cible ne le partage.
        std::fs::remove_file(root.join(".lapinux-cnc-ddraw-v1/LICENSE.cnc-ddraw.txt")).unwrap();
        std::fs::create_dir(root.join("sub")).unwrap();
        let other = root.join("sub/other.exe");
        std::fs::write(&other, fake_pe32()).unwrap();
        let mut game = shared.store.game(42).unwrap().unwrap();
        game.exe = Some(other);
        shared.store.save_game(&game).unwrap();
        let status = cnc_statuses(&shared);
        assert_eq!(status[0].install, "executable_changed");
        assert_eq!(status[0].action, "reinstall");
        let (bytes, dll_hash, ini_hash) = cnc_archive();
        let warnings: Vec<String> = shared
            .reinstall_graphics_injector_inner(
                42,
                "cnc-ddraw",
                Some((&bytes, &digest(&bytes), Some(&dll_hash), Some(&ini_hash))),
            )
            .unwrap()
            .into_data()
            .unwrap();
        assert!(warnings
            .iter()
            .any(|warning| warning.contains("ddraw.ini personnalisé")));
        assert!(warnings
            .iter()
            .any(|warning| warning.contains("fichiers modifiés sauvegardés")));
        assert!(warnings
            .iter()
            .any(|warning| warning.contains("ancienne copie de suivi")));
        let sub = root.join("sub");
        assert_eq!(std::fs::read(sub.join("ddraw.ini")).unwrap(), ini);
        assert_eq!(std::fs::read(sub.join("ddraw.dll")).unwrap(), fake_pe32());
        assert_eq!(
            std::fs::read(root.join(".lapinux-cnc-ddraw-user-modified/ddraw.dll")).unwrap(),
            b"older backup"
        );
        assert_eq!(
            std::fs::read(root.join(".lapinux-cnc-ddraw-user-modified/ddraw.dll.1")).unwrap(),
            b"patched dll"
        );
        assert!(root.join(".lapinux-cnc-ddraw-stage-ancienne").is_dir());
        assert!(!root.join("ddraw.dll").exists());
        assert!(!root.join("ddraw.ini").exists());
        let status = cnc_statuses(&shared);
        assert_eq!(status[0].install, "ok");
        assert_eq!(status[0].action, "none");
        assert_eq!(status[0].executable, "sub/other.exe");
        assert!(status[0]
            .backups
            .iter()
            .any(|backup| backup.kind == "user_modified" && Path::new(&backup.path).is_dir()));
        assert!(status[0]
            .backups
            .iter()
            .any(|backup| backup.kind == "stage" && Path::new(&backup.path).is_dir()));
        assert_eq!(
            status[0]
                .files
                .iter()
                .find(|file| file.name == "ddraw.ini")
                .unwrap()
                .state,
            "personalized"
        );
        assert_eq!(
            status[0]
                .files
                .iter()
                .find(|file| file.name == "ddraw.dll")
                .unwrap()
                .state,
            "intact"
        );
    }

    #[test]
    fn cnc_repair_works_after_executable_change() {
        let (_dir, shared, game) = setup32();
        cnc_install(&shared, &game).unwrap();
        let root = game.install_dir();
        std::fs::write(root.join("ddraw.dll"), b"patched dll").unwrap();
        std::fs::remove_file(root.join(".lapinux-cnc-ddraw-v1/LICENSE.cnc-ddraw.txt")).unwrap();
        std::fs::create_dir(root.join("sub")).unwrap();
        let other = root.join("sub/other.exe");
        std::fs::write(&other, fake_pe32()).unwrap();
        let mut game = shared.store.game(42).unwrap().unwrap();
        game.exe = Some(other);
        shared.store.save_game(&game).unwrap();
        let (bytes, dll_hash, ini_hash) = cnc_archive();
        let warnings: Vec<String> = shared
            .repair_graphics_injector_inner(
                game.clone(),
                "cnc-ddraw",
                Some((&bytes, &digest(&bytes), Some(&dll_hash), Some(&ini_hash))),
            )
            .unwrap()
            .into_data()
            .unwrap();
        assert!(!warnings.is_empty());
        assert_eq!(std::fs::read(root.join("ddraw.dll")).unwrap(), fake_pe32());
        assert_eq!(
            std::fs::read(root.join(".lapinux-cnc-ddraw-v1/user-modified/ddraw.dll")).unwrap(),
            b"patched dll"
        );
        let status = cnc_statuses(&shared);
        assert_eq!(status[0].install, "executable_changed");
        assert_eq!(status[0].action, "reinstall");
        assert_eq!(
            status[0]
                .files
                .iter()
                .find(|file| file.name == "ddraw.dll")
                .unwrap()
                .state,
            "intact"
        );
    }

    #[test]
    fn reshade_diagnostic_missing_modified_and_repair() {
        let (_dir, shared, game) = setup();
        install(&shared, &game).unwrap();
        let root = game.install_dir();
        let status: Vec<GraphicsInjectorStatus> = shared
            .graphics_injector_statuses(42)
            .unwrap()
            .into_data()
            .unwrap();
        assert_eq!(status[0].install, "ok");
        std::fs::remove_file(root.join("dxgi.dll")).unwrap();
        let status: Vec<GraphicsInjectorStatus> = shared
            .graphics_injector_statuses(42)
            .unwrap()
            .into_data()
            .unwrap();
        assert_eq!(status[0].install, "missing");
        shared.repair_graphics_injector(42, "reshade").unwrap();
        assert!(root.join("dxgi.dll").is_file());
        std::fs::write(root.join("dxgi.dll"), b"edited").unwrap();
        let status: Vec<GraphicsInjectorStatus> = shared
            .graphics_injector_statuses(42)
            .unwrap()
            .into_data()
            .unwrap();
        assert_eq!(status[0].install, "modified");
        let archive = archive();
        let dll_hash = digest(b"fake dll for tests");
        shared
            .repair_graphics_injector_inner(
                game.clone(),
                "reshade",
                Some((&archive, &digest(&archive), Some(&dll_hash), None)),
            )
            .unwrap();
        assert_eq!(
            std::fs::read(root.join("dxgi.dll")).unwrap(),
            b"fake dll for tests"
        );
        assert_eq!(
            std::fs::read(root.join(".lapinux-reshade-v1/user-modified/dxgi.dll")).unwrap(),
            b"edited"
        );
        let status: Vec<GraphicsInjectorStatus> = shared
            .graphics_injector_statuses(42)
            .unwrap()
            .into_data()
            .unwrap();
        assert_eq!(status[0].install, "ok");
    }

    #[test]
    fn cnc_real_network_download_install_remove() {
        if std::env::var_os("LAPINUX_GRAPHICS_NETWORK_TEST").is_none() {
            return;
        }
        let (_dir, shared, game) = setup32();
        shared
            .install_graphics_injector(42, "cnc-ddraw", "7.1.0.0", "game.exe", "ddraw.dll")
            .unwrap();
        assert!(game.install_dir().join("ddraw.dll").is_file());
        assert!(cnc_warnings(&shared).is_empty());
    }

    #[test]
    fn injectors_optional_real_release_archives() {
        // Les tests ordinaires utilisent une archive synthétique ; l'exécution
        // explicite avec cette variable vérifie également les archives locales officielles.
        let Some(base) = std::env::var_os("LAPINUX_GRAPHICS_ARCHIVE_TEST_DIR") else {
            return;
        };
        let base = PathBuf::from(base);
        for (kind, name, hash, version) in [
            ("reshade", "ReShade_Setup_6.8.0.exe", RESHADE_HASH, "6.8.0"),
            ("optiscaler", "Optiscaler.7z", OPTI_HASH, "0.9.4"),
        ] {
            let bytes = std::fs::read(base.join(name)).unwrap();
            assert_eq!(digest(&bytes), hash);
            let (_dir, shared, game) = setup();
            shared
                .install_graphics_injector_inner(
                    game.clone(),
                    kind,
                    version,
                    "game.exe",
                    "dxgi.dll",
                    Some((&bytes, hash, None, None)),
                )
                .unwrap();
            assert!(shared
                .launch_injector_override(&game, game.exe.as_ref().unwrap(), FrameGeneration::Off)
                .is_ok());
            let warnings: Vec<String> = shared
                .remove_graphics_injector(42, kind)
                .unwrap()
                .into_data()
                .unwrap();
            assert!(warnings.is_empty(), "{warnings:?}");
            assert!(!game.install_dir().join("dxgi.dll").exists());
        }
    }
}
