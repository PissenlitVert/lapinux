//! Réglages et aperçu du HUD de performance, dans l'onglet « En jeu ».

use super::*;

const HUD_POSITIONS: [(&str, &str); 9] = [
    ("top-left", "settings.captures.top_left"),
    ("top-center", "settings.captures.top_center"),
    ("top-right", "settings.captures.top_right"),
    ("middle-left", "settings.captures.middle_left"),
    ("middle-center", "settings.captures.center"),
    ("middle-right", "settings.captures.middle_right"),
    ("bottom-left", "settings.captures.bottom_left"),
    ("bottom-center", "settings.captures.bottom_center"),
    ("bottom-right", "settings.captures.bottom_right"),
];

fn t(key: &'static str) -> &'static str {
    lapinux_core::i18n::t(key)
}

fn hud_position_label(value: &str) -> &'static str {
    HUD_POSITIONS
        .iter()
        .find(|(position, _)| *position == value)
        .map(|(_, label)| t(label))
        .unwrap_or("—")
}

/// Petite courbe de frametime et barres de cœurs, dessinées dans la maquette
/// avec les mêmes couleurs que la plaque réelle.
fn preview_graph(painter: &egui::Painter, rect: egui::Rect, accent: egui::Color32, scale: f32) {
    let samples = [0.55, 0.42, 0.66, 0.35, 0.52, 0.28, 0.46, 0.38, 0.6, 0.3];
    let points: Vec<egui::Pos2> = samples
        .iter()
        .enumerate()
        .map(|(index, value)| {
            egui::pos2(
                rect.left() + rect.width() * index as f32 / (samples.len() - 1) as f32,
                rect.bottom() - rect.height() * value,
            )
        })
        .collect();
    let mut area = points.clone();
    area.push(egui::pos2(rect.right(), rect.bottom()));
    area.push(egui::pos2(rect.left(), rect.bottom()));
    painter.add(egui::Shape::convex_polygon(
        area,
        egui::Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 36),
        egui::Stroke::NONE,
    ));
    painter.add(egui::Shape::line(
        points,
        egui::Stroke::new((1.0 * scale).max(0.6), accent),
    ));
}

fn preview_cores(painter: &egui::Painter, rect: egui::Rect, accent: egui::Color32, scale: f32) {
    let loads = [0.35, 0.62, 0.48, 0.78, 0.28, 0.55, 0.7, 0.4];
    let gap = (2.0 * scale).max(1.0);
    let bar_width = ((rect.width() - gap * 7.0) / 8.0).max(1.0);
    for (index, load) in loads.iter().enumerate() {
        let left = rect.left() + index as f32 * (bar_width + gap);
        let bottom = rect.bottom();
        let top = bottom - rect.height() * load;
        let color = if *load > 0.8 {
            egui::Color32::from_rgb(248, 113, 113)
        } else if *load > 0.6 {
            egui::Color32::from_rgb(251, 191, 36)
        } else {
            accent
        };
        painter.rect_filled(
            egui::Rect::from_min_max(egui::pos2(left, top), egui::pos2(left + bar_width, bottom)),
            1.0,
            color,
        );
    }
}

fn preview_badge(
    painter: &egui::Painter,
    area: egui::Rect,
    label: &str,
    position: &str,
    scale: f32,
    colors: &lapinux_core::hud::HudColors,
) {
    let scale = scale.clamp(0.65, 2.0);
    let font = (7.0 * scale).max(5.0);
    let label = label.trim();
    let text = if label.is_empty() {
        "00:42:12".to_string()
    } else {
        format!("{label}  00:42:12")
    };
    let text_color = color32_of(&colors.badge_text, egui::Color32::from_rgb(254, 226, 226));
    let panel = color32_of(&colors.badge_panel, egui::Color32::from_black_alpha(220));
    let accent = color32_of(&colors.badge_accent, egui::Color32::from_rgb(248, 113, 113));
    let galley = painter.layout_no_wrap(text, egui::FontId::proportional(font), text_color);
    let pad = 8.0 * scale;
    let dot = 3.0 * scale;
    let gap = 5.0 * scale;
    let height = (font + 8.0 * scale).max(16.0 * scale);
    let size = egui::vec2(2.0 * pad + 2.0 * dot + gap + galley.size().x, height);
    let origin = hud_origin(area, size, position);
    let rect = egui::Rect::from_min_size(origin, size);
    painter.rect_filled(rect, height / 2.0, panel);
    painter.rect_stroke(
        rect,
        height / 2.0,
        egui::Stroke::new(
            1.0_f32,
            egui::Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 140),
        ),
    );
    painter.circle_filled(
        egui::pos2(rect.left() + pad + dot, rect.center().y),
        dot,
        accent,
    );
    painter.galley(
        egui::pos2(
            rect.left() + pad + 2.0 * dot + gap,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        text_color,
    );
}

