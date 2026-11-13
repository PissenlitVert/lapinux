//! Bandeau de succès : une carte posée en haut au centre de l'écran, au-dessus
//! du jeu, qui glisse depuis le haut et s'efface après quelques secondes.
//!
//! L'icône du succès arrive par un fichier déjà téléchargé par le daemon : ce
//! processus ne fait aucun réseau, il ne peint que ce qu'on lui donne. La
//! surface est une bande plein écran transparente aux clics et au clavier : le
//! jeu continue de recevoir la souris pendant l'affichage.

use crate::font;
use crate::render::{
    decode_bytes, decode_image, draw_text_baseline, draw_text_right_baseline, ease_out, fill_path,
    fill_round, fit_text, stroke_path, text_width, with_alpha,
};
use crate::shm::Mapping;
use lapinux_core::i18n::t;
use crate::{Error, Result};
use ab_glyph::FontVec;
use lapinux_core::stores::Store;
use std::os::fd::{AsRawFd, BorrowedFd};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tiny_skia::{FilterQuality, PathBuilder, Pixmap, PixmapPaint, Transform};
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_region, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

/// Largeur de la carte, plafonnée à l'écran : un rectangle large, pas une
/// vignette carrée.
const CARD_W: f32 = 680.0;
const CARD_H: f32 = 148.0;
/// Distance du haut de l'écran une fois posée.
const TOP: f32 = 18.0;
/// Hauteur de la bande Wayland : le bandeau, sa marge et la place de la glissade.
const AREA_H: u32 = 196;
/// Côté du carré de l'icône.
const ICON: f32 = 148.0;
/// Côté de l'icône du jeu dans l'en-tête.
const GAME_ICON: u32 = 32;
/// Hauteur de la marque de la boutique, et largeur maximale d'un lettrage.
const LOGO_H: f32 = 20.0;
const LOGO_W: f32 = 72.0;
/// Entrée et sortie du bandeau, en secondes.
const ENTER: f32 = 0.88;
const LEAVE: f32 = 1.04;

const TEXT: (u8, u8, u8, u8) = (234, 235, 238, 255);
const TEXT_DIM: (u8, u8, u8, u8) = (178, 184, 194, 255);

pub struct ToastOptions {
    pub title: String,
    /// Nom du jeu, rappelé au bord droit de l'en-tête.
    pub game: String,
    pub store: Store,
    /// Part des joueurs qui l'ont obtenu ; absente quand la boutique ne la
    /// publie pas.
    pub percent: Option<f32>,
    /// Icône déjà téléchargée, lue sur le disque.
    pub icon: Option<PathBuf>,
    /// Icône du jeu, posée à gauche de son nom dans l'en-tête.
    pub game_icon: Option<PathBuf>,
    /// Temps pendant lequel la carte reste posée, entrée et sortie exclues.
    pub hold: Duration,
    pub font: Option<PathBuf>,
    /// Écran Wayland cible (`DP-2`) : celui du jeu, quand il est connu.
    pub output: Option<String>,
}

impl Default for ToastOptions {
    fn default() -> Self {
        Self {
            title: t("overlay.toast_achievement").into(),
            game: String::new(),
            store: Store::Steam,
            percent: None,
            icon: None,
            game_icon: None,
            hold: Duration::from_secs(6),
            font: None,
            output: None,
        }
    }
}

impl ToastOptions {
    /// Durée totale du bandeau, entrée et sortie comprises.
    fn total(&self) -> f32 {
        ENTER + self.hold.as_secs_f32() + LEAVE
    }
}

/// Couleur de la boutique : elle colore l'en-tête et le halo de l'icône.
fn store_accent(store: Store) -> (u8, u8, u8, u8) {
    match store {
        Store::Steam => (102, 192, 244, 255),
        Store::Epic => (176, 168, 255, 255),
        Store::Gog => (196, 128, 250, 255),
        Store::Itch => (250, 137, 161, 255),
    }
}

/// Fond de la carte : un dégradé propre à la boutique (haut, bas). Assez
/// sombre pour le texte clair, mais teinté pour ne jamais paraître gris.
fn store_palette(store: Store) -> ((u8, u8, u8), (u8, u8, u8)) {
    match store {
        Store::Steam => ((44, 82, 112), (18, 34, 47)),
        Store::Epic => ((60, 55, 100), (26, 24, 44)),
        Store::Gog => ((72, 45, 94), (33, 21, 45)),
        Store::Itch => ((82, 47, 59), (37, 22, 28)),
    }
}

