//! Page personnelle des jeux Steam Famille connus de Lapinux.
//! Le plugin n'accède ni au client Steam ni à ses fichiers et n'invente aucune
//! licence : il affiche les données publiées par l'hôte et ouvre ses fiches.

use lapinux_plugin_api::{
    HostAction, HostApi, LibraryGame, PageCommand, PageInput, PageTextAlign, PluginDescriptor,
    HOST_ABI_VERSION,
};
use std::ffi::{c_char, CString};
use std::sync::Mutex;

static FRAME: Mutex<Option<CString>> = Mutex::new(None);
static OFFSET: Mutex<usize> = Mutex::new(0);

fn family_games(mut games: Vec<LibraryGame>) -> Vec<LibraryGame> {
    games.retain(|game| game.family || game.family_available);
    games.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then(a.appid.cmp(&b.appid))
    });
    games
}

fn text(x: f32, y: f32, value: impl Into<String>, size: f32, color: [u8; 4]) -> PageCommand {
    PageCommand::Text {
        x,
        y,
        text: value.into(),
        size,
        color,
        align: PageTextAlign::Left,
    }
}

fn inside(mouse: Option<[f32; 2]>, x: f32, y: f32, width: f32, height: f32) -> bool {
    mouse.is_some_and(|[mx, my]| mx >= x && mx <= x + width && my >= y && my <= y + height)
}

fn render(api: &HostApi, input: &PageInput, width: f32, height: f32) -> Vec<PageCommand> {
    let page = api.page().unwrap_or_default();
    let colors = &page.colors;
    let mut commands = vec![
        PageCommand::Rect {
            x: 0.0,
            y: 0.0,
            width,
            height,
            rounding: 0.0,
            color: colors.background,
        },
        text(24.0, 30.0, "Steam Famille", 25.0, colors.text),
        text(
            24.0,
            58.0,
            "Jeux partagés connus de Lapinux · droits vérifiés par Steam",
            13.0,
            colors.text_dim,
        ),
    ];
    let games = match api.library() {
        Ok(games) => family_games(games),
        Err(_) => {
            commands.push(text(
                24.0,
                110.0,
                "Bibliothèque Lapinux indisponible.",
                15.0,
                colors.text,
            ));
            return commands;
        }
    };
    if games.is_empty() {
        commands.push(text(
            24.0,
            110.0,
            "Aucun jeu familial connu. Connecte ton compte Steam et actualise la bibliothèque.",
            14.0,
            colors.text_dim,
        ));
        return commands;
    }
    let row_height = 42.0;
    let top = 106.0;
    let visible = (((height - top - 48.0).max(row_height) / row_height) as usize).max(1);
    let max_offset = games.len().saturating_sub(visible);
    let mut offset = OFFSET.lock().unwrap_or_else(|err| err.into_inner());
    *offset = (*offset).min(max_offset);
    if input.scroll < 0.0 || input.keys.iter().any(|key| key == "down") {
        *offset = offset.saturating_add(1).min(max_offset);
    } else if input.scroll > 0.0 || input.keys.iter().any(|key| key == "up") {
        *offset = offset.saturating_sub(1);
    }
    commands.push(text(
        24.0,
        84.0,
        format!("{} jeu(x) · molette ou flèches pour parcourir", games.len()),
        12.0,
        colors.text_dim,
    ));
    let row_width = (width - 48.0).max(1.0);
    for (row, game) in games.iter().skip(*offset).take(visible).enumerate() {
        let y = top + row as f32 * row_height;
        let hovered = inside(input.mouse, 24.0, y, row_width, row_height - 4.0);
        commands.push(PageCommand::Rect {
            x: 24.0,
            y,
            width: row_width,
            height: row_height - 4.0,
            rounding: 5.0,
            color: if hovered {
                colors.card_hover
            } else {
                colors.card
            },
        });
        let name: String = if game.name.chars().count() > 60 {
            format!("{}…", game.name.chars().take(59).collect::<String>())
        } else {
            game.name.clone()
        };
        commands.push(text(34.0, y + 19.0, name, 14.0, colors.text));
        if input.clicked && hovered {
            let _ = api.dispatch(&HostAction::OpenGame { appid: game.appid });
        }
    }
    commands
}

