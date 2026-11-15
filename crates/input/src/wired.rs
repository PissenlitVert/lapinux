//! Interface HID USB d'une manette sans fil.
//!
//! Le pilote `playstation` refuse une seconde instance quand la même manette
//! est encore liée en Bluetooth (même adresse MAC). Le câble est alors visible
//! côté USB, mais aucune entrée evdev n'est créée : le noyau continue
//! d'utiliser le Bluetooth. Ce module retrouve cette interface USB par son
//! adresse pour que l'interface puisse proposer la bascule et pour que la
//! fiche garde la même identité d'un bus à l'autre.

use std::fs;
use std::path::{Path, PathBuf};

/// Une interface HID USB appartenant à la même manette que l'adresse donnée.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsbHid {
    /// Identifiant sysfs, par exemple `0003:054C:0CE6.001C`.
    pub id: String,
    pub vendor: u16,
    pub product: u16,
    pub mac: String,
    /// Nœud usbfs, par exemple `/dev/bus/usb/001/017`.
    pub node: PathBuf,
    pub interface: u32,
    /// Le pilote HID a déjà créé la manette filaire.
    pub bound: bool,
}

/// Cherche l'interface HID USB de la manette dont l'adresse est `mac`.
pub fn usb_hid_for(mac: &str) -> Option<UsbHid> {
    usb_hid_in(Path::new("/sys/bus/hid/devices"), mac)
}

fn usb_hid_in(root: &Path, mac: &str) -> Option<UsbHid> {
    let entries = fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        if let Some(hid) = usb_hid_from_path(&entry.path(), mac) {
            return Some(hid);
        }
    }
    None
}

fn usb_hid_from_path(hid: &Path, mac: &str) -> Option<UsbHid> {
    let uevent = fs::read_to_string(hid.join("uevent")).ok()?;
    let fields = parse_uevent(&uevent);
    let hid_id = fields.get("HID_ID")?;
    if !hid_id.starts_with("0003:") {
        return None;
    }
    let uniq = fields.get("HID_UNIQ")?;
    if !uniq.eq_ignore_ascii_case(mac.trim()) {
        return None;
    }
    let (vendor, product) = parse_usb_hid_id(hid_id)?;
    let id = hid.file_name()?.to_string_lossy().to_string();
    let hid = fs::canonicalize(hid).ok()?;
    let interface_dir = hid.parent()?;
    let usb_dir = interface_dir.parent()?;
    let busnum = read_trim(usb_dir.join("busnum"))?.parse::<u32>().ok()?;
    let devnum = read_trim(usb_dir.join("devnum"))?.parse::<u32>().ok()?;
    let interface = read_trim(interface_dir.join("bInterfaceNumber"))?
        .parse::<u32>()
        .ok()?;
    Some(UsbHid {
        id,
        vendor,
        product,
        mac: uniq.to_string(),
        node: PathBuf::from(format!("/dev/bus/usb/{busnum:03}/{devnum:03}")),
        interface,
        bound: hid.join("driver").exists(),
    })
}

fn parse_uevent(text: &str) -> std::collections::HashMap<String, String> {
    text.lines()
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            Some((key.to_string(), value.trim().to_string()))
        })
        .collect()
}

fn parse_usb_hid_id(value: &str) -> Option<(u16, u16)> {
    let mut parts = value.split(':');
    if parts.next()? != "0003" {
        return None;
    }
    let vendor = u16::from_str_radix(parts.next()?, 16).ok()?;
    let product = u16::from_str_radix(parts.next()?, 16).ok()?;
    Some((vendor, product))
}

fn read_trim(path: PathBuf) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, text).unwrap();
    }

    fn fake_hid(root: &Path, bound: bool) -> PathBuf {
        let usb = root.join("usb1/1-4");
        let interface = usb.join("1-4:1.3");
        let hid = interface.join("0003:054C:0CE6.001C");
        write(&usb.join("busnum"), "1\n");
        write(&usb.join("devnum"), "17\n");
        write(&interface.join("bInterfaceNumber"), "3\n");
        write(
            &hid.join("uevent"),
            "HID_ID=0003:0000054C:00000CE6\nHID_NAME=DualSense\nHID_UNIQ=02:00:00:00:00:01\n",
        );
        if bound {
            fs::create_dir_all(hid.join("driver")).unwrap();
        }
        hid
    }

    #[test]
    fn finds_the_usb_interface_by_mac() {
        let tmp = tempfile::tempdir().unwrap();
        let hid = fake_hid(tmp.path(), false);
        let found = usb_hid_from_path(&hid, "02:00:00:00:00:01").unwrap();
        assert_eq!(found.vendor, 0x054c);
        assert_eq!(found.product, 0x0ce6);
        assert_eq!(found.interface, 3);
        assert!(!found.bound);
        assert_eq!(found.node, PathBuf::from("/dev/bus/usb/001/017"));
        assert!(usb_hid_from_path(&hid, "AA:BB:CC:DD:EE:FF").is_none());
    }

    #[test]
    fn a_bound_hid_needs_no_handoff() {
        let tmp = tempfile::tempdir().unwrap();
        let hid = fake_hid(tmp.path(), true);
        assert!(usb_hid_from_path(&hid, "02:00:00:00:00:01").unwrap().bound);
    }
}
