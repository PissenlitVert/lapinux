//! Aperçu en direct des manettes : lecture evdev sans grab ni pad virtuel.
//!
//! L'interface affiche les boutons pressés, la position des sticks, les
//! gâchettes, le pavé tactile et le gyroscope. Le lecteur peut tourner en même
//! temps que la navigation (`NavReader`) : evdev diffuse les événements à tous
//! les lecteurs.
//!
//! Certaines manettes exposent leurs capteurs sur des nœuds evdev séparés
//! (« Motion Sensors », « Touchpad ») qui partagent le même périphérique HID.
//! Ils sont rattachés à la manette par ce parent commun, puis fusionnés dans
//! un seul [`PadState`].

use crate::battery::{self, BatteryInfo, BatteryReading, BatteryWorker};
use crate::codes::{self, *};
use crate::{Error, Result};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const ABS_GAS: u16 = 0x09;
const ABS_BRAKE: u16 = 0x0a;
const ABS_MT_SLOT: u16 = 0x2f;
const ABS_MT_POSITION_X: u16 = 0x35;
const ABS_MT_POSITION_Y: u16 = 0x36;
const ABS_MT_TRACKING_ID: u16 = 0x39;
const BTN_TOUCH: u16 = 0x14a;

/// Les capteurs émettent à 125–250 Hz : publier à la fréquence de l'interface
/// évite de copier tout l'état à chaque échantillon.
const PUBLISH_INTERVAL: Duration = Duration::from_millis(33);

/// État des quatre boutons de façade, indépendamment des libellés de marque.
/// La position physique (sud, est, nord, ouest) est la seule information que
/// le pilote evdev garantit ; l'interface choisit ensuite ✕/○/△/□ ou A/B/X/Y.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FaceButtons {
    pub south: bool,
    pub east: bool,
    pub north: bool,
    pub west: bool,
}

/// Point de contact du pavé tactile, normalisé de 0 à 1 dans le rectangle du
/// pavé (origine en haut à gauche).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TouchPoint {
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct TouchpadState {
    pub points: Vec<TouchPoint>,
}

/// Valeurs normalisées de −1 à 1. Pour le gyroscope, ±1 correspond à
/// ±2048 °/s ; pour l'accéléromètre, ±1 correspond à ±4 g. Le pilote ne
/// fournit pas la calibration usine : ces valeurs sont une estimation.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MotionState {
    pub gyro: [f32; 3],
    pub accel: [f32; 3],
}

/// Cadence des rapports HID, comptée sur le flux brut de la manette : c'est
/// l'intervalle réel entre deux trames, donc une bonne mesure de la réactivité
/// de la liaison. Il est mesuré fenêtre par fenêtre pour rester stable.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ReportCadence {
    pub interval_ms: f32,
    pub rate_hz: f32,
}

/// État d'une manette à un instant donné.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PadState {
    pub path: PathBuf,
    pub name: String,
    /// Libellés des boutons actuellement pressés (A, LB, Haut…).
    pub pressed: Vec<String>,
    /// Axes normalisés : left_x, left_y, right_x, right_y, left_trigger,
    /// right_trigger, dpad_x, dpad_y.
    pub axes: BTreeMap<String, f32>,
    /// Les quatre boutons de façade, dans leur position physique.
    pub face: FaceButtons,
    /// `Some` dès que la manette expose un pavé tactile, même au repos.
    pub touchpad: Option<TouchpadState>,
    /// `Some` dès que la manette expose des capteurs de mouvement.
    pub motion: Option<MotionState>,
    /// État du bouton mute du microphone, quand la manette l'expose.
    pub microphone_muted: Option<bool>,
    /// Batterie enrichie : palier firmware, tension et compteurs sonde.
    pub battery: Option<BatteryInfo>,
    /// Intervalle et débit des rapports d'entrée.
    pub report: Option<ReportCadence>,
    /// Horodatage (secondes Unix) du dernier événement reçu. Un récepteur
    /// 2,4 GHz garde son nœud evdev même manette éteinte : seule cette activité
    /// distingue une manette réveillée d'un dongle branché à vide. `None` tant
    /// qu'aucune entrée n'a été vue depuis le lancement du moniteur.
    pub last_input_unix: Option<u64>,
}

impl PadState {
    pub fn axis(&self, name: &str) -> f32 {
        self.axes.get(name).copied().unwrap_or(0.0)
    }
}

/// Thread de lecture partagé par la page Manettes.
pub struct PadMonitor {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    snapshot: Arc<Mutex<Vec<PadState>>>,
}

