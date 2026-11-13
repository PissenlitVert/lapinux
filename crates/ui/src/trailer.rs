//! Lecteur de bande-annonce dans l'interface.
//!
//! GStreamer décode le manifeste DASH ou HLS dans un petit hôte Python
//! (`trailer_helper.py`) : l'audio part vers la sortie sonore du bureau, les
//! images arrivent en RGBA par un tube et sont peintes dans une texture egui.
//! Rien n'ouvre de navigateur ni de fenêtre vidéo séparée ; le panneau reste
//! celui de l'application, au-dessus de la fiche.

use eframe::egui;
use lapinux_core::paths::LapinuxPaths;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::{TEXT, TEXT_DIM};

/// Une vidéo de plateforme (YouTube, Vimeo) est une page web, pas un flux que
/// GStreamer sait ouvrir directement. La fiche la confie à la fenêtre WebKit
/// au lieu de lancer le lecteur intégré.
pub(crate) fn is_web_page(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.contains("youtube.com") || lower.contains("youtu.be") || lower.contains("vimeo.com")
}

/// L'hôte GStreamer, embarqué dans le binaire et écrit dans le cache.
const HELPER: &str = include_str!("trailer_helper.py");
/// Les commandes s'effacent après ce délai de lecture immobile.
const CONTROLS_TIMEOUT: Duration = Duration::from_secs(3);
/// Repère du premier clic : il ne doit pas refermer la visionneuse ouverte.
const FRESH_ID: &str = "trailer-cinema-fresh";

struct Frame {
    width: usize,
    height: usize,
    rgba: Vec<u8>,
}

enum Event {
    Time {
        position: f64,
        duration: f64,
        playing: bool,
    },
    Buffering(u32),
    Error(String),
    Ended,
    Closed,
}

struct Session {
    child: Child,
    stdin: ChildStdin,
    events: mpsc::Receiver<Event>,
    frame: Arc<Mutex<Option<Frame>>>,
}

impl Session {
    fn send(&mut self, value: serde_json::Value) {
        let _ = writeln!(self.stdin, "{value}");
        let _ = self.stdin.flush();
    }
}

/// Le lecteur : au plus une bande-annonce à la fois.
pub(crate) struct Trailer {
    session: Option<Session>,
    visible: bool,
    texture: Option<egui::TextureHandle>,
    title: String,
    position: f64,
    duration: f64,
    playing: bool,
    buffering: Option<u32>,
    muted: bool,
    volume: f32,
    /// Position choisie pendant que le curseur est tenu.
    seek_preview: Option<f64>,
    /// Dernier mouvement de la souris, pour effacer les commandes au repos.
    controls_at: Instant,
    error: Option<String>,
}

impl Default for Trailer {
    fn default() -> Self {
        Self {
            session: None,
            visible: false,
            texture: None,
            title: String::new(),
            position: 0.0,
            duration: 0.0,
            playing: false,
            buffering: None,
            muted: false,
            volume: 0.85,
            seek_preview: None,
            controls_at: Instant::now(),
            error: None,
        }
    }
}

impl Drop for Trailer {
    fn drop(&mut self) {
        self.close();
    }
}

impl Trailer {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Ouvre la bande-annonce d'un jeu, en remplaçant celle qui jouait.
    pub(crate) fn open(
        &mut self,
        ctx: &egui::Context,
        paths: &LapinuxPaths,
        url: &str,
        title: &str,
    ) {
        self.close();
        self.visible = true;
        self.title = if title.trim().is_empty() {
            String::new()
        } else {
            title.trim().to_string()
        };
        self.texture = None;
        self.position = 0.0;
        self.duration = 0.0;
        self.playing = false;
        self.buffering = None;
        self.muted = false;
        self.volume = 0.85;
        self.seek_preview = None;
        self.controls_at = Instant::now();
        self.error = None;
        ctx.memory_mut(|memory| memory.data.remove::<bool>(egui::Id::new(FRESH_ID)));
        match spawn(paths, ctx, url) {
            Ok(session) => self.session = Some(session),
            Err(message) => self.error = Some(message),
        }
    }

