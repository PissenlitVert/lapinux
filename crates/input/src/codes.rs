//! Codes evdev utilisés et tables de noms lisibles.

pub const EV_KEY: u16 = 1;
pub const EV_ABS: u16 = 3;
pub const EV_SYN: u16 = 0;

pub const BTN_SOUTH: u16 = 0x130; // A / croix
pub const BTN_EAST: u16 = 0x131; // B / rond
pub const BTN_NORTH: u16 = 0x133; // X / carré
pub const BTN_WEST: u16 = 0x134; // Y / triangle
/// Bouton Capture (Switch) / partage selon la manette.
pub const BTN_Z: u16 = 0x135;
pub const BTN_TL: u16 = 0x136;
pub const BTN_TR: u16 = 0x137;
pub const BTN_TL2: u16 = 0x138;
pub const BTN_TR2: u16 = 0x139;
pub const BTN_SELECT: u16 = 0x13a;
pub const BTN_START: u16 = 0x13b;
pub const BTN_MODE: u16 = 0x13c;
pub const BTN_THUMBL: u16 = 0x13d;
pub const BTN_THUMBR: u16 = 0x13e;
pub const BTN_C: u16 = 0x132;
pub const BTN_LEFT: u16 = 0x110;
pub const BTN_RIGHT: u16 = 0x111;
pub const BTN_TOUCH: u16 = 0x14a;
/// Bouton d'accès rapide du Steam Deck et de la Steam Controller 2026.
pub const BTN_BASE: u16 = 0x126;
/// Clic du pavé gauche ; la Steam Controller 2015 y met le contact.
pub const BTN_THUMB: u16 = 0x12c;
/// Clic du pavé droit ; la Steam Controller 2015 y met le contact.
pub const BTN_THUMB2: u16 = 0x12d;
pub const BTN_GRIPL: u16 = 0x224;
pub const BTN_GRIPR: u16 = 0x225;
pub const BTN_GRIPL2: u16 = 0x226;
pub const BTN_GRIPR2: u16 = 0x227;
pub const BTN_TRIGGER_HAPPY1: u16 = 0x2c0;
pub const BTN_TRIGGER_HAPPY2: u16 = 0x2c1;
pub const BTN_TRIGGER_HAPPY3: u16 = 0x2c2;
pub const BTN_TRIGGER_HAPPY4: u16 = 0x2c3;
pub const BTN_TRIGGER_HAPPY5: u16 = 0x2c4;
pub const BTN_TRIGGER_HAPPY6: u16 = 0x2c5;
pub const BTN_TRIGGER_HAPPY7: u16 = 0x2c6;
pub const BTN_TRIGGER_HAPPY8: u16 = 0x2c7;
pub const BTN_TRIGGER_HAPPY9: u16 = 0x2c8;
pub const BTN_TRIGGER_HAPPY10: u16 = 0x2c9;

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
pub const ABS_GAS: u16 = 0x09;
pub const ABS_BRAKE: u16 = 0x0a;
pub const ABS_HAT0X: u16 = 0x10;
pub const ABS_HAT0Y: u16 = 0x11;
/// Pavé tactile droit de la Steam Controller 2026 (`ABS_HAT1X/Y`).
pub const ABS_HAT1X: u16 = 0x12;
pub const ABS_HAT1Y: u16 = 0x13;
/// Gâchettes analogiques des manettes Valve : `hid-steam` les place sur la
/// croix 2 (`Y` à gauche, `X` à droite) au lieu de `ABS_Z`/`ABS_RZ`.
pub const ABS_HAT2X: u16 = 0x14;
pub const ABS_HAT2Y: u16 = 0x15;
pub const ABS_MT_POSITION_X: u16 = 0x35;
pub const ABS_MT_POSITION_Y: u16 = 0x36;

/// Touches clavier utiles aux raccourcis globaux.
pub const KEY_A: u16 = 30;
pub const KEY_ESC: u16 = 1;
pub const KEY_TAB: u16 = 15;
pub const KEY_ENTER: u16 = 28;
pub const KEY_LEFTCTRL: u16 = 29;
pub const KEY_LEFTSHIFT: u16 = 42;
pub const KEY_LEFTALT: u16 = 56;
pub const KEY_SPACE: u16 = 57;
pub const KEY_F1: u16 = 59;
pub const KEY_F2: u16 = 60;
pub const KEY_F3: u16 = 61;
pub const KEY_F4: u16 = 62;
pub const KEY_F5: u16 = 63;
pub const KEY_F6: u16 = 64;
pub const KEY_F7: u16 = 65;
pub const KEY_F8: u16 = 66;
pub const KEY_F9: u16 = 67;
pub const KEY_F10: u16 = 68;
pub const KEY_F11: u16 = 87;
pub const KEY_F12: u16 = 88;
pub const KEY_LEFTMETA: u16 = 125;
pub const KEY_RIGHTMETA: u16 = 126;