impl PadMonitor {
    pub fn start() -> Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let snapshot = Arc::new(Mutex::new(Vec::new()));
        let thread_stop = Arc::clone(&stop);
        let thread_snapshot = Arc::clone(&snapshot);
        let thread = std::thread::Builder::new()
            .name("lapinux-pad-monitor".into())
            .spawn(move || run(thread_snapshot, thread_stop))
            .map_err(Error::Io)?;
        Ok(Self {
            stop,
            thread: Some(thread),
            snapshot,
        })
    }

    pub fn snapshot(&self) -> Vec<PadState> {
        self.snapshot
            .lock()
            .map(|state| state.clone())
            .unwrap_or_default()
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for PadMonitor {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompanionKind {
    Motion,
    Touchpad,
}

enum CompanionState {
    Motion {
        gyro: [f32; 3],
        accel: [f32; 3],
    },
    Touchpad {
        slot: i32,
        slots: BTreeMap<i32, (f32, f32)>,
    },
}

struct Companion {
    path: PathBuf,
    device: evdev::Device,
    state: CompanionState,
    /// Bornes de chaque axe, lues une fois à la connexion.
    ranges: HashMap<u16, (i32, i32)>,
    alive: bool,
}

struct Pad {
    device: evdev::Device,
    name: String,
    vendor: u16,
    product: u16,
    pressed: HashSet<u16>,
    axes: BTreeMap<String, f32>,
    /// Bornes de chaque axe, lues une fois à la connexion.
    ranges: HashMap<u16, (i32, i32)>,
    /// Périphérique HID commun : sert à rattacher capteurs et pavé tactile.
    group: String,
    companions: Vec<Companion>,
    /// Repli quand les nœuds evdev des capteurs ne sont pas accessibles :
    /// la DualSense expose les mêmes données dans ses rapports HID bruts.
    hidraw: Option<std::fs::File>,
    hidraw_motion: Option<MotionState>,
    hidraw_touchpad: Option<TouchpadState>,
    hidraw_muted: Option<bool>,
    hidraw_battery: Option<BatteryReading>,
    /// Pavé tactile exposé directement par le nœud evdev principal (manettes
    /// génériques, Steam Controller) : contacts suivis comme les satellites.
    has_touchpad: bool,
    touchpad_slot: i32,
    touchpad_slots: BTreeMap<i32, (f32, f32)>,
    /// Sondes de batterie Sony (tension et compteurs) hors du fil evdev.
    battery_worker: Option<BatteryWorker>,
    battery_revision: u64,
    /// Compteur de rapports HID sur une fenêtre courte, pour le débit réel.
    packet_window: Instant,
    packet_count: u32,
    report_rate: Option<f32>,
    /// Repli evdev : les manettes non Sony n'ont pas de flux hidraw décodable,
    /// mais chaque trame livrée au noyau se termine par un SYN_REPORT.
    evdev_window: Instant,
    evdev_frames: u32,
    evdev_rate: Option<f32>,
    /// Dernier événement reçu, quel qu'il soit (trame vide comprise).
    last_input: Option<Instant>,
}

impl Pad {
    fn new(device: evdev::Device, name: String, group: String) -> Option<Self> {
        if device.set_nonblocking(true).is_err() {
            return None;
        }
        let id = device.input_id();
        let has_touchpad = device.supported_absolute_axes().map_or(false, |axes| {
            axes.contains(evdev::AbsoluteAxisCode(ABS_MT_POSITION_X))
                && axes.contains(evdev::AbsoluteAxisCode(ABS_MT_POSITION_Y))
        });
        let mut pad = Self {
            device,
            name,
            vendor: id.vendor(),
            product: id.product(),
            pressed: HashSet::new(),
            axes: BTreeMap::new(),
            ranges: HashMap::new(),
            group,
            companions: Vec::new(),
            hidraw: None,
            hidraw_motion: None,
            hidraw_touchpad: None,
            hidraw_muted: None,
            hidraw_battery: None,
            has_touchpad,
            touchpad_slot: 0,
            touchpad_slots: BTreeMap::new(),
            battery_worker: None,
            battery_revision: 0,
            packet_window: Instant::now(),
            packet_count: 0,
            report_rate: None,
            evdev_window: Instant::now(),
            evdev_frames: 0,
            evdev_rate: None,
            last_input: None,
        };
        if let Ok(abs) = pad.device.get_abs_state() {
            for (code, info) in abs.iter().enumerate() {
                if info.maximum <= info.minimum {
                    continue;
                }
                let code = code as u16;
                pad.ranges.insert(code, (info.minimum, info.maximum));
                if let Some(key) = pad_axis_key(pad.vendor, code) {
                    pad.axes.insert(
                        key.to_string(),
                        normalize_axis(code, info.value, info.minimum, info.maximum),
                    );
                }
            }
        }
        pad.open_hidraw();
        Some(pad)
    }

    /// Ouvre le hidraw de la manette pour lire ses capteurs sans dépendre des
    /// ACL udev de ses nœuds evdev satellites. La DualSense y expose le pavé
    /// tactile et le bouton mute ; les manettes Switch y exposent leur IMU et
    /// la cadence réelle des rapports, que le nœud evdev ne donne pas au repos.
    fn open_hidraw(&mut self) {
        if self.hidraw.is_some() {
            return;
        }
        let sony = self.vendor == 0x054c && matches!(self.product, 0x0ce6 | 0x0df2);
        let nintendo =
            self.vendor == 0x057e && matches!(self.product, 0x2006 | 0x2007 | 0x2009 | 0x200e);
        if !sony && !nintendo {
            return;
        }
        let Some(path) = find_hidraw(&self.group, self.vendor, self.product) else {
            return;
        };
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&path);
        if let Ok(file) = file {
            self.hidraw = Some(file);
            // Le fil de sonde Sony a son propre descripteur : une commande de
            // test lente ne doit jamais retarder la lecture des boutons. Les
            // manettes Nintendo n'ont pas cette sonde.
            if sony && self.battery_worker.is_none() {
                self.battery_worker =
                    BatteryWorker::start(path, self.product, self.group.starts_with("uniq:"));
            }
        }
    }

    /// Lit quelques rapports HID sans bloquer le fil de lecture.
    fn read_hidraw(&mut self) -> bool {
        let mut packets: Vec<Vec<u8>> = Vec::new();
        let mut lost = false;
        if let Some(file) = self.hidraw.as_mut() {
            for _ in 0..8 {
                let mut buffer = [0u8; 128];
                match file.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => packets.push(buffer[..n].to_vec()),
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(_) => {
                        lost = true;
                        break;
                    }
                }
            }
        }
        if lost {
            self.hidraw = None;
            // Le descripteur du fil de sonde est mort avec la connexion ; il
            // sera recréé au prochain scan.
            self.battery_worker = None;
            self.battery_revision = 0;
            self.report_rate = None;
            self.packet_count = 0;
            self.packet_window = Instant::now();
        }
        if !packets.is_empty() {
            // Le flux brut sort à ~300 Hz sur une DualSense Bluetooth ; on
            // compte les trames sur une demi-seconde pour lisser la lecture.
            self.packet_count = self.packet_count.saturating_add(packets.len() as u32);
            let elapsed = self.packet_window.elapsed();
            if elapsed >= Duration::from_millis(500) {
                self.report_rate = Some(self.packet_count as f32 / elapsed.as_secs_f32());
                self.packet_count = 0;
                self.packet_window = Instant::now();
            }
        }
        let mut changed = false;
        for packet in &packets {
            changed |= if self.vendor == 0x057e {
                self.parse_nintendo(packet)
            } else {
                self.parse_dualsense(packet)
            };
        }
        changed
    }

    fn parse_dualsense(&mut self, data: &[u8]) -> bool {
        let Some((motion, touchpad, muted)) = decode_dualsense(data) else {
            return false;
        };
        self.hidraw_motion = Some(motion);
        self.hidraw_touchpad = Some(touchpad);
        self.hidraw_muted = Some(muted);
        if let Some(reading) = battery::decode_dualsense_battery(data) {
            self.hidraw_battery = Some(reading);
        }
        true
    }

    fn parse_nintendo(&mut self, data: &[u8]) -> bool {
        let mut changed = false;
        if let Some(motion) = decode_nintendo(data) {
            self.hidraw_motion = Some(motion);
            changed = true;
        }
        if let Some(reading) = battery::decode_nintendo_battery(data) {
            // Le décodeur ne publie jamais 100 % pour le dernier palier : le
            // firmware l'annonce « pleine » dès le branchement (et même à
            // ~80 %), le palier 80–89 % est plus honnête et ne saute plus.
            self.hidraw_battery = Some(reading);
            changed = true;
        }
        changed
    }

    fn add_companion(&mut self, path: PathBuf, device: evdev::Device, name: &str) {
        let Some(kind) = companion_kind(name) else {
            return;
        };
        if self.companions.iter().any(|c| c.path == path) {
            return;
        }
        if device.set_nonblocking(true).is_err() {
            return;
        }
        let mut ranges = HashMap::new();
        if let Ok(abs) = device.get_abs_state() {
            for (code, info) in abs.iter().enumerate() {
                if info.maximum > info.minimum {
                    ranges.insert(code as u16, (info.minimum, info.maximum));
                }
            }
        }
        self.companions.push(Companion {
            path,
            device,
            state: match kind {
                CompanionKind::Motion => CompanionState::Motion {
                    gyro: [0.0; 3],
                    accel: [0.0; 3],
                },
                CompanionKind::Touchpad => CompanionState::Touchpad {
                    slot: 0,
                    slots: BTreeMap::new(),
                },
            },
            ranges,
            alive: true,
        });
    }

    /// Renvoie vrai quand l'état affiché a changé.
    fn handle_event(&mut self, event: &evdev::InputEvent) -> bool {
        match event.event_type().0 {
            EV_KEY => {
                let code = event.code();
                // Un pavé tactile intégré au nœud principal signale aussi ses
                // contacts par BTN_TOUCH ; ce n'est pas un bouton de manette.
                if code == BTN_TOUCH && self.has_touchpad {
                    if event.value() == 0 {
                        self.touchpad_slots.clear();
                    } else {
                        self.touchpad_slot = 0;
                        self.touchpad_slots.entry(0).or_insert((0.5, 0.5));
                    }
                    return true;
                }
                if button_label(code).is_none() {
                    return false;
                }
                if event.value() == 0 {
                    self.pressed.remove(&code);
                } else {
                    self.pressed.insert(code);
                }
                true
            }
            EV_ABS => {
                let code = event.code();
                if self.has_touchpad
                    && matches!(
                        code,
                        ABS_MT_SLOT | ABS_MT_TRACKING_ID | ABS_MT_POSITION_X | ABS_MT_POSITION_Y
                    )
                {
                    return self.handle_primary_touch(code, event.value());
                }
                let Some(key) = pad_axis_key(self.vendor, code) else {
                    return false;
                };
                let (min, max) = self
                    .ranges
                    .get(&code)
                    .copied()
                    .unwrap_or_else(|| default_range(code));
                let value = normalize_axis(code, event.value(), min, max);
                self.axes.insert(key.to_string(), value);
                true
            }
            _ => false,
        }
    }

    /// Contacts du pavé tactile porté par le nœud principal. Les bornes sont
    /// celles qu'evdev expose ; sans elles, on retombe sur les tailles des
    /// pavés courants plutôt que d'inventer une position.
    fn handle_primary_touch(&mut self, code: u16, value: i32) -> bool {
        let (min, max) = self
            .ranges
            .get(&code)
            .copied()
            .unwrap_or_else(|| match code {
                ABS_MT_POSITION_X => (0, 1919),
                ABS_MT_POSITION_Y => (0, 1079),
                _ => (0, 0),
            });
        touch_event(
            &mut self.touchpad_slot,
            &mut self.touchpad_slots,
            code,
            value,
            min,
            max,
        )
    }

    /// Compte les trames evdev : cadence de secours pour les manettes dont on
    /// ne décode pas le flux hidraw (tout ce qui n'est pas Sony). Renvoie vrai
    /// quand une nouvelle fenêtre est close, pour publier le nouveau débit
    /// même si aucune touche n'a bougé.
    fn note_evdev_frames(&mut self, frames: u32) -> bool {
        self.evdev_frames = self.evdev_frames.saturating_add(frames);
        let elapsed = self.evdev_window.elapsed();
        if elapsed >= Duration::from_millis(500) {
            self.evdev_rate = Some(self.evdev_frames as f32 / elapsed.as_secs_f32());
            self.evdev_frames = 0;
            self.evdev_window = Instant::now();
            return true;
        }
        false
    }

    /// Intervalle et débit des rapports HID de la dernière fenêtre. Le flux
    /// hidraw est plus précis ; evdev prend le relais pour les autres manettes.
    fn report_cadence(&self) -> Option<ReportCadence> {
        let evdev_rate = self
            .evdev_rate
            .filter(|_| self.evdev_window.elapsed() < Duration::from_secs(2));
        let rate = self.report_rate.or(evdev_rate)?;
        (rate > 1.0).then(|| ReportCadence {
            interval_ms: 1000.0 / rate,
            rate_hz: rate,
        })
    }
}

