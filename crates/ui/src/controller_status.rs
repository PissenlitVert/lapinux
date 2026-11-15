//! État, batterie et identification des manettes.

use super::*;

/// Vert, orange puis rouge : le même seuil que la batterie des manettes.
fn battery_color(battery: u8) -> egui::Color32 {
    if battery <= 20 {
        DANGER
    } else if battery <= 40 {
        WARN
    } else {
        OK
    }
}

/// Niveau de batterie en cercle : anneau proportionnel et pourcentage au
/// centre, plus discret qu'une pastille rectangulaire.
pub(super) fn battery_ring(
    ui: &mut egui::Ui,
    battery: u8,
    size: f32,
    charging: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    if charging {
        // La surbrillance doit avancer même quand la souris ne bouge pas.
        ui.ctx().request_repaint_after(Duration::from_millis(16));
    }
    paint_battery_ring(
        ui.painter(),
        rect.center(),
        size,
        battery,
        charging,
        ui.input(|input| input.time),
    );
    response.on_hover_text(lapinux_core::i18n::t_args(
        "controllers.battery.ring_hover",
        &[
            ("battery", &battery.to_string()),
            (
                "band",
                &battery_band_label(Some(band_from_percent(battery))),
            ),
        ],
    ))
}

pub(super) fn paint_battery_arc(
    painter: &egui::Painter,
    center: egui::Pos2,
    radius: f32,
    battery: u8,
    thickness: f32,
) {
    let color = battery_color(battery);
    painter.circle_stroke(
        center,
        radius,
        egui::Stroke::new(thickness, with_alpha(TEXT_DIM(), 0.28)),
    );
    let level = (battery.min(100) as f32 / 100.0).clamp(0.0, 1.0);
    if level > 0.005 {
        let start = -std::f32::consts::FRAC_PI_2;
        let sweep = std::f32::consts::TAU * level;
        let steps = ((level * 64.0).ceil() as usize).max(2);
        painter.add(egui::Shape::line(
            arc_points(center, radius, start, start + sweep, steps),
            egui::Stroke::new(thickness + 0.5, color),
        ));
    }
}

fn arc_points(
    center: egui::Pos2,
    radius: f32,
    start: f32,
    end: f32,
    steps: usize,
) -> Vec<egui::Pos2> {
    let steps = steps.max(2);
    (0..=steps)
        .map(|step| {
            let angle = start + (end - start) * step as f32 / steps as f32;
            center + egui::vec2(angle.cos(), angle.sin()) * radius
        })
        .collect()
}

/// Comète claire qui parcourt l'arc vert pendant la charge : la ligne semble
/// alimentée plutôt que simplement colorée. Les trois passes donnent la queue,
/// le corps et le point brillant, sans texture ni animation d'image.
pub(super) fn paint_charge_sheen(
    painter: &egui::Painter,
    center: egui::Pos2,
    radius: f32,
    battery: u8,
    thickness: f32,
    time: f64,
) {
    let level = (battery.min(100) as f32 / 100.0).clamp(0.0, 1.0);
    if level <= 0.005 {
        return;
    }
    let start = -std::f32::consts::FRAC_PI_2;
    let sweep = std::f32::consts::TAU * level;
    let phase = ((time as f32) * 0.34).fract();
    // La comète s'éteint en douceur avant la fin de l'arc et se rallume au
    // début : sans ce fondu, elle sautait d'un coup du bout de l'arc au haut
    // du cercle à chaque tour.
    let fade = sheen_fade(phase);
    if fade <= 0.001 {
        return;
    }
    let head = start + sweep * phase;
    let tail = (sweep * 0.30).clamp(0.10, 1.15);
    let shine = egui::Color32::from_rgb(226, 255, 237);
    for (length, width, alpha) in [
        (tail, thickness + 4.5, 0.08),
        (tail * 0.55, thickness + 2.4, 0.20),
        ((tail * 0.22).max(0.06), thickness + 0.8, 0.88),
    ] {
        let from = (head - length).max(start);
        if head - from < 0.01 {
            continue;
        }
        painter.add(egui::Shape::line(
            arc_points(center, radius, from, head, 14),
            egui::Stroke::new(width, with_alpha(shine, alpha * fade)),
        ));
    }
    let head_pos = center + egui::vec2(head.cos(), head.sin()) * radius;
    painter.circle_filled(head_pos, thickness * 1.7, with_alpha(shine, 0.10 * fade));
    painter.circle_filled(head_pos, thickness * 0.55, with_alpha(shine, 0.92 * fade));
}

pub(super) fn smoothstep(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    value * value * (3.0 - 2.0 * value)
}

