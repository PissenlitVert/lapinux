//! Onglet Atelier d'une fiche : nos abonnements, leurs fichiers réels et leur
//! poids, le téléchargement groupé et la matérialisation dans `Mods`.
//!
//! Le rangement écrit de vrais fichiers dans
//! `LapinuxLibrary/Mods/<jeu> (<appid>)/SteamWorkshop` à partir du contenu que
//! Steam ou DepotDownloader a déjà téléchargé. La lecture, la matérialisation
//! et le téléchargement vivent dans des fils pour ne jamais bloquer le dessin
//! de la fiche.

use crate::{muted, ACCENT, CARD_HOVER, DANGER, OK, TEXT, TEXT_DIM, WARN};
use eframe::egui;
use lapinux_core::client::Client;
use lapinux_core::ipc::Req;
use lapinux_core::model::{
    WorkshopAction, WorkshopArrangeReport, WorkshopDownloadReport, WorkshopItem,
    WorkshopManageReport, WorkshopReport,
};
use lapinux_core::util::format_bytes;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Une vignette pèse environ 0,3 Mio après réduction : le budget garde assez
/// d'images pour plusieurs écrans de cartes, sans accumuler les 139 visuels
/// d'un atelier entier.
const PREVIEW_BUDGET: usize = 28 * 1024 * 1024;
/// On ne lance que quelques téléchargements à la fois : une ouverture
/// d'atelier ne doit pas créer cent requêtes simultanées.
const PREVIEW_FETCHES: usize = 5;
/// Une vignette est réduite à la taille d'une carte HiDPI.
const PREVIEW_SIZE: (u32, u32) = (480, 270);
/// Une confirmation de suppression reste armée quelques secondes seulement.
const CONFIRM_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Default)]
pub struct Shelf {
    reports: HashMap<u32, WorkshopReport>,
    errors: HashMap<u32, String>,
    asked: HashSet<u32>,
    /// Jeux dont la prochaine lecture doit interroger Steam : l'icône de
    /// rafraîchissement relit les abonnements en ligne, pas seulement les
    /// fichiers locaux.
    online_refresh: HashSet<u32>,
    rx: Option<mpsc::Receiver<(u32, Result<WorkshopReport, String>)>>,
    loading: bool,
    arrange: Option<Arrange>,
    manage: Option<Manage>,
    confirm: Option<Confirm>,
    notice: Option<(bool, String)>,
    client: Option<Client>,
    previews: PreviewArt,
    /// Vue des fichiers réellement présents dans `SteamWorkshop`.
    local: Option<LocalView>,
    /// Dossiers repliés par l'utilisateur, désignés par leur chemin.
    collapsed: HashSet<String>,
    /// Vue par défaut : les tuiles illustrées ; `true` bascule sur les fichiers.
    file_view: bool,
    offline: Option<OfflineDownload>,
    /// Page Workshop demandée par une tuile ou un lien : l'application
    /// l'ouvre dans sa fenêtre WebKit, jamais dans le navigateur externe.
    webview_want: Option<(String, String)>,
}

#[derive(Clone, Default)]
struct LocalView {
    dir: PathBuf,
    entries: Vec<LocalEntry>,
    files: usize,
    bytes: u64,
    error: Option<String>,
}

#[derive(Clone, Default)]
struct LocalEntry {
    name: String,
    path: PathBuf,
    id: Option<u64>,
    files: Vec<LocalFile>,
    bytes: u64,
    is_link: bool,
    truncated: bool,
    error: Option<String>,
}

#[derive(Clone, Default)]
struct LocalFile {
    path: PathBuf,
    rel: String,
    bytes: u64,
}

impl LocalView {
    fn read(dir: &Path) -> Self {
        let mut view = Self {
            dir: dir.to_path_buf(),
            ..Default::default()
        };
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(error) => {
                view.error = Some(lapinux_core::i18n::t_args(
                    "workshop.folder_unreadable",
                    &[("error", &error.to_string())],
                ));
                return view;
            }
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(".lapinux") {
                continue;
            }
            let path = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            let mut local = LocalEntry {
                name,
                path: path.clone(),
                id: local_entry_id(&path),
                is_link: meta.file_type().is_symlink(),
                ..Default::default()
            };
            let Ok(followed) = std::fs::metadata(&path) else {
                local.error = Some(lapinux_core::i18n::t("workshop.broken_link").into());
                view.entries.push(local);
                continue;
            };
            if followed.is_dir() {
                collect_local_files(
                    &path,
                    &path,
                    0,
                    &mut local.files,
                    &mut local.bytes,
                    &mut local.truncated,
                    &mut local.error,
                );
            } else if followed.is_file() {
                local.bytes = followed.len();
                local.files.push(LocalFile {
                    path: path.clone(),
                    rel: local.name.clone(),
                    bytes: followed.len(),
                });
            } else {
                continue;
            }
            view.files += local.files.len();
            view.bytes += local.bytes;
            view.entries.push(local);
        }
        view.entries
            .sort_by(|left, right| left.name.to_lowercase().cmp(&right.name.to_lowercase()));
        view
    }
}

fn local_entry_id(path: &Path) -> Option<u64> {
    let name = path.file_name()?.to_str()?;
    let start = name.rfind('[')?;
    let end = name[start + 1..].strip_suffix(']')?;
    end.trim().parse().ok()
}

