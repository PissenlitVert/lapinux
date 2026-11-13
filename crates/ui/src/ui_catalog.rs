//! Synchronisation du catalogue et des actualités.

use super::*;

impl LapinuxApp {
    /// Ne relit le catalogue seul que s'il est vide : un cache existant suffit
    /// au démarrage. Une simple ancienneté ne doit plus déclencher une rafale
    /// de requêtes Steam à chaque lancement ; la synchronisation manuelle et le
    /// contrôle léger du daemon couvrent les nouveaux achats.
    pub(super) fn ensure_catalog_freshness(&mut self) {
        if self.catalog_freshness_checked || !self.steam_account_known() {
            return;
        }
        let Some(catalog) = self.catalog.as_ref() else {
            return;
        };
        if catalog.syncing || self.catalog_rx.is_some() {
            return;
        }
        self.catalog_freshness_checked = true;
        if catalog.entries.is_empty() {
            self.catalog_auto = true;
            self.load_catalog(true);
        }
    }

    /// Relit le catalogue du compte : `refresh` interroge Steam, sinon le cache
    /// disque du daemon suffit.
    pub(super) fn load_catalog(&mut self, refresh: bool) {
        // La demande est seulement notée : la lecture passe par un fil, la
        // peinture n'attend jamais le réseau.
        self.catalog_want = Some(refresh);
    }

    /// La session Lapinux permet de relire les licences sans mot de passe :
    /// elle seule autorise la synchronisation automatique du catalogue. Un nom
    /// de compte resté en configuration ne suffit pas, son jeton peut avoir
    /// été oublié.
    pub(super) fn steam_account_known(&self) -> bool {
        self.steam_status
            .as_ref()
            .map(|status| status.token_saved)
            .unwrap_or(false)
    }

    /// Suit la synchronisation du catalogue, quelle que soit la page ouverte.
    /// Tant qu'elle tourne, la comparaison des nouveaux jeux attend ; à la fin,
    /// elle repart sur les données fraîches, pour que le jeu acheté la veille
    /// apparaisse dans la carte « Nouveaux jeux ».
    pub(super) fn follow_catalog_sync(&mut self, ctx: &egui::Context) {
        // La jaquette d'un jeu qui vient d'être activé n'est pas encore sur le
        // disque : la célébration l'attend, sinon le bandeau s'ouvre sur
        // l'initiale et l'image arrive après lui.
        if self.game_reveal.is_none() {
            if let Some((appid, name)) = self.reveal_queue.take() {
                self.reveal_wait = Some((appid, name, Instant::now()));
            }
        }
        let mut waiting = self.reveal_wait.clone();
        if let Some((appid, name, _)) = waiting.clone() {
            if self.game_reveal.is_some() {
                // Une célébration tourne : celle-ci prend la file.
                self.reveal_wait = None;
                self.reveal_queue = Some((appid, name));
                waiting = None;
            }
        }
        if let Some((appid, name, since)) = waiting {
            // La demande part par le cache de la grille ; la disponibilité,
            // elle, se lit dans le fichier.
            if appid != 0 {
                self.covers
                    .get(ctx, &self.paths, appid, &name, None, "", CoverTier::Sheet);
            }
            let ready = appid != 0 && self.reveal_cover_texture(ctx, appid).is_some();
            if ready || appid == 0 || since.elapsed() > Duration::from_secs(5) {
                self.reveal_wait = None;
                self.start_game_reveal(appid, name);
            } else {
                ctx.request_repaint_after(Duration::from_millis(80));
            }
        }
        // Une activation en attente d'identification relit le catalogue
        // jusqu'à ce que la clé y apparaisse.
        if let Some(pending) = self.reveal_pending.clone() {
            // La fiche du jeu arrive une seconde ou deux après la demande :
            // relire à ce rythme la fait voir tout de suite, au lieu
            // d'attendre la synchronisation suivante.
            ctx.request_repaint_after(Duration::from_millis(700));
            if self.catalog_poll.elapsed() > Duration::from_millis(700) {
                self.catalog_poll = Instant::now();
                self.load_catalog(false);
            }
            // La fiche tarde : le bandeau part quand même, avec ce qu'on a,
            // plutôt que de laisser l'activation sans célébration. Une fiche
            // qui arrive pendant ce bandeau le complète sur place.
            if pending.since.elapsed() > Duration::from_secs(12) && self.game_reveal.is_none() {
                self.reveal_pending = None;
                let name = pending
                    .name
                    .unwrap_or_else(|| lapinux_core::i18n::t("catalog.new_game").to_string());
                self.start_game_reveal(0, name);
            }
        }
        let syncing = self.catalog.as_ref().map(|c| c.syncing).unwrap_or(false);
        if syncing {
            ctx.request_repaint_after(Duration::from_millis(600));
            if self.catalog_poll.elapsed() > Duration::from_millis(700) {
                self.catalog_poll = Instant::now();
                self.load_catalog(false);
            }
        }
        if self.catalog_was_syncing && !syncing {
            let error = self.catalog.as_ref().and_then(|c| c.error.clone());
            let count = self.catalog.as_ref().map(|c| c.entries.len()).unwrap_or(0);
            match (error, self.catalog_notice) {
                (Some(error), _) => self.notify(error),
                (None, true) => self.notify(lapinux_core::i18n::t_args(
                    "catalog.synced",
                    &[("count", &count.to_string())],
                )),
                (None, false) => {}
            }
            self.catalog_notice = false;
            // La synchronisation n'écrit son catalogue qu'à la toute fin :
            // sans cette relecture, l'interface gardait la version d'avant et
            // un jeu fraîchement activé restait invisible dans la bibliothèque.
            self.load_catalog(false);
            self.touch_library();
            self.library_checked = false;
            self.library_check_at = Some(Instant::now());
        }
        self.catalog_was_syncing = syncing;
    }