impl Companion {
    fn handle_event(&mut self, event: &evdev::InputEvent) -> bool {
        match event.event_type().0 {
            EV_ABS => {
                let code = event.code();
                let value = event.value();
                let kind = match &self.state {
                    CompanionState::Motion { .. } => CompanionKind::Motion,
                    CompanionState::Touchpad { .. } => CompanionKind::Touchpad,
                };
                let (min, max) = self
                    .ranges
                    .get(&code)
                    .copied()
                    .unwrap_or_else(|| companion_range(kind, code));
                match &mut self.state {
                    CompanionState::Motion { gyro, accel } => {
                        let Some(index) = motion_index(code) else {
                            return false;
                        };
                        let value = motion_norm(value, min, max);
                        if matches!(code, ABS_RX | ABS_RY | ABS_RZ) {
                            gyro[index] = value;
                        } else {
                            accel[index] = value;
                        }
                        true
                    }
                    CompanionState::Touchpad { slot, slots } => {
                        touch_event(slot, slots, code, value, min, max)
                    }
                }
            }
            EV_KEY if event.code() == BTN_TOUCH => {
                if let CompanionState::Touchpad { slot, slots } = &mut self.state {
                    if event.value() == 0 {
                        slots.clear();
                    } else {
                        *slot = 0;
                        slots.entry(0).or_insert((0.5, 0.5));
                    }
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }
}

fn run(snapshot: Arc<Mutex<Vec<PadState>>>, stop: Arc<AtomicBool>) {
    let mut pads: HashMap<PathBuf, Pad> = HashMap::new();
    let mut last_scan = Instant::now() - Duration::from_secs(10);
    let mut last_publish = Instant::now() - Duration::from_secs(1);
    let mut dirty = false;
    while !stop.load(Ordering::Relaxed) {
        let mut changed = false;
        if last_scan.elapsed() >= Duration::from_secs(2) {
            last_scan = Instant::now();
            let before = pads.len();
            scan(&mut pads);
            if pads.len() != before {
                changed = true;
            }
        }

        let mut stale: Vec<PathBuf> = Vec::new();
        for (path, pad) in pads.iter_mut() {
            let mut alive = true;
            let fetched = match pad.device.fetch_events() {
                Ok(events) => Some(events.collect::<Vec<_>>()),
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => None,
                Err(_) => {
                    alive = false;
                    None
                }
            };
            if let Some(events) = fetched {
                if !events.is_empty() {
                    // Même une trame sans changement prouve que la manette
                    // parle encore au récepteur.
                    pad.last_input = Some(Instant::now());
                }
                let frames = events
                    .iter()
                    .filter(|event| event.event_type().0 == EV_SYN && event.code() == 0)
                    .count() as u32;
                if frames > 0 && pad.note_evdev_frames(frames) {
                    changed = true;
                }
                for event in &events {
                    if pad.handle_event(event) {
                        changed = true;
                    }
                }
            }
            for companion in &mut pad.companions {
                if !companion.alive {
                    continue;
                }
                let fetched = match companion.device.fetch_events() {
                    Ok(events) => Some(events.collect::<Vec<_>>()),
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => None,
                    Err(_) => {
                        companion.alive = false;
                        None
                    }
                };
                if let Some(events) = fetched {
                    for event in &events {
                        if companion.handle_event(event) {
                            changed = true;
                        }
                    }
                }
            }
            if pad.read_hidraw() {
                changed = true;
            }
            if let Some(worker) = &pad.battery_worker {
                let revision = worker.revision();
                if revision != pad.battery_revision {
                    pad.battery_revision = revision;
                    changed = true;
                }
            }
            if !alive {
                stale.push(path.clone());
            }
        }
        for path in stale {
            pads.remove(&path);
            changed = true;
        }
        if changed {
            dirty = true;
        }
        // Une manette devenue silencieuse garde son dernier état ; on publie
        // au plus à la cadence de l'interface, pas à celle des capteurs.
        if dirty && last_publish.elapsed() >= PUBLISH_INTERVAL {
            publish(&pads, &snapshot);
            dirty = false;
            last_publish = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(8));
    }
    if let Ok(mut guard) = snapshot.lock() {
        guard.clear();
    }
}

fn scan(pads: &mut HashMap<PathBuf, Pad>) {
    let mut devices: Vec<(PathBuf, evdev::Device)> = evdev::enumerate()
        .filter(|(path, device)| path.exists() && !crate::device::is_output_pad(device.name()))
        .collect();

    // 1. Les nouvelles manettes emportent leurs nœuds satellites.
    let mut index = 0;
    while index < devices.len() {
        let is_gamepad = {
            let (path, device) = &devices[index];
            !pads.contains_key(path) && crate::device::is_gamepad(device)
        };
        if !is_gamepad {
            index += 1;
            continue;
        }
        let (path, device) = devices.remove(index);
        let name = device.name().unwrap_or("manette").to_string();
        let group = group_key(&path, &device);
        let Some(mut pad) = Pad::new(device, name, group.clone()) else {
            continue;
        };
        let mut i = 0;
        while i < devices.len() {
            let take = {
                let (cpath, cdev) = &devices[i];
                let cname = cdev.name().unwrap_or("");
                companion_kind(cname).is_some() && group_key(cpath, cdev) == group
            };
            if take {
                let (cpath, cdev) = devices.remove(i);
                let cname = cdev.name().unwrap_or("capteur").to_string();
                pad.add_companion(cpath, cdev, &cname);
            } else {
                i += 1;
            }
        }
        pads.insert(path, pad);
    }

    // 2. Un capteur branché après coup rejoint sa manette déjà connue.
    let mut i = 0;
    while i < devices.len() {
        let cname = devices[i].1.name().unwrap_or("").to_string();
        if companion_kind(&cname).is_none() {
            i += 1;
            continue;
        }
        let group = group_key(&devices[i].0, &devices[i].1);
        if let Some(pad) = pads.values_mut().find(|pad| pad.group == group) {
            let (cpath, cdev) = devices.remove(i);
            pad.add_companion(cpath, cdev, &cname);
        } else {
            i += 1;
        }
    }
    pads.retain(|path, _| path.exists());
    for pad in pads.values_mut() {
        pad.open_hidraw();
    }
}

/// Trouve le hidraw du même périphérique HID (même adresse ou même dossier
/// sysfs), sans dépendre des droits des nœuds evdev satellites. Le filtre de
/// modèle évite de prendre l'interface d'un satellite pour la manette.
fn find_hidraw(group: &str, vendor: u16, product: u16) -> Option<PathBuf> {
    for entry in std::fs::read_dir("/sys/class/hidraw").ok()?.flatten() {
        let device = entry.path().join("device");
        let Ok(canonical) = std::fs::canonicalize(&device) else {
            continue;
        };
        let Ok(uevent) = std::fs::read_to_string(device.join("uevent")) else {
            continue;
        };
        let uniq = uevent
            .lines()
            .find_map(|line| line.strip_prefix("HID_UNIQ="))
            .unwrap_or("");
        // evdev rend l'adresse en majuscules, hidraw en minuscules : la
        // comparaison doit ignorer la casse, sinon la manette Switch n'a
        // jamais son flux HID (ni IMU, ni cadence).
        let same = group == format!("sysfs:{}", canonical.display())
            || (!uniq.is_empty() && group.eq_ignore_ascii_case(&format!("uniq:{uniq}")));
        if !same {
            continue;
        }
        let mut parts = uevent
            .lines()
            .find_map(|line| line.strip_prefix("HID_ID="))
            .unwrap_or("")
            .split(':');
        let _bus = parts.next();
        let hid_vendor = parts
            .next()
            .and_then(|value| u32::from_str_radix(value, 16).ok());
        let hid_product = parts
            .next()
            .and_then(|value| u32::from_str_radix(value, 16).ok());
        if hid_vendor == Some(vendor as u32) && hid_product == Some(product as u32) {
            let name = entry.file_name();
            return Some(PathBuf::from("/dev").join(name));
        }
    }
    None
}

fn le_i16(bytes: &[u8]) -> i16 {
    i16::from_le_bytes([bytes[0], bytes[1]])
}

/// Décode un rapport DualSense. Le rapport Bluetooth 0x31 commence par deux
/// octets de transport, l'USB 0x01 par un seul ; les champs utiles ont ensuite
/// le même décalage.
fn decode_dualsense(data: &[u8]) -> Option<(MotionState, TouchpadState, bool)> {
    let base = battery::dualsense_base(data)?;
    if data.len() < base + 63 {
        return None;
    }
    let gyro = [
        le_i16(&data[base + 15..]) as f32 / 32768.0,
        le_i16(&data[base + 17..]) as f32 / 32768.0,
        le_i16(&data[base + 19..]) as f32 / 32768.0,
    ];
    let accel = [
        le_i16(&data[base + 21..]) as f32 / 32768.0,
        le_i16(&data[base + 23..]) as f32 / 32768.0,
        le_i16(&data[base + 25..]) as f32 / 32768.0,
    ];
    let mut points = Vec::new();
    for index in 0..2 {
        let point = &data[base + 32 + index * 4..base + 36 + index * 4];
        if point[0] & 0x80 != 0 {
            continue;
        }
        let x = (((point[2] & 0x0f) as u16) << 8 | point[1] as u16) as f32 / 1920.0;
        let y = ((point[3] as u16) << 4 | (point[2] >> 4) as u16) as f32 / 1080.0;
        points.push(TouchPoint {
            x: x.clamp(0.0, 1.0),
            y: y.clamp(0.0, 1.0),
        });
    }
    let muted = data[base + 10] & 0x04 != 0;
    Some((MotionState { gyro, accel }, TouchpadState { points }, muted))
}

/// Décode le rapport complet 0x30 d'une manette Switch. Il contient trois
/// échantillons IMU ; le dernier donne l'orientation du moment. Sans la
/// calibration SPI du pilote, on applique les échelles d'usine : 4096 LSB/g
/// et 14,247 LSB par °/s (soit 0,0702 °/s par LSB). PadState reste en ±1 =
/// ±4 g et ±2048 °/s.
fn decode_nintendo(data: &[u8]) -> Option<MotionState> {
    const IMU_START: usize = 13;
    const SAMPLE: usize = 12;
    /// Le capteur donne 14,247 unités par °/s : un geste de 90 °/s vaut
    /// environ 1282 en brut, pas 0,09.
    const RAW_PER_DPS: f32 = 14.247;
    if data.first() != Some(&0x30) || data.len() < IMU_START + SAMPLE * 3 {
        return None;
    }
    let raw = &data[IMU_START + SAMPLE * 2..IMU_START + SAMPLE * 3];
    let accel = [
        le_i16(&raw[0..]) as f32 / 4096.0 / 4.0,
        le_i16(&raw[2..]) as f32 / 4096.0 / 4.0,
        le_i16(&raw[4..]) as f32 / 4096.0 / 4.0,
    ];
    let gyro = [
        le_i16(&raw[6..]) as f32 / RAW_PER_DPS / 2048.0,
        le_i16(&raw[8..]) as f32 / RAW_PER_DPS / 2048.0,
        le_i16(&raw[10..]) as f32 / RAW_PER_DPS / 2048.0,
    ];
    Some(MotionState { gyro, accel })
}

fn publish(pads: &HashMap<PathBuf, Pad>, snapshot: &Mutex<Vec<PadState>>) {
    let mut list: Vec<PadState> = pads
        .iter()
        .map(|(path, pad)| {
            let mut pressed: Vec<String> = pad
                .pressed
                .iter()
                .filter_map(|code| button_label(*code))
                .collect();
            // Certains pilotes (xpad, notamment) exposent la croix sur les axes
            // HAT au lieu de BTN_DPAD_* : sans cette conversion, la croix
            // fonctionne dans le test mais n'apparaît jamais dans les boutons
            // pressés de la liste.
            for label in dpad_labels(&pad.axes) {
                if !pressed.iter().any(|known| known == label) {
                    pressed.push(label.to_string());
                }
            }
            pressed.sort_by_key(|label| (button_rank(label), label.clone()));
            let mut touchpad = None;
            let mut motion = None;
            for companion in &pad.companions {
                if !companion.alive {
                    continue;
                }
                match &companion.state {
                    CompanionState::Motion { gyro, accel } => {
                        motion = Some(MotionState {
                            gyro: *gyro,
                            accel: *accel,
                        });
                    }
                    CompanionState::Touchpad { slots, .. } => {
                        touchpad = Some(TouchpadState {
                            points: slots
                                .values()
                                .map(|(x, y)| TouchPoint { x: *x, y: *y })
                                .collect(),
                        });
                    }
                }
            }
            // Les rapports HID prennent le relais quand les nœuds evdev des
            // capteurs sont refusés par les ACL udev.
            if motion.is_none() {
                motion = pad.hidraw_motion;
            }
            if touchpad.is_none() {
                touchpad = pad.hidraw_touchpad.clone();
            }
            if touchpad.is_none() && pad.has_touchpad {
                touchpad = Some(TouchpadState {
                    points: pad
                        .touchpad_slots
                        .values()
                        .map(|(x, y)| TouchPoint { x: *x, y: *y })
                        .collect(),
                });
            }
            let mut battery = pad.battery_worker.as_ref().map(BatteryWorker::snapshot);
            if let Some(reading) = pad.hidraw_battery {
                battery
                    .get_or_insert_with(BatteryInfo::default)
                    .apply_report(reading);
            }
            PadState {
                path: path.clone(),
                name: pad.name.clone(),
                pressed,
                axes: pad.axes.clone(),
                face: face_buttons(&pad.pressed),
                touchpad,
                motion,
                microphone_muted: pad.hidraw_muted,
                battery,
                report: pad.report_cadence(),
                last_input_unix: pad
                    .last_input
                    .map(|at| unix_now_secs().saturating_sub(at.elapsed().as_secs())),
            }
        })
        .collect();
    list.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.path.cmp(&b.path)));
    if let Ok(mut guard) = snapshot.lock() {
        *guard = list;
    }
}

fn unix_now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

fn face_buttons(pressed: &HashSet<u16>) -> FaceButtons {
    FaceButtons {
        south: pressed.contains(&BTN_SOUTH),
        east: pressed.contains(&BTN_EAST),
        north: pressed.contains(&BTN_NORTH),
        west: pressed.contains(&BTN_WEST),
    }
}

/// Parent HID commun aux nœuds d'une même manette. L'adresse Bluetooth est
/// stable entre la manette, ses capteurs et son pavé ; en USB, le dossier
/// sysfs du périphérique HID les réunit.
fn group_key(path: &Path, device: &evdev::Device) -> String {
    if let Some(uniq) = device.unique_name() {
        let uniq = uniq.trim();
        if is_bluetooth_address(uniq) {
            return format!("uniq:{uniq}");
        }
    }
    if let Some(dir) = sysfs_input_dir(path) {
        if let Some(parent) = dir.parent() {
            return format!("sysfs:{}", parent.display());
        }
    }
    let id = device.input_id();
    format!(
        "id:{:04x}:{:04x}:{}",
        id.vendor(),
        id.product(),
        device.name().unwrap_or("")
    )
}

fn sysfs_input_dir(event: &Path) -> Option<PathBuf> {
    let name = event.file_name()?.to_str()?;
    if !name.starts_with("event") {
        return None;
    }
    std::fs::canonicalize(format!("/sys/class/input/{name}/device")).ok()
}

fn is_bluetooth_address(value: &str) -> bool {
    value.split(':').count() == 6
        && value != "00:00:00:00:00:00"
        && value
            .split(':')
            .all(|part| part.len() == 2 && part.chars().all(|c| c.is_ascii_hexdigit()))
}

fn companion_kind(name: &str) -> Option<CompanionKind> {
    let name = name.trim_end().to_lowercase();
    if name.contains("motion") || name.contains("gyro") || name.contains("accelerometer") {
        Some(CompanionKind::Motion)
    } else if name.contains("touchpad") || name.contains("trackpad") || name.contains("touch pad") {
        Some(CompanionKind::Touchpad)
    } else {
        None
    }
}

/// Gâchettes analogiques des manettes Valve : `hid-steam` les porte sur la
/// croix 2 (`Y` à gauche, `X` à droite) au lieu de `ABS_Z`/`ABS_RZ`. Le reste
/// du code lit `left_trigger`/`right_trigger` comme sur les autres manettes.
fn steam_axis_key(vendor: u16, code: u16) -> Option<&'static str> {
    if vendor != 0x28de {
        return None;
    }
    match code {
        ABS_HAT0X => Some("left_pad_x"),
        ABS_HAT0Y => Some("left_pad_y"),
        ABS_HAT1X => Some("right_pad_x"),
        ABS_HAT1Y => Some("right_pad_y"),
        ABS_HAT2Y => Some("left_trigger"),
        ABS_HAT2X => Some("right_trigger"),
        _ => None,
    }
}