fn collect_local_files(
    root: &Path,
    dir: &Path,
    depth: usize,
    files: &mut Vec<LocalFile>,
    bytes: &mut u64,
    truncated: &mut bool,
    error: &mut Option<String>,
) {
    if depth > 12 || files.len() >= 500 {
        *truncated = true;
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        *error = Some(lapinux_core::i18n::t_fmt(
            "workshop.subfolder_unreadable",
            &[dir.display().to_string()],
        ));
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        if files.len() >= 500 {
            *truncated = true;
            return;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with(".lapinux") {
            continue;
        }
        let path = entry.path();
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        if meta.is_dir() {
            collect_local_files(root, &path, depth + 1, files, bytes, truncated, error);
        } else if meta.is_file() {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            files.push(LocalFile {
                path,
                rel,
                bytes: meta.len(),
            });
            *bytes += meta.len();
        }
    }
}

struct Arrange {
    appid: u32,
    rx: mpsc::Receiver<Result<WorkshopArrangeReport, String>>,
}

struct Manage {
    appid: u32,
    action: WorkshopAction,
    count: usize,
    rx: mpsc::Receiver<Result<WorkshopManageReport, String>>,
}

/// Téléchargement DepotDownloader : il ne dépend pas du client Steam et peut
/// durer plusieurs minutes, d'où un travail de fond séparé de la vérification.
struct OfflineDownload {
    appid: u32,
    count: usize,
    rx: mpsc::Receiver<Result<WorkshopDownloadReport, String>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pending {
    Uninstall(u64),
}

struct Confirm {
    appid: u32,
    pending: Pending,
    at: Instant,
}

#[derive(Clone, Copy)]
enum CardAction {
    DownloadWithoutSteam(u64),
    SetEnabled { id: u64, enabled: bool },
    Uninstall(u64),
    OpenWorkshop,
    Cancel,
}

/// Visibilité de l'onglet Atelier : tant que le rapport n'est pas lu, le
/// drapeau PICS suffit ; ensuite, seuls de vrais contenus gardent l'onglet.
/// Un atelier avec zéro abonnement ne doit pas laisser une fiche vide.
pub(crate) fn workshop_tab_visible(pics: bool, shelf: &Shelf, appid: u32) -> bool {
    match shelf.reports.get(&appid) {
        Some(_) => shelf.has_content(appid),
        None => pics,
    }
}

impl Shelf {
    /// Page Workshop à confier à la fenêtre WebKit de Lapinux.
    pub(crate) fn take_webview(&mut self) -> Option<(String, String)> {
        self.webview_want.take()
    }

    /// Prépare l'ouverture de la page Workshop du jeu dans la fenêtre Lapinux.
    fn request_workshop(&mut self, appid: u32, name: &str, report: &WorkshopReport) {
        let url = if report.workshop_url.trim().is_empty() {
            format!("https://steamcommunity.com/app/{appid}/workshop/")
        } else {
            report.workshop_url.clone()
        };
        self.webview_want = Some((url, format!("{name} — Workshop · Lapinux")));
    }

    /// Demande l'atelier une fois par jeu : les abonnements ne changent pas
    /// pendant que la fiche est ouverte. « Relire » force la lecture.
    pub fn ensure(&mut self, ctx: &egui::Context, client: &Client, appid: u32, name: &str) {
        self.client = Some(client.detached());
        if self.loading || !self.asked.insert(appid) {
            return;
        }
        let online = self.online_refresh.remove(&appid);
        let Some(client) = self.client.clone() else {
            return;
        };
        let name = name.to_string();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = client
                .request(&Req::SteamWorkshop {
                    appid,
                    name,
                    refresh: online,
                })
                .map_err(|error| error.to_string())
                .and_then(|envelope| {
                    envelope
                        .into_data::<WorkshopReport>()
                        .map_err(|error| error.to_string())
                });
            let _ = tx.send((appid, result));
        });
        self.rx = Some(rx);
        self.loading = true;
        ctx.request_repaint();
    }

    /// Un atelier local existe pour ce jeu : le drapeau PICS peut manquer,
    /// le dossier de contenu non. L'onglet s'appuie sur les deux.
    pub fn has_content(&self, appid: u32) -> bool {
        self.reports
            .get(&appid)
            .is_some_and(|report| !report.items.is_empty())
    }

    fn ensure_local(&mut self, dir: &Path) {
        if self.local.as_ref().is_some_and(|local| local.dir == dir) {
            return;
        }
        self.local = Some(LocalView::read(dir));
    }

    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.rx {
            match rx.try_recv() {
                Ok((appid, result)) => {
                    self.rx = None;
                    self.loading = false;
                    match result {
                        Ok(report) => {
                            self.errors.remove(&appid);
                            self.reports.insert(appid, report);
                            self.local = None;
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

        if let Some(job) = &self.arrange {
            match job.rx.try_recv() {
                Ok(result) => {
                    let appid = job.appid;
                    self.arrange = None;
                    match result {
                        Ok(report) => {
                            let mut text = lapinux_core::i18n::t_fmt(
                                "workshop.links_placed",
                                &[report.links.to_string(), report.dir.display().to_string()],
                            );
                            if report.removed > 0 {
                                text.push_str(&lapinux_core::i18n::t_fmt(
                                    "workshop.old_links",
                                    &[report.removed.to_string()],
                                ));
                            }
                            if report.bytes > 0 {
                                text.push_str(&format!(" · {}", format_bytes(report.bytes)));
                            }
                            self.notice = Some((true, text));
                            self.reload(appid);
                        }
                        Err(error) => {
                            self.notice = Some((false, error));
                        }
                    }
                    ctx.request_repaint();
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(Duration::from_millis(150));
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.arrange = None;
                    self.notice =
                        Some((false, lapinux_core::i18n::t("ui.workshop_no_report").into()));
                }
            }
        }

        if let Some(job) = &self.manage {
            match job.rx.try_recv() {
                Ok(result) => {
                    let appid = job.appid;
                    let action = job.action;
                    let count = job.count;
                    self.manage = None;
                    match result {
                        Ok(report) => {
                            let mut text = match action {
                                WorkshopAction::Enable => lapinux_core::i18n::t_args(
                                    "workshop.activated",
                                    &[("count", &count.to_string())],
                                ),
                                WorkshopAction::Disable => lapinux_core::i18n::t_args(
                                    "workshop.deactivated",
                                    &[("count", &count.to_string())],
                                ),
                                WorkshopAction::Uninstall => lapinux_core::i18n::t_fmt(
                                    "workshop.deleted",
                                    &[
                                        report.removed_items.to_string(),
                                        format_bytes(report.freed_bytes).to_string(),
                                    ],
                                ),
                            };
                            if action != WorkshopAction::Uninstall {
                                text.push_str(&lapinux_core::i18n::t_fmt(
                                    "workshop.active_links",
                                    &[report.links.to_string()],
                                ));
                            }
                            if report.errors.is_empty() {
                                self.notice = Some((true, text));
                            } else {
                                text.push_str(&format!(" {}", report.errors.join(" · ")));
                                self.notice = Some((false, text));
                            }
                            self.reload(appid);
                        }
                        Err(error) => {
                            self.notice = Some((false, error));
                        }
                    }
                    ctx.request_repaint();
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(Duration::from_millis(150));
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.manage = None;
                    self.notice = Some((false, lapinux_core::i18n::t("workshop.no_report").into()));
                }
            }
        }

        let Some(job) = &self.offline else {
            return;
        };
        match job.rx.try_recv() {
            Ok(result) => {
                let appid = job.appid;
                let count = job.count;
                self.offline = None;
                match result {
                    Ok(report) => {
                        let mut text = if report.downloaded.is_empty() {
                            lapinux_core::i18n::t_args(
                                "workshop.download_done",
                                &[("count", &count.to_string())],
                            )
                        } else {
                            lapinux_core::i18n::t_fmt(
                                "workshop.downloaded_via",
                                &[
                                    report.downloaded.len().to_string(),
                                    format_bytes(report.bytes).to_string(),
                                ],
                            )
                        };
                        if report.links > 0 {
                            text.push_str(&lapinux_core::i18n::t_fmt(
                                "workshop.links_in_mods",
                                &[report.links.to_string()],
                            ));
                        }
                        if report.errors.is_empty() {
                            self.notice = Some((true, text));
                        } else {
                            let details = report
                                .errors
                                .iter()
                                .take(3)
                                .cloned()
                                .collect::<Vec<_>>()
                                .join(" · ");
                            text.push_str(&format!(" {details}"));
                            self.notice = Some((false, text));
                        }
                        self.reload(appid);
                    }
                    Err(error) => self.notice = Some((false, error)),
                }
                ctx.request_repaint();
            }
            Err(mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(150));
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.offline = None;
                self.notice = Some((
                    false,
                    lapinux_core::i18n::t("ui.downloader_no_report").into(),
                ));
            }
        }
    }

    /// Relit l'Atelier en interrogeant Steam : un article suivi depuis la
    /// fenêtre web n'est pas toujours écrit dans `*_subscriptions.vdf`.
    fn refresh_online(&mut self, appid: u32) {
        self.online_refresh.insert(appid);
        self.reload(appid);
    }

    fn reload(&mut self, appid: u32) {
        self.asked.remove(&appid);
        self.errors.remove(&appid);
        self.local = None;
    }

    fn start_arrange(&mut self, appid: u32, name: &str) {
        if self.arrange.is_some() || self.manage.is_some() || self.offline.is_some() {
            return;
        }
        let Some(client) = self.client.clone() else {
            return;
        };
        let name = name.to_string();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = client
                .request(&Req::SteamWorkshopArrange { appid, name })
                .map_err(|error| error.to_string())
                .and_then(|envelope| {
                    envelope
                        .into_data::<WorkshopArrangeReport>()
                        .map_err(|error| error.to_string())
                });
            let _ = tx.send(result);
        });
        self.notice = None;
        self.confirm = None;
        self.arrange = Some(Arrange { appid, rx });
    }

    fn start_manage(&mut self, appid: u32, name: &str, ids: Vec<u64>, action: WorkshopAction) {
        if ids.is_empty()
            || self.arrange.is_some()
            || self.manage.is_some()
            || self.offline.is_some()
        {
            return;
        }
        let Some(client) = self.client.clone() else {
            return;
        };
        let count = ids.len();
        let name = name.to_string();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = client
                .request(&Req::SteamWorkshopManage {
                    appid,
                    name,
                    ids,
                    action,
                })
                .map_err(|error| error.to_string())
                .and_then(|envelope| {
                    envelope
                        .into_data::<WorkshopManageReport>()
                        .map_err(|error| error.to_string())
                });
            let _ = tx.send(result);
        });
        self.notice = None;
        self.confirm = None;
        self.manage = Some(Manage {
            appid,
            action,
            count,
            rx,
        });
    }

    /// Télécharge via DepotDownloader, sans client Steam. La session Lapinux
    /// enregistrée est réutilisée quand elle existe, sans redemander de mot de
    /// passe. Le travail long reste dans un fil : l'interface continue
    /// d'afficher l'Atelier.
    fn start_offline_download(&mut self, appid: u32, name: &str, ids: Vec<u64>) {
        if ids.is_empty()
            || self.arrange.is_some()
            || self.manage.is_some()
            || self.offline.is_some()
        {
            return;
        }
        let Some(client) = self.client.clone() else {
            return;
        };
        let count = ids.len();
        let name = name.to_string();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = client
                .request(&Req::SteamWorkshopDownload { appid, name, ids })
                .map_err(|error| error.to_string())
                .and_then(|envelope| {
                    envelope
                        .into_data::<WorkshopDownloadReport>()
                        .map_err(|error| error.to_string())
                });
            let _ = tx.send(result);
        });
        self.notice = Some((
            true,
            lapinux_core::i18n::t_args("workshop.downloading", &[("count", &count.to_string())]),
        ));
        self.confirm = None;
        self.offline = Some(OfflineDownload { appid, count, rx });
    }

    fn confirm_pending(&self, appid: u32, pending: Pending) -> bool {
        self.confirm.as_ref().is_some_and(|confirm| {
            confirm.appid == appid
                && confirm.pending == pending
                && confirm.at.elapsed() < CONFIRM_TIMEOUT
        })
    }

    fn arm_uninstall(&mut self, appid: u32, pending: Pending) {
        self.confirm = Some(Confirm {
            appid,
            pending,
            at: Instant::now(),
        });
    }

    fn handle_card_action(
        &mut self,
        appid: u32,
        name: &str,
        report: &WorkshopReport,
        action: CardAction,
    ) {
        match action {
            CardAction::DownloadWithoutSteam(id) => {
                self.start_offline_download(appid, name, vec![id])
            }
            CardAction::SetEnabled { id, enabled } => self.start_manage(
                appid,
                name,
                vec![id],
                if enabled {
                    WorkshopAction::Enable
                } else {
                    WorkshopAction::Disable
                },
            ),
            CardAction::OpenWorkshop => self.request_workshop(appid, name, report),
            CardAction::Uninstall(id) => {
                if self.confirm_pending(appid, Pending::Uninstall(id)) {
                    self.start_manage(appid, name, vec![id], WorkshopAction::Uninstall);
                } else {
                    self.arm_uninstall(appid, Pending::Uninstall(id));
                    // Une seule confirmation à la fois : les autres cartes
                    // retrouvent aussitôt leur bouton normal.
                    self.notice = Some((
                        true,
                        lapinux_core::i18n::t("workshop.confirm_delete").into(),
                    ));
                }
            }
            CardAction::Cancel => {
                if self.confirm.as_ref().is_some_and(|c| c.appid == appid) {
                    self.confirm = None;
                }
            }
        }
    }

    /// Choisit la vue par défaut : images et informations superposées, ou
    /// fichiers réels. Le choix se fait depuis l'en-tête et reste en mémoire
    /// pendant la session.
    pub fn page_view(&mut self, ui: &mut egui::Ui, appid: u32, name: &str) {
        let steam_count = self
            .reports
            .get(&appid)
            .map(|report| report.items.len())
            .unwrap_or(0);
        let width = ui.available_width();
        let rail_left = 16.0;
        let rail_width = 158.0;
        let gap = 14.0;
        let rail_end = rail_left + rail_width + gap;
        let main_width = (width - rail_end - 16.0).clamp(320.0, 780.0);
        let main_left = rail_end + ((width - rail_end - main_width) / 2.0).max(0.0);

        if main_width >= 420.0 {
            ui.horizontal_top(|ui| {
                ui.add_space(rail_left);
                ui.allocate_ui_with_layout(
                    egui::vec2(rail_width, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(rail_width);
                        sources_list(ui, steam_count);
                    },
                );
                ui.add_space((main_left - rail_left - rail_width).max(0.0));
                ui.allocate_ui_with_layout(
                    egui::vec2(main_width, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(main_width);
                        if self.file_view {
                            self.files_page(ui, appid, name);
                        } else {
                            self.images_page(ui, appid, name);
                        }
                    },
                );
            });
        } else {
            sources_list(ui, steam_count);
            ui.add_space(10.0);
            if self.file_view {
                self.files_page(ui, appid, name);
            } else {
                self.images_page(ui, appid, name);
            }
        }
    }

    /// La vue demandée : une grande vignette avec le titre, l'identifiant et le
    /// poids directement dessus, sans cadre ni bouton plein. Les fichiers
    /// réels restent accessibles par « Voir les fichiers ».
    pub fn images_page(&mut self, ui: &mut egui::Ui, appid: u32, name: &str) {
        self.poll(ui.ctx());
        self.previews.poll(ui.ctx());
        let Some(report) = self.reports.get(&appid).cloned() else {
            self.files_waiting(ui, appid);
            return;
        };
        self.ensure_local(&report.mods_dir);
        let local = self.local.clone().unwrap_or_default();
        let busy = self.arrange.as_ref().is_some_and(|job| job.appid == appid)
            || self.manage.as_ref().is_some_and(|job| job.appid == appid)
            || self.offline.as_ref().is_some_and(|job| job.appid == appid);
        let installed = report.items.iter().filter(|item| item.installed).count();
        let missing = report.items.len().saturating_sub(installed);
        let needs_files = needs_materialization(&report, &local);
        let armed_item = self.confirm.as_ref().and_then(|confirm| {
            if confirm.appid != appid || confirm.at.elapsed() >= CONFIRM_TIMEOUT {
                return None;
            }
            match confirm.pending {
                Pending::Uninstall(id) => Some(id),
            }
        });
        let mut actions = Vec::new();
        let mut arrange = false;

        let active = report
            .items
            .iter()
            .filter(|item| item.installed && !item.disabled)
            .count();
        let mut summary = lapinux_core::i18n::t_fmt(
            "workshop.mods_installed_fetch",
            &[
                report.items.len().to_string(),
                installed.to_string(),
                missing.to_string(),
            ],
        );
        if local.error.is_none() {
            summary.push_str(&lapinux_core::i18n::t_fmt(
                "workshop.files_bytes",
                &[local.files.to_string(), format_bytes(local.bytes)],
            ));
        }
        let path_text = report.mods_dir.display().to_string();
        let available = ui.available_width();
        let status_width = 150.0;
        let path_width = (available * 0.38).clamp(190.0, 320.0);
        let gap = 12.0;
        let center_width = available - status_width - path_width - gap * 2.0;
        let mut refresh = false;
        ui.add_space(1.0);
        if center_width >= 230.0 {
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(status_width, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(status_width);
                        activity_line(ui, active, report.links);
                    },
                );
                ui.add_space(gap);
                ui.allocate_ui_with_layout(
                    egui::vec2(center_width, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(center_width);
                        ui.vertical_centered(|ui| {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&summary).size(9.5).color(TEXT_DIM()),
                                )
                                .truncate(),
                            )
                            .on_hover_text(&summary);
                        });
                    },
                );
                ui.add_space(gap);
                ui.allocate_ui_with_layout(
                    egui::vec2(path_width, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(path_width);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if refresh_icon(ui, !busy && !self.loading, self.loading) {
                                refresh = true;
                            }
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&path_text)
                                        .size(8.0)
                                        .monospace()
                                        .color(TEXT_DIM()),
                                )
                                .truncate(),
                            )
                            .on_hover_text(&path_text);
                        });
                    },
                );
            });
        } else {
            ui.horizontal_top(|ui| {
                activity_line(ui, active, report.links);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if refresh_icon(ui, !busy && !self.loading, self.loading) {
                        refresh = true;
                    }
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&path_text)
                                .size(8.0)
                                .monospace()
                                .color(TEXT_DIM()),
                        )
                        .truncate(),
                    )
                    .on_hover_text(&path_text);
                });
            });
            ui.add_space(1.0);
            ui.vertical_centered(|ui| {
                ui.add(
                    egui::Label::new(egui::RichText::new(&summary).size(9.5).color(TEXT_DIM()))
                        .truncate(),
                )
                .on_hover_text(&summary);
            });
        }
        if refresh {
            self.refresh_online(appid);
            self.notice = None;
            ui.ctx().request_repaint();
        }
        let workshop_url = if report.workshop_url.trim().is_empty() {
            format!("https://steamcommunity.com/app/{}/workshop/", report.appid)
        } else {
            report.workshop_url.clone()
        };

        if needs_files {
            ui.add_enabled_ui(!busy, |ui| {
                if text_action(ui, lapinux_core::i18n::t("workshop.create_real")).clicked() {
                    arrange = true;
                }
            });
        }

        if let Some((ok, text)) = self.notice.clone() {
            if !ok {
                ui.add_space(5.0);
                notice_line(ui, false, &text);
            }
        }

        if report.items.is_empty() {
            ui.add_space(18.0);
            muted(ui, lapinux_core::i18n::t("workshop.none_in_section"));
            return;
        }

        ui.add_space(6.0);
        item_grid(
            ui,
            &report.items,
            &mut self.previews,
            armed_item,
            busy,
            false,
            Some(&workshop_url),
            &mut actions,
        );

        if arrange {
            self.start_arrange(appid, name);
        }
        for action in actions {
            self.handle_card_action(appid, name, &report, action);
        }
    }

    /// Vue brute de l'Atelier : les fichiers réellement présents dans le
    /// dossier `SteamWorkshop`, puis les contenus qui restent à récupérer.
    /// Aucune carte, aucun cadre : le nom, la taille et le chemin suffisent.
    pub fn files_page(&mut self, ui: &mut egui::Ui, appid: u32, name: &str) {
        self.poll(ui.ctx());
        let Some(report) = self.reports.get(&appid).cloned() else {
            self.files_waiting(ui, appid);
            return;
        };
        self.ensure_local(&report.mods_dir);
        let local = self.local.clone().unwrap_or_default();
        let busy = self.arrange.as_ref().is_some_and(|job| job.appid == appid)
            || self.manage.as_ref().is_some_and(|job| job.appid == appid)
            || self.offline.as_ref().is_some_and(|job| job.appid == appid);
        let installed = report.items.iter().filter(|item| item.installed).count();
        let missing: Vec<&WorkshopItem> =
            report.items.iter().filter(|item| !item.installed).collect();
        let needs_files = needs_materialization(&report, &local);
        let armed_item = self.confirm.as_ref().and_then(|confirm| {
            if confirm.appid != appid || confirm.at.elapsed() >= CONFIRM_TIMEOUT {
                return None;
            }
            match confirm.pending {
                Pending::Uninstall(id) => Some(id),
            }
        });
        let mut actions = Vec::new();
        let mut arrange = false;

        let mut summary = lapinux_core::i18n::t_fmt(
            "workshop.mods_installed",
            &[report.items.len().to_string(), installed.to_string()],
        );
        if !missing.is_empty() {
            summary.push_str(&lapinux_core::i18n::t_fmt(
                "workshop.to_fetch_suffix",
                &[missing.len().to_string()],
            ));
        }
        if local.error.is_none() {
            summary.push_str(&lapinux_core::i18n::t_fmt(
                "workshop.files_bytes",
                &[local.files.to_string(), format_bytes(local.bytes)],
            ));
        }
        ui.add_space(1.0);
        ui.horizontal(|ui| {
            ui.add(
                egui::Label::new(egui::RichText::new(name).size(14.5).strong().color(TEXT()))
                    .truncate(),
            );
            ui.add_space(8.0);
            ui.add(
                egui::Label::new(egui::RichText::new(&summary).size(9.5).color(TEXT_DIM()))
                    .truncate(),
            )
            .on_hover_text(&summary);
        });
        ui.add_space(1.0);
        let path_text = report.mods_dir.display().to_string();
        ui.add(
            egui::Label::new(
                egui::RichText::new(&path_text)
                    .size(8.5)
                    .monospace()
                    .color(TEXT_DIM()),
            )
            .truncate(),
        )
        .on_hover_text(&path_text);

        if needs_files {
            ui.add_space(2.0);
            ui.add_enabled_ui(!busy, |ui| {
                if text_action(ui, lapinux_core::i18n::t("workshop.create_real")).clicked() {
                    arrange = true;
                }
            });
        }

        if let Some((ok, text)) = self.notice.clone() {
            ui.add_space(7.0);
            notice_line(ui, ok, &text);
        }

        if report.items.is_empty() {
            ui.add_space(18.0);
            muted(ui, lapinux_core::i18n::t("workshop.no_subscription"));
        }

        ui.add_space(8.0);
        files_section(
            ui,
            &local,
            &mut self.collapsed,
            &mut actions,
            busy,
            armed_item,
        );

        if !missing.is_empty() {
            ui.add_space(20.0);
            missing_section(ui, &missing, &mut actions, busy);
        }

        ui.add_space(12.0);
        muted(ui, lapinux_core::i18n::t("workshop.real_files_hint"));

        if arrange {
            self.start_arrange(appid, name);
        }
        for action in actions {
            self.handle_card_action(appid, name, &report, action);
        }
    }

    fn files_waiting(&mut self, ui: &mut egui::Ui, appid: u32) {
        ui.add_space(24.0);
        if let Some(error) = self.errors.get(&appid).cloned() {
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new(&error).size(11.0).color(WARN));
                ui.add_space(8.0);
                if text_action(ui, lapinux_core::i18n::t("configurations.retry")).clicked() {
                    self.reload(appid);
                }
            });
        } else {
            ui.vertical_centered(|ui| {
                crate::section_icons::loading(ui, 18.0);
                ui.add_space(6.0);
                muted(ui, lapinux_core::i18n::t("ui.reading_mods"));
            });
        }
    }
}

