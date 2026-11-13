//! Lecture et synchronisation des sauvegardes Steam Cloud.

use super::*;

impl Shared {
    /// Sauvegardes Steam Cloud d'un jeu, lues par la session enregistrée : la
    /// liste des fichiers gardés, le quota consommé et — quand une partie vient
    /// de se terminer — ce qu'elle a changé par rapport au lancement.
    pub fn steam_cloud(self: &Arc<Self>, appid: u32) -> Result<Envelope> {
        let shared = Arc::clone(self);
        let envelope = self.cached_remote_read("cloud", appid, move || {
            let mut report = lapinux_core::model::CloudReport::default();
            match shared.read_cloud_result(appid) {
                Ok(saves) => report.saves = saves,
                Err(error) => report.error = Some(error),
            }
            report.diff = shared.cloud_diff_after(appid);
            serde_json::to_value(&report).map_err(|error| {
                lapinux_core::Error::Other(format!("sauvegardes cloud illisibles : {error}"))
            })
        })?;
        // La liste distante peut venir du cache ; l'état local, lui, change
        // dès qu'une reprise écrit un fichier. Il est donc recalculé à chaque
        // lecture au lieu d'être servi avec une ancienne réponse.
        let mut report = envelope
            .into_data::<lapinux_core::model::CloudReport>()
            .unwrap_or_default();
        self.mark_cloud_locals(appid, &mut report);
        report.syncs = crate::cloud_journal::history(&self.paths, appid);
        Ok(Envelope::ok(&report))
    }

    /// Lecture réseau sans cache, utilisée avant de proposer une décision.
    pub fn steam_cloud_fresh(&self, appid: u32) -> Result<Envelope> {
        let mut report = lapinux_core::model::CloudReport::default();
        match self.read_cloud_result(appid) {
            Ok(saves) => report.saves = saves,
            Err(error) => report.error = Some(error),
        }
        self.mark_cloud_locals(appid, &mut report);
        report.syncs = crate::cloud_journal::history(&self.paths, appid);
        Ok(Envelope::ok(&report))
    }

    /// Remplit l'état local de chaque fichier cloud : présent ou non dans le
    /// préfixe du jeu, et à quelle date. C'est ce qui distingue « à récupérer »
    /// de « déjà là » dans les réglages du jeu.
    fn mark_cloud_locals(&self, appid: u32, report: &mut lapinux_core::model::CloudReport) {
        let Some(game) = self.store.game(appid).ok().flatten() else {
            return;
        };
        // Le relevé peut avoir été lu dans le cache : ses états locaux sont
        // repartis de zéro avant d'être vérifiés sur le disque.
        report.local_present = 0;
        report.local_dir = None;
        let mut dirs: Vec<std::path::PathBuf> = Vec::new();
        for file in &mut report.saves.files {
            // Une réponse en cache peut porter un ancien état local : un
            // chemin devenu introuvable doit repartir de « absent ».
            file.local = None;
            file.local_resolved = false;
            file.local_sha = None;
            let Some(path) = cloud_file_path(&game, file, &self.paths.data_dir) else {
                continue;
            };
            match read_cloud_local(&path) {
                Ok(Some(data)) => {
                    file.local_sha = Some(lapinux_steam::cloud::file_sha(&data));
                    file.local = local_modified(&path);
                    file.local_resolved = true;
                }
                Ok(None) => file.local_resolved = true,
                Err(_) => continue,
            }
            if file.local_sha.is_some() {
                report.local_present += 1;
            }
            if let Some(dir) = path.parent() {
                if !dirs.iter().any(|known| known == dir) {
                    dirs.push(dir.to_path_buf());
                }
            }
        }
        report.local_dir = dirs.first().map(|dir| dir.display().to_string());
    }

    /// Récupère du cloud les sauvegardes d'un jeu et les écrit dans son
    /// préfixe, à l'endroit où le jeu les lit.
    ///
    /// Steam ne synchronise rien sans son client : un jeu lancé par Lapinux
    /// démarre donc sans les sauvegardes du compte. Cette restauration les
    /// télécharge par le protocole (le CM rend l'adresse CDN), fichier par
    /// fichier, et remplace la copie locale quand elle diffère.
    pub fn cloud_restore(&self, appid: u32) -> Result<Envelope> {
        let _guard = self.save_operation.try_lock().map_err(|_| {
            lapinux_core::Error::Other(
                "une opération de sauvegarde ou de lancement est déjà en cours".into(),
            )
        })?;
        if self
            .session
            .lock()
            .map(|session| {
                session
                    .as_ref()
                    .map(|state| state.info.appid == appid)
                    .unwrap_or(false)
            })
            .unwrap_or(false)
        {
            return Err(lapinux_core::Error::Other(
                "quittez le jeu avant de récupérer ses sauvegardes".into(),
            ));
        }
        let game = self
            .store
            .game(appid)
            .map_err(|e| lapinux_core::Error::Other(e.to_string()))?
            .ok_or_else(|| lapinux_core::Error::Other("jeu introuvable".into()))?;
        let (written, bytes) = self
            .sync_cloud(&game, appid, false)
            .map_err(|error| lapinux_core::Error::Other(format!("sauvegardes cloud : {error}")))?;
        // La copie locale a changé : la fiche relira plutôt que de garder
        // l'état d'avant.
        if let Ok(mut cache) = self.game_reads.lock() {
            cache.forget("cloud", appid);
        }
        self.forget_remote_sheet("cloud", appid);
        tracing::info!(
            "cloud : {written} sauvegarde(s) récupérée(s) pour {appid} ({} octets)",
            bytes
        );
        Ok(Envelope::ok(&serde_json::json!({
            "files": written,
            "bytes": bytes,
        })))
    }

