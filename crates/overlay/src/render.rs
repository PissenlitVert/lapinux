//! Rendu CPU de l'overlay (tiny-skia + police TTF), sans contexte GPU.

use crate::menu::{Item, MenuState};
use crate::{Error, Result};
use ab_glyph::{Font, FontVec, PxScale, ScaleFont};
use std::path::Path;
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Rect, Transform};

const PANEL_WIDTH: f32 = 620.0;
const PANEL_RADIUS: f32 = 16.0;

const BG_PANEL: (u8, u8, u8, u8) = (22, 24, 29, 242);
const BORDER: (u8, u8, u8, u8) = (44, 50, 62, 255);
const TEXT: (u8, u8, u8, u8) = (232, 236, 243, 255);
const TEXT_DIM: (u8, u8, u8, u8) = (154, 164, 178, 255);
const ACCENT: (u8, u8, u8, u8) = (110, 168, 254, 255);
const ROW_SELECTED: (u8, u8, u8, u8) = (35, 43, 57, 255);

pub struct Renderer {
    font: FontVec,
    pub width: u32,
    pub height: u32,
}

impl Renderer {
    pub fn new(font_path: &Path, width: u32, height: u32) -> Result<Self> {
        let bytes = std::fs::read(font_path)?;
        let font = FontVec::try_from_vec(bytes)
            .map_err(|e| Error::Other(format!("police illisible : {e}")))?;
        Ok(Self {
            font,
            width: width.max(320),
            height: height.max(240),
        })
    }

    pub fn render(&self, menu: &MenuState) -> Pixmap {
        let mut pixmap = Pixmap::new(self.width, self.height)
            .unwrap_or_else(|| Pixmap::new(320, 240).expect("pixmap minimal"));
        let ease = ease_out(menu.anim);
        let dim_alpha = (120.0 * ease) as u8;
        fill_rect(
            &mut pixmap,
            0.0,
            0.0,
            self.width as f32,
            self.height as f32,
            (0, 0, 0, dim_alpha),
        );

        let panel_h = menu.panel_height();
        let panel_w = PANEL_WIDTH.min(self.width as f32 - 48.0);
        let panel_x = (self.width as f32 - panel_w) / 2.0;
        let panel_y = (self.height as f32 - panel_h) / 2.0 + (1.0 - ease) * 18.0;
        let scale = 1.0 - (1.0 - ease) * 0.02;

        let mut paint = Paint::default();
        paint.anti_alias = true;
        paint.set_color_rgba8(BG_PANEL.0, BG_PANEL.1, BG_PANEL.2, (BG_PANEL.3 as f32 * ease) as u8);
        if let Some(path) = rounded(panel_x, panel_y, panel_w, panel_h, PANEL_RADIUS, scale) {
            pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
        }
        let mut border = Paint::default();
        border.anti_alias = true;
        border.set_color_rgba8(BORDER.0, BORDER.1, BORDER.2, (BORDER.3 as f32 * ease) as u8);
        if let Some(path) = rounded(panel_x, panel_y, panel_w, panel_h, PANEL_RADIUS, scale) {
            let stroke = tiny_skia::Stroke {
                width: 1.0,
                ..Default::default()
            };
            pixmap.stroke_path(&path, &border, &stroke, Transform::identity(), None);
        }

        // En-tête
        let header_alpha = (255.0 * ease) as u8;
        draw_text(
            &mut pixmap,
            &self.font,
            "LUDUS",
            panel_x + 24.0,
            panel_y + 18.0,
            15.0,
            (ACCENT.0, ACCENT.1, ACCENT.2, header_alpha),
        );
        let title = &menu.game_name;
        draw_text(
            &mut pixmap,
            &self.font,
            &ellipsis(title, 42),
            panel_x + 84.0,
            panel_y + 18.0,
            15.0,
            (TEXT_DIM.0, TEXT_DIM.1, TEXT_DIM.2, header_alpha),
        );
        fill_rect(
            &mut pixmap,
            panel_x + 24.0,
            panel_y + 50.0,
            panel_w - 48.0,
            1.0,
            (BORDER.0, BORDER.1, BORDER.2, header_alpha),
        );

        for (index, item) in menu.items().iter().enumerate() {
            let row_top = panel_y + menu.row_y(index);
            let is_selected = index == menu.selected;
            if is_selected {
                let glow = 0.75 + 0.25 * menu.pulse;
                let mut row_paint = Paint::default();
                row_paint.anti_alias = true;
                row_paint.set_color_rgba8(
                    ROW_SELECTED.0,
                    ROW_SELECTED.1,
                    ROW_SELECTED.2,
                    (240.0 * glow * ease) as u8,
                );
                if let Some(path) = rounded(
                    panel_x + 14.0,
                    row_top,
                    panel_w - 28.0,
                    40.0,
                    10.0,
                    1.0,
                ) {
                    pixmap.fill_path(&path, &row_paint, FillRule::Winding, Transform::identity(), None);
                }
                fill_rect(
                    &mut pixmap,
                    panel_x + 14.0 + 2.0,
                    row_top + 8.0,
                    3.0,
                    24.0,
                    (ACCENT.0, ACCENT.1, ACCENT.2, header_alpha),
                );
            }
            let enabled = menu.enabled(*item);
            let color = if !enabled {
                (TEXT_DIM.0, TEXT_DIM.1, TEXT_DIM.2, (120.0 * ease) as u8)
            } else if is_selected {
                (TEXT.0, TEXT.1, TEXT.2, header_alpha)
            } else {
                (TEXT.0, TEXT.1, TEXT.2, (215.0 * ease) as u8)
            };
            draw_text(
                &mut pixmap,
                &self.font,
                &menu.label(*item),
                panel_x + 34.0,
                row_top + 10.0,
                16.0,
                color,
            );
            if is_selected {
                let hint = menu.hint(*item);
                let hint_width = text_width(&self.font, hint, 12.0);
                draw_text(
                    &mut pixmap,
                    &self.font,
                    hint,
                    panel_x + panel_w - 24.0 - hint_width,
                    row_top + 14.0,
                    12.0,
                    (TEXT_DIM.0, TEXT_DIM.1, TEXT_DIM.2, (200.0 * ease) as u8),
                );
            }
        }

        let footer_y = panel_y + menu.panel_height() - 30.0;
        draw_text(
            &mut pixmap,
            &self.font,
            "↑↓ naviguer    ←→ modifier    Entrée valider    Échap reprendre",
            panel_x + 24.0,
            footer_y,
            12.0,
            (TEXT_DIM.0, TEXT_DIM.1, TEXT_DIM.2, (180.0 * ease) as u8),
        );
        let _ = Item::Resume;
        pixmap
    }
}

