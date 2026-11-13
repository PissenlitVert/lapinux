//! Page **Téléchargements** : cartes dossier et débit, tâches rangées en
//! quatre catégories (en cours, programmes, en pause, historique) et cartes
//! larges avec leurs actions.
//!
//! La page possède son propre état (`Downloads`) :
//! brouillon de dossier, caches d'icônes et de mises à jour, débits lissés,
//! préférences d'affichage et test de débit. L'application lui prête ce qu'elle
//! seule sait obtenir du daemon (`self.jobs`, catalogue, disque, réglages) et
//! exécute les gestes que la page déclenche (`install_game`, `notify`,
//! enregistrement du dossier).
//!
//! Point d'entrée unique, appelé par le sélecteur de pages de `lib.rs` :
//! `LapinuxApp::downloads_page`.

use super::*;
use lapinux_core::model::JobReport;
use std::collections::VecDeque;

/// État que la page Téléchargements possède entre deux images. Les données du
/// daemon (tâches, catalogue, disque) restent à `LapinuxApp` : la page les lit
/// et les met en forme, elle ne les possède pas.
/// Icône de l'onglet Téléchargements pendant une tâche : l'icône du jeu,
/// sa couleur dominante et l'avancement, pour le halo lumineux.
pub(crate) struct NavBadge {
    pub icon: egui::TextureHandle,
    pub color: egui::Color32,
    pub progress: f32,
    pub running: bool,
}

/// Résultat d'un clic sur l'en-tête d'une catégorie : replier ou lancer
/// l'action de groupe. Les deux gestes sont exclusifs.
pub(crate) struct SectionHeader {
    pub(crate) toggle: bool,
    pub(crate) action: bool,
    /// Espace libre entre le compteur/action et le chevron : les vignettes
    /// d'une catégorie repliée s'y placent, sur la même ligne.
    pub(crate) free: egui::Rect,
}

/// Tâche qui mérite l'icône de jeu dans la barre du haut : uniquement un jeu
/// en cours de téléchargement. Une pause ou une tâche terminée rend l'icône
/// de base à l'onglet.
fn nav_badge_job(jobs: &[Job]) -> Option<&Job> {
    jobs.iter()
        .find(|job| job.state == JobState::Running && job.appid.is_some())
}

pub(crate) struct Downloads {
    /// Champ de saisie du dossier, ouvert par « Modifier ».
    dir_draft: Option<String>,
    /// Icônes des cartes de téléchargement, gardées en mémoire.
    art: DownloadArtCache,
    /// Débits lissés par tâche, calculés à partir des compteurs du daemon.
    rates: HashMap<String, (f32, f32)>,
    /// Débit moyen sur une fenêtre plus longue, réservé au temps restant :
    /// l'estimation ne doit pas sauter à chaque petit trou de réseau.
    eta_rates: HashMap<String, f32>,
    samples: HashMap<String, VecDeque<(Instant, u64, u64)>>,
    /// Dernier état connu de chaque tâche : une reprise repart d'une fenêtre
    /// de mesure vide, sinon les chunks déjà en vol à la reprise gonflent le
    /// débit d'un pic irréel (plusieurs Gb/s pendant une image).
    states: HashMap<String, JobState>,
    /// Historique agrégé (réseau, écriture) des dernières secondes, pour le
    /// graphique « en direct » de la colonne de droite.
    rate_history: VecDeque<(f32, f32)>,
    rate_history_at: Option<Instant>,
    /// Mises à jour disponibles, recalculées seulement quand la bibliothèque
    /// ou le catalogue changent, jamais à chaque image.
    updates: Vec<UpdateRow>,
    updates_dirty: bool,
    updates_show_all: bool,
    /// État replié/déplié de chaque catégorie. Toutes commencent ouvertes :
    /// une tâche terminée laisse son rapport accessible depuis sa carte.
    running_open: bool,
    programs_open: bool,
    paused_open: bool,
    updates_open: bool,
    /// Les plus anciennes tâches de l'historique attendent « Tout afficher »
    /// pour ne pas charger cinquante bannières d'un coup.
    history_open: bool,
    history_show_all: bool,
    /// Test de débit intégré : ping, descendant et ascendant mesurés en
    /// direct par un fil réseau dédié.
    speedtest: Speedtest,
    /// Plafonds de débit en cours d'édition. Conservés entre les images pour
    /// qu'un glissement de curseur ne soit pas écrasé par la config du daemon.
    limits: Option<lapinux_core::config::DownloadsConfig>,
    /// Dernière modification d'un curseur, pour enregistrer après un court
    /// temps d'arrêt sans écrire la configuration à chaque image.
    limits_apply_at: Option<Instant>,
    /// Section « Par launcher » de la carte des limites, repliée de base : un
    /// seul store la remplirait pour rien.
    limits_stores_open: bool,
    /// Rapport ouvert depuis une carte de l'historique.
    report_open: Option<String>,
    /// Panneau d'informations ouvert depuis le bouton « i » d'une carte.
    info_open: Option<String>,
    /// Glossaire des serveurs ouvert par le « i » de la carte.
    servers_help_open: bool,
    /// Serveurs CDN Steam, lus une fois par session depuis l'annuaire public
    /// (le même que celui du client Steam).
    servers: Option<Vec<lapinux_core::model::DownloadServer>>,
    servers_rx: Option<mpsc::Receiver<Vec<lapinux_core::model::DownloadServer>>>,
    /// Mises à jour publiées par les boutiques hors Steam, lues à la demande :
    /// chaque jeu installé coûte un appel à sa boutique.
    store_updates: Vec<UpdateRow>,
    store_updates_rx: Option<mpsc::Receiver<Vec<UpdateRow>>>,
    store_updates_at: Option<Instant>,
}

impl Downloads {
    pub(crate) fn new() -> Self {
        Self {
            dir_draft: None,
            art: DownloadArtCache::default(),
            rates: HashMap::new(),
            eta_rates: HashMap::new(),
            samples: HashMap::new(),
            states: HashMap::new(),
            rate_history: VecDeque::new(),
            rate_history_at: None,
            updates: Vec::new(),
            store_updates: Vec::new(),
            store_updates_rx: None,
            store_updates_at: None,
            updates_dirty: true,
            updates_show_all: false,
            updates_open: true,
            running_open: true,
            programs_open: true,
            paused_open: true,
            history_open: true,
            history_show_all: false,
            speedtest: Speedtest::default(),
            limits: None,
            limits_apply_at: None,
            limits_stores_open: false,
            report_open: None,
            info_open: None,
            servers_help_open: false,
            servers: None,
            servers_rx: None,
        }
    }

    /// Construit la page avec les catégories repliées enregistrées.
    pub(crate) fn from_config(config: &lapinux_core::config::DownloadsConfig) -> Self {
        let mut page = Self::new();
        page.running_open = !config.collapsed_sections.iter().any(|key| key == "running");
        page.programs_open = !config
            .collapsed_sections
            .iter()
            .any(|key| key == "programs");
        page.paused_open = !config.collapsed_sections.iter().any(|key| key == "paused");
        page.updates_open = !config.collapsed_sections.iter().any(|key| key == "updates");
        page.history_open = !config.collapsed_sections.iter().any(|key| key == "history");
        page.limits_stores_open = !config
            .collapsed_sections
            .iter()
            .any(|key| key == "limits_stores");
        page
    }

    /// Clés des catégories repliées, dans un ordre stable pour la configuration.
    fn collapsed_keys(&self) -> Vec<String> {
        let mut keys = Vec::new();
        if !self.running_open {
            keys.push("running".to_string());
        }
        if !self.programs_open {
            keys.push("programs".to_string());
        }
        if !self.paused_open {
            keys.push("paused".to_string());
        }
        if !self.updates_open {
            keys.push("updates".to_string());
        }
        if !self.history_open {
            keys.push("history".to_string());
        }
        if !self.limits_stores_open {
            keys.push("limits_stores".to_string());
        }
        keys
    }

    /// Relève la liste des serveurs Steam quand son fil a répondu.
    fn poll_servers(&mut self, ctx: &egui::Context) {
        let Some(rx) = self.servers_rx.take() else {
            return;
        };
        match rx.try_recv() {
            Ok(servers) => self.servers = Some(servers),
            Err(mpsc::TryRecvError::Empty) => {
                self.servers_rx = Some(rx);
                // L'annuaire arrive par un fil qui réveille l'écran : une image
                // par seconde suffit à ne rien manquer.
                ctx.request_repaint_after(Duration::from_secs(1));
            }
            Err(mpsc::TryRecvError::Disconnected) => self.servers = Some(Vec::new()),
        }
    }

