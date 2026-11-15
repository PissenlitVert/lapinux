//! Pad virtuel Xbox exposé au système via /dev/uinput.

use crate::codes::*;
use crate::mapping::{Kind, VEvent};
use crate::{Error, Result};
use evdev::uinput::VirtualDevice;
use evdev::{
    AbsInfo, AbsoluteAxisCode, AttributeSet, BusType, InputEvent, InputId, KeyCode, UinputAbsSetup,
};

const VIRTUAL_KEYS: [u16; 13] = [
    BTN_SOUTH, BTN_EAST, BTN_NORTH, BTN_WEST, BTN_TL, BTN_TR, BTN_TL2, BTN_TR2, BTN_SELECT,
    BTN_START, BTN_MODE, BTN_THUMBL, BTN_THUMBR,
];

const VIRTUAL_AXES: [(u16, i32, i32); 8] = [
    (ABS_X, -32768, 32767),
    (ABS_Y, -32768, 32767),
    (ABS_RX, -32768, 32767),
    (ABS_RY, -32768, 32767),
    (ABS_Z, 0, 255),
    (ABS_RZ, 0, 255),
    (ABS_HAT0X, -1, 1),
    (ABS_HAT0Y, -1, 1),
];

pub struct VirtualPad {
    device: VirtualDevice,
}

impl VirtualPad {
    pub fn create(name: &str) -> Result<Self> {
        let mut keys = AttributeSet::<KeyCode>::new();
        for code in VIRTUAL_KEYS {
            keys.insert(KeyCode::new(code));
        }
        let mut builder = VirtualDevice::builder()
            .map_err(|e| Error::Other(format!("uinput indisponible : {e}")))?
            .name(name)
            // Le pad doit être reconnu comme une manette Xbox 360, pas comme
            // un joystick anonyme : Wine/XInput se sert de ces identifiants
            // pour créer le slot XInput attendu par le jeu.
            .input_id(InputId::new(BusType::BUS_USB, 0x045e, 0x028e, 0x0110))
            .with_keys(&keys)
            .map_err(|e| Error::Other(format!("uinput keys : {e}")))?;
        for (code, min, max) in VIRTUAL_AXES {
            let setup =
                UinputAbsSetup::new(AbsoluteAxisCode(code), AbsInfo::new(0, min, max, 0, 0, 0));
            builder = builder
                .with_absolute_axis(&setup)
                .map_err(|e| Error::Other(format!("uinput axe {code} : {e}")))?;
        }
        let device = builder
            .build()
            .map_err(|e| Error::Other(format!("création du pad virtuel : {e}")))?;
        Ok(Self { device })
    }

    /// Émet les événements puis un SYN_REPORT (obligatoire par lot).
    pub fn emit(&mut self, events: &[VEvent]) -> Result<()> {
        if events.is_empty() {
            return Ok(());
        }
        let mut raw: Vec<InputEvent> = events
            .iter()
            .map(|event| match event.kind {
                Kind::Key => InputEvent::new(EV_KEY, event.code, event.value),
                Kind::Abs => InputEvent::new(EV_ABS, event.code, event.value),
            })
            .collect();
        raw.push(InputEvent::new(EV_SYN, 0, 0));
        self.device
            .emit(&raw)
            .map_err(|e| Error::Other(format!("écriture uinput : {e}")))
    }

    pub fn name(&self) -> String {
        format!("{} Virtual Pad", lapinux_core::APP_NAME)
    }
}
