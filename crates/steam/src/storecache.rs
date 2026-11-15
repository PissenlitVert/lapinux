//! Cache disque des réponses publiques de la boutique : paquets de licences et
//! fiches d'application. Sans lui, chaque synchronisation redemandait les mêmes
//! paquets muets de PICS — 271 appels pour rien, la plupart inchangés depuis des
//! années.
//!
//! Seules les réponses **exploitables** sont écrites : une réponse vide peut
//! être un refus d'Edge autant qu'un paquet inconnu, et la figer pour un mois
//! laisserait un trou. Un échec réseau relit l'entrée périmée, si elle existe :
//! une panne de la boutique ne vide pas la bibliothèque.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Durée de validité d'une réponse. Le nom d'un paquet et le type d'une
/// application ne changent qu'exceptionnellement.
const TTL: i64 = 30 * 86_400;

/// Ce que la boutique dit d'un paquet : son nom et les applications qu'il
/// porte. Les deux passes (noms, puis appids) partagent l'entrée.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct PackageEntry {
    pub at: i64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub apps: Vec<(u32, String)>,
}

/// Nom, type et DLC publiés d'une application, tels que la boutique les
/// publie. `dlc_ids` reste `None` pour une entrée écrite avant ce relevé :
/// elle sera relue une fois, puis les DLC vides seront mémorisés eux aussi.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct AppEntry {
    pub at: i64,
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub dlc_ids: Option<Vec<u32>>,
}

/// Compteurs du dernier passage : servis par le cache, demandés au réseau.
static PACKAGE_HITS: AtomicU64 = AtomicU64::new(0);
static PACKAGE_FETCHES: AtomicU64 = AtomicU64::new(0);
static APP_HITS: AtomicU64 = AtomicU64::new(0);
static APP_FETCHES: AtomicU64 = AtomicU64::new(0);