    /// Relit l'annuaire des serveurs de contenu Steam en tâche de fond.
    fn load_servers(&mut self, ctx: &egui::Context) {
        if self.servers_rx.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.servers_rx = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(steam_content_servers());
            ctx.request_repaint();
        });
    }

    /// Icône de jeu + progression pour l'onglet de navigation. Seule une
    /// tâche en cours remplace le pictogramme ; dès qu'elle est en pause, en
    /// attente ou terminée, l'icône de base revient.
    pub(crate) fn nav_badge(
        &mut self,
        ctx: &egui::Context,
        paths: &LapinuxPaths,
        jobs: &[Job],
    ) -> Option<NavBadge> {
        let job = nav_badge_job(jobs)?;
        let appid = job.appid?;
        let icon = self.art.icon(ctx, paths, appid)?;
        let color = self.art.icon_color(appid).unwrap_or(ACCENT());
        let progress = if job.total > 0 {
            job.downloaded as f32 / job.total as f32
        } else {
            job.progress
        };
        Some(NavBadge {
            icon,
            color,
            progress: progress.clamp(0.0, 1.0),
            running: job.state == JobState::Running,
        })
    }

    /// Couleur dominante d'un jeu : ses barres de progression la reprennent au
    /// lieu du vert générique, qui jurait avec la jaquette.
    pub(crate) fn accent(
        &mut self,
        ctx: &egui::Context,
        paths: &LapinuxPaths,
        appid: u32,
    ) -> egui::Color32 {
        self.art.accent(ctx, paths, appid)
    }

    /// Le scan de la bibliothèque ou le catalogue ont changé : la liste des
    /// mises à jour est périmée.
    pub(crate) fn mark_updates_dirty(&mut self) {
        self.updates_dirty = true;
    }

    pub(crate) fn speedtest_running(&self) -> bool {
        self.speedtest.running
    }

    /// Recalcule la liste des mises à jour hors du dessin : une seule fois par
    /// changement de bibliothèque ou de catalogue.
    fn refresh_updates(&mut self, entries: &[LibraryEntry], catalog: Option<&SteamCatalog>) {
        if !self.updates_dirty {
            return;
        }
        self.updates_dirty = false;
        let mut rows = Vec::new();
        for entry in entries {
            if entry.update.is_none() {
                continue;
            }
            let details = catalog
                .and_then(|catalog| {
                    catalog
                        .entries
                        .iter()
                        .find(|item| item.appid == entry.appid)
                })
                .map(|item| item.details.clone());
            let size = details.as_ref().and_then(public_download_bytes);
            rows.push(UpdateRow {
                appid: entry.appid,
                name: entry.name.clone(),
                size,
                details,
                store: None,
                store_id: String::new(),
                platform: None,
            });
        }
        self.updates = rows;
    }
    /// Relève la liste des mises à jour de boutique quand elle arrive.
    fn poll_store_updates(&mut self) {
        let Some(rx) = &self.store_updates_rx else {
            return;
        };
        match rx.try_recv() {
            Ok(rows) => {
                self.store_updates = rows;
                self.store_updates_rx = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => self.store_updates_rx = None,
        }
    }

    /// Lance le test de débit complet (ping, descendant, ascendant). Le fil
    /// réseau ne bloque jamais l'interface et publie chaque mesure en direct.
    fn start_speedtest(&mut self) {
        if self.speedtest.running {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.speedtest = Speedtest {
            rx: Some(rx),
            running: true,
            ..Speedtest::default()
        };
        std::thread::spawn(move || {
            speedtest_run(tx);
        });
    }

    pub(crate) fn poll_speedtest(&mut self) {
        let Some(rx) = self.speedtest.rx.take() else {
            return;
        };
        let mut finished = false;
        let mut disconnected = false;
        loop {
            match rx.try_recv() {
                Ok(SpeedtestEvent::Stage(stage)) => self.speedtest.stage = stage,
                Ok(SpeedtestEvent::Ping(ms)) => self.speedtest.ping_ms = Some(ms),
                Ok(SpeedtestEvent::Down(rate)) => self.speedtest.down = Some(rate),
                Ok(SpeedtestEvent::Up(rate)) => self.speedtest.up = Some(rate),
                Ok(SpeedtestEvent::DownRange(low, high)) => {
                    self.speedtest.down_low = Some(low);
                    self.speedtest.down_high = Some(high);
                }
                Ok(SpeedtestEvent::UpRange(low, high)) => {
                    self.speedtest.up_low = Some(low);
                    self.speedtest.up_high = Some(high);
                }
                Ok(SpeedtestEvent::Elapsed(seconds)) => {
                    self.speedtest.elapsed = Some(seconds);
                }
                Ok(SpeedtestEvent::Warning(note)) => self.speedtest.note = Some(note),
                Ok(SpeedtestEvent::Failed(note)) => {
                    self.speedtest.note = Some(note);
                    finished = true;
                    break;
                }
                Ok(SpeedtestEvent::Done) => {
                    finished = true;
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            }
        }
        if self.speedtest.running && !finished && !disconnected {
            self.speedtest.rx = Some(rx);
        } else {
            self.speedtest.running = false;
            self.speedtest.stage = SpeedtestStage::Idle;
            if !finished && self.speedtest.note.is_none() {
                self.speedtest.note =
                    Some(lapinux_core::i18n::t("downloads.speedtest_interrupted").into());
            }
        }
    }

    /// Débits réseau et disque par tâche, mesurés sur une fenêtre de quatre
    /// secondes : les chunks arrivent par paquets d'un mégaoctet et le
    /// compteur du daemon n'avance qu'à la fin de chacun. Sur 450 ms la courbe
    /// tombait à zéro entre deux paquets et remontait d'un coup, ce qui faisait
    /// croire à une connexion instable alors que le débit moyen était constant.
    /// Quatre secondes contiennent assez de chunks pour lire une vraie vitesse.
    /// Le temps restant, lui, se base sur quinze secondes : il bouge moins.
    fn sample_rates(&mut self, jobs: &[Job]) {
        let now = Instant::now();
        let active: Vec<(String, JobState, u64, u64)> = jobs
            .iter()
            .filter(|job| {
                matches!(
                    job.state,
                    JobState::Running | JobState::Paused | JobState::Queued
                )
            })
            .map(|job| (job.id.clone(), job.state, job.network, job.written))
            .collect();
        const WINDOW: f32 = 4.0;
        const ETA_WINDOW: f32 = 15.0;
        for (id, state, network, written) in &active {
            if is_resume(self.states.get(id).copied(), *state) {
                self.samples.remove(id);
                self.rates.remove(id);
                self.eta_rates.remove(id);
            }
            let window = self.samples.entry(id.clone()).or_default();
            let due = window
                .back()
                .map(|(at, _, _)| now.duration_since(*at).as_secs_f32() >= 0.25)
                .unwrap_or(true);
            if due {
                window.push_back((now, *network, *written));
            }
            while window.len() > 2
                && now
                    .duration_since(window.front().map(|s| s.0).unwrap_or(now))
                    .as_secs_f32()
                    > ETA_WINDOW
            {
                window.pop_front();
            }
            let Some((first_at, first_network, first_written)) = window.front().copied() else {
                continue;
            };
            let elapsed = now.duration_since(first_at).as_secs_f32();
            if elapsed < 0.8 {
                continue;
            }
            // Le débit affiché reste réactif (4 s), mais le temps restant
            // s'appuie sur la moyenne de 15 s et se corrige lentement.
            let (rate_at, rate_network, rate_written) = window
                .iter()
                .find(|(at, _, _)| now.duration_since(*at).as_secs_f32() <= WINDOW)
                .copied()
                .unwrap_or((first_at, first_network, first_written));
            let rate_elapsed = now.duration_since(rate_at).as_secs_f32().max(0.25);
            let net = network.saturating_sub(rate_network) as f32 / rate_elapsed;
            let write = written.saturating_sub(rate_written) as f32 / rate_elapsed;
            let (old_net, old_write) = self.rates.get(id).copied().unwrap_or((net, write));
            self.rates.insert(
                id.clone(),
                (old_net * 0.35 + net * 0.65, old_write * 0.35 + write * 0.65),
            );
            let eta_net = network.saturating_sub(first_network) as f32 / elapsed;
            let old_eta = self.eta_rates.get(id).copied().unwrap_or(eta_net);
            self.eta_rates
                .insert(id.clone(), old_eta * 0.8 + eta_net * 0.2);
        }
        let ids: Vec<String> = active.iter().map(|(id, _, _, _)| id.clone()).collect();
        self.samples.retain(|id, _| ids.contains(id));
        self.rates.retain(|id, _| ids.contains(id));
        self.eta_rates.retain(|id, _| ids.contains(id));
        self.states.retain(|id, _| ids.contains(id));
        for (id, state, _, _) in &active {
            self.states.insert(id.clone(), *state);
        }

        // Un échantillon agrégé par seconde : le graphique avance à pas fixes
        // (un point par seconde) au lieu de suivre le rythme des images, qui
        // déforme la courbe quand la fenêtre ne redessine pas.
        if ids.is_empty() {
            self.rate_history.clear();
            self.rate_history_at = None;
            return;
        }
        let due = self
            .rate_history_at
            .map(|at| now.duration_since(at).as_secs_f32() >= 1.0)
            .unwrap_or(true);
        let (net, write) = ids
            .iter()
            .filter_map(|id| self.rates.get(id))
            .fold((0.0_f32, 0.0_f32), |(net, write), (job_net, job_write)| {
                (net + job_net, write + job_write)
            });
        if due {
            self.rate_history.push_back((net, write));
            while self.rate_history.len() > 60 {
                self.rate_history.pop_front();
            }
            self.rate_history_at = Some(now);
        } else if let Some(last) = self.rate_history.back_mut() {
            // Entre deux échantillons, le bord droit de la courbe suit le
            // débit affiché au lieu de rester sur un ancien point.
            *last = (net, write);
        }
    }
}

impl LapinuxApp {
    /// Page en deux colonnes : les téléchargements à gauche, le dossier et le
    /// test de débit à droite. La liste de gauche défile seule ; les repères
    /// de disque et de réseau restent visibles pendant le défilement.
    pub(crate) fn downloads_page(&mut self, ui: &mut egui::Ui) {
        // Une seule lecture des tâches par image : toute la page travaille
        // ensuite sur ce regroupement, jamais sur `self.jobs` directement.
        let sections = job_sections(&self.jobs);
        self.downloads.sample_rates(&self.jobs);
        self.load_catalog_if_needed();
        let entries = self.library_entries_all();
        self.downloads
            .refresh_updates(&entries, self.catalog.as_ref());

        let history: Vec<Job> = sections.history.iter().cloned().collect();
        let empty = sections.running.is_empty()
            && sections.programs.is_empty()
            && sections.paused.is_empty()
            && self.downloads.updates.is_empty()
            && self.downloads.store_updates.is_empty()
            && history.is_empty();

        let available = ui.available_width();
        // Colonne de droite : assez large pour la jauge et le test de débit,
        // jamais plus d'un tiers de la fenêtre.
        let right_width = (available * 0.32).clamp(280.0, 420.0);
        let gap = 16.0;
        let left_width = (available - right_width - gap).max(420.0);
        let height = ui.available_height();

        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(left_width, height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(left_width);
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            // La barre de défilement flottante recouvre le bord
                            // droit : on lui réserve sa largeur.
                            ui.set_width((left_width - 14.0).max(360.0));
                            if empty {
                                self.empty_downloads_hint(ui);
                            } else {
                                self.running_section(ui, &sections.running);
                                self.programs_section(ui, &sections.programs);
                                self.paused_section(ui, &sections.paused);
                                self.history_section(ui, &history);
                            }
                            // Les mises à jour se vérifient même sans tâche en
                            // cours : c'est justement le moment où on cherche
                            // ce qu'il y a à télécharger.
                            self.updates_section(ui, &sections);
                        });
                },
            );
            // Huit pixels de disposition plus huit explicites : seize d'écart.
            ui.add_space(8.0);
            ui.allocate_ui_with_layout(
                egui::vec2(right_width, height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(right_width);
                    // Les encarts de droite défilent comme la liste : le
                    // dernier (réseau) dépassait sous la fenêtre sans moyen
                    // d'y descendre.
                    egui::ScrollArea::vertical()
                        .id_salt("downloads-side")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.set_width((right_width - 14.0).max(240.0));
                            if !sections.running.is_empty()
                                || !sections.programs.is_empty()
                                || !sections.paused.is_empty()
                            {
                                let running = sections
                                    .running
                                    .iter()
                                    .chain(sections.programs.iter())
                                    .any(|job| job.state == JobState::Running);
                                self.live_rates_card(ui, right_width, running);
                                ui.add_space(TILE_GAP);
                            }
                            self.download_folder_card(ui, right_width);
                            ui.add_space(TILE_GAP);
                            self.download_limits_card(ui);
                            ui.add_space(TILE_GAP);
                            self.speedtest_card(ui);
                            ui.add_space(TILE_GAP);
                            self.download_servers_card(ui);
                        });
                },
            );
        });
        // Au-dessus de la page : le rapport choisi dans une carte de
        // l'historique et le panneau d'informations s'ouvrent dans leur propre
        // fenêtre, sans déplacer la liste.
        self.report_window(ui.ctx());
        self.info_window(ui.ctx());
        self.servers_help_window(ui.ctx());
    }

    /// Charge le catalogue dès qu'un compte est connu : il fournit le poids
    /// annoncé des mises à jour.
    fn load_catalog_if_needed(&mut self) {
        let known_account = !self.steam_user.trim().is_empty()
            || self
                .steam_status
                .as_ref()
                .map(|status| status.token_saved)
                .unwrap_or(false);
        if known_account && !self.catalog_loaded {
            self.load_catalog(false);
        }
    }

    fn empty_downloads_hint(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        egui::Frame::none()
            .fill(CARD())
            .stroke(egui::Stroke::new(1.0_f32, BORDER()))
            .rounding(egui::Rounding::same(12.0))
            .inner_margin(16.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(42.0, 42.0), egui::Sense::hover());
                    ui.painter()
                        .rect_filled(rect, egui::Rounding::same(10.0), CARD_HOVER());
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "⬇",
                        egui::FontId::proportional(19.0),
                        TEXT_DIM(),
                    );
                    ui.add_space(4.0);
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new(lapinux_core::i18n::t("downloads.empty_title"))
                                .size(13.0)
                                .color(TEXT()),
                        );
                        muted(ui, lapinux_core::i18n::t("downloads.empty_hint"));
                    });
                });
            });
    }

    /// En-tête repliable d'une catégorie : titre, nombre en clair, action
    /// éventuelle en petite icône, puis le chevron. Le compte n'est plus dans
    /// une pastille : il reste discret à côté du titre. L'action (suspendre,
    /// reprendre, effacer) occupe toujours le même emplacement, juste après
    /// le compteur, avec une infobulle.
    /// En-tête partagé avec la page Activité : même titre, même compte, même
    /// chevron que les catégories de Téléchargements.
    pub(crate) fn section_heading(
        ui: &mut egui::Ui,
        title: &str,
        detail: &str,
        action: Option<(HeroGlyph, &str)>,
        open: bool,
    ) -> SectionHeader {
        const HEIGHT: f32 = 26.0;
        let width = ui.available_width();
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(width, HEIGHT), egui::Sense::click());
        let response = response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(if open {
                lapinux_core::i18n::t("downloads.collapse")
            } else {
                lapinux_core::i18n::t("downloads.expand")
            });
        if response.hovered() {
            ui.painter().rect_filled(
                rect,
                egui::Rounding::same(6.0),
                with_alpha(CARD_HOVER(), 0.35),
            );
        }

        let painter = ui.painter().clone();
        let title_galley = painter.layout_no_wrap(title.to_string(), display_font(15.0), TEXT());
        painter.galley(
            egui::pos2(
                rect.left() + 2.0,
                rect.center().y - title_galley.size().y / 2.0,
            ),
            title_galley.clone(),
            TEXT(),
        );
        let mut x = rect.left() + 2.0 + title_galley.size().x + 8.0;
        let caret = egui::pos2(rect.right() - 12.0, rect.center().y);
        let right_limit = caret.x - 16.0;
        if !detail.is_empty() {
            let max_width = (right_limit - x).max(24.0);
            let text = lapinux_core::util::ellipsize(detail, (max_width / 6.0).max(3.0) as usize);
            let galley = painter.layout_no_wrap(text, egui::FontId::proportional(11.5), TEXT_DIM());
            painter.galley(
                egui::pos2(x, rect.center().y - galley.size().y / 2.0),
                galley.clone(),
                TEXT_DIM(),
            );
            x += galley.size().x + 6.0;
        }

        let mut action_clicked = false;
        if let Some((glyph, tooltip)) = action {
            if x + 22.0 <= right_limit {
                let action_rect = egui::Rect::from_center_size(
                    egui::pos2(x + 11.0, rect.center().y),
                    egui::vec2(22.0, 22.0),
                );
                let action_response = ui.interact(
                    action_rect,
                    ui.id().with(("section-action", title)),
                    egui::Sense::click(),
                );
                let hovered = action_response.hovered();
                if hovered || action_response.has_focus() {
                    ui.painter()
                        .rect_filled(action_rect, egui::Rounding::same(6.0), CARD_HOVER());
                }
                paint_hero_glyph(
                    ui.painter(),
                    action_rect.center(),
                    glyph,
                    if hovered { TEXT() } else { TEXT_DIM() },
                );
                action_clicked = action_response.clicked();
                let _ = action_response.on_hover_text(tooltip);
            }
        }

        // Les vignettes d'une catégorie repliée occupent la place restante,
        // jamais le titre ni le chevron.
        let free = egui::Rect::from_min_max(
            egui::pos2(x, rect.top() + 2.0),
            egui::pos2(right_limit - 4.0, rect.bottom() - 2.0),
        );

        let ink = if response.hovered() {
            TEXT()
        } else {
            TEXT_DIM()
        };
        let size = 4.2;
        let points = if open {
            vec![
                caret + egui::vec2(-size, -size * 0.45),
                caret + egui::vec2(0.0, size * 0.6),
                caret + egui::vec2(size, -size * 0.45),
            ]
        } else {
            vec![
                caret + egui::vec2(-size * 0.45, -size),
                caret + egui::vec2(size * 0.6, 0.0),
                caret + egui::vec2(-size * 0.45, size),
            ]
        };
        ui.painter()
            .add(egui::Shape::line(points, egui::Stroke::new(1.4_f32, ink)));

        let keyboard = response.has_focus()
            && ui.input(|input| {
                input.key_pressed(egui::Key::Enter) || input.key_pressed(egui::Key::Space)
            });
        SectionHeader {
            toggle: (response.clicked() || keyboard) && !action_clicked,
            action: action_clicked,
            free,
        }
    }

    /// Jeux qui avancent : une carte large par jeu, avec pause et annulation.
    fn running_section(&mut self, ui: &mut egui::Ui, jobs: &[Job]) {
        if jobs.is_empty() {
            return;
        }
        let open = self.downloads.running_open;
        // Une tâche dans la file n'a pas de pause : l'action de groupe ne
        // s'affiche que si au moins une tâche peut réellement être suspendue.
        let pausable = jobs
            .iter()
            .any(|job| matches!(job.state, JobState::Running | JobState::Paused));
        let header = Self::section_heading(
            ui,
            lapinux_core::i18n::t("downloads.active"),
            &jobs.len().to_string(),
            pausable.then_some((
                HeroGlyph::Pause,
                lapinux_core::i18n::t("downloads.pause_all"),
            )),
            open,
        );
        if header.action {
            self.pause_all(jobs, true);
        } else if header.toggle {
            self.downloads.running_open = !open;
            self.remember_download_sections();
        }
        if !open {
            self.collapsed_icons(ui, header.free, &job_icons(jobs));
            return;
        }
        ui.add_space(4.0);
        let action = self.hero_tiles(ui, jobs);
        self.apply_job_action(action);
        ui.add_space(18.0);
    }

    /// Programmes téléchargés par l'application (Proton, outils) : ils ont
    /// leur propre catégorie et ne se mêlent pas aux jeux. Le daemon ne gère
    /// pas la pause pour eux, la carte ne montre donc que l'annulation.
    fn programs_section(&mut self, ui: &mut egui::Ui, jobs: &[Job]) {
        if jobs.is_empty() {
            return;
        }
        let open = self.downloads.programs_open;
        let header = Self::section_heading(
            ui,
            lapinux_core::i18n::t("downloads.programs"),
            &jobs.len().to_string(),
            None,
            open,
        );
        if header.toggle {
            self.downloads.programs_open = !open;
            self.remember_download_sections();
        }
        if !open {
            self.collapsed_icons(ui, header.free, &job_icons(jobs));
            return;
        }
        ui.add_space(4.0);
        let action = self.hero_tiles(ui, jobs);
        self.apply_job_action(action);
        ui.add_space(18.0);
    }

    /// Téléchargements suspendus : la même carte que les autres catégories,
    /// avec « Tout reprendre » pour repartir au même point.
    fn paused_section(&mut self, ui: &mut egui::Ui, jobs: &[Job]) {
        if jobs.is_empty() {
            return;
        }
        let open = self.downloads.paused_open;
        let header = Self::section_heading(
            ui,
            lapinux_core::i18n::t("downloads.paused"),
            &jobs.len().to_string(),
            Some((
                HeroGlyph::Play,
                lapinux_core::i18n::t("downloads.resume_all"),
            )),
            open,
        );
        if header.action {
            self.pause_all(jobs, false);
        } else if header.toggle {
            self.downloads.paused_open = !open;
            self.remember_download_sections();
        }
        if !open {
            self.collapsed_icons(ui, header.free, &job_icons(jobs));
            return;
        }
        ui.add_space(4.0);
        let action = self.hero_tiles(ui, jobs);
        self.apply_job_action(action);
        ui.add_space(18.0);
    }

    /// Fenêtre du rapport ouvert depuis une carte de l'historique. Elle
    /// laisse la liste en place et se ferme par « Fermer » ou Échap.
    fn report_window(&mut self, ctx: &egui::Context) {
        let Some(id) = self.downloads.report_open.clone() else {
            return;
        };
        let Some(job) = self.jobs.iter().find(|job| job.id == id).cloned() else {
            self.downloads.report_open = None;
            return;
        };
        let Some(report) = report_for(&job) else {
            self.downloads.report_open = None;
            return;
        };
        let icon = job
            .appid
            .and_then(|appid| self.downloads.art.icon(ctx, &self.paths, appid));
        let screen = ctx.screen_rect();
        let width = 560.0_f32.min((screen.width() - 40.0).max(320.0));
        let pos = screen.center() - egui::vec2(width / 2.0, 220.0);
        let mut close = false;
        egui::Window::new(lapinux_core::i18n::t("downloads.report_title"))
            .id(egui::Id::new("download-report-window"))
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .default_width(width)
            .default_pos(pos)
            .frame(
                egui::Frame::window(&ctx.style())
                    .fill(PANEL())
                    .stroke(egui::Stroke::new(1.0_f32, BORDER()))
                    .rounding(egui::Rounding::same(12.0))
                    .inner_margin(16.0),
            )
            .show(ctx, |ui| {
                ui.set_width(width - 32.0);
                close = report_window_body(ui, &job, &report, icon.as_ref());
            });
        if close || ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.downloads.report_open = None;
        }
    }

    /// Fenêtre ouverte par le bouton « i » d'une carte : tous les champs que
    /// le daemon connaît du transfert, en cours comme terminé. Elle laisse la
    /// liste en place et se ferme par « Fermer » ou Échap.
    fn info_window(&mut self, ctx: &egui::Context) {
        let Some(id) = self.downloads.info_open.clone() else {
            return;
        };
        let Some(job) = self.jobs.iter().find(|job| job.id == id).cloned() else {
            self.downloads.info_open = None;
            return;
        };
        let rate = self
            .downloads
            .rates
            .get(&job.id)
            .map(|(net, _)| *net)
            .unwrap_or(0.0);
        // Noms des contenus additionnels demandés avec le jeu : le catalogue
        // les connaît, la tâche ne garde que leurs identifiants.
        let known = self.catalog.as_ref().and_then(|catalog| {
            catalog
                .entries
                .iter()
                .find(|entry| Some(entry.appid) == job.appid)
        });
        let dlc_names: Vec<String> = job
            .dlc
            .iter()
            .map(|id| {
                known
                    .and_then(|entry| entry.dlc.iter().find(|dlc| dlc.appid == *id))
                    .map(|dlc| dlc.name.clone())
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or_else(|| {
                        lapinux_core::i18n::t_args("downloads.content", &[("id", &id.to_string())])
                    })
            })
            .collect();
        let screen = ctx.screen_rect();
        let width = 520.0_f32.min((screen.width() - 40.0).max(320.0));
        let pos = screen.center() - egui::vec2(width / 2.0, 220.0);
        let mut close = false;
        egui::Window::new(lapinux_core::i18n::t("downloads.info_title"))
            .id(egui::Id::new("download-info-window"))
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .default_width(width)
            .default_pos(pos)
            .frame(
                egui::Frame::window(&ctx.style())
                    .fill(PANEL())
                    .stroke(egui::Stroke::new(1.0_f32, BORDER()))
                    .rounding(egui::Rounding::same(12.0))
                    .inner_margin(16.0),
            )
            .show(ctx, |ui| {
                ui.set_width(width - 32.0);
                close = info_window_body(ui, &job, rate, &dlc_names);
            });
        if close || ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.downloads.info_open = None;
        }
    }

    /// Fenêtre du glossaire des serveurs, ouverte par le bouton « i » de la
    /// carte. Elle garde les explications à un clic sans alourdir la colonne.
    fn servers_help_window(&mut self, ctx: &egui::Context) {
        if !self.downloads.servers_help_open {
            return;
        }
        let screen = ctx.screen_rect();
        let width = 520.0_f32.min((screen.width() - 40.0).max(300.0));
        let pos = screen.center() - egui::vec2(width / 2.0, 210.0);
        let mut close = false;
        egui::Window::new(lapinux_core::i18n::t("downloads.servers_help_title"))
            .id(egui::Id::new("download-servers-help"))
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .default_width(width)
            .default_pos(pos)
            .frame(
                egui::Frame::window(&ctx.style())
                    .fill(PANEL())
                    .stroke(egui::Stroke::new(1.0_f32, BORDER()))
                    .rounding(egui::Rounding::same(12.0))
                    .inner_margin(16.0),
            )
            .show(ctx, |ui| {
                ui.set_width(width - 32.0);
                close = servers_help_body(ui);
            });
        if close || ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.downloads.servers_help_open = false;
        }
    }

    fn pause_all(&mut self, jobs: &[Job], paused: bool) {
        let mut failure = None;
        for job in jobs.iter().filter(|job| {
            job.kind == JobKind::InstallGame
                && matches!(
                    job.state,
                    JobState::Running | JobState::Paused | JobState::Interrupted
                )
        }) {
            // Une tâche interrompue par un redémarrage n'a pas de drapeau de
            // pause : elle reprend comme la carte le fait, avec ses DLC.
            if !paused && job.state == JobState::Interrupted {
                self.resume_job(job);
                continue;
            }
            if let Err(err) = self.client.request(&Req::PauseJob {
                id: job.id.clone(),
                paused,
            }) {
                failure = Some(err);
                break;
            }
        }
        if let Some(err) = failure {
            self.notify(lapinux_core::i18n::t_args(
                "downloads.pause_error",
                &[("error", &err.to_string())],
            ));
        } else {
            self.refresh_jobs_now();
        }
    }

    /// Catégorie « Terminées » : transferts aboutis ou échoués, avec la même
    /// carte large que les autres catégories. La poubelle de l'en-tête efface
    /// tout l'historique ; chaque carte peut aussi supprimer son transfert.
    fn history_section(&mut self, ui: &mut egui::Ui, history: &[Job]) {
        if history.is_empty() {
            return;
        }
        let open = self.downloads.history_open;
        let header = Self::section_heading(
            ui,
            lapinux_core::i18n::t("downloads.finished"),
            &history.len().to_string(),
            Some((
                HeroGlyph::Trash,
                lapinux_core::i18n::t("downloads.clear_history"),
            )),
            open,
        );
        if header.action {
            // La suppression doit survivre au redémarrage : c'est le daemon
            // qui retire les tâches terminées, pas une simple liste masquée.
            match self.client.request(&Req::ClearHistory) {
                Ok(envelope) if envelope.ok => {
                    self.refresh_jobs_now();
                    self.notify(lapinux_core::i18n::t("downloads.history_cleared"));
                }
                Ok(envelope) => self.notify(envelope.error.unwrap_or_default()),
                Err(err) => self.notify(format!("{err}")),
            }
            return;
        }
        if header.toggle {
            self.downloads.history_open = !open;
            self.remember_download_sections();
        }
        if !open {
            self.collapsed_icons(ui, header.free, &job_icons(history));
            return;
        }
        ui.add_space(4.0);
        // Chaque carte large charge une bannière floutée : on n'en dessine
        // que dix à la fois, le reste s'ouvre sur demande.
        const VISIBLE: usize = 10;
        let show_all = self.downloads.history_show_all || history.len() <= VISIBLE;
        let shown = if show_all {
            history
        } else {
            &history[..VISIBLE]
        };
        let action = self.hero_tiles(ui, shown);
        self.apply_job_action(action);
        if history.len() > VISIBLE {
            ui.add_space(6.0);
            let label = if show_all {
                lapinux_core::i18n::t("downloads.collapse_list").to_string()
            } else {
                lapinux_core::i18n::t_args(
                    "downloads.show_all",
                    &[("count", &history.len().to_string())],
                )
            };
            if link_text(ui, &label).clicked() {
                self.downloads.history_show_all = !self.downloads.history_show_all;
            }
        }
        ui.add_space(14.0);
    }

    /// Catégorie repliée : les vignettes des jeux concernés, alignées à
    /// droite sur la ligne de l'en-tête, juste avant le chevron. Au-delà de
    /// la place disponible, un « +N » résume le reste.
    fn collapsed_icons(&mut self, ui: &mut egui::Ui, free: egui::Rect, entries: &[(u32, String)]) {
        if entries.is_empty() || free.width() < 24.0 {
            return;
        }
        const SIZE: f32 = 20.0;
        const GAP: f32 = 5.0;
        let (shown, more, offset) = collapsed_strip(free.width(), entries.len());
        let painter = ui.painter().clone();
        let mut x = free.left() + offset;
        let top = free.center().y - SIZE / 2.0;
        for (appid, name) in entries.iter().take(shown) {
            let icon_rect = egui::Rect::from_min_size(egui::pos2(x, top), egui::vec2(SIZE, SIZE));
            painter.rect_filled(
                icon_rect,
                egui::Rounding::same(0.0),
                egui::Color32::from_black_alpha(70),
            );
            match self.downloads.art.icon(ui.ctx(), &self.paths, *appid) {
                Some(icon) => {
                    painter.image(
                        icon.id(),
                        icon_rect,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                }
                None => {
                    let initial = name
                        .chars()
                        .find(|character| character.is_alphanumeric())
                        .map(|character| character.to_uppercase().to_string())
                        .unwrap_or_else(|| "?".into());
                    painter.text(
                        icon_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        initial,
                        display_font(11.0),
                        TEXT_DIM(),
                    );
                }
            }
            x += SIZE + GAP;
        }
        if more > 0 {
            painter.text(
                egui::pos2(x + 1.0, free.center().y),
                egui::Align2::LEFT_CENTER,
                format!("+{more}"),
                egui::FontId::proportional(10.5),
                TEXT_DIM(),
            );
        }
    }

    /// Une carte large par téléchargement actif, à la Epic : illustration,
    /// titre, barre de progression, volumes et commandes.
    fn hero_tiles(&mut self, ui: &mut egui::Ui, jobs: &[Job]) -> JobTileAction {
        let width = ui.available_width().max(320.0);
        let mut action = JobTileAction::default();
        {
            let art = &mut self.downloads.art;
            let paths = &self.paths;
            let rates = &self.downloads.rates;
            let eta_rates = &self.downloads.eta_rates;
            for (index, job) in jobs.iter().enumerate() {
                if index > 0 {
                    ui.add_space(10.0);
                }
                let title = if job.title.trim().is_empty() {
                    sentence(&job.label)
                } else {
                    job.title.clone()
                };
                let (cover, icon) = match job.appid {
                    Some(appid) => (
                        art.cover(ui.ctx(), paths, appid),
                        art.icon(ui.ctx(), paths, appid),
                    ),
                    None => (None, None),
                };
                let net = rates.get(&job.id).map(|(net, _)| *net).unwrap_or(0.0);
                let eta = eta_rates.get(&job.id).copied().unwrap_or(net);
                let source = if job.kind == JobKind::InstallGame {
                    Some(crate::store_mark_texture(ui.ctx(), job_store(job), 48))
                } else {
                    None
                };
                action.merge(download_hero_tile(
                    ui,
                    width,
                    job,
                    &title,
                    cover.as_ref(),
                    icon.as_ref(),
                    source.as_ref(),
                    net,
                    eta,
                ));
            }
        }
        action
    }

    fn apply_job_action(&mut self, action: JobTileAction) {
        // Les champs sont consommés un par un : on note le geste avant.
        let touched = !action.is_empty();
        // Une suppression retire déjà la carte localement : redemander la
        // liste tout de suite ferait revenir la tâche avant que le daemon
        // n'ait enregistré sa disparition.
        let deleting = action.delete.is_some();
        if let Some((id, paused)) = action.pause {
            match self.client.request(&Req::PauseJob { id, paused }) {
                Ok(env) if env.ok => self.notify(if paused {
                    lapinux_core::i18n::t("downloads.notify_paused")
                } else {
                    lapinux_core::i18n::t("downloads.notify_resumed")
                }),
                Ok(env) => self.notify(
                    env.error
                        .unwrap_or_else(|| lapinux_core::i18n::t("downloads.pause_failed").into()),
                ),
                Err(err) => self.notify(lapinux_core::i18n::t_args(
                    "downloads.pause_error",
                    &[("error", &err.to_string())],
                )),
            }
        }
        if let Some(id) = action.cancel {
            // Une mise à jour garde le jeu installé ; une installation neuve
            // voit son dossier partiel supprimé. Le message le dit, plutôt
            // que de promettre une reprise qui n'existe plus.
            let updating = self
                .jobs
                .iter()
                .find(|job| job.id == id)
                .map_or(false, |job| job.updating);
            match self.client.request(&Req::CancelJob { id }) {
                Ok(env) if env.ok => self.notify(if updating {
                    lapinux_core::i18n::t("downloads.cancel_kept")
                } else {
                    lapinux_core::i18n::t("downloads.cancel_removed")
                }),
                Ok(env) => self
                    .notify(env.error.unwrap_or_else(|| {
                        lapinux_core::i18n::t("downloads.cancel_failed").into()
                    })),
                Err(err) => self.notify(lapinux_core::i18n::t_args(
                    "downloads.cancel_error",
                    &[("error", &err.to_string())],
                )),
            }
        }
        if let Some(appid) = action.resume {
            // La reprise garde les DLC et la version de la tâche arrêtée :
            // elle ne doit pas relancer une installation nue.
            let interrupted = self
                .jobs
                .iter()
                .rev()
                .find(|job| {
                    job.appid == Some(appid)
                        && matches!(
                            job.state,
                            JobState::Interrupted | JobState::Failed | JobState::Cancelled
                        )
                })
                .cloned();
            match interrupted {
                Some(job) => self.resume_job(&job),
                None => self.install_game(appid, Vec::new()),
            }
        }
        if let Some(id) = action.delete {
            self.delete_download(id);
        }
        if let Some(id) = action.report {
            self.downloads.report_open = Some(id);
        }
        if let Some(id) = action.info {
            self.downloads.info_open = Some(id);
        }
        if let Some((source, target, after)) = action.reorder {
            self.reorder_job(&source, &target, after);
        }
        if touched && !deleting {
            self.refresh_jobs_now();
        }
    }

    /// Déplace une tâche avant ou après une autre. L'ordre local change tout
    /// de suite ; le daemon reçoit la liste complète pour que la file suive le
    /// même ordre et qu'un redémarrage le conserve.
    fn reorder_job(&mut self, source: &str, target: &str, after: bool) {
        let Some(source_position) = self.jobs.iter().position(|job| job.id == source) else {
            return;
        };
        let job = self.jobs.remove(source_position);
        let Some(mut target_position) = self.jobs.iter().position(|job| job.id == target) else {
            self.jobs.insert(source_position, job);
            return;
        };
        if after {
            target_position += 1;
        }
        self.jobs.insert(target_position, job);
        let ids: Vec<String> = self.jobs.iter().map(|job| job.id.clone()).collect();
        match self.client.request(&Req::ReorderJobs { ids }) {
            Ok(envelope) if envelope.ok => {}
            Ok(envelope) => {
                self.notify(envelope.error.unwrap_or_default());
                self.refresh_jobs_now();
            }
            Err(err) => {
                self.notify(format!("{err}"));
                self.refresh_jobs_now();
            }
        }
    }

    /// Relit les tâches dès qu'une action est envoyée : sinon la carte garde
    /// « En pause » (ou « En cours ») jusqu'au cycle de trois secondes et le
    /// libellé contredit le bouton qu'on vient de cliquer.
    fn refresh_jobs_now(&mut self) {
        if let Some(jobs) = ui_request::<Vec<Job>>(&self.client, &Req::Jobs) {
            self.jobs = jobs;
        }
        self.jobs_poll = Instant::now() - Duration::from_secs(1);
    }

    /// Mises à jour des jeux qui ne sont pas déjà en téléchargement : leur
    /// carte vit dans « En cours », jamais aux deux endroits.
    fn updates_section(&mut self, ui: &mut egui::Ui, sections: &JobSections) {
        self.downloads.poll_store_updates();
        // Les mises à jour de boutique se lisent à la demande : le bouton
        // reste visible même quand la liste est vide, sinon on ne saurait
        // jamais qu'une vérification est possible.
        let mut check = false;
        let checking = self.downloads.store_updates_rx.is_some();
        let mut all: Vec<UpdateRow> = self.downloads.updates.clone();
        all.extend(self.downloads.store_updates.iter().cloned());
        let rows = sorted_updates(all, &sections.active_appids);
        if rows.is_empty() && !checking {
            ui.horizontal(|ui| {
                if link_text(ui, lapinux_core::i18n::t("downloads.check_updates")).clicked() {
                    check = true;
                }
                if let Some(at) = self.downloads.store_updates_at {
                    muted(
                        ui,
                        &lapinux_core::i18n::t_args(
                            "downloads.verified",
                            &[(
                                "when",
                                &lapinux_core::util::format_duration(at.elapsed().as_secs()),
                            )],
                        ),
                    );
                }
            });
            if check {
                self.check_store_updates(ui.ctx());
            }
            ui.add_space(18.0);
            return;
        }
        if checking {
            ui.horizontal(|ui| {
                crate::section_icons::loading(ui, 12.0);
                muted(ui, lapinux_core::i18n::t("downloads.reading_builds"));
            });
            ui.add_space(6.0);
        }
        const VISIBLE: usize = 6;
        let show_all = self.downloads.updates_show_all || rows.len() <= VISIBLE;
        let displayed = if show_all { rows.len() } else { VISIBLE };
        let total: u64 = rows.iter().filter_map(|row| row.size).sum();
        let open = self.downloads.updates_open;
        let detail = if total > 0 {
            format!("{} · {}", rows.len(), wording::update_download_label(total))
        } else {
            rows.len().to_string()
        };
        let header = Self::section_heading(
            ui,
            lapinux_core::i18n::t("downloads.updates"),
            &detail,
            None,
            open,
        );
        if header.toggle {
            self.downloads.updates_open = !open;
            self.remember_download_sections();
        }
        if !open {
            let icons: Vec<(u32, String)> = rows
                .iter()
                .map(|row| (row.appid, row.name.clone()))
                .collect();
            self.collapsed_icons(ui, header.free, &icons);
            return;
        }
        ui.add_space(2.0);
        muted(ui, lapinux_core::i18n::t("downloads.updates_note"));
        ui.add_space(8.0);

        // Une mise à jour occupe la même largeur qu'un téléchargement actif :
        // une seule carte par ligne, jamais une vignette étroite à côté.
        let width = ui.available_width();
        let mut start: Option<UpdateRow> = None;
        {
            let art = &mut self.art;
            let icons = &mut self.downloads.art;
            let paths = &self.paths;
            for row in rows.iter().take(displayed) {
                if let Some(details) = &row.details {
                    let _ = art.get(
                        ui.ctx(),
                        paths,
                        row.appid,
                        details,
                        SteamArt::Icon,
                        &row.name,
                    );
                }
                let icon = icons.icon(ui.ctx(), paths, row.appid);
                if available_update_tile(ui, width, row.appid, &row.name, icon.as_ref(), row.size) {
                    start = Some(row.clone());
                }
                ui.add_space(TILE_GAP);
            }
        }
        if rows.len() > VISIBLE {
            ui.add_space(6.0);
            let label = if show_all {
                lapinux_core::i18n::t("downloads.collapse_list").to_string()
            } else {
                lapinux_core::i18n::t_fmt("downloads.show_all_count", &[rows.len().to_string()])
            };
            if link_text(ui, &label).clicked() {
                self.downloads.updates_show_all = !self.downloads.updates_show_all;
            }
        }
        ui.add_space(18.0);
        if let Some(row) = start {
            match row.store {
                Some(store) => {
                    // Une mise à jour de boutique se réinstalle par-dessus le
                    // dossier existant : elle apparaît dans la liste comme
                    // n'importe quel téléchargement.
                    match self.client.request(&Req::StoreInstall {
                        store,
                        store_id: row.store_id.clone(),
                        platform: row.platform.clone(),
                    }) {
                        Ok(envelope) if envelope.ok => {}
                        Ok(envelope) => {
                            self.notify(envelope.error.unwrap_or_default());
                        }
                        Err(error) => self.notify(format!("{error}")),
                    }
                }
                None => self.install_game(row.appid, Vec::new()),
            }
        }
    }

    /// Demande aux boutiques les builds publiés des jeux installés. La
    /// lecture coûte un appel par jeu : elle tourne hors de l'interface, et
    /// son résultat remplace celui de la vérification précédente.
    fn check_store_updates(&mut self, ctx: &egui::Context) {
        if self.downloads.store_updates_rx.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.downloads.store_updates_rx = Some(rx);
        self.downloads.store_updates_at = Some(Instant::now());
        let client = self.client.detached();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let mut rows = Vec::new();
            let reply = client.request_with_timeout(&Req::StoreUpdates, Duration::from_secs(300));
            if let Ok(envelope) = reply.and_then(|envelope| {
                if envelope.ok {
                    Ok(envelope)
                } else {
                    Err(lapinux_core::Error::Other(
                        envelope.error.unwrap_or_default(),
                    ))
                }
            }) {
                for item in envelope.data.as_array().into_iter().flatten() {
                    let Some(appid) = item.get("appid").and_then(|value| value.as_u64()) else {
                        continue;
                    };
                    let store = item.get("store").and_then(|value| {
                        serde_json::from_value::<lapinux_core::stores::Store>(value.clone()).ok()
                    });
                    rows.push(UpdateRow {
                        appid: appid as u32,
                        name: item
                            .get("name")
                            .and_then(|value| value.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        size: item.get("size").and_then(|value| value.as_u64()),
                        details: None,
                        store,
                        store_id: item
                            .get("store_id")
                            .and_then(|value| value.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        platform: item
                            .get("platform")
                            .and_then(|value| value.as_str())
                            .map(str::to_string),
                    });
                }
            }
            let _ = tx.send(rows);
            ctx.request_repaint();
        });
    }

    /// Carte du dossier : place réelle, part réservée par les téléchargements
    /// et changement de destination. Tout est peint dans un rectangle connu,
    /// avec de vrais boutons posés dessus : la carte garde exactement la
    /// largeur demandée et ne pousse jamais le test de débit hors écran.
    fn download_folder_card(&mut self, ui: &mut egui::Ui, width: f32) {
        let root = self.config.library_root();
        let disk = self.disk;
        let reserved = self
            .jobs
            .iter()
            .filter(|job| {
                matches!(
                    job.state,
                    JobState::Running | JobState::Queued | JobState::Paused
                )
            })
            .map(|job| job.total.saturating_sub(job.downloaded))
            .sum::<u64>();
        // Hauteur constante : passer en édition ne doit pas faire remonter la
        // carte des limites située juste en dessous.
        let height = 118.0;
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
        let painter = ui.painter().clone();
        painter.rect(
            rect,
            egui::Rounding::same(12.0),
            CARD(),
            egui::Stroke::new(1.0_f32, BORDER()),
        );
        let inner = rect.shrink(14.0);
        let mut start_edit = false;
        let mut cancel = false;
        let mut save = false;
        let mut open_folder = false;

        if let Some(text) = self.downloads.dir_draft.as_mut() {
            let top = inner.top() + ((inner.height() - 60.0) * 0.5).max(0.0);
            let field = egui::Rect::from_min_size(
                egui::pos2(inner.left(), top),
                egui::vec2(inner.width(), 24.0),
            );
            ui.put(
                field,
                egui::TextEdit::singleline(text).hint_text("~/Jeux/LapinuxLibrary"),
            );
            let buttons_y = field.bottom() + 10.0;
            let save_rect = egui::Rect::from_min_size(
                egui::pos2(inner.left(), buttons_y),
                egui::vec2(104.0, 26.0),
            );
            if tile_button(
                ui,
                save_rect,
                lapinux_core::i18n::t("downloads.save"),
                false,
            ) {
                save = true;
            }
            let cancel_rect = egui::Rect::from_min_size(
                egui::pos2(save_rect.right() + 8.0, buttons_y),
                egui::vec2(84.0, 26.0),
            );
            if tile_button(
                ui,
                cancel_rect,
                lapinux_core::i18n::t("downloads.cancel"),
                false,
            ) {
                cancel = true;
            }
            // La colonne de droite est étroite : le rappel ne s'affiche que
            // s'il reste la place à côté des deux boutons.
            if cancel_rect.right() + 12.0 + 150.0 < inner.right() {
                painter.text(
                    egui::pos2(cancel_rect.right() + 12.0, buttons_y + 13.0),
                    egui::Align2::LEFT_CENTER,
                    lapinux_core::i18n::t("downloads.created_on_download"),
                    egui::FontId::proportional(10.0),
                    TEXT_DIM(),
                );
            }
        } else {
            let path = root.display().to_string();
            // Le bloc d'informations est centré verticalement dans la carte :
            // une hauteur commune laisse sinon un vide sous la dernière ligne.
            const CONTENT_HEIGHT: f32 = 94.0;
            let shift = ((inner.height() - CONTENT_HEIGHT) * 0.5).max(0.0);
            // Deux icônes collées plutôt que deux boutons empilés : le chemin
            // gagne toute la largeur et la carte reste basse.
            const ICON_SIZE: f32 = 28.0;
            const ICON_GAP: f32 = 5.0;
            let icons_y = inner.top() + shift - 2.0;
            let open = egui::Rect::from_min_size(
                egui::pos2(inner.right() - ICON_SIZE, icons_y),
                egui::vec2(ICON_SIZE, ICON_SIZE),
            );
            let edit = egui::Rect::from_min_size(
                egui::pos2(open.left() - ICON_SIZE - ICON_GAP, icons_y),
                egui::vec2(ICON_SIZE, ICON_SIZE),
            );
            if folder_icon_button(ui, edit, FolderAction::Edit) {
                start_edit = true;
            }
            if folder_icon_button(ui, open, FolderAction::Open) {
                open_folder = true;
            }
            painter.text(
                egui::pos2(inner.left(), inner.top() + 9.0 + shift),
                egui::Align2::LEFT_CENTER,
                lapinux_core::i18n::t("downloads.folder"),
                egui::FontId::proportional(11.0),
                TEXT_DIM(),
            );
            let path_x = inner.left() + 52.0;
            let path_width = (edit.left() - 10.0 - path_x).max(40.0);
            painter.text(
                egui::pos2(path_x, inner.top() + 9.0 + shift),
                egui::Align2::LEFT_CENTER,
                lapinux_core::util::ellipsize(&path, (path_width / 5.6).max(6.0) as usize),
                egui::FontId::proportional(11.0),
                TEXT_DIM(),
            );

            let info_y = inner.top() + 38.0 + shift;
            match disk {
                Some(disk) => {
                    let used = disk.used_percent();
                    let color = if used > 90.0 {
                        DANGER
                    } else if used > 75.0 {
                        WARN
                    } else {
                        OK
                    };
                    painter.text(
                        egui::pos2(inner.left(), info_y),
                        egui::Align2::LEFT_CENTER,
                        lapinux_core::i18n::t_args(
                            "downloads.used_percent",
                            &[("percent", &format!("{used:.0}"))],
                        ),
                        egui::FontId::proportional(12.5),
                        color,
                    );
                    let label_x = inner.left() + 88.0;
                    let label_width = (inner.right() - label_x).max(60.0);
                    let free = lapinux_core::i18n::t_args(
                        "downloads.free_of",
                        &[
                            ("free", &lapinux_core::util::format_bytes(disk.free)),
                            ("total", &lapinux_core::util::format_bytes(disk.total)),
                        ],
                    );
                    painter.text(
                        egui::pos2(label_x, info_y),
                        egui::Align2::LEFT_CENTER,
                        lapinux_core::util::ellipsize(&free, (label_width / 5.6).max(6.0) as usize),
                        egui::FontId::proportional(12.0),
                        TEXT(),
                    );

                    let bar = egui::Rect::from_min_size(
                        egui::pos2(inner.left(), info_y + 20.0),
                        egui::vec2(inner.width(), 16.0),
                    );
                    // Barre peinte à la main : le segment hachuré démarre
                    // exactement où finit le pourcentage utilisé (la
                    // ProgressBar animait son remplissage, d'où un décalage).
                    let radius = 8.0;
                    painter.rect_filled(
                        bar,
                        egui::Rounding::same(radius),
                        egui::Color32::from_black_alpha(90),
                    );
                    let used_ratio = (used / 100.0).clamp(0.0, 1.0);
                    let used_width = bar.width() * used_ratio;
                    if used_width > 0.5 {
                        let used_rect = egui::Rect::from_min_max(
                            bar.min,
                            egui::pos2(bar.left() + used_width, bar.bottom()),
                        );
                        let right_round = if used_ratio >= 0.999 { radius } else { 0.0 };
                        painter.rect_filled(
                            used_rect,
                            egui::Rounding {
                                nw: radius,
                                sw: radius,
                                ne: right_round,
                                se: right_round,
                            },
                            color,
                        );
                    }
                    if reserved > 0 && disk.total > 0 {
                        let reserved_ratio = (reserved as f64 / disk.total as f64) as f32;
                        let start = bar.left() + used_width;
                        let end = (start + bar.width() * reserved_ratio).min(bar.right());
                        let segment = egui::Rect::from_min_max(
                            egui::pos2(start, bar.top()),
                            egui::pos2(end, bar.bottom()),
                        );
                        let tint = if used_ratio + reserved_ratio > 1.0 {
                            DANGER
                        } else {
                            ACCENT()
                        };
                        let hatch = painter.with_clip_rect(segment);
                        hatch.rect_filled(segment, 0.0, with_alpha(tint, 0.22));
                        let step = 7.0;
                        let mut x = start - bar.height();
                        while x < end {
                            hatch.line_segment(
                                [
                                    egui::pos2(x, bar.bottom()),
                                    egui::pos2(x + bar.height(), bar.top()),
                                ],
                                egui::Stroke::new(1.0_f32, with_alpha(tint, 0.55)),
                            );
                            x += step;
                        }
                    }
                    painter.rect_stroke(
                        bar,
                        egui::Rounding::same(radius),
                        egui::Stroke::new(1.0_f32, with_alpha(BORDER(), 0.9)),
                    );
                    if reserved > 0 {
                        painter.text(
                            egui::pos2(inner.left(), bar.bottom() + 12.0),
                            egui::Align2::LEFT_CENTER,
                            lapinux_core::i18n::t_args(
                                "downloads.reserved",
                                &[("size", &lapinux_core::util::format_bytes(reserved))],
                            ),
                            egui::FontId::proportional(10.0),
                            TEXT_DIM(),
                        );
                    }
                }
                None => {
                    painter.text(
                        egui::pos2(inner.left(), info_y),
                        egui::Align2::LEFT_CENTER,
                        lapinux_core::i18n::t("downloads.disk_unavailable"),
                        egui::FontId::proportional(12.0),
                        TEXT_DIM(),
                    );
                }
            }
        }

        // `ui.put` place le curseur juste sous chaque champ, donc au-dessus du
        // bas de la carte : sans ce rappel, la carte des limites remontait
        // sur celle-ci dès qu'on ouvrait le renommage du dossier.
        ui.advance_cursor_after_rect(rect);

        let mut tooltip = format!("{}", root.display());
        if let Some(disk) = disk {
            tooltip.push_str(&lapinux_core::i18n::t_args(
                "downloads.disk_usage",
                &[
                    ("used", &lapinux_core::util::format_bytes(disk.used())),
                    ("total", &lapinux_core::util::format_bytes(disk.total)),
                    ("free", &lapinux_core::util::format_bytes(disk.free)),
                ],
            ));
        }
        if reserved > 0 {
            tooltip.push_str(&lapinux_core::i18n::t_args(
                "downloads.disk_reserved",
                &[("size", &lapinux_core::util::format_bytes(reserved))],
            ));
        }
        let _ = response.on_hover_text(tooltip);

        if start_edit {
            self.downloads.dir_draft = Some(root.display().to_string());
        }
        if open_folder {
            self.open_download_folder();
        }
        if cancel {
            self.downloads.dir_draft = None;
        }
        if save {
            if let Some(text) = self.downloads.dir_draft.take() {
                self.set_download_dir(&text);
            }
        }
    }

    /// Carte autonome du test de débit, placée à gauche du dossier.
    /// Carte des plafonds de débit : un global pour tous les stores, puis un
    /// plafond par store. Un store inconnu apparaît dès qu'il a une entrée de
    /// configuration, sans toucher à cette carte.
    fn download_limits_card(&mut self, ui: &mut egui::Ui) {
        // Le globe du plafond global, le logo du store : deux lignes sans
        // libellé, le nom restant dans l'infobulle.
        #[derive(Clone)]
        enum LimitIcon<'a> {
            Globe(&'a str),
            Store(egui::TextureHandle, String),
        }

        fn limit_icon(ui: &mut egui::Ui, icon: LimitIcon) {
            let size = LIMIT_ROW_HEIGHT;
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
            let painter = ui.painter();
            match icon {
                LimitIcon::Globe(label) => {
                    paint_globe(painter, rect.shrink(3.0), TEXT_DIM());
                    let _ = response.on_hover_text(label);
                }
                LimitIcon::Store(texture, label) => {
                    let source = texture.size_vec2();
                    let ratio = if source.y > 0.0 {
                        source.x / source.y
                    } else {
                        1.0
                    };
                    let inner = rect.shrink(2.0);
                    let (width, height) = if ratio >= inner.width() / inner.height().max(1.0) {
                        (inner.width(), inner.width() / ratio)
                    } else {
                        (inner.height() * ratio, inner.height())
                    };
                    let target =
                        egui::Rect::from_center_size(inner.center(), egui::vec2(width, height));
                    painter.image(
                        texture.id(),
                        target,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                    let _ = response.on_hover_text(label);
                }
            }
        }

        fn paint_globe(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
            let center = rect.center();
            let radius = rect.width().min(rect.height()) * 0.5;
            let stroke = egui::Stroke::new(1.2_f32, color);
            painter.circle_stroke(center, radius, stroke);
            painter.line_segment(
                [
                    egui::pos2(center.x - radius, center.y),
                    egui::pos2(center.x + radius, center.y),
                ],
                stroke,
            );
            let mut meridian = Vec::with_capacity(25);
            for index in 0..=24 {
                let angle = std::f32::consts::TAU * index as f32 / 24.0;
                meridian.push(egui::pos2(
                    center.x + radius * 0.42 * angle.sin(),
                    center.y - radius * angle.cos(),
                ));
            }
            painter.add(egui::Shape::line(meridian, stroke));
        }
        let mut draft = self
            .downloads
            .limits
            .clone()
            .unwrap_or_else(|| self.config.downloads.clone());
        let marks: Vec<(String, egui::TextureHandle)> = {
            let mut list: Vec<(String, egui::TextureHandle)> = Vec::new();
            for store in lapinux_core::stores::Store::ALL {
                list.push((
                    store.key().to_string(),
                    crate::store_mark_texture(ui.ctx(), store, 48),
                ));
            }
            list
        };
        let mut changed = false;
        let mut apply_now = false;
        let mut stores_open = self.downloads.limits_stores_open;

        egui::Frame::none()
            .fill(CARD())
            .stroke(egui::Stroke::new(1.0_f32, BORDER()))
            .rounding(egui::Rounding::same(12.0))
            .inner_margin(14.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 3.0;
                ui.label(
                    egui::RichText::new(lapinux_core::i18n::t("downloads.limits"))
                        .size(12.5)
                        .strong(),
                );
                muted(ui, lapinux_core::i18n::t("downloads.limits_note"));
                ui.add_space(7.0);

                // Chaque limite tient sur une ligne : l'icône, la piste de
                // réglage à sa droite, puis la valeur en bout de ligne. La
                // ligne est haute comme l'icône, donc tout ce qu'elle contient
                // partage la même ligne de centre ; la piste elle-même est
                // peinte par `limit_slider`.
                let mut row = |ui: &mut egui::Ui, icon: LimitIcon, value: &mut u32| {
                    let spacing = ui.spacing().clone();
                    ui.spacing_mut().interact_size.y = LIMIT_ROW_HEIGHT;
                    ui.horizontal(|ui| {
                        limit_icon(ui, icon);
                        let text = if *value == 0 {
                            lapinux_core::i18n::t("downloads.unlimited").to_string()
                        } else {
                            lapinux_core::i18n::t_args(
                                "downloads.mbit",
                                &[("value", &value.to_string())],
                            )
                        };
                        let label_width = ui
                            .painter()
                            .layout_no_wrap(text.clone(), egui::FontId::proportional(10.5), TEXT())
                            .size()
                            .x;
                        let width = (ui.available_width() - label_width - 10.0).max(80.0);
                        let response = limit_slider(ui, value, LIMIT_MAX_MBIT, width);
                        ui.label(egui::RichText::new(text).size(10.5).color(TEXT()));
                        if response.changed() {
                            changed = true;
                        }
                        if response.drag_stopped() {
                            apply_now = true;
                        }
                    });
                    *ui.spacing_mut() = spacing;
                    ui.add_space(2.0);
                };
                row(
                    ui,
                    LimitIcon::Globe(lapinux_core::i18n::t("downloads.global")),
                    &mut draft.global_mbit_per_sec,
                );

                ui.add_space(5.0);

                // Tous les launchers gérés ont leur plafond, même sans entrée
                // enregistrée : Steam, Epic Games, GOG et itch.io.
                let mut stores: Vec<String> = draft.store_mbit_per_sec.keys().cloned().collect();
                for store in lapinux_core::stores::Store::ALL {
                    let key = store.key().to_string();
                    if !stores.iter().any(|seen| *seen == key) {
                        stores.push(key);
                    }
                }
                // Steam reste en tête, quel que soit l'ordre alphabétique :
                // c'est le launcher que la majorité des joueurs règle ici.
                stores.sort_by(|a, b| {
                    let rank = |key: &str| usize::from(!key.eq_ignore_ascii_case("steam"));
                    rank(a)
                        .cmp(&rank(b))
                        .then_with(|| download_store_label(a).cmp(&download_store_label(b)))
                });
                let header = Self::section_heading(
                    ui,
                    lapinux_core::i18n::t("downloads.per_launcher"),
                    &stores.len().to_string(),
                    None,
                    stores_open,
                );
                if header.toggle {
                    stores_open = !stores_open;
                }
                if stores_open {
                    ui.add_space(2.0);
                    for store in stores {
                        // Un store sans entrée s'affiche à 0 sans être ajouté
                        // au brouillon : sinon la carte serait toujours « en
                        // attente », même sans aucune modification.
                        let mut value = draft.store_mbit_per_sec.get(&store).copied().unwrap_or(0);
                        match marks.iter().find(|(key, _)| *key == store) {
                            Some((_, texture)) => row(
                                ui,
                                LimitIcon::Store(texture.clone(), download_store_label(&store)),
                                &mut value,
                            ),
                            None => {
                                let label = download_store_label(&store);
                                row(ui, LimitIcon::Globe(&label), &mut value)
                            }
                        }
                        if value != 0 || draft.store_mbit_per_sec.contains_key(&store) {
                            draft.store_mbit_per_sec.insert(store, value);
                        }
                    }
                }

                ui.add_space(4.0);
                ui.separator();
                ui.add_space(3.0);
                let mut parallel = draft.parallel_installs;
                if ui
                    .checkbox(
                        &mut parallel,
                        egui::RichText::new(lapinux_core::i18n::t("downloads.concurrent"))
                            .size(11.0),
                    )
                    .changed()
                {
                    draft.parallel_installs = parallel;
                    changed = true;
                }
                muted(
                    ui,
                    if parallel {
                        lapinux_core::i18n::t("downloads.concurrent_on")
                    } else {
                        lapinux_core::i18n::t("downloads.concurrent_off")
                    },
                );
            });

        let stores_changed = self.downloads.limits_stores_open != stores_open;
        self.downloads.limits_stores_open = stores_open;
        if stores_changed {
            self.remember_download_sections();
        }

        // On ne fige le brouillon que s'il diffère réellement de la
        // configuration. Au lancement, la configuration du daemon arrive
        // parfois après la première image : la mémoriser à 0 masquait ensuite
        // la vraie limite et la carte restait « en attente » pour toujours.
        let dirty = draft != self.config.downloads;
        if dirty {
            self.downloads.limits = Some(draft.clone());
        } else {
            self.downloads.limits = None;
        }
        if changed {
            self.downloads.limits_apply_at = Some(Instant::now());
        }
        if apply_now {
            self.downloads.limits_apply_at = None;
            if self.apply_download_limits(draft.clone()) {
                self.downloads.limits = None;
            }
        } else if let Some(at) = self.downloads.limits_apply_at {
            // Enregistrement automatique un peu après le dernier mouvement :
            // le clic sur la piste, qui ne déclenche pas `drag_stopped`, est
            // donc appliqué aussi.
            let quiet = Duration::from_millis(350);
            if at.elapsed() >= quiet {
                self.downloads.limits_apply_at = None;
                if self.apply_download_limits(draft.clone()) {
                    self.downloads.limits = None;
                }
            } else {
                ui.ctx().request_repaint_after(quiet - at.elapsed());
            }
        }
    }

    /// Carte « Serveurs » : le cache régional de Steam et la famille de CDN
    /// d'Epic. GOG et itch n'en publient qu'un : leur ligne le dit, sans
    /// choix inutile.
    fn download_servers_card(&mut self, ui: &mut egui::Ui) {
        self.downloads.poll_servers(ui.ctx());
        if self.downloads.servers.is_none() {
            self.downloads.load_servers(ui.ctx());
        }
        let mut draft = self
            .downloads
            .limits
            .clone()
            .unwrap_or_else(|| self.config.downloads.clone());
        let mut changed = false;

        egui::Frame::none()
            .fill(CARD())
            .stroke(egui::Stroke::new(1.0_f32, BORDER()))
            .rounding(egui::Rounding::same(12.0))
            .inner_margin(14.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 6.0;
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(lapinux_core::i18n::t("downloads.servers"))
                            .size(12.5)
                            .strong(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if help_button(
                            ui,
                            self.downloads.servers_help_open,
                            lapinux_core::i18n::t("downloads.servers_why"),
                        ) {
                            self.downloads.servers_help_open = !self.downloads.servers_help_open;
                        }
                    });
                });
                muted(ui, lapinux_core::i18n::t("downloads.servers_note"));
                ui.add_space(4.0);

                let label_width = 96.0;
                // Steam : son réseau de contenu, avec l'hôte affiché.
                ui.horizontal(|ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(label_width, 20.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            crate::paint_store_mark(
                                ui,
                                lapinux_core::stores::Store::Steam,
                                16.0,
                                TEXT_DIM(),
                            );
                            ui.add_space(6.0);
                            ui.label(egui::RichText::new("Steam").size(11.5).color(TEXT()));
                        },
                    );
                    let current = draft.steam_server.trim().to_string();
                    let text = if current.is_empty() {
                        lapinux_core::i18n::t("downloads.automatic").to_string()
                    } else {
                        current.clone()
                    };
                    let combo = egui::ComboBox::from_id_salt("steam-cdn-server")
                        .selected_text(egui::RichText::new(text).size(11.0).color(TEXT()))
                        .width(230.0)
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_label(
                                    current.is_empty(),
                                    lapinux_core::i18n::t("downloads.automatic"),
                                )
                                .clicked()
                            {
                                draft.steam_server.clear();
                                changed = true;
                            }
                            for server in self.downloads.servers.iter().flatten() {
                                let selected = current == server.id;
                                if ui
                                    .selectable_label(
                                        selected,
                                        egui::RichText::new(&server.label).size(11.0),
                                    )
                                    .on_hover_text(&server.detail)
                                    .clicked()
                                {
                                    draft.steam_server = server.id.clone();
                                    changed = true;
                                }
                            }
                        });
                    // Le serveur retenu garde ses détails dans une bulle : le
                    // nom seul ne dit ni la région ni la charge du cache.
                    let detail = if current.is_empty() {
                        lapinux_core::i18n::t("downloads.auto_steam").to_string()
                    } else {
                        self.downloads
                            .servers
                            .iter()
                            .flatten()
                            .find(|server| server.id == current)
                            .map(|server| format!("{} · {}", server.label, server.detail))
                            .unwrap_or_default()
                    };
                    if !detail.is_empty() {
                        combo.response.on_hover_text(detail);
                    }
                    ui.add_space(6.0);
                    match self.downloads.servers.as_ref() {
                        None => muted(ui, lapinux_core::i18n::t("downloads.reading_dir")),
                        Some(list) if list.is_empty() => {
                            muted(ui, lapinux_core::i18n::t("downloads.dir_offline"))
                        }
                        Some(list) => muted(
                            ui,
                            &lapinux_core::i18n::t_args(
                                "downloads.cache_count",
                                &[("count", &list.len().to_string())],
                            ),
                        ),
                    }
                });

                // Epic : le même build sur trois CDN.
                ui.horizontal(|ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(label_width, 20.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            crate::paint_store_mark(
                                ui,
                                lapinux_core::stores::Store::Epic,
                                16.0,
                                TEXT_DIM(),
                            );
                            ui.add_space(6.0);
                            ui.label(egui::RichText::new("Epic Games").size(11.5).color(TEXT()));
                        },
                    );
                    let current = draft.epic_cdn.trim().to_string();
                    let text = match current.as_str() {
                        "cloudfront" => "CloudFront".to_string(),
                        "akamai" => "Akamai".to_string(),
                        "fastly" => "Fastly".to_string(),
                        _ => lapinux_core::i18n::t("downloads.automatic").to_string(),
                    };
                    egui::ComboBox::from_id_salt("epic-cdn")
                        .selected_text(egui::RichText::new(text).size(11.0).color(TEXT()))
                        .width(230.0)
                        .show_ui(ui, |ui| {
                            for (id, label, detail) in [
                                (
                                    "",
                                    lapinux_core::i18n::t("downloads.automatic"),
                                    lapinux_core::i18n::t("downloads.cdn.epic_auto"),
                                ),
                                (
                                    "cloudfront",
                                    "CloudFront",
                                    lapinux_core::i18n::t("downloads.cdn.cloudfront"),
                                ),
                                (
                                    "akamai",
                                    "Akamai",
                                    lapinux_core::i18n::t("downloads.cdn.akamai"),
                                ),
                                (
                                    "fastly",
                                    "Fastly",
                                    lapinux_core::i18n::t("downloads.cdn.fastly"),
                                ),
                            ] {
                                if ui
                                    .selectable_label(
                                        current == id,
                                        egui::RichText::new(label).size(11.0),
                                    )
                                    .on_hover_text(detail)
                                    .clicked()
                                {
                                    draft.epic_cdn = id.to_string();
                                    changed = true;
                                }
                            }
                        });
                });

                // GOG et itch n'ont qu'un CDN : la ligne l'annonce plutôt
                // que d'offrir un choix qui ne changerait rien.
                for store in [
                    lapinux_core::stores::Store::Gog,
                    lapinux_core::stores::Store::Itch,
                ] {
                    ui.horizontal(|ui| {
                        ui.allocate_ui_with_layout(
                            egui::vec2(label_width, 20.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                crate::paint_store_mark(ui, store, 16.0, TEXT_DIM());
                                ui.add_space(6.0);
                                ui.label(
                                    egui::RichText::new(store.label()).size(11.5).color(TEXT()),
                                );
                            },
                        );
                        muted(ui, lapinux_core::i18n::t("downloads.auto_single_cdn"));
                    });
                }
            });

        if draft != self.config.downloads {
            self.downloads.limits = Some(draft.clone());
        } else {
            self.downloads.limits = None;
        }
        if changed && self.apply_download_limits(draft) {
            self.downloads.limits = None;
        }
    }

    fn apply_download_limits(&mut self, limits: lapinux_core::config::DownloadsConfig) -> bool {
        let mut config = self.config.clone();
        config.downloads = limits.clone();
        match self.client.request(&Req::SetConfig {
            config: config.clone(),
        }) {
            Ok(envelope) if envelope.ok => {
                self.config = config;
                if let Some(draft) = self.settings_draft.as_mut() {
                    draft.downloads = limits.clone();
                }
                self.downloads.limits = None;
                // Aucune notification : la valeur est déjà visible dans la
                // carte, et le curseur en produit une à chaque mouvement.
                true
            }
            Ok(envelope) => {
                self.notify(envelope.error.unwrap_or_default());
                false
            }
            Err(err) => {
                self.notify(format!("{err}"));
                false
            }
        }
    }

    /// Enregistre l'état replié/déplié des catégories. Un clic sur un chevron
    /// ne doit pas se perdre au prochain redémarrage.
    fn remember_download_sections(&mut self) {
        let collapsed = self.downloads.collapsed_keys();
        if collapsed == self.config.downloads.collapsed_sections {
            return;
        }
        let mut config = self.config.clone();
        config.downloads.collapsed_sections = collapsed.clone();
        match self.client.request(&Req::SetConfig {
            config: config.clone(),
        }) {
            Ok(envelope) if envelope.ok => {
                self.config = config;
                if let Some(draft) = self.settings_draft.as_mut() {
                    draft.downloads.collapsed_sections = collapsed;
                }
            }
            Ok(envelope) => self.notify(envelope.error.unwrap_or_default()),
            Err(err) => self.notify(format!("{err}")),
        }
    }

    fn speedtest_card(&mut self, ui: &mut egui::Ui) {
        let mut run = false;
        let mut copy = false;
        let running = self.downloads.speedtest.running;
        let stage = self.downloads.speedtest.stage;
        let ping = self.downloads.speedtest.ping_ms;
        let down = self.downloads.speedtest.down;
        let up = self.downloads.speedtest.up;
        let down_range = match (
            self.downloads.speedtest.down_low,
            self.downloads.speedtest.down_high,
        ) {
            (Some(low), Some(high)) => Some((low, high)),
            _ => None,
        };
        let up_range = match (
            self.downloads.speedtest.up_low,
            self.downloads.speedtest.up_high,
        ) {
            (Some(low), Some(high)) => Some((low, high)),
            _ => None,
        };
        let elapsed = self.downloads.speedtest.elapsed;
        let note = self.downloads.speedtest.note.clone();
        let card = egui::Frame::none()
            .fill(CARD())
            .stroke(egui::Stroke::new(1.0_f32, BORDER()))
            .rounding(egui::Rounding::same(12.0))
            .inner_margin(12.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(lapinux_core::i18n::t("downloads.throughput"))
                                .size(12.5)
                                .strong(),
                        );
                        match elapsed {
                            Some(seconds) => muted(ui, format!("· {}", format_seconds(seconds))),
                            None if running => {
                                muted(ui, lapinux_core::i18n::t("downloads.running"))
                            }
                            None => {}
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            // Une mesure existe : l'icône copie le relevé
                            // complet, prêt à coller ailleurs.
                            if ping.is_some() || down.is_some() || up.is_some() {
                                let (icon_rect, _) = ui.allocate_exact_size(
                                    egui::vec2(20.0, 20.0),
                                    egui::Sense::hover(),
                                );
                                if hero_glyph_button(
                                    ui,
                                    icon_rect,
                                    "speedtest-copy",
                                    HeroGlyph::Copy,
                                    egui::Color32::from_black_alpha(80),
                                    TEXT(),
                                    lapinux_core::i18n::t("downloads.copy_speeds"),
                                ) {
                                    copy = true;
                                }
                            }
                            muted(ui, "Cloudflare");
                        });
                    });
                    ui.add_space(2.0);
                    speedtest_row(
                        ui,
                        "Ping",
                        ping.map(|ms| format!("{ms:.0}")),
                        "ms",
                        running && stage == SpeedtestStage::Ping,
                        None,
                    );
                    speedtest_row(
                        ui,
                        lapinux_core::i18n::t("downloads.down"),
                        format_mbit(down),
                        "Mb/s",
                        running && stage == SpeedtestStage::Download,
                        down_range,
                    );
                    speedtest_row(
                        ui,
                        lapinux_core::i18n::t("downloads.up"),
                        format_mbit(up),
                        "Mb/s",
                        running && stage == SpeedtestStage::Upload,
                        up_range,
                    );
                    ui.add_space(2.0);
                    if running {
                        ui.horizontal(|ui| {
                            crate::section_icons::loading(ui, 18.0);
                            ui.label(
                                egui::RichText::new(lapinux_core::i18n::t("downloads.testing"))
                                    .size(10.5)
                                    .color(TEXT_DIM()),
                            );
                        });
                    } else {
                        let label = if ping.is_some() || down.is_some() || up.is_some() {
                            lapinux_core::i18n::t("downloads.retest")
                        } else if note.is_some() {
                            lapinux_core::i18n::t("downloads.retry")
                        } else {
                            lapinux_core::i18n::t("downloads.start_test")
                        };
                        if link_text(ui, label).clicked() {
                            run = true;
                        }
                    }
                });
            });
        if let Some(note) = note {
            card.response.on_hover_text(note);
        }
        if copy {
            ui.ctx().copy_text(speedtest_summary(
                ping, down, down_range, up, up_range, elapsed,
            ));
        }
        if run {
            self.downloads.start_speedtest();
        }
    }

    /// Carte « en direct » affichée seulement pendant un téléchargement :
    /// débit réseau et écriture disque sur les dernières secondes, avec un
    /// graphique par courbe comme la fiche Epic.
    fn live_rates_card(&mut self, ui: &mut egui::Ui, width: f32, running: bool) {
        const HEIGHT: f32 = 150.0;
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(width, HEIGHT), egui::Sense::hover());
        let painter = ui.painter().clone();
        painter.rect(
            rect,
            egui::Rounding::same(12.0),
            CARD(),
            egui::Stroke::new(1.0_f32, BORDER()),
        );
        let inner = rect.shrink(14.0);
        let history: Vec<(f32, f32)> = self.downloads.rate_history.iter().copied().collect();
        // Fenêtre d'environ 30 secondes (un point par seconde) : au-delà, les
        // points se tassent et la courbe devient une bande bruyante illisible.
        let samples = if history.len() > 30 {
            &history[history.len() - 30..]
        } else {
            &history[..]
        };
        // Les libellés montrent le débit courant, pas le dernier point
        // historisé qui peut avoir une demi-seconde de retard.
        let (net, write) = self
            .downloads
            .rates
            .values()
            .fold((0.0_f32, 0.0_f32), |(net, write), (job_net, job_write)| {
                (net + job_net, write + job_write)
            });

        painter.text(
            egui::pos2(inner.left(), inner.top() + 6.0),
            egui::Align2::LEFT_CENTER,
            lapinux_core::i18n::t("downloads.speed"),
            display_font(12.5),
            TEXT(),
        );
        painter.circle_filled(
            egui::pos2(inner.right() - 6.0, inner.top() + 6.0),
            3.0,
            if running { RATE_NET } else { TEXT_DIM() },
        );
        painter.text(
            egui::pos2(inner.right() - 14.0, inner.top() + 6.0),
            egui::Align2::RIGHT_CENTER,
            if running {
                lapinux_core::i18n::t("downloads.avg_4s")
            } else {
                lapinux_core::i18n::t("downloads.paused_short")
            },
            egui::FontId::proportional(10.5),
            TEXT_DIM(),
        );

        let net_values: Vec<f32> = samples.iter().map(|(net, _)| *net).collect();
        let write_values: Vec<f32> = samples.iter().map(|(_, write)| *write).collect();
        paint_rate_row(
            &painter,
            inner,
            inner.top() + 26.0,
            lapinux_core::i18n::t("downloads.download_label"),
            net,
            &net_values,
            RATE_NET,
            HeroGlyph::Down,
        );
        paint_rate_row(
            &painter,
            inner,
            inner.top() + 72.0,
            lapinux_core::i18n::t("downloads.write_label"),
            write,
            &write_values,
            RATE_WRITE,
            HeroGlyph::Disk,
        );
        let _ = response.on_hover_text(lapinux_core::i18n::t("ui.downloads_rate_hint"));
    }

    /// Ouvre l'emplacement du dossier dans le gestionnaire de fichiers du
    /// bureau, sans bloquer l'interface.
    fn open_download_folder(&mut self) {
        let root = self.config.library_root();
        self.open_folder(&root);
    }

    fn set_download_dir(&mut self, input: &str) {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            self.notify(lapinux_core::i18n::t("downloads.empty_folder"));
            return;
        }
        let mut path = lapinux_core::paths::expand_tilde(trimmed);
        if !path.is_absolute() {
            path = lapinux_core::paths::home_dir().join(path);
        }
        let mut config = self.config.clone();
        config.general.library_root = path.display().to_string();
        match self.client.request(&Req::SetConfig {
            config: config.clone(),
        }) {
            Ok(envelope) if envelope.ok => {
                self.config = config;
                self.settings_draft = None;
                self.downloads.dir_draft = None;
                self.disk = lapinux_core::system::disk_usage(&self.config.library_root());
                self.disk_poll = Instant::now();
                self.notify(lapinux_core::i18n::t("downloads.folder_updated"));
            }
            Ok(envelope) => self.notify(envelope.error.unwrap_or_default()),
            Err(err) => self.notify(format!("{err}")),
        }
    }
}