    /// Ferme le lecteur et arrête son processus.
    pub(crate) fn close(&mut self) {
        if let Some(mut session) = self.session.take() {
            session.send(serde_json::json!({ "cmd": "quit" }));
            // Le décodeur relâche son audio et son réseau en s'arrêtant : on
            // lui laisse un instant, puis on termine le processus.
            let deadline = Instant::now() + Duration::from_millis(500);
            while Instant::now() < deadline {
                match session.child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                    Err(_) => break,
                }
            }
            let _ = session.child.kill();
            let _ = session.child.wait();
        }
        self.visible = false;
        self.texture = None;
        self.buffering = None;
        self.seek_preview = None;
    }

    /// Lit les réponses du décodeur et met la texture à jour.
    fn poll(&mut self, ctx: &egui::Context) {
        let mut events = Vec::new();
        if let Some(session) = &self.session {
            while let Ok(event) = session.events.try_recv() {
                events.push(event);
            }
        }
        let mut closed = false;
        for event in events {
            match event {
                Event::Time {
                    position,
                    duration,
                    playing,
                } => {
                    self.position = position;
                    if duration > 0.0 {
                        self.duration = duration;
                    }
                    self.playing = playing;
                }
                Event::Buffering(percent) => {
                    // GStreamer répète 100 % plusieurs fois : seul un
                    // remplissage en cours mérite un message.
                    self.buffering = if percent < 100 { Some(percent) } else { None };
                }
                Event::Error(message) => {
                    self.error = Some(message);
                    self.playing = false;
                    self.buffering = None;
                }
                Event::Ended => {
                    self.playing = false;
                    self.position = self.duration;
                    self.buffering = None;
                }
                Event::Closed => closed = true,
            }
        }
        if closed {
            self.session = None;
            if self.error.is_none() {
                self.error = Some(lapinux_core::i18n::t("trailer.stopped").into());
            }
        }
        let frame = self
            .session
            .as_ref()
            .and_then(|session| session.frame.lock().ok().and_then(|mut slot| slot.take()));
        if let Some(frame) = frame {
            let image =
                egui::ColorImage::from_rgba_unmultiplied([frame.width, frame.height], &frame.rgba);
            match &mut self.texture {
                Some(texture) => texture.set(image, egui::TextureOptions::LINEAR),
                None => {
                    self.texture =
                        Some(ctx.load_texture("trailer-frame", image, egui::TextureOptions::LINEAR))
                }
            }
            self.buffering = None;
        }
        if self.visible {
            // Une nouvelle image appelle déjà un repaint ; ce délai garde le
            // temps de lecture et le curseur vivants même en pause.
            ctx.request_repaint_after(Duration::from_millis(33));
        }
    }

    fn send(&mut self, value: serde_json::Value) {
        if let Some(session) = self.session.as_mut() {
            session.send(value);
        }
    }

    /// Visionneuse cinéma : la vidéo occupe la surface laissée par la barre
    /// de fenêtre, les commandes se posent dessus et s'effacent pendant la
    /// lecture.
    pub(crate) fn panel(&mut self, ctx: &egui::Context, screen: egui::Rect) {
        if !self.visible {
            return;
        }
        self.poll(ctx);
        if !self.visible {
            return;
        }

        let rect = viewer_rect(screen);
        let top_rect = egui::Rect::from_min_size(
            rect.min + egui::vec2(22.0, 16.0),
            egui::vec2((rect.width() - 44.0).max(120.0), 56.0),
        );
        let controls_rect = egui::Rect::from_min_max(
            egui::pos2(rect.left(), (rect.bottom() - 112.0).max(rect.top())),
            egui::pos2(rect.right(), rect.bottom()),
        );
        let bar = egui::Rect::from_min_max(
            egui::pos2(rect.left() + 22.0, (rect.bottom() - 84.0).max(rect.top())),
            egui::pos2(rect.right() - 22.0, (rect.bottom() - 18.0).max(rect.top())),
        );
        let pointer = ctx.input(|input| input.pointer.hover_pos());
        let pointer_moved = ctx.input(|input| {
            input.pointer.delta() != egui::Vec2::ZERO || input.pointer.any_pressed()
        });
        let hover_chrome =
            pointer.is_some_and(|point| top_rect.contains(point) || controls_rect.contains(point));
        if pointer_moved || hover_chrome {
            self.controls_at = Instant::now();
        }
        let controls_visible =
            !self.playing || self.controls_at.elapsed() < CONTROLS_TIMEOUT || hover_chrome;

        let fresh_key = egui::Id::new(FRESH_ID);
        let fresh = ctx.memory(|memory| memory.data.get_temp::<bool>(fresh_key).is_none());
        ctx.memory_mut(|memory| memory.data.insert_temp(fresh_key, true));

        let close =
            ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        let (space, left, right, mute_key) = ctx.input_mut(|input| {
            (
                input.consume_key(egui::Modifiers::NONE, egui::Key::Space),
                input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft),
                input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight),
                input.consume_key(egui::Modifiers::NONE, egui::Key::M),
            )
        });

        let texture = self.texture.clone();
        let error = self.error.clone();
        let title = self.title.clone();
        let playing = self.playing;
        let duration = self.duration;
        let position = self.position;
        let buffering = self.buffering;
        let muted = self.muted;
        let volume = self.volume;
        let name = if title.trim().is_empty() {
            lapinux_core::i18n::t("trailer.title")
        } else {
            title.as_str()
        };
        let mut toggle = space;
        let mut seek = None;
        if duration > 0.0 {
            if left {
                seek = Some(((position - 5.0) / duration).clamp(0.0, 1.0));
            }
            if right {
                seek = Some(((position + 5.0) / duration).clamp(0.0, 1.0));
            }
        }
        let mut next_seek_preview = self.seek_preview;
        let mut next_muted = muted;
        let mut next_volume = None;
        let mut close_from_ui = false;

        egui::Area::new(egui::Id::new("trailer-cinema"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                ui.painter().rect_filled(screen, 0.0, egui::Color32::BLACK);

                // Le clic à côté referme, sauf celui qui vient d'ouvrir la
                // visionneuse.
                if !fresh {
                    let bands = [
                        egui::Rect::from_min_max(
                            screen.min,
                            egui::pos2(rect.left(), screen.bottom()),
                        ),
                        egui::Rect::from_min_max(
                            egui::pos2(rect.right(), screen.top()),
                            screen.max,
                        ),
                        egui::Rect::from_min_max(
                            egui::pos2(rect.left(), screen.top()),
                            egui::pos2(rect.right(), rect.top()),
                        ),
                        egui::Rect::from_min_max(
                            egui::pos2(rect.left(), rect.bottom()),
                            egui::pos2(rect.right(), screen.bottom()),
                        ),
                    ];
                    for (index, band) in bands.into_iter().enumerate() {
                        if band.width() > 0.0
                            && band.height() > 0.0
                            && ui
                                .interact(
                                    band,
                                    egui::Id::new(("trailer-outside", index)),
                                    egui::Sense::click(),
                                )
                                .clicked()
                        {
                            close_from_ui = true;
                        }
                    }
                }

                let video = ui.interact(rect, egui::Id::new("trailer-video"), egui::Sense::click());
                let chrome_hit = controls_visible && hover_chrome;
                if video.clicked() && !chrome_hit {
                    toggle = true;
                }
                if video.hovered() && !chrome_hit {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }

                ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                    egui::Frame::none()
                        .fill(egui::Color32::BLACK)
                        .show(ui, |ui| {
                            ui.set_min_size(rect.size());
                            ui.set_max_size(rect.size());
                            ui.set_clip_rect(rect.intersect(ui.clip_rect()));
                            ui.painter().rect_filled(rect, 0.0, egui::Color32::BLACK);
                            if error.is_none() {
                                if let Some(texture) = &texture {
                                    crate::gameinfo::paint_fit(ui, rect, texture);
                                }
                            }
                            paint_scrims(ui.painter(), rect);

                            if error.is_none() && texture.is_none() {
                                ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                                    ui.vertical_centered(|ui| {
                                        ui.add_space((rect.height() * 0.5 - 24.0).max(12.0));
                                        crate::section_icons::loading(ui, 18.0);
                                        ui.add_space(8.0);
                                        let text = match buffering {
                                            Some(percent) => lapinux_core::i18n::t_args(
                                                "trailer.buffering",
                                                &[("percent", &percent.to_string())],
                                            ),
                                            None => {
                                                lapinux_core::i18n::t("trailer.loading").to_string()
                                            }
                                        };
                                        ui.label(
                                            egui::RichText::new(text).size(11.0).color(TEXT_DIM()),
                                        );
                                    });
                                });
                            }

                            if let Some(error) = &error {
                                ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                                    ui.vertical_centered(|ui| {
                                        ui.add_space((rect.height() * 0.5 - 34.0).max(12.0));
                                        ui.label(
                                            egui::RichText::new(lapinux_core::i18n::t(
                                                "ui.playback_failed",
                                            ))
                                            .size(16.0)
                                            .strong()
                                            .color(TEXT()),
                                        );
                                        ui.add_space(5.0);
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(error)
                                                    .size(11.5)
                                                    .color(TEXT_DIM()),
                                            )
                                            .wrap(),
                                        );
                                    });
                                });
                            } else if texture.is_some() && !playing {
                                let center = rect.center();
                                ui.painter().circle_filled(
                                    center,
                                    34.0,
                                    egui::Color32::from_black_alpha(160),
                                );
                                ui.painter().circle_stroke(
                                    center,
                                    34.0,
                                    egui::Stroke::new(1.0_f32, egui::Color32::from_white_alpha(35)),
                                );
                                paint_play_pause(ui.painter(), center, false, 14.0);
                            }

                            if controls_visible {
                                ui.scope_builder(egui::UiBuilder::new().max_rect(top_rect), |ui| {
                                    ui.horizontal(|ui| {
                                        let reserve = 52.0;
                                        let title_width =
                                            (ui.available_width() - reserve).max(120.0);
                                        ui.allocate_ui_with_layout(
                                            egui::vec2(title_width, 52.0),
                                            egui::Layout::top_down(egui::Align::Min),
                                            |ui| {
                                                if !title.trim().is_empty() {
                                                    ui.label(
                                                        egui::RichText::new(lapinux_core::i18n::t(
                                                            "ui.trailer",
                                                        ))
                                                        .size(9.0)
                                                        .strong()
                                                        .color(TEXT_DIM()),
                                                    );
                                                    ui.add_space(2.0);
                                                }
                                                ui.add(
                                                    egui::Label::new(
                                                        egui::RichText::new(name)
                                                            .size(18.0)
                                                            .strong()
                                                            .color(TEXT()),
                                                    )
                                                    .truncate(),
                                                );
                                            },
                                        );
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                if round_button(
                                                    ui,
                                                    30.0,
                                                    lapinux_core::i18n::t("game_sheet.close"),
                                                    |painter, center| {
                                                        paint_close(painter, center, 8.0);
                                                    },
                                                ) {
                                                    close_from_ui = true;
                                                }
                                            },
                                        );
                                    });
                                });

                                ui.scope_builder(egui::UiBuilder::new().max_rect(bar), |ui| {
                                    ui.set_width(bar.width());
                                    let (preview, wanted) = timeline(
                                        ui,
                                        if duration > 0.0 {
                                            (position / duration).clamp(0.0, 1.0) as f32
                                        } else {
                                            0.0
                                        },
                                        next_seek_preview.map(|value| value as f32),
                                        duration,
                                    );
                                    next_seek_preview = preview.map(f64::from);
                                    if let Some(value) = wanted {
                                        seek = Some(f64::from(value));
                                    }
                                    ui.add_space(4.0);
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 8.0;
                                        if round_button(
                                            ui,
                                            30.0,
                                            if playing {
                                                "Pause"
                                            } else {
                                                lapinux_core::i18n::t("ui.play")
                                            },
                                            |painter, center| {
                                                paint_play_pause(painter, center, playing, 10.0);
                                            },
                                        ) {
                                            toggle = true;
                                        }
                                        let shown = next_seek_preview.unwrap_or(position);
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "{}  /  {}",
                                                clock(shown),
                                                clock(duration)
                                            ))
                                            .size(11.0)
                                            .color(TEXT_DIM()),
                                        );
                                        ui.add_space(4.0);
                                        if round_button(
                                            ui,
                                            28.0,
                                            if next_muted {
                                                lapinux_core::i18n::t("trailer.unmute")
                                            } else {
                                                lapinux_core::i18n::t("ui.mute")
                                            },
                                            |painter, center| {
                                                paint_speaker(painter, center, next_muted, 10.0);
                                            },
                                        ) {
                                            next_muted = !next_muted;
                                        }
                                        if let Some(value) = volume_bar(ui, volume, next_muted) {
                                            next_volume = Some(value);
                                            next_muted = false;
                                        }
                                    });
                                });
                            }
                        });
                });
            });

        self.seek_preview = next_seek_preview;
        if toggle && error.is_none() {
            self.send(serde_json::json!({
                "cmd": if playing { "pause" } else { "play" }
            }));
            self.playing = !playing;
        }
        if let Some(value) = seek {
            self.send(serde_json::json!({ "cmd": "seek", "value": value }));
            self.position = value * duration;
        }
        if mute_key {
            next_muted = !next_muted;
        }
        if next_muted != muted {
            self.muted = next_muted;
            self.send(serde_json::json!({ "cmd": "mute", "value": next_muted }));
        }
        if let Some(value) = next_volume {
            self.volume = value;
            self.muted = false;
            self.send(serde_json::json!({ "cmd": "volume", "value": value }));
        }
        if close || close_from_ui {
            ctx.memory_mut(|memory| memory.data.remove::<bool>(fresh_key));
            self.close();
        }
    }
}

