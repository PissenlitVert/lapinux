//! Commandes de test envoyées à la manette.
//!
//! Le retour de force passe par evdev, comme les jeux. La barre lumineuse et
//! les gâchettes adaptatives n'ont pas d'API evdev : on écrit directement un
//! rapport de sortie DualSense sur son hidraw, avec le CRC attendu en
//! Bluetooth.

use crate::{Error, Result};
use evdev::{FFEffectData, FFEffectKind, FFReplay, FFTrigger};
use lapinux_core::model::{DeviceInfo, DeviceLink};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};
use std::time::{Duration, Instant};

/// Numéro de séquence Bluetooth, dans le quartet haut de l'octet `seq_tag`.
static BLUETOOTH_SEQ: AtomicU8 = AtomicU8::new(0);
/// Une nouvelle demande annule la remise à zéro de la précédente.
static TRIGGER_GENERATION: AtomicU32 = AtomicU32::new(0);
/// Un nouveau test haptique remplace le précédent au lieu de s'y superposer.
static HAPTIC_GENERATION: AtomicU32 = AtomicU32::new(0);

/// Effet proposé dans la carte « Gâchettes adaptatives ».
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerTest {
    /// Résistance progressive à partir du deuxième tiers de la course.
    Resistance,
    /// Résistance maximale dès le début de la course : la gâchette est bloquée.
    Block,
    /// Gâchette qui vibre à partir du deuxième tiers.
    Vibration,
    /// Résistance sur une plage, comme une détente d'arme.
    Weapon,
}

impl TriggerTest {
    pub fn label(self) -> &'static str {
        match self {
            TriggerTest::Resistance => "Résistance",
            TriggerTest::Block => "Bloquer",
            TriggerTest::Vibration => "Vibration",
            TriggerTest::Weapon => "Gâchette",
        }
    }

    /// Explication courte affichée au survol du bouton.
    pub fn hint(self) -> &'static str {
        match self {
            TriggerTest::Resistance => {
                "Résistance progressive à partir du deuxième tiers de la course."
            }
            TriggerTest::Block => "Gâchette bloquée dès le début de la course.",
            TriggerTest::Vibration => "Vibration dans le deuxième tiers de la course.",
            TriggerTest::Weapon => "Résistance sur une plage, comme une détente d'arme.",
        }
    }
}

/// Motif du test haptique. Le pilote Linux n'expose que le rumble des deux
/// moteurs (fort à gauche, faible à droite) : les motifs sont donc joués en
/// séquences de impulsions réelles sur chaque moteur, pas en ondes simulées
/// que le noyau aplatirait en une seule vibration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HapticPattern {
    /// Les deux moteurs en continu pendant toute la durée.
    Steady,
    /// Impulsions franches au rythme demandé.
    Pulse,
    /// Montée progressive du plus doux au plus fort.
    Ramp,
    /// Deux battements rapprochés, puis une pause, comme un cœur.
    Heartbeat,
}

impl HapticPattern {
    pub fn label(self) -> &'static str {
        match self {
            HapticPattern::Steady => "Continu",
            HapticPattern::Pulse => "Impulsions",
            HapticPattern::Ramp => "Rampe",
            HapticPattern::Heartbeat => "Battement",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            HapticPattern::Steady => "Les deux moteurs vibrent sans interruption.",
            HapticPattern::Pulse => "Une suite d'impulsions nettes, à la cadence choisie.",
            HapticPattern::Ramp => "La force monte doucement du plus faible au maximum.",
            HapticPattern::Heartbeat => "Deux battements rapprochés, puis une pause.",
        }
    }

    /// Seuls les motifs rythmés utilisent la cadence.
    pub fn uses_rate(self) -> bool {
        matches!(self, HapticPattern::Pulse | HapticPattern::Heartbeat)
    }
}