pub fn panel_bounds(width: u32, height: u32, menu: &MenuState) -> (f32, f32, f32, f32) {
    let panel_h = menu.panel_height();
    let panel_w = PANEL_WIDTH.min(width as f32 - 48.0);
    (
        (width as f32 - panel_w) / 2.0,
        (height as f32 - panel_h) / 2.0,
        panel_w,
        panel_h,
    )
}

fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

fn rounded(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    radius: f32,
    _scale: f32,
) -> Option<tiny_skia::Path> {
    let rect = Rect::from_xywh(x, y, w, h)?;
    let r = radius.min(w / 2.0).min(h / 2.0);
    let (x0, y0, x1, y1) = (rect.x(), rect.y(), rect.x() + w, rect.y() + h);
    let mut builder = PathBuilder::new();
    builder.move_to(x0 + r, y0);
    builder.line_to(x1 - r, y0);
    builder.quad_to(x1, y0, x1, y0 + r);
    builder.line_to(x1, y1 - r);
    builder.quad_to(x1, y1, x1 - r, y1);
    builder.line_to(x0 + r, y1);
    builder.quad_to(x0, y1, x0, y1 - r);
    builder.line_to(x0, y0 + r);
    builder.quad_to(x0, y0, x0 + r, y0);
    builder.close();
    builder.finish()
}

fn fill_rect(pixmap: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, rgba: (u8, u8, u8, u8)) {
    let Some(rect) = Rect::from_xywh(x, y, w, h) else {
        return;
    };
    let mut paint = Paint::default();
    paint.set_color_rgba8(rgba.0, rgba.1, rgba.2, rgba.3);
    paint.anti_alias = true;
    if let Some(path) = {
        let mut builder = PathBuilder::new();
        builder.push_rect(rect);
        builder.finish()
    } {
        pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
    }
}

pub fn text_width(font: &FontVec, text: &str, size: f32) -> f32 {
    let scaled = font.as_scaled(PxScale::from(size));
    text.chars()
        .map(|ch| scaled.h_advance(scaled.glyph_id(ch)))
        .sum()
}

