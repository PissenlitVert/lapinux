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
    Author, Configuration, ConfigurationSet, GraphicsInjectorStatus, InstallReport,
    InstalledConfiguration,
};
use lapinux_core::ipc::Req;
use lapinux_core::model::{DlssState, Game, GameTech};
use lapinux_core::util::format_bytes;
use std::collections::{HashMap, HashSet};
use std::path::Path;
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
    graphics: HashMap<u32, Vec<GraphicsInjectorStatus>>,
    graphics_seen: HashMap<u32, (String, std::time::Instant)>,
    graphics_drafts: HashMap<u32, GraphicsDraft>,
    graphics_job: Option<(u32, bool, bool, mpsc::Receiver<Result<Vec<String>, String>>)>,
    /// Une correction de réglages a réussi : la fiche doit relire le jeu
    /// plutôt que de réenregistrer sa copie d'affichage devenue périmée.
    graphics_reload: bool,
    graphics_notice: Option<Notice>,
    graphics_armed: Option<(u32, String)>,
    /// Moteur et technologies repérés dans le dossier du jeu, relus une fois
    /// par minute : le parcours des fichiers coûte trop cher à chaque image.
    tech: HashMap<u32, (String, std::time::Instant, Result<GameTech, String>)>,
    tech_pending: Option<(u32, String, mpsc::Receiver<Result<GameTech, String>>)>,
    /// Bibliothèques DLSS du jeu : relues une minute, et aussitôt après une
    /// mise à jour ou un retour en arrière.
    dlss: HashMap<u32, (std::time::Instant, Result<DlssState, String>)>,
    dlss_pending: Option<(u32, mpsc::Receiver<Result<DlssState, String>>)>,
}

struct GraphicsDraft {
    kind: String,
    proxy: String,
}

/// Nom lisible d'un outil ; les noms propres ne se traduisent pas.
/// Empreinte des réglages qui influencent le diagnostic : un changement
/// d'exécutable, de compatibilité ou de variable le fait recalculer.
fn graphics_signature(game: &Game) -> String {
    let mut env: Vec<String> = game
        .settings
        .env
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    env.sort();
    format!(
        "{:?}|{:?}|{:?}|{:?}|{}",
        game.exe,
        game.settings.compatibility.wined3d,
        game.settings.frame_generation,
        game.settings.vkbasalt,
        env.join(";")
    )
}

fn injector_name(kind: &str) -> &str {
    match kind {
        "reshade" => "ReShade",
        "optiscaler" => "OptiScaler",
        "cnc-ddraw" => "cnc-ddraw",
        other => other,
    }
}

