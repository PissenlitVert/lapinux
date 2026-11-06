//! Rapports HID bruts de la Steam Controller 2026 (« Triton »).
//!
//! `hid-steam` masque `event27` tant qu'un client garde le hidraw ouvert. On
//! ne garde donc jamais ce descripteur : les relevés de toucher, pression et
//! Grip Sense sont ponctuels, et l'haptique des pavés est envoyée en un seul
//! rapport.

use crate::{Error, Result};
use lapinux_core::model::DeviceInfo;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Pavé tactile gauche ou droit.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SteamPad {
    /// Contact capacitif signalé par le rapport HID.
    pub touched: bool,
    /// Position horizontale normalisée, de 0 (gauche) à 1 (droite).
    pub x: f32,
    /// Position verticale normalisée, de 0 (haut) à 1 (bas).
    pub y: f32,
    /// Pression normalisée, de 0 à 1.
    pub pressure: f32,
}

/// État complet des deux pavés et des capteurs capacitifs de la 2026.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SteamPads {
    pub left: SteamPad,
    pub right: SteamPad,
    pub left_stick_touch: bool,
    pub right_stick_touch: bool,
    /// Grip Sense : contact capacitif sur la poignée, distinct du bouton.
    pub left_grip_touch: bool,
    pub right_grip_touch: bool,
    /// Masque brut des boutons et capteurs capacitifs (octets 2 à 5).
    /// Les capteurs des poignées et des sticks y occupent les bits 28 à 31
    /// et 20 à 25 ; l'afficher permet de distinguer un capteur muet d'un
    /// simple décalage de décodage.
    pub buttons: u32,
}

/// Vrai pour les quatre identifiants de la Steam Controller 2026.
pub fn is_steam_2026(device: &DeviceInfo) -> bool {
    device.vendor == 0x28de && matches!(device.product, 0x1302..=0x1305)
}

/// Décode un rapport d'entrée Triton (`0x42` ou `0x45`).
///
/// Les champs sont ceux documentés par `hid-steam` : octets de boutons en 4 et
/// 5, coordonnées et pressions des pavés en 18..30. Les bits de contact
/// capacitif ne sont pas transmis par evdev ; ils sont encore présents dans le
/// rapport brut, c'est tout l'intérêt de ce décodeur.
pub fn decode_report(data: &[u8]) -> Option<SteamPads> {
    if data.len() < 30 || !matches!(data[0], 0x42 | 0x45) {
        return None;
    }

    let left_x = le_i16(&data[18..]);
    let left_y = le_i16(&data[20..]);
    let right_x = le_i16(&data[24..]);
    let right_y = le_i16(&data[26..]);
    let left_pressure = u16::from_le_bytes([data[22], data[23]]) as f32 / 32768.0;
    let right_pressure = u16::from_le_bytes([data[28], data[29]]) as f32 / 32768.0;
    let buttons = u32::from_le_bytes([data[2], data[3], data[4], data[5]]);

    Some(SteamPads {
        left: SteamPad {
            touched: data[5] & 0x02 != 0,
            x: pad_axis(i32::from(left_x)),
            y: pad_axis(-i32::from(left_y)),
            pressure: left_pressure.clamp(0.0, 1.0),
        },
        right: SteamPad {
            touched: data[4] & 0x20 != 0,
            x: pad_axis(i32::from(right_x)),
            y: pad_axis(-i32::from(right_y)),
            pressure: right_pressure.clamp(0.0, 1.0),
        },
        left_stick_touch: data[5] & 0x01 != 0,
        right_stick_touch: data[4] & 0x10 != 0,
        left_grip_touch: data[5] & 0x20 != 0,
        right_grip_touch: data[5] & 0x10 != 0,
        buttons,
    })
}

fn pad_axis(raw: i32) -> f32 {
    (raw as f32 / 65536.0 + 0.5).clamp(0.0, 1.0)
}

fn le_i16(bytes: &[u8]) -> i16 {
    i16::from_le_bytes([bytes[0], bytes[1]])
}