fn pad_axis_key(vendor: u16, code: u16) -> Option<&'static str> {
    steam_axis_key(vendor, code).or_else(|| axis_key(code))
}

fn axis_key(code: u16) -> Option<&'static str> {
    match code {
        ABS_X => Some("left_x"),
        ABS_Y => Some("left_y"),
        ABS_RX => Some("right_x"),
        ABS_RY => Some("right_y"),
        ABS_Z | ABS_GAS => Some("left_trigger"),
        ABS_RZ | ABS_BRAKE => Some("right_trigger"),
        ABS_HAT0X => Some("dpad_x"),
        ABS_HAT0Y => Some("dpad_y"),
        _ => None,
    }
}

fn default_range(code: u16) -> (i32, i32) {
    match code {
        ABS_X | ABS_Y | ABS_RX | ABS_RY | ABS_HAT0X | ABS_HAT0Y | ABS_HAT1X | ABS_HAT1Y => {
            (-32768, 32767)
        }
        _ => (0, 255),
    }
}

fn companion_range(kind: CompanionKind, code: u16) -> (i32, i32) {
    match kind {
        CompanionKind::Motion => match code {
            ABS_X | ABS_Y | ABS_Z | ABS_RX | ABS_RY | ABS_RZ => (-32768, 32767),
            _ => (0, 255),
        },
        CompanionKind::Touchpad => match code {
            ABS_MT_POSITION_X | ABS_X => (0, 1919),
            ABS_MT_POSITION_Y | ABS_Y => (0, 1079),
            _ => (0, 255),
        },
    }
}

