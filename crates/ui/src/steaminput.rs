//! Onglet Steam Input d'une fiche : les configurations du compte, filtrées par
//! manette, avec une vraie sélection.
//!
//! Les sélections sont conservées par Lapinux et converties partiellement
//! au lancement direct, sans démarrer ni installer le client Steam.

use crate::{
    muted, open_path, primary_button, secondary_button, ACCENT, BACKGROUND, BORDER, CARD,
    CARD_HOVER, OK, PANEL, TEXT, TEXT_DIM, WARN,
};
use eframe::egui;
use lapinux_core::client::Client;
use lapinux_core::i18n::{t, t_args, t_fmt};
use lapinux_core::ipc::Req;
use lapinux_core::model::{
    steam_input_label, SteamInputAction, SteamInputActionGroup, SteamInputCommunityProfile,
    SteamInputCommunityReport, SteamInputController, SteamInputProfile, SteamInputReport,
    SteamInputSaved, SteamInputSetting,
};
use std::collections::{HashMap, HashSet};
use std::sync::mpsc;
use std::time::Duration;

#[derive(Default)]
pub struct Shelf {
    reports: HashMap<u32, SteamInputReport>,
    errors: HashMap<u32, String>,
    asked: HashSet<u32>,
    rx: Option<mpsc::Receiver<(u32, Result<SteamInputReport, String>)>>,
    loading: bool,
    client: Option<Client>,
    /// Recherche libre : titre, description, créateur, type de manette.
    query: String,
    /// Type de manette retenu ; `None` montre toutes les manettes.
    controller: Option<String>,
    apply: Option<ApplyJob>,
    notice: Option<(bool, String)>,
    /// Source affichée : les profils du disque ou le catalogue Steam.
    source: Source,
    community: HashMap<u32, SteamInputCommunityReport>,
    community_errors: HashMap<u32, String>,
    community_keys: HashMap<u32, String>,
    community_rx: Option<mpsc::Receiver<(u32, String, Result<SteamInputCommunityReport, String>)>>,
    community_loading: bool,
    community_pending: Option<(u32, String)>,
    /// La recherche a changé depuis le dernier appel au catalogue.
    community_dirty: bool,
    /// Le filtre initial ne doit pas écraser un choix explicite de l'utilisateur.
    community_initialized: HashSet<u32>,
    /// Dernière signature de filtrage journalisée, pour ne pas spammer.
    community_trace: String,
    /// Configurations communautaires téléchargées pendant cette session.
    community_installed: HashSet<u64>,
    /// Le harnais de test relève les réponses egui réelles du panneau.
    #[cfg(test)]
    interaction: Option<InteractionProbe>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Source {
    Local,
    /// Le catalogue communautaire s’ouvre en premier : c’est là que se
    /// trouvent les configurations notées par les joueurs, alors que le
    /// disque ne contient souvent que les profils déjà choisis.
    #[default]
    Community,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApplyKind {
    Local,
    Community(u64),
}

#[cfg(test)]
#[derive(Default)]
struct InteractionProbe {
    controller_cards: Vec<(String, egui::Rect)>,
    chips: Vec<(Option<String>, egui::Rect)>,
    displayed_profiles: Vec<String>,
    results_detail: String,
    empty_hint: Option<String>,
}

struct ApplyJob {
    appid: u32,
    title: String,
    kind: ApplyKind,
    rx: mpsc::Receiver<Result<SteamInputReport, String>>,
}

impl Shelf {
    /// Demande les profils une fois par jeu : la réponse est gardée tant que
    /// l'application vit.
    pub fn ensure(&mut self, ctx: &egui::Context, client: &Client, appid: u32) {
        self.client = Some(client.detached());
        if self.loading || !self.asked.insert(appid) {
            return;
        }
        let Some(client) = self.client.clone() else {
            return;
        };
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = client
                .request(&Req::SteamInput { appid })
                .map_err(|error| error.to_string())
                .and_then(|envelope| {
                    envelope
                        .into_data::<SteamInputReport>()
                        .map_err(|error| error.to_string())
                });
            let _ = tx.send((appid, result));
        });
        self.rx = Some(rx);
        self.loading = true;
        ctx.request_repaint();
    }

    fn poll(&mut self, ctx: &egui::Context) {
        self.poll_community(ctx);
        if let Some(rx) = &self.rx {
            match rx.try_recv() {
                Ok((appid, result)) => {
                    self.rx = None;
                    self.loading = false;
                    match result {
                        Ok(report) => {
                            self.errors.remove(&appid);
                            self.reports.insert(appid, report);
                        }
                        Err(error) => {
                            self.errors.insert(appid, error);
                        }
                    }
                    ctx.request_repaint();
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(Duration::from_millis(150))
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.rx = None;
                    self.loading = false;
                }
            }
        }

        let outcome = self
            .apply
            .as_ref()
            .map(|job| (job.appid, job.title.clone(), job.kind, job.rx.try_recv()));
        let Some((appid, title, kind, outcome)) = outcome else {
            return;
        };
        match outcome {
            Ok(result) => {
                self.apply = None;
                match result {
                    Ok(report) => {
                        self.reports.insert(appid, report);
                        if let ApplyKind::Community(id) = kind {
                            self.community_installed.insert(id);
                            if let Some(community) = self.community.get_mut(&appid) {
                                if let Some(profile) = community
                                    .profiles
                                    .iter_mut()
                                    .find(|profile| profile.id == id)
                                {
                                    profile.installed = true;
                                }
                            }
                            // La sélection est conservée et relue par Lapinux.
                            self.source = Source::Local;
                            self.notice = Some((
                                true,
                                t_args(
                                    "steaminput.applied_community",
                                    &[("title", title.as_str())],
                                ),
                            ));
                        } else {
                            self.notice = Some((
                                true,
                                t_args("steaminput.applied_local", &[("title", title.as_str())]),
                            ));
                        }
                    }
                    Err(error) => self.notice = Some((false, error)),
                }
                ctx.request_repaint();
            }
            Err(mpsc::TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(150)),
            Err(mpsc::TryRecvError::Disconnected) => {
                self.apply = None;
                self.notice = Some((false, t("steaminput.selection_failed").into()));
            }
        }
    }

    fn community_key(&self, appid: u32) -> String {
        format!(
            "{appid}\u{0}{}\u{0}{}",
            self.query.trim().to_lowercase(),
            self.controller.as_deref().unwrap_or("")
        )
    }