    /// Synchronise les sauvegardes après la partie. Les fichiers modifiés
    /// partent dans un lot ; un fichier présent au lancement et disparu depuis
    /// est supprimé du cloud au lieu de bloquer l'envoi. Si le cloud a changé
    /// pendant la partie, rien n'est écrasé : le choix reste à l'écran.
    pub(crate) fn sync_cloud_up(
        &self,
        game: &Game,
        appid: u32,
    ) -> std::result::Result<(usize, u64, Option<lapinux_core::model::CloudDiff>, bool), String>
    {
        let config = self.config_snapshot();
        let saved = self.saved_session(&config.steam.username).ok_or_else(|| {
            "connectez votre compte Steam dans Réglages → Compte Steam".to_string()
        })?;
        if saved.refresh_token.trim().is_empty() {
            return Err("la session Lapinux n'a pas de jeton réutilisable".to_string());
        }
        let data_dir = self.paths.data_dir.clone();
        let before = self
            .cloud_before
            .lock()
            .ok()
            .and_then(|mut slot| slot.take())
            .filter(|(id, _)| *id == appid)
            .map(|(_, saves)| saves);
        let result = lapinux_steam::shared::block_on(async move {
            let session = lapinux_steam::shared::session(&saved.account_name, &saved.refresh_token)
                .await
                .map_err(|error| error.to_string())?;
            let cloud = lapinux_steam::cloud::game_cloud(&session, appid)
                .await
                .map_err(|error| error.to_string())?;
            let diff = before
                .as_ref()
                .map(|before| lapinux_core::model::cloud_diff(before, &cloud));
            // Le cloud garde-t-il déjà cette partie ? La réponse guide le
            // réglage qui saute la copie locale des jeux sauvegardés en ligne.
            let present = !cloud.files.is_empty();
            if cloud.files.is_empty() {
                return Ok((0, 0, diff, false));
            }
            if let Some(before) = &before {
                if cloud.change_number != before.change_number {
                    return Err("le cloud a changé pendant la partie : choisissez une version dans l'onglet Sauvegardes".into());
                }
            }
            let mut uploads: Vec<(lapinux_core::model::CloudFile, Vec<u8>)> = Vec::new();
            let mut deletes: Vec<lapinux_core::model::CloudFile> = Vec::new();
            for file in &cloud.files {
                let Some(path) = cloud_file_path(game, file, &data_dir) else {
                    continue;
                };
                let data = read_cloud_local(&path)?;
                if data
                    .as_ref()
                    .is_some_and(|data| data.len() as u64 > lapinux_steam::cloud::MAX_UPLOAD)
                {
                    tracing::warn!("cloud : {} trop gros pour l'envoi", file.name);
                    continue;
                }
                match cloud_action(file, data.as_deref(), before.as_ref()) {
                    CloudAction::Skip => {}
                    CloudAction::IgnoreMissing => tracing::warn!(
                        "cloud : {}{} absent localement et inconnu au lancement ; ignoré",
                        file.prefix,
                        file.name
                    ),
                    CloudAction::Delete => deletes.push(file.clone()),
                    CloudAction::Upload => {
                        let data = data.expect("un envoi a des octets locaux");
                        uploads.push((file.clone(), data));
                    }
                    CloudAction::Conflict => {
                        return Err("sauvegardes différentes ou empreinte inconnue : choisissez une version dans l'onglet Sauvegardes".into());
                    }
                }
            }
            let mut sent = 0usize;
            let mut bytes = 0u64;
            if !uploads.is_empty() {
                let names: Vec<String> = uploads
                    .iter()
                    .map(|(file, _)| format!("{}{}", file.prefix, file.name))
                    .collect();
                let batch =
                    lapinux_steam::cloud::begin_batch(&session, appid, &machine_name(), &names)
                        .await
                        .map_err(|error| error.to_string())?;
                for (file, data) in &uploads {
                    lapinux_steam::cloud::upload_file(&session, appid, file, data, batch)
                        .await
                        .map_err(|error| error.to_string())?;
                    sent += 1;
                    bytes += data.len() as u64;
                }
                lapinux_steam::cloud::complete_batch(&session, appid, batch)
                    .await
                    .map_err(|error| error.to_string())?;
            }
            for file in &deletes {
                match lapinux_steam::cloud::delete_file(&session, appid, file).await {
                    Ok(()) => sent += 1,
                    Err(error) => tracing::warn!(
                        "cloud : suppression de {}{} impossible ({error})",
                        file.prefix,
                        file.name
                    ),
                }
            }
            Ok((sent, bytes, diff, present))
        });
        if let Ok((sent, bytes, _, _)) = &result {
            if *sent > 0 {
                crate::cloud_journal::record(
                    &self.paths,
                    appid,
                    lapinux_core::model::CloudSyncEntry {
                        at: lapinux_core::util::now_unix(),
                        direction: lapinux_core::model::CloudSyncDirection::Upload,
                        machine: machine_name(),
                        files: *sent,
                        bytes: *bytes,
                    },
                );
            }
        }
        result
    }