pub(crate) fn note_package_hit() {
    PACKAGE_HITS.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn note_package_fetch() {
    PACKAGE_FETCHES.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn note_app_hit() {
    APP_HITS.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn note_app_fetch() {
    APP_FETCHES.fetch_add(1, Ordering::Relaxed);
}

/// Paquets servis par le cache et paquets demandés au réseau depuis le dernier
/// relevé : c'est la mesure qui dit si le cache fait son travail.
pub(crate) fn take_package_stats() -> (u64, u64) {
    (
        PACKAGE_HITS.swap(0, Ordering::Relaxed),
        PACKAGE_FETCHES.swap(0, Ordering::Relaxed),
    )
}

pub(crate) fn take_app_stats() -> (u64, u64) {
    (
        APP_HITS.swap(0, Ordering::Relaxed),
        APP_FETCHES.swap(0, Ordering::Relaxed),
    )
}

/// Dossier du cache : `cache/steam-store`, à côté des fiches de jeu.
fn dir() -> &'static Path {
    static DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        lapinux_core::paths::LapinuxPaths::discover()
            .cache_dir
            .join("steam-store")
    })
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

fn read<T: serde::de::DeserializeOwned>(file: &Path) -> Option<T> {
    let bytes = std::fs::read(file).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn write<T: serde::Serialize>(file: PathBuf, value: &T) {
    let Some(parent) = file.parent() else {
        return;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let temp = file.with_extension(format!("tmp.{}", std::process::id()));
    let Ok(bytes) = serde_json::to_vec(value) else {
        return;
    };
    if std::fs::write(&temp, bytes).is_ok() {
        let _ = std::fs::rename(&temp, &file);
    }
}

fn fresh<T>(file: &Path) -> Option<T>
where
    T: serde::de::DeserializeOwned + HasStamp,
{
    let entry: T = read(file)?;
    (now() - entry.stamp() <= TTL).then_some(entry)
}

/// Une entrée porte sa date d'écriture.
trait HasStamp {
    fn stamp(&self) -> i64;
}

impl HasStamp for PackageEntry {
    fn stamp(&self) -> i64 {
        self.at
    }
}

impl HasStamp for AppEntry {
    fn stamp(&self) -> i64 {
        self.at
    }
}

fn package_path(dir: &Path, package: u32) -> PathBuf {
    dir.join(format!("paquet-{package}.json"))
}

fn app_path(dir: &Path, appid: u32) -> PathBuf {
    dir.join(format!("app-{appid}.json"))
}

/// Entrée de paquet encore valable, si elle existe.
pub(crate) fn fresh_package(package: u32) -> Option<PackageEntry> {
    fresh(&package_path(dir(), package))
}

/// Entrée de paquet, même périmée : elle sert quand la boutique ne répond pas.
pub(crate) fn package(package: u32) -> Option<PackageEntry> {
    read(&package_path(dir(), package))
}

/// Garde les applications d'un paquet. Une réponse vide n'est pas écrite.
pub(crate) fn save_package_apps(package: u32, apps: Vec<(u32, String)>) {
    if apps.is_empty() {
        return;
    }
    let mut entry: PackageEntry = read(&package_path(dir(), package)).unwrap_or_default();
    entry.at = now();
    entry.apps = apps;
    write(package_path(dir(), package), &entry);
}

/// Garde le nom d'un paquet, sans effacer les applications déjà connues.
pub(crate) fn save_package_name(package: u32, name: &str) {
    if name.trim().is_empty() {
        return;
    }
    let mut entry: PackageEntry = read(&package_path(dir(), package)).unwrap_or_default();
    entry.at = now();
    entry.name = Some(name.to_string());
    write(package_path(dir(), package), &entry);
}

/// Fiche d'application encore valable.
pub(crate) fn fresh_app(appid: u32) -> Option<AppEntry> {
    fresh(&app_path(dir(), appid))
}

/// Fiche d'application, même périmée : dernier recours hors ligne.
pub(crate) fn app(appid: u32) -> Option<AppEntry> {
    read(&app_path(dir(), appid))
}

/// Garde le nom, le type et les DLC publiés d'une application.
pub(crate) fn save_app(appid: u32, name: &str, kind: &str, dlc_ids: &[u32]) {
    if name.trim().is_empty() {
        return;
    }
    write(
        app_path(dir(), appid),
        &AppEntry {
            at: now(),
            name: name.to_string(),
            kind: kind.to_string(),
            dlc_ids: Some(dlc_ids.to_vec()),
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lapinux-store-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn un_paquet_ecrit_se_relit_et_se_perime() {
        let dir = temp_dir("paquet");
        let file = package_path(&dir, 4242);
        let entry = PackageEntry {
            at: now(),
            name: Some("Lot (WW)".into()),
            apps: vec![(10, "Dix".into()), (20, "Vingt".into())],
        };
        write(file.clone(), &entry);
        assert_eq!(read::<PackageEntry>(&file), Some(entry.clone()));
        assert_eq!(fresh::<PackageEntry>(&file), Some(entry.clone()));

        // Une entrée plus vieille que le délai n'est plus servie fraîche, mais
        // reste lisible comme secours.
        let old = PackageEntry {
            at: now() - TTL - 60,
            ..entry.clone()
        };
        write(file.clone(), &old);
        assert_eq!(fresh::<PackageEntry>(&file), None);
        assert_eq!(read::<PackageEntry>(&file), Some(old));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn une_fiche_d_application_se_relit() {
        let dir = temp_dir("app");
        let file = app_path(&dir, 220);
        let entry = AppEntry {
            at: now(),
            name: "Half-Life 2".into(),
            kind: "game".into(),
            dlc_ids: Some(vec![420, 421]),
        };
        write(file.clone(), &entry);
        assert_eq!(read::<AppEntry>(&file), Some(entry.clone()));
        assert_eq!(fresh::<AppEntry>(&file), Some(entry));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn une_reponse_vide_n_est_pas_ecrite() {
        // Un refus d'Edge ne doit pas figer un paquet pour un mois : on ne
        // garde que ce qui porte un nom ou des applications.
        let dir = temp_dir("vide");
        write(package_path(&dir, 7), &PackageEntry::default());
        assert_eq!(fresh::<PackageEntry>(&package_path(&dir, 7)), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
