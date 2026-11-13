//! Contenus téléchargeables et installation des jeux.

use super::*;
use lapinux_core::i18n::{t, t_args, t_fmt};

impl LapinuxApp {
    /// Poids publié du jeu de base dans la version retenue, sans ses contenus
    /// additionnels.
    fn download_base(&self, appid: u32) -> Option<u64> {
        let catalog = self.catalog.as_ref()?;
        let entry = catalog.entries.iter().find(|entry| entry.appid == appid)?;
        let versions = game_versions(&entry.details);
        versions
            .get(self.version_index(appid, &versions))
            .and_then(|version| version.size)
            .or_else(|| public_download_bytes(&entry.details))
    }

    /// Bannière d'un contenu additionnel, gardée sur disque comme les autres
    /// illustrations : la première ouverture la télécharge, les suivantes la
    /// relisent.
    fn dlc_cover(
        &mut self,
        ctx: &egui::Context,
        appid: u32,
        image: &str,
    ) -> Option<egui::TextureHandle> {
        let name = format!("dlc-{appid}");
        if let Some(entry) = self.dlc_art.get(&appid) {
            return entry.clone();
        }
        let dir = self.paths.art_dir();
        let path = dir.join(format!("{name}.jpg"));
        if let Some(texture) = load_texture_file(ctx, &path, &name) {
            self.dlc_art.insert(appid, Some(texture.clone()));
            return Some(texture);
        }
        let now = Instant::now();
        let due = self
            .dlc_art_at
            .get(&appid)
            .map(|at| now.duration_since(*at) > Duration::from_secs(30))
            .unwrap_or(true);
        if due {
            self.dlc_art_at.insert(appid, now);
            // La boutique n'est interrogée qu'une fois par contenu et par
            // session : sa limite d'appels est basse, et une vignette absente
            // le reste presque toujours (contenu non listé).
            let ask_store = !self.dlc_store.contains(&appid);
            if ask_store {
                self.dlc_store.insert(appid);
            }
            let ctx = ctx.clone();
            let image = image.to_string();
            std::thread::spawn(move || {
                if let Some(bytes) = dlc_banner(appid, &image, ask_store) {
                    std::fs::create_dir_all(&dir).ok();
                    // Écriture à côté puis renommage : l'interface ne doit
                    // jamais décoder un fichier encore à moitié écrit.
                    let temporary = path.with_extension("tmp");
                    if std::fs::write(&temporary, &bytes).is_ok() {
                        let _ = std::fs::rename(&temporary, &path);
                    }
                }
                ctx.request_repaint();
            });
        }
        None
    }

    /// Contenus additionnels d'un jeu, pour la version retenue.
    ///
    /// La fiche complète (PICS) nomme tout ce que Steam publie et donne le
    /// poids de chacun ; le catalogue dit lesquels sont possédés. La version
    /// personnelle garde les contenus publiés qu'elle ne possède pas, pour
    /// que la liste dise « 4 sur 9 ». Une version familiale ne garde que les
    /// contenus du membre : les autres ne sont pas téléchargeables.
    fn contents_of(&self, appid: u32) -> Vec<ContentRow> {
        let published: &[lapinux_core::model::SteamDlcRef] = self
            .gameinfo
            .info()
            .filter(|info| info.appid == appid)
            .map(|info| info.update.dlc.as_slice())
            .unwrap_or(&[]);
        // La version retenue décide des contenus : ceux du compte pour
        // « Moi », ceux du membre choisi pour une version familiale. Une
        // version familiale ne montre que ce que ce membre possède : un DLC
        // publié qu'il n'a pas ne peut pas être téléchargé.
        let owned = self.version_dlc(appid);
        content_rows_for_version(published, &owned, self.family_member_of(appid).is_some())
    }