    /// Envoie à la demande les sauvegardes d'un jeu, depuis ses réglages.
    pub fn cloud_upload(&self, appid: u32) -> Result<Envelope> {
        let _guard = self.save_operation.try_lock().map_err(|_| {
            lapinux_core::Error::Other(
                "une opération de sauvegarde ou de lancement est déjà en cours".into(),
            )
        })?;
        if self
            .session
            .lock()
            .map(|session| {
                session
                    .as_ref()
                    .map(|state| state.info.appid == appid)
                    .unwrap_or(false)
            })
            .unwrap_or(false)
        {
            return Err(lapinux_core::Error::Other(
                "quittez le jeu avant d'envoyer ses sauvegardes".into(),
            ));
        }
        let game = self
            .store
            .game(appid)
            .map_err(|e| lapinux_core::Error::Other(e.to_string()))?
            .ok_or_else(|| lapinux_core::Error::Other("jeu introuvable".into()))?;
        let (sent, bytes, diff, _) = self
            .sync_cloud_up(&game, appid)
            .map_err(|error| lapinux_core::Error::Other(format!("sauvegardes cloud : {error}")))?;
        if let Ok(mut cache) = self.game_reads.lock() {
            cache.forget("cloud", appid);
        }
        self.forget_remote_sheet("cloud", appid);
        if let Some(diff) = diff {
            if let Ok(mut slot) = self.cloud_last.lock() {
                *slot = Some((appid, diff));
            }
        }
        tracing::info!("cloud : {sent} sauvegarde(s) envoyée(s) pour {appid} ({bytes} octets)");
        Ok(Envelope::ok(&serde_json::json!({
            "files": sent,
            "bytes": bytes,
        })))
    }