fn needs_materialization(report: &WorkshopReport, local: &LocalView) -> bool {
    let installed: Vec<&WorkshopItem> = report.items.iter().filter(|item| item.installed).collect();
    if installed.is_empty() {
        return false;
    }
    if local.error.is_some() || local.entries.iter().any(|entry| entry.is_link) {
        return true;
    }
    installed.iter().any(|item| {
        !local
            .entries
            .iter()
            .any(|entry| entry.id == Some(item.published_file_id))
    })
}

fn section_heading(ui: &mut egui::Ui, label: &str, detail: &str) {
    ui.horizontal(|ui| {
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
    ui.add(egui::Button::new(egui::RichText::new(label).size(10.0).color(ACCENT())).frame(false))
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

fn files_section(
    ui: &mut egui::Ui,
    local: &LocalView,
    collapsed: &mut HashSet<String>,
    actions: &mut Vec<CardAction>,
    busy: bool,
    armed_item: Option<u64>,
) {
    section_heading(
        ui,
        lapinux_core::i18n::t("workshop.real_files_upper"),
        &lapinux_core::i18n::t_fmt(
            "workshop.dirs_files_bytes",
            &[
                local.entries.len().to_string(),
                local.files.to_string(),
                format_bytes(local.bytes),
            ],
        ),
    );
    if let Some(error) = &local.error {
        ui.add_space(5.0);
        muted(ui, error);
        return;
    }
    if local.entries.is_empty() {
        ui.add_space(5.0);
        muted(ui, lapinux_core::i18n::t("workshop.no_materialized"));
        return;
    }

    ui.add_space(3.0);
    for entry in &local.entries {
        let key = format!("{}::{}", local.dir.display(), entry.name);
        let expanded = !collapsed.contains(&key);
        let response = local_folder_row(ui, entry, expanded);
        if response.clicked() {
            if expanded {
                collapsed.insert(key);
            } else {
                collapsed.remove(&key);
            }
        }
        if let Some(id) = entry.id {
            if !busy {
                response
                    .on_hover_text(lapinux_core::i18n::t("workshop.context_hint"))
                    .context_menu(|ui| {
                        if ui
                            .button(lapinux_core::i18n::t("settings.appearance.disable"))
                            .clicked()
                        {
                            actions.push(CardAction::SetEnabled { id, enabled: false });
                            ui.close_menu();
                        }
                        let armed = armed_item == Some(id);
                        if ui
                            .button(if armed {
                                lapinux_core::i18n::t("workshop.confirm_delete")
                            } else {
                                lapinux_core::i18n::t("game_contents.uninstall")
                            })
                            .clicked()
                        {
                            actions.push(CardAction::Uninstall(id));
                            ui.close_menu();
                        }
                        if armed
                            && ui
                                .button(lapinux_core::i18n::t("workshop.cancel"))
                                .clicked()
                        {
                            actions.push(CardAction::Cancel);
                            ui.close_menu();
                        }
                    });
            }
        }
        if !expanded {
            continue;
        }
        if let Some(error) = &entry.error {
            ui.add_space(2.0);
            muted(ui, error);
        }
        if entry.files.is_empty() && entry.error.is_none() {
            ui.horizontal(|ui| {
                ui.add_space(28.0);
                muted(ui, lapinux_core::i18n::t("workshop.empty_folder"));
            });
        } else {
            for file in &entry.files {
                file_row(ui, file);
            }
        }
        if entry.truncated {
            ui.horizontal(|ui| {
                ui.add_space(28.0);
                muted(ui, lapinux_core::i18n::t("workshop.list_limited"));
            });
        }
    }
}

fn local_folder_row(ui: &mut egui::Ui, entry: &LocalEntry, expanded: bool) -> egui::Response {
    let height = 25.0;
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::click(),
    );
    let response = response.on_hover_text(entry.path.display().to_string());
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, egui::Rounding::same(4.0), CARD_HOVER());
    }
    let content = rect.shrink2(egui::vec2(8.0, 3.0));
    ui.scope_builder(egui::UiBuilder::new().max_rect(content), |ui| {
        ui.set_clip_rect(rect);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.label(
                egui::RichText::new(if expanded { "▾" } else { "▸" })
                    .size(11.0)
                    .color(ACCENT()),
            );
            let name_color = if entry.error.is_some() { WARN } else { TEXT() };
            let name_width = (ui.available_width() - 78.0).max(60.0);
            ui.add_sized(
                [name_width, 18.0],
                egui::Label::new(
                    egui::RichText::new(&entry.name)
                        .size(11.5)
                        .strong()
                        .color(name_color),
                )
                .truncate(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(format_bytes(entry.bytes))
                        .size(10.0)
                        .monospace()
                        .color(TEXT_DIM()),
                );
            });
        });
    });
    response
}

