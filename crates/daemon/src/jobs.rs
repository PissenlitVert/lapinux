//! Téléchargements (jeux, Proton, outils) et réglages persistés.

use crate::{Result, Shared};
use lapinux_core::config::Config;
use lapinux_core::ipc::Envelope;
use lapinux_core::model::{GameSettings, Job, JobKind, JobState};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

impl Shared {
    pub fn save_settings(&self, appid: u32, settings: GameSettings) -> Result<Envelope> {
        let mut game = self
            .store
            .game(appid)
            .map_err(|e| lapinux_core::Error::Other(e.to_string()))?
            .ok_or_else(|| lapinux_core::Error::Other("jeu introuvable".into()))?;
        game.settings = settings;
        self.store
            .save_game(&game)
            .map_err(|e| lapinux_core::Error::Other(e.to_string()))?;
        Ok(Envelope::ok(&game.settings))
    }

    pub fn save_config(&self, config: Config) -> Result<Envelope> {
        let config = config.normalized();
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
        Ok(Envelope::ok(&config))
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
            .ok_or_else(|| lapinux_core::Error::Other(format!("asset {asset} introuvable")))?;
        let destination = proton_install_dir(self);
        let job_id = format!("proton-{}-{}", release.asset, lapinux_core::util::now_unix());
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
            message: "téléchargement".into(),
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
                        job.message = "téléchargement et vérification".into();
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
                            format!("installé : {}", path.display())
                        } else {
                            format!(
                                "installé : {} · {} ancien(s) retiré(s) : {}",
                                path.display(),
                                replaced.len(),
                                replaced.join(", ")
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
            .ok_or_else(|| lapinux_core::Error::Other(format!("Proton {name} introuvable")))?;
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
            return Err(lapinux_core::Error::Other("tâche inconnue".into()));
        };
        if matches!(
            job.state,
            JobState::Done
                | JobState::Failed
                | JobState::Cancelled
                | JobState::Interrupted
        ) {
            return Err(lapinux_core::Error::Other(
                "cette tâche est déjà terminée".into(),
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
        if job.state == JobState::Queued {
            self.forget_job(id);
            self.pump_install_queue();
            return Ok(Envelope::ok(
                &serde_json::json!({ "message": "tâche retirée de la file" }),
            ));
        }
        Err(lapinux_core::Error::Other("tâche inconnue".into()))
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
            .ok_or_else(|| lapinux_core::Error::Other("tâche inconnue".into()))?;
        if !matches!(
            job.state,
            JobState::Done | JobState::Failed | JobState::Cancelled
        ) {
            return Err(lapinux_core::Error::Other(
                "ce téléchargement n'est pas terminé".into(),
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
                self.set_aside_running(&id, "en pause : un autre jeu est passé devant");
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
        let flag = self
            .pause
            .lock()
            .ok()
            .and_then(|map| map.get(id).cloned());
        let Some(flag) = flag else {
            return Err(lapinux_core::Error::Other("tâche inconnue".into()));
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
                self.set_aside_running(&other, "en pause : un autre jeu a repris le créneau");
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
                        "en pause".to_string()
                    } else {
                        "reprise".to_string()
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
    let Some(name) = installed.file_name().map(|name| name.to_string_lossy().to_string()) else {
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