/// Ramène une valeur brute dans -1..1 pour les sticks et la croix, 0..1 pour
/// les gâchettes. Certaines manettes Bluetooth déclarent des sticks sur
/// 0..255 : il faut alors recentrer sur le milieu de la plage, pas sur zéro.
fn normalize_axis(code: u16, value: i32, min: i32, max: i32) -> f32 {
    match code {
        ABS_X | ABS_Y | ABS_RX | ABS_RY => {
            let center = (min as f32 + max as f32) / 2.0;
            let half = ((max - min) as f32 / 2.0).max(1.0);
            ((value as f32 - center) / half).clamp(-1.0, 1.0)
        }
        ABS_HAT0X | ABS_HAT0Y | ABS_HAT1X | ABS_HAT1Y => {
            // Les HAT d'une croix valent -1..1, mais les pavés Steam 2026
            // utilisent toute la plage signée -32767..32767. Normaliser avec
            // les bornes réelles évite de transformer tout mouvement en ±1.
            let center = (min as f32 + max as f32) / 2.0;
            let half = ((max - min) as f32 / 2.0).max(1.0);
            ((value as f32 - center) / half).clamp(-1.0, 1.0)
        }
        _ => {
            let span = (max - min).max(1) as f32;
            ((value - min) as f32 / span).clamp(0.0, 1.0)
        }
    }
}

