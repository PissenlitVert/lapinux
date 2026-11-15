//! Copies locales des sauvegardes, dans la fiche d'un jeu : trois emplacements
//! à recharger, la copie active et son origine, la restauration et l'export.
//!
//! Le daemon fait le travail (copie, empreinte, rotation, journal) ; ici on ne
//! garde qu'un petit cache par jeu, relu après chaque action.

use crate::{
    field_label, link_text, muted, settings_card, ACCENT, BORDER, CARD, CARD_ACTIVE, CARD_HOVER,
    DANGER, OK, TEXT, TEXT_DIM,
};
use lapinux_core::client::Client;
use lapinux_core::ipc::Req;
use lapinux_core::model::{SaveEvent, SaveEventKind, SaveSnapshot};
use lapinux_core::util::{format_bytes, format_datetime};
use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

/// Hauteur d'un emplacement : de quoi placer le numéro, la date et le poids.
const SLOT_HEIGHT: f32 = 88.0;
/// Gouttière entre deux emplacements.
const SLOT_GAP: f32 = 12.0;

/// Nature de l'action lancée en tâche de fond : le bilan d'une restauration
/// ou d'une suppression s'affiche sous les emplacements, les autres dans
/// l'encart des réglages locaux.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ActionKind {
    Other,
    Restore,
    Delete,
    Save,
}

/// Ce que la fiche retient d'un jeu entre deux images.
#[derive(Default)]
pub struct Panel {
    snapshots: HashMap<u32, Vec<SaveSnapshot>>,
    events: HashMap<u32, Vec<SaveEvent>>,
    /// Erreur de lecture, pour ne pas afficher « aucune copie » à tort.
    errors: HashMap<u32, String>,
    /// Dernier bilan d'une copie ou d'un export.
    status: HashMap<u32, (bool, String)>,
    /// Bilan d'une restauration, montré sous les emplacements.
    slot_status: HashMap<u32, (bool, String)>,
    /// Emplacement choisi pour la restauration.
    selected: HashMap<u32, String>,
    /// Jeu à relancer une fois sa sauvegarde remise en place. Le jeu ne relit
    /// ses fichiers qu'au démarrage : charger sans relancer ne changerait rien
    /// à la partie. Le lancement attend la fin de la restauration.
    pending_launch: Option<u32>,
    /// Suppression demandée : la poubelle devient une coche.
    delete_armed: Option<(u32, String)>,
    fetch: Option<(u32, mpsc::Receiver<Result<Loaded, String>>)>,
    action: Option<(u32, ActionKind, mpsc::Receiver<Result<String, String>>)>,
    /// Jeu dont le champ d'export est ouvert.
    export_for: Option<u32>,
    export_dir: String,
}

/// Réponse d'une lecture : les copies et leur journal arrivent ensemble, pour
/// que la fiche n'affiche jamais l'un sans l'autre.
struct Loaded {
    snapshots: Vec<SaveSnapshot>,
    events: Vec<SaveEvent>,
}

/// Le client n'est pas clonable : une tâche de fond en reçoit une copie qui
/// porte le même socket et le même journal.
fn detached(client: &Client) -> Client {
    client.detached()
}

impl Panel {
    /// Lit les copies d'un jeu une fois ; `reload` force la relecture.
    pub fn ensure(&mut self, ctx: &egui::Context, client: &Client, appid: u32) {
        if self.snapshots.contains_key(&appid)
            || self.errors.contains_key(&appid)
            || self
                .fetch
                .as_ref()
                .map(|(id, _)| *id == appid)
                .unwrap_or(false)
        {
            return;
        }
        self.start_fetch(ctx, client, appid);
    }