    /// Résout explicitement une divergence après relecture des deux états.
    pub fn resolve_steam_cloud(
        &self,
        appid: u32,
        choice: lapinux_core::ipc::SteamCloudChoice,
        expected: lapinux_core::model::CloudSaves,
    ) -> Result<Envelope> {
        use lapinux_core::ipc::SteamCloudChoice;
        let _guard = self.save_operation.try_lock().map_err(|_| {
            lapinux_core::Error::Other(
                "une opération de sauvegarde ou de lancement est déjà en cours".into(),
            )
        })?;
        if self.session_info().map(|session| session.appid) == Some(appid) {
            return Err(lapinux_core::Error::Other(
                "fermez le jeu avant de résoudre le conflit".into(),
            ));
        }
        let game = self
            .store
            .game(appid)?
            .ok_or_else(|| lapinux_core::Error::Other("jeu inconnu".into()))?;
        let config = self.config_snapshot();
        let saved = self
            .saved_session(&config.steam.username)
            .ok_or_else(|| lapinux_core::Error::Other("compte Steam non connecté".into()))?;
        let data_dir = self.paths.data_dir.clone();
        let result: std::result::Result<(String, usize), String> = lapinux_steam::shared::block_on(
            async {
                let session =
                    lapinux_steam::shared::session(&saved.account_name, &saved.refresh_token)
                        .await
                        .map_err(|e| e.to_string())?;
                let cloud = lapinux_steam::cloud::game_cloud(&session, appid)
                    .await
                    .map_err(|e| e.to_string())?;
                if !same_cloud_snapshot(&expected, &cloud) || cloud.files.is_empty() {
                    return Err("le cloud a changé depuis la lecture : actualisez la fiche".into());
                }
                let mut local = Vec::new();
                let mut remote = Vec::new();
                let mut destinations = HashSet::new();
                for (file, shown) in cloud.files.iter().zip(&expected.files) {
                    let path = cloud_file_path(&game, file, &data_dir)
                        .ok_or("chemin de sauvegarde inconnu")?;
                    if !destinations.insert(path.clone()) {
                        return Err(
                            "plusieurs fichiers cloud désignent le même chemin local".into()
                        );
                    }
                    let bytes = read_cloud_local(&path)?;
                    if !shown.local_resolved
                        || bytes.as_ref().map(|v| lapinux_steam::cloud::file_sha(v))
                            != shown.local_sha
                    {
                        return Err("les sauvegardes locales ont changé depuis la lecture : actualisez la fiche".into());
                    }
                    if choice == SteamCloudChoice::Local && bytes.is_none() {
                        return Err("un fichier local manque ; impossible de choisir la version locale complète".into());
                    }
                    let remote_bytes = lapinux_steam::cloud::download_file(&session, appid, file)
                        .await
                        .map_err(|e| e.to_string())?;
                    // Steam ne sert qu'une version par nom : celle qui revient
                    // peut être l'une des entrées écartées du même chemin.
                    // L'empreinte est donc vérifiée contre toutes les versions
                    // connues, pas seulement contre la plus récente.
                    let digest = lapinux_steam::cloud::file_sha(&remote_bytes);
                    let known = file.sha == digest
                        || cloud.superseded.iter().any(|old| {
                            old.prefix == file.prefix && old.name == file.name && old.sha == digest
                        });
                    if file.sha.len() != 20 || !known {
                        return Err(format!(
                            "empreinte cloud inconnue ou incorrecte : {}",
                            file.name
                        ));
                    }
                    local.push((path, bytes));
                    remote.push(remote_bytes);
                }
                let recovery_remote: Vec<Option<Vec<u8>>> =
                    remote.iter().cloned().map(Some).collect();
                let backup = save_cloud_recovery(
                    &data_dir,
                    &game,
                    &cloud,
                    &local,
                    &recovery_remote,
                    &cloud.superseded,
                )?;
                let fresh = lapinux_steam::cloud::game_cloud(&session, appid)
                    .await
                    .map_err(|e| e.to_string())?;
                if !same_cloud_snapshot(&cloud, &fresh) {
                    return Err(format!(
                        "le cloud a changé ; aucune résolution appliquée. Copies : {}",
                        backup.display()
                    ));
                }
                for (path, bytes) in &local {
                    if read_cloud_local(path)? != *bytes {
                        return Err(format!("les fichiers locaux ont changé ; aucune résolution appliquée. Copies : {}", backup.display()));
                    }
                }
                if self.session_info().map(|session| session.appid) == Some(appid) {
                    return Err(format!(
                        "le jeu a démarré ; résolution annulée. Copies : {}",
                        backup.display()
                    ));
                }
                let mut count = 0;
                match choice {
                    SteamCloudChoice::Cloud => {
                        for ((path, old), bytes) in local.iter().zip(&remote) {
                            if old.as_ref() != Some(bytes) {
                                write_cloud_file(path, bytes, old.as_deref())
                                    .map_err(|e| format!("{e} ; copies : {}", backup.display()))?;
                                count += 1;
                            }
                        }
                    }
                    SteamCloudChoice::Local => {
                        let changes: Vec<_> = cloud
                            .files
                            .iter()
                            .zip(&local)
                            .zip(&remote)
                            .filter(|((_, (_, local)), remote)| local.as_ref() != Some(*remote))
                            .collect();
                        if !changes.is_empty() {
                            let names: Vec<_> = changes
                                .iter()
                                .map(|((file, _), _)| format!("{}{}", file.prefix, file.name))
                                .collect();
                            let batch = lapinux_steam::cloud::begin_batch(
                                &session,
                                appid,
                                &machine_name(),
                                &names,
                            )
                            .await
                            .map_err(|e| {
                                format!(
                                    "ouverture du lot impossible : {e} ; copies : {}",
                                    backup.display()
                                )
                            })?;
                            for ((file, (_, bytes)), _) in changes {
                                let bytes = bytes.as_ref().ok_or("sauvegarde locale absente")?;
                                lapinux_steam::cloud::upload_file(
                                    &session, appid, file, bytes, batch,
                                )
                                .await
                                .map_err(|e| {
                                    format!(
                                        "envoi interrompu : {e} ; copies : {}",
                                        backup.display()
                                    )
                                })?;
                                count += 1;
                            }
                            let before_commit = lapinux_steam::cloud::game_cloud(&session, appid).await
                            .map_err(|e| format!("vérification du cloud impossible : {e} ; lot non terminé ; copies : {}", backup.display()))?;
                            if !same_cloud_snapshot(&cloud, &before_commit) {
                                return Err(format!("le cloud a changé pendant l'envoi ; lot non terminé ; copies : {}", backup.display()));
                            }
                            lapinux_steam::cloud::complete_batch(&session, appid, batch)
                                .await
                                .map_err(|e| {
                                    format!(
                                        "validation interrompue : {e} ; copies : {}",
                                        backup.display()
                                    )
                                })?;
                        }
                    }
                    SteamCloudChoice::KeepBoth => {}
                }
                Ok((backup.display().to_string(), count))
            },
        );
        let (backup, files) = result.map_err(lapinux_core::Error::Other)?;
        if choice != SteamCloudChoice::KeepBoth && files > 0 {
            crate::cloud_journal::record(
                &self.paths,
                appid,
                lapinux_core::model::CloudSyncEntry {
                    at: lapinux_core::util::now_unix(),
                    direction: if choice == SteamCloudChoice::Local {
                        lapinux_core::model::CloudSyncDirection::Upload
                    } else {
                        lapinux_core::model::CloudSyncDirection::Download
                    },
                    machine: if choice == SteamCloudChoice::Local {
                        machine_name()
                    } else {
                        "Steam Cloud".into()
                    },
                    files,
                    bytes: 0,
                },
            );
        }
        if let Ok(mut cache) = self.game_reads.lock() {
            cache.forget("cloud", appid);
        }
        self.forget_remote_sheet("cloud", appid);
        Ok(Envelope::ok(
            &serde_json::json!({ "backup": backup, "files": files }),
        ))
    }

    /// Prépare le préfixe Proton avant une reprise cloud. Un jeu jamais lancé
    /// n'a pas de `drive_c` : Proton le crée par `wineboot`, sans démarrer le
    /// jeu. En cas d'échec, le chemin est rendu quand même pour les
    /// sauvegardes `%GameInstall%`, qui n'ont pas besoin du préfixe.
    pub(super) fn cloud_prefix(&self, game: &Game) -> std::path::PathBuf {
        let prefix = lapinux_proton::prefix_for(game, &self.paths.data_dir);
        if lapinux_proton::is_initialized(&prefix) {
            return prefix;
        }
        let config = self.config_snapshot();
        let protons = self.protons_cached();
        let wanted = game
            .settings
            .proton
            .clone()
            .unwrap_or_else(|| config.launch.proton.clone());
        let proton = if wanted == "auto" {
            lapinux_proton::recommended(&protons).cloned()
        } else {
            protons.iter().find(|proton| proton.name == wanted).cloned()
        };
        let Some(proton) = proton else {
            tracing::warn!(
                "cloud : aucun Proton utilisable pour préparer le préfixe de {}",
                game.appid
            );
            return prefix;
        };
        tracing::info!(
            "cloud : préparation du préfixe Proton de {} avec {}",
            game.appid,
            proton.name
        );
        if let Err(error) =
            lapinux_proton::initialize_prefix(&prefix, game.appid, &proton, &game.install_dir())
        {
            tracing::warn!(
                "cloud : préfixe Proton de {} non préparé : {error}",
                game.appid
            );
        }
        prefix
    }

