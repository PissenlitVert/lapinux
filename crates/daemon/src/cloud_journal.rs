//! Journal local des synchronisations cloud.
//!
//! Steam ne publie pas le nom de la machine qui a envoyé un fichier. Quand
//! Lapinux envoie une sauvegarde, il note donc lui-même l'heure, la machine et
//! le volume : la fiche peut ensuite dire d'où vient le dernier envoi, même
//! après un redémarrage.

use lapinux_core::model::CloudSyncEntry;
use lapinux_core::paths::LapinuxPaths;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Un jeu n'a pas besoin d'un journal infini : les dernières opérations
/// suffisent à expliquer l'état de la fiche.
const KEEP: usize = 8;
static JOURNAL_LOCK: Mutex<()> = Mutex::new(());

/// Chemin du journal d'un jeu, sous les données de Lapinux.
fn path(paths: &LapinuxPaths, appid: u32) -> PathBuf {
    paths
        .data_dir
        .join("cloud-sync")
        .join(format!("{appid}.json"))
}

/// Lit le journal sans verrou : l'appelant le tient déjà.
fn read(path: &Path) -> Vec<CloudSyncEntry> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Synchronisations connues pour un jeu, la plus récente en premier.
///
/// Un journal absent ou illisible ne fait pas échouer la fiche : il est
/// simplement vide, et la prochaine opération le recréera.
pub fn history(paths: &LapinuxPaths, appid: u32) -> Vec<CloudSyncEntry> {
    let _guard = JOURNAL_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    read(&path(paths, appid))
}

/// Ajoute une synchronisation réussie au journal du jeu.
///
/// L'écriture passe par un fichier temporaire puis un renommage : une fiche
/// lue pendant l'écriture ne voit jamais un JSON à moitié écrit. Un échec est
/// journalisé mais ne remet pas en cause la synchronisation cloud elle-même.
pub fn record(paths: &LapinuxPaths, appid: u32, mut entry: CloudSyncEntry) {
    if entry.files == 0 {
        return;
    }
    if entry.at <= 0 {
        entry.at = lapinux_core::util::now_unix();
    }
    let _guard = JOURNAL_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let file = path(paths, appid);
    let Some(parent) = file.parent() else {
        return;
    };
    if let Err(error) = std::fs::create_dir_all(parent) {
        tracing::warn!("journal cloud : dossier {} inaccessible ({error})", parent.display());
        return;
    }
    let mut entries = read(&file);
    entries.insert(0, entry);
    entries.truncate(KEEP);
    let Ok(bytes) = serde_json::to_vec_pretty(&entries) else {
        return;
    };
    let temporary = file.with_extension("json.tmp");
    if let Err(error) = std::fs::write(&temporary, bytes) {
        tracing::warn!("journal cloud : écriture impossible ({error})");
        return;
    }
    if let Err(error) = std::fs::rename(&temporary, &file) {
        tracing::warn!("journal cloud : remplacement impossible ({error})");
        let _ = std::fs::remove_file(&temporary);
    }
}

/// Nom d'hôte, la même étiquette que celle envoyée dans un lot Steam.
pub fn machine_name() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "lapinux".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lapinux_core::model::CloudSyncDirection;

    fn entry(at: i64, files: usize) -> CloudSyncEntry {
        CloudSyncEntry {
            at,
            direction: CloudSyncDirection::Upload,
            machine: "pc-test".into(),
            files,
            bytes: files as u64 * 10,
        }
    }

    #[test]
    fn the_journal_keeps_the_newest_entries_first() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        for at in 1..=10 {
            record(&paths, 42, entry(at, 1));
        }
        let entries = history(&paths, 42);
        assert_eq!(entries.len(), KEEP);
        assert_eq!(entries[0].at, 10);
        assert_eq!(entries.last().unwrap().at, 3);
    }

    #[test]
    fn an_empty_sync_is_not_written() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        record(&paths, 42, entry(1, 0));
        assert!(history(&paths, 42).is_empty());
    }

    #[test]
    fn a_broken_journal_reads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        let file = path(&paths, 42);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, b"{pas du json").unwrap();
        assert!(history(&paths, 42).is_empty());
    }
}
