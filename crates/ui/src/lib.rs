//! Interface graphique egui : bibliothèque, téléchargements, Protons, manettes, réglages.

use eframe::egui;
use ludus_core::client::Client;
use ludus_core::config::Config;
use ludus_core::ipc::Req;
use ludus_core::model::{
    CheckStatus, DeviceInfo, DoctorReport, Game, GameSettings, Job, JobState, ProtonInfo,
    ProtonRelease, SessionInfo,
};
use ludus_core::paths::LudusPaths;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const ACCENT: egui::Color32 = egui::Color32::from_rgb(110, 168, 254);
const PANEL: egui::Color32 = egui::Color32::from_rgb(22, 24, 29);
const CARD: egui::Color32 = egui::Color32::from_rgb(28, 31, 38);

pub fn run() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1360.0, 860.0])
            .with_min_inner_size([980.0, 640.0])
            .with_title("Ludus"),
        vsync: true,
        ..Default::default()
    };
    eframe::run_native(
        "Ludus",
        options,
        Box::new(|cc| Ok(Box::new(LudusApp::new(cc)))),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Library,
    Downloads,
    Protons,
    Controllers,
    Settings,
    Doctor,
}

struct Toast {
    text: String,
    until: Instant,
}

struct CoverCache {
    textures: HashMap<u32, Option<egui::TextureHandle>>,
    requested: HashMap<u32, Instant>,
}

impl CoverCache {
    fn new() -> Self {
        Self {
            textures: HashMap::new(),
            requested: HashMap::new(),
        }
    }

    fn get(&mut self, ctx: &egui::Context, paths: &LudusPaths, appid: u32) -> Option<egui::TextureHandle> {
        if let Some(entry) = self.textures.get(&appid) {
            return entry.clone();
        }
        let dir = paths.covers_dir();
        let path = dir.join(format!("{appid}.jpg"));
        if path.is_file() {
            match std::fs::read(&path)
                .ok()
                .and_then(|bytes| image::load_from_memory(&bytes).ok())
            {
                Some(image) => {
                    let rgba = image.to_rgba8();
                    let size = [rgba.width() as usize, rgba.height() as usize];
                    let color = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
                    let texture = ctx.load_texture(
                        format!("cover-{appid}"),
                        color,
                        egui::TextureOptions::LINEAR,
                    );
                    self.textures.insert(appid, Some(texture.clone()));
                    return Some(texture);
                }
                None => {
                    self.textures.insert(appid, None);
                    return None;
                }
            }
        }
        let now = Instant::now();
        let should_request = self
            .requested
            .get(&appid)
            .map(|t| now.duration_since(*t) > Duration::from_secs(30))
            .unwrap_or(true);
        if should_request {
            self.requested.insert(appid, now);
            std::fs::create_dir_all(&dir).ok();
            let url = format!(
                "https://cdn.cloudflare.steamstatic.com/steam/apps/{appid}/library_600x900.jpg"
            );
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                if let Ok(response) = ureq::get(&url).call() {
                    let mut reader = response.into_reader();
                    let mut bytes = Vec::new();
                    if std::io::Read::read_to_end(&mut reader, &mut bytes).is_ok()
                        && bytes.len() > 1024
                    {
                        let _ = std::fs::write(&path, &bytes);
                    }
                }
                std::thread::sleep(Duration::from_millis(120));
                ctx.request_repaint();
            });
        }
        None
    }
}

struct LudusApp {
    client: Client,
    paths: LudusPaths,
    page: Page,
    config: Config,
    games: Vec<Game>,
    protons: Vec<ProtonInfo>,
    releases: Vec<ProtonRelease>,
    jobs: Vec<Job>,
    session: Option<SessionInfo>,
    devices: Vec<DeviceInfo>,
    doctor: Option<DoctorReport>,
    selected: Option<u32>,
    search: String,
    install_appid: String,
    toast: Option<Toast>,
    dry_run: Option<String>,
    covers: CoverCache,
    nav: Option<ludus_input::NavReader>,
    big_picture: bool,
    last_refresh: Instant,
    tool_status: Option<serde_json::Value>,
    busy: bool,
}

