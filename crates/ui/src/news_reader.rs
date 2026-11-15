//! Lecteur des actualités.

use super::*;

impl LapinuxApp {
    /// Ouvre des annonces dans le lecteur intégré. La carte cliquée amène son
    /// annonce en tête ; le lien global, lui, commence par la plus récente.
    pub(super) fn open_news_reader(
        &mut self,
        items: Vec<lapinux_core::model::SteamNews>,
        at: usize,
    ) {
        if items.is_empty() {
            return;
        }
        self.trailer.close();
        // Une seule description ouverte : le lecteur peut ainsi garder ses
        // quatre-vingt-dix-huit entrées sans mettre en page tout leur texte à
        // chaque frame. Les autres restent lisibles par leur aperçu.
        let at = at.min(items.len() - 1);
        let mut expanded = vec![false; items.len()];
        expanded[at] = true;
        self.news_reader = Some(NewsReader {
            heights: vec![0.0; items.len()],
            layout_width: 0.0,
            items,
            expanded,
            hovered: None,
            scroll_to: Some(at),
            opened_at: Instant::now(),
        });
    }

    /// Toutes les annonces du jeu dans un lecteur plein écran : une colonne
    /// de sections rectangulaires, sa barre de défilement au bord droit de la
    /// colonne, puis le sommaire daté, comme une page d'annonce de Steam.
    pub(super) fn news_reader_window(&mut self, ctx: &egui::Context) {
        let Some(mut reader) = self.news_reader.take() else {
            return;
        };
        if reader.items.is_empty() {
            self.news_reader = None;
            return;
        }
        if reader.expanded.len() != reader.items.len() {
            reader.expanded.resize(reader.items.len(), false);
            reader.heights.resize(reader.items.len(), 0.0);
        }
        let items = std::mem::take(&mut reader.items);
        let art = &mut self.gameinfo.news_art;
        art.poll(ctx);
        let mut open_link = None;
        let screen = ctx.screen_rect();
        // Un article isolé n'a pas besoin de sommaire : la colonne de lecture
        // reste centrée, sans panneau latéral vide.
        let index_width = if screen.width() >= 1120.0 && items.len() > 1 {
            NEWS_INDEX_WIDTH
        } else {
            0.0
        };
        let mut jump = None;
        let body = screen;
        let sidebar = if index_width > 0.0 {
            NEWS_INDEX_GAP + index_width
        } else {
            0.0
        };
        let feed_width =
            (NEWS_READER_WIDTH + NEWS_SCROLLBAR_WIDTH).min((body.width() - sidebar).max(200.0));
        let group_width = feed_width + sidebar;
        let group_left = ((body.width() - group_width) / 2.0).max(0.0);
        let inner = (feed_width - NEWS_SCROLLBAR_WIDTH).max(200.0);
        let feed_rect = egui::Rect::from_min_size(
            egui::pos2(body.left() + group_left, body.top()),
            egui::vec2(feed_width, body.height()),
        );
        let index_left = body.left() + group_left + feed_width + NEWS_INDEX_GAP;
        // Le sommaire ne remplit pas la hauteur de la fenêtre : ses marges
        // restent cliquables pour refermer le lecteur.
        let index_rect = egui::Rect::from_min_max(
            egui::pos2(index_left, body.top() + 24.0),
            egui::pos2(index_left + index_width, body.bottom() - 24.0),
        );
        let mut dismiss = false;
        // Un clic à côté du lecteur referme le voile ; un clic dans la colonne
        // ne fait qu'effacer le survol de l'article.
        if ctx.input(|input| input.pointer.any_click()) {
            reader.hovered = None;
        }
        let close =
            ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        egui::Window::new("Annonces")
            .id(egui::Id::new("news-reader"))
            .title_bar(false)
            .resizable(false)
            .collapsible(false)
            .movable(false)
            .fixed_pos(screen.min)
            .fixed_size(screen.size())
            .frame(egui::Frame::none())
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                // Le lecteur n'est plus une page opaque : le launcher reste
                // visible derrière un simple voile sombre. egui ne floute pas
                // la fenêtre déjà peinte, on garde donc ce voile translucide.
                ui.painter()
                    .rect_filled(screen, 0.0, egui::Color32::from_black_alpha(140));
                // Bandes autour de la colonne et du sommaire : elles absorbent
                // le clic destiné au launcher et referment le lecteur, comme
                // une lightbox. Les marges du sommaire et l'écart qui le
                // sépare du flux en font partie : sans elles, une bande morte
                // avalait le clic sans rien faire.
                let mut outside = vec![egui::Rect::from_min_max(
                    screen.min,
                    egui::pos2(feed_rect.left(), screen.max.y),
                )];
                if index_width > 0.0 {
                    outside.push(egui::Rect::from_min_max(
                        egui::pos2(index_rect.right(), screen.min.y),
                        screen.max,
                    ));
                    outside.push(egui::Rect::from_min_max(
                        egui::pos2(feed_rect.right(), screen.min.y),
                        egui::pos2(index_rect.left(), screen.max.y),
                    ));
                    outside.push(egui::Rect::from_min_max(
                        egui::pos2(index_rect.left(), screen.min.y),
                        egui::pos2(index_rect.right(), index_rect.top()),
                    ));
                    outside.push(egui::Rect::from_min_max(
                        egui::pos2(index_rect.left(), index_rect.bottom()),
                        egui::pos2(index_rect.right(), screen.max.y),
                    ));
                } else {
                    outside.push(egui::Rect::from_min_max(
                        egui::pos2(feed_rect.right(), screen.min.y),
                        screen.max,
                    ));
                }
                if reader.opened_at.elapsed() > Duration::from_millis(140) {
                    for (index, area) in outside.into_iter().enumerate() {
                        if area.width() > 0.0
                            && area.height() > 0.0
                            && ui
                                .interact(
                                    area,
                                    egui::Id::new(("news-backdrop", index)),
                                    egui::Sense::click(),
                                )
                                .clicked()
                        {
                            dismiss = true;
                        }
                    }
                }
                ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
                    ui.scope_builder(egui::UiBuilder::new().max_rect(feed_rect), |ui| {
                        ui.set_width(feed_width);
                        egui::ScrollArea::vertical()
                            .id_salt("news-feed")
                            .auto_shrink([false, false])
                            .scroll_bar_visibility(
                                egui::scroll_area::ScrollBarVisibility::AlwaysVisible,
                            )
                            .show(ui, |ui| {
                                // La barre occupe le bord de cette zone :
                                // l'article reste dans la largeur prévue.
                                let inner = ui.available_width().min(inner);
                                if (reader.layout_width - inner).abs() > 0.5 {
                                    reader.heights.fill(0.0);
                                    reader.layout_width = inner;
                                }
                                ui.horizontal_top(|ui| {
                                    ui.allocate_ui_with_layout(
                                        egui::vec2(inner, 0.0),
                                        egui::Layout::top_down(egui::Align::Min),
                                        |ui| {
                                            ui.set_width(inner);
                                            ui.add_space(24.0);
                                            let mut hovered_index = None;
                                            let mut toggle = None;
                                            let clip = ui.clip_rect();
                                            for (index, news) in items.iter().enumerate() {
                                                let estimated = reader
                                                    .heights
                                                    .get(index)
                                                    .copied()
                                                    .unwrap_or(0.0);
                                                let top = ui.cursor().top();
                                                let offscreen = estimated > 0.5
                                                    && reader.scroll_to != Some(index)
                                                    && (top + estimated < clip.top()
                                                        || top > clip.bottom());
                                                if offscreen {
                                                    ui.add_space(estimated + 16.0);
                                                    continue;
                                                }
                                                let expanded = reader
                                                    .expanded
                                                    .get(index)
                                                    .copied()
                                                    .unwrap_or(false);
                                                if reader.scroll_to == Some(index) {
                                                    // Le sommaire amène l'article
                                                    // demandé en haut du lecteur.
                                                    ui.scroll_to_cursor(Some(egui::Align::TOP));
                                                    reader.scroll_to = None;
                                                }
                                                let hovered = reader.hovered == Some(index);
                                                let article = ui.vertical(|ui| {
                                                    ui.spacing_mut().item_spacing.y = 0.0;
                                                    ui.set_width(inner);
                                                    // Pleine largeur, sans bordure :
                                                    // l'image ouvre l'annonce.
                                                    if !news.image.trim().is_empty() {
                                                        let height = (inner * 9.0 / 16.0)
                                                            .clamp(180.0, 340.0);
                                                        let (rect, _) = ui.allocate_exact_size(
                                                            egui::vec2(inner, height),
                                                            egui::Sense::hover(),
                                                        );
                                                        // Une annonce hors écran ne doit pas
                                                        // télécharger ni retenir son image : le
                                                        // lecteur peut lister des dizaines
                                                        // d'entrées sans remplir la mémoire.
                                                        if ui.clip_rect().intersects(rect) {
                                                            art.ensure(
                                                                ui.ctx(),
                                                                std::slice::from_ref(&news.image),
                                                            );
                                                        }
                                                        let texture = art.get(&news.image).cloned();
                                                        match texture.as_ref() {
                                                            Some(texture) => crate::paint_cover(
                                                                ui,
                                                                rect,
                                                                texture,
                                                                egui::Rounding::same(0.0),
                                                            ),
                                                            None => {
                                                                ui.painter().rect_filled(
                                                                    rect,
                                                                    0.0,
                                                                    egui::Color32::from_rgb(
                                                                        24, 25, 28,
                                                                    ),
                                                                );
                                                                ui.painter().text(
                                                                    rect.center(),
                                                                    egui::Align2::CENTER_CENTER,
                                                                    lapinux_core::i18n::t(
                                                                        "news_reader.image_loading",
                                                                    ),
                                                                    egui::FontId::proportional(
                                                                        10.0,
                                                                    ),
                                                                    TEXT_DIM(),
                                                                );
                                                            }
                                                        }
                                                    }
                                                    egui::Frame::none()
                                                        // Verre sombre : le fond
                                                        // reste perceptible sous
                                                        // le texte de l'annonce.
                                                        .fill(if hovered {
                                                            egui::Color32::from_rgba_unmultiplied(
                                                                26, 28, 33, 235,
                                                            )
                                                        } else {
                                                            egui::Color32::from_rgba_unmultiplied(
                                                                18, 19, 22, 220,
                                                            )
                                                        })
                                                        .stroke(egui::Stroke::new(
                                                            1.0_f32,
                                                            if hovered {
                                                                egui::Color32::from_rgb(72, 76, 84)
                                                            } else {
                                                                egui::Color32::from_rgb(43, 44, 47)
                                                            },
                                                        ))
                                                        .rounding(egui::Rounding::same(2.0))
                                                        .inner_margin(egui::Margin::symmetric(
                                                            24.0, 18.0,
                                                        ))
                                                        .show(ui, |ui| {
                                                            let content = (inner - 48.0).max(180.0);
                                                            ui.set_width(content);
                                                            let mut meta =
                                                                lapinux_core::util::format_date(
                                                                    news.date,
                                                                );
                                                            if !news.author.trim().is_empty() {
                                                                meta.push_str(" · ");
                                                                meta.push_str(news.author.trim());
                                                            }
                                                            ui.label(
                                                                egui::RichText::new(meta)
                                                                    .size(10.0)
                                                                    .color(TEXT_DIM()),
                                                            );
                                                            ui.add_space(5.0);
                                                            ui.label(
                                                                egui::RichText::new(&news.title)
                                                                    .size(19.0)
                                                                    .strong()
                                                                    .color(TEXT()),
                                                            );
                                                            let text = if expanded {
                                                                if news.body.trim().is_empty() {
                                                                    std::borrow::Cow::Borrowed(
                                                                        news.excerpt.as_str(),
                                                                    )
                                                                } else {
                                                                    std::borrow::Cow::Borrowed(
                                                                        news.body.as_str(),
                                                                    )
                                                                }
                                                            } else if !news
                                                                .excerpt
                                                                .trim()
                                                                .is_empty()
                                                            {
                                                                std::borrow::Cow::Borrowed(
                                                                    news.excerpt.as_str(),
                                                                )
                                                            } else {
                                                                std::borrow::Cow::Owned(
                                                                    crate::gameinfo::clamp_words(
                                                                        &news.body, 260,
                                                                    ),
                                                                )
                                                            };
                                                            if !text.trim().is_empty() {
                                                                ui.add_space(8.0);
                                                                if expanded {
                                                                    markdown_body(
                                                                        ui,
                                                                        text.as_ref(),
                                                                        content,
                                                                        TEXT(),
                                                                    );
                                                                } else {
                                                                    ui.add(
                                                                        egui::Label::new(
                                                                            egui::RichText::new(
                                                                                text.as_ref(),
                                                                            )
                                                                            .size(12.5)
                                                                            .color(TEXT_DIM()),
                                                                        )
                                                                        .wrap(),
                                                                    );
                                                                }
                                                            }
                                                            ui.add_space(12.0);
                                                            ui.horizontal(|ui| {
                                                                if !news.body.trim().is_empty() {
                                                                    let label = if expanded {
                                                                        lapinux_core::i18n::t(
                                                                            "news_reader.collapse",
                                                                        )
                                                                    } else {
                                                                        lapinux_core::i18n::t(
                                                                            "news_reader.read_more",
                                                                        )
                                                                    };
                                                                    if link_text(ui, label)
                                                                        .clicked()
                                                                    {
                                                                        toggle = Some(index);
                                                                    }
                                                                    if !news.url.trim().is_empty() {
                                                                        ui.add_space(14.0);
                                                                    }
                                                                }
                                                                if !news.url.trim().is_empty()
                                                                    && link_text(
                                                                        ui,
                                                                        lapinux_core::i18n::t(
                                                                            "news_reader.open_post",
                                                                        ),
                                                                    )
                                                                    .clicked()
                                                                {
                                                                    open_link = Some((
                                                                        news.url.clone(),
                                                                        format!(
                                                                            "{} · Lapinux",
                                                                            news.title
                                                                        ),
                                                                    ));
                                                                }
                                                            });
                                                        });
                                                });
                                                let measured =
                                                    article.response.rect.height().max(1.0);
                                                if let Some(slot) = reader.heights.get_mut(index) {
                                                    *slot = measured;
                                                }
                                                if ui.ctx().pointer_hover_pos().is_some_and(|pos| {
                                                    article.response.rect.contains(pos)
                                                }) {
                                                    hovered_index = Some(index);
                                                }
                                                ui.add_space(16.0);
                                            }
                                            if let Some(index) = toggle {
                                                let previous =
                                                    reader.expanded.iter().position(|open| *open);
                                                let opening = !reader
                                                    .expanded
                                                    .get(index)
                                                    .copied()
                                                    .unwrap_or(false);
                                                for open in &mut reader.expanded {
                                                    *open = false;
                                                }
                                                if opening && index < reader.expanded.len() {
                                                    reader.expanded[index] = true;
                                                }
                                                // Le corps change de hauteur : on
                                                // le remesurera à la prochaine frame.
                                                for changed in
                                                    [previous, Some(index)].into_iter().flatten()
                                                {
                                                    if let Some(height) =
                                                        reader.heights.get_mut(changed)
                                                    {
                                                        *height = 0.0;
                                                    }
                                                }
                                            }
                                            reader.hovered = hovered_index;
                                            ui.add_space(40.0);
                                        },
                                    );
                                });
                            });
                    });
                    // Sommaire fixe : une ligne datée par annonce. Il sert de
                    // repère dans les quatre-vingt-dix-huit publications et
                    // permet d'y sauter sans chercher dans le défilement.
                    if index_width > 0.0 {
                        ui.scope_builder(egui::UiBuilder::new().max_rect(index_rect), |ui| {
                            let panel = egui::Frame::none()
                                .fill(egui::Color32::from_rgba_unmultiplied(14, 16, 19, 235))
                                .stroke(egui::Stroke::new(
                                    1.0_f32,
                                    egui::Color32::from_rgb(43, 44, 47),
                                ))
                                .rounding(egui::Rounding::same(3.0))
                                .inner_margin(egui::Margin::symmetric(10.0, 10.0))
                                .show(ui, |ui| {
                                    ui.set_width(index_width - 20.0);
                                    let back = ui.add_sized(
                                        egui::vec2(ui.available_width(), 28.0),
                                        egui::Button::new(
                                            egui::RichText::new(format!(
                                                "←  {}",
                                                lapinux_core::i18n::t("news_reader.back")
                                            ))
                                            .size(11.5),
                                        )
                                        .fill(CARD_HOVER())
                                        .stroke(egui::Stroke::new(1.0_f32, BORDER()))
                                        .rounding(egui::Rounding::same(3.0)),
                                    );
                                    if back.clicked() {
                                        dismiss = true;
                                    }
                                    ui.add_space(8.0);
                                    // Le titre est centré dans la colonne,
                                    // au-dessus de la liste datée.
                                    ui.vertical_centered(|ui| {
                                        ui.label(
                                            egui::RichText::new(lapinux_core::i18n::t(
                                                "ui.news_summary",
                                            ))
                                            .size(11.0)
                                            .strong()
                                            .color(TEXT()),
                                        );
                                    });
                                    ui.add_space(6.0);
                                    egui::ScrollArea::vertical()
                                        .id_salt("news-index")
                                        .max_height((index_rect.height() - 108.0).max(90.0))
                                        .show_rows(ui, 39.0, items.len(), |ui, range| {
                                            for index in range {
                                                let news = &items[index];
                                                let (rect, response) = ui.allocate_exact_size(
                                                    egui::vec2(ui.available_width(), 38.0),
                                                    egui::Sense::click(),
                                                );
                                                if response.hovered()
                                                    || reader.hovered == Some(index)
                                                {
                                                    ui.painter().rect_filled(
                                                        rect,
                                                        3.0,
                                                        egui::Color32::from_rgb(35, 37, 42),
                                                    );
                                                }
                                                let painter = ui.painter().with_clip_rect(rect);
                                                painter.text(
                                                    rect.left_top() + egui::vec2(7.0, 4.0),
                                                    egui::Align2::LEFT_TOP,
                                                    lapinux_core::util::format_date(news.date),
                                                    egui::FontId::proportional(9.5),
                                                    TEXT_DIM(),
                                                );
                                                painter.text(
                                                    rect.left_top() + egui::vec2(7.0, 18.0),
                                                    egui::Align2::LEFT_TOP,
                                                    crate::gameinfo::clamp_words(&news.title, 30),
                                                    egui::FontId::proportional(10.5),
                                                    TEXT(),
                                                );
                                                if response.hovered() {
                                                    ui.ctx().set_cursor_icon(
                                                        egui::CursorIcon::PointingHand,
                                                    );
                                                }
                                                if response.clicked() {
                                                    jump = Some(index);
                                                }
                                                ui.add_space(1.0);
                                            }
                                        });
                                });
                            // Le panneau garde la hauteur de son contenu :
                            // toute la zone vide qui reste sous le
                            // sommaire referme le lecteur, comme le voile.
                            let empty = egui::Rect::from_min_max(
                                egui::pos2(index_rect.left(), panel.response.rect.bottom()),
                                egui::pos2(index_rect.right(), screen.bottom()),
                            );
                            if empty.height() > 0.0
                                && ui
                                    .interact(
                                        empty,
                                        egui::Id::new("news-index-backdrop"),
                                        egui::Sense::click(),
                                    )
                                    .clicked()
                            {
                                dismiss = true;
                            }
                        });
                    }
                });
            });
        if let Some(index) = jump {
            // Sauter depuis le sommaire ouvre l'article visé et referme le
            // précédent : une seule longue description reste en page.
            let previous = reader.expanded.iter().position(|open| *open);
            for (position, open) in reader.expanded.iter_mut().enumerate() {
                *open = position == index;
            }
            for changed in [previous, Some(index)].into_iter().flatten() {
                if let Some(height) = reader.heights.get_mut(changed) {
                    *height = 0.0;
                }
            }
            reader.scroll_to = Some(index);
            ctx.request_repaint();
        }
        reader.items = items;
        if close || dismiss {
            self.news_reader = None;
        } else {
            self.news_reader = Some(reader);
        }
        if let Some((url, title)) = open_link {
            self.webview.open(&self.paths, &url, &title);
            self.webview.present();
        }
    }
}

