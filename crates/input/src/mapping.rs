//! Moteur de mapping manettes : deadzones, courbes, remaps et raccourcis.

use crate::codes::{self, *};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Profile {
    pub name: String,
    /// Zone morte radiale des sticks (0.0 - 0.6).
    pub deadzone: f32,
    /// Courbe de réponse (>1 = plus de précision au centre).
    pub curve: f32,
    pub swap_ab: bool,
    pub invert_left_y: bool,
    pub invert_right_y: bool,
    /// Remap physique → virtuel (noms evdev : "BTN_SOUTH" → "BTN_EAST").
    pub remap: BTreeMap<String, String>,
    /// Bouton maintenant une couche alternative.
    pub shift_button: Option<String>,
    pub shift_remap: BTreeMap<String, String>,
    /// Raccourci manette d'ouverture de l'overlay, ex. ["Guide"] ou ["LB", "RB", "Start"].
    pub hotkey_chord: Vec<String>,
    /// Réglages gyroscopique (activé quand un périphérique IIO est disponible).
    pub gyro_to_right_stick: bool,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            name: "defaut".into(),
            deadzone: 0.12,
            curve: 1.6,
            swap_ab: false,
            invert_left_y: false,
            invert_right_y: false,
            remap: BTreeMap::new(),
            shift_button: None,
            shift_remap: BTreeMap::new(),
            hotkey_chord: vec!["Guide".into()],
            gyro_to_right_stick: false,
        }
    }
}