    fn start_fetch(&mut self, ctx: &egui::Context, client: &Client, appid: u32) {
        let (tx, rx) = mpsc::channel();
        self.fetch = Some((appid, rx));
        let client = detached(client);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = load(&client, appid);
            if tx.send(result).is_ok() {
                ctx.request_repaint();
            }
        });
    }

    /// Ramasse les réponses des tâches de fond.
    pub fn poll(&mut self, _ctx: &egui::Context) {
        if let Some((appid, rx)) = &self.fetch {
            match rx.try_recv() {
                Ok(Ok(loaded)) => {
                    self.snapshots.insert(*appid, loaded.snapshots);
                    self.events.insert(*appid, loaded.events);
                    self.fetch = None;
                }
                Ok(Err(error)) => {
                    self.errors.insert(*appid, error);
                    self.fetch = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.errors
                        .insert(*appid, lapinux_core::i18n::t("ui.read_interrupted").into());
                    self.fetch = None;
                }
            }
        }
        if let Some((appid, kind, rx)) = &self.action {
            match rx.try_recv() {
                Ok(result) => {
                    let (ok, message) = match result {
                        Ok(message) => (true, message),
                        Err(error) => (false, error),
                    };
                    let appid = *appid;
                    // Un chargement qui échoue ne doit pas lancer le jeu sur
                    // des sauvegardes qui n'ont pas bougé.
                    if !ok && *kind == ActionKind::Restore {
                        self.pending_launch = None;
                    }
                    if *kind == ActionKind::Other {
                        self.status.insert(appid, (ok, message));
                    } else {
                        self.slot_status.insert(appid, (ok, message));
                    }
                    self.action = None;
                    self.invalidate(appid);
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    let message = "action interrompue".to_string();
                    let appid = *appid;
                    if *kind == ActionKind::Other {
                        self.status.insert(appid, (false, message));
                    } else {
                        self.slot_status.insert(appid, (false, message));
                    }
                    self.action = None;
                }
            }
        }
    }

    /// Jeu à relancer après un chargement, une seule fois et seulement quand
    /// la restauration est terminée.
    pub fn take_launch(&mut self) -> Option<u32> {
        if self.action.is_some() {
            return None;
        }
        self.pending_launch.take()
    }

    /// Oublie le cache d'un jeu : la prochaine image relit copies et journal.
    fn invalidate(&mut self, appid: u32) {
        self.snapshots.remove(&appid);
        self.events.remove(&appid);
        self.errors.remove(&appid);
    }

    fn busy(&self, appid: u32) -> bool {
        self.action
            .as_ref()
            .map(|(id, _, _)| *id == appid)
            .unwrap_or(false)
    }

    fn spawn_action<F>(&mut self, ctx: &egui::Context, appid: u32, kind: ActionKind, task: F)
    where
        F: FnOnce() -> Result<String, String> + Send + 'static,
    {
        if self.action.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.action = Some((appid, kind, rx));
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            if tx.send(task()).is_ok() {
                ctx.request_repaint();
            }
        });
    }

    /// Les trois emplacements, en haut de l'onglet : charger un emplacement
    /// remet ses fichiers en place, puis relance le jeu dessus quand le
    /// réglage le demande.
    pub fn slots_card(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        client: &Client,
        appid: u32,
        installed: bool,
        relaunch: bool,
    ) {
        self.poll(ctx);
        self.ensure(ctx, client, appid);
        let snapshots = self.snapshots.get(&appid).cloned().unwrap_or_default();
        let events = self.events.get(&appid).cloned().unwrap_or_default();
        // La copie marquée « en jeu » est la dernière appliquée ou prise :
        // c'est l'état que le jeu lira à son prochain démarrage.
        let active = active_copy(&snapshots, &events);
        // Une copie restaurée ou tournée disparaît : la sélection ne survit
        // pas à son absence.
        if let Some(id) = self.selected.get(&appid).cloned() {
            if !snapshots.iter().any(|snapshot| snapshot.id == id) {
                self.selected.remove(&appid);
            }
        }
        let busy = self.busy(appid);
        let selected = self.selected.get(&appid).cloned();
        let subtitle = if snapshots.is_empty() {
            lapinux_core::i18n::t("saves.slots_empty")
                .to_string()
                .to_string()
        } else {
            lapinux_core::i18n::t_args(
                "saves.slots_used",
                &[("count", &snapshots.len().to_string())],
            )
        };
        let mut click = None;
        let mut load = None;
        let mut arm_delete = None;
        let mut delete_now = None;
        let mut save_slot = None;
        settings_card(
            ui,
            lapinux_core::i18n::t("saves.slots_title"),
            &subtitle,
            |ui| {
                let width = slot_width(ui.available_width(), SLOT_GAP);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(SLOT_GAP, 0.0);
                    for index in 0..3 {
                        let slot = index as u8 + 1;
                        match snapshots.iter().find(|snapshot| snapshot.slot == slot) {
                            Some(snapshot) => {
                                let selected = selected.as_deref() == Some(snapshot.id.as_str());
                                let armed_delete = self
                                    .delete_armed
                                    .as_ref()
                                    .map(|(id, chosen)| *id == appid && chosen == &snapshot.id)
                                    .unwrap_or(false);
                                let reply = slot_tile(
                                    ui,
                                    width,
                                    slot,
                                    snapshot,
                                    selected,
                                    busy,
                                    armed_delete,
                                    active
                                        .as_ref()
                                        .map(|copy| copy.id == snapshot.id)
                                        .unwrap_or(false),
                                );
                                if reply.delete {
                                    if armed_delete {
                                        delete_now = Some(snapshot.id.clone());
                                    } else {
                                        arm_delete = Some(snapshot.id.clone());
                                    }
                                } else if reply.selected {
                                    click = Some(snapshot.id.clone());
                                }
                            }
                            None => {
                                if empty_slot(ui, width, slot, busy).clicked() && !busy {
                                    save_slot = Some(slot);
                                }
                            }
                        }
                    }
                });
                if let Some(copy) = &active {
                    ui.add_space(4.0);
                    muted(
                        ui,
                        lapinux_core::i18n::t_args(
                            "saves.active_copy",
                            &[
                                ("place", &copy_place(copy)),
                                ("origin", &copy_origin(&events, copy)),
                            ],
                        ),
                    );
                }
                ui.add_space(10.0);
                let selected_copy = selected
                    .as_ref()
                    .and_then(|id| snapshots.iter().find(|snapshot| &snapshot.id == id))
                    .cloned();
                let armed_delete = self
                    .delete_armed
                    .as_ref()
                    .map(|(id, chosen)| {
                        *id == appid
                            && selected_copy
                                .as_ref()
                                .map(|copy| &copy.id == chosen)
                                .unwrap_or(false)
                    })
                    .unwrap_or(false);
                let place = match selected_copy.as_ref() {
                    Some(copy) if (1..=3).contains(&copy.slot) => lapinux_core::i18n::t_args(
                        "saves.slot",
                        &[("slot", &copy.slot.to_string())],
                    ),
                    Some(copy) => lapinux_core::i18n::t_args(
                        "saves.copy_of_date",
                        &[("date", &format_datetime(copy.created))],
                    ),
                    None => String::new(),
                };
                let label = if selected_copy.is_none() {
                    lapinux_core::i18n::t("saves.choose_slot")
                        .to_string()
                        .to_string()
                } else if installed && relaunch {
                    // Le jeu ne relit ses fichiers qu'au démarrage : charger le
                    // relance aussitôt, comme un chargement depuis son menu.
                    lapinux_core::i18n::t_args("saves.load_and_play", &[("place", &place)])
                } else {
                    lapinux_core::i18n::t_args("saves.load", &[("place", &place)])
                };
                let ready = selected_copy.is_some() && !busy;
                ui.horizontal(|ui| {
                    let spacing = ui.spacing().item_spacing.x;
                    let used = crate::CARD_GLYPH_SIZE * 2.0 + spacing * 2.0;
                    let width = (ui.available_width() - used).max(120.0);
                    if ui
                        .add_enabled(
                            ready,
                            egui::Button::new(label)
                                .min_size(egui::vec2(width, crate::CARD_GLYPH_SIZE)),
                        )
                        .clicked()
                    {
                        if let Some(copy) = &selected_copy {
                            load = Some(copy.id.clone());
                        }
                    }
                    let folder = ui
                        .add_enabled_ui(ready, |ui| {
                            crate::card_glyph_button(
                                ui,
                                crate::CardGlyph::Folder,
                                lapinux_core::i18n::t("saves.open_slot"),
                            )
                        })
                        .inner;
                    if folder.clicked() {
                        if let Some(copy) = &selected_copy {
                            crate::open_path(Path::new(&copy.path));
                        }
                    }
                    let glyph = if armed_delete {
                        crate::CardGlyph::Check
                    } else {
                        crate::CardGlyph::Trash
                    };
                    let hint = if armed_delete {
                        lapinux_core::i18n::t("saves.confirm_delete_slot")
                    } else {
                        lapinux_core::i18n::t("saves.delete_slot")
                    };
                    let trash = ui
                        .add_enabled_ui(ready, |ui| crate::card_glyph_button(ui, glyph, hint))
                        .inner;
                    if trash.clicked() {
                        if let Some(copy) = &selected_copy {
                            if armed_delete {
                                delete_now = Some(copy.id.clone());
                            } else {
                                arm_delete = Some(copy.id.clone());
                            }
                        }
                    }
                });
                if armed_delete {
                    if let Some(copy) = &selected_copy {
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new(lapinux_core::i18n::t_args(
                                "saves.confirm_delete",
                                &[("place", &copy_place(copy))],
                            ))
                            .size(10.5)
                            .color(DANGER),
                        );
                    }
                }
                if let Some(first) = snapshots.first() {
                    if Path::new(&first.source).is_dir() {
                        ui.add_space(4.0);
                        if link_text(ui, lapinux_core::i18n::t("saves.open_game_folder")).clicked()
                        {
                            crate::open_path(Path::new(&first.source));
                        }
                    }
                }
                if let Some((ok, message)) = self.slot_status.get(&appid) {
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new(message).size(11.0).color(if *ok {
                        OK
                    } else {
                        DANGER
                    }));
                }
                if snapshots.is_empty() && !busy {
                    ui.add_space(4.0);
                    muted(ui, lapinux_core::i18n::t("saves.none_hint"));
                }
            },
        );
        if let Some(id) = click {
            // Un autre choix annule la suppression armée.
            self.delete_armed = None;
            if selected.as_deref() == Some(id.as_str()) {
                self.selected.remove(&appid);
            } else {
                self.selected.insert(appid, id);
            }
        }
        if let Some(id) = arm_delete {
            // La copie armée devient la sélection : la rangée d'actions et
            // l'avertissement parlent d'elle.
            self.selected.insert(appid, id.clone());
            self.delete_armed = Some((appid, id));
        }
        if let Some(id) = delete_now {
            self.delete_armed = None;
            self.selected.remove(&appid);
            let worker = detached(client);
            self.spawn_action(ctx, appid, ActionKind::Delete, move || {
                delete(&worker, appid, &id)
            });
        }
        if let Some(slot) = save_slot {
            self.delete_armed = None;
            self.selected.remove(&appid);
            let worker = detached(client);
            self.spawn_action(ctx, appid, ActionKind::Save, move || {
                backup_slot(&worker, appid, slot)
            });
        }
        if let Some(chosen) = load {
            self.delete_armed = None;
            // Le jeu ne lit ses sauvegardes qu'au démarrage : le lancement
            // suit la restauration, comme un chargement depuis son menu.
            let launching = installed && relaunch;
            if launching {
                self.pending_launch = Some(appid);
            }
            let worker = detached(client);
            self.spawn_action(ctx, appid, ActionKind::Restore, move || {
                restore(&worker, appid, &chosen, launching)
            });
        }
    }

    /// Encart de la fiche : prendre une copie, exporter les copies et, au
    /// besoin, choisir un dossier de sauvegarde quand la détection se trompe.
    pub fn card<F>(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        client: &Client,
        appid: u32,
        installed: bool,
        folder: Option<F>,
    ) where
        F: FnOnce(&mut egui::Ui),
    {
        self.poll(ctx);
        self.ensure(ctx, client, appid);
        let busy = self.busy(appid);
        let snapshots = self.snapshots.get(&appid).cloned().unwrap_or_default();
        let error = self.errors.get(&appid).cloned();
        let status = self.status.get(&appid).cloned();
        let subtitle = if snapshots.is_empty() {
            lapinux_core::i18n::t("saves.settings_subtitle")
                .to_string()
                .to_string()
        } else {
            let bytes: u64 = snapshots.iter().map(|snapshot| snapshot.bytes).sum();
            lapinux_core::i18n::t_args(
                "saves.settings_counts",
                &[
                    ("copies", &snapshots.len().to_string()),
                    ("size", &format_bytes(bytes)),
                ],
            )
        };
        settings_card(
            ui,
            lapinux_core::i18n::t("saves.settings_title"),
            &subtitle,
            |ui| {
                if !installed {
                    ui.add_space(10.0);
                    muted(ui, lapinux_core::i18n::t("saves.not_installed"));
                }
                if let Some(error) = &error {
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new(lapinux_core::i18n::t_args(
                            "ui.read_failed",
                            &[("error", &error.to_string())],
                        ))
                        .size(11.0)
                        .color(DANGER),
                    );
                }
                if let Some((ok, message)) = &status {
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new(message).size(11.0).color(if *ok {
                        OK
                    } else {
                        DANGER
                    }));
                }
                ui.add_space(10.0);
                let width = ui.available_width();
                if busy {
                    ui.add_enabled(
                        false,
                        egui::Button::new(lapinux_core::i18n::t("ui.saves_busy"))
                            .min_size(egui::vec2(width, 28.0)),
                    );
                } else if ui
                    .add_sized(
                        [width, 28.0],
                        egui::Button::new(lapinux_core::i18n::t("saves.backup_now")),
                    )
                    .on_hover_text(lapinux_core::i18n::t("saves.backup_now_desc"))
                    .clicked()
                {
                    let worker = detached(client);
                    self.spawn_action(ctx, appid, ActionKind::Other, move || {
                        backup(&worker, appid)
                    });
                }
                ui.add_space(8.0);
                let open = self.export_for == Some(appid);
                if ui
                    .button(
                        egui::RichText::new(if open {
                            lapinux_core::i18n::t("saves.close_export")
                        } else {
                            lapinux_core::i18n::t("saves.export")
                        })
                        .size(11.0),
                    )
                    .clicked()
                {
                    self.export_for = if open { None } else { Some(appid) };
                }
                if self.export_for == Some(appid) {
                    ui.add_space(6.0);
                    field_label(ui, lapinux_core::i18n::t("saves.export_folder"));
                    let mut dir = self.export_dir.clone();
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut dir)
                                .hint_text(lapinux_core::i18n::t("saves.export_example"))
                                .desired_width(f32::INFINITY),
                        )
                        .changed()
                    {
                        self.export_dir = dir.clone();
                    }
                    muted(ui, lapinux_core::i18n::t("saves.export_desc"));
                    let ready = !self.export_dir.trim().is_empty();
                    ui.add_space(6.0);
                    if ui
                        .add_enabled(
                            ready && !busy,
                            egui::Button::new(lapinux_core::i18n::t("saves.export"))
                                .min_size(egui::vec2(ui.available_width(), 26.0)),
                        )
                        .clicked()
                    {
                        let dest = self.export_dir.clone();
                        let worker = detached(client);
                        self.spawn_action(ctx, appid, ActionKind::Other, move || {
                            export(&worker, appid, &dest)
                        });
                    }
                }
                if let Some(folder) = folder {
                    ui.add_space(10.0);
                    egui::CollapsingHeader::new(
                        egui::RichText::new(lapinux_core::i18n::t("saves.custom_folder"))
                            .size(11.5)
                            .color(TEXT_DIM()),
                    )
                    .id_salt(("saves-folder", appid))
                    .default_open(false)
                    .show(ui, folder);
                }
            },
        );
    }
}

