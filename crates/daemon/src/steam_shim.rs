//! Pont Steamworks Windows : remplace le `steam_api64.dll` d'un jeu Proton
//! par celui de Lapinux pendant la partie, puis remet l'original.
//!
//! Wine charge le DLL du dossier du jeu avant tout autre : il n'existe pas de
//! préchargement équivalent au `.so` Linux. Le remplacement est donc un choix
//! explicite, activé par `LAPINUX_STEAMSHIM_WIN`, et toujours sauvegardé.
//!
//! Un jeu Unreal ne charge pas le DLL à la racine : le plugin Steamworks le
//! prend dans `Engine/Binaries/ThirdParty/Steamworks/SteamvXXX/Win64/`. Toutes
//! les copies trouvées sous la racine du jeu sont donc remplacées.

use std::path::{Path, PathBuf};

/// Une copie du DLL remplacée, avec son original sauvegardé.
struct ShimFile {
    target: PathBuf,
    backup: PathBuf,
    had_original: bool,
}

/// DLL posés, avec leurs originaux sauvegardés.
pub(crate) struct SteamShimSwap {
    files: Vec<ShimFile>,
}

impl SteamShimSwap {
    /// Remet le jeu exactement dans son état d'origine.
    pub(crate) fn restore(self) {
        // Une seule restauration par sauvegarde : si un autre fil l'a deja
        // consommee, le fichier en place est l'original et ne doit pas etre
        // supprime (sinon le jeu perd son DLL).
        for file in self.files {
            if file.backup.is_file() {
                let _ = std::fs::remove_file(&file.target);
                match std::fs::rename(&file.backup, &file.target) {
                    Ok(()) => tracing::info!(
                        "succès Steamworks : DLL d'origine restauré ({})",
                        file.target.display()
                    ),
                    Err(error) => tracing::warn!(
                        "pont Steamworks : restauration impossible pour {} ({error})",
                        file.target.display()
                    ),
                }
            } else if !file.had_original {
                let _ = std::fs::remove_file(&file.target);
            }
        }
    }
}

/// Écrit la liste des contenus d'atelier installés pour le pont Windows.
/// Le chemin de chaque dossier est converti en `Z:\...`, la racine du système
/// vue par Wine. Le fichier est réécrit à chaque lancement : un contenu
/// désactivé ou supprimé disparaît donc de la liste.
pub(crate) fn write_workshop_manifest(
    data_dir: &Path,
    libraries: &[lapinux_steam::Library],
    appid: u32,
) -> Option<PathBuf> {
    let dir = data_dir.join("workshop");
    if let Err(error) = std::fs::create_dir_all(&dir) {
        tracing::warn!("atelier : dossier de manifeste impossible ({error})");
        return None;
    }
    let path = dir.join(format!("{appid}.txt"));
    let mut items = lapinux_steam::workshop::installed(libraries, appid);
    items.sort_by_key(|item| item.published_file_id);
    let count = items.len();
    let mut body = String::new();
    for item in items {
        let folder = format!("Z:{}", item.path.to_string_lossy().replace('/', "\\"))
            .replace('\t', " ")
            .replace('\n', " ")
            .replace('\r', " ");
        body.push_str(&format!(
            "{}\t{}\t{}\n",
            item.published_file_id, item.size, folder
        ));
    }
    if let Err(error) = std::fs::write(&path, body) {
        tracing::warn!("atelier : écriture du manifeste impossible ({error})");
        return None;
    }
    tracing::info!(
        "atelier : manifeste {} prêt ({} contenu(s))",
        path.display(),
        count
    );
    Some(path)
}

