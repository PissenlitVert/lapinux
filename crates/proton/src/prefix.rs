//! Gestion des préfixes Wine/Proton, par jeu.

use crate::{Error, Result};
use lapinux_core::model::{Game, GameOrigin, ProtonInfo};
use std::path::{Path, PathBuf};

/// Préfixe d'un jeu, rangé comme le fait Steam : un `compatdata` dans la
/// bibliothèque qui héberge le jeu, où le préfixe porte son numéro. Un jeu
/// Steam garde le `compatdata` de `steamapps`, celui que Steam utilise ; un
/// jeu de boutique ou installé par Lapinux a le sien à la racine de la
/// bibliothèque. Sans bibliothèque connue, il retombe sous les données de
/// Lapinux.
pub fn prefix_for(game: &Game, data_dir: &Path) -> PathBuf {
    let target = prefix_path(game, data_dir);
    if target.exists() {
        return target;
    }
    // Deux anciens rangements suivent leur jeu une seule fois, sur place si
    // le disque le permet : le préfixe par numéro des données Lapinux, puis
    // l'ancien `compatdata` de la bibliothèque. Le nouveau nom lisible porte
    // le jeu, avec son identifiant entre parenthèses.
    let mut legacy = vec![data_dir.join("prefixes").join(game.appid.to_string())];
    if !game.library.as_os_str().is_empty() && game.origin != GameOrigin::Steam {
        legacy.push(game.library.join("compatdata").join(game.appid.to_string()));
        if let Some(existing) = &game.prefix {
            legacy.push(existing.clone());
        }
    }
    for old in legacy {
        if old.is_dir() {
            move_prefix(&old, &target);
            break;
        }
    }
    target
}

/// Nom du dossier de préfixe d'un jeu : son nom lisible, puis son identifiant
/// pour qu'aucun jeu ne prenne la place d'un autre. Les caractères interdits
/// dans un nom de fichier deviennent des espaces, et le tout reste assez
/// court pour un système de fichiers.
pub fn prefix_name(name: &str, appid: u32) -> String {
    let clean: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => ' ',
            c if c.is_control() => ' ',
            c => c,
        })
        .take(80)
        .collect();
    let clean = clean.trim().trim_matches('.').trim();
    let clean = if clean.is_empty() { "Jeu" } else { clean };
    format!("{clean} ({appid})")
}

/// Même nom pour un jeu complet : le daemon s'en sert pour la migration.
pub fn prefix_name_for(game: &Game) -> String {
    prefix_name(&game.name, game.appid)
}

/// Chemin du préfixe, sans rien déplacer : une lecture (sauvegardes, cloud)
/// ne doit pas déclencher la migration d'un ancien préfixe de plusieurs
/// centaines de mégaoctets.
pub fn prefix_path(game: &Game, data_dir: &Path) -> PathBuf {
    // Un vrai jeu Steam garde le `compatdata` de Steam, éventuellement déjà
    // présent sur le disque.
    if game.origin == GameOrigin::Steam {
        if let Some(existing) = &game.prefix {
            if existing.is_dir() {
                return existing.clone();
            }
        }
        let id = game.appid.to_string();
        if game.library.as_os_str().is_empty() {
            return data_dir.join("prefixes").join(id);
        }
        return game.library.join("steamapps").join("compatdata").join(id);
    }
    if game.library.as_os_str().is_empty() {
        return data_dir
            .join("prefixes")
            .join(game.appid.to_string());
    }
    let compat = game.library.join("CompatData");
    let expected = compat.join(prefix_name_for(game));
    if expected.is_dir() {
        return expected;
    }
    // Le jeu a pu être renommé : le dossier garde alors son ancien nom, mais
    // toujours son identifiant entre parenthèses. Le retrouver évite de
    // perdre le préfixe, donc les sauvegardes, au moindre changement de titre.
    let suffix = format!(" ({})", game.appid);
    if let Ok(entries) = std::fs::read_dir(&compat) {
        let mut found: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_dir()
                    && path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .map(|name| name.ends_with(&suffix))
                        .unwrap_or(false)
            })
            .collect();
        found.sort();
        if let Some(existing) = found.into_iter().next() {
            return existing;
        }
    }
    expected
}

