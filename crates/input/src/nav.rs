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
    kind == EV_ABS && value == 0 && matches!(code, ABS_HAT0X | ABS_HAT0Y | ABS_X | ABS_Y)
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

/// Filtre d'appuis conservé pour les utilisateurs de l'API d'entrée.
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
        if !self.held || now.duration_since(self.last) >= Duration::from_millis(140) {
            self.held = true;
            self.last = now;
            return true;
        }
        false
    }
}

/// Répète les seules directions maintenues, même sans nouveaux événements evdev.
/// Chaque bouton ou axe est suivi séparément pour qu'un relâchement ne coupe
/// pas une autre direction tenue sur la même manette.
#[derive(Default)]
pub struct NavRepeats {
    held: HashMap<(PathBuf, u16, u16), (Nav, Instant)>,
}

impl NavRepeats {
    pub fn update(
        &mut self,
        path: &std::path::Path,
        kind: u16,
        code: u16,
        value: i32,
        now: Instant,
    ) -> Option<Nav> {
        let key = (path.to_path_buf(), kind, code);
        match nav_from_event(kind, code, value) {
            Some((nav, true)) => {
                if self
                    .held
                    .get(&key)
                    .is_some_and(|(previous, _)| *previous == nav)
                {
                    return None;
                }
                self.held
                    .insert(key, (nav, now + Duration::from_millis(350)));
                Some(nav)
            }
            Some((_, false)) => {
                self.held.remove(&key);
                None
            }
            None => {
                if kind == EV_ABS && matches!(code, ABS_HAT0X | ABS_HAT0Y | ABS_X | ABS_Y) {
                    self.held.remove(&key);
                }
                None
            }
        }
    }

    pub fn due(&mut self, now: Instant) -> Vec<Nav> {
        let mut events = Vec::new();
        for (nav, next) in self.held.values_mut() {
            if matches!(nav, Nav::Up | Nav::Down | Nav::Left | Nav::Right) && now >= *next {
                events.push(*nav);
                *next = now + Duration::from_millis(140);
            }
        }
        events
    }

    pub fn remove_device(&mut self, path: &std::path::Path) {
        self.held.retain(|(device, _, _), _| device != path);
    }

    pub fn clear(&mut self) {
        self.held.clear();
    }
}

fn run(tx: std::sync::mpsc::Sender<Nav>, stop: Arc<AtomicBool>) {
    let mut devices: HashMap<PathBuf, evdev::Device> = HashMap::new();
    let mut repeats = NavRepeats::default();
    let mut last_scan = Instant::now() - Duration::from_secs(5);
    while !stop.load(Ordering::Relaxed) {
        if last_scan.elapsed() >= Duration::from_secs(2) {
            last_scan = Instant::now();
            for (path, device) in evdev::enumerate() {
                if devices.contains_key(&path)
                    || crate::device::is_output_pad(device.name())
                    || !crate::device::is_gamepad(&device)
                {
                    continue;
                }
                if device.set_nonblocking(true).is_ok() {
                    devices.insert(path, device);
                }
            }
            devices.retain(|path, _| {
                let exists = path.exists();
                if !exists {
                    repeats.remove_device(path);
                }
                exists
            });
        }

        let mut stale = Vec::new();
        for (path, device) in devices.iter_mut() {
            let events = match device.fetch_events() {
                Ok(events) => events,
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => continue,
                Err(_) => {
                    stale.push(path.clone());
                    continue;
                }
            };
            for event in events {
                if let Some(nav) = repeats.update(
                    path,
                    event.event_type().0,
                    event.code(),
                    event.value(),
                    Instant::now(),
                ) {
                    if tx.send(nav).is_err() {
                        return;
                    }
                }
            }
        }

        for path in stale {
            devices.remove(&path);
            repeats.remove_device(&path);
        }
        for nav in repeats.due(Instant::now()) {
            if tx.send(nav).is_err() {
                return;
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
        assert_eq!(nav_from_event(EV_ABS, ABS_HAT0Y, -1), Some((Nav::Up, true)));
        assert_eq!(
            nav_from_event(EV_ABS, ABS_X, 30_000),
            Some((Nav::Right, true))
        );
        assert_eq!(nav_from_event(EV_ABS, ABS_X, 0), None);
        assert!(axis_released(EV_ABS, ABS_X, 0));
        assert!(!axis_released(EV_KEY, BTN_SOUTH, 0));
    }

    #[test]
    fn virtual_pad_is_not_a_navigation_source() {
        let virtual_pad = format!("{} Virtual Pad", lapinux_core::APP_NAME);
        assert!(crate::device::is_output_pad(Some(virtual_pad.as_str())));
        assert!(!crate::device::is_output_pad(Some(
            "Microsoft X-Box 360 pad"
        )));
        assert!(!crate::device::is_output_pad(None));
    }

    #[test]
    fn held_directions_repeat_without_new_events_but_confirm_never_repeats() {
        let mut repeat = NavRepeats::default();
        let path = std::path::Path::new("manette");
        let now = Instant::now();
        assert_eq!(
            repeat.update(path, EV_KEY, BTN_DPAD_UP, 1, now),
            Some(Nav::Up)
        );
        assert_eq!(
            repeat.update(path, EV_KEY, BTN_SOUTH, 1, now),
            Some(Nav::Confirm)
        );
        assert!(repeat.due(now + Duration::from_millis(349)).is_empty());
        assert_eq!(repeat.due(now + Duration::from_millis(350)), vec![Nav::Up]);
        assert_eq!(
            repeat.update(path, EV_ABS, ABS_X, 30000, now),
            Some(Nav::Right)
        );
        repeat.update(path, EV_ABS, ABS_X, 100, now);
        assert_eq!(repeat.due(now + Duration::from_millis(500)), vec![Nav::Up]);
        repeat.update(path, EV_KEY, BTN_DPAD_UP, 0, now);
        assert!(repeat.due(now + Duration::from_secs(2)).is_empty());
        assert_eq!(
            repeat.update(path, EV_KEY, BTN_DPAD_DOWN, 1, now),
            Some(Nav::Down)
        );
        repeat.remove_device(path);
        assert!(repeat.due(now + Duration::from_secs(2)).is_empty());
    }
}
