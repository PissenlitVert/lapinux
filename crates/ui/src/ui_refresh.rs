//! Actualisation des données et des tâches.

use super::*;

impl LapinuxApp {
    pub(super) fn refresh(&mut self, rescan: bool) {
        let snapshot = Self::fetch_snapshot(&self.client, rescan);
        self.apply_snapshot(snapshot);
        self.config = self
            .client
            .request(&Req::Config)
            .ok()
            .and_then(|e| e.into_data().ok())
            .unwrap_or_else(|| self.config.clone());
        self.grid_width = self.config.general.grid_width as f32;
    }

    fn fetch_snapshot(client: &Client, rescan: bool) -> UiSnapshot {
        if rescan {
            let _ = client.request_with_timeout(&Req::Scan, Duration::from_secs(10));
        }
        UiSnapshot {
            games: ui_request(client, &Req::Games).unwrap_or_default(),
            protons: ui_request(client, &Req::Protons).unwrap_or_default(),
            jobs: ui_request(client, &Req::Jobs).unwrap_or_default(),
            session: ui_request(client, &Req::Session).flatten(),
            devices: ui_request(client, &Req::Devices).unwrap_or_default(),
            steam_status: ui_request(client, &Req::SteamStatus),
            config: ui_request(client, &Req::Config),
        }
    }