/// Largeur d'un emplacement : trois colonnes égales entre deux gouttières.
fn slot_width(width: f32, gap: f32) -> f32 {
    ((width - gap * 2.0) / 3.0).max(64.0)
}

/// Comment nommer une copie dans une phrase : son emplacement, ou sa date si
/// elle précède le découpage en emplacements.
fn copy_place(snapshot: &SaveSnapshot) -> String {
    if (1..=3).contains(&snapshot.slot) {
        lapinux_core::i18n::t_args("saves.slot", &[("slot", &snapshot.slot.to_string())])
    } else {
        lapinux_core::i18n::t_args(
            "saves.copy_of_date",
            &[("date", &format_datetime(snapshot.created))],
        )
    }
}

/// La copie qui correspond à l'état du jeu : la dernière opération notée.
/// Une restauration laisse en place une copie de sécurité plus récente que
/// l'emplacement chargé, mais non notée : le journal reste la seule source.
fn active_copy(snapshots: &[SaveSnapshot], events: &[SaveEvent]) -> Option<SaveSnapshot> {
    let latest = events
        .iter()
        .filter(|event| matches!(event.kind, SaveEventKind::Backup | SaveEventKind::Restore))
        .max_by_key(|event| event.at);
    if let Some(copy) =
        latest.and_then(|event| snapshots.iter().find(|copy| copy.id == event.snapshot))
    {
        return Some(copy.clone());
    }
    // Sans journal lisible, la plus récente reste la meilleure indication.
    snapshots.iter().max_by_key(|copy| copy.created).cloned()
}

