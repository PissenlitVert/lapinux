//! Service d'entrée : lit les manettes, applique le profil, écrit sur le pad virtuel.

use crate::codes;
use crate::device;
use crate::mapping::{Kind, Mapper, Profile, VEvent};
use crate::nav::{Nav, NavRepeats};
use crate::virtual_pad::VirtualPad;
use crate::{Error, Result};
use std::collections::{BTreeMap, HashMap, HashSet};
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
    /// Sources ouvertes lors du scan initial, pour le masquage du jeu.
    initial_sources: Vec<PathBuf>,
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
        let (ready_tx, ready_rx) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("lapinux-input".into())
            .spawn(move || run(profile, grab, remap, tx, stop_thread, ready_tx))
            .map_err(Error::Io)?;
        let service = Self {
            stop,
            thread: Some(thread),
            events: rx,
            initial_sources: Vec::new(),
        };
        // Le pad virtuel est créé dans le fil d'entrée. Attendre qu'il existe
        // évite de lancer le jeu avant son énumération : bwrap masque les
        // sources physiques, donc le jeu doit voir le pad Lapinux dès le
        // premier scan, pas seulement après un branchement à chaud.
        match ready_rx.recv_timeout(Duration::from_secs(2)) {
            Ok(Ok(sources)) => {
                let mut service = service;
                service.initial_sources = sources;
                Ok(service)
            }
            Ok(Err(err)) => {
                let mut service = service;
                service.stop();
                Err(Error::Other(err))
            }
            Err(_) => {
                let mut service = service;
                service.stop();
                Err(Error::Other(
                    "le pad virtuel n'a pas démarré dans les deux secondes".into(),
                ))
            }
        }
    }

    /// Sources ouvertes et synchronisées avant le lancement ; si la capture
    /// est demandée, seules les captures réussies sont incluses.
    pub fn initial_sources(&self) -> &[PathBuf] {
        &self.initial_sources
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
    output: PadOutput,
}

/// État de sortie propre à une manette ; plusieurs sources peuvent alimenter
/// le même pad sans que la déconnexion de l'une relâche l'autre.
#[derive(Default)]
struct PadOutput {
    keys: HashSet<u16>,
    axes: BTreeMap<u16, i32>,
}

impl PadOutput {
    fn apply(&mut self, events: &[VEvent]) {
        for event in events {
            match event.kind {
                Kind::Key if event.value == 0 => {
                    self.keys.remove(&event.code);
                }
                Kind::Key => {
                    self.keys.insert(event.code);
                }
                Kind::Abs => {
                    self.axes.insert(event.code, event.value);
                }
            }
        }
    }
}

#[derive(Clone, Default)]
struct CombinedOutput {
    keys: HashSet<u16>,
    axes: BTreeMap<u16, i32>,
}

impl CombinedOutput {
    fn update<'a>(
        &mut self,
        sources: impl Iterator<Item = (&'a PathBuf, &'a PadOutput)>,
    ) -> Vec<VEvent> {
        let mut sources: Vec<_> = sources.collect();
        sources.sort_by(|(a, _), (b, _)| a.cmp(b));
        let mut next = Self::default();
        for (_, source) in sources {
            next.keys.extend(source.keys.iter().copied());
            for (&code, &value) in &source.axes {
                let current = next.axes.entry(code).or_insert(0);
                if value.unsigned_abs() > current.unsigned_abs() {
                    *current = value;
                }
            }
        }
        let mut events = Vec::new();
        let mut key_codes: Vec<_> = self.keys.union(&next.keys).copied().collect();
        key_codes.sort_unstable();
        for code in key_codes {
            let was = self.keys.contains(&code);
            let now = next.keys.contains(&code);
            if was != now {
                events.push(VEvent::key(code, i32::from(now)));
            }
        }
        let axis_codes: std::collections::BTreeSet<_> =
            self.axes.keys().chain(next.axes.keys()).copied().collect();
        for code in axis_codes {
            let was = self.axes.get(&code).copied().unwrap_or(0);
            let now = next.axes.get(&code).copied().unwrap_or(0);
            if was != now {
                events.push(VEvent::abs(code, now));
            }
        }
        *self = next;
        events
    }
}

