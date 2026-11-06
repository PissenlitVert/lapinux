//! Rendu CPU de l'overlay (tiny-skia + police TTF), sans contexte GPU.
//!
//! Le panneau est une colonne posée à gauche du jeu : capsule du jeu en tête,
//! puis une ligne par réglage (pictogramme, libellé, valeur).

use crate::menu::{
    icon, FpsStatus, HudLayout, HudPosition, HudPreset, Icon, Item, MenuState, Page, CAPSULE_HEIGHT,
    CAPSULE_TOP, CHAT_COMPOSER_HEIGHT, FOOTER_HEIGHT, FRIENDS_BUTTON_SIZE, FRIENDS_HEIGHT,
    FRIENDS_WIDTH, FPS_STEPS, FPS_TRACK_LEFT, FPS_TRACK_RIGHT, HEADER_HEIGHT,
    INVITE_TITLE_TOP, INVITE_TOP, LIVE_HEIGHT, LIVE_TOP, PADDING,
    PERF_ENGINE_GAP, PERF_ENGINE_W, PERF_GRID, PERF_INFO, PERF_ROW, PERF_SECTION, PERF_TOP,
    PROFILE_TOP, RECORDING_STATUS_HEIGHT, ROW_HEIGHT,
};
use crate::{Error, Result};
use ab_glyph::{Font, FontVec, PxScale, ScaleFont};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tiny_skia::{
    FillRule, IntSize, Paint, PathBuilder, Pixmap, PixmapPaint, Rect, Stroke, Transform,
};

const PANEL_WIDTH: f32 = 520.0;
/// Distance du bord gauche de l'écran.
const PANEL_MARGIN: f32 = 40.0;
/// Angles à l'équerre : le panneau et ses cartes sont rectangulaires.
const PANEL_RADIUS: f32 = 0.0;
const CARD_RADIUS: f32 = 0.0;

const BG_PANEL: (u8, u8, u8, u8) = (13, 14, 16, 247);
const BORDER: (u8, u8, u8, u8) = (255, 255, 255, 24);
const ROW: (u8, u8, u8, u8) = (255, 255, 255, 10);
const ROW_PRIMARY: (u8, u8, u8, u8) = (255, 255, 255, 20);
const SELECTED: (u8, u8, u8, u8) = (238, 239, 241, 255);
const TEXT: (u8, u8, u8, u8) = (234, 235, 238, 255);
const TEXT_DIM: (u8, u8, u8, u8) = (150, 152, 157, 255);
const TEXT_DARK: (u8, u8, u8, u8) = (21, 22, 24, 255);
const TEXT_DARK_DIM: (u8, u8, u8, u8) = (78, 80, 85, 255);
const DANGER: (u8, u8, u8, u8) = (231, 148, 148, 255);
/// Bleu discret : repères de section, onglet actif, titre en jeu. Il donne au
/// panneau une lecture d'ensemble sans concurrencer les valeurs.
const ACCENT: (u8, u8, u8, u8) = (138, 178, 255, 255);

pub struct Renderer {
    font: FontVec,
    /// Jaquette du jeu en cours, déjà décodée (facultative).
    capsule: Option<Pixmap>,
    /// Icône carrée du jeu, pour l'en-tête de la plaque de performance.
    game_icon: Option<Pixmap>,
    /// Avatars d'amis déjà décodés, par chemin du cache du daemon.
    invite_avatars: RefCell<HashMap<PathBuf, Option<Pixmap>>>,
    pub width: u32,
    pub height: u32,
}

impl Renderer {
    pub fn new(
        font_path: &Path,
        width: u32,
        height: u32,
        capsule: Option<&Path>,
        icon: Option<&Path>,
    ) -> Result<Self> {
        let bytes = std::fs::read(font_path)?;
        let font = FontVec::try_from_vec(bytes)
            .map_err(|e| Error::Other(format!("police illisible : {e}")))?;
        Ok(Self {
            font,
            capsule: capsule.and_then(decode_image),
            game_icon: icon.and_then(decode_image),
            invite_avatars: RefCell::new(HashMap::new()),
            width: width.max(320),
            height: height.max(240),
        })
    }

    /// Plaque de performance : la lecture de MangoHud — fond noir translucide
    /// sans bordure, une ligne par grandeur, libellé à gauche et valeur à
    /// droite dans la couleur du composant, barres par cœur et courbe de
    /// frametime en bas.
    ///
    /// Elle est dessinée dans la surface de l'overlay, qui est une fenêtre
    /// distincte de celle du jeu : une capture du jeu ne la contient jamais,
    /// alors que MangoHud dessine dans l'image du jeu.
    fn draw_perf_hud(&self, pixmap: &mut Pixmap, menu: &MenuState) {
        // Palette de la plaque : une teinte franche par composant, un blanc
        // doux pour les mesures et du gris bleuté pour les libellés. Les
        // couleurs se lisent d'un coup d'œil sans saturer l'image du jeu.
        const SKY: (u8, u8, u8, u8) = (125, 211, 252, 255);
        const GREEN: (u8, u8, u8, u8) = (52, 211, 153, 255);
        const BLUE: (u8, u8, u8, u8) = (96, 165, 250, 255);
        const VIOLET: (u8, u8, u8, u8) = (167, 139, 250, 255);
        const PINK: (u8, u8, u8, u8) = (244, 114, 182, 255);
        const VALUE: (u8, u8, u8, u8) = (241, 245, 249, 255);

        let live = &menu.live;
        // Le nom du matériel, comme MangoHud : « RTX 4070 », « Ryzen 7
        // 5800X3D ». Les deux libellés sont mis en cache.
        let gpu_label = lapinux_perf::mangohud::gpu_label();
        let cpu_label = lapinux_perf::mangohud::cpu_label();
        // Chaque ligne porte sa couleur et, pour les charges, la fraction qui
        // remplit sa barre de fond.
        let mut rows: Vec<(String, String, (u8, u8, u8, u8), Option<f32>)> = Vec::new();
        rows.push((
            "FPS".into(),
            live.fps
                .map(|fps| fps.to_string())
                .unwrap_or_else(|| "—".into()),
            SKY,
            None,
        ));
        if menu.hud != HudPreset::Simple {
            rows.push((
                "FrameTime".into(),
                live.fps
                    .filter(|fps| *fps > 0)
                    .map(|fps| format!("{:.1} ms", 1000.0 / fps as f32))
                    .unwrap_or_else(|| "—".into()),
                VALUE,
                None,
            ));
        }
        if let Some(system) = menu.system.as_ref() {
            if let Some(gpu) = system.gpu_load {
                rows.push((
                    gpu_label.unwrap_or_else(|| "GPU".into()),
                    format!("{:.0} %", gpu.usage),
                    GREEN,
                    Some(gpu.usage / 100.0),
                ));
                if menu.hud == HudPreset::Detailed && gpu.vram_total > 0 {
                    rows.push((
                        "VRAM".into(),
                        format!(
                            "{:.1} / {:.1} GiB",
                            gib(gpu.vram_used),
                            gib(gpu.vram_total)
                        ),
                        VIOLET,
                        Some(gpu.vram_used as f32 / gpu.vram_total as f32),
                    ));
                }
            }
            rows.push((
                cpu_label.unwrap_or_else(|| "CPU".into()),
                format!("{:.0} %", system.cpu_usage),
                BLUE,
                Some(system.cpu_usage / 100.0),
            ));
            if menu.hud != HudPreset::Simple && system.ram_total > 0 {
                rows.push((
                    "RAM".into(),
                    format!(
                        "{:.1} / {:.1} GiB",
                        gib(system.ram_used),
                        gib(system.ram_total)
                    ),
                    PINK,
                    Some(system.ram_used as f32 / system.ram_total as f32),
                ));
            }
        }

        let inline = menu.hud_layout == HudLayout::Inline;
        let cores = if menu.hud_cores {
            menu.system
                .as_ref()
                .map(|system| system.cpu_cores.clone())
                .filter(|cores| !cores.is_empty())
        } else {
            None
        };
        let graph = menu.hud_graph && live.series.len() >= 2;
        let margin = 18.0;

        if inline {
            self.draw_inline_perf_hud(pixmap, menu, cores.as_deref(), graph, margin);
            return;
        }

        // Une seule échelle pour tout : texte, marges, lignes, barres et
        // courbe. Le réglage va de 0,75 à 2,0 ; la valeur par défaut est un
        // peu plus grande que la plaque d'origine, illisible à 1080p.
        let scale = menu.perf_scale.clamp(0.75, 2.0);
        let font_size = 14.0 * scale;
        let label_size = 12.0 * scale;
        let row_h = 24.0 * scale;
        let pad = 12.0 * scale;
        let header_h = 22.0 * scale;
        let mut label_w: f32 = 0.0;
        let mut value_w: f32 = 0.0;
        for (label, value, _, _) in &rows {
            label_w = label_w.max(text_width(&self.font, label, label_size));
            value_w = value_w.max(text_width(&self.font, value, font_size));
        }
        let width = (label_w + 30.0 * scale + value_w).max(210.0 * scale) + 2.0 * pad;
        let mut height = 2.0 * pad + header_h + rows.len() as f32 * row_h;
        if cores.is_some() {
            height += 38.0 * scale;
        }
        if graph {
            height += 46.0 * scale;
        }
        let width = width
            .min(self.width as f32 - 2.0 * margin)
            .max(140.0 * scale);
        let (x, y) = hud_origin(
            self.width,
            self.height,
            width,
            height,
            menu.hud_position,
            margin,
        );
        draw_hud_card(pixmap, x, y, width, height);
        let left = x + pad;
        let right = x + width - pad;
        let inner = right - left;

        // En-tête : l'icône carrée du jeu et son nom remplacent le mot
        // « PERFORMANCE ». La durée de session reste à droite.
        let mut header_x = left + 2.0;
        if let Some(icon) = &self.game_icon {
            let size = 18.0 * scale;
            let icon_y = y + pad + (header_h - size) * 0.5 - 2.0 * scale;
            let sx = size / icon.width() as f32;
            let sy = size / icon.height() as f32;
            pixmap.draw_pixmap(
                header_x as i32,
                icon_y as i32,
                icon.as_ref(),
                &PixmapPaint::default(),
                Transform::from_scale(sx, sy),
                None,
            );
            header_x += size + 7.0 * scale;
        }
        let title = fit_text(
            &self.font,
            &menu.game_name,
            (right - header_x - 64.0 * scale).max(40.0),
            11.5 * scale,
        );
        draw_text_baseline(
            pixmap,
            &self.font,
            &title,
            header_x,
            y + pad + 9.0 * scale,
            11.5 * scale,
            (234, 235, 238, 225),
        );
        let session = live
            .session_secs
            .map(clock)
            .unwrap_or_else(|| "—".into());
        draw_text_right_baseline(
            pixmap,
            &self.font,
            &session,
            right,
            y + pad + 9.0 * scale,
            10.0 * scale,
            (148, 163, 184, 190),
        );

        let mut cursor = y + pad + header_h;
        for (label, value, color, fill) in &rows {
            let baseline = cursor + row_h * 0.64;
            let mut dot = PathBuilder::new();
            dot.push_circle(left + 3.0 * scale, cursor + row_h * 0.5, 2.4 * scale);
            fill_path(pixmap, dot, *color);
            draw_text_baseline(
                pixmap,
                &self.font,
                label,
                left + 14.0 * scale,
                baseline,
                label_size,
                (148, 163, 184, 225),
            );
            draw_text_right_baseline(
                pixmap,
                &self.font,
                value,
                right,
                baseline,
                font_size,
                *color,
            );
            if let Some(fraction) = fill {
                let bar_y = cursor + row_h - 3.5 * scale;
                fill_round(
                    pixmap,
                    left,
                    bar_y,
                    inner,
                    2.5 * scale,
                    1.2 * scale,
                    (255, 255, 255, 16),
                );
                fill_round(
                    pixmap,
                    left,
                    bar_y,
                    inner * fraction.clamp(0.0, 1.0),
                    2.5 * scale,
                    1.2 * scale,
                    (color.0, color.1, color.2, 175),
                );
            }
            cursor += row_h;
        }
        if let Some(cores) = cores.as_deref() {
            draw_text_baseline(
                pixmap,
                &self.font,
                "CŒURS",
                left,
                cursor + 10.0 * scale,
                9.5 * scale,
                (148, 163, 184, 210),
            );
            draw_cores(
                pixmap,
                left,
                cursor + 15.0 * scale,
                inner,
                13.0 * scale,
                cores,
                BLUE,
            );
            cursor += 38.0 * scale;
        }
        if graph {
            draw_text_baseline(
                pixmap,
                &self.font,
                "FRAMETIME",
                left,
                cursor + 10.0 * scale,
                9.5 * scale,
                (148, 163, 184, 210),
            );
            draw_frametime_graph(
                pixmap,
                &live.series,
                left,
                cursor + 15.0 * scale,
                inner,
                28.0 * scale,
                (56, 189, 248, 220),
            );
        }
    }

}

/// Une ligne façon MangoHud : le matériel d'abord, la cadence à la fin, des
/// Capacité en Gio : une barrette de 32 Go annonce « 32 », pas « 34 », qui
/// est la même quantité comptée en Go décimaux.
fn gib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0 * 1024.0)
}

/// valeurs colorées et aucun séparateur. La plaque native ne lit pas toutes
/// les sondes de MangoHud (API graphique, par exemple), mais elle garde sa
/// grammaire visuelle et l'ordre de ses grandeurs.
fn inline_hud_spans(menu: &MenuState) -> Vec<(String, (u8, u8, u8, u8))> {
    const GPU: (u8, u8, u8, u8) = (126, 217, 87, 255);
    const CPU: (u8, u8, u8, u8) = (106, 181, 255, 255);
    const TEMP: (u8, u8, u8, u8) = (242, 181, 90, 255);
    const RAM: (u8, u8, u8, u8) = (226, 120, 180, 255);
    const VRAM: (u8, u8, u8, u8) = (184, 142, 255, 255);
    const FPS: (u8, u8, u8, u8) = (255, 145, 130, 255);
    const FRAMETIME: (u8, u8, u8, u8) = (235, 235, 235, 255);

    let mut spans = Vec::new();
    if let Some(system) = menu.system.as_ref() {
        let gpu_label = lapinux_perf::mangohud::gpu_label();
        if let Some(gpu) = system.gpu_load {
            spans.push((
                format!(
                    "{} {:.0}%",
                    gpu_label.as_deref().unwrap_or("GPU"),
                    gpu.usage
                ),
                GPU,
            ));
            if let Some(temperature) = gpu.temperature {
                spans.push((format!("{temperature:.0}°C"), TEMP));
            }
            if menu.hud == HudPreset::Detailed && gpu.vram_total > 0 {
                spans.push((
                    format!(
                        "VRAM {:.1}/{:.1} GiB",
                        gib(gpu.vram_used),
                        gib(gpu.vram_total)
                    ),
                    VRAM,
                ));
            }
        }
        spans.push((
            format!(
                "{} {:.0}%",
                lapinux_perf::mangohud::cpu_label()
                    .as_deref()
                    .unwrap_or("CPU"),
                system.cpu_usage
            ),
            CPU,
        ));
        if let Some(temperature) = system.cpu_temp {
            spans.push((format!("{temperature:.0}°C"), TEMP));
        }
        if menu.hud != HudPreset::Simple && system.ram_total > 0 {
            spans.push((
                format!("RAM {:.1} GiB", gib(system.ram_used)),
                RAM,
            ));
        }
    }
    if let Some(fps) = menu.live.fps {
        spans.push((format!("{fps} FPS"), FPS));
        if menu.hud != HudPreset::Simple && fps > 0 {
            spans.push((format!("{:.1} ms", 1000.0 / fps as f32), FRAMETIME));
        }
    } else {
        spans.push(("— FPS".into(), FPS));
    }
    spans
}