    /// Reprend les sauvegardes cloud plus récentes juste après une installation.
    ///
    /// La lecture préalable évite de préparer un préfixe Proton pour un jeu qui
    /// ne publie rien en ligne. Un échec (session, réseau, chemin inconnu) ne
    /// remet pas l'installation en cause : le premier lancement retentera la
    /// reprise de son côté.
    pub(super) fn pull_cloud_after_install(&self, appid: u32) -> Option<String> {
        if !self.config_snapshot().downloads.cloud_saves_after_install {
            return None;
        }
        let game = self.store.game(appid).ok().flatten()?;
        if !game.settings.cloud_sync_enabled() {
            tracing::debug!("cloud : synchronisation automatique désactivée pour {appid}");
            return None;
        }
        match self.read_cloud_result(appid) {
            Ok(saves) if saves.files.is_empty() => {
                tracing::debug!(
                    "cloud : aucune sauvegarde à reprendre après l'installation de {appid}"
                );
                return None;
            }
            Ok(_) => {}
            Err(error) => {
                tracing::debug!(
                    "cloud : lecture impossible après l'installation de {appid} ({error})"
                );
                return None;
            }
        }
        match self.sync_cloud(&game, appid, true) {
            Ok((0, _)) => None,
            Ok((files, bytes)) => {
                // Le relevé affiché par la fiche a été lu avant l'installation :
                // sans cette invalidation, le tag restait « à récupérer » alors
                // que la reprise venait d'écrire les fichiers.
                if let Ok(mut cache) = self.game_reads.lock() {
                    cache.forget("cloud", appid);
                }
                self.forget_remote_sheet("cloud", appid);
                tracing::info!(
                    "cloud : {files} sauvegarde(s) reprise(s) après l'installation de {appid} ({bytes} octets)"
                );
                Some(format!("{files} sauvegarde(s) cloud"))
            }
            Err(error) => {
                tracing::debug!(
                    "cloud : reprise impossible après l'installation de {appid} ({error})"
                );
                None
            }
        }
    }

    /// Reprend dans le préfixe du jeu les sauvegardes que le compte a sur le
    /// cloud.
    ///
    /// `only_newer` ne ramène que les fichiers que le cloud a plus récents que
    /// le disque : c'est ce qui précède un lancement, où une progression locale
    /// déjà jouée ne doit pas être écrasée. Rendre `(0, 0)` veut dire « rien à
    /// reprendre ». Un fichier dont le chemin local est inconnu est ignoré pour
    /// ne pas abandonner la reprise des autres sauvegardes.
    pub(super) fn sync_cloud(
        &self,
        game: &Game,
        appid: u32,
        only_newer: bool,
    ) -> std::result::Result<(usize, u64), String> {
        let config = self.config_snapshot();
        let saved = self.saved_session(&config.steam.username).ok_or_else(|| {
            "connectez votre compte Steam dans Réglages → Compte Steam".to_string()
        })?;
        if saved.refresh_token.trim().is_empty() {
            return Err("la session Lapinux n'a pas de jeton réutilisable".to_string());
        }
        // Un jeu jamais lancé n'a pas encore de `pfx/drive_c` : Proton le crée
        // par `wineboot`, sans démarrer le jeu, pour que les sauvegardes
        // puissent être posées au bon endroit.
        let prefix_dir = self.cloud_prefix(game);
        let result = lapinux_steam::shared::block_on(async move {
            let session = lapinux_steam::shared::session(&saved.account_name, &saved.refresh_token)
                .await
                .map_err(|error| error.to_string())?;
            let saves = lapinux_steam::cloud::game_cloud(&session, appid)
                .await
                .map_err(|error| error.to_string())?;
            if saves.files.is_empty() {
                return Ok((0, 0));
            }
            // Inspecter tous les fichiers avant toute écriture : un conflit
            // tardif ne doit pas laisser une synchronisation partielle.
            for file in &saves.files {
                let path = cloud_file_path_in(game, file, &prefix_dir)
                    .ok_or("chemin de sauvegarde inconnu")?;
                if let Some(local) = read_cloud_local(&path)? {
                    let digest = lapinux_steam::cloud::file_sha(&local);
                    if !saves.knows(file, &digest) {
                        return Err("sauvegardes différentes : choisissez une version dans l'onglet Sauvegardes".into());
                    }
                }
            }
            let mut written = 0usize;
            let mut bytes = 0u64;
            // Une reprise automatique ne doit pas retarder le lancement : passé
            // le délai, le reste s'obtient par le bouton des réglages.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(25);
            for file in &saves.files {
                if only_newer && std::time::Instant::now() > deadline {
                    tracing::warn!(
                        "cloud : délai de reprise dépassé, fichiers restants laissés au bouton"
                    );
                    break;
                }
                let Some(path) = cloud_file_path_in(game, file, &prefix_dir) else {
                    tracing::warn!(
                        "cloud : {} ignoré, chemin de sauvegarde inconnu (préfixe « {} »)",
                        file.name,
                        file.prefix
                    );
                    continue;
                };
                // Une date ne prouve pas que la version locale peut être
                // remplacée. Toute différence attend un choix explicite.
                if let Some(local) = read_cloud_local(&path)? {
                    if saves.knows(file, &lapinux_steam::cloud::file_sha(&local)) {
                        continue;
                    }
                    return Err("sauvegardes différentes : choisissez une version dans l'onglet Sauvegardes".into());
                }
                let data = lapinux_steam::cloud::download_file(&session, appid, file)
                    .await
                    .map_err(|error| error.to_string())?;
                if !saves.knows(file, &lapinux_steam::cloud::file_sha(&data)) {
                    return Err(format!(
                        "empreinte cloud indisponible ou incorrecte : {}",
                        file.name
                    ));
                }
                write_cloud_file(&path, &data, None)?;
                if file.modified > 0 {
                    // La date du cloud guide le jeu quand il compare ses
                    // sauvegardes : elle est reposée sur la copie locale.
                    let stamp = std::time::UNIX_EPOCH
                        + std::time::Duration::from_secs(file.modified as u64);
                    if let Ok(handle) = std::fs::OpenOptions::new().write(true).open(&path) {
                        let _ = handle.set_times(std::fs::FileTimes::new().set_modified(stamp));
                    }
                }
                written += 1;
                bytes += data.len() as u64;
            }
            Ok::<(usize, u64), String>((written, bytes))
        });
        if let Ok((files, bytes)) = &result {
            if *files > 0 {
                crate::cloud_journal::record(
                    &self.paths,
                    appid,
                    lapinux_core::model::CloudSyncEntry {
                        at: lapinux_core::util::now_unix(),
                        direction: lapinux_core::model::CloudSyncDirection::Download,
                        machine: "Steam Cloud".into(),
                        files: *files,
                        bytes: *bytes,
                    },
                );
            }
        }
        result
    }

