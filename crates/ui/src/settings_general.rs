//! Onglet Général : langue, comportement de la fenêtre et lancement.

use super::*;

impl LapinuxApp {
    pub(super) fn settings_general_tab(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        let mut config = self
            .settings_draft
            .clone()
            .unwrap_or_else(|| self.config.clone());
        let proton_names: Vec<String> = self.protons.iter().map(|p| p.name.clone()).collect();
        let automatic_proton = self.automatic_proton_label();
        let displays = self.displays.clone();

        settings_card(
            ui,
            lapinux_core::i18n::t("settings.language.interface"),
            lapinux_core::i18n::t("settings.language.interface_desc"),
            |ui| {
                settings_row(
                    ui,
                    lapinux_core::i18n::t("settings.language.interface"),
                    "",
                    |ui| {
                        let selected = lapinux_core::i18n::LANGUAGES
                            .iter()
                            .find(|(code, _)| *code == config.general.lang)
                            .map(|(_, label)| *label)
                            .unwrap_or_default();
                        egui::ComboBox::from_id_salt("settings-interface-language")
                            .width(180.0)
                            .selected_text(selected)
                            .show_ui(ui, |ui| {
                                for (code, label) in lapinux_core::i18n::LANGUAGES {
                                    ui.selectable_value(
                                        &mut config.general.lang,
                                        (*code).to_string(),
                                        *label,
                                    );
                                }
                            });
                    },
                );
                settings_row(
                    ui,
                    lapinux_core::i18n::t("game_sheet.game_language"),
                    lapinux_core::i18n::t("game_sheet.language_hint"),
                    |ui| {
                        let selected = if config.general.game_language.is_empty() {
                            lapinux_core::i18n::t("settings.general.game_language_auto").to_string()
                        } else {
                            lapinux_core::lang::label(&config.general.game_language)
                                .map(str::to_string)
                                .unwrap_or_else(|| config.general.game_language.clone())
                        };
                        egui::ComboBox::from_id_salt("settings-game-language")
                            .width(190.0)
                            .selected_text(selected)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut config.general.game_language,
                                    String::new(),
                                    lapinux_core::i18n::t("settings.general.game_language_auto"),
                                );
                                for language in lapinux_core::lang::GAME_LANGUAGES {
                                    ui.selectable_value(
                                        &mut config.general.game_language,
                                        language.code.to_string(),
                                        language.label,
                                    );
                                }
                            });
                    },
                );
            },
        );

        settings_card(
            ui,
            lapinux_core::i18n::t("settings.general.application_card"),
            lapinux_core::i18n::t("settings.general.subtitle"),
            |ui| {
                settings_row(
                    ui,
                    lapinux_core::i18n::t("settings.general.parking"),
                    lapinux_core::i18n::t("settings.general.tray_note"),
                    |ui| {
                        egui::ComboBox::from_id_salt("settings-parking")
                            .width(170.0)
                            .selected_text(parking_label(config.general.tray_idle_minutes))
                            .show_ui(ui, |ui| {
                                for minutes in [0, 5, 10, 15, 30, 60] {
                                    ui.selectable_value(
                                        &mut config.general.tray_idle_minutes,
                                        minutes,
                                        parking_label(minutes),
                                    );
                                }
                            });
                    },
                );
                settings_row(
                    ui,
                    lapinux_core::i18n::t("settings.general.activity_retention"),
                    lapinux_core::i18n::t("settings.general.activity_retention_desc"),
                    |ui| {
                        let suffix = format!(
                            " {}",
                            lapinux_core::i18n::t("settings.general.activity_retention_unit")
                        );
                        settings_slider(
                            ui,
                            &mut config.general.activity_retention_days,
                            1..=90,
                            &suffix,
                        );
                    },
                );
            },
        );

        settings_card(
            ui,
            lapinux_core::i18n::t("settings.general.launch_section"),
            lapinux_core::i18n::t("settings.general.launch_desc"),
            |ui| {
                settings_row(
                    ui,
                    lapinux_core::i18n::t("settings.general.default_proton"),
                    if proton_names.is_empty() {
                        lapinux_core::i18n::t("settings.general.no_proton")
                    } else if config.launch.proton == "auto" {
                        lapinux_core::i18n::t("settings.general.proton_auto_desc")
                    } else {
                        lapinux_core::i18n::t("settings.general.proton_desc")
                    },
                    |ui| {
                        egui::ComboBox::from_id_salt("settings-proton")
                            .width(180.0)
                            .selected_text(if config.launch.proton == "auto" {
                                automatic_proton.clone()
                            } else {
                                config.launch.proton.clone()
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut config.launch.proton,
                                    "auto".to_string(),
                                    lapinux_core::i18n::t("settings.general.proton_auto"),
                                );
                                if !proton_names.is_empty() {
                                    ui.separator();
                                }
                                for name in &proton_names {
                                    ui.selectable_value(
                                        &mut config.launch.proton,
                                        name.clone(),
                                        name,
                                    );
                                }
                            });
                    },
                );

                settings_row(
                    ui,
                    lapinux_core::i18n::t("settings.general.display"),
                    lapinux_core::i18n::t("settings.general.display_desc"),
                    |ui| {
                        egui::ComboBox::from_id_salt("settings-display")
                            .width(190.0)
                            .selected_text(if config.launch.output.is_empty() {
                                lapinux_core::i18n::t("settings.general.display_auto").to_string()
                            } else {
                                displays
                                    .iter()
                                    .find(|display| display.name == config.launch.output)
                                    .map(|display| display.label())
                                    .unwrap_or_else(|| config.launch.output.clone())
                            })
                            .show_ui(ui, |ui| {
                                if ui
                                    .selectable_label(
                                        config.launch.output.is_empty(),
                                        lapinux_core::i18n::t("settings.general.display_auto"),
                                    )
                                    .clicked()
                                {
                                    config.launch.output.clear();
                                    if let Some(display) = displays.iter().find(|d| d.primary) {
                                        config.launch.apply_display(display);
                                    }
                                }
                                for display in &displays {
                                    if ui
                                        .selectable_label(
                                            config.launch.output == display.name,
                                            display.label(),
                                        )
                                        .clicked()
                                    {
                                        config.launch.apply_display(display);
                                    }
                                }
                            });
                    },
                );

                settings_row(
                    ui,
                    lapinux_core::i18n::t("settings.general.resolution"),
                    lapinux_core::i18n::t("settings.general.resolution_desc"),
                    |ui| {
                        ui.add_sized(
                            [88.0, 24.0],
                            egui::DragValue::new(&mut config.launch.height),
                        );
                        ui.label("×");
                        ui.add_sized([88.0, 24.0], egui::DragValue::new(&mut config.launch.width));
                    },
                );

                settings_row(
                    ui,
                    lapinux_core::i18n::t("settings.general.fps_limit"),
                    lapinux_core::i18n::t("settings.general.refresh_desc"),
                    |ui| {
                        fps_picker(ui, "settings-fps", &mut config.launch.fps_cap);
                    },
                );

                // Certains jeux interrogent le client Steam lui-même : ce
                // réglage le démarre sans fenêtre avant Proton.
                settings_row(
                    ui,
                    lapinux_core::i18n::t("settings.general.steam_headless"),
                    lapinux_core::i18n::t("settings.general.steam_headless_desc"),
                    |ui| {
                        switch(ui, &mut config.launch.steam_headless);
                    },
                );
            },
        );

        self.settings_draft = Some(config);
    }
}