impl Renderer {
    /// Ligne compacte façon MangoHud : fond noir translucide carré, texte
    /// serré, chaque composant dans sa couleur, puis la cadence à la fin.
    fn draw_inline_perf_hud(
        &self,
        pixmap: &mut Pixmap,
        menu: &MenuState,
        cores: Option<&[f32]>,
        graph: bool,
        margin: f32,
    ) {
        const CPU: (u8, u8, u8, u8) = (106, 181, 255, 230);
        let spans = inline_hud_spans(menu);
        let scale = menu.perf_scale.clamp(0.75, 2.0);
        let font_size = 13.0 * scale;
        let pad_x = 8.0 * scale;
        let pad_y = 4.0 * scale;
        let gap = 9.0 * scale;
        let line_h = font_size + 3.0 * scale;
        let mut content_w = 0.0;
        for (index, (text, _)) in spans.iter().enumerate() {
            content_w += text_width(&self.font, text, font_size);
            if index + 1 < spans.len() {
                content_w += gap;
            }
        }
        let strip = cores.is_some() || graph;
        let chart_h = if graph {
            26.0 * scale
        } else if cores.is_some() {
            14.0 * scale
        } else {
            0.0
        };
        let width = (content_w.max(if graph { 280.0 * scale } else { 0.0 }) + 2.0 * pad_x)
            .min(self.width as f32 - 2.0 * margin)
            .max(140.0 * scale);
        let height = 2.0 * pad_y + line_h + if strip { chart_h + 4.0 * scale } else { 0.0 };
        let (x, y) = hud_origin(
            self.width,
            self.height,
            width,
            height,
            menu.hud_position,
            margin,
        );
        // MangoHud : fond noir translucide, sans bordure ni coins arrondis.
        fill_rect(pixmap, x, y, width, height, (0, 0, 0, 90));
        let baseline = y + pad_y + font_size;
        let mut cursor = x + pad_x;
        for (text, color) in &spans {
            draw_text_baseline(
                pixmap,
                &self.font,
                text,
                cursor,
                baseline,
                font_size,
                *color,
            );
            cursor += text_width(&self.font, text, font_size) + gap;
        }
        if !strip {
            return;
        }
        let top = y + pad_y + line_h + 2.0 * scale;
        let inner = width - 2.0 * pad_x;
        match (cores, graph) {
            (Some(cores), true) => {
                let half = (inner - 12.0 * scale) / 2.0;
                draw_cores(
                    pixmap,
                    x + pad_x,
                    top,
                    half,
                    12.0 * scale,
                    cores,
                    CPU,
                );
                draw_frametime_graph(
                    pixmap,
                    &menu.live.series,
                    x + pad_x + half + 12.0 * scale,
                    top,
                    half,
                    26.0 * scale,
                    (56, 189, 248, 220),
                );
            }
            (Some(cores), false) => {
                draw_cores(
                    pixmap,
                    x + pad_x,
                    top,
                    inner,
                    14.0 * scale,
                    cores,
                    CPU,
                );
            }
            (None, true) => {
                draw_frametime_graph(
                    pixmap,
                    &menu.live.series,
                    x + pad_x,
                    top,
                    inner,
                    26.0 * scale,
                    (56, 189, 248, 220),
                );
            }
            (None, false) => {}
        }
    }

    /// Badge « REC » indépendant : le statut de l'enregistrement se place et
    /// se dimensionne sans être collé aux chiffres de performance.
    fn draw_record_badge(&self, pixmap: &mut Pixmap, menu: &MenuState) {
        if !menu.record_badge {
            return;
        }
        let Some(state) = menu.live.recording else {
            return;
        };
        let scale = menu.record_scale.clamp(0.65, 2.0);
        let font_size = 11.5 * scale;
        let pad = 9.0 * scale;
        let dot = 7.0 * scale;
        let text = if state.paused {
            format!("PAUSE  {}", clock(state.secs))
        } else {
            format!("REC  {}", clock(state.secs))
        };
        let text_w = text_width(&self.font, &text, font_size);
        let width = 2.0 * pad + dot + 7.0 * scale + text_w;
        let height = (font_size + 2.0 * pad).max(26.0 * scale);
        let (x, y) = hud_origin(
            self.width,
            self.height,
            width,
            height,
            menu.record_position,
            18.0,
        );
        let live = !state.paused;
        let accent = if live {
            (248, 113, 113, 255)
        } else {
            (148, 163, 184, 255)
        };
        fill_round(
            pixmap,
            x,
            y,
            width,
            height,
            height / 2.0,
            (8, 10, 14, 220),
        );
        stroke_round(
            pixmap,
            x,
            y,
            width,
            height,
            height / 2.0,
            (accent.0, accent.1, accent.2, 110),
        );
        let cy = y + height / 2.0;
        let dot_x = x + pad;
        fill_round(
            pixmap,
            dot_x - 2.0,
            cy - dot / 2.0 - 2.0,
            dot + 4.0,
            dot + 4.0,
            (dot + 4.0) / 2.0,
            (accent.0, accent.1, accent.2, 45),
        );
        fill_round(pixmap, dot_x, cy - dot / 2.0, dot, dot, dot / 2.0, accent);
        draw_text_baseline(
            pixmap,
            &self.font,
            &text,
            dot_x + dot + 7.0 * scale,
            cy + font_size * 0.35,
            font_size,
            if live {
                (254, 226, 226, 255)
            } else {
                (203, 213, 225, 235)
            },
        );
    }

    pub fn render(&self, menu: &MenuState) -> Pixmap {
        let mut pixmap = Pixmap::new(self.width, self.height)
            .unwrap_or_else(|| Pixmap::new(320, 240).expect("pixmap minimal"));
        let ease = ease_out(menu.anim);
        // Panneau fermé : sa surface devient transparente. Seul le processus
        // `--hud-only` peint la plaque et le badge ; le processus du menu les
        // connaît pour ses interrupteurs, mais ne les redessine pas, sinon
        // deux plaques se superposeraient à la fermeture du panneau.
        if ease < 0.02 {
            if menu.hud_only {
                if menu.perf_hud {
                    self.draw_perf_hud(&mut pixmap, menu);
                }
                self.draw_record_badge(&mut pixmap, menu);
            }
            return pixmap;
        }
        // Voile léger : le jeu reste lisible à droite du panneau.
        fill_rect(
            &mut pixmap,
            0.0,
            0.0,
            self.width as f32,
            self.height as f32,
            (0, 0, 0, (104.0 * ease) as u8),
        );

        let (panel_x, panel_y, panel_w, panel_h) = panel_origin(self.width, self.height, menu);
        let alpha = (255.0 * ease) as u8;

        fill_round(
            &mut pixmap,
            panel_x,
            panel_y,
            panel_w,
            panel_h,
            PANEL_RADIUS,
            with_alpha(BG_PANEL, alpha),
        );
        // Dégradé très léger : le haut du panneau se détache du bas, sans
        // éclaircir franchement le fond derrière le texte.
        shade_round(
            &mut pixmap,
            panel_x,
            panel_y,
            panel_w,
            panel_h,
            PANEL_RADIUS,
            (24, 26, 31),
            |t| ((1.0 - t).powf(1.4) * 46.0) as u8,
            alpha,
        );
        stroke_round(
            &mut pixmap,
            panel_x,
            panel_y,
            panel_w,
            panel_h,
            PANEL_RADIUS,
            with_alpha(BORDER, alpha),
        );

        match menu.page {
            Page::Menu => {
                self.draw_capsule(&mut pixmap, menu, panel_x, panel_y, panel_w, alpha);
                self.draw_live(&mut pixmap, menu, panel_x, panel_y, panel_w, alpha);
                for index in 0..menu.items().len() {
                    draw_row(
                        &mut pixmap,
                        &self.font,
                        menu,
                        menu.items()[index],
                        index,
                        panel_x,
                        panel_w,
                        panel_y + menu.row_y(index),
                        alpha,
                    );
                }
                draw_friends_button(
                    &mut pixmap,
                    &self.font,
                    menu,
                    panel_x,
                    panel_y,
                    panel_h,
                    alpha,
                );
            }
            Page::Performance => {
                draw_performance(
                    &mut pixmap,
                    &self.font,
                    menu,
                    panel_x,
                    panel_y,
                    panel_w,
                    alpha,
                );
            }
            Page::Recordings => {
                draw_record_settings(
                    &mut pixmap,
                    &self.font,
                    menu,
                    panel_x,
                    panel_y,
                    panel_w,
                    alpha,
                );
            }
            Page::Profiles => {
                draw_profiles(
                    &mut pixmap,
                    &self.font,
                    menu,
                    panel_x,
                    panel_y,
                    panel_w,
                    alpha,
                );
            }
            Page::Invite => {
                draw_invite(
                    &mut pixmap,
                    &self.font,
                    menu,
                    panel_x,
                    panel_y,
                    panel_w,
                    panel_h,
                    alpha,
                    &self.invite_avatars,
                );
            }
        }

        // Filet de séparation entre l'en-tête et la première ligne : il donne
        // un repère fixe au regard, que la jaquette soit claire ou sombre.
        // La page d'amis a son propre en-tête compact et trace le sien.
        if menu.page != Page::Invite {
            fill_rect(
                &mut pixmap,
                panel_x + PADDING,
                panel_y + HEADER_HEIGHT - 6.0,
                panel_w - 2.0 * PADDING,
                1.0,
                with_alpha(BORDER, alpha),
            );
        }

        // Pied de page : raccourcis clavier et manette.
        let footer_top = panel_y + panel_h - FOOTER_HEIGHT;
        fill_rect(
            &mut pixmap,
            panel_x + PADDING + 6.0,
            footer_top,
            panel_w - 2.0 * PADDING - 12.0,
            1.0,
            with_alpha(BORDER, alpha),
        );
        let hint_y = footer_top + 15.0;
        let mut hint_x = panel_x + PADDING + 4.0;
        if menu.page == Page::Menu {
            hint_x += FRIENDS_BUTTON_SIZE + 12.0;
        }
        hint_x = key_hint(
            &mut pixmap,
            &self.font,
            hint_x,
            hint_y,
            "A",
            if menu.page == Page::Invite {
                if menu.chat_open() {
                    "Envoyer"
                } else {
                    "Ouvrir"
                }
            } else {
                "Valider"
            },
            alpha,
        );
        hint_x = key_hint(
            &mut pixmap,
            &self.font,
            hint_x,
            hint_y,
            "B",
            if menu.page != Page::Menu {
                "Retour"
            } else {
                "Fermer"
            },
            alpha,
        );
        if !(menu.page == Page::Invite && menu.chat_open()) {
            arrow_hint(
                &mut pixmap,
                &self.font,
                hint_x,
                hint_y,
                if menu.page == Page::Profiles {
                    "Choisir"
                } else if menu.page == Page::Invite {
                    "Défiler"
                } else {
                    "Régler"
                },
                alpha,
            );
        }
        if menu.page == Page::Performance {
            // GameScope peut changer sa limite de FPS à chaud ; hors de lui,
            // elle attend le prochain lancement. MangoHud relit toujours sa
            // configuration quand un HUD tourne.
            let note = if menu.perf_selected == 0 {
                if menu.live.gamescope {
                    "GameScope : à chaud"
                } else {
                    "prochain lancement"
                }
            } else if menu.perf_selected == 1 {
                "prochain lancement"
            } else if menu.perf_selected == 9 {
                "à chaud"
            } else if menu.live.hud_live {
                "MangoHud : à chaud"
            } else {
                "prochain lancement"
            };
            draw_text_right(
                &mut pixmap,
                &self.font,
                note,
                panel_x + panel_w - PADDING - 4.0,
                hint_y + 4.0,
                11.0,
                with_alpha(TEXT_DIM, alpha),
            );
        }

        pixmap
    }

    /// Carte de tête : jaquette du jeu, voile dégradé et titre.
    fn draw_capsule(
        &self,
        pixmap: &mut Pixmap,
        menu: &MenuState,
        panel_x: f32,
        panel_y: f32,
        panel_w: f32,
        alpha: u8,
    ) {
    let x = panel_x + PADDING;
    let y = panel_y + CAPSULE_TOP;
    let w = panel_w - 2.0 * PADDING;
    let h = CAPSULE_HEIGHT;
    let radius = CARD_RADIUS;
    // Filet lumineux au sommet de la carte : elle se détache du fond du
    // panneau sans empiéter sur la jaquette.
    fill_rect(
        pixmap,
        x,
        y,
        w,
        1.0,
        with_alpha((255, 255, 255, 40), alpha),
    );

        match &self.capsule {
            Some(image) => draw_cover(pixmap, image, x, y, w, h, radius, alpha),
            None => {
                // Sans jaquette : un dégradé sombre garde la même silhouette.
                fill_round(
                    pixmap,
                    x,
                    y,
                    w,
                    h,
                    radius,
                    with_alpha((34, 36, 42, 255), alpha),
                );
                shade_round(
                    pixmap,
                    x,
                    y,
                    w,
                    h,
                    radius,
                    (8, 9, 11),
                    |t| (150.0 * t.powf(1.5)) as u8,
                    alpha,
                );
            }
        }

        // Voile bas pour que le titre tienne sur n'importe quelle image.
        shade_round(
            pixmap,
            x,
            y,
            w,
            h,
            radius,
            (7, 8, 10),
            |t| {
                let t = ((t - 0.38) / 0.62).clamp(0.0, 1.0);
                (196.0 * t.powf(1.15)) as u8
            },
            alpha,
        );

        // Étiquette « LAPINUX » en haut à gauche.
        let pill_w = text_width(&self.font, "LAPINUX", 10.5) + 20.0;
        fill_round(
            pixmap,
            x + 12.0,
            y + 12.0,
            pill_w,
            22.0,
            0.0,
            with_alpha((12, 13, 15, 190), alpha),
        );
        draw_text(
            pixmap,
            &self.font,
            "LAPINUX",
            x + 22.0,
            y + 16.5,
            10.5,
            with_alpha((240, 241, 243, 255), alpha),
        );

        let title = fit_text(&self.font, &menu.game_name, w - 40.0, 25.0);
        draw_text(
            pixmap,
            &self.font,
            &title,
            x + 18.0,
            y + h - 46.0,
            25.0,
            with_alpha((252, 252, 253, 255), alpha),
        );
        let overline = "EN JEU";
        draw_text(
            pixmap,
            &self.font,
            overline,
            x + 18.0,
            y + h - 20.0,
            11.0,
            with_alpha(ACCENT, alpha),
        );
        draw_text(
            pixmap,
            &self.font,
            "· menu rapide",
            x + 18.0 + text_width(&self.font, overline, 11.0) + 8.0,
            y + h - 20.0,
            11.0,
            with_alpha(TEXT_DIM, alpha),
        );
    }