/// Relevé d'une fenêtre HID brute.
///
/// Ouvrir le hidraw du pilote Steam fait disparaître temporairement `event27` ;
/// le descripteur est donc refermé dès le relevé terminé. On conserve tous les
/// contacts vus pendant la fenêtre : un clic de souris oblige souvent à lâcher
/// les poignées avant que le relevé commence, et le premier rapport reçu peut
/// être une trame de repos. Sans contact, le dernier rapport est quand même
/// rendu. Les bits de boutons sont fusionnés de la même façon pour le
/// diagnostic.
pub fn sample(device: &DeviceInfo, duration_ms: u16) -> Result<SteamPads> {
    if !is_steam_2026(device) {
        return Err(Error::Other(
            "les pavés HID sont réservés à la Steam Controller 2026".into(),
        ));
    }
    let path = raw_path(device)?;
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)?;
    let timeout = Duration::from_millis(u64::from(duration_ms.clamp(80, 2_500)));
    let deadline = Instant::now() + timeout;
    let mut buffer = [0u8; 128];
    let mut latest = None;
    let mut seen = SteamPads::default();

    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                if let Some(pads) = decode_report(&buffer[..n]) {
                    // On garde les contacts et les bits vus, même s'ils sont
                    // relâchés avant la fin de la fenêtre.
                    merge_contacts(&mut seen, &pads);
                    latest = Some(pads);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error.into()),
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }

    let mut pads = latest.ok_or_else(|| Error::Other("aucun rapport HID des pavés reçu".into()))?;
    merge_contacts(&mut pads, &seen);
    Ok(pads)
}

/// Fusionne les contacts et les bits de boutons d'un relevé dans un autre.
/// Les positions et pressions restent celles du dernier rapport complet.
fn merge_contacts(into: &mut SteamPads, from: &SteamPads) {
    into.left.touched |= from.left.touched;
    into.right.touched |= from.right.touched;
    into.left_stick_touch |= from.left_stick_touch;
    into.right_stick_touch |= from.right_stick_touch;
    into.left_grip_touch |= from.left_grip_touch;
    into.right_grip_touch |= from.right_grip_touch;
    into.buttons |= from.buttons;
}

/// Joue une courte séquence d'impulsions sur les pavés, pas sur les moteurs de
/// rumble. Le rapport `0x81` est celui que `hid-steam` utilise lui-même pour
/// les clics haptiques des pavés.
pub fn haptic(device: &DeviceInfo, left: bool, right: bool, duration_ms: u16) -> Result<()> {
    if !is_steam_2026(device) {
        return Err(Error::Other(
            "l'haptique des pavés est réservée à la Steam Controller 2026".into(),
        ));
    }
    if !left && !right {
        return Err(Error::Other("choisis au moins un pavé".into()));
    }
    let path = raw_path(device)?;
    let report = haptic_report(left, right, duration_ms)?;
    let mut file = OpenOptions::new().write(true).open(path)?;
    file.write_all(&report)?;
    Ok(())
}