    /// Liste des sauvegardes cloud d'un jeu, ou `None` quand le compte n'a pas
    /// de session utilisable : la fiche se passe alors de la ligne.
    pub(super) fn read_cloud(&self, appid: u32) -> Option<lapinux_core::model::CloudSaves> {
        self.read_cloud_result(appid).ok()
    }

    /// Même lecture, avec la raison de l'échec : la carte des réglages doit
    /// distinguer « aucune sauvegarde » de « session coupée ».
    fn read_cloud_result(
        &self,
        appid: u32,
    ) -> std::result::Result<lapinux_core::model::CloudSaves, String> {
        let config = self.config_snapshot();
        let saved = self.saved_session(&config.steam.username).ok_or_else(|| {
            "connectez votre compte Steam dans Réglages → Compte Steam".to_string()
        })?;
        if saved.refresh_token.trim().is_empty() {
            return Err("la session Lapinux n'a pas de jeton réutilisable".to_string());
        }
        lapinux_steam::shared::block_on(async {
            let session =
                lapinux_steam::shared::session(&saved.account_name, &saved.refresh_token).await?;
            lapinux_steam::cloud::game_cloud(&session, appid).await
        })
        .map_err(|error| {
            tracing::warn!("cloud du jeu {appid} illisible : {error}");
            error.to_string()
        })
    }

    /// Différence entre le relevé pris au lancement et l'état actuel du cloud.
    ///
    /// Rien n'est comparé tant que le jeu tourne : la partie est justement en
    /// train d'écrire. La comparaison est faite une fois, gardée pour la fiche,
    /// puis le relevé est oublié — le lancement suivant en prendra un neuf.
    fn cloud_diff_after(&self, appid: u32) -> Option<lapinux_core::model::CloudDiff> {
        if self
            .session_info()
            .map(|session| session.appid == appid)
            .unwrap_or(false)
        {
            return None;
        }
        if let Some((known, diff)) = self.cloud_last.lock().ok().and_then(|slot| slot.clone()) {
            if known == appid {
                return Some(diff);
            }
        }
        let (known, before) = self
            .cloud_before
            .lock()
            .ok()
            .and_then(|slot| slot.clone())?;
        if known != appid {
            return None;
        }
        let after = self.read_cloud(appid)?;
        let diff = lapinux_core::model::cloud_diff(&before, &after);
        tracing::info!("cloud : {}", diff.summary());
        if let Ok(mut slot) = self.cloud_last.lock() {
            *slot = Some((appid, diff.clone()));
        }
        if let Ok(mut slot) = self.cloud_before.lock() {
            *slot = None;
        }
        Some(diff)
    }
}

/// Compare le relevé montré au joueur avec le relevé frais du serveur.
pub(super) fn same_cloud_snapshot(
    expected: &lapinux_core::model::CloudSaves,
    actual: &lapinux_core::model::CloudSaves,
) -> bool {
    expected.change_number == actual.change_number
        && expected.files.len() == actual.files.len()
        && expected.files.iter().zip(&actual.files).all(|(a, b)| {
            a.name == b.name
                && a.prefix == b.prefix
                && a.sha == b.sha
                && a.size == b.size
                && a.modified == b.modified
        })
        && expected.superseded.len() == actual.superseded.len()
        && expected.superseded.iter().zip(&actual.superseded).all(|(a, b)| {
            a.name == b.name
                && a.prefix == b.prefix
                && a.sha == b.sha
                && a.size == b.size
                && a.modified == b.modified
        })
}