    /// Lance la lecture demandée, s'il y en a une et si aucune n'est en vol.
    pub(super) fn ensure_catalog(&mut self, ctx: &egui::Context) {
        let Some(refresh) = self.catalog_want.take() else {
            return;
        };
        if self.catalog_rx.is_some() {
            // Une lecture est déjà en cours : la demande repassera après elle.
            self.catalog_want = Some(refresh);
            return;
        }
        self.catalog_loaded = true;
        // Une activation de clé ne demande que le jeu qu'elle vient d'ajouter :
        // relire toute la bibliothèque prendrait deux minutes pour un appid.
        let request = match self.catalog_patch.take() {
            Some((appid, name)) => Req::SteamCatalogPatch { appid, name },
            None => Req::SteamCatalog { refresh },
        };
        let (tx, rx) = mpsc::channel();
        self.catalog_rx = Some(rx);
        let worker = self.client.detached();
        let wake = ctx.clone();
        std::thread::spawn(move || {
            let reply = match worker.request(&request) {
                Ok(envelope) if envelope.ok => {
                    envelope.into_data::<SteamCatalog>().map_err(|err| {
                        lapinux_core::i18n::t_args(
                            "catalog.unexpected_response",
                            &[("err", &err.to_string())],
                        )
                    })
                }
                Ok(envelope) => Err(envelope
                    .error
                    .unwrap_or_else(|| lapinux_core::i18n::t("catalog.unavailable").into())),
                Err(err) => Err(format!("{err}")),
            };
            if tx.send((refresh, reply)).is_ok() {
                wake.request_repaint();
            }
        });
    }