/// Joue un motif haptique précis : moteur gauche et droit réglés séparément,
/// cadence et durée contrôlées. Le noyau attend deux magnitudes de rumble ;
/// les impulsions sont donc envoyées une par une, ce qui laisse entendre la
/// différence entre un continu, une suite de clics, une rampe et un battement.
pub fn haptic(
    device: &DeviceInfo,
    pattern: HapticPattern,
    left: f32,
    right: f32,
    rate_hz: f32,
    duration_ms: u16,
) -> Result<()> {
    if !device.connected || device.path.as_os_str().is_empty() {
        return Err(Error::Other("manette hors ligne".into()));
    }
    let left = left.clamp(0.0, 1.0);
    let right = right.clamp(0.0, 1.0);
    if left <= 0.01 && right <= 0.01 {
        return Ok(());
    }
    let rate_hz = rate_hz.clamp(0.8, 20.0);
    let total = Duration::from_millis(duration_ms.clamp(100, 5000) as u64);
    let generation = HAPTIC_GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
    let mut pad = evdev::Device::open(&device.path)?;
    let _ = pad.set_ff_gain(u16::MAX);
    // Vérifie tout de suite que la manette accepte un effet : une erreur est
    // ainsi renvoyée à l'interface au lieu de disparaître dans le fil.
    let probe = pad.upload_ff_effect(FFEffectData {
        direction: 0,
        trigger: FFTrigger::default(),
        replay: FFReplay { length: 1, delay: 0 },
        kind: FFEffectKind::Rumble {
            strong_magnitude: 0,
            weak_magnitude: 0,
        },
    })?;
    drop(probe);

    std::thread::spawn(move || {
        let started = Instant::now();
        let superseded = || HAPTIC_GENERATION.load(Ordering::Relaxed) != generation;
        let left = (left * u16::MAX as f32).round() as u16;
        let right = (right * u16::MAX as f32).round() as u16;
        match pattern {
            HapticPattern::Steady => {
                let _ = pulse(&mut pad, left, right, total);
            }
            HapticPattern::Ramp => {
                let steps = 12u64;
                let step = total / steps as u32;
                for index in 0..steps {
                    if started.elapsed() >= total || superseded() {
                        break;
                    }
                    let strength = 0.12 + 0.88 * (index + 1) as f32 / steps as f32;
                    let _ = pulse(
                        &mut pad,
                        (left as f32 * strength) as u16,
                        (right as f32 * strength) as u16,
                        step,
                    );
                }
            }
            HapticPattern::Pulse => {
                let period = Duration::from_secs_f32(1.0 / rate_hz);
                let on = period.mul_f32(0.38).max(Duration::from_millis(8));
                let off = period.saturating_sub(on);
                while started.elapsed() < total && !superseded() {
                    let _ = pulse(&mut pad, left, right, on);
                    if started.elapsed() >= total || superseded() {
                        break;
                    }
                    std::thread::sleep(off.min(total.saturating_sub(started.elapsed())));
                }
            }
            HapticPattern::Heartbeat => {
                // Battement fort, second plus doux, puis repos : la pause est
                // calée sur la cadence pour rester régulière. Au-delà de 4 Hz,
                // les deux battements ne tiendraient plus dans un cycle.
                let rate_hz = rate_hz.min(4.0);
                let period = Duration::from_secs_f32(1.0 / rate_hz);
                let beat = Duration::from_millis(90);
                let echo = Duration::from_millis(70);
                let gap = Duration::from_millis(110);
                let rest = period.saturating_sub(beat + echo + gap);
                while started.elapsed() < total && !superseded() {
                    let _ = pulse(&mut pad, left, right, beat);
                    std::thread::sleep(gap.min(total.saturating_sub(started.elapsed())));
                    if started.elapsed() >= total || superseded() {
                        break;
                    }
                    let _ = pulse(
                        &mut pad,
                        (left as f32 * 0.55) as u16,
                        (right as f32 * 0.55) as u16,
                        echo,
                    );
                    if started.elapsed() >= total || superseded() {
                        break;
                    }
                    std::thread::sleep(rest.min(total.saturating_sub(started.elapsed())));
                }
            }
        }
        // Coupe nette : un dernier effet de longueur nulle efface tout effet
        // encore actif si la durée a été dépassée pendant un sommeil.
        if let Ok(mut effect) = pad.upload_ff_effect(FFEffectData {
            direction: 0,
            trigger: FFTrigger::default(),
            replay: FFReplay { length: 0, delay: 0 },
            kind: FFEffectKind::Rumble {
                strong_magnitude: 0,
                weak_magnitude: 0,
            },
        }) {
            let _ = effect.play(1);
        }
    });
    Ok(())
}