    fn start_community(&mut self, appid: u32) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let key = self.community_key(appid);
        let pending_key = key.clone();
        self.community_errors.remove(&appid);
        if self
            .community_pending
            .as_ref()
            .is_some_and(|(pending_appid, pending_key)| {
                *pending_appid == appid && pending_key == &key
            })
        {
            return;
        }
        tracing::info!(
            "Steam Input communauté : requête appid={appid} manette={:?} recherche={:?}",
            self.controller.as_deref().unwrap_or("toutes"),
            self.query.trim()
        );
        let request = Req::SteamInputCommunity {
            appid,
            query: self.query.trim().to_string(),
            controller: self.controller.clone(),
            page: 1,
        };
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = client
                .request(&request)
                .map_err(|error| error.to_string())
                .and_then(|envelope| {
                    envelope
                        .into_data::<SteamInputCommunityReport>()
                        .map_err(|error| error.to_string())
                });
            let _ = tx.send((appid, key, result));
        });
        self.community_rx = Some(rx);
        self.community_pending = Some((appid, pending_key));
        self.community_loading = true;
        self.community_dirty = false;
    }

    fn poll_community(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.community_rx else {
            return;
        };
        match rx.try_recv() {
            Ok((appid, key, result)) => {
                self.community_rx = None;
                self.community_loading = false;
                self.community_pending = None;
                self.community_keys.insert(appid, key);
                match result {
                    Ok(report) => {
                        self.community_errors.remove(&appid);
                        tracing::info!(
                            "Steam Input communauté : réponse brute appid={appid} total={} profils={} fallback={} types={:?}",
                            report.total,
                            report.profiles.len(),
                            report.fallback,
                            report
                                .profiles
                                .iter()
                                .take(4)
                                .map(|profile| profile.controller_type.as_str())
                                .collect::<Vec<_>>()
                        );
                        self.community.insert(appid, report);
                    }
                    Err(error) => {
                        tracing::warn!("Steam Input community: error appid={appid}: {error}");
                        self.community_errors.insert(appid, error);
                    }
                }
                ctx.request_repaint();
            }
            Err(mpsc::TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(150)),
            Err(mpsc::TryRecvError::Disconnected) => {
                self.community_rx = None;
                self.community_loading = false;
                self.community_pending = None;
            }
        }
    }

    /// Filtre sur le seul type détecté, sans choisir arbitrairement entre
    /// plusieurs manettes ni réécrire un choix explicite lors d'une réouverture.
    fn initialize_community_filter(&mut self, appid: u32, report: &SteamInputReport) {
        if !self.community_initialized.insert(appid) || self.controller.is_some() {
            return;
        }
        let mut kinds = report.controllers.iter().map(|pad| pad.kind.as_str());
        if let Some(first) = kinds.next() {
            if !first.is_empty() && kinds.all(|kind| kind == first) {
                self.controller = Some(first.to_string());
            }
        }
    }

    /// Lance le catalogue à la première ouverture de la vue Communauté, ou
    /// quand le jeu change : les résultats déjà chargés restent affichés le
    /// temps d'une nouvelle recherche demandée explicitement.
    fn ensure_community_for_game(&mut self, appid: u32) {
        if self.source != Source::Community || self.community_loading {
            return;
        }
        if self.community.contains_key(&appid) || self.community_errors.contains_key(&appid) {
            return;
        }
        if let Some(report) = self.reports.get(&appid).cloned() {
            self.initialize_community_filter(appid, &report);
        }
        self.start_community(appid);
    }

    fn reload(&mut self, appid: u32) {
        self.asked.remove(&appid);
        self.errors.remove(&appid);
        self.community.remove(&appid);
        self.community_errors.remove(&appid);
        self.community_keys.remove(&appid);
        self.community_rx = None;
        self.community_loading = false;
        self.community_pending = None;
        self.community_dirty = false;
        self.community_trace.clear();
        self.notice = None;
    }

    /// Page de l'onglet : les manettes, la recherche, la configuration active,
    /// puis les profils.
    pub fn page(&mut self, ui: &mut egui::Ui, appid: u32) {
        #[cfg(test)]
        if let Some(probe) = self.interaction.as_mut() {
            probe.controller_cards.clear();
            probe.chips.clear();
            probe.displayed_profiles.clear();
            probe.results_detail.clear();
            probe.empty_hint = None;
        }
        self.poll(ui.ctx());
        let Some(report) = self.reports.get(&appid).cloned() else {
            self.waiting(ui, appid);
            return;
        };

        self.header(ui, &report, appid);
        ui.add_space(8.0);
        let width = ui.available_width();
        if width >= 820.0 {
            let gap = 16.0;
            let sidebar = 250.0;
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                ui.allocate_ui_with_layout(
                    egui::vec2(sidebar, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(sidebar);
                        self.filters(ui, appid, &report);
                    },
                );
                ui.allocate_ui_with_layout(
                    egui::vec2((width - sidebar - gap).max(0.0), 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width((width - sidebar - gap).max(0.0));
                        self.results(ui, appid, &report);
                    },
                );
            });
        } else {
            self.filters(ui, appid, &report);
            self.results(ui, appid, &report);
        }
    }

    fn filters(&mut self, ui: &mut egui::Ui, appid: u32, report: &SteamInputReport) {
        self.source_tabs(ui, appid, report);
        self.ensure_community_for_game(appid);
        self.controllers(ui, appid, report);
        self.toolbar(ui, appid, report);
        ui.add_space(8.0);
        muted(ui, t("steaminput.compat_partial"));
        egui::CollapsingHeader::new(t("steaminput.compat_details"))
            .id_salt(("steam-input-compatibility", appid))
            .show(ui, |ui| {
                muted(ui, t("steaminput.compat_body"));
            });
    }

    fn results(&mut self, ui: &mut egui::Ui, appid: u32, report: &SteamInputReport) {
        if let Some((ok, text)) = self.notice.clone() {
            notice_line(ui, ok, &text);
        }
        self.active_section(ui, appid, report);
        self.profiles_section(ui, appid, report);

        for note in &report.notes {
            // Les états vides ont déjà leur propre encart : répéter la note du
            // rapport ajouterait deux fois le même constat dans la même vue.
            let duplicate_controller = report.controllers.is_empty()
                && note.starts_with(lapinux_core::i18n::t("controllers.none"));
            let duplicate_profile = report.profiles.is_empty()
                && note.starts_with(lapinux_core::i18n::t("steaminput.no_profile_found"));
            if duplicate_controller || duplicate_profile {
                continue;
            }
            ui.add_space(4.0);
            muted(ui, note);
        }
        ui.add_space(4.0);
    }

    fn waiting(&mut self, ui: &mut egui::Ui, appid: u32) {
        ui.add_space(24.0);
        if let Some(error) = self.errors.get(&appid).cloned() {
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new(&error).size(11.5).color(WARN));
                ui.add_space(8.0);
                if secondary_button(ui, t("steaminput.retry")).clicked() {
                    self.reload(appid);
                    ui.ctx().request_repaint();
                }
            });
        } else {
            ui.vertical_centered(|ui| {
                crate::section_icons::loading(ui, 18.0);
                ui.add_space(6.0);
                muted(ui, t("steaminput.reading_local"));
            });
        }
    }

    fn header(&mut self, ui: &mut egui::Ui, report: &SteamInputReport, appid: u32) {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new("Steam Input")
                        .size(19.0)
                        .strong()
                        .color(TEXT()),
                );
                ui.add_space(4.0);
                muted(ui, t("steaminput.pick_hint"));
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    badge(
                        ui,
                        &t_fmt(
                            "steaminput.local_tab",
                            &[report.profiles.len().to_string()],
                        ),
                        ACCENT(),
                    );
                    if report.profiles.iter().any(|profile| profile.active) {
                        badge(ui, t("steaminput.active"), OK);
                    }
                });
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if text_action(ui, t("steaminput.reload")).clicked() {
                    self.reload(appid);
                    ui.ctx().request_repaint();
                }
                if self.loading || self.apply.is_some() {
                    crate::section_icons::loading(ui, 18.0);
                }
            });
        });
    }

    fn source_tabs(&mut self, ui: &mut egui::Ui, appid: u32, report: &SteamInputReport) {
        let local_selected = self.source == Source::Local;
        let community_selected = self.source == Source::Community;
        let remote = self.community.get(&appid);
        let local_label = t_fmt("steaminput.local_tab", &[report.profiles.len().to_string()]);
        let community_label = remote
            .map(|report| {
                if report.fallback {
                    t("steaminput.community_other_games").to_string()
                } else if report.total == 0 {
                    t("steaminput.community_none").to_string()
                } else {
                    t_fmt("steaminput.community_count", &[report.total.to_string()])
                }
            })
            .unwrap_or_else(|| t("steaminput.community").into());
        let mut open_community = false;
        ui.horizontal_wrapped(|ui| {
            if source_tab(ui, &local_label, local_selected).clicked() && !local_selected {
                self.source = Source::Local;
            }
            if source_tab(ui, &community_label, community_selected).clicked() && !community_selected
            {
                self.source = Source::Community;
                self.community_dirty = false;
                open_community = true;
            }
            if self.community_loading && community_selected {
                crate::section_icons::loading(ui, 18.0);
            }
        });
        if open_community {
            self.initialize_community_filter(appid, report);
            self.community_trace.clear();
            self.start_community(appid);
        }
    }

    fn controllers(&mut self, ui: &mut egui::Ui, appid: u32, report: &SteamInputReport) {
        let remote = self.source == Source::Community;
        let mut search_remote = false;
        ui.add_space(14.0);
        section_heading(ui, t("steaminput.controllers_heading"), "");
        ui.add_space(4.0);
        if report.controllers.is_empty() {
            muted(ui, t("steaminput.no_controller"));
            return;
        }
        let width = ui.available_width();
        // `horizontal_wrapped` ajoute déjà son espacement entre deux cartes.
        // Un second `add_space` faisait dépasser la ligne de 8 px et empilait
        // les deux manettes au lieu de les mettre côte à côte.
        let card_width = if width >= 720.0 {
            (width - 10.0) / 2.0
        } else {
            width
        };
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            for controller in &report.controllers {
                let selected = self.controller.as_deref() == Some(controller.kind.as_str());
                let response = controller_row(ui, controller, selected, card_width);
                #[cfg(test)]
                if let Some(probe) = self.interaction.as_mut() {
                    probe
                        .controller_cards
                        .push((controller.kind.clone(), response.rect));
                }
                if response.clicked() {
                    self.controller = if selected {
                        None
                    } else {
                        Some(controller.kind.clone())
                    };
                    search_remote = remote;
                }
            }
        });
        if search_remote {
            self.start_community(appid);
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, appid: u32, report: &SteamInputReport) {
        ui.add_space(12.0);
        let remote = self.source == Source::Community;
        let mut search_now = false;
        egui::Frame::none()
            .fill(PANEL())
            .stroke(egui::Stroke::new(1.0_f32, BORDER()))
            .rounding(egui::Rounding::same(10.0))
            .inner_margin(egui::Margin::symmetric(10.0, 6.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    search_icon(ui);
                    let reserved = if remote { 118.0 } else { 24.0 };
                    let width = (ui.available_width() - reserved).max(40.0);
                    let response = ui.add_sized(
                        [width, 22.0],
                        egui::TextEdit::singleline(&mut self.query)
                            .frame(false)
                            .hint_text(if remote {
                                t("steaminput.search_community")
                            } else {
                                t("steaminput.search_local")
                            }),
                    );
                    if remote && response.changed() {
                        self.community_dirty = true;
                    }
                    if response.lost_focus()
                        && ui.input(|input| input.key_pressed(egui::Key::Enter))
                    {
                        search_now = true;
                    }
                    if !self.query.is_empty()
                        && ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new("×").size(14.0).color(TEXT_DIM()),
                                )
                                .frame(false),
                            )
                            .on_hover_text(t("steaminput.clear_search"))
                            .clicked()
                    {
                        self.query.clear();
                        search_now = remote;
                    }
                    if remote && text_action(ui, t("steaminput.search")).clicked() {
                        search_now = true;
                    }
                });
            });

        ui.add_space(8.0);
        let mut kinds: Vec<String> = if remote {
            self.community
                .get(&appid)
                .map(|community| {
                    community
                        .profiles
                        .iter()
                        .map(|profile| profile.controller_type.clone())
                        .collect()
                })
                .unwrap_or_default()
        } else {
            report
                .profiles
                .iter()
                .map(|profile| profile.controller_type.clone())
                .collect()
        };
        // Une manette branchée reste proposée même si les vingt résultats
        // chargés n'en contiennent pas encore : le clic relancera la recherche
        // côté catalogue avec ce filtre.
        for controller in &report.controllers {
            if !kinds.iter().any(|kind| kind == &controller.kind) {
                kinds.push(controller.kind.clone());
            }
        }
        kinds.retain(|kind| !kind.is_empty());
        kinds.sort();
        kinds.dedup();

        ui.horizontal_wrapped(|ui| {
            let all = chip(ui, t("steaminput.all"), self.controller.is_none());
            #[cfg(test)]
            if let Some(probe) = self.interaction.as_mut() {
                probe.chips.push((None, all.rect));
            }
            if all.clicked() {
                self.controller = None;
                search_now = remote;
            }
            for kind in kinds {
                let count = if remote {
                    self.community
                        .get(&appid)
                        .map(|community| {
                            community
                                .profiles
                                .iter()
                                .filter(|profile| profile.controller_type == kind)
                                .count()
                        })
                        .unwrap_or(0)
                } else {
                    report
                        .profiles
                        .iter()
                        .filter(|profile| profile.controller_type == kind)
                        .count()
                };
                let label = format!("{} · {count}", steam_input_label(&kind));
                let selected = self.controller.as_deref() == Some(kind.as_str());
                let response = chip(ui, &label, selected);
                #[cfg(test)]
                if let Some(probe) = self.interaction.as_mut() {
                    probe.chips.push((Some(kind.clone()), response.rect));
                }
                if response.clicked() {
                    self.controller = if selected { None } else { Some(kind.clone()) };
                    search_now = remote;
                }
            }
            if !self.query.is_empty() || self.controller.is_some() {
                ui.add_space(2.0);
                if text_action(ui, t("steaminput.clear")).clicked() {
                    self.query.clear();
                    self.controller = None;
                    search_now = remote;
                }
            }
        });
        if search_now {
            self.start_community(appid);
        }
    }

    fn active_section(&mut self, ui: &mut egui::Ui, appid: u32, report: &SteamInputReport) {
        let active: Vec<&SteamInputProfile> = report
            .profiles
            .iter()
            .filter(|profile| profile.active)
            .collect();
        ui.add_space(4.0);
        section_heading(
            ui,
            t("steaminput.active_heading"),
            if active.is_empty() {
                t("steaminput.no_selection")
            } else {
                t("steaminput.next_launch")
            },
        );
        ui.add_space(6.0);
        if active.is_empty() {
            muted(ui, t("steaminput.pick_profile"));
            return;
        }
        for profile in active {
            self.active_card(ui, appid, profile);
            ui.add_space(7.0);
        }
    }

    fn profiles_section(&mut self, ui: &mut egui::Ui, appid: u32, report: &SteamInputReport) {
        if self.source == Source::Community {
            self.community_section(ui, appid, report);
            return;
        }
        let active_present = report.profiles.iter().any(|profile| profile.active);
        // La carte active est déjà mise en avant au-dessus : la répéter en
        // tête de liste doublait la même configuration à quelques pixels.
        let profiles: Vec<&SteamInputProfile> =
            filtered(report, appid, &self.query, self.controller.as_deref())
                .into_iter()
                .filter(|profile| !profile.active)
                .collect();
        let detail = if self.query.trim().is_empty() && self.controller.is_none() {
            if active_present {
                counted(
                    profiles.len(),
                    t("steaminput.other_profile_one"),
                    t("steaminput.other_profile_many"),
                )
            } else {
                counted(
                    profiles.len(),
                    t("steaminput.profile_one"),
                    t("steaminput.profile_many"),
                )
            }
        } else {
            counted(
                profiles.len(),
                t("steaminput.result_one"),
                t("steaminput.result_many"),
            )
        };
        #[cfg(test)]
        if let Some(probe) = self.interaction.as_mut() {
            probe.displayed_profiles = profiles
                .iter()
                .map(|profile| display_title(&profile.title))
                .collect();
            probe.results_detail = detail.clone();
        }
        ui.add_space(16.0);
        section_heading(
            ui,
            if active_present {
                t("steaminput.other_profiles_heading")
            } else {
                t("steaminput.profiles_heading")
            },
            detail,
        );
        ui.add_space(6.0);
        if profiles.is_empty() {
            let (message, color) = if report.profiles.is_empty() {
                (t("steaminput.empty_local"), TEXT_DIM())
            } else if active_present {
                if self.query.trim().is_empty() && self.controller.is_none() {
                    (t("steaminput.empty_all_active"), TEXT_DIM())
                } else {
                    (t("steaminput.empty_other_no_match"), WARN)
                }
            } else {
                (t("steaminput.empty_no_match"), WARN)
            };
            #[cfg(test)]
            if let Some(probe) = self.interaction.as_mut() {
                probe.empty_hint = Some(message.to_string());
            }
            hint_box(ui, message, color);
            if !report.profiles.is_empty()
                && secondary_button(ui, t("steaminput.show_all")).clicked()
            {
                self.query.clear();
                self.controller = None;
                ui.ctx().request_repaint();
            }
            return;
        }
        for profile in profiles {
            self.profile_card(ui, appid, profile);
            ui.add_space(8.0);
        }
    }

    fn community_section(&mut self, ui: &mut egui::Ui, appid: u32, report: &SteamInputReport) {
        let community = self.community.get(&appid).cloned();
        let error = self.community_errors.get(&appid).cloned();
        let local_ids: HashSet<u64> = report
            .profiles
            .iter()
            .map(|profile| profile.workshop_id)
            .filter(|id| *id > 0)
            .collect();
        let installed_now = self.community_installed.clone();
        let mut profiles = community
            .as_ref()
            .map(|community| community.profiles.clone())
            .unwrap_or_default();
        for profile in &mut profiles {
            if installed_now.contains(&profile.id) {
                profile.installed = true;
            }
        }
        let needle = self.query.trim().to_lowercase();
        profiles.retain(|profile| {
            !local_ids.contains(&profile.id)
                && self
                    .controller
                    .as_deref()
                    .map_or(true, |kind| kind == profile.controller_type)
                && (needle.is_empty() || community_search_text(profile).contains(&needle))
        });
        // Les mieux notées d’abord : la note Steam, puis les votes positifs,
        // puis l’ancienneté. Une configuration sans note reste en fin de liste
        // plutôt que de passer devant un choix éprouvé.
        profiles.sort_by(|left, right| {
            right
                .rating
                .total_cmp(&left.rating)
                .then_with(|| right.votes_up.cmp(&left.votes_up))
                .then_with(|| right.subscriptions.cmp(&left.subscriptions))
                .then_with(|| right.modified.cmp(&left.modified))
                .then_with(|| left.title.cmp(&right.title))
        });
        let fallback = community.as_ref().is_some_and(|report| report.fallback);
        let raw_count = community.as_ref().map_or(0, |report| report.profiles.len());
        let trace = format!(
            "appid={appid} recherche={:?} manette={:?} local={} bruts={raw_count} affiches={} total={} fallback={fallback}",
            self.query.trim(),
            self.controller.as_deref().unwrap_or("toutes"),
            local_ids.len(),
            profiles.len(),
            community.as_ref().map_or(0, |report| report.total),
        );
        if self.community_trace != trace {
            tracing::info!("Steam Input community: display {trace}");
            self.community_trace = trace;
        }
        let detail = match &community {
            Some(community) if profiles.is_empty() && community.total == 0 => {
                t("steaminput.community_empty").into()
            }
            Some(community) if profiles.is_empty() => t_fmt(
                "steaminput.community_no_match",
                &[community.total.to_string()],
            ),
            Some(community) => t_fmt(
                "steaminput.community_count_total",
                &[
                    counted(
                        profiles.len(),
                        t("steaminput.result_one"),
                        t("steaminput.result_many"),
                    ),
                    community.total.to_string(),
                ],
            ),
            None if self.community_loading => t("steaminput.community_loading").into(),
            None if error.is_some() => t("steaminput.community_unavailable").into(),
            None => t("steaminput.community_catalog").into(),
        };
        #[cfg(test)]
        if let Some(probe) = self.interaction.as_mut() {
            probe.displayed_profiles = profiles
                .iter()
                .map(|profile| display_title(&profile.title))
                .collect();
            probe.results_detail = detail.clone();
        }
        ui.add_space(16.0);
        section_heading(
            ui,
            if fallback {
                t("steaminput.community_other_games_heading")
            } else {
                t("steaminput.community_heading")
            },
            detail,
        );
        ui.add_space(6.0);
        if fallback {
            hint_box(ui, t("steaminput.community_fallback_hint"), ACCENT());
            ui.add_space(6.0);
        }

        if let Some(error) = &error {
            hint_box(ui, error, WARN);
            if community.is_none() || !self.community_loading {
                if secondary_button(ui, t("steaminput.retry")).clicked() {
                    self.start_community(appid);
                    ui.ctx().request_repaint();
                }
            }
            if community.is_none() {
                return;
            }
            ui.add_space(6.0);
        }

        let Some(community) = community else {
            if self.community_loading {
                ui.vertical_centered(|ui| {
                    crate::section_icons::loading(ui, 18.0);
                    ui.add_space(6.0);
                    muted(ui, t("steaminput.reading_community"));
                });
            } else {
                hint_box(ui, t("steaminput.community_not_loaded"), TEXT_DIM());
                if secondary_button(ui, t("steaminput.load_catalog")).clicked() {
                    self.start_community(appid);
                }
            }
            return;
        };

        if self.community_dirty {
            hint_box(ui, t("steaminput.dirty_hint"), ACCENT());
            ui.add_space(6.0);
        } else if community.total > community.per_page {
            muted(
                ui,
                &t_args(
                    "steaminput.top_voted",
                    &[("per_page", &community.per_page.to_string())],
                ),
            );
            ui.add_space(6.0);
        }

        if profiles.is_empty() {
            if self.community_loading {
                ui.vertical_centered(|ui| {
                    crate::section_icons::loading(ui, 18.0);
                    ui.add_space(6.0);
                    muted(ui, t("steaminput.community_loading"));
                });
                return;
            }
            let controller_label = self
                .controller
                .as_deref()
                .map(steam_input_label)
                .unwrap_or(t("steaminput.all_controllers"));
            let message = if !self.query.trim().is_empty() {
                t_args(
                    "steaminput.no_community_query",
                    &[
                        ("appid", &appid.to_string()),
                        ("query", self.query.trim()),
                        ("controller", controller_label),
                    ],
                )
            } else if self.controller.is_some() {
                t_args(
                    "steaminput.no_community_controller",
                    &[
                        ("appid", &appid.to_string()),
                        ("controller", controller_label),
                    ],
                )
            } else {
                t_args(
                    "steaminput.no_community_all",
                    &[("appid", &appid.to_string())],
                )
            };
            hint_box(ui, &message, WARN);
            #[cfg(test)]
            if let Some(probe) = self.interaction.as_mut() {
                probe.empty_hint = Some(message.clone());
            }
            let mut clear_query = false;
            let mut all_controllers = false;
            let mut use_controller = false;
            let mut retry = false;
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                if !self.query.trim().is_empty()
                    && secondary_button(ui, t("steaminput.clear_search")).clicked()
                {
                    clear_query = true;
                }
                if self.controller.is_some()
                    && secondary_button(ui, t("steaminput.search_all_controllers")).clicked()
                {
                    all_controllers = true;
                } else if self.controller.is_none()
                    && !report.controllers.is_empty()
                    && secondary_button(ui, t("steaminput.filter_my_controller")).clicked()
                {
                    use_controller = true;
                }
                if secondary_button(ui, t("steaminput.retry")).clicked() {
                    retry = true;
                }
            });
            if clear_query {
                self.query.clear();
            }
            if all_controllers {
                self.controller = None;
            }
            if use_controller {
                self.controller = report
                    .controllers
                    .first()
                    .map(|controller| controller.kind.clone());
            }
            if clear_query || all_controllers || use_controller || retry {
                self.start_community(appid);
                ui.ctx().request_repaint();
            }
            return;
        }

        for profile in &profiles {
            self.community_card(ui, appid, profile);
            ui.add_space(8.0);
        }
        ui.add_space(2.0);
        muted(ui, t("steaminput.catalog_source"));
    }

    fn community_card(
        &mut self,
        ui: &mut egui::Ui,
        appid: u32,
        profile: &SteamInputCommunityProfile,
    ) {
        let busy = self.apply.is_some() || self.loading || self.community_loading;
        let installed = profile.installed || self.community_installed.contains(&profile.id);
        let other_game = profile.appid != appid;
        let mut metadata = steam_input_label(&profile.controller_type).to_string();
        if profile.rating > 0.0 {
            metadata.push_str(&t_fmt(
                "steaminput.rating",
                &[((profile.rating * 100.0).round() as i32).to_string()],
            ));
        }
        let mut use_profile = false;
        egui::Frame::none()
            .fill(CARD())
            .stroke(egui::Stroke::new(1.0_f32, BORDER()))
            .rounding(egui::Rounding::same(10.0))
            .inner_margin(egui::Margin::symmetric(12.0, 8.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                use_profile = profile_summary(
                    ui,
                    &display_title(&profile.title),
                    &metadata,
                    if other_game {
                        Some((t("steaminput.other_game"), WARN))
                    } else if installed {
                        Some((t("steaminput.downloaded"), OK))
                    } else {
                        None
                    },
                    Some((
                        if installed {
                            t("steaminput.reuse")
                        } else {
                            t("steaminput.download_use")
                        },
                        !busy,
                        if installed {
                            t("steaminput.reuse_hint")
                        } else {
                            t("steaminput.download_hint")
                        },
                    )),
                );
                egui::CollapsingHeader::new(t("steaminput.details"))
                    .id_salt(("steam-input-community-details", appid, profile.id))
                    .show(ui, |ui| {
                        if !profile.description.trim().is_empty() {
                            muted(ui, &profile.description);
                        }
                        muted(ui, creator_label(&profile.creator));
                        if other_game {
                            muted(
                                ui,
                                t_fmt("steaminput.origin_game", &[profile.appid.to_string()]),
                            );
                        }
                        if profile.rating > 0.0 {
                            muted(
                                ui,
                                t_fmt(
                                    "steaminput.votes",
                                    &[profile.votes_up.to_string(), profile.votes_down.to_string()],
                                ),
                            );
                        }
                        if profile.modified > 0 {
                            muted(
                                ui,
                                t_fmt(
                                    "steaminput.updated",
                                    &[lapinux_core::util::format_date(profile.modified)],
                                ),
                            );
                        }
                        if profile.file_size > 0 {
                            muted(
                                ui,
                                t_fmt(
                                    "steaminput.size",
                                    &[lapinux_core::util::format_bytes(profile.file_size)],
                                ),
                            );
                        }
                    });
            });
        if use_profile {
            self.start_apply_community(appid, profile.clone());
            ui.ctx().request_repaint();
        }
    }

    fn active_card(&mut self, ui: &mut egui::Ui, appid: u32, profile: &SteamInputProfile) {
        egui::Frame::none()
            .fill(CARD())
            .stroke(egui::Stroke::new(1.4_f32, ACCENT()))
            .rounding(egui::Rounding::same(10.0))
            .inner_margin(egui::Margin::symmetric(12.0, 8.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                profile_summary(
                    ui,
                    &display_title(&profile.title),
                    &profile_metadata(profile),
                    Some((t("steaminput.active"), OK)),
                    None,
                );
                local_profile_details(ui, appid, profile, "active");
            });
    }

    fn profile_card(&mut self, ui: &mut egui::Ui, appid: u32, profile: &SteamInputProfile) {
        let busy = self.apply.is_some() || self.loading;
        let mut use_profile = false;
        let marker = if profile.appid != 0 && profile.appid != appid {
            Some((t("steaminput.other_game"), WARN))
        } else {
            None
        };
        egui::Frame::none()
            .fill(CARD())
            .stroke(egui::Stroke::new(1.0_f32, BORDER()))
            .rounding(egui::Rounding::same(10.0))
            .inner_margin(egui::Margin::symmetric(12.0, 8.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                use_profile = profile_summary(
                    ui,
                    &display_title(&profile.title),
                    &profile_metadata(profile),
                    marker,
                    Some((t("steaminput.use"), !busy, t("steaminput.select_hint"))),
                );
                local_profile_details(ui, appid, profile, "local");
            });
        if use_profile {
            self.start_apply(appid, profile.clone());
            ui.ctx().request_repaint();
        }
    }

    fn start_apply(&mut self, appid: u32, profile: SteamInputProfile) {
        if self.apply.is_some() {
            return;
        }
        let Some(client) = self.client.clone() else {
            return;
        };
        let path = profile.path.clone();
        let title = profile.title.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = client
                .request(&Req::SteamInputApply {
                    appid,
                    profile: path,
                })
                .map_err(|error| error.to_string())
                .and_then(|envelope| {
                    envelope
                        .into_data::<SteamInputReport>()
                        .map_err(|error| error.to_string())
                });
            let _ = tx.send(result);
        });
        self.notice = Some((
            true,
            t_args("steaminput.applying", &[("title", title.as_str())]),
        ));
        self.apply = Some(ApplyJob {
            appid,
            title,
            kind: ApplyKind::Local,
            rx,
        });
    }

    fn start_apply_community(&mut self, appid: u32, profile: SteamInputCommunityProfile) {
        if self.apply.is_some() {
            return;
        }
        let Some(client) = self.client.clone() else {
            return;
        };
        let title = profile.title.clone();
        let id = profile.id;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = client
                .request(&Req::SteamInputCommunityApply { appid, profile })
                .map_err(|error| error.to_string())
                .and_then(|envelope| {
                    envelope
                        .into_data::<SteamInputReport>()
                        .map_err(|error| error.to_string())
                });
            let _ = tx.send(result);
        });
        self.notice = Some((
            true,
            t_args("steaminput.downloading", &[("title", title.as_str())]),
        ));
        self.apply = Some(ApplyJob {
            appid,
            title,
            kind: ApplyKind::Community(id),
            rx,
        });
    }
}