    /// Relève la lecture du catalogue : la liste fusionnée et les mises à jour
    /// sont périmées dès qu'elle arrive.
    pub(super) fn poll_catalog(&mut self, ctx: &egui::Context) {
        let Some(rx) = self.catalog_rx.as_ref() else {
            return;
        };
        match rx.try_recv() {
            Ok((refresh, Ok(catalog))) => {
                self.catalog_rx = None;
                let automatic = std::mem::take(&mut self.catalog_auto);
                let count = catalog.entries.len();
                self.catalog = Some(catalog);
                self.touch_library();
                self.downloads.mark_updates_dirty();
                // Une clé que Steam n'a pas nommée se révèle ici, avec sa
                // jaquette, son icône et son vrai nom.
                self.resolve_pending_reveal();
                // La vérification de fraîcheur ne parle pas : elle relit le
                // compte sans qu'on l'ait demandé.
                if refresh && !automatic {
                    self.notify(lapinux_core::i18n::t_args(
                        "catalog.owned",
                        &[("count", &count.to_string())],
                    ));
                }
            }
            Ok((_, Err(error))) => {
                self.catalog_rx = None;
                self.catalog_auto = false;
                // Une demande qui n'a pas abouti ne laisse pas un message de
                // réussite en attente pour la synchronisation suivante.
                self.catalog_notice = false;
                self.notify(error);
            }
            Err(mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_secs(1));
            }
            Err(mpsc::TryRecvError::Disconnected) => self.catalog_rx = None,
        }
    }

    /// Tableau de bord : jeux, présences connues et activité en cours.
    pub(super) fn home_page(&mut self, ui: &mut egui::Ui) {
        self.ensure_home_friends(ui.ctx());
        let entries = self.library_entries_all();
        let mut recent: Vec<_> = entries
            .iter()
            .filter(|entry| entry.last_played > 0)
            .collect();
        recent.sort_by(|a, b| {
            b.last_played
                .cmp(&a.last_played)
                .then_with(|| a.name.cmp(&b.name))
        });
        let mut installed: Vec<_> = entries.iter().filter(|entry| entry.installed).collect();
        installed.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        let controllers = merge_controllers(
            self.devices
                .iter()
                .filter(|device| device.gamepad && device.connected)
                .cloned()
                .collect(),
            Vec::new(),
        );
        // Toute la bibliothèque alimente le flux, comme l'onglet Annonces :
        // une annonce récente d'un jeu non installé doit aussi pouvoir remonter
        // sur l'Accueil.
        self.ensure_news_games(&entries);
        let (updates, loading_updates) =
            self.news_feed
                .home_updates(ui.ctx(), &self.news_games, &self.config.general.lang);
        let news_progress = self.news_feed.home_progress();
        let friends = home_friends(
            self.social.as_ref(),
            &self.store_friends,
            self.friends_store_available,
        );
        let capsule_appids: Vec<u32> = friends
            .iter()
            .filter(|friend| friend.in_game && friend.appid != 0)
            .map(|friend| friend.appid)
            .collect();
        self.profile.ensure_game_capsules(ui.ctx(), &capsule_appids);
        let (known, _) = home_friends_status(
            self.social.as_ref(),
            &self.store_friends,
            self.friends_store_available,
        );
        self.covers.begin_frame(6);
        self.covers
            .set_tile(self.grid_width, ui.ctx().pixels_per_point());
        let mut actions = HomeActions::default();

        egui::ScrollArea::vertical()
            .id_salt("home-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_max_width(1280.0);
                let stats = [
                    (
                        entries.len().to_string(),
                        lapinux_core::i18n::t("home.library"),
                        Some(Page::Library),
                    ),
                    (
                        if known {
                            friends.len().to_string()
                        } else {
                            "—".to_string()
                        },
                        lapinux_core::i18n::t("home.friends_online"),
                        None,
                    ),
                    (
                        controllers.len().to_string(),
                        lapinux_core::i18n::t("home.controllers"),
                        Some(Page::Controllers),
                    ),
                ];
                let columns = if ui.available_width() >= 760.0 {
                    3
                } else if ui.available_width() >= 340.0 {
                    2
                } else {
                    1
                };
                for row in stats.chunks(columns) {
                    ui.columns(columns, |uis| {
                        for (index, (value, label, target)) in row.iter().enumerate() {
                            if home_stat(&mut uis[index], value, label, *target) {
                                if let Some(target) = target {
                                    actions.page = Some(*target);
                                } else {
                                    actions.friends = true;
                                }
                            }
                        }
                    });
                    ui.add_space(9.0);
                }
                ui.add_space(5.0);
                if ui.available_width() >= 800.0 {
                    let gap = ui.spacing().item_spacing.x;
                    let side = (ui.available_width() * 0.34).clamp(300.0, 380.0);
                    let main = ui.available_width() - side - gap;
                    ui.horizontal_top(|ui| {
                        ui.allocate_ui_with_layout(
                            egui::vec2(main, 0.0),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                ui.set_width(main);
                                self.home_main(
                                    ui,
                                    &recent,
                                    &installed,
                                    &updates,
                                    loading_updates,
                                    news_progress,
                                    &mut actions,
                                );
                            },
                        );
                        ui.allocate_ui_with_layout(
                            egui::vec2(side, 0.0),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                ui.set_width(side);
                                self.home_side(ui, &friends, known, &mut actions);
                            },
                        );
                    });
                } else {
                    self.home_main(
                        ui,
                        &recent,
                        &installed,
                        &updates,
                        loading_updates,
                        news_progress,
                        &mut actions,
                    );
                    self.home_side(ui, &friends, known, &mut actions);
                }
            });
        if let Some(update) = actions.update {
            if let Some(article) = self.news_feed.open_home_update(&update) {
                self.open_news_reader(article.items, article.at);
            }
        }
        if let Some(appid) = actions.game {
            self.selected = Some(appid);
            actions.page = Some(Page::Library);
        }
        if let Some(page) = actions.page {
            if page == Page::Controllers {
                self.controllers_tab = ControllersTab::Devices;
            }
            self.page = page;
        }
        if actions.friends && !self.friends_open {
            self.toggle_friends();
        }
    }

    fn home_main(
        &mut self,
        ui: &mut egui::Ui,
        recent: &[&LibraryEntry],
        installed: &[&LibraryEntry],
        updates: &[news::HomeUpdate],
        loading: bool,
        progress: Option<(usize, usize)>,
        actions: &mut HomeActions,
    ) {
        home_card_with_fill(ui, "", "", home_group_fill(), |ui| {
            Self::home_section_title(ui, lapinux_core::i18n::t("home.recent_title"));
            if recent.is_empty() {
                muted(ui, lapinux_core::i18n::t("home.no_recent"));
            } else {
                egui::ScrollArea::horizontal()
                    .id_salt("home-recent-games")
                    .show(ui, |ui| {
                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing.x = 14.0;
                            for entry in recent.iter().take(8) {
                                ui.vertical(|ui| {
                                    ui.set_width(132.0);
                                    let texture = self.covers.get(
                                        ui.ctx(),
                                        &self.paths,
                                        entry.appid,
                                        &entry.name,
                                        entry.store,
                                        &entry.cover,
                                        CoverTier::Tile,
                                    );
                                    if game_card(
                                        ui,
                                        texture.as_ref(),
                                        &entry.name,
                                        false,
                                        132.0,
                                        9.0,
                                        entry.steam,
                                        entry.family,
                                        entry.family_available,
                                        entry.store,
                                        self.config.library.show_badges,
                                        self.config.library.badge_scale,
                                        entry.update.is_some(),
                                        false,
                                    )
                                    .clicked()
                                    {
                                        actions.game = Some(entry.appid);
                                    }
                                    ui.add_space(4.0);
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(&entry.name)
                                                .size(12.5)
                                                .strong()
                                                .color(TEXT()),
                                        )
                                        .truncate(),
                                    );
                                    ui.label(
                                        egui::RichText::new(lapinux_core::util::format_date(
                                            entry.last_played,
                                        ))
                                        .size(11.0)
                                        .color(TEXT_DIM()),
                                    );
                                });
                            }
                        });
                    });
            }
        });
        home_card_with_fill(ui, "", "", home_group_fill(), |ui| {
            Self::home_section_title(ui, lapinux_core::i18n::t("home.announcements_title"));
            if let Some((done, total)) = progress.filter(|(_, total)| *total > 0) {
                let done_text = done.to_string();
                let total_text = total.to_string();
                muted(
                    ui,
                    lapinux_core::i18n::t_args(
                        "home.loading_progress",
                        &[("done", done_text.as_str()), ("total", total_text.as_str())],
                    ),
                );
                let fraction = (done as f32 / total as f32).clamp(0.0, 1.0);
                let shown = ui.ctx().animate_value_with_time(
                    ui.id().with("home-news-progress"),
                    fraction,
                    0.25,
                );
                if (shown - fraction).abs() > 0.001 {
                    ui.ctx().request_repaint();
                }
                let (bar, _) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), 3.0),
                    egui::Sense::hover(),
                );
                ui.painter().rect_filled(bar, 1.5, CARD_ACTIVE());
                if shown > 0.0 {
                    ui.painter().rect_filled(
                        egui::Rect::from_min_size(
                            bar.min,
                            egui::vec2(bar.width() * shown, bar.height()),
                        ),
                        1.5,
                        ACCENT(),
                    );
                }
                ui.add_space(8.0);
            }
            if updates.is_empty() {
                muted(
                    ui,
                    lapinux_core::i18n::t(if loading && progress.is_none() {
                        "home.loading_updates"
                    } else {
                        "home.no_updates"
                    }),
                );
                if !loading {
                    ui.add_space(6.0);
                    if crate::action_button(
                        ui,
                        lapinux_core::i18n::t("news.load"),
                        crate::ActionTone::Quiet,
                    )
                    .clicked()
                    {
                        actions.page = Some(Page::News);
                    }
                }
            } else {
                for update in updates {
                    ui.horizontal_top(|ui| {
                        let image = self.news_feed.home_image(ui.ctx(), &update.image);
                        let (thumb, _) =
                            ui.allocate_exact_size(egui::vec2(76.0, 42.0), egui::Sense::hover());
                        match image {
                            Some(texture) => {
                                egui::Image::new((texture.id(), thumb.size()))
                                    .rounding(egui::Rounding::same(6.0))
                                    .paint_at(ui, thumb);
                            }
                            None => {
                                ui.painter().rect_filled(
                                    thumb,
                                    egui::Rounding::same(6.0),
                                    mix(CARD_ACTIVE(), ACCENT(), 0.12),
                                );
                                section_icons::page(
                                    ui.painter(),
                                    Page::News,
                                    thumb.center(),
                                    18.0,
                                    ACCENT(),
                                );
                            }
                        }
                        ui.vertical(|ui| {
                            let title = ui
                                .add(
                                    egui::Label::new(
                                        egui::RichText::new(&update.title)
                                            .size(12.5)
                                            .strong()
                                            .color(TEXT()),
                                    )
                                    .truncate()
                                    .sense(egui::Sense::click()),
                                )
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .on_hover_text(&update.title);
                            if title.clicked() {
                                actions.update = Some(update.clone());
                            }
                            ui.add_space(3.0);
                            muted(
                                ui,
                                if update.date > 0 {
                                    format!(
                                        "{} · {}",
                                        update.game,
                                        lapinux_core::util::format_date(update.date)
                                    )
                                } else {
                                    update.game.clone()
                                },
                            );
                        });
                    });
                    ui.add_space(7.0);
                }
            }
            ui.add_space(5.0);
            ui.vertical_centered(|ui| {
                if action_button(
                    ui,
                    lapinux_core::i18n::t("home.open_news"),
                    ActionTone::Light,
                )
                .clicked()
                {
                    actions.page = Some(Page::News);
                }
            });
        });
        let installed_icons: Vec<Option<egui::TextureHandle>> = installed
            .iter()
            .take(8)
            .map(|entry| {
                let icon = if entry.store.is_some() {
                    // Un jeu de boutique n'a pas d'appid Steam : on lit
                    // l'icône SGDB déjà téléchargée par sa fiche, sans
                    // interroger le CDN Steam. La jaquette reste le repli.
                    self.art.cached_icon(ui.ctx(), &self.paths, entry.appid)
                } else {
                    self.art.game_icon(ui.ctx(), &self.paths, entry.appid)
                };
                icon.or_else(|| {
                    self.covers.get(
                        ui.ctx(),
                        &self.paths,
                        entry.appid,
                        &entry.name,
                        entry.store,
                        &entry.cover,
                        CoverTier::Tile,
                    )
                })
            })
            .collect();
        home_card_with_fill(ui, "", "", home_group_fill(), |ui| {
            Self::home_section_title(ui, lapinux_core::i18n::t("home.installed_title"));
            if installed.is_empty() {
                muted(ui, lapinux_core::i18n::t("home.no_installed"));
            } else {
                let gap = 8.0;
                let available = ui.available_width();
                let columns = if available >= 640.0 {
                    4
                } else if available >= 480.0 {
                    3
                } else if available >= 320.0 {
                    2
                } else {
                    1
                };
                let card_width =
                    ((available - gap * (columns as f32 - 1.0)) / columns as f32 - 0.5).max(140.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(gap, gap);
                    for (entry, icon) in installed.iter().take(8).zip(&installed_icons) {
                        if Self::installed_game_card(ui, &entry.name, icon.as_ref(), card_width)
                            .clicked()
                        {
                            actions.game = Some(entry.appid);
                        }
                    }
                });
                ui.add_space(8.0);
            }
            ui.vertical_centered(|ui| {
                if action_button(
                    ui,
                    lapinux_core::i18n::t("home.view_library"),
                    ActionTone::Light,
                )
                .clicked()
                {
                    actions.page = Some(Page::Library);
                }
            });
        });
    }

    /// Titre de section centré, sans barre latérale.
    fn home_section_title(ui: &mut egui::Ui, title: &str) {
        ui.add_space(1.0);
        ui.vertical_centered(|ui| {
            ui.label(egui::RichText::new(title).size(13.0).strong().color(TEXT()));
        });
        ui.add_space(9.0);
    }

    /// Carte rectangulaire d'un jeu installé : icône réelle à gauche, nom à
    /// droite. Le bouton texte d'origine ne montrait ni visuel ni surface.
    fn installed_game_card(
        ui: &mut egui::Ui,
        name: &str,
        icon: Option<&egui::TextureHandle>,
        width: f32,
    ) -> egui::Response {
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(width, 46.0), egui::Sense::click());
        let fill = if response.hovered() {
            with_alpha(TEXT(), 0.06)
        } else {
            egui::Color32::TRANSPARENT
        };
        ui.painter().rect_filled(rect, 0.0, fill);
        ui.painter().rect_stroke(
            rect,
            0.0,
            egui::Stroke::new(
                1.0_f32,
                with_alpha(TEXT(), if response.hovered() { 0.18 } else { 0.10 }),
            ),
        );
        let icon_rect =
            egui::Rect::from_min_size(rect.min + egui::vec2(8.0, 8.0), egui::vec2(30.0, 30.0));
        match icon {
            Some(texture) => {
                crate::paint_cover(ui, icon_rect, texture, egui::Rounding::same(0.0));
            }
            None => {
                ui.painter().rect_filled(icon_rect, 0.0, CARD());
                let initial = name
                    .chars()
                    .find(|character| character.is_alphanumeric())
                    .map(|character| character.to_uppercase().to_string())
                    .unwrap_or_else(|| "?".to_string());
                ui.painter().text(
                    icon_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    initial,
                    egui::FontId::proportional(13.0),
                    TEXT_DIM(),
                );
            }
        }
        let text_rect = egui::Rect::from_min_max(
            egui::pos2(icon_rect.right() + 9.0, rect.top()),
            egui::pos2(rect.right() - 8.0, rect.bottom()),
        );
        let length = (text_rect.width() / 6.2).max(4.0) as usize;
        ui.painter().with_clip_rect(text_rect).text(
            egui::pos2(text_rect.left(), rect.center().y),
            egui::Align2::LEFT_CENTER,
            lapinux_core::util::ellipsize(name, length),
            egui::FontId::proportional(12.0),
            TEXT(),
        );
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    }

    fn home_side(
        &mut self,
        ui: &mut egui::Ui,
        friends: &[HomeFriend],
        known: bool,
        actions: &mut HomeActions,
    ) {
        home_card_with_fill(ui, "", "", home_group_fill(), |ui| {
            if self.friends_loading && !known {
                muted(ui, lapinux_core::i18n::t("friends.loading_lists"));
            } else if !known {
                muted(ui, lapinux_core::i18n::t("home.friends_unavailable"));
            } else if friends.is_empty() {
                muted(ui, lapinux_core::i18n::t("home.friends_none"));
            } else {
                // Ordre strict : les personnes en jeu d'abord, puis les
                // présences en ligne, puis les absents.
                let friend_rank = |friend: &HomeFriend| {
                    if friend.in_game {
                        0
                    } else if friend.away {
                        2
                    } else {
                        1
                    }
                };
                let mut ordered: Vec<&HomeFriend> = friends.iter().collect();
                ordered.sort_by(|a, b| {
                    friend_rank(a)
                        .cmp(&friend_rank(b))
                        .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                });
                let mut previous_rank = None;
                for friend in ordered.into_iter().take(8) {
                    let rank = friend_rank(friend);
                    if previous_rank.is_some_and(|previous| previous != rank) {
                        ui.add_space(4.0);
                    }
                    previous_rank = Some(rank);
                    self.home_friend(ui, friend);
                    ui.add_space(3.0);
                }
            }
            ui.add_space(6.0);
            ui.vertical_centered(|ui| {
                if action_button(
                    ui,
                    lapinux_core::i18n::t("home.friends_open"),
                    ActionTone::Light,
                )
                .clicked()
                {
                    actions.friends = true;
                }
            });
        });
    }

    /// Carte d'un ami en ligne : photo carrée et présence à gauche, puis la
    /// marque de sa boutique et la capsule du jeu en cours à droite. La capsule
    /// vient du jeu lui-même ; le logo de la plateforme reste un élément séparé.
    fn home_friend(&mut self, ui: &mut egui::Ui, friend: &HomeFriend) {
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 46.0), egui::Sense::hover());
        let painter = ui.painter().with_clip_rect(rect);

        let avatar_size = 32.0;
        let avatar = egui::Rect::from_min_size(
            egui::pos2(rect.left() + 10.0, rect.center().y - avatar_size / 2.0),
            egui::vec2(avatar_size, avatar_size),
        );
        match self.friend_avatar(ui.ctx(), &friend.key, &friend.avatar) {
            Some(texture) => {
                egui::Image::new((texture.id(), avatar.size()))
                    .rounding(egui::Rounding::same(6.0))
                    .paint_at(ui, avatar);
            }
            None => {
                painter.rect_filled(avatar, egui::Rounding::same(6.0), CARD_ACTIVE());
                let initial = friend
                    .name
                    .chars()
                    .next()
                    .unwrap_or('?')
                    .to_uppercase()
                    .to_string();
                painter.text(
                    avatar.center(),
                    egui::Align2::CENTER_CENTER,
                    initial,
                    egui::FontId::proportional(13.0),
                    TEXT(),
                );
            }
        }
        let presence_color = if friend.in_game || (friend.online && !friend.away) {
            OK
        } else if friend.away {
            WARN
        } else {
            TEXT_DIM()
        };
        painter.circle_filled(
            avatar.right_bottom() - egui::vec2(3.0, 3.0),
            3.5,
            presence_color,
        );

        let game = if friend.game.trim().is_empty() {
            friend.state.as_str()
        } else {
            friend.game.as_str()
        };
        let capsule_size = egui::vec2(64.0, 24.0);
        let block_right = rect.right() - 10.0;
        let capsule = egui::Rect::from_min_size(
            egui::pos2(
                block_right - capsule_size.x,
                rect.center().y - capsule_size.y / 2.0,
            ),
            capsule_size,
        );
        let logo_size = 15.0;
        let logo_center_x = if friend.in_game {
            capsule.left() - 9.0 - logo_size / 2.0
        } else {
            block_right - logo_size / 2.0
        };
        let logo = egui::Rect::from_center_size(
            egui::pos2(logo_center_x, rect.center().y),
            egui::vec2(logo_size, logo_size),
        );
        crate::paint_store_mark_in(&painter, friend.store, logo, TEXT_DIM());

        if friend.in_game {
            // La fiche du profil et l'Accueil partagent le même cache : la
            // capsule est celle du jeu courant, pas une vignette construite
            // depuis un appid qui peut être périmé.
            let capsule_texture = if friend.appid != 0 {
                self.profile.game_capsule(friend.appid)
            } else {
                None
            };
            if let Some(texture) = capsule_texture {
                painter.image(
                    texture.id(),
                    capsule,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            } else {
                painter.rect_filled(capsule, egui::Rounding::same(5.0), CARD_ACTIVE());
                let label = lapinux_core::util::ellipsize(game, 10);
                painter.text(
                    capsule.center(),
                    egui::Align2::CENTER_CENTER,
                    label,
                    egui::FontId::proportional(9.0),
                    TEXT_DIM(),
                );
            }
            painter.rect_stroke(
                capsule,
                egui::Rounding::same(5.0),
                egui::Stroke::new(1.0_f32, BORDER()),
            );
        }

        let text_left = avatar.right() + 10.0;
        let text_right = logo.left() - 8.0;
        let text_width = (text_right - text_left).max(40.0);
        let name =
            lapinux_core::util::ellipsize(&friend.name, (text_width / 6.8).max(4.0) as usize);
        let state = if friend.in_game {
            if friend.game.trim().is_empty() {
                lapinux_core::i18n::t("profile.in_game").to_string()
            } else {
                format!(
                    "{} · {}",
                    lapinux_core::i18n::t("profile.in_game"),
                    friend.game
                )
            }
        } else if friend.state.trim().is_empty() {
            lapinux_core::i18n::t("profile.online").to_string()
        } else {
            friend.state.clone()
        };
        let state = lapinux_core::util::ellipsize(&state, (text_width / 6.0).max(4.0) as usize);
        painter.text(
            egui::pos2(text_left, rect.top() + 5.0),
            egui::Align2::LEFT_TOP,
            name,
            egui::FontId::proportional(12.0),
            TEXT(),
        );
        painter.text(
            egui::pos2(text_left, rect.top() + 22.0),
            egui::Align2::LEFT_TOP,
            state,
            egui::FontId::proportional(10.5),
            TEXT_DIM(),
        );
        let _ = response.on_hover_text(if friend.in_game {
            format!("{} · {}", friend.name, game)
        } else {
            friend.name.clone()
        });
    }

    /// Section Actualités : les annonces des jeux de la bibliothèque, Steam
    /// comme Epic Games, GOG et itch.io. Installés et non installés alternent ;
    /// la liste n'est reconstruite que lorsque la bibliothèque change.
    pub(super) fn news_page(&mut self, ui: &mut egui::Ui) {
        // Les jeux des boutiques ne sont pas forcément relus depuis la page
        // Bibliothèque : sans cette demande, seuls les jeux Steam entrent
        // dans les actualités.
        self.stores.ensure(&self.client);
        self.stores.ensure_games(&self.client);
        if let Some(message) = self.stores.poll(ui.ctx()) {
            self.notify(message);
            self.touch_library();
        }
        let entries = self.library_entries_all();
        self.ensure_news_games(&entries);
        if let Some(article) = self.news_feed.page(
            ui,
            &self.news_games,
            &self.config.general.lang,
            &self.client,
        ) {
            self.open_news_reader(article.items, article.at);
        }
        if let Some((url, title)) = self.news_feed.take_webview() {
            self.webview.open(&self.paths, &url, &title);
            self.webview.present();
        }
    }

    /// Reconstruit la liste du flux d'actualités quand la bibliothèque change.
    fn ensure_news_games(&mut self, entries: &Rc<Vec<LibraryEntry>>) {
        let unchanged = self
            .news_entries
            .as_ref()
            .map_or(false, |cached| Rc::ptr_eq(cached, entries));
        if !unchanged {
            self.news_games = Self::build_news_games(entries, &self.paths);
            self.news_entries = Some(Rc::clone(entries));
        }
    }

    /// Liste des jeux du flux d'actualités : installés et non installés
    /// alternés, sans doublon, dans l'ordre déjà trié de la bibliothèque.
    fn build_news_games(entries: &[LibraryEntry], paths: &LapinuxPaths) -> Vec<news::GameSource> {
        let mut installed = Vec::new();
        let mut missing = Vec::new();
        let mut seen = HashSet::new();
        for entry in entries {
            if entry.appid == 0
                || (entry.store.is_none() && !entry.steam && !entry.on_disk)
                || entry.name.trim().is_empty()
            {
                continue;
            }
            // Un appid n'est unique que dans sa boutique : Steam et Epic
            // peuvent publier le même numéro pour deux jeux différents.
            if !seen.insert((entry.appid, entry.store)) {
                continue;
            }
            let game = news::GameSource {
                appid: entry.appid,
                name: entry.name.clone(),
                store: entry.store,
                icon: crate::captures::icon_file(paths, entry.appid),
            };
            if entry.installed {
                installed.push(game);
            } else {
                missing.push(game);
            }
        }
        // La bibliothèque arrive dans l'ordre de tri choisi : chaque moitié
        // garde cet ordre, l'alternance remplit la première page des deux.
        let mut games = Vec::with_capacity(installed.len() + missing.len());
        let mut installed = installed.into_iter();
        let mut missing = missing.into_iter();
        loop {
            let mut pushed = false;
            if let Some(game) = installed.next() {
                games.push(game);
                pushed = true;
            }
            if let Some(game) = missing.next() {
                games.push(game);
                pushed = true;
            }
            if !pushed {
                break;
            }
        }
        games
    }
}