/// D'où vient la copie qui correspond au jeu : une fermeture de partie, un
/// clic dans la fiche, ou un emplacement chargé dans le jeu.
fn copy_origin(events: &[SaveEvent], copy: &SaveSnapshot) -> String {
    match events
        .iter()
        .filter(|event| event.snapshot == copy.id)
        .filter(|event| matches!(event.kind, SaveEventKind::Backup | SaveEventKind::Restore))
        .max_by_key(|event| event.at)
    {
        Some(event) if event.kind == SaveEventKind::Restore => lapinux_core::i18n::t_args(
            "saves.origin.restored",
            &[("date", &format_datetime(event.at))],
        ),
        Some(event) if event.auto => lapinux_core::i18n::t("saves.origin.auto")
            .to_string()
            .to_string(),
        Some(event) if event.imported => lapinux_core::i18n::t("saves.origin.imported")
            .to_string()
            .to_string(),
        Some(_) => lapinux_core::i18n::t("saves.origin.manual")
            .to_string()
            .to_string(),
        None => lapinux_core::i18n::t("saves.origin.local")
            .to_string()
            .to_string(),
    }
}

/// Ce qu'un clic sur un emplacement demande : le choisir, ou supprimer sa
/// copie quand le clic tombe sur la poubelle.
struct SlotReply {
    selected: bool,
    delete: bool,
}

