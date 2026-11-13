//! Interface graphique egui : bibliothèque, téléchargements, Protons, manettes, réglages.
//!
//! Les pages et fonctionnalités sont réparties en modules dédiés.

mod achievements;
mod captures;
mod configurations;
mod controller_status;
mod controllers;
mod downloads;
mod drm;
mod gameinfo;
mod launch;
mod news;
mod pad_audio;
mod pad_id;
mod profile;
mod saves;
mod section_icons;
mod steam_bridge;
mod steaminput;
mod stores;
use controller_status::{
    band_from_percent, battery_band_label, battery_charging, battery_ring,
    battery_ring_placeholder, battery_test, bluetooth_test, charge_state_label, connection_badge,
    console_label, controller_group_header, controller_key, controller_tooltip, dongle_asleep,
    is_steam_controller_2026, legacy_path_key, merge_controllers, pad_identity, pad_setting,
    pad_shown_name, paint_battery_arc, paint_battery_ring, paint_charge_sheen, smoothstep,
    split_controller_groups, steam_input_kind_for, steam_puck_card,
};
#[cfg(test)]
use controller_status::{format_capacity, sheen_fade, DONGLE_SILENCE_SECS};
mod collections_page;
mod cover_picker;
mod folder_picker;
use folder_picker::pick_folder;
mod freegames;
mod friends_panel;
mod game_contents;
mod game_sheet;
mod library_page;
mod news_reader;
mod profile_page;
mod protons_page;
mod settings_general;
mod settings_hud;
mod settings_in_game;
mod settings_tabs;
mod settings_window;
mod sheet_saves;
#[cfg(test)]
mod snapshot_gpu;
mod stores_page;
mod system_page;
mod theme;
mod trailer;
mod ui_accounts;
mod ui_catalog;
mod ui_launch;
mod ui_refresh;
mod ui_shell;
mod ui_window;
mod webview;
mod workshop;

use downloads::{paint_hero_glyph, public_download_bytes, Downloads, HeroGlyph, NavBadge};
use pad_audio::{HeadsetState, PadAudio};
#[cfg(test)]
use pad_id::identify;
use pad_id::{Brand, PadIdentity};
use profile::{Profile, ProfileAction};

use eframe::egui;
use lapinux_core::client::Client;
use lapinux_core::config::{ArtSource, Config, FaceButtonProfile};
use lapinux_core::ipc::{DaemonBuild, Envelope, Req};
use lapinux_core::model::{
    CheckStatus, DeviceInfo, DeviceLink, DoctorReport, Game, GameOrigin, GameSettings, Job,
    JobKind, JobState, LogsView, ProtonInfo, ProtonRelease, SessionInfo, SteamAchievements,
    SteamCatalog, SteamCatalogGame, SteamDetails, SteamDlc, SteamGameInfo, SteamGuardKind,
    SteamInputSaved, SteamLoginState, SteamStatus, SteamTokenVerdict,
};
use lapinux_core::paths::LapinuxPaths;
use lapinux_core::social::{SteamFriendEvent, SteamPresence, SteamSocial, StoreFriends};
use lapinux_core::system::{DiskUsage, GpuLoad, SystemInfo, SystemSampler};
use lapinux_input::{BatteryDesign, ChargeState};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::sync::{mpsc, Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// Palette dynamique : les pages historiques lisaient des constantes, elles
// passent maintenant par ces accesseurs. Changer de thème repeint donc toute
// l'interface au lieu de seulement la coque NX.
std::thread_local! {
    static ACTIVE_PALETTE: std::cell::Cell<theme::Palette> =
        std::cell::Cell::new(theme::Palette::classic());
}

fn active_palette() -> theme::Palette {
    ACTIVE_PALETTE.with(|palette| palette.get())
}

fn set_active_palette(palette: theme::Palette) {
    ACTIVE_PALETTE.with(|active| active.set(palette));
}

macro_rules! palette_color {
    ($name:ident, $field:ident) => {
        #[allow(non_snake_case)]
        pub(crate) fn $name() -> egui::Color32 {
            active_palette().$field
        }
    };
}

palette_color!(BACKGROUND, background);
palette_color!(PANEL, panel);
palette_color!(CARD, card);
palette_color!(CARD_HOVER, card_hover);
palette_color!(CARD_ACTIVE, card_active);
palette_color!(BORDER, border);
palette_color!(ACCENT, accent);
palette_color!(TEXT, text);
palette_color!(TEXT_DIM, text_dim);
const OK: egui::Color32 = egui::Color32::from_rgb(126, 205, 149);
const BLUETOOTH_BLUE: egui::Color32 = egui::Color32::from_rgb(96, 165, 250);
const WARN: egui::Color32 = egui::Color32::from_rgb(232, 192, 118);
const DANGER: egui::Color32 = egui::Color32::from_rgb(232, 132, 132);

fn apply_theme(ctx: &egui::Context) {
    apply_active_theme(ctx, &theme::ActiveTheme::default());
}

/// Applique la palette, les arrondis et l'échelle de texte du thème actif.
/// La palette historique reste utilisée par toutes les pages qui n'ont pas
/// encore de rendu NX ; ce style commun donne au moins aux widgets egui la
/// bonne couleur et à la coque la bonne ambiance.
fn apply_active_theme(ctx: &egui::Context, active: &theme::ActiveTheme) {
    let colors = active.colors;
    set_active_palette(colors);
    let mut visuals = if colors.is_light() {
        egui::Visuals::light()
    } else {
        egui::Visuals::dark()
    };
    visuals.override_text_color = Some(colors.text);
    visuals.panel_fill = colors.background;
    visuals.window_fill = colors.panel;
    visuals.faint_bg_color = blend_colors(colors.background, colors.card, 0.55);
    visuals.extreme_bg_color = blend_colors(colors.background, egui::Color32::BLACK, 0.35);
    visuals.code_bg_color = colors.background;
    visuals.hyperlink_color = colors.accent;
    visuals.selection.bg_fill = colors.card_active;
    visuals.selection.stroke = egui::Stroke::new(1.0_f32, colors.accent);
    let rounding = active.rounding.clamp(0.0, 24.0);
    visuals.window_rounding = egui::Rounding::same(rounding);
    visuals.menu_rounding = egui::Rounding::same((rounding - 2.0).max(0.0));
    visuals.widgets.noninteractive.bg_fill = colors.panel;
    visuals.widgets.noninteractive.weak_bg_fill = colors.panel;
    visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, colors.border);
    visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0_f32, colors.text_dim);
    visuals.widgets.inactive.bg_fill = colors.card;
    visuals.widgets.inactive.weak_bg_fill = colors.card;
    visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0_f32, colors.border);
    visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0_f32, colors.text);
    visuals.widgets.hovered.bg_fill = colors.card_hover;
    visuals.widgets.hovered.weak_bg_fill = colors.card_hover;
    visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0_f32, colors.accent);
    visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0_f32, colors.text);
    visuals.widgets.active.bg_fill = colors.card_active;
    visuals.widgets.active.weak_bg_fill = colors.card_active;
    visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0_f32, colors.accent);
    visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0_f32, colors.text);

    // On repart du style par défaut à chaque changement : appliquer une
    // échelle à des tailles déjà mises à l'échelle ferait grossir le texte à
    // chaque enregistrement.
    let mut style = egui::Style::default();
    style.visuals = visuals;
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    let scale = active.font_scale.clamp(0.75, 1.6);
    if (scale - 1.0).abs() > f32::EPSILON {
        for font in style.text_styles.values_mut() {
            font.size = (font.size * scale).clamp(7.0, 40.0);
        }
    }
    ctx.set_style(style);
}

fn blend_colors(a: egui::Color32, b: egui::Color32, amount: f32) -> egui::Color32 {
    let amount = amount.clamp(0.0, 1.0);
    let mix = |first: u8, second: u8| {
        (first as f32 + (second as f32 - first as f32) * amount).round() as u8
    };
    egui::Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}

/// Teinte dominante d'une jaquette, échantillonnée au centre et pondérée par
/// la saturation : un fond gris ou une bordure blanche ne doivent pas noyer
/// la couleur du jeu. Renvoie aussi une version sombre pour les traits fins.
fn cover_tone(path: &std::path::Path) -> Option<(egui::Color32, egui::Color32)> {
    let image = image::open(path).ok()?;
    let small = image.thumbnail(32, 32).to_rgba8();
    let width = small.width() as usize;
    let height = small.height() as usize;
    if width == 0 || height == 0 {
        return None;
    }
    let raw = small.as_raw();
    let left = width / 5;
    let right = (width * 4 / 5).max(left + 1).min(width);
    let top = height / 5;
    let bottom = (height * 4 / 5).max(top + 1).min(height);
    let mut sum = (0u64, 0u64, 0u64);
    let mut weight = 0u64;
    for y in top..bottom {
        for x in left..right {
            let index = (y * width + x) * 4;
            let r = raw[index];
            let g = raw[index + 1];
            let b = raw[index + 2];
            let a = raw[index + 3];
            if a < 128 {
                continue;
            }
            let max = r.max(g).max(b) as u32;
            let min = r.min(g).min(b) as u32;
            let saturation = max - min;
            let luminance = (r as u32 + g as u32 + b as u32) / 3;
            if luminance < 18 || luminance > 246 {
                continue;
            }
            let w = 1 + saturation * saturation / 8;
            sum.0 += r as u64 * w as u64;
            sum.1 += g as u64 * w as u64;
            sum.2 += b as u64 * w as u64;
            weight += w as u64;
        }
    }
    if weight == 0 {
        return None;
    }
    let tone = egui::Color32::from_rgb(
        (sum.0 / weight) as u8,
        (sum.1 / weight) as u8,
        (sum.2 / weight) as u8,
    );
    let deep = blend_colors(tone, egui::Color32::from_rgb(18, 18, 20), 0.55);
    Some((tone, deep))
}

/// Teinte de secours, stable pour un appid donné, quand aucune jaquette n'est
/// encore lisible : le fond reste vivant sans dépendre du disque.
fn fallback_cover_tone(appid: u32) -> (egui::Color32, egui::Color32) {
    const TONES: [(u8, u8, u8); 6] = [
        (0, 200, 224),
        (255, 125, 0),
        (230, 0, 18),
        (106, 176, 255),
        (179, 140, 255),
        (94, 210, 110),
    ];
    let (r, g, b) = TONES[appid as usize % TONES.len()];
    let tone = egui::Color32::from_rgb(r, g, b);
    (
        tone,
        blend_colors(tone, egui::Color32::from_rgb(18, 18, 20), 0.55),
    )
}

/// Icône de fenêtre (barre des tâches, Alt-Tab) : le lapin sur fond sombre,
/// identique au fichier installé pour le lanceur du bureau.
fn window_icon() -> egui::IconData {
    match image::load_from_memory(include_bytes!("../../../assets/lapinux-icon.png")) {
        Ok(image) => {
            let image = image.to_rgba8();
            egui::IconData {
                rgba: image.as_raw().clone(),
                width: image.width(),
                height: image.height(),
            }
        }
        Err(_) => egui::IconData::default(),
    }
}

pub fn run() -> eframe::Result<()> {
    // Point de départ du chronomètre de démarrage (voir `FrameTrace`).
    ui_start();
    start_heartbeat();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1360.0, 860.0])
            .with_min_inner_size([980.0, 640.0])
            .with_title(lapinux_core::APP_NAME)
            .with_app_id(lapinux_core::APP_ID)
            .with_decorations(false)
            .with_icon(window_icon()),
        vsync: true,
        ..Default::default()
    };
    eframe::run_native(
        lapinux_core::APP_NAME,
        options,
        Box::new(|cc| {
            let app = LapinuxApp::new(cc);
            // Une requête de session indépendante de la peinture : pendant une
            // partie, KWin peut cacher la fenêtre et interrompre les images.
            // Sans ce réveil, l'UI garderait ses descripteurs evdev ouverts
            // alors que le service d'entrée les a déjà capturés pour le jeu.
            let worker = app.client.detached();
            let ctx = cc.egui_ctx.clone();
            std::thread::spawn(move || loop {
                let session: Option<SessionInfo> = ui_request(&worker, &Req::Session);
                if session.is_some() {
                    ctx.request_repaint();
                }
                std::thread::sleep(Duration::from_millis(750));
            });
            Ok(Box::new(app))
        }),
    )
}

/// Options de la scène persistante utilisée par `lapinux scene`.
pub struct SceneOptions {
    /// Nom affiché sous le logo.
    pub name: String,
    /// Taille exacte de la surface GameScope qui porte la scène.
    pub width: u32,
    pub height: u32,
    /// Jaquette locale du jeu, quand elle est déjà en cache.
    pub cover: Option<PathBuf>,
    /// Visuel large 16:9 de la scène, quand il est disponible.
    pub backdrop: Option<PathBuf>,
    /// Commande du jeu, programme puis arguments.
    pub command: Vec<String>,
    /// Durée de l'écran de fin, en secondes.
    pub end_seconds: f32,
    /// Fichier posé dès que le jeu est terminé : le daemon y voit le signal
    /// pour arrêter la capture et le HUD avant l'écran de fin.
    pub exit_file: Option<PathBuf>,
    /// Fichier posé par le daemon à la première image du jeu : la scène
    /// s'efface alors pour laisser le jeu visible dans GameScope.
    pub start_file: Option<PathBuf>,
}

/// Prototype de scène persistante : une seule fenêtre eframe reste ouverte
/// pendant toute la session. Elle affiche le lancement, puis laisse le jeu
/// (un client séparé de GameScope) la couvrir ; quand le jeu sort, elle révèle
/// l'écran de fin sans créer de nouvelle fenêtre et sans repasser par le bureau.
///
/// Le daemon s'en sert à chaque lancement dans GameScope (`lapinux perf`
/// compose la commande) : la scène porte l'écran de lancement, laisse le jeu
/// la couvrir à la première image, puis montre l'écran de fin.
pub fn run_scene(options: SceneOptions) -> eframe::Result<()> {
    let width = options.width.max(320) as f32;
    let height = options.height.max(240) as f32;
    let native = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(lapinux_core::i18n::t("app.window_title"))
            .with_app_id("lapinux-scene")
            // Le plein écran imbriqué de GameScope n'applique pas toujours
            // `with_fullscreen` : la taille explicite garantit que la scène
            // couvre toute la surface et ne laisse pas le bureau sur le côté.
            .with_inner_size([width, height])
            .with_fullscreen(true)
            .with_decorations(false),
        vsync: true,
        ..Default::default()
    };
    eframe::run_native(
        lapinux_core::i18n::t("app.window_title"),
        native,
        Box::new(move |cc| Ok(Box::new(SceneApp::new(cc, options)))),
    )
}

/// Peint la scène : jaquette du jeu au centre, puis son nom et l'état de la
/// session. Sans jaquette, le lapin animé garde l'écran habité.
fn paint_scene(
    painter: &egui::Painter,
    rect: egui::Rect,
    backdrop: Option<&egui::TextureHandle>,
    cover: Option<&egui::TextureHandle>,
    bunny: Option<&launch::Bunny>,
    name: &str,
    subtitle: &str,
    ended: bool,
    left: Option<f32>,
    elapsed: f32,
) {
    painter.rect_filled(rect, 0.0, BACKGROUND());
    let full = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    if let Some(backdrop) = backdrop {
        // Visuel large du jeu : il remplit la surface 16:9, sans recadrer une
        // jaquette portrait. Un voile sombre garde le titre lisible.
        let size = backdrop.size_vec2();
        if size.x > 0.0 && size.y > 0.0 {
            let scale = (rect.width() / size.x).max(rect.height() / size.y);
            let image_rect = egui::Rect::from_center_size(rect.center(), size * scale);
            painter.image(backdrop.id(), image_rect, full, egui::Color32::WHITE);
        }
        painter.rect_filled(rect, 0.0, egui::Color32::from_black_alpha(38));
        // Dégradé vers le bas, en bandes : le titre reste lisible même sur un
        // screenshot très clair.
        let bands = 18;
        let gradient_top = rect.bottom() - rect.height() * 0.42;
        for index in 0..bands {
            let top = gradient_top + (rect.bottom() - gradient_top) * index as f32 / bands as f32;
            let bottom =
                gradient_top + (rect.bottom() - gradient_top) * (index + 1) as f32 / bands as f32;
            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(rect.left(), top),
                    egui::pos2(rect.right(), bottom),
                ),
                0.0,
                egui::Color32::from_black_alpha(10 + index as u8 * 9),
            );
        }
        painter.text(
            rect.left_top() + egui::vec2(48.0, 44.0),
            egui::Align2::LEFT_TOP,
            "LAPINUX",
            egui::FontId::proportional(13.0),
            TEXT_DIM(),
        );
        let title_at = rect.left_bottom() + egui::vec2(64.0, -78.0);
        painter.text(
            title_at,
            egui::Align2::LEFT_BOTTOM,
            name,
            egui::FontId::proportional(34.0),
            TEXT(),
        );
        painter.text(
            title_at + egui::vec2(0.0, 30.0),
            egui::Align2::LEFT_BOTTOM,
            subtitle,
            egui::FontId::proportional(15.0),
            if ended { TEXT() } else { TEXT_DIM() },
        );
        if let Some(left) = left {
            painter.text(
                rect.right_bottom() - egui::vec2(48.0, 30.0),
                egui::Align2::RIGHT_BOTTOM,
                lapinux_core::i18n::t_args(
                    "game_sheet.closing_in",
                    &[("seconds", &format!("{left:.1}"))],
                ),
                egui::FontId::proportional(11.0),
                TEXT_DIM(),
            );
        }
        return;
    }
    if let Some(cover) = cover {
        // Repli sans visuel large : la jaquette reste entière dans une carte,
        // jamais recadrée pour remplir un fond 16:9.
        let size = cover.size_vec2();
        painter.rect_filled(rect, 0.0, egui::Color32::from_black_alpha(35));

        let ratio = if size.y > 0.0 {
            (size.x / size.y).clamp(0.58, 0.86)
        } else {
            2.0 / 3.0
        };
        let height = (rect.height() * 0.62)
            .min(rect.width() * 0.66)
            .clamp(220.0, 620.0);
        let capsule = egui::Rect::from_center_size(
            rect.center() - egui::vec2(0.0, rect.height() * 0.035),
            egui::vec2(height * ratio, height),
        );
        painter.rect_filled(
            capsule.expand(10.0),
            egui::Rounding::same(16.0),
            egui::Color32::from_black_alpha(160),
        );
        painter.image(cover.id(), capsule, full, egui::Color32::WHITE);

        let text_at = capsule.center_bottom() + egui::vec2(0.0, 24.0);
        painter.text(
            text_at,
            egui::Align2::CENTER_TOP,
            name,
            egui::FontId::proportional(26.0),
            TEXT(),
        );
        painter.text(
            text_at + egui::vec2(0.0, 34.0),
            egui::Align2::CENTER_TOP,
            subtitle,
            egui::FontId::proportional(14.0),
            if ended { TEXT() } else { TEXT_DIM() },
        );
    } else {
        let center = rect.center() - egui::vec2(0.0, 56.0);
        if let Some(bunny) = bunny {
            bunny.draw_sized(painter, center, 180.0, elapsed);
        }
        painter.text(
            center + egui::vec2(0.0, 130.0),
            egui::Align2::CENTER_TOP,
            name,
            egui::FontId::proportional(30.0),
            TEXT(),
        );
        painter.text(
            center + egui::vec2(0.0, 174.0),
            egui::Align2::CENTER_TOP,
            subtitle,
            egui::FontId::proportional(15.0),
            if ended { TEXT() } else { TEXT_DIM() },
        );
    }
    painter.text(
        rect.center_top() + egui::vec2(0.0, 52.0),
        egui::Align2::CENTER_CENTER,
        "LAPINUX",
        egui::FontId::proportional(13.0),
        TEXT_DIM(),
    );
    if let Some(left) = left {
        painter.text(
            rect.center_bottom() - egui::vec2(0.0, 34.0),
            egui::Align2::CENTER_BOTTOM,
            format!("Fermeture dans {left:.1} s · cliquez pour revenir"),
            egui::FontId::proportional(11.0),
            TEXT_DIM(),
        );
    }
}

struct SceneApp {
    name: String,
    command: Vec<String>,
    end_seconds: f32,
    spawn_at: Instant,
    child: Option<std::process::Child>,
    ended_at: Option<Instant>,
    error: Option<String>,
    bunny: Option<launch::Bunny>,
    cover: Option<egui::TextureHandle>,
    backdrop: Option<egui::TextureHandle>,
    exit_file: Option<PathBuf>,
    start_file: Option<PathBuf>,
    exit_marked: bool,
    hidden: bool,
    wake_stop: Arc<AtomicBool>,
}

impl SceneApp {
    fn new(cc: &eframe::CreationContext<'_>, options: SceneOptions) -> Self {
        let cover = options
            .cover
            .as_deref()
            .and_then(|path| load_texture_file(&cc.egui_ctx, path, "scene-cover"));
        let backdrop = options
            .backdrop
            .as_deref()
            .and_then(|path| load_texture_file(&cc.egui_ctx, path, "scene-backdrop"));
        Self {
            name: options.name,
            command: options.command,
            end_seconds: options.end_seconds.clamp(0.5, 30.0),
            // La première image du splash doit être peinte avant que le jeu
            // ne puisse la recouvrir : on laisse une courte avance.
            spawn_at: Instant::now() + Duration::from_millis(250),
            child: None,
            ended_at: None,
            error: None,
            bunny: launch::Bunny::load(&cc.egui_ctx),
            cover,
            backdrop,
            exit_file: options.exit_file,
            start_file: options.start_file,
            exit_marked: false,
            hidden: false,
            wake_stop: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Signale la fin du jeu au daemon. Le fichier est posé avant l'écran de
    /// fin : la vidéo ne doit pas enregistrer ces quatre secondes.
    fn mark_game_finished(&mut self) {
        if self.exit_marked {
            return;
        }
        self.exit_marked = true;
        if let Some(path) = self.exit_file.as_ref() {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(path, b"1");
        }
    }

    fn spawn_game(&mut self, ctx: &egui::Context) {
        let Some(program) = self.command.first() else {
            self.error = Some(lapinux_core::i18n::t("game_sheet.no_game_command").into());
            self.ended_at = Some(Instant::now());
            return;
        };
        match std::process::Command::new(program)
            .args(&self.command[1..])
            .spawn()
        {
            Ok(child) => {
                eprintln!("[scene] jeu lancé: {program}");
                self.child = Some(child);
                // Une fenêtre cachée peut ne plus recevoir de redraw : ce fil
                // la réveille pour que la sortie du jeu soit vue même quand la
                // scène s'est effacée derrière lui.
                let ctx = ctx.clone();
                let stop = Arc::clone(&self.wake_stop);
                std::thread::spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        ctx.request_repaint();
                        std::thread::sleep(Duration::from_millis(250));
                    }
                });
            }
            Err(err) => {
                eprintln!("[scene] lancement impossible : {err}");
                self.error = Some(lapinux_core::i18n::t_args(
                    "game_sheet.launch_failed",
                    &[("error", &err.to_string())],
                ));
                self.ended_at = Some(Instant::now());
            }
        }
    }
}

impl eframe::App for SceneApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let now = Instant::now();
        if self.ended_at.is_none() {
            if self.child.is_none() && now >= self.spawn_at {
                self.spawn_game(ctx);
            }
            if let Some(child) = self.child.as_mut() {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        eprintln!("[scene] game finished ({status})");
                        self.child = None;
                        self.mark_game_finished();
                        self.ended_at = Some(now);
                    }
                    Ok(None) => {}
                    Err(err) => {
                        eprintln!("[scene] game state unreadable: {err}");
                        self.child = None;
                        self.mark_game_finished();
                        self.ended_at = Some(now);
                        self.error = Some(err.to_string());
                    }
                }
            }
            if self.ended_at.is_some() {
                self.wake_stop.store(true, Ordering::Relaxed);
                if self.hidden {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    self.hidden = false;
                }
            } else if !self.hidden
                && self.child.is_some()
                && self.start_file.as_ref().is_some_and(|path| path.is_file())
            {
                // GameScope empile la scène au-dessus du jeu : sans ce retrait,
                // son fond opaque restait la seule image visible.
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                self.hidden = true;
                if let Some(path) = self.start_file.as_ref() {
                    let _ = std::fs::remove_file(path);
                }
            }
        }

        if let Some(ended) = self.ended_at {
            let elapsed = now.duration_since(ended).as_secs_f32();
            if elapsed >= self.end_seconds || ctx.input(|input| input.pointer.any_click()) {
                eprintln!("[scene] closing scene");
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }
        }

        let ended = self.ended_at.is_some();
        let left = self
            .ended_at
            .map(|ended| (self.end_seconds - now.duration_since(ended).as_secs_f32()).max(0.0));
        let subtitle = if let Some(error) = self.error.as_ref() {
            error.clone()
        } else if ended {
            lapinux_core::i18n::t("app.session_ended").to_string()
        } else {
            "Lancement…".to_string()
        };
        let elapsed = now.duration_since(self.spawn_at).as_secs_f32().max(0.0);
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(BACKGROUND()))
            .show(ctx, |ui| {
                paint_scene(
                    ui.painter(),
                    ctx.screen_rect(),
                    self.backdrop.as_ref(),
                    self.cover.as_ref(),
                    self.bunny.as_ref(),
                    &self.name,
                    &subtitle,
                    ended,
                    left,
                    elapsed,
                );
            });
        ctx.request_repaint_after(Duration::from_millis(16));
    }
}

impl Drop for SceneApp {
    fn drop(&mut self) {
        self.wake_stop.store(true, Ordering::Relaxed);
        if let Some(path) = self.start_file.as_ref() {
            let _ = std::fs::remove_file(path);
        }
        self.mark_game_finished();
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Signale au daemon que l'interface tourne, toutes les deux secondes.
///
/// Une requête IPC ne conviendrait pas : le daemon peut être occupé par une
/// longue lecture quand la fenêtre se ferme, et la présence resterait alors
/// affichée. Un simple marqueur horodaté survit aussi à une fermeture brutale :
/// le daemon le voit vieillir et retire l'activité de lui-même.
fn start_heartbeat() {
    let paths = LapinuxPaths::discover();
    let marker = paths.ui_marker();
    let pid = paths.ui_pid();
    if let Some(dir) = marker.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&pid, std::process::id().to_string());
    HEARTBEAT_STOP.store(false, Ordering::SeqCst);
    std::thread::spawn(move || {
        while !HEARTBEAT_STOP.load(Ordering::SeqCst) {
            if let Ok(_guard) = HEARTBEAT_WRITE.lock() {
                if HEARTBEAT_STOP.load(Ordering::SeqCst) {
                    return;
                }
                let _ = std::fs::write(&marker, b"");
            }
            for _ in 0..20 {
                if HEARTBEAT_STOP.load(Ordering::SeqCst) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    });
}

/// Le marqueur ne doit pas réapparaître après `on_exit` : le daemon croirait
/// l'interface encore vivante et refuserait de la relancer. Le verrou sépare
/// la dernière écriture du battement de la suppression des marqueurs.
static HEARTBEAT_STOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static HEARTBEAT_WRITE: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Library,
    /// Collections : catégories manuelles et regroupements par crédits.
    Collections,
    Downloads,
    Controllers,
    /// Compte Steam : la page est à droite de Manettes, dans le même bloc.
    Profile,
    /// Boutiques : les quatre comptes et une recherche dans toute leur
    /// bibliothèque, à droite de Profil.
    Boutiques,
    /// Captures : les images et les vidéos prises en jeu par l'overlay.
    Captures,
    /// Actualités : les annonces publiées par les éditeurs des jeux Steam.
    News,
    /// Vue d'ensemble de l'application.
    Home,
}

/// Onglets de la page Manettes : réglages généraux ou liste des appareils.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ControllersTab {
    General,
    Devices,
}

/// Les écrans du quotidien ; les outils (Protons, Système, Diagnostic) vivent
/// dans les réglages, ouverts dans une fenêtre centrée.
fn main_pages() -> [(Page, &'static str); 9] {
    [
        (Page::Home, lapinux_core::i18n::t("page.home")),
        (
            Page::Library,
            lapinux_core::i18n::t("settings.steam.library"),
        ),
        (
            Page::Collections,
            lapinux_core::i18n::t("collections.title"),
        ),
        (Page::News, lapinux_core::i18n::t("gameinfo.news")),
        (Page::Downloads, lapinux_core::i18n::t("app.downloads")),
        (Page::Controllers, lapinux_core::i18n::t("page.controllers")),
        (Page::Profile, lapinux_core::i18n::t("page.profile")),
        (Page::Boutiques, lapinux_core::i18n::t("page.stores")),
        (Page::Captures, lapinux_core::i18n::t("page.captures")),
    ]
}

/// Libellé de la barre du haut, dans la langue active. Le tableau garde les
/// mots français comme repère de développement ; l'affichage passe toujours
/// par cette fonction.
fn page_label(page: Page) -> &'static str {
    match page {
        Page::Home => lapinux_core::i18n::t("page.home"),
        Page::Library => lapinux_core::i18n::t("page.library"),
        Page::Collections => lapinux_core::i18n::t("page.collections"),
        Page::Downloads => lapinux_core::i18n::t("page.downloads"),
        Page::Controllers => lapinux_core::i18n::t("page.controllers"),
        Page::Profile => lapinux_core::i18n::t("page.profile"),
        Page::Boutiques => lapinux_core::i18n::t("page.stores"),
        Page::Captures => lapinux_core::i18n::t("page.captures"),
        Page::News => lapinux_core::i18n::t("page.news"),
    }
}

impl Page {
    /// Clé stable de la section dans la configuration : elle ne change pas
    /// quand le libellé est traduit.
    fn key(self) -> &'static str {
        match self {
            Page::Home => "home",
            Page::Library => "library",
            Page::Collections => "collections",
            Page::Downloads => "downloads",
            Page::Controllers => "controllers",
            Page::Profile => "profile",
            Page::Boutiques => "boutiques",
            Page::Captures => "captures",
            Page::News => "news",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        match key {
            "home" => Some(Page::Home),
            "library" => Some(Page::Library),
            "collections" => Some(Page::Collections),
            "downloads" => Some(Page::Downloads),
            "controllers" => Some(Page::Controllers),
            "profile" => Some(Page::Profile),
            "boutiques" => Some(Page::Boutiques),
            "captures" => Some(Page::Captures),
            "news" => Some(Page::News),
            _ => None,
        }
    }
}

/// Ordre des sections de la barre du haut : celui enregistré, complété par les
/// sections absentes (configuration antérieure ou clé inconnue). Chaque section
/// n'apparaît qu'une fois.
fn topbar_pages(order: &[String]) -> Vec<Page> {
    let mut pages = Vec::with_capacity(main_pages().len());
    for key in order {
        if let Some(page) = Page::from_key(key) {
            if !pages.contains(&page) {
                pages.push(page);
            }
        }
    }
    for (page, _) in main_pages() {
        if !pages.contains(&page) {
            pages.push(page);
        }
    }
    pages
}

fn visible_topbar_pages(order: &[String], hidden: &[String]) -> Vec<Page> {
    let pages = topbar_pages(order);
    let visible: Vec<_> = pages
        .iter()
        .copied()
        .filter(|page| !hidden.iter().any(|key| key == page.key()))
        .collect();
    if visible.is_empty() {
        vec![pages[0]]
    } else {
        visible
    }
}

/// Section à garder après un changement de topbar : la page courante si elle
/// reste visible, sinon la première section visible. Évite de laisser l'app
/// sur un écran dont l'onglet vient d'être retiré.
fn page_after_navigation(current: Page, general: &lapinux_core::config::GeneralConfig) -> Page {
    let visible = visible_topbar_pages(&general.topbar_order, &general.topbar_hidden);
    if visible.contains(&current) {
        current
    } else {
        visible[0]
    }
}

fn startup_page(general: &lapinux_core::config::GeneralConfig, layout: theme::ThemeLayout) -> Page {
    let visible = visible_topbar_pages(&general.topbar_order, &general.topbar_hidden);
    let default = if layout == theme::ThemeLayout::Nx {
        Page::Library
    } else {
        Page::Home
    };
    Page::from_key(&general.startup_page)
        .filter(|page| visible.contains(page))
        .or_else(|| visible.contains(&default).then_some(default))
        .unwrap_or(visible[0])
}

/// Entrée de la bibliothèque unifiée (scan local + catalogue Steam).
#[derive(Clone)]
struct LibraryEntry {
    appid: u32,
    name: String,
    installed: bool,
    /// Présent dans une bibliothèque Steam du disque.
    on_disk: bool,
    /// Possédé directement par le compte Steam.
    steam: bool,
    /// Prêté par un autre membre de la famille Steam.
    family: bool,
    /// Un autre membre de la famille partage aussi ce jeu, même s'il est
    /// possédé directement : les deux pastilles et le choix « Moi » restent
    /// disponibles.
    family_available: bool,
    /// Build Steam plus récent que celui installé sur le disque.
    update: Option<u64>,
    /// Boutique d'origine quand le jeu ne vient pas de Steam (Epic…).
    store: Option<lapinux_core::stores::Store>,
    /// Jaquette publiée par la boutique, téléchargée au premier affichage.
    cover: String,
    /// Plateformes de bureau publiées par la boutique (`windows`, `macos`,
    /// `linux`). Vide pour Steam et pour le disque.
    platforms: Vec<String>,
    /// Jeu également publié sur mobile (Android/iOS) : le filtre Epic peut le
    /// masquer ou ne garder que lui.
    mobile: bool,
    /// Poids à télécharger, quand la boutique l'annonce.
    size: Option<u64>,
    /// Éditeurs lus dans la fiche Steam du jeu : ils viennent du catalogue, qui
    /// porte la fiche PICS de chaque jeu possédé.
    publishers: Vec<String>,
    /// Développeurs de la même fiche.
    developers: Vec<String>,
    /// Genres publiés par la fiche : la matière des collections par genre.
    genres: Vec<String>,
    /// Secondes jouées dans le lanceur : Lapinux chronomètre ses parties, donc
    /// le total couvre Steam comme Epic, GOG et itch.
    playtime: u64,
    /// Date d'obtention publiée par la boutique, en secondes Unix. Pour Steam,
    /// les jeux du compte viennent des licences et les jeux de la famille de
    /// `rt_time_acquired` ; la fiche reste sans date si Steam n'en publie pas.
    acquired: Option<i64>,
    /// Dernière mise à jour connue, en secondes Unix : le manifeste installé
    /// ou la date du build public. Zéro quand aucune source ne la donne.
    updated: i64,
    /// Début de la dernière partie, en secondes Unix. Zéro : jamais lancé.
    last_played: i64,
}

/// Ordre de la bibliothèque, choisi dans le menu des filtres.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LibrarySort {
    Name,
    Playtime,
    /// De la date d'obtention la plus récente à la plus ancienne ; les fiches
    /// que Steam ne date pas passent en fin de liste.
    Acquired,
    /// Dernière partie lancée, la plus récente d'abord.
    Recent,
    /// Dernière mise à jour publiée par la boutique, la plus récente d'abord.
    Updated,
    /// Poids installé, du plus gros au plus petit.
    Size,
}

impl LibrarySort {
    /// Sens proposé quand on choisit ce tri : les dates et les poids se lisent
    /// du plus grand au plus petit, les noms de A à Z.
    fn default_descending(self) -> bool {
        !matches!(self, Self::Name)
    }
}

impl LibrarySort {
    fn from_config(value: lapinux_core::config::LibrarySort) -> Self {
        match value {
            lapinux_core::config::LibrarySort::Name => Self::Name,
            lapinux_core::config::LibrarySort::Playtime => Self::Playtime,
            lapinux_core::config::LibrarySort::Acquired => Self::Acquired,
            lapinux_core::config::LibrarySort::Recent => Self::Recent,
            lapinux_core::config::LibrarySort::Updated => Self::Updated,
            lapinux_core::config::LibrarySort::Size => Self::Size,
        }
    }

    fn to_config(self) -> lapinux_core::config::LibrarySort {
        match self {
            Self::Name => lapinux_core::config::LibrarySort::Name,
            Self::Playtime => lapinux_core::config::LibrarySort::Playtime,
            Self::Acquired => lapinux_core::config::LibrarySort::Acquired,
            Self::Recent => lapinux_core::config::LibrarySort::Recent,
            Self::Updated => lapinux_core::config::LibrarySort::Updated,
            Self::Size => lapinux_core::config::LibrarySort::Size,
        }
    }
}

/// Jeu retenu au dernier contrôle de bibliothèque. L'appid est l'identité
/// courte, mais une boutique peut le réattribuer au même jeu ; le titre sert
/// alors de seconde clé pour ne pas annoncer une fausse nouveauté.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct SeenGame {
    appid: u32,
    name: String,
}

/// Photographie des bibliothèques au dernier lancement : la comparaison avec
/// la lecture courante donne les jeux ajoutés depuis.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct SeenLibrary {
    games: Vec<SeenGame>,
    checked: i64,
}

/// Empreinte de l'état familial : les appids partagés décident des pastilles
/// « compte » et « famille ». La liste fusionnée est gardée entre deux images ;
/// sans cette empreinte, celle construite avant l'arrivée du snapshot social
/// restait sans les jeux à double accès.
fn family_fingerprint(social: Option<&SteamSocial>) -> u64 {
    let Some(family) = social.and_then(|social| social.family.as_ref()) else {
        return 0;
    };
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for member in &family.members {
        // L'ordre des appids d'une réponse ne doit pas périmer la liste : ils
        // entrent par un masque commutatif.
        let mut mask = 0u64;
        for appid in &member.appids {
            mask ^= u64::from(*appid).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        }
        for value in [
            member.steam_id,
            u64::from(member.is_self),
            member.appids.len() as u64,
            mask,
        ] {
            hash ^= value;
            hash = hash.wrapping_mul(0x100_0000_01b3);
        }
    }
    hash
}

/// Ce qui rend la liste fusionnée de la bibliothèque périmée : les données
/// elles-mêmes (compteur d'époque), l'état social et les réglages qui décident
/// quelles sources sont fusionnées.
#[derive(PartialEq, Eq, Clone)]
struct LibraryKey {
    epoch: u64,
    /// Empreinte des appids de la famille : un jeu partagé en plus change les
    /// deux pastilles sans changer le compteur d'époque.
    family: u64,
    hide_tools: bool,
    show_demos: bool,
    hide_all_tools: bool,
    hide_epic_tools: bool,
    hide_gog_tools: bool,
    hide_itch_tools: bool,
    hidden: Vec<u32>,
    show_hidden: bool,
    account: String,
    connected_as: String,
}

/// Liste fusionnée gardée entre deux images, partagée par les pages.
struct LibraryCache {
    key: LibraryKey,
    entries: Rc<Vec<LibraryEntry>>,
}

/// Ce qui rend la vue filtrée périmée : les données, la recherche, les
/// sources, le filtre d'installation, le tri et les crédits choisis.
#[derive(PartialEq, Eq, Clone)]
struct LibraryViewKey {
    epoch: u64,
    /// Adresse de la liste fusionnée : les indices ne valent que pour elle.
    base: usize,
    needle: String,
    sources: SourceFilter,
    catalog_filter: CatalogFilter,
    sort: LibrarySort,
    /// Sens du tri : vrai pour décroissant.
    descending: bool,
    /// Filtres d'attributs, en plus du filtre d'installation.
    only_updates: bool,
    never_played: bool,
    publisher: Option<String>,
    developer: Option<String>,
}

/// Vue filtrée et triée de la bibliothèque, avec les compteurs de la pastille
/// de sources. Gardée entre deux images tant que sa clé ne change pas.
struct LibraryView {
    key: LibraryViewKey,
    /// Indices dans la liste fusionnée, déjà dans l'ordre d'affichage.
    order: Vec<usize>,
    disk: usize,
    steam: usize,
    family: usize,
    all_steam: usize,
    epic: usize,
    epic_pc: usize,
    epic_mobile: usize,
    gog: usize,
    itch: usize,
}

/// Sources affichées dans la bibliothèque : plusieurs peuvent être actives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SourceFilter {
    disk: bool,
    steam: bool,
    family: bool,
    /// Jeux Epic de bureau sélectionnés.
    epic_pc: bool,
    /// Jeux Epic mobiles sélectionnés.
    epic_mobile: bool,
    /// Jeux possédés dans une autre boutique : GOG et itch.io ont chacun leur
    /// pastille, Epic a son groupe PC/mobile.
    stores: [bool; 3],
}

/// Boutiques hors Steam, dans l'ordre de leurs pastilles.
const LIBRARY_STORES: [lapinux_core::stores::Store; 3] = [
    lapinux_core::stores::Store::Epic,
    lapinux_core::stores::Store::Gog,
    lapinux_core::stores::Store::Itch,
];

/// Rang d'une boutique dans [LIBRARY_STORES] : Steam n'en a pas.
fn store_slot(store: lapinux_core::stores::Store) -> Option<usize> {
    LIBRARY_STORES.iter().position(|other| *other == store)
}

impl Default for SourceFilter {
    fn default() -> Self {
        Self {
            disk: true,
            steam: true,
            family: true,
            epic_pc: true,
            epic_mobile: true,
            stores: [true; 3],
        }
    }
}

impl SourceFilter {
    fn keeps_store(&self, store: lapinux_core::stores::Store) -> bool {
        if store == lapinux_core::stores::Store::Epic {
            return self.epic_pc || self.epic_mobile;
        }
        store_slot(store)
            .map(|slot| self.stores[slot])
            .unwrap_or(false)
    }

    fn keeps(&self, entry: &LibraryEntry) -> bool {
        if let Some(store) = entry.store {
            if store == lapinux_core::stores::Store::Epic {
                return (self.epic_pc
                    && lapinux_core::config::EpicPlatform::Pc
                        .keeps(&entry.platforms, entry.mobile))
                    || (self.epic_mobile
                        && lapinux_core::config::EpicPlatform::Mobile
                            .keeps(&entry.platforms, entry.mobile));
            }
            return self.keeps_store(store);
        }
        // Un jeu présent sur le disque est installé : une entrée qui n'a que
        // des fichiers sans installation complète n'a rien à faire dans la
        // source « disque », où elle n'offrirait qu'un bouton « Installer ».
        (self.disk && entry.on_disk && entry.installed)
            || (self.steam && entry.steam && !entry.family)
            || (self.family && (entry.family || entry.family_available))
    }

    fn any(&self) -> bool {
        self.disk
            || self.steam
            || self.family
            || self.epic_pc
            || self.epic_mobile
            || self.stores[1]
            || self.stores[2]
    }

    fn all_steam(&self) -> bool {
        self.steam && self.family
    }

    fn from_config(sources: lapinux_core::config::LibrarySources) -> Self {
        let epic_pc = sources.epic && sources.epic_pc;
        let epic_mobile = sources.epic && sources.epic_mobile;
        Self {
            disk: sources.disk,
            steam: sources.steam,
            family: sources.family,
            epic_pc,
            epic_mobile,
            stores: [epic_pc || epic_mobile, sources.gog, sources.itch],
        }
    }

    fn to_config(self) -> lapinux_core::config::LibrarySources {
        lapinux_core::config::LibrarySources {
            disk: self.disk,
            steam: self.steam,
            family: self.family,
            epic: self.epic_pc || self.epic_mobile,
            epic_pc: self.epic_pc,
            epic_mobile: self.epic_mobile,
            gog: self.stores[1],
            itch: self.stores[2],
        }
    }
}

/// Onglets de la fiche dépliée : le jeu, ses sauvegardes, ses détails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SheetTab {
    General,
    Configurations,
    Saves,
    SteamInput,
    Workshop,
    Details,
}

/// Bilan des copies locales, tel que le daemon le rend.
#[derive(Debug, Clone, serde::Deserialize)]
struct SavesUsage {
    root: String,
    games: usize,
    copies: usize,
    bytes: u64,
}

/// Une ligne de la page Stockage : un jeu et la place qu'il occupe.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StorageRow {
    name: String,
    size: u64,
    installed: bool,
    store: &'static str,
}

/// Tri et totaux de la page Stockage. Ils ne sont recalculés qu'après un
/// scan de la bibliothèque, jamais à chaque image.
#[derive(Default)]
struct StorageCache {
    rows: Vec<StorageRow>,
    total: u64,
    installed: usize,
    unknown: usize,
    disk: Option<DiskUsage>,
    disk_at: Option<Instant>,
    dirty: bool,
}

impl StorageCache {
    fn rebuild(&mut self, games: &[Game]) {
        self.rows.clear();
        self.rows.reserve(games.len());
        self.total = 0;
        self.installed = 0;
        self.unknown = 0;
        for game in games {
            if !game.installed {
                continue;
            }
            if game.installed {
                self.installed += 1;
                if game.size_on_disk == 0 {
                    self.unknown += 1;
                }
            }
            self.total = self.total.saturating_add(game.size_on_disk);
            self.rows.push(StorageRow {
                name: game.name.clone(),
                size: game.size_on_disk,
                installed: game.installed,
                store: game.store.label(),
            });
        }
        // Les plus lourds d'abord ; à poids égal, l'ordre alphabétique évite
        // que la liste saute d'un rafraîchissement à l'autre.
        self.rows
            .sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name)));
        self.dirty = false;
    }

    fn ensure(&mut self, games: &[Game], library_root: &Path) {
        if self.dirty {
            self.rebuild(games);
        }
        // `statvfs` est bon marché, mais la page n'a pas besoin de le relire
        // à chaque image : dix secondes suffisent à suivre une installation.
        let stale = self
            .disk_at
            .map(|at| at.elapsed() >= Duration::from_secs(10))
            .unwrap_or(true);
        if stale {
            self.disk = lapinux_core::system::disk_usage(library_root);
            self.disk_at = Some(Instant::now());
        }
    }
}

#[cfg(test)]
mod storage_tests {
    use super::*;

    fn game(name: &str, size: u64, installed: bool) -> Game {
        Game {
            name: name.to_string(),
            size_on_disk: size,
            installed,
            ..Game::default()
        }
    }

    #[test]
    fn storage_rows_are_sorted_heaviest_first() {
        let mut cache = StorageCache::default();
        cache.rebuild(&[
            game("Petit", 10, true),
            game("Gros", 100, true),
            game("Absent", 999, false),
            game(
                lapinux_core::i18n::t("controllers.information.unknown"),
                0,
                true,
            ),
        ]);
        assert_eq!(
            cache
                .rows
                .iter()
                .map(|row| row.name.as_str())
                .collect::<Vec<_>>(),
            vec![
                "Gros",
                "Petit",
                lapinux_core::i18n::t("controllers.information.unknown")
            ]
        );
        assert_eq!(cache.total, 110);
        assert_eq!(cache.installed, 3);
        assert_eq!(cache.unknown, 1);
    }
}

/// Onglets de la fenêtre Réglages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsTab {
    General,
    Notifications,
    InGame,
    Appearance,
    Library,
    Connection,
    /// Poids des jeux installés, du plus lourd au plus léger.
    Storage,
    Saves,
    Protons,
    Graphics,
    /// Captures : l'enregistreur vidéo et le dossier où tout arrive.
    Captures,
    System,
    Doctor,
    Logs,
    Updates,
    Application,
}

/// Ordre du rail : les réglages du quotidien d'abord, les outils ensuite.
/// Chaque groupe garde ses onglets ensemble, et les données du joueur
/// (sauvegardes, Proton, captures) ne se mélangent plus aux diagnostics.
fn settings_tabs_list() -> [(SettingsTab, &'static str); 16] {
    [
        (
            SettingsTab::General,
            lapinux_core::i18n::t("controllers.general"),
        ),
        (
            SettingsTab::Notifications,
            lapinux_core::i18n::t("settings.tab.notifications"),
        ),
        (
            SettingsTab::InGame,
            lapinux_core::i18n::t("settings.tab.in_game"),
        ),
        (
            SettingsTab::Appearance,
            lapinux_core::i18n::t("settings.tab.appearance"),
        ),
        (
            SettingsTab::Library,
            lapinux_core::i18n::t("settings.steam.library"),
        ),
        (
            SettingsTab::Connection,
            lapinux_core::i18n::t("settings.tab.accounts"),
        ),
        (
            SettingsTab::Storage,
            lapinux_core::i18n::t("settings.tab.storage"),
        ),
        (
            SettingsTab::Saves,
            lapinux_core::i18n::t("settings.tab.saves"),
        ),
        (
            SettingsTab::Protons,
            lapinux_core::i18n::t("settings.tab.protons"),
        ),
        (
            SettingsTab::Graphics,
            lapinux_core::i18n::t("graphics.title"),
        ),
        (
            SettingsTab::Captures,
            lapinux_core::i18n::t("page.captures"),
        ),
        (
            SettingsTab::System,
            lapinux_core::i18n::t("settings.application.system"),
        ),
        (
            SettingsTab::Doctor,
            lapinux_core::i18n::t("settings.tab.diagnostics"),
        ),
        (
            SettingsTab::Logs,
            lapinux_core::i18n::t("settings.application.logs"),
        ),
        (
            SettingsTab::Updates,
            lapinux_core::i18n::t("settings.tab.updates"),
        ),
        (
            SettingsTab::Application,
            lapinux_core::i18n::t("settings.general.application_card"),
        ),
    ]
}

impl SettingsTab {
    /// Catégorie du rail : les réglages du quotidien, les jeux et leurs
    /// données, puis les outils de la machine.
    fn group(self) -> usize {
        match self {
            SettingsTab::General
            | SettingsTab::Notifications
            | SettingsTab::InGame
            | SettingsTab::Appearance
            | SettingsTab::Library => 0,
            SettingsTab::Connection
            | SettingsTab::Storage
            | SettingsTab::Saves
            | SettingsTab::Protons
            | SettingsTab::Graphics
            | SettingsTab::Captures => 1,
            SettingsTab::System
            | SettingsTab::Doctor
            | SettingsTab::Logs
            | SettingsTab::Updates
            | SettingsTab::Application => 2,
        }
    }

    /// Position dans sa catégorie : les icônes gardent une identité stable
    /// quand l'ordre du rail change.
    fn group_index(self) -> usize {
        match self {
            SettingsTab::General => 0,
            SettingsTab::Notifications => 1,
            SettingsTab::InGame => 2,
            SettingsTab::Appearance => 3,
            SettingsTab::Library => 4,
            SettingsTab::Connection => 0,
            SettingsTab::Storage => 1,
            SettingsTab::Saves => 2,
            SettingsTab::Protons => 3,
            SettingsTab::Graphics => 4,
            SettingsTab::Captures => 5,
            SettingsTab::System => 0,
            SettingsTab::Doctor => 1,
            SettingsTab::Logs => 2,
            SettingsTab::Updates => 3,
            SettingsTab::Application => 4,
        }
    }

    fn title(self) -> &'static str {
        match self {
            SettingsTab::General => lapinux_core::i18n::t("settings.tab.general"),
            SettingsTab::Notifications => lapinux_core::i18n::t("settings.tab.notifications"),
            SettingsTab::InGame => lapinux_core::i18n::t("settings.tab.in_game"),
            SettingsTab::Library => lapinux_core::i18n::t("settings.tab.library"),
            SettingsTab::Appearance => lapinux_core::i18n::t("settings.tab.appearance"),
            SettingsTab::Connection => lapinux_core::i18n::t("settings.tab.accounts"),
            SettingsTab::Storage => lapinux_core::i18n::t("settings.tab.storage"),
            SettingsTab::Logs => lapinux_core::i18n::t("settings.tab.logs"),
            SettingsTab::Saves => lapinux_core::i18n::t("settings.tab.saves"),
            SettingsTab::Protons => lapinux_core::i18n::t("settings.tab.protons"),
            SettingsTab::Graphics => lapinux_core::i18n::t("graphics.title"),
            SettingsTab::System => lapinux_core::i18n::t("settings.tab.system"),
            SettingsTab::Doctor => lapinux_core::i18n::t("settings.tab.diagnostics"),
            SettingsTab::Updates => lapinux_core::i18n::t("settings.tab.updates"),
            SettingsTab::Application => lapinux_core::i18n::t("settings.tab.application"),
            SettingsTab::Captures => lapinux_core::i18n::t("settings.tab.captures"),
        }
    }

    fn subtitle(self) -> &'static str {
        match self {
            SettingsTab::General => lapinux_core::i18n::t("settings.desc.general"),
            SettingsTab::Notifications => lapinux_core::i18n::t("settings.desc.notifications"),
            SettingsTab::InGame => lapinux_core::i18n::t("settings.desc.in_game"),
            SettingsTab::Library => lapinux_core::i18n::t("settings.desc.library"),
            SettingsTab::Appearance => lapinux_core::i18n::t("settings.desc.appearance"),
            SettingsTab::Connection => lapinux_core::i18n::t("settings.desc.accounts"),
            SettingsTab::Storage => lapinux_core::i18n::t("settings.desc.storage"),
            SettingsTab::Logs => lapinux_core::i18n::t("settings.desc.logs"),
            SettingsTab::Saves => lapinux_core::i18n::t("settings.desc.saves"),
            SettingsTab::Protons => lapinux_core::i18n::t("settings.desc.protons"),
            SettingsTab::Graphics => lapinux_core::i18n::t("graphics.subtitle"),
            SettingsTab::System => lapinux_core::i18n::t("settings.desc.system"),
            SettingsTab::Doctor => lapinux_core::i18n::t("settings.desc.diagnostics"),
            SettingsTab::Updates => lapinux_core::i18n::t("settings.desc.updates"),
            SettingsTab::Application => lapinux_core::i18n::t("settings.desc.application"),
            SettingsTab::Captures => lapinux_core::i18n::t("settings.desc.captures"),
        }
    }
}

/// Onglets de la fenêtre de réglages d'un jeu. Ils reprennent le principe de
/// la fenêtre Réglages : une seule famille de contrôles à la fois, plutôt
/// qu'un long formulaire où il faut chercher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GameSettingsTab {
    Launch,
    InGame,
    Display,
    Graphics,
    Captures,
    Saves,
    Compatibility,
    Language,
    Info,
}

const GAME_SETTINGS_TABS: [GameSettingsTab; 9] = [
    GameSettingsTab::Launch,
    GameSettingsTab::InGame,
    GameSettingsTab::Display,
    GameSettingsTab::Graphics,
    GameSettingsTab::Captures,
    GameSettingsTab::Saves,
    GameSettingsTab::Compatibility,
    GameSettingsTab::Language,
    GameSettingsTab::Info,
];

impl GameSettingsTab {
    fn title(self) -> &'static str {
        match self {
            GameSettingsTab::Launch => lapinux_core::i18n::t("game_settings.tab_launch"),
            GameSettingsTab::InGame => lapinux_core::i18n::t("game_settings.tab_in_game"),
            GameSettingsTab::Display => lapinux_core::i18n::t("game_settings.tab_display"),
            GameSettingsTab::Graphics => lapinux_core::i18n::t("game_settings.tab_graphics"),
            GameSettingsTab::Captures => lapinux_core::i18n::t("game_settings.tab_captures"),
            GameSettingsTab::Saves => lapinux_core::i18n::t("game_settings.tab_saves"),
            GameSettingsTab::Compatibility => {
                lapinux_core::i18n::t("game_settings.tab_compatibility")
            }
            GameSettingsTab::Language => lapinux_core::i18n::t("game_settings.tab_language"),
            GameSettingsTab::Info => lapinux_core::i18n::t("game_settings.tab_info"),
        }
    }

    fn subtitle(self) -> &'static str {
        match self {
            GameSettingsTab::Launch => lapinux_core::i18n::t("game_settings.desc_launch"),
            GameSettingsTab::InGame => lapinux_core::i18n::t("game_settings.desc_in_game"),
            GameSettingsTab::Display => lapinux_core::i18n::t("game_settings.desc_display"),
            GameSettingsTab::Graphics => lapinux_core::i18n::t("game_settings.desc_graphics"),
            GameSettingsTab::Captures => lapinux_core::i18n::t("game_settings.desc_captures"),
            GameSettingsTab::Saves => lapinux_core::i18n::t("game_settings.desc_saves"),
            GameSettingsTab::Compatibility => {
                lapinux_core::i18n::t("game_settings.desc_compatibility")
            }
            GameSettingsTab::Language => lapinux_core::i18n::t("game_settings.desc_language"),
            GameSettingsTab::Info => lapinux_core::i18n::t("game_settings.desc_info"),
        }
    }

    /// Les icônes du rail global servent aussi ici : elles rendent les onglets
    /// reconnaissables sans ajouter une seconde famille de pictogrammes.
    fn icon(self) -> SettingsTab {
        match self {
            GameSettingsTab::Launch => SettingsTab::General,
            GameSettingsTab::InGame => SettingsTab::InGame,
            GameSettingsTab::Display => SettingsTab::Appearance,
            GameSettingsTab::Graphics => SettingsTab::Graphics,
            GameSettingsTab::Captures => SettingsTab::Captures,
            GameSettingsTab::Saves => SettingsTab::Saves,
            GameSettingsTab::Compatibility => SettingsTab::Protons,
            GameSettingsTab::Language => SettingsTab::Connection,
            GameSettingsTab::Info => SettingsTab::Application,
        }
    }
}

/// Filtre de la bibliothèque Steam (catalogue du compte).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CatalogFilter {
    All,
    Installed,
    Missing,
}

impl CatalogFilter {
    fn keeps(self, installed: bool) -> bool {
        match self {
            CatalogFilter::All => true,
            CatalogFilter::Installed => installed,
            CatalogFilter::Missing => !installed,
        }
    }

    fn from_config(value: lapinux_core::config::LibraryInstallFilter) -> Self {
        match value {
            lapinux_core::config::LibraryInstallFilter::All => Self::All,
            lapinux_core::config::LibraryInstallFilter::Installed => Self::Installed,
            lapinux_core::config::LibraryInstallFilter::Missing => Self::Missing,
        }
    }

    fn to_config(self) -> lapinux_core::config::LibraryInstallFilter {
        match self {
            Self::All => lapinux_core::config::LibraryInstallFilter::All,
            Self::Installed => lapinux_core::config::LibraryInstallFilter::Installed,
            Self::Missing => lapinux_core::config::LibraryInstallFilter::Missing,
        }
    }
}

struct Logo {
    texture: egui::TextureHandle,
    aspect: f32,
}

impl Logo {
    fn load(ctx: &egui::Context) -> Option<Self> {
        Self::load_bytes(
            ctx,
            "lapinux-logo",
            include_bytes!("../../../assets/lapinux-logo.png"),
        )
    }

    fn load_bytes(ctx: &egui::Context, name: &str, bytes: &[u8]) -> Option<Self> {
        let image = image::load_from_memory(bytes).ok()?.to_rgba8();
        let size = [image.width() as usize, image.height() as usize];
        let color = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
        // Les traits sont fins : les mipmaps évitent le crénelage quand la
        // barre de fenêtre ne laisse que quelques pixels de haut.
        let options = egui::TextureOptions {
            mipmap_mode: Some(egui::TextureFilter::Linear),
            ..egui::TextureOptions::LINEAR
        };
        let texture = ctx.load_texture(name, color, options);
        Some(Self {
            aspect: size[0] as f32 / size[1] as f32,
            texture,
        })
    }

    fn draw(&self, painter: &egui::Painter, center: egui::Pos2, height: f32, alpha: f32) {
        let width = height * self.aspect;
        painter.image(
            self.texture.id(),
            egui::Rect::from_center_size(center, egui::vec2(width, height)),
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::from_rgba_unmultiplied(
                255,
                255,
                255,
                (alpha.clamp(0.0, 1.0) * 255.0) as u8,
            ),
        );
    }
}

/// Lancement en cours : la requête au daemon part en tâche de fond, sinon
/// l'interface se figerait et le lapin d'attente ne pourrait pas bouger.
/// Essai à blanc affiché dans la fiche, sous l'onglet Informations : la
/// commande préparée par le daemon reste dans le contexte du jeu au lieu
/// d'ouvrir une fenêtre par-dessus la bibliothèque.
#[derive(Clone)]
struct DryRunPlan {
    appid: u32,
    layers: String,
    command: String,
}

struct Launching {
    /// Jeu lancé : sa fiche relira l'état du cloud que le daemon vient de
    /// reprendre.
    appid: u32,
    /// Nom du jeu, affiché sous le lapin.
    name: String,
    /// Essai à blanc : on montre la commande au lieu de couvrir la fenêtre.
    dry_run: bool,
    /// Le daemon enveloppe le jeu dans la scène : le splash est déjà dessiné
    /// dans GameScope, l'interface ne doit pas en ouvrir un second.
    scene: bool,
    /// Demande déposée par le tray : le splash couvre l'interface tout de
    /// suite, au lieu d'attendre le délai d'apparition habituel.
    from_tray: bool,
    started: Instant,
    rx: mpsc::Receiver<Result<Envelope, String>>,
}

/// La requête de lancement est revenue mais le jeu n'a pas encore peint sa
/// première image : l'écran d'attente reste affiché dans sa propre fenêtre,
/// jusqu'à ce que KWin signale la fenêtre du jeu.
#[derive(Clone)]
struct LaunchHold {
    appid: u32,
    name: String,
    /// La scène GameScope sert de visuel : l'écran de l'interface n'est qu'un
    /// relais pendant l'initialisation du compositeur, puis s'efface sans fondu.
    scene: bool,
    /// Temps écoulé avant la réponse : les animations continuent sans sauter.
    elapsed: f32,
    /// Début de cette seconde phase, pour l'expiration.
    since: Instant,
    /// Une session a été vue pour ce jeu : son absence ensuite veut dire que
    /// le jeu s'est arrêté avant d'afficher quoi que ce soit.
    seen_session: bool,
    /// Premier signe de rendu observé (swapchain). Le splash reste encore un
    /// instant après : la swapchain est créée avant que le menu ou la scène du
    /// jeu ne soit vraiment visible.
    render_seen_at: Option<Instant>,
    /// Première cadence réellement mesurée par GameScope. C'est un signal plus
    /// proche de l'image présentée que la création de la swapchain ; le splash
    /// attend encore un peu après lui.
    fps_seen_at: Option<Instant>,
    /// Le lancement vient du tray : le voile doit être opaque dès la première
    /// image, sans laisser voir la bibliothèque derrière.
    from_tray: bool,
}

/// Écran d'attente qui s'efface : la réponse du lancement est arrivée, la
/// couverture reste dessinée le temps du fondu. Sans cela, un écran plein
/// cadre disparaissait d'une image à l'autre.
struct LaunchFade {
    appid: u32,
    name: String,
    /// Temps d'attente écoulé au moment de la réponse : les animations du
    /// dernier écran continuent là où elles étaient.
    elapsed: f32,
    at: Instant,
}

/// Sous-ensemble Remix Icon embarqué dans `assets/fonts/RemixIcons.ttf`.
/// Les icônes sont des glyphes vectoriels, rasterisés à la taille exacte de
/// l'écran : aucun redimensionnement de PNG ne vient les adoucir.
fn icon_font(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name("Icons".into()))
}

fn nav_glyph(page: Page) -> char {
    match page {
        Page::Home => ' ',
        Page::Library => '\u{eadb}',
        // Le pictogramme des collections est un dossier dessiné au trait.
        Page::Collections => ' ',
        Page::Downloads => '\u{ec5a}',
        Page::Controllers => '\u{edab}',
        // Le sous-ensemble Remix embarqué n'a pas de glyphe « personne » : le
        // pictogramme du profil est dessiné au trait (`paint_profile_glyph`),
        // comme les icônes de marque.
        Page::Profile => ' ',
        // Le pictogramme des boutiques est dessiné (un sac), faute de glyphe
        // lisible dans le sous-ensemble Remix.
        Page::Boutiques => ' ',
        // L'appareil photo des captures est dessiné au trait, comme le profil
        // et le sac : le sous-ensemble Remix n'en a pas.
        Page::Captures => ' ',
        // Le journal des actualités est dessiné au trait, lui aussi.
        Page::News => ' ',
    }
}

/// Pictogramme des collections : un dossier au trait, dans la même graisse
/// que le profil et le sac.
fn paint_collection_glyph(
    painter: &egui::Painter,
    center: egui::Pos2,
    size: f32,
    ink: egui::Color32,
) {
    let scale = size / 19.0;
    let stroke = egui::Stroke::new(1.5_f32 * scale, ink);
    let body = egui::Rect::from_center_size(
        center + egui::vec2(0.0, 1.6) * scale,
        egui::vec2(14.0, 10.0) * scale,
    );
    painter.rect_stroke(body, egui::Rounding::same(1.8 * scale), stroke);
    let tab = egui::Rect::from_min_max(
        center + egui::vec2(-7.0, -6.4) * scale,
        center + egui::vec2(-2.0, -3.4) * scale,
    );
    painter.rect_filled(tab, egui::Rounding::same(1.0 * scale), ink);
    // Deux fiches qui dépassent du dossier.
    painter.line_segment(
        [
            center + egui::vec2(-4.5, -1.8) * scale,
            center + egui::vec2(4.5, -1.8) * scale,
        ],
        egui::Stroke::new(1.0_f32 * scale, ink),
    );
}

/// Glyphes d'action du sous-ensemble Remix : alimentation, crayon, coche.
const ICON_POWER: char = '\u{f126}';
const ICON_EDIT: char = '\u{ec86}';
const ICON_CHECK: char = '\u{eb7a}';

/// Glyphe du logo de marque, dans le même sous-ensemble Remix.
fn brand_glyph(brand: Brand) -> char {
    match brand {
        Brand::PlayStation => '\u{f015}',
        Brand::Xbox => '\u{f2d1}',
        Brand::Nintendo => '\u{f1cd}',
        Brand::Steam => '\u{f191}',
        // La marque générique est dessinée (globe), pas prise dans la police.
        Brand::Generic => '\u{edab}',
    }
}

/// Largeur du logo dans la liste : le globe est dessiné, les autres sont des
/// glyphes de la police d'icônes.
fn brand_mark_advance(painter: &egui::Painter, brand: Brand, size: f32) -> f32 {
    match brand {
        Brand::Generic => size,
        _ => {
            painter
                .layout_no_wrap(brand_glyph(brand).to_string(), icon_font(size), TEXT())
                .size()
                .x
        }
    }
}

/// Dessine le logo de marque à gauche de `left`, centré verticalement sur
/// `center_y`. Le globe générique est composé au trait pour rester net et
/// cohérent avec les icônes de l'application.
fn paint_brand_mark(
    painter: &egui::Painter,
    brand: Brand,
    left: f32,
    center_y: f32,
    size: f32,
    color: egui::Color32,
) {
    if brand != Brand::Generic {
        painter.text(
            egui::pos2(left, center_y),
            egui::Align2::LEFT_CENTER,
            brand_glyph(brand).to_string(),
            icon_font(size),
            color,
        );
        return;
    }
    let radius = size * 0.46;
    let center = egui::pos2(left + radius, center_y);
    let stroke = egui::Stroke::new(1.4_f32, color);
    painter.circle_stroke(center, radius, stroke);
    let meridian: Vec<egui::Pos2> = (0..=28)
        .map(|index| {
            let angle = index as f32 / 28.0 * std::f32::consts::TAU;
            center + egui::vec2(radius * 0.45 * angle.cos(), radius * angle.sin())
        })
        .collect();
    painter.add(egui::Shape::line(meridian, stroke));
    for latitude in [-0.5_f32, 0.0, 0.5] {
        let y = center.y + radius * latitude;
        let half = (radius * radius - (radius * latitude).powi(2)).sqrt();
        painter.line_segment(
            [
                egui::pos2(center.x - half, y),
                egui::pos2(center.x + half, y),
            ],
            egui::Stroke::new(1.0_f32, color),
        );
    }
}

struct Startup {
    started: Instant,
    done: bool,
    /// Première réponse du daemon reçue : l'écran de démarrage peut céder la place.
    ready: bool,
    /// Début du fondu de sortie.
    closing: Option<Instant>,
    /// La première mise en page complète a eu lieu : l'écran de démarrage peut
    /// céder la place sans laisser une fenêtre figée derrière lui.
    painted: bool,
}

impl Startup {
    fn new() -> Self {
        Self {
            started: Instant::now(),
            done: false,
            ready: false,
            closing: None,
            painted: false,
        }
    }
}

/// Largeur de la colonne de lecture : plus étroite qu'une page entière,
/// pour que les lignes restent lisibles.
const NEWS_READER_WIDTH: f32 = 720.0;
/// Largeur du sommaire daté, quand l'écran laisse la place.
const NEWS_INDEX_WIDTH: f32 = 196.0;
/// Espace entre la colonne de lecture et son sommaire.
const NEWS_INDEX_GAP: f32 = 16.0;
/// Largeur réservée à la barre de défilement de la colonne.
const NEWS_SCROLLBAR_WIDTH: f32 = 12.0;

#[derive(Clone)]
struct NewsReader {
    items: Vec<lapinux_core::model::SteamNews>,
    expanded: Vec<bool>,
    /// Hauteur mesurée de chaque section. Les sections hors écran sont sautées
    /// au lieu d'être remises en page à chaque image.
    heights: Vec<f32>,
    /// Largeur de la dernière mesure : un redimensionnement invalide les
    /// hauteurs, qui dépendent du retour à la ligne.
    layout_width: f32,
    /// Article sous le pointeur, pour que le clic extérieur puisse l'effacer.
    hovered: Option<usize>,
    /// Article demandé par le sommaire, à faire défiler au prochain passage.
    scroll_to: Option<usize>,
    /// Instant d'ouverture : le clic qui a ouvert le lecteur ne doit pas le
    /// refermer dans la même image en touchant le voile.
    opened_at: Instant,
}

/// Événement d'ami affiché temporairement à la place de l'icône Amis :
/// avatar, légende courte et instant de départ.
#[derive(Clone)]
struct FriendActivityFlash {
    steam_id: u64,
    name: String,
    avatar: String,
    game: String,
    appid: u32,
    caption: String,
    started: Instant,
}

const FRIEND_ACTIVITY_FLASH_SECS: f32 = 6.0;

/// Diagnostic de performance : avec `LAPINUX_UI_TRACE=1`, la cadence réelle et
/// le coût moyen de la peinture sont écrits sur la sortie d'erreur toutes les
/// cinq secondes. Sans la variable, il ne coûte qu'une lecture unique.
struct FrameTrace {
    started: Instant,
}

impl FrameTrace {
    fn start() -> Self {
        Self {
            started: Instant::now(),
        }
    }
}

fn trace_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("LAPINUX_UI_TRACE").is_some())
}

/// Instant du lancement de l'interface : il donne le délai avant la première
/// image, utile pour juger un démarrage à froid.
fn ui_start() -> &'static Instant {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now)
}

impl Drop for FrameTrace {
    fn drop(&mut self) {
        if !trace_enabled() {
            return;
        }
        static FIRST: std::sync::Once = std::sync::Once::new();
        FIRST.call_once(|| {
            eprintln!(
                "Lapinux première image : {} ms",
                ui_start().elapsed().as_millis()
            );
        });
        // Une image qui dépasse ce seuil explique un gel ressenti : elle est
        // signalée avec le temps passé dans les sondages avant la peinture.
        let painted = self.started.elapsed().as_secs_f32() * 1000.0;
        if painted > 200.0 {
            let polls = FRAME_MARK
                .lock()
                .ok()
                .and_then(|mark| *mark)
                .map(|start| start.elapsed().as_secs_f32() * 1000.0)
                .unwrap_or(0.0);
            let page = PAGE_MS.lock().map(|ms| *ms).unwrap_or(0.0);
            eprintln!(
                "image lente : {painted:.0} ms (sondages et reste {polls:.0} ms, page {page:.0} ms)"
            );
        }
        static WINDOW: Mutex<Option<(Instant, u32, f32)>> = Mutex::new(None);
        let Ok(mut window) = WINDOW.lock() else {
            return;
        };
        let (since, frames, total) = window.get_or_insert_with(|| (Instant::now(), 0, 0.0));
        *frames += 1;
        *total += self.started.elapsed().as_secs_f32() * 1000.0;
        if since.elapsed() >= Duration::from_secs(5) {
            let seconds = since.elapsed().as_secs_f32();
            eprintln!(
                "Lapinux images : {:.1}/s · peinture {:.2} ms en moyenne",
                *frames as f32 / seconds,
                *total / *frames as f32
            );
            window.replace((Instant::now(), 0, 0.0));
        }
    }
}

/// Données relues périodiquement. Le daemon est interrogé depuis un fil
/// séparé pour que l'interface continue de répondre pendant les E/S.
/// Jaquette d'un jeu fraîchement obtenu : elle traverse l'écran jusqu'à
/// l'onglet Bibliothèque, puis s'y tient un instant avec le mot « Nouveau ».
#[derive(Debug, Clone)]
struct GameReveal {
    appid: u32,
    name: String,
    started: Instant,
}

/// Activation dont Steam n'a pas encore nommé le jeu : la célébration attend
/// le catalogue rafraîchi, qui lui donnera la jaquette, l'icône et le vrai nom.
#[derive(Debug, Clone)]
struct PendingReveal {
    /// Nom lu dans le reçu, quand Steam en donne un.
    name: Option<String>,
    since: Instant,
    /// Jeux déjà connus : ce qui n'en fait pas partie vient d'arriver.
    known: std::collections::HashSet<u32>,
}

/// Cadrage « contain » : la jaquette entière tient dans la case, centrée, sans
/// être rognée ni déformée. Les marges prennent le fond du panneau.
fn contain_fit_rect(target: egui::Rect, source: egui::Vec2) -> egui::Rect {
    let ratio = source.x.max(1.0) / source.y.max(1.0);
    let wanted = target.width() / target.height().max(1.0);
    if ratio > wanted {
        let height = target.width() / ratio;
        egui::Rect::from_center_size(target.center(), egui::vec2(target.width(), height))
    } else {
        let width = target.height() * ratio;
        egui::Rect::from_center_size(target.center(), egui::vec2(width, target.height()))
    }
}

struct UiSnapshot {
    games: Vec<Game>,
    protons: Vec<ProtonInfo>,
    jobs: Vec<Job>,
    session: Option<SessionInfo>,
    devices: Vec<DeviceInfo>,
    steam_status: Option<SteamStatus>,
    /// Configuration du daemon : elle peut changer de son côté (écran principal
    /// détecté, migration), l'interface doit suivre.
    config: Option<Config>,
}

/// Budget des textures gardées en mémoire : une grande bibliothèque ne doit
/// pas garder toutes ses jaquettes décodées en pixels. Au-delà de la limite,
/// les textures les moins récemment vues sortent du cache ; leurs fichiers
/// restent sur disque, l'image revient donc au prochain affichage.
pub(crate) struct TextureBudget<K: Eq + std::hash::Hash + Clone> {
    /// Du plus ancien au plus récent usage.
    order: VecDeque<K>,
    sizes: HashMap<K, usize>,
    /// Dernier usage : une texture peinte à l'image précédente ne doit jamais
    /// sortir du cache, sinon elle serait décodée et renvoyée à la carte à
    /// chaque image — un va-et-vient permanent pour la même vignette.
    last: HashMap<K, Instant>,
    bytes: usize,
    limit: usize,
}

/// Délai pendant lequel une texture reste protégée après son dernier usage.
/// Il couvre plusieurs images, même au repos, sans garder éternellement ce qui
/// a défilé hors de l'écran. Deux secondes suffisent : au repos les tuiles
/// visibles sont retouchées à chaque image, et un défilement rapide peut ainsi
/// libérer la mémoire qu'il vient de traverser.
const TEXTURE_PROTECT: Duration = Duration::from_secs(2); // textures visibles épargnées

impl<K: Eq + std::hash::Hash + Clone> TextureBudget<K> {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            order: VecDeque::new(),
            sizes: HashMap::new(),
            last: HashMap::new(),
            bytes: 0,
            limit,
        }
    }

    /// Note qu'une texture vient d'être utilisée, et son poids en pixels.
    pub(crate) fn note(&mut self, key: K, texture: &egui::TextureHandle) {
        let [width, height] = texture.size();
        let bytes = width.max(1) * height.max(1) * 4;
        if let Some(previous) = self.sizes.insert(key.clone(), bytes) {
            self.bytes = self.bytes.saturating_sub(previous);
        }
        self.bytes += bytes;
        self.touch(&key);
    }

    /// Remet la clé en fin de file : elle est la plus récemment vue.
    pub(crate) fn touch(&mut self, key: &K) {
        if let Some(position) = self.order.iter().position(|item| item == key) {
            self.order.remove(position);
        }
        self.order.push_back(key.clone());
        self.last.insert(key.clone(), Instant::now());
    }

    /// Clés à oublier pour repasser sous le budget, de la plus ancienne à la
    /// plus récente. Les textures encore visibles au dernier passage sont
    /// épargnées : le budget ne peut donc pas descendre sous la taille des
    /// tuiles affichées, mais il ne provoque jamais de rechargement en boucle.
    pub(crate) fn trim(&mut self) -> Vec<K> {
        if self.bytes <= self.limit {
            return Vec::new();
        }
        let now = Instant::now();
        let mut out = Vec::new();
        let mut index = 0;
        while self.bytes > self.limit && index < self.order.len() {
            let key = self.order[index].clone();
            let visible = self
                .last
                .get(&key)
                .map_or(false, |at| now.duration_since(*at) < TEXTURE_PROTECT);
            if visible {
                index += 1;
                continue;
            }
            self.order.remove(index);
            if let Some(bytes) = self.sizes.remove(&key) {
                self.bytes = self.bytes.saturating_sub(bytes);
            }
            self.last.remove(&key);
            out.push(key);
        }
        out
    }

    /// Poids actuel des textures gardées (diagnostic).
    pub(crate) fn bytes(&self) -> usize {
        self.bytes
    }

    fn forget(&mut self, key: &K) {
        if let Some(bytes) = self.sizes.remove(key) {
            self.bytes = self.bytes.saturating_sub(bytes);
        }
        self.last.remove(key);
        if let Some(position) = self.order.iter().position(|item| item == key) {
            self.order.remove(position);
        }
    }
}

/// Poids maximal des jaquettes décodées : de quoi couvrir deux écrans de
/// grille, sans garder une bibliothèque entière en mémoire. Une jaquette qui
/// sort est relue du disque au défilement suivant, en quelques millisecondes.
const COVER_BUDGET: usize = 96 * 1024 * 1024;

/// Captures demandées pour l'aperçu de survol.
const HOVER_SHOT_LIMIT: usize = 3;
/// Téléchargements de fiches de captures simultanés au maximum.
const HOVER_SHOT_FETCHES: usize = 4;

/// Délai laissé à une demande de jaquette pour écrire son image sur le
/// disque : passé ce temps, le fichier absent veut dire que la source n'a rien
/// pour ce jeu. Sans ce constat, chaque jaquette introuvable était reproposée
/// toutes les 30 s, indéfiniment.
const COVER_SETTLE: Duration = Duration::from_secs(20);

/// Poids maximal des illustrations de fiches (bannière, captures, visuels de
/// contenus) : la fiche n'en montre qu'une poignée à la fois.
const ART_BUDGET: usize = 64 * 1024 * 1024;

/// À quelle taille une jaquette est demandée. Une tuile de grille est peinte
/// sur quelques dizaines de pixels de large : lui donner la jaquette 600×900
/// revenait à garder seize fois trop de pixels par jeu, et une bibliothèque
/// entière affichée dépassait la centaine de mégaoctets. La fiche, elle, garde
/// une image nette même sur un écran HiDPI.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum CoverTier {
    /// Vignette de grille, réduite à la taille réellement peinte.
    Tile,
    /// Jaquette de fiche, du lancement et de la pastille du jeu en cours.
    Sheet,
}

impl CoverTier {
    fn suffix(self) -> &'static str {
        match self {
            CoverTier::Tile => "tuile",
            CoverTier::Sheet => "fiche",
        }
    }
}

/// Captures d'un jeu pour l'aperçu de survol. Les adresses sont d'abord lues
/// dans la fiche déjà en cache ; à défaut, Steam est interrogé en tâche de
/// fond, une seule fois par jeu. Les images elles-mêmes restent dans le cache
/// des visuels de fiche.
#[derive(Default)]
struct HoverShots {
    urls: HashMap<u32, Vec<String>>,
    pending: HashSet<u32>,
    /// Jeux dont les captures attendent un créneau de lecture.
    queued: std::collections::VecDeque<u32>,
    queued_set: HashSet<u32>,
    tx: Option<mpsc::Sender<(u32, Vec<String>)>>,
    rx: Option<mpsc::Receiver<(u32, Vec<String>)>>,
}

impl HoverShots {
    fn ensure(
        &mut self,
        ctx: &egui::Context,
        paths: &LapinuxPaths,
        appid: u32,
        store: Option<lapinux_core::stores::Store>,
    ) {
        if self.urls.contains_key(&appid)
            || self.pending.contains(&appid)
            || self.queued_set.contains(&appid)
        {
            return;
        }
        if let Some(urls) = cached_screenshot_urls(paths, appid, store) {
            self.urls.insert(appid, urls);
            return;
        }
        // Les boutiques n'ont pas toutes une API publique : sans fiche locale,
        // l'aperçu garde simplement la capsule.
        if store.is_some() {
            self.urls.insert(appid, Vec::new());
            return;
        }
        self.queued.push_back(appid);
        self.queued_set.insert(appid);
        self.pump(ctx);
    }

    /// Démarre les lectures suivantes dans la limite des fils autorisés.
    fn pump(&mut self, ctx: &egui::Context) {
        while self.pending.len() < HOVER_SHOT_FETCHES {
            let Some(appid) = self.queued.pop_front() else {
                break;
            };
            self.queued_set.remove(&appid);
            if self.urls.contains_key(&appid) || self.pending.contains(&appid) {
                continue;
            }
            if self.tx.is_none() {
                let (tx, rx) = mpsc::channel();
                self.tx = Some(tx);
                self.rx = Some(rx);
            }
            let Some(tx) = self.tx.clone() else {
                break;
            };
            self.pending.insert(appid);
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let urls = steam_screenshot_urls(appid);
                if tx.send((appid, urls)).is_ok() {
                    ctx.request_repaint();
                }
            });
        }
    }

    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.rx {
            while let Ok((appid, urls)) = rx.try_recv() {
                self.pending.remove(&appid);
                self.urls.insert(appid, urls);
            }
        }
        self.pump(ctx);
    }

    fn get(&self, appid: u32) -> &[String] {
        self.urls.get(&appid).map_or(&[], Vec::as_slice)
    }
}

/// Adresses des captures déjà présentes dans la fiche en cache : aucune
/// requête réseau au premier survol d'un jeu déjà consulté.
fn cached_screenshot_urls(
    paths: &LapinuxPaths,
    appid: u32,
    store: Option<lapinux_core::stores::Store>,
) -> Option<Vec<String>> {
    let dir = if store.is_some() {
        "fiches-boutique"
    } else {
        "fiches"
    };
    let sheet = paths.cache_dir.join(dir).join(format!("{appid}.json"));
    let text = std::fs::read_to_string(sheet).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let list = ["/store/screenshots", "/screenshots", "/details/screenshots"]
        .into_iter()
        .find_map(|pointer| value.pointer(pointer))?
        .as_array()?;
    let urls: Vec<String> = list
        .iter()
        .filter_map(|shot| shot.as_str())
        .filter(|url| url.starts_with("http"))
        .take(HOVER_SHOT_LIMIT)
        .map(str::to_string)
        .collect();
    (!urls.is_empty()).then_some(urls)
}

/// Captures publiques d'un jeu Steam, demandées seulement quand la fiche n'est
/// pas déjà sur disque. La vignette 600 px suffit à l'aperçu et pèse bien
/// moins que l'image pleine résolution.
fn steam_screenshot_urls(appid: u32) -> Vec<String> {
    let url = format!(
        "https://store.steampowered.com/api/appdetails?appids={appid}&filters=screenshots&l=fr"
    );
    let Some(response) = ureq::get(&url)
        .set("User-Agent", "Lapinux")
        .timeout(Duration::from_secs(4))
        .call()
        .ok()
    else {
        return Vec::new();
    };
    let Ok(text) = response.into_string() else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    let Some(shots) = value
        .get(appid.to_string())
        .and_then(|game| game.get("data"))
        .and_then(|data| data.get("screenshots"))
        .and_then(|shots| shots.as_array())
    else {
        return Vec::new();
    };
    shots
        .iter()
        .filter_map(|shot| {
            shot.get("path_thumbnail")
                .or_else(|| shot.get("path_full"))
                .and_then(|url| url.as_str())
        })
        .filter(|url| url.starts_with("http"))
        .take(HOVER_SHOT_LIMIT)
        .map(str::to_string)
        .collect()
}

struct CoverCache {
    textures: HashMap<(u32, CoverTier), Option<egui::TextureHandle>>,
    budget: TextureBudget<(u32, CoverTier)>,
    requested: HashMap<(u32, CoverTier), Instant>,
    /// Jaquettes qu'aucune source n'a rendues : les redemander ne rapportait
    /// rien et bombardait le CDN de Steam.
    missing: HashSet<(u32, CoverTier)>,
    /// Taille des vignettes de grille, en pixels de l'écran : elle suit le
    /// réglage de largeur des cartes et l'échelle de l'affichage.
    tile: [u32; 2],
    /// Jaquettes trouvées dans le cache du client Steam (une seule recherche).
    steam_cache: HashMap<u32, Option<PathBuf>>,
    steamgriddb_key: String,
    steamgriddb: bool,
    /// Sources dans l'ordre de préférence choisi dans les réglages.
    sources: Vec<ArtSource>,
    /// Jaquettes de boutique à télécharger d'avance : la lecture de la
    /// bibliothèque les remplit une fois, et trois fils les écoulent.
    prefetch: std::collections::VecDeque<(u32, String, lapinux_core::stores::Store)>,
    prefetch_queued: HashSet<u32>,
    prefetch_active: usize,
    prefetch_rx: Option<mpsc::Receiver<(u32, Option<Vec<u8>>)>>,
    prefetch_tx: Option<mpsc::Sender<(u32, Option<Vec<u8>>)>>,
    /// Nombre de jeux de boutique déjà pris en compte par la file : les
    /// bibliothèques arrivent boutique par boutique, et une file remplie une
    /// seule fois laissait les jeux d'une boutique plus lente sans jaquette.
    prefetch_seen: usize,
    /// Décodages de jaquettes encore autorisés pour l'image en cours.
    decode_budget: usize,
    /// Pendant le défilement, un budget nul est intentionnel : il ne doit pas
    /// provoquer de rendus supplémentaires jusqu'à l'arrêt du mouvement.
    decode_enabled: bool,
    /// Nouvelles demandes réseau encore autorisées pour l'image en cours.
    /// Séparé du décodage : les lectures locales peuvent épuiser leur budget
    /// sans empêcher les jaquettes manquantes d'être demandées.
    request_budget: usize,
    /// Jaquettes animées visibles : leurs frames sont jouées à chaque image.
    gifs: HashMap<(u32, CoverTier), GifPlayer>,
    /// Couleurs dominantes des jaquettes, pour le halo de l'accueil NX.
    tones: HashMap<u32, (egui::Color32, egui::Color32)>,
}

impl CoverCache {
    fn new(steamgriddb_key: &str, steamgriddb: bool) -> Self {
        Self {
            textures: HashMap::new(),
            budget: TextureBudget::new(COVER_BUDGET),
            requested: HashMap::new(),
            missing: HashSet::new(),
            tile: [256, 384],
            steam_cache: HashMap::new(),
            steamgriddb_key: steamgriddb_key.trim().to_string(),
            steamgriddb,
            sources: vec![ArtSource::Steam, ArtSource::Steamgriddb],
            prefetch: std::collections::VecDeque::new(),
            prefetch_queued: HashSet::new(),
            prefetch_active: 0,
            prefetch_rx: None,
            prefetch_tx: None,
            prefetch_seen: 0,
            decode_budget: 0,
            decode_enabled: false,
            request_budget: 0,
            gifs: HashMap::new(),
            tones: HashMap::new(),
        }
    }

    fn set_config(&mut self, steamgriddb_key: &str, steamgriddb: bool, sources: &[ArtSource]) {
        let key = steamgriddb_key.trim().to_string();
        if key != self.steamgriddb_key
            || steamgriddb != self.steamgriddb
            || sources != self.sources.as_slice()
        {
            self.steamgriddb_key = key;
            self.steamgriddb = steamgriddb;
            self.sources = sources.to_vec();
            self.textures.clear();
            self.budget = TextureBudget::new(COVER_BUDGET);
            self.requested.clear();
            self.missing.clear();
            self.tones.clear();
        }
    }

    /// Taille des vignettes : la carte de la bibliothèque a `width` points de
    /// large et une fois et demie de haut.
    fn set_tile(&mut self, width: f32, scale: f32) {
        let width = (width * scale).ceil().clamp(96.0, COVER_MAX.0 as f32) as u32;
        let height = ((width as f32 * 1.5).ceil() as u32).clamp(144, COVER_MAX.1);
        self.tile = [width, height];
    }

    /// Taille maximale d'une texture, selon l'usage demandé.
    fn limit(&self, tier: CoverTier) -> [u32; 2] {
        match tier {
            CoverTier::Tile => self.tile,
            CoverTier::Sheet => [COVER_MAX.0, COVER_MAX.1],
        }
    }

    /// Oublie toutes les jaquettes en mémoire : vider les caches se voit tout
    /// de suite, sans relancer l'interface.
    fn clear(&mut self) {
        self.textures.clear();
        self.budget = TextureBudget::new(COVER_BUDGET);
        self.requested.clear();
        self.missing.clear();
        self.steam_cache.clear();
        self.gifs.clear();
        self.tones.clear();
        self.prefetch.clear();
        self.prefetch_queued.clear();
        self.prefetch_seen = 0;
    }

    /// Oublie la texture en mémoire pour recharger la jaquette au prochain affichage.
    fn forget(&mut self, appid: u32) {
        for tier in [CoverTier::Tile, CoverTier::Sheet] {
            self.textures.remove(&(appid, tier));
            self.budget.forget(&(appid, tier));
            self.requested.remove(&(appid, tier));
            self.missing.remove(&(appid, tier));
            self.gifs.remove(&(appid, tier));
        }
        self.tones.remove(&appid);
        self.steam_cache.remove(&appid);
    }

    /// Poids et nombre des jaquettes décodées (diagnostic).
    pub(crate) fn bytes(&self) -> usize {
        self.budget.bytes()
    }

    /// Nombre de jaquettes gardées en mémoire (diagnostic).
    pub(crate) fn count(&self) -> usize {
        self.textures.len()
    }

    /// Couleurs dominantes d'une jaquette : le halo du fond et le panneau du
    /// dock prennent la teinte du jeu sélectionné. La lecture n'a lieu qu'une
    /// fois par jeu ; tant que la jaquette manque, une teinte stable par
    /// appid évite un fond qui clignote.
    fn tone(
        &mut self,
        paths: &LapinuxPaths,
        appid: u32,
        store: Option<lapinux_core::stores::Store>,
    ) -> (egui::Color32, egui::Color32) {
        if let Some(tone) = self.tones.get(&appid) {
            return *tone;
        }
        let dir = paths.covers_dir();
        let mut candidates = Vec::new();
        let custom = custom_cover_path(paths, appid);
        if custom.is_file() {
            candidates.push(custom);
        }
        if store.is_some() {
            candidates.push(dir.join(format!("store-{appid}.jpg")));
        } else {
            candidates.push(dir.join(format!("{appid}.jpg")));
            candidates.push(dir.join(format!("sgdb-{appid}.jpg")));
            if let Some(local) = steam_cached_cover(appid) {
                candidates.push(local);
            }
        }
        if let Some(tone) = candidates.iter().find_map(|path| cover_tone(path)) {
            self.tones.insert(appid, tone);
            return tone;
        }
        fallback_cover_tone(appid)
    }

    /// Jaquette déjà décodée, si le budget la garde encore.
    fn cached(&mut self, appid: u32, tier: CoverTier) -> Option<Option<egui::TextureHandle>> {
        let found = self.textures.get(&(appid, tier)).cloned();
        if found
            .as_ref()
            .and_then(|texture| texture.as_ref())
            .is_some()
        {
            self.budget.touch(&(appid, tier));
        }
        found
    }

    /// Range une jaquette décodée et libère les plus anciennes si le budget
    /// est dépassé.
    fn remember(&mut self, appid: u32, tier: CoverTier, texture: &egui::TextureHandle) {
        self.textures.insert((appid, tier), Some(texture.clone()));
        self.budget.note((appid, tier), texture);
        self.trim();
    }

    /// Libère les jaquettes sorties de l'écran depuis un moment. Appelé à
    /// chaque image de la bibliothèque : sans cet appel, le cache ne se vidait
    /// qu'à l'arrivée d'une nouvelle jaquette, et celles d'un défilement
    /// restaient en mémoire indéfiniment.
    fn trim(&mut self) {
        for key in self.budget.trim() {
            self.textures.remove(&key);
            self.gifs.remove(&key);
        }
    }

    /// Fait avancer les jaquettes animées et redemande une image à la frame
    /// suivante.
    fn tick(&mut self, ctx: &egui::Context) {
        for player in self.gifs.values_mut() {
            player.tick(ctx);
        }
    }

    /// Nombre de jaquettes animées suivies (diagnostic).
    #[allow(dead_code)]
    pub(crate) fn animated(&self) -> usize {
        self.gifs.len()
    }

    /// Jaquette choisie à la main. Un GIF garde ses frames : la première sert
    /// de texture, les suivantes la remplacent.
    fn load_custom(
        &mut self,
        ctx: &egui::Context,
        appid: u32,
        tier: CoverTier,
        path: &Path,
        name: &str,
        limit: [u32; 2],
    ) -> Option<egui::TextureHandle> {
        let bytes = std::fs::read(path).ok()?;
        if bytes.starts_with(b"GIF8") {
            if let Some(player) =
                GifPlayer::load(ctx, &bytes, name, [limit[0].min(320), limit[1].min(480)])
            {
                let texture = player.texture.clone();
                self.gifs.insert((appid, tier), player);
                self.remember(appid, tier, &texture);
                return Some(texture);
            }
        }
        let image = image::load_from_memory(&bytes).ok()?;
        let texture = cover_texture(ctx, image, name, limit);
        self.remember(appid, tier, &texture);
        Some(texture)
    }

    /// Autorise quelques décodages pour l'image en cours. Décoder d'un coup
    /// toutes les jaquettes visibles coûtait près de sept cents millisecondes
    /// au premier affichage ; réparties sur les images suivantes, elles ne se
    /// voient plus.
    fn begin_frame(&mut self, budget: usize) {
        self.decode_budget = budget;
        self.decode_enabled = budget > 0;
        // Trois demandes réseau maximum par image : assez pour remplir une
        // grille au démarrage, sans lancer une requête par tuile traversée.
        self.request_budget = budget.min(3);
    }

    fn take_decode(&mut self) -> bool {
        if self.decode_budget == 0 {
            return false;
        }
        self.decode_budget -= 1;
        true
    }

    fn take_decode_or_wake(&mut self, ctx: &egui::Context) -> bool {
        if self.take_decode() {
            return true;
        }
        if self.decode_enabled {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        false
    }

    fn take_request(&mut self) -> bool {
        if self.request_budget == 0 {
            return false;
        }
        self.request_budget -= 1;
        true
    }

    /// Relève le plafond de demandes pour un préchargement ponctuel. Le budget
    /// normal revient à l'image suivante : seules les premières rangées en
    /// profitent, pas la file entière.
    fn boost_requests(&mut self, budget: usize) {
        self.request_budget = self.request_budget.max(budget);
    }

    fn get(
        &mut self,
        ctx: &egui::Context,
        paths: &LapinuxPaths,
        appid: u32,
        name: &str,
        store: Option<lapinux_core::stores::Store>,
        remote: &str,
        tier: CoverTier,
    ) -> Option<egui::TextureHandle> {
        self.tick(ctx);
        // Un jeu de boutique n'a rien à voir avec le CDN Steam : sa jaquette
        // est celle que la boutique publie, téléchargée une fois.
        if let Some(store) = store {
            return self.store_cover(ctx, paths, appid, remote, store, tier);
        }
        if let Some(entry) = self.cached(appid, tier) {
            return entry;
        }
        let dir = paths.covers_dir();
        let limit = self.limit(tier);
        let texture_name = format!("cover-{}-{}", tier.suffix(), appid);
        // Une jaquette choisie à la main prime toujours sur la sélection
        // automatique, quel que soit l'ordre des sources.
        let custom = custom_cover_path(paths, appid);
        if custom.is_file() {
            if !self.take_decode_or_wake(ctx) {
                return None;
            }
            if let Some(texture) = self.load_custom(ctx, appid, tier, &custom, &texture_name, limit)
            {
                return Some(texture);
            }
        }
        let sgdb_ready =
            self.steamgriddb && !self.steamgriddb_key.is_empty() && !name.trim().is_empty();
        let ordered: Vec<ArtSource> = self
            .sources
            .iter()
            .copied()
            .filter(|source| *source == ArtSource::Steam || sgdb_ready)
            .collect();
        // 1. Les fichiers déjà téléchargés, dans l'ordre choisi.
        for source in &ordered {
            let path = match source {
                ArtSource::Steam => dir.join(format!("{appid}.jpg")),
                ArtSource::Steamgriddb => dir.join(format!("sgdb-{appid}.jpg")),
            };
            if path.is_file() {
                if !self.take_decode_or_wake(ctx) {
                    return None;
                }
                if let Some(texture) = load_cached_cover(ctx, &path, &texture_name, limit) {
                    self.remember(appid, tier, &texture);
                    return Some(texture);
                }
            }
        }
        // 2. Le cache du client Steam appartient à la source Steam : il ne
        //    compte tout de suite que si Steam est en tête. Sinon, la source
        //    prioritaire doit d'abord avoir sa chance (téléchargement lancé
        //    ci-dessous), et ce cache ne sert qu'après son échec.
        if ordered.first() == Some(&ArtSource::Steam) {
            if let Some(texture) = self.local_cover(ctx, appid, tier) {
                return Some(texture);
            }
        }
        let now = Instant::now();
        let key = (appid, tier);
        // Une source qui n'a pas rendu d'image ne change pas d'avis : passé le
        // délai laissé à la demande pour écrire son fichier, l'absence de
        // jaquette est définitive. La redemander toutes les 30 s ne ramenait
        // rien et faisait pleuvoir des 404 sur le CDN de Steam — de quoi faire
        // limiter l'adresse IP sur un historique d'achats entier.
        if let Some(last) = self.requested.get(&key).copied() {
            if now.duration_since(last) >= COVER_SETTLE {
                self.missing.insert(key);
            }
        }
        if self.missing.contains(&key) {
            // Le repli local ne consomme un décodage que s'il existe vraiment.
            return self.local_cover(ctx, appid, tier);
        }
        let should_request = self
            .requested
            .get(&key)
            .map(|t| now.duration_since(*t) > Duration::from_secs(30))
            .unwrap_or(true);
        // Une demande réseau a son propre créneau : les lectures locales qui
        // ont épuisé le budget de décodage ne doivent pas empêcher les
        // jaquettes manquantes d'être téléchargées. Pendant un défilement
        // rapide, le budget de requêtes vaut zéro, donc aucun orage réseau.
        if should_request && self.take_request() {
            self.requested.insert(key, now);
            std::fs::create_dir_all(&dir).ok();
            let ctx = ctx.clone();
            let key = self.steamgriddb_key.clone();
            let name = name.to_string();
            std::thread::spawn(move || {
                // La source préférée d'abord ; la suivante seulement si elle
                // ne rend rien (jeu absent, clé refusée, image 404…).
                for source in ordered {
                    let fetched = match source {
                        ArtSource::Steam => steam_cdn_cover(appid),
                        ArtSource::Steamgriddb => steamgriddb_cover(appid, &name, &key),
                    };
                    let Some(bytes) = fetched else { continue };
                    if bytes.len() <= 1024 {
                        continue;
                    }
                    let path = match source {
                        ArtSource::Steam => dir.join(format!("{appid}.jpg")),
                        ArtSource::Steamgriddb => dir.join(format!("sgdb-{appid}.jpg")),
                    };
                    let temporary = path.with_extension("tmp");
                    if std::fs::write(&temporary, &bytes).is_ok() {
                        let _ = std::fs::rename(&temporary, &path);
                    }
                    break;
                }
                std::thread::sleep(Duration::from_millis(120));
                ctx.request_repaint();
            });
            return None;
        }
        if should_request && self.decode_enabled {
            // La demande attend seulement une place dans le budget réseau.
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        // 3. Toutes les sources ont eu leur chance : repli sur le client Steam.
        self.local_cover(ctx, appid, tier)
    }

    /// Jaquette publiée par une boutique : gardée sur disque comme les autres,
    /// elle est demandée en tâche de fond au premier affichage.
    fn store_cover(
        &mut self,
        ctx: &egui::Context,
        paths: &LapinuxPaths,
        appid: u32,
        remote: &str,
        store: lapinux_core::stores::Store,
        tier: CoverTier,
    ) -> Option<egui::TextureHandle> {
        self.tick(ctx);
        if let Some(entry) = self.cached(appid, tier) {
            return entry;
        }
        let dir = paths.covers_dir();
        let limit = self.limit(tier);
        let texture_name = format!("cover-{}-{}", tier.suffix(), appid);
        let custom = custom_cover_path(paths, appid);
        if custom.is_file() {
            if !self.take_decode_or_wake(ctx) {
                return None;
            }
            if let Some(texture) = self.load_custom(ctx, appid, tier, &custom, &texture_name, limit)
            {
                return Some(texture);
            }
        }
        let path = dir.join(format!("store-{appid}.jpg"));
        if path.is_file() {
            if !self.take_decode_or_wake(ctx) {
                return None;
            }
            if let Some(texture) = load_cached_cover(ctx, &path, &texture_name, limit) {
                self.remember(appid, tier, &texture);
                return Some(texture);
            }
        }
        if remote.trim().is_empty() {
            return None;
        }
        let now = Instant::now();
        let should_request = self
            .requested
            .get(&(appid, tier))
            .map(|last| now.duration_since(*last) > Duration::from_secs(30))
            .unwrap_or(true);
        if !should_request {
            return None;
        }
        self.requested.insert((appid, tier), now);
        std::fs::create_dir_all(&dir).ok();
        let url = cover_request(remote, store);
        let ctx = ctx.clone();
        let path = path.clone();
        std::thread::spawn(move || {
            if let Some(bytes) = crate::achievements::http_bytes(&url) {
                if let Some(bytes) = portrait_store_cover(&bytes) {
                    let temporary = path.with_extension("tmp");
                    if std::fs::write(&temporary, &bytes).is_ok() {
                        let _ = std::fs::rename(&temporary, &path);
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(120));
            ctx.request_repaint();
        });
        None
    }

    /// Vrai quand la liste des jeux de boutique a changé depuis le dernier
    /// remplissage : la page Bibliothèque en profite pour ne parcourir la
    /// liste que lorsque c'est utile.
    fn needs_prefetch(&self, total: usize) -> bool {
        total != self.prefetch_seen
    }

    /// Remplit la file des jaquettes de boutique : tout ce qui n'est pas déjà
    /// sur disque part en tâche de fond, pour qu'aucune tuile ne reste en
    /// lettre le temps de son téléchargement au premier défilement.
    fn prefetch_store(&mut self, paths: &LapinuxPaths, games: &[lapinux_core::stores::StoreGame]) {
        if games.is_empty() || games.len() == self.prefetch_seen {
            return;
        }
        self.prefetch_seen = games.len();
        let dir = paths.covers_dir();
        std::fs::create_dir_all(&dir).ok();
        for game in games {
            if game.store == lapinux_core::stores::Store::Steam || game.cover.trim().is_empty() {
                continue;
            }
            if !self.prefetch_queued.insert(game.appid) {
                continue;
            }
            if dir.join(format!("store-{}.jpg", game.appid)).is_file() {
                continue;
            }
            self.prefetch
                .push_back((game.appid, game.cover.clone(), game.store));
        }
    }

    /// Remonte les jeux des premières rangées dans la file des jaquettes de
    /// boutique. Les téléchargements déjà partis ne sont pas interrompus ; les
    /// suivants commencent par les tuiles du haut.
    fn prioritize_prefetch(&mut self, appids: &[u32]) {
        if appids.is_empty() || self.prefetch.is_empty() {
            return;
        }
        let first: HashSet<u32> = appids.iter().copied().collect();
        let mut front = std::collections::VecDeque::new();
        let mut rest = std::collections::VecDeque::new();
        while let Some(item) = self.prefetch.pop_front() {
            if first.contains(&item.0) {
                front.push_back(item);
            } else {
                rest.push_back(item);
            }
        }
        front.append(&mut rest);
        self.prefetch = front;
    }

    /// Démarre jusqu'à trois téléchargements à la fois et relève ceux qui
    /// finissent. Appelée à chaque image : la file avance toute seule, même
    /// quand la tuile n'est pas visible.
    fn pump_prefetch(&mut self, ctx: &egui::Context, paths: &LapinuxPaths) {
        if self.prefetch_rx.is_none() {
            let (tx, rx) = mpsc::channel();
            self.prefetch_tx = Some(tx);
            self.prefetch_rx = Some(rx);
        }
        if let Some(rx) = &self.prefetch_rx {
            while let Ok((appid, bytes)) = rx.try_recv() {
                self.prefetch_active = self.prefetch_active.saturating_sub(1);
                let Some(bytes) = bytes else {
                    continue;
                };
                let path = paths.covers_dir().join(format!("store-{appid}.jpg"));
                let temporary = path.with_extension("tmp");
                if std::fs::write(&temporary, &bytes).is_ok() {
                    let _ = std::fs::rename(&temporary, &path);
                    // La tuile a déjà été peinte en lettre : sans cette
                    // demande, elle garderait la lettre jusqu'au prochain
                    // mouvement de la souris.
                    for tier in [CoverTier::Tile, CoverTier::Sheet] {
                        self.textures.remove(&(appid, tier));
                        self.budget.forget(&(appid, tier));
                    }
                    self.tones.remove(&appid);
                    ctx.request_repaint();
                }
            }
        }
        while self.prefetch_active < 3 {
            let Some((appid, remote, store)) = self.prefetch.pop_front() else {
                break;
            };
            let Some(tx) = self.prefetch_tx.clone() else {
                break;
            };
            let url = cover_request(&remote, store);
            self.prefetch_active += 1;
            std::thread::spawn(move || {
                // Le décodage se fait hors de l'interface : une image cassée
                // n'écrit rien, et la fiche garde sa place. Le visuel d'itch.io
                // est un bandeau : il est mis au format d'une pochette ici.
                let bytes = crate::achievements::http_bytes(&url)
                    .and_then(|bytes| portrait_store_cover(&bytes));
                let _ = tx.send((appid, bytes));
            });
        }
        if self.prefetch_active > 0 || !self.prefetch.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(400));
        }
    }

    /// Jaquette trouvée dans le cache du client Steam, chargée une fois.
    fn local_cover(
        &mut self,
        ctx: &egui::Context,
        appid: u32,
        tier: CoverTier,
    ) -> Option<egui::TextureHandle> {
        let local = if let Some(cached) = self.steam_cache.get(&appid) {
            cached.clone()
        } else {
            let found = steam_cached_cover(appid);
            self.steam_cache.insert(appid, found.clone());
            found
        }?;
        if !self.take_decode_or_wake(ctx) {
            return None;
        }
        let texture = load_cover_file(
            ctx,
            &local,
            &format!("cover-{}-{}", tier.suffix(), appid),
            self.limit(tier),
        )?;
        self.remember(appid, tier, &texture);
        Some(texture)
    }
}

/// Jaquette de boutique prête pour une tuile en 2:3.
///
/// Si seule une image paysage est publiée, la centrer sur un fond flouté
/// évite d'en rogner le contenu pour la tuile verticale.
fn portrait_store_cover(bytes: &[u8]) -> Option<Vec<u8>> {
    let image = image::load_from_memory(bytes).ok()?;
    if portrait_cover(image.width(), image.height()) {
        return Some(bytes.to_vec());
    }
    if image.width() == 0 || image.height() == 0 {
        return None;
    }
    let image = if image.width() > 600 {
        image.thumbnail(600, 600)
    } else {
        image
    };
    let width = image.width();
    let height = ((width as f32) * 1.5).round().max(1.0) as u32;
    // Fond : le même visuel agrandi pour couvrir la tuile, puis flouté et
    // assombri. Sans lui, les deux bandes vides au-dessus et au-dessous d'un
    // bandeau 4:3 dans une tuile 2:3 ressemblaient à un cadre cassé.
    let backdrop = image
        .resize_to_fill(width, height, image::imageops::FilterType::Triangle)
        .to_rgba8();
    let backdrop = image::imageops::blur(&backdrop, 14.0);
    let mut canvas = image::RgbaImage::from_pixel(
        width,
        height,
        image::Rgba([CARD().r(), CARD().g(), CARD().b(), 255]),
    );
    for (x, y, pixel) in backdrop.enumerate_pixels() {
        let dim = |channel: u8| ((u16::from(channel) * 110) / 255) as u8;
        canvas.put_pixel(
            x,
            y,
            image::Rgba([dim(pixel[0]), dim(pixel[1]), dim(pixel[2]), 255]),
        );
    }
    // Le visuel net par-dessus, à sa taille publiée, centré.
    let top = height.saturating_sub(image.height()) / 2;
    image::imageops::overlay(&mut canvas, &image.to_rgba8(), 0, i64::from(top));
    let mut out = Vec::new();
    image::DynamicImage::ImageRgba8(canvas)
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .ok()?;
    Some(out)
}

/// Adresse de la jaquette à demander.
///
/// Les vignettes d'Epic pèsent plusieurs mégaoctets en pleine résolution, pour
/// une tuile large de 170 px : le CDN de la boutique sait les redimensionner,
/// et chaque jaquette passe alors de 1,7 Mo à 40 Ko — sans quoi une rangée de
/// tuiles saturait la connexion et les téléchargements expiraient. Une adresse
/// qui contient une espace est encodée : le client HTTP la refuserait.
fn cover_request(remote: &str, store: lapinux_core::stores::Store) -> String {
    let mut url = remote.trim().replace(' ', "%20");
    // L'API GOG rend ses visuels sans schéma (`//images-…`) : le client HTTP
    // les refuserait, et la jaquette resterait une lettre.
    if url.starts_with("//") {
        url.insert_str(0, "https:");
    }
    if store == lapinux_core::stores::Store::Epic
        && url.contains("epicgames.com")
        && !url.contains('?')
    {
        return format!("{url}?resize=1&w=600");
    }
    url
}

/// Fichiers d'illustrations rangés par le client Steam lui-même dans
/// `appcache/librarycache/<appid>` (parfois sous un dossier de hachage).
/// Les jeux non publiés ou retirés du magasin n'ont rien sur le CDN public,
/// mais le client a déjà ces images : c'est la seule source officielle qui
/// reste pour eux, sans service tiers.
fn steam_librarycache_files(appid: u32) -> Vec<PathBuf> {
    for root in lapinux_core::paths::steam_roots() {
        let dir = root
            .join("appcache")
            .join("librarycache")
            .join(appid.to_string());
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut files: Vec<PathBuf> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                files.push(path);
            } else if path.is_dir() {
                if let Ok(children) = std::fs::read_dir(&path) {
                    files.extend(
                        children
                            .flatten()
                            .map(|child| child.path())
                            .filter(|path| path.is_file()),
                    );
                }
            }
        }
        if !files.is_empty() {
            return files;
        }
    }
    Vec::new()
}

fn file_named<'a>(files: &'a [PathBuf], name: &str) -> Option<&'a PathBuf> {
    files.iter().find(|path| {
        path.file_name()
            .and_then(|candidate| candidate.to_str())
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(name))
    })
}

/// Une icône de bibliothèque porte le nom de son hachage (40 caractères).
fn is_icon_file(path: &Path) -> bool {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
        return false;
    };
    matches!(extension.as_str(), "jpg" | "png")
        && stem.len() >= 32
        && stem.chars().all(|character| character.is_ascii_hexdigit())
}

/// Portrait de jaquette déjà présent dans le cache du client Steam.
fn steam_cached_cover(appid: u32) -> Option<PathBuf> {
    let files = steam_librarycache_files(appid);
    ["library_600x900.jpg", "library_capsule.jpg"]
        .iter()
        .find_map(|name| file_named(&files, name).cloned())
}

/// Icône carrée déjà rangée par le client Steam. Elle sert aux captures :
/// celles-ci n'ont pas toujours l'appid dans leur nom et leur jeu n'est pas
/// forcément encore installé.
pub(crate) fn steam_cached_icon(appid: u32) -> Option<PathBuf> {
    let files = steam_librarycache_files(appid);
    file_named(&files, "icon.jpg")
        .cloned()
        .or_else(|| files.into_iter().find(|path| is_icon_file(path)))
}

/// Illustrations d'une fiche : le CDN de Valve les sert à partir de l'appid
/// seul ; le cache local du client Steam et SteamGridDB complètent les
/// formats absents. Le CDN Steam ne publie jamais la jaquette portrait de
/// `Cover`, qui vient du cache local ou de SteamGridDB.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum SteamArt {
    /// Bandeau large : hero de bibliothèque, repli sur la bannière. Plus
    /// affiché depuis que les images vivent dans la colonne média, mais le
    /// cache le connaît encore et la galerie peut le proposer.
    Banner,
    Capsule,
    /// Capsule large de la fiche, servie en 616×353 pour l'aperçu de survol.
    Wide,
    Logo,
    /// Jaquette portrait communautaire, utilisée par la galerie quand la
    /// boutique ne publie pas de jaquette exploitable.
    Cover,
    Icon,
}

impl SteamArt {
    fn file(self) -> &'static str {
        match self {
            SteamArt::Banner => "hero",
            SteamArt::Capsule => "capsule",
            SteamArt::Wide => "wide",
            SteamArt::Logo => "logo",
            SteamArt::Cover => "cover",
            SteamArt::Icon => "icon",
        }
    }

    fn urls(self, details: &SteamDetails) -> Vec<String> {
        let mut urls: Vec<String> = match self {
            SteamArt::Banner => vec![details.hero_image.clone(), details.header_image.clone()],
            SteamArt::Capsule => vec![details.capsule_image.clone(), details.header_image.clone()],
            SteamArt::Wide => {
                // La capsule de la fiche est publiée en 231×87 : trop petite
                // pour un aperçu de 420 points. La même image existe en
                // 616×353 et garde le cadrage rectangulaire demandé.
                let large = details
                    .capsule_image
                    .replace("capsule_231x87", "capsule_616x353");
                vec![
                    large,
                    details.header_image.clone(),
                    details.capsule_image.clone(),
                ]
            }
            SteamArt::Logo => vec![details.logo_image.clone()],
            // Une jaquette portrait ne vient jamais du CDN Steam dans ce
            // cache : elle est lue dans le cache du client ou sur SteamGridDB.
            SteamArt::Cover => Vec::new(),
            SteamArt::Icon => {
                // Une icône de jeu ne se remplace ni par un logo ni par une
                // capsule : l'icône communautaire est demandée par
                // `game_icon` quand Steam n'en publie pas, et l'appelant garde
                // sinon son propre repli.
                vec![details.icon_image.clone()]
            }
        };
        urls.retain(|url| !url.trim().is_empty());
        urls.dedup();
        urls
    }

    /// Chemin dans le cache du client Steam, s'il contient cette illustration.
    fn cached(self, appid: u32, details: &SteamDetails) -> Option<PathBuf> {
        let files = steam_librarycache_files(appid);
        if files.is_empty() {
            return None;
        }
        match self {
            SteamArt::Banner => file_named(&files, "library_hero.jpg")
                .or_else(|| file_named(&files, "library_header.jpg"))
                .cloned(),
            SteamArt::Capsule => file_named(&files, "library_header.jpg")
                .or_else(|| file_named(&files, "library_capsule.jpg"))
                .cloned(),
            SteamArt::Wide => file_named(&files, "library_header.jpg").cloned(),
            SteamArt::Logo => file_named(&files, "logo.png").cloned(),
            SteamArt::Cover => steam_cached_cover(appid),
            SteamArt::Icon => {
                // L'appid PICS peut être en retard sur le client : on cherche
                // d'abord son hachage, puis n'importe quelle icône en cache.
                let hint = details.icon_image.rsplit('/').next().unwrap_or_default();
                file_named(&files, hint)
                    .cloned()
                    .or_else(|| file_named(&files, "icon.jpg").cloned())
                    .or_else(|| files.into_iter().find(|path| is_icon_file(path)))
            }
        }
    }
}

/// Une seule migration : les versions précédentes pouvaient enregistrer le
/// logo d'un jeu à la place de son icône. Elles sont supprimées, puis les
/// vraies icônes sont relues ou retéléchargées ; SteamGridDB reste intact.
fn purge_legacy_icons(art_dir: &Path) {
    std::fs::create_dir_all(art_dir).ok();
    let marker = art_dir.join(".icons-are-game-icons-v1");
    if marker.is_file() {
        return;
    }
    if let Ok(entries) = std::fs::read_dir(art_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let legacy = name.starts_with("steam-icon-") || name.starts_with("icon-");
            let image = matches!(
                path.extension().and_then(|extension| extension.to_str()),
                Some("jpg" | "png")
            );
            if legacy && image {
                let _ = std::fs::remove_file(path);
            }
        }
    }
    let _ = std::fs::write(marker, b"1");
}

/// Textures des illustrations de fiches Steam, chargées en arrière-plan et
/// gardées sur disque : la fiche s'affiche tout de suite, l'image arrive après.
struct ArtCache {
    textures: HashMap<(u32, SteamArt), Option<egui::TextureHandle>>,
    budget: TextureBudget<(u32, SteamArt)>,
    requested: HashMap<(u32, SteamArt), Instant>,
    /// Chemin trouvé dans le cache du client Steam (`None` = cherché, absent).
    local: HashMap<(u32, SteamArt), Option<PathBuf>>,
    /// Point d'intérêt des capsules larges, calculé une fois au chargement.
    focus: HashMap<(u32, SteamArt), Option<egui::Vec2>>,
    steamgriddb_key: String,
    steamgriddb: bool,
    /// Même ordre de préférence que les jaquettes, choisi dans les réglages.
    sources: Vec<ArtSource>,
}

impl ArtCache {
    fn new(art_dir: &Path) -> Self {
        purge_legacy_icons(art_dir);
        Self {
            textures: HashMap::new(),
            budget: TextureBudget::new(ART_BUDGET),
            requested: HashMap::new(),
            local: HashMap::new(),
            focus: HashMap::new(),
            steamgriddb_key: String::new(),
            steamgriddb: false,
            sources: vec![ArtSource::Steam, ArtSource::Steamgriddb],
        }
    }

    /// Changer l'ordre des sources se voit tout de suite : les textures en
    /// mémoire sont relues, les fichiers déjà téléchargés restent sur disque.
    fn set_config(&mut self, steamgriddb_key: &str, steamgriddb: bool, sources: &[ArtSource]) {
        let key = steamgriddb_key.trim().to_string();
        if key != self.steamgriddb_key
            || steamgriddb != self.steamgriddb
            || sources != self.sources.as_slice()
        {
            self.steamgriddb_key = key;
            self.steamgriddb = steamgriddb;
            self.sources = sources.to_vec();
            self.textures.clear();
            self.budget = TextureBudget::new(ART_BUDGET);
            self.requested.clear();
            self.focus.clear();
        }
    }

    /// Sources utilisables, dans l'ordre choisi dans les réglages.
    fn ordered(&self, name: &str) -> Vec<ArtSource> {
        let sgdb_ready =
            self.steamgriddb && !self.steamgriddb_key.is_empty() && !name.trim().is_empty();
        self.sources
            .iter()
            .copied()
            .filter(|source| *source == ArtSource::Steam || sgdb_ready)
            .collect()
    }

    fn local(&mut self, appid: u32, details: &SteamDetails, art: SteamArt) -> Option<PathBuf> {
        let key = (appid, art);
        if let Some(path) = self.local.get(&key) {
            return path.clone();
        }
        let path = art.cached(appid, details);
        self.local.insert(key, path.clone());
        path
    }

    /// Retient le point d'intérêt d'une capsule large, calculé une seule fois
    /// à partir du fichier chargé : le recadrage du survol garde alors le logo
    /// et les personnages au lieu de couper au milieu de l'image. Une image
    /// sans contours nets reste au centre, et n'est pas réanalysée.
    fn note_focus(&mut self, key: (u32, SteamArt), path: &Path) {
        if key.1 != SteamArt::Wide || self.focus.contains_key(&key) {
            return;
        }
        self.focus.insert(key, image_focus(path));
    }

    /// Ancre de recadrage connue ; le centre par défaut reproduit l'ancien
    /// cadrage pour les images sans point d'intérêt.
    fn focus_of(&self, appid: u32, art: SteamArt) -> egui::Vec2 {
        self.focus
            .get(&(appid, art))
            .copied()
            .flatten()
            .unwrap_or(egui::vec2(0.5, 0.5))
    }

    fn get(
        &mut self,
        ctx: &egui::Context,
        paths: &LapinuxPaths,
        appid: u32,
        details: &SteamDetails,
        art: SteamArt,
        name: &str,
    ) -> Option<egui::TextureHandle> {
        let key = (appid, art);
        // Une fiche refermée depuis longtemps ne garde pas ses illustrations.
        self.trim();
        if let Some(found) = self.textures.get(&key) {
            if found.is_some() {
                self.budget.touch(&key);
            }
            return found.clone();
        }
        let dir = paths.art_dir();
        let texture_name = format!("steam-{}-{appid}", art.file());
        let ordered = self.ordered(name);
        // Premier tour : les fichiers déjà en cache, dans l'ordre demandé. Un
        // fichier à l'ancien nommage vient du CDN Steam.
        for source in &ordered {
            let path = art_source_path(&dir, appid, art, *source);
            if let Some(texture) = load_texture_file(ctx, &path, &texture_name) {
                self.note_focus(key, &path);
                self.remember(key, &texture);
                return Some(texture);
            }
            if *source == ArtSource::Steam {
                let legacy = dir.join(format!("{}-{appid}.jpg", art.file()));
                if let Some(texture) = load_texture_file(ctx, &legacy, &texture_name) {
                    self.note_focus(key, &legacy);
                    self.remember(key, &texture);
                    return Some(texture);
                }
            }
        }
        // Le cache du client Steam appartient à la source Steam : il ne compte
        // tout de suite que si Steam est en tête. Sinon, une source plus
        // prioritaire doit d'abord avoir sa chance.
        if ordered.first() == Some(&ArtSource::Steam) {
            if let Some(texture) = self.local_texture(ctx, appid, details, art) {
                return Some(texture);
            }
        }
        let now = Instant::now();
        let should_request = self
            .requested
            .get(&key)
            .map(|time| now.duration_since(*time) > Duration::from_secs(30))
            .unwrap_or(true);
        if should_request && !ordered.is_empty() {
            self.requested.insert(key, now);
            let urls = art.urls(details);
            let sgdb_key = self.steamgriddb_key.clone();
            let name = name.to_string();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                for source in ordered {
                    let fetched = match source {
                        ArtSource::Steam => urls.iter().find_map(|url| {
                            download_image(url)
                                .filter(|bytes| image::load_from_memory(bytes).is_ok())
                        }),
                        ArtSource::Steamgriddb => sgdb_art(appid, &name, &sgdb_key, art),
                    };
                    let Some(bytes) = fetched else { continue };
                    if image::load_from_memory(&bytes).is_err() {
                        continue;
                    }
                    let path = art_source_path(&dir, appid, art, source);
                    std::fs::create_dir_all(&dir).ok();
                    // Écriture à côté puis renommage : l'interface ne doit
                    // jamais décoder un fichier encore à moitié écrit.
                    let temporary = path.with_extension("tmp");
                    if std::fs::write(&temporary, &bytes).is_ok() {
                        let _ = std::fs::rename(&temporary, &path);
                    }
                    break;
                }
                ctx.request_repaint();
            });
            return None;
        }
        // Toutes les sources ont eu leur chance : repli sur le client Steam.
        self.local_texture(ctx, appid, details, art)
    }

    /// Illustration trouvée dans le cache du client Steam, chargée une fois.
    fn local_texture(
        &mut self,
        ctx: &egui::Context,
        appid: u32,
        details: &SteamDetails,
        art: SteamArt,
    ) -> Option<egui::TextureHandle> {
        let local = self.local(appid, details, art)?;
        let texture = load_texture_file(ctx, &local, &format!("steam-{}-{appid}", art.file()))?;
        self.note_focus((appid, art), &local);
        self.remember((appid, art), &texture);
        Some(texture)
    }

    /// Icône carrée du cache du client Steam, pour les notifications
    /// d'activité. Une icône absente ne retombe pas sur la jaquette :
    /// l'appelant garde alors son glyphe générique.
    fn icon_local(
        &mut self,
        ctx: &egui::Context,
        paths: &LapinuxPaths,
        appid: u32,
    ) -> Option<egui::TextureHandle> {
        let key = (appid, SteamArt::Icon);
        if let Some(found) = self.textures.get(&key) {
            if found.is_some() {
                self.budget.touch(&key);
            }
            return found.clone();
        }
        // Une icône de fiche téléchargée à la demande (jeu d'un ami absent du
        // cache Steam) est rangée comme les autres illustrations. La même
        // icône SGDB doit servir partout : accueil, amis, wishlist et
        // notifications, pas seulement dans la galerie de la fiche.
        let cached = [ArtSource::Steamgriddb, ArtSource::Steam]
            .into_iter()
            .map(|source| art_source_path(&paths.art_dir(), appid, SteamArt::Icon, source))
            .find(|path| path.is_file());
        if let Some(cached) = cached {
            if let Some(texture) = load_texture_file(ctx, &cached, &format!("game-icon-{appid}")) {
                self.remember(key, &texture);
                return Some(texture);
            }
        }
        let path = if let Some(path) = self.local.get(&key) {
            path.clone()
        } else {
            let path = steam_cached_icon(appid);
            self.local.insert(key, path.clone());
            path
        };
        let Some(path) = path else {
            // Ne pas mémoriser l'absence : une icône distante peut arriver
            // pendant que le panneau reste ouvert.
            return None;
        };
        let Some(texture) = load_texture_file(ctx, &path, &format!("steam-icon-{appid}")) else {
            return None;
        };
        self.remember(key, &texture);
        Some(texture)
    }

    /// Icône déjà présente sur disque, sans interroger la boutique Steam.
    /// Les jeux Epic, GOG et itch.io n'ont pas d'appid Steam à envoyer au CDN.
    fn cached_icon(
        &mut self,
        ctx: &egui::Context,
        paths: &LapinuxPaths,
        appid: u32,
    ) -> Option<egui::TextureHandle> {
        if appid == 0 {
            return None;
        }
        self.icon_local(ctx, paths, appid)
    }

    /// Icône d'un jeu dont on n'a pas forcément la fiche : le cache Steam
    /// local d'abord, puis l'icône communautaire publiée par la page boutique.
    fn game_icon(
        &mut self,
        ctx: &egui::Context,
        paths: &LapinuxPaths,
        appid: u32,
    ) -> Option<egui::TextureHandle> {
        if appid == 0 {
            return None;
        }
        if let Some(icon) = self.icon_local(ctx, paths, appid) {
            return Some(icon);
        }
        let key = (appid, SteamArt::Icon);
        let now = Instant::now();
        let should_request = self
            .requested
            .get(&key)
            .map(|time| now.duration_since(*time) > Duration::from_secs(30))
            .unwrap_or(true);
        if should_request {
            self.requested.insert(key, now);
            let ctx = ctx.clone();
            let dir = paths.art_dir();
            std::thread::spawn(move || {
                let page = download_text(&format!(
                    "https://store.steampowered.com/app/{appid}/?l=french"
                ));
                let Some(icon_url) = page.as_deref().and_then(steam_store_icon_url) else {
                    ctx.request_repaint();
                    return;
                };
                let Some(bytes) = download_image(&icon_url) else {
                    ctx.request_repaint();
                    return;
                };
                if image::load_from_memory(&bytes).is_err() {
                    ctx.request_repaint();
                    return;
                }
                let path = art_source_path(&dir, appid, SteamArt::Icon, ArtSource::Steam);
                std::fs::create_dir_all(&dir).ok();
                let temporary = path.with_extension("tmp");
                if std::fs::write(&temporary, &bytes).is_ok() {
                    let _ = std::fs::rename(&temporary, &path);
                }
                ctx.request_repaint();
            });
        }
        None
    }

    /// Poids des illustrations décodées (diagnostic).
    pub(crate) fn bytes(&self) -> usize {
        self.budget.bytes()
    }

    /// Vide le cache des illustrations : elles seront relues au prochain écran.
    fn clear(&mut self) {
        self.textures.clear();
        self.budget = TextureBudget::new(ART_BUDGET);
        self.requested.clear();
        self.local.clear();
        self.focus.clear();
    }

    /// Range une illustration décodée et libère les plus anciennes si le
    /// budget est dépassé.
    fn remember(&mut self, key: (u32, SteamArt), texture: &egui::TextureHandle) {
        self.textures.insert(key, Some(texture.clone()));
        self.budget.note(key, texture);
        self.trim();
    }

    /// Libère les illustrations d'une fiche depuis longtemps refermée.
    fn trim(&mut self) {
        for key in self.budget.trim() {
            self.textures.remove(&key);
        }
    }
}

/// Une illustration gardée sur disque est rangée par source : changer l'ordre
/// dans les réglages prend le fichier de la source préférée, sans retélécharger.
fn art_source_path(dir: &Path, appid: u32, art: SteamArt, source: ArtSource) -> PathBuf {
    match source {
        ArtSource::Steam => dir.join(format!("steam-{}-{appid}.jpg", art.file())),
        ArtSource::Steamgriddb => dir.join(format!("sgdb-{}-{appid}.jpg", art.file())),
    }
}

/// `/games/steam/:appid` répond parfois un objet, parfois un tableau.
fn first_game_id(value: &serde_json::Value) -> Option<u64> {
    let data = value.get("data")?;
    if let Some(array) = data.as_array() {
        array.first()?.get("id")?.as_u64()
    } else {
        data.get("id")?.as_u64()
    }
}

/// Portraits publiés par Valve, du plus récent au plus ancien. `library_600x900`
/// manque pour une partie des jeux (sorties récentes, fiches déplacées) : sans
/// ces variantes, la jaquette venait d'une source communautaire, parfois un
/// autre jeu que le nôtre.
const STEAM_COVER_FILES: [&str; 4] = [
    // 600×900 en premier : la version simple n'est que du 300×450, flou sur la
    // fiche et dans la grille.
    "library_600x900_2x.jpg",
    "library_600x900.jpg",
    "library_capsule_2x.jpg",
    "library_capsule.jpg",
];

/// Bannière d'un contenu additionnel : les visuels de la boutique quand PICS en
/// donne le chemin, sinon l'ancienne adresse à plat, puis la petite capsule, et
/// enfin la fiche de la boutique — PICS tait le visuel de contenus que
/// `appdetails` publie encore.
/// Un contenu n'a pas de jaquette verticale : sa vignette est paysage.
fn dlc_banner(appid: u32, image: &str, ask_store: bool) -> Option<Vec<u8>> {
    let mut urls: Vec<String> = Vec::new();
    if !image.trim().is_empty() {
        // L'hôte `akamai` sert le fichier directement ; `cloudflare` répond une
        // redirection vers `shared.steamstatic.com`, et la vignette restait
        // vide. Les deux sont essayés, le direct d'abord.
        for host in [
            "https://shared.akamai.steamstatic.com",
            "https://shared.cloudflare.steamstatic.com",
        ] {
            urls.push(format!(
                "{host}/store_item_assets/steam/apps/{appid}/{image}"
            ));
        }
    }
    for file in ["header.jpg", "capsule_231x87.jpg"] {
        urls.push(format!(
            "https://cdn.cloudflare.steamstatic.com/steam/apps/{appid}/{file}"
        ));
    }
    urls.into_iter()
        .find_map(|url| download_image(&url).filter(|bytes| bytes.len() > 512))
        .or_else(|| if ask_store { store_banner(appid) } else { None })
}

/// Bannière telle que la boutique la publie, seule source quand PICS ne donne
/// aucun chemin : les adresses `steam/apps/<id>/header.jpg` répondent 404 pour
/// les visuels récents. Un contenu non listé (sortie à venir, fiche retirée)
/// n'a aucune image, ni ici ni ailleurs.
fn store_banner(appid: u32) -> Option<Vec<u8>> {
    let url = format!("https://store.steampowered.com/api/appdetails?appids={appid}&l=french");
    let response = ureq::get(&url)
        .timeout(Duration::from_secs(8))
        .call()
        .ok()?;
    let value: serde_json::Value = serde_json::from_reader(response.into_reader()).ok()?;
    let entry = value.get(appid.to_string())?;
    if !entry
        .get("success")
        .and_then(|ok| ok.as_bool())
        .unwrap_or(false)
    {
        return None;
    }
    let image = entry.get("data")?.get("header_image")?.as_str()?;
    download_image(image).filter(|bytes| bytes.len() > 512)
}

/// Jaquette officielle, essayée sur les deux hôtes du CDN. Une image qui n'est
/// pas portrait (hero, bannière) est refusée : recadrée en 2:3, elle ne montre
/// qu'une bande de ciel.
fn steam_cdn_cover(appid: u32) -> Option<Vec<u8>> {
    for file in STEAM_COVER_FILES {
        for host in [
            "https://cdn.cloudflare.steamstatic.com",
            "https://shared.cloudflare.steamstatic.com",
        ] {
            let url = format!("{host}/steam/apps/{appid}/{file}");
            let Some(bytes) = download_image(&url) else {
                continue;
            };
            let portrait = image::load_from_memory(&bytes)
                .map(|image| portrait_cover(image.width(), image.height()))
                .unwrap_or(false);
            if portrait {
                return Some(bytes);
            }
        }
    }
    // Aucune jaquette portrait publiée : une bannière ou un héros recadré au
    // centre ne montre qu'un fragment de logotype, jamais une pochette. La case
    // reste vide ici, et la source suivante — SteamGridDB — ou la vignette au
    // nom du jeu prend le relais.
    None
}

/// Une jaquette doit être portrait ou carrée.
fn portrait_cover(width: u32, height: u32) -> bool {
    height > 0 && width as f32 <= height as f32 * 1.05
}

/// Fichier de jaquette utilisable comme icône : le choix manuel d'abord, puis
/// la jaquette de la tuile. Les jeux de boutique sont rangés sous
/// `store-<appid>.jpg` ; sans cette lecture, leur aperçu au survol n'aurait
/// aucune icône à recadrer.
fn cover_for_icon(paths: &LapinuxPaths, appid: u32) -> Option<PathBuf> {
    let dir = paths.covers_dir();
    [
        paths.existing_custom_cover_path(appid),
        dir.join(format!("store-{appid}.jpg")),
        dir.join(format!("{appid}.jpg")),
        dir.join(format!("sgdb-{appid}.jpg")),
    ]
    .into_iter()
    .find(|path| path.is_file())
}

/// Icône carrée faute de mieux : le centre de la jaquette, recadré. Une partie
/// des jeux récents n'a aucune icône publiée par Steam ; une case carrée vaut
/// mieux qu'une jaquette écrasée à sa place.
fn square_icon_from_cover(
    ctx: &egui::Context,
    paths: &LapinuxPaths,
    appid: u32,
) -> Option<egui::TextureHandle> {
    let id = egui::Id::new(("square-icon", appid));
    if let Some(found) = ctx.data(|data| data.get_temp::<Option<egui::TextureHandle>>(id)) {
        return found;
    }
    let found = (|| {
        let bytes = std::fs::read(cover_for_icon(paths, appid)?).ok()?;
        let image = image::load_from_memory(&bytes).ok()?;
        let (width, height) = (image.width(), image.height());
        let side = width.min(height);
        if side == 0 {
            return None;
        }
        let cropped = image.crop_imm((width - side) / 2, (height - side) / 2, side, side);
        let rgba = cropped.to_rgba8();
        let size = [rgba.width() as usize, rgba.height() as usize];
        let color = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
        Some(ctx.load_texture(
            format!("icon-square-{appid}"),
            color,
            egui::TextureOptions::LINEAR,
        ))
    })();
    ctx.data_mut(|data| data.insert_temp(id, found.clone()));
    found
}

/// Barycentre des zones contrastées d'une illustration, en coordonnées 0..1.
/// Les logos et les personnages portent l'essentiel des contours : ce point
/// sert d'ancre au recadrage pour ne pas les couper. Le calcul se fait sur une
/// vignette de 96 pixels, une seule fois par capsule.
fn image_focus(path: &Path) -> Option<egui::Vec2> {
    let bytes = std::fs::read(path).ok()?;
    let image = image::load_from_memory(&bytes).ok()?;
    let thumb = image.thumbnail(96, 96).to_luma8();
    let (width, height) = (thumb.width() as usize, thumb.height() as usize);
    if width < 3 || height < 3 {
        return None;
    }
    let luma = |x: usize, y: usize| thumb.get_pixel(x as u32, y as u32)[0] as f32;
    let mut total = 0.0_f32;
    let (mut sx, mut sy) = (0.0_f32, 0.0_f32);
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let edge =
                (luma(x + 1, y) - luma(x - 1, y)).abs() + (luma(x, y + 1) - luma(x, y - 1)).abs();
            if edge <= 0.0 {
                continue;
            }
            total += edge;
            sx += edge * x as f32;
            sy += edge * y as f32;
        }
    }
    if total < 1.0 {
        return None;
    }
    // Un rappel modéré vers le centre : un détail isolé dans un coin ne doit
    // pas déplacer tout le cadrage.
    let clamp = |value: f32| (0.5 + (value - 0.5) * 0.6).clamp(0.25, 0.75);
    Some(egui::vec2(
        clamp(sx / total / (width - 1) as f32),
        clamp(sy / total / (height - 1) as f32),
    ))
}

/// Décode un fichier image et le téléverse comme texture egui.
fn load_texture_file(ctx: &egui::Context, path: &Path, name: &str) -> Option<egui::TextureHandle> {
    let bytes = std::fs::read(path).ok()?;
    let image = image::load_from_memory(&bytes).ok()?;
    Some(texture_from_image(ctx, image, name))
}

fn texture_from_image(
    ctx: &egui::Context,
    image: image::DynamicImage,
    name: &str,
) -> egui::TextureHandle {
    let rgba = image.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let color = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
    ctx.load_texture(name, color, egui::TextureOptions::LINEAR)
}

/// Taille maximale d'une jaquette conservée en texture. Une jaquette `2x`
/// (1200×1800) pèse huit mégaoctets en pixels pour une vignette large de
/// 150 pixels : la réduire à 600×900 la garde nette, y compris sur un écran
/// HiDPI, et divise par quatre la mémoire des tuiles affichées.
const COVER_MAX: (u32, u32) = (600, 900);

/// Décodage d'une jaquette, réduite à la taille où elle est réellement peinte.
fn cover_texture(
    ctx: &egui::Context,
    image: image::DynamicImage,
    name: &str,
    limit: [u32; 2],
) -> egui::TextureHandle {
    // `thumbnail` échantillonne par blocs au lieu de filtrer chaque pixel :
    // trois à cinq fois plus rapide qu'un redimensionnement triangulaire sur
    // une jaquette 1200×1800, pour un résultat indiscernable à cette taille.
    let image = if image.width() > limit[0] || image.height() > limit[1] {
        image.thumbnail(limit[0], limit[1])
    } else {
        image
    };
    texture_from_image(ctx, image, name)
}

/// Nombre de frames gardées pour un visuel animé : au-delà, la mémoire par
/// image dépasserait celle de la bibliothèque entière.
const MAX_GIF_FRAMES: usize = 20;

/// Joue un GIF : une texture unique remplacée à chaque frame, sans créer une
/// texture par image.
pub(crate) struct GifPlayer {
    frames: Vec<egui::ColorImage>,
    delays: Vec<f32>,
    texture: egui::TextureHandle,
    index: usize,
    next_at: f64,
}

impl GifPlayer {
    pub(crate) fn load(
        ctx: &egui::Context,
        bytes: &[u8],
        name: &str,
        limit: [u32; 2],
    ) -> Option<Self> {
        let decoded = gif_frames(bytes, limit)?;
        let (first, delay) = decoded.first()?;
        let texture = ctx.load_texture(name, first.clone(), egui::TextureOptions::LINEAR);
        let now = ctx.input(|input| input.time);
        Some(Self {
            frames: decoded.iter().map(|(frame, _)| frame.clone()).collect(),
            delays: decoded.iter().map(|(_, delay)| *delay).collect(),
            texture,
            index: 0,
            next_at: now + *delay as f64,
        })
    }

    fn tick(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|input| input.time);
        let mut changed = false;
        while now >= self.next_at {
            self.index = (self.index + 1) % self.frames.len();
            self.next_at += self.delays[self.index] as f64;
            changed = true;
        }
        if changed {
            self.texture.set(
                self.frames[self.index].clone(),
                egui::TextureOptions::LINEAR,
            );
        }
        let wait = (self.next_at - now).max(0.01);
        ctx.request_repaint_after(Duration::from_secs_f64(wait));
    }

    pub(crate) fn first_texture(&self) -> egui::TextureHandle {
        self.texture.clone()
    }

    pub(crate) fn texture(&mut self, ctx: &egui::Context) -> &egui::TextureHandle {
        self.tick(ctx);
        &self.texture
    }
}

/// Décode un GIF en frames RGBA déjà réduites à la taille d'affichage.
fn gif_frames(bytes: &[u8], limit: [u32; 2]) -> Option<Vec<(egui::ColorImage, f32)>> {
    use image::AnimationDecoder;
    let decoder = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(bytes)).ok()?;
    let frames = decoder
        .into_frames()
        .take(MAX_GIF_FRAMES)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    let mut out = Vec::with_capacity(frames.len());
    for frame in frames {
        let (num, den) = frame.delay().numer_denom_ms();
        let delay = if den == 0 {
            0.1
        } else {
            (num as f32 / den as f32 / 1000.0).clamp(0.04, 0.5)
        };
        let image =
            image::DynamicImage::ImageRgba8(frame.into_buffer()).thumbnail(limit[0], limit[1]);
        let rgba = image.to_rgba8();
        let color = egui::ColorImage::from_rgba_unmultiplied(
            [rgba.width() as usize, rgba.height() as usize],
            rgba.as_raw(),
        );
        out.push((color, delay));
    }
    (!out.is_empty()).then_some(out)
}

/// Vignette de la galerie : fixe ou animée.
enum Thumb {
    Still(egui::TextureHandle),
    Gif(GifPlayer),
}

impl Thumb {
    fn texture(&mut self, ctx: &egui::Context) -> &egui::TextureHandle {
        match self {
            Thumb::Still(texture) => texture,
            Thumb::Gif(player) => player.texture(ctx),
        }
    }
}

/// Charge une jaquette en refusant les images paysage : celles d'un ancien
/// cache ou d'une source communautaire ne doivent pas être rognées en 2:3.
fn load_cover_file(
    ctx: &egui::Context,
    path: &Path,
    name: &str,
    limit: [u32; 2],
) -> Option<egui::TextureHandle> {
    let bytes = std::fs::read(path).ok()?;
    let image = image::load_from_memory(&bytes).ok()?;
    if !portrait_cover(image.width(), image.height()) {
        return None;
    }
    Some(cover_texture(ctx, image, name, limit))
}

/// Variante réservée à notre cache : un fichier paysage qui traîne (ancien
/// téléchargement d'un hero) est effacé pour que la source suivante reprenne.
fn load_cached_cover(
    ctx: &egui::Context,
    path: &Path,
    name: &str,
    limit: [u32; 2],
) -> Option<egui::TextureHandle> {
    match load_cover_file(ctx, path, name, limit) {
        Some(texture) => Some(texture),
        None => {
            if path.is_file() {
                let _ = std::fs::remove_file(path);
            }
            None
        }
    }
}

fn download_image(url: &str) -> Option<Vec<u8>> {
    let response = ureq::get(url)
        .timeout(Duration::from_secs(12))
        .call()
        .ok()?;
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut response.into_reader(), &mut bytes).ok()?;
    Some(bytes)
}

fn download_text(url: &str) -> Option<String> {
    let response = ureq::get(url)
        .set("User-Agent", "Lapinux/1.0")
        .timeout(Duration::from_secs(12))
        .call()
        .ok()?;
    response.into_string().ok()
}

/// Icône communautaire de la fiche boutique : la page l'expose à côté du nom,
/// dans le bloc `apphub_AppIcon`. Elle n'est pas dans l'API `appdetails`, qui
/// ne publie que les jaquettes.
pub(crate) fn steam_store_icon_url(html: &str) -> Option<String> {
    let block = &html[html.find("apphub_AppIcon")?..];
    let image = &block[block.find("<img")?..];
    let source = &image[image.find("src=\"")? + "src=\"".len()..];
    let end = source.find('"')?;
    let url = source[..end].trim().replace("&amp;", "&");
    if url.starts_with("https://") && (url.contains(".jpg") || url.contains(".png")) {
        Some(url)
    } else {
        None
    }
}

/// Noms de jeux absents du catalogue, résolus en tâche de fond. L'événement
/// d'ami garde l'appid quand Steam n'a pas publié le nom : le panneau complète
/// son affichage dès que la boutique a répondu.
fn app_name_cache() -> &'static Mutex<HashMap<u32, Option<String>>> {
    static CACHE: OnceLock<Mutex<HashMap<u32, Option<String>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn app_name_requests() -> &'static Mutex<HashSet<u32>> {
    static REQUESTS: OnceLock<Mutex<HashSet<u32>>> = OnceLock::new();
    REQUESTS.get_or_init(|| Mutex::new(HashSet::new()))
}

pub(crate) fn steam_app_name_async(ctx: &egui::Context, appid: u32) -> Option<String> {
    if appid == 0 {
        return None;
    }
    if let Ok(names) = app_name_cache().lock() {
        if let Some(name) = names.get(&appid) {
            return name.clone();
        }
    }
    let mut start = false;
    if let Ok(mut requests) = app_name_requests().lock() {
        if requests.insert(appid) {
            start = true;
        }
    }
    if start {
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let url = format!(
                "https://store.steampowered.com/api/appdetails?appids={appid}&filters=basic&l=french"
            );
            let name = download_text(&url)
                .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
                .and_then(|value| {
                    value
                        .get(appid.to_string())?
                        .get("data")?
                        .get("name")?
                        .as_str()
                        .map(str::trim)
                        .filter(|name| !name.is_empty())
                        .map(str::to_string)
                });
            if let Ok(mut names) = app_name_cache().lock() {
                names.insert(appid, name);
            }
            if let Ok(mut requests) = app_name_requests().lock() {
                requests.remove(&appid);
            }
            ctx.request_repaint();
        });
    }
    None
}

/// Une image proposée par SteamGridDB : miniature pour la galerie, URL pleine
/// résolution pour le téléchargement, et dimensions annoncées.
#[derive(Clone)]
struct CoverOption {
    thumb: String,
    full: String,
    width: u32,
    height: u32,
    /// Image animée : la galerie la fait jouer.
    gif: bool,
}

/// Type d'image demandé à SteamGridDB. Une affiche portrait n'est pas une
/// bannière : chaque type a son point d'entrée, ses dimensions et son cadre
/// dans la galerie.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CoverKind {
    /// Affiche portrait 2:3 : la jaquette de la grille.
    Poster,
    /// Grille paysage : la capsule de bibliothèque (460×215).
    Wide,
    /// Icône carrée : la pastille du jeu (512×512).
    Square,
    /// Illustration large de la fiche (héros, 1920×620).
    Hero,
    /// Logo détouré, sur fond transparent.
    Logo,
}

impl CoverKind {
    const ALL: [CoverKind; 5] = [
        CoverKind::Poster,
        CoverKind::Wide,
        CoverKind::Square,
        CoverKind::Hero,
        CoverKind::Logo,
    ];

    fn label(self) -> &'static str {
        match self {
            CoverKind::Poster => "Jaquettes",
            CoverKind::Wide => "Capsules",
            CoverKind::Square => lapinux_core::i18n::t("app.icons"),
            CoverKind::Hero => lapinux_core::i18n::t("app.banners"),
            CoverKind::Logo => "Logos",
        }
    }

    /// Adresse du type demandé. `animated` ramène aussi les GIF animés, que
    /// la galerie sait jouer ; les WebM sont écartés localement, le décodeur
    /// d'images ne les lit pas. Aucun filtre de format ni de dimensions :
    /// SteamGridDB répond 400 à certaines combinaisons, et le tri se fait ici.
    fn url(self, game_id: u64) -> String {
        let base = "https://www.steamgriddb.com/api/v2";
        let still = "types=static,animated";
        match self {
            CoverKind::Poster => {
                format!("{base}/grids/game/{game_id}?{still}&dimensions=600x900,342x482")
            }
            CoverKind::Wide => {
                format!("{base}/grids/game/{game_id}?{still}&dimensions=460x215,920x430")
            }
            CoverKind::Square => format!("{base}/icons/game/{game_id}?{still}"),
            CoverKind::Hero => format!("{base}/heroes/game/{game_id}?{still}"),
            CoverKind::Logo => format!("{base}/logos/game/{game_id}?{still}"),
        }
    }

    /// Cadre d'une vignette dans la galerie : le rapport du type demandé.
    fn frame(self) -> egui::Vec2 {
        match self {
            CoverKind::Poster => egui::vec2(140.0, 210.0),
            CoverKind::Wide => egui::vec2(212.0, 99.0),
            CoverKind::Square => egui::vec2(104.0, 104.0),
            CoverKind::Hero => egui::vec2(284.0, 92.0),
            CoverKind::Logo => egui::vec2(212.0, 99.0),
        }
    }

    /// Ce type accepte-t-il une image de ces dimensions ? SteamGridDB rend
    /// parfois d'autres tailles que celles demandées.
    fn accepts(self, width: u32, height: u32) -> bool {
        if width == 0 || height == 0 {
            // Dimensions absentes de la réponse : le type demandé fait foi.
            return true;
        }
        let ratio = width as f32 / height as f32;
        match self {
            CoverKind::Poster => (0.6..=0.75).contains(&ratio),
            CoverKind::Wide => (1.8..=2.3).contains(&ratio),
            CoverKind::Square => (0.9..=1.1).contains(&ratio),
            CoverKind::Hero => ratio > 2.3,
            // Un logo est détouré : son cadre suit son dessin, sans rapport
            // imposé.
            CoverKind::Logo => true,
        }
    }
}

fn sgdb_json(key: &str, url: &str) -> Result<serde_json::Value, String> {
    let response = ureq::get(url)
        .set("Authorization", &format!("Bearer {key}"))
        .set(
            "User-Agent",
            &format!("{}/{}", lapinux_core::APP_NAME, lapinux_core::VERSION),
        )
        .timeout(Duration::from_secs(10))
        .call();
    match response {
        Ok(response) => response
            .into_string()
            .ok()
            .and_then(|body| serde_json::from_str(&body).ok())
            .ok_or_else(|| lapinux_core::i18n::t("app.sgdb_unreadable").to_string()),
        Err(ureq::Error::Status(401, _)) | Err(ureq::Error::Status(403, _)) => {
            Err(lapinux_core::i18n::t("app.sgdb_key_refused").to_string())
        }
        Err(ureq::Error::Status(404, _)) => Ok(serde_json::Value::Null),
        Err(ureq::Error::Status(code, _)) => Err(lapinux_core::i18n::t_args(
            "app.sgdb_status",
            &[("code", &code.to_string())],
        )),
        Err(_) => Err("SteamGridDB injoignable".to_string()),
    }
}

/// Retrouve l'identifiant SteamGridDB d'un jeu : l'appid Steam d'abord, le nom
/// en repli pour les jeux absents du catalogue sous cet identifiant.
fn sgdb_game_id(appid: u32, name: &str, key: &str) -> Result<u64, String> {
    let by_steam = sgdb_json(
        key,
        &format!("https://www.steamgriddb.com/api/v2/games/steam/{appid}"),
    )
    .ok()
    .and_then(|value| first_game_id(&value));
    if let Some(id) = by_steam {
        return Ok(id);
    }
    let value = sgdb_json(
        key,
        &format!(
            "https://www.steamgriddb.com/api/v2/search/autocomplete/{}",
            url_encode(name.trim())
        ),
    )?;
    let matches: Vec<_> = value
        .get("data")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter(|game| {
            game.get("name")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|candidate| candidate.trim().eq_ignore_ascii_case(name.trim()))
        })
        .filter_map(|game| game.get("id").and_then(serde_json::Value::as_u64))
        .collect();
    match matches.as_slice() {
        [id] => Ok(*id),
        _ => Err(format!(
            "correspondance exacte unique introuvable pour « {name} » sur SteamGridDB"
        )),
    }
}

/// Liste les jaquettes disponibles, la plus proche du format portrait 2:3 en tête.
fn sgdb_grids(appid: u32, name: &str, key: &str) -> Result<Vec<CoverOption>, String> {
    let game_id = sgdb_game_id(appid, name, key)?;
    let value = sgdb_json(
        key,
        &format!("https://www.steamgriddb.com/api/v2/grids/game/{game_id}?types=static"),
    )?;
    let mut options: Vec<(f64, CoverOption)> = value
        .get("data")
        .and_then(|data| data.as_array())
        .map(|grids| grids.as_slice())
        .unwrap_or_default()
        .iter()
        .filter_map(|grid| {
            let full = grid.get("url")?.as_str()?.to_string();
            let thumb = grid
                .get("thumb")
                .and_then(|thumb| thumb.as_str())
                .unwrap_or(&full)
                .to_string();
            let width = grid.get("width").and_then(|v| v.as_u64()).unwrap_or(0);
            let height = grid.get("height").and_then(|v| v.as_u64()).unwrap_or(0);
            // Une pochette est en 2:3 (parfois 3:4). Les grilles paysage
            // (hero réuploadé, bannière) comme les carrés 1:1, qui ne
            // laisseraient qu'une bande une fois recadrés, sont écartées.
            let ratio = width as f32 / height.max(1) as f32;
            if !(0.60..=0.76).contains(&ratio) {
                return None;
            }
            Some((
                grid_score(width, height),
                CoverOption {
                    thumb,
                    full,
                    width: width as u32,
                    height: height as u32,
                    gif: false,
                },
            ))
        })
        .collect();
    options.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    options.truncate(48);
    if options.is_empty() {
        return Err(lapinux_core::i18n::t_args(
            "app.cover_not_found",
            &[("name", &name)],
        ));
    }
    Ok(options.into_iter().map(|(_, option)| option).collect())
}

/// Galerie d'un type d'image demandé : jaquettes, capsules, icônes, bannières
/// ou logos. Les vidéos animées sont écartées — le décodeur d'images ne sait
/// pas les lire — et chaque option garde ses dimensions pour que la galerie les
/// affiche sous la vignette.
fn sgdb_options(
    appid: u32,
    name: &str,
    key: &str,
    kind: CoverKind,
) -> Result<Vec<CoverOption>, String> {
    let game_id = sgdb_game_id(appid, name, key)?;
    let value = sgdb_json(key, &kind.url(game_id))?;
    let mut options: Vec<(f64, CoverOption)> = value
        .get("data")
        .and_then(|data| data.as_array())
        .map(|assets| assets.as_slice())
        .unwrap_or_default()
        .iter()
        .filter_map(|asset| {
            let full = asset.get("url")?.as_str()?.to_string();
            let mime = asset
                .get("mime")
                .and_then(|mime| mime.as_str())
                .unwrap_or_default();
            if mime.starts_with("video/") {
                return None;
            }
            // Le décodeur ne lit ni AVIF ni les autres formats rares : une
            // entrée qu'on ne saurait pas peindre n'a rien à faire ici.
            let supported = mime.is_empty()
                || matches!(
                    mime,
                    "image/png" | "image/jpeg" | "image/jpg" | "image/webp" | "image/gif"
                );
            if !supported {
                return None;
            }
            let gif = mime.eq_ignore_ascii_case("image/gif");
            let thumb = asset
                .get("thumb")
                .and_then(|thumb| thumb.as_str())
                .unwrap_or(&full)
                .to_string();
            let width = asset.get("width").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            let height = asset.get("height").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            if !kind.accepts(width, height) {
                return None;
            }
            Some((
                f64::from(width.min(4096) * height.min(4096)),
                CoverOption {
                    thumb,
                    full,
                    width,
                    height,
                    gif,
                },
            ))
        })
        .collect();
    options.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    options.truncate(60);
    if options.is_empty() {
        return Err(lapinux_core::i18n::t_args(
            "app.cover_type_not_found",
            &[("name", name.trim())],
        ));
    }
    Ok(options.into_iter().map(|(_, option)| option).collect())
}

fn steamgriddb_cover(appid: u32, name: &str, key: &str) -> Option<Vec<u8>> {
    let options = sgdb_grids(appid, name, key).ok()?;
    let best = options.first()?;
    download_image(&best.full)
}

/// Bannière, capsule, logo ou icône communautaires, avec le même choix
/// automatique que pour les jaquettes : la meilleure image du jeu, sans
/// intervention. Une clé refusée ou un jeu inconnu rend simplement `None`, et
/// la source suivante de la liste est essayée.
fn sgdb_art(appid: u32, name: &str, key: &str, art: SteamArt) -> Option<Vec<u8>> {
    let game_id = sgdb_game_id(appid, name, key).ok()?;
    let base = "https://www.steamgriddb.com/api/v2";
    let endpoint = match art {
        SteamArt::Banner => format!("{base}/heroes/game/{game_id}"),
        // Les capsules Steam sont larges : les grilles paysage conviennent
        // mieux que les affiches portrait, d'où le repli sans filtre.
        SteamArt::Capsule | SteamArt::Wide => {
            format!("{base}/grids/game/{game_id}?types=static&dimensions=920x430,460x215")
        }
        SteamArt::Logo => format!("{base}/logos/game/{game_id}?types=static"),
        SteamArt::Cover => {
            format!("{base}/grids/game/{game_id}?types=static&dimensions=600x900,342x482")
        }
        SteamArt::Icon => format!("{base}/icons/game/{game_id}"),
    };
    let mut value = sgdb_json(key, &endpoint).ok()?;
    let empty = value
        .get("data")
        .and_then(|data| data.as_array())
        .map(|assets| assets.is_empty())
        .unwrap_or(true);
    if empty && art == SteamArt::Capsule {
        value = sgdb_json(key, &format!("{base}/grids/game/{game_id}?types=static")).ok()?;
    }
    let best = sgdb_asset_pick(&value, art)?;
    download_image(&best)
}

/// Choisit l'illustration la plus utile d'une réponse SteamGridDB : les images
/// marquées adulte ou floues sont écartées, puis la meilleure résolution gagne,
/// les logos et icônes préférant le PNG (transparence) et l'icône le carré.
fn sgdb_asset_pick(value: &serde_json::Value, art: SteamArt) -> Option<String> {
    let assets = value.get("data")?.as_array()?;
    let mut best: Option<(f64, String)> = None;
    for asset in assets {
        if asset
            .get("nsfw")
            .and_then(|flag| flag.as_bool())
            .unwrap_or(false)
            || asset
                .get("humor")
                .and_then(|flag| flag.as_bool())
                .unwrap_or(false)
            || asset.get("style").and_then(|style| style.as_str()) == Some("blurred")
        {
            continue;
        }
        let Some(url) = asset.get("url").and_then(|url| url.as_str()) else {
            continue;
        };
        let mime = asset
            .get("mime")
            .and_then(|mime| mime.as_str())
            .unwrap_or("");
        let width = asset.get("width").and_then(|v| v.as_u64()).unwrap_or(0) as f64;
        let height = asset.get("height").and_then(|v| v.as_u64()).unwrap_or(0) as f64;
        let mut score = (width.min(4096.0) * height.min(4096.0)).sqrt();
        if matches!(art, SteamArt::Logo | SteamArt::Icon) && mime.contains("png") {
            score += 2000.0;
        }
        if art == SteamArt::Icon {
            score -= (width / height.max(1.0) - 1.0).abs() * 4000.0;
        }
        if art == SteamArt::Cover {
            score -= (width / height.max(1.0) - 2.0 / 3.0).abs() * 6000.0;
        }
        if best.as_ref().map(|(top, _)| score > *top).unwrap_or(true) {
            best = Some((score, url.to_string()));
        }
    }
    best.map(|(_, url)| url)
}

/// Messages envoyés par le fil de récupération au fil d'affichage.
enum CoverMsg {
    Grids(Vec<CoverOption>),
    Thumb { url: String, bytes: Vec<u8> },
    Failed(String),
    Saved(Result<(), String>),
}

/// Galerie de jaquettes SteamGridDB ouverte pour un jeu.
struct CoverPicker {
    appid: u32,
    name: String,
    /// Type d'image affiché par la galerie.
    kind: CoverKind,
    /// Ouvert depuis le bouton « Choisir une jaquette » : la galerie ne
    /// montre alors que des affiches portrait, sans sélecteur de type.
    only_covers: bool,
    options: Vec<CoverOption>,
    thumbs: HashMap<String, Option<Thumb>>,
    loading: bool,
    saving: bool,
    error: Option<String>,
    sender: mpsc::Sender<CoverMsg>,
    rx: mpsc::Receiver<CoverMsg>,
}

impl CoverPicker {
    fn start(appid: u32, name: String) -> (Self, mpsc::Sender<CoverMsg>) {
        let (sender, rx) = mpsc::channel();
        (
            Self {
                appid,
                name,
                kind: CoverKind::Poster,
                only_covers: false,
                options: Vec::new(),
                thumbs: HashMap::new(),
                loading: true,
                saving: false,
                error: None,
                sender: sender.clone(),
                rx,
            },
            sender,
        )
    }
}

/// Enregistre l'image choisie comme jaquette personnalisée du jeu.
fn save_cover_from_url(url: &str, path: &Path) -> Result<(), String> {
    let bytes = download_image(url)
        .ok_or_else(|| lapinux_core::i18n::t("app.cover_download_failed").to_string())?;
    if bytes.len() < 1024 {
        return Err(lapinux_core::i18n::t("app.invalid_image").to_string());
    }
    image::load_from_memory(&bytes)
        .map_err(|_| lapinux_core::i18n::t("app.image_format_unsupported").to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| {
                lapinux_core::i18n::t_args("app.covers_folder_error", &[("error", &error.to_string())])
            })?;
    }
    std::fs::write(path, &bytes).map_err(|error| {
        lapinux_core::i18n::t_args("app.write_failed", &[("error", &error.to_string())])
    })
}

fn grid_score(width: u64, height: u64) -> f64 {
    if width == 0 || height == 0 {
        return f64::NEG_INFINITY;
    }
    let ratio = width as f64 / height as f64;
    let ratio_penalty = (ratio - 2.0 / 3.0).abs() * 1000.0;
    let orientation_penalty = if height >= width { 0.0 } else { 500.0 };
    let resolution = (width.min(height) as f64).sqrt();
    resolution - ratio_penalty - orientation_penalty
}

/// Encode un nom de jeu pour un segment d'URL (`Portal 2` → `Portal%202`).
fn url_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'_' | b'.' | b'~') {
            out.push(*byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Renommage en cours d'une manette : la clé stable la retrouve d'une
/// image à l'autre, même quand evdev change de numéro de nœud.
struct PadRename {
    key: String,
    text: String,
    focus: bool,
}

/// Extinction ou allumage en cours : la première pression sur une manette
/// allumée arme la confirmation pendant quelques secondes, la seconde coupe
/// la liaison en arrière-plan. Une manette hors ligne se reconnecte en un clic.
struct PadPower {
    key: String,
    kind: PowerKind,
    armed_until: Option<Instant>,
    rx: Option<mpsc::Receiver<Result<(), String>>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PowerKind {
    Off,
    On,
    /// Coupure du Bluetooth pour laisser le câble USB prendre le relais.
    Wired,
}

/// Ce que l'en-tête de la manette peut proposer pour l'allumer ou l'éteindre.
/// Une manette filaire est alimentée par le câble : aucun rapport HID ne
/// l'éteint, le bouton est donc désactivé.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PadPowerControl {
    Hidden,
    Off { armed: bool, pending: bool },
    On { pending: bool, failed: bool },
    Wired,
}

/// Dernier état complet de la manette sélectionnée. Un relevé hidraw retire
/// brièvement `event27` : sans cet instantané, la fiche et son défilement
/// disparaîtraient à chaque test avant de revenir.
struct PadCache {
    key: String,
    device: DeviceInfo,
    pad: lapinux_input::PadState,
}

/// Réglages des tests matériels, conservés entre deux images.
struct PadTestState {
    vibration: f32,
    vibration_note: Option<String>,
    steam_raw: Option<lapinux_input::SteamPads>,
    steam_raw_note: Option<String>,
    /// Relevé HID lancé en fond : l'interface reste réactive et l'utilisateur
    /// peut poser les mains après le clic, sans tenir la souris.
    steam_raw_rx: Option<mpsc::Receiver<std::result::Result<lapinux_input::SteamPads, String>>>,
    /// Garde la fiche affichée pendant qu'un accès hidraw fait disparaître
    /// evdev. Expire vite pour ne pas masquer une vraie déconnexion.
    steam_hold_until: Option<Instant>,
    steam_haptic_note: Option<String>,
    steam_haptic_duration: f32,
    steam_grip_note: Option<String>,
    steam_grip_rx: Option<mpsc::Receiver<std::result::Result<(), String>>>,
    haptic_pattern: lapinux_input::control::HapticPattern,
    haptic_hd: bool,
    haptic_tone: f32,
    haptic_left: f32,
    haptic_right: f32,
    haptic_rate: f32,
    haptic_duration: f32,
    trigger_seconds: f32,
    trigger_note: Option<String>,
    led_hue: f32,
    led_on: bool,
    led_note: Option<String>,
    speaker_note: Option<String>,
    /// Volume du haut-parleur de la manette, en pourcentage.
    speaker_volume: f32,
    /// Sortie dont le volume a été chargé dans `speaker_volume`.
    speaker_for: Option<String>,
    /// Le curseur a bougé : appliquer le volume au relâchement.
    speaker_dirty: bool,
    /// Écoute directe du micro de la manette (USB uniquement).
    mic_monitor: bool,
    mic_note: Option<String>,
    /// Infos BlueZ de la manette affichée, chargées hors du fil d'affichage.
    bluetooth: Option<lapinux_input::bluetooth::BluetoothDetails>,
    bluetooth_for: Option<String>,
    bluetooth_rx: Option<mpsc::Receiver<Option<lapinux_input::bluetooth::BluetoothDetails>>>,
    /// Configuration Lapinux dont le test en direct est ouvert.
    steam_input_test: Option<u64>,
}

impl Default for PadTestState {
    fn default() -> Self {
        Self {
            vibration: 60.0,
            vibration_note: None,
            steam_raw: None,
            steam_raw_note: None,
            steam_raw_rx: None,
            steam_hold_until: None,
            steam_haptic_note: None,
            steam_haptic_duration: 240.0,
            steam_grip_note: None,
            steam_grip_rx: None,
            haptic_pattern: lapinux_input::control::HapticPattern::Steady,
            haptic_hd: true,
            haptic_tone: 150.0,
            haptic_left: 70.0,
            haptic_right: 70.0,
            haptic_rate: 4.0,
            haptic_duration: 1.2,
            trigger_seconds: 3.0,
            trigger_note: None,
            led_hue: 210.0,
            led_on: true,
            led_note: None,
            speaker_note: None,
            speaker_volume: 70.0,
            speaker_for: None,
            speaker_dirty: false,
            mic_monitor: false,
            mic_note: None,
            bluetooth: None,
            bluetooth_for: None,
            bluetooth_rx: None,
            steam_input_test: None,
        }
    }
}

/// Ce que la boutique publie d'un jeu — description, captures, éditeur —,
/// lu une seule fois par fiche et gardé en mémoire. Les captures suivent le
/// même chemin que les visuels d'annonces : un fil par adresse, puis une
/// texture réutilisée.
#[derive(Default)]
struct StoreInfo {
    data: HashMap<u32, lapinux_core::stores::StoreDetails>,
    asked: HashSet<u32>,
    error: HashMap<u32, String>,
    rx: Option<
        mpsc::Receiver<(
            u32,
            std::result::Result<lapinux_core::stores::StoreDetails, String>,
        )>,
    >,
}

impl StoreInfo {
    fn details(&self, appid: u32) -> Option<&lapinux_core::stores::StoreDetails> {
        self.data.get(&appid)
    }

    /// Captures déjà lues chez la boutique, dans l'ordre de sa fiche.
    fn screenshots(&self, appid: u32) -> Vec<String> {
        self.data
            .get(&appid)
            .map(|details| {
                details
                    .screenshots
                    .iter()
                    .take(HOVER_SHOT_LIMIT)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Visuel large publié par la boutique : il ouvre l'aperçu à la place de
    /// la capsule Steam, qui n'existe pas pour ces jeux.
    fn banner(&self, appid: u32) -> Option<String> {
        self.data
            .get(&appid)
            .map(|details| details.banner.trim())
            .filter(|banner| !banner.is_empty())
            .map(str::to_string)
    }

    /// La fiche est-elle demandée et pas encore arrivée ? C'est ce qui met le
    /// lapin dans les encarts vides, comme sur une fiche Steam.
    fn waiting(&self, appid: u32) -> bool {
        self.asked.contains(&appid)
            && !self.data.contains_key(&appid)
            && !self.error.contains_key(&appid)
    }

    /// Demande la fiche à la boutique une seule fois par jeu.
    fn ensure(&mut self, ctx: &egui::Context, client: &Client, appid: u32) {
        if self.rx.is_some() || self.data.contains_key(&appid) || !self.asked.insert(appid) {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        let worker = client.detached();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let reply = worker
                .request_with_timeout(&Req::StoreDetails { appid }, Duration::from_secs(60))
                .map_err(|error| error.to_string())
                .and_then(|envelope| {
                    envelope
                        .into_data::<lapinux_core::stores::StoreDetails>()
                        .map_err(|error| error.to_string())
                });
            if tx.send((appid, reply)).is_ok() {
                ctx.request_repaint();
            }
        });
    }

    /// Relève la fiche et les captures arrivées.
    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.rx {
            match rx.try_recv() {
                Ok((appid, Ok(details))) => {
                    self.data.insert(appid, details);
                    self.rx = None;
                }
                Ok((appid, Err(error))) => {
                    self.error.insert(appid, error);
                    self.rx = None;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    // Le fil de fond réveille l'écran dès sa réponse.
                    ctx.request_repaint_after(Duration::from_secs(1));
                }
                Err(mpsc::TryRecvError::Disconnected) => self.rx = None,
            }
        }
    }
}

/// Dossiers de cache que les réglages savent compter et vider.
fn cache_rows() -> [(&'static str, &'static str); 7] {
    [
        (lapinux_core::i18n::t("app.covers_kept"), "Covers"),
        (lapinux_core::i18n::t("app.banners_and_art"), "Art"),
        (lapinux_core::i18n::t("app.web_pages"), "webview"),
        (lapinux_core::i18n::t("app.game_pages"), "fiches"),
        (lapinux_core::i18n::t("app.store_pages"), "fiches-boutique"),
        (lapinux_core::i18n::t("gameinfo.achievements"), "succes"),
        (lapinux_core::i18n::t("stores.capability.cloud"), "clouds"),
    ]
}

/// Tailles des caches, mesurées hors de l'interface : un dossier de deux mille
/// jaquettes ne se compte pas à chaque image.
#[derive(Default)]
struct CacheUsage {
    rows: Vec<(&'static str, u64, usize)>,
    total: u64,
    rx: Option<mpsc::Receiver<Vec<(&'static str, u64, usize)>>>,
    /// Instant de la dernière mesure : les tailles ne sont pas relues à chaque
    /// image, seulement quand la page revient ou après un vidage.
    asked: Option<Instant>,
    /// Vidage en cours : l'espace libéré arrive par ce canal.
    clearing: Option<mpsc::Receiver<Result<u64, String>>>,
    note: Option<String>,
}

impl CacheUsage {
    /// Relit les tailles au plus toutes les dix secondes, jamais dans l'image.
    fn ensure(&mut self, ctx: &egui::Context, paths: &LapinuxPaths) {
        if self.rx.is_some() || self.clearing.is_some() {
            return;
        }
        if self
            .asked
            .is_some_and(|at| at.elapsed() < Duration::from_secs(10))
        {
            return;
        }
        self.asked = Some(Instant::now());
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        let mut dirs: Vec<(&'static str, PathBuf)> = cache_rows()
            .iter()
            .map(|(name, dir)| (*name, paths.cache_dir.join(dir)))
            .collect();
        // Le cache des pages de boutique est tenu par WebKit, dans le profil
        // des fenêtres de boutique ; il se vide avec les autres.
        dirs.push((
            lapinux_core::i18n::t("app.store_web_pages"),
            paths.store_web_dir().join("cache"),
        ));
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let rows = dirs
                .into_iter()
                .map(|(name, dir)| {
                    let (bytes, files) = if name == lapinux_core::i18n::t("app.covers_kept") {
                        cache_covers_usage(&dir)
                    } else {
                        tree_usage(&dir)
                    };
                    (name, bytes, files)
                })
                .collect();
            let _ = tx.send(rows);
            ctx.request_repaint();
        });
    }

    fn poll(&mut self) {
        if let Some(rx) = &self.rx {
            if let Ok(rows) = rx.try_recv() {
                self.total = rows.iter().map(|(_, bytes, _)| *bytes).sum();
                self.rows = rows;
                self.rx = None;
            }
        }
        if let Some(rx) = &self.clearing {
            if let Ok(result) = rx.try_recv() {
                self.note = Some(match result {
                    Ok(freed) => lapinux_core::i18n::t_fmt(
                        "app.freed",
                        &[lapinux_core::util::format_bytes(freed).to_string()],
                    ),
                    Err(error) => format!("Vidage incomplet : {error}"),
                });
                self.clearing = None;
                self.asked = None;
            }
        }
    }

    /// Une mesure ou un vidage est en route : les boutons attendent.
    fn working(&self) -> bool {
        self.rx.is_some() || self.clearing.is_some()
    }
}

/// Ne compte pas les anciens choix personnels, conservés au vidage.
fn cache_covers_usage(dir: &Path) -> (u64, usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (0, 0);
    };
    let mut bytes = 0;
    let mut files = 0;
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with("custom-") {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            let (inside, count) = tree_usage(&entry.path());
            bytes += inside;
            files += count;
        } else {
            bytes += meta.len();
            files += 1;
        }
    }
    (bytes, files)
}

/// Poids et nombre de fichiers d'un dossier, sous-dossiers compris.
fn tree_usage(dir: &Path) -> (u64, usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (0, 0);
    };
    let mut bytes = 0u64;
    let mut files = 0usize;
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            let (inside, count) = tree_usage(&entry.path());
            bytes += inside;
            files += count;
        } else {
            bytes += meta.len();
            files += 1;
        }
    }
    (bytes, files)
}

/// Poids sur disque des Protons installés, mesuré en tâche de fond : un build
/// compte des dizaines de milliers de fichiers, la mesure ne se fait jamais
/// dans l'image.
#[derive(Default)]
struct ProtonSizes {
    /// Taille mesurée, par dossier de build.
    sizes: HashMap<PathBuf, u64>,
    /// Mesures encore en route, pour ne pas relancer la même.
    pending: HashSet<PathBuf>,
    rx: Option<mpsc::Receiver<(PathBuf, u64)>>,
}

impl ProtonSizes {
    #[cfg(test)]
    fn get(&self, path: &Path) -> Option<u64> {
        self.sizes.get(path).copied()
    }

    /// Mesure les builds de la liste qui ne le sont pas encore. Une liste plus
    /// courte range les builds partis : une suppression n'en laisse pas la
    /// trace.
    fn ensure(&mut self, ctx: &egui::Context, protons: &[ProtonInfo]) {
        self.sizes
            .retain(|path, _| protons.iter().any(|proton| proton.path == *path));
        self.pending
            .retain(|path| protons.iter().any(|proton| proton.path == *path));
        if self.rx.is_some() {
            return;
        }
        let missing: Vec<PathBuf> = protons
            .iter()
            .map(|proton| proton.path.clone())
            .filter(|path| !self.sizes.contains_key(path) && !self.pending.contains(path))
            .collect();
        if missing.is_empty() {
            return;
        }
        for path in &missing {
            self.pending.insert(path.clone());
        }
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            for path in missing {
                let (bytes, _) = tree_usage(&path);
                if tx.send((path, bytes)).is_err() {
                    break;
                }
                ctx.request_repaint();
            }
        });
    }

    /// Récolte les mesures arrivées depuis l'image précédente.
    fn poll(&mut self) {
        let mut done: Vec<(PathBuf, u64)> = Vec::new();
        let mut finished = false;
        if let Some(rx) = &self.rx {
            loop {
                match rx.try_recv() {
                    Ok(pair) => done.push(pair),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        finished = true;
                        break;
                    }
                }
            }
        }
        for (path, bytes) in done {
            self.pending.remove(&path);
            self.sizes.insert(path, bytes);
        }
        if finished {
            self.rx = None;
        }
    }
}

/// Poids publié des jeux de boutique, demandé une fois par jeu et gardé : la
/// fiche l'affiche sur le bouton d'installation et dans ses étiquettes.
/// Poids publié d'un contenu additionnel de boutique.
#[derive(Debug, Clone, Default)]
struct DlcSize {
    /// Identifiant de l'offre : c'est lui qui distingue deux contenus, même
    /// quand aucun nom d'application n'est publié.
    item: String,
    app_name: String,
    download: u64,
    installed: u64,
}

#[derive(Default)]
struct StoreSizes {
    sizes: HashMap<u32, u64>,
    /// Poids sur le disque une fois installé, quand la boutique le publie.
    installed: HashMap<u32, u64>,
    asked: HashSet<u32>,
    rx: Option<mpsc::Receiver<(u32, u64, u64)>>,
}

impl StoreSizes {
    fn get(&self, appid: u32) -> Option<u64> {
        self.sizes.get(&appid).copied()
    }

    fn installed_of(&self, appid: u32) -> Option<u64> {
        self.installed.get(&appid).copied()
    }

    fn ensure(&mut self, ctx: &egui::Context, client: &Client, appid: u32) {
        if self.rx.is_some() || self.sizes.contains_key(&appid) || !self.asked.insert(appid) {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        let worker = client.detached();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let (size, installed) = worker
                .request_with_timeout(&Req::StoreSize { appid }, Duration::from_secs(60))
                .ok()
                .map(|envelope| {
                    (
                        envelope
                            .data
                            .get("size")
                            .and_then(|value| value.as_u64())
                            .unwrap_or(0),
                        envelope
                            .data
                            .get("installed")
                            .and_then(|value| value.as_u64())
                            .unwrap_or(0),
                    )
                })
                .unwrap_or((0, 0));
            if tx.send((appid, size, installed)).is_ok() {
                ctx.request_repaint();
            }
        });
    }

    fn poll(&mut self) {
        if let Some(rx) = &self.rx {
            if let Ok((appid, size, installed)) = rx.try_recv() {
                if size > 0 {
                    self.sizes.insert(appid, size);
                }
                if installed > 0 {
                    self.installed.insert(appid, installed);
                }
                self.rx = None;
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StudioKind {
    Theme,
    Plugin,
}

/// Éditeur de manifeste ouvert depuis Réglages › Apparence. Le contenu est du
/// TOML brut : un champ ajouté au moteur est immédiatement accessible sans
/// nouvelle fenêtre de formulaire.
struct ThemeStudio {
    kind: StudioKind,
    /// Fichier remplacé par « Enregistrer ». `None` crée un nouveau fichier.
    path: Option<PathBuf>,
    body: String,
    error: Option<String>,
}

impl ThemeStudio {
    fn new(kind: StudioKind) -> Self {
        Self {
            kind,
            path: None,
            body: match kind {
                StudioKind::Theme => theme::theme_template(theme::ThemeLayout::Classic),
                StudioKind::Plugin => theme::plugin_template(),
            },
            error: None,
        }
    }

    fn edit(kind: StudioKind, path: Option<PathBuf>, body: String) -> Self {
        Self {
            kind,
            path,
            body,
            error: None,
        }
    }

    fn title(&self) -> &'static str {
        match (self.kind, self.path.is_some()) {
            (StudioKind::Theme, false) => lapinux_core::i18n::t("settings.appearance.create_theme"),
            (StudioKind::Theme, true) => lapinux_core::i18n::t("app.edit_theme"),
            (StudioKind::Plugin, false) => {
                lapinux_core::i18n::t("settings.appearance.create_plugin")
            }
            (StudioKind::Plugin, true) => lapinux_core::i18n::t("app.edit_plugin"),
        }
    }
}

/// Une animation d'autocollant : toutes ses images sont rangées dans une
/// seule texture, en grille. Un autocollant Steam est un APNG de plusieurs
/// dizaines d'images ; une texture par image saturerait la mémoire de la
/// carte graphique.
#[derive(Clone)]
struct StickerAnimation {
    texture: egui::TextureHandle,
    width: u32,
    height: u32,
    columns: u32,
    rows: u32,
    delays: Vec<f32>,
    total: f32,
}

impl StickerAnimation {
    fn new(ctx: &egui::Context, name: &str, frames: StickerFrames) -> Option<Self> {
        let expected = frames.width as usize * frames.height as usize * 4;
        if frames.delays.is_empty() || frames.pixels.len() != expected {
            return None;
        }
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [frames.width as usize, frames.height as usize],
            &frames.pixels,
        );
        let texture = ctx.load_texture(
            format!("sticker-{name}"),
            image,
            egui::TextureOptions::LINEAR,
        );
        let total = frames.delays.iter().sum::<f32>();
        Some(Self {
            texture,
            width: frames.width,
            height: frames.height,
            columns: frames.columns,
            rows: frames.rows,
            delays: frames.delays,
            total,
        })
    }

    fn animated(&self) -> bool {
        self.delays.len() > 1 && self.total > 0.001
    }

    fn uv(&self, frame: usize) -> egui::Rect {
        let column = frame as u32 % self.columns.max(1);
        let row = frame as u32 / self.columns.max(1);
        let width = 1.0 / self.columns.max(1) as f32;
        let height = 1.0 / self.rows.max(1) as f32;
        // Un demi-texel de retrait évite que le filtrage linéaire mélange
        // l'image avec celle de la case voisine.
        let inset_x = 0.5 / self.width.max(1) as f32;
        let inset_y = 0.5 / self.height.max(1) as f32;
        egui::Rect::from_min_max(
            egui::pos2(
                column as f32 * width + inset_x,
                row as f32 * height + inset_y,
            ),
            egui::pos2(
                (column + 1) as f32 * width - inset_x,
                (row + 1) as f32 * height - inset_y,
            ),
        )
    }

    /// Image à peindre et temps restant avant la suivante. Une image fixe
    /// n'impose aucun rafraîchissement.
    fn frame_at(&self, time: f32) -> (egui::TextureId, egui::Rect, f32) {
        if !self.animated() {
            return (self.texture.id(), self.uv(0), f32::INFINITY);
        }
        let mut remaining = time.rem_euclid(self.total);
        for (index, delay) in self.delays.iter().enumerate() {
            if remaining < *delay {
                return (
                    self.texture.id(),
                    self.uv(index),
                    (*delay - remaining).max(0.016),
                );
            }
            remaining -= *delay;
        }
        let last = self.delays.len() - 1;
        (
            self.texture.id(),
            self.uv(last),
            self.delays[last].max(0.016),
        )
    }
}

/// Atlas RGBA préparé hors du fil d'affichage par le téléchargement des
/// autocollants : chaque image est réduite à la taille où elle est peinte.
struct StickerFrames {
    width: u32,
    height: u32,
    columns: u32,
    rows: u32,
    pixels: Vec<u8>,
    delays: Vec<f32>,
}

/// État de l'onglet Mises à jour : contrôle en cours, téléchargement suivi et
/// message laissé à l'écran.
#[derive(Default)]
struct UpdateUiState {
    status: Option<lapinux_core::update::UpdateStatus>,
    rx: Option<mpsc::Receiver<Result<lapinux_core::update::UpdateStatus, String>>>,
    progress: Option<lapinux_core::update::UpdateProgress>,
    poll: Option<Instant>,
    message: String,
    /// Une mise à jour a été installée : proposer le redémarrage.
    installed: bool,
    /// Un contrôle a déjà eu lieu : un échec réseau ne relance pas une requête
    /// à chaque image.
    attempted: bool,
}

struct LapinuxApp {
    client: Client,
    paths: LapinuxPaths,
    page: Page,
    /// Thèmes fournis et thèmes personnels, relus après chaque modification.
    theme_library: theme::ThemeLibrary,
    /// Plugins déclaratifs, actifs selon `config.general.active_plugins`.
    plugin_library: theme::PluginLibrary,
    /// Thème effectif, plugins compris.
    active_theme: theme::ActiveTheme,
    /// Le style egui doit être recalculé à l'image suivante.
    theme_dirty: bool,
    /// Jeu sélectionné dans le carrousel NX, conservé entre les images.
    nx_focus: Option<u32>,
    /// Éditeur de thème ou de plugin ouvert.
    theme_studio: Option<ThemeStudio>,
    /// Captures prises en jeu, relues depuis `~/Images`.
    captures: captures::Gallery,
    /// Lapinux s'est effacé derrière une partie : la fenêtre revient à sa
    /// sortie.
    hidden_for_game: bool,
    /// Dernière partie vue, pour l'écran de fin : le jeu a déjà disparu de
    /// `session` quand la fenêtre revient.
    last_session: Option<(u32, String)>,
    /// La dernière partie utilisait la scène : son écran de fin est déjà dans
    /// GameScope, l'interface ne doit pas en afficher un autre.
    last_session_scene: bool,
    /// Une partie était en cours à l'image précédente.
    playing_before: bool,
    /// Écran de fin affiché quelques secondes au retour du jeu : nom du jeu et
    /// instant où il s'efface.
    game_end: Option<(String, Instant)>,
    /// La fenêtre plein écran de fin est ouverte : elle doit être fermée même
    /// quand un lancement a effacé `game_end` entre deux images.
    game_end_open: bool,
    /// L'écran de fin a peint sa première image : son minuteur de quatre
    /// secondes ne part qu'à ce moment, pas avant le mappage de la fenêtre.
    game_end_timer_started: bool,
    /// Instant où la fenêtre a perdu le focus, pour le rangement automatique.
    idle_since: Option<Instant>,
    /// Instant du premier changement de réglage non enregistré : le brouillon
    /// part tout seul après une courte pause de saisie.
    settings_dirty_at: Option<Instant>,
    /// Enregistrement automatique en cours : la pastille « Configuration
    /// enregistrée » ne clignote pas à chaque cran de curseur.
    settings_autosave: bool,
    /// Un enregistrement a été lancé depuis les réglages, pour que le bouton
    /// propose l'arrêt au lieu du lancement.
    recording: bool,
    /// État réel de l'enregistreur, relu sur la page Captures.
    recording_state: Option<lapinux_core::model::RecordingState>,
    recording_poll: Instant,
    recording_rx: Option<mpsc::Receiver<Option<lapinux_core::model::RecordingState>>>,
    config: Config,
    games: Vec<Game>,
    protons: Vec<ProtonInfo>,
    releases: Vec<ProtonRelease>,
    /// Instant du dernier contrôle des builds publiés : la page Protons dit
    /// depuis quand sa liste date.
    protons_checked: Option<Instant>,
    /// Sources dépliées dans la page Protons (`repo` → ouvert) : chaque ligne
    /// ouvre la liste de ses builds.
    proton_open: HashMap<String, bool>,
    /// Lecture des builds publiés lancée à la première ouverture de la page :
    /// trois dépôts interrogés en tâche de fond, jamais dans l'interface.
    proton_fetch: Option<mpsc::Receiver<Result<Envelope, String>>>,
    /// Vrai dès que la page a demandé sa liste une fois.
    protons_auto: bool,
    /// Proton dont la suppression attend confirmation : la poubelle d'une
    /// ligne n'efface jamais un build d'un seul clic.
    proton_confirm: Option<String>,
    /// Poids sur disque des Protons, mesuré en tâche de fond.
    proton_sizes: ProtonSizes,
    jobs: Vec<Job>,
    session: Option<SessionInfo>,
    devices: Vec<DeviceInfo>,
    doctor: Option<DoctorReport>,
    doctor_requested: bool,
    selected: Option<u32>,
    /// Jeu affiché par la fiche : elle garde son contenu le temps de la
    /// refermer, même quand la sélection est déjà vide.
    detail_shown: Option<u32>,
    /// Avancement de la glissière de la fiche (0 fermée, 1 ouverte).
    detail_anim: f32,
    /// Fiche dépliée : la ligne compacte garde son en-tête et le corps
    /// détaillé (bannière, fiche Steam, réglages) s'ouvre en dessous.
    detail_expanded: bool,
    /// Succès Steam du jeu affiché dans la fiche (ligne compacte).
    achievements: achievements::Sheet,
    /// Détails Steam du jeu affiché (compte, mises à jour, boutique, stats),
    /// relus à chaque ouverture de la fiche dépliée.
    gameinfo: gameinfo::Shelf,
    /// Configurations du catalogue pour le jeu affiché — correctifs,
    /// traductions, mods — lues une fois par jeu à l'ouverture de la fiche.
    configurations: configurations::Shelf,
    /// Profils Steam Input de la manette branchée, lus une fois par jeu.
    steaminput: steaminput::Shelf,
    /// Configurations Steam Input recopiées dans Lapinux, pour la page
    /// Manettes : elles restent consultables même sans le jeu.
    saved_steam_inputs: Vec<SteamInputSaved>,
    saved_steam_inputs_rx: Option<mpsc::Receiver<Vec<SteamInputSaved>>>,
    saved_steam_inputs_poll: Instant,
    saved_steam_input_contents: HashMap<PathBuf, Result<Vec<String>, String>>,
    saved_steam_input_contents_rx: Option<mpsc::Receiver<(PathBuf, Result<Vec<String>, String>)>>,
    /// Atelier du jeu : abonnements, poids et rangement dans `Mods`.
    workshop: workshop::Shelf,
    /// Onglet ouvert dans la fiche, et le jeu auquel il appartient : changer
    /// de jeu ramène à Général.
    sheet_tab: SheetTab,
    sheet_tab_for: Option<u32>,
    /// Copies locales des sauvegardes, dans les réglages de la fiche.
    saves: saves::Panel,
    /// Poids de toutes les copies, lu pour la page Sauvegardes des réglages.
    saves_usage: Option<SavesUsage>,
    saves_usage_rx: Option<mpsc::Receiver<Option<SavesUsage>>>,
    /// Sauvegardes cloud du compte, par jeu, lues par le protocole : la fiche
    /// les demande une fois, puis garde la réponse.
    cloud: HashMap<u32, lapinux_core::model::CloudReport>,
    /// Lecture du cloud en cours : une seule à la fois, la fiche n'affiche
    /// qu'un jeu.
    cloud_fetch: Option<mpsc::Receiver<(u32, lapinux_core::model::CloudReport)>>,
    /// Jeu de cette lecture en vol : une fin d'installation ne doit annuler
    /// que la sienne, sans perdre le relevé d'un autre jeu.
    cloud_fetch_appid: Option<u32>,
    /// Jeux dont le cloud a déjà été demandé, réponse en vol comprise.
    cloud_asked: HashSet<u32>,
    /// Ponts Steamworks orphelins du jeu, chemins relatifs au dossier du jeu.
    steam_bridge_status: HashMap<u32, Vec<String>>,
    steam_bridge_fetch: Option<(u32, mpsc::Receiver<Result<Vec<String>, String>>)>,
    steam_bridge_repair: Option<(u32, mpsc::Receiver<Result<Vec<String>, String>>)>,
    /// Jeu dont le bouton de réparation attend le second clic.
    steam_bridge_armed: Option<u32>,
    /// Outil d'un jeu protégé dont la réactivation attend une confirmation
    /// (`appid:outil`). Un anti-triche ne se rallume pas d'un clic distrait.
    protect_armed: Option<String>,
    /// Récupération du cloud en cours : la tâche vit en fond, la carte des
    /// réglages du jeu annonce son état.
    cloud_restore: Option<(u32, mpsc::Receiver<Result<String, String>>)>,
    /// Jeu dont le bouton « Récupérer » attend sa confirmation.
    cloud_restore_armed: Option<u32>,
    /// Envoi vers le cloud en cours.
    cloud_upload: Option<(u32, mpsc::Receiver<Result<String, String>>)>,
    /// Jeu dont le bouton « Envoyer » attend sa confirmation.
    cloud_upload_armed: Option<u32>,
    cloud_resolution_armed: Option<(u32, lapinux_core::ipc::SteamCloudChoice)>,
    /// Contenus additionnels retenus par jeu, avant de lancer le
    /// téléchargement : c'est ce qui fait grossir le poids annoncé.
    dlc_selection: HashMap<u32, HashSet<u32>>,
    /// Version retenue par jeu dans le menu de la flèche (`linux`,
    /// `windows`, `macos`) : sans choix, la première publiée s'applique.
    version_choice: HashMap<u32, String>,
    /// Membre de la famille retenu par jeu quand plusieurs membres partagent
    /// le même titre : sans choix, le premier membre de Steam s'applique.
    family_choice: HashMap<u32, u64>,
    /// Panneau de choix des contenus additionnels, ouvert par le bouton « DLC ».
    dlc_modal: Option<u32>,
    /// Annonce ouverte dans la fenêtre de lecture.
    news_reader: Option<NewsReader>,
    /// Fenêtre des réglages d'un jeu, ouverte par l'engrenage de l'encart.
    game_settings: Option<u32>,
    /// Onglet actif dans les réglages du jeu.
    game_settings_tab: GameSettingsTab,
    /// Jeu dont la poubelle attend le second clic, avec l'instant du premier :
    /// la confirmation tient dans l'icône, sans fenêtre.
    uninstall_confirm: Option<(u32, Instant)>,
    uninstall_modal: Option<u32>,
    uninstall_error: Option<String>,
    uninstall_pending: Option<mpsc::Receiver<Result<Option<String>, String>>>,
    /// Identité du daemon qui tourne : version, binaire et démarrage. Relue
    /// quand l'onglet Système est ouvert.
    daemon_build: Option<DaemonBuild>,
    /// Veille sur la liaison avec le daemon : sa perte et son retour sont
    /// annoncés, sans attendre qu'une page l'affiche.
    daemon_watch: Instant,
    daemon_watch_rx: Option<mpsc::Receiver<bool>>,
    daemon_seen: bool,
    daemon_seen_once: bool,
    daemon_poll: Instant,
    search: String,
    search_open: bool,
    /// Ordre de la grille, alphabétique au départ.
    library_sort: LibrarySort,
    /// Sens du tri, relu depuis la configuration.
    library_descending: bool,
    /// Jeu qui a le focus clavier dans la grille.
    library_focus: Option<u32>,
    /// Jeu vers lequel la grille doit défiler après un déplacement au clavier.
    library_scroll_to: Option<u32>,
    /// Filtres d'attributs : mises à jour disponibles, jeux jamais lancés.
    only_updates: bool,
    never_played: bool,
    /// Instant du dernier changement de vue ou de collection : la
    /// configuration n'est écrite qu'une fois la sélection stabilisée.
    config_dirty_at: Option<Instant>,
    /// Collection ouverte dans la page Collections.
    collection_selected: Option<String>,
    /// Vrai quand la fiche ouverte vient d'un jeu d'une collection : elle
    /// s'affiche par-dessus la collection, sans quitter la page.
    collection_detail_open: bool,
    /// Formulaire de création déplié.
    collection_new: bool,
    collection_new_name: String,
    collection_new_dynamic: bool,
    collection_new_field: lapinux_core::config::CollectionField,
    /// Mode ajout/retrait des jeux d'une collection manuelle.
    collection_manage: bool,
    collection_search: String,
    /// Collection dont la suppression attend une confirmation.
    collection_delete: Option<String>,
    /// Menu des filtres ouvert.
    sort_menu: bool,
    /// Éditeur choisi dans le panneau des filtres.
    publisher_filter: Option<String>,
    /// Développeur choisi de la même façon.
    developer_filter: Option<String>,
    /// Liste de crédits ouverte : `Some(true)` pour les éditeurs, `Some(false)`
    /// pour les développeurs. Elle s'affiche à part, sans remplacer le panneau.
    credit_menu: Option<bool>,
    /// Recherche tapée dans cette liste.
    credit_query: String,
    /// Dernière vue publiée sur Discord, pour n'envoyer que les changements.
    discord_view_sent: Option<(
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<lapinux_core::stores::Store>,
        Option<String>,
        Option<u32>,
    )>,
    /// Instant de la dernière publication : une demande peut se perdre quand
    /// le daemon lit les boutiques au démarrage. La vue est donc renvoyée de
    /// temps en temps, même sans changement, pour que la présence revienne.
    discord_view_sent_at: Option<Instant>,
    /// Photographie des bibliothèques au lancement précédent, et jeux ajoutés
    /// depuis : la carte « Nouveaux jeux » du Profil les montre.
    library_seen: Option<SeenLibrary>,
    new_games: Vec<LibraryEntry>,
    library_checked: bool,
    library_check_at: Option<Instant>,
    /// Synchronisation lancée depuis l'icône de la Bibliothèque : la flèche
    /// tourne et la pastille d'état reste jusqu'au retour des trois lectures.
    library_sync: Option<Instant>,
    /// Fenêtre de synchronisation ouverte : les sources à relire, cochées.
    sync_dialog: Option<SyncDialog>,
    /// Une relecture est déjà en vol : le scan du disque partira à sa place,
    /// pour ne pas bloquer l'interface pendant l'attente du daemon.
    rescan_after_refresh: bool,
    /// Parties terminées, lues au daemon à la première ouverture du Profil :
    /// la page Activité en fait des cartes.
    play_history: Vec<lapinux_core::model::PlaySession>,
    play_history_rx: Option<mpsc::Receiver<Vec<lapinux_core::model::PlaySession>>>,
    play_history_asked: bool,
    /// Journal demandé depuis une carte d'Activité : le panneau fixe l'affiche
    /// en grand et le rend copiable, au lieu de déplier la carte.
    activity_log: Option<LogRequest>,
    /// Boutique retenue dans la page Boutiques : c'est son site qui s'affiche.
    store_filter: Option<lapinux_core::stores::Store>,
    focus_search: bool,
    friend_activity: Option<FriendActivityFlash>,
    notifications_open: bool,
    /// Notifications wishlist dont les détails sont dépliés, par clé stable.
    notifications_wishlist_open: HashSet<String>,
    notification_history: VecDeque<String>,
    /// Clés des événements d'amis déjà vus : le premier snapshot sert de
    /// référence, les suivants déclenchent le bandeau et le compteur non lu.
    friend_events_seen: HashSet<String>,
    friend_events_initialized: bool,
    friend_events_unread: u32,
    /// Clés des messages Steam déjà signalés. Le premier instantané vivant
    /// sert de référence, et seuls les messages `received_live` peuvent
    /// produire un toast : ouvrir un ancien historique reste silencieux.
    steam_messages_seen: HashSet<String>,
    steam_messages_initialized: bool,
    /// Dernier total de notifications Steam vu par la cloche : une hausse
    /// rallume la pastille, une baisse (notification lue ailleurs) abaisse la
    /// référence sans allumer quoi que ce soit.
    steam_notifications_seen: u32,
    /// Dernier jeu connu de chaque ami, gardé après son arrêt : Steam ne
    /// renvoie plus le titre dans l'événement « jeu quitté ».
    friend_games: HashMap<u64, (u32, String)>,
    /// Historique local des événements d'amis : il survit aux redémarrages et
    /// reste visible dans le modal après la reconnexion.
    friend_events_history: VecDeque<SteamFriendEvent>,
    dry_run: Option<String>,
    /// Essai à blanc de la fiche ouverte, rendu en bas d'Informations.
    dry_run_info: Option<DryRunPlan>,
    covers: CoverCache,
    /// Captures de l'aperçu de survol, chargées à la demande.
    hover_shots: HoverShots,
    /// Icônes déjà demandées en avance pour les tuiles visibles.
    hover_prefetched: HashSet<u32>,
    /// Début du survol courant : la capsule reste seule quelques secondes
    /// avant de céder la place aux captures.
    hover_since: Option<(u32, Instant)>,
    /// Jaquette d'un jeu fraîchement obtenu, tant qu'elle voyage vers
    /// l'onglet Bibliothèque.
    game_reveal: Option<GameReveal>,
    /// Icône carrée du jeu révélé, cherchée une seule fois : la célébration
    /// la préfère à la jaquette.
    reveal_icon: Option<(u32, Option<egui::TextureHandle>)>,
    /// Activation dont le jeu n'est pas encore identifié.
    reveal_pending: Option<PendingReveal>,
    /// Dernier verdict d'activation de clé lu par l'application.
    redeem_seen: Option<(String, String, String, String)>,
    /// Visuels larges de la scène, chargés une fois pour l'écran d'attente qui
    /// précède l'ouverture de GameScope.
    launch_backdrops: HashMap<u32, Option<egui::TextureHandle>>,
    art: ArtCache,
    /// Fiche publiée par la boutique (description, captures), lue quand la
    /// fiche d'un jeu de boutique se déplie.
    store_info: StoreInfo,
    /// Poids des caches et bouton pour les vider (Réglages › Application).
    cache_usage: CacheUsage,
    /// Poids publié des jeux de boutique (bouton Installer et étiquettes).
    store_sizes: StoreSizes,
    /// Protection lue dans le dossier d'un jeu de boutique installé : un scan
    /// par dossier, gardé tant que la fiche le montre.
    drm_reports: HashMap<u32, (PathBuf, Option<drm::Report>)>,
    /// Poids des contenus additionnels d'un jeu de boutique, demandés à
    /// l'ouverture du panneau puis gardés.
    store_dlc_sizes: HashMap<u32, Vec<DlcSize>>,
    store_dlc_sizes_rx: Option<mpsc::Receiver<(u32, Vec<DlcSize>)>>,
    /// Contenus de boutique retenus pour le téléchargement.
    store_dlc_pick: HashMap<u32, HashSet<String>>,
    /// Résultat d'une demande de téléchargement de contenus, dit une fois.
    store_dlc_install_rx: Option<mpsc::Receiver<String>>,
    /// Vignettes des contenus additionnels, par appid : leur bannière Steam,
    /// téléchargée à la première ouverture du panneau puis gardée.
    dlc_art: HashMap<u32, Option<egui::TextureHandle>>,
    /// Dernière demande de bannière par contenu, pour ne pas marteler le CDN.
    dlc_art_at: HashMap<u32, Instant>,
    /// Contenus dont la boutique a déjà été interrogée : sa limite d'appels est
    /// basse, la question ne se repose pas.
    dlc_store: HashSet<u32>,
    cover_picker: Option<CoverPicker>,
    /// Icône carrée du jeu dont le menu de jaquette est ouvert. Le menu se
    /// redessine à chaque image alors que le pointeur le survole : relire
    /// l'icône depuis le disque à chaque fois coûterait une recherche de
    /// fichiers par tuile affichée.
    cover_menu_icon: Option<(u32, Option<egui::TextureHandle>)>,
    logo: Option<Logo>,
    /// Le mot « Lapinux » écrit à la main : il remplace le texte dans la barre
    /// de fenêtre, centré au-dessus des commandes.
    wordmark: Option<Logo>,
    /// Le lapin d'attente, joué pendant un lancement qui traîne.
    bunny: Option<launch::Bunny>,
    /// Lancement en cours : la requête vit en tâche de fond.
    launching: Option<Launching>,
    /// Écran d'attente en train de s'effacer après la réponse du lancement.
    launch_fade: Option<LaunchFade>,
    /// Attente de la première image du jeu, après la réponse du daemon.
    launch_hold: Option<LaunchHold>,
    /// Pendant l'attente, la session est relue plus vite que le rafraîchissement
    /// général : le signal de première image doit lever l'écran en moins d'une
    /// seconde, pas trois secondes après que le jeu soit visible.
    splash_poll: Option<mpsc::Receiver<Option<SessionInfo>>>,
    splash_poll_at: Instant,
    /// Dernière lecture de la demande de lancement déposée par le tray.
    launch_request_at: Instant,
    nav: Option<lapinux_input::NavReader>,
    big_picture: bool,
    /// Dernier état connu de la fenêtre agrandie (barre de fenêtre maison).
    maximized: bool,
    grid_width: f32,
    startup: Startup,
    last_refresh: Instant,
    refresh_rx: Option<mpsc::Receiver<UiSnapshot>>,
    /// Une relecture a été demandée pendant qu'une autre était en vol :
    /// elle repart dès que la réponse courante est appliquée.
    refresh_queued: bool,
    /// Section Profil : profil public du compte Steam, lu par le réseau.
    profile: Profile,
    /// État social publié par le daemon : présence, amis, famille,
    /// notifications.
    social: Option<SteamSocial>,
    /// Instant de la dernière famille réellement lue : un snapshot vivant
    /// peut précéder sa relecture pendant une reconnexion.
    family_seen_at: Option<Instant>,
    social_poll: Instant,
    login_poll: Instant,
    login_since: Option<Instant>,
    /// Dernière demande de vérification du jeton enregistré : elle est évitée
    /// tant que le verdict n'a pas bougé.
    token_check_at: Option<Instant>,
    steam_status: Option<SteamStatus>,
    steam_user: String,
    steam_password: String,
    steam_token: String,
    steam_guard: String,
    /// QR code de la connexion Steam en cours, régénéré seulement quand le
    /// défi change.
    steam_qr: Option<QrView>,
    settings_open: bool,
    /// Remet le défilement en haut à la prochaine ouverture ou au prochain
    /// changement d'onglet : une page raccourcie ne s'ouvre pas au milieu.
    settings_scroll_reset: bool,
    settings_tab: SettingsTab,
    /// Catégories du rail repliées : Essentiel, Jeux & données, Système.
    settings_collapsed: [bool; 3],
    settings_draft: Option<Config>,
    /// Dernière release connue et téléchargement de mise à jour en cours.
    update_ui: UpdateUiState,
    /// Panneau Amis : la liste réunie des amis Steam, Epic, GOG et itch.io.
    friends_open: bool,
    /// Ce que chaque boutique a répondu pour sa liste d'amis.
    store_friends: Vec<StoreFriends>,
    /// Lecture des amis de boutique en cours.
    friends_rx: Option<mpsc::Receiver<Option<Vec<StoreFriends>>>>,
    friends_loading: bool,
    /// Dernière demande des listes de boutique pour l'aperçu de l'accueil.
    friends_last_refresh: Option<Instant>,
    /// La dernière lecture de boutique a réussi (les anciennes présences peuvent sinon être périmées).
    friends_store_available: bool,
    /// Filtre de boutique du panneau (`None` = toutes).
    friends_store: Option<lapinux_core::stores::Store>,
    friends_search: String,
    /// Ami dont la fiche est ouverte dans le panneau (`None` = la liste).
    friends_selected: Option<String>,
    /// Ami dont la conversation est ouverte dans le panneau.
    friends_chat: Option<String>,
    /// Brouillon du message écrit dans la conversation avec un ami.
    friends_message: String,
    /// La fiche ou la conversation vient de s'ouvrir : on la replace en haut
    /// une seule fois.
    friends_profile_top: bool,
    /// Le haut de la conversation a été atteint : la page précédente est
    /// demandée une seule fois par remontée.
    friends_history_top_armed: bool,
    /// La conversation suit le bas tant que l'utilisateur n'est pas remonté.
    friends_chat_autoscroll: bool,
    /// Demande un défilement unique tout en bas (ouverture ou envoi).
    friends_chat_scroll_to_bottom: bool,
    /// Photos des amis, par clé (`steam:<id>`, `epic:<id>`…).
    friends_avatars: HashMap<String, Option<egui::TextureHandle>>,
    friends_avatar_pending: HashSet<String>,
    friends_avatar_tx: Option<mpsc::Sender<(String, Vec<u8>)>>,
    friends_avatar_rx: Option<mpsc::Receiver<(String, Vec<u8>)>>,
    /// Images des emotes Steam, par nom ; `None` pour une image illisible.
    emote_textures: HashMap<String, Option<egui::TextureHandle>>,
    emote_pending: HashSet<String>,
    emote_tx: Option<mpsc::Sender<(String, Vec<u8>)>>,
    emote_rx: Option<mpsc::Receiver<(String, Vec<u8>)>>,
    /// Animations des autocollants Steam, par nom ; `None` pour une image
    /// illisible.
    sticker_textures: HashMap<String, Option<StickerAnimation>>,
    sticker_pending: HashSet<String>,
    sticker_tx: Option<mpsc::Sender<(String, Option<StickerFrames>)>>,
    sticker_rx: Option<mpsc::Receiver<(String, Option<StickerFrames>)>>,
    /// Le sélecteur d'emotes est-il déplié sous la conversation ?
    friends_emotes_open: bool,
    /// L'onglet Autocollants du sélecteur est-il actif ?
    friends_picker_stickers: bool,
    /// Import d'un ancien export JSON dans l'archive locale.
    friends_import_open: bool,
    friends_import_path: String,
    friends_import_note: Option<String>,
    /// Tri et totaux de la page Stockage, recalculés après un scan seulement.
    storage: StorageCache,
    /// La page Stockage inclut-elle les jeux qui ne sont pas installés ?
    /// Écrans détectés, rafraîchis à l'ouverture des réglages.
    displays: Vec<lapinux_core::display::DisplayInfo>,
    steam_login_state: SteamLoginState,
    logs: Option<LogsView>,
    logs_file: Option<String>,
    logs_error: Option<String>,
    logs_poll: Instant,
    catalog: Option<SteamCatalog>,
    /// Version des données de la bibliothèque : incrémentée quand le scan, le
    /// catalogue ou les boutiques changent, pour ne reconstruire la liste
    /// fusionnée qu'à ce moment-là.
    library_epoch: u64,
    library_cache: RefCell<Option<LibraryCache>>,
    /// Vue filtrée et triée gardée entre deux images : la refaire à chaque
    /// image coûtait plusieurs millisecondes sur une grande collection.
    library_view_cache: RefCell<Option<Rc<LibraryView>>>,
    /// Les premières rangées ont déjà lancé leurs jaquettes. Le préchargement
    /// ne repart qu'après un changement de bibliothèque ou un vidage du cache.
    library_covers_warmed: bool,
    /// Instant jusqu'auquel la grille redemande des rendus pour les tuiles
    /// visibles encore sans jaquette : le budget de décodage les répartit, mais
    /// sans ces passes la fenêtre s'endort avant l'arrivée des téléchargements.
    library_fill_until: Option<Instant>,
    /// Le défilement était actif à l'image précédente : son arrêt relance une
    /// courte passe de remplissage sur les tuiles qui viennent d'apparaître.
    library_was_scrolling: bool,
    catalog_filter: CatalogFilter,
    sources: SourceFilter,
    catalog_loaded: bool,
    /// Lecture du catalogue demandée, pas encore lancée.
    catalog_want: Option<bool>,
    /// Jeu d'une clé activée à faire entrer au catalogue : appid (0 si le reçu
    /// ne le donne pas) et nom.
    catalog_patch: Option<(u32, String)>,
    /// Célébration en attente de sa jaquette : elle part dès que l'image est
    /// arrivée, ou au bout de deux secondes pour ne pas la faire attendre.
    reveal_wait: Option<(u32, String, Instant)>,
    /// Jaquette de la célébration, lue dans le cache sans passer par l'état
    /// du cache de la grille.
    reveal_cover: Option<(u32, egui::TextureHandle)>,
    /// Célébration en attente : une clé activée pendant l'animation de la
    /// précédente attend son tour au lieu d'être perdue.
    reveal_queue: Option<(u32, String)>,
    /// Lecture du catalogue en vol : elle arrive par un fil.
    catalog_rx: Option<mpsc::Receiver<(bool, Result<SteamCatalog, String>)>>,
    catalog_poll: Instant,
    catalog_was_syncing: bool,
    /// Synchronisation du catalogue tentée une fois l'application prête : un
    /// jeu acheté depuis le dernier lancement doit apparaître sans passer par
    /// les réglages, comme dans Steam.
    catalog_boot_synced: bool,
    /// Fraîcheur du cache du catalogue vérifiée : une relecture automatique
    /// part au plus une fois par lancement.
    catalog_freshness_checked: bool,
    /// La relecture en cours vient de la vérification de fraîcheur : elle ne
    /// mérite pas le message d'une synchronisation demandée à la main.
    catalog_auto: bool,
    /// La synchronisation en cours a été demandée à la main : elle mérite un
    /// message à la fin, contrairement à celle du lancement.
    catalog_notice: bool,
    system_sampler: SystemSampler,
    system: SystemInfo,
    pad_monitor: Option<lapinux_input::PadMonitor>,
    /// Onglet ouvert dans Manettes : réglages généraux ou appareils.
    controllers_tab: ControllersTab,
    /// Manette sélectionnée dans la liste de la page Manettes ; son test
    /// s'affiche dans la colonne de droite. La clé est le chemin evdev, ou
    /// l'adresse Bluetooth pour une manette appairée mais hors ligne.
    pad_selected: Option<String>,
    /// Saisie de renommage de la manette sélectionnée.
    pad_rename: Option<PadRename>,
    /// Sections « En ligne » et « Appairées » de la liste, repliées ou non.
    pads_collapsed: [bool; 2],
    /// Carte Informations de la fiche : repliée au premier affichage.
    pad_info_open: bool,
    /// Réglages et retours des tests matériels de la manette.
    pad_tests: PadTestState,
    /// Manette à laquelle appartiennent les tests. La clé est conservée
    /// pendant une disparition temporaire d'evdev : un accès hidraw retire
    /// brièvement le nœud, mais ne doit pas effacer le relevé ni les réglages.
    pad_tests_for: Option<String>,
    /// Instantané de la manette sélectionnée, réutilisé le temps d'un relevé
    /// hidraw pour que la page ne se vide pas.
    pad_cache: Option<PadCache>,
    /// Extinction ou connexion en cours, hors du fil d'affichage.
    pad_power: Option<PadPower>,
    /// Sortie, entrée et prise casque de la manette sélectionnée.
    pad_audio: Option<PadAudio>,
    /// Manettes Bluetooth connues de BlueZ, y compris hors ligne.
    paired_pads: Option<Vec<DeviceInfo>>,
    /// Requête BlueZ en cours, hors du fil de l'interface.
    paired_rx: Option<mpsc::Receiver<Vec<DeviceInfo>>>,
    /// Chemins evdev vus au dernier rafraîchissement : un branchement ou un
    /// débranchement relance la liste des manettes appairées.
    pad_evdev_paths: Vec<PathBuf>,
    /// Dongle Steam Controller 2026 repéré en USB, mais pas encore exposé
    /// comme gamepad par le noyau.
    steam_puck_present: bool,
    /// Pont userspace actif : la carte d'aide n'a plus lieu d'être.
    steam_puck_bridge: bool,
    /// `hid-steam` garde la 2026 en mode clavier/souris : le pad natif existe
    /// mais reste muet tant que `lizard_mode` vaut Y.
    steam_lizard_mode: Option<bool>,
    /// Le processus peut écrire `lizard_mode` (règle udev installée).
    steam_lizard_writable: bool,
    /// Dernière erreur du changement de mode lizard, affichée dans la fiche.
    steam_lizard_note: Option<String>,
    /// Installation de la règle udev en cours (pkexec).
    steam_lizard_install: Option<mpsc::Receiver<Result<(), String>>>,
    /// `99-steam-controller.conf` installé : natif au prochain démarrage.
    steam_lizard_persistent: bool,
    /// Installation de la configuration modprobe en cours (pkexec).
    steam_lizard_persist: Option<mpsc::Receiver<Result<(), String>>>,
    steam_puck_poll: Instant,
    /// Service utilisateur du pont Steam Controller : état, relance, fichier.
    steam_bridge: steam_bridge::SteamBridge,
    system_poll: Instant,
    disk: Option<DiskUsage>,
    disk_poll: Instant,
    /// État de la page Téléchargements : dossier, caches, débits lissés,
    /// mises à jour et test de débit. Voir `downloads`.
    downloads: Downloads,
    /// Comptes et bibliothèques des boutiques hors Steam (réglages › Comptes).
    stores: stores::Stores,
    /// Le site de la boutique, rendu hors écran et peint dans la page.
    webview: webview::Webview,
    /// Lecteur de bande-annonce intégré à l'interface.
    trailer: trailer::Trailer,
    /// Section **Boutiques › Jeux gratuits** : les offres du moment chez Epic.
    free: freegames::FreeGames,
    /// Page Actualités : mises à jour, soldes et actus du gaming.
    news_feed: news::Page,
    /// Bibliothèque dont `news_games` est tiré : la liste n'est reconstruite
    /// que lorsque ce cache change, pas à chaque image.
    news_entries: Option<Rc<Vec<LibraryEntry>>>,
    /// Jeux du flux d'actualités, installés et non installés alternés.
    news_games: Vec<news::GameSource>,
    /// Une offre a été renvoyée vers la boutique : à la fermeture de la
    /// fenêtre, la liste se relit pour confirmer le droit obtenu.
    free_watch: bool,
    /// La page Boutiques montre-t-elle les jeux gratuits plutôt qu'un site ?
    free_open: bool,
    jobs_poll: Instant,
    /// Réponse des tâches en vol : la lecture arrive par un fil, jamais par un
    /// échange bloquant pendant la peinture.
    jobs_rx: Option<mpsc::Receiver<Option<Vec<Job>>>>,
    /// Partie en cours relue chaque seconde : la pastille du jeu affiche sa
    /// cadence vivante, sans attendre le rafraîchissement général de 3 s.
    session_poll: Instant,
    session_rx: Option<mpsc::Receiver<Option<SessionInfo>>>,
    busy: bool,
}

/// Requête IPC courte pour les données d'affichage : contrairement à une
/// action utilisateur, un rafraîchissement ne doit jamais immobiliser l'UI.
fn ui_request<T: serde::de::DeserializeOwned>(client: &Client, req: &Req) -> Option<T> {
    client
        .request_with_timeout(req, Duration::from_secs(10))
        .ok()
        .and_then(|envelope| envelope.into_data().ok())
}

/// Fichier de l'historique des événements d'amis, à côté des autres données
/// locales de Lapinux.
fn friend_events_path(paths: &LapinuxPaths) -> std::path::PathBuf {
    paths.data_dir.join("friend-events.json")
}

/// Clé stable d'une notification d'ami, préfixée pour ne pas entrer en
/// collision avec les compteurs Steam dans la liste des suppressions.
fn friend_notification_key(event: &SteamFriendEvent) -> String {
    format!("friend:{}", friend_event_key(event))
}

/// Clé stable d'un compteur Steam. Le nombre et l'heure changent quand Steam
/// publie une nouvelle notification : une carte supprimée ne masque donc pas
/// les suivantes.
fn notification_counter_key(notification: &lapinux_core::social::SteamNotification) -> String {
    format!(
        "steam-counter:{}:{}:{}",
        notification.kind, notification.at, notification.count
    )
}

/// Clé stable d'une notification Web Steam. Le titre et l'adresse distinguent
/// deux événements publiés à la même seconde.
fn notification_web_key(notification: &lapinux_core::social::SteamWebNotification) -> String {
    format!(
        "steam-web:{}:{}:{}",
        notification.kind, notification.at, notification.url
    )
}

/// Clé stable d'un événement : un même événement ne produit qu'un toast et
/// une seule entrée d'historique.
fn friend_event_key(event: &SteamFriendEvent) -> String {
    // Ni le nom du jeu ni l'appid n'entrent dans la clé : le daemon peut les
    // compléter après coup (surtout pour un « jeu quitté »), et l'événement ne
    // doit pas être notifié ou réapparaître deux fois pour autant.
    format!("{}:{:?}:{}", event.at, event.kind, event.steam_id)
}

/// Clé stable d'un message reçu : un même message ne produit qu'un toast,
/// même si plusieurs instantanés sociaux le répètent.
fn steam_message_key(friend: u64, message: &lapinux_core::social::SteamChatMessage) -> String {
    format!(
        "{}:{}:{}:{}",
        friend, message.from, message.at, message.text
    )
}

/// Aperçu court du message pour le toast : espaces normalisés et taille
/// bornée, sans jamais couper au milieu d'un caractère UTF-8.
fn message_notification_preview(text: &str) -> String {
    let mut preview = text
        .replace("[emoticon]", ":")
        .replace("[/emoticon]", ":")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if preview.chars().count() > 80 {
        preview = preview.chars().take(77).collect::<String>();
        preview.push('…');
    }
    preview
}

fn friend_events_load(paths: &LapinuxPaths) -> VecDeque<SteamFriendEvent> {
    let Ok(text) = std::fs::read_to_string(friend_events_path(paths)) else {
        return VecDeque::new();
    };
    let Ok(mut events) = serde_json::from_str::<Vec<SteamFriendEvent>>(&text) else {
        return VecDeque::new();
    };
    events.sort_by(|a, b| b.at.cmp(&a.at));
    // Deux versions d'un même événement peuvent coexister sur disque : garder
    // celle qui porte le nom du jeu, et compléter l'avatar manquant, désormais
    // que la clé ne contient plus le titre.
    let mut merged: Vec<SteamFriendEvent> = Vec::new();
    for event in events {
        let key = friend_event_key(&event);
        if let Some(existing) = merged
            .iter_mut()
            .find(|existing| friend_event_key(existing) == key)
        {
            if existing.game.trim().is_empty() && !event.game.trim().is_empty() {
                existing.game = event.game.clone();
            }
            if existing.avatar.trim().is_empty() && !event.avatar.trim().is_empty() {
                existing.avatar = event.avatar.clone();
            }
        } else {
            merged.push(event);
        }
    }
    merged.truncate(100);
    merged.into()
}

fn load_friend_event_state(paths: &LapinuxPaths) -> (VecDeque<SteamFriendEvent>, HashSet<String>) {
    let history = friend_events_load(paths);
    let seen = history.iter().map(friend_event_key).collect();
    (history, seen)
}

fn friend_events_save(paths: &LapinuxPaths, events: &VecDeque<SteamFriendEvent>) {
    let Ok(body) = serde_json::to_vec_pretty(events) else {
        return;
    };
    let path = friend_events_path(paths);
    let Ok(()) = std::fs::create_dir_all(&paths.data_dir) else {
        return;
    };
    let temporary = path.with_extension("json.tmp");
    if std::fs::write(&temporary, body).is_ok() {
        let _ = std::fs::rename(&temporary, &path);
    }
}

const NOTIFICATION_SECONDS_PER_DAY: i64 = 86_400;

fn notification_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

fn notification_cutoff(notifications: &lapinux_core::config::NotificationsConfig, now: i64) -> i64 {
    now - i64::from(notifications.retention_days.max(1)) * NOTIFICATION_SECONDS_PER_DAY
}

fn notification_is_recent(at: i64, cutoff: i64) -> bool {
    at <= 0 || at >= cutoff
}

/// Retire de l'historique les événements plus vieux que la durée choisie.
fn prune_old_friend_events(history: &mut VecDeque<SteamFriendEvent>, cutoff: i64) -> bool {
    let before = history.len();
    history.retain(|event| notification_is_recent(event.at, cutoff));
    history.len() != before
}

/// Fichier de la photographie des bibliothèques, à côté des autres données
/// locales de Lapinux.
fn library_seen_path(paths: &LapinuxPaths) -> std::path::PathBuf {
    paths.data_dir.join("library-seen.json")
}

fn library_seen_load(paths: &LapinuxPaths) -> Option<SeenLibrary> {
    let text = std::fs::read_to_string(library_seen_path(paths)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Jeux arrivés depuis la photographie précédente. L'appid suffit presque
/// toujours ; le titre rattrape les jeux dont la boutique a changé
/// l'identifiant. Epic réattribue parfois un `appName` au même jeu, et Botany
/// Manor revenait ainsi à chaque lancement comme s'il venait d'arriver.
fn new_library_entries(
    entries: &[LibraryEntry],
    previous: Option<&SeenLibrary>,
) -> Vec<LibraryEntry> {
    let Some(previous) = previous else {
        // Premier lancement : tout est la référence, rien n'est nouveau.
        return Vec::new();
    };
    let known: HashSet<u32> = previous.games.iter().map(|game| game.appid).collect();
    let known_titles: HashSet<String> = previous
        .games
        .iter()
        .map(|game| lapinux_core::stores::title_key(&game.name))
        .filter(|title| !title.is_empty())
        .collect();
    entries
        .iter()
        .filter(|entry| !known.contains(&entry.appid))
        .filter(|entry| {
            let title = lapinux_core::stores::title_key(&entry.name);
            title.is_empty() || !known_titles.contains(&title)
        })
        .filter(|entry| !is_steam_tool(&entry.name))
        .cloned()
        .collect()
}

/// Nombre de polices système ajoutées au plus : chacune est lue et analysée
/// au démarrage, la liste reste donc courte.
const MAX_FALLBACK_FONTS: usize = 8;
/// Taille maximale d'une police de repli : elle doit couvrir les écritures
/// asiatiques sans avaler une collection entière de plusieurs dizaines de Mo.
const MAX_FALLBACK_BYTES: u64 = 24 * 1024 * 1024;

/// Fichiers de police, du plus utile au moins utile : symboles et écritures
/// que les polices latines ne couvrent pas, puis les familles à large
/// couverture. Chaque entrée est un morceau de nom de fichier, en minuscules.
const FALLBACK_PATTERNS: &[&[&str]] = &[
    // Émojis récents, absents de la police embarquée par le moteur d'interface.
    &["notoemoji"],
    // Symboles, dingbats, ponctuation exotique : les noms Steam en sont pleins.
    &["notosanssymbols2", "notosanssymbols-2"],
    &["symbola"],
    &["notosanssymbols"],
    &["notosansmath"],
    // Écritures absentes des polices latines (syllabaire autochtone, etc.).
    &["notosanscanadianaborigin"],
    // Noms japonais, chinois et coréens : courants sur Steam. Une seule de
    // ces familles suffit, la première installée gagne.
    &[
        "notosanscjk",
        "notosansjp",
        "notosanssc",
        "notosanstc",
        "notosanskr",
        "wqy",
        "droidsansfallback",
        "uming",
        "ukai",
    ],
    // Familles à très large couverture, quand elles sont installées.
    &["dejavusans", "dejavuserif"],
    &["unifont"],
    &["freesans", "freeserif", "freefont"],
    // Latin, grec et cyrillique élargis.
    &["notosans-regular", "notosans[", "notosans.ttf"],
];

/// Dossiers où chercher les polices du système : ceux de la distribution et
/// ceux de l'utilisateur.
fn font_directories() -> Vec<PathBuf> {
    let mut directories = vec![
        PathBuf::from("/usr/share/fonts"),
        PathBuf::from("/usr/local/share/fonts"),
        PathBuf::from("/usr/share/X11/fonts"),
    ];
    let data_home = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .ok()
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|home| PathBuf::from(home).join(".local/share"))
        });
    if let Some(data_home) = data_home {
        directories.push(data_home.join("fonts"));
    }
    if let Ok(home) = std::env::var("HOME") {
        directories.push(PathBuf::from(home).join(".fonts"));
    }
    directories
}

/// Parcourt un dossier de polices sans descendre à l'infini.
fn collect_font_files(directory: &Path, depth: usize, found: &mut Vec<PathBuf>) {
    if depth > 5 || found.len() > 4000 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_font_files(&path, depth + 1, found);
            continue;
        }
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if matches!(extension.as_str(), "ttf" | "otf" | "ttc") {
            found.push(path);
        }
    }
}

/// Vrai si le fichier est une police que le moteur de texte sait lire : une
/// police illisible fait échouer le rendu, elle est donc écartée ici.
fn readable_font(bytes: &[u8]) -> bool {
    ab_glyph::FontRef::try_from_slice_and_index(bytes, 0).is_ok()
}

/// Graisse d'une police de repli : le texte courant d'abord, les graisses
/// extrêmes en dernier (un nom Steam ne doit pas s'afficher en très gras).
fn fallback_weight_rank(path: &Path) -> u8 {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if name.contains("regular") || name.contains("[wght]") {
        0
    } else if [
        "black",
        "bold",
        "italic",
        "light",
        "thin",
        "demilight",
        "semibold",
    ]
    .iter()
    .any(|weight| name.contains(weight))
    {
        2
    } else {
        1
    }
}

/// Vrai si le fichier répond à l'un des motifs (noms de fichiers, minuscules).
///
/// Les polices de couleur sont écartées : le moteur de texte ne sait tracer
/// que des contours, une police COLR ou bitmap sortirait vide.
fn matches_fallback_pattern(path: &Path, patterns: &[&str]) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if name.contains("color") || name.contains("colr") {
        return false;
    }
    patterns.iter().any(|pattern| name.contains(pattern))
}

/// Choix des polices : une par motif, dans l'ordre de [`FALLBACK_PATTERNS`].
/// `read` rend le contenu d'un fichier exploitable, ou `None` s'il est vide,
/// trop gros ou illisible : la main passe alors au fichier suivant du motif.
fn pick_fallback_fonts(
    files: &[PathBuf],
    mut read: impl FnMut(&Path) -> Option<Vec<u8>>,
) -> Vec<(String, Vec<u8>)> {
    let mut chosen: Vec<(String, Vec<u8>)> = Vec::new();
    let mut used: HashSet<&Path> = HashSet::new();
    for patterns in FALLBACK_PATTERNS {
        if chosen.len() >= MAX_FALLBACK_FONTS {
            break;
        }
        for path in files {
            if used.contains(path.as_path()) || !matches_fallback_pattern(path, patterns) {
                continue;
            }
            let Some(bytes) = read(path) else {
                continue;
            };
            used.insert(path.as_path());
            chosen.push((path.display().to_string(), bytes));
            break;
        }
    }
    chosen
}

/// Lit une police du disque en écartant ce que le moteur de texte refuserait :
/// un fichier vide, trop gros ou illisible ferait échouer le rendu.
fn read_system_font(path: &Path) -> Option<Vec<u8>> {
    let metadata = std::fs::metadata(path).ok()?;
    if metadata.len() == 0 || metadata.len() > MAX_FALLBACK_BYTES {
        tracing::debug!(
            "police de repli ignorée ({} octets) : {}",
            metadata.len(),
            path.display()
        );
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    if !readable_font(&bytes) {
        tracing::debug!("unreadable font, ignored: {}", path.display());
        return None;
    }
    Some(bytes)
}

/// Polices de repli trouvées sur le système, lues et vérifiées.
fn system_fallback_fonts() -> Vec<(String, Vec<u8>)> {
    let mut files = Vec::new();
    for directory in font_directories() {
        collect_font_files(&directory, 0, &mut files);
    }
    // L'ordre des dossiers n'est pas garanti : trier rend le choix identique
    // d'un démarrage à l'autre, et la graisse courante passe devant.
    files.sort();
    files.dedup();
    files.sort_by(|a, b| fallback_weight_rank(a).cmp(&fallback_weight_rank(b)));
    pick_fallback_fonts(&files, read_system_font)
}

/// Installe les polices de repli du système derrière celles du produit : les
/// noms de joueurs contiennent des écritures et des symboles que ni Inter ni
/// les polices embarquées ne couvrent. Rend leur nom et leur rôle : une
/// police d'émojis n'a pas la même place dans la chaîne qu'une police de
/// texte.
fn add_system_fallbacks(fonts: &mut egui::FontDefinitions) -> Vec<(String, bool)> {
    let mut names = Vec::new();
    for (index, (file, bytes)) in system_fallback_fonts().into_iter().enumerate() {
        let name = format!("system-{index}");
        tracing::debug!("fallback font {name}: {file}");
        let emoji = file.to_ascii_lowercase().contains("emoji");
        // Les octets vivent aussi longtemps que le contexte : les emprunter
        // évite une seconde copie, qui pèserait lourd pour une police CJK.
        let bytes: &'static [u8] = Box::leak(bytes.into_boxed_slice());
        let data = egui::FontData::from_static(bytes);
        // Un émoji monochrome tracé à la taille du texte paraît plus petit
        // que la lettre voisine : un léger agrandissement le rend lisible.
        let data = if emoji {
            data.tweak(egui::FontTweak {
                scale: 1.18,
                ..Default::default()
            })
        } else {
            data
        };
        fonts.font_data.insert(name.clone(), data);
        names.push((name, emoji));
    }
    names
}

/// Police du produit : Inter Medium remplace la police fine par défaut,
/// et Inter SemiBold sert aux titres. Les deux fichiers sont embarqués avec
/// leur licence OFL dans `assets/fonts`.
pub(crate) fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let fallbacks = fonts
        .families
        .get(&egui::FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    fonts.font_data.insert(
        "inter-medium".to_owned(),
        egui::FontData::from_static(include_bytes!("../../../assets/fonts/Inter-Medium.ttf")),
    );
    fonts.font_data.insert(
        "inter-semibold".to_owned(),
        egui::FontData::from_static(include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf")),
    );
    fonts.font_data.insert(
        "remix-icons".to_owned(),
        egui::FontData::from_static(include_bytes!("../../../assets/fonts/RemixIcons.ttf")),
    );
    fonts.families.insert(
        egui::FontFamily::Name("Icons".into()),
        vec!["remix-icons".to_owned()],
    );
    let mut emoji = Vec::new();
    let mut others = Vec::new();
    for (name, is_emoji) in add_system_fallbacks(&mut fonts) {
        if is_emoji {
            emoji.push(name);
        } else {
            others.push(name);
        }
    }
    let fallbacks = fallback_chain(fallbacks, emoji.clone(), others.clone());
    let mut proportional = vec!["inter-medium".to_owned()];
    proportional.extend(fallbacks.clone());
    fonts
        .families
        .insert(egui::FontFamily::Proportional, proportional);
    if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
        family.extend(emoji.iter().cloned());
        family.extend(others.iter().cloned());
    }
    let mut display = vec!["inter-semibold".to_owned()];
    display.extend(fallbacks);
    fonts
        .families
        .insert(egui::FontFamily::Name("Display".into()), display);
    ctx.set_fonts(fonts);
}

/// Chaîne de repli d'une famille : les polices de texte du moteur, les émojis
/// de la machine — plus récents et plus nets que ceux du moteur, qui datent —
/// puis ceux du moteur, puis les autres écritures trouvées sur le système.
/// Les polices de repli non-émoji restent derrière les polices de texte.
fn fallback_chain(base: Vec<String>, emoji: Vec<String>, others: Vec<String>) -> Vec<String> {
    let mut chain = base;
    let position = chain
        .iter()
        .position(|name| name.to_ascii_lowercase().contains("emoji"))
        .unwrap_or(chain.len());
    chain.splice(position..position, emoji);
    chain.extend(others);
    chain
}

pub(crate) fn display_family() -> egui::FontFamily {
    egui::FontFamily::Name("Display".into())
}

pub(crate) fn display_font(size: f32) -> egui::FontId {
    egui::FontId::new(size, display_family())
}

impl LapinuxApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        apply_theme(&cc.egui_ctx);
        install_fonts(&cc.egui_ctx);
        let paths = LapinuxPaths::discover();
        let news_cache_path = paths.cache_dir.join("news-feed.json");
        // Un réveil de fin laissé par une session précédente ne doit pas
        // faire tourner le fil de surveillance pour rien.
        let _ = std::fs::remove_file(paths.end_request());
        let client = Client::new(&paths);
        // Un daemon resté sur un binaire antérieur effacerait les réglages
        // qu'il ne connaît pas (un réglage tout neuf, par exemple) : il est
        // remplacé avant la première lecture de la configuration.
        client.restart_stale_daemon();
        // Le fichier fait foi au démarrage : interroger le daemon ici
        // bloquerait la première image le temps qu'il démarre et scanne.
        let config = Config::load(&paths.config_file).unwrap_or_default();
        lapinux_core::i18n::set(&config.general.lang);
        // Les manifestes sont lus après la configuration : le thème actif et
        // les plugins cochés peuvent venir d'une version précédente.
        let theme_library = theme::ThemeLibrary::load(paths.themes_dir());
        let plugin_library = theme::PluginLibrary::load(paths.plugins_dir());
        let active_plugins = plugin_library.active(&config.general.active_plugins);
        let active_theme = theme_library.resolve(&config.general.theme, &active_plugins);
        apply_active_theme(&cc.egui_ctx, &active_theme);
        // Quel moteur dessine réellement la fenêtre : un repli logiciel se paie
        // en processeur comme en mémoire, et il est invisible à l'œil.
        if let Some(gl) = cc.gl.as_ref() {
            use eframe::glow::HasContext as _;
            // SAFETY: requête GL faite sous le contexte courant, comme eframe.
            let (vendor, renderer) = unsafe {
                (
                    gl.get_parameter_string(eframe::glow::VENDOR),
                    gl.get_parameter_string(eframe::glow::RENDERER),
                )
            };
            eprintln!("rendu : {vendor} · {renderer}");
        }
        // Une demande de lancement déposée par le tray doit réveiller
        // l'interface même quand elle est rangée derrière une partie : le fil
        // de peinture est alors à l'arrêt et ne lirait jamais le fichier.
        let launch_ctx = cc.egui_ctx.clone();
        let launch_paths = paths.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(250));
            // Le fil de peinture est à l'arrêt quand la fenêtre est rangée :
            // ce réveil suffit à faire créer le splash, sans ramener la
            // bibliothèque au premier plan. Le second fichier réveille la
            // même boucle à la fermeture du jeu, pour que l'écran de fin ne
            // laisse pas s'écouler son minuteur dans le noir.
            if launch_paths.launch_request().is_file() || launch_paths.end_request().is_file() {
                launch_ctx.request_repaint();
            }
        });
        let big_picture = config.general.big_picture;
        let grid_width = config.general.grid_width as f32;
        let library_sort = LibrarySort::from_config(config.library.sort);
        // Les configurations écrites avant que le sens du tri ne soit
        // enregistré portent `false` par défaut : on repart du sens naturel du
        // tri, puis le choix réel est enregistré au premier changement.
        let library_descending = if config.library.sort_direction_chosen {
            config.library.sort_descending
        } else {
            library_sort.default_descending()
        };
        let library_sources = SourceFilter::from_config(config.library.sources);
        let library_filter = CatalogFilter::from_config(config.library.filter);
        let steam_user = config.steam.username.clone();
        let artwork_key = config.artwork.steamgriddb_key.clone();
        let artwork_enabled = config.artwork.steamgriddb;
        let library_seen = library_seen_load(&paths);
        let disk = lapinux_core::system::disk_usage(&config.library_root());
        let mut system_sampler = SystemSampler::new();
        let system = system_sampler.sample();
        // winit n'applique pas « toujours au-dessus » sous Wayland : KWin
        // s'en charge pour la fenêtre d'attente, reconnue à son titre.
        if let Err(err) = lapinux_input::shortcut::keep_splash_above(&config.launch.output) {
            eprintln!("waiting screen: {err}");
        }
        // Un lancement manuel doit se voir : sous Wayland, la nouvelle
        // fenêtre peut rester derrière le bureau ou hériter d'un état réduit.
        // Une demande tray, elle, a déjà son splash et ne doit pas ramener la
        // bibliothèque au premier plan.
        if !paths.launch_request().is_file() {
            let _ = lapinux_input::shortcut::reveal_lapinux();
        }
        let (mut friend_events_history, friend_events_seen) = load_friend_event_state(&paths);
        let cutoff = notification_cutoff(&config.notifications, notification_now());
        if prune_old_friend_events(&mut friend_events_history, cutoff) {
            friend_events_save(&paths, &friend_events_history);
        }
        let art_cache = ArtCache::new(&paths.art_dir());
        let downloads = Downloads::from_config(&config.downloads);
        let settings_collapsed = config.general.settings_collapsed;
        let app = Self {
            client,
            paths,
            page: startup_page(&config.general, active_theme.layout),
            theme_library,
            plugin_library,
            active_theme,
            theme_dirty: false,
            nx_focus: None,
            theme_studio: None,
            captures: captures::Gallery::default(),
            hidden_for_game: false,
            last_session: None,
            last_session_scene: false,
            playing_before: false,
            game_end: None,
            game_end_open: false,
            game_end_timer_started: false,
            idle_since: None,
            settings_dirty_at: None,
            settings_autosave: false,
            recording: false,
            recording_state: None,
            recording_poll: Instant::now(),
            recording_rx: None,
            config,
            games: Vec::new(),
            protons: Vec::new(),
            proton_confirm: None,
            proton_sizes: ProtonSizes::default(),
            releases: Vec::new(),
            protons_checked: None,
            proton_open: HashMap::new(),
            proton_fetch: None,
            protons_auto: false,
            jobs: Vec::new(),
            session: None,
            devices: Vec::new(),
            doctor: None,
            doctor_requested: false,
            selected: None,
            detail_shown: None,
            detail_anim: 0.0,
            detail_expanded: false,
            achievements: achievements::Sheet::default(),
            gameinfo: gameinfo::Shelf::default(),
            configurations: configurations::Shelf::default(),
            steaminput: steaminput::Shelf::default(),
            saved_steam_inputs: Vec::new(),
            saved_steam_inputs_rx: None,
            saved_steam_inputs_poll: Instant::now(),
            saved_steam_input_contents: HashMap::new(),
            saved_steam_input_contents_rx: None,
            workshop: workshop::Shelf::default(),
            sheet_tab: SheetTab::General,
            sheet_tab_for: None,
            saves: saves::Panel::default(),
            saves_usage: None,
            saves_usage_rx: None,
            cloud: HashMap::new(),
            cloud_fetch: None,
            cloud_fetch_appid: None,
            cloud_asked: HashSet::new(),
            steam_bridge_status: HashMap::new(),
            steam_bridge_fetch: None,
            steam_bridge_repair: None,
            steam_bridge_armed: None,
            protect_armed: None,
            cloud_restore: None,
            cloud_restore_armed: None,
            cloud_upload: None,
            cloud_upload_armed: None,
            cloud_resolution_armed: None,
            dlc_selection: HashMap::new(),
            version_choice: HashMap::new(),
            family_choice: HashMap::new(),
            dlc_modal: None,
            news_reader: None,
            game_settings: None,
            game_settings_tab: GameSettingsTab::Launch,
            uninstall_confirm: None,
            uninstall_modal: None,
            uninstall_error: None,
            uninstall_pending: None,
            daemon_build: None,
            daemon_watch: Instant::now(),
            daemon_watch_rx: None,
            daemon_seen: false,
            daemon_seen_once: false,
            daemon_poll: Instant::now() - Duration::from_secs(10),
            search: String::new(),
            search_open: false,
            library_sort,
            library_descending,
            library_focus: None,
            library_scroll_to: None,
            only_updates: false,
            never_played: false,
            config_dirty_at: None,
            collection_selected: None,
            collection_detail_open: false,
            collection_new: false,
            collection_new_name: String::new(),
            collection_new_dynamic: false,
            collection_new_field: lapinux_core::config::CollectionField::Publisher,
            collection_manage: false,
            collection_search: String::new(),
            collection_delete: None,
            sort_menu: false,
            publisher_filter: None,
            developer_filter: None,
            credit_menu: None,
            credit_query: String::new(),
            discord_view_sent: None,
            discord_view_sent_at: None,
            library_seen,
            new_games: Vec::new(),
            library_checked: false,
            // Le contrôle part dès que les bibliothèques sont lues : les
            // conditions de `maybe_check_library` suffisent à attendre le scan
            // Steam et la relecture des boutiques, sans délai artificiel.
            library_check_at: Some(Instant::now()),
            library_sync: None,
            sync_dialog: None,
            rescan_after_refresh: false,
            play_history: Vec::new(),
            play_history_rx: None,
            play_history_asked: false,
            activity_log: None,
            store_filter: None,
            focus_search: false,
            friend_activity: None,
            notifications_open: false,
            notifications_wishlist_open: HashSet::new(),
            notification_history: VecDeque::new(),
            friend_events_seen,
            friend_events_initialized: false,
            friend_events_unread: 0,
            steam_messages_seen: HashSet::new(),
            steam_messages_initialized: false,
            steam_notifications_seen: 0,
            friend_games: HashMap::new(),
            friend_events_history,
            dry_run: None,
            dry_run_info: None,
            covers: CoverCache::new(&artwork_key, artwork_enabled),
            hover_shots: HoverShots::default(),
            hover_prefetched: HashSet::new(),
            hover_since: None,
            game_reveal: None,
            reveal_icon: None,
            reveal_pending: None,
            redeem_seen: None,
            launch_backdrops: HashMap::new(),
            art: art_cache,
            dlc_art: HashMap::new(),
            store_info: StoreInfo::default(),
            cache_usage: CacheUsage::default(),
            store_sizes: StoreSizes::default(),
            drm_reports: HashMap::new(),
            store_dlc_sizes: HashMap::new(),
            store_dlc_sizes_rx: None,
            store_dlc_pick: HashMap::new(),
            store_dlc_install_rx: None,
            dlc_art_at: HashMap::new(),
            dlc_store: HashSet::new(),
            cover_picker: None,
            cover_menu_icon: None,
            logo: Logo::load(&cc.egui_ctx),
            bunny: launch::Bunny::load(&cc.egui_ctx),
            launching: None,
            launch_fade: None,
            launch_hold: None,
            splash_poll: None,
            splash_poll_at: Instant::now(),
            launch_request_at: Instant::now(),
            wordmark: Logo::load_bytes(
                &cc.egui_ctx,
                "lapinux-letters",
                include_bytes!("../../../assets/lapinux-letters.png"),
            ),
            nav: lapinux_input::NavReader::start().ok(),
            big_picture,
            maximized: false,
            grid_width,
            startup: Startup::new(),
            last_refresh: Instant::now() - Duration::from_secs(10),
            refresh_rx: None,
            refresh_queued: false,
            profile: Profile::new(),
            social: None,
            family_seen_at: None,
            social_poll: Instant::now() - Duration::from_secs(60),
            login_poll: Instant::now(),
            token_check_at: None,
            login_since: None,
            steam_status: None,
            steam_user,
            steam_password: String::new(),
            steam_token: String::new(),
            steam_guard: String::new(),
            steam_qr: None,
            settings_open: false,
            settings_scroll_reset: true,
            settings_tab: SettingsTab::General,
            settings_collapsed,
            settings_draft: None,
            update_ui: UpdateUiState::default(),
            friends_open: false,
            store_friends: Vec::new(),
            friends_rx: None,
            friends_loading: false,
            friends_last_refresh: None,
            friends_store_available: false,
            friends_store: None,
            friends_search: String::new(),
            friends_selected: None,
            friends_chat: None,
            friends_message: String::new(),
            friends_profile_top: false,
            friends_history_top_armed: false,
            friends_chat_autoscroll: false,
            friends_chat_scroll_to_bottom: false,
            friends_avatars: HashMap::new(),
            friends_avatar_pending: HashSet::new(),
            friends_avatar_tx: None,
            friends_avatar_rx: None,
            emote_textures: HashMap::new(),
            emote_pending: HashSet::new(),
            emote_tx: None,
            emote_rx: None,
            sticker_textures: HashMap::new(),
            sticker_pending: HashSet::new(),
            sticker_tx: None,
            sticker_rx: None,
            friends_emotes_open: false,
            friends_picker_stickers: false,
            friends_import_open: false,
            friends_import_path: String::new(),
            friends_import_note: None,
            storage: StorageCache {
                dirty: true,
                ..StorageCache::default()
            },
            displays: lapinux_core::display::displays(),
            steam_login_state: SteamLoginState::default(),
            logs: None,
            logs_file: None,
            logs_error: None,
            logs_poll: Instant::now() - Duration::from_secs(10),
            catalog: None,
            library_epoch: 0,
            library_cache: RefCell::new(None),
            library_view_cache: RefCell::new(None),
            library_covers_warmed: false,
            library_fill_until: None,
            library_was_scrolling: false,
            catalog_filter: library_filter,
            sources: library_sources,
            catalog_loaded: false,
            catalog_want: None,
            catalog_patch: None,
            reveal_wait: None,
            reveal_cover: None,
            reveal_queue: None,
            catalog_rx: None,
            catalog_poll: Instant::now(),
            catalog_was_syncing: false,
            catalog_boot_synced: false,
            catalog_freshness_checked: false,
            catalog_auto: false,
            catalog_notice: false,
            // Le moniteur tourne dès le lancement : l'indicateur de la barre
            // du haut a besoin du niveau réel sur toutes les pages, pas
            // seulement quand la page Manettes est ouverte. Sinon la barre
            // retombait sur le 100 % de sysfs et perdait le halo.
            pad_monitor: lapinux_input::PadMonitor::start().ok(),
            controllers_tab: ControllersTab::Devices,
            pad_selected: None,
            pad_rename: None,
            pads_collapsed: [false, false],
            pad_info_open: false,
            pad_tests: PadTestState::default(),
            pad_tests_for: None,
            pad_cache: None,
            pad_power: None,
            pad_audio: None,
            paired_pads: None,
            paired_rx: None,
            pad_evdev_paths: Vec::new(),
            steam_puck_present: false,
            steam_puck_bridge: false,
            steam_lizard_mode: None,
            steam_lizard_writable: false,
            steam_lizard_note: None,
            steam_lizard_install: None,
            steam_lizard_persistent: false,
            steam_lizard_persist: None,
            steam_puck_poll: Instant::now(),
            steam_bridge: steam_bridge::SteamBridge::new(),
            system_sampler,
            system,
            system_poll: Instant::now(),
            disk,
            disk_poll: Instant::now(),
            downloads,
            stores: stores::Stores::default(),
            webview: webview::Webview::new(),
            trailer: trailer::Trailer::new(),
            free: freegames::FreeGames::default(),
            news_feed: news::Page::default().with_cache(news_cache_path),
            news_entries: None,
            news_games: Vec::new(),
            free_watch: false,
            free_open: false,
            jobs_poll: Instant::now(),
            jobs_rx: None,
            session_poll: Instant::now(),
            session_rx: None,
            busy: false,
        };
        // Une nouvelle fenêtre rend la parole à la présence Discord, qu'une
        // fermeture précédente avait coupée.
        {
            let worker = app.client.detached();
            std::thread::spawn(move || {
                let _ = worker.request_with_timeout(
                    &Req::DiscordPresence { active: true },
                    Duration::from_secs(2),
                );
            });
        }
        app
    }

    fn notify(&mut self, text: impl Into<String>) {
        // Les messages viennent aussi du daemon : on impose la majuscule
        // ici pour que toutes les notifications soient des phrases.
        let text = sentence(&text.into());
        self.notification_history.push_front(text);
        self.notification_history.truncate(50);
    }
}

/// Repère du chronomètre d'image (voir `frame_timer_new`).
static FRAME_MARK: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);

/// Repère du jalon d'étape, remis à zéro à chaque étape (voir `frame_stage`).
static STAGE_MARK: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);

/// Durée du dessin de la page à la dernière image.
static PAGE_MS: std::sync::Mutex<f32> = std::sync::Mutex::new(0.0);

impl eframe::App for LapinuxApp {
    /// Sans couleur de fond opaque, eframe laisse le compositeur voir à travers
    /// la fenêtre dans les zones non couvertes par un panneau.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        // Le canal alpha reste à zéro : la fenêtre principale est opaque et
        // l'ignore, tandis que l'écran d'attente, lui, peut s'effacer
        // progressivement en laissant voir le jeu derrière.
        let [r, g, b, _] = self
            .active_theme
            .colors
            .background
            .to_normalized_gamma_f32();
        [r, g, b, 0.0]
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Fermer l'application ne doit pas laisser « Regarde … » affiché sur
        // Discord : le marqueur disparaît, et le daemon retire l'activité sans
        // attendre que la requête ci-dessous aboutisse.
        HEARTBEAT_STOP.store(true, Ordering::SeqCst);
        let _guard = HEARTBEAT_WRITE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _ = std::fs::remove_file(self.paths.ui_marker());
        let _ = std::fs::remove_file(self.paths.ui_pid());
        // Une fermeture normale ne doit pas laisser le marqueur de crash du
        // pilote : il forcerait sinon Mesa pendant sept jours au prochain
        // lancement, alors que la fenêtre s'est fermée proprement.
        let _ = std::fs::remove_file(self.paths.data_dir.join("gui-wayland-crashed"));
        let worker = self.client.detached_quiet();
        let _ = worker.request_with_timeout(
            &Req::DiscordPresence { active: false },
            Duration::from_millis(900),
        );
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let _trace = FrameTrace::start();
        // Un thème activé ou un plugin coché dans les réglages prend effet à
        // l'image suivante, sans redémarrage.
        if self.theme_dirty {
            self.refresh_theme(ctx);
        }
        // La relecture démarre et se récupère même pendant l'écran de
        // démarrage : sinon le lapin resterait figé jusqu'au premier scan.
        self.poll_refresh(ctx);
        // Un clic sur rafraîchir tombé pendant une lecture attend son tour :
        // le scan repart dès que la précédente réponse est arrivée.
        if self.rescan_after_refresh && self.refresh_rx.is_none() {
            self.rescan_after_refresh = false;
            self.refresh_async(ctx, true);
        }
        self.poll_catalog(ctx);
        // Le catalogue du disque ne connaît pas un jeu acheté mais pas encore
        // installé : sa lecture part d'elle-même au lancement, une fois le
        // daemon et le compte prêts. Le cache s'affiche tout de suite ; s'il a
        // dépassé l'âge limite, une relecture du compte suit en tâche de fond,
        // sans bloquer le démarrage ni multiplier les appels Steam.
        if !self.catalog_boot_synced && self.startup.ready && self.steam_account_known() {
            self.catalog_boot_synced = true;
            self.load_catalog(false);
        }
        self.ensure_catalog_freshness();
        self.ensure_catalog(ctx);
        self.follow_catalog_sync(ctx);
        // La comparaison des nouveaux jeux a besoin de la vue des comptes et
        // de leur bibliothèque, même si l'utilisateur ouvre une autre page que
        // la Bibliothèque au démarrage.
        self.stores.ensure(&self.client);
        if let Some(message) = self.stores.poll(ctx) {
            self.notify(message);
        }
        let _frame_timer = Self::frame_timer_new();
        if self.stores.take_changed() {
            self.touch_library();
        }
        // Une boutique peut livrer ses jeux après la photographie du lancement :
        // sans relancer le contrôle, ils apparaîtraient dans la grille sans
        // jamais être annoncés comme nouveaux.
        if self.stores.take_arrivals() {
            self.library_checked = false;
            self.library_check_at = Some(Instant::now());
        }
        self.stores.ensure_games(&self.client);
        // Le contrôle passe avant de relancer un scan : sinon une relecture
        // permanente laissait `refresh_rx` occupé à chaque image et la
        // comparaison des nouveaux jeux ne partait jamais.
        self.maybe_check_library();
        Self::frame_stage("controle bibliotheque");
        self.poll_library_sync(ctx);
        Self::frame_stage("synchronisation");
        self.sync_discord_view();
        self.downloads.poll_speedtest();
        if self.downloads.speedtest_running() {
            // Chaque nouvelle mesure doit apparaître tout de suite.
            ctx.request_repaint_after(Duration::from_millis(120));
        }
        if self.last_refresh.elapsed() > Duration::from_secs(3) {
            let rescan = self.games.is_empty();
            self.refresh_async(ctx, rescan);
        }
        Self::frame_stage("relecture");
        // Pendant un téléchargement, les compteurs doivent bouger plus vite
        // que le rafraîchissement général (3 s).
        self.poll_jobs(ctx);
        // La pastille du jeu suit la cadence à la seconde.
        self.poll_session(ctx);
        self.sync_gamepad_input();
        self.poll_recording(ctx);
        // Réglages : le brouillon part tout seul, le bouton « Enregistrer »
        // n'a plus lieu d'être.
        self.sync_library_view();
        self.autosave_settings(ctx);
        self.autosave_config(ctx);
        // Inactive depuis longtemps : l'interface se ferme pour rendre sa
        // mémoire, le tray du daemon restant seul.
        Self::frame_timer_split();
        Self::count_frames();
        self.idle_tick(ctx);
        // Lapinux s'efface derrière une partie et revient à sa sortie.
        self.follow_game_window(ctx);
        if self.jobs.iter().any(|job| job.state == JobState::Running) {
            // L'animation du contour et des débits n'a lieu que sur la page qui
            // les montre ; ailleurs, la pastille de navigation se contente de
            // quelques images par seconde.
            let refresh = if self.page == Page::Downloads {
                50
            } else {
                350
            };
            ctx.request_repaint_after(Duration::from_millis(refresh));
        }
        // L'écran de démarrage couvre la fenêtre sans plus l'empêcher de se
        // dessiner : la première mise en page de la bibliothèque, quatre cents
        // millisecondes sur une grande collection, se paie sous le voile au
        // lieu de figer la fenêtre juste après son apparition.
        self.startup_screen(ctx);
        if self.system_poll.elapsed() > Duration::from_secs(1) {
            self.system = self.system_sampler.sample();
            self.system_poll = Instant::now();
        }
        // Le panneau de performances doit continuer à vivre même sans
        // interaction de l'utilisateur.
        ctx.request_repaint_after(Duration::from_secs(1));
        if self.disk_poll.elapsed() > Duration::from_secs(5) {
            self.disk = lapinux_core::system::disk_usage(&self.config.library_root());
            self.disk_poll = Instant::now();
        }
        if self.steam_login_state.running && self.login_poll.elapsed() > Duration::from_millis(500)
        {
            self.login_poll = Instant::now();
            self.poll_steam_login();
        }
        self.handle_gamepad(ctx);
        // La réponse du lancement en cours arrive en tâche de fond ; une
        // demande venue du tray démarre le même écran d'attente.
        self.poll_launch_request();
        self.poll_launch(ctx);
        // Un emplacement de sauvegarde vient d'être chargé : le jeu ne relit
        // ses fichiers qu'au démarrage, on le relance sur cette partie. La
        // réponse est relevée ici pour qu'une fiche refermée pendant la copie
        // ne perde pas le lancement.
        self.saves.poll(ctx);
        if let Some(appid) = self.saves.take_launch() {
            // Le réglage a pu être décoché pendant la restauration : la
            // relance ne part que s'il est toujours actif.
            let relaunch = self
                .games
                .iter()
                .find(|game| game.appid == appid)
                .map(|game| {
                    game.settings
                        .saves_relaunch(self.config.general.saves_relaunch)
                })
                .unwrap_or(self.config.general.saves_relaunch);
            if relaunch {
                self.launch(appid, false);
            }
        }
        // La lecture du profil se fait sur la page Profil ; le fil réseau, lui,
        // doit être vidé à chaque image pour ne pas garder les données en
        // attente quand la page est ouverte.
        self.profile.poll(ctx);
        // Les succès de la fiche arrivent en tâche de fond : les réponses sont
        // relevées même quand la fiche est refermée en cours de route.
        self.achievements.poll(ctx);
        // La fiche complète du jeu (compte, mises à jour, boutique) suit le
        // même chemin : sa lecture ne bloque jamais la peinture.
        self.gameinfo.poll(ctx);
        self.hover_shots.poll(&ctx);
        // Les fiches de boutique demandées au survol arrivent ici, comme
        // celles de la fiche ouverte.
        self.store_info.poll(&ctx);
        // L'état social se relit même hors Profil : les événements d'amis
        // doivent devenir des toasts sur toutes les pages. Une activation de
        // clé est plus urgente que le rythme habituel.
        let redeeming = self
            .social
            .as_ref()
            .and_then(|social| social.redeem.as_ref())
            .is_some_and(|redeem| redeem.state == "running");
        let social_delay = if redeeming {
            Duration::from_millis(600)
        } else {
            Duration::from_secs(4)
        };
        if self.social_poll.elapsed() > social_delay {
            self.social_poll = Instant::now();
            self.poll_social(false);
        }
        // La lecture en direct ne sert que sur la page Manettes : on libère
        // les descripteurs dès qu'elle est quittée. La sélection, elle, est
        // conservée pour retrouver la même manette au retour.
        if self.page != Page::Controllers {
            // Le micro n'a de sens que sur la page Manettes, mais le niveau
            // de batterie de la barre du haut doit rester en direct partout.
            self.pad_audio = None;
            self.pad_tests.mic_monitor = false;
            self.pad_tests.mic_note = None;
        }

        if ctx.input(|input| input.modifiers.command && input.key_pressed(egui::Key::Comma)) {
            if self.settings_open {
                self.close_settings();
            } else {
                self.open_settings(SettingsTab::General);
            }
        }
        let windowbar = self.window_bar(ctx);
        self.resize_grips(ctx);
        let nx_home =
            self.active_theme.layout == theme::ThemeLayout::Nx && self.page == Page::Library;
        if nx_home {
            // L'accueil NX porte son propre bandeau d'utilisateurs et son
            // horloge ; la barre de sections classique serait redondante.
        } else if self.active_theme.layout == theme::ThemeLayout::Nx {
            self.nx_page_bar(ctx);
        } else {
            self.topbar(ctx);
        }
        // Une clé fraîchement activée fait entrer un jeu : la bibliothèque est
        // relue et sa jaquette rejoint l'onglet Bibliothèque.
        self.note_redeem(ctx);
        self.watch_daemon(ctx);
        self.paint_game_reveal(ctx);
        self.sync_dialog_ui(ctx);
        self.settings_window(ctx);
        self.friends_panel(ctx);
        self.notifications_panel(ctx);
        self.theme_studio_window(ctx);
        if let Some(feed) = self.gameinfo.take_news() {
            self.open_news_reader(feed.items, feed.at);
        }
        if let Some((url, title)) = self.gameinfo.take_trailer() {
            self.news_reader = None;
            if trailer::is_web_page(&url) {
                // YouTube et Vimeo publient une page, pas un flux HLS : le
                // lecteur GStreamer ne peut pas l'ouvrir. Le navigateur du
                // bureau, lui, a les codecs et la session de l'utilisateur.
                crate::open_url(&url);
                self.notify(lapinux_core::i18n::t("gameinfo.trailer_external"));
            } else {
                self.trailer.open(ctx, &self.paths, &url, &title);
            }
        }
        self.news_reader_window(ctx);
        // En fenêtre, la barre de titre garde ses commandes : la visionneuse
        // s'arrête sous elle au lieu de la recouvrir. En plein écran, elle
        // n'existe pas et la vidéo prend toute la surface.
        let screen = ctx.screen_rect();
        let viewer_area = match windowbar {
            Some(bar) => {
                egui::Rect::from_min_max(egui::pos2(screen.left(), bar.bottom()), screen.max)
            }
            None => screen,
        };
        self.trailer.panel(ctx, viewer_area);
        self.cover_picker_window(ctx);
        self.game_end_overlay(ctx);
        self.gameinfo.photo_panel(ctx, viewer_area);
        self.achievements.panel(ctx);
        if let Some(appid) = self.dlc_modal {
            self.contents_panel(ctx, appid);
        }
        if let Some(appid) = self.game_settings {
            self.game_settings_window(ctx, appid);
        }
        self.poll_store_dlc_sizes();
        self.poll_logs();
        self.uninstall_window(ctx);

        let detail_page = self.page == Page::Library
            || (self.page == Page::Collections && self.collection_detail_open);
        if !self.settings_open && detail_page {
            // La barre de recherche ouverte consomme Échap en premier. Dans
            // une collection, Échap referme la fiche du jeu sans quitter la
            // collection.
            let closes_detail = self.page == Page::Library || self.selected.is_some();
            if closes_detail
                && !self.search_open
                && ctx.input(|input| input.key_pressed(egui::Key::Escape))
            {
                self.selected = None;
            }
            self.detail_sheet(ctx);
        }

        // La fenêtre de boutique survit au changement de page : elle a sa
        // propre vie, et la fermer est son affaire — ou celle du bouton qui
        // l'accompagne. On se contente de lire son état, pour ne pas garder un
        // processus terminé en mémoire.
        if self.webview.is_open() {
            let _ = self.webview.status(ctx);
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
        let page_draw = std::time::Instant::now();
        if nx_home {
            self.nx_home(ctx);
        } else {
            egui::CentralPanel::default()
                .frame(
                    egui::Frame::none()
                        .fill(self.active_theme.colors.background)
                        .inner_margin(egui::Margin::symmetric(24.0, 18.0)),
                )
                .show(ctx, |ui| match self.page {
                    Page::Home => self.home_page(ui),
                    Page::Library => self.library_page(ui),
                    Page::Collections => self.collections_page(ui),
                    Page::Downloads => self.downloads_page(ui),
                    Page::Controllers => self.controllers_page(ui),
                    Page::Profile => self.profile_page(ui),
                    Page::Boutiques => self.boutiques_page(ui),
                    Page::News => self.news_page(ui),
                    Page::Captures => {
                        // La section montre tout elle-même : l'image en grand et
                        // la vidéo image par image, dans la place de la page.
                        self.captures.page(
                            ui,
                            &self.paths,
                            &self.config.capture,
                            &self.games,
                            self.catalog.as_ref(),
                        );
                    }
                });
        }
        if let Ok(mut ms) = PAGE_MS.lock() {
            *ms = page_draw.elapsed().as_secs_f32() * 1000.0;
        }
        // La page vient d'être dessinée en entier : la plus coûteuse des
        // images est derrière nous, l'écran de démarrage peut se retirer.
        self.startup.painted = true;

        self.activity_log_panel(ctx);

        if let Some(text) = self.dry_run.clone() {
            let mut open = true;
            egui::Window::new(lapinux_core::i18n::t("game_settings.info_command"))
                .open(&mut open)
                .collapsible(false)
                .resizable(true)
                .default_width(720.0)
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut text.clone())
                                .desired_rows(16)
                                .desired_width(f32::INFINITY)
                                .font(egui::TextStyle::Monospace),
                        );
                    });
                });
            if !open {
                self.dry_run = None;
            }
        }

        // Le lapin d'attente passe par-dessus tout le reste : il se dessine
        // en dernier, une fois les pages et les fenêtres posées.
        self.launch_screen(ctx);

        if trace_enabled() {
            static AT: Mutex<Option<Instant>> = Mutex::new(None);
            if let Ok(mut at) = AT.lock() {
                if at.map_or(true, |last| last.elapsed() >= Duration::from_secs(5)) {
                    *at = Some(Instant::now());
                    eprintln!(
                        "Lapinux mémoire : jaquettes {:.1} Mo ({} tuiles) · illustrations \
                         {:.1} Mo · catalogue {} jeux · bibliothèque {} entrées",
                        self.covers.bytes() as f32 / 1_048_576.0,
                        self.covers.count(),
                        self.art.bytes() as f32 / 1_048_576.0,
                        self.catalog
                            .as_ref()
                            .map(|catalog| catalog.entries.len())
                            .unwrap_or(0),
                        self.library_entries_all().len()
                    );
                }
            }
        }

        let animating = self.session.is_some();
        if animating {
            ctx.request_repaint_after(Duration::from_secs(1));
        } else {
            ctx.request_repaint_after(Duration::from_secs(3));
        }
    }
}

/// Index de la version retenue : le choix du menu s'il est encore publié,
/// sinon la première de la liste. Un choix disparu (le jeu ne publie plus
/// cette plateforme) revient donc à la version par défaut, jamais à un index
/// hors de la liste.
fn version_index(choice: Option<&str>, versions: &[GameVersion]) -> usize {
    choice
        .and_then(|choice| versions.iter().position(|version| version.id == choice))
        .unwrap_or(0)
}

/// Une version d'un jeu publiée par Steam : la plateforme dont les dépôts se
/// téléchargent, et le poids de son build public.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GameVersion {
    /// Identifiant du protocole : `linux`, `windows`, `macos`.
    id: &'static str,
    /// Nom écrit dans le menu.
    label: &'static str,
    /// Octets à télécharger, quand Steam publie le poids du build.
    size: Option<u64>,
}

/// Poids occupé une fois installé, pour la version retenue : ce que Steam
/// publie pour cette plateforme, à défaut la première qui publie quelque
/// chose. C'est le chiffre que l'encart montre pour un jeu absent du disque.
fn published_install_bytes(details: &SteamDetails, version: &str) -> Option<u64> {
    let chosen = match version {
        "linux" => details.size_linux,
        "windows" => details.size_windows,
        "macos" => details.size_macos,
        _ => None,
    };
    let size = chosen
        .or(details.size_linux)
        .or(details.size_windows)
        .or(details.size_macos)?;
    if size.install > 0 {
        Some(size.install)
    } else if size.download > 0 {
        Some(size.download)
    } else {
        None
    }
}

/// Poids d'une plateforme : ce qui se télécharge, sinon ce qui s'installe.
fn version_bytes(size: Option<lapinux_core::model::SteamSize>) -> Option<u64> {
    let size = size?;
    if size.download > 0 {
        Some(size.download)
    } else if size.install > 0 {
        Some(size.install)
    } else {
        None
    }
}

/// Versions que le jeu publie, dans l'ordre où Lapinux les propose : Linux
/// d'abord, puisque c'est la version que la machine peut lancer sans Proton,
/// puis Windows, puis macOS.
///
/// Une plateforme compte dès que la fiche la cite : un jeu dont Steam ne
/// publie pas le poids du dépôt reste téléchargeable, son bouton n'annonce
/// simplement pas de taille.
fn game_versions(details: &SteamDetails) -> Vec<GameVersion> {
    let published = |label: &str| {
        details
            .platforms
            .iter()
            .any(|platform| platform.eq_ignore_ascii_case(label))
    };
    let mut versions = Vec::new();
    if published("Linux") || details.size_linux.is_some() {
        versions.push(GameVersion {
            id: "linux",
            label: "Linux",
            size: version_bytes(details.size_linux),
        });
    }
    if published("Windows") || details.size_windows.is_some() {
        versions.push(GameVersion {
            id: "windows",
            label: "Windows",
            size: version_bytes(details.size_windows),
        });
    }
    if published("macOS") || details.size_macos.is_some() {
        versions.push(GameVersion {
            id: "macos",
            label: "macOS",
            size: version_bytes(details.size_macos),
        });
    }
    versions
}

/// Poids annoncé sur le bouton de téléchargement : le jeu de base, plus les
/// contenus retenus dont Steam publie les dépôts. Nul quand rien n'est publié.
/// Réunit ce que Steam publie et ce que le compte possède : la fiche nomme et
/// pèse les contenus publiés, le catalogue sait lesquels sont au compte et
/// lesquels sont déjà sur le disque. Un contenu non possédé reste dans la
/// liste, pour que le panneau puisse dire « 4 sur 9 ».
fn content_rows(
    published: &[lapinux_core::model::SteamDlcRef],
    owned: &[SteamDlc],
) -> Vec<ContentRow> {
    let owned_of: HashMap<u32, &SteamDlc> = owned.iter().map(|dlc| (dlc.appid, dlc)).collect();
    let mut rows: Vec<ContentRow> = Vec::new();
    let mut seen: HashSet<u32> = HashSet::new();
    for content in published {
        seen.insert(content.appid);
        let known = owned_of.get(&content.appid);
        rows.push(ContentRow {
            appid: content.appid,
            name: if content.name.trim().is_empty() {
                known.map(|dlc| dlc.name.clone()).unwrap_or_default()
            } else {
                content.name.clone()
            },
            // Le poids le plus précis des deux lectures.
            download: content
                .download
                .max(known.map(|dlc| dlc.download).unwrap_or(0)),
            owned: content.owned || known.is_some(),
            installed: known.map(|dlc| dlc.installed).unwrap_or(false),
            image: content.image.clone(),
        });
    }
    // Ce que le compte possède et que la fiche ne liste pas : contenu retiré de
    // la liste publique, fiche pas encore lue, jeu absent de Steam.
    for content in owned {
        if seen.contains(&content.appid) {
            continue;
        }
        rows.push(ContentRow {
            appid: content.appid,
            name: content.name.clone(),
            download: content.download,
            owned: true,
            installed: content.installed,
            image: String::new(),
        });
    }
    rows
}

/// Contenus du panneau pour une version : la liste personnelle garde ceux que
/// Steam publie même sans les posséder ; une version familiale se limite à ce
/// que le membre possède, puisque le reste n'est pas téléchargeable.
fn content_rows_for_version(
    published: &[lapinux_core::model::SteamDlcRef],
    owned: &[SteamDlc],
    family: bool,
) -> Vec<ContentRow> {
    let mut rows = content_rows(published, owned);
    if family {
        let available: HashSet<u32> = owned.iter().map(|dlc| dlc.appid).collect();
        rows.retain(|row| available.contains(&row.appid));
    }
    rows
}

/// Contenus qu'un membre apporte à un jeu : ses DLC familiaux que la fiche du
/// jeu publie. Les deux filtres sont nécessaires : sans le propriétaire, un
/// contenu du compte passerait pour le sien ; sans la fiche, un contenu d'un
/// autre jeu du même membre se glisserait dans la liste.
fn family_version_dlc(
    details: &lapinux_core::model::SteamDetails,
    family_dlcs: &[lapinux_core::model::SteamFamilyDlc],
    member: u64,
) -> Vec<SteamDlc> {
    family_dlcs
        .iter()
        .filter(|dlc| dlc.owners.contains(&member))
        .filter(|dlc| details.dlc_ids.contains(&dlc.appid))
        .map(|dlc| SteamDlc {
            appid: dlc.appid,
            name: dlc.name.clone(),
            download: 0,
            install: 0,
            installed: false,
        })
        .collect()
}

/// Un contenu additionnel tel que le panneau l'affiche : publié par Steam,
/// possédé ou non, avec son poids et sa bannière.
#[derive(Clone)]
struct ContentRow {
    appid: u32,
    name: String,
    download: u64,
    owned: bool,
    installed: bool,
    /// Chemin de la bannière dans les visuels de la boutique, vide quand Steam
    /// n'en publie pas.
    image: String,
}

/// Les contenus possédés passent devant ceux qui manquent : deux sections, ce
/// que le compte peut télécharger puis le reste. Le tri est stable, l'ordre de
/// Steam est gardé à l'intérieur d'un groupe.
fn contents_owned_first(rows: &mut [ContentRow]) {
    rows.sort_by_key(|row| !row.owned);
}

/// Découpe la liste en sections affichables : possédés d'abord, puis le reste.
/// Chaque section porte son titre et son compte, et n'existe que si elle a des
/// lignes — un jeu dont tout est possédé n'affiche pas une section vide.
fn content_sections(rows: &[ContentRow]) -> Vec<(String, Vec<&ContentRow>)> {
    let owned_count = rows.iter().filter(|row| row.owned).count();
    let mut sections: Vec<(String, Vec<&ContentRow>)> = Vec::new();
    for owned in [true, false] {
        let picked: Vec<&ContentRow> = rows.iter().filter(|row| row.owned == owned).collect();
        if picked.is_empty() {
            continue;
        }
        let title = if owned {
            lapinux_core::i18n::t_args(
                "app.owned_count",
                &[("owned_count", &owned_count.to_string())],
            )
        } else {
            lapinux_core::i18n::t_fmt(
                "app.not_owned_count",
                &[(rows.len() - owned_count).to_string()],
            )
        };
        sections.push((title, picked));
    }
    sections
}

/// Contenus retenus pour un jeu : ce que l'utilisateur a coché, ou — tant qu'il
/// n'a rien décoché — tout ce que le compte possède pour ce jeu. Ceux qui sont
/// déjà sur le disque restent cochés sans rien peser, la ligne le dit.
fn selected_contents(
    state: &HashMap<u32, HashSet<u32>>,
    appid: u32,
    owned: &[SteamDlc],
) -> HashSet<u32> {
    match state.get(&appid) {
        Some(selection) => selection.clone(),
        None => owned.iter().map(|content| content.appid).collect(),
    }
}

/// Une case de la liste : cochée par l'utilisateur, ou possédée par défaut.
fn content_checked(selection: Option<&HashSet<u32>>, owned: bool, appid: u32) -> bool {
    selection.map_or(owned, |selection| selection.contains(&appid))
}

/// Ce que la ligne dit du poids, à droite du nom.
fn content_weight_text(row: &ContentRow) -> String {
    if row.installed {
        lapinux_core::i18n::t("game_contents.already_on_disk").to_string()
    } else if row.download > 0 {
        format!("+ {}", lapinux_core::util::format_bytes(row.download))
    } else if row.owned {
        // Aucun dépôt à lui : le contenu voyage dans les fichiers du jeu.
        lapinux_core::i18n::t("app.included_in_game").to_string()
    } else {
        lapinux_core::i18n::t("app.unknown_size").to_string()
    }
}

fn download_size(
    base: Option<u64>,
    dlc: &[SteamDlc],
    selected: Option<&HashSet<u32>>,
) -> Option<u64> {
    let mut total = base.unwrap_or(0);
    for content in dlc {
        // Un contenu déjà sur le disque n'ajoute rien au poids annoncé :
        // l'installation se contente de vérifier ses fichiers.
        if content.installed {
            continue;
        }
        if selected.is_some_and(|selected| selected.contains(&content.appid)) {
            total += content.download;
        }
    }
    (total > 0).then_some(total)
}

/// Vrai si un jeu du catalogue passe le filtre d'affichage de la bibliothèque.
#[cfg(test)]
fn catalog_matches(entry: &SteamCatalogGame, filter: CatalogFilter, needle: &str) -> bool {
    filter.keeps(entry.installed) && contains_lower(&entry.name, needle)
}

/// Les outils Steam (Proton, runtimes, redistribuables) ne sont pas des jeux.
fn is_steam_tool(name: &str) -> bool {
    let name = name.trim();
    if name.starts_with("Steam Linux Runtime") || name == "Steamworks Common Redistributables" {
        return true;
    }
    let Some(rest) = name.strip_prefix("Proton ") else {
        return name == "Proton";
    };
    // « Proton Bus Simulator » est un jeu ; « Proton 10.0 » et
    // « Proton Experimental » sont les outils livrés par Steam.
    rest.chars().next().is_some_and(|c| c.is_ascii_digit())
        || rest.contains("Experimental")
        || rest.contains("Hotfix")
}

/// Une démo Steam est une application distincte de son jeu, mais pas un outil :
/// le réglage de la bibliothèque peut la montrer ou la retirer de la grille.
fn is_steam_demo(entry: &SteamCatalogGame) -> bool {
    entry.details.app_type.eq_ignore_ascii_case("demo")
}

fn steam_entry_visible(entry: &SteamCatalogGame, show_demos: bool) -> bool {
    show_demos || !is_steam_demo(entry)
}

/// Compare deux noms sans tenir compte de la casse, sans allouer de chaîne.
/// Les clés de tri de la bibliothèque passaient par `to_lowercase` à chaque
/// comparaison : trois mille entrées à chaque image, des dizaines de milliers
/// de chaînes fabriquées puis jetées, et une mémoire qui grimpait au défilement.
/// PICS sépare parfois un nom de sa forme juridique (« Square Enix » puis
/// « Ltd. ») : les recoller évite de voir « Ltd. » en tête du menu des
/// éditeurs, avec plus de jeux que n'importe quel studio.
fn merge_credit_forms(credits: &[String]) -> Vec<String> {
    const FORMS: [&str; 8] = ["ltd.", "inc.", "llc", "gmbh", "co.", "corp.", "s.a.", "ag"];
    let mut merged: Vec<String> = Vec::new();
    for credit in credits {
        let trimmed = credit.trim();
        if trimmed.is_empty() {
            continue;
        }
        if FORMS.contains(&trimmed.to_lowercase().as_str()) {
            if let Some(last) = merged.last_mut() {
                last.push_str(", ");
                last.push_str(trimmed);
                continue;
            }
        }
        merged.push(trimmed.to_string());
    }
    merged
}

/// Nom commun des jeux dont la fiche ne donne ni éditeur ni développeur.
const UNKNOWN_CREDIT: &str = "Inconnu";

/// Une rangée du menu : le nom à gauche, son nombre de jeux à droite, remplie
/// quand elle est choisie. Vrai si on l'a cliquée.
fn credit_row(
    ui: &mut egui::Ui,
    width: f32,
    name: &str,
    count: Option<usize>,
    selected: bool,
) -> bool {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 21.0), egui::Sense::click());
    let painter = ui.painter();
    if selected {
        painter.rect_filled(
            rect.shrink2(egui::vec2(2.0, 1.0)),
            egui::Rounding::same(5.0),
            CARD_ACTIVE(),
        );
    } else if response.hovered() {
        painter.rect_filled(
            rect.shrink2(egui::vec2(2.0, 1.0)),
            egui::Rounding::same(5.0),
            CARD_HOVER(),
        );
    }
    painter.text(
        egui::pos2(rect.left() + 8.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        name,
        egui::FontId::proportional(12.0),
        if selected { TEXT() } else { TEXT_DIM() },
    );
    if let Some(count) = count {
        painter.text(
            egui::pos2(rect.right() - 8.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            count.to_string(),
            egui::FontId::proportional(11.0),
            TEXT_DIM(),
        );
    }
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
}

/// Mots et formules qui n'ajoutent rien au nom d'un éditeur : forme juridique
/// ou implantation régionale. « SEGA of Europe Ltd. » et « SEGA » désignent le
/// même éditeur, les filtres ne doivent donc pas les séparer.
const CREDIT_TAILS: [&str; 22] = [
    "ltd.",
    "ltd",
    "inc.",
    "inc",
    "llc",
    "gmbh",
    "co.",
    "corp.",
    "corp",
    "s.a.",
    "s.a.s.",
    "sas",
    "ag",
    "b.v.",
    "bv",
    "ab",
    "oy",
    "pty",
    "europe",
    "america",
    "emea",
    "worldwide",
];

/// Implantations qui se disent en plusieurs mots, de la plus longue à la plus
/// courte : « of north america » avant « of america ».
const CREDIT_REGIONS: [&str; 4] = [
    "of north america",
    "of south america",
    "of europe",
    "of america",
];

/// Nom d'éditeur ramené à ce qu'il a d'essentiel : « SEGA of Europe Ltd. »
/// devient « SEGA », « Square Enix, Ltd. » devient « Square Enix ».
fn clean_credit(name: &str) -> String {
    let mut words: Vec<&str> = name.split_whitespace().collect();
    loop {
        let lowered: Vec<String> = words
            .iter()
            .map(|word| word.trim_matches(',').to_lowercase())
            .collect();
        let mut cut = 0usize;
        for phrase in CREDIT_REGIONS {
            let size = phrase.split_whitespace().count();
            if lowered.len() > size && lowered[lowered.len() - size..].join(" ") == phrase {
                cut = size;
                break;
            }
        }
        if cut == 0 {
            if let Some(last) = lowered.last() {
                if CREDIT_TAILS.contains(&last.as_str()) {
                    cut = 1;
                }
            }
        }
        // Le nom entier ne disparaît jamais : il reste au moins un mot.
        if cut == 0 || lowered.len() <= cut {
            break;
        }
        words.truncate(words.len() - cut);
    }
    words
        .join(" ")
        .trim_matches(|ch: char| ch == ',' || ch.is_whitespace())
        .to_string()
}

/// Crédits d'une fiche Steam, fusionnés puis nettoyés, sans doublon.
fn clean_credits(credits: &[String]) -> Vec<String> {
    let mut cleaned: Vec<String> = Vec::new();
    for credit in merge_credit_forms(credits) {
        let name = clean_credit(&credit);
        if name.is_empty() || cleaned.iter().any(|kept| kept.eq_ignore_ascii_case(&name)) {
            continue;
        }
        cleaned.push(name);
    }
    cleaned
}

/// Vrai si l'entrée porte ce crédit, quelle que soit la casse.
fn credit_matches(entry: &LibraryEntry, publisher: bool, name: &str) -> bool {
    let list = if publisher {
        &entry.publishers
    } else {
        &entry.developers
    };
    if name == UNKNOWN_CREDIT {
        return list.is_empty();
    }
    let wanted = clean_credit(name);
    list.iter()
        .any(|credit| clean_credit(credit).eq_ignore_ascii_case(&wanted))
}

fn cmp_lower(left: &str, right: &str) -> std::cmp::Ordering {
    let mut a = left.chars().flat_map(char::to_lowercase);
    let mut b = right.chars().flat_map(char::to_lowercase);
    loop {
        match (a.next(), b.next()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(x), Some(y)) => match x.cmp(&y) {
                std::cmp::Ordering::Equal => continue,
                other => return other,
            },
        }
    }
}

/// Cherche `needle` (déjà en minuscules) dans `hay`, sans allouer.
fn contains_lower(hay: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let bytes = hay.as_bytes();
    let size = needle.len();
    if bytes.len() < size {
        return false;
    }
    (0..=bytes.len() - size)
        .any(|start| bytes[start..start + size].eq_ignore_ascii_case(needle.as_bytes()))
}

/// Repli d'un texte pour la recherche : minuscules Unicode et diacritiques
/// retirés. « Pokémon » et « POKEMON » se rejoignent, comme « Éditeur » et
/// « editeur » : le nom d'un jeu ne s'écrit pas toujours comme on le tape.
fn fold_search(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        for lower in ch.to_lowercase() {
            match lower {
                'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => out.push('a'),
                'ç' | 'ć' | 'č' => out.push('c'),
                'ď' | 'đ' => out.push('d'),
                'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ė' | 'ę' | 'ě' => out.push('e'),
                'ğ' => out.push('g'),
                'ì' | 'í' | 'î' | 'ï' | 'ī' | 'į' => out.push('i'),
                'ł' => out.push('l'),
                'ñ' | 'ń' | 'ň' => out.push('n'),
                'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ő' => out.push('o'),
                'ř' => out.push('r'),
                'ś' | 'š' | 'ş' => out.push('s'),
                'ť' | 'ţ' => out.push('t'),
                'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ů' | 'ű' => out.push('u'),
                'ý' | 'ÿ' => out.push('y'),
                'ź' | 'ż' | 'ž' => out.push('z'),
                'æ' => out.push_str("ae"),
                'œ' => out.push_str("oe"),
                'ß' => out.push_str("ss"),
                other => out.push(other),
            }
        }
    }
    out
}

/// Mots d'une recherche, repliés : tous doivent se retrouver dans le texte
/// replié, dans n'importe quel ordre.
fn search_tokens(query: &str) -> Vec<String> {
    fold_search(query)
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

fn matches_tokens(hay: &str, tokens: &[String]) -> bool {
    tokens.iter().all(|token| hay.contains(token.as_str()))
}

/// Texte où chercher une entrée : son titre et ses crédits, repliés une fois.
fn entry_search_text(entry: &LibraryEntry) -> String {
    fold_search(&format!(
        "{} {} {}",
        entry.name,
        entry.publishers.join(" "),
        entry.developers.join(" ")
    ))
}

/// Compare deux entrées pour la grille : le tri demandé s'applique seul, puis
/// le nom tranche les égalités. L'installation ne change pas l'ordre : un jeu
/// du catalogue passe devant un jeu du disque si le tri le demande.
fn compare_library(
    a: &LibraryEntry,
    b: &LibraryEntry,
    sort: LibrarySort,
    descending: bool,
) -> std::cmp::Ordering {
    let by_name = |x: &LibraryEntry, y: &LibraryEntry| cmp_lower(&x.name, &y.name);
    let primary = match sort {
        LibrarySort::Name => by_name(a, b),
        LibrarySort::Playtime => a.playtime.cmp(&b.playtime),
        LibrarySort::Acquired => a.acquired.cmp(&b.acquired),
        LibrarySort::Recent => a.last_played.cmp(&b.last_played),
        LibrarySort::Updated => a.updated.cmp(&b.updated),
        LibrarySort::Size => a.size.unwrap_or(0).cmp(&b.size.unwrap_or(0)),
    };
    let primary = if descending {
        primary.reverse()
    } else {
        primary
    };
    primary.then_with(|| by_name(a, b))
}

/// Cadre une image de couverture dans un rectangle au format demandé, en
/// coupant l'excédent plutôt qu'en la déformant. `focus` (0..1) est le point
/// de l'image qui doit rester au même endroit du cadre : au centre, le
/// recadrage est classique ; décalé, il privilégie le logo ou les personnages.
fn cover_uv_at(size: egui::Vec2, target_ratio: f32, focus: egui::Vec2) -> egui::Rect {
    let full = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    if size.x <= 0.0 || size.y <= 0.0 || target_ratio <= 0.0 {
        return full;
    }
    let focus = egui::vec2(focus.x.clamp(0.0, 1.0), focus.y.clamp(0.0, 1.0));
    let source_ratio = size.x / size.y;
    if source_ratio > target_ratio {
        let visible = target_ratio / source_ratio;
        let x = (focus.x * (1.0 - visible)).clamp(0.0, 1.0 - visible);
        egui::Rect::from_min_max(egui::pos2(x, 0.0), egui::pos2(x + visible, 1.0))
    } else {
        let visible = source_ratio / target_ratio;
        let y = (focus.y * (1.0 - visible)).clamp(0.0, 1.0 - visible);
        egui::Rect::from_min_max(egui::pos2(0.0, y), egui::pos2(1.0, y + visible))
    }
}

fn cover_uv(size: egui::Vec2, target_ratio: f32) -> egui::Rect {
    cover_uv_at(size, target_ratio, egui::vec2(0.5, 0.5))
}

fn paint_cover_at(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    texture: &egui::TextureHandle,
    rounding: egui::Rounding,
    focus: egui::Vec2,
) {
    let uv = cover_uv_at(texture.size_vec2(), rect.width() / rect.height(), focus);
    egui::Image::from_texture(egui::load::SizedTexture::new(texture.id(), rect.size()))
        .uv(uv)
        .rounding(rounding)
        .paint_at(ui, rect);
}

fn paint_cover(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    texture: &egui::TextureHandle,
    rounding: egui::Rounding,
) {
    paint_cover_at(ui, rect, texture, rounding, egui::vec2(0.5, 0.5));
}

fn paint_cover_placeholder(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    name: &str,
    rounding: egui::Rounding,
) {
    let painter = ui.painter().with_clip_rect(rect);
    painter.rect_filled(rect, rounding, CARD());

    let initial = name
        .trim()
        .chars()
        .find(|character| character.is_alphanumeric())
        .map(|character| character.to_uppercase().to_string())
        .unwrap_or_else(|| "?".to_string());
    painter.text(
        rect.center() - egui::vec2(0.0, rect.height() * 0.06),
        egui::Align2::CENTER_CENTER,
        initial,
        egui::FontId::proportional((rect.width() * 0.38).clamp(24.0, 72.0)),
        with_alpha(TEXT_DIM(), 0.16),
    );

    let title = painter.layout(
        name.trim().to_string(),
        egui::FontId::proportional((rect.width() * 0.105).clamp(11.0, 19.0)),
        TEXT(),
        rect.width() - 20.0,
    );
    let y = (rect.bottom() - title.size().y - 12.0).max(rect.top() + 10.0);
    painter.galley(
        egui::pos2(rect.center().x - title.size().x * 0.5, y),
        title,
        TEXT(),
    );
    painter.rect_stroke(
        rect.shrink(1.0),
        rounding,
        egui::Stroke::new(1.0_f32, with_alpha(BORDER(), 0.9)),
    );
}

/// Provenance d'un jeu : `Some(true)` famille, `Some(false)` compte perso,
/// `None` jeu local présent uniquement sur le disque.
fn origin_color(family: Option<bool>) -> egui::Color32 {
    match family {
        Some(true) => egui::Color32::from_rgb(235, 180, 120),
        Some(false) => egui::Color32::from_rgb(150, 215, 165),
        None => egui::Color32::from_rgb(178, 184, 194),
    }
}

/// Deux médaillons en haut à gauche des jaquettes, tous deux sur disque sombre
/// pour rester lisibles sur n'importe quelle image : la provenance (Steam blanc
/// ou disque) puis le type d'accès, cerclé de la couleur du compte.
fn paint_origin_badges(
    ui: &egui::Ui,
    rect: egui::Rect,
    family: Option<bool>,
    family_available: bool,
    store: Option<lapinux_core::stores::Store>,
    badge_scale: u32,
) {
    let painter = ui.painter();
    let base = (rect.width() / 168.0).clamp(0.5, 1.0);
    let asked = base * (badge_scale.max(10) as f32 / 100.0).clamp(0.6, 2.0);
    let margin = 6.0 * base;
    // Jusqu'à trois pastilles (Steam, compte personnel, famille) tiennent
    // toujours dans la jaquette : sur une petite tuile, l'échelle demandée est
    // ramenée à la place réelle.
    let room = (rect.width() - margin * 2.0 - 7.0 * asked) / 6.0;
    let radius = (14.5 * asked).min(room.max(5.0));
    let scale = radius / 14.5;
    let y = rect.top() + margin + radius;
    let dark = egui::Color32::from_black_alpha(190);
    let white = egui::Color32::from_rgb(248, 250, 252);

    // Provenance : la marque de la boutique, le piston Steam, ou le disque dur
    // pour un jeu local.
    let source = egui::pos2(rect.left() + margin + radius, y);
    painter.circle_filled(source, radius, dark);
    painter.circle_stroke(
        source,
        radius,
        egui::Stroke::new(1.0 * scale, with_alpha(white, 0.22)),
    );
    if let Some(store) = store {
        let mark = egui::Rect::from_center_size(source, egui::vec2(radius * 1.45, radius * 1.45));
        paint_store_mark_in(painter, store, mark, white);
        return;
    }
    match family {
        Some(_) => paint_steam_icon(painter, source, white, radius * 0.13),
        None => {
            let ink = egui::Color32::from_rgb(198, 204, 214);
            paint_disk_icon(painter, source, ink, scale);
        }
    }

    let Some(family) = family else {
        return;
    };
    let gap = radius * 2.0 + 7.0 * scale + 1.0;
    let mut access = source + egui::vec2(gap, 0.0);

    // Le compte personnel et la famille peuvent tous deux donner accès au
    // même jeu : les deux silhouettes restent alors côte à côte.
    if !family {
        painter.circle_filled(access, radius, dark);
        painter.circle_stroke(
            access,
            radius - 1.0,
            egui::Stroke::new(1.7 * scale, origin_color(Some(false))),
        );
        paint_person_icon(painter, access, white, 0.95 * scale);
        access.x += gap;
    }
    if family || family_available {
        painter.circle_filled(access, radius, dark);
        painter.circle_stroke(
            access,
            radius - 1.0,
            egui::Stroke::new(1.7 * scale, origin_color(Some(true))),
        );
        paint_family_icon(painter, access, white, 0.9 * scale);
    }
}

fn game_card(
    ui: &mut egui::Ui,
    texture: Option<&egui::TextureHandle>,
    name: &str,
    selected: bool,
    width: f32,
    cover_radius: f32,
    steam: bool,
    family: bool,
    family_available: bool,
    store: Option<lapinux_core::stores::Store>,
    show_badge: bool,
    badge_scale: u32,
    update: bool,
    is_new: bool,
) -> egui::Response {
    let size = egui::vec2(width, width * 1.5);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let rounding = egui::Rounding::same(cover_radius);
    match texture {
        Some(texture) => {
            paint_cover(ui, rect, texture, rounding);
        }
        None => {
            paint_cover_placeholder(ui, rect, name, rounding);
        }
    }
    if show_badge {
        paint_origin_badges(
            ui,
            rect,
            steam.then_some(family),
            family_available,
            store,
            badge_scale,
        );
    }
    // Pastilles d'état, dessinées à la taille de la tuile : le texte vient des
    // traductions, sa largeur n'est donc plus fixée à l'avance.
    let badge = |ui: &egui::Ui, text: &str, at: egui::Pos2, scale: f32| {
        let ink = egui::Color32::from_rgb(12, 14, 18);
        let galley = ui.painter().layout_no_wrap(
            text.to_string(),
            egui::FontId::proportional(10.5 * scale),
            ink,
        );
        let size = egui::vec2(galley.size().x + 14.0 * scale, 18.0 * scale);
        let rect = egui::Rect::from_min_size(at, size);
        ui.painter()
            .rect_filled(rect, egui::Rounding::same(9.0 * scale), ACCENT());
        ui.painter()
            .galley(rect.center() - galley.size() / 2.0, galley, ink);
    };
    let scale = (width / 168.0).clamp(0.5, 1.0);
    if update {
        let text = lapinux_core::i18n::t("library.badge_update");
        let galley_width = ui
            .painter()
            .layout_no_wrap(
                text.to_string(),
                egui::FontId::proportional(10.5 * scale),
                TEXT(),
            )
            .size()
            .x;
        badge(
            ui,
            text,
            egui::pos2(
                rect.right() - 6.0 * scale - galley_width - 14.0 * scale,
                rect.top() + 6.0 * scale,
            ),
            scale,
        );
    }
    if is_new {
        badge(
            ui,
            lapinux_core::i18n::t("library.badge_new"),
            egui::pos2(rect.left() + 6.0 * scale, rect.bottom() - 24.0 * scale),
            scale,
        );
    }
    let stroke = if selected {
        Some(egui::Stroke::new(1.5_f32, ACCENT()))
    } else if response.hovered() {
        Some(egui::Stroke::new(1.0_f32, with_alpha(ACCENT(), 0.45)))
    } else {
        None
    };
    if let Some(stroke) = stroke {
        ui.painter().rect_stroke(rect.shrink(1.0), rounding, stroke);
    }
    response
}

/// Provenance d'un jeu, telle qu'elle s'écrit dans la fiche et le survol.
fn game_origin_label(
    store: Option<lapinux_core::stores::Store>,
    steam: bool,
    family: bool,
    family_available: bool,
) -> String {
    match store {
        Some(store) => {
            lapinux_core::i18n::t_args("library.origin_store", &[("store", store.label())])
        }
        None if steam && family => lapinux_core::i18n::t("library.origin_family").to_string(),
        None if steam && family_available => {
            lapinux_core::i18n::t("library.origin_both").to_string()
        }
        None if steam => lapinux_core::i18n::t("library.origin_personal").to_string(),
        None => lapinux_core::i18n::t("library.origin_local").to_string(),
    }
}

/// Marque de la boutique à poser après le nom de la plateforme dans le pied
/// du survol. Un jeu Steam n'a pas de `store` explicite : sa présence au
/// catalogue ou en famille suffit à le reconnaître. Un jeu uniquement local
/// n'a pas de marque.
fn game_store_mark(
    store: Option<lapinux_core::stores::Store>,
    steam: bool,
    family: bool,
    family_available: bool,
) -> Option<lapinux_core::stores::Store> {
    store.or_else(|| {
        (steam || family || family_available).then_some(lapinux_core::stores::Store::Steam)
    })
}

/// Texte clair avec une ombre d'un pixel : le fond du survol est une image,
/// et le titre doit rester lisible sur ses zones claires.
fn hover_text(
    painter: &egui::Painter,
    pos: egui::Pos2,
    align: egui::Align2,
    text: &str,
    font: egui::FontId,
    color: egui::Color32,
) {
    painter.text(
        pos + egui::vec2(0.0, 1.0),
        align,
        text,
        font.clone(),
        egui::Color32::from_black_alpha(185),
    );
    painter.text(pos, align, text, font, color);
}

/// Aperçu de survol : la capsule large du jeu, puis ses captures. L'image
/// remplit le cadre ; le titre tient dans un pied discret, précédé de l'icône
/// du jeu, et la marque de la boutique suit le nom de la plateforme. La carte
/// ne capte jamais la souris et se pose hors de la tuile survolée, du côté
/// opposé au pointeur.
fn game_hover_card(
    ctx: &egui::Context,
    anchor: egui::Rect,
    appid: u32,
    name: &str,
    image: Option<&egui::TextureHandle>,
    capsule_focus: egui::Vec2,
    icon: Option<&egui::TextureHandle>,
    store: Option<lapinux_core::stores::Store>,
    shots: &[egui::TextureHandle],
    origin: &str,
    update: bool,
    elapsed: Duration,
    width: f32,
) {
    const MARGIN: f32 = 10.0;
    const GAP: f32 = 14.0;
    const FOOTER: f32 = 44.0;
    const HOLD: f32 = 2.4;
    const SHOT_INTERVAL: f32 = 4.2;
    const SHOT_FADE: f32 = 0.65;

    let width = width.clamp(220.0, 420.0);
    // Le cadre reprend le ratio de la capsule large 616×353 : elle le remplit
    // sans perdre le logo ni les personnages. Les captures 16:9 n'y perdent
    // que deux pour cent.
    let image_height = width * 353.0 / 616.0;
    let height = image_height + FOOTER;

    let screen = ctx.screen_rect();
    let pointer = ctx.pointer_hover_pos().unwrap_or_else(|| anchor.center());
    let centered = (anchor.center().x - width / 2.0).clamp(
        screen.left() + MARGIN,
        (screen.right() - width - MARGIN).max(screen.left() + MARGIN),
    );
    // La carte ne recouvre pas la tuile survolée : elle se pose dessous quand
    // le pointeur est dans la moitié haute, dessus quand il est dans la moitié
    // basse. Si cette place manque, elle tente l'autre côté, puis le côté
    // opposé au pointeur.
    let below = pointer.y < anchor.center().y;
    let below_pos = egui::pos2(centered, anchor.bottom() + GAP);
    let above_pos = egui::pos2(centered, anchor.top() - height - GAP);
    let fits = |pos: egui::Pos2| {
        pos.y >= screen.top() + MARGIN
            && pos.y + height <= screen.bottom() - MARGIN
            && pos.x >= screen.left() + MARGIN
            && pos.x + width <= screen.right() - MARGIN
    };
    let mut pos = if below { below_pos } else { above_pos };
    if !fits(pos) {
        let other = if below { above_pos } else { below_pos };
        if fits(other) {
            pos = other;
        } else {
            let right = pointer.x < anchor.center().x;
            let y = (anchor.center().y - height / 2.0).clamp(
                screen.top() + MARGIN,
                (screen.bottom() - height - MARGIN).max(screen.top() + MARGIN),
            );
            pos = egui::pos2(
                if right {
                    anchor.right() + GAP
                } else {
                    anchor.left() - width - GAP
                },
                y,
            );
        }
    }
    pos.x = pos.x.clamp(
        screen.left() + MARGIN,
        (screen.right() - width - MARGIN).max(screen.left() + MARGIN),
    );
    pos.y = pos.y.clamp(
        screen.top() + MARGIN,
        (screen.bottom() - height - MARGIN).max(screen.top() + MARGIN),
    );

    let elapsed = elapsed.as_secs_f32();
    egui::Area::new(egui::Id::new(("game-hover", appid)))
        .order(egui::Order::Tooltip)
        .fixed_pos(pos)
        .show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
            let rounding = egui::Rounding::same(8.0);
            let image_rounding = egui::Rounding {
                nw: 8.0,
                ne: 8.0,
                sw: 0.0,
                se: 0.0,
            };
            let painter = ui.painter().clone();

            // Ombre portée : quelques rectangles élargis détachent la carte
            // de la grille sans ajouter de flou à l'image.
            for step in (0..6).rev() {
                let grow = (step + 1) as f32 * 1.7;
                painter.rect_filled(
                    rect.expand(grow).translate(egui::vec2(0.0, 3.0)),
                    egui::Rounding::same(8.0 + grow),
                    egui::Color32::from_black_alpha(10),
                );
            }
            painter.rect_filled(rect, rounding, CARD());

            let image_rect = egui::Rect::from_min_max(
                rect.min,
                egui::pos2(rect.right(), rect.top() + image_height),
            );
            match image {
                Some(texture) => {
                    paint_cover_at(ui, image_rect, texture, image_rounding, capsule_focus)
                }
                None => paint_cover_placeholder(ui, image_rect, name, image_rounding),
            }

            // Après quelques secondes, la capsule cède sa place aux captures
            // du jeu. Chaque nouvelle image entre en fondu par-dessus la
            // précédente, dans le même cadre.
            let mut showing_shots = false;
            let mut active = 0usize;
            if !shots.is_empty() && elapsed >= HOLD {
                showing_shots = true;
                let seconds = elapsed - HOLD;
                let index = (seconds / SHOT_INTERVAL).floor() as usize;
                let local = seconds - index as f32 * SHOT_INTERVAL;
                let fade = smoothstep(local / SHOT_FADE);
                active = index % shots.len();
                if index > 0 {
                    paint_cover(
                        ui,
                        image_rect,
                        &shots[(index - 1) % shots.len()],
                        image_rounding,
                    );
                }
                paint_cover_fade(ui, image_rect, &shots[active], image_rounding, fade);
            }

            let footer = egui::Rect::from_min_max(
                egui::pos2(rect.left(), rect.top() + image_height),
                egui::pos2(rect.right(), rect.bottom()),
            );
            let footer_rounding = egui::Rounding {
                nw: 0.0,
                ne: 0.0,
                sw: 8.0,
                se: 8.0,
            };
            painter.rect_filled(footer, footer_rounding, CARD());
            painter.line_segment(
                [
                    egui::pos2(footer.left(), footer.top()),
                    egui::pos2(footer.right(), footer.top()),
                ],
                egui::Stroke::new(1.0_f32, with_alpha(TEXT(), 0.07)),
            );

            // L'icône du jeu occupe la gauche du pied ; la marque de la
            // boutique viendra après le nom de la plateforme, dans la ligne
            // d'origine.
            let icon_rect = egui::Rect::from_center_size(
                egui::pos2(rect.left() + 22.0, footer.center().y),
                egui::vec2(20.0, 20.0),
            );
            painter.rect_filled(
                icon_rect.expand(1.0),
                egui::Rounding::same(5.0),
                egui::Color32::from_black_alpha(80),
            );
            match icon {
                Some(texture) => {
                    paint_cover(ui, icon_rect, texture, egui::Rounding::same(4.0));
                }
                None => {
                    // Toutes les boutiques ne publient pas d'icône carrée. Le
                    // visuel large déjà affiché au-dessus reste une image du
                    // jeu : son centre recadré vaut mieux qu'une initiale.
                    if let Some(texture) = image {
                        paint_cover(ui, icon_rect, texture, egui::Rounding::same(4.0));
                    } else {
                        painter.rect_filled(icon_rect, egui::Rounding::same(4.0), CARD_HOVER());
                        let initial = name
                            .chars()
                            .find(|character| character.is_alphanumeric())
                            .map(|character| character.to_uppercase().to_string())
                            .unwrap_or_else(|| "?".into());
                        painter.text(
                            icon_rect.center(),
                            egui::Align2::CENTER_CENTER,
                            initial,
                            egui::FontId::proportional(11.0),
                            TEXT_DIM(),
                        );
                    }
                }
            }

            let text_x = rect.left() + 40.0;
            let text_width = rect.right() - 14.0 - text_x;
            let title =
                elide_to_width(&painter, name, egui::FontId::proportional(13.5), text_width);
            hover_text(
                &painter,
                egui::pos2(text_x, footer.top() + 6.0),
                egui::Align2::LEFT_TOP,
                &title,
                egui::FontId::proportional(13.5),
                TEXT(),
            );
            let origin_color = egui::Color32::from_rgb(202, 205, 211);
            let mark_size = 12.0;
            let mark_gap = 4.0;
            let origin_font = egui::FontId::proportional(9.5);
            let origin_pos = egui::pos2(text_x, footer.top() + 29.0);
            match store {
                Some(store) => {
                    // La ligne d'origine est « Plateforme · détail » : la
                    // marque se glisse juste après le nom de la plateforme,
                    // avant le détail du compte.
                    let (platform, detail) = origin.split_once(" · ").unwrap_or((origin, ""));
                    let mark_room = mark_size + mark_gap * 2.0;
                    let platform = elide_to_width(
                        &painter,
                        platform,
                        origin_font.clone(),
                        (text_width - mark_room).max(20.0),
                    );
                    let platform_width = painter
                        .layout_no_wrap(platform.clone(), origin_font.clone(), origin_color)
                        .size()
                        .x;
                    hover_text(
                        &painter,
                        origin_pos,
                        egui::Align2::LEFT_CENTER,
                        &platform,
                        origin_font.clone(),
                        origin_color,
                    );
                    let mark_rect = egui::Rect::from_center_size(
                        egui::pos2(
                            origin_pos.x + platform_width + mark_gap + mark_size / 2.0,
                            origin_pos.y,
                        ),
                        egui::vec2(mark_size, mark_size),
                    );
                    paint_store_mark_in(&painter, store, mark_rect, origin_color);
                    if !detail.is_empty() {
                        let detail = format!("· {detail}");
                        let detail_x = mark_rect.right() + mark_gap;
                        let detail_max = (text_width - (detail_x - text_x)).max(0.0);
                        if detail_max > 4.0 {
                            let detail =
                                elide_to_width(&painter, &detail, origin_font.clone(), detail_max);
                            hover_text(
                                &painter,
                                egui::pos2(detail_x, origin_pos.y),
                                egui::Align2::LEFT_CENTER,
                                &detail,
                                origin_font,
                                origin_color,
                            );
                        }
                    }
                }
                None => {
                    let origin = elide_to_width(&painter, origin, origin_font.clone(), text_width);
                    hover_text(
                        &painter,
                        origin_pos,
                        egui::Align2::LEFT_CENTER,
                        &origin,
                        origin_font,
                        origin_color,
                    );
                }
            }

            if update {
                let pill = egui::Rect::from_min_size(
                    egui::pos2(rect.left() + 12.0, rect.top() + 12.0),
                    egui::vec2(30.0, 16.0),
                );
                painter.rect_filled(pill, egui::Rounding::same(8.0), WARN);
                painter.text(
                    pill.center(),
                    egui::Align2::CENTER_CENTER,
                    "MAJ",
                    egui::FontId::proportional(8.0),
                    egui::Color32::from_rgb(20, 21, 22),
                );
            }

            // Trois points discrets disent où en est le petit diaporama.
            if showing_shots {
                let count = shots.len().min(5);
                let width = count as f32 * 7.0 + 5.0;
                let pill = egui::Rect::from_min_size(
                    egui::pos2(rect.right() - 12.0 - width, rect.top() + 12.0),
                    egui::vec2(width, 12.0),
                );
                painter.rect_filled(
                    pill,
                    egui::Rounding::same(6.0),
                    egui::Color32::from_black_alpha(110),
                );
                for dot in 0..count {
                    let center = egui::pos2(pill.left() + 5.0 + dot as f32 * 7.0, pill.center().y);
                    painter.circle_filled(
                        center,
                        2.0,
                        if dot == active.min(count - 1) {
                            TEXT()
                        } else {
                            with_alpha(TEXT(), 0.35)
                        },
                    );
                }
            }

            painter.rect_stroke(
                rect.shrink(0.5),
                rounding,
                egui::Stroke::new(1.0_f32, with_alpha(TEXT(), 0.14)),
            );

            // Le diaporama et son fondu ont besoin de quelques images ; sans
            // capture à charger, un rythme plus lent suffit.
            let refresh = if shots.is_empty() { 180 } else { 40 };
            ctx.request_repaint_after(Duration::from_millis(refresh));
        });
}

/// Peint une image de capture par-dessus l'aperçu, avec un fondu.
fn paint_cover_fade(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    texture: &egui::TextureHandle,
    rounding: egui::Rounding,
    alpha: f32,
) {
    let alpha = (alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
    let uv = cover_uv(texture.size_vec2(), rect.width() / rect.height());
    egui::Image::from_texture(egui::load::SizedTexture::new(texture.id(), rect.size()))
        .uv(uv)
        .rounding(rounding)
        .tint(egui::Color32::from_white_alpha(alpha))
        .paint_at(ui, rect);
}

/// Menu contextuel commun aux jaquettes : la vignette et le nom du jeu en
/// tête, puis les actions sur le dossier installé, le choix d'une image
/// SteamGridDB et le retour à la sélection automatique. Les collections
/// manuelles s'ajoutent ici : retrait direct dans une collection, ou cases à
/// cocher depuis la bibliothèque. Le chemin lui-même reste dans le
/// presse-papiers, sans envahir le menu.
#[allow(clippy::too_many_arguments)]
/// Action choisie dans le menu d'une jaquette, appliquée par la page qui l'a
/// ouvert : le menu ne lance rien lui-même, il ne fait que choisir.
#[derive(Clone, Copy, PartialEq, Eq)]
enum GameAction {
    Play(u32),
    Install(u32),
    Sheet(u32),
    Hide(u32),
    Unhide(u32),
}

fn cover_context_menu(
    response: egui::Response,
    appid: u32,
    name: &str,
    icon: Option<&egui::TextureHandle>,
    has_custom: bool,
    installed: bool,
    hidden: bool,
    install_dir: Option<&Path>,
    collections: &[lapinux_core::config::GameCollection],
    current_collection: Option<&lapinux_core::config::GameCollection>,
) -> (
    Option<u32>,
    Option<u32>,
    Option<PathBuf>,
    Vec<(String, bool)>,
    Option<GameAction>,
) {
    let mut picker = None;
    let mut reset = None;
    let mut folder = None;
    let mut action = None;
    let mut collection_changes = Vec::new();
    response.context_menu(|ui| {
        // Largeur fixe et courte : dans un menu, le séparateur et les boutons
        // occupent toute la place offerte, si bien que c'est la largeur du
        // menu qui décide, pas son contenu. Une largeur maximale le laissait
        // donc s'étirer jusqu'à elle ; 252 points suffisent au titre comme
        // aux libellés. Les rangées se resserrent d'autant.
        ui.set_width(252.0);
        ui.spacing_mut().item_spacing.y = 3.0;
        ui.spacing_mut().button_padding = egui::vec2(8.0, 3.0);
        ui.spacing_mut().interact_size.y = 24.0;
        // En-tête : l'icône carrée et le nom du jeu, avant toute action. Un
        // menu s'ouvre sur une jaquette qu'on reconnaît à son image : il doit
        // la rappeler, au lieu de ne parler que d'un appid.
        ui.horizontal(|ui| {
            // L'icône carrée du jeu, comme dans la fiche : elle identifie le
            // jeu d'un coup d'œil, là où une jaquette ressemble à sa voisine.
            let (rect, _) = ui.allocate_exact_size(egui::vec2(36.0, 36.0), egui::Sense::hover());
            let rounding = egui::Rounding::same(8.0);
            match icon {
                Some(texture) => paint_cover(ui, rect, texture, rounding),
                None => paint_cover_placeholder(ui, rect, name, rounding),
            }
            ui.add_space(2.0);
            // Un libellé qui se replie réclame toute la largeur du menu et
            // l'étire jusqu'à son maximum : le titre est coupé sur une ligne,
            // et le menu garde la taille de son contenu.
            let title = elide_to_width(ui.painter(), name, egui::FontId::proportional(13.0), 176.0);
            ui.add(egui::Label::new(
                egui::RichText::new(title).size(13.0).strong().color(TEXT()),
            ));
        });
        ui.separator();
        // Les actions rapides passent avant le reste : lancer un jeu installé,
        // ou l'installer, ne devrait pas demander d'ouvrir sa fiche.
        let primary = if installed {
            (
                lapinux_core::i18n::t("library.action.play"),
                GameAction::Play(appid),
            )
        } else {
            (
                lapinux_core::i18n::t("library.action.install"),
                GameAction::Install(appid),
            )
        };
        if ui.button(primary.0).clicked() {
            action = Some(primary.1);
            ui.close_menu();
        }
        if ui
            .button(lapinux_core::i18n::t("library.action.sheet"))
            .clicked()
        {
            action = Some(GameAction::Sheet(appid));
            ui.close_menu();
        }
        ui.separator();
        if let Some(path) = install_dir {
            let location = path.display().to_string();
            if ui
                .button(lapinux_core::i18n::t("library.action.open_folder"))
                .clicked()
            {
                folder = Some(path.to_path_buf());
                ui.close_menu();
            }
            if ui
                .button(lapinux_core::i18n::t("library.action.copy_path"))
                .clicked()
            {
                ui.ctx().copy_text(location);
                ui.close_menu();
            }
            ui.separator();
        }
        if ui
            .button(lapinux_core::i18n::t("library.action.choose_cover"))
            .clicked()
        {
            picker = Some(appid);
            ui.close_menu();
        }
        if has_custom
            && ui
                .button(lapinux_core::i18n::t("library.action.reset_cover"))
                .clicked()
        {
            reset = Some(appid);
            ui.close_menu();
        }
        // Une collection ouverte ne propose que le retrait de ce jeu ; depuis
        // la bibliothèque, le sous-menu montre toutes les collections
        // manuelles et leur appartenance. Les collections dynamiques se
        // recalculent seules et ne sont pas modifiables.
        let manual: Vec<&lapinux_core::config::GameCollection> = collections
            .iter()
            .filter(|collection| collection.field.is_none())
            .collect();
        if let Some(current) = current_collection.filter(|collection| collection.field.is_none()) {
            ui.separator();
            let label = format!(
                "{} « {} »",
                lapinux_core::i18n::t("library.action.collection_remove"),
                elide_to_width(
                    ui.painter(),
                    &current.name,
                    egui::FontId::proportional(12.0),
                    176.0,
                )
            );
            if ui
                .button(egui::RichText::new(label).color(DANGER))
                .clicked()
            {
                collection_changes.push((current.id.clone(), false));
                ui.close_menu();
            }
        } else if !manual.is_empty() {
            ui.separator();
            ui.menu_button(lapinux_core::i18n::t("library.action.collections"), |ui| {
                for collection in manual {
                    let mut member = collection.appids.contains(&appid);
                    let label = elide_to_width(
                        ui.painter(),
                        &collection.name,
                        egui::FontId::proportional(11.0),
                        190.0,
                    );
                    let response = ui.checkbox(&mut member, label);
                    if response.clicked() {
                        collection_changes.push((collection.id.clone(), member));
                    }
                    response.on_hover_text(&collection.name);
                }
            });
        }
        ui.separator();
        // Masquer un jeu ne le désinstalle pas : il sort de la grille, et le
        // réglage « afficher les jeux masqués » le ramène.
        if hidden {
            if ui
                .button(lapinux_core::i18n::t("library.action.unhide"))
                .clicked()
            {
                action = Some(GameAction::Unhide(appid));
                ui.close_menu();
            }
        } else if ui
            .button(lapinux_core::i18n::t("library.action.hide"))
            .clicked()
        {
            action = Some(GameAction::Hide(appid));
            ui.close_menu();
        }
    });
    (picker, reset, folder, collection_changes, action)
}

/// Année civile d'un horodatage Unix, sans dépendance de calendrier :
/// l'algorithme civil de Howard Hinnant, réduit au seul millésime.
fn year_from_unix(seconds: i64) -> i32 {
    let days = seconds.div_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    if mp >= 10 {
        year += 1;
    }
    year as i32
}

fn grid_columns(available: f32, card_width: f32, gap: f32) -> usize {
    if available <= 0.0 || card_width <= 0.0 {
        return 1;
    }
    (((available + gap) / (card_width + gap)).floor() as usize).max(1)
}

fn row_padding(available: f32, count: usize, card_width: f32, gap: f32) -> f32 {
    if count == 0 {
        return 0.0;
    }
    let row_width = count as f32 * card_width + count.saturating_sub(1) as f32 * gap;
    ((available - row_width) / 2.0).max(0.0)
}

/// Tronque un libellé à la largeur réellement disponible, points de
/// suspension compris, plutôt qu'à un nombre de caractères arbitraire.
fn elide_to_width(
    painter: &egui::Painter,
    text: &str,
    font: egui::FontId,
    max_width: f32,
) -> String {
    if painter
        .layout_no_wrap(text.to_owned(), font.clone(), TEXT())
        .size()
        .x
        <= max_width
    {
        return text.to_owned();
    }
    let mut chars: Vec<char> = text.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let candidate = format!("{}…", chars.iter().collect::<String>().trim_end());
        if painter
            .layout_no_wrap(candidate.clone(), font.clone(), TEXT())
            .size()
            .x
            <= max_width
        {
            return candidate;
        }
    }
    "…".to_string()
}

/// Marge latérale de la fiche : elle laisse la bibliothèque respirer sur les
/// côtés, plutôt que de s'étaler sur toute la largeur de la fenêtre.
fn sheet_margin_x(width: f32) -> f32 {
    (width * 0.05).clamp(24.0, 120.0)
}
/// Hauteur de la fiche repliée : une seule ligne (identité, succès, rail) et
/// la bande de tags qui la ferme.
const SHEET_COMPACT_HEIGHT: f32 = 260.0;
/// Hauteur de la bande de tags de faits, et l'écart qui la sépare des encarts.
const SHEET_TAGS_HEIGHT: f32 = 26.0;
const SHEET_TAGS_GAP: f32 = 10.0;
/// Hauteur de la ligne d'identité, la même repliée et dépliée : la carte du
/// jeu ne change pas de taille quand on ouvre le corps. Elle laisse
/// exactement la bande de tags sous la fiche repliée (260 − 32 − 36).
const SHEET_HEADER_HEIGHT: f32 = 192.0;
/// Hauteur maximale de la fiche dépliée : au-delà, elle masquerait la
/// bibliothèque.
const SHEET_MAX_HEIGHT: f32 = 660.0;
/// Écart entre les deux colonnes de la fiche.
const SHEET_GAP: f32 = 26.0;
/// Durée de la glissière de la fiche, en secondes.
const SHEET_SLIDE: f32 = 0.18;

/// Rectangle de la fiche : plaquée au bas de la fenêtre, avec une marge sur
/// les côtés. `progress` va de 0 (rangée sous la fenêtre) à 1 (ouverte).
/// Repliée, elle garde la hauteur d'une seule ligne ; dépliée, elle monte
/// jusqu'à 62 % de la fenêtre pour le corps détaillé.
fn detail_sheet_rect(screen: egui::Rect, progress: f32, expanded: bool) -> egui::Rect {
    let margin = sheet_margin_x(screen.width());
    let width = (screen.width() - margin * 2.0).max(280.0);
    let room = (screen.height() - 96.0).max(200.0);
    let height = if expanded {
        (screen.height() * 0.62)
            .clamp(360.0, SHEET_MAX_HEIGHT)
            .min(room)
    } else {
        SHEET_COMPACT_HEIGHT
            .min(room)
            .max(180.0)
            .min(screen.height())
    };
    let hidden = (1.0 - progress.clamp(0.0, 1.0)) * (height + 24.0);
    let top = screen.bottom() - height + hidden;
    egui::Rect::from_min_size(
        egui::pos2(screen.left() + margin, top),
        egui::vec2(width, height),
    )
}

/// Avancement de la glissière de la fiche : elle met [`SHEET_SLIDE`] secondes
/// à monter du bas, et autant à redescendre.
fn advance_detail_sheet(current: f32, open: bool, dt: f32) -> f32 {
    let target = if open { 1.0 } else { 0.0 };
    let step = (dt.clamp(0.0, 0.1) / SHEET_SLIDE).max(0.0);
    if target > current {
        (current + step).min(target)
    } else {
        (current - step).max(target)
    }
}

/// Largeur d'une colonne latérale du corps déplié : les **deux** colonnes ont
/// la même, pour que la description du jeu reste centrée entre elles.
/// En dessous de la largeur qui laisse [`SHEET_CENTER_MIN`] au centre, elles
/// disparaissent et les encarts s'empilent.
const SHEET_SIDE_WIDTH: f32 = 286.0;
const SHEET_SIDE_MIN: f32 = 236.0;
const SHEET_CENTER_MIN: f32 = 320.0;

/// Largeurs des colonnes du corps de la fiche : les actualités à gauche, la
/// description au centre, les images (bande-annonce et captures) à droite.
/// Les deux colonnes latérales sont égales ; la première vaut zéro quand la
/// fiche est trop étroite pour trois colonnes, et les encarts s'empilent alors
/// dans l'ordre du dessin.
/// Onglets de la page Manettes, alignés sur les pastilles segmentées du reste
/// de l'interface.
fn controllers_tab_bar(ui: &mut egui::Ui, current: ControllersTab) -> Option<ControllersTab> {
    let tabs = [
        (
            ControllersTab::General,
            lapinux_core::i18n::t("controllers.general"),
            104.0,
        ),
        (
            ControllersTab::Devices,
            lapinux_core::i18n::t("controllers.devices"),
            122.0,
        ),
    ];
    let widths: Vec<f32> = tabs.iter().map(|(_, _, width)| *width).collect();
    let active: Vec<bool> = tabs.iter().map(|(tab, _, _)| *tab == current).collect();
    let (rect, responses) = segmented_pill(ui, &widths, &active, "controllers-tabs");
    let painter = ui.painter();
    let mut chosen = None;
    for (index, (tab, label, _)) in tabs.iter().enumerate() {
        let slot = segment_rect(rect, &widths, index);
        painter.text(
            slot.center(),
            egui::Align2::CENTER_CENTER,
            *label,
            egui::FontId::proportional(12.5),
            if active[index] { TEXT() } else { TEXT_DIM() },
        );
        if responses[index].clicked() {
            chosen = Some(*tab);
        }
    }
    chosen
}

/// Onglets disponibles dans la fiche, dans l'ordre d'affichage. Configurations
/// disparaît pour un jeu que le catalogue ne connaît pas ; Mods pour un jeu
/// sans atelier local ni drapeau PICS.
fn sheet_tabs(
    show_configurations: bool,
    show_steam_input: bool,
    show_workshop: bool,
) -> Vec<(SheetTab, &'static str, f32)> {
    let mut tabs: Vec<(SheetTab, &'static str, f32)> = vec![(
        SheetTab::General,
        lapinux_core::i18n::t("game_sheet.tab_general"),
        90.0,
    )];
    if show_configurations {
        tabs.push((
            SheetTab::Configurations,
            lapinux_core::i18n::t("game_sheet.tab_configurations"),
            126.0,
        ));
    }
    tabs.push((
        SheetTab::Saves,
        lapinux_core::i18n::t("game_settings.tab_saves"),
        112.0,
    ));
    if show_steam_input {
        tabs.push((SheetTab::SteamInput, "Steam Input", 106.0));
    }
    if show_workshop {
        tabs.push((
            SheetTab::Workshop,
            lapinux_core::i18n::t("game_sheet.tab_mods"),
            64.0,
        ));
    }
    tabs.push((
        SheetTab::Details,
        lapinux_core::i18n::t("configurations.details"),
        84.0,
    ));
    tabs
}

/// Barre d'onglets de la fiche, sous la ligne compacte : un rectangle par
/// onglet disponible, celui qui est ouvert se distingue par son fond, comme la
/// fiche Profil. Le sélecteur est centré dans la largeur de la fiche, encadré
/// d'un filet horizontal à gauche et à droite. Configurations disparaît pour
/// un jeu que le catalogue ne connaît pas.
fn sheet_tab_bar(
    ui: &mut egui::Ui,
    appid: u32,
    current: SheetTab,
    show_configurations: bool,
    show_steam_input: bool,
    show_workshop: bool,
) -> Option<SheetTab> {
    let tabs = sheet_tabs(show_configurations, show_steam_input, show_workshop);
    let widths: Vec<f32> = tabs.iter().map(|(_, _, width)| *width).collect();
    let active: Vec<bool> = tabs.iter().map(|(tab, _, _)| *tab == current).collect();
    let salt = format!("sheet-tabs-{appid}");
    let total: f32 = widths.iter().sum();
    let full = ui.available_width();
    let full = if full.is_finite() {
        full.max(total)
    } else {
        total
    };
    let (row, _) = ui.allocate_exact_size(egui::vec2(full, FILTER_H), egui::Sense::hover());
    let rect = egui::Rect::from_center_size(row.center(), egui::vec2(total, FILTER_H));
    // Un filet de chaque côté, à mi-hauteur du sélecteur, détaché de 16 px.
    const RULE_GAP: f32 = 16.0;
    let painter = ui.painter().clone();
    let y = rect.center().y.round() + 0.5;
    for (from, to) in [
        (row.left(), rect.left() - RULE_GAP),
        (rect.right() + RULE_GAP, row.right()),
    ] {
        if to - from >= 2.0 {
            painter.line_segment(
                [egui::pos2(from, y), egui::pos2(to, y)],
                egui::Stroke::new(1.0_f32, BORDER()),
            );
        }
    }
    let responses = segmented_pill_at(ui, rect, &widths, &active, &salt, egui::Rounding::same(8.0));
    let mut chosen = None;
    for (index, (tab, label, _)) in tabs.iter().enumerate() {
        let slot = segment_rect(rect, &widths, index);
        let ink = if active[index] { TEXT() } else { TEXT_DIM() };
        painter.text(
            slot.center(),
            egui::Align2::CENTER_CENTER,
            *label,
            egui::FontId::proportional(13.0),
            ink,
        );
        if responses[index].clicked() {
            chosen = Some(*tab);
        }
        if !active[index] && responses[index].hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
    }
    chosen
}

fn sheet_body_columns(width: f32) -> (f32, f32, f32) {
    let side = (width * 0.22).clamp(SHEET_SIDE_MIN, SHEET_SIDE_WIDTH);
    let center = width - side * 2.0 - SHEET_GAP * 2.0;
    if center < SHEET_CENTER_MIN {
        return (0.0, width, 0.0);
    }
    (side, center, side)
}

/// Écart à réserver entre deux colonnes du corps.
///
/// Une rangée egui insère déjà son `item_spacing` entre deux enfants, et
/// `add_space` s'y ajoute : sans cette déduction, la colonne de droite sortait
/// de la fiche de deux espacements et ses images étaient coupées au bord.
fn sheet_column_gap(spacing: f32) -> f32 {
    (SHEET_GAP - spacing).max(0.0)
}

/// Membre de la famille Steam qui apporte un jeu à la bibliothèque partagée.
/// La photo est résolue au dessin de la fiche, pas à la construction de
/// l'encart : le catalogue ne connaît pas les textures.
#[derive(Clone)]
struct FamilyVersion {
    steam_id: u64,
    name: String,
    avatar: String,
    /// Jeux que ce membre partage avec la famille.
    games: u32,
    texture: Option<egui::TextureHandle>,
    /// Vrai pour l'entrée « Moi » du menu, qui représente la licence du compte
    /// connecté plutôt qu'un membre qui prête le jeu.
    personal: bool,
    /// Contenus additionnels apportés par cette version, tels que la fiche
    /// les montre : ceux du compte pour « Moi », ceux du membre pour les
    /// autres entrées.
    dlc_names: Vec<String>,
}

/// Versions proposées pour un jeu partagé : « Moi » d'abord quand le compte
/// possède aussi le jeu, puis les membres qui le partagent. Le compte connecté
/// n'est jamais répété dans la liste des membres.
fn family_versions_for(
    appid: u32,
    family: Option<&lapinux_core::social::SteamFamily>,
    include_self: bool,
) -> Vec<FamilyVersion> {
    let mut versions = Vec::new();
    if include_self {
        let me = family.and_then(|family| family.members.iter().find(|member| member.is_self));
        versions.push(FamilyVersion {
            steam_id: me.map(|member| member.steam_id).unwrap_or(0),
            name: lapinux_core::i18n::t("game_sheet.personal_version").to_string(),
            avatar: me.map(|member| member.avatar.clone()).unwrap_or_default(),
            games: me.map(|member| member.games).unwrap_or(0),
            texture: None,
            personal: true,
            dlc_names: Vec::new(),
        });
    }
    if let Some(family) = family {
        versions.extend(family_versions_of(appid, family));
    }
    versions
}

/// Membres qui apportent un jeu à la bibliothèque partagée. Le compte
/// connecté est écarté : son jeu ne serait pas marqué familial.
fn family_versions_of(
    appid: u32,
    family: &lapinux_core::social::SteamFamily,
) -> Vec<FamilyVersion> {
    family
        .members
        .iter()
        .filter(|member| !member.is_self && member.appids.contains(&appid))
        .map(|member| FamilyVersion {
            steam_id: member.steam_id,
            name: if member.name.trim().is_empty() {
                lapinux_core::i18n::t("profile.family_member").to_string()
            } else {
                member.name.clone()
            },
            avatar: member.avatar.clone(),
            games: member.games,
            texture: None,
            personal: false,
            dlc_names: Vec::new(),
        })
        .collect()
}

/// Un snapshot social peut précéder la famille (reconnexion) ou arriver
/// pendant une pause du catalogue : il ne doit pas effacer le propriétaire
/// déjà affiché. Un état vivant sans famille, lui, dit que le groupe a
/// réellement disparu. Les membres reviennent souvent avec leur seul
/// SteamID : leurs appids, leur nom et leur photo sont repris du snapshot
/// précédent.
fn keep_family_through_snapshot(
    previous: Option<&SteamSocial>,
    social: &mut SteamSocial,
    keep_missing: bool,
) {
    let Some(previous_family) = previous.and_then(|old| old.family.as_ref()) else {
        return;
    };
    if social.family.is_none() {
        if !social.live() || keep_missing {
            social.family = Some(previous_family.clone());
        }
        return;
    }
    let Some(family) = social.family.as_mut() else {
        return;
    };
    for member in &mut family.members {
        let Some(old) = previous_family
            .members
            .iter()
            .find(|old| old.steam_id == member.steam_id)
        else {
            continue;
        };
        if member.appids.is_empty() {
            member.appids = old.appids.clone();
            if member.games == 0 {
                member.games = old.games;
            }
        }
        if member.name.trim().is_empty() {
            member.name = old.name.clone();
        }
        if member.avatar.trim().is_empty() {
            member.avatar = old.avatar.clone();
        }
    }
}

/// Ce que l'encart propose selon l'état du jeu : les trois états occupent la
/// même ligne d'actions, à la même place.
enum CardButtons {
    /// Installé : jouer, régler, désinstaller — et mettre à jour quand Steam
    /// publie un build plus récent que celui du disque.
    Installed {
        /// Un build plus récent est publié : la rangée gagne le bouton de mise
        /// à jour, à la place qu'occupait le rail d'informations.
        update: bool,
    },
    /// Absent du disque : télécharger, avec le poids de la version retenue et
    /// le nombre de contenus additionnels retenus.
    Download {
        /// Jeu concerné : identifie le menu de version, qui survit d'une image
        /// à l'autre.
        appid: u32,
        /// Octets à télécharger pour la version retenue, contenus compris.
        size: Option<u64>,
        dlc: usize,
        /// Versions publiées par le jeu, dans l'ordre proposé : Linux,
        /// Windows, puis macOS. Vide quand Steam ne les publie pas (fiche pas
        /// encore relue) : le bouton garde alors son apparence d'origine.
        versions: Vec<GameVersion>,
        /// Versions mobiles du jeu, montrées dans le menu pour information :
        /// elles ne se téléchargent pas sur PC.
        mobile: Vec<GameVersion>,
        /// Index de la version retenue dans `versions`.
        chosen: usize,
        /// Membres de la famille qui partagent ce jeu. Vide pour un jeu
        /// possédé : le bouton de version garde alors sa forme d'origine.
        family: Vec<FamilyVersion>,
        /// Index du membre retenu dans `family`, 0 quand la liste est vide.
        family_chosen: usize,
        /// Contenus additionnels du jeu, nommés pour le menu de la famille.
        dlc_names: Vec<String>,
        /// D'où part le téléchargement : Steam a son propre moteur, une
        /// boutique passe par son API. Le dessin est le même — libellé,
        /// flèche de version, bouton DLC, empilement, poids — seule l'action
        /// du clic change de camp.
        steam: bool,
    },
    /// Publié uniquement sur mobile : le menu montre Android/iOS, mais aucun
    /// téléchargement PC n'est proposé.
    Mobile {
        appid: u32,
        versions: Vec<GameVersion>,
    },
    /// Installé par le client Steam mais inconnu de Lapinux : le relire
    /// l'inscrit dans la bibliothèque, et la fiche suivante le proposera.
    Rescan,
    /// En cours de téléchargement : l'icône du jeu, la barre, le pourcentage,
    /// puis la croix qui annule la tâche.
    Progress {
        fraction: f32,
        job: String,
        appid: u32,
    },
    /// Hors du compte Steam : rien à télécharger ni à lancer.
    Unavailable,
}

/// Ce qu'un clic dans l'encart demande : changer la jaquette, jouer,
/// télécharger, régler, désinstaller, choisir des contenus, annuler. Les
/// actions sont appliquées après la peinture, une fois l'emprunt relâché.
#[derive(Default)]
struct CardAction {
    choose_cover: bool,
    reset_cover: bool,
    open_folder: Option<PathBuf>,
    play: bool,
    install: bool,
    /// Installation demandée à la boutique du jeu, pas à Steam.
    store_install: bool,
    /// Version choisie dans le menu de la flèche : elle rejoint la mémoire de
    /// la fenêtre et le bouton reprend le poids de cette version.
    version: Option<&'static str>,
    /// Membre de la famille retenu pour ce jeu : l'avatar du bouton change.
    family_owner: Option<u64>,
    rescan: bool,
    settings: bool,
    uninstall: bool,
    contents: bool,
    cancel: Option<String>,
    /// Hauteur que l'encart a réellement occupée, cadre compris : le résumé
    /// s'y arrête pour que les deux cartes de la ligne finissent ensemble.
    height: f32,
}

/// Icône dessinée de l'encart : engrenage ou poubelle. Le sous-ensemble Remix
/// embarqué ne contient ni l'un ni l'autre, et un glyphe manquant s'afficherait
/// en carré vide : ils sont tracés comme les autres pictogrammes maison.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CardGlyph {
    Gear,
    Trash,
    /// Flèche circulaire : télécharge le build Steam le plus récent.
    Refresh,
    /// Dossier ouvert : montre les fichiers du jeu sur le disque.
    Folder,
    /// Coche : confirme une suppression armée par la poubelle.
    Check,
}

/// Bouton carré à icône dessinée, de la même hauteur que les autres boutons de
/// la ligne d'actions.
///
/// Il porte son propre cadre, comme « Jouer » : trois boutons de même taille
/// sur la rangée, et non un bouton suivi de deux pictogrammes nus.
fn card_glyph_button(ui: &mut egui::Ui, glyph: CardGlyph, hint: &str) -> egui::Response {
    let size = CARD_GLYPH_SIZE;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click());
    let hovered = response.hovered();
    let confirming = glyph == CardGlyph::Check;
    let fill = if confirming {
        with_alpha(DANGER, if hovered { 0.24 } else { 0.16 })
    } else if hovered {
        CARD_HOVER()
    } else {
        ui.visuals().widgets.inactive.weak_bg_fill
    };
    let rounding = ui.style().visuals.widgets.inactive.rounding;
    let border = if confirming {
        with_alpha(DANGER, if hovered { 0.9 } else { 0.6 })
    } else {
        BORDER()
    };
    ui.painter()
        .rect(rect, rounding, fill, egui::Stroke::new(1.0_f32, border));
    let ink = match (glyph, hovered) {
        (CardGlyph::Trash, true) | (CardGlyph::Check, _) => DANGER,
        (_, true) => TEXT(),
        _ => TEXT_DIM(),
    };
    paint_card_glyph(
        ui.painter(),
        rect.center(),
        glyph,
        egui::Stroke::new(1.4_f32, ink),
        ink,
    );
    response.on_hover_text(hint)
}

/// Tracé commun des pictogrammes d'encart : la même forme sert au bouton
/// encadré d'une carte et à l'icône nue d'une ligne de liste.
fn paint_card_glyph(
    painter: &egui::Painter,
    center: egui::Pos2,
    glyph: CardGlyph,
    stroke: egui::Stroke,
    ink: egui::Color32,
) {
    match glyph {
        CardGlyph::Gear => {
            painter.circle_stroke(center, 6.0, stroke);
            painter.circle_stroke(center, 2.2, stroke);
            for index in 0..8 {
                let angle = index as f32 / 8.0 * std::f32::consts::TAU;
                let (sin, cos) = angle.sin_cos();
                let direction = egui::vec2(cos, sin);
                painter.line_segment([center + direction * 6.0, center + direction * 8.4], stroke);
            }
        }
        CardGlyph::Trash => {
            // Couvercle, poignée, corps resserré, deux stries intérieures.
            painter.line_segment(
                [
                    center + egui::vec2(-6.4, -3.8),
                    center + egui::vec2(6.4, -3.8),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    center + egui::vec2(-2.2, -6.2),
                    center + egui::vec2(2.2, -6.2),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    center + egui::vec2(-2.2, -6.2),
                    center + egui::vec2(-2.2, -3.8),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    center + egui::vec2(2.2, -6.2),
                    center + egui::vec2(2.2, -3.8),
                ],
                stroke,
            );
            painter.add(egui::Shape::line(
                vec![
                    center + egui::vec2(-5.0, -3.8),
                    center + egui::vec2(-4.2, 6.2),
                    center + egui::vec2(4.2, 6.2),
                    center + egui::vec2(5.0, -3.8),
                ],
                stroke,
            ));
            for x in [-1.9_f32, 1.9] {
                painter.line_segment(
                    [center + egui::vec2(x, -1.4), center + egui::vec2(x, 3.6)],
                    stroke,
                );
            }
        }
        CardGlyph::Folder => {
            // Un dossier entrouvert : la languette en haut à gauche, le corps
            // et le rabat incliné qui dit qu'il est ouvert.
            painter.add(egui::Shape::line(
                vec![
                    center + egui::vec2(-6.4, -4.4),
                    center + egui::vec2(-6.4, 5.6),
                    center + egui::vec2(6.4, 5.6),
                    center + egui::vec2(6.4, -2.0),
                ],
                stroke,
            ));
            painter.add(egui::Shape::line(
                vec![
                    center + egui::vec2(-6.4, -4.4),
                    center + egui::vec2(-2.2, -4.4),
                    center + egui::vec2(-1.2, -2.6),
                    center + egui::vec2(2.0, -2.6),
                ],
                stroke,
            ));
            painter.line_segment(
                [
                    center + egui::vec2(-4.6, 3.4),
                    center + egui::vec2(6.4, 3.4),
                ],
                stroke,
            );
        }
        CardGlyph::Check => {
            // Deux traits, celui de gauche court : la coche se lit même à
            // 22 px, dans l'icône comme dans son bouton.
            painter.add(egui::Shape::line(
                vec![
                    center + egui::vec2(-4.8, 0.2),
                    center + egui::vec2(-1.2, 3.8),
                    center + egui::vec2(5.0, -4.2),
                ],
                stroke,
            ));
        }
        CardGlyph::Refresh => {
            // Un tour presque complet, puis la pointe de la flèche : le sens
            // se lit même à 26 px.
            let radius = 5.6;
            let arc: Vec<egui::Pos2> = (0..=26)
                .map(|index| {
                    let angle = -0.6 + index as f32 / 26.0 * std::f32::consts::TAU * 0.82;
                    center + egui::vec2(angle.cos(), angle.sin()) * radius
                })
                .collect();
            let end = *arc.last().expect("arc tracé");
            painter.add(egui::Shape::line(arc, egui::Stroke::new(1.5_f32, ink)));
            painter.add(egui::Shape::convex_polygon(
                vec![
                    end + egui::vec2(-1.3, -3.6),
                    end + egui::vec2(3.2, 0.2),
                    end + egui::vec2(-2.6, 2.6),
                ],
                ink,
                egui::Stroke::NONE,
            ));
        }
    }
}

/// Icône nue d'une ligne de liste : pas de cadre, un fond au survol — la
/// poubelle et le dossier d'un Proton se lisent comme des commandes, sans
/// ajouter deux boutons pleins à chaque ligne.
fn row_glyph_button(ui: &mut egui::Ui, glyph: CardGlyph, hint: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::click());
    let hovered = response.hovered();
    if hovered {
        ui.painter()
            .rect_filled(rect, egui::Rounding::same(6.0), CARD_HOVER());
    }
    let ink = match (glyph, hovered) {
        (CardGlyph::Trash, true) => DANGER,
        (_, true) => TEXT(),
        _ => TEXT_DIM(),
    };
    paint_card_glyph(
        ui.painter(),
        rect.center(),
        glyph,
        egui::Stroke::new(1.4_f32, ink),
        ink,
    );
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(hint)
}

/// Poids total des Protons de la liste, en une ligne : les mesures arrivent au
/// fil de l'eau, donc tant que la liste n'est pas entièrement pesée, la ligne
/// le dit au lieu d'annoncer un total partiel.
fn proton_weight_line(protons: &[ProtonInfo], sizes: &HashMap<PathBuf, u64>) -> String {
    if protons.is_empty() {
        return String::new();
    }
    let builds = protons.len();
    let mut total = 0u64;
    let mut measured = 0usize;
    for proton in protons {
        if let Some(bytes) = sizes.get(&proton.path) {
            total += bytes;
            measured += 1;
        }
    }
    if measured == 0 {
        return lapinux_core::i18n::t("protons.weight_measuring").to_string();
    }
    let weight = lapinux_core::util::format_bytes(total);
    if measured < builds {
        lapinux_core::i18n::t_args(
            "protons.weight_partial",
            &[
                ("weight", &weight),
                ("measured", &measured.to_string()),
                ("builds", &builds.to_string()),
            ],
        )
    } else {
        lapinux_core::i18n::t_args(
            "protons.weight_total",
            &[("weight", &weight), ("builds", &builds.to_string())],
        )
    }
}

/// Icône de confirmation : elle prend la place de la poubelle après le
/// premier clic, sur un fond d'alerte. Un second clic supprime, sans fenêtre.
fn row_confirm_button(ui: &mut egui::Ui, hint: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::click());
    let alpha = if response.hovered() { 0.24 } else { 0.16 };
    ui.painter()
        .rect_filled(rect, egui::Rounding::same(6.0), with_alpha(DANGER, alpha));
    paint_card_glyph(
        ui.painter(),
        rect.center(),
        CardGlyph::Check,
        egui::Stroke::new(1.6_f32, DANGER),
        DANGER,
    );
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(hint)
}

/// Provenance du jeu, en icônes : le logo Steam quand le jeu vient du compte,
/// puis une personne pour le compte personnel ou deux pour la famille. Un jeu
/// du disque montre un disque. Toutes sont dessinées au trait, comme sur les
/// jaquettes. L'infobulle porte ce que les icônes ne disent pas.
/// Faits du compte sous les icônes : temps de jeu, puis poids sur le disque,
/// chacun devant son pictogramme.
///
/// Le temps s'affiche même pour un jeu absent du disque — c'est le compte qui
/// le sait, pas les fichiers — et « Jamais joué » remplace une absence de
/// chiffre. Le poids, lui, est celui du disque pour un jeu installé et celui
/// que Steam publie pour un jeu qui ne l'est pas encore : on sait ainsi ce
/// qu'il prendra. Une ligne sans rien ne s'affiche pas du tout.
fn facts_line(ui: &mut egui::Ui, played: &str, size: &str, hint: &str) {
    let played = played.trim();
    let size = size.trim();
    if played.is_empty() && size.is_empty() {
        return;
    }
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        if !played.is_empty() {
            // Les heures et leur cadran se survolent ensemble : l'infobulle
            // détaille ce que le compte publie, par plateforme.
            let group = ui
                .horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    fact_mark(ui, FactMark::Clock);
                    ui.label(egui::RichText::new(played).size(11.5).color(TEXT_DIM()));
                })
                .response;
            if !hint.is_empty() {
                group.on_hover_text(hint);
            }
        }
        if !size.is_empty() {
            if !played.is_empty() {
                ui.add_space(7.0);
            }
            fact_mark(ui, FactMark::Disk);
            ui.label(egui::RichText::new(size).size(11.5).color(TEXT_DIM()));
        }
    });
}

/// Phrase de la fiche : ce que le jeu demande de Steam.
fn local_drm_sentence(drm: lapinux_core::model::Drm) -> Option<&'static str> {
    use lapinux_core::model::Drm;
    match drm {
        Drm::None => Some(lapinux_core::i18n::t("game_sheet.drm_free")),
        Drm::Steamworks => Some(lapinux_core::i18n::t("game_sheet.drm_steamworks")),
        Drm::SteamStub => Some(lapinux_core::i18n::t("game_sheet.drm_requires_steam")),
        Drm::Unknown => None,
    }
}

/// Phrase d'un jeu de boutique : le dossier installé d'abord, puis ce que la
/// boutique garantit pour un jeu qui n'est pas encore sur le disque.
fn store_drm_sentence(
    store: lapinux_core::stores::Store,
    report: Option<&drm::Report>,
) -> Option<String> {
    use lapinux_core::stores::Store;
    if let Some(report) = report {
        return match report.drm {
            None => Some(lapinux_core::i18n::t("game_sheet.drm_free").to_string()),
            Some("Steam") if report.requires_steam => Some(
                lapinux_core::i18n::t("game_sheet.drm_requires_steam").to_string(),
            ),
            Some("Steam") => {
                Some(lapinux_core::i18n::t("game_sheet.drm_steamworks").to_string())
            }
            Some(name) => Some(lapinux_core::i18n::t_args(
                "game_sheet.drm_protection",
                &[("name", name)],
            )),
        };
    }
    match store {
        Store::Gog | Store::Itch => {
            Some(lapinux_core::i18n::t("game_sheet.drm_free").to_string())
        }
        // Epic est un client, comme Steam : sans dossier lu, la fiche annonce
        // l'exigence de la boutique, pas une absence de Steam.
        Store::Epic => Some(lapinux_core::i18n::t_args(
            "game_sheet.drm_requires_store",
            &[("store", "Epic Games")],
        )),
        Store::Steam => None,
    }
}

/// Pictogramme d'un fait de l'encart, tracé au trait comme les autres marques
/// maison (la police livrée n'a pas d'icônes fiables).
enum FactMark {
    Clock,
    Disk,
}

fn fact_mark(ui: &mut egui::Ui, mark: FactMark) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(13.0, 13.0), egui::Sense::hover());
    match mark {
        FactMark::Clock => paint_clock_icon(ui.painter(), rect.center(), TEXT_DIM(), 1.0),
        FactMark::Disk => paint_disk_icon(ui.painter(), rect.center(), TEXT_DIM(), 0.78),
    }
}

/// Cadran : un cercle et deux aiguilles, midi et trois heures.
fn paint_clock_icon(painter: &egui::Painter, center: egui::Pos2, color: egui::Color32, scale: f32) {
    let stroke = egui::Stroke::new(1.2_f32 * scale, color);
    painter.circle_stroke(center, 5.0 * scale, stroke);
    painter.line_segment([center, center - egui::vec2(0.0, 2.9 * scale)], stroke);
    painter.line_segment(
        [center, center + egui::vec2(2.5 * scale, 0.9 * scale)],
        stroke,
    );
}

/// Infobulle du temps de jeu : ce que le compte publie, plateforme par
/// plateforme. Les lignes à zéro ne s'écrivent pas — une plateforme jamais
/// utilisée n'a rien à dire. Avec seulement le manifeste local, la ligne le
/// nomme, pour ne pas faire passer un fichier pour un total Steam.
fn playtime_tooltip(
    account: Option<&lapinux_core::model::SteamAccountPlay>,
    local_secs: Option<u64>,
) -> String {
    let mut lines: Vec<String> = Vec::new();
    if let Some(account) = account {
        let mut push = |label: &str, minutes: Option<u64>| {
            if let Some(minutes) = minutes.filter(|minutes| *minutes > 0) {
                lines.push(format!(
                    "{label} · {}",
                    lapinux_core::util::format_duration(minutes.saturating_mul(60))
                ));
            }
        };
        push("Total Steam", account.minutes_total);
        push(
            lapinux_core::i18n::t("gameinfo.last_two_weeks"),
            account.minutes_2weeks,
        );
        push("Linux", account.minutes_linux);
        push("Windows", account.minutes_windows);
        push("macOS", account.minutes_macos);
        push("Steam Deck", account.minutes_deck);
    }
    if lines.is_empty() {
        if let Some(secs) = local_secs.filter(|secs| *secs > 0) {
            lines.push(format!(
                "Fichiers locaux · {}",
                lapinux_core::util::format_duration(secs)
            ));
        }
    }
    lines.join("\n")
}

/// Temps de jeu tel que l'encart l'écrit : la durée seule, « Jamais joué »
/// quand le compte n'a aucune minute, rien quand personne ne l'a dit.
fn played_label(minutes: Option<u64>) -> String {
    match minutes {
        Some(minutes) if minutes > 0 => {
            lapinux_core::util::format_duration(minutes.saturating_mul(60))
        }
        Some(_) => lapinux_core::i18n::t("library.never_played_game").to_string(),
        None => String::new(),
    }
}

fn origin_marks(
    ui: &mut egui::Ui,
    steam: Option<bool>,
    // Boutique du jeu quand il n'est pas sur Steam : sa marque remplace
    // l'icône du disque, comme la marque Steam à sa place.
    store: Option<lapinux_core::stores::Store>,
    detail: &str,
    played: &str,
    cloud: bool,
) -> egui::Response {
    let size = 15.0;
    let gap = 7.0;
    let count = if steam.is_some() { 2 } else { 1 } + 1;
    let icons = count as f32 * size + (count - 1) as f32 * gap;
    // L'état des sauvegardes cloud se lit en toutes lettres à côté du nuage :
    // une icône barrée seule demande de connaître la convention.
    let cloud_word = ui.fonts(|fonts| {
        fonts.layout_no_wrap(
            "Cloud".to_string(),
            egui::FontId::proportional(11.0),
            if cloud { OK } else { TEXT_DIM() },
        )
    });
    let cloud_gap = 4.0;
    let marks = icons + cloud_gap + cloud_word.size().x;
    // Les heures de jeu suivent la provenance du jeu : elles se lisent comme
    // un fait du compte, pas comme une colonne d'informations à part.
    let available = ui.available_width();
    let label = (!played.trim().is_empty())
        .then(|| {
            let font = egui::FontId::proportional(11.5);
            ui.fonts(|fonts| fonts.layout_no_wrap(played.to_string(), font, TEXT_DIM()))
        })
        // Sur une carte serrée, les heures n'ont pas la place : elles passent
        // plutôt que de déborder de l'encart.
        .filter(|galley| marks + 8.0 + galley.size().x <= available);
    let text_gap = if label.is_some() { 8.0 } else { 0.0 };
    let width = marks + text_gap + label.as_ref().map(|galley| galley.size().x).unwrap_or(0.0);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, size), egui::Sense::hover());
    let mut left = rect.left() + size / 2.0;
    let center_y = rect.center().y;
    match steam {
        Some(family) => {
            paint_steam_icon(
                ui.painter(),
                egui::pos2(left, center_y),
                egui::Color32::from_rgb(120, 170, 240),
                1.0,
            );
            left += size + gap;
            if family {
                paint_family_icon(
                    ui.painter(),
                    egui::pos2(left, center_y),
                    origin_color(Some(true)),
                    0.62,
                );
            } else {
                paint_person_icon(
                    ui.painter(),
                    egui::pos2(left, center_y),
                    origin_color(Some(false)),
                    0.68,
                );
            }
        }
        None => match store {
            Some(store) => {
                let mark = egui::Rect::from_center_size(
                    egui::pos2(left, center_y),
                    egui::vec2(size - 2.0, size - 2.0),
                );
                paint_store_mark_in(ui.painter(), store, mark, origin_color(None));
            }
            None => paint_disk_icon(
                ui.painter(),
                egui::pos2(left, center_y),
                origin_color(None),
                0.85,
            ),
        },
    }
    // Les sauvegardes cloud : la marque du nuage, à la suite de la provenance
    // du jeu. Elle dit aussi l'état — barrée et grise quand elles sont
    // désactivées — pour que l'information se lise sans ouvrir les réglages.
    left += size + gap;
    let cloud_center = egui::pos2(left, center_y);
    crate::gameinfo::paint_feature_icon(
        ui.painter(),
        cloud_center,
        13.5,
        if cloud { OK } else { TEXT_DIM() },
        crate::gameinfo::FeatureIcon::Cloud,
    );
    if !cloud {
        let half = 5.5;
        ui.painter().line_segment(
            [
                cloud_center + egui::vec2(-half, half),
                cloud_center + egui::vec2(half, -half),
            ],
            egui::Stroke::new(1.4_f32, egui::Color32::from_rgb(120, 124, 130)),
        );
    }
    ui.painter().galley(
        egui::pos2(
            rect.left() + icons + cloud_gap,
            center_y - cloud_word.size().y / 2.0,
        ),
        cloud_word,
        if cloud { OK } else { TEXT_DIM() },
    );
    if let Some(galley) = label {
        let pos = egui::pos2(
            rect.left() + marks + text_gap,
            center_y - galley.size().y / 2.0,
        );
        ui.painter().galley(pos, galley, TEXT_DIM());
    }
    response.on_hover_text(lapinux_core::i18n::t_args(
        "game_sheet.cloud_state_tooltip",
        &[
            ("detail", &detail),
            (
                "state",
                if cloud {
                    lapinux_core::i18n::t("game_sheet.enabled")
                } else {
                    lapinux_core::i18n::t("game_sheet.disabled")
                },
            ),
        ],
    ))
}

/// Encart d'identité de la fiche : la jaquette, le nom, la provenance du jeu
/// et l'action du moment, dans une carte au fond distinct — le même bloc que
/// les succès, à gauche. La jaquette prend la hauteur de la carte et les
/// actions se rangent à sa droite, sous le nom ; quand la colonne de texte ne
/// peut pas les porter, elles reprennent le pied de la carte.
#[allow(clippy::too_many_arguments)]
fn identity_card(
    ui: &mut egui::Ui,
    width: f32,
    height: f32,
    appid: u32,
    name: &str,
    texture: &Option<egui::TextureHandle>,
    // Icône de bibliothèque du jeu : c'est elle qui s'affiche dans la barre
    // de transfert, où une jaquette recadrée en 22 px ne se lisait pas.
    icon: &Option<egui::TextureHandle>,
    has_custom: bool,
    install_dir: Option<&Path>,
    steam: Option<bool>,
    // Un autre membre de la famille partage aussi ce jeu : les deux accès
    // restent affichés, même quand la licence personnelle est retenue.
    family_available: bool,
    // Boutique du jeu quand il n'est pas sur Steam.
    store: Option<lapinux_core::stores::Store>,
    show_badges: bool,
    badge_scale: u32,
    detail: &str,
    played: &str,
    // Détail du temps de jeu, affiché au survol : ce que le compte publie pour
    // chaque plateforme.
    played_hint: &str,
    // Poids, déjà mis en forme ; celui du disque ou celui qui sera pris.
    size: &str,
    // Le jeu synchronise-t-il ses sauvegardes sur le cloud Steam ?
    cloud: bool,
    // Phrase de protection : Steamworks, Steam requis ou DRM-free.
    drm: Option<&str>,
    buttons: &CardButtons,
    uninstall_armed: bool,
) -> CardAction {
    let mut action = CardAction::default();
    let inner = (width - 20.0).max(80.0);
    // La jaquette garde ses proportions 2:3 : sa hauteur fixe sa largeur. Elle
    // monte jusqu'à la hauteur de la carte, en laissant toujours à la colonne
    // de texte de quoi porter le nom et ses actions. Ce repère ne dépend que
    // de la carte : l'état du jeu peut changer, la jaquette ne bouge pas —
    // c'est ce qui la laisse en place à l'instant où un téléchargement
    // démarre.
    let beside_height = (height - 20.0)
        .min(COVER_BESIDE_HEIGHT)
        .min((inner - CARD_TEXT_MIN - 10.0) * 87.0 / SHEET_COVER_WIDTH);
    let beside = beside_height >= COVER_BESIDE_MIN;
    let cover_height = if beside {
        beside_height
    } else {
        (height - 54.0).clamp(56.0, 87.0)
    };
    let cover_width = (cover_height * SHEET_COVER_WIDTH / 87.0).round();
    let text = (inner - cover_width - 10.0).max(40.0);
    // Sans pied, la carte s'arrête sous la jaquette : l'espace laissé libre
    // reste disponible pour de petites informations.
    let card_height = cover_height + if beside { 20.0 } else { 34.0 };
    // La rangée d'actions est mesurée sur la colonne qui la porte : à droite
    // de la jaquette quand la fiche est assez large, en pied de carte sinon.
    let contents = matches!(buttons, CardButtons::Download { dlc, .. } if *dlc > 0);
    // Le bouton d'avatar de la famille se réserve à droite de la flèche :
    // sans cette place, la rangée déborderait de son bloc.
    let family = matches!(buttons, CardButtons::Download { family, .. } if !family.is_empty());
    let family_width = if family { FAMILY_BUTTON_WIDTH } else { 0.0 };
    // La flèche du choix de version fait partie du bouton : sa largeur se
    // réserve avant le poids, sinon le libellé serait coupé.
    let arrow = match buttons {
        CardButtons::Download { versions, .. } if !versions.is_empty() => VERSION_ARROW_WIDTH,
        CardButtons::Mobile { versions, .. } if !versions.is_empty() => VERSION_ARROW_WIDTH,
        _ => 0.0,
    };
    let row_inner = if beside { text } else { inner };
    // Dans la colonne de texte, tous les états du jeu occupent la même
    // largeur : un bloc d'actions franc, au lieu de boutons qui se rétrécissent
    // chacun à leur libellé et laissaient le vide à droite. En pied de carte,
    // la carte est étroite : la rangée suit la place disponible.
    let block = if beside {
        row_inner.min(CARD_ACTIONS_WIDTH)
    } else {
        0.0
    };
    let base_width = match buttons {
        // Largeur du contrôle entier, flèche comprise : `split` en retire la
        // flèche pour le bouton du poids.
        CardButtons::Download { .. } if beside => (block - family_width).max(60.0),
        CardButtons::Download { .. } => {
            (row_inner - if contents { DLC_BUTTON + 10.0 } else { 0.0 } - arrow - family_width)
                .clamp(120.0, 186.0)
        }
        CardButtons::Mobile { .. } if beside => block.max(60.0),
        CardButtons::Mobile { .. } => (row_inner - arrow).clamp(120.0, 186.0),
        _ => 0.0,
    };
    let row_width = match buttons {
        CardButtons::Unavailable | CardButtons::Progress { .. } => inner,
        CardButtons::Installed { .. } if beside => block,
        CardButtons::Installed { update } => {
            104.0
                + CARD_GLYPH_SIZE
                + CARD_GLYPH_SIZE
                + if *update { CARD_GLYPH_SIZE } else { 0.0 }
                + 20.0
        }
        CardButtons::Rescan if beside => block,
        CardButtons::Rescan => 150.0_f32.min(inner),
        CardButtons::Download { .. } if beside => block,
        CardButtons::Download { .. } => {
            base_width + arrow + family_width + if contents { DLC_BUTTON + 10.0 } else { 0.0 }
        }
        CardButtons::Mobile { .. } if beside => block,
        CardButtons::Mobile { .. } => base_width + arrow,
    };
    // Le bloc du téléchargement porte le poids du jeu et sa flèche : les
    // contenus passent dessous plutôt que de le rétrécir.
    let stacked = beside && contents;
    let row = CardRow {
        width: row_inner,
        block,
        // La place libre devant la rangée ne se pose qu'en pied de carte : les
        // actions rangées à droite de la jaquette partent du bord de la
        // colonne de texte.
        pad: if beside {
            0.0
        } else {
            (inner - row_width).max(0.0)
        },
        stacked,
        // Une ligne, ou deux quand les boutons s'empilent.
        height: if stacked {
            CARD_ROW_STACKED_HEIGHT
        } else {
            CARD_ROW_HEIGHT
        },
    };
    let framed = egui::Frame::none()
        .fill(CARD())
        .rounding(egui::Rounding::same(10.0))
        .inner_margin(egui::Margin::same(10.0))
        .show(ui, |ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(inner, card_height.max(60.0)),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(inner);
                    ui.spacing_mut().item_spacing.x = 10.0;
                    ui.spacing_mut().item_spacing.y = 4.0;
                    ui.horizontal_top(|ui| {
                        let (rect, response) = ui.allocate_exact_size(
                            egui::vec2(cover_width, cover_height),
                            egui::Sense::click(),
                        );
                        match texture {
                            Some(texture) => {
                                paint_cover(ui, rect, texture, egui::Rounding::same(8.0))
                            }
                            None => {
                                paint_cover_placeholder(ui, rect, name, egui::Rounding::same(8.0))
                            }
                        }
                        if show_badges {
                            paint_origin_badges(
                                ui,
                                rect,
                                steam,
                                family_available,
                                store,
                                badge_scale,
                            );
                        }
                        // Cette carte sait déjà si le jeu est installé : le
                        // menu propose donc « Jouer » ou « Installer » sans
                        // ouvrir la fiche.
                        let installed = matches!(buttons, CardButtons::Installed { .. });
                        let (pick, reset, folder, _, _) = cover_context_menu(
                            response,
                            appid,
                            name,
                            icon.as_ref(),
                            has_custom,
                            installed,
                            false,
                            install_dir,
                            &[],
                            None,
                        );
                        action.choose_cover = pick.is_some();
                        action.reset_cover = reset.is_some();
                        if folder.is_some() {
                            action.open_folder = folder;
                        }

                        // Carte courte (pied de carte ou fenêtre basse) : la
                        // ligne ne tient pas trois lignes de texte à la taille
                        // de la fiche repliée.
                        let compact = cover_height < 80.0;
                        ui.allocate_ui_with_layout(
                            egui::vec2(text, cover_height),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                // Haut et bord de la colonne : le bas de la
                                // jaquette sert de repère pour y coller les
                                // actions.
                                let top = ui.min_rect().top();
                                let left = ui.min_rect().left();
                                ui.set_width(text);
                                ui.spacing_mut().item_spacing.x = 8.0;
                                ui.spacing_mut().item_spacing.y = if compact { 2.0 } else { 6.0 };
                                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                                ui.label(
                                    egui::RichText::new(name)
                                        .size(if compact { 15.5 } else { 17.0 })
                                        .strong()
                                        .color(TEXT()),
                                )
                                .on_hover_text(name);
                                // Sur une carte serrée, les faits restent sur la
                                // ligne des icônes : une ligne de plus passerait
                                // sous les boutons.
                                origin_marks(
                                    ui,
                                    steam,
                                    store,
                                    detail,
                                    if compact { played } else { "" },
                                    cloud,
                                );
                                if !compact {
                                    facts_line(ui, played, size, played_hint);
                                }
                                // La protection se lit sous les faits, avant
                                // l'action : elle ne dépend pas du bouton.
                                if beside {
                                    if let Some(drm) = drm {
                                        ui.add_space(2.0);
                                        ui.label(
                                            egui::RichText::new(drm).size(10.5).color(TEXT_DIM()),
                                        );
                                    }
                                }
                                // Les actions se rangent sous le nom, à droite
                                // de la jaquette, collées au bas de sa hauteur.
                                if beside {
                                    // Les actions tiennent le bas de la colonne,
                                    // à hauteur du bas de la jaquette. Leur
                                    // place est posée explicitement : ni les
                                    // écarts de la colonne ni la hauteur des
                                    // boutons ne peuvent les descendre sous la
                                    // carte.
                                    let actions = egui::Rect::from_min_size(
                                        egui::pos2(left, top + cover_height - row.height),
                                        egui::vec2(row.width, row.height),
                                    );
                                    ui.scope_builder(
                                        egui::UiBuilder::new()
                                            .max_rect(actions)
                                            .layout(egui::Layout::top_down(egui::Align::Min)),
                                        |ui| {
                                            card_actions_row(
                                                ui,
                                                buttons,
                                                row,
                                                base_width,
                                                texture,
                                                icon,
                                                install_dir,
                                                uninstall_armed,
                                                &mut action,
                                            )
                                        },
                                    );
                                }
                            },
                        );
                    });
                    // Le pied de carte : les actions n'y reviennent que quand
                    // la colonne de texte n'a pas pu les porter.
                    if !beside {
                        ui.add_space((ui.available_height() - CARD_ROW_HEIGHT).max(0.0));
                        card_actions_row(
                            ui,
                            buttons,
                            row,
                            base_width,
                            texture,
                            icon,
                            install_dir,
                            uninstall_armed,
                            &mut action,
                        );
                    }
                },
            );
        });
    action.height = framed.response.rect.height();
    action
}

/// Géométrie de la rangée d'actions dans l'encart : la colonne qui la porte,
/// la place libre devant elle, sa disposition et sa hauteur.
#[derive(Clone, Copy)]
struct CardRow {
    /// Largeur que la rangée peut occuper.
    width: f32,
    /// Largeur du bloc d'actions quand il se range dans la colonne de texte :
    /// la même pour jouer, télécharger ou resynchroniser. Nulle en pied de
    /// carte, où la rangée suit les boutons.
    block: f32,
    /// Place libre posée devant elle, en pied de carte.
    pad: f32,
    /// Téléchargement et contenus l'un sous l'autre : côte à côte, le poids du
    /// jeu n'aurait plus la place de s'écrire sur son bouton.
    stacked: bool,
    /// Hauteur de la rangée, pour la poser au bas de la jaquette.
    height: f32,
}

/// Actions de l'encart : jouer, régler et désinstaller ; télécharger, avec le
/// poids et les contenus ; resynchroniser ; ou la barre de transfert et sa
/// croix. Elles se rangent à droite de la jaquette, sous le nom, et reprennent
/// le pied de la carte quand la fiche est trop étroite pour deux colonnes.
fn card_actions_row(
    ui: &mut egui::Ui,
    buttons: &CardButtons,
    row: CardRow,
    base_width: f32,
    texture: &Option<egui::TextureHandle>,
    icon: &Option<egui::TextureHandle>,
    install_dir: Option<&Path>,
    uninstall_armed: bool,
    action: &mut CardAction,
) {
    match buttons {
        CardButtons::Unavailable => {}
        CardButtons::Installed { update } => {
            // Jouer prend la place qui reste dans le bloc : les boutons carrés
            // occupent la même largeur que le bouton de téléchargement, au
            // lieu de s'entasser à gauche.
            let mut icons = if *update { 3.0 } else { 2.0 };
            if install_dir.is_some() {
                icons += 1.0;
            }
            let play = if row.block > 0.0 {
                (row.block - CARD_GLYPH_SIZE * icons - 8.0 * icons).max(72.0)
            } else {
                104.0
            };
            ui.horizontal(|ui| {
                ui.add_space(row.pad);
                if ui
                    .add_sized(
                        [play, 30.0],
                        egui::Button::new(
                            egui::RichText::new(format!(
                                "▶  {}",
                                lapinux_core::i18n::t("library.action.play")
                            ))
                            .size(13.5)
                            .strong(),
                        ),
                    )
                    .clicked()
                {
                    action.play = true;
                }
                if *update
                    && card_glyph_button(
                        ui,
                        CardGlyph::Refresh,
                        lapinux_core::i18n::t("game_sheet.update_build_hint"),
                    )
                    .clicked()
                {
                    action.install = true;
                }
                if let Some(dir) = install_dir {
                    if card_glyph_button(
                        ui,
                        CardGlyph::Folder,
                        lapinux_core::i18n::t("game_sheet.open_game_folder"),
                    )
                    .clicked()
                    {
                        action.open_folder = Some(dir.to_path_buf());
                    }
                }
                if card_glyph_button(
                    ui,
                    CardGlyph::Gear,
                    lapinux_core::i18n::t("game_contents.settings"),
                )
                .clicked()
                {
                    action.settings = true;
                }
                let uninstall = if uninstall_armed {
                    card_glyph_button(
                        ui,
                        CardGlyph::Check,
                        lapinux_core::i18n::t("game_sheet.confirm_uninstall"),
                    )
                } else {
                    card_glyph_button(
                        ui,
                        CardGlyph::Trash,
                        lapinux_core::i18n::t("game_contents.uninstall"),
                    )
                };
                if uninstall.clicked() {
                    action.uninstall = true;
                }
            });
        }
        CardButtons::Rescan => {
            let width = if row.block > 0.0 {
                row.block
            } else {
                150.0_f32.min(row.width)
            };
            ui.horizontal(|ui| {
                ui.add_space(row.pad);
                if ui
                    .add_sized(
                        [width, 30.0],
                        egui::Button::new(
                            egui::RichText::new(lapinux_core::i18n::t("game_sheet.sync_library"))
                                .size(13.0)
                                .strong(),
                        ),
                    )
                    .on_hover_text(lapinux_core::i18n::t("game_sheet.sync_library_hint"))
                    .clicked()
                {
                    action.rescan = true;
                }
            });
        }
        CardButtons::Mobile { appid, versions } => {
            let arrow = if versions.is_empty() {
                0.0
            } else {
                VERSION_ARROW_WIDTH
            };
            let width = if row.stacked { row.block } else { base_width };
            let main = if arrow > 0.0 {
                (width - arrow).max(60.0)
            } else {
                width
            };
            ui.horizontal(|ui| {
                ui.add_space(row.pad);
                ui.spacing_mut().item_spacing.x = 0.0;
                let rounding = ui.style().visuals.widgets.inactive.rounding;
                let left = egui::Rounding {
                    nw: rounding.nw,
                    sw: rounding.sw,
                    ne: 0.0,
                    se: 0.0,
                };
                let right = egui::Rounding {
                    nw: 0.0,
                    sw: 0.0,
                    ne: rounding.ne,
                    se: rounding.se,
                };
                let fill = ui.style().visuals.widgets.inactive.weak_bg_fill;
                let stroke = ui.style().visuals.widgets.inactive.bg_stroke;
                let label = lapinux_core::i18n::t("game_sheet.mobile_only");
                let hint = lapinux_core::i18n::t("game_sheet.mobile_install_hint");
                let button = egui::Button::new(egui::RichText::new(label).size(12.5).strong())
                    .rounding(if arrow > 0.0 { left } else { rounding })
                    .fill(fill)
                    .stroke(stroke);
                let response = ui.add_sized([main, 30.0], button).on_hover_text(hint);
                let popup_id = ui.id().with(("mobile-version", *appid));
                if arrow <= 0.0 {
                    return;
                }
                let chevron = ui
                    .add_sized(
                        [arrow, 30.0],
                        egui::Button::new("")
                            .rounding(right)
                            .fill(fill)
                            .stroke(stroke),
                    )
                    .on_hover_text(hint);
                paint_mobile_icon(
                    ui.painter(),
                    chevron.rect.center(),
                    if chevron.hovered() {
                        TEXT()
                    } else {
                        TEXT_DIM()
                    },
                    0.85,
                );
                if response.clicked() || chevron.clicked() {
                    ui.memory_mut(|memory| memory.toggle_popup(popup_id));
                }
                version_menu(ui, popup_id, &chevron, &[], versions, 0, action);
            });
        }
        CardButtons::Download {
            appid,
            size,
            dlc,
            versions,
            mobile,
            chosen,
            family,
            family_chosen,
            dlc_names,
            steam,
        } => {
            let contents = *dlc > 0;
            let label = match size {
                Some(bytes) => {
                    lapinux_core::i18n::t_args(
                        "game_sheet.download_size",
                        &[("size", &lapinux_core::util::format_bytes(*bytes))],
                    )
                }
                None => lapinux_core::i18n::t("workshop.download").to_string(),
            };
            let contents_label = format!("DLC · {dlc}");
            let contents_button = |ui: &mut egui::Ui, width: f32| -> bool {
                ui.add_sized(
                    [width, 30.0],
                    egui::Button::new(egui::RichText::new(contents_label.as_str()).size(12.0)),
                )
                .on_hover_text(lapinux_core::i18n::t("game_sheet.choose_dlc"))
                .clicked()
            };
            // Le poids s'écrit toujours sur le bouton : empilés, les deux
            // boutons prennent la largeur de la colonne entière.
            let family_width = if family.is_empty() {
                0.0
            } else {
                FAMILY_BUTTON_WIDTH
            };
            // Empilés, les boutons prennent le bloc entier : le bouton
            // familial se retire du contrôle de version, pas de la carte.
            let width = if row.stacked {
                (row.block - family_width).max(60.0)
            } else {
                base_width
            };
            let arrow = if versions.is_empty() {
                0.0
            } else {
                VERSION_ARROW_WIDTH
            };
            // Télécharger et sa flèche ne font qu'un bouton : le bord commun
            // ne s'arrondit pas, l'ensemble garde les coins du thème.
            let split = |ui: &mut egui::Ui, action: &mut CardAction| {
                // Les deux moitiés se touchent ; le reste de la rangée garde
                // l'écart du thème, sinon les contenus colleraient au poids.
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    let rounding = ui.style().visuals.widgets.inactive.rounding;
                    let left = egui::Rounding {
                        nw: rounding.nw,
                        sw: rounding.sw,
                        ne: 0.0,
                        se: 0.0,
                    };
                    let right = egui::Rounding {
                        nw: 0.0,
                        sw: 0.0,
                        ne: rounding.ne,
                        se: rounding.se,
                    };
                    let main = if arrow > 0.0 {
                        (width - arrow).max(60.0)
                    } else {
                        width
                    };
                    // Le contrôle est un seul bouton coupé en deux : survoler
                    // l'une des moitiés allume les deux, sinon la flèche de
                    // version restait éteinte au bord droit.
                    let whole =
                        egui::Rect::from_min_size(ui.cursor().min, egui::vec2(main + arrow, 30.0));
                    let lit = ui.rect_contains_pointer(whole);
                    let style = ui.style().clone();
                    let widgets = &style.visuals.widgets;
                    let fill = if lit {
                        widgets.hovered.weak_bg_fill
                    } else {
                        widgets.inactive.weak_bg_fill
                    };
                    let stroke = if lit {
                        widgets.hovered.bg_stroke
                    } else {
                        widgets.inactive.bg_stroke
                    };
                    let button =
                        egui::Button::new(egui::RichText::new(label.as_str()).size(12.5).strong())
                            .rounding(if arrow > 0.0 { left } else { rounding })
                            .fill(fill)
                            .stroke(stroke);
                    // Le survol et l'action suivent la source ; le dessin,
                    // lui, est le même pour les deux.
                    let hover = if *steam {
                        "Sans ouvrir le client Steam"
                    } else {
                        "Téléchargé depuis la boutique, sans son client"
                    };
                    if ui
                        .add_sized([main, 30.0], button)
                        .on_hover_text(hover)
                        .clicked()
                    {
                        if *steam {
                            action.install = true;
                        } else {
                            action.store_install = true;
                        }
                    }
                    if arrow <= 0.0 {
                        return;
                    }
                    let chevron = ui
                        .add_sized(
                            [arrow, 30.0],
                            egui::Button::new("")
                                .rounding(right)
                                .fill(fill)
                                .stroke(stroke),
                        )
                        .on_hover_text("Choisir la version à télécharger");
                    // La flèche dit d'elle-même ce qu'elle choisit : c'est
                    // l'emblème de la version retenue (manchot, fenêtres,
                    // pomme). Il est tracé : la police embarquée n'a pas ces
                    // marques.
                    paint_platform_icon(
                        ui.painter(),
                        chevron.rect.center(),
                        versions[*chosen].id,
                        if chevron.hovered() || lit {
                            TEXT()
                        } else {
                            TEXT_DIM()
                        },
                        0.85,
                    );
                    let popup_id = ui.id().with(("download-version", *appid));
                    if chevron.clicked() {
                        ui.memory_mut(|memory| memory.toggle_popup(popup_id));
                    }
                    version_menu(ui, popup_id, &chevron, versions, mobile, *chosen, action);
                    if let Some(member) = family.get(*family_chosen) {
                        ui.add_space(4.0);
                        let avatar = family_avatar_button(ui, member);
                        let popup_id = ui.id().with(("download-family", *appid));
                        if avatar.clicked() {
                            ui.memory_mut(|memory| memory.toggle_popup(popup_id));
                        }
                        family_menu(
                            ui,
                            popup_id,
                            &avatar,
                            family,
                            *family_chosen,
                            dlc_names,
                            action,
                        );
                    }
                });
            };
            if row.stacked {
                // Hauteur fixée : la carte s'arrête sous la jaquette, la
                // rangée des contenus ne doit pas la faire monter.
                ui.allocate_ui_with_layout(
                    egui::vec2(row.width, CARD_ROW_STACKED_HEIGHT),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.spacing_mut().item_spacing.y = 4.0;
                        ui.set_width(row.width);
                        ui.horizontal(|ui| {
                            ui.add_space(row.pad);
                            split(ui, action);
                        });
                        ui.horizontal(|ui| {
                            ui.add_space(row.pad);
                            if contents_button(ui, width) {
                                action.contents = true;
                            }
                        });
                    },
                );
            } else {
                ui.horizontal(|ui| {
                    ui.add_space(row.pad);
                    split(ui, action);
                    if contents && contents_button(ui, DLC_BUTTON) {
                        action.contents = true;
                    }
                });
            }
        }
        CardButtons::Progress {
            fraction,
            job,
            appid,
        } => {
            let fraction = fraction.clamp(0.0, 1.0);
            // La teinte dominante du jeu, posée par la fiche qui l'affiche :
            // un vert générique jurait avec la jaquette.
            let accent = ui
                .ctx()
                .data(|data| data.get_temp::<egui::Color32>(egui::Id::new(("game-accent", *appid))))
                .filter(|color| *color != egui::Color32::TRANSPARENT)
                .unwrap_or(OK);
            // Une seule hauteur pour la vignette, la barre, le pourcentage et
            // la croix : tout partage le même axe, et la croix cesse de
            // flotter sous la barre.
            const HEIGHT: f32 = 26.0;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.set_height(HEIGHT);
                let (cell, _) =
                    ui.allocate_exact_size(egui::vec2(22.0, HEIGHT), egui::Sense::hover());
                let thumb = egui::Rect::from_center_size(cell.center(), egui::vec2(22.0, 22.0));
                // L'icône du jeu, pas la jaquette : recadrée dans 22 px, la
                // jaquette ne montrait qu'un morceau d'image.
                match icon.as_ref().or(texture.as_ref()) {
                    Some(texture) => paint_cover(ui, thumb, texture, egui::Rounding::same(5.0)),
                    // Pas de nom écrit dans 22 px : un carré plein suffit, la
                    // ligne ne doit pas grandir.
                    None => {
                        ui.painter()
                            .rect_filled(thumb, egui::Rounding::same(5.0), CARD_ACTIVE());
                    }
                }
                let percent = format!("{:.0} %", fraction * 100.0);
                let text = 40.0;
                let bar = (row.width - 22.0 - 8.0 - text - 8.0 - 26.0 - 8.0).max(48.0);
                let (cell, _) =
                    ui.allocate_exact_size(egui::vec2(bar, HEIGHT), egui::Sense::hover());
                let track = egui::Rect::from_center_size(cell.center(), egui::vec2(bar, 6.0));
                ui.painter()
                    .rect_filled(track, egui::Rounding::same(3.0), CARD_ACTIVE());
                ui.painter().rect_filled(
                    egui::Rect::from_min_size(
                        track.min,
                        egui::vec2(track.width() * fraction, track.height()),
                    ),
                    egui::Rounding::same(3.0),
                    accent,
                );
                ui.allocate_ui_with_layout(
                    egui::vec2(text, HEIGHT),
                    egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
                    |ui| {
                        ui.label(egui::RichText::new(&percent).size(11.5).color(TEXT_DIM()));
                    },
                );
                ui.allocate_ui_with_layout(
                    egui::vec2(26.0, HEIGHT),
                    egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
                    |ui| {
                        if sheet_button(ui, SheetButton::Close)
                            .on_hover_text(lapinux_core::i18n::t("downloads.cancel"))
                            .clicked()
                        {
                            action.cancel = Some(job.clone());
                        }
                    },
                );
            });
        }
    }
}

/// Menu de la flèche : les versions que le jeu publie, celle qui partira au
/// téléchargement cochée. Choisir une version ne lance rien : le bouton garde
/// la main, et son poids se met à jour.
fn version_menu(
    ui: &mut egui::Ui,
    popup_id: egui::Id,
    anchor: &egui::Response,
    versions: &[GameVersion],
    mobile: &[GameVersion],
    chosen: usize,
    action: &mut CardAction,
) {
    // L'encart vit au bas de la fiche : sous le bouton il ne reste souvent
    // que quelques dizaines de pixels, le menu s'ouvrirait hors de la fenêtre.
    // Il passe donc au-dessus quand la place manque dessous.
    let space_below = ui.ctx().screen_rect().bottom() - anchor.rect.bottom();
    let side = if space_below < VERSION_MENU_HEIGHT {
        egui::AboveOrBelow::Above
    } else {
        egui::AboveOrBelow::Below
    };
    egui::popup_above_or_below_widget(
        ui,
        popup_id,
        anchor,
        side,
        egui::PopupCloseBehavior::CloseOnClick,
        |ui| {
            // Les libellés portent leur poids : deux versions du même jeu
            // n'ont pas la même taille, c'est souvent ce qui décide.
            ui.set_min_width(190.0);
            if !mobile.is_empty() {
                ui.label(
                    egui::RichText::new(lapinux_core::i18n::t("game_sheet.mobile_versions"))
                        .size(11.0)
                        .color(TEXT_DIM()),
                );
                for version in mobile {
                    ui.label(
                        egui::RichText::new(version.label)
                            .size(12.5)
                            .color(TEXT_DIM()),
                    )
                    .on_hover_text(lapinux_core::i18n::t("game_sheet.mobile_install_hint"));
                }
                ui.label(
                    egui::RichText::new(lapinux_core::i18n::t("game_sheet.mobile_not_installable"))
                        .size(10.0)
                        .color(TEXT_DIM()),
                );
                if !versions.is_empty() {
                    ui.separator();
                }
            }
            if !versions.is_empty() {
                ui.label(
                    egui::RichText::new(lapinux_core::i18n::t("game_sheet.download_version"))
                        .size(11.0)
                        .color(TEXT_DIM()),
                );
                for (index, version) in versions.iter().enumerate() {
                    let text = match version.size {
                        Some(bytes) => format!(
                            "{} · {}",
                            version.label,
                            lapinux_core::util::format_bytes(bytes)
                        ),
                        None => version.label.to_string(),
                    };
                    let response =
                        ui.selectable_label(index == chosen, egui::RichText::new(text).size(12.5));
                    if response.clicked() {
                        action.version = Some(version.id);
                    }
                    if version.id == "windows" {
                        response.on_hover_text("Version Windows, lancée avec Proton");
                    }
                }
            }
        },
    );
}

/// Bouton de la version familiale : la photo du membre qui partage le jeu,
/// au même format carré que les boutons d'icône de la rangée.
fn family_avatar_button(ui: &mut egui::Ui, member: &FamilyVersion) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(CARD_ROW_HEIGHT, CARD_ROW_HEIGHT),
        egui::Sense::click(),
    );
    let hovered = response.hovered();
    let rounding = egui::Rounding::same(8.0);
    let fill = if hovered {
        CARD_HOVER()
    } else {
        ui.visuals().widgets.inactive.weak_bg_fill
    };
    let border = if hovered { TEXT_DIM() } else { BORDER() };
    ui.painter()
        .rect(rect, rounding, fill, egui::Stroke::new(1.0_f32, border));
    let inner = rect.shrink(2.0);
    match &member.texture {
        Some(texture) => {
            egui::Image::new((texture.id(), inner.size()))
                .rounding(egui::Rounding::same(6.0))
                .paint_at(ui, inner);
        }
        None => {
            let initial = member
                .name
                .trim()
                .chars()
                .next()
                .map(|letter| letter.to_uppercase().to_string())
                .unwrap_or_else(|| "?".to_string());
            ui.painter().text(
                inner.center(),
                egui::Align2::CENTER_CENTER,
                initial,
                egui::FontId::proportional(12.5),
                TEXT(),
            );
        }
    }
    response.on_hover_text(lapinux_core::i18n::t_args(
        "game_sheet.family_version_hint",
        &[("name", member.name.as_str())],
    ))
}

/// Menu du bouton familial : les membres qui partagent ce jeu, celui qui est
/// retenu coché, puis les contenus additionnels que la version partagée
/// apporte. Choisir un membre ne lance rien : le bouton garde la main.
fn family_menu(
    ui: &mut egui::Ui,
    popup_id: egui::Id,
    anchor: &egui::Response,
    family: &[FamilyVersion],
    chosen: usize,
    dlc_names: &[String],
    action: &mut CardAction,
) {
    const MAX_DLC: usize = 6;
    let selected = family.get(chosen);
    let personal = selected.map_or(true, |member| member.personal);
    let names = selected
        .map(|member| member.dlc_names.as_slice())
        .unwrap_or(dlc_names);
    let listed = names.len().min(MAX_DLC);
    let height = 30.0 + family.len() as f32 * FAMILY_MENU_ROW + 30.0 + listed as f32 * 16.0 + 12.0;
    let space_below = ui.ctx().screen_rect().bottom() - anchor.rect.bottom();
    let side = if space_below < height {
        egui::AboveOrBelow::Above
    } else {
        egui::AboveOrBelow::Below
    };
    egui::popup_above_or_below_widget(
        ui,
        popup_id,
        anchor,
        side,
        egui::PopupCloseBehavior::CloseOnClick,
        |ui| {
            ui.set_min_width(248.0);
            ui.label(
                egui::RichText::new(lapinux_core::i18n::t("game_sheet.family_version"))
                    .size(11.0)
                    .color(TEXT_DIM()),
            );
            for (index, member) in family.iter().enumerate() {
                if family_menu_row(ui, member, index == chosen).clicked() {
                    action.family_owner = Some(member.steam_id);
                }
            }
            ui.separator();
            ui.label(
                egui::RichText::new(lapinux_core::i18n::t("game_sheet.family_contents"))
                    .size(11.0)
                    .color(TEXT_DIM()),
            );
            if names.is_empty() {
                let key = if personal {
                    "game_sheet.family_no_contents"
                } else {
                    "game_sheet.family_contents_unknown"
                };
                ui.label(
                    egui::RichText::new(lapinux_core::i18n::t(key))
                        .size(11.0)
                        .color(TEXT_DIM()),
                );
            } else {
                for name in names.iter().take(MAX_DLC) {
                    ui.label(
                        egui::RichText::new(format!("• {name}"))
                            .size(11.0)
                            .color(TEXT_DIM()),
                    );
                }
                if names.len() > MAX_DLC {
                    ui.label(
                        egui::RichText::new(lapinux_core::i18n::t_fmt(
                            "game_sheet.family_contents_more",
                            &[(names.len() - MAX_DLC).to_string()],
                        ))
                        .size(10.5)
                        .color(TEXT_DIM()),
                    );
                }
            }
        },
    );
}

/// Une ligne du menu familial : la photo du membre, son nom, le nombre de
/// jeux qu'il partage, et un fond quand il est retenu. Toute la ligne répond
/// au clic.
fn family_menu_row(ui: &mut egui::Ui, member: &FamilyVersion, chosen: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width().max(220.0), FAMILY_MENU_ROW),
        egui::Sense::click(),
    );
    let hovered = response.hovered();
    if chosen || hovered {
        ui.painter().rect_filled(
            rect.shrink2(egui::vec2(2.0, 1.0)),
            egui::Rounding::same(7.0),
            if chosen { CARD_ACTIVE() } else { CARD_HOVER() },
        );
    }
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let avatar = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 17.0, rect.center().y),
        egui::vec2(24.0, 24.0),
    );
    match &member.texture {
        Some(texture) => {
            egui::Image::new((texture.id(), avatar.size()))
                .rounding(egui::Rounding::same(6.0))
                .paint_at(ui, avatar);
        }
        None => {
            ui.painter()
                .rect_filled(avatar, egui::Rounding::same(6.0), CARD_HOVER());
            let initial = member
                .name
                .trim()
                .chars()
                .next()
                .map(|letter| letter.to_uppercase().to_string())
                .unwrap_or_else(|| "?".to_string());
            ui.painter().text(
                avatar.center(),
                egui::Align2::CENTER_CENTER,
                initial,
                egui::FontId::proportional(11.0),
                TEXT_DIM(),
            );
        }
    }
    let name = egui::Rect::from_min_max(
        egui::pos2(avatar.right() + 8.0, rect.top()),
        egui::pos2(rect.right() - 104.0, rect.bottom()),
    );
    ui.painter().with_clip_rect(name).text(
        egui::pos2(name.left(), rect.center().y),
        egui::Align2::LEFT_CENTER,
        &member.name,
        egui::FontId::proportional(12.5),
        if chosen { TEXT() } else { TEXT_DIM() },
    );
    let count = if member.personal {
        lapinux_core::i18n::t("game_sheet.personal_account").to_string()
    } else {
        lapinux_core::i18n::t_fmt("profile.family_games_count", &[member.games.to_string()])
    };
    let count_rect = egui::Rect::from_min_max(
        egui::pos2(rect.right() - 102.0, rect.top()),
        egui::pos2(rect.right() - 6.0, rect.bottom()),
    );
    ui.painter().with_clip_rect(count_rect).text(
        egui::pos2(count_rect.right(), rect.center().y),
        egui::Align2::RIGHT_CENTER,
        count,
        egui::FontId::proportional(10.5),
        TEXT_DIM(),
    );
    response
}

/// Largeur de la jaquette de la ligne compacte, quand les actions reprennent
/// le pied de la carte.
const SHEET_COVER_WIDTH: f32 = 58.0;
/// Hauteur maximale de la jaquette quand les actions se rangent à sa droite :
/// elle monte alors jusqu'à la hauteur de la carte.
const COVER_BESIDE_HEIGHT: f32 = 150.0;
/// En deçà, la jaquette ne se distingue plus : les actions reprennent le pied.
const COVER_BESIDE_MIN: f32 = 72.0;
/// Largeur que la colonne de texte garde à droite de la jaquette : le nom et
/// les actions y tiennent, quel que soit l'état du jeu.
const CARD_TEXT_MIN: f32 = 200.0;
/// Largeur du bouton des contenus additionnels, sur la ligne de l'encart.
const DLC_BUTTON: f32 = 74.0;
/// Largeur du bloc d'actions de l'encart, quand la fiche est assez large pour
/// le porter à droite de la jaquette : la même pour tous les états du jeu.
const CARD_ACTIONS_WIDTH: f32 = 220.0;
/// Largeur de la flèche qui ouvre le choix de version, collée au bouton de
/// téléchargement : assez pour la viser, assez peu pour garder le poids.
const VERSION_ARROW_WIDTH: f32 = 26.0;
/// Largeur du bouton d'avatar familial, collé à droite de la flèche de
/// version : la hauteur de la rangée et l'écart qui l'en sépare.
const FAMILY_BUTTON_WIDTH: f32 = 34.0;
/// Délai pendant lequel une famille déjà lue survit à un snapshot vivant qui
/// ne l'a pas encore relue, le temps d'une reconnexion Steam.
const FAMILY_SNAPSHOT_GRACE: Duration = Duration::from_secs(90);
/// Hauteur d'une ligne du menu familial : la photo du membre y tient avec
/// son nom et le nombre de jeux qu'il partage.
const FAMILY_MENU_ROW: f32 = 34.0;
/// Hauteur approximative du menu de version : sous cette place disponible, il
/// s'ouvre au-dessus du bouton plutôt que sous lui.
const VERSION_MENU_HEIGHT: f32 = 150.0;
/// Hauteur de la rangée d'actions : une ligne de boutons.
const CARD_ROW_HEIGHT: f32 = 30.0;
/// Côté du bouton carré de la rangée (engrenage, poubelle, mise à jour) : la
/// hauteur de la rangée, pour qu'il s'aligne sur le bouton qui l'accompagne.
const CARD_GLYPH_SIZE: f32 = CARD_ROW_HEIGHT;
/// Temps laissé pour confirmer une désinstallation : passé ce délai, la
/// poubelle reprend son icône et redemande un premier clic.
const UNINSTALL_CONFIRM: Duration = Duration::from_secs(4);
/// La même, quand les contenus s'empilent sous le bouton du poids du jeu.
const CARD_ROW_STACKED_HEIGHT: f32 = 64.0;
/// Colonne des boutons de la fiche (fermeture et repli), à gauche.
const SHEET_CONTROLS_WIDTH: f32 = 26.0;
/// Écart entre les boutons de la fiche et la jaquette.
const SHEET_CONTROLS_GAP: f32 = 10.0;

/// Colonnes de la ligne compacte de la fiche, après la marge de gauche
/// réservée aux boutons de fermeture et de repli. L'identité comprend son
/// encart, jaquette incluse.
struct CompactColumns {
    identity: f32,
    /// Écart entre l'encart d'identité et la colonne des succès — il absorbe
    /// la place que le jeu ne remplit pas.
    identity_gap: f32,
    middle: f32,
}

/// Colonne des succès : une part mesurée de la fiche. Au-delà, la bande
/// d'icônes s'étale sans montrer plus de succès, et la fiche se déséquilibre.
const SHEET_MIDDLE_SHARE: f32 = 0.34;
/// Bornes de cette colonne, quelle que soit la largeur de la fiche.
const SHEET_MIDDLE_MIN: f32 = 320.0;
const SHEET_MIDDLE_MAX: f32 = 520.0;

/// Répartit la largeur de la fiche entre l'identité, les succès et les
/// boutons. L'identité porte la jaquette et ses actions ; les succès gardent
/// une colonne mesurée, et la place restante s'ouvre entre les deux. Aucun
/// rail d'informations ne prend le bord droit : les heures de jeu vivent sous
/// le nom, le reste n'apprenait rien.
fn compact_columns(width: f32) -> CompactColumns {
    const GAP_IDENTITY: f32 = 18.0;
    // L'encart contient la jaquette et, à sa droite, le nom et ses actions.
    let identity = (width * 0.32).clamp(300.0, 400.0);
    let room =
        (width - identity - GAP_IDENTITY - SHEET_CONTROLS_WIDTH - SHEET_CONTROLS_GAP).max(120.0);
    let middle = room.min((width * SHEET_MIDDLE_SHARE).clamp(SHEET_MIDDLE_MIN, SHEET_MIDDLE_MAX));
    let slack = (room - middle).max(0.0);
    CompactColumns {
        identity,
        identity_gap: GAP_IDENTITY + slack,
        middle,
    }
}

/// Bouton chevron dessiné : il ne dépend d'aucune police d'icônes pour
/// indiquer si la fiche peut monter ou redescendre.
enum SheetButton {
    /// Ferme la fiche.
    Close,
    /// Replie la fiche, ou la déplie (`up` = le chevron pointe vers le haut).
    Fold(bool),
}

/// Bouton dessiné de la fiche : la croix et le chevron partagent la même
/// taille, le même survol et la même épaisseur de trait, pour se lire comme
/// une paire au lieu d'un bouton standard à côté d'une icône nue.
fn sheet_button(ui: &mut egui::Ui, button: SheetButton) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(26.0, 26.0), egui::Sense::click());
    let hovered = response.hovered();
    if hovered {
        ui.painter()
            .rect_filled(rect, egui::Rounding::same(8.0), CARD_HOVER());
    }
    let color = if hovered { TEXT() } else { TEXT_DIM() };
    let stroke = egui::Stroke::new(1.5_f32, color);
    let center = rect.center();
    match button {
        SheetButton::Close => {
            let d = 4.5;
            ui.painter().line_segment(
                [center + egui::vec2(-d, -d), center + egui::vec2(d, d)],
                stroke,
            );
            ui.painter().line_segment(
                [center + egui::vec2(d, -d), center + egui::vec2(-d, d)],
                stroke,
            );
        }
        SheetButton::Fold(up) => {
            let half = 5.0;
            let dy = if up { half } else { -half };
            ui.painter().line_segment(
                [
                    center + egui::vec2(-half, dy),
                    center + egui::vec2(0.0, -dy),
                ],
                stroke,
            );
            ui.painter().line_segment(
                [center + egui::vec2(0.0, -dy), center + egui::vec2(half, dy)],
                stroke,
            );
        }
    }
    response
}

/// Largeurs d'un en-tête de panneau : la marge gauche, le titre, la croix.
///
/// La marge vaut exactement la croix, si bien que le titre tombe au centre du
/// bandeau et non au centre de ce qui reste à côté d'elle.
fn panel_header_split(width: f32) -> (f32, f32, f32) {
    let (left, middle, _, close) = panel_header_split_with_action(width, false);
    (left, middle, close)
}

/// Même découpage, avec une place pour une action (corbeille) juste avant la
/// croix. La marge gauche réserve la même largeur : le titre reste centré sur
/// le bandeau entier.
fn panel_header_split_with_action(width: f32, has_action: bool) -> (f32, f32, f32, f32) {
    let close = 30.0;
    let action = if has_action { 30.0 } else { 0.0 };
    let side = close + action;
    let middle = (width - side * 2.0).max(120.0);
    (side, middle, action, close)
}

/// Panneau fixe de l'application : le même châssis que les Réglages — voile
/// sombre, panneau centré qui ne se déplace ni ne se redimensionne, en-tête
/// avec titre et fermeture, contenu qui défile. Il sert aux listes qu'on
/// ouvrait dans une fenêtre flottante (succès, contenus additionnels).
///
/// Rend `true` quand il faut refermer le panneau : croix, Échap ou clic à
/// côté. Échap est consommé ici pour ne pas fermer aussi la fiche du jeu.
fn fixed_panel(
    ctx: &egui::Context,
    id: &'static str,
    title: &str,
    subtitle: &str,
    size: egui::Vec2,
    body: impl FnOnce(&mut egui::Ui),
) -> bool {
    fixed_panel_with_scroll(ctx, id, title, subtitle, size, true, true, 196, body)
}

/// Panneau fixe avec ou sans défilement extérieur. Une conversation porte son
/// propre viewport : lui ajouter la ScrollArea du panneau créerait deux barres
/// et ferait défiler la carte sous le compositeur.
fn fixed_panel_with_scroll(
    ctx: &egui::Context,
    id: &'static str,
    title: &str,
    subtitle: &str,
    size: egui::Vec2,
    scroll: bool,
    show_header: bool,
    dim_alpha: u8,
    body: impl FnOnce(&mut egui::Ui),
) -> bool {
    let (close, _) = fixed_panel_with_scroll_action(
        ctx,
        id,
        title,
        subtitle,
        size,
        scroll,
        show_header,
        dim_alpha,
        None,
        body,
    );
    close
}

/// Même panneau, avec une action facultative dans l'en-tête. L'action est
/// peinte à gauche de la croix ; son clic est renvoyé avec la fermeture.
fn fixed_panel_with_scroll_action(
    ctx: &egui::Context,
    id: &'static str,
    title: &str,
    subtitle: &str,
    size: egui::Vec2,
    scroll: bool,
    show_header: bool,
    dim_alpha: u8,
    header_trash: Option<&str>,
    body: impl FnOnce(&mut egui::Ui),
) -> (bool, bool) {
    let screen = ctx.screen_rect();
    let panel = size.min(screen.size() - egui::vec2(48.0, 48.0));
    let rect = egui::Rect::from_center_size(screen.center(), panel);
    let header_height = if show_header { 64.0 } else { 0.0 };
    let mut close =
        ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
    let mut header_action = false;
    // Le clic qui a ouvert le panneau ne doit pas le refermer dans la même
    // image : c'est seulement à partir de la deuxième qu'un clic à côté
    // compte.
    let seen = egui::Id::new((id, "deja-vu"));
    let fresh = ctx.memory(|memory| memory.data.get_temp::<bool>(seen).is_none());
    ctx.memory_mut(|memory| memory.data.insert_temp(seen, true));
    egui::Window::new(title)
        .id(egui::Id::new(id))
        .title_bar(false)
        .resizable(false)
        .collapsible(false)
        .movable(false)
        .fixed_pos(screen.min)
        .fixed_size(screen.size())
        .frame(egui::Frame::none())
        // Au-dessus de la fiche du jeu, qui est une zone flottante.
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            ui.painter()
                .rect_filled(screen, 0.0, egui::Color32::from_black_alpha(dim_alpha));
            // Le clic à côté referme : quatre bandes autour du panneau, qui
            // absorbent aussi le clic destiné aux cartes de la bibliothèque.
            let outside = [
                egui::Rect::from_min_max(screen.min, egui::pos2(rect.left(), screen.max.y)),
                egui::Rect::from_min_max(egui::pos2(rect.right(), screen.min.y), screen.max),
                egui::Rect::from_min_max(
                    egui::pos2(rect.left(), screen.min.y),
                    egui::pos2(rect.right(), rect.top()),
                ),
                egui::Rect::from_min_max(
                    egui::pos2(rect.left(), rect.bottom()),
                    egui::pos2(rect.right(), screen.max.y),
                ),
            ];
            for (index, area) in outside.into_iter().enumerate() {
                if !fresh
                    && area.width() > 0.0
                    && area.height() > 0.0
                    && ui
                        .interact(area, egui::Id::new((id, index)), egui::Sense::click())
                        .clicked()
                {
                    close = true;
                }
            }
            ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                egui::Frame::none()
                    .fill(PANEL())
                    .stroke(egui::Stroke::new(1.0_f32, BORDER()))
                    .rounding(egui::Rounding::same(16.0))
                    .shadow(egui::epaint::Shadow {
                        offset: egui::vec2(0.0, 18.0),
                        blur: 44.0,
                        spread: 0.0,
                        color: egui::Color32::from_black_alpha(170),
                    })
                    .show(ui, |ui| {
                        ui.set_min_size(panel);
                        ui.set_max_size(panel);
                        let header = egui::Rect::from_min_max(
                            rect.min,
                            egui::pos2(rect.right(), rect.top() + header_height),
                        );
                        let painter = ui.painter();
                        if show_header {
                            painter.rect_filled(
                                header,
                                egui::Rounding {
                                    nw: 16.0,
                                    ne: 16.0,
                                    sw: 0.0,
                                    se: 0.0,
                                },
                                BACKGROUND(),
                            );
                            painter.line_segment(
                                [
                                    egui::pos2(rect.left(), header.bottom()),
                                    egui::pos2(rect.right(), header.bottom()),
                                ],
                                egui::Stroke::new(1.0_f32, BORDER()),
                            );
                            ui.scope_builder(
                                egui::UiBuilder::new()
                                    .max_rect(header.shrink2(egui::vec2(22.0, 0.0))),
                                |ui| {
                                    // Sans sous-titre, le titre seul est centré dans
                                    // les 64 px du bandeau au lieu de rester haut.
                                    ui.add_space(if subtitle.is_empty() { 20.0 } else { 13.0 });
                                    ui.horizontal(|ui| {
                                        // Sans écart implicite entre les trois
                                        // blocs : les largeurs se calculent
                                        // exactement, et la croix garde le bord.
                                        ui.spacing_mut().item_spacing.x = 0.0;
                                        // Le titre est centré sur le bandeau entier :
                                        // la même largeur est réservée à gauche qu'à
                                        // la croix à droite.
                                        let (left, middle, action_width, close_width) =
                                            panel_header_split_with_action(
                                                ui.available_width(),
                                                header_trash.is_some(),
                                            );
                                        ui.add_space(left);
                                        ui.allocate_ui_with_layout(
                                            egui::vec2(middle, 38.0),
                                            egui::Layout::top_down(egui::Align::Center),
                                            |ui| {
                                                ui.add(
                                                    egui::Label::new(
                                                        egui::RichText::new(title)
                                                            .size(17.0)
                                                            .strong()
                                                            .color(TEXT()),
                                                    )
                                                    .truncate(),
                                                );
                                                if !subtitle.is_empty() {
                                                    ui.add_space(1.0);
                                                    ui.add(
                                                        egui::Label::new(
                                                            egui::RichText::new(subtitle)
                                                                .size(11.0)
                                                                .color(TEXT_DIM()),
                                                        )
                                                        .truncate(),
                                                    );
                                                }
                                            },
                                        );
                                        if let Some(tooltip) = header_trash {
                                            let (trash_rect, response) = ui.allocate_exact_size(
                                                egui::vec2(action_width, 30.0),
                                                egui::Sense::click(),
                                            );
                                            let response = response
                                                .on_hover_cursor(egui::CursorIcon::PointingHand);
                                            if response.hovered() {
                                                ui.painter().circle_filled(
                                                    trash_rect.center(),
                                                    14.0,
                                                    with_alpha(CARD_HOVER(), 0.85),
                                                );
                                            }
                                            crate::captures::paint_trash_glyph(
                                                ui.painter(),
                                                trash_rect.center(),
                                                13.0,
                                                if response.hovered() {
                                                    DANGER
                                                } else {
                                                    TEXT_DIM()
                                                },
                                            );
                                            if response.on_hover_text(tooltip).clicked() {
                                                header_action = true;
                                            }
                                        }
                                        let (close_rect, response) = ui.allocate_exact_size(
                                            egui::vec2(close_width, 30.0),
                                            egui::Sense::click(),
                                        );
                                        let response = response
                                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                                        if response.hovered() {
                                            ui.painter().circle_filled(
                                                close_rect.center(),
                                                14.0,
                                                with_alpha(CARD_HOVER(), 0.85),
                                            );
                                        }
                                        ui.painter().text(
                                            close_rect.center(),
                                            egui::Align2::CENTER_CENTER,
                                            "×",
                                            egui::FontId::proportional(19.0),
                                            if response.hovered() {
                                                TEXT()
                                            } else {
                                                TEXT_DIM()
                                            },
                                        );
                                        if response
                                            .on_hover_text(lapinux_core::i18n::t(
                                                "game_sheet.close",
                                            ))
                                            .clicked()
                                        {
                                            close = true;
                                        }
                                    });
                                },
                            );
                        }
                        let content = egui::Rect::from_min_max(
                            egui::pos2(
                                rect.left() + 22.0,
                                if show_header {
                                    header.bottom() + 10.0
                                } else {
                                    rect.top() + 10.0
                                },
                            ),
                            egui::pos2(rect.right() - 12.0, rect.bottom() - 16.0),
                        );
                        ui.scope_builder(egui::UiBuilder::new().max_rect(content), |ui| {
                            let width = (content.width() - 10.0).max(120.0);
                            if scroll {
                                egui::ScrollArea::vertical()
                                    .id_salt((id, "contenu"))
                                    .auto_shrink([false, false])
                                    .show(ui, |ui| {
                                        ui.set_width(width);
                                        body(ui);
                                    });
                            } else {
                                // La conversation gère son propre défilement.
                                ui.set_width(width);
                                ui.set_clip_rect(content);
                                body(ui);
                            }
                        });
                        // Les panneaux sans bandeau gardent une fermeture
                        // visible : la croix se pose au-dessus du titre de la
                        // page, dans le même coin que celle du bandeau.
                        if !show_header {
                            let close_rect = egui::Rect::from_min_size(
                                egui::pos2(rect.right() - 38.0, rect.top() + 9.0),
                                egui::vec2(28.0, 28.0),
                            );
                            let response = ui
                                .interact(
                                    close_rect,
                                    ui.id().with((id, "close")),
                                    egui::Sense::click(),
                                )
                                .on_hover_cursor(egui::CursorIcon::PointingHand);
                            if response.hovered() {
                                ui.painter().circle_filled(
                                    close_rect.center(),
                                    13.0,
                                    with_alpha(CARD_HOVER(), 0.85),
                                );
                            }
                            ui.painter().text(
                                close_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                "×",
                                egui::FontId::proportional(19.0),
                                if response.hovered() {
                                    TEXT()
                                } else {
                                    TEXT_DIM()
                                },
                            );
                            if response
                                .on_hover_text(lapinux_core::i18n::t("game_sheet.close"))
                                .clicked()
                            {
                                close = true;
                            }
                        }
                    });
            });
        });
    if close {
        ctx.memory_mut(|memory| memory.data.remove::<bool>(seen));
    }
    (close, header_action)
}

/// Peint la fiche et son contenu. La géométrie vit hors de l'état de
/// l'application, pour pouvoir être vérifiée sans fenêtre.
fn show_detail_sheet(
    ctx: &egui::Context,
    progress: f32,
    expanded: bool,
    body: impl FnOnce(&mut egui::Ui),
) -> egui::Rect {
    let rect = detail_sheet_rect(ctx.screen_rect(), progress, expanded);
    egui::Area::new(egui::Id::new("game_detail"))
        .fixed_pos(rect.min)
        // Hors champ pendant la glissière : la zone ne doit pas être ramenée
        // de force dans la fenêtre.
        .constrain(false)
        .order(egui::Order::Middle)
        .show(ctx, |ui| {
            ui.set_width(rect.width());
            egui::Frame::none()
                .fill(PANEL())
                .stroke(egui::Stroke::new(1.0_f32, BORDER()))
                .rounding(egui::Rounding {
                    nw: 16.0,
                    ne: 16.0,
                    sw: 0.0,
                    se: 0.0,
                })
                .shadow(egui::epaint::Shadow {
                    offset: egui::vec2(0.0, -12.0),
                    blur: 28.0,
                    spread: 0.0,
                    color: egui::Color32::from_black_alpha(140),
                })
                .inner_margin(egui::Margin::symmetric(22.0, 16.0))
                .show(ui, |ui| {
                    ui.set_width(rect.width() - 44.0);
                    ui.set_height(rect.height() - 32.0);
                    // Rien ne doit déborder du rectangle : un texte long ne
                    // peut plus élargir le panneau et décaler sa marge.
                    ui.set_clip_rect(ui.clip_rect().intersect(rect));
                    body(ui);
                });
        })
        .response
        .rect
}

/// Une ligne du tableau d'identité : libellé discret, valeur lisible.
fn detail_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.label(egui::RichText::new(label).size(11.0).color(TEXT_DIM()));
    ui.label(egui::RichText::new(value).size(11.0).color(TEXT()));
    ui.end_row();
}

#[derive(Default, Clone, Copy)]
struct ControllerRowAction {
    select: bool,
    power: bool,
}

#[derive(Default)]
struct PadHeaderAction {
    start_rename: bool,
    power: bool,
    save: bool,
    cancel: bool,
}

/// Identité de la fiche : nom éditable, console en texte simple. La batterie
/// et la connexion vivent dans leur propre bloc, à l'extrême droite.
fn controller_identity(
    ui: &mut egui::Ui,
    device: &DeviceInfo,
    identity: &PadIdentity,
    shown: &str,
    editing: bool,
    request_focus: bool,
    power: PadPowerControl,
    rename_text: &mut String,
) -> PadHeaderAction {
    let mut action = PadHeaderAction::default();
    ui.horizontal_top(|ui| {
        let (tile, _) = ui.allocate_exact_size(egui::vec2(56.0, 56.0), egui::Sense::hover());
        ui.painter()
            .rect_filled(tile, egui::Rounding::same(12.0), BACKGROUND());
        ui.painter().text(
            tile.center(),
            egui::Align2::CENTER_CENTER,
            nav_glyph(Page::Controllers),
            icon_font(32.0),
            if device.connected { OK } else { TEXT_DIM() },
        );
        ui.add_space(6.0);
        ui.vertical(|ui| {
            if editing {
                let field = ui.add_sized(
                    [190.0, 24.0],
                    egui::TextEdit::singleline(rename_text)
                        .hint_text(lapinux_core::i18n::t("controllers.ui.name_label")),
                );
                if request_focus {
                    field.request_focus();
                }
                // Entrée retire le focus au champ dans la même image : il faut
                // accepter aussi `lost_focus`, sinon la touche ne valide rien.
                let enter = ui.input(|input| input.key_pressed(egui::Key::Enter));
                if enter && (field.has_focus() || field.lost_focus()) {
                    action.save = true;
                }
                if ui.input(|input| input.key_pressed(egui::Key::Escape))
                    && (field.has_focus() || field.lost_focus())
                {
                    action.cancel = true;
                }
                ui.horizontal(|ui| {
                    let save = egui::Button::new(
                        egui::RichText::new(lapinux_core::i18n::t("controllers.ui.save"))
                            .size(10.5)
                            .color(TEXT()),
                    )
                    .fill(CARD_HOVER())
                    .stroke(egui::Stroke::new(1.0_f32, BORDER()))
                    .rounding(egui::Rounding::same(6.0))
                    .min_size(egui::vec2(0.0, 22.0));
                    if ui.add(save).clicked() {
                        action.save = true;
                    }
                    let cancel = egui::Button::new(
                        egui::RichText::new(lapinux_core::i18n::t("controllers.ui.cancel"))
                            .size(10.5)
                            .color(TEXT_DIM()),
                    )
                    .fill(CARD())
                    .stroke(egui::Stroke::new(1.0_f32, BORDER()))
                    .rounding(egui::Rounding::same(6.0))
                    .min_size(egui::vec2(0.0, 22.0));
                    if ui.add(cancel).clicked() {
                        action.cancel = true;
                    }
                    muted(ui, lapinux_core::i18n::t("controllers.ui.empty_name"));
                });
            } else {
                ui.horizontal(|ui| {
                    let logo_space = if identity.brand.is_some() { 26.0 } else { 0.0 };
                    let actions_space = if matches!(power, PadPowerControl::Hidden) {
                        32.0
                    } else {
                        56.0
                    };
                    let name_width = (ui.available_width() - logo_space - actions_space).max(110.0);
                    let name = elide_to_width(ui.painter(), shown, display_font(17.0), name_width);
                    ui.label(
                        egui::RichText::new(name)
                            .font(display_font(17.0))
                            .color(TEXT()),
                    )
                    .on_hover_text(controller_tooltip(device, identity, shown));
                    if let Some(brand) = identity.brand {
                        if brand == Brand::Generic {
                            let (rect, _) = ui
                                .allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::hover());
                            paint_brand_mark(
                                ui.painter(),
                                brand,
                                rect.left(),
                                rect.center().y,
                                19.0,
                                TEXT_DIM(),
                            );
                        } else {
                            ui.label(
                                egui::RichText::new(brand_glyph(brand).to_string())
                                    .font(icon_font(19.0))
                                    .color(TEXT_DIM()),
                            );
                        }
                    }
                    let (id, rect) = ui.allocate_space(egui::vec2(24.0, 24.0));
                    if icon_button_at(
                        ui,
                        rect,
                        id,
                        ICON_EDIT,
                        TEXT_DIM(),
                        lapinux_core::i18n::t("controllers.ui.rename"),
                        true,
                        false,
                    )
                    .clicked()
                    {
                        action.start_rename = true;
                    }
                    if !matches!(power, PadPowerControl::Hidden) {
                        let (id, rect) = ui.allocate_space(egui::vec2(24.0, 24.0));
                        if pad_power_button_at(ui, rect, id, power) {
                            action.power = true;
                        }
                    }
                });
            }
            ui.add_space(3.0);
            ui.horizontal(|ui| {
                if let Some(console) = console_label(identity) {
                    ui.label(egui::RichText::new(console).size(11.5).color(TEXT_DIM()));
                }
                if matches!(power, PadPowerControl::On { pending: true, .. }) {
                    ui.label(
                        egui::RichText::new(lapinux_core::i18n::t(
                            "controllers.ui.searching_ps_short",
                        ))
                        .size(10.5)
                        .color(ACCENT()),
                    )
                    .on_hover_text(lapinux_core::i18n::t("controllers.ui.searching_ps_hint"));
                } else if matches!(power, PadPowerControl::On { failed: true, .. }) {
                    ui.label(
                        egui::RichText::new(lapinux_core::i18n::t("controllers.ui.asleep_short"))
                            .size(10.5)
                            .color(WARN),
                    )
                    .on_hover_text(lapinux_core::i18n::t("controllers.ui.asleep_hint"));
                } else if !device.connected {
                    ui.label(
                        egui::RichText::new(lapinux_core::i18n::t("controllers.ui.offline"))
                            .size(10.5)
                            .color(WARN),
                    );
                }
            });
        });
    });
    action
}

/// Connexion et batterie, sur une seule rangée à l'extrême droite de la
/// fiche : même ordre que dans la liste des manettes.
fn controller_power(ui: &mut egui::Ui, device: &DeviceInfo, pad: Option<&lapinux_input::PadState>) {
    let live = pad.and_then(|pad| pad.battery.as_ref());
    let battery = live.and_then(|battery| battery.percent).or(device.battery);
    let state = live
        .map(|battery| battery.state)
        .filter(|state| *state != ChargeState::Unknown)
        .or_else(|| {
            device
                .battery_status
                .as_deref()
                .map(ChargeState::from_sysfs)
        });
    // Batterie en haut, connexion en dessous : la colonne reste étroite et
    // les deux pastilles partagent le même gabarit.
    ui.vertical_centered(|ui| {
        ui.spacing_mut().item_spacing.y = 6.0;
        if let Some(battery) = battery {
            battery_ring(ui, battery, 38.0, battery_charging(state));
        } else if lapinux_input::steam::is_steam_2026(device) {
            battery_ring_placeholder(
                ui,
                38.0,
                lapinux_core::i18n::t("controllers.battery.pending"),
            );
        } else if device.connection == DeviceLink::Dongle {
            // Un dongle 2,4 GHz ne transporte presque jamais le niveau de
            // batterie : afficher un tiret explique l'absence au lieu de la
            // laisser passer pour un bug de détection.
            battery_ring_placeholder(
                ui,
                38.0,
                lapinux_core::i18n::t("controllers.battery.dongle_hint"),
            );
        }
        let (icon, response) = ui.allocate_exact_size(egui::vec2(38.0, 38.0), egui::Sense::hover());
        connection_badge(ui.painter(), icon.center(), device.connection, 38.0);
        response.on_hover_text(device.connection.label());
    });
}

/// Carte de debug en bas de la fiche : toutes les informations du périphérique
/// et l'état en direct, copiables en un clic.
fn controller_information(
    ui: &mut egui::Ui,
    device: &DeviceInfo,
    pad: Option<&lapinux_input::PadState>,
    shown: &str,
    open: &mut bool,
) -> bool {
    let identity = pad_identity(&device);
    let console = console_label(&identity)
        .unwrap_or(lapinux_core::i18n::t("controllers.information.unknown"));
    let brand = identity
        .brand
        .map(Brand::label)
        .unwrap_or(lapinux_core::i18n::t("controllers.information.unknown"));
    let yes = lapinux_core::i18n::t("controllers.information.yes");
    let no = lapinux_core::i18n::t("controllers.information.no");
    let dash = lapinux_core::i18n::t("controllers.information.dash");
    let battery = device
        .battery
        .map(|battery| {
            lapinux_core::i18n::t_args(
                "controllers.information.battery_percent",
                &[("percent", &battery.to_string())],
            )
        })
        .unwrap_or_else(|| {
            if lapinux_input::steam::is_steam_2026(device) {
                lapinux_core::i18n::t("controllers.information.battery_pending").to_string()
            } else if device.connection == DeviceLink::Dongle {
                lapinux_core::i18n::t("controllers.information.battery_dongle").to_string()
            } else {
                dash.to_string()
            }
        });
    let buttons = if device.button_names.is_empty() {
        dash.to_string()
    } else {
        device.button_names.join(" · ")
    };
    let axes = if device.axes.is_empty() {
        dash.to_string()
    } else {
        device.axes.join(" · ")
    };
    let mut lines = vec![
        lapinux_core::i18n::t_args("controllers.information.shown_name", &[("name", shown)]),
        lapinux_core::i18n::t_args(
            "controllers.information.kernel_name",
            &[("name", &device.name)],
        ),
        lapinux_core::i18n::t_args(
            "controllers.information.model",
            &[("model", identity.model.unwrap_or(dash))],
        ),
        lapinux_core::i18n::t_args("controllers.information.console", &[("console", console)]),
        lapinux_core::i18n::t_args("controllers.information.brand", &[("brand", brand)]),
        lapinux_core::i18n::t_args(
            "controllers.information.node",
            &[("node", &device.path.display().to_string())],
        ),
        lapinux_core::i18n::t_args(
            "controllers.information.identifier",
            &[(
                "id",
                &format!("{:04x}:{:04x}", device.vendor, device.product),
            )],
        ),
        lapinux_core::i18n::t_args(
            "controllers.information.id_vendor",
            &[("value", device.udev.vendor.as_deref().unwrap_or(dash))],
        ),
        lapinux_core::i18n::t_args(
            "controllers.information.id_model",
            &[("value", device.udev.model.as_deref().unwrap_or(dash))],
        ),
        lapinux_core::i18n::t_args(
            "controllers.information.id_serial",
            &[("value", device.udev.serial.as_deref().unwrap_or(dash))],
        ),
        lapinux_core::i18n::t_args(
            "controllers.information.link",
            &[("link", device.connection.label())],
        ),
        lapinux_core::i18n::t_args(
            "controllers.information.address",
            &[("address", device.address.as_deref().unwrap_or(dash))],
        ),
        lapinux_core::i18n::t_args(
            "controllers.information.connected",
            &[("state", if device.connected { yes } else { no })],
        ),
        lapinux_core::i18n::t_args(
            "controllers.information.buttons",
            &[("count", &device.buttons.to_string()), ("names", &buttons)],
        ),
        lapinux_core::i18n::t_args(
            "controllers.information.axes",
            &[("count", &device.axes.len().to_string()), ("names", &axes)],
        ),
        lapinux_core::i18n::t_args(
            "controllers.information.force_feedback",
            &[("state", if device.force_feedback { yes } else { no })],
        ),
        lapinux_core::i18n::t_args(
            "controllers.information.touchpad_state",
            &[("state", if device.touchpad { yes } else { no })],
        ),
        lapinux_core::i18n::t_args("controllers.information.battery", &[("value", &battery)]),
    ];
    if let Some(status) = device.battery_status.as_deref() {
        lines.push(lapinux_core::i18n::t_args(
            "controllers.information.battery_state",
            &[("status", status)],
        ));
    }
    if let Some(pad) = pad {
        lines.push(String::new());
        lines.push(lapinux_core::i18n::t("controllers.information.live_state").to_string());
        lines.push(lapinux_core::i18n::t_args(
            "controllers.information.axes_live",
            &[(
                "axes",
                &serde_json::to_string(&pad.axes).unwrap_or_else(|_| dash.to_string()),
            )],
        ));
        lines.push(lapinux_core::i18n::t_args(
            "controllers.information.hid",
            &[("value", &format!("{:?}", pad.report))],
        ));
        lines.push(lapinux_core::i18n::t_args(
            "controllers.information.motion",
            &[("value", &format!("{:?}", pad.motion))],
        ));
        lines.push(lapinux_core::i18n::t_args(
            "controllers.information.touchpad_live",
            &[(
                "value",
                &format!(
                    "{:?}",
                    pad.touchpad.as_ref().map(|touchpad| touchpad.points.len())
                ),
            )],
        ));
        lines.push(lapinux_core::i18n::t_args(
            "controllers.information.mic_muted",
            &[("value", &format!("{:?}", pad.microphone_muted))],
        ));
        if let Some(battery) = pad.battery.as_ref() {
            lines.push(lapinux_core::i18n::t_args(
                "controllers.information.battery_live",
                &[
                    (
                        "percent",
                        &battery
                            .percent
                            .map(|percent| percent.to_string())
                            .unwrap_or_else(|| dash.to_string()),
                    ),
                    ("state", charge_state_label(battery.state)),
                    (
                        "band",
                        &battery
                            .band()
                            .map(|band| battery_band_label(Some(band)))
                            .unwrap_or_else(|| {
                                lapinux_core::i18n::t("controllers.information.unknown_band")
                                    .to_string()
                            }),
                    ),
                ],
            ));
            if let Some(voltage) = battery.voltage_mv {
                lines.push(lapinux_core::i18n::t_args(
                    "controllers.information.voltage",
                    &[("voltage", &voltage.to_string())],
                ));
            }
            if let Some(cycles) = battery.cycles {
                lines.push(lapinux_core::i18n::t_args(
                    "controllers.information.cycles",
                    &[("cycles", &cycles.to_string())],
                ));
            }
            if let Some(code) = battery.battery_code.as_ref() {
                lines.push(lapinux_core::i18n::t_args(
                    "controllers.information.battery_code",
                    &[("code", code)],
                ));
            }
        }
    }
    let summary = lines.join("\n");
    let json = serde_json::to_string_pretty(device).unwrap_or_else(|error| {
        lapinux_core::i18n::t_args(
            "controllers.information.serialize_failed",
            &[("error", &error.to_string())],
        )
    });
    let text = lapinux_core::i18n::t_args(
        "controllers.information.json_text",
        &[("summary", &summary), ("json", &json)],
    );
    let mut copied = false;
    egui::Frame::none()
        .fill(CARD())
        .stroke(egui::Stroke::new(1.0_f32, BORDER()))
        .rounding(egui::Rounding::same(12.0))
        .inner_margin(14.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            // Bandeau repliable maison : l'état vit dans l'application, la
            // carte reste donc fermée à chaque lancement, comme les
            // catégories des réglages.
            ui.horizontal(|ui| {
                let copy_space = 34.0;
                let header_width = (ui.available_width() - copy_space).max(60.0);
                let (rect, response) =
                    ui.allocate_exact_size(egui::vec2(header_width, 28.0), egui::Sense::click());
                let response = response
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text(if *open {
                        lapinux_core::i18n::t("controllers.information.collapse")
                    } else {
                        lapinux_core::i18n::t("controllers.information.expand")
                    });
                let ink = if response.hovered() {
                    TEXT()
                } else {
                    TEXT_DIM()
                };
                // Le titre est toujours blanc : la carte reste lisible même
                // quand le bandeau n'est survolé que par hasard.
                let title_font = egui::FontId::proportional(13.0);
                let title_width = ui
                    .painter()
                    .layout_no_wrap(
                        lapinux_core::i18n::t("controllers.information.title").to_string(),
                        title_font.clone(),
                        TEXT(),
                    )
                    .size()
                    .x;
                ui.painter().text(
                    rect.left_center(),
                    egui::Align2::LEFT_CENTER,
                    lapinux_core::i18n::t("controllers.information.title"),
                    title_font,
                    TEXT(),
                );
                let caret_x = rect.left() + title_width + 9.0;
                let caret = egui::pos2(caret_x, rect.center().y);
                let size = 4.2;
                let points = if *open {
                    vec![
                        caret + egui::vec2(-size, -size * 0.45),
                        caret + egui::vec2(0.0, size * 0.6),
                        caret + egui::vec2(size, -size * 0.45),
                    ]
                } else {
                    vec![
                        caret + egui::vec2(-size * 0.45, -size),
                        caret + egui::vec2(size * 0.6, 0.0),
                        caret + egui::vec2(-size * 0.45, size),
                    ]
                };
                ui.painter()
                    .add(egui::Shape::line(points, egui::Stroke::new(1.4_f32, ink)));
                if response.clicked() {
                    *open = !*open;
                }
                let (rect, response) =
                    ui.allocate_exact_size(egui::vec2(28.0, 28.0), egui::Sense::click());
                if response.hovered() {
                    ui.painter()
                        .rect_filled(rect, egui::Rounding::same(6.0), CARD_HOVER());
                }
                paint_copy_icon(
                    ui.painter(),
                    rect,
                    if response.hovered() {
                        TEXT()
                    } else {
                        TEXT_DIM()
                    },
                );
                if response.clicked() {
                    ui.ctx().copy_text(text.clone());
                    copied = true;
                }
                response.on_hover_text(lapinux_core::i18n::t("controllers.information.copy_all"));
            });
            if *open {
                ui.add_space(8.0);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(&summary)
                            .monospace()
                            .size(10.0)
                            .color(TEXT_DIM()),
                    )
                    .selectable(true),
                );
                ui.add_space(6.0);
                egui::CollapsingHeader::new(lapinux_core::i18n::t(
                    "controllers.information.json_full",
                ))
                .id_salt("controller-information-json")
                .default_open(false)
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&json)
                                .monospace()
                                .size(9.5)
                                .color(TEXT_DIM()),
                        )
                        .selectable(true),
                    );
                });
            }
        });
    copied
}

/// Deux feuilles superposées : l'icône « copier » du bandeau Informations.
fn paint_copy_icon(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    let back = egui::Rect::from_min_size(rect.min + egui::vec2(5.0, 4.0), egui::vec2(12.0, 14.0));
    let front = egui::Rect::from_min_size(rect.min + egui::vec2(9.0, 8.0), egui::vec2(12.0, 14.0));
    painter.rect_stroke(
        back,
        egui::Rounding::same(2.5),
        egui::Stroke::new(1.2_f32, color),
    );
    painter.rect_filled(front, egui::Rounding::same(2.5), CARD());
    painter.rect_stroke(
        front,
        egui::Rounding::same(2.5),
        egui::Stroke::new(1.2_f32, color),
    );
}

/// Peint un bouton d'icône dans un rectangle déjà réservé : fond au survol
/// et état actif (confirmation). L'interaction reste à l'appelant, ce qui
/// permet de l'utiliser dans une ligne elle-même cliquable.
fn paint_icon_button(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    glyph: char,
    color: egui::Color32,
    hovered: bool,
    active: bool,
) {
    if active {
        ui.painter()
            .rect_filled(rect, egui::Rounding::same(6.0), with_alpha(WARN, 0.18));
    } else if hovered {
        ui.painter()
            .rect_filled(rect, egui::Rounding::same(6.0), CARD_HOVER());
    }
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        glyph,
        icon_font((rect.height() * 0.62).max(10.0)),
        color,
    );
}

/// Bouton d'icône autonome (en-tête de la fiche) : interaction, survol et
/// infobulle.
fn icon_button_at(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    id: egui::Id,
    glyph: char,
    color: egui::Color32,
    hover: &str,
    enabled: bool,
    active: bool,
) -> egui::Response {
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let response = ui.interact(rect, id, sense);
    let response = if enabled {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response
    };
    paint_icon_button(
        ui,
        rect,
        glyph,
        color,
        enabled && response.hovered(),
        active,
    );
    response.on_hover_text(hover)
}

/// Arc qui tourne pendant une action BlueZ : la connexion peut prendre
/// plusieurs secondes, l'icône doit dire qu'elle travaille.
fn paint_spinner(ui: &mut egui::Ui, rect: egui::Rect) {
    let time = ui.input(|input| input.time) as f32;
    let center = rect.center();
    let radius = rect.height() * 0.26;
    let start = time * 4.0;
    let points: Vec<egui::Pos2> = (0..=16)
        .map(|index| {
            let angle = start + std::f32::consts::TAU * 0.7 * index as f32 / 16.0;
            center + egui::vec2(angle.cos(), angle.sin()) * radius
        })
        .collect();
    ui.painter().add(egui::Shape::line(
        points,
        egui::Stroke::new(1.8_f32, ACCENT()),
    ));
    ui.ctx().request_repaint();
}

/// Texte d'aide du bouton d'alimentation, selon son état.
fn pad_power_hint(power: PadPowerControl) -> &'static str {
    match power {
        PadPowerControl::Hidden => "",
        PadPowerControl::Wired => lapinux_core::i18n::t("controllers.ui.power_wired"),
        PadPowerControl::Off { pending: true, .. } => {
            lapinux_core::i18n::t("controllers.ui.power_off_pending")
        }
        PadPowerControl::Off { armed: true, .. } => {
            lapinux_core::i18n::t("controllers.ui.power_off_confirm")
        }
        PadPowerControl::Off { .. } => lapinux_core::i18n::t("controllers.ui.power_off"),
        PadPowerControl::On { pending: true, .. } => {
            lapinux_core::i18n::t("controllers.ui.power_on_search")
        }
        PadPowerControl::On { failed: true, .. } => {
            lapinux_core::i18n::t("controllers.ui.power_on_off")
        }
        PadPowerControl::On { .. } => lapinux_core::i18n::t("controllers.ui.power_on_retry"),
    }
}

/// Glyphe, couleur et fond actif du bouton d'alimentation, hors attente.
/// `None` signifie « rien à peindre » : bouton caché ou arc de progression.
fn pad_power_symbol(power: PadPowerControl) -> Option<(char, egui::Color32, bool)> {
    match power {
        PadPowerControl::Hidden => None,
        PadPowerControl::Wired => Some((ICON_POWER, with_alpha(TEXT_DIM(), 0.4), false)),
        PadPowerControl::Off { pending: true, .. } => None,
        PadPowerControl::Off { armed: true, .. } => Some((ICON_CHECK, WARN, true)),
        PadPowerControl::Off { .. } => Some((ICON_POWER, TEXT_DIM(), false)),
        PadPowerControl::On { pending: true, .. } => None,
        PadPowerControl::On { failed: true, .. } => Some((ICON_POWER, WARN, true)),
        PadPowerControl::On { .. } => Some((ICON_POWER, TEXT(), false)),
    }
}

fn pad_power_pending(power: PadPowerControl) -> bool {
    matches!(
        power,
        PadPowerControl::Off { pending: true, .. } | PadPowerControl::On { pending: true, .. }
    )
}

/// Peint l'icône d'alimentation ou l'arc de progression (en-tête).
fn paint_pad_power(ui: &mut egui::Ui, rect: egui::Rect, power: PadPowerControl, hovered: bool) {
    if pad_power_pending(power) {
        paint_spinner(ui, rect);
    } else if let Some((glyph, color, active)) = pad_power_symbol(power) {
        paint_icon_button(ui, rect, glyph, color, hovered, active);
    }
}

/// Même bouton, dans un rond du même gabarit que la pastille Bluetooth
/// de la liste des manettes.
fn paint_pad_power_circled(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    power: PadPowerControl,
    hovered: bool,
) {
    if matches!(power, PadPowerControl::Hidden) {
        return;
    }
    let center = rect.center();
    let radius = (rect.width() / 2.0 - 1.0).max(2.0);
    let active = pad_power_symbol(power).map_or(false, |(_, _, active)| active);
    if active {
        ui.painter()
            .circle_filled(center, radius, with_alpha(WARN, 0.18));
    } else if hovered {
        ui.painter().circle_filled(center, radius, CARD_HOVER());
    }
    ui.painter().circle_stroke(
        center,
        radius - 0.5,
        egui::Stroke::new(
            1.0_f32,
            if active {
                WARN
            } else {
                with_alpha(TEXT_DIM(), 0.55)
            },
        ),
    );
    if pad_power_pending(power) {
        paint_spinner(ui, rect);
    } else if let Some((glyph, color, _)) = pad_power_symbol(power) {
        ui.painter().text(
            center,
            egui::Align2::CENTER_CENTER,
            glyph,
            icon_font((rect.height() * 0.55).max(10.0)),
            color,
        );
    }
}

/// Bouton d'alimentation autonome (en-tête) : renvoie vrai quand il faut agir.
fn pad_power_button_at(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    id: egui::Id,
    power: PadPowerControl,
) -> bool {
    let enabled = !matches!(power, PadPowerControl::Hidden | PadPowerControl::Wired);
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let response = ui.interact(rect, id, sense);
    let response = if enabled {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response
    };
    paint_pad_power(ui, rect, power, response.hovered());
    let clicked = enabled && response.clicked();
    response.on_hover_text(pad_power_hint(power));
    clicked
}

/// Colonne de gauche de la fiche : nœud, identifiants et entrées evdev.
fn controller_metadata(ui: &mut egui::Ui, device: &DeviceInfo) {
    let node = device
        .path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "—".to_string());
    let axes = if device.axes.is_empty() {
        lapinux_core::i18n::t("controllers.information.none").to_string()
    } else {
        device.axes.join(", ")
    };
    egui::Grid::new("controller-detail-grid")
        .num_columns(2)
        .spacing([16.0, 6.0])
        .show(ui, |ui| {
            detail_row(
                ui,
                lapinux_core::i18n::t("controllers.info.node_label"),
                &node,
            );
            detail_row(
                ui,
                lapinux_core::i18n::t("controllers.info.identifier_label"),
                &format!("{:04x}:{:04x}", device.vendor, device.product),
            );
            ui.label(
                egui::RichText::new(lapinux_core::i18n::t("controllers.info.inputs"))
                    .size(11.0)
                    .color(TEXT_DIM()),
            );
            ui.label(
                egui::RichText::new(lapinux_core::i18n::t_args(
                    "controllers.info.input_counts",
                    &[
                        ("buttons", &device.buttons.to_string()),
                        ("axes", &device.axes.len().to_string()),
                    ],
                ))
                .size(11.0),
            )
            .on_hover_text(lapinux_core::i18n::t_args(
                "controllers.info.axes_hint",
                &[("axes", &axes)],
            ));
            ui.end_row();
            if let Some(address) = &device.address {
                detail_row(
                    ui,
                    lapinux_core::i18n::t("controllers.info.address_label"),
                    address,
                );
            }
            if device.touchpad {
                detail_row(
                    ui,
                    lapinux_core::i18n::t("controllers.info.touchpad_label"),
                    lapinux_core::i18n::t("controllers.info.yes"),
                );
            }
        });
    if !device.button_names.is_empty() {
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(lapinux_core::i18n::t("controllers.info.buttons"))
                .size(11.0)
                .color(TEXT_DIM()),
        );
        ui.label(
            egui::RichText::new(device.button_names.join(" · "))
                .size(10.5)
                .color(TEXT()),
        )
        .on_hover_text(lapinux_core::i18n::t("controllers.info.buttons_hint"));
    }
}

/// Colonne de droite de la fiche : une seule liste, adaptée à la largeur.
fn controller_feature_list(
    ui: &mut egui::Ui,
    features: &[&str],
    dongle_axes_toggle: bool,
    hide_dongle_axes: &mut bool,
) {
    ui.label(
        egui::RichText::new(lapinux_core::i18n::t("controllers.feature.title"))
            .size(12.0)
            .strong()
            .color(TEXT()),
    );
    ui.add_space(6.0);
    let columns = if ui.available_width() >= 480.0 { 3 } else { 2 };
    egui::Grid::new("controller-features")
        .num_columns(columns)
        .spacing([18.0, 5.0])
        .show(ui, |ui| {
            for (index, feature) in features.iter().enumerate() {
                ui.horizontal(|ui| {
                    status_dot(ui, with_alpha(OK, 0.75));
                    ui.label(
                        egui::RichText::new(feature_label(*feature))
                            .size(11.0)
                            .color(TEXT()),
                    );
                });
                if (index + 1) % columns == 0 {
                    ui.end_row();
                }
            }
            if features.len() % columns != 0 {
                ui.end_row();
            }
        });
    if dongle_axes_toggle {
        ui.add_space(8.0);
        let mut show = !*hide_dongle_axes;
        let response = ui.checkbox(
            &mut show,
            egui::RichText::new(lapinux_core::i18n::t("controllers.feature.show_controls"))
                .size(10.5),
        );
        if response.changed() {
            *hide_dongle_axes = !show;
        }
        response.on_hover_text(lapinux_core::i18n::t(
            "controllers.feature.show_controls_hint",
        ));
        if *hide_dongle_axes {
            // Sans ce rappel, la fiche semble simplement incomplète : les
            // tests des sticks, des gâchettes et des vibrations disparaissent
            // avec les fonctionnalités.
            ui.add_space(2.0);
            muted(ui, lapinux_core::i18n::t("controllers.feature.hidden_hint"));
        }
    }
}

/// Libellé traduit d'une fonctionnalité : les identifiants internes restent
/// stables, seule l'affichage passe par le catalogue.
fn feature_label(feature: &str) -> &str {
    let key = match feature {
        "sticks" => "controllers.feature.sticks",
        "dpad" => "controllers.feature.dpad",
        "triggers" => "controllers.feature.triggers",
        "touchpad" => "controllers.feature.touchpad",
        "gyro" => "controllers.feature.gyro",
        "accel" => "controllers.feature.accel",
        "vibrations" => "controllers.feature.vibrations",
        "hd_vibrations" => "controllers.feature.hd_vibrations",
        "paddle" => "controllers.feature.paddle",
        "paddles" => "controllers.feature.paddles",
        "extra_buttons" => "controllers.feature.extra_buttons",
        "battery" => "controllers.feature.battery",
        "bluetooth" => "controllers.feature.bluetooth",
        "haptics" => "controllers.feature.haptics",
        "adaptive_triggers" => "controllers.feature.adaptive_triggers",
        "led" => "controllers.feature.led",
        "microphone" => "controllers.feature.microphone",
        "speaker" => "controllers.feature.speaker",
        "headset_jack" => "controllers.feature.headset_jack",
        "impulse_triggers" => "controllers.feature.impulse_triggers",
        "amiibo" => "controllers.feature.amiibo",
        "touch_pads" => "controllers.feature.touch_pads",
        "Grip Sense" => "controllers.feature.grip_sense",
        "pad_haptics" => "controllers.feature.pad_haptics",
        _ => return feature,
    };
    lapinux_core::i18n::t(key)
}

/// Retire de la liste les fonctions optionnelles qu'un dongle annonce sans
/// qu'elles soient réellement présentes sur la manette.
fn hide_dongle_features(features: &mut Vec<&'static str>) {
    features.retain(|feature| {
        !matches!(
            *feature,
            "sticks" | "triggers" | "haptics" | "vibrations" | "hd_vibrations"
        )
    });
}

/// Boutons qui ne sont couverts par aucune carte de test standard : palettes,
/// boutons de raccourci, entrées constructeur. Ils méritent leur propre test.
fn is_extra_button(label: &str) -> bool {
    !matches!(
        label,
        "A" | "B"
            | "X"
            | "Y"
            | "Capture"
            | "LB"
            | "RB"
            | "LT"
            | "RT"
            | "Select"
            | "Start"
            | "Guide"
            | "L3"
            | "R3"
            | "Haut"
            | "Bas"
            | "Gauche"
            | "Droite"
    )
}

fn has_axis(device: &DeviceInfo, name: &str) -> bool {
    device.axes.iter().any(|axis| axis == name)
}

fn has_button(device: &DeviceInfo, name: &str) -> bool {
    device.button_names.iter().any(|button| button == name)
}

fn has_sticks(device: &DeviceInfo) -> bool {
    has_axis(device, "ABS_X") && has_axis(device, "ABS_Y")
}

fn has_dpad(device: &DeviceInfo) -> bool {
    has_axis(device, "ABS_HAT0X")
        || has_axis(device, "ABS_HAT0Y")
        || ["Haut", "Bas", "Gauche", "Droite"]
            .iter()
            .any(|name| has_button(device, name))
}

fn has_shoulders(device: &DeviceInfo) -> bool {
    has_button(device, "LB") || has_button(device, "RB")
}

/// Une gâchette est analogique (ABS_Z/ABS_RZ) ou tout ou rien (LT/RT).
fn has_triggers(device: &DeviceInfo) -> bool {
    has_axis(device, "ABS_Z")
        || has_axis(device, "ABS_RZ")
        || has_axis(device, "ABS_GAS")
        || has_axis(device, "ABS_BRAKE")
        || has_button(device, "LT")
        || has_button(device, "RT")
}

/// Fonctionnalités prises en charge, détectées puis complétées par le modèle
/// connu. Une seule liste alimente la fiche de la manette.
fn controller_features(
    device: &DeviceInfo,
    identity: &PadIdentity,
    pad: Option<&lapinux_input::PadState>,
) -> Vec<&'static str> {
    let mut features: Vec<&'static str> = Vec::new();
    let mut add = |feature: &'static str| {
        if !features.contains(&feature) {
            features.push(feature);
        }
    };

    // Bluetooth reste dans la liste même quand le câble est utilisé : c'est
    // une capacité de la manette, pas l'état de la liaison du moment. Un pad
    // générique en dongle 2,4 GHz n'a en revanche aucune raison de l'annoncer.
    let bluetooth = device.connection == DeviceLink::Bluetooth
        || matches!(
            identity.family,
            Some("PS5")
                | Some("PS4")
                | Some("PS3")
                | Some("Switch")
                | Some("Xbox One")
                | Some("Xbox Series")
                | Some("Steam")
                | Some("Steam Deck")
        );
    if bluetooth {
        add("bluetooth");
    }
    if has_sticks(device) {
        add("sticks");
    }
    if has_dpad(device) {
        add("dpad");
    }
    if has_triggers(device) {
        add("triggers");
    }
    if device.touchpad || pad.map_or(false, |pad| pad.touchpad.is_some()) {
        add("touchpad");
    }
    if pad.map_or(false, |pad| pad.motion.is_some()) {
        add("gyro");
        add("accel");
    }
    // Détection générique, avant les listes par modèle : une manette
    // inconnue garde tout ce que son nœud evdev expose réellement.
    if device.force_feedback {
        add("vibrations");
    }
    // Les noms evdev peuvent être anglais (« Paddle ») ou hérités d'un
    // ancien noyau localisé (« Palette ») : les deux écritures désignent la
    // même commande physique et restent reconnues.
    if device.button_names.iter().any(|name| {
        let name = name.to_lowercase();
        name.contains("paddle") || name.contains("palette")
    }) {
        add("paddles");
    }
    if device.button_names.iter().any(|name| is_extra_button(name)) {
        add("extra_buttons");
    }
    if device.battery.is_some()
        || device.battery_status.is_some()
        || pad
            .and_then(|pad| pad.battery.as_ref())
            .map_or(false, |battery| {
                battery.percent.is_some() || battery.voltage_mv.is_some()
            })
    {
        add("battery");
    }

    match identity.family {
        Some("PS5") => {
            for feature in [
                "sticks",
                "dpad",
                "triggers",
                "touchpad",
                "gyro",
                "accel",
                "haptics",
                "adaptive_triggers",
                "led",
                "microphone",
                "speaker",
                "headset_jack",
            ] {
                add(feature);
            }
        }
        Some("PS4") => {
            for feature in [
                "sticks",
                "dpad",
                "triggers",
                "touchpad",
                "gyro",
                "accel",
                "vibrations",
                "led",
                "speaker",
                "headset_jack",
            ] {
                add(feature);
            }
        }
        Some("PS3") => {
            for feature in ["sticks", "dpad", "triggers", "gyro", "accel", "vibrations"] {
                add(feature);
            }
        }
        Some("Switch") => {
            for feature in [
                "sticks",
                "dpad",
                "triggers",
                "gyro",
                "accel",
                "hd_vibrations",
                "amiibo",
            ] {
                add(feature);
            }
        }
        Some("Xbox One") | Some("Xbox Series") => {
            for feature in [
                "sticks",
                "dpad",
                "triggers",
                "vibrations",
                "impulse_triggers",
            ] {
                add(feature);
            }
        }
        Some("Steam") | Some("Steam Deck") => {
            for feature in [
                "sticks",
                "dpad",
                "triggers",
                "touchpad",
                "gyro",
                "accel",
                "vibrations",
            ] {
                add(feature);
            }
        }
        _ => {}
    }
    if lapinux_input::steam::is_steam_2026(device) {
        add("touch_pads");
        add("Grip Sense");
        add("pad_haptics");
    }
    features
}

#[derive(Clone, Copy)]
enum TestKind {
    Battery,
    Face,
    System,
    Dpad,
    Sticks,
    Shoulders,
    Triggers,
    Touchpad,
    SteamPads,
    SteamPadHaptics,
    SteamGripHaptics,
    Gyro,
    Accel,
    Microphone,
    Vibration,
    AdaptiveTriggers,
    Led,
    Speaker,
    Headset,
    Bluetooth,
    Nfc,
}

struct TestCard {
    title: &'static str,
    hint: Option<&'static str>,
    kind: TestKind,
}

fn controller_test_panel(
    ui: &mut egui::Ui,
    device: &DeviceInfo,
    pad: Option<&lapinux_input::PadState>,
    brand: Option<Brand>,
    face_profile: FaceButtonProfile,
    family: Option<&'static str>,
    features: &[&str],
    tests: &mut PadTestState,
    audio: Option<&PadAudio>,
) {
    egui::Frame::none()
        .fill(CARD())
        .stroke(egui::Stroke::new(1.0_f32, BORDER()))
        .rounding(egui::Rounding::same(12.0))
        .inner_margin(14.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                egui::RichText::new(lapinux_core::i18n::t("controllers.test.live"))
                    .size(13.0)
                    .strong(),
            );
            ui.add_space(12.0);
            let Some(pad) = pad else {
                ui.add_space(8.0);
                muted(ui, lapinux_core::i18n::t("controllers.test.waiting"));
                return;
            };
            let mut cards = vec![
                test_card(
                    lapinux_core::i18n::t("controllers.test.face"),
                    None,
                    TestKind::Face,
                ),
                test_card(
                    lapinux_core::i18n::t("controllers.test.system"),
                    None,
                    TestKind::System,
                ),
            ];
            if has_dpad(device) {
                cards.push(test_card(
                    lapinux_core::i18n::t("controllers.feature.dpad"),
                    None,
                    TestKind::Dpad,
                ));
            }
            if features.contains(&"sticks") {
                cards.push(test_card(
                    lapinux_core::i18n::t("controllers.feature.sticks"),
                    None,
                    TestKind::Sticks,
                ));
            }
            if has_shoulders(device) {
                cards.push(test_card(
                    lapinux_core::i18n::t("controllers.test.shoulders"),
                    None,
                    TestKind::Shoulders,
                ));
            }
            if features.contains(&"triggers") {
                cards.push(test_card(
                    lapinux_core::i18n::t("controllers.feature.triggers"),
                    None,
                    TestKind::Triggers,
                ));
            }
            let mut insert_at = 0;
            if device.battery.is_some() || device.battery_status.is_some() || pad.battery.is_some()
            {
                cards.insert(
                    0,
                    test_card(
                        lapinux_core::i18n::t("controllers.feature.battery"),
                        Some(lapinux_core::i18n::t("controllers.test.battery_hint")),
                        TestKind::Battery,
                    ),
                );
                insert_at = 1;
            }
            // La fiche de la manette est la seule source de vérité : la carte
            // de test suit la liste des fonctionnalités.
            if features.contains(&"bluetooth") {
                cards.insert(
                    insert_at,
                    test_card(
                        lapinux_core::i18n::t("controllers.feature.bluetooth"),
                        Some(lapinux_core::i18n::t("controllers.test.bluetooth_hint")),
                        TestKind::Bluetooth,
                    ),
                );
            }
            if lapinux_input::steam::is_steam_2026(device) {
                cards.push(test_card(
                    lapinux_core::i18n::t("controllers.test.steam_pads"),
                    Some(lapinux_core::i18n::t("controllers.test.steam_pads_hint")),
                    TestKind::SteamPads,
                ));
                cards.push(test_card(
                    lapinux_core::i18n::t("controllers.feature.pad_haptics"),
                    Some(lapinux_core::i18n::t("controllers.test.pad_haptics_hint")),
                    TestKind::SteamPadHaptics,
                ));
                cards.push(test_card(
                    lapinux_core::i18n::t("controllers.test.grip_haptics"),
                    Some(lapinux_core::i18n::t("controllers.test.grip_haptics_hint")),
                    TestKind::SteamGripHaptics,
                ));
            } else if device.touchpad || pad.touchpad.is_some() {
                cards.push(test_card(
                    lapinux_core::i18n::t("controllers.feature.touchpad"),
                    None,
                    TestKind::Touchpad,
                ));
            }
            if pad.motion.is_some() {
                cards.push(test_card(
                    lapinux_core::i18n::t("controllers.feature.gyro"),
                    Some(lapinux_core::i18n::t("controllers.test.gyro_hint")),
                    TestKind::Gyro,
                ));
                cards.push(test_card(
                    lapinux_core::i18n::t("controllers.feature.accel"),
                    Some(lapinux_core::i18n::t("controllers.test.accel_hint")),
                    TestKind::Accel,
                ));
            }
            if features.contains(&"amiibo") {
                cards.push(test_card(
                    lapinux_core::i18n::t("controllers.feature.amiibo"),
                    Some(lapinux_core::i18n::t("controllers.test.nfc_hint")),
                    TestKind::Nfc,
                ));
            }
            if features.contains(&"microphone") {
                cards.push(test_card(
                    lapinux_core::i18n::t("controllers.feature.microphone"),
                    None,
                    TestKind::Microphone,
                ));
            }
            if features
                .iter()
                .any(|feature| matches!(*feature, "vibrations" | "hd_vibrations" | "haptics"))
            {
                let haptic = matches!(family, Some("PS5"));
                cards.push(test_card(
                    if haptic {
                        lapinux_core::i18n::t("controllers.feature.haptics")
                    } else {
                        lapinux_core::i18n::t("controllers.feature.vibrations")
                    },
                    if haptic {
                        Some(lapinux_core::i18n::t("controllers.test.haptics_left_right"))
                    } else {
                        Some(lapinux_core::i18n::t(
                            "controllers.test.vibration_intensity",
                        ))
                    },
                    TestKind::Vibration,
                ));
            }
            if features.contains(&"adaptive_triggers") {
                cards.push(test_card(
                    lapinux_core::i18n::t("controllers.feature.adaptive_triggers"),
                    Some(lapinux_core::i18n::t("controllers.test.adaptive_hint")),
                    TestKind::AdaptiveTriggers,
                ));
            }
            if matches!(family, Some("PS5") | Some("PS4")) && features.contains(&"led") {
                cards.push(test_card(
                    lapinux_core::i18n::t("controllers.test.led"),
                    Some(lapinux_core::i18n::t("controllers.test.led_hint")),
                    TestKind::Led,
                ));
            }
            if features.contains(&"speaker") {
                cards.push(test_card(
                    lapinux_core::i18n::t("controllers.feature.speaker"),
                    Some(lapinux_core::i18n::t("controllers.test.speaker_hint")),
                    TestKind::Speaker,
                ));
            }
            if features.contains(&"headset_jack") {
                cards.push(test_card(
                    lapinux_core::i18n::t("controllers.feature.headset_jack"),
                    Some(lapinux_core::i18n::t("controllers.test.headset_hint")),
                    TestKind::Headset,
                ));
            }
            test_card_grid(
                ui,
                &cards,
                pad,
                brand,
                face_profile,
                family,
                device,
                tests,
                audio,
            );
        });
}

fn test_card(title: &'static str, hint: Option<&'static str>, kind: TestKind) -> TestCard {
    TestCard { title, hint, kind }
}

fn test_card_grid(
    ui: &mut egui::Ui,
    cards: &[TestCard],
    pad: &lapinux_input::PadState,
    brand: Option<Brand>,
    face_profile: FaceButtonProfile,
    family: Option<&'static str>,
    device: &DeviceInfo,
    tests: &mut PadTestState,
    audio: Option<&PadAudio>,
) {
    let available = ui.available_width();
    let columns = if available >= 780.0 {
        3
    } else if available >= 480.0 {
        2
    } else {
        1
    };
    let gap = 10.0;
    let width = ((available - gap * (columns - 1) as f32) / columns as f32).max(150.0);
    for row in cards.chunks(columns) {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for card in row {
                ui.allocate_ui_with_layout(
                    egui::vec2(width, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(width);
                        egui::Frame::none()
                            .fill(PANEL())
                            .stroke(egui::Stroke::new(1.0_f32, BORDER()))
                            .rounding(egui::Rounding::same(10.0))
                            .inner_margin(12.0)
                            .show(ui, |ui| {
                                ui.set_width(width - 24.0);
                                // Même hauteur pour toutes les cartes d'une
                                // rangée : les tests courts ne flottent plus au
                                // milieu d'une grille irrégulière.
                                ui.set_min_height(180.0);
                                ui.label(
                                    egui::RichText::new(card.title)
                                        .size(11.5)
                                        .strong()
                                        .color(TEXT()),
                                );
                                if let Some(hint) = card.hint {
                                    ui.label(egui::RichText::new(hint).size(9.5).color(TEXT_DIM()));
                                }
                                ui.add_space(8.0);
                                test_card_content(
                                    ui,
                                    card.kind,
                                    pad,
                                    brand,
                                    face_profile,
                                    family,
                                    device,
                                    tests,
                                    audio,
                                );
                            });
                    },
                );
            }
        });
        ui.add_space(gap);
    }
}

fn test_card_content(
    ui: &mut egui::Ui,
    kind: TestKind,
    pad: &lapinux_input::PadState,
    brand: Option<Brand>,
    face_profile: FaceButtonProfile,
    family: Option<&'static str>,
    device: &DeviceInfo,
    tests: &mut PadTestState,
    audio: Option<&PadAudio>,
) {
    match kind {
        TestKind::Face => {
            ui.vertical_centered(|ui| face_buttons_widget(ui, pad, brand, face_profile, device));
        }
        TestKind::System => {
            ui.vertical_centered(|ui| system_buttons_widget(ui, pad, brand, device));
        }
        TestKind::Dpad => {
            ui.vertical_centered(|ui| dpad_widget(ui, pad));
        }
        TestKind::Sticks => {
            ui.vertical_centered(|ui| sticks_widget(ui, pad));
        }
        TestKind::Shoulders => {
            ui.vertical_centered(|ui| shoulder_buttons_widget(ui, pad, brand));
        }
        TestKind::Triggers => {
            trigger_bars_widget(ui, pad, brand);
        }
        TestKind::Touchpad => {
            ui.vertical_centered(|ui| touchpad_widget(ui, pad));
        }
        TestKind::SteamPads => steam_pads_test(ui, device, pad, tests),
        TestKind::SteamPadHaptics => steam_pad_haptics_test(ui, device, tests),
        TestKind::SteamGripHaptics => steam_grip_haptics_test(ui, device, tests),
        TestKind::Gyro => {
            if let Some(motion) = &pad.motion {
                motion_axes(ui, &motion.gyro, "°/s", 0);
            }
        }
        TestKind::Accel => {
            if let Some(motion) = &pad.motion {
                motion_axes(ui, &motion.accel, "g", 2);
            }
        }
        TestKind::Microphone => microphone_test(ui, pad, audio, tests),
        TestKind::Battery => battery_test(ui, device, Some(pad)),
        TestKind::Bluetooth => bluetooth_test(ui, device, pad, tests),
        TestKind::Vibration => {
            vibration_test(ui, device, tests, matches!(family, Some("PS5")), audio)
        }
        TestKind::AdaptiveTriggers => adaptive_triggers_test(ui, device, tests),
        TestKind::Led if matches!(family, Some("PS5") | Some("PS4")) => led_test(ui, device, tests),
        TestKind::Led => {}
        TestKind::Speaker => speaker_test(ui, audio, tests),
        TestKind::Headset => headset_test(ui, audio),
        TestKind::Nfc => nfc_test(ui),
    }
}

/// Le lecteur amiibo est dans la manette, mais le pilote Linux ne l'expose pas
/// à l'espace utilisateur : la carte le dit, plutôt que d'afficher un test
/// factice qui ne lirait jamais rien.
fn nfc_test(ui: &mut egui::Ui) {
    ui.vertical_centered(|ui| {
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.add_space(2.0);
            status_dot(ui, WARN);
            ui.label(egui::RichText::new("Indisponible").size(11.0).color(WARN))
                .on_hover_text(
                    "Les émulateurs ne lisent pas ce lecteur : ils simulent la console\n\
                     et chargent un fichier amiibo. Le NFC physique de la manette\n\
                     demande un pilote dédié qui prend le contrôle de son MCU.",
                );
        });
        ui.add_space(5.0);
        muted(ui, "Le pilote Linux ne transmet pas");
        muted(ui, "le lecteur NFC de la manette.");
        muted(ui, "Il faudrait un pilote dédié.");
    });
}

/// Jauge segmentée du microphone : elle reste vide quand aucune source audio
/// n'est exposée, pour ne pas inventer un niveau.
fn microphone_test(
    ui: &mut egui::Ui,
    pad: &lapinux_input::PadState,
    audio: Option<&PadAudio>,
    tests: &mut PadTestState,
) {
    let mic_muted = pad.microphone_muted;
    let (text, color) = match mic_muted {
        Some(true) => ("Micro coupé", WARN),
        Some(false) => ("Micro actif", OK),
        None => ("État non exposé", TEXT_DIM()),
    };
    let source = audio.and_then(|audio| audio.source());
    let level = if mic_muted == Some(true) {
        0.0
    } else {
        audio.map(|audio| audio.mic_level()).unwrap_or(0.0)
    };
    ui.vertical_centered(|ui| {
        ui.horizontal(|ui| {
            ui.add_space(2.0);
            status_dot(ui, color);
            ui.label(egui::RichText::new(text).size(11.0).color(color));
        });
        ui.add_space(2.0);
        mic_gauge(ui, level, source.is_some());
        ui.label(
            egui::RichText::new(format!("Niveau {:>3} %", (level * 100.0).round()))
                .size(9.5)
                .color(TEXT_DIM()),
        );
        ui.add_space(2.0);
        if source.is_none() {
            muted(ui, lapinux_core::i18n::t("controllers.mic_missing"));
            muted(ui, lapinux_core::i18n::t("controllers.mic_plug"));
        } else {
            let label = if tests.mic_monitor {
                lapinux_core::i18n::t("controllers.stop_listening")
            } else {
                lapinux_core::i18n::t("controllers.listen")
            };
            if secondary_button(ui, label).clicked() {
                tests.mic_monitor = !tests.mic_monitor;
                tests.mic_note = None;
            }
            if let Some(note) = &tests.mic_note {
                muted(ui, note);
            } else if tests.mic_monitor {
                muted(ui, "Casque conseillé : évite le larsen.");
            } else if mic_muted == Some(true) {
                muted(ui, "Appuie sur le bouton mute de la manette.");
            } else {
                muted(ui, "Le son sort sur la sortie système.");
            }
        }
    });
}

fn mic_gauge(ui: &mut egui::Ui, level: f32, live: bool) {
    let width = ui.available_width().min(190.0).max(80.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 13.0), egui::Sense::hover());
    let painter = ui.painter();
    let segments = 18usize;
    let gap = 2.0;
    let segment = ((rect.width() - gap * (segments - 1) as f32) / segments as f32).max(1.0);
    let lit = (level.clamp(0.0, 1.0) * segments as f32).round() as usize;
    for index in 0..segments {
        let left = rect.left() + index as f32 * (segment + gap);
        let color = if live && index < lit {
            if index > segments - 4 {
                WARN
            } else {
                OK
            }
        } else {
            with_alpha(TEXT_DIM(), 0.16)
        };
        painter.rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(left, rect.top()),
                egui::vec2(segment, rect.height()),
            ),
            egui::Rounding::same(1.5),
            color,
        );
    }
}

fn vibration_test(
    ui: &mut egui::Ui,
    device: &DeviceInfo,
    tests: &mut PadTestState,
    haptic: bool,
    audio: Option<&PadAudio>,
) {
    if haptic {
        haptic_test(ui, device, tests, audio);
        return;
    }
    ui.vertical_centered(|ui| {
        ui.spacing_mut().slider_width = (ui.available_width() - 34.0).clamp(90.0, 180.0);
        ui.add(
            egui::Slider::new(&mut tests.vibration, 10.0..=100.0)
                .show_value(false)
                .trailing_fill(true),
        );
        ui.label(
            egui::RichText::new(format!("Intensité {:.0} %", tests.vibration))
                .size(9.5)
                .color(TEXT_DIM()),
        );
        ui.add_space(2.0);
        if secondary_button(ui, lapinux_core::i18n::t("controllers.vibrate")).clicked() {
            tests.vibration_note = Some(
                match lapinux_input::control::rumble(device, tests.vibration / 100.0) {
                    Ok(()) => lapinux_core::i18n::t("controllers.vibration_sent").to_string(),
                    Err(err) => lapinux_core::i18n::t_args(
                        "controllers.vibration_failed",
                        &[("error", &err.to_string())],
                    ),
                },
            );
        }
        if let Some(note) = &tests.vibration_note {
            muted(ui, note);
        }
    });
}

/// Test haptique précis, sur deux chemins :
/// - **HD** : un vrai son de bobine par le flux audio Bluetooth (rapport
///   0x32), avec fréquence et niveau gauche/droite séparés ;
/// - **Moteurs** : le rumble des deux moteurs, en motifs réels (continu,
///   impulsions, rampe, battement), pour comparer avec le rendu HD.
fn haptic_test(
    ui: &mut egui::Ui,
    device: &DeviceInfo,
    tests: &mut PadTestState,
    audio: Option<&PadAudio>,
) {
    use lapinux_input::control::HapticPattern;
    let over_bluetooth = matches!(device.connection, DeviceLink::Bluetooth);
    // En USB, les bobines HD passent par les canaux arrière de la carte son
    // quatre canaux de la manette : elles ne demandent pas le sans-fil.
    let over_usb = matches!(device.connection, DeviceLink::Usb | DeviceLink::Dongle)
        && audio.map_or(false, |audio| audio.sink().is_some());
    let hd_available = over_bluetooth || over_usb;
    let mut chosen_pattern = None;
    let mut chosen_mode = None;
    ui.vertical_centered(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(5.0, 4.0);
        if hd_available {
            ui.horizontal(|ui| {
                for (hd, label, hint) in [
                    (
                        true,
                        "HD",
                        "Bobines audio du câble USB ou du Bluetooth, comme les jeux.",
                    ),
                    (false, "Moteurs", "Rumble classique des deux moteurs."),
                ] {
                    let shown = if tests.haptic_hd == hd {
                        format!("• {label}")
                    } else {
                        label.to_string()
                    };
                    if secondary_button(ui, &shown).on_hover_text(hint).clicked() {
                        chosen_mode = Some(hd);
                    }
                }
            });
            if let Some(hd) = chosen_mode {
                if tests.haptic_hd != hd {
                    tests.vibration_note = None;
                }
                tests.haptic_hd = hd;
            }
        }
        let hd = hd_available && tests.haptic_hd;
        if hd {
            haptic_slider(ui, "Fréq.", &mut tests.haptic_tone, 20.0, 300.0, " Hz");
            haptic_slider(ui, "Gauche", &mut tests.haptic_left, 0.0, 100.0, "%");
            haptic_slider(ui, "Droite", &mut tests.haptic_right, 0.0, 100.0, "%");
            haptic_slider(ui, lapinux_core::i18n::t("downloads.duration"), &mut tests.haptic_duration, 0.2, 5.0, " s");
            ui.add_space(2.0);
            if secondary_button(ui, "Tester")
                .on_hover_text(if over_bluetooth {
                    "Envoie un son de bobine à 3 kHz par le flux audio Bluetooth de la DualSense."
                } else {
                    "Envoie un son de bobine par les canaux arrière de la carte son USB ; le volume du haut-parleur règle aussi la force."
                })
                .clicked()
            {
                let duration_ms = (tests.haptic_duration * 1000.0).round() as u16;
                let result: Result<(), String> = if over_bluetooth {
                    lapinux_input::control::haptic_hd(
                        device,
                        tests.haptic_left / 100.0,
                        tests.haptic_right / 100.0,
                        tests.haptic_tone,
                        duration_ms,
                    )
                    .map_err(|err| err.to_string())
                } else {
                    audio.map_or_else(
                        || Err("carte son de la manette introuvable".to_string()),
                        |audio| {
                            audio.play_haptics(
                                tests.haptic_left / 100.0,
                                tests.haptic_right / 100.0,
                                tests.haptic_tone,
                                duration_ms,
                            )
                        },
                    )
                };
                tests.vibration_note = Some(match result {
                    Ok(()) => format!(
                        "HD · {:.0} Hz · G {:.0} % · D {:.0} % · {} s",
                        tests.haptic_tone,
                        tests.haptic_left,
                        tests.haptic_right,
                        format!("{:.1}", tests.haptic_duration).replace('.', ",")
                    ),
                    Err(err) => format!("HD indisponible : {err}"),
                });
            }
        } else {
            ui.horizontal_wrapped(|ui| {
                for pattern in [
                    HapticPattern::Steady,
                    HapticPattern::Pulse,
                    HapticPattern::Ramp,
                    HapticPattern::Heartbeat,
                ] {
                    let label = if tests.haptic_pattern == pattern {
                        format!("• {}", pattern.label())
                    } else {
                        pattern.label().to_string()
                    };
                    if secondary_button(ui, &label)
                        .on_hover_text(pattern.hint())
                        .clicked()
                    {
                        chosen_pattern = Some(pattern);
                    }
                }
            });
            if let Some(pattern) = chosen_pattern {
                tests.haptic_pattern = pattern;
            }
            haptic_slider(ui, "Gauche", &mut tests.haptic_left, 0.0, 100.0, "%");
            haptic_slider(ui, "Droite", &mut tests.haptic_right, 0.0, 100.0, "%");
            ui.add_enabled_ui(tests.haptic_pattern.uses_rate(), |ui| {
                haptic_slider(ui, "Cadence", &mut tests.haptic_rate, 1.0, 12.0, " Hz");
            });
            haptic_slider(ui, lapinux_core::i18n::t("downloads.duration"), &mut tests.haptic_duration, 0.2, 5.0, " s");
            ui.add_space(2.0);
            if secondary_button(ui, "Tester")
                .on_hover_text(
                    "Rumble des deux moteurs ; il n'utilise pas les bobines HD.",
                )
                .clicked()
            {
                let duration_ms = (tests.haptic_duration * 1000.0).round() as u16;
                let result = lapinux_input::control::haptic(
                    device,
                    tests.haptic_pattern,
                    tests.haptic_left / 100.0,
                    tests.haptic_right / 100.0,
                    tests.haptic_rate,
                    duration_ms,
                );
                tests.vibration_note = Some(match result {
                    Ok(()) => format!(
                        "Moteurs · {} · G {:.0} % · D {:.0} % · {} s",
                        tests.haptic_pattern.label(),
                        tests.haptic_left,
                        tests.haptic_right,
                        format!("{:.1}", tests.haptic_duration).replace('.', ",")
                    ),
                    Err(err) => format!("Haptique indisponible : {err}"),
                });
            }
        }
        if let Some(note) = &tests.vibration_note {
            muted(ui, note);
        } else if hd {
            muted(
                ui,
                if over_bluetooth {
                    "Bobines audio · Bluetooth"
                } else {
                    "Bobines audio · USB"
                    },
            );
        } else if hd_available {
            muted(ui, "Deux moteurs, indépendants.");
        } else {
            muted(ui, "HD : Bluetooth ou câble USB requis.");
        }
    });
}

/// Une ligne compacte « libellé → curseur → valeur » pour la carte Haptique.
fn haptic_slider(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    min: f32,
    max: f32,
    suffix: &str,
) {
    ui.horizontal(|ui| {
        ui.add_sized(
            egui::vec2(44.0, 14.0),
            egui::Label::new(egui::RichText::new(label).size(9.5).color(TEXT_DIM())),
        );
        ui.spacing_mut().slider_width = (ui.available_width() - 34.0).clamp(60.0, 170.0);
        ui.add(
            egui::Slider::new(value, min..=max)
                .show_value(false)
                .trailing_fill(true),
        );
        let shown = if suffix == " s" {
            format!("{:.1}{suffix}", value)
        } else {
            format!("{:.0}{suffix}", value)
        };
        ui.label(
            egui::RichText::new(shown.replace('.', ","))
                .size(9.5)
                .color(TEXT_DIM()),
        );
    });
}

fn adaptive_triggers_test(ui: &mut egui::Ui, device: &DeviceInfo, tests: &mut PadTestState) {
    use lapinux_input::control::TriggerTest;
    let mut chosen = None;
    ui.vertical_centered(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(6.0, 5.0);
        ui.horizontal_wrapped(|ui| {
            for test in [
                TriggerTest::Resistance,
                TriggerTest::Block,
                TriggerTest::Vibration,
                TriggerTest::Weapon,
            ] {
                if secondary_button(ui, test.label())
                    .on_hover_text(test.hint())
                    .clicked()
                {
                    chosen = Some(test);
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(lapinux_core::i18n::t("downloads.duration"))
                    .size(9.5)
                    .color(TEXT_DIM()),
            );
            ui.spacing_mut().slider_width = 62.0;
            ui.add(
                egui::Slider::new(&mut tests.trigger_seconds, 1.0..=10.0)
                    .step_by(1.0)
                    .show_value(false)
                    .trailing_fill(true),
            )
            .on_hover_text("Durée du test, de 1 à 10 secondes.");
            ui.label(
                egui::RichText::new(format!("{:.0} s", tests.trigger_seconds))
                    .size(9.5)
                    .color(TEXT_DIM()),
            );
            ui.add_space(4.0);
            if secondary_button(ui, "Relâcher")
                .on_hover_text("Relâche tout de suite R2 et L2.")
                .clicked()
            {
                tests.trigger_note = Some(
                    match lapinux_input::control::dualsense_trigger_off(device) {
                        Ok(()) => "Gâchettes relâchées.".to_string(),
                        Err(err) => format!("Relâchement impossible : {err}"),
                    },
                );
            }
        });
        if let Some(test) = chosen {
            let seconds = tests.trigger_seconds.round().clamp(1.0, 10.0) as u8;
            tests.trigger_note = Some(
                match lapinux_input::control::dualsense_trigger_test(device, test, seconds) {
                    Ok(()) => format!("Test « {} » lancé pour {seconds} s.", test.label()),
                    Err(err) => format!("Gâchettes adaptatives indisponibles : {err}"),
                },
            );
        }
        if let Some(note) = &tests.trigger_note {
            muted(ui, note);
        } else {
            muted(ui, "Choisis un effet, puis il se relâche tout seul.");
        }
    });
}

/// Couleur saturée d'une teinte, de 0 à 360 degrés.
fn hue_rgb(hue: f32) -> [u8; 3] {
    let hue = hue.rem_euclid(360.0);
    let sector = (hue / 60.0).floor() as i32;
    let f = hue / 60.0 - sector as f32;
    let (r, g, b) = match sector.rem_euclid(6) {
        0 => (1.0, f, 0.0),
        1 => (1.0 - f, 1.0, 0.0),
        2 => (0.0, 1.0, f),
        3 => (0.0, 1.0 - f, 1.0),
        4 => (f, 0.0, 1.0),
        _ => (1.0, 0.0, 1.0 - f),
    };
    [
        (r * 255.0).round() as u8,
        (g * 255.0).round() as u8,
        (b * 255.0).round() as u8,
    ]
}

fn hue_name(hue: f32) -> &'static str {
    match hue.rem_euclid(360.0) {
        h if h < 15.0 || h >= 345.0 => "Rouge",
        h if h < 45.0 => "Orange",
        h if h < 75.0 => "Jaune",
        h if h < 150.0 => "Vert",
        h if h < 195.0 => "Cyan",
        h if h < 255.0 => "Bleu",
        h if h < 285.0 => "Violet",
        h if h < 345.0 => "Magenta",
        _ => "Rouge",
    }
}

fn led_test(ui: &mut egui::Ui, device: &DeviceInfo, tests: &mut PadTestState) {
    let mut send: Option<[u8; 3]> = None;
    let mut extinguish = false;
    ui.vertical_centered(|ui| {
        ui.add_space(2.0);
        let width = ui.available_width().clamp(120.0, 260.0);
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(width, 22.0), egui::Sense::click_and_drag());
        let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
        let painter = ui.painter().clone();
        const STEPS: usize = 90;
        for i in 0..STEPS {
            let t0 = i as f32 / STEPS as f32;
            let t1 = (i + 1) as f32 / STEPS as f32;
            let color = hue_rgb((t0 + t1) * 180.0);
            let segment = egui::Rect::from_min_max(
                egui::pos2(rect.left() + rect.width() * t0, rect.top()),
                egui::pos2(rect.left() + rect.width() * t1, rect.bottom()),
            );
            painter.rect_filled(
                segment,
                0.0,
                egui::Color32::from_rgb(color[0], color[1], color[2]),
            );
        }
        painter.rect_stroke(
            rect,
            egui::Rounding::same(7.0),
            egui::Stroke::new(1.0_f32, BORDER()),
        );
        if response.clicked() || response.dragged() {
            if let Some(pos) = response.interact_pointer_pos() {
                let hue = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0) * 360.0;
                if (tests.led_hue - hue).abs() > 0.01 {
                    tests.led_hue = hue;
                }
            }
        }
        let current = hue_rgb(tests.led_hue);
        let shown = if tests.led_on { current } else { [0, 0, 0] };
        let knob = egui::pos2(
            rect.left() + rect.width() * (tests.led_hue / 360.0),
            rect.center().y,
        );
        painter.circle_filled(
            knob,
            9.0,
            egui::Color32::from_rgb(current[0], current[1], current[2]),
        );
        painter.circle_stroke(knob, 9.5, egui::Stroke::new(2.0_f32, egui::Color32::WHITE));
        if response.drag_stopped() || response.clicked() {
            send = Some(current);
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let (dot, _) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::hover());
            ui.painter().circle_filled(
                dot.center(),
                9.0,
                egui::Color32::from_rgb(shown[0], shown[1], shown[2]),
            );
            ui.painter()
                .circle_stroke(dot.center(), 9.0, egui::Stroke::new(1.0_f32, BORDER()));
            ui.label(
                egui::RichText::new(if tests.led_on {
                    format!("{} · {:.0}°", hue_name(tests.led_hue), tests.led_hue)
                } else {
                    "Éteinte".to_string()
                })
                .size(9.5)
                .color(TEXT_DIM()),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if secondary_button(ui, "Éteindre")
                    .on_hover_text("Éteint la barre lumineuse.")
                    .clicked()
                {
                    extinguish = true;
                }
            });
        });
        if extinguish {
            send = Some([0, 0, 0]);
        }
        if let Some(color) = send {
            tests.led_note = Some(match lapinux_input::control::sony_lightbar(device, color) {
                Ok(()) if color == [0, 0, 0] => {
                    tests.led_on = false;
                    "Barre lumineuse éteinte.".to_string()
                }
                Ok(()) => {
                    tests.led_on = true;
                    format!(
                        "Barre lumineuse : {} ({}°).",
                        hue_name(tests.led_hue),
                        tests.led_hue.round()
                    )
                }
                Err(err) => format!("Barre lumineuse indisponible : {err}"),
            });
        }
        ui.add_space(2.0);
        muted(
            ui,
            tests
                .led_note
                .as_deref()
                .unwrap_or("Glisse sur la teinte, puis relâche pour appliquer."),
        );
    });
}

fn speaker_test(ui: &mut egui::Ui, audio: Option<&PadAudio>, tests: &mut PadTestState) {
    let sink = audio.and_then(|audio| audio.sink());
    ui.vertical_centered(|ui| {
        if let Some(audio) = audio.filter(|_| sink.is_some()) {
            // Le curseur montre le volume réel de la manette et le règle sur
            // la destination : le son peut monter bien plus haut qu'un flux
            // de note fixé à 35 %.
            if tests.speaker_for.as_deref() != Some(audio.key.as_str()) {
                tests.speaker_for = Some(audio.key.clone());
                if let Some(volume) = audio.speaker_volume() {
                    tests.speaker_volume = volume as f32;
                }
            }
            if secondary_button(ui, lapinux_core::i18n::t("controllers.speaker_test")).clicked() {
                tests.speaker_note = Some(match audio.play_tone() {
                    Ok(()) => lapinux_core::i18n::t("controllers.speaker_sent").to_string(),
                    Err(err) => lapinux_core::i18n::t_args(
                        "controllers.speaker_failed",
                        &[("error", &err.to_string())],
                    ),
                });
            }
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(format!("Volume · {:.0} %", tests.speaker_volume))
                    .size(10.5)
                    .color(TEXT_DIM()),
            );
            let width = ui.available_width().clamp(80.0, 220.0);
            ui.spacing_mut().slider_width = width;
            let response = ui
                .add(egui::Slider::new(&mut tests.speaker_volume, 0.0..=100.0).show_value(false))
                .on_hover_text(
                    "Volume du haut-parleur de la manette, appliqué au relâchement du curseur.",
                );
            if response.changed() {
                tests.speaker_dirty = true;
            }
            if tests.speaker_dirty && !ui.input(|input| input.pointer.primary_down()) {
                tests.speaker_dirty = false;
                tests.speaker_note = Some(
                    match audio.set_speaker_volume(tests.speaker_volume.round() as u8) {
                        Ok(()) => {
                            format!("Volume du haut-parleur : {:.0} %.", tests.speaker_volume)
                        }
                        Err(err) => format!("Volume non appliqué : {err}"),
                    },
                );
            }
            ui.add_space(2.0);
            muted(
                ui,
                lapinux_core::i18n::t("controllers.speaker_output_hint"),
            );
        } else {
            muted(ui, "Aucune sortie audio de la manette détectée.");
            muted(ui, lapinux_core::i18n::t("controllers.speaker_plug"));
        }
        if let Some(note) = &tests.speaker_note {
            muted(ui, note);
        }
    });
}

fn headset_test(ui: &mut egui::Ui, audio: Option<&PadAudio>) {
    let state = audio
        .map(|audio| audio.headset())
        .unwrap_or(HeadsetState::Unknown);
    let (text, color, detail) = match state {
        HeadsetState::Plugged => (
            "Casque branché",
            OK,
            "La prise jack de la manette est occupée.",
        ),
        HeadsetState::Unplugged => (
            "Aucun casque",
            TEXT_DIM(),
            "Branche un casque sur la manette pour le voir ici.",
        ),
        HeadsetState::Bluetooth => (
            "Non exposée en Bluetooth",
            WARN,
            "Le Bluetooth ne transmet pas l'état de la prise jack.",
        ),
        HeadsetState::Unknown => (
            "Non détectée",
            TEXT_DIM(),
            "Aucune carte audio de la manette n'est visible.",
        ),
    };
    ui.vertical_centered(|ui| {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_space(2.0);
            status_dot(ui, color);
            ui.label(egui::RichText::new(text).size(11.0).color(color));
        });
        ui.add_space(2.0);
        muted(ui, detail);
    });
}

fn face_buttons_widget(
    ui: &mut egui::Ui,
    pad: &lapinux_input::PadState,
    brand: Option<Brand>,
    face_profile: FaceButtonProfile,
    device: &DeviceInfo,
) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(118.0, 118.0), egui::Sense::hover());
    let center = rect.center();
    let labels = face_labels(face_profile);
    let active_buttons = face_active(pad, brand, pad_driver(&device.path).as_deref());
    for (index, (dx, dy)) in [(0.0, -23.0), (23.0, 0.0), (0.0, 23.0), (-23.0, 0.0)]
        .iter()
        .enumerate()
    {
        let active = active_buttons[index];
        paint_face_button(
            ui.painter(),
            center + egui::vec2(*dx, *dy),
            12.5,
            active,
            brand,
            labels[index],
            index,
        );
    }
}

/// Créer, Guide et Options : les boutons système, dans les libellés de la
/// marque. Une manette Switch ajoute son bouton Capture, que le pilote expose
/// comme BTN_Z.
fn system_buttons_widget(
    ui: &mut egui::Ui,
    pad: &lapinux_input::PadState,
    brand: Option<Brand>,
    device: &DeviceInfo,
) {
    let mut buttons = vec![
        ("Select", brand_button_text("Select", brand)),
        ("Guide", brand_button_text("Guide", brand)),
        ("Start", brand_button_text("Start", brand)),
    ];
    if brand == Some(Brand::Nintendo) || has_button(device, "Capture") {
        buttons.push(("Capture", "Capture".to_string()));
    }
    // Les boutons constructeur (palettes, macros, boutons de raccourci) sont
    // exposés par certains pilotes sous forme de BTN_TRIGGER_HAPPY. Ils
    // restent dans la carte système au lieu d'une liste exhaustive séparée.
    for name in &device.button_names {
        if !is_extra_button(name) {
            continue;
        }
        if !buttons.iter().any(|(_, label)| label == name) {
            buttons.push((name.as_str(), name.clone()));
        }
    }
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(5.0, 5.0);
        for (code, label) in &buttons {
            let width = ui
                .painter()
                .layout_no_wrap(label.to_string(), egui::FontId::proportional(8.5), TEXT())
                .size()
                .x
                .clamp(32.0, 92.0)
                + 14.0;
            let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 22.0), egui::Sense::hover());
            let active = pad.pressed.iter().any(|pressed| pressed.as_str() == *code);
            paint_system_button(ui.painter(), rect, label, active);
        }
    });
}

/// Certaines manettes exposent la croix en axes ABS_HAT0X/Y, d'autres en
/// boutons BTN_DPAD_*. Le test doit réagir aux deux.
fn dpad_axes(pad: &lapinux_input::PadState) -> (f32, f32) {
    let mut x = pad.axis("dpad_x");
    let mut y = pad.axis("dpad_y");
    for label in &pad.pressed {
        match label.as_str() {
            "Haut" => y = -1.0,
            "Bas" => y = 1.0,
            "Gauche" | "gauche" | "left" | "Left" => x = -1.0,
            "Droite" | "droite" | "right" | "Right" => x = 1.0,
            _ => {}
        }
    }
    (x, y)
}

fn dpad_widget(ui: &mut egui::Ui, pad: &lapinux_input::PadState) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(118.0, 118.0), egui::Sense::hover());
    let (x, y) = dpad_axes(pad);
    paint_dpad(ui.painter(), rect.center(), 19.5, 13.0, x, y);
}

fn stick_widget(ui: &mut egui::Ui, label: &str, code: &str, x: f32, y: f32, pressed: bool) {
    ui.vertical(|ui| {
        ui.label(
            egui::RichText::new(format!("{label} · {code}"))
                .size(10.0)
                .color(if pressed { OK } else { TEXT_DIM() }),
        );
        let (rect, _) = ui.allocate_exact_size(egui::vec2(72.0, 72.0), egui::Sense::hover());
        paint_stick(ui.painter(), rect.center(), 25.0, x, y);
    });
}

fn sticks_widget(ui: &mut egui::Ui, pad: &lapinux_input::PadState) {
    let left_click = pad.pressed.iter().any(|label| label == "L3");
    let right_click = pad.pressed.iter().any(|label| label == "R3");
    ui.allocate_ui_with_layout(
        egui::vec2(154.0, 96.0),
        egui::Layout::left_to_right(egui::Align::Min),
        |ui| {
            stick_widget(
                ui,
                "Gauche",
                "L3",
                pad.axis("left_x"),
                pad.axis("left_y"),
                left_click,
            );
            ui.add_space(10.0);
            stick_widget(
                ui,
                "Droit",
                "R3",
                pad.axis("right_x"),
                pad.axis("right_y"),
                right_click,
            );
        },
    );
}
fn shoulder_buttons_widget(ui: &mut egui::Ui, pad: &lapinux_input::PadState, brand: Option<Brand>) {
    ui.allocate_ui_with_layout(
        egui::vec2(184.0, 24.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            for (code, width) in [("LB", 84.0), ("RB", 84.0)] {
                if code == "RB" {
                    ui.add_space(8.0);
                }
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(width, 24.0), egui::Sense::hover());
                let active = pad.pressed.iter().any(|label| label == code);
                paint_shoulder(ui.painter(), rect, &brand_button_text(code, brand), active);
            }
        },
    );
}

/// Une gâchette analogique expose un axe ABS_Z/ABS_RZ. Certaines manettes,
/// comme la Pro Controller, n'ont que des contacts : la carte doit alors
/// montrer deux boutons et leur état, pas deux jauges bloquées à zéro.
fn analog_triggers(pad: &lapinux_input::PadState) -> bool {
    pad.axes.contains_key("left_trigger") || pad.axes.contains_key("right_trigger")
}

fn trigger_bars_widget(ui: &mut egui::Ui, pad: &lapinux_input::PadState, brand: Option<Brand>) {
    if !analog_triggers(pad) {
        ui.vertical_centered(|ui| {
            ui.add_space(14.0);
            ui.allocate_ui_with_layout(
                egui::vec2(184.0, 24.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    for (code, width) in [("LT", 84.0), ("RT", 84.0)] {
                        if code == "RT" {
                            ui.add_space(8.0);
                        }
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(width, 24.0), egui::Sense::hover());
                        let active = pad.pressed.iter().any(|label| label == code);
                        paint_shoulder(ui.painter(), rect, &brand_button_text(code, brand), active);
                    }
                },
            );
            ui.add_space(10.0);
            ui.label(
                egui::RichText::new("Course tout ou rien")
                    .size(9.5)
                    .color(TEXT_DIM()),
            );
        });
        return;
    }
    ui.vertical(|ui| {
        for (label, value) in [
            (brand_button_text("LT", brand), pad.axis("left_trigger")),
            (brand_button_text("RT", brand), pad.axis("right_trigger")),
        ] {
            ui.add_space(7.0);
            let width = ui.available_width().min(240.0);
            let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 12.0), egui::Sense::hover());
            paint_trigger(ui.painter(), rect, value, &label);
            ui.add_space(4.0);
        }
    });
}

fn motion_axes(ui: &mut egui::Ui, values: &[f32; 3], unit: &str, decimals: usize) {
    ui.vertical(|ui| {
        for (label, value) in [("X", values[0]), ("Y", values[1]), ("Z", values[2])] {
            motion_axis(ui, label, value, unit, decimals);
        }
    });
}

fn steam_yes_no(value: bool) -> &'static str {
    if value {
        lapinux_core::i18n::t("controllers.information.yes")
    } else {
        lapinux_core::i18n::t("controllers.information.no")
    }
}

/// Position issue du flux evdev, pour le direct sans ouvrir le HID brut.
fn steam_live_pad(pad: &lapinux_input::PadState, side: &str) -> lapinux_input::SteamPad {
    let x = pad.axis(&format!("{side}_pad_x"));
    let y = pad.axis(&format!("{side}_pad_y"));
    lapinux_input::SteamPad {
        touched: x.abs() > 0.01 || y.abs() > 0.01,
        x: ((x + 1.0) * 0.5).clamp(0.0, 1.0),
        y: ((1.0 - y) * 0.5).clamp(0.0, 1.0),
        pressure: 0.0,
    }
}

fn steam_pad_widget(ui: &mut egui::Ui, label: &str, pad: lapinux_input::SteamPad, source: &str) {
    ui.vertical(|ui| {
        let (area, _) = ui.allocate_exact_size(egui::vec2(100.0, 58.0), egui::Sense::hover());
        let painter = ui.painter();
        painter.rect_filled(
            area,
            egui::Rounding::same(8.0),
            egui::Color32::from_rgb(32, 33, 36),
        );
        painter.rect_stroke(
            area,
            egui::Rounding::same(8.0),
            egui::Stroke::new(1.0_f32, BORDER()),
        );
        painter.circle_stroke(
            area.center(),
            area.height() * 0.36,
            egui::Stroke::new(1.0_f32, with_alpha(BORDER(), 0.7)),
        );

        let position = area.min + egui::vec2(pad.x * area.width(), pad.y * area.height());
        let dot = if pad.touched {
            OK
        } else {
            with_alpha(TEXT_DIM(), 0.45)
        };
        painter.circle_filled(position, 3.6, dot);
        if pad.pressure > 0.005 {
            painter.circle_stroke(
                position,
                7.0,
                egui::Stroke::new(1.0_f32, with_alpha(OK, pad.pressure.max(0.35))),
            );
        }
        painter.text(
            area.left_top() + egui::vec2(7.0, 5.0),
            egui::Align2::LEFT_TOP,
            label,
            egui::FontId::proportional(9.0),
            TEXT_DIM(),
        );
        painter.text(
            area.right_bottom() + egui::vec2(-7.0, -5.0),
            egui::Align2::RIGHT_BOTTOM,
            source,
            egui::FontId::proportional(8.5),
            with_alpha(TEXT_DIM(), 0.8),
        );
    });
}

fn steam_pads_test(
    ui: &mut egui::Ui,
    device: &DeviceInfo,
    pad: &lapinux_input::PadState,
    tests: &mut PadTestState,
) {
    let left = steam_live_pad(pad, "left");
    let right = steam_live_pad(pad, "right");
    // Les positions restent alimentées par evdev, même après un relevé HID :
    // le rapport brut ne sert qu'aux bits de contact et de pression, pas à
    // remplacer le flux direct par une position figée.
    let source = "evdev";
    let side_by_side = ui.available_width() >= 215.0;
    let ctx = ui.ctx().clone();

    // Le relevé tourne en fond : cliquer avec la souris oblige à lâcher les
    // poignées, donc le fil attend un court délai avant d'ouvrir hidraw.
    let received = tests.steam_raw_rx.as_ref().map(|rx| rx.try_recv());
    match received {
        Some(Ok(Ok(pads))) => {
            let detected = pads.left.touched
                || pads.right.touched
                || pads.left_stick_touch
                || pads.right_stick_touch
                || pads.left_grip_touch
                || pads.right_grip_touch;
            tests.steam_raw = Some(pads);
            tests.steam_raw_note = Some(if detected {
                lapinux_core::i18n::t("controllers.steam.pads_raw_received_touch").to_string()
            } else {
                lapinux_core::i18n::t("controllers.steam.pads_raw_received_none").to_string()
            });
            tests.steam_raw_rx = None;
        }
        Some(Ok(Err(error))) => {
            tests.steam_raw = None;
            tests.steam_raw_note = Some(lapinux_core::i18n::t_args(
                "controllers.steam.pads_raw_failed",
                &[("error", &error)],
            ));
            tests.steam_raw_rx = None;
        }
        Some(Err(mpsc::TryRecvError::Disconnected)) => {
            tests.steam_raw_note =
                Some(lapinux_core::i18n::t("controllers.steam.pads_raw_interrupted").to_string());
            tests.steam_raw_rx = None;
        }
        Some(Err(mpsc::TryRecvError::Empty)) => {
            ctx.request_repaint_after(Duration::from_millis(80));
        }
        None => {}
    }
    let pending = tests.steam_raw_rx.is_some();
    let raw = tests.steam_raw;

    ui.vertical_centered(|ui| {
        if side_by_side {
            ui.horizontal(|ui| {
                ui.add_space(2.0);
                steam_pad_widget(
                    ui,
                    lapinux_core::i18n::t("controllers.steam.left"),
                    left,
                    source,
                );
                ui.add_space(6.0);
                steam_pad_widget(
                    ui,
                    lapinux_core::i18n::t("controllers.steam.right"),
                    right,
                    source,
                );
            });
        } else {
            steam_pad_widget(
                ui,
                lapinux_core::i18n::t("controllers.steam.left"),
                left,
                source,
            );
            ui.add_space(6.0);
            steam_pad_widget(
                ui,
                lapinux_core::i18n::t("controllers.steam.right"),
                right,
                source,
            );
        }
        ui.add_space(4.0);
        if let Some(raw) = raw {
            ui.label(
                egui::RichText::new(lapinux_core::i18n::t_args(
                    "controllers.steam.pads_contact_seen",
                    &[
                        ("left", steam_yes_no(raw.left.touched)),
                        ("right", steam_yes_no(raw.right.touched)),
                    ],
                ))
                .size(9.5)
                .color(TEXT_DIM()),
            );
            ui.label(
                egui::RichText::new(lapinux_core::i18n::t_args(
                    "controllers.steam.pads_pressure",
                    &[
                        ("left", &format!("{:.0}", raw.left.pressure * 100.0)),
                        ("right", &format!("{:.0}", raw.right.pressure * 100.0)),
                    ],
                ))
                .size(9.5)
                .color(TEXT_DIM()),
            );
            ui.label(
                egui::RichText::new(lapinux_core::i18n::t_args(
                    "controllers.steam.pads_grip_sense",
                    &[
                        ("left", steam_yes_no(raw.left_grip_touch)),
                        ("right", steam_yes_no(raw.right_grip_touch)),
                    ],
                ))
                .size(9.5)
                .color(TEXT_DIM()),
            );
            ui.label(
                egui::RichText::new(lapinux_core::i18n::t_args(
                    "controllers.steam.pads_sticks_touched",
                    &[
                        ("left", steam_yes_no(raw.left_stick_touch)),
                        ("right", steam_yes_no(raw.right_stick_touch)),
                    ],
                ))
                .size(9.5)
                .color(TEXT_DIM()),
            );
            ui.label(
                egui::RichText::new(lapinux_core::i18n::t_args(
                    "controllers.steam.pads_contact_bits",
                    &[("bits", &format!("{:08x}", raw.buttons))],
                ))
                .monospace()
                .size(9.0)
                .color(with_alpha(TEXT_DIM(), 0.85)),
            );
            if !raw.left_grip_touch && !raw.right_grip_touch {
                muted(
                    ui,
                    lapinux_core::i18n::t("controllers.steam.pads_grip_muted"),
                );
            }
            if !raw.left_stick_touch && !raw.right_stick_touch {
                muted(
                    ui,
                    lapinux_core::i18n::t("controllers.steam.pads_sticks_muted"),
                );
            }
        } else {
            muted(ui, lapinux_core::i18n::t("controllers.steam.pads_prompt"));
            muted(
                ui,
                lapinux_core::i18n::t("controllers.steam.pads_live_note"),
            );
            muted(
                ui,
                lapinux_core::i18n::t("controllers.steam.pads_reconnect_note"),
            );
        }
        if !pending
            && secondary_button(
                ui,
                lapinux_core::i18n::t("controllers.steam.pads_sample_button"),
            )
            .on_hover_text(lapinux_core::i18n::t("controllers.steam.pads_sample_hover"))
            .clicked()
        {
            let device = device.clone();
            let ctx = ctx.clone();
            let (tx, rx) = mpsc::channel();
            tests.steam_raw_rx = Some(rx);
            tests.steam_hold_until = Some(Instant::now() + Duration::from_secs(3));
            tests.steam_raw_note =
                Some(lapinux_core::i18n::t("controllers.steam.pads_sampling").to_string());
            std::thread::spawn(move || {
                let result =
                    lapinux_input::steam::sample(&device, 1_500).map_err(|error| error.to_string());
                let _ = tx.send(result);
                ctx.request_repaint();
            });
        }
        if pending {
            muted(
                ui,
                lapinux_core::i18n::t("controllers.steam.pads_sampling_short"),
            );
            ctx.request_repaint_after(Duration::from_millis(80));
        }
        if let Some(note) = &tests.steam_raw_note {
            muted(ui, note.clone());
        }
    });
}

fn steam_grip_haptics_test(ui: &mut egui::Ui, device: &DeviceInfo, tests: &mut PadTestState) {
    let ctx = ui.ctx().clone();
    let received = tests.steam_grip_rx.as_ref().map(|rx| rx.try_recv());
    match received {
        Some(Ok(Ok(()))) => {
            tests.steam_grip_note =
                Some(lapinux_core::i18n::t("controllers.steam.grip_done").to_string());
            tests.steam_grip_rx = None;
        }
        Some(Ok(Err(error))) => {
            tests.steam_grip_note = Some(lapinux_core::i18n::t_args(
                "controllers.steam.haptics_failed",
                &[("error", &error)],
            ));
            tests.steam_grip_rx = None;
        }
        Some(Err(mpsc::TryRecvError::Disconnected)) => {
            tests.steam_grip_note =
                Some(lapinux_core::i18n::t("controllers.steam.grip_interrupted").to_string());
            tests.steam_grip_rx = None;
        }
        Some(Err(mpsc::TryRecvError::Empty)) => {
            ctx.request_repaint_after(Duration::from_millis(80));
        }
        None => {}
    }
    let pending = tests.steam_grip_rx.is_some();

    ui.vertical_centered(|ui| {
        muted(ui, lapinux_core::i18n::t("controllers.steam.grip_prompt"));
        ui.horizontal_wrapped(|ui| {
            for (label, left, right) in [
                (
                    lapinux_core::i18n::t("controllers.steam.grip_left"),
                    true,
                    false,
                ),
                (
                    lapinux_core::i18n::t("controllers.steam.grip_right"),
                    false,
                    true,
                ),
                (
                    lapinux_core::i18n::t("controllers.steam.haptics_both"),
                    true,
                    true,
                ),
            ] {
                if !pending && secondary_button(ui, label).clicked() {
                    let device = device.clone();
                    let ctx = ctx.clone();
                    let (tx, rx) = mpsc::channel();
                    tests.steam_grip_rx = Some(rx);
                    tests.steam_hold_until = Some(Instant::now() + Duration::from_secs(2));
                    tests.steam_grip_note = Some(lapinux_core::i18n::t_args(
                        "controllers.steam.grip_sending",
                        &[("zone", label)],
                    ));
                    std::thread::spawn(move || {
                        let result = lapinux_input::steam::grip_haptic(&device, left, right, 240)
                            .map_err(|error| error.to_string());
                        let _ = tx.send(result);
                        ctx.request_repaint();
                    });
                }
            }
        });
        if pending {
            muted(ui, lapinux_core::i18n::t("controllers.steam.grip_running"));
            ctx.request_repaint_after(Duration::from_millis(80));
        }
        ui.add_space(3.0);
        if let Some(note) = &tests.steam_grip_note {
            muted(ui, note.clone());
        } else {
            muted(ui, lapinux_core::i18n::t("controllers.steam.grip_report"));
        }
        muted(
            ui,
            lapinux_core::i18n::t("controllers.steam.grip_reconnect"),
        );
    });
}

fn steam_pad_haptics_test(ui: &mut egui::Ui, device: &DeviceInfo, tests: &mut PadTestState) {
    ui.vertical_centered(|ui| {
        muted(
            ui,
            lapinux_core::i18n::t("controllers.steam.haptics_prompt"),
        );
        ui.horizontal_wrapped(|ui| {
            for (label, left, right) in [
                (
                    lapinux_core::i18n::t("controllers.steam.haptics_pad_left"),
                    true,
                    false,
                ),
                (
                    lapinux_core::i18n::t("controllers.steam.haptics_pad_right"),
                    false,
                    true,
                ),
                (
                    lapinux_core::i18n::t("controllers.steam.haptics_both"),
                    true,
                    true,
                ),
            ] {
                if secondary_button(ui, label).clicked() {
                    let duration = tests.steam_haptic_duration.round().clamp(40.0, 800.0) as u16;
                    tests.steam_haptic_note = Some(
                        match lapinux_input::steam::haptic(device, left, right, duration) {
                            Ok(()) => lapinux_core::i18n::t_args(
                                "controllers.steam.haptics_sent",
                                &[("duration", &duration.to_string()), ("zone", label)],
                            ),
                            Err(error) => lapinux_core::i18n::t_args(
                                "controllers.steam.haptics_failed",
                                &[("error", &error.to_string())],
                            ),
                        },
                    );
                }
            }
        });
        ui.add(
            egui::Slider::new(&mut tests.steam_haptic_duration, 40.0..=800.0)
                .suffix(" ms")
                .text(lapinux_core::i18n::t("controllers.steam.haptics_duration")),
        );
        ui.add_space(3.0);
        if let Some(note) = &tests.steam_haptic_note {
            muted(ui, note.clone());
        } else {
            muted(
                ui,
                lapinux_core::i18n::t("controllers.steam.haptics_whole_pad"),
            );
        }
        muted(ui, lapinux_core::i18n::t("controllers.steam.haptics_zones"));
        muted(
            ui,
            lapinux_core::i18n::t("controllers.steam.haptics_reconnect"),
        );
    });
}

fn touchpad_widget(ui: &mut egui::Ui, pad: &lapinux_input::PadState) {
    let Some(touchpad) = &pad.touchpad else {
        return;
    };
    let (area, _) = ui.allocate_exact_size(egui::vec2(200.0, 62.0), egui::Sense::hover());
    ui.painter().rect_filled(
        area,
        egui::Rounding::same(6.0),
        egui::Color32::from_rgb(32, 33, 36),
    );
    ui.painter().rect_stroke(
        area,
        egui::Rounding::same(6.0),
        egui::Stroke::new(1.0_f32, BORDER()),
    );
    for point in &touchpad.points {
        let position = area.min + egui::vec2(point.x * area.width(), point.y * area.height());
        ui.painter().circle_filled(position, 3.6, OK);
        ui.painter().circle_stroke(
            position,
            6.0,
            egui::Stroke::new(1.0_f32, with_alpha(OK, 0.32)),
        );
    }
    if touchpad.points.is_empty() {
        ui.painter().text(
            area.center(),
            egui::Align2::CENTER_CENTER,
            lapinux_core::i18n::t("controllers.steam.touch_hint"),
            egui::FontId::proportional(9.5),
            with_alpha(TEXT_DIM(), 0.7),
        );
    }
}

fn paint_face_button(
    painter: &egui::Painter,
    center: egui::Pos2,
    radius: f32,
    active: bool,
    brand: Option<Brand>,
    label: &str,
    index: usize,
) {
    let well = radius + 2.0;
    painter.circle_filled(
        center,
        well,
        if active {
            egui::Color32::from_rgb(48, 64, 54)
        } else {
            egui::Color32::from_rgb(58, 59, 64)
        },
    );
    painter.circle_stroke(
        center,
        well,
        egui::Stroke::new(
            1.0_f32,
            if active {
                with_alpha(OK, 0.6)
            } else {
                with_alpha(BORDER(), 0.9)
            },
        ),
    );
    let ink = if active {
        egui::Color32::from_rgb(198, 232, 208)
    } else {
        egui::Color32::from_rgb(212, 214, 218)
    };
    if brand == Some(Brand::PlayStation) {
        paint_face_symbol(painter, center, index, ink);
    } else {
        painter.text(
            center,
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(11.5),
            ink,
        );
    }
}

fn paint_system_button(painter: &egui::Painter, rect: egui::Rect, label: &str, active: bool) {
    painter.rect_filled(
        rect,
        egui::Rounding::same(4.0),
        if active {
            egui::Color32::from_rgb(46, 60, 51)
        } else {
            BACKGROUND()
        },
    );
    painter.rect_stroke(
        rect,
        egui::Rounding::same(4.0),
        egui::Stroke::new(
            1.0_f32,
            if active {
                with_alpha(OK, 0.55)
            } else {
                with_alpha(BORDER(), 0.9)
            },
        ),
    );
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(8.5),
        if active { OK } else { TEXT_DIM() },
    );
}

fn paint_shoulder(painter: &egui::Painter, rect: egui::Rect, label: &str, active: bool) {
    painter.rect_filled(
        rect,
        egui::Rounding::same(4.0),
        if active {
            egui::Color32::from_rgb(46, 60, 51)
        } else {
            BACKGROUND()
        },
    );
    painter.rect_stroke(
        rect,
        egui::Rounding::same(4.0),
        egui::Stroke::new(
            1.0_f32,
            if active {
                with_alpha(OK, 0.55)
            } else {
                with_alpha(BORDER(), 0.9)
            },
        ),
    );
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(9.0),
        if active { OK } else { TEXT_DIM() },
    );
}

fn paint_trigger(painter: &egui::Painter, rect: egui::Rect, value: f32, label: &str) {
    let value = value.clamp(0.0, 1.0);
    let bar = egui::Rect::from_center_size(rect.center(), egui::vec2(rect.width(), 8.0));
    painter.rect_filled(bar, egui::Rounding::same(3.0), BACKGROUND());
    if value > 0.01 {
        let filled = egui::Rect::from_min_size(
            bar.min + egui::vec2(1.0, 1.0),
            egui::vec2((bar.width() - 2.0) * value, bar.height() - 2.0),
        );
        painter.rect_filled(
            filled,
            egui::Rounding::same(2.0),
            if value > 0.05 {
                with_alpha(OK, 0.85)
            } else {
                with_alpha(TEXT_DIM(), 0.35)
            },
        );
    }
    painter.rect_stroke(
        bar,
        egui::Rounding::same(3.0),
        egui::Stroke::new(1.0_f32, with_alpha(BORDER(), 0.9)),
    );
    painter.text(
        egui::pos2(rect.left() + 1.0, rect.top() - 7.0),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(9.5),
        TEXT_DIM(),
    );
    painter.text(
        egui::pos2(rect.right() - 1.0, rect.top() - 7.0),
        egui::Align2::RIGHT_CENTER,
        format!("{:.0} %", value * 100.0),
        egui::FontId::proportional(9.5),
        if value > 0.05 { OK } else { TEXT_DIM() },
    );
}

fn paint_stick(painter: &egui::Painter, center: egui::Pos2, radius: f32, x: f32, y: f32) {
    let x = x.clamp(-1.0, 1.0);
    let y = y.clamp(-1.0, 1.0);
    // Un léger drift ne doit pas allumer le stick : le seuil reprend la zone
    // morte par défaut du profil, sans masquer la position réelle du capuchon.
    let active = x.abs().max(y.abs()) > 0.18;
    let well = radius + 4.0;

    painter.circle_filled(center, well, BACKGROUND());
    painter.circle_stroke(center, well, egui::Stroke::new(1.0_f32, BORDER()));
    // Quatre repères discrets, pour situer le débattement sans dessiner une croix.
    for (dx, dy) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
        painter.line_segment(
            [
                center + egui::vec2(dx * (well - 4.0), dy * (well - 4.0)),
                center + egui::vec2(dx * (well - 1.5), dy * (well - 1.5)),
            ],
            egui::Stroke::new(1.0_f32, with_alpha(BORDER(), 0.8)),
        );
    }

    let travel = radius - 8.5;
    let cap_center = center + egui::vec2(x * travel, y * travel);
    let cap_radius = radius * 0.64;
    painter.circle_filled(
        cap_center,
        cap_radius,
        if active {
            egui::Color32::from_rgb(50, 68, 56)
        } else {
            egui::Color32::from_rgb(67, 68, 73)
        },
    );
    painter.circle_stroke(
        cap_center,
        cap_radius,
        egui::Stroke::new(
            1.0_f32,
            if active {
                with_alpha(OK, 0.6)
            } else {
                with_alpha(egui::Color32::WHITE, 0.12)
            },
        ),
    );
    // Léger creux central : le capuchon se lit comme une pièce, pas comme un trou.
    painter.circle_stroke(
        cap_center,
        cap_radius * 0.52,
        egui::Stroke::new(1.0_f32, with_alpha(egui::Color32::BLACK, 0.28)),
    );
}

fn paint_dpad(painter: &egui::Painter, center: egui::Pos2, arm: f32, thick: f32, x: f32, y: f32) {
    let well = arm + thick / 2.0 + 4.0;
    painter.circle_filled(center, well, BACKGROUND());
    painter.circle_stroke(center, well, egui::Stroke::new(1.0_f32, BORDER()));

    let half = thick / 2.0;
    let base = egui::Color32::from_rgb(58, 59, 64);
    painter.rect_filled(
        egui::Rect::from_center_size(center, egui::vec2(thick, arm * 2.0 + thick)),
        egui::Rounding::same(2.5),
        base,
    );
    painter.rect_filled(
        egui::Rect::from_center_size(center, egui::vec2(arm * 2.0 + thick, thick)),
        egui::Rounding::same(2.5),
        base,
    );
    // Seule la branche pressée s'éclaire : vert franc, lisible même sur le
    // fond sombre de la carte.
    for (offset, size, active) in [
        (
            egui::vec2(0.0, -(arm / 2.0 + half)),
            egui::vec2(thick, arm),
            y < -0.5,
        ),
        (
            egui::vec2(0.0, arm / 2.0 + half),
            egui::vec2(thick, arm),
            y > 0.5,
        ),
        (
            egui::vec2(-(arm / 2.0 + half), 0.0),
            egui::vec2(arm, thick),
            x < -0.5,
        ),
        (
            egui::vec2(arm / 2.0 + half, 0.0),
            egui::vec2(arm, thick),
            x > 0.5,
        ),
    ] {
        if active {
            let lit = egui::Rect::from_center_size(center + offset, size - egui::vec2(3.0, 3.0));
            painter.rect_filled(lit, egui::Rounding::same(2.0), with_alpha(OK, 0.62));
            painter.rect_stroke(
                lit,
                egui::Rounding::same(2.0),
                egui::Stroke::new(1.0_f32, with_alpha(OK, 0.9)),
            );
        }
    }
    let p = |x: f32, y: f32| center + egui::vec2(x, y);
    let outline = vec![
        p(-half, -arm - half),
        p(half, -arm - half),
        p(half, -half),
        p(arm + half, -half),
        p(arm + half, half),
        p(half, half),
        p(half, arm + half),
        p(-half, arm + half),
        p(-half, half),
        p(-arm - half, half),
        p(-arm - half, -half),
        p(-half, -half),
    ];
    painter.add(egui::Shape::closed_line(
        outline,
        egui::Stroke::new(1.0_f32, BORDER()),
    ));
}

/// Nord, est, sud, ouest. Nintendo garde les lettres de ses boutons, qui ne
/// suivent pas la disposition Xbox ; les autres manettes utilisent A/B/X/Y.
/// Comment le pilote nomme les quatre boutons de façade.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum FaceConvention {
    /// Les codes suivent la position physique : nord = haut. C'est le cas des
    /// pilotes Nintendo et Sony, qui nomment le bouton du haut.
    Position,
    /// Les codes suivent les lettres Xbox : `BTN_NORTH` porte le bouton X,
    /// qui se trouve à gauche sur la manette. xpad et le pont Steam
    /// Controller suivent cette convention.
    Letters,
}

/// Quels cercles du losange s'allument, dans l'ordre haut, droite, bas,
/// gauche. Les codes du noyau ne désignent pas tous la même position : sur
/// xpad, BTN_NORTH porte le bouton *X*, dessiné à gauche ; il doit donc
/// allumer le cercle de gauche, pas celui du haut.
fn face_active(
    pad: &lapinux_input::PadState,
    brand: Option<Brand>,
    driver: Option<&str>,
) -> [bool; 4] {
    let (north, west) = match face_convention(brand, driver) {
        FaceConvention::Letters => (pad.face.west, pad.face.north),
        FaceConvention::Position => (pad.face.north, pad.face.west),
    };
    [north, pad.face.east, pad.face.south, west]
}

fn face_convention(brand: Option<Brand>, driver: Option<&str>) -> FaceConvention {
    match brand {
        Some(Brand::PlayStation) | Some(Brand::Nintendo) => FaceConvention::Position,
        // Une manette sans marque connue suit son pilote : xpad annonce les
        // lettres, un pilote HID brut annonce les positions.
        Some(Brand::Xbox) | Some(Brand::Steam) => FaceConvention::Letters,
        Some(Brand::Generic) | None => {
            if driver.map_or(false, |driver| driver.contains("xpad")) {
                FaceConvention::Letters
            } else {
                FaceConvention::Position
            }
        }
    }
}

/// Pilote noyau lié à un nœud evdev, mémorisé quelques secondes : la fiche se
/// redessine trente fois par seconde et ce lien sysfs ne bouge pas.
fn pad_driver(path: &std::path::Path) -> Option<String> {
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, (Option<String>, Instant)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(guard) = cache.lock() {
        if let Some((driver, at)) = guard.get(path) {
            if at.elapsed() < Duration::from_secs(15) {
                return driver.clone();
            }
        }
    }
    let driver = read_pad_driver(path);
    if let Ok(mut guard) = cache.lock() {
        guard.insert(path.to_path_buf(), (driver.clone(), Instant::now()));
    }
    driver
}

fn read_pad_driver(path: &std::path::Path) -> Option<String> {
    let name = path.file_name()?;
    let mut dir = std::fs::canonicalize(
        std::path::Path::new("/sys/class/input")
            .join(name)
            .join("device"),
    )
    .ok()?;
    // `driver` vit sur l'interface parente (xpad, hid-generic, playstation…).
    for _ in 0..3 {
        if let Ok(target) = std::fs::read_link(dir.join("driver")) {
            return target
                .file_name()
                .map(|name| name.to_string_lossy().to_string());
        }
        dir = dir.parent()?.to_path_buf();
    }
    None
}

fn face_labels(profile: FaceButtonProfile) -> [&'static str; 4] {
    match profile {
        FaceButtonProfile::Xbox => ["Y", "B", "A", "X"],
        FaceButtonProfile::Nintendo => ["X", "A", "B", "Y"],
    }
}

fn face_profile_label(profile: FaceButtonProfile) -> &'static str {
    match profile {
        FaceButtonProfile::Xbox => "Xbox",
        FaceButtonProfile::Nintendo => "Nintendo",
    }
}

fn face_profile_lines(profile: FaceButtonProfile) -> [&'static str; 2] {
    match profile {
        FaceButtonProfile::Xbox => ["A en bas · B à droite", "X à gauche · Y en haut"],
        FaceButtonProfile::Nintendo => ["A à droite · B en bas", "X en haut · Y à gauche"],
    }
}

/// Carte de choix du profil de façade, avec son aperçu de losange. Le profil
/// PlayStation garde ses symboles : ces cartes ne changent que les lettres.
fn face_profile_card(
    ui: &mut egui::Ui,
    profile: FaceButtonProfile,
    selected: bool,
    width: f32,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 216.0), egui::Sense::click());
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    let painter = ui.painter();
    let fill = if selected {
        CARD_ACTIVE()
    } else if response.hovered() {
        CARD_HOVER()
    } else {
        PANEL()
    };
    let stroke = if selected {
        egui::Stroke::new(1.4_f32, ACCENT())
    } else {
        egui::Stroke::new(1.0_f32, BORDER())
    };
    painter.rect(rect, egui::Rounding::same(10.0), fill, stroke);
    painter.text(
        rect.left_top() + egui::vec2(16.0, 14.0),
        egui::Align2::LEFT_TOP,
        face_profile_label(profile),
        egui::FontId::proportional(14.0),
        TEXT(),
    );
    if profile == FaceButtonProfile::Xbox {
        painter.text(
            rect.left_top() + egui::vec2(16.0, 34.0),
            egui::Align2::LEFT_TOP,
            "PROFIL PAR DÉFAUT",
            egui::FontId::proportional(8.5),
            TEXT_DIM(),
        );
    }
    for (index, line) in face_profile_lines(profile).iter().enumerate() {
        painter.text(
            rect.left_top() + egui::vec2(16.0, 52.0 + index as f32 * 15.0),
            egui::Align2::LEFT_TOP,
            *line,
            egui::FontId::proportional(9.5),
            TEXT_DIM(),
        );
    }
    // Même losange que le test en direct, mais sans appui : il montre la
    // disposition des lettres au lieu d'un état de manette.
    let center = egui::pos2(rect.center().x, rect.top() + 139.0);
    let labels = face_labels(profile);
    for (index, (dx, dy)) in [(0.0, -23.0), (23.0, 0.0), (0.0, 23.0), (-23.0, 0.0)]
        .iter()
        .enumerate()
    {
        paint_face_button(
            painter,
            center + egui::vec2(*dx, *dy),
            12.5,
            false,
            None,
            labels[index],
            index,
        );
    }
    let dot = egui::pos2(rect.right() - 17.0, rect.top() + 18.0);
    painter.circle_stroke(
        dot,
        6.0,
        egui::Stroke::new(1.2_f32, if selected { OK } else { BORDER() }),
    );
    if selected {
        painter.circle_filled(dot, 3.2, OK);
    }
    response
}

/// Symboles PlayStation : triangle au nord, rond à l'est, croix au sud et
/// carré à l'ouest. Dessinés au trait pour rester nets à toute échelle.
fn paint_face_symbol(
    painter: &egui::Painter,
    center: egui::Pos2,
    side: usize,
    color: egui::Color32,
) {
    let stroke = egui::Stroke::new(2.2_f32, color);
    let radius = 8.6;
    match side {
        0 => {
            painter.add(egui::Shape::closed_line(
                vec![
                    center + egui::vec2(0.0, -radius),
                    center + egui::vec2(radius, radius * 0.8),
                    center + egui::vec2(-radius, radius * 0.8),
                ],
                stroke,
            ));
        }
        1 => {
            painter.circle_stroke(center, radius, stroke);
        }
        2 => {
            let d = radius * 0.72;
            painter.line_segment(
                [center + egui::vec2(-d, -d), center + egui::vec2(d, d)],
                stroke,
            );
            painter.line_segment(
                [center + egui::vec2(d, -d), center + egui::vec2(-d, d)],
                stroke,
            );
        }
        _ => {
            painter.rect_stroke(
                egui::Rect::from_center_size(center, egui::vec2(radius * 1.65, radius * 1.65)),
                egui::Rounding::same(1.5),
                stroke,
            );
        }
    }
}

/// Plage affichée du gyroscope, en °/s. Les rapports DualSense/Switch
/// rapportent ±2048 °/s sur une valeur normalisée à ±1.
const GYRO_FULL_SCALE: f32 = 2048.0;
/// Plage affichée de l'accéléromètre, en g (±4 g sur une valeur à ±1).
const ACCEL_FULL_SCALE: f32 = 4.0;
/// Demi-barre du gyroscope : un geste normal (50 à 150 °/s) reste lisible.
const GYRO_DISPLAY_RANGE: f32 = 500.0;
fn gyro_degrees(value: f32) -> f32 {
    value.clamp(-1.0, 1.0) * GYRO_FULL_SCALE
}

fn accel_in_g(value: f32) -> f32 {
    value.clamp(-1.0, 1.0) * ACCEL_FULL_SCALE
}

fn motion_axis(ui: &mut egui::Ui, label: &str, value: f32, unit: &str, decimals: usize) {
    let physical = if unit == "°/s" {
        gyro_degrees(value)
    } else {
        accel_in_g(value)
    };
    let range = if unit == "°/s" {
        GYRO_DISPLAY_RANGE
    } else {
        ACCEL_FULL_SCALE
    };
    let ratio = (physical / range).clamp(-1.0, 1.0);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).size(10.5).color(TEXT_DIM()));
        let (rect, _) = ui.allocate_exact_size(egui::vec2(124.0, 12.0), egui::Sense::hover());
        let painter = ui.painter();
        painter.rect_filled(rect, egui::Rounding::same(5.0), BACKGROUND());
        let center = rect.center();
        // Graduations : quart de plage de chaque côté.
        for step in 1..4 {
            let offset = rect.width() / 2.0 * step as f32 / 4.0;
            for x in [center.x - offset, center.x + offset] {
                painter.line_segment(
                    [
                        egui::pos2(x, rect.top() + 3.0),
                        egui::pos2(x, rect.bottom() - 3.0),
                    ],
                    egui::Stroke::new(1.0_f32, with_alpha(BORDER(), 0.5)),
                );
            }
        }
        painter.line_segment(
            [
                egui::pos2(center.x, rect.top() + 1.0),
                egui::pos2(center.x, rect.bottom() - 1.0),
            ],
            egui::Stroke::new(1.0_f32, with_alpha(BORDER(), 0.95)),
        );
        // Remplissage depuis le centre jusqu'à la valeur.
        let dot_x = center.x + ratio * (rect.width() / 2.0 - 2.0);
        let fill = egui::Rect::from_min_max(
            egui::pos2(center.x.min(dot_x), rect.top() + 3.0),
            egui::pos2(center.x.max(dot_x), rect.bottom() - 3.0),
        );
        painter.rect_filled(
            fill,
            egui::Rounding::same(2.0),
            if physical.abs() > range * 0.02 {
                OK
            } else {
                with_alpha(TEXT_DIM(), 0.35)
            },
        );
        painter.rect_stroke(
            rect,
            egui::Rounding::same(5.0),
            egui::Stroke::new(1.0_f32, BORDER()),
        );
        // Évite le « -0 °/s » d'un capteur au repos : la valeur affichée
        // s'arrondit à zéro dès qu'elle est plus petite qu'un demi-pas.
        let shown = if physical.abs() < 0.5 * 10f32.powi(-(decimals as i32)) {
            0.0
        } else {
            physical
        };
        ui.label(
            egui::RichText::new(format!("{shown:+.decimals$} {unit}").replace('.', ","))
                .size(10.5)
                .monospace()
                .color(if physical.abs() > range * 0.02 {
                    TEXT()
                } else {
                    TEXT_DIM()
                }),
        );
    });
}

/// Petites touches vivantes dans la liste : les boutons enfoncés, dans les
/// libellés de la marque, sans déborder sur la batterie ni sur la connexion.
fn paint_pressed_preview(
    painter: &egui::Painter,
    pad: &lapinux_input::PadState,
    brand: Option<Brand>,
    driver: Option<&str>,
    face_profile: FaceButtonProfile,
    left: f32,
    right: f32,
    center_y: f32,
) {
    let available = right - left;
    if available < 24.0 {
        return;
    }
    let font = egui::FontId::proportional(9.0);
    let labels: Vec<String> = row_preview_labels(pad, brand, driver, face_profile);
    if labels.is_empty() {
        return;
    }
    let mut x = left;
    let mut shown = 0usize;
    for (index, label) in labels.iter().enumerate() {
        if shown >= 6 {
            break;
        }
        let text = elide_to_width(painter, label, font.clone(), 54.0);
        let width = (painter
            .layout_no_wrap(text.clone(), font.clone(), TEXT())
            .size()
            .x
            + 10.0)
            .max(20.0);
        let remaining = labels.len() - index;
        let reserve = if remaining > 1 { 24.0 } else { 0.0 };
        if x + width + reserve > right {
            break;
        }
        let rect =
            egui::Rect::from_min_size(egui::pos2(x, center_y - 7.5), egui::vec2(width, 15.0));
        painter.rect_filled(rect, egui::Rounding::same(4.0), with_alpha(ACCENT(), 0.18));
        painter.rect_stroke(
            rect,
            egui::Rounding::same(4.0),
            egui::Stroke::new(1.0_f32, with_alpha(ACCENT(), 0.45)),
        );
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            text,
            font.clone(),
            TEXT(),
        );
        x += width + 4.0;
        shown += 1;
    }
    let hidden = labels.len().saturating_sub(shown);
    if hidden > 0 && x + 18.0 <= right {
        painter.text(
            egui::pos2(x + 2.0, center_y),
            egui::Align2::LEFT_CENTER,
            format!("+{hidden}"),
            font,
            with_alpha(TEXT_DIM(), 0.9),
        );
    }
}

/// Étiquettes de l'aperçu d'une ligne de la liste : les mouvements de sticks
/// passent devant, pour ne pas être masqués par les boutons déjà maintenus
/// dans la limite des six touches affichées.
fn row_preview_labels(
    pad: &lapinux_input::PadState,
    brand: Option<Brand>,
    driver: Option<&str>,
    face_profile: FaceButtonProfile,
) -> Vec<String> {
    analog_preview_labels(pad)
        .into_iter()
        .chain(pad.pressed.iter().cloned())
        .map(|label| face_button_text(&label, brand, driver, face_profile))
        .collect()
}

/// Libellé d'un bouton de façade dans l'aperçu : le profil choisi dans
/// Manettes remplace les lettres à leur position physique, sauf pour
/// PlayStation dont les symboles ne sont pas des lettres. Les autres boutons
/// suivent la marque.
fn face_button_text(
    label: &str,
    brand: Option<Brand>,
    driver: Option<&str>,
    profile: FaceButtonProfile,
) -> String {
    if brand == Some(Brand::PlayStation) {
        return brand_button_text(label, brand);
    }
    let labels = face_labels(profile);
    // Même convention que le losange : xpad nomme les boutons par leur
    // lettre Xbox, les pilotes Nintendo et HID brut par leur position.
    let position = match face_convention(brand, driver) {
        FaceConvention::Letters => match label {
            "A" => Some(2),
            "B" => Some(1),
            "X" => Some(3),
            "Y" => Some(0),
            _ => None,
        },
        FaceConvention::Position => match label {
            "A" => Some(2),
            "B" => Some(1),
            "X" => Some(0),
            "Y" => Some(3),
            _ => None,
        },
    };
    match position {
        Some(index) => labels[index].to_string(),
        None => brand_button_text(label, brand),
    }
}

/// État des sticks et du pavé tactile dans la liste, en plus des boutons.
/// Un stick au repos ne produit rien ; dès qu'il dépasse la zone morte, une
/// touche compacte indique sa direction. Un doigt sur le pavé ajoute « Pavé ».
fn analog_preview_labels(pad: &lapinux_input::PadState) -> Vec<String> {
    // Assez sensible pour voir un mouvement ordinaire, assez haut pour ne
    // pas afficher en permanence le bruit de repos d'un stick usé.
    const DEADZONE: f32 = 0.20;
    let mut labels = Vec::new();
    for (prefix, x_key, y_key) in [("G", "left_x", "left_y"), ("D", "right_x", "right_y")] {
        let x = pad.axis(x_key);
        let y = pad.axis(y_key);
        if x.abs() < DEADZONE && y.abs() < DEADZONE {
            continue;
        }
        let arrow = match (x > DEADZONE, x < -DEADZONE, y > DEADZONE, y < -DEADZONE) {
            (true, _, true, _) => "SE",
            (true, _, _, true) => "NE",
            (_, true, true, _) => "SO",
            (_, true, _, true) => "NO",
            (true, _, _, _) => "E",
            (_, true, _, _) => "O",
            (_, _, true, _) => "S",
            _ => "N",
        };
        labels.push(format!("{prefix} {arrow}"));
    }
    if pad
        .touchpad
        .as_ref()
        .map_or(false, |touchpad| !touchpad.points.is_empty())
    {
        labels.push("Pavé".to_string());
    }
    labels.extend(analog_trigger_labels(pad));
    labels
}

/// Les gâchettes analogiques ne sont pas des boutons evdev : elles n'arrivent
/// que par leurs axes. Sans ce relevé, une pression partielle n'apparaissait
/// jamais dans la liste. Le seuil évite d'afficher une gâchette au repos.
fn analog_trigger_labels(pad: &lapinux_input::PadState) -> Vec<String> {
    const PRESSED: f32 = 0.08;
    let mut labels = Vec::new();
    for (axis, label) in [("left_trigger", "LT"), ("right_trigger", "RT")] {
        if pad.axis(axis) > PRESSED && !pad.pressed.iter().any(|pressed| pressed == label) {
            labels.push(label.to_string());
        }
    }
    labels
}

/// Traduit les libellés positionnels d'evdev (A/B/X/Y à la Xbox) vers les
/// touches imprimées sur la manette. Les boutons hors façade gardent leur nom.
fn brand_button_text(label: &str, brand: Option<Brand>) -> String {
    let mapped = match brand {
        Some(Brand::PlayStation) => match label {
            "A" => "Croix",
            "B" => "Rond",
            "X" => "Triangle",
            "Y" => "Carré",
            "LB" => "L1",
            "RB" => "R1",
            "LT" => "L2",
            "RT" => "R2",
            "Select" => "Créer",
            "Start" => "Options",
            "Guide" => "PS",
            other => other,
        },
        Some(Brand::Nintendo) => match label {
            "A" => "B",
            "B" => "A",
            "LB" => "L",
            "RB" => "R",
            "LT" => "ZL",
            "RT" => "ZR",
            "Select" => "Moins",
            "Start" => "Plus",
            "Guide" => "Home",
            other => other,
        },
        Some(Brand::Xbox) => match label {
            "Select" => "View",
            "Start" => "Menu",
            other => other,
        },
        Some(Brand::Steam) => match label {
            "Select" => "View",
            "Start" => "Menu",
            "Guide" => "Steam",
            other => other,
        },
        // Un pad générique suit le profil Xbox : mêmes libellés de façade et
        // de boutons système, seul le logo change.
        Some(Brand::Generic) => match label {
            "Select" => "View",
            "Start" => "Menu",
            other => other,
        },
        _ => label,
    };
    mapped.to_string()
}

/// Nom de fichier sans séparateur de chemin.
/// Libellé de champ des réglages d'un jeu : petit et discret, au-dessus du
/// contrôle qu'il nomme.
fn field_label(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).size(11.0).color(TEXT_DIM()));
}

/// Rendu du journal : date et heure en gris, niveau coloré, puis le message.
/// Les sorties brutes (jeu, bandeaux) restent grises, sans mise en forme.
fn log_layout(lines: &[String]) -> egui::text::LayoutJob {
    let font = egui::FontId::monospace(10.5);
    let ink = |color: egui::Color32| egui::text::TextFormat {
        font_id: font.clone(),
        color,
        ..Default::default()
    };
    let mut job = egui::text::LayoutJob::default();
    for line in lines {
        if let Some((date, time, level, message)) = log_fields(line) {
            job.append(&format!("{date} {time} "), 0.0, ink(TEXT_DIM()));
            let color = match level {
                "ERROR" => DANGER,
                "WARN" => WARN,
                "DEBUG" | "TRACE" => with_alpha(TEXT_DIM(), 0.75),
                _ => TEXT_DIM(),
            };
            job.append(&format!("{level:<5} "), 0.0, ink(color));
            job.append(message, 0.0, ink(TEXT()));
        } else if line.starts_with("===") {
            // En-tête de session ou d'overlay : il sépare les entrées.
            job.append(line, 0.0, ink(ACCENT()));
        } else {
            job.append(line, 0.0, ink(TEXT_DIM()));
        }
        job.append("\n", 0.0, ink(TEXT()));
    }
    job
}

/// Découpe `2026-11-04T09:43:52.002460Z INFO message` en date courte, heure,
/// niveau et message. Renvoie `None` pour une ligne brute.
fn log_fields(line: &str) -> Option<(String, &str, &str, &str)> {
    let bytes = line.as_bytes();
    if line.len() < 21 || bytes.get(4) != Some(&b'-') || bytes.get(10) != Some(&b'T') {
        return None;
    }
    let (date, rest) = line.split_at(10);
    let (time, rest) = rest.get(1..)?.split_once(' ')?;
    let time = time.split('.').next().unwrap_or(time);
    let rest = rest.trim_start();
    let (level, message) = rest.split_once(' ').unwrap_or((rest, ""));
    if !matches!(level, "TRACE" | "DEBUG" | "INFO" | "WARN" | "ERROR") {
        return None;
    }
    Some((
        format!("{}/{}", &date[8..10], &date[5..7]),
        time,
        level,
        message.trim_start(),
    ))
}

fn muted(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.label(
        egui::RichText::new(text.into())
            .size(11.0)
            .color(TEXT_DIM()),
    );
}

/// Profil d'encodage lisible dans les réglages.
fn profile_label(profile: &str) -> &'static str {
    match profile {
        "performance" => "Performance · jeu fluide",
        "quality" => lapinux_core::i18n::t("settings.captures.quality"),
        _ => lapinux_core::i18n::t("settings.captures.balanced"),
    }
}

/// Résolution maximale de la vidéo.
fn resolution_label(resolution: &str) -> &'static str {
    match resolution {
        "720p" => lapinux_core::i18n::t("settings.captures.720p"),
        "1440p" => "1440p · 2K max",
        "native" => "Native · taille du jeu",
        _ => lapinux_core::i18n::t("settings.captures.1080p"),
    }
}

/// Encodeur vidéo préféré.
fn encoder_label(encoder: &str) -> &'static str {
    match encoder {
        "nvenc" => "NVENC · carte graphique",
        "software" => "Logiciel · processeur",
        _ => "Automatique · GPU si possible",
    }
}

/// Codec vidéo lisible dans la liste des réglages.
fn codec_label(codec: &str) -> &'static str {
    match codec {
        "h264" => "H.264",
        "hevc" => "H.265 · HEVC",
        "av1" => "AV1",
        _ => "Automatique",
    }
}

/// La capture est-elle possible ? `ffmpeg` encode, KWin fournit l'image :
/// aucun outil de capture externe n'est requis.
fn recorder_available() -> bool {
    lapinux_core::paths::which("ffmpeg").is_some()
}

/// Le fichier d'autorisation KWin est-il déjà écrit ? Aucune commande
/// lancée : seul le disque est regardé.
fn permission_granted() -> bool {
    let path =
        lapinux_core::paths::home_dir().join(".local/share/applications/lapinux-capture.desktop");
    std::fs::read_to_string(path)
        .map(|text| text.contains("org.kde.KWin.ScreenShot2"))
        .unwrap_or(false)
}

/// Interrupteur à glissière : la piste s'allume quand le réglage est actif et
/// le bouton glisse à droite. Un clic bascule, comme une case à cocher — mais
/// il se lit d'un coup d'œil dans une colonne de réglages.
fn switch(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let size = egui::vec2(42.0, 23.0);
    let (rect, mut response) = ui.allocate_exact_size(size, egui::Sense::click());
    let enabled = ui.is_enabled();
    if enabled && response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let response = response.on_hover_cursor(if enabled {
        egui::CursorIcon::PointingHand
    } else {
        egui::CursorIcon::Default
    });
    let progress = ui.ctx().animate_bool(response.id, *on);
    let painter = ui.painter();
    let track = if !enabled {
        with_alpha(CARD_ACTIVE(), 0.55)
    } else if *on {
        ACCENT()
    } else if response.hovered() {
        CARD_ACTIVE().gamma_multiply(1.25)
    } else {
        egui::Color32::from_rgb(58, 62, 70)
    };
    painter.rect_filled(rect, egui::Rounding::same(rect.height() / 2.0), track);
    painter.rect_stroke(
        rect,
        egui::Rounding::same(rect.height() / 2.0),
        egui::Stroke::new(1.0_f32, with_alpha(BORDER(), 0.75)),
    );
    let knob = rect.height() / 2.0 - 2.5;
    let travel = rect.width() - 2.0 * (knob + 2.5);
    let centre = egui::pos2(
        rect.left() + 2.5 + knob + travel * progress,
        rect.center().y,
    );
    painter.circle_filled(
        centre,
        knob,
        if enabled {
            egui::Color32::from_rgb(244, 246, 250)
        } else {
            TEXT_DIM().gamma_multiply(0.55)
        },
    );
    response
}

/// Âge du jeton enregistré, en clair : minutes, heures puis jours. Un jeton
/// vit des semaines, « 268 h 12 » ne se lit pas.
fn token_age(seconds: i64) -> String {
    let seconds = seconds.max(0) as u64;
    if seconds < 60 {
        "enregistré à l'instant".to_string()
    } else if seconds < 86_400 {
        format!(
            "enregistré il y a {}",
            lapinux_core::util::format_duration(seconds)
        )
    } else {
        format!("enregistré il y a {} j", seconds / 86_400)
    }
}

/// Lien texte compact, utilisé par la carte de débit et la fiche de profil.
///
/// Le texte est peint dans son propre rectangle plutôt que confié à un
/// `Label` : placé par une mise en page alignée à droite, un libellé se peint
/// décalé de sa largeur, et le clic restait alors à côté du texte affiché.
fn link_text(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let galley =
        ui.painter()
            .layout_no_wrap(text.to_owned(), egui::FontId::proportional(10.5), ACCENT());
    let (rect, response) = ui.allocate_exact_size(galley.size(), egui::Sense::click());
    ui.painter().galley(rect.min, galley, ACCENT());
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

/// Taille d'un texte court, mesurée sans repli : les lignes posées à la main
/// réservent leur place avant de peindre.
fn text_size(ui: &egui::Ui, text: &str, size: f32) -> egui::Vec2 {
    ui.painter()
        .layout_no_wrap(
            text.to_owned(),
            egui::FontId::proportional(size),
            egui::Color32::WHITE,
        )
        .size()
}

/// Texte peint à une position donnée, rendant le rectangle qu'il occupe : le
/// texte et sa zone de clic restent au même endroit, contrairement à un
/// `Label` dans une mise en page alignée à droite.
fn painted_text_at(
    ui: &egui::Ui,
    pos: egui::Pos2,
    text: &str,
    size: f32,
    color: egui::Color32,
) -> egui::Rect {
    let galley =
        ui.painter()
            .layout_no_wrap(text.to_owned(), egui::FontId::proportional(size), color);
    let rect = egui::Rect::from_min_size(pos, galley.size());
    ui.painter().galley(pos, galley, color);
    rect
}

/// Une ligne de mesure du test de débit : libellé, valeur et unité. La valeur
/// prend la couleur de succès et un indicateur tourne pendant sa mesure.
/// Ouvre une adresse web dans le navigateur du bureau.
pub(crate) fn open_url(url: &str) {
    let _ = std::process::Command::new("xdg-open")
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

fn open_path(path: &Path) {
    let _ = std::process::Command::new("xdg-open")
        .arg(path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// Champ de dossier avec un vrai sélecteur natif. Le champ reste éditable à
/// la main : le dialogue n'est qu'un raccourci, jamais une obligation.
fn capture_folder_editor(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut String,
    default: &Path,
    title: &str,
) {
    field_label(ui, label);
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(value)
                .desired_width(420.0)
                .hint_text(default.display().to_string()),
        );
        if ui.button("Parcourir…").clicked() {
            let current = lapinux_core::captures::resolve_dir(value, || default.to_path_buf());
            if let Some(path) = pick_folder(&current, title) {
                *value = path.to_string_lossy().to_string();
            }
        }
        if ui.button("Ouvrir").clicked() {
            let path = lapinux_core::captures::resolve_dir(value, || default.to_path_buf());
            open_path(&path);
        }
    });
    let path = lapinux_core::captures::resolve_dir(value, || default.to_path_buf());
    muted(ui, format!("Dossier utilisé : {}", path.display()));
}

fn application_value_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal_top(|ui| {
        ui.add_sized(
            [138.0, 18.0],
            egui::Label::new(egui::RichText::new(label).size(10.5).color(TEXT_DIM())),
        );
        ui.add(
            egui::Label::new(
                egui::RichText::new(value)
                    .monospace()
                    .size(10.0)
                    .color(TEXT()),
            )
            .wrap(),
        );
    });
    ui.add_space(3.0);
}

/// Paliers de limite de FPS proposés : l'illimité en tête (zéro), puis les
/// fréquences que les écrans et GameScope connaissent. Le dernier palier est
/// aussi le plus haut que la configuration accepte (`MAX_FPS_CAP`) : au-delà,
/// c'est « illimité ».
const FPS_CHOICES: [u32; 12] = [0, 30, 40, 60, 75, 90, 120, 144, 165, 180, 240, 360];

/// Texte d'une limite : « Illimité » pour zéro, la valeur suivie de son unité
/// sinon.
fn fps_text(fps: u32) -> String {
    if fps == 0 {
        lapinux_core::i18n::t("settings.general.fps_unlimited").to_string()
    } else {
        format!("{fps} FPS")
    }
}

/// Sélecteur de limite de FPS : une liste de paliers — dont l'illimité — et
/// une valeur libre pour les fréquences d'écran inhabituelles.
fn fps_picker(ui: &mut egui::Ui, id: impl std::hash::Hash, fps: &mut u32) {
    egui::ComboBox::from_id_salt(id)
        .width(160.0)
        .selected_text(fps_text(*fps))
        .show_ui(ui, |ui| {
            for choice in FPS_CHOICES {
                ui.selectable_value(fps, choice, fps_text(choice));
            }
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(lapinux_core::i18n::t("settings.general.fps_custom"));
                ui.add(egui::DragValue::new(fps).range(0..=1000).speed(1.0));
            });
        });
}

/// Sources de build Proton, dans l'ordre de préférence du daemon : dépôt
/// GitHub et nom court affiché.
const PROTON_SOURCES: [(&str, &str); 3] = [
    ("GloriousEggroll/proton-ge-custom", "GE-Proton"),
    ("CachyOS/proton-cachyos", "Proton-CachyOS"),
    ("dawn-winery/dwproton", "DW-Proton"),
];

/// Ce que chaque source apporte, en une ligne : la liste se lit comme un
/// catalogue, sans avoir à ouvrir un build.
const PROTON_BLURBS: [(&str, &str); 3] = [
    ("GloriousEggroll/proton-ge-custom", "protons.blurb_ge"),
    ("CachyOS/proton-cachyos", "protons.blurb_cachyos"),
    ("dawn-winery/dwproton", "protons.blurb_dw"),
];

/// Description d'une source ; un dépôt inconnu reste présentable.
fn source_blurb(repo: &str) -> &'static str {
    lapinux_core::i18n::t(
        PROTON_BLURBS
            .iter()
            .find(|(known, _)| *known == repo)
            .map(|(_, blurb)| *blurb)
            .unwrap_or("protons.blurb_fallback"),
    )
}

/// Une ligne de source, façon ProtonPlus : nom, ce qu'elle apporte, son état et
/// un chevron à droite. Toute la ligne déplie ses builds ; renvoie vrai quand
/// elle est cliquée.
fn proton_source_header(
    ui: &mut egui::Ui,
    label: &str,
    description: &str,
    tags: &[(&str, egui::Color32)],
    open: bool,
) -> bool {
    const HEIGHT: f32 = 64.0;
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, HEIGHT), egui::Sense::click());
    // Le fond change au survol : la ligne entière est la cible du clic.
    let fill = if response.hovered() {
        CARD_ACTIVE()
    } else {
        CARD()
    };
    let rounding = if open {
        egui::Rounding {
            nw: 10.0,
            ne: 10.0,
            sw: 0.0,
            se: 0.0,
        }
    } else {
        egui::Rounding::same(10.0)
    };
    ui.painter().rect_filled(rect, rounding, fill);
    let inner = rect.shrink2(egui::vec2(14.0, 10.0));
    let mut content = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    content.set_width(inner.width() - 20.0);
    content.horizontal(|ui| {
        ui.label(egui::RichText::new(label).size(13.0).strong().color(TEXT()));
        for (text, color) in tags {
            chip(ui, text, *color);
        }
    });
    content.add_space(2.0);
    content.add(
        egui::Label::new(
            egui::RichText::new(description)
                .size(11.0)
                .color(TEXT_DIM()),
        )
        .truncate(),
    );
    // Chevron peint à la main : la police n'a ni ▾ ni ▴, la flèche resterait
    // invisible.
    let caret = egui::pos2(rect.right() - 18.0, rect.center().y);
    let size = 4.2;
    let points = if open {
        vec![
            caret + egui::vec2(-size, size * 0.45),
            caret + egui::vec2(0.0, -size * 0.6),
            caret + egui::vec2(size, size * 0.45),
        ]
    } else {
        vec![
            caret + egui::vec2(-size, -size * 0.45),
            caret + egui::vec2(0.0, size * 0.6),
            caret + egui::vec2(size, -size * 0.45),
        ]
    };
    ui.painter().add(egui::Shape::line(
        points,
        egui::Stroke::new(1.4_f32, TEXT_DIM()),
    ));
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
}

/// Un trait fin qui sépare deux lignes d'une même liste.
fn hairline(ui: &mut egui::Ui) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter().hline(
        rect.x_range(),
        rect.center().y,
        egui::Stroke::new(1.0_f32, BORDER()),
    );
}

/// Nom du dossier qu'une archive Proton crée une fois installée : l'asset sans
/// son suffixe d'archive. C'est ce nom que la détection retrouve sur le disque.
fn release_folder(asset: &str) -> &str {
    for suffix in [".tar.gz", ".tar.xz", ".tar.zst", ".zip"] {
        if let Some(stem) = asset.strip_suffix(suffix) {
            return stem;
        }
    }
    asset
}

/// Famille d'un build : son nom avant son numéro de version, en minuscules
/// sans ponctuation — « GE-Proton11-7-x86_64 » donne « geproton ».
fn proton_family(name: &str) -> String {
    let cut = name
        .find(|c: char| c.is_ascii_digit())
        .unwrap_or(name.len());
    name[..cut]
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase()
}

/// Un Proton installé appartient-il à la famille du build publié ? Sert
/// seulement à dire « plus récent que l'installé » : la date reste la preuve.
fn same_proton_family(installed: &str, released: &str) -> bool {
    let family = proton_family(released);
    !family.is_empty() && proton_family(installed).starts_with(&family)
}

/// Le build le plus récent d'une source : les préversions passent après les
/// versions stables, puis la date de publication tranche.
fn latest_release<'a>(releases: &'a [ProtonRelease], repo: &str) -> Option<&'a ProtonRelease> {
    releases
        .iter()
        .filter(|release| release.repo == repo)
        .reduce(|best, release| {
            let newer = match (release.prerelease, best.prerelease) {
                (true, false) => false,
                (false, true) => true,
                _ => release.published > best.published,
            };
            if newer {
                release
            } else {
                best
            }
        })
}

/// Tous les builds publiés d'une source, le plus récent en tête : c'est la
/// liste qu'une ligne de la page Protons déplie. Une préversion passe derrière
/// les versions stables de la même date.
fn source_builds<'a>(releases: &'a [ProtonRelease], repo: &str) -> Vec<&'a ProtonRelease> {
    let mut builds: Vec<&ProtonRelease> = releases
        .iter()
        .filter(|release| release.repo == repo)
        .collect();
    builds.sort_by(|a, b| {
        a.prerelease
            .cmp(&b.prerelease)
            .then_with(|| b.published.cmp(&a.published))
    });
    builds
}

/// Date publiée d'un build, telle que GitHub la donne (`2026-10-01T12:00:00Z`),
/// écrite courtement : « 1 oct. 2026 ».
fn release_date(published: &str) -> String {
    const MONTHS: [&str; 12] = [
        "protons.month_1",
        "protons.month_2",
        "protons.month_3",
        "protons.month_4",
        "protons.month_5",
        "protons.month_6",
        "protons.month_7",
        "protons.month_8",
        "protons.month_9",
        "protons.month_10",
        "protons.month_11",
        "protons.month_12",
    ];
    let mut parts = published.split(['-', 'T']);
    let (Some(year), Some(month), Some(day)) = (parts.next(), parts.next(), parts.next()) else {
        return published.to_string();
    };
    let (Ok(month), Ok(day)) = (month.parse::<usize>(), day.parse::<u32>()) else {
        return published.to_string();
    };
    match MONTHS.get(month.saturating_sub(1)) {
        Some(name) => format!("{day} {} {year}", lapinux_core::i18n::t(name)),
        None => published.to_string(),
    }
}

/// Âge d'un contrôle en mots courts.
fn since_text(seconds: u64) -> String {
    match seconds {
        0..=45 => lapinux_core::i18n::t("protons.status_now").to_string(),
        46..=3_599 => lapinux_core::i18n::t_args(
            "protons.since_minutes",
            &[("count", &((seconds + 30) / 60).to_string())],
        ),
        3_600..=86_399 => lapinux_core::i18n::t_args(
            "protons.since_hours",
            &[("count", &(seconds / 3_600).to_string())],
        ),
        _ => lapinux_core::i18n::t_args(
            "protons.since_days",
            &[("count", &(seconds / 86_400).to_string())],
        ),
    }
}

/// Étiquette d'état : le texte sur un fond plein arrondi, sans contour. Un tag
/// se lit ; l'encadrer d'un trait ne faisait qu'ajouter du bruit.
fn chip(ui: &mut egui::Ui, text: &str, color: egui::Color32) -> egui::Response {
    // Une teinte discrète (texte secondaire) garde un fond sourd ; les teintes
    // franches passent en fond plein avec un texte sombre, comme le vert d'un
    // badge « installé » ou le blanc d'une source.
    let (fill, ink) = if color == TEXT_DIM() {
        (CARD_ACTIVE(), TEXT_DIM())
    } else {
        (color, BACKGROUND())
    };
    egui::Frame::none()
        .fill(fill)
        .rounding(egui::Rounding::same(5.0))
        .inner_margin(egui::Margin::symmetric(7.0, 2.0))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).size(10.0).strong().color(ink));
        })
        .response
}

/// Ton d'un bouton d'action : plein (vert), clair (blanc), discret ou rouge
/// sourd pour une suppression.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActionTone {
    /// L'action attendue : installer.
    Primary,
    /// Une vérification : lisible, sans crier plus fort que l'installation.
    Light,
    /// Une relecture locale, presque silencieuse.
    Quiet,
    /// Une suppression : rouge lisible, sans remplir la page.
    Danger,
}

/// Mélange deux teintes : `t` = 0 rend `a`, `t` = 1 rend `b`.
fn mix(a: egui::Color32, b: egui::Color32, t: f32) -> egui::Color32 {
    let t = t.clamp(0.0, 1.0);
    let lerp = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    egui::Color32::from_rgb(lerp(a.r(), b.r()), lerp(a.g(), b.g()), lerp(a.b(), b.b()))
}

/// Bouton de la page : fond plein, coins doux, hauteur constante, et un
/// éclaircissement au survol — les quatre tons se ressemblent assez pour former
/// une même famille.
///
/// Peint à la main plutôt qu'avec `egui::Button` : un `fill` explicite y fige
/// les couleurs, donc plus aucun retour au survol ni à l'appui.
fn action_button(ui: &mut egui::Ui, text: impl Into<String>, tone: ActionTone) -> egui::Response {
    action_button_sized(ui, text, tone, true, 27.0)
}

/// Le même bouton, avec un état désactivé : la taille ne change pas, le fond
/// reste lisible et le clic ne répond pas.
fn action_button_enabled(
    ui: &mut egui::Ui,
    text: impl Into<String>,
    tone: ActionTone,
    enabled: bool,
) -> egui::Response {
    action_button_sized(ui, text, tone, enabled, 27.0)
}

/// Variante à hauteur imposée, pour aligner un bouton sur les autres contrôles
/// d'une même barre sans changer la taille des boutons ordinaires.
fn action_button_sized(
    ui: &mut egui::Ui,
    text: impl Into<String>,
    tone: ActionTone,
    enabled: bool,
    height: f32,
) -> egui::Response {
    let (fill, ink) = if enabled {
        match tone {
            ActionTone::Primary => (OK, BACKGROUND()),
            ActionTone::Light => (ACCENT(), BACKGROUND()),
            ActionTone::Quiet => (CARD_ACTIVE(), TEXT()),
            ActionTone::Danger => (mix(CARD(), DANGER, 0.14), DANGER),
        }
    } else {
        (CARD_ACTIVE(), with_alpha(TEXT_DIM(), 0.45))
    };
    let galley = ui
        .painter()
        .layout_no_wrap(text.into(), egui::FontId::proportional(12.0), ink);
    let size = egui::vec2(galley.size().x + 28.0, height);
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    let background = if !enabled {
        fill
    } else if response.is_pointer_button_down_on() {
        mix(fill, BACKGROUND(), 0.22)
    } else if response.hovered() {
        mix(fill, egui::Color32::WHITE, 0.16)
    } else {
        fill
    };
    ui.painter()
        .rect_filled(rect, egui::Rounding::same(7.0), background);
    let text_pos = rect.center() - galley.size() / 2.0;
    ui.painter().galley(text_pos, galley, ink);
    if enabled && response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

/// Cible de la corbeille d'une carte d'Activité : une partie de
/// l'historique, ou un téléchargement terminé.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ActivityDelete {
    Session { appid: u32, started: i64 },
    Job { id: String },
}

impl ActivityDelete {
    fn tooltip(&self) -> &'static str {
        match self {
            Self::Session { .. } => "Supprimer cette partie",
            Self::Job { .. } => "Supprimer ce téléchargement",
        }
    }

    fn request(&self) -> ActivityRequest {
        match self {
            Self::Session { appid, started } => ActivityRequest::DeleteSession {
                appid: *appid,
                started: *started,
            },
            Self::Job { id } => ActivityRequest::DeleteJob { id: id.clone() },
        }
    }
}

/// Une carte de la page Activité : un jeu, ce qu'il en est, et son visuel.
struct ActivityCard {
    appid: u32,
    name: String,
    store: Option<lapinux_core::stores::Store>,
    remote: String,
    detail: String,
    tag: String,
    tone: egui::Color32,
    /// Progression d'un téléchargement en cours, de 0 à 1.
    progress: Option<f32>,
    /// Lancement et fermeture d'une partie ; la fin manque tant qu'elle
    /// tourne, et les cartes de téléchargement n'en ont pas.
    started: Option<i64>,
    ended: Option<i64>,
    /// Journal de la session sur le disque, déplié depuis la carte.
    log: String,
    /// Cadence des images relevée pendant la partie.
    fps: Option<lapinux_core::model::FpsStats>,
    /// Courbe de la cadence, du début de la partie à sa fin.
    fps_series: Vec<u32>,
    /// Premier et dernier instant de la courbe : le curseur y lit une heure.
    fps_series_start: i64,
    fps_series_end: i64,
    /// Entrée que la poubelle de la carte peut retirer : une partie ou un
    /// téléchargement terminé.
    delete: Option<ActivityDelete>,
}

/// Hauteur du graphique de cadence, partagée avec la légende posée à sa
/// droite pour que les deux se centrent l'un sur l'autre.
const FPS_CHART_HEIGHT: f32 = 62.0;

/// Hauteurs estimées d'une carte d'Activité. Elles servent uniquement aux
/// cartes hors écran : une carte mesurée remplace aussitôt son estimation.
const ACTIVITY_CARD_WITH_CHART: f32 = 106.0;
const ACTIVITY_CARD_PLAIN: f32 = 82.0;
const ACTIVITY_CARD_GAP: f32 = 8.0;

/// Ton d'un point de courbe : vert tant que la session tient sa cadence,
/// jaune quand elle fléchit, rouge sous le seuil jouable. La référence est le
/// maximum de la partie, avec un plancher absolu : une session à 60 images
/// stable reste verte même sur un écran à 165 Hz.
fn fps_tone(value: u32, highest: u32) -> egui::Color32 {
    // Le vert garde un plancher absolu : 60 et 120 images restent confortables
    // même sur une session dont le maximum observé monte à 165.
    if lapinux_core::util::fps_is_comfortable(value, highest) {
        OK
    } else if lapinux_core::util::fps_is_playable(value, highest) {
        WARN
    } else {
        DANGER
    }
}

/// Graphique détaillé de la cadence : une courbe segment par segment — chaque
/// tronçon prend le ton de sa cadence — sous laquelle l'aire se dégrade vers
/// le bas. Le curseur y lit la cadence et l'heure du point survolé.
fn fps_sparkline(ui: &mut egui::Ui, series: &[u32], first: i64, last: i64, width: f32) {
    if series.len() < 2 {
        return;
    }
    let width = width.max(80.0);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, FPS_CHART_HEIGHT), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, egui::Rounding::same(8.0), PANEL());
    let highest = series.iter().copied().max().unwrap_or(1).max(1);
    let lowest = series.iter().copied().min().unwrap_or(0);
    let span = (highest.saturating_sub(lowest)).max(1) as f32;
    let inner = rect.shrink2(egui::vec2(8.0, 7.0));
    let y_of = |value: f32| inner.bottom() - inner.height() * (value - lowest as f32) / span;
    // Trois repères horizontaux discrets : l'œil situe un creux sans avoir à
    // lire l'infobulle.
    for level in [0.25, 0.5, 0.75] {
        let y = inner.bottom() - inner.height() * level;
        painter.line_segment(
            [egui::pos2(inner.left(), y), egui::pos2(inner.right(), y)],
            egui::Stroke::new(1.0_f32, with_alpha(BORDER(), 0.45)),
        );
    }
    let points: Vec<egui::Pos2> = series
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let x = inner.left() + inner.width() * index as f32 / (series.len() - 1) as f32;
            egui::pos2(x, y_of(*value as f32))
        })
        .collect();
    // L'aire sous la courbe se dégrade vers le bas et prend le ton de la
    // cadence : une chute colore la zone qu'elle occupe.
    let mut mesh = egui::epaint::Mesh::default();
    for (index, point) in points.iter().enumerate() {
        let tone = fps_tone(series[index], highest);
        mesh.colored_vertex(*point, with_alpha(tone, 0.26));
        mesh.colored_vertex(egui::pos2(point.x, inner.bottom()), with_alpha(tone, 0.02));
        if index > 0 {
            let base = (index as u32 - 1) * 2;
            mesh.add_triangle(base, base + 1, base + 2);
            mesh.add_triangle(base + 1, base + 2, base + 3);
        }
    }
    painter.add(egui::Shape::mesh(mesh));
    // La moyenne, en pointillés : la ligne dit si la session tient sa cadence
    // ou si elle vit sur un pic isolé.
    let average = series.iter().map(|value| *value as f32).sum::<f32>() / series.len() as f32;
    let avg_y = y_of(average);
    let mut dash_x = inner.left();
    while dash_x < inner.right() {
        let dash_end = (dash_x + 4.0).min(inner.right());
        painter.line_segment(
            [egui::pos2(dash_x, avg_y), egui::pos2(dash_end, avg_y)],
            egui::Stroke::new(1.0_f32, with_alpha(TEXT_DIM(), 0.55)),
        );
        dash_x += 8.0;
    }
    // Un tronçon par segment : la chute se voit à sa couleur, pas seulement à
    // sa pente. Le tronçon prend le ton le plus bas des deux points qu'il joint.
    for index in 1..points.len() {
        let tone = fps_tone(series[index].min(series[index - 1]), highest);
        painter.line_segment(
            [points[index - 1], points[index]],
            egui::Stroke::new(1.7_f32, tone),
        );
    }
    if let Some(last_point) = points.last() {
        painter.circle_filled(*last_point, 2.6, OK);
    }
    // Curseur : une verticale, un point, et le relevé posé près de la souris.
    //
    // Le relevé est dessiné ici plutôt que confié à une infobulle : celle-ci
    // vit sur une couche au-dessus du graphique et lui prend le survol dès
    // qu'elle passe sous le curseur, ce qui fait sauter le repère. La position
    // vient de la réponse du graphique — déjà ramenée dans son espace — et non
    // du pointeur brut, qui peut être décalé par un défilement.
    // Deux lectures, dans l'ordre : celle de la réponse (ramenée dans son
    // espace), puis le pointeur brut si la première sort du cadre — un
    // défilement ou une couche transformée peut décaler l'une des deux, et
    // seule la position vraiment dans le cadre compte.
    let raw = ui.ctx().input(|input| input.pointer.hover_pos());
    let pointer = response
        .hover_pos()
        .filter(|pointer| rect.contains(*pointer))
        .or_else(|| raw.filter(|pointer| rect.contains(*pointer)));
    if let Some(pointer) = pointer {
        let ratio = ((pointer.x - inner.left()) / inner.width()).clamp(0.0, 1.0);
        let index = ((ratio * (series.len() - 1) as f32).round() as usize).min(series.len() - 1);
        let point = points[index];
        painter.line_segment(
            [point, egui::pos2(point.x, inner.bottom())],
            egui::Stroke::new(1.0_f32, with_alpha(TEXT(), 0.35)),
        );
        painter.circle_filled(point, 3.2, fps_tone(series[index], highest));
        // Les points sont à pas de temps constant : l'index suffit à placer
        // l'instant, et la minute affichée est celle de la mesure.
        let time = if first > 0 && last >= first {
            let total = (series.len() - 1).max(1) as i64;
            let at = first + (last - first) * index as i64 / total;
            lapinux_core::util::format_time(at)
        } else {
            String::new()
        };
        let readout = if time.is_empty() {
            format!("{} FPS", series[index])
        } else {
            format!("{time} · {} FPS", series[index])
        };
        let galley = painter.layout_no_wrap(readout, egui::FontId::proportional(10.0), TEXT());
        let size = galley.size() + egui::vec2(12.0, 5.0);
        // Au-dessus du curseur, puis ramené dans le cadre pour ne pas sortir
        // de la carte quand le pointeur est sur un bord.
        let at = egui::pos2(
            (pointer.x - size.x / 2.0).clamp(rect.left() + 2.0, rect.right() - size.x - 2.0),
            (pointer.y - size.y - 6.0).clamp(rect.top() + 2.0, rect.bottom() - size.y - 2.0),
        );
        painter.rect_filled(
            egui::Rect::from_min_size(at, size),
            egui::Rounding::same(5.0),
            with_alpha(CARD_ACTIVE(), 0.96),
        );
        painter.galley(at + egui::vec2(6.0, 2.5), galley, TEXT());
    }
}

/// Ce qu'une carte ou une catégorie d'Activité demande : ouvrir un journal,
/// retirer une partie ou un téléchargement, ou vider une journée.
#[derive(Debug, Clone)]
enum ActivityRequest {
    Log(LogRequest),
    DeleteSession { appid: u32, started: i64 },
    DeleteJob { id: String },
    ClearDay(i64),
}

/// « 78 FPS maintenant · 45 min · 78 moy · 120 max » : la cadence se lit
/// d'abord en direct, puis ses bornes.
fn fps_line(fps: &lapinux_core::model::FpsStats) -> String {
    if fps.is_empty() {
        return String::new();
    }
    // Les bornes vivent dans la légende, sous la courbe : la ligne du dessus
    // ne garde que la cadence du moment.
    if fps.current == 0 {
        "Cadence de la partie".into()
    } else {
        format!("{} FPS maintenant", fps.current)
    }
}

/// Point de légende : la pastille, sa valeur et son nom. Ajouté dans un
/// layout de droite à gauche, le nom tombe à droite du chiffre, la pastille à
/// sa gauche — « ● 81 min » se lit de gauche à droite.
fn fps_legend_item(ui: &mut egui::Ui, name: &str, value: u32, color: egui::Color32) {
    ui.label(egui::RichText::new(name).size(10.0).color(TEXT_DIM()));
    ui.label(
        egui::RichText::new(value.to_string())
            .size(10.0)
            .strong()
            .color(TEXT()),
    );
    let (rect, _) = ui.allocate_exact_size(egui::vec2(7.0, 7.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 3.0, color);
}

/// Deux feuilles superposées, centrées dans leur bouton : l'icône « copier »
/// d'une carte d'activité, dessinée au même poids que les autres glyphes.
fn paint_copy_glyph(
    painter: &egui::Painter,
    center: egui::Pos2,
    fill: egui::Color32,
    color: egui::Color32,
) {
    let back = egui::Rect::from_min_size(center + egui::vec2(-7.0, -8.5), egui::vec2(10.0, 13.0));
    let front = egui::Rect::from_min_size(center + egui::vec2(-1.5, -4.5), egui::vec2(10.0, 13.0));
    painter.rect_stroke(
        back,
        egui::Rounding::same(2.5),
        egui::Stroke::new(1.1_f32, color),
    );
    // La feuille du dessus masque le trait de celle du dessous.
    painter.rect_filled(front, egui::Rounding::same(2.5), fill);
    painter.rect_stroke(
        front,
        egui::Rounding::same(2.5),
        egui::Stroke::new(1.1_f32, color),
    );
}

/// Icône « copier » d'une carte : le journal part dans le presse-papiers sans
/// ouvrir le panneau. L'icône passe au vert une seconde et demie, assez pour
/// voir que la copie a eu lieu.
fn activity_copy(ui: &mut egui::Ui, path: &str) {
    let id = ui.id().with("activity-copy");
    let now = ui.input(|input| input.time);
    let copied_at: Option<f64> = ui.data(|data| data.get_temp(id));
    let fresh = copied_at.is_some_and(|at| now - at < 1.5);
    if fresh {
        ui.ctx().request_repaint_after(Duration::from_millis(150));
    }
    let (rect, response) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::click());
    if response.hovered() || fresh {
        ui.painter()
            .rect_filled(rect, egui::Rounding::same(6.0), CARD_HOVER());
    }
    let tint = if fresh {
        OK
    } else if response.hovered() {
        TEXT()
    } else {
        TEXT_DIM()
    };
    let fill = if response.hovered() || fresh {
        CARD_HOVER()
    } else {
        CARD()
    };
    paint_copy_glyph(ui.painter(), rect.center(), fill, tint);
    let _ = response
        .clone()
        .on_hover_text(if fresh { "Copié" } else { "Copier le journal" });
    if response.clicked() {
        if let Some(text) = read_log_tail(path) {
            ui.ctx().copy_text(text);
            ui.data_mut(|data| data.insert_temp(id, now));
        }
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand);
}

/// Petite poubelle de carte, dessinée au trait comme les glyphes des
/// catégories.
fn activity_trash(ui: &mut egui::Ui, tooltip: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::click());
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, egui::Rounding::same(6.0), CARD_HOVER());
    }
    paint_hero_glyph(
        ui.painter(),
        rect.center(),
        HeroGlyph::Trash,
        if response.hovered() {
            DANGER
        } else {
            TEXT_DIM()
        },
    );
    let _ = response.clone().on_hover_text(tooltip);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Quand la partie a commencé et fini, tel que la carte l'écrit.
fn session_times(started: i64, ended: i64) -> String {
    use lapinux_core::util::{day_start, format_date, format_time};
    if started <= 0 {
        return String::new();
    }
    if ended <= 0 {
        return format!("Lancée à {}", format_time(started));
    }
    if day_start(started) == day_start(ended) {
        return format!(
            "Lancée à {} · fermée à {}",
            format_time(started),
            format_time(ended)
        );
    }
    // Une partie à cheval sur minuit garde ses deux dates : l'heure seule
    // ferait croire à une session de quelques minutes.
    format!(
        "Lancée le {} à {} · fermée le {} à {}",
        format_date(started),
        format_time(started),
        format_date(ended),
        format_time(ended)
    )
}

/// Fin d'un journal de session : les dernières lignes suffisent à raconter
/// le lancement, et une partie de plusieurs heures ne pèse pas dans l'écran.
fn read_log_tail(path: &str) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    let tail = 160 * 1024;
    let from = length.saturating_sub(tail);
    file.seek(SeekFrom::Start(from)).ok()?;
    let mut text = String::new();
    file.take(tail).read_to_string(&mut text).ok()?;
    if from > 0 {
        text.insert_str(0, "…\n");
    }
    Some(text)
}

/// Une journée de la page Activité : ses parties et ses téléchargements sous
/// un en-tête repliable daté.
struct ActivitySection {
    /// Minuit local du jour : clé de l'en-tête, du repli et de la suppression.
    day: i64,
    /// Compte affiché à côté du titre, comme dans Téléchargements.
    detail: String,
    cards: Vec<ActivityCard>,
}

/// Journal demandé par une carte : le panneau fixe l'ouvre sans quitter la
/// page Activité.
#[derive(Debug, Clone)]
struct LogRequest {
    name: String,
    path: String,
}

/// État replié/déplié d'une catégorie d'Activité, gardé dans la mémoire egui
/// pour survivre aux images sans champ dédié dans l'application.
fn activity_open(ui: &egui::Ui, key: egui::Id, default: bool) -> bool {
    ui.data_mut(|data| *data.get_temp_mut_or_insert_with(key, || default))
}

fn activity_set_open(ui: &egui::Ui, key: egui::Id, open: bool) {
    ui.data_mut(|data| data.insert_temp(key, open));
}

/// Catégorie repliée : les vignettes des jeux, alignées à droite de l'en-tête
/// juste avant le chevron — la même rangée que dans Téléchargements.
fn activity_icons(
    ui: &mut egui::Ui,
    free: egui::Rect,
    covers: &mut CoverCache,
    paths: &LapinuxPaths,
    cards: &[&ActivityCard],
) {
    let entries: Vec<&ActivityCard> = cards
        .iter()
        .copied()
        .filter(|card| card.appid != 0)
        .collect();
    if entries.is_empty() || free.width() < 24.0 {
        return;
    }
    const SIZE: f32 = 20.0;
    const GAP: f32 = 5.0;
    let (shown, more, offset) = downloads::collapsed_strip(free.width(), entries.len());
    let painter = ui.painter().clone();
    let mut x = free.left() + offset;
    let top = free.center().y - SIZE / 2.0;
    for card in entries.iter().take(shown) {
        let icon_rect = egui::Rect::from_min_size(egui::pos2(x, top), egui::vec2(SIZE, SIZE));
        painter.rect_filled(icon_rect, egui::Rounding::same(4.0), PANEL());
        match covers.get(
            ui.ctx(),
            paths,
            card.appid,
            &card.name,
            card.store,
            &card.remote,
            CoverTier::Tile,
        ) {
            Some(texture) => {
                let size = texture.size_vec2();
                if size.x > 0.0 && size.y > 0.0 {
                    let scale = (SIZE / size.x).max(SIZE / size.y);
                    let shown = egui::Rect::from_center_size(icon_rect.center(), size * scale);
                    painter.with_clip_rect(icon_rect).image(
                        texture.id(),
                        shown,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                }
            }
            None => {
                let initial = card
                    .name
                    .chars()
                    .next()
                    .map(|ch| ch.to_uppercase().to_string())
                    .unwrap_or_default();
                painter.text(
                    icon_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    initial,
                    display_font(11.0),
                    TEXT_DIM(),
                );
            }
        }
        x += SIZE + GAP;
    }
    if more > 0 {
        painter.text(
            egui::pos2(x + 1.0, free.center().y),
            egui::Align2::LEFT_CENTER,
            format!("+{more}"),
            egui::FontId::proportional(10.5),
            TEXT_DIM(),
        );
    }
}

/// Page **Activité** : l'historique de la session en cartes de jeu, sous la
/// liste du Profil. Elle ne dépend d'aucune boutique ni d'aucun compte.
///
/// Rend le journal demandé par une carte, à ouvrir dans le panneau fixe.
/// Carte « Nouveaux jeux » de la page Activité : le nombre de jeux arrivés
/// depuis le dernier lancement, avec leurs jaquettes. C'est le résultat du
/// contrôle automatique du démarrage ou de l'icône de rafraîchissement.
fn new_games_card(
    ui: &mut egui::Ui,
    covers: &mut CoverCache,
    paths: &LapinuxPaths,
    games: &[LibraryEntry],
) {
    egui::Frame::none()
        .fill(CARD())
        .stroke(egui::Stroke::new(1.0_f32, BORDER()))
        .rounding(egui::Rounding::same(12.0))
        .inner_margin(16.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let count = games.len();
            ui.label(
                egui::RichText::new(if count == 1 {
                    "1 nouveau jeu ajouté".to_string()
                } else {
                    format!("{count} nouveaux jeux ajoutés")
                })
                .size(13.0)
                .strong()
                .color(TEXT()),
            );
            ui.add_space(2.0);
            muted(ui, "Depuis le dernier lancement de Lapinux.");
            ui.add_space(10.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                for entry in games.iter().take(12) {
                    let texture = covers.get(
                        ui.ctx(),
                        paths,
                        entry.appid,
                        &entry.name,
                        entry.store,
                        &entry.cover,
                        CoverTier::Sheet,
                    );
                    let (rect, response) =
                        ui.allocate_exact_size(egui::vec2(44.0, 58.0), egui::Sense::hover());
                    ui.painter()
                        .rect_filled(rect, egui::Rounding::same(6.0), PANEL());
                    match &texture {
                        Some(texture) => {
                            let size = texture.size_vec2();
                            if size.x > 0.0 && size.y > 0.0 {
                                let scale = (rect.width() / size.x).max(rect.height() / size.y);
                                let shown =
                                    egui::Rect::from_center_size(rect.center(), size * scale);
                                ui.painter().with_clip_rect(rect).image(
                                    texture.id(),
                                    shown,
                                    egui::Rect::from_min_max(
                                        egui::pos2(0.0, 0.0),
                                        egui::pos2(1.0, 1.0),
                                    ),
                                    egui::Color32::WHITE,
                                );
                            }
                        }
                        None => {
                            let initial = entry
                                .name
                                .chars()
                                .next()
                                .map(|ch| ch.to_uppercase().to_string())
                                .unwrap_or_default();
                            ui.painter().text(
                                rect.center(),
                                egui::Align2::CENTER_CENTER,
                                initial,
                                egui::FontId::proportional(18.0),
                                TEXT_DIM(),
                            );
                        }
                    }
                    response.on_hover_text(&entry.name);
                }
                if count > 12 {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(44.0, 58.0), egui::Sense::hover());
                    ui.painter()
                        .rect_filled(rect, egui::Rounding::same(6.0), PANEL());
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        format!("+{}", count - 12),
                        egui::FontId::proportional(13.0),
                        TEXT_DIM(),
                    );
                }
            });
        });
}

fn activity_card_height_id(ui: &egui::Ui, card: &ActivityCard) -> egui::Id {
    ui.id().with((
        "activity-card-height",
        card.appid,
        card.started,
        card.ended,
        card.fps_series.len(),
        ui.available_width().round() as u32,
    ))
}

fn activity_page(
    ui: &mut egui::Ui,
    covers: &mut CoverCache,
    paths: &LapinuxPaths,
    sections: &[ActivitySection],
    new_games: &[LibraryEntry],
) -> Option<ActivityRequest> {
    ui.add_space(2.0);
    if !new_games.is_empty() {
        new_games_card(ui, covers, paths, new_games);
        ui.add_space(10.0);
    }
    if sections.is_empty() {
        ui.label(
            egui::RichText::new(
                "Rien à raconter pour l'instant : lance un jeu ou un téléchargement, cette page \
                 se remplira.",
            )
            .size(12.0)
            .color(TEXT_DIM()),
        );
        return None;
    }
    let now = lapinux_core::util::now_unix();
    let mut requested: Option<ActivityRequest> = None;
    for section in sections {
        let key = egui::Id::new(("activity-day", section.day));
        // Aujourd'hui et hier s'ouvrent d'emblée ; les autres jours attendent
        // un clic.
        let default = section.day == lapinux_core::util::day_start(now)
            || section.day == lapinux_core::util::day_start(now - 86_400);
        let open = activity_open(ui, key, default);
        let label = lapinux_core::util::day_label(section.day, now);
        // Une journée peut contenir des parties et des téléchargements : la
        // corbeille de l'en-tête retire les deux. Ce qui tourne encore n'est
        // pas supprimable et ne bloque donc pas le nettoyage de l'historique.
        let has_deletable = section.cards.iter().any(|card| card.delete.is_some());
        let trash = if has_deletable
            && section
                .cards
                .iter()
                .all(|card| card.delete.is_some() || card.ended.is_none())
        {
            Some((HeroGlyph::Trash, "Supprimer cette journée"))
        } else {
            None
        };
        let header = LapinuxApp::section_heading(ui, &label, &section.detail, trash, open);
        if header.action {
            return Some(ActivityRequest::ClearDay(section.day));
        }
        if header.toggle {
            activity_set_open(ui, key, !open);
        }
        let cards: Vec<&ActivityCard> = section.cards.iter().collect();
        if !open || (header.toggle && open) {
            // Replié : les vignettes des jeux prennent la ligne du titre.
            activity_icons(ui, header.free, covers, paths, &cards);
            ui.add_space(14.0);
            continue;
        }
        ui.add_space(4.0);
        for card in &section.cards {
            let height_id = activity_card_height_id(ui, card);
            let measured: Option<f32> = ui.data(|data| data.get_temp(height_id));
            let estimate = measured.unwrap_or(if card.fps_series.len() > 1 {
                ACTIVITY_CARD_WITH_CHART
            } else {
                ACTIVITY_CARD_PLAIN
            });
            let top = ui.cursor().top();
            let clip = ui.clip_rect();
            // Une carte hors écran n'est ni construite ni peinte : la page
            // Activité peut garder des centaines de parties sans les payer à
            // chaque image. La hauteur mesurée garde le défilement stable.
            if top + estimate + ACTIVITY_CARD_GAP < clip.top() || top > clip.bottom() {
                ui.add_space(estimate + ACTIVITY_CARD_GAP);
                continue;
            }
            if let Some(request) = activity_card(ui, covers, paths, card) {
                requested = Some(request);
            }
            ui.add_space(ACTIVITY_CARD_GAP);
        }
        ui.add_space(8.0);
    }
    requested
}

/// Range des cartes par journée locale, la plus récente en tête : les parties
/// et les téléchargements d'un même jour se retrouvent dans la même
/// catégorie, ce qui tourne encore passant devant, puis le plus récent.
///
/// La date vient du début de la carte ; une carte sans horodatage tombe
/// aujourd'hui plutôt qu'au 1er janvier 1970.
fn group_by_day(cards: Vec<ActivityCard>, now: i64) -> Vec<(i64, Vec<ActivityCard>)> {
    let mut days: Vec<(i64, Vec<ActivityCard>)> = Vec::new();
    for card in cards {
        let stamp = card
            .started
            .or(card.ended)
            .filter(|stamp| *stamp > 0)
            .unwrap_or(now);
        let day = lapinux_core::util::day_start(stamp);
        match days.iter_mut().find(|(key, _)| *key == day) {
            Some((_, list)) => list.push(card),
            None => days.push((day, vec![card])),
        }
    }
    days.sort_by(|left, right| right.0.cmp(&left.0));
    for (_, cards) in &mut days {
        cards.sort_by(|left, right| {
            let live = |card: &ActivityCard| card.ended.is_none();
            live(right)
                .cmp(&live(left))
                .then_with(|| right.started.cmp(&left.started))
        });
    }
    days
}

/// Icône « journal » : une page et ses lignes, dessinée au trait.
fn paint_log_glyph(painter: &egui::Painter, center: egui::Pos2, color: egui::Color32) {
    let page = egui::Rect::from_center_size(center, egui::vec2(13.0, 16.0));
    painter.rect_stroke(
        page,
        egui::Rounding::same(2.5),
        egui::Stroke::new(1.1_f32, color),
    );
    let left = page.left() + 3.0;
    let right = page.right() - 3.0;
    for line in 0..3 {
        let y = page.top() + 4.5 + line as f32 * 3.5;
        painter.line_segment(
            [
                egui::pos2(left, y),
                egui::pos2(if line == 2 { left + 3.5 } else { right }, y),
            ],
            egui::Stroke::new(1.0_f32, color),
        );
    }
}

/// Bouton du journal d'une carte : même gabarit que la copie et la poubelle.
fn activity_log_icon(ui: &mut egui::Ui, tooltip: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::click());
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, egui::Rounding::same(6.0), CARD_HOVER());
    }
    paint_log_glyph(
        ui.painter(),
        rect.center(),
        if response.hovered() {
            TEXT()
        } else {
            TEXT_DIM()
        },
    );
    let _ = response.clone().on_hover_text(tooltip);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Une carte : vignette portrait, titre, ce qu'il en est, et une pastille à
/// droite (durée, état, progression). La jaquette vient du même cache que la
/// bibliothèque ; sans elle, l'initiale du jeu tient la place.
///
/// La ligne se lit de gauche à droite : la jaquette, le titre suivi de ses
/// heures, le graphique qui court jusqu'aux actions, et la liste d'icônes au
/// bord droit.
fn activity_card(
    ui: &mut egui::Ui,
    covers: &mut CoverCache,
    paths: &LapinuxPaths,
    card: &ActivityCard,
) -> Option<ActivityRequest> {
    let mut requested = None;
    // Une tâche sans jeu identifié n'a pas de jaquette à chercher : son
    // appid nul enverrait le cache vers une adresse qui n'existe pas.
    let texture = if card.appid == 0 {
        None
    } else {
        covers.get(
            ui.ctx(),
            paths,
            card.appid,
            &card.name,
            card.store,
            &card.remote,
            CoverTier::Sheet,
        )
    };
    let height_id = activity_card_height_id(ui, card);
    let frame = egui::Frame::none()
        .fill(CARD())
        .stroke(egui::Stroke::new(1.0_f32, BORDER()))
        .rounding(egui::Rounding::same(12.0))
        .inner_margin(egui::Margin::same(10.0))
        .show(ui, |ui| {
            let content = ui.available_width();
            ui.set_width(content);
            ui.horizontal_top(|ui| {
                // Un seul écart, celui de la jaquette à la colonne du texte.
                ui.spacing_mut().item_spacing.x = 12.0;
                const COVER_W: f32 = 46.0;
                const ACTIONS_W: f32 = 34.0;
                const GAPS: f32 = 24.0;
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(COVER_W, 62.0), egui::Sense::hover());
                ui.painter()
                    .rect_filled(rect, egui::Rounding::same(7.0), PANEL());
                match &texture {
                    Some(texture) => {
                        let size = texture.size_vec2();
                        if size.x > 0.0 && size.y > 0.0 {
                            let scale = (rect.width() / size.x).max(rect.height() / size.y);
                            let shown = egui::Rect::from_center_size(rect.center(), size * scale);
                            ui.painter().with_clip_rect(rect).image(
                                texture.id(),
                                shown,
                                egui::Rect::from_min_max(
                                    egui::pos2(0.0, 0.0),
                                    egui::pos2(1.0, 1.0),
                                ),
                                egui::Color32::WHITE,
                            );
                        }
                    }
                    None => {
                        let initial = card
                            .name
                            .chars()
                            .next()
                            .map(|ch| ch.to_uppercase().to_string())
                            .unwrap_or_default();
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            initial,
                            egui::FontId::proportional(18.0),
                            TEXT_DIM(),
                        );
                    }
                }
                // Le bloc qui court de la jaquette aux icônes : le titre, les
                // heures et les nombres sur une ligne, puis la courbe collée
                // dessous — elle occupe tout le haut de la carte.
                let block = (content - COVER_W - ACTIONS_W - GAPS).max(240.0);
                ui.vertical(|ui| {
                    ui.set_width(block);
                    ui.spacing_mut().item_spacing.y = 4.0;
                    let line = card.fps.map(|fps| fps_line(&fps)).unwrap_or_default();
                    let chart = !card.fps_series.is_empty();
                    let fps = card.fps.filter(|fps| !fps.is_empty());
                    let legend = |ui: &mut egui::Ui, fps: lapinux_core::model::FpsStats| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        fps_legend_item(ui, "max", fps.max, OK);
                        ui.add_space(8.0);
                        fps_legend_item(ui, "moy", fps.avg, ACCENT());
                        ui.add_space(8.0);
                        fps_legend_item(ui, "min", fps.min, WARN);
                    };
                    // Les nombres min, moyenne et max, au bout de la ligne du
                    // titre : ils coiffent la courbe de près.
                    let mut legend_inline = false;
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        ui.label(
                            egui::RichText::new(card.name.clone())
                                .size(13.5)
                                .strong()
                                .color(TEXT()),
                        );
                        if !card.tag.is_empty() {
                            ui.add_space(2.0);
                            activity_tag(ui, &card.tag, card.tone);
                        }
                        if !card.detail.is_empty() {
                            ui.add_space(2.0);
                            // Bornée : la légende garde sa place au bout de la
                            // ligne, même quand les heures s'allongent.
                            let width = (ui.available_width() - 176.0).max(60.0);
                            ui.add_sized(
                                egui::vec2(width, 16.0),
                                egui::Label::new(
                                    egui::RichText::new(card.detail.clone())
                                        .size(11.0)
                                        .color(TEXT_DIM()),
                                )
                                .truncate(),
                            );
                        } else if !chart && !line.is_empty() {
                            ui.add_space(6.0);
                            ui.label(
                                egui::RichText::new(line.clone())
                                    .size(10.5)
                                    .strong()
                                    .color(ACCENT()),
                            );
                        }
                        if let Some(fps) = fps {
                            // Sur une carte étroite, la légende descend d'une
                            // ligne plutôt que de sortir du cadre.
                            if ui.available_width() >= 150.0 {
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| legend(ui, fps),
                                );
                                legend_inline = true;
                            }
                        }
                    });
                    if let Some(fps) = fps {
                        if !legend_inline {
                            ui.allocate_ui_with_layout(
                                egui::vec2(block, 12.0),
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| legend(ui, fps),
                            );
                        }
                    }
                    if let Some(progress) = card.progress {
                        ui.add_space(5.0);
                        let width = ui.available_width().min(420.0);
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(width, 4.0), egui::Sense::hover());
                        ui.painter()
                            .rect_filled(rect, egui::Rounding::same(2.0), CARD_ACTIVE());
                        let done = egui::Rect::from_min_size(
                            rect.min,
                            egui::vec2(rect.width() * progress.clamp(0.0, 1.0), rect.height()),
                        );
                        ui.painter()
                            .rect_filled(done, egui::Rounding::same(2.0), ACCENT());
                    }
                    if chart {
                        fps_sparkline(
                            ui,
                            &card.fps_series,
                            card.fps_series_start,
                            card.fps_series_end,
                            block,
                        );
                    }
                });
                // Icônes au bord droit, en liste : la poubelle en tête, puis
                // le journal et la copie.
                ui.with_layout(egui::Layout::top_down(egui::Align::Max), |ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    if let Some(delete) = &card.delete {
                        if activity_trash(ui, delete.tooltip()).clicked() {
                            requested = Some(delete.request());
                        }
                    }
                    if !card.log.trim().is_empty() {
                        if activity_log_icon(ui, "Journal de la session").clicked() {
                            requested = Some(ActivityRequest::Log(LogRequest {
                                name: card.name.clone(),
                                path: card.log.clone(),
                            }));
                        }
                        activity_copy(ui, &card.log);
                    }
                });
            });
        });
    ui.data_mut(|data| data.insert_temp(height_id, frame.response.rect.height()));
    requested
}

/// Pastille d'une carte : un fond discret, le texte dans la couleur du ton.
fn activity_tag(ui: &mut egui::Ui, text: &str, color: egui::Color32) {
    let galley =
        ui.painter()
            .layout_no_wrap(text.to_string(), egui::FontId::proportional(11.0), color);
    let size = egui::vec2(galley.size().x + 16.0, 20.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, egui::Rounding::same(10.0), CARD_ACTIVE());
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, color);
}

/// Page **Discord** : la Rich Presence et ce qu'elle publie. Le titre venant
/// de l'application qui publie, jeux et fiches passent sous celle du jeu ;
/// les pages restent sous celle de Lapinux, avec leur phrase et leur marque.
fn discord_page(
    ui: &mut egui::Ui,
    discord: &lapinux_core::config::DiscordConfig,
    update: &mut Option<lapinux_core::config::DiscordConfig>,
) {
    let mut draft = discord.clone();
    let mut changed = false;
    ui.add_space(2.0);
    ui.label(
        egui::RichText::new("Discord")
            .size(20.0)
            .strong()
            .color(TEXT()),
    );
    ui.add_space(3.0);
    ui.label(
        egui::RichText::new("Ce que tes amis voient de toi quand Lapinux est lancé.")
            .size(11.5)
            .color(TEXT_DIM()),
    );
    ui.add_space(14.0);
    settings_card(
        ui,
        "Rich Presence",
        "Une activité Discord qui suit ce que tu fais dans Lapinux.",
        |ui| {
            ui.horizontal(|ui| {
                status_dot(ui, if draft.enabled { OK } else { TEXT_DIM() });
                ui.add_space(2.0);
                ui.label(
                    egui::RichText::new(if draft.enabled {
                        "Présence active"
                    } else {
                        lapinux_core::i18n::t("profile_page.presence_off")
                    })
                    .size(12.0)
                    .color(TEXT()),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if switch(ui, &mut draft.enabled).changed() {
                        changed = true;
                    }
                });
            });
            ui.add_space(6.0);
            muted(
                ui,
                "Le titre d'une activité Discord est le nom de l'application qui la \
                 publie : en partie et sur une fiche, Lapinux se présente sous \
                 l'application officielle du jeu quand Discord la connaît. Sinon la \
                 présence se publie sous Lapinux, avec le nom et la jaquette du jeu. \
                 Une fiche se lit « Regarde », une partie « Joue ».",
            );
            ui.add_space(12.0);
            ui.label(
                egui::RichText::new("Ce qui est publié")
                    .size(11.0)
                    .color(TEXT_DIM()),
            );
            ui.add_space(4.0);
            for (label, hint, value) in [
                (
                    "La partie en cours",
                    "Jaquette, cadence et moyenne des images, version de Proton et temps écoulé.",
                    &mut draft.show_game,
                ),
                (
                    "Le téléchargement",
                    "Le jeu, sa progression, son débit et le temps écoulé.",
                    &mut draft.show_downloads,
                ),
                (
                    "Le jeu regardé",
                    "La fiche ouverte, annoncée par « Regarde le jeu … » : jaquette, boutique, temps de jeu et installation.",
                    &mut draft.show_viewing,
                ),
                (
                    "Les pages de l'application",
                    "La page ouverte et son contenu : jeux installés, téléchargements, manettes, captures, onglet des réglages.",
                    &mut draft.show_library,
                ),
            ] {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(label).size(11.5).color(TEXT()));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if switch(ui, value).changed() {
                            changed = true;
                        }
                    });
                });
                muted(ui, hint);
                ui.add_space(6.0);
            }
        },
    );
    if changed {
        *update = Some(draft);
    }
}

/// Une case vide veut dire « comme la carte » : le plafond DXVK reste celui
/// de la machine. Un zéro, une valeur négative ou un texte illisible ne
/// touchait rien non plus.
fn memory_from_text(text: &str) -> Option<u32> {
    text.trim().parse::<u32>().ok().filter(|mb| *mb > 0)
}

/// Curseur des cartes de réglages. La piste d'un `Slider` prend
/// `inactive.bg_fill`, qui vaut `CARD()` : sur une carte de cette même couleur,
/// elle disparaît et seule la poignée reste visible. Le scope local éclaircit
/// la piste sans changer les boutons du reste de l'interface.
/// Durée de rangement automatique, telle que la liste la propose.
fn parking_label(minutes: u32) -> String {
    match minutes {
        0 => lapinux_core::i18n::t("settings.general.parking_never").to_string(),
        n => format!("{n} min"),
    }
}

fn nx_avatar(
    painter: &egui::Painter,
    center: egui::Pos2,
    radius: f32,
    initial: &str,
    ring: egui::Color32,
    fill: egui::Color32,
    ink: egui::Color32,
) {
    painter.circle_filled(center, radius + 2.0, ring);
    painter.circle_filled(center, (radius - 2.0).max(1.0), fill);
    let text = initial
        .chars()
        .next()
        .map(|ch| ch.to_uppercase().to_string())
        .unwrap_or_else(|| "?".to_string());
    painter.text(
        center,
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::proportional((radius * 0.82).max(9.0)),
        ink,
    );
}

fn nx_mii_avatar(
    painter: &egui::Painter,
    center: egui::Pos2,
    radius: f32,
    fill: egui::Color32,
    active: bool,
    ink: egui::Color32,
) {
    if active {
        painter.circle_filled(center, radius + 2.5, egui::Color32::WHITE);
    }
    painter.circle_filled(center, radius, fill);
    // Une silhouette de joueur, plutôt qu'une initiale : le buste et la tête
    // restent dans le disque de l'avatar.
    painter.circle_filled(center + egui::vec2(0.0, radius * 0.56), radius * 0.44, ink);
    painter.circle_filled(center + egui::vec2(0.0, -radius * 0.14), radius * 0.31, ink);
}

fn nx_key_hint(
    painter: &egui::Painter,
    right: f32,
    y: f32,
    key: &str,
    label: &str,
    ink: egui::Color32,
    dim: egui::Color32,
    background: egui::Color32,
) -> f32 {
    let galley = painter.layout_no_wrap(label.to_string(), egui::FontId::proportional(11.5), dim);
    let cap = 9.0;
    let label_left = right - galley.size().x;
    let center = egui::pos2(label_left - 6.0 - cap, y);
    painter.circle_filled(center, cap, ink);
    painter.text(
        center,
        egui::Align2::CENTER_CENTER,
        key,
        egui::FontId::proportional(10.5),
        background,
    );
    painter.galley(
        egui::pos2(label_left, y - galley.size().y / 2.0),
        galley,
        dim,
    );
    center.x - cap - 18.0
}

/// Halo de fond de l'accueil NX : la couleur du jeu sélectionné s'épanouit
/// au centre de l'écran et retombe vers les bords, sans repeindre les cartes.
fn paint_nx_ambient(
    painter: &egui::Painter,
    rect: egui::Rect,
    tone: egui::Color32,
    background: egui::Color32,
) {
    let edge = blend_colors(background, tone, 0.16);
    let upper = blend_colors(background, tone, 0.28);
    let center = blend_colors(background, tone, 0.50);
    let lower = blend_colors(background, tone, 0.12);
    let mut mesh = egui::Mesh::default();
    let xs = [rect.left(), rect.center().x, rect.right()];
    let ys = [rect.top(), rect.center().y, rect.bottom()];
    let colors = [
        [edge, upper, edge],
        [upper, center, upper],
        [background, lower, background],
    ];
    for (row, y) in colors.iter().zip(ys) {
        for (color, x) in row.iter().zip(xs) {
            mesh.colored_vertex(egui::pos2(x, y), *color);
        }
    }
    for (a, b, c, d) in [(0, 1, 3, 4), (1, 2, 4, 5), (3, 4, 6, 7), (4, 5, 7, 8)] {
        mesh.add_triangle(a, b, c);
        mesh.add_triangle(b, d, c);
    }
    painter.add(egui::Shape::mesh(mesh));
}

fn nx_wifi_icon(painter: &egui::Painter, center: egui::Pos2, ink: egui::Color32) {
    painter.circle_filled(center + egui::vec2(0.0, 5.0), 1.6, ink);
    for radius in [4.0_f32, 8.0, 12.0] {
        let mut points = Vec::with_capacity(12);
        for step in 0..=10 {
            let angle = -150.0_f32.to_radians() + (120.0_f32.to_radians()) * step as f32 / 10.0;
            points.push(center + egui::vec2(angle.cos(), angle.sin()) * radius);
        }
        painter.add(egui::Shape::line(points, egui::Stroke::new(1.35_f32, ink)));
    }
}

fn nx_battery_icon(
    painter: &egui::Painter,
    center: egui::Pos2,
    percent: Option<u8>,
    ink: egui::Color32,
    dim: egui::Color32,
) {
    let body = egui::Rect::from_center_size(center, egui::vec2(24.0, 12.0));
    painter.rect_stroke(
        body,
        egui::Rounding::same(2.0),
        egui::Stroke::new(1.3_f32, dim),
    );
    painter.rect_filled(
        egui::Rect::from_min_size(
            egui::pos2(body.right() + 0.5, body.center().y - 2.5),
            egui::vec2(2.2, 5.0),
        ),
        0.8,
        dim,
    );
    if let Some(percent) = percent {
        let fill = (body.width() - 4.0) * percent.min(100) as f32 / 100.0;
        painter.rect_filled(
            egui::Rect::from_min_size(
                body.min + egui::vec2(2.0, 2.0),
                egui::vec2(fill, body.height() - 4.0),
            ),
            1.0,
            if percent <= 15 {
                egui::Color32::from_rgb(232, 132, 132)
            } else {
                ink
            },
        );
    }
}

fn paint_nx_status(
    painter: &egui::Painter,
    rect: egui::Rect,
    now: i64,
    battery: Option<u8>,
    font_size: f32,
    ink: egui::Color32,
    dim: egui::Color32,
) {
    painter.text(
        egui::pos2(rect.right() - 100.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        lapinux_core::util::format_time_seconds(now),
        egui::FontId::proportional(font_size),
        ink,
    );
    nx_wifi_icon(
        painter,
        egui::pos2(rect.right() - 66.0, rect.center().y),
        ink,
    );
    nx_battery_icon(
        painter,
        egui::pos2(rect.right() - 20.0, rect.center().y),
        battery,
        ink,
        dim,
    );
}

fn paint_nx_controller(painter: &egui::Painter, center: egui::Pos2) {
    let ink = egui::Color32::from_rgb(166, 169, 176);
    let body = egui::Rect::from_center_size(center, egui::vec2(27.0, 14.0));
    painter.rect_stroke(
        body,
        egui::Rounding::same(4.0),
        egui::Stroke::new(1.2_f32, ink),
    );
    painter.circle_stroke(
        center + egui::vec2(-10.0, 0.0),
        3.0,
        egui::Stroke::new(1.0_f32, ink),
    );
    painter.circle_filled(center + egui::vec2(8.0, -3.0), 1.1, ink);
    painter.circle_filled(center + egui::vec2(11.0, 1.5), 1.1, ink);
}

/// Curseur de réglage avec une piste lisible : le remplissage montre la
/// position, les bornes sont écrites aux deux extrémités et la valeur courante
/// reste affichée au-dessus. Un clic sur la piste place directement le curseur.
fn settings_slider<T: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    value: &mut T,
    range: std::ops::RangeInclusive<T>,
    suffix: &str,
) -> egui::Response {
    let min = range.start().to_f64();
    let max = range.end().to_f64();
    let available = ui.available_width();
    let width = if available.is_finite() {
        available.clamp(150.0, 220.0)
    } else {
        150.0
    };
    let (rect, mut response) =
        ui.allocate_exact_size(egui::vec2(width, 38.0), egui::Sense::click_and_drag());
    let enabled = ui.is_enabled();
    let mut current = value.to_f64().clamp(min, max);
    let track = egui::Rect::from_min_max(
        egui::pos2(rect.left(), rect.top() + 19.0),
        egui::pos2(rect.right(), rect.top() + 24.0),
    );
    if enabled && (response.clicked() || response.dragged()) {
        if let Some(pointer) = response.interact_pointer_pos() {
            let pointer_t = ((pointer.x - track.left()) / track.width()).clamp(0.0, 1.0);
            let raw = min + pointer_t as f64 * (max - min);
            current = if T::INTEGRAL { raw.round() } else { raw };
            *value = T::from_f64(current.clamp(min, max));
            response.mark_changed();
        }
    }
    let t = if max > min {
        ((current - min) / (max - min)).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let painter = ui.painter();
    let rail = if enabled {
        with_alpha(BORDER(), 0.95)
    } else {
        with_alpha(BORDER(), 0.4)
    };
    painter.rect_filled(track, egui::Rounding::same(track.height() / 2.0), rail);
    let filled = egui::Rect::from_min_max(
        track.min,
        egui::pos2(track.left() + track.width() * t as f32, track.bottom()),
    );
    if t > 0.0 {
        painter.rect_filled(
            filled,
            egui::Rounding::same(track.height() / 2.0),
            if enabled {
                ACCENT()
            } else {
                TEXT_DIM().gamma_multiply(0.55)
            },
        );
    }
    let knob_x = track.left() + track.width() * t as f32;
    painter.circle_filled(
        egui::pos2(knob_x, track.center().y),
        7.0,
        if enabled {
            egui::Color32::from_rgb(244, 246, 250)
        } else {
            TEXT_DIM().gamma_multiply(0.6)
        },
    );
    painter.circle_stroke(
        egui::pos2(knob_x, track.center().y),
        7.0,
        egui::Stroke::new(
            1.5_f32,
            if enabled {
                ACCENT()
            } else {
                TEXT_DIM().gamma_multiply(0.5)
            },
        ),
    );
    let number = if (current - current.round()).abs() < 0.005 {
        format!("{}", current.round() as i64)
    } else {
        format!("{current:.2}")
    };
    painter.text(
        egui::pos2(rect.right(), rect.top() + 4.0),
        egui::Align2::RIGHT_TOP,
        format!("{number}{suffix}"),
        egui::FontId::proportional(10.5),
        if enabled { TEXT() } else { TEXT_DIM() },
    );
    painter.text(
        egui::pos2(rect.left(), rect.bottom()),
        egui::Align2::LEFT_BOTTOM,
        format!("{}{suffix}", format_slider_bound(min)),
        egui::FontId::proportional(9.0),
        TEXT_DIM().gamma_multiply(0.9),
    );
    painter.text(
        egui::pos2(rect.right(), rect.bottom()),
        egui::Align2::RIGHT_BOTTOM,
        format!("{}{suffix}", format_slider_bound(max)),
        egui::FontId::proportional(9.0),
        TEXT_DIM().gamma_multiply(0.9),
    );
    if response.has_focus() {
        painter.rect_stroke(
            rect.expand(2.0),
            egui::Rounding::same(6.0),
            egui::Stroke::new(1.0_f32, with_alpha(ACCENT(), 0.7)),
        );
    }
    response.on_hover_cursor(if enabled {
        egui::CursorIcon::PointingHand
    } else {
        egui::CursorIcon::Default
    })
}

fn format_slider_bound(value: f64) -> String {
    if (value - value.round()).abs() < 0.005 {
        format!("{}", value.round() as i64)
    } else {
        format!("{value:.2}")
    }
}

/// Une ligne de réglage : le libellé à gauche, le contrôle collé au bord
/// droit de la carte. L'explication du réglage vit dans l'infobulle du
/// libellé, pour que la ligne garde une hauteur constante.
fn settings_row(ui: &mut egui::Ui, label: &str, hint: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.set_min_height(32.0);
        let response = ui.label(egui::RichText::new(label).size(12.5).color(TEXT()));
        if !hint.is_empty() {
            response.on_hover_text(hint);
        }
        // Toute la largeur restante de la ligne : le contrôle est collé au
        // bord droit de la carte, y compris dans le modal Réglages. Borner à
        // 480 px laissait un grand vide entre le libellé et le contrôle.
        let available = ui.available_width();
        let controls_width = if available.is_finite() {
            available.max(0.0)
        } else {
            0.0
        };
        ui.allocate_ui_with_layout(
            egui::vec2(controls_width, 32.0),
            egui::Layout::right_to_left(egui::Align::Center),
            add,
        );
    });
}

fn settings_card(ui: &mut egui::Ui, title: &str, subtitle: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::none()
        .fill(CARD())
        .stroke(egui::Stroke::new(1.0_f32, BORDER()))
        .rounding(egui::Rounding::same(12.0))
        .inner_margin(16.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new(title).size(14.0).strong().color(TEXT()));
            muted(ui, subtitle);
            ui.add_space(9.0);
            // Un trait sépare l'en-tête des réglages : la carte se lit comme
            // une section, pas comme un bloc de texte continu.
            let (line, _) =
                ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
            ui.painter().hline(
                line.x_range(),
                line.center().y,
                egui::Stroke::new(1.0_f32, with_alpha(BORDER(), 0.7)),
            );
            ui.add_space(9.0);
            add(ui);
        });
    ui.add_space(12.0);
}

/// Groupe de contrôles sans cadre, pour un onglet qui n'a qu'une seule
/// famille de réglages : le titre de l'onglet suffit, et les contrôles se
/// posent directement sur le fond de la fenêtre.
fn settings_panel(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    ui.scope(|ui| {
        ui.set_width(ui.available_width());
        add(ui);
    });
    ui.add_space(12.0);
}

/// Verdict sur le daemon qui tourne, face au binaire présent sur le disque.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DaemonState {
    /// Personne n'écoute sur le socket.
    Absent,
    /// Le daemon vient du binaire courant.
    Current,
    /// Le daemon a été lancé avant la dernière compilation.
    Stale,
}

impl DaemonState {
    fn label(self) -> &'static str {
        match self {
            DaemonState::Absent => lapinux_core::i18n::t("settings.system.state_stopped"),
            DaemonState::Current => lapinux_core::i18n::t("settings.system.state_current"),
            DaemonState::Stale => lapinux_core::i18n::t("settings.system.state_stale"),
        }
    }

    fn color(self) -> egui::Color32 {
        match self {
            DaemonState::Absent => TEXT_DIM(),
            DaemonState::Current => OK,
            DaemonState::Stale => WARN,
        }
    }
}

/// Un daemon plus ancien que le binaire efface les réglages qu'il ne connaît
/// pas ; un daemon muet (empreinte zéro) est traité comme ancien.
fn daemon_state(build: Option<&DaemonBuild>, binary: Option<u64>) -> DaemonState {
    let Some(build) = build else {
        return DaemonState::Absent;
    };
    match binary {
        Some(date) if !lapinux_core::client::is_current(build.stamp, date) => DaemonState::Stale,
        // Sans date de binaire, impossible d'affirmer qu'il est ancien.
        _ => DaemonState::Current,
    }
}

/// Date d'un build à partir de son empreinte (millisecondes Unix).
fn build_stamp_label(stamp: u64) -> String {
    if stamp == 0 {
        lapinux_core::i18n::t("settings.system.unknown_date").to_string()
    } else {
        lapinux_core::util::format_datetime((stamp / 1000) as i64)
    }
}

fn status_dot(ui: &mut egui::Ui, color: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 3.5, color);
}

fn meter_color(percent: f32) -> egui::Color32 {
    if percent >= 88.0 {
        DANGER
    } else if percent >= 70.0 {
        WARN
    } else {
        OK
    }
}

/// Manettes connectées dans la barre du haut : une jauge de batterie autour
/// de l'icône de manette par appareil, le nom et l'état au survol. Toutes les
/// manettes en ligne sont représentées, y compris celles sans batterie.
fn topbar_controller_indicators(
    ui: &mut egui::Ui,
    devices: &[DeviceInfo],
    names: &HashMap<String, String>,
    pads: &[lapinux_input::PadState],
) {
    let now = lapinux_core::util::now_unix().max(0) as u64;
    let shown = topbar_controller_devices(devices, pads, now);
    if shown.is_empty() {
        return;
    }
    // Les pastilles sont posées directement dans la rangée de la barre : un
    // `ui.horizontal` imbriqué réserve d'abord sa hauteur minimale puis grandit
    // vers le bas, ce qui les décalait sous les autres blocs de 34 px.
    let spacing = ui.spacing().item_spacing.x;
    ui.spacing_mut().item_spacing.x = 2.0;
    for device in shown {
        topbar_controller_indicator(ui, device, names, pads);
    }
    ui.spacing_mut().item_spacing.x = spacing;
}

/// Manettes en ligne avec une seule icône chacune : la clé de sélection
/// dédoublonne un même appareil vu en USB et en Bluetooth.
fn topbar_controller_devices<'a>(
    devices: &'a [DeviceInfo],
    pads: &[lapinux_input::PadState],
    now: u64,
) -> Vec<&'a DeviceInfo> {
    let mut seen = HashSet::new();
    devices
        .iter()
        .filter(|device| device.gamepad && device.connected)
        // Un dongle muet ne prouve pas qu'une manette est allumée : pas
        // d'icône dans la barre pour un récepteur branché à vide.
        .filter(|device| !dongle_asleep(device, pads, now))
        .filter(|device| seen.insert(controller_key(device)))
        .collect()
}

fn topbar_controller_indicator(
    ui: &mut egui::Ui,
    device: &DeviceInfo,
    names: &HashMap<String, String>,
    pads: &[lapinux_input::PadState],
) {
    let identity = pad_identity(&device);
    let shown = pad_shown_name(names, device, &identity);
    let live = pads
        .iter()
        .find(|pad| pad.path == device.path)
        .and_then(|pad| pad.battery.as_ref());
    let battery = live.and_then(|battery| battery.percent).or(device.battery);
    let state = live
        .map(|battery| battery.state)
        .filter(|state| *state != ChargeState::Unknown)
        .or_else(|| {
            device
                .battery_status
                .as_deref()
                .map(ChargeState::from_sysfs)
        });
    let charging = battery_charging(state);
    // Même gabarit que le bloc CPU/RAM et que les boutons d'écran : 36 px de
    // haut, 36 de large. L'anneau est tracé à 17 px du centre, donc il touche
    // presque le bord de la case et la pastille a la même hauteur que les
    // autres blocs de la barre.
    let (rect, response) = ui.allocate_exact_size(egui::vec2(36.0, 36.0), egui::Sense::hover());
    if charging {
        ui.ctx().request_repaint_after(Duration::from_millis(16));
    }
    let time = ui.input(|input| input.time);
    let painter = ui.painter();
    let center = rect.center();
    if let Some(battery) = battery {
        paint_battery_arc(painter, center, 17.0, battery, 2.0);
        if charging {
            paint_charge_sheen(painter, center, 17.0, battery, 2.0, time);
        }
    } else {
        painter.circle_stroke(
            center,
            17.0,
            egui::Stroke::new(2.0_f32, with_alpha(TEXT_DIM(), 0.22)),
        );
    }
    painter.text(
        center,
        egui::Align2::CENTER_CENTER,
        nav_glyph(Page::Controllers),
        icon_font(19.0),
        TEXT(),
    );
    let battery = battery
        .map(|battery| {
            let band = battery_band_label(Some(band_from_percent(battery)));
            let mut text = lapinux_core::i18n::t_args(
                "controllers.battery.topbar",
                &[("battery", &battery.to_string()), ("band", &band)],
            );
            if let Some(state) = state {
                text.push_str(&format!(" · {}", charge_state_label(state)));
            }
            text
        })
        .unwrap_or_else(|| {
            lapinux_core::i18n::t("controllers.battery.not_exposed_short").to_string()
        });
    let place = match console_label(&identity) {
        Some(console) => format!("{console} · {}", device.connection.label()),
        None => device.connection.label().to_string(),
    };
    response.on_hover_text(format!("{shown}\n{place}\n{battery}"));
}

/// Largeur du bloc d'horloge : elle sert aussi à le centrer dans la barre.
/// L'uptime n'occupant plus la ligne, le bloc reste étroit autour de l'heure.
/// Les huit caractères de `HH:MM:SS` sont à chasse fixe : cette largeur les
/// laisse respirer sans jamais rogner le dernier chiffre.
const TOPBAR_CLOCK_WIDTH: f32 = 144.0;
/// Hauteur du bloc d'horloge, alignée sur les autres commandes de la barre.
const TOPBAR_CLOCK_HEIGHT: f32 = 34.0;

/// Police de l'horloge : tous les chiffres ont la même largeur, donc le
/// galley de `HH:MM:SS` ne change pas de taille d'une seconde à l'autre.
fn topbar_clock_font() -> egui::FontId {
    egui::FontId::monospace(26.0)
}

/// Heure locale au centre de la barre. La durée d'allumage n'est plus une
/// pastille à côté : elle reste disponible dans l'infobulle, au survol de
/// l'heure.
fn topbar_clock(ui: &mut egui::Ui, now: i64, uptime_secs: u64) {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(TOPBAR_CLOCK_WIDTH, TOPBAR_CLOCK_HEIGHT),
        egui::Sense::hover(),
    );
    let time_galley = ui.painter().layout_no_wrap(
        lapinux_core::util::format_time_seconds(now),
        // Chasse fixe : avec les chiffres proportionnels, un « 1 » plus
        // étroit qu'un « 8 » déplaçait le bord gauche de l'heure à chaque
        // seconde, alors que son centre restait immobile.
        topbar_clock_font(),
        TEXT(),
    );
    let time_left = rect.left() + (rect.width() - time_galley.size().x) / 2.0;
    let time_top = rect.center().y - time_galley.size().y / 2.0;
    ui.painter()
        .galley(egui::pos2(time_left, time_top), time_galley, TEXT());
    let uptime = lapinux_core::util::format_uptime(uptime_secs);
    response.on_hover_text(format!(
        "{} {uptime}",
        lapinux_core::i18n::t("settings.system.uptime")
    ));
}

/// Relevé compact CPU / mémoire / GPU affiché dans la barre du haut.
fn performance_panel(ui: &mut egui::Ui, info: &SystemInfo) {
    let cpu_color = meter_color(info.cpu_usage);
    let ram_color = meter_color(info.ram_percent());
    let response = ui
        .allocate_ui_with_layout(
            egui::vec2(282.0, 34.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                egui::Frame::none()
                    .fill(CARD())
                    .stroke(egui::Stroke::new(1.0_f32, BORDER()))
                    .rounding(egui::Rounding::same(8.0))
                    .inner_margin(egui::Margin::symmetric(9.0, 5.0))
                    .show(ui, |ui| {
                        ui.set_width(264.0);
                        ui.spacing_mut().item_spacing.x = 5.0;
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            performance_stat(ui, "CPU", info.cpu_usage, cpu_color, 56.0);
                            performance_divider(ui);
                            performance_stat(ui, "RAM", info.ram_percent(), ram_color, 56.0);
                            performance_divider(ui);
                            gpu_performance_stat(ui, info.gpu_load);
                        });
                    })
                    .response
            },
        )
        .inner;
    let gpu = info
        .active_gpu()
        .map(|gpu| gpu.name.as_str())
        .unwrap_or("Aucune détectée");
    let vram = info
        .gpu_load
        .map(|load| {
            format!(
                "\nVRAM : {} / {}",
                lapinux_core::util::format_gib(load.vram_used),
                lapinux_core::util::format_gib(load.vram_total)
            )
        })
        .unwrap_or_default();
    response.on_hover_text(format!(
        "{} · {} threads\nMémoire : {} / {}\nCarte graphique : {}{}",
        info.machine.cpu_model,
        info.machine.cores,
        lapinux_core::util::format_gib(info.ram_used),
        lapinux_core::util::format_gib(info.ram_total),
        gpu,
        vram
    ));
}

fn performance_divider(ui: &mut egui::Ui) {
    let (divider, _) = ui.allocate_exact_size(egui::vec2(1.0, 24.0), egui::Sense::hover());
    ui.painter().rect_filled(divider, 0.0, BORDER());
}

fn performance_stat(
    ui: &mut egui::Ui,
    label: &str,
    percent: f32,
    color: egui::Color32,
    width: f32,
) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 3.0;
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(label)
                    .size(9.5)
                    .strong()
                    .color(TEXT_DIM()),
            );
            ui.label(
                egui::RichText::new(format!("{percent:.0} %"))
                    .size(12.0)
                    .strong()
                    .color(color),
            );
        });
        let (track, _) = ui.allocate_exact_size(egui::vec2(width, 4.0), egui::Sense::hover());
        ui.painter().rect_filled(
            track,
            egui::Rounding::same(2.0),
            with_alpha(TEXT_DIM(), 0.18),
        );
        let ratio = (percent / 100.0).clamp(0.0, 1.0);
        if ratio > 0.0 {
            let filled = egui::Rect::from_min_size(
                track.min,
                egui::vec2(track.width() * ratio, track.height()),
            );
            ui.painter()
                .rect_filled(filled, egui::Rounding::same(2.0), color);
        }
    });
}

fn gpu_performance_stat(ui: &mut egui::Ui, load: Option<GpuLoad>) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 3.0;
        let (value, color, vram) = match load {
            Some(load) => (
                format!("{:.0} %", load.usage),
                meter_color(load.usage),
                format_vram(load.vram_used, load.vram_total),
            ),
            None => ("n/d".to_string(), TEXT_DIM(), "n/d".to_string()),
        };
        let width = 130.0;
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("GPU")
                    .size(9.5)
                    .strong()
                    .color(TEXT_DIM()),
            );
            ui.label(egui::RichText::new(value).size(12.0).strong().color(color));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(egui::RichText::new(vram).size(9.5).color(TEXT_DIM()));
            });
        });
        let (track, _) = ui.allocate_exact_size(egui::vec2(width, 4.0), egui::Sense::hover());
        ui.painter().rect_filled(
            track,
            egui::Rounding::same(2.0),
            with_alpha(TEXT_DIM(), 0.18),
        );
        let ratio = load
            .map(|load| (load.usage / 100.0).clamp(0.0, 1.0))
            .unwrap_or(0.0);
        if ratio > 0.0 {
            let filled = egui::Rect::from_min_size(
                track.min,
                egui::vec2(track.width() * ratio, track.height()),
            );
            ui.painter()
                .rect_filled(filled, egui::Rounding::same(2.0), color);
        }
    });
}

fn format_vram(used: u64, total: u64) -> String {
    format!(
        "{} / {}",
        lapinux_core::util::format_gib(used),
        lapinux_core::util::format_gib(total)
    )
}

/// Mesure compacte d'une colonne de la carte Système.
fn compact_system_metric(
    ui: &mut egui::Ui,
    icon: &str,
    title: &str,
    value: &str,
    detail: &str,
    meter: Option<(f32, egui::Color32)>,
) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(icon).size(15.0));
        ui.label(egui::RichText::new(title).size(9.5).color(TEXT_DIM()));
    });
    ui.add_space(2.0);
    ui.label(
        egui::RichText::new(lapinux_core::util::ellipsize(value, 27))
            .size(12.5)
            .strong(),
    )
    .on_hover_text(value);
    ui.label(egui::RichText::new(detail).size(9.5).color(TEXT_DIM()));
    if let Some((ratio, color)) = meter {
        ui.add_space(4.0);
        ui.add(
            egui::ProgressBar::new(ratio.clamp(0.0, 1.0))
                .desired_height(3.0)
                .fill(color)
                .text(""),
        );
    }
}

/// QR code de la connexion Steam : une texture par défi, marge blanche
/// comprise et échantillonnage au plus proche pour rester net à toute taille.
struct QrView {
    url: String,
    texture: egui::TextureHandle,
}

/// Matrice du QR sur fond blanc, marge de quatre modules comprise : un
/// scanner ne lit pas un code collé au bord ni dessiné sur un fond sombre.
fn qr_image(url: &str) -> Option<egui::ColorImage> {
    let code = qrcode::QrCode::new(url.as_bytes()).ok()?;
    let modules = code.width();
    let quiet = 4usize;
    let side = modules + quiet * 2;
    let mut image = egui::ColorImage::new([side, side], egui::Color32::WHITE);
    for (index, color) in code.to_colors().iter().enumerate() {
        if matches!(color, qrcode::Color::Dark) {
            let x = index % modules + quiet;
            let y = index / modules + quiet;
            image.pixels[y * side + x] = egui::Color32::BLACK;
        }
    }
    Some(image)
}

fn steam_qr_texture(
    ctx: &egui::Context,
    view: &mut Option<QrView>,
    url: &str,
) -> Option<egui::TextureHandle> {
    if view.as_ref().map(|view| view.url != url).unwrap_or(true) {
        let image = qr_image(url)?;
        // Le nom doit changer avec l'URL : autrement la texture précédente
        // resterait allouée sous le même nom.
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for byte in url.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100_0000_01b3);
        }
        let texture = ctx.load_texture(
            format!("steam-qr-{hash:016x}"),
            image,
            egui::TextureOptions::NEAREST,
        );
        *view = Some(QrView {
            url: url.to_string(),
            texture,
        });
    }
    view.as_ref().map(|view| view.texture.clone())
}

/// Même étiquette pleine et sans contour que les tags Proton.
fn status_pill(ui: &mut egui::Ui, text: &str, color: egui::Color32) {
    chip(ui, text, color);
}

fn primary_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(
            egui::RichText::new(text)
                .size(12.5)
                .strong()
                .color(BACKGROUND()),
        )
        .fill(TEXT())
        .rounding(egui::Rounding::same(8.0))
        .min_size(egui::vec2(0.0, 34.0)),
    )
}

/// État du glisser-déposer d'une liste réordonnable.
#[derive(Clone, Copy, Default)]
struct RowDrag {
    from: Option<usize>,
    over: usize,
}

/// Une phrase pour la section survolée dans l'aperçu.
fn page_note(page: Page) -> &'static str {
    match page {
        Page::Home => lapinux_core::i18n::t("page.note.home"),
        Page::Library => lapinux_core::i18n::t("page.note.library"),
        Page::Collections => lapinux_core::i18n::t("page.note.collections"),
        Page::Downloads => lapinux_core::i18n::t("page.note.downloads"),
        Page::Controllers => lapinux_core::i18n::t("page.note.controllers"),
        Page::Profile => lapinux_core::i18n::t("page.note.profile"),
        Page::Boutiques => lapinux_core::i18n::t("page.note.stores"),
        Page::Captures => lapinux_core::i18n::t("page.note.captures"),
        Page::News => lapinux_core::i18n::t("page.note.news"),
    }
}

/// Un onglet de l'aperçu, dessiné comme dans la barre du haut : son icône
/// puis son libellé, dans une pastille arrondie.
fn paint_topbar_tab(
    painter: &egui::Painter,
    rect: egui::Rect,
    page: Page,
    fill: egui::Color32,
    stroke: egui::Stroke,
    ink: egui::Color32,
) {
    painter.rect(rect, egui::Rounding::same(8.0), fill, stroke);
    let icon = egui::pos2(rect.left() + 8.0 + 9.0, rect.center().y);
    section_icons::page(painter, page, icon, 18.0, ink);
    painter.text(
        egui::pos2(rect.left() + 8.0 + 18.0 + 6.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        page_label(page),
        egui::FontId::proportional(11.5),
        ink,
    );
}

/// Carte « navigation » des réglages : aperçu réordonnable de la barre du
/// haut, masquage réel des sections et choix de la section de démarrage.
/// Renvoie `true` si la configuration a changé.
fn navigation_settings_card(ui: &mut egui::Ui, config: &mut Config) -> bool {
    let mut changed = false;
    settings_card(
        ui,
        lapinux_core::i18n::t("settings.appearance.title"),
        lapinux_core::i18n::t("settings.appearance.hint"),
        |ui| {
            let mut pages = topbar_pages(&config.general.topbar_order);
            let mut hidden = config.general.topbar_hidden.clone();
            if topbar_order(ui, &mut pages, &mut hidden) {
                config.general.topbar_order =
                    pages.iter().map(|page| page.key().to_string()).collect();
                config.general.topbar_hidden = hidden;
                // Masquer la section de départ rendrait le réglage
                // incohérent : on revient au défaut du thème.
                if Page::from_key(&config.general.startup_page).is_some_and(|page| {
                    config
                        .general
                        .topbar_hidden
                        .iter()
                        .any(|key| key == page.key())
                }) {
                    config.general.startup_page.clear();
                }
                changed = true;
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(lapinux_core::i18n::t("settings.appearance.startup_page"));
                let label = Page::from_key(&config.general.startup_page)
                    .filter(|page| {
                        visible_topbar_pages(
                            &config.general.topbar_order,
                            &config.general.topbar_hidden,
                        )
                        .contains(page)
                    })
                    .map(page_label)
                    .unwrap_or_else(|| {
                        lapinux_core::i18n::t("settings.appearance.startup_default")
                    });
                egui::ComboBox::from_id_salt("startup-page")
                    .selected_text(label)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_value(
                                &mut config.general.startup_page,
                                String::new(),
                                lapinux_core::i18n::t("settings.appearance.startup_default"),
                            )
                            .changed()
                        {
                            changed = true;
                        }
                        for page in visible_topbar_pages(
                            &config.general.topbar_order,
                            &config.general.topbar_hidden,
                        ) {
                            if ui
                                .selectable_value(
                                    &mut config.general.startup_page,
                                    page.key().to_string(),
                                    page_label(page),
                                )
                                .changed()
                            {
                                changed = true;
                            }
                        }
                    });
            });
            ui.add_space(4.0);
            muted(
                ui,
                lapinux_core::i18n::t("settings.appearance.settings_control_note"),
            );
        },
    );
    changed
}

/// Aperçu horizontal de la barre du haut, réordonnable par glisser-déposer :
/// les onglets se déplacent dans la ligne, comme dans la vraie barre. Renvoie
/// `true` si l'ordre a changé.
fn topbar_order(ui: &mut egui::Ui, pages: &mut Vec<Page>, hidden: &mut Vec<String>) -> bool {
    let id = ui.make_persistent_id("topbar-order");
    let mut drag: RowDrag = ui.data_mut(|data| *data.get_temp_mut_or_default(id));
    let pointer = ui.ctx().pointer_interact_pos();
    let pressed = ui.input(|input| input.pointer.any_pressed());
    let released = ui.input(|input| input.pointer.any_released());
    let mut hovered_page: Option<Page> = None;
    let mut changed = false;
    let visible_count = pages
        .iter()
        .filter(|page| !hidden.iter().any(|key| key == page.key()))
        .count();
    let mut tabs: Vec<(Page, egui::Rect, egui::Response, usize)> = Vec::new();

    egui::Frame::none()
        .fill(PANEL())
        .stroke(egui::Stroke::new(1.0_f32, BORDER()))
        .rounding(egui::Rounding::same(10.0))
        .inner_margin(egui::Margin::symmetric(7.0, 7.0))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
            ui.horizontal_wrapped(|ui| {
                for (page_index, page) in pages.iter().copied().enumerate() {
                    if hidden.iter().any(|key| key == page.key()) {
                        continue;
                    }
                    let label = page_label(page);
                    let text_width = ui
                        .painter()
                        .layout_no_wrap(label.to_owned(), egui::FontId::proportional(11.5), TEXT())
                        .size()
                        .x;
                    let remove_width = 20.0;
                    let width = 8.0 + 18.0 + 6.0 + text_width + 9.0 + remove_width;
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(width, 34.0), egui::Sense::hover());
                    let divider_x = rect.right() - remove_width;
                    let remove_rect = egui::Rect::from_center_size(
                        egui::pos2(divider_x + remove_width / 2.0, rect.center().y),
                        egui::vec2(16.0, 16.0),
                    );
                    let drag_rect = egui::Rect::from_min_max(
                        rect.min,
                        egui::pos2(divider_x - 1.0, rect.bottom()),
                    );
                    let drag_response = ui
                        .interact(
                            drag_rect,
                            id.with(("drag", page.key())),
                            egui::Sense::click_and_drag(),
                        )
                        .on_hover_cursor(if drag.from.is_some() {
                            egui::CursorIcon::Grabbing
                        } else {
                            egui::CursorIcon::Grab
                        });
                    let remove_enabled = visible_count > 1;
                    let remove_response = ui
                        .interact(
                            remove_rect,
                            id.with(("remove", page.key())),
                            if remove_enabled {
                                egui::Sense::click()
                            } else {
                                egui::Sense::hover()
                            },
                        )
                        .on_hover_cursor(if remove_enabled {
                            egui::CursorIcon::PointingHand
                        } else {
                            egui::CursorIcon::NotAllowed
                        })
                        .on_hover_text(if remove_enabled {
                            lapinux_core::i18n::t("settings.appearance.hide_section")
                        } else {
                            lapinux_core::i18n::t("settings.appearance.keep_one_section")
                        });
                    if drag_response.hovered() || remove_response.hovered() {
                        hovered_page = Some(page);
                    }
                    if pressed
                        && drag.from.is_none()
                        && pointer.is_some_and(|position| drag_rect.contains(position))
                    {
                        drag.from = Some(tabs.len());
                        drag.over = tabs.len();
                    }
                    if remove_response.clicked() && remove_enabled {
                        hidden.push(page.key().to_string());
                        changed = true;
                    }
                    tabs.push((page, rect, drag_response, page_index));
                }
            });

            // La cible est l'onglet visible le plus proche du pointeur.
            if drag.from.is_some() {
                if let Some(position) = pointer {
                    let mut best = drag.over.min(tabs.len().saturating_sub(1));
                    let mut best_distance = f32::INFINITY;
                    for (index, (_, rect, _, _)) in tabs.iter().enumerate() {
                        let distance = rect.center().distance_sq(position);
                        if distance < best_distance {
                            best_distance = distance;
                            best = index;
                        }
                    }
                    drag.over = best;
                }
            }

            for (index, (page, rect, response, _)) in tabs.iter().enumerate() {
                let lifted = drag.from == Some(index);
                let target = drag.from.is_some_and(|from| from != index) && drag.over == index;
                let fill = if lifted {
                    with_alpha(CARD(), 0.35)
                } else if target {
                    CARD_ACTIVE()
                } else if response.hovered() {
                    CARD_HOVER()
                } else {
                    CARD()
                };
                let stroke = if target {
                    egui::Stroke::new(1.4_f32, ACCENT())
                } else if lifted {
                    egui::Stroke::new(1.0_f32, with_alpha(BORDER(), 0.5))
                } else {
                    egui::Stroke::new(1.0_f32, BORDER())
                };
                paint_topbar_tab(
                    ui.painter(),
                    *rect,
                    *page,
                    fill,
                    stroke,
                    if lifted { TEXT_DIM() } else { TEXT() },
                );
                let divider_x = rect.right() - 20.0;
                ui.painter().line_segment(
                    [
                        egui::pos2(divider_x, rect.top() + 8.0),
                        egui::pos2(divider_x, rect.bottom() - 8.0),
                    ],
                    egui::Stroke::new(1.0_f32, with_alpha(BORDER(), 0.7)),
                );
                let remove_enabled = visible_count > 1;
                let remove_rect = egui::Rect::from_center_size(
                    egui::pos2(divider_x + 10.0, rect.center().y),
                    egui::vec2(16.0, 16.0),
                );
                let remove_ink = if !remove_enabled {
                    with_alpha(TEXT_DIM(), 0.35)
                } else if remove_rect.contains(pointer.unwrap_or(egui::Pos2::ZERO)) {
                    ACCENT()
                } else {
                    TEXT_DIM()
                };
                if remove_enabled && remove_rect.contains(pointer.unwrap_or(egui::Pos2::ZERO)) {
                    ui.painter()
                        .circle_filled(remove_rect.center(), 8.0, CARD_ACTIVE());
                }
                ui.painter().text(
                    remove_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "×",
                    egui::FontId::proportional(13.0),
                    remove_ink,
                );
            }

            // L'onglet soulevé suit le pointeur, avec une ombre légère.
            if let (Some(from), Some(position)) = (drag.from, pointer) {
                if let Some((page, rect, _, _)) = tabs.get(from) {
                    let float = egui::Rect::from_center_size(position, rect.size());
                    ui.painter().rect_filled(
                        float.translate(egui::vec2(0.0, 2.0)),
                        egui::Rounding::same(8.0),
                        with_alpha(BACKGROUND(), 0.45),
                    );
                    paint_topbar_tab(
                        ui.painter(),
                        float,
                        *page,
                        CARD_ACTIVE(),
                        egui::Stroke::new(1.2_f32, ACCENT()),
                        TEXT(),
                    );
                }
            }

            let hidden_pages: Vec<Page> = pages
                .iter()
                .copied()
                .filter(|page| hidden.iter().any(|key| key == page.key()))
                .collect();
            if !hidden_pages.is_empty() {
                ui.add_space(7.0);
                ui.separator();
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(lapinux_core::i18n::t(
                        "settings.appearance.hidden_sections",
                    ))
                    .size(9.5)
                    .color(TEXT_DIM()),
                );
                ui.add_space(3.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
                    for page in hidden_pages {
                        let text_width = ui
                            .painter()
                            .layout_no_wrap(
                                page_label(page).to_owned(),
                                egui::FontId::proportional(11.0),
                                TEXT_DIM(),
                            )
                            .size()
                            .x;
                        let width = 10.0 + 14.0 + text_width + 8.0;
                        let (rect, response) =
                            ui.allocate_exact_size(egui::vec2(width, 26.0), egui::Sense::click());
                        let hovered = response.hovered();
                        ui.painter().rect(
                            rect,
                            egui::Rounding::same(8.0),
                            if hovered {
                                CARD_HOVER()
                            } else {
                                egui::Color32::TRANSPARENT
                            },
                            egui::Stroke::new(1.0_f32, with_alpha(BORDER(), 0.8)),
                        );
                        ui.painter().text(
                            egui::pos2(rect.left() + 10.0, rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            "+",
                            egui::FontId::proportional(13.0),
                            if hovered { ACCENT() } else { TEXT_DIM() },
                        );
                        ui.painter().text(
                            egui::pos2(rect.left() + 24.0, rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            page_label(page),
                            egui::FontId::proportional(11.0),
                            if hovered { TEXT() } else { TEXT_DIM() },
                        );
                        if response
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .on_hover_text(lapinux_core::i18n::t(
                                "settings.appearance.show_section",
                            ))
                            .clicked()
                        {
                            hidden.retain(|key| key != page.key());
                            changed = true;
                        }
                    }
                });
            }
        });

    ui.add_space(9.0);
    let hint = match hovered_page {
        Some(page) => format!("{} — {}", page_label(page), page_note(page)),
        None => lapinux_core::i18n::t("settings.appearance.drag_hint").to_string(),
    };
    muted(ui, hint);

    if released {
        if let Some(from) = drag.from.take() {
            let to = drag.over.min(tabs.len().saturating_sub(1));
            if from != to && from < tabs.len() && to < tabs.len() {
                let from_page = tabs[from].3;
                let to_page = tabs[to].3;
                let page = pages.remove(from_page);
                // `to_page` est la position d'origine de la cible : après le
                // retrait, insérer à cet index place la section après la cible
                // quand on descend, et avant quand on remonte.
                pages.insert(to_page.min(pages.len()), page);
                changed = true;
            }
        }
    }
    ui.data_mut(|data| data.insert_temp(id, drag));
    changed
}

/// Hauteur commune de tous les contrôles de la barre de filtres : pastilles de
/// source, loupe, dé et filtre d'installation s'alignent sur la même ligne.
const FILTER_H: f32 = 32.0;

/// Largeur d'une cellule d'icône de la même rangée : un peu plus étroite que
/// haute, pour que la barre ne s'étale pas.
const FILTER_W: f32 = 29.0;

/// Pastille segmentée : un seul cadre, des segments séparés par un filet.
/// Renvoie la zone occupée et la réponse de chaque segment, dans l'ordre.
fn segmented_pill(
    ui: &mut egui::Ui,
    widths: &[f32],
    active: &[bool],
    salt: &str,
) -> (egui::Rect, Vec<egui::Response>) {
    segmented_pill_with(ui, widths, active, salt, egui::Rounding::same(9.0))
}

/// Même pastille, avec ses angles choisis par l'appelant : un segment collé à
/// un champ de recherche garde ses angles gauches droits.
fn segmented_pill_with(
    ui: &mut egui::Ui,
    widths: &[f32],
    active: &[bool],
    salt: &str,
    rounding: egui::Rounding,
) -> (egui::Rect, Vec<egui::Response>) {
    let total: f32 = widths.iter().sum();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(total, FILTER_H), egui::Sense::hover());
    let responses = segmented_pill_at(ui, rect, widths, active, salt, rounding);
    (rect, responses)
}

/// Dessine la pastille dans le rectangle donné : la fiche du jeu la centre dans
/// sa largeur, les filtres la posent au fil de la rangée.
fn segmented_pill_at(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    widths: &[f32],
    active: &[bool],
    salt: &str,
    rounding: egui::Rounding,
) -> Vec<egui::Response> {
    let painter = ui.painter().clone();
    painter.rect(rect, rounding, CARD(), egui::Stroke::new(1.0_f32, BORDER()));

    let mut responses = Vec::with_capacity(widths.len());
    let mut left = rect.left();
    for (index, width) in widths.iter().enumerate() {
        let segment = egui::Rect::from_min_size(
            egui::pos2(left, rect.top()),
            egui::vec2(*width, rect.height()),
        );
        let response = ui.interact(
            segment,
            ui.id().with(("filter", salt, index)),
            egui::Sense::click(),
        );
        let selected = active.get(index).copied().unwrap_or(false);
        let inset = segment.shrink(3.0);
        if selected {
            painter.rect_filled(inset, egui::Rounding::same(6.0), CARD_ACTIVE());
        } else if response.hovered() {
            painter.rect_filled(inset, egui::Rounding::same(6.0), CARD_HOVER());
        }
        left += *width;
        responses.push(response.on_hover_cursor(egui::CursorIcon::PointingHand));
    }

    // Filets verticaux entre les segments.
    let mut separator = rect.left();
    for width in widths.iter().take(widths.len().saturating_sub(1)) {
        separator += *width;
        painter.line_segment(
            [
                egui::pos2(separator + 0.5, rect.top() + 8.0),
                egui::pos2(separator + 0.5, rect.bottom() - 8.0),
            ],
            egui::Stroke::new(1.0_f32, BORDER()),
        );
    }

    responses
}

/// Pastille d'outils de la Bibliothèque : la relecture et les réglages dans le
/// même cadre, comme les sélections voisines.
///
/// Pendant une synchronisation, la flèche tourne sur place : aucun mot, aucune
/// pastille de plus sur la ligne. Ce qu'elle fait et où elle en est se lit dans
/// son infobulle.
fn library_icon_group(
    ui: &mut egui::Ui,
    syncing: Option<Instant>,
    refresh_hint: &str,
    gear_hint: &str,
) -> Vec<egui::Response> {
    let widths = [FILTER_W; 2];
    let active = [false; 2];
    let (rect, responses) = segmented_pill(ui, &widths, &active, "library-filter-icons");

    let refresh = segment_rect(rect, &widths, 0);
    let refresh_ink = if responses[0].hovered() {
        TEXT()
    } else {
        TEXT_DIM()
    };
    match syncing {
        Some(started) => {
            let turn = started.elapsed().as_secs_f32() * 3.2;
            paint_refresh_spinner(ui.painter(), refresh.center(), turn, refresh_ink);
        }
        None => paint_card_glyph(
            ui.painter(),
            refresh.center(),
            CardGlyph::Refresh,
            egui::Stroke::new(1.4_f32, refresh_ink),
            refresh_ink,
        ),
    }

    let gear = segment_rect(rect, &widths, 1);
    let gear_ink = if responses[1].hovered() {
        TEXT()
    } else {
        TEXT_DIM()
    };
    paint_card_glyph(
        ui.painter(),
        gear.center(),
        CardGlyph::Gear,
        egui::Stroke::new(1.4_f32, gear_ink),
        gear_ink,
    );

    let hints = [refresh_hint, gear_hint];
    responses
        .into_iter()
        .enumerate()
        .map(|(index, response)| {
            response
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text(hints[index])
        })
        .collect()
}

/// Flèche de rafraîchissement en rotation : le tracé de l'icône fixe, tourné
/// autour du centre.
fn paint_refresh_spinner(
    painter: &egui::Painter,
    center: egui::Pos2,
    turn: f32,
    ink: egui::Color32,
) {
    let rotate = |point: egui::Pos2| {
        let offset = point - center;
        let (sin, cos) = turn.sin_cos();
        center
            + egui::vec2(
                offset.x * cos - offset.y * sin,
                offset.x * sin + offset.y * cos,
            )
    };
    let radius = 5.6;
    let start = -0.6;
    let sweep = std::f32::consts::TAU * 0.62;
    let arc: Vec<egui::Pos2> = (0..=26)
        .map(|index| {
            let angle = start + index as f32 / 26.0 * sweep;
            rotate(center + egui::vec2(angle.cos(), angle.sin()) * radius)
        })
        .collect();
    // La pointe se pose sur le bout du trait *avant* rotation : ajouter le
    // décalage à un point déjà tourné, puis tourner le tout, la faisait
    // tourner deux fois plus vite que l'arc.
    let end = start + sweep;
    let tip = center + egui::vec2(end.cos(), end.sin()) * radius;
    painter.add(egui::Shape::line(arc, egui::Stroke::new(1.5_f32, ink)));
    painter.add(egui::Shape::convex_polygon(
        vec![
            rotate(tip + egui::vec2(-1.3, -3.6)),
            rotate(tip + egui::vec2(3.2, 0.2)),
            rotate(tip + egui::vec2(-2.6, 2.6)),
        ],
        ink,
        egui::Stroke::NONE,
    ));
}

/// Segment cliquable centré dans une pastille.
/// Déplacer ou redimensionner la fenêtre confie le pointeur au compositeur :
/// l'événement de relâchement ne revient jamais à egui. Sans cette remise à
/// zéro, le bouton resterait « enfoncé » et il faudrait cliquer une fois dans
/// la fenêtre avant de pouvoir la déplacer de nouveau.
fn release_pointer_after_window_command(ctx: &egui::Context) {
    ctx.stop_dragging();
    ctx.input_mut(|input| input.pointer = egui::PointerState::default());
}

/// Faits de la fiche qui tiennent sur une ligne, sous les succès : l'âge
/// recommandé, qui développe et publie, le prix. Rien de ce que la fiche
/// n'aurait pas lu : chaque valeur absente disparaît plutôt que d'être
/// inventée.
/// Faits d'un jeu de boutique, dans la bande de tags : ce que la boutique
/// publie vraiment — sa marque, la version proposée, le poids annoncé.
fn store_facts(
    game: &lapinux_core::stores::StoreGame,
    details: Option<&lapinux_core::stores::StoreDetails>,
    // Poids publié pour ce jeu, quand la boutique le donne.
    size: Option<u64>,
    // Version retenue dans le menu du bouton, si l'utilisateur en a choisi une.
    chosen: Option<&str>,
    // Protection lue dans le dossier, pour un jeu installé.
    drm: Option<&drm::Report>,
) -> Vec<(&'static str, String)> {
    let mut facts: Vec<(&'static str, String)> = Vec::new();
    // Les versions mobiles ouvrent la bande : sur une fiche étroite, les
    // derniers tags passent dans le « + N » et ces badges seraient cachés.
    for platform in &game.mobile_platforms {
        facts.push(("", crate::stores::platform_label(platform).to_string()));
    }
    let mobile_only =
        game.platforms.is_empty() && (game.mobile || !game.mobile_platforms.is_empty());
    if mobile_only {
        facts.push((
            "",
            lapinux_core::i18n::t("game_sheet.mobile_not_installable").to_string(),
        ));
    }
    if let Some(details) = details {
        if !details.developer.trim().is_empty() {
            facts.push((
                lapinux_core::i18n::t("game_sheet.fact_developer"),
                details.developer.clone(),
            ));
        }
        if !details.publisher.trim().is_empty()
            && !details.publisher.eq_ignore_ascii_case(&details.developer)
        {
            facts.push((
                lapinux_core::i18n::t("game_sheet.fact_publisher"),
                details.publisher.clone(),
            ));
        }
        if !details.released.trim().is_empty() {
            facts.push((
                lapinux_core::i18n::t("game_sheet.fact_release"),
                details.released.clone(),
            ));
        }
    }
    facts.push((
        lapinux_core::i18n::t("game_sheet.fact_store"),
        game.store.label().to_string(),
    ));
    // La protection lue dans le dossier, pas ce que la boutique annonce : un
    // dossier absent ou illisible ne produit rien plutôt qu'un « sans DRM » de
    // complaisance.
    if let Some(report) = drm {
        match report.drm {
            Some(name) => facts.push(("DRM", name.to_string())),
            None => facts.push(("", "Sans DRM".to_string())),
        }
        match report.client {
            Some(name) => facts.push(("Client", name.to_string())),
            None => facts.push(("", "Sans client".to_string())),
        }
    }
    // La version affichée est celle que le bouton installera : le choix du
    // menu d'abord, puis Windows, macOS, Linux. L'ordre brut de la boutique
    // mettait « macOS » sur un jeu qui n'a qu'un build Windows.
    let platform = chosen
        .filter(|chosen| {
            game.platforms
                .iter()
                .any(|platform| platform.eq_ignore_ascii_case(chosen))
        })
        .map(str::to_string)
        .or_else(|| {
            ["windows", "macos", "linux"]
                .into_iter()
                .find(|key| {
                    game.platforms
                        .iter()
                        .any(|platform| platform.eq_ignore_ascii_case(key))
                })
                .map(str::to_string)
        })
        .or_else(|| game.platforms.first().cloned());
    if let Some(platform) = platform {
        facts.push((
            "Version",
            crate::stores::platform_label(&platform).to_string(),
        ));
    }
    if let Some(size) = size.filter(|size| *size > 0) {
        facts.push(("Poids", lapinux_core::util::format_bytes(size)));
    }
    // Le temps de jeu que la boutique publie : Epic le donne, les autres non.
    if let Some(minutes) = game.minutes.filter(|minutes| *minutes > 0) {
        facts.push((
            "Temps de jeu",
            lapinux_core::util::format_duration(minutes.saturating_mul(60)),
        ));
    }
    match details.and_then(|details| details.cloud) {
        Some(true) => facts.push(("Cloud", lapinux_core::i18n::t("game_sheet.enabled").into())),
        Some(false) => facts.push(("Cloud", "non proposé".into())),
        None => {}
    }
    facts
}

/// Corps déplié d'un jeu de boutique, dans les trois mêmes colonnes que la
/// fiche Steam : les annonces puis le compte à gauche, la présentation au
/// centre, la bande-annonce et les captures à droite. Les encarts sont ceux de
/// Steam — même dessin, même place, même comportement.
#[allow(clippy::too_many_arguments)]
fn store_sheet_body(
    ui: &mut egui::Ui,
    info: &mut crate::gameinfo::Shelf,
    bunny: Option<&crate::launch::Bunny>,
    game: &lapinux_core::stores::StoreGame,
    details: Option<&lapinux_core::stores::StoreDetails>,
    waiting: bool,
) {
    let content =
        details.map(|details| crate::gameinfo::SheetContent::from_store_details(Some(details)));
    let page = content.as_ref();
    // La page du jeu chez la boutique : c'est elle que l'affiche de la
    // bande-annonce ouvre, faute de lecteur vidéo dans l'interface.
    let site = details
        .map(|details| details.site.trim().to_string())
        .unwrap_or_default();
    egui::ScrollArea::vertical()
        .id_salt(("store-sheet", game.appid))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let width = ui.available_width();
            let (side, center, media) = sheet_body_columns(width);
            // Sous 860 px, les encarts s'empilent dans l'ordre où ils se
            // lisent, comme sur une fiche Steam.
            if side <= 0.0 {
                if info.news_page(ui, page, waiting, bunny) {
                    ui.add_space(14.0);
                }
                // La fiche boutique n'ajoute pas de bannière au-dessus d'« À
                // propos » : sa jaquette est déjà dans l'encart d'identité.
                if info.description_page(ui, page, ("store-sheet", game.appid)) {
                    ui.add_space(12.0);
                }
                info.media_page(
                    ui,
                    page,
                    &site,
                    "Ouvrir la fiche du jeu chez la boutique",
                    width,
                );
                ui.add_space(14.0);
                return;
            }
            ui.horizontal_top(|ui| {
                let gap = sheet_column_gap(ui.spacing().item_spacing.x);
                ui.allocate_ui_with_layout(
                    egui::vec2(side, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(side);
                        if info.news_page(ui, page, waiting, bunny) {
                            ui.add_space(14.0);
                        }
                    },
                );
                ui.add_space(gap);
                ui.allocate_ui_with_layout(
                    egui::vec2(center, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(center);
                        // Pas de bannière ici : « À propos » commence par le
                        // texte, comme dans la fiche installée d'un jeu.
                        info.description_page(ui, page, ("store-sheet", game.appid));
                    },
                );
                if media > 0.0 {
                    ui.add_space(gap);
                    ui.allocate_ui_with_layout(
                        egui::vec2(media, 0.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_width(media);
                            info.media_page(
                                ui,
                                page,
                                &site,
                                "Ouvrir la fiche du jeu chez la boutique",
                                media,
                            );
                        },
                    );
                }
            });
        });
}

/// Onglet Sauvegardes d'un jeu de boutique : ce que sa boutique annonce, puis
/// les copies locales quand le jeu est installé.
fn store_sheet_saves_tab(
    ui: &mut egui::Ui,
    saves: &mut crate::saves::Panel,
    client: &Client,
    game: &lapinux_core::stores::StoreGame,
    details: Option<&lapinux_core::stores::StoreDetails>,
    relaunch: bool,
) {
    let ctx = ui.ctx().clone();
    egui::ScrollArea::vertical()
        .id_salt(("store-saves", game.appid))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let width = ui.available_width();
            let inner = width.min(1120.0);
            let left = ((width - inner) / 2.0).max(0.0);
            ui.horizontal_top(|ui| {
                ui.add_space(left);
                ui.allocate_ui_with_layout(
                    egui::vec2(inner, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(inner);
                        if !game.installed {
                            store_sheet_saves_card(
                                ui,
                                game,
                                details.and_then(|details| details.cloud),
                            );
                            return;
                        }
                        saves.slots_card(ui, &ctx, client, game.appid, game.installed, relaunch);
                        if inner >= 880.0 {
                            let gap = 16.0;
                            let column = (inner - gap) / 2.0;
                            ui.horizontal_top(|ui| {
                                ui.allocate_ui_with_layout(
                                    egui::vec2(column, 0.0),
                                    egui::Layout::top_down(egui::Align::Min),
                                    |ui| {
                                        ui.set_width(column);
                                        saves.card(
                                            ui,
                                            &ctx,
                                            client,
                                            game.appid,
                                            game.installed,
                                            None::<fn(&mut egui::Ui)>,
                                        );
                                    },
                                );
                                ui.add_space(gap);
                                ui.allocate_ui_with_layout(
                                    egui::vec2(column, 0.0),
                                    egui::Layout::top_down(egui::Align::Min),
                                    |ui| {
                                        ui.set_width(column);
                                        store_sheet_saves_card(
                                            ui,
                                            game,
                                            details.and_then(|details| details.cloud),
                                        );
                                    },
                                );
                            });
                        } else {
                            saves.card(
                                ui,
                                &ctx,
                                client,
                                game.appid,
                                game.installed,
                                None::<fn(&mut egui::Ui)>,
                            );
                            ui.add_space(14.0);
                            store_sheet_saves_card(
                                ui,
                                game,
                                details.and_then(|details| details.cloud),
                            );
                        }
                    },
                );
            });
            ui.add_space(12.0);
        });
}

/// Encart « Détails » d'un jeu de boutique : ce qu'elle publie, dans la même
/// grille que les détails Steam. Partagé par la fiche d'un jeu de boutique et
/// par l'onglet Détails d'un jeu de boutique installé.
fn store_details_card(
    ui: &mut egui::Ui,
    store: lapinux_core::stores::Store,
    platforms: &[String],
    tool: bool,
    // Contenus additionnels rattachés au jeu par la boutique.
    contents: usize,
    details: Option<&lapinux_core::stores::StoreDetails>,
    size: Option<u64>,
    chosen: Option<&str>,
) {
    let mut rows: Vec<(&str, String)> = Vec::new();
    if let Some(details) = details {
        if !details.developer.trim().is_empty() {
            rows.push((
                lapinux_core::i18n::t("game_sheet.fact_developer"),
                details.developer.clone(),
            ));
        }
        if !details.publisher.trim().is_empty() {
            rows.push((
                lapinux_core::i18n::t("game_sheet.fact_publisher"),
                details.publisher.clone(),
            ));
        }
        if !details.released.trim().is_empty() {
            rows.push((
                lapinux_core::i18n::t("game_sheet.fact_release"),
                details.released.clone(),
            ));
        }
        if !details.genres.is_empty() {
            rows.push(("Genres", details.genres.join(" · ")));
        }
        if !details.site.trim().is_empty() {
            rows.push(("Page", details.site.clone()));
        }
        match details.cloud {
            Some(true) => rows.push(("Cloud", lapinux_core::i18n::t("game_sheet.enabled").into())),
            Some(false) => rows.push(("Cloud", "non proposé".into())),
            None => {}
        }
    }
    rows.push(("Boutique", store.label().to_string()));
    if !platforms.is_empty() {
        rows.push((
            "Plateformes",
            platforms
                .iter()
                .map(|platform| crate::stores::platform_label(platform).to_string())
                .collect::<Vec<_>>()
                .join(" · "),
        ));
    }
    if let Some(platform) = chosen {
        rows.push((
            "Version",
            crate::stores::platform_label(platform).to_string(),
        ));
    }
    if let Some(size) = size.filter(|size| *size > 0) {
        rows.push(("Poids", lapinux_core::util::format_bytes(size)));
    }
    if contents > 0 {
        rows.push((
            "DLC",
            match contents {
                1 => "1 DLC".to_string(),
                count => format!("{count} DLC"),
            },
        ));
    }
    if tool {
        rows.push(("Type", "Outil".into()));
    }
    settings_card(
        ui,
        lapinux_core::i18n::t("configurations.details"),
        "Ce que la boutique publie pour ce jeu.",
        |ui| {
            if rows.is_empty() {
                muted(ui, "La boutique ne publie pas de détails.");
                return;
            }
            egui::Grid::new(("store-details-grid", store as u8, size.unwrap_or(0)))
                .num_columns(2)
                .spacing([12.0, 5.0])
                .show(ui, |ui| {
                    for (label, value) in &rows {
                        ui.label(egui::RichText::new(*label).size(11.0).color(TEXT_DIM()));
                        ui.label(egui::RichText::new(value).size(11.0).color(TEXT()));
                        ui.end_row();
                    }
                });
        },
    );
}

/// Onglet Détails d'un jeu de boutique : ce qu'elle publie (développeur,
/// éditeur, sortie, genres, plateformes, poids, cloud), puis son compte et ses
/// capacités.
fn store_sheet_details_tab(
    ui: &mut egui::Ui,
    media: &mut crate::gameinfo::NewsArt,
    art: &mut ArtCache,
    paths: &LapinuxPaths,
    cover_texture: Option<egui::TextureHandle>,
    game: &lapinux_core::stores::StoreGame,
    account: Option<&lapinux_core::stores::StoreAccount>,
    details: Option<&lapinux_core::stores::StoreDetails>,
    size: Option<u64>,
    chosen: Option<&str>,
) {
    egui::ScrollArea::vertical()
        .id_salt(("store-details", game.appid))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let width = ui.available_width();
            let inner = width.min(780.0);
            let left = ((width - inner) / 2.0).max(0.0);
            ui.horizontal_top(|ui| {
                ui.add_space(left);
                ui.allocate_ui_with_layout(
                    egui::vec2(inner, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(inner);
                        store_base_art_gallery(
                            ui,
                            media,
                            art,
                            paths,
                            game.appid,
                            &game.name,
                            cover_texture.clone(),
                            details,
                            &game.cover,
                        );
                        store_details_card(
                            ui,
                            game.store,
                            &game.platforms,
                            game.tool,
                            game.dlc.len(),
                            details,
                            size,
                            chosen,
                        );
                        store_sheet_store_card(ui, game, account);
                    },
                );
            });
            ui.add_space(12.0);
        });
}

/// Encart de la boutique : la même carte que dans Réglages › Comptes, pour
/// savoir d'où vient le jeu et ce que la boutique sait faire.
fn store_sheet_store_card(
    ui: &mut egui::Ui,
    game: &lapinux_core::stores::StoreGame,
    account: Option<&lapinux_core::stores::StoreAccount>,
) {
    let caps = game.store.capabilities();
    let connected = account.map(|account| account.connected).unwrap_or(false);
    let name = account
        .map(|account| account.account.clone())
        .unwrap_or_default();
    settings_card(ui, game.store.label(), "Compte et capacités", |ui| {
        ui.horizontal(|ui| {
            crate::paint_store_mark(ui, game.store, 38.0, TEXT());
            ui.add_space(8.0);
            status_dot(ui, if connected { OK } else { TEXT_DIM() });
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(if name.trim().is_empty() {
                    game.store.label().to_string()
                } else {
                    name
                })
                .size(13.0)
                .strong(),
            );
        });
        ui.add_space(10.0);
        // Une capacité par ligne : dans cette colonne étroite, quatre
        // pastilles côte à côte coupaient leurs libellés en deux (« Clo ud »).
        let chips: [(&str, bool, &str); 4] = [
            (
                lapinux_core::i18n::t("settings.steam.library"),
                caps.library,
                "L'API de la boutique ne publie pas la bibliothèque.",
            ),
            (
                "Téléchargement",
                caps.install,
                "L'installation n'est pas exposée pour cette boutique.",
            ),
            (
                "Cloud",
                caps.cloud,
                "Cette boutique n'expose pas de sauvegardes cloud.",
            ),
            (
                lapinux_core::i18n::t("gameinfo.achievements"),
                caps.achievements,
                "Cette boutique n'expose pas de succès.",
            ),
        ];
        for (label, available, missing) in chips {
            crate::stores::capability_chip(ui, label, available, missing);
            ui.add_space(4.0);
        }
    });
}

/// Encart des sauvegardes : ce que Lapinux synchronise, et quand.
/// `cloud` est la réponse de la boutique **pour ce jeu** : `None` quand elle
/// ne publie rien, pour ne pas annoncer une synchronisation inventée.
fn store_sheet_saves_card(
    ui: &mut egui::Ui,
    game: &lapinux_core::stores::StoreGame,
    cloud: Option<bool>,
) {
    settings_card(
        ui,
        lapinux_core::i18n::t("sheet_saves.title"),
        lapinux_core::i18n::t("sheet_saves.store_subtitle"),
        |ui| {
            // L'état se lit d'abord : l'encart dit ce que la boutique annonce pour
            // ce jeu, avant d'expliquer comment la synchronisation marche.
            let (dot, label) = match cloud {
                Some(true) => (OK, "Synchronisation activée"),
                Some(false) => (TEXT_DIM(), "Le jeu n'annonce pas de cloud"),
                None => (TEXT_DIM(), "Cloud non annoncé par la boutique"),
            };
            ui.horizontal(|ui| {
                status_dot(ui, dot);
                ui.add_space(2.0);
                ui.label(egui::RichText::new(label).size(12.0).strong().color(dot));
            });
            ui.add_space(6.0);
            muted(
            ui,
            match cloud {
                Some(true) => "Les fichiers de sauvegarde sont comparés au cloud au lancement et après la partie : le plus récent gagne, et rien n'est envoyé sans que le jeu se ferme.",
                Some(false) => "La boutique publie les fonctions de ce jeu et les sauvegardes cloud n'en font pas partie : Lapinux laisse les fichiers sur le disque.",
                None => "La boutique ne dit pas si ce jeu utilise le cloud. Lapinux ne synchronise que lorsqu'un dossier de sauvegarde est défini dans les réglages du jeu.",
            },
        );
            if cloud != Some(false) && game.installed {
                ui.add_space(10.0);
                muted(
                ui,
                "Dossier de sauvegarde : à choisir dans les réglages du jeu s'il n'est pas encore deviné.",
            );
            }
        },
    );
}

fn sheet_facts(
    details: &SteamDetails,
    info: Option<&SteamGameInfo>,
    // Sauvegardes cloud du compte pour ce jeu, quand elles ont été lues :
    // leur nombre, leur date et l'état de la copie locale.
    cloud: Option<&lapinux_core::model::CloudReport>,
) -> Vec<(&'static str, String)> {
    let mut facts: Vec<(&'static str, String)> = Vec::new();
    if let Some(age) = info.and_then(|info| info.update.ratings.first()) {
        facts.push((lapinux_core::i18n::t("game_sheet.fact_age"), age.clone()));
    }
    if let Some(developers) = details.developers.first() {
        facts.push((
            lapinux_core::i18n::t("game_sheet.fact_developer"),
            developers.clone(),
        ));
    }
    if let Some(publishers) = details
        .publishers
        .iter()
        .find(|publisher| !details.developers.contains(publisher))
    {
        facts.push((
            lapinux_core::i18n::t("game_sheet.fact_publisher"),
            publishers.clone(),
        ));
    }
    match info
        .map(|info| info.store.price.as_str())
        .filter(|price| !price.is_empty())
    {
        Some(price) => facts.push((
            lapinux_core::i18n::t("game_sheet.fact_price"),
            price.to_string(),
        )),
        None if details.free => facts.push((
            lapinux_core::i18n::t("game_sheet.fact_price"),
            lapinux_core::i18n::t("game_sheet.fact_free").into(),
        )),
        None => {}
    }
    if let Some(players) = info.and_then(|info| info.account.players_now) {
        facts.push((
            lapinux_core::i18n::t("game_sheet.fact_players"),
            grouped(players),
        ));
    }
    if let Some(votes) =
        info.and_then(|info| info.store.review_count.or(info.store.recommendations))
    {
        facts.push((
            lapinux_core::i18n::t("game_sheet.fact_reviews"),
            grouped(votes),
        ));
    }
    if let Some(percent) = details.review_percent {
        facts.push((
            lapinux_core::i18n::t("game_sheet.fact_positive"),
            format!("{percent} %"),
        ));
    }
    if let Some(date) = details.release_date.filter(|date| *date > 0) {
        // Le jour précis d'une sortie ne se lit pas ici : l'année suffit, le
        // bloc Détails Steam donne la date entière.
        let year = lapinux_core::util::format_date(date);
        facts.push((
            lapinux_core::i18n::t("game_sheet.fact_release"),
            year.rsplit('/').next().unwrap_or(&year).to_string(),
        ));
    }
    match cloud {
        Some(report) if report.saves.has_saves() => {
            let saves = &report.saves;
            let mut value = format!(
                "{} · {}",
                lapinux_core::i18n::t_fmt(
                    "sheet_saves.files_count",
                    &[saves.files.len().to_string()]
                ),
                cloud_sync_label(report)
            );
            let synced = report
                .syncs
                .first()
                .map(|entry| {
                    let origin = match entry.direction {
                        lapinux_core::model::CloudSyncDirection::Upload => {
                            lapinux_core::i18n::t_args(
                                "game_sheet.sync_from",
                                &[("machine", &lapinux_core::util::ellipsize(&entry.machine, 10))],
                            )
                        }
                        lapinux_core::model::CloudSyncDirection::Download => {
                            lapinux_core::i18n::t_args(
                                "game_sheet.sync_to",
                                &[("machine", &lapinux_core::util::ellipsize(&entry.machine, 12))],
                            )
                        }
                    };
                    format!("{} · {origin}", lapinux_core::util::format_date(entry.at))
                })
                .or_else(|| {
                    saves.latest_local_file().map(|(_, local)| {
                        lapinux_core::i18n::t_fmt(
                            "saves.on_this_pc",
                            &[lapinux_core::util::format_date(local)],
                        )
                    })
                })
                .or_else(|| {
                    saves
                        .last_modified()
                        .map(|stamp| format!("{} · cloud", lapinux_core::util::format_date(stamp)))
                });
            if let Some(synced) = synced {
                value.push_str(" · sync ");
                value.push_str(&synced);
            }
            facts.push(("Cloud", value));
        }
        Some(report) if report.error.is_some() => facts.push((
            "Cloud",
            lapinux_core::i18n::t("game_sheet.cloud_unavailable").into(),
        )),
        // Le jeu annonce le cloud mais le compte n'y a rien : le dire vaut
        // mieux que de laisser croire à une sauvegarde.
        Some(_) if cloud_saves(info) => facts.push((
            "Cloud",
            lapinux_core::i18n::t("game_sheet.cloud_no_saves").into(),
        )),
        _ => {}
    }
    facts
}

/// Ce que vaut la copie locale face au cloud : à jour, ou à récupérer quand un
/// fichier manque ou que le compte en a un plus récent.
fn cloud_sync_label(report: &lapinux_core::model::CloudReport) -> &'static str {
    if report.error.is_some() {
        return lapinux_core::i18n::t("sheet_saves.state_unknown");
    }
    if report.saves.files.is_empty() {
        return lapinux_core::i18n::t("sheet_saves.no_backup");
    }
    match report.saves.steam_state() {
        lapinux_core::model::SteamCloudState::UpToDate => {
            lapinux_core::i18n::t("protons.up_to_date")
        }
        lapinux_core::model::SteamCloudState::Conflict => {
            lapinux_core::i18n::t("sheet_saves.conflict")
        }
        lapinux_core::model::SteamCloudState::MissingLocal => {
            lapinux_core::i18n::t("sheet_saves.to_fetch")
        }
        lapinux_core::model::SteamCloudState::Unknown => {
            lapinux_core::i18n::t("sheet_saves.state_unknown")
        }
    }
}

/// Journal local des synchronisations cloud, sous la carte du jeu.
///
/// La ligne « Dernier envoi » répond directement à la question de la machine
/// d'origine : Steam ne la publie pas, mais Lapinux l'a notée au moment de
/// l'envoi. Les lignes suivantes gardent les derniers gestes automatiques ou
/// manuels, dans les deux sens.
fn cloud_sync_journal(ui: &mut egui::Ui, appid: u32, report: &lapinux_core::model::CloudReport) {
    ui.add_space(8.0);
    egui::CollapsingHeader::new(
        egui::RichText::new("Journal des synchronisations")
            .size(11.0)
            .color(TEXT_DIM()),
    )
    .id_salt(("cloud-journal", appid))
    .default_open(false)
    .show(ui, |ui| {
        if let Some(upload) = report
            .syncs
            .iter()
            .find(|entry| entry.direction == lapinux_core::model::CloudSyncDirection::Upload)
        {
            ui.label(
                egui::RichText::new(format!(
                    "Dernier envoi : {} depuis {}",
                    lapinux_core::util::format_datetime(upload.at),
                    upload.machine
                ))
                .size(10.5)
                .color(TEXT()),
            );
        } else {
            muted(ui, "Aucun envoi enregistré depuis ce PC.");
        }
        if report.syncs.is_empty() {
            muted(ui, "Aucune synchronisation cloud enregistrée.");
        } else {
            for entry in report.syncs.iter().take(4) {
                let action = match entry.direction {
                    lapinux_core::model::CloudSyncDirection::Upload => "envoyé depuis",
                    lapinux_core::model::CloudSyncDirection::Download => "reçu de",
                };
                ui.label(
                    egui::RichText::new(format!(
                        "{} · {action} {} · {} fichier(s) · {}",
                        lapinux_core::util::format_datetime(entry.at),
                        entry.machine,
                        entry.files,
                        lapinux_core::util::format_bytes(entry.bytes)
                    ))
                    .size(10.5)
                    .color(TEXT_DIM()),
                );
            }
        }
        muted(ui, "Journal local, gardé sur ce PC.");
    });
}

/// Nombre lisible : les milliers séparés par une espace fine insécable.
fn grouped(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push('\u{202f}');
        }
        out.push(digit);
    }
    out
}

/// Le jeu synchronise-t-il ses sauvegardes sur le cloud Steam ? La réponse
/// vient des catégories de la boutique, relues par le protocole.
fn cloud_saves(info: Option<&SteamGameInfo>) -> bool {
    info.map(|info| {
        info.store
            .features
            .iter()
            .any(|feature| feature.id == 23 || feature.label.to_lowercase().contains("cloud"))
    })
    .unwrap_or(false)
}

/// Bande de tags sous tous les encarts, sur toute la largeur de la fiche :
/// chaque fait (âge, développeur, éditeur, prix, sortie) y prend son propre
/// rectangle. Ceux qui ne tiennent pas dans la largeur sont annoncés par un
/// dernier tag « + N » dont l'infobulle les nomme.
fn sheet_tags(ui: &mut egui::Ui, facts: &[(&'static str, String)]) {
    const GAP: f32 = 6.0;
    let width = ui.max_rect().width();
    let shown: Vec<(&'static str, String)> = facts
        .iter()
        .map(|(label, value)| {
            // Le tag Cloud rassemble le compte, l'état et la dernière
            // synchronisation : le couper à 30 caractères masquerait
            // justement la date et la provenance.
            let limit = if *label == "Cloud" { 64 } else { 30 };
            (*label, lapinux_core::util::ellipsize(value, limit))
        })
        .collect();
    // Les marges du rectangle (8 de chaque côté) et l'écart entre le libellé
    // et sa valeur : sans eux, la bande se croit plus large qu'elle n'est et
    // se décale vers la gauche.
    let chip_width = |ui: &egui::Ui, label: &str, value: &str| {
        let gap = if label.is_empty() { 0.0 } else { 5.0 };
        16.0 + gap + crate::text_size(ui, label, 9.0).x + crate::text_size(ui, value, 10.5).x
    };
    // Le nombre de tags qui tiennent décide de la largeur de la bande : on la
    // mesure avant de la peindre, pour la centrer dans la fiche. Le « + N »
    // qui ferme la bande compte dans la place.
    let mut shown_count = shown.len();
    let mut total;
    loop {
        total = 0.0;
        for (index, (label, value)) in shown.iter().take(shown_count).enumerate() {
            if index > 0 {
                total += GAP;
            }
            total += chip_width(ui, label, value);
        }
        let rest = shown.len().saturating_sub(shown_count);
        if rest > 0 {
            total += GAP + chip_width(ui, "", &format!("+ {rest}"));
        }
        if total <= width || shown_count == 0 {
            break;
        }
        shown_count -= 1;
    }
    let pad = ((width - total) / 2.0).max(0.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        ui.add_space(pad);
        for (index, (label, value)) in shown.iter().take(shown_count).enumerate() {
            // L'infobulle garde la valeur entière, la puce le texte coupé.
            tag_chip(ui, label, value, &facts[index].1);
        }
        let rest = shown.len().saturating_sub(shown_count);
        if rest > 0 {
            let names = facts[shown_count..]
                .iter()
                .map(|(label, value)| format!("{label} {value}"))
                .collect::<Vec<_>>()
                .join(" · ");
            tag_chip(ui, "", &format!("+ {rest}"), &names);
        }
    });
}

/// Un tag : le libellé en petit, la valeur à sa suite, dans un rectangle
/// arrondi. L'infobulle porte la valeur entière quand elle a été coupée.
fn tag_chip(ui: &mut egui::Ui, label: &str, value: &str, full: &str) {
    let response = egui::Frame::none()
        .fill(CARD_HOVER())
        .rounding(egui::Rounding::same(7.0))
        .inner_margin(egui::Margin::symmetric(8.0, 4.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                if !label.is_empty() {
                    ui.label(egui::RichText::new(label).size(9.0).color(TEXT_DIM()));
                }
                ui.label(egui::RichText::new(value).size(10.5).color(TEXT()));
            });
        })
        .response;
    response.on_hover_text(full);
}

/// Provenance d'une case de la galerie : un format officiel, un autre visuel
/// de la même fiche, ou une image communautaire SteamGridDB.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ArtSlotSource {
    Published,
    StoreFallback,
    Community,
    Missing,
}

impl ArtSlotSource {
    fn label(self) -> &'static str {
        match self {
            Self::Published => "",
            Self::StoreFallback => "Repli",
            Self::Community => "SGDB",
            Self::Missing => "En attente",
        }
    }

    fn tooltip(self) -> Option<&'static str> {
        match self {
            Self::Published => None,
            Self::StoreFallback => Some(
                "Image de remplacement : ce format n'est pas disponible auprès de la boutique.",
            ),
            Self::Community => {
                Some("Image communautaire SteamGridDB : la boutique ne publie pas ce format.")
            }
            Self::Missing => None,
        }
    }
}

/// Les quatre emplacements restent visibles, même si une source manque.
fn base_art_gallery(ui: &mut egui::Ui, images: &[(Option<egui::TextureHandle>, ArtSlotSource); 4]) {
    settings_card(
        ui,
        "Images du jeu",
        "Les quatre formats utilisés dans Lapinux.",
        |ui| {
            ui.horizontal_wrapped(|ui| {
                for (index, (label, size)) in [
                    ("Icône", egui::vec2(86.0, 86.0)),
                    ("Capsule", egui::vec2(145.0, 86.0)),
                    ("Bannière", egui::vec2(180.0, 86.0)),
                    ("Jaquette", egui::vec2(68.0, 102.0)),
                ]
                .into_iter()
                .enumerate()
                {
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new(label).size(11.0).strong());
                        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
                        ui.painter()
                            .rect_filled(rect, egui::Rounding::same(5.0), CARD_HOVER());
                        if let Some(texture) = &images[index].0 {
                            let target = contain_fit_rect(rect.shrink(2.0), texture.size_vec2());
                            ui.painter().image(
                                texture.id(),
                                target,
                                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                                egui::Color32::WHITE,
                            );
                            if let Some(tooltip) = images[index].1.tooltip() {
                                response.on_hover_text(tooltip);
                            }
                        } else {
                            ui.painter().text(
                                rect.center(),
                                egui::Align2::CENTER_CENTER,
                                "—",
                                egui::FontId::proportional(16.0),
                                TEXT_DIM(),
                            );
                            response.on_hover_text("Image en cours de chargement ou indisponible.");
                        }
                        ui.label(
                            egui::RichText::new(images[index].1.label())
                                .size(9.0)
                                .color(TEXT_DIM()),
                        );
                    });
                }
            });
            muted(ui, "Les visuels officiels passent en premier ; si un format manque, SteamGridDB peut le compléter avec une image communautaire marquée SGDB.");
        },
    );
}

fn steam_art_defaults(appid: u32, details: &SteamDetails) -> SteamDetails {
    let mut details = details.clone();
    if appid != 0 {
        let cdn = format!("https://cdn.cloudflare.steamstatic.com/steam/apps/{appid}");
        if details.header_image.is_empty() {
            details.header_image = format!("{cdn}/header.jpg");
        }
        if details.capsule_image.is_empty() {
            details.capsule_image = format!("{cdn}/capsule_231x87.jpg");
        }
        if details.hero_image.is_empty() {
            details.hero_image = format!("{cdn}/library_hero.jpg");
        }
    }
    details
}

fn steam_base_art_gallery(
    ui: &mut egui::Ui,
    paths: &LapinuxPaths,
    art: &mut ArtCache,
    covers: &mut CoverCache,
    appid: u32,
    name: &str,
    details: &SteamDetails,
) {
    let details = steam_art_defaults(appid, details);
    let details = &details;
    // Une icône reste une icône, jamais le logo du jeu. Quand l'API n'en
    // publie pas, l'icône communautaire de la fiche boutique est demandée.
    let mut icon = if details.icon_image.trim().is_empty() {
        art.game_icon(ui.ctx(), paths, appid)
    } else {
        None
    };
    if icon.is_none() {
        icon = art.get(ui.ctx(), paths, appid, details, SteamArt::Icon, name);
    }
    let capsule = art.get(ui.ctx(), paths, appid, details, SteamArt::Capsule, name);
    let banner = art.get(ui.ctx(), paths, appid, details, SteamArt::Banner, name);
    let cover = covers.get(ui.ctx(), paths, appid, name, None, "", CoverTier::Sheet);
    let fallback = cover
        .clone()
        .or_else(|| capsule.clone())
        .or_else(|| banner.clone())
        .or_else(|| icon.clone());
    let slot = |image: Option<egui::TextureHandle>| match image {
        Some(texture) => (Some(texture), ArtSlotSource::Published),
        None if fallback.is_some() => (fallback.clone(), ArtSlotSource::StoreFallback),
        None => (None, ArtSlotSource::Missing),
    };
    let icon_slot = slot(icon);
    base_art_gallery(ui, &[icon_slot, slot(capsule), slot(banner), slot(cover)]);
}

/// Sources de la boutique par format, suivies de replis du même jeu.
fn store_art_candidates(
    details: Option<&lapinux_core::stores::StoreDetails>,
    cover: &str,
) -> [Vec<String>; 4] {
    let (icon, capsule, banner, portrait) = details
        .map(|d| {
            (
                d.icon.as_str(),
                d.capsule.as_str(),
                d.banner.as_str(),
                d.cover.as_str(),
            )
        })
        .unwrap_or(("", "", "", ""));
    let choices = [
        [icon, portrait, cover, capsule, banner],
        [capsule, banner, cover, portrait, icon],
        [banner, capsule, cover, portrait, icon],
        [portrait, cover, capsule, banner, icon],
    ];
    choices.map(|items| {
        let mut urls = Vec::new();
        for item in items {
            let url = item.trim();
            if url.is_empty() {
                continue;
            }
            let url = if url.starts_with("//") {
                format!("https:{url}")
            } else {
                url.to_string()
            };
            if (url.starts_with("https://") || url.starts_with("http://")) && !urls.contains(&url) {
                urls.push(url);
            }
        }
        urls
    })
}

/// Adresse publiée par la boutique pour un format donné.
fn store_art_published<'a>(
    details: Option<&'a lapinux_core::stores::StoreDetails>,
    index: usize,
) -> &'a str {
    details
        .map(|details| match index {
            0 => details.icon.as_str(),
            1 => details.capsule.as_str(),
            2 => details.banner.as_str(),
            _ => details.cover.as_str(),
        })
        .unwrap_or_default()
}

/// Vrai seulement si la case montre autre chose que le format exact publié :
/// format absent, ou adresse publiée qui n'a pas pu être chargée.
fn store_art_fallback(
    details: Option<&lapinux_core::stores::StoreDetails>,
    index: usize,
    rank: usize,
) -> bool {
    rank > 0 || store_art_published(details, index).trim().is_empty()
}

fn store_base_art_gallery(
    ui: &mut egui::Ui,
    media: &mut crate::gameinfo::NewsArt,
    art: &mut ArtCache,
    paths: &LapinuxPaths,
    appid: u32,
    name: &str,
    cover_texture: Option<egui::TextureHandle>,
    details: Option<&lapinux_core::stores::StoreDetails>,
    cover: &str,
) {
    let candidates = store_art_candidates(details, cover);
    // Les candidats suivants ne sont demandés qu'après échec du précédent.
    let wanted: Vec<String> = candidates
        .iter()
        .filter_map(|urls| {
            urls.iter()
                .find(|url| !media.failed(url))
                .cloned()
                .or_else(|| urls.first().cloned())
        })
        .collect();
    media.ensure(ui.ctx(), &wanted);
    media.poll(ui.ctx());
    let steam_details = SteamDetails::default();
    let images = std::array::from_fn(|index| {
        let selected = candidates[index]
            .iter()
            .enumerate()
            .find_map(|(rank, url)| media.get(url).cloned().map(|texture| (texture, rank)));
        let published = store_art_published(details, index).trim();
        let exact_url = candidates[index].first();
        // Le format exact de la boutique passe avant tout. Tant que son
        // adresse est simplement en cours de chargement, on ne lance pas un
        // téléchargement communautaire concurrent : on attend son échec.
        let exact_missing_or_failed = published.is_empty()
            || exact_url.is_none()
            || exact_url.is_some_and(|url| media.failed(url));
        if exact_missing_or_failed {
            let kind = match index {
                0 => SteamArt::Icon,
                1 => SteamArt::Capsule,
                2 => SteamArt::Banner,
                _ => SteamArt::Cover,
            };
            if let Some(texture) = art.get(ui.ctx(), paths, appid, &steam_details, kind, name) {
                return (Some(texture), ArtSlotSource::Community);
            }
        }
        if let Some((texture, rank)) = selected {
            return (
                Some(texture),
                if store_art_fallback(details, index, rank) {
                    ArtSlotSource::StoreFallback
                } else {
                    ArtSlotSource::Published
                },
            );
        }
        if let Some(texture) = cover_texture.clone() {
            return (Some(texture), ArtSlotSource::StoreFallback);
        }
        (None, ArtSlotSource::Missing)
    });
    base_art_gallery(ui, &images);
}

/// Somme des droits et du téléchargement pour une plateforme.
fn size_line(size: &lapinux_core::model::SteamSize) -> Option<String> {
    if size.is_zero() {
        return None;
    }
    let bytes = lapinux_core::util::format_bytes;
    Some(match (size.install, size.download) {
        (install, 0) => format!("{} installé", bytes(install)),
        (0, download) => {
            lapinux_core::i18n::t_fmt("downloads.to_download", &[bytes(download).to_string()])
        }
        (install, download) => format!(
            "{} installé · {} à télécharger",
            bytes(install),
            bytes(download)
        ),
    })
}

/// Fiche Steam complète : tout ce que le catalogue PICS rapporte (genres,
/// éditeur, dates, langues, succès, illustrations, poids par plateforme…),
/// plus la taille sur disque quand le jeu est installé.
fn steam_details_section(
    ui: &mut egui::Ui,
    paths: &LapinuxPaths,
    art_cache: &mut ArtCache,
    cover_cache: &mut CoverCache,
    appid: u32,
    name: &str,
    details: &SteamDetails,
    installed_size: Option<u64>,
    // Sauvegardes cloud du compte pour ce jeu, lues par le protocole : la
    // liste des fichiers et ce que la dernière partie y a changé.
    cloud: Option<&lapinux_core::model::CloudReport>,
) {
    let mut rows: Vec<(&str, String)> = Vec::new();
    let joined = |list: &[String]| list.join(" · ");
    if !details.developers.is_empty() {
        rows.push(("Développeur", joined(&details.developers)));
    }
    if !details.publishers.is_empty() {
        rows.push(("Éditeur", joined(&details.publishers)));
    }
    let app_type = match details.app_type.as_str() {
        "game" => "Jeu",
        "dlc" => "DLC",
        "demo" => "Démo",
        "tool" => "Outil",
        other => other,
    };
    if !app_type.is_empty() {
        rows.push(("Type", app_type.to_string()));
    }
    if !details.genres.is_empty() {
        rows.push(("Genres", joined(&details.genres)));
    }
    if !details.primary_genre.is_empty()
        && !details
            .genres
            .iter()
            .any(|genre| genre.eq_ignore_ascii_case(&details.primary_genre))
    {
        rows.push(("Genre principal", details.primary_genre.clone()));
    }
    if !details.categories.is_empty() {
        rows.push((
            lapinux_core::i18n::t("gameinfo.features"),
            joined(&details.categories),
        ));
    }
    if !details.descriptors.is_empty() {
        rows.push(("Contenu", joined(&details.descriptors)));
    }
    if let Some(date) = details.release_date {
        rows.push(("Sortie", lapinux_core::util::format_date(date)));
    }
    if let Some(date) = details
        .original_release_date
        .filter(|date| Some(*date) != details.release_date)
    {
        rows.push(("Sortie originale", lapinux_core::util::format_date(date)));
    }
    if !details.platforms.is_empty() {
        rows.push(("Plateformes", joined(&details.platforms)));
    }
    if details.os_arch == "64" {
        rows.push(("Architecture", "64 bits".into()));
    } else if !details.os_arch.is_empty() {
        rows.push(("Architecture", format!("{} bits", details.os_arch)));
    }
    if !details.controller.is_empty() {
        rows.push(("Manette", details.controller.clone()));
    }
    if !details.deck.is_empty() {
        rows.push(("Steam Deck", details.deck.clone()));
    }
    if let Some(score) = details.metacritic {
        rows.push(("Metacritic", score.to_string()));
    }
    if !details.metacritic_url.is_empty() {
        rows.push(("Critiques presse", details.metacritic_url.clone()));
    }
    if let Some(percent) = details.review_percent {
        let value = match details.review_score {
            Some(score) => format!("{percent} % d'avis positifs · {score}/10"),
            None => format!("{percent} % d'avis positifs"),
        };
        rows.push(("Avis Steam", value));
    }
    if !details.languages.is_empty() {
        rows.push(("Langues", joined(&details.languages)));
    }
    if !details.full_audio.is_empty() {
        rows.push(("Audio complet", joined(&details.full_audio)));
    }
    if !details.default_language.is_empty() {
        // La langue que Steam prendrait : c'est elle qui décide du dépôt de
        // doublage téléchargé, donc du poids annoncé plus bas.
        let language = &details.default_language;
        let coverage = if details.full_audio.contains(language) {
            "doublage complet"
        } else if details.languages.contains(language) {
            "sous-titres"
        } else {
            "non proposé par le jeu"
        };
        rows.push(("Langue par défaut", format!("{language} · {coverage}")));
    }
    if details.free {
        rows.push(("Prix", "Gratuit".into()));
    }
    if details.workshop {
        rows.push((
            lapinux_core::i18n::t("game_sheet.steam_workshop"),
            lapinux_core::i18n::t("game_sheet.yes").into(),
        ));
    }
    if details.dlc_count > 0 {
        rows.push(("DLC", details.dlc_count.to_string()));
    }
    if let Some(quota) = details.cloud_quota {
        let value = match details.cloud_files {
            Some(files) => lapinux_core::i18n::t_args(
                "sheet_saves.files_size_count",
                &[
                    ("size", &lapinux_core::util::format_bytes(quota)),
                    ("count", &files.to_string()),
                ],
            ),
            None => lapinux_core::util::format_bytes(quota),
        };
        // Quota publié par le jeu : ce que le cloud peut garder, à comparer
        // avec ce que le compte y a réellement, juste en dessous.
        rows.push(("Cloud Steam (quota)", value));
    }
    if let Some(report) = cloud.filter(|report| report.saves.has_saves()) {
        let saves = &report.saves;
        let mut value = lapinux_core::i18n::t_args(
            "sheet_saves.files_size_count",
            &[
                ("size", &lapinux_core::util::format_bytes(saves.total_bytes())),
                ("count", &saves.files.len().to_string()),
            ],
        );
        if let Some(stamp) = saves.last_modified() {
            value = lapinux_core::i18n::t_args(
                "game_sheet.save_dated",
                &[
                    ("value", &value),
                    ("date", &lapinux_core::util::format_date(stamp)),
                ],
            );
        }
        rows.push((lapinux_core::i18n::t("game_sheet.cloud_saves_row"), value));
        if let Some(diff) = &report.diff {
            // Ce que la partie qui vient de se terminer a changé : la preuve
            // que le cloud a bien reçu la progression.
            rows.push((
                lapinux_core::i18n::t("game_sheet.cloud_after_session"),
                diff.summary(),
            ));
        }
    }
    for (label, size) in [
        ("Taille Windows", details.size_windows.as_ref()),
        ("Taille Linux", details.size_linux.as_ref()),
        ("Taille macOS", details.size_macos.as_ref()),
    ] {
        if let Some(line) = size.and_then(size_line) {
            rows.push((label, line));
        }
    }
    if let Some(size) = installed_size.filter(|size| *size > 0) {
        rows.push(("Taille installée", lapinux_core::util::format_bytes(size)));
    }
    if let Some(build) = details.build_id {
        let value = match details.build_time {
            Some(time) => format!("{} · {}", build, lapinux_core::util::format_date(time)),
            None => build.to_string(),
        };
        rows.push(("Build public", value));
    }
    if !details.branches.is_empty() {
        rows.push(("Branches", joined(&details.branches)));
    }
    if details.depot_count > 0 {
        rows.push(("Dépôts", details.depot_count.to_string()));
    }
    if !details.homepage.is_empty() {
        rows.push(("Site", details.homepage.clone()));
    }
    rows.push(("appid", appid.to_string()));
    steam_base_art_gallery(ui, paths, art_cache, cover_cache, appid, name, details);
    if rows.len() <= 1 {
        ui.add_space(8.0);
        muted(
            ui,
            "Aucune fiche Steam en cache : relancez une synchronisation du catalogue.",
        );
        return;
    }
    egui::CollapsingHeader::new("Fiche Steam")
        .id_salt(("steam-details", appid))
        .default_open(true)
        .show(ui, |ui| {
            egui::Grid::new(("steam-details-grid", appid))
                .num_columns(2)
                .spacing([10.0, 5.0])
                .show(ui, |ui| {
                    for (label, value) in rows {
                        ui.label(egui::RichText::new(label).size(10.5).color(TEXT_DIM()));
                        let text = egui::RichText::new(&value).size(10.5);
                        if value.starts_with("http") {
                            ui.hyperlink_to(text, value);
                        } else {
                            // La grille ne contraint pas la largeur d'une cellule :
                            // sans ce plafond, les longues listes débordent du panneau.
                            let width = ui.available_width();
                            ui.allocate_ui_with_layout(
                                egui::vec2(width, 0.0),
                                egui::Layout::top_down(egui::Align::Min),
                                |ui| {
                                    ui.add(egui::Label::new(text).wrap());
                                },
                            );
                        }
                        ui.end_row();
                    }
                });
        });
}

fn segment_rect(rect: egui::Rect, widths: &[f32], index: usize) -> egui::Rect {
    let left = rect.left() + widths.iter().take(index).sum::<f32>();
    egui::Rect::from_min_size(
        egui::pos2(left, rect.top()),
        egui::vec2(widths[index], rect.height()),
    )
}

/// Demi-ellipse pleine : les épaules d'une silhouette.
fn paint_dome(
    painter: &egui::Painter,
    center: egui::Pos2,
    radius_x: f32,
    radius_y: f32,
    color: egui::Color32,
) {
    const STEPS: usize = 18;
    let points: Vec<egui::Pos2> = (0..=STEPS)
        .map(|step| {
            let angle = std::f32::consts::PI * step as f32 / STEPS as f32;
            center + egui::vec2(angle.cos() * radius_x, angle.sin() * radius_y)
        })
        .collect();
    painter.add(egui::Shape::convex_polygon(
        points,
        color,
        egui::Stroke::NONE,
    ));
}

/// Silhouette d'une personne : tête pleine posée sur des épaules.
fn paint_person_icon(
    painter: &egui::Painter,
    center: egui::Pos2,
    color: egui::Color32,
    scale: f32,
) {
    painter.circle_filled(center + egui::vec2(0.0, -3.6) * scale, 3.1 * scale, color);
    paint_dome(
        painter,
        center + egui::vec2(0.0, 1.2) * scale,
        5.6 * scale,
        4.4 * scale,
        color,
    );
}

/// Emblème d'une plateforme de téléchargement, celui que porte le bouton de
/// version : manchot pour Linux, fenêtres pour Windows, pomme pour macOS.
fn paint_platform_icon(
    painter: &egui::Painter,
    center: egui::Pos2,
    platform: &str,
    color: egui::Color32,
    k: f32,
) {
    match platform {
        "linux" => paint_penguin_icon(painter, center, color, k),
        "windows" => paint_windows_icon(painter, center, color, k),
        "macos" => paint_apple_icon(painter, center, color, k),
        _ => {}
    }
}

/// Manchot au trait : corps, tête, bec plein et deux yeux.
fn paint_penguin_icon(painter: &egui::Painter, center: egui::Pos2, color: egui::Color32, k: f32) {
    let stroke = egui::Stroke::new(1.4 * k, color);
    painter.circle_stroke(center + egui::vec2(0.0, 1.7) * k, 3.4 * k, stroke);
    painter.circle_stroke(center + egui::vec2(0.0, -3.4) * k, 2.4 * k, stroke);
    painter.add(egui::Shape::convex_polygon(
        vec![
            center + egui::vec2(0.3, -2.6) * k,
            center + egui::vec2(2.4, -1.6) * k,
            center + egui::vec2(0.3, -0.6) * k,
        ],
        color,
        egui::Stroke::NONE,
    ));
    for x in [-0.9_f32, 0.9] {
        painter.circle_filled(center + egui::vec2(x, -3.9) * k, 0.55 * k, color);
    }
}

/// Fenêtres : quatre carreaux, comme le logo du système.
fn paint_windows_icon(painter: &egui::Painter, center: egui::Pos2, color: egui::Color32, k: f32) {
    let pane = 5.2 * k;
    let step = pane + 1.2 * k;
    for (dx, dy) in [(-1.0_f32, -1.0_f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        let offset = egui::vec2(dx * step / 2.0, dy * step / 2.0);
        painter.rect_filled(
            egui::Rect::from_center_size(center + offset, egui::vec2(pane, pane)),
            0.5 * k,
            color,
        );
    }
}

/// Pomme : deux lobes, une tige et sa feuille.
fn paint_apple_icon(painter: &egui::Painter, center: egui::Pos2, color: egui::Color32, k: f32) {
    painter.circle_filled(center + egui::vec2(-2.1, 1.2) * k, 4.0 * k, color);
    painter.circle_filled(center + egui::vec2(2.1, 1.2) * k, 4.0 * k, color);
    painter.line_segment(
        [
            center + egui::vec2(0.0, -2.6) * k,
            center + egui::vec2(0.9, -6.0) * k,
        ],
        egui::Stroke::new(1.4 * k, color),
    );
    let leaf: Vec<egui::Pos2> = (0..=14)
        .map(|index| {
            let angle = index as f32 / 14.0 * std::f32::consts::TAU;
            center + egui::vec2(2.6 + 2.1 * angle.cos(), -5.2 + 1.0 * angle.sin()) * k
        })
        .collect();
    painter.add(egui::Shape::convex_polygon(leaf, color, egui::Stroke::NONE));
}

/// Deux personnes côte à côte : le compte et la famille.
fn paint_family_icon(
    painter: &egui::Painter,
    center: egui::Pos2,
    color: egui::Color32,
    scale: f32,
) {
    painter.circle_filled(center + egui::vec2(-3.3, -3.6) * scale, 2.5 * scale, color);
    painter.circle_filled(center + egui::vec2(3.3, -3.6) * scale, 2.5 * scale, color);
    paint_dome(
        painter,
        center + egui::vec2(0.0, 1.0) * scale,
        6.0 * scale,
        4.4 * scale,
        color,
    );
}

/// Disque dur : boîtier, fente et témoin.
fn paint_disk_icon(painter: &egui::Painter, center: egui::Pos2, color: egui::Color32, scale: f32) {
    let stroke = egui::Stroke::new(1.2_f32 * scale, color);
    let body = egui::Rect::from_center_size(center, egui::vec2(15.0, 11.0) * scale);
    painter.rect_stroke(body, egui::Rounding::same(2.5 * scale), stroke);
    painter.line_segment(
        [
            egui::pos2(body.left() + 2.5 * scale, body.bottom() - 3.2 * scale),
            egui::pos2(body.right() - 5.0 * scale, body.bottom() - 3.2 * scale),
        ],
        stroke,
    );
    painter.circle_filled(
        egui::pos2(body.right() - 3.0 * scale, body.bottom() - 3.2 * scale),
        1.1 * scale,
        color,
    );
}

/// Écran de bureau : moniteur et pied.
fn paint_desktop_icon(
    painter: &egui::Painter,
    center: egui::Pos2,
    color: egui::Color32,
    scale: f32,
) {
    let stroke = egui::Stroke::new(1.2_f32 * scale, color);
    let screen = egui::Rect::from_center_size(
        center + egui::vec2(0.0, -1.5) * scale,
        egui::vec2(14.0, 9.5) * scale,
    );
    painter.rect_stroke(screen, egui::Rounding::same(1.5 * scale), stroke);
    painter.line_segment(
        [
            egui::pos2(screen.center().x, screen.bottom()),
            egui::pos2(screen.center().x, screen.bottom() + 2.6 * scale),
        ],
        stroke,
    );
    painter.line_segment(
        [
            egui::pos2(
                screen.center().x - 4.0 * scale,
                screen.bottom() + 3.6 * scale,
            ),
            egui::pos2(
                screen.center().x + 4.0 * scale,
                screen.bottom() + 3.6 * scale,
            ),
        ],
        stroke,
    );
}

/// Téléphone : cadre, haut-parleur et bouton.
fn paint_mobile_icon(
    painter: &egui::Painter,
    center: egui::Pos2,
    color: egui::Color32,
    scale: f32,
) {
    let stroke = egui::Stroke::new(1.2_f32 * scale, color);
    let body = egui::Rect::from_center_size(center, egui::vec2(8.0, 13.5) * scale);
    painter.rect_stroke(body, egui::Rounding::same(2.0 * scale), stroke);
    painter.line_segment(
        [
            egui::pos2(body.center().x - 1.5 * scale, body.top() + 2.0 * scale),
            egui::pos2(body.center().x + 1.5 * scale, body.top() + 2.0 * scale),
        ],
        stroke,
    );
    painter.circle_filled(
        egui::pos2(body.center().x, body.bottom() - 2.0 * scale),
        0.65 * scale,
        color,
    );
}

/// Logo Steam embarqué, relu une fois pour se voir sur un fond sombre.
///
/// Le fichier fourni est un disque **noir** au glyphe blanc : posé tel quel sur
/// une carte sombre, seul le disque se confondait avec le fond et la marque
/// disparaissait. L'image est donc recomposée ici, à partir de ses propres
/// pixels — le glyphe (les pixels clairs) en blanc plein, le disque en blanc
/// très léger — pour rester lisible de 8 à 30 px.
fn steam_mark_image(px: u32) -> egui::ColorImage {
    /// Opacité du disque : assez pour dessiner le rond, assez peu pour que le
    /// glyphe reste ce qu'on voit.
    const DISC: f32 = 0.16;
    let source = image::load_from_memory(include_bytes!("../../../assets/steam.png"))
        .expect("logo Steam embarqué")
        .to_rgba8();
    let image = resize_to(&source, px);
    let (width, height) = (image.width() as usize, image.height() as usize);
    let radius = width.min(height) as f32 / 2.0;
    let centre = egui::vec2(width as f32 / 2.0, height as f32 / 2.0);
    let mut bytes = Vec::with_capacity(width * height * 4);
    for (x, y, pixel) in image.enumerate_pixels() {
        let [red, green, blue, alpha] = pixel.0;
        let luminance =
            (0.2126 * red as f32 + 0.7152 * green as f32 + 0.0722 * blue as f32) / 255.0;
        // Le fond transparent du fichier a beau être blanc, sa couverture le
        // neutralise : le glyphe sort de la luminance, pas de la seule teinte.
        let glyph = luminance.clamp(0.0, 1.0) * (alpha as f32 / 255.0);
        let distance = (egui::vec2(x as f32 + 0.5, y as f32 + 0.5) - centre).length();
        // Bord adouci sur un pixel : l'image est fortement réduite ensuite.
        let disc = ((radius - distance) * radius).clamp(0.0, 1.0) * DISC;
        let opacity = (disc + glyph * (1.0 - disc)).clamp(0.0, 1.0);
        bytes.extend_from_slice(&[255, 255, 255, (opacity * 255.0).round() as u8]);
    }
    egui::ColorImage::from_rgba_unmultiplied([width, height], &bytes)
}

/// Marque d'une boutique embarquée, ramenée au blanc.
///
/// Les fichiers de [assets/stores](assets/stores) sont les logos d'origine,
/// blancs sur fond transparent et rognés à leur contenu. Leur couverture alpha
/// dessine la marque, et la teinte demandée s'applique par-dessus : le lettrage
/// (« EPIC GAMES », « gog.com ») reste donc lisible quand la marque est assez
/// grande, et s'efface proprement quand elle est petite.
fn store_mark_image(bytes: &[u8], px: u32) -> egui::ColorImage {
    let source = image::load_from_memory(bytes)
        .expect("logo de boutique embarqué")
        .to_rgba8();
    // Le fichier fait 512 px de côté ; il est ramené ici à la taille exacte
    // où la marque se dessine. La carte graphique n'a donc plus à réduire une
    // grande image — c'est cette réduction, sans mipmaps, qui rendait le
    // lettrage des logos flou.
    let scaled = resize_to(&source, px);
    let (width, height) = (scaled.width() as usize, scaled.height() as usize);
    let mut pixels = Vec::with_capacity(width * height * 4);
    for pixel in scaled.pixels() {
        let [_, _, _, alpha] = pixel.0;
        pixels.extend_from_slice(&[255, 255, 255, alpha]);
    }
    egui::ColorImage::from_rgba_unmultiplied([width, height], &pixels)
}

/// Fichier embarqué de chaque boutique.
fn store_mark_bytes(store: lapinux_core::stores::Store) -> &'static [u8] {
    match store {
        lapinux_core::stores::Store::Epic => {
            include_bytes!("../../../assets/stores/epic.png")
        }
        lapinux_core::stores::Store::Gog => include_bytes!("../../../assets/stores/gog.png"),
        lapinux_core::stores::Store::Itch => include_bytes!("../../../assets/stores/itch.png"),
        // Steam garde son disque et son glyphe d'origine, déjà recomposés.
        lapinux_core::stores::Store::Steam => include_bytes!("../../../assets/steam.png"),
    }
}

/// Taille en pixels d'une marque : ses points multipliés par la densité de
/// l'écran. Une texture rendue à cette taille s'affiche sans réduction, donc
/// sans le flou d'un logo de 512 px écrasé en 30.
fn mark_pixels(ctx: &egui::Context, points: f32) -> u32 {
    (points * ctx.pixels_per_point()).round().clamp(12.0, 512.0) as u32
}

/// Ramène une image à la longueur demandée, en gardant ses proportions.
fn resize_to(image: &image::RgbaImage, longest: u32) -> image::RgbaImage {
    use image::imageops::FilterType;
    let current = image.width().max(image.height()).max(1);
    if current == longest {
        return image.clone();
    }
    let ratio = longest as f32 / current as f32;
    image::imageops::resize(
        image,
        ((image.width() as f32 * ratio).round() as u32).max(1),
        ((image.height() as f32 * ratio).round() as u32).max(1),
        FilterType::Lanczos3,
    )
}

/// Marque d'une boutique, téléversée une fois par boutique et par taille.
pub(crate) fn store_mark_texture(
    ctx: &egui::Context,
    store: lapinux_core::stores::Store,
    px: u32,
) -> egui::TextureHandle {
    if store == lapinux_core::stores::Store::Steam {
        return steam_mark_texture(ctx, px);
    }
    let id = egui::Id::new(("store-mark", store.key(), px));
    if let Some(texture) = ctx.data(|data| data.get_temp::<egui::TextureHandle>(id)) {
        return texture;
    }
    let texture = ctx.load_texture(
        format!("store-mark-{}-{px}", store.key()),
        store_mark_image(store_mark_bytes(store), px),
        egui::TextureOptions::LINEAR,
    );
    ctx.data_mut(|data| data.insert_temp(id, texture.clone()));
    texture
}

/// Dessine la marque d'une boutique à la taille demandée, sans la déformer.
pub(crate) fn paint_store_mark(
    ui: &mut egui::Ui,
    store: lapinux_core::stores::Store,
    size: f32,
    color: egui::Color32,
) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    paint_store_mark_in(ui.painter(), store, rect, color);
}

/// Même marque, posée dans un rectangle déjà réservé : une pastille de filtre
/// ou une tuile peinte à la main.
pub(crate) fn paint_store_mark_in(
    painter: &egui::Painter,
    store: lapinux_core::stores::Store,
    rect: egui::Rect,
    color: egui::Color32,
) {
    let px = mark_pixels(painter.ctx(), rect.width().max(rect.height()));
    let texture = store_mark_texture(painter.ctx(), store, px);
    let source = texture.size_vec2();
    let ratio = if source.y > 0.0 {
        source.x / source.y
    } else {
        1.0
    };
    let (width, height) = if ratio >= rect.width() / rect.height().max(1.0) {
        (rect.width(), rect.width() / ratio)
    } else {
        (rect.height() * ratio, rect.height())
    };
    let target = egui::Rect::from_center_size(rect.center(), egui::vec2(width, height));
    painter.image(
        texture.id(),
        target,
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        color,
    );
}

/// Marque Steam du contexte : téléversée une fois, puis gardée dans la mémoire
/// d'egui — les pastilles, les jaquettes et les cartes la partagent.
fn steam_mark_texture(ctx: &egui::Context, px: u32) -> egui::TextureHandle {
    let id = egui::Id::new(("steam-mark", px));
    if let Some(texture) = ctx.data(|data| data.get_temp::<egui::TextureHandle>(id)) {
        return texture;
    }
    let texture = ctx.load_texture(
        format!("steam-mark-{px}"),
        steam_mark_image(px),
        egui::TextureOptions::LINEAR,
    );
    ctx.data_mut(|data| data.insert_temp(id, texture.clone()));
    texture
}

/// Marque Steam, dessinée depuis le logo embarqué : la teinte demandée est
/// appliquée au glyphe, comme aux silhouettes qui l'entourent.
fn paint_steam_icon(painter: &egui::Painter, center: egui::Pos2, color: egui::Color32, k: f32) {
    // Le fichier est carré et la marque le remplit : cette taille occupe
    // exactement la place de l'ancien tracé, du disque bas au haut de l'anneau.
    let size = egui::vec2(11.4 * k, 11.4 * k);
    let px = mark_pixels(painter.ctx(), size.x.max(size.y));
    painter.image(
        steam_mark_texture(painter.ctx(), px).id(),
        egui::Rect::from_center_size(center, size),
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        color,
    );
}

/// Sélecteur de sources : un groupe Steam (tout, compte, famille), le choix
/// Epic (tout, PC, mobile), puis la marque de chaque boutique possédée et le
/// disque. Un clic désactive la source, un clic sur le logo Steam bascule
/// compte + famille ensemble.
fn source_switch(
    ui: &mut egui::Ui,
    sources: SourceFilter,
    disk: usize,
    personal: usize,
    family: usize,
    all_steam: usize,
    epic: usize,
    epic_pc: usize,
    epic_mobile: usize,
    gog: usize,
    itch: usize,
) -> Option<SourceFilter> {
    const SEGMENT: f32 = 33.0;
    let mut next = sources;
    let mut clicked = false;

    let widths = [SEGMENT; 3];
    let active = [
        sources.steam && sources.family,
        sources.steam,
        sources.family,
    ];
    let (rect, responses) = segmented_pill(ui, &widths, &active, "sources");
    let painter = ui.painter().clone();
    let segment = |index: usize| segment_rect(rect, &widths, index);

    for (index, response) in responses.iter().enumerate() {
        let ink = if active[index] || response.hovered() {
            TEXT()
        } else {
            TEXT_DIM()
        };
        let center = segment(index).center();
        match index {
            // Le logo Steam a de la marge dans son carré : il reçoit une
            // échelle un peu plus forte que ses voisins pour paraître de la
            // même taille qu'eux.
            0 => paint_steam_icon(&painter, center, ink, 1.55),
            1 => paint_person_icon(&painter, center, ink, 1.35),
            _ => paint_family_icon(&painter, center, ink, 1.35),
        }
    }

    if responses[0].clicked() {
        clicked = true;
        let enable = !next.all_steam();
        next.steam = enable;
        next.family = enable;
    }
    if responses[1].clicked() {
        clicked = true;
        next.steam = !next.steam;
    }
    if responses[2].clicked() {
        clicked = true;
        next.family = !next.family;
    }

    responses[0]
        .clone()
        .on_hover_text(lapinux_core::i18n::t_args(
            "library.source_all",
            &[("count", &all_steam.to_string())],
        ));
    responses[1]
        .clone()
        .on_hover_text(lapinux_core::i18n::t_args(
            "library.source_personal",
            &[("count", &personal.to_string())],
        ));
    responses[2]
        .clone()
        .on_hover_text(lapinux_core::i18n::t_args(
            "library.source_family",
            &[("count", &family.to_string())],
        ));

    // Les boutiques et le disque ont chacun leur pastille, séparées : dans un
    // même cadre, la marque d'une boutique collait au disque et se lisait comme
    // une seule commande. Une pastille par boutique possédée.
    for (slot, store) in LIBRARY_STORES.iter().enumerate() {
        // Epic a deux catalogues dans la même bibliothèque : le choix se pose
        // au même endroit que le groupe Steam, pas dans le panneau de filtres.
        if *store == lapinux_core::stores::Store::Epic {
            let pc = sources.epic_pc;
            let mobile = sources.epic_mobile;
            let widths = [SEGMENT; 3];
            let active = [pc && mobile, pc, mobile];
            let (pill, hits) = segmented_pill(ui, &widths, &active, "epic-platform-source");
            let inks = [
                if active[0] || hits[0].hovered() {
                    TEXT()
                } else {
                    TEXT_DIM()
                },
                if active[1] || hits[1].hovered() {
                    TEXT()
                } else {
                    TEXT_DIM()
                },
                if active[2] || hits[2].hovered() {
                    TEXT()
                } else {
                    TEXT_DIM()
                },
            ];
            paint_store_mark_in(
                &painter,
                *store,
                segment_rect(pill, &widths, 0).shrink(5.0),
                inks[0],
            );
            paint_desktop_icon(
                &painter,
                segment_rect(pill, &widths, 1).center(),
                inks[1],
                1.0,
            );
            paint_mobile_icon(
                &painter,
                segment_rect(pill, &widths, 2).center(),
                inks[2],
                1.0,
            );
            if hits[0].clicked() {
                clicked = true;
                let enable = !(pc && mobile);
                next.epic_pc = enable;
                next.epic_mobile = enable;
                next.stores[slot] = enable;
            }
            if hits[1].clicked() {
                clicked = true;
                next.epic_pc = !pc;
                next.stores[slot] = next.epic_pc || next.epic_mobile;
            }
            if hits[2].clicked() {
                clicked = true;
                next.epic_mobile = !mobile;
                next.stores[slot] = next.epic_pc || next.epic_mobile;
            }
            hits[0].clone().on_hover_text(lapinux_core::i18n::t_args(
                "library.epic_all",
                &[("count", &epic.to_string())],
            ));
            hits[1].clone().on_hover_text(lapinux_core::i18n::t_args(
                "library.epic_pc",
                &[("count", &epic_pc.to_string())],
            ));
            hits[2].clone().on_hover_text(lapinux_core::i18n::t_args(
                "library.epic_mobile",
                &[("count", &epic_mobile.to_string())],
            ));
            continue;
        }
        // L'espacement de la rangée suffit : chaque pastille est déjà un cadre
        // à elle seule. Elle reste juste assez large pour que la marque tienne
        // en hauteur sans coller aux bords.
        let on = sources.stores[slot];
        let salt = format!("store-source-{}", store.key());
        let (pill, responses) = segmented_pill(ui, &[38.0], &[on], &salt);
        let rect = segment_rect(pill, &[38.0], 0);
        let ink = if on || responses[0].hovered() {
            TEXT()
        } else {
            TEXT_DIM()
        };
        // La marque est posée dans son segment, pas à la fin du curseur, et
        // prend presque toute la hauteur : le lettrage reste lisible.
        paint_store_mark_in(&ui.painter().clone(), *store, rect.shrink(5.0), ink);
        if responses[0].clicked() {
            clicked = true;
            next.stores[slot] = !on;
        }
        let count = match *store {
            lapinux_core::stores::Store::Epic => epic,
            lapinux_core::stores::Store::Gog => gog,
            _ => itch,
        };
        responses[0]
            .clone()
            .on_hover_text(lapinux_core::i18n::t_args(
                "library.source_store",
                &[("store", store.label()), ("count", &count.to_string())],
            ));
    }

    let (disk_pill, disk_responses) = segmented_pill(ui, &[38.0], &[sources.disk], "disk-source");
    let disk_rect = segment_rect(disk_pill, &[38.0], 0);
    let ink = if sources.disk || disk_responses[0].hovered() {
        TEXT()
    } else {
        TEXT_DIM()
    };
    paint_disk_icon(&ui.painter().clone(), disk_rect.center(), ink, 1.05);
    if disk_responses[0].clicked() {
        clicked = true;
        next.disk = !next.disk;
    }
    disk_responses[0]
        .clone()
        .on_hover_text(lapinux_core::i18n::t_args(
            "library.disk_hint",
            &[("disk", &disk.to_string())],
        ));

    if !next.any() && clicked {
        return None;
    }
    Some(next)
}

/// Filtre d'installation : Tous / Installés / À installer, dans la même
/// pastille segmentée que les sources. L'appelant le colle au bord droit.
///
/// « À installer » disparaît quand rien ne manque sur le disque — Steam
/// décoché, la bibliothèque ne montre que des jeux installés : proposer le
/// filtre laisserait une catégorie toujours vide.
fn install_filter_group(
    ui: &mut egui::Ui,
    current: CatalogFilter,
    total: usize,
    installed: usize,
    missing: usize,
) -> Option<CatalogFilter> {
    let font = egui::FontId::proportional(11.5);
    let mut items = vec![
        (
            CatalogFilter::All,
            lapinux_core::i18n::t_args("library.filter.all", &[("total", &total.to_string())]),
        ),
        (
            CatalogFilter::Installed,
            lapinux_core::i18n::t_args(
                "library.filter.installed",
                &[("installed", &installed.to_string())],
            ),
        ),
    ];
    if missing > 0 {
        items.push((
            CatalogFilter::Missing,
            lapinux_core::i18n::t_args(
                "library.filter.missing",
                &[("missing", &missing.to_string())],
            ),
        ));
    }
    // Le filtre actif a disparu : on revient à « Tous » pour ne pas afficher
    // une liste vide sous une pastille qui n'existe plus.
    let reset = !items.iter().any(|(filter, _)| *filter == current);
    let widths: Vec<f32> = items
        .iter()
        .map(|(_, text)| {
            ui.fonts(|fonts| {
                fonts
                    .layout_no_wrap(text.clone(), font.clone(), TEXT())
                    .size()
                    .x
            }) + 26.0
        })
        .collect();
    let active: Vec<bool> = items.iter().map(|(filter, _)| current == *filter).collect();
    let (rect, responses) = segmented_pill(ui, &widths, &active, "install");

    let painter = ui.painter().clone();
    let mut chosen = None;
    for (index, (filter, text)) in items.iter().enumerate() {
        let segment = segment_rect(rect, &widths, index);
        let ink = if current == *filter || responses[index].hovered() {
            TEXT()
        } else {
            TEXT_DIM()
        };
        painter.text(
            segment.center(),
            egui::Align2::CENTER_CENTER,
            text,
            font.clone(),
            ink,
        );
        if responses[index].clicked() {
            chosen = Some(*filter);
        }
    }
    if reset {
        return Some(CatalogFilter::All);
    }
    chosen
}

fn paint_search_icon(
    painter: &egui::Painter,
    center: egui::Pos2,
    radius: f32,
    color: egui::Color32,
) {
    let stroke = egui::Stroke::new(1.4_f32, color);
    let circle = center - egui::vec2(1.0, 1.0);
    painter.circle_stroke(circle, radius, stroke);
    painter.line_segment(
        [
            circle + egui::vec2(radius * 0.72, radius * 0.72),
            center + egui::vec2(radius + 3.0, radius + 3.0),
        ],
        stroke,
    );
}

/// Raccorde deux pastilles voisines : les deux bordures pleines s'effacent sur
/// toute la hauteur du bloc et laissent un filet court, identique aux
/// séparateurs internes. Aucun trait horizontal n'est retracé.
fn paint_glued_seam(painter: &egui::Painter, x: f32, top: f32, bottom: f32) {
    painter.rect_filled(
        egui::Rect::from_min_max(egui::pos2(x - 2.0, top), egui::pos2(x + 2.0, bottom)),
        0.0,
        CARD(),
    );
    painter.line_segment(
        [egui::pos2(x, top + 8.0), egui::pos2(x, bottom - 8.0)],
        egui::Stroke::new(1.0_f32, BORDER()),
    );
}

fn search_field(
    ui: &mut egui::Ui,
    text: &mut String,
    width: f32,
    focus: &mut bool,
    glued_left: bool,
) -> bool {
    let mut close = false;
    // Jonction exacte avec la pastille voisine : le curseur est posé là où le
    // cadre commence, avant que sa marge intérieure ne décale tout.
    let glue = ui.cursor().left_top();
    egui::Frame::none()
        .fill(CARD())
        .stroke(egui::Stroke::new(1.0_f32, BORDER()))
        // Le dé vient se coller à droite quand la recherche est ouverte : le
        // champ perd ses angles droits.À gauche, la pastille des filtres s'y
        // accole : le champ perd alors aussi ses angles gauches, sinon la
        // bordure dessinait un décrochement au milieu du groupe.
        .rounding(egui::Rounding {
            nw: if glued_left { 0.0 } else { 9.0 },
            sw: if glued_left { 0.0 } else { 9.0 },
            ne: 0.0,
            se: 0.0,
        })
        .inner_margin(egui::Margin::symmetric(10.0, 0.0))
        .show(ui, |ui| {
            ui.set_height(FILTER_H);
            ui.set_width(width - 20.0);
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let (icon, _) =
                    ui.allocate_exact_size(egui::vec2(15.0, 15.0), egui::Sense::hover());
                paint_search_icon(ui.painter(), icon.center(), 4.5, TEXT_DIM());
                let response = ui.add(
                    egui::TextEdit::singleline(text)
                        .frame(false)
                        .hint_text(
                            egui::RichText::new(lapinux_core::i18n::t(
                                "library.search_placeholder",
                            ))
                            .color(TEXT_DIM()),
                        )
                        .desired_width(width - 80.0),
                );
                if *focus {
                    response.request_focus();
                    *focus = false;
                }
                // Croix dessinée au trait dans une pastille ronde : le glyphe
                // « × » de la police tombait mal aligné et paraissait maigre.
                let (cross_rect, cross) =
                    ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::click());
                let cross = cross.on_hover_cursor(egui::CursorIcon::PointingHand);
                let painter = ui.painter();
                if cross.hovered() {
                    painter.circle_filled(cross_rect.center(), 11.0, CARD_HOVER());
                }
                let ink = if cross.hovered() { TEXT() } else { TEXT_DIM() };
                let center = cross_rect.center();
                let arm = 4.5;
                let stroke = egui::Stroke::new(1.6_f32, ink);
                painter.line_segment(
                    [
                        center + egui::vec2(-arm, -arm),
                        center + egui::vec2(arm, arm),
                    ],
                    stroke,
                );
                painter.line_segment(
                    [
                        center + egui::vec2(arm, -arm),
                        center + egui::vec2(-arm, arm),
                    ],
                    stroke,
                );
                if cross
                    .on_hover_text(lapinux_core::i18n::t("library.close_search"))
                    .clicked()
                {
                    close = true;
                }
            });
        });

    // Accolé à la pastille des filtres, le bord gauche du champ redevient un
    // filet court, comme ceux qui séparent les segments : les deux bordures
    // pleines s'effacent sous le fond, une seule ligne reste.
    if glued_left {
        let painter = ui
            .painter()
            .clone()
            .with_clip_rect(ui.clip_rect().expand(12.0));
        let x = glue.x + 0.5;
        paint_glued_seam(&painter, x, glue.y, glue.y + FILTER_H);
    }
    close
}

/// Dé dessiné au trait, centré sur un point : cinq points, comme l'icône du
/// hasard. Le dé était auparavant le seul habitant de son bouton ; il partage
/// maintenant une pastille avec la loupe, il lui faut donc sa propre fonction.
fn paint_die(painter: &egui::Painter, center: egui::Pos2, color: egui::Color32) {
    let die = egui::Rect::from_center_size(center, egui::vec2(13.0, 13.0));
    painter.rect_stroke(
        die,
        egui::Rounding::same(3.5),
        egui::Stroke::new(1.2_f32, color),
    );
    for (x, y) in [
        (0.28, 0.28),
        (0.72, 0.28),
        (0.5, 0.5),
        (0.28, 0.72),
        (0.72, 0.72),
    ] {
        painter.circle_filled(
            egui::pos2(die.left() + die.width() * x, die.top() + die.height() * y),
            1.2,
            color,
        );
    }
}

fn library_filter_panel(
    ui: &mut egui::Ui,
    sort: &mut LibrarySort,
    descending: &mut bool,
    only_updates: &mut bool,
    never_played: &mut bool,
    open: &mut bool,
    publisher: &mut Option<String>,
    developer: &mut Option<String>,
    credit_menu: &mut Option<bool>,
    query: &mut String,
    options: &[(String, usize)],
) {
    let widths = [FILTER_H];
    let active = [*open];
    // Accolée à la recherche : angles droits à droite, arrondis à gauche.
    let (rect, responses) = segmented_pill_with(
        ui,
        &widths,
        &active,
        "library-sort",
        egui::Rounding {
            nw: 9.0,
            sw: 9.0,
            ne: 0.0,
            se: 0.0,
        },
    );
    let painter = ui.painter().clone();
    let ink = if responses[0].hovered() || *open {
        TEXT()
    } else {
        TEXT_DIM()
    };
    paint_sort_glyph(&painter, rect.center(), ink);
    responses[0]
        .clone()
        .on_hover_text(lapinux_core::i18n::t("library.sort_tooltip"));
    if responses[0].clicked() {
        *open = !*open;
        if !*open {
            *credit_menu = None;
        }
    }
    if !*open {
        return;
    }
    // Position des deux sélecteurs : leur liste s'ouvre juste dessous.
    let mut anchors: [Option<egui::Rect>; 2] = [None, None];
    egui::Area::new(egui::Id::new("library-filter-panel"))
        .order(egui::Order::Foreground)
        // Centré sous la pastille du filtre.
        // Le panneau est borné à la fenêtre : sur une fenêtre étroite, un
        // panneau centré sur la pastille sortait par le bord.
        .fixed_pos({
            let screen = ui.ctx().screen_rect();
            const W: f32 = 364.0;
            let x = (rect.center().x - W / 2.0).clamp(
                screen.left() + 8.0,
                (screen.right() - W - 8.0).max(screen.left() + 8.0),
            );
            egui::pos2(x, rect.bottom() + 6.0)
        })
        .show(ui.ctx(), |ui| {
            egui::Frame::none()
                .fill(CARD())
                .stroke(egui::Stroke::new(1.0_f32, BORDER()))
                .rounding(egui::Rounding::same(10.0))
                .inner_margin(egui::Margin::same(10.0))
                .show(ui, |ui| {
                    // Largeur fixe : le contenu se cale proprement au centre.
                    ui.set_min_width(344.0);
                    ui.set_max_width(344.0);
                    // L'ordre des jeux, groupé en une seule pastille à trois
                    // cases : un choix de groupe, pas trois boutons épars.
                    ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(lapinux_core::i18n::t("library.order"))
                                .size(11.0)
                                .color(TEXT_DIM()),
                        );
                        ui.add_space(6.0);
                        let labels = [
                            lapinux_core::i18n::t("library.sort.name"),
                            lapinux_core::i18n::t("library.sort.playtime"),
                            lapinux_core::i18n::t("library.sort.acquired"),
                        ];
                        let choices = [
                            LibrarySort::Name,
                            LibrarySort::Playtime,
                            LibrarySort::Acquired,
                        ];
                        let widths = [112.0, 110.0, 96.0];
                        let active = [
                            *sort == choices[0],
                            *sort == choices[1],
                            *sort == choices[2],
                        ];
                        let (group, hits) = segmented_pill_with(
                            ui,
                            &widths,
                            &active,
                            "library-order",
                            egui::Rounding::same(9.0),
                        );
                        let painter = ui.painter().clone();
                        let mut left = group.left();
                        for (index, width) in widths.iter().enumerate() {
                            if hits[index].clicked() {
                                *sort = choices[index];
                                // Chaque tri a un sens naturel : les dates et
                                // les poids du plus grand au plus petit. Sans
                                // cela, choisir « date d'obtention » gardait le
                                // sens du tri précédent et affichait les jeux
                                // les plus anciens en premier.
                                *descending = choices[index].default_descending();
                            }
                            // Le survol dit ce que la date vaut : c'est le tri
                            // qui peut surprendre quand une boutique se tait.
                            if index == 1 {
                                hits[index]
                                    .clone()
                                    .on_hover_text(lapinux_core::i18n::t("library.hint.playtime"));
                            } else if index == 2 {
                                hits[index]
                                    .clone()
                                    .on_hover_text(lapinux_core::i18n::t("library.hint.acquired"));
                            }
                            painter.text(
                                egui::pos2(left + width / 2.0, group.center().y),
                                egui::Align2::CENTER_CENTER,
                                labels[index],
                                egui::FontId::proportional(12.0),
                                if active[index] { TEXT() } else { TEXT_DIM() },
                            );
                            left += *width;
                        }
                    });
                    ui.add_space(8.0);
                    // Les trois tris qui ne tiennent pas dans la pastille, puis
                    // le sens, puis les filtres d'attributs.
                    ui.horizontal_wrapped(|ui| {
                        for (choice, key) in [
                            (LibrarySort::Recent, "library.sort.recent"),
                            (LibrarySort::Updated, "library.sort.updated"),
                            (LibrarySort::Size, "library.sort.size"),
                        ] {
                            let selected = *sort == choice;
                            let response = ui.add(
                                egui::Button::new(
                                    egui::RichText::new(lapinux_core::i18n::t(key)).size(11.5),
                                )
                                .fill(if selected { CARD_ACTIVE() } else { CARD() })
                                .stroke(egui::Stroke::new(
                                    1.0_f32,
                                    if selected { ACCENT() } else { BORDER() },
                                ))
                                .rounding(egui::Rounding::same(7.0)),
                            );
                            if response.clicked() {
                                *sort = choice;
                                *descending = choice.default_descending();
                            }
                        }
                    });
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let arrow = if *descending { "↓" } else { "↑" };
                        let label =
                            format!("{} {}", lapinux_core::i18n::t("library.direction"), arrow);
                        if ui
                            .add(
                                egui::Button::new(egui::RichText::new(label).size(11.5))
                                    .fill(CARD())
                                    .stroke(egui::Stroke::new(1.0_f32, BORDER()))
                                    .rounding(egui::Rounding::same(7.0)),
                            )
                            .on_hover_text(lapinux_core::i18n::t("library.direction_hint"))
                            .clicked()
                        {
                            *descending = !*descending;
                        }
                        ui.add_space(6.0);
                        ui.checkbox(only_updates, lapinux_core::i18n::t("library.only_updates"));
                        ui.checkbox(never_played, lapinux_core::i18n::t("library.never_played"));
                    });
                    ui.add_space(10.0);
                    ui.separator();
                    ui.add_space(6.0);
                    // Les deux crédits, chacun avec un vrai bouton de sélection.
                    for is_publisher in [true, false] {
                        let label = if is_publisher {
                            lapinux_core::i18n::t("library.publisher")
                        } else {
                            lapinux_core::i18n::t("library.developer")
                        };
                        let chosen = if is_publisher {
                            publisher.clone()
                        } else {
                            developer.clone()
                        };
                        let lit = *credit_menu == Some(is_publisher);
                        let hint = if is_publisher {
                            lapinux_core::i18n::t("library.hint.publisher")
                        } else {
                            lapinux_core::i18n::t("library.hint.developer")
                        };
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(label).size(12.0).color(TEXT_DIM()))
                                .on_hover_text(hint);
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let shown = chosen.clone().unwrap_or_else(|| {
                                        lapinux_core::i18n::t("library.all").to_string()
                                    });
                                    let text =
                                        egui::RichText::new(format!("{shown}   ▾")).size(12.5);
                                    let button = egui::Button::new(text)
                                        .fill(if lit { CARD_ACTIVE() } else { CARD() })
                                        .stroke(egui::Stroke::new(1.0_f32, BORDER()))
                                        .rounding(egui::Rounding::same(7.0))
                                        .min_size(egui::vec2(190.0, 30.0));
                                    let response = ui.add(button);
                                    response.clone().on_hover_text(hint);
                                    anchors[usize::from(!is_publisher)] = Some(response.rect);
                                    if response.clicked() {
                                        *credit_menu = if lit { None } else { Some(is_publisher) };
                                        *query = String::new();
                                    }
                                },
                            );
                        });
                    }
                });
        });
    // La liste entière, déroulée juste sous le bouton qui l'a ouverte, avec sa
    // barre de recherche. Les rangées sont virtualisées : huit cents éditeurs
    // ne coûtent que l'écran, et la hauteur ne bouge pas d'une image à l'autre.
    if let Some(is_publisher) = *credit_menu {
        if let Some(anchor) = anchors[usize::from(!is_publisher)] {
            let chosen = if is_publisher {
                publisher.clone()
            } else {
                developer.clone()
            };
            let popup_width = 260.0_f32;
            egui::Area::new(egui::Id::new("library-credit-menu"))
                .order(egui::Order::Tooltip)
                .fixed_pos(anchor.left_bottom() + egui::vec2(0.0, 4.0))
                .show(ui.ctx(), |ui| {
                    egui::Frame::none()
                        .fill(CARD())
                        .stroke(egui::Stroke::new(1.0_f32, BORDER()))
                        .rounding(egui::Rounding::same(9.0))
                        .inner_margin(egui::Margin::same(8.0))
                        .show(ui, |ui| {
                            ui.set_width(popup_width);
                            ui.add(
                                egui::TextEdit::singleline(query)
                                    .hint_text(lapinux_core::i18n::t("library.credit_search"))
                                    .desired_width(popup_width),
                            );
                            ui.add_space(6.0);
                            let needle = query.trim().to_lowercase();
                            let hits: Vec<&(String, usize)> = options
                                .iter()
                                .filter(|(name, _)| contains_lower(name, &needle))
                                .collect();
                            let all = if is_publisher {
                                lapinux_core::i18n::t("library.all_publishers")
                            } else {
                                lapinux_core::i18n::t("library.all_developers")
                            };
                            if credit_row(ui, popup_width, all, None, chosen.is_none()) {
                                if is_publisher {
                                    *publisher = None;
                                } else {
                                    *developer = None;
                                }
                                *credit_menu = None;
                            }
                            ui.add_space(4.0);
                            ui.separator();
                            ui.add_space(4.0);
                            let row_height = 21.0;
                            let list_height = row_height * hits.len().clamp(1, 11) as f32;
                            ui.allocate_ui_with_layout(
                                egui::vec2(popup_width, list_height),
                                egui::Layout::top_down(egui::Align::LEFT),
                                |ui| {
                                    egui::ScrollArea::vertical()
                                        .max_height(list_height)
                                        .show_rows(ui, row_height, hits.len(), |ui, range| {
                                            for item in &hits[range] {
                                                let (name, count) = (&item.0, item.1);
                                                let selected =
                                                    chosen.as_deref() == Some(name.as_str());
                                                if credit_row(
                                                    ui,
                                                    popup_width,
                                                    name,
                                                    Some(count),
                                                    selected,
                                                ) {
                                                    let value = if selected {
                                                        None
                                                    } else {
                                                        Some(name.clone())
                                                    };
                                                    if is_publisher {
                                                        *publisher = value;
                                                    } else {
                                                        *developer = value;
                                                    }
                                                    *credit_menu = None;
                                                }
                                            }
                                        });
                                },
                            );
                        });
                });
        }
    }
    if ui.ctx().input(|input| input.key_pressed(egui::Key::Escape)) {
        *open = false;
        *credit_menu = None;
    }
}

/// Curseurs au trait : trois lignes et leurs boutons.
fn paint_sort_glyph(painter: &egui::Painter, center: egui::Pos2, ink: egui::Color32) {
    let stroke = egui::Stroke::new(1.3_f32, ink);
    for (index, offset) in [-4.5_f32, 0.0, 4.5].iter().enumerate() {
        let y = center.y + offset;
        painter.line_segment(
            [egui::pos2(center.x - 5.5, y), egui::pos2(center.x + 5.5, y)],
            stroke,
        );
        let x = center.x
            + match index {
                0 => -2.0,
                1 => 2.0,
                _ => -1.0,
            };
        painter.circle_filled(egui::pos2(x, y), 1.8, ink);
    }
}

fn search_random_pill(ui: &mut egui::Ui, search_open: bool, glued_left: bool) -> (bool, bool) {
    let widths = if search_open {
        vec![FILTER_W]
    } else {
        vec![FILTER_W, FILTER_W]
    };
    let active = vec![false; widths.len()];
    // Ouverte, la recherche a déjà son champ à gauche : le dé s'y accole en
    // gardant des angles gauches droits, sans attendre qu'on lui passe
    // `glued_left` — le champ n'a pas d'angle droit de son côté droit.
    let glued = glued_left || search_open;
    let radius_left = if glued { 0.0 } else { 9.0 };
    let radius_right = if search_open { 9.0 } else { 9.0 };
    let rounding = egui::Rounding {
        nw: radius_left,
        sw: radius_left,
        ne: radius_right,
        se: radius_right,
    };
    let (rect, responses) = segmented_pill_with(ui, &widths, &active, "search-random", rounding);
    let painter = ui.painter().clone();
    // Accolée à la pastille des filtres — recherche ouverte ou non —, cette
    // pastille remplace sa bordure gauche par le filet court de ses propres
    // séparateurs.
    // Le filet ne sert que devant la pastille des filtres : devant le champ de
    // recherche, la bordure droite du champ fait déjà la séparation.
    if glued_left {
        let painter = painter.clone().with_clip_rect(ui.clip_rect().expand(12.0));
        let x = rect.left() + 0.5;
        paint_glued_seam(&painter, x, rect.top(), rect.bottom());
    }
    let mut open_search = false;
    let mut die_segment = 0;
    if !search_open {
        let segment = segment_rect(rect, &widths, 0);
        let ink = if responses[0].hovered() {
            TEXT()
        } else {
            TEXT_DIM()
        };
        paint_search_icon(&painter, segment.center(), 5.5, ink);
        responses[0]
            .clone()
            .on_hover_text(lapinux_core::i18n::t("library.search_tooltip"));
        open_search = responses[0].clicked();
        die_segment = 1;
    }
    let segment = segment_rect(rect, &widths, die_segment);
    let ink = if responses[die_segment].hovered() {
        TEXT()
    } else {
        TEXT_DIM()
    };
    paint_die(&painter, segment.center(), ink);
    responses[die_segment]
        .clone()
        .on_hover_text(lapinux_core::i18n::t("library.random_tooltip"));
    (open_search, responses[die_segment].clicked())
}

fn secondary_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(text).size(12.5).color(TEXT()))
            .fill(CARD())
            .stroke(egui::Stroke::new(1.0_f32, BORDER()))
            .rounding(egui::Rounding::same(8.0))
            .min_size(egui::vec2(0.0, 34.0)),
    )
}

/// Trois onglets de navigation réunis dans un même bloc arrondi, avec des
/// icônes dessinées au trait — les emoji colorés juraient avec la palette.
/// Au repos, seules les icônes restent visibles ; le libellé de l'onglet actif
/// s'ouvre à droite de son icône avec un glissement doux. Pendant un
/// téléchargement, l'icône du jeu remplace le pictogramme, sans pastille de
/// compteur à côté du libellé.
fn main_tabs(
    ui: &mut egui::Ui,
    current: Page,
    badge: Option<&NavBadge>,
    pages: &[Page],
) -> Option<Page> {
    const HEIGHT: f32 = 34.0;
    const INSET: f32 = 2.0;
    const ICON: f32 = 18.0;
    const GAP: f32 = 6.0;
    const PAD: f32 = 10.0;
    const SLIDE: f32 = 0.22;

    // La largeur du bloc et l'opacité du texte suivent la même valeur animée :
    // le libellé s'ouvre en glissant, sans faire sauter la barre.
    let opens: Vec<f32> = pages
        .iter()
        .map(|page| {
            ui.ctx().animate_bool_with_time_and_easing(
                ui.id().with(("main-tab-open", *page as u8)),
                *page == current,
                SLIDE,
                egui::emath::easing::cubic_out,
            )
        })
        .collect();

    // Largeur mesurée sur le libellé : la police semi-grasse des titres est un
    // peu plus large que la medium des onglets au repos.
    let labels: Vec<f32> = pages
        .iter()
        .map(|page| {
            ui.painter()
                .layout_no_wrap(page_label(*page).to_owned(), display_font(13.5), TEXT())
                .size()
                .x
        })
        .collect();
    let widths: Vec<f32> = labels
        .iter()
        .zip(&opens)
        .map(|(label, open)| PAD * 2.0 + ICON + (GAP + label) * open)
        .collect();
    let width = widths.iter().sum::<f32>() + INSET * 2.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, HEIGHT), egui::Sense::hover());

    let mut x = rect.left() + INSET;
    let mut tabs = Vec::with_capacity(pages.len());
    for ((page, tab_width), open) in pages.iter().zip(widths).zip(opens) {
        let label = page_label(*page);
        let tab = egui::Rect::from_min_size(
            egui::pos2(x, rect.top() + INSET),
            egui::vec2(tab_width, rect.height() - INSET * 2.0),
        );
        x += tab_width;
        // La position de chaque onglet sert à la jaquette qui vient s'y poser
        // après l'activation d'une clé.
        ui.data_mut(|data| {
            data.insert_temp(egui::Id::new(("nav-tab-rect", *page as u8)), tab);
        });
        let response = ui.interact(
            tab,
            ui.id().with(("main-tab", *page as u8)),
            egui::Sense::click(),
        );
        tabs.push((*page, label, tab, response, open));
    }
    if tabs.iter().any(|(_, _, _, response, _)| response.hovered()) {
        ui.output_mut(|out| out.cursor_icon = egui::CursorIcon::PointingHand);
    }

    let painter = ui.painter();
    painter.rect(
        rect,
        egui::Rounding::same(9.0),
        CARD(),
        egui::Stroke::new(1.0_f32, BORDER()),
    );
    let mut clicked = None;
    for (page, label, tab, response, open) in tabs {
        let selected = page == current;
        let hovered = response.hovered();
        if selected || hovered {
            painter.rect_filled(
                tab.shrink2(egui::vec2(1.0, 0.0)),
                egui::Rounding::same(7.0),
                if selected {
                    CARD_ACTIVE()
                } else {
                    CARD_HOVER()
                },
            );
        }
        if response.has_focus() {
            painter.rect_stroke(
                tab.shrink2(egui::vec2(1.0, 0.0)),
                egui::Rounding::same(7.0),
                egui::Stroke::new(1.0_f32, ACCENT()),
            );
        }
        let ink = if selected || hovered {
            TEXT()
        } else {
            TEXT_DIM()
        };
        let icon = egui::pos2(tab.left() + PAD + ICON / 2.0, tab.center().y);
        match badge.filter(|_| page == Page::Downloads) {
            // Pendant un téléchargement, l'icône du jeu remplace le pictogramme
            // et un halo coloré fait le tour au rythme de la progression.
            Some(badge) => paint_download_nav_badge(ui, painter, icon, badge),
            None => {
                let progress = icon_hover_progress(
                    ui,
                    ui.id().with(("nav-icon-motion", page as u8)),
                    hovered || response.has_focus(),
                );
                section_icons::page_animated(
                    painter,
                    page,
                    icon,
                    section_icons::INTERFACE_ICON_SIZE,
                    ink,
                    progress,
                );
            }
        }
        if open > 0.01 {
            let font = if selected {
                display_font(13.5)
            } else {
                egui::FontId::proportional(13.5)
            };
            let color = with_alpha(ink, open);
            let text = painter.layout_no_wrap(label.to_owned(), font, color);
            // Le libellé est découpé au cadre de l'onglet : pendant le
            // glissement, il ne déborde pas sur l'icône suivante.
            painter
                .with_clip_rect(tab.shrink2(egui::vec2(1.0, 0.0)))
                .galley(
                    egui::pos2(
                        tab.left() + PAD + ICON + GAP * open,
                        tab.center().y - text.size().y / 2.0,
                    ),
                    text,
                    color,
                );
        }
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, label)
        });
        // Enter et Espace activent l'onglet au clavier, comme un vrai bouton.
        let keyboard = response.has_focus()
            && ui.input(|input| {
                input.key_pressed(egui::Key::Enter) || input.key_pressed(egui::Key::Space)
            });
        if response.clicked() || keyboard {
            clicked = Some(page);
        }
    }
    clicked
}

/// Icône du jeu dans l'onglet Téléchargements, entourée d'un halo lumineux
/// dont l'arc suit la progression. L'icône remplace le pictogramme habituel.
fn paint_download_nav_badge(
    ui: &egui::Ui,
    painter: &egui::Painter,
    center: egui::Pos2,
    badge: &NavBadge,
) {
    use std::f32::consts::{FRAC_PI_2, TAU};
    let color = if badge.running {
        badge.color
    } else {
        badge.color.gamma_multiply(0.6)
    };
    // Pas de halo plein cercle : il colorait aussi la partie non téléchargée.
    // La lueur est dessinée plus bas, uniquement le long de l'arc acquis.
    // Part restant à télécharger : un anneau gris très sombre. Un blanc
    // translucide y était plus lumineux que l'arc de progression lui-même, ce
    // qui laissait croire que le halo était déjà presque rempli.
    painter.circle_stroke(
        center,
        11.0,
        egui::Stroke::new(2.0_f32, egui::Color32::from_white_alpha(14)),
    );
    // Ce qui est installé, à l'inverse, se détache : la couleur du jeu est
    // tirée vers le blanc, sans perdre sa teinte.
    let arc_ink = downloads::lerp_color(color, TEXT(), 0.3);
    let progress = badge.progress.clamp(0.0, 1.0);
    if progress > 0.0 {
        let steps = (progress * 48.0).ceil().max(2.0) as usize;
        let sweep = progress * TAU;
        let arc: Vec<egui::Pos2> = (0..=steps)
            .map(|index| {
                let angle = -FRAC_PI_2 + sweep * index as f32 / steps as f32;
                center + egui::vec2(angle.cos(), angle.sin()) * 11.0
            })
            .collect();
        if badge.running {
            painter.add(egui::Shape::line(
                arc.clone(),
                egui::Stroke::new(5.0_f32, with_alpha(arc_ink, 0.18)),
            ));
        }
        painter.add(egui::Shape::line(arc, egui::Stroke::new(2.0_f32, arc_ink)));
        let end = -FRAC_PI_2 + sweep;
        painter.circle_filled(
            center + egui::vec2(end.cos(), end.sin()) * 11.0,
            if badge.running { 2.2 } else { 1.6 },
            arc_ink,
        );
    }
    let icon = egui::Rect::from_center_size(center, egui::vec2(15.0, 15.0));
    egui::Image::new((badge.icon.id(), icon.size()))
        .rounding(egui::Rounding::same(7.5))
        .paint_at(ui, icon);
}

/// Une ligne du panneau Amis : les amis Steam et ceux des boutiques partagent
/// la même forme, seule la provenance change.
struct FriendRow {
    store: lapinux_core::stores::Store,
    /// Clé du cache de photos (`steam:<id>`, `epic:<id>`…).
    key: String,
    name: String,
    state: String,
    avatar: String,
    online: bool,
    in_game: bool,
    /// Présence « absent » : Steam la compte en ligne, mais elle doit passer
    /// après les amis réellement disponibles.
    away: bool,
    /// Dernière déconnexion connue, 0 si la boutique ne la publie pas.
    last_seen: i64,
    /// Jeu en cours, vide si l'ami n'en publie pas.
    game: String,
    /// AppID Steam du jeu en cours, 0 sinon.
    game_appid: u32,
    /// Dernière connexion Steam, 0 si elle n'est pas publiée.
    last_logon: i64,
}

/// Ordre des sections du panneau Amis : Steam d'abord, puis les boutiques.
fn store_rank(store: lapinux_core::stores::Store) -> u8 {
    match store {
        lapinux_core::stores::Store::Steam => 0,
        lapinux_core::stores::Store::Epic => 1,
        lapinux_core::stores::Store::Gog => 2,
        lapinux_core::stores::Store::Itch => 3,
    }
}

/// Segment du sélecteur d'amis : logo de boutique, libellé et compte.
fn friend_chip(
    ui: &mut egui::Ui,
    label: &str,
    active: bool,
    store: Option<lapinux_core::stores::Store>,
    width: f32,
) -> egui::Response {
    let text =
        egui::RichText::new(label)
            .size(11.0)
            .color(if active { TEXT() } else { TEXT_DIM() });
    // Même galley que le bouton : même famille de police, même taille, même
    // style. Une mesure approximative décalerait le contenu de quelques pixels
    // sur les libellés dont les glyphes ont des largeurs différentes.
    let text_width = egui::WidgetText::from(text.clone())
        .into_galley(ui, None, f32::INFINITY, egui::TextStyle::Button)
        .size()
        .x;
    let (button, icon_width) = if let Some(store) = store {
        let texture = store_mark_texture(ui.ctx(), store, mark_pixels(ui.ctx(), 15.0));
        let source = texture.size_vec2();
        let scale = 15.0 / source.x.max(source.y).max(1.0);
        // La marque garde son rapport : sa largeur n'est pas toujours 15 px.
        let image_size = source * scale;
        let image = egui::Image::new((texture.id(), image_size)).tint(if active {
            store_tint(store)
        } else {
            TEXT_DIM()
        });
        (egui::Button::image_and_text(image, text), image_size.x)
    } else {
        (egui::Button::new(text), 0.0)
    };
    // egui 0.29 pose l'image et le texte à partir du bord gauche du bouton et
    // laisse l'espace excédentaire à droite. On répartit cet espace en marge
    // horizontale pour que le contenu soit centré dans chaque segment, que le
    // bouton ait une marque ou non. L'écart marque/texte est `icon_spacing`.
    const ICON_SPACING: f32 = 7.0;
    let content_width = text_width + icon_width + if store.is_some() { ICON_SPACING } else { 0.0 };
    let saved_padding = ui.spacing().button_padding;
    let saved_icon_spacing = ui.spacing().icon_spacing;
    ui.spacing_mut().button_padding.x = ((width - content_width) / 2.0).max(0.0);
    ui.spacing_mut().icon_spacing = ICON_SPACING;
    let response = ui.add(
        button
            .fill(if active {
                CARD_ACTIVE()
            } else {
                egui::Color32::TRANSPARENT
            })
            .stroke(egui::Stroke::NONE)
            .rounding(egui::Rounding::same(8.0))
            .min_size(egui::vec2(width, 28.0)),
    );
    ui.spacing_mut().button_padding = saved_padding;
    ui.spacing_mut().icon_spacing = saved_icon_spacing;
    if active {
        // Un trait de la couleur de la boutique souligne le segment choisi :
        // le sélecteur se lit d'un coup d'œil, sans quatre cadres séparés.
        let tint = store.map(store_tint).unwrap_or(ACCENT());
        let line = egui::Rect::from_min_max(
            egui::pos2(response.rect.left() + 9.0, response.rect.bottom() - 2.0),
            egui::pos2(response.rect.right() - 9.0, response.rect.bottom()),
        );
        ui.painter().rect_filled(line, 1.0, tint);
    }
    response
}

/// Couleur de la marque d'une boutique, pour les segments et la fiche d'un ami.
fn store_tint(store: lapinux_core::stores::Store) -> egui::Color32 {
    match store {
        lapinux_core::stores::Store::Steam => egui::Color32::from_rgb(102, 192, 244),
        lapinux_core::stores::Store::Epic => egui::Color32::from_rgb(176, 168, 255),
        lapinux_core::stores::Store::Gog => egui::Color32::from_rgb(196, 128, 250),
        lapinux_core::stores::Store::Itch => egui::Color32::from_rgb(250, 92, 92),
    }
}

#[derive(Clone, Copy)]
struct TopbarFriendFlash<'a> {
    texture: Option<&'a egui::TextureHandle>,
    game_icon: Option<&'a egui::TextureHandle>,
    name: &'a str,
    caption: &'a str,
    game_event: bool,
    progress: f32,
    /// Réduite quand un modal assombrit déjà l'application : la bulle reste
    /// lisible sans redevenir la seule surface lumineuse au-dessus du voile.
    opacity: f32,
}

/// Progression de l'animation d'une icône survolée. Le mouvement ne se joue
/// qu'à l'entrée : à la sortie, l'état est remis à zéro sans rejouer la courbe
/// en sens inverse.
fn icon_hover_progress(ui: &egui::Ui, id: egui::Id, active: bool) -> f32 {
    if active {
        ui.ctx().animate_bool_with_time(id, true, 0.55)
    } else {
        // Animation quasi nulle pour remettre l'état interne au repos sans
        // peindre de transition de sortie.
        let _ = ui.ctx().animate_bool_with_time(id, false, 0.01);
        0.0
    }
}

/// Commandes de la barre : plein écran, notifications, amis et réglages.
fn topbar_controls(
    ui: &mut egui::Ui,
    settings_open: bool,
    friends_open: bool,
    notifications_open: bool,
    fullscreen: bool,
    notification_unread: bool,
    friend_flash: Option<TopbarFriendFlash<'_>>,
) -> (bool, bool, bool, bool) {
    const SEG: f32 = 34.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(SEG * 4.0, SEG), egui::Sense::hover());
    let segment = |index: usize| {
        egui::Rect::from_min_size(
            egui::pos2(rect.left() + SEG * index as f32, rect.top()),
            egui::vec2(SEG, SEG),
        )
    };
    let screen_rect = segment(0);
    let notifications_rect = segment(1);
    let friends_rect = segment(2);
    let settings_rect = segment(3);
    let screen = ui.interact(
        screen_rect,
        ui.id().with("topbar-screen"),
        egui::Sense::click(),
    );
    let notifications = ui.interact(
        notifications_rect,
        ui.id().with("topbar-notifications"),
        egui::Sense::click(),
    );
    let friends = ui.interact(
        friends_rect,
        ui.id().with("topbar-friends"),
        egui::Sense::click(),
    );
    let settings = ui.interact(
        settings_rect,
        ui.id().with("topbar-settings"),
        egui::Sense::click(),
    );
    if screen.hovered() || notifications.hovered() || friends.hovered() || settings.hovered() {
        ui.output_mut(|out| out.cursor_icon = egui::CursorIcon::PointingHand);
    }
    let painter = ui.painter();
    painter.rect(
        rect,
        egui::Rounding::same(9.0),
        CARD(),
        egui::Stroke::new(1.0_f32, BORDER()),
    );
    for (segment, active, hovered) in [
        (screen_rect, fullscreen, screen.hovered()),
        (
            notifications_rect,
            notifications_open,
            notifications.hovered(),
        ),
        (friends_rect, friends_open, friends.hovered()),
        (settings_rect, settings_open, settings.hovered()),
    ] {
        if active || hovered {
            painter.rect_filled(
                segment.shrink(2.0),
                egui::Rounding::same(6.0),
                if active { CARD_ACTIVE() } else { CARD_HOVER() },
            );
        }
    }
    let ink = |active: bool, hovered: bool| {
        if active || hovered {
            TEXT()
        } else {
            TEXT_DIM()
        }
    };
    let progress = |id: &'static str, response: &egui::Response| {
        icon_hover_progress(
            ui,
            ui.id().with(id),
            response.hovered() || response.has_focus(),
        )
    };
    section_icons::fullscreen(
        painter,
        screen_rect.center(),
        section_icons::INTERFACE_ICON_SIZE,
        ink(fullscreen, screen.hovered()),
        progress("screen-icon-motion", &screen),
        fullscreen,
    );
    section_icons::notifications(
        painter,
        notifications_rect.center(),
        section_icons::INTERFACE_ICON_SIZE,
        ink(notifications_open, notifications.hovered()),
        progress("notifications-icon-motion", &notifications),
    );
    if notification_unread {
        painter.circle_filled(
            notifications_rect.right_top() + egui::vec2(-6.0, 6.0),
            3.0,
            ACCENT(),
        );
    }
    if let Some(flash) = friend_flash {
        // Entrée douce depuis l'avatar, puis sortie vers le haut : la bulle
        // ne tombe plus hors de l'écran quand elle s'efface.
        let enter = smoothstep(flash.progress / 0.09);
        let leave = smoothstep((1.0 - flash.progress) / 0.14);
        let alpha = enter * leave * flash.opacity;
        let pop = smoothstep(flash.progress / 0.12);
        let avatar_size = 20.0 + 4.0 * pop;
        let avatar_rect = egui::Rect::from_center_size(
            friends_rect.center(),
            egui::vec2(avatar_size, avatar_size),
        );
        painter.circle_filled(
            friends_rect.center(),
            13.0,
            with_alpha(CARD_ACTIVE(), alpha),
        );
        match flash.texture {
            Some(texture) => {
                egui::Image::new((texture.id(), avatar_rect.size()))
                    .rounding(egui::Rounding::same(avatar_size / 2.0))
                    .tint(with_alpha(egui::Color32::WHITE, alpha))
                    .paint_at(ui, avatar_rect);
            }
            None => {
                let initial = flash
                    .name
                    .trim()
                    .chars()
                    .next()
                    .map(|letter| letter.to_uppercase().to_string())
                    .unwrap_or_else(|| "?".to_string());
                painter.text(
                    avatar_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    initial,
                    egui::FontId::proportional(11.0),
                    with_alpha(TEXT(), alpha),
                );
            }
        }
        painter.circle_stroke(
            avatar_rect.center(),
            avatar_size / 2.0 + 0.5,
            egui::Stroke::new(1.0_f32, with_alpha(ACCENT(), 0.35 + 0.65 * alpha)),
        );
        let halo = 1.0 - smoothstep(flash.progress / 0.22);
        if halo > 0.01 {
            painter.circle_stroke(
                friends_rect.center(),
                14.0 + (1.0 - halo) * 8.0,
                egui::Stroke::new(1.0_f32, with_alpha(ACCENT(), 0.28 * halo)),
            );
        }
        if alpha > 0.01 {
            let lift = (1.0 - enter) * 6.0 + (1.0 - leave) * 8.0;
            let right = ui.ctx().screen_rect().right() - 18.0;
            // La bulle doit rester au-dessus de la page mais derrière les
            // modaux, qui occupent eux-mêmes le calque `Foreground`.
            egui::Area::new(egui::Id::new("friend-activity-caption"))
                .order(egui::Order::Middle)
                .fixed_pos(egui::pos2(right, friends_rect.bottom() + 3.0 - lift))
                .pivot(egui::Align2::RIGHT_TOP)
                .show(ui.ctx(), |ui| {
                    // Une petite réserve au-dessus du cadre laisse passer la
                    // pointe qui désigne l'avatar.
                    ui.add_space(7.0);
                    let bubble_top = ui.cursor().top();
                    let bubble = egui::Frame::none()
                        .fill(with_alpha(CARD(), alpha))
                        .stroke(egui::Stroke::new(
                            1.0_f32,
                            with_alpha(ACCENT(), 0.45 * alpha),
                        ))
                        .rounding(egui::Rounding::same(9.0))
                        .shadow(egui::epaint::Shadow {
                            offset: egui::vec2(0.0, 4.0),
                            blur: 12.0,
                            spread: 0.0,
                            color: egui::Color32::from_black_alpha((96.0 * alpha) as u8),
                        })
                        .inner_margin(egui::Margin::symmetric(11.0, 6.0))
                        .show(ui, |ui| {
                            ui.set_max_width(280.0);
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing.x = 5.0;
                                ui.label(
                                    egui::RichText::new(flash.name)
                                        .size(11.0)
                                        .strong()
                                        .color(with_alpha(TEXT(), alpha)),
                                );
                                ui.label(
                                    egui::RichText::new(flash.caption)
                                        .size(10.5)
                                        .color(with_alpha(TEXT_DIM(), alpha)),
                                );
                                if flash.game_event {
                                    match flash.game_icon {
                                        Some(texture) => {
                                            ui.add(
                                                egui::Image::new((
                                                    texture.id(),
                                                    egui::vec2(16.0, 16.0),
                                                ))
                                                .rounding(egui::Rounding::same(4.0))
                                                .tint(with_alpha(egui::Color32::WHITE, alpha)),
                                            );
                                        }
                                        None => {
                                            let (rect, _) = ui.allocate_exact_size(
                                                egui::vec2(16.0, 16.0),
                                                egui::Sense::hover(),
                                            );
                                            ui.painter().rect_filled(
                                                rect,
                                                egui::Rounding::same(4.0),
                                                PANEL(),
                                            );
                                            ui.painter().text(
                                                rect.center(),
                                                egui::Align2::CENTER_CENTER,
                                                nav_glyph(Page::Library),
                                                egui::FontId::proportional(9.0),
                                                with_alpha(TEXT_DIM(), alpha),
                                            );
                                        }
                                    }
                                }
                            });
                        });
                    let bubble_rect = bubble.response.rect;
                    let arrow_x = friends_rect
                        .center()
                        .x
                        .clamp(bubble_rect.left() + 14.0, bubble_rect.right() - 14.0);
                    let tip = egui::pos2(arrow_x, bubble_top - 5.5);
                    let left_point = egui::pos2(arrow_x - 6.0, bubble_top + 0.5);
                    let right_point = egui::pos2(arrow_x + 6.0, bubble_top + 0.5);
                    ui.painter().add(egui::Shape::convex_polygon(
                        vec![tip, left_point, right_point],
                        with_alpha(CARD(), alpha),
                        egui::Stroke::NONE,
                    ));
                    let edge = egui::Stroke::new(1.0_f32, with_alpha(ACCENT(), 0.45 * alpha));
                    ui.painter().line_segment([tip, left_point], edge);
                    ui.painter().line_segment([tip, right_point], edge);
                });
        }
    } else {
        section_icons::friends(
            painter,
            friends_rect.center(),
            section_icons::INTERFACE_ICON_SIZE,
            ink(friends_open, friends.hovered()),
            progress("friends-icon-motion", &friends),
        );
    }
    section_icons::gear(
        painter,
        settings_rect.center(),
        section_icons::INTERFACE_ICON_SIZE,
        ink(settings_open, settings.hovered()),
        progress("settings-icon-motion", &settings),
    );
    for offset in [SEG, SEG * 2.0, SEG * 3.0] {
        let x = rect.left() + offset;
        painter.line_segment(
            [
                egui::pos2(x, rect.top() + 7.0),
                egui::pos2(x, rect.bottom() - 7.0),
            ],
            egui::Stroke::new(1.0_f32, BORDER()),
        );
    }
    let screen = screen.on_hover_text(if fullscreen {
        "Quitter le plein écran"
    } else {
        "Passer en plein écran"
    });
    let notifications =
        notifications.on_hover_text(lapinux_core::i18n::t("notifications_panel.title"));
    let friends = friends.on_hover_text("Amis");
    let settings = settings.on_hover_text(lapinux_core::i18n::t("shell.settings_shortcut"));
    (
        settings.clicked(),
        screen.clicked(),
        friends.clicked(),
        notifications.clicked(),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WindowButton {
    Minimize,
    Maximize,
    Restore,
    Close,
}

/// Bouton de la barre de fenêtre : trait fin, pas de fond au repos.
fn window_button(ui: &mut egui::Ui, kind: WindowButton) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(30.0, 24.0), egui::Sense::click());
    let hovered = response.hovered();
    let painter = ui.painter();
    if hovered {
        let fill = match kind {
            WindowButton::Close => egui::Color32::from_rgb(176, 68, 68),
            _ => CARD_HOVER(),
        };
        painter.rect_filled(rect, egui::Rounding::same(6.0), fill);
    }
    let ink = if hovered { TEXT() } else { TEXT_DIM() };
    let center = rect.center();
    let stroke = egui::Stroke::new(1.2_f32, ink);
    match kind {
        WindowButton::Minimize => {
            painter.line_segment(
                [
                    center + egui::vec2(-4.5, 0.0),
                    center + egui::vec2(4.5, 0.0),
                ],
                stroke,
            );
        }
        WindowButton::Maximize => {
            painter.rect_stroke(
                egui::Rect::from_center_size(center, egui::vec2(9.0, 9.0)),
                egui::Rounding::same(1.5),
                stroke,
            );
        }
        WindowButton::Restore => {
            // Deux carrés décalés : la fenêtre est déjà agrandie.
            let back =
                egui::Rect::from_min_size(center + egui::vec2(-3.0, -4.5), egui::vec2(7.5, 7.5));
            let front =
                egui::Rect::from_min_size(center + egui::vec2(-4.5, -3.0), egui::vec2(7.5, 7.5));
            painter.rect_stroke(back, egui::Rounding::same(1.5), stroke);
            painter.rect_stroke(front, egui::Rounding::same(1.5), stroke);
        }
        WindowButton::Close => {
            painter.line_segment(
                [
                    center + egui::vec2(-4.0, -4.0),
                    center + egui::vec2(4.0, 4.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    center + egui::vec2(-4.0, 4.0),
                    center + egui::vec2(4.0, -4.0),
                ],
                stroke,
            );
        }
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Tuile d'une valeur courte : un libellé discret au-dessus, la valeur en
/// dessous. Trois tuiles côte à côte se lisent mieux qu'une ligne de texte où
/// tout se mélange.
fn stat_tile(ui: &mut egui::Ui, width: f32, label: &str, value: &str, color: egui::Color32) {
    egui::Frame::none()
        .fill(CARD_ACTIVE())
        .stroke(egui::Stroke::new(1.0_f32, BORDER()))
        .rounding(egui::Rounding::same(10.0))
        .inner_margin(egui::Margin::symmetric(12.0, 9.0))
        .show(ui, |ui| {
            ui.set_width((width - 24.0).max(60.0));
            ui.label(egui::RichText::new(label).size(10.0).color(TEXT_DIM()));
            ui.add_space(3.0);
            ui.add(
                egui::Label::new(egui::RichText::new(value).size(12.5).strong().color(color))
                    .truncate(),
            );
        });
}

fn steam_step(ui: &mut egui::Ui, number: u32, text: &str) {
    ui.horizontal(|ui| {
        let (badge, _) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::hover());
        ui.painter()
            .circle_filled(badge.center(), 11.0, CARD_ACTIVE());
        ui.painter().text(
            badge.center(),
            egui::Align2::CENTER_CENTER,
            number,
            egui::FontId::proportional(10.5),
            TEXT(),
        );
        ui.add_space(8.0);
        ui.label(egui::RichText::new(text).size(12.0).color(TEXT()));
    });
}

/// Majuscule initiale pour un texte venu du daemon ou d'un message technique,
/// afin que tout ce qui s'affiche soit une phrase.
fn sentence(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn with_alpha(color: egui::Color32, alpha: f32) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(
        color.r(),
        color.g(),
        color.b(),
        (alpha.clamp(0.0, 1.0) * 255.0) as u8,
    )
}

/// Expose la couverture locale d'un jeu (utilisé par les tests et la CLI).
pub fn cover_file(paths: &LapinuxPaths, appid: u32) -> PathBuf {
    paths.covers_dir().join(format!("{appid}.jpg"))
}

/// Jaquette choisie manuellement dans SteamGridDB ; elle prime sur les
/// jaquettes récupérées automatiquement.
fn custom_cover_path(paths: &LapinuxPaths, appid: u32) -> PathBuf {
    paths.existing_custom_cover_path(appid)
}

fn remove_custom_cover(paths: &LapinuxPaths, appid: u32) -> bool {
    let mut removed = false;
    for path in [
        paths.custom_cover_path(appid),
        paths.legacy_custom_cover_path(appid),
    ] {
        if std::fs::remove_file(path).is_ok() {
            removed = true;
        }
    }
    removed
}

pub fn request_json<T: serde::de::DeserializeOwned>(client: &Client, req: Req) -> Option<T> {
    client.request(&req).ok().and_then(|e| e.into_data().ok())
}

#[cfg(test)]
#[path = "ui_tests.rs"]
mod tests;

#[cfg(test)]
mod drm_tags {
    use super::*;

    fn report(drm: Option<&'static str>, client: Option<&'static str>) -> drm::Report {
        drm::Report {
            drm,
            client,
            requires_steam: false,
        }
    }

    fn game() -> lapinux_core::stores::StoreGame {
        lapinux_core::stores::StoreGame {
            store_id: "1200".into(),
            name: "Jeu test".into(),
            ..Default::default()
        }
    }

    #[test]
    fn the_sentence_names_steamworks_steam_or_drm_free() {
        use lapinux_core::model::Drm;
        assert_eq!(
            local_drm_sentence(Drm::None),
            Some(lapinux_core::i18n::t("game_sheet.drm_free"))
        );
        assert_eq!(
            local_drm_sentence(Drm::Steamworks),
            Some(lapinux_core::i18n::t("game_sheet.drm_steamworks"))
        );
        assert_eq!(
            local_drm_sentence(Drm::SteamStub),
            Some(lapinux_core::i18n::t("game_sheet.drm_requires_steam"))
        );
        assert_eq!(local_drm_sentence(Drm::Unknown), None);
    }

    #[test]
    fn the_store_sentence_reads_the_folder_then_the_store() {
        use lapinux_core::stores::Store;
        assert_eq!(
            store_drm_sentence(Store::Gog, None).as_deref(),
            Some(lapinux_core::i18n::t("game_sheet.drm_free"))
        );
        let epic = lapinux_core::i18n::t_args(
            "game_sheet.drm_requires_store",
            &[("store", "Epic Games")],
        );
        assert_eq!(
            store_drm_sentence(Store::Epic, None).as_deref(),
            Some(epic.as_str())
        );
        assert_eq!(
            store_drm_sentence(Store::Gog, Some(&report(Some("Steam"), Some("Steam")))).as_deref(),
            Some(lapinux_core::i18n::t("game_sheet.drm_steamworks"))
        );
        let required = drm::Report {
            drm: Some("Steam"),
            client: Some("Steam"),
            requires_steam: true,
        };
        assert_eq!(
            store_drm_sentence(Store::Epic, Some(&required)).as_deref(),
            Some(lapinux_core::i18n::t("game_sheet.drm_requires_steam"))
        );
        let protection =
            lapinux_core::i18n::t_args("game_sheet.drm_protection", &[("name", "Denuvo")]);
        assert_eq!(
            store_drm_sentence(Store::Epic, Some(&report(Some("Denuvo"), None))).as_deref(),
            Some(protection.as_str())
        );
    }

    #[test]
    fn an_installed_game_without_drm_says_sans_drm_and_sans_client() {
        let facts = store_facts(&game(), None, None, None, Some(&report(None, None)));
        assert!(facts
            .iter()
            .any(|(label, value)| label.is_empty() && value.as_str() == "Sans DRM"));
        assert!(facts
            .iter()
            .any(|(label, value)| label.is_empty() && value.as_str() == "Sans client"));
    }

    #[test]
    fn a_detected_protection_names_the_drm_and_the_client() {
        let facts = store_facts(
            &game(),
            None,
            None,
            None,
            Some(&report(Some("Denuvo"), Some("Steam"))),
        );
        assert!(facts
            .iter()
            .any(|(label, value)| *label == "DRM" && value.as_str() == "Denuvo"));
        assert!(facts
            .iter()
            .any(|(label, value)| *label == "Client" && value.as_str() == "Steam"));
        assert!(!facts.iter().any(|(_, value)| value.as_str() == "Sans DRM"));
    }

    #[test]
    fn a_game_that_is_not_on_disk_has_no_protection_tag() {
        let facts = store_facts(&game(), None, None, None, None);
        assert!(!facts
            .iter()
            .any(|(label, _)| *label == "DRM" || *label == "Client"));
        assert!(!facts
            .iter()
            .any(|(_, value)| { value.as_str() == "Sans DRM" || value.as_str() == "Sans client" }));
    }

    #[test]
    fn mobile_platforms_lead_the_information_badges() {
        let game = lapinux_core::stores::StoreGame {
            mobile: true,
            mobile_platforms: vec!["android".into(), "ios".into()],
            ..game()
        };
        let facts = store_facts(&game, None, None, None, None);
        assert_eq!(&facts[0], &("", "Android".to_string()));
        assert_eq!(&facts[1], &("", "iOS".to_string()));
        assert!(
            facts
                .iter()
                .any(|(label, value)| label.is_empty() && value == lapinux_core::i18n::t("game_sheet.mobile_not_installable")),
            "{facts:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Synchronisation : la fenêtre qui choisit les sources
// ---------------------------------------------------------------------------

/// Ce que la fenêtre de synchronisation propose : les sources relues, cochées
/// par défaut.
struct SyncDialog {
    steam: bool,
    /// Une case par boutique, dans l'ordre de `Store::EXTERNAL`.
    stores: [bool; 3],
    disk: bool,
    /// Jeux trouvés sur le disque à l'ouverture : la ligne annonce ce qu'elle
    /// relit.
    disk_count: usize,
}

impl SyncDialog {
    fn new(disk_count: usize) -> Self {
        Self {
            steam: true,
            stores: [true; 3],
            disk: true,
            disk_count,
        }
    }

    /// Au moins une source cochée : sans elle, le bouton reste éteint.
    fn any(&self) -> bool {
        self.steam || self.disk || self.stores.iter().any(|on| *on)
    }
}

/// Marque dessinée à gauche d'une ligne de la fenêtre.
#[derive(Clone, Copy)]
enum SyncGlyph {
    Steam,
    Store(lapinux_core::stores::Store),
    Disk,
}

/// Une ligne cochable : la case, la marque de la source, son nom, son état et
/// le nombre de jeux qu'elle apporte.
fn sync_row(
    ui: &mut egui::Ui,
    glyph: SyncGlyph,
    title: &str,
    note: &str,
    enabled: bool,
    on: &mut bool,
) {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 46.0), egui::Sense::click());
    let hovered = enabled && response.hovered();
    let painter = ui.painter().clone();
    let checked = *on && enabled;

    let fill = if checked {
        CARD_ACTIVE()
    } else if hovered {
        CARD_HOVER()
    } else {
        CARD()
    };
    painter.rect(
        rect,
        egui::Rounding::same(9.0),
        fill,
        egui::Stroke::new(1.0_f32, if checked { BORDER() } else { CARD_HOVER() }),
    );

    // La case est peinte : elle garde la hauteur des lignes et se cale sur la
    // même grille que les marques.
    let case = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 20.0, rect.center().y),
        egui::vec2(17.0, 17.0),
    );
    if checked {
        painter.rect_filled(case, egui::Rounding::same(5.0), TEXT());
        let center = case.center();
        let stroke = egui::Stroke::new(1.9_f32, BACKGROUND());
        painter.line_segment(
            [
                egui::pos2(center.x - 4.2, center.y + 0.4),
                egui::pos2(center.x - 1.0, center.y + 3.6),
            ],
            stroke,
        );
        painter.line_segment(
            [
                egui::pos2(center.x - 1.0, center.y + 3.6),
                egui::pos2(center.x + 4.4, center.y - 3.4),
            ],
            stroke,
        );
    } else {
        painter.rect(
            case,
            egui::Rounding::same(5.0),
            egui::Color32::TRANSPARENT,
            egui::Stroke::new(1.2_f32, BORDER()),
        );
    }

    let mark = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 46.0, rect.center().y),
        egui::vec2(16.0, 16.0),
    );
    let ink = if enabled { TEXT() } else { TEXT_DIM() };
    match glyph {
        // Le logo Steam se dimensionne en 11,4 × k : il lui faut la même
        // hauteur que les marques de boutique, pas la moitié.
        SyncGlyph::Steam => paint_steam_icon(&painter, mark.center(), ink, 1.28),
        SyncGlyph::Store(store) => paint_store_mark_in(&painter, store, mark, ink),
        SyncGlyph::Disk => paint_disk_icon(&painter, mark.center(), ink, 0.95),
    }

    painter.text(
        egui::pos2(rect.left() + 64.0, rect.center().y - 8.0),
        egui::Align2::LEFT_CENTER,
        title,
        egui::FontId::proportional(13.0),
        ink,
    );
    painter.text(
        egui::pos2(rect.left() + 64.0, rect.center().y + 9.0),
        egui::Align2::LEFT_CENTER,
        note,
        egui::FontId::proportional(10.5),
        TEXT_DIM(),
    );

    if enabled && response.clicked() {
        *on = !*on;
    }
    if enabled {
        response.on_hover_cursor(egui::CursorIcon::PointingHand);
    }
}

impl LapinuxApp {
    /// La fenêtre de synchronisation, au centre de l'application : on y coche
    /// les sources à relire, chacune avec ce qu'elle contient.
    fn sync_dialog_ui(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.sync_dialog.take() else {
            return;
        };
        let screen = ctx.screen_rect();
        let mut close = false;
        let mut run = false;

        // Le voile assombrit la page et reçoit le clic qui referme.
        egui::Area::new(egui::Id::new("sync-dialog-veil"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                let (rect, response) = ui.allocate_exact_size(screen.size(), egui::Sense::click());
                ui.painter().rect_filled(
                    rect,
                    egui::Rounding::ZERO,
                    egui::Color32::from_black_alpha(150),
                );
                if response.clicked() {
                    close = true;
                }
            });
        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            close = true;
        }

        let steam_count = self
            .catalog
            .as_ref()
            .map(|catalog| catalog.entries.len())
            .unwrap_or(0);
        let steam_note = match self.catalog.as_ref() {
            None => "catalogue jamais relu".to_string(),
            Some(catalog) => {
                let age = lapinux_core::util::now_unix().saturating_sub(catalog.updated);
                if catalog.updated <= 0 {
                    "catalogue jamais relu".to_string()
                } else if age < 90 {
                    format!("{steam_count} jeux · relu à l'instant")
                } else if age < 3600 {
                    format!("{steam_count} jeux · relu il y a {} min", age / 60)
                } else if age < 86_400 {
                    format!("{steam_count} jeux · relu il y a {} h", age / 3600)
                } else {
                    format!("{steam_count} jeux · relu il y a {} j", age / 86_400)
                }
            }
        };

        let mut store_counts = [0usize; 3];
        let mut store_ready = [false; 3];
        for library in &self.stores.view.libraries {
            if let Some(slot) = lapinux_core::stores::Store::EXTERNAL
                .iter()
                .position(|store| *store == library.store)
            {
                store_counts[slot] = library.entries.len();
            }
        }
        for account in &self.stores.view.accounts {
            if account.connected {
                if let Some(slot) = lapinux_core::stores::Store::EXTERNAL
                    .iter()
                    .position(|store| *store == account.store)
                {
                    store_ready[slot] = true;
                }
            }
        }

        egui::Window::new("Synchronisation")
            .id(egui::Id::new("sync-dialog-window"))
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .order(egui::Order::Foreground)
            .frame(
                egui::Frame::none()
                    .fill(PANEL())
                    .stroke(egui::Stroke::new(1.0_f32, BORDER()))
                    .rounding(egui::Rounding::same(14.0))
                    .inner_margin(egui::Margin::same(18.0)),
            )
            .show(ctx, |ui| {
                ui.set_width(420.0);
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new("Synchroniser la bibliothèque")
                                .size(16.0)
                                .strong()
                                .color(TEXT()),
                        );
                        ui.add_space(3.0);
                        ui.label(
                            egui::RichText::new(
                                "Cochez ce que Lapinux relit. Le reste de la bibliothèque est gardé tel quel.",
                            )
                            .size(11.0)
                            .color(TEXT_DIM()),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                        let button = egui::Button::new(
                            egui::RichText::new("✕").size(13.0).color(TEXT_DIM()),
                        )
                        .frame(false)
                        .min_size(egui::vec2(24.0, 24.0));
                        if ui.add(button).on_hover_text("Fermer").clicked() {
                            close = true;
                        }
                    });
                });
                ui.add_space(14.0);

                sync_row(
                    ui,
                    SyncGlyph::Steam,
                    "Compte Steam",
                    &steam_note,
                    true,
                    &mut dialog.steam,
                );
                ui.add_space(6.0);
                for slot in 0..3 {
                    let store = lapinux_core::stores::Store::EXTERNAL[slot];
                    let ready = store_ready[slot];
                    // Une boutique sans compte n'a rien à relire : sa ligne le
                    // dit et reste décochée.
                    if !ready {
                        dialog.stores[slot] = false;
                    }
                    let note = if !ready {
                        "compte non connecté".to_string()
                    } else if store_counts[slot] == 0 {
                        "aucun jeu en cache · relire".to_string()
                    } else {
                        format!("{} jeux en cache", store_counts[slot])
                    };
                    sync_row(
                        ui,
                        SyncGlyph::Store(store),
                        store.label(),
                        &note,
                        ready,
                        &mut dialog.stores[slot],
                    );
                    ui.add_space(6.0);
                }
                sync_row(
                    ui,
                    SyncGlyph::Disk,
                    "Disque",
                    &format!("{} jeux installés · scan local", dialog.disk_count),
                    true,
                    &mut dialog.disk,
                );

                ui.add_space(14.0);
                ui.separator();
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let all = dialog.steam && dialog.disk && dialog.stores.iter().all(|on| *on);
                    let label = if all {
                        "Tout désélectionner"
                    } else {
                        "Tout sélectionner"
                    };
                    let link = egui::Button::new(
                        egui::RichText::new(label).size(11.5).color(TEXT_DIM()),
                    )
                    .frame(false);
                    if ui.add(link).clicked() {
                        let value = !all;
                        dialog.steam = value;
                        dialog.disk = value;
                        for slot in 0..3 {
                            dialog.stores[slot] = value && store_ready[slot];
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_enabled_ui(dialog.any(), |ui| {
                            if primary_button(ui, "Synchroniser").clicked() {
                                run = true;
                            }
                        });
                        ui.add_space(8.0);
                        if secondary_button(ui, "Annuler").clicked() {
                            close = true;
                        }
                    });
                });
            });

        if run {
            self.run_library_sync(ctx, dialog.steam, dialog.disk, dialog.stores);
        } else if !close {
            self.sync_dialog = Some(dialog);
        }
    }

    /// Lance les relectures cochées : chaque source reste indépendante, et le
    /// catalogue Steam n'est relu que si on l'a demandé.
    fn run_library_sync(
        &mut self,
        ctx: &egui::Context,
        steam: bool,
        disk: bool,
        stores: [bool; 3],
    ) {
        self.library_sync = Some(Instant::now());
        if disk {
            // Le scan local ne voit pas un jeu acheté mais pas encore installé :
            // la relève du disque reste un scan complet, en tâche de fond.
            if self.refresh_rx.is_some() {
                self.rescan_after_refresh = true;
            } else {
                self.refresh_async(ctx, true);
            }
        }
        let only: Vec<lapinux_core::stores::Store> = lapinux_core::stores::Store::EXTERNAL
            .iter()
            .copied()
            .enumerate()
            .filter(|(slot, _)| stores[*slot])
            .map(|(_, store)| store)
            .collect();
        if !only.is_empty() {
            self.stores.forget();
            self.stores.sync_stores(&self.client, only.clone());
        }
        if steam {
            // Une relève demandée à la main relit tout le catalogue : elle sert
            // justement quand la lecture ciblée d'une clé ne suffit pas.
            self.catalog_notice = true;
            self.catalog_patch = None;
            self.load_catalog(true);
        }
        self.library_checked = false;
        self.library_check_at = Some(Instant::now());
        let mut parts: Vec<&str> = Vec::new();
        if steam {
            parts.push("catalogue Steam");
        }
        if !only.is_empty() {
            parts.push("boutiques");
        }
        if disk {
            parts.push("disque");
        }
        let message = format!("Relu : {}", parts.join(", "));
        self.notify(message);
    }
}

impl LapinuxApp {
    /// Surveille la liaison avec le daemon, hors du fil de peinture : le
    /// message arrive une fois la réponse lue, et la perte comme le retour
    /// sont annoncés une seule fois. Sans cela, le service qui tombe laissait
    /// l'application muette, avec un état seulement visible dans Réglages.
    fn watch_daemon(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.daemon_watch_rx {
            match rx.try_recv() {
                Ok(alive) => {
                    self.daemon_watch_rx = None;
                    if self.daemon_seen_once && alive != self.daemon_seen {
                        self.notify(if alive {
                            "Lapinux : service reconnecté"
                        } else {
                            "Lapinux : connexion au service perdue"
                        });
                    }
                    self.daemon_seen = alive;
                    self.daemon_seen_once = true;
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => self.daemon_watch_rx = None,
            }
        }
        if self.daemon_watch_rx.is_none() && self.daemon_watch.elapsed() > Duration::from_secs(3) {
            self.daemon_watch = Instant::now();
            let (tx, rx) = mpsc::channel();
            self.daemon_watch_rx = Some(rx);
            // Le sondage a son propre fil : un daemon figé ne retarde pas la
            // peinture, dont le délai d'attente est de 600 ms.
            let client = self.client.detached();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let alive = client.daemon_build().is_some();
                let _ = tx.send(alive);
                ctx.request_repaint();
            });
        }
    }
}

#[cfg(test)]
mod cloud_sync_tests {
    use super::*;
    use lapinux_core::model::{
        CloudFile, CloudReport, CloudSaves, CloudSyncDirection, CloudSyncEntry,
    };

    fn report_with(local: Option<i64>, syncs: Vec<CloudSyncEntry>) -> CloudReport {
        CloudReport {
            saves: CloudSaves {
                files: vec![CloudFile {
                    name: "save.sav".into(),
                    size: 10,
                    modified: 1_700_000_000,
                    local,
                    ..Default::default()
                }],
                ..Default::default()
            },
            syncs,
            ..Default::default()
        }
    }

    #[test]
    fn a_journal_does_not_prove_an_unknown_local_state() {
        let report = report_with(
            None,
            vec![CloudSyncEntry {
                at: 1_700_000_100,
                direction: CloudSyncDirection::Download,
                machine: "Steam Cloud".into(),
                files: 1,
                bytes: 10,
            }],
        );
        assert_eq!(
            cloud_sync_label(&report),
            lapinux_core::i18n::t("sheet_saves.state_unknown")
        );
    }

    #[test]
    fn an_unresolved_local_path_remains_unknown() {
        let report = report_with(None, Vec::new());
        assert_eq!(
            cloud_sync_label(&report),
            lapinux_core::i18n::t("sheet_saves.state_unknown")
        );
    }

    #[test]
    fn a_local_timestamp_alone_does_not_prove_sync() {
        let report = report_with(Some(1_700_000_000), Vec::new());
        assert_eq!(
            cloud_sync_label(&report),
            lapinux_core::i18n::t("sheet_saves.state_unknown")
        );
    }
}

#[cfg(test)]
mod hover_focus_tests {
    use super::*;

    /// Le cadre 616×353 garde toute la capsule large : le point focal ne
    /// change rien quand l'image a déjà le bon format.
    #[test]
    fn a_capsule_matching_the_frame_is_not_cropped() {
        let uv = cover_uv_at(
            egui::vec2(616.0, 353.0),
            616.0 / 353.0,
            egui::vec2(0.7, 0.3),
        );
        assert!((uv.width() - 1.0).abs() < 1e-5);
        assert!((uv.height() - 1.0).abs() < 1e-5);
    }

    /// Une bannière 460×215, plus large que le cadre, perd des bandes
    /// latérales ; un point focal décalé garde le sujet du côté demandé.
    #[test]
    fn a_wide_banner_keeps_the_focal_subject() {
        let size = egui::vec2(460.0, 215.0);
        let ratio = 616.0 / 353.0;
        let visible = ratio / (size.x / size.y);
        let left = cover_uv_at(size, ratio, egui::vec2(0.0, 0.5));
        let right = cover_uv_at(size, ratio, egui::vec2(1.0, 0.5));
        assert!(left.left() < 0.01);
        assert!((right.right() - 1.0).abs() < 1e-5);
        assert!((left.width() - visible).abs() < 1e-5);
    }

    /// Le point focal reste dans l'image, même demandé hors bornes.
    #[test]
    fn a_focus_outside_the_image_is_clamped() {
        let uv = cover_uv_at(
            egui::vec2(1920.0, 620.0),
            616.0 / 353.0,
            egui::vec2(4.0, -2.0),
        );
        assert!(uv.left() >= 0.0 && uv.right() <= 1.0);
        assert!(uv.top() >= 0.0 && uv.bottom() <= 1.0);
    }

    /// Les captures lues chez une boutique alimentent le diaporama, bornées
    /// comme celles de Steam.
    #[test]
    fn store_screenshots_feed_the_hover_slideshow() {
        let mut info = StoreInfo::default();
        info.data.insert(
            7,
            lapinux_core::stores::StoreDetails {
                screenshots: vec![
                    "https://cdn.example/1.jpg".into(),
                    "https://cdn.example/2.jpg".into(),
                    "https://cdn.example/3.jpg".into(),
                    "https://cdn.example/4.jpg".into(),
                ],
                ..Default::default()
            },
        );
        assert_eq!(info.screenshots(7).len(), HOVER_SHOT_LIMIT);
        assert!(info.screenshots(8).is_empty());
    }

    /// Le point d'intérêt se pose sur la zone contrastée, pas au centre.
    #[test]
    fn the_focus_follows_the_contrasted_area() {
        let mut image = image::RgbaImage::from_pixel(160, 90, image::Rgba([10, 10, 10, 255]));
        for y in 20..70 {
            for x in 110..150 {
                image.put_pixel(x, y, image::Rgba([240, 240, 240, 255]));
            }
        }
        let path = std::env::temp_dir().join(format!("lapinux-focus-{}.png", std::process::id()));
        image.save(&path).unwrap();
        let focus = image_focus(&path).unwrap();
        std::fs::remove_file(&path).ok();
        assert!(focus.x > 0.6, "{focus:?}");
        assert!((focus.y - 0.5).abs() < 0.12, "{focus:?}");
    }
}

/// Libellé d'un axe de regroupement.
fn collection_field_label(field: lapinux_core::config::CollectionField) -> &'static str {
    match field {
        lapinux_core::config::CollectionField::Publisher => {
            lapinux_core::i18n::t("collections.publisher")
        }
        lapinux_core::config::CollectionField::Developer => {
            lapinux_core::i18n::t("collections.developer")
        }
        lapinux_core::config::CollectionField::Store => lapinux_core::i18n::t("collections.store"),
        lapinux_core::config::CollectionField::Genre => lapinux_core::i18n::t("collections.genre"),
        lapinux_core::config::CollectionField::Year => lapinux_core::i18n::t("collections.year"),
    }
}

/// « 1 jeu » ou « 3 jeux », sans laisser la quantité au pluriel.
fn collection_games_label(count: usize) -> &'static str {
    if count == 1 {
        lapinux_core::i18n::t("collections.games_one")
    } else {
        lapinux_core::i18n::t("collections.games_many")
    }
}

/// Jeux d'une collection : les appartenances enregistrées pour une catégorie
/// manuelle, ou tout le catalogue pour un regroupement dynamique.
fn collection_members<'a>(
    collection: &lapinux_core::config::GameCollection,
    entries: &'a [LibraryEntry],
) -> Vec<&'a LibraryEntry> {
    match collection.field {
        None => entries
            .iter()
            .filter(|entry| collection.appids.contains(&entry.appid))
            .collect(),
        Some(_) => entries.iter().collect(),
    }
}

/// Ligne secondaire d'une tuile de collection.
fn collection_subtitle(collection: &lapinux_core::config::GameCollection, count: usize) -> String {
    match collection.field {
        None => format!("{} {}", count, collection_games_label(count)),
        Some(field) => format!(
            "{} · {}",
            lapinux_core::i18n::t("collections.auto"),
            collection_field_label(field)
        ),
    }
}

/// Nom proposé pour une collection dynamique créée sans nom.
fn collection_field_default_name(field: lapinux_core::config::CollectionField) -> &'static str {
    match field {
        lapinux_core::config::CollectionField::Publisher => {
            lapinux_core::i18n::t("collections.by_publisher")
        }
        lapinux_core::config::CollectionField::Developer => {
            lapinux_core::i18n::t("collections.by_developer")
        }
        lapinux_core::config::CollectionField::Store => {
            lapinux_core::i18n::t("collections.by_store")
        }
        lapinux_core::config::CollectionField::Genre => {
            lapinux_core::i18n::t("collections.by_genre")
        }
        lapinux_core::config::CollectionField::Year => lapinux_core::i18n::t("collections.by_year"),
    }
}

/// Groupes d'une collection dynamique, triés par nom. Un jeu co-édité ou
/// co-développé apparaît sous chacun de ses crédits, comme dans une fiche.
fn collection_groups<'a>(
    entries: &'a [LibraryEntry],
    field: lapinux_core::config::CollectionField,
) -> Vec<(String, Vec<&'a LibraryEntry>)> {
    use lapinux_core::config::CollectionField;
    let unknown_publisher = lapinux_core::i18n::t("collections.unknown_publisher");
    let unknown_developer = lapinux_core::i18n::t("collections.unknown_developer");
    let local_disk = lapinux_core::i18n::t("collections.local_disk");
    let unknown_genre = lapinux_core::i18n::t("collections.unknown_genre");
    let no_date = lapinux_core::i18n::t("collections.no_date");
    let mut groups: BTreeMap<String, Vec<&'a LibraryEntry>> = BTreeMap::new();
    for entry in entries {
        let mut names: Vec<String> = match field {
            CollectionField::Publisher => entry.publishers.clone(),
            CollectionField::Developer => entry.developers.clone(),
            CollectionField::Store => vec![match entry.store {
                Some(store) => store.label().to_string(),
                None if entry.steam => "Steam".to_string(),
                None => local_disk.to_string(),
            }],
            // Un jeu sans genre reste visible : il rejoint le groupe explicite
            // des fiches muettes plutôt que de disparaître de l'axe.
            CollectionField::Genre => entry.genres.clone(),
            // L'année vient de l'obtention ; les boutiques qui ne la datent
            // pas (GOG) se rabattent sur la date du build installé, et les
            // jeux sans aucune date forment le groupe « Sans date ».
            CollectionField::Year => {
                let stamp = entry
                    .acquired
                    .filter(|at| *at > 0)
                    .or_else(|| (entry.updated > 0).then_some(entry.updated));
                vec![match stamp {
                    Some(at) => year_from_unix(at).to_string(),
                    None => no_date.to_string(),
                }]
            }
        };
        if names.is_empty() {
            names.push(match field {
                CollectionField::Publisher => unknown_publisher.to_string(),
                CollectionField::Developer => unknown_developer.to_string(),
                CollectionField::Store => local_disk.to_string(),
                CollectionField::Genre => unknown_genre.to_string(),
                CollectionField::Year => no_date.to_string(),
            });
        }
        let mut seen = HashSet::new();
        for name in names {
            let name = name.trim().to_string();
            if name.is_empty() || !seen.insert(name.clone()) {
                continue;
            }
            groups.entry(name).or_default().push(entry);
        }
    }
    let mut groups: Vec<_> = groups.into_iter().collect();
    groups.sort_by(|a, b| {
        let a_unknown = a.0.as_str() == unknown_publisher
            || a.0.as_str() == unknown_developer
            || a.0.as_str() == local_disk
            || a.0.as_str() == unknown_genre
            || a.0.as_str() == no_date;
        let b_unknown = b.0.as_str() == unknown_publisher
            || b.0.as_str() == unknown_developer
            || b.0.as_str() == local_disk
            || b.0.as_str() == unknown_genre
            || b.0.as_str() == no_date;
        a_unknown
            .cmp(&b_unknown)
            .then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
    });
    groups
}

/// Côté d'une tuile de collection : le même gabarit carré sur la page
/// d'accueil, qu'il y ait une collection ou dix.
const COLLECTION_TILE: f32 = 164.0;

#[cfg(test)]
mod library_view_tests {
    use super::*;

    /// Tri, sources et filtre d'installation traversent la configuration
    /// sans perdre leur valeur : c'est ce qui les fait survivre au redémarrage.
    #[test]
    fn the_library_view_roundtrips_through_the_config() {
        let sources = SourceFilter {
            disk: false,
            steam: true,
            family: false,
            epic_pc: true,
            epic_mobile: false,
            stores: [true, false, true],
        };
        assert_eq!(SourceFilter::from_config(sources.to_config()), sources);
        for sort in [
            LibrarySort::Name,
            LibrarySort::Playtime,
            LibrarySort::Acquired,
            LibrarySort::Recent,
            LibrarySort::Updated,
            LibrarySort::Size,
        ] {
            assert_eq!(LibrarySort::from_config(sort.to_config()), sort);
        }
        for filter in [
            CatalogFilter::All,
            CatalogFilter::Installed,
            CatalogFilter::Missing,
        ] {
            assert_eq!(CatalogFilter::from_config(filter.to_config()), filter);
        }
    }

    #[test]
    fn collections_group_credits_and_put_unknowns_last() {
        let game = |appid: u32,
                    name: &str,
                    publishers: &[&str],
                    developers: &[&str],
                    store: Option<lapinux_core::stores::Store>,
                    steam: bool| LibraryEntry {
            appid,
            name: name.to_string(),
            installed: false,
            on_disk: false,
            steam,
            family: false,
            family_available: false,
            update: None,
            store,
            cover: String::new(),
            platforms: Vec::new(),
            mobile: false,
            size: None,
            publishers: publishers.iter().map(|value| value.to_string()).collect(),
            developers: developers.iter().map(|value| value.to_string()).collect(),
            genres: Vec::new(),
            playtime: 0,
            acquired: None,
            updated: 0,
            last_played: 0,
        };
        let entries = vec![
            game(1, "Half-Life", &["Valve", "Sierra"], &["Valve"], None, true),
            game(2, "Mystery", &[], &[], None, false),
        ];
        let publishers =
            collection_groups(&entries, lapinux_core::config::CollectionField::Publisher);
        assert_eq!(publishers.len(), 3);
        assert_eq!(publishers[0].0, "Sierra");
        assert_eq!(publishers[1].0, "Valve");
        assert_eq!(publishers[1].1[0].appid, 1);
        assert_eq!(
            publishers[2].0,
            lapinux_core::i18n::t("collections.unknown_publisher")
        );
        assert_eq!(publishers[2].1[0].appid, 2);

        let developers =
            collection_groups(&entries, lapinux_core::config::CollectionField::Developer);
        assert_eq!(developers[0].0, "Valve");
        assert_eq!(
            developers[1].0,
            lapinux_core::i18n::t("collections.unknown_developer")
        );

        let stores = collection_groups(&entries, lapinux_core::config::CollectionField::Store);
        assert_eq!(stores[0].0, "Steam");
        assert_eq!(stores[1].0, lapinux_core::i18n::t("collections.local_disk"));
    }

    /// Les deux axes récents couvrent toute la bibliothèque : un jeu sans
    /// genre ou sans date rejoint un groupe explicite, jamais le vide, et
    /// l'obtention absente se rabat sur l'année du build.
    #[test]
    fn collections_genre_and_year_axes_cover_every_game() {
        use lapinux_core::config::CollectionField;
        let game =
            |appid: u32, genres: &[&str], acquired: Option<i64>, updated: i64| LibraryEntry {
                appid,
                name: format!("Jeu {appid}"),
                installed: false,
                on_disk: false,
                steam: true,
                family: false,
                family_available: false,
                update: None,
                store: None,
                cover: String::new(),
                platforms: Vec::new(),
                mobile: false,
                size: None,
                publishers: Vec::new(),
                developers: Vec::new(),
                genres: genres.iter().map(|value| value.to_string()).collect(),
                playtime: 0,
                acquired,
                updated,
                last_played: 0,
            };
        let entries = vec![
            game(1, &["Action", "RPG"], None, 0),
            game(2, &[], Some(1_600_000_000), 0),
            game(3, &[], None, 1_700_000_000),
            game(4, &[], None, 0),
        ];

        let genres = collection_groups(&entries, CollectionField::Genre);
        let unknown_genre = lapinux_core::i18n::t("collections.unknown_genre");
        assert_eq!(
            genres
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            vec!["Action", "RPG", unknown_genre]
        );
        assert_eq!(genres[2].1.len(), 3);
        assert!(genres
            .iter()
            .all(|(name, games)| !name.is_empty() && !games.is_empty()));
        assert!(genres
            .iter()
            .any(|(_, games)| games.iter().any(|entry| entry.appid == 1)));

        let years = collection_groups(&entries, CollectionField::Year);
        let no_date = lapinux_core::i18n::t("collections.no_date");
        assert_eq!(
            years
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            vec!["2020", "2023", no_date]
        );
        assert_eq!(years[0].1[0].appid, 2);
        assert_eq!(
            years[1].1[0].appid, 3,
            "sans obtention, l'année du build prend le relais"
        );
        assert_eq!(years[2].1.len(), 2);
        let covered: usize = years.iter().map(|(_, games)| games.len()).sum();
        assert_eq!(covered, entries.len(), "aucun jeu ne doit sortir de l'axe");
    }
}

#[cfg(test)]
mod palette_tests {
    use super::*;

    /// Toutes les pages lisent la palette globale : basculer de thème change
    /// donc réellement leurs couleurs, sans reconstruire les écrans.
    #[test]
    fn the_active_palette_drives_every_colour_accessor() {
        let previous = active_palette();
        set_active_palette(theme::Palette::nx());
        assert_eq!(BACKGROUND(), egui::Color32::from_rgb(28, 28, 30));
        assert_eq!(PANEL(), egui::Color32::from_rgb(43, 43, 46));
        assert_eq!(ACCENT(), egui::Color32::from_rgb(0, 200, 224));
        assert_eq!(TEXT(), egui::Color32::from_rgb(244, 245, 247));
        set_active_palette(theme::Palette::classic());
        assert_eq!(BACKGROUND(), egui::Color32::from_rgb(20, 21, 22));
        assert_eq!(TEXT_DIM(), egui::Color32::from_rgb(160, 162, 166));
        set_active_palette(previous);
    }
}
