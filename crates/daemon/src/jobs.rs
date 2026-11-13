//! Téléchargements (jeux, Proton, outils) et réglages persistés.

use crate::{Result, Shared};
use lapinux_core::i18n::{t, t_args};
use lapinux_core::config::Config;
use lapinux_core::ipc::Envelope;
use lapinux_core::model::{GameSettings, Job, JobKind, JobState};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

struct AfmfRelease {
    version: &'static str,
    url: &'static str,
    size: u64,
    archive_hash: &'static str,
    hashes: [&'static str; 5],
    installed_manifest_hash: &'static str,
}

fn afmf_release(version: &str) -> Option<AfmfRelease> {
    let (version, url, size, archive_hash, library, manifest, installed_manifest_hash, notice) = match version {
        "1.3.1" => ("1.3.1", "https://github.com/serialexperimentslainnnn/afmf-linux/releases/download/v1.3.1/afmf-linux-1.3.1-x86_64.tar.xz", 91064, "7fafa09021ee94dc5ec8da694cb02fe298b54c114b486611bb24b1e1500edb18", "7e16de98040859988119c2836f7f777be35c575568187ba41ef38dfd871a0394", "e396bdeba35649e8a182120abaaa6b17027cb5b7ab6d60d03f7cd0409569fd2a", "b727d2e12a5241946d55b66d95319f70e462f658fdd79d4aa7827f3f1948f316", "deb1917d589dc54bfb1cede259ba963596129f332333165ba73f95d9b0d4442f"),
        "1.3.0" => ("1.3.0", "https://github.com/serialexperimentslainnnn/afmf-linux/releases/download/v1.3.0/afmf-linux-1.3.0-x86_64.tar.xz", 88492, "96e394fc46c5628fe6573fb46a4898c0675e7bf92aa7a048b7acd877c82b0364", "381fafd5f63471b6e74afa9cd64dfa2932ef5c5834f59da63eadfae6db38f4af", "30bb480723cfba6dc82fb35ee690560c1339035711b1ca067730c5279b11c3ef", "caea4e2832b71b296e3f39bada6cdc15caf7200ebd1ea3316b0a9554b57db236", "a552ec91b60df157eb0f91ff0e988266ab0a18f7d82ee44575a12a85cc3a62aa"),
        _ => return None,
    };
    Some(AfmfRelease {
        version,
        url,
        size,
        archive_hash,
        hashes: [
            library,
            manifest,
            "72c6d7a9b6ff04f3f58cc37c4e27f101b378eb42f5509b33fae8fe3feba32554",
            "82cf74fc23885107c7f69249ac3f1dac55898ff68320eb2c753984cae6b2d78a",
            notice,
        ],
        installed_manifest_hash,
    })
}

/// Archive publiée par le projet, empreinte épinglée : pas de script distant.
fn install_afmf(
    release: &AfmfRelease,
    destination: &std::path::Path,
    cancelled: &AtomicBool,
    mut progress: impl FnMut(u64),
) -> std::result::Result<(), String> {
    use std::io::Read;
    let response = ureq::get(release.url)
        .set("User-Agent", "Lapinux")
        .timeout(std::time::Duration::from_secs(90))
        .call()
        .map_err(|e| format!("afmf-linux download: {e}"))?;
    let mut reader = response.into_reader();
    let mut bytes = Vec::new();
    let mut buf = [0u8; 16384];
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let count = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        if bytes.len() + count > 2 * 1024 * 1024 {
            return Err("archive too large".into());
        }
        bytes.extend_from_slice(&buf[..count]);
        progress(bytes.len() as u64);
    }
    install_afmf_archive(
        &bytes,
        destination,
        cancelled,
        release.archive_hash,
        release.hashes,
        release.version,
    )
}

/// Accepte uniquement l'archive épinglée, sans extraire de chemins fournis par celle-ci.
fn install_afmf_archive(
    bytes: &[u8],
    destination: &std::path::Path,
    cancelled: &AtomicBool,
    expected_hash: &str,
    file_hashes: [&str; 5],
    version: &str,
) -> std::result::Result<(), String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let hash = format!("{:x}", Sha256::digest(bytes));
    if hash != expected_hash {
        return Err("afmf-linux SHA-256 mismatch".into());
    }
    let mut archive = tar::Archive::new(xz2::read::XzDecoder::new(bytes));
    let mut library = None;
    let mut manifest = None;
    let mut license = None;
    let mut fidelity_license = None;
    let mut fidelity_notice = None;
    for entry in archive.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path().map_err(|e| e.to_string())?.into_owned();
        let prefix = format!("afmf-linux-{version}/");
        let target = match path.to_str().and_then(|p| p.strip_prefix(&prefix)) {
            Some("libafmf-linux.so") => &mut library,
            Some("afmf-linux.json") => &mut manifest,
            Some("LICENSE") => &mut license,
            Some("LICENSE.fidelityfx.txt") => &mut fidelity_license,
            Some("NOTICE.fidelityfx.md") => &mut fidelity_notice,
            _ => continue,
        };
        if !entry.header().entry_type().is_file()
            || target.is_some()
            || entry.size() > 4 * 1024 * 1024
        {
            return Err("invalid afmf-linux archive entry".into());
        }
        let mut body = Vec::new();
        entry.read_to_end(&mut body).map_err(|e| e.to_string())?;
        *target = Some(body);
    }
    let library = library.ok_or("afmf-linux library missing")?;
    let manifest = manifest.ok_or("afmf-linux manifest missing")?;
    let license = license.ok_or("afmf-linux license missing")?;
    let fidelity_license = fidelity_license.ok_or("FidelityFX license missing")?;
    let fidelity_notice = fidelity_notice.ok_or("FidelityFX notice missing")?;
    if [
        &library,
        &manifest,
        &license,
        &fidelity_license,
        &fidelity_notice,
    ]
    .into_iter()
    .zip(file_hashes)
    .any(|(file, expected)| format!("{:x}", Sha256::digest(file)) != expected)
    {
        return Err("afmf-linux extracted file SHA-256 mismatch".into());
    }
    let parent = destination.parent().ok_or("invalid destination")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let stage = parent.join(format!(
        ".stage-{}-{}",
        std::process::id(),
        lapinux_core::util::now_unix()
    ));
    std::fs::create_dir(&stage).map_err(|e| e.to_string())?;
    let result = (|| {
        std::fs::write(stage.join("libafmf-linux.so"), library).map_err(|e| e.to_string())?;
        std::fs::write(stage.join("LICENSE"), license).map_err(|e| e.to_string())?;
        std::fs::write(stage.join("LICENSE.fidelityfx.txt"), fidelity_license)
            .map_err(|e| e.to_string())?;
        std::fs::write(stage.join("NOTICE.fidelityfx.md"), fidelity_notice)
            .map_err(|e| e.to_string())?;
        // Vulkan interprète un chemin contenant '/' relativement au manifeste.
        let manifest = String::from_utf8(manifest)
            .map_err(|e| e.to_string())?
            .replace(
                "\"library_path\": \"libafmf-linux.so\"",
                "\"library_path\": \"./libafmf-linux.so\"",
            );
        std::fs::write(stage.join("afmf-linux.json"), manifest).map_err(|e| e.to_string())?;
        if cancelled.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        if destination.exists() {
            return Err("version already exists".into());
        }
        std::fs::rename(&stage, destination).map_err(|e| e.to_string())?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&stage);
    }
    result
}

