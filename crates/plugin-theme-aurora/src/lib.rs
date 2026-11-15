//! Thème natif « Aurora ».
//!
//! Le thème ne se contente pas de couleurs : il dessine lui-même les halos de
//! fond de l'application avec des commandes génériques. Lapinux ne connaît ni
//! la palette Aurora ni la forme de ses halos.

use lapinux_plugin_api::{
    HostApi, PageCommand, PluginDescriptor, ThemeColors, ThemeSpec, HOST_ABI_VERSION,
};
use std::ffi::{c_char, CString};
use std::sync::Mutex;

static SPEC: Mutex<Option<CString>> = Mutex::new(None);
static FRAME: Mutex<Option<CString>> = Mutex::new(None);

const COLORS: ThemeColors = ThemeColors {
    background: [12, 15, 32, 255],
    panel: [18, 22, 44, 255],
    card: [25, 30, 58, 255],
    card_hover: [35, 42, 78, 255],
    card_active: [45, 55, 96, 255],
    border: [55, 68, 112, 255],
    accent: [116, 236, 214, 255],
    text: [236, 240, 255, 255],
    text_dim: [154, 165, 200, 255],
};

unsafe extern "C" fn init(api: *const HostApi) -> bool {
    if api.is_null() {
        return false;
    }
    // SAFETY: l'hôte passe une API valide pendant l'initialisation.
    unsafe { (*api).is_compatible() }
}

unsafe extern "C" fn theme(_api: *const HostApi) -> *const c_char {
    let spec = ThemeSpec {
        colors: COLORS,
        rounding: 18.0,
        font_scale: 1.0,
    };
    let json = serde_json::to_string(&spec).unwrap_or_else(|_| "{}".to_string());
    let Ok(json) = CString::new(json) else {
        return std::ptr::null();
    };
    let Ok(mut slot) = SPEC.lock() else {
        return std::ptr::null();
    };
    *slot = Some(json);
    slot.as_ref()
        .map(|value| value.as_ptr())
        .unwrap_or(std::ptr::null())
}

unsafe extern "C" fn theme_frame(
    _api: *const HostApi,
    width: f32,
    height: f32,
    time: f64,
) -> *const c_char {
    let width = width.max(1.0);
    let height = height.max(1.0);
    let now = time as f32;
    let mut commands = Vec::with_capacity(7);
    commands.push(PageCommand::Rect {
        x: 0.0,
        y: 0.0,
        width,
        height,
        rounding: 0.0,
        color: COLORS.background,
    });
    // Trois halos lents, propres au thème : leur géométrie et leur mouvement
    // appartiennent au plugin, pas au shell Lapinux.
    for (index, (base_x, base_y, radius, alpha)) in [
        (0.18, 0.20, 0.34, 34),
        (0.82, 0.28, 0.28, 26),
        (0.55, 0.88, 0.40, 22),
    ]
    .into_iter()
    .enumerate()
    {
        let drift = (now * (0.12 + index as f32 * 0.03) + index as f32 * 2.1).sin();
        commands.push(PageCommand::Circle {
            x: width * base_x + drift * width * 0.025,
            y: height * base_y + drift * height * 0.04,
            radius: width.min(height) * radius,
            color: [COLORS.accent[0], COLORS.accent[1], COLORS.accent[2], alpha],
        });
    }
    let json = serde_json::to_string(&commands).unwrap_or_else(|_| "[]".to_string());
    let Ok(json) = CString::new(json) else {
        return std::ptr::null();
    };
    let Ok(mut slot) = FRAME.lock() else {
        return std::ptr::null();
    };
    *slot = Some(json);
    slot.as_ref()
        .map(|value| value.as_ptr())
        .unwrap_or(std::ptr::null())
}

#[no_mangle]
pub extern "C" fn lapinux_plugin_entry() -> *const PluginDescriptor {
    Box::into_raw(Box::new(PluginDescriptor {
        abi_version: HOST_ABI_VERSION,
        init: Some(init),
        draw: None,
        page: None,
        theme: Some(theme),
        theme_frame: Some(theme_frame),
        home: None,
    }))
}