/// Test HD : un vrai son de bobine, envoyé par le flux audio Bluetooth de la
/// DualSense (rapport 0x32, 32 échantillons stéréo 8 bits à 3 kHz). C'est le
/// chemin que les jeux utilisent pour les retours haptiques, pas le rumble
/// des deux moteurs. En USB, les mêmes bobines passent par les canaux arrière
/// de la carte son quatre canaux : l'interface emprunte alors `pad_audio`.
pub fn haptic_hd(
    device: &DeviceInfo,
    left: f32,
    right: f32,
    frequency_hz: f32,
    duration_ms: u16,
) -> Result<()> {
    if !is_dualsense(device) {
        return Err(Error::Other(
            "les retours HD sont réservés à la DualSense".into(),
        ));
    }
    if !matches!(device.connection, DeviceLink::Bluetooth) {
        return Err(Error::Other(
            "ce chemin HD demande le Bluetooth ; en USB, la carte son de la manette le remplace".into(),
        ));
    }
    let left = left.clamp(0.0, 1.0);
    let right = right.clamp(0.0, 1.0);
    if left <= 0.001 && right <= 0.001 {
        return Ok(());
    }
    let frequency_hz = frequency_hz.clamp(20.0, 400.0);
    let total_ms = duration_ms.clamp(100, 5000) as u64;
    let generation = HAPTIC_GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
    let path = dualsense_hidraw(device)?;
    let mut file = OpenOptions::new().write(true).open(path)?;
    // Pleine puissance des bobines, filtre passe-bas actif : sans ce rapport,
    // le contrôleur garde la réduction de puissance laissée par le dernier
    // propriétaire du lien (12,5 % du niveau dans le pire cas).
    file.write_all(&haptics_power_report())?;

    std::thread::spawn(move || {
        const RATE: u32 = 3_000;
        const FRAMES: u32 = 32;
        const FADE: u32 = 48;
        let total = (total_ms * RATE as u64 / 1000) as u32;
        let mut seq = 0u8;
        let mut counter = 0u8;
        let mut phase = 0f32;
        let step = std::f32::consts::TAU * frequency_hz / RATE as f32;
        let mut next = Instant::now();
        let mut sent = 0u32;
        while sent < total {
            if HAPTIC_GENERATION.load(Ordering::Relaxed) != generation {
                return;
            }
            let mut report = [0u8; 142];
            report[0] = 0x32;
            report[1] = (seq & 0x0f) << 4;
            report[2] = 0x91;
            report[3] = 0x07;
            report[4] = 0xfe;
            report[5..10].fill(16);
            report[10] = counter;
            report[11] = 0x92;
            report[12] = 0x40;
            for frame in 0..FRAMES {
                let index = sent + frame;
                let attack = (index as f32 / FADE as f32).min(1.0);
                let release = (total.saturating_sub(index) as f32 / FADE as f32).min(1.0);
                let gain = attack.min(release).clamp(0.0, 1.0);
                let wave = phase.sin() * gain;
                phase += step;
                let sample_left = (wave * left * 120.0) as i8;
                let sample_right = (wave * right * 120.0) as i8;
                report[13 + frame as usize * 2] = sample_left as u8;
                report[14 + frame as usize * 2] = sample_right as u8;
            }
            haptics_sign(&mut report);
            if file.write_all(&report).is_err() {
                return;
            }
            seq = (seq + 1) & 0x0f;
            counter = counter.wrapping_add(1);
            sent = sent.saturating_add(FRAMES);
            next += Duration::from_micros(10_672);
            if let Some(wait) = next.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
        }
        // Deux blocs de silence : le contrôleur vide sa file sans clic.
        for _ in 0..2 {
            if HAPTIC_GENERATION.load(Ordering::Relaxed) != generation {
                return;
            }
            let mut report = [0u8; 142];
            report[0] = 0x32;
            report[1] = (seq & 0x0f) << 4;
            report[2] = 0x91;
            report[3] = 0x07;
            report[4] = 0xfe;
            report[5..10].fill(16);
            report[10] = counter;
            report[11] = 0x92;
            report[12] = 0x40;
            haptics_sign(&mut report);
            let _ = file.write_all(&report);
            seq = (seq + 1) & 0x0f;
            counter = counter.wrapping_add(1);
            std::thread::sleep(Duration::from_micros(10_672));
        }
    });
    Ok(())
}

/// Rapport 0x31 qui rend toute la puissance aux bobines et active le filtre
/// passe-bas des retours HD, sans toucher au reste de l'état.
fn haptics_power_report() -> [u8; 78] {
    let mut report = [0u8; 78];
    report[0] = 0x31;
    report[2] = 0x10;
    report[3] = 0x00; // valid_flag0 : aucun bit de rumble, la bobine reste en audio
    report[4] = 0x02 | 0x20 | 0x40;
    report[12] = 0x00;
    report[39] = 0x00;
    report[42] = 0x01; // filtre passe-bas actif
    let crc = bluetooth_crc(&report[..74]);
    report[74..78].copy_from_slice(&crc.to_le_bytes());
    report
}

/// CRC d'un bloc 0x32 : 0xA2 puis les 138 premiers octets.
fn haptics_sign(report: &mut [u8; 142]) {
    let crc = crc32_le(0xffff_ffff, &[0xa2]);
    let crc = !crc32_le(crc, &report[..138]);
    report[138..142].copy_from_slice(&crc.to_le_bytes());
}