/// Vérifie les deux fichiers exécutés : une installation altérée ou partielle
/// ne doit jamais être chargée par Vulkan.
pub(crate) fn validate_afmf_runtime(
    dir: &std::path::Path,
    version: &str,
) -> std::result::Result<(), String> {
    if version == lapinux_core::config::AFMF_LOCAL_VERSION {
        return validate_local_afmf(dir);
    }
    let release = afmf_release(version).ok_or("version afmf-linux non prise en charge")?;
    for (name, hash, limit) in [
        ("libafmf-linux.so", release.hashes[0], 4 * 1024 * 1024),
        ("afmf-linux.json", release.installed_manifest_hash, 65536),
    ] {
        verify_afmf_file(dir, name, hash, limit)
            .map_err(|error| format!("{error} (version {version})"))?;
    }
    Ok(())
}

fn verify_afmf_file(
    dir: &std::path::Path,
    name: &str,
    hash: &str,
    limit: u64,
) -> std::result::Result<(), String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let path = dir.join(name);
    let meta = std::fs::symlink_metadata(&path)
        .map_err(|_| format!("afmf-linux : {name} absent"))?;
    if !meta.file_type().is_file() || meta.len() > limit {
        return Err(format!("afmf-linux : fichier invalide {name}"));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(&path)
        .map_err(|e| e.to_string())?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit || format!("{:x}", Sha256::digest(bytes)) != hash {
        return Err(format!(
            "afmf-linux : empreinte incorrecte pour {name} ; fichier conservé"
        ));
    }
    Ok(())
}

/// Manifeste écrit à côté d'une couche construite sur cette machine :
/// `{"files": {"libafmf-linux.so": "<sha256>", ...}}`. Il ne prouve pas une
/// provenance publiée, seulement que les fichiers chargés sont ceux qui ont
/// été construits et installés ici.
#[derive(serde::Deserialize)]
struct AfmfLocalManifest {
    files: std::collections::BTreeMap<String, String>,
}

fn validate_local_afmf(dir: &std::path::Path) -> std::result::Result<(), String> {
    let path = dir.join("lapinux-local.json");
    let text = std::fs::read_to_string(&path).map_err(|_| {
        "afmf-linux : couche locale absente ; construis-la avec scripts/build-afmf-lapinux.sh"
            .to_string()
    })?;
    let manifest: AfmfLocalManifest = serde_json::from_str(&text)
        .map_err(|_| "afmf-linux : manifeste local illisible".to_string())?;
    for name in ["libafmf-linux.so", "afmf-linux.json"] {
        if !manifest.files.contains_key(name) {
            return Err(format!("afmf-linux : manifeste local sans {name}"));
        }
    }
    for (name, hash) in &manifest.files {
        if name.contains('/') || name.starts_with('.') || hash.len() != 64 {
            return Err(format!("afmf-linux : entrée invalide dans le manifeste local ({name})"));
        }
        verify_afmf_file(dir, name, hash, 8 * 1024 * 1024)?;
    }
    Ok(())
}

/// Retrait conservateur : seuls les cinq fichiers connus et inchangés sont
/// supprimés. Une modification ou un fichier ajouté bloque tout le retrait.
fn remove_verified_afmf(
    destination: &std::path::Path,
    hashes: [(&str, &str); 5],
) -> std::result::Result<(), String> {
    use sha2::{Digest, Sha256};
    let meta = std::fs::symlink_metadata(destination).map_err(|e| e.to_string())?;
    if !meta.file_type().is_dir() {
        return Err("afmf-linux: dossier invalide".into());
    }
    let mut count = 0;
    for entry in std::fs::read_dir(destination).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        let expected = hashes.iter().find(|(key, _)| name == *key).ok_or_else(|| {
            format!(
                "afmf-linux: fichier inconnu conservé : {}",
                name.to_string_lossy()
            )
        })?;
        if !entry.file_type().map_err(|e| e.to_string())?.is_file() {
            return Err(format!(
                "afmf-linux: fichier non ordinaire : {}",
                name.to_string_lossy()
            ));
        }
        let mut f = std::fs::File::open(entry.path()).map_err(|e| e.to_string())?;
        use std::io::Read;
        let mut data = Vec::new();
        f.by_ref()
            .take(4 * 1024 * 1024 + 1)
            .read_to_end(&mut data)
            .map_err(|e| e.to_string())?;
        if data.len() > 4 * 1024 * 1024 || format!("{:x}", Sha256::digest(&data)) != expected.1 {
            return Err(format!(
                "afmf-linux: fichier modifié conservé : {}",
                name.to_string_lossy()
            ));
        }
        count += 1;
    }
    if count != hashes.len() {
        return Err("afmf-linux: installation incomplète ; fichiers conservés".into());
    }
    for (name, _) in hashes {
        std::fs::remove_file(destination.join(name)).map_err(|e| e.to_string())?;
    }
    std::fs::remove_dir(destination).map_err(|e| e.to_string())
}

