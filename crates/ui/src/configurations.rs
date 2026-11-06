//! Onglet Configurations d'une fiche : le catalogue Pissenlit Vert pour ce
//! jeu, l'installation et le retrait en un clic.
//!
//! Le daemon garde le catalogue et fait le travail sur disque ; l'interface ne
//! lit que l'état du jeu, puis lance l'installation dans un fil à part. Une
//! configuration posée modifie les réglages du jeu ou des fichiers : la fiche
//! relit le jeu quand le bilan arrive.

use crate::{muted, primary_button, secondary_button, BORDER, CARD, OK, TEXT, TEXT_DIM, WARN};
use eframe::egui;
use lapinux_core::client::Client;
use lapinux_core::configurations::{
    Author, Configuration, ConfigurationSet, InstallReport, InstalledConfiguration,
};
use lapinux_core::ipc::Req;
use lapinux_core::model::Game;
use lapinux_core::util::format_bytes;
use std::collections::{HashMap, HashSet};
use std::sync::mpsc;
use std::time::Duration;

#[derive(Default)]
pub struct Shelf {
    sets: HashMap<u32, ConfigurationSet>,
    asked: HashSet<u32>,
    client: Option<Client>,
    job: Option<Job>,
    notice: Option<Notice>,
    /// Retrait armé : le premier clic demande, le second retire.
    armed: Option<(u32, String)>,
    /// Une installation ou un retrait a changé les réglages : la fiche relit
    /// le jeu.
    changed: bool,
}

/// Une installation ou un retrait en cours, avec son fil.
struct Job {
    appid: u32,
    id: String,
    removing: bool,
    rx: mpsc::Receiver<Result<InstallReport, String>>,
}

struct Notice {
    ok: bool,
    text: String,
}

impl Shelf {
    /// Catalogue d'un jeu, demandé une seule fois par session. Le daemon
    /// répond de mémoire : l'appel ne bloque pas plus qu'une lecture de
    /// réglages.
    pub fn ensure(&mut self, client: &Client, appid: u32) {
        self.client = Some(client.detached());
        if !self.asked.insert(appid) {
            return;
        }
        self.refresh(appid);
    }

    /// Vrai quand le catalogue connaît au moins une entrée pour ce jeu, ou
    /// qu'une configuration y est encore posée. L'onglet reste caché sinon :
    /// une fiche sans correctif n'a pas à porter une page vide.
    pub fn has_entries(&self, appid: u32) -> bool {
        self.sets
            .get(&appid)
            .map(|set| {
                set.configurations.iter().any(Configuration::available)
                    || !set.installed.is_empty()
            })
            .unwrap_or(false)
    }

    /// Redemande l'état du jeu, après une installation par exemple.
    fn refresh(&mut self, appid: u32) {
        let Some(client) = self.client.clone() else {
            return;
        };
        if let Some(set) =
            crate::request_json::<ConfigurationSet>(&client, Req::GameConfigurations { appid })
        {
            self.sets.insert(appid, set);
        }
    }

    /// Lance l'installation ou le retrait dans un fil : le catalogue et les
    /// fichiers peuvent prendre une seconde, la fiche reste fluide.
    fn start(&mut self, appid: u32, id: String, removing: bool) {
        if self.job.is_some() {
            return;
        }
        let Some(client) = self.client.clone() else {
            return;
        };
        let (tx, rx) = mpsc::channel();
        let asked = id.clone();
        std::thread::spawn(move || {
            let req = if removing {
                Req::RemoveConfiguration {
                    appid,
                    id: asked.clone(),
                }
            } else {
                Req::InstallConfiguration {
                    appid,
                    id: asked.clone(),
                }
            };
            let result = client
                .request(&req)
                .map_err(|error| error.to_string())
                .and_then(|envelope| {
                    if envelope.ok {
                        envelope
                            .into_data::<InstallReport>()
                            .map_err(|error| error.to_string())
                    } else {
                        Err(envelope
                            .error
                            .unwrap_or_else(|| "erreur inconnue".into()))
                    }
                });
            let _ = tx.send(result);
        });
        self.armed = None;
        self.notice = None;
        self.job = Some(Job {
            appid,
            id,
            removing,
            rx,
        });
    }