    /// Bandeau sous la capsule : cadence en direct et durée de la session.
    fn draw_live(
        &self,
        pixmap: &mut Pixmap,
        menu: &MenuState,
        panel_x: f32,
        panel_y: f32,
        panel_w: f32,
        alpha: u8,
    ) {
        let y = panel_y + LIVE_TOP;
        let palette = match menu.live.fps_status(menu.fps_cap) {
            FpsStatus::Good => (
                with_alpha((74, 222, 128, 34), alpha),
                with_alpha((74, 222, 128, 96), alpha),
                with_alpha((150, 242, 180, 255), alpha),
            ),
            FpsStatus::Low => (
                with_alpha((248, 113, 113, 34), alpha),
                with_alpha((248, 113, 113, 96), alpha),
                with_alpha((252, 172, 172, 255), alpha),
            ),
            FpsStatus::Unknown => (
                with_alpha((255, 255, 255, 14), alpha),
                with_alpha((255, 255, 255, 22), alpha),
                with_alpha((225, 227, 231, 255), alpha),
            ),
        };
        let neutral = (
            with_alpha((255, 255, 255, 14), alpha),
            with_alpha((255, 255, 255, 22), alpha),
            with_alpha((225, 227, 231, 255), alpha),
        );
        let mut cursor = panel_x + PADDING;
        cursor = draw_tag(pixmap, &self.font, cursor, y, &menu.live.fps_label(), palette);
        cursor = draw_tag(
            pixmap,
            &self.font,
            cursor,
            y,
            &menu.live.session_label(),
            neutral,
        );
        // La capture en cours : rouge quand elle tourne, grise en pause.
        if let Some(label) = menu.live.recording_label() {
            let paused = menu
                .live
                .recording
                .map(|state| state.paused)
                .unwrap_or(false);
            let capture = if paused {
                neutral
            } else {
                (
                    with_alpha((248, 113, 113, 34), alpha),
                    with_alpha((248, 113, 113, 96), alpha),
                    with_alpha((252, 172, 172, 255), alpha),
                )
            };
            cursor = draw_tag(pixmap, &self.font, cursor, y, &label, capture);
        }

        // Mini-courbe calée à droite du bandeau, si la place le permet.
        let curve_w = 150.0;
        let right = panel_x + panel_w - PADDING;
        if menu.live.series.len() >= 2 && right - curve_w > cursor + 8.0 {
            let left = right - curve_w;
            fill_round(
                pixmap,
                left,
                y + 2.0,
                curve_w,
                LIVE_HEIGHT - 4.0,
                0.0,
                with_alpha((255, 255, 255, 12), alpha),
            );
            draw_sparkline(
                pixmap,
                left,
                y + 2.0,
                curve_w,
                LIVE_HEIGHT - 4.0,
                &menu.live.series,
                palette.2,
            );
        }
    }
}

/// Mini-courbe de cadence : aire légère sous la ligne, point sur la dernière
/// mesure, comme les cartes d'activité.
fn draw_sparkline(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    series: &[u32],
    color: (u8, u8, u8, u8),
) {
    let highest = series.iter().copied().max().unwrap_or(1).max(1) as f32;
    let lowest = series.iter().copied().min().unwrap_or(0) as f32;
    let span = (highest - lowest).max(1.0);
    let inset_x = 5.0;
    let inset_y = 3.0;
    let points: Vec<(f32, f32)> = series
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let px = x + inset_x + (w - 2.0 * inset_x) * index as f32 / (series.len() - 1) as f32;
            let ratio = (*value as f32 - lowest) / span;
            let py = y + h - inset_y - (h - 2.0 * inset_y) * ratio;
            (px, py)
        })
        .collect();

    let mut area = PathBuilder::new();
    area.move_to(points[0].0, y + h - inset_y);
    for (px, py) in &points {
        area.line_to(*px, *py);
    }
    area.line_to(points[points.len() - 1].0, y + h - inset_y);
    area.close();
    fill_path(
        pixmap,
        area,
        (color.0, color.1, color.2, (color.3 as f32 * 0.22) as u8),
    );

    let mut line = PathBuilder::new();
    line.move_to(points[0].0, points[0].1);
    for (px, py) in &points[1..] {
        line.line_to(*px, *py);
    }
    stroke_path(pixmap, line, color, 1.2);

    let last = points[points.len() - 1];
    let mut dot = PathBuilder::new();
    dot.push_circle(last.0, last.1, 1.8);
    fill_path(pixmap, dot, color);
}

/// Étiquette rectangulaire du bandeau de statistiques ; renvoie l'abscisse
/// suivante (étiquette et espace).
fn draw_tag(
    pixmap: &mut Pixmap,
    font: &FontVec,
    x: f32,
    y: f32,
    text: &str,
    palette: ((u8, u8, u8, u8), (u8, u8, u8, u8), (u8, u8, u8, u8)),
) -> f32 {
    let (fill, border, ink) = palette;
    let width = text_width(font, text, 12.5) + 24.0;
    fill_round(pixmap, x, y, width, LIVE_HEIGHT, 0.0, fill);
    stroke_round(pixmap, x, y, width, LIVE_HEIGHT, 0.0, border);
    draw_text(pixmap, font, text, x + 12.0, y + 7.0, 12.5, ink);
    x + width + 8.0
}

fn draw_row(
    pixmap: &mut Pixmap,
    font: &FontVec,
    menu: &MenuState,
    item: Item,
    index: usize,
    panel_x: f32,
    panel_w: f32,
    row_top: f32,
    alpha: u8,
) {
    let selected = menu.selected == index;
    let card_x = panel_x + PADDING;
    let card_w = panel_w - 2.0 * PADDING;
    let card_y = row_top + 2.0;
    let card_h = ROW_HEIGHT - 4.0;
    let radius = CARD_RADIUS;

    if selected {
        fill_round(
            pixmap,
            card_x,
            card_y,
            card_w,
            card_h,
            radius,
            with_alpha(SELECTED, alpha),
        );
        // Barre de focus à gauche, dans l'alignement du pictogramme.
        fill_round(
            pixmap,
            card_x + 3.0,
            card_y + 12.0,
            3.0,
            card_h - 24.0,
            0.0,
            with_alpha(ACCENT, alpha),
        );
    } else if item == Item::Resume {
        fill_round(
            pixmap,
            card_x,
            card_y,
            card_w,
            card_h,
            radius,
            with_alpha(ROW_PRIMARY, alpha),
        );
    } else {
        fill_round(
            pixmap,
            card_x,
            card_y,
            card_w,
            card_h,
            radius,
            with_alpha(ROW, alpha),
        );
    }

    let (title_color, icon_color, value_color) = if selected {
        (
            with_alpha(TEXT_DARK, alpha),
            with_alpha(TEXT_DARK, alpha),
            with_alpha(TEXT_DARK_DIM, alpha),
        )
    } else if item == Item::QuitGame {
        (
            with_alpha(DANGER, alpha),
            with_alpha(DANGER, alpha),
            with_alpha(DANGER, (alpha as f32 * 0.7) as u8),
        )
    } else if item == Item::Resume {
        (
            with_alpha(TEXT, alpha),
            with_alpha(ACCENT, alpha),
            with_alpha(TEXT_DIM, alpha),
        )
    } else {
        (
            with_alpha(TEXT, alpha),
            with_alpha(TEXT, alpha),
            with_alpha(TEXT_DIM, alpha),
        )
    };

    draw_icon(
        pixmap,
        icon(item),
        card_x + 34.0,
        card_y + card_h / 2.0,
        icon_color,
    );

    let title = match item {
        Item::Resume => "Reprendre le jeu",
        Item::Performance => "Performances",
        Item::Recordings => "Enregistrements",
        Item::Volume => "Volume du jeu",
        Item::Profile => "Profil manettes",
        Item::Proton => "Proton",
        Item::Screenshot => "Capture d'écran",
        Item::QuitGame => "Quitter le jeu",
    };
    draw_text(pixmap, font, title, card_x + 58.0, card_y + 16.5, 17.5, title_color);

    if item == Item::Volume {
        // Une vraie piste sous le titre : le curseur se règle au pointeur,
        // au clavier et à la manette.
        let left = card_x + 58.0;
        let right = card_x + card_w - 16.0;
        let track_w = (right - left).max(1.0);
        let track_y = card_y + card_h - 11.0;
        let track_color = if selected {
            with_alpha((24, 26, 30, 38), alpha)
        } else {
            with_alpha((255, 255, 255, 30), alpha)
        };
        let fill_color = if selected {
            with_alpha((24, 26, 30, 255), alpha)
        } else {
            with_alpha((236, 237, 239, 190), alpha)
        };
        fill_round(pixmap, left, track_y, track_w, 4.0, 2.0, track_color);
        fill_round(
            pixmap,
            left,
            track_y,
            track_w * menu.volume_ratio(),
            4.0,
            2.0,
            fill_color,
        );
        let knob = left + track_w * menu.volume_ratio();
        fill_round(pixmap, knob - 4.0, track_y - 3.0, 8.0, 10.0, 4.0, fill_color);
    }

    if let Some(value) = menu.value(item) {
        let text = fit_text(font, &value, 134.0, 14.0);
        draw_text_right(
            pixmap,
            font,
            &text,
            card_x + card_w - 16.0,
            card_y + 16.5,
            14.0,
            value_color,
        );
    }
}

/// Panneau « Performances » : limite de FPS, MangoHud, trois profils,
/// disposition ligne/bloc, grille des neuf ancrages et détail DLSS.
fn draw_performance(
    pixmap: &mut Pixmap,
    font: &FontVec,
    menu: &MenuState,
    panel_x: f32,
    panel_y: f32,
    panel_w: f32,
    alpha: u8,
) {
    let left = panel_x + PADDING;
    let width = panel_w - 2.0 * PADDING;

    draw_back_button(pixmap, font, menu, panel_x, panel_y, panel_w, alpha);
    // L'interrupteur de la plaque est la première ligne du panneau.
    draw_perf_card(pixmap, font, menu, 0, left, panel_y + menu.perf_row_y(0), width, alpha);
    draw_perf_card(pixmap, font, menu, 1, left, panel_y + menu.perf_row_y(1), width, alpha);
    draw_fps_track(pixmap, font, menu, left, panel_y + menu.perf_row_y(1), width, alpha);
    draw_perf_card(pixmap, font, menu, 2, left, panel_y + menu.perf_row_y(2), width, alpha);
    draw_engine_buttons(
        pixmap,
        font,
        menu,
        left,
        panel_y + menu.perf_row_y(2),
        width,
        alpha,
    );

    draw_section_label(
        pixmap,
        font,
        left,
        panel_y + menu.perf_row_y(3) - PERF_SECTION + 7.0,
        width,
        "PROFIL MANGOHUD",
        alpha,
    );
    for row in 3..=5 {
        draw_perf_card(
            pixmap,
            font,
            menu,
            row,
            left,
            panel_y + menu.perf_row_y(row),
            width,
            alpha,
        );
    }

    draw_section_label(
        pixmap,
        font,
        left,
        panel_y + menu.perf_row_y(6) - PERF_SECTION + 7.0,
        width,
        "POSITION",
        alpha,
    );
    for row in 6..=9 {
        draw_perf_card(
            pixmap,
            font,
            menu,
            row,
            left,
            panel_y + menu.perf_row_y(row),
            width,
            alpha,
        );
    }
    // Taille de la plaque native, en dernier avant la grille.
    draw_perf_card(
        pixmap,
        font,
        menu,
        10,
        left,
        panel_y + menu.perf_row_y(10),
        width,
        alpha,
    );
    draw_value_track(
        pixmap,
        menu,
        left,
        panel_y + menu.perf_row_y(10),
        width,
        ((menu.perf_scale - 0.75) / (2.0 - 0.75)).clamp(0.0, 1.0),
        menu.perf_selected == 10,
        alpha,
    );
    // La grille reste cliquable dans les deux dispositions : la couche
    // MangoHud de Lapinux place les neuf ancrages aussi bien en ligne qu'en
    // bloc.
    draw_position_grid(
        pixmap,
        font,
        menu,
        left,
        panel_y + menu.perf_grid_top(),
        width,
        false,
        alpha,
    );

    let dlss_y = panel_y + menu.perf_height() - FOOTER_HEIGHT - 10.0 - PERF_INFO;
    draw_dlss_card(pixmap, font, menu, left, dlss_y, width, alpha);
}

/// Page « Profils manettes » : tous les profils enregistrés, le profil actif
/// marqué, et une ligne sélectionnable au clavier, à la manette ou à la souris.
fn draw_profiles(
    pixmap: &mut Pixmap,
    font: &FontVec,
    menu: &MenuState,
    panel_x: f32,
    panel_y: f32,
    panel_w: f32,
    alpha: u8,
) {
    let left = panel_x + PADDING;
    let width = panel_w - 2.0 * PADDING;
    draw_back_button(pixmap, font, menu, panel_x, panel_y, panel_w, alpha);
    draw_text(
        pixmap,
        font,
        "Profils manettes",
        left,
        panel_y + HEADER_HEIGHT + 16.0,
        18.0,
        with_alpha(TEXT, alpha),
    );
    if menu.profiles.is_empty() {
        draw_text(
            pixmap,
            font,
            "Aucun profil enregistré",
            left,
            panel_y + HEADER_HEIGHT + PROFILE_TOP + 14.0,
            13.0,
            with_alpha(TEXT_DIM, alpha),
        );
        return;
    }
    for (index, name) in menu.profiles.iter().enumerate() {
        let top = panel_y + menu.profile_row_y(index);
        let selected = index == menu.profile_selected;
        let active = *name == menu.profile;
        fill_round(
            pixmap,
            left,
            top + 2.0,
            width,
            ROW_HEIGHT - 4.0,
            CARD_RADIUS,
            with_alpha(if selected { SELECTED } else { ROW }, alpha),
        );
        if selected {
            fill_round(
                pixmap,
                left + 3.0,
                top + 14.0,
                3.0,
                ROW_HEIGHT - 28.0,
                0.0,
                with_alpha(ACCENT, alpha),
            );
        }
        let ink = if selected {
            with_alpha(TEXT_DARK, alpha)
        } else {
            with_alpha(TEXT, alpha)
        };
        let dim = if selected {
            with_alpha(TEXT_DARK_DIM, alpha)
        } else {
            with_alpha(TEXT_DIM, alpha)
        };
        draw_icon(
            pixmap,
            Icon::Gamepad,
            left + 28.0,
            top + ROW_HEIGHT / 2.0,
            if active && !selected {
                with_alpha(ACCENT, alpha)
            } else {
                ink
            },
        );
        draw_text(pixmap, font, name, left + 52.0, top + 13.0, 16.5, ink);
        if active {
            draw_text_right(
                pixmap,
                font,
                "Actuel",
                left + width - 16.0,
                top + 18.0,
                12.0,
                if selected {
                    dim
                } else {
                    with_alpha(ACCENT, alpha)
                },
            );
        }
    }
}

/// Carré d'accès aux amis, dans le pied du panneau principal, avec le nombre
/// de contacts en ligne.
fn draw_friends_button(
    pixmap: &mut Pixmap,
    font: &FontVec,
    menu: &MenuState,
    panel_x: f32,
    panel_y: f32,
    panel_h: f32,
    alpha: u8,
) {
    let (x, y, width, height) = menu.friends_button_rect(panel_x, panel_y, panel_h);
    fill_round(
        pixmap,
        x,
        y,
        width,
        height,
        CARD_RADIUS,
        with_alpha((255, 255, 255, 16), alpha),
    );
    stroke_round(
        pixmap,
        x,
        y,
        width,
        height,
        CARD_RADIUS,
        with_alpha(BORDER, alpha),
    );
    draw_icon(
        pixmap,
        Icon::Friend,
        x + width / 2.0,
        y + height / 2.0,
        with_alpha(TEXT, alpha),
    );
    let online = menu
        .invite
        .friends
        .iter()
        .filter(|friend| friend.presence.online())
        .count();
    if online > 0 {
        fill_round(
            pixmap,
            x + width - 13.0,
            y + 2.0,
            14.0,
            14.0,
            7.0,
            with_alpha(ACCENT, alpha),
        );
        let label = if online > 9 {
            "9+".to_string()
        } else {
            online.to_string()
        };
        let label_width = text_width(font, &label, 9.0);
        draw_text(
            pixmap,
            font,
            &label,
            x + width - 13.0 + (14.0 - label_width) / 2.0,
            y + 4.5,
            9.0,
            with_alpha(TEXT_DARK, alpha),
        );
    }
}