/// Met à jour les contacts d'un pavé tactile, qu'il soit porté par le nœud
/// principal ou par un nœud satellite. Les deux chemins doivent se comporter
/// exactement pareil.
fn touch_event(
    slot: &mut i32,
    slots: &mut BTreeMap<i32, (f32, f32)>,
    code: u16,
    value: i32,
    min: i32,
    max: i32,
) -> bool {
    let current = *slot;
    match code {
        ABS_MT_SLOT => *slot = value.max(0),
        ABS_MT_TRACKING_ID => {
            if value < 0 {
                slots.remove(&current);
            } else {
                slots.entry(current).or_insert((0.5, 0.5));
            }
        }
        ABS_MT_POSITION_X | ABS_X => {
            slots.entry(current).or_insert((0.5, 0.5)).0 = touch_norm(value, min, max);
        }
        ABS_MT_POSITION_Y | ABS_Y => {
            slots.entry(current).or_insert((0.5, 0.5)).1 = touch_norm(value, min, max);
        }
        _ => return false,
    }
    true
}

fn motion_norm(value: i32, min: i32, max: i32) -> f32 {
    if max <= min {
        return 0.0;
    }
    let center = (min as f32 + max as f32) / 2.0;
    let half = ((max - min) as f32 / 2.0).max(1.0);
    ((value as f32 - center) / half).clamp(-1.0, 1.0)
}

fn motion_index(code: u16) -> Option<usize> {
    match code {
        ABS_X | ABS_RX => Some(0),
        ABS_Y | ABS_RY => Some(1),
        ABS_Z | ABS_RZ => Some(2),
        _ => None,
    }
}

fn touch_norm(value: i32, min: i32, max: i32) -> f32 {
    if max <= min {
        return 0.5;
    }
    ((value - min) as f32 / (max - min) as f32).clamp(0.0, 1.0)
}

fn button_label(code: u16) -> Option<String> {
    if !codes::is_pad_button(code) {
        return None;
    }
    let label = codes::button_label(code);
    (label != "?").then_some(label)
}