/// `42,3` : la virgule française, une décimale comme le reste de l'interface.
fn share(value: f32) -> String {
    format!("{value:.1}").replace('.', ",")
}

/// Marque blanche de la boutique, embarquée comme dans l'interface : le
/// processus du bandeau n'a aucun fichier à chercher au démarrage.
fn store_mark(store: Store) -> &'static [u8] {
    match store {
        Store::Steam => include_bytes!("../../../assets/steam.png"),
        Store::Epic => include_bytes!("../../../assets/stores/epic.png"),
        Store::Gog => include_bytes!("../../../assets/stores/gog.png"),
        Store::Itch => include_bytes!("../../../assets/stores/itch.png"),
    }
}

/// Ramène l'icône à un carré plein : l'illustration remplit le cadre, quitte
/// à rogner un bord, plutôt que de flotter dans une vignette.
fn cover(icon: &Pixmap, size: u32) -> Pixmap {
    let mut tile =
        Pixmap::new(size, size).unwrap_or_else(|| Pixmap::new(1, 1).expect("pixmap minimal"));
    let scale = (size as f32 / icon.width() as f32).max(size as f32 / icon.height() as f32);
    let width = icon.width() as f32 * scale;
    let height = icon.height() as f32 * scale;
    let paint = PixmapPaint {
        quality: FilterQuality::Bilinear,
        ..Default::default()
    };
    // Le décalage est dans la transformation : le rognage reste centré.
    let transform =
        Transform::from_translate((size as f32 - width) / 2.0, (size as f32 - height) / 2.0)
            .pre_scale(scale, scale);
    tile.draw_pixmap(0, 0, icon.as_ref(), &paint, transform, None);
    tile
}

/// Rogne les marges transparentes d'une marque : le centrage se fait sur le
/// dessin, pas sur un canevas plus haut que lui.
fn trim_mark(mark: &Pixmap) -> Pixmap {
    let (w, h) = (mark.width() as i32, mark.height() as i32);
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, -1, -1);
    for (index, pixel) in mark.data().chunks_exact(4).enumerate() {
        if pixel[3] < 8 {
            continue;
        }
        let x = index as i32 % w;
        let y = index as i32 / w;
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    if x1 < 0 || (x0 == 0 && y0 == 0 && x1 == w - 1 && y1 == h - 1) {
        return mark.clone();
    }
    let (cw, ch) = ((x1 - x0 + 1) as u32, (y1 - y0 + 1) as u32);
    let mut out = Pixmap::new(cw, ch).unwrap_or_else(|| Pixmap::new(1, 1).expect("pixmap minimal"));
    let src = mark.data();
    let dst = out.data_mut();
    for row in 0..ch as usize {
        let start = ((y0 as usize + row) * w as usize + x0 as usize) * 4;
        let end = start + cw as usize * 4;
        dst[row * cw as usize * 4..(row + 1) * cw as usize * 4].copy_from_slice(&src[start..end]);
    }
    out
}

/// Une image presque uniforme ne dit rien : un carré noir, une icône
/// transparente ou un aplat. Le seuil reste bas pour ne pas écarter une
/// illustration sombre mais dessinée.
fn is_blank(icon: &Pixmap) -> bool {
    let mut count = 0.0f64;
    let mut sum = 0.0f64;
    let mut squares = 0.0f64;
    for pixel in icon.data().chunks_exact(4) {
        if pixel[3] < 16 {
            continue;
        }
        // Les canaux sont prémultipliés : la luminance se ramène à l'opacité.
        let alpha = pixel[3] as f64 / 255.0;
        let lum = (pixel[0] as f64 * 0.2126 + pixel[1] as f64 * 0.7152 + pixel[2] as f64 * 0.0722)
            / alpha.max(0.01);
        sum += lum;
        squares += lum * lum;
        count += 1.0;
    }
    if count < 1.0 {
        return true;
    }
    let mean = sum / count;
    (squares / count - mean * mean).max(0.0).sqrt() < 3.0
}