impl Shared {
    pub fn remove_frame_generation(&self, version: &str) -> Result<Envelope> {
        let _guard = self
            .save_operation
            .try_lock()
            .map_err(|_| lapinux_core::Error::Other(t("downloads.err_operation_busy").into()))?;
        let release = afmf_release(version).ok_or_else(|| {
            lapinux_core::Error::Other("version afmf-linux non prise en charge".into())
        })?;
        if self.session_info().is_some() {
            return Err(lapinux_core::Error::Other(
                "ferme le jeu avant de retirer afmf-linux".into(),
            ));
        }
        if self
            .jobs
            .lock()
            .map_err(|_| lapinux_core::Error::Other("tâches indisponibles".into()))?
            .iter()
            .any(|job| job.kind == JobKind::InstallGraphics && job.state == JobState::Running)
        {
            return Err(lapinux_core::Error::Other(
                "installation graphique en cours".into(),
            ));
        }
        let path = self
            .paths
            .tools_dir
            .join("frame-generation/afmf-linux")
            .join(version);
        remove_verified_afmf(
            &path,
            [
                ("libafmf-linux.so", release.hashes[0]),
                ("afmf-linux.json", release.installed_manifest_hash),
                ("LICENSE", release.hashes[2]),
                ("LICENSE.fidelityfx.txt", release.hashes[3]),
                ("NOTICE.fidelityfx.md", release.hashes[4]),
            ],
        )
        .map_err(lapinux_core::Error::Other)?;
        Ok(Envelope::ok(&serde_json::json!({"removed": version})))
    }

    pub fn save_settings(&self, appid: u32, settings: GameSettings) -> Result<Envelope> {
        // La fin d'une partie relit et réécrit aussi le jeu (temps de jeu).
        // Sérialiser les deux opérations évite d'écraser les nouveaux réglages
        // avec l'ancienne copie, notamment le choix du pont Steamworks.
        let _guard = self
            .save_operation
            .lock()
            .map_err(|_| lapinux_core::Error::Other("verrou de sauvegarde indisponible".into()))?;
        let mut game = self
            .store
            .game(appid)
            .map_err(|e| lapinux_core::Error::Other(e.to_string()))?
            .ok_or_else(|| lapinux_core::Error::Other(t("downloads.err_game_not_found").into()))?;
        game.settings = settings;
        self.store
            .save_game(&game)
            .map_err(|e| lapinux_core::Error::Other(e.to_string()))?;
        Ok(Envelope::ok(&game.settings))
    }

    /// Retient le volume choisi pour un jeu sans toucher au reste de ses
    /// réglages. Le menu rapide l'appelle à chaque geste : la valeur est
    /// arrondie au centième pour qu'un même niveau n'écrive pas deux fois, et
    /// la même sérialisation que les autres écritures est prise pour ne pas
    /// écraser un réglage enregistré au même instant.
    pub fn remember_volume(&self, appid: u32, volume: f32) -> Result<Envelope> {
        let _guard = self
            .save_operation
            .lock()
            .map_err(|_| lapinux_core::Error::Other("verrou de sauvegarde indisponible".into()))?;
        let mut game = self
            .store
            .game(appid)
            .map_err(|e| lapinux_core::Error::Other(e.to_string()))?
            .ok_or_else(|| lapinux_core::Error::Other(t("downloads.err_game_not_found").into()))?;
        let rounded = (volume.clamp(0.0, 1.0) * 100.0).round() / 100.0;
        if game.settings.volume == Some(rounded) {
            return Ok(Envelope::ok(&serde_json::json!({ "volume": rounded })));
        }
        game.settings.volume = Some(rounded);
        self.store
            .save_game(&game)
            .map_err(|e| lapinux_core::Error::Other(e.to_string()))?;
        Ok(Envelope::ok(&serde_json::json!({ "volume": rounded })))
    }

    pub fn save_config(&self, config: Config) -> Result<Envelope> {
        let config = config.normalized();
        let previous = self.config_snapshot();
        let previous_lang = self.config_snapshot().general.lang;
        config
            .save(&self.paths.config_file)
            .map_err(|e| lapinux_core::Error::Other(e.to_string()))?;
        if let Ok(mut guard) = self.config.lock() {
            *guard = config.clone();
        }
        // Le tray lit ses libellés dans la langue globale : il doit suivre
        // un changement fait depuis les réglages sans redémarrer le daemon.
        lapinux_core::i18n::set(&config.general.lang);
        // La langue des jeux choisit les dépôts de doublage : les lectures
        // suivantes doivent la connaître sans redémarrer le daemon.
        crate::apply_game_language(&config);
        // Les limiteurs existants changent de rythme immédiatement, sans
        // interrompre le téléchargement en cours.
        self.throttle.apply(&config);
        // La présence publiée sur Steam suit le réglage sans redémarrer quoi
        // que ce soit.
        self.sync_social();
        // Description et annonces d'une boutique sont écrites dans la langue
        // choisie : les fiches gardées ne valent plus.
        if previous_lang != config.general.lang {
            self.forget_store_sheets();
        }
        // Une durée de conservation raccourcie s'applique tout de suite, sans
        // attendre la prochaine ouverture du profil.
        if previous.general.activity_retention_days != config.general.activity_retention_days {
            self.prune_activity_history();
        }
        Ok(Envelope::ok(&config))
    }