/// Opacité de la comète selon sa position sur l'arc : nulle aux deux
/// extrémités, pleine au milieu. C'est ce fondu qui rend le passage de la fin
/// de l'arc au début invisible au lieu de faire un saut.
pub(super) fn sheen_fade(phase: f32) -> f32 {
    let fade_in = smoothstep(phase / 0.10);
    let fade_out = smoothstep((1.0 - phase) / 0.12);
    fade_in * fade_out
}

pub(super) fn paint_battery_ring(
    painter: &egui::Painter,
    center: egui::Pos2,
    size: f32,
    battery: u8,
    charging: bool,
    time: f64,
) {
    let color = battery_color(battery);
    let radius = (size / 2.0 - 2.0).max(2.0);
    paint_battery_arc(painter, center, radius, battery, 2.0);
    if charging {
        paint_charge_sheen(painter, center, radius, battery, 2.0, time);
    }
    painter.text(
        center,
        egui::Align2::CENTER_CENTER,
        format!("{battery} %"),
        egui::FontId::proportional((size * 0.31).clamp(8.0, 12.5)),
        color,
    );
}

pub(super) fn battery_ring_placeholder(ui: &mut egui::Ui, size: f32, hint: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    let painter = ui.painter();
    painter.circle_stroke(
        rect.center(),
        (size / 2.0 - 2.0).max(2.0),
        egui::Stroke::new(2.0_f32, with_alpha(TEXT_DIM(), 0.28)),
    );
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "—",
        egui::FontId::proportional((size * 0.31).clamp(8.0, 12.5)),
        TEXT_DIM(),
    );
    response.on_hover_text(hint)
}

pub(super) fn battery_charging(state: Option<ChargeState>) -> bool {
    // « Full » veut dire « sur secteur, charge terminée » : le halo doit rester
    // visible même à 100 %, sinon il disparaît au moment où la manette est
    // branchée. Il s'arrête quand l'état repasse à « Sur batterie ».
    matches!(state, Some(ChargeState::Charging | ChargeState::Full))
}

/// Seul libellé d'état de charge de l'application. Le firmware annonce
/// « Full » dès que le chargeur est branché, parfois à 85 % : « Pleine »
/// serait faux, donc cet état parle de l'alimentation (« Sur secteur »).
pub(super) fn charge_state_label(state: ChargeState) -> &'static str {
    match state {
        ChargeState::Discharging => lapinux_core::i18n::t("controllers.battery.state_discharging"),
        ChargeState::Charging => lapinux_core::i18n::t("controllers.battery.state_charging"),
        ChargeState::Full => lapinux_core::i18n::t("controllers.battery.state_full"),
        ChargeState::NotCharging => lapinux_core::i18n::t("controllers.battery.state_not_charging"),
        ChargeState::VoltageError => {
            lapinux_core::i18n::t("controllers.battery.state_voltage_error")
        }
        ChargeState::TemperatureError => {
            lapinux_core::i18n::t("controllers.battery.state_temperature_error")
        }
        ChargeState::ChargingError => {
            lapinux_core::i18n::t("controllers.battery.state_charging_error")
        }
        ChargeState::Unknown => lapinux_core::i18n::t("controllers.battery.state_unknown"),
    }
}

pub(super) fn band_from_percent(percent: u8) -> (u8, u8) {
    let level = if percent >= 100 { 10 } else { percent / 10 };
    (level * 10, (level * 10 + 9).min(100))
}

pub(super) fn battery_band_label(band: Option<(u8, u8)>) -> String {
    match band {
        Some((start, end)) if start == end => format!("{start} %"),
        Some((start, end)) => format!("{start}–{end} %"),
        None => lapinux_core::i18n::t("controllers.battery.unknown_band").to_string(),
    }
}

fn group_thousands(value: u32) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + 2);
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(' ');
        }
        out.push(ch);
    }
    out
}

pub(super) fn format_capacity(design: BatteryDesign) -> String {
    let tenths = design.milliwatt_hours % 1_000 / 100;
    let whole = design.milliwatt_hours / 1_000;
    let wh = if tenths == 0 {
        format!("{whole} Wh")
    } else {
        format!("{whole},{tenths} Wh")
    };
    format!(
        "{} mAh · {wh}",
        group_thousands(design.milliamp_hours as u32)
    )
}