/// Couleur d'un réglage hexadécimal, avec un repli lisible si la valeur est
/// vide ou ancienne.
fn color32_of(value: &str, fallback: egui::Color32) -> egui::Color32 {
    lapinux_core::hud::color_rgba(value)
        .map(|[r, g, b, a]| egui::Color32::from_rgba_unmultiplied(r, g, b, a))
        .unwrap_or(fallback)
}

/// Position de la plaque dans l'aperçu, selon les neuf ancrages.
fn hud_origin(area: egui::Rect, size: egui::Vec2, position: &str) -> egui::Pos2 {
    let margin = 8.0;
    let x = match position {
        "top-center" | "middle-center" | "bottom-center" => area.center().x - size.x / 2.0,
        "top-right" | "middle-right" | "bottom-right" => area.right() - size.x - margin,
        _ => area.left() + margin,
    };
    let y = match position {
        "middle-left" | "middle-center" | "middle-right" => area.center().y - size.y / 2.0,
        "bottom-left" | "bottom-center" | "bottom-right" => area.bottom() - size.y - margin,
        _ => area.top() + margin,
    };
    egui::pos2(x, y)
}

/// Grille 3 × 3 des ancrages, partagée par la plaque et le badge.
fn hud_positions(ui: &mut egui::Ui, value: &mut String, id_salt: &'static str) -> bool {
    let mut changed = false;
    // Une grille dans un `settings_row` en right-to-left étirait la largeur
    // disponible à l'infini. Une liste déroulante reste bornée et tient dans
    // la ligne.
    egui::ComboBox::from_id_salt(id_salt)
        .width(160.0)
        .selected_text(hud_position_label(value))
        .show_ui(ui, |ui| {
            for position in HUD_POSITIONS.iter().map(|(position, _)| *position) {
                if ui
                    .selectable_label(value == position, hud_position_label(position))
                    .clicked()
                {
                    *value = position.to_string();
                    changed = true;
                }
            }
        });
    changed
}

/// Éditeur d'une couleur : pipette, valeur hexadécimale et palette rapide.
fn hud_color_editor(
    ui: &mut egui::Ui,
    colors: &mut lapinux_core::hud::HudColors,
    slot: lapinux_core::hud::HudColorSlot,
    label: &str,
    recent: &mut Vec<String>,
) -> bool {
    let mut changed = false;
    let mut value = colors.get(slot).to_string();
    let mut color = color32_of(&value, egui::Color32::WHITE);
    settings_row(ui, label, "", |ui| {
        ui.horizontal(|ui| {
            if ui.color_edit_button_srgba(&mut color).changed() {
                value = lapinux_core::hud::hex_rgba([color.r(), color.g(), color.b(), color.a()]);
                if colors.set(slot, &value) {
                    changed = true;
                }
            }
            let edit = ui.add(
                egui::TextEdit::singleline(&mut value)
                    .desired_width(86.0)
                    .font(egui::TextStyle::Monospace),
            );
            if edit.lost_focus() {
                if let Some(normalized) = lapinux_core::hud::normalize_hex(&value) {
                    if colors.set(slot, &normalized) {
                        remember_color_in(recent, &normalized);
                        changed = true;
                    }
                }
            }
        });
    });
    ui.horizontal_wrapped(|ui| {
        for quick in lapinux_core::hud::QUICK_COLORS {
            let color = color32_of(quick, egui::Color32::WHITE);
            if ui
                .add(
                    egui::Button::new(" ")
                        .fill(color)
                        .min_size(egui::vec2(16.0, 16.0)),
                )
                .on_hover_text(quick)
                .clicked()
            {
                if colors.set(slot, quick) {
                    remember_color_in(recent, quick);
                    changed = true;
                }
            }
        }
    });
    changed
}

fn remember_color_in(recent: &mut Vec<String>, value: &str) {
    if let Some(value) = lapinux_core::hud::normalize_hex(value) {
        recent.retain(|old| *old != value);
        recent.insert(0, value);
        recent.truncate(8);
    }
}