/// Joue un rumble court sur les moteurs des poignées, via le rapport `0x80`.
///
/// `0x80` est le rumble stéréo de la 2026, distinct des impulsions de pavé
/// (`0x81`) et du rumble global FF. Le firmware coupe les moteurs environ
/// 50 ms après le dernier rapport : on le réémet toutes les 40 ms pendant la
/// durée demandée, puis on envoie un rapport d'arrêt. Le hidraw reste ouvert
/// le temps du test ; l'interface doit donc garder son état si evdev
/// disparaît temporairement (manette sans fil).
pub fn grip_haptic(device: &DeviceInfo, left: bool, right: bool, duration_ms: u16) -> Result<()> {
    if !is_steam_2026(device) {
        return Err(Error::Other(
            "l'haptique des poignées est réservée à la Steam Controller 2026".into(),
        ));
    }
    let report = grip_haptic_report(left, right)?;
    let path = raw_path(device)?;
    let mut file = OpenOptions::new().write(true).open(path)?;
    let duration = Duration::from_millis(u64::from(duration_ms.clamp(80, 1_000)));
    let started = Instant::now();
    let mut write_error = None;

    // La manette s'arrête seule après ~50 ms : sans réémission, le test
    // ressemblerait à un clic minuscule au lieu d'un vrai rumble.
    while started.elapsed() < duration {
        if let Err(error) = file.write_all(&report) {
            write_error = Some(Error::Io(error));
            break;
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    let stop = [0x80u8, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    let stop_result = file.write_all(&stop);
    if let Some(error) = write_error {
        return Err(error);
    }
    stop_result?;
    Ok(())
}

/// Trame `0x80` : type continu, intensité nulle, vitesse et gain par moteur.
/// L'offset correspond à `struct steam_ibex_haptic_rumble` du noyau.
fn grip_haptic_report(left: bool, right: bool) -> Result<[u8; 10]> {
    if !left && !right {
        return Err(Error::Other("choisis au moins une poignée".into()));
    }
    let speed = 0x7fffu16;
    let gain = 2u8;
    let mut report = [0u8; 10];
    report[0] = 0x80;
    // report[1] = type 0 (continu) ; report[2..4] = intensité 0.
    if left {
        report[4..6].copy_from_slice(&speed.to_le_bytes());
        report[6] = gain;
    }
    if right {
        report[7..9].copy_from_slice(&speed.to_le_bytes());
        report[9] = gain;
    }
    Ok(report)
}

fn haptic_report(left: bool, right: bool, duration_ms: u16) -> Result<[u8; 8]> {
    if !left && !right {
        return Err(Error::Other("choisis au moins un pavé".into()));
    }
    // Le pilote inverse gauche/droite dans ce rapport pour compatibilité avec
    // l'ancienne manette : gauche logique = 1, droite logique = 0.
    let side = match (left, right) {
        (true, true) => 2,
        (true, false) => 1,
        (false, true) => 0,
        (false, false) => unreachable!(),
    };
    let count = (u32::from(duration_ms.clamp(40, 1000)) * 1000 / 8_000).max(1) as u16;
    let mut report = [0u8; 8];
    report[0] = 0x81;
    report[1] = side;
    report[2..4].copy_from_slice(&4_000u16.to_le_bytes());
    report[4..6].copy_from_slice(&4_000u16.to_le_bytes());
    report[6..8].copy_from_slice(&count.to_le_bytes());
    Ok(report)
}

/// Le hidraw « client » correspondant à l'event evdev : même chemin physique
/// HID, même couple Valve 28de:130x. On ne prend pas un autre slot du Puck.
fn raw_path(device: &DeviceInfo) -> Result<PathBuf> {
    let event = evdev::Device::open(&device.path)?;
    let phys = event
        .physical_path()
        .ok_or_else(|| Error::Other("chemin physique de la manette inconnu".into()))?;
    let entries = std::fs::read_dir("/sys/class/hidraw").map_err(Error::Io)?;
    for entry in entries.flatten() {
        let uevent = match std::fs::read_to_string(entry.path().join("device/uevent")) {
            Ok(uevent) => uevent,
            Err(_) => continue,
        };
        let hid_phys = uevent
            .lines()
            .find_map(|line| line.strip_prefix("HID_PHYS="))
            .map(str::trim);
        let id = uevent
            .lines()
            .find_map(|line| line.strip_prefix("HID_ID="))
            .and_then(parse_hid_id);
        let Some((vendor, product)) = id else {
            continue;
        };
        if hid_phys == Some(phys)
            && vendor == 0x28de
            && matches!(product, 0x1302..=0x1305)
        {
            return Ok(PathBuf::from("/dev").join(entry.file_name()));
        }
    }
    Err(Error::Other(
        "hidraw de la Steam Controller 2026 introuvable".into(),
    ))
}

fn parse_hid_id(value: &str) -> Option<(u32, u32)> {
    let mut parts = value.trim().split(':');
    let _bus = u32::from_str_radix(parts.next()?, 16).ok()?;
    let vendor = u32::from_str_radix(parts.next()?, 16).ok()? & 0xffff;
    let product = u32::from_str_radix(parts.next()?, 16).ok()? & 0xffff;
    Some((vendor, product))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_report() -> Vec<u8> {
        let mut data = vec![0u8; 46];
        data[0] = 0x45;
        // Clic droit et contact droit des sticks ; Grip Sense gauche + droit,
        // toucher du pavé gauche et du stick gauche.
        data[4] = 0x20 | 0x10;
        data[5] = 0x02 | 0x01 | 0x20 | 0x10;
        data[18..20].copy_from_slice(&16384i16.to_le_bytes());
        data[20..22].copy_from_slice(&8192i16.to_le_bytes());
        data[22..24].copy_from_slice(&16384u16.to_le_bytes());
        data[24..26].copy_from_slice(&32767i16.to_le_bytes());
        data[26..28].copy_from_slice(&(-32768i16).to_le_bytes());
        data[28..30].copy_from_slice(&8192u16.to_le_bytes());
        data
    }

    #[test]
    fn raw_report_exposes_both_pads_and_capacitive_sensors() {
        let pads = decode_report(&full_report()).unwrap();
        assert!(pads.left.touched);
        assert!(pads.right.touched);
        assert!((pads.left.x - 0.75).abs() < 0.01);
        assert!((pads.left.y - 0.375).abs() < 0.01);
        assert!((pads.left.pressure - 0.5).abs() < 0.01);
        assert!((pads.right.x - 0.999).abs() < 0.01);
        assert!((pads.right.y - 1.0).abs() < 0.01);
        assert!((pads.right.pressure - 0.25).abs() < 0.01);
        assert!(pads.left_stick_touch);
        assert!(pads.right_stick_touch);
        assert!(pads.left_grip_touch);
        assert!(pads.right_grip_touch);
        assert_eq!(pads.buttons, 0x3330_0000);
    }

    #[test]
    fn grip_haptic_uses_the_ibex_rumble_layout() {
        let left = grip_haptic_report(true, false).unwrap();
        assert_eq!(left, [0x80, 0, 0, 0, 0xff, 0x7f, 2, 0, 0, 0]);

        let both = grip_haptic_report(true, true).unwrap();
        assert_eq!(&both[4..7], &[0xff, 0x7f, 2]);
        assert_eq!(&both[7..10], &[0xff, 0x7f, 2]);
        assert!(grip_haptic_report(false, false).is_err());
    }

    #[test]
    fn capture_merges_contacts_seen_during_the_window() {
        let touched = decode_report(&full_report()).unwrap();
        let mut rest = [0u8; 46];
        rest[0] = 0x45;
        let mut latest = decode_report(&rest).unwrap();
        let mut seen = SteamPads::default();
        merge_contacts(&mut seen, &touched);
        merge_contacts(&mut latest, &seen);

        assert!(latest.left.touched);
        assert!(latest.right.touched);
        assert!(latest.left_stick_touch);
        assert!(latest.right_stick_touch);
        assert!(latest.left_grip_touch);
        assert!(latest.right_grip_touch);
        assert_eq!(latest.buttons, touched.buttons);
    }

    #[test]
    fn short_or_unknown_reports_are_ignored() {
        assert!(decode_report(&[]).is_none());
        assert!(decode_report(&[0x45; 29]).is_none());
        assert!(decode_report(&[0x31; 46]).is_none());
    }

    #[test]
    fn pad_haptic_uses_the_reversed_legacy_sides() {
        let left = haptic_report(true, false, 240).unwrap();
        assert_eq!(left[0], 0x81);
        assert_eq!(left[1], 1);
        assert_eq!(u16::from_le_bytes([left[6], left[7]]), 30);
        let right = haptic_report(false, true, 240).unwrap();
        assert_eq!(right[1], 0);
        let both = haptic_report(true, true, 240).unwrap();
        assert_eq!(both[1], 2);
        assert!(haptic_report(false, false, 240).is_err());
    }
}