/// Carte Batterie du test en direct : le palier firmware, la capacité
/// nominale, les deux compteurs utiles et le code de la pièce. Les détails
/// techniques du lecteur Sony (tension brute, erreurs, fraîcheur) ne sont pas
/// affichés ici : cette carte doit rester lisible d'un coup d'œil.
pub(super) fn battery_test(
    ui: &mut egui::Ui,
    device: &DeviceInfo,
    pad: Option<&lapinux_input::PadState>,
) {
    let live = pad.and_then(|pad| pad.battery.as_ref());
    if live.is_none() && device.battery.is_none() && device.battery_status.is_none() {
        return;
    }
    let percent = live.and_then(|battery| battery.percent).or(device.battery);
    let band = live
        .and_then(|battery| battery.band())
        .or_else(|| percent.map(band_from_percent));
    let design = live
        .and_then(|battery| battery.design)
        .or_else(|| BatteryDesign::for_product(device.product));
    let state = live
        .map(|battery| battery.state)
        .filter(|state| *state != ChargeState::Unknown)
        .or_else(|| {
            device
                .battery_status
                .as_deref()
                .map(ChargeState::from_sysfs)
        });
    let charging = battery_charging(state);

    ui.vertical_centered(|ui| {
        if let Some(percent) = percent {
            battery_ring(ui, percent, 54.0, charging);
        } else {
            let hint = if lapinux_input::steam::is_steam_2026(device) {
                lapinux_core::i18n::t("controllers.battery.pending")
            } else {
                lapinux_core::i18n::t("controllers.battery.not_exposed")
            };
            battery_ring_placeholder(ui, 54.0, hint);
        }
        ui.add_space(2.0);
        ui.label(
            egui::RichText::new(lapinux_core::i18n::t_args(
                "controllers.battery.band",
                &[("band", &battery_band_label(band))],
            ))
            .size(9.0)
            .color(TEXT_DIM()),
        );
        // Le firmware Switch annonce « pleine » dès le branchement : on
        // conserve le dernier palier réel et on le dit, plutôt que d'afficher
        // un 100 % prématuré.
        if charging && device.vendor == 0x057e && live.is_some() {
            ui.label(
                egui::RichText::new(lapinux_core::i18n::t(
                    "controllers.battery.charging_last_band",
                ))
                .size(8.5)
                .color(TEXT_DIM()),
            );
        }
        ui.add_space(6.0);
        if let Some(design) = design {
            ui.label(
                egui::RichText::new(format_capacity(design))
                    .size(9.5)
                    .color(TEXT()),
            )
            .on_hover_text(lapinux_core::i18n::t("controllers.battery.capacity_hover"));
        }
        if let Some(live) = live {
            let mut counts = Vec::new();
            if let Some(cycles) = live.cycles {
                counts.push(lapinux_core::i18n::t_args(
                    "controllers.battery.cycles",
                    &[("count", &cycles.to_string())],
                ));
            }
            if let Some(full) = live.full_charges {
                counts.push(lapinux_core::i18n::t_args(
                    "controllers.battery.charges",
                    &[("count", &full.to_string())],
                ));
            }
            if !counts.is_empty() {
                ui.label(
                    egui::RichText::new(counts.join(" · "))
                        .size(9.0)
                        .color(TEXT_DIM()),
                );
            }
            if let Some(code) = live.battery_code.as_deref() {
                ui.add_space(3.0);
                ui.add(
                    egui::Label::new(egui::RichText::new(code).size(8.5).color(TEXT_DIM()))
                        .selectable(true),
                )
                .on_hover_text(lapinux_core::i18n::t("controllers.battery.code_hover"));
            }
        }
    });
}