fn file_row(ui: &mut egui::Ui, file: &LocalFile) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.add_space(25.0);
        ui.label(egui::RichText::new("·").size(11.0).color(TEXT_DIM()));
        let width = (ui.available_width() - 76.0).max(60.0);
        ui.add_sized(
            [width, 18.0],
            egui::Label::new(
                egui::RichText::new(&file.rel)
                    .size(10.5)
                    .monospace()
                    .color(TEXT()),
            )
            .truncate(),
        )
        .on_hover_text(file.path.display().to_string());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(format_bytes(file.bytes))
                    .size(9.5)
                    .monospace()
                    .color(TEXT_DIM()),
            );
        });
    });
}

fn missing_section(
    ui: &mut egui::Ui,
    items: &[&WorkshopItem],
    actions: &mut Vec<CardAction>,
    busy: bool,
) {
    section_heading(
        ui,
        lapinux_core::i18n::t("workshop.to_fetch_upper"),
        &lapinux_core::i18n::t_fmt("workshop.not_downloaded", &[items.len().to_string()]),
    );
    ui.add_space(3.0);
    for item in items {
        let title = if item.title.trim().is_empty() {
            format!("Contenu {}", item.published_file_id)
        } else {
            item.title.clone()
        };
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let width = (ui.available_width() - 94.0).max(90.0);
            ui.add_sized(
                [width, 18.0],
                egui::Label::new(
                    egui::RichText::new(&title)
                        .size(11.5)
                        .strong()
                        .color(TEXT()),
                )
                .truncate(),
            )
            .on_hover_text(&title);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    if text_action(ui, lapinux_core::i18n::t("sheet_saves.fetch"))
                        .on_hover_text(lapinux_core::i18n::t("workshop.download_hint"))
                        .clicked()
                    {
                        actions.push(CardAction::DownloadWithoutSteam(item.published_file_id));
                    }
                });
            });
        });
        ui.horizontal(|ui| {
            ui.add_space(2.0);
            muted(
                ui,
                if item.size > 0 {
                    format!("#{} · {}", item.published_file_id, format_bytes(item.size))
                } else {
                    format!("#{}", item.published_file_id)
                },
            );
        });
    }
}

