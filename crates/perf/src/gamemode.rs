//! Détection des outils de performance et enrobage gamemode.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PerfCaps {
    pub gamescope: Option<PathBuf>,
    pub gamemoderun: Option<PathBuf>,
    pub mangohud: Option<PathBuf>,
    pub recorder: Option<PathBuf>,
    pub power_profiles: Option<PathBuf>,
}

impl PerfCaps {
    pub fn detect(recorder: &str) -> Self {
        Self {
            gamescope: ludus_core::paths::which("gamescope"),
            gamemoderun: ludus_core::paths::which("gamemoderun"),
            mangohud: ludus_core::paths::which("mangohud"),
            recorder: ludus_core::paths::which(recorder),
            power_profiles: ludus_core::paths::which("powerprofilesctl"),
        }
    }

    pub fn has_gamemode(&self) -> bool {
        self.gamemoderun.is_some()
    }

    /// Message affiché dans l'écran « Diagnostic » de l'UI.
    pub fn gamemode_hint(&self) -> String {
        match &self.gamemoderun {
            Some(_) => "Feral GameMode actif".into(),
            None => {
                "gamemode absent : installez-le avec « rpm-ostree install gamemode » (redémarrage requis, Bazzite atomic), ou lancez sans GameMode.".into()
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Wrapped {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub gamemode: bool,
}

/// Enrobe la commande avec `gamemoderun` quand il est disponible.
pub fn wrap(program: &Path, args: &[String], caps: &PerfCaps, gamemode: bool) -> Wrapped {
    if gamemode {
        if let Some(gamemoderun) = &caps.gamemoderun {
            let mut new_args = vec![program.to_string_lossy().to_string()];
            new_args.extend(args.iter().cloned());
            return Wrapped {
                program: gamemoderun.clone(),
                args: new_args,
                gamemode: true,
            };
        }
    }
    Wrapped {
        program: program.to_path_buf(),
        args: args.to_vec(),
        gamemode: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps_with(bin: Option<PathBuf>) -> PerfCaps {
        PerfCaps {
            gamemoderun: bin,
            ..Default::default()
        }
    }

    #[test]
    fn wraps_when_available() {
        let caps = caps_with(Some(PathBuf::from("/usr/bin/gamemoderun")));
        let wrapped = wrap(
            Path::new("/usr/bin/gamescope"),
            &["-f".into(), "--".into(), "/jeu".into()],
            &caps,
            true,
        );
        assert!(wrapped.gamemode);
        assert_eq!(wrapped.program, PathBuf::from("/usr/bin/gamemoderun"));
        assert_eq!(wrapped.args[0], "/usr/bin/gamescope");
        assert_eq!(wrapped.args.last().unwrap(), "/jeu");
    }

    #[test]
    fn no_wrap_without_binary() {
        let caps = caps_with(None);
        let wrapped = wrap(Path::new("/usr/bin/gamescope"), &[], &caps, true);
        assert!(!wrapped.gamemode);
        assert_eq!(wrapped.program, PathBuf::from("/usr/bin/gamescope"));
    }

    #[test]
    fn no_wrap_when_disabled() {
        let caps = caps_with(Some(PathBuf::from("/usr/bin/gamemoderun")));
        let wrapped = wrap(Path::new("/bin/true"), &[], &caps, false);
        assert!(!wrapped.gamemode);
        assert_eq!(wrapped.program, PathBuf::from("/bin/true"));
    }
}
