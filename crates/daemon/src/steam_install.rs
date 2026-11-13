//! Installation, file d’attente et désinstallation Steam.

use super::*;
use lapinux_core::i18n::{t, t_args};

impl Shared {
    /// Télécharge un jeu possédé ou met à jour une installation existante,
    /// dans la bibliothèque où le jeu se trouve déjà.
    ///
    /// `platform` est la version choisie dans le menu du bouton (`linux`,
    /// `windows`, `macos`) ; `None` garde la préférence du moteur.
    pub fn install_game(
        self: &Arc<Self>,
        appid: u32,
        dlc: Vec<u32>,
        platform: Option<String>,
    ) -> Result<Envelope> {
        // Sérialise la création de la tâche avec les installations graphiques.
        // Le verrou est libéré avant que le téléchargement de fond ne travaille.
        let _guard = self
            .save_operation
            .try_lock()
            .map_err(|_| lapinux_core::Error::Other(t("downloads.err_operation_busy").into()))?;
        let platform = normalize_platform(platform);
        let config = self.config_snapshot();
        if config.steam.username.trim().is_empty() {
            return Err(lapinux_core::Error::Other(
                t("downloads.err_steam_account_downloads").into(),
            ));
        }
        let saved = self.saved_session(&config.steam.username).ok_or_else(|| {
            lapinux_core::Error::Other(
                t("downloads.err_steam_account_settings").into(),
            )
        })?;
        if saved.refresh_token.trim().is_empty() {
            return Err(lapinux_core::Error::Other(
                t("downloads.err_session_no_token").into(),
            ));
        }

        let busy = self
            .jobs
            .lock()
            .map(|jobs| {
                jobs.iter().any(|job| {
                    job.appid == Some(appid)
                        && matches!(
                            job.state,
                            JobState::Running | JobState::Queued | JobState::Paused
                        )
                })
            })
            .unwrap_or(false);
        if busy {
            return Err(lapinux_core::Error::Other(
                t("downloads.err_install_running").into(),
            ));
        }

        let existing = self.store.game(appid).ok().flatten();
        // Mettre à jour un jeu déjà installé se fait dans SA bibliothèque :
        // un jeu Steam garde son dossier, son préfixe et ses fichiers.
        let installed_here = existing.as_ref().filter(|game| game.installed);
        let updating = installed_here.is_some();
        // Même un manifeste incomplet indique la bibliothèque à réparer.
        // Une bibliothèque gérée par Lapinux n'a plus de `steamapps` : ses
        // jeux vivent sous `Steam`, à côté des manifestes. Seule une vraie
        // bibliothèque Steam garde `steamapps/common`.
        let managed_layout = existing
            .as_ref()
            .map(|game| game.origin != lapinux_core::model::GameOrigin::Steam)
            .unwrap_or(true);
        let library_root = existing
            .as_ref()
            .map(|game| game.library.clone())
            .filter(|library| library.join("steamapps").is_dir() || library.join("Steam").is_dir())
            .unwrap_or_else(|| config.library_root());
        let games_root = if managed_layout {
            library_root.join("Steam")
        } else {
            library_root.join("steamapps").join("common")
        };
        let catalog = lapinux_steam::catalog::Catalog::load(&self.catalog_path());
        let catalog_entry = catalog.get(appid);
        let catalog_build = catalog_entry.and_then(|entry| entry.details.build_id);
        // Noms et poids des contenus demandés, pour leur écrire un manifeste
        // d'installation une fois les fichiers en place : c'est ce manifeste
        // qui dira ensuite, dans la fiche du jeu, que le contenu est déjà là.
        let extra_names: Vec<(u32, String, u64)> = dlc
            .iter()
            .filter(|id| **id != appid)
            .map(|id| {
                let entry = catalog.dlc(*id);
                (
                    *id,
                    entry
                        .map(|dlc| dlc.name.clone())
                        .filter(|name| !name.trim().is_empty())
                        .unwrap_or_else(|| format!("Contenu {id}")),
                    entry.map(|dlc| dlc.install).unwrap_or(0),
                )
            })
            .collect();
        let name = existing
            .as_ref()
            .map(|game| game.name.clone())
            .filter(|name| !name.is_empty())
            .or_else(|| catalog_entry.map(|entry| entry.display_name()))
            .unwrap_or_else(|| format!("App {appid}"));
        // En mode « un seul jeu à la fois », une demande qui arrive pendant
        // qu'une autre installation tourne rejoint la file. Le pompage la
        // reprend dès que la place se libère.
        let parallel = config.downloads.parallel_installs;
        // En mode « un seul jeu à la fois », seul un téléchargement qui avance
        // occupe le créneau : une tâche mise en pause par l'utilisateur ne
        // bloque pas la suivante.
        let active = self
            .jobs
            .lock()
            .map(|jobs| {
                jobs.iter()
                    .any(|job| job.kind == JobKind::InstallGame && job.state == JobState::Running)
            })
            .unwrap_or(false);
        let queued = !parallel && active;
        let job_id = format!("game-{appid}-{}", lapinux_core::util::now_unix());
        let job = Job {
            id: job_id.clone(),
            kind: JobKind::InstallGame,
            label: format!("{name} ({appid})"),
            title: name.clone(),
            appid: Some(appid),
            state: if queued {
                JobState::Queued
            } else {
                JobState::Running
            },
            progress: 0.0,
            downloaded: 0,
            total: 0,
            network: 0,
            written: 0,
            server: String::new(),
            depot: 0,
            depots: 0,
            updating,
            dlc: dlc.clone(),
            platform: platform.clone(),
            message: if queued {
                "en attente".to_string()
            } else if updating {
                "mise à jour".to_string()
            } else {
                "téléchargement".to_string()
            },
            backend: "Lapinux".to_string(),
            started: lapinux_core::util::now_unix(),
            finished: None,
            report: None,
        };
        if let Ok(mut jobs) = self.jobs.lock() {
            // Une carte arrêtée du même jeu est remplacée par celle qui
            // repart : sans cela, la reprise laissait l'ancienne en
            // « interrompu » à côté de la nouvelle, et la section « En pause »
            // ne se vidait jamais.
            jobs.retain(|old| {
                old.appid != Some(appid)
                    || !matches!(
                        old.state,
                        JobState::Interrupted | JobState::Failed | JobState::Cancelled
                    )
            });
            jobs.push(job);
        }
        self.persist_jobs(true);
        if queued {
            return Ok(Envelope::ok(&serde_json::json!({
                "job": job_id,
                "queued": true,
            })));
        }
        let cancel = Arc::new(AtomicBool::new(false));
        if let Ok(mut map) = self.cancel.lock() {
            map.insert(job_id.clone(), Arc::clone(&cancel));
        }
        let pause = Arc::new(AtomicBool::new(false));
        if let Ok(mut map) = self.pause.lock() {
            map.insert(job_id.clone(), Arc::clone(&pause));
        }

        let shared = Arc::clone(self);
        let download_limit = self.throttle.limit("steam");
        // Serveur CDN choisi dans les réglages de téléchargement : il passe en
        // tête de liste, sans bannir les autres.
        let preferred_server = self
            .config_snapshot()
            .downloads
            .steam_server
            .trim()
            .to_string();
        let preferred_server = if preferred_server.is_empty() {
            None
        } else {
            Some(preferred_server)
        };
        let job_for_thread = job_id.clone();
        let job_dlc = dlc;
        let job_platform = platform;
        // Langue retenue pour ce jeu : la sienne, sinon celle des réglages
        // généraux. Elle choisit les dépôts de doublage téléchargés.
        let language = self
            .store
            .game(appid)
            .ok()
            .flatten()
            .and_then(|game| game.settings.language)
            .unwrap_or_default();
        let language =
            lapinux_core::lang::resolve(&language, &self.config_snapshot().general.game_language)
                .map(|language| language.code);
        let account = saved.account_name.clone();
        let refresh_token = saved.refresh_token.clone();
        std::thread::spawn(move || {
            // Journal détaillé du transfert : chaque chunk, chaque pause, et le
            // débit réellement reçu par la machine. Il répond à la question
            // « est-ce que le téléchargement continue après l'arrêt ? ».
            let log = lapinux_steam::download_log::DownloadLog::open(
                &shared.paths.log_dir.join("downloads.log"),
            );
            log.write(format!(
                "tâche {job_for_thread} · appid={appid} · « {name} » · version {} · {} · bibliothèque={} · mise à jour={updating}",
                job_platform.as_deref().unwrap_or("automatique"),
                if cancel.load(Ordering::Relaxed) {
                    "annulée avant démarrage"
                } else {
                    "démarrage"
                },
                library_root.display()
            ));
            tracing::info!("journal de téléchargement : {}", log.path().display());
            let mut installed_name = name.clone();
            // Dossier réel du jeu, connu dès les métadonnées : une annulation
            // d'installation neuve le supprimera en entier.
            let mut installed_dir: Option<String> = None;
            let mut on_event = |event: InstallEvent| match event {
                InstallEvent::Metadata { meta } => {
                    installed_name = meta.name.clone();
                    installed_dir = Some(meta.installdir.clone());
                    shared.update_job(&job_for_thread, |job| {
                        job.title = meta.name.clone();
                        job.label = format!("{} ({appid})", meta.name);
                        job.message = t("downloads.preparing").to_string();
                    });
                }
                InstallEvent::Status { message } => {
                    shared.update_job(&job_for_thread, |job| {
                        job.message = message;
                    });
                }
                InstallEvent::Progress {
                    depot,
                    depots,
                    fraction,
                    downloaded,
                    total,
                    network,
                    written,
                    server,
                    label,
                } => {
                    // La fraction est déjà globale au build : l'afficher
                    // telle quelle évite un pourcentage qui repart à zéro à
                    // chaque dépôt.
                    let overall = fraction.clamp(0.0, 1.0);
                    shared.update_job(&job_for_thread, |job| {
                        job.progress = overall;
                        job.downloaded = downloaded;
                        job.total = total;
                        job.network = network;
                        job.written = written;
                        job.server = server;
                        job.depot = depot + 1;
                        job.depots = depots;
                        job.message = label;
                    });
                }
            };

            let result = if cancel.load(Ordering::Relaxed) {
                Err(lapinux_steam::Error::Other(t("downloads.store_cancelled").into()))
            } else {
                native::install(
                    &account,
                    &refresh_token,
                    appid,
                    &job_dlc,
                    job_platform.as_deref(),
                    language,
                    &library_root,
                    &games_root,
                    Arc::clone(&cancel),
                    Arc::clone(&pause),
                    &log,
                    download_limit,
                    preferred_server.as_deref(),
                    &mut on_event,
                )
            };

            let cancelled = cancel.load(Ordering::Relaxed);
            log.write(format!(
                "tâche {job_for_thread} terminée · état={} · {}",
                if cancelled {
                    "annulée"
                } else if result.is_ok() {
                    "réussie"
                } else {
                    "échouée"
                },
                match &result {
                    Ok(outcome) => format!("{} o réseau", outcome.bytes),
                    Err(error) => error.to_string(),
                }
            ));
            match result {
                Ok(outcome) => {
                    let name = if outcome.meta.name.trim().is_empty() {
                        installed_name
                    } else {
                        outcome.meta.name.clone()
                    };
                    // Les contenus additionnels installés reçoivent eux aussi
                    // leur manifeste, comme le fait le client Steam : sans lui,
                    // la fiche du jeu les croirait absents du disque.
                    for (dlc_appid, dlc_name, dlc_size) in &extra_names {
                        if !outcome.apps.contains(dlc_appid) {
                            continue;
                        }
                        if let Err(err) = lapinux_steam::write_manifest(
                            &library_root,
                            *dlc_appid,
                            dlc_name,
                            &outcome.meta.installdir,
                            *dlc_size,
                            managed_layout,
                        ) {
                            tracing::debug!("manifeste du contenu {dlc_appid} non écrit : {err}");
                        }
                    }
                    match lapinux_steam::manifest::record_build(
                        &library_root,
                        appid,
                        &name,
                        &outcome.meta.installdir,
                        outcome.size,
                        outcome.build_id.or(catalog_build),
                        managed_layout,
                    ) {
                        Ok(_) => match shared.rescan_library_at(&library_root) {
                            Ok(_) => {
                                let cloud =
                                    if shared.config_snapshot().downloads.cloud_saves_after_install
                                    {
                                        shared.update_job(&job_for_thread, |job| {
                                            job.message = t("downloads.job_cloud_saves").into();
                                        });
                                        shared
                                            .pull_cloud_after_install(appid)
                                            .map(|note| format!(" · {note}"))
                                            .unwrap_or_default()
                                    } else {
                                        String::new()
                                    };
                                shared.update_job(&job_for_thread, |job| {
                                    job.state = JobState::Done;
                                    job.progress = 1.0;
                                    job.finished = Some(lapinux_core::util::now_unix());
                                    job.message = format!(
                                        "{}{cloud}",
                                        if updating {
                                            t_args(
                                                "downloads.job_updated_size",
                                                &[("size", &outcome.size.to_string())],
                                            )
                                        } else {
                                            t_args(
                                                "downloads.job_installed_size",
                                                &[("size", &outcome.size.to_string())],
                                            )
                                        }
                                    );
                                });
                            }
                            Err(err) => shared.update_job(&job_for_thread, |job| {
                                job.state = JobState::Failed;
                                job.finished = Some(lapinux_core::util::now_unix());
                                job.message = t_args("downloads.job_library_refresh_failed", &[("error", &err.to_string())]);
                            }),
                        },
                        Err(err) => {
                            shared.update_job(&job_for_thread, |job| {
                                job.state = JobState::Failed;
                                job.finished = Some(lapinux_core::util::now_unix());
                                job.message = t_args("downloads.job_manifest_failed", &[("error", &err.to_string())]);
                            });
                        }
                    }
                }
                Err(err) => {
                    shared.update_job(&job_for_thread, |job| {
                        job.state = if cancelled {
                            JobState::Cancelled
                        } else {
                            JobState::Failed
                        };
                        job.finished = Some(lapinux_core::util::now_unix());
                        job.message = err.to_string();
                    });
                    if cancelled && !updating {
                        if let Some(dir) = installed_dir.as_deref() {
                            remove_partial_install(&games_root, dir);
                        }
                    }
                }
            }
            if let Ok(mut map) = shared.cancel.lock() {
                map.remove(&job_for_thread);
            }
            if let Ok(mut map) = shared.pause.lock() {
                map.remove(&job_for_thread);
            }
            // La place est libre : la file peut lancer la tâche suivante.
            shared.pump_install_queue();
            if cancelled {
                // Preuve horodatée : le moteur est arrêté, le compteur du
                // journal est figé. On mesure six secondes de trafic machine
                // pour montrer s'il reste autre chose sur la connexion.
                log.write("annulation terminée · mesure du trafic machine pendant 6 s");
                let mut previous = lapinux_steam::download_log::system_rx_bytes();
                for second in 1..=6 {
                    std::thread::sleep(Duration::from_secs(1));
                    let now = lapinux_steam::download_log::system_rx_bytes();
                    if let (Some(before), Some(after)) = (previous, now) {
                        let delta = after.saturating_sub(before);
                        log.write(format!(
                            "post-annulation {second} s · machine +{delta} o ({:.2} MiB/s)",
                            lapinux_steam::download_log::rate_mib(delta, 1000)
                        ));
                    }
                    previous = now;
                }
                log.write("fin des mesures post-annulation");
            }
        });

        Ok(Envelope::ok(&serde_json::json!({ "job": job_id })))
    }