struct ToastRenderer {
    font: FontVec,
    icon: Option<Pixmap>,
    game_icon: Option<Pixmap>,
    /// Marque blanche de la boutique, embarquée dans le binaire.
    logo: Option<Pixmap>,
}

impl ToastRenderer {
    fn new(
        font_path: &Path,
        icon: Option<&Path>,
        game_icon: Option<&Path>,
        store: Store,
    ) -> Result<Self> {
        let bytes = std::fs::read(font_path)?;
        let font = FontVec::try_from_vec(bytes)
            .map_err(|e| Error::Other(format!("police illisible : {e}")))?;
        let mut icon = icon.and_then(decode_image);
        if icon.as_ref().is_some_and(is_blank) {
            icon = None;
        }
        let icon = icon.map(|icon| cover(&icon, ICON as u32));
        let game_icon = game_icon
            .and_then(decode_image)
            .map(|icon| cover(&icon, GAME_ICON));
        Ok(Self {
            font,
            icon,
            game_icon,
            // La marque garde ses couleurs : le disque noir de Steam se fond
            // dans la carte et laisse voir son glyphe clair, tandis que les
            // autres boutiques sont déjà blanches sur fond transparent.
            logo: decode_bytes(store_mark(store)).map(|mark| trim_mark(&mark)),
        })
    }