#[derive(Default)]
struct HomeActions {
    game: Option<u32>,
    page: Option<Page>,
    update: Option<news::HomeUpdate>,
    friends: bool,
}

struct HomeFriend {
    key: String,
    store: lapinux_core::stores::Store,
    appid: u32,
    name: String,
    state: String,
    game: String,
    avatar: String,
    online: bool,
    in_game: bool,
    away: bool,
}

/// Une boutique en erreur ne permet pas de conclure à zéro présence.
fn home_friends_status(
    social: Option<&SteamSocial>,
    boards: &[StoreFriends],
    stores_available: bool,
) -> (bool, bool) {
    let steam_live = social.is_some_and(|social| social.live());
    let store_known = stores_available
        && boards.iter().any(|board| {
            board.connected
                && (board.note.is_empty()
                    || board
                        .friends
                        .iter()
                        .any(|friend| !friend.status.is_empty() || !friend.game.trim().is_empty()))
        });
    let partial = !steam_live
        || !stores_available
        || boards
            .iter()
            .any(|board| board.connected && !board.note.is_empty());
    (steam_live || store_known, partial)
}

/// Même critère que le panneau Amis : en ligne ou en jeu, sans compter le compte courant.
fn home_friends(
    social: Option<&SteamSocial>,
    boards: &[StoreFriends],
    stores_available: bool,
) -> Vec<HomeFriend> {
    let mut friends = Vec::new();
    if let Some(social) = social.filter(|social| social.live()) {
        for friend in &social.friends {
            if friend.steam_id != 0
                && friend.steam_id != social.steam_id
                && (friend.presence.online() || friend.in_game())
            {
                friends.push(HomeFriend {
                    key: format!("steam:{}", friend.steam_id),
                    store: lapinux_core::stores::Store::Steam,
                    appid: friend.game_appid,
                    name: friend.name.clone(),
                    state: friend.state_line(),
                    game: friend.game.clone(),
                    avatar: friend.avatar.clone(),
                    online: friend.presence.online(),
                    in_game: friend.in_game(),
                    away: matches!(
                        friend.presence,
                        lapinux_core::social::SteamPresence::Away
                            | lapinux_core::social::SteamPresence::Snooze
                    ),
                });
            }
        }
    }
    if stores_available {
        for board in boards.iter().filter(|board| board.connected) {
            for friend in &board.friends {
                let in_game = friend.status == "in-game" || !friend.game.trim().is_empty();
                if friend.online() || in_game {
                    friends.push(HomeFriend {
                        key: format!("{}:{}", friend.store.key(), friend.id),
                        store: board.store,
                        appid: 0,
                        name: if friend.alias.trim().is_empty() {
                            friend.name.clone()
                        } else {
                            friend.alias.clone()
                        },
                        state: friend.state_line(),
                        game: friend.game.clone(),
                        avatar: friend.avatar.clone(),
                        online: friend.online(),
                        in_game,
                        away: matches!(friend.status.as_str(), "away" | "snooze"),
                    });
                }
            }
        }
    }
    friends.sort_by(|a, b| {
        b.in_game
            .cmp(&a.in_game)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    friends
}

fn home_stat(ui: &mut egui::Ui, value: &str, label: &str, page: Option<Page>) -> bool {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 84.0), egui::Sense::click());
    let painter = ui.painter();
    painter.rect(
        rect,
        egui::Rounding::same(0.0),
        home_group_fill(),
        egui::Stroke::NONE,
    );
    let icon = egui::Rect::from_min_size(rect.min + egui::vec2(15.0, 20.0), egui::vec2(42.0, 42.0));
    if let Some(page) = page {
        section_icons::page(painter, page, icon.center(), 21.0, TEXT());
    } else {
        section_icons::friends(painter, icon.center(), 21.0, TEXT(), 0.0);
    }
    painter.text(
        rect.min + egui::vec2(70.0, 16.0),
        egui::Align2::LEFT_TOP,
        value,
        egui::FontId::proportional(24.0),
        TEXT(),
    );
    let text_rect = egui::Rect::from_min_max(
        rect.min + egui::vec2(70.0, 49.0),
        rect.max - egui::vec2(10.0, 4.0),
    );
    painter.with_clip_rect(text_rect).text(
        text_rect.min,
        egui::Align2::LEFT_TOP,
        label,
        egui::FontId::proportional(12.0),
        TEXT_DIM(),
    );
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.on_hover_text(label).clicked()
}

