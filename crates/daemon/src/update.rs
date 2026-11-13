//! Mises à jour de Lapinux : dernière release GitHub, téléchargement suivi et
//! remplacement de l'AppImage courante.

use crate::Shared;
use lapinux_core::update::{self, UpdateProgress, UpdateStage, UpdateStatus};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

const USER_AGENT: &str = "lapinux";
const API_TIMEOUT: Duration = Duration::from_secs(20);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(900);
const CHUNK: usize = 64 * 1024;

/// Ce que l'interface demande de télécharger : l'asset choisi dans la release.
pub struct Download {
    pub tag: String,
    pub name: String,
    pub url: String,
    pub size: u64,
    pub sha256: Option<String>,
}

pub fn check() -> Result<UpdateStatus, String> {
    let repository = update::repository();
    let url = format!("https://api.github.com/repos/{repository}/releases/latest");
    let body = ureq::get(&url)
        .set("User-Agent", USER_AGENT)
        .set("Accept", "application/vnd.github+json")
        .timeout(API_TIMEOUT)
        .call()
        .map_err(|error| format!("GitHub : {error}"))?
        .into_string()
        .map_err(|error| format!("GitHub : {error}"))?;
    let release = update::parse_release(&body)?;
    Ok(update::status(
        lapinux_core::VERSION,
        update::BUILD_DATE,
        release,
    ))
}

/// Lance le téléchargement en arrière-plan ; la progression est relue par
/// `UpdateState`. Un seul téléchargement tourne à la fois : un second appel est
/// ignoré, et une AppImage déjà complète n'est pas retéléchargée.
pub fn download(shared: &Arc<Shared>, download: Download) -> Result<(), String> {
    if !download.url.starts_with("https://")
        || !download.name.to_ascii_lowercase().ends_with(".appimage")
        || download.tag.trim().is_empty()
    {
        return Err("fichier de mise à jour refusé".to_string());
    }
    if shared.update_busy.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    let job = match prepare(shared, &download) {
        Ok(Some(job)) => job,
        Ok(None) => {
            shared.update_busy.store(false, Ordering::SeqCst);
            return Ok(());
        }
        Err(error) => {
            shared.update_busy.store(false, Ordering::SeqCst);
            return Err(error);
        }
    };
    let shared = Arc::clone(shared);
    std::thread::spawn(move || {
        let _busy = Busy(&shared.update_busy);
        let result = fetch(&shared, &download, &job);
        if let Err(message) = result {
            let _ = std::fs::remove_file(&job.partial);
            publish(
                &shared,
                Some(UpdateProgress {
                    tag: download.tag,
                    stage: UpdateStage::Failed,
                    message,
                    ..Default::default()
                }),
            );
        }
    });
    Ok(())
}

/// Rend la place même si le téléchargement panique.
struct Busy<'a>(&'a std::sync::atomic::AtomicBool);

impl Drop for Busy<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Fichier partiel et destination finale du téléchargement.
struct Job {
    partial: PathBuf,
    target: PathBuf,
}

/// Prépare le dossier de cache et annonce l'état initial. Renvoie `None` quand
/// le fichier complet est déjà là.
fn prepare(shared: &Arc<Shared>, download: &Download) -> Result<Option<Job>, String> {
    let directory = shared
        .paths
        .updates_dir()
        .join(safe_component(&download.tag));
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("dossier de mise à jour : {error}"))?;
    let target = directory.join(safe_component(&download.name));
    let complete = download.size > 0
        && std::fs::metadata(&target).is_ok_and(|metadata| metadata.len() == download.size);
    if complete {
        publish(
            shared,
            Some(UpdateProgress {
                tag: download.tag.clone(),
                stage: UpdateStage::Ready,
                received: download.size,
                total: download.size,
                message: String::new(),
                path: Some(target.display().to_string()),
            }),
        );
        return Ok(None);
    }
    publish(
        shared,
        Some(UpdateProgress {
            tag: download.tag.clone(),
            stage: UpdateStage::Downloading,
            received: 0,
            total: download.size,
            message: String::new(),
            path: None,
        }),
    );
    Ok(Some(Job {
        partial: directory.join(".download.part"),
        target,
    }))
}