    /// Peint une image du bandeau, à `elapsed` secondes depuis son apparition.
    fn render(&self, width: u32, height: u32, toast: &ToastOptions, elapsed: f32) -> Pixmap {
        let mut pixmap = Pixmap::new(width.max(1), height.max(1))
            .unwrap_or_else(|| Pixmap::new(320, 200).expect("pixmap minimal"));
        // L'icône tombe au centre de l'écran, puis glisse à gauche pour
        // découvrir le panneau ; la sortie refait le trajet à l'envers.
        let drop = ease_out(((elapsed - 0.03) / 0.34).clamp(0.0, 1.0));
        let shift = ease_out(((elapsed - 0.03 - 0.32) / 0.40).clamp(0.0, 1.0));
        let panel_in = ease_out(((elapsed - 0.03 - 0.48) / 0.26).clamp(0.0, 1.0));
        let out =
            ease_out(((elapsed - 0.03 - ENTER - toast.hold.as_secs_f32()) / LEAVE).clamp(0.0, 1.0));
        // La sortie rejoue l'entrée : retour au centre pendant que le panneau
        // s'efface, puis remontée hors de l'écran. L'icône ne s'efface pas en
        // route, sinon la marche arrière ne se voit pas.
        // D'abord les écritures s'effacent (0,30 s), puis l'icône rejoue
        // l'entrée à l'envers : 0,40 s vers le centre, 0,34 s de remontée.
        let exit = out * LEAVE;
        let text_out = ease_out((exit / 0.30).clamp(0.0, 1.0));
        let back = ease_out(((exit - 0.30) / 0.40).clamp(0.0, 1.0));
        let up = ease_out(((exit - 0.70) / 0.34).clamp(0.0, 1.0));
        let icon_alpha = drop;
        let fade = panel_in * (1.0 - text_out);
        if icon_alpha <= 0.001 && fade <= 0.001 {
            return pixmap;
        }
        let accent = store_accent(toast.store);
        let (_, bottom) = store_palette(toast.store);
        let alpha = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;

        let card_w = CARD_W.min((width as f32 - 32.0).max(240.0));
        let x = ((width as f32 - card_w) / 2.0).max(0.0);
        let center_x = ((width as f32 - ICON) / 2.0).max(0.0);
        let side = shift * (1.0 - back);
        let icon_x = center_x + (x - center_x) * side;
        let icon_y = -ICON + (TOP + ICON) * drop - up * (TOP + ICON + 30.0);
        let y = TOP;

        // Un panneau rectangulaire derrière les écritures, à plat : pas de
        // carte englobante qui fait ressembler le bandeau à un lecteur.
        let panel_x = x + ICON + 14.0;
        let panel_w = card_w - ICON - 14.0;
        fill_round(
            &mut pixmap,
            panel_x,
            y,
            panel_w,
            CARD_H,
            0.0,
            (bottom.0, bottom.1, bottom.2, alpha(0.94 * fade)),
        );
        // Filet de la boutique au bord du panneau : la seule couleur du fond.
        fill_round(
            &mut pixmap,
            panel_x,
            y,
            3.0,
            CARD_H,
            0.0,
            with_alpha(accent, alpha(0.9 * fade)),
        );
        // Icône : l'illustration seule, à plat, sans cadre ni vignette. Elle
        // remplit son carré, sinon elle semble posée dans un lecteur embarqué.
        match &self.icon {
            Some(icon) => {
                let paint = PixmapPaint {
                    opacity: icon_alpha,
                    quality: FilterQuality::Bilinear,
                    ..Default::default()
                };
                pixmap.draw_pixmap(
                    icon_x as i32,
                    icon_y as i32,
                    icon.as_ref(),
                    &paint,
                    Transform::identity(),
                    None,
                );
            }
            None => {
                fill_round(
                    &mut pixmap,
                    icon_x,
                    icon_y,
                    ICON,
                    ICON,
                    0.0,
                    (bottom.0, bottom.1, bottom.2, alpha(0.9 * icon_alpha)),
                );
                draw_trophy(
                    &mut pixmap,
                    icon_x + 24.0,
                    icon_y + 24.0,
                    ICON - 48.0,
                    TEXT,
                    icon_alpha,
                );
            }
        }
        // Pastille de déblocage au coin de l'icône : un anneau de la couleur
        // de la carte la sépare du cadre, puis la coche dit le succès obtenu.
        let badge_x = icon_x + ICON - 5.0;
        let badge_y = icon_y + ICON - 5.0;
        fill_round(
            &mut pixmap,
            badge_x - 12.0,
            badge_y - 12.0,
            24.0,
            24.0,
            12.0,
            (bottom.0, bottom.1, bottom.2, alpha(0.95 * icon_alpha)),
        );
        let mut badge = PathBuilder::new();
        badge.push_circle(badge_x, badge_y, 9.5);
        fill_path(&mut pixmap, badge, with_alpha(accent, alpha(icon_alpha)));
        let mut check = PathBuilder::new();
        check.move_to(badge_x - 4.0, badge_y + 0.2);
        check.line_to(badge_x - 1.2, badge_y + 3.0);
        check.line_to(badge_x + 4.2, badge_y - 3.0);
        stroke_path(
            &mut pixmap,
            check,
            (bottom.0, bottom.1, bottom.2, alpha(0.95 * icon_alpha)),
            2.0,
        );

        let text_x = panel_x + 20.0;
        let right = panel_x + panel_w - 20.0;
        // En-tête : le jeu à gauche, la boutique collée au bord droit.
        let header = y + 40.0;
        // Les textes entrent l'un après l'autre, en glissant vers le haut.
        let header_in = ease_out(((elapsed - 0.46) / 0.22).clamp(0.0, 1.0));
        let title_in = ease_out(((elapsed - 0.54) / 0.24).clamp(0.0, 1.0));
        let percent_in = ease_out(((elapsed - 0.62) / 0.24).clamp(0.0, 1.0));
        // À la sortie, tout le texte monte pour partir : un fondu sur place
        // laissait le nom de la plateforme flotter au bord du panneau.
        let lift = text_out * 10.0;
        let header_y = header + (1.0 - header_in) * 10.0 - lift;
        let platform = toast.store.label().to_uppercase();
        let platform_w = text_width(&self.font, &platform, 15.0);
        let mut platform_block = platform_w;
        if let Some(logo) = &self.logo {
            let scale = (LOGO_H / logo.height() as f32).min(LOGO_W / logo.width() as f32);
            let width = logo.width() as f32 * scale;
            let height = logo.height() as f32 * scale;
            let paint = PixmapPaint {
                opacity: fade * header_in * 0.95,
                quality: FilterQuality::Bilinear,
                ..Default::default()
            };
            let logo_x = right - platform_w - 8.0 - width;
            // Position comprise dans la transformation, et centrée sur la
            // ligne du nom : la marque ne flotte pas plus haut.
            let transform = Transform::from_translate(logo_x, header_y - height / 2.0 - 4.5)
                .pre_scale(scale, scale);
            pixmap.draw_pixmap(0, 0, logo.as_ref(), &paint, transform, None);
            platform_block += width + 8.0;
        } else {
            platform_block += 9.0;
        }
        draw_text_right_baseline(
            &mut pixmap,
            &self.font,
            &platform,
            right,
            header_y,
            15.0,
            (232, 235, 240, alpha(0.95 * fade * header_in)),
        );
        // Jeu : son icône puis son nom, à gauche de la boutique.
        if !toast.game.trim().is_empty() {
            let icon_w = if self.game_icon.is_some() {
                GAME_ICON as f32 + 8.0
            } else {
                0.0
            };
            let room = right - platform_block - 18.0 - text_x - icon_w;
            if room > 40.0 {
                let game = fit_text(&self.font, &toast.game, room, 21.0);
                let mut after = text_x;
                if let Some(icon) = &self.game_icon {
                    let paint = PixmapPaint {
                        opacity: fade * 0.95,
                        quality: FilterQuality::Bilinear,
                        ..Default::default()
                    };
                    let size = GAME_ICON as f32;
                    pixmap.draw_pixmap(
                        text_x as i32,
                        (header_y - size / 2.0 - 7.0) as i32,
                        icon.as_ref(),
                        &paint,
                        Transform::identity(),
                        None,
                    );
                    after += size + 8.0;
                }
                draw_text_baseline(
                    &mut pixmap,
                    &self.font,
                    &game,
                    after,
                    header_y,
                    21.0,
                    with_alpha((214, 218, 226, 255), alpha(0.95 * fade * header_in)),
                );
            }
        }
        let title = fit_text(&self.font, &toast.title, (right - text_x).max(40.0), 36.0);
        let title_y = y + 90.0 + (1.0 - title_in) * 14.0 - lift;
        draw_text_baseline(
            &mut pixmap,
            &self.font,
            &title,
            text_x,
            title_y,
            36.0,
            with_alpha(TEXT, alpha(fade * title_in)),
        );
        let percent_y = y + 126.0 + (1.0 - percent_in) * 12.0 - lift;
        match toast.percent {
            Some(percent) => {
                // Le nombre prend la couleur de la boutique, la phrase reste
                // discrète : la rareté se lit d'un coup d'œil.
                let value = format!("{} %", share(percent));
                draw_text_baseline(
                    &mut pixmap,
                    &self.font,
                    &value,
                    text_x,
                    percent_y,
                    22.0,
                    with_alpha(accent, alpha(0.95 * fade * percent_in)),
                );
                let after = text_x + text_width(&self.font, &value, 22.0) + 6.0;
                draw_text_baseline(
                    &mut pixmap,
                    &self.font,
                    t("overlay.toast_players"),
                    after,
                    percent_y,
                    17.0,
                    with_alpha(TEXT_DIM, alpha(0.9 * fade * percent_in)),
                );
            }
            None => draw_text_baseline(
                &mut pixmap,
                &self.font,
                t("overlay.toast_new_achievement"),
                text_x,
                percent_y,
                17.0,
                with_alpha(TEXT_DIM, alpha(0.9 * fade * percent_in)),
            ),
        }
        pixmap
    }
}