/// Valeur d'exemple d'un élément, avec sa précision et son unité réelles.
fn preview_value(
    kind: lapinux_core::hud::HudElement,
    element: &lapinux_core::hud::HudElementStyle,
) -> String {
    use lapinux_core::hud::HudElement;
    let precision = element.precision.min(2) as usize;
    match kind {
        HudElement::Fps => format!("{:.*}", precision, 144.0),
        HudElement::Frametime => format!("{:.*} ms", precision, 6.9),
        HudElement::Gpu => format!("{:.*} %", precision, 72.0),
        HudElement::GpuTemp => {
            if element.unit == "fahrenheit" {
                format!("{:.*} °F", precision, 142.0)
            } else {
                format!("{:.*} °C", precision, 61.0)
            }
        }
        HudElement::Vram | HudElement::Ram => {
            if element.unit == "percent" {
                format!("{:.*} %", precision, 57.0)
            } else {
                format!("{:.*} GiB", precision, 6.8)
            }
        }
        HudElement::Cpu => format!("{:.*} %", precision, 38.0),
        HudElement::CpuTemp => {
            if element.unit == "fahrenheit" {
                format!("{:.*} °F", precision, 129.0)
            } else {
                format!("{:.*} °C", precision, 54.0)
            }
        }
        HudElement::Cores => t("settings.in_game.hud_cores_value").into(),
        HudElement::Graph => "frametime".into(),
    }
}

/// Libellé réel d'un élément, identique à celui du HUD natif : les cartes
/// CPU et GPU portent le nom du matériel, pas un mot générique.
fn preview_label(
    kind: lapinux_core::hud::HudElement,
    style: &lapinux_core::hud::HudStyle,
) -> String {
    use lapinux_core::hud::HudElement;
    match kind {
        HudElement::Gpu => {
            let custom = style.gpu_label.trim();
            if custom.is_empty() {
                lapinux_perf::labels::gpu_label().unwrap_or_else(|| "GPU".into())
            } else {
                custom.to_string()
            }
        }
        HudElement::Cpu => {
            let custom = style.cpu_label.trim();
            if custom.is_empty() {
                lapinux_perf::labels::cpu_label().unwrap_or_else(|| "CPU".into())
            } else {
                custom.to_string()
            }
        }
        _ => kind.label().to_string(),
    }
}

/// Texte de la ligne compacte, avec exactement les mêmes mots que le HUD
/// natif : pas de « FrameTime » ni de « Temp. GPU » que le jeu n'affiche pas.
fn preview_inline_text(
    kind: lapinux_core::hud::HudElement,
    element: &lapinux_core::hud::HudElementStyle,
    style: &lapinux_core::hud::HudStyle,
) -> String {
    use lapinux_core::hud::HudElement;
    let precision = element.precision.min(2) as usize;
    match kind {
        HudElement::Fps => format!("{:.*} FPS", precision, 144.0),
        HudElement::Frametime => format!("{:.*} ms", precision, 6.9),
        HudElement::Gpu => format!("{} {:.*}%", preview_label(kind, style), precision, 72.0),
        HudElement::GpuTemp => {
            if element.unit == "fahrenheit" {
                format!("{:.*} °F", precision, 142.0)
            } else {
                format!("{:.*} °C", precision, 61.0)
            }
        }
        HudElement::Vram => format!("VRAM {}", preview_memory(element, 6.8)),
        HudElement::Cpu => format!("{} {:.*}%", preview_label(kind, style), precision, 38.0),
        HudElement::CpuTemp => {
            if element.unit == "fahrenheit" {
                format!("{:.*} °F", precision, 129.0)
            } else {
                format!("{:.*} °C", precision, 54.0)
            }
        }
        HudElement::Ram => format!("RAM {}", preview_memory(element, 6.8)),
        HudElement::Cores | HudElement::Graph => kind.label().to_string(),
    }
}

fn preview_memory(element: &lapinux_core::hud::HudElementStyle, gib: f32) -> String {
    if element.unit == "percent" {
        format!("{:.*} %", element.precision.min(2) as usize, 57.0)
    } else {
        format!("{:.*} GiB", element.precision.min(2) as usize, gib)
    }
}

fn preview_color(
    kind: lapinux_core::hud::HudElement,
    colors: &lapinux_core::hud::HudColors,
) -> egui::Color32 {
    use lapinux_core::hud::{HudColorSlot, HudElement};
    let slot = match kind {
        HudElement::Fps => HudColorSlot::Fps,
        HudElement::Frametime => HudColorSlot::Frametime,
        HudElement::Gpu => HudColorSlot::Gpu,
        HudElement::GpuTemp | HudElement::CpuTemp => HudColorSlot::Temp,
        HudElement::Vram => HudColorSlot::Vram,
        HudElement::Cpu => HudColorSlot::Cpu,
        HudElement::Ram => HudColorSlot::Ram,
        HudElement::Cores => HudColorSlot::Cores,
        HudElement::Graph => HudColorSlot::Graph,
    };
    color32_of(colors.get(slot), egui::Color32::WHITE)
}