    /// Installe une version figée et vérifiée dans le répertoire privé de Lapinux.
    pub fn install_frame_generation(self: &Arc<Self>, version: String) -> Result<Envelope> {
        let _guard = self
            .save_operation
            .try_lock()
            .map_err(|_| lapinux_core::Error::Other(t("downloads.err_operation_busy").into()))?;
        let release = afmf_release(&version)
            .ok_or_else(|| lapinux_core::Error::Other("Unsupported afmf-linux version".into()))?;
        if std::env::consts::ARCH != "x86_64" {
            return Err(lapinux_core::Error::Other(
                "Unsupported afmf-linux version or architecture".into(),
            ));
        }
        let destination = self
            .paths
            .tools_dir
            .join("frame-generation/afmf-linux")
            .join(&version);
        if destination.exists() {
            return Err(lapinux_core::Error::Other(
                "afmf-linux version already exists".into(),
            ));
        }
        let id = format!(
            "graphics-afmf-{}-{}",
            version,
            lapinux_core::util::now_unix()
        );
        let job = Job {
            id: id.clone(),
            kind: JobKind::InstallGraphics,
            label: format!("afmf-linux {version}"),
            title: format!("afmf-linux {version}"),
            appid: None,
            state: JobState::Running,
            progress: 0.0,
            downloaded: 0,
            total: release.size,
            network: 0,
            written: 0,
            server: "GitHub".into(),
            depot: 0,
            depots: 0,
            updating: false,
            dlc: Vec::new(),
            platform: None,
            message: t("downloads.job_downloading_verifying").into(),
            backend: "GitHub".into(),
            started: lapinux_core::util::now_unix(),
            finished: None,
            report: None,
        };
        let cancelled = Arc::new(AtomicBool::new(false));
        {
            let mut jobs = self
                .jobs
                .lock()
                .map_err(|_| lapinux_core::Error::Other("tâches indisponibles".into()))?;
            if jobs
                .iter()
                .any(|j| j.kind == JobKind::InstallGraphics && j.state == JobState::Running)
            {
                return Err(lapinux_core::Error::Other(
                    "graphics installation already running".into(),
                ));
            }
            let mut map = self
                .cancel
                .lock()
                .map_err(|_| lapinux_core::Error::Other("annulation indisponible".into()))?;
            map.insert(id.clone(), Arc::clone(&cancelled));
            jobs.push(job);
        }
        self.persist_jobs(true);
        let shared = Arc::clone(self);
        let job_id = id.clone();
        std::thread::spawn(move || {
            let result = install_afmf(&release, &destination, &cancelled, |done| {
                shared.update_job(&job_id, |job| {
                    job.downloaded = done;
                    job.network = done;
                    job.progress = (done as f32 / release.size as f32).min(1.0);
                });
            });
            shared.update_job(&job_id, |job| {
                job.finished = Some(lapinux_core::util::now_unix());
                match result {
                    Ok(()) => {
                        job.state = JobState::Done;
                        job.progress = 1.0;
                        job.message = t_args("downloads.job_afmf_installed", &[("version", &release.version)]);
                    }
                    Err(ref err) => {
                        job.state = if cancelled.load(Ordering::Relaxed) {
                            JobState::Cancelled
                        } else {
                            JobState::Failed
                        };
                        job.message = err.clone();
                    }
                }
            });
            if let Ok(mut map) = shared.cancel.lock() {
                map.remove(&job_id);
            }
        });
        Ok(Envelope::ok(&serde_json::json!({ "job": id })))
    }

    pub fn install_proton(
        self: &Arc<Self>,
        repo: String,
        asset: String,
        checksum_url: Option<String>,
    ) -> Result<Envelope> {
        let releases = lapinux_proton::fetch_releases(&repo)
            .map_err(|e| lapinux_core::Error::Other(e.to_string()))?;
        let release = releases
            .into_iter()
            .find(|r| r.asset == asset)
            .ok_or_else(|| lapinux_core::Error::Other(t_args("downloads.err_asset_missing", &[("asset", &asset.to_string())])))?;
        let destination = proton_install_dir(self);
        let job_id = format!(
            "proton-{}-{}",
            release.asset,
            lapinux_core::util::now_unix()
        );
        let job = Job {
            id: job_id.clone(),
            kind: JobKind::InstallProton,
            label: release.asset.clone(),
            title: release.asset.clone(),
            appid: None,
            state: JobState::Running,
            progress: 0.0,
            downloaded: 0,
            total: release.size,
            network: 0,
            written: 0,
            server: "GitHub".into(),
            depot: 0,
            depots: 0,
            updating: false,
            dlc: Vec::new(),
            platform: None,
            message: t("downloads.job_downloading").into(),
            backend: "GitHub".into(),
            started: lapinux_core::util::now_unix(),
            finished: None,
            report: None,
        };
        if let Ok(mut jobs) = self.jobs.lock() {
            jobs.push(job);
        }
        self.persist_jobs(true);
        let cancel = Arc::new(AtomicBool::new(false));
        if let Ok(mut map) = self.cancel.lock() {
            map.insert(job_id.clone(), Arc::clone(&cancel));
        }
        let shared = Arc::clone(self);
        let job_for_thread = job_id.clone();
        let checksum = checksum_url.or(release.checksum_url.clone());
        std::thread::spawn(move || {
            let mut last_update = 0u64;
            let result = lapinux_proton::install_from_url(
                &release.asset_url,
                checksum.as_deref(),
                &destination,
                &mut |done, total| {
                    if cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    if done - last_update < 4 * 1024 * 1024 && done != total.unwrap_or(0) {
                        return;
                    }
                    last_update = done;
                    shared.update_job(&job_for_thread, |job| {
                        job.downloaded = done;
                        job.network = done;
                        job.written = done;
                        if let Some(total) = total {
                            job.total = total;
                            job.progress = (done as f32 / total as f32).clamp(0.0, 1.0);
                        }
                        job.message = t("downloads.job_downloading_verifying").into();
                    });
                },
            );
            match result {
                Ok(path) => {
                    let _ = shared.scan_protons();
                    // Un nouveau build remplace les anciens de sa famille :
                    // c'est ce qu'on attend d'un « latest », et deux versions
                    // du même Proton ne restent pas côte à côte.
                    let replaced = replace_older_protons(&shared, &path);
                    shared.update_job(&job_for_thread, |job| {
                        job.state = JobState::Done;
                        job.progress = 1.0;
                        job.finished = Some(lapinux_core::util::now_unix());
                        job.message = if replaced.is_empty() {
                            t_args(
                                "downloads.job_installed_path",
                                &[("path", &path.display().to_string())],
                            )
                        } else {
                            t_args(
                                "downloads.job_installed_replaced",
                                &[
                                    ("path", &path.display().to_string()),
                                    ("count", &replaced.len().to_string()),
                                    ("names", &replaced.join(", ")),
                                ],
                            )
                        };
                    });
                }
                Err(err) => {
                    let cancelled = cancel.load(Ordering::Relaxed);
                    shared.update_job(&job_for_thread, |job| {
                        job.state = if cancelled {
                            JobState::Cancelled
                        } else {
                            JobState::Failed
                        };
                        job.finished = Some(lapinux_core::util::now_unix());
                        job.message = err.to_string();
                    });
                }
            }
            if let Ok(mut map) = shared.cancel.lock() {
                map.remove(&job_for_thread);
            }
        });
        Ok(Envelope::ok(&serde_json::json!({ "job": job_id })))
    }