fn danger_link(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(egui::Button::new(egui::RichText::new(text).size(11.0).color(DANGER)).frame(false))
}

fn grid_columns(width: f32) -> usize {
    if width >= 760.0 {
        3
    } else if width >= 460.0 {
        2
    } else {
        1
    }
}

/// Future navigation de l'Atelier : Steam Workshop aujourd'hui, les autres
/// sources de mods demain. La liste reste volontairement sans cadre ni bouton.
fn sources_list(ui: &mut egui::Ui, steam_count: usize) {
    ui.label(
        egui::RichText::new(lapinux_core::i18n::t("ui.sources"))
            .size(9.0)
            .strong()
            .color(ACCENT()),
    );
    ui.add_space(5.0);
    source_entry(ui, "Steam Workshop", Some(steam_count), true, None);
    ui.add_space(3.0);
    source_entry(
        ui,
        "Nexus Mods",
        None,
        false,
        Some(lapinux_core::i18n::t("workshop.soon")),
    );
}

fn activity_line(ui: &mut egui::Ui, active: usize, links: usize) {
    let color = if active > 0 { OK } else { TEXT_DIM() };
    let row = ui.horizontal(|ui| {
        let (dot, _) = ui.allocate_exact_size(egui::vec2(6.0, 6.0), egui::Sense::hover());
        ui.painter().circle_filled(dot.center(), 3.0, color);
        ui.label(
            egui::RichText::new(lapinux_core::i18n::t_args(
                "workshop.active_links_pair",
                &[
                    ("active", &active.to_string()),
                    ("links", &links.to_string()),
                ],
            ))
            .size(9.5)
            .color(color),
        );
    });
    row.response
        .on_hover_text(lapinux_core::i18n::t("workshop.subtitle"));
}

/// Icône de relecture : un arc presque complet et sa flèche, dessinés au trait
/// pour ne pas dépendre d'un glyphe absent du sous-ensemble Remix embarqué.
fn refresh_icon(ui: &mut egui::Ui, enabled: bool, loading: bool) -> bool {
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), sense);
    let hovered = enabled && response.hovered();
    let color = if !enabled {
        TEXT_DIM().gamma_multiply(0.4)
    } else if hovered {
        TEXT()
    } else {
        TEXT_DIM()
    };
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        ui.painter()
            .rect_filled(rect, egui::Rounding::same(5.0), CARD_HOVER());
    }

    let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(rect));
    let center = rect.center();
    let radius = 5.2_f32;
    let start = -0.15 * std::f32::consts::PI;
    let sweep = 1.7 * std::f32::consts::PI;
    let segments = 20;
    let mut points = Vec::with_capacity(segments + 1);
    for index in 0..=segments {
        let angle = start + sweep * index as f32 / segments as f32;
        points.push(center + egui::vec2(angle.cos(), angle.sin()) * radius);
    }
    painter.add(egui::Shape::line(
        points,
        egui::Stroke::new(1.35_f32, color),
    ));

    // La pointe suit la tangente de l'arc : le cercle se lit dans le sens
    // horaire, avec l'ouverture en haut à droite.
    let end_angle = start + sweep;
    let end = center + egui::vec2(end_angle.cos(), end_angle.sin()) * radius;
    let tangent = egui::vec2(-end_angle.sin(), end_angle.cos());
    let normal = egui::vec2(end_angle.cos(), end_angle.sin());
    let tip = end + tangent * 2.4;
    let base = end - tangent * 1.0;
    painter.add(egui::Shape::convex_polygon(
        vec![tip, base + normal * 1.7, base - normal * 1.7],
        color,
        egui::Stroke::new(0.0_f32, egui::Color32::TRANSPARENT),
    ));

    let clicked = response.clicked();
    let hint = if loading {
        lapinux_core::i18n::t("ui.reloading")
    } else if !enabled {
        lapinux_core::i18n::t("workshop.wait")
    } else {
        lapinux_core::i18n::t("workshop.refresh")
    };
    response.on_hover_text(hint);
    clicked
}

fn source_entry(
    ui: &mut egui::Ui,
    label: &str,
    count: Option<usize>,
    active: bool,
    note: Option<&str>,
) {
    let row = ui.horizontal(|ui| {
        let (dot, _) = ui.allocate_exact_size(egui::vec2(6.0, 6.0), egui::Sense::hover());
        ui.painter().circle_filled(
            dot.center(),
            3.0,
            if active {
                ACCENT()
            } else {
                TEXT_DIM().gamma_multiply(0.55)
            },
        );
        ui.label(
            egui::RichText::new(label)
                .size(11.5)
                .strong()
                .color(if active { TEXT() } else { TEXT_DIM() }),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if let Some(count) = count {
                ui.label(
                    egui::RichText::new(count.to_string())
                        .size(9.5)
                        .color(if active { ACCENT() } else { TEXT_DIM() }),
                );
            } else if let Some(note) = note {
                ui.label(egui::RichText::new(note).size(9.0).color(TEXT_DIM()));
            }
        });
    });
    row.response.on_hover_text(if active {
        lapinux_core::i18n::t("workshop.source_active_hint")
    } else {
        lapinux_core::i18n::t("workshop.source_coming")
    });
}

fn compact_columns(width: f32) -> usize {
    if width >= 960.0 {
        4
    } else if width >= 700.0 {
        3
    } else if width >= 440.0 {
        2
    } else {
        1
    }
}