/// Une impulsion de rumble sur les deux moteurs, bloquante pour le fil de
/// test seulement.
fn pulse(
    pad: &mut evdev::Device,
    strong: u16,
    weak: u16,
    length: Duration,
) -> Result<()> {
    if length.is_zero() {
        return Ok(());
    }
    let mut effect = pad.upload_ff_effect(FFEffectData {
        direction: 0,
        trigger: FFTrigger::default(),
        replay: FFReplay {
            length: length.as_millis().min(u16::MAX as u128) as u16,
            delay: 0,
        },
        kind: FFEffectKind::Rumble {
            strong_magnitude: strong,
            weak_magnitude: weak,
        },
    })?;
    effect.play(1)?;
    std::thread::sleep(length);
    let _ = effect.stop();
    Ok(())
}

/// Fait vibrer la manette pendant environ une demi-seconde.
///
/// L'effet est envoyé tout de suite, puis relâché par un fil séparé : l'appel
/// ne bloque jamais l'interface.
pub fn rumble(device: &DeviceInfo, intensity: f32) -> Result<()> {
    if !device.connected || device.path.as_os_str().is_empty() {
        return Err(Error::Other("manette hors ligne".into()));
    }
    let intensity = intensity.clamp(0.0, 1.0);
    if intensity <= 0.01 {
        return Ok(());
    }
    let mut pad = evdev::Device::open(&device.path)?;
    // Le gain maître n'est pas garanti à 100 % par le pilote : sans ce réglage,
    // un effet correctement téléversé peut rester muet sur certaines manettes.
    let _ = pad.set_ff_gain(u16::MAX);
    let magnitude = (intensity * u16::MAX as f32).round() as u16;
    let effect = pad.upload_ff_effect(FFEffectData {
        direction: 0,
        trigger: FFTrigger::default(),
        replay: FFReplay {
            length: 600,
            delay: 0,
        },
        kind: FFEffectKind::Rumble {
            strong_magnitude: magnitude,
            weak_magnitude: (magnitude as f32 * 0.7).round() as u16,
        },
    })?;
    let mut effect = effect;
    effect.play(1)?;
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(650));
        let _ = effect.stop();
    });
    Ok(())
}

/// Applique le même effet aux deux gâchettes pendant `seconds` secondes.
///
/// La remise à zéro est faite par un fil séparé, même si l'interface n'est plus
/// affichée. Une nouvelle demande annule la remise à zéro de la précédente.
pub fn dualsense_trigger_test(
    device: &DeviceInfo,
    test: TriggerTest,
    seconds: u8,
) -> Result<()> {
    if !is_dualsense(device) {
        return Err(Error::Other(
            "les gâchettes adaptatives sont réservées à la DualSense".into(),
        ));
    }
    let path = dualsense_hidraw(device)?;
    let bluetooth = matches!(device.connection, DeviceLink::Bluetooth);
    let (right, left) = trigger_effect(test);
    let generation = TRIGGER_GENERATION.fetch_add(1, Ordering::Relaxed) + 1;

    let mut file = OpenOptions::new().write(true).open(&path)?;
    file.write_all(&output_report(
        bluetooth,
        next_bluetooth_seq(),
        &right,
        &left,
    ))?;

    let seconds = seconds.clamp(1, 10) as u64;
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(seconds));
        // Une demande plus récente gère elle-même sa propre remise à zéro.
        if TRIGGER_GENERATION.load(Ordering::Relaxed) != generation {
            return;
        }
        if let Ok(mut file) = OpenOptions::new().write(true).open(&path) {
            let _ = file.write_all(&off_report(bluetooth));
        }
    });
    Ok(())
}

/// Relâche immédiatement les deux gâchettes.
pub fn dualsense_trigger_off(device: &DeviceInfo) -> Result<()> {
    if !is_dualsense(device) {
        return Err(Error::Other(
            "les gâchettes adaptatives sont réservées à la DualSense".into(),
        ));
    }
    TRIGGER_GENERATION.fetch_add(1, Ordering::Relaxed);
    let path = dualsense_hidraw(device)?;
    let bluetooth = matches!(device.connection, DeviceLink::Bluetooth);
    let mut file = OpenOptions::new().write(true).open(path)?;
    file.write_all(&off_report(bluetooth))?;
    Ok(())
}

/// Change la couleur de la barre lumineuse d'une manette Sony.
/// `[0, 0, 0]` l'éteint. La DualShock 4 et la DualSense ont chacune leur
/// rapport de sortie ; le reste est refusé proprement.
pub fn sony_lightbar(device: &DeviceInfo, color: [u8; 3]) -> Result<()> {
    if is_dualsense(device) {
        dualsense_lightbar(device, color)
    } else if is_dualshock4(device) {
        dualshock4_lightbar(device, color)
    } else {
        Err(Error::Other(
            "la barre lumineuse n'est pas prise en charge par cette manette".into(),
        ))
    }
}