unsafe extern "C" fn init(api: *const HostApi) -> bool {
    !api.is_null() && unsafe { (*api).is_compatible() }
}

unsafe extern "C" fn page(
    api: *const HostApi,
    width: f32,
    height: f32,
    _time: f64,
) -> *const c_char {
    if api.is_null() {
        return std::ptr::null();
    }
    let api = unsafe { &*api };
    let input = api.input().unwrap_or_default();
    let commands = render(api, &input, width.max(1.0), height.max(1.0));
    let Ok(json) = serde_json::to_string(&commands) else {
        return std::ptr::null();
    };
    let Ok(json) = CString::new(json) else {
        return std::ptr::null();
    };
    let Ok(mut frame) = FRAME.lock() else {
        return std::ptr::null();
    };
    *frame = Some(json);
    frame
        .as_ref()
        .map_or(std::ptr::null(), |frame| frame.as_ptr())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_family_games_are_shown_and_sorted() {
        let games = vec![
            LibraryGame {
                appid: 1,
                name: "Zulu".into(),
                family: true,
                ..Default::default()
            },
            LibraryGame {
                appid: 2,
                name: "Mine".into(),
                steam: true,
                ..Default::default()
            },
            LibraryGame {
                appid: 3,
                name: "Alpha".into(),
                family_available: true,
                ..Default::default()
            },
        ];
        let ids: Vec<_> = family_games(games)
            .into_iter()
            .map(|game| game.appid)
            .collect();
        assert_eq!(ids, vec![3, 1]);
    }

    #[test]
    fn clicking_a_shared_game_requests_its_existing_sheet() {
        use std::ffi::{c_char, c_void, CStr};
        struct Mock {
            actions: Vec<HostAction>,
        }
        unsafe extern "C" fn library(_: *mut c_void) -> *mut c_char {
            CString::new(r#"[{"appid":42,"name":"Jeu familial","family":true}]"#)
                .unwrap()
                .into_raw()
        }
        unsafe extern "C" fn free(_: *mut c_void, value: *mut c_char) {
            if !value.is_null() {
                drop(unsafe { CString::from_raw(value) });
            }
        }
        unsafe extern "C" fn action(context: *mut c_void, json: *const c_char) -> u32 {
            let mock = unsafe { &mut *(context as *mut Mock) };
            let text = unsafe { CStr::from_ptr(json) }.to_str().unwrap();
            mock.actions.push(serde_json::from_str(text).unwrap());
            1
        }
        let mut mock = Mock {
            actions: Vec::new(),
        };
        let api = HostApi {
            abi_version: HOST_ABI_VERSION,
            struct_size: std::mem::size_of::<HostApi>(),
            context: (&mut mock as *mut Mock).cast(),
            library_json: Some(library),
            action: Some(action),
            free_string: Some(free),
            page_json: None,
            input_json: None,
        };
        let input = PageInput {
            mouse: Some([30.0, 110.0]),
            clicked: true,
            ..Default::default()
        };
        let commands = render(&api, &input, 800.0, 600.0);
        assert!(commands.iter().any(
            |command| matches!(command, PageCommand::Text { text, .. } if text == "Jeu familial")
        ));
        assert_eq!(mock.actions, vec![HostAction::OpenGame { appid: 42 }]);
    }

    #[test]
    fn hit_testing_uses_page_coordinates() {
        assert!(inside(Some([30.0, 110.0]), 24.0, 106.0, 200.0, 38.0));
        assert!(!inside(Some([10.0, 110.0]), 24.0, 106.0, 200.0, 38.0));
        assert!(!inside(None, 24.0, 106.0, 200.0, 38.0));
    }
}