    /// Lance la relecture périodique sans bloquer la boucle egui.
    pub(super) fn refresh_async(&mut self, ctx: &egui::Context, rescan: bool) {
        if self.refresh_rx.is_some() {
            // La réponse en vol a été demandée avant le changement : elle
            // peut déjà être périmée. Une seconde lecture suivra.
            self.refresh_queued = true;
            return;
        }
        self.refresh_queued = false;
        let (tx, rx) = mpsc::channel();
        self.refresh_rx = Some(rx);
        self.last_refresh = Instant::now();
        // Le client est réutilisé : une connexion neuve toutes les trois
        // secondes ouvrait et refermait un socket pour rien.
        let client = self.client.detached();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let snapshot = Self::fetch_snapshot(&client, rescan);
            let _ = tx.send(snapshot);
            ctx.request_repaint();
        });
    }

    /// Relit les tâches sans bloquer la peinture : la demande part sur un fil
    /// séparé et la réponse est relevée à l'image suivante. Un échange IPC en
    /// attente ne retarde donc jamais le dessin.
    pub(super) fn poll_jobs(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.jobs_rx {
            match rx.try_recv() {
                Ok(jobs) => {
                    if let Some(jobs) = jobs {
                        let finished = self.newly_finished_appids(&jobs);
                        self.jobs = jobs;
                        if !finished.is_empty() {
                            for appid in finished {
                                self.forget_cloud(appid);
                            }
                            self.refresh_async(ctx, false);
                        }
                    }
                    self.jobs_rx = None;
                }
                Err(mpsc::TryRecvError::Empty) => return,
                Err(mpsc::TryRecvError::Disconnected) => self.jobs_rx = None,
            }
        }
        let running = self.jobs.iter().any(|job| job.state == JobState::Running);
        if !running || self.jobs_poll.elapsed() < Duration::from_millis(300) {
            return;
        }
        self.jobs_poll = Instant::now();
        let (tx, rx) = mpsc::channel();
        self.jobs_rx = Some(rx);
        let worker = self.client.detached();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let jobs = worker
                .request(&Req::Jobs)
                .ok()
                .and_then(|envelope| envelope.into_data::<Vec<Job>>().ok());
            if tx.send(jobs).is_ok() {
                ctx.request_repaint();
            }
        });
    }

    /// Jeux dont une tâche vient de passer à « terminé ». Leur fiche et leur
    /// relevé cloud sont encore ceux d'avant : les deux doivent être relus.
    fn newly_finished_appids(&self, jobs: &[Job]) -> Vec<u32> {
        jobs.iter()
            .filter(|job| job.state == JobState::Done)
            .filter(|job| {
                self.jobs
                    .iter()
                    .any(|old| old.id == job.id && old.state != JobState::Done)
            })
            .filter_map(|job| job.appid)
            .collect()
    }

    /// Oublie le relevé cloud d'un jeu, y compris la lecture en vol : elle a
    /// été demandée avant la fin de l'installation et peindrait l'ancien état.
    pub(super) fn forget_cloud(&mut self, appid: u32) {
        self.cloud.remove(&appid);
        self.cloud_asked.remove(&appid);
        if self.cloud_fetch_appid == Some(appid) {
            self.cloud_fetch = None;
            self.cloud_fetch_appid = None;
        }
    }

    /// Relit seulement la partie en cours : la cadence de la pastille suit le
    /// jeu à la seconde, alors que l'état complet ne revient que toutes les
    /// trois secondes (et porte toute la bibliothèque).
    pub(super) fn poll_session(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.session_rx {
            match rx.try_recv() {
                Ok(session) => {
                    if session != self.session {
                        ctx.request_repaint();
                    }
                    self.session = session;
                    self.session_rx = None;
                }
                Err(mpsc::TryRecvError::Empty) => return,
                Err(mpsc::TryRecvError::Disconnected) => self.session_rx = None,
            }
        }
        // Un daemon arrêté en cours de lecture ne doit pas figer la pastille.
        if self.session_rx.is_some() && self.session_poll.elapsed() > Duration::from_secs(10) {
            self.session_rx = None;
        }
        // Une partie peut se terminer par la fermeture de sa fenêtre, avant
        // la sortie du processus : quatre relevés par seconde font apparaître
        // l'écran de fin sans attendre le prochain relevé général. Une partie
        // lancée depuis le CLI doit aussi être découverte quand la session est
        // encore inconnue de l'UI : un relevé par seconde suffit.
        let interval = if self.session.is_some() {
            Duration::from_millis(250)
        } else {
            Duration::from_secs(1)
        };
        if self.session_poll.elapsed() < interval {
            return;
        }
        self.session_poll = Instant::now();
        let (tx, rx) = mpsc::channel();
        self.session_rx = Some(rx);
        let worker = self.client.detached();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let session = ui_request(&worker, &Req::Session);
            if tx.send(session).is_ok() {
                ctx.request_repaint();
            }
        });
    }

    /// État de l'enregistreur pour la page Captures : une requête par seconde,
    /// dans un fil, pour ne jamais bloquer la peinture.
    pub(super) fn poll_recording(&mut self, ctx: &egui::Context) {
        if let Some(rx) = self.recording_rx.as_ref() {
            match rx.try_recv() {
                Ok(state) => {
                    self.recording_state = state;
                    self.recording_rx = None;
                    ctx.request_repaint();
                }
                Err(mpsc::TryRecvError::Empty) => return,
                Err(mpsc::TryRecvError::Disconnected) => self.recording_rx = None,
            }
        }
        if self.page != Page::Captures || self.recording_poll.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.recording_poll = Instant::now();
        let (tx, rx) = mpsc::channel();
        self.recording_rx = Some(rx);
        let worker = self.client.detached();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let state = ui_request::<Option<lapinux_core::model::RecordingState>>(
                &worker,
                &Req::RecordingStatus,
            )
            .flatten();
            if tx.send(state).is_ok() {
                ctx.request_repaint();
            }
        });
    }

    pub(super) fn poll_refresh(&mut self, ctx: &egui::Context) {
        // Un daemon arrêté pendant une requête peut laisser le fil bloqué sur
        // la connexion : sans ce garde-fou, la liste des manettes ne se
        // rafraîchirait plus jamais après un redémarrage du daemon.
        if self.refresh_rx.is_some() && self.last_refresh.elapsed() > Duration::from_secs(20) {
            self.refresh_rx = None;
        }
        let received = self.refresh_rx.as_ref().map(|rx| rx.try_recv());
        match received {
            Some(Ok(snapshot)) => {
                self.refresh_rx = None;
                self.apply_snapshot(snapshot);
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => self.refresh_rx = None,
            Some(Err(mpsc::TryRecvError::Empty)) | None => {}
        }
        if self.refresh_queued && self.refresh_rx.is_none() {
            self.refresh_queued = false;
            self.refresh_async(ctx, false);
        }
    }

    fn apply_snapshot(&mut self, snapshot: UiSnapshot) {
        // La relecture générale tourne toutes les trois secondes. Invalider
        // la bibliothèque à chaque passage reconstruisait la liste fusionnée
        // et les jeux des Actualités même quand aucun jeu n'avait bougé :
        // c'était un à-coup périodique pendant le défilement de Soldes.
        if snapshot.games != self.games {
            // Un jeu apparu pendant la session (scan, téléchargement) est une
            // arrivée : le contrôle des nouveaux jeux doit repartir pour le
            // comparer à la photographie précédente.
            let grew = !self.games.is_empty()
                && snapshot
                    .games
                    .iter()
                    .any(|game| !self.games.iter().any(|known| known.appid == game.appid));
            self.games = snapshot.games;
            // Les tailles de la page Stockage viennent de cette liste : le tri
            // sera refait à sa prochaine ouverture, pas pendant la peinture.
            self.storage.dirty = true;
            // Le scan a pu changer : la liste fusionnée sera reconstruite.
            self.touch_library();
            // La liste des mises à jour dépend des jeux installés : un scan
            // terminé la rend caduque.
            self.downloads.mark_updates_dirty();
            if grew {
                self.library_checked = false;
                self.library_check_at = Some(Instant::now());
            }
        }
        let finished = self.newly_finished_appids(&snapshot.jobs);
        self.protons = snapshot.protons;
        self.jobs = snapshot.jobs;
        if !finished.is_empty() {
            for appid in finished {
                self.forget_cloud(appid);
            }
            self.refresh_queued = true;
        }
        // Une partie vient de se terminer : le cloud a pu changer, la fiche le
        // relira au lieu de garder le relevé d'avant le jeu.
        let finished = self
            .session
            .as_ref()
            .map(|session| session.appid)
            .filter(|appid| snapshot.session.as_ref().map(|session| session.appid) != Some(*appid));
        if let Some(appid) = finished {
            self.cloud.remove(&appid);
            self.cloud_asked.remove(&appid);
            // La partie qui vient de se fermer entre dans l'historique : la
            // page Activité le relit au lieu de garder la liste d'avant.
            self.play_history_asked = false;
            self.play_history_rx = None;
        }
        self.session = snapshot.session;
        self.devices = snapshot.devices;
        self.apply_steam_status(snapshot.steam_status);
        if let Some(config) = snapshot.config {
            self.grid_width = config.general.grid_width as f32;
            self.covers.set_config(
                &config.artwork.steamgriddb_key,
                config.artwork.steamgriddb,
                &config.artwork.sources,
            );
            self.art.set_config(
                &config.artwork.steamgriddb_key,
                config.artwork.steamgriddb,
                &config.artwork.sources,
            );
            self.config = config;
        }
        self.startup.ready = true;
        self.last_refresh = Instant::now();
    }

    /// Applique l'état Steam et signale la fin d'une connexion demandée depuis
    /// l'écran de connexion. Utilisé par le rafraîchissement général et par la
    /// sonde rapide pendant une connexion.
    pub(super) fn apply_steam_status(&mut self, status: Option<SteamStatus>) {
        let was_running = self.steam_login_state.running;
        let previous_user = self.steam_user.clone();
        self.steam_status = status;
        if let Some(status) = &self.steam_status {
            if !status.username.is_empty() && self.steam_user.is_empty() {
                self.steam_user = status.username.clone();
            }
            self.steam_login_state = status.login.clone();
        }
        // Un autre compte : ses sauvegardes cloud ne sont pas les mêmes, et
        // son catalogue non plus.
        if self.steam_user != previous_user {
            self.cloud.clear();
            self.cloud_asked.clear();
            self.touch_library();
        }
        if self.steam_login_state.running {
            self.login_since.get_or_insert_with(Instant::now);
        } else {
            self.login_since = None;
        }
        if was_running && !self.steam_login_state.running {
            if self.steam_login_state.connected {
                self.steam_password.clear();
                self.steam_token.clear();
                self.steam_guard.clear();
                self.notify(
                    self.steam_login_state
                        .message
                        .clone()
                        .unwrap_or_else(|| lapinux_core::i18n::t("refresh.steam_connected").into()),
                );
                self.load_catalog(true);
                // Une session fraîche peut publier un cloud que la fiche
                // n'avait pas pu lire : elle le redemandera.
                self.cloud.clear();
                self.cloud_asked.clear();
            } else if let Some(error) = self.steam_login_state.error.clone() {
                self.notify(error);
            }
        }
    }

    pub(super) fn poll_steam_login(&mut self) {
        let status = self
            .client
            .request(&Req::SteamStatus)
            .ok()
            .and_then(|e| e.into_data().ok());
        self.apply_steam_status(status);
    }

    pub(super) fn save_settings(&mut self, appid: u32, settings: GameSettings) {
        self.busy = true;
        match self.client.request(&Req::SaveSettings { appid, settings }) {
            Ok(envelope) if envelope.ok => {
                self.notify(lapinux_core::i18n::t("refresh.settings_saved"))
            }
            Ok(envelope) => self.notify(
                envelope
                    .error
                    .unwrap_or_else(|| lapinux_core::i18n::t("refresh.error").into()),
            ),
            Err(err) => self.notify(format!("{err}")),
        }
        self.busy = false;
    }
}