/// Page « Amis » : liste des contacts puis conversation. Les cartes ne
/// portent plus d'invitation ; un clic ouvre les messages échangés avec
/// l'ami, et le champ du bas en envoie un nouveau.
fn draw_invite(
    pixmap: &mut Pixmap,
    font: &FontVec,
    menu: &MenuState,
    panel_x: f32,
    panel_y: f32,
    panel_w: f32,
    panel_h: f32,
    alpha: u8,
    avatars: &RefCell<HashMap<PathBuf, Option<Pixmap>>>,
) {
    if menu.chat_open() {
        draw_chat(pixmap, font, menu, panel_x, panel_y, panel_w, panel_h, alpha);
        return;
    }

    let left = panel_x + PADDING;
    let width = panel_w - 2.0 * PADDING;
    let list_w = menu.invite_list_width(panel_w);
    let row_h = menu.invite_row_height(panel_h);
    let count = menu.invite.friends.len();
    let online = menu
        .invite
        .friends
        .iter()
        .filter(|friend| friend.presence.online())
        .count();
    draw_back_button(pixmap, font, menu, panel_x, panel_y, panel_w, alpha);
    draw_text(
        pixmap,
        font,
        "Amis",
        left,
        panel_y + INVITE_TITLE_TOP,
        26.0,
        with_alpha(TEXT, alpha),
    );
    let subtitle = if menu.invite.live {
        format!(
            "{count} ami{} · {online} en ligne",
            if count > 1 { "s" } else { "" }
        )
    } else if count > 0 {
        // La liste reste affichée pendant une reconnexion : le sous-titre dit
        // pourquoi elle ne bouge plus, au lieu de laisser croire à une erreur.
        "Reconnexion à Steam…".to_string()
    } else {
        menu.invite
            .error
            .as_deref()
            .filter(|error| !error.trim().is_empty())
            .unwrap_or("Connexion à Steam…")
            .to_string()
    };
    draw_text(
        pixmap,
        font,
        &fit_text(font, &subtitle, width, 12.5),
        left,
        panel_y + INVITE_TITLE_TOP + 31.0,
        12.5,
        with_alpha(TEXT_DIM, alpha),
    );
    fill_rect(
        pixmap,
        left,
        panel_y + INVITE_TOP - 16.0,
        width,
        1.0,
        with_alpha(BORDER, alpha),
    );

    if menu.invite.friends.is_empty() {
        let top = panel_y + menu.invite_row_y(0, panel_h);
        let empty_h = row_h.max(88.0);
        fill_round(
            pixmap,
            left,
            top + 2.0,
            list_w,
            empty_h,
            CARD_RADIUS,
            with_alpha(ROW, alpha),
        );
        draw_icon(
            pixmap,
            Icon::Friend,
            left + 34.0,
            top + empty_h / 2.0,
            with_alpha(TEXT_DIM, alpha),
        );
        draw_text(
            pixmap,
            font,
            "Aucun ami Steam pour le moment",
            left + 62.0,
            top + 20.0,
            15.5,
            with_alpha(TEXT_DIM, alpha),
        );
        let detail = menu
            .invite
            .error
            .as_deref()
            .filter(|error| !error.trim().is_empty())
            .unwrap_or("La liste apparaîtra dès que la session sociale sera connectée.");
        draw_text(
            pixmap,
            font,
            &fit_text(font, detail, list_w - 78.0, 11.5),
            left + 62.0,
            top + 43.0,
            11.5,
            with_alpha(TEXT_DIM, alpha),
        );
    } else {
        let offset = menu.invite_offset();
        let avatar_size = (row_h * 0.72).clamp(32.0, 48.0);
        for (row, friend) in menu.invite_visible().iter().enumerate() {
            let index = offset + row;
            let top = panel_y + menu.invite_row_y(row, panel_h);
            let selected = index == menu.invite_selected;
            fill_round(
                pixmap,
                left,
                top + 2.0,
                list_w,
                row_h - 4.0,
                CARD_RADIUS,
                with_alpha(if selected { SELECTED } else { ROW }, alpha),
            );
            if selected {
                fill_round(
                    pixmap,
                    left + 3.0,
                    top + 12.0,
                    3.0,
                    row_h - 24.0,
                    0.0,
                    with_alpha(ACCENT, alpha),
                );
            }
            let ink = if selected {
                with_alpha(TEXT_DARK, alpha)
            } else if friend.presence.online() {
                with_alpha(TEXT, alpha)
            } else {
                with_alpha(TEXT_DIM, alpha)
            };
            let dim = if selected {
                with_alpha(TEXT_DARK_DIM, alpha)
            } else {
                with_alpha(TEXT_DIM, alpha)
            };
            let avatar_left = left + 14.0;
            let avatar_top = top + (row_h - avatar_size) / 2.0;
            let mut cache = avatars.borrow_mut();
            let avatar = menu
                .invite
                .avatars
                .get(&friend.steam_id)
                .and_then(|path| {
                    cache
                        .entry(path.clone())
                        .or_insert_with(|| decode_image(path))
                        .as_ref()
                });
            if let Some(image) = avatar {
                draw_cover(
                    pixmap,
                    image,
                    avatar_left,
                    avatar_top,
                    avatar_size,
                    avatar_size,
                    avatar_size / 2.0,
                    if friend.presence.online() {
                        alpha
                    } else {
                        (alpha as u16 * 3 / 4) as u8
                    },
                );
            } else {
                fill_round(
                    pixmap,
                    avatar_left,
                    avatar_top,
                    avatar_size,
                    avatar_size,
                    avatar_size / 2.0,
                    with_alpha(
                        if selected {
                            (0, 0, 0, 30)
                        } else {
                            (255, 255, 255, 28)
                        },
                        alpha,
                    ),
                );
                let initial = friend
                    .name
                    .trim()
                    .chars()
                    .next()
                    .map(|ch| ch.to_uppercase().collect::<String>())
                    .unwrap_or_else(|| "?".into());
                let initial_width = text_width(font, &initial, 15.0);
                draw_text(
                    pixmap,
                    font,
                    &initial,
                    avatar_left + (avatar_size - initial_width) / 2.0,
                    avatar_top + (avatar_size - 15.0) / 2.0,
                    15.0,
                    ink,
                );
            }
            drop(cache);
            if friend.presence.online() {
                fill_round(
                    pixmap,
                    avatar_left + avatar_size - 6.0,
                    avatar_top + avatar_size - 6.0,
                    6.0,
                    6.0,
                    3.0,
                    with_alpha((74, 222, 128, 230), alpha),
                );
            }
            let text_left = avatar_left + avatar_size + 14.0;
            let text_w = (list_w - (text_left - left) - 30.0).max(80.0);
            let name = fit_text(font, &menu.invite_friend_label(index), text_w, 16.0);
            draw_text(
                pixmap,
                font,
                &name,
                text_left,
                top + (row_h - 16.0) / 2.0 - 7.0,
                16.0,
                ink,
            );
            let preview = menu.last_message(friend.steam_id).map(|message| {
                let prefix = if message.outgoing { "Vous : " } else { "" };
                format!("{prefix}{}", message.text.replace('\n', " "))
            });
            let state = fit_text(
                font,
                &preview.unwrap_or_else(|| friend.state_line()),
                text_w,
                11.5,
            );
            draw_text(
                pixmap,
                font,
                &state,
                text_left,
                top + (row_h - 11.5) / 2.0 + 8.0,
                11.5,
                dim,
            );
            // Petit chevron : la carte entière ouvre la conversation.
            let cx = left + list_w - 20.0;
            let cy = top + row_h / 2.0;
            let mut chevron = PathBuilder::new();
            chevron.move_to(cx - 2.0, cy - 5.0);
            chevron.line_to(cx + 3.0, cy);
            chevron.line_to(cx - 2.0, cy + 5.0);
            stroke_path(pixmap, chevron, dim, 1.6);
        }
    }

    if let (Some((track_x, track_y, track_w, track_h)), Some((thumb_x, thumb_y, thumb_w, thumb_h))) = (
        menu.invite_scrollbar(panel_x, panel_y, panel_w, panel_h),
        menu.invite_scrollbar_thumb(panel_x, panel_y, panel_w, panel_h),
    ) {
        fill_round(
            pixmap,
            track_x,
            track_y,
            track_w,
            track_h,
            1.5,
            with_alpha((255, 255, 255, 28), alpha),
        );
        fill_round(
            pixmap,
            thumb_x,
            thumb_y,
            thumb_w,
            thumb_h,
            1.5,
            with_alpha(ACCENT, alpha),
        );
    }
}

/// Conversation avec un ami : les messages de la session, puis le champ de
/// saisie. Les messages sortants sont alignés à droite, comme dans les
/// messageries habituelles.
fn draw_chat(
    pixmap: &mut Pixmap,
    font: &FontVec,
    menu: &MenuState,
    panel_x: f32,
    panel_y: f32,
    panel_w: f32,
    panel_h: f32,
    alpha: u8,
) {
    let left = panel_x + PADDING;
    let width = panel_w - 2.0 * PADDING;
    draw_back_button(pixmap, font, menu, panel_x, panel_y, panel_w, alpha);
    let friend = menu.chat_friend_name();
    draw_text(
        pixmap,
        font,
        &fit_text(font, &friend, width, 22.0),
        left,
        panel_y + INVITE_TITLE_TOP,
        22.0,
        with_alpha(TEXT, alpha),
    );
    draw_text(
        pixmap,
        font,
        &fit_text(font, &menu.chat_friend_status(), width, 11.5),
        left,
        panel_y + INVITE_TITLE_TOP + 28.0,
        11.5,
        with_alpha(TEXT_DIM, alpha),
    );
    fill_rect(
        pixmap,
        left,
        panel_y + INVITE_TOP - 16.0,
        width,
        1.0,
        with_alpha(BORDER, alpha),
    );

    let composer_top = panel_y + panel_h - FOOTER_HEIGHT - CHAT_COMPOSER_HEIGHT - 8.0;
    let area_top = panel_y + INVITE_TOP;
    let area_bottom = composer_top - 10.0;
    let messages = menu.chat_messages();
    if messages.is_empty() {
        draw_text_center(
            pixmap,
            font,
            "Aucun message pour le moment",
            panel_x + panel_w / 2.0,
            area_top + 24.0,
            14.0,
            with_alpha(TEXT_DIM, alpha),
        );
        draw_text_center(
            pixmap,
            font,
            "Écris le premier message ci-dessous.",
            panel_x + panel_w / 2.0,
            area_top + 45.0,
            11.5,
            with_alpha(TEXT_DIM, alpha),
        );
    } else {
        struct Bubble {
            top: f32,
            outgoing: bool,
            label: String,
            lines: Vec<String>,
            width: f32,
        }
        let bubble_max = width * 0.72;
        let mut bubbles: Vec<Bubble> = Vec::new();
        let mut bottom = area_bottom;
        for message in messages.iter().rev().take(40) {
            let label = if message.outgoing {
                "Vous".to_string()
            } else {
                friend.clone()
            };
            let text = message.text.replace('\n', " ");
            let lines = wrap_text(font, &text, bubble_max - 24.0, 13.5);
            let height = 27.0 + lines.len() as f32 * 18.0;
            if bottom - height < area_top {
                break;
            }
            let widest = lines
                .iter()
                .map(|line| text_width(font, line, 13.5))
                .fold(text_width(font, &label, 10.5), f32::max);
            let bubble_w = (widest + 24.0).clamp(110.0, bubble_max);
            bottom -= height;
            bubbles.push(Bubble {
                top: bottom,
                outgoing: message.outgoing,
                label,
                lines,
                width: bubble_w,
            });
            bottom -= 8.0;
        }
        for bubble in bubbles.iter().rev() {
            let x = if bubble.outgoing {
                left + width - bubble.width
            } else {
                left
            };
            fill_round(
                pixmap,
                x,
                bubble.top,
                bubble.width,
                27.0 + bubble.lines.len() as f32 * 18.0,
                10.0,
                with_alpha(
                    if bubble.outgoing { ACCENT } else { ROW_PRIMARY },
                    alpha,
                ),
            );
            let (ink, dim) = if bubble.outgoing {
                (with_alpha(TEXT_DARK, alpha), with_alpha(TEXT_DARK_DIM, alpha))
            } else {
                (with_alpha(TEXT, alpha), with_alpha(TEXT_DIM, alpha))
            };
            draw_text(
                pixmap,
                font,
                &fit_text(font, &bubble.label, bubble.width - 24.0, 10.5),
                x + 12.0,
                bubble.top + 7.0,
                10.5,
                dim,
            );
            for (index, line) in bubble.lines.iter().enumerate() {
                draw_text(
                    pixmap,
                    font,
                    line,
                    x + 12.0,
                    bubble.top + 22.0 + index as f32 * 18.0,
                    13.5,
                    ink,
                );
            }
        }
    }

    fill_round(
        pixmap,
        left,
        composer_top,
        width,
        CHAT_COMPOSER_HEIGHT,
        12.0,
        with_alpha(ROW_PRIMARY, alpha),
    );
    stroke_round(
        pixmap,
        left,
        composer_top,
        width,
        CHAT_COMPOSER_HEIGHT,
        12.0,
        with_alpha(BORDER, alpha),
    );
    let empty = menu.chat_input.is_empty();
    let shown = if empty {
        String::new()
    } else {
        fit_text(font, &menu.chat_input, width - 34.0, 14.0)
    };
    let text_left = left + 16.0;
    let text_top = composer_top + (CHAT_COMPOSER_HEIGHT - 14.0) / 2.0 - 2.0;
    if empty {
        draw_text(
            pixmap,
            font,
            "Écris un message…",
            text_left,
            text_top,
            14.0,
            with_alpha(TEXT_DIM, alpha),
        );
    } else {
        draw_text(
            pixmap,
            font,
            &shown,
            text_left,
            text_top,
            14.0,
            with_alpha(TEXT, alpha),
        );
    }
    let caret_x = text_left + text_width(font, &shown, 14.0);
    fill_rect(
        pixmap,
        caret_x + 1.0,
        composer_top + 15.0,
        1.5,
        20.0,
        with_alpha(ACCENT, alpha),
    );
}

