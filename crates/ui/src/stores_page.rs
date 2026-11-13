//! Pages des boutiques.

use super::*;

impl LapinuxApp {
    /// Page **Boutiques** : la liste des quatre boutiques à gauche, et à
    /// droite le **site** de celle qui est retenue — son identité, l'état du
    /// compte branché dessus et le bouton qui l'ouvre dans le navigateur.
    ///
    /// Les jeux possédés restent dans la Bibliothèque : cette page ne les
    /// répète pas, elle mène à la boutique elle-même.
    pub(super) fn boutiques_page(&mut self, ui: &mut egui::Ui) {
        self.stores.ensure(&self.client);
        if let Some(message) = self.stores.poll(ui.ctx()) {
            self.notify(message);
        }
        let steam_connected = self
            .steam_status
            .as_ref()
            .is_some_and(|status| status.connected);
        let selected = self
            .store_filter
            .unwrap_or(lapinux_core::stores::Store::Steam);

        // Deux colonnes qui prennent toute la hauteur : la liste à gauche, le
        // site à droite — c'est la page qui défile, pas la fenêtre.
        const SIDE: f32 = 214.0;
        let gap = ui.spacing().item_spacing.x;
        let height = ui.available_height().max(260.0);
        if ui.available_width() - SIDE - gap >= 380.0 {
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(SIDE, height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(SIDE);
                        self.store_rail(ui, SIDE, selected, steam_connected);
                    },
                );
                let page = ui.available_width().max(320.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(page, height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(page);
                        if self.free_open {
                            let list_height = ui.available_height().max(160.0);
                            egui::ScrollArea::vertical()
                                .id_salt("free-games-scroll")
                                .max_height(list_height)
                                .auto_shrink([false, false])
                                .scroll_bar_visibility(
                                    egui::scroll_area::ScrollBarVisibility::AlwaysVisible,
                                )
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    self.free_games_panel(ui);
                                });
                        } else {
                            let connected = self.store_connected(selected, steam_connected);
                            self.store_site(ui, selected, connected);
                        }
                    },
                );
            });
        } else {
            // Étroite : la liste au-dessus, le site en dessous, sans cadre.
            self.store_rail(
                ui,
                ui.available_width().min(320.0),
                selected,
                steam_connected,
            );
            ui.add_space(8.0);
            let page = ui.available_width().max(320.0);
            ui.allocate_ui_with_layout(
                egui::vec2(page, ui.available_height().max(260.0)),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(page);
                    if self.free_open {
                        let list_height = ui.available_height().max(160.0);
                        egui::ScrollArea::vertical()
                            .id_salt("free-games-scroll-narrow")
                            .max_height(list_height)
                            .auto_shrink([false, false])
                            .scroll_bar_visibility(
                                egui::scroll_area::ScrollBarVisibility::AlwaysVisible,
                            )
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                self.free_games_panel(ui);
                            });
                    } else {
                        let connected = self.store_connected(selected, steam_connected);
                        self.store_site(ui, selected, connected);
                    }
                },
            );
        }
    }

    /// Section **Jeux gratuits** : la liste, le panier Epic et la fenêtre de
    /// boutique qui n'ouvre que l'offre cliquée.
    fn free_games_panel(&mut self, ui: &mut egui::Ui) {
        // L'état d'avant la mise à jour : c'est lui qui révèle une fermeture,
        // puisque `status` referme la session dans la même image.
        let was_open = self.webview.is_open();
        // La fenêtre de boutique se ferme de son côté : son état est relu ici,
        // retour compris.
        self.webview.status(ui.ctx());
        let target = self.free.page(ui, &self.client);
        if let Some((url, title)) = target {
            self.free_watch = true;
            self.webview.open(&self.paths, &url, &title);
        }
        // Envoi au panier : toutes les offres Epic entrent dans le panier du
        // site en tâche de fond, avec la session déjà enregistrée.
        if let Some(products) = self.free.take_cart_request() {
            self.free_watch = false;
            self.free.start_cart(&self.paths, products);
        }
        // Le checkout gratuit Epic porte toutes les offres dans son adresse.
        // Il part dans le navigateur du bureau : Cloudflare bloque sa
        // vérification dans la fenêtre WebKit intégrée et la page reste noire.
        if let Some(url) = self.free.take_cart_page() {
            crate::open_url(&url);
            self.free.checkout_opened_external();
        }
        if self.free_watch && was_open && !self.webview.is_open() {
            // La page s'est fermée : les droits obtenus sont relus.
            self.free_watch = false;
            self.free.refresh(&self.client);
        }
    }

    /// Le compte d'une boutique est-il branché ? Steam répond par sa session,
    /// les autres par la vue du daemon.
    fn store_connected(&self, store: lapinux_core::stores::Store, steam_connected: bool) -> bool {
        if store == lapinux_core::stores::Store::Steam {
            return steam_connected;
        }
        self.stores
            .view
            .accounts
            .iter()
            .any(|account| account.store == store && account.connected)
    }

    /// Liste de gauche : une ligne par boutique — son logo, son nom et l'état
    /// de son compte. La ligne cliquée devient le site affiché à droite.
    fn store_rail(
        &mut self,
        ui: &mut egui::Ui,
        width: f32,
        selected: lapinux_core::stores::Store,
        steam_connected: bool,
    ) {
        ui.set_width(width);
        ui.add_space(5.0);
        ui.horizontal(|ui| {
            ui.add_space(9.0);
            ui.label(
                egui::RichText::new(lapinux_core::i18n::t("page.stores"))
                    .size(11.5)
                    .strong()
                    .color(TEXT_DIM()),
            );
        });
        ui.add_space(8.0);
        for store in lapinux_core::stores::Store::ALL {
            let active = store == selected;
            let connected = self.store_connected(store, steam_connected);
            const HEIGHT: f32 = 52.0;
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(width, HEIGHT), egui::Sense::click());
            let painter = ui.painter();
            if active || response.hovered() {
                painter.rect_filled(
                    rect,
                    egui::Rounding::same(8.0),
                    if active { CARD_ACTIVE() } else { CARD_HOVER() },
                );
            }
            let tile = egui::Rect::from_center_size(
                egui::pos2(rect.left() + 9.0 + 18.0, rect.center().y),
                egui::vec2(36.0, 36.0),
            );
            painter.rect_filled(
                tile,
                egui::Rounding::same(7.0),
                egui::Color32::from_black_alpha(70),
            );
            paint_store_mark_in(painter, store, tile.shrink(6.0), TEXT());
            let text_left = tile.right() + 10.0;
            painter.text(
                egui::pos2(text_left, rect.center().y - 8.0),
                egui::Align2::LEFT_CENTER,
                store.label(),
                egui::FontId::proportional(12.5),
                if active { TEXT() } else { TEXT_DIM() },
            );
            painter.text(
                egui::pos2(text_left, rect.center().y + 9.0),
                egui::Align2::LEFT_CENTER,
                if connected {
                    lapinux_core::i18n::t("stores.account_connected")
                } else {
                    lapinux_core::i18n::t("stores.no_account")
                },
                egui::FontId::proportional(10.5),
                if connected { OK } else { TEXT_DIM() },
            );
            // Le point d'état, au bord droit de la ligne.
            let dot = egui::pos2(rect.right() - 14.0, rect.center().y);
            painter.circle_filled(dot, 3.5, if connected { OK } else { DANGER });
            if response.clicked() {
                self.store_filter = Some(store);
                self.free_open = false;
            }
            ui.add_space(2.0);
        }
        // Les offres de la semaine ne sont pas une boutique : elles ont leur
        // propre entrée, sous les quatre enseignes.
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_space(9.0);
            ui.label(
                egui::RichText::new(lapinux_core::i18n::t("ui.offers"))
                    .size(11.5)
                    .strong()
                    .color(TEXT_DIM()),
            );
        });
        ui.add_space(8.0);
        let active = self.free_open;
        const FREE_HEIGHT: f32 = 52.0;
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(width, FREE_HEIGHT), egui::Sense::click());
        if active || response.hovered() {
            ui.painter().rect_filled(
                rect,
                egui::Rounding::same(8.0),
                if active { CARD_ACTIVE() } else { CARD_HOVER() },
            );
        }
        let tile = egui::Rect::from_center_size(
            egui::pos2(rect.left() + 9.0 + 18.0, rect.center().y),
            egui::vec2(36.0, 36.0),
        );
        ui.painter().rect_filled(
            tile,
            egui::Rounding::same(7.0),
            egui::Color32::from_black_alpha(70),
        );
        Self::paint_gift_glyph(ui.painter(), tile.shrink(8.0), TEXT());
        let text_left = tile.right() + 10.0;
        ui.painter().text(
            egui::pos2(text_left, rect.center().y - 8.0),
            egui::Align2::LEFT_CENTER,
            lapinux_core::i18n::t("stores.free_games"),
            egui::FontId::proportional(12.5),
            if active { TEXT() } else { TEXT_DIM() },
        );
        let offers = self.free.current_count();
        ui.painter().text(
            egui::pos2(text_left, rect.center().y + 9.0),
            egui::Align2::LEFT_CENTER,
            if offers == 0 {
                lapinux_core::i18n::t("freegames.rail_empty").to_string()
            } else {
                lapinux_core::i18n::t_args(
                    "freegames.rail_count",
                    &[("count", &offers.to_string())],
                )
            },
            egui::FontId::proportional(10.5),
            if offers > 0 { OK } else { TEXT_DIM() },
        );
        if response.clicked() {
            self.free_open = true;
        }
    }

    /// Pictogramme d'un cadeau, au trait : la boîte, son couvercle et le
    /// ruban, à la même graisse que les autres icônes de la page.
    fn paint_gift_glyph(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
        let stroke = egui::Stroke::new(1.6_f32, color);
        let body = egui::Rect::from_min_max(
            egui::pos2(rect.left(), rect.top() + rect.height() * 0.38),
            rect.max,
        );
        painter.rect_stroke(body, egui::Rounding::same(2.0), stroke);
        let lid = egui::Rect::from_min_max(
            egui::pos2(rect.left(), rect.top() + rect.height() * 0.22),
            egui::pos2(rect.right(), rect.top() + rect.height() * 0.38),
        );
        painter.rect_stroke(lid, egui::Rounding::same(2.0), stroke);
        // Le ruban, puis les deux boucles du nœud.
        painter.line_segment(
            [
                egui::pos2(rect.center().x, lid.top()),
                egui::pos2(rect.center().x, rect.bottom()),
            ],
            stroke,
        );
        for side in [-1.0_f32, 1.0] {
            let center = egui::pos2(
                rect.center().x + side * rect.width() * 0.16,
                rect.top() + rect.height() * 0.12,
            );
            painter.circle_stroke(center, rect.width() * 0.14, stroke);
        }
    }

    /// La boutique retenue : son identité, l'état du compte et la porte vers
    /// son site. La page elle-même n'est pas dessinée ici — elle vit dans sa
    /// propre fenêtre WebKit, où le défilement, les animations et le clavier
    /// se comportent comme dans un navigateur.
    pub(super) fn store_site(
        &mut self,
        ui: &mut egui::Ui,
        store: lapinux_core::stores::Store,
        connected: bool,
    ) {
        let url = store.site();
        // La fenêtre peut être fermée par son propre bouton : son état est relu
        // ici, sinon le panneau proposerait d'« afficher » une fenêtre absente.
        let status = self.webview.status(ui.ctx());
        if status.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(400));
        }
        let open_now = self.webview.is_open();
        let showing = open_now && self.webview.url() == url;
        let description = match store {
            lapinux_core::stores::Store::Steam => lapinux_core::i18n::t("stores.steam_description"),
            lapinux_core::stores::Store::Epic => lapinux_core::i18n::t("stores.epic_description"),
            lapinux_core::stores::Store::Gog => lapinux_core::i18n::t("stores.gog_description"),
            lapinux_core::stores::Store::Itch => lapinux_core::i18n::t("stores.itch_description"),
        };
        let mut open = false;
        let mut browser = false;
        let mut close = false;
        egui::Frame::none()
            .fill(CARD())
            .stroke(egui::Stroke::new(1.0_f32, BORDER()))
            .rounding(egui::Rounding::same(14.0))
            .inner_margin(egui::Margin::symmetric(20.0, 18.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    crate::paint_store_mark(ui, store, 52.0, TEXT());
                    ui.add_space(14.0);
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new(store.label())
                                .size(20.0)
                                .strong()
                                .color(TEXT()),
                        );
                        ui.add_space(3.0);
                        ui.label(
                            egui::RichText::new(description)
                                .size(11.5)
                                .color(TEXT_DIM()),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        status_dot(ui, if connected { OK } else { DANGER });
                        ui.add_space(2.0);
                        ui.label(
                            egui::RichText::new(if connected {
                                lapinux_core::i18n::t("stores.account_connected")
                            } else {
                                lapinux_core::i18n::t("stores.no_account")
                            })
                            .size(11.0)
                            .color(if connected {
                                OK
                            } else {
                                TEXT_DIM()
                            }),
                        );
                    });
                });
                ui.add_space(18.0);
                ui.horizontal(|ui| {
                    if action_button(
                        ui,
                        if showing {
                            lapinux_core::i18n::t("stores.show_window").to_string()
                        } else {
                            lapinux_core::i18n::t("stores.open_store").to_string()
                        },
                        ActionTone::Primary,
                    )
                    .clicked()
                    {
                        open = true;
                    }
                    ui.add_space(8.0);
                    if action_button(
                        ui,
                        lapinux_core::i18n::t("stores.in_browser"),
                        ActionTone::Quiet,
                    )
                    .clicked()
                    {
                        browser = true;
                    }
                    if open_now {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if action_button(
                                ui,
                                lapinux_core::i18n::t("stores.close_window"),
                                ActionTone::Quiet,
                            )
                            .clicked()
                            {
                                close = true;
                            }
                        });
                    }
                });
                ui.add_space(10.0);
                match &status {
                    Some(status) => {
                        // Vert quand la fenêtre est là, rouge quand c'est la
                        // raison de son absence qui s'affiche.
                        let color = if open_now { OK } else { DANGER };
                        ui.label(egui::RichText::new(status.clone()).size(11.0).color(color));
                    }
                    None => {
                        ui.label(
                            egui::RichText::new(lapinux_core::i18n::t("ui.browser_hint"))
                                .size(11.0)
                                .color(TEXT_DIM()),
                        );
                    }
                }
            });
        ui.add_space(10.0);
        ui.label(egui::RichText::new(url).size(10.5).color(TEXT_DIM()));
        if browser {
            crate::open_url(url);
        }
        if close {
            self.webview.close();
        } else if open {
            if showing {
                self.webview.present();
            } else {
                let title =
                    lapinux_core::i18n::t_args("ui.store_window", &[("name", store.label())]);
                self.webview.open(&self.paths, url, &title);
            }
        }
    }
}
