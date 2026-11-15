//! Sélection et gestion des jaquettes.

use super::*;

impl LapinuxApp {
    pub(super) fn open_cover_picker(&mut self, appid: u32, name: String, ctx: &egui::Context) {
        let (mut picker, sender) = CoverPicker::start(appid, name.clone());
        // La fenêtre ouverte depuis une jaquette ne propose que des jaquettes :
        // les capsules, icônes, bannières et logos ont leur propre entrée.
        picker.only_covers = true;
        let key = self.config.artwork.steamgriddb_key.trim().to_string();
        let kind = picker.kind;
        if key.is_empty() {
            picker.loading = false;
            picker.error = Some(lapinux_core::i18n::t("cover_picker.key_for_cover").to_string());
            self.cover_picker = Some(picker);
            return;
        }
        let repaint = ctx.clone();
        std::thread::spawn(move || match sgdb_options(appid, &name, &key, kind) {
            Ok(options) => {
                let _ = sender.send(CoverMsg::Grids(options.clone()));
                repaint.request_repaint();
                for option in options {
                    // Certaines miniatures SteamGridDB répondent 404 alors
                    // que l'image pleine taille existe : on retombe sur elle.
                    let mut downloaded = if option.gif {
                        download_image(&option.full)
                    } else {
                        download_image(&option.thumb).or_else(|| download_image(&option.full))
                    };
                    // Une miniature illisible (format exotique, réponse
                    // tronquée) ne doit pas laisser un cadre vide : l'image
                    // pleine taille est essayée, puis une seconde fois.
                    if downloaded
                        .as_deref()
                        .is_some_and(|bytes| image::load_from_memory(bytes).is_err())
                    {
                        downloaded = download_image(&option.full);
                    }
                    if downloaded.is_none() {
                        std::thread::sleep(Duration::from_millis(250));
                        downloaded = download_image(&option.full);
                    }
                    if let Some(bytes) = downloaded {
                        let _ = sender.send(CoverMsg::Thumb {
                            url: option.thumb,
                            bytes,
                        });
                        repaint.request_repaint();
                    }
                    std::thread::sleep(Duration::from_millis(60));
                }
            }
            Err(error) => {
                let _ = sender.send(CoverMsg::Failed(error));
                repaint.request_repaint();
            }
        });
        self.cover_picker = Some(picker);
    }