#[derive(Clone, Copy)]
enum MarkdownInline {
    Bold,
    Italic,
    Code,
    Link,
}

/// Rend le corps d'une annonce Steam. Les notes de mise à jour utilisent un
/// Markdown simple : titres, listes, gras, italique, code et liens. Les blocs
/// inconnus restent du texte, donc une annonce sans Markdown s'affiche comme
/// avant.
fn markdown_body(ui: &mut egui::Ui, text: &str, width: f32, color: egui::Color32) {
    let lines: Vec<&str> = text.lines().collect();
    let mut index = 0;
    let mut painted = false;
    while index < lines.len() {
        let line = lines[index].trim();
        if line.is_empty() {
            if painted {
                ui.add_space(4.0);
            }
            index += 1;
            continue;
        }
        // Bloc de code : les triples accents graves délimitent les lignes.
        if line.starts_with("```") {
            index += 1;
            let start = index;
            while index < lines.len() && !lines[index].trim().starts_with("```") {
                index += 1;
            }
            let code = lines[start..index].join("\n");
            if !code.trim().is_empty() {
                if painted {
                    ui.add_space(4.0);
                }
                egui::Frame::none()
                    .fill(PANEL())
                    .rounding(egui::Rounding::same(5.0))
                    .inner_margin(egui::Margin::symmetric(9.0, 7.0))
                    .show(ui, |ui| {
                        ui.set_width((width - 18.0).max(80.0));
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(code)
                                    .monospace()
                                    .size(11.0)
                                    .color(TEXT()),
                            )
                            .wrap(),
                        );
                    });
                painted = true;
            }
            if index < lines.len() {
                index += 1;
            }
            continue;
        }
        if let Some((level, heading)) = crate::gameinfo::markdown_heading(line) {
            if painted {
                ui.add_space(7.0);
            }
            ui.label(
                egui::RichText::new(heading)
                    .font(display_font(crate::gameinfo::heading_size(level)))
                    .color(TEXT()),
            );
            ui.add_space(2.0);
            painted = true;
            index += 1;
            continue;
        }
        if line == "---" || line == "***" || line == "___" {
            ui.add_space(3.0);
            ui.separator();
            ui.add_space(3.0);
            painted = true;
            index += 1;
            continue;
        }
        if let Some((marker, item)) = markdown_list_item(line) {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.label(egui::RichText::new(marker).size(12.0).color(ACCENT()));
                let job = markdown_inline_job(item, 12.5, color, ui.available_width());
                ui.add(egui::Label::new(job));
            });
            painted = true;
            index += 1;
            continue;
        }
        if let Some(quote) = line.strip_prefix('>') {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 7.0;
                ui.label(egui::RichText::new("▎").size(12.0).color(ACCENT()));
                let job =
                    markdown_inline_job(quote.trim_start(), 12.5, TEXT_DIM(), ui.available_width());
                ui.add(egui::Label::new(job));
            });
            painted = true;
            index += 1;
            continue;
        }
        let job = markdown_inline_job(line, 12.5, color, width);
        ui.add(egui::Label::new(job));
        painted = true;
        index += 1;
    }
}

