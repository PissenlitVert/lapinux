//! Codes evdev utilisés et tables de noms lisibles.

pub const EV_KEY: u16 = 1;
pub const EV_ABS: u16 = 3;
pub const EV_SYN: u16 = 0;

pub const BTN_SOUTH: u16 = 0x130; // A / croix
pub const BTN_EAST: u16 = 0x131; // B / rond
pub const BTN_NORTH: u16 = 0x133; // X / carré
pub const BTN_WEST: u16 = 0x134; // Y / triangle
pub const BTN_TL: u16 = 0x136;
pub const BTN_TR: u16 = 0x137;
pub const BTN_TL2: u16 = 0x138;
pub const BTN_TR2: u16 = 0x139;
pub const BTN_SELECT: u16 = 0x13a;
pub const BTN_START: u16 = 0x13b;
pub const BTN_MODE: u16 = 0x13c;
pub const BTN_THUMBL: u16 = 0x13d;
pub const BTN_THUMBR: u16 = 0x13e;

pub const BTN_DPAD_UP: u16 = 0x220;
pub const BTN_DPAD_DOWN: u16 = 0x221;
pub const BTN_DPAD_LEFT: u16 = 0x222;
pub const BTN_DPAD_RIGHT: u16 = 0x223;

pub const ABS_X: u16 = 0x00;
pub const ABS_Y: u16 = 0x01;
pub const ABS_Z: u16 = 0x02;
pub const ABS_RX: u16 = 0x03;
pub const ABS_RY: u16 = 0x04;
pub const ABS_RZ: u16 = 0x05;
pub const ABS_HAT0X: u16 = 0x10;
pub const ABS_HAT0Y: u16 = 0x11;

/// Touches clavier utiles aux raccourcis globaux.
pub const KEY_F1: u16 = 59;
pub const KEY_F12: u16 = 88;
pub const KEY_LEFTMETA: u16 = 125;
pub const KEY_RIGHTMETA: u16 = 126;

pub const BTN_TABLE: [(&str, u16); 13] = [
    ("BTN_SOUTH", BTN_SOUTH),
    ("BTN_EAST", BTN_EAST),
    ("BTN_NORTH", BTN_NORTH),
    ("BTN_WEST", BTN_WEST),
    ("BTN_TL", BTN_TL),
    ("BTN_TR", BTN_TR),
    ("BTN_TL2", BTN_TL2),
    ("BTN_TR2", BTN_TR2),
    ("BTN_SELECT", BTN_SELECT),
    ("BTN_START", BTN_START),
    ("BTN_MODE", BTN_MODE),
    ("BTN_THUMBL", BTN_THUMBL),
    ("BTN_THUMBR", BTN_THUMBR),
];

pub const AXIS_TABLE: [(&str, u16); 8] = [
    ("ABS_X", ABS_X),
    ("ABS_Y", ABS_Y),
    ("ABS_Z", ABS_Z),
    ("ABS_RX", ABS_RX),
    ("ABS_RY", ABS_RY),
    ("ABS_RZ", ABS_RZ),
    ("ABS_HAT0X", ABS_HAT0X),
    ("ABS_HAT0Y", ABS_HAT0Y),
];

pub const KEY_ALIASES: [(&str, u16); 3] = [
    ("Meta", KEY_LEFTMETA),
    ("F1", KEY_F1),
    ("F12", KEY_F12),
];

pub fn button_name(code: u16) -> Option<&'static str> {
    BTN_TABLE
        .iter()
        .find(|(_, c)| *c == code)
        .map(|(name, _)| *name)
}

pub fn button_code(name: &str) -> Option<u16> {
    let upper = name.to_ascii_uppercase();
    BTN_TABLE
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(&upper))
        .map(|(_, code)| *code)
}

pub fn axis_name(code: u16) -> Option<&'static str> {
    AXIS_TABLE
        .iter()
        .find(|(_, c)| *c == code)
        .map(|(name, _)| *name)
}

pub fn axis_code(name: &str) -> Option<u16> {
    let upper = name.to_ascii_uppercase();
    AXIS_TABLE
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(&upper))
        .map(|(_, code)| *code)
}

pub fn key_or_button_code(name: &str) -> Option<u16> {
    button_code(name)
        .or_else(|| key_code(name))
        .or_else(|| axis_code(name))
}

pub fn key_code(name: &str) -> Option<u16> {
    KEY_ALIASES
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, code)| *code)
}

/// Libellé court pour l'UI (A, B, X, Y, LB, RB, LT, RT…).
pub fn button_label(code: u16) -> &'static str {
    match code {
        BTN_SOUTH => "A",
        BTN_EAST => "B",
        BTN_NORTH => "X",
        BTN_WEST => "Y",
        BTN_TL => "LB",
        BTN_TR => "RB",
        BTN_TL2 => "LT",
        BTN_TR2 => "RT",
        BTN_SELECT => "Select",
        BTN_START => "Start",
        BTN_MODE => "Guide",
        BTN_THUMBL => "L3",
        BTN_THUMBR => "R3",
        KEY_LEFTMETA | KEY_RIGHTMETA => "Meta",
        KEY_F1 => "F1",
        KEY_F12 => "F12",
        _ => "?",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_roundtrip() {
        for (name, code) in BTN_TABLE {
            assert_eq!(button_name(code), Some(name));
            assert_eq!(button_code(name), Some(code));
        }
        for (name, code) in AXIS_TABLE {
            assert_eq!(axis_name(code), Some(name));
            assert_eq!(axis_code(name), Some(code));
        }
    }

    #[test]
    fn aliases_are_case_insensitive() {
        assert_eq!(button_code("btn_south"), Some(BTN_SOUTH));
        assert_eq!(key_code("meta"), Some(KEY_LEFTMETA));
        assert_eq!(button_label(BTN_SOUTH), "A");
        assert_eq!(button_label(0x999), "?");
    }
}
