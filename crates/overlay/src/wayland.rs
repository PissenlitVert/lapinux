//! Client Wayland minimal : surface wlr-layer-shell de type overlay.

use crate::menu::{Action, Live, MenuState, Page};
use crate::render::{hover_at, panel_bounds, Renderer};
use crate::shm::Mapping;
use crate::xkb::{fallback_text, Xkb, KEY_BACKSPACE, KEY_ESCAPE, KEY_RETURN};
use crate::{Error, Result};
use lapinux_core::client::Client;
use lapinux_core::ipc::Req;
use lapinux_core::model::SessionInfo;
use lapinux_core::paths::LapinuxPaths;
use lapinux_core::social::SteamFriendsState;
use lapinux_input::{Nav, NavReader};
use std::io::Read;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_keyboard, wl_output, wl_pointer, wl_region, wl_registry, wl_seat,
    wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};
use wayland_cursor::{Cursor, CursorTheme};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

/// Le daemon demande la fermeture par SIGTERM : l'overlay termine son
/// animation de sortie au lieu d'être coupé au milieu d'une image.
static TERMINATE: AtomicBool = AtomicBool::new(false);

extern "C" fn on_terminate(_signal: libc::c_int) {
    TERMINATE.store(true, Ordering::Relaxed);
}

pub struct OverlayOptions {
    pub width: u32,
    pub height: u32,
    /// Si défini, l'overlay se ferme après cette durée (mode démo).
    pub duration: Option<Duration>,
    pub font: Option<PathBuf>,
    /// Jaquette du jeu, dessinée en tête du panneau.
    pub capsule: Option<PathBuf>,
    /// Icône carrée du jeu, dessinée dans l'en-tête du HUD.
    pub icon: Option<PathBuf>,
    /// Tube où le daemon écrit les événements de la manette (octets `Nav`).
    /// Absent en mode démonstration : l'overlay lit alors les manettes seul.
    pub nav_fifo: Option<PathBuf>,
    /// Plaque seule : ni menu, ni clavier, ni souris. Le processus vit toute
    /// la partie, alors que le menu rapide ne dure que son ouverture.
    pub hud_only: bool,
    /// Écran Wayland cible (`DP-2`). Sans lui, le compositeur choisit la
    /// sortie par défaut, qui n'est pas forcément celle du jeu.
    pub output: Option<String>,
    /// Échecs renvoyés par le fil qui interroge le daemon. L'overlay les
    /// applique entre deux images, sans bloquer le menu.
    pub results: Option<std::sync::mpsc::Receiver<ActionFailed>>,
}

/// Action que le daemon a refusée, renvoyée au panneau après coup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionFailed {
    /// Forme filaire de l'action, par exemple `invite:7656119…`.
    pub action: String,
    /// Message d'erreur du daemon, affiché tel quel sur la page d'amis.
    pub error: String,
}

impl Default for OverlayOptions {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            duration: None,
            font: None,
            capsule: None,
            icon: None,
            nav_fifo: None,
            hud_only: false,
            output: None,
            results: None,
        }
    }
}

/// Reçoit le nom des sorties Wayland avant de créer la surface : le protocole
/// layer-shell exige l'objet `wl_output` de l'écran, pas son nom.
struct OutputScan {
    outputs: Vec<(wl_output::WlOutput, Option<String>)>,
}

impl Dispatch<wl_output::WlOutput, usize> for OutputScan {
    fn event(
        state: &mut Self,
        _: &wl_output::WlOutput,
        event: <wl_output::WlOutput as Proxy>::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Name { name } = event {
            if let Some((_, slot)) = state.outputs.get_mut(*index) {
                *slot = Some(name);
            }
        }
    }
}