/// Directions de la croix lues sur les axes HAT (valeurs normalisées −1/0/1).
/// Renvoie les libellés dans l'ordre Haut, Bas, Gauche, Droite.
fn dpad_labels(axes: &BTreeMap<String, f32>) -> Vec<&'static str> {
    let mut labels = Vec::with_capacity(4);
    if axes.get("dpad_y").copied().unwrap_or(0.0) < -0.5 {
        labels.push("Haut");
    }
    if axes.get("dpad_y").copied().unwrap_or(0.0) > 0.5 {
        labels.push("Bas");
    }
    if axes.get("dpad_x").copied().unwrap_or(0.0) < -0.5 {
        labels.push("Gauche");
    }
    if axes.get("dpad_x").copied().unwrap_or(0.0) > 0.5 {
        labels.push("Droite");
    }
    labels
}

fn button_rank(label: &str) -> usize {
    match label {
        "Haut" => 0,
        "Bas" => 1,
        "Gauche" => 2,
        "Droite" => 3,
        "A" => 4,
        "B" => 5,
        "X" => 6,
        "Y" => 7,
        "LB" => 8,
        "RB" => 9,
        "LT" => 10,
        "RT" => 11,
        "L3" => 12,
        "R3" => 13,
        "Capture" => 14,
        "Select" => 15,
        "Start" => 16,
        "Guide" => 17,
        _ => 18,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sticks_and_triggers_are_normalised() {
        assert!((normalize_axis(ABS_X, 0, -32768, 32767)).abs() < 0.001);
        assert!((normalize_axis(ABS_X, 32767, -32768, 32767) - 1.0).abs() < 0.001);
        assert!((normalize_axis(ABS_X, -32768, -32768, 32767) + 1.0).abs() < 0.001);
        // Sticks Bluetooth sur 0..255 : le repos est au milieu de la plage.
        assert!((normalize_axis(ABS_X, 127, 0, 255)).abs() < 0.01);
        assert!((normalize_axis(ABS_X, 255, 0, 255) - 1.0).abs() < 0.01);
        assert!((normalize_axis(ABS_Z, 200, 0, 255) - 0.784).abs() < 0.01);
        assert_eq!(normalize_axis(ABS_Z, 0, 0, 0), 0.0);
    }

    #[test]
    fn steam_triggers_are_read_from_the_hat_two_axis() {
        // Les manettes Valve exposent leurs gâchettes sur HAT2, contrairement
        // aux autres marques : le test en direct doit les voir pareil.
        assert_eq!(pad_axis_key(0x28de, ABS_HAT0X), Some("left_pad_x"));
        assert_eq!(pad_axis_key(0x28de, ABS_HAT0Y), Some("left_pad_y"));
        assert_eq!(pad_axis_key(0x28de, ABS_HAT1X), Some("right_pad_x"));
        assert_eq!(pad_axis_key(0x28de, ABS_HAT1Y), Some("right_pad_y"));
        assert_eq!(pad_axis_key(0x28de, ABS_HAT2Y), Some("left_trigger"));
        assert_eq!(pad_axis_key(0x28de, ABS_HAT2X), Some("right_trigger"));
        // Les HAT0 d'une manette non Valve restent la croix directionnelle.
        assert_eq!(pad_axis_key(0x054c, ABS_HAT0X), Some("dpad_x"));
        assert_eq!(pad_axis_key(0x054c, ABS_HAT2Y), None);
        assert!((normalize_axis(ABS_HAT1X, 32767, -32768, 32767) - 1.0).abs() < 0.001);
        assert!((normalize_axis(ABS_HAT1Y, -32768, -32768, 32767) + 1.0).abs() < 0.001);
        // Position intermédiaire : le centre doit rester au centre, et un quart
        // de course doit donner un quart de sortie, pas un plein régime.
        assert!(normalize_axis(ABS_HAT1X, 0, -32767, 32767).abs() < 0.001);
        assert!((normalize_axis(ABS_HAT1X, 16384, -32767, 32767) - 0.5).abs() < 0.001);
        assert!((normalize_axis(ABS_HAT1Y, -16384, -32767, 32767) + 0.5).abs() < 0.001);
        // Une vraie croix -1..1 garde son comportement d'origine.
        assert!(normalize_axis(ABS_HAT0X, 0, -1, 1).abs() < 0.001);
        // Les manettes Sony gardent ABS_Z/ABS_RZ.
        assert_eq!(pad_axis_key(0x28de, ABS_RZ), Some("right_trigger"));
        assert_eq!(pad_axis_key(0x054c, ABS_Z), Some("left_trigger"));
        assert!((normalize_axis(ABS_HAT2Y, 32767, 0, 32767) - 1.0).abs() < 0.001);
        assert_eq!(normalize_axis(ABS_HAT2Y, 0, 0, 32767), 0.0);
    }

    #[test]
    fn dpad_and_face_buttons_are_named() {
        assert_eq!(button_label(BTN_DPAD_UP).as_deref(), Some("Haut"));
        assert_eq!(button_label(BTN_SOUTH).as_deref(), Some("A"));
        assert_eq!(button_label(BTN_TL2).as_deref(), Some("LT"));
        assert_eq!(button_label(KEY_A), None);
    }

    #[test]
    fn paddle_and_extra_buttons_are_tracked() {
        assert_eq!(button_label(BTN_GRIPL).as_deref(), Some("Palette L"));
        assert_eq!(button_label(0x2e0).as_deref(), Some("Bouton 0x2e0"));
        // Les boutons de pavé tactile et de souris ne polluent pas la liste.
        assert_eq!(button_label(BTN_TOUCH), None);
        assert_eq!(button_label(0x110), None);
    }

    #[test]
    fn hat_axes_become_dpad_labels() {
        let mut axes = BTreeMap::new();
        axes.insert("dpad_x".to_string(), -1.0);
        axes.insert("dpad_y".to_string(), 1.0);
        assert_eq!(dpad_labels(&axes), vec!["Bas", "Gauche"]);
        axes.insert("dpad_x".to_string(), 0.0);
        axes.insert("dpad_y".to_string(), 0.0);
        assert!(dpad_labels(&axes).is_empty());
    }

    #[test]
    fn face_positions_are_independent_of_brand_labels() {
        let mut pressed = HashSet::new();
        pressed.insert(BTN_EAST);
        pressed.insert(BTN_NORTH);
        assert_eq!(
            face_buttons(&pressed),
            FaceButtons {
                east: true,
                north: true,
                ..FaceButtons::default()
            }
        );
    }

    #[test]
    fn sensor_nodes_are_recognised() {
        assert_eq!(
            companion_kind("DualSense Wireless Controller Motion Sensors"),
            Some(CompanionKind::Motion)
        );
        assert_eq!(
            companion_kind("DualSense Wireless Controller Touchpad"),
            Some(CompanionKind::Touchpad)
        );
        assert_eq!(companion_kind("DualSense Wireless Controller"), None);
        assert_eq!(
            companion_kind("Wireless Controller Touchpad "),
            Some(CompanionKind::Touchpad)
        );
    }

    #[test]
    fn touch_coordinates_are_normalised_and_clamped() {
        assert!((touch_norm(0, 0, 1919)).abs() < 0.001);
        assert!((touch_norm(1919, 0, 1919) - 1.0).abs() < 0.001);
        assert!((touch_norm(959, 0, 1919) - 0.5).abs() < 0.01);
        assert_eq!(touch_norm(500, 1920, 0), 0.5);
    }

    #[test]
    fn touch_events_track_slots_and_release() {
        let mut slot = 0;
        let mut slots = BTreeMap::new();
        assert!(touch_event(&mut slot, &mut slots, ABS_MT_SLOT, 1, 0, 9));
        assert!(touch_event(
            &mut slot,
            &mut slots,
            ABS_MT_TRACKING_ID,
            7,
            0,
            0
        ));
        assert!(touch_event(
            &mut slot,
            &mut slots,
            ABS_MT_POSITION_X,
            960,
            0,
            1919
        ));
        assert!(touch_event(
            &mut slot,
            &mut slots,
            ABS_MT_POSITION_Y,
            540,
            0,
            1079
        ));
        let point = slots.get(&1).copied().unwrap();
        assert!((point.0 - 0.5).abs() < 0.01);
        assert!((point.1 - 0.5).abs() < 0.01);
        assert!(touch_event(
            &mut slot,
            &mut slots,
            ABS_MT_TRACKING_ID,
            -1,
            0,
            0
        ));
        assert!(slots.is_empty());
    }

    #[test]
    fn motion_values_are_centred_without_a_deadzone() {
        assert!((motion_norm(0, -32768, 32767)).abs() < 0.001);
        assert!((motion_norm(32767, -32768, 32767) - 1.0).abs() < 0.001);
        // 0,01 normalisé = 20 °/s : un petit geste doit rester visible.
        assert!((motion_norm(328, -32768, 32767) - 0.01).abs() < 0.001);
    }

    fn dualsense_packet(bluetooth: bool) -> Vec<u8> {
        let base = if bluetooth { 2 } else { 1 };
        let mut data = vec![0u8; if bluetooth { 78 } else { 64 }];
        data[0] = if bluetooth { 0x31 } else { 0x01 };
        data[base + 15..base + 17].copy_from_slice(&4096i16.to_le_bytes());
        data[base + 17..base + 19].copy_from_slice(&(-2048i16).to_le_bytes());
        data[base + 19..base + 21].copy_from_slice(&32767i16.to_le_bytes());
        data[base + 21..base + 23].copy_from_slice(&8192i16.to_le_bytes());
        data[base + 23..base + 25].copy_from_slice(&0i16.to_le_bytes());
        data[base + 25..base + 27].copy_from_slice(&(-8192i16).to_le_bytes());
        // Un doigt en (1024, 18), le second contact inactif.
        data[base + 32..base + 36].copy_from_slice(&[0x01, 0x00, 0x24, 0x01]);
        data[base + 36..base + 40].copy_from_slice(&[0x80, 0x00, 0x00, 0x00]);
        data
    }

    #[test]
    fn dualsense_reports_expose_motion_touch_and_mute() {
        for bluetooth in [false, true] {
            let base = if bluetooth { 2 } else { 1 };
            let mut packet = dualsense_packet(bluetooth);
            let (motion, touchpad, muted) = decode_dualsense(&packet).unwrap();
            assert!((motion.gyro[0] - 0.125).abs() < 0.001);
            assert!((motion.gyro[1] + 0.0625).abs() < 0.001);
            assert!((motion.accel[0] - 0.25).abs() < 0.001);
            assert!((motion.accel[2] + 0.25).abs() < 0.001);
            assert_eq!(touchpad.points.len(), 1);
            assert!((touchpad.points[0].x - 0.533).abs() < 0.01);
            assert!((touchpad.points[0].y - 0.0167).abs() < 0.001);
            assert!(!muted);
            packet[base + 10] |= 0x04;
            assert!(decode_dualsense(&packet).unwrap().2);
        }
    }

    #[test]
    fn nintendo_full_reports_expose_motion() {
        let mut packet = vec![0u8; 49];
        packet[0] = 0x30;
        let base = 13 + 24;
        packet[base..base + 2].copy_from_slice(&4096i16.to_le_bytes());
        packet[base + 2..base + 4].copy_from_slice(&(-2048i16).to_le_bytes());
        packet[base + 4..base + 6].copy_from_slice(&8192i16.to_le_bytes());
        packet[base + 6..base + 8].copy_from_slice(&14247i16.to_le_bytes());
        packet[base + 8..base + 10].copy_from_slice(&(-7123i16).to_le_bytes());
        packet[base + 10..base + 12].copy_from_slice(&0i16.to_le_bytes());
        let motion = decode_nintendo(&packet).unwrap();
        assert!((motion.accel[0] - 0.25).abs() < 0.001);
        assert!((motion.accel[1] + 0.125).abs() < 0.001);
        assert!((motion.accel[2] - 0.5).abs() < 0.001);
        // 14 247 brut = 1 000 °/s, et 7 123 brut = 500 °/s.
        assert!((motion.gyro[0] - 1000.0 / 2048.0).abs() < 1e-4);
        assert!((motion.gyro[1] + 500.0 / 2048.0).abs() < 1e-3);
        assert_eq!(motion.gyro[2], 0.0);
        // Un rapport d'une autre manette ou tronqué est ignoré.
        assert!(decode_nintendo(&[0x31; 49]).is_none());
        assert!(decode_nintendo(&packet[..48]).is_none());
    }

    #[test]
    fn malformed_hid_reports_are_ignored() {
        assert!(decode_dualsense(&[]).is_none());
        assert!(decode_dualsense(&[0x31; 10]).is_none());
        assert!(decode_dualsense(&[0x02; 78]).is_none());
    }

    #[test]
    fn bluetooth_addresses_are_recognised_for_grouping() {
        assert!(is_bluetooth_address("02:00:00:00:00:01"));
        assert!(!is_bluetooth_address("FXB9954802F92"));
        assert!(!is_bluetooth_address("000000000000"));
        assert!(!is_bluetooth_address(""));
    }
}