/// Ce qu'il y a à faire d'un fichier cloud après la partie.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CloudAction {
    Skip,
    Upload,
    Delete,
    IgnoreMissing,
    Conflict,
}

fn same_cloud_file(
    left: &lapinux_core::model::CloudFile,
    right: &lapinux_core::model::CloudFile,
) -> bool {
    left.prefix == right.prefix && left.name == right.name
}

/// Compare un fichier cloud à sa copie locale et à l'instantané du lancement.
/// Une copie locale absente n'est une suppression que si le fichier était déjà
/// dans l'instantané : sinon il vient d'une autre machine et on l'ignore.
pub(super) fn cloud_action(
    file: &lapinux_core::model::CloudFile,
    local: Option<&[u8]>,
    before: Option<&lapinux_core::model::CloudSaves>,
) -> CloudAction {
    match local {
        Some(data) => {
            let digest = lapinux_steam::cloud::file_sha(data);
            // Une version déjà connue du cloud (autre machine) n'est pas une
            // progression à envoyer : rien n'a changé de ce côté.
            if before.is_some_and(|before| before.knows(file, &digest)) {
                return CloudAction::Skip;
            }
            if file.sha.len() != 20 {
                return CloudAction::Conflict;
            }
            if file.sha == digest {
                CloudAction::Skip
            } else {
                CloudAction::Upload
            }
        }
        None => {
            if before.is_some_and(|before| {
                before
                    .files
                    .iter()
                    .any(|known| same_cloud_file(known, file))
            }) {
                CloudAction::Delete
            } else {
                CloudAction::IgnoreMissing
            }
        }
    }
}

pub(crate) fn read_cloud_local(path: &Path) -> std::result::Result<Option<Vec<u8>>, String> {
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{} : {e}", path.display())),
        Ok(meta) if !meta.is_file() => {
            Err(format!("{} n'est pas un fichier ordinaire", path.display()))
        }
        Ok(meta) if meta.len() > lapinux_steam::cloud::MAX_UPLOAD => {
            Err(format!("{} est trop volumineux", path.display()))
        }
        Ok(_) => {
            use std::io::Read;
            let mut data = Vec::new();
            std::fs::File::open(path)
                .and_then(|file| {
                    file.take(lapinux_steam::cloud::MAX_UPLOAD + 1)
                        .read_to_end(&mut data)
                })
                .map_err(|e| format!("{} : {e}", path.display()))?;
            if data.len() as u64 > lapinux_steam::cloud::MAX_UPLOAD {
                return Err(format!("{} est trop volumineux", path.display()));
            }
            Ok(Some(data))
        }
    }
}