    pub fn remove_proton(&self, name: &str) -> Result<Envelope> {
        let protons = self.scan_protons()?;
        let proton = protons
            .iter()
            .find(|p| p.name == name)
            .ok_or_else(|| lapinux_core::Error::Other(t_args("downloads.err_proton_missing", &[("name", &name)])))?;
        // Tout build trouvé sur le disque est retirable, y compris ceux que
        // Steam a posés : le client le retélécharge s'il en a besoin. Le
        // chemin vient de la liste scannée, jamais de l'appelant.
        let path = proton.path.clone();
        std::fs::remove_dir_all(&path).map_err(lapinux_core::Error::Io)?;
        tracing::info!("Proton supprimé : {}", path.display());
        let protons = self.scan_protons()?;
        Ok(Envelope::ok(&protons))
    }

    /// Annule un téléchargement et **retire sa carte de la liste** : une tâche
    /// annulée n'a rien à faire dans « Terminées », elle n'a pas abouti. Les
    /// fichiers déjà reçus d'une installation neuve sont supprimés par le
    /// moteur ; l'installation existante d'une mise à jour reste intacte.
    /// Reste dans la liste uniquement ce que le redémarrage a interrompu, et
    /// qui peut être repris d'un clic.
    pub fn cancel_job(self: &Arc<Self>, id: &str) -> Result<Envelope> {
        let known = self
            .jobs
            .lock()
            .ok()
            .and_then(|jobs| jobs.iter().find(|job| job.id == id).cloned());
        let Some(job) = known else {
            return Err(lapinux_core::Error::Other(t("downloads.err_task_unknown").into()));
        };
        if matches!(
            job.state,
            JobState::Done | JobState::Failed | JobState::Cancelled
        ) {
            return Err(lapinux_core::Error::Other(
                t("downloads.err_task_finished").into(),
            ));
        }
        let flag = self.cancel.lock().ok().and_then(|map| map.get(id).cloned());
        if let Some(flag) = flag {
            // Le moteur s'arrête en quelques dixièmes de seconde : on lui dit
            // de s'arrêter, puis la carte disparaît tout de suite.
            flag.store(true, Ordering::Relaxed);
            lapinux_steam::download_log::note(
                &self.paths.log_dir.join("downloads.log"),
                format!("annulation demandée par l'interface · tâche {id}"),
            );
            self.forget_job(id);
            return Ok(Envelope::ok(
                &serde_json::json!({ "message": "annulation demandée" }),
            ));
        }
        // Une tâche encore dans la file n'a pas de drapeau : on la retire ici
        // pour ne pas la laisser démarrer après coup.
        if matches!(job.state, JobState::Queued | JobState::Interrupted) {
            self.forget_job(id);
            self.pump_install_queue();
            return Ok(Envelope::ok(
                &serde_json::json!({ "message": "tâche retirée" }),
            ));
        }
        Err(lapinux_core::Error::Other(t("downloads.err_task_unknown").into()))
    }

    /// Retire une tâche de la liste et enregistre la liste : la carte
    /// disparaît de l'interface et ne revient pas après un redémarrage. Le
    /// moteur s'arrête de son côté en quelques dixièmes de seconde, une fois
    /// le drapeau d'annulation posé, et nettoie ses fichiers partiels.
    fn forget_job(&self, id: &str) {
        if let Ok(mut jobs) = self.jobs.lock() {
            jobs.retain(|job| job.id != id);
        }
        self.persist_jobs(true);
    }

    /// Supprime de l'historique les tâches terminées ou échouées. Les
    /// téléchargements actifs, la file et les transferts en pause restent
    /// intacts : une tâche `Interrupted` (arrêtée par un redémarrage) est
    /// encore à reprendre, elle vit dans « En pause » et non dans
    /// « Terminées ». La suppression est persistée : un redémarrage ne fait
    /// pas revenir les anciennes.
    pub fn clear_history(&self) -> Result<Envelope> {
        let mut jobs = self
            .jobs
            .lock()
            .map_err(|_| lapinux_core::Error::Other("tâches indisponibles".into()))?;
        let before = jobs.len();
        jobs.retain(|job| {
            matches!(
                job.state,
                JobState::Running | JobState::Queued | JobState::Paused | JobState::Interrupted
            )
        });
        let removed = before - jobs.len();
        drop(jobs);
        self.persist_jobs(true);
        Ok(Envelope::ok(&serde_json::json!({ "removed": removed })))
    }

    /// Retire un téléchargement terminé de l'historique. Un transfert actif,
    /// en pause ou interrompu reste dans la liste : il a encore une
    /// progression à reprendre. Les fichiers installés ne sont jamais touchés.
    pub fn delete_job(&self, id: &str) -> Result<Envelope> {
        let job = self
            .jobs
            .lock()
            .map_err(|_| lapinux_core::Error::Other("tâches indisponibles".into()))?
            .iter()
            .find(|job| job.id == id)
            .cloned()
            .ok_or_else(|| lapinux_core::Error::Other(t("downloads.err_task_unknown").into()))?;
        if !matches!(
            job.state,
            JobState::Done | JobState::Failed | JobState::Cancelled
        ) {
            return Err(lapinux_core::Error::Other(
                t("downloads.err_download_unfinished").into(),
            ));
        }
        self.forget_job(id);
        Ok(Envelope::ok(&serde_json::json!({ "deleted": true })))
    }

    /// Applique l'ordre choisi par glisser-déposer. En mode « un seul jeu à
    /// la fois », une carte en attente passée devant le téléchargement en
    /// cours lui prend le créneau : celui-ci est mis en pause (fichiers
    /// conservés, « Reprendre » le relance au même point). En mode simultané,
    /// la file repart simplement.
    pub fn apply_install_priority(self: &Arc<Self>) {
        if !self.config_snapshot().downloads.parallel_installs {
            while let Some(id) = self.install_losing_the_slot() {
                self.set_aside_running(&id, t("downloads.job_paused_other_ahead"));
            }
        }
        self.pump_install_queue();
    }