/// La visionneuse occupe toute la surface qui lui est confiée ; la vidéo garde
/// ses proportions à l'intérieur, sur un fond noir.
fn viewer_rect(screen: egui::Rect) -> egui::Rect {
    screen
}

/// Voile dégradé en haut et en bas : le titre et les commandes restent
/// lisibles sur n'importe quelle image.
fn paint_scrims(painter: &egui::Painter, rect: egui::Rect) {
    const STEPS: usize = 14;
    let top = 96.0_f32.min(rect.height() * 0.3);
    for index in 0..STEPS {
        let t0 = index as f32 / STEPS as f32;
        let t1 = (index + 1) as f32 / STEPS as f32;
        let band = egui::Rect::from_min_max(
            egui::pos2(rect.left(), rect.top() + top * t0),
            egui::pos2(rect.right(), rect.top() + top * t1 + 1.0),
        );
        let alpha = ((1.0 - t0) * 150.0) as u8;
        painter.rect_filled(band, 0.0, egui::Color32::from_black_alpha(alpha));
    }
    let bottom = 118.0_f32.min(rect.height() * 0.35);
    for index in 0..STEPS {
        let t0 = index as f32 / STEPS as f32;
        let t1 = (index + 1) as f32 / STEPS as f32;
        let band = egui::Rect::from_min_max(
            egui::pos2(rect.left(), rect.bottom() - bottom + bottom * t0),
            egui::pos2(rect.right(), rect.bottom() - bottom + bottom * t1 + 1.0),
        );
        let alpha = (t0 * 190.0) as u8;
        painter.rect_filled(band, 0.0, egui::Color32::from_black_alpha(alpha));
    }
}