/// Réseau de contenu Steam, tel que le client Steam le demande.
///
/// L'annuaire est public : la liste sert à proposer les caches régionaux dans
/// les réglages, le même hôte que celui que Steam choisirait.
fn steam_content_servers() -> Vec<lapinux_core::model::DownloadServer> {
    let url = "https://api.steampowered.com/IContentServerDirectoryService/GetServersForSteamPipe/v1/?cell_id=0&max_servers=40";
    let Ok(response) = ureq::get(url).timeout(Duration::from_secs(8)).call() else {
        return Vec::new();
    };
    let mut text = String::new();
    if std::io::Read::read_to_string(&mut response.into_reader(), &mut text).is_err() {
        return Vec::new();
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    let Some(servers) = value
        .pointer("/response/servers")
        .and_then(serde_json::Value::as_array)
    else {
        return Vec::new();
    };
    let mut out: Vec<lapinux_core::model::DownloadServer> = Vec::new();
    for server in servers {
        let Some(host) = server.get("host").and_then(serde_json::Value::as_str) else {
            continue;
        };
        if out.iter().any(|seen| seen.id == host) {
            continue;
        }
        let group = server
            .get("group")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let load = server
            .get("load")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        out.push(lapinux_core::model::DownloadServer {
            id: host.to_string(),
            label: host.to_string(),
            detail: if group.is_empty() {
                lapinux_core::i18n::t_args("downloads.load_percent", &[("load", &load.to_string())])
            } else {
                lapinux_core::i18n::t_args(
                    "downloads.group_load",
                    &[("group", &group), ("load", &load.to_string())],
                )
            },
        });
    }
    out
}

/// Nom lisible d'un store de téléchargement. Les clés inconnues sont
/// simplement capitalisées, pour qu'un store ajouté plus tard apparaisse sans
/// modifier cette carte.
fn download_store_label(store: &str) -> String {
    match store {
        "steam" => "Steam".to_string(),
        other => {
            let mut characters = other.chars();
            match characters.next() {
                Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
                None => "Store".to_string(),
            }
        }
    }
}

/// Espacement horizontal et vertical des grilles de cartes.
const TILE_GAP: f32 = 12.0;

/// Hauteur d'une ligne du réglage de débit : l'icône, la piste et la poignée
/// partagent cette hauteur, donc la même ligne de centre.
const LIMIT_ROW_HEIGHT: f32 = 22.0;

/// Plafond réglable d'un store, en Mbit/s. Le curseur du thème allait jusqu'à
/// 1 000 : au-delà, un plafond ne sert plus à rien.
const LIMIT_MAX_MBIT: u32 = 1000;

/// Piste de réglage dessinée à la main, pour la carte des limites.
///
/// Le curseur du thème peignait sa piste avec la couleur des widgets au repos —
/// la même que le fond de cette carte, d'où une barre invisible — et sa poignée
/// faisait la moitié de la hauteur allouée : un disque de 24 px qui débordait
/// sous les icônes. Ici la piste, la part réglée et la poignée sont peintes,
/// fines et centrées sur la ligne de l'icône. La poignée se déplace au clic
/// comme au glissement, et les flèches règlent la valeur quand la piste a le
/// focus.
fn limit_slider(ui: &mut egui::Ui, value: &mut u32, max: u32, width: f32) -> egui::Response {
    let max = max.max(1);
    let (rect, mut response) = ui.allocate_exact_size(
        egui::vec2(width.max(80.0), LIMIT_ROW_HEIGHT),
        egui::Sense::click_and_drag(),
    );
    if response.hovered() || response.dragged() {
        ui.output_mut(|out| out.cursor_icon = egui::CursorIcon::PointingHand);
    }
    // La piste occupe toute la largeur allouée, moins la moitié de la poignée
    // à chaque bout : la poignée reste entière une fois à zéro comme au maximum.
    const HANDLE: f32 = 5.5;
    let rail = egui::Rect::from_min_max(
        egui::pos2(rect.left() + HANDLE, rect.center().y - 2.5),
        egui::pos2(rect.right() - HANDLE, rect.center().y + 2.5),
    );
    if let Some(pointer) = response.interact_pointer_pos() {
        if response.dragged() || response.clicked() {
            let ratio = ((pointer.x - rail.left()) / rail.width().max(1.0)).clamp(0.0, 1.0);
            let next = (ratio * max as f32).round() as u32;
            if next != *value {
                *value = next;
                response.mark_changed();
            }
        }
    }
    if response.has_focus() {
        let step = ui.input(|input| {
            input.num_presses(egui::Key::ArrowRight) as i64
                - input.num_presses(egui::Key::ArrowLeft) as i64
        });
        if step != 0 {
            let next = (*value as i64 + step).clamp(0, max as i64) as u32;
            if next != *value {
                *value = next;
                response.mark_changed();
            }
        }
    }

    let ratio = (*value).min(max) as f32 / max as f32;
    let painter = ui.painter();
    let rounding = egui::Rounding::same(rail.height() / 2.0);
    painter.rect_filled(rail, rounding, BORDER());
    let center = egui::pos2(rail.left() + rail.width() * ratio, rail.center().y);
    let filled = egui::Rect::from_min_max(rail.min, egui::pos2(center.x, rail.max.y));
    if filled.width() > 0.5 {
        painter.rect_filled(filled, rounding, ACCENT());
    }
    painter.circle_filled(center, HANDLE, TEXT());
    response
}

/// Couleurs du graphique en direct : bleu pour le réseau, rose pour le disque.
const RATE_NET: egui::Color32 = egui::Color32::from_rgb(112, 160, 255);
const RATE_WRITE: egui::Color32 = egui::Color32::from_rgb(240, 120, 180);

/// Mise à jour disponible, pré-calculée hors du dessin.
#[derive(Clone)]
struct UpdateRow {
    appid: u32,
    name: String,
    size: Option<u64>,
    details: Option<SteamDetails>,
    /// Boutique qui publie la mise à jour ; `None` pour Steam.
    store: Option<lapinux_core::stores::Store>,
    store_id: String,
    platform: Option<String>,
}

/// Tâches regroupées par catégorie : ce qui tourne, les programmes, ce qui
/// est en pause, et l'historique. Un jeu n'apparaît que dans une seule
/// catégorie, avec sa tâche la plus récente.
#[derive(Default)]
struct JobSections {
    /// Jeux en cours ou en attente de démarrage.
    running: Vec<Job>,
    /// Installations de programmes (Proton, outils) encore actives.
    programs: Vec<Job>,
    /// Téléchargements de jeux suspendus.
    paused: Vec<Job>,
    /// Terminés ou échoués : même carte, avec « Reprendre » quand c'est
    /// possible et le rapport pour les transferts terminés. Une tâche annulée
    /// n'y figure jamais : elle n'a rien terminé.
    history: Vec<Job>,
    /// Jeux dont une tâche a déjà une carte visible.
    active_appids: HashSet<u32>,
}

/// Phrases de la page Téléchargements, regroupées ici pour que les cartes et
/// les fiches ne puissent plus se contredire : chaque libellé n'existe qu'une
/// fois.
pub(crate) mod wording {
    use lapinux_core::model::Job;
    use lapinux_core::util::format_bytes;

    /// Poids annoncé par Steam pour une mise à jour : ce qu'il reste à
    /// télécharger, jamais le volume déjà installé. La tuile, la pastille et
    /// la fiche du jeu partagent ce libellé.
    pub(crate) fn update_download_label(bytes: u64) -> String {
        lapinux_core::i18n::t_fmt("downloads.to_download", &[format_bytes(bytes).to_string()])
    }

    pub(crate) fn update_announcement(size: Option<u64>) -> String {
        match size {
            Some(bytes) => lapinux_core::i18n::t_fmt(
                "downloads.update_with",
                &[update_download_label(bytes).to_string()],
            ),
            None => lapinux_core::i18n::t("downloads.update_unknown_size").to_string(),
        }
    }

    /// Octets repris des fichiers déjà sur le disque : le volume traité
    /// dépasse alors ce que le réseau a réellement servi.
    pub(crate) fn reused_bytes(job: &Job) -> u64 {
        job.downloaded.saturating_sub(job.network)
    }
}

/// Vrai pour un état d'installation qui attend encore quelque chose de
/// l'utilisateur : elle avance, patiente dans la file, ou reprendra d'un clic.
/// Une tâche `Interrupted` (arrêtée par un redémarrage) fait partie du lot :
/// rien n'a été effacé, elle a sa place dans « En pause » et non dans
/// « Terminées », où elle passerait pour un transfert abouti.
fn awaiting_resume(state: JobState) -> bool {
    matches!(
        state,
        JobState::Running | JobState::Paused | JobState::Queued | JobState::Interrupted
    )
}

/// États qu'une suppression d'historique peut réellement retirer : le daemon
/// refuse les transferts qui ont encore une progression à reprendre.
fn can_delete_history_job(state: JobState) -> bool {
    matches!(
        state,
        JobState::Done | JobState::Failed | JobState::Cancelled
    )
}

/// Une seule carte par jeu : la tâche la plus récente gagne, et une tâche
/// active n'apparaît jamais aussi dans l'historique. Les programmes (Proton,
/// outils) ont leur propre catégorie : ils ne se mélangent pas aux jeux.
fn job_sections(jobs: &[Job]) -> JobSections {
    let mut sections = JobSections::default();
    let mut seen: HashSet<u32> = HashSet::new();
    // L'ordre du vecteur est celui voulu par l'utilisateur : la tâche du haut
    // est la prochaine à partir. Pour le regroupement par jeu, c'est la carte
    // active qui représente le jeu même si une vieille tâche terminée du même
    // jeu traîne plus haut dans la liste ; l'historique est retrié par date
    // tout à la fin.
    let active_appids: HashSet<u32> = jobs
        .iter()
        .filter(|job| awaiting_resume(job.state))
        .filter_map(|job| job.appid)
        .collect();
    for job in jobs.iter() {
        // Une tâche annulée n'a pas abouti : elle quitte la page au lieu de
        // s'empiler dans « Terminées ». Le daemon retire déjà sa carte (et les
        // fichiers partiels) ; cette garde évite qu'une annulation laissée par
        // un redémarrage précédent reste visible.
        if job.state == JobState::Cancelled {
            continue;
        }
        let active = awaiting_resume(job.state);
        match job.appid {
            Some(appid) => {
                if !active && active_appids.contains(&appid) {
                    // Le jeu a déjà sa carte active : pas de doublon en
                    // dessous ni de reprise proposée.
                    continue;
                }
                if !seen.insert(appid) {
                    continue;
                }
                if active {
                    sections.active_appids.insert(appid);
                    if matches!(job.state, JobState::Paused | JobState::Interrupted) {
                        sections.paused.push(job.clone());
                    } else {
                        sections.running.push(job.clone());
                    }
                } else {
                    // Une mise à jour du même jeu ne doit pas doubler la
                    // carte de reprise de l'historique.
                    if job.state == JobState::Failed {
                        sections.active_appids.insert(appid);
                    }
                    sections.history.push(job.clone());
                }
            }
            None => {
                if active {
                    if matches!(job.kind, JobKind::InstallProton | JobKind::InstallGraphics) {
                        sections.programs.push(job.clone());
                    } else if matches!(job.state, JobState::Paused | JobState::Interrupted) {
                        sections.paused.push(job.clone());
                    } else {
                        sections.running.push(job.clone());
                    }
                } else {
                    sections.history.push(job.clone());
                }
            }
        }
    }
    sections
        .history
        .sort_by_key(|job| std::cmp::Reverse(job.finished.unwrap_or(job.started)));
    sections
}

/// Géométrie du bandeau de vignettes d'une catégorie repliée : nombre de
/// vignettes qui tiennent, nombre résumé par « +N », et décalage depuis le
/// bord gauche de l'espace libre pour que la rangée finisse collée à droite.
pub(crate) fn collapsed_strip(free_width: f32, count: usize) -> (usize, usize, f32) {
    const SIZE: f32 = 20.0;
    const GAP: f32 = 5.0;
    let capacity = (((free_width + GAP) / (SIZE + GAP)).floor() as usize).max(1);
    let mut shown = count.min(capacity);
    let mut more = count - shown;
    // Le « +N » garde sa place : on retire des vignettes tant qu'il ne rentre
    // pas à droite du chevron.
    while more > 0
        && shown > 0
        && shown as f32 * SIZE + shown.saturating_sub(1) as f32 * GAP + 26.0 > free_width
    {
        shown -= 1;
        more += 1;
    }
    let strip = shown as f32 * SIZE + shown.saturating_sub(1) as f32 * GAP;
    let plus = if more > 0 { 22.0 } else { 0.0 };
    (shown, more, free_width - (strip + plus))
}

/// Vignettes d'une catégorie repliée : appid et nom. Les tâches sans jeu
/// (programmes, outils) n'ont pas d'illustration et sont écartées.
fn job_icons(jobs: &[Job]) -> Vec<(u32, String)> {
    jobs.iter()
        .filter_map(|job| {
            let appid = job.appid?;
            let name = if job.title.trim().is_empty() {
                sentence(&job.label)
            } else {
                job.title.clone()
            };
            Some((appid, name))
        })
        .collect()
}

/// Mises à jour triées par poids décroissant, sans les jeux déjà en
/// téléchargement : leur carte vit dans « En cours ».
fn sorted_updates(mut rows: Vec<UpdateRow>, active: &HashSet<u32>) -> Vec<UpdateRow> {
    rows.retain(|row| !active.contains(&row.appid));
    rows.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name)));
    rows
}