/// Aperçu réel du HUD nommé : ordre, unités, couleurs, courbes et badge.
fn style_preview(ui: &mut egui::Ui, style: &lapinux_core::hud::HudStyle, height: f32) {
    let width = ui.available_width().clamp(120.0, 430.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 8.0, egui::Color32::from_rgb(25, 35, 42));
    painter.rect_filled(
        egui::Rect::from_min_max(
            rect.min,
            egui::pos2(rect.right(), rect.top() + height * 0.56),
        ),
        8.0,
        egui::Color32::from_rgb(43, 63, 60),
    );
    let area = rect.shrink(6.0);
    let scale = style.scale.clamp(0.75, 2.0);
    let inline = style.layout == "inline";
    let pad_x = if inline { 8.0 * scale } else { 7.0 * scale };
    let pad_y = if inline { 4.0 * scale } else { 7.0 * scale };
    let pad = pad_x;
    let row_h = 12.5 * scale;
    let rows: Vec<_> = style
        .elements
        .iter()
        .filter(|element| element.enabled && !element.kind.is_chart())
        .collect();
    let charts: Vec<_> = style
        .elements
        .iter()
        .filter(|element| element.enabled && element.kind.is_chart())
        .collect();
    let has_cores = charts
        .iter()
        .any(|element| element.kind == lapinux_core::hud::HudElement::Cores);
    let has_graph = charts
        .iter()
        .any(|element| element.kind == lapinux_core::hud::HudElement::Graph);
    let inline_texts: Vec<(String, egui::Color32)> = rows
        .iter()
        .map(|element| {
            (
                preview_inline_text(element.kind, element, style),
                preview_color(element.kind, &style.colors),
            )
        })
        .collect();
    let max_line_w = (area.width() - 2.0 * pad_x).max(1.0);
    let base_font = 7.0 * scale;
    let gap = 3.0 * scale;
    let base_content: f32 = inline_texts
        .iter()
        .map(|(text, color)| {
            painter
                .layout_no_wrap(text.clone(), egui::FontId::monospace(base_font), *color)
                .size()
                .x
        })
        .sum::<f32>()
        + inline_texts.len().saturating_sub(1) as f32 * gap;
    let fit = (max_line_w / base_content.max(1.0)).min(1.0);
    let inline_font = (base_font * fit).max(3.0);
    let inline_gap = gap * fit;
    let inline_line_h = inline_font + 3.0 * scale * fit;
    let inline_content_w: f32 = inline_texts
        .iter()
        .map(|(text, color)| {
            painter
                .layout_no_wrap(text.clone(), egui::FontId::monospace(inline_font), *color)
                .size()
                .x
        })
        .sum::<f32>()
        + inline_texts.len().saturating_sub(1) as f32 * inline_gap;
    let chart_h = 26.0 * scale;
    let charts_h = if charts.is_empty() {
        0.0
    } else {
        charts.len() as f32 * chart_h + (charts.len() - 1) as f32 * 3.0 * scale
    };
    let block_w = rows
        .iter()
        .map(|element| {
            let label = painter
                .layout_no_wrap(
                    preview_label(element.kind, style),
                    egui::FontId::proportional(6.8 * scale),
                    color32_of(&style.colors.label, egui::Color32::from_rgb(148, 163, 184)),
                )
                .size()
                .x;
            let value = painter
                .layout_no_wrap(
                    preview_value(element.kind, element),
                    egui::FontId::monospace(7.4 * scale),
                    preview_color(element.kind, &style.colors),
                )
                .size()
                .x;
            label + value + 30.0 * scale
        })
        .fold(210.0 * scale, f32::max);
    let panel_w = if inline {
        (inline_content_w + 2.0 * pad_x)
            .max(if has_graph { 280.0 * scale } else { 0.0 })
            .max(140.0 * scale)
            .min(area.width())
    } else {
        block_w.min(area.width())
    };
    let rows_h = if inline {
        inline_line_h
    } else {
        rows.iter()
            .map(|element| row_h * element.scale.clamp(0.75, 1.5))
            .sum()
    };
    let inline_chart_h = if has_graph {
        26.0 * scale
    } else if has_cores {
        14.0 * scale
    } else {
        0.0
    };
    let panel_h = if inline {
        2.0 * pad_y
            + inline_line_h
            + if inline_chart_h > 0.0 {
                inline_chart_h + 4.0 * scale
            } else {
                0.0
            }
    } else {
        2.0 * pad_y + rows_h + charts_h
    };
    let origin = hud_origin(area, egui::vec2(panel_w, panel_h), &style.position);
    let panel = egui::Rect::from_min_size(origin, egui::vec2(panel_w, panel_h));
    painter.rect_filled(
        panel,
        6.0,
        color32_of(&style.colors.panel, egui::Color32::from_black_alpha(208)),
    );
    painter.rect_stroke(
        panel,
        6.0,
        egui::Stroke::new(
            1.0_f32,
            color32_of(&style.colors.border, egui::Color32::from_white_alpha(26)),
        ),
    );
    let left = panel.left() + pad_x;
    let right = panel.right() - pad_x;
    let label = color32_of(&style.colors.label, egui::Color32::from_rgb(148, 163, 184));
    if inline {
        let mut x = left;
        for (text, color) in &inline_texts {
            let galley =
                painter.layout_no_wrap(text.clone(), egui::FontId::monospace(inline_font), *color);
            painter.galley(egui::pos2(x, panel.top() + pad_y), galley.clone(), label);
            x += galley.size().x + inline_gap;
        }
        let top = panel.top() + pad_y + inline_line_h + 2.0 * scale;
        let inner_w = (right - left).max(1.0);
        match (has_cores, has_graph) {
            (true, true) => {
                let half = (inner_w - 12.0 * scale) / 2.0;
                preview_cores(
                    painter,
                    egui::Rect::from_min_size(
                        egui::pos2(left, top),
                        egui::vec2(half.max(1.0), 12.0 * scale),
                    ),
                    color32_of(&style.colors.cores, egui::Color32::from_rgb(96, 165, 250)),
                    scale,
                );
                preview_graph(
                    painter,
                    egui::Rect::from_min_size(
                        egui::pos2(left + half + 12.0 * scale, top),
                        egui::vec2(half.max(1.0), 26.0 * scale),
                    ),
                    color32_of(&style.colors.graph, egui::Color32::from_rgb(56, 189, 248)),
                    scale,
                );
            }
            (true, false) => preview_cores(
                painter,
                egui::Rect::from_min_size(egui::pos2(left, top), egui::vec2(inner_w, 14.0 * scale)),
                color32_of(&style.colors.cores, egui::Color32::from_rgb(96, 165, 250)),
                scale,
            ),
            (false, true) => preview_graph(
                painter,
                egui::Rect::from_min_size(egui::pos2(left, top), egui::vec2(inner_w, 26.0 * scale)),
                color32_of(&style.colors.graph, egui::Color32::from_rgb(56, 189, 248)),
                scale,
            ),
            (false, false) => {}
        }
    } else {
        let mut y = panel.top() + pad;
        for element in &rows {
            let color = preview_color(element.kind, &style.colors);
            painter.text(
                egui::pos2(left, y + row_h * 0.5),
                egui::Align2::LEFT_CENTER,
                preview_label(element.kind, style),
                egui::FontId::proportional(6.8 * scale),
                label,
            );
            painter.text(
                egui::pos2(right, y + row_h * 0.5),
                egui::Align2::RIGHT_CENTER,
                preview_value(element.kind, element),
                egui::FontId::monospace(7.4 * scale),
                color,
            );
            y += row_h * element.scale.clamp(0.75, 1.5);
        }
    }
    let mut charts_top = panel.top() + pad_y + rows_h + 3.0 * scale;
    for element in charts.iter().filter(|_| !inline) {
        let inner = egui::Rect::from_min_size(
            egui::pos2(left, charts_top),
            egui::vec2((right - left).max(1.0), chart_h),
        );
        match element.kind {
            lapinux_core::hud::HudElement::Cores => preview_cores(
                painter,
                inner,
                color32_of(&style.colors.cores, egui::Color32::from_rgb(96, 165, 250)),
                scale,
            ),
            lapinux_core::hud::HudElement::Graph => preview_graph(
                painter,
                inner,
                color32_of(&style.colors.graph, egui::Color32::from_rgb(56, 189, 248)),
                scale,
            ),
            _ => {}
        }
        charts_top += chart_h + 3.0 * scale;
    }
}

