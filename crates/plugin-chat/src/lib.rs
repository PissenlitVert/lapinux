//! Plugin natif « chat rain ».
//!
//! Il ne dépend d'aucun rendu de Lapinux : à chaque image il calcule sa pluie
//! de mots et renvoie des commandes de dessin génériques à l'hôte.

use lapinux_plugin_api::{HostApi, PageCommand, PageTextAlign, PluginDescriptor, HOST_ABI_VERSION};
use std::ffi::{c_char, CString};
use std::sync::Mutex;

static FRAME: Mutex<Option<CString>> = Mutex::new(None);

unsafe extern "C" fn init(api: *const HostApi) -> bool {
    if api.is_null() {
        return false;
    }
    // SAFETY: l'hôte passe une API valide pendant l'initialisation.
    unsafe { (*api).is_compatible() }
}

unsafe extern "C" fn page(
    api: *const HostApi,
    width: f32,
    height: f32,
    time: f64,
) -> *const c_char {
    if api.is_null() {
        return std::ptr::null();
    }
    let width = width.max(1.0);
    let height = height.max(1.0);
    let now = time as f32;
    // SAFETY: le chargeur garde l'API vivante pendant le rendu.
    let text = unsafe { &*api }.page().unwrap_or_default();
    let word = if text.word.trim().is_empty() {
        text.title.as_str()
    } else {
        text.word.as_str()
    };
    let mut commands = Vec::with_capacity(56);
    let colors = &text.colors;
    let ink = colors.text;
    let dim = colors.text_dim;
    let accent = colors.accent;

    commands.push(PageCommand::Rect {
        x: 0.0,
        y: 0.0,
        width,
        height,
        rounding: 0.0,
        color: colors.background,
    });

    if !text.title.trim().is_empty() {
        commands.push(PageCommand::Text {
            x: width * 0.5,
            y: 34.0,
            text: text.title.clone(),
            size: 34.0,
            color: ink,
            align: PageTextAlign::Center,
        });
    }
    if !text.subtitle.trim().is_empty() {
        commands.push(PageCommand::Text {
            x: width * 0.5,
            y: 62.0,
            text: text.subtitle.clone(),
            size: 12.0,
            color: dim,
            align: PageTextAlign::Center,
        });
    }

    // La pluie est calculée par le plugin, pas par Lapinux : le nombre, le
    // mot, les vitesses et les couleurs appartiennent à ce fichier.
    for index in 0..52 {
        let seed = index as f32;
        let speed = 20.0 + (seed * 17.0) % 34.0;
        let x = ((seed * 97.0).sin() * 0.5 + 0.5) * width;
        let start = ((seed * 53.0).sin() * 0.5 + 0.5) * (height + 100.0);
        let y = -55.0 + ((start + now * speed) % (height + 100.0));
        let sway = (now * 1.15 + seed * 0.9).sin() * 13.0;
        let size = 10.0 + (seed * 7.0) % 13.0;
        let alpha = (55 + (seed * 31.0) as u8 % 125) as u8;
        commands.push(PageCommand::Text {
            x: x + sway,
            y,
            text: word.to_string(),
            size,
            color: [accent[0], accent[1], accent[2], alpha],
            align: PageTextAlign::Center,
        });
    }

    if !text.footer.trim().is_empty() {
        commands.push(PageCommand::Text {
            x: width * 0.5,
            y: height - 24.0,
            text: text.footer.clone(),
            size: 18.0,
            color: dim,
            align: PageTextAlign::Center,
        });
    }

    let json = serde_json::to_string(&commands).unwrap_or_else(|_| "[]".to_string());
    let Ok(json) = CString::new(json) else {
        return std::ptr::null();
    };
    let Ok(mut frame) = FRAME.lock() else {
        return std::ptr::null();
    };
    *frame = Some(json);
    frame
        .as_ref()
        .map(|frame| frame.as_ptr())
        .unwrap_or(std::ptr::null())
}

#[no_mangle]
pub extern "C" fn lapinux_plugin_entry() -> *const PluginDescriptor {
    Box::into_raw(Box::new(PluginDescriptor {
        abi_version: HOST_ABI_VERSION,
        init: Some(init),
        draw: None,
        page: Some(page),
        theme: None,
        theme_frame: None,
        home: None,
    }))
}
