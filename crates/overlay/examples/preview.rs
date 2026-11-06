use lapinux_overlay::menu::{HudPosition, HudPreset, MenuState, Page};
use lapinux_overlay::render::Renderer;

fn main() {
    let font = lapinux_overlay::font::find_font().expect("police");
    let renderer = Renderer::new(&font, 1280, 800, None, None).expect("rendu");
    let mut menu = MenuState::new("A Short Hike");
    menu.anim = 1.0;
    menu.fps_cap = 120;
    menu.hud_on = true;
    menu.hud = HudPreset::Detailed;
    menu.hud_position = HudPosition::TopCenter;
    menu.set_live(lapinux_overlay::menu::Live {
        fps: Some(117),
        session_secs: Some(2_400),
        series: vec![110, 114, 117, 118, 116, 117, 119, 117],
        dlss: None,
        hud_live: true,
        gamescope: true,
        recording: Some(lapinux_core::model::RecordingState {
            paused: false,
            secs: 1_337,
        }),
        volume: Some(0.72),
    });
    menu.set_page(Page::Menu);
    renderer.render(&menu).save_png("/tmp/overlay-menu.png").unwrap();
    menu.set_page(Page::Performance);
    menu.perf_selected = 3;
    renderer.render(&menu).save_png("/tmp/overlay-perf.png").unwrap();
    println!("ok");
}