/// Bouton rond transparent, avec une icône dessinée au trait ou en aplat.
fn round_button(
    ui: &mut egui::Ui,
    size: f32,
    tip: &str,
    icon: impl FnOnce(&egui::Painter, egui::Pos2),
) -> bool {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click());
    let fill = if response.hovered() {
        egui::Color32::from_white_alpha(38)
    } else {
        egui::Color32::from_black_alpha(110)
    };
    ui.painter().circle_filled(rect.center(), size * 0.5, fill);
    icon(ui.painter(), rect.center());
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.on_hover_text(tip).clicked()
}

fn paint_play_pause(painter: &egui::Painter, center: egui::Pos2, playing: bool, size: f32) {
    if playing {
        for offset in [-size * 0.22, size * 0.22] {
            painter.rect_filled(
                egui::Rect::from_center_size(
                    center + egui::vec2(offset, 0.0),
                    egui::vec2(size * 0.28, size * 1.35),
                ),
                egui::Rounding::same(1.0),
                TEXT(),
            );
        }
    } else {
        painter.add(egui::Shape::convex_polygon(
            vec![
                center + egui::vec2(-size * 0.42, -size * 0.68),
                center + egui::vec2(-size * 0.42, size * 0.68),
                center + egui::vec2(size * 0.75, 0.0),
            ],
            TEXT(),
            egui::Stroke::NONE,
        ));
    }
}