fn hud_style_select(ui: &mut egui::Ui, config: &mut Config) {
    let current = config.capture.active_hud_style();
    egui::ComboBox::from_id_salt("hud-style-select")
        .width(180.0)
        .selected_text(current.name)
        .show_ui(ui, |ui| {
            for preset in ["simple", "essential", "full"] {
                let style = lapinux_core::hud::HudStyle::builtin(preset);
                if ui
                    .selectable_label(config.capture.hud_active == style.id, style.name)
                    .clicked()
                {
                    config.capture.hud_active = style.id;
                }
            }
            for style in &config.capture.hud_custom {
                if ui
                    .selectable_label(config.capture.hud_active == style.id, &style.name)
                    .clicked()
                {
                    config.capture.hud_active = style.id.clone();
                }
            }
        });
}

/// Carte complète du HUD natif : chaque élément, ses couleurs, son ordre,
/// ses unités et les HUD nommés.
pub(super) fn hud_settings_card(ui: &mut egui::Ui, config: &mut Config) {
    settings_card(
        ui,
        t("ui.capture_hud"),
        t("settings.in_game.hud_desc"),
        |ui| {
            settings_row(
                ui,
                t("settings.in_game.hud_visible"),
                t("settings.in_game.hud_visible_hint"),
                |ui| {
                    switch(ui, &mut config.capture.hud_on_screen);
                },
            );
            ui.add_space(6.0);
            // La sélection du HUD et ses actions forment une seule ligne
            // alignée à droite. Le layout est inversé : les boutons ajoutés
            // en premier occupent le bord droit, le sélecteur reste à leur
            // gauche, dans l'ordre visuel Nouveau / Dupliquer / Supprimer.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.set_min_height(32.0);
                if ui.button(t("settings.in_game.hud_delete")).clicked() {
                    let id = config.capture.hud_active.clone();
                    config.capture.remove_hud_style(&id);
                }
                if ui.button(t("settings.in_game.hud_duplicate")).clicked() {
                    let name = config.capture.active_hud_style().name;
                    config.capture.duplicate_hud_style(&format!(
                        "{name} {}",
                        t("settings.in_game.hud_copy_suffix")
                    ));
                }
                if ui
                    .button(t("settings.in_game.hud_create"))
                    .on_hover_text(t("settings.in_game.hud_create_hint"))
                    .clicked()
                {
                    config
                        .capture
                        .duplicate_hud_style(&t("settings.in_game.hud_new_name"));
                }
                hud_style_select(ui, config);
            });

            let mut style = config.capture.active_hud_style();
            let mut changed = false;
            ui.add_space(6.0);
            settings_row(ui, t("settings.in_game.hud_name"), "", |ui| {
                if ui
                    .add(egui::TextEdit::singleline(&mut style.name).desired_width(220.0))
                    .changed()
                {
                    changed = true;
                }
            });
            settings_row(
                ui,
                t("settings.in_game.hud_cpu_label"),
                t("settings.in_game.hud_cpu_label_hint"),
                |ui| {
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut style.cpu_label)
                                .desired_width(220.0)
                                .char_limit(40),
                        )
                        .changed()
                    {
                        changed = true;
                    }
                },
            );
            settings_row(
                ui,
                t("settings.in_game.hud_gpu_label"),
                t("settings.in_game.hud_gpu_label_hint"),
                |ui| {
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut style.gpu_label)
                                .desired_width(220.0)
                                .char_limit(40),
                        )
                        .changed()
                    {
                        changed = true;
                    }
                },
            );
            // Même principe : les thèmes sont ajoutés à l'envers pour que
            // « Classique » reste le premier à gauche dans le layout inversé.
            settings_row(ui, t("settings.in_game.hud_theme"), "", |ui| {
                for (id, label) in lapinux_core::hud::HUD_THEMES.into_iter().rev() {
                    if ui.button(label).clicked() {
                        style.apply_theme(id);
                        changed = true;
                    }
                }
            });
            ui.add_space(8.0);
            let row_h = 12.5 * style.scale;
            let rows_h = if style.layout == "inline" {
                row_h
            } else {
                style
                    .elements
                    .iter()
                    .filter(|element| element.enabled && !element.kind.is_chart())
                    .map(|element| row_h * element.scale.clamp(0.75, 1.5))
                    .sum()
            };
            let chart_count = style
                .elements
                .iter()
                .filter(|element| element.enabled && element.kind.is_chart())
                .count() as f32;
            let charts_h = if chart_count == 0.0 {
                0.0
            } else {
                chart_count * 26.0 * style.scale + (chart_count - 1.0) * 3.0 * style.scale
            };
            let preview_height =
                (20.0 + 14.0 * style.scale + rows_h + charts_h).clamp(150.0, 520.0);
            style_preview(ui, &style, preview_height);
            settings_row(ui, t("settings.in_game.hud_position"), "", |ui| {
                if hud_positions(ui, &mut style.position, "performance-hud-position") {
                    changed = true;
                }
            });
            settings_row(ui, t("settings.in_game.hud_layout"), "", |ui| {
                egui::ComboBox::from_id_salt("native-hud-layout")
                    .width(180.0)
                    .selected_text(if style.layout == "inline" {
                        t("settings.in_game.hud_layout_inline")
                    } else {
                        t("settings.in_game.hud_layout_block")
                    })
                    .show_ui(ui, |ui| {
                        for (value, label) in [
                            ("block", t("settings.in_game.hud_layout_block")),
                            ("inline", t("settings.in_game.hud_layout_inline")),
                        ] {
                            ui.selectable_value(&mut style.layout, value.to_string(), label);
                        }
                    });
            });
            settings_row(ui, t("settings.captures.hud_plate_size"), "", |ui| {
                if settings_slider(ui, &mut style.scale, 0.75..=2.0, " ×").changed() {
                    changed = true;
                }
            });
            let mut recent = config.capture.hud_recent_colors.clone();
            egui::CollapsingHeader::new(t("settings.in_game.hud_colors"))
                .default_open(true)
                .show(ui, |ui| {
                    for slot in lapinux_core::hud::HudColorSlot::ALL {
                        if slot.wire().starts_with("badge") {
                            continue;
                        }
                        if hud_color_editor(ui, &mut style.colors, slot, slot.label(), &mut recent)
                        {
                            changed = true;
                        }
                    }
                });
            config.capture.hud_recent_colors = recent;
            egui::CollapsingHeader::new(t("settings.in_game.hud_elements_order"))
                .default_open(true)
                .show(ui, |ui| {
                    let mut move_up = None;
                    let mut move_down = None;
                    for index in 0..style.elements.len() {
                        let kind = style.elements[index].kind;
                        ui.horizontal(|ui| {
                            let element = &mut style.elements[index];
                            if switch(ui, &mut element.enabled).changed() {
                                changed = true;
                            }
                            // Colonnes fixes : le libellé, la précision, l'unité
                            // et les flèches restent alignés d'une ligne à
                            // l'autre, au lieu de danser selon le contenu.
                            ui.add_sized(
                                egui::vec2(120.0, 24.0),
                                egui::Label::new(
                                    egui::RichText::new(kind.label()).size(11.5).color(TEXT()),
                                )
                                .truncate(),
                            );
                            ui.allocate_ui_with_layout(
                                egui::vec2(200.0, 38.0),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    ui.set_min_width(200.0);
                                    ui.set_max_width(200.0);
                                    ui.set_min_height(38.0);
                                    if kind.has_precision() {
                                        let suffix =
                                            format!(" {}", t("settings.in_game.hud_decimals"));
                                        if settings_slider(
                                            ui,
                                            &mut element.precision,
                                            0..=2,
                                            &suffix,
                                        )
                                        .changed()
                                        {
                                            changed = true;
                                        }
                                    }
                                },
                            );
                            let units = kind.units();
                            ui.allocate_ui_with_layout(
                                egui::vec2(105.0, 24.0),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    ui.set_min_width(105.0);
                                    ui.set_max_width(105.0);
                                    if !units.is_empty() {
                                        egui::ComboBox::from_id_salt((
                                            "hud-element-unit",
                                            kind.label(),
                                            index,
                                        ))
                                        .width(100.0)
                                        .selected_text(&element.unit)
                                        .show_ui(
                                            ui,
                                            |ui| {
                                                for unit in units {
                                                    ui.selectable_value(
                                                        &mut element.unit,
                                                        (*unit).to_string(),
                                                        *unit,
                                                    );
                                                }
                                            },
                                        );
                                    }
                                },
                            );
                            ui.allocate_ui_with_layout(
                                egui::vec2(58.0, 24.0),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    ui.set_min_width(58.0);
                                    ui.set_max_width(58.0);
                                    if ui.small_button("↑").clicked() {
                                        move_up = Some(index);
                                    }
                                    if ui.small_button("↓").clicked() {
                                        move_down = Some(index);
                                    }
                                },
                            );
                        });
                    }
                    if let Some(index) = move_up {
                        if index > 0 {
                            style.elements.swap(index, index - 1);
                            changed = true;
                        }
                    }
                    if let Some(index) = move_down {
                        if index + 1 < style.elements.len() {
                            style.elements.swap(index, index + 1);
                            changed = true;
                        }
                    }
                });
            if changed {
                config.capture.set_active_hud_style(style);
            }
        },
    );
}