pub const BTN_TABLE: [(&str, u16); 14] = [
    ("BTN_SOUTH", BTN_SOUTH),
    ("BTN_EAST", BTN_EAST),
    ("BTN_NORTH", BTN_NORTH),
    ("BTN_WEST", BTN_WEST),
    ("BTN_Z", BTN_Z),
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

pub const AXIS_TABLE: [(&str, u16); 14] = [
    ("ABS_X", ABS_X),
    ("ABS_Y", ABS_Y),
    ("ABS_Z", ABS_Z),
    ("ABS_RX", ABS_RX),
    ("ABS_RY", ABS_RY),
    ("ABS_RZ", ABS_RZ),
    ("ABS_GAS", ABS_GAS),
    ("ABS_BRAKE", ABS_BRAKE),
    ("ABS_HAT0X", ABS_HAT0X),
    ("ABS_HAT0Y", ABS_HAT0Y),
    ("ABS_HAT1X", ABS_HAT1X),
    ("ABS_HAT1Y", ABS_HAT1Y),
    ("ABS_MT_POSITION_X", ABS_MT_POSITION_X),
    ("ABS_MT_POSITION_Y", ABS_MT_POSITION_Y),
];

pub const KEY_ALIASES: [(&str, u16); 25] = [
    ("Ctrl", KEY_LEFTCTRL),
    ("Control", KEY_LEFTCTRL),
    ("Alt", KEY_LEFTALT),
    ("Shift", KEY_LEFTSHIFT),
    ("Tab", KEY_TAB),
    ("Space", KEY_SPACE),
    ("Espace", KEY_SPACE),
    ("Enter", KEY_ENTER),
    ("Return", KEY_ENTER),
    ("Esc", KEY_ESC),
    ("Escape", KEY_ESC),
    ("Meta", KEY_LEFTMETA),
    ("Super", KEY_LEFTMETA),
    ("F1", KEY_F1),
    ("F2", KEY_F2),
    ("F3", KEY_F3),
    ("F4", KEY_F4),
    ("F5", KEY_F5),
    ("F6", KEY_F6),
    ("F7", KEY_F7),
    ("F8", KEY_F8),
    ("F9", KEY_F9),
    ("F10", KEY_F10),
    ("F11", KEY_F11),
    ("F12", KEY_F12),
];

/// Noms courts utilisés dans les réglages (`Guide`, `A`, `LB`…).
pub const BUTTON_ALIASES: [(&str, u16); 18] = [
    ("A", BTN_SOUTH),
    ("B", BTN_EAST),
    ("X", BTN_NORTH),
    ("Y", BTN_WEST),
    ("LB", BTN_TL),
    ("RB", BTN_TR),
    ("L1", BTN_TL),
    ("R1", BTN_TR),
    ("LT", BTN_TL2),
    ("RT", BTN_TR2),
    ("L2", BTN_TL2),
    ("R2", BTN_TR2),
    ("SELECT", BTN_SELECT),
    ("BACK", BTN_SELECT),
    ("START", BTN_START),
    ("GUIDE", BTN_MODE),
    ("PS", BTN_MODE),
    ("HOME", BTN_MODE),
];

pub fn button_name(code: u16) -> Option<&'static str> {
    BTN_TABLE
        .iter()
        .find(|(_, c)| *c == code)
        .map(|(name, _)| *name)
}

pub fn button_code(name: &str) -> Option<u16> {
    let name = name.trim();
    let upper = name.to_ascii_uppercase();
    BUTTON_ALIASES
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(&upper))
        .or_else(|| BTN_TABLE.iter().find(|(n, _)| n.eq_ignore_ascii_case(&upper)))
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
    let name = name.trim();
    KEY_ALIASES
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, code)| *code)
}

/// Boutons qu'un nœud de manette peut exposer au-delà des boutons de façade :
/// joystick, gamepad, croix, palettes et boutons supplémentaires. Cette plage
/// exclut volontairement les boutons de souris et de pavé tactile.
pub fn is_pad_button(code: u16) -> bool {
    matches!(code, 0x120..=0x13e | 0x220..=0x22f | 0x2c0..=0x2e7)
}