/// Repart de l'état du noyau, sans déclencher le raccourci Guide pour un
/// bouton déjà maintenu au moment de l'ouverture ou du retour de l'overlay.
fn sync_binding(binding: &mut PadBinding) -> std::io::Result<()> {
    let keys = binding.device.get_key_state()?;
    let axes = binding.device.get_abs_state()?;
    binding.mapper.reset();
    binding.output = PadOutput::default();
    let shift = binding.mapper.shift_code();
    if let Some(code) = shift.filter(|code| keys.contains(evdev::KeyCode::new(*code))) {
        binding.output.apply(&binding.mapper.on_key(code, 1).0);
    }
    for key in keys.iter() {
        if Some(key.0) != shift {
            binding.output.apply(&binding.mapper.on_key(key.0, 1).0);
        }
    }
    if let Some(supported) = binding.device.supported_absolute_axes() {
        for axis in supported.iter() {
            if let Some(info) = axes.get(axis.0 as usize) {
                binding
                    .output
                    .apply(&binding.mapper.on_abs(axis.0, info.value));
            }
        }
    }
    Ok(())
}

fn emit_combined(
    pad: &mut VirtualPad,
    combined: &mut CombinedOutput,
    pads: &HashMap<PathBuf, PadBinding>,
) -> Result<()> {
    let previous = combined.clone();
    let events = combined.update(pads.iter().map(|(path, binding)| (path, &binding.output)));
    if let Err(err) = pad.emit(&events) {
        *combined = previous;
        return Err(err);
    }
    Ok(())
}

