//! Palettes de couleurs : presets, import depuis le catalogue et export vers
//! `lapinux-configurations`.
//!
//! Une palette n'est pas un thème : elle remplace seulement les neuf couleurs
//! de base du thème actif. Le thème continue de décider de la disposition, des
//! arrondis et de l'échelle du texte.

use super::*;

impl LapinuxApp {
    /// Réglages › Apparence : palettes de couleurs, au-dessus des thèmes.
    /// Renvoie vrai quand quelque chose a changé, pour repeindre et enregistrer.
    pub(super) fn settings_palette_section(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;
        // La carte demande elle-même le catalogue distant : sans cela elle
        // restait sur la copie chargée au démarrage, et une palette publiée
        // depuis n'apparaissait jamais. La réponse arrive en tâche de fond,
        // d'où la nouvelle tentative une seconde plus tard.
        let stale = self.plugin_catalog.as_ref().map_or(true, |set| !set.remote);
        if stale
            && self.plugin_catalog_attempts < 12
            && self.plugin_catalog_rx.is_none()
            && self.plugin_catalog_asked.map_or(true, |asked| {
                asked.elapsed() > std::time::Duration::from_millis(800)
            })
        {
            self.ask_plugin_catalog(ui.ctx());
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(800));
        }
        // Aucune palette n'est livrée en dur : Kokiri, Vice et les suivantes
        // viennent du catalogue et se téléchargent, comme les thèmes.
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
            if action_button(
                ui,
                lapinux_core::i18n::t("settings.appearance.palette_reset"),
                ActionTone::Quiet,
            )
            .clicked()
            {
                self.config.general.palette.clear();
                changed = true;
            }
            if action_button(
                ui,
                lapinux_core::i18n::t("settings.appearance.palette_export"),
                ActionTone::Quiet,
            )
            .clicked()
            {
                self.export_palette();
            }
            if action_button(
                ui,
                lapinux_core::i18n::t("settings.appearance.palette_refresh"),
                ActionTone::Quiet,
            )
            .clicked()
            {
                // Le catalogue est mis en cache par le démon : on force sa
                // relecture pour voir arriver une palette publiée à l'instant.
                let _ = self.client.request(&Req::RefreshCatalog);
                self.plugin_catalog = None;
                self.plugin_catalog_asked = None;
                self.plugin_catalog_attempts = 0;
                self.ask_plugin_catalog(ui.ctx());
            }
        });
        ui.add_space(6.0);
        // Les neuf couleurs tiennent sur une ligne : une pastille + son nom,
        // le code hexadécimal en infobulle. Neuf lignes prenaient la moitié de
        // la carte pour un réglage qu'on touche rarement.
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(7.0, 5.0);
            // Les clés sont statiques : `i18n::t` refuse une clé construite.
            for (field, label_key) in [
                ("background", "settings.appearance.color_background"),
                ("panel", "settings.appearance.color_panel"),
                ("card", "settings.appearance.color_card"),
                ("card_hover", "settings.appearance.color_card_hover"),
                ("card_active", "settings.appearance.color_card_active"),
                ("border", "settings.appearance.color_border"),
                ("accent", "settings.appearance.color_accent"),
                ("text", "settings.appearance.color_text"),
                ("text_dim", "settings.appearance.color_text_dim"),
            ] {
                // Une couleur non choisie montre celle du thème : le
                // sélecteur part donc de ce que l'écran affiche déjà.
                let mut color = palette_hex(self.config.general.palette.get(field))
                    .unwrap_or_else(|| palette_field(self.active_theme.colors, field));
                ui.allocate_ui_with_layout(
                    egui::vec2(56.0, 40.0),
                    egui::Layout::top_down(egui::Align::Center),
                    |ui| {
                        let response = ui.color_edit_button_srgba(&mut color);
                        if response.changed() {
                            self.config
                                .general
                                .palette
                                .set(field, &palette_hex_of(color));
                            changed = true;
                        }
                        response.on_hover_text(format!(
                            "{} · {}",
                            lapinux_core::i18n::t(label_key),
                            palette_hex_of(color)
                        ));
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(lapinux_core::i18n::t(label_key))
                                    .size(8.5)
                                    .color(TEXT_DIM()),
                            )
                            .truncate(),
                        );
                    },
                );
            }
        });
        muted(ui, lapinux_core::i18n::t("settings.appearance.palette_hex"));

        // Catalogue : les palettes déjà téléchargées d'abord, puis celles que
        // le dépôt propose. Une palette est un jeu de couleurs : la
        // télécharger ne fait que la garder dans la configuration, sans dépôt
        // de fichier ni redémarrage.
        ui.add_space(12.0);
        ui.label(
            egui::RichText::new(lapinux_core::i18n::t("settings.appearance.palette_catalog"))
                .size(12.0)
                .strong()
                .color(TEXT()),
        );
        ui.add_space(4.0);
        let saved = self.config.general.saved_palettes.clone();
        let remote: Vec<lapinux_core::configurations::PaletteEntry> = self
            .plugin_catalog
            .as_ref()
            .map(|catalog| catalog.palettes.clone())
            .unwrap_or_default();
        if saved.is_empty() {
            muted(
                ui,
                lapinux_core::i18n::t("settings.appearance.palette_none"),
            );
        }
        for entry in &saved {
            let mut use_it = false;
            let mut remove_it = false;
            // Une palette du catalogue plus récente remplace la copie locale :
            // sinon une correction de couleurs resterait invisible.
            let newer = remote
                .iter()
                .find(|remote| remote.id == entry.id && remote.version != entry.version)
                .cloned();
            let mut update_it = false;
            ui.horizontal(|ui| {
                palette_swatch(ui, &entry.colors);
                ui.add_space(4.0);
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(&entry.title)
                            .size(12.0)
                            .strong()
                            .color(TEXT()),
                    );
                    let author = if entry.author.name.trim().is_empty() {
                        lapinux_core::i18n::t("settings.appearance.palette_saved")
                    } else {
                        entry.author.name.as_str()
                    };
                    muted(ui, &format!("{author} · {}", entry.version));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if action_button(
                        ui,
                        lapinux_core::i18n::t("settings.appearance.palette_remove"),
                        ActionTone::Quiet,
                    )
                    .clicked()
                    {
                        remove_it = true;
                    }
                    if action_button(
                        ui,
                        lapinux_core::i18n::t("settings.appearance.palette_use"),
                        ActionTone::Light,
                    )
                    .clicked()
                    {
                        use_it = true;
                    }
                    if newer.is_some()
                        && action_button(
                            ui,
                            lapinux_core::i18n::t("settings.appearance.palette_update"),
                            ActionTone::Light,
                        )
                        .clicked()
                    {
                        update_it = true;
                    }
                });
            });
            if let Some(newer) = newer.filter(|_| update_it) {
                self.config.general.palette = newer.colors.clone();
                if let Some(slot) = self
                    .config
                    .general
                    .saved_palettes
                    .iter_mut()
                    .find(|saved| saved.id == entry.id)
                {
                    *slot = newer;
                }
                changed = true;
            }
            if use_it {
                self.config.general.palette = entry.colors.clone();
                changed = true;
            }
            if remove_it {
                self.config
                    .general
                    .saved_palettes
                    .retain(|saved| saved.id != entry.id);
                changed = true;
            }
            ui.add_space(4.0);
        }
        let offered: Vec<_> = remote
            .iter()
            .filter(|entry| {
                !self
                    .config
                    .general
                    .saved_palettes
                    .iter()
                    .any(|saved| saved.id == entry.id)
            })
            .cloned()
            .collect();
        for entry in &offered {
            let mut download = false;
            ui.horizontal(|ui| {
                palette_swatch(ui, &entry.colors);
                ui.add_space(4.0);
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(&entry.title)
                            .size(12.0)
                            .strong()
                            .color(TEXT()),
                    );
                    muted(ui, &entry.description);
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if action_button(
                        ui,
                        lapinux_core::i18n::t("settings.appearance.palette_download"),
                        ActionTone::Light,
                    )
                    .clicked()
                    {
                        download = true;
                    }
                });
            });
            if download {
                self.config.general.saved_palettes.push(entry.clone());
                self.config.general.palette = entry.colors.clone();
                changed = true;
            }
            ui.add_space(4.0);
        }
        if offered.is_empty() && self.plugin_catalog.as_ref().is_some_and(|set| !set.remote) {
            muted(
                ui,
                lapinux_core::i18n::t("settings.appearance.palette_offline"),
            );
        }
        changed
    }

    /// Écrit la palette courante comme une entrée de catalogue, prête à être
    /// déposée dans `lapinux-configurations/palettes/`. Le dépôt n'accepte que
    /// des données : aucune authentification n'est demandée ici.
    fn export_palette(&mut self) {
        let colors = self.config.general.palette.clone();
        if !colors.is_complete() {
            self.notify(lapinux_core::i18n::t(
                "settings.appearance.palette_export_incomplete",
            ));
            return;
        }
        let stamp = lapinux_core::util::now_unix().max(0);
        let entry = lapinux_core::configurations::PaletteEntry {
            id: format!("palette-{stamp}"),
            title: format!("Palette {stamp}"),
            description: lapinux_core::i18n::t("settings.appearance.palette_export_desc")
                .to_string(),
            author: lapinux_core::configurations::Author::default(),
            version: "1.0.0".into(),
            license: "CC0-1.0".into(),
            status: "experimental".into(),
            colors,
        };
        let Some(root) = self.theme_library.dir.parent() else {
            self.notify(lapinux_core::i18n::t(
                "settings.appearance.palette_export_failed",
            ));
            return;
        };
        let dir = root.join("palettes");
        let path = dir.join(format!("{}.json", entry.id));
        let written = std::fs::create_dir_all(&dir)
            .and_then(|_| serde_json::to_string_pretty(&entry).map_err(std::io::Error::other))
            .and_then(|text| std::fs::write(&path, text));
        match written {
            Ok(()) => {
                self.notify(lapinux_core::i18n::t_args(
                    "settings.appearance.palette_exported",
                    &[("path", &path.display().to_string())],
                ));
                open_path(&dir);
            }
            Err(error) => self.notify(lapinux_core::i18n::t_args(
                "settings.appearance.palette_export_failed",
                &[("error", &error.to_string())],
            )),
        }
    }
}

/// Trois pastilles — fond, carte, accent — pour reconnaître une palette d'un
/// coup d'œil dans la liste.
fn palette_swatch(ui: &mut egui::Ui, colors: &lapinux_core::config::PaletteOverride) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(38.0, 30.0), egui::Sense::hover());
    let painter = ui.painter();
    let background = palette_hex(colors.get("background")).unwrap_or(CARD());
    let card = palette_hex(colors.get("card")).unwrap_or(CARD());
    let accent = palette_hex(colors.get("accent")).unwrap_or(ACCENT());
    painter.rect_filled(rect, egui::Rounding::same(7.0), background);
    painter.rect_filled(
        egui::Rect::from_min_size(
            rect.left_top() + egui::vec2(5.0, 5.0),
            egui::vec2(16.0, 12.0),
        ),
        egui::Rounding::same(3.0),
        card,
    );
    painter.circle_filled(rect.right_bottom() - egui::vec2(9.0, 9.0), 4.5, accent);
    painter.rect_stroke(
        rect,
        egui::Rounding::same(7.0),
        egui::Stroke::new(1.0_f32, BORDER()),
    );
}
