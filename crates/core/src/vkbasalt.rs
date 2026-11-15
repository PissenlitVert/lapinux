//! Runtime vkBasalt fourni par Lapinux.
//!
//! Le layer est embarqué dans le binaire et copié dans le dossier de données
//! au premier lancement. Le jeu le reçoit par `VK_ADD_IMPLICIT_LAYER_PATH` :
//! aucun paquet système ni droit administrateur n'est nécessaire.

use crate::paths::LapinuxPaths;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const VERSION: &str = "0.3.2.10";
pub const LAYER_NAME: &str = "VK_LAYER_VKBASALT_post_processing";
pub const X86_64: &str = "x86_64";
pub const I686: &str = "i686";

#[cfg(target_arch = "x86_64")]
const LIB_X86_64: &[u8] = include_bytes!("../../../assets/vkbasalt/x86_64/libvkbasalt.so");
#[cfg(target_arch = "x86_64")]
const LIB_I686: &[u8] = include_bytes!("../../../assets/vkbasalt/i686/libvkbasalt.so");
#[cfg(target_arch = "x86_64")]
const MANIFEST_X86_64: &str = include_str!("../../../assets/vkbasalt/x86_64/vkBasalt.json");
#[cfg(target_arch = "x86_64")]
const MANIFEST_I686: &str = include_str!("../../../assets/vkbasalt/i686/vkBasalt.json");
const LICENSE: &str = include_str!("../../../assets/vkbasalt/LICENSE");

/// Architectures réellement embarquées dans ce binaire.
#[cfg(target_arch = "x86_64")]
pub const BUNDLED_ARCHES: &[&str] = &[X86_64, I686];
#[cfg(not(target_arch = "x86_64"))]
pub const BUNDLED_ARCHES: &[&str] = &[];

pub const BUNDLED: bool = !BUNDLED_ARCHES.is_empty();

/// État du runtime pour l'écran de réglages et le lancement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub version: String,
    pub arch: String,
    pub bundled: bool,
    pub installed: bool,
    /// L'utilisateur a retiré le runtime : le lancement ne le réinstalle pas
    /// tout seul et laisse le layer système prendre le relais s'il existe.
    pub disabled: bool,
    pub system: bool,
    pub path: Option<PathBuf>,
    pub arches: Vec<String>,
    pub bundled_arches: Vec<String>,
    pub bytes: u64,
}

fn asset(arch: &str) -> Option<(&'static [u8], &'static str)> {
    match arch {
        #[cfg(target_arch = "x86_64")]
        X86_64 => Some((LIB_X86_64, MANIFEST_X86_64)),
        #[cfg(target_arch = "x86_64")]
        I686 => Some((LIB_I686, MANIFEST_I686)),
        _ => None,
    }
}

/// Dossier où Lapinux range ses runtimes vkBasalt.
pub fn root(paths: &LapinuxPaths) -> PathBuf {
    paths.tools_dir.join("vkbasalt")
}

pub fn version_dir(paths: &LapinuxPaths) -> PathBuf {
    root(paths).join(VERSION)
}

fn disabled_marker(paths: &LapinuxPaths) -> PathBuf {
    root(paths).join(".disabled")
}

pub fn disabled(paths: &LapinuxPaths) -> bool {
    disabled_marker(paths).is_file()
}

pub fn runtime_dir(paths: &LapinuxPaths, arch: &str) -> PathBuf {
    version_dir(paths).join(arch)
}

fn lib_path(paths: &LapinuxPaths, arch: &str) -> PathBuf {
    runtime_dir(paths, arch).join("libvkbasalt.so")
}

fn manifest_path(paths: &LapinuxPaths, arch: &str) -> PathBuf {
    runtime_dir(paths, arch).join("vkBasalt.json")
}

/// Vrai si le runtime Lapinux de cette architecture est complet sur le disque.
pub fn installed_arch(paths: &LapinuxPaths, arch: &str) -> bool {
    let Some((expected, _)) = asset(arch) else {
        return false;
    };
    let lib = lib_path(paths, arch);
    let manifest = manifest_path(paths, arch);
    std::fs::metadata(&lib).is_ok_and(|m| m.is_file() && m.len() == expected.len() as u64)
        && std::fs::metadata(&manifest).is_ok_and(|m| m.is_file() && m.len() > 0)
}