fn paint_close(painter: &egui::Painter, center: egui::Pos2, size: f32) {
    let stroke = egui::Stroke::new(1.6_f32, TEXT());
    painter.line_segment(
        [
            center + egui::vec2(-size, -size),
            center + egui::vec2(size, size),
        ],
        stroke,
    );
    painter.line_segment(
        [
            center + egui::vec2(size, -size),
            center + egui::vec2(-size, size),
        ],
        stroke,
    );
}

fn paint_speaker(painter: &egui::Painter, center: egui::Pos2, muted: bool, size: f32) {
    let fill = TEXT();
    painter.add(egui::Shape::convex_polygon(
        vec![
            center + egui::vec2(-size * 0.9, -size * 0.28),
            center + egui::vec2(-size * 0.5, -size * 0.28),
            center + egui::vec2(-size * 0.05, -size * 0.72),
            center + egui::vec2(-size * 0.05, size * 0.72),
            center + egui::vec2(-size * 0.5, size * 0.28),
            center + egui::vec2(-size * 0.9, size * 0.28),
        ],
        fill,
        egui::Stroke::NONE,
    ));
    if muted {
        painter.line_segment(
            [
                center + egui::vec2(size * 0.15, -size * 0.55),
                center + egui::vec2(size * 0.85, size * 0.55),
            ],
            egui::Stroke::new(1.6_f32, TEXT()),
        );
        painter.line_segment(
            [
                center + egui::vec2(size * 0.85, -size * 0.55),
                center + egui::vec2(size * 0.15, size * 0.55),
            ],
            egui::Stroke::new(1.6_f32, TEXT()),
        );
    } else {
        for radius in [size * 0.42, size * 0.78] {
            painter.add(egui::Shape::line(
                vec![
                    center + egui::vec2(size * 0.2, -radius * 0.72),
                    center + egui::vec2(size * 0.38, -radius * 0.35),
                    center + egui::vec2(size * 0.38, radius * 0.35),
                    center + egui::vec2(size * 0.2, radius * 0.72),
                ],
                egui::Stroke::new(1.25_f32, TEXT()),
            ));
        }
    }
}