    /// Lance les tâches en attente. En mode simultané, toutes partent ; en
    /// mode file d'attente, une seule, les autres gardent leur place.
    pub fn pump_install_queue(self: &Arc<Self>) {
        let gate = self.gate_read();
        if *gate { return; }
        let parallel = self.config_snapshot().downloads.parallel_installs;
        loop {
            let next = {
                let Ok(jobs) = self.jobs.lock() else {
                    return;
                };
                next_queued_install(&jobs, parallel)
            };
            let Some(job) = next else {
                return;
            };
            let Some(appid) = job.appid else {
                // Une entrée sans jeu ne doit pas bloquer la file.
                if let Ok(mut jobs) = self.jobs.lock() {
                    jobs.retain(|entry| entry.id != job.id);
                }
                self.persist_jobs(true);
                continue;
            };
            // La demande en attente est remplacée par une vraie tâche Running :
            // install_game recrée le job avec son heure de départ réelle.
            if let Ok(mut jobs) = self.jobs.lock() {
                jobs.retain(|entry| entry.id != job.id);
            }
            self.persist_jobs(true);
            if let Err(error) = self.install_game(appid, job.dlc.clone(), job.platform.clone()) {
                let mut failed = job;
                failed.state = JobState::Failed;
                failed.finished = Some(lapinux_core::util::now_unix());
                failed.message = error.to_string();
                if let Ok(mut jobs) = self.jobs.lock() {
                    jobs.push(failed);
                }
                self.persist_jobs(true);
            }
            if !parallel {
                return;
            }
        }
    }