/// Carte Bluetooth du test en direct : la cadence réelle des rapports HID
/// (mesurée par le moniteur sur les horodatages du noyau) et les propriétés
/// BlueZ de la liaison. Le RSSI n'apparaît que si BlueZ le publie : une
/// manette HID classique (BR/EDR) ne l'expose pas sans droits administrateur,
/// et la carte le dit plutôt que d'inventer une valeur.
pub(super) fn bluetooth_test(
    ui: &mut egui::Ui,
    device: &DeviceInfo,
    pad: &lapinux_input::PadState,
    tests: &mut PadTestState,
) {
    // La carte est en direct : quatre images par seconde suffisent pour les
    // chiffres, et on sonde plus vite tant que BlueZ répond.
    let refresh = if tests.bluetooth_rx.is_some() {
        100
    } else {
        250
    };
    ui.ctx()
        .request_repaint_after(Duration::from_millis(refresh));
    let address = device.address.clone().unwrap_or_default();
    if !address.is_empty() && tests.bluetooth_for.as_deref() != Some(address.as_str()) {
        tests.bluetooth_for = Some(address.clone());
        tests.bluetooth = None;
        tests.bluetooth_rx = None;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(lapinux_input::bluetooth::details(&address));
        });
        tests.bluetooth_rx = Some(rx);
    }
    if let Some(rx) = tests.bluetooth_rx.take() {
        match rx.try_recv() {
            Ok(details) => tests.bluetooth = details,
            Err(mpsc::TryRecvError::Empty) => tests.bluetooth_rx = Some(rx),
            Err(mpsc::TryRecvError::Disconnected) => {}
        }
    }

    let cadence = pad.report;
    let decimal =
        |value: f32, suffix: &str| lapinux_core::i18n::decimal(format!("{:.1}{suffix}", value));
    ui.vertical_centered(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
        let width = ((ui.available_width() - 4.0) / 2.0).max(70.0);
        ui.horizontal(|ui| {
            bluetooth_stat(
                ui,
                width,
                lapinux_core::i18n::t("controllers.bluetooth.latency"),
                cadence
                    .map(|cadence| decimal(cadence.interval_ms, " ms"))
                    .unwrap_or_else(|| "—".to_string()),
            )
            .on_hover_text(lapinux_core::i18n::t("controllers.bluetooth.latency_hover"));
            bluetooth_stat(
                ui,
                width,
                lapinux_core::i18n::t("controllers.bluetooth.rate"),
                cadence
                    .map(|cadence| format!("{:.0} Hz", cadence.rate_hz))
                    .unwrap_or_else(|| "—".to_string()),
            )
            .on_hover_text(lapinux_core::i18n::t("controllers.bluetooth.rate_hover"));
        });
        ui.add_space(4.0);
        if let Some(details) = &tests.bluetooth {
            bluetooth_row(
                ui,
                lapinux_core::i18n::t("controllers.bluetooth.link"),
                details.link_label(),
            );
            bluetooth_row(
                ui,
                lapinux_core::i18n::t("controllers.bluetooth.paired"),
                details.pairing_label(),
            );
            let address_type = match details.address_type.as_str() {
                "public" => lapinux_core::i18n::t("controllers.bluetooth.public"),
                "random" => lapinux_core::i18n::t("controllers.bluetooth.random"),
                other => other,
            };
            let vendor = format!("{:04x}", device.vendor);
            let product = format!("{:04x}", device.product);
            bluetooth_row(
                ui,
                lapinux_core::i18n::t("controllers.bluetooth.address"),
                &details.address,
            )
            .on_hover_text(lapinux_core::i18n::t_args(
                "controllers.bluetooth.address_hover",
                &[
                    ("address_type", address_type),
                    ("vendor", &vendor),
                    ("product", &product),
                ],
            ));
            let class = match details.class_label() {
                Some(label) => format!("{label} · {}", details.class),
                None => details.class.clone(),
            };
            bluetooth_row(
                ui,
                lapinux_core::i18n::t("controllers.bluetooth.class"),
                &class,
            );
            let signal = match details.rssi {
                Some(rssi) => format!("{rssi} dBm"),
                None => lapinux_core::i18n::t("controllers.bluetooth.not_exposed").to_string(),
            };
            let hover = if let Some(tx_power) = details.tx_power {
                lapinux_core::i18n::t_args(
                    "controllers.bluetooth.signal_tx_hover",
                    &[("power", &tx_power.to_string())],
                )
            } else if details.rssi.is_none() {
                lapinux_core::i18n::t("controllers.bluetooth.signal_ble_hover").to_string()
            } else {
                lapinux_core::i18n::t("controllers.bluetooth.signal_hover").to_string()
            };
            bluetooth_row(
                ui,
                lapinux_core::i18n::t("controllers.bluetooth.signal"),
                &signal,
            )
            .on_hover_text(hover);
            bluetooth_row(
                ui,
                lapinux_core::i18n::t("controllers.bluetooth.wake"),
                if details.wake_allowed {
                    lapinux_core::i18n::t("controllers.bluetooth.allowed")
                } else {
                    lapinux_core::i18n::t("controllers.bluetooth.denied")
                },
            )
            .on_hover_text(lapinux_core::i18n::t("controllers.bluetooth.wake_hover"));
            if details.blocked {
                bluetooth_row(
                    ui,
                    lapinux_core::i18n::t("controllers.bluetooth.blocked"),
                    lapinux_core::i18n::t("controllers.bluetooth.yes"),
                );
            }
        } else if tests.bluetooth_rx.is_some() {
            muted(ui, lapinux_core::i18n::t("controllers.bluetooth.reading"));
        } else {
            muted(
                ui,
                lapinux_core::i18n::t("controllers.bluetooth.unavailable"),
            );
        }
    });
}

fn bluetooth_stat(ui: &mut egui::Ui, width: f32, label: &str, value: String) -> egui::Response {
    ui.allocate_ui_with_layout(
        egui::vec2(width, 30.0),
        egui::Layout::top_down(egui::Align::Center),
        |ui| {
            let response = ui.label(egui::RichText::new(value).size(12.5).color(TEXT()));
            ui.label(egui::RichText::new(label).size(7.5).color(TEXT_DIM()));
            response
        },
    )
    .inner
}

fn bluetooth_row(ui: &mut egui::Ui, label: &str, value: &str) -> egui::Response {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).size(9.0).color(TEXT_DIM()));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new(value).size(9.5).color(TEXT()));
        });
    })
    .response
}