/// Découpe un texte en lignes qui tiennent dans `max_width`. Les mots trop
/// longs sont coupés caractère par caractère plutôt que de déborder.
fn wrap_text(font: &FontVec, text: &str, max_width: f32, size: f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let candidate = if line.is_empty() {
            word.to_string()
        } else {
            format!("{line} {word}")
        };
        if text_width(font, &candidate, size) <= max_width {
            line = candidate;
            continue;
        }
        if !line.is_empty() {
            lines.push(std::mem::take(&mut line));
        }
        if text_width(font, word, size) <= max_width {
            line = word.to_string();
            continue;
        }
        for ch in word.chars() {
            let mut next = line.clone();
            next.push(ch);
            if text_width(font, &next, size) <= max_width {
                line = next;
            } else {
                if !line.is_empty() {
                    lines.push(std::mem::take(&mut line));
                }
                line.push(ch);
            }
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Une ligne du panneau Performances : pictogramme, titre, sous-titre et
/// valeur. Le profil actif et le HUD allumé portent une valeur verte.
fn draw_perf_card(
    pixmap: &mut Pixmap,
    font: &FontVec,
    menu: &MenuState,
    row: usize,
    x: f32,
    y: f32,
    w: f32,
    alpha: u8,
) {
    let selected = if menu.page == Page::Recordings {
        menu.recording_selected == row
    } else {
        menu.perf_selected == row
    };
    let active = if menu.page == Page::Recordings {
        match row {
            0 => menu.live.recording.is_some_and(|state| !state.paused),
            2 => menu.record_badge,
            _ => false,
        }
    } else {
        match row {
            0 => menu.perf_hud || menu.hud_on,
            2 => menu.hud_on,
            3..=5 => menu.hud_on && menu.hud == HudPreset::ALL[row - 3],
            8 => menu.hud_graph,
            9 => menu.hud_cores,
            _ => false,
        }
    };
    let card_y = y + 2.0;
    let card_h = PERF_ROW - 4.0;
    let (fill, title_color, icon_color, value_color) = if selected {
        (
            with_alpha(SELECTED, alpha),
            with_alpha(TEXT_DARK, alpha),
            with_alpha(TEXT_DARK, alpha),
            with_alpha(TEXT_DARK_DIM, alpha),
        )
    } else {
        (
            with_alpha(ROW, alpha),
            with_alpha(TEXT, alpha),
            with_alpha(TEXT, alpha),
            if active {
                with_alpha((74, 222, 128, 255), alpha)
            } else {
                with_alpha(TEXT_DIM, alpha)
            },
        )
    };
    fill_round(pixmap, x, card_y, w, card_h, CARD_RADIUS, fill);

    let glyph = if menu.page == Page::Recordings {
        match row {
            0 => Icon::Play,
            1 => Icon::Cross,
            _ => Icon::Film,
        }
    } else {
        match row {
            0 => Icon::Screen,
            1 => Icon::Gauge,
            2 => Icon::Screen,
            3..=5 => Icon::Chip,
            _ => Icon::Screen,
        }
    };
    draw_icon(pixmap, glyph, x + 30.0, card_y + card_h / 2.0, icon_color);

    let title = menu.perf_label(row);
    let subtitle = menu.perf_subtitle(row);
    let title_y = if subtitle.is_empty() {
        card_y + 12.0
    } else {
        card_y + 8.0
    };
    draw_text(pixmap, font, &title, x + 52.0, title_y, 16.5, title_color);
    if !subtitle.is_empty() {
        draw_text(
            pixmap,
            font,
            &subtitle,
            x + 52.0,
            card_y + 28.0,
            11.0,
            value_color,
        );
    }

    let toggle = if menu.page == Page::Recordings {
        row == 2
    } else {
        row == 0 || row == 8 || row == 9
    };
    if toggle {
        // Un interrupteur se lit d'un coup d'œil : la valeur textuelle
        // (« Activé »/« Désactivé ») n'est plus nécessaire.
        draw_switch(
            pixmap,
            x + w - 16.0 - 38.0,
            card_y + card_h / 2.0 - 10.0,
            active,
            selected,
            alpha,
        );
    } else {
        let value = menu.perf_value(row);
        if !value.is_empty() {
            let text = fit_text(font, &value, 150.0, 14.0);
            draw_text_right(
                pixmap,
                font,
                &text,
                x + w - 16.0,
                card_y + if subtitle.is_empty() { 15.0 } else { 12.0 },
                14.0,
                value_color,
            );
        }
    }
}

/// Sélecteur Natif / MangoHud de la ligne « Moteur du HUD ». Les deux
/// boutons occupent la droite de la carte, à la place d'une valeur unique.
fn draw_engine_buttons(
    pixmap: &mut Pixmap,
    font: &FontVec,
    menu: &MenuState,
    x: f32,
    row_top: f32,
    w: f32,
    alpha: u8,
) {
    let card_y = row_top + 2.0;
    let card_h = PERF_ROW - 4.0;
    let right = x + w - 16.0;
    let left = right - (2.0 * PERF_ENGINE_W + PERF_ENGINE_GAP);
    let top = card_y + (card_h - 26.0) / 2.0;
    // Sur la carte blanche de la ligne sélectionnée, un bouton actif blanc
    // disparaîtrait : il passe alors en encre sombre.
    let selected_row = menu.perf_selected == 2;
    for (index, (label, active)) in [
        ("Natif", matches!(menu.hud_engine, crate::menu::HudEngine::Native)),
        ("MangoHud", matches!(menu.hud_engine, crate::menu::HudEngine::MangoHud)),
    ]
    .iter()
    .enumerate()
    {
        let bx = left + index as f32 * (PERF_ENGINE_W + PERF_ENGINE_GAP);
        let (fill, ink) = if *active {
            if selected_row {
                ((24, 26, 30, 255), (250, 250, 252, 255))
            } else {
                (SELECTED, TEXT_DARK)
            }
        } else if selected_row {
            ((24, 26, 30, 40), (78, 80, 85, 255))
        } else {
            (ROW_PRIMARY, TEXT_DIM)
        };
        fill_round(
            pixmap,
            bx,
            top,
            PERF_ENGINE_W,
            26.0,
            4.0,
            with_alpha(fill, alpha),
        );
        if *active && !selected_row {
            stroke_round(
                pixmap,
                bx,
                top,
                PERF_ENGINE_W,
                26.0,
                4.0,
                with_alpha(BORDER, alpha),
            );
        }
        draw_text(
            pixmap,
            font,
            label,
            bx + 10.0,
            top + 7.0,
            12.5,
            with_alpha(ink, alpha),
        );
    }
}

/// Piste de la limite de FPS : un vrai curseur, étalé sous le titre de la
/// carte. Chaque cran est marqué d'une graduation, le crayon porte la valeur
/// choisie — un clic ou un glissement sur la piste change la limite.
fn draw_fps_track(
    pixmap: &mut Pixmap,
    _font: &FontVec,
    menu: &MenuState,
    x: f32,
    row_top: f32,
    w: f32,
    alpha: u8,
) {
    let selected = menu.perf_selected == 1;
    let card_y = row_top + 2.0;
    let card_h = PERF_ROW - 4.0;
    let left = x + FPS_TRACK_LEFT;
    let right = x + w - FPS_TRACK_RIGHT;
    let track_w = (right - left).max(1.0);
    let track_y = card_y + card_h - 12.0;
    // La carte sélectionnée est claire : la piste doit alors s'assombrir au
    // lieu de blanchir, sinon le curseur disparaît.
    let track_color = if selected {
        with_alpha((24, 26, 30, 38), alpha)
    } else {
        with_alpha((255, 255, 255, 30), alpha)
    };
    let fill_color = if selected {
        with_alpha((24, 26, 30, 255), alpha)
    } else {
        with_alpha((236, 237, 239, 190), alpha)
    };
    let tick_color = if selected {
        with_alpha((24, 26, 30, 46), alpha)
    } else {
        with_alpha((255, 255, 255, 40), alpha)
    };
    fill_round(pixmap, left, track_y, track_w, 3.0, 1.5, track_color);
    let ratio = menu.fps_ratio();
    fill_round(
        pixmap,
        left,
        track_y,
        track_w * ratio,
        3.0,
        1.5,
        fill_color,
    );
    for index in 0..FPS_STEPS.len() {
        let tick = left + track_w * index as f32 / (FPS_STEPS.len() - 1) as f32;
        fill_round(
            pixmap,
            tick - 0.5,
            track_y + 5.0,
            1.0,
            3.0,
            0.5,
            tick_color,
        );
    }
    let knob_x = left + track_w * ratio;
    fill_round(pixmap, knob_x - 4.0, track_y - 3.5, 8.0, 10.0, 4.0, fill_color);
}

/// Bouton Retour, au sommet du panneau Performances : une flèche et un
/// libellé, comme une ligne de menu.
fn draw_back_button(
    pixmap: &mut Pixmap,
    font: &FontVec,
    menu: &MenuState,
    panel_x: f32,
    panel_y: f32,
    panel_w: f32,
    alpha: u8,
) {
    let selected = menu.perf_selected == usize::MAX;
    let (x, y, w, h) = menu.perf_back_rect(panel_x, panel_y, panel_w);
    fill_round(
        pixmap,
        x,
        y,
        w,
        h,
        CARD_RADIUS,
        with_alpha(if selected { SELECTED } else { ROW }, alpha),
    );
    let ink = with_alpha(if selected { TEXT_DARK } else { TEXT }, alpha);
    let cy = y + h / 2.0;
    let mut arrow = PathBuilder::new();
    arrow.move_to(x + 24.0, cy);
    arrow.line_to(x + 31.0, cy - 6.0);
    arrow.move_to(x + 24.0, cy);
    arrow.line_to(x + 31.0, cy + 6.0);
    stroke_path(pixmap, arrow, ink, 1.8);
    draw_text(pixmap, font, "Retour", x + 42.0, cy - 7.0, 16.0, ink);
    let destination = if menu.page == Page::Invite && menu.chat_open() {
        "Amis"
    } else {
        "Menu du jeu"
    };
    draw_text_right(
        pixmap,
        font,
        destination,
        x + w - 16.0,
        cy - 6.0,
        12.5,
        with_alpha(TEXT_DIM, alpha),
    );
}

/// Interrupteur à glissière des lignes MangoHud et graphique de cadence.
fn draw_switch(pixmap: &mut Pixmap, x: f32, y: f32, on: bool, selected: bool, alpha: u8) {
    const W: f32 = 38.0;
    const H: f32 = 20.0;
    let track = if on {
        with_alpha((74, 222, 128, 230), alpha)
    } else if selected {
        with_alpha((24, 26, 30, 32), alpha)
    } else {
        with_alpha((255, 255, 255, 46), alpha)
    };
    fill_round(pixmap, x, y, W, H, H / 2.0, track);
    let knob_x = if on { x + W - H + 2.0 } else { x + 2.0 };
    let knob = if selected && !on {
        with_alpha((86, 90, 98, 255), alpha)
    } else {
        with_alpha((252, 252, 253, 255), alpha)
    };
    fill_round(
        pixmap,
        knob_x,
        y + 2.0,
        H - 4.0,
        H - 4.0,
        (H - 4.0) / 2.0,
        knob,
    );
}

/// Titre de section du panneau, suivi d'un filet jusqu'au bord.
fn draw_section_label(
    pixmap: &mut Pixmap,
    font: &FontVec,
    x: f32,
    y: f32,
    width: f32,
    label: &str,
    alpha: u8,
) {
    draw_text(
        pixmap,
        font,
        label,
        x + 2.0,
        y,
        11.0,
        with_alpha(ACCENT, alpha),
    );
    let start = x + 2.0 + text_width(font, label, 11.0) + 10.0;
    if start < x + width {
        fill_rect(
            pixmap,
            start,
            y + 8.0,
            x + width - start,
            1.0,
            with_alpha(BORDER, alpha),
        );
    }
}

/// Grille 3 × 3 des ancrages MangoHud. Chaque case montre un écran en
/// miniature avec le HUD posé là où il tombera ; la position choisie est
/// blanche, et un clic sur une case la sélectionne directement.
fn draw_position_grid(
    pixmap: &mut Pixmap,
    font: &FontVec,
    menu: &MenuState,
    x: f32,
    top: f32,
    width: f32,
    record: bool,
    alpha: u8,
) {
    const GAP: f32 = 4.0;
    const SCREEN_W: f32 = 50.0;
    const SCREEN_H: f32 = 26.0;
    let cell_w = (width - 2.0 * GAP) / 3.0;
    let cell_h = (PERF_GRID - 2.0 * GAP) / 3.0;
    for (index, position) in HudPosition::ALL.iter().enumerate() {
        let col = (index % 3) as f32;
        let row = (index / 3) as f32;
        let cell_x = x + col * (cell_w + GAP);
        let cell_y = top + row * (cell_h + GAP);
        let selected = if record {
            menu.record_position == *position
        } else {
            menu.hud_position == *position
        };
        let (fill, border, ink) = if selected {
            (
                with_alpha(SELECTED, alpha),
                with_alpha(SELECTED, alpha),
                with_alpha(TEXT_DARK, alpha),
            )
        } else {
            (
                with_alpha(ROW, alpha),
                with_alpha(BORDER, alpha),
                with_alpha(TEXT_DIM, alpha),
            )
        };
        fill_round(pixmap, cell_x, cell_y, cell_w, cell_h, 0.0, fill);
        stroke_round(pixmap, cell_x, cell_y, cell_w, cell_h, 0.0, border);
        draw_position_preview(
            pixmap,
            menu,
            index,
            cell_x + 8.0,
            cell_y + (cell_h - SCREEN_H) / 2.0,
            record,
            alpha,
        );
        draw_text(
            pixmap,
            font,
            position.label(),
            cell_x + 8.0 + SCREEN_W + 10.0,
            cell_y + cell_h / 2.0 + 4.0,
            11.0,
            ink,
        );
    }
}

/// Écran en miniature d'une case de la grille : un fond sombre et la barre du
/// HUD à l'ancre correspondante. Sa forme suit la disposition choisie — une
/// ligne fine en mode « en ligne », un petit bloc sinon — pour que l'aperçu
/// dise aussi ce qui sera affiché.
fn draw_position_preview(
    pixmap: &mut Pixmap,
    menu: &MenuState,
    index: usize,
    x: f32,
    y: f32,
    record: bool,
    alpha: u8,
) {
    const SCREEN_W: f32 = 50.0;
    const SCREEN_H: f32 = 26.0;
    const MARGIN: f32 = 4.0;
    // L'écran reste sombre dans les deux états : sur la case blanche, il fait
    // ressortir le contour et la barre au lieu de se fondre dans le fond.
    fill_rect(
        pixmap,
        x,
        y,
        SCREEN_W,
        SCREEN_H,
        with_alpha((0, 0, 0, 150), alpha),
    );
    stroke_round(
        pixmap,
        x,
        y,
        SCREEN_W,
        SCREEN_H,
        2.0,
        with_alpha((255, 255, 255, 64), alpha),
    );
    let (bar_w, bar_h) = if record {
        (18.0, 5.0)
    } else {
        match menu.hud_layout {
            HudLayout::Inline => (20.0, 3.0),
            HudLayout::Block => (12.0, 7.0),
        }
    };
    let bar_x = match index % 3 {
        0 => x + MARGIN,
        1 => x + (SCREEN_W - bar_w) / 2.0,
        _ => x + SCREEN_W - MARGIN - bar_w,
    };
    let bar_y = match index / 3 {
        0 => y + MARGIN,
        1 => y + (SCREEN_H - bar_h) / 2.0,
        _ => y + SCREEN_H - MARGIN - bar_h,
    };
    fill_rect(
        pixmap,
        bar_x,
        bar_y,
        bar_w,
        bar_h,
        with_alpha(ACCENT, alpha),
    );
}

/// Bandeau du bas : ce que le jeu expose en DLSS.
fn draw_dlss_card(
    pixmap: &mut Pixmap,
    font: &FontVec,
    menu: &MenuState,
    x: f32,
    y: f32,
    width: f32,
    alpha: u8,
) {
    fill_round(
        pixmap,
        x,
        y,
        width,
        PERF_INFO,
        CARD_RADIUS,
        with_alpha(ROW, alpha),
    );
    draw_icon(
        pixmap,
        Icon::Chip,
        x + 24.0,
        y + PERF_INFO / 2.0,
        with_alpha(TEXT_DIM, alpha),
    );
    let text = match menu.live.dlss {
        Some(info) if !info.is_empty() => format!("DLSS · {}", info.summary()),
        _ => "DLSS · non détecté".to_string(),
    };
    draw_text(
        pixmap,
        font,
        &text,
        x + 46.0,
        y + 13.0,
        13.0,
        with_alpha(TEXT_DIM, alpha),
    );
}

fn draw_icon(pixmap: &mut Pixmap, glyph: Icon, cx: f32, cy: f32, color: (u8, u8, u8, u8)) {
    let mut builder = PathBuilder::new();
    match glyph {
        Icon::Play => {
            builder.move_to(cx - 4.5, cy - 7.0);
            builder.line_to(cx + 7.0, cy);
            builder.line_to(cx - 4.5, cy + 7.0);
            builder.close();
            fill_path(pixmap, builder, color);
        }
        Icon::Gauge => {
            // Trois barres croissantes : la cadence.
            for (index, height) in [6.0f32, 10.0, 14.0].iter().enumerate() {
                fill_round(
                    pixmap,
                    cx - 7.4 + index as f32 * 5.6,
                    cy + 7.0 - height,
                    3.4,
                    *height,
                    1.2,
                    color,
                );
            }
        }
        Icon::Screen => {
            let mut frame = PathBuilder::new();
            push_round(&mut frame, cx - 8.5, cy - 7.0, 17.0, 11.5, 2.6);
            stroke_path(pixmap, frame, color, 1.5);
            let mut stand = PathBuilder::new();
            stand.move_to(cx, cy + 4.5);
            stand.line_to(cx, cy + 8.0);
            stand.move_to(cx - 3.5, cy + 8.0);
            stand.line_to(cx + 3.5, cy + 8.0);
            stroke_path(pixmap, stand, color, 1.5);
        }
        Icon::Chip => {
            let mut body = PathBuilder::new();
            push_round(&mut body, cx - 6.4, cy - 5.4, 12.8, 10.8, 1.2);
            stroke_path(pixmap, body, color, 1.3);
            let mut pins = PathBuilder::new();
            for offset in [-3.0f32, 0.0, 3.0] {
                pins.move_to(cx + offset, cy - 8.0);
                pins.line_to(cx + offset, cy - 5.4);
                pins.move_to(cx + offset, cy + 5.4);
                pins.line_to(cx + offset, cy + 8.0);
            }
            for offset in [-2.0f32, 2.0] {
                pins.move_to(cx - 9.0, cy + offset);
                pins.line_to(cx - 6.4, cy + offset);
                pins.move_to(cx + 6.4, cy + offset);
                pins.line_to(cx + 9.0, cy + offset);
            }
            stroke_path(pixmap, pins, color, 1.1);
        }
        Icon::Gamepad => {
            let mut frame = PathBuilder::new();
            push_round(&mut frame, cx - 8.0, cy - 5.0, 16.0, 10.0, 4.6);
            stroke_path(pixmap, frame, color, 1.4);
            let mut dots = PathBuilder::new();
            dots.push_circle(cx - 3.6, cy, 1.3);
            dots.push_circle(cx + 3.6, cy, 1.3);
            fill_path(pixmap, dots, color);
        }
        Icon::Flask => {
            let mut flask = PathBuilder::new();
            flask.move_to(cx - 2.4, cy - 7.0);
            flask.line_to(cx - 2.4, cy - 2.2);
            flask.line_to(cx - 6.4, cy + 6.0);
            flask.line_to(cx + 6.4, cy + 6.0);
            flask.line_to(cx + 2.4, cy - 2.2);
            flask.line_to(cx + 2.4, cy - 7.0);
            flask.move_to(cx - 4.4, cy - 7.0);
            flask.line_to(cx + 4.4, cy - 7.0);
            stroke_path(pixmap, flask, color, 1.4);
        }
        Icon::Friend => {
            let mut head = PathBuilder::new();
            head.push_circle(cx - 2.0, cy - 4.2, 3.0);
            stroke_path(pixmap, head, color, 1.4);
            let mut shoulders = PathBuilder::new();
            shoulders.move_to(cx - 7.0, cy + 7.0);
            shoulders.quad_to(cx - 6.0, cy + 1.0, cx - 1.0, cy + 1.0);
            shoulders.quad_to(cx + 3.0, cy + 1.0, cx + 4.0, cy + 6.0);
            stroke_path(pixmap, shoulders, color, 1.4);
            // Le second compagnon, décalé, donne le pictogramme « groupe ».
            let mut plus_head = PathBuilder::new();
            plus_head.push_circle(cx + 6.0, cy - 2.5, 2.2);
            stroke_path(pixmap, plus_head, color, 1.2);
            let mut plus_body = PathBuilder::new();
            plus_body.move_to(cx + 3.0, cy + 6.5);
            plus_body.quad_to(cx + 5.0, cy + 2.5, cx + 8.0, cy + 3.5);
            stroke_path(pixmap, plus_body, color, 1.2);
        }
        Icon::Camera => {
            let mut body = PathBuilder::new();
            push_round(&mut body, cx - 8.0, cy - 5.0, 16.0, 11.0, 3.0);
            stroke_path(pixmap, body, color, 1.4);
            let mut lens = PathBuilder::new();
            lens.push_circle(cx, cy + 0.5, 2.7);
            stroke_path(pixmap, lens, color, 1.4);
            let mut bump = PathBuilder::new();
            push_round(&mut bump, cx - 3.2, cy - 7.6, 6.4, 2.6, 1.0);
            fill_path(pixmap, bump, color);
        }
        Icon::Speaker => {
            let mut body = PathBuilder::new();
            body.move_to(cx - 7.0, cy - 3.0);
            body.line_to(cx - 2.8, cy - 3.0);
            body.line_to(cx + 2.0, cy - 7.0);
            body.line_to(cx + 2.0, cy + 7.0);
            body.line_to(cx - 2.8, cy + 3.0);
            body.line_to(cx - 7.0, cy + 3.0);
            body.close();
            fill_path(pixmap, body, color);
            fill_round(pixmap, cx + 4.5, cy - 3.0, 1.6, 6.0, 0.8, color);
            fill_round(pixmap, cx + 8.5, cy - 6.0, 1.6, 12.0, 0.8, color);
        }
        Icon::Film => {
            let mut body = PathBuilder::new();
            push_round(&mut body, cx - 7.5, cy - 5.5, 15.0, 11.0, 1.6);
            stroke_path(pixmap, body, color, 1.3);
            let mut holes = PathBuilder::new();
            for offset in [-4.5f32, -1.5, 1.5, 4.5] {
                holes.push_circle(cx + offset, cy - 3.2, 0.9);
                holes.push_circle(cx + offset, cy + 3.2, 0.9);
            }
            fill_path(pixmap, holes, color);
            let mut play = PathBuilder::new();
            play.move_to(cx - 1.8, cy - 2.2);
            play.line_to(cx + 3.2, cy);
            play.line_to(cx - 1.8, cy + 2.2);
            play.close();
            fill_path(pixmap, play, color);
        }
        Icon::Cross => {
            let mut cross = PathBuilder::new();
            cross.move_to(cx - 5.4, cy - 5.4);
            cross.line_to(cx + 5.4, cy + 5.4);
            cross.move_to(cx + 5.4, cy - 5.4);
            cross.line_to(cx - 5.4, cy + 5.4);
            stroke_path(pixmap, cross, color, 1.8);
        }
    }
}

fn key_hint(
    pixmap: &mut Pixmap,
    font: &FontVec,
    x: f32,
    y: f32,
    key: &str,
    label: &str,
    alpha: u8,
) -> f32 {
    let key_w = (text_width(font, key, 11.0) + 14.0).max(22.0);
    fill_round(
        pixmap,
        x,
        y,
        key_w,
        22.0,
        0.0,
        with_alpha((255, 255, 255, 26), alpha),
    );
    draw_text_center(
        pixmap,
        font,
        key,
        x + key_w / 2.0,
        y + 5.5,
        11.0,
        with_alpha(TEXT, alpha),
    );
    draw_text(
        pixmap,
        font,
        label,
        x + key_w + 6.0,
        y + 4.5,
        12.0,
        with_alpha(TEXT_DIM, alpha),
    );
    x + key_w + 6.0 + text_width(font, label, 12.0) + 12.0
}

/// Position du panneau pour l'image courante. L'ouverture part du bord
/// gauche, entièrement hors écran, puis glisse jusqu'à sa marge.
pub fn panel_origin(width: u32, height: u32, menu: &MenuState) -> (f32, f32, f32, f32) {
    let ease = ease_out(menu.anim);
    // Les amis s'ouvrent dans une fenêtre compacte au-dessus du jeu, pas dans
    // un écran plein : le carré du pied de page reste le point d'entrée.
    if menu.page == Page::Invite {
        let panel_w = (width as f32 - 48.0).min(FRIENDS_WIDTH).max(280.0);
        let panel_h = (height as f32 - 48.0).min(FRIENDS_HEIGHT).max(360.0);
        let panel_x = (width as f32 - panel_w) / 2.0;
        let panel_y = (height as f32 - panel_h) / 2.0 + (1.0 - ease) * 24.0;
        return (panel_x, panel_y, panel_w, panel_h);
    }
    let panel_h = menu.panel_height();
    let panel_w = PANEL_WIDTH.min(width as f32 - 64.0);
    let panel_x = PANEL_MARGIN - (1.0 - ease) * (PANEL_MARGIN + panel_w);
    let panel_y = ((height as f32 - panel_h) / 2.0).max(14.0);
    (panel_x, panel_y, panel_w, panel_h)
}

/// Rectangle du panneau, animation comprise : les clics visent ce que l'on
/// voit, même pendant l'entrée.
pub fn panel_bounds(width: u32, height: u32, menu: &MenuState) -> (f32, f32, f32, f32) {
    panel_origin(width, height, menu)
}

pub(crate) fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// `mm:ss`, ou `h:mm:ss` au-delà de l'heure.
fn clock(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

pub(crate) fn with_alpha(rgba: (u8, u8, u8, u8), alpha: u8) -> (u8, u8, u8, u8) {
    (
        rgba.0,
        rgba.1,
        rgba.2,
        ((rgba.3 as u32 * alpha as u32) / 255).min(255) as u8,
    )
}

/// Décode une image (JPEG ou PNG) en pixmap prémultiplié.
pub(crate) fn decode_image(path: &Path) -> Option<Pixmap> {
    decode_bytes(&std::fs::read(path).ok()?)
}

/// Même décodage, depuis des octets : les marques de boutique sont embarquées
/// dans le binaire et n'ont pas de fichier à ouvrir.
pub(crate) fn decode_bytes(bytes: &[u8]) -> Option<Pixmap> {
    let image = image::load_from_memory(bytes).ok()?;
    // Les bannières Steam font 1920 px de large : un prérédu en une passe
    // évite l'aliasing du texte pendant l'échantillonnage par trame.
    let (src_w, src_h) = (image.width(), image.height());
    let image = if src_w > 900 {
        let height = (src_h as f64 * 900.0 / src_w as f64).round().max(1.0) as u32;
        image.thumbnail(900, height)
    } else {
        image
    };
    let image = image.to_rgba8();
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 {
        return None;
    }
    let mut data = Vec::with_capacity((width * height * 4) as usize);
    for pixel in image.pixels() {
        let alpha = pixel[3] as u32;
        let premultiply = |value: u8| ((value as u32 * alpha + 127) / 255).min(255) as u8;
        data.extend_from_slice(&[
            premultiply(pixel[0]),
            premultiply(pixel[1]),
            premultiply(pixel[2]),
            pixel[3],
        ]);
    }
    Pixmap::from_vec(data, IntSize::from_wh(width, height)?)
}

/// Dessine `src` en couvrant le rectangle (`object-fit: cover`), coins arrondis.
fn draw_cover(
    pixmap: &mut Pixmap,
    src: &Pixmap,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    radius: f32,
    alpha: u8,
) {
    let (sw, sh) = (src.width() as f32, src.height() as f32);
    let scale = (w / sw).max(h / sh);
    let visible_w = w / scale;
    let visible_h = h / scale;
    let origin_x = (sw - visible_w) / 2.0;
    let origin_y = (sh - visible_h) / 2.0;
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let x1 = (x + w).ceil() as i32;
    let y1 = (y + h).ceil() as i32;
    for py in y0..y1 {
        for px in x0..x1 {
            if px < 0 || py < 0 || px as u32 >= pixmap.width() || py as u32 >= pixmap.height() {
                continue;
            }
            let fx = px as f32 + 0.5;
            let fy = py as f32 + 0.5;
            if !inside_round(x, y, w, h, radius, fx, fy) {
                continue;
            }
            let u = origin_x + (fx - x) / scale;
            let v = origin_y + (fy - y) / scale;
            let (r, g, b, a) = sample(src, u, v);
            let coverage = (alpha as f32 / 255.0) * (a as f32 / 255.0);
            blend(pixmap, px as u32, py as u32, (r, g, b, 255), coverage);
        }
    }
}

/// Assombrit un rectangle arrondi, avec une opacité variable selon la hauteur
/// (`t` valant 0 en haut et 1 en bas).
pub(crate) fn shade_round(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    radius: f32,
    color: (u8, u8, u8),
    alpha_at: impl Fn(f32) -> u8,
    alpha: u8,
) {
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let x1 = (x + w).ceil() as i32;
    let y1 = (y + h).ceil() as i32;
    for py in y0..y1 {
        let t = ((py as f32 + 0.5 - y) / h).clamp(0.0, 1.0);
        let shade = alpha_at(t);
        if shade == 0 {
            continue;
        }
        for px in x0..x1 {
            if px < 0 || py < 0 || px as u32 >= pixmap.width() || py as u32 >= pixmap.height() {
                continue;
            }
            if !inside_round(x, y, w, h, radius, px as f32 + 0.5, py as f32 + 0.5) {
                continue;
            }
            let coverage = (shade as f32 / 255.0) * (alpha as f32 / 255.0);
            blend(
                pixmap,
                px as u32,
                py as u32,
                (color.0, color.1, color.2, 255),
                coverage,
            );
        }
    }
}

fn inside_round(x: f32, y: f32, w: f32, h: f32, radius: f32, px: f32, py: f32) -> bool {
    if px < x || px > x + w || py < y || py > y + h {
        return false;
    }
    // Une marge d'un millième évite que `clamp` reçoive une borne basse
    // légèrement supérieure à sa borne haute quand le rayon vaut exactement
    // la moitié d'un côté calculé en flottants.
    let radius = radius.min((w / 2.0 - 0.001).max(0.0)).min((h / 2.0 - 0.001).max(0.0));
    let cx = px.clamp(x + radius, x + w - radius);
    let cy = py.clamp(y + radius, y + h - radius);
    let (dx, dy) = (px - cx, py - cy);
    dx * dx + dy * dy <= radius * radius
}

fn sample(src: &Pixmap, u: f32, v: f32) -> (u8, u8, u8, u8) {
    let w = src.width() as i32;
    let h = src.height() as i32;
    let sx = u - 0.5;
    let sy = v - 0.5;
    let x0 = sx.floor() as i32;
    let y0 = sy.floor() as i32;
    let fx = sx - x0 as f32;
    let fy = sy - y0 as f32;
    let clamp = |value: i32, max: i32| value.clamp(0, max - 1);
    let data = src.data();
    let pixel = |px: i32, py: i32| {
        let index = ((clamp(py, h) * w + clamp(px, w)) * 4) as usize;
        (
            data[index] as f32,
            data[index + 1] as f32,
            data[index + 2] as f32,
            data[index + 3] as f32,
        )
    };
    let (r00, g00, b00, a00) = pixel(x0, y0);
    let (r10, g10, b10, a10) = pixel(x0 + 1, y0);
    let (r01, g01, b01, a01) = pixel(x0, y0 + 1);
    let (r11, g11, b11, a11) = pixel(x0 + 1, y0 + 1);
    let mix = |v00: f32, v10: f32, v01: f32, v11: f32| {
        let top = v00 + (v10 - v00) * fx;
        let bottom = v01 + (v11 - v01) * fx;
        (top + (bottom - top) * fy).round().clamp(0.0, 255.0) as u8
    };
    (
        mix(r00, r10, r01, r11),
        mix(g00, g10, g01, g11),
        mix(b00, b10, b01, b11),
        mix(a00, a10, a01, a11),
    )
}

fn push_round(builder: &mut PathBuilder, x: f32, y: f32, w: f32, h: f32, radius: f32) {
    let Some(rect) = Rect::from_xywh(x, y, w, h) else {
        return;
    };
    let r = radius.min(w / 2.0).min(h / 2.0);
    let (x0, y0, x1, y1) = (rect.x(), rect.y(), rect.x() + w, rect.y() + h);
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
}

pub(crate) fn fill_path(pixmap: &mut Pixmap, builder: PathBuilder, rgba: (u8, u8, u8, u8)) {
    let Some(path) = builder.finish() else {
        return;
    };
    let mut paint = Paint::default();
    paint.anti_alias = true;
    paint.set_color_rgba8(rgba.0, rgba.1, rgba.2, rgba.3);
    pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
}

pub(crate) fn stroke_path(pixmap: &mut Pixmap, builder: PathBuilder, rgba: (u8, u8, u8, u8), width: f32) {
    let Some(path) = builder.finish() else {
        return;
    };
    let mut paint = Paint::default();
    paint.anti_alias = true;
    paint.set_color_rgba8(rgba.0, rgba.1, rgba.2, rgba.3);
    let stroke = Stroke {
        width,
        line_cap: tiny_skia::LineCap::Round,
        line_join: tiny_skia::LineJoin::Round,
        ..Default::default()
    };
    pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
}

pub(crate) fn fill_round(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    radius: f32,
    rgba: (u8, u8, u8, u8),
) {
    fill_path(pixmap, path_round(x, y, w, h, radius), rgba);
}

fn path_round(x: f32, y: f32, w: f32, h: f32, radius: f32) -> PathBuilder {
    let mut builder = PathBuilder::new();
    push_round(&mut builder, x, y, w, h, radius);
    builder
}

pub(crate) fn stroke_round(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    radius: f32,
    rgba: (u8, u8, u8, u8),
) {
    stroke_path(pixmap, path_round(x, y, w, h, radius), rgba, 1.0);
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

fn draw_text_right(
    pixmap: &mut Pixmap,
    font: &FontVec,
    text: &str,
    right: f32,
    y: f32,
    size: f32,
    rgba: (u8, u8, u8, u8),
) {
    let width = text_width(font, text, size);
    draw_text(pixmap, font, text, right - width, y, size, rgba);
}

/// `draw_text` prend le haut de la chasse ; les plaques raisonnent en lignes
/// de base. Sans cette conversion, le texte descendait sous son fond.
pub(crate) fn draw_text_baseline(
    pixmap: &mut Pixmap,
    font: &FontVec,
    text: &str,
    x: f32,
    baseline: f32,
    size: f32,
    rgba: (u8, u8, u8, u8),
) {
    let ascent = font.as_scaled(PxScale::from(size)).ascent();
    draw_text(pixmap, font, text, x, baseline - ascent, size, rgba);
}

pub(crate) fn draw_text_right_baseline(
    pixmap: &mut Pixmap,
    font: &FontVec,
    text: &str,
    right: f32,
    baseline: f32,
    size: f32,
    rgba: (u8, u8, u8, u8),
) {
    let width = text_width(font, text, size);
    draw_text_baseline(pixmap, font, text, right - width, baseline, size, rgba);
}

/// Pastille « ‹ › » dessinée en traits : les flèches manquent à la police
/// système et s'affichaient en carrés vides.
fn arrow_hint(
    pixmap: &mut Pixmap,
    font: &FontVec,
    x: f32,
    y: f32,
    label: &str,
    alpha: u8,
) -> f32 {
    let key_w = 36.0;
    fill_round(
        pixmap,
        x,
        y,
        key_w,
        22.0,
        0.0,
        with_alpha((255, 255, 255, 26), alpha),
    );
    let color = with_alpha(TEXT, alpha);
    for (index, direction) in [-1.0f32, 1.0].iter().enumerate() {
        let cx = x + 10.0 + index as f32 * 16.0;
        let mut chevron = PathBuilder::new();
        chevron.move_to(cx - 2.8 * direction, y + 6.5);
        chevron.line_to(cx + 1.6 * direction, y + 11.0);
        chevron.line_to(cx - 2.8 * direction, y + 15.5);
        stroke_path(pixmap, chevron, color, 1.6);
    }
    draw_text(
        pixmap,
        font,
        label,
        x + key_w + 6.0,
        y + 4.5,
        12.0,
        with_alpha(TEXT_DIM, alpha),
    );
    x + key_w + 6.0 + text_width(font, label, 12.0) + 12.0
}

fn draw_text_center(
    pixmap: &mut Pixmap,
    font: &FontVec,
    text: &str,
    center_x: f32,
    y: f32,
    size: f32,
    rgba: (u8, u8, u8, u8),
) {
    let width = text_width(font, text, size);
    draw_text(pixmap, font, text, center_x - width / 2.0, y, size, rgba);
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

/// Coin et dimensions d'une plaque ancrée à l'écran, avec une marge fixe.
fn hud_origin(
    screen_w: u32,
    screen_h: u32,
    width: f32,
    height: f32,
    position: HudPosition,
    margin: f32,
) -> (f32, f32) {
    let x = match position {
        HudPosition::TopLeft | HudPosition::MiddleLeft | HudPosition::BottomLeft => margin,
        HudPosition::TopCenter | HudPosition::MiddleCenter | HudPosition::BottomCenter => {
            ((screen_w as f32 - width) / 2.0).max(margin)
        }
        _ => (screen_w as f32 - width - margin).max(margin),
    };
    let y = match position {
        HudPosition::TopLeft | HudPosition::TopCenter | HudPosition::TopRight => margin,
        HudPosition::MiddleLeft | HudPosition::MiddleCenter | HudPosition::MiddleRight => {
            ((screen_h as f32 - height) / 2.0).max(margin)
        }
        _ => (screen_h as f32 - height - margin).max(margin),
    };
    (x, y)
}

/// Fond commun des plaques : carte sombre arrondie, dégradé léger, filet clair
/// et trait d'accent au sommet.
fn draw_hud_card(pixmap: &mut Pixmap, x: f32, y: f32, w: f32, h: f32) {
    fill_round(pixmap, x, y, w, h, 12.0, (8, 10, 14, 208));
    shade_round(
        pixmap,
        x,
        y,
        w,
        h,
        12.0,
        (46, 58, 76),
        |t| ((1.0 - t).powf(1.7) * 34.0) as u8,
        255,
    );
    stroke_round(pixmap, x, y, w, h, 12.0, (255, 255, 255, 26));
    fill_round(pixmap, x + 12.0, y, 28.0, 2.0, 1.0, ACCENT);
}

/// Barres verticales des cœurs : une piste discrète, remplie par la charge du
/// cœur. Les cœurs très chargés passent à l'ambre puis au rouge.
fn draw_cores(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    cores: &[f32],
    color: (u8, u8, u8, u8),
) {
    if cores.is_empty() || width <= 1.0 {
        return;
    }
    let gap = 2.0;
    let bar_w = ((width - gap * (cores.len() as f32 - 1.0)) / cores.len() as f32).max(2.0);
    for (index, load) in cores.iter().enumerate() {
        let bar_x = x + index as f32 * (bar_w + gap);
        let ratio = (load / 100.0).clamp(0.0, 1.0);
        fill_round(pixmap, bar_x, y, bar_w, height, 1.0, (255, 255, 255, 20));
        let filled = (height * ratio).max(1.0);
        let tone = if *load >= 85.0 {
            (248, 113, 113, 230)
        } else if *load >= 60.0 {
            (251, 191, 36, 230)
        } else {
            (color.0, color.1, color.2, 220)
        };
        fill_round(
            pixmap,
            bar_x,
            y + height - filled,
            bar_w,
            filled,
            1.0,
            tone,
        );
    }
}

/// Bandeau d'état de la page « Enregistrements » : la capture en cours, sa
/// durée et l'état de la pause, avant la liste des vidéos.
fn draw_recording_status(
    pixmap: &mut Pixmap,
    font: &FontVec,
    menu: &MenuState,
    x: f32,
    y: f32,
    w: f32,
    alpha: u8,
) {
    let height = RECORDING_STATUS_HEIGHT - 8.0;
    fill_round(pixmap, x, y, w, height, 10.0, with_alpha(ROW, alpha));
    stroke_round(pixmap, x, y, w, height, 10.0, with_alpha(BORDER, alpha));
    let state = menu.live.recording;
    let accent = match state {
        Some(state) if !state.paused => (248, 113, 113, 255),
        _ => (148, 163, 184, 255),
    };
    let detail = match state {
        Some(state) if state.paused => format!("PAUSE · {}", clock(state.secs)),
        Some(state) => format!("REC · {}", clock(state.secs)),
        None => "Aucun enregistrement en cours".into(),
    };
    let cy = y + height / 2.0;
    fill_round(
        pixmap,
        x + 14.0,
        cy - 3.5,
        7.0,
        7.0,
        3.5,
        with_alpha(accent, alpha),
    );
    draw_text_baseline(
        pixmap,
        font,
        "ENREGISTREMENT",
        x + 30.0,
        cy - 5.0,
        8.5,
        with_alpha(TEXT_DIM, alpha),
    );
    draw_text_baseline(
        pixmap,
        font,
        &detail,
        x + 30.0,
        cy + 10.0,
        13.0,
        with_alpha(
            if state.is_some() { accent } else { TEXT_DIM },
            alpha,
        ),
    );
    let tag = match state {
        Some(state) if state.paused => "EN PAUSE",
        Some(_) => "EN COURS",
        None => "À L'ARRÊT",
    };
    let tag_w = text_width(font, tag, 9.0) + 16.0;
    let tag_x = x + w - tag_w - 12.0;
    fill_round(
        pixmap,
        tag_x,
        cy - 9.0,
        tag_w,
        18.0,
        9.0,
        with_alpha((accent.0, accent.1, accent.2, 38), alpha),
    );
    draw_text_center(
        pixmap,
        font,
        tag,
        tag_x + tag_w / 2.0,
        cy - 4.5,
        9.0,
        with_alpha(accent, alpha),
    );
}

/// Page « Enregistrements » : le statut, les deux commandes de capture, puis
/// les réglages du badge — affichage, ancrage et taille. Aucune liste de
/// vidéos ici : les fichiers restent dans la page Captures de l'application.
fn draw_record_settings(
    pixmap: &mut Pixmap,
    font: &FontVec,
    menu: &MenuState,
    panel_x: f32,
    panel_y: f32,
    panel_w: f32,
    alpha: u8,
) {
    draw_back_button(pixmap, font, menu, panel_x, panel_y, panel_w, alpha);
    draw_recording_status(
        pixmap,
        font,
        menu,
        panel_x + PADDING,
        panel_y + PERF_TOP,
        panel_w - 2.0 * PADDING,
        alpha,
    );
    // Pause et arrêt sont les deux premières lignes : ils restent accessibles
    // à la manette comme au pointeur, sans quitter la page.
    for row in 0..=1 {
        draw_perf_card(
            pixmap,
            font,
            menu,
            row,
            panel_x + PADDING,
            panel_y + menu.perf_row_y(row),
            panel_w - 2.0 * PADDING,
            alpha,
        );
    }
    draw_section_label(
        pixmap,
        font,
        panel_x + PADDING,
        panel_y + menu.perf_row_y(2) - PERF_SECTION + 7.0,
        panel_w - 2.0 * PADDING,
        "HUD D'ENREGISTREMENT",
        alpha,
    );
    draw_perf_card(
        pixmap,
        font,
        menu,
        2,
        panel_x + PADDING,
        panel_y + menu.perf_row_y(2),
        panel_w - 2.0 * PADDING,
        alpha,
    );
    draw_section_label(
        pixmap,
        font,
        panel_x + PADDING,
        panel_y + menu.perf_row_y(3) - PERF_SECTION + 7.0,
        panel_w - 2.0 * PADDING,
        "POSITION",
        alpha,
    );
    draw_perf_card(
        pixmap,
        font,
        menu,
        3,
        panel_x + PADDING,
        panel_y + menu.perf_row_y(3),
        panel_w - 2.0 * PADDING,
        alpha,
    );
    draw_section_label(
        pixmap,
        font,
        panel_x + PADDING,
        panel_y + menu.perf_row_y(4) - PERF_SECTION + 7.0,
        panel_w - 2.0 * PADDING,
        "TAILLE",
        alpha,
    );
    draw_perf_card(
        pixmap,
        font,
        menu,
        4,
        panel_x + PADDING,
        panel_y + menu.perf_row_y(4),
        panel_w - 2.0 * PADDING,
        alpha,
    );
    draw_value_track(
        pixmap,
        menu,
        panel_x + PADDING,
        panel_y + menu.perf_row_y(4),
        panel_w - 2.0 * PADDING,
        ((menu.record_scale - 0.65) / (2.0 - 0.65)).clamp(0.0, 1.0),
        menu.recording_selected == 4,
        alpha,
    );
    draw_position_grid(
        pixmap,
        font,
        menu,
        panel_x + PADDING,
        panel_y + menu.perf_grid_top(),
        panel_w - 2.0 * PADDING,
        true,
        alpha,
    );
}

/// Piste horizontale d'une valeur continue, dessinée au bas d'une carte comme
/// celle de la limite de FPS : un clic ou un glissement choisit la valeur.
fn draw_value_track(
    pixmap: &mut Pixmap,
    _menu: &MenuState,
    x: f32,
    row_top: f32,
    w: f32,
    ratio: f32,
    selected: bool,
    alpha: u8,
) {
    let card_y = row_top + 2.0;
    let card_h = PERF_ROW - 4.0;
    let left = x + FPS_TRACK_LEFT;
    let right = x + w - FPS_TRACK_RIGHT;
    let track_w = (right - left).max(1.0);
    let track_y = card_y + card_h - 12.0;
    let track_color = if selected {
        with_alpha((24, 26, 30, 38), alpha)
    } else {
        with_alpha((255, 255, 255, 30), alpha)
    };
    let fill_color = if selected {
        with_alpha((24, 26, 30, 255), alpha)
    } else {
        with_alpha((236, 237, 239, 190), alpha)
    };
    let tick_color = if selected {
        with_alpha((24, 26, 30, 46), alpha)
    } else {
        with_alpha((255, 255, 255, 40), alpha)
    };
    fill_round(pixmap, left, track_y, track_w, 3.0, 1.5, track_color);
    fill_round(
        pixmap,
        left,
        track_y,
        track_w * ratio.clamp(0.0, 1.0),
        3.0,
        1.5,
        fill_color,
    );
    for index in 0..9 {
        let tick = left + track_w * index as f32 / 8.0;
        fill_round(
            pixmap,
            tick - 0.5,
            track_y + 5.0,
            1.0,
            3.0,
            0.5,
            tick_color,
        );
    }
    let knob_x = left + track_w * ratio.clamp(0.0, 1.0);
    fill_round(pixmap, knob_x - 4.0, track_y - 3.5, 8.0, 10.0, 4.0, fill_color);
}


/// Courbe de frametime de MangoHud : la ligne verte monte quand une image met
/// plus de temps à arriver. Elle se trace à partir de la cadence mesurée
/// seconde par seconde (ms = 1000 / i/s), avec une aire légère dessous.
fn draw_frametime_graph(
    pixmap: &mut Pixmap,
    series: &[u32],
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    rgba: (u8, u8, u8, u8),
) {
    if series.len() < 2 || width <= 1.0 {
        return;
    }
    let max = series.iter().copied().max().unwrap_or(1).max(1) as f32;
    let min = series.iter().copied().min().unwrap_or(1).max(1) as f32;
    let span = (max - min).max(1.0);
    let point = |index: usize, value: u32| {
        let px = x + width * index as f32 / (series.len() - 1) as f32;
        let py = y + height * (1.0 - ((value as f32 - min) / span));
        (px, py)
    };

    let mut area = PathBuilder::new();
    area.move_to(x, y + height);
    for (index, value) in series.iter().enumerate() {
        let (px, py) = point(index, *value);
        area.line_to(px, py);
    }
    area.line_to(x + width, y + height);
    area.close();
    fill_path(pixmap, area, (rgba.0, rgba.1, rgba.2, 36));

    let mut line = PathBuilder::new();
    for (index, value) in series.iter().enumerate() {
        let (px, py) = point(index, *value);
        if index == 0 {
            line.move_to(px, py);
        } else {
            line.line_to(px, py);
        }
    }
    stroke_path(pixmap, line, rgba, 1.2);
}

/// Tronque un texte à la largeur disponible (le « … » compris).
pub(crate) fn fit_text(font: &FontVec, text: &str, max_width: f32, size: f32) -> String {
    if text_width(font, text, size) <= max_width {
        return text.to_string();
    }
    let mut out = String::new();
    let mut width = 0.0;
    let limit = max_width - text_width(font, "…", size);
    for ch in text.chars() {
        let advance = {
            let scaled = font.as_scaled(PxScale::from(size));
            scaled.h_advance(scaled.glyph_id(ch))
        };
        if width + advance > limit {
            break;
        }
        width += advance;
        out.push(ch);
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menu::MenuState;

    fn renderer() -> Option<Renderer> {
        let font = crate::font::find_font().ok()?;
        Renderer::new(&font, 1280, 800, None, None).ok()
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

    /// Les amis s'ouvrent dans une fenêtre compacte, pas dans un écran plein.
    #[test]
    fn invite_page_uses_a_compact_window() {
        let mut menu = MenuState::new("Portal 2");
        menu.anim = 1.0;
        menu.set_page(Page::Invite);
        let (x, y, width, height) = panel_bounds(1280, 800, &menu);
        assert_eq!((x, y, width, height), (300.0, 90.0, 680.0, 620.0));
        if let Some(renderer) = renderer() {
            let pixmap = renderer.render(&menu);
            assert!(pixmap.pixel(640, 400).unwrap().alpha() > 200);
        }
    }

    /// Les neuf cases de la grille montrent chacune une position différente :
    /// aucun aperçu ne doit ressembler à un autre.
    #[test]
    fn invite_page_draws_the_friend_list() {
        let Some(renderer) = renderer() else {
            return;
        };
        let mut menu = MenuState::new("Portal 2");
        menu.anim = 1.0;
        menu.set_invite(lapinux_core::social::SteamFriendsState {
            live: true,
            friends: vec![lapinux_core::social::SteamFriend {
                steam_id: 111,
                name: "Alice".into(),
                presence: lapinux_core::social::SteamPresence::Online,
                ..Default::default()
            }],
            ..Default::default()
        });
        menu.set_page(Page::Invite);
        let first = renderer.render(&menu);
        menu.activate();
        let second = renderer.render(&menu);
        assert_ne!(first.data(), second.data(), "la conversation doit être visible");
    }

    #[test]
    fn chat_page_draws_the_conversation_and_the_composer() {
        let Some(renderer) = renderer() else {
            return;
        };
        let mut menu = MenuState::new("Portal 2");
        menu.anim = 1.0;
        menu.set_invite(lapinux_core::social::SteamFriendsState {
            live: true,
            friends: vec![lapinux_core::social::SteamFriend {
                steam_id: 111,
                name: "Alice".into(),
                presence: lapinux_core::social::SteamPresence::Online,
                ..Default::default()
            }],
            chats: [(
                111,
                vec![lapinux_core::social::SteamChatMessage {
                    from: 111,
                    text: "Tu joues ce soir ?".into(),
                    at: 0,
                    outgoing: false,
                }],
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        });
        menu.set_page(Page::Invite);
        let list = renderer.render(&menu);
        assert!(menu.open_chat());
        let conversation = renderer.render(&menu);
        assert_ne!(list.data(), conversation.data(), "la conversation doit être dessinée");
        menu.push_chat_char('O');
        menu.push_chat_char('k');
        let typed = renderer.render(&menu);
        assert_ne!(
            conversation.data(),
            typed.data(),
            "le champ de saisie doit afficher le texte"
        );
    }

    #[test]
    fn invite_page_draws_the_cached_avatar() {
        let Some(renderer) = renderer() else {
            return;
        };
        let directory = tempfile::tempdir().unwrap();
        let avatar = directory.path().join("111.png");
        image::RgbaImage::from_pixel(32, 32, image::Rgba([255, 24, 24, 255]))
            .save(&avatar)
            .unwrap();
        let mut menu = MenuState::new("Portal 2");
        menu.anim = 1.0;
        menu.set_invite(lapinux_core::social::SteamFriendsState {
            live: true,
            friends: vec![lapinux_core::social::SteamFriend {
                steam_id: 111,
                name: "Alice".into(),
                presence: lapinux_core::social::SteamPresence::Online,
                ..Default::default()
            }],
            avatars: [(111, avatar)].into_iter().collect(),
            ..Default::default()
        });
        menu.set_page(Page::Invite);
        let pixmap = renderer.render(&menu);
        let (panel_x, panel_y, _, panel_h) = panel_bounds(1280, 800, &menu);
        let row_h = menu.invite_row_height(panel_h);
        let avatar_size = (row_h * 0.72).clamp(32.0, 48.0);
        let x = (panel_x + PADDING + 14.0 + avatar_size / 2.0) as u32;
        let y = (panel_y + menu.invite_row_y(0, panel_h) + row_h / 2.0) as u32;
        let pixel = pixmap.pixel(x, y).unwrap();
        assert!(
            pixel.red() > 180 && pixel.green() < 80,
            "l'avatar en cache doit être dessiné"
        );
    }

    #[test]
    fn every_position_preview_differs() {
        let Some(renderer) = renderer() else {
            return;
        };
        let mut menu = MenuState::new("Jeu");
        menu.anim = 1.0;
        menu.page = Page::Performance;
        let mut rendered: Vec<Vec<u8>> = Vec::new();
        for position in HudPosition::ALL {
            menu.place_hud(position);
            rendered.push(renderer.render(&menu).data().to_vec());
        }
        for (index, first) in rendered.iter().enumerate() {
            for second in rendered.iter().skip(index + 1) {
                assert_ne!(first, second, "deux aperçus identiques");
            }
        }
    }

    #[test]
    fn panel_is_anchored_left() {
        let mut menu = MenuState::new("Jeu");
        menu.anim = 1.0;
        let (x, y, w, h) = panel_bounds(1280, 800, &menu);
        assert!(x > 0.0 && y > 0.0);
        assert!(x < 64.0, "panneau collé au bord gauche : x={x}");
        assert!(x + w < 1280.0 / 2.0, "le jeu reste visible à droite");
        assert!(w <= PANEL_WIDTH);
        assert!(h > 400.0 && h < 800.0);
    }

    #[test]
    fn panel_slides_in_from_the_left() {
        let mut menu = MenuState::new("Jeu");
        menu.anim = 0.0;
        let (start_x, ..) = panel_bounds(1280, 800, &menu);
        menu.anim = 0.5;
        let (middle_x, ..) = panel_bounds(1280, 800, &menu);
        menu.anim = 1.0;
        let (end_x, ..) = panel_bounds(1280, 800, &menu);
        assert!(start_x < 0.0, "le panneau part hors écran : x={start_x}");
        assert!(middle_x > start_x && middle_x < end_x);
        assert!((end_x - PANEL_MARGIN).abs() < 0.01);
    }

    #[test]
    fn capsule_keeps_its_slot_at_the_top() {
        let menu = MenuState::new("Jeu");
        // L'en-tête réserve la carte capsule et sa marge.
        assert!(CAPSULE_HEIGHT + 2.0 * PADDING <= crate::menu::HEADER_HEIGHT);
        assert_eq!(menu.row_y(0), crate::menu::HEADER_HEIGHT);
    }

    #[test]
    fn text_width_is_positive() {
        let Some(font_path) = crate::font::find_font().ok() else {
            return;
        };
        let bytes = std::fs::read(font_path).unwrap();
        let font = FontVec::try_from_vec(bytes).unwrap();
        assert!(text_width(&font, "Lapinux", 16.0) > 10.0);
    }

    #[test]
    fn long_titles_are_shortened() {
        let Some(font_path) = crate::font::find_font().ok() else {
            return;
        };
        let bytes = std::fs::read(font_path).unwrap();
        let font = FontVec::try_from_vec(bytes).unwrap();
        let fitted = fit_text(&font, "A Very Long Game Title Indeed", 120.0, 19.0);
        assert!(fitted.ends_with('…'));
        assert!(text_width(&font, &fitted, 19.0) <= 120.0);
        assert_eq!(fit_text(&font, "Court", 400.0, 19.0), "Court");
    }

    /// En disposition « en ligne », les barres par cœur et la courbe prennent
    /// une seconde bande : elles ne sont plus ignorées par le retour anticipé.
    #[test]
    fn inline_hud_draws_cores_and_graph() {
        let Some(renderer) = renderer() else {
            return;
        };
        let mut menu = MenuState::new("Jeu");
        menu.anim = 0.0;
        menu.hud_only = true;
        menu.perf_hud = true;
        menu.hud_layout = HudLayout::Inline;
        menu.hud_cores = true;
        menu.hud_graph = true;
        menu.system = Some(lapinux_core::system::SystemInfo {
            cpu_cores: vec![12.0, 78.0, 41.0, 63.0],
            ..Default::default()
        });
        menu.live.series = vec![58, 60, 57, 61];
        let with = renderer.render(&menu);
        menu.hud_cores = false;
        menu.hud_graph = false;
        let without = renderer.render(&menu);
        assert_ne!(
            with.data(),
            without.data(),
            "les cœurs et la courbe doivent changer la plaque en ligne"
        );
    }

    /// Le badge REC est indépendant : il se déplace sans toucher aux chiffres.
    #[test]
    fn recording_badge_is_separate_and_movable() {
        let Some(renderer) = renderer() else {
            return;
        };
        let mut menu = MenuState::new("Jeu");
        menu.anim = 0.0;
        menu.hud_only = true;
        menu.record_badge = true;
        menu.record_position = HudPosition::TopLeft;
        menu.live.recording = Some(lapinux_core::model::RecordingState {
            paused: false,
            secs: 83,
        });
        let first = renderer.render(&menu);
        menu.record_position = HudPosition::BottomRight;
        let second = renderer.render(&menu);
        assert_ne!(first.data(), second.data());
    }

    /// La ligne native reprend l'ordre et les unités de MangoHud : matériel,
    /// mémoire, puis cadence. Aucun libellé « FrameTime » ne s'y glisse.
    #[test]
    fn inline_hud_follows_mangohud_order_and_units() {
        let mut menu = MenuState::new("Jeu");
        menu.hud = HudPreset::Detailed;
        menu.live.fps = Some(120);
        menu.system = Some(lapinux_core::system::SystemInfo {
            cpu_usage: 42.0,
            ram_total: 16_000_000_000,
            ram_used: 8_000_000_000,
            gpu_load: Some(lapinux_core::system::GpuLoad {
                usage: 63.0,
                vram_used: 4_000_000_000,
                vram_total: 8_000_000_000,
                temperature: Some(52.0),
            }),
            cpu_temp: Some(66.0),
            ..Default::default()
        });
        let joined = inline_hud_spans(&menu)
            .into_iter()
            .map(|(text, _)| text)
            .collect::<Vec<_>>()
            .join(" | ");
        assert!(joined.contains("63%"), "{joined}");
        assert!(joined.contains("42%"), "{joined}");
        assert!(joined.contains("RAM 7.5 GiB"), "{joined}");
        assert!(joined.contains("120 FPS"), "{joined}");
        assert!(joined.contains("52°C"), "{joined}");
        assert!(joined.contains("66°C"), "{joined}");
        assert!(joined.contains("8.3 ms"), "{joined}");
        assert!(!joined.contains("FrameTime"), "{joined}");
        assert!(joined.find("RAM").unwrap() < joined.find("120 FPS").unwrap());
        menu.hud = HudPreset::Simple;
        let simple = inline_hud_spans(&menu)
            .into_iter()
            .map(|(text, _)| text)
            .collect::<Vec<_>>()
            .join(" | ");
        assert!(!simple.contains("RAM"), "{simple}");
        assert!(!simple.contains(" ms"), "{simple}");
    }

    /// Le processus du menu connaît l'état de la plaque sans la peindre :
    /// sinon elle apparaîtrait en double à la fermeture du panneau.
    #[test]
    fn menu_process_never_paints_the_standalone_hud() {
        let Some(renderer) = renderer() else {
            return;
        };
        let mut menu = MenuState::new("Jeu");
        menu.anim = 0.0;
        menu.perf_hud = true;
        let menu_only = renderer.render(&menu);
        menu.hud_only = true;
        let hud = renderer.render(&menu);
        assert_ne!(
            menu_only.data(),
            hud.data(),
            "la plaque doit être réservée à la surface --hud-only"
        );
    }

    /// La plaque de performance ne porte plus le statut REC : il vit dans le
    /// badge séparé, et seulement quand celui-ci est demandé.
    #[test]
    fn perf_hud_no_longer_carries_the_rec_status() {
        let Some(renderer) = renderer() else {
            return;
        };
        let mut menu = MenuState::new("Jeu");
        menu.anim = 0.0;
        menu.hud_only = true;
        menu.perf_hud = true;
        menu.record_badge = false;
        let without = renderer.render(&menu);
        menu.live.recording = Some(lapinux_core::model::RecordingState {
            paused: false,
            secs: 83,
        });
        let with = renderer.render(&menu);
        assert_eq!(without.data(), with.data());
    }
}
