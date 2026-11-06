//! Chemins XDG, découverte des racines Steam et utilitaires système.

use std::path::{Path, PathBuf};

/// Emplacements utilisés par Lapinux sur le disque. Les dossiers gardent
/// leur nom historique `lapinux` : une installation existante reste lue.
#[derive(Debug, Clone)]
pub struct LapinuxPaths {
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

impl LapinuxPaths {
    pub fn discover() -> Self {
        if let Some(root) = crate::env_var("HOME") {
            let root = PathBuf::from(root);
            return Self::under(&root);
        }
        let home = home_dir();
        let config_dir = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"))
            .join("lapinux");
        let data_dir = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"))
            .join("lapinux");
        let cache_dir = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".cache"))
            .join("lapinux");
        let run_dir = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("lapinux");
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
            db_file: data_dir.join("lapinux.redb"),
            tools_dir: data_dir.join("tools"),
            // Les builds téléchargés vivent sous « compatibility », comme
            // dans Steam, dans un dossier « protons » à Lapinux.
            proton_dir: data_dir.join("compatibility").join("protons"),
            socket: run_dir.join("control.sock"),
            config_dir,
            data_dir,
            cache_dir,
            run_dir,
        }
    }

    /// Profil WebKit des fenêtres de boutique : cookies des comptes, cache
    /// des pages et stockage local. Nom anglais depuis le renommage des
    /// dossiers ; l'ancien « boutique » est repris une fois au démarrage.
    pub fn store_web_dir(&self) -> PathBuf {
        self.data_dir.join("store-web")
    }

    /// Crée les répertoires nécessaires (idempotent).
    pub fn ensure(&self) -> std::io::Result<()> {
        // Renommages : une version antérieure nommait ces dossiers en
        // français. Les cookies de connexion suivent, une session de boutique
        // ne se perd pas dans la mise à jour.
        migrate_dir(&self.data_dir.join("boutique"), &self.store_web_dir());
        migrate_dir(
            &self.store_web_dir().join("donnees"),
            &self.store_web_dir().join("data"),
        );
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

    /// Thèmes créés par l'utilisateur. Le dossier est créé à la première
    /// sauvegarde, pas au démarrage : une configuration sans thème personnel
    /// ne laisse pas de dossier vide.
    pub fn themes_dir(&self) -> PathBuf {
        self.config_dir.join("themes")
    }

    /// Plugins déclaratifs de l'utilisateur.
    pub fn plugins_dir(&self) -> PathBuf {
        self.config_dir.join("plugins")
    }

    /// Marqueur de vie de l'interface : réécrit toutes les deux secondes
    /// pendant qu'elle tourne. Le daemon le lit pour retirer la présence
    /// Discord même quand l'application est tuée sans fermeture propre.
    pub fn ui_marker(&self) -> PathBuf {
        self.run_dir.join("ui.alive")
    }

    /// PID de l'interface : le daemon s'en sert pour la fermer quand son menu
    /// demande de la masquer, KWin ne répondant pas toujours.
    pub fn ui_pid(&self) -> PathBuf {
        self.run_dir.join("ui.pid")
    }

    /// Demande de lancement déposée par le tray : l'interface la lit et
    /// affiche son écran d'attente, puis la retire.
    pub fn launch_request(&self) -> PathBuf {
        self.run_dir.join("launch.request")
    }

    /// Réveil de l'interface à la fermeture du jeu. La fenêtre principale peut
    /// être réduite et son fil de peinture endormi : ce petit fichier lui
    /// demande une image pour que l'écran de fin soit posé sans attendre un
    /// minuteur lointain.
    pub fn end_request(&self) -> PathBuf {
        self.run_dir.join("end.request")
    }

    /// Illustrations officielles des fiches Steam (bannière, logo, icône).
    pub fn art_dir(&self) -> PathBuf {
        self.cache_dir.join("art")
    }
}

/// Dossier personnel de l'utilisateur (`XDG_*_DIR`), lu dans
/// `~/.config/user-dirs.dirs` : les noms y sont localisés (« Vidéos »,
/// « Images »), et les écrire en dur enverrait les fichiers à côté du dossier
/// que le bureau affiche.
pub fn user_dir(key: &str, fallback: &[&str]) -> PathBuf {
    let home = home_dir();
    if let Ok(text) = std::fs::read_to_string(home.join(".config/user-dirs.dirs")) {
        for line in text.lines() {
            let line = line.trim();
            let Some(value) = line.strip_prefix(key).and_then(|rest| rest.trim().strip_prefix('=')) else {
                continue;
            };
            let value = value.trim().trim_matches('"').trim();
            if value.is_empty() {
                break;
            }
            let expanded = value.replace("$HOME", &home.to_string_lossy());
            return expand_tilde(&expanded);
        }
    }
    // Bureau sans configuration XDG : le premier nom qui existe déjà, sinon le
    // premier de la liste.
    fallback
        .iter()
        .map(|name| home.join(name))
        .find(|path| path.is_dir())
        .unwrap_or_else(|| home.join(fallback.first().copied().unwrap_or("Documents")))
}

/// Dossier XDG des vidéos (celui que le bureau affiche).
pub fn videos_dir() -> PathBuf {
    user_dir("XDG_VIDEOS_DIR", &["Vidéos", "Videos"])
}

/// Dossier XDG des images.
pub fn pictures_dir() -> PathBuf {
    user_dir("XDG_PICTURES_DIR", &["Images", "Pictures"])
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

/// Renomme un dossier laissé par une version antérieure, seulement si sa
/// nouvelle place est libre : deux dossiers ne fusionnent jamais en silence.
fn migrate_dir(from: &Path, to: &Path) {
    if from.is_dir() && !to.exists() {
        let _ = std::fs::rename(from, to);
    }
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

/// Date de modification d'un binaire, en millisecondes Unix : elle identifie
/// le build qui tourne.
pub fn binary_stamp(path: &Path) -> Option<u64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let since = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(since.as_millis() as u64)
}

/// Empreinte du binaire en cours d'exécution.
pub fn current_binary_stamp() -> u64 {
    std::env::current_exe()
        .ok()
        .and_then(|path| binary_stamp(&path))
        .unwrap_or(0)
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

    /// Le profil des boutiques déménage de « boutique » vers « store-web »
    /// sans perdre ses cookies ni son sous-dossier « donnees » devenu « data ».
    #[test]
    fn legacy_web_profile_is_renamed() {
        let root = std::env::temp_dir().join(format!(
            "lapinux-paths-{}",
            std::process::id() as u64 * 1000
                + std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .subsec_nanos() as u64
        ));
        let _ = std::fs::remove_dir_all(&root);
        let paths = LapinuxPaths::under(&root);
        std::fs::create_dir_all(paths.data_dir.join("boutique/donnees")).unwrap();
        std::fs::write(paths.data_dir.join("boutique/cookies.sqlite"), b"cookie").unwrap();
        paths.ensure().unwrap();
        assert!(paths.store_web_dir().join("cookies.sqlite").is_file());
        assert!(paths.store_web_dir().join("data").is_dir());
        assert!(!paths.data_dir.join("boutique").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

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