/// Trophée au trait, dessiné quand la boutique ne fournit aucune icône.
fn draw_trophy(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    size: f32,
    color: (u8, u8, u8, u8),
    alpha: f32,
) {
    let color = with_alpha(color, (alpha.clamp(0.0, 1.0) * 255.0).round() as u8);
    let s = size / 24.0;
    let cx = x + size / 2.0;
    let cy = y + size / 2.0;
    // Coupe, anses, pied et socle : le même pictogramme que la fiche.
    fill_round(
        pixmap,
        cx - 8.0 * s,
        cy - 9.0 * s,
        16.0 * s,
        10.0 * s,
        2.5 * s,
        color,
    );
    for direction in [-1.0f32, 1.0] {
        let mut handle = PathBuilder::new();
        handle.move_to(cx + direction * 8.0 * s, cy - 7.0 * s);
        handle.quad_to(
            cx + direction * 13.0 * s,
            cy - 7.0 * s,
            cx + direction * 13.0 * s,
            cy - 3.0 * s,
        );
        handle.quad_to(
            cx + direction * 13.0 * s,
            cy + 1.5 * s,
            cx + direction * 8.0 * s,
            cy + 1.0 * s,
        );
        stroke_path(pixmap, handle, color, 1.6 * s);
    }
    fill_round(
        pixmap,
        cx - 1.5 * s,
        cy + 0.5 * s,
        3.0 * s,
        6.0 * s,
        1.2 * s,
        color,
    );
    fill_round(
        pixmap,
        cx - 6.5 * s,
        cy + 6.5 * s,
        13.0 * s,
        3.0 * s,
        1.2 * s,
        color,
    );
}

