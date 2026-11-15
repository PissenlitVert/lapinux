//! Thème natif « NX ».
//!
//! L'accueil NX est entièrement dessiné par ce plugin : carrousel de jaquettes,
//! sélection, dock, réglages et extinction. Lapinux lui fournit la
//! bibliothèque, les libellés de navigation et les entrées clavier/souris ; il
//! ne connaît aucune disposition NX.

use lapinux_plugin_api::{
    HostAction, HostApi, LibraryGame, PageCommand, PageInput, PageTextAlign, PluginDescriptor,
    ThemeColors, ThemeSpec, HOST_ABI_VERSION,
};
use std::ffi::{c_char, CString};
use std::sync::Mutex;

static SPEC: Mutex<Option<CString>> = Mutex::new(None);
static FRAME: Mutex<Option<CString>> = Mutex::new(None);
static FOCUS: Mutex<u32> = Mutex::new(0);

const COLORS: ThemeColors = ThemeColors {
    background: [28, 28, 30, 255],
    panel: [43, 43, 46, 255],
    card: [57, 57, 61, 255],
    card_hover: [70, 70, 75, 255],
    card_active: [82, 83, 89, 255],
    border: [73, 74, 79, 255],
    accent: [0, 200, 224, 255],
    text: [244, 245, 247, 255],
    text_dim: [166, 169, 176, 255],
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
        rounding: 12.0,
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
    _time: f64,
) -> *const c_char {
    let width = width.max(1.0);
    let height = height.max(1.0);
    // Le fond du thème reste neutre : la décoration de l'accueil NX est
    // dessinée par `home`, pas par toutes les pages de l'application.
    let commands = vec![PageCommand::Rect {
        x: 0.0,
        y: 0.0,
        width,
        height,
        rounding: 0.0,
        color: COLORS.background,
    }];
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

fn clock(now: i64) -> String {
    let seconds = now.rem_euclid(86_400);
    format!("{:02}:{:02}", seconds / 3_600, (seconds % 3_600) / 60)
}

fn rect(x: f32, y: f32, width: f32, height: f32, rounding: f32, color: [u8; 4]) -> PageCommand {
    PageCommand::Rect {
        x,
        y,
        width,
        height,
        rounding,
        color,
    }
}

fn text(x: f32, y: f32, value: impl Into<String>, size: f32, color: [u8; 4]) -> PageCommand {
    PageCommand::Text {
        x,
        y,
        text: value.into(),
        size,
        color,
        align: PageTextAlign::Center,
    }
}

fn activate(api: &HostApi, appid: u32, installed: bool) {
    if installed {
        let _ = api.launch_game(appid, false);
    } else {
        let _ = api.dispatch(&HostAction::OpenGame { appid });
    }
}

/// Retrouve la carte sélectionnée par son appid. `FOCUS` garde un appid pour
/// que la sélection survive à un réordonnancement de la bibliothèque ; il ne
/// faut donc jamais l'utiliser directement comme un index.
fn focus_index(games: &[LibraryGame], appid: u32) -> usize {
    games
        .iter()
        .position(|game| game.appid == appid)
        .unwrap_or(0)
}

unsafe extern "C" fn home(
    api: *const HostApi,
    width: f32,
    height: f32,
    _time: f64,
) -> *const c_char {
    if api.is_null() {
        return std::ptr::null();
    }
    // SAFETY: le chargeur garde l'API vivante pendant le rendu.
    let api = unsafe { &*api };
    let input = api.input().unwrap_or_else(|_| PageInput {
        width,
        height,
        ..PageInput::default()
    });
    let width = width.max(1.0);
    let height = height.max(1.0);
    let mut games = api.library().unwrap_or_default();
    games.sort_by(|left, right| {
        right
            .last_played
            .cmp(&left.last_played)
            .then_with(|| right.installed.cmp(&left.installed))
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });

    let margin = 34.0;
    let dock_y = height - 56.0;
    let mut commands = Vec::with_capacity(96);
    commands.push(rect(0.0, 0.0, width, height, 0.0, COLORS.background));

    // Bandeau supérieur : deux profils et l'horloge, comme une console.
    commands.push(PageCommand::Circle {
        x: margin + 23.0,
        y: 34.0,
        radius: 23.0,
        color: [224, 122, 47, 255],
    });
    commands.push(PageCommand::Circle {
        x: margin + 76.0,
        y: 34.0,
        radius: 19.0,
        color: [47, 127, 193, 255],
    });
    commands.push(PageCommand::Text {
        x: width - margin,
        y: 34.0,
        text: clock(input.now),
        size: 24.0,
        color: COLORS.text,
        align: PageTextAlign::Right,
    });

    // Dock : les pages visibles sont fournies par l'hôte, le plugin décide de
    // leur ordre horizontal et de leur apparence.
    let nav = &input.nav;
    let item_w = 58.0;
    let item_gap = 12.0;
    let item_count = nav.len() + 2;
    let dock_w = item_count as f32 * item_w + item_count.saturating_sub(1) as f32 * item_gap;
    let mut item_x = width * 0.5 - dock_w * 0.5 + item_w * 0.5;
    let panel_w = dock_w + 44.0;
    commands.push(rect(
        width * 0.5 - panel_w * 0.5 - 3.0,
        dock_y - 38.0,
        panel_w + 6.0,
        76.0,
        38.0,
        [0, 0, 0, 30],
    ));
    commands.push(rect(
        width * 0.5 - panel_w * 0.5,
        dock_y - 35.0,
        panel_w,
        70.0,
        35.0,
        [248, 248, 248, 238],
    ));
    let dock_ink = [30, 30, 33, 255];
    let mut nav_rects = Vec::with_capacity(nav.len());
    for entry in nav {
        let x = item_x - item_w * 0.5;
        nav_rects.push((x, dock_y - item_w * 0.5, item_w, item_w, entry.key.clone()));
        commands.push(PageCommand::NavIcon {
            page: entry.key.clone(),
            x: item_x,
            y: dock_y,
            size: 26.0,
            color: dock_ink,
        });
        item_x += item_w + item_gap;
    }
    let settings_x = item_x;
    commands.push(text(settings_x, dock_y, "⚙".to_string(), 24.0, dock_ink));
    item_x += item_w + item_gap;
    let power_x = item_x;
    commands.push(text(power_x, dock_y, "⏻".to_string(), 24.0, dock_ink));

    // Carrousel : cinq placeholders au maximum, comme une rangée de console.
    let dock_zone_top = dock_y - 56.0;
    if games.is_empty() {
        commands.push(text(
            width * 0.5,
            height * 0.42,
            input
                .labels
                .get("empty")
                .cloned()
                .unwrap_or_else(|| "Bibliothèque vide".to_string()),
            22.0,
            COLORS.text,
        ));
    } else {
        let stored = *FOCUS.lock().unwrap_or_else(|error| error.into_inner());
        let mut focus = focus_index(&games, stored);
        if input.keys.iter().any(|key| key == "right") || input.scroll < 0.0 {
            focus = focus.saturating_add(1);
        }
        if input.keys.iter().any(|key| key == "left") || input.scroll > 0.0 {
            focus = focus.saturating_sub(1);
        }
        focus = focus.min(games.len().saturating_sub(1));
        let selected = &games[focus];
        if input.keys.iter().any(|key| key == "enter") {
            activate(api, selected.appid, selected.installed);
        }

        let available_w = (width - margin * 2.0).max(280.0);
        let gap = 16.0;
        let max_card = ((available_w - gap * 4.0) / 5.0).min(300.0);
        let row_top = 92.0;
        let row_bottom = (dock_zone_top - 50.0).max(row_top + 120.0);
        let card_h = max_card
            .min((row_bottom - row_top - 30.0).max(120.0) * 0.82)
            .clamp(120.0, 300.0);
        let card_w = card_h;
        let slots = {
            let fit = (((available_w + gap) / (card_w + gap)).floor() as usize).clamp(1, 5);
            if fit % 2 == 0 {
                (fit - 1).max(1)
            } else {
                fit
            }
        };
        let (start, shown) = if games.len() <= slots {
            (0, games.len())
        } else {
            (
                focus.saturating_sub(slots / 2).min(games.len() - slots),
                slots,
            )
        };
        let row_center = (row_top + row_bottom) * 0.5;
        let card_top = row_center - card_h * 0.5;
        let row_w = shown as f32 * card_w + shown.saturating_sub(1) as f32 * gap;
        let row_left = width * 0.5 - row_w * 0.5;
        let radius = 6.0;
        let mut selected_rect = None;
        let mut activated = false;
        for slot in 0..shown {
            let index = start + slot;
            let game = &games[index];
            let x = row_left + slot as f32 * (card_w + gap);
            let card = (x, card_top, card_w, card_h);
            if index == focus {
                selected_rect = Some(card);
            }
            commands.push(rect(
                x,
                card_top,
                card_w,
                card_h,
                radius,
                if index == focus {
                    COLORS.card_active
                } else {
                    COLORS.card
                },
            ));
            commands.push(PageCommand::Cover {
                appid: game.appid,
                x,
                y: card_top,
                width: card_w,
                height: card_h,
                rounding: radius,
            });
            let hovered = input.mouse.is_some_and(|[mx, my]| {
                mx >= x && mx <= x + card_w && my >= card_top && my <= card_top + card_h
            });
            if hovered || index == focus {
                commands.push(rect(
                    x - 3.0,
                    card_top - 3.0,
                    card_w + 6.0,
                    card_h + 6.0,
                    radius + 3.0,
                    if index == focus {
                        COLORS.text
                    } else {
                        COLORS.text_dim
                    },
                ));
                // Le cadre est dessiné avant la jaquette : on redessine la
                // carte par-dessus pour garder un liseré autour d'elle.
                commands.push(rect(x, card_top, card_w, card_h, radius, COLORS.card));
                commands.push(PageCommand::Cover {
                    appid: game.appid,
                    x,
                    y: card_top,
                    width: card_w,
                    height: card_h,
                    rounding: radius,
                });
            }
            if input.clicked && hovered {
                if index == focus {
                    activate(api, game.appid, game.installed);
                    activated = true;
                } else {
                    focus = index;
                }
            }
        }

        if let Some((x, card_top, card_w, card_h)) = selected_rect {
            let selected = &games[focus];
            commands.push(text(
                x + card_w * 0.5,
                card_top - 22.0,
                selected.name.clone(),
                18.0,
                COLORS.text,
            ));
            let label = if selected.installed {
                input
                    .labels
                    .get("play")
                    .cloned()
                    .unwrap_or_else(|| "Jouer".to_string())
            } else {
                input
                    .labels
                    .get("view")
                    .cloned()
                    .unwrap_or_else(|| "Voir la fiche".to_string())
            };
            let pill_w = 150.0;
            let pill_y = card_top + card_h - 26.0;
            commands.push(rect(
                x + card_w * 0.5 - pill_w * 0.5,
                pill_y - 15.0,
                pill_w,
                30.0,
                15.0,
                [248, 248, 248, 255],
            ));
            commands.push(text(
                x + card_w * 0.5,
                pill_y,
                label,
                13.5,
                [25, 25, 25, 255],
            ));
            if input.clicked && !activated {
                if let Some([mx, my]) = input.mouse {
                    if mx >= x + card_w * 0.5 - pill_w * 0.5
                        && mx <= x + card_w * 0.5 + pill_w * 0.5
                        && my >= pill_y - 15.0
                        && my <= pill_y + 15.0
                    {
                        activate(api, selected.appid, selected.installed);
                    }
                }
            }
        }

        if let Ok(mut stored) = FOCUS.lock() {
            *stored = games[focus].appid;
        }

        // Clic ailleurs dans la rangée : le dock et les boutons système sont
        // testés après le carrousel, sans dépendre d'egui.
        if input.clicked {
            if let Some([mx, my]) = input.mouse {
                for (x, y, w, h, key) in &nav_rects {
                    if mx >= *x && mx <= x + w && my >= *y && my <= y + h {
                        if key != "home" {
                            let _ = api.navigate(key);
                        }
                    }
                }
                if mx >= settings_x - item_w * 0.5
                    && mx <= settings_x + item_w * 0.5
                    && my >= dock_y - item_w * 0.5
                    && my <= dock_y + item_w * 0.5
                {
                    let _ = api.open_settings("general");
                }
                if mx >= power_x - item_w * 0.5
                    && mx <= power_x + item_w * 0.5
                    && my >= dock_y - item_w * 0.5
                    && my <= dock_y + item_w * 0.5
                {
                    let _ = api.dispatch(&HostAction::Quit);
                }
            }
        }
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
        home: Some(home),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game(appid: u32) -> LibraryGame {
        LibraryGame {
            appid,
            ..LibraryGame::default()
        }
    }

    #[test]
    fn la_selection_retrouve_l_appid_et_pas_sa_position() {
        let games = [game(10), game(20), game(30)];
        assert_eq!(focus_index(&games, 20), 1);
        assert_eq!(focus_index(&games, 30), 2);
        // Un jeu retiré de la bibliothèque ramène la sélection au début.
        assert_eq!(focus_index(&games, 99), 0);
    }
}