/// Un emplacement rempli : numéro, date de la copie, fichiers, poids et une
/// poubelle dans le coin. Armée, la poubelle devient une coche. La copie que
/// le jeu lit actuellement porte « en jeu ».
fn slot_tile(
    ui: &mut egui::Ui,
    width: f32,
    slot: u8,
    snapshot: &SaveSnapshot,
    selected: bool,
    busy: bool,
    armed_delete: bool,
    active: bool,
) -> SlotReply {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, SLOT_HEIGHT), egui::Sense::click());
    let hovered = response.hovered() && !busy;
    let fill = if selected {
        CARD_ACTIVE()
    } else if hovered {
        CARD_HOVER()
    } else {
        CARD()
    };
    let stroke = if selected {
        egui::Stroke::new(1.5_f32, ACCENT())
    } else {
        egui::Stroke::new(1.0_f32, BORDER())
    };
    let painter = ui.painter();
    painter.rect(rect, egui::Rounding::same(10.0), fill, stroke);
    let inner = rect.shrink(12.0);
    painter.text(
        inner.left_top(),
        egui::Align2::LEFT_TOP,
        format!("Emplacement {slot}"),
        egui::FontId::proportional(10.5),
        TEXT_DIM(),
    );
    painter.text(
        egui::pos2(inner.left(), inner.top() + 17.0),
        egui::Align2::LEFT_TOP,
        format_datetime(snapshot.created),
        egui::FontId::proportional(12.0),
        TEXT(),
    );
    painter.text(
        egui::pos2(inner.left(), inner.top() + 37.0),
        egui::Align2::LEFT_TOP,
        lapinux_core::i18n::t_fmt("saves.files_count", &[snapshot.files.to_string()]),
        egui::FontId::proportional(10.5),
        TEXT_DIM(),
    );
    painter.text(
        inner.left_bottom(),
        egui::Align2::LEFT_BOTTOM,
        format_bytes(snapshot.bytes),
        egui::FontId::proportional(10.5),
        TEXT_DIM(),
    );
    if active {
        painter.text(
            inner.right_bottom(),
            egui::Align2::RIGHT_BOTTOM,
            lapinux_core::i18n::t("saves.in_game"),
            egui::FontId::proportional(9.5),
            OK,
        );
    }
    // La poubelle occupe le coin : le clic s'y décide par la position du
    // pointeur, sans imbriquer un second bouton dans la carte.
    const ACTION: f32 = 24.0;
    let action = egui::Rect::from_min_size(
        egui::pos2(rect.right() - ACTION - 8.0, rect.top() + 8.0),
        egui::vec2(ACTION, ACTION),
    );
    let over_action = response.hovered()
        && ui
            .ctx()
            .pointer_hover_pos()
            .map(|pos| action.contains(pos))
            .unwrap_or(false);
    let glyph = if armed_delete {
        crate::CardGlyph::Check
    } else {
        crate::CardGlyph::Trash
    };
    let ink = if armed_delete {
        DANGER
    } else if over_action {
        DANGER
    } else {
        TEXT_DIM()
    };
    crate::paint_card_glyph(
        painter,
        action.center(),
        glyph,
        egui::Stroke::new(1.3_f32, ink),
        ink,
    );
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let clicked = response.clicked() && !busy;
    let on_action = response
        .interact_pointer_pos()
        .map(|pos| action.contains(pos))
        .unwrap_or(false);
    SlotReply {
        selected: clicked && !on_action,
        delete: clicked && on_action,
    }
}