impl LudusApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let paths = LudusPaths::discover();
        let client = Client::new(&paths);
        let config = client
            .request(&Req::Config)
            .ok()
            .and_then(|e| e.into_data::<Config>().ok())
            .unwrap_or_default();
        let big_picture = config.general.big_picture;
        let mut app = Self {
            client,
            paths,
            page: Page::Library,
            config,
            games: Vec::new(),
            protons: Vec::new(),
            releases: Vec::new(),
            jobs: Vec::new(),
            session: None,
            devices: Vec::new(),
            doctor: None,
            selected: None,
            search: String::new(),
            install_appid: String::new(),
            toast: None,
            dry_run: None,
            covers: CoverCache::new(),
            nav: ludus_input::NavReader::start().ok(),
            big_picture,
            last_refresh: Instant::now() - Duration::from_secs(10),
            tool_status: None,
            busy: false,
        };
        app.refresh(true);
        app
    }

    fn notify(&mut self, text: impl Into<String>) {
        self.toast = Some(Toast {
            text: text.into(),
            until: Instant::now() + Duration::from_secs(6),
        });
    }

    fn refresh(&mut self, rescan: bool) {
        if rescan {
            let _ = self.client.request(&Req::Scan);
        }
        self.games = self
            .client
            .request(&Req::Games)
            .ok()
            .and_then(|e| e.into_data().ok())
            .unwrap_or_default();
        self.protons = self
            .client
            .request(&Req::Protons)
            .ok()
            .and_then(|e| e.into_data().ok())
            .unwrap_or_default();
        self.jobs = self
            .client
            .request(&Req::Jobs)
            .ok()
            .and_then(|e| e.into_data().ok())
            .unwrap_or_default();
        self.session = self
            .client
            .request(&Req::Session)
            .ok()
            .and_then(|e| e.into_data().ok())
            .flatten();
        self.devices = self
            .client
            .request(&Req::Devices)
            .ok()
            .and_then(|e| e.into_data().ok())
            .unwrap_or_default();
        self.config = self
            .client
            .request(&Req::Config)
            .ok()
            .and_then(|e| e.into_data().ok())
            .unwrap_or_else(|| self.config.clone());
        self.tool_status = self
            .client
            .request(&Req::Tools)
            .ok()
            .map(|e| e.data);
        self.last_refresh = Instant::now();
    }

    fn save_settings(&mut self, appid: u32, settings: GameSettings) {
        self.busy = true;
        match self.client.request(&Req::SaveSettings { appid, settings }) {
            Ok(envelope) if envelope.ok => self.notify("réglages enregistrés"),
            Ok(envelope) => self.notify(envelope.error.unwrap_or_else(|| "erreur".into())),
            Err(err) => self.notify(format!("{err}")),
        }
        self.busy = false;
    }

    fn launch(&mut self, appid: u32, dry_run: bool) {
        match self.client.request(&Req::Launch { appid, dry_run }) {
            Ok(envelope) if envelope.ok => {
                if dry_run {
                    self.dry_run = Some(format!(
                        "couches : {}\n\n{}",
                        envelope.data["layers"].as_str().unwrap_or("?"),
                        envelope.data["command"].as_str().unwrap_or("")
                    ));
                } else {
                    let note = envelope.data["note"].as_str().unwrap_or("");
                    self.notify(format!("session démarrée {note}"));
                }
            }
            Ok(envelope) => self.notify(envelope.error.unwrap_or_else(|| "échec".into())),
            Err(err) => self.notify(format!("{err}")),
        }
    }

    fn handle_gamepad(&mut self, ctx: &egui::Context) {
        let Some(nav) = self.nav.as_ref() else {
            return;
        };
        let events: Vec<ludus_input::Nav> = nav.events.try_iter().collect();
        if events.is_empty() {
            return;
        }
        for event in events {
            match event {
                ludus_input::Nav::Confirm => {
                    if let Some(appid) = self.selected {
                        self.launch(appid, false);
                    }
                }
                ludus_input::Nav::Back => self.selected = None,
                ludus_input::Nav::Menu => {
                    let _ = self.client.request(&Req::OverlayToggle);
                }
                ludus_input::Nav::Left | ludus_input::Nav::Right => {
                    let step = if event == ludus_input::Nav::Left { -1 } else { 1 };
                    self.move_selection(step, ctx);
                }
                ludus_input::Nav::Up | ludus_input::Nav::Down => {
                    let step = if event == ludus_input::Nav::Up { -4 } else { 4 };
                    self.move_selection(step, ctx);
                }
            }
        }
    }

    fn move_selection(&mut self, delta: i32, _ctx: &egui::Context) {
        let visible: Vec<u32> = self
            .games
            .iter()
            .filter(|g| self.matches_search(g))
            .map(|g| g.appid)
            .collect();
        if visible.is_empty() {
            return;
        }
        let current = self
            .selected
            .and_then(|appid| visible.iter().position(|id| *id == appid))
            .unwrap_or(0) as i32;
        let next = (current + delta).clamp(0, visible.len() as i32 - 1) as usize;
        self.selected = Some(visible[next]);
    }

    fn matches_search(&self, game: &Game) -> bool {
        self.search.trim().is_empty()
            || game
                .name
                .to_ascii_lowercase()
                .contains(&self.search.to_ascii_lowercase())
    }

    fn sidebar(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("sidebar")
            .exact_width(if self.big_picture { 240.0 } else { 196.0 })
            .frame(egui::Frame::none().fill(PANEL).inner_margin(16.0))
            .show(ctx, |ui| {
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("LUDUS")
                        .color(ACCENT)
                        .size(26.0)
                        .strong(),
                );
                ui.label(
                    egui::RichText::new("gestionnaire de jeux")
                        .size(11.0)
                        .color(egui::Color32::GRAY),
                );
                ui.add_space(24.0);
                let entries = [
                    (Page::Library, "⌂  Bibliothèque"),
                    (Page::Downloads, "↓  Téléchargements"),
                    (Page::Protons, "⚙  Protons"),
                    (Page::Controllers, "◉  Manettes"),
                    (Page::Settings, "☰  Réglages"),
                    (Page::Doctor, "✓  Diagnostic"),
                ];
                for (page, label) in entries {
                    let selected = self.page == page;
                    let button = egui::Button::new(
                        egui::RichText::new(label).size(14.0).color(if selected {
                            egui::Color32::WHITE
                        } else {
                            egui::Color32::from_gray(190)
                        }),
                    )
                    .fill(if selected { CARD } else { egui::Color32::TRANSPARENT })
                    .rounding(egui::Rounding::same(8.0))
                    .min_size(egui::vec2(ui.available_width(), 38.0));
                    if ui.add(button).clicked() {
                        self.page = page;
                    }
                }
                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    let label = if self.big_picture {
                        "quitter le mode console"
                    } else {
                        "mode console"
                    };
                    if ui.button(label).clicked() {
                        self.big_picture = !self.big_picture;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.big_picture));
                    }
                    ui.add_space(6.0);
                    match &self.session {
                        Some(session) => {
                            ui.label(
                                egui::RichText::new(format!("▶ {}", session.name))
                                    .color(egui::Color32::from_rgb(130, 220, 150)),
                            );
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} min",
                                    (ludus_core::util::now_unix() - session.started) / 60
                                ))
                                .size(11.0)
                                .color(egui::Color32::GRAY),
                            );
                        }
                        None => {
                            ui.label(
                                egui::RichText::new("aucune session")
                                    .size(11.0)
                                    .color(egui::Color32::GRAY),
                            );
                        }
                    }
                });
            });
    }

    fn library_page(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("library_top")
            .frame(egui::Frame::none().inner_margin(egui::Margin::symmetric(20.0, 12.0)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Bibliothèque")
                            .size(22.0)
                            .strong(),
                    );
                    ui.add_space(12.0);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.search)
                            .hint_text("rechercher…")
                            .desired_width(240.0),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("resynchroniser").clicked() {
                            self.refresh(true);
                            self.notify("bibliothèques scannées");
                        }
                        ui.label(
                            egui::RichText::new(format!(
                                "{} jeux · {} installés",
                                self.games.len(),
                                self.games.iter().filter(|g| g.installed).count()
                            ))
                            .size(12.0)
                            .color(egui::Color32::GRAY),
                        );
                    });
                });
            });

        egui::SidePanel::right("game_detail")
            .exact_width(if self.big_picture { 380.0 } else { 336.0 })
            .frame(egui::Frame::none().fill(PANEL).inner_margin(16.0))
            .show(ctx, |ui| {
                self.detail_panel(ui);
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::none().inner_margin(16.0))
            .show(ctx, |ui| {
                let games: Vec<Game> = self
                    .games
                    .iter()
                    .filter(|g| self.matches_search(g))
                    .cloned()
                    .collect();
                if games.is_empty() {
                    ui.label(
                        "Aucun jeu. Vérifiez la racine de bibliothèque dans les réglages, puis « resynchroniser ».",
                    );
                    return;
                }
                let mut clicked: Option<u32> = None;
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        for game in &games {
                            let texture = self.covers.get(ctx, &self.paths, game.appid);
                            if game_card(ui, game, texture.as_ref(), self.selected == Some(game.appid))
                                .clicked()
                            {
                                clicked = Some(game.appid);
                            }
                        }
                    });
                });
                if let Some(appid) = clicked {
                    self.selected = Some(appid);
                }
            });
    }

    fn detail_panel(&mut self, ui: &mut egui::Ui) {
        let Some(appid) = self.selected else {
            ui.add_space(40.0);
            ui.label(
                egui::RichText::new("Sélectionnez un jeu")
                    .size(16.0)
                    .color(egui::Color32::GRAY),
            );
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(
                    "La manette fonctionne aussi : croix directionnelle pour naviguer, A pour lancer.",
                )
                .size(12.0)
                .color(egui::Color32::GRAY),
            );
            return;
        };
        let Some(index) = self.games.iter().position(|g| g.appid == appid) else {
            self.selected = None;
            return;
        };
        let game = self.games[index].clone();
        ui.label(egui::RichText::new(&game.name).size(20.0).strong());
        ui.horizontal(|ui| {
            let drm_color = match game.drm {
                ludus_core::model::Drm::SteamStub => egui::Color32::from_rgb(235, 140, 140),
                ludus_core::model::Drm::Steamworks => egui::Color32::from_rgb(220, 200, 120),
                _ => egui::Color32::from_rgb(130, 210, 150),
            };
            ui.label(
                egui::RichText::new(game.drm.label())
                    .size(11.0)
                    .color(drm_color),
            );
            ui.label(
                egui::RichText::new(if game.native { "Linux natif" } else { "Proton" })
                    .size(11.0)
                    .color(egui::Color32::GRAY),
            );
            ui.label(
                egui::RichText::new(format!("#{}", game.appid))
                    .size(11.0)
                    .color(egui::Color32::GRAY),
            );
        });
        ui.add_space(10.0);
        if !game.installed {
            ui.label(
                egui::RichText::new("Non installé")
                    .color(egui::Color32::from_rgb(235, 180, 120)),
            );
        }

        let mut settings = game.settings.clone();
        let mut save = false;

        let proton_names: Vec<String> = self.protons.iter().map(|p| p.name.clone()).collect();
        let mut proton_choice = settings.proton.clone().unwrap_or_else(|| "auto".into());
        egui::Grid::new("game_grid")
            .num_columns(2)
            .spacing([10.0, 8.0])
            .show(ui, |ui| {
                ui.label("Proton");
                egui::ComboBox::from_id_salt("proton_choice")
                    .selected_text(&proton_choice)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut proton_choice, "auto".to_string(), "auto");
                        for name in &proton_names {
                            ui.selectable_value(&mut proton_choice, name.clone(), name);
                        }
                    });
                ui.end_row();

                ui.label("Jeu");
                if game.installed {
                    if ui.button("▶  Jouer").clicked() {
                        self.launch(game.appid, false);
                    }
                } else {
                    ui.label(egui::RichText::new("installez d'abord").size(11.0));
                }
                ui.end_row();

                ui.label("Commande");
                if ui.button("afficher (dry-run)").clicked() {
                    self.launch(game.appid, true);
                }
                ui.end_row();
            });
        settings.proton = (proton_choice != "auto").then_some(proton_choice);

        ui.add_space(8.0);
        ui.separator();
        let gamescope = settings
            .gamescope
            .unwrap_or(self.config.launch.gamescope);
        let mut gamescope_value = gamescope;
        ui.checkbox(&mut gamescope_value, "gamescope");
        settings.gamescope = Some(gamescope_value);

        let mangohud = settings.mangohud.unwrap_or(self.config.launch.mangohud);
        let mut mangohud_value = mangohud;
        ui.checkbox(&mut mangohud_value, "MangoHud");
        settings.mangohud = Some(mangohud_value);

        let mut fps = settings.fps_cap.unwrap_or(self.config.launch.fps_cap);
        ui.horizontal(|ui| {
            ui.label("limite FPS");
            ui.add(egui::Slider::new(&mut fps, 30..=240).clamping(egui::SliderClamping::Always));
        });
        settings.fps_cap = Some(fps);

        ui.add_space(8.0);
        ui.label(egui::RichText::new("Variables d'environnement").size(12.0));
        let mut env_text = settings
            .env
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("\n");
        if ui
            .add(
                egui::TextEdit::multiline(&mut env_text)
                    .desired_rows(2)
                    .hint_text("DXVK_HUD=0"),
            )
            .changed()
        {
            settings.env = env_text
                .lines()
                .filter_map(|line| {
                    let (key, value) = line.split_once('=')?;
                    let key = key.trim();
                    if key.is_empty() {
                        None
                    } else {
                        Some((key.to_string(), value.trim().to_string()))
                    }
                })
                .collect();
            save = true;
        }

        ui.add_space(12.0);
        if ui.button("Enregistrer les réglages").clicked() {
            save = true;
        }
        if save {
            self.games[index].settings = settings.clone();
            self.save_settings(game.appid, settings);
        }

        ui.add_space(16.0);
        ui.separator();
        egui::Grid::new("game_info")
            .num_columns(2)
            .spacing([10.0, 4.0])
            .show(ui, |ui| {
                ui.label(egui::RichText::new("Dossier").size(11.0).color(egui::Color32::GRAY));
                ui.label(
                    egui::RichText::new(ludus_core::util::ellipsize(
                        &game.install_dir().display().to_string(),
                        44,
                    ))
                    .size(11.0),
                );
                ui.end_row();
                ui.label(egui::RichText::new("Temps de jeu").size(11.0).color(egui::Color32::GRAY));
                ui.label(
                    egui::RichText::new(ludus_core::util::format_duration(game.playtime_secs))
                        .size(11.0),
                );
                ui.end_row();
                ui.label(egui::RichText::new("Taille").size(11.0).color(egui::Color32::GRAY));
                ui.label(
                    egui::RichText::new(ludus_core::util::format_bytes(game.size_on_disk))
                        .size(11.0),
                );
                ui.end_row();
            });
    }

    fn downloads_page(&mut self, ui: &mut egui::Ui) {
        ui.heading("Téléchargements");
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(
                "Les jeux sont téléchargés avec SteamCMD (compte Steam + 2FA requis) dans la racine de bibliothèque. Le mot de passe n'est jamais stocké par Ludus.",
            )
            .size(11.0)
            .color(egui::Color32::GRAY),
        );
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.label("appid Steam");
            ui.add(
                egui::TextEdit::singleline(&mut self.install_appid)
                    .desired_width(100.0)
                    .hint_text("440"),
            );
            if ui.button("Installer").clicked() {
                if let Ok(appid) = self.install_appid.trim().parse::<u32>() {
                    match self.client.request(&Req::InstallGame { appid }) {
                        Ok(envelope) if envelope.ok => {
                            self.notify(format!("téléchargement de {appid} lancé"))
                        }
                        Ok(envelope) => {
                            self.notify(envelope.error.unwrap_or_else(|| "échec".into()))
                        }
                        Err(err) => self.notify(format!("{err}")),
                    }
                } else {
                    self.notify("appid invalide");
                }
            }
        });
        ui.add_space(16.0);
        if self.jobs.is_empty() {
            ui.label("Aucune tâche en cours.");
            return;
        }
        let jobs = self.jobs.clone();
        for job in jobs {
            egui::Frame::none()
                .fill(CARD)
                .rounding(egui::Rounding::same(10.0))
                .inner_margin(12.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(&job.label).strong());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                egui::RichText::new(job.state.label())
                                    .size(11.0)
                                    .color(match job.state {
                                        JobState::Done => egui::Color32::from_rgb(130, 210, 150),
                                        JobState::Failed => egui::Color32::from_rgb(230, 130, 130),
                                        _ => egui::Color32::GRAY,
                                    }),
                            );
                        });
                    });
                    ui.add(
                        egui::ProgressBar::new(job.progress)
                            .text(format!("{:.0}%", job.progress * 100.0))
                            .desired_height(14.0),
                    );
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(&job.message)
                                .size(11.0)
                                .color(egui::Color32::GRAY),
                        );
                        if matches!(job.state, JobState::Running | JobState::Queued)
                            && ui.small_button("annuler").clicked()
                        {
                            let _ = self.client.request(&Req::CancelJob { id: job.id.clone() });
                        }
                    });
                });
            ui.add_space(8.0);
        }
    }

    fn protons_page(&mut self, ui: &mut egui::Ui) {
        ui.heading("Protons");
        ui.horizontal(|ui| {
            if ui.button("détecter").clicked() {
                self.protons = self
                    .client
                    .request(&Req::Protons)
                    .ok()
                    .and_then(|e| e.into_data().ok())
                    .unwrap_or_default();
                self.notify("Protons rescannés");
            }
            if ui.button("voir les nouveautés (GitHub)").clicked() {
                match self.client.request(&Req::ProtonReleases) {
                    Ok(envelope) if envelope.ok => {
                        self.releases = envelope.into_data().unwrap_or_default();
                        self.notify(format!("{} build(s) disponibles", self.releases.len()));
                    }
                    Ok(envelope) => self.notify(envelope.error.unwrap_or_default()),
                    Err(err) => self.notify(format!("{err}")),
                }
            }
        });
        ui.add_space(10.0);
        let recommended = self
            .protons
            .iter()
            .filter(|p| p.runnable)
            .map(|p| p.name.clone())
            .next();
        for proton in self.protons.clone() {
            egui::Frame::none()
                .fill(CARD)
                .rounding(egui::Rounding::same(8.0))
                .inner_margin(10.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let star = if Some(&proton.name) == recommended.as_ref() {
                            "★ "
                        } else {
                            "  "
                        };
                        ui.label(format!("{star}{}", proton.name));
                        ui.label(
                            egui::RichText::new(format!("{} · {}", proton.source, proton.version))
                                .size(11.0)
                                .color(egui::Color32::GRAY),
                        );
                    });
                    ui.label(
                        egui::RichText::new(proton.path.display().to_string())
                            .size(10.0)
                            .color(egui::Color32::GRAY),
                    );
                });
            ui.add_space(4.0);
        }
        if !self.releases.is_empty() {
            ui.add_space(16.0);
            ui.heading("Disponibles au téléchargement");
            for release in self.releases.clone().into_iter().take(24) {
                ui.horizontal(|ui| {
                    ui.label(ludus_core::util::ellipsize(&release.asset, 54));
                    ui.label(
                        egui::RichText::new(format!(
                            "{} Mio",
                            release.size / 1024 / 1024
                        ))
                        .size(11.0)
                        .color(egui::Color32::GRAY),
                    );
                    let exists = self.protons.iter().any(|p| release.asset.contains(&p.name));
                    if exists {
                        ui.label(egui::RichText::new("installé").size(11.0).color(ACCENT));
                    } else if ui.small_button("installer").clicked() {
                        match self.client.request(&Req::InstallProton {
                            repo: release.repo.clone(),
                            asset: release.asset.clone(),
                            checksum_url: release.checksum_url.clone(),
                        }) {
                            Ok(envelope) if envelope.ok => {
                                self.notify(format!("téléchargement de {}", release.asset))
                            }
                            Ok(envelope) => self.notify(envelope.error.unwrap_or_default()),
                            Err(err) => self.notify(format!("{err}")),
                        }
                    }
                });
            }
        }
    }

    fn controllers_page(&mut self, ui: &mut egui::Ui) {
        ui.heading("Manettes");
        ui.label(
            egui::RichText::new(
                "Couche maison : evdev → remapping (zone morte, courbe, remaps) → pad virtuel uinput. Le profile est appliqué automatiquement au lancement d'un jeu.",
            )
            .size(11.0)
            .color(egui::Color32::GRAY),
        );
        ui.add_space(10.0);
        ui.label(egui::RichText::new("Périphériques").strong());
        for device in self.devices.clone() {
            ui.horizontal(|ui| {
                ui.label(if device.gamepad { "MANETTE" } else { "périphérique" });
                ui.label(ludus_core::util::ellipsize(&device.name, 44));
                ui.label(
                    egui::RichText::new(format!(
                        "{:04x}:{:04x}",
                        device.vendor, device.product
                    ))
                    .size(11.0)
                    .color(egui::Color32::GRAY),
                );
            });
        }
        ui.add_space(14.0);
        ui.label(egui::RichText::new("Profils").strong());
        for profile in ludus_input::list_profiles(&self.paths.data_dir) {
            ui.horizontal(|ui| {
                ui.label(&profile.name);
                if profile.is_default {
                    ui.label(egui::RichText::new("par défaut").size(11.0).color(ACCENT));
                }
                ui.label(
                    egui::RichText::new(profile.path.display().to_string())
                        .size(10.0)
                        .color(egui::Color32::GRAY),
                );
            });
        }
        ui.add_space(6.0);
        if ui.button("créer/mettre à jour le profil « defaut »").clicked() {
            let mut profile = ludus_input::Profile::default();
            profile.deadzone = self.config.input.deadzone;
            profile.curve = self.config.input.stick_curve;
            profile.name = "defaut".into();
            match profile.save(&ludus_input::profile_path(&self.paths.data_dir, "defaut")) {
                Ok(()) => self.notify("profil enregistré"),
                Err(err) => self.notify(format!("{err}")),
            }
        }
        ui.add_space(14.0);
        ui.label(
            egui::RichText::new(
                "Test en direct : « ludus cli input-test » (affiche les raccourcis). Vérification du pad virtuel : « ludus cli virtual-pad ».",
            )
            .size(11.0)
            .color(egui::Color32::GRAY),
        );
    }

    fn settings_page(&mut self, ui: &mut egui::Ui) {
        ui.heading("Réglages");
        ui.add_space(8.0);
        let mut config = self.config.clone();
        egui::Grid::new("settings")
            .num_columns(2)
            .spacing([14.0, 10.0])
            .show(ui, |ui| {
                ui.label("Racine bibliothèque");
                ui.text_edit_singleline(&mut config.general.library_root);
                ui.end_row();

                ui.label("Compte Steam");
                ui.text_edit_singleline(&mut config.steam.username);
                ui.end_row();

                ui.label("Backend");
                egui::ComboBox::from_id_salt("backend")
                    .selected_text(&config.steam.backend)
                    .show_ui(ui, |ui| {
                        for value in ["auto", "steamcmd", "depotdownloader"] {
                            ui.selectable_value(
                                &mut config.steam.backend,
                                value.to_string(),
                                value,
                            );
                        }
                    });
                ui.end_row();

                ui.label("Proton par défaut");
                ui.text_edit_singleline(&mut config.launch.proton);
                ui.end_row();

                ui.label("Résolution gamescope");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut config.launch.width));
                    ui.label("×");
                    ui.add(egui::DragValue::new(&mut config.launch.height));
                });
                ui.end_row();

                ui.label("Limite FPS");
                ui.add(egui::Slider::new(&mut config.launch.fps_cap, 30..=240));
                ui.end_row();

                ui.label("Jeu");
                ui.vertical(|ui| {
                    ui.checkbox(&mut config.launch.gamescope, "gamescope");
                    ui.checkbox(&mut config.launch.mangohud, "MangoHud");
                    ui.checkbox(&mut config.launch.gamemode, "GameMode");
                    ui.checkbox(&mut config.launch.steam_for_drm, "Steam en fond si DRM");
                });
                ui.end_row();

                ui.label("Overlay");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut config.overlay.hotkey).desired_width(110.0),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut config.overlay.pad_hotkey)
                            .desired_width(150.0),
                    );
                });
                ui.end_row();

                ui.label("Manettes");
                ui.horizontal(|ui| {
                    ui.checkbox(&mut config.input.grab_devices, "grab");
                    ui.add(egui::Slider::new(&mut config.input.deadzone, 0.0..=0.5).text("zone morte"));
                    ui.add(egui::Slider::new(&mut config.input.stick_curve, 0.5..=3.0).text("courbe"));
                });
                ui.end_row();
            });
        ui.add_space(12.0);
        if ui.button("Enregistrer").clicked() {
            match self.client.request(&Req::SetConfig {
                config: config.clone(),
            }) {
                Ok(envelope) if envelope.ok => {
                    self.config = config.clone();
                    self.notify("configuration enregistrée");
                }
                Ok(envelope) => self.notify(envelope.error.unwrap_or_default()),
                Err(err) => self.notify(format!("{err}")),
            }
        }
        ui.add_space(18.0);
        ui.separator();
        ui.label(egui::RichText::new("Outils Steam").strong());
        let steamcmd = self
            .tool_status
            .as_ref()
            .and_then(|v| v.get("steamcmd"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let depot = self
            .tool_status
            .as_ref()
            .and_then(|v| v.get("depotdownloader"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        ui.label(format!(
            "SteamCMD : {}   ·   DepotDownloader : {}",
            if steamcmd { "installé" } else { "absent" },
            if depot { "installé" } else { "absent" }
        ));
        ui.horizontal(|ui| {
            if ui.button("télécharger SteamCMD").clicked() {
                let _ = self.client.request(&Req::InstallTools {
                    which: vec!["steamcmd".into()],
                });
                self.notify("téléchargement de SteamCMD lancé");
            }
            if ui.button("télécharger DepotDownloader").clicked() {
                let _ = self.client.request(&Req::InstallTools {
                    which: vec!["depotdownloader".into()],
                });
                self.notify("téléchargement de DepotDownloader lancé");
            }
        });
        ui.label(
            egui::RichText::new(
                "Connexion Steam : « ludus cli login <compte> » dans un terminal (mot de passe et 2FA saisis par SteamCMD, jamais stockés).",
            )
            .size(11.0)
            .color(egui::Color32::GRAY),
        );
    }

    fn doctor_page(&mut self, ui: &mut egui::Ui) {
        ui.heading("Diagnostic");
        if ui.button("relancer les vérifications").clicked() || self.doctor.is_none() {
            self.doctor = self
                .client
                .request(&Req::Doctor)
                .ok()
                .and_then(|e| e.into_data().ok());
        }
        ui.add_space(10.0);
        let Some(report) = self.doctor.clone() else {
            ui.label("diagnostic indisponible (daemon injoignable)");
            return;
        };
        for check in report.checks {
            ui.horizontal(|ui| {
                let (symbol, color) = match check.status {
                    CheckStatus::Ok => ("OK", egui::Color32::from_rgb(130, 210, 150)),
                    CheckStatus::Warn => ("ATTN", egui::Color32::from_rgb(230, 200, 120)),
                    CheckStatus::Fail => ("ERR", egui::Color32::from_rgb(230, 130, 130)),
                };
                ui.label(egui::RichText::new(symbol).color(color).strong());
                ui.label(egui::RichText::new(check.name).strong());
            });
            ui.label(
                egui::RichText::new(check.detail)
                    .size(11.0)
                    .color(egui::Color32::GRAY),
            );
            ui.add_space(4.0);
        }
    }
}

impl eframe::App for LudusApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.last_refresh.elapsed() > Duration::from_secs(3) {
            let rescan = self.games.is_empty();
            self.refresh(rescan);
        }
        self.handle_gamepad(ctx);

        egui::TopBottomPanel::top("topbar")
            .frame(egui::Frame::none().fill(PANEL).inner_margin(egui::Margin::symmetric(16.0, 8.0)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(match self.page {
                            Page::Library => "Bibliothèque",
                            Page::Downloads => "Téléchargements",
                            Page::Protons => "Protons",
                            Page::Controllers => "Manettes",
                            Page::Settings => "Réglages",
                            Page::Doctor => "Diagnostic",
                        })
                        .size(13.0)
                        .color(egui::Color32::GRAY),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let label = if self.big_picture { "fenêtré" } else { "plein écran" };
                        if ui.button(label).clicked() {
                            self.big_picture = !self.big_picture;
                            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.big_picture));
                        }
                        if self.session.is_some() && ui.button("arrêter le jeu").clicked() {
                            let _ = self.client.request(&Req::Stop);
                            self.notify("jeu arrêté");
                        }
                    });
                });
            });

        self.sidebar(ctx);

        egui::CentralPanel::default()
            .frame(egui::Frame::none().inner_margin(20.0))
            .show(ctx, |ui| match self.page {
                Page::Library => self.library_page(ctx),
                Page::Downloads => self.downloads_page(ui),
                Page::Protons => self.protons_page(ui),
                Page::Controllers => self.controllers_page(ui),
                Page::Settings => self.settings_page(ui),
                Page::Doctor => self.doctor_page(ui),
            });

        if let Some(toast) = &self.toast {
            if Instant::now() < toast.until {
                egui::Area::new(egui::Id::new("toast"))
                    .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -24.0])
                    .show(ctx, |ui| {
                        egui::Frame::none()
                            .fill(CARD)
                            .rounding(egui::Rounding::same(10.0))
                            .inner_margin(12.0)
                            .show(ui, |ui| {
                                ui.label(&toast.text);
                            });
                    });
            }
        }

        if let Some(text) = self.dry_run.clone() {
            let mut open = true;
            egui::Window::new("Commande de lancement")
                .open(&mut open)
                .collapsible(false)
                .resizable(true)
                .default_width(720.0)
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut text.clone())
                                .desired_rows(16)
                                .desired_width(f32::INFINITY)
                                .font(egui::TextStyle::Monospace),
                        );
                    });
                });
            if !open {
                self.dry_run = None;
            }
        }

        let animating = self.session.is_some();
        if animating {
            ctx.request_repaint_after(Duration::from_secs(1));
        } else {
            ctx.request_repaint_after(Duration::from_secs(3));
        }
    }
}