fn filtered<'a>(
    report: &'a SteamInputReport,
    appid: u32,
    query: &str,
    controller: Option<&str>,
) -> Vec<&'a SteamInputProfile> {
    let needle = query.trim().to_lowercase();
    let mut profiles: Vec<&SteamInputProfile> = report
        .profiles
        .iter()
        .filter(|profile| {
            controller.map_or(true, |kind| kind == profile.controller_type)
                && (needle.is_empty() || search_text(profile, report).contains(&needle))
        })
        .collect();
    profiles.sort_by(|left, right| {
        let left_connected = report
            .controllers
            .iter()
            .any(|controller| controller.kind == left.controller_type);
        let right_connected = report
            .controllers
            .iter()
            .any(|controller| controller.kind == right.controller_type);
        right
            .active
            .cmp(&left.active)
            .then_with(|| (right.appid == appid).cmp(&(left.appid == appid)))
            .then_with(|| right_connected.cmp(&left_connected))
            .then_with(|| right.modified.cmp(&left.modified))
            .then_with(|| left.title.to_lowercase().cmp(&right.title.to_lowercase()))
    });
    profiles
}

fn community_search_text(profile: &SteamInputCommunityProfile) -> String {
    format!(
        "{} {} {} {} {} {} {}",
        profile.title,
        profile.description,
        profile.creator,
        steam_input_label(&profile.controller_type),
        profile.controller_type,
        profile.appid,
        t("steaminput.community")
    )
    .to_lowercase()
}