fn item_grid(
    ui: &mut egui::Ui,
    items: &[WorkshopItem],
    previews: &mut PreviewArt,
    armed_item: Option<u64>,
    busy: bool,
    compact: bool,
    workshop_url: Option<&str>,
    actions: &mut Vec<CardAction>,
) {
    let available = ui.available_width().max(260.0);
    let columns = if compact {
        compact_columns(available)
    } else {
        grid_columns(available)
    };
    let gap = 10.0;
    let card_width =
        ((available - gap * (columns.saturating_sub(1) as f32)) / columns as f32).max(220.0);

    let total = items.len() + if workshop_url.is_some() { 1 } else { 0 };
    for start in (0..total).step_by(columns) {
        let row_len = (total - start).min(columns);
        // Une rangée incomplète est centrée : deux cartes ne restent pas
        // collées à gauche d'une large fiche avec un grand vide à droite.
        let row_width = card_width * row_len as f32 + gap * (row_len.saturating_sub(1) as f32);
        let left = ((available - row_width) / 2.0).max(0.0);
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            if left > 0.0 {
                ui.add_space(left);
            }
            for offset in 0..row_len {
                let index = start + offset;
                if index < items.len() {
                    let item = &items[index];
                    if let Some(action) = item_card(
                        ui,
                        item,
                        card_width.min(ui.available_width()),
                        previews,
                        armed_item == Some(item.published_file_id),
                        busy,
                        compact,
                    ) {
                        actions.push(action);
                    }
                } else if workshop_url.is_some() {
                    if let Some(action) =
                        workshop_card(ui, card_width.min(ui.available_width()), compact)
                    {
                        actions.push(action);
                    }
                }
            }
        });
        ui.add_space(gap);
    }
}

/// Tuile de fin de grille : elle ressemble aux autres mods, mais ouvre la page
/// Steam Workshop du jeu dans la fenêtre WebKit de Lapinux.
fn workshop_card(ui: &mut egui::Ui, width: f32, compact: bool) -> Option<CardAction> {
    let card_width = width.max(220.0);
    let (image_height, overlay_height, scrim_height) = if compact {
        ((card_width * 0.45).clamp(144.0, 184.0), 72.0, 92.0)
    } else {
        ((card_width * 0.58).clamp(188.0, 268.0), 90.0, 120.0)
    };
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(card_width, image_height), egui::Sense::click());
    paint_workshop_preview(ui, rect);
    paint_bottom_scrim(ui.painter(), rect, scrim_height);

    let overlay_bottom = rect.bottom() - if compact { 8.0 } else { 10.0 };
    let overlay = egui::Rect::from_min_max(
        egui::pos2(rect.left() + 12.0, rect.bottom() - overlay_height),
        egui::pos2(rect.right() - 12.0, overlay_bottom),
    );
    ui.scope_builder(egui::UiBuilder::new().max_rect(overlay), |ui| {
        ui.set_clip_rect(ui.clip_rect().intersect(rect));
        ui.set_width(overlay.width());
        ui.spacing_mut().item_spacing = egui::vec2(6.0, 2.0);
        let stack_gap = if compact { 1.0 } else { 2.0 };
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            ui.label(
                egui::RichText::new(lapinux_core::i18n::t("workshop.open_page"))
                    .size(if compact { 10.0 } else { 11.0 })
                    .strong()
                    .color(egui::Color32::from_white_alpha(235)),
            );
            ui.add_space(stack_gap);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(lapinux_core::i18n::t("workshop.all_mods"))
                        .size(if compact { 9.5 } else { 10.5 })
                        .color(egui::Color32::from_white_alpha(215)),
                )
                .truncate(),
            );
            ui.add_space(stack_gap);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(lapinux_core::i18n::t("ui.browse_workshop"))
                        .size(if compact { 13.0 } else { 15.0 })
                        .strong()
                        .color(TEXT()),
                )
                .truncate(),
            );
        });
    });

    let clip = ui.clip_rect().intersect(rect);
    paint_status(
        &ui.painter().with_clip_rect(clip),
        rect.left_top() + egui::vec2(9.0, 9.0),
        "WORKSHOP",
        ACCENT(),
    );

    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let clicked = response.clicked();
    response.on_hover_text(lapinux_core::i18n::t("workshop.open_page_hint"));
    clicked.then_some(CardAction::OpenWorkshop)
}

fn paint_workshop_preview(ui: &mut egui::Ui, rect: egui::Rect) {
    let clip = ui.clip_rect().intersect(rect);
    let painter = ui.painter().with_clip_rect(clip);
    let rounding = egui::Rounding::same(9.0);
    painter.rect_filled(rect, rounding, CARD_HOVER());

    let top = CARD_HOVER();
    let bottom = blend(top, ACCENT(), 0.16);
    let bands = 24;
    for index in 0..bands {
        let t = (index as f32 + 1.0) / bands as f32;
        let y0 = rect.top() + rect.height() * index as f32 / bands as f32;
        let y1 = rect.top() + rect.height() * (index + 1) as f32 / bands as f32;
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(rect.left() + 1.0, y0),
                egui::pos2(rect.right() - 1.0, y1),
            ),
            0.0,
            blend(top, bottom, t),
        );
    }

    // Une grille de petites vignettes rappelle la page Workshop, sans image
    // réseau supplémentaire.
    let pad = 14.0;
    let columns = 4;
    let rows = 3;
    let gap = 5.0;
    let tile_width = (rect.width() - pad * 2.0 - gap * (columns - 1) as f32) / columns as f32;
    let tile_height = (rect.height() * 0.48 - gap * (rows - 1) as f32) / rows as f32;
    let start_y = rect.top() + 17.0;
    for row in 0..rows {
        for column in 0..columns {
            let tile = egui::Rect::from_min_size(
                egui::pos2(
                    rect.left() + pad + column as f32 * (tile_width + gap),
                    start_y + row as f32 * (tile_height + gap),
                ),
                egui::vec2(tile_width, tile_height),
            );
            painter.rect_filled(
                tile,
                egui::Rounding::same(3.0),
                egui::Color32::from_white_alpha(11),
            );
            painter.rect_stroke(
                tile,
                egui::Rounding::same(3.0),
                egui::Stroke::new(1.0_f32, ACCENT().gamma_multiply(0.14)),
            );
        }
    }
    painter.text(
        rect.center() + egui::vec2(0.0, -2.0),
        egui::Align2::CENTER_CENTER,
        "+",
        egui::FontId::proportional(28.0),
        egui::Color32::from_white_alpha(90),
    );
}

fn item_card(
    ui: &mut egui::Ui,
    item: &WorkshopItem,
    width: f32,
    previews: &mut PreviewArt,
    armed: bool,
    busy: bool,
    compact: bool,
) -> Option<CardAction> {
    let title = if item.title.trim().is_empty() {
        format!("Contenu {}", item.published_file_id)
    } else {
        item.title.clone()
    };
    let card_width = width.max(220.0);
    let (image_height, overlay_height, scrim_height) = if compact {
        ((card_width * 0.45).clamp(144.0, 184.0), 72.0, 92.0)
    } else {
        ((card_width * 0.58).clamp(188.0, 268.0), 90.0, 120.0)
    };
    let mut action = None;

    egui::Frame::none()
        .fill(egui::Color32::TRANSPARENT)
        .inner_margin(0.0)
        .show(ui, |ui| {
            ui.set_width(card_width);
            let (rect, _) =
                ui.allocate_exact_size(egui::vec2(card_width, image_height), egui::Sense::hover());
            paint_preview(ui, rect, &title, item, previews);
            paint_bottom_scrim(ui.painter(), rect, scrim_height);

            // Le bas de l'image porte le titre, le poids et les actions.
            let overlay_bottom = rect.bottom() - if compact { 8.0 } else { 10.0 };
            let overlay = egui::Rect::from_min_max(
                egui::pos2(rect.left() + 12.0, rect.bottom() - overlay_height),
                egui::pos2(rect.right() - 12.0, overlay_bottom),
            );
            ui.scope_builder(egui::UiBuilder::new().max_rect(overlay), |ui| {
                // La tuile peut dépasser le bas du viewport quand la fiche est
                // courte. Son propre clip ne doit pas remplacer celui de la
                // zone de défilement, sinon le texte passe sous les tags.
                ui.set_clip_rect(ui.clip_rect().intersect(rect));
                ui.set_width(overlay.width());
                ui.spacing_mut().item_spacing = egui::vec2(6.0, 2.0);
                let stack_gap = if compact { 1.0 } else { 2.0 };
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    ui.add_enabled_ui(!busy, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            if item.installed {
                                if ui
                                    .link(
                                        egui::RichText::new(if item.disabled {
                                            lapinux_core::i18n::t("workshop.activate")
                                        } else {
                                            lapinux_core::i18n::t("settings.appearance.disable")
                                        })
                                        .size(if compact { 10.0 } else { 11.0 })
                                        .color(ACCENT()),
                                    )
                                    .clicked()
                                {
                                    action = Some(CardAction::SetEnabled {
                                        id: item.published_file_id,
                                        enabled: item.disabled,
                                    });
                                }
                                if danger_link(
                                    ui,
                                    if armed {
                                        lapinux_core::i18n::t("workshop.confirm")
                                    } else {
                                        lapinux_core::i18n::t("game_contents.uninstall")
                                    },
                                )
                                .clicked()
                                {
                                    action = Some(CardAction::Uninstall(item.published_file_id));
                                }
                                if armed
                                    && ui
                                        .small_button(lapinux_core::i18n::t("workshop.cancel"))
                                        .clicked()
                                {
                                    action = Some(CardAction::Cancel);
                                }
                            } else {
                                if overlay_primary(
                                    ui,
                                    lapinux_core::i18n::t("workshop.download"),
                                    compact,
                                )
                                .on_hover_text(lapinux_core::i18n::t("workshop.depot_hint"))
                                .clicked()
                                {
                                    action = Some(CardAction::DownloadWithoutSteam(
                                        item.published_file_id,
                                    ));
                                }
                            }
                        });
                    });
                    ui.add_space(stack_gap);
                    let meta = if item.size > 0 {
                        format!("#{} · {}", item.published_file_id, format_bytes(item.size))
                    } else {
                        format!("#{}", item.published_file_id)
                    };
                    let updated = if item.time_updated > 0 {
                        lapinux_core::i18n::t_fmt(
                            "workshop.updated_suffix",
                            &[lapinux_core::util::format_date(item.time_updated).to_string()],
                        )
                    } else {
                        String::new()
                    };
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&meta)
                                .size(if compact { 9.5 } else { 10.5 })
                                .color(egui::Color32::from_white_alpha(215)),
                        )
                        .truncate(),
                    )
                    .on_hover_text(format!("{meta}{updated}"));
                    ui.add_space(stack_gap);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&title)
                                .size(if compact { 13.0 } else { 15.0 })
                                .strong()
                                .color(TEXT()),
                        )
                        .truncate(),
                    )
                    .on_hover_text(&title);
                });
            });
        });
    action
}

