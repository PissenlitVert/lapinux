//! Téléchargements (jeux, Proton, outils) et réglages persistés.

use crate::{Result, Shared};
use ludus_core::config::Config;
use ludus_core::ipc::Envelope;
use ludus_core::model::{GameSettings, Job, JobKind, JobState};
use ludus_steam::steamcmd::{InstallArgs, Platform, SteamEvent};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

impl Shared {
    pub fn save_settings(&self, appid: u32, settings: GameSettings) -> Result<Envelope> {
        let mut game = self
            .store
            .game(appid)
            .map_err(|e| ludus_core::Error::Other(e.to_string()))?
            .ok_or_else(|| ludus_core::Error::Other("jeu introuvable".into()))?;
        game.settings = settings;
        self.store
            .save_game(&game)
            .map_err(|e| ludus_core::Error::Other(e.to_string()))?;
        Ok(Envelope::ok(&game.settings))
    }

    pub fn save_config(&self, config: Config) -> Result<Envelope> {
        let config = config.normalized();
        config
            .save(&self.paths.config_file)
            .map_err(|e| ludus_core::Error::Other(e.to_string()))?;
        if let Ok(mut guard) = self.config.lock() {
            *guard = config.clone();
        }
        Ok(Envelope::ok(&config))
    }

    pub fn install_game(self: &Arc<Self>, appid: u32) -> Result<Envelope> {
        let config = self.config_snapshot();
        if config.steam.username.trim().is_empty() {
            return Err(ludus_core::Error::Other(
                "renseignez votre compte Steam dans les réglages (le mot de passe n'est jamais stocké)"
                    .into(),
            ));
        }
        let existing = self.store.game(appid).ok().flatten();
        let (name, installdir) = match existing {
            Some(game) if !game.name.is_empty() && !game.installdir.is_empty() => {
                (game.name, game.installdir)
            }
            _ => {
                if !self.steamcmd.available() {
                    return Err(ludus_core::Error::Other(
                        "SteamCMD requis pour identifier le jeu : installez-le depuis l'écran Outils"
                            .into(),
                    ));
                }
                let info = self
                    .steamcmd
                    .app_info(appid)
                    .map_err(|e| ludus_core::Error::Other(e.to_string()))?;
                (info.name, info.installdir)
            }
        };
        if installdir.is_empty() {
            return Err(ludus_core::Error::Other(format!(
                "répertoire d'installation inconnu pour {appid}"
            )));
        }
        let backend = ludus_steam::choose_backend(
            &config.steam.backend,
            config.steam.allow_depotdownloader,
            self.steamcmd.available(),
            self.depot.available(),
        )
        .ok_or_else(|| {
            ludus_core::Error::Other(
                "aucun outil de téléchargement disponible : installez SteamCMD (écran Outils)".into(),
            )
        })?;
        let library_root = config.library_root();
        let target = library_root.join("steamapps/common").join(&installdir);
        let job_id = format!("game-{appid}-{}", ludus_core::util::now_unix());
        let job = Job {
            id: job_id.clone(),
            kind: JobKind::InstallGame,
            label: format!("{name} ({appid})"),
            appid: Some(appid),
            state: JobState::Running,
            progress: 0.0,
            downloaded: 0,
            total: 0,
            message: format!("via {}", backend.label()),
            backend: backend.label().to_string(),
            started: ludus_core::util::now_unix(),
            finished: None,
        };
        if let Ok(mut jobs) = self.jobs.lock() {
            jobs.push(job);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        if let Ok(mut map) = self.cancel.lock() {
            map.insert(job_id.clone(), Arc::clone(&cancel));
        }

        let shared = Arc::clone(self);
        let job_for_thread = job_id.clone();
        let name_for_thread = name.clone();
        let installdir_for_thread = installdir.clone();
        std::thread::spawn(move || {
            let args = InstallArgs {
                username: config.steam.username.clone(),
                password: None,
                appid,
                target: target.clone(),
                platform: Platform::Windows,
                validate: true,
            };
            let mut on_event = |event: SteamEvent| {
                if cancel.load(Ordering::Relaxed) {
                    return false;
                }
                match event {
                    SteamEvent::Progress {
                        percent,
                        downloaded,
                        total,
                    } => shared.update_job(&job_for_thread, |job| {
                        job.progress = (percent / 100.0).clamp(0.0, 1.0);
                        job.downloaded = downloaded;
                        job.total = total;
                        job.message = backend.label().to_string();
                    }),
                    SteamEvent::Line(line) => {
                        if !line.trim().is_empty() {
                            shared.update_job(&job_for_thread, |job| {
                                job.message = line.chars().take(160).collect();
                            });
                        }
                    }
                    SteamEvent::Fatal(message) => shared.update_job(&job_for_thread, |job| {
                        job.state = JobState::Failed;
                        job.message = message;
                    }),
                    SteamEvent::Done => {}
                }
                true
            };

            let result = if cancel.load(Ordering::Relaxed) {
                Err(ludus_steam::Error::Other("annulé".into()))
            } else {
                match backend {
                    ludus_steam::Backend::SteamCmd => {
                        shared.steamcmd.install(&args, &mut on_event).map_err(|e| {
                            ludus_steam::Error::Other(e.to_string())
                        })
                    }
                    ludus_steam::Backend::DepotDownloader => shared
                        .depot
                        .install(&args, &mut on_event)
                        .map_err(|e| ludus_steam::Error::Other(e.to_string())),
                }
            };

            let cancelled = cancel.load(Ordering::Relaxed);
            match result {
                Ok(()) => {
                    let size = dir_size(&target);
                    if let Err(err) = ludus_steam::write_manifest(
                        &library_root,
                        appid,
                        &name_for_thread,
                        &installdir_for_thread,
                        size,
                    ) {
                        tracing::warn!("manifeste {appid} : {err}");
                    }
                    let _ = shared.rescan_library();
                    shared.update_job(&job_for_thread, |job| {
                        job.state = JobState::Done;
                        job.progress = 1.0;
                        job.finished = Some(ludus_core::util::now_unix());
                        job.message = format!("installé ({} o)", size);
                    });
                }
                Err(err) => {
                    shared.update_job(&job_for_thread, |job| {
                        job.state = if cancelled {
                            JobState::Cancelled
                        } else {
                            JobState::Failed
                        };
                        job.finished = Some(ludus_core::util::now_unix());
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

    pub fn install_proton(
        self: &Arc<Self>,
        repo: String,
        asset: String,
        checksum_url: Option<String>,
    ) -> Result<Envelope> {
        let releases = ludus_proton::fetch_releases(&repo)
            .map_err(|e| ludus_core::Error::Other(e.to_string()))?;
        let release = releases
            .into_iter()
            .find(|r| r.asset == asset)
            .ok_or_else(|| ludus_core::Error::Other(format!("asset {asset} introuvable")))?;
        let destination = proton_install_dir(self);
        let job_id = format!("proton-{}-{}", release.asset, ludus_core::util::now_unix());
        let job = Job {
            id: job_id.clone(),
            kind: JobKind::InstallProton,
            label: release.asset.clone(),
            appid: None,
            state: JobState::Running,
            progress: 0.0,
            downloaded: 0,
            total: release.size,
            message: "téléchargement".into(),
            backend: "GitHub".into(),
            started: ludus_core::util::now_unix(),
            finished: None,
        };
        if let Ok(mut jobs) = self.jobs.lock() {
            jobs.push(job);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        if let Ok(mut map) = self.cancel.lock() {
            map.insert(job_id.clone(), Arc::clone(&cancel));
        }
        let shared = Arc::clone(self);
        let job_for_thread = job_id.clone();
        let checksum = checksum_url.or(release.checksum_url.clone());
        std::thread::spawn(move || {
            let mut last_update = 0u64;
            let result = ludus_proton::install_from_url(
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
                        if let Some(total) = total {
                            job.total = total;
                            job.progress = (done as f32 / total as f32).clamp(0.0, 1.0);
                        }
                        job.message = "téléchargement et vérification".into();
                    });
                },
            );
            match result {
                Ok(path) => {
                    shared.update_job(&job_for_thread, |job| {
                        job.state = JobState::Done;
                        job.progress = 1.0;
                        job.finished = Some(ludus_core::util::now_unix());
                        job.message = format!("installé : {}", path.display());
                    });
                    let _ = shared.scan_protons();
                }
                Err(err) => {
                    let cancelled = cancel.load(Ordering::Relaxed);
                    shared.update_job(&job_for_thread, |job| {
                        job.state = if cancelled {
                            JobState::Cancelled
                        } else {
                            JobState::Failed
                        };
                        job.finished = Some(ludus_core::util::now_unix());
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
            .ok_or_else(|| ludus_core::Error::Other(format!("Proton {name} introuvable")))?;
        let path = proton.path.clone();
        let text = path.to_string_lossy();
        let allowed = text.contains("compatibilitytools.d")
            || text.contains(&self.paths.proton_dir.to_string_lossy().to_string());
        if !allowed {
            return Err(ludus_core::Error::Other(
                "refus de supprimer un Proton installé par Steam".into(),
            ));
        }
        std::fs::remove_dir_all(&path).map_err(ludus_core::Error::Io)?;
        let protons = self.scan_protons()?;
        Ok(Envelope::ok(&protons))
    }

    pub fn cancel_job(&self, id: &str) -> Result<Envelope> {
        if let Ok(map) = self.cancel.lock() {
            if let Some(flag) = map.get(id) {
                flag.store(true, Ordering::Relaxed);
                return Ok(Envelope::ok(
                    &serde_json::json!({ "message": "annulation demandée" }),
                ));
            }
        }
        Err(ludus_core::Error::Other("tâche inconnue".into()))
    }

    pub fn install_tools(self: &Arc<Self>, which: Vec<String>) -> Result<Envelope> {
        let job_id = format!("tools-{}", ludus_core::util::now_unix());
        let job = Job {
            id: job_id.clone(),
            kind: JobKind::InstallProton,
            label: "Outils Steam".into(),
            appid: None,
            state: JobState::Running,
            progress: 0.0,
            downloaded: 0,
            total: 0,
            message: "téléchargement des outils".into(),
            backend: "réseau".into(),
            started: ludus_core::util::now_unix(),
            finished: None,
        };
        if let Ok(mut jobs) = self.jobs.lock() {
            jobs.push(job);
        }
        let shared = Arc::clone(self);
        let job_for_thread = job_id.clone();
        std::thread::spawn(move || {
            let mut errors = Vec::new();
            for tool in &which {
                let mut line = |text: &str| {
                    shared.update_job(&job_for_thread, |job| job.message = text.to_string());
                };
                let result = match tool.as_str() {
                    "steamcmd" => shared.steamcmd.ensure(&mut line),
                    "depotdownloader" => shared.depot.ensure(&mut line),
                    other => {
                        errors.push(format!("outil inconnu : {other}"));
                        continue;
                    }
                };
                if let Err(err) = result {
                    errors.push(format!("{tool} : {err}"));
                }
            }
            shared.update_job(&job_for_thread, |job| {
                job.state = if errors.is_empty() {
                    JobState::Done
                } else {
                    JobState::Failed
                };
                job.finished = Some(ludus_core::util::now_unix());
                job.message = if errors.is_empty() {
                    "outils prêts".to_string()
                } else {
                    errors.join(" ; ")
                };
            });
        });
        Ok(Envelope::ok(&serde_json::json!({ "job": job_id })))
    }
}

fn proton_install_dir(shared: &Shared) -> std::path::PathBuf {
    let roots = ludus_core::paths::steam_roots();
    if let Some(root) = roots.first() {
        let dir = root.join("compatibilitytools.d");
        if dir.is_dir() || std::fs::create_dir_all(&dir).is_ok() {
            return dir;
        }
    }
    shared.paths.proton_dir.clone()
}

fn dir_size(path: &std::path::Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![path.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                stack.push(entry.path());
            } else {
                total += meta.len();
            }
        }
    }
    total
}
