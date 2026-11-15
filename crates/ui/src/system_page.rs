//! État du système, diagnostic et daemon.

use super::*;

impl LapinuxApp {
    /// Vue matérielle : processeur, mémoire, cartes graphiques et renderer
    /// réellement utilisé par la session.
    pub(super) fn system_page(&mut self, ui: &mut egui::Ui) {
        // L'identité du daemon se relit doucement : le socket local répond
        // tout de suite, deux secondes suffisent à voir un redémarrage.
        if self.daemon_poll.elapsed() > Duration::from_secs(2) {
            self.daemon_poll = Instant::now();
            self.daemon_build = self.client.daemon_build();
        }
        let info = self.system.clone();
        let cpu_color = meter_color(info.cpu_usage);
        let ram_color = meter_color(info.ram_percent());
        let gpu = info.active_gpu().cloned();
        let (gpu_name, gpu_detail) = match &gpu {
            Some(gpu) => (
                gpu.name.clone(),
                if gpu.driver.is_empty() {
                    lapinux_core::i18n::t("settings.system.active_gpu").to_string()
                } else {
                    lapinux_core::i18n::t_args("settings.system.driver", &[("driver", &gpu.driver)])
                },
            ),
            None => (
                lapinux_core::i18n::t("settings.system.no_gpu").to_string(),
                String::new(),
            ),
        };

        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(lapinux_core::i18n::t("settings.system.load_desc"))
                    .size(11.0)
                    .color(TEXT_DIM()),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if secondary_button(ui, lapinux_core::i18n::t("settings.system.refresh")).clicked()
                {
                    self.system_sampler.rescan();
                    self.system = self.system_sampler.sample();
                    self.system_poll = Instant::now();
                    self.notify(lapinux_core::i18n::t("settings.system.refreshed"));
                }
            });
        });
        ui.add_space(8.0);

        // Les portables et les tours à processeur graphique intégré ont
        // souvent deux cartes : le jeu doit pouvoir choisir la bonne sans
        // dépendre du pilote ou de l'ordre de découverte.
        let mut config = self
            .settings_draft
            .clone()
            .unwrap_or_else(|| self.config.clone());
        let selected_gpu = info
            .machine
            .gpus
            .iter()
            .find(|candidate| candidate.pci == config.general.preferred_gpu)
            .map(|candidate| candidate.name.clone())
            .unwrap_or_else(|| lapinux_core::i18n::t("settings.system.gpu_auto").to_string());
        // Une seule carte pour les trois mesures : la page reste courte même
        // quand le panneau des réglages n'est pas très haut.
        egui::Frame::none()
            .fill(CARD())
            .stroke(egui::Stroke::new(1.0_f32, BORDER()))
            .rounding(egui::Rounding::same(12.0))
            .inner_margin(12.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.columns(3, |columns| {
                    compact_system_metric(
                        &mut columns[0],
                        "⚡",
                        lapinux_core::i18n::t("settings.system.cpu"),
                        &info.machine.cpu_model,
                        &lapinux_core::i18n::t_args(
                            "settings.system.cpu_detail",
                            &[
                                ("threads", &info.machine.cores.to_string()),
                                ("percent", &format!("{:.0}", info.cpu_usage)),
                            ],
                        ),
                        Some((info.cpu_usage / 100.0, cpu_color)),
                    );
                    compact_system_metric(
                        &mut columns[1],
                        "📊",
                        lapinux_core::i18n::t("settings.system.memory"),
                        &format!(
                            "{} / {}",
                            lapinux_core::util::format_gib(info.ram_used),
                            lapinux_core::util::format_gib(info.ram_total)
                        ),
                        &lapinux_core::i18n::t_args(
                            "settings.system.ram_used",
                            &[("percent", &format!("{:.0}", info.ram_percent()))],
                        ),
                        Some((info.ram_percent() / 100.0, ram_color)),
                    );
                    compact_system_metric(
                        &mut columns[2],
                        "🖼",
                        lapinux_core::i18n::t("settings.system.gpu"),
                        &gpu_name,
                        &gpu_detail,
                        None,
                    );
                });
            });

        ui.add_space(8.0);
        egui::Frame::none()
            .fill(CARD())
            .stroke(egui::Stroke::new(1.0_f32, BORDER()))
            .rounding(egui::Rounding::same(12.0))
            .inner_margin(12.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.columns(2, |columns| {
                    let gpu_column = &mut columns[0];
                    gpu_column.label(
                        egui::RichText::new(lapinux_core::i18n::t("settings.system.gpus"))
                            .size(12.5)
                            .strong(),
                    );
                    gpu_column.add_space(4.0);
                    if info.machine.gpus.is_empty() {
                        muted(gpu_column, lapinux_core::i18n::t("settings.system.no_drm"));
                    }
                    for gpu in &info.machine.gpus {
                        gpu_column.horizontal_wrapped(|ui| {
                            status_dot(ui, if gpu.active { OK } else { TEXT_DIM() });
                            ui.label(egui::RichText::new(&gpu.name).size(11.5).strong());
                            if gpu.active {
                                status_pill(ui, lapinux_core::i18n::t("settings.system.used"), OK);
                            }
                        });
                        muted(
                            gpu_column,
                            format!(
                                "{} · {}",
                                if gpu.driver.is_empty() {
                                    lapinux_core::i18n::t("settings.system.unknown_driver")
                                } else {
                                    gpu.driver.as_str()
                                },
                                gpu.pci
                            ),
                        );
                        gpu_column.add_space(3.0);
                    }
                    if let Some(renderer) = &info.machine.renderer {
                        gpu_column.add_space(2.0);
                        gpu_column.separator();
                        gpu_column.add_space(4.0);
                        muted(gpu_column, lapinux_core::i18n::t("settings.system.opengl"));
                        gpu_column.label(egui::RichText::new(renderer).size(11.0).color(TEXT()));
                    }

                    let machine_column = &mut columns[1];
                    machine_column.label(
                        egui::RichText::new(lapinux_core::i18n::t("settings.system.machine"))
                            .size(12.5)
                            .strong(),
                    );
                    machine_column.add_space(4.0);
                    egui::Grid::new("machine-info")
                        .num_columns(2)
                        .spacing([14.0, 3.0])
                        .show(machine_column, |ui| {
                            let mut row = |label: &str, value: &str| {
                                ui.label(egui::RichText::new(label).size(10.5).color(TEXT_DIM()));
                                ui.label(egui::RichText::new(value).size(11.0))
                                    .on_hover_text(value);
                                ui.end_row();
                            };
                            row(
                                lapinux_core::i18n::t("settings.system.hostname"),
                                &info.machine.hostname,
                            );
                            row(
                                lapinux_core::i18n::t("settings.system.system"),
                                &info.machine.os,
                            );
                            row(
                                lapinux_core::i18n::t("settings.system.kernel"),
                                &info.machine.kernel,
                            );
                            row(
                                lapinux_core::i18n::t("settings.system.uptime"),
                                &lapinux_core::util::format_duration(info.uptime_secs),
                            );
                        });
                });
            });

        ui.add_space(8.0);
        self.daemon_card(ui);
        ui.add_space(8.0);
        // Le choix de la carte graphique termine la page : il est rarement
        // modifié et ne doit pas repousser les mesures du système.
        settings_card(
            ui,
            lapinux_core::i18n::t("settings.system.gpu_choice"),
            lapinux_core::i18n::t("settings.system.gpu_choice_desc"),
            |ui| {
                settings_row(
                    ui,
                    lapinux_core::i18n::t("settings.system.gpu_preferred"),
                    lapinux_core::i18n::t("settings.system.gpu_preferred_hint"),
                    |ui| {
                        egui::ComboBox::from_id_salt("preferred-gpu")
                            .width(280.0)
                            .selected_text(selected_gpu)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut config.general.preferred_gpu,
                                    String::new(),
                                    lapinux_core::i18n::t("settings.system.gpu_auto"),
                                );
                                for gpu in &info.machine.gpus {
                                    if gpu.pci.trim().is_empty() {
                                        continue;
                                    }
                                    ui.selectable_value(
                                        &mut config.general.preferred_gpu,
                                        gpu.pci.clone(),
                                        &gpu.name,
                                    );
                                }
                            });
                    },
                );
            },
        );
        self.settings_draft = Some(config);
    }

    /// Encart du daemon : la version qui tourne, le binaire présent sur le
    /// disque et de quoi relancer proprement après une mise à jour.
    fn daemon_card(&mut self, ui: &mut egui::Ui) {
        let binary = lapinux_core::client::daemon_binary().ok();
        let binary_date = binary
            .as_deref()
            .and_then(lapinux_core::paths::binary_stamp);
        let state = daemon_state(self.daemon_build.as_ref(), binary_date);
        let mut restart = false;
        egui::Frame::none()
            .fill(CARD())
            .stroke(egui::Stroke::new(1.0_f32, BORDER()))
            .rounding(egui::Rounding::same(12.0))
            .inner_margin(16.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    status_dot(ui, state.color());
                    ui.label(
                        egui::RichText::new(lapinux_core::i18n::t("ui.daemon"))
                            .size(13.0)
                            .strong(),
                    );
                    status_pill(ui, state.label(), state.color());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if secondary_button(
                            ui,
                            lapinux_core::i18n::t("settings.system.daemon_restart"),
                        )
                        .clicked()
                        {
                            restart = true;
                        }
                    });
                });
                ui.add_space(8.0);
                egui::Grid::new("daemon-build")
                    .num_columns(2)
                    .spacing([18.0, 6.0])
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(lapinux_core::i18n::t(
                                "settings.system.daemon_running_version",
                            ))
                            .size(11.0)
                            .color(TEXT_DIM()),
                        );
                        let running = match &self.daemon_build {
                            // Le daemon répond mais ne publie pas son identité :
                            // il date d'avant ce protocole, donc de ce champ.
                            Some(build) if build.stamp == 0 => {
                                lapinux_core::i18n::t("settings.system.daemon_old").to_string()
                            }
                            Some(build) if build.version.is_empty() => lapinux_core::i18n::t_args(
                                "settings.system.daemon_build",
                                &[("date", &build_stamp_label(build.stamp))],
                            ),
                            Some(build) => lapinux_core::i18n::t_args(
                                "settings.system.daemon_version_build",
                                &[
                                    ("version", &build.version),
                                    ("date", &build_stamp_label(build.stamp)),
                                ],
                            ),
                            None => {
                                lapinux_core::i18n::t("settings.system.daemon_absent").to_string()
                            }
                        };
                        ui.label(running);
                        ui.end_row();

                        ui.label(
                            egui::RichText::new(lapinux_core::i18n::t(
                                "settings.system.daemon_binary",
                            ))
                            .size(11.0)
                            .color(TEXT_DIM()),
                        );
                        match (&binary, binary_date) {
                            (Some(path), Some(date)) => {
                                let full = path.display().to_string();
                                ui.label(lapinux_core::util::ellipsize(&full, 58))
                                    .on_hover_text(lapinux_core::i18n::t_args(
                                        "settings.system.daemon_binary_hint",
                                        &[("path", &full), ("date", &build_stamp_label(date))],
                                    ));
                            }
                            _ => {
                                ui.label(
                                    egui::RichText::new(lapinux_core::i18n::t(
                                        "settings.system.daemon_binary_missing",
                                    ))
                                    .color(WARN),
                                );
                            }
                        }
                        ui.end_row();

                        ui.label(
                            egui::RichText::new(lapinux_core::i18n::t(
                                "settings.system.daemon_started",
                            ))
                            .size(11.0)
                            .color(TEXT_DIM()),
                        );
                        let started = self
                            .daemon_build
                            .as_ref()
                            .map(|build| build.started)
                            .unwrap_or(0);
                        let stamp = lapinux_core::util::now_unix().max(0) as u64;
                        ui.label(if started > 0 {
                            lapinux_core::i18n::t_args(
                                "settings.system.daemon_started_ago",
                                &[
                                    ("date", &lapinux_core::util::format_datetime(started as i64)),
                                    (
                                        "ago",
                                        &lapinux_core::util::format_duration(
                                            stamp.saturating_sub(started),
                                        ),
                                    ),
                                ],
                            )
                        } else {
                            "—".to_string()
                        });
                        ui.end_row();
                    });
                if state == DaemonState::Stale {
                    ui.add_space(6.0);
                    muted(ui, lapinux_core::i18n::t("settings.system.daemon_stale"));
                }
            });

        if restart {
            let restarted = self.client.restart_daemon();
            self.daemon_build = self.client.daemon_build();
            self.daemon_poll = Instant::now();
            // La vue des boutiques vient du daemon : celui qui arrive est un
            // autre binaire, sa réponse est redemandée plutôt que gardée.
            self.stores.forget();
            self.notify(if restarted && self.daemon_build.is_some() {
                lapinux_core::i18n::t("settings.system.daemon_restarted")
            } else {
                lapinux_core::i18n::t("settings.system.daemon_no_response")
            });
        }
    }

    /// Résumé sans données personnelles : réglages visibles, taille de la
    /// bibliothèque et état des comptes. Les noms de jeux et les chemins n'y
    /// entrent pas.
    pub(super) fn app_summary(&self) -> lapinux_core::diagnostic::AppSummary {
        use lapinux_core::diagnostic::AppSummary;
        let entries = self.library_entries_all();
        let installed = entries.iter().filter(|entry| entry.installed).count();
        let active = self.theme_library.active(&self.config.general.theme);
        let theme_kind = if active.builtin {
            "classic"
        } else if active.native {
            "native"
        } else {
            "custom"
        };
        let steam = self.steam_status.as_ref();
        AppSummary {
            language: lapinux_core::i18n::active().to_string(),
            theme_kind: theme_kind.to_string(),
            page: self.page.key().to_string(),
            // Seules les clés connues sortent : une valeur libre du fichier de
            // configuration ne doit pas devenir un texte exporté.
            startup_page: Page::from_key(&self.config.general.startup_page)
                .map(|page| page.key().to_string())
                .unwrap_or_else(|| "default".to_string()),
            big_picture: self.big_picture,
            gamescope: self.config.launch.gamescope,
            end_screen: self.config.general.end_screen,
            capture_on_launch: self.config.capture.record_on_launch,
            discord: self.config.discord.enabled,
            plugins: self.config.general.active_plugins.len() as u32,
            library_total: entries.len() as u32,
            library_installed: installed as u32,
            steam_connected: steam.map(|status| status.connected).unwrap_or(false),
            steam_token_saved: steam.map(|status| status.token_saved).unwrap_or(false),
        }
    }
}