pub(super) fn steam_puck_card(ui: &mut egui::Ui) {
    let card = egui::Frame::none()
        .fill(mix(CARD(), WARN, 0.025))
        .stroke(egui::Stroke::new(1.0_f32, with_alpha(WARN, 0.35)))
        .rounding(egui::Rounding::same(12.0))
        .inner_margin(egui::Margin::symmetric(12.0, 12.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (icon, _) =
                    ui.allocate_exact_size(egui::vec2(34.0, 34.0), egui::Sense::hover());
                ui.painter()
                    .rect_filled(icon, egui::Rounding::same(10.0), with_alpha(WARN, 0.14));
                ui.painter().text(
                    icon.center(),
                    egui::Align2::CENTER_CENTER,
                    nav_glyph(Page::Controllers),
                    icon_font(18.0),
                    WARN,
                );
                ui.add_space(3.0);
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(lapinux_core::i18n::t("controllers.steam.puck_title"))
                            .size(13.0)
                            .strong()
                            .color(TEXT()),
                    );
                    ui.horizontal(|ui| {
                        let (dot, _) =
                            ui.allocate_exact_size(egui::vec2(7.0, 7.0), egui::Sense::hover());
                        ui.painter().circle_filled(dot.center(), 2.5, WARN);
                        ui.label(
                            egui::RichText::new(lapinux_core::i18n::t(
                                "controllers.steam.puck_subtitle",
                            ))
                            .size(11.0)
                            .color(WARN),
                        );
                    });
                });
            });
            ui.add_space(9.0);
            ui.label(
                egui::RichText::new(lapinux_core::i18n::t("controllers.steam.puck_body"))
                    .size(11.5)
                    .color(TEXT_DIM()),
            );
        });
    card.response
        .on_hover_text(lapinux_core::i18n::t("controllers.steam.puck_hover"));
}

/// Identité d'une manette décrite par Lapinux. Le pont `steam-puck-bridge`
/// expose la Steam Controller 2026 comme un pad Xbox 360 uinput : sans ce
/// drapeau, la fiche afficherait « Xbox 360 » et un logo Xbox.
pub(super) fn pad_identity(device: &DeviceInfo) -> PadIdentity {
    if device.steam_bridge {
        return pad_id::steam_bridge_identity();
    }
    pad_id::identify(device.vendor, device.product, &device.name)
}

/// Steam Controller 2026 native : Puck filaire ou sans fil. Le paramètre
/// `lizard_mode` de `hid_steam` ne concerne que cette famille.
pub(super) fn is_steam_controller_2026(device: &DeviceInfo) -> bool {
    lapinux_core::model::is_steam_controller_2026(device.vendor, device.product)
}

/// Infobulle d'une manette : nom affiché, nom brut d'evdev quand il a été
/// reconnu, puis marque, famille, connexion et identifiants.
pub(super) fn controller_tooltip(
    device: &DeviceInfo,
    identity: &PadIdentity,
    shown: &str,
) -> String {
    let detected = identity.display_name(&device.name);
    let mut text = shown.to_string();
    if shown != detected {
        text.push('\n');
        text.push_str(detected);
    }
    if detected != device.name {
        text.push('\n');
        text.push_str(&device.name);
    }
    if let Some(console) = console_label(identity) {
        text.push_str(&format!("\n{console}"));
    } else if let Some(brand) = identity.brand {
        text.push_str(&format!("\n{}", brand.label()));
    }
    text.push_str(&format!(
        "\n{} · {:04x}:{:04x}",
        device.connection.label(),
        device.vendor,
        device.product
    ));
    if device.steam_bridge {
        text.push_str(lapinux_core::i18n::t("controllers.ui.bridge"));
    }
    if let Some(battery) = device.battery {
        text.push_str(&lapinux_core::i18n::t_args(
            "controllers.ui.battery_tooltip",
            &[
                ("battery", &battery.to_string()),
                (
                    "state",
                    &battery_band_label(Some(band_from_percent(battery))),
                ),
            ],
        ));
        if let Some(status) = device.battery_status.as_deref() {
            text.push_str(&format!(
                " · {}",
                charge_state_label(ChargeState::from_sysfs(status))
            ));
        }
    }
    text
}