#[derive(Default)]
struct JobTileAction {
    pause: Option<(String, bool)>,
    cancel: Option<String>,
    resume: Option<u32>,
    /// Retirer définitivement une tâche de l'historique.
    delete: Option<String>,
    /// Ouvrir le rapport d'une tâche terminée.
    report: Option<String>,
    /// Ouvrir le panneau d'informations de la tâche.
    info: Option<String>,
    /// Glisser-déposer : (source, cible, insérer après la cible).
    reorder: Option<(String, String, bool)>,
}

impl JobTileAction {
    fn merge(&mut self, other: Self) {
        if other.pause.is_some() {
            self.pause = other.pause;
        }
        if other.cancel.is_some() {
            self.cancel = other.cancel;
        }
        if other.resume.is_some() {
            self.resume = other.resume;
        }
        if other.delete.is_some() {
            self.delete = other.delete;
        }
        if other.report.is_some() {
            self.report = other.report;
        }
        if other.info.is_some() {
            self.info = other.info;
        }
        if other.reorder.is_some() {
            self.reorder = other.reorder;
        }
    }

    fn is_empty(&self) -> bool {
        self.pause.is_none()
            && self.cancel.is_none()
            && self.resume.is_none()
            && self.delete.is_none()
            && self.report.is_none()
            && self.info.is_none()
            && self.reorder.is_none()
    }
}

/// Libellé d'étape prêt à afficher. Les anciens daemons parlent encore de
/// « doublages » et de « manifeste n/n » : l'interface emploie la même
/// formulation que la version actuelle, même avant le redémarrage du daemon.
fn phase_label(phase: &str) -> String {
    let phase = phase.trim();
    if phase.eq_ignore_ascii_case("nettoyage des doublages") {
        sentence(lapinux_core::i18n::t(
            "downloads.removing_audio_langs_title",
        ))
    } else if let Some(rest) = phase.strip_prefix("manifeste ") {
        lapinux_core::i18n::t_args("ui.reading_manifests", &[("rest", &rest.to_string())])
    } else {
        sentence(phase)
    }
}

/// Nature du transfert, affichée sous le titre : mise à jour, jeu de base
/// ou programme. Le moteur installe le build public du jeu ; les DLC restent
/// des applications séparées, d'où « Jeu de base » plutôt que « Jeu ».
fn job_kind_label(job: &Job) -> String {
    let base = match job.kind {
        JobKind::InstallGame if job.updating => lapinux_core::i18n::t("downloads.update"),
        JobKind::InstallGame => lapinux_core::i18n::t("downloads.base_game"),
        _ => lapinux_core::i18n::t("downloads.program"),
    };
    // Les contenus additionnels partent avec le jeu : la carte le dit, sinon
    // rien n'indique ce qui sera installé en plus du build public.
    match job.dlc.len() {
        0 => base.to_string(),
        1 => format!("{base} · 1 DLC"),
        count => format!("{base} · {count} DLC"),
    }
}

fn state_title(state: JobState) -> &'static str {
    match state {
        JobState::Queued => lapinux_core::i18n::t("downloads.state_queued"),
        JobState::Running => lapinux_core::i18n::t("downloads.active"),
        JobState::Paused => lapinux_core::i18n::t("downloads.paused"),
        JobState::Done => lapinux_core::i18n::t("downloads.done_title"),
        JobState::Failed => lapinux_core::i18n::t("downloads.failed"),
        JobState::Cancelled => lapinux_core::i18n::t("downloads.cancelled"),
        JobState::Interrupted => lapinux_core::i18n::t("downloads.state_interrupted"),
    }
}

/// Situation d'une tâche terminée, relative à maintenant.
fn job_when(job: &Job) -> String {
    let unix = job.finished.unwrap_or(job.started);
    if unix <= 0 {
        return lapinux_core::i18n::t("downloads.unknown_date").to_string();
    }
    let seconds = (lapinux_core::util::now_unix() - unix).max(0) as u64;
    if seconds < 60 {
        lapinux_core::i18n::t("downloads.just_now").to_string()
    } else {
        lapinux_core::i18n::t_args(
            "downloads.ago",
            &[("time", &lapinux_core::util::format_duration(seconds))],
        )
    }
}

fn job_remaining_seconds(job: &Job, net_rate: f32) -> Option<u64> {
    if job.total > job.downloaded && net_rate > 1024.0 {
        Some(((job.total - job.downloaded) as f32 / net_rate.max(1.0)) as u64)
    } else {
        None
    }
}

/// Débit réseau seul : le compteur « écrit » du daemon inclut les fichiers
/// simplement vérifiés lors d'une reprise, ce qui affichait un débit disque
/// spectaculairement faux.
/// Reprise après une pause : la fenêtre de débit doit repartir de zéro. Les
/// chunks déjà demandés terminent d'un coup au redémarrage ; mesurés dans
/// l'ancienne fenêtre, ils affichaient un débit de plusieurs Gb/s.
fn is_resume(previous: Option<JobState>, current: JobState) -> bool {
    current == JobState::Running && matches!(previous, Some(JobState::Paused | JobState::Queued))
}

/// Poids du téléchargement annoncé par Steam pour le build public. Notre
/// téléchargement récupère tout le build (pas de delta), donc la taille du
/// téléchargement passe avant celle installée.
pub(crate) fn public_download_bytes(details: &lapinux_core::model::SteamDetails) -> Option<u64> {
    let size = details
        .size_windows
        .or(details.size_linux)
        .or(details.size_macos)?;
    if size.download > 0 {
        Some(size.download)
    } else if size.install > 0 {
        Some(size.install)
    } else {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FolderAction {
    Edit,
    Open,
}

/// Bouton-icône de la carte dossier : même apparence qu'un bouton de carte,
/// mais l'icône est dessinée au trait (la police n'a pas d'icônes fiables).
fn folder_icon_button(ui: &mut egui::Ui, rect: egui::Rect, action: FolderAction) -> bool {
    // `ui.interact` plutôt qu'un widget posé avec `ui.put` : ce dernier
    // déplace le curseur de la carte, qui ne mesurait alors que la hauteur
    // des icônes et laissait le test de débit se superposer.
    let id = ui.id().with((
        "folder-icon",
        match action {
            FolderAction::Edit => "edit",
            FolderAction::Open => "open",
        },
    ));
    let response = ui.interact(rect, id, egui::Sense::click());
    let hovered = response.hovered();
    let pressed = response.is_pointer_button_down_on();
    let rounding = egui::Rounding::same(8.0);
    // Un fond de carte, sans ombre ni cadre lourd : les deux pictogrammes
    // restent les seuls éléments visibles.
    ui.painter().rect(
        rect,
        rounding,
        if pressed {
            CARD_ACTIVE()
        } else if hovered {
            CARD_HOVER()
        } else {
            CARD()
        },
        egui::Stroke::new(
            1.0_f32,
            if hovered || response.has_focus() {
                with_alpha(ACCENT(), 0.65)
            } else {
                BORDER()
            },
        ),
    );
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect.shrink(2.0),
            rounding,
            egui::Stroke::new(1.0_f32, ACCENT()),
        );
    }
    let ink = if hovered || response.has_focus() {
        ACCENT()
    } else {
        TEXT_DIM()
    };
    match action {
        FolderAction::Edit => paint_pencil(ui.painter(), rect.center(), ink),
        FolderAction::Open => paint_folder(ui.painter(), rect.center(), ink),
    }
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(match action {
            FolderAction::Edit => lapinux_core::i18n::t("downloads.edit_folder"),
            FolderAction::Open => lapinux_core::i18n::t("downloads.open_folder"),
        })
        .clicked()
}

/// Crayon au trait : deux flancs, une pointe et un talon, lisible à 24 px.
fn paint_pencil(painter: &egui::Painter, center: egui::Pos2, color: egui::Color32) {
    let stroke = egui::Stroke::new(1.35_f32, color);
    let back = center + egui::vec2(-4.8, 4.8);
    let front = center + egui::vec2(3.8, -3.8);
    let offset = egui::vec2(1.3, 1.3);
    painter.line_segment([back + offset, front + offset], stroke);
    painter.line_segment([back - offset, front - offset], stroke);
    painter.line_segment([back - offset, back + offset], stroke);
    painter.add(egui::Shape::convex_polygon(
        vec![
            front - offset,
            front + offset,
            front + egui::vec2(2.4, -2.4),
        ],
        color,
        egui::Stroke::NONE,
    ));
}

/// Dossier et flèche sortante : le pictogramme « ouvrir le dossier ».
fn paint_folder(painter: &egui::Painter, center: egui::Pos2, color: egui::Color32) {
    let stroke = egui::Stroke::new(1.35_f32, color);
    let body = egui::Rect::from_center_size(center + egui::vec2(0.0, 2.0), egui::vec2(12.0, 8.0));
    painter.rect_stroke(body, egui::Rounding::same(2.0), stroke);
    let tab = egui::Rect::from_min_size(body.min + egui::vec2(0.0, -3.0), egui::vec2(5.5, 3.0));
    painter.line_segment([body.left_top(), tab.left_top()], stroke);
    painter.line_segment([tab.left_top(), tab.right_top()], stroke);
    painter.line_segment([tab.right_top(), tab.right_bottom()], stroke);
    let start = center + egui::vec2(0.0, 1.0);
    let tip = center + egui::vec2(4.7, -4.7);
    painter.line_segment([start, tip], stroke);
    painter.line_segment([tip, tip + egui::vec2(-3.0, 0.0)], stroke);
    painter.line_segment([tip, tip + egui::vec2(0.0, 3.0)], stroke);
}

/// Bouton de carte : un vrai widget egui, donc focusable au clavier et à la
/// manette, posé à un emplacement précis de la carte.
fn tile_button(ui: &mut egui::Ui, rect: egui::Rect, text: &str, danger: bool) -> bool {
    let ink = if danger { DANGER } else { TEXT() };
    let stroke = if danger {
        with_alpha(DANGER, 0.7)
    } else {
        egui::Color32::from_white_alpha(45)
    };
    ui.put(
        rect,
        egui::Button::new(egui::RichText::new(text).size(10.5).color(ink))
            .fill(egui::Color32::from_black_alpha(90))
            .stroke(egui::Stroke::new(1.0_f32, stroke))
            .rounding(egui::Rounding::same(7.0)),
    )
    .clicked()
}

/// Contexte visible d'une tâche : le pas du daemon quand la carte ne
/// l'affiche pas déjà, le serveur, le dépôt et l'ancienneté.
fn download_facts(job: &Job) -> Vec<String> {
    let working = matches!(
        job.state,
        JobState::Running | JobState::Queued | JobState::Paused
    );
    if !working {
        return Vec::new();
    }
    let step = job.message.trim();
    let mut facts = Vec::new();
    // Certains messages ne font que répéter l'état (« en pause ») : la carte
    // l'affiche déjà, inutile de l'écrire deux fois.
    if job.state == JobState::Paused
        && !step.is_empty()
        && step.to_lowercase() != state_title(JobState::Paused).to_lowercase()
    {
        facts.push(step.to_string());
    }
    // Steam installe les DLC comme des applications séparées : une
    // installation de jeu ne les comprend pas. Placé tôt pour rester visible
    // quand la carte compacte coupe la fin de la ligne.
    if job.kind == JobKind::InstallGame {
        facts.push(lapinux_core::i18n::t("downloads.no_dlc").to_string());
    }
    if !job.server.is_empty() {
        let source = if job.kind == JobKind::InstallGame {
            // Chaque boutique a son propre réseau : un jeu Epic se télécharge
            // chez Epic, pas sur les CDN de Steam. Les tâches Steam portent le
            // moteur interne (« Lapinux ») comme backend, pas un nom de
            // boutique : elles gardent donc l'étiquette historique.
            let backend = job.backend.trim();
            let steam = backend.is_empty()
                || backend.eq_ignore_ascii_case("steam")
                || backend.eq_ignore_ascii_case("lapinux");
            if steam {
                "CDN Steam".to_string()
            } else {
                format!("CDN {backend}")
            }
        } else {
            "Serveur".to_string()
        };
        facts.push(format!("{source} · {}", job.server));
    }
    // Le pas du daemon cite déjà le dépôt (« dépôt 1/11 · téléchargement ») :
    // inutile de répéter « dépôt 1/11 » juste à côté.
    let step_lower = step.to_lowercase();
    let part_word = lapinux_core::i18n::t("downloads.part");
    let part_token = part_word.split(" {}").next().unwrap_or("partie");
    if job.depots > 1
        && !step_lower.contains("partie")
        && !step_lower.contains(part_token)
        && !step_lower.contains("dépôt")
        && !step_lower.contains("depot")
    {
        facts.push(lapinux_core::i18n::t_fmt(
            "downloads.part",
            &[job.depot.to_string(), job.depots.to_string()],
        ));
    }
    if job.started > 0 {
        let elapsed = (lapinux_core::util::now_unix() - job.started).max(0) as u64;
        facts.push(lapinux_core::i18n::t_fmt(
            "downloads.since",
            &[lapinux_core::util::format_duration(elapsed)],
        ));
    }
    facts
}

/// Pictogrammes des commandes rondes de la carte large.
#[derive(Clone, Copy)]
pub(crate) enum HeroGlyph {
    Pause,
    Play,
    Close,
    Check,
    Down,
    Disk,
    Document,
    Trash,
    Info,
    Copy,
}

/// Boutique qui mène une tâche, d'après le nom de son moteur. Les
/// téléchargements Steam passent par le moteur interne (« Lapinux ») et
/// gardent la marque Steam. Sans cette lecture, la carte d'un jeu Epic
/// affichait le logo Steam.
fn job_store(job: &Job) -> lapinux_core::stores::Store {
    crate::LapinuxApp::job_store(job).unwrap_or(lapinux_core::stores::Store::Steam)
}