/// Change la couleur de la barre lumineuse de la DualSense.
///
/// En Bluetooth, la manette garde la couleur de son animation de démarrage
/// tant qu'un rapport de configuration de la barre ne l'a pas rendue au
/// logiciel. On l'éteint donc une fraction de seconde avant d'appliquer la
/// couleur, sinon les rapports RGB sont acceptés mais ignorés.
pub fn dualsense_lightbar(device: &DeviceInfo, color: [u8; 3]) -> Result<()> {
    if !is_dualsense(device) {
        return Err(Error::Other(
            "la barre lumineuse de test est réservée à la DualSense".into(),
        ));
    }
    let path = dualsense_hidraw(device)?;
    let bluetooth = matches!(device.connection, DeviceLink::Bluetooth);
    let mut file = OpenOptions::new().write(true).open(path)?;
    file.write_all(&lightbar_setup_report(
        bluetooth,
        next_bluetooth_seq(),
        LIGHTBAR_LIGHT_OUT,
    ))?;
    if color == [0, 0, 0] {
        return Ok(());
    }
    std::thread::sleep(Duration::from_millis(120));
    file.write_all(&lightbar_report(
        bluetooth,
        next_bluetooth_seq(),
        color,
    ))?;
    Ok(())
}

/// Change la couleur de la barre lumineuse d'une DualShock 4.
pub fn dualshock4_lightbar(device: &DeviceInfo, color: [u8; 3]) -> Result<()> {
    if !is_dualshock4(device) {
        return Err(Error::Other(
            "la barre lumineuse de test est réservée à la DualShock 4".into(),
        ));
    }
    let path = dualshock4_hidraw(device)?;
    let bluetooth = matches!(device.connection, DeviceLink::Bluetooth);
    let mut file = OpenOptions::new().write(true).open(path)?;
    file.write_all(&dualshock4_lightbar_report(bluetooth, color))?;
    Ok(())
}

fn is_dualsense(device: &DeviceInfo) -> bool {
    device.vendor == 0x054c && matches!(device.product, 0x0ce6 | 0x0df2)
}

fn is_dualshock4(device: &DeviceInfo) -> bool {
    device.vendor == 0x054c && matches!(device.product, 0x05c4 | 0x09cc | 0x0ba0)
}

fn dualsense_hidraw(device: &DeviceInfo) -> Result<PathBuf> {
    sony_hidraw(device, &[0x0ce6, 0x0df2])
}

fn dualshock4_hidraw(device: &DeviceInfo) -> Result<PathBuf> {
    sony_hidraw(device, &[0x05c4, 0x09cc, 0x0ba0])
}

/// Trouve le hidraw d'une manette Sony, par adresse Bluetooth quand elle est
/// connue, sinon par ses identifiants USB.
fn sony_hidraw(device: &DeviceInfo, products: &[u16]) -> Result<PathBuf> {
    let mut fallback = None;
    for entry in std::fs::read_dir("/sys/class/hidraw")
        .map_err(Error::Io)?
        .flatten()
    {
        let sysfs = entry.path().join("device");
        let Ok(uevent) = std::fs::read_to_string(sysfs.join("uevent")) else {
            continue;
        };
        let id = uevent
            .lines()
            .find_map(|line| line.strip_prefix("HID_ID="))
            .and_then(parse_hid_id);
        let Some((_bus, vendor, product)) = id else {
            continue;
        };
        if vendor != 0x054c || !products.contains(&(product as u16)) {
            continue;
        }
        let uniq = uevent
            .lines()
            .find_map(|line| line.strip_prefix("HID_UNIQ="))
            .unwrap_or("")
            .trim();
        let path = PathBuf::from("/dev").join(entry.file_name());
        if let Some(address) = &device.address {
            if uniq.eq_ignore_ascii_case(address) {
                return Ok(path);
            }
        }
        fallback.get_or_insert(path);
    }
    fallback.ok_or_else(|| Error::Other("hidraw de la manette introuvable".into()))
}

fn parse_hid_id(value: &str) -> Option<(u32, u32, u32)> {
    let mut parts = value.trim().split(':');
    let bus = u32::from_str_radix(parts.next()?, 16).ok()?;
    let vendor = u32::from_str_radix(parts.next()?, 16).ok()? & 0xffff;
    let product = u32::from_str_radix(parts.next()?, 16).ok()? & 0xffff;
    Some((bus, vendor, product))
}

fn next_bluetooth_seq() -> u8 {
    let seq = BLUETOOTH_SEQ.fetch_add(1, Ordering::Relaxed) & 0x0f;
    seq << 4
}

