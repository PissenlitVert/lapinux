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
            .name("ludus-nav".into())
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

struct Repeat {
    last: Instant,
    held: bool,
}

impl Default for Repeat {
    fn default() -> Self {
        Self {
            last: Instant::now() - Duration::from_secs(1),
            held: false,
        }
    }
}

impl Repeat {
    fn fire(&mut self, pressed: bool) -> bool {
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
    let mut repeats: HashMap<(PathBuf, Nav), Repeat> = HashMap::new();
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
                let nav = match event.event_type().0 {
                    EV_KEY => match (event.code(), event.value()) {
                        (c, 1) if c == BTN_DPAD_UP => Some(Nav::Up),
                        (c, 1) if c == BTN_DPAD_DOWN => Some(Nav::Down),
                        (c, 1) if c == BTN_DPAD_LEFT => Some(Nav::Left),
                        (c, 1) if c == BTN_DPAD_RIGHT => Some(Nav::Right),
                        (c, 1) if c == BTN_SOUTH => Some(Nav::Confirm),
                        (c, 1) if c == BTN_EAST => Some(Nav::Back),
                        (c, 1) if c == BTN_START => Some(Nav::Menu),
                        // Stick numérique simulé par les manettes Xbox.
                        (c, 1) if c == BTN_THUMBL => Some(Nav::Confirm),
                        _ => None,
                    },
                    EV_ABS => match event.code() {
                        ABS_HAT0X => {
                            match event.value() {
                                -1 => Some(Nav::Left),
                                1 => Some(Nav::Right),
                                _ => None,
                            }
                        }
                        ABS_HAT0Y => {
                            match event.value() {
                                -1 => Some(Nav::Up),
                                1 => Some(Nav::Down),
                                _ => None,
                            }
                        }
                        ABS_X => {
                            if event.value() > 24000 {
                                Some(Nav::Right)
                            } else if event.value() < -24000 {
                                Some(Nav::Left)
                            } else {
                                None
                            }
                        }
                        ABS_Y => {
                            if event.value() > 24000 {
                                Some(Nav::Down)
                            } else if event.value() < -24000 {
                                Some(Nav::Up)
                            } else {
                                None
                            }
                        }
                        _ => None,
                    },
                    _ => None,
                };
                if let Some(nav) = nav {
                    let pressed = match event.event_type().0 {
                        EV_KEY => event.value() == 1,
                        EV_ABS => event.value() != 0,
                        _ => false,
                    };
                    let repeat = repeats.entry((path.clone(), nav)).or_default();
                    if repeat.fire(pressed) && tx.send(nav).is_err() {
                        return;
                    }
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
    fn repeat_has_initial_delay() {
        let mut repeat = Repeat::default();
        assert!(repeat.fire(true));
        assert!(!repeat.fire(true));
        repeat.last = Instant::now() - Duration::from_millis(200);
        assert!(repeat.fire(true));
        assert!(!repeat.fire(false));
        assert!(repeat.fire(true));
    }
}
