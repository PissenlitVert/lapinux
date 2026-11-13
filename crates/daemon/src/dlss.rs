//! Mise à jour des bibliothèques DLSS d'un jeu, avec retour possible.
//!
//! Les Protons installés ne remplacent pas les DLL DLSS d'un jeu :
//! `PROTON_ENABLE_NGX_UPDATER` n'y est pas implémenté, et rien n'est
//! téléchargé. Lapinux le fait donc lui-même, à partir des bibliothèques déjà
//! présentes sur la machine (autres jeux, dossier `umu` d'un préfixe), en
//! gardant l'original pour « revenir aux DLL d'origine ».

use std::path::{Path, PathBuf};

use lapinux_core::model::{DlssLibrary as Library, DlssState, Game};

const NAMES: [&str; 3] = ["nvngx_dlss.dll", "nvngx_dlssg.dll", "nvngx_dlssd.dll"];
/// Un jeu peut contenir des dizaines de milliers de fichiers : la recherche
/// s'arrête bien avant. Les plugins Unreal rangent leurs DLL à sept niveaux.
const MAX_ENTRIES: usize = 6_000;
const MAX_DEPTH: usize = 8;

fn folder(data_dir: &Path, appid: u32) -> PathBuf {
    data_dir.join("dlss").join(appid.to_string())
}

