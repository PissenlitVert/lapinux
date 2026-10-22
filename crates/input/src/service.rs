//! Service d'entrée : lit les manettes, applique le profil, écrit sur le pad virtuel.

use crate::codes;
use crate::device;
use crate::mapping::{Mapper, Profile};
use crate::virtual_pad::VirtualPad;
use crate::{Error, Result};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeySource {
    Pad,
    Keyboard,
}

#[derive(Debug, Clone, PartialEq)]
pub enum InputMsg {
    Hotkey(HotkeySource),
    DevicesChanged,
    Error(String),
}

pub struct InputService {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    pub events: Receiver<InputMsg>,
}

impl InputService {
    pub fn start(profile: Profile, grab: bool) -> Result<Self> {
        let (tx, rx) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("ludus-input".into())
            .spawn(move || run(profile, grab, tx, stop_thread))
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

impl Drop for InputService {
    fn drop(&mut self) {
        self.stop();
    }
}

struct PadBinding {
    path: PathBuf,
    device: evdev::Device,
    mapper: Mapper,
}

struct KeyboardBinding {
    path: PathBuf,
    device: evdev::Device,
    chord: Vec<u16>,
    held: HashSet<u16>,
    armed: bool,
}

fn run(profile: Profile, grab: bool, tx: Sender<InputMsg>, stop: Arc<AtomicBool>) {
    let mut virtual_pad = match VirtualPad::create("Ludus Virtual Pad") {
        Ok(pad) => Some(pad),
        Err(err) => {
            let _ = tx.send(InputMsg::Error(format!(
                "{err} ; remapping manette désactivé"
            )));
            None
        }
    };

    let pad_chord: Vec<u16> = profile
        .hotkey_chord
        .iter()
        .filter_map(|name| codes::button_code(name))
        .collect();
    let keyboard_chord: Vec<u16> = if profile.keyboard_hotkey.is_empty() {
        profile
            .hotkey_chord
            .iter()
            .filter_map(|name| codes::key_code(name))
            .collect()
    } else {
        profile
            .keyboard_hotkey
            .iter()
            .filter_map(|name| codes::key_code(name))
            .collect()
    };

    let mut pads: HashMap<PathBuf, PadBinding> = HashMap::new();
    let mut keyboards: HashMap<PathBuf, KeyboardBinding> = HashMap::new();
    let mut last_scan = Instant::now() - Duration::from_secs(10);
    let can_remap = virtual_pad.is_some();

    while !stop.load(Ordering::Relaxed) {
        if last_scan.elapsed() >= Duration::from_secs(2) {
            last_scan = Instant::now();
            let changed = scan_devices(
                &profile,
                grab && can_remap,
                &pad_chord,
                &keyboard_chord,
                &mut pads,
                &mut keyboards,
            );
            if changed {
                let _ = tx.send(InputMsg::DevicesChanged);
            }
        }

        let mut stale: Vec<PathBuf> = Vec::new();
        for binding in pads.values_mut() {
            let events = match binding.device.fetch_events() {
                Ok(events) => events,
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => continue,
                Err(_) => {
                    stale.push(binding.path.clone());
                    continue;
                }
            };
            let mut out = Vec::new();
            let mut hotkey = false;
            for event in events {
                match event.event_type().0 {
                    codes::EV_KEY => {
                        let (mut mapped, hit) = binding.mapper.on_key(event.code(), event.value());
                        out.append(&mut mapped);
                        hotkey |= hit;
                    }
                    codes::EV_ABS => {
                        out.extend(binding.mapper.on_abs(event.code(), event.value()));
                    }
                    _ => {}
                }
            }
            if let Some(pad) = virtual_pad.as_mut() {
                if let Err(err) = pad.emit(&out) {
                    let _ = tx.send(InputMsg::Error(err.to_string()));
                }
            }
            if hotkey {
                let _ = tx.send(InputMsg::Hotkey(HotkeySource::Pad));
            }
        }
        if !stale.is_empty() {
            for path in stale.drain(..) {
                pads.remove(&path);
            }
            let _ = tx.send(InputMsg::DevicesChanged);
        }

        let mut stale_keyboards: Vec<PathBuf> = Vec::new();
        for keyboard in keyboards.values_mut() {
            let events = match keyboard.device.fetch_events() {
                Ok(events) => events,
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => continue,
                Err(_) => {
                    stale_keyboards.push(keyboard.path.clone());
                    continue;
                }
            };
            let mut triggered = false;
            for event in events {
                if event.event_type().0 != codes::EV_KEY {
                    continue;
                }
                if event.value() == 0 {
                    keyboard.held.remove(&event.code());
                } else {
                    keyboard.held.insert(event.code());
                }
                if chord_hit(&keyboard.held, &keyboard.chord, &mut keyboard.armed) {
                    triggered = true;
                }
            }
            if triggered {
                let _ = tx.send(InputMsg::Hotkey(HotkeySource::Keyboard));
            }
        }
        for path in stale_keyboards {
            keyboards.remove(&path);
        }

        std::thread::sleep(Duration::from_millis(4));
    }
}

fn scan_devices(
    profile: &Profile,
    grab: bool,
    pad_chord: &[u16],
    keyboard_chord: &[u16],
    pads: &mut HashMap<PathBuf, PadBinding>,
    keyboards: &mut HashMap<PathBuf, KeyboardBinding>,
) -> bool {
    let mut changed = false;
    for (path, mut device) in evdev::enumerate() {
        if device::is_gamepad(&device) {
            if pads.contains_key(&path) {
                continue;
            }
            if grab {
                if let Err(err) = device.grab() {
                    tracing::warn!("grab {} : {err}", path.display());
                }
            }
            if let Err(err) = device.set_nonblocking(true) {
                tracing::warn!("nonblocking {} : {err}", path.display());
                continue;
            }
            let mut mapper = Mapper::new(profile.clone());
            mapper.set_hotkey_chord(pad_chord.to_vec());
            pads.insert(
                path.clone(),
                PadBinding {
                    path,
                    device,
                    mapper,
                },
            );
            changed = true;
        } else if !keyboard_chord.is_empty() && device::is_keyboard(&device) {
            if keyboards.contains_key(&path) {
                continue;
            }
            if device.set_nonblocking(true).is_err() {
                continue;
            }
            keyboards.insert(
                path.clone(),
                KeyboardBinding {
                    path,
                    device,
                    chord: keyboard_chord.to_vec(),
                    held: HashSet::new(),
                    armed: false,
                },
            );
            changed = true;
        }
    }
    // Purge des périphériques disparus.
    pads.retain(|path, _| path.exists());
    keyboards.retain(|path, _| path.exists());
    changed
}

pub fn chord_hit(held: &HashSet<u16>, chord: &[u16], armed: &mut bool) -> bool {
    if chord.is_empty() {
        return false;
    }
    let all = chord.iter().all(|code| held.contains(code));
    if all && !*armed {
        *armed = true;
        return true;
    }
    if !all && *armed {
        *armed = false;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chord_triggers_on_complete_press() {
        let chord = vec![codes::KEY_LEFTMETA, codes::KEY_F1];
        let mut held = HashSet::new();
        let mut armed = false;
        held.insert(codes::KEY_LEFTMETA);
        assert!(!chord_hit(&held, &chord, &mut armed));
        held.insert(codes::KEY_F1);
        assert!(chord_hit(&held, &chord, &mut armed));
        assert!(!chord_hit(&held, &chord, &mut armed));
        held.clear();
        assert!(!chord_hit(&held, &chord, &mut armed));
        held.insert(codes::KEY_LEFTMETA);
        held.insert(codes::KEY_F1);
        assert!(chord_hit(&held, &chord, &mut armed));
    }

    #[test]
    fn empty_chord_never_fires() {
        let mut armed = false;
        assert!(!chord_hit(&HashSet::new(), &[], &mut armed));
    }
}