/// Un emplacement libre, dessiné comme les autres pour que la rangée garde
/// ses trois colonnes. Un clic y prend une copie des sauvegardes du jeu.
fn empty_slot(ui: &mut egui::Ui, width: f32, slot: u8, busy: bool) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, SLOT_HEIGHT), egui::Sense::click());
    let hovered = response.hovered() && !busy;
    let fill = if hovered { CARD_HOVER() } else { CARD() };
    let stroke = if hovered {
        egui::Stroke::new(1.0_f32, ACCENT())
    } else {
        egui::Stroke::new(1.0_f32, BORDER())
    };
    let painter = ui.painter();
    painter.rect(rect, egui::Rounding::same(10.0), fill, stroke);
    painter.text(
        rect.center() - egui::vec2(0.0, 11.0),
        egui::Align2::CENTER_CENTER,
        format!("Emplacement {slot}"),
        egui::FontId::proportional(10.5),
        TEXT_DIM(),
    );
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "vide",
        egui::FontId::proportional(10.5),
        TEXT_DIM(),
    );
    painter.text(
        rect.center() + egui::vec2(0.0, 16.0),
        egui::Align2::CENTER_CENTER,
        lapinux_core::i18n::t("saves.save_here"),
        egui::FontId::proportional(9.5),
        if hovered { TEXT() } else { TEXT_DIM() },
    );
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