/// Déplace un préfixe vers son nouveau rangement. Le renommage suffit sur le
/// même système de fichiers ; sinon la copie prend le relais et l'ancien
/// dossier ne part qu'une fois la copie terminée.
fn move_prefix(from: &Path, to: &Path) {
    if std::fs::rename(from, to).is_ok() {
        return;
    }
    let Some(parent) = to.parent() else {
        return;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    // La copie se fait à côté puis prend la place en une fois : une coupure
    // au milieu ne laisse pas un préfixe à moitié écrit à la place du vrai.
    let Some(name) = to.file_name() else {
        return;
    };
    let staging = parent.join(format!(".{}.moving", name.to_string_lossy()));
    let _ = std::fs::remove_dir_all(&staging);
    if lapinux_core::util::copy_tree(from, &staging).is_err() {
        let _ = std::fs::remove_dir_all(&staging);
        return;
    }
    if std::fs::rename(&staging, to).is_ok() {
        let _ = std::fs::remove_dir_all(from);
    } else {
        let _ = std::fs::remove_dir_all(&staging);
    }
}

pub fn marker_path(prefix: &Path) -> PathBuf {
    prefix.join("lapinux-prefix.json")
}

pub fn is_initialized(prefix: &Path) -> bool {
    prefix.join("pfx").is_dir() || prefix.join("drive_c").is_dir()
}

/// Crée le répertoire de préfixe et son marqueur.
pub fn ensure(prefix: &Path, appid: u32) -> Result<()> {
    std::fs::create_dir_all(prefix)?;
    let marker = marker_path(prefix);
    if !marker.exists() {
        let payload = serde_json::json!({
            "appid": appid,
            "created_by": "lapinux",
            "version": env!("CARGO_PKG_VERSION"),
        });
        std::fs::write(&marker, serde_json::to_vec_pretty(&payload)?)?;
    }
    Ok(())
}

/// Initialise un préfixe complet en lançant `wineboot` par le Proton choisi.
///
/// Un jeu jamais démarré n'a encore que le dossier du préfixe, sans `pfx` ni
/// `drive_c` : Steam Cloud ne peut alors poser aucune sauvegarde Windows.
/// Proton fait exactement cette étape au premier lancement ; la lancer ici
/// permet de restaurer les sauvegardes avant la première partie, sans jeu.
pub fn initialize(
    prefix: &Path,
    appid: u32,
    proton: &ProtonInfo,
    install_dir: &Path,
) -> Result<()> {
    if is_initialized(prefix) {
        return Ok(());
    }
    ensure(prefix, appid)?;
    let script = proton.path.join("proton");
    if !script.is_file() {
        return Err(Error::Other(format!(
            "script proton absent dans {}",
            proton.path.display()
        )));
    }
    // Même invocation que le premier lancement : Proton copie son préfixe par
    // défaut et `wineboot` l'enregistre. Le Python de l'hôte est utilisé comme
    // dans `launch` (le runtime Steam embarque une version trop ancienne).
    let python = lapinux_core::paths::which("python3");
    let mut command = match &python {
        Some(python) => {
            let mut command = std::process::Command::new(python);
            command.arg(&script);
            command
        }
        None => std::process::Command::new(&script),
    };
    let status = command
        .args(["run", "wineboot"])
        .env("STEAM_COMPAT_CLIENT_INSTALL_PATH", &proton.path)
        .env("STEAM_COMPAT_DATA_PATH", prefix)
        .env("SteamAppId", appid.to_string())
        .env("SteamGameId", appid.to_string())
        .env("WINEDEBUG", "-all")
        .current_dir(install_dir)
        .status()?;
    if !status.success() || !is_initialized(prefix) {
        return Err(Error::Other(format!(
            "wineboot a rendu {status} sans créer le préfixe"
        )));
    }
    Ok(())
}

/// Supprime un préfixe (opération destructive, confirmée par l'appelant).
pub fn reset(prefix: &Path) -> Result<bool> {
    if !prefix.exists() {
        return Ok(false);
    }
    let text = prefix.to_string_lossy();
    // Le nom peut être numérique (ancien rangement) ou « Jeu (id) » (nouveau
    // CompatData) : dans les deux cas, le dossier doit rester sous un parent
    // connu, jamais à la racine du disque.
    let safe_name = prefix
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| {
            n.chars().all(|c| c.is_ascii_digit())
                || (n.ends_with(')') && n.rsplit_once(" (").is_some())
        })
        .unwrap_or(false);
    let looks_safe = safe_name
        && (text.contains("/compatdata/")
            || text.contains("/CompatData/")
            || text.contains("/prefixes/"))
        && prefix.components().count() >= 4;
    if !looks_safe {
        return Err(Error::Other(format!(
            "refus de supprimer un chemin inattendu : {}",
            prefix.display()
        )));
    }
    std::fs::remove_dir_all(prefix)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game_with_prefix(prefix: Option<PathBuf>) -> Game {
        Game {
            appid: 440,
            prefix,
            ..Default::default()
        }
    }

    #[test]
    fn prefers_existing_steam_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let compat = dir.path().join("compatdata/440");
        std::fs::create_dir_all(&compat).unwrap();
        let chosen = prefix_for(&game_with_prefix(Some(compat.clone())), dir.path());
        assert_eq!(chosen, compat);
    }

    /// Un jeu Steam sans compatdata encore créé le reçoit dans sa
    /// bibliothèque, comme le ferait Steam lui-même.
    #[test]
    fn steam_game_uses_its_library_compatdata() {
        let dir = tempfile::tempdir().unwrap();
        let game = Game {
            appid: 440,
            library: dir.path().join("SteamLibrary"),
            origin: GameOrigin::Steam,
            ..Default::default()
        };
        let chosen = prefix_for(&game, dir.path());
        assert_eq!(
            chosen,
            dir.path().join("SteamLibrary/steamapps/compatdata/440")
        );
    }

    /// Un jeu de boutique range son préfixe dans `CompatData`, sous le nom du
    /// jeu et son identifiant : le dossier reste lisible dans un gestionnaire
    /// de fichiers.
    #[test]
    fn store_game_uses_library_compatdata() {
        let dir = tempfile::tempdir().unwrap();
        let game = Game {
            appid: 3590371856,
            name: "A Short Hike".into(),
            library: dir.path().join("LapinuxLibrary"),
            origin: GameOrigin::Store,
            ..Default::default()
        };
        let chosen = prefix_for(&game, dir.path());
        assert_eq!(
            chosen,
            dir.path().join("LapinuxLibrary/CompatData/A Short Hike (3590371856)")
        );
    }

    #[test]
    fn prefix_name_removes_path_separators() {
        assert_eq!(prefix_name("A/B: C?", 42), "A B  C (42)");
    }

    /// Un titre renommé ne perd pas son préfixe : l'identifiant entre
    /// parenthèses retrouve le dossier existant, même avec l'ancien nom.
    #[test]
    fn a_renamed_game_finds_its_existing_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let library = dir.path().join("LapinuxLibrary");
        let existing = library.join("CompatData/Ancien titre (42)");
        std::fs::create_dir_all(&existing).unwrap();
        let game = Game {
            appid: 42,
            name: "Nouveau titre".into(),
            library,
            origin: GameOrigin::Store,
            ..Default::default()
        };
        assert_eq!(prefix_path(&game, dir.path()), existing);
    }

    /// Sans bibliothèque connue (jeu jamais installé, entrée ancienne), le
    /// préfixe retombe sous les données de Lapinux.
    #[test]
    fn falls_back_to_lapinux_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let chosen = prefix_for(&game_with_prefix(None), dir.path());
        assert!(chosen.ends_with("prefixes/440"));
    }

    /// Un préfixe de l'ancien rangement suit son jeu, liens symboliques
    /// compris.
    #[test]
    fn a_legacy_prefix_moves_into_the_library() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join("prefixes/440");
        std::fs::create_dir_all(legacy.join("pfx/drive_c/windows")).unwrap();
        std::fs::write(legacy.join("pfx/drive_c/windows/system.reg"), b"reg").unwrap();
        std::os::unix::fs::symlink(".", legacy.join("pfx/self")).unwrap();
        let library = dir.path().join("LapinuxLibrary");
        let game = Game {
            appid: 440,
            library: library.clone(),
            origin: GameOrigin::Lapinux,
            ..Default::default()
        };
        let chosen = prefix_for(&game, dir.path());
        assert_eq!(chosen, library.join("CompatData/Jeu (440)"));
        assert!(chosen.join("pfx/drive_c/windows/system.reg").is_file());
        let link = std::fs::read_link(chosen.join("pfx/self")).unwrap();
        assert_eq!(link, std::path::PathBuf::from("."));
        assert!(!legacy.exists());
    }

    #[test]
    fn ensure_creates_marker() {
        let dir = tempfile::tempdir().unwrap();
        let prefix = dir.path().join("prefixes/440");
        ensure(&prefix, 440).unwrap();
        assert!(marker_path(&prefix).is_file());
        assert!(!is_initialized(&prefix));
        std::fs::create_dir_all(prefix.join("pfx")).unwrap();
        assert!(is_initialized(&prefix));
    }

    #[test]
    fn reset_refuses_unexpected_path() {
        let dir = tempfile::tempdir().unwrap();
        let weird = dir.path().join("home/prefix");
        std::fs::create_dir_all(&weird).unwrap();
        assert!(reset(&weird).is_err());
    }

    #[test]
    fn reset_removes_compatdata_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let prefix = dir.path().join("steamapps/compatdata/440");
        std::fs::create_dir_all(prefix.join("pfx")).unwrap();
        assert!(reset(&prefix).unwrap());
        assert!(!prefix.exists());
        assert!(!reset(&prefix).unwrap());
    }
}