/// Curseur de progression dessiné à la main : une piste discrète, un
/// remplissage clair et une poignée qui ne grossit qu'au survol.
fn timeline(
    ui: &mut egui::Ui,
    progress: f32,
    preview: Option<f32>,
    duration: f64,
) -> (Option<f32>, Option<f32>) {
    let width = ui.available_width().max(80.0);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, 20.0), egui::Sense::click_and_drag());
    let mut next = preview;
    let mut wanted = None;
    if (response.dragged() || response.clicked()) && duration > 0.0 {
        if let Some(pointer) = response.interact_pointer_pos() {
            let value = ((pointer.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
            next = Some(value);
            if response.clicked() {
                wanted = Some(value);
                next = None;
            }
        }
    }
    if response.drag_stopped() {
        wanted = next;
        next = None;
    }
    let shown = next.unwrap_or(progress).clamp(0.0, 1.0);
    let track = egui::Rect::from_center_size(rect.center(), egui::vec2(rect.width(), 4.0));
    ui.painter()
        .rect_filled(track, 2.0, egui::Color32::from_white_alpha(45));
    ui.painter().rect_filled(
        egui::Rect::from_min_size(track.min, egui::vec2(track.width() * shown, track.height())),
        2.0,
        TEXT(),
    );
    let knob = egui::pos2(track.left() + track.width() * shown, track.center().y);
    let radius = if response.hovered() || response.dragged() {
        6.0
    } else {
        4.0
    };
    ui.painter().circle_filled(knob, radius, TEXT());
    if response.hovered() {
        let response = response.on_hover_text(format!(
            "{} / {}",
            clock(f64::from(shown) * duration),
            clock(duration)
        ));
        let _ = response;
    }
    (next, wanted)
}

/// Petit curseur de volume, dans le même langage que la ligne de temps.
fn volume_bar(ui: &mut egui::Ui, volume: f32, muted: bool) -> Option<f32> {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(66.0, 20.0), egui::Sense::click_and_drag());
    let shown = if muted { 0.0 } else { volume };
    let mut wanted = None;
    if response.dragged() || response.clicked() {
        if let Some(pointer) = response.interact_pointer_pos() {
            wanted = Some(((pointer.x - rect.left()) / rect.width()).clamp(0.0, 1.0));
        }
    }
    let value = wanted.unwrap_or(shown);
    let track = egui::Rect::from_center_size(rect.center(), egui::vec2(rect.width(), 3.0));
    ui.painter()
        .rect_filled(track, 1.5, egui::Color32::from_white_alpha(40));
    ui.painter().rect_filled(
        egui::Rect::from_min_size(track.min, egui::vec2(track.width() * value, track.height())),
        1.5,
        TEXT(),
    );
    let knob = egui::pos2(track.left() + track.width() * value, track.center().y);
    ui.painter().circle_filled(knob, 4.0, TEXT());
    if response.hovered() {
        let _ = response.on_hover_text("Volume");
    }
    wanted
}

