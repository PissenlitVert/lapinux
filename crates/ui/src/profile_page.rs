//! Profil, historique et interactions sociales.

use super::*;

impl LapinuxApp {
    /// Section **Profil** : le compte Steam tel qu'il se montre (pseudo,
    /// avatar, description, présence, amis, famille, commentaires), à droite
    /// de Manettes.
    pub(super) fn profile_page(&mut self, ui: &mut egui::Ui) {
        // La page Activité aligne des dizaines de vignettes : deux décodages
        // par image suffisent, et le cache se vide avant de peindre.
        self.covers.begin_frame(2);
        self.covers.trim();
        let steam_id = self
            .steam_status
            .as_ref()
            .and_then(|status| status.steam_id);
        self.profile.ensure_loaded(steam_id);
        self.profile.set_social(self.social.clone());
        // Les comptes des boutiques : lus une fois par le daemon, servis
        // ensuite depuis son cache. La liste du profil les affiche avec l'état
        // de chacun.
        self.stores.ensure(&self.client);
        if let Some(message) = self.stores.poll(ui.ctx()) {
            self.notify(message);
        }
        let account = self
            .steam_status
            .as_ref()
            .map(|status| status.username.clone())
            .unwrap_or_default();
        let steam_connected = self
            .steam_status
            .as_ref()
            .is_some_and(|status| status.connected);
        // L'activité et Discord passent au-dessus des plateformes : ce sont
        // les pages de la session, celles qu'on regarde en premier.
        let live = self.session.as_ref().map(|session| session.name.clone());
        let mut platforms = vec![
            profile::PlatformEntry {
                platform: profile::Platform::Activity,
                account: match &live {
                    Some(name) => lapinux_core::i18n::t_args(
                        "profile_page.running_now",
                        &[("name", name.as_str())],
                    ),
                    None if !self.play_history.is_empty() => lapinux_core::i18n::t_fmt(
                        "profile_page.games_played",
                        &[self.play_history.len().to_string()],
                    ),
                    None => lapinux_core::i18n::t("profile_page.history").to_string(),
                },
                connected: live.is_some(),
            },
            profile::PlatformEntry {
                platform: profile::Platform::Discord,
                account: if self.config.discord.enabled {
                    lapinux_core::i18n::t("profile_page.presence_on").to_string()
                } else {
                    lapinux_core::i18n::t("profile_page.presence_off").to_string()
                },
                connected: self.config.discord.enabled,
            },
            profile::PlatformEntry {
                platform: profile::Platform::Steam,
                account: if account.trim().is_empty() {
                    lapinux_core::i18n::t("profile_page.personal_account").to_string()
                } else {
                    account.clone()
                },
                connected: steam_connected,
            },
        ];
        for store in lapinux_core::stores::Store::EXTERNAL {
            let account = self
                .stores
                .view
                .accounts
                .iter()
                .find(|account| account.store == store);
            let platform = match store {
                lapinux_core::stores::Store::Epic => profile::Platform::Epic,
                lapinux_core::stores::Store::Gog => profile::Platform::Gog,
                lapinux_core::stores::Store::Itch => profile::Platform::Itch,
                lapinux_core::stores::Store::Steam => profile::Platform::Steam,
            };
            platforms.push(profile::PlatformEntry {
                platform,
                account: account
                    .map(|account| account.account.clone())
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or_default(),
                connected: account.map(|account| account.connected).unwrap_or(false),
            });
        }
        self.ensure_play_history(ui.ctx());
        self.poll_play_history(ui.ctx());
        let sections = self.activity_sections();
        let new_games = self.new_games.clone();
        let discord = self.config.discord.clone();
        let mut discord_update = None;
        let mut discord_tab = |ui: &mut egui::Ui| {
            discord_page(ui, &discord, &mut discord_update);
        };
        let mut opened_request: Option<ActivityRequest> = None;
        let actions = {
            let covers = &mut self.covers;
            let paths = &self.paths;
            let mut activity_tab = |ui: &mut egui::Ui| {
                opened_request = activity_page(ui, covers, paths, &sections, &new_games);
            };
            self.profile.page(
                ui,
                &account,
                &platforms,
                &mut discord_tab,
                &mut activity_tab,
            )
        };
        // Le modal se dessine au-dessus de la page et de son scroll : un clic
        // sur un compteur de la famille ne doit pas être rogné par la colonne.
        self.profile
            .family_games_modal(ui.ctx(), self.catalog.as_ref());
        if let Some(request) = opened_request {
            match request {
                ActivityRequest::Log(log) => self.activity_log = Some(log),
                ActivityRequest::DeleteSession { appid, started } => {
                    self.delete_session(appid, started)
                }
                ActivityRequest::DeleteJob { id } => self.delete_download(id),
                ActivityRequest::ClearDay(day) => self.clear_activity_day(day),
            }
        }
        if let Some(updated) = discord_update {
            let mut config = self.config.clone();
            config.discord = updated;
            self.apply_config(config);
        }
        self.apply_profile_actions(ui.ctx(), actions);
    }

    /// Parties terminées : une lecture locale du daemon, une seule fois par
    /// session. Une réponse perdue relance la demande au tour suivant.
    fn ensure_play_history(&mut self, ctx: &egui::Context) {
        if self.play_history_asked || self.play_history_rx.is_some() {
            return;
        }
        self.play_history_asked = true;
        let (tx, rx) = mpsc::channel();
        self.play_history_rx = Some(rx);
        let worker = self.client.detached_quiet();
        let wake = ctx.clone();
        std::thread::spawn(move || {
            let reply = worker
                .request_with_timeout(&Req::SessionHistory, Duration::from_secs(20))
                .ok()
                .and_then(|envelope| {
                    serde_json::from_value::<Vec<lapinux_core::model::PlaySession>>(envelope.data)
                        .ok()
                })
                .unwrap_or_default();
            if tx.send(reply).is_ok() {
                wake.request_repaint();
            }
        });
    }