/// `lightbar_setup` : rend la barre au logiciel (elle sort de l'animation
/// de démarrage de la DualSense).
const LIGHTBAR_LIGHT_OUT: u8 = 0x02;

fn trigger_effect(test: TriggerTest) -> ([u8; 11], [u8; 11]) {
    let effect = match test {
        TriggerTest::Resistance => feedback_from(2, 5),
        TriggerTest::Block => feedback_from(0, 8),
        TriggerTest::Vibration => vibration_from(2, 6, 10),
        TriggerTest::Weapon => weapon(2, 8, 8),
    };
    (effect, effect)
}

/// Effet « résistance » officiel : elle commence à la zone `start` et couvre
/// toutes les zones suivantes. Le trio de bits par zone vaut `force - 1`.
fn feedback_from(start: u8, strength: u8) -> [u8; 11] {
    let strength = strength.clamp(1, 8);
    let start = start.min(9);
    let force = strength - 1;
    let mut active: u16 = 0;
    let mut forces: u32 = 0;
    for zone in start..10 {
        active |= 1 << zone;
        forces |= (force as u32) << (3 * zone);
    }
    let mut effect = [0u8; 11];
    effect[0] = 0x21;
    effect[1] = (active & 0xff) as u8;
    effect[2] = (active >> 8) as u8;
    effect[3..7].copy_from_slice(&forces.to_le_bytes());
    effect
}

/// Résistance sur une plage de zones, comme une détente d'arme.
fn weapon(start: u8, end: u8, strength: u8) -> [u8; 11] {
    let start = start.clamp(2, 7);
    let end = end.clamp(start + 1, 8);
    let mask = (1u16 << start) | (1u16 << end);
    let mut effect = [0u8; 11];
    effect[0] = 0x25;
    effect[1] = (mask & 0xff) as u8;
    effect[2] = (mask >> 8) as u8;
    effect[3] = strength.clamp(1, 8) - 1;
    effect
}

/// Gâchette qui vibre à partir d'une zone.
fn vibration_from(start: u8, amplitude: u8, frequency: u8) -> [u8; 11] {
    let amplitude = amplitude.clamp(1, 8);
    let start = start.min(9);
    let force = amplitude - 1;
    let mut active: u16 = 0;
    let mut forces: u32 = 0;
    for zone in start..10 {
        active |= 1 << zone;
        forces |= (force as u32) << (3 * zone);
    }
    let mut effect = [0u8; 11];
    effect[0] = 0x26;
    effect[1] = (active & 0xff) as u8;
    effect[2] = (active >> 8) as u8;
    effect[3..7].copy_from_slice(&forces.to_le_bytes());
    effect[9] = frequency.max(1);
    effect
}

fn trigger_off() -> [u8; 11] {
    let mut effect = [0u8; 11];
    effect[0] = 0x05;
    effect
}

fn off_report(bluetooth: bool) -> Vec<u8> {
    let off = trigger_off();
    output_report(bluetooth, next_bluetooth_seq(), &off, &off)
}

/// Rapport de sortie USB (0x02, 63 octets) ou Bluetooth (0x31, 78 octets).
///
/// `right` et `left` occupent les deux emplacements d'effet ; `lightbar`
/// n'active que la couleur RGB et ne touche pas aux gâchettes.
fn output_report(
    bluetooth: bool,
    seq: u8,
    right: &[u8; 11],
    left: &[u8; 11],
) -> Vec<u8> {
    build_report(bluetooth, seq, Some((right, left)), None)
}

fn lightbar_report(bluetooth: bool, seq: u8, color: [u8; 3]) -> Vec<u8> {
    build_report(bluetooth, seq, None, Some(color))
}

fn build_report(
    bluetooth: bool,
    seq: u8,
    triggers: Option<(&[u8; 11], &[u8; 11])>,
    lightbar: Option<[u8; 3]>,
) -> Vec<u8> {
    let mut report = vec![0u8; if bluetooth { 78 } else { 63 }];
    let common = if bluetooth {
        report[0] = 0x31;
        report[1] = seq;
        report[2] = 0x10;
        3
    } else {
        report[0] = 0x02;
        1
    };
    let block = &mut report[common..common + 47];
    if let Some((right, left)) = triggers {
        // R2 et L2 sont les deux seules fonctions modifiées.
        block[0] = 0x04 | 0x08;
        block[10..21].copy_from_slice(right);
        block[21..32].copy_from_slice(left);
    }
    if let Some(color) = lightbar {
        // Commande de la barre RGB, sans player LEDs ni fondu.
        block[1] = 0x04;
        block[44..47].copy_from_slice(&color);
    }
    if bluetooth {
        let crc = bluetooth_crc(&report[..74]);
        report[74..78].copy_from_slice(&crc.to_le_bytes());
    }
    report
}