/// Lit copies et journal dans la même tâche : la fiche n'affiche pas l'un
/// avant l'autre.
fn load(client: &Client, appid: u32) -> Result<Loaded, String> {
    let snapshots = client
        .request_with_timeout(&Req::SaveSnapshots { appid }, Duration::from_secs(20))
        .map_err(|error| error.to_string())
        .and_then(|envelope| {
            envelope
                .into_data::<Vec<SaveSnapshot>>()
                .map_err(|error| error.to_string())
        })?;
    // Le journal est secondaire : une erreur de lecture le laisse vide plutôt
    // que d'empêcher les emplacements de s'afficher.
    let events = client
        .request_with_timeout(&Req::SaveHistory { appid }, Duration::from_secs(20))
        .ok()
        .and_then(|envelope| envelope.into_data::<Vec<SaveEvent>>().ok())
        .unwrap_or_default();
    Ok(Loaded { snapshots, events })
}

fn envelope_message(envelope: lapinux_core::ipc::Envelope, ok: &str) -> Result<String, String> {
    if !envelope.ok {
        return Err(envelope
            .error
            .unwrap_or_else(|| lapinux_core::i18n::t("saves.unknown_error").into()));
    }
    Ok(ok.to_string())
}

fn backup(client: &Client, appid: u32) -> Result<String, String> {
    let envelope = client
        .request_with_timeout(
            &Req::BackupSaves { appid, slot: None },
            Duration::from_secs(120),
        )
        .map_err(|error| error.to_string())?;
    if !envelope.ok {
        return Err(envelope
            .error
            .unwrap_or_else(|| lapinux_core::i18n::t("saves.copy_failed").into()));
    }
    let data = envelope.data;
    if data.get("unchanged").and_then(|value| value.as_bool()) == Some(true) {
        return Ok(lapinux_core::i18n::t("saves.unchanged").into());
    }
    let files = data
        .get("snapshot")
        .and_then(|snapshot| snapshot.get("files"))
        .and_then(|value| value.as_u64())
        .unwrap_or(0);
    Ok(lapinux_core::i18n::t_args(
        "saves.copy_taken",
        &[("files", &files.to_string())],
    ))
}

/// Retire une copie des emplacements ; le daemon supprime son dossier et son
/// manifeste, jamais les sauvegardes du jeu elles-mêmes.
fn delete(client: &Client, appid: u32, id: &str) -> Result<String, String> {
    let envelope = client
        .request_with_timeout(
            &Req::DeleteSaveSnapshot {
                appid,
                snapshot: id.to_string(),
            },
            Duration::from_secs(60),
        )
        .map_err(|error| error.to_string())?;
    envelope_message(envelope, lapinux_core::i18n::t("saves.deleted"))
}

/// Copie l'état courant dans un emplacement précis : le clic sur un carré
/// vide. Le daemon remplace l'ancienne copie de cet emplacement.
fn backup_slot(client: &Client, appid: u32, slot: u8) -> Result<String, String> {
    let envelope = client
        .request_with_timeout(
            &Req::BackupSaves {
                appid,
                slot: Some(slot),
            },
            Duration::from_secs(120),
        )
        .map_err(|error| error.to_string())?;
    if !envelope.ok {
        return Err(envelope
            .error
            .unwrap_or_else(|| lapinux_core::i18n::t("saves.copy_failed").into()));
    }
    let data = envelope.data;
    if data.get("unchanged").and_then(|value| value.as_bool()) == Some(true) {
        return Ok(lapinux_core::i18n::t_args(
            "saves.unchanged_slot",
            &[("slot", &slot.to_string())],
        ));
    }
    let files = data
        .get("snapshot")
        .and_then(|snapshot| snapshot.get("files"))
        .and_then(|value| value.as_u64())
        .unwrap_or(0);
    Ok(lapinux_core::i18n::t_args(
        "saves.copy_taken_slot",
        &[("slot", &slot.to_string()), ("files", &files.to_string())],
    ))
}