fn fetch(shared: &Arc<Shared>, download: &Download, job: &Job) -> Result<(), String> {
    let response = ureq::get(&download.url)
        .set("User-Agent", USER_AGENT)
        .timeout(DOWNLOAD_TIMEOUT)
        .call()
        .map_err(|error| format!("téléchargement : {error}"))?;
    let total = response
        .header("Content-Length")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(download.size);
    let mut file = File::create(&job.partial).map_err(|error| format!("écriture : {error}"))?;
    let mut reader = response.into_reader();
    let mut hasher = Sha256::new();
    let mut received = 0_u64;
    let mut buffer = [0_u8; CHUNK];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| format!("téléchargement : {error}"))?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read])
            .map_err(|error| format!("écriture : {error}"))?;
        hasher.update(&buffer[..read]);
        received += read as u64;
        advance(shared, received, total);
    }
    file.sync_all().map_err(|error| format!("écriture : {error}"))?;
    drop(file);

    if let Some(expected) = &download.sha256 {
        publish(
            shared,
            Some(UpdateProgress {
                tag: download.tag.clone(),
                stage: UpdateStage::Verifying,
                received,
                total,
                message: String::new(),
                path: None,
            }),
        );
        let found = format!("{:x}", hasher.finalize());
        if !found.eq_ignore_ascii_case(expected) {
            return Err(format!(
                "empreinte incorrecte : attendu {expected}, obtenu {found}"
            ));
        }
    }

    std::fs::rename(&job.partial, &job.target)
        .map_err(|error| format!("fichier reçu : {error}"))?;
    publish(
        shared,
        Some(UpdateProgress {
            tag: download.tag.clone(),
            stage: UpdateStage::Ready,
            received,
            total,
            message: String::new(),
            path: Some(job.target.display().to_string()),
        }),
    );
    Ok(())
}

/// Remplace l'AppImage qui fait tourner Lapinux. Le fichier est écrit à côté de
/// la cible puis renommé : le processus en cours garde l'ancien inode, et une
/// copie interrompue ne casse pas l'installation.
pub fn install(shared: &Arc<Shared>) -> Result<String, String> {
    if shared.update_busy.swap(true, Ordering::SeqCst) {
        return Err("une mise à jour est déjà en cours".to_string());
    }
    let _busy = Busy(&shared.update_busy);
    install_locked(shared)
}

fn install_locked(shared: &Arc<Shared>) -> Result<String, String> {
    let progress = shared
        .update_progress
        .lock()
        .map_err(|_| "état de mise à jour illisible".to_string())?
        .clone()
        .ok_or_else(|| "aucune mise à jour téléchargée".to_string())?;
    if progress.stage != UpdateStage::Ready {
        return Err("le téléchargement n'est pas terminé".to_string());
    }
    let source = progress
        .path
        .clone()
        .ok_or_else(|| "fichier de mise à jour absent".to_string())?;
    let appimage = std::env::var("APPIMAGE").map_err(|_| not_an_appimage())?;
    if appimage.trim().is_empty() {
        return Err(not_an_appimage());
    }
    let target = PathBuf::from(&appimage);
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "lapinux.AppImage".to_string());
    let staged = target.with_file_name(format!(".{name}.new"));
    std::fs::copy(&source, &staged).map_err(|error| format!("copie de l'AppImage : {error}"))?;
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(&staged)
        .map_err(|error| format!("AppImage reçue : {error}"))?
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&staged, permissions)
        .map_err(|error| format!("AppImage reçue : {error}"))?;
    std::fs::rename(&staged, &target)
        .map_err(|error| format!("remplacement de l'AppImage : {error}"))?;
    // Le fichier est consommé : rouvrir la page ne repropose pas de l'installer.
    publish(shared, None);
    Ok(progress.tag)
}

fn not_an_appimage() -> String {
    "Lapinux ne tourne pas depuis une AppImage".to_string()
}

fn publish(shared: &Arc<Shared>, value: Option<UpdateProgress>) {
    if let Ok(mut slot) = shared.update_progress.lock() {
        *slot = value;
    }
}

fn advance(shared: &Arc<Shared>, received: u64, total: u64) {
    if let Ok(mut slot) = shared.update_progress.lock() {
        if let Some(current) = slot.as_mut() {
            current.received = received;
            current.total = total;
        }
    }
}

/// Un tag ou un nom de fichier ne doit pas sortir du dossier de cache : ni
/// séparateur, ni composant `..`.
fn safe_component(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_' | '+' | '~') {
                character
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim_start_matches('.');
    if trimmed.is_empty() {
        "_".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::safe_component;

    #[test]
    fn a_tag_cannot_escape_the_cache_directory() {
        assert_eq!(safe_component(".."), "_");
        assert_eq!(safe_component("../x"), "_x");
        assert_eq!(safe_component("/etc"), "_etc");
    }

    #[test]
    fn real_asset_names_are_kept() {
        assert_eq!(safe_component("v1.2.3"), "v1.2.3");
        assert_eq!(
            safe_component("Lapinux-0.2.0-x86_64.AppImage"),
            "Lapinux-0.2.0-x86_64.AppImage"
        );
    }
}