/// Rapport qui rend la barre lumineuse au logiciel. `setup` vaut
/// `LIGHTBAR_LIGHT_OUT` pour couper l'animation de démarrage.
fn lightbar_setup_report(bluetooth: bool, seq: u8, setup: u8) -> Vec<u8> {
    let mut report = build_report(bluetooth, seq, None, None);
    let common = if bluetooth { 3 } else { 1 };
    let block = &mut report[common..common + 47];
    block[38] = 0x02; // valid_flag2 : contrôle de la configuration de la barre
    block[41] = setup;
    if bluetooth {
        let crc = bluetooth_crc(&report[..74]);
        report[74..78].copy_from_slice(&crc.to_le_bytes());
    }
    report
}

/// Rapport de sortie DualShock 4 : 0x05 sur USB, 0x11 sur Bluetooth.
fn dualshock4_lightbar_report(bluetooth: bool, color: [u8; 3]) -> Vec<u8> {
    let mut report = vec![0u8; if bluetooth { 78 } else { 32 }];
    let common = if bluetooth {
        report[0] = 0x11;
        // HID data + CRC32 dans les 4 derniers octets.
        report[1] = 0xc0;
        3
    } else {
        report[0] = 0x05;
        1
    };
    let block = &mut report[common..common + 10];
    // LED et moteur : certains pads tiers annulent l'un quand l'autre manque.
    block[0] = 0x03;
    block[5..8].copy_from_slice(&color);
    if bluetooth {
        let crc = bluetooth_crc(&report[..74]);
        report[74..78].copy_from_slice(&crc.to_le_bytes());
    }
    report
}

/// CRC Bluetooth de la DualSense : graine 0xA2, CRC-32 réfléchi, résultat
/// complémenté puis écrit en little-endian.
fn bluetooth_crc(report: &[u8]) -> u32 {
    let crc = crc32_le(0xffff_ffff, &[0xa2]);
    !crc32_le(crc, report)
}

