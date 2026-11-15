//! Installation native des dépôts et téléchargement des chunks.

use super::*;
use lapinux_core::i18n::{t, t_args};

fn decompress_manifest(data: &[u8]) -> Result<Vec<u8>> {
    if data.len() > 2 && data[0] == 0x50 && data[1] == 0x4b {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(data))
            .map_err(|error| Error::Other(format!("manifeste ZIP : {error}")))?;
        let mut file = archive
            .by_index(0)
            .map_err(|error| Error::Other(format!("manifeste ZIP vide : {error}")))?;
        let mut out = Vec::new();
        file.read_to_end(&mut out)?;
        Ok(out)
    } else {
        Ok(data.to_vec())
    }
}

/// Dépôt prêt à télécharger : manifeste déjà lu, donc son poids exact est
/// connu avant le premier octet.
struct PreparedDepot {
    index: usize,
    depot_id: DepotId,
    key: DepotKey,
    manifest: DepotManifest,
    total: u64,
    cdn_auth_token: Option<String>,
    /// Application dont ce dépôt vient : le jeu de base, ou le contenu
    /// additionnel coché qui l'apporte.
    from_app: Option<u32>,
}

/// Fetcher de suivi : compte les requêtes en vol, journalise chaque chunk et
/// coupe une requête dès la pause ou l'annulation. Les octets réseau sont
/// comptés par le fetcher CDN au fil de la lecture, pas ici : un chunk bridé
/// peut durer longtemps et la jauge doit avancer avant sa fin.
pub(super) struct CountingFetcher<F> {
    inner: F,
    /// Compteur réseau partagé, lu pour le cumul du journal ; il est mis à
    /// jour par le fetcher CDN.
    fetched: Arc<AtomicU64>,
    in_flight: Arc<AtomicUsize>,
    log: DownloadLog,
    depot: u32,
    cancel: Arc<AtomicBool>,
    pause: Arc<AtomicBool>,
}

impl<F> CountingFetcher<F> {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        inner: F,
        fetched: Arc<AtomicU64>,
        in_flight: Arc<AtomicUsize>,
        log: DownloadLog,
        depot: u32,
        cancel: Arc<AtomicBool>,
        pause: Arc<AtomicBool>,
    ) -> Self {
        Self {
            inner,
            fetched,
            in_flight,
            log,
            depot,
            cancel,
            pause,
        }
    }
}

