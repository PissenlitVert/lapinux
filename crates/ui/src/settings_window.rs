//! Fenêtre des réglages et configuration commune.

use super::*;

impl LapinuxApp {
    /// Fenêtre centrée : un seul endroit pour les réglages, la connexion Steam
    /// et les journaux, sans quitter la bibliothèque.
    pub(super) fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.settings_open {
            return;
        }
        let mut close_by_tab = false;
        let close = fixed_panel_with_scroll(
            ctx,
            "settings-modal",
            lapinux_core::i18n::t("settings.title"),
            "",
            egui::vec2(960.0, 680.0),
            false,
            false,
            64,
            |ui| {
                self.settings_window_body(ui, &mut close_by_tab);
            },
        );
        if close || close_by_tab {
            self.close_settings();
        }
    }

    /// Même châssis que la fenêtre d'un jeu : identité à gauche, onglets
    /// plats, titre de page à droite et croix dans le coin supérieur droit.
    fn settings_window_body(&mut self, ui: &mut egui::Ui, close: &mut bool) {
        let body = ui.max_rect();
        let rail_width = 172.0;
        let rail = egui::Rect::from_min_max(
            body.min,
            egui::pos2(body.left() + rail_width, body.bottom()),
        );
        ui.painter().line_segment(
            [
                egui::pos2(rail.right(), rail.top()),
                egui::pos2(rail.right(), rail.bottom()),
            ],
            egui::Stroke::new(1.0_f32, BORDER()),
        );

        let height = body.height();
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(rail_width, height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(rail_width);
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        ui.add_space(10.0);
                        let (icon_rect, _) =
                            ui.allocate_exact_size(egui::vec2(40.0, 40.0), egui::Sense::hover());
                        ui.painter()
                            .rect_filled(icon_rect, egui::Rounding::same(9.0), CARD());
                        // L'identité du rail est celle de l'application : son
                        // logo, puis son nom, au lieu d'un engrenage générique.
                        match &self.logo {
                            Some(logo) => logo.draw(ui.painter(), icon_rect.center(), 27.0, 1.0),
                            None => section_icons::gear(
                                ui.painter(),
                                icon_rect.center(),
                                18.0,
                                ACCENT(),
                                0.0,
                            ),
                        }
                        ui.add_space(9.0);
                        let name_width = (ui.available_width() - 8.0).max(0.0);
                        ui.allocate_ui_with_layout(
                            egui::vec2(name_width, 40.0),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                ui.add_space(9.0);
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(lapinux_core::APP_NAME)
                                            .size(13.0)
                                            .strong()
                                            .color(TEXT()),
                                    )
                                    .truncate(),
                                );
                                ui.add_space(1.0);
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(lapinux_core::i18n::t(
                                            "settings.title",
                                        ))
                                        .size(9.5)
                                        .color(TEXT_DIM()),
                                    )
                                    .truncate(),
                                );
                            },
                        );
                    });
                    ui.add_space(9.0);
                    let (separator, _) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), 1.0),
                        egui::Sense::hover(),
                    );
                    ui.painter().hline(
                        separator.x_range(),
                        separator.center().y,
                        egui::Stroke::new(1.0_f32, with_alpha(BORDER(), 0.9)),
                    );
                    ui.add_space(8.0);

                    let tabs_height = (ui.available_height() - 30.0).max(0.0);
                    ui.allocate_ui_with_layout(
                        egui::vec2(ui.available_width(), tabs_height),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_height(tabs_height);
                            egui::ScrollArea::vertical()
                                .id_salt("settings-rail")
                                .scroll_bar_visibility(
                                    egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded,
                                )
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    ui.set_width((ui.available_width() - 8.0).max(120.0));
                                    ui.spacing_mut().item_spacing = egui::vec2(0.0, 2.0);
                                    let mut current_group: Option<usize> = None;
                                    for (tab, _label) in settings_tabs_list() {
                                        let group = tab.group();
                                        if current_group != Some(group) {
                                            current_group = Some(group);
                                            if group > 0 {
                                                ui.add_space(7.0);
                                            }
                                            let (header, response) = ui.allocate_exact_size(
                                                egui::vec2(ui.available_width(), 24.0),
                                                egui::Sense::click(),
                                            );
                                            let response = response
                                                .on_hover_cursor(egui::CursorIcon::PointingHand);
                                            if response.hovered() {
                                                ui.painter().rect_filled(
                                                    header,
                                                    egui::Rounding::same(6.0),
                                                    with_alpha(CARD_HOVER(), 0.45),
                                                );
                                            }
                                            let collapsed = self.settings_collapsed[group];
                                            let label = match group {
                                                0 => lapinux_core::i18n::t(
                                                    "settings.group.essential",
                                                ),
                                                1 => lapinux_core::i18n::t("settings.group.data"),
                                                _ => lapinux_core::i18n::t("settings.group.system"),
                                            };
                                            ui.painter().text(
                                                egui::pos2(header.left() + 10.0, header.center().y),
                                                egui::Align2::LEFT_CENTER,
                                                label,
                                                egui::FontId::proportional(9.0),
                                                TEXT_DIM(),
                                            );
                                            ui.painter().text(
                                                egui::pos2(
                                                    header.right() - 12.0,
                                                    header.center().y,
                                                ),
                                                egui::Align2::CENTER_CENTER,
                                                if collapsed { "▸" } else { "▾" },
                                                egui::FontId::proportional(9.0),
                                                TEXT_DIM(),
                                            );
                                            let hint = if collapsed {
                                                lapinux_core::i18n::t(
                                                    "settings.window.show_category",
                                                )
                                            } else {
                                                lapinux_core::i18n::t(
                                                    "settings.window.hide_category",
                                                )
                                            };
                                            if response.on_hover_text(hint).clicked() {
                                                self.settings_collapsed[group] = !collapsed;
                                                self.settings_categories_changed();
                                            }
                                            ui.add_space(2.0);
                                        }
                                        if self.settings_collapsed[group] {
                                            continue;
                                        }
                                        let selected = self.settings_tab == tab;
                                        let (row, response) = ui.allocate_exact_size(
                                            egui::vec2(ui.available_width(), 34.0),
                                            egui::Sense::click(),
                                        );
                                        let response = response
                                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                                        if selected {
                                            ui.painter().rect_filled(
                                                egui::Rect::from_min_size(
                                                    egui::pos2(row.left(), row.center().y - 9.0),
                                                    egui::vec2(3.0, 18.0),
                                                ),
                                                egui::Rounding::same(2.0),
                                                ACCENT(),
                                            );
                                        } else if response.hovered() {
                                            ui.painter().rect_filled(
                                                row,
                                                egui::Rounding::same(9.0),
                                                with_alpha(CARD_HOVER(), 0.55),
                                            );
                                        }
                                        let ink = if selected || response.hovered() {
                                            TEXT()
                                        } else {
                                            TEXT_DIM()
                                        };
                                        let progress = icon_hover_progress(
                                            ui,
                                            ui.id().with((
                                                "settings-tab-icon-motion",
                                                tab.group(),
                                                tab.group_index(),
                                            )),
                                            response.hovered() || response.has_focus(),
                                        );
                                        section_icons::settings_tab_animated(
                                            ui.painter(),
                                            tab,
                                            egui::pos2(row.left() + 19.0, row.center().y),
                                            18.0,
                                            ink,
                                            progress,
                                        );
                                        ui.painter().text(
                                            egui::pos2(row.left() + 35.0, row.center().y),
                                            egui::Align2::LEFT_CENTER,
                                            tab.title(),
                                            egui::FontId::proportional(12.0),
                                            ink,
                                        );
                                        if response.clicked() && !selected {
                                            self.settings_tab = tab;
                                            self.settings_scroll_reset = true;
                                            if tab == SettingsTab::Storage {
                                                self.storage.dirty = true;
                                            }
                                            if tab != SettingsTab::Connection {
                                                self.steam_password.clear();
                                                self.steam_token.clear();
                                                self.steam_guard.clear();
                                            }
                                        }
                                        ui.add_space(2.0);
                                    }
                                });
                        },
                    );
                    ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                        ui.add_space(8.0);
                        muted(ui, lapinux_core::i18n::t("settings.autosave"));
                    });
                },
            );
            ui.add_space(20.0);
            // La largeur restante est calculée après le rail et l'espacement :
            // la formule globale dépassait de quelques pixels et poussait la
            // barre de défilement sous le bord droit du modal.
            let max_content_width = (body.width() - rail_width - 32.0).max(280.0);
            let content_width = (ui.available_width() - 10.0)
                .min(max_content_width)
                .max(280.0);
            ui.allocate_ui_with_layout(
                egui::vec2(content_width, height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new(self.settings_tab.title())
                            .size(18.0)
                            .strong()
                            .color(TEXT()),
                    );
                    ui.add_space(2.0);
                    muted(ui, self.settings_tab.subtitle());
                    ui.add_space(8.0);
                    let content_width = (ui.available_width() - 10.0).max(280.0);
                    let reset_scroll = std::mem::take(&mut self.settings_scroll_reset);
                    // Sans hauteur maximale explicite, la zone de défilement
                    // peut se croire non bornée : la carte HUD s'échappe alors
                    // sous le bas du modal au lieu de défiler à l'intérieur.
                    // On calcule la place restante depuis le bas réellement
                    // visible (le clip du panneau), et non depuis
                    // `available_height()` qui peut inclure le bandeau déjà
                    // consommé par le titre.
                    let clip_bottom = ui.clip_rect().bottom();
                    let content_height = if clip_bottom.is_finite() {
                        (clip_bottom - ui.cursor().top() - 8.0).min(560.0).max(80.0)
                    } else {
                        560.0
                    };
                    let mut scroll = egui::ScrollArea::vertical()
                        .id_salt(("settings-content", self.settings_tab as u8))
                        .scroll_bar_visibility(
                            egui::scroll_area::ScrollBarVisibility::AlwaysVisible,
                        )
                        .max_height(content_height)
                        .max_width(content_width)
                        .auto_shrink([false, false]);
                    if reset_scroll {
                        scroll = scroll.vertical_scroll_offset(0.0);
                    }
                    scroll.show(ui, |ui| {
                        egui::Frame::none()
                            .inner_margin(egui::Margin::symmetric(12.0, 12.0))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width().min(content_width));
                                match self.settings_tab {
                                    SettingsTab::General => self.settings_general_tab(ui),
                                    SettingsTab::Notifications => {
                                        self.settings_notifications_tab(ui)
                                    }
                                    SettingsTab::InGame => self.settings_in_game_tab(ui),
                                    SettingsTab::Library => {
                                        self.settings_library_tab(ui);
                                    }
                                    SettingsTab::Storage => self.settings_storage_tab(ui),
                                    SettingsTab::Appearance => {
                                        self.settings_appearance_tab(ui);
                                    }
                                    SettingsTab::Connection => {
                                        *close = self.accounts_tab(ui) || *close;
                                    }
                                    SettingsTab::Logs => self.logs_tab(ui),
                                    SettingsTab::Saves => self.settings_saves_tab(ui),
                                    SettingsTab::Protons => self.protons_page(ui),
                                    SettingsTab::Graphics => self.settings_graphics_tab(ui),
                                    SettingsTab::System => self.system_page(ui),
                                    SettingsTab::Doctor => self.doctor_page(ui),
                                    SettingsTab::Updates => self.settings_updates_tab(ui),
                                    SettingsTab::Application => self.settings_application_tab(ui),
                                    SettingsTab::Captures => self.settings_captures_tab(ui),
                                }
                                // Une petite marge basse pour que le dernier
                                // contrôle puisse remonter au-dessus du bord.
                                ui.add_space(12.0);
                            });
                    });
                },
            );
        });
    }

    /// Recopie le tri, les sources et le filtre d'installation dans la
    /// configuration : la bibliothèque rouvre comme elle a été laissée.
    pub(super) fn sync_library_view(&mut self) {
        let sort = self.library_sort.to_config();
        let sources = self.sources.to_config();
        let filter = self.catalog_filter.to_config();
        if self.config.library.sort == sort
            && self.config.library.sources == sources
            && self.config.library.filter == filter
            && self.config.library.sort_descending == self.library_descending
        {
            return;
        }
        self.config.library.sort = sort;
        self.config.library.sources = sources;
        self.config.library.filter = filter;
        // Le sens fait partie de la vue : sans l'enregistrer, la bibliothèque
        // rouvrait toujours du plus ancien au plus récent.
        self.config.library.sort_descending = self.library_descending;
        self.config.library.sort_direction_chosen = true;
        // Un brouillon de réglages ouvert garde les mêmes valeurs : sans cela
        // son enregistrement automatique écraserait la vue de bibliothèque.
        if let Some(draft) = self.settings_draft.as_mut() {
            draft.library.sort = sort;
            draft.library.sources = sources;
            draft.library.filter = filter;
            draft.library.sort_descending = self.library_descending;
            draft.library.sort_direction_chosen = true;
        }
        self.config_dirty_at = Some(Instant::now());
    }

    /// Marque les collections comme modifiées et garde le brouillon des
    /// réglages aligné, pour qu'un enregistrement de réglages ne les écrase pas.
    pub(super) fn collections_changed(&mut self) {
        if let Some(draft) = self.settings_draft.as_mut() {
            draft.collections = self.config.collections.clone();
        }
        self.config_dirty_at = Some(Instant::now());
    }

    /// Les catégories repliées de la barre des réglages font partie de la
    /// configuration : elles reviennent au prochain lancement.
    pub(super) fn settings_categories_changed(&mut self) {
        self.config.general.settings_collapsed = self.settings_collapsed;
        if let Some(draft) = self.settings_draft.as_mut() {
            draft.general.settings_collapsed = self.settings_collapsed;
        }
        self.config_dirty_at = Some(Instant::now());
    }

    /// Enregistre la configuration (vue de bibliothèque, collections) après
    /// une courte pause : plusieurs clics ne déclenchent qu'une écriture.
    pub(super) fn autosave_config(&mut self, ctx: &egui::Context) {
        let Some(since) = self.config_dirty_at else {
            return;
        };
        if since.elapsed() < Duration::from_millis(350) {
            ctx.request_repaint_after(Duration::from_millis(150));
            return;
        }
        self.config_dirty_at = None;
        let client = self.client.detached();
        let config = self.config.clone();
        std::thread::spawn(move || {
            let _ = client.request(&Req::SetConfig { config });
        });
    }

    /// Envoie la configuration au daemon et met à jour les caches locaux.
    /// Enregistre les réglages dès qu'ils changent, sans bouton : le brouillon
    /// part après une courte pause, pour qu'un champ en cours de frappe ne
    /// déclenche pas une écriture par caractère.
    pub(super) fn autosave_settings(&mut self, ctx: &egui::Context) {
        let Some(draft) = self.settings_draft.clone() else {
            self.settings_dirty_at = None;
            return;
        };
        if draft == self.config {
            self.settings_dirty_at = None;
            return;
        }
        let now = Instant::now();
        let since = *self.settings_dirty_at.get_or_insert(now);
        if now.duration_since(since) >= Duration::from_millis(500) {
            self.settings_dirty_at = None;
            self.settings_autosave = true;
            let saved = self.apply_config(draft);
            self.settings_autosave = false;
            if !saved {
                // Le daemon a refusé : on garde la main sur la configuration
                // réellement active au lieu de réessayer sans fin.
                let current = self.config.clone();
                self.settings_draft = Some(current);
            }
        } else {
            // Un champ tapé au clavier doit partir même sans autre interaction.
            ctx.request_repaint_after(Duration::from_millis(200));
        }
    }

    pub(super) fn apply_config(&mut self, config: Config) -> bool {
        match self.client.request(&Req::SetConfig {
            config: config.clone(),
        }) {
            Ok(envelope) if envelope.ok => {
                // Une langue de jeu changée change les poids annoncés : la fiche
                // relira ses détails Steam au lieu de garder les anciens.
                if self.config.general.game_language != config.general.game_language {
                    self.gameinfo.forget();
                }
                lapinux_core::i18n::set(&config.general.lang);
                if self.config.notifications != config.notifications {
                    self.friend_events_unread = 0;
                }
                // Une durée de conservation plus courte doit retirer les
                // anciennes cartes sans attendre un redémarrage.
                if self.config.general.activity_retention_days
                    != config.general.activity_retention_days
                {
                    self.play_history.clear();
                    self.play_history_rx = None;
                    self.play_history_asked = false;
                }
                self.config = config.clone();
                // Retirer l'onglet de la page courante ne doit pas laisser
                // l'application sur un écran sans entrée dans la barre.
                let next_page = page_after_navigation(self.page, &self.config.general);
                if next_page != self.page {
                    self.page = next_page;
                    self.collection_detail_open = false;
                }
                self.prune_friend_events_history();
                self.theme_dirty = true;
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
                self.settings_draft = Some(config);
                // Un enregistrement automatique ne dit rien : la pastille
                // clignoterait à chaque cran de curseur.
                if !self.settings_autosave {
                    self.notify(lapinux_core::i18n::t("settings.window.saved"));
                }
                true
            }
            Ok(envelope) => {
                self.notify(envelope.error.unwrap_or_default());
                false
            }
            Err(err) => {
                self.notify(format!("{err}"));
                false
            }
        }
    }

    /// Onglet Général : tout ce qui n'a pas besoin de la connexion Steam.
    /// Libellé de « Automatique » : le Proton qu'il prendrait, nommé, pour que
    /// les listes disent toujours ce qui sert vraiment.
    pub(super) fn automatic_proton_label(&self) -> String {
        match self
            .steam_status
            .as_ref()
            .and_then(|status| status.default_proton.clone())
        {
            Some(name) => lapinux_core::i18n::t_args("protons.automatic", &[("name", &name)]),
            None => lapinux_core::i18n::t("protons.automatic_none").to_string(),
        }
    }

    /// Langue que le moteur prend quand on ne choisit rien : celle du client
    /// Steam, sinon celle du bureau.
    pub(super) fn detected_language(&self) -> Option<String> {
        self.steam_status
            .as_ref()
            .and_then(|status| status.default_language.clone())
    }
}