    /// Panneau de choix d'une image : galerie de vignettes SteamGridDB, par
    /// type (jaquette, capsule, icône, bannière, logo), dans le châssis fixe
    /// de l'application.
    pub(super) fn cover_picker_window(&mut self, ctx: &egui::Context) {
        let Some(mut picker) = self.cover_picker.take() else {
            return;
        };
        let mut close = false;
        while let Ok(message) = picker.rx.try_recv() {
            match message {
                CoverMsg::Grids(options) => {
                    picker.options = options;
                    picker.loading = false;
                }
                CoverMsg::Thumb { url, bytes } => {
                    let name = format!("cover-pick-{url}");
                    let thumb = if bytes.starts_with(b"GIF8") {
                        GifPlayer::load(ctx, &bytes, &name, [320, 240]).map(Thumb::Gif)
                    } else {
                        image::load_from_memory(&bytes)
                            .ok()
                            .map(|image| Thumb::Still(texture_from_image(ctx, image, &name)))
                    };
                    picker.thumbs.insert(url, thumb);
                }
                CoverMsg::Failed(error) => {
                    picker.error = Some(error);
                    picker.loading = false;
                    picker.saving = false;
                }
                CoverMsg::Saved(result) => {
                    picker.saving = false;
                    match result {
                        Ok(()) => {
                            self.covers.forget(picker.appid);
                            self.notify(lapinux_core::i18n::t("cover_picker.saved"));
                            close = true;
                        }
                        Err(error) => picker.error = Some(error),
                    }
                }
            }
        }
        if picker.loading || picker.saving {
            ctx.request_repaint_after(Duration::from_millis(150));
        }

        let mut chosen: Option<String> = None;
        let mut reset = false;
        let mut wanted_kind: Option<CoverKind> = None;
        let has_custom = custom_cover_path(&self.paths, picker.appid).is_file();
        let subtitle = lapinux_core::i18n::t_fmt("cover_picker.subtitle", &[picker.name.clone()]);
        if !close {
            // Le châssis fixe de l'application, comme les Succès ou les
            // contenus additionnels : voile, panneau centré, croix et Échap.
            // Une fenêtre flottante se perdait derrière la fiche du jeu.
            let panel_close = fixed_panel(
                ctx,
                "cover-panel",
                lapinux_core::i18n::t("cover_picker.choose"),
                &subtitle,
                egui::vec2(820.0, 780.0),
                |ui| {
                    ui.horizontal(|ui| {
                        if has_custom
                            && link_text(ui, lapinux_core::i18n::t("cover_picker.reset")).clicked()
                        {
                            reset = true;
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if !picker.options.is_empty() {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{} image(s)",
                                        picker.options.len()
                                    ))
                                    .size(11.0)
                                    .color(TEXT_DIM()),
                                );
                            }
                        });
                    });
                    ui.add_space(10.0);
                    // Le type d'image : une jaquette portrait, une capsule
                    // paysage, une icône carrée, une bannière ou un logo. La
                    // galerie change de cadre et de contenu avec lui.
                    if !picker.only_covers {
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                            for kind in CoverKind::ALL {
                                let selected = picker.kind == kind;
                                let text = egui::RichText::new(kind.label())
                                    .size(11.5)
                                    .color(if selected { TEXT() } else { TEXT_DIM() });
                                let button = egui::Button::new(text)
                                    .fill(if selected { CARD_ACTIVE() } else { CARD() })
                                    .rounding(egui::Rounding::same(7.0))
                                    .min_size(egui::vec2(0.0, 24.0));
                                if ui.add(button).clicked() && !selected {
                                    wanted_kind = Some(kind);
                                }
                            }
                        });
                    }
                    ui.add_space(12.0);
                    if let Some(error) = &picker.error {
                        ui.label(egui::RichText::new(sentence(error)).size(12.0).color(WARN));
                        ui.add_space(8.0);
                    }
                    if picker.saving {
                        ui.horizontal(|ui| {
                            crate::section_icons::loading(ui, 18.0);
                            ui.label(
                                egui::RichText::new(lapinux_core::i18n::t("ui.cover_saving"))
                                    .size(12.0)
                                    .color(TEXT_DIM()),
                            );
                        });
                    } else if picker.loading {
                        ui.horizontal(|ui| {
                            crate::section_icons::loading(ui, 18.0);
                            ui.label(
                                egui::RichText::new(lapinux_core::i18n::t(
                                    "cover_picker.searching",
                                ))
                                .size(12.0)
                                .color(TEXT_DIM()),
                            );
                        });
                    } else if picker.options.is_empty() && picker.error.is_none() {
                        ui.label(
                            egui::RichText::new(lapinux_core::i18n::t("cover_picker.none"))
                                .size(12.0)
                                .color(TEXT_DIM()),
                        );
                    }
                    ui.add_space(10.0);
                    // Le cadre suit le type demandé : une bannière ne se lit
                    // pas dans une vignette portrait.
                    let frame = picker.kind.frame();
                    let gap = 14.0;
                    let columns =
                        (((ui.available_width() + gap) / (frame.x + gap)).floor() as usize).max(1);
                    for row in picker.options.chunks(columns) {
                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing.x = gap;
                            for option in row {
                                ui.allocate_ui_with_layout(
                                    egui::vec2(frame.x, frame.y + 16.0),
                                    egui::Layout::top_down(egui::Align::Center),
                                    |ui| {
                                        ui.set_width(frame.x);
                                        let (rect, response) =
                                            ui.allocate_exact_size(frame, egui::Sense::click());
                                        let rounding = egui::Rounding::same(8.0);
                                        match picker.thumbs.get_mut(&option.thumb) {
                                            Some(Some(thumb)) => paint_cover(
                                                ui,
                                                rect,
                                                thumb.texture(ui.ctx()),
                                                rounding,
                                            ),
                                            _ => {
                                                ui.painter().rect_filled(rect, rounding, CARD());
                                                ui.painter().text(
                                                    rect.center(),
                                                    egui::Align2::CENTER_CENTER,
                                                    "…",
                                                    egui::FontId::proportional(18.0),
                                                    TEXT_DIM(),
                                                );
                                            }
                                        }
                                        if response.hovered() {
                                            // La vignette survolée s'encadre : le
                                            // clic pose l'image tout de suite.
                                            ui.painter().rect_stroke(
                                                rect.shrink(1.0),
                                                rounding,
                                                egui::Stroke::new(1.6_f32, ACCENT()),
                                            );
                                        }
                                        if response.clicked() {
                                            chosen = Some(option.full.clone());
                                        }
                                        response.on_hover_text(lapinux_core::i18n::t(
                                            "cover_picker.use_image",
                                        ));
                                        // La résolution sous la vignette : c'est
                                        // elle qui distingue deux images du même
                                        // jeu.
                                        let caption = if option.width > 0 && option.height > 0 {
                                            if option.gif {
                                                format!("GIF · {}×{}", option.width, option.height)
                                            } else {
                                                format!("{}×{}", option.width, option.height)
                                            }
                                        } else if option.gif {
                                            "GIF".to_string()
                                        } else {
                                            picker.kind.label().to_string()
                                        };
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(caption)
                                                    .size(10.0)
                                                    .color(TEXT_DIM()),
                                            )
                                            .truncate(),
                                        );
                                    },
                                );
                            }
                        });
                        ui.add_space(gap);
                    }
                },
            );
            if panel_close {
                close = true;
            }
        }

        if let Some(url) = chosen {
            picker.saving = true;
            picker.error = None;
            let path = self.paths.custom_cover_path(picker.appid);
            let legacy = self.paths.legacy_custom_cover_path(picker.appid);
            let sender = picker.sender.clone();
            let repaint = ctx.clone();
            std::thread::spawn(move || {
                let result = save_cover_from_url(&url, &path).and_then(|()| {
                    match std::fs::remove_file(&legacy) {
                        Ok(()) => Ok(()),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                        Err(error) => Err(lapinux_core::i18n::t_args(
                            "cover_picker.old_kept",
                            &[("error", &error.to_string())],
                        )),
                    }
                });
                let _ = sender.send(CoverMsg::Saved(result));
                repaint.request_repaint();
            });
        }

        if reset {
            if remove_custom_cover(&self.paths, picker.appid) {
                self.covers.forget(picker.appid);
                self.notify(lapinux_core::i18n::t("library.cover_reset"));
            }
            close = true;
        }

        if close {
            self.cover_picker = None;
        } else {
            self.cover_picker = Some(picker);
            if let Some(kind) = wanted_kind {
                self.load_cover_kind(kind, ctx);
            }
        }
    }

    /// Relance la recherche pour un autre type d'image : la galerie change de
    /// cadre et de contenu, et les miniatures déjà reçues ne servent plus.
    fn load_cover_kind(&mut self, kind: CoverKind, ctx: &egui::Context) {
        let Some(mut picker) = self.cover_picker.take() else {
            return;
        };
        picker.kind = kind;
        picker.options.clear();
        picker.thumbs.clear();
        picker.error = None;
        picker.loading = true;
        // Nouveau canal : les réponses du type précédent, encore en vol, ne
        // doivent pas repeupler la galerie qu'on vient de vider.
        let (sender, rx) = mpsc::channel();
        picker.sender = sender.clone();
        picker.rx = rx;
        let appid = picker.appid;
        let name = picker.name.clone();
        let key = self.config.artwork.steamgriddb_key.trim().to_string();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            if key.is_empty() {
                let _ = sender.send(CoverMsg::Failed(
                    lapinux_core::i18n::t("cover_picker.key_for_image").to_string(),
                ));
                repaint.request_repaint();
                return;
            }
            match sgdb_options(appid, &name, &key, kind) {
                Ok(options) => {
                    let _ = sender.send(CoverMsg::Grids(options.clone()));
                    repaint.request_repaint();
                    for option in options {
                        if let Some(bytes) = download_image(&option.thumb) {
                            let _ = sender.send(CoverMsg::Thumb {
                                url: option.thumb,
                                bytes,
                            });
                            repaint.request_repaint();
                        }
                        std::thread::sleep(Duration::from_millis(40));
                    }
                }
                Err(error) => {
                    let _ = sender.send(CoverMsg::Failed(error));
                    repaint.request_repaint();
                }
            }
        });
        self.cover_picker = Some(picker);
    }
}
