//! Versions et mises à jour de Proton.

use super::*;

impl LapinuxApp {
    /// Page **Protons**, à la manière de ProtonPlus : un en-tête, une liste de
    /// sources — nom, ce qu'elle apporte, état, chevron — dont chacune déplie
    /// tous ses builds, puis les versions du disque.
    pub(super) fn protons_page(&mut self, ui: &mut egui::Ui) {
        // La liste se remplit toute seule à la première ouverture : personne ne
        // doit cliquer « Vérifier » pour voir les builds.
        self.poll_proton_releases(ui.ctx());
        if !self.protons_auto {
            self.protons_auto = true;
            self.fetch_proton_releases(ui.ctx());
        }

        // En-tête : ce que fait l'outil, ce qu'il occupe sur le disque, et les
        // deux relectures à droite.
        let mut open_proton_folder = false;
        let weight = self.proton_total_line();
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new("Proton")
                        .size(15.0)
                        .strong()
                        .color(TEXT()),
                );
                muted(ui, lapinux_core::i18n::t("protons.subtitle"));
                if !weight.is_empty() {
                    muted(ui, &weight);
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if action_button(
                    ui,
                    lapinux_core::i18n::t("protons.detect"),
                    ActionTone::Quiet,
                )
                .on_hover_text(lapinux_core::i18n::t("protons.detect_hint"))
                .clicked()
                {
                    self.rescan_protons();
                }
                if action_button(
                    ui,
                    lapinux_core::i18n::t("protons.check_updates"),
                    ActionTone::Light,
                )
                .on_hover_text(lapinux_core::i18n::t("protons.check_hint"))
                .clicked()
                {
                    self.check_proton_updates();
                }
                // Le dossier des Protons, d'un clic : on y dépose un build à
                // la main, ou on vérifie ce qui occupe le disque.
                if card_glyph_button(
                    ui,
                    CardGlyph::Folder,
                    lapinux_core::i18n::t("protons.open_folder"),
                )
                .clicked()
                {
                    open_proton_folder = true;
                }
            });
        });
        if open_proton_folder {
            open_path(&self.paths.proton_dir);
        }
        ui.add_space(6.0);
        muted(ui, &self.proton_status_line());
        ui.add_space(12.0);

        let releases = self.releases.clone();
        let protons = self.protons.clone();
        // Poids de chaque build : mesuré hors de l'image, affiché dès qu'il
        // est connu. Un Proton pèse plusieurs centaines de mégaoctets.
        self.proton_sizes.ensure(ui.ctx(), &protons);
        self.proton_sizes.poll();
        let sizes = self.proton_sizes.sizes.clone();
        let recommended = protons
            .iter()
            .find(|proton| proton.runnable)
            .map(|proton| proton.name.clone());
        let mut install: Vec<ProtonRelease> = Vec::new();
        // Un build déjà en cours de téléchargement n'est pas reproposé : sa
        // ligne en montre l'avancement.
        let downloading: Vec<String> = self
            .jobs
            .iter()
            .filter(|job| job.kind == JobKind::InstallProton && job.state == JobState::Running)
            .map(|job| release_folder(&job.label).to_string())
            .collect();
        let latest: Vec<(&str, ProtonRelease)> = PROTON_SOURCES
            .iter()
            .filter_map(|(repo, label)| {
                latest_release(&releases, repo).map(|release| (*label, release.clone()))
            })
            .collect();
        let missing: Vec<ProtonRelease> = latest
            .iter()
            .filter(|(_, release)| {
                let folder = release_folder(&release.asset);
                !protons.iter().any(|proton| proton.name == folder)
                    && !downloading.iter().any(|name| name == folder)
            })
            .map(|(_, release)| release.clone())
            .collect();

        if releases.is_empty() {
            egui::Frame::none()
                .fill(CARD())
                .stroke(egui::Stroke::new(1.0_f32, BORDER()))
                .rounding(egui::Rounding::same(12.0))
                .inner_margin(14.0)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(
                        egui::RichText::new(lapinux_core::i18n::t("protons.published"))
                            .size(13.0)
                            .strong(),
                    );
                    if self.proton_fetch.is_some() {
                        ui.horizontal(|ui| {
                            crate::section_icons::loading(ui, 18.0);
                            muted(ui, lapinux_core::i18n::t("protons.reading"));
                        });
                    } else {
                        muted(ui, lapinux_core::i18n::t("protons.sources_note"));
                    }
                });
            ui.add_space(10.0);
        } else if !missing.is_empty() {
            // La proposition : tout ce qui manque, en une seule action.
            let names = missing
                .iter()
                .map(|release| release.tag.as_str())
                .collect::<Vec<_>>()
                .join(" · ");
            let title = match missing.len() {
                1 => lapinux_core::i18n::t("protons.install_one").to_string(),
                count => lapinux_core::i18n::t_args(
                    "protons.install_many",
                    &[("count", &count.to_string())],
                ),
            };
            egui::Frame::none()
                .fill(with_alpha(OK, 0.10))
                .stroke(egui::Stroke::new(1.0_f32, with_alpha(OK, 0.35)))
                .rounding(egui::Rounding::same(12.0))
                .inner_margin(14.0)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(egui::RichText::new(title).size(13.0).strong());
                            muted(ui, &names);
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if action_button(
                                ui,
                                lapinux_core::i18n::t("protons.install_all"),
                                ActionTone::Primary,
                            )
                            .on_hover_text(lapinux_core::i18n::t("protons.install_missing_hint"))
                            .clicked()
                            {
                                install.extend(missing.iter().cloned());
                            }
                        });
                    });
                });
            ui.add_space(10.0);
        } else if !protons.is_empty() && !latest.is_empty() {
            ui.horizontal(|ui| {
                chip(ui, lapinux_core::i18n::t("protons.up_to_date"), OK);
                muted(ui, lapinux_core::i18n::t("protons.all_installed"));
            });
            ui.add_space(10.0);
        }

        // Une source par ligne : son nom, ce qu'elle apporte, son état, et un
        // chevron qui déplie tous ses builds — le plus récent en tête.
        for (repo, label) in PROTON_SOURCES {
            let builds = source_builds(&releases, repo);
            let newest = builds.first().copied();
            let open = self.proton_open.get(repo).copied().unwrap_or(false);
            let mut tags: Vec<(&str, egui::Color32)> = Vec::new();
            if let Some(name) = recommended.as_deref() {
                if builds
                    .iter()
                    .any(|build| same_proton_family(name, release_folder(&build.asset)))
                {
                    tags.push((lapinux_core::i18n::t("protons.recommended"), OK));
                }
            }
            let newest_installed = newest
                .map(|build| {
                    protons
                        .iter()
                        .any(|proton| proton.name == release_folder(&build.asset))
                })
                .unwrap_or(false);
            if builds.is_empty() {
                tags.push((lapinux_core::i18n::t("protons.installed_only"), TEXT_DIM()));
            } else if newest_installed {
                tags.push((lapinux_core::i18n::t("protons.up_to_date"), OK));
            } else if protons.iter().any(|proton| {
                builds
                    .iter()
                    .any(|build| same_proton_family(&proton.name, release_folder(&build.asset)))
            }) {
                tags.push((lapinux_core::i18n::t("protons.update"), WARN));
            }
            let mut toggle = false;
            egui::Frame::none()
                .fill(CARD())
                .rounding(egui::Rounding::same(10.0))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    if proton_source_header(ui, label, source_blurb(repo), &tags, open) {
                        toggle = true;
                    }
                    if open {
                        ui.add_space(2.0);
                        hairline(ui);
                        ui.add_space(4.0);
                        // Les builds s'alignent sur le texte de l'en-tête, pas
                        // sur le bord de la carte.
                        ui.horizontal(|ui| {
                            ui.add_space(14.0);
                            ui.vertical(|ui| {
                                ui.set_width(ui.available_width() - 14.0);
                                for build in &builds {
                                    let folder = release_folder(&build.asset);
                                    let installed =
                                        protons.iter().any(|proton| proton.name == folder);
                                    let running = self.jobs.iter().find(|job| {
                                        job.kind == JobKind::InstallProton
                                            && job.state == JobState::Running
                                            && release_folder(&job.label) == folder
                                    });
                                    let progress = running
                                        .map(|job| (job.progress, job.downloaded, job.total));
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            egui::RichText::new(&build.tag)
                                                .size(12.0)
                                                .strong()
                                                .color(TEXT()),
                                        );
                                        if newest
                                            .map(|newest| newest.asset == build.asset)
                                            .unwrap_or(false)
                                        {
                                            chip(ui, "latest", ACCENT());
                                        }
                                        if build.prerelease {
                                            chip(
                                                ui,
                                                lapinux_core::i18n::t("protons.prerelease"),
                                                TEXT_DIM(),
                                            );
                                        }
                                        muted(
                                            ui,
                                            &lapinux_core::i18n::t_args(
                                                "protons.build_date_size",
                                                &[
                                                    ("date", &release_date(&build.published)),
                                                    (
                                                        "size",
                                                        &lapinux_core::util::format_bytes(
                                                            build.size,
                                                        ),
                                                    ),
                                                ],
                                            ),
                                        );
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| match progress {
                                                Some((fraction, done, total)) => {
                                                    let percent = (fraction * 100.0).round() as u32;
                                                    ui.label(
                                                        egui::RichText::new(
                                                            lapinux_core::i18n::t_args(
                                                                "protons.percent",
                                                                &[(
                                                                    "percent",
                                                                    &percent.to_string(),
                                                                )],
                                                            ),
                                                        )
                                                        .size(11.5)
                                                        .strong()
                                                        .color(OK),
                                                    );
                                                    ui.add(
                                                        egui::ProgressBar::new(fraction)
                                                            .desired_width(130.0)
                                                            .fill(OK)
                                                            .rounding(4.0),
                                                    )
                                                    .on_hover_text(lapinux_core::i18n::t_args(
                                                        "protons.downloaded",
                                                        &[
                                                            (
                                                                "done",
                                                                &lapinux_core::util::format_bytes(
                                                                    done,
                                                                ),
                                                            ),
                                                            (
                                                                "total",
                                                                &lapinux_core::util::format_bytes(
                                                                    total.max(done),
                                                                ),
                                                            ),
                                                        ],
                                                    ));
                                                }
                                                None if installed => {
                                                    chip(
                                                        ui,
                                                        lapinux_core::i18n::t("protons.installed"),
                                                        OK,
                                                    );
                                                }
                                                None => {
                                                    if action_button(
                                                        ui,
                                                        lapinux_core::i18n::t("protons.install"),
                                                        ActionTone::Primary,
                                                    )
                                                    .on_hover_text(lapinux_core::i18n::t_args(
                                                        "protons.install_hint",
                                                        &[("asset", &build.asset)],
                                                    ))
                                                    .clicked()
                                                    {
                                                        install.push((**build).clone());
                                                    }
                                                }
                                            },
                                        );
                                    });
                                    ui.add_space(6.0);
                                }
                                if builds.is_empty() {
                                    muted(ui, lapinux_core::i18n::t("protons.none_published"));
                                }
                            });
                        });
                    }
                });
            ui.add_space(6.0);
            if toggle {
                self.proton_open.insert(repo.to_string(), !open);
            }
        }

        // Versions présentes sur le disque, la recommandée en tête.
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(lapinux_core::i18n::t("protons.installed_section"))
                    .size(12.5)
                    .strong(),
            );
            chip(ui, &protons.len().to_string(), TEXT_DIM());
        });
        ui.add_space(8.0);
        let mut open_proton: Option<std::path::PathBuf> = None;
        let mut remove_proton: Option<String> = None;
        let mut arm_proton: Option<String> = None;
        let armed = self.proton_confirm.clone();
        let mut listed: Vec<&ProtonInfo> = protons.iter().collect();
        listed.sort_by(|a, b| {
            let rank = |proton: &ProtonInfo| {
                (
                    Some(&proton.name) != recommended.as_ref(),
                    !proton.runnable,
                    proton.name.to_lowercase(),
                )
            };
            rank(a).cmp(&rank(b))
        });
        if listed.is_empty() {
            muted(ui, lapinux_core::i18n::t("protons.none_installed"));
        } else {
            // Une carte par build, comme les tâches de téléchargement : le nom
            // sur sa ligne, la provenance, le poids et la version dessous, et
            // les deux actions collées au bord droit.
            for proton in listed.iter().copied() {
                let is_armed = armed.as_deref() == Some(proton.name.as_str());
                let weight = sizes
                    .get(&proton.path)
                    .copied()
                    .filter(|bytes| *bytes > 0)
                    .map(lapinux_core::util::format_bytes)
                    .unwrap_or_default();
                egui::Frame::none()
                    .fill(CARD())
                    .rounding(egui::Rounding::same(10.0))
                    .inner_margin(egui::Margin::symmetric(14.0, 9.0))
                    .stroke(if is_armed {
                        egui::Stroke::new(1.0_f32, DANGER)
                    } else {
                        egui::Stroke::NONE
                    })
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            const ICON: f32 = 24.0;
                            const GAP: f32 = 6.0;
                            let actions = ICON * 2.0 + GAP + 10.0;
                            let text_width = (ui.available_width() - actions).max(180.0);
                            ui.allocate_ui_with_layout(
                                egui::vec2(text_width, 38.0),
                                egui::Layout::top_down(egui::Align::Min),
                                |ui| {
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 6.0;
                                        if Some(&proton.name) == recommended.as_ref() {
                                            ui.label(egui::RichText::new("★").size(12.0).color(OK))
                                                .on_hover_text(lapinux_core::i18n::t(
                                                    "protons.auto_choice",
                                                ));
                                        }
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(&proton.name)
                                                    .size(12.5)
                                                    .strong()
                                                    .color(TEXT()),
                                            )
                                            .truncate(),
                                        )
                                        .on_hover_text(proton.path.display().to_string());
                                        if !proton.runnable {
                                            chip(
                                                ui,
                                                lapinux_core::i18n::t("protons.incomplete"),
                                                WARN,
                                            )
                                            .on_hover_text(lapinux_core::i18n::t(
                                                "protons.broken_build",
                                            ));
                                        }
                                    });
                                    ui.add_space(5.0);
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 8.0;
                                        chip(ui, &proton.source, ACCENT());
                                        if !weight.is_empty() {
                                            muted(ui, &weight);
                                        }
                                        if !proton.version.trim().is_empty() {
                                            ui.add(
                                                egui::Label::new(
                                                    egui::RichText::new(&proton.version)
                                                        .size(11.5)
                                                        .color(TEXT_DIM()),
                                                )
                                                .truncate(),
                                            );
                                        }
                                    });
                                },
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.spacing_mut().item_spacing.x = GAP;
                                    let confirm = if is_armed {
                                        row_confirm_button(
                                            ui,
                                            lapinux_core::i18n::t("protons.confirm_remove"),
                                        )
                                    } else {
                                        row_glyph_button(
                                            ui,
                                            CardGlyph::Trash,
                                            lapinux_core::i18n::t("protons.remove"),
                                        )
                                    };
                                    if confirm.clicked() {
                                        if is_armed {
                                            remove_proton = Some(proton.name.clone());
                                        } else {
                                            arm_proton = Some(proton.name.clone());
                                        }
                                    }
                                    if row_glyph_button(
                                        ui,
                                        CardGlyph::Folder,
                                        lapinux_core::i18n::t("protons.open_build_folder"),
                                    )
                                    .clicked()
                                    {
                                        open_proton = Some(proton.path.clone());
                                    }
                                },
                            );
                        });
                    });
                ui.add_space(7.0);
            }
        }

        if let Some(path) = open_proton {
            open_path(&path);
        }
        if let Some(name) = arm_proton {
            self.proton_confirm = Some(name);
        }
        if let Some(name) = remove_proton.take() {
            self.proton_confirm = None;
            match self
                .client
                .request(&Req::RemoveProton { name: name.clone() })
            {
                Ok(envelope) if envelope.ok => {
                    self.protons = envelope.into_data::<Vec<ProtonInfo>>().unwrap_or_default();
                    self.notify(lapinux_core::i18n::t_args(
                        "protons.removed",
                        &[("name", &name)],
                    ));
                }
                Ok(envelope) => self.notify(
                    envelope
                        .error
                        .unwrap_or_else(|| lapinux_core::i18n::t("protons.not_removed").into()),
                ),
                Err(err) => self.notify(format!("{err}")),
            }
        }

        // Les demandes partent après le dessin : une ligne cliquée ou
        // plusieurs via « Tout installer ».
        for release in install {
            match self.client.request(&Req::InstallProton {
                repo: release.repo.clone(),
                asset: release.asset.clone(),
                checksum_url: release.checksum_url.clone(),
            }) {
                Ok(envelope) if envelope.ok => self.notify(lapinux_core::i18n::t_args(
                    "protons.downloading",
                    &[("asset", &release.asset)],
                )),
                Ok(envelope) => self.notify(envelope.error.unwrap_or_default()),
                Err(err) => self.notify(format!("{err}")),
            }
        }
    }

    /// Lit les builds publiés en tâche de fond : trois dépôts interrogés, et
    /// l'interface doit rester vivante pendant ce temps.
    fn fetch_proton_releases(&mut self, ctx: &egui::Context) {
        if self.proton_fetch.is_some() {
            return;
        }
        let client = self.client.detached();
        let (tx, rx) = mpsc::channel();
        let wake = ctx.clone();
        std::thread::spawn(move || {
            let result = client
                .request(&Req::ProtonReleases)
                .map_err(|err| format!("{err}"));
            if tx.send(result).is_ok() {
                wake.request_repaint();
            }
        });
        self.proton_fetch = Some(rx);
        ctx.request_repaint_after(Duration::from_secs(1));
    }

    /// Récolte la lecture de fond : la liste publiée, l'instant du contrôle et
    /// le verdict, comme le bouton « Vérifier ».
    fn poll_proton_releases(&mut self, ctx: &egui::Context) {
        let Some(rx) = self.proton_fetch.take() else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(envelope)) if envelope.ok => {
                self.releases = envelope.into_data().unwrap_or_default();
                self.protons_checked = Some(Instant::now());
                if self.releases.is_empty() {
                    self.notify(lapinux_core::i18n::t("protons.poll_unread"));
                }
            }
            Ok(Ok(envelope)) => self.notify(
                envelope
                    .error
                    .unwrap_or_else(|| lapinux_core::i18n::t("protons.poll_unread_short").into()),
            ),
            Ok(Err(err)) => self.notify(err),
            // Toujours en cours : le canal revient en place et la page se
            // redessine bientôt.
            Err(mpsc::TryRecvError::Empty) => {
                self.proton_fetch = Some(rx);
                ctx.request_repaint_after(Duration::from_secs(1));
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.notify(lapinux_core::i18n::t("protons.poll_interrupted"))
            }
        }
    }

    /// Relit les builds publiés (GitHub, par le daemon) et retient l'instant du
    /// contrôle : la page peut alors dire depuis quand sa liste date.
    fn check_proton_updates(&mut self) {
        match self.client.request(&Req::ProtonReleases) {
            Ok(envelope) if envelope.ok => {
                self.releases = envelope.into_data().unwrap_or_default();
                self.protons_checked = Some(Instant::now());
                if self.releases.is_empty() {
                    self.notify(lapinux_core::i18n::t("protons.check_unread"));
                    return;
                }
                match self.pending_latest() {
                    0 => self.notify(lapinux_core::i18n::t("protons.check_all")),
                    pending => self.notify(lapinux_core::i18n::t_args(
                        "protons.check_pending",
                        &[("pending", &pending.to_string())],
                    )),
                }
            }
            Ok(envelope) => self.notify(envelope.error.unwrap_or_default()),
            Err(err) => self.notify(format!("{err}")),
        }
    }

    /// Relit les Protons présents sur le disque.
    fn rescan_protons(&mut self) {
        self.protons = self
            .client
            .request(&Req::Protons)
            .ok()
            .and_then(|envelope| envelope.into_data().ok())
            .unwrap_or_default();
        self.notify(lapinux_core::i18n::t_args(
            "protons.detected",
            &[("count", &self.protons.len().to_string())],
        ));
    }

    /// Combien de builds « latest » ne sont pas encore installés ?
    fn pending_latest(&self) -> usize {
        PROTON_SOURCES
            .iter()
            .filter_map(|(repo, _)| latest_release(&self.releases, repo))
            .filter(|latest| {
                !self
                    .protons
                    .iter()
                    .any(|proton| proton.name == release_folder(&latest.asset))
            })
            .count()
    }

    /// Ligne d'état du contrôle : sa date et ce qui reste à installer.
    /// Poids total des Protons installés, tel que l'en-tête l'écrit.
    fn proton_total_line(&self) -> String {
        proton_weight_line(&self.protons, &self.proton_sizes.sizes)
    }

    fn proton_status_line(&self) -> String {
        if self.proton_fetch.is_some() {
            return lapinux_core::i18n::t("protons.status_reading").to_string();
        }
        if self.releases.is_empty() {
            return lapinux_core::i18n::t("protons.status_unread").to_string();
        }
        let checked = self
            .protons_checked
            .map(|instant| since_text(instant.elapsed().as_secs()))
            .unwrap_or_else(|| lapinux_core::i18n::t("protons.status_now").to_string());
        match self.pending_latest() {
            0 => lapinux_core::i18n::t_args("protons.status_all", &[("checked", &checked)]),
            pending => lapinux_core::i18n::t_args(
                "protons.status_pending",
                &[("checked", &checked), ("pending", &pending.to_string())],
            ),
        }
    }
}