/// Carte large d'un téléchargement actif, dans l'esprit d'Epic : illustration
/// pleine hauteur, titre, une ligne d'informations (volume, débit,
/// pourcentage) et commandes rondes. Les commandes restent de vrais widgets :
/// le clavier et la manette les atteignent.
fn download_hero_tile(
    ui: &mut egui::Ui,
    width: f32,
    job: &Job,
    title: &str,
    cover: Option<&CoverArt>,
    icon: Option<&egui::TextureHandle>,
    source: Option<&egui::TextureHandle>,
    net_rate: f32,
    eta_rate: f32,
) -> JobTileAction {
    const HEIGHT: f32 = 96.0;
    const PAD: f32 = 20.0;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, HEIGHT), egui::Sense::hover());
    let painter = ui.painter().clone();
    // Bords droits, comme la vignette du jeu : une carte sobre, sans arrondi.
    painter.rect_filled(rect, egui::Rounding::same(0.0), CARD());

    let working = matches!(
        job.state,
        JobState::Running | JobState::Queued | JobState::Paused
    );
    let paused = job.state == JobState::Paused;
    // Une tâche dans la file n'a pas encore de drapeau de pause : seule
    // l'annulation la retire.
    let can_pause = matches!(job.state, JobState::Running | JobState::Paused)
        && job.kind == JobKind::InstallGame;
    let ratio = if job.state == JobState::Done {
        1.0
    } else {
        job.progress.clamp(0.0, 1.0)
    };

    // La bannière du jeu, floutée, occupe toute la carte. Un voile sombre
    // dégradé garde le titre et les chiffres lisibles par-dessus.
    paint_hero_background(ui, rect, title, cover, icon);
    let palette = match job.state {
        JobState::Failed | JobState::Cancelled | JobState::Interrupted => None,
        _ => cover
            .map(|cover| cover.palette.as_slice())
            .filter(|palette| palette.len() > 1),
    };
    let progress_color = match job.state {
        JobState::Failed => DANGER,
        JobState::Cancelled | JobState::Interrupted => WARN,
        JobState::Paused => ACCENT().gamma_multiply(0.55),
        _ => ACCENT(),
    };
    // Le fond se remplit de gauche à droite, sous le texte.
    paint_card_fill(&painter, rect, ratio, palette, progress_color, paused);
    // Poignée de glisser-déposer, dans la marge gauche : six points discrets.
    // Elle ne prend la place ni de l'icône ni du titre.
    if working {
        let handle_rect = egui::Rect::from_center_size(
            egui::pos2(rect.left() + 9.0, rect.center().y),
            egui::vec2(14.0, 40.0),
        );
        let handle = ui.interact(
            handle_rect,
            ui.id().with(("job-drag", job.id.as_str())),
            egui::Sense::drag(),
        );
        let ink = if handle.hovered() || handle.dragged() {
            TEXT()
        } else {
            TEXT_DIM()
        };
        for row in -1..=1 {
            for column in 0..2 {
                painter.circle_filled(
                    egui::pos2(
                        handle_rect.center().x - 2.0 + column as f32 * 4.0,
                        handle_rect.center().y + row as f32 * 4.0,
                    ),
                    1.0,
                    ink,
                );
            }
        }
        handle.dnd_set_drag_payload::<String>(job.id.clone());
        let _ = handle.on_hover_text(lapinux_core::i18n::t("downloads.drag_order"));
    }

    let icon_rect = egui::Rect::from_min_size(
        egui::pos2(rect.left() + PAD, rect.center().y - 18.0),
        egui::vec2(36.0, 36.0),
    );
    painter.rect_filled(
        icon_rect.translate(egui::vec2(0.0, 2.0)),
        egui::Rounding::same(0.0),
        egui::Color32::from_black_alpha(90),
    );
    painter.rect_filled(
        icon_rect,
        egui::Rounding::same(0.0),
        egui::Color32::from_black_alpha(55),
    );
    match icon {
        Some(icon) => {
            egui::Image::new((icon.id(), icon_rect.size()))
                .rounding(egui::Rounding::same(0.0))
                .paint_at(ui, icon_rect);
        }
        None => {
            let initial = title
                .chars()
                .find(|character| character.is_alphanumeric())
                .map(|character| character.to_uppercase().to_string())
                .unwrap_or_else(|| "?".into());
            painter.text(
                icon_rect.center(),
                egui::Align2::CENTER_CENTER,
                initial,
                display_font(19.0),
                with_alpha(TEXT(), 0.9),
            );
        }
    }
    painter.rect_stroke(
        icon_rect,
        egui::Rounding::same(0.0),
        egui::Stroke::new(1.0_f32, egui::Color32::from_white_alpha(48)),
    );

    // Commandes rondes, en haut à droite : informations, action principale
    // (pause, reprise ou rapport), puis annulation ou suppression. Une tâche
    // interrompue garde seulement sa reprise : elle n'est pas dans l'historique.
    let controls_y = rect.center().y;
    // Les trois commandes ont la même taille et le même écart : une carte
    // ressemble à sa voisine quelle que soit la catégorie.
    const CONTROL: f32 = 30.0;
    const CONTROL_GAP: f32 = 8.0;
    let step = CONTROL + CONTROL_GAP;
    let cancel_rect = egui::Rect::from_center_size(
        egui::pos2(rect.right() - 25.0, controls_y),
        egui::vec2(CONTROL, CONTROL),
    );
    let primary_rect = egui::Rect::from_center_size(
        egui::pos2(cancel_rect.center().x - step, controls_y),
        egui::vec2(CONTROL, CONTROL),
    );
    let info_rect = egui::Rect::from_center_size(
        egui::pos2(primary_rect.center().x - step, controls_y),
        egui::vec2(CONTROL, CONTROL),
    );
    // Reprendre vaut pour ce qui peut continuer : une installation interrompue
    // par un redémarrage (fichiers conservés) ou un échec retentable. Une
    // annulation, elle, n'arrive jamais jusqu'ici.
    let can_resume = matches!(
        job.state,
        JobState::Interrupted | JobState::Failed | JobState::Cancelled
    ) && job.appid.is_some();
    let has_report = job.state == JobState::Done && report_for(job).is_some();
    let mut action = JobTileAction::default();
    if can_pause {
        let (glyph, fill, ink, tip) = if paused {
            (
                HeroGlyph::Play,
                ACCENT(),
                egui::Color32::from_rgb(24, 25, 27),
                lapinux_core::i18n::t("downloads.resume"),
            )
        } else {
            (
                HeroGlyph::Pause,
                egui::Color32::from_black_alpha(80),
                TEXT(),
                lapinux_core::i18n::t("downloads.pause"),
            )
        };
        if hero_glyph_button(ui, primary_rect, &job.id, glyph, fill, ink, tip) {
            action.pause = Some((job.id.clone(), !paused));
        }
    } else if can_resume {
        let tip = if matches!(job.state, JobState::Cancelled | JobState::Interrupted) {
            lapinux_core::i18n::t("downloads.resume")
        } else {
            lapinux_core::i18n::t("downloads.retry")
        };
        if hero_glyph_button(
            ui,
            primary_rect,
            &job.id,
            HeroGlyph::Play,
            ACCENT(),
            egui::Color32::from_rgb(24, 25, 27),
            tip,
        ) {
            action.resume = job.appid;
        }
    } else if has_report {
        if hero_glyph_button(
            ui,
            primary_rect,
            &job.id,
            HeroGlyph::Document,
            egui::Color32::from_black_alpha(80),
            TEXT(),
            lapinux_core::i18n::t("downloads.report_title"),
        ) {
            action.report = Some(job.id.clone());
        }
    }
    if working || job.state == JobState::Interrupted {
        if hero_cancel(ui, cancel_rect, &job.id) {
            action.cancel = Some(job.id.clone());
        }
    } else if can_delete_history_job(job.state)
        && hero_glyph_button(
            ui,
            cancel_rect,
            &job.id,
            HeroGlyph::Trash,
            egui::Color32::from_black_alpha(80),
            TEXT(),
            lapinux_core::i18n::t("downloads.remove_history"),
        )
    {
        action.delete = Some(job.id.clone());
    }
    if hero_glyph_button(
        ui,
        info_rect,
        &job.id,
        HeroGlyph::Info,
        egui::Color32::from_black_alpha(80),
        TEXT(),
        lapinux_core::i18n::t("downloads.info_title"),
    ) {
        action.info = Some(job.id.clone());
    }

    let text_x = icon_rect.right() + 10.0;
    // La ligne de titre s'arrête avant la colonne de boutons, informations
    // comprises.
    let text_right = info_rect.left() - 16.0;
    let text_width = (text_right - text_x).max(80.0);
    let bytes = |value: u64| lapinux_core::util::format_bytes(value);
    // Les chiffres et la ligne de contexte commencent sous l'icône, pas après :
    // leur clip doit donc démarrer au même endroit, sinon « Téléchargé »
    // perdait ses premières lettres et la ligne « sans DLC · CDN… » son début.
    let text_painter = painter.with_clip_rect(egui::Rect::from_min_max(
        egui::pos2(rect.left() + PAD - 2.0, rect.top()),
        egui::pos2(rect.right() - 14.0, rect.bottom()),
    ));

    // Titre, puis une seule ligne d'informations : le volume et le débit
    // courant, le pourcentage restant à droite. Sans séparateur ni rangée de
    // métriques en bas : la carte reste lisible d'un coup d'œil.
    text_painter.text(
        egui::pos2(text_x, icon_rect.center().y - 11.0),
        egui::Align2::LEFT_CENTER,
        lapinux_core::util::ellipsize(title, (text_width / 8.0).max(8.0) as usize),
        display_font(16.5),
        TEXT(),
    );
    // La nature du transfert passe juste sous le nom : « Jeu de base »,
    // « Mise à jour » ou « Programme », puis la marque du store.
    let kind_rect = text_painter.text(
        egui::pos2(text_x, icon_rect.center().y + 11.0),
        egui::Align2::LEFT_CENTER,
        lapinux_core::util::ellipsize(&job_kind_label(job), (text_width / 6.0).max(6.0) as usize),
        egui::FontId::proportional(10.5),
        TEXT_DIM(),
    );
    paint_source_logo(
        &text_painter,
        kind_rect.right() + 6.0,
        kind_rect.center().y,
        text_right,
        source,
    );

    let phase = job.message.trim();
    let preparing = working && job.total == 0;
    let volume = if job.total > 0 {
        format!("{} / {}", bytes(job.downloaded), bytes(job.total))
    } else {
        bytes(job.downloaded)
    };
    let meaningful_phase = !phase.is_empty()
        && !phase.eq_ignore_ascii_case(lapinux_core::i18n::t("downloads.download_lower"));
    let detail = if job.state == JobState::Queued {
        "En attente…".to_string()
    } else if paused {
        volume
    } else if preparing {
        let mut text = if meaningful_phase {
            phase_label(phase)
        } else {
            lapinux_core::i18n::t("downloads.preparing").to_string()
        };
        if !text.ends_with('…') {
            text.push('…');
        }
        text
    } else if working {
        if net_rate <= 0.5 && job.downloaded == 0 && meaningful_phase {
            // Vérification ou manifestes sans octet téléchargé : l'étape est
            // plus utile que « 0 o / 50,3 Go ».
            let mut text = phase_label(phase);
            if !text.ends_with('…') {
                text.push('…');
            }
            text
        } else if net_rate > 0.5 {
            format!("{volume} · {}", network_rate(net_rate))
        } else {
            volume
        }
    } else if matches!(
        job.state,
        JobState::Failed | JobState::Cancelled | JobState::Interrupted
    ) {
        // L'état reste le premier mot de la carte : une tâche arrêtée ne se
        // confond pas avec une tâche qui avance.
        format!("{} · {volume}", state_title(job.state))
    } else if job.state == JobState::Done {
        lapinux_core::i18n::t_args(
            "downloads.finished_volume",
            &[("volume", &volume.to_string())],
        )
    } else {
        volume
    };

    // Les deux lignes de synthèse sont centrées dans la carte : volume et
    // débit d'abord, pourcentage et temps restant ensuite.
    let center_x = rect.center().x;
    let detail_chars = ((rect.width() - 40.0) / 6.0).max(10.0) as usize;
    text_painter.text(
        egui::pos2(center_x, rect.center().y - 11.0),
        egui::Align2::CENTER_CENTER,
        lapinux_core::util::ellipsize(&detail, detail_chars),
        display_font(13.5),
        if working && !paused {
            TEXT()
        } else {
            TEXT_DIM()
        },
    );
    // Tant que le poids exact n'est pas connu (manifestes en cours), un
    // « 0 % » serait faux : la carte affiche une attente discrète.
    if preparing {
        text_painter.text(
            egui::pos2(center_x, rect.center().y + 11.0),
            egui::Align2::CENTER_CENTER,
            "…",
            display_font(14.0),
            TEXT_DIM(),
        );
    } else {
        let percent = format!("{:.0} %", (ratio * 100.0).round());
        let extra = if working && !paused {
            job_remaining_seconds(job, eta_rate)
                .filter(|seconds| *seconds > 0)
                .map(|seconds| {
                    lapinux_core::i18n::t_args(
                        "downloads.time_left_suffix",
                        &[("time", &lapinux_core::util::format_duration(seconds))],
                    )
                })
                .unwrap_or_default()
        } else if paused {
            lapinux_core::i18n::t("downloads.paused_suffix").to_string()
        } else if job.state == JobState::Done {
            match job_when(job) {
                when if when == lapinux_core::i18n::t("downloads.unknown_date") => String::new(),
                when => format!(" · {when}"),
            }
        } else {
            String::new()
        };
        let percent_galley = text_painter.layout_no_wrap(percent, display_font(14.0), TEXT());
        let extra_galley = text_painter.layout_no_wrap(
            extra.clone(),
            egui::FontId::proportional(10.5),
            TEXT_DIM(),
        );
        let total = percent_galley.size().x + extra_galley.size().x;
        let start = center_x - total / 2.0;
        text_painter.galley(
            egui::pos2(
                start,
                rect.center().y + 11.0 - percent_galley.size().y / 2.0,
            ),
            percent_galley.clone(),
            TEXT(),
        );
        if !extra.is_empty() {
            text_painter.galley(
                egui::pos2(
                    start + percent_galley.size().x,
                    rect.center().y + 11.0 - extra_galley.size().y / 2.0,
                ),
                extra_galley,
                TEXT_DIM(),
            );
        }
    }

    paint_card_border(&painter, rect);

    let mut tooltip = lapinux_core::i18n::t_fmt(
        "downloads.summary_block",
        &[
            state_title(job.state).to_string(),
            job.backend.to_string(),
            bytes(job.downloaded),
            if job.total > 0 {
                bytes(job.total)
            } else {
                "?".to_string()
            },
            bytes(job.network),
            bytes(job.written),
            if job.server.is_empty() {
                "—".to_string()
            } else {
                job.server.clone()
            },
            job.depot.to_string(),
            job.depots.to_string(),
        ],
    );
    if job.state == JobState::Running {
        if let Some(seconds) = job_remaining_seconds(job, eta_rate).filter(|seconds| *seconds > 0) {
            tooltip.push_str(&lapinux_core::i18n::t_fmt(
                "downloads.remaining",
                &[lapinux_core::util::format_duration(seconds)],
            ));
        }
    }
    let reused = wording::reused_bytes(job);
    if reused >= 1024 * 1024 {
        tooltip.push_str(&lapinux_core::i18n::t_fmt(
            "downloads.reused_from_disk",
            &[bytes(reused)],
        ));
    }
    let facts = download_facts(job);
    if !facts.is_empty() {
        tooltip.push('\n');
        tooltip.push_str(&facts.join(" · "));
    }
    if job.kind == JobKind::InstallGame {
        tooltip.push_str(lapinux_core::i18n::t("downloads.content_line"));
    }
    if job.started > 0 {
        tooltip.push_str(&lapinux_core::i18n::t_fmt(
            "downloads.started_at",
            &[lapinux_core::util::format_datetime(job.started)],
        ));
    }
    let response = response.on_hover_text(tooltip);
    if working && response.dnd_hover_payload::<String>().is_some() {
        painter.rect_stroke(
            rect,
            egui::Rounding::same(0.0),
            egui::Stroke::new(1.5_f32, ACCENT()),
        );
    }
    if working {
        if let Some(source) = response.dnd_release_payload::<String>() {
            if source.as_str() != job.id {
                let after = ui
                    .ctx()
                    .pointer_interact_pos()
                    .map(|pointer| pointer.y > rect.center().y)
                    .unwrap_or(false);
                action.reorder = Some((source.to_string(), job.id.clone(), after));
            }
        }
    }
    action
}

/// Logo de la plateforme d'où vient le téléchargement, dessiné après le titre
/// et le type de contenu. Rien n'est dessiné s'il déborderait sur les boutons.
fn paint_source_logo(
    painter: &egui::Painter,
    left: f32,
    center_y: f32,
    limit: f32,
    source: Option<&egui::TextureHandle>,
) {
    let Some(texture) = source else {
        return;
    };
    const SIZE: f32 = 14.0;
    if left + SIZE > limit {
        return;
    }
    let rect = egui::Rect::from_min_size(
        egui::pos2(left, center_y - SIZE / 2.0),
        egui::vec2(SIZE, SIZE),
    );
    painter.image(
        texture.id(),
        rect,
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );
}

/// Remplissage de progression : une teinte pleine hauteur part du bord
/// gauche et s'arrête au bord droit quand la tâche est terminée. Le bord de
/// tête est adouci, il ne s'agit pas d'une ligne.
fn paint_card_fill(
    painter: &egui::Painter,
    rect: egui::Rect,
    ratio: f32,
    palette: Option<&[egui::Color32]>,
    fallback: egui::Color32,
    paused: bool,
) {
    let ratio = ratio.clamp(0.0, 1.0);
    // Au-delà du front de progression, la bannière s'éteint : la carte ne
    // doit pas rayonner sur des octets qui ne sont pas encore arrivés.
    paint_pending_veil(painter, rect, ratio);
    if ratio <= 0.0 {
        return;
    }
    let left = rect.left();
    let right = left + rect.width() * ratio;
    let width = right - left;
    if width <= 0.5 {
        return;
    }
    let radius = 0.0;
    let steps = ((width / 2.0).ceil() as usize).clamp(8, 640);
    let fade = 24.0_f32;
    let base_alpha = if paused { 36.0 } else { 64.0 };
    let full = ratio >= 0.999;
    let mut mesh = egui::Mesh::default();
    for index in 0..=steps {
        let t = index as f32 / steps as f32;
        let x = left + width * t;
        let color = progress_color(palette, t * ratio, fallback);
        let edge = if full {
            1.0
        } else {
            ((right - x) / fade).clamp(0.0, 1.0)
        };
        let color = egui::Color32::from_rgba_unmultiplied(
            color.r(),
            color.g(),
            color.b(),
            (base_alpha * edge) as u8,
        );
        let (top, bottom) = rounded_extent(rect, x, radius);
        mesh.colored_vertex(egui::pos2(x, top), color);
        mesh.colored_vertex(egui::pos2(x, bottom), color);
        if index > 0 {
            let base = (index as u32 - 1) * 2;
            mesh.add_triangle(base, base + 1, base + 2);
            mesh.add_triangle(base + 1, base + 2, base + 3);
        }
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// Voile posé sur la partie **pas encore téléchargée** de la carte : la
/// bannière y perd son éclat, la lumière s'arrête au front de progression.
fn paint_pending_veil(painter: &egui::Painter, rect: egui::Rect, ratio: f32) {
    if ratio >= 0.999 {
        return;
    }
    let left = rect.left() + rect.width() * ratio;
    let width = rect.right() - left;
    if width <= 0.5 {
        return;
    }
    let steps = ((width / 2.0).ceil() as usize).clamp(8, 640);
    // Le voile s'installe en douceur sur quelques dizaines de pixels, puis
    // reste franc : une frontière nette aurait l'air d'un bug d'affichage.
    let soft = 48.0_f32.min(width);
    let mut mesh = egui::Mesh::default();
    for index in 0..=steps {
        let t = index as f32 / steps as f32;
        let x = left + width * t;
        let strength = (width * t / soft).clamp(0.0, 1.0);
        // Assez pour éteindre l'éclat de la bannière, jamais au point de faire
        // une moitié de carte noire.
        let alpha = 8.0 + 58.0 * strength;
        let color = egui::Color32::from_rgba_unmultiplied(9, 10, 12, alpha as u8);
        let (top, bottom) = rounded_extent(rect, x, 0.0);
        mesh.colored_vertex(egui::pos2(x, top), color);
        mesh.colored_vertex(egui::pos2(x, bottom), color);
        if index > 0 {
            let base = (index as u32 - 1) * 2;
            mesh.add_triangle(base, base + 1, base + 2);
            mesh.add_triangle(base + 1, base + 2, base + 3);
        }
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// Bord de la carte, indépendant de la progression : il masque les franges
/// de couleur de la bannière sur les coins.
fn paint_card_border(painter: &egui::Painter, rect: egui::Rect) {
    let mask = rounded_outline(rect.shrink(0.5), 0.0);
    painter.add(egui::Shape::closed_line(
        mask,
        egui::Stroke::new(1.0_f32, egui::Color32::from_black_alpha(80)),
    ));
    let border = rounded_outline(rect.shrink(1.25), 0.0);
    painter.add(egui::Shape::closed_line(
        border,
        egui::Stroke::new(1.0_f32, egui::Color32::from_white_alpha(24)),
    ));
}

fn progress_color(
    palette: Option<&[egui::Color32]>,
    t: f32,
    fallback: egui::Color32,
) -> egui::Color32 {
    match palette {
        Some(palette) if !palette.is_empty() => palette_at(palette, t),
        _ => fallback,
    }
}

fn palette_at(palette: &[egui::Color32], t: f32) -> egui::Color32 {
    if palette.is_empty() {
        return ACCENT();
    }
    let last = palette.len() - 1;
    if last == 0 {
        return palette[0];
    }
    let position = t.clamp(0.0, 1.0) * last as f32;
    let index = position.floor() as usize;
    if index >= last {
        return palette[last];
    }
    lerp_color(palette[index], palette[index + 1], position - index as f32)
}

/// Fond de la carte large : la bannière du jeu, floutée, pleine largeur,
/// recouverte d'un voile sombre pour que titre, état et chiffres restent
/// lisibles quelle que soit l'image.
fn paint_hero_background(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    title: &str,
    cover: Option<&CoverArt>,
    icon: Option<&egui::TextureHandle>,
) {
    let rounding = egui::Rounding::same(0.0);
    let painter = ui.painter().clone();
    match cover {
        Some(cover) => paint_cover(ui, rect, &cover.blurred, rounding),
        None => {
            painter.rect_filled(rect, rounding, CARD_HOVER());
            match icon {
                Some(icon) => {
                    // Icône géante et très discrète pour meubler le fond quand
                    // aucune bannière n'est disponible.
                    let ghost = egui::Rect::from_center_size(
                        egui::pos2(rect.right() - 72.0, rect.center().y),
                        egui::vec2(190.0, 190.0),
                    );
                    painter.image(
                        icon.id(),
                        ghost,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::from_white_alpha(16),
                    );
                }
                None => {
                    let initial = title
                        .chars()
                        .find(|character| character.is_alphanumeric())
                        .map(|character| character.to_uppercase().to_string())
                        .unwrap_or_else(|| "?".into());
                    painter.text(
                        egui::pos2(rect.right() - 74.0, rect.center().y),
                        egui::Align2::CENTER_CENTER,
                        initial,
                        display_font(96.0),
                        egui::Color32::from_white_alpha(18),
                    );
                }
            }
        }
    }

    paint_hero_overlay(&painter, rect, 0.0);
}

/// Voile dégradé de gauche à droite, découpé aux coins arrondis de la carte.
/// Un simple maillage rectangulaire aurait rempli les quatre coins et cassé
/// l'arrondi ; chaque colonne suit donc le bord du rectangle arrondi.
/// Bords haut et bas d'un rectangle arrondi à l'abscisse `x` : sert aux
/// maillages qui doivent suivre les coins de la carte.
fn rounded_extent(rect: egui::Rect, x: f32, radius: f32) -> (f32, f32) {
    let left = rect.left() + radius;
    let right = rect.right() - radius;
    let dx = if x < left {
        left - x
    } else if x > right {
        x - right
    } else {
        0.0
    };
    let dy = if dx > 0.0 {
        radius - (radius * radius - dx.min(radius).powi(2)).max(0.0).sqrt()
    } else {
        0.0
    };
    (rect.top() + dy, rect.bottom() - dy)
}

fn paint_hero_overlay(painter: &egui::Painter, rect: egui::Rect, radius: f32) {
    let radius = radius
        .min(rect.width() / 2.0)
        .min(rect.height() / 2.0)
        .max(0.0);
    let steps = ((rect.width() / 2.0).ceil() as usize).max(8);
    let mut mesh = egui::Mesh::default();
    for index in 0..=steps {
        let t = index as f32 / steps as f32;
        let alpha = if t < 0.58 {
            220.0 + (165.0 - 220.0) * (t / 0.58)
        } else {
            165.0 + (95.0 - 165.0) * ((t - 0.58) / 0.42)
        };
        let top = egui::Color32::from_rgba_unmultiplied(13, 14, 16, alpha as u8);
        let bottom =
            egui::Color32::from_rgba_unmultiplied(13, 14, 16, (alpha + 40.0).min(230.0) as u8);
        let x = rect.left() + rect.width() * t;
        let (top_y, bottom_y) = rounded_extent(rect, x, radius);
        mesh.colored_vertex(egui::pos2(x, top_y), top);
        mesh.colored_vertex(egui::pos2(x, bottom_y), bottom);
        if index > 0 {
            let base = (index as u32 - 1) * 2;
            mesh.add_triangle(base, base + 1, base + 2);
            mesh.add_triangle(base + 1, base + 2, base + 3);
        }
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// Petit bouton « i » : un cercle dessiné, une infobulle et un état actif,
/// pour révéler une explication sans l'afficher en permanence.
fn help_button(ui: &mut egui::Ui, open: bool, tooltip: &str) -> bool {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::click());
    let response = response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(tooltip);
    if response.hovered() || open {
        ui.painter()
            .rect_filled(rect, egui::Rounding::same(6.0), CARD_HOVER());
    }
    paint_hero_glyph(
        ui.painter(),
        rect.center(),
        HeroGlyph::Info,
        if response.hovered() || open {
            TEXT()
        } else {
            TEXT_DIM()
        },
    );
    response.clicked()
}

/// Bouton rond : fond plein, pictogramme centré, infobulle.
fn hero_glyph_button(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    key: &str,
    glyph: HeroGlyph,
    fill: egui::Color32,
    ink: egui::Color32,
    tooltip: &str,
) -> bool {
    let response = ui.interact(
        rect,
        ui.id().with(("hero-button", key, tooltip)),
        egui::Sense::click(),
    );
    let painter = ui.painter();
    painter.rect_filled(rect, egui::Rounding::same(rect.height() / 2.0), fill);
    if response.hovered() {
        painter.rect_stroke(
            rect,
            egui::Rounding::same(rect.height() / 2.0),
            egui::Stroke::new(1.5_f32, egui::Color32::from_white_alpha(140)),
        );
    }
    paint_hero_glyph(painter, rect.center(), glyph, ink);
    let clicked = response.clicked();
    let _ = response.on_hover_text(tooltip);
    clicked
}

/// Annulation en deux temps : la première pression arme le bouton pendant
/// quatre secondes, la seconde annule la tâche.
fn hero_cancel(ui: &mut egui::Ui, rect: egui::Rect, job_id: &str) -> bool {
    let confirm = ui.id().with(("hero-cancel", job_id));
    let now = ui.ctx().input(|input| input.time);
    let armed = ui
        .ctx()
        .memory(|memory| memory.data.get_temp::<f64>(confirm))
        .map(|until| until > now)
        .unwrap_or(false);
    if armed {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(400));
    }
    let (glyph, fill, ink, tooltip) = if armed {
        (
            HeroGlyph::Check,
            DANGER,
            egui::Color32::from_rgb(24, 25, 27),
            lapinux_core::i18n::t("downloads.confirm_cancel"),
        )
    } else {
        (
            HeroGlyph::Close,
            egui::Color32::from_black_alpha(80),
            TEXT(),
            lapinux_core::i18n::t("downloads.cancel"),
        )
    };
    if !hero_glyph_button(ui, rect, job_id, glyph, fill, ink, tooltip) {
        return false;
    }
    if armed {
        ui.ctx()
            .memory_mut(|memory| memory.data.insert_temp(confirm, 0.0_f64));
        true
    } else {
        ui.ctx()
            .memory_mut(|memory| memory.data.insert_temp(confirm, now + 4.0));
        false
    }
}

/// Une ligne de la carte en direct : libellé, débit courant et courbe.
fn paint_rate_row(
    painter: &egui::Painter,
    inner: egui::Rect,
    top: f32,
    label: &str,
    value: f32,
    samples: &[f32],
    color: egui::Color32,
    glyph: HeroGlyph,
) {
    let graph_width = (inner.width() * 0.40).clamp(76.0, 180.0);
    let graph = egui::Rect::from_min_max(
        egui::pos2(inner.right() - graph_width, top + 4.0),
        egui::pos2(inner.right(), top + 40.0),
    );
    paint_hero_glyph(
        painter,
        egui::pos2(inner.left() + 5.0, top + 11.0),
        glyph,
        color,
    );
    painter.text(
        egui::pos2(inner.left() + 16.0, top + 11.0),
        egui::Align2::LEFT_CENTER,
        label,
        display_font(12.5),
        TEXT(),
    );
    painter.text(
        egui::pos2(inner.left() + 16.0, top + 30.0),
        egui::Align2::LEFT_CENTER,
        network_rate(value),
        egui::FontId::proportional(11.5),
        TEXT_DIM(),
    );
    paint_rate_graph(painter, graph, samples, color);
}

/// Courbe d'un débit, normalisée sur son propre maximum, avec un léger halo.
fn paint_rate_graph(
    painter: &egui::Painter,
    rect: egui::Rect,
    values: &[f32],
    color: egui::Color32,
) {
    painter.line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        egui::Stroke::new(1.0_f32, egui::Color32::from_white_alpha(14)),
    );
    if values.len() < 2 {
        return;
    }
    let max = values.iter().copied().fold(0.0_f32, f32::max).max(1024.0);
    let last = (values.len() - 1) as f32;
    let points: Vec<egui::Pos2> = values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let x = rect.left() + rect.width() * index as f32 / last;
            let y = rect.bottom() - (value / max).clamp(0.0, 1.0) * rect.height();
            egui::pos2(x, y)
        })
        .collect();
    // Remplissage dégradé sous la courbe : la ligne se détache du fond sans
    // devenir un simple trait, comme les mini-graphiques d'Epic.
    let mut area = egui::Mesh::default();
    let top_color = with_alpha(color, 0.20);
    let bottom_color = egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 0);
    for (index, point) in points.iter().enumerate() {
        area.colored_vertex(*point, top_color);
        area.colored_vertex(egui::pos2(point.x, rect.bottom()), bottom_color);
        if index > 0 {
            let base = (index as u32 - 1) * 2;
            area.add_triangle(base, base + 1, base + 2);
            area.add_triangle(base + 1, base + 2, base + 3);
        }
    }
    painter.add(egui::Shape::mesh(area));
    painter.add(egui::Shape::line(
        points.clone(),
        egui::Stroke::new(4.0_f32, with_alpha(color, 0.22)),
    ));
    painter.add(egui::Shape::line(points, egui::Stroke::new(1.6_f32, color)));
}