fn run(
    profile: Profile,
    grab: bool,
    remap: bool,
    tx: Sender<InputMsg>,
    stop: Arc<AtomicBool>,
    ready: Sender<std::result::Result<Vec<PathBuf>, String>>,
) {
    let mut virtual_pad = if remap {
        match VirtualPad::create(&format!("{} Virtual Pad", lapinux_core::APP_NAME)) {
            Ok(pad) => Some(pad),
            Err(err) => {
                let _ = ready.send(Err(format!("{err} ; service d'entrée arrêté")));
                return;
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
    let mut nav_repeats = NavRepeats::default();
    let mut last_scan;
    let can_remap = virtual_pad.is_some();
    let should_grab = grab && can_remap;
    // Le pad virtuel est la sortie du service : l'énumérer comme une source
    // le ferait se grabber lui-même, le rendrait invisible au jeu et
    // réinjecterait ses propres événements. On ne garde que les manettes
    // réelles, y compris le pad virtuel du pont Steam.
    let own_pad_name = virtual_pad.as_ref().map(VirtualPad::name);
    if !overlay_navigation() {
        let (_, capture_error) = scan_devices(
            &profile,
            should_grab,
            &pad_chord,
            own_pad_name.as_deref(),
            &mut pads,
        );
        if let Some(error) = capture_error {
            let _ = ready.send(Err(format!(
                "capture impossible ({error}) ; périphériques physiques conservés"
            )));
            return;
        }
    }
    let mut combined = CombinedOutput::default();
    if let Some(pad) = virtual_pad.as_mut() {
        if let Err(err) = emit_combined(pad, &mut combined, &pads) {
            let _ = ready.send(Err(format!("état initial du pad virtuel : {err}")));
            return;
        }
    }
    let mut initial_sources: Vec<_> = pads.keys().cloned().collect();
    initial_sources.sort();
    let _ = ready.send(Ok(initial_sources));
    last_scan = Instant::now();
    let mut previous_overlay_nav = false;

    while !stop.load(Ordering::Relaxed) {
        let overlay_nav = overlay_navigation();
        if overlay_nav != previous_overlay_nav {
            if let Some(pad) = virtual_pad.as_mut() {
                if overlay_nav {
                    let previous = combined.clone();
                    let events = combined.update(std::iter::empty());
                    if let Err(err) = pad.emit(&events) {
                        combined = previous;
                        let _ = tx.send(InputMsg::Error(err.to_string()));
                    }
                } else {
                    for binding in pads.values_mut() {
                        // Consommer les événements en attente avant de lire
                        // l'état actuel pour ne pas rejouer un ancien appui.
                        let drained = match binding.device.fetch_events() {
                            Ok(events) => {
                                for _ in events {}
                                Ok(())
                            }
                            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => Ok(()),
                            Err(err) => Err(err),
                        };
                        if let Err(err) = drained.and_then(|()| sync_binding(binding)) {
                            binding.mapper.reset();
                            binding.output = PadOutput::default();
                            let _ = tx.send(InputMsg::Error(format!(
                                "état {} : {err}",
                                binding.path.display()
                            )));
                        }
                    }
                    if let Err(err) = emit_combined(pad, &mut combined, &pads) {
                        let _ = tx.send(InputMsg::Error(err.to_string()));
                    }
                }
            }
            nav_repeats.clear();
            previous_overlay_nav = overlay_nav;
        }
        if last_scan.elapsed() >= Duration::from_secs(2) {
            last_scan = Instant::now();
            let (changed, _) = scan_devices(
                &profile,
                should_grab,
                &pad_chord,
                own_pad_name.as_deref(),
                &mut pads,
            );
            if changed {
                let _ = tx.send(InputMsg::DevicesChanged);
                if !overlay_nav {
                    if let Some(pad) = virtual_pad.as_mut() {
                        if let Err(err) = emit_combined(pad, &mut combined, &pads) {
                            let _ = tx.send(InputMsg::Error(err.to_string()));
                        }
                    }
                }
            }
        }

        if overlay_nav {
            // Le grab reste en place : le jeu ne voit pas la manette tant que
            // le menu est ouvert. Le service, lui, continue de lire les
            // périphériques et transmet la navigation à l'overlay.
            // Retenter la neutralisation si une écriture uinput a échoué.
            if let Some(pad) = virtual_pad.as_mut() {
                let previous = combined.clone();
                let events = combined.update(std::iter::empty());
                if let Err(err) = pad.emit(&events) {
                    combined = previous;
                    let _ = tx.send(InputMsg::Error(err.to_string()));
                }
            }
            let mut stale = Vec::new();
            for binding in pads.values_mut() {
                let events = match binding.device.fetch_events() {
                    Ok(events) => events,
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => continue,
                    Err(_) => {
                        stale.push(binding.path.clone());
                        continue;
                    }
                };
                for event in events {
                    if let Some(nav) = nav_repeats.update(
                        &binding.path,
                        event.event_type().0,
                        event.code(),
                        event.value(),
                        Instant::now(),
                    ) {
                        let _ = tx.send(InputMsg::Nav(nav));
                    }
                }
            }
            if !stale.is_empty() {
                for path in stale {
                    pads.remove(&path);
                    nav_repeats.remove_device(&path);
                }
                let _ = tx.send(InputMsg::DevicesChanged);
            }
            for nav in nav_repeats.due(Instant::now()) {
                let _ = tx.send(InputMsg::Nav(nav));
            }
            std::thread::sleep(Duration::from_millis(8));
            continue;
        }

        let mut stale: Vec<PathBuf> = Vec::new();
        let mut updates: Vec<(PathBuf, Vec<VEvent>)> = Vec::new();
        for binding in pads.values_mut() {
            let events = match binding.device.fetch_events() {
                Ok(events) => events,
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => continue,
                Err(_) => {
                    stale.push(binding.path.clone());
                    continue;
                }
            };
            let mut hotkey = false;
            for event in events {
                let mapped = match event.event_type().0 {
                    codes::EV_KEY => {
                        let (mapped, hit) = binding.mapper.on_key(event.code(), event.value());
                        hotkey |= hit;
                        mapped
                    }
                    codes::EV_ABS => binding.mapper.on_abs(event.code(), event.value()),
                    _ => Vec::new(),
                };
                if !mapped.is_empty() {
                    updates.push((binding.path.clone(), mapped));
                }
            }
            if hotkey {
                let _ = tx.send(InputMsg::Hotkey);
            }
        }
        // Ne pas fusionner tout le lot : un appui et son relâchement peuvent
        // arriver dans le même fetch_events et doivent tous deux être émis.
        for (path, events) in updates {
            if let Some(binding) = pads.get_mut(&path) {
                binding.output.apply(&events);
            }
            if let Some(pad) = virtual_pad.as_mut() {
                if let Err(err) = emit_combined(pad, &mut combined, &pads) {
                    let _ = tx.send(InputMsg::Error(err.to_string()));
                }
            }
        }
        if !stale.is_empty() {
            for path in stale.drain(..) {
                pads.remove(&path);
            }
            let _ = tx.send(InputMsg::DevicesChanged);
        }
        if let Some(pad) = virtual_pad.as_mut() {
            if let Err(err) = emit_combined(pad, &mut combined, &pads) {
                let _ = tx.send(InputMsg::Error(err.to_string()));
            }
        }

        std::thread::sleep(Duration::from_millis(4));
    }
}

fn scan_devices(
    profile: &Profile,
    grab: bool,
    pad_chord: &[u16],
    own_pad_name: Option<&str>,
    pads: &mut HashMap<PathBuf, PadBinding>,
) -> (bool, Option<String>) {
    let mut changed = false;
    let mut capture_error = None;
    for (path, mut device) in evdev::enumerate() {
        if own_pad_name == device.name() {
            continue;
        }
        if device::is_gamepad(&device) {
            if pads.contains_key(&path) {
                continue;
            }
            if grab {
                match device.grab() {
                    Ok(()) => {}
                    Err(err) => {
                        tracing::warn!("grab {} : {err}", path.display());
                        capture_error.get_or_insert_with(|| format!("{} : {err}", path.display()));
                        continue;
                    }
                }
            }
            if let Err(err) = device.set_nonblocking(true) {
                tracing::warn!("nonblocking {} : {err}", path.display());
                continue;
            }
            let id = device.input_id();
            let mut mapper =
                if lapinux_core::model::is_steam_controller_2026(id.vendor(), id.product()) {
                    let ranges = device.get_abs_state().ok();
                    let range = |code: u16| {
                        ranges
                            .as_ref()
                            .and_then(|axes| axes.get(code as usize))
                            .filter(|info| info.maximum > info.minimum)
                            .map(|info| (info.minimum, info.maximum))
                            .unwrap_or((0, 32767))
                    };
                    Mapper::steam_2026(
                        profile.clone(),
                        range(codes::ABS_HAT2Y),
                        range(codes::ABS_HAT2X),
                    )
                } else {
                    Mapper::new(profile.clone())
                };
            mapper.set_hotkey_chord(pad_chord.to_vec());
            let mut binding = PadBinding {
                path: path.clone(),
                device,
                mapper,
                output: PadOutput::default(),
            };
            if let Err(err) = sync_binding(&mut binding) {
                tracing::warn!("état {} : {err}", path.display());
                continue;
            }
            pads.insert(path, binding);
            changed = true;
        }
    }
    // Purge des périphériques disparus.
    let before = pads.len();
    pads.retain(|path, _| path.exists());
    changed |= pads.len() != before;
    (changed, capture_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codes::*;

    #[test]
    fn disconnect_clears_only_the_disconnected_pads_state() {
        let a = PathBuf::from("event-a");
        let b = PathBuf::from("event-b");
        let mut first = PadOutput::default();
        let mut second = PadOutput::default();
        first.apply(&[VEvent::key(BTN_SOUTH, 1), VEvent::abs(ABS_X, 20000)]);
        second.apply(&[VEvent::key(BTN_SOUTH, 1), VEvent::abs(ABS_X, -10000)]);
        let mut combined = CombinedOutput::default();
        assert_eq!(
            combined.update([(&a, &first), (&b, &second)].into_iter()),
            vec![VEvent::key(BTN_SOUTH, 1), VEvent::abs(ABS_X, 20000)]
        );
        assert_eq!(
            combined.update([(&b, &second)].into_iter()),
            vec![VEvent::abs(ABS_X, -10000)]
        );
        assert_eq!(
            combined.update(std::iter::empty()),
            vec![VEvent::key(BTN_SOUTH, 0), VEvent::abs(ABS_X, 0)]
        );
    }

    #[test]
    fn overlay_neutralizes_then_restores_current_output() {
        let path = PathBuf::from("event-a");
        let mut source = PadOutput::default();
        source.apply(&[VEvent::key(BTN_SOUTH, 1), VEvent::abs(ABS_HAT0Y, -1)]);
        let mut combined = CombinedOutput::default();
        let active = combined.update([(&path, &source)].into_iter());
        assert_eq!(
            active,
            vec![VEvent::key(BTN_SOUTH, 1), VEvent::abs(ABS_HAT0Y, -1)]
        );
        assert_eq!(
            combined.update(std::iter::empty()),
            vec![VEvent::key(BTN_SOUTH, 0), VEvent::abs(ABS_HAT0Y, 0)]
        );
        assert_eq!(combined.update([(&path, &source)].into_iter()), active);
    }

    #[test]
    fn quick_press_and_release_are_not_collapsed() {
        let path = PathBuf::from("event-a");
        let mut source = PadOutput::default();
        let mut combined = CombinedOutput::default();
        source.apply(&[VEvent::key(BTN_SOUTH, 1)]);
        assert_eq!(
            combined.update([(&path, &source)].into_iter()),
            vec![VEvent::key(BTN_SOUTH, 1)]
        );
        source.apply(&[VEvent::key(BTN_SOUTH, 0)]);
        assert_eq!(
            combined.update([(&path, &source)].into_iter()),
            vec![VEvent::key(BTN_SOUTH, 0)]
        );
    }
}
