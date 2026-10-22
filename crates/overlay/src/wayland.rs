//! Client Wayland minimal : surface wlr-layer-shell de type overlay.

use crate::menu::{Action, MenuState};
use crate::render::{panel_bounds, Renderer};
use crate::shm::Mapping;
use crate::{Error, Result};
use std::os::fd::{AsRawFd, BorrowedFd};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_keyboard, wl_pointer, wl_registry, wl_seat, wl_shm, wl_shm_pool,
    wl_surface,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

pub struct OverlayOptions {
    pub width: u32,
    pub height: u32,
    /// Si défini, l'overlay se ferme après cette durée (mode démo).
    pub duration: Option<Duration>,
    pub font: Option<PathBuf>,
}

impl Default for OverlayOptions {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            duration: None,
            font: None,
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
    shm: wl_shm::WlShm,
    qh: QueueHandle<App>,
    surface: Option<wl_surface::WlSurface>,
    layer: Option<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    pointer: Option<wl_pointer::WlPointer>,
    pointer_pos: (f32, f32),
    width: u32,
    height: u32,
    configured: bool,
    slots: Vec<Slot>,
    on_action: Box<dyn FnMut(Action)>,
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
            self.renderer = Some(Renderer::new(&self.font_path, self.width, self.height)?);
        }
        Ok(())
    }

    fn set_selected(&mut self, index: usize) {
        if index < self.menu.items().len() {
            self.menu.selected = index;
            self.menu.pulse = 1.0;
        }
    }

    fn dispatch_action(&mut self, action: Option<Action>) {
        if let Some(action) = action {
            if matches!(action, Action::Resume | Action::QuitGame) {
                self.exit = true;
            }
            (self.on_action)(action);
        }
    }

    fn on_keycode(&mut self, keycode: u32) {
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
                self.exit = true;
            }
            _ => return,
        }
        self.menu.pulse = 1.0;
    }

    fn click_at(&mut self, x: f32, y: f32) {
        let (panel_x, panel_y, panel_w, _panel_h) =
            panel_bounds(self.width, self.height, &self.menu);
        if x < panel_x || x > panel_x + panel_w {
            return;
        }
        let local_y = y - panel_y;
        if let Some(row) = self.menu.hit_row(local_y) {
            self.set_selected(row);
            let action = self.menu.activate();
            self.dispatch_action(action);
        }
    }

    fn draw(&mut self, conn: &Connection) -> Result<()> {
        if !self.configured {
            return Ok(());
        }
        self.ensure_renderer()?;
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
                slot.width == width && slot.height == height && slot.released.load(Ordering::Relaxed)
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
        slot.mapping.write(pixmap.data());
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
        if let wl_keyboard::Event::Key {
            key,
            state: key_state,
            ..
        } = event
        {
            if let WEnum::Value(wl_keyboard::KeyState::Pressed) = key_state {
                app.on_keycode(key + 8);
                let _ = app.draw(conn);
            }
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
                surface_x,
                surface_y,
                ..
            }
            | wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => {
                app.pointer_pos = (surface_x as f32, surface_y as f32);
            }
            wl_pointer::Event::Button {
                button,
                state: button_state,
                ..
            } => {
                if button == 0x110 {
                    if let WEnum::Value(wl_pointer::ButtonState::Pressed) = button_state {
                        let (x, y) = app.pointer_pos;
                        app.click_at(x, y);
                        let _ = app.draw(conn);
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

pub fn run(
    menu: MenuState,
    options: OverlayOptions,
    on_action: impl FnMut(Action) + 'static,
) -> Result<()> {
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
    let layer_shell: zwlr_layer_shell_v1::ZwlrLayerShellV1 = globals
        .bind(&qh, 4..=4, ())
        .map_err(|e| Error::Wayland(format!("wlr-layer-shell indisponible : {e}")))?;
    let seat: Option<wl_seat::WlSeat> = globals.bind(&qh, 1..=8, ()).ok();

    let width = options.width.max(320);
    let height = options.height.max(240);
    let mut app = App {
        menu,
        renderer: None,
        font_path,
        shm,
        qh: qh.clone(),
        surface: None,
        layer: None,
        keyboard: None,
        pointer: None,
        pointer_pos: (0.0, 0.0),
        width,
        height,
        configured: false,
        slots: Vec::new(),
        on_action: Box::new(on_action),
        deadline: options.duration.map(|d| Instant::now() + d),
        exit: false,
    };
    app.ensure_renderer()?;

    let surface = compositor.create_surface(&qh, ());
    let layer = layer_shell.get_layer_surface(
        &surface,
        None,
        zwlr_layer_shell_v1::Layer::Overlay,
        "ludus-overlay".to_string(),
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
    layer.set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::OnDemand);
    layer.set_size(width, height);
    app.surface = Some(surface.clone());
    app.layer = Some(layer);
    app.keyboard = seat.as_ref().map(|s| s.get_keyboard(&qh, ()));
    app.pointer = seat.as_ref().map(|s| s.get_pointer(&qh, ()));
    surface.commit();

    queue
        .roundtrip(&mut app)
        .map_err(|e| Error::Wayland(format!("configure : {e}")))?;
    if !app.configured {
        return Err(Error::Wayland(
            "aucun configure reçu du compositeur".into(),
        ));
    }

    // Animation d'ouverture (quelques images seulement).
    for _ in 0..9 {
        app.menu.tick(0.016);
        app.draw(&conn)?;
        std::thread::sleep(Duration::from_millis(16));
    }

    loop {
        if app.exit {
            break;
        }
        if let Some(deadline) = app.deadline {
            if Instant::now() >= deadline {
                break;
            }
            app.menu.tick(0.016);
            app.draw(&conn)?;
            std::thread::sleep(Duration::from_millis(16));
            continue;
        }
        queue
            .flush()
            .map_err(|e| Error::Wayland(format!("flush : {e}")))?;
        if let Some(guard) = queue.prepare_read() {
            guard
                .read()
                .map_err(|e| Error::Wayland(format!("lecture : {e}")))?;
        }
        queue
            .dispatch_pending(&mut app)
            .map_err(|e| Error::Wayland(format!("dispatch : {e}")))?;
        if app.menu.tick(0.016) {
            app.draw(&conn)?;
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

    #[test]
    fn options_have_sane_defaults() {
        let options = OverlayOptions::default();
        assert!(options.width >= 1280);
        assert!(options.duration.is_none());
    }
}