/// Libellé court pour l'UI (A, B, X, Y, LB, RB, LT, RT…). Tout bouton de
/// manette reçoit un nom, même exotique : « Bouton 0x2e0 » vaut mieux qu'un
/// bouton invisible et intestable.
pub fn button_label(code: u16) -> String {
    let label = match code {
        BTN_SOUTH => "A",
        BTN_EAST => "B",
        BTN_NORTH => "X",
        BTN_WEST => "Y",
        BTN_Z => "Capture",
        BTN_TL => "LB",
        BTN_TR => "RB",
        BTN_TL2 => "LT",
        BTN_TR2 => "RT",
        BTN_SELECT => "Select",
        BTN_START => "Start",
        BTN_MODE => "Guide",
        BTN_THUMBL => "L3",
        BTN_THUMBR => "R3",
        BTN_C => "C",
        BTN_LEFT => "Clic gauche",
        BTN_RIGHT => "Clic droit",
        BTN_TOUCH => "Pavé tactile",
        BTN_BASE => "Accès rapide",
        BTN_THUMB => "Pad gauche",
        BTN_THUMB2 => "Pad droit",
        BTN_GRIPL => "Palette L",
        BTN_GRIPR => "Palette R",
        BTN_GRIPL2 => "Palette L2",
        BTN_GRIPR2 => "Palette R2",
        BTN_TRIGGER_HAPPY1 => "Palette 1",
        BTN_TRIGGER_HAPPY2 => "Palette 2",
        BTN_TRIGGER_HAPPY3 => "Palette 3",
        BTN_TRIGGER_HAPPY4 => "Palette 4",
        BTN_TRIGGER_HAPPY5 => "Bouton 5",
        BTN_TRIGGER_HAPPY6 => "Bouton 6",
        BTN_TRIGGER_HAPPY7 => "Bouton 7",
        BTN_TRIGGER_HAPPY8 => "Bouton 8",
        BTN_TRIGGER_HAPPY9 => "Bouton 9",
        BTN_TRIGGER_HAPPY10 => "Bouton 10",
        0x100..=0x109 => return format!("Bouton {}", code - 0x100),
        BTN_DPAD_UP => "Haut",
        BTN_DPAD_DOWN => "Bas",
        BTN_DPAD_LEFT => "Gauche",
        BTN_DPAD_RIGHT => "Droite",
        KEY_LEFTMETA | KEY_RIGHTMETA => "Meta",
        KEY_F1 => "F1",
        KEY_F12 => "F12",
        _ if is_pad_button(code) => return format!("Bouton 0x{code:03x}"),
        _ => "?",
    };
    label.to_string()
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
        assert_eq!(button_code(" guide "), Some(BTN_MODE));
        assert_eq!(button_code("LB"), Some(BTN_TL));
        assert_eq!(button_code("Start"), Some(BTN_START));
        assert_eq!(key_code("meta"), Some(KEY_LEFTMETA));
        assert_eq!(key_code("shift"), Some(KEY_LEFTSHIFT));
        assert_eq!(key_code("Tab"), Some(KEY_TAB));
        assert_eq!(key_code("F10"), Some(KEY_F10));
        assert_eq!(key_code("Ctrl"), Some(KEY_LEFTCTRL));
        assert_eq!(button_label(BTN_SOUTH), "A");
        assert_eq!(button_label(BTN_Z), "Capture");
        assert_eq!(button_label(0x999), "?");
        // Même un bouton inconnu d'une manette exotique garde un nom.
        assert_eq!(button_label(0x2e0), "Bouton 0x2e0");
        assert!(is_pad_button(BTN_GRIPL));
        assert!(is_pad_button(0x2c0));
        assert!(!is_pad_button(BTN_TOUCH));
        assert_eq!(button_label(BTN_BASE), "Accès rapide");
        assert_eq!(button_label(BTN_THUMB), "Pad gauche");
        assert_eq!(button_label(BTN_THUMB2), "Pad droit");
    }

    #[test]
    fn every_steam_controller_2026_button_is_named() {
        // Ensemble exposé par hid-steam pour la Steam Controller 2026 :
        // façade, gâchettes, croix, boutons système, clics de sticks, pavés
        // et palettes. Tous doivent apparaître dans le test en direct et
        // pouvoir servir de source à un remap.
        for code in [
            BTN_SOUTH,
            BTN_EAST,
            BTN_NORTH,
            BTN_WEST,
            BTN_TL,
            BTN_TR,
            BTN_TL2,
            BTN_TR2,
            BTN_SELECT,
            BTN_START,
            BTN_MODE,
            BTN_THUMBL,
            BTN_THUMBR,
            BTN_DPAD_UP,
            BTN_DPAD_DOWN,
            BTN_DPAD_LEFT,
            BTN_DPAD_RIGHT,
            BTN_GRIPL,
            BTN_GRIPR,
            BTN_GRIPL2,
            BTN_GRIPR2,
            BTN_BASE,
            BTN_THUMB,
            BTN_THUMB2,
        ] {
            assert!(
                is_pad_button(code),
                "0x{code:03x} doit rester un bouton de manette"
            );
            assert_ne!(button_label(code), "?", "0x{code:03x} doit avoir un libellé");
        }
    }
}
