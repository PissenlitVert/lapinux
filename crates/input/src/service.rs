//! Service d'entrée : lit les manettes, applique le profil, écrit sur le pad virtuel.

use crate::codes;
use crate::device;
use crate::mapping::{Mapper, Profile};
use crate::nav::{axis_released, nav_from_event, Nav, NavRepeat};
use crate::virtual_pad::VirtualPad;
use crate::{Error, Result};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Pendant que l'overlay est ouvert, les manettes pilotent seulement le menu :
/// le service garde le grab (le jeu ne reçoit rien) et transmet la navigation
/// à l'overlay.
static OVERLAY_NAVIGATION: AtomicBool = AtomicBool::new(false);

pub fn set_overlay_navigation(active: bool) {
    OVERLAY_NAVIGATION.store(active, Ordering::Relaxed);
}

fn overlay_navigation() -> bool {
    OVERLAY_NAVIGATION.load(Ordering::Relaxed)
}

#[derive(Debug, Clone, PartialEq)]
pub enum InputMsg {
    /// Le raccourci manette vient d'être pressé.
    Hotkey,
    /// Navigation dans l'overlay, quand il est ouvert.
    Nav(Nav),
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
        Self::start_with(profile, grab, true)
    }

    /// Lecture du raccourci manette (Guide) sans grab ni pad virtuel.
    pub fn start_hotkeys(profile: Profile) -> Result<Self> {
        Self::start_with(profile, false, false)
    }

    fn start_with(profile: Profile, grab: bool, remap: bool) -> Result<Self> {
        let (tx, rx) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("lapinux-input".into())
            .spawn(move || run(profile, grab, remap, tx, stop_thread))
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
    /// Le périphérique est-il exclusivement pris par le service ?
    grabbed: bool,
}

fn run(profile: Profile, grab: bool, remap: bool, tx: Sender<InputMsg>, stop: Arc<AtomicBool>) {
    let mut virtual_pad = if remap {
        match VirtualPad::create(&format!("{} Virtual Pad", lapinux_core::APP_NAME)) {
            Ok(pad) => Some(pad),
            Err(err) => {
                let _ = tx.send(InputMsg::Error(format!(
                    "{err} ; remapping manette désactivé"
                )));
                None
            }
        }
    } else {
        None
    };

    let pad_chord: Vec<u16> = profile
        .hotkey_chord
        .iter()
        .filter_map(|name| codes::button_code(name))
        .collect();
    let mut pads: HashMap<PathBuf, PadBinding> = HashMap::new();
    let mut nav_repeats: HashMap<(PathBuf, Nav), NavRepeat> = HashMap::new();
    let mut last_scan = Instant::now() - Duration::from_secs(10);
    let can_remap = virtual_pad.is_some();
    let should_grab = grab && can_remap;

    while !stop.load(Ordering::Relaxed) {
        let overlay_nav = overlay_navigation();
        if last_scan.elapsed() >= Duration::from_secs(2) {
            last_scan = Instant::now();
            let changed = scan_devices(&profile, should_grab, &pad_chord, &mut pads);
            if changed {
                let _ = tx.send(InputMsg::DevicesChanged);
            }
        }

        if overlay_nav {
            // Le grab reste en place : le jeu ne voit pas la manette tant que
            // le menu est ouvert. Le service, lui, continue de lire les
            // périphériques et transmet la navigation à l'overlay.
            for binding in pads.values_mut() {
                let Ok(events) = binding.device.fetch_events() else {
                    continue;
                };
                for event in events {
                    let kind = event.event_type().0;
                    if axis_released(kind, event.code(), event.value()) {
                        nav_repeats.retain(|(device, _), _| device != &binding.path);
                        continue;
                    }
                    let Some((nav, pressed)) =
                        nav_from_event(kind, event.code(), event.value())
                    else {
                        continue;
                    };
                    let repeat = nav_repeats
                        .entry((binding.path.clone(), nav))
                        .or_default();
                    if repeat.fire(pressed) {
                        let _ = tx.send(InputMsg::Nav(nav));
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(8));
            continue;
        }

        let mut stale: Vec<PathBuf> = Vec::new();
        for binding in pads.values_mut() {
            if should_grab && !binding.grabbed {
                match binding.device.grab() {
                    Ok(()) => binding.grabbed = true,
                    Err(err) => tracing::warn!("grab {} : {err}", binding.path.display()),
                }
            }
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
                let _ = tx.send(InputMsg::Hotkey);
            }
        }
        if !stale.is_empty() {
            for path in stale.drain(..) {
                pads.remove(&path);
            }
            let _ = tx.send(InputMsg::DevicesChanged);
        }

        std::thread::sleep(Duration::from_millis(4));
    }
}

fn scan_devices(
    profile: &Profile,
    grab: bool,
    pad_chord: &[u16],
    pads: &mut HashMap<PathBuf, PadBinding>,
) -> bool {
    let mut changed = false;
    for (path, mut device) in evdev::enumerate() {
        if device::is_gamepad(&device) {
            if pads.contains_key(&path) {
                continue;
            }
            let grabbed = if grab {
                match device.grab() {
                    Ok(()) => true,
                    Err(err) => {
                        tracing::warn!("grab {} : {err}", path.display());
                        false
                    }
                }
            } else {
                false
            };
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
                    grabbed,
                },
            );
            changed = true;
        }
    }
    // Purge des périphériques disparus.
    pads.retain(|path, _| path.exists());
    changed
}