fn search_text(profile: &SteamInputProfile, report: &SteamInputReport) -> String {
    let mut text = format!(
        "{} {} {} {} {} {} {}",
        profile.title,
        display_title(&profile.title),
        profile.description,
        profile.creator,
        steam_input_label(&profile.controller_type),
        profile.controller_type,
        profile.appid
    );
    for controller in &report.controllers {
        if controller.kind == profile.controller_type {
            text.push(' ');
            text.push_str(&controller.name);
            text.push(' ');
            text.push_str(&controller.label);
        }
    }
    text.to_lowercase()
}

/// Steam garde parfois des jetons de localisation dans le titre d'un profil
/// (`#Title`, `#Library_ControllerSaveDefaultTitle`). La fiche montre une
/// version lisible plutôt qu'un identifiant interne.
fn display_title(title: &str) -> String {
    let title = title.trim();
    if title.is_empty() {
        return t("steaminput.untitled").into();
    }
    let Some(token) = title.strip_prefix('#') else {
        return title.to_string();
    };
    if token.eq_ignore_ascii_case("title") {
        return t("steaminput.untitled").into();
    }
    if token.eq_ignore_ascii_case("library_controllersavedefaulttitle") {
        return t("steaminput.default_profile").into();
    }
    let words: Vec<String> = token
        .split('_')
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut out = String::new();
            let mut previous_lower = false;
            for character in word.chars() {
                if character.is_uppercase() && previous_lower {
                    out.push(' ');
                }
                out.push(character);
                previous_lower = character.is_lowercase();
            }
            out
        })
        .collect();
    if words.is_empty() {
        t("steaminput.untitled").into()
    } else {
        words.join(" ")
    }
}

fn game_label(profile: &SteamInputProfile, appid: u32) -> String {
    if profile.appid == appid && appid != 0 {
        t("steaminput.this_game").into()
    } else if profile.appid == 0 {
        t("steaminput.no_game").into()
    } else {
        t_fmt("steaminput.game", &[profile.appid.to_string()])
    }
}

fn profile_metadata(profile: &SteamInputProfile) -> String {
    format!(
        "{} · {}",
        steam_input_label(&profile.controller_type),
        origin_label(&profile.origin)
    )
}

fn creator_label(creator: &str) -> String {
    let creator = creator.trim();
    if creator.starts_with("0x") {
        t("steaminput.author_steam").into()
    } else if creator.is_empty() || creator.chars().all(|character| character.is_ascii_digit()) {
        t("steaminput.community").into()
    } else {
        t_args("steaminput.by", &[("creator", creator)])
    }
}

fn origin_label(origin: &str) -> &str {
    match origin {
        "jeu" | "game" => t("steaminput.origin_game_config"),
        "compte" | "account" => t("steaminput.origin_account"),
        "Lapinux" => t("steaminput.origin_lapinux"),
        _ => t("steaminput.origin_shared"),
    }
}

fn counted(count: usize, singular: &str, plural: &str) -> String {
    format!("{count} {}", if count > 1 { plural } else { singular })
}

/// Présentation commune : les informations comparables et l'action restent
/// visibles, tandis que la description et les métadonnées sont repliées.
fn profile_summary(
    ui: &mut egui::Ui,
    title: &str,
    metadata: &str,
    marker: Option<(&str, egui::Color32)>,
    action: Option<(&str, bool, &str)>,
) -> bool {
    let mut clicked = false;
    let show_text = |ui: &mut egui::Ui| {
        ui.add(
            egui::Label::new(egui::RichText::new(title).size(13.5).strong().color(TEXT()))
                .truncate(),
        )
        .on_hover_text(title);
        ui.horizontal_wrapped(|ui| {
            muted(ui, metadata);
            if let Some((label, color)) = marker {
                badge(ui, label, color);
            }
        });
    };
    let width = ui.available_width();
    if let Some((label, enabled, hint)) = action {
        if width >= 360.0 {
            ui.horizontal_top(|ui| {
                let action_width = 180.0;
                let gap = ui.spacing().item_spacing.x;
                ui.allocate_ui_with_layout(
                    egui::vec2((width - action_width - gap).max(0.0), 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width((width - action_width - gap).max(0.0));
                        show_text(ui);
                    },
                );
                ui.add_enabled_ui(enabled, |ui| {
                    clicked = primary_button(ui, label).on_hover_text(hint).clicked();
                });
            });
        } else {
            show_text(ui);
            ui.add_enabled_ui(enabled, |ui| {
                clicked = primary_button(ui, label).on_hover_text(hint).clicked();
            });
        }
    } else {
        show_text(ui);
    }
    clicked
}

fn local_profile_details(
    ui: &mut egui::Ui,
    appid: u32,
    profile: &SteamInputProfile,
    section: &str,
) {
    egui::CollapsingHeader::new(t("steaminput.profile_actions"))
        .id_salt(("steam-input-local-details", section, appid, &profile.path))
        .default_open(section == "active")
        .show(ui, |ui| {
            if !profile.description.trim().is_empty() {
                muted(ui, &profile.description);
            }
            ui.horizontal_wrapped(|ui| {
                badge(ui, &game_label(profile, appid), TEXT_DIM());
                badge(ui, origin_label(&profile.origin), TEXT_DIM());
                badge(ui, steam_input_label(&profile.controller_type), ACCENT());
                muted(ui, creator_label(&profile.creator));
                if profile.modified > 0 {
                    muted(ui, "·");
                    muted(
                        ui,
                        t_fmt(
                            "steaminput.modified",
                            &[lapinux_core::util::format_date(profile.modified)],
                        ),
                    );
                }
            });
            ui.add_space(5.0);
            action_details(
                ui,
                &profile.preset,
                profile.details_loaded,
                &profile.details_error,
                &profile.action_groups,
                &profile.settings,
                &format!("{section}-{}", profile.path.display()),
            );
            if ui.link(t("steaminput.show_folder")).clicked() {
                if let Some(parent) = profile.path.parent() {
                    open_path(parent);
                }
            }
        });
}

/// Détail d'une copie conservée dans Lapinux, partagé par la fiche jeu et
/// l'onglet Général des manettes.
pub(crate) fn saved_action_details(ui: &mut egui::Ui, saved: &SteamInputSaved) {
    action_details(
        ui,
        &saved.preset,
        saved.details_loaded,
        &saved.details_error,
        &saved.action_groups,
        &saved.settings,
        &format!("saved-{}", saved.path.display()),
    );
}