/// Intitulé de groupe repliable, dessiné comme les catégories des réglages :
/// un chevron à droite et un simple survol, sans cadre.
pub(super) fn controller_group_header(
    ui: &mut egui::Ui,
    title: &str,
    count: usize,
    collapsed: &mut bool,
) {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 24.0), egui::Sense::click());
    let response = response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(if *collapsed {
            lapinux_core::i18n::t("controllers.ui.show_category")
        } else {
            lapinux_core::i18n::t("controllers.ui.hide_category")
        });
    if response.hovered() {
        ui.painter().rect_filled(
            rect,
            egui::Rounding::same(6.0),
            with_alpha(CARD_HOVER(), 0.35),
        );
    }
    let ink = if response.hovered() {
        TEXT()
    } else {
        TEXT_DIM()
    };
    ui.painter().text(
        egui::pos2(rect.left() + 2.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        title,
        egui::FontId::proportional(10.0),
        ink,
    );
    let caret = egui::pos2(rect.right() - 12.0, rect.center().y);
    let count_pos = egui::pos2(caret.x - 14.0, rect.center().y);
    ui.painter().text(
        count_pos,
        egui::Align2::RIGHT_CENTER,
        count.to_string(),
        egui::FontId::proportional(9.5),
        TEXT_DIM(),
    );
    let size = 4.2;
    let points = if *collapsed {
        vec![
            caret + egui::vec2(-size * 0.45, -size),
            caret + egui::vec2(size * 0.6, 0.0),
            caret + egui::vec2(-size * 0.45, size),
        ]
    } else {
        vec![
            caret + egui::vec2(-size, -size * 0.45),
            caret + egui::vec2(0.0, size * 0.6),
            caret + egui::vec2(size, -size * 0.45),
        ]
    };
    ui.painter()
        .add(egui::Shape::line(points, egui::Stroke::new(1.4_f32, ink)));
    if response.clicked() {
        *collapsed = !*collapsed;
    }
    ui.add_space(4.0);
}

/// Nom affiché d'une manette : le surnom choisi par l'utilisateur, sinon le
/// modèle reconnu, sinon le nom brut d'evdev.
pub(super) fn pad_shown_name(
    names: &HashMap<String, String>,
    device: &DeviceInfo,
    identity: &PadIdentity,
) -> String {
    pad_setting(names, device)
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| identity.display_name(&device.name).to_string())
}

/// Ancienne clé de configuration d'un pad, avant que l'adresse Bluetooth ne
/// soit conservée lors de la fusion avec une entrée dongle. Elle sert de
/// repli pour ne pas perdre les surnoms déjà enregistrés — mais jamais pour
/// une manette identifiée par son adresse : le numéro de nœud evdev est
/// réutilisé par le noyau, et la DualSense héritait alors du masquage et du
/// nom laissés par le dongle 2,4 GHz qui occupait le même `event`.
pub(super) fn legacy_path_key(device: &DeviceInfo) -> Option<String> {
    if device.address.is_some() || device.path.as_os_str().is_empty() {
        return None;
    }
    let key = format!("path:{}", device.path.display());
    (key != controller_key(device)).then_some(key)
}

/// Lit un réglage de pad sous sa clé actuelle, puis sous l'ancienne clé de
/// chemin : un renommage ou un masquage enregistré avant la fusion reste
/// valable après.
pub(super) fn pad_setting<'a, T>(
    settings: &'a HashMap<String, T>,
    device: &DeviceInfo,
) -> Option<&'a T> {
    settings
        .get(&controller_key(device))
        .or_else(|| legacy_path_key(device).and_then(|key| settings.get(&key)))
}

/// Nom de la console tel qu'il est imprimé sur la boîte, sans pastille.
pub(super) fn console_label(identity: &PadIdentity) -> Option<&'static str> {
    match identity.family {
        Some("PS5") => Some("PlayStation 5"),
        Some("PS4") => Some("PlayStation 4"),
        Some("PS3") => Some("PlayStation 3"),
        Some("Switch") => Some("Nintendo Switch"),
        Some("Xbox 360") => Some("Xbox 360"),
        Some("Xbox One") => Some("Xbox One"),
        Some("Xbox Series") => Some("Xbox Series"),
        Some("Steam Deck") => Some("Steam Deck"),
        Some("Steam") => Some("Steam Controller"),
        // Un identifiant et une famille inconnus ne doivent pas être
        // présentés comme une marque : « Inconnu » est plus honnête que
        // « Générique ».
        _ => match identity.brand {
            Some(Brand::PlayStation) => Some("PlayStation"),
            Some(Brand::Xbox) => Some("Xbox"),
            Some(Brand::Nintendo) => Some("Nintendo"),
            Some(Brand::Steam) => Some("Steam"),
            _ => Some(lapinux_core::i18n::t("controllers.information.unknown")),
        },
    }
}