/// Vrai si toutes les architectures embarquées sont installées.
pub fn installed(paths: &LapinuxPaths) -> bool {
    BUNDLED
        && BUNDLED_ARCHES
            .iter()
            .all(|arch| installed_arch(paths, arch))
}

pub fn installed_arches(paths: &LapinuxPaths) -> Vec<String> {
    BUNDLED_ARCHES
        .iter()
        .filter(|arch| installed_arch(paths, arch))
        .map(|arch| (*arch).to_string())
        .collect()
}

/// Détecte un layer vkBasalt déjà installé par la distribution ou l'utilisateur.
/// Le runtime embarqué reste prioritaire quand il est disponible.
pub fn system_installed() -> bool {
    let manifests = [
        "/usr/share/vulkan/implicit_layer.d/vkBasalt.json".to_string(),
        "/usr/local/share/vulkan/implicit_layer.d/vkBasalt.json".to_string(),
        "/etc/vulkan/implicit_layer.d/vkBasalt.json".to_string(),
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| {
                home.join(".local/share/vulkan/implicit_layer.d/vkBasalt.json")
                    .to_string_lossy()
                    .into_owned()
            })
            .unwrap_or_default(),
    ];
    let libraries = [
        "/usr/lib64/vkbasalt/libvkbasalt.so",
        "/usr/lib/vkbasalt/libvkbasalt.so",
        "/usr/lib/x86_64-linux-gnu/vkbasalt/libvkbasalt.so",
        "/usr/lib/aarch64-linux-gnu/vkbasalt/libvkbasalt.so",
        "/usr/lib/i386-linux-gnu/vkbasalt/libvkbasalt.so",
        "/usr/lib64/libvkbasalt.so",
        "/usr/lib/libvkbasalt.so",
    ];
    manifests.iter().any(|path| Path::new(path).is_file())
        && libraries.iter().any(|path| Path::new(path).is_file())
}

pub fn status(paths: &LapinuxPaths) -> Status {
    let present = installed(paths);
    let arches = installed_arches(paths);
    Status {
        version: VERSION.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        bundled: BUNDLED,
        installed: present,
        disabled: disabled(paths),
        system: system_installed(),
        path: present.then(|| version_dir(paths)),
        arches,
        bundled_arches: BUNDLED_ARCHES.iter().map(|arch| arch.to_string()).collect(),
        bytes: BUNDLED_ARCHES
            .iter()
            .filter_map(|arch| asset(arch))
            .map(|(lib, _)| lib.len() as u64)
            .sum(),
    }
}

fn install_arch(paths: &LapinuxPaths, arch: &str) -> Result<PathBuf, String> {
    let (lib, manifest) =
        asset(arch).ok_or("aucun runtime vkBasalt embarqué pour cette architecture")?;
    let dir = runtime_dir(paths, arch);
    if installed_arch(paths, arch) {
        return Ok(dir);
    }
    let parent = dir.parent().ok_or("chemin de runtime vkBasalt invalide")?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let stage = parent.join(format!(
        ".stage-{}-{}-{}",
        arch,
        std::process::id(),
        crate::util::now_unix()
    ));
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir(&stage).map_err(|error| error.to_string())?;
    let result = (|| {
        std::fs::write(stage.join("libvkbasalt.so"), lib).map_err(|error| error.to_string())?;
        std::fs::write(stage.join("vkBasalt.json"), manifest).map_err(|error| error.to_string())?;
        std::fs::write(stage.join("LICENSE"), LICENSE).map_err(|error| error.to_string())?;
        if dir.exists() {
            std::fs::remove_dir_all(&dir).map_err(|error| error.to_string())?;
        }
        std::fs::rename(&stage, &dir).map_err(|error| error.to_string())?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&stage);
    }
    result.map(|_| dir)
}

/// Installe les runtimes embarqués, sans réseau et sans root. L'opération est
/// idempotente ; un dossier incomplet est remplacé par une copie neuve.
pub fn install(paths: &LapinuxPaths) -> Result<PathBuf, String> {
    if !BUNDLED {
        return Err("aucun runtime vkBasalt embarqué pour cette architecture".into());
    }
    for arch in BUNDLED_ARCHES {
        install_arch(paths, arch)?;
    }
    let _ = std::fs::remove_file(disabled_marker(paths));
    Ok(version_dir(paths))
}