struct Slot {
    buffer: wl_buffer::WlBuffer,
    mapping: Mapping,
    released: Arc<AtomicBool>,
    width: u32,
    height: u32,
}

struct ToastApp {
    shm: wl_shm::WlShm,
    qh: QueueHandle<ToastApp>,
    surface: Option<wl_surface::WlSurface>,
    layer: Option<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1>,
    width: u32,
    height: u32,
    configured: bool,
    slots: Vec<Slot>,
    renderer: ToastRenderer,
    toast: ToastOptions,
    started: Instant,
    exit: bool,
}

impl ToastApp {
    fn draw(&mut self, conn: &Connection) -> Result<()> {
        if !self.configured {
            return Ok(());
        }
        let width = self.width.max(1);
        let height = self.height.max(1);
        let pixmap = self.renderer.render(
            width,
            height,
            &self.toast,
            self.started.elapsed().as_secs_f32(),
        );
        let stride = width as usize * 4;
        let size = stride * height as usize;

        let slot_index = self
            .slots
            .iter()
            .position(|slot| {
                slot.width == width
                    && slot.height == height
                    && slot.released.load(Ordering::Relaxed)
            })
            .or_else(|| {
                if self.slots.len() < 2 {
                    let released = Arc::new(AtomicBool::new(true));
                    let mapping = match Mapping::new(size) {
                        Ok(mapping) => mapping,
                        Err(err) => {
                            tracing::warn!("{err}");
                            return None;
                        }
                    };
                    let fd = unsafe { BorrowedFd::borrow_raw(mapping.fd.as_raw_fd()) };
                    let pool = self.shm.create_pool(fd, size as i32, &self.qh, ());
                    let buffer = pool.create_buffer(
                        0,
                        width as i32,
                        height as i32,
                        stride as i32,
                        wl_shm::Format::Argb8888,
                        &self.qh,
                        Arc::clone(&released),
                    );
                    pool.destroy();
                    self.slots.push(Slot {
                        buffer,
                        mapping,
                        released,
                        width,
                        height,
                    });
                    Some(self.slots.len() - 1)
                } else {
                    None
                }
            });

        let Some(slot_index) = slot_index else {
            return Ok(());
        };
        let Some(surface) = &self.surface else {
            return Ok(());
        };
        let slot = &mut self.slots[slot_index];
        slot.released.store(false, Ordering::Relaxed);
        slot.mapping.write_rgba(pixmap.data());
        surface.attach(Some(&slot.buffer), 0, 0);
        surface.damage(0, 0, width as i32, height as i32);
        surface.commit();
        conn.flush().map_err(|e| Error::Wayland(e.to_string()))?;
        Ok(())
    }
}

macro_rules! ignore_events {
    ($iface:ty) => {
        impl Dispatch<$iface, ()> for ToastApp {
            fn event(
                _: &mut Self,
                _: &$iface,
                _: <$iface as Proxy>::Event,
                _: &(),
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
            }
        }
    };
}