impl Profile {
    pub fn load(path: &std::path::Path) -> crate::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        let mut profile: Profile = toml::from_str(&text)?;
        profile.deadzone = profile.deadzone.clamp(0.0, 0.6);
        profile.curve = profile.curve.clamp(0.5, 4.0);
        Ok(profile)
    }

    pub fn save(&self, path: &std::path::Path) -> crate::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, toml::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn chord_codes(&self) -> Vec<u16> {
        self.hotkey_chord
            .iter()
            .filter_map(|name| codes::key_or_button_code(name))
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Key,
    Abs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VEvent {
    pub kind: Kind,
    pub code: u16,
    pub value: i32,
}

impl VEvent {
    pub fn key(code: u16, value: i32) -> Self {
        Self {
            kind: Kind::Key,
            code,
            value,
        }
    }

    pub fn abs(code: u16, value: i32) -> Self {
        Self {
            kind: Kind::Abs,
            code,
            value,
        }
    }
}

pub struct Mapper {
    profile: Profile,
    held: HashSet<u16>,
    steam_2026_dpad: HashSet<u16>,
    /// Sortie choisie à l'appui : une couche relâchée entre-temps ne doit
    /// pas libérer un autre bouton que celui qui a été pressé.
    pressed_outputs: BTreeMap<u16, u16>,
    shift_code: Option<u16>,
    hotkey_codes: Vec<u16>,
    hotkey_armed: bool,
    raw_axes: [i32; 6],
    /// Bornes des gâchettes du pilote natif 2026, si c'est cette manette.
    steam_2026_triggers: Option<[(i32, i32); 2]>,
}

impl Mapper {
    pub fn new(profile: Profile) -> Self {
        let shift_code = profile
            .shift_button
            .as_deref()
            .and_then(|name| codes::key_or_button_code(name));
        let hotkey_codes = profile.chord_codes();
        Self {
            profile,
            held: HashSet::new(),
            steam_2026_dpad: HashSet::new(),
            pressed_outputs: BTreeMap::new(),
            shift_code,
            hotkey_codes,
            hotkey_armed: false,
            raw_axes: [0; 6],
            steam_2026_triggers: None,
        }
    }

    /// Les bornes viennent d'evdev : certains pilotes exposent 0–32767.
    pub fn steam_2026(profile: Profile, left: (i32, i32), right: (i32, i32)) -> Self {
        let mut mapper = Self::new(profile);
        mapper.steam_2026_triggers = Some([left, right]);
        mapper
    }

    pub fn profile(&self) -> &Profile {
        &self.profile
    }

    pub fn hotkey_codes(&self) -> &[u16] {
        &self.hotkey_codes
    }

    /// Remplace le raccourci (utilisé quand seuls les boutons du pad comptent).
    pub fn set_hotkey_chord(&mut self, codes: Vec<u16>) {
        self.hotkey_codes = codes;
        self.hotkey_armed = false;
    }

    pub fn is_held(&self, code: u16) -> bool {
        self.held.contains(&code)
    }

    /// Efface l'état transitoire avant une nouvelle lecture de l'état evdev.
    pub(crate) fn reset(&mut self) {
        self.held.clear();
        self.steam_2026_dpad.clear();
        self.pressed_outputs.clear();
        self.hotkey_armed = false;
        self.raw_axes = [0; 6];
    }

    pub(crate) fn shift_code(&self) -> Option<u16> {
        self.shift_code
    }

    /// Traite une touche/bouton ; renvoie les événements virtuels et un éventuel
    /// déclenchement du raccourci overlay.
    pub fn on_key(&mut self, code: u16, value: i32) -> (Vec<VEvent>, bool) {
        if value == 0 {
            self.held.remove(&code);
        } else {
            self.held.insert(code);
        }

        let mut hotkey = false;
        if value != 0
            && !self.hotkey_codes.is_empty()
            && !self.hotkey_armed
            && self.hotkey_codes.iter().all(|c| self.held.contains(c))
        {
            self.hotkey_armed = true;
            hotkey = true;
        } else if value == 0 && self.hotkey_armed {
            self.hotkey_armed = false;
        }

        let shift_active = self
            .shift_code
            .map(|shift| self.held.contains(&shift))
            .unwrap_or(false);

        let physical_name = codes::button_name(code);
        let mut mapped_name: Option<&str> = None;
        if let Some(name) = physical_name {
            if shift_active {
                if let Some(target) = self.profile.shift_remap.get(name) {
                    mapped_name = Some(target.as_str());
                }
            }
            if mapped_name.is_none() {
                if let Some(target) = self.profile.remap.get(name) {
                    mapped_name = Some(target.as_str());
                }
            }
        }

        // Le pilote natif publie la croix en boutons, pas en axes HAT.
        // Un remap explicite conserve la priorité sur cette traduction.
        if self.steam_2026_triggers.is_some() {
            let axis = match code {
                BTN_DPAD_LEFT | BTN_DPAD_RIGHT => Some((ABS_HAT0X, BTN_DPAD_LEFT, BTN_DPAD_RIGHT)),
                BTN_DPAD_UP | BTN_DPAD_DOWN => Some((ABS_HAT0Y, BTN_DPAD_UP, BTN_DPAD_DOWN)),
                _ => None,
            };
            if let Some((axis, negative, positive)) = axis {
                let translated = self.steam_2026_dpad.contains(&code)
                    || (value != 0
                        && mapped_name.is_none()
                        && !self.pressed_outputs.contains_key(&code));
                if translated {
                    if value == 0 {
                        self.steam_2026_dpad.remove(&code);
                    } else {
                        self.steam_2026_dpad.insert(code);
                    }
                    let value = i32::from(self.steam_2026_dpad.contains(&positive))
                        - i32::from(self.steam_2026_dpad.contains(&negative));
                    return (vec![VEvent::abs(axis, value)], hotkey);
                }
            }
        }

        let mut virtual_code = match mapped_name {
            Some(name) => codes::key_or_button_code(name).unwrap_or(code),
            None => code,
        };
        if self.profile.swap_ab {
            virtual_code = match virtual_code {
                BTN_SOUTH => BTN_EAST,
                BTN_EAST => BTN_SOUTH,
                other => other,
            };
        }

        // La sortie active est fixée à l'appui, même si la couche change
        // avant le relâchement. Les répétitions conservent la même sortie.
        if value == 0 {
            let Some(pressed) = self.pressed_outputs.remove(&code) else {
                return (Vec::new(), hotkey);
            };
            virtual_code = pressed;
            if self.pressed_outputs.values().any(|other| *other == pressed) {
                return (Vec::new(), hotkey);
            }
        } else if let Some(pressed) = self.pressed_outputs.get(&code) {
            virtual_code = *pressed;
        } else if is_virtual_key(virtual_code) {
            let already_pressed = self
                .pressed_outputs
                .values()
                .any(|other| *other == virtual_code);
            self.pressed_outputs.insert(code, virtual_code);
            if already_pressed {
                return (Vec::new(), hotkey);
            }
        }
        if !is_virtual_key(virtual_code) {
            return (Vec::new(), hotkey);
        }
        (vec![VEvent::key(virtual_code, value)], hotkey)
    }

    /// Traite un axe ; renvoie les événements virtuels correspondants.
    pub fn on_abs(&mut self, code: u16, value: i32) -> Vec<VEvent> {
        if let Some(ranges) = self.steam_2026_triggers {
            // HAT0/1 sont les pavés tactiles, pas la croix directionnelle.
            if matches!(code, ABS_HAT0X | ABS_HAT0Y | ABS_HAT1X | ABS_HAT1Y) {
                return Vec::new();
            }
            let trigger = match code {
                ABS_HAT2Y => Some((ABS_Z, ranges[0])),
                ABS_HAT2X => Some((ABS_RZ, ranges[1])),
                _ => None,
            };
            if let Some((target, (min, max))) = trigger {
                let span = i64::from(max) - i64::from(min);
                if span <= 0 {
                    return Vec::new();
                }
                let scaled =
                    ((i64::from(value) - i64::from(min)).clamp(0, span) * 255 / span) as i32;
                return vec![VEvent::abs(target, scaled)];
            }
        }
        match code {
            ABS_X | ABS_Y => {
                self.raw_axes[if code == ABS_X { 0 } else { 1 }] = value;
                self.process_stick(false)
            }
            ABS_RX | ABS_RY => {
                self.raw_axes[if code == ABS_RX { 3 } else { 4 }] = value;
                self.process_stick(true)
            }
            ABS_Z | ABS_RZ => vec![VEvent::abs(code, value.clamp(0, 255))],
            ABS_HAT0X | ABS_HAT0Y => vec![VEvent::abs(code, value.clamp(-1, 1))],
            // Le pad Xbox virtuel ne déclare pas d'autres axes.
            _ => Vec::new(),
        }
    }

    fn process_stick(&self, right: bool) -> Vec<VEvent> {
        let (x_index, y_index, x_code, y_code) = if right {
            (3, 4, ABS_RX, ABS_RY)
        } else {
            (0, 1, ABS_X, ABS_Y)
        };
        let invert = if right {
            self.profile.invert_right_y
        } else {
            self.profile.invert_left_y
        };
        let x = self.raw_axes[x_index] as f32 / 32767.0;
        let y = self.raw_axes[y_index] as f32 / 32767.0;
        let (x, y) = apply_deadzone_curve(x, y, self.profile.deadzone, self.profile.curve);
        let y = if invert { -y } else { y };
        vec![
            VEvent::abs(x_code, (x * 32767.0).round() as i32),
            VEvent::abs(y_code, (y * 32767.0).round() as i32),
        ]
    }
}

/// Applique une zone morte radiale puis une courbe de réponse.
pub fn apply_deadzone_curve(x: f32, y: f32, deadzone: f32, curve: f32) -> (f32, f32) {
    let magnitude = (x * x + y * y).sqrt().min(1.0);
    if magnitude <= deadzone || magnitude == 0.0 {
        return (0.0, 0.0);
    }
    let normalized = ((magnitude - deadzone) / (1.0 - deadzone)).clamp(0.0, 1.0);
    let shaped = normalized.powf(curve.max(0.1));
    let scale = shaped / magnitude;
    ((x * scale).clamp(-1.0, 1.0), (y * scale).clamp(-1.0, 1.0))
}

/// Codes acceptés par le pad virtuel.
pub fn is_virtual_key(code: u16) -> bool {
    matches!(
        code,
        BTN_SOUTH
            | BTN_EAST
            | BTN_NORTH
            | BTN_WEST
            | BTN_TL
            | BTN_TR
            | BTN_TL2
            | BTN_TR2
            | BTN_SELECT
            | BTN_START
            | BTN_MODE
            | BTN_THUMBL
            | BTN_THUMBR
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mapper() -> Mapper {
        Mapper::new(Profile {
            deadzone: 0.1,
            curve: 1.0,
            ..Default::default()
        })
    }

    #[test]
    fn identity_mapping_emits_xbox_codes() {
        let mut mapper = mapper();
        let (events, hotkey) = mapper.on_key(BTN_SOUTH, 1);
        assert!(!hotkey);
        assert_eq!(events, vec![VEvent::key(BTN_SOUTH, 1)]);
    }

    #[test]
    fn swap_ab_swaps_buttons() {
        let mut mapper = Mapper::new(Profile {
            swap_ab: true,
            ..Default::default()
        });
        let (events, _) = mapper.on_key(BTN_SOUTH, 1);
        assert_eq!(events[0].code, BTN_EAST);
        let (events, _) = mapper.on_key(BTN_EAST, 1);
        assert_eq!(events[0].code, BTN_SOUTH);
        assert_eq!(
            mapper.on_key(BTN_EAST, 0).0,
            vec![VEvent::key(BTN_SOUTH, 0)]
        );
    }

    #[test]
    fn custom_remap_wins() {
        let mut profile = Profile::default();
        profile.remap.insert("BTN_SOUTH".into(), "BTN_NORTH".into());
        let mut mapper = Mapper::new(profile);
        let (events, _) = mapper.on_key(BTN_SOUTH, 1);
        assert_eq!(events[0].code, BTN_NORTH);
    }

    #[test]
    fn native_steam_2026_translates_triggers_dpad_and_not_trackpads() {
        let mut mapper = Mapper::steam_2026(Profile::default(), (0, 32767), (0, 32767));
        assert_eq!(mapper.on_abs(ABS_HAT2Y, 0), vec![VEvent::abs(ABS_Z, 0)]);
        assert_eq!(
            mapper.on_abs(ABS_HAT2Y, 16384),
            vec![VEvent::abs(ABS_Z, 127)]
        );
        assert_eq!(
            mapper.on_abs(ABS_HAT2X, 32767),
            vec![VEvent::abs(ABS_RZ, 255)]
        );
        assert!(mapper.on_abs(ABS_HAT0X, 20000).is_empty());
        assert!(mapper.on_abs(ABS_HAT1Y, -20000).is_empty());
        assert!(mapper.on_abs(ABS_GAS, 123).is_empty());
        assert_eq!(
            mapper.on_key(BTN_DPAD_LEFT, 1).0,
            vec![VEvent::abs(ABS_HAT0X, -1)]
        );
        assert_eq!(
            mapper.on_key(BTN_DPAD_RIGHT, 1).0,
            vec![VEvent::abs(ABS_HAT0X, 0)]
        );
        assert_eq!(
            mapper.on_key(BTN_DPAD_LEFT, 0).0,
            vec![VEvent::abs(ABS_HAT0X, 1)]
        );
        assert_eq!(
            mapper.on_key(BTN_DPAD_RIGHT, 0).0,
            vec![VEvent::abs(ABS_HAT0X, 0)]
        );
        assert_eq!(
            mapper.on_key(BTN_DPAD_UP, 1).0,
            vec![VEvent::abs(ABS_HAT0Y, -1)]
        );
        assert_eq!(
            mapper.on_key(BTN_DPAD_UP, 0).0,
            vec![VEvent::abs(ABS_HAT0Y, 0)]
        );
        assert_eq!(
            mapper.on_key(BTN_SOUTH, 1).0,
            vec![VEvent::key(BTN_SOUTH, 1)]
        );
        assert_eq!(mapper.on_abs(ABS_X, 0)[0].code, ABS_X);
        assert_eq!(mapper.on_abs(ABS_RX, 0)[0].code, ABS_RX);
    }

    #[test]
    fn native_steam_2026_respects_trigger_ranges_and_explicit_remaps() {
        let mut profile = Profile::default();
        profile.remap.insert("BTN_GRIPL".into(), "BTN_SOUTH".into());
        profile
            .remap
            .insert("BTN_DPAD_DOWN".into(), "BTN_EAST".into());
        let mut mapper = Mapper::steam_2026(profile, (100, 1100), (0, 0));
        assert_eq!(mapper.on_abs(ABS_HAT2Y, 600), vec![VEvent::abs(ABS_Z, 127)]);
        assert!(mapper.on_abs(ABS_HAT2X, 1).is_empty());
        assert_eq!(
            mapper.on_key(BTN_GRIPL, 1).0,
            vec![VEvent::key(BTN_SOUTH, 1)]
        );
        assert_eq!(
            mapper.on_key(BTN_DPAD_DOWN, 1).0,
            vec![VEvent::key(BTN_EAST, 1)]
        );
        assert_eq!(
            mapper.on_key(BTN_DPAD_UP, 1).0,
            vec![VEvent::abs(ABS_HAT0Y, -1)]
        );
        assert_eq!(
            mapper.on_key(BTN_DPAD_UP, 0).0,
            vec![VEvent::abs(ABS_HAT0Y, 0)]
        );
        assert_eq!(
            mapper.on_key(BTN_DPAD_DOWN, 0).0,
            vec![VEvent::key(BTN_EAST, 0)]
        );
    }

    #[test]
    fn palette_buttons_are_remappable_by_evdev_name() {
        let mut profile = Profile::default();
        profile
            .remap
            .insert("BTN_TRIGGER_HAPPY1".into(), "BTN_SOUTH".into());
        let mut mapper = Mapper::new(profile);
        let (events, _) = mapper.on_key(BTN_TRIGGER_HAPPY1, 1);
        assert_eq!(events, vec![VEvent::key(BTN_SOUTH, 1)]);
    }

    #[test]
    fn shift_layer_applies_while_held() {
        let mut profile = Profile::default();
        profile.shift_button = Some("BTN_TL2".into());
        profile
            .shift_remap
            .insert("BTN_SOUTH".into(), "BTN_START".into());
        let mut mapper = Mapper::new(profile);
        let (events, _) = mapper.on_key(BTN_SOUTH, 1);
        assert_eq!(events[0].code, BTN_SOUTH);
        assert_eq!(
            mapper.on_key(BTN_SOUTH, 0).0,
            vec![VEvent::key(BTN_SOUTH, 0)]
        );
        let _ = mapper.on_key(BTN_TL2, 1);
        let (events, _) = mapper.on_key(BTN_SOUTH, 1);
        assert_eq!(events[0].code, BTN_START);
        assert_eq!(
            mapper.on_key(BTN_SOUTH, 0).0,
            vec![VEvent::key(BTN_START, 0)]
        );
        let _ = mapper.on_key(BTN_TL2, 0);
        let (events, _) = mapper.on_key(BTN_SOUTH, 1);
        assert_eq!(events[0].code, BTN_SOUTH);
    }

    #[test]
    fn shift_release_does_not_leave_the_mapped_button_pressed() {
        let mut profile = Profile::default();
        profile.shift_button = Some("BTN_TL2".into());
        profile
            .shift_remap
            .insert("BTN_SOUTH".into(), "BTN_START".into());
        let mut mapper = Mapper::new(profile);
        mapper.on_key(BTN_TL2, 1);
        assert_eq!(
            mapper.on_key(BTN_SOUTH, 1).0,
            vec![VEvent::key(BTN_START, 1)]
        );
        mapper.on_key(BTN_TL2, 0);
        assert_eq!(
            mapper.on_key(BTN_SOUTH, 0).0,
            vec![VEvent::key(BTN_START, 0)]
        );
    }

    #[test]
    fn two_physical_buttons_can_hold_the_same_virtual_button() {
        let mut profile = Profile::default();
        profile.remap.insert("BTN_GRIPL".into(), "BTN_SOUTH".into());
        let mut mapper = Mapper::steam_2026(profile, (0, 32767), (0, 32767));
        assert_eq!(
            mapper.on_key(BTN_SOUTH, 1).0,
            vec![VEvent::key(BTN_SOUTH, 1)]
        );
        assert!(mapper.on_key(BTN_GRIPL, 1).0.is_empty());
        assert!(mapper.on_key(BTN_SOUTH, 0).0.is_empty());
        assert_eq!(
            mapper.on_key(BTN_GRIPL, 0).0,
            vec![VEvent::key(BTN_SOUTH, 0)]
        );
    }

    #[test]
    fn release_without_an_observed_press_does_not_release_another_mapping() {
        let mut profile = Profile::default();
        profile.remap.insert("BTN_GRIPL".into(), "BTN_SOUTH".into());
        let mut mapper = Mapper::steam_2026(profile, (0, 32767), (0, 32767));
        assert_eq!(
            mapper.on_key(BTN_SOUTH, 1).0,
            vec![VEvent::key(BTN_SOUTH, 1)]
        );
        assert!(mapper.on_key(BTN_GRIPL, 0).0.is_empty());
        assert_eq!(
            mapper.on_key(BTN_SOUTH, 0).0,
            vec![VEvent::key(BTN_SOUTH, 0)]
        );
    }

    #[test]
    fn native_dpad_release_respects_the_layer_used_on_press() {
        let mut profile = Profile::default();
        profile.shift_button = Some("BTN_TL".into());
        profile
            .shift_remap
            .insert("BTN_DPAD_LEFT".into(), "BTN_SOUTH".into());
        let mut mapper = Mapper::steam_2026(profile, (0, 32767), (0, 32767));
        assert_eq!(
            mapper.on_key(BTN_DPAD_LEFT, 1).0,
            vec![VEvent::abs(ABS_HAT0X, -1)]
        );
        mapper.on_key(BTN_TL, 1);
        assert_eq!(
            mapper.on_key(BTN_DPAD_LEFT, 0).0,
            vec![VEvent::abs(ABS_HAT0X, 0)]
        );
        assert_eq!(
            mapper.on_key(BTN_DPAD_LEFT, 1).0,
            vec![VEvent::key(BTN_SOUTH, 1)]
        );
        mapper.on_key(BTN_TL, 0);
        assert_eq!(
            mapper.on_key(BTN_DPAD_LEFT, 0).0,
            vec![VEvent::key(BTN_SOUTH, 0)]
        );
    }

    #[test]
    fn guide_button_triggers_once_until_release() {
        let mut mapper = mapper();
        let (_, hotkey) = mapper.on_key(BTN_MODE, 1);
        assert!(hotkey);
        let (_, hotkey) = mapper.on_key(BTN_MODE, 1);
        assert!(!hotkey);
        let _ = mapper.on_key(BTN_MODE, 0);
        let (_, hotkey) = mapper.on_key(BTN_MODE, 1);
        assert!(hotkey);
    }

    #[test]
    fn chord_triggers_once_until_release() {
        let mut mapper = Mapper::new(Profile {
            hotkey_chord: vec!["LB".into(), "RB".into(), "Start".into()],
            ..Default::default()
        });
        let _ = mapper.on_key(BTN_TL, 1);
        let _ = mapper.on_key(BTN_TR, 1);
        let (_, hotkey) = mapper.on_key(BTN_START, 1);
        assert!(hotkey);
        let (_, hotkey) = mapper.on_key(BTN_START, 1);
        assert!(!hotkey);
        let _ = mapper.on_key(BTN_START, 0);
        let _ = mapper.on_key(BTN_TR, 0);
        let _ = mapper.on_key(BTN_TR, 1);
        let (_, hotkey) = mapper.on_key(BTN_START, 1);
        assert!(hotkey);
    }

    #[test]
    fn deadzone_zeroes_small_input() {
        assert_eq!(apply_deadzone_curve(0.02, 0.02, 0.12, 1.6), (0.0, 0.0));
    }

    #[test]
    fn deadzone_preserves_direction_and_range() {
        let (x, y) = apply_deadzone_curve(1.0, 0.0, 0.12, 1.0);
        assert!((x - 1.0).abs() < 0.001);
        assert_eq!(y, 0.0);
        let (x, y) = apply_deadzone_curve(0.5, 0.0, 0.0, 1.0);
        assert!((x - 0.5).abs() < 0.001);
        assert_eq!(y, 0.0);
    }

    #[test]
    fn curve_reduces_midrange_precision() {
        let (x, _) = apply_deadzone_curve(0.5, 0.0, 0.0, 2.0);
        assert!(x < 0.3, "valeur attendue plus basse, obtenu {x}");
    }

    #[test]
    fn stick_events_are_recomputed() {
        let mut mapper = Mapper::new(Profile {
            deadzone: 0.0,
            curve: 1.0,
            ..Default::default()
        });
        let events = mapper.on_abs(ABS_X, 32767);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].code, ABS_X);
        assert_eq!(events[0].value, 32767);
        let events = mapper.on_abs(ABS_Y, -32767);
        assert_eq!(events[1].code, ABS_Y);
        assert!(events[1].value < 0);
    }

    #[test]
    fn invert_right_y() {
        let mut mapper = Mapper::new(Profile {
            deadzone: 0.0,
            curve: 1.0,
            invert_right_y: true,
            ..Default::default()
        });
        let events = mapper.on_abs(ABS_RY, 1000);
        assert!(events[1].value < 0);
    }

    #[test]
    fn triggers_are_clamped() {
        let mut mapper = mapper();
        let events = mapper.on_abs(ABS_Z, 999);
        assert_eq!(events[0].value, 255);
        let events = mapper.on_abs(ABS_Z, -5);
        assert_eq!(events[0].value, 0);
    }
}
