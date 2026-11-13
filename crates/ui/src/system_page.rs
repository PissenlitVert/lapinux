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

    pub(super) fn doctor_page(&mut self, ui: &mut egui::Ui) {
        if ui.button(lapinux_core::i18n::t("doctor.reload")).clicked() || !self.doctor_requested {
            self.doctor_requested = true;
            self.doctor = self
                .client
                .request_with_timeout(&Req::Doctor, Duration::from_secs(3))
                .ok()
                .and_then(|e| e.into_data().ok());
        }
        ui.add_space(10.0);
        ui.label(lapinux_core::diagnostic::label("hint"));
        ui.horizontal(|ui| {
            let export = ui.button(lapinux_core::diagnostic::label("export")).clicked();
            let copy = ui.button(lapinux_core::diagnostic::label("copy")).clicked();
            if export || copy {
                let client = self.client.detached_quiet();
                let daemon = match client.request_with_timeout(&Req::Ping, Duration::from_millis(600)) {
                    Ok(envelope) => Some(envelope.ok),
                    Err(lapinux_core::Error::IpcTimeout(_)) => None,
                    Err(_) => Some(false),
                };
                let jobs: Option<Vec<Job>> = client.request_with_timeout(&Req::Jobs, Duration::from_secs(2))
                    .ok().and_then(|envelope| envelope.into_data().ok());
                let value = lapinux_core::diagnostic::report(self.doctor.as_ref(), jobs.as_deref(), daemon);
                if let Ok(text) = serde_json::to_string_pretty(&value) {
                    if copy {
                        ui.ctx().copy_text(text);
                        self.notify(lapinux_core::diagnostic::label("copied"));
                    } else if let Some(folder) = pick_folder(&lapinux_core::paths::home_dir(), lapinux_core::diagnostic::label("choose")) {
                        let path = folder.join(format!("lapinux-diagnostic-{}.json", lapinux_core::util::now_unix()));
                        use std::io::Write;
                        use std::os::unix::fs::OpenOptionsExt;
                        let result = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path)
                            .and_then(|mut file| file.write_all(text.as_bytes()));
                        self.notify(lapinux_core::diagnostic::label(if result.is_ok() { "success" } else { "failure" }));
                    }
                } else {
                    self.notify(lapinux_core::diagnostic::label("failure"));
                }
            }
        });
        ui.add_space(8.0);
        let Some(report) = self.doctor.clone() else {
            ui.label(lapinux_core::i18n::t("doctor.unavailable"));
            return;
        };
        for check in report.checks {
            ui.horizontal(|ui| {
                let (symbol, color) = match check.status {
                    CheckStatus::Ok => ("OK", egui::Color32::from_rgb(130, 210, 150)),
                    CheckStatus::Warn => ("ATTN", egui::Color32::from_rgb(230, 200, 120)),
                    CheckStatus::Fail => ("ERR", egui::Color32::from_rgb(230, 130, 130)),
                };
                ui.label(egui::RichText::new(symbol).color(color).strong());
                ui.label(egui::RichText::new(check.name).strong());
            });
            ui.label(
                egui::RichText::new(sentence(&check.detail))
                    .size(11.0)
                    .color(egui::Color32::GRAY),
            );
            ui.add_space(4.0);
        }
    }
}