/// Retire uniquement le runtime Lapinux. Une installation système n'est
/// jamais touchée.
pub fn remove(paths: &LapinuxPaths) -> Result<(), String> {
    let dir = version_dir(paths);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|error| error.to_string())?;
    }
    if let Some(version) = dir.parent() {
        let _ = std::fs::remove_dir(version);
    }
    std::fs::create_dir_all(root(paths)).map_err(|error| error.to_string())?;
    std::fs::write(disabled_marker(paths), b"removed by user\n")
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// Dossier à ajouter à `VK_ADD_IMPLICIT_LAYER_PATH` au lancement. `bits` vient
/// de l'exécutable du jeu : 32 ou 64 bits, `None` si un lanceur le cache.
/// Sur une architecture sans binaire embarqué, on laisse le layer système
/// faire le travail s'il existe.
pub fn ensure_launch_layer(
    paths: &LapinuxPaths,
    bits: Option<u32>,
) -> Result<Option<PathBuf>, String> {
    if !BUNDLED {
        return Ok(None);
    }
    if disabled(paths) {
        return Ok(None);
    }
    let wanted = if bits == Some(32) && BUNDLED_ARCHES.contains(&I686) {
        I686
    } else if BUNDLED_ARCHES.contains(&X86_64) {
        X86_64
    } else {
        return Ok(None);
    };
    if !installed(paths) {
        install(paths)?;
    }
    Ok(Some(runtime_dir(paths, wanted)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn le_runtime_embarque_sinstalle_et_se_retire() {
        let root = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(root.path());
        assert!(!installed(&paths));
        assert!(!disabled(&paths));
        assert_eq!(status(&paths).version, VERSION);
        assert_eq!(status(&paths).arch, std::env::consts::ARCH);

        let dir = install(&paths).unwrap();
        assert!(installed(&paths));
        assert_eq!(dir, version_dir(&paths));
        for arch in BUNDLED_ARCHES {
            let arch_dir = runtime_dir(&paths, arch);
            assert!(arch_dir.join("libvkbasalt.so").is_file());
            let manifest = std::fs::read_to_string(arch_dir.join("vkBasalt.json")).unwrap();
            assert!(manifest.contains("\"library_path\": \"./libvkbasalt.so\""));
            assert!(manifest.contains(LAYER_NAME));
            assert!(arch_dir.join("LICENSE").is_file());
        }
        assert_eq!(
            std::fs::metadata(runtime_dir(&paths, X86_64).join("libvkbasalt.so"))
                .unwrap()
                .len() as usize,
            LIB_X86_64.len()
        );
        assert!(status(&paths).installed);
        assert_eq!(status(&paths).arches.len(), BUNDLED_ARCHES.len());

        // Un second appel ne casse rien et ne versionne pas deux copies.
        assert_eq!(install(&paths).unwrap(), dir);
        remove(&paths).unwrap();
        assert!(!installed(&paths));
        assert!(disabled(&paths));
        assert!(!dir.exists());
        assert_eq!(ensure_launch_layer(&paths, Some(64)).unwrap(), None);
        install(&paths).unwrap();
        assert!(!disabled(&paths));
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn le_lancement_choisit_le_runtime_32_ou_64_bits() {
        let root = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(root.path());
        assert_eq!(
            ensure_launch_layer(&paths, Some(64)).unwrap(),
            Some(runtime_dir(&paths, X86_64))
        );
        assert_eq!(
            ensure_launch_layer(&paths, Some(32)).unwrap(),
            Some(runtime_dir(&paths, I686))
        );
        // Un lanceur masqué utilise le runtime 64 bits par défaut.
        assert_eq!(
            ensure_launch_layer(&paths, None).unwrap(),
            Some(runtime_dir(&paths, X86_64))
        );
    }

    #[test]
    fn le_layer_est_implicite_et_active_par_la_variable_vkbasalt() {
        assert!(MANIFEST_X86_64.contains("\"type\": \"GLOBAL\""));
        assert!(MANIFEST_X86_64.contains("ENABLE_VKBASALT"));
        assert!(MANIFEST_X86_64.contains("DISABLE_VKBASALT"));
        assert!(MANIFEST_X86_64.contains("vkBasalt_GetInstanceProcAddr"));
    }
}