    /// Demande une fois le poids des contenus additionnels d'un jeu de
    /// boutique : chaque contenu est une application à part, avec son propre
    /// manifeste, donc un appel par contenu.
    fn ensure_store_dlc_sizes(&mut self, ctx: &egui::Context, appid: u32) {
        let known = self
            .store_dlc_sizes
            .get(&appid)
            .is_some_and(|rows| rows.iter().any(|row| row.download > 0));
        if known || self.store_dlc_sizes_rx.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.store_dlc_sizes_rx = Some(rx);
        let worker = self.client.detached();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let rows = worker
                .request_with_timeout(&Req::StoreDlcSizes { appid }, Duration::from_secs(120))
                .ok()
                .and_then(|envelope| {
                    envelope
                        .data
                        .get("contents")
                        .and_then(|value| value.as_array())
                        .cloned()
                })
                .map(|contents| {
                    contents
                        .iter()
                        .map(|row| DlcSize {
                            item: row
                                .get("item")
                                .and_then(|value| value.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            app_name: row
                                .get("app")
                                .and_then(|value| value.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            download: row
                                .get("download")
                                .and_then(|value| value.as_u64())
                                .unwrap_or(0),
                            installed: row
                                .get("installed")
                                .and_then(|value| value.as_u64())
                                .unwrap_or(0),
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if tx.send((appid, rows)).is_ok() {
                ctx.request_repaint();
            }
        });
    }

    /// Lance le téléchargement des contenus retenus, un par un, et dit ce qui
    /// est parti une fois la liste parcourue.
    fn install_store_dlc(
        &mut self,
        ctx: &egui::Context,
        store: lapinux_core::stores::Store,
        app_names: Vec<String>,
    ) {
        if app_names.is_empty() || self.store_dlc_install_rx.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.store_dlc_install_rx = Some(rx);
        let worker = self.client.detached();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let mut started = 0usize;
            let mut error = None;
            for app in app_names {
                match worker.request(&Req::StoreInstall {
                    store,
                    store_id: app,
                    platform: None,
                }) {
                    Ok(envelope) if envelope.ok => started += 1,
                    Ok(envelope) => {
                        error = envelope.error.or_else(|| {
                            Some(t("game_contents.content_refused").into())
                        });
                        break;
                    }
                    Err(err) => {
                        error = Some(err.to_string());
                        break;
                    }
                }
            }
            let message = match (started, error) {
                (0, Some(err)) => format!("{} : {err}", store.label()),
                (count, Some(err)) => t_args(
                    "game_contents.contents_launched_err",
                    &[("count", &count.to_string()), ("err", &err.to_string())],
                ),
                (count, None) => t_args(
                    "game_contents.contents_launched",
                    &[("count", &count.to_string())],
                ),
            };
            let _ = tx.send(message);
            ctx.request_repaint();
        });
    }

    pub(super) fn poll_store_dlc_sizes(&mut self) {
        if let Some(rx) = self.store_dlc_sizes_rx.take() {
            match rx.try_recv() {
                Ok((appid, rows)) => {
                    self.store_dlc_sizes.insert(appid, rows);
                }
                Err(mpsc::TryRecvError::Empty) => self.store_dlc_sizes_rx = Some(rx),
                Err(mpsc::TryRecvError::Disconnected) => {}
            }
        }
        if let Some(rx) = self.store_dlc_install_rx.take() {
            match rx.try_recv() {
                Ok(message) => self.notify(message),
                Err(mpsc::TryRecvError::Empty) => self.store_dlc_install_rx = Some(rx),
                Err(mpsc::TryRecvError::Disconnected) => {}
            }
        }
    }

    pub(super) fn contents_panel(&mut self, ctx: &egui::Context, appid: u32) {
        // La fiche nomme les contenus non possédés : la demander si elle n'est
        // pas déjà là, le panneau se complète à son arrivée.
        self.gameinfo.ensure(ctx, &self.client, appid);
        let mut rows = self.contents_of(appid);
        if rows.is_empty() {
            // Un jeu de boutique : ses contenus sont ceux que la boutique
            // rattache au jeu. Chacun est une application à part, avec son
            // manifeste : le panneau les pèse et laisse choisir ceux qui
            // partent, comme celui de Steam.
            let store_game = self
                .stores
                .owned_games()
                .into_iter()
                .find(|game| game.appid == appid && !game.dlc.is_empty());
            let Some(game) = store_game else {
                self.dlc_modal = None;
                return;
            };
            self.ensure_store_dlc_sizes(ctx, appid);
            let sizes: HashMap<String, (String, u64, u64)> = self
                .store_dlc_sizes
                .get(&appid)
                .map(|rows| {
                    rows.iter()
                        .map(|row| {
                            (
                                row.item.clone(),
                                (row.app_name.clone(), row.download, row.installed),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            let loading = !self.store_dlc_sizes.contains_key(&appid);
            // Tout est retenu au départ, comme les contenus possédés de Steam ;
            // un choix déjà fait l'emporte, décocher tous les contenus compris.
            let mut pick: HashSet<String> =
                self.store_dlc_pick.get(&appid).cloned().unwrap_or_else(|| {
                    game.dlc
                        .iter()
                        .map(|content| content.store_id.clone())
                        .collect()
                });
            let subtitle = t_fmt(
                "game_contents.contents_attached",
                &[
                    game.name.to_string(),
                    game.dlc.len().to_string(),
                    game.store.label().to_string(),
                ],
            );
            let mut download = false;
            let close = fixed_panel(
                ctx,
                "store-contents-panel",
                "Contenus additionnels",
                &subtitle,
                egui::vec2(620.0, 520.0),
                |ui| {
                    muted(ui, t("game_contents.tick_weight_app"));
                    ui.add_space(10.0);
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, true])
                        .max_height((ui.available_height() - 96.0).max(160.0))
                        .show(ui, |ui| {
                            for content in &game.dlc {
                                let known = sizes
                                    .get(&content.store_id)
                                    .map(|size| (size.1, size.2))
                                    .unwrap_or((0, 0));
                                let mut checked = pick.contains(&content.store_id);
                                ui.horizontal(|ui| {
                                    // Toujours cochable : le choix et le poids
                                    // ne dépendent pas de la publication du
                                    // nom d'application, qui ne sert qu'au
                                    // téléchargement.
                                    let toggle = ui.add(egui::Checkbox::without_text(&mut checked));
                                    if toggle.changed() {
                                        if checked {
                                            pick.insert(content.store_id.clone());
                                        } else {
                                            pick.remove(&content.store_id);
                                        }
                                    }
                                    ui.add_space(6.0);
                                    let width = (ui.available_width() - 130.0).max(140.0);
                                    ui.allocate_ui_with_layout(
                                        egui::vec2(width, 22.0),
                                        egui::Layout::top_down(egui::Align::Min),
                                        |ui| {
                                            ui.add(
                                                egui::Label::new(
                                                    egui::RichText::new(&content.name)
                                                        .size(12.5)
                                                        .color(TEXT()),
                                                )
                                                .truncate(),
                                            );
                                        },
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            let text = if known.0 > 0 {
                                                lapinux_core::util::format_bytes(known.0)
                                            } else if loading {
                                                "…".to_string()
                                            } else if content.app_name.is_empty() {
                                                t(
                                                    "game_contents.weight_unknown",
                                                )
                                                .to_string()
                                            } else {
                                                t("game_contents.included")
                                                    .to_string()
                                            };
                                            ui.label(
                                                egui::RichText::new(text)
                                                    .size(11.0)
                                                    .color(TEXT_DIM()),
                                            );
                                        },
                                    );
                                });
                                ui.add_space(6.0);
                            }
                        });
                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(6.0);
                    let total: u64 = game
                        .dlc
                        .iter()
                        .filter(|content| pick.contains(&content.store_id))
                        .map(|content| sizes.get(&content.store_id).map(|size| size.1).unwrap_or(0))
                        .sum();
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(t(
                                "game_contents.total_download",
                            ))
                            .size(12.0)
                            .strong(),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let text = if total > 0 {
                                lapinux_core::util::format_bytes(total)
                            } else {
                                "—".to_string()
                            };
                            ui.label(egui::RichText::new(text).size(12.5).strong());
                        });
                    });
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let ready = !loading && !pick.is_empty();
                        if ui
                            .add_enabled(
                                ready,
                                egui::Button::new(
                                    egui::RichText::new(t(
                                        "game_contents.download_selection",
                                    ))
                                    .strong(),
                                ),
                            )
                            .on_hover_text(t("game_contents.each_queue"))
                            .clicked()
                        {
                            download = true;
                        }
                        if pick.is_empty() {
                            muted(ui, t("game_contents.none"));
                        } else if game.dlc.iter().any(|content| {
                            pick.contains(&content.store_id)
                                && sizes
                                    .get(&content.store_id)
                                    .map(|size| size.0.is_empty())
                                    .unwrap_or(content.app_name.is_empty())
                        }) {
                            muted(
                                ui,
                                t("game_contents.content_no_appname"),
                            );
                        }
                    });
                },
            );
            self.store_dlc_pick.insert(appid, pick.clone());
            if download && !pick.is_empty() {
                let app_names: Vec<String> = game
                    .dlc
                    .iter()
                    .filter(|content| pick.contains(&content.store_id))
                    .map(|content| {
                        sizes
                            .get(&content.store_id)
                            .map(|size| size.0.clone())
                            .unwrap_or_else(|| content.app_name.clone())
                    })
                    .filter(|app| !app.is_empty())
                    .collect();
                self.install_store_dlc(ctx, game.store, app_names);
            }
            if close {
                self.dlc_modal = None;
            }
            return;
        }
        contents_owned_first(&mut rows);
        let name = self
            .games
            .iter()
            .find(|game| game.appid == appid)
            .map(|game| game.name.clone())
            .or_else(|| {
                self.catalog.as_ref().and_then(|catalog| {
                    catalog
                        .entries
                        .iter()
                        .find(|entry| entry.appid == appid)
                        .map(|entry| entry.name.clone())
                })
            })
            .unwrap_or_else(|| format!("App {appid}"));
        // Un jeu déjà installé n'attend que ses contenus : le jeu de base est
        // sur le disque, et l'installation le vérifie sans le recharger.
        let installed = self
            .games
            .iter()
            .any(|game| game.appid == appid && game.installed);
        let base = if installed {
            None
        } else {
            self.download_base(appid)
        };
        let total_label = if installed {
            t("game_contents.extra_to_download")
        } else {
            t("game_contents.total_download")
        };
        let empty_label = if installed {
            t("game_contents.none_yet")
        } else {
            t("game_contents.base_only")
        };
        let owned_count = rows.iter().filter(|row| row.owned).count();
        // Amorce du premier clic : tout ce que le compte possède, c'est-à-dire
        // l'état que le panneau affiche tant que rien n'a été décoché.
        let preselected: HashSet<u32> = rows
            .iter()
            .filter(|row| row.owned)
            .map(|row| row.appid)
            .collect();
        let subtitle = t_fmt(
            "game_contents.owned_of",
            &[
                name.to_string(),
                owned_count.to_string(),
                rows.len().to_string(),
            ],
        );
        let close = fixed_panel(
            ctx,
            "contents-panel",
            "Contenus additionnels",
            &subtitle,
            egui::vec2(620.0, 560.0),
            |ui| {
                muted(ui, t("game_contents.tick_weight_owned"));
                ui.add_space(10.0);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, true])
                    // Le pied (total et rappel) garde sa place : la liste
                    // défile au-dessus, jamais dessous.
                    .max_height((ui.available_height() - 74.0).max(160.0))
                    .show(ui, |ui| {
                        // Ce que le compte possède d'abord, le reste ensuite :
                        // chaque catégorie a son titre et son compte.
                        for (index, (title, section)) in
                            content_sections(&rows).into_iter().enumerate()
                        {
                            if index > 0 {
                                ui.add_space(12.0);
                            }
                            ui.label(
                                egui::RichText::new(title)
                                    .size(10.5)
                                    .strong()
                                    .color(TEXT_DIM()),
                            );
                            ui.add_space(3.0);
                            ui.separator();
                            ui.add_space(4.0);
                            for row in section {
                                let selected = content_checked(
                                    self.dlc_selection.get(&appid),
                                    row.owned,
                                    row.appid,
                                );
                                ui.horizontal(|ui| {
                                    let mut checked = selected;
                                    let toggle = ui.add_enabled(
                                        row.owned,
                                        egui::Checkbox::without_text(&mut checked),
                                    );
                                    if toggle.changed() {
                                        // Le premier clic part de ce que le panneau
                                        // montrait : les contenus possédés cochés.
                                        // Sans cette amorce, décocher une case les
                                        // décochait toutes.
                                        let selection = self
                                            .dlc_selection
                                            .entry(appid)
                                            .or_insert_with(|| preselected.clone());
                                        if checked {
                                            selection.insert(row.appid);
                                        } else {
                                            selection.remove(&row.appid);
                                        }
                                    }
                                    // Vignette : la bannière du contenu, ou sa place
                                    // tant qu'elle n'est pas arrivée.
                                    let (thumb, _) = ui.allocate_exact_size(
                                        egui::vec2(78.0, 36.0),
                                        egui::Sense::hover(),
                                    );
                                    match self.dlc_cover(ctx, row.appid, &row.image) {
                                        Some(texture) => {
                                            let uv = egui::Rect::from_min_max(
                                                egui::pos2(0.0, 0.0),
                                                egui::pos2(1.0, 1.0),
                                            );
                                            ui.painter().image(
                                                texture.id(),
                                                thumb,
                                                uv,
                                                egui::Color32::WHITE,
                                            );
                                        }
                                        None => {
                                            ui.painter().rect_filled(thumb, 5.0, CARD_ACTIVE());
                                        }
                                    }
                                    ui.add_space(6.0);
                                    let text_width = (ui.available_width() - 150.0).max(140.0);
                                    ui.allocate_ui_with_layout(
                                        egui::vec2(text_width, 36.0),
                                        egui::Layout::top_down(egui::Align::Min),
                                        |ui| {
                                            ui.add_space(7.0);
                                            ui.add(
                                                egui::Label::new(
                                                    egui::RichText::new(if row.name.is_empty() {
                                                        format!("Contenu {}", row.appid)
                                                    } else {
                                                        row.name.clone()
                                                    })
                                                    .size(12.5)
                                                    .color(if row.owned {
                                                        TEXT()
                                                    } else {
                                                        TEXT_DIM()
                                                    }),
                                                )
                                                .truncate(),
                                            );
                                        },
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            // Ce qui est déjà sur le disque se dit, ce
                                            // qui manque annonce son poids.
                                            let color = if row.installed { OK } else { TEXT_DIM() };
                                            ui.label(
                                                egui::RichText::new(content_weight_text(row))
                                                    .size(11.0)
                                                    .color(color),
                                            );
                                        },
                                    );
                                });
                                ui.add_space(4.0);
                            }
                        }
                    });
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(total_label).size(12.0).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let selection = self.dlc_selection.get(&appid);
                        let mut total = base.unwrap_or(0);
                        let mut any = base.is_some();
                        for row in &rows {
                            if !row.owned || row.installed {
                                continue;
                            }
                            if !content_checked(selection, row.owned, row.appid) {
                                continue;
                            }
                            if row.download > 0 {
                                total += row.download;
                                any = true;
                            }
                        }
                        let checked = rows
                            .iter()
                            .filter(|row| content_checked(selection, row.owned, row.appid))
                            .count();
                        // Rien à télécharger alors que des contenus sont
                        // cochés : ils sont déjà sur le disque.
                        let text = if any && total > 0 {
                            lapinux_core::util::format_bytes(total)
                        } else if checked == 0 {
                            "—".to_string()
                        } else {
                            t("game_contents.already_on_disk").to_string()
                        };
                        ui.label(egui::RichText::new(text).size(12.5).strong());
                    });
                });
                if !rows.iter().any(|row| {
                    content_checked(self.dlc_selection.get(&appid), row.owned, row.appid)
                }) {
                    ui.add_space(4.0);
                    muted(ui, empty_label);
                }
            },
        );
        if close {
            self.dlc_modal = None;
        }
    }

    /// Fenêtre des réglages d'un jeu, ouverte par l'engrenage de l'encart.
    /// Elle reprend le châssis des Réglages avec un rail à gauche : chaque
    /// onglet ne montre qu'une famille de contrôles, au lieu d'un long
    /// formulaire unique.
    pub(super) fn game_settings_window(&mut self, ctx: &egui::Context, appid: u32) {
        let Some(index) = self.games.iter().position(|game| game.appid == appid) else {
            self.game_settings = None;
            return;
        };
        let game = self.games[index].clone();
        let close = fixed_panel_with_scroll(
            ctx,
            "game-settings-panel",
            t("game_contents.settings"),
            "",
            egui::vec2(740.0, 500.0),
            false,
            false,
            64,
            |ui| {
                self.game_settings_panel(ui, index, &game);
            },
        );
        if close {
            self.game_settings = None;
        }
    }

    fn game_settings_panel(&mut self, ui: &mut egui::Ui, index: usize, game: &Game) {
        let body = ui.max_rect();
        let rail_width = 172.0;
        // La colonne de formulaire doit garder une largeur finie. Sans elle,
        // les lignes alignées à droite se placent au bord de l'écran (hors du
        // panneau) et leurs contrôles disparaissent.
        let content_width = (body.width() - rail_width - 28.0).clamp(280.0, 560.0);
        let rail = egui::Rect::from_min_max(
            body.min,
            egui::pos2(body.left() + rail_width, body.bottom()),
        );
        ui.painter().line_segment(
            [
                egui::pos2(rail.right(), rail.top()),
                egui::pos2(rail.right(), rail.bottom()),
            ],
            egui::Stroke::new(1.0_f32, BORDER()),
        );

        let height = body.height();
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(rail_width, height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(rail_width);
                    let icon = self
                        .art
                        .cached_icon(ui.ctx(), &self.paths, game.appid)
                        .or_else(|| square_icon_from_cover(ui.ctx(), &self.paths, game.appid));
                    // Le corps du panneau a déjà une marge à gauche ; on
                    // l'ajoute aussi en haut pour que la pochette soit à la
                    // même distance des deux bords du modal.
                    let rail_side_padding = 12.0;
                    let rail_top_padding = 24.0;
                    ui.add_space(rail_top_padding);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        ui.add_space(rail_side_padding);
                        let size = 40.0;
                        let (icon_rect, _) =
                            ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
                        let rounding = egui::Rounding::same(9.0);
                        match icon {
                            Some(texture) => paint_cover(ui, icon_rect, &texture, rounding),
                            None => {
                                let initial = game
                                    .name
                                    .trim()
                                    .chars()
                                    .find(|character| character.is_alphanumeric())
                                    .map(|character| character.to_uppercase().to_string())
                                    .unwrap_or_else(|| "?".to_string());
                                ui.painter().rect_filled(icon_rect, rounding, CARD());
                                ui.painter().text(
                                    icon_rect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    initial,
                                    egui::FontId::proportional(16.0),
                                    TEXT_DIM(),
                                );
                            }
                        }
                        ui.add_space(9.0);
                        let name_width = (ui.available_width() - 8.0).max(0.0);
                        ui.allocate_ui_with_layout(
                            egui::vec2(name_width, size),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                ui.add_space(10.0);
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(&game.name)
                                            .size(12.0)
                                            .strong()
                                            .color(TEXT()),
                                    )
                                    .truncate(),
                                )
                                .on_hover_text(&game.name);
                            },
                        );
                    });
                    ui.add_space(9.0);
                    let (separator, _) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), 1.0),
                        egui::Sense::hover(),
                    );
                    ui.painter().hline(
                        separator.x_range(),
                        separator.center().y,
                        egui::Stroke::new(1.0_f32, with_alpha(BORDER(), 0.9)),
                    );
                    ui.add_space(8.0);
                    ui.spacing_mut().item_spacing = egui::vec2(0.0, 2.0);
                    for tab in GAME_SETTINGS_TABS {
                        let selected = self.game_settings_tab == tab;
                        let (row, response) = ui.allocate_exact_size(
                            egui::vec2(ui.available_width(), 34.0),
                            egui::Sense::click(),
                        );
                        let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
                        if selected {
                            // Pas de pastille encadrée : une barre d'accent et
                            // un texte plus clair suffisent à marquer l'onglet.
                            ui.painter().rect_filled(
                                egui::Rect::from_min_size(
                                    egui::pos2(row.left(), row.center().y - 9.0),
                                    egui::vec2(3.0, 18.0),
                                ),
                                egui::Rounding::same(2.0),
                                ACCENT(),
                            );
                        } else if response.hovered() {
                            ui.painter().rect_filled(
                                row,
                                egui::Rounding::same(9.0),
                                with_alpha(CARD_HOVER(), 0.55),
                            );
                        }
                        let ink = if selected || response.hovered() {
                            TEXT()
                        } else {
                            TEXT_DIM()
                        };
                        let progress = icon_hover_progress(
                            ui,
                            ui.id().with(("game-settings-tab-icon", tab as u8)),
                            response.hovered() || response.has_focus(),
                        );
                        section_icons::settings_tab_animated(
                            ui.painter(),
                            tab.icon(),
                            egui::pos2(row.left() + 19.0, row.center().y),
                            18.0,
                            ink,
                            progress,
                        );
                        ui.painter().text(
                            egui::pos2(row.left() + 35.0, row.center().y),
                            egui::Align2::LEFT_CENTER,
                            tab.title(),
                            egui::FontId::proportional(12.0),
                            ink,
                        );
                        if response.clicked() && !selected {
                            self.game_settings_tab = tab;
                        }
                        ui.add_space(2.0);
                    }
                    ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                        ui.add_space(8.0);
                        muted(ui, t("game_settings.autosave"));
                    });
                },
            );
            ui.add_space(20.0);
            ui.allocate_ui_with_layout(
                egui::vec2(content_width, height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new(self.game_settings_tab.title())
                            .size(18.0)
                            .strong()
                            .color(TEXT()),
                    );
                    ui.add_space(2.0);
                    muted(ui, self.game_settings_tab.subtitle());
                    ui.add_space(8.0);
                    egui::ScrollArea::vertical()
                        .id_salt((
                            "game-settings-content",
                            game.appid,
                            self.game_settings_tab as u8,
                        ))
                        .auto_shrink([false, false])
                        .scroll_bar_visibility(
                            egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded,
                        )
                        .show(ui, |ui| {
                            // `set_width` ne borne pas la zone de mise en page
                            // d'une ScrollArea : on crée explicitement un enfant
                            // de la largeur du panneau pour que les contrôles
                            // alignés à droite restent visibles.
                            let form_width = (content_width - 8.0).max(250.0);
                            ui.allocate_ui_with_layout(
                                egui::vec2(form_width, ui.available_height().max(1.0)),
                                egui::Layout::top_down(egui::Align::Min),
                                |ui| {
                                    ui.set_width(form_width);
                                    self.game_settings_form(ui, index);
                                },
                            );
                        });
                },
            );
        });
    }

    /// Vrai quand la poubelle de ce jeu a été cliquée une fois et attend
    /// encore le clic de confirmation.
    pub(super) fn uninstall_armed(&self, appid: u32) -> bool {
        self.uninstall_confirm
            .is_some_and(|(armed, at)| armed == appid && at.elapsed() < UNINSTALL_CONFIRM)
    }

    /// Contenus que la version retenue possède : ceux du compte pour « Moi »,
    /// ceux du membre choisi pour une version familiale.
    fn dlc_of(&self, appid: u32) -> Vec<SteamDlc> {
        self.version_dlc(appid)
    }

    /// Contenus d'une version : le catalogue ne garde les DLC familiaux
    /// qu'avec leur propriétaire, et la fiche du jeu dit lesquels lui
    /// appartiennent. Le choix vient du menu de la fiche.
    pub(super) fn version_dlc(&self, appid: u32) -> Vec<SteamDlc> {
        let Some(entry) = self
            .catalog
            .as_ref()
            .and_then(|catalog| catalog.entries.iter().find(|entry| entry.appid == appid))
        else {
            return Vec::new();
        };
        let Some(member) = self.family_member_of(appid) else {
            return entry.dlc.clone();
        };
        let Some(catalog) = self.catalog.as_ref() else {
            return Vec::new();
        };
        let mut dlc = family_version_dlc(&entry.details, &catalog.family_dlcs, member);
        // Le poids publié par la fiche complète vaut aussi pour le contenu
        // d'un membre : le catalogue familial ne le porte pas.
        if let Some(info) = self.gameinfo.info().filter(|info| info.appid == appid) {
            for content in &mut dlc {
                let Some(published) = info
                    .update
                    .dlc
                    .iter()
                    .find(|row| row.appid == content.appid)
                else {
                    continue;
                };
                content.download = published.download;
                content.install = published.install;
                if content.name.trim().is_empty() {
                    content.name = published.name.clone();
                }
            }
        }
        dlc
    }

    /// Membre dont la version est retenue pour ce jeu, `None` pour la licence
    /// personnelle. Un choix devenu invalide — le membre ne partage plus le
    /// jeu — retombe sur la version que le bouton affiche par défaut.
    fn family_member_of(&self, appid: u32) -> Option<u64> {
        let entry = self
            .catalog
            .as_ref()
            .and_then(|catalog| catalog.entries.iter().find(|entry| entry.appid == appid));
        let family = self
            .social
            .as_ref()
            .and_then(|social| social.family.as_ref());
        let shares = |member: &lapinux_core::social::SteamFamilyMember| {
            !member.is_self && member.appids.contains(&appid)
        };
        let chosen = self.family_choice.get(&appid).copied().filter(|chosen| {
            family.is_some_and(|family| {
                family
                    .members
                    .iter()
                    .any(|member| member.steam_id == *chosen && shares(member))
            })
        });
        chosen.or_else(|| {
            if entry.is_some_and(|entry| entry.family) {
                family
                    .and_then(|family| family.members.iter().find(|member| shares(member)))
                    .map(|member| member.steam_id)
            } else {
                None
            }
        })
    }

    /// Contenus retenus pour un jeu, dans l'ordre du catalogue.
    pub(super) fn selected_dlc(&self, appid: u32) -> Vec<u32> {
        let contents = self.dlc_of(appid);
        let selected = selected_contents(&self.dlc_selection, appid, &contents);
        contents
            .into_iter()
            .filter(|dlc| selected.contains(&dlc.appid))
            .map(|dlc| dlc.appid)
            .collect()
    }

    /// Poids sur le disque d'un jeu installé, quand la bibliothèque le connaît.
    pub(super) fn installed_size(&self, appid: u32) -> Option<u64> {
        self.games
            .iter()
            .find(|game| game.appid == appid && game.installed)
            .map(|game| game.size_on_disk)
            .filter(|bytes| *bytes > 0)
    }

    /// Minutes jouées d'un jeu, dans l'ordre : la lecture des succès (elle
    /// porte le total du compte), la fiche Steam, puis le manifeste local.
    /// `None` quand aucune source ne l'a dit — l'encart n'invente rien.
    pub(super) fn play_minutes(&self, appid: u32, local_secs: Option<u64>) -> Option<u64> {
        if let Some(minutes) = self.achievements.play_minutes() {
            return Some(minutes);
        }
        if let Some(minutes) = self
            .gameinfo
            .info()
            .filter(|info| info.appid == appid)
            .and_then(|info| info.account.minutes_total)
        {
            return Some(minutes);
        }
        // Le manifeste ne parle que d'un jeu installé : sans heures dedans, il
        // dit « jamais joué ».
        local_secs.map(|secs| secs / 60)
    }

    /// Ce que l'encart propose : le transfert en cours prime, puis
    /// l'installation, puis le téléchargement avec son poids.
    pub(super) fn card_buttons(
        &self,
        appid: u32,
        installed: bool,
        steam: Option<bool>,
        owned: bool,
        family_available: bool,
        details: &SteamDetails,
        dlc: &[SteamDlc],
        update: bool,
    ) -> CardButtons {
        if let Some((fraction, job)) = self.running_job(appid) {
            return CardButtons::Progress {
                fraction,
                job,
                appid,
            };
        }
        if installed {
            return CardButtons::Installed { update };
        }
        if steam.is_none() {
            return CardButtons::Unavailable;
        }
        let versions = game_versions(details);
        let chosen = self.version_index(appid, &versions);
        // « Moi » ouvre la liste quand le jeu est aussi possédé ; les autres
        // entrées restent les membres qui le partagent.
        let (mut family, family_chosen) = if family_available {
            let family = self.family_versions(appid, owned);
            let chosen = self
                .family_choice
                .get(&appid)
                .and_then(|steam_id| {
                    family
                        .iter()
                        .position(|member| member.steam_id == *steam_id)
                })
                .unwrap_or(0);
            (family, chosen)
        } else {
            (Vec::new(), 0)
        };
        self.fill_family_dlc_names(&mut family, details, dlc);
        // Les contenus comptés et pesés suivent la version retenue : le
        // membre choisi peut n'avoir aucun des DLC du compte.
        let version_dlc = self.version_dlc(appid);
        // Le poids suit la version affichée : choisir Windows ne doit pas
        // laisser le poids du build Linux sur le bouton.
        let base = versions
            .get(chosen)
            .and_then(|version| version.size)
            .or_else(|| public_download_bytes(details));
        CardButtons::Download {
            appid,
            size: download_size(
                base,
                &version_dlc,
                Some(&selected_contents(&self.dlc_selection, appid, &version_dlc)),
            ),
            dlc: version_dlc.len(),
            versions,
            mobile: Vec::new(),
            chosen,
            family,
            family_chosen,
            dlc_names: version_dlc
                .iter()
                .map(|dlc| dlc.name.clone())
                .filter(|name| !name.trim().is_empty())
                .collect(),
            steam: true,
        }
    }
    /// Contenus de chaque version : ceux du compte pour « Moi », ceux que le
    /// membre possède réellement pour les autres. Le catalogue ne garde que
    /// les DLC partagés avec leur propriétaire ; les jeux publiés par Steam
    /// donnent la liste des contenus du jeu.
    pub(crate) fn fill_family_dlc_names(
        &self,
        family: &mut [FamilyVersion],
        details: &SteamDetails,
        own: &[SteamDlc],
    ) {
        let personal: Vec<String> = own
            .iter()
            .map(|dlc| dlc.name.clone())
            .filter(|name| !name.trim().is_empty())
            .collect();
        let mut published: std::collections::HashSet<u32> =
            details.dlc_ids.iter().copied().collect();
        published.extend(own.iter().map(|dlc| dlc.appid));
        let shared = self
            .catalog
            .as_ref()
            .map(|catalog| catalog.family_dlcs.as_slice())
            .unwrap_or(&[]);
        for member in family {
            member.dlc_names = if member.personal {
                personal.clone()
            } else {
                shared
                    .iter()
                    .filter(|dlc| dlc.owners.contains(&member.steam_id))
                    .filter(|dlc| published.contains(&dlc.appid))
                    .map(|dlc| dlc.name.clone())
                    .filter(|name| !name.trim().is_empty())
                    .collect()
            };
        }
    }

    /// Versions proposées pour un jeu : « Moi » d'abord si le compte le
    /// possède, puis les membres qui le partagent. Le compte connecté n'est
    /// jamais répété dans la liste des membres.
    pub(crate) fn family_versions(&self, appid: u32, include_self: bool) -> Vec<FamilyVersion> {
        let family = self
            .social
            .as_ref()
            .and_then(|social| social.family.as_ref());
        let mut versions = family_versions_for(appid, family, include_self);
        // La session ne nomme pas les membres qui ne sont pas amis : leur
        // fiche publique, lue par le profil, complète le libellé. « Moi » garde
        // son libellé, qui ne dépend d'aucune fiche publique.
        for member in &mut versions {
            if member.personal {
                continue;
            }
            if let Some(name) = self.profile.family_name(member.steam_id) {
                member.name = name;
            }
        }
        versions
    }

    /// Vrai si le compte connecté possède ce jeu dans le groupe familial.
    pub(crate) fn family_self_owns(&self, appid: u32) -> bool {
        self.social
            .as_ref()
            .and_then(|social| social.family.as_ref())
            .and_then(|family| family.members.iter().find(|member| member.is_self))
            .is_some_and(|member| member.appids.contains(&appid))
    }

    /// Vrai si un autre membre partage ce jeu avec la famille.
    pub(crate) fn family_shared_by_other(&self, appid: u32) -> bool {
        self.social
            .as_ref()
            .and_then(|social| social.family.as_ref())
            .is_some_and(|family| {
                family
                    .members
                    .iter()
                    .any(|member| !member.is_self && member.appids.contains(&appid))
            })
    }

    /// Version retenue pour un jeu : celle du menu du bouton quand elle est
    /// toujours publiée, sinon la première proposée (Linux en tête).
    pub(super) fn version_index(&self, appid: u32, versions: &[GameVersion]) -> usize {
        version_index(
            self.version_choice.get(&appid).map(String::as_str),
            versions,
        )
    }

    /// Version que la demande de téléchargement doit envoyer au moteur :
    /// `None` quand la fiche ne publie encore aucune plateforme, pour laisser
    /// la préférence de Lapinux décider.
    fn version_for(&self, appid: u32) -> Option<String> {
        let catalog = self.catalog.as_ref()?;
        let entry = catalog.entries.iter().find(|entry| entry.appid == appid)?;
        let versions = game_versions(&entry.details);
        let chosen = self.version_index(appid, &versions);
        versions.get(chosen).map(|version| version.id.to_string())
    }

    /// Transfert en cours pour un jeu : de quoi peindre la barre de l'encart.
    pub(super) fn running_job(&self, appid: u32) -> Option<(f32, String)> {
        self.jobs
            .iter()
            .find(|job| {
                job.appid == Some(appid)
                    && job.kind == JobKind::InstallGame
                    && matches!(
                        job.state,
                        JobState::Running | JobState::Queued | JobState::Paused
                    )
            })
            .map(|job| (job.progress, job.id.clone()))
    }

    /// Annule une tâche depuis la croix de l'encart : le moteur s'arrête, les
    /// fichiers déjà reçus restent sur le disque.
    pub(super) fn cancel_job(&mut self, id: &str) {
        match self.client.request(&Req::CancelJob { id: id.to_string() }) {
            Ok(envelope) if envelope.ok => {
                self.notify(t("game_contents.download_cancelled"))
            }
            Ok(envelope) => self.notify(
                envelope
                    .error
                    .unwrap_or_else(|| t("ui.cancel_failed_title").into()),
            ),
            Err(err) => self.notify(t_args(
                "ui.cancel_failed",
                &[("err", &err.to_string())],
            )),
        }
        if let Some(jobs) = ui_request::<Vec<Job>>(&self.client, &Req::Jobs) {
            self.jobs = jobs;
        }
    }

    pub(super) fn install_game(&mut self, appid: u32, dlc: Vec<u32>) {
        let updating = self
            .games
            .iter()
            .any(|game| game.appid == appid && game.installed);
        // Une mise à jour complète l'installation existante : elle garde la
        // version déjà sur le disque, que le menu ne concerne pas. Seule une
        // installation neuve envoie le choix affiché sur le bouton.
        let platform = if updating {
            None
        } else {
            self.version_for(appid)
        };
        self.queue_install(appid, dlc, platform);
    }

    /// Reprend une tâche arrêtée avec exactement ce qu'elle installait : ses
    /// contenus additionnels et la version choisie. La reprise repartait sinon
    /// sur une installation nue, sans les DLC demandés.
    pub(super) fn resume_job(&mut self, job: &Job) {
        let Some(appid) = job.appid else {
            return;
        };
        // Une tâche de boutique repart par sa boutique. Confiée au moteur
        // Steam, elle échouait (« PICS ne connaît pas l'application ») et
        // portait la marque Steam sur un jeu Epic.
        if let Some(store) = Self::job_store(job) {
            let store_id = self
                .stores
                .view
                .libraries
                .iter()
                .flat_map(|library| library.entries.iter())
                .find(|entry| entry.appid == appid && entry.store == store)
                .map(|entry| entry.store_id.clone());
            if let Some(store_id) = store_id {
                if let Some(message) =
                    self.stores
                        .start_install(&self.client, store, &store_id, job.platform.clone())
                {
                    self.notify(message);
                }
                return;
            }
        }
        self.queue_install(appid, job.dlc.clone(), job.platform.clone());
    }

    /// Boutique qui mène une tâche, d'après le nom de son moteur : les
    /// téléchargements Steam passent par le moteur interne et n'en ont donc
    /// pas.
    pub(crate) fn job_store(job: &Job) -> Option<lapinux_core::stores::Store> {
        const STORES: [lapinux_core::stores::Store; 3] = [
            lapinux_core::stores::Store::Epic,
            lapinux_core::stores::Store::Gog,
            lapinux_core::stores::Store::Itch,
        ];
        let backend = job.backend.trim();
        STORES.into_iter().find(|store| {
            backend.eq_ignore_ascii_case(store.label()) || backend.eq_ignore_ascii_case(store.key())
        })
    }

    fn queue_install(&mut self, appid: u32, dlc: Vec<u32>, platform: Option<String>) {
        if self.jobs.iter().any(|job| {
            job.appid == Some(appid)
                && matches!(
                    job.state,
                    JobState::Running | JobState::Queued | JobState::Paused
                )
        }) {
            self.notify(t("game_contents.already_downloading"));
            return;
        }
        let updating = self
            .games
            .iter()
            .any(|game| game.appid == appid && game.installed);
        let contents = dlc.len();
        match self.client.request(&Req::InstallGame {
            appid,
            dlc,
            platform,
        }) {
            Ok(envelope) if envelope.ok => {
                let queued = envelope
                    .data
                    .get("queued")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false);
                self.notify(if queued {
                    t_args(
                        "game_contents.queued",
                        &[("appid", &appid.to_string())],
                    )
                } else if contents > 0 {
                    t_args(
                        "game_contents.contents_downloading",
                        &[
                            ("contents", &contents.to_string()),
                            ("appid", &appid.to_string()),
                        ],
                    )
                } else if updating {
                    t_args(
                        "game_contents.update_started",
                        &[("appid", &appid.to_string())],
                    )
                } else {
                    t_args(
                        "game_contents.download_started",
                        &[("appid", &appid.to_string())],
                    )
                });
            }
            Ok(envelope) => self.notify(
                envelope
                    .error
                    .unwrap_or_else(|| t("downloads.failed").into()),
            ),
            Err(err) => self.notify(format!("{err}")),
        }
        // La tâche apparaît tout de suite : sans cela, un second clic pourrait
        // relancer le même téléchargement avant le rafraîchissement suivant.
        if let Some(jobs) = ui_request::<Vec<Job>>(&self.client, &Req::Jobs) {
            self.jobs = jobs;
        }
    }

    pub(super) fn uninstall_window(&mut self, ctx: &egui::Context) {
        let Some(appid) = self.uninstall_modal else {
            return;
        };
        if let Some(result) = self.uninstall_pending.as_ref().map(|rx| rx.try_recv()) {
            match result {
                Ok(Ok(backup)) => {
                    self.uninstall_pending = None;
                    self.uninstall_modal = None;
                    self.notify(t(if backup.is_some() {
                        "game_contents.uninstalled_backup"
                    } else {
                        "game_contents.uninstalled_no_backup"
                    }));
                    if let Some(catalog) = &mut self.catalog {
                        if let Some(entry) = catalog
                            .entries
                            .iter_mut()
                            .find(|entry| entry.appid == appid)
                        {
                            entry.installed = false;
                        }
                    }
                    self.games.retain(|game| game.appid != appid);
                    if self.selected == Some(appid) {
                        self.selected = None;
                    }
                    self.downloads.mark_updates_dirty();
                    return;
                }
                Ok(Err(error)) => {
                    self.uninstall_pending = None;
                    self.uninstall_error = Some(error);
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.uninstall_pending = None;
                    self.uninstall_error =
                        Some(t("game_contents.interrupted").into());
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(Duration::from_millis(200))
                }
            }
        }
        let name = self
            .games
            .iter()
            .find(|game| game.appid == appid)
            .map(|game| game.name.clone())
            .unwrap_or_default();
        let busy = self.uninstall_pending.is_some();
        let mut confirm = false;
        let mut cancel = false;
        let close = fixed_panel(
            ctx,
            "uninstall-saves-panel",
            t("game_contents.uninstall_confirm"),
            &name,
            egui::vec2(540.0, 350.0),
            |ui| {
                ui.label(t("game_contents.delete_warning"));
                ui.add_space(12.0);
                ui.label(t("game_contents.backup_before_delete"));
                muted(ui, t("game_contents.detection_warning"));
                if let Some(error) = &self.uninstall_error {
                    ui.label(egui::RichText::new(error).color(DANGER));
                }
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            !busy,
                            egui::Button::new(t("game_contents.cancel")),
                        )
                        .clicked()
                    {
                        cancel = true;
                    }
                    if ui
                        .add_enabled(
                            !busy,
                            egui::Button::new(if busy {
                                t("game_contents.copying")
                            } else {
                                t("game_contents.uninstall")
                            }),
                        )
                        .clicked()
                    {
                        confirm = true;
                    }
                });
            },
        );
        if (close || cancel) && !busy {
            self.uninstall_modal = None;
        }
        if confirm {
            self.uninstall_error = None;
            let (tx, rx) = mpsc::channel();
            self.uninstall_pending = Some(rx);
            let client = self.client.detached();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let result = client
                    .request_with_timeout(
                        &Req::UninstallGameWithBackup { appid },
                        Duration::from_secs(600),
                    )
                    .and_then(|envelope| envelope.into_data::<serde_json::Value>())
                    .map(|value| {
                        value
                            .get("backup")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string)
                    })
                    .map_err(|e| e.to_string());
                let _ = tx.send(result);
                ctx.request_repaint();
            });
        }
    }

    /// Ouvre la fenêtre de réglages sur un onglet donné.
    pub(super) fn open_settings(&mut self, tab: SettingsTab) {
        if self.settings_draft.is_none() {
            self.settings_draft = Some(self.config.clone());
        }
        tracing::debug!("settings opened: {tab:?}");
        // Un écran a pu être branché depuis le lancement.
        self.displays = lapinux_core::display::displays();
        self.settings_open = true;
        self.settings_scroll_reset = true;
        self.settings_tab = tab;
        if tab == SettingsTab::Logs {
            self.logs_poll = Instant::now() - Duration::from_secs(10);
        }
    }

    /// Ferme la fenêtre et efface les secrets saisis (mot de passe, jeton,
    /// code Steam Guard). Une connexion déjà lancée continue dans le daemon.
    pub(super) fn close_settings(&mut self) {
        // Le brouillon part normalement tout seul après une courte pause ;
        // fermer tout de suite ne doit pas perdre le dernier changement.
        if let Some(draft) = self.settings_draft.clone() {
            if draft != self.config {
                self.apply_config(draft);
            }
        }
        self.settings_open = false;
        self.settings_draft = None;
        self.steam_password.clear();
        self.steam_token.clear();
        self.steam_guard.clear();
    }
}