pub(crate) fn paint_hero_glyph(
    painter: &egui::Painter,
    center: egui::Pos2,
    glyph: HeroGlyph,
    color: egui::Color32,
) {
    let stroke = egui::Stroke::new(2.0_f32, color);
    match glyph {
        HeroGlyph::Pause => {
            for dx in [-4.5, 4.5] {
                painter.rect_filled(
                    egui::Rect::from_center_size(
                        egui::pos2(center.x + dx, center.y),
                        egui::vec2(4.0, 13.0),
                    ),
                    1.5,
                    color,
                );
            }
        }
        HeroGlyph::Play => {
            painter.add(egui::Shape::convex_polygon(
                vec![
                    egui::pos2(center.x - 4.0, center.y - 7.0),
                    egui::pos2(center.x + 7.0, center.y),
                    egui::pos2(center.x - 4.0, center.y + 7.0),
                ],
                color,
                egui::Stroke::NONE,
            ));
        }
        HeroGlyph::Close => {
            painter.line_segment(
                [
                    egui::pos2(center.x - 5.0, center.y - 5.0),
                    egui::pos2(center.x + 5.0, center.y + 5.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(center.x + 5.0, center.y - 5.0),
                    egui::pos2(center.x - 5.0, center.y + 5.0),
                ],
                stroke,
            );
        }
        HeroGlyph::Check => {
            painter.line_segment(
                [
                    egui::pos2(center.x - 5.0, center.y),
                    egui::pos2(center.x - 1.0, center.y + 4.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(center.x - 1.0, center.y + 4.0),
                    egui::pos2(center.x + 5.0, center.y - 4.0),
                ],
                stroke,
            );
        }
        HeroGlyph::Down => {
            let direction = 1.0;
            painter.line_segment(
                [
                    egui::pos2(center.x, center.y - 5.0 * direction),
                    egui::pos2(center.x, center.y + 3.0 * direction),
                ],
                egui::Stroke::new(1.6_f32, color),
            );
            painter.line_segment(
                [
                    egui::pos2(center.x - 3.5, center.y + 1.0 * direction),
                    egui::pos2(center.x, center.y + 4.5 * direction),
                ],
                egui::Stroke::new(1.6_f32, color),
            );
            painter.line_segment(
                [
                    egui::pos2(center.x + 3.5, center.y + 1.0 * direction),
                    egui::pos2(center.x, center.y + 4.5 * direction),
                ],
                egui::Stroke::new(1.6_f32, color),
            );
            painter.line_segment(
                [
                    egui::pos2(center.x - 5.0, center.y + 6.5 * direction),
                    egui::pos2(center.x + 5.0, center.y + 6.5 * direction),
                ],
                egui::Stroke::new(1.6_f32, color),
            );
        }
        HeroGlyph::Disk => {
            let body = egui::Rect::from_center_size(center, egui::vec2(11.0, 11.0));
            painter.rect_stroke(body, 2.0_f32, egui::Stroke::new(1.4_f32, color));
            painter.circle_filled(center, 1.4, color);
        }
        HeroGlyph::Document => {
            // Une page et trois lignes : le rapport d'un transfert terminé.
            let page = egui::Rect::from_center_size(center, egui::vec2(9.0, 11.5));
            painter.rect_stroke(page, 1.0_f32, egui::Stroke::new(1.3_f32, color));
            for offset in [-3.0_f32, 0.0, 3.0] {
                painter.line_segment(
                    [
                        egui::pos2(page.left() + 2.2, center.y + offset),
                        egui::pos2(page.right() - 2.2, center.y + offset),
                    ],
                    egui::Stroke::new(1.1_f32, color),
                );
            }
        }
        HeroGlyph::Copy => {
            // Deux feuilles décalées : la copie du relevé de débit.
            let back = egui::Rect::from_min_max(
                egui::pos2(center.x - 3.6, center.y - 6.6),
                egui::pos2(center.x + 6.4, center.y + 2.8),
            );
            let front = egui::Rect::from_min_max(
                egui::pos2(center.x - 6.4, center.y - 2.8),
                egui::pos2(center.x + 3.6, center.y + 6.6),
            );
            painter.rect_stroke(back, 1.6_f32, egui::Stroke::new(1.3_f32, color));
            painter.rect_stroke(front, 1.6_f32, egui::Stroke::new(1.3_f32, color));
        }
        HeroGlyph::Info => {
            // Un cercle et un « i » : l'accès aux détails du transfert.
            painter.circle_stroke(center, 7.0, egui::Stroke::new(1.4_f32, color));
            painter.circle_filled(egui::pos2(center.x, center.y - 3.4), 1.1, color);
            painter.line_segment(
                [
                    egui::pos2(center.x, center.y - 0.6),
                    egui::pos2(center.x, center.y + 4.6),
                ],
                egui::Stroke::new(1.5_f32, color),
            );
        }
        HeroGlyph::Trash => {
            // Couvercle, poignée, corps et deux traits : une poubelle sobre.
            let lid = center.y - 5.0;
            painter.line_segment(
                [
                    egui::pos2(center.x - 6.5, lid),
                    egui::pos2(center.x + 6.5, lid),
                ],
                egui::Stroke::new(1.6_f32, color),
            );
            painter.line_segment(
                [
                    egui::pos2(center.x - 2.2, lid - 2.4),
                    egui::pos2(center.x + 2.2, lid - 2.4),
                ],
                egui::Stroke::new(1.6_f32, color),
            );
            let body = egui::Rect::from_min_max(
                egui::pos2(center.x - 5.0, lid + 2.2),
                egui::pos2(center.x + 5.0, center.y + 7.0),
            );
            painter.rect_stroke(body, 1.5_f32, egui::Stroke::new(1.3_f32, color));
            for dx in [-1.8_f32, 1.8] {
                painter.line_segment(
                    [
                        egui::pos2(center.x + dx, lid + 4.5),
                        egui::pos2(center.x + dx, center.y + 4.5),
                    ],
                    egui::Stroke::new(1.0_f32, color),
                );
            }
        }
    }
}

fn paint_tile_icon(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    name: &str,
    icon: Option<&egui::TextureHandle>,
) {
    let painter = ui.painter();
    painter.rect_filled(
        rect,
        egui::Rounding::same(9.0),
        egui::Color32::from_black_alpha(70),
    );
    match icon {
        Some(icon) => {
            egui::Image::new((icon.id(), rect.size()))
                .rounding(9.0)
                .paint_at(ui, rect);
        }
        None => {
            let initial = name
                .chars()
                .find(|character| character.is_alphanumeric())
                .map(|character| character.to_uppercase().to_string())
                .unwrap_or_else(|| "?".into());
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                initial,
                egui::FontId::proportional(16.0),
                egui::Color32::WHITE,
            );
        }
    }
    ui.painter().rect_stroke(
        rect,
        egui::Rounding::same(9.0),
        egui::Stroke::new(1.0_f32, egui::Color32::from_white_alpha(30)),
    );
}

/// Tuile d'une mise à jour pas encore lancée : poids du téléchargement et
/// bouton de lancement. Dès qu'elle démarre, sa carte vit dans « En cours ».
fn available_update_tile(
    ui: &mut egui::Ui,
    width: f32,
    appid: u32,
    name: &str,
    icon: Option<&egui::TextureHandle>,
    size: Option<u64>,
) -> bool {
    // Même hauteur, même icône et même fond que les cartes de téléchargement
    // actives : une mise à jour n'est pas une autre sorte de contenu.
    const HEIGHT: f32 = 96.0;
    const PAD: f32 = 20.0;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, HEIGHT), egui::Sense::hover());
    let hovered = response.hovered();
    let painter = ui.painter().clone();
    painter.rect_filled(
        rect,
        egui::Rounding::same(0.0),
        if hovered { CARD_HOVER() } else { CARD() },
    );
    painter.rect_filled(
        egui::Rect::from_min_size(rect.min, egui::vec2(3.0, rect.height())),
        egui::Rounding::same(0.0),
        ACCENT(),
    );

    let icon_rect = egui::Rect::from_min_size(
        egui::pos2(rect.left() + PAD, rect.center().y - 18.0),
        egui::vec2(36.0, 36.0),
    );
    paint_tile_icon(ui, icon_rect, name, icon);

    let button_width = 118.0;
    let text_x = icon_rect.right() + 10.0;
    let text_right = rect.right() - button_width - 24.0;
    let text_width = (text_right - text_x).max(70.0);
    painter.text(
        egui::pos2(text_x, icon_rect.center().y - 11.0),
        egui::Align2::LEFT_CENTER,
        lapinux_core::util::ellipsize(name, (text_width / 7.0).max(8.0) as usize),
        display_font(16.5),
        TEXT(),
    );
    painter.text(
        egui::pos2(text_x, icon_rect.center().y + 11.0),
        egui::Align2::LEFT_CENTER,
        rate_label(wording::update_announcement(size), text_width),
        egui::FontId::proportional(10.5),
        TEXT_DIM(),
    );

    let button = egui::Rect::from_min_size(
        egui::pos2(rect.right() - button_width - 16.0, rect.center().y - 15.0),
        egui::vec2(button_width, 30.0),
    );
    let clicked = tile_button(
        ui,
        button,
        lapinux_core::i18n::t("configurations.update"),
        false,
    );
    let _ = response.on_hover_text(lapinux_core::i18n::t_fmt(
        "downloads.build_replaces",
        &[name.to_string(), appid.to_string()],
    ));
    // Même précaution que la carte dossier : `ui.put` recule le curseur, la
    // tuile suivante de la grille pourrait sinon se poser par-dessus.
    ui.advance_cursor_after_rect(rect);
    clicked
}

/// Tronque une ligne secondaire à la largeur disponible, sans dépasser le
/// bouton d'action.
fn rate_label(text: String, width: f32) -> String {
    lapinux_core::util::ellipsize(&text, (width / 6.0).max(8.0) as usize)
}

/// Illustration de la carte large et les couleurs de sa barre de progression.
#[derive(Clone)]
struct CoverArt {
    /// Bannière réduite puis floutée : elle sert de fond plein cadre.
    blurred: egui::TextureHandle,
    /// Couleurs moyennes lues de gauche à droite dans l'image.
    palette: Vec<egui::Color32>,
}

/// Icône des tuiles de téléchargement, gardée en mémoire.
#[derive(Default)]
struct DownloadArtCache {
    icons: HashMap<u32, egui::TextureHandle>,
    icon_colors: HashMap<u32, egui::Color32>,
    icon_tries: HashMap<u32, Instant>,
    /// Illustrations larges pour la carte de téléchargement (capsule, héros).
    covers: HashMap<u32, CoverArt>,
    cover_tries: HashMap<u32, Instant>,
}

impl DownloadArtCache {
    fn icon(
        &mut self,
        ctx: &egui::Context,
        paths: &LapinuxPaths,
        appid: u32,
    ) -> Option<egui::TextureHandle> {
        if let Some(texture) = self.icons.get(&appid) {
            return Some(texture.clone());
        }
        let now = Instant::now();
        if self
            .icon_tries
            .get(&appid)
            .map(|last| now.duration_since(*last) < Duration::from_secs(2))
            .unwrap_or(false)
        {
            return None;
        }
        self.icon_tries.insert(appid, now);
        let (texture, color) = download_icon_texture(ctx, paths, appid)?;
        self.icon_colors.insert(appid, color);
        self.icons.insert(appid, texture.clone());
        Some(texture)
    }

    /// Couleur dominante de l'icône, calculée au chargement de celle-ci.
    fn icon_color(&self, appid: u32) -> Option<egui::Color32> {
        self.icon_colors.get(&appid).copied()
    }

    /// Couleur dominante du jeu, pour ses barres de progression. L'icône est
    /// chargée au besoin, comme pour la pastille de l'onglet.
    fn accent(&mut self, ctx: &egui::Context, paths: &LapinuxPaths, appid: u32) -> egui::Color32 {
        if let Some(color) = self.icon_color(appid) {
            return color;
        }
        self.icon(ctx, paths, appid);
        self.icon_color(appid).unwrap_or(ACCENT())
    }

    /// Illustration large de la carte (capsule Steam ou héros), chargée une
    /// fois puis gardée en texture.
    fn cover(&mut self, ctx: &egui::Context, paths: &LapinuxPaths, appid: u32) -> Option<CoverArt> {
        if let Some(art) = self.covers.get(&appid) {
            return Some(art.clone());
        }
        let now = Instant::now();
        if self
            .cover_tries
            .get(&appid)
            .map(|last| now.duration_since(*last) < Duration::from_secs(2))
            .unwrap_or(false)
        {
            return None;
        }
        self.cover_tries.insert(appid, now);
        let art = download_cover_art(ctx, paths, appid)?;
        self.covers.insert(appid, art.clone());
        Some(art)
    }
}

/// Plusieurs sources, de la plus large à la plus modeste : l'image de
/// bibliothèque Steam est déjà en cache pour un jeu installé ; le cache
/// d'illustrations du client fournit la capsule sinon.
fn download_cover_art(ctx: &egui::Context, paths: &LapinuxPaths, appid: u32) -> Option<CoverArt> {
    let dir = paths.art_dir();
    // Bannière large d'abord : les capsules portrait ne servent qu'en dernier
    // recours, sinon le fond de la carte serait un portrait rogné.
    let mut candidates = vec![
        dir.join(format!("steam-hero-{appid}.jpg")),
        dir.join(format!("sgdb-hero-{appid}.jpg")),
    ];
    let files = steam_librarycache_files(appid);
    for name in ["library_hero.jpg", "library_header.jpg"] {
        if let Some(path) = file_named(&files, name) {
            candidates.push(path.clone());
        }
    }
    candidates.push(dir.join(format!("steam-capsule-{appid}.jpg")));
    candidates.push(dir.join(format!("sgdb-capsule-{appid}.jpg")));
    // Jaquette publiée par une boutique : un jeu Epic, GOG ou itch n'a pas de
    // capsule Steam, et sa carte de téléchargement restait sans image.
    candidates.push(paths.covers_dir().join(format!("store-{appid}.jpg")));
    candidates.push(paths.covers_dir().join(format!("{appid}.jpg")));
    candidates.push(paths.covers_dir().join(format!("sgdb-{appid}.jpg")));
    for name in ["library_capsule.jpg", "header.jpg"] {
        if let Some(path) = file_named(&files, name) {
            candidates.push(path.clone());
        }
    }
    candidates.iter().find_map(|path| {
        let bytes = std::fs::read(path).ok()?;
        let image = image::load_from_memory(&bytes).ok()?;
        let palette = cover_palette(&image);
        // Une bannière 4K floutée coûterait cher à chaque ouverture : on la
        // réduit d'abord à la taille d'une vignette, puis on lisse. Le fond
        // est étiré par le GPU avec un filtrage linéaire.
        let small = image
            .resize(420, 236, image::imageops::FilterType::Triangle)
            .to_rgba8();
        let blurred = image::imageops::blur(&small, 9.0);
        let blurred = texture_from_image(
            ctx,
            image::DynamicImage::ImageRgba8(blurred),
            &format!("download-banner-blur-{appid}"),
        );
        Some(CoverArt { blurred, palette })
    })
}

/// Couleur moyenne de chaque bande verticale de l'illustration. L'image est
/// lue de gauche à droite : la barre de progression reprend ces teintes, dans
/// le même ordre, à mesure qu'elle avance.
fn cover_palette(image: &image::DynamicImage) -> Vec<egui::Color32> {
    const SLICES: usize = 16;
    let rgba = image.to_rgba8();
    let (width, height) = (rgba.width().max(1), rgba.height().max(1));
    // La bande centrale évite les bords noirs des illustrations panoramiques.
    let y0 = height / 4;
    let y1 = (height * 3 / 4).max(y0 + 1);
    let mut palette = Vec::with_capacity(SLICES);
    for slice in 0..SLICES {
        let x0 = width * slice as u32 / SLICES as u32;
        let x1 = (width * (slice + 1) as u32 / SLICES as u32).max(x0 + 1);
        let mut sum = [0u64; 3];
        let mut count = 0u64;
        for row in 0..8u32 {
            let y = y0 + (y1 - y0) * row / 8;
            for column in 0..6u32 {
                let x = x0 + (x1 - x0) * column / 6;
                let pixel = rgba.get_pixel(x.min(width - 1), y.min(height - 1));
                if pixel[3] < 32 {
                    continue;
                }
                sum[0] += pixel[0] as u64;
                sum[1] += pixel[1] as u64;
                sum[2] += pixel[2] as u64;
                count += 1;
            }
        }
        if count == 0 {
            continue;
        }
        let raw = egui::Color32::from_rgb(
            (sum[0] / count) as u8,
            (sum[1] / count) as u8,
            (sum[2] / count) as u8,
        );
        palette.push(vivid(raw));
    }
    palette
}

/// Éclaircit les teintes trop sombres et ravive les couleurs : sur une barre
/// de six pixels, une moyenne brute vire vite au gris indistinct.
fn vivid(color: egui::Color32) -> egui::Color32 {
    let r = color.r() as f32 / 255.0;
    let g = color.g() as f32 / 255.0;
    let b = color.b() as f32 / 255.0;
    let luminance = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let target = luminance.max(0.36);
    let mix = |channel: f32| {
        ((target + (channel - luminance) * 1.35).clamp(0.0, 1.0) * 255.0).round() as u8
    };
    egui::Color32::from_rgb(mix(r), mix(g), mix(b))
}

fn download_icon_texture(
    ctx: &egui::Context,
    paths: &LapinuxPaths,
    appid: u32,
) -> Option<(egui::TextureHandle, egui::Color32)> {
    let dir = paths.art_dir();
    let mut candidates = vec![
        dir.join(format!("steam-icon-{appid}.jpg")),
        dir.join(format!("sgdb-icon-{appid}.jpg")),
        dir.join(format!("icon-{appid}.jpg")),
    ];
    let files = steam_librarycache_files(appid);
    if let Some(path) = file_named(&files, "icon.jpg") {
        candidates.push(path.clone());
    }
    if let Some(path) = files.iter().find(|path| is_icon_file(path)) {
        candidates.push(path.clone());
    }
    // Dernier recours pour un jeu de boutique : sa jaquette, rognée au carré.
    let covers = paths.covers_dir();
    for name in ["store", "", "sgdb-"] {
        candidates.push(covers.join(if name.is_empty() {
            format!("{appid}.jpg")
        } else {
            format!("{name}-{appid}.jpg")
        }));
    }
    candidates.iter().find_map(|path| {
        let bytes = std::fs::read(path).ok()?;
        let image = image::load_from_memory(&bytes).ok()?;
        let color = dominant_icon_color(&image);
        // Une jaquette de boutique est en 2:3 : sans rognage, la vignette
        // carrée de l'onglet l'écraserait.
        let image = square_icon(image);
        let texture = texture_from_image(ctx, image, &format!("download-icon-{appid}"));
        Some((texture, color))
    })
}

/// Rognage central au carré : une pochette portrait devient une vignette
/// d'icône sans déformation.
fn square_icon(image: image::DynamicImage) -> image::DynamicImage {
    let (width, height) = (image.width(), image.height());
    if width == 0 || height == 0 || width == height {
        return image;
    }
    let side = width.min(height);
    let x = (width - side) / 2;
    let y = (height - side) / 2;
    image.crop_imm(x, y, side, side)
}

/// Couleur dominante d'une icône, pour le halo de l'onglet : on regroupe les
/// teintes proches et on préfère une couleur saturée à un gris majoritaire.
fn dominant_icon_color(image: &image::DynamicImage) -> egui::Color32 {
    let rgba = image.to_rgba8();
    let mut buckets: HashMap<u16, (u64, u64, [u64; 3])> = HashMap::new();
    let mut fallback = (0u64, [0u64; 3]);
    for pixel in rgba.pixels() {
        if pixel[3] < 64 {
            continue;
        }
        let (r, g, b) = (pixel[0], pixel[1], pixel[2]);
        fallback.0 += 1;
        fallback.1[0] += r as u64;
        fallback.1[1] += g as u64;
        fallback.1[2] += b as u64;
        let max = r.max(g).max(b) as f32;
        let min = r.min(g).min(b) as f32;
        let luminosity = 0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32;
        if luminosity < 12.0 || luminosity > 246.0 {
            continue;
        }
        let saturation = if max > 0.0 { (max - min) / max } else { 0.0 };
        let key = (((r >> 4) as u16) << 8) | (((g >> 4) as u16) << 4) | (b >> 4) as u16;
        let entry = buckets.entry(key).or_insert((0, 0, [0; 3]));
        // Une teinte vive pèse plus qu'un grand aplat gris.
        entry.0 += 1 + (saturation * 4.0) as u64;
        entry.1 += 1;
        entry.2[0] += r as u64;
        entry.2[1] += g as u64;
        entry.2[2] += b as u64;
    }
    let Some((_, count, sum)) = buckets
        .values()
        .max_by_key(|(weight, _, _)| *weight)
        .filter(|(weight, _, _)| *weight > 0)
    else {
        if fallback.0 == 0 {
            return ACCENT();
        }
        let average = egui::Color32::from_rgb(
            (fallback.1[0] / fallback.0) as u8,
            (fallback.1[1] / fallback.0) as u8,
            (fallback.1[2] / fallback.0) as u8,
        );
        let chroma = average.r().max(average.g()).max(average.b()) as i16
            - average.r().min(average.g()).min(average.b()) as i16;
        return if chroma < 16 {
            ACCENT()
        } else {
            vivid(average)
        };
    };
    let count = (*count).max(1);
    let color = egui::Color32::from_rgb(
        (sum[0] / count) as u8,
        (sum[1] / count) as u8,
        (sum[2] / count) as u8,
    );
    let chroma = color.r().max(color.g()).max(color.b()) as f32
        - color.r().min(color.g()).min(color.b()) as f32;
    if chroma < 16.0 {
        ACCENT()
    } else {
        vivid(color)
    }
}

pub(crate) fn lerp_color(from: egui::Color32, to: egui::Color32, t: f32) -> egui::Color32 {
    let t = t.clamp(0.0, 1.0);
    let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t) as u8;
    egui::Color32::from_rgb(
        mix(from.r(), to.r()),
        mix(from.g(), to.g()),
        mix(from.b(), to.b()),
    )
}

/// Points d'un contour arrondi, dans le sens horaire depuis le coin
/// supérieur gauche. Les bords sont découpés en segments courts pour que le
/// dégradé reste lisse même sur une carte large.
fn rounded_outline(rect: egui::Rect, radius: f32) -> Vec<egui::Pos2> {
    use std::f32::consts::{FRAC_PI_2, PI};
    const EDGE_STEP: f32 = 7.0;
    const CORNER_STEPS: usize = 10;

    fn edge(points: &mut Vec<egui::Pos2>, from: egui::Pos2, to: egui::Pos2) {
        let steps = (from.distance(to) / EDGE_STEP).ceil().max(1.0) as usize;
        for index in 0..steps {
            points.push(from + (to - from) * (index as f32 / steps as f32));
        }
    }

    fn corner(points: &mut Vec<egui::Pos2>, center: egui::Pos2, start: f32, radius: f32) {
        for index in 0..=CORNER_STEPS {
            let angle = start + FRAC_PI_2 * index as f32 / CORNER_STEPS as f32;
            points.push(center + egui::vec2(angle.cos(), angle.sin()) * radius);
        }
    }

    let radius = radius.clamp(0.0, rect.width().min(rect.height()) / 2.0);
    let mut points = Vec::new();
    // Haut, puis dans le sens horaire : droite, bas, gauche.
    edge(
        &mut points,
        egui::pos2(rect.left() + radius, rect.top()),
        egui::pos2(rect.right() - radius, rect.top()),
    );
    corner(
        &mut points,
        egui::pos2(rect.right() - radius, rect.top() + radius),
        -FRAC_PI_2,
        radius,
    );
    edge(
        &mut points,
        egui::pos2(rect.right(), rect.top() + radius),
        egui::pos2(rect.right(), rect.bottom() - radius),
    );
    corner(
        &mut points,
        egui::pos2(rect.right() - radius, rect.bottom() - radius),
        0.0,
        radius,
    );
    edge(
        &mut points,
        egui::pos2(rect.right() - radius, rect.bottom()),
        egui::pos2(rect.left() + radius, rect.bottom()),
    );
    corner(
        &mut points,
        egui::pos2(rect.left() + radius, rect.bottom() - radius),
        FRAC_PI_2,
        radius,
    );
    edge(
        &mut points,
        egui::pos2(rect.left(), rect.bottom() - radius),
        egui::pos2(rect.left(), rect.top() + radius),
    );
    corner(
        &mut points,
        egui::pos2(rect.left() + radius, rect.top() + radius),
        PI,
        radius,
    );
    points
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum SpeedtestStage {
    #[default]
    Idle,
    Ping,
    Download,
    Upload,
}

#[derive(Default)]
struct Speedtest {
    rx: Option<mpsc::Receiver<SpeedtestEvent>>,
    running: bool,
    stage: SpeedtestStage,
    /// Latence moyenne en millisecondes.
    ping_ms: Option<f32>,
    /// Débits en octets par seconde, convertis en Mbit/s à l'affichage.
    down: Option<f32>,
    up: Option<f32>,
    /// Plus bas et plus haut débits instantanés du volet, hors montée en
    /// charge TCP. En octets par seconde, comme `down` et `up`.
    down_low: Option<f32>,
    down_high: Option<f32>,
    up_low: Option<f32>,
    up_high: Option<f32>,
    /// Durée totale du test, une fois terminé.
    elapsed: Option<f32>,
    /// Erreur partielle (un seul volet indisponible) ou fatale.
    note: Option<String>,
}

enum SpeedtestEvent {
    Stage(SpeedtestStage),
    Ping(f32),
    Down(f32),
    Up(f32),
    DownRange(f32, f32),
    UpRange(f32, f32),
    Elapsed(f32),
    Warning(String),
    Failed(String),
    Done,
}

/// Endpoint public de mesure Cloudflare, sans clé ni compte.
const SPEEDTEST_BASE: &str = "https://speed.cloudflare.com";
/// 50 Mo : assez long pour dépasser le démarrage lent de TCP sur les liens
/// rapides, assez court pour rester sous quelques secondes à 100 Mbit/s.
const SPEEDTEST_DOWN_BYTES: u64 = 50_000_000;
const SPEEDTEST_UP_BYTES: u64 = 15_000_000;
const SPEEDTEST_DOWN_LIMIT: Duration = Duration::from_secs(8);

fn speedtest_send(tx: &mpsc::Sender<SpeedtestEvent>, event: SpeedtestEvent) {
    let _ = tx.send(event);
}

/// Corps d'upload généré à la volée : chaque lecture alimente le compteur et
/// publie un débit instantané, sans buffer de 15 Mo en mémoire.
struct UploadBody {
    remaining: u64,
    sent: Arc<AtomicU64>,
    started: Instant,
    window_started: Instant,
    window_bytes: u64,
    low: f32,
    high: f32,
    tx: mpsc::Sender<SpeedtestEvent>,
}

impl Read for UploadBody {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.remaining == 0 || buffer.is_empty() {
            return Ok(0);
        }
        let count = buffer.len().min(self.remaining as usize).min(64 * 1024);
        buffer[..count].fill(0);
        self.remaining -= count as u64;
        self.window_bytes += count as u64;
        self.sent.fetch_add(count as u64, Ordering::Relaxed);
        let now = Instant::now();
        let elapsed = now.duration_since(self.window_started);
        if elapsed >= Duration::from_millis(200) {
            let rate = self.window_bytes as f32 / elapsed.as_secs_f32().max(0.001);
            speedtest_send(&self.tx, SpeedtestEvent::Up(rate));
            // Les premières fenêtres montent en charge : les inclure
            // donnerait un « plus bas » artificiellement faible.
            if now.duration_since(self.started) >= Duration::from_millis(600) {
                self.low = self.low.min(rate);
                self.high = self.high.max(rate);
                let low = self.low.min(self.high);
                speedtest_send(&self.tx, SpeedtestEvent::UpRange(low, self.high));
            }
            self.window_started = now;
            self.window_bytes = 0;
        }
        Ok(count)
    }
}

