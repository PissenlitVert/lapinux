//! Énumération des manettes et claviers exposés par evdev.

use crate::codes::*;
use ludus_core::model::DeviceInfo;
use std::path::PathBuf;

pub fn is_gamepad(device: &evdev::Device) -> bool {
    let has_pad_button = device
        .supported_keys()
        .map(|keys| {
            keys.contains(evdev::KeyCode::new(BTN_SOUTH))
        })
        .unwrap_or(false);
    let has_stick = device
        .supported_absolute_axes()
        .map(|axes| {
            axes.contains(evdev::AbsoluteAxisCode(ABS_X))
                && axes.contains(evdev::AbsoluteAxisCode(ABS_Y))
        })
        .unwrap_or(false);
    has_pad_button && has_stick
}

pub fn is_keyboard(device: &evdev::Device) -> bool {
    if is_gamepad(device) {
        return false;
    }
    device
        .supported_keys()
        .map(|keys| {
            keys.contains(evdev::KeyCode::new(30)) // KEY_A
                && keys.contains(evdev::KeyCode::new(KEY_LEFTMETA))
        })
        .unwrap_or(false)
}

pub fn axes_of(device: &evdev::Device) -> Vec<String> {
    device
        .supported_absolute_axes()
        .map(|axes| {
            axes.iter()
                .filter_map(|axis| axis_name(axis.0).map(|n| n.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

pub fn describe(path: &PathBuf, device: &evdev::Device) -> DeviceInfo {
    let id = device.input_id();
    DeviceInfo {
        path: path.clone(),
        name: device.name().unwrap_or("périphérique inconnu").to_string(),
        vendor: id.vendor(),
        product: id.product(),
        gamepad: is_gamepad(device),
        buttons: device
            .supported_keys()
            .map(|keys| keys.iter().count())
            .unwrap_or(0),
        axes: axes_of(device),
    }
}

pub fn list() -> Vec<DeviceInfo> {
    let mut out: Vec<DeviceInfo> = evdev::enumerate()
        .map(|(path, device)| describe(&path, &device))
        .collect();
    out.sort_by(|a, b| b.gamepad.cmp(&a.gamepad).then(a.name.cmp(&b.name)));
    out
}

pub fn list_gamepads() -> Vec<DeviceInfo> {
    list().into_iter().filter(|d| d.gamepad).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enumerates_without_panicking() {
        // Aucune manette n'est garantie sur la machine de test : on vérifie
        // seulement que l'énumération fonctionne et que les champs sont cohérents.
        let devices = list();
        for device in &devices {
            assert!(!device.name.is_empty());
            assert!(device.path.starts_with("/dev/input"));
        }
    }
}