/// Début du hachage d'un chunk, assez pour le retrouver dans un journal sans
/// écrire quarante caractères hexadécimaux par ligne.
fn chunk_tag(chunk_id: &ChunkId) -> String {
    chunk_id.0[..6]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Attend qu'une pause soit levée ou qu'une annulation arrive. Sert à
/// interrompre une requête déjà en cours au lieu de la laisser finir : sans
/// cela, une pause attendait le dernier chunk en vol, parfois vingt secondes.
async fn wait_until_resumed(cancel: &AtomicBool, pause: &AtomicBool) {
    loop {
        if cancel.load(Ordering::Relaxed) || pause.load(Ordering::Relaxed) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

impl<F: ChunkFetcher> ChunkFetcher for CountingFetcher<F> {
    async fn fetch_chunk(
        &self,
        depot_id: DepotId,
        chunk_id: &ChunkId,
    ) -> std::result::Result<Bytes, BoxError> {
        // Chaque chunk demandé au CDN laisse une trace datée : c'est ce qui
        // permet de vérifier, horodatage en main, qu'aucune requête ne part
        // après une pause ou une annulation.
        let tag = chunk_tag(chunk_id);
        let mut attempt = 0u32;
        loop {
            // La bibliothèque lance les chunks d'un fichier dans des tâches
            // détachées : arrêter de sonder la progression ne les arrête pas.
            // Chaque tâche attend ici avant de toucher au réseau.
            let mut waiting = false;
            loop {
                if self.cancel.load(Ordering::Relaxed) {
                    self.log.write(format!(
                        "chunk ✗ dépôt {} · {tag} abandonné · annulation demandée",
                        self.depot
                    ));
                    return Err("téléchargement annulé".into());
                }
                if !self.pause.load(Ordering::Relaxed) {
                    break;
                }
                if !waiting {
                    waiting = true;
                    self.log.write(format!(
                        "chunk ⋯ dépôt {} · {tag} · pause active, requête retenue",
                        self.depot
                    ));
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }

            attempt += 1;
            let running = self.in_flight.fetch_add(1, Ordering::Relaxed) + 1;
            let started = Instant::now();
            self.log.write(format!(
                "chunk → dépôt {} · {tag} · essai {attempt} · {running} en vol",
                self.depot
            ));
            // Pause ou annulation coupe la requête en cours dans les 50 ms :
            // le corps déjà reçu est perdu, mais plus rien n'est téléchargé.
            let outcome: Option<std::result::Result<Bytes, BoxError>> = tokio::select! {
                biased;
                result = self.inner.fetch_chunk(depot_id, chunk_id) => Some(result),
                _ = wait_until_resumed(&self.cancel, &self.pause) => None,
            };
            self.in_flight.fetch_sub(1, Ordering::Relaxed);
            let elapsed = started.elapsed().as_millis() as u64;
            match outcome {
                Some(result) => {
                    match &result {
                        Ok(data) => {
                            let bytes = data.len() as u64;
                            // Le compteur est tenu par le fetcher, au fil des
                            // octets reçus : ne pas l'incrémenter une seconde
                            // fois ici, le débit serait doublé.
                            let total = self.fetched.load(Ordering::Relaxed);
                            self.log.write(format!(
                                "chunk ← dépôt {} · {tag} · {} Kio en {elapsed} ms ({:.2} Mio/s) · cumul {} o",
                                self.depot,
                                bytes / 1024,
                                download_log::rate_mib(bytes, elapsed),
                                total
                            ));
                        }
                        Err(error) => {
                            self.log.write(format!(
                                "chunk ✗ dépôt {} · {tag} · échec après {elapsed} ms · {error}",
                                self.depot
                            ));
                        }
                    }
                    return result;
                }
                None => {
                    if self.cancel.load(Ordering::Relaxed) {
                        self.log.write(format!(
                            "chunk ✗ dépôt {} · {tag} · interrompu après {elapsed} ms par l'annulation",
                            self.depot
                        ));
                        return Err("téléchargement annulé".into());
                    }
                    self.log.write(format!(
                        "chunk ⏸ dépôt {} · {tag} · interrompu après {elapsed} ms par la pause",
                        self.depot
                    ));
                }
            }
        }
    }
}

/// Télécharge une application dans une bibliothèque Lapinux.
///
/// Le jeton de refresh Lapinux remplace totalement SteamCMD : authentification
/// CM, clés de dépôts, manifestes et chunks passent par notre client. La
/// progression rapportée est globale au build : les compteurs ne repartent
/// jamais de zéro d'un dépôt à l'autre.
pub fn install(
    account: &str,
    refresh_token: &str,
    appid: u32,
    dlc: &[u32],
    // Version demandée (`linux`, `windows`, `macos`), ou `None` pour la
    // préférence de Lapinux (Windows sous Proton, repli Linux).
    platform_choice: Option<&str>,
    // Langue de doublage demandée pour ce jeu ; `None` suit les réglages ou le
    // client Steam.
    preferred_language: Option<&str>,
    library_root: &Path,
    // Dossier qui reçoit les jeux : `Steam` dans une bibliothèque gérée, ou
    // `steamapps/common` pour une vraie bibliothèque Steam.
    games_root: &Path,
    cancel: Arc<AtomicBool>,
    pause: Arc<AtomicBool>,
    log: &DownloadLog,
    limit: DownloadLimit,
    // Serveur CDN préféré (hôte de l'annuaire Steam) ; `None` laisse Steam
    // ordonner ses serveurs, comme son client.
    preferred_server: Option<&str>,
    report: &mut dyn FnMut(InstallEvent),
) -> Result<InstallOutcome> {
    // Le journal complet (voir `download_log`) : une ligne par chunk, un point
    // par seconde avec le débit moteur et celui de la machine.
    let log = log.clone();
    // Profondeur fixée au démarrage : elle dépend du plafond choisi, pas de
    // la tâche, et reste stable pendant tout le transfert.
    let concurrency = chunk_downloads(&limit);
    log.write(format!(
        "install appid={appid} compte={account} bibliothèque={} · {} téléchargements simultanés",
        library_root.display(),
        concurrency
    ));
    let mbit = |rate: u64| rate as f64 * 8.0 / 1_000_000.0;
    log.write(format!(
        "plafonds · global {} · store {}",
        limit
            .global
            .as_ref()
            .map(|limiter| format!("{:.1} Mb/s", mbit(limiter.rate())))
            .unwrap_or_else(|| "illimité".into()),
        limit
            .store
            .as_ref()
            .map(|limiter| format!("{:.1} Mb/s", mbit(limiter.rate())))
            .unwrap_or_else(|| "illimité".into())
    ));
    let started = Instant::now();
    let log_job = log.clone();
    let rt = runtime()?;
    // Première étape signalée tout de suite : l'interface affiche la vraie
    // phase pendant la poignée de main CM, au lieu d'un 0 % immobile.
    report(InstallEvent::Status {
        message: t("downloads.job_connecting_steam").into(),
    });
    let result = rt.block_on(async move {
        // Une seule connexion Steam à la fois : les lectures de fiches
        // attendent la fin du téléchargement au lieu de rouvrir une session
        // qui ferait fermer celle-ci par Steam (erreur « disconnected »).
        let _install = crate::shared::begin_install().await;
        let client = login(account, refresh_token).await?;
        log_job.write("CM Steam connecté, lecture de l'appinfo");
        report(InstallEvent::Status {
            message: t("downloads.job_reading_appinfo").into(),
        });
        let (mut meta, root) = product(appid, &client).await?;
        let build_id = public_build_id(&root);
        let installdir = safe_installdir(&meta, appid);
        meta.installdir = installdir.clone();
        if meta.name.trim().is_empty() {
            meta.name = format!("App {appid}");
        }
        if meta.kind.eq_ignore_ascii_case("dlc")
            || meta.kind.eq_ignore_ascii_case("tool")
            || meta.kind.eq_ignore_ascii_case("demo")
        {
            return Err(Error::Other(t_args("downloads.err_not_installable", &[("name", &meta.name)])));
        }
        report(InstallEvent::Metadata { meta: meta.clone() });

        let target = games_root.join(&installdir);
        std::fs::create_dir_all(&target)?;
        log_job.write(format!(
            "jeu « {} » · build {} · dossier {}",
            meta.name,
            build_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "inconnu".into()),
            target.display()
        ));
        // La langue demandée pour ce jeu prime sur celle des réglages, qui
        // prime elle-même sur le client Steam et le bureau. Le nom Steam est
        // ramené à la table partagée : un code inconnu n'impose rien.
        let language: Option<&'static str> = preferred_language
            .and_then(lapinux_core::lang::find)
            .map(|language| language.code)
            .or_else(default_language);
        let (platform, mut plans) = pick_platform(
            app_section(&root),
            language,
            false,
            platform_choice,
        );
        if plans.is_empty() {
            return Err(Error::Other(t_args("downloads.err_no_depot", &[("name", &meta.name)])));
        }
        // Contenus additionnels demandés : leurs dépôts rejoignent ceux du jeu
        // et s'installent dans le même dossier, puisque c'est là que Steam les
        // attend. Un DLC non possédé refuse sa clé de dépôt et se saute plus
        // loin, sans interrompre le reste.
        let mut extras: Vec<u32> = Vec::new();
        for id in dlc.iter().copied().filter(|id| *id != appid) {
            report(InstallEvent::Status {
                message: t_args("downloads.job_reading_content", &[("id", &id.to_string())]),
            });
            let (dlc_meta, dlc_root) = match product(id, &client).await {
                Ok(found) => found,
                Err(error) => {
                    log_job.write(format!("contenu {id} ignoré : {error}"));
                    continue;
                }
            };
            let (dlc_platform, dlc_plans) = pick_platform(
                app_section(&dlc_root),
                language,
                true,
                Some(platform),
            );
            if dlc_plans.is_empty() {
                log_job.write(format!(
                    "contenu « {} » ({id}) sans dépôt {} à télécharger",
                    dlc_meta.name, dlc_platform
                ));
                continue;
            }
            log_job.write(format!(
                "contenu « {} » ({id}) · {dlc_platform} · {} partie(s) : {:?}",
                dlc_meta.name,
                dlc_plans.len(),
                dlc_plans.iter().map(|depot| depot.depot_id).collect::<Vec<_>>()
            ));
            extras.push(id);
            plans.extend(dlc_plans);
        }
        plans.sort_by_key(|plan| plan.depot_id);
        plans.dedup_by_key(|plan| plan.depot_id);
        log_job.write(format!(
            "plateforme {platform} · {} partie(s) retenue(s) : {:?} · doublage {} · {} contenu(s) additionnel(s)",
            plans.len(),
            plans.iter().map(|depot| depot.depot_id).collect::<Vec<_>>(),
            language.unwrap_or("aucun"),
            extras.len()
        ));

        let mut cdn_servers = client
            .get_cdn_servers(CellId(0), Some(20))
            .await
            .map_err(steam_error)?;
        if cdn_servers.is_empty() {
            return Err(Error::Other(t("downloads.err_no_cdn").into()));
        }
        // Le serveur choisi passe en tête ; les autres restent en repli, pour
        // qu'un cache régional en panne n'arrête pas le téléchargement.
        if let Some(wanted) = preferred_server.filter(|host| !host.trim().is_empty()) {
            if let Some(index) = cdn_servers.iter().position(|server| server.host == wanted) {
                let chosen = cdn_servers.remove(index);
                cdn_servers.insert(0, chosen);
                log_job.write(format!("serveur CDN demandé : {wanted}"));
            } else {
                log_job.write(format!(
                    "serveur CDN {wanted} absent de la liste : ordre de Steam conservé"
                ));
            }
        }
        log_job.write(format!(
            "{} serveur(s) CDN, premier : {}",
            cdn_servers.len(),
            cdn_servers
                .first()
                .map(|candidate| candidate.host.as_str())
                .unwrap_or("aucun")
        ));

        // Premier serveur retenu : c'est lui que l'interface affiche à côté de
        // la vitesse réseau. Les chunks restent répartis sur le pool.
        let server = cdn_servers
            .first()
            .map(|candidate| candidate.host.clone())
            .unwrap_or_default();
        let cdn = CdnClient::new().map_err(steam_error)?;

        // Première passe : lire tous les manifestes avant de télécharger. Elle
        // donne le poids exact du build et permet une progression globale dès
        // le premier chunk, au lieu de compteurs qui repartent de zéro à
        // chaque dépôt.
        report(InstallEvent::Status {
            message: t("downloads.job_reading_manifests").into(),
        });
        let mut prepared: Vec<PreparedDepot> = Vec::new();
        let mut last_error = None;
        for (index, depot) in plans.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Other(t("downloads.err_install_cancelled").into()));
            }
            let depot_id = DepotId(depot.depot_id);
            let key_app = AppId(depot.from_app.unwrap_or(appid));
            let key: DepotKey = match client
                .get_depot_decryption_key(depot_id, key_app)
                .await
            {
                Ok(key) => key,
                Err(error) => {
                    // Un dépôt DLC non possédé répond « accès refusé » : ce
                    // n'est pas une raison pour annuler le reste du jeu.
                    tracing::debug!("dépôt {} ignoré : {error}", depot.depot_id);
                    last_error = Some(format!("dépôt {} : {error}", depot.depot_id));
                    continue;
                }
            };
            report(InstallEvent::Status {
                message: t_args("downloads.job_manifest", &[("index", &(index + 1).to_string()), ("total", &plans.len().to_string())]),
            });
            let manifest_id = ManifestId(depot.manifest_id);
            let request_code = client
                .get_manifest_request_code(key_app, depot_id, manifest_id, Some("public"), None)
                .await
                .ok()
                .flatten()
                .unwrap_or(0);
            let cdn_auth_token = client
                .get_cdn_auth_token(key_app, depot_id, &cdn_servers[0].host)
                .await
                .ok()
                .and_then(|token| token.token);
            let pool = CdnServerPool::new(cdn_servers.clone());
            let raw = cdn
                .download_manifest_pooled(
                    &pool,
                    depot_id,
                    manifest_id,
                    request_code,
                    cdn_auth_token.as_deref(),
                )
                .await
                .map_err(steam_error)?;
            let manifest_bytes = decompress_manifest(&raw)?;
            let mut manifest = DepotManifest::parse(&manifest_bytes).map_err(steam_error)?;
            if manifest.filenames_encrypted {
                let _ = manifest.decrypt_filenames(&key);
            }
            let total = manifest.files.iter().map(|file| file.size).sum();
            log_job.write(format!(
                "dépôt {} · manifeste {} · {} Kio compressés · {} fichiers · {} o",
                depot.depot_id,
                depot.manifest_id,
                raw.len() / 1024,
                manifest.files.len(),
                total
            ));
            // Numérotation des dépôts réellement préparés : un dépôt sans
            // clé (DLC non possédé) est écarté, et l'index du plan affichait
            // alors « dépôt 2/1 ».
            prepared.push(PreparedDepot {
                index: prepared.len(),
                depot_id,
                key,
                manifest,
                total,
                cdn_auth_token,
                from_app: depot.from_app,
            });
        }

        if prepared.is_empty() {
            return Err(Error::Other(t_args(
                "downloads.err_no_depot_prepared",
                &[
                    ("name", &meta.name),
                    ("details", &last_error.map(|error| format!(" ({error})")).unwrap_or_default()),
                ],
            )));
        }
        let depot_count = prepared.len();
        let grand_total: u64 = prepared.iter().map(|depot| depot.total).sum();
        log_job.write(format!(
            "préparation terminée : {} dépôt(s), {} o à télécharger",
            depot_count, grand_total
        ));
        let network_counter = Arc::new(AtomicU64::new(0));
        let mut bytes = 0u64;
        let mut files = 0u64;
        let mut downloaded = 0u64;
        let mut downloaded_depots = 0usize;
        // Applications dont les dépôts sont allés au bout : le jeu de base et
        // les contenus additionnels réellement installés.
        let mut installed_apps: HashSet<u32> = HashSet::new();
        let mut last_report = Instant::now() - Duration::from_secs(1);
        report(InstallEvent::Progress {
            depot: 0,
            depots: depot_count,
            fraction: 0.0,
            downloaded: 0,
            total: grand_total,
            network: 0,
            written: 0,
            server: server.clone(),
            label: "démarrage".into(),
        });

        for depot in prepared {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Other(t("downloads.err_install_cancelled").into()));
            }
            let (events, mut progress) = tokio::sync::mpsc::unbounded_channel();
            let job = DepotJob::builder()
                .depot_id(depot.depot_id)
                .depot_key(depot.key)
                .install_dir(target.clone())
                .verify(true)
                .max_downloads(concurrency)
                .event_sender(events)
                .build()
                .map_err(steam_error)?;
            let in_flight = Arc::new(AtomicUsize::new(0));
            let fetcher = CountingFetcher::new(
                StreamingFetcher::new(
                    CdnServerPool::new(cdn_servers.clone()),
                    depot.cdn_auth_token,
                    limit.clone(),
                    Arc::clone(&network_counter),
                ),
                Arc::clone(&network_counter),
                Arc::clone(&in_flight),
                log_job.clone(),
                depot.depot_id.0,
                Arc::clone(&cancel),
                Arc::clone(&pause),
            );
            // Taille de chaque fichier du manifeste : les fichiers déjà
            // présents que la vérification saute comptent quand même dans
            // l'avancement, sinon la mise à jour reste à 0 % pendant tout le
            // hachage de l'installation existante.
            let file_sizes: std::collections::HashMap<&str, u64> = depot
                .manifest
                .files
                .iter()
                .map(|file| (file.filename.as_str(), file.size))
                .collect();
            report(InstallEvent::Status {
                message: t_args("downloads.job_verifying_files", &[("part", &(depot.index + 1).to_string()), ("total", &depot_count.to_string())]),
            });
            log_job.write(format!(
                "dépôt {} · démarrage · {} fichiers · {} o à écrire",
                depot.depot_id,
                depot.manifest.files.len(),
                depot.total
            ));
            let network_before = network_counter.load(Ordering::Relaxed);
            let download = job.download(&depot.manifest, Arc::new(fetcher));
            tokio::pin!(download);
            let mut depot_written = 0u64;
            let mut depot_total = depot.total;
            let mut verified = 0u64;
            let mut ticker = tokio::time::interval(Duration::from_millis(150));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            // Échantillonnage : le débit vu par le moteur (chunks qu'il a
            // demandés) et celui reçu par la machine, pour distinguer un
            // téléchargement d'un autre trafic.
            let mut last_paused = pause.load(Ordering::Relaxed);
            let mut last_sample = Instant::now();
            let mut sampled_network = network_counter.load(Ordering::Relaxed);
            let mut sampled_rx = download_log::system_rx_bytes();
            let mut last_progress_at = Instant::now();
            let mut last_stall_log = Instant::now();
            loop {
                let paused = pause.load(Ordering::Relaxed);
                tokio::select! {
                    biased;
                    _ = ticker.tick() => {
                        if cancel.load(Ordering::Relaxed) {
                            log_job.write(format!(
                                "annulation lue par le moteur · dépôt {} · {} chunk(s) en vol · {} o reçus",
                                depot.depot_id,
                                in_flight.load(Ordering::Relaxed),
                                network_counter.load(Ordering::Relaxed)
                            ));
                            return Err(Error::Other(t("downloads.err_install_cancelled").into()));
                        }
                        let paused_now = pause.load(Ordering::Relaxed);
                        if paused_now != last_paused {
                            log_job.write(format!(
                                "pause {} · {} chunk(s) déjà en vol se terminent · {} o reçus",
                                if paused_now { "activée" } else { "levée" },
                                in_flight.load(Ordering::Relaxed),
                                network_counter.load(Ordering::Relaxed)
                            ));
                            last_paused = paused_now;
                            last_sample = Instant::now();
                            sampled_network = network_counter.load(Ordering::Relaxed);
                            sampled_rx = download_log::system_rx_bytes();
                        }
                        if last_sample.elapsed() >= Duration::from_secs(1) {
                            let elapsed_ms = last_sample.elapsed().as_millis() as u64;
                            let network = network_counter.load(Ordering::Relaxed);
                            let rx = download_log::system_rx_bytes();
                            let motor_delta = network.saturating_sub(sampled_network);
                            let rx_delta = match (sampled_rx, rx) {
                                (Some(before), Some(after)) => Some(after.saturating_sub(before)),
                                _ => None,
                            };
                            log_job.write(format!(
                                "tick · dépôt {}/{} · {} · en vol {} · moteur +{} o ({:.2} Mio/s){}",
                                depot.index + 1,
                                depot_count,
                                if paused_now { "EN PAUSE" } else { "téléchargement" },
                                in_flight.load(Ordering::Relaxed),
                                motor_delta,
                                download_log::rate_mib(motor_delta, elapsed_ms),
                                match rx_delta {
                                    Some(delta) => format!(
                                        " · machine +{} o ({:.2} MiB/s)",
                                        delta,
                                        download_log::rate_mib(delta, elapsed_ms)
                                    ),
                                    None => String::new(),
                                }
                            ));
                            if motor_delta > 0 {
                                last_progress_at = Instant::now();
                            }
                            last_sample = Instant::now();
                            sampled_network = network;
                            sampled_rx = rx;
                        }
                        // Un moteur qui ne demande plus rien alors qu'il n'est
                        // pas en pause est le signal d'un blocage local.
                        let silent = !paused_now
                            && in_flight.load(Ordering::Relaxed) == 0
                            && last_progress_at.elapsed() >= Duration::from_secs(4);
                        if silent && last_stall_log.elapsed() >= Duration::from_secs(10) {
                            last_stall_log = Instant::now();
                            log_job.write(format!(
                                "aucun chunk depuis {} s (dépôt {}) : vérification locale ou attente réseau",
                                last_progress_at.elapsed().as_secs(),
                                depot.depot_id
                            ));
                        }
                    }
                    result = &mut download, if !paused => {
                        let stats = result.map_err(steam_error)?;
                        let network_used = network_counter
                            .load(Ordering::Relaxed)
                            .saturating_sub(network_before);
                        log_job.write(format!(
                            "dépôt {} terminé · {} o récupérés sur le réseau · {} o traités (réutilisés compris) · {} fichiers · {} en vol",
                            depot.depot_id,
                            network_used,
                            stats.bytes_downloaded,
                            stats.files_completed,
                            in_flight.load(Ordering::Relaxed)
                        ));
                        bytes += stats.bytes_downloaded;
                        files += stats.files_completed;
                        downloaded = downloaded.saturating_add(depot.total).min(grand_total);
                        downloaded_depots += 1;
                        installed_apps.insert(depot.from_app.unwrap_or(appid));
                        let fraction = if grand_total == 0 {
                            1.0
                        } else {
                            downloaded as f32 / grand_total as f32
                        };
                        report(InstallEvent::Progress {
                            depot: depot.index,
                            depots: depot_count,
                            fraction: fraction.clamp(0.0, 1.0),
                            downloaded,
                            total: grand_total,
                            network: network_counter.load(Ordering::Relaxed),
                            written: downloaded,
                            server: server.clone(),
                            label: format!("partie {} terminée", depot.index + 1),
                        });
                        break;
                    }
                    Some(event) = progress.recv() => {
                        match event {
                            DownloadEvent::DownloadStarted { total_bytes, .. } => {
                                depot_total = total_bytes;
                            }
                            // Les chunks font avancer la tuile en continu : la
                            // bibliothèque ne signale sinon qu'à la fin de
                            // chaque fichier, ce qui paraissait bloqué.
                            DownloadEvent::ChunkCompleted { bytes: chunk } => {
                                depot_written = depot_written.saturating_add(chunk);
                                // Pendant une pause, l'interface garde « en pause » :
                                // les chunks déjà en vol ne doivent pas réécrire
                                // « téléchargement » ni relancer l'avancement.
                                if !pause.load(Ordering::Relaxed)
                                    && last_report.elapsed() >= Duration::from_millis(120)
                                {
                                    last_report = Instant::now();
                                    let done = downloaded
                                        .saturating_add(depot_written.min(depot_total))
                                        .min(grand_total);
                                    let fraction = if grand_total == 0 {
                                        0.0
                                    } else {
                                        (done as f32 / grand_total as f32).clamp(0.0, 1.0)
                                    };
                                    report(InstallEvent::Progress {
                                        depot: depot.index,
                                        depots: depot_count,
                                        fraction,
                                        downloaded: done,
                                        total: grand_total,
                                        network: network_counter.load(Ordering::Relaxed),
                                        written: done,
                                        server: server.clone(),
                                        label: t_args("downloads.job_downloading_part", &[("part", &(depot.index + 1).to_string()), ("total", &depot_count.to_string())]),
                                    });
                                }
                            }
                            DownloadEvent::DepotProgress {
                                completed_bytes,
                                total_bytes,
                            } => {
                                depot_total = total_bytes;
                                depot_written = completed_bytes;
                            }
                            DownloadEvent::FileSkipped { filename } => {
                                verified += 1;
                                depot_written = depot_written.saturating_add(
                                    file_sizes.get(filename.as_str()).copied().unwrap_or(0),
                                );
                                if last_report.elapsed() >= Duration::from_millis(200) {
                                    last_report = Instant::now();
                                    let done = downloaded
                                        .saturating_add(depot_written.min(depot_total))
                                        .min(grand_total);
                                    let fraction = if grand_total == 0 {
                                        0.0
                                    } else {
                                        (done as f32 / grand_total as f32).clamp(0.0, 1.0)
                                    };
                                    report(InstallEvent::Progress {
                                        depot: depot.index,
                                        depots: depot_count,
                                        fraction,
                                        downloaded: done,
                                        total: grand_total,
                                        network: network_counter.load(Ordering::Relaxed),
                                        written: done,
                                        server: server.clone(),
                                        label: t_args("downloads.job_verifying_present", &[("count", &verified.to_string())]),
                                    });
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        if downloaded_depots == 0 {
            return Err(Error::Other(t_args(
                "downloads.err_no_depot_downloaded",
                &[
                    ("name", &meta.name),
                    ("details", &last_error.map(|error| format!(" ({error})")).unwrap_or_default()),
                ],
            )));
        }
        let size = directory_size(&target);
        log_job.write(format!(
            "tous les dépôts terminés · {} o réseau · {} fichiers · {} o sur disque",
            bytes, files, size
        ));
        let mut apps: Vec<u32> = installed_apps.into_iter().collect();
        apps.sort_unstable();
        Ok(InstallOutcome {
            meta,
            target,
            bytes,
            files,
            size,
            build_id,
            apps,
        })
    });
    match &result {
        Ok(outcome) => log.write(format!(
            "installation appid={appid} réussie en {} s · {} o réseau · {} fichiers",
            started.elapsed().as_secs(),
            outcome.bytes,
            outcome.files
        )),
        Err(error) => log.write(format!(
            "installation appid={appid} interrompue après {} s · {error}",
            started.elapsed().as_secs()
        )),
    }
    drop(rt);
    log.write("runtime tokio libéré : connexions CDN fermées, plus aucune requête en cours");
    result
}