fn overlay_primary(ui: &mut egui::Ui, text: &str, compact: bool) -> egui::Response {
    ui.add(
        egui::Button::new(
            egui::RichText::new(text)
                .size(if compact { 10.5 } else { 11.5 })
                .strong()
                .color(egui::Color32::from_white_alpha(235)),
        )
        .frame(false)
        .min_size(egui::vec2(0.0, if compact { 26.0 } else { 30.0 })),
    )
}

fn paint_bottom_scrim(painter: &egui::Painter, rect: egui::Rect, height: f32) {
    let top = rect.bottom() - height;
    let bands = 28;
    for index in 0..bands {
        let t0 = index as f32 / bands as f32;
        let t1 = (index + 1) as f32 / bands as f32;
        let alpha = (t1 * t1 * 238.0).round() as u8;
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(rect.left(), top + height * t0),
                egui::pos2(rect.right(), top + height * t1 + 0.5),
            ),
            0.0,
            egui::Color32::from_black_alpha(alpha),
        );
    }
}

fn paint_preview(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    title: &str,
    item: &WorkshopItem,
    previews: &mut PreviewArt,
) {
    // Une carte hors de la zone visible ne lance aucune requête : un grand
    // atelier ne télécharge pas ses centaines de vignettes d'un coup, et le
    // cache ne les fait pas revenir en boucle après éviction.
    if ui.is_rect_visible(rect) {
        previews.request(ui.ctx(), &item.preview_url);
    }
    let clip = ui.clip_rect().intersect(rect);
    let painter = ui.painter().with_clip_rect(clip);
    let rounding = egui::Rounding::same(9.0);
    painter.rect_filled(rect, rounding, CARD_HOVER());

    // Un dégradé discret remplace le rectangle plat pendant le chargement et
    // donne une identité aux contenus sans visuel.
    let top = CARD_HOVER();
    let bottom = blend(top, ACCENT(), 0.16);
    let bands = 24;
    for index in 0..bands {
        let t = (index as f32 + 1.0) / bands as f32;
        let y0 = rect.top() + rect.height() * index as f32 / bands as f32;
        let y1 = rect.top() + rect.height() * (index + 1) as f32 / bands as f32;
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(rect.left() + 1.0, y0),
                egui::pos2(rect.right() - 1.0, y1),
            ),
            0.0,
            blend(top, bottom, t),
        );
    }

    if let Some(texture) = previews.texture(&item.preview_url) {
        paint_cover(&painter, rect, &texture);
    } else {
        let center = rect.center();
        painter.circle_stroke(
            center + egui::vec2(rect.width() * 0.30, -rect.height() * 0.21),
            42.0,
            egui::Stroke::new(1.0_f32, ACCENT().gamma_multiply(0.13)),
        );
        painter.circle_stroke(
            center + egui::vec2(-rect.width() * 0.34, rect.height() * 0.28),
            28.0,
            egui::Stroke::new(1.0_f32, ACCENT().gamma_multiply(0.10)),
        );
        let initial = title
            .chars()
            .find(|ch| ch.is_alphanumeric())
            .map(|ch| ch.to_uppercase().to_string())
            .unwrap_or_else(|| "?".into());
        painter.text(
            center + egui::vec2(0.0, -4.0),
            egui::Align2::CENTER_CENTER,
            initial,
            egui::FontId::proportional(38.0),
            ACCENT().gamma_multiply(0.78),
        );
        painter.text(
            center + egui::vec2(0.0, 28.0),
            egui::Align2::CENTER_CENTER,
            "CONTENU WORKSHOP",
            egui::FontId::proportional(8.5),
            TEXT_DIM(),
        );
    }

    if item.disabled {
        painter.rect_filled(rect, rounding, egui::Color32::from_black_alpha(125));
    }
    let (label, color) = if item.disabled {
        (lapinux_core::i18n::t("workshop.disabled_state"), TEXT_DIM())
    } else if item.installed {
        (lapinux_core::i18n::t("workshop.installed_state"), OK)
    } else {
        (lapinux_core::i18n::t("workshop.to_download"), WARN)
    };
    paint_status(
        &painter,
        rect.left_top() + egui::vec2(9.0, 9.0),
        label,
        color,
    );
}

fn blend(from: egui::Color32, to: egui::Color32, amount: f32) -> egui::Color32 {
    let amount = amount.clamp(0.0, 1.0);
    let mix = |a: u8, b: u8| (a as f32 * (1.0 - amount) + b as f32 * amount).round() as u8;
    egui::Color32::from_rgb(
        mix(from.r(), to.r()),
        mix(from.g(), to.g()),
        mix(from.b(), to.b()),
    )
}

fn paint_cover(painter: &egui::Painter, rect: egui::Rect, texture: &egui::TextureHandle) {
    let size = texture.size_vec2();
    if size.x <= 0.0 || size.y <= 0.0 {
        return;
    }
    let target = rect.width() / rect.height().max(1.0);
    let source = if size.x / size.y > target {
        let width = size.y * target;
        egui::Rect::from_min_size(
            egui::pos2((size.x - width) / 2.0, 0.0),
            egui::vec2(width, size.y),
        )
    } else {
        let height = size.x / target;
        egui::Rect::from_min_size(
            egui::pos2(0.0, (size.y - height) / 2.0),
            egui::vec2(size.x, height),
        )
    };
    let uv = egui::Rect::from_min_max(
        egui::pos2(source.left() / size.x, source.top() / size.y),
        egui::pos2(source.right() / size.x, source.bottom() / size.y),
    );
    painter.image(texture.id(), rect, uv, egui::Color32::WHITE);
}

fn paint_status(painter: &egui::Painter, origin: egui::Pos2, label: &str, color: egui::Color32) {
    let galley = painter.layout_no_wrap(label.to_string(), egui::FontId::proportional(10.0), color);
    let rect = egui::Rect::from_min_size(
        origin,
        egui::vec2(galley.size().x + 12.0, galley.size().y + 6.0),
    );
    painter.rect_filled(
        rect,
        egui::Rounding::same(5.0),
        egui::Color32::from_black_alpha(205),
    );
    painter.galley(rect.min + egui::vec2(6.0, 3.0), galley, color);
}