fn crc32_le(mut crc: u32, data: &[u8]) -> u32 {
    for byte in data {
        crc ^= *byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_matches_the_standard_test_vector() {
        assert_eq!(!crc32_le(0xffff_ffff, b"123456789"), 0xcbf4_3926);
    }

    #[test]
    fn haptics_bluetooth_block_has_the_audio_layout_and_crc() {
        let mut report = [0u8; 142];
        report[0] = 0x32;
        report[2] = 0x91;
        report[3] = 0x07;
        report[4] = 0xfe;
        report[5..10].fill(16);
        report[11] = 0x92;
        report[12] = 0x40;
        haptics_sign(&mut report);
        assert_eq!(report[0], 0x32);
        assert_eq!(report[10], 0);
        assert!(report[13..77].iter().all(|byte| *byte == 0));
        assert_eq!(
            u32::from_le_bytes(report[138..142].try_into().unwrap()),
            0x716d_048b
        );
    }

    #[test]
    fn haptics_power_report_gives_the_coils_full_power_and_the_filter() {
        let report = haptics_power_report();
        assert_eq!(report[0], 0x31);
        assert_eq!(report[3], 0x00);
        assert_eq!(report[4], 0x62);
        assert_eq!(report[12], 0x00);
        assert_eq!(report[39], 0x00);
        assert_eq!(report[42], 0x01);
        assert_eq!(
            u32::from_le_bytes(report[74..78].try_into().unwrap()),
            0xa25a_0779
        );
    }

    #[test]
    fn feedback_starts_at_zone_two() {
        let effect = feedback_from(2, 8);
        assert_eq!(effect[0], 0x21);
        assert_eq!(effect[1], 0b1111_1100);
        assert_eq!(effect[2], 0b0000_0011);
        // Zone 2 avec la force maximale (7 sur 3 bits) : ses deux premiers
        // bits tombent dans l'octet 3, le troisième dans l'octet 4.
        assert_eq!(effect[3], 0b1100_0000);
        assert_eq!(effect[4] & 1, 1);
    }

    #[test]
    fn block_starts_at_the_first_zone() {
        let effect = feedback_from(0, 8);
        assert_eq!(effect[0], 0x21);
        assert_eq!(effect[1], 0xff);
        assert_eq!(effect[2], 0b0000_0011);
    }

    #[test]
    fn vibration_and_weapon_have_their_own_modes() {
        let vibration = vibration_from(2, 6, 10);
        assert_eq!(vibration[0], 0x26);
        assert_eq!(vibration[9], 10);
        let weapon = weapon(2, 8, 8);
        assert_eq!(weapon[0], 0x25);
        assert_eq!(weapon[1], 0b0000_0100);
        assert_eq!(weapon[2], 0b0000_0001);
        assert_eq!(weapon[3], 7);
    }

    #[test]
    fn lightbar_report_only_touches_the_led() {
        let usb = lightbar_report(false, 0, [10, 20, 30]);
        assert_eq!(usb.len(), 63);
        assert_eq!(usb[0], 0x02);
        assert_eq!(usb[1], 0);
        assert_eq!(usb[2], 0x04);
        assert_eq!(&usb[45..48], &[10, 20, 30]);
        assert!(usb[11..33].iter().all(|byte| *byte == 0));

        let bluetooth = lightbar_report(true, 0x30, [30, 20, 10]);
        assert_eq!(bluetooth.len(), 78);
        assert_eq!(bluetooth[0], 0x31);
        assert_eq!(bluetooth[4], 0x04);
        assert_eq!(&bluetooth[47..50], &[30, 20, 10]);
        assert!(bluetooth[13..35].iter().all(|byte| *byte == 0));
        let stored = u32::from_le_bytes(bluetooth[74..78].try_into().unwrap());
        assert_eq!(stored, bluetooth_crc(&bluetooth[..74]));
        assert_ne!(stored, 0);
    }

    #[test]
    fn lightbar_setup_releases_the_led_without_touching_the_color() {
        let usb = lightbar_setup_report(false, 0, LIGHTBAR_LIGHT_OUT);
        assert_eq!(usb.len(), 63);
        assert_eq!(usb[0], 0x02);
        assert_eq!(usb[39], 0x02); // valid_flag2
        assert_eq!(usb[42], LIGHTBAR_LIGHT_OUT); // lightbar_setup
        assert!(usb[45..48].iter().all(|byte| *byte == 0));

        let bluetooth = lightbar_setup_report(true, 0x30, LIGHTBAR_LIGHT_OUT);
        assert_eq!(bluetooth.len(), 78);
        assert_eq!(bluetooth[0], 0x31);
        assert_eq!(bluetooth[41], 0x02);
        assert_eq!(bluetooth[44], LIGHTBAR_LIGHT_OUT);
        let stored = u32::from_le_bytes(bluetooth[74..78].try_into().unwrap());
        assert_eq!(stored, bluetooth_crc(&bluetooth[..74]));
    }

    #[test]
    fn dualshock_4_lightbar_uses_the_legacy_report() {
        let usb = dualshock4_lightbar_report(false, [10, 20, 30]);
        assert_eq!(usb.len(), 32);
        assert_eq!(usb[0], 0x05);
        assert_eq!(usb[1], 0x03);
        assert_eq!(&usb[6..9], &[10, 20, 30]);

        let bluetooth = dualshock4_lightbar_report(true, [30, 20, 10]);
        assert_eq!(bluetooth.len(), 78);
        assert_eq!(bluetooth[0], 0x11);
        assert_eq!(bluetooth[1], 0xc0);
        assert_eq!(bluetooth[3], 0x03);
        assert_eq!(&bluetooth[8..11], &[30, 20, 10]);
        let stored = u32::from_le_bytes(bluetooth[74..78].try_into().unwrap());
        assert_eq!(stored, bluetooth_crc(&bluetooth[..74]));
    }

    #[test]
    fn trigger_reports_still_set_the_expected_wire_format() {
        let right = feedback_from(2, 7);
        let left = feedback_from(2, 7);
        let usb = output_report(false, 0, &right, &left);
        assert_eq!(usb.len(), 63);
        assert_eq!(usb[0], 0x02);
        assert_eq!(usb[1], 0x0c);
        assert_eq!(&usb[11..22], &right);
        assert_eq!(&usb[22..33], &left);

        let bluetooth = output_report(true, 0x30, &right, &left);
        assert_eq!(bluetooth.len(), 78);
        assert_eq!(bluetooth[0], 0x31);
        assert_eq!(bluetooth[1], 0x30);
        assert_eq!(bluetooth[2], 0x10);
        assert_eq!(bluetooth[3], 0x0c);
        assert_eq!(&bluetooth[13..24], &right);
        assert_eq!(&bluetooth[24..35], &left);
        let stored = u32::from_le_bytes(bluetooth[74..78].try_into().unwrap());
        assert_eq!(stored, bluetooth_crc(&bluetooth[..74]));
        assert_ne!(stored, 0);
    }

    #[test]
    fn hid_ids_are_parsed_from_sysfs() {
        assert_eq!(
            parse_hid_id("0005:0000054C:00000CE6"),
            Some((0x0005, 0x054c, 0x0ce6))
        );
        assert_eq!(parse_hid_id("invalid"), None);
    }
}