/// Une sauvegarde existe : le jeu tourne sur des DLL remplacées.
pub(crate) fn has_backup(data_dir: &Path, appid: u32) -> bool {
    folder(data_dir, appid).is_dir()
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

/// Bibliothèques DLSS des chemins trouvés, avec la provenance à afficher.
fn libraries(files: Vec<PathBuf>, source: &str) -> Vec<Library> {
    files
        .into_iter()
        .map(|path| Library {
            name: file_name(&path),
            version: crate::steam::dll_version(&path),
            path,
            source: source.to_string(),
        })
        .collect()
}

/// Bibliothèques DLSS du dossier du jeu.
fn in_game(root: &Path) -> Vec<Library> {
    let mut files = Vec::new();
    collect(root, 0, &mut files, &mut 0);
    libraries(files, "")
}

/// Bibliothèques DLSS du préfixe Proton du jeu : c'est la version que Proton
/// lui fournit. Les Protons installés n'y déposent que leur `nvngx.dll`, mais
/// un jeu installé côté Windows ou `umu` peut y laisser des DLL DLSS.
fn in_prefix(data_dir: &Path, game: &Game) -> Vec<Library> {
    // Un jeu natif n'a pas de préfixe ; et `prefix_for` déplace un ancien
    // rangement, ce qu'une simple lecture ne doit pas déclencher.
    if game.native {
        return Vec::new();
    }
    let prefix = lapinux_proton::prefix_for(game, data_dir);
    let mut files = Vec::new();
    for root in [
        prefix.join("drive_c/windows/system32"),
        prefix.join("pfx/drive_c/windows/system32"),
    ] {
        if root.is_dir() {
            collect(&root, 0, &mut files, &mut 0);
        }
    }
    libraries(files, "préfixe Proton")
}

fn collect(dir: &Path, depth: usize, files: &mut Vec<PathBuf>, seen: &mut usize) {
    if depth > MAX_DEPTH || *seen > MAX_ENTRIES {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        *seen += 1;
        if *seen > MAX_ENTRIES {
            return;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_dir() {
            // Les préfixes Proton rangent des milliers de fichiers qui ne
            // disent rien du jeu lui-même.
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if !name.starts_with("prefix") && name != "drive_c" && name != "pfx" {
                collect(&path, depth + 1, files, seen);
            }
        } else if NAMES
            .iter()
            .any(|wanted| entry.file_name().to_string_lossy().eq_ignore_ascii_case(wanted))
        {
            files.push(path);
        }
    }
}

/// Une seule bibliothèque par nom : la plus ancienne (pour dire ce que le jeu
/// a réellement) ou la plus récente (pour remplacer). Un jeu peut embarquer
/// plusieurs copies d'une même DLL, dans deux plugins Unreal par exemple.
fn per_name(libraries: Vec<Library>, newest: bool) -> Vec<Library> {
    let mut kept: Vec<Library> = Vec::new();
    for library in libraries {
        match kept.iter_mut().find(|other| other.name == library.name) {
            Some(other) => {
                if (library.version > other.version) == newest {
                    *other = library;
                }
            }
            None => kept.push(library),
        }
    }
    kept
}

/// La bibliothèque la plus récente pour ce nom, si elle dépasse `current`.
/// Le pool ne garde qu'une entrée par nom : inutile de comparer plusieurs
/// candidates du même nom.
fn newest(candidates: &[Library], current: &Library) -> Option<Library> {
    candidates
        .iter()
        .find(|found| found.name == current.name && found.version > current.version)
        .cloned()
}

/// Nom de sauvegarde d'une DLL : le chemin relatif au jeu, aplati. Deux
/// copies de la même DLL dans un jeu ne se recouvrent donc pas.
fn backup_name(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace(['/', '\\'], "_")
}

impl crate::Shared {
    /// Jeux installés qui peuvent fournir une bibliothèque plus récente. Un
    /// jeu natif en possède rarement, mais rien n'oblige à l'écarter.
    fn dlss_games(&self) -> Vec<Game> {
        self.store
            .load_games()
            .unwrap_or_default()
            .into_iter()
            .filter(|game| game.installed)
            .collect()
    }

    /// Le jeu de cette requête, ou un message lisible.
    fn dlss_game(&self, appid: u32) -> Result<Game, String> {
        self.store
            .game(appid)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "jeu introuvable".to_string())
    }

    /// La bibliothèque la plus récente de chaque nom trouvée sur la machine :
    /// les autres jeux, le préfixe Proton de celui-ci et ses propres copies.
    /// Une DLL peut ainsi en remplacer une autre du même jeu.
    fn dlss_pool(&self, game: &Game) -> Vec<Library> {
        let mut pool = in_game(&game.install_dir());
        for found in &mut pool {
            found.source = "ce jeu".into();
        }
        for other in self.dlss_games() {
            if other.appid == game.appid {
                continue;
            }
            for mut found in in_game(&other.install_dir()) {
                found.source = other.name.clone();
                pool.push(found);
            }
        }
        pool.extend(in_prefix(&self.paths.data_dir, game));
        per_name(pool, true)
    }

    /// État des bibliothèques DLSS de ce jeu : une entrée par nom de DLL, la
    /// plus ancienne copie étant celle qui dit s'il y a quelque chose à
    /// mettre à jour.
    pub(crate) fn dlss_state(&self, appid: u32) -> Result<DlssState, String> {
        let game = self.dlss_game(appid)?;
        let root = game.install_dir();
        if !root.is_dir() {
            return Err("dossier du jeu inaccessible".into());
        }
        let current = per_name(in_game(&root), false);
        let pool = self.dlss_pool(&game);
        let available = current
            .iter()
            .filter_map(|library| newest(&pool, library))
            .collect();
        Ok(DlssState {
            current,
            available,
            updated: has_backup(&self.paths.data_dir, appid),
            // Un jeu protégé garde ses DLL d'origine tant que le joueur n'a
            // pas autorisé explicitement les injections.
            blocked: game
                .settings
                .injections_blocked(self.anticheat_protected(&game)),
        })
    }

    /// Remplace chaque copie du jeu par la version la plus récente trouvée, en
    /// gardant les originales. Rend l'état d'après.
    pub(crate) fn dlss_update(&self, appid: u32) -> Result<DlssState, String> {
        let game = self.dlss_game(appid)?;
        if game
            .settings
            .injections_blocked(self.anticheat_protected(&game))
        {
            return Err(
                "jeu protégé par un anti-triche : autorise les injecteurs de ce jeu avant de remplacer ses DLL"
                    .into(),
            );
        }
        let root = game.install_dir();
        let pool = self.dlss_pool(&game);
        let dir = folder(&self.paths.data_dir, appid);
        // Chaque copie présente dans le jeu est traitée séparément : Unreal
        // embarque parfois deux versions de la même DLL dans ses plugins.
        for library in in_game(&root) {
            let Some(candidate) = newest(&pool, &library) else {
                continue;
            };
            if candidate.path == library.path {
                continue;
            }
            std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
            let backup = dir.join(backup_name(&root, &library.path));
            if !backup.exists() {
                std::fs::copy(&library.path, &backup).map_err(|error| error.to_string())?;
            }
            std::fs::copy(&candidate.path, &library.path).map_err(|error| error.to_string())?;
            tracing::info!(
                "DLSS du jeu {appid} : {} {} → {} (depuis {})",
                library.name,
                library.version_label(),
                candidate.version_label(),
                candidate.source
            );
        }
        self.dlss_state(appid)
    }

    /// Un jeu protégé par un anti-triche démarre sur ses DLL d'origine : un
    /// remplacement laissé en place est annulé avant le lancement, sauf si le
    /// joueur a explicitement autorisé les injections. Rend vrai quand des
    /// bibliothèques ont été remises.
    pub(crate) fn dlss_restore_if_blocked(&self, game: &Game, protected: bool) -> bool {
        if !game.settings.injections_blocked(protected)
            || !has_backup(&self.paths.data_dir, game.appid)
        {
            return false;
        }
        match self.dlss_restore(game.appid) {
            Ok(_) => true,
            Err(error) => {
                tracing::warn!(
                    "DLSS du jeu {} : bibliothèques d'origine non remises ({error})",
                    game.appid
                );
                false
            }
        }
    }

    /// Remet les DLL d'origine et oublie la sauvegarde.
    pub(crate) fn dlss_restore(&self, appid: u32) -> Result<DlssState, String> {
        let game = self.dlss_game(appid)?;
        let dir = folder(&self.paths.data_dir, appid);
        if dir.is_dir() {
            let root = game.install_dir();
            for library in in_game(&root) {
                let backup = dir.join(backup_name(&root, &library.path));
                if backup.is_file() {
                    std::fs::copy(&backup, &library.path).map_err(|error| error.to_string())?;
                }
            }
            let _ = std::fs::remove_dir_all(&dir);
            tracing::info!("DLSS du jeu {appid} : bibliothèques d'origine remises");
        }
        self.dlss_state(appid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_version_s_affiche_sans_le_quatrieme_nombre_nul() {
        let library = Library {
            name: "nvngx_dlss.dll".into(),
            version: Some([3, 7, 10, 0]),
            path: PathBuf::from("/tmp/x"),
            source: String::new(),
        };
        assert_eq!(library.version_label(), "3.7.10");
        let library = Library {
            version: Some([3, 1, 2, 4]),
            ..library
        };
        assert_eq!(library.version_label(), "3.1.2.4");
        let library = Library {
            version: None,
            ..library
        };
        assert_eq!(library.version_label(), "version inconnue");
    }

    #[test]
    fn la_version_la_plus_recente_est_retenue() {
        let current = Library {
            name: "nvngx_dlss.dll".into(),
            version: Some([2, 4, 0, 0]),
            path: PathBuf::from("/jeu/bin/x64/nvngx_dlss.dll"),
            source: String::new(),
        };
        let older = Library {
            version: Some([2, 1, 0, 0]),
            source: "vieux jeu".into(),
            ..current.clone()
        };
        let newer = Library {
            version: Some([3, 7, 10, 0]),
            source: "préfixe Proton".into(),
            ..current.clone()
        };
        let best = newest(&[older, newer], &current).expect("une version plus récente");
        assert_eq!(best.version_label(), "3.7.10");
        assert_eq!(best.source, "préfixe Proton");

        // Même version ou plus ancienne : rien à proposer, et une autre DLL ne
        // remplace pas celle-ci.
        let same = Library {
            version: Some([2, 4, 0, 0]),
            ..current.clone()
        };
        let other = Library {
            name: "nvngx_dlssg.dll".into(),
            version: Some([9, 9, 9, 0]),
            ..current.clone()
        };
        assert!(newest(&[same, other], &current).is_none());
    }

    #[test]
    fn une_seule_entree_par_nom_de_dll() {
        let library = |name: &str, version: [u16; 3], path: &str| Library {
            name: name.into(),
            version: Some([version[0], version[1], version[2], 0]),
            path: PathBuf::from(path),
            source: String::new(),
        };
        let copies = vec![
            library("nvngx_dlss.dll", [3, 7, 10], "/a/nvngx_dlss.dll"),
            library("nvngx_dlss.dll", [3, 1, 2], "/b/nvngx_dlss.dll"),
            library("nvngx_dlssg.dll", [3, 1, 1], "/a/nvngx_dlssg.dll"),
        ];
        let oldest = per_name(copies.clone(), false);
        assert_eq!(oldest.len(), 2);
        assert_eq!(oldest[0].path, PathBuf::from("/b/nvngx_dlss.dll"));
        let newest = per_name(copies, true);
        assert_eq!(newest.len(), 2);
        assert_eq!(newest[0].path, PathBuf::from("/a/nvngx_dlss.dll"));
    }

    #[test]
    fn un_jeu_protege_demarre_avec_ses_dll_d_origine() {
        let dir = tempfile::tempdir().unwrap();
        let paths = lapinux_core::paths::LapinuxPaths::under(dir.path());
        paths.ensure().unwrap();
        let root = dir.path().join("library/steamapps/common/Test");
        std::fs::create_dir_all(root.join("bin/x64")).unwrap();
        let dll = root.join("bin/x64/nvngx_dlss.dll");
        std::fs::write(&dll, b"officielle").unwrap();
        let game = Game {
            appid: 42,
            installdir: "Test".into(),
            library: dir.path().join("library"),
            installed: true,
            ..Default::default()
        };
        let shared = crate::Shared::new(&paths).unwrap();
        shared.store.save_game(&game).unwrap();
        // Une mise à jour avait été appliquée : l'originale est sauvegardée.
        let backups = folder(&paths.data_dir, 42);
        std::fs::create_dir_all(&backups).unwrap();
        std::fs::write(
            backups.join(backup_name(&root, &dll)),
            b"officielle",
        )
        .unwrap();
        std::fs::write(&dll, b"remplacee").unwrap();

        assert!(has_backup(&paths.data_dir, 42));
        assert!(shared.dlss_restore_if_blocked(&game, true));
        assert_eq!(std::fs::read(&dll).unwrap(), b"officielle");
        assert!(!has_backup(&paths.data_dir, 42));

        // Un accord explicite laisse le remplacement en place.
        let mut allowed = game.clone();
        allowed.settings.allow_protected_injections = Some(true);
        std::fs::create_dir_all(&backups).unwrap();
        std::fs::write(backups.join(backup_name(&root, &dll)), b"officielle").unwrap();
        std::fs::write(&dll, b"remplacee").unwrap();
        assert!(!shared.dlss_restore_if_blocked(&allowed, true));
        assert_eq!(std::fs::read(&dll).unwrap(), b"remplacee");
    }

    #[test]
    fn la_recherche_s_arrete_aux_dll_dlss() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("bin/x64")).unwrap();
        std::fs::write(root.path().join("bin/x64/nvngx_dlss.dll"), b"x").unwrap();
        std::fs::write(root.path().join("bin/x64/nvngx_dlssg.dll"), b"x").unwrap();
        std::fs::write(root.path().join("bin/x64/autre.dll"), b"x").unwrap();
        let found = in_game(root.path());
        assert_eq!(found.len(), 2);
        assert!(found
            .iter()
            .all(|library| NAMES.contains(&library.name.as_str())));
    }
}