/// Ping, descendant puis ascendant. Chaque mesure part vers l'interface dès
/// qu'elle est connue, d'où les valeurs qui montent pendant le test.
fn speedtest_run(tx: mpsc::Sender<SpeedtestEvent>) {
    let test_started = Instant::now();
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(30))
        .build();
    // Poignée de main TLS mise de côté avant de mesurer le ping.
    let _ = agent
        .get(&format!("{SPEEDTEST_BASE}/__down?bytes=1"))
        .timeout(Duration::from_secs(5))
        .call()
        .map(|response| response.into_string());

    speedtest_send(&tx, SpeedtestEvent::Stage(SpeedtestStage::Ping));
    // Aucune réponse à l'un des trois volets : le test entier a échoué, ce
    // n'est pas un avertissement partiel.
    let mut reachable = false;
    let mut pings = Vec::new();
    for _ in 0..3 {
        let started = Instant::now();
        match agent
            .get(&format!("{SPEEDTEST_BASE}/__down?bytes=1"))
            .timeout(Duration::from_secs(5))
            .call()
        {
            Ok(response) => {
                reachable = true;
                let elapsed = started.elapsed().as_secs_f32() * 1000.0;
                let _ = response.into_string();
                pings.push(elapsed);
                speedtest_send(&tx, SpeedtestEvent::Ping(elapsed));
            }
            Err(error) => speedtest_send(&tx, SpeedtestEvent::Warning(format!("ping : {error}"))),
        }
    }
    if !pings.is_empty() {
        let average = pings.iter().sum::<f32>() / pings.len() as f32;
        speedtest_send(&tx, SpeedtestEvent::Ping(average));
    }

    speedtest_send(&tx, SpeedtestEvent::Stage(SpeedtestStage::Download));
    let url = format!("{SPEEDTEST_BASE}/__down?bytes={SPEEDTEST_DOWN_BYTES}");
    match agent.get(&url).timeout(Duration::from_secs(15)).call() {
        Ok(response) => {
            reachable = true;
            let mut reader = response.into_reader();
            let mut buffer = vec![0u8; 64 * 1024];
            let started = Instant::now();
            let mut window_started = started;
            let mut window_bytes = 0u64;
            let mut total = 0u64;
            let mut low = f32::MAX;
            let mut high = 0.0f32;
            // Démarrage lent de TCP : la montée en charge des premières
            // centaines de millisecondes ne doit pas tirer la moyenne vers le
            // bas, sinon le chiffre reste très en dessous du vrai débit.
            let mut warm: Option<(Instant, u64)> = None;
            loop {
                if started.elapsed() >= SPEEDTEST_DOWN_LIMIT {
                    break;
                }
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(read) => {
                        total += read as u64;
                        window_bytes += read as u64;
                        let now = Instant::now();
                        if warm.is_none()
                            && now.duration_since(started) >= Duration::from_millis(700)
                        {
                            warm = Some((now, total));
                        }
                        let elapsed = now.duration_since(window_started);
                        if elapsed >= Duration::from_millis(200) {
                            let rate = window_bytes as f32 / elapsed.as_secs_f32().max(0.001);
                            speedtest_send(&tx, SpeedtestEvent::Down(rate));
                            if warm.is_some() {
                                low = low.min(rate);
                                high = high.max(rate);
                                speedtest_send(&tx, SpeedtestEvent::DownRange(low, high));
                            }
                            window_started = now;
                            window_bytes = 0;
                        }
                    }
                    Err(error) => {
                        speedtest_send(
                            &tx,
                            SpeedtestEvent::Warning(format!("descendant : {error}")),
                        );
                        break;
                    }
                }
            }
            if total > 0 {
                let (from, already) = warm.unwrap_or((started, 0));
                let elapsed = from.elapsed().as_secs_f32().max(0.001);
                let measured = total.saturating_sub(already) as f32;
                let rate = if measured > 0.0 {
                    measured / elapsed
                } else {
                    total as f32 / started.elapsed().as_secs_f32().max(0.001)
                };
                speedtest_send(&tx, SpeedtestEvent::Down(rate));
            }
        }
        Err(error) => speedtest_send(
            &tx,
            SpeedtestEvent::Warning(format!("descendant : {error}")),
        ),
    }

    speedtest_send(&tx, SpeedtestEvent::Stage(SpeedtestStage::Upload));
    let sent = Arc::new(AtomicU64::new(0));
    let body = UploadBody {
        remaining: SPEEDTEST_UP_BYTES,
        sent: Arc::clone(&sent),
        started: Instant::now(),
        window_started: Instant::now(),
        window_bytes: 0,
        low: f32::MAX,
        high: 0.0,
        tx: tx.clone(),
    };
    let started = Instant::now();
    let result = agent
        .post(&format!("{SPEEDTEST_BASE}/__up"))
        .set("Content-Type", "application/octet-stream")
        .set("Content-Length", &SPEEDTEST_UP_BYTES.to_string())
        .timeout(Duration::from_secs(25))
        .send(body);
    let sent_bytes = sent.load(Ordering::Relaxed);
    reachable |= result.is_ok();
    match result {
        Ok(_) if sent_bytes > 0 => {
            let elapsed = started.elapsed().as_secs_f32().max(0.001);
            speedtest_send(&tx, SpeedtestEvent::Up(sent_bytes as f32 / elapsed));
        }
        Ok(_) => speedtest_send(
            &tx,
            SpeedtestEvent::Warning(lapinux_core::i18n::t("downloads.upstream_none").into()),
        ),
        Err(error) => speedtest_send(&tx, SpeedtestEvent::Warning(format!("ascendant : {error}"))),
    }

    speedtest_send(
        &tx,
        SpeedtestEvent::Elapsed(test_started.elapsed().as_secs_f32()),
    );
    if !reachable {
        speedtest_send(
            &tx,
            SpeedtestEvent::Failed(lapinux_core::i18n::t("downloads.no_speedtest_server").into()),
        );
    }
    speedtest_send(&tx, SpeedtestEvent::Done);
}

fn speedtest_row(
    ui: &mut egui::Ui,
    label: &str,
    value: Option<String>,
    unit: &str,
    active: bool,
    range: Option<(f32, f32)>,
) {
    // Une seule ligne, de hauteur fixe : les extrêmes se placent à gauche du
    // chiffre au lieu d'ajouter une ligne sous lui. La carte garde donc la
    // même taille avant, pendant et après un test.
    const HEIGHT: f32 = 18.0;
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, HEIGHT), egui::Sense::hover());
    let painter = ui.painter();
    let cy = rect.center().y;
    let label_galley = painter.layout_no_wrap(
        label.to_string(),
        egui::FontId::proportional(10.5),
        TEXT_DIM(),
    );
    painter.galley(
        egui::pos2(rect.left(), cy - label_galley.size().y / 2.0),
        label_galley.clone(),
        TEXT_DIM(),
    );

    let mut right = rect.right();
    if value.is_some() {
        let unit_galley = painter.layout_no_wrap(
            unit.to_string(),
            egui::FontId::proportional(9.5),
            TEXT_DIM(),
        );
        let x = right - unit_galley.size().x;
        painter.galley(
            egui::pos2(x, cy - unit_galley.size().y / 2.0),
            unit_galley.clone(),
            TEXT_DIM(),
        );
        right = x - 4.0;
    }
    let (text, color) = match &value {
        Some(text) => (text.clone(), if active { OK } else { TEXT() }),
        None => ("--".to_string(), TEXT_DIM()),
    };
    let value_galley = painter.layout_no_wrap(text, egui::FontId::proportional(12.5), color);
    let value_x = (right - value_galley.size().x).max(rect.left());
    painter.galley(
        egui::pos2(value_x, cy - value_galley.size().y / 2.0),
        value_galley.clone(),
        color,
    );

    if let Some((low, high)) = range {
        // Le minimum à gauche du maximum, en rouge et en vert : les deux
        // repères se lisent sans chercher lequel est lequel. Le maximum est
        // conservé seul quand la carte est trop étroite pour les deux.
        let font = egui::FontId::proportional(9.5);
        let (min_text, max_text) = speedtest_range_labels(low, high);
        let min_galley = painter.layout_no_wrap(min_text, font.clone(), DANGER);
        let gap_galley = painter.layout_no_wrap(" · ".to_string(), font.clone(), TEXT_DIM());
        let max_galley = painter.layout_no_wrap(max_text, font, OK);
        let together = min_galley.size().x + gap_galley.size().x + max_galley.size().x;
        let left_limit = rect.left() + label_galley.size().x + 8.0;
        let available = value_x - 10.0 - left_limit;
        if available > max_galley.size().x + 6.0 {
            let show_min = available > together;
            let start = if show_min {
                value_x - 10.0 - together
            } else {
                value_x - 10.0 - max_galley.size().x
            };
            let mut x = start;
            if show_min {
                painter.galley(
                    egui::pos2(x, cy - min_galley.size().y / 2.0),
                    min_galley.clone(),
                    DANGER,
                );
                x += min_galley.size().x;
                painter.galley(
                    egui::pos2(x, cy - gap_galley.size().y / 2.0),
                    gap_galley.clone(),
                    TEXT_DIM(),
                );
                x += gap_galley.size().x;
            }
            painter.galley(
                egui::pos2(x, cy - max_galley.size().y / 2.0),
                max_galley,
                OK,
            );
        }
        let _ = response.on_hover_text(lapinux_core::i18n::t("downloads.rate_hint"));
    }
}

/// « min 488,0 » et « max 612,4 » : les deux repères colorés de la ligne.
/// L'unité est déjà affichée à droite du chiffre courant.
fn speedtest_range_labels(low: f32, high: f32) -> (String, String) {
    (
        format!(
            "min {}",
            format_mbit(Some(low.max(0.0))).unwrap_or_default()
        ),
        format!(
            "max {}",
            format_mbit(Some(high.max(0.0))).unwrap_or_default()
        ),
    )
}

/// Relevé du test, prêt à coller dans un message : une seule ligne, unités et
/// extrêmes compris.
fn speedtest_summary(
    ping: Option<f32>,
    down: Option<f32>,
    down_range: Option<(f32, f32)>,
    up: Option<f32>,
    up_range: Option<(f32, f32)>,
    elapsed: Option<f32>,
) -> String {
    let mbit = |value: Option<f32>| format_mbit(value).unwrap_or_else(|| "--".to_string());
    let mut parts: Vec<String> = Vec::new();
    if let Some(ping) = ping {
        parts.push(format!("Ping {ping:.0} ms"));
    }
    let mut volet = |label: &str, value: Option<f32>, range: Option<(f32, f32)>| {
        let mut text = format!("{label} {} Mb/s", mbit(value));
        if let Some((low, high)) = range {
            text.push_str(&format!(
                " (min {} · max {})",
                mbit(Some(low.max(0.0))),
                mbit(Some(high.max(0.0)))
            ));
        }
        parts.push(text);
    };
    volet(lapinux_core::i18n::t("downloads.down"), down, down_range);
    volet(lapinux_core::i18n::t("downloads.up"), up, up_range);
    if let Some(seconds) = elapsed {
        parts.push(lapinux_core::i18n::t_args(
            "downloads.duration",
            &[("duration", &format_seconds(seconds))],
        ));
    }
    lapinux_core::i18n::t_args(
        "downloads.speedtest_report",
        &[("results", &parts.join(" · "))],
    )
}

/// Durée du test avec une décimale, virgule française.
fn format_seconds(seconds: f32) -> String {
    lapinux_core::i18n::decimal(format!("{:.1}", seconds.max(0.0))) + " s"
}

/// Octets par seconde → Mbit/s : l'unité qu'affichent les tests de débit grand
/// public (Google, Speedtest) et désormais tous les débits de l'application.
fn format_mbit(bytes_per_sec: Option<f32>) -> Option<String> {
    bytes_per_sec.map(|v| lapinux_core::i18n::decimal(format!("{:.1}", v * 8.0 / 1_000_000.0)))
}

/// Débit prêt à afficher, dans l'unité unique de la page : « 23,4 Mb/s ».
/// Les cases, les cartes et le graphique passent tous par ici, pour qu'aucun
/// débit ne réapparaisse en Mo/s à côté d'un autre en Mb/s.
fn network_rate(bytes_per_sec: f32) -> String {
    format!(
        "{} Mb/s",
        format_mbit(Some(bytes_per_sec.max(0.0))).unwrap_or_default()
    )
}

/// Débit d'un rapport : un tiret quand aucune mesure n'existe (tâche annulée
/// avant le premier octet).
fn report_rate(bytes_per_sec: u64) -> String {
    if bytes_per_sec == 0 {
        "—".to_string()
    } else {
        network_rate(bytes_per_sec as f32)
    }
}

fn report_metric(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 1.0;
        ui.label(egui::RichText::new(label).size(9.5).color(TEXT_DIM()));
        ui.label(egui::RichText::new(value).size(12.5).strong().color(TEXT()));
    });
}

/// Rapport d'une tâche terminée. Les téléchargements antérieurs au suivi des
/// débits ont un rapport partiel : durée, volumes et moyenne calculée sur le
/// compteur réseau, mais pas d'extrêmes mesurés.
fn report_for(job: &Job) -> Option<JobReport> {
    if let Some(report) = &job.report {
        return Some(report.clone());
    }
    if job.state != JobState::Done {
        return None;
    }
    let finished = job.finished?;
    let elapsed = finished.saturating_sub(job.started).max(0) as u64;
    let network = if job.network > 0 {
        job.network
    } else {
        job.downloaded
    };
    let average = if elapsed > 0 { network / elapsed } else { 0 };
    Some(JobReport {
        elapsed_secs: elapsed,
        active_secs: elapsed,
        average_bps: average,
        peak_bps: 0,
        low_bps: 0,
        network,
        written: job.written,
        samples: 0,
    })
}

/// Source lisible d'une tâche : les téléchargements Steam passent par le
/// moteur natif Lapinux, mais l'utilisateur veut voir « Steam ».
fn report_source(job: &Job) -> &str {
    if job.kind == JobKind::InstallGame {
        "Steam"
    } else {
        job.backend.as_str()
    }
}

/// Les chiffres communs à la carte du dernier rapport et à la fenêtre de
/// l'historique, dans l'ordre d'affichage.
fn report_metrics(job: &Job, report: &JobReport) -> Vec<(&'static str, String)> {
    let mut metrics: Vec<(&'static str, String)> = vec![
        (
            lapinux_core::i18n::t("downloads.duration"),
            lapinux_core::util::format_duration(report.elapsed_secs),
        ),
        (
            lapinux_core::i18n::t("downloads.active_time"),
            lapinux_core::util::format_duration(report.active_secs),
        ),
        (
            lapinux_core::i18n::t("downloads.average"),
            report_rate(report.average_bps),
        ),
        (
            lapinux_core::i18n::t("downloads.peak"),
            report_rate(report.peak_bps),
        ),
        (
            lapinux_core::i18n::t("downloads.low"),
            report_rate(report.low_bps),
        ),
        (
            lapinux_core::i18n::t("downloads.network"),
            lapinux_core::util::format_bytes(report.network),
        ),
        (
            lapinux_core::i18n::t("downloads.written"),
            lapinux_core::util::format_bytes(report.written),
        ),
    ];
    let reused = job.downloaded.saturating_sub(report.network);
    if reused >= 1024 * 1024 {
        metrics.push((
            lapinux_core::i18n::t("downloads.reused"),
            lapinux_core::util::format_bytes(reused),
        ));
    } else {
        metrics.push(("Source", report_source(job).to_string()));
    }
    metrics
}

/// Contenu de la fenêtre ouverte depuis l'historique. Renvoie vrai si
/// l'utilisateur a cliqué sur « Fermer ».
fn report_window_body(
    ui: &mut egui::Ui,
    job: &Job,
    report: &JobReport,
    icon: Option<&egui::TextureHandle>,
) -> bool {
    let title = if job.title.trim().is_empty() {
        sentence(&job.label)
    } else {
        job.title.clone()
    };
    let source = report_source(job);
    let mut close = false;
    ui.horizontal(|ui| {
        let icon_rect = egui::Rect::from_min_size(ui.cursor().min, egui::vec2(40.0, 40.0));
        ui.allocate_rect(icon_rect, egui::Sense::hover());
        ui.painter().rect_filled(
            icon_rect,
            egui::Rounding::same(8.0),
            egui::Color32::from_black_alpha(70),
        );
        match icon {
            Some(icon) => {
                egui::Image::new((icon.id(), icon_rect.size()))
                    .rounding(egui::Rounding::same(8.0))
                    .paint_at(ui, icon_rect);
            }
            None => {
                ui.painter().text(
                    icon_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    title
                        .chars()
                        .find(|character| character.is_alphanumeric())
                        .map(|character| character.to_uppercase().to_string())
                        .unwrap_or_else(|| "?".into()),
                    display_font(18.0),
                    TEXT_DIM(),
                );
            }
        }
        ui.add_space(10.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            ui.label(
                egui::RichText::new(lapinux_core::i18n::t("downloads.report_title"))
                    .size(10.0)
                    .color(TEXT_DIM()),
            );
            ui.label(egui::RichText::new(&title).size(16.0).strong());
            muted(
                ui,
                lapinux_core::i18n::t_fmt(
                    "downloads.finished_two",
                    &[job_when(job).to_string(), source.to_string()],
                ),
            );
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if link_text(ui, lapinux_core::i18n::t("downloads.close")).clicked() {
                close = true;
            }
        });
    });
    ui.add_space(8.0);
    ui.separator();
    ui.add_space(8.0);
    egui::Grid::new(("download-report-window-grid", job.id.as_str()))
        .num_columns(4)
        .spacing([22.0, 9.0])
        .show(ui, |ui| {
            for (index, (label, value)) in report_metrics(job, report).iter().enumerate() {
                report_metric(ui, label, value);
                if (index + 1) % 4 == 0 {
                    ui.end_row();
                }
            }
        });
    ui.add_space(7.0);
    muted(
        ui,
        if report.samples == 0 {
            lapinux_core::i18n::t("downloads.report_partial")
        } else {
            lapinux_core::i18n::t("downloads.rates_hint")
        },
    );
    close
}

/// Contenu du glossaire des serveurs. Renvoie vrai si l'utilisateur a cliqué
/// sur « Fermer ».
fn servers_help_body(ui: &mut egui::Ui) -> bool {
    let mut close = false;
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            ui.label(
                egui::RichText::new(lapinux_core::i18n::t("downloads.servers"))
                    .size(10.0)
                    .color(TEXT_DIM()),
            );
            ui.label(
                egui::RichText::new(lapinux_core::i18n::t("downloads.choices_help"))
                    .size(16.0)
                    .strong(),
            );
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if link_text(ui, lapinux_core::i18n::t("downloads.close")).clicked() {
                close = true;
            }
        });
    });
    ui.add_space(8.0);
    ui.separator();
    ui.add_space(8.0);
    for (term, text) in [
        (
            lapinux_core::i18n::t("downloads.automatic"),
            lapinux_core::i18n::t("downloads.server_choice"),
        ),
        ("Cache", lapinux_core::i18n::t("downloads.cache_explainer")),
        ("CDN", lapinux_core::i18n::t("downloads.cdn_definition")),
        (
            "CloudFront, Akamai, Fastly",
            lapinux_core::i18n::t("downloads.cdn_explainer"),
        ),
    ] {
        ui.label(egui::RichText::new(term).size(11.5).strong().color(TEXT()));
        ui.add(egui::Label::new(egui::RichText::new(text).size(11.0).color(TEXT_DIM())).wrap());
        ui.add_space(9.0);
    }
    close
}

/// Contenu du panneau « Informations » d'une carte : chaque champ connu du
/// transfert, en cours comme terminé. Renvoie vrai si l'utilisateur a cliqué
/// sur « Fermer ».
fn info_window_body(ui: &mut egui::Ui, job: &Job, net_rate: f32, dlc_names: &[String]) -> bool {
    let title = if job.title.trim().is_empty() {
        sentence(&job.label)
    } else {
        job.title.clone()
    };
    let bytes = |value: u64| lapinux_core::util::format_bytes(value);
    let mut close = false;
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            ui.label(
                egui::RichText::new(lapinux_core::i18n::t("controllers.information.title"))
                    .size(10.0)
                    .color(TEXT_DIM()),
            );
            ui.label(egui::RichText::new(&title).size(16.0).strong());
            muted(
                ui,
                format!("{} · {}", job_kind_label(job), state_title(job.state)),
            );
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if link_text(ui, lapinux_core::i18n::t("downloads.close")).clicked() {
                close = true;
            }
        });
    });
    ui.add_space(8.0);
    ui.separator();
    ui.add_space(8.0);
    let running = job.state == JobState::Running;
    let remaining = job_remaining_seconds(job, net_rate)
        .map(lapinux_core::util::format_duration)
        .unwrap_or_else(|| "—".to_string());
    let mut rows: Vec<(&str, String)> = vec![
        (
            lapinux_core::i18n::t("settings.captures.state"),
            state_title(job.state).to_string(),
        ),
        ("Contenu", job_kind_label(job)),
        (
            lapinux_core::i18n::t("downloads.downloaded"),
            if job.total > 0 {
                format!("{} / {}", bytes(job.downloaded), bytes(job.total))
            } else {
                bytes(job.downloaded)
            },
        ),
        (
            "Progression",
            format!("{:.0} %", job.progress.clamp(0.0, 1.0) * 100.0),
        ),
        (
            lapinux_core::i18n::t("downloads.network"),
            bytes(job.network),
        ),
        (
            lapinux_core::i18n::t("downloads.written"),
            bytes(job.written),
        ),
        (
            lapinux_core::i18n::t("downloads.rate"),
            if running && net_rate > 0.5 {
                network_rate(net_rate)
            } else {
                "—".to_string()
            },
        ),
        (
            lapinux_core::i18n::t("downloads.remaining_label"),
            if running {
                remaining
            } else {
                "—".to_string()
            },
        ),
        (
            "Serveur",
            if job.server.is_empty() {
                "—".to_string()
            } else {
                job.server.clone()
            },
        ),
        (
            lapinux_core::i18n::t("downloads.depot"),
            if job.depots > 0 {
                format!("{} / {}", job.depot, job.depots)
            } else {
                "—".to_string()
            },
        ),
        ("Moteur", job.backend.clone()),
        (
            lapinux_core::i18n::t("downloads.started"),
            lapinux_core::util::format_datetime(job.started),
        ),
        (
            lapinux_core::i18n::t("downloads.finished"),
            job.finished
                .map(lapinux_core::util::format_datetime)
                .unwrap_or_else(|| "—".to_string()),
        ),
    ];
    if !dlc_names.is_empty() {
        rows.push((
            "Contenus additionnels",
            lapinux_core::util::ellipsize(&dlc_names.join(", "), 120),
        ));
    }
    let reused = wording::reused_bytes(job);
    if reused > 0 {
        rows.push((lapinux_core::i18n::t("downloads.reused"), bytes(reused)));
    }
    let phase = job.message.trim();
    if !phase.is_empty() {
        rows.push((lapinux_core::i18n::t("downloads.step"), phase_label(phase)));
    }
    egui::Grid::new(("download-info-grid", job.id.as_str()))
        .num_columns(2)
        .spacing([24.0, 7.0])
        .show(ui, |ui| {
            for (label, value) in rows {
                muted(ui, label);
                ui.label(egui::RichText::new(value).color(TEXT()));
                ui.end_row();
            }
        });
    if job.kind == JobKind::InstallGame {
        ui.add_space(7.0);
        muted(
            ui,
            if dlc_names.is_empty() {
                lapinux_core::i18n::t("downloads.base_build_only")
            } else {
                lapinux_core::i18n::t("downloads.build_and_content")
            },
        );
    }
    close
}