/// Cache des vignettes : les téléchargements sont limités, décodés une fois
/// puis gardés sous un budget. Le CDN communautaire peut être lent ; la carte
/// affiche son visuel de repli sans bloquer la fiche.
#[derive(Default)]
struct PreviewArt {
    textures: HashMap<String, egui::TextureHandle>,
    pending: HashSet<String>,
    failed: HashSet<String>,
    budget: Option<crate::TextureBudget<String>>,
    tx: Option<mpsc::Sender<(String, Option<Vec<u8>>)>>,
    rx: Option<mpsc::Receiver<(String, Option<Vec<u8>>)>>,
}

impl PreviewArt {
    /// Demande le visuel d'une carte visible. Le téléchargement reste borné
    /// et ne concerne jamais un contenu dont la carte est hors écran.
    fn request(&mut self, ctx: &egui::Context, url: &str) {
        if url.is_empty()
            || self.textures.contains_key(url)
            || self.pending.contains(url)
            || self.failed.contains(url)
        {
            return;
        }
        if self.pending.len() >= PREVIEW_FETCHES {
            return;
        }
        if self.tx.is_none() {
            let (tx, rx) = mpsc::channel();
            self.tx = Some(tx);
            self.rx = Some(rx);
        }
        let Some(tx) = self.tx.clone() else {
            return;
        };
        let url = url.to_string();
        self.pending.insert(url.clone());
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let bytes = crate::achievements::http_bytes(&url);
            if tx.send((url, bytes)).is_ok() {
                ctx.request_repaint();
            }
        });
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.rx else {
            return;
        };
        let mut received = Vec::new();
        while let Ok(message) = rx.try_recv() {
            received.push(message);
        }
        for (url, bytes) in received {
            self.pending.remove(&url);
            let Some(bytes) = bytes else {
                self.failed.insert(url);
                continue;
            };
            if let Some(texture) = preview_texture(ctx, &url, &bytes) {
                self.budget
                    .get_or_insert_with(|| crate::TextureBudget::new(PREVIEW_BUDGET))
                    .note(url.clone(), &texture);
                self.textures.insert(url, texture);
            } else {
                self.failed.insert(url);
            }
        }
        if let Some(budget) = &mut self.budget {
            for url in budget.trim() {
                self.textures.remove(&url);
            }
        }
    }

    fn texture(&self, url: &str) -> Option<egui::TextureHandle> {
        self.textures.get(url).cloned()
    }
}

fn preview_texture(ctx: &egui::Context, url: &str, bytes: &[u8]) -> Option<egui::TextureHandle> {
    let image = image::load_from_memory(bytes).ok()?;
    let image = image.thumbnail(PREVIEW_SIZE.0, PREVIEW_SIZE.1).to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    if size[0] == 0 || size[1] == 0 {
        return None;
    }
    let color = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
    Some(ctx.load_texture(
        format!("workshop-preview:{url}"),
        color,
        egui::TextureOptions::LINEAR,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn un_atelier_local_ouvre_l_onglet_sans_drapeau_pics() {
        let mut shelf = Shelf::default();
        assert!(!workshop_tab_visible(false, &shelf, 431960));
        shelf.reports.insert(
            431960,
            WorkshopReport {
                appid: 431960,
                supported: true,
                items: vec![WorkshopItem {
                    published_file_id: 3719150807,
                    installed: true,
                    ..Default::default()
                }],
                ..Default::default()
            },
        );
        assert!(workshop_tab_visible(false, &shelf, 431960));
        assert!(workshop_tab_visible(true, &Shelf::default(), 431960));
    }

    #[test]
    fn un_atelier_vide_ne_garde_pas_l_onglet_meme_avec_pics() {
        let mut shelf = Shelf::default();
        shelf.reports.insert(
            480,
            WorkshopReport {
                appid: 480,
                supported: true,
                ..Default::default()
            },
        );
        assert!(!workshop_tab_visible(true, &shelf, 480));
    }

    #[test]
    fn la_grille_s_adapte_a_la_largeur_de_la_fiche() {
        assert_eq!(grid_columns(420.0), 1);
        assert_eq!(grid_columns(459.0), 1);
        assert_eq!(grid_columns(460.0), 2);
        assert_eq!(grid_columns(759.0), 2);
        assert_eq!(grid_columns(760.0), 3);
    }

    #[test]
    fn la_grille_compacte_affiche_plus_de_colonnes() {
        assert_eq!(compact_columns(439.0), 1);
        assert_eq!(compact_columns(440.0), 2);
        assert_eq!(compact_columns(699.0), 2);
        assert_eq!(compact_columns(700.0), 3);
        assert_eq!(compact_columns(959.0), 3);
        assert_eq!(compact_columns(960.0), 4);
    }

    #[test]
    fn un_contenu_installe_sans_dossier_demande_sa_materialisation() {
        let report = WorkshopReport {
            appid: 4000,
            items: vec![WorkshopItem {
                published_file_id: 111,
                installed: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut local = LocalView::default();
        assert!(needs_materialization(&report, &local));
        local.entries.push(LocalEntry {
            id: Some(111),
            ..Default::default()
        });
        assert!(!needs_materialization(&report, &local));
        local.entries[0].is_link = true;
        assert!(needs_materialization(&report, &local));
    }

    #[test]
    fn la_vue_des_fichiers_lit_le_dossier_et_ignore_le_marqueur() {
        let root =
            std::env::temp_dir().join(format!("lapinux-workshop-vue-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Backrooms [111]")).unwrap();
        std::fs::write(root.join("Backrooms [111]/CMAP.sav"), "donnees").unwrap();
        std::fs::write(root.join("Backrooms [111]/.lapinux-workshop"), "111").unwrap();
        let view = LocalView::read(&root);
        assert!(view.error.is_none());
        assert_eq!(view.entries.len(), 1);
        assert_eq!(view.entries[0].id, Some(111));
        assert_eq!(view.entries[0].files.len(), 1);
        assert_eq!(view.entries[0].files[0].rel, "CMAP.sav");
        assert_eq!(view.bytes, 7);
        assert!(!view.entries[0].is_link);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn la_page_fichiers_se_dessine_sans_carte() {
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(900.0, 600.0),
        ));
        let root =
            std::env::temp_dir().join(format!("lapinux-workshop-page-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Backrooms [111]")).unwrap();
        std::fs::write(root.join("Backrooms [111]/CMAP.sav"), "données").unwrap();
        std::fs::write(root.join("Backrooms [111]/.lapinux-workshop"), "111").unwrap();
        let mut shelf = Shelf::default();
        shelf.reports.insert(
            4000,
            WorkshopReport {
                appid: 4000,
                mods_dir: root.clone(),
                items: vec![WorkshopItem {
                    published_file_id: 111,
                    title: "Backrooms".into(),
                    installed: true,
                    ..Default::default()
                }],
                ..Default::default()
            },
        );
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                shelf.files_page(ui, 4000, "Garry's Mod");
            });
        });
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn une_carte_coupee_par_le_defilement_ne_deborde_pas() {
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1000.0, 300.0),
        ));
        let clip = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 150.0));
        let output = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.set_clip_rect(clip);
                let mut previews = PreviewArt::default();
                item_card(
                    ui,
                    &WorkshopItem {
                        published_file_id: 42,
                        title: "Un mod".into(),
                        installed: true,
                        ..Default::default()
                    },
                    246.0,
                    &mut previews,
                    false,
                    false,
                    false,
                );
            });
        });
        let outside = output.shapes.iter().any(|clipped| match &clipped.shape {
            egui::Shape::Text(text) => {
                let label = text.galley.text();
                (label == "Un mod"
                    || label == lapinux_core::i18n::t("settings.appearance.disable")
                    || label == lapinux_core::i18n::t("game_contents.uninstall"))
                    && clipped.clip_rect.bottom() > clip.bottom() + 0.5
            }
            _ => false,
        });
        assert!(!outside, "la carte peint son texte sous le viewport");
    }

    #[test]
    fn une_carte_ne_prend_pas_toute_la_rangee() {
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1000.0, 700.0),
        ));
        let mut used = 0.0;
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.horizontal_top(|ui| {
                    let before = ui.available_width();
                    let mut previews = PreviewArt::default();
                    item_card(
                        ui,
                        &WorkshopItem {
                            published_file_id: 42,
                            title: "Un mod".into(),
                            ..Default::default()
                        },
                        246.0,
                        &mut previews,
                        false,
                        false,
                        false,
                    );
                    used = before - ui.available_width();
                });
            });
        });
        assert!(
            used < 320.0,
            "la carte doit rester dans sa colonne, pas s'étendre sur la rangée ({used})"
        );
    }
}