    /// Suit le fil en cours : réveille la fiche tant qu'il travaille, relit
    /// l'état du jeu quand il a fini. Rend vrai si les réglages ont changé.
    fn poll(&mut self, ctx: &egui::Context) -> bool {
        let Some(job) = &self.job else {
            return false;
        };
        let received = job.rx.try_recv();
        match received {
            Ok(result) => {
                let appid = job.appid;
                let removing = job.removing;
                self.job = None;
                match result {
                    Ok(report) => {
                        let mut text = if removing {
                            format!("« {} » retirée", report.title)
                        } else {
                            format!("« {} » installée · v{}", report.title, report.version)
                        };
                        if report.files > 0 {
                            text.push_str(&format!(" · {} fichier(s)", report.files));
                        }
                        for warning in &report.warnings {
                            text.push_str(" · ");
                            text.push_str(warning);
                        }
                        self.notice = Some(Notice { ok: true, text });
                        self.refresh(appid);
                        self.changed = true;
                        ctx.request_repaint();
                        true
                    }
                    Err(error) => {
                        self.notice = Some(Notice { ok: false, text: error });
                        ctx.request_repaint();
                        false
                    }
                }
            }
            Err(mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(150));
                false
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.job = None;
                self.notice = Some(Notice {
                    ok: false,
                    text: "Le daemon n'a pas rendu de bilan.".into(),
                });
                false
            }
        }
    }

    /// Page de l'onglet. Rend vrai quand la fiche doit relire le jeu.
    pub fn page(&mut self, ui: &mut egui::Ui, game: &Game) -> bool {
        let mut changed = std::mem::take(&mut self.changed) | self.poll(ui.ctx());
        let appid = game.appid;
        let Some(set) = self.sets.get(&appid).cloned() else {
            ui.add_space(18.0);
            ui.vertical_centered(|ui| {
                ui.spinner();
                ui.add_space(6.0);
                muted(ui, "Lecture du catalogue…");
                if secondary_button(ui, "Réessayer").clicked() {
                    self.refresh(appid);
                    ui.ctx().request_repaint();
                }
            });
            changed |= std::mem::take(&mut self.changed);
            return changed;
        };

        let busy = self
            .job
            .as_ref()
            .map(|job| job.appid == appid)
            .unwrap_or(false);
        self.header(ui, &set, busy);
        if let Some(notice) = &self.notice {
            ui.add_space(6.0);
            let color = if notice.ok { OK } else { WARN };
            ui.label(
                egui::RichText::new(&notice.text)
                    .size(11.0)
                    .color(color),
            );
        }

        // Les configurations posées d'abord : ce qui est actif se lit avant ce
        // qui reste à faire.
        let mut entries: Vec<(&Configuration, Option<&InstalledConfiguration>)> = Vec::new();
        for config in &set.configurations {
            let installed = set.installed.iter().find(|entry| entry.id == config.id);
            if config.available() || installed.is_some() {
                entries.push((config, installed));
            }
        }
        entries.sort_by_key(|(_, installed)| installed.is_none());
        // Une entrée retirée du catalogue mais toujours posée reste
        // désinstallable : ses fichiers sont sur le disque.
        let orphans: Vec<&InstalledConfiguration> = set
            .installed
            .iter()
            .filter(|entry| !set.configurations.iter().any(|config| config.id == entry.id))
            .collect();

        ui.add_space(10.0);
        if entries.is_empty() && orphans.is_empty() {
            self.empty(ui);
        } else {
            for (config, installed) in entries {
                self.card(ui, appid, game.installed, config, installed, busy);
                ui.add_space(8.0);
            }
            for entry in orphans {
                self.orphan_card(ui, appid, entry, busy);
                ui.add_space(8.0);
            }
        }

        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            muted(ui, "Catalogue communautaire");
            if let Some(url) = set.maintainer.profile() {
                ui.hyperlink_to(
                    egui::RichText::new(&set.maintainer.name).size(11.0),
                    url,
                );
            } else if !set.maintainer.name.trim().is_empty() {
                ui.label(
                    egui::RichText::new(&set.maintainer.name)
                        .size(11.0)
                        .color(TEXT_DIM()),
                );
            }
            muted(ui, "· aucune donnée n'est envoyée sans votre clic.");
        });

        changed |= std::mem::take(&mut self.changed);
        changed
    }

    fn header(&self, ui: &mut egui::Ui, set: &ConfigurationSet, busy: bool) {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Configurations")
                    .size(15.0)
                    .strong()
                    .color(TEXT()),
            );
            if busy {
                ui.add_space(6.0);
                ui.spinner();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (text, color) = if set.remote {
                    ("Catalogue du dépôt", OK)
                } else {
                    ("Copie hors ligne", WARN)
                };
                ui.label(egui::RichText::new(text).size(10.5).color(color))
                    .on_hover_text(&set.catalog_url);
            });
        });
        ui.add_space(2.0);
        muted(
            ui,
            "Correctifs, traductions et mods de fichiers, installés et retirés en un clic.",
        );
    }

    fn empty(&self, ui: &mut egui::Ui) {
        ui.add_space(18.0);
        ui.vertical_centered(|ui| {
            muted(ui, "Aucune configuration connue pour ce jeu.");
            ui.add_space(3.0);
            muted(ui, "Le catalogue s'enrichit au fil des jeux testés.");
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn card(
        &mut self,
        ui: &mut egui::Ui,
        appid: u32,
        game_installed: bool,
        config: &Configuration,
        installed: Option<&InstalledConfiguration>,
        busy: bool,
    ) {
        let running = busy
            && self
                .job
                .as_ref()
                .map(|job| job.id == config.id)
                .unwrap_or(false);
        let armed = self.armed.as_ref().map(|(app, id)| (*app, id.as_str()))
            == Some((appid, config.id.as_str()));
        let update = installed
            .map(|entry| entry.version != config.version)
            .unwrap_or(false);
        let stroke = if installed.is_some() { OK } else { BORDER() };
        egui::Frame::none()
            .fill(CARD())
            .stroke(egui::Stroke::new(1.0_f32, stroke))
            .rounding(egui::Rounding::same(10.0))
            .inner_margin(12.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new(&config.title)
                            .size(13.0)
                            .strong()
                            .color(TEXT()),
                    );
                    kind_badge(ui, &config.kind);
                    if installed.is_some() {
                        status_badge(ui, "Installée", OK);
                    }
                    status_badge(ui, &config.status, status_color(&config.status));
                });
                ui.add_space(2.0);
                ui.horizontal_wrapped(|ui| {
                    author_link(ui, &config.author);
                    muted(ui, format!("· v{}", config.version));
                    if config.size > 0 {
                        muted(ui, format!("· {}", format_bytes(config.size)));
                    }
                    if let Some(language) = config.language.as_deref().filter(|l| !l.is_empty()) {
                        muted(ui, format!("· {}", language.to_uppercase()));
                    }
                });
                ui.add_space(5.0);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(&config.description)
                            .size(11.5)
                            .color(TEXT_DIM()),
                    )
                    .wrap(),
                );
                details(ui, config);
                if !game_installed && installed.is_none() {
                    ui.add_space(4.0);
                    muted(ui, "Installe le jeu pour appliquer cette configuration.");
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if running {
                        ui.spinner();
                        muted(
                            ui,
                            if self
                                .job
                                .as_ref()
                                .map(|job| job.removing)
                                .unwrap_or(false)
                            {
                                "Retrait…"
                            } else {
                                "Installation…"
                            },
                        );
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_enabled_ui(!busy && (game_installed || installed.is_some()), |ui| {
                            match installed {
                                None => {
                                    if primary_button(ui, "Installer cette configuration")
                                        .clicked()
                                    {
                                        self.start(appid, config.id.clone(), false);
                                    }
                                }
                                Some(entry) => {
                                    if armed {
                                        if secondary_button(ui, "Annuler").clicked() {
                                            self.armed = None;
                                        }
                                        if primary_button(ui, "Confirmer le retrait").clicked() {
                                            self.start(appid, config.id.clone(), true);
                                        }
                                    } else {
                                        if secondary_button(ui, "Retirer").clicked() {
                                            self.armed = Some((appid, config.id.clone()));
                                        }
                                        if update {
                                            if primary_button(ui, "Mettre à jour").clicked() {
                                                self.start(appid, config.id.clone(), false);
                                            }
                                        } else {
                                            muted(ui, &format!("Installée · v{}", entry.version));
                                        }
                                    }
                                }
                            }
                        });
                    });
                });
            });
    }

    /// Configuration posée dont le catalogue a disparu : on peut toujours la
    /// retirer, ses fichiers sont sur le disque.
    fn orphan_card(
        &mut self,
        ui: &mut egui::Ui,
        appid: u32,
        entry: &InstalledConfiguration,
        busy: bool,
    ) {
        let armed = self.armed.as_ref().map(|(app, id)| (*app, id.as_str()))
            == Some((appid, entry.id.as_str()));
        egui::Frame::none()
            .fill(CARD())
            .stroke(egui::Stroke::new(1.0_f32, WARN))
            .rounding(egui::Rounding::same(10.0))
            .inner_margin(12.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new(&entry.title)
                            .size(13.0)
                            .strong()
                            .color(TEXT()),
                    );
                    status_badge(ui, "Hors catalogue", WARN);
                });
                ui.add_space(3.0);
                muted(
                    ui,
                    format!(
                        "Posée le {} · v{} · cette entrée n'est plus publiée.",
                        lapinux_core::util::format_date(entry.installed_at),
                        entry.version
                    ),
                );
                ui.add_space(6.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_enabled_ui(!busy, |ui| {
                        if armed {
                            if secondary_button(ui, "Annuler").clicked() {
                                self.armed = None;
                            }
                            if primary_button(ui, "Confirmer le retrait").clicked() {
                                self.start(appid, entry.id.clone(), true);
                            }
                        } else if secondary_button(ui, "Retirer").clicked() {
                            self.armed = Some((appid, entry.id.clone()));
                        }
                    });
                });
            });
    }
}