    /// Une copie vérifiée est exigée avant toute suppression demandée avec
    /// sauvegarde. Un jeu jamais lancé, sans dossier de sauvegarde connu, n'a
    /// rien à copier : le bloquer rendait la désinstallation impossible.
    pub fn uninstall_game_with_backup(&self, appid: u32) -> Result<Envelope> {
        let _guard = self.save_operation.try_lock().map_err(|_| {
            lapinux_core::Error::Other(
                t("downloads.err_save_operation_running").into(),
            )
        })?;
        if self.session_info().map(|session| session.appid) == Some(appid) {
            return Err(lapinux_core::Error::Other(
                t("downloads.err_close_before_saves").into(),
            ));
        }
        let game = self
            .store
            .game(appid)?
            .ok_or_else(|| lapinux_core::Error::Other(t("downloads.err_game_not_found").into()))?;
        let copy = match crate::saves::verified_backup(&game, &self.paths, &self.config_snapshot())
        {
            Ok(copy) => Some(copy),
            Err(error) => {
                // Un jeu qui n'a jamais tourné n'a pas pu écrire de
                // sauvegarde. Sans dossier configuré ni dossier détecté, il
                // n'y a rien à copier : refuser laissait le jeu impossible à
                // désinstaller, notamment juste après une installation.
                let never_launched =
                    game.playtime_secs == 0 && game.last_played.is_none() && game.prefix.is_none();
                let no_configured_dir = game
                    .settings
                    .save_dir
                    .as_deref()
                    .map(str::trim)
                    .map(str::is_empty)
                    .unwrap_or(true);
                if never_launched
                    && no_configured_dir
                    && crate::saves::source(&game, &self.paths).is_none()
                {
                    tracing::info!(
                        "jeu {appid} jamais lancé et sans dossier de sauvegarde : désinstallation sans copie"
                    );
                    None
                } else {
                    return Err(lapinux_core::Error::Other(t_args("downloads.err_save_copy_failed", &[("error", &error.to_string())])));
                }
            }
        };
        let mut result = self.uninstall_game_locked(appid)?;
        if let Some(data) = result.data.as_object_mut() {
            data.insert(
                "backup".into(),
                copy.as_ref().map_or(serde_json::Value::Null, |path| {
                    serde_json::Value::String(path.clone())
                }),
            );
        }
        if let Some(copy) = &copy {
            tracing::info!("copie vérifiée avant désinstallation : {copy}");
        }
        Ok(result)
    }

