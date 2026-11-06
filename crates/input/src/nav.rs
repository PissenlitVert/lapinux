//! Lecture manette pour la navigation dans l'interface (sans grab ni pad virtuel).

use crate::codes::*;
use crate::{Error, Result};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Nav {
    Up,
    Down,
    Left,
    Right,
    Confirm,
    Back,
    Menu,
}

impl Nav {
    /// Octet écrit dans le tube de navigation de l'overlay.
    pub fn wire_byte(self) -> u8 {
        match self {
            Nav::Up => 0,
            Nav::Down => 1,
            Nav::Left => 2,
            Nav::Right => 3,
            Nav::Confirm => 4,
            Nav::Back => 5,
            Nav::Menu => 6,
        }
    }
}

/// Traduit un événement evdev en navigation. Un bouton relâché renvoie aussi
/// sa direction, pour que la répétition sache qu'il est remonté. Les axes ne
/// signalent que l'appui : leur retour au repos est traité par l'appelant.
/// Le service d'entrée et la lecture directe partagent cette table, pour que
/// la manette se comporte pareil partout.
pub fn nav_from_event(kind: u16, code: u16, value: i32) -> Option<(Nav, bool)> {
    match kind {
        EV_KEY => {
            let nav = match code {
                BTN_DPAD_UP => Nav::Up,
                BTN_DPAD_DOWN => Nav::Down,
                BTN_DPAD_LEFT => Nav::Left,
                BTN_DPAD_RIGHT => Nav::Right,
                BTN_SOUTH => Nav::Confirm,
                BTN_EAST => Nav::Back,
                BTN_START | BTN_MODE => Nav::Menu,
                // Stick numérique simulé par les manettes Xbox.
                BTN_THUMBL => Nav::Confirm,
                _ => return None,
            };
            match value {
                1 => Some((nav, true)),
                0 => Some((nav, false)),
                _ => None,
            }
        }
        EV_ABS => match code {
            ABS_HAT0X => match value {
                -1 => Some((Nav::Left, true)),
                1 => Some((Nav::Right, true)),
                _ => None,
            },
            ABS_HAT0Y => match value {
                -1 => Some((Nav::Up, true)),
                1 => Some((Nav::Down, true)),
                _ => None,
            },
            ABS_X => {
                if value > 24000 {
                    Some((Nav::Right, true))
                } else if value < -24000 {
                    Some((Nav::Left, true))
                } else {
                    None
                }
            }
            ABS_Y => {
                if value > 24000 {
                    Some((Nav::Down, true))
                } else if value < -24000 {
                    Some((Nav::Up, true))
                } else {
                    None
                }
            }
            _ => None,
        },
        _ => None,
    }
}

/// Axe revenu au repos : la répétition de toutes ses directions s'arrête.
pub fn axis_released(kind: u16, code: u16, value: i32) -> bool {
    kind == EV_ABS
        && value == 0
        && matches!(code, ABS_HAT0X | ABS_HAT0Y | ABS_X | ABS_Y)
}

pub struct NavReader {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    pub events: Receiver<Nav>,
}

impl NavReader {
    pub fn start() -> Result<Self> {
        let (tx, rx) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("lapinux-nav".into())
            .spawn(move || run(tx, stop_thread))
            .map_err(Error::Io)?;
        Ok(Self {
            stop,
            thread: Some(thread),
            events: rx,
        })
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for NavReader {
    fn drop(&mut self) {
        self.stop();
    }
}

pub struct NavRepeat {
    last: Instant,
    held: bool,
}

impl Default for NavRepeat {
    fn default() -> Self {
        Self {
            last: Instant::now() - Duration::from_secs(1),
            held: false,
        }
    }
}

impl NavRepeat {
    pub fn fire(&mut self, pressed: bool) -> bool {
        if !pressed {
            self.held = false;
            return false;
        }
        let now = Instant::now();
        if !self.held {
            self.held = true;
            self.last = now;
            return true;
        }
        if now.duration_since(self.last) >= Duration::from_millis(140) {
            self.last = now;
            return true;
        }
        false
    }
}

fn run(tx: std::sync::mpsc::Sender<Nav>, stop: Arc<AtomicBool>) {
    let mut devices: HashMap<PathBuf, evdev::Device> = HashMap::new();
    let mut repeats: HashMap<(PathBuf, Nav), NavRepeat> = HashMap::new();
    let mut last_scan = Instant::now() - Duration::from_secs(5);
    while !stop.load(Ordering::Relaxed) {
        if last_scan.elapsed() >= Duration::from_secs(2) {
            last_scan = Instant::now();
            for (path, device) in evdev::enumerate() {
                if devices.contains_key(&path) || !crate::device::is_gamepad(&device) {
                    continue;
                }
                if device.set_nonblocking(true).is_ok() {
                    devices.insert(path, device);
                }
            }
            devices.retain(|path, _| path.exists());
        }

        for (path, device) in devices.iter_mut() {
            let Ok(events) = device.fetch_events() else {
                continue;
            };
            for event in events {
                let kind = event.event_type().0;
                if axis_released(kind, event.code(), event.value()) {
                    repeats.retain(|(device, _), _| device != path);
                    continue;
                }
                let Some((nav, pressed)) =
                    nav_from_event(kind, event.code(), event.value())
                else {
                    continue;
                };
                let repeat = repeats.entry((path.clone(), nav)).or_default();
                if repeat.fire(pressed) && tx.send(nav).is_err() {
                    return;
                }
            }
        }

        std::thread::sleep(Duration::from_millis(8));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buttons_map_to_navigation_and_release() {
        assert_eq!(
            nav_from_event(EV_KEY, BTN_SOUTH, 1),
            Some((Nav::Confirm, true))
        );
        assert_eq!(
            nav_from_event(EV_KEY, BTN_SOUTH, 0),
            Some((Nav::Confirm, false))
        );
        assert_eq!(
            nav_from_event(EV_KEY, BTN_DPAD_LEFT, 1),
            Some((Nav::Left, true))
        );
        assert_eq!(nav_from_event(EV_KEY, BTN_TL, 1), None);
        assert_eq!(
            nav_from_event(EV_ABS, ABS_HAT0Y, -1),
            Some((Nav::Up, true))
        );
        assert_eq!(
            nav_from_event(EV_ABS, ABS_X, 30_000),
            Some((Nav::Right, true))
        );
        assert_eq!(nav_from_event(EV_ABS, ABS_X, 0), None);
        assert!(axis_released(EV_ABS, ABS_X, 0));
        assert!(!axis_released(EV_KEY, BTN_SOUTH, 0));
    }

    #[test]
    fn repeat_has_initial_delay() {
        let mut repeat = NavRepeat::default();
        assert!(repeat.fire(true));
        assert!(!repeat.fire(true));
        repeat.last = Instant::now() - Duration::from_millis(200);
        assert!(repeat.fire(true));
        assert!(!repeat.fire(false));
        assert!(repeat.fire(true));
    }
}