/// Marqueur de liste : les puces Markdown, puis les listes numérotées.
fn markdown_list_item(line: &str) -> Option<(String, &str)> {
    for marker in ["- ", "* ", "+ "] {
        if let Some(item) = line.strip_prefix(marker) {
            return Some(("•".to_string(), item.trim_start()));
        }
    }
    let digits = line
        .bytes()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if digits > 0 && line[digits..].starts_with(". ") {
        return Some((
            format!("{}. ", &line[..digits]),
            line[digits + 2..].trim_start(),
        ));
    }
    None
}

/// Découpe le texte en sections colorées : gras, italique, code et liens. Un
/// marqueur sans fermeture reste du texte, ce qui évite de manger une étoile
/// isolée dans une phrase.
fn markdown_inline_job(
    text: &str,
    size: f32,
    color: egui::Color32,
    width: f32,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = width.max(40.0);
    let mut plain = String::new();
    let mut index = 0;
    while index < text.len() {
        let rest = &text[index..];
        if let Some((open, close, style)) = markdown_inline_marker(rest) {
            if let Some(relative) = rest[open.len()..].find(close) {
                let end = index + open.len() + relative;
                if !plain.is_empty() {
                    append_markdown_span(&mut job, &plain, None, size, color);
                    plain.clear();
                }
                append_markdown_span(
                    &mut job,
                    &text[index + open.len()..end],
                    Some(style),
                    size,
                    color,
                );
                index = end + close.len();
                continue;
            }
        }
        if rest.starts_with('[') {
            if let Some(mid) = rest.find("](") {
                if let Some(relative) = rest[mid + 2..].find(')') {
                    let end = index + mid + 2 + relative;
                    let label = &text[index + 1..index + mid];
                    let url = &text[index + mid + 2..end];
                    if !plain.is_empty() {
                        append_markdown_span(&mut job, &plain, None, size, color);
                        plain.clear();
                    }
                    let shown = if label.trim().is_empty() { url } else { label };
                    append_markdown_span(&mut job, shown, Some(MarkdownInline::Link), size, color);
                    index = end + 1;
                    continue;
                }
            }
        }
        let character = rest.chars().next().unwrap_or_default();
        plain.push(character);
        index += character.len_utf8();
    }
    if !plain.is_empty() {
        append_markdown_span(&mut job, &plain, None, size, color);
    }
    job
}