/// Ce que la configuration change, les fichiers qu'elle pose et sur quoi elle
/// a été essayée : replié, pour que la carte reste courte.
fn details(ui: &mut egui::Ui, config: &Configuration) {
    let files = &config.actions.files;
    egui::CollapsingHeader::new(
        egui::RichText::new("Détails")
            .size(11.5)
            .color(TEXT_DIM()),
    )
    .id_salt(format!("configuration-details-{}", config.id))
    .show(ui, |ui| {
        ui.set_width(ui.available_width());
        if !config.solution.trim().is_empty() {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(&config.solution)
                        .size(11.0)
                        .color(TEXT()),
                )
                .wrap(),
            );
            ui.add_space(4.0);
        }
        if !files.is_empty() {
            muted(ui, "Fichiers remplacés :");
            for file in files {
                ui.label(
                    egui::RichText::new(format!("· {}", file.target))
                        .size(11.0)
                        .color(TEXT_DIM()),
                );
            }
            ui.add_space(4.0);
        }
        let mut facts = Vec::new();
        if !config.platform.trim().is_empty() {
            facts.push(format!("Plateforme : {}", platform_label(&config.platform)));
        }
        if !config.layers.is_empty() {
            facts.push(format!("Couches : {}", config.layers.join(", ")));
        }
        if !config.license.trim().is_empty() {
            facts.push(format!("Licence : {}", config.license));
        }
        if config.tested.proton.trim().is_empty() && config.tested.gpu.trim().is_empty() {
        } else {
            let mut tested = String::new();
            for value in [&config.tested.proton, &config.tested.gpu, &config.tested.ram] {
                if value.trim().is_empty() {
                    continue;
                }
                if !tested.is_empty() {
                    tested.push_str(" · ");
                }
                tested.push_str(value);
            }
            if !config.tested.date.trim().is_empty() {
                if !tested.is_empty() {
                    tested.push_str(" · ");
                }
                tested.push_str(&config.tested.date);
            }
            if !tested.is_empty() {
                facts.push(format!("Essayée sur : {tested}"));
            }
        }
        for fact in facts {
            muted(ui, fact);
        }
        if !config.links.is_empty() {
            ui.add_space(3.0);
            for link in &config.links {
                ui.hyperlink_to(
                    egui::RichText::new("Documentation de la configuration").size(11.0),
                    link,
                );
            }
        }
    });
}