ignore_events!(wl_compositor::WlCompositor);
ignore_events!(wl_region::WlRegion);
ignore_events!(wl_surface::WlSurface);
ignore_events!(wl_shm::WlShm);
ignore_events!(wl_shm_pool::WlShmPool);
ignore_events!(zwlr_layer_shell_v1::ZwlrLayerShellV1);

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for ToastApp {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: <wl_registry::WlRegistry as Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_buffer::WlBuffer, Arc<AtomicBool>> for ToastApp {
    fn event(
        _: &mut Self,
        _: &wl_buffer::WlBuffer,
        event: <wl_buffer::WlBuffer as Proxy>::Event,
        released: &Arc<AtomicBool>,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event {
            released.store(true, Ordering::Relaxed);
        }
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for ToastApp {
    fn event(
        app: &mut Self,
        _: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: <zwlr_layer_surface_v1::ZwlrLayerSurfaceV1 as Proxy>::Event,
        _: &(),
        conn: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => {
                if let Some(layer) = &app.layer {
                    layer.ack_configure(serial);
                }
                if width > 0 {
                    app.width = width;
                }
                if height > 0 {
                    app.height = height;
                }
                app.configured = true;
                let _ = app.draw(conn);
            }
            zwlr_layer_surface_v1::Event::Closed => app.exit = true,
            _ => {}
        }
    }
}

/// Affiche un bandeau de succès puis rend la main. Le processus est jetable :
/// le daemon en lance un par succès, ce qui sérialise leur affichage.
pub fn run(options: ToastOptions) -> Result<()> {
    let font_path = match options.font.clone() {
        Some(path) => path,
        None => font::find_font()?,
    };
    let conn = Connection::connect_to_env()
        .map_err(|e| Error::Wayland(format!("connexion Wayland : {e}")))?;
    let (globals, mut queue) = registry_queue_init::<ToastApp>(&conn)
        .map_err(|e| Error::Wayland(format!("registre Wayland : {e}")))?;
    let qh = queue.handle();
    let compositor: wl_compositor::WlCompositor = globals
        .bind(&qh, 4..=6, ())
        .map_err(|e| Error::Wayland(format!("wl_compositor : {e}")))?;
    let shm: wl_shm::WlShm = globals
        .bind(&qh, 1..=1, ())
        .map_err(|e| Error::Wayland(format!("wl_shm : {e}")))?;
    let layer_shell: zwlr_layer_shell_v1::ZwlrLayerShellV1 = globals
        .bind(&qh, 4..=4, ())
        .map_err(|e| Error::Wayland(format!("wlr-layer-shell indisponible : {e}")))?;
    let output = options
        .output
        .as_deref()
        .and_then(|wanted| crate::wayland::output_by_name(&conn, &globals, wanted));
    let renderer = ToastRenderer::new(
        &font_path,
        options.icon.as_deref(),
        options.game_icon.as_deref(),
        options.store,
    )?;
    let mut app = ToastApp {
        shm,
        qh: qh.clone(),
        surface: None,
        layer: None,
        width: 0,
        height: AREA_H,
        configured: false,
        slots: Vec::new(),
        renderer,
        toast: options,
        started: Instant::now(),
        exit: false,
    };

    let surface = compositor.create_surface(&qh, ());
    let layer = layer_shell.get_layer_surface(
        &surface,
        output.as_ref(),
        zwlr_layer_shell_v1::Layer::Overlay,
        "lapinux-achievement".to_string(),
        &qh,
        (),
    );
    // Bande collée en haut, étirée à la largeur de l'écran : la carte s'y
    // centre, et rien ne couvre le jeu en dehors d'elle.
    layer.set_anchor(
        zwlr_layer_surface_v1::Anchor::Top
            | zwlr_layer_surface_v1::Anchor::Left
            | zwlr_layer_surface_v1::Anchor::Right,
    );
    layer.set_exclusive_zone(0);
    // Ni clavier ni souris : le bandeau informe, il ne prend jamais la main.
    layer.set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::None);
    let region = compositor.create_region(&qh, ());
    surface.set_input_region(Some(&region));
    region.destroy();
    // Largeur nulle avec les deux ancrages horizontaux : le compositeur
    // l'étire. La hauteur, elle, est celle de la carte et de sa glissade.
    layer.set_size(0, AREA_H);
    app.surface = Some(surface.clone());
    app.layer = Some(layer);
    surface.commit();

    queue
        .roundtrip(&mut app)
        .map_err(|e| Error::Wayland(format!("configure : {e}")))?;
    if !app.configured {
        return Err(Error::Wayland("aucun configure reçu du compositeur".into()));
    }
    app.started = Instant::now();

    let deadline = Instant::now() + Duration::from_secs_f32(app.toast.total() + 0.2);
    while !app.exit && Instant::now() < deadline {
        let frame = Instant::now();
        app.draw(&conn)?;
        queue
            .roundtrip(&mut app)
            .map_err(|e| Error::Wayland(format!("bandeau : {e}")))?;
        let elapsed = frame.elapsed();
        if elapsed < Duration::from_millis(16) {
            std::thread::sleep(Duration::from_millis(16) - elapsed);
        }
    }
    if let Some(surface) = &app.surface {
        surface.destroy();
    }
    conn.flush().ok();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn renderer() -> Option<ToastRenderer> {
        let font = font::find_font().ok()?;
        ToastRenderer::new(&font, None, None, Store::Steam).ok()
    }

    fn toast() -> ToastOptions {
        ToastOptions {
            title: "Finir le jeu sans mourir".into(),
            game: "Jeu de test".into(),
            store: Store::Steam,
            percent: Some(12.4),
            icon: None,
            game_icon: None,
            hold: Duration::from_secs(4),
            font: None,
            output: None,
        }
    }

    /// Boîte des pixels visibles, pour vérifier la place de la carte.
    fn bounds(pixmap: &Pixmap) -> Option<(u32, u32, u32, u32)> {
        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
        for (index, pixel) in pixmap.data().chunks(4).enumerate() {
            if pixel[3] < 90 {
                continue;
            }
            let x = (index as u32) % pixmap.width();
            let y = (index as u32) / pixmap.width();
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
        (x0 != u32::MAX).then_some((x0, y0, x1, y1))
    }

    #[test]
    fn the_toast_is_painted_horizontally_centred() {
        let Some(renderer) = renderer() else {
            return;
        };
        let pixmap = renderer.render(1280, AREA_H, &toast(), 1.5);
        let (x0, y0, x1, _) = bounds(&pixmap).expect("des pixels peints");
        // La carte occupe le centre : ses deux marges se ressemblent.
        let left = x0 as i32;
        let right = 1280 - x1 as i32;
        assert!((left - right).abs() < 6, "gauche {left}, droite {right}");
        // Et elle est posée en haut de l'écran.
        assert!(y0 < 40, "carte trop basse : {y0}");
    }

    #[test]
    fn the_toast_disappears_after_its_hold() {
        let Some(renderer) = renderer() else {
            return;
        };
        let toast = toast();
        let shown = renderer.render(1280, AREA_H, &toast, 2.0);
        assert!(bounds(&shown).is_some());
        let gone = renderer.render(1280, AREA_H, &toast, toast.total() + 0.1);
        assert!(bounds(&gone).is_none(), "le bandeau reste après sa sortie");
    }

    #[test]
    fn the_toast_starts_inside_the_toast_area() {
        let Some(renderer) = renderer() else {
            return;
        };
        let pixmap = renderer.render(1280, AREA_H, &toast(), 0.0);
        // Première image : rien n'est encore peint, ou seulement la carte en
        // cours d'entrée, jamais un écran plein.
        assert!(bounds(&pixmap).is_none());
    }

    #[test]
    fn a_uniform_icon_is_treated_as_missing() {
        let mut flat = Pixmap::new(8, 8).unwrap();
        flat.fill(tiny_skia::Color::from_rgba8(10, 10, 12, 255));
        assert!(is_blank(&flat));
        let mut varied = Pixmap::new(8, 8).unwrap();
        for (index, pixel) in varied.data_mut().chunks_exact_mut(4).enumerate() {
            let value = if index % 2 == 0 { 20 } else { 220 };
            pixel.copy_from_slice(&[value, value, value, 255]);
        }
        assert!(!is_blank(&varied));
    }

    #[test]
    fn a_share_uses_the_french_comma() {
        assert_eq!(share(42.3), "42,3");
        assert_eq!(share(100.0), "100,0");
        assert_eq!(share(0.0), "0,0");
    }

    #[test]
    fn a_long_title_is_shortened() {
        let Some(font_path) = font::find_font().ok() else {
            return;
        };
        let bytes = std::fs::read(font_path).unwrap();
        let font = FontVec::try_from_vec(bytes).unwrap();
        let fitted = fit_text(&font, "Un titre de succès vraiment très long", 120.0, 15.5);
        assert!(fitted.ends_with('…'));
        assert!(text_width(&font, &fitted, 15.5) <= 120.0);
    }
}