/// `m:ss`, ou `h:mm:ss` pour les films longs.
fn clock(seconds: f64) -> String {
    let total = seconds.max(0.0).round() as u64;
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

fn spawn(paths: &LapinuxPaths, ctx: &egui::Context, url: &str) -> Result<Session, String> {
    let dir = paths.run_dir.join("trailer");
    std::fs::create_dir_all(&dir).unwrap_or(());
    let script = dir.join("player.py");
    let script = if std::fs::write(&script, HELPER).is_ok() {
        script
    } else {
        let fallback = std::env::temp_dir().join("lapinux-trailer.py");
        std::fs::write(&fallback, HELPER).map_err(|error| {
            lapinux_core::i18n::t_args("trailer.not_installable", &[("error", &error.to_string())])
        })?;
        fallback
    };

    let mut child = Command::new("python3")
        .arg(&script)
        .arg(url)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("python3 introuvable ({error})"))?;
    let Some(stdin) = child.stdin.take() else {
        let _ = child.kill();
        return Err(lapinux_core::i18n::t("trailer.input_unavailable").into());
    };
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        return Err(lapinux_core::i18n::t("trailer.player_unavailable").into());
    };

    let (tx, rx) = mpsc::channel();
    let frame = Arc::new(Mutex::new(None));
    let frame_slot = Arc::clone(&frame);
    let repaint = ctx.clone();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut header = Vec::new();
        loop {
            header.clear();
            match reader.read_until(b'\n', &mut header) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let line = String::from_utf8_lossy(&header);
            let line = line.trim_end();
            if let Some(rest) = line.strip_prefix("F ") {
                let mut parts = rest.split(' ');
                let (Some(width), Some(height), Some(length)) =
                    (parts.next(), parts.next(), parts.next())
                else {
                    break;
                };
                let (Ok(width), Ok(height), Ok(length)) = (
                    width.parse::<usize>(),
                    height.parse::<usize>(),
                    length.parse::<usize>(),
                ) else {
                    break;
                };
                if length > 32 * 1024 * 1024 {
                    break;
                }
                let mut rgba = vec![0u8; length];
                if reader.read_exact(&mut rgba).is_err() {
                    break;
                }
                if width > 0
                    && height > 0
                    && width
                        .checked_mul(height)
                        .and_then(|size| size.checked_mul(4))
                        == Some(length)
                {
                    if let Ok(mut slot) = frame_slot.lock() {
                        *slot = Some(Frame {
                            width,
                            height,
                            rgba,
                        });
                    }
                    repaint.request_repaint();
                }
            } else if let Some(rest) = line.strip_prefix("E ") {
                let Ok(value) = serde_json::from_str::<serde_json::Value>(rest) else {
                    continue;
                };
                let event = match value.get("event").and_then(|event| event.as_str()) {
                    Some("time") => Event::Time {
                        position: value
                            .get("position")
                            .and_then(|value| value.as_f64())
                            .unwrap_or(0.0),
                        duration: value
                            .get("duration")
                            .and_then(|value| value.as_f64())
                            .unwrap_or(0.0),
                        playing: value
                            .get("playing")
                            .and_then(|value| value.as_bool())
                            .unwrap_or(false),
                    },
                    Some("buffering") => Event::Buffering(
                        value
                            .get("percent")
                            .and_then(|value| value.as_u64())
                            .unwrap_or(0) as u32,
                    ),
                    Some("error") => Event::Error(
                        value
                            .get("message")
                            .and_then(|value| value.as_str())
                            .unwrap_or("lecture impossible")
                            .to_string(),
                    ),
                    Some("ended") => Event::Ended,
                    _ => continue,
                };
                if tx.send(event).is_err() {
                    break;
                }
                repaint.request_repaint();
            } else if line.is_empty() {
                continue;
            } else {
                break;
            }
        }
        let _ = tx.send(Event::Closed);
    });

    Ok(Session {
        child,
        stdin,
        events: rx,
        frame,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cinema_takes_the_area_it_is_given() {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1360.0, 860.0));
        assert_eq!(viewer_rect(screen), screen);
    }

    #[test]
    fn the_clock_reads_like_a_player() {
        assert_eq!(clock(0.0), "0:00");
        assert_eq!(clock(65.0), "1:05");
        assert_eq!(clock(3_661.0), "1:01:01");
    }

    #[test]
    fn the_cinema_panel_paints_its_loading_state() {
        let ctx = egui::Context::default();
        crate::apply_theme(&ctx);
        crate::install_fonts(&ctx);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1360.0, 860.0));
        let mut trailer = Trailer::default();
        trailer.visible = true;
        trailer.title = "007 First Light".into();
        let mut seen = Vec::new();
        for _ in 0..3 {
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ctx| trailer.panel(ctx, screen),
            );
            seen.clear();
            fn walk(shape: &egui::Shape, out: &mut Vec<String>) {
                match shape {
                    egui::Shape::Text(text) => out.push(text.galley.text().to_string()),
                    egui::Shape::Vec(shapes) => {
                        for shape in shapes {
                            walk(shape, out);
                        }
                    }
                    _ => {}
                }
            }
            for clipped in &output.shapes {
                walk(&clipped.shape, &mut seen);
            }
        }
        for expected in [
            lapinux_core::i18n::t("ui.trailer"),
            "007 First Light",
            lapinux_core::i18n::t("trailer.loading"),
            "0:00  /  0:00",
        ] {
            assert!(
                seen.iter().any(|text| text == expected),
                "« {expected} » manque : {seen:?}"
            );
        }
    }
}