/// Variable explicite, sinon DLL dans le répertoire de données Lapinux
/// (XDG ou LAPINUX_HOME), comme le reste du projet.
fn shim_path(data_dir: &Path, explicit: Option<PathBuf>) -> Option<PathBuf> {
    if let Some(shim) = explicit {
        return shim.is_file().then_some(shim);
    }
    let configured = data_dir.join("steamshim.dll");
    if configured.is_file() {
        return Some(configured);
    }
    // Avant la prise en charge de XDG et LAPINUX_HOME, la DLL était cherchée
    // ici. Continuer à l'utiliser sans déplacer ni effacer l'ancien fichier.
    let historical = lapinux_core::paths::home_dir().join(".local/share/lapinux/steamshim.dll");
    historical.is_file().then_some(historical)
}

/// Le pont doit-il accompagner ce jeu ? Le choix du joueur l'emporte ; sans
/// choix, le pont est posé (plus aucun jeu n'est écarté d'office).
fn shim_wanted(appid: u32, preference: Option<bool>) -> bool {
    preference.unwrap_or(!lapinux_core::model::steam_shim_excluded(appid))
}

/// Remplace les `steam_api64.dll` du jeu par celui de `LAPINUX_STEAMSHIM_WIN`.
/// Sans variable, sans DLL ou pour un jeu écarté, aucun changement n'est fait.
pub(crate) fn install_steam_shim(
    root: &Path,
    data_dir: &Path,
    appid: u32,
    preference: Option<bool>,
) -> Option<SteamShimSwap> {
    if !shim_wanted(appid, preference) {
        tracing::info!("pont Steamworks écarté pour {appid}");
        return None;
    }
    let shim = shim_path(
        data_dir,
        std::env::var_os("LAPINUX_STEAMSHIM_WIN").map(PathBuf::from),
    )?;
    install_steam_shim_with(root, &shim)
}

/// Pose le pont sous `root` en cherchant les DLL Steamworks dans toute
/// l'arborescence du jeu. Sans DLL existant, pose à la racine, comme avant.
fn install_steam_shim_with(root: &Path, shim: &Path) -> Option<SteamShimSwap> {
    if !shim.is_file() {
        tracing::warn!("pont Steamworks : {} introuvable", shim.display());
        return None;
    }
    let shim_bytes = std::fs::read(shim).ok()?;
    let mut targets = find_steam_dlls(root);
    if targets.is_empty() {
        targets.push(root.join("steam_api64.dll"));
    }
    let mut files = Vec::new();
    for target in targets {
        let mut backup_name = target.as_os_str().to_owned();
        backup_name.push(".lapinux-orig");
        let backup = PathBuf::from(backup_name);
        // Un pont est deja pose pour cette copie : ne pas empiler une seconde
        // sauvegarde, qui ferait courir deux restaurations sur le meme fichier.
        // Mais une session precedente a pu etre interrompue (daemon tue) sans
        // restaurer le DLL : reprendre cette pose pour que la fin de la partie
        // remette enfin l'original, au lieu de laisser le pont pour toujours.
        if backup.exists() {
            let target_is_shim = target.is_file()
                && std::fs::read(&target).ok().as_deref() == Some(shim_bytes.as_slice());
            // Une sauvegarde qui traine alors que le DLL du jeu est en place
            // vient d'une restauration deja faite (par la partie suivante ou a
            // la main) : elle est perimee. La prendre pour un pont pose
            // lancerait le jeu sans pont, donc sans Steam, et le jeu refuse de
            // demarrer ("Unable to run Steam version without any connection to
            // a valid instance of Steam").
            let stale_backup = target.is_file() && !target_is_shim;
            if stale_backup {
                let _ = std::fs::remove_file(&backup);
                tracing::warn!(
                    "pont Steamworks : sauvegarde perimee retiree pour {}",
                    backup.display()
                );
            } else {
                if !target.is_file() && std::fs::copy(shim, &target).is_err() {
                    tracing::warn!(
                        "pont Steamworks : copie du DLL impossible pour {}",
                        target.display()
                    );
                    continue;
                }
                tracing::warn!(
                    "pont Steamworks deja pose, reprise de la restauration pour {}",
                    target.display()
                );
                files.push(ShimFile {
                    target,
                    backup,
                    had_original: true,
                });
                continue;
            }
        }
        let had_original = target.is_file();
        if had_original {
            if let Err(error) = std::fs::rename(&target, &backup) {
                tracing::warn!(
                    "pont Steamworks : sauvegarde impossible pour {} ({error})",
                    target.display()
                );
                continue;
            }
        }
        if let Err(error) = std::fs::copy(shim, &target) {
            tracing::warn!(
                "pont Steamworks : copie du DLL impossible pour {} ({error})",
                target.display()
            );
            if had_original {
                let _ = std::fs::rename(&backup, &target);
            }
            continue;
        }
        tracing::info!(
            "succès Steamworks : pont {} posé pour {}",
            shim.display(),
            target.display()
        );
        files.push(ShimFile {
            target,
            backup,
            had_original,
        });
    }
    if files.is_empty() {
        None
    } else {
        Some(SteamShimSwap { files })
    }
}