    /// Désinstalle un jeu : fichiers, préfixe Proton et manifeste. Refuse
    /// pendant une session ou un téléchargement, et ne supprime jamais un
    /// chemin qui n'est pas exactement `<bibliothèque>/steamapps/common/<jeu>`.
    pub fn uninstall_game(&self, appid: u32) -> Result<Envelope> {
        self.uninstall_game_with_backup(appid)
    }

    pub(super) fn uninstall_game_locked(&self, appid: u32) -> Result<Envelope> {
        let game = self
            .store
            .game(appid)
            .map_err(|e| lapinux_core::Error::Other(e.to_string()))?
            .ok_or_else(|| lapinux_core::Error::Other(t("downloads.err_game_not_found").into()))?;
        if !game.installed {
            return Err(lapinux_core::Error::Other(
                t("downloads.err_game_not_installed").into(),
            ));
        }
        if self
            .session_info()
            .map(|session| session.appid == appid)
            .unwrap_or(false)
        {
            return Err(lapinux_core::Error::Other(
                t("downloads.err_close_before_uninstall").into(),
            ));
        }
        if self
            .jobs
            .lock()
            .map(|jobs| {
                jobs.iter().any(|job| {
                    job.appid == Some(appid)
                        && matches!(
                            job.state,
                            JobState::Running | JobState::Queued | JobState::Paused
                        )
                })
            })
            .unwrap_or(false)
        {
            return Err(lapinux_core::Error::Other(
                t("downloads.err_install_running").into(),
            ));
        }

        // Un jeu de boutique vit sous le rayon de sa boutique, à la racine de
        // la bibliothèque ; un jeu Steam dans `steamapps/common` ; un jeu
        // Lapinux sous `Steam`. Le contrôle est le même pour tous : aucun
        // `..`, et le dossier doit rester sous la racine de la bibliothèque.
        let common = if game.origin == lapinux_core::model::GameOrigin::Store
            && game.store != lapinux_core::stores::Store::Steam
        {
            game.library.clone()
        } else if game.origin == lapinux_core::model::GameOrigin::Steam {
            game.library.join("steamapps").join("common")
        } else {
            game.library.join("Steam")
        };
        let dir = game.install_dir();
        let name = game.installdir.trim();
        let safe_name = !name.is_empty()
            && name
                .split('/')
                .all(|part| !part.is_empty() && part != "." && part != "..")
            && !name.contains('\\');
        let inside_library = dir
            .strip_prefix(&common)
            .map(|rest| !rest.as_os_str().is_empty())
            .unwrap_or(false);
        if !safe_name || !inside_library {
            return Err(lapinux_core::Error::Other(format!(
                "dossier d'installation inattendu : {}",
                dir.display()
            )));
        }

        // Un lien symbolique est retiré lui-même, jamais sa cible.
        if let Ok(metadata) = std::fs::symlink_metadata(&dir) {
            if metadata.file_type().is_symlink() {
                std::fs::remove_file(&dir)?;
            } else if metadata.is_dir() {
                std::fs::remove_dir_all(&dir)?;
            }
        }

        // Le rayon d'une boutique vidé de son dernier jeu ne reste pas : un
        // dossier vide à la racine se lirait comme une bibliothèque fantôme.
        if game.origin == lapinux_core::model::GameOrigin::Store
            && game.store != lapinux_core::stores::Store::Steam
        {
            if let Some(shelf) = dir.parent() {
                if shelf != game.library.as_path() {
                    let _ = std::fs::remove_dir(shelf);
                }
            }
        }

        // Préfixe Proton : Steam dans `steamapps/compatdata/`, les jeux de
        // Lapinux et des boutiques dans le `CompatData/` lisible de leur
        // bibliothèque, l'ancien rangement sous les données ou `compatdata/`.
        // Aucun autre chemin n'est supprimé.
        let mut prefixes = Vec::new();
        if let Some(prefix) = &game.prefix {
            prefixes.push(prefix.clone());
        }
        prefixes.push(self.paths.data_dir.join("prefixes").join(appid.to_string()));
        prefixes.push(
            game.library
                .join("steamapps")
                .join("compatdata")
                .join(appid.to_string()),
        );
        prefixes.push(game.library.join("compatdata").join(appid.to_string()));
        if !game.library.as_os_str().is_empty() {
            let compat = game.library.join("CompatData");
            prefixes.push(compat.join(lapinux_proton::prefix_name_for(&game)));
            // Le jeu a pu être renommé : son dossier garde l'ancien nom et
            // son identifiant. Tous les candidats de cet identifiant partent,
            // sinon un préfixe orphelin resterait après la désinstallation.
            let suffix = format!(" ({appid})");
            if let Ok(entries) = std::fs::read_dir(&compat) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let matches = path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .map(|name| name.ends_with(&suffix))
                        .unwrap_or(false);
                    if matches && path.is_dir() {
                        prefixes.push(path);
                    }
                }
            }
        }
        for prefix in prefixes {
            let lapinux_prefix = prefix.starts_with(self.paths.data_dir.join("prefixes"));
            let steam_prefix =
                prefix.starts_with(game.library.join("steamapps").join("compatdata"));
            let library_prefix = prefix.starts_with(game.library.join("compatdata"));
            let readable_prefix = prefix.starts_with(game.library.join("CompatData"));
            if (lapinux_prefix || steam_prefix || library_prefix || readable_prefix)
                && prefix.exists()
            {
                let _ = std::fs::remove_dir_all(&prefix);
            }
        }

        // Le manifeste a suivi les jeux : `Steam` pour la bibliothèque gérée,
        // `steamapps` pour une vraie bibliothèque Steam.
        for manifest in [
            game.library
                .join("Steam")
                .join(format!("appmanifest_{appid}.acf")),
            game.library
                .join("steamapps")
                .join(format!("appmanifest_{appid}.acf")),
        ] {
            if manifest.is_file() {
                let _ = std::fs::remove_file(&manifest);
            }
        }
        self.store
            .remove_game(appid)
            .map_err(|e| lapinux_core::Error::Other(e.to_string()))?;
        // Pas de scan complet ici : seule l'entrée désinstallée doit bouger,
        // les autres jeux de la bibliothèque restent tels quels.
        tracing::info!(
            "jeu {appid} désinstallé : {} · préfixe et manifeste supprimés",
            dir.display()
        );
        Ok(Envelope::ok(&serde_json::json!({
            "appid": appid,
            "removed": dir.display().to_string(),
        })))
    }
}