fn game_card(
    ui: &mut egui::Ui,
    game: &Game,
    texture: Option<&egui::TextureHandle>,
    selected: bool,
) -> egui::Response {
    let size = egui::vec2(168.0, 252.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let painter = ui.painter_at(rect);
    let fill = if selected {
        egui::Color32::from_rgb(38, 47, 62)
    } else if response.hovered() {
        egui::Color32::from_rgb(33, 37, 45)
    } else {
        CARD
    };
    painter.rect_filled(rect, egui::Rounding::same(10.0), fill);
    let image_rect = egui::Rect::from_min_size(
        rect.min + egui::vec2(8.0, 8.0),
        egui::vec2(size.x - 16.0, 196.0),
    );
    match texture {
        Some(texture) => {
            painter.image(
                texture.id(),
                image_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        None => {
            painter.rect_filled(image_rect, egui::Rounding::same(6.0), egui::Color32::from_gray(40));
            painter.text(
                image_rect.center(),
                egui::Align2::CENTER_CENTER,
                "LUDUS",
                egui::FontId::proportional(14.0),
                egui::Color32::from_gray(110),
            );
        }
    }
    painter.text(
        egui::pos2(rect.left() + 10.0, rect.bottom() - 36.0),
        egui::Align2::LEFT_CENTER,
        ludus_core::util::ellipsize(&game.name, 21),
        egui::FontId::proportional(12.5),
        egui::Color32::from_gray(225),
    );
    painter.text(
        egui::pos2(rect.left() + 10.0, rect.bottom() - 18.0),
        egui::Align2::LEFT_CENTER,
        if game.installed { "installé" } else { "non installé" },
        egui::FontId::proportional(10.0),
        if game.installed {
            egui::Color32::from_rgb(130, 210, 150)
        } else {
            egui::Color32::from_gray(120)
        },
    );
    if selected {
        painter.rect_stroke(
            rect.shrink(1.0),
            egui::Rounding::same(10.0),
            egui::Stroke::new(1.5_f32, ACCENT),
        );
    }
    response
}

/// Expose la couverture locale d'un jeu (utilisé par les tests et la CLI).
pub fn cover_file(paths: &LudusPaths, appid: u32) -> PathBuf {
    paths.covers_dir().join(format!("{appid}.jpg"))
}

pub fn request_json<T: serde::de::DeserializeOwned>(
    client: &Client,
    req: Req,
) -> Option<T> {
    client.request(&req).ok().and_then(|e| e.into_data().ok())
}