/// Bluetooth ou USB, dessinés au trait pour rester nets à petite taille.
fn paint_connection_icon(
    painter: &egui::Painter,
    center: egui::Pos2,
    connection: DeviceLink,
    color: egui::Color32,
    scale: f32,
) {
    let stroke = egui::Stroke::new(1.2_f32 * scale.clamp(0.9, 1.3), color);
    let p = |x: f32, y: f32| center + egui::vec2(x, y) * scale;
    match connection {
        DeviceLink::Bluetooth => {
            painter.line_segment([p(0.0, -6.5), p(0.0, 6.5)], stroke);
            painter.line_segment([p(0.0, -6.5), p(5.0, -2.6)], stroke);
            painter.line_segment([p(5.0, -2.6), p(-5.0, 2.6)], stroke);
            painter.line_segment([p(0.0, 6.5), p(5.0, 2.6)], stroke);
            painter.line_segment([p(5.0, 2.6), p(-5.0, -2.6)], stroke);
        }
        DeviceLink::Usb | DeviceLink::Dongle => {
            painter.line_segment([p(0.0, 6.5), p(0.0, -5.0)], stroke);
            painter.line_segment([p(-5.0, -2.5), p(0.0, 0.8)], stroke);
            painter.line_segment([p(5.0, -3.8), p(0.0, 0.8)], stroke);
            painter.circle_stroke(p(-5.0, -3.0), 1.1 * scale, stroke);
            painter.rect_stroke(
                egui::Rect::from_center_size(p(5.0, -4.3), egui::vec2(2.6, 2.6) * scale),
                egui::Rounding::same(0.4),
                stroke,
            );
            painter.add(egui::Shape::convex_polygon(
                vec![p(0.0, -6.8), p(-2.2, -4.8), p(2.2, -4.8)],
                color,
                egui::Stroke::NONE,
            ));
        }
        _ => {
            painter.circle_stroke(center, 2.0 * scale, stroke);
        }
    }
}

/// Pastille ronde de connexion : même diamètre que l'anneau de batterie,
/// fond et contour gris, glyphe dans la couleur du bus.
pub(super) fn connection_badge(
    painter: &egui::Painter,
    center: egui::Pos2,
    connection: DeviceLink,
    diameter: f32,
) {
    let color = if connection == DeviceLink::Bluetooth {
        BLUETOOTH_BLUE
    } else {
        TEXT_DIM()
    };
    // Même bord extérieur que l'anneau de batterie, qui laisse deux pixels
    // de marge de chaque côté dans son gabarit. Pas de remplissage : seul le
    // contour gris reste, pour ne pas alourdir la carte.
    let radius = (diameter / 2.0 - 1.0).max(2.0);
    painter.circle_stroke(
        center,
        radius - 0.5,
        egui::Stroke::new(1.0_f32, with_alpha(TEXT_DIM(), 0.55)),
    );
    paint_connection_icon(painter, center, connection, color, diameter / 22.0);
}

/// Identité stable d'une manette pour la sélection : les manettes appairées
/// hors ligne n'ont pas de chemin evdev, seul leur nom ou leur adresse diffère.
pub(super) fn controller_key(device: &DeviceInfo) -> String {
    // L'adresse survit aux rebranchements et aux changements de numéro de
    // nœud evdev : elle porte le renommage et la sélection.
    if let Some(address) = &device.address {
        return format!("bluetooth:{}", address.to_ascii_lowercase());
    }
    // Le Puck USB du Steam Controller peut recréer son nœud evdev quand
    // hidraw est ouvert : le numéro `eventN` change alors et la sélection
    // repartirait de zéro. Son numéro de série udev, lui, ne change pas.
    if device.vendor == 0x28de && device.product == 0x1304 {
        if let Some(serial) = device
            .udev
            .serial
            .as_deref()
            .map(str::trim)
            .filter(|serial| !serial.is_empty())
        {
            return format!("steam-serial:{}", serial.to_ascii_lowercase());
        }
    }
    if !device.path.as_os_str().is_empty() {
        return format!("path:{}", device.path.display());
    }
    format!("name:{}", device.name)
}

/// Type Steam Input de la manette affichée dans Manettes. Le pont de la
/// Steam Controller 2026 se présente comme un pad Xbox à evdev ; on le ramène
/// au type Triton pour retrouver les configurations Steam qui la concernent.
pub(super) fn steam_input_kind_for(device: &DeviceInfo) -> &'static str {
    if device.steam_bridge {
        "controller_triton"
    } else {
        lapinux_core::model::steam_input_kind(device.vendor, device.product)
    }
}

