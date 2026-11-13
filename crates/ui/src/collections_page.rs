//! Page des collections de jeux.

use super::*;

impl LapinuxApp {
    /// Page Collections : la grille des collections, ou le contenu d'une
    /// collection ouverte. Une collection se parcourt comme la bibliothèque.
    pub(super) fn collections_page(&mut self, ui: &mut egui::Ui) {
        self.covers.begin_frame(10);
        let entries = self.library_entries_all();
        let selected = self.collection_selected.as_ref().is_some_and(|id| {
            self.config
                .collections
                .items
                .iter()
                .any(|item| &item.id == id)
        });
        if selected {
            self.collections_content(ui, &entries);
        } else {
            if self.collection_selected.take().is_some() {
                self.collection_manage = false;
                self.collection_search.clear();
                self.collection_delete = None;
            }
            self.collections_overview(ui, &entries);
        }
        if self.config_dirty_at.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(200));
        }
    }

    /// Grille d'entrée : chaque collection est une tuile, et un clic ouvre son
    /// contenu. La création passe par la tuile +, sans en-tête de page.
    fn collections_overview(&mut self, ui: &mut egui::Ui, entries: &[LibraryEntry]) {
        if self.collection_new {
            self.collection_new_form(ui);
            ui.add_space(12.0);
        }
        let collections = self.config.collections.items.clone();
        if collections.is_empty() {
            ui.label(
                egui::RichText::new(lapinux_core::i18n::t("collections.empty"))
                    .size(11.0)
                    .color(TEXT_DIM()),
            );
            ui.label(
                egui::RichText::new(lapinux_core::i18n::t("collections.empty_hint"))
                    .size(10.5)
                    .color(TEXT_DIM()),
            );
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                for (field, label) in [
                    (
                        lapinux_core::config::CollectionField::Publisher,
                        lapinux_core::i18n::t("collections.by_publisher"),
                    ),
                    (
                        lapinux_core::config::CollectionField::Developer,
                        lapinux_core::i18n::t("collections.by_developer"),
                    ),
                    (
                        lapinux_core::config::CollectionField::Store,
                        lapinux_core::i18n::t("collections.by_store"),
                    ),
                    (
                        lapinux_core::config::CollectionField::Genre,
                        lapinux_core::i18n::t("collections.by_genre"),
                    ),
                    (
                        lapinux_core::config::CollectionField::Year,
                        lapinux_core::i18n::t("collections.by_year"),
                    ),
                ] {
                    let button =
                        egui::Button::new(egui::RichText::new(label).size(11.0).color(TEXT_DIM()))
                            .fill(CARD())
                            .stroke(egui::Stroke::new(1.0_f32, BORDER()));
                    if ui.add(button).clicked() {
                        self.create_collection(label.to_string(), Some(field));
                    }
                }
            });
            ui.add_space(10.0);
        }
        let gap = self.config.library.grid_spacing as f32;
        let total = collections.len() + 1;
        let columns = grid_columns(ui.available_width(), COLLECTION_TILE, gap);
        egui::ScrollArea::vertical()
            .id_salt("collections-overview")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // Mêmes rangées centrées que la bibliothèque, avec la tuile
                // d'ajout à la suite des collections.
                ui.spacing_mut().item_spacing = egui::vec2(0.0, gap);
                for start in (0..total).step_by(columns) {
                    let count = (total - start).min(columns);
                    let pad = row_padding(ui.available_width(), count, COLLECTION_TILE, gap);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = gap;
                        ui.add_space(pad);
                        for index in start..start + count {
                            match collections.get(index) {
                                Some(collection) => self.collection_tile(ui, collection, entries),
                                None => self.new_collection_tile(ui),
                            }
                        }
                    });
                }
            });
    }

    /// Une collection dans la grille : mosaïque des premières jaquettes, nom
    /// et nombre de jeux sur un bandeau sombre. Un clic ouvre la collection.
    fn collection_tile(
        &mut self,
        ui: &mut egui::Ui,
        collection: &lapinux_core::config::GameCollection,
        entries: &[LibraryEntry],
    ) {
        let members = collection_members(collection, entries);
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(COLLECTION_TILE, COLLECTION_TILE),
            egui::Sense::click(),
        );
        let subtitle = collection_subtitle(collection, members.len());
        if ui.is_rect_visible(rect) {
            ui.painter()
                .rect_filled(rect, egui::Rounding::same(12.0), CARD());
            if members.is_empty() {
                paint_collection_glyph(ui.painter(), rect.center(), 54.0, TEXT_DIM());
            } else {
                let inner = rect.shrink(3.0);
                let shown = &members[..members.len().min(4)];
                let gap = 3.0;
                if shown.len() == 1 {
                    self.paint_collection_cell(ui, inner, shown[0], 9.0);
                } else {
                    let half_w = (inner.width() - gap) / 2.0;
                    let half_h = (inner.height() - gap) / 2.0;
                    let left = inner.left();
                    let right = left + half_w + gap;
                    let top = inner.top();
                    let bottom = top + half_h + gap;
                    if shown.len() == 2 {
                        self.paint_collection_cell(
                            ui,
                            egui::Rect::from_min_size(
                                egui::pos2(left, top),
                                egui::vec2(half_w, inner.height()),
                            ),
                            shown[0],
                            9.0,
                        );
                        self.paint_collection_cell(
                            ui,
                            egui::Rect::from_min_size(
                                egui::pos2(right, top),
                                egui::vec2(half_w, inner.height()),
                            ),
                            shown[1],
                            9.0,
                        );
                    } else {
                        let cells = [
                            egui::Rect::from_min_size(
                                egui::pos2(left, top),
                                egui::vec2(half_w, half_h),
                            ),
                            egui::Rect::from_min_size(
                                egui::pos2(right, top),
                                egui::vec2(half_w, half_h),
                            ),
                            egui::Rect::from_min_size(
                                egui::pos2(left, bottom),
                                egui::vec2(half_w, half_h),
                            ),
                            egui::Rect::from_min_size(
                                egui::pos2(right, bottom),
                                egui::vec2(half_w, half_h),
                            ),
                        ];
                        for (cell, entry) in cells.iter().zip(shown.iter()) {
                            self.paint_collection_cell(ui, *cell, entry, 8.0);
                        }
                    }
                }
            }
            let overlay = egui::Rect::from_min_max(
                egui::pos2(rect.left() + 1.0, rect.bottom() - 45.0),
                egui::pos2(rect.right() - 1.0, rect.bottom() - 1.0),
            );
            let painter = ui.painter();
            painter.rect_filled(
                overlay,
                egui::Rounding {
                    nw: 0.0,
                    ne: 0.0,
                    sw: 12.0,
                    se: 12.0,
                },
                egui::Color32::from_rgba_unmultiplied(10, 12, 16, 232),
            );
            painter.text(
                egui::pos2(overlay.left() + 10.0, overlay.top() + 7.0),
                egui::Align2::LEFT_TOP,
                lapinux_core::util::ellipsize(&collection.name, 19),
                egui::FontId::proportional(12.0),
                TEXT(),
            );
            painter.text(
                egui::pos2(overlay.left() + 10.0, overlay.bottom() - 7.0),
                egui::Align2::LEFT_BOTTOM,
                &subtitle,
                egui::FontId::proportional(9.5),
                TEXT_DIM(),
            );
            painter.rect_stroke(
                rect,
                egui::Rounding::same(12.0),
                if response.hovered() {
                    egui::Stroke::new(1.5_f32, ACCENT())
                } else {
                    egui::Stroke::new(1.0_f32, BORDER())
                },
            );
        }
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if response.clicked() {
            self.collection_selected = Some(collection.id.clone());
            self.collection_detail_open = false;
            self.selected = None;
            self.collection_manage = false;
            self.collection_search.clear();
            self.collection_delete = None;
            self.collection_new = false;
        }
        let _ = response.on_hover_text(format!("{}\n{}", collection.name, subtitle));
    }

    /// Tuile d'ajout, à la fin de la grille des collections.
    fn new_collection_tile(&mut self, ui: &mut egui::Ui) {
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(COLLECTION_TILE, COLLECTION_TILE),
            egui::Sense::click(),
        );
        if ui.is_rect_visible(rect) {
            let painter = ui.painter();
            painter.rect_filled(rect, egui::Rounding::same(12.0), CARD());
            let stroke = if response.hovered() {
                egui::Stroke::new(1.5_f32, ACCENT())
            } else {
                egui::Stroke::new(1.0_f32, BORDER())
            };
            painter.rect_stroke(rect, egui::Rounding::same(12.0), stroke);
            let center = rect.center() - egui::vec2(0.0, 8.0);
            let ink = if response.hovered() {
                ACCENT()
            } else {
                TEXT_DIM()
            };
            painter.line_segment(
                [center - egui::vec2(9.0, 0.0), center + egui::vec2(9.0, 0.0)],
                egui::Stroke::new(1.6_f32, ink),
            );
            painter.line_segment(
                [center - egui::vec2(0.0, 9.0), center + egui::vec2(0.0, 9.0)],
                egui::Stroke::new(1.6_f32, ink),
            );
            painter.text(
                egui::pos2(rect.center().x, rect.center().y + 24.0),
                egui::Align2::CENTER_CENTER,
                lapinux_core::i18n::t("collections.new"),
                egui::FontId::proportional(10.5),
                TEXT_DIM(),
            );
        }
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if response.clicked() {
            self.collection_new = true;
            self.collection_new_name.clear();
            self.collection_new_dynamic = false;
            self.collection_new_field = lapinux_core::config::CollectionField::Publisher;
        }
    }

    /// Jaquette d'une case de la mosaïque d'une collection.
    fn paint_collection_cell(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        entry: &LibraryEntry,
        rounding: f32,
    ) {
        let texture = self.covers.get(
            ui.ctx(),
            &self.paths,
            entry.appid,
            &entry.name,
            entry.store,
            &entry.cover,
            CoverTier::Tile,
        );
        match texture.as_ref() {
            Some(texture) => paint_cover(ui, rect, texture, egui::Rounding::same(rounding)),
            None => paint_cover_placeholder(ui, rect, &entry.name, egui::Rounding::same(rounding)),
        }
    }

    pub(super) fn back_to_collections(&mut self) {
        self.collection_selected = None;
        self.collection_detail_open = false;
        self.selected = None;
        self.collection_manage = false;
        self.collection_search.clear();
        self.collection_delete = None;
    }

    fn collection_new_form(&mut self, ui: &mut egui::Ui) {
        egui::Frame::none()
            .fill(CARD())
            .stroke(egui::Stroke::new(1.0_f32, BORDER()))
            .rounding(egui::Rounding::same(8.0))
            .inner_margin(egui::Margin::same(9.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width().min(460.0));
                ui.label(
                    egui::RichText::new(lapinux_core::i18n::t("collections.name"))
                        .size(10.0)
                        .color(TEXT_DIM()),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut self.collection_new_name)
                        .hint_text(if self.collection_new_dynamic {
                            lapinux_core::i18n::t("collections.name_optional")
                        } else {
                            lapinux_core::i18n::t("collections.name")
                        })
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(5.0);
                ui.horizontal(|ui| {
                    if ui
                        .selectable_label(
                            !self.collection_new_dynamic,
                            lapinux_core::i18n::t("collections.manual"),
                        )
                        .clicked()
                    {
                        self.collection_new_dynamic = false;
                    }
                    if ui
                        .selectable_label(
                            self.collection_new_dynamic,
                            lapinux_core::i18n::t("collections.dynamic"),
                        )
                        .clicked()
                    {
                        self.collection_new_dynamic = true;
                    }
                });
                if self.collection_new_dynamic {
                    ui.add_space(4.0);
                    egui::ComboBox::from_id_salt("collection-new-field")
                        .selected_text(collection_field_label(self.collection_new_field))
                        .show_ui(ui, |ui| {
                            for field in lapinux_core::config::CollectionField::ALL {
                                ui.selectable_value(
                                    &mut self.collection_new_field,
                                    field,
                                    collection_field_label(field),
                                );
                            }
                        });
                }
                ui.add_space(6.0);
                let can_create =
                    self.collection_new_dynamic || !self.collection_new_name.trim().is_empty();
                ui.horizontal(|ui| {
                    if action_button_enabled(
                        ui,
                        lapinux_core::i18n::t("collections.create"),
                        ActionTone::Light,
                        can_create,
                    )
                    .clicked()
                    {
                        let name = if self.collection_new_name.trim().is_empty() {
                            collection_field_default_name(self.collection_new_field).to_string()
                        } else {
                            self.collection_new_name.trim().to_string()
                        };
                        let field = self
                            .collection_new_dynamic
                            .then_some(self.collection_new_field);
                        self.create_collection(name, field);
                    }
                    if action_button(
                        ui,
                        lapinux_core::i18n::t("collections.cancel"),
                        ActionTone::Quiet,
                    )
                    .clicked()
                    {
                        self.collection_new = false;
                    }
                });
            });
    }

    fn create_collection(
        &mut self,
        name: String,
        field: Option<lapinux_core::config::CollectionField>,
    ) {
        let id = format!(
            "c{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or(0)
        );
        self.config
            .collections
            .items
            .push(lapinux_core::config::GameCollection {
                id: id.clone(),
                name,
                field,
                appids: Vec::new(),
            });
        self.collection_selected = Some(id);
        self.collection_detail_open = false;
        self.selected = None;
        self.collection_manage = field.is_none();
        self.collection_search.clear();
        self.collection_new = false;
        self.collection_new_name.clear();
        self.collections_changed();
    }

    fn collections_content(&mut self, ui: &mut egui::Ui, entries: &[LibraryEntry]) {
        let Some(id) = self.collection_selected.clone() else {
            self.collections_overview(ui, entries);
            return;
        };
        let Some(collection) = self
            .config
            .collections
            .items
            .iter()
            .find(|item| item.id == id)
            .cloned()
        else {
            self.collection_selected = None;
            self.collections_overview(ui, entries);
            return;
        };
        let back_label = format!("← {}", lapinux_core::i18n::t("collections.title"));
        let mut back = false;
        let mut ask_delete = false;
        let mut toggle_manage = false;
        ui.horizontal(|ui| {
            if action_button(ui, back_label, ActionTone::Quiet).clicked() {
                back = true;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if action_button(
                    ui,
                    lapinux_core::i18n::t("collections.delete"),
                    ActionTone::Danger,
                )
                .clicked()
                {
                    ask_delete = true;
                }
                if collection.field.is_none() {
                    let label = if self.collection_manage {
                        lapinux_core::i18n::t("collections.done")
                    } else {
                        lapinux_core::i18n::t("collections.manage")
                    };
                    if action_button(ui, label, ActionTone::Light).clicked() {
                        toggle_manage = true;
                    }
                }
            });
        });
        if back {
            if self.collection_detail_open && self.selected.is_some() {
                self.selected = None;
            } else {
                self.back_to_collections();
            }
            return;
        }
        if ask_delete {
            self.collection_delete = Some(collection.id.clone());
        }
        if toggle_manage {
            self.collection_manage = !self.collection_manage;
            self.collection_search.clear();
        }
        ui.add_space(10.0);
        if self.collection_delete.as_deref() == Some(collection.id.as_str()) {
            let mut delete = false;
            let mut cancel = false;
            egui::Frame::none()
                .fill(mix(CARD(), DANGER, 0.06))
                .stroke(egui::Stroke::new(1.0_f32, mix(DANGER, CARD(), 0.55)))
                .rounding(egui::Rounding::same(8.0))
                .inner_margin(egui::Margin::symmetric(12.0, 8.0))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(lapinux_core::i18n::t(
                                "collections.delete_confirm",
                            ))
                            .size(11.5)
                            .color(TEXT()),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if action_button(
                                ui,
                                lapinux_core::i18n::t("collections.delete"),
                                ActionTone::Danger,
                            )
                            .clicked()
                            {
                                delete = true;
                            }
                            if action_button(
                                ui,
                                lapinux_core::i18n::t("collections.cancel"),
                                ActionTone::Quiet,
                            )
                            .clicked()
                            {
                                cancel = true;
                            }
                        });
                    });
                });
            if delete {
                self.delete_collection(&collection.id);
                return;
            }
            if cancel {
                self.collection_delete = None;
            }
            ui.add_space(8.0);
        }
        if collection.field.is_none() && self.collection_manage {
            self.collections_manage(ui, &collection, entries);
            return;
        }
        match collection.field {
            Some(field) => self.collections_dynamic(ui, &collection, entries, field),
            None => {
                let members: Vec<&LibraryEntry> = entries
                    .iter()
                    .filter(|entry| collection.appids.contains(&entry.appid))
                    .collect();
                if members.is_empty() {
                    ui.label(
                        egui::RichText::new(lapinux_core::i18n::t("collections.no_games"))
                            .size(11.5)
                            .color(TEXT_DIM()),
                    );
                    ui.add_space(8.0);
                    if action_button(
                        ui,
                        lapinux_core::i18n::t("collections.manage"),
                        ActionTone::Light,
                    )
                    .clicked()
                    {
                        self.collection_manage = true;
                    }
                } else {
                    // La grille possède sa zone de défilement : une seule
                    // ScrollArea, celle qui virtualise les rangées.
                    self.collection_grid(ui, &members, Some(&collection), true);
                }
            }
        }
    }

    fn collections_dynamic(
        &mut self,
        ui: &mut egui::Ui,
        collection: &lapinux_core::config::GameCollection,
        entries: &[LibraryEntry],
        field: lapinux_core::config::CollectionField,
    ) {
        let groups = collection_groups(entries, field);
        if groups.is_empty() {
            ui.label(
                egui::RichText::new(lapinux_core::i18n::t("collections.no_games"))
                    .size(11.5)
                    .color(TEXT_DIM()),
            );
            return;
        }
        let groups_label = if groups.len() == 1 {
            lapinux_core::i18n::t("collections.groups_one")
        } else {
            lapinux_core::i18n::t("collections.groups_many")
        };
        ui.label(
            egui::RichText::new(format!("{} {}", groups.len(), groups_label))
                .size(10.5)
                .color(TEXT_DIM()),
        );
        // Ce que vaut chaque axe : le repli de l'année et le sort des jeux
        // sans donnée sont dits ici, une fois, plutôt que devinés.
        let axis_hint = match field {
            lapinux_core::config::CollectionField::Year => {
                Some(lapinux_core::i18n::t("collections.year_hint"))
            }
            lapinux_core::config::CollectionField::Genre => {
                Some(lapinux_core::i18n::t("collections.genre_hint"))
            }
            _ => None,
        };
        if let Some(hint) = axis_hint {
            ui.label(egui::RichText::new(hint).size(10.5).color(TEXT_DIM()));
        }
        ui.add_space(6.0);
        egui::ScrollArea::vertical()
            .id_salt(("collection-dynamic", &collection.id))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (name, games) in &groups {
                    egui::CollapsingHeader::new(
                        egui::RichText::new(format!("{}  ·  {}", name, games.len()))
                            .size(12.5)
                            .strong()
                            .color(TEXT()),
                    )
                    .id_salt(("collection-group", &collection.id, name))
                    .default_open(false)
                    .show(ui, |ui| {
                        ui.add_space(4.0);
                        // Le repli est déjà dans la ScrollArea des groupes :
                        // la grille se contente d'empiler ses rangées.
                        self.collection_grid(ui, games, None, false);
                        ui.add_space(8.0);
                    });
                }
            });
    }

    fn collections_manage(
        &mut self,
        ui: &mut egui::Ui,
        collection: &lapinux_core::config::GameCollection,
        entries: &[LibraryEntry],
    ) {
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.collection_search)
                    .hint_text(lapinux_core::i18n::t("collections.search"))
                    .desired_width(280.0),
            );
            ui.label(
                egui::RichText::new(format!(
                    "{} {}",
                    collection.appids.len(),
                    collection_games_label(collection.appids.len())
                ))
                .size(10.5)
                .color(TEXT_DIM()),
            );
        });
        ui.add_space(8.0);
        let needle = self.collection_search.trim().to_lowercase();
        let filtered: Vec<&LibraryEntry> = entries
            .iter()
            .filter(|entry| needle.is_empty() || entry.name.to_lowercase().contains(&needle))
            .collect();
        if filtered.is_empty() {
            ui.label(
                egui::RichText::new(lapinux_core::i18n::t("collections.no_result"))
                    .size(11.0)
                    .color(TEXT_DIM()),
            );
            return;
        }
        let id = collection.id.clone();
        const ROW: f32 = 72.0;
        egui::ScrollArea::vertical()
            .id_salt(("collection-manage", &id))
            .auto_shrink([false, false])
            .show_rows(ui, ROW, filtered.len(), |ui, range| {
                for &entry in &filtered[range] {
                    self.collection_manage_row(ui, &id, entry);
                    ui.add_space(4.0);
                }
            });
    }

    fn collection_manage_row(
        &mut self,
        ui: &mut egui::Ui,
        collection_id: &str,
        entry: &LibraryEntry,
    ) {
        let member = self
            .config
            .collections
            .items
            .iter()
            .find(|item| item.id == collection_id)
            .is_some_and(|item| item.appids.contains(&entry.appid));
        egui::Frame::none()
            .fill(CARD())
            .stroke(egui::Stroke::new(1.0_f32, BORDER()))
            .rounding(egui::Rounding::same(8.0))
            .inner_margin(egui::Margin::symmetric(8.0, 7.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let texture = self.covers.get(
                        ui.ctx(),
                        &self.paths,
                        entry.appid,
                        &entry.name,
                        entry.store,
                        &entry.cover,
                        CoverTier::Tile,
                    );
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(36.0, 54.0), egui::Sense::hover());
                    match texture.as_ref() {
                        Some(texture) => {
                            paint_cover(ui, rect, texture, egui::Rounding::same(5.0));
                        }
                        None => {
                            paint_cover_placeholder(
                                ui,
                                rect,
                                &entry.name,
                                egui::Rounding::same(5.0),
                            );
                        }
                    }
                    ui.add_space(4.0);
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new(&entry.name)
                                .size(12.5)
                                .strong()
                                .color(TEXT()),
                        );
                        let credits = if !entry.publishers.is_empty() {
                            entry.publishers.join(", ")
                        } else if !entry.developers.is_empty() {
                            entry.developers.join(", ")
                        } else {
                            lapinux_core::i18n::t("collections.no_credit").to_string()
                        };
                        ui.label(
                            egui::RichText::new(lapinux_core::util::ellipsize(&credits, 60))
                                .size(10.0)
                                .color(TEXT_DIM()),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let label = if member {
                            lapinux_core::i18n::t("collections.remove")
                        } else {
                            lapinux_core::i18n::t("collections.add")
                        };
                        let tone = if member {
                            ActionTone::Quiet
                        } else {
                            ActionTone::Light
                        };
                        if action_button(ui, label, tone).clicked() {
                            self.toggle_collection_game(collection_id, entry.appid, !member);
                            ui.ctx().request_repaint();
                        }
                    });
                });
            });
    }

    pub(super) fn toggle_collection_game(&mut self, collection_id: &str, appid: u32, member: bool) {
        if let Some(collection) = self
            .config
            .collections
            .items
            .iter_mut()
            .find(|item| item.id == collection_id)
        {
            if member {
                if !collection.appids.contains(&appid) {
                    collection.appids.push(appid);
                }
            } else {
                collection.appids.retain(|id| *id != appid);
            }
        }
        self.collections_changed();
    }

    fn delete_collection(&mut self, id: &str) {
        self.config.collections.items.retain(|item| item.id != id);
        self.collection_selected = None;
        self.collection_detail_open = false;
        self.selected = None;
        self.collection_delete = None;
        self.collection_manage = false;
        self.collection_search.clear();
        self.collections_changed();
    }

    /// Même aperçu riche que la bibliothèque : capsule large, captures après
    /// quelques secondes, icône et provenance. Il ne s'affiche que pour la
    /// jaquette survolée, sans précharger toute la collection.
    fn collection_game_hover(
        &mut self,
        ui: &mut egui::Ui,
        entry: &LibraryEntry,
        response: &egui::Response,
        cover: Option<&egui::TextureHandle>,
    ) {
        let ctx = ui.ctx().clone();
        let now = Instant::now();
        let started = match self.hover_since {
            Some((id, at)) if id == entry.appid => at,
            _ => {
                self.hover_since = Some((entry.appid, now));
                now
            }
        };
        self.hover_shots
            .ensure(&ctx, &self.paths, entry.appid, entry.store);
        let mut urls = self.hover_shots.get(entry.appid).to_vec();
        if urls.is_empty() {
            urls = self.store_info.screenshots(entry.appid);
        }
        // Ni fiche sur disque ni fiche déjà lue : la boutique est interrogée,
        // et le diaporama arrive dès qu'elle répond.
        if urls.is_empty() && entry.store.is_some() {
            self.store_info.ensure(&ctx, &self.client, entry.appid);
        }
        let store_banner = self.store_info.banner(entry.appid);
        if let Some(banner) = &store_banner {
            self.gameinfo
                .news_art
                .ensure(&ctx, std::slice::from_ref(banner));
        }
        if !urls.is_empty() {
            self.gameinfo.news_art.ensure(&ctx, &urls);
        }
        let shots: Vec<egui::TextureHandle> = urls
            .iter()
            .filter_map(|url| self.gameinfo.news_art.get(url).cloned())
            .collect();
        let details = self
            .catalog
            .as_ref()
            .and_then(|catalog| {
                catalog
                    .entries
                    .iter()
                    .find(|game| game.appid == entry.appid)
            })
            .map(|game| game.details.clone())
            .unwrap_or_default();
        // Même priorité que dans la bibliothèque : illustration large déjà
        // en cache, puis bannière de la boutique, puis jaquette de la tuile.
        let capsule = if entry.store.is_some() {
            self.art
                .get(
                    &ctx,
                    &self.paths,
                    entry.appid,
                    &details,
                    SteamArt::Wide,
                    &entry.name,
                )
                .or_else(|| {
                    store_banner
                        .as_deref()
                        .and_then(|url| self.gameinfo.news_art.get(url).cloned())
                })
                .or_else(|| cover.cloned())
        } else {
            self.art.get(
                &ctx,
                &self.paths,
                entry.appid,
                &details,
                SteamArt::Wide,
                &entry.name,
            )
        };
        let game_icon = self
            .art
            .get(
                &ctx,
                &self.paths,
                entry.appid,
                &details,
                SteamArt::Icon,
                &entry.name,
            )
            .or_else(|| square_icon_from_cover(&ctx, &self.paths, entry.appid));
        let origin = game_origin_label(
            entry.store,
            entry.steam,
            entry.family,
            entry.family_available,
        );
        let store_mark = game_store_mark(
            entry.store,
            entry.steam,
            entry.family,
            entry.family_available,
        );
        game_hover_card(
            &ctx,
            response.rect,
            entry.appid,
            &entry.name,
            capsule.as_ref(),
            self.art.focus_of(entry.appid, SteamArt::Wide),
            game_icon.as_ref(),
            store_mark,
            &shots,
            &origin,
            entry.update.is_some(),
            now.duration_since(started),
            self.config.library.hover_width as f32,
        );
    }

    /// Menu contextuel d'une jaquette de collection, le même que dans la
    /// bibliothèque : dossier installé, copie du chemin et jaquette.
    fn collection_game_menu(
        &mut self,
        ui: &mut egui::Ui,
        entry: &LibraryEntry,
        response: egui::Response,
        current_collection: Option<&lapinux_core::config::GameCollection>,
    ) {
        let has_custom = custom_cover_path(&self.paths, entry.appid).is_file();
        let install_dir = self
            .games
            .iter()
            .find(|game| game.appid == entry.appid && game.installed)
            .map(Game::install_dir);
        // L'icône n'est lue qu'à l'ouverture du menu, pas pour chaque tuile
        // de la grille.
        if response.secondary_clicked() {
            let details = self
                .catalog
                .as_ref()
                .and_then(|catalog| {
                    catalog
                        .entries
                        .iter()
                        .find(|game| game.appid == entry.appid)
                })
                .map(|game| game.details.clone())
                .unwrap_or_default();
            let icon = {
                let found = self.art.get(
                    ui.ctx(),
                    &self.paths,
                    entry.appid,
                    &details,
                    SteamArt::Icon,
                    &entry.name,
                );
                found.or_else(|| square_icon_from_cover(ui.ctx(), &self.paths, entry.appid))
            };
            self.cover_menu_icon = Some((entry.appid, icon));
        }
        let (pick, reset, folder, collection_changes, action) = {
            let icon = self
                .cover_menu_icon
                .as_ref()
                .filter(|(appid, _)| *appid == entry.appid)
                .and_then(|(_, icon)| icon.as_ref());
            cover_context_menu(
                response,
                entry.appid,
                &entry.name,
                icon,
                has_custom,
                entry.installed,
                false,
                install_dir.as_deref(),
                &[],
                current_collection,
            )
        };
        for (collection_id, member) in collection_changes {
            self.toggle_collection_game(&collection_id, entry.appid, member);
            ui.ctx().request_repaint();
        }
        if let Some(appid) = pick {
            self.open_cover_picker(appid, entry.name.clone(), ui.ctx());
        }
        match action {
            Some(GameAction::Play(appid)) => self.launch(appid, false),
            Some(GameAction::Install(appid)) => self.install_game(appid, Vec::new()),
            Some(GameAction::Sheet(appid)) => {
                self.selected = Some(appid);
                self.collection_detail_open = true;
            }
            Some(GameAction::Hide(appid)) => {
                if !self.config.library.hidden.contains(&appid) {
                    self.config.library.hidden.push(appid);
                }
                self.config_dirty_at = Some(Instant::now());
                self.touch_library();
            }
            Some(GameAction::Unhide(appid)) => {
                self.config.library.hidden.retain(|id| *id != appid);
                self.config_dirty_at = Some(Instant::now());
                self.touch_library();
            }
            None => {}
        }
        if let Some(appid) = reset {
            if remove_custom_cover(&self.paths, appid) {
                self.covers.forget(appid);
                self.notify(lapinux_core::i18n::t("library.cover_reset"));
            }
        }
        if let Some(path) = folder {
            self.open_folder(&path);
        }
    }

    /// Grille des collections. `scroll` dit qui possède le défilement : la
    /// grille elle-même (collection manuelle, plein écran) ou le parent (un
    /// groupe repliable déjà dans une ScrollArea). Jamais les deux.
    fn collection_grid(
        &mut self,
        ui: &mut egui::Ui,
        entries: &[&LibraryEntry],
        current_collection: Option<&lapinux_core::config::GameCollection>,
        scroll: bool,
    ) {
        if entries.is_empty() {
            ui.label(
                egui::RichText::new(lapinux_core::i18n::t("collections.no_games"))
                    .size(11.5)
                    .color(TEXT_DIM()),
            );
            return;
        }
        // Même grille que la bibliothèque : largeur réglable, mêmes écarts,
        // mêmes rangées centrées et jaquettes seules (le titre au survol).
        let gap = self.config.library.grid_spacing as f32;
        let card_width = self.grid_width;
        let columns = grid_columns(ui.available_width(), card_width, gap).max(1);
        let rows = entries.len().div_ceil(columns);
        if !scroll {
            self.collection_rows(
                ui,
                entries,
                0..rows,
                current_collection,
                columns,
                card_width,
                gap,
            );
            return;
        }
        // Une rangée vaut une jaquette au ratio 3:2 plus l'écart vertical :
        // show_rows ne construit alors que la fenêtre visible, et une
        // collection de mille jeux ne paie plus mille tuiles.
        let row_height = card_width * 1.5 + gap;
        let id = current_collection
            .map(|collection| collection.id.clone())
            .unwrap_or_else(|| "all".to_string());
        egui::ScrollArea::vertical()
            .id_salt(("collection-grid", id))
            .auto_shrink([false, false])
            .show_rows(ui, row_height, rows, |ui, range| {
                self.collection_rows(
                    ui,
                    entries,
                    range,
                    current_collection,
                    columns,
                    card_width,
                    gap,
                );
            });
    }

    #[allow(clippy::too_many_arguments)]
    fn collection_rows(
        &mut self,
        ui: &mut egui::Ui,
        entries: &[&LibraryEntry],
        range: std::ops::Range<usize>,
        current_collection: Option<&lapinux_core::config::GameCollection>,
        columns: usize,
        card_width: f32,
        gap: f32,
    ) {
        let radius = self.config.library.cover_radius as f32;
        let badges = self.config.library.show_badges;
        let badge_scale = self.config.library.badge_scale;
        ui.spacing_mut().item_spacing = egui::vec2(0.0, gap);
        for row in entries.chunks(columns).skip(range.start).take(range.len()) {
            let pad = row_padding(ui.available_width(), row.len(), card_width, gap);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                ui.add_space(pad);
                for &entry in row {
                    let texture = self.covers.get(
                        ui.ctx(),
                        &self.paths,
                        entry.appid,
                        &entry.name,
                        entry.store,
                        &entry.cover,
                        CoverTier::Tile,
                    );
                    let response = game_card(
                        ui,
                        texture.as_ref(),
                        &entry.name,
                        self.collection_detail_open && self.selected == Some(entry.appid),
                        card_width,
                        radius,
                        entry.steam,
                        entry.family,
                        entry.family_available,
                        entry.store,
                        badges,
                        badge_scale,
                        entry.update.is_some(),
                        false,
                    );
                    if response.clicked() {
                        self.selected = Some(entry.appid);
                        self.collection_detail_open = true;
                    }
                    if response.hovered() {
                        self.collection_game_hover(ui, entry, &response, texture.as_ref());
                    }
                    self.collection_game_menu(ui, entry, response, current_collection);
                }
            });
        }
    }
}