/// Groupes d'actions réellement lus dans le VDF. Une donnée absente est dite
/// absente : aucun mode, binding ou réglage n'est fabriqué pour remplir la vue.
fn action_details(
    ui: &mut egui::Ui,
    preset: &str,
    details_loaded: bool,
    details_error: &str,
    groups: &[SteamInputActionGroup],
    settings: &[SteamInputSetting],
    salt: &str,
) {
    if !details_error.trim().is_empty() {
        hint_box(
            ui,
            &format!("{} {details_error}", t("steaminput.details_unreadable")),
            WARN,
        );
        return;
    }
    if !details_loaded {
        hint_box(ui, t("steaminput.details_unloaded"), TEXT_DIM());
        return;
    }

    let actions: usize = groups.iter().map(|group| group.actions.len()).sum();
    ui.horizontal_wrapped(|ui| {
        if preset.trim().is_empty() {
            muted(ui, t("steaminput.preset_none"));
        } else {
            badge(
                ui,
                &t_args("steaminput.preset", &[("name", preset)]),
                ACCENT(),
            );
        }
        badge(
            ui,
            &t_args(
                "steaminput.actions_count",
                &[("count", &actions.to_string())],
            ),
            OK,
        );
    });
    muted(ui, t("steaminput.details_scope"));

    // Ce qui change vraiment : les entrées qui ne suivent pas le mapping
    // xinput standard (clavier, souris, gyro, macro). Le profil téléchargé se
    // lit ainsi en quelques lignes au lieu de la liste complète des entrées.
    let custom: Vec<(&SteamInputActionGroup, &SteamInputAction)> = groups
        .iter()
        .flat_map(|group| group.actions.iter().map(move |action| (group, action)))
        .filter(|(_, action)| !is_standard_binding(&action.binding))
        .collect();
    ui.add_space(6.0);
    section_heading(ui, t("steaminput.changes_title"), "");
    if custom.is_empty() {
        muted(ui, t("steaminput.changes_none"));
    } else {
        muted(
            ui,
            t_args(
                "steaminput.changes_count",
                &[("count", &custom.len().to_string())],
            ),
        );
        for (group, action) in custom.iter().take(8) {
            let binding = binding_label(&action.binding);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(format!(
                        "{} · {} → {binding}",
                        group_label(&group.source, &group.mode),
                        input_label(&action.input, &group.source)
                    ))
                    .size(10.0)
                    .color(TEXT_DIM()),
                )
                .truncate(),
            )
            .on_hover_text(action.binding.trim());
        }
        if custom.len() > 8 {
            muted(
                ui,
                t_args(
                    "steaminput.changes_more",
                    &[("count", &(custom.len() - 8).to_string())],
                ),
            );
        }
    }
    ui.add_space(6.0);
    // La liste complète reste accessible : aucune information ne disparaît.
    egui::CollapsingHeader::new(t("steaminput.all_inputs"))
        .id_salt(("steam-input-all", salt))
        .show(ui, |ui| {
    if groups.is_empty() {
        ui.add_space(4.0);
        hint_box(ui, t("steaminput.no_actions"), TEXT_DIM());
    }
    for (index, group) in groups.iter().enumerate() {
        ui.add_space(6.0);
        egui::Frame::none()
            .fill(PANEL())
            .stroke(egui::Stroke::new(1.0_f32, BORDER()))
            .rounding(egui::Rounding::same(8.0))
            .inner_margin(egui::Margin::symmetric(9.0, 7.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new(group_label(&group.source, &group.mode))
                            .size(11.5)
                            .strong()
                            .color(TEXT()),
                    );
                    if !group.mode.trim().is_empty() {
                        badge(ui, &humanize(&group.mode), TEXT_DIM());
                    }
                    badge(
                        ui,
                        &group.actions.len().to_string(),
                        if group.actions.is_empty() {
                            WARN
                        } else {
                            ACCENT()
                        },
                    );
                });
                if !group.settings.is_empty() {
                    ui.add_space(3.0);
                    settings_rows(
                        ui,
                        &group.settings,
                        ("steam-input-group-settings", salt, index),
                    );
                }
                if group.actions.is_empty() {
                    muted(ui, t("steaminput.no_actions"));
                    return;
                }
                ui.add_space(5.0);
                egui::Grid::new(("steam-input-actions", salt, index))
                    .num_columns(3)
                    .spacing([12.0, 3.0])
                    .striped(true)
                    .show(ui, |ui| {
                        muted(ui, t("steaminput.column_input"));
                        muted(ui, t("steaminput.column_activation"));
                        muted(ui, t("steaminput.column_binding"));
                        ui.end_row();
                        for action in &group.actions {
                            ui.label(
                                egui::RichText::new(input_label(&action.input, &group.source))
                                    .size(10.5)
                                    .strong()
                                    .color(TEXT()),
                            );
                            if action.activator.trim().is_empty() {
                                muted(ui, "—");
                            } else {
                                badge(ui, &humanize(&action.activator), TEXT_DIM());
                            }
                            let binding = binding_label(&action.binding);
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&binding)
                                        .size(9.5)
                                        .monospace()
                                        .color(TEXT_DIM()),
                                )
                                .truncate(),
                            )
                            .on_hover_text(action.binding.trim());
                            ui.end_row();
                        }
                    });
            });
    }
        });

    if !settings.is_empty() {
        ui.add_space(6.0);
        egui::Frame::none()
            .fill(PANEL())
            .stroke(egui::Stroke::new(1.0_f32, BORDER()))
            .rounding(egui::Rounding::same(8.0))
            .inner_margin(egui::Margin::symmetric(9.0, 7.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    egui::RichText::new(t("steaminput.profile_settings"))
                        .size(11.5)
                        .strong()
                        .color(TEXT()),
                );
                settings_rows(ui, settings, ("steam-input-profile-settings", salt));
            });
    }
}

/// Une sortie `xinput` suit le mapping Steam standard ; tout le reste est un
/// choix propre au profil, donc la partie intéressante d'un fichier téléchargé.
fn is_standard_binding(binding: &str) -> bool {
    binding.to_ascii_lowercase().contains("xinput_button")
}

fn settings_rows(ui: &mut egui::Ui, settings: &[SteamInputSetting], id: impl std::hash::Hash) {
    egui::Grid::new(id)
        .num_columns(2)
        .spacing([14.0, 3.0])
        .striped(true)
        .show(ui, |ui| {
            for setting in settings {
                muted(ui, setting_label(&setting.key));
                ui.label(
                    egui::RichText::new(&setting.value)
                        .size(10.0)
                        .monospace()
                        .color(TEXT()),
                );
                ui.end_row();
            }
        });
}

fn group_label(source: &str, mode: &str) -> String {
    let key = match source {
        "button_diamond" => Some("steaminput.group_buttons"),
        "joystick" => Some("steaminput.group_stick"),
        "right_joystick" => Some("steaminput.group_right_stick"),
        "trigger" | "left_trigger" | "right_trigger" => Some("steaminput.group_triggers"),
        "dpad" => Some("steaminput.group_dpad"),
        "trackpad" | "left_trackpad" | "right_trackpad" => Some("steaminput.group_trackpad"),
        "switch" => Some("steaminput.group_switches"),
        _ => None,
    };
    if let Some(key) = key {
        return t(key).to_string();
    }
    let raw = if mode.trim().is_empty() { source } else { mode };
    if raw.trim().is_empty() {
        t("steaminput.group_other").to_string()
    } else {
        humanize(raw)
    }
}

fn setting_label(key: &str) -> String {
    match key {
        "left_stick_deadzone" => t("steaminput.deadzone_left").to_string(),
        "right_stick_deadzone" => t("steaminput.deadzone_right").to_string(),
        _ => humanize(key),
    }
}

fn input_label(input: &str, source: &str) -> String {
    match input {
        "button_a" => "A".into(),
        "button_b" => "B".into(),
        "button_x" => "X".into(),
        "button_y" => "Y".into(),
        "button_escape" => "Start".into(),
        "button_menu" => "Menu".into(),
        "left_bumper" => "LB".into(),
        "right_bumper" => "RB".into(),
        "button_back_left" => "L4".into(),
        "button_back_right" => "R4".into(),
        "button_back_left_upper" => "L5".into(),
        "button_back_right_upper" => "R5".into(),
        "left_trigger" => "LT".into(),
        "right_trigger" => "RT".into(),
        "click" if source == "joystick" => "L3".into(),
        "click" if source == "right_joystick" => "R3".into(),
        "dpad_north" => "D-pad ↑".into(),
        "dpad_south" => "D-pad ↓".into(),
        "dpad_west" => "D-pad ←".into(),
        "dpad_east" => "D-pad →".into(),
        _ => humanize(input),
    }
}

fn binding_label(binding: &str) -> String {
    binding
        .trim()
        .trim_end_matches(|character: char| character == ',' || character.is_whitespace())
        .to_string()
}

fn humanize(value: &str) -> String {
    value.trim().replace('_', " ")
}

fn section_heading(ui: &mut egui::Ui, label: &str, detail: impl Into<String>) {
    ui.horizontal_wrapped(|ui| {
        ui.label(
            egui::RichText::new(label)
                .size(9.5)
                .strong()
                .color(ACCENT()),
        );
        muted(ui, detail);
    });
}

fn text_action(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(egui::Button::new(egui::RichText::new(label).size(10.5).color(ACCENT())).frame(false))
}

fn source_tab(ui: &mut egui::Ui, text: &str, selected: bool) -> egui::Response {
    let response = ui.add(
        egui::Button::new(
            egui::RichText::new(text)
                .size(10.5)
                .strong()
                .color(if selected { ACCENT() } else { TEXT_DIM() }),
        )
        .fill(if selected {
            ACCENT().gamma_multiply(0.12)
        } else {
            PANEL()
        })
        .stroke(egui::Stroke::new(
            if selected { 1.3_f32 } else { 1.0 },
            if selected { ACCENT() } else { BORDER() },
        ))
        .rounding(egui::Rounding::same(9.0))
        .min_size(egui::vec2(0.0, 28.0)),
    );
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

fn chip(ui: &mut egui::Ui, text: &str, selected: bool) -> egui::Response {
    let response = ui
        .add(
            egui::Button::new(
                egui::RichText::new(text)
                    .size(10.0)
                    .strong()
                    .color(if selected { BACKGROUND() } else { TEXT_DIM() }),
            )
            .fill(if selected { ACCENT() } else { PANEL() })
            .stroke(egui::Stroke::new(
                1.0_f32,
                if selected { ACCENT() } else { BORDER() },
            ))
            .rounding(egui::Rounding::same(12.0))
            .min_size(egui::vec2(0.0, 24.0)),
        )
        .on_hover_text(text);
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

fn badge(ui: &mut egui::Ui, text: &str, color: egui::Color32) {
    egui::Frame::none()
        .fill(color.gamma_multiply(0.16))
        .rounding(egui::Rounding::same(5.0))
        .inner_margin(egui::Margin::symmetric(6.0, 2.0))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).size(9.5).strong().color(color));
        });
}

fn notice_line(ui: &mut egui::Ui, ok: bool, text: &str) {
    let color = if ok { OK } else { WARN };
    ui.horizontal_wrapped(|ui| {
        let (dot, _) = ui.allocate_exact_size(egui::vec2(6.0, 6.0), egui::Sense::hover());
        ui.painter().circle_filled(dot.center(), 3.0, color);
        ui.label(
            egui::RichText::new(text)
                .size(10.5)
                .color(if ok { TEXT() } else { WARN }),
        );
    });
}

fn hint_box(ui: &mut egui::Ui, text: &str, color: egui::Color32) {
    egui::Frame::none()
        .fill(color.gamma_multiply(0.06))
        .stroke(egui::Stroke::new(1.0_f32, color.gamma_multiply(0.28)))
        .rounding(egui::Rounding::same(10.0))
        .inner_margin(10.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new(text).size(10.5).color(TEXT_DIM()));
        });
}

fn controller_row(
    ui: &mut egui::Ui,
    controller: &SteamInputController,
    selected: bool,
    width: f32,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 54.0), egui::Sense::click());
    let response = response.on_hover_text(format!("{} · {}", controller.name, controller.label));
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(rect));
    painter.rect_filled(
        rect,
        egui::Rounding::same(10.0),
        if selected { CARD_HOVER() } else { CARD() },
    );
    painter.rect_stroke(
        rect,
        egui::Rounding::same(10.0),
        egui::Stroke::new(
            if selected { 1.4_f32 } else { 1.0 },
            if selected { ACCENT() } else { BORDER() },
        ),
    );
    let icon = egui::Rect::from_min_size(rect.min + egui::vec2(10.0, 9.0), egui::vec2(36.0, 36.0));
    paint_controller(&painter, icon, if selected { ACCENT() } else { TEXT_DIM() });
    painter.text(
        egui::pos2(rect.left() + 54.0, rect.top() + 17.0),
        egui::Align2::LEFT_CENTER,
        &controller.name,
        egui::FontId::proportional(12.5),
        TEXT(),
    );
    painter.text(
        egui::pos2(rect.left() + 54.0, rect.top() + 35.0),
        egui::Align2::LEFT_CENTER,
        format!(
            "{} · {}",
            controller.label,
            if selected {
                t("steaminput.filter_active")
            } else {
                t("steaminput.filter_hint")
            }
        ),
        egui::FontId::proportional(10.0),
        if selected { ACCENT() } else { TEXT_DIM() },
    );
    response
}

fn paint_controller(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    let body = egui::Rect::from_center_size(
        rect.center(),
        egui::vec2(rect.width(), rect.height() * 0.52),
    );
    let rounding = egui::Rounding::same(rect.width() * 0.16);
    painter.rect_filled(body, rounding, color.gamma_multiply(0.12));
    painter.rect_stroke(
        body,
        rounding,
        egui::Stroke::new(1.0_f32, color.gamma_multiply(0.55)),
    );
    let dot = rect.width() * 0.075;
    let left = egui::pos2(body.left() + body.width() * 0.18, body.center().y);
    let right = egui::pos2(body.right() - body.width() * 0.18, body.center().y);
    painter.circle_filled(
        egui::pos2(body.left() + body.width() * 0.3, body.center().y),
        dot,
        color.gamma_multiply(0.5),
    );
    painter.circle_filled(
        egui::pos2(body.right() - body.width() * 0.3, body.center().y),
        dot,
        color.gamma_multiply(0.5),
    );
    let arm = rect.width() * 0.055;
    let stroke = egui::Stroke::new(1.4_f32, color.gamma_multiply(0.75));
    painter.line_segment(
        [
            egui::pos2(left.x - arm, left.y),
            egui::pos2(left.x + arm, left.y),
        ],
        stroke,
    );
    painter.line_segment(
        [
            egui::pos2(left.x, left.y - arm),
            egui::pos2(left.x, left.y + arm),
        ],
        stroke,
    );
    for (dx, dy) in [(-0.04, 0.0), (0.04, 0.0), (0.0, -0.055), (0.0, 0.055)] {
        painter.circle_filled(
            egui::pos2(right.x + rect.width() * dx, right.y + rect.width() * dy),
            dot * 0.55,
            color.gamma_multiply(0.65),
        );
    }
}