/// Toutes les copies d'un nom donné sous la racine du jeu. Les dossiers de
/// contenu et de sauvegarde sont ignorés : ils n'en contiennent jamais.
fn find_under_root(root: &Path, wanted: &dyn Fn(&str) -> bool) -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: usize, wanted: &dyn Fn(&str) -> bool, found: &mut Vec<PathBuf>) {
        if depth > 8 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
                if matches!(
                    name.as_str(),
                    "content" | "saved" | "logs" | "cache" | "config"
                ) {
                    continue;
                }
                walk(&entry.path(), depth + 1, wanted, found);
            } else if kind.is_file() && wanted(&entry.file_name().to_string_lossy()) {
                found.push(entry.path());
            }
        }
    }

    let mut found = Vec::new();
    if root.is_dir() {
        walk(root, 0, wanted, &mut found);
    }
    found.sort();
    found.dedup();
    found
}

fn find_steam_dlls(root: &Path) -> Vec<PathBuf> {
    find_under_root(root, &|name| name.eq_ignore_ascii_case("steam_api64.dll"))
}

fn find_steam_backups(root: &Path) -> Vec<PathBuf> {
    find_under_root(root, &|name| {
        name.eq_ignore_ascii_case("steam_api64.dll.lapinux-orig")
    })
}

fn original_for(backup: &Path) -> Option<PathBuf> {
    let name = backup
        .file_name()?
        .to_str()?
        .strip_suffix(".lapinux-orig")?;
    Some(backup.with_file_name(name))
}

/// DLL encore occupés par le pont alors que la sauvegarde est restée en place.
/// Un DLL disparu compte aussi : la sauvegarde est la seule copie restante.
pub(crate) fn stale_shim_targets(root: &Path, data_dir: &Path) -> Vec<PathBuf> {
    let shim = shim_path(data_dir, None).and_then(|path| std::fs::read(path).ok());
    find_steam_backups(root)
        .into_iter()
        .filter_map(|backup| {
            let target = original_for(&backup)?;
            let stale = !target.is_file()
                || shim
                    .as_ref()
                    .is_some_and(|bytes| std::fs::read(&target).ok().as_deref() == Some(bytes));
            stale.then_some(target)
        })
        .collect()
}