    /// Le téléchargement en cours qu'une carte en attente dépasse dans
    /// l'ordre. `None` quand la file respecte déjà la priorité affichée.
    pub(crate) fn install_losing_the_slot(&self) -> Option<String> {
        let jobs = self.jobs.lock().ok()?;
        let queued = jobs
            .iter()
            .position(|job| job.kind == JobKind::InstallGame && job.state == JobState::Queued)?;
        jobs.iter()
            .skip(queued + 1)
            .find(|job| job.kind == JobKind::InstallGame && job.state == JobState::Running)
            .map(|job| job.id.clone())
    }

    /// Met un téléchargement en pause sans rien perdre : le drapeau est lu par
    /// le moteur, les chunks en vol se terminent, les fichiers déjà reçus
    /// restent sur le disque.
    pub fn set_aside_running(&self, id: &str, reason: &str) {
        if let Ok(map) = self.pause.lock() {
            if let Some(flag) = map.get(id) {
                flag.store(true, Ordering::Relaxed);
            }
        }
        lapinux_steam::download_log::note(
            &self.paths.log_dir.join("downloads.log"),
            format!("priorité : tâche {id} mise de côté ({reason})"),
        );
        let mut network = None;
        if let Ok(mut jobs) = self.jobs.lock() {
            if let Some(job) = jobs.iter_mut().find(|job| job.id == id) {
                if job.state == JobState::Running {
                    job.state = JobState::Paused;
                    job.message = reason.to_string();
                    network = Some(job.network);
                }
            }
        }
        if let Some(network) = network {
            self.reset_rate_window(id, network);
        }
        self.persist_jobs(true);
    }

    /// Place les tâches fournies en tête, dans leur ordre, puis laisse les
    /// autres derrière. La file d'attente lit ce même ordre : la première
    /// carte visible est la prochaine à partir.
    pub fn reorder_jobs(&self, ids: Vec<String>) -> Result<Envelope> {
        if ids.is_empty() {
            return Err(lapinux_core::Error::Other(
                "aucune tâche à réordonner".into(),
            ));
        }
        let mut jobs = self
            .jobs
            .lock()
            .map_err(|_| lapinux_core::Error::Other("tâches indisponibles".into()))?;
        let mut ordered = Vec::with_capacity(jobs.len());
        for id in ids {
            if let Some(position) = jobs.iter().position(|job| job.id == id) {
                ordered.push(jobs.remove(position));
            }
        }
        ordered.append(&mut jobs);
        *jobs = ordered;
        drop(jobs);
        self.persist_jobs(true);
        Ok(Envelope::ok(&serde_json::json!({ "reordered": true })))
    }

    /// Met un téléchargement en pause ou le reprend. Le drapeau est lu par le
    /// moteur toutes les 150 ms ; les chunks déjà en vol se terminent, rien
    /// n'est perdu et la reprise repart du même point.
    pub fn pause_job(&self, id: &str, paused: bool) -> Result<Envelope> {
        let flag = self.pause.lock().ok().and_then(|map| map.get(id).cloned());
        let Some(flag) = flag else {
            return Err(lapinux_core::Error::Other(t("downloads.err_task_unknown").into()));
        };
        // Reprendre un jeu dans le mode « un seul à la fois » : il prend le
        // créneau, les autres téléchargements en cours lui laissent la place.
        if !paused && !self.config_snapshot().downloads.parallel_installs {
            let others: Vec<String> = self
                .jobs
                .lock()
                .map(|jobs| {
                    jobs.iter()
                        .filter(|job| {
                            job.id != id
                                && job.kind == JobKind::InstallGame
                                && job.state == JobState::Running
                        })
                        .map(|job| job.id.clone())
                        .collect()
                })
                .unwrap_or_default();
            for other in others {
                self.set_aside_running(&other, t("downloads.job_paused_other_slot"));
            }
        }
        flag.store(paused, Ordering::Relaxed);
        lapinux_steam::download_log::note(
            &self.paths.log_dir.join("downloads.log"),
            format!(
                "{} demandée par l'interface · tâche {id}",
                if paused { "pause" } else { "reprise" }
            ),
        );
        let mut network = None;
        if let Ok(mut jobs) = self.jobs.lock() {
            if let Some(job) = jobs.iter_mut().find(|job| job.id == id) {
                if matches!(job.state, JobState::Running | JobState::Paused) {
                    job.state = if paused {
                        JobState::Paused
                    } else {
                        JobState::Running
                    };
                    job.message = if paused {
                        t("downloads.paused").to_string()
                    } else {
                        t("downloads.job_resumed").to_string()
                    };
                    network = Some(job.network);
                }
            }
        }
        if let Some(network) = network {
            self.reset_rate_window(id, network);
        }
        self.persist_jobs(true);
        Ok(Envelope::ok(&serde_json::json!({ "paused": paused })))
    }
}

/// Retire les autres builds de la même famille que celui qui vient d'être
/// installé, dans le même dossier. Rend les noms retirés : la tâche les
/// annonce au lieu de les faire disparaître en silence.
fn replace_older_protons(shared: &Shared, installed: &std::path::Path) -> Vec<String> {
    let mut removed = Vec::new();
    for proton in older_protons(installed, &shared.scan_protons().unwrap_or_default()) {
        if std::fs::remove_dir_all(&proton.path).is_ok() {
            tracing::info!("ancien Proton retiré : {}", proton.name);
            removed.push(proton.name);
        }
    }
    if !removed.is_empty() {
        let _ = shared.scan_protons();
    }
    removed
}

/// Builds de la même famille que `installed`, dans son dossier : ce qu'un
/// nouveau build rend caducs. La règle est séparée de l'effacement pour être
/// vérifiable sans toucher au disque.
fn older_protons(
    installed: &std::path::Path,
    protons: &[lapinux_core::model::ProtonInfo],
) -> Vec<lapinux_core::model::ProtonInfo> {
    let Some(dir) = installed.parent() else {
        return Vec::new();
    };
    let Some(name) = installed
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
    else {
        return Vec::new();
    };
    let family = lapinux_core::util::proton_family_key(&name);
    if family.is_empty() {
        return Vec::new();
    }
    protons
        .iter()
        .filter(|proton| proton.path != installed)
        .filter(|proton| proton.path.parent() == Some(dir))
        .filter(|proton| lapinux_core::util::proton_family_key(&proton.name) == family)
        .cloned()
        .collect()
}