fn search_icon(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
    let painter = ui.painter();
    let center = rect.center() + egui::vec2(-1.0, -1.0);
    painter.circle_stroke(center, 4.5, egui::Stroke::new(1.2_f32, TEXT_DIM()));
    painter.line_segment(
        [
            egui::pos2(center.x + 3.5, center.y + 3.5),
            egui::pos2(rect.right() - 1.0, rect.bottom() - 1.0),
        ],
        egui::Stroke::new(1.2_f32, TEXT_DIM()),
    );
}

/// Captures hors écran de la vraie page Steam Input.
///
/// Les images ne sont pas une maquette : elles passent par `Shelf::page`, la
/// même colonne de fiche et les mêmes polices que l'application. Le rendu wgpu
/// lit ensuite la texture en RGBA pour produire les PNG de référence.
#[cfg(test)]
mod visual_snapshots {
    use super::*;
    use eframe::egui::{self, Pos2, Rect, Vec2};
    use egui_wgpu::{wgpu, ScreenDescriptor};
    use std::future::Future;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::task::{Context as TaskContext, Poll, Wake, Waker};

    const WIDTH: u32 = 1180;
    const PORTAL: u32 = 620;

    struct Case {
        name: &'static str,
        height: u32,
        shelf: Shelf,
    }

    #[test]
    fn steam_input_visual_states() {
        let snapshots = snapshot_dir();
        std::fs::create_dir_all(&snapshots)
            .expect("le dossier des captures Steam Input doit être créable");
        let (_gpu, _instance, device, queue) = gpu();
        let mut renderer =
            egui_wgpu::Renderer::new(&device, wgpu::TextureFormat::Rgba8Unorm, None, 1, false);

        for mut case in cases() {
            let rgba = render_case(&device, &queue, &mut renderer, &mut case.shelf, case.height);
            check_snapshot(case.name, WIDTH, case.height, &rgba, &snapshots);
        }
    }

    const INTERACTION_HEIGHT: u32 = 1400;
    fn full_profiles() -> [&'static str; 5] {
        [
            "Précision au gyroscope",
            lapinux_core::i18n::t("steaminput.default_profile"),
            "Steam Deck — portable",
            lapinux_core::i18n::t("steaminput.untitled"),
            "Détente au canapé",
        ]
    }

    #[test]
    fn une_recherche_toutes_vide_ne_change_pas_le_filtre() {
        let mut shelf = shelf(
            demo_report(vec![triton()], Vec::new(), Vec::new()),
            "",
            None,
        );
        shelf.source = Source::Community;
        let key = shelf.community_key(PORTAL);
        let (tx, rx) = mpsc::channel();
        tx.send((
            PORTAL,
            key.clone(),
            Ok(SteamInputCommunityReport {
                profiles: Vec::new(),
                total: 0,
                page: 1,
                per_page: 20,
                fallback: false,
                notes: Vec::new(),
            }),
        ))
        .unwrap();
        shelf.community_rx = Some(rx);
        shelf.community_loading = true;
        shelf.community_pending = Some((PORTAL, key));

        shelf.poll_community(&egui::Context::default());

        assert_eq!(shelf.controller, None);
        assert!(shelf.community.contains_key(&PORTAL));
    }

    #[test]
    fn le_filtre_initial_prefere_la_manette_detectee_sans_ecraser_toutes() {
        let report = demo_report(vec![triton()], Vec::new(), Vec::new());
        let mut state = shelf(report.clone(), "", None);
        state.initialize_community_filter(PORTAL, &report);
        assert_eq!(state.controller.as_deref(), Some("controller_triton"));
        assert!(state.community_key(PORTAL).ends_with("controller_triton"));
        state.controller = None;
        state.initialize_community_filter(PORTAL, &report);
        assert_eq!(state.controller, None);

        let multiple = demo_report(vec![triton(), ps5()], Vec::new(), Vec::new());
        let mut shelf = shelf(multiple.clone(), "", None);
        shelf.initialize_community_filter(PORTAL, &multiple);
        assert_eq!(shelf.controller, None);
    }

    #[test]
    fn une_communaute_non_vide_ne_selectionne_pas_une_manette() {
        let mut shelf = shelf(
            demo_report(vec![triton(), ps5()], Vec::new(), Vec::new()),
            "",
            None,
        );
        shelf.source = Source::Community;
        let key = shelf.community_key(PORTAL);
        let (tx, rx) = mpsc::channel();
        tx.send((
            PORTAL,
            key.clone(),
            Ok(SteamInputCommunityReport {
                profiles: vec![
                    community_profile(11, "Triton", "controller_triton"),
                    community_profile(22, "DualSense", "controller_ps5"),
                ],
                total: 2,
                page: 1,
                per_page: 20,
                fallback: false,
                notes: Vec::new(),
            }),
        ))
        .unwrap();
        shelf.community_rx = Some(rx);
        shelf.community_loading = true;
        shelf.community_pending = Some((PORTAL, key));

        shelf.poll_community(&egui::Context::default());

        assert_eq!(shelf.controller, None);
        assert_eq!(
            shelf
                .community
                .get(&PORTAL)
                .map(|report| report.profiles.len()),
            Some(2)
        );
    }

    #[test]
    fn un_etat_vide_indique_le_jeu_et_la_manette() {
        let mut shelf = shelf(
            demo_report(vec![triton()], Vec::new(), Vec::new()),
            "",
            Some("controller_triton"),
        );
        shelf.source = Source::Community;
        shelf.community.insert(
            PORTAL,
            SteamInputCommunityReport {
                profiles: Vec::new(),
                total: 0,
                page: 1,
                per_page: 20,
                fallback: false,
                notes: Vec::new(),
            },
        );
        shelf.interaction = Some(InteractionProbe::default());
        let ctx = visual_context();
        let mut time = 0.0;
        for _ in 0..3 {
            interaction_frame(&ctx, &mut shelf, Vec::new(), &mut time);
        }
        let probe = shelf.interaction.as_ref().expect("probe présente");
        assert_eq!(
            probe.results_detail,
            lapinux_core::i18n::t("steaminput.community_empty")
        );
        let hint = probe.empty_hint.as_deref().unwrap_or_default();
        assert!(hint.contains("appid 620"), "{hint}");
        assert!(hint.contains("Steam Controller"), "{hint}");
    }

    #[test]
    fn la_fiche_jeu_peint_les_actions_reelles_du_profil_actif() {
        let mut shelf = shelf(
            demo_report(vec![ps5()], demo_profiles(), Vec::new()),
            "",
            None,
        );
        let ctx = visual_context();
        let mut time = 0.0;
        let mut output = None;
        for _ in 0..3 {
            output = Some(page_frame(&ctx, &mut shelf, WIDTH, 1900, Vec::new(), time));
            time += 1.0 / 60.0;
        }
        let texts = painted_texts(&output.expect("une frame doit être peinte"));
        for expected in [
            t("steaminput.profile_actions"),
            t("steaminput.changes_title"),
            t("steaminput.all_inputs"),
            t("steaminput.deadzone_left"),
        ] {
            assert!(
                texts.iter().any(|text| text.contains(expected)),
                "la fiche doit peindre « {expected} » : {texts:?}"
            );
        }
        // La liste complète est repliée : seuls les vrais changements restent
        // visibles d'emblée, et le remappage personnalisé en fait partie.
        assert!(texts.iter().any(|text| text.contains("mouse_button LEFT")));
        assert!(texts.iter().any(|text| text.contains("Default")));
        assert!(texts.iter().any(|text| text == "0.15"));
    }