/// Remet les DLL d'origine des ponts orphelins. Une cible absente est recréée
/// depuis la sauvegarde ; une cible qui a changé est laissée intacte.
pub(crate) fn repair_stale_shims(root: &Path, data_dir: &Path) -> Result<Vec<PathBuf>, String> {
    let targets = stale_shim_targets(root, data_dir);
    let mut restored = Vec::new();
    for target in targets {
        let mut backup_name = target.as_os_str().to_owned();
        backup_name.push(".lapinux-orig");
        let backup = PathBuf::from(backup_name);
        if !backup.is_file() {
            continue;
        }
        if target.is_file() {
            std::fs::remove_file(&target)
                .map_err(|error| format!("pont Steamworks : {} ({error})", target.display()))?;
        }
        std::fs::rename(&backup, &target)
            .map_err(|error| format!("pont Steamworks : {} ({error})", target.display()))?;
        restored.push(target);
    }
    Ok(restored)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_dll_suit_le_dossier_de_donnees_configure() {
        let dir = tempfile::tempdir().unwrap();
        let default = dir.path().join("steamshim.dll");
        std::fs::write(&default, b"test").unwrap();
        assert_eq!(shim_path(dir.path(), None), Some(default));
        let explicit = dir.path().join("explicit.dll");
        std::fs::write(&explicit, b"test").unwrap();
        assert_eq!(
            shim_path(dir.path(), Some(explicit.clone())),
            Some(explicit)
        );
        assert_eq!(
            shim_path(dir.path(), Some(dir.path().join("missing.dll"))),
            None
        );
    }

    #[test]
    fn le_choix_du_joueur_prime_sur_le_defaut() {
        // Plus aucun jeu n'est écarté d'office : SMS Returns reçoit le pont
        // comme les autres, et le joueur peut toujours le refuser.
        assert!(shim_wanted(4670920, None));
        assert!(shim_wanted(4670920, Some(true)));
        assert!(!shim_wanted(4670920, Some(false)));
        assert!(shim_wanted(480, None));
        assert!(!shim_wanted(480, Some(false)));
    }

    #[test]
    fn le_dll_du_jeu_est_remplace_puis_restaure() {
        let dir = std::env::temp_dir().join(format!("lapinux-swap-racine-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("steam_api64.dll");
        std::fs::write(&target, b"original").unwrap();
        let shim = dir.join("shim.dll");
        std::fs::write(&shim, b"pont").unwrap();

        let swap = install_steam_shim_with(&dir, &shim).expect("pose du pont");
        assert_eq!(std::fs::read(&target).unwrap(), b"pont");
        assert_eq!(
            std::fs::read(dir.join("steam_api64.dll.lapinux-orig")).unwrap(),
            b"original"
        );

        swap.restore();
        assert_eq!(std::fs::read(&target).unwrap(), b"original");
        assert!(!dir.join("steam_api64.dll.lapinux-orig").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn une_pose_interrompue_est_reprise_puis_restauree() {
        // Un daemon tue pendant la partie laisse le pont et sa sauvegarde.
        // Le lancement suivant doit reprendre cette pose, pas l'ignorer pour
        // toujours : sinon le DLL du jeu reste celui de Lapinux hors session.
        let dir = std::env::temp_dir().join(format!("lapinux-swap-reprise-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("steam_api64.dll");
        std::fs::write(&target, b"original").unwrap();
        let shim = dir.join("shim.dll");
        std::fs::write(&shim, b"pont").unwrap();

        // Premiere pose, volontairement non restauree.
        drop(install_steam_shim_with(&dir, &shim).expect("premiere pose"));
        assert_eq!(std::fs::read(&target).unwrap(), b"pont");

        let swap = install_steam_shim_with(&dir, &shim).expect("reprise de la pose");
        assert_eq!(std::fs::read(&target).unwrap(), b"pont");
        swap.restore();
        assert_eq!(std::fs::read(&target).unwrap(), b"original");
        assert!(!dir.join("steam_api64.dll.lapinux-orig").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn une_sauvegarde_perimee_ne_bloque_pas_le_pont() {
        // Une restauration deja faite (a la main, ou par la partie suivante)
        // laisse la sauvegarde sur disque alors que le DLL du jeu est revenu.
        // La prendre pour un pont pose lancerait le jeu sans pont, donc sans
        // Steam : le jeu refuse de demarrer.
        let dir =
            std::env::temp_dir().join(format!("lapinux-swap-perimee-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("steam_api64.dll");
        std::fs::write(&target, b"original").unwrap();
        let backup = dir.join("steam_api64.dll.lapinux-orig");
        std::fs::write(&backup, b"original").unwrap();
        let shim = dir.join("shim.dll");
        std::fs::write(&shim, b"pont").unwrap();

        let swap = install_steam_shim_with(&dir, &shim).expect("pont pose malgre la sauvegarde");
        assert_eq!(std::fs::read(&target).unwrap(), b"pont");
        assert_eq!(std::fs::read(&backup).unwrap(), b"original");

        swap.restore();
        assert_eq!(std::fs::read(&target).unwrap(), b"original");
        assert!(!backup.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn un_pont_orphelin_est_detecte_puis_repare() {
        let dir =
            std::env::temp_dir().join(format!("lapinux-swap-orphelin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("steam_api64.dll");
        std::fs::write(&target, b"original").unwrap();
        let shim = dir.join("steamshim.dll");
        std::fs::write(&shim, b"pont").unwrap();

        drop(install_steam_shim_with(&dir, &shim).expect("pose du pont"));
        assert_eq!(stale_shim_targets(&dir, &dir), vec![target.clone()]);
        let restored = repair_stale_shims(&dir, &dir).expect("reparation");
        assert_eq!(restored, vec![target.clone()]);
        assert_eq!(std::fs::read(&target).unwrap(), b"original");
        assert!(!dir.join("steam_api64.dll.lapinux-orig").exists());
        assert!(stale_shim_targets(&dir, &dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn un_pont_orphelin_sans_dll_est_recree_depuis_la_sauvegarde() {
        let dir =
            std::env::temp_dir().join(format!("lapinux-swap-manquant-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("steam_api64.dll");
        std::fs::write(&target, b"original").unwrap();
        let shim = dir.join("shim.dll");
        std::fs::write(&shim, b"pont").unwrap();

        drop(install_steam_shim_with(&dir, &shim).expect("pose du pont"));
        std::fs::remove_file(&target).unwrap();
        assert_eq!(stale_shim_targets(&dir, &dir), vec![target.clone()]);
        repair_stale_shims(&dir, &dir).expect("reparation");
        assert_eq!(std::fs::read(&target).unwrap(), b"original");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn le_dll_steamworks_d_unreal_est_trouve_sous_la_racine() {
        let root = std::env::temp_dir().join(format!("lapinux-swap-unreal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let nested = root
            .join("Engine")
            .join("Binaries")
            .join("ThirdParty")
            .join("Steamworks")
            .join("Steamv164")
            .join("Win64");
        std::fs::create_dir_all(&nested).unwrap();
        let target = nested.join("steam_api64.dll");
        std::fs::write(&target, b"original unreal").unwrap();
        let shim = root.join("shim.dll");
        std::fs::write(&shim, b"pont").unwrap();

        let swap = install_steam_shim_with(&root, &shim).expect("pose du pont");
        assert_eq!(std::fs::read(&target).unwrap(), b"pont");
        // Le DLL de la racine ne doit pas être créé en double.
        assert!(!root.join("steam_api64.dll").exists());

        swap.restore();
        assert_eq!(std::fs::read(&target).unwrap(), b"original unreal");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn le_manifeste_atelier_convertit_les_chemins_windows() {
        let dir = tempfile::tempdir().unwrap();
        let library_root = dir.path().join("LapinuxLibrary");
        let content = library_root.join("steamapps/workshop/content/42/100");
        std::fs::create_dir_all(&content).unwrap();
        std::fs::write(content.join("map.sav"), b"abcde").unwrap();
        let data = dir.path().join("data");
        let libraries = [lapinux_steam::Library {
            root: library_root.clone(),
            managed: true,
        }];
        let manifest = write_workshop_manifest(&data, &libraries, 42).expect("manifeste");
        let body = std::fs::read_to_string(manifest).unwrap();
        let fields: Vec<&str> = body.trim().split('\t').collect();
        assert_eq!(fields[0], "100");
        assert_eq!(fields[1], "5");
        assert_eq!(
            fields[2],
            format!("Z:{}", content.to_string_lossy().replace('/', "\\"))
        );
    }
}