/// Interroge le daemon une fois par seconde : cadence mesurée et âge de la
/// session. Le panneau reste à jour tant que l'overlay est ouvert.
fn watch_session(
    live: Arc<Mutex<Live>>,
    system: Arc<Mutex<Option<lapinux_core::system::SystemInfo>>>,
    session: Arc<Mutex<Option<SessionInfo>>>,
    friends: Arc<Mutex<SteamFriendsState>>,
    stop: Arc<AtomicBool>,
) {
    let paths = LapinuxPaths::discover();
    let client = Client::new(&paths).without_spawn();
    while !stop.load(Ordering::Relaxed) {
        // La capture se lit à part : elle peut tourner ou s'arrêter sans que
        // la session change.
        let recording = client
            .request_with_timeout(&Req::RecordingStatus, Duration::from_secs(2))
            .ok()
            .and_then(|envelope| {
                envelope
                    .into_data::<Option<lapinux_core::model::RecordingState>>()
                    .ok()
            })
            .flatten();
        // La charge de la machine voyage à part : le panneau n'en a pas
        // besoin, la plaque de performance si.
        let measured = client
            .request_with_timeout(&Req::System, Duration::from_secs(2))
            .ok()
            .and_then(|envelope| {
                envelope
                    .into_data::<lapinux_core::system::SystemInfo>()
                    .ok()
            });
        if let Ok(mut slot) = system.lock() {
            *slot = measured;
        }
        let info = client
            .request_with_timeout(&Req::Session, Duration::from_secs(2))
            .and_then(|envelope| envelope.into_data::<Option<SessionInfo>>())
            .ok()
            .flatten();
        // Le volume réel du flux du jeu : le curseur part de la valeur
        // effective, pas d'un 100 % arbitraire.
        let volume = client
            .request_with_timeout(&Req::GameVolume { volume: None }, Duration::from_secs(2))
            .ok()
            .and_then(|envelope| envelope.data.get("volume").and_then(|value| value.as_f64()))
            .map(|value| (value as f32).clamp(0.0, 1.0));
        if let Ok(mut slot) = session.lock() {
            *slot = info.clone();
        }
        // La liste d'amis, les avatars et les messages viennent du daemon :
        // l'overlay n'a aucune source réseau à interroger lui-même.
        let friends_state = client
            .request_with_timeout(&Req::SteamFriendsState, Duration::from_secs(2))
            .ok()
            .and_then(|envelope| envelope.into_data::<SteamFriendsState>().ok());
        // Une lecture ratée ne ferme pas la conversation : l'état précédent
        // reste valable jusqu'au prochain verdict du daemon.
        if let Some(friends_state) = friends_state {
            if let Ok(mut slot) = friends.lock() {
                *slot = friends_state;
            }
        }
        let snapshot = match info {
            Some(info) => Live {
                fps: info
                    .fps
                    .filter(|fps| !fps.is_empty())
                    .map(|fps| fps.current),
                session_secs: Some((lapinux_core::util::now_unix() - info.started).max(0) as u64),
                dlss: info.dlss,
                series: info.fps_series,
                gamescope: info.gamescope,
                recording,
                volume,
            },
            None => Live {
                recording,
                volume,
                ..Live::default()
            },
        };
        if let Ok(mut guard) = live.lock() {
            *guard = snapshot;
        }
        // Sommeil découpé : la sortie de l'overlay ne doit pas attendre la
        // seconde suivante avant d'arrêter le fil.
        for _ in 0..9 {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

struct Slot {
    buffer: wl_buffer::WlBuffer,
    mapping: Mapping,
    released: Arc<AtomicBool>,
    width: u32,
    height: u32,
}

struct App {
    menu: MenuState,
    renderer: Option<Renderer>,
    font_path: PathBuf,
    capsule: Option<PathBuf>,
    icon: Option<PathBuf>,
    shm: wl_shm::WlShm,
    qh: QueueHandle<App>,
    surface: Option<wl_surface::WlSurface>,
    layer: Option<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    pointer: Option<wl_pointer::WlPointer>,
    /// Disposition du clavier, reçue du compositeur : sans elle, les touches
    /// Wayland ne donnent que des codes et aucun texte (AZERTY compris).
    xkb: Option<Xkb>,
    pointer_pos: (f32, f32),
    /// Le survol disparaît dès qu'une autre méthode de navigation prend le relais.
    pointer_hover: bool,
    /// Bouton gauche maintenu : le curseur de cadence se règle au glissement.
    dragging_fps: bool,
    /// Bouton gauche maintenu sur le rail de génération d'images.
    dragging_afmf: bool,
    dragging_scale: bool,
    dragging_record_scale: bool,
    dragging_volume: bool,
    /// Dernière image envoyée pendant un glissement : le pointeur peut
    /// produire des centaines d'événements par seconde, on n'en rend qu'une
    /// par rafraîchissement.
    last_pointer_draw: Instant,
    /// Dernier serial d'entrée du pointeur, requis par `set_cursor`.
    pointer_serial: u32,
    /// Curseur du thème système, déclaré explicitement au compositeur : sans
    /// cela, le pointeur peut rester celui, masqué, du jeu en plein écran.
    _cursor_theme: Option<CursorTheme>,
    cursor: Option<Cursor>,
    cursor_surface: Option<wl_surface::WlSurface>,
    /// Fermeture demandée : jouer l'animation de sortie avant de quitter.
    closing: bool,
    /// Action différée jusqu'à la fin de l'animation (quitter le jeu).
    pending_action: Option<Action>,
    width: u32,
    height: u32,
    configured: bool,
    slots: Vec<Slot>,
    on_action: Box<dyn FnMut(Action)>,
    /// Réponses d'échec du daemon, lues par la boucle Wayland.
    results: Option<std::sync::mpsc::Receiver<ActionFailed>>,
    deadline: Option<Instant>,
    exit: bool,
}

impl App {
    fn ensure_renderer(&mut self) -> Result<()> {
        let needs_new = match &self.renderer {
            Some(renderer) => renderer.width != self.width || renderer.height != self.height,
            None => true,
        };
        if needs_new {
            self.renderer = Some(Renderer::new(
                &self.font_path,
                self.width,
                self.height,
                self.capsule.as_deref(),
                self.icon.as_deref(),
            )?);
        }
        Ok(())
    }

    fn set_selected(&mut self, index: usize) {
        if self.menu.page == Page::Menu && index < self.menu.items().len() {
            self.menu.selected = index;
            self.menu.pulse = 1.0;
        }
    }

    fn dispatch_action(&mut self, action: Option<Action>) {
        if let Some(action) = action {
            // Une invitation est marquée avant de partir : le clic suivant
            // sur le bouton ne produit pas un second message Steam.
            if let Action::InviteFriend(steam_id) = &action {
                if !self.menu.mark_invite_sent(*steam_id) {
                    return;
                }
                (self.on_action)(Action::InviteFriend(*steam_id));
                return;
            }
            if matches!(action, Action::Resume | Action::QuitGame) {
                self.closing = true;
                // « Quitter » ne doit pas couper l'animation : le daemon
                // reçoit l'ordre une fois le panneau sorti de l'écran.
                self.pending_action = Some(action);
            } else {
                (self.on_action)(action);
            }
        }
    }

    /// Redessine au plus une fois par rafraîchissement pendant un glissement.
    /// Les événements Wayland arrivent souvent plus vite que l'écran ; sans ce
    /// plafond, chaque pixel de mouvement relançait un rendu CPU plein écran.
    fn draw_if_pointer_due(&mut self, conn: &Connection) {
        if self.last_pointer_draw.elapsed() < Duration::from_millis(16) {
            return;
        }
        self.last_pointer_draw = Instant::now();
        let _ = self.draw(conn);
    }

    /// Déclare le curseur du thème sur la surface du pointeur.
    fn apply_cursor(&self) {
        let (Some(pointer), Some(surface), Some(cursor)) =
            (&self.pointer, &self.cursor_surface, &self.cursor)
        else {
            return;
        };
        if cursor.image_count() == 0 {
            return;
        }
        let image = &cursor[0];
        let (hot_x, hot_y) = image.hotspot();
        let (width, height) = image.dimensions();
        surface.attach(Some(&**image), 0, 0);
        surface.damage(0, 0, width as i32, height as i32);
        surface.commit();
        pointer.set_cursor(
            self.pointer_serial,
            Some(surface),
            hot_x as i32,
            hot_y as i32,
        );
    }

    /// Navigation à la manette : l'overlay lit les périphériques directement,
    /// le service d'entrée du daemon relâchant son grab tant qu'il est ouvert.
    fn on_nav(&mut self, nav: Nav) {
        self.pointer_hover = false;
        self.menu.hover = None;
        match nav {
            Nav::Up => self.menu.prev(),
            Nav::Down => self.menu.next(),
            Nav::Left => {
                let action = self.menu.adjust(-1);
                self.dispatch_action(action);
            }
            Nav::Right => {
                let action = self.menu.adjust(1);
                self.dispatch_action(action);
            }
            Nav::Confirm => {
                let action = self.menu.activate();
                self.dispatch_action(action);
            }
            Nav::Back | Nav::Menu => {
                self.menu.close();
                if self.menu.exit {
                    self.closing = true;
                    self.pending_action = None;
                }
            }
        }
        self.menu.pulse = 1.0;
    }

    fn on_keycode(&mut self, keycode: u32) {
        self.pointer_hover = false;
        self.menu.hover = None;
        // Dans une conversation, le clavier écrit. Entrée envoie, Échap
        // revient à la liste et Retour arrière corrige le dernier caractère.
        if self.menu.page == Page::Invite && self.menu.chat_open() {
            let keysym = self.xkb.as_ref().map(|xkb| xkb.keysym(keycode));
            let enter = keycode == 28 || keycode == 96 || keysym == Some(KEY_RETURN);
            let escape = keycode == 1 || keysym == Some(KEY_ESCAPE);
            let backspace = keycode == 14 || keysym == Some(KEY_BACKSPACE);
            if enter {
                let action = self.menu.send_chat();
                self.dispatch_action(action);
            } else if escape {
                self.menu.close_chat();
            } else if backspace {
                self.menu.pop_chat_char();
            } else {
                let text = self
                    .xkb
                    .as_ref()
                    .and_then(|xkb| xkb.text(keycode))
                    .or_else(|| fallback_text(keycode).map(|ch| ch.to_string()));
                if let Some(text) = text {
                    for ch in text.chars().filter(|ch| !ch.is_control()) {
                        self.menu.push_chat_char(ch);
                    }
                }
            }
            self.menu.pulse = 1.0;
            return;
        }
        match keycode {
            103 | 17 => self.menu.prev(),
            108 | 31 => self.menu.next(),
            105 => {
                let action = self.menu.adjust(-1);
                self.dispatch_action(action);
            }
            106 => {
                let action = self.menu.adjust(1);
                self.dispatch_action(action);
            }
            28 | 96 | 57 => {
                let action = self.menu.activate();
                self.dispatch_action(action);
            }
            1 => {
                self.menu.close();
                if self.menu.exit {
                    self.closing = true;
                    self.pending_action = None;
                }
            }
            _ => return,
        }
        self.menu.pulse = 1.0;
    }

    fn click_at(&mut self, x: f32, y: f32) {
        let (panel_x, panel_y, panel_w, panel_h) =
            panel_bounds(self.width, self.height, &self.menu);
        if x < panel_x || x > panel_x + panel_w {
            return;
        }
        // Le bouton Retour ferme le panneau Performances et ramène au menu.
        if self.menu.back_at(panel_x, panel_y, panel_w, x, y) {
            self.menu.close();
            return;
        }
        let local_y = y - panel_y;
        match self.menu.page {
            Page::Menu => {
                if let Some(row) = self.menu.hit_row(local_y) {
                    self.set_selected(row);
                    if row == self.menu.volume_row() {
                        if let Some(value) = self.menu.volume_at(x, panel_x, panel_w) {
                            self.dragging_volume = true;
                            self.menu.begin_volume_drag();
                            let action = self.menu.set_volume(value);
                            self.dispatch_action(action);
                            return;
                        }
                    }
                    let action = self.menu.activate();
                    self.dispatch_action(action);
                }
            }
            Page::Performance => {
                if let Some(row) = self.menu.hit_perf_row(local_y) {
                    self.menu.perf_selected = row;
                    self.menu.pulse = 1.0;
                    if row == 1 {
                        // Un clic n'importe où sur la ligne déplace le
                        // curseur, même un peu hors de la piste : viser une
                        // piste fine au pixel près est illusoire.
                        let index = self.menu.fps_index_near(x, panel_x, panel_w);
                        self.dragging_fps = true;
                        let action = self.menu.set_fps_index(index);
                        self.dispatch_action(action);
                        return;
                    }
                    if row == 2 {
                        // Le rail se règle comme la piste de cadence : le clic
                        // choisit le cran le plus proche, les flèches des
                        // extrémités coupent ou montent à ×4.
                        let index = self.menu.afmf_index_near(x, panel_x, panel_w);
                        self.dragging_afmf = true;
                        let action = self.menu.set_afmf_index(index);
                        self.dispatch_action(action);
                        return;
                    }
                    if row == 10 {
                        if let Some(value) = self.menu.scale_at(x, panel_x, panel_w) {
                            self.dragging_scale = true;
                            let action = self.menu.set_scale(value);
                            self.dispatch_action(action);
                            return;
                        }
                    }
                    let action = self.menu.activate();
                    self.dispatch_action(action);
                }
            }
            Page::Profiles => {
                if let Some(row) = self.menu.profile_hit_row(local_y) {
                    self.menu.profile_selected = row;
                    self.menu.pulse = 1.0;
                    let action = self.menu.activate();
                    self.dispatch_action(action);
                }
            }
            Page::Invite => {
                // La conversation ouverte garde le clavier et la molette ;
                // seul le bouton Retour, traité plus haut, en sort.
                if self.menu.chat_open() {
                    if let Some(steam_id) = self
                        .menu
                        .chat_invite_button_at(x, y, panel_x, panel_y, panel_w)
                    {
                        self.menu.pulse = 1.0;
                        self.dispatch_action(Some(Action::InviteFriend(steam_id)));
                    }
                    return;
                }
                // Le bouton « Inviter » de la fiche passe avant l'ouverture
                // de la conversation : un clic dessus n'ouvre pas le chat.
                if let Some(steam_id) = self
                    .menu
                    .invite_button_at(x, y, panel_x, panel_y, panel_w, panel_h)
                {
                    self.menu.pulse = 1.0;
                    self.dispatch_action(Some(Action::InviteFriend(steam_id)));
                    return;
                }
                if let Some(index) = self
                    .menu
                    .invite_scroll_index_at(panel_x, panel_y, panel_w, panel_h, x, y)
                {
                    self.menu.invite_selected = index;
                    self.menu.pulse = 1.0;
                    return;
                }
                // Une carte ouvre directement la conversation de l'ami.
                if x <= self.menu.invite_list_right(panel_x, panel_w) {
                    if let Some(row) = self.menu.invite_hit_row(local_y, panel_h) {
                        self.menu.open_chat_at(row);
                    }
                }
            }
            Page::Recordings => {
                if let Some(position) = self
                    .menu
                    .record_position_cell_at(panel_x, panel_y, panel_w, x, y)
                {
                    self.menu.record_position = position;
                    self.menu.recording_selected = 3;
                    let action = self.menu.record_badge_action();
                    self.dispatch_action(Some(action));
                } else if let Some(row) = self.menu.hit_recording_row(local_y) {
                    self.menu.recording_selected = row;
                    self.menu.pulse = 1.0;
                    if row == 4 {
                        if let Some(value) = self.menu.record_scale_at(x, panel_x, panel_w) {
                            self.dragging_record_scale = true;
                            let action = self.menu.set_record_scale(value);
                            self.dispatch_action(action);
                            return;
                        }
                    }
                    let action = self.menu.activate();
                    self.dispatch_action(action);
                }
            }
        }
    }

    fn draw(&mut self, conn: &Connection) -> Result<()> {
        if !self.configured {
            return Ok(());
        }
        self.ensure_renderer()?;
        self.menu.hover = if self.pointer_hover {
            hover_at(
                self.width,
                self.height,
                &self.menu,
                self.pointer_pos.0,
                self.pointer_pos.1,
            )
        } else {
            None
        };
        let pixmap = match &self.renderer {
            Some(renderer) => renderer.render(&self.menu),
            None => return Ok(()),
        };
        let width = self.width;
        let height = self.height;
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
                if self.slots.len() < 3 {
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
        impl Dispatch<$iface, ()> for App {
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
ignore_events!(wl_surface::WlSurface);
ignore_events!(wl_region::WlRegion);
ignore_events!(wl_shm::WlShm);
ignore_events!(wl_shm_pool::WlShmPool);
ignore_events!(zwlr_layer_shell_v1::ZwlrLayerShellV1);

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for App {
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

impl Dispatch<wl_buffer::WlBuffer, Arc<AtomicBool>> for App {
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

impl Dispatch<wl_seat::WlSeat, ()> for App {
    fn event(
        app: &mut Self,
        seat: &wl_seat::WlSeat,
        event: <wl_seat::WlSeat as Proxy>::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities { capabilities } = event {
            if let WEnum::Value(caps) = capabilities {
                if caps.contains(wl_seat::Capability::Keyboard) && app.keyboard.is_none() {
                    app.keyboard = Some(seat.get_keyboard(qh, ()));
                }
                if caps.contains(wl_seat::Capability::Pointer) && app.pointer.is_none() {
                    app.pointer = Some(seat.get_pointer(qh, ()));
                }
            }
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for App {
    fn event(
        app: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: <wl_keyboard::WlKeyboard as Proxy>::Event,
        _: &(),
        conn: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_keyboard::Event::Keymap { format, fd, size } => {
                if size == 0 || size > 4 * 1024 * 1024 {
                    return;
                }
                if !matches!(format, WEnum::Value(wl_keyboard::KeymapFormat::XkbV1)) {
                    return;
                }
                let mut file = std::fs::File::from(fd);
                let mut keymap = Vec::with_capacity(size as usize);
                if file.read_to_end(&mut keymap).is_ok() {
                    app.xkb = Xkb::from_keymap(&keymap);
                    if app.xkb.is_none() {
                        tracing::warn!("keymap Wayland illisible : saisie ASCII seulement");
                    }
                }
            }
            wl_keyboard::Event::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
                ..
            } => {
                if let Some(xkb) = app.xkb.as_mut() {
                    xkb.update_mask(mods_depressed, mods_latched, mods_locked, group);
                }
            }
            wl_keyboard::Event::Key {
                key,
                state: key_state,
                ..
            } => {
                if let WEnum::Value(wl_keyboard::KeyState::Pressed) = key_state {
                    app.on_keycode(key + 8);
                    let _ = app.draw(conn);
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for App {
    fn event(
        app: &mut Self,
        _: &wl_pointer::WlPointer,
        event: <wl_pointer::WlPointer as Proxy>::Event,
        _: &(),
        conn: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_pointer::Event::Enter {
                serial,
                surface_x,
                surface_y,
                ..
            } => {
                app.pointer_serial = serial;
                app.pointer_pos = (surface_x as f32, surface_y as f32);
                app.pointer_hover = true;
                let _ = app.draw(conn);
                // Sans cette déclaration, le compositeur peut garder le
                // curseur masqué que le jeu avait installé en plein écran.
                app.apply_cursor();
                let _ = conn.flush();
            }
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => {
                app.pointer_pos = (surface_x as f32, surface_y as f32);
                app.pointer_hover = true;
                let hovered = hover_at(
                    app.width,
                    app.height,
                    &app.menu,
                    app.pointer_pos.0,
                    app.pointer_pos.1,
                );
                if hovered != app.menu.hover {
                    let _ = app.draw(conn);
                }
                let (panel_x, _, panel_w, _) = panel_bounds(app.width, app.height, &app.menu);
                if app.dragging_fps && app.menu.page == Page::Performance {
                    let index = app.menu.fps_index_near(surface_x as f32, panel_x, panel_w);
                    if index != app.menu.fps_index() {
                        let action = app.menu.set_fps_index(index);
                        app.dispatch_action(action);
                        app.draw_if_pointer_due(conn);
                    }
                }
                if app.dragging_afmf && app.menu.page == Page::Performance {
                    let index = app.menu.afmf_index_near(surface_x as f32, panel_x, panel_w);
                    if index != app.menu.afmf_index() {
                        let action = app.menu.set_afmf_index(index);
                        app.dispatch_action(action);
                        app.draw_if_pointer_due(conn);
                    }
                }
                if app.dragging_scale && app.menu.page == Page::Performance {
                    if let Some(value) = app.menu.scale_at(surface_x as f32, panel_x, panel_w) {
                        if (value - app.menu.perf_scale).abs() > 0.001 {
                            let action = app.menu.set_scale(value);
                            app.dispatch_action(action);
                            app.draw_if_pointer_due(conn);
                        }
                    }
                }
                if app.dragging_record_scale && app.menu.page == Page::Recordings {
                    if let Some(value) =
                        app.menu.record_scale_at(surface_x as f32, panel_x, panel_w)
                    {
                        if (value - app.menu.record_scale).abs() > 0.001 {
                            let action = app.menu.set_record_scale(value);
                            app.dispatch_action(action);
                            app.draw_if_pointer_due(conn);
                        }
                    }
                }
                if app.dragging_volume && app.menu.page == Page::Menu {
                    if let Some(value) = app.menu.volume_at(surface_x as f32, panel_x, panel_w) {
                        if (value - app.menu.volume).abs() > 0.001 {
                            let action = app.menu.set_volume(value);
                            app.dispatch_action(action);
                            app.draw_if_pointer_due(conn);
                        }
                    }
                }
            }
            wl_pointer::Event::Leave { .. } => {
                app.pointer_hover = false;
                if app.menu.hover.take().is_some() {
                    let _ = app.draw(conn);
                }
            }
            wl_pointer::Event::Axis { axis, value, .. } => {
                if app.menu.page == Page::Invite
                    && matches!(axis, WEnum::Value(wl_pointer::Axis::VerticalScroll))
                    && value.abs() > 0.01
                {
                    let steps = (value.abs() / 10.0).round().clamp(1.0, 3.0) as i32;
                    let direction = if value > 0.0 { steps } else { -steps };
                    if app.menu.scroll_invite(direction) {
                        let _ = app.draw(conn);
                    }
                }
            }
            wl_pointer::Event::Button {
                button,
                state: button_state,
                ..
            } => {
                if button == 0x110 {
                    match button_state {
                        WEnum::Value(wl_pointer::ButtonState::Pressed) => {
                            let (x, y) = app.pointer_pos;
                            app.click_at(x, y);
                            let _ = app.draw(conn);
                        }
                        WEnum::Value(wl_pointer::ButtonState::Released) => {
                            let was_volume = app.dragging_volume;
                            let was_dragging = was_volume
                                || app.dragging_fps
                                || app.dragging_afmf
                                || app.dragging_scale
                                || app.dragging_record_scale;
                            // Le dernier pas d'un glissement peut avoir été
                            // perdu par la file d'actions : la valeur finale
                            // repart une fois. Le daemon la reçoit en double au
                            // pire, ce qui ne change rien à son état.
                            if app.dragging_fps {
                                app.dispatch_action(Some(Action::SetFpsCap(app.menu.fps_cap)));
                            }
                            if app.dragging_afmf {
                                if let Some(value) = app.menu.afmf_multiplier {
                                    app.dispatch_action(Some(Action::AfmfMultiplier(value)));
                                }
                            }
                            app.dragging_fps = false;
                            app.dragging_afmf = false;
                            app.dragging_scale = false;
                            app.dragging_record_scale = false;
                            app.dragging_volume = false;
                            if was_volume {
                                app.menu.end_volume_drag();
                            }
                            // La dernière image a pu être sautée par le
                            // plafond : on rend l'état final au relâchement.
                            if was_dragging {
                                let _ = app.draw(conn);
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for App {
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

/// Joue l'ouverture ou la fermeture image par image. Chaque image passe par
/// un aller-retour Wayland : sans cela, les tampons ne sont jamais rendus et
/// seules les trois premières images de l'animation sont présentées.
fn animate(
    app: &mut App,
    conn: &Connection,
    queue: &mut EventQueue<App>,
    opening: bool,
) -> Result<()> {
    let started = Instant::now();
    let mut last = started;
    while !app.exit && started.elapsed() < Duration::from_millis(700) {
        if TERMINATE.load(Ordering::Relaxed) {
            app.closing = true;
        }
        if opening {
            if app.closing || app.menu.anim >= 1.0 {
                break;
            }
        } else if app.menu.anim <= 0.0 {
            break;
        }
        let now = Instant::now();
        // Le pas suit l'horloge réelle : un aller-retour lent ne ralentit pas
        // l'animation, il la rend seulement moins dense.
        let dt = (now - last).as_secs_f32().clamp(0.001, 0.05);
        last = now;
        if opening {
            app.menu.tick(dt);
        } else {
            app.menu.tick_out(dt);
        }
        app.draw(conn)?;
        queue
            .roundtrip(app)
            .map_err(|e| Error::Wayland(format!("animation : {e}")))?;
        // Une image par rafraîchissement suffit : le panneau est rendu en
        // CPU, inutile de saturer le compositeur avec des images en double.
        let elapsed = now.elapsed();
        if elapsed < Duration::from_millis(16) {
            std::thread::sleep(Duration::from_millis(16) - elapsed);
        }
    }
    if opening && !app.closing && !app.exit && app.menu.anim < 1.0 {
        app.menu.anim = 1.0;
        app.draw(conn)?;
        queue
            .roundtrip(app)
            .map_err(|e| Error::Wayland(format!("animation : {e}")))?;
    }
    Ok(())
}

/// Retrouve une sortie Wayland par son nom (`DP-2`) : le protocole
/// layer-shell exige l'objet `wl_output`, que seul un aller-retour du
/// registre rend.
pub(crate) fn output_by_name(
    conn: &Connection,
    globals: &wayland_client::globals::GlobalList,
    wanted: &str,
) -> Option<wl_output::WlOutput> {
    let mut scan_queue: EventQueue<OutputScan> = conn.new_event_queue();
    let scan_qh = scan_queue.handle();
    let mut scan = OutputScan {
        outputs: Vec::new(),
    };
    for global in globals.contents().clone_list() {
        if global.interface != "wl_output" {
            continue;
        }
        let index = scan.outputs.len();
        let output: wl_output::WlOutput =
            globals
                .registry()
                .bind(global.name, global.version.min(4), &scan_qh, index);
        scan.outputs.push((output, None));
    }
    if scan_queue.roundtrip(&mut scan).is_err() {
        tracing::warn!("écran de l'overlay : noms des sorties indisponibles");
        return None;
    }
    let found = scan
        .outputs
        .iter()
        .find(|(_, name)| name.as_deref() == Some(wanted))
        .map(|(output, _)| output.clone());
    if found.is_none() {
        tracing::warn!("écran de l'overlay : sortie {wanted} introuvable");
    }
    found
}

pub fn run(
    mut menu: MenuState,
    options: OverlayOptions,
    on_action: impl FnMut(Action) + 'static,
) -> Result<()> {
    // Le processus du menu ne peint que le panneau ; les surfaces HUD ont
    // leur propre processus et leur propre `--hud-only`.
    menu.hud_only = options.hud_only;
    // Les panneaux Plasma des autres écrans resteraient visibles au-dessus
    // du jeu : les masquer le temps de l'overlay donne un écran propre. Le
    // garde restaure leur mode exact en sortant, y compris par SIGTERM.
    let paths = LapinuxPaths::discover();
    let _panels = lapinux_input::panels::PanelGuard::hide(&paths.data_dir);
    let font_path = match options.font.clone() {
        Some(path) => path,
        None => crate::font::find_font()?,
    };
    let conn = Connection::connect_to_env()
        .map_err(|e| Error::Wayland(format!("connexion Wayland : {e}")))?;
    let (globals, mut queue) = registry_queue_init::<App>(&conn)
        .map_err(|e| Error::Wayland(format!("registre Wayland : {e}")))?;
    let qh = queue.handle();
    let compositor: wl_compositor::WlCompositor = globals
        .bind(&qh, 4..=6, ())
        .map_err(|e| Error::Wayland(format!("wl_compositor : {e}")))?;
    let shm: wl_shm::WlShm = globals
        .bind(&qh, 1..=1, ())
        .map_err(|e| Error::Wayland(format!("wl_shm : {e}")))?;
    // Thème de curseur du bureau : le pointeur est déclaré au compositeur
    // avec la surface de l'overlay, au lieu de dépendre du dernier curseur
    // posé par le jeu.
    let cursor_size = std::env::var("XCURSOR_SIZE")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(24)
        .clamp(16, 64);
    let mut cursor_theme = match CursorTheme::load(&conn, shm.clone(), cursor_size) {
        Ok(theme) => Some(theme),
        Err(err) => {
            tracing::warn!("curseur Wayland indisponible : {err:?}");
            None
        }
    };
    let mut cursor = None;
    if let Some(theme) = cursor_theme.as_mut() {
        cursor = theme.get_cursor("left_ptr").cloned();
        if cursor.is_none() {
            cursor = theme.get_cursor("default").cloned();
        }
    }
    let layer_shell: zwlr_layer_shell_v1::ZwlrLayerShellV1 = globals
        .bind(&qh, 4..=4, ())
        .map_err(|e| Error::Wayland(format!("wlr-layer-shell indisponible : {e}")))?;
    let seat: Option<wl_seat::WlSeat> = globals.bind(&qh, 1..=8, ()).ok();

    // Le HUD doit naître sur l'écran du jeu. Sans sortie explicite, KWin
    // choisit sa sortie par défaut : la plaque atterrissait sur DP-1 alors que
    // le jeu tournait sur DP-2. On lie toutes les sorties Wayland, on lit leur
    // nom (`wl_output.name`, version 4), puis on garde celle demandée.
    let target_output = options
        .output
        .as_deref()
        .and_then(|wanted| output_by_name(&conn, &globals, wanted));

    let width = options.width.max(320);
    let height = options.height.max(240);
    let mut app = App {
        menu,
        renderer: None,
        font_path,
        capsule: options.capsule.clone(),
        icon: options.icon.clone(),
        shm,
        qh: qh.clone(),
        surface: None,
        layer: None,
        keyboard: None,
        pointer: None,
        xkb: None,
        pointer_pos: (0.0, 0.0),
        pointer_hover: false,
        dragging_fps: false,
        dragging_afmf: false,
        dragging_scale: false,
        dragging_record_scale: false,
        dragging_volume: false,
        last_pointer_draw: Instant::now(),
        pointer_serial: 0,
        _cursor_theme: cursor_theme,
        cursor,
        cursor_surface: None,
        closing: false,
        pending_action: None,
        width,
        height,
        configured: false,
        slots: Vec::new(),
        on_action: Box::new(on_action),
        results: options.results,
        deadline: options.duration.map(|d| Instant::now() + d),
        exit: false,
    };
    app.ensure_renderer()?;

    let live = Arc::new(Mutex::new(Live::default()));
    let system: Arc<Mutex<Option<lapinux_core::system::SystemInfo>>> = Arc::new(Mutex::new(None));
    let session: Arc<Mutex<Option<SessionInfo>>> = Arc::new(Mutex::new(None));
    let friends: Arc<Mutex<SteamFriendsState>> = Arc::new(Mutex::new(SteamFriendsState::default()));
    let stop_watch = Arc::new(AtomicBool::new(false));
    {
        let live = Arc::clone(&live);
        let system = Arc::clone(&system);
        let session = Arc::clone(&session);
        let friends = Arc::clone(&friends);
        let stop = Arc::clone(&stop_watch);
        std::thread::spawn(move || watch_session(live, system, session, friends, stop));
    }
    // Le daemon pousse les événements de manette dans un tube : le service
    // d'entrée, qui tient déjà les manettes et connaît leurs boutons, reste
    // le seul lecteur des périphériques. Sans tube (démonstration), l'overlay
    // lit encore les manettes lui-même.
    let hud_only = options.hud_only;
    let nav_path = if hud_only {
        None
    } else {
        options.nav_fifo.clone()
    };
    let mut nav_fifo: Option<std::fs::File> = None;
    let mut nav_retry = Instant::now();
    if let Some(path) = nav_path.as_deref() {
        if let Some((file, pending)) = attach_nav_fifo(path) {
            nav_fifo = Some(file);
            for byte in pending {
                if let Some(event) = nav_from_byte(byte) {
                    app.on_nav(event);
                }
            }
        }
    }
    let nav = if hud_only || nav_path.is_some() {
        // Le tube remplace la lecture directe : les deux ensemble
        // dupliqueraient chaque appui une fois le service d'entrée relié.
        None
    } else {
        match NavReader::start() {
            Ok(reader) => Some(reader),
            Err(err) => {
                tracing::warn!("navigation manette de l'overlay : {err}");
                None
            }
        }
    };

    let surface = compositor.create_surface(&qh, ());
    let layer = layer_shell.get_layer_surface(
        &surface,
        target_output.as_ref(),
        zwlr_layer_shell_v1::Layer::Overlay,
        "lapinux-overlay".to_string(),
        &qh,
        (),
    );
    layer.set_anchor(
        zwlr_layer_surface_v1::Anchor::Top
            | zwlr_layer_surface_v1::Anchor::Bottom
            | zwlr_layer_surface_v1::Anchor::Left
            | zwlr_layer_surface_v1::Anchor::Right,
    );
    layer.set_exclusive_zone(-1);
    // Clavier exclusif : le menu prend le focus dès qu'il s'affiche. En mode
    // « à la demande », KWin n'envoyait les touches qu'après un clic sur le
    // panneau, d'où l'impression qu'il n'était pas actif.
    if hud_only {
        // Plaque seule : elle ne doit ni voler le clavier ni avaler les clics
        // du jeu — une région d'entrée vide rend la surface transparente aux
        // événements, tout en restant peinte.
        layer.set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::None);
        let region = compositor.create_region(&qh, ());
        surface.set_input_region(Some(&region));
        region.destroy();
    } else {
        layer.set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive);
    }
    // Quatre ancrages : la taille demandée est ignorée par KWin et la surface
    // n'est alors jamais affichée. (0, 0) laisse le compositeur donner l'écran.
    layer.set_size(0, 0);
    app.surface = Some(surface.clone());
    app.layer = Some(layer);
    app.cursor_surface = Some(compositor.create_surface(&qh, ()));
    app.keyboard = seat.as_ref().map(|s| s.get_keyboard(&qh, ()));
    app.pointer = seat.as_ref().map(|s| s.get_pointer(&qh, ()));
    surface.commit();

    queue
        .roundtrip(&mut app)
        .map_err(|e| Error::Wayland(format!("configure : {e}")))?;
    if !app.configured {
        return Err(Error::Wayland("aucun configure reçu du compositeur".into()));
    }

    // Le daemon ferme l'overlay en douceur : SIGTERM déclenche la sortie
    // animée au lieu d'interrompre la surface.
    unsafe {
        libc::signal(
            libc::SIGTERM,
            on_terminate as *const () as libc::sighandler_t,
        );
    }

    if hud_only {
        // Pas d'ouverture animée : le panneau ne doit jamais apparaître.
        app.menu.anim = 0.0;
        app.draw(&conn)?;
    } else {
        animate(&mut app, &conn, &mut queue, true)?;
    }

    loop {
        if app.exit {
            break;
        }
        if TERMINATE.load(Ordering::Relaxed) {
            app.closing = true;
        }
        if app.closing {
            animate(&mut app, &conn, &mut queue, false)?;
            break;
        }
        if let Some(deadline) = app.deadline {
            if Instant::now() >= deadline {
                break;
            }
            if !hud_only {
                app.menu.tick(0.016);
            }
            app.draw(&conn)?;
            std::thread::sleep(Duration::from_millis(16));
            continue;
        }
        queue
            .flush()
            .map_err(|e| Error::Wayland(format!("flush : {e}")))?;
        // Le tube n'a pas encore d'écrivain au démarrage : on le rouvre par
        // petits pas au lieu d'abandonner la manette pour toute la session.
        if nav_fifo.is_none() {
            if let Some(path) = nav_path.as_deref() {
                if nav_retry.elapsed() >= Duration::from_millis(120) {
                    nav_retry = Instant::now();
                    if let Some((file, pending)) = attach_nav_fifo(path) {
                        nav_fifo = Some(file);
                        let mut moved = false;
                        for byte in pending {
                            if let Some(event) = nav_from_byte(byte) {
                                app.on_nav(event);
                                moved = true;
                            }
                        }
                        if moved {
                            app.draw(&conn)?;
                        }
                    }
                }
            }
        }
        // Attente bornée : sans événement Wayland, la cadence et l'heure de
        // session doivent quand même rafraîchir le panneau.
        let mut fds = Vec::with_capacity(2);
        fds.push(libc::pollfd {
            fd: queue.as_fd().as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        });
        if let Some(file) = nav_fifo.as_ref() {
            fds.push(libc::pollfd {
                fd: file.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            });
        }
        // 16 ms : la manette doit répondre sans attendre le rafraîchissement
        // des statistiques, qui n'arrive qu'une fois par seconde.
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, 16) };
        if ready > 0 && fds[0].revents & libc::POLLIN != 0 {
            if let Some(guard) = queue.prepare_read() {
                guard
                    .read()
                    .map_err(|e| Error::Wayland(format!("lecture : {e}")))?;
            }
        }
        queue
            .dispatch_pending(&mut app)
            .map_err(|e| Error::Wayland(format!("dispatch : {e}")))?;
        if let (Some(file), Some(nav_fd)) = (nav_fifo.as_ref(), fds.get(1)) {
            if nav_fd.revents & libc::POLLIN != 0 {
                let mut moved = false;
                if let Some(bytes) = read_nav_bytes(file) {
                    for byte in bytes {
                        if let Some(nav) = nav_from_byte(byte) {
                            app.on_nav(nav);
                            moved = true;
                        }
                    }
                }
                if moved {
                    app.draw(&conn)?;
                }
            }
            // Sans écrivain, le tube reste en POLLHUP : le retirer évite une
            // boucle qui tournerait à vide jusqu'à la fermeture du panneau.
            // Il sera rouvert si le daemon revient.
            if nav_fd.revents & libc::POLLHUP != 0 && nav_fd.revents & libc::POLLIN == 0 {
                nav_fifo = None;
                nav_retry = Instant::now();
            }
        }
        if let Some(reader) = nav.as_ref() {
            let mut moved = false;
            while let Ok(event) = reader.events.try_recv() {
                app.on_nav(event);
                moved = true;
            }
            if moved {
                app.draw(&conn)?;
            }
        }
        let snapshot = live.lock().map(|guard| guard.clone()).ok();
        if let Some(snapshot) = snapshot {
            if app.menu.set_live(snapshot) {
                app.draw(&conn)?;
            }
        }
        // Les amis, avatars et nouveaux messages arrivent sans fermer
        // l'overlay : la conversation ouverte se met à jour sur place.
        let friends_state = friends.lock().map(|guard| guard.clone()).ok();
        if let Some(friends_state) = friends_state {
            if app.menu.set_invite(friends_state) {
                app.draw(&conn)?;
            }
        }
        // Le daemon peut refuser une action après coup : l'invitation est
        // alors réarmée et le message d'échec s'affiche sur la page d'amis.
        let mut failed = false;
        if let Some(results) = app.results.as_ref() {
            while let Ok(outcome) = results.try_recv() {
                failed |= app
                    .menu
                    .apply_action_failure(&outcome.action, &outcome.error);
            }
        }
        if failed {
            app.draw(&conn)?;
        }
        // Les réglages changés dans le panneau rapide arrivent par la session,
        // sans relancer la partie. La plaque seule reprend tout le HUD ; le
        // menu ne reprend que l'identité (profil de manettes, Proton, limite),
        // pour ne pas écraser un réglage en cours de modification.
        let info = session.lock().ok().and_then(|slot| slot.clone());
        if let Some(info) = info {
            let changed = if hud_only {
                app.menu.apply_session_hud(&info)
            } else {
                app.menu.apply_session_identity(&info)
            };
            if changed {
                app.draw(&conn)?;
            }
        }
        // La plaque de performance suit le même rythme : une lecture par
        // seconde suffit à ses chiffres.
        let measured = system.lock().ok().and_then(|slot| slot.clone());
        if app.menu.system != measured {
            app.menu.system = measured;
            if hud_only && app.menu.perf_hud {
                app.draw(&conn)?;
            }
        }
        // La plaque seule ne s'anime pas : `tick` ferait entrer le panneau.
        if !hud_only && app.menu.tick(0.016) {
            app.draw(&conn)?;
        }
    }
    stop_watch.store(true, Ordering::Relaxed);

    // « Quitter le jeu » attend la fin de l'animation : le jeu reste visible
    // pendant la sortie du panneau, puis le daemon reçoit l'ordre.
    if let Some(action) = app.pending_action.take() {
        (app.on_action)(action);
    }

    if let Some(surface) = &app.surface {
        surface.destroy();
    }
    conn.flush().ok();
    Ok(())
}

/// Ouvre le tube de navigation en lecture non bloquante : l'overlay ne doit
/// jamais attendre que le daemon écrive pour afficher son panneau.
fn open_nav_fifo(path: &Path) -> Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .map_err(|err| Error::Wayland(format!("tube manette : {err}")))
}

/// Ouvre le tube et vérifie qu'un écrivain est déjà là. Sans écrivain, la
/// lecture rend zéro octet et `poll` signalerait une fin de flux en
/// permanence : le tube est alors refermé et retenté plus tard. Les octets
/// déjà déposés par le daemon sont rendus pour être traités tout de suite.
fn attach_nav_fifo(path: &Path) -> Option<(std::fs::File, Vec<u8>)> {
    let mut file = open_nav_fifo(path).ok()?;
    let mut buffer = [0u8; 64];
    let pending = match file.read(&mut buffer) {
        Ok(0) => return None,
        Ok(count) => buffer[..count].to_vec(),
        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => Vec::new(),
        Err(err) => {
            tracing::warn!("tube manette : {err}");
            return None;
        }
    };
    Some((file, pending))
}

/// Vide le tube sans bloquer ; `None` quand il n'y a rien à lire.
fn read_nav_bytes(mut file: &std::fs::File) -> Option<Vec<u8>> {
    let mut buffer = [0u8; 64];
    match file.read(&mut buffer) {
        Ok(0) => None,
        Ok(count) => Some(buffer[..count].to_vec()),
        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => None,
        Err(err) => {
            tracing::warn!("lecture manette : {err}");
            None
        }
    }
}

/// Octet écrit par le daemon : l'ordre suit `Nav`.
fn nav_from_byte(byte: u8) -> Option<Nav> {
    match byte {
        0 => Some(Nav::Up),
        1 => Some(Nav::Down),
        2 => Some(Nav::Left),
        3 => Some(Nav::Right),
        4 => Some(Nav::Confirm),
        5 => Some(Nav::Back),
        6 => Some(Nav::Menu),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nav_bytes_match_the_enum() {
        assert_eq!(nav_from_byte(0), Some(Nav::Up));
        assert_eq!(nav_from_byte(6), Some(Nav::Menu));
        assert_eq!(nav_from_byte(7), None);
    }

    #[test]
    fn options_have_sane_defaults() {
        let options = OverlayOptions::default();
        assert!(options.width >= 1280);
        assert!(options.duration.is_none());
        assert!(options.capsule.is_none());
    }
}