/// Fond des grands blocs de l'Accueil : un seul rectangle noir translucide
/// autour de toute la section, pas une tuile par élément.
fn home_group_fill() -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(0, 0, 0, 160)
}

fn home_card(ui: &mut egui::Ui, title: &str, subtitle: &str, add: impl FnOnce(&mut egui::Ui)) {
    home_card_with_fill(ui, title, subtitle, home_group_fill(), add);
}

fn home_card_with_fill(
    ui: &mut egui::Ui,
    title: &str,
    subtitle: &str,
    fill: egui::Color32,
    add: impl FnOnce(&mut egui::Ui),
) {
    egui::Frame::none()
        .fill(fill)
        // Les grands blocs de l'Accueil gardent des angles droits : le fond
        // noir translucide marque la section, sans arrondi de carte ni trait
        // de contour autour du rectangle.
        .rounding(egui::Rounding::same(0.0))
        .inner_margin(20.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if !title.is_empty() {
                ui.horizontal_centered(|ui| {
                    let (bar, _) =
                        ui.allocate_exact_size(egui::vec2(3.0, 30.0), egui::Sense::hover());
                    ui.painter().rect_filled(bar, 1.5, ACCENT());
                    ui.add_space(5.0);
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new(title).size(16.0).strong().color(TEXT()));
                        if !subtitle.is_empty() {
                            ui.label(egui::RichText::new(subtitle).size(11.5).color(TEXT_DIM()));
                        }
                    });
                });
                ui.add_space(16.0);
            }
            add(ui);
        });
    ui.add_space(14.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accueil_amis_compte_les_presences_connues_sans_le_compte_courant() {
        use lapinux_core::social::{SteamFriend, StoreFriend};
        use lapinux_core::stores::Store;

        let social = SteamSocial {
            state: "live".into(),
            steam_id: 1,
            friends: vec![
                SteamFriend {
                    steam_id: 1,
                    name: "Moi".into(),
                    presence: SteamPresence::Online,
                    ..Default::default()
                },
                SteamFriend {
                    steam_id: 2,
                    name: "Camille".into(),
                    presence: SteamPresence::Online,
                    ..Default::default()
                },
                SteamFriend {
                    steam_id: 3,
                    name: "Absent".into(),
                    ..Default::default()
                },
                SteamFriend {
                    steam_id: 4,
                    name: "Joueur".into(),
                    game: "Jeu".into(),
                    game_appid: 620,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let boards = vec![StoreFriends {
            store: Store::Epic,
            connected: true,
            friends: vec![
                StoreFriend {
                    store: Store::Epic,
                    id: "1".into(),
                    name: "Epic".into(),
                    alias: "Alias".into(),
                    status: "online".into(),
                    ..Default::default()
                },
                StoreFriend {
                    store: Store::Epic,
                    id: "2".into(),
                    name: "Inconnu".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        }];
        let friends = home_friends(Some(&social), &boards, true);
        assert_eq!(friends.len(), 3);
        assert_eq!(friends[0].name, "Joueur");
        assert_eq!(friends[0].store, Store::Steam);
        assert_eq!(friends[0].appid, 620);
        assert_eq!(friends[0].game, "Jeu");
        assert!(friends.iter().any(|friend| friend.name == "Alias"));
        assert_eq!(home_friends(Some(&social), &boards, false).len(), 2);
        assert_eq!(home_friends_status(None, &[], false), (false, true));
        let failed = vec![StoreFriends {
            store: Store::Epic,
            connected: true,
            note: "échec".into(),
            ..Default::default()
        }];
        assert_eq!(home_friends_status(None, &failed, true), (false, true));
        assert_eq!(
            home_friends_status(Some(&social), &boards, true),
            (true, false)
        );
    }
}