/// Applique une copie aux sauvegardes du jeu. Le jeu ne les lit qu'au
/// démarrage : quand il est installé, l'interface le relance aussitôt, et le
/// message le dit.
fn restore(client: &Client, appid: u32, id: &str, launching: bool) -> Result<String, String> {
    let envelope = client
        .request_with_timeout(
            &Req::RestoreSaves {
                appid,
                snapshot: id.to_string(),
            },
            Duration::from_secs(120),
        )
        .map_err(|error| error.to_string())?;
    if !envelope.ok {
        return Err(envelope
            .error
            .unwrap_or_else(|| lapinux_core::i18n::t("saves.load_error").into()));
    }
    let files = envelope
        .data
        .get("files")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);
    if launching {
        Ok(lapinux_core::i18n::t_args(
            "saves.restored_launch",
            &[("files", &files.to_string())],
        ))
    } else {
        Ok(lapinux_core::i18n::t_args(
            "saves.restored",
            &[("files", &files.to_string())],
        ))
    }
}

fn export(client: &Client, appid: u32, dest: &str) -> Result<String, String> {
    let envelope = client
        .request_with_timeout(
            &Req::ExportSaves {
                appid,
                dest: dest.to_string(),
            },
            Duration::from_secs(120),
        )
        .map_err(|error| error.to_string())?;
    if !envelope.ok {
        return Err(envelope
            .error
            .unwrap_or_else(|| lapinux_core::i18n::t("saves.export_failed").into()));
    }
    let path = envelope
        .data
        .get("path")
        .and_then(|value| value.as_str())
        .unwrap_or(dest)
        .to_string();
    Ok(lapinux_core::i18n::t_args(
        "saves.exported",
        &[("path", &path)],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_slots_share_the_available_width() {
        let width = 780.0;
        let each = slot_width(width, SLOT_GAP);
        assert!(
            (each * 3.0 + SLOT_GAP * 2.0 - width).abs() < 0.01,
            "les trois emplacements doivent remplir la rangée : {each}"
        );
        // Sur une fiche étroite, un emplacement garde une taille lisible.
        assert_eq!(slot_width(120.0, SLOT_GAP), 64.0);
    }

    #[test]
    fn a_copy_is_named_by_its_slot() {
        let copy = SaveSnapshot {
            slot: 2,
            ..Default::default()
        };
        assert_eq!(
            copy_place(&copy),
            lapinux_core::i18n::t_args("saves.slot", &[("slot", "2")])
        );
        let legacy = SaveSnapshot {
            slot: 0,
            created: 1_700_000_000,
            ..Default::default()
        };
        let copy_prefix = lapinux_core::i18n::t("saves.copy_of_date")
            .split("{date}")
            .next()
            .unwrap();
        assert!(copy_place(&legacy).starts_with(copy_prefix));
    }

    #[test]
    fn the_loaded_slot_stays_the_active_copy() {
        let loaded = SaveSnapshot {
            id: "100".into(),
            slot: 1,
            created: 100,
            ..Default::default()
        };
        // La copie de sécurité est plus récente, mais aucune opération ne la
        // désigne : le jeu lit toujours l'emplacement chargé.
        let safety = SaveSnapshot {
            id: "200".into(),
            slot: 2,
            created: 200,
            ..Default::default()
        };
        let events = vec![SaveEvent {
            at: 300,
            kind: SaveEventKind::Restore,
            snapshot: loaded.id.clone(),
            ..Default::default()
        }];
        let active = active_copy(&[loaded, safety], &events).unwrap();
        assert_eq!(active.id, "100");
        assert!(copy_origin(&events, &active)
            .starts_with(lapinux_core::i18n::t("saves.restored_prefix")));
    }

    #[test]
    fn the_origin_separates_automatic_and_manual_copies() {
        let copy = SaveSnapshot {
            id: "300".into(),
            slot: 3,
            created: 300,
            ..Default::default()
        };
        let auto = SaveEvent {
            at: 300,
            kind: SaveEventKind::Backup,
            snapshot: copy.id.clone(),
            auto: true,
            ..Default::default()
        };
        assert_eq!(
            copy_origin(&[auto], &copy),
            lapinux_core::i18n::t("saves.origin.auto").to_string()
        );
        let manual = SaveEvent {
            at: 400,
            kind: SaveEventKind::Backup,
            snapshot: copy.id.clone(),
            ..Default::default()
        };
        assert_eq!(
            copy_origin(&[manual], &copy),
            lapinux_core::i18n::t("saves.origin.manual").to_string()
        );
    }
}