pub fn draw_text(
    pixmap: &mut Pixmap,
    font: &FontVec,
    text: &str,
    x: f32,
    y: f32,
    size: f32,
    rgba: (u8, u8, u8, u8),
) {
    if rgba.3 == 0 {
        return;
    }
    let scaled = font.as_scaled(PxScale::from(size));
    let ascent = scaled.ascent();
    let mut caret = x;
    for ch in text.chars() {
        let glyph_id = scaled.glyph_id(ch);
        let glyph = glyph_id.with_scale_and_position(size, ab_glyph::point(caret, y + ascent));
        if let Some(outlined) = font.outline_glyph(glyph) {
            let bounds = outlined.px_bounds();
            let origin_x = bounds.min.x as i32;
            let origin_y = bounds.min.y as i32;
            outlined.draw(|gx, gy, coverage| {
                let px = origin_x + gx as i32;
                let py = origin_y + gy as i32;
                if px < 0 || py < 0 {
                    return;
                }
                if px as u32 >= pixmap.width() || py as u32 >= pixmap.height() {
                    return;
                }
                blend(
                    pixmap,
                    px as u32,
                    py as u32,
                    rgba,
                    coverage * (rgba.3 as f32 / 255.0),
                );
            });
        }
        caret += scaled.h_advance(glyph_id);
    }
}

fn blend(pixmap: &mut Pixmap, x: u32, y: u32, rgba: (u8, u8, u8, u8), alpha: f32) {
    let idx = ((y * pixmap.width() + x) * 4) as usize;
    let data = pixmap.data_mut();
    if idx + 3 >= data.len() {
        return;
    }
    let a = alpha.clamp(0.0, 1.0);
    let src_r = rgba.0 as f32 / 255.0 * a;
    let src_g = rgba.1 as f32 / 255.0 * a;
    let src_b = rgba.2 as f32 / 255.0 * a;
    let dst_r = data[idx] as f32 / 255.0;
    let dst_g = data[idx + 1] as f32 / 255.0;
    let dst_b = data[idx + 2] as f32 / 255.0;
    let dst_a = data[idx + 3] as f32 / 255.0;
    let out_a = a + dst_a * (1.0 - a);
    if out_a <= 0.0 {
        return;
    }
    let out_r = (src_r + dst_r * (1.0 - a)) / out_a;
    let out_g = (src_g + dst_g * (1.0 - a)) / out_a;
    let out_b = (src_b + dst_b * (1.0 - a)) / out_a;
    data[idx] = (out_r * 255.0).round().clamp(0.0, 255.0) as u8;
    data[idx + 1] = (out_g * 255.0).round().clamp(0.0, 255.0) as u8;
    data[idx + 2] = (out_b * 255.0).round().clamp(0.0, 255.0) as u8;
    data[idx + 3] = (out_a * 255.0).round().clamp(0.0, 255.0) as u8;
}

fn ellipsis(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menu::MenuState;

    fn renderer() -> Option<Renderer> {
        let font = crate::font::find_font().ok()?;
        Renderer::new(&font, 1280, 800).ok()
    }

    #[test]
    fn renders_non_empty_panel() {
        let Some(renderer) = renderer() else {
            return;
        };
        let mut menu = MenuState::new("Team Fortress 2");
        menu.anim = 1.0;
        let pixmap = renderer.render(&menu);
        assert_eq!(pixmap.width(), 1280);
        let opaque = pixmap
            .data()
            .chunks(4)
            .filter(|px| px[3] > 200)
            .count();
        assert!(opaque > 5000, "pixels opaques : {opaque}");
    }

    #[test]
    fn selection_changes_pixels() {
        let Some(renderer) = renderer() else {
            return;
        };
        let mut menu = MenuState::new("Jeu");
        menu.anim = 1.0;
        let first = renderer.render(&menu);
        menu.selected = 6;
        let second = renderer.render(&menu);
        assert_ne!(first.data(), second.data());
    }

    #[test]
    fn panel_is_centred() {
        let menu = MenuState::new("Jeu");
        let (x, y, w, h) = panel_bounds(1280, 800, &menu);
        assert!(x > 0.0 && y > 0.0);
        assert!(w <= 620.0);
        assert!(h > 300.0);
    }

    #[test]
    fn text_width_is_positive() {
        let Some(font_path) = crate::font::find_font().ok() else {
            return;
        };
        let bytes = std::fs::read(font_path).unwrap();
        let font = FontVec::try_from_vec(bytes).unwrap();
        assert!(text_width(&font, "Ludus", 16.0) > 10.0);
    }
}
