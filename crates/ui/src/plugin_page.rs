//! Pages fournies par les plugins.
//!
//! Une page native est entièrement décrite par son `.so` : Lapinux lui donne
//! la taille disponible et le temps, puis peint les commandes JSON génériques
//! qu'il renvoie. Aucune apparence de plugin n'est codée ici.

use super::*;
use lapinux_plugin_api::{PageCommand, PageTextAlign};

impl LapinuxApp {
    pub(super) fn plugin_page(&mut self, ui: &mut egui::Ui) {
        let Some(page) = self.active_theme.plugin_page.clone() else {
            ui.centered_and_justified(|ui| {
                muted(ui, lapinux_core::i18n::t("page.plugin_missing"));
            });
            return;
        };
        let rect = ui.available_rect_before_wrap();
        let input = self.page_input(ui, rect);
        self.plugin_host.set_input(input);
        let time = ui.input(|input| input.time);
        if let Some(commands) =
            self.native_plugins
                .page_frame(&page.plugin, rect.width(), rect.height(), time)
        {
            // Une page native peut animer : elle décide elle-même de demander
            // une nouvelle image en renvoyant des commandes différentes.
            ui.ctx().request_repaint();
            // Une page peut aussi demander des jaquettes : sans ce budget,
            // elles resteraient en initiale jusqu'au prochain écran.
            self.covers.begin_frame(6);
            self.paint_plugin_commands(ui, rect.min, &commands);
            return;
        }

        // Repli sans code de plugin : les textes déclarés dans le manifeste
        // restent visibles même si la bibliothèque native n'est pas chargée.
        let title = if page.title.trim().is_empty() {
            lapinux_core::i18n::t("page.plugin")
        } else {
            page.title.as_str()
        };
        ui.painter().text(
            egui::pos2(rect.center().x, rect.top() + 34.0),
            egui::Align2::CENTER_CENTER,
            title,
            display_font(34.0),
            TEXT(),
        );
        if !page.subtitle.trim().is_empty() {
            ui.painter().text(
                egui::pos2(rect.center().x, rect.top() + 62.0),
                egui::Align2::CENTER_CENTER,
                &page.subtitle,
                display_font(12.0),
                TEXT_DIM(),
            );
        }
        if !page.footer.trim().is_empty() {
            ui.painter().text(
                egui::pos2(rect.center().x, rect.bottom() - 24.0),
                egui::Align2::CENTER_CENTER,
                &page.footer,
                display_font(18.0),
                TEXT_DIM(),
            );
        }
    }

    /// Peint une page native qui peut demander des jaquettes. Le cache de
    /// couvertures appartient à l'application ; le plugin ne fournit que
    /// l'identifiant du jeu et le rectangle à remplir.
    pub(super) fn paint_plugin_commands(
        &mut self,
        ui: &mut egui::Ui,
        origin: egui::Pos2,
        commands: &[PageCommand],
    ) {
        for command in commands {
            if let PageCommand::Cover {
                appid,
                x,
                y,
                width,
                height,
                rounding,
            } = command
            {
                self.paint_plugin_cover(ui, origin, *appid, *x, *y, *width, *height, *rounding);
            } else {
                paint_command(ui, origin, command);
            }
        }
    }

    fn paint_plugin_cover(
        &mut self,
        ui: &mut egui::Ui,
        origin: egui::Pos2,
        appid: u32,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        rounding: f32,
    ) {
        let rect = egui::Rect::from_min_size(origin + egui::vec2(x, y), egui::vec2(width, height));
        let entry = self
            .library_entries_all()
            .iter()
            .find(|entry| entry.appid == appid)
            .cloned();
        let Some(entry) = entry else {
            return;
        };
        let texture = self.covers.get(
            ui.ctx(),
            &self.paths,
            appid,
            &entry.name,
            entry.store,
            &entry.cover,
            CoverTier::Sheet,
        );
        match texture {
            Some(texture) => paint_cover_at(
                ui,
                rect,
                &texture,
                egui::Rounding::same(rounding),
                egui::vec2(0.5, 0.5),
            ),
            None => {
                let colors = self.active_theme.colors;
                ui.painter()
                    .rect_filled(rect, egui::Rounding::same(rounding), colors.card);
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    entry.name.chars().next().unwrap_or('?'),
                    display_font(34.0),
                    colors.text_dim,
                );
            }
        }
    }
}

/// Peint une liste de commandes dans la page. `origin` est le coin supérieur
/// gauche de la zone allouée au plugin.
pub(super) fn paint_page(ui: &mut egui::Ui, origin: egui::Pos2, commands: &[PageCommand]) {
    for command in commands {
        paint_command(ui, origin, command);
    }
}

fn paint_command(ui: &mut egui::Ui, origin: egui::Pos2, command: &PageCommand) {
    let painter = ui.painter();
    match command {
        PageCommand::Text {
            x,
            y,
            text,
            size,
            color,
            align,
        } => {
            let align = match align {
                PageTextAlign::Left => egui::Align2::LEFT_CENTER,
                PageTextAlign::Center => egui::Align2::CENTER_CENTER,
                PageTextAlign::Right => egui::Align2::RIGHT_CENTER,
            };
            painter.text(
                origin + egui::vec2(*x, *y),
                align,
                text,
                egui::FontId::proportional(*size),
                page_color(*color),
            );
        }
        PageCommand::Rect {
            x,
            y,
            width,
            height,
            rounding,
            color,
        } => {
            painter.rect_filled(
                egui::Rect::from_min_size(origin + egui::vec2(*x, *y), egui::vec2(*width, *height)),
                egui::Rounding::same(*rounding),
                page_color(*color),
            );
        }
        PageCommand::Circle {
            x,
            y,
            radius,
            color,
        } => {
            painter.circle_filled(origin + egui::vec2(*x, *y), *radius, page_color(*color));
        }
        PageCommand::Line {
            x1,
            y1,
            x2,
            y2,
            width,
            color,
        } => {
            painter.line_segment(
                [origin + egui::vec2(*x1, *y1), origin + egui::vec2(*x2, *y2)],
                egui::Stroke::new(*width, page_color(*color)),
            );
        }
        PageCommand::Cover { .. } => {
            // Les cadres de thème n'ont pas de jaquettes ; une page native
            // interactive les peint avec `paint_plugin_commands`.
        }
        PageCommand::NavIcon {
            page,
            x,
            y,
            size,
            color,
        } => {
            if let Some(page) = Page::from_key(page) {
                section_icons::page(
                    painter,
                    page,
                    origin + egui::vec2(*x, *y),
                    *size,
                    page_color(*color),
                );
            }
        }
    }
}

fn page_color(color: [u8; 4]) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(color[0], color[1], color[2], color[3])
}