/// Carte de réglages : un titre, une ligne d'aide et son contenu.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_phases_use_clear_wording() {
        assert_eq!(
            phase_label("nettoyage des doublages"),
            lapinux_core::i18n::t("downloads.removing_audio_langs_title")
        );
        assert_eq!(
            phase_label("manifeste 2/2"),
            lapinux_core::i18n::t_args("ui.reading_manifests", &[("rest", "2/2")])
        );
    }

    #[test]
    fn hero_tile_stacks_the_kind_under_the_title() {
        let ctx = egui::Context::default();
        crate::install_fonts(&ctx);
        let job = sample_job(Some(1), JobState::Running, 1);
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                download_hero_tile(ui, 420.0, &job, &job.title, None, None, None, 0.0, 0.0);
            });
        });
        let mut title = None;
        let mut kind = None;
        for clipped in &output.shapes {
            if let egui::Shape::Text(shape) = &clipped.shape {
                match shape.galley.text() {
                    text if text == job.title => title = Some((shape.pos.x, shape.pos.y)),
                    text if text == lapinux_core::i18n::t("downloads.update") => {
                        kind = Some((shape.pos.x, shape.pos.y))
                    }
                    _ => {}
                }
            }
        }
        let (title_x, title_y) = title.expect("le titre doit être dessiné");
        let (kind_x, kind_y) = kind.expect("la nature du transfert doit être dessinée");
        assert!(
            kind_y - title_y >= 18.0,
            "la nature doit être nettement espacée sous le nom : {kind_y} / {title_y}"
        );
        assert!(
            (kind_x - title_x).abs() < 1.0,
            "la nature reste sous le nom, pas au centre : {kind_x} / {title_x}"
        );
    }

    #[test]
    fn collapsed_icon_strip_sits_flush_right() {
        let (shown, more, offset) = collapsed_strip(300.0, 3);
        assert_eq!((shown, more), (3, 0));
        assert!((offset + 3.0 * 20.0 + 2.0 * 5.0 - 300.0).abs() < 0.01);
        // Beaucoup de jeux : la rangée reste à droite et réserve le « +N ».
        let (shown, more, offset) = collapsed_strip(400.0, 30);
        assert!(shown >= 8);
        assert_eq!(more, 30 - shown);
        let strip = shown as f32 * 20.0 + (shown - 1) as f32 * 5.0;
        assert!(
            (offset + strip + 22.0 - 400.0).abs() < 0.01,
            "la rangée doit finir au bord droit : {offset} + {strip}"
        );
    }

    #[test]
    fn collapsed_download_sections_survive_a_reload() {
        let config = lapinux_core::config::DownloadsConfig {
            collapsed_sections: vec!["paused".into(), "history".into()],
            ..lapinux_core::config::DownloadsConfig::default()
        };
        let page = Downloads::from_config(&config);
        assert!(!page.paused_open);
        assert!(!page.history_open);
        assert!(page.running_open);
        assert_eq!(
            page.collapsed_keys(),
            vec!["paused".to_string(), "history".to_string()]
        );
    }

    #[test]
    fn section_heading_reserves_inline_space_for_collapsed_icons() {
        let ctx = egui::Context::default();
        crate::install_fonts(&ctx);
        let mut free_width = 0.0;
        let mut row_height = 0.0;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let before = ui.cursor().min.y;
                let header = LapinuxApp::section_heading(
                    ui,
                    lapinux_core::i18n::t("downloads.active"),
                    "1",
                    Some((
                        HeroGlyph::Pause,
                        lapinux_core::i18n::t("downloads.pause_all"),
                    )),
                    false,
                );
                free_width = header.free.width();
                row_height = ui.cursor().min.y - before;
            });
        });
        assert!(
            free_width > 100.0,
            "espace libre trop étroit : {free_width}"
        );
        assert!(
            row_height < 30.0,
            "les vignettes ne doivent pas ajouter de ligne : {row_height}"
        );
    }

    #[test]
    fn resume_restarts_the_rate_window() {
        assert!(is_resume(Some(JobState::Paused), JobState::Running));
        assert!(is_resume(Some(JobState::Queued), JobState::Running));
        assert!(!is_resume(Some(JobState::Running), JobState::Running));
        assert!(!is_resume(None, JobState::Running));
        assert!(!is_resume(Some(JobState::Paused), JobState::Paused));
    }

    #[test]
    fn report_rates_use_megabits() {
        assert_eq!(
            report_rate(12_500_000),
            format!(
                "{} {}",
                lapinux_core::i18n::decimal("100.0"),
                lapinux_core::i18n::t("downloads.unit_mbit")
            )
        );
        assert_eq!(
            report_rate(0),
            lapinux_core::i18n::t("controllers.information.dash")
        );
    }

    #[test]
    fn report_window_renders_every_metric() {
        let ctx = egui::Context::default();
        crate::install_fonts(&ctx);
        let mut job = sample_job(Some(1), JobState::Done, 1);
        job.finished = Some(120);
        job.report = Some(JobReport {
            elapsed_secs: 90,
            active_secs: 60,
            average_bps: 18_750_000,
            peak_bps: 25_000_000,
            low_bps: 12_500_000,
            network: 140_625_000,
            written: 150_000_000,
            samples: 60,
        });
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(640.0, 480.0),
        ));
        let output = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.set_width(560.0);
                let report = job.report.as_ref().unwrap();
                let closed = report_window_body(ui, &job, report, None);
                assert!(!closed);
            });
        });
        assert!(!output.shapes.is_empty());
        let metrics = report_metrics(&job, job.report.as_ref().unwrap());
        assert!(metrics
            .iter()
            .any(|(label, _)| *label == lapinux_core::i18n::t("downloads.peak")));
        assert!(metrics
            .iter()
            .any(|(label, _)| *label == lapinux_core::i18n::t("downloads.low")));
    }

    #[test]
    fn done_jobs_without_measurements_get_a_partial_report() {
        // L'historique d'avant le suivi des débits doit rester consultable :
        // durée, volumes et moyenne se déduisent de la tâche.
        let mut job = sample_job(Some(2), JobState::Done, 100);
        job.finished = Some(160);
        job.network = 600_000_000;
        let report = report_for(&job).expect("rapport partiel");
        assert_eq!(report.elapsed_secs, 60);
        assert_eq!(report.average_bps, 10_000_000);
        assert_eq!(report.samples, 0);

        let running = sample_job(Some(3), JobState::Running, 100);
        assert!(report_for(&running).is_none());
    }

    #[test]
    fn speedtest_extremes_and_duration_are_readable() {
        assert_eq!(
            speedtest_range_labels(12_500_000.0, 25_000_000.0),
            (
                format!("min {}", lapinux_core::i18n::decimal("100.0")),
                format!("max {}", lapinux_core::i18n::decimal("200.0")),
            )
        );
        assert_eq!(
            format_seconds(8.42),
            format!("{} s", lapinux_core::i18n::decimal("8.4"))
        );
        // Le relevé copié garde les deux volets, leurs extrêmes et la durée.
        let summary = speedtest_summary(
            Some(12.4),
            Some(25_000_000.0),
            Some((12_500_000.0, 30_000_000.0)),
            Some(5_000_000.0),
            None,
            Some(8.42),
        );
        assert!(summary.contains("Ping 12 ms"), "{summary}");
        assert!(
            summary.contains(&format!(
                "{} {} Mb/s (min {} · max {})",
                lapinux_core::i18n::t("downloads.down"),
                lapinux_core::i18n::decimal("200.0"),
                lapinux_core::i18n::decimal("100.0"),
                lapinux_core::i18n::decimal("240.0"),
            )),
            "{summary}"
        );
        assert!(
            summary.contains(&format!(
                "{} {} Mb/s",
                lapinux_core::i18n::t("downloads.up"),
                lapinux_core::i18n::decimal("40.0"),
            )),
            "{summary}"
        );
        assert!(
            summary.contains(&lapinux_core::i18n::t_args(
                "downloads.duration",
                &[(
                    "duration",
                    &format!("{} s", lapinux_core::i18n::decimal("8.4"))
                )],
            )),
            "{summary}"
        );
        assert_eq!(download_store_label("steam"), "Steam");
        assert_eq!(download_store_label("epic"), "Epic");
    }

    #[test]
    fn editing_the_folder_card_keeps_the_next_card_below_it() {
        // `ui.put` recule le curseur sous chaque widget posé. Sans le rappel
        // `advance_cursor_after_rect` en fin de carte, la carte des limites
        // remontait sur celle du dossier pendant le renommage.
        let ctx = egui::Context::default();
        crate::install_fonts(&ctx);
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(360.0, 480.0),
        ));
        let mut text = String::from("/var/mnt/Jeux/LapinuxLibrary");
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.set_width(320.0);
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(320.0, 118.0), egui::Sense::hover());
                let field = egui::Rect::from_min_size(
                    egui::pos2(rect.left() + 14.0, rect.top() + 40.0),
                    egui::vec2(292.0, 24.0),
                );
                ui.put(field, egui::TextEdit::singleline(&mut text));
                let button = egui::Rect::from_min_size(
                    egui::pos2(rect.left() + 14.0, rect.top() + 74.0),
                    egui::vec2(100.0, 26.0),
                );
                let _ = tile_button(ui, button, lapinux_core::i18n::t("downloads.save"), false);
                // Sans ce geste, le curseur serait resté sous le bouton, à
                // mi-hauteur de la carte.
                assert!(ui.cursor().top() < rect.bottom() - 10.0);
                ui.advance_cursor_after_rect(rect);
                let expected = rect.bottom() + ui.spacing().item_spacing.y;
                assert!(
                    (ui.cursor().top() - expected).abs() < 0.01,
                    "la carte suivante reste sous la carte : {} vs {expected}",
                    ui.cursor().top()
                );
            });
        });
    }

    #[test]
    fn speedtest_extremes_do_not_change_the_card_height() {
        // Les extrêmes tiennent sur la ligne du chiffre : afficher un test
        // terminé ne doit pas allonger la carte d'une ligne par mesure.
        fn row_height(range: Option<(f32, f32)>) -> f32 {
            let ctx = egui::Context::default();
            crate::install_fonts(&ctx);
            let mut input = egui::RawInput::default();
            input.screen_rect = Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(360.0, 240.0),
            ));
            let mut height = 0.0;
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.set_width(320.0);
                    let before = ui.cursor().top();
                    speedtest_row(
                        ui,
                        lapinux_core::i18n::t("downloads.down"),
                        Some("590,0".into()),
                        lapinux_core::i18n::t("downloads.unit_mbit"),
                        false,
                        range.clone(),
                    );
                    // Le curseur descend d'exactement une ligne, avec ou sans
                    // extrêmes : c'est la hauteur que prend la carte.
                    height = ui.cursor().top() - before;
                });
            });
            height
        }
        let empty = row_height(None);
        let filled = row_height(Some((12_500_000.0, 30_000_000.0)));
        assert_eq!(empty, filled);
        assert!(empty <= 22.0, "une ligne de mesure reste fine : {empty}");
    }

    #[test]
    fn every_rate_uses_megabits_per_second() {
        // 100 Mbit/s = 12,5 Mo/s : le test de débit et les téléchargements
        // doivent afficher le même 100,0 Mb/s, jamais l'un en Mo/s à côté de
        // l'autre en Mb/s.
        assert_eq!(
            format_mbit(Some(12_500_000.0)).as_deref(),
            Some(lapinux_core::i18n::decimal("100.0").as_str())
        );
        assert_eq!(
            network_rate(12_500_000.0),
            format!(
                "{} {}",
                lapinux_core::i18n::decimal("100.0"),
                lapinux_core::i18n::t("downloads.unit_mbit")
            )
        );
    }

    #[test]
    fn icon_dominant_color_prefers_the_saturated_logo() {
        let mut image = image::RgbaImage::from_pixel(32, 32, image::Rgba([42, 44, 50, 255]));
        for y in 8..24 {
            for x in 8..24 {
                image.put_pixel(x, y, image::Rgba([220, 45, 70, 255]));
            }
        }
        let color = dominant_icon_color(&image::DynamicImage::ImageRgba8(image));
        assert!(
            color.r() as i16 > color.g() as i16 + 40,
            "la couleur du logo doit primer sur le fond gris : {color:?}"
        );
        assert!(
            color.r() > 150,
            "la teinte doit rester lumineuse : {color:?}"
        );
    }

    #[test]
    fn monochrome_icon_falls_back_to_the_accent() {
        let white = image::RgbaImage::from_pixel(16, 16, image::Rgba([255, 255, 255, 255]));
        assert_eq!(
            dominant_icon_color(&image::DynamicImage::ImageRgba8(white)),
            ACCENT()
        );
    }

    fn sample_job(appid: Option<u32>, state: JobState, started: i64) -> Job {
        Job {
            id: format!("game-{}-{started}", appid.unwrap_or(0)),
            kind: JobKind::InstallGame,
            label: format!("Jeu {} ({})", appid.unwrap_or(0), appid.unwrap_or(0)),
            title: format!(
                "{} {}",
                lapinux_core::i18n::t("steaminput.game"),
                appid.unwrap_or(0)
            ),
            appid,
            state,
            progress: 0.42,
            downloaded: 3 * 1024 * 1024 * 1024,
            total: 8 * 1024 * 1024 * 1024,
            network: 3 * 1024 * 1024 * 1024,
            written: 2 * 1024 * 1024 * 1024,
            server: "cache1-fra1.steamcontent.com".into(),
            depot: 2,
            depots: 3,
            updating: true,
            dlc: Vec::new(),
            platform: None,
            message: "dépôt 441".into(),
            backend: "Lapinux".into(),
            started,
            finished: None,
            report: None,
        }
    }

    /// Un jeu de boutique se télécharge chez sa boutique : la carte ne doit
    /// plus annoncer un CDN Steam pour une tâche Epic.
    #[test]
    fn a_store_download_names_its_own_cdn() {
        let mut job = sample_job(Some(1), JobState::Running, 0);
        job.backend = "Steam".into();
        job.server = "cache1-fra1.steamcontent.com".into();
        let facts = download_facts(&job);
        assert!(
            facts
                .iter()
                .any(|fact| fact.contains("CDN Steam") && fact.contains("cache1")),
            "{facts:?}"
        );
        job.backend = "Epic Games".into();
        job.server = "Epic Games".into();
        let facts = download_facts(&job);
        assert!(
            facts.iter().any(|fact| fact.contains("CDN Epic Games")),
            "{facts:?}"
        );
        assert!(
            !facts.iter().any(|fact| fact.contains("CDN Steam")),
            "{facts:?}"
        );
    }

    #[test]
    fn dlc_contents_are_named_on_the_card() {
        let mut job = sample_job(Some(1), JobState::Running, 0);
        job.updating = false;
        assert_eq!(
            job_kind_label(&job),
            lapinux_core::i18n::t("downloads.base_game")
        );
        job.dlc = vec![10, 11];
        assert_eq!(
            job_kind_label(&job),
            format!("{} · 2 DLC", lapinux_core::i18n::t("downloads.base_game"))
        );
        job.dlc = vec![10];
        assert_eq!(
            job_kind_label(&job),
            format!("{} · 1 DLC", lapinux_core::i18n::t("downloads.base_game"))
        );
    }

    #[test]
    fn server_glossary_waits_behind_the_info_button() {
        assert!(!Downloads::new().servers_help_open);
        let ctx = egui::Context::default();
        crate::install_fonts(&ctx);
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(640.0, 520.0),
        ));
        let output = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.set_width(480.0);
                assert!(!servers_help_body(ui));
            });
        });
        let texts: Vec<&str> = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) => Some(text.galley.text()),
                _ => None,
            })
            .collect();
        assert!(texts
            .iter()
            .any(|text| *text == lapinux_core::i18n::t("downloads.choices_help")));
        for term in [
            lapinux_core::i18n::t("downloads.automatic"),
            lapinux_core::i18n::t("settings.application.cache"),
            "CDN",
            "CloudFront, Akamai, Fastly",
        ] {
            assert!(texts.contains(&term), "{term} manque au glossaire");
        }
    }

    #[test]
    fn info_window_lists_a_running_transfer() {
        let ctx = egui::Context::default();
        crate::install_fonts(&ctx);
        let mut job = sample_job(Some(1), JobState::Running, 1_700_000_000);
        job.updating = false;
        job.progress = 0.66;
        job.depots = 2;
        job.depot = 1;
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(640.0, 560.0),
        ));
        let output = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.set_width(480.0);
                let closed = info_window_body(ui, &job, 4_000_000.0, &[]);
                assert!(!closed);
            });
        });
        assert!(!output.shapes.is_empty());
    }

    #[test]
    fn hero_tile_renders_a_blurred_banner_and_a_title_icon() {
        let ctx = egui::Context::default();
        crate::install_fonts(&ctx);
        let pixels = vec![120u8; 4 * 4 * 4];
        let banner = ctx.load_texture(
            "test-banner-blur",
            egui::ColorImage::from_rgba_unmultiplied([4, 4], &pixels),
            egui::TextureOptions::LINEAR,
        );
        let icon = ctx.load_texture(
            "test-game-icon",
            egui::ColorImage::from_rgba_unmultiplied([4, 4], &pixels),
            egui::TextureOptions::LINEAR,
        );
        let cover = CoverArt {
            blurred: banner,
            palette: vec![ACCENT(), OK],
        };
        let job = sample_job(Some(1), JobState::Running, 1);
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(800.0, 600.0),
        ));
        let output = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.set_width(760.0);
                let _ = download_hero_tile(
                    ui,
                    760.0,
                    &job,
                    "Jeu",
                    Some(&cover),
                    Some(&icon),
                    None,
                    1_000_000.0,
                    1_000_000.0,
                );
            });
        });
        assert!(!output.shapes.is_empty(), "la carte doit être dessinée");
    }

    #[test]
    fn tasks_are_grouped_into_one_card_per_game() {
        let mut cancelled = sample_job(Some(1), JobState::Cancelled, 10);
        cancelled.finished = Some(11);
        let mut done = sample_job(Some(2), JobState::Done, 30);
        done.finished = Some(31);
        let mut older_done = sample_job(Some(2), JobState::Done, 25);
        older_done.finished = Some(26);
        let jobs = vec![
            cancelled,
            sample_job(Some(1), JobState::Running, 20),
            done,
            older_done,
        ];

        let sections = job_sections(&jobs);
        // Un seul jeu actif (le plus récent gagne) et un seul jeu terminé.
        assert_eq!(sections.running.len(), 1);
        assert_eq!(sections.running[0].appid, Some(1));
        assert_eq!(sections.running[0].state, JobState::Running);
        assert_eq!(sections.history.len(), 1);
        assert_eq!(sections.history[0].appid, Some(2));
        assert!(sections.active_appids.contains(&1));
        assert!(!sections.active_appids.contains(&2));
    }

    #[test]
    fn terminal_tasks_wait_in_the_history_and_keep_their_resume() {
        let mut failed = sample_job(Some(8), JobState::Failed, 12);
        failed.finished = Some(13);
        let mut done = sample_job(Some(9), JobState::Done, 14);
        done.finished = Some(15);

        let sections = job_sections(&[failed, done]);
        // L'historique porte les tâches qui ont abouti ou échoué ; la carte
        // d'une tâche en échec garde « Réessayer ».
        assert_eq!(sections.history.len(), 2);
        assert_eq!(sections.history[0].appid, Some(9));
        assert_eq!(sections.history[1].appid, Some(8));
        // Une mise à jour du même jeu ne doit pas doubler la carte.
        assert!(sections.active_appids.contains(&8));
        assert!(!sections.active_appids.contains(&9));
    }

    #[test]
    fn a_cancelled_task_leaves_the_page_instead_of_joining_the_history() {
        let mut cancelled = sample_job(Some(7), JobState::Cancelled, 10);
        cancelled.finished = Some(11);
        let mut cancelled_program = sample_job(None, JobState::Cancelled, 12);
        cancelled_program.kind = JobKind::InstallProton;
        cancelled_program.title = "GE-Proton".into();

        let sections = job_sections(&[cancelled, cancelled_program]);
        // Rien dans « Terminées », rien dans « Programmes » : une annulation
        // efface la carte au lieu de la déplacer.
        assert!(sections.history.is_empty());
        assert!(sections.programs.is_empty());
        // La liste reste vide même quand un même jeu a une carte terminée :
        // c'est cette carte-là qui compte, pas l'annulation.
        let mut done = sample_job(Some(7), JobState::Done, 20);
        done.finished = Some(21);
        let sections = job_sections(&[done]);
        assert_eq!(sections.history.len(), 1);
        assert_eq!(sections.history[0].state, JobState::Done);
    }

    #[test]
    fn programs_and_paused_games_get_their_own_categories() {
        let running = sample_job(Some(1), JobState::Running, 1);
        let paused = sample_job(Some(2), JobState::Paused, 2);
        let mut program = sample_job(None, JobState::Running, 3);
        program.kind = JobKind::InstallProton;
        program.title = "GE-Proton".into();

        let sections = job_sections(&[running, paused, program.clone()]);
        assert_eq!(sections.running.len(), 1);
        assert_eq!(sections.running[0].appid, Some(1));
        assert_eq!(sections.paused.len(), 1);
        assert_eq!(sections.paused[0].appid, Some(2));
        assert_eq!(sections.programs.len(), 1);
        assert_eq!(sections.programs[0].kind, JobKind::InstallProton);

        // Une installation de Proton terminée rejoint l'historique, jamais
        // la catégorie des programmes actifs.
        let mut finished = program;
        finished.state = JobState::Done;
        finished.finished = Some(4);
        let sections = job_sections(&[finished]);
        assert!(sections.programs.is_empty());
        assert_eq!(sections.history.len(), 1);
    }

    #[test]
    fn only_terminal_transfers_offer_history_deletion() {
        for state in [JobState::Done, JobState::Failed, JobState::Cancelled] {
            assert!(can_delete_history_job(state), "{state:?}");
        }
        for state in [
            JobState::Running,
            JobState::Queued,
            JobState::Paused,
            JobState::Interrupted,
        ] {
            assert!(!can_delete_history_job(state), "{state:?}");
        }
    }

    #[test]
    fn a_task_stopped_by_a_restart_waits_in_the_paused_category() {
        // Un redémarrage laisse la tâche « interrompue » avec ses fichiers :
        // elle attend une reprise, elle n'a rien terminé.
        let mut interrupted = sample_job(Some(4), JobState::Interrupted, 10);
        interrupted.finished = Some(11);
        let mut older_done = sample_job(Some(4), JobState::Done, 5);
        older_done.finished = Some(6);

        let sections = job_sections(&[interrupted, older_done]);
        assert!(sections.history.is_empty(), "rien dans « Terminées »");
        assert_eq!(sections.paused.len(), 1);
        assert_eq!(sections.paused[0].state, JobState::Interrupted);
        // Une ancienne tâche terminée du même jeu ne double pas la carte.
        assert!(sections.active_appids.contains(&4));

        // Un programme arrêté par un redémarrage garde sa catégorie — les
        // programmes ne se mêlent pas aux jeux — mais quitte lui aussi
        // l'historique, où sa carte passerait pour un transfert abouti.
        let mut program = sample_job(None, JobState::Interrupted, 20);
        program.kind = JobKind::InstallProton;
        program.title = "GE-Proton".into();
        let sections = job_sections(&[program]);
        assert!(sections.history.is_empty());
        assert_eq!(sections.programs.len(), 1);
        assert!(sections.paused.is_empty());
    }

    /// Une tâche de boutique porte sa marque ; une tâche Steam, menée par le
    /// moteur interne, garde celle de Steam. Les cartes affichaient le logo
    /// Steam sur un téléchargement Epic.
    #[test]
    fn job_cards_carry_the_store_mark() {
        let mut epic = sample_job(Some(4), JobState::Running, 10);
        epic.backend = "Epic Games".into();
        assert_eq!(job_store(&epic), lapinux_core::stores::Store::Epic);
        let mut steam = sample_job(Some(4), JobState::Running, 10);
        steam.backend = "Lapinux".into();
        assert_eq!(job_store(&steam), lapinux_core::stores::Store::Steam);
        steam.backend = "gog".into();
        assert_eq!(job_store(&steam), lapinux_core::stores::Store::Gog);
    }

    #[test]
    fn nav_badge_only_follows_a_running_game() {
        let running = sample_job(Some(1), JobState::Running, 1);
        let paused = sample_job(Some(2), JobState::Paused, 2);
        let done = sample_job(Some(3), JobState::Done, 3);
        // En pause ou terminé : l'onglet garde son pictogramme de base.
        assert!(nav_badge_job(&[paused.clone(), done.clone()]).is_none());
        // Un jeu en cours donne son icône, même si une pause traîne à côté.
        assert_eq!(
            nav_badge_job(&[paused, running, done]).and_then(|job| job.appid),
            Some(1)
        );
    }

    #[test]
    fn paused_task_facts_name_the_step_and_server() {
        let job = sample_job(
            Some(9),
            JobState::Paused,
            lapinux_core::util::now_unix() - 120,
        );
        let facts = download_facts(&job);
        assert!(facts.iter().any(|fact| fact.contains("dépôt 441")));
        assert!(facts.iter().any(|fact| fact.contains("cache1-fra1")));
        assert!(facts
            .iter()
            .any(|fact| fact.contains(&lapinux_core::i18n::t_fmt(
                "downloads.since",
                &["2 min".to_string()]
            ))));
        assert!(facts
            .iter()
            .any(|fact| fact == lapinux_core::i18n::t("downloads.no_dlc")));
        assert!(
            facts
                .iter()
                .any(|fact| fact.starts_with("CDN Steam · cache1-fra1")),
            "le logo Steam a un équivalent texte dans les faits"
        );
        // Le pas du daemon cite déjà le dépôt : pas de doublon.
        assert!(!facts.iter().any(|fact| fact.as_str()
            == lapinux_core::i18n::t_fmt("downloads.part", &["2".to_string(), "3".to_string()])));

        // Un message qui ne fait que répéter l'état n'est pas répété.
        let mut quiet = sample_job(
            Some(9),
            JobState::Paused,
            lapinux_core::util::now_unix() - 120,
        );
        quiet.message = lapinux_core::i18n::t("downloads.paused_short").into();
        let facts = download_facts(&quiet);
        assert!(!facts
            .iter()
            .any(|fact| fact.to_lowercase() == lapinux_core::i18n::t("downloads.paused_short")));
        assert!(facts.iter().any(|fact| fact.contains("cache1-fra1")));
    }

    #[test]
    fn the_speed_limit_track_is_visible_and_centred_on_its_row() {
        // Deux défauts d'origine : la piste du curseur du thème portait la
        // couleur des widgets au repos — celle du fond de la carte, donc
        // invisible — et sa poignée faisait la moitié de la hauteur allouée, un
        // gros disque qui descendait sous les icônes. La piste est maintenant
        // peinte, fine, et centrée sur la même ligne que l'icône.
        let ctx = egui::Context::default();
        apply_theme(&ctx);
        let mut value = 500u32;
        let mut icon = egui::Rect::NOTHING;
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let spacing = ui.spacing().clone();
                ui.spacing_mut().interact_size.y = LIMIT_ROW_HEIGHT;
                ui.horizontal(|ui| {
                    // L'icône de la ligne, même allocation que `limit_icon`.
                    let (rect, _) = ui.allocate_exact_size(
                        egui::vec2(LIMIT_ROW_HEIGHT, LIMIT_ROW_HEIGHT),
                        egui::Sense::hover(),
                    );
                    icon = rect;
                    limit_slider(ui, &mut value, LIMIT_MAX_MBIT, 180.0);
                });
                *ui.spacing_mut() = spacing;
            });
        });
        assert_eq!(
            value, 500,
            "une image sans pointeur ne change pas la valeur"
        );

        let mut tracks = Vec::new();
        let mut handles = Vec::new();
        fn walk(
            shape: &egui::Shape,
            tracks: &mut Vec<(egui::Rect, egui::Color32)>,
            handles: &mut Vec<(egui::Pos2, f32, egui::Color32)>,
        ) {
            match shape {
                egui::Shape::Rect(rect)
                    if rect.stroke.width == 0.0 && rect.rect.height() <= 8.0 =>
                {
                    tracks.push((rect.rect, rect.fill));
                }
                egui::Shape::Circle(circle)
                    if circle.stroke.width == 0.0 && circle.radius <= 8.0 =>
                {
                    handles.push((circle.center, circle.radius, circle.fill));
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, tracks, handles);
                    }
                }
                _ => {}
            }
        }
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut tracks, &mut handles);
        }
        assert_eq!(tracks.len(), 2, "la piste et sa part réglée");
        // La piste se voit : sa couleur n'est pas celle du fond de la carte.
        assert!(
            tracks.iter().all(|(_, fill)| *fill != CARD()),
            "la piste ne se confond pas avec la carte"
        );
        assert_eq!(handles.len(), 1, "une seule poignée");
        let (center, radius, _) = handles[0];
        assert!(radius <= 6.0, "la poignée reste petite (rayon = {radius})");
        // Poignée et icône partagent la même ligne de centre.
        let line = icon.center().y;
        assert!(
            (center.y - line).abs() < 0.5,
            "poignée (y = {}) alignée sur l'icône (y = {line})",
            center.y
        );
        assert!(tracks
            .iter()
            .all(|(rect, _)| (rect.center().y - line).abs() < 0.5));
        // À mi-course, la poignée est au milieu de la piste.
        let rail = tracks
            .iter()
            .map(|(rect, _)| *rect)
            .max_by(|a, b| a.width().partial_cmp(&b.width()).unwrap())
            .unwrap();
        assert!(
            (center.x - rail.center().x).abs() < 1.0,
            "à 500 sur 1 000, la poignée est au milieu"
        );
    }

    #[test]
    fn update_announcements_name_fetched_bytes() {
        assert_eq!(
            wording::update_announcement(Some(329 * 1024 * 1024)),
            lapinux_core::i18n::t_fmt(
                "downloads.update_with",
                &[lapinux_core::i18n::t_fmt(
                    "downloads.to_download",
                    &["345 Mo".to_string()]
                )],
            )
        );
        assert_eq!(
            wording::update_announcement(None),
            lapinux_core::i18n::t("downloads.update_unknown_size")
        );
        assert_eq!(
            wording::update_download_label(64 * 1024 * 1024 * 1024),
            lapinux_core::i18n::t_fmt(
                "downloads.to_download",
                &[lapinux_core::util::format_bytes(64 * 1024 * 1024 * 1024)],
            )
        );
    }

    #[test]
    fn active_games_leave_the_updates_list() {
        let rows = vec![
            UpdateRow {
                appid: 1,
                name: "Alpha".into(),
                size: Some(10),
                details: None,
                store: None,
                store_id: String::new(),
                platform: None,
            },
            UpdateRow {
                appid: 2,
                name: "Beta".into(),
                size: Some(30),
                details: None,
                store: None,
                store_id: String::new(),
                platform: None,
            },
            UpdateRow {
                appid: 3,
                name: "Gamma".into(),
                size: None,
                details: None,
                store: None,
                store_id: String::new(),
                platform: None,
            },
        ];
        let active: HashSet<u32> = [1u32].into_iter().collect();
        let sorted = sorted_updates(rows, &active);
        assert_eq!(
            sorted.iter().map(|row| row.appid).collect::<Vec<_>>(),
            vec![2, 3]
        );
        // Le plus gros d'abord, les tailles inconnues à la fin.
        assert_eq!(sorted[0].size, Some(30));
        assert_eq!(sorted[1].size, None);
    }

    #[test]
    fn hero_download_tiles_paint_progress_and_actions() {
        let ctx = egui::Context::default();
        crate::install_fonts(&ctx);
        let mut job = sample_job(
            Some(1),
            JobState::Running,
            lapinux_core::util::now_unix() - 120,
        );
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let action = download_hero_tile(
                    ui,
                    420.0,
                    &job,
                    &job.title,
                    None,
                    None,
                    None,
                    5.2 * 1024.0 * 1024.0,
                    5.2 * 1024.0 * 1024.0,
                );
                assert!(action.is_empty(), "aucun clic ne doit être simulé");
                job.state = JobState::Paused;
                assert!(download_hero_tile(
                    ui, 420.0, &job, &job.title, None, None, None, 0.0, 0.0
                )
                .is_empty());
                job.state = JobState::Cancelled;
                assert!(download_hero_tile(
                    ui, 420.0, &job, &job.title, None, None, None, 0.0, 0.0
                )
                .is_empty());
                job.state = JobState::Done;
                job.finished = Some(lapinux_core::util::now_unix());
                assert!(download_hero_tile(
                    ui, 420.0, &job, &job.title, None, None, None, 0.0, 0.0
                )
                .is_empty());
                // Une mise à jour pas encore lancée n'a que son bouton.
                assert!(!available_update_tile(
                    ui,
                    420.0,
                    1,
                    "Jeu 1",
                    None,
                    Some(8 * 1024 * 1024 * 1024)
                ));
            });
        });
        let texts: Vec<String> = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_string()),
                _ => None,
            })
            .collect();
        assert!(
            output.shapes.len() > 20,
            "les cartes doivent dessiner leur contenu"
        );
        assert!(texts.iter().any(|text| text == &job.title));
        // La ligne unique porte le volume et le débit courant.
        assert!(texts.iter().any(|text| text.contains("Go /")
            && text.contains(lapinux_core::i18n::t("downloads.unit_mbit"))));
        assert!(texts
            .iter()
            .any(|text| text.starts_with(lapinux_core::i18n::t("downloads.cancelled"))));
        assert!(texts
            .iter()
            .any(|text| text.starts_with(state_title(JobState::Done))));
        assert!(texts
            .iter()
            .any(|text| text == lapinux_core::i18n::t("configurations.update")));
        // L'annonce dit ce qui reste à télécharger, pas le volume installé.
        assert!(texts.iter().any(|text| text.as_str()
            == lapinux_core::i18n::t_fmt(
                "downloads.update_with",
                &[lapinux_core::i18n::t_fmt(
                    "downloads.to_download",
                    &[lapinux_core::util::format_bytes(8 * 1024 * 1024 * 1024)],
                )],
            )));
        // Tous les débits sont en Mb/s, la même unité que le test de débit.
        assert!(texts
            .iter()
            .any(|text| text.contains(lapinux_core::i18n::t("downloads.unit_mbit"))));
    }

    #[test]
    fn updates_cache_rebuilds_only_when_marked_dirty() {
        let entry = LibraryEntry {
            playtime: 0,
            acquired: None,
            appid: 1,
            name: "Alpha".into(),
            installed: true,
            on_disk: true,
            steam: true,
            family: false,
            family_available: false,
            update: Some(42),
            store: None,
            cover: String::new(),
            platforms: Vec::new(),
            mobile: false,
            size: None,
            publishers: Vec::new(),
            developers: Vec::new(),
            genres: Vec::new(),
            updated: 0,
            last_played: 0,
        };
        let mut downloads = Downloads::new();
        downloads.refresh_updates(&[entry], None);
        assert_eq!(downloads.updates.len(), 1);
        assert_eq!(downloads.updates[0].appid, 1);

        // Tant que rien n'a changé, la liste reste telle quelle, même si on
        // présente une bibliothèque différente : le cache est la propriété de
        // la page, pas une reconstruction par image.
        downloads.refresh_updates(&[], None);
        assert_eq!(downloads.updates.len(), 1);

        // Un scan ou un catalogue neuf la périme explicitement.
        downloads.mark_updates_dirty();
        downloads.refresh_updates(&[], None);
        assert!(downloads.updates.is_empty());
    }

    #[test]
    fn le_fait_de_depot_accepte_les_deux_ecritures() {
        for step in ["dépôt 2/3 · téléchargement", "depot 2/3 · download"] {
            let mut job = sample_job(Some(9), JobState::Paused, 10);
            job.message = step.into();
            job.depot = 2;
            job.depots = 3;
            let facts = download_facts(&job);
            assert!(
                !facts.iter().any(|fact| fact.as_str()
                    == lapinux_core::i18n::t_fmt(
                        "downloads.part",
                        &["2".to_string(), "3".to_string()]
                    )),
                "{step} : pas de doublon"
            );
        }
    }
}