/// Écriture préparée puis publiée atomiquement ; une nouvelle sauvegarde ne
/// doit jamais écraser un fichier créé entre la lecture et l'écriture.
pub(crate) fn write_cloud_file(
    path: &Path,
    data: &[u8],
    expected: Option<&[u8]>,
) -> std::result::Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let parent = path.parent().ok_or("dossier parent inconnu")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut attempt = 0u32;
    let (tmp, mut file) = loop {
        let tmp = parent.join(format!(
            ".lapinux-cloud-{}-{attempt}.tmp",
            std::process::id()
        ));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)
        {
            Ok(file) => break (tmp, file),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && attempt < 1000 => {
                attempt += 1
            }
            Err(e) => return Err(format!("préparation de {} : {e}", path.display())),
        }
    };
    let result = (|| -> std::result::Result<(), String> {
        file.write_all(data)
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        if std::fs::read(&tmp).map_err(|e| e.to_string())? != data {
            return Err("fichier préparé différent des données reçues".into());
        }
        if read_cloud_local(path)?.as_deref() != expected {
            return Err(format!(
                "{} a changé depuis la confirmation",
                path.display()
            ));
        }
        if expected.is_none() {
            std::fs::hard_link(&tmp, path).map_err(|e| format!("{} : {e}", path.display()))?;
            std::fs::remove_file(&tmp).map_err(|e| e.to_string())?;
        } else {
            std::fs::rename(&tmp, path).map_err(|e| format!("{} : {e}", path.display()))?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Copies distinctes des deux versions, conservées hors des trois emplacements.
pub(crate) fn save_cloud_recovery(
    data_dir: &Path,
    game: &Game,
    cloud: &lapinux_core::model::CloudSaves,
    local: &[(PathBuf, Option<Vec<u8>>)],
    remote: &[Option<Vec<u8>>],
    superseded: &[lapinux_core::model::CloudFile],
) -> std::result::Result<PathBuf, String> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    if cloud.files.len() != local.len() || local.len() != remote.len() {
        return Err("copies incomplètes".into());
    }
    let root = data_dir.join("cloud-recovery").join(game.appid.to_string());
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let canonical = std::fs::canonicalize(&root).map_err(|e| e.to_string())?;
    if std::fs::canonicalize(game.install_dir())
        .ok()
        .is_some_and(|p| canonical.starts_with(p))
        || game
            .prefix
            .as_ref()
            .and_then(|p| std::fs::canonicalize(p).ok())
            .is_some_and(|p| canonical.starts_with(p))
        || std::fs::canonicalize(lapinux_proton::prefix_path(game, data_dir))
            .ok()
            .is_some_and(|p| canonical.starts_with(p))
    {
        return Err("le dossier de récupération se trouve dans les fichiers du jeu".into());
    }
    let mut n = 0u32;
    let dir = loop {
        let dir = root.join(format!(
            "{}-{}-{n}",
            lapinux_core::util::now_unix(),
            std::process::id()
        ));
        match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
            Ok(()) => break dir,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && n < 1000 => n += 1,
            Err(e) => return Err(format!("création des copies : {e}")),
        }
    };
    let result = (|| -> std::result::Result<(), String> {
        let mut entries = Vec::new();
        for (index, ((path, local_bytes), remote_bytes)) in local.iter().zip(remote).enumerate() {
            let store = |name: &str, bytes: &[u8]| -> std::result::Result<(), String> {
                let path = dir.join(name);
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&path)
                    .map_err(|e| e.to_string())?;
                file.write_all(bytes)
                    .and_then(|_| file.sync_all())
                    .map_err(|e| e.to_string())?;
                if std::fs::read(&path).map_err(|e| e.to_string())? != bytes {
                    return Err("vérification de la copie impossible".into());
                }
                Ok(())
            };
            let local_name = format!("{index}-local");
            let remote_name = format!("{index}-cloud");
            let mut stored: Vec<(String, Vec<u8>)> = Vec::new();
            let mut cloud_sha: Option<Vec<u8>> = None;
            if let Some(bytes) = local_bytes {
                store(&local_name, bytes)?;
                stored.push((
                    local_name.clone(),
                    lapinux_steam::cloud::file_sha(bytes),
                ));
            }
            if let Some(bytes) = remote_bytes {
                store(&remote_name, bytes)?;
                let digest = lapinux_steam::cloud::file_sha(bytes);
                stored.push((remote_name.clone(), digest.clone()));
                cloud_sha = Some(digest);
            }
            // Versions écartées du même chemin : Steam ne sert qu'une version
            // par nom, donc celles dont les octets sont déjà dans la copie
            // (souvent la version locale) sont nommées ici, les autres
            // restent décrites par leur empreinte, leur taille et leur date.
            let replaced: Vec<serde_json::Value> = superseded
                .iter()
                .filter(|old| {
                    old.prefix == cloud.files[index].prefix && old.name == cloud.files[index].name
                })
                .map(|old| {
                    serde_json::json!({
                        "sha": &old.sha,
                        "size": old.size,
                        "modified": old.modified,
                        "stored": stored
                            .iter()
                            .find(|(_, sha)| *sha == old.sha)
                            .map(|(name, _)| name.clone()),
                    })
                })
                .collect();
            // `cloud_sha` dit ce que contient vraiment la copie enregistrée :
            // Steam ne sert qu'une version par nom, qui peut être l'une des
            // entrées écartées plutôt que celle retenue.
            entries.push(serde_json::json!({ "original": path, "cloud_name": cloud.files[index].name, "cloud_prefix": cloud.files[index].prefix, "local": local_bytes.as_ref().map(|_| local_name), "cloud": remote_bytes.as_ref().map(|_| remote_name), "cloud_sha": cloud_sha, "cloud_retained": &cloud.files[index].sha, "superseded": replaced }));
        }
        store_recovery_manifest(&dir, &entries)?;
        Ok(())
    })();
    if let Err(error) = result {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(format!("copies de sécurité impossibles : {error}"));
    }
    Ok(dir)
}

fn store_recovery_manifest(
    dir: &Path,
    entries: &[serde_json::Value],
) -> std::result::Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let bytes = serde_json::to_vec_pretty(entries).map_err(|e| e.to_string())?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(dir.join("manifest.json"))
        .map_err(|e| e.to_string())?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Fichier local d'une sauvegarde cloud : le préfixe Windows déclaré par le
/// jeu, ou le jeton porté par le nom (`%GameInstall%save.json`), résolu dans
/// l'installation ou dans le préfixe Proton qui exécute le jeu.
fn cloud_file_path(
    game: &Game,
    file: &lapinux_core::model::CloudFile,
    data_dir: &std::path::Path,
) -> Option<std::path::PathBuf> {
    let prefix_dir = lapinux_proton::prefix_for(game, data_dir);
    cloud_file_path_in(game, file, &prefix_dir)
}

/// Même résolution, avec un préfixe déjà préparé : la reprise cloud initialise
/// Proton avant d'écrire, les autres lectures se contentent du chemin existant.
fn cloud_file_path_in(
    game: &Game,
    file: &lapinux_core::model::CloudFile,
    prefix_dir: &std::path::Path,
) -> Option<std::path::PathBuf> {
    lapinux_steam::cloud::local_file(&file.prefix, &file.name, &game.install_dir(), prefix_dir)
}

/// Nom de la machine, tel que Steam le range dans un lot d'envoi : le nom
/// d'hôte suffit, c'est une étiquette lisible dans le client.
fn machine_name() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "lapinux".to_string())
}

/// Date de modification d'un fichier local, en secondes Unix.
fn local_modified(path: &std::path::Path) -> Option<i64> {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|stamp| stamp.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|age| age.as_secs() as i64)
}