fn markdown_inline_marker(rest: &str) -> Option<(&'static str, &'static str, MarkdownInline)> {
    if rest.starts_with("**") {
        Some(("**", "**", MarkdownInline::Bold))
    } else if rest.starts_with('`') {
        Some(("`", "`", MarkdownInline::Code))
    } else if rest.starts_with('*') {
        Some(("*", "*", MarkdownInline::Italic))
    } else {
        None
    }
}

fn append_markdown_span(
    job: &mut egui::text::LayoutJob,
    text: &str,
    style: Option<MarkdownInline>,
    size: f32,
    color: egui::Color32,
) {
    if text.is_empty() {
        return;
    }
    let mut format = egui::TextFormat::default();
    format.font_id = egui::FontId::proportional(size);
    format.color = color;
    match style {
        Some(MarkdownInline::Bold) => {
            format.font_id = display_font(size + 0.2);
            format.color = TEXT();
        }
        Some(MarkdownInline::Italic) => format.italics = true,
        Some(MarkdownInline::Code) => {
            format.font_id = egui::FontId::monospace((size - 0.5).max(9.0));
            format.color = TEXT();
            format.background = PANEL();
        }
        Some(MarkdownInline::Link) => {
            format.color = ACCENT();
            format.underline = egui::Stroke::new(1.0_f32, ACCENT());
        }
        None => {}
    }
    job.append(text, 0.0, format);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_inline_strips_markers_and_keeps_styles() {
        let job = markdown_inline_job("**Crash** corrigé et `code`", 12.5, TEXT(), 300.0);
        assert_eq!(job.text, "Crash corrigé et code");
        assert!(
            job.sections
                .iter()
                .any(|section| section.format.font_id.family == display_family()),
            "le gras doit utiliser la police d'affichage"
        );
        assert!(
            job.sections
                .iter()
                .any(|section| section.format.font_id.family == egui::FontFamily::Monospace),
            "le code doit utiliser la police monospace"
        );
    }

    #[test]
    fn markdown_list_items_keep_their_number() {
        assert_eq!(
            markdown_list_item("1. Corrige le crash"),
            Some(("1. ".to_string(), "Corrige le crash"))
        );
        assert_eq!(
            markdown_list_item("- **Équilibrage**"),
            Some(("•".to_string(), "**Équilibrage**"))
        );
    }
}