/// Carte du HUD d'enregistrement.
///
/// C'est une pastille de statut, pas une seconde plaque de performance : elle
/// n'a ni FPS ni températures, seulement REC/PAUSE et le temps écoulé. Elle
/// possède en revanche sa place, sa taille et ses trois couleurs, réglables
/// ici sans passer par l'éditeur de la plaque.
pub(super) fn record_hud_settings_card(ui: &mut egui::Ui, config: &mut Config) {
    settings_card(
        ui,
        t("ui.capture_record_hud"),
        t("ui.capture_record_hint"),
        |ui| {
            let mut style = config.capture.active_hud_style();
            let mut changed = false;
            settings_row(
                ui,
                t("settings.captures.badge_show"),
                t("settings.captures.record_hud_switch_hint"),
                |ui| {
                    if switch(ui, &mut style.badge.enabled).changed() {
                        changed = true;
                    }
                },
            );
            if style.badge.enabled {
                settings_row(
                    ui,
                    t("settings.captures.record_hud_label"),
                    t("settings.captures.record_hud_label_hint"),
                    |ui| {
                        if ui
                            .add(
                                egui::TextEdit::singleline(&mut style.badge.label)
                                    .desired_width(150.0)
                                    .char_limit(16),
                            )
                            .changed()
                        {
                            changed = true;
                        }
                    },
                );
                settings_row(
                    ui,
                    t("settings.captures.badge_position"),
                    t("settings.captures.record_hud_position_hint"),
                    |ui| {
                        if hud_positions(ui, &mut style.badge.position, "record-hud-position") {
                            changed = true;
                        }
                    },
                );
                settings_row(
                    ui,
                    t("settings.captures.badge_size"),
                    t("settings.captures.record_hud_size_hint"),
                    |ui| {
                        if settings_slider(ui, &mut style.badge.scale, 0.65..=2.0, " ×").changed()
                        {
                            changed = true;
                        }
                    },
                );
                ui.add_space(4.0);
                record_badge_preview(ui, &style);
                ui.add_space(4.0);
                muted(ui, t("settings.captures.record_hud_explanation"));
                egui::CollapsingHeader::new(t("settings.captures.record_hud_colors"))
                    .default_open(true)
                    .show(ui, |ui| {
                        let mut recent = config.capture.hud_recent_colors.clone();
                        for (slot, label) in [
                            (
                                lapinux_core::hud::HudColorSlot::BadgeText,
                                t("settings.captures.record_hud_color_text"),
                            ),
                            (
                                lapinux_core::hud::HudColorSlot::BadgePanel,
                                t("settings.captures.record_hud_color_panel"),
                            ),
                            (
                                lapinux_core::hud::HudColorSlot::BadgeAccent,
                                t("settings.captures.record_hud_color_accent"),
                            ),
                        ] {
                            if hud_color_editor(ui, &mut style.colors, slot, label, &mut recent) {
                                changed = true;
                            }
                        }
                        config.capture.hud_recent_colors = recent;
                    });
            } else {
                ui.add_space(6.0);
                muted(ui, t("settings.captures.record_hud_disabled"));
            }
            if changed {
                // Le style actif est la source de vérité ; `sync_hud_legacy`
                // remet aussi les champs historiques à jour pour le lancement
                // et les configurations existantes.
                config.capture.set_active_hud_style(style);
            }
        },
    );
}

/// Maquette du HUD d'enregistrement, dessinée avec les couleurs choisies.
fn record_badge_preview(ui: &mut egui::Ui, style: &lapinux_core::hud::HudStyle) {
    let width = ui.available_width().clamp(120.0, 430.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 76.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 8.0, egui::Color32::from_rgb(25, 35, 42));
    painter.rect_filled(
        egui::Rect::from_min_max(
            rect.min,
            egui::pos2(rect.right(), rect.top() + rect.height() * 0.56),
        ),
        8.0,
        egui::Color32::from_rgb(43, 63, 60),
    );
    preview_badge(
        painter,
        rect.shrink(6.0),
        &style.badge.label,
        &style.badge.position,
        style.badge.scale,
        &style.colors,
    );
}