fn platform_label(platform: &str) -> &str {
    match platform {
        "windows" => "Windows (Proton)",
        "linux" => "Linux",
        "both" => "Windows et Linux",
        other => other,
    }
}

fn author_link(ui: &mut egui::Ui, author: &Author) {
    if author.name.trim().is_empty() {
        return;
    }
    match author.profile() {
        Some(url) => {
            ui.hyperlink_to(egui::RichText::new(&author.name).size(11.0), url);
        }
        None => {
            ui.label(
                egui::RichText::new(&author.name)
                    .size(11.0)
                    .color(TEXT_DIM()),
            );
        }
    }
}

fn status_color(status: &str) -> egui::Color32 {
    match status {
        "verified" => OK,
        "broken" => WARN,
        _ => TEXT_DIM(),
    }
}

fn status_badge(ui: &mut egui::Ui, text: &str, color: egui::Color32) {
    let label = match text {
        "verified" => "vérifiée",
        "proposed" => "à vérifier",
        "broken" => "cassée",
        other => other,
    };
    egui::Frame::none()
        .fill(color.gamma_multiply(0.16))
        .rounding(egui::Rounding::same(5.0))
        .inner_margin(egui::Margin::symmetric(6.0, 2.0))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(label)
                    .size(10.0)
                    .strong()
                    .color(color),
            );
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shelf(entries: Vec<Configuration>, installed: Vec<InstalledConfiguration>) -> Shelf {
        let mut shelf = Shelf::default();
        shelf.sets.insert(
            7,
            ConfigurationSet {
                configurations: entries,
                installed,
                ..Default::default()
            },
        );
        shelf
    }

    #[test]
    fn l_onglet_reste_cache_sans_entree() {
        assert!(!Shelf::default().has_entries(7));
        assert!(!shelf(Vec::new(), Vec::new()).has_entries(7));
    }

    #[test]
    fn une_entree_disponible_montre_l_onglet() {
        assert!(shelf(vec![Configuration::default()], Vec::new()).has_entries(7));
    }

    #[test]
    fn une_entree_retiree_et_non_installee_ne_montre_rien() {
        let retired = Configuration {
            retired: true,
            ..Default::default()
        };
        assert!(!shelf(vec![retired], Vec::new()).has_entries(7));
    }

    #[test]
    fn une_configuration_installee_reste_visible_meme_retiree() {
        let retired = Configuration {
            retired: true,
            ..Default::default()
        };
        let installed = InstalledConfiguration {
            id: retired.id.clone(),
            ..Default::default()
        };
        assert!(shelf(vec![retired], vec![installed]).has_entries(7));
    }
}

fn kind_badge(ui: &mut egui::Ui, kind: &str) {
    let (label, color) = match kind {
        "graphics" => ("Graphismes", egui::Color32::from_rgb(126, 165, 232)),
        "performance" => ("Performances", OK),
        "display" => ("Affichage", egui::Color32::from_rgb(178, 148, 232)),
        "language" => ("Langue", egui::Color32::from_rgb(232, 164, 118)),
        "compatibility" => ("Compatibilité", WARN),
        "input" => ("Entrées", egui::Color32::from_rgb(120, 196, 204)),
        "content" => ("Contenu", egui::Color32::from_rgb(206, 148, 196)),
        other => (other, TEXT_DIM()),
    };
    egui::Frame::none()
        .fill(color.gamma_multiply(0.14))
        .rounding(egui::Rounding::same(5.0))
        .inner_margin(egui::Margin::symmetric(6.0, 2.0))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(label).size(10.0).color(color));
        });
}
