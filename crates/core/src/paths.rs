//! Chemins XDG, découverte des racines Steam et utilitaires système.

use std::path::{Path, PathBuf};

/// Emplacements utilisés par Ludus sur le disque.
#[derive(Debug, Clone)]
pub struct LudusPaths {
    pub config_dir: PathBuf,
    pub config_file: PathBuf,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub log_dir: PathBuf,
    pub db_file: PathBuf,
    pub tools_dir: PathBuf,
    pub proton_dir: PathBuf,
    pub run_dir: PathBuf,
    pub socket: PathBuf,
}

impl LudusPaths {
    pub fn discover() -> Self {
        if let Ok(root) = std::env::var("LUDUS_HOME") {
            let root = PathBuf::from(root);
            return Self::under(&root);
        }
        let home = home_dir();
        let config_dir = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"))
            .join("ludus");
        let data_dir = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"))
            .join("ludus");
        let cache_dir = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".cache"))
            .join("ludus");
        let run_dir = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("ludus");
        Self::assemble(config_dir, data_dir, cache_dir, run_dir)
    }

    /// Tous les chemins sous une racine donnée (tests, profils isolés).
    pub fn under(root: &Path) -> Self {
        Self::assemble(
            root.join("config"),
            root.join("data"),
            root.join("cache"),
            root.join("run"),
        )
    }

    fn assemble(config_dir: PathBuf, data_dir: PathBuf, cache_dir: PathBuf, run_dir: PathBuf) -> Self {
        Self {
            config_file: config_dir.join("config.toml"),
            log_dir: data_dir.join("logs"),
            db_file: data_dir.join("ludus.redb"),
            tools_dir: data_dir.join("tools"),
            proton_dir: data_dir.join("protons"),
            socket: run_dir.join("control.sock"),
            config_dir,
            data_dir,
            cache_dir,
            run_dir,
        }
    }

    /// Crée les répertoires nécessaires (idempotent).
    pub fn ensure(&self) -> std::io::Result<()> {
        for dir in [
            &self.config_dir,
            &self.data_dir,
            &self.cache_dir,
            &self.log_dir,
            &self.tools_dir,
            &self.proton_dir,
            &self.run_dir,
        ] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }

    pub fn covers_dir(&self) -> PathBuf {
        self.cache_dir.join("covers")
    }
}

pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// Racines Steam plausibles, dédupliquées et existantes.
pub fn steam_roots() -> Vec<PathBuf> {
    let home = home_dir();
    let candidates = [
        home.join(".local/share/Steam"),
        home.join(".steam/steam"),
        home.join(".steam/root"),
        home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
    ];
    let mut out: Vec<PathBuf> = Vec::new();
    for c in candidates {
        if c.is_dir() {
            let canon = std::fs::canonicalize(&c).unwrap_or(c);
            if !out.iter().any(|p| p == &canon) {
                out.push(canon);
            }
        }
    }
    out
}

pub fn expand_tilde(input: &str) -> PathBuf {
    if let Some(rest) = input.strip_prefix("~/") {
        home_dir().join(rest)
    } else if input == "~" {
        home_dir()
    } else {
        PathBuf::from(input)
    }
}

pub fn which(bin: &str) -> Option<PathBuf> {
    if bin.contains('/') {
        let p = PathBuf::from(bin);
        return p.is_file().then_some(p);
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(bin))
        .find(|p| is_executable(p))
}

pub fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tilde_expansion() {
        let home = home_dir();
        assert_eq!(expand_tilde("~/jeux"), home.join("jeux"));
        assert_eq!(expand_tilde("/abs"), PathBuf::from("/abs"));
    }

    #[test]
    fn which_finds_sh() {
        assert!(which("sh").is_some());
    }
}