    #[test]
    fn la_vue_manettes_peint_le_detail_du_profil_selectionne() {
        let saved = SteamInputSaved {
            id: 42,
            appid: PORTAL,
            source_appid: PORTAL,
            title: "Campagne Portal 2 — DualSense".into(),
            description: "Profil conservé pour le test.".into(),
            controller_type: "controller_ps5".into(),
            creator: "Valve".into(),
            modified: 1_760_000_000,
            saved_at: 1_760_000_100,
            path: PathBuf::from("/tmp/lapinux/community-42.vdf"),
            preset: "Default".into(),
            details_loaded: true,
            details_error: String::new(),
            action_groups: demo_action_groups(),
            settings: vec![SteamInputSetting {
                key: "left_stick_deadzone".into(),
                value: "0.15".into(),
            }],
        };
        let ctx = visual_context();
        let mut output = None;
        for _ in 0..2 {
            output = Some(ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 900.0))),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            crate::settings_card(
                                ui,
                                t("controllers.saved_profiles_title"),
                                t("controllers.saved_profiles_hint"),
                                |ui| {
                                    egui::CollapsingHeader::new(format!(
                                        "{} · {} · {}",
                                        saved.title,
                                        t("steaminput.this_game"),
                                        lapinux_core::model::steam_input_label(
                                            &saved.controller_type
                                        )
                                    ))
                                    .default_open(true)
                                    .show(ui, |ui| {
                                        crate::steaminput::saved_action_details(ui, &saved);
                                    });
                                },
                            );
                        });
                    });
                },
            ));
        }
        let texts = painted_texts(&output.expect("une frame doit être peinte"));
        for expected in [
            t("controllers.saved_profiles_title"),
            t("steaminput.changes_title"),
            t("steaminput.all_inputs"),
            t("steaminput.deadzone_left"),
        ] {
            assert!(
                texts.iter().any(|text| text.contains(expected)),
                "la vue manettes doit peindre « {expected} » : {texts:?}"
            );
        }
        assert!(texts.iter().any(|text| text.contains("Campagne Portal 2")));
        assert!(texts.iter().any(|text| text.contains("mouse_button LEFT")));
    }

    #[test]
    fn un_profil_sans_detail_affiche_un_etat_honnete() {
        let saved = SteamInputSaved {
            id: 43,
            appid: PORTAL,
            title: "Profil non chargé".into(),
            controller_type: "controller_ps5".into(),
            path: PathBuf::from("/tmp/lapinux/community-43.vdf"),
            ..Default::default()
        };
        let ctx = visual_context();
        let mut output = None;
        for _ in 0..2 {
            output = Some(ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 400.0))),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        crate::steaminput::saved_action_details(ui, &saved);
                    });
                },
            ));
        }
        let texts = painted_texts(&output.expect("une frame doit être peinte"));
        assert!(texts
            .iter()
            .any(|text| text.contains(t("steaminput.details_unloaded"))));
        assert!(!texts.iter().any(|text| text.contains("xinput_button")));
    }

    fn painted_texts(output: &egui::FullOutput) -> Vec<String> {
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
        let mut out = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut out);
        }
        out
    }

    #[test]
    fn steam_input_community_masque_les_configurations_deja_presentes() {
        let mut profiles = demo_profiles();
        profiles[0].workshop_id = 111;
        let mut shelf = shelf(demo_report(vec![ps5()], profiles, Vec::new()), "", None);
        shelf.source = Source::Community;
        shelf.community.insert(
            PORTAL,
            SteamInputCommunityReport {
                profiles: vec![
                    community_profile(111, "Déjà installée", "controller_ps5"),
                    community_profile(222, "Gyro communautaire", "controller_ps5"),
                ],
                total: 2,
                page: 1,
                per_page: 20,
                fallback: false,
                notes: Vec::new(),
            },
        );
        shelf.interaction = Some(InteractionProbe::default());
        let ctx = visual_context();
        let mut time = 0.0;
        for _ in 0..3 {
            interaction_frame(&ctx, &mut shelf, Vec::new(), &mut time);
        }
        let probe = shelf.interaction.as_ref().expect("probe présente");
        assert_eq!(
            probe.displayed_profiles,
            vec!["Gyro communautaire".to_string()],
            "une configuration déjà présente sur le PC doit être masquée"
        );
        assert_eq!(
            probe.results_detail,
            format!(
                "{} · {}",
                counted(1, t("steaminput.result_one"), t("steaminput.result_many")),
                lapinux_core::i18n::t_fmt("steaminput.on_steam", &["2".to_string()])
            )
        );
    }

    #[test]
    fn steam_input_controller_filter_clicks() {
        let mut shelf = shelf(
            demo_report(
                vec![ps5(), xbox(), switch_pro()],
                demo_profiles(),
                Vec::new(),
            ),
            "",
            None,
        );
        shelf.interaction = Some(InteractionProbe::default());
        let ctx = visual_context();
        let mut time = 0.0;
        for _ in 0..3 {
            interaction_frame(&ctx, &mut shelf, Vec::new(), &mut time);
        }

        assert_displayed(
            &shelf,
            &full_profiles(),
            &counted(
                5,
                t("steaminput.other_profile_one"),
                t("steaminput.other_profile_many"),
            ),
            None,
        );
        assert_active_absent(&shelf);

        // Filtre sur la manette Xbox : seuls ses deux profils restent.
        let xbox_rect = controller_rect(&shelf, "controller_xboxone");
        click_at(&ctx, &mut shelf, xbox_rect.center(), &mut time);
        assert_eq!(shelf.controller.as_deref(), Some("controller_xboxone"));
        assert_displayed(
            &shelf,
            &[
                "Précision au gyroscope",
                lapinux_core::i18n::t("steaminput.default_profile"),
            ],
            &counted(2, t("steaminput.result_one"), t("steaminput.result_many")),
            None,
        );
        {
            let probe = shelf.interaction.as_ref().expect("probe présente");
            assert!(
                probe
                    .displayed_profiles
                    .iter()
                    .any(|title| title == "Précision au gyroscope"),
                "le profil Xbox doit rester affiché"
            );
            assert!(
                !probe
                    .displayed_profiles
                    .iter()
                    .any(|title| title == "Steam Deck — portable"),
                "le profil Steam Deck doit disparaître du filtre Xbox"
            );
        }

        // Re-clic sur la même carte : retour à la liste complète.
        let xbox_rect = controller_rect(&shelf, "controller_xboxone");
        click_at(&ctx, &mut shelf, xbox_rect.center(), &mut time);
        assert_eq!(shelf.controller, None);
        assert_displayed(
            &shelf,
            &full_profiles(),
            &counted(
                5,
                t("steaminput.other_profile_one"),
                t("steaminput.other_profile_many"),
            ),
            None,
        );

        // Une manette branchée sans profil associé doit produire l'état vide.
        let switch_rect = controller_rect(&shelf, "controller_switch_pro");
        click_at(&ctx, &mut shelf, switch_rect.center(), &mut time);
        assert_eq!(
            shelf.controller.as_deref(),
            Some("controller_switch_pro"),
            "la carte Switch Pro doit devenir le filtre actif"
        );
        assert_displayed(
            &shelf,
            &[],
            &counted(0, t("steaminput.result_one"), t("steaminput.result_many")),
            Some(t("steaminput.empty_other_no_match")),
        );

        // « Toutes » doit aussi ramener la liste complète.
        let all_rect = chip_rect(&shelf, None);
        click_at(&ctx, &mut shelf, all_rect.center(), &mut time);
        assert_eq!(shelf.controller, None);
        assert_displayed(
            &shelf,
            &full_profiles(),
            &counted(
                5,
                t("steaminput.other_profile_one"),
                t("steaminput.other_profile_many"),
            ),
            None,
        );
    }

    fn interaction_frame(
        ctx: &egui::Context,
        shelf: &mut Shelf,
        events: Vec<egui::Event>,
        time: &mut f64,
    ) {
        let stamp = *time;
        *time += 1.0 / 60.0;
        let _ = page_frame(ctx, shelf, WIDTH, INTERACTION_HEIGHT, events, stamp);
    }

    fn click_at(ctx: &egui::Context, shelf: &mut Shelf, pos: egui::Pos2, time: &mut f64) {
        interaction_frame(ctx, shelf, vec![egui::Event::PointerMoved(pos)], time);
        interaction_frame(
            ctx,
            shelf,
            vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            }],
            time,
        );
        interaction_frame(
            ctx,
            shelf,
            vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            }],
            time,
        );
        // Une frame calme laisse le panneau reconstruire son état après le clic.
        interaction_frame(ctx, shelf, Vec::new(), time);
    }

    fn controller_rect(shelf: &Shelf, kind: &str) -> egui::Rect {
        shelf
            .interaction
            .as_ref()
            .expect("le probe d'interaction doit être actif")
            .controller_cards
            .iter()
            .find_map(|(candidate, rect)| (candidate == kind).then_some(*rect))
            .unwrap_or_else(|| panic!("carte manette absente : {kind}"))
    }

    fn chip_rect(shelf: &Shelf, kind: Option<&str>) -> egui::Rect {
        shelf
            .interaction
            .as_ref()
            .expect("le probe d'interaction doit être actif")
            .chips
            .iter()
            .find_map(|(candidate, rect)| (candidate.as_deref() == kind).then_some(*rect))
            .unwrap_or_else(|| panic!("chip manette absent : {kind:?}"))
    }

    fn assert_displayed(shelf: &Shelf, expected: &[&str], detail: &str, empty_hint: Option<&str>) {
        let probe = shelf
            .interaction
            .as_ref()
            .expect("le probe d'interaction doit être actif");
        let expected: Vec<String> = expected.iter().map(|title| (*title).to_owned()).collect();
        assert_eq!(
            probe.displayed_profiles, expected,
            "la liste réellement rendue ne correspond pas au filtre"
        );
        assert_eq!(probe.results_detail, detail, "compteur de résultats");
        assert_eq!(probe.empty_hint.as_deref(), empty_hint, "état vide");
    }

    fn assert_active_absent(shelf: &Shelf) {
        let probe = shelf
            .interaction
            .as_ref()
            .expect("le probe d'interaction doit être actif");
        assert!(
            !probe
                .displayed_profiles
                .iter()
                .any(|title| title == "Campagne Portal 2 — DualSense"),
            "la configuration active est déjà affichée dans sa propre carte"
        );
    }

    fn cases() -> Vec<Case> {
        vec![
            Case {
                name: "01-config-active.png",
                height: 1500,
                shelf: shelf(
                    demo_report(vec![ps5(), xbox()], demo_profiles(), Vec::new()),
                    "",
                    None,
                ),
            },
            Case {
                name: "02-sans-manette.png",
                height: 1400,
                shelf: shelf(
                    demo_report(
                        Vec::new(),
                        demo_profiles(),
                        vec!["Aucune manette détectée : tous les profils sont montrés.".into()],
                    ),
                    "",
                    None,
                ),
            },
            Case {
                name: "03-aucun-profil.png",
                height: 800,
                shelf: shelf(
                    demo_report(
                        vec![ps5()],
                        Vec::new(),
                        vec![lapinux_core::i18n::t("steaminput.no_profile_found").to_string()],
                    ),
                    "",
                    None,
                ),
            },
            Case {
                name: "04-aucun-resultat.png",
                height: 900,
                shelf: shelf(
                    demo_report(vec![ps5(), xbox()], demo_profiles(), Vec::new()),
                    "switch",
                    None,
                ),
            },
            Case {
                name: "05-recherche-filtre.png",
                height: 1100,
                shelf: shelf(
                    demo_report(vec![ps5(), xbox()], demo_profiles(), Vec::new()),
                    "deck",
                    Some("controller_neptune"),
                ),
            },
            Case {
                name: "06-communaute.png",
                height: 1320,
                shelf: community_shelf(),
            },
        ]
    }

    fn community_shelf() -> Shelf {
        let mut shelf = shelf(
            demo_report(vec![ps5(), xbox()], demo_profiles(), Vec::new()),
            "",
            None,
        );
        shelf.source = Source::Community;
        let mut profiles = vec![
            community_profile(791932662, "Gyro FPS — DualSense", "controller_ps5"),
            community_profile(938932606, "Portal 2 — manette Xbox", "controller_xboxone"),
            community_profile(812345678, "Steam Deck — gyro précis", "controller_neptune"),
        ];
        // Le cas réel d'un jeu sans configuration publiée : les résultats
        // viennent d'autres jeux et doivent rester clairement identifiés.
        profiles[0].appid = 960910;
        profiles[1].appid = 1671210;
        profiles[2].appid = 427520;
        shelf.community.insert(
            PORTAL,
            SteamInputCommunityReport {
                profiles,
                total: 9297,
                page: 1,
                per_page: 20,
                fallback: true,
                notes: vec![
                    "Aucune configuration n'est publiée pour ce jeu. Les configurations affichées viennent d'autres jeux et restent applicables à celui-ci.".into(),
                ],
            },
        );
        shelf
    }

    fn shelf(report: SteamInputReport, query: &str, controller: Option<&str>) -> Shelf {
        let mut shelf = Shelf::default();
        // Le catalogue communautaire s’ouvre par défaut dans l’application ;
        // ces harnais décrivent la vue des profils du disque, ils le choisissent
        // donc explicitement.
        shelf.source = Source::Local;
        shelf.reports.insert(PORTAL, report);
        shelf.query = query.into();
        shelf.controller = controller.map(str::to_owned);
        shelf
    }

    #[test]
    fn le_catalogue_communautaire_s_ouvre_en_premier() {
        assert_eq!(Shelf::default().source, Source::Community);
    }

    #[test]
    fn les_auteurs_steam_et_anonymes_sont_nommes() {
        assert_eq!(creator_label("0x1234"), t("steaminput.author_steam"));
        assert_eq!(creator_label("76561198000000000"), t("steaminput.community"));
        assert_eq!(creator_label("  "), t("steaminput.community"));
        assert_eq!(
            creator_label(" Valve "),
            t_args("steaminput.by", &[("creator", "Valve")])
        );
    }

    fn demo_report(
        controllers: Vec<SteamInputController>,
        profiles: Vec<SteamInputProfile>,
        notes: Vec<String>,
    ) -> SteamInputReport {
        SteamInputReport {
            controllers,
            profiles,
            available: true,
            notes,
        }
    }

    fn ps5() -> SteamInputController {
        controller(
            "controller_ps5",
            "DualSense Wireless Controller",
            "DualSense",
            0x054c,
            0x0ce6,
        )
    }

    fn triton() -> SteamInputController {
        controller(
            "controller_triton",
            "Steam Controller",
            "Steam Controller",
            0x28de,
            0x1304,
        )
    }

    fn xbox() -> SteamInputController {
        controller(
            "controller_xboxone",
            "Xbox Wireless Controller",
            "Manette Xbox",
            0x045e,
            0x0b13,
        )
    }

    fn switch_pro() -> SteamInputController {
        controller(
            "controller_switch_pro",
            "Nintendo Switch Pro Controller",
            "Nintendo Switch Pro",
            0x057e,
            0x2009,
        )
    }

    fn controller(
        kind: &str,
        name: &str,
        label: &str,
        vendor: u16,
        product: u16,
    ) -> SteamInputController {
        SteamInputController {
            kind: kind.into(),
            label: label.into(),
            name: name.into(),
            vendor,
            product,
            connected: true,
        }
    }

    fn demo_profiles() -> Vec<SteamInputProfile> {
        vec![
            profile(
                "Campagne Portal 2 — DualSense",
                "Une configuration complète pour Portal 2 : gâchettes analogiques, gyroscope pour viser, vibrations fines et raccourcis pour les tests de saut. Réglée pour une session confortable sur DualSense, avec les menus, le zoom et les commandes vocales accessibles sans quitter les sticks. Cette description volontairement longue vérifie que les cartes ne débordent pas.",
                "controller_ps5",
                "Valve",
                "compte",
                1_760_000_000,
                true,
                "userdata/485264600/241100/remote/3123456789_controller_config.vdf",
            ),
            profile(
                "Précision au gyroscope",
                "Viser au gyroscope avec les gâchettes courtes, pour garder les pouces sur les sticks. Une création de l'Atelier Steam testée sur Portal 2.",
                "controller_xboxone",
                "ApertureFan",
                lapinux_core::i18n::t("collections.games_one"),
                1_759_900_000,
                false,
                "steamapps/workshop/content/241100/2876543210/controller_configs/controller_xboxone.vdf",
            ),
            profile(
                "#Library_ControllerSaveDefaultTitle",
                "La configuration par défaut enregistrée par Steam pour cette manette.",
                "controller_xboxone",
                "76561198000000000",
                "compte",
                1_759_800_000,
                false,
                "userdata/485264600/config/controller_configs/controller_xboxone.vdf",
            ),
            profile(
                "Steam Deck — portable",
                "Une disposition pensée pour le Steam Deck : commandes tactiles, pavé droit pour la caméra et boutons arrière pour les portails.",
                "controller_neptune",
                "JoueurExemple",
                "compte",
                1_759_700_000,
                false,
                "userdata/485264600/241100/remote/2987654321_controller_config.vdf",
            ),
            profile(
                "#Title",
                "Profil partagé sans nom lisible dans les fichiers Steam.",
                "controller_neptune",
                "",
                "partagé",
                1_759_600_000,
                false,
                "steamapps/common/Portal 2/controller_configs/controller_neptune.vdf",
            ),
            profile(
                "Détente au canapé",
                "Une configuration locale simple et confortable pour jouer posé, avec les vibrations activées.",
                "controller_xbox360",
                lapinux_core::i18n::t("steaminput.community"),
                "partagé",
                1_759_500_000,
                false,
                "userdata/485264600/241100/remote/2765432109_controller_config.vdf",
            ),
        ]
    }

    fn community_profile(
        id: u64,
        title: &str,
        controller_type: &str,
    ) -> SteamInputCommunityProfile {
        SteamInputCommunityProfile {
            id,
            appid: PORTAL,
            title: title.into(),
            description: "Une configuration publiée par la communauté Steam, avec gyroscope, gâchettes courtes et raccourcis pour garder les pouces sur les sticks. Le fichier sera téléchargé depuis le CDN Steam et appliqué à ce jeu.".into(),
            controller_type: controller_type.into(),
            creator: "ApertureFan".into(),
            download_url: format!("https://cdn.steamusercontent.com/ugc/{id}/"),
            file_size: 18_240,
            modified: 1_760_000_000,
            rating: 0.92,
            votes_up: 340,
            votes_down: 30,
            subscriptions: 12_000,
            installed: false,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn profile(
        title: &str,
        description: &str,
        controller_type: &str,
        creator: &str,
        origin: &str,
        modified: i64,
        active: bool,
        file: &str,
    ) -> SteamInputProfile {
        SteamInputProfile {
            appid: PORTAL,
            title: title.into(),
            description: description.into(),
            controller_type: controller_type.into(),
            creator: creator.into(),
            path: PathBuf::from("/home/joueur/.local/share/Steam").join(file),
            workshop_id: 0,
            modified,
            origin: origin.into(),
            active,
            preset: if active {
                "Default".into()
            } else {
                String::new()
            },
            details_loaded: active,
            details_error: String::new(),
            action_groups: if active {
                demo_action_groups()
            } else {
                Vec::new()
            },
            settings: if active {
                vec![SteamInputSetting {
                    key: "left_stick_deadzone".into(),
                    value: "0.15".into(),
                }]
            } else {
                Vec::new()
            },
        }
    }

    fn demo_action_groups() -> Vec<SteamInputActionGroup> {
        use lapinux_core::model::SteamInputAction;
        vec![
            SteamInputActionGroup {
                source: "button_diamond".into(),
                mode: "four_buttons".into(),
                actions: vec![
                    SteamInputAction {
                        input: "button_a".into(),
                        activator: "Full_Press".into(),
                        binding: "xinput_button A, , ".into(),
                    },
                    SteamInputAction {
                        input: "button_b".into(),
                        activator: "Full_Press".into(),
                        binding: "xinput_button B, , ".into(),
                    },
                ],
                settings: Vec::new(),
            },
            SteamInputActionGroup {
                source: "right_trackpad".into(),
                mode: "absolute_mouse".into(),
                actions: vec![SteamInputAction {
                    input: "click".into(),
                    activator: "Soft_Press".into(),
                    binding: "mouse_button LEFT, , ".into(),
                }],
                settings: Vec::new(),
            },
        ]
    }

    fn visual_context() -> egui::Context {
        let ctx = egui::Context::default();
        ctx.set_pixels_per_point(1.0);
        crate::apply_theme(&ctx);
        crate::install_fonts(&ctx);
        ctx
    }

    fn page_frame(
        ctx: &egui::Context,
        shelf: &mut Shelf,
        width: u32,
        height: u32,
        events: Vec<egui::Event>,
        time: f64,
    ) -> egui::FullOutput {
        let mut input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                Pos2::ZERO,
                Vec2::new(width as f32, height as f32),
            )),
            events,
            ..Default::default()
        };
        input.time = Some(time);
        ctx.run(input, |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::none().fill(BACKGROUND()))
                .show(ctx, |ui| {
                    crate::LapinuxApp::sheet_workshop_column(
                        ui,
                        ("steam-input-visual", PORTAL),
                        |ui| shelf.page(ui, PORTAL),
                    );
                });
        })
    }

    fn render_case(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        renderer: &mut egui_wgpu::Renderer,
        shelf: &mut Shelf,
        height: u32,
    ) -> Vec<u8> {
        let ctx = visual_context();

        let mut shapes = None;
        let mut pixels_per_point = 1.0;
        for frame in 0..3 {
            let output = page_frame(&ctx, shelf, WIDTH, height, Vec::new(), frame as f64 / 60.0);

            for (id, delta) in &output.textures_delta.set {
                renderer.update_texture(device, queue, *id, delta);
            }
            for id in &output.textures_delta.free {
                renderer.free_texture(id);
            }
            if frame == 2 {
                pixels_per_point = output.pixels_per_point;
                shapes = Some(output.shapes);
            }
        }

        let paint_jobs = ctx.tessellate(
            shapes.expect("la dernière frame egui doit produire des formes"),
            pixels_per_point,
        );
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("steam-input-snapshot"),
            size: wgpu::Extent3d {
                width: WIDTH,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let screen = ScreenDescriptor {
            size_in_pixels: [WIDTH, height],
            pixels_per_point,
        };
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("steam-input-snapshot"),
        });
        renderer.update_buffers(device, queue, &mut encoder, &paint_jobs, &screen);
        {
            let background = BACKGROUND();
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("steam-input-snapshot"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: background.r() as f64 / 255.0,
                            g: background.g() as f64 / 255.0,
                            b: background.b() as f64 / 255.0,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            renderer.render(&mut pass.forget_lifetime(), &paint_jobs, &screen);
        }

        let bytes_per_row = (WIDTH * 4).div_ceil(256) * 256;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("steam-input-snapshot-readback"),
            size: (bytes_per_row * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            wgpu::ImageCopyTexture {
                texture: &target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::ImageCopyBuffer {
                buffer: &readback,
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width: WIDTH,
                height,
                depth_or_array_layers: 1,
            },
        );
        queue.submit(Some(encoder.finish()));

        let slice = readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        let _ = device.poll(wgpu::Maintain::Wait);
        rx.recv()
            .expect("le readback wgpu doit répondre")
            .expect("le readback wgpu doit réussir");

        let mapped = slice.get_mapped_range();
        let mut rgba = vec![0u8; (WIDTH * height * 4) as usize];
        for row in 0..height {
            let source =
                &mapped[(row * bytes_per_row) as usize..(row * bytes_per_row + WIDTH * 4) as usize];
            let destination =
                &mut rgba[(row * WIDTH * 4) as usize..((row + 1) * WIDTH * 4) as usize];
            destination.copy_from_slice(source);
        }
        drop(mapped);
        readback.unmap();
        rgba
    }

    fn snapshot_dir() -> PathBuf {
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots/steam-input");
        if lapinux_core::i18n::french() {
            base
        } else {
            base.join(lapinux_core::i18n::active())
        }
    }

    fn check_snapshot(name: &str, width: u32, height: u32, rgba: &[u8], dir: &Path) {
        let path = dir.join(name);
        if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
            save_png(&path, width, height, rgba);
            return;
        }
        let expected = match image::open(&path) {
            Ok(image) => image.to_rgba8(),
            Err(error) => {
                let new_path = path.with_extension("new.png");
                save_png(&new_path, width, height, rgba);
                panic!(
                    "capture absente : {}\nrendu écrit dans {} ({error})",
                    path.display(),
                    new_path.display()
                );
            }
        };
        assert_eq!(
            expected.dimensions(),
            (width, height),
            "taille de capture inattendue pour {}",
            path.display()
        );
        let expected = expected.as_raw();
        let mut differing = 0usize;
        let mut max_delta = 0u8;
        for (left, right) in expected.chunks_exact(4).zip(rgba.chunks_exact(4)) {
            let delta = left
                .iter()
                .zip(right)
                .map(|(a, b)| a.abs_diff(*b))
                .max()
                .unwrap_or(0);
            max_delta = max_delta.max(delta);
            if delta > 3 {
                differing += 1;
            }
        }
        let ratio = differing as f64 / (width * height) as f64;
        if ratio > 0.002 {
            let new_path = path.with_extension("new.png");
            save_png(&new_path, width, height, rgba);
            panic!(
                "capture différente pour {} : {:.3} % de pixels, écart max {}\nrendu écrit dans {}",
                path.display(),
                ratio * 100.0,
                max_delta,
                new_path.display()
            );
        }
    }

    fn save_png(path: &Path, width: u32, height: u32, rgba: &[u8]) {
        image::save_buffer(path, rgba, width, height, image::ExtendedColorType::Rgba8)
            .unwrap_or_else(|error| panic!("écriture de {} impossible : {error}", path.display()));
    }

    fn gpu() -> (
        crate::snapshot_gpu::Guard,
        wgpu::Instance,
        wgpu::Device,
        wgpu::Queue,
    ) {
        // Le chargeur Vulkan n'est pas réentrant : une capture à la fois.
        let guard = crate::snapshot_gpu::serialized();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });
        let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .expect("aucun adaptateur GPU pour les captures Steam Input");
        let (device, queue) = block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("steam-input-snapshot"),
                ..Default::default()
            },
            None,
        ))
        .expect("création du device GPU impossible");
        (guard, instance, device, queue)
    }

    fn block_on<F: Future>(future: F) -> F::Output {
        struct ThreadWake(std::thread::Thread);
        impl Wake for ThreadWake {
            fn wake(self: Arc<Self>) {
                self.0.unpark();
            }
            fn wake_by_ref(self: &Arc<Self>) {
                self.0.unpark();
            }
        }

        let mut future = Box::pin(future);
        let waker = Waker::from(Arc::new(ThreadWake(std::thread::current())));
        let mut context = TaskContext::from_waker(&waker);
        loop {
            match future.as_mut().poll(&mut context) {
                Poll::Ready(output) => return output,
                Poll::Pending => std::thread::park(),
            }
        }
    }

    #[test]
    fn les_origines_anciennes_et_neutres_sont_lues() {
        for origin in ["jeu", "game"] {
            assert_eq!(
                origin_label(origin),
                t("steaminput.origin_game_config"),
                "{origin}"
            );
        }
        for origin in ["compte", "account"] {
            assert_eq!(
                origin_label(origin),
                t("steaminput.origin_account"),
                "{origin}"
            );
        }
        assert_eq!(origin_label("Lapinux"), t("steaminput.origin_lapinux"));
    }
}