/// Dossier où Lapinux pose les Proton qu'il télécharge : le sien, jamais
/// celui de Steam. Un build installé ici ne disparaît pas avec Steam et ne
/// s'y mélange pas — le scan le lit comme les autres.
fn proton_install_dir(shared: &Shared) -> std::path::PathBuf {
    let dir = shared.paths.proton_dir.clone();
    let _ = std::fs::create_dir_all(&dir);
    dir
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_volume_du_jeu_est_retenu_au_centieme() {
        let root = tempfile::tempdir().unwrap();
        let paths = lapinux_core::paths::LapinuxPaths::under(root.path());
        paths.ensure().unwrap();
        let shared = Shared::new(&paths).unwrap();
        shared
            .store
            .save_game(&lapinux_core::model::Game {
                appid: 424242,
                settings: GameSettings {
                    steam_shim: Some(true),
                    ..Default::default()
                },
                ..Default::default()
            })
            .unwrap();
        shared.remember_volume(424242, 0.4242).unwrap();
        let game = shared.store.game(424242).unwrap().unwrap();
        assert_eq!(game.settings.volume, Some(0.42));
        // Les autres réglages ne bougent pas : le menu rapide n'écrase rien.
        assert_eq!(game.settings.steam_shim, Some(true));
        // Un même niveau n'écrit pas deux fois, et le volume est borné.
        shared.remember_volume(424242, 0.4249).unwrap();
        assert_eq!(shared.store.game(424242).unwrap().unwrap().settings.volume, Some(0.42));
        shared.remember_volume(424242, 3.0).unwrap();
        assert_eq!(shared.store.game(424242).unwrap().unwrap().settings.volume, Some(1.0));
        // Un jeu inconnu est refusé sans écrire.
        assert!(shared.remember_volume(1, 0.5).is_err());
    }

    #[test]
    fn game_settings_wait_for_session_finalization() {
        let root = tempfile::tempdir().unwrap();
        let paths = lapinux_core::paths::LapinuxPaths::under(root.path());
        paths.ensure().unwrap();
        let shared = std::sync::Arc::new(Shared::new(&paths).unwrap());
        shared
            .store
            .save_game(&lapinux_core::model::Game {
                appid: 424242,
                ..Default::default()
            })
            .unwrap();
        let guard = shared.save_operation.lock().unwrap();
        let worker = std::sync::Arc::clone(&shared);
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            worker
                .save_settings(
                    424242,
                    GameSettings {
                        steam_shim: Some(true),
                        ..Default::default()
                    },
                )
                .unwrap();
            done_tx.send(()).unwrap();
        });
        started_rx.recv().unwrap();
        assert!(done_rx
            .recv_timeout(std::time::Duration::from_millis(100))
            .is_err());
        let mut stale = shared.store.game(424242).unwrap().unwrap();
        stale.playtime_secs = 42;
        shared.store.save_game(&stale).unwrap();
        drop(guard);
        done_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        thread.join().unwrap();
        let game = shared.store.game(424242).unwrap().unwrap();
        assert_eq!(game.playtime_secs, 42);
        assert_eq!(game.settings.steam_shim, Some(true));
    }

    #[test]
    fn afmf_catalog_matches_supported_versions_and_rejects_unknown() {
        for version in lapinux_core::config::AFMF_VERSIONS {
            let release = afmf_release(version).expect("version épinglée");
            assert_eq!(release.version, *version);
            assert!(release.url.starts_with(
                "https://github.com/serialexperimentslainnnn/afmf-linux/releases/download/"
            ));
            assert_eq!(release.archive_hash.len(), 64);
            assert!(release.hashes.iter().all(|h| h.len() == 64));
            assert_eq!(release.installed_manifest_hash.len(), 64);
        }
        assert!(afmf_release("../1.3.1").is_none());
        assert!(afmf_release("latest").is_none());
    }

    #[test]
    fn afmf_runtime_rejects_wrong_files_and_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("libafmf-linux.so"), "not the layer").unwrap();
        std::fs::write(dir.path().join("afmf-linux.json"), "{}").unwrap();
        assert!(validate_afmf_runtime(dir.path(), "1.3.1").is_err());
        std::fs::remove_file(dir.path().join("libafmf-linux.so")).unwrap();
        std::os::unix::fs::symlink(
            dir.path().join("afmf-linux.json"),
            dir.path().join("libafmf-linux.so"),
        )
        .unwrap();
        assert!(validate_afmf_runtime(dir.path(), "1.3.1").is_err());
    }

    #[test]
    fn afmf_local_runtime_needs_its_own_manifest() {
        use sha2::{Digest, Sha256};
        let dir = tempfile::tempdir().unwrap();
        let library = b"couche locale";
        let manifest = br#"{"layer":{"library_path":"./libafmf-linux.so"}}"#;
        std::fs::write(dir.path().join("libafmf-linux.so"), library).unwrap();
        std::fs::write(dir.path().join("afmf-linux.json"), manifest).unwrap();
        // Sans manifeste local, une couche non publiée n'est jamais chargée.
        assert!(validate_afmf_runtime(dir.path(), "local").is_err());
        let hashes = serde_json::json!({
            "files": {
                "libafmf-linux.so": format!("{:x}", Sha256::digest(library)),
                "afmf-linux.json": format!("{:x}", Sha256::digest(manifest)),
            }
        });
        std::fs::write(
            dir.path().join("lapinux-local.json"),
            serde_json::to_vec(&hashes).unwrap(),
        )
        .unwrap();
        assert!(validate_afmf_runtime(dir.path(), "local").is_ok());
        // Un fichier modifié après coup arrête le chargement.
        std::fs::write(dir.path().join("libafmf-linux.so"), b"autre chose").unwrap();
        assert!(validate_afmf_runtime(dir.path(), "local").is_err());
    }

    #[test]
    fn afmf_remove_refuses_modified_or_unrelated_files() {
        use sha2::{Digest, Sha256};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("version");
        std::fs::create_dir(&path).unwrap();
        let files = ["one", "two", "three", "four", "five"];
        for name in files {
            std::fs::write(path.join(name), name).unwrap();
        }
        let hashes: Vec<String> = files
            .iter()
            .map(|name| format!("{:x}", Sha256::digest(name.as_bytes())))
            .collect();
        let expected = std::array::from_fn(|i| (files[i], hashes[i].as_str()));
        std::fs::write(path.join("one"), "changed").unwrap();
        assert!(remove_verified_afmf(&path, expected).is_err());
        assert!(path.join("two").exists());
        std::fs::write(path.join("one"), "one").unwrap();
        std::fs::write(path.join("unrelated"), "data").unwrap();
        assert!(remove_verified_afmf(&path, expected).is_err());
        assert!(path.join("one").exists());
        std::fs::remove_file(path.join("unrelated")).unwrap();
        remove_verified_afmf(&path, expected).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn afmf_rejects_modified_archive_before_creating_files() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("afmf/1.3.1");
        let result = install_afmf_archive(
            b"modified",
            &destination,
            &AtomicBool::new(false),
            "7fafa09021ee94dc5ec8da694cb02fe298b54c114b486611bb24b1e1500edb18",
            [""; 5],
            "1.3.1",
        );
        assert!(result.unwrap_err().contains("SHA-256"));
        assert!(!destination.exists());
    }

    #[test]
    fn afmf_rejects_unexpected_or_missing_contents() {
        use sha2::{Digest, Sha256};
        let mut archive = tar::Builder::new(xz2::write::XzEncoder::new(Vec::new(), 6));
        let mut header = tar::Header::new_gnu();
        header.set_size(4);
        header.set_mode(0o644);
        header.set_cksum();
        archive
            .append_data(&mut header, "afmf-linux-1.3.1/other", &b"test"[..])
            .unwrap();
        let bytes = archive.into_inner().unwrap().finish().unwrap();
        let hash = format!("{:x}", Sha256::digest(&bytes));
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("version");
        let result = install_afmf_archive(
            &bytes,
            &dest,
            &AtomicBool::new(false),
            &hash,
            [""; 5],
            "1.3.1",
        );
        assert!(result.unwrap_err().contains("missing"));
        assert!(!dest.exists());
    }

    #[test]
    fn afmf_installs_verified_files_without_replacing_existing_version() {
        use sha2::{Digest, Sha256};
        let entries: [(&str, &[u8]); 5] = [
            ("libafmf-linux.so", b"library"),
            (
                "afmf-linux.json",
                br#"{"layer":{"library_path": "libafmf-linux.so"}}"#,
            ),
            ("LICENSE", b"license"),
            ("LICENSE.fidelityfx.txt", b"fidelity license"),
            ("NOTICE.fidelityfx.md", b"notice"),
        ];
        let mut archive = tar::Builder::new(xz2::write::XzEncoder::new(Vec::new(), 6));
        let mut hashes = Vec::new();
        for (name, body) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            archive
                .append_data(&mut header, format!("afmf-linux-1.3.1/{name}"), body)
                .unwrap();
            hashes.push(format!("{:x}", Sha256::digest(body)));
        }
        let bytes = archive.into_inner().unwrap().finish().unwrap();
        let hash = format!("{:x}", Sha256::digest(&bytes));
        let hashes: [&str; 5] = std::array::from_fn(|i| hashes[i].as_str());
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("afmf/1.3.1");
        let cancel = AtomicBool::new(false);
        install_afmf_archive(&bytes, &destination, &cancel, &hash, hashes, "1.3.1").unwrap();
        assert_eq!(
            std::fs::read(destination.join("libafmf-linux.so")).unwrap(),
            b"library"
        );
        assert!(std::fs::read_to_string(destination.join("afmf-linux.json"))
            .unwrap()
            .contains("./libafmf-linux.so"));
        assert!(destination.join("LICENSE.fidelityfx.txt").is_file());
        assert!(
            install_afmf_archive(&bytes, &destination, &cancel, &hash, hashes, "1.3.1").is_err()
        );
        assert_eq!(
            std::fs::read(destination.join("libafmf-linux.so")).unwrap(),
            b"library"
        );
        // Deux versions restent côte à côte et le nom du répertoire de
        // l'archive doit correspondre à la version demandée.
        let second = dir.path().join("afmf/1.3.0");
        assert!(install_afmf_archive(&bytes, &second, &cancel, &hash, hashes, "1.3.0").is_err());
        let cancelled_destination = dir.path().join("afmf/other");
        assert!(install_afmf_archive(
            &bytes,
            &cancelled_destination,
            &AtomicBool::new(true),
            &hash,
            hashes,
            "1.3.1"
        )
        .is_err());
        assert!(!cancelled_destination.exists());
    }

    fn proton(path: &str) -> lapinux_core::model::ProtonInfo {
        lapinux_core::model::ProtonInfo {
            name: path.rsplit('/').next().unwrap_or(path).to_string(),
            path: std::path::PathBuf::from(path),
            source: "compatibilitytools".into(),
            version: String::new(),
            runnable: true,
        }
    }

    /// « Latest » est la même famille que son build numéroté : installer
    /// l'un retire l'autre, sans emporter les autres Proton ni ceux d'un
    /// autre dossier.
    #[test]
    fn a_new_build_supersedes_its_family_only() {
        let protons = vec![
            proton("/x/compatibilitytools.d/DW-Proton Latest"),
            proton("/x/compatibilitytools.d/dwproton-11.0-15-x86_64"),
            proton("/x/compatibilitytools.d/GE-Proton11-11-x86_64"),
            proton("/y/compatibilitytools.d/dwproton-10.0-1"),
        ];
        let older = older_protons(
            std::path::Path::new("/x/compatibilitytools.d/dwproton-11.0-15-x86_64"),
            &protons,
        );
        let names: Vec<&str> = older.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["DW-Proton Latest"]);
    }

    /// Un build seul ne retire rien, et une famille inconnue (nom sans
    /// lettre, comme « 123 ») ne touche à personne.
    #[test]
    fn a_lonely_build_removes_nothing() {
        let protons = vec![proton("/x/compatibilitytools.d/GE-Proton11-11-x86_64")];
        assert!(older_protons(
            std::path::Path::new("/x/compatibilitytools.d/GE-Proton11-11-x86_64"),
            &protons
        )
        .is_empty());
        assert!(older_protons(std::path::Path::new("/"), &protons).is_empty());
    }
}