/// Fusionne les manettes evdev (en ligne) et les manettes Bluetooth appairées
/// mais hors ligne. Une même manette ne doit pas apparaître deux fois.
pub(super) fn merge_controllers(
    evdev: Vec<DeviceInfo>,
    paired: Vec<DeviceInfo>,
) -> Vec<DeviceInfo> {
    let mut merged: Vec<DeviceInfo> = Vec::new();
    for pad in evdev.into_iter().chain(paired) {
        if let Some(existing) = merged.iter_mut().find(|known| same_controller(known, &pad)) {
            // Le câble gagne sur le Bluetooth, et une entrée en ligne sur
            // une entrée appairée mais éteinte. Une seule carte par manette.
            let prefer_new = controller_preference(&pad) > controller_preference(existing);
            let (winner, loser) = if prefer_new {
                (pad, existing.clone())
            } else {
                (existing.clone(), pad)
            };
            *existing = winner;
            // BlueZ connaît souvent la batterie d'une manette Bluetooth
            // alors que son nœud evdev n'expose aucun power_supply : garder
            // cette information quand les deux entrées fusionnent.
            if existing.battery.is_none() {
                existing.battery = loser.battery;
            }
            if existing.battery_status.is_none() {
                existing.battery_status = loser.battery_status;
            }
            if existing.button_names.is_empty() {
                existing.button_names = loser.button_names;
            }
            // Le dongle n'expose pas d'adresse : celle de l'entrée Bluetooth
            // de la même manette garde le surnom et la sélection d'une
            // liaison à l'autre.
            if existing.address.is_none() && existing.connection == DeviceLink::Dongle {
                existing.address = loser.address.clone();
            }
            existing.force_feedback |= loser.force_feedback;
            existing.touchpad |= loser.touchpad;
        } else {
            merged.push(pad);
        }
    }
    merged.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| b.connected.cmp(&a.connected))
            .then_with(|| a.path.cmp(&b.path))
            .then_with(|| a.address.cmp(&b.address))
    });
    merged
}

/// Préférence pour une seule carte : USB plutôt que Bluetooth, en ligne
/// plutôt que hors ligne.
fn controller_preference(device: &DeviceInfo) -> (bool, bool) {
    (
        device.connected,
        device.connection == lapinux_core::model::DeviceLink::Usb,
    )
}

fn same_controller(a: &DeviceInfo, b: &DeviceInfo) -> bool {
    match (&a.address, &b.address) {
        // Même identifiant HID : c'est la même manette, quel que soit le bus.
        (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
        // Une entrée BlueZ appairée ne porte pas forcément le même nom que
        // l'interface evdev (« DS5 » contre le nom complet) : le modèle
        // identifie alors la même manette, branchée en USB. Deux manettes
        // génériques qui partagent le même identifiant 045e:028e mais deux
        // récepteurs distincts ne doivent pas fusionner pour autant.
        _ if a.gamepad
            && b.gamepad
            && a.connected != b.connected
            && (a.connection == lapinux_core::model::DeviceLink::Bluetooth
                || b.connection == lapinux_core::model::DeviceLink::Bluetooth) =>
        {
            a.vendor == b.vendor && a.product == b.product
        }
        // Deux manettes identiques en ligne sans identifiant restent deux
        // manettes : ne pas les fusionner par leur seul modèle.
        _ => false,
    }
}

/// Durée sans aucune entrée au-delà de laquelle une manette sous dongle est
/// considérée éteinte. Un récepteur 2,4 GHz ne signale pas la connexion : deux
/// minutes de silence valent « manette éteinte » plutôt que « connectée ».
pub(super) const DONGLE_SILENCE_SECS: u64 = 120;

/// Un dongle n'annonce pas l'état de sa manette. Sans entrée récente, la carte
/// décrit le récepteur, pas une manette allumée : elle passe donc hors ligne.
pub(super) fn dongle_asleep(
    device: &DeviceInfo,
    pads: &[lapinux_input::PadState],
    now: u64,
) -> bool {
    if device.connection != DeviceLink::Dongle {
        return false;
    }
    // hid-steam retire le nœud evdev du pad Valve quand la manette s'éteint :
    // sa présence prouve la connexion, même sans entrée. Les récepteurs Xbox,
    // eux, gardent leur nœud allumé quand la manette dort.
    if device.vendor == 0x28de {
        return false;
    }
    let Some(pad) = pads.iter().find(|pad| pad.path == device.path) else {
        // Pas encore d'état : ne pas inventer une déconnexion.
        return false;
    };
    pad.last_input_unix
        .map_or(true, |at| now.saturating_sub(at) > DONGLE_SILENCE_SECS)
}

/// Répartit les manettes entre « En ligne » et « Appairées ». Une manette
/// sous dongle allumée reste en ligne, comme les liaisons USB et Bluetooth
/// actives ; seule une entrée hors ligne va dans Appairées.
pub(super) fn split_controller_groups<'a>(
    gamepads: &'a [DeviceInfo],
) -> (Vec<&'a DeviceInfo>, Vec<&'a DeviceInfo>) {
    let mut online = Vec::new();
    let mut paired = Vec::new();
    for pad in gamepads {
        if pad.connected {
            online.push(pad);
        } else {
            paired.push(pad);
        }
    }
    (online, paired)
}