    fn poll_play_history(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.play_history_rx else {
            return;
        };
        match rx.try_recv() {
            Ok(history) => {
                self.play_history = history;
                self.play_history_rx = None;
            }
            Err(mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_secs(1));
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.play_history_rx = None;
                // Le daemon a pu redémarrer : un nouvel essai plus tard.
                self.play_history_asked = false;
            }
        }
    }

    /// Boutique et jaquette d'un jeu, pour la vignette d'une carte d'activité.
    pub(super) fn activity_cover(
        &self,
        appid: u32,
    ) -> (Option<lapinux_core::stores::Store>, String) {
        // Une boutique publie sa jaquette ; un jeu Steam passe par le cache.
        if let Some((store, cover)) = self.stores.cover_of(appid) {
            return (Some(store), cover);
        }
        match self.games.iter().find(|game| game.appid == appid) {
            Some(game) if game.store != lapinux_core::stores::Store::Steam => {
                (Some(game.store), String::new())
            }
            _ => (None, String::new()),
        }
    }

    /// Journal ouvert depuis une carte d'Activité : le panneau fixe de
    /// l'application, plus lisible qu'un dépliage dans la carte et copiable
    /// d'un seul geste.
    pub(super) fn activity_log_panel(&mut self, ctx: &egui::Context) {
        let Some(log) = self.activity_log.clone() else {
            return;
        };
        let path = lapinux_core::util::ellipsize(&log.path, 72);
        let subtitle = format!("{} · {path}", log.name);
        let close = fixed_panel(
            ctx,
            "activity-log-panel",
            lapinux_core::i18n::t("ui.activity_log"),
            &subtitle,
            egui::vec2(940.0, 660.0),
            |ui| {
                let Some(text) = read_log_tail(&log.path) else {
                    muted(ui, lapinux_core::i18n::t("profile_page.log_missing"));
                    return;
                };
                ui.horizontal(|ui| {
                    if action_button(
                        ui,
                        lapinux_core::i18n::t("settings.application.copy"),
                        ActionTone::Quiet,
                    )
                    .clicked()
                    {
                        ui.ctx().copy_text(text.clone());
                    }
                    ui.add_space(8.0);
                    muted(ui, lapinux_core::i18n::t("profile_page.launch_log"));
                });
                ui.add_space(8.0);
                let mut shown = text;
                ui.add(
                    egui::TextEdit::multiline(&mut shown)
                        .font(egui::TextStyle::Monospace)
                        .desired_width(f32::INFINITY)
                        .desired_rows(28),
                );
            },
        );
        if close {
            self.activity_log = None;
        }
    }

    /// Retire une partie de l'historique : tout de suite à l'écran, et sur le
    /// disque par le daemon, qui tient le fichier des sessions.
    fn delete_session(&mut self, appid: u32, started: i64) {
        let mut seconds = 0u64;
        self.play_history.retain(|session| {
            if session.appid == appid && session.started == started {
                seconds += session.seconds;
                false
            } else {
                true
            }
        });
        if let Some(game) = self.games.iter_mut().find(|game| game.appid == appid) {
            game.playtime_secs = game.playtime_secs.saturating_sub(seconds);
        }
        self.history_edit(
            Req::DeleteSession { appid, started },
            lapinux_core::i18n::t("profile_page.session_deleted"),
        );
    }

    /// Retire un téléchargement terminé de l'historique. Le daemon garde les
    /// transferts encore repris (en pause ou interrompus) et ne touche pas aux
    /// fichiers du jeu.
    pub(crate) fn delete_download(&mut self, id: String) {
        self.jobs.retain(|job| job.id != id);
        self.history_edit(
            Req::DeleteJob { id },
            lapinux_core::i18n::t("profile_page.download_deleted"),
        );
    }

    /// Vide une journée de l'historique, sans toucher aux autres : les parties
    /// et les téléchargements terminés de cette journée disparaissent ensemble.
    fn clear_activity_day(&mut self, day: i64) {
        let mut removed: HashMap<u32, u64> = HashMap::new();
        self.play_history.retain(|session| {
            let started = if session.started > 0 {
                session.started
            } else {
                session.ended
            };
            if lapinux_core::util::day_start(started) == day {
                *removed.entry(session.appid).or_default() += session.seconds;
                false
            } else {
                true
            }
        });
        for (appid, seconds) in &removed {
            if let Some(game) = self.games.iter_mut().find(|game| game.appid == *appid) {
                game.playtime_secs = game.playtime_secs.saturating_sub(*seconds);
            }
        }
        let mut requests = vec![Req::ClearSessionDay { day }];
        let mut job_ids: Vec<String> = Vec::new();
        self.jobs.retain(|job| {
            let finished = matches!(
                job.state,
                lapinux_core::model::JobState::Done
                    | lapinux_core::model::JobState::Failed
                    | lapinux_core::model::JobState::Cancelled
            );
            if finished && lapinux_core::util::day_start(job.started) == day {
                job_ids.push(job.id.clone());
                false
            } else {
                true
            }
        });
        requests.extend(job_ids.into_iter().map(|id| Req::DeleteJob { id }));
        self.history_edit_requests(requests, lapinux_core::i18n::t("profile_page.day_deleted"));
    }

    /// Envoie plusieurs modifications au daemon en tâche de fond : l'écriture
    /// de l'historique ne doit pas figer la page.
    fn history_edit_requests(&mut self, requests: Vec<Req>, message: &str) {
        let worker = self.client.detached_quiet();
        std::thread::spawn(move || {
            for request in requests {
                let _ = worker.request_with_timeout(&request, Duration::from_secs(20));
            }
        });
        self.notify(message);
    }

    fn history_edit(&mut self, request: Req, message: &str) {
        self.history_edit_requests(vec![request], message);
    }

    /// Ce que la page Activité raconte, jour par jour : chaque partie et
    /// chaque téléchargement tombe dans la catégorie du jour où il a eu lieu,
    /// du plus récent au plus ancien. La partie et le transfert en cours
    /// appartiennent à la journée d'aujourd'hui.
    fn activity_sections(&self) -> Vec<ActivitySection> {
        let now = lapinux_core::util::now_unix();
        let mut cards: Vec<ActivityCard> = Vec::new();
        if let Some(session) = &self.session {
            let (store, remote) = self.activity_cover(session.appid);
            let played = (now - session.started).max(0) as u64;
            cards.push(ActivityCard {
                appid: session.appid,
                name: session.name.clone(),
                store,
                remote,
                detail: lapinux_core::i18n::t_fmt(
                    "profile_page.launched_at",
                    &[
                        lapinux_core::util::format_time(session.started).to_string(),
                        lapinux_core::util::format_duration(played).to_string(),
                    ],
                ),
                tag: lapinux_core::i18n::t("profile.in_game").into(),
                tone: OK,
                progress: None,
                started: Some(session.started),
                ended: None,
                log: session.log_path.to_string_lossy().to_string(),
                fps: session.fps,
                fps_series: session.fps_series.clone(),
                fps_series_start: session.fps_series_start,
                fps_series_end: session.fps_series_end,
                delete: None,
            });
        }
        if let Some(job) = self
            .jobs
            .iter()
            .find(|job| job.state == lapinux_core::model::JobState::Running)
        {
            let (store, remote) = match job.appid {
                Some(appid) => self.activity_cover(appid),
                None => (None, String::new()),
            };
            cards.push(ActivityCard {
                appid: job.appid.unwrap_or(0),
                name: job.title.clone(),
                store,
                remote,
                detail: format!(
                    "{} / {}",
                    lapinux_core::util::format_bytes(job.downloaded),
                    lapinux_core::util::format_bytes(job.total)
                ),
                tag: format!("{:.0} %", job.progress.clamp(0.0, 1.0) * 100.0),
                tone: ACCENT(),
                progress: Some(job.progress.clamp(0.0, 1.0)),
                started: Some(job.started),
                ended: None,
                log: String::new(),
                fps: None,
                fps_series: Vec::new(),
                fps_series_start: 0,
                fps_series_end: 0,
                delete: None,
            });
        }
        for session in &self.play_history {
            let (store, remote) = self.activity_cover(session.appid);
            cards.push(ActivityCard {
                appid: session.appid,
                name: session.name.clone(),
                store,
                remote,
                detail: session_times(session.started, session.ended),
                tag: lapinux_core::util::format_duration(session.seconds),
                tone: TEXT(),
                progress: None,
                started: Some(session.started),
                ended: Some(session.ended),
                log: session.log.clone(),
                // La partie est finie : la cadence « actuelle » n'a plus de
                // sens, seules les bornes restent.
                fps: session.fps.map(|mut stats| {
                    stats.current = 0;
                    stats
                }),
                fps_series: session.fps_series.clone(),
                // Une courbe enregistrée avant que l'heure des points ne soit
                // gardée n'a pas ses bornes : la partie les donne, la mesure
                // commençant après l'échauffement du lancement.
                fps_series_start: if session.fps_series_start > 0 {
                    session.fps_series_start
                } else {
                    session.started + 8
                },
                fps_series_end: if session.fps_series_end > 0 {
                    session.fps_series_end
                } else {
                    session.ended
                },
                delete: Some(ActivityDelete::Session {
                    appid: session.appid,
                    started: session.started,
                }),
            });
        }
        for job in self
            .jobs
            .iter()
            .rev()
            .filter(|job| job.state != lapinux_core::model::JobState::Running)
            .take(6)
        {
            let (store, remote) = match job.appid {
                Some(appid) => self.activity_cover(appid),
                None => (None, String::new()),
            };
            let (state, tone) = match job.state {
                lapinux_core::model::JobState::Done => {
                    (lapinux_core::i18n::t("profile_page.finished"), OK)
                }
                lapinux_core::model::JobState::Failed => {
                    (lapinux_core::i18n::t("profile_page.failed"), DANGER)
                }
                lapinux_core::model::JobState::Cancelled => {
                    (lapinux_core::i18n::t("profile_page.cancelled"), TEXT_DIM())
                }
                lapinux_core::model::JobState::Paused => ("en pause", WARN),
                lapinux_core::model::JobState::Interrupted => ("interrompu", WARN),
                _ => ("en attente", TEXT_DIM()),
            };
            // L'en-tête du jour porte déjà la date : la carte se contente de
            // l'heure de fin.
            let detail = match job.finished.filter(|stamp| *stamp > 0) {
                Some(finished) => {
                    format!("{state} · {}", lapinux_core::util::format_time(finished))
                }
                None => state.to_string(),
            };
            cards.push(ActivityCard {
                appid: job.appid.unwrap_or(0),
                name: job.title.clone(),
                store,
                remote,
                detail,
                tag: if job.total > 0 {
                    lapinux_core::util::format_bytes(job.total)
                } else {
                    String::new()
                },
                tone,
                progress: None,
                started: Some(job.started),
                ended: job.finished,
                log: String::new(),
                fps: None,
                fps_series: Vec::new(),
                fps_series_start: 0,
                fps_series_end: 0,
                delete: matches!(
                    job.state,
                    lapinux_core::model::JobState::Done
                        | lapinux_core::model::JobState::Failed
                        | lapinux_core::model::JobState::Cancelled
                )
                .then(|| ActivityDelete::Job { id: job.id.clone() }),
            });
        }
        group_by_day(cards, now)
            .into_iter()
            .map(|(day, cards)| {
                let count = cards.len();
                ActivitySection {
                    day,
                    detail: if count > 1 {
                        lapinux_core::i18n::t_args(
                            "profile_page.elements",
                            &[("count", &count.to_string())],
                        )
                    } else {
                        lapinux_core::i18n::t("profile_page.element").to_string()
                    },
                    cards,
                }
            })
            .collect()
    }

    /// Publie sur Discord ce que l'interface montre, seulement quand cela
    /// change : la section ouverte et le jeu dont la fiche s'affiche.
    pub(super) fn sync_discord_view(&mut self) {
        // Les réglages ouverts priment : c'est ce que l'on regarde à l'écran,
        // quelle que soit la page dessous.
        let section = if self.settings_open {
            "settings".to_string()
        } else {
            match self.page {
                Page::Home => "home",
                Page::Library => "library",
                Page::Collections => "collections",
                Page::Downloads => "downloads",
                Page::Controllers => "controllers",
                Page::Profile => "profile",
                Page::Boutiques => "boutiques",
                Page::Captures => "captures",
                Page::News => "news",
            }
            .to_string()
        };
        let game = self.selected.and_then(|appid| {
            self.games
                .iter()
                .find(|game| game.appid == appid)
                .map(|game| game.name.clone())
                .or_else(|| {
                    self.stores
                        .owned_games()
                        .into_iter()
                        .find(|game| game.appid == appid)
                        .map(|game| game.name.clone())
                })
                .or_else(|| {
                    self.catalog.as_ref().and_then(|catalog| {
                        catalog
                            .entries
                            .iter()
                            .find(|entry| entry.appid == appid)
                            .map(|entry| entry.name.clone())
                    })
                })
        });
        let (note, cover, appid, store, store_id) =
            self.discord_activity_info(&section, game.is_some());
        let sent = (
            section.clone(),
            game.clone(),
            note.clone(),
            cover.clone(),
            store,
            store_id.clone(),
            appid,
        );
        let unchanged = self.discord_view_sent.as_ref() == Some(&sent);
        let due = self
            .discord_view_sent_at
            .map(|at| at.elapsed() >= std::time::Duration::from_secs(15))
            .unwrap_or(true);
        if unchanged && !due {
            return;
        }
        self.discord_view_sent = Some(sent);
        self.discord_view_sent_at = Some(Instant::now());
        // Une demande locale, sans réponse à attendre : un daemon endormi n'a
        // pas à figer l'interface pour une présence.
        let request = Req::DiscordView {
            section,
            game,
            note,
            cover,
            store,
            store_id,
            appid,
        };
        let worker = self.client.detached_quiet();
        std::thread::spawn(move || {
            let _ = worker.request_with_timeout(&request, Duration::from_secs(2));
        });
    }

    /// Les informations publiées sur Discord : celles de la fiche ouverte
    /// (boutique, temps de jeu, installation, jaquette, application du jeu),
    /// ou la ligne chiffrée de la page et sa boutique.
    fn discord_activity_info(
        &self,
        section: &str,
        game_open: bool,
    ) -> (
        Option<String>,
        Option<String>,
        Option<u32>,
        Option<lapinux_core::stores::Store>,
        Option<String>,
    ) {
        if game_open {
            let Some(entry) = self.selected.and_then(|appid| {
                self.library_entries_all()
                    .iter()
                    .find(|entry| entry.appid == appid)
                    .cloned()
            }) else {
                return (None, None, None, None, None);
            };
            // Un jeu de boutique se reconnaît aussi par son identifiant chez
            // elle : c'est lui que le catalogue Discord indexe.
            let store_game = self
                .stores
                .owned_games()
                .into_iter()
                .find(|game| game.appid == entry.appid);
            // « Steam · 12 h de jeu · installé · sur Lapinux » : d'où vient le
            // jeu, ce qu'on y a joué, s'il est prêt à partir, et l'application
            // en dernier, comme une info à part.
            let store = entry.store.map(|store| store.label()).unwrap_or("Steam");
            let mut parts = vec![store.to_string()];
            let minutes = entry.playtime / 60;
            if minutes > 0 {
                parts.push(lapinux_core::i18n::t_fmt(
                    "profile_page.playtime_of",
                    &[played_label(Some(minutes))],
                ));
            }
            parts.push(
                if entry.installed {
                    lapinux_core::i18n::t("settings.captures.installed")
                } else {
                    lapinux_core::i18n::t("profile_page.not_installed_lower")
                }
                .to_string(),
            );
            parts.push(lapinux_core::i18n::t("profile_page.on_lapinux").to_string());
            let cover = (!entry.cover.trim().is_empty()).then(|| entry.cover.clone());
            // Steam se reconnaît à son appid ; une boutique à son identifiant.
            let appid = entry.store.is_none().then_some(entry.appid);
            let (store, store_id) = match store_game {
                Some(game) => (Some(game.store), Some(game.store_id)),
                None => (entry.store, None),
            };
            return (Some(parts.join(" · ")), cover, appid, store, store_id);
        }
        let note = match section {
            "library" => {
                let entries = self.library_entries_all();
                let installed = entries.iter().filter(|entry| entry.installed).count();
                Some(lapinux_core::i18n::t_fmt(
                    "profile_page.library_summary",
                    &[entries.len().to_string(), installed.to_string()],
                ))
            }
            "downloads" => {
                let running = self
                    .jobs
                    .iter()
                    .filter(|job| job.state == JobState::Running)
                    .count();
                let queued = self
                    .jobs
                    .iter()
                    .filter(|job| job.state == JobState::Queued)
                    .count();
                Some(match (running, queued) {
                    (0, 0) => lapinux_core::i18n::t("profile_page.no_downloads").to_string(),
                    (0, queued) => lapinux_core::i18n::t_args(
                        "profile_page.downloads_queued",
                        &[("queued", &queued.to_string())],
                    ),
                    (running, 0) => lapinux_core::i18n::t_args(
                        "profile_page.downloads_running",
                        &[("running", &running.to_string())],
                    ),
                    (running, queued) => lapinux_core::i18n::t_args(
                        "profile_page.downloads_mixed",
                        &[("running", &running.to_string()), ("queued", &queued.to_string())],
                    ),
                })
            }
            "settings" => Some(lapinux_core::i18n::t_fmt(
                "profile_page.settings_of",
                &[self.settings_tab.title().to_string()],
            )),
            "controllers" => {
                let count = self.devices.iter().filter(|device| device.gamepad).count();
                Some(match count {
                    0 => lapinux_core::i18n::t("profile_page.no_pad").to_string(),
                    1 => lapinux_core::i18n::t("profile_page.one_pad").to_string(),
                    count => lapinux_core::i18n::t_args(
                        "profile_page.pad_count",
                        &[("count", &count.to_string())],
                    ),
                })
            }
            "captures" => {
                let count = self.captures.count();
                Some(match count {
                    0 => lapinux_core::i18n::t("profile_page.no_captures").to_string(),
                    1 => lapinux_core::i18n::t("profile_page.one_capture").to_string(),
                    count => lapinux_core::i18n::t_args(
                        "profile_page.captures_count",
                        &[("count", &count.to_string())],
                    ),
                })
            }
            _ => None,
        };
        let store = if !self.settings_open && self.page == Page::Boutiques {
            self.store_filter
        } else {
            None
        };
        (note, None, None, store, None)
    }

    /// Réagit aux commandes de la page Profil : présence publiée, relectures,
    /// copie et ouverture du profil.
    fn apply_profile_actions(&mut self, ctx: &egui::Context, actions: Vec<ProfileAction>) {
        for action in actions {
            match action {
                ProfileAction::SetPresence(presence) => self.set_steam_presence(presence),
                ProfileAction::RedeemKey(key) => self.redeem_steam_key(key),
                // Démonstration : la célébration se joue sans clé ni appel.
                // Démonstration : la célébration se joue sans clé ni appel.
                ProfileAction::TestRedeem => self.start_game_reveal(400, "Portal"),
                ProfileAction::RefreshSocial => self.poll_social(true),
                ProfileAction::JoinInvite {
                    appid,
                    lobby,
                    connect,
                } => self.join_steam_lobby(appid, lobby, connect),
                ProfileAction::Reload => {
                    if let Some(steam_id) = self.profile.steam_id().or_else(|| {
                        self.steam_status
                            .as_ref()
                            .and_then(|status| status.steam_id)
                    }) {
                        self.profile.reload(steam_id);
                    }
                    self.poll_social(true);
                }
                // Une plateforme sans page propre mène à son compte : c'est
                // là qu'on la branche.
                ProfileAction::Accounts(store) => {
                    self.stores.selected = store;
                    self.open_settings(SettingsTab::Connection);
                }
                ProfileAction::Copy(text) => ctx.copy_text(text),
                ProfileAction::Game { appid, name } => {
                    // La fiche du jeu s'ouvre dans la fenêtre de boutique :
                    // c'est la page que Steam publie pour lui.
                    let url = format!("https://store.steampowered.com/app/{appid}/?l=french");
                    let title = if name.trim().is_empty() {
                        lapinux_core::i18n::t_args(
                            "profile_page.store_title",
                            &[("appid", &appid.to_string())],
                        )
                    } else {
                        lapinux_core::i18n::t_args("ui.store_window", &[("name", &name)])
                    };
                    self.webview.open(&self.paths, &url, &title);
                }
                ProfileAction::Open(url) => {
                    let _ = std::process::Command::new("xdg-open")
                        .arg(&url)
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .spawn();
                }
            }
        }
    }

    /// Publie une nouvelle présence sur Steam, et suit la réponse du daemon.
    /// Rejoint le lobby d'un ami : le daemon lance le jeu invité avec
    /// `+connect_lobby` et l'adresse de la partie, que le pont Windows relit.
    fn join_steam_lobby(&mut self, appid: u32, lobby: u64, connect: String) {
        match self.client.request(&Req::SteamLobbyJoin {
            appid,
            lobby,
            connect,
        }) {
            Ok(envelope) if envelope.ok => self.notify("Partie rejointe".to_string()),
            Ok(envelope) => self.notify(envelope.error.unwrap_or_else(|| {
                lapinux_core::i18n::t("profile_page.session_not_joined").into()
            })),
            Err(err) => self.notify(format!("{err}")),
        }
    }

    fn set_steam_presence(&mut self, presence: SteamPresence) {
        match self.client.request(&Req::SteamSocialPresence { presence }) {
            Ok(envelope) if envelope.ok => match envelope.into_data::<SteamSocial>() {
                Ok(social) => self.apply_social_snapshot(social),
                Err(err) => self.notify(format!("{err}")),
            },
            Ok(envelope) => self.notify(envelope.error.unwrap_or_else(|| {
                lapinux_core::i18n::t("profile_page.presence_not_published").into()
            })),
            Err(err) => self.notify(format!("{err}")),
        }
    }

    /// Active une clé produit Steam : le daemon la transmet au CM et
    /// l'historique s'en souvient ; le verdict arrive avec l'état social.
    fn redeem_steam_key(&mut self, key: String) {
        match self
            .client
            .request_with_timeout(&Req::SteamSocialRedeemKey { key }, Duration::from_secs(5))
        {
            Ok(envelope) if envelope.ok => match envelope.into_data::<SteamSocial>() {
                Ok(social) => {
                    self.apply_social_snapshot(social);
                    self.profile.set_social(self.social.clone());
                }
                Err(err) => self.notify(format!("{err}")),
            },
            Ok(envelope) => {
                self.notify(envelope.error.unwrap_or_else(|| {
                    lapinux_core::i18n::t("profile_page.key_not_activated").into()
                }))
            }
            Err(err) => self.notify(format!("{err}")),
        }
    }

    /// Applique un snapshot social sans faire disparaître la famille pendant
    /// une pause du catalogue ou une reconnexion : seule une famille lue il y
    /// a longtemps et absente d'un état vivant est réellement retirée.
    fn apply_social_snapshot(&mut self, mut social: SteamSocial) {
        let family_present = social
            .family
            .as_ref()
            .is_some_and(|family| !family.members.is_empty());
        let keep_missing = !family_present
            && self
                .family_seen_at
                .is_some_and(|at| at.elapsed() < FAMILY_SNAPSHOT_GRACE);
        keep_family_through_snapshot(self.social.as_ref(), &mut social, keep_missing);
        // Une pause du catalogue ou une reconnexion ne doit pas effacer les
        // dernières notifications reçues : le panneau les garde lisibles
        // jusqu'au prochain état vivant.
        if !social.live() {
            if let Some(previous) = self.social.as_ref() {
                if social.notifications.is_empty() {
                    social.notifications = previous.notifications.clone();
                }
                if social.web_notifications.is_empty() {
                    social.web_notifications = previous.web_notifications.clone();
                }
            }
        }
        if family_present {
            self.family_seen_at = Some(Instant::now());
        }
        self.observe_friend_events(&social);
        self.observe_steam_messages(&social);
        self.social = Some(social);
    }

    /// Nom d'un jeu connu localement : bibliothèque installée, catalogue ou
    /// jeux possédés dans une boutique. Les événements d'amis n'arrivent pas
    /// toujours avec leur titre ; on le retrouve avant de dire « Un Jeu ».
    fn known_game_name(&self, appid: u32) -> Option<String> {
        if appid == 0 {
            return None;
        }
        self.games
            .iter()
            .find(|game| game.appid == appid && !game.name.trim().is_empty())
            .map(|game| game.name.clone())
            .or_else(|| {
                self.catalog.as_ref().and_then(|catalog| {
                    catalog
                        .entries
                        .iter()
                        .find(|entry| entry.appid == appid && !entry.name.trim().is_empty())
                        .map(|entry| entry.name.clone())
                })
            })
            .or_else(|| {
                self.stores
                    .owned_games()
                    .into_iter()
                    .find(|game| game.appid == appid && !game.name.trim().is_empty())
                    .map(|game| game.name.clone())
            })
    }

    /// Repère les nouvelles transitions d'amis. Le premier état vivant sert
    /// de référence : les amis déjà connectés au démarrage ne produisent pas
    /// une rafale de notifications.
    fn observe_friend_events(&mut self, social: &SteamSocial) {
        if !social.live() {
            return;
        }
        // Le daemon complète parfois le nom après le premier snapshot ; on
        // résout aussi ici, pour que l'historique, le toast et la barre du haut
        // ne montrent jamais « Un Jeu » quand le jeu est dans le catalogue.
        // Le cache reflète l'instantané précédent : le mettre à jour avant de
        // lire les événements ferait résoudre un « jeu quitté » vers le jeu
        // suivant quand l'ami vient d'en changer.
        let previous_games = self.friend_games.clone();
        // L'historique prend le relais après un redémarrage : le dernier jeu
        // connu d'un ami y figure encore même si le cache mémoire est vide.
        let mut known_games = previous_games.clone();
        for event in &self.friend_events_history {
            if !event.game.trim().is_empty() {
                known_games
                    .entry(event.steam_id)
                    .or_insert_with(|| (event.appid, event.game.clone()));
            }
        }
        let mut events = social.friend_events.clone();
        for event in &mut events {
            if event.game.trim().is_empty() {
                if let Some(friend) = social
                    .friends
                    .iter()
                    .find(|friend| friend.steam_id == event.steam_id)
                {
                    event.game = friend.game.clone();
                    if event.appid == 0 {
                        event.appid = friend.game_appid;
                    }
                }
            }
            if event.game.trim().is_empty() {
                if let Some(name) = self.known_game_name(event.appid) {
                    event.game = name;
                }
            }
            // Un « jeu quitté » arrive après la disparition du jeu de la
            // présence : le dernier jeu connu de cet ami est le bon titre.
            if event.game.trim().is_empty() {
                if let Some((appid, name)) = known_games.get(&event.steam_id) {
                    // Un arrêt sans appid désigne le dernier jeu connu. Pour un
                    // lancement, on ne réutilise ce titre que si l'appid est le
                    // même : sinon on afficherait le jeu précédent.
                    let same_game = event.appid == 0 || event.appid == *appid;
                    let stopped =
                        event.kind == lapinux_core::social::SteamFriendEventKind::GameStopped;
                    if (stopped && same_game) || (event.appid != 0 && same_game) {
                        event.game = name.clone();
                        if event.appid == 0 {
                            event.appid = *appid;
                        }
                    }
                }
            }
        }
        // Le jeu courant est la référence pour le prochain événement. Un
        // lancement plus récent corrige une présence encore en retard ; un
        // arrêt, lui, ne remplace jamais le dernier jeu mémorisé.
        for friend in &social.friends {
            if !friend.game.trim().is_empty() {
                self.friend_games
                    .insert(friend.steam_id, (friend.game_appid, friend.game.clone()));
            }
        }
        // Les événements arrivent du plus récent au plus ancien ; on garde le
        // lancement le plus récent par ami, pas le dernier parcouru.
        let mut latest_starts: HashMap<u64, (i64, u32, String)> = HashMap::new();
        for event in &events {
            if event.kind == lapinux_core::social::SteamFriendEventKind::GameStarted
                && !event.game.trim().is_empty()
            {
                let slot = latest_starts
                    .entry(event.steam_id)
                    .or_insert_with(|| (event.at, event.appid, event.game.clone()));
                if event.at > slot.0 {
                    *slot = (event.at, event.appid, event.game.clone());
                }
            }
        }
        for (steam_id, (_, appid, game)) in latest_starts {
            self.friend_games.insert(steam_id, (appid, game));
        }
        // Le daemon complète le nom du jeu après le premier snapshot : mettre
        // à jour les entrées déjà enregistrées, sans les compter comme
        // nouvelles.
        let mut history_changed = false;
        for event in &events {
            let key = friend_event_key(event);
            if let Some(existing) = self
                .friend_events_history
                .iter_mut()
                .find(|existing| friend_event_key(existing) == key)
            {
                if existing.game.trim().is_empty() && !event.game.trim().is_empty() {
                    existing.game = event.game.clone();
                    history_changed = true;
                }
                if existing.avatar.trim().is_empty() && !event.avatar.trim().is_empty() {
                    existing.avatar = event.avatar.clone();
                    history_changed = true;
                }
            }
        }
        // Complète aussi les anciennes entrées avec le dernier jeu mémorisé :
        // un redémarrage peut avoir conservé un événement sans son titre.
        let last_games = self.friend_games.clone();
        for existing in &mut self.friend_events_history {
            if existing.game.trim().is_empty() {
                if let Some((appid, name)) = last_games.get(&existing.steam_id) {
                    // Un appid différent désigne un autre jeu : ne pas lui
                    // donner le titre du jeu que l'ami occupe aujourd'hui.
                    if existing.appid == 0 || existing.appid == *appid {
                        existing.game = name.clone();
                        if existing.appid == 0 {
                            existing.appid = *appid;
                        }
                        history_changed = true;
                    }
                }
            }
        }
        let mut fresh = Vec::new();
        for event in &events {
            if self.friend_events_seen.insert(friend_event_key(event)) {
                fresh.push(event.clone());
            }
        }
        // Une notification supprimée depuis le panneau ne doit pas revenir au
        // prochain instantané Steam. La clé reste mémorisée dans la config.
        fresh.retain(|event| {
            !self
                .config
                .notifications
                .dismissed_notifications
                .contains(&friend_notification_key(event))
        });
        if !fresh.is_empty() {
            // La liste arrive du plus récent au plus ancien : on remplit
            // l'historique dans cet ordre, puis on le plafonne.
            for event in fresh.iter().rev() {
                self.friend_events_history.push_front(event.clone());
            }
            self.friend_events_history.truncate(100);
            history_changed = true;
        }
        let cutoff = notification_cutoff(&self.config.notifications, notification_now());
        history_changed |= prune_old_friend_events(&mut self.friend_events_history, cutoff);
        if history_changed {
            friend_events_save(&self.paths, &self.friend_events_history);
        }
        if !self.friend_events_initialized {
            self.friend_events_initialized = true;
            return;
        }
        for event in fresh.iter().rev() {
            if !self.friend_event_enabled(event.kind) {
                continue;
            }
            self.notify(Self::friend_event_text(event));
            let friend = social
                .friends
                .iter()
                .find(|friend| friend.steam_id == event.steam_id);
            let avatar = if !event.avatar.trim().is_empty() {
                event.avatar.clone()
            } else {
                friend
                    .map(|friend| friend.avatar.clone())
                    .unwrap_or_default()
            };
            let mut caption_event = event.clone();
            if caption_event.game.trim().is_empty() {
                if let Some(friend) = friend {
                    caption_event.game = friend.game.clone();
                    if caption_event.appid == 0 {
                        caption_event.appid = friend.game_appid;
                    }
                }
            }
            let name = if caption_event.name.trim().is_empty() {
                lapinux_core::i18n::t("notifications_panel.a_friend").to_string()
            } else {
                caption_event.name.clone()
            };
            self.friend_activity = Some(FriendActivityFlash {
                steam_id: event.steam_id,
                name,
                avatar,
                game: caption_event.game.clone(),
                appid: caption_event.appid,
                caption: Self::friend_activity_caption(&caption_event),
                started: Instant::now(),
            });
            // L'ouverture du panneau remet le compteur à zéro ; un événement
            // suivant doit donc rallumer la pastille, même panneau ouvert.
            self.friend_events_unread = self.friend_events_unread.saturating_add(1);
        }
    }

    /// Signale les messages Steam reçus en direct. Les messages relus dans un
    /// historique ne portent pas `received_live` : les charger ne produit donc
    /// jamais une rafale de notifications. Le premier instantané vivant sert
    /// de référence, et la conversation déjà ouverte reste silencieuse.
    fn observe_steam_messages(&mut self, social: &SteamSocial) {
        if !social.live() {
            return;
        }
        let received: Vec<(u64, &lapinux_core::social::SteamChatMessage)> = social
            .chats
            .iter()
            .flat_map(|(friend, messages)| {
                messages
                    .iter()
                    .filter(|message| message.received_live && !message.outgoing)
                    .map(move |message| (*friend, message))
            })
            .collect();
        let current: HashSet<String> = received
            .iter()
            .map(|(friend, message)| steam_message_key(*friend, message))
            .collect();
        self.steam_messages_seen.retain(|key| current.contains(key));
        if !self.steam_messages_initialized {
            self.steam_messages_initialized = true;
            self.steam_messages_seen = current;
            return;
        }
        let mut fresh: Vec<(u64, lapinux_core::social::SteamChatMessage)> = received
            .into_iter()
            .filter(|(friend, message)| {
                !self
                    .steam_messages_seen
                    .contains(&steam_message_key(*friend, message))
            })
            .map(|(friend, message)| (friend, message.clone()))
            .collect();
        if fresh.is_empty() {
            return;
        }
        fresh.sort_by_key(|(_, message)| message.at);
        for (friend, message) in fresh {
            self.steam_messages_seen
                .insert(steam_message_key(friend, &message));
            if !self.config.notifications.friend_messages_enabled() {
                continue;
            }
            let chat_key = format!("steam:{friend}");
            if self.friends_open && self.friends_chat.as_deref() == Some(chat_key.as_str()) {
                continue;
            }
            let entry = social.friends.iter().find(|entry| entry.steam_id == friend);
            let name = entry
                .map(|entry| entry.name.trim())
                .filter(|name| !name.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| {
                    lapinux_core::i18n::t("notifications_panel.a_friend").to_string()
                });
            let avatar = entry.map(|entry| entry.avatar.clone()).unwrap_or_default();
            let preview = message_notification_preview(&message.text);
            self.notify(lapinux_core::i18n::t_args(
                "notifications_panel.friend_message",
                &[("name", name.as_str()), ("text", preview.as_str())],
            ));
            self.friend_activity = Some(FriendActivityFlash {
                steam_id: friend,
                name,
                avatar,
                game: String::new(),
                appid: 0,
                caption: lapinux_core::i18n::t_args(
                    "notifications_panel.friend_message_caption",
                    &[("text", preview.as_str())],
                ),
                started: Instant::now(),
            });
        }
    }

    /// Respecte les cases « Notifications d'amis » des réglages.
    pub(super) fn friend_event_enabled(
        &self,
        kind: lapinux_core::social::SteamFriendEventKind,
    ) -> bool {
        self.config.notifications.friend_event_enabled(kind)
    }

    /// Supprime les événements d'amis trop anciens après un changement de
    /// durée de conservation.
    pub(super) fn prune_friend_events_history(&mut self) {
        let cutoff = notification_cutoff(&self.config.notifications, notification_now());
        if prune_old_friend_events(&mut self.friend_events_history, cutoff) {
            friend_events_save(&self.paths, &self.friend_events_history);
        }
    }

    /// Aperçu à la demande : réutilise un ami réel pour montrer l'avatar et la
    /// bulle de la barre du haut, sans attendre un événement Steam.
    pub(super) fn start_notification_animation_test(&mut self) {
        let (steam_id, name, avatar, appid, game) = {
            let friend = self.social.as_ref().and_then(|social| {
                social
                    .friends
                    .iter()
                    .find(|friend| {
                        !friend.avatar.trim().is_empty()
                            && friend.game_appid != 0
                            && !friend.game.trim().is_empty()
                    })
                    .or_else(|| {
                        social
                            .friends
                            .iter()
                            .find(|friend| !friend.avatar.trim().is_empty())
                    })
                    .or_else(|| social.friends.first())
            });
            let friend_game = friend
                .filter(|friend| friend.game_appid != 0 && !friend.game.trim().is_empty())
                .map(|friend| (friend.game_appid, friend.game.clone()));
            let catalog_game = self.catalog.as_ref().and_then(|catalog| {
                catalog
                    .entries
                    .iter()
                    .find(|entry| entry.appid != 0 && !entry.name.trim().is_empty())
                    .map(|entry| (entry.appid, entry.name.clone()))
            });
            let (appid, game) = friend_game.or(catalog_game).unwrap_or((0, String::new()));
            let (steam_id, name, avatar) = friend.map_or_else(
                || (0, lapinux_core::APP_NAME.to_string(), String::new()),
                |friend| {
                    (
                        friend.steam_id,
                        if friend.name.trim().is_empty() {
                            lapinux_core::APP_NAME.to_string()
                        } else {
                            friend.name.clone()
                        },
                        friend.avatar.clone(),
                    )
                },
            );
            (steam_id, name, avatar, appid, game)
        };
        let caption = if game.trim().is_empty() {
            lapinux_core::i18n::t("settings.notifications.test_animation_caption").to_string()
        } else {
            lapinux_core::i18n::t_args(
                "notifications_panel.friend_activity_started",
                &[("game", game.as_str())],
            )
        };
        self.friend_activity = Some(FriendActivityFlash {
            steam_id,
            name,
            avatar,
            game,
            appid,
            caption,
            started: Instant::now(),
        });
    }

    /// Légende courte affichée sous l'avatar, à la place de l'icône Amis.
    fn friend_activity_caption(event: &lapinux_core::social::SteamFriendEvent) -> String {
        use lapinux_core::social::SteamFriendEventKind;
        let game = event.game.trim();
        let game = if game.is_empty() {
            lapinux_core::i18n::t("notifications_panel.a_game")
        } else {
            game
        };
        match event.kind {
            SteamFriendEventKind::Online => {
                lapinux_core::i18n::t("notifications_panel.friend_activity_online").to_string()
            }
            SteamFriendEventKind::Offline => {
                lapinux_core::i18n::t("notifications_panel.friend_activity_offline").to_string()
            }
            SteamFriendEventKind::GameStarted => lapinux_core::i18n::t_args(
                "notifications_panel.friend_activity_started",
                &[("game", game)],
            ),
            SteamFriendEventKind::GameStopped => lapinux_core::i18n::t_args(
                "notifications_panel.friend_activity_stopped",
                &[("game", game)],
            ),
        }
    }

    /// Phrase affichée dans le toast et dans le modal pour un événement d'ami.
    pub(super) fn friend_event_text(event: &lapinux_core::social::SteamFriendEvent) -> String {
        use lapinux_core::social::SteamFriendEventKind;
        let name = event.name.trim();
        let name = if name.is_empty() {
            lapinux_core::i18n::t("notifications_panel.a_friend")
        } else {
            name
        };
        let game = event.game.trim();
        let game = if game.is_empty() {
            lapinux_core::i18n::t("notifications_panel.a_game")
        } else {
            game
        };
        match event.kind {
            SteamFriendEventKind::Online => {
                lapinux_core::i18n::t_args("notifications_panel.friend_online", &[("name", name)])
            }
            SteamFriendEventKind::Offline => {
                lapinux_core::i18n::t_args("notifications_panel.friend_offline", &[("name", name)])
            }
            SteamFriendEventKind::GameStarted => lapinux_core::i18n::t_args(
                "notifications_panel.friend_game_started",
                &[("name", name), ("game", game)],
            ),
            SteamFriendEventKind::GameStopped => lapinux_core::i18n::t_args(
                "notifications_panel.friend_game_stopped",
                &[("name", name), ("game", game)],
            ),
        }
    }

    /// Lit l'état social du daemon, qui abrite la session Steam.
    pub(super) fn poll_social(&mut self, refresh: bool) {
        match self
            .client
            .request_with_timeout(&Req::SteamSocial { refresh }, Duration::from_secs(5))
        {
            Ok(envelope) if envelope.ok => {
                if let Ok(social) = envelope.into_data::<SteamSocial>() {
                    self.apply_social_snapshot(social);
                }
            }
            _ => {}
        }
        self.profile.set_social(self.social.clone());
    }
}