/// Architecture PE d'un exécutable Windows, lue sans l'exécuter.
fn pe_machine(path: &Path) -> Option<u16> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let mut header = [0u8; 64];
    file.read_exact(&mut header).ok()?;
    if &header[..2] != b"MZ" {
        return None;
    }
    let offset = u32::from_le_bytes(header[60..64].try_into().ok()?);
    if offset > 16 * 1024 * 1024 {
        return None;
    }
    file.seek(SeekFrom::Start(offset as u64)).ok()?;
    let mut pe = [0u8; 6];
    file.read_exact(&mut pe).ok()?;
    (&pe[..4] == b"PE\0\0").then(|| u16::from_le_bytes([pe[4], pe[5]]))
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
                set.configurations.iter().any(Configuration::available) || !set.installed.is_empty()
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
                        Err(envelope.error.unwrap_or_else(|| {
                            lapinux_core::i18n::t("configurations.unknown_error").into()
                        }))
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
                            lapinux_core::i18n::t_fmt(
                                "configurations.removed_quoted",
                                &[report.title.to_string()],
                            )
                        } else {
                            lapinux_core::i18n::t_fmt(
                                "configurations.installed_quoted",
                                &[report.title.to_string(), report.version.to_string()],
                            )
                        };
                        if report.files > 0 {
                            text.push_str(&lapinux_core::i18n::t_fmt(
                                "configurations.files_suffix",
                                &[report.files.to_string()],
                            ));
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
                        self.notice = Some(Notice {
                            ok: false,
                            text: error,
                        });
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
                    text: lapinux_core::i18n::t("ui.config_no_report").into(),
                });
                false
            }
        }
    }

    /// Recherche asynchrone : un jeu peut contenir des dizaines de milliers
    /// de fichiers, il ne faut pas les parcourir à chaque image de l'interface.
    pub fn game_tech(
        &mut self,
        ctx: &egui::Context,
        game: &Game,
    ) -> Option<Result<GameTech, String>> {
        if game.native || !game.installed {
            return None;
        }
        if let Some((appid, signature, rx)) = &self.tech_pending {
            match rx.try_recv() {
                Ok(result) => {
                    self.tech.insert(
                        *appid,
                        (signature.clone(), std::time::Instant::now(), result),
                    );
                    self.tech_pending = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.tech.insert(
                        *appid,
                        (
                            signature.clone(),
                            std::time::Instant::now(),
                            Err("scan interrupted".into()),
                        ),
                    );
                    self.tech_pending = None;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(Duration::from_millis(250));
                }
            }
        }
        let signature = format!("{}|{}", game.install_dir().display(), game.build_id);
        let cached = self.tech.get(&game.appid).filter(|(seen, time, _)| {
            *seen == signature && time.elapsed() < Duration::from_secs(60)
        });
        if let Some((_, _, result)) = cached {
            return Some(result.clone());
        }
        if self.tech_pending.is_none() {
            if let Some(client) = self.client.clone() {
                let (tx, rx) = mpsc::channel();
                let appid = game.appid;
                let repaint = ctx.clone();
                self.tech_pending = Some((appid, signature, rx));
                std::thread::spawn(move || {
                    let result = client
                        .request_with_timeout(&Req::GameTech { appid }, Duration::from_secs(30))
                        .and_then(|reply| reply.into_data::<GameTech>())
                        .map_err(|error| error.to_string());
                    let _ = tx.send(result);
                    repaint.request_repaint();
                });
            }
        }
        None
    }

    /// Un réglage du jeu vient de changer : la protection lue et l'état des
    /// injecteurs ne sont plus ceux qui avaient été mis en cache.
    pub fn forget_protection(&mut self, appid: u32) {
        self.dlss.remove(&appid);
        self.graphics.remove(&appid);
        self.graphics_seen.remove(&appid);
    }

    /// État des bibliothèques DLSS du jeu, relu une fois par minute.
    pub fn game_dlss(
        &mut self,
        ctx: &egui::Context,
        game: &Game,
    ) -> Option<Result<DlssState, String>> {
        // Un jeu natif n'a pas de Proton, mais rien n'empêche de remplacer
        // des DLL DLSS qu'il embarque lui-même.
        if !game.installed {
            return None;
        }
        self.poll_dlss(ctx);
        if let Some((time, result)) = self.dlss.get(&game.appid) {
            if time.elapsed() < Duration::from_secs(60) {
                return Some(result.clone());
            }
        }
        if self.dlss_pending.is_none() {
            self.ask_dlss(ctx, game.appid, None);
        }
        None
    }

    /// Remplace les DLL DLSS du jeu par les plus récentes trouvées sur la
    /// machine ; la réponse rafraîchit l'état affiché.
    pub fn dlss_update(&mut self, ctx: &egui::Context, appid: u32) {
        self.ask_dlss(ctx, appid, Some(true));
    }

    /// Remet les DLL DLSS d'origine du jeu.
    pub fn dlss_restore(&mut self, ctx: &egui::Context, appid: u32) {
        self.ask_dlss(ctx, appid, Some(false));
    }

    fn ask_dlss(&mut self, ctx: &egui::Context, appid: u32, action: Option<bool>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let (tx, rx) = mpsc::channel();
        self.dlss_pending = Some((appid, rx));
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let request = match action {
                Some(true) => Req::DlssUpdate { appid },
                Some(false) => Req::DlssRestore { appid },
                None => Req::DlssState { appid },
            };
            let result = client
                .request_with_timeout(&request, Duration::from_secs(90))
                .and_then(|reply| reply.into_data::<DlssState>())
                .map_err(|error| error.to_string());
            let _ = tx.send(result);
            repaint.request_repaint();
        });
    }

    fn poll_dlss(&mut self, ctx: &egui::Context) {
        let Some((appid, rx)) = &self.dlss_pending else {
            return;
        };
        let appid = *appid;
        match rx.try_recv() {
            Ok(result) => {
                self.dlss.insert(appid, (std::time::Instant::now(), result));
                self.dlss_pending = None;
            }
            Err(mpsc::TryRecvError::Disconnected) => self.dlss_pending = None,
            Err(mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(250));
            }
        }
    }

    /// Commandes d'injecteurs autonomes dans les réglages graphiques par jeu.
    pub fn graphics_controls(&mut self, ui: &mut egui::Ui, game: &Game) {
        if game.native || !game.installed {
            return;
        }
        let appid = game.appid;
        if let Some((job_appid, fixes, reinstall, rx)) = &self.graphics_job {
            let fixes = *fixes;
            let reinstall = *reinstall;
            match rx.try_recv() {
                Ok(result) => {
                    let id = *job_appid;
                    self.graphics_job = None;
                    let clean = matches!(&result, Ok(warnings) if warnings.is_empty());
                    self.graphics_reload = fixes && clean;
                    self.graphics_notice = Some(match result {
                        Ok(warnings) if warnings.is_empty() => Notice {
                            ok: true,
                            text: lapinux_core::i18n::t("graphics.injector_done").into(),
                        },
                        // Une réinstallation réussie peut signaler l'INI
                        // conservé et l'emplacement des fichiers sauvegardés.
                        Ok(warnings) if reinstall => Notice {
                            ok: true,
                            text: warnings.join(" · "),
                        },
                        Ok(warnings) => Notice {
                            ok: false,
                            text: warnings.join(" · "),
                        },
                        Err(error) => Notice {
                            ok: false,
                            text: error,
                        },
                    });
                    self.graphics.remove(&id);
                    self.graphics_seen.remove(&id);
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ui.ctx().request_repaint_after(Duration::from_millis(250))
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.graphics_job = None;
                    self.graphics_notice = Some(Notice {
                        ok: false,
                        text: lapinux_core::i18n::t("ui.config_no_report").into(),
                    });
                    self.graphics.remove(&appid);
                    self.graphics_seen.remove(&appid);
                }
            }
        }
        // Le diagnostic est recalculé après chaque action et dès qu'un réglage
        // qui influence l'override change ; un rafraîchissement lent couvre
        // aussi les changements globaux.
        let signature = graphics_signature(game);
        let stale = self.graphics_seen.get(&appid).map_or(true, |(seen, at)| {
            seen != &signature || at.elapsed() >= Duration::from_secs(3)
        });
        if stale {
            if let Some(client) = &self.client {
                if let Some(statuses) = crate::request_json::<Vec<GraphicsInjectorStatus>>(
                    client,
                    Req::GraphicsInjectorStatuses { appid },
                ) {
                    self.graphics.insert(appid, statuses);
                }
            }
            self.graphics_seen
                .insert(appid, (signature, std::time::Instant::now()));
        }
        egui::CollapsingHeader::new(lapinux_core::i18n::t("graphics.injector_details"))
            .id_salt(("injector-details", appid))
            .show(ui, |ui| {
                muted(ui, lapinux_core::i18n::t("graphics.injector_warning"));
            });
        if let Some(notice) = &self.graphics_notice {
            let color = if notice.ok { OK } else { WARN };
            ui.label(egui::RichText::new(&notice.text).color(color));
        }
        let loaded = self.graphics.contains_key(&appid);
        let statuses = self.graphics.get(&appid).cloned().unwrap_or_default();
        let busy = self.graphics_job.is_some();
        for status in &statuses {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(injector_name(&status.kind)).strong());
                ui.label(format!("{} · {}", status.version, status.executable));
            });
            let (state, color) = match status.install.as_str() {
                "ok" => ("graphics.injector_state_ok", OK),
                "missing" => ("graphics.injector_state_missing", WARN),
                "modified" => ("graphics.injector_state_modified", WARN),
                "incomplete" => ("graphics.injector_state_incomplete", WARN),
                _ => ("graphics.injector_state_exe_changed", WARN),
            };
            ui.label(egui::RichText::new(lapinux_core::i18n::t(state)).color(color));
            for file in status.files.iter().filter(|file| file.state != "intact") {
                let key = match file.state.as_str() {
                    "missing" => "graphics.injector_file_missing",
                    "modified" => "graphics.injector_file_modified",
                    "unlinked" => "graphics.injector_file_unlinked",
                    _ => "graphics.injector_file_personalized",
                };
                let color = if file.state == "personalized" || file.state == "unlinked" {
                    TEXT()
                } else {
                    WARN
                };
                ui.label(
                    egui::RichText::new(format!("{} · {}", file.name, lapinux_core::i18n::t(key)))
                        .size(11.0)
                        .color(color),
                );
            }
            if status.launch_override == "blocked" {
                let reason = match status.override_reason.as_str() {
                    "wined3d" => "graphics.injector_override_wined3d",
                    "wined3d_env" => "graphics.injector_override_wined3d_env",
                    "frame_generation" => "graphics.injector_override_frame",
                    "frame_generation_env" => "graphics.injector_override_frame_env",
                    "vkbasalt" => "graphics.injector_override_vkbasalt",
                    "vkbasalt_env" => "graphics.injector_override_vkbasalt_env",
                    "config_file" => "graphics.injector_override_config_file",
                    "manual_override" => "graphics.injector_override_manual",
                    "optiscaler_removed" => "graphics.optiscaler_removed",
                    "anticheat" => "graphics.injector_override_anticheat",
                    _ => "graphics.injector_override_optiscaler",
                };
                ui.label(
                    egui::RichText::new(lapinux_core::i18n::t(
                        "graphics.injector_override_blocked",
                    ))
                    .color(WARN),
                );
                muted(ui, lapinux_core::i18n::t(reason));
            } else {
                let key = if status.launch_override == "already" {
                    "graphics.injector_override_already"
                } else {
                    "graphics.injector_override_applied"
                };
                ui.label(egui::RichText::new(lapinux_core::i18n::t(key)).color(OK));
            }
            if !status.backups.is_empty() {
                ui.add_space(4.0);
                crate::field_label(ui, lapinux_core::i18n::t("graphics.injector_backups_title"));
                muted(ui, lapinux_core::i18n::t("graphics.injector_backup_hint"));
                for backup in &status.backups {
                    let label = if backup.kind == "stage" {
                        "graphics.injector_backup_stage"
                    } else {
                        "graphics.injector_backup_user_modified"
                    };
                    ui.horizontal(|ui| {
                        if ui
                            .button(lapinux_core::i18n::t("graphics.injector_backup_open"))
                            .clicked()
                        {
                            self.open_graphics_backup(appid, Path::new(&backup.path));
                        }
                        ui.label(
                            egui::RichText::new(lapinux_core::i18n::t(label))
                                .size(11.0)
                                .color(TEXT()),
                        );
                    });
                    muted(ui, &backup.path);
                }
            }
            if status.kind == "cnc-ddraw" {
                muted(ui, lapinux_core::i18n::t("graphics.cnc_installed_hint"));
            }
            ui.horizontal(|ui| {
                let kind = &status.kind;
                let action = status.action.as_str();
                if status.kind == "optiscaler" {
                    // Une ancienne installation reste visible pour être retirée.
                } else if action == "repair" {
                    if ui
                        .add_enabled(
                            !busy,
                            egui::Button::new(lapinux_core::i18n::t(
                                "graphics.injector_action_repair",
                            )),
                        )
                        .clicked()
                    {
                        self.start_graphics(
                            appid,
                            Req::RepairGraphicsInjector {
                                appid,
                                kind: kind.clone(),
                            },
                        );
                    }
                } else if action == "reinstall" {
                    if ui
                        .add_enabled(
                            !busy,
                            egui::Button::new(lapinux_core::i18n::t(
                                "graphics.injector_action_reinstall",
                            )),
                        )
                        .clicked()
                    {
                        self.start_graphics(
                            appid,
                            Req::ReinstallGraphicsInjector {
                                appid,
                                kind: kind.clone(),
                            },
                        );
                    }
                } else if action == "fix_override" {
                    if ui
                        .add_enabled(
                            !busy,
                            egui::Button::new(lapinux_core::i18n::t(
                                "graphics.injector_action_fix",
                            )),
                        )
                        .clicked()
                    {
                        self.start_graphics(
                            appid,
                            Req::FixGraphicsInjectorOverride {
                                appid,
                                kind: kind.clone(),
                            },
                        );
                    }
                }
                let key = format!("remove:{kind}");
                let armed = self.graphics_armed.as_ref() == Some(&(appid, key.clone()));
                let label = if armed {
                    lapinux_core::i18n::t("graphics.afmf_confirm_remove")
                } else {
                    lapinux_core::i18n::t("graphics.afmf_remove")
                };
                if ui.add_enabled(!busy, egui::Button::new(label)).clicked() {
                    if armed {
                        self.start_graphics(
                            appid,
                            Req::RemoveGraphicsInjector {
                                appid,
                                kind: kind.clone(),
                            },
                        );
                    } else {
                        self.graphics_armed = Some((appid, key));
                    }
                }
            });
        }
        if !statuses.is_empty() {
            if busy {
                muted(ui, lapinux_core::i18n::t("graphics.injector_downloading"));
            }
            return;
        }
        // L'injecteur ne peut viser que l'exécutable de lancement : le daemon
        // refuse tout autre chemin, l'interface n'a donc rien à faire saisir.
        let target = game.exe.as_ref().and_then(|exe| {
            exe.strip_prefix(game.install_dir())
                .ok()
                .map(|relative| (exe.clone(), relative.to_string_lossy().into_owned()))
        });
        crate::field_label(ui, lapinux_core::i18n::t("graphics.injector_target"));
        let machine = target.as_ref().and_then(|(exe, _)| pe_machine(exe));
        match &target {
            Some((_, relative)) => {
                ui.label(relative);
                let hint = match machine {
                    Some(0x14c) => "graphics.injector_32",
                    Some(0x8664) => "graphics.injector_64",
                    _ => "graphics.injector_unknown_arch",
                };
                muted(ui, lapinux_core::i18n::t(hint));
            }
            None => {
                ui.label(
                    egui::RichText::new(lapinux_core::i18n::t("graphics.injector_no_exe"))
                        .color(WARN),
                );
            }
        }
        let cnc_ok = machine == Some(0x14c);
        let draft = self
            .graphics_drafts
            .entry(appid)
            .or_insert_with(|| GraphicsDraft {
                kind: "reshade".into(),
                proxy: "dxgi.dll".into(),
            });
        if (draft.kind == "cnc-ddraw" && !cnc_ok) || draft.kind == "optiscaler" {
            draft.kind = "reshade".into();
            draft.proxy = "dxgi.dll".into();
        }
        crate::field_label(ui, lapinux_core::i18n::t("graphics.injector_choose_tool"));
        ui.horizontal(|ui| {
            ui.selectable_value(&mut draft.kind, "reshade".into(), "ReShade 6.8.0");
            let _ = ui.add_enabled_ui(cnc_ok, |ui| {
                ui.selectable_value(&mut draft.kind, "cnc-ddraw".into(), "cnc-ddraw 7.1")
            });
        });
        let hint = if draft.kind == "reshade" {
            "graphics.injector_reshade_hint"
        } else if draft.kind == "optiscaler" {
            "graphics.injector_optiscaler_hint"
        } else {
            "graphics.cnc_hint"
        };
        muted(ui, lapinux_core::i18n::t(hint));
        match draft.kind.as_str() {
            "reshade" => {
                ui.hyperlink_to(
                    lapinux_core::i18n::t("graphics.reshade_open"),
                    "https://reshade.me/",
                );
            }
            "optiscaler" => {
                ui.hyperlink_to(
                    lapinux_core::i18n::t("graphics.optiscaler_open"),
                    "https://github.com/optiscaler/OptiScaler/wiki/Installation",
                );
            }
            "cnc-ddraw" => {
                ui.hyperlink_to(
                    lapinux_core::i18n::t("graphics.cnc_docs"),
                    "https://github.com/FunkyFr3sh/cnc-ddraw/wiki",
                );
            }
            _ => {}
        }
        crate::field_label(ui, lapinux_core::i18n::t("graphics.injector_proxy"));
        if draft.kind == "cnc-ddraw" {
            // Proxy imposé : l'override Wine est ajouté au lancement, jamais au préfixe.
            // Nom de fichier technique, jamais traduit.
            let proxy = "ddraw.dll";
            ui.label(proxy);
            muted(ui, lapinux_core::i18n::t("graphics.cnc_override_hint"));
            ui.hyperlink_to(
                lapinux_core::i18n::t("graphics.cnc_docs"),
                "https://github.com/FunkyFr3sh/cnc-ddraw/wiki",
            );
        } else {
            let options: &[&str] = if draft.kind == "reshade" {
                &[
                    "dxgi.dll",
                    "d3d9.dll",
                    "d3d11.dll",
                    "d3d12.dll",
                    "opengl32.dll",
                ]
            } else {
                &[
                    "dxgi.dll",
                    "d3d12.dll",
                    "dbghelp.dll",
                    "winmm.dll",
                    "version.dll",
                    "wininet.dll",
                    "winhttp.dll",
                ]
            };
            if !options.contains(&draft.proxy.as_str()) {
                draft.proxy = options[0].into();
            }
            egui::ComboBox::from_id_salt(("injector-proxy", appid))
                .selected_text(&draft.proxy)
                .show_ui(ui, |ui| {
                    for option in options {
                        ui.selectable_value(&mut draft.proxy, (*option).into(), *option);
                    }
                });
        }
        let executable = target
            .as_ref()
            .map(|(_, relative)| relative.clone())
            .unwrap_or_default();
        let request = Req::InstallGraphicsInjector {
            appid,
            kind: draft.kind.clone(),
            version: if draft.kind == "reshade" {
                "6.8.0"
            } else if draft.kind == "cnc-ddraw" {
                "7.1.0.0"
            } else {
                "0.9.4"
            }
            .into(),
            executable: executable.clone(),
            proxy: draft.proxy.clone(),
        };
        let key = format!("install:{}:{executable}:{}", draft.kind, draft.proxy);
        let armed = self.graphics_armed.as_ref() == Some(&(appid, key.clone()));
        let label = if armed {
            lapinux_core::i18n::t("graphics.injector_confirm").to_string()
        } else {
            lapinux_core::i18n::t_args(
                "graphics.injector_install_named",
                &[("tool", injector_name(&draft.kind))],
            )
        };
        let install_button =
            egui::Button::new(egui::RichText::new(label).size(12.5).strong().color(TEXT()))
                .min_size(egui::vec2(220.0, 34.0));
        if ui
            .add_enabled(
                loaded && !busy && target.is_some() && machine.is_some(),
                install_button,
            )
            .clicked()
        {
            if armed {
                self.start_graphics(appid, request);
            } else {
                self.graphics_armed = Some((appid, key));
            }
        }
        if busy {
            muted(ui, lapinux_core::i18n::t("graphics.injector_downloading"));
        }
    }

    /// Ouvre un dossier de sauvegarde conservé. Le chemin est revérifié au
    /// clic : le diagnostic ne propose que des dossiers existants, mais l'un
    /// d'eux peut disparaître entre deux rafraîchissements.
    fn open_graphics_backup(&mut self, appid: u32, path: &Path) {
        if !path.is_dir() {
            self.graphics_notice = Some(Notice {
                ok: false,
                text: lapinux_core::i18n::t_fmt(
                    "graphics.injector_backup_missing",
                    &[path.display().to_string()],
                ),
            });
            self.graphics.remove(&appid);
            self.graphics_seen.remove(&appid);
            return;
        }
        let attempts: [(&str, &[&str]); 3] = [("xdg-open", &[]), ("gio", &["open"]), ("open", &[])];
        let mut last = String::new();
        for (program, arguments) in attempts {
            match std::process::Command::new(program)
                .args(arguments)
                .arg(path)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                Ok(_) => {
                    self.graphics_notice = None;
                    return;
                }
                Err(err) => last = format!("{program}: {err}"),
            }
        }
        self.graphics_notice = Some(Notice {
            ok: false,
            text: lapinux_core::i18n::t_args(
                "graphics.injector_backup_open_error",
                &[("error", &last)],
            ),
        });
    }

    /// Vrai une seule fois après une correction de réglages réussie.
    pub fn take_reload_request(&mut self) -> bool {
        std::mem::take(&mut self.graphics_reload)
    }

    fn start_graphics(&mut self, appid: u32, request: Req) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let fixes = matches!(request, Req::FixGraphicsInjectorOverride { .. });
        let reinstall = matches!(request, Req::ReinstallGraphicsInjector { .. });
        let lists = matches!(
            request,
            Req::RemoveGraphicsInjector { .. }
                | Req::RepairGraphicsInjector { .. }
                | Req::ReinstallGraphicsInjector { .. }
                | Req::FixGraphicsInjectorOverride { .. }
        );
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = client
                .request_with_timeout(&request, Duration::from_secs(600))
                .map_err(|e| e.to_string())
                .and_then(|envelope| {
                    if !envelope.ok {
                        return Err(envelope.error.unwrap_or_default());
                    }
                    if lists {
                        envelope
                            .into_data::<Vec<String>>()
                            .map_err(|e| e.to_string())
                    } else {
                        envelope
                            .into_data::<String>()
                            .map(|_| Vec::new())
                            .map_err(|e| e.to_string())
                    }
                });
            let _ = tx.send(result);
        });
        self.graphics_job = Some((appid, fixes, reinstall, rx));
        self.graphics_armed = None;
        self.graphics_notice = None;
        self.graphics_reload = false;
    }

    /// Page de l'onglet. Rend vrai quand la fiche doit relire le jeu.
    pub fn page(&mut self, ui: &mut egui::Ui, game: &Game) -> bool {
        let mut changed = std::mem::take(&mut self.changed) | self.poll(ui.ctx());
        let appid = game.appid;
        let Some(set) = self.sets.get(&appid).cloned() else {
            ui.add_space(18.0);
            ui.vertical_centered(|ui| {
                crate::section_icons::loading(ui, 18.0);
                ui.add_space(6.0);
                muted(ui, lapinux_core::i18n::t("ui.config_reading"));
                if secondary_button(ui, lapinux_core::i18n::t("configurations.retry")).clicked() {
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
            ui.label(egui::RichText::new(&notice.text).size(11.0).color(color));
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
            .filter(|entry| {
                !set.configurations
                    .iter()
                    .any(|config| config.id == entry.id)
            })
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
            muted(ui, lapinux_core::i18n::t("ui.config_community"));
            if let Some(url) = set.maintainer.profile() {
                ui.hyperlink_to(egui::RichText::new(&set.maintainer.name).size(11.0), url);
            } else if !set.maintainer.name.trim().is_empty() {
                ui.label(
                    egui::RichText::new(&set.maintainer.name)
                        .size(11.0)
                        .color(TEXT_DIM()),
                );
            }
            muted(ui, lapinux_core::i18n::t("configurations.no_data"));
        });

        changed |= std::mem::take(&mut self.changed);
        changed
    }

    fn header(&self, ui: &mut egui::Ui, set: &ConfigurationSet, busy: bool) {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(lapinux_core::i18n::t("ui.config_title"))
                    .size(15.0)
                    .strong()
                    .color(TEXT()),
            );
            if busy {
                ui.add_space(6.0);
                crate::section_icons::loading(ui, 18.0);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (text, color) = if set.remote {
                    (lapinux_core::i18n::t("configurations.catalog"), OK)
                } else {
                    (lapinux_core::i18n::t("ui.config_offline_copy"), WARN)
                };
                ui.label(egui::RichText::new(text).size(10.5).color(color))
                    .on_hover_text(&set.catalog_url);
            });
        });
        ui.add_space(2.0);
        muted(ui, lapinux_core::i18n::t("configurations.subtitle"));
    }

    fn empty(&self, ui: &mut egui::Ui) {
        ui.add_space(18.0);
        ui.vertical_centered(|ui| {
            muted(ui, lapinux_core::i18n::t("configurations.none_known"));
            ui.add_space(3.0);
            muted(ui, lapinux_core::i18n::t("configurations.catalog_hint"));
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
                        status_badge(ui, lapinux_core::i18n::t("configurations.installed_f"), OK);
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
                    muted(ui, lapinux_core::i18n::t("configurations.install_hint"));
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if running {
                        crate::section_icons::loading(ui, 18.0);
                        muted(
                            ui,
                            if self.job.as_ref().map(|job| job.removing).unwrap_or(false) {
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
                                    if primary_button(
                                        ui,
                                        lapinux_core::i18n::t("configurations.install_this"),
                                    )
                                    .clicked()
                                    {
                                        self.start(appid, config.id.clone(), false);
                                    }
                                }
                                Some(entry) => {
                                    if armed {
                                        if secondary_button(
                                            ui,
                                            lapinux_core::i18n::t("configurations.cancel"),
                                        )
                                        .clicked()
                                        {
                                            self.armed = None;
                                        }
                                        if primary_button(
                                            ui,
                                            lapinux_core::i18n::t("configurations.confirm_remove"),
                                        )
                                        .clicked()
                                        {
                                            self.start(appid, config.id.clone(), true);
                                        }
                                    } else {
                                        if secondary_button(ui, lapinux_core::i18n::t("configurations.remove")).clicked() {
                                            self.armed = Some((appid, config.id.clone()));
                                        }
                                        if update {
                                            if primary_button(
                                                ui,
                                                lapinux_core::i18n::t("configurations.update"),
                                            )
                                            .clicked()
                                            {
                                                self.start(appid, config.id.clone(), false);
                                            }
                                        } else {
                                            muted(
                                                ui,
                                                &lapinux_core::i18n::t_fmt(
                                                    "configurations.installed_version",
                                                    &[entry.version.to_string()],
                                                ),
                                            );
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
                    lapinux_core::i18n::t_fmt(
                        "configurations.unlisted",
                        &[
                            lapinux_core::util::format_date(entry.installed_at).to_string(),
                            entry.version.to_string(),
                        ],
                    ),
                );
                ui.add_space(6.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_enabled_ui(!busy, |ui| {
                        if armed {
                            if secondary_button(ui, lapinux_core::i18n::t("configurations.cancel"))
                                .clicked()
                            {
                                self.armed = None;
                            }
                            if primary_button(
                                ui,
                                lapinux_core::i18n::t("configurations.confirm_remove"),
                            )
                            .clicked()
                            {
                                self.start(appid, entry.id.clone(), true);
                            }
                        } else if secondary_button(ui, lapinux_core::i18n::t("configurations.remove")).clicked() {
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
        egui::RichText::new(lapinux_core::i18n::t("configurations.details"))
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
            muted(ui, lapinux_core::i18n::t("configurations.files_replaced"));
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
            for value in [
                &config.tested.proton,
                &config.tested.gpu,
                &config.tested.ram,
            ] {
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
                facts.push(lapinux_core::i18n::t_args(
                    "configurations.tested_on",
                    &[("tested", &tested.to_string())],
                ));
            }
        }
        for fact in facts {
            muted(ui, fact);
        }
        if !config.links.is_empty() {
            ui.add_space(3.0);
            for link in &config.links {
                ui.hyperlink_to(
                    egui::RichText::new(lapinux_core::i18n::t("ui.config_docs")).size(11.0),
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
        "both" => lapinux_core::i18n::t("ui.config_platform_both"),
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
        "verified" => lapinux_core::i18n::t("configurations.verified"),
        "proposed" => lapinux_core::i18n::t("configurations.to_verify"),
        "broken" => lapinux_core::i18n::t("configurations.broken"),
        other => other,
    };
    egui::Frame::none()
        .fill(color.gamma_multiply(0.16))
        .rounding(egui::Rounding::same(5.0))
        .inner_margin(egui::Margin::symmetric(6.0, 2.0))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(label).size(10.0).strong().color(color));
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
    fn tech_cache_respects_installation_platform_and_game_build() {
        let ctx = egui::Context::default();
        let mut shelf = Shelf::default();
        let mut game = Game {
            appid: 7,
            installed: true,
            build_id: "one".into(),
            ..Default::default()
        };
        let signature = format!("{}|{}", game.install_dir().display(), game.build_id);
        shelf.tech.insert(
            game.appid,
            (
                signature,
                std::time::Instant::now(),
                Ok(GameTech::default()),
            ),
        );
        assert!(matches!(shelf.game_tech(&ctx, &game), Some(Ok(_))));
        game.native = true;
        assert!(shelf.game_tech(&ctx, &game).is_none());
        game.native = false;
        game.installed = false;
        assert!(shelf.game_tech(&ctx, &game).is_none());
        game.installed = true;
        game.build_id = "two".into();
        assert!(shelf.game_tech(&ctx, &game).is_none());
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
        "graphics" => (
            lapinux_core::i18n::t("configurations.cat_graphics"),
            egui::Color32::from_rgb(126, 165, 232),
        ),
        "performance" => (lapinux_core::i18n::t("configurations.cat_performance"), OK),
        "display" => (
            lapinux_core::i18n::t("settings.library.display"),
            egui::Color32::from_rgb(178, 148, 232),
        ),
        "language" => (
            lapinux_core::i18n::t("game_sheet.language"),
            egui::Color32::from_rgb(232, 164, 118),
        ),
        "compatibility" => (lapinux_core::i18n::t("configurations.cat_compatibility"), WARN),
        "input" => (
            lapinux_core::i18n::t("controllers.info.inputs"),
            egui::Color32::from_rgb(120, 196, 204),
        ),
        "content" => (
            lapinux_core::i18n::t("configurations.cat_content"),
            egui::Color32::from_rgb(206, 148, 196),
        ),
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
