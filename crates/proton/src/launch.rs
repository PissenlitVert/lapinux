//! Construction de la ligne de commande et de l'environnement de lancement.

use crate::{Error, Result};
use ludus_core::model::ProtonInfo;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct LaunchPlan {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct LaunchOptions {
    pub appid: u32,
    pub exe: PathBuf,
    /// Jeu Linux natif : exécution directe.
    pub native: bool,
    /// Proton à utiliser (obligatoire si non natif).
    pub proton: Option<ProtonInfo>,
    pub prefix: PathBuf,
    pub steam_root: Option<PathBuf>,
    /// Chemin vers `umu-run` si disponible.
    pub umu_run: Option<PathBuf>,
    pub use_umu: bool,
    pub extra_env: Vec<(String, String)>,
    pub extra_args: Vec<String>,
}

pub fn umu_run_path() -> Option<PathBuf> {
    ludus_core::paths::which("umu-run")
}

/// Répertoire `steamapps` de la bibliothèque qui contient l'exécutable.
/// Proton s'en sert pour mapper le lecteur du jeu (`STEAM_COMPAT_LIBRARY_PATHS`).
pub fn steamapps_of(exe: &Path) -> Option<PathBuf> {
    exe.ancestors()
        .find(|p| p.file_name().and_then(|n| n.to_str()) == Some("steamapps"))
        .map(|p| p.to_path_buf())
}

pub fn build(options: &LaunchOptions) -> Result<LaunchPlan> {
    if options.native {
        return Ok(LaunchPlan {
            program: options.exe.clone(),
            args: options.extra_args.clone(),
            env: options.extra_env.clone(),
            cwd: options.exe.parent().map(|p| p.to_path_buf()),
        });
    }
    let proton = options
        .proton
        .as_ref()
        .ok_or_else(|| Error::Other("aucun Proton sélectionné".into()))?;
    if !proton.path.is_dir() {
        return Err(Error::Other(format!(
            "Proton introuvable : {}",
            proton.path.display()
        )));
    }
    let exe = options.exe.to_string_lossy().to_string();

    if options.use_umu {
        if let Some(umu) = &options.umu_run {
            let mut env = vec![
                ("GAMEID".to_string(), format!("ludus-{}", options.appid)),
                ("PROTONPATH".to_string(), proton.path.to_string_lossy().to_string()),
                ("WINEPREFIX".to_string(), options.prefix.to_string_lossy().to_string()),
                ("STORE".to_string(), "none".to_string()),
                ("UMU_RUNTIME_UPDATE".to_string(), "0".to_string()),
            ];
            env.extend(options.extra_env.iter().cloned());
            let mut args = vec![exe];
            args.extend(options.extra_args.iter().cloned());
            return Ok(LaunchPlan {
                program: umu.clone(),
                args,
                env,
                cwd: options.exe.parent().map(|p| p.to_path_buf()),
            });
        }
    }

    let proton_script = proton.path.join("proton");
    if !proton_script.is_file() {
        return Err(Error::Other(format!(
            "script proton absent dans {}",
            proton.path.display()
        )));
    }
    let client = options
        .steam_root
        .clone()
        .unwrap_or_else(|| proton.path.clone());
    let mut env = vec![
        (
            "STEAM_COMPAT_CLIENT_INSTALL_PATH".to_string(),
            client.to_string_lossy().to_string(),
        ),
        (
            "STEAM_COMPAT_DATA_PATH".to_string(),
            options.prefix.to_string_lossy().to_string(),
        ),
        ("SteamAppId".to_string(), options.appid.to_string()),
        ("SteamGameId".to_string(), options.appid.to_string()),
    ];
    if let Some(dir) = options.exe.parent() {
        env.push((
            "STEAM_COMPAT_INSTALL_PATH".to_string(),
            dir.to_string_lossy().to_string(),
        ));
    }
    if let Some(apps) = steamapps_of(&options.exe) {
        // Proton attend le répertoire `steamapps` lui-même, séparé par `:`.
        env.push((
            "STEAM_COMPAT_LIBRARY_PATHS".to_string(),
            apps.to_string_lossy().to_string(),
        ));
    }
    env.extend(options.extra_env.iter().cloned());

    // Le script `proton` est du Python moderne (>= 3.11). Le runtime sniper de
    // Steam embarque Python 3.9, donc on l'exécute avec le Python de l'hôte :
    // c'est ce qui fonctionne sur Bazzite/Fedora, sans conteneur supplémentaire.
    let mut proton_args: Vec<String> = vec!["run".to_string(), exe];
    proton_args.extend(options.extra_args.iter().cloned());
    let (program, mut args) = match ludus_core::paths::which("python3") {
        Some(python) => {
            let mut args = vec![proton_script.to_string_lossy().to_string()];
            args.extend(proton_args);
            (python, args)
        }
        None => (proton_script, proton_args),
    };

    Ok(LaunchPlan {
        program: program.clone(),
        args: std::mem::take(&mut args),
        env,
        cwd: options.exe.parent().map(|p| p.to_path_buf()),
    })
}

/// Représentation lisible (utilisée par `--dry-run` et les journaux).
pub fn render_plan(plan: &LaunchPlan) -> String {
    let mut out = String::new();
    for (key, value) in &plan.env {
        out.push_str(&format!("{key}={value}\n"));
    }
    out.push_str(&plan.program.to_string_lossy());
    for arg in &plan.args {
        out.push(' ');
        out.push_str(arg);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn proton(path: &Path) -> ProtonInfo {
        ProtonInfo {
            name: "GE-Proton11-7".into(),
            path: path.to_path_buf(),
            source: "test".into(),
            version: "11-7".into(),
            runnable: true,
        }
    }

    fn options(proton_dir: &Path) -> LaunchOptions {
        LaunchOptions {
            appid: 440,
            exe: PathBuf::from("/jeux/TF2/hl2.exe"),
            native: false,
            proton: Some(proton(proton_dir)),
            prefix: PathBuf::from("/data/prefixes/440"),
            steam_root: None,
            umu_run: None,
            use_umu: false,
            extra_env: Vec::new(),
            extra_args: Vec::new(),
        }
    }

    #[test]
    fn builds_umu_plan() {
        let dir = tempfile::tempdir().unwrap();
        let mut opts = options(dir.path());
        opts.use_umu = true;
        opts.umu_run = Some(PathBuf::from("/usr/bin/umu-run"));
        opts.extra_env = vec![("DXVK_HUD".into(), "0".into())];
        let plan = build(&opts).unwrap();
        assert_eq!(plan.program, PathBuf::from("/usr/bin/umu-run"));
        assert_eq!(plan.args[0], "/jeux/TF2/hl2.exe");
        let env: std::collections::HashMap<_, _> = plan.env.iter().cloned().collect();
        assert_eq!(env.get("GAMEID").unwrap(), "ludus-440");
        assert_eq!(env.get("PROTONPATH").unwrap(), &dir.path().to_string_lossy());
        assert_eq!(env.get("WINEPREFIX").unwrap(), "/data/prefixes/440");
        assert_eq!(env.get("DXVK_HUD").unwrap(), "0");
    }

    #[test]
    fn builds_direct_proton_plan() {
        let dir = tempfile::tempdir().unwrap();
        let proton_dir = dir.path().join("GE-Proton11-7");
        std::fs::create_dir_all(&proton_dir).unwrap();
        std::fs::write(proton_dir.join("proton"), "#!/bin/sh\n").unwrap();
        let steam = tempfile::tempdir().unwrap();

        let mut opts = options(&proton_dir);
        opts.exe = PathBuf::from("/data/SteamLibrary/steamapps/common/TF2/hl2.exe");
        opts.steam_root = Some(steam.path().to_path_buf());
        let plan = build(&opts).unwrap();
        assert_eq!(plan.args.len(), 3);
        assert!(plan.args[0].ends_with("/GE-Proton11-7/proton"));
        assert_eq!(plan.args[1], "run");
        assert_eq!(plan.args[2], "/data/SteamLibrary/steamapps/common/TF2/hl2.exe");
        let env: std::collections::HashMap<_, _> = plan.env.iter().cloned().collect();
        assert_eq!(
            env.get("STEAM_COMPAT_CLIENT_INSTALL_PATH").unwrap(),
            &steam.path().to_string_lossy()
        );
        assert_eq!(
            env.get("STEAM_COMPAT_LIBRARY_PATHS").unwrap(),
            "/data/SteamLibrary/steamapps"
        );
        assert_eq!(
            env.get("STEAM_COMPAT_INSTALL_PATH").unwrap(),
            "/data/SteamLibrary/steamapps/common/TF2"
        );
        assert_eq!(env.get("SteamAppId").unwrap(), "440");
    }

    #[test]
    fn native_plan_runs_exe_directly() {
        let dir = tempfile::tempdir().unwrap();
        let mut opts = options(dir.path());
        opts.native = true;
        opts.proton = None;
        let plan = build(&opts).unwrap();
        assert_eq!(plan.program, PathBuf::from("/jeux/TF2/hl2.exe"));
        assert_eq!(plan.cwd.unwrap(), PathBuf::from("/jeux/TF2"));
    }

    #[test]
    fn missing_proton_is_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut opts = options(dir.path());
        opts.proton = None;
        assert!(build(&opts).is_err());
    }

    #[test]
    fn steamapps_detected_from_exe() {
        let exe = Path::new("/mnt/Jeux/SteamLibrary/steamapps/common/Game/game.exe");
        assert_eq!(
            steamapps_of(exe).unwrap(),
            PathBuf::from("/mnt/Jeux/SteamLibrary/steamapps")
        );
        assert!(steamapps_of(Path::new("/tmp/game.exe")).is_none());
    }

    #[test]
    fn render_contains_env_and_command() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("proton"), "#!/bin/sh\n").unwrap();
        let plan = build(&options(dir.path())).unwrap();
        let text = render_plan(&plan);
        assert!(text.contains("STEAM_COMPAT_DATA_PATH=/data/prefixes/440"));
        assert!(text.contains("STEAM_COMPAT_CLIENT_INSTALL_PATH"));
        assert!(text.contains("run"));
        assert!(text.contains("/jeux/TF2/hl2.exe"));
    }
}
