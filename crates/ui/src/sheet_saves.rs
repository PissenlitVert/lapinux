//! Sauvegardes et colonnes des fiches de jeu.

use super::*;
use lapinux_core::i18n::{t, t_args, t_fmt};

impl LapinuxApp {
    /// Encart des sauvegardes cloud du jeu : ce que le compte garde et les
    /// deux sens de synchronisation. Il vit dans l'onglet Sauvegardes.
    fn cloud_saves_card(&mut self, ui: &mut egui::Ui, game: &Game) {
        let store_game = game.store != lapinux_core::stores::Store::Steam;
        let cloud_title = if store_game {
            t_args(
                "sheet_saves.title_store",
                &[("store", game.store.label())],
            )
        } else {
            t("sheet_saves.title").to_string()
        };
        settings_card(
            ui,
            &cloud_title,
            if store_game {
                t("sheet_saves.subtitle_folder")
            } else {
                t("sheet_saves.subtitle_prefix")
            },
            |ui| {
                self.poll_cloud(ui.ctx(), game.appid);
                // Le réglage agit sur la synchronisation automatique : les
                // deux boutons de la carte restent des gestes explicites.
                let mut settings = game.settings.clone();
                let mut cloud_sync = settings.cloud_sync_enabled();
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(t("sheet_saves.auto_sync_title"))
                            .size(11.5)
                            .color(TEXT()),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if switch(ui, &mut cloud_sync).changed() {
                            settings.cloud_sync = Some(cloud_sync);
                            if let Some(index) = self
                                .games
                                .iter()
                                .position(|entry| entry.appid == game.appid)
                            {
                                self.games[index].settings = settings.clone();
                            }
                            self.save_settings(game.appid, settings.clone());
                        }
                    });
                });
                muted(
                    ui,
                    if cloud_sync
                        && (!store_game
                            || matches!(
                                game.store,
                                lapinux_core::stores::Store::Epic
                                    | lapinux_core::stores::Store::Gog
                            ))
                    {
                        t("sheet_saves.missing_auto")
                    } else if cloud_sync {
                        t("sheet_saves.no_cloud")
                    } else {
                        t("sheet_saves.no_auto")
                    },
                );
                ui.add_space(8.0);
                let restoring = self
                    .cloud_restore
                    .as_ref()
                    .map(|(id, _)| *id == game.appid)
                    .unwrap_or(false);
                let Some(report) = self.cloud.get(&game.appid).cloned() else {
                    muted(ui, t("ui.saves_reading_cloud"));
                    return;
                };
                if let Some(error) = &report.error {
                    ui.label(
                        egui::RichText::new(t_args(
                            "sheet_saves.unknown_read",
                            &[("error", &error.to_string())],
                        ))
                        .color(DANGER),
                    );
                    if ui
                        .button(t("configurations.retry"))
                        .clicked()
                    {
                        self.cloud.remove(&game.appid);
                        self.cloud_asked.remove(&game.appid);
                    }
                    return;
                }
                if !report.saves.has_saves() {
                    match &report.error {
                        Some(error) => muted(
                            ui,
                            t_args(
                                "ui.read_failed",
                                &[("error", &error.to_string())],
                            ),
                        ),
                        None => muted(ui, t("sheet_saves.no_cloud_saves")),
                    }
                    cloud_sync_journal(ui, game.appid, &report);
                    return;
                }
                let saves = &report.saves;
                let protected_cloud = !store_game
                    || matches!(
                        game.store,
                        lapinux_core::stores::Store::Epic | lapinux_core::stores::Store::Gog
                    );
                if protected_cloud {
                    use lapinux_core::model::SteamCloudState;
                    let (state, color) = match saves.steam_state() {
                        SteamCloudState::UpToDate => {
                            (t("sheet_saves.up_to_date"), OK)
                        }
                        SteamCloudState::Conflict => {
                            (t("sheet_saves.conflict"), DANGER)
                        }
                        SteamCloudState::MissingLocal => (
                            t("sheet_saves.cloud_files_missing"),
                            TEXT_DIM(),
                        ),
                        SteamCloudState::Unknown => (
                            t("sheet_saves.unknown_state"),
                            TEXT_DIM(),
                        ),
                    };
                    ui.label(egui::RichText::new(state).strong().color(color));
                    if ui
                        .button(t("sheet_saves.refresh_state"))
                        .clicked()
                    {
                        self.forget_cloud(game.appid);
                    }
                    if saves.steam_state() == SteamCloudState::Conflict {
                        muted(ui, t("sheet_saves.dates_not_enough"));
                        let busy = self.cloud_restore.is_some() || self.cloud_upload.is_some();
                        ui.add_enabled_ui(!busy, |ui| {
                            if let Some((_, choice)) = self
                                .cloud_resolution_armed
                                .filter(|(id, _)| *id == game.appid)
                            {
                                let detail = match choice {
                                    lapinux_core::ipc::SteamCloudChoice::Local => {
                                        t("sheet_saves.local_to_cloud")
                                    }
                                    lapinux_core::ipc::SteamCloudChoice::Cloud => {
                                        t("sheet_saves.cloud_replaces")
                                    }
                                    lapinux_core::ipc::SteamCloudChoice::KeepBoth => {
                                        t("sheet_saves.both_copied")
                                    }
                                };
                                ui.label(detail);
                                ui.horizontal(|ui| {
                                    if ui
                                        .button(t("sheet_saves.confirm_choice"))
                                        .clicked()
                                    {
                                        self.cloud_resolution_armed = None;
                                        self.resolve_cloud(
                                            ui.ctx(),
                                            game.appid,
                                            choice,
                                            saves.clone(),
                                            store_game,
                                        );
                                    }
                                    if ui
                                        .button(t("sheet_saves.cancel"))
                                        .clicked()
                                    {
                                        self.cloud_resolution_armed = None;
                                    }
                                });
                            } else {
                                ui.horizontal_wrapped(|ui| {
                                    for (label, choice) in [
                                        (
                                            t("sheet_saves.keep_pc"),
                                            lapinux_core::ipc::SteamCloudChoice::Local,
                                        ),
                                        (
                                            t("sheet_saves.use_cloud"),
                                            lapinux_core::ipc::SteamCloudChoice::Cloud,
                                        ),
                                        (
                                            t("sheet_saves.keep_both"),
                                            lapinux_core::ipc::SteamCloudChoice::KeepBoth,
                                        ),
                                    ] {
                                        if ui.button(label).clicked() {
                                            self.cloud_resolution_armed =
                                                Some((game.appid, choice));
                                        }
                                    }
                                });
                            }
                        });
                        muted(ui, t("sheet_saves.decide_later"));
                    }
                }
                let latest_cloud = saves.latest_file();
                let latest_local = saves.latest_local_file();
                let cloud_label = if store_game {
                    format!("{} Cloud", game.store.label())
                } else {
                    "Steam Cloud".to_string()
                };
                let remote_count = saves
                    .files
                    .iter()
                    .filter(|file| file.cloud_present != Some(false))
                    .count();
                ui.label(
                    egui::RichText::new(t_fmt(
                        "sheet_saves.remote_files",
                        &[
                            remote_count.to_string(),
                            lapinux_core::util::format_bytes(saves.total_bytes()),
                        ],
                    ))
                    .size(11.5)
                    .color(TEXT()),
                );
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(t("sheet_saves.last_save"))
                        .size(11.0)
                        .strong()
                        .color(TEXT()),
                );
                let cloud_line = match latest_cloud {
                    Some(file) => format!(
                        "{} · {} · {} ({})",
                        cloud_label,
                        if file.modified > 0 {
                            lapinux_core::util::format_datetime(file.modified)
                        } else {
                            t("sheet_saves.unknown_date").to_string()
                        },
                        file.name,
                        lapinux_core::util::format_bytes(file.size)
                    ),
                    None => format!(
                        "{cloud_label} · {}",
                        t("sheet_saves.unknown_date")
                    ),
                };
                ui.label(egui::RichText::new(cloud_line).size(10.5).color(TEXT()));
                let local_line = match latest_local {
                    Some((file, stamp)) => t_fmt(
                        "ui.this_pc",
                        &[
                            lapinux_core::util::format_datetime(stamp),
                            file.name.clone(),
                        ],
                    ),
                    None if protected_cloud => {
                        t("sheet_saves.this_pc_unknown").to_string()
                    }
                    None => t("sheet_saves.this_pc_no_copy").to_string(),
                };
                ui.label(egui::RichText::new(local_line).size(10.5).color(TEXT()));
                let source = if protected_cloud {
                    t("sheet_saves.contents_decide")
                } else {
                    match (latest_cloud, latest_local) {
                        (Some(cloud), Some((_, local))) if local > cloud.modified => {
                            t("sheet_saves.newest_pc")
                        }
                        (Some(cloud), Some((_, local))) if cloud.modified > local => {
                            t("sheet_saves.newest_cloud")
                        }
                        (Some(_), Some(_)) => t("sheet_saves.same_date"),
                        (Some(_), None) => t("sheet_saves.copy_from_cloud"),
                        (None, Some(_)) => t("sheet_saves.cloud_date_missing"),
                        (None, None) => t("sheet_saves.no_date"),
                    }
                };
                ui.label(
                    egui::RichText::new(t_args(
                        "sheet_saves.detected",
                        &[("source", &source.to_string())],
                    ))
                    .size(10.5)
                    .color(TEXT_DIM()),
                );
                if let Some(dir) = &report.local_dir {
                    ui.label(
                        egui::RichText::new(lapinux_core::util::ellipsize(dir, 56))
                            .size(10.5)
                            .color(TEXT_DIM()),
                    )
                    .on_hover_text(dir);
                }
                ui.add_space(8.0);
                egui::CollapsingHeader::new(
                    egui::RichText::new(t_fmt(
                        "sheet_saves.files_details",
                        &[saves.files.len().to_string()],
                    ))
                    .size(11.0)
                    .color(TEXT_DIM()),
                )
                .id_salt(("cloud-files", game.appid))
                .default_open(false)
                .show(ui, |ui| {
                    for file in &saves.files {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(&file.name).size(11.0).color(TEXT()));
                            if file.cloud_present == Some(false) {
                                ui.label(
                                    egui::RichText::new(t(
                                        "sheet_saves.only_this_pc",
                                    ))
                                    .size(10.5)
                                    .color(TEXT_DIM()),
                                );
                                return;
                            }
                            ui.label(
                                egui::RichText::new(lapinux_core::util::format_bytes(file.size))
                                    .size(10.5)
                                    .color(TEXT_DIM()),
                            );
                            match file.local {
                                Some(stamp) => ui.label(
                                    egui::RichText::new(t_fmt(
                                        "sheet_saves.on_disk",
                                        &[lapinux_core::util::format_date(stamp)],
                                    ))
                                    .size(10.5)
                                    .color(TEXT_DIM()),
                                ),
                                None => ui.label(
                                    egui::RichText::new(
                                        if protected_cloud && !file.local_resolved {
                                            t("sheet_saves.local_unknown")
                                        } else {
                                            t("sheet_saves.to_fetch")
                                        },
                                    )
                                    .size(10.5)
                                    .color(DANGER),
                                ),
                            };
                        });
                    }
                });
                if protected_cloud {
                    muted(ui, t("sheet_saves.no_auto_replace"));
                    if store_game {
                        muted(ui, t("sheet_saves.upload_suspended"));
                    }
                }
                ui.add_space(10.0);
                let uploading = self
                    .cloud_upload
                    .as_ref()
                    .map(|(id, _)| *id == game.appid)
                    .unwrap_or(false);
                if restoring || uploading {
                    ui.add_enabled(
                        false,
                        egui::Button::new(if restoring {
                            t("sheet_saves.fetching")
                        } else {
                            t("sheet_saves.uploading")
                        })
                        .min_size(egui::vec2(ui.available_width(), 28.0)),
                    );
                } else if protected_cloud
                    && (saves.steam_state() == lapinux_core::model::SteamCloudState::Conflict
                        || (store_game
                            && saves.steam_state()
                                == lapinux_core::model::SteamCloudState::Unknown))
                {
                    // La résolution explicite est proposée au-dessus ; un état inconnu ne permet aucune mutation.
                } else if store_game && protected_cloud {
                    if saves.steam_state() == lapinux_core::model::SteamCloudState::MissingLocal
                        && ui
                            .button(t("sheet_saves.fetch_missing"))
                            .clicked()
                    {
                        self.restore_cloud(ui.ctx(), game.appid);
                    }
                } else if self.cloud_restore_armed == Some(game.appid) {
                    ui.horizontal(|ui| {
                        if ui
                            .button(t("sheet_saves.replace_cloud"))
                            .clicked()
                        {
                            self.restore_cloud(ui.ctx(), game.appid);
                        }
                        if ui
                            .button(t("sheet_saves.cancel"))
                            .clicked()
                        {
                            self.cloud_restore_armed = None;
                        }
                    });
                    muted(
                        ui,
                        if store_game {
                            t("sheet_saves.folder_replaced")
                        } else {
                            t("sheet_saves.prefix_replaced")
                        },
                    );
                } else if self.cloud_upload_armed == Some(game.appid) {
                    ui.horizontal(|ui| {
                        if ui
                            .button(t("ui.replace_cloud"))
                            .clicked()
                        {
                            self.upload_cloud(ui.ctx(), game.appid);
                        }
                        if ui
                            .button(t("sheet_saves.cancel"))
                            .clicked()
                        {
                            self.cloud_upload_armed = None;
                        }
                    });
                    muted(
                        ui,
                        t("sheet_saves.account_replaced_by_disk"),
                    );
                } else {
                    // Deux sens, deux boutons : ce que le compte garde, et ce
                    // que le disque a de plus récent à lui donner.
                    let half = (ui.available_width() - 8.0) / 2.0;
                    ui.horizontal(|ui| {
                        if ui
                            .add_sized(
                                [half, 28.0],
                                egui::Button::new(t("sheet_saves.fetch")),
                            )
                            .on_hover_text(t("sheet_saves.fetch_hint"))
                            .clicked()
                        {
                            self.cloud_restore_armed = Some(game.appid);
                        }
                        if ui
                            .add_sized([half, 28.0], egui::Button::new("Envoyer"))
                            .on_hover_text(t("sheet_saves.upload_newer"))
                            .clicked()
                        {
                            self.cloud_upload_armed = Some(game.appid);
                        }
                    });
                }
                cloud_sync_journal(ui, game.appid, &report);
            },
        );
    }

    /// Colonne centrée d'un onglet de la fiche : une seule largeur, la même
    /// pour les trois, et son propre défilement.
    pub(super) fn sheet_tab_column(
        ui: &mut egui::Ui,
        salt: (&str, u32),
        add: impl FnOnce(&mut egui::Ui),
    ) {
        Self::sheet_tab_column_with(ui, salt, 780.0, add);
    }

    /// L'Atelier occupe toute la largeur de la fiche : sa liste de sources
    /// reste collée au bord gauche, tandis que les contenus gardent une
    /// colonne centrale lisible.
    pub(super) fn sheet_workshop_column(
        ui: &mut egui::Ui,
        salt: (&str, u32),
        add: impl FnOnce(&mut egui::Ui),
    ) {
        egui::ScrollArea::vertical()
            .id_salt(salt)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                add(ui);
                ui.add_space(12.0);
            });
    }

    /// Ouvre la page demandée par l'onglet Mods dans la fenêtre WebKit de
    /// Lapinux, au lieu de la confier au navigateur du bureau.
    pub(super) fn take_workshop_webview(&mut self, ctx: &egui::Context) {
        // La fenêtre peut avoir été fermée de son côté : on relit son état
        // avant de la réutiliser, sinon une session morte resterait en place.
        self.webview.status(ctx);
        if let Some((url, title)) = self.workshop.take_webview() {
            self.webview.open(&self.paths, &url, &title);
            self.webview.present();
            self.webview.status(ctx);
        }
        if self.webview.is_open() {
            ctx.request_repaint_after(Duration::from_millis(500));
        }
    }

    /// Même colonne, avec sa largeur maximale : l'onglet Sauvegardes aligne
    /// deux embeds côte à côte et prend donc plus de place que la description.
    pub(super) fn sheet_tab_column_with(
        ui: &mut egui::Ui,
        salt: (&str, u32),
        max_width: f32,
        add: impl FnOnce(&mut egui::Ui),
    ) {
        egui::ScrollArea::vertical()
            .id_salt(salt)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let width = ui.available_width();
                let inner = width.min(max_width);
                let left = ((width - inner) / 2.0).max(0.0);
                ui.horizontal_top(|ui| {
                    ui.add_space(left);
                    ui.allocate_ui_with_layout(
                        egui::vec2(inner, 0.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_width(inner);
                            add(ui);
                        },
                    );
                });
                ui.add_space(12.0);
            });
    }

    /// Onglet Sauvegardes : les emplacements en pleine largeur, puis les
    /// copies locales et le cloud en deux colonnes quand la fiche est assez
    /// large.
    pub(super) fn saves_tab(&mut self, ui: &mut egui::Ui, game: &Game) {
        let appid = game.appid;
        Self::sheet_tab_column_with(ui, ("sheet-saves", appid), 1120.0, |ui| {
            let ctx = ui.ctx().clone();
            self.saves.slots_card(
                ui,
                &ctx,
                &self.client,
                appid,
                game.installed,
                game.settings
                    .saves_relaunch(self.config.general.saves_relaunch),
            );
            let width = ui.available_width();
            if width >= 880.0 {
                let gap = 16.0;
                let column = (width - gap) / 2.0;
                ui.horizontal_top(|ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(column, 0.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_width(column);
                            self.saves_tab_settings(ui, game, appid);
                        },
                    );
                    ui.add_space(gap);
                    ui.allocate_ui_with_layout(
                        egui::vec2(column, 0.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_width(column);
                            self.saves_tab_cloud(ui, game);
                        },
                    );
                });
            } else {
                self.saves_tab_settings(ui, game, appid);
                self.saves_tab_cloud(ui, game);
            }
        });
    }

    /// Colonne gauche : le dossier où le jeu écrit et les réglages des copies.
    fn saves_tab_settings(&mut self, ui: &mut egui::Ui, game: &Game, appid: u32) {
        let ctx = ui.ctx().clone();
        let installed = game.installed;
        let mut settings = game.settings.clone();
        let mut save = false;
        self.saves.card(
            ui,
            &ctx,
            &self.client,
            appid,
            installed,
            Some(|ui: &mut egui::Ui| {
                field_label(ui, t("sheet_saves.folder"));
                let mut dir = settings.save_dir.clone().unwrap_or_default();
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut dir)
                            .hint_text(t("sheet_saves.folder_hint"))
                            .desired_width(f32::INFINITY),
                    )
                    .changed()
                {
                    settings.save_dir = Some(dir).filter(|dir| !dir.trim().is_empty());
                    save = true;
                }
                muted(ui, t("sheet_saves.empty_hint"));
            }),
        );
        if save {
            if let Some(index) = self.games.iter().position(|entry| entry.appid == appid) {
                self.games[index].settings = settings.clone();
            }
            self.save_settings(appid, settings);
        }
    }

    /// Colonne droite : le cloud du compte.
    fn saves_tab_cloud(&mut self, ui: &mut egui::Ui, game: &Game) {
        self.cloud_saves_card(ui, game);
    }

    /// Onglet Détails d'un jeu du disque : tout ce que la boutique publie
    /// (genres, dates, langues, poids, succès, cloud), les configurations
    /// minimale et recommandée, puis l'installation sur ce poste.
    pub(super) fn local_sheet_details_tab(
        &mut self,
        ui: &mut egui::Ui,
        game: &Game,
        steam_details: &SteamDetails,
    ) {
        let appid = game.appid;
        self.poll_cloud(ui.ctx(), appid);
        // Un jeu de boutique installé n'a pas de fiche Steam : ses détails
        // viennent de sa boutique, dans la même grille.
        if game.store != lapinux_core::stores::Store::Steam {
            self.store_info.poll(ui.ctx());
            self.store_info.ensure(ui.ctx(), &self.client, appid);
            self.store_sizes.poll();
            self.store_sizes.ensure(ui.ctx(), &self.client, appid);
            let details = self.store_info.details(appid).cloned();
            let chosen = self.stores.platform.get(&appid).cloned();
            let size = self.store_sizes.get(appid);
            let cover = self
                .stores
                .cover_of(appid)
                .map(|(_, cover)| cover)
                .unwrap_or_default();
            let cover_texture = self.covers.get(
                ui.ctx(),
                &self.paths,
                appid,
                &game.name,
                Some(game.store),
                &cover,
                CoverTier::Sheet,
            );
            Self::sheet_tab_column(ui, ("sheet-details", appid), |ui| {
                store_base_art_gallery(
                    ui,
                    &mut self.gameinfo.news_art,
                    &mut self.art,
                    &self.paths,
                    appid,
                    &game.name,
                    cover_texture.clone(),
                    details.as_ref(),
                    &cover,
                );
                store_details_card(
                    ui,
                    game.store,
                    &[],
                    false,
                    0,
                    details.as_ref(),
                    size,
                    chosen.as_deref(),
                );
                self.sheet_disk_card(ui, game);
            });
            return;
        }
        // Les détails viennent de la même lecture que l'encart « Détails
        // Steam » : elle part d'ici pour que les configurations soient là
        // dès la première image de l'onglet.
        self.gameinfo.ensure(ui.ctx(), &self.client, appid);
        Self::sheet_tab_column(ui, ("sheet-details", appid), |ui| {
            steam_details_section(
                ui,
                &self.paths,
                &mut self.art,
                &mut self.covers,
                appid,
                &game.name,
                steam_details,
                Some(game.size_on_disk),
                self.cloud.get(&appid),
            );
            if self.gameinfo.requirements(ui, appid) {
                ui.add_space(12.0);
            }
            self.sheet_disk_card(ui, game);
            ui.add_space(12.0);
            self.gameinfo.block(ui, &self.client, appid);
        });
    }

    /// Encart d'installation : où le jeu est rangé, ce qu'il pèse, son
    /// identifiant, et les gestes d'entretien qui n'ont pas leur place
    /// ailleurs.
    fn sheet_disk_card(&mut self, ui: &mut egui::Ui, game: &Game) {
        settings_card(
            ui,
            "Installation",
            t("sheet_saves.install_hint"),
            |ui| {
                egui::Grid::new(("game-install-grid", game.appid))
                    .num_columns(2)
                    .spacing([10.0, 4.0])
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(t("sheet_saves.folder"))
                                .size(11.0)
                                .color(TEXT_DIM()),
                        );
                        ui.label(
                            egui::RichText::new(lapinux_core::util::ellipsize(
                                &game.install_dir().display().to_string(),
                                64,
                            ))
                            .size(11.0),
                        )
                        .on_hover_text(game.install_dir().display().to_string());
                        ui.end_row();
                        ui.label(
                            egui::RichText::new(t("sheet_saves.size"))
                                .size(11.0)
                                .color(TEXT_DIM()),
                        );
                        ui.label(
                            egui::RichText::new(lapinux_core::util::format_bytes(
                                game.size_on_disk,
                            ))
                            .size(11.0),
                        );
                        ui.end_row();
                        ui.label(egui::RichText::new("appid").size(11.0).color(TEXT_DIM()));
                        ui.label(egui::RichText::new(game.appid.to_string()).size(11.0));
                        ui.end_row();
                    });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button(t("ui.dry_run")).clicked() {
                        self.launch(game.appid, true);
                    }
                    if game.installed
                        && game.origin == GameOrigin::Lapinux
                        && ui
                            .button(t("configurations.update"))
                            .clicked()
                    {
                        self.install_game(game.appid, Vec::new());
                    }
                });
            },
        );
    }

    /// Onglet Sauvegardes : le dossier des copies locales, leur rythme et
    /// leur poids. Les copies d'un jeu se gèrent dans sa fiche.
    pub(super) fn settings_saves_tab(&mut self, ui: &mut egui::Ui) {
        self.poll_saves_usage(ui.ctx());
        let mut config = self.config.clone();
        let mut save = false;
        let usage = self.saves_usage.clone();

        settings_card(
            ui,
            t("settings.saves.local_copies"),
            t("settings.saves.subtitle"),
            |ui| {
                if ui
                    .horizontal(|ui| {
                        let response = switch(ui, &mut config.general.saves_auto);
                        ui.label(t("settings.saves.take_on_close"));
                        response
                    })
                    .inner
                    .changed()
                {
                    save = true;
                }
                muted(
                    ui,
                    t("settings.saves.take_on_close_desc"),
                );
                ui.add_space(10.0);
                if ui
                    .horizontal(|ui| {
                        let response = switch(ui, &mut config.general.saves_skip_cloud);
                        ui.label(t("settings.saves.skip_cloud"));
                        response
                    })
                    .inner
                    .changed()
                {
                    save = true;
                }
                muted(ui, t("settings.saves.skip_cloud_desc"));
                ui.add_space(10.0);
                muted(ui, t("settings.saves.slots_note"));
                ui.add_space(10.0);
                if ui
                    .horizontal(|ui| {
                        let response = switch(ui, &mut config.general.saves_relaunch);
                        ui.label(t("settings.saves.relaunch"));
                        response
                    })
                    .inner
                    .changed()
                {
                    save = true;
                }
                muted(ui, t("settings.saves.relaunch_desc"));
                ui.add_space(10.0);
                ui.separator();
                ui.add_space(8.0);
                let mut after_install = config.downloads.cloud_saves_after_install;
                if ui
                    .horizontal(|ui| {
                        let response = switch(ui, &mut after_install);
                        ui.label(t("settings.saves.cloud_after_install"));
                        response
                    })
                    .inner
                    .changed()
                {
                    config.downloads.cloud_saves_after_install = after_install;
                    save = true;
                }
                muted(
                    ui,
                    t("settings.saves.cloud_after_install_desc"),
                );
            },
        );

        settings_card(
            ui,
            t("settings.saves.folder"),
            t("settings.saves.folder_desc"),
            |ui| {
                field_label(ui, t("settings.saves.root"));
                let mut root = config.general.saves_root.clone();
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut root)
                            .hint_text(self.paths.data_dir.join("saves").display().to_string())
                            .desired_width(f32::INFINITY),
                    )
                    .changed()
                {
                    config.general.saves_root = root;
                    save = true;
                }
                muted(ui, t("settings.saves.folder_note"));
                ui.add_space(10.0);
                ui.separator();
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(t("settings.saves.usage"))
                        .size(12.5)
                        .strong()
                        .color(TEXT()),
                );
                muted(ui, t("settings.saves.usage_desc"));
                match &usage {
                    Some(usage) => {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(lapinux_core::util::format_bytes(usage.bytes))
                                    .size(16.0)
                                    .strong()
                                    .color(TEXT()),
                            );
                            ui.label(
                                egui::RichText::new(t_args(
                                    "settings.saves.usage_counts",
                                    &[
                                        ("copies", &usage.copies.to_string()),
                                        ("games", &usage.games.to_string()),
                                    ],
                                ))
                                .size(11.0)
                                .color(TEXT_DIM()),
                            );
                        });
                        muted(
                            ui,
                            t_args(
                                "settings.saves.effective_root",
                                &[("path", &usage.root)],
                            ),
                        );
                    }
                    None => muted(ui, t("settings.saves.measuring")),
                }
            },
        );

        if save {
            lapinux_core::i18n::set(&config.general.lang);
            self.config = config.clone();
            self.apply_config(config);
            self.saves_usage = None;
        }
    }

    /// Lit le bilan des copies une fois par affichage de la page.
    fn poll_saves_usage(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.saves_usage_rx {
            match rx.try_recv() {
                Ok(value) => {
                    self.saves_usage = value;
                    self.saves_usage_rx = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.saves_usage_rx = None;
                }
            }
        }
        if self.saves_usage.is_some() || self.saves_usage_rx.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.saves_usage_rx = Some(rx);
        let client = self.client.detached();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let usage = client
                .request_with_timeout(&Req::SavesOverview, Duration::from_secs(20))
                .ok()
                .and_then(|envelope| envelope.into_data::<SavesUsage>().ok());
            if tx.send(usage).is_ok() {
                ctx.request_repaint();
            }
        });
    }
}
