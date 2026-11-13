//! Pages des réglages et fiches de sauvegarde.

use super::*;

const PAD_HOTKEY_CHOICES: [&str; 13] = [
    "Guide", "A", "B", "X", "Y", "LB", "RB", "LT", "RT", "Select", "Start", "L3", "R3",
];

fn normalized_pad_hotkey_part(raw: &str) -> Option<&'static str> {
    let code = lapinux_input::codes::button_code(raw)?;
    PAD_HOTKEY_CHOICES
        .iter()
        .copied()
        .find(|choice| lapinux_input::codes::button_code(choice) == Some(code))
}

fn pad_hotkey_parts(value: &str) -> Vec<String> {
    let mut parts = Vec::new();
    for raw in value.split('+') {
        let part = normalized_pad_hotkey_part(raw).unwrap_or(raw.trim());
        if !part.is_empty() && !parts.iter().any(|existing| existing == part) {
            parts.push(part.to_string());
        }
    }
    if parts.is_empty() {
        parts.push("Guide".into());
    }
    parts
}

fn pad_hotkey_label(value: &str) -> String {
    pad_hotkey_parts(value).join(" + ")
}

pub(super) fn pad_hotkey_picker(ui: &mut egui::Ui, value: &mut String) {
    let mut chosen = pad_hotkey_parts(value);
    let mut changed = false;
    egui::ComboBox::from_id_salt("settings-pad-hotkey")
        .width(190.0)
        .selected_text(pad_hotkey_label(value))
        .show_ui(ui, |ui| {
            ui.set_min_width(190.0);
            for button in PAD_HOTKEY_CHOICES {
                let mut enabled = chosen.iter().any(|selected| selected == button);
                let response = ui.horizontal(|ui| {
                    let response = switch(ui, &mut enabled);
                    ui.label(button);
                    response
                });
                if response.inner.changed() {
                    changed = true;
                    if enabled {
                        chosen.push(button.to_string());
                    } else {
                        chosen.retain(|selected| selected != button);
                    }
                }
            }
            ui.separator();
            muted(
                ui,
                lapinux_core::i18n::t("settings.in_game.pad_hotkey_hint"),
            );
        });
    if chosen.is_empty() {
        chosen.push("Guide".into());
    }
    if changed {
        *value = chosen.join("+");
    }
}

#[cfg(test)]
mod pad_hotkey_tests {
    use super::*;

    #[test]
    fn choices_and_old_aliases_share_the_input_codes() {
        for name in PAD_HOTKEY_CHOICES {
            assert!(lapinux_input::codes::button_code(name).is_some(), "{name}");
        }
        assert_eq!(
            pad_hotkey_parts("PS+L1+R1+L3+R3"),
            ["Guide", "LB", "RB", "L3", "R3"]
        );
        assert_eq!(pad_hotkey_parts("BTN_BASE+R1"), ["BTN_BASE", "RB"]);
    }
}

pub(super) fn frame_generation_label(value: lapinux_core::config::FrameGeneration) -> &'static str {
    use lapinux_core::config::FrameGeneration;
    match value {
        FrameGeneration::Off => lapinux_core::i18n::t("graphics.off"),
        FrameGeneration::AfmfLinux => "afmf-linux",
        FrameGeneration::NvidiaSmoothMotion
        | FrameGeneration::OptiScaler
        | FrameGeneration::LsfgVk => lapinux_core::i18n::t("graphics.legacy_disabled"),
    }
}

/// Explication courte du moteur effectif, affichée sous le choix de
/// génération d'images. Elle évite de deviner ce que fait « comme les
/// réglages » quand le choix global change.
pub(super) fn frame_generation_hint(value: lapinux_core::config::FrameGeneration) -> &'static str {
    use lapinux_core::config::FrameGeneration;
    match value {
        FrameGeneration::Off => lapinux_core::i18n::t("graphics.off_hint"),
        FrameGeneration::AfmfLinux => lapinux_core::i18n::t("graphics.afmf_hint"),
        FrameGeneration::NvidiaSmoothMotion
        | FrameGeneration::OptiScaler
        | FrameGeneration::LsfgVk => lapinux_core::i18n::t("graphics.off_hint"),
    }
}

pub(super) fn afmf_performance_label(
    value: lapinux_core::config::AfmfPerformanceMode,
) -> &'static str {
    use lapinux_core::config::AfmfPerformanceMode::*;
    lapinux_core::i18n::t(match value {
        Auto => "graphics.auto",
        Quality => "graphics.afmf_quality",
        Performance => "graphics.afmf_fast",
    })
}

pub(super) fn afmf_search_label(value: lapinux_core::config::AfmfSearchMode) -> &'static str {
    use lapinux_core::config::AfmfSearchMode::*;
    lapinux_core::i18n::t(match value {
        Auto => "graphics.auto",
        Standard => "graphics.afmf_standard",
        High => "graphics.afmf_high",
    })
}

/// Libellé d'un multiplicateur de génération d'images : 0 coupe la génération,
/// les autres intercalent des images entre celles du jeu.
pub(super) fn afmf_multiplier_label(value: u32) -> String {
    if value == 0 {
        lapinux_core::i18n::t("graphics.afmf_multiplier_off").to_string()
    } else {
        format!("×{value}")
    }
}

/// La couche construite sur cette machine est-elle installée ?
pub(super) fn afmf_local_installed(paths: &lapinux_core::paths::LapinuxPaths) -> bool {
    paths
        .tools_dir
        .join("frame-generation/afmf-linux")
        .join(lapinux_core::config::AFMF_LOCAL_VERSION)
        .join("lapinux-local.json")
        .is_file()
}

pub(super) fn vkbasalt_effect_label(value: lapinux_core::config::VkBasaltEffect) -> &'static str {
    use lapinux_core::config::VkBasaltEffect;
    match value {
        VkBasaltEffect::Existing => lapinux_core::i18n::t("graphics.vkbasalt_existing"),
        VkBasaltEffect::Cas => "CAS",
        VkBasaltEffect::Fxaa => "FXAA",
        VkBasaltEffect::Smaa => "SMAA",
        VkBasaltEffect::Ascii => lapinux_core::i18n::t("graphics.vkbasalt_ascii"),
    }
}

/// Ce que fait chaque préréglage vkBasalt, en une phrase : CAS nettoie,
/// FXAA lisse vite, SMAA lisse mieux, ASCII est un délire expérimental.
/// Sans cette ligne, les quatre noms ne disent rien à qui découvre l’outil.
pub(super) fn vkbasalt_effect_about(value: lapinux_core::config::VkBasaltEffect) -> &'static str {
    use lapinux_core::config::VkBasaltEffect;
    match value {
        VkBasaltEffect::Existing => lapinux_core::i18n::t("graphics.vkbasalt_about_existing"),
        VkBasaltEffect::Cas => lapinux_core::i18n::t("graphics.vkbasalt_about_cas"),
        VkBasaltEffect::Fxaa => lapinux_core::i18n::t("graphics.vkbasalt_about_fxaa"),
        VkBasaltEffect::Smaa => lapinux_core::i18n::t("graphics.vkbasalt_about_smaa"),
        VkBasaltEffect::Ascii => lapinux_core::i18n::t("graphics.vkbasalt_about_ascii"),
    }
}

pub(super) fn scaling_filter_label(value: lapinux_core::config::ScalingFilter) -> &'static str {
    use lapinux_core::config::ScalingFilter;
    match value {
        ScalingFilter::Auto => lapinux_core::i18n::t("graphics.auto"),
        ScalingFilter::Fsr => "FSR 1",
        ScalingFilter::Nis => "NIS",
        ScalingFilter::Linear => lapinux_core::i18n::t("graphics.linear"),
        ScalingFilter::Nearest => lapinux_core::i18n::t("graphics.nearest"),
        ScalingFilter::Pixel => "Pixel",
    }
}

/// Une ligne « libellé, valeur » des cartes de mise à jour.
fn update_value_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal_top(|ui| {
        ui.add_sized(
            [138.0, 18.0],
            egui::Label::new(egui::RichText::new(label).size(10.5).color(TEXT_DIM())),
        );
        ui.add(
            egui::Label::new(
                egui::RichText::new(value)
                    .monospace()
                    .size(10.5)
                    .color(TEXT()),
            )
            .wrap(),
        );
    });
}

/// Taille lisible pour une progression de téléchargement.
fn update_size(bytes: u64) -> String {
    let value = format!("{:.1}", bytes as f64 / (1024.0 * 1024.0));
    lapinux_core::i18n::t_args("settings.updates.megabytes", &[("value", value.as_str())])
}

/// Message d'échec commun aux étapes de mise à jour.
fn update_failure(error: &str) -> String {
    lapinux_core::i18n::t_args("settings.updates.failed", &[("error", error)])
}

impl LapinuxApp {
    /// Réglages graphiques communs aux jeux natifs et Proton.
    pub(super) fn settings_graphics_tab(&mut self, ui: &mut egui::Ui) {
        use lapinux_core::config::{FrameGeneration, ScalingFilter};
        let vkbasalt_runtime = lapinux_core::vkbasalt::status(&self.paths);
        let local_afmf = afmf_local_installed(&self.paths);
        let runtime_paths = self.paths.clone();
        let mut config = self
            .settings_draft
            .clone()
            .unwrap_or_else(|| self.config.clone());
        settings_card(
            ui,
            lapinux_core::i18n::t("graphics.frame_title"),
            lapinux_core::i18n::t("graphics.frame_hint"),
            |ui| {
                settings_row(
                    ui,
                    lapinux_core::i18n::t("graphics.default_engine"),
                    lapinux_core::i18n::t("graphics.engine_hint"),
                    |ui| {
                        egui::ComboBox::from_id_salt("graphics-engine")
                            .width(220.0)
                            .selected_text(frame_generation_label(config.graphics.frame_generation))
                            .show_ui(ui, |ui| {
                                for value in [FrameGeneration::Off, FrameGeneration::AfmfLinux] {
                                    ui.selectable_value(
                                        &mut config.graphics.frame_generation,
                                        value,
                                        frame_generation_label(value),
                                    );
                                }
                            });
                    },
                );
                muted(ui, lapinux_core::i18n::t("graphics.anticheat"));
                muted(ui, frame_generation_hint(config.graphics.frame_generation));
                if config.graphics.frame_generation == FrameGeneration::AfmfLinux {
                    ui.add_space(8.0);
                    let busy = self.jobs.iter().any(|job| {
                        job.kind == JobKind::InstallGraphics && job.state == JobState::Running
                    });
                    ui.horizontal(|ui| {
                        ui.label(lapinux_core::i18n::t("graphics.afmf_version"));
                        egui::ComboBox::from_id_salt("afmf-version")
                            .selected_text(&config.graphics.afmf_version)
                            .show_ui(ui, |ui| {
                                for version in lapinux_core::config::afmf_versions(local_afmf) {
                                    ui.selectable_value(
                                        &mut config.graphics.afmf_version,
                                        version.to_string(),
                                        if version == lapinux_core::config::AFMF_LOCAL_VERSION {
                                            lapinux_core::i18n::t("graphics.afmf_version_local")
                                        } else {
                                            version
                                        },
                                    );
                                }
                            });
                        // La version peut avoir changé pendant ce même affichage.
                        let afmf_dir = self
                            .paths
                            .tools_dir
                            .join("frame-generation/afmf-linux")
                            .join(&config.graphics.afmf_version);
                        let installed = afmf_dir.join("afmf-linux.json").is_file()
                            && afmf_dir.join("libafmf-linux.so").is_file();
                        let incomplete = afmf_dir.exists() && !installed;
                        if config.graphics.afmf_version == lapinux_core::config::AFMF_LOCAL_VERSION {
                            // La couche locale ne se télécharge pas : elle est
                            // construite et installée par
                            // `scripts/build-afmf-lapinux.sh`, qui écrit ses
                            // empreintes à côté d'elle.
                            ui.label(lapinux_core::i18n::t("graphics.afmf_local_built"));
                        } else if installed && !busy {
                            ui.label(lapinux_core::i18n::t("graphics.afmf_installed"));
                            let armed_id = ui
                                .id()
                                .with(("afmf-remove-armed", &config.graphics.afmf_version));
                            let armed = ui
                                .ctx()
                                .data(|d| d.get_temp::<bool>(armed_id).unwrap_or(false));
                            let label = if armed {
                                lapinux_core::i18n::t("graphics.afmf_confirm_remove")
                            } else {
                                lapinux_core::i18n::t("graphics.afmf_remove")
                            };
                            if ui.button(label).clicked() {
                                if armed {
                                    ui.ctx().data_mut(|d| d.remove::<bool>(armed_id));
                                    match self.client.request(&Req::RemoveFrameGeneration {
                                        version: config.graphics.afmf_version.clone(),
                                    }) {
                                        Ok(response) if response.ok => self
                                            .notify(lapinux_core::i18n::t("graphics.afmf_removed")),
                                        Ok(response) => {
                                            self.notify(response.error.unwrap_or_default())
                                        }
                                        Err(error) => self.notify(error.to_string()),
                                    }
                                } else {
                                    ui.ctx().data_mut(|d| d.insert_temp(armed_id, true));
                                }
                            }
                        } else if busy {
                            ui.label(lapinux_core::i18n::t("graphics.afmf_downloading"));
                        } else if incomplete {
                            ui.label(lapinux_core::i18n::t("graphics.afmf_incomplete"));
                        } else if ui
                            .button(lapinux_core::i18n::t("graphics.afmf_install"))
                            .clicked()
                        {
                            match self.client.request(&Req::InstallFrameGeneration {
                                version: config.graphics.afmf_version.clone(),
                            }) {
                                Ok(envelope) if envelope.ok => {
                                    self.notify(lapinux_core::i18n::t("graphics.afmf_started"))
                                }
                                Ok(envelope) => self.notify(envelope.error.unwrap_or_default()),
                                Err(error) => self.notify(error.to_string()),
                            }
                        }
                    });
                    use lapinux_core::config::{AfmfPerformanceMode, AfmfSearchMode};
                    settings_row(
                        ui,
                        lapinux_core::i18n::t("graphics.afmf_performance"),
                        "",
                        |ui| {
                            egui::ComboBox::from_id_salt("afmf-performance")
                                .width(180.0)
                                .selected_text(afmf_performance_label(
                                    config.graphics.afmf_performance_mode,
                                ))
                                .show_ui(ui, |ui| {
                                    for value in [
                                        AfmfPerformanceMode::Auto,
                                        AfmfPerformanceMode::Quality,
                                        AfmfPerformanceMode::Performance,
                                    ] {
                                        ui.selectable_value(
                                            &mut config.graphics.afmf_performance_mode,
                                            value,
                                            afmf_performance_label(value),
                                        );
                                    }
                                });
                        },
                    );
                    settings_row(
                        ui,
                        lapinux_core::i18n::t("graphics.afmf_search"),
                        "",
                        |ui| {
                            egui::ComboBox::from_id_salt("afmf-search")
                                .width(180.0)
                                .selected_text(afmf_search_label(config.graphics.afmf_search_mode))
                                .show_ui(ui, |ui| {
                                    for value in [
                                        AfmfSearchMode::Auto,
                                        AfmfSearchMode::Standard,
                                        AfmfSearchMode::High,
                                    ] {
                                        ui.selectable_value(
                                            &mut config.graphics.afmf_search_mode,
                                            value,
                                            afmf_search_label(value),
                                        );
                                    }
                                });
                        },
                    );
                    settings_row(
                        ui,
                        lapinux_core::i18n::t("graphics.afmf_multiplier"),
                        lapinux_core::i18n::t("graphics.afmf_multiplier_hint"),
                        |ui| {
                            egui::ComboBox::from_id_salt("afmf-multiplier")
                                .width(180.0)
                                .selected_text(afmf_multiplier_label(
                                    config.graphics.afmf_multiplier,
                                ))
                                .show_ui(ui, |ui| {
                                    for value in lapinux_core::config::AFMF_MULTIPLIERS {
                                        ui.selectable_value(
                                            &mut config.graphics.afmf_multiplier,
                                            *value,
                                            afmf_multiplier_label(*value),
                                        );
                                    }
                                });
                        },
                    );
                    muted(ui, lapinux_core::i18n::t("graphics.afmf_options_hint"));
                }
            },
        );
        settings_card(
            ui,
            lapinux_core::i18n::t("graphics.scaling_title"),
            lapinux_core::i18n::t("graphics.scaling_hint"),
            |ui| {
                settings_row(
                    ui,
                    lapinux_core::i18n::t("graphics.filter"),
                    lapinux_core::i18n::t("graphics.filter_hint"),
                    |ui| {
                        egui::ComboBox::from_id_salt("graphics-filter")
                            .width(220.0)
                            .selected_text(scaling_filter_label(config.graphics.scaling_filter))
                            .show_ui(ui, |ui| {
                                for value in [
                                    ScalingFilter::Auto,
                                    ScalingFilter::Fsr,
                                    ScalingFilter::Nis,
                                    ScalingFilter::Linear,
                                    ScalingFilter::Nearest,
                                    ScalingFilter::Pixel,
                                ] {
                                    ui.selectable_value(
                                        &mut config.graphics.scaling_filter,
                                        value,
                                        scaling_filter_label(value),
                                    );
                                }
                            });
                    },
                );
                settings_row(
                    ui,
                    lapinux_core::i18n::t("graphics.game_resolution"),
                    lapinux_core::i18n::t("graphics.resolution_hint"),
                    |ui| {
                        ui.add(
                            egui::DragValue::new(&mut config.graphics.game_width).range(0..=7680),
                        );
                        ui.label("×");
                        ui.add(
                            egui::DragValue::new(&mut config.graphics.game_height).range(0..=4320),
                        );
                    },
                );
                settings_row(
                    ui,
                    lapinux_core::i18n::t("graphics.sharpness"),
                    lapinux_core::i18n::t("graphics.sharpness_hint"),
                    |ui| {
                        settings_slider(ui, &mut config.graphics.sharpness, 0..=20, "");
                    },
                );
            },
        );
        settings_card(
            ui,
            lapinux_core::i18n::t("graphics.effects_title"),
            lapinux_core::i18n::t("graphics.effects_hint"),
            |ui| {
                settings_row(
                    ui,
                    lapinux_core::i18n::t("graphics.vkbasalt_enable"),
                    "",
                    |ui| {
                        let was_enabled = config.graphics.vkbasalt;
                        switch(ui, &mut config.graphics.vkbasalt);
                        if config.graphics.vkbasalt && !was_enabled {
                            match lapinux_core::vkbasalt::install(&runtime_paths) {
                                Ok(_) => self.notify(lapinux_core::i18n::t(
                                    "graphics.vkbasalt_runtime_installed_toast",
                                )),
                                Err(error) => self.notify(error),
                            }
                        }
                    },
                );
                muted(ui, lapinux_core::i18n::t("graphics.anticheat"));
                ui.add_space(10.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new(lapinux_core::i18n::t(
                            "graphics.vkbasalt_runtime_title",
                        ))
                        .strong(),
                    );
                    if vkbasalt_runtime.installed {
                        let arches = if vkbasalt_runtime.arches.is_empty() {
                            vkbasalt_runtime.arch.clone()
                        } else {
                            vkbasalt_runtime.arches.join(" + ")
                        };
                        ui.label(format!("{} · {arches}", vkbasalt_runtime.version));
                        ui.label(lapinux_core::i18n::t("graphics.vkbasalt_runtime_installed"));
                        let armed_id = ui.id().with("vkbasalt-runtime-remove-armed");
                        let armed = ui
                            .ctx()
                            .data(|data| data.get_temp::<bool>(armed_id).unwrap_or(false));
                        let label = if armed {
                            lapinux_core::i18n::t("graphics.vkbasalt_runtime_remove_confirm")
                        } else {
                            lapinux_core::i18n::t("graphics.vkbasalt_runtime_remove")
                        };
                        if ui.small_button(label).clicked() {
                            if armed {
                                ui.ctx().data_mut(|data| data.remove::<bool>(armed_id));
                                match lapinux_core::vkbasalt::remove(&runtime_paths) {
                                    Ok(()) => self.notify(lapinux_core::i18n::t(
                                        "graphics.vkbasalt_runtime_removed_toast",
                                    )),
                                    Err(error) => self.notify(error),
                                }
                            } else {
                                ui.ctx().data_mut(|data| data.insert_temp(armed_id, true));
                            }
                        }
                    } else if vkbasalt_runtime.disabled {
                        ui.label(lapinux_core::i18n::t("graphics.vkbasalt_runtime_disabled"));
                        if vkbasalt_runtime.system {
                            ui.label(lapinux_core::i18n::t("graphics.vkbasalt_runtime_system"));
                        }
                        if vkbasalt_runtime.bundled
                            && ui
                                .small_button(lapinux_core::i18n::t(
                                    "graphics.vkbasalt_runtime_install",
                                ))
                                .clicked()
                        {
                            match lapinux_core::vkbasalt::install(&runtime_paths) {
                                Ok(_) => self.notify(lapinux_core::i18n::t(
                                    "graphics.vkbasalt_runtime_installed_toast",
                                )),
                                Err(error) => self.notify(error),
                            }
                        }
                    } else if vkbasalt_runtime.bundled {
                        ui.label(lapinux_core::i18n::t("graphics.vkbasalt_runtime_missing"));
                        if ui
                            .small_button(lapinux_core::i18n::t(
                                "graphics.vkbasalt_runtime_install",
                            ))
                            .clicked()
                        {
                            match lapinux_core::vkbasalt::install(&runtime_paths) {
                                Ok(_) => self.notify(lapinux_core::i18n::t(
                                    "graphics.vkbasalt_runtime_installed_toast",
                                )),
                                Err(error) => self.notify(error),
                            }
                        }
                    } else if vkbasalt_runtime.system {
                        ui.label(lapinux_core::i18n::t("graphics.vkbasalt_runtime_system"));
                    } else {
                        ui.label(lapinux_core::i18n::t("graphics.vkbasalt_runtime_missing"));
                    }
                });
                muted(ui, lapinux_core::i18n::t("graphics.vkbasalt_runtime_hint"));
                settings_row(
                    ui,
                    lapinux_core::i18n::t("graphics.vkbasalt_effect"),
                    "",
                    |ui| {
                        egui::ComboBox::from_id_salt("graphics-vkbasalt-effect")
                            .width(220.0)
                            .selected_text(vkbasalt_effect_label(config.graphics.vkbasalt_effect))
                            .show_ui(ui, |ui| {
                                use lapinux_core::config::VkBasaltEffect;
                                for value in [
                                    VkBasaltEffect::Existing,
                                    VkBasaltEffect::Cas,
                                    VkBasaltEffect::Fxaa,
                                    VkBasaltEffect::Smaa,
                                    VkBasaltEffect::Ascii,
                                ] {
                                    ui.selectable_value(
                                        &mut config.graphics.vkbasalt_effect,
                                        value,
                                        vkbasalt_effect_label(value),
                                    );
                                }
                            });
                    },
                );
                muted(
                    ui,
                    format!(
                        "{} {}",
                        lapinux_core::i18n::t("graphics.vkbasalt_about_title"),
                        vkbasalt_effect_about(config.graphics.vkbasalt_effect)
                    ),
                );
                muted(ui, lapinux_core::i18n::t("graphics.vkbasalt_effect_hint"));
                settings_row(
                    ui,
                    lapinux_core::i18n::t("graphics.vkbasalt_config"),
                    "",
                    |ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut config.graphics.vkbasalt_config)
                                .desired_width(260.0),
                        );
                    },
                );
                muted(ui, lapinux_core::i18n::t("graphics.vkbasalt_hint"));
                ui.hyperlink_to("vkBasalt", "https://github.com/DadSchoorse/vkBasalt");
                ui.hyperlink_to("ReShade", "https://reshade.me/");
                muted(ui, lapinux_core::i18n::t("graphics.reshade_hint"));
            },
        );
        self.settings_draft = Some(config);
    }

    /// Onglet Captures : l'enregistreur, ce qu'il fait tout seul, et le dossier
    /// où arrivent les photos comme les vidéos.
    pub(super) fn settings_captures_tab(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        let mut config = self
            .settings_draft
            .clone()
            .unwrap_or_else(|| self.config.clone());

        settings_card(
            ui,
            lapinux_core::i18n::t("ui.settings_captures"),
            lapinux_core::i18n::t("settings.captures.hint_video"),
            |ui| {
                settings_row(
                    ui,
                    lapinux_core::i18n::t("settings.captures.record_on_launch"),
                    "",
                    |ui| {
                        switch(ui, &mut config.capture.record_on_launch);
                    },
                );
                muted(
                    ui,
                    lapinux_core::i18n::t("settings.captures.record_on_launch_hint"),
                );
                ui.add_space(12.0);
                settings_row(
                    ui,
                    lapinux_core::i18n::t("settings.captures.stop_on_close"),
                    "",
                    |ui| {
                        switch(ui, &mut config.capture.stop_on_exit);
                    },
                );
                muted(
                    ui,
                    lapinux_core::i18n::t("settings.captures.stop_on_close_hint"),
                );
                ui.add_space(14.0);
                settings_row(
                    ui,
                    lapinux_core::i18n::t("settings.captures.profile"),
                    "",
                    |ui| {
                        egui::ComboBox::from_id_salt("settings-capture-profile")
                            .width(220.0)
                            .selected_text(profile_label(&config.capture.video_profile))
                            .show_ui(ui, |ui| {
                                for (value, label) in [
                                    (
                                        "performance",
                                        lapinux_core::i18n::t(
                                            "settings.captures.profile_performance",
                                        ),
                                    ),
                                    (
                                        "balanced",
                                        lapinux_core::i18n::t("settings.captures.balanced"),
                                    ),
                                    (
                                        "quality",
                                        lapinux_core::i18n::t("settings.captures.quality"),
                                    ),
                                ] {
                                    ui.selectable_value(
                                        &mut config.capture.video_profile,
                                        value.to_string(),
                                        label,
                                    );
                                }
                            });
                    },
                );
                settings_row(
                    ui,
                    lapinux_core::i18n::t("settings.captures.resolution"),
                    "",
                    |ui| {
                        egui::ComboBox::from_id_salt("settings-capture-resolution")
                            .width(220.0)
                            .selected_text(resolution_label(&config.capture.video_resolution))
                            .show_ui(ui, |ui| {
                                for (value, label) in [
                                    ("720p", lapinux_core::i18n::t("settings.captures.720p")),
                                    ("1080p", lapinux_core::i18n::t("settings.captures.1080p")),
                                    ("1440p", lapinux_core::i18n::t("settings.captures.1440p")),
                                    ("native", lapinux_core::i18n::t("settings.captures.native")),
                                ] {
                                    ui.selectable_value(
                                        &mut config.capture.video_resolution,
                                        value.to_string(),
                                        label,
                                    );
                                }
                            });
                    },
                );
                settings_row(ui, lapinux_core::i18n::t("ui.capture_fps"), "", |ui| {
                    settings_slider(ui, &mut config.capture.video_fps, 15..=240, " i/s");
                });
                settings_row(ui, lapinux_core::i18n::t("ui.capture_encoder"), "", |ui| {
                    egui::ComboBox::from_id_salt("settings-capture-encoder")
                        .width(220.0)
                        .selected_text(encoder_label(&config.capture.video_encoder))
                        .show_ui(ui, |ui| {
                            for (value, label) in [
                                (
                                    "auto",
                                    lapinux_core::i18n::t("settings.captures.encoder_auto"),
                                ),
                                (
                                    "nvenc",
                                    lapinux_core::i18n::t("settings.captures.encoder_nvenc"),
                                ),
                                (
                                    "software",
                                    lapinux_core::i18n::t("settings.captures.encoder_software"),
                                ),
                            ] {
                                ui.selectable_value(
                                    &mut config.capture.video_encoder,
                                    value.to_string(),
                                    label,
                                );
                            }
                        });
                });
                settings_row(ui, lapinux_core::i18n::t("ui.capture_codec"), "", |ui| {
                    egui::ComboBox::from_id_salt("settings-capture-codec")
                        .width(220.0)
                        .selected_text(codec_label(&config.capture.video_codec))
                        .show_ui(ui, |ui| {
                            for (value, label) in [
                                (
                                    "auto",
                                    lapinux_core::i18n::t("settings.captures.codec_auto"),
                                ),
                                ("h264", "H.264"),
                                ("hevc", "H.265 · HEVC"),
                                ("av1", "AV1"),
                            ] {
                                ui.selectable_value(
                                    &mut config.capture.video_codec,
                                    value.to_string(),
                                    label,
                                );
                            }
                        });
                });
                ui.add_space(8.0);
                muted(ui, lapinux_core::i18n::t("ui.capture_performance_hint"));
                ui.add_space(4.0);
                muted(ui, lapinux_core::i18n::t("ui.capture_fps_hint"));
                ui.add_space(10.0);
                settings_row(
                    ui,
                    lapinux_core::i18n::t("settings.captures.record_game_audio"),
                    "",
                    |ui| {
                        switch(ui, &mut config.capture.record_audio);
                    },
                );
                muted(
                    ui,
                    lapinux_core::i18n::t("settings.captures.game_audio_only"),
                );
                settings_row(
                    ui,
                    lapinux_core::i18n::t("settings.captures.record_microphone"),
                    "",
                    |ui| {
                        switch(ui, &mut config.capture.record_microphone);
                    },
                );
                muted(ui, lapinux_core::i18n::t("settings.captures.no_room_audio"));
                ui.add_space(6.0);
            },
        );

        ui.add_space(14.0);
        super::settings_hud::hud_settings_card(ui, &mut config);
        ui.add_space(2.0);
        super::settings_hud::record_hud_settings_card(ui, &mut config);
        self.settings_draft = Some(config);
    }

    /// Onglet Application : comment Lapinux est fabriqué. Les chemins, les
    /// tailles de cache et le rapport de diagnostic vivent ailleurs (Stockage
    /// et Journal) : cette page ne garde que l'architecture.
    pub(super) fn settings_application_tab(&mut self, ui: &mut egui::Ui) {
        ui.label(
            egui::RichText::new(lapinux_core::i18n::t_args(
                "settings.application.subtitle",
                &[("version", env!("CARGO_PKG_VERSION"))],
            ))
            .size(11.0)
            .color(TEXT_DIM()),
        );
        ui.add_space(10.0);

        settings_card(
            ui,
            lapinux_core::i18n::t("settings.application.layout"),
            lapinux_core::i18n::t("settings.application.layout_desc"),
            |ui| {
                for (name, role) in [
                    (
                        "lapinux",
                        lapinux_core::i18n::t("settings.application.crate_main"),
                    ),
                    (
                        "lapinux-core",
                        lapinux_core::i18n::t("settings.application.crate_core"),
                    ),
                    (
                        "lapinux-steam",
                        lapinux_core::i18n::t("settings.application.crate_steam"),
                    ),
                    (
                        "lapinux-daemon",
                        lapinux_core::i18n::t("settings.application.crate_daemon"),
                    ),
                    (
                        "lapinux-ui",
                        lapinux_core::i18n::t("settings.application.crate_ui"),
                    ),
                    (
                        "lapinux-input",
                        lapinux_core::i18n::t("settings.application.input_crate"),
                    ),
                    (
                        "lapinux-overlay",
                        lapinux_core::i18n::t("settings.application.crate_overlay"),
                    ),
                    (
                        "lapinux-proton",
                        lapinux_core::i18n::t("settings.application.crate_proton"),
                    ),
                    (
                        "lapinux-perf",
                        lapinux_core::i18n::t("settings.application.crate_perf"),
                    ),
                ] {
                    ui.horizontal_top(|ui| {
                        ui.add_sized(
                            [138.0, 18.0],
                            egui::Label::new(
                                egui::RichText::new(name)
                                    .monospace()
                                    .size(10.5)
                                    .color(TEXT()),
                            ),
                        );
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(role).size(10.5).color(TEXT_DIM()),
                            )
                            .wrap(),
                        );
                    });
                }
                ui.add_space(6.0);
                muted(
                    ui,
                    lapinux_core::i18n::t("settings.application.runtime_desc"),
                );
            },
        );

        settings_card(
            ui,
            lapinux_core::i18n::t("settings.application.flow"),
            lapinux_core::i18n::t("settings.application.flow_desc"),
            |ui| {
                for (name, role) in [
                    (
                        lapinux_core::i18n::t("settings.application.flow_click"),
                        lapinux_core::i18n::t("settings.application.flow_click_desc"),
                    ),
                    (
                        lapinux_core::i18n::t("settings.application.flow_daemon"),
                        lapinux_core::i18n::t("settings.application.flow_daemon_desc"),
                    ),
                    (
                        lapinux_core::i18n::t("settings.application.flow_reply"),
                        lapinux_core::i18n::t("settings.application.flow_reply_desc"),
                    ),
                    (
                        lapinux_core::i18n::t("settings.application.flow_game"),
                        lapinux_core::i18n::t("settings.application.flow_game_desc"),
                    ),
                ] {
                    ui.horizontal_top(|ui| {
                        ui.add_sized(
                            [138.0, 18.0],
                            egui::Label::new(
                                egui::RichText::new(name).size(10.5).strong().color(TEXT()),
                            ),
                        );
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(role).size(10.5).color(TEXT_DIM()),
                            )
                            .wrap(),
                        );
                    });
                }
            },
        );

        settings_card(
            ui,
            lapinux_core::i18n::t("settings.application.stack"),
            lapinux_core::i18n::t("settings.application.stack_desc"),
            |ui| {
                for (name, role) in [
                    (
                        lapinux_core::i18n::t("settings.application.stack_language"),
                        lapinux_core::i18n::t("settings.application.stack_language_desc"),
                    ),
                    (
                        lapinux_core::i18n::t("settings.application.stack_ui"),
                        lapinux_core::i18n::t("settings.application.stack_ui_desc"),
                    ),
                    (
                        lapinux_core::i18n::t("settings.application.stack_data"),
                        lapinux_core::i18n::t("settings.application.stack_data_desc"),
                    ),
                    (
                        lapinux_core::i18n::t("settings.application.stack_hardware"),
                        lapinux_core::i18n::t("settings.application.stack_hardware_desc"),
                    ),
                ] {
                    ui.horizontal_top(|ui| {
                        ui.add_sized(
                            [138.0, 18.0],
                            egui::Label::new(
                                egui::RichText::new(name).size(10.5).strong().color(TEXT()),
                            ),
                        );
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(role).size(10.5).color(TEXT_DIM()),
                            )
                            .wrap(),
                        );
                    });
                }
            },
        );
    }

    /// Poids réel des caches, mesuré en tâche de fond, et de quoi les vider.
    /// La carte vit dans Stockage : l'onglet Application ne décrit plus que
    /// l'architecture.
    pub(super) fn settings_cache_card(&mut self, ui: &mut egui::Ui) {
        self.cache_usage.ensure(ui.ctx(), &self.paths);
        self.cache_usage.poll();
        let rows = self.cache_usage.rows.clone();
        let total = self.cache_usage.total;
        let note = self.cache_usage.note.clone();
        let working = self.cache_usage.working();
        let mut clear = false;
        settings_card(
            ui,
            lapinux_core::i18n::t("settings.application.caches"),
            lapinux_core::i18n::t("settings.application.cache_desc"),
            |ui| {
                let files: usize = rows.iter().map(|(_, _, files)| *files).sum();
                application_value_row(
                    ui,
                    lapinux_core::i18n::t("settings.application.total"),
                    &lapinux_core::i18n::t_args(
                        "settings.application.total_value",
                        &[
                            ("size", &lapinux_core::util::format_bytes(total)),
                            ("files", &files.to_string()),
                        ],
                    ),
                );
                ui.add_space(2.0);
                for (name, bytes, count) in &rows {
                    if *count == 0 {
                        continue;
                    }
                    application_value_row(
                        ui,
                        name,
                        &lapinux_core::i18n::t_args(
                            "settings.application.cache_row",
                            &[
                                ("size", &lapinux_core::util::format_bytes(*bytes)),
                                ("count", &count.to_string()),
                            ],
                        ),
                    );
                }
                if let Some(note) = &note {
                    ui.add_space(4.0);
                    muted(ui, note);
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            !working,
                            egui::Button::new(lapinux_core::i18n::t(
                                "settings.application.clear_caches",
                            )),
                        )
                        .clicked()
                    {
                        clear = true;
                    }
                    if working {
                        crate::section_icons::loading(ui, 13.0);
                    }
                    if ui
                        .button(lapinux_core::i18n::t("settings.application.open_folder"))
                        .clicked()
                    {
                        open_path(&self.paths.cache_dir);
                    }
                });
            },
        );
        if clear {
            self.clear_caches(ui.ctx());
        }
    }
    /// Vide les caches : le daemon efface les fichiers et oublie ses lectures,
    /// l'interface jette ses textures. Configuration, journaux et jeux restent.
    fn clear_caches(&mut self, ctx: &egui::Context) {
        if self.cache_usage.clearing.is_some() {
            return;
        }
        self.covers.clear();
        self.library_covers_warmed = false;
        self.library_fill_until = None;
        self.art.clear();
        self.gameinfo.forget();
        let (tx, rx) = mpsc::channel();
        self.cache_usage.clearing = Some(rx);
        self.cache_usage.note = None;
        let client = self.client.detached();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = client
                .request(&Req::ClearCaches)
                .map_err(|error| error.to_string())
                .and_then(|envelope| {
                    if !envelope.ok {
                        return Err(envelope.error.unwrap_or_else(|| {
                            lapinux_core::i18n::t("settings.storage.unknown_error").into()
                        }));
                    }
                    envelope
                        .data
                        .get("freed")
                        .and_then(|value| value.as_u64())
                        .ok_or_else(|| {
                            lapinux_core::i18n::t("settings.captures.daemon_invalid").into()
                        })
                });
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }

    /// Onglet Mises à jour : version installée datée, dernière release publiée
    /// sur GitHub et téléchargement suivi de l'AppImage.
    pub(super) fn settings_updates_tab(&mut self, ui: &mut egui::Ui) {
        use lapinux_core::update::UpdateStage;

        if !self.update_ui.attempted && self.update_ui.status.is_none() && self.update_ui.rx.is_none() {
            self.start_update_check();
        }
        if let Some(rx) = self.update_ui.rx.take() {
            match rx.try_recv() {
                Ok(Ok(status)) => self.update_ui.status = Some(status),
                Ok(Err(error)) => {
                    self.update_ui.message = lapinux_core::i18n::t_args(
                        "settings.updates.failed",
                        &[("error", error.as_str())],
                    );
                }
                Err(mpsc::TryRecvError::Empty) => self.update_ui.rx = Some(rx),
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.update_ui.message =
                        lapinux_core::i18n::t("settings.updates.daemon_unreachable").to_string();
                }
            }
        }

        let checking = self.update_ui.rx.is_some();
        let downloading = self.update_ui.progress.as_ref().is_some_and(|progress| {
            matches!(
                progress.stage,
                UpdateStage::Downloading | UpdateStage::Verifying
            )
        });
        let running = checking || downloading;
        if running {
            let due = self
                .update_ui
                .poll
                .map_or(true, |last| last.elapsed() >= Duration::from_millis(400));
            if due {
                self.update_ui.poll = Some(Instant::now());
                self.refresh_update_state();
            }
            ui.ctx().request_repaint_after(Duration::from_millis(200));
        }

        let status = self.update_ui.status.clone();
        let current = status
            .as_ref()
            .map(|status| status.current.clone())
            .unwrap_or_else(|| lapinux_core::VERSION.to_string());
        let current_date = status
            .as_ref()
            .map(|status| status.current_date.clone())
            .filter(|date| !date.is_empty())
            .unwrap_or_else(|| lapinux_core::update::BUILD_DATE.to_string());

        settings_card(
            ui,
            lapinux_core::i18n::t("settings.updates.current"),
            lapinux_core::i18n::t("settings.updates.current_desc"),
            |ui| {
                update_value_row(ui, lapinux_core::i18n::t("settings.updates.version"), &current);
                update_value_row(ui, lapinux_core::i18n::t("settings.updates.built"), &current_date);
                update_value_row(
                    ui,
                    lapinux_core::i18n::t("settings.updates.repository"),
                    &lapinux_core::update::repository(),
                );
            },
        );

        settings_card(
            ui,
            lapinux_core::i18n::t("settings.updates.latest"),
            lapinux_core::i18n::t("settings.updates.latest_desc"),
            |ui| match status.as_ref().and_then(|status| status.latest.as_ref()) {
                Some(release) => {
                    update_value_row(ui, lapinux_core::i18n::t("settings.updates.version"), &release.version);
                    update_value_row(
                        ui,
                        lapinux_core::i18n::t("settings.updates.published"),
                        &release.published,
                    );
                    if let Some(asset) = &release.asset {
                        update_value_row(ui, lapinux_core::i18n::t("settings.updates.asset"), &asset.name);
                    }
                    ui.add_space(6.0);
                    if status.as_ref().is_some_and(|status| status.available) {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(lapinux_core::i18n::t_args(
                                    "settings.updates.available",
                                    &[("version", release.version.as_str())],
                                ))
                                .size(11.0)
                                .color(TEXT()),
                            )
                            .wrap(),
                        );
                    } else {
                        muted(ui, lapinux_core::i18n::t("settings.updates.up_to_date"));
                    }
                    if !release.notes.is_empty() {
                        ui.add_space(6.0);
                        muted(ui, lapinux_core::i18n::t("settings.updates.notes"));
                        egui::ScrollArea::vertical()
                            .id_salt("update-notes")
                            .max_height(140.0)
                            .show(ui, |ui| {
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(&release.notes)
                                            .size(10.5)
                                            .color(TEXT_DIM()),
                                    )
                                    .wrap(),
                                );
                            });
                    }
                }
                None => muted(ui, lapinux_core::i18n::t("settings.updates.no_release")),
            },
        );

        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !running,
                    egui::Button::new(lapinux_core::i18n::t("settings.updates.check")),
                )
                .clicked()
            {
                self.start_update_check();
            }
            let downloadable = !running
                && status.as_ref().is_some_and(|status| status.available)
                && status
                    .as_ref()
                    .and_then(|status| status.latest.as_ref())
                    .and_then(|release| release.asset.as_ref())
                    .is_some();
            if ui
                .add_enabled(
                    downloadable,
                    egui::Button::new(lapinux_core::i18n::t("settings.updates.download")),
                )
                .clicked()
            {
                if let Some(release) = status.as_ref().and_then(|status| status.latest.as_ref()) {
                    self.start_update_download(release);
                }
            }
        });

        if checking {
            ui.add_space(6.0);
            muted(ui, lapinux_core::i18n::t("settings.updates.checking"));
        }

        if let Some(progress) = self.update_ui.progress.clone() {
            ui.add_space(10.0);
            match progress.stage {
                UpdateStage::Downloading | UpdateStage::Verifying => {
                    ui.add(
                        egui::ProgressBar::new(progress.fraction())
                            .desired_height(6.0)
                            .desired_width(ui.available_width())
                            .text(format!(
                                "{} %",
                                (progress.fraction() * 100.0).round() as u32
                            )),
                    );
                    let detail = if progress.stage == UpdateStage::Verifying {
                        lapinux_core::i18n::t("settings.updates.verifying").to_string()
                    } else {
                        let received = update_size(progress.received);
                        let total = update_size(progress.total);
                        lapinux_core::i18n::t_args(
                            "settings.updates.downloading",
                            &[("received", received.as_str()), ("total", total.as_str())],
                        )
                    };
                    muted(ui, detail);
                }
                UpdateStage::Ready => {
                    // Un fichier téléchargé pour une autre release ne doit pas
                    // être présenté comme la mise à jour courante.
                    let latest = status
                        .as_ref()
                        .and_then(|status| status.latest.as_ref())
                        .map(|release| release.tag.as_str());
                    if latest.map_or(true, |tag| tag == progress.tag)
                        && ui
                            .button(lapinux_core::i18n::t("settings.updates.install"))
                            .clicked()
                    {
                        self.install_update();
                    }
                }
                UpdateStage::Failed => muted(ui, &progress.message),
            }
        }

        if self.update_ui.installed {
            ui.add_space(8.0);
            if ui
                .button(lapinux_core::i18n::t("settings.updates.restart"))
                .clicked()
            {
                self.restart_after_update();
            }
        }
        if !self.update_ui.message.is_empty() {
            ui.add_space(8.0);
            muted(ui, &self.update_ui.message);
        }
    }

    fn start_update_check(&mut self) {
        let (tx, rx) = mpsc::channel();
        let client = self.client.detached();
        std::thread::spawn(move || {
            let result = client
                .request_with_timeout(&Req::UpdateCheck, Duration::from_secs(25))
                .map_err(|error| error.to_string())
                .and_then(|envelope| {
                    envelope
                        .into_data::<lapinux_core::update::UpdateStatus>()
                        .map_err(|error| error.to_string())
                });
            let _ = tx.send(result);
        });
        self.update_ui.rx = Some(rx);
        self.update_ui.attempted = true;
        self.update_ui.message.clear();
        self.update_ui.installed = false;
    }

    fn refresh_update_state(&mut self) {
        if let Some(progress) = ui_request::<Option<lapinux_core::update::UpdateProgress>>(
            &self.client,
            &Req::UpdateState,
        ) {
            self.update_ui.progress = progress;
        }
    }

    fn start_update_download(&mut self, release: &lapinux_core::update::Release) {
        let Some(asset) = release.asset.clone() else {
            return;
        };
        let sha256 = asset.sha256().map(|digest| digest.to_string());
        let request = Req::UpdateDownload {
            tag: release.tag.clone(),
            name: asset.name,
            url: asset.url,
            size: asset.size,
            sha256,
        };
        match self.client.request(&request) {
            Ok(_) => {
                self.update_ui.message.clear();
                self.update_ui.installed = false;
                self.update_ui.poll = None;
                self.refresh_update_state();
            }
            Err(error) => self.update_ui.message = update_failure(&error.to_string()),
        }
    }

    fn install_update(&mut self) {
        match self.client.request(&Req::UpdateInstall) {
            Ok(envelope) => match envelope.into_data::<String>() {
                Ok(tag) => {
                    let version = lapinux_core::update::version_of(&tag);
                    self.update_ui.installed = true;
                    self.update_ui.progress = None;
                    self.update_ui.message = lapinux_core::i18n::t_args(
                        "settings.updates.installed",
                        &[("version", version.as_str())],
                    );
                }
                Err(error) => self.update_ui.message = update_failure(&error.to_string()),
            },
            Err(error) => self.update_ui.message = update_failure(&error.to_string()),
        }
    }

    /// Le nouveau binaire prend la main : le daemon est arrêté proprement pour
    /// repartir sur la version installée au prochain lancement.
    fn restart_after_update(&mut self) {
        let _ = self.client.request(&Req::ShutdownIfIdle);
        // Dans une AppImage, `current_exe` pointe le binaire monté, qui
        // disparaît à la sortie : c'est l'AppImage elle-même qu'on relance.
        let executable = std::env::var("APPIMAGE")
            .ok()
            .filter(|path| std::path::Path::new(path).is_file())
            .or_else(|| {
                std::env::current_exe()
                    .ok()
                    .map(|path| path.display().to_string())
            });
        if let Some(executable) = executable {
            if std::process::Command::new(executable).spawn().is_ok() {
                std::process::exit(0);
            }
        }
    }

    /// Onglet Bibliothèque : racine, taille des vignettes et contenus masqués.
    /// Onglet Stockage : les jeux installés, du plus lourd au plus léger, et
    /// l'espace du disque qui les porte. Le tri est mis en cache : la page ne
    /// reparcourt la bibliothèque qu'après un scan, pas à chaque image.
    pub(super) fn settings_storage_tab(&mut self, ui: &mut egui::Ui) {
        muted(ui, lapinux_core::i18n::t("settings.storage.declared_size"));
        self.settings_cache_card(ui);
        ui.add_space(14.0);
        let library_root = self.config.library_root();
        self.storage.ensure(&self.games, &library_root);
        let cache = &self.storage;

        egui::Frame::none()
            .fill(CARD())
            .stroke(egui::Stroke::new(1.0_f32, BORDER()))
            .rounding(egui::Rounding::same(14.0))
            .inner_margin(18.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new(lapinux_core::i18n::t("storage.games"))
                                .size(11.0)
                                .color(TEXT_DIM()),
                        );
                        ui.add_space(2.0);
                        ui.label(
                            egui::RichText::new(lapinux_core::util::format_bytes(cache.total))
                                .size(30.0)
                                .strong()
                                .color(TEXT()),
                        );
                        ui.add_space(1.0);
                        ui.label(
                            egui::RichText::new(lapinux_core::i18n::t_args(
                                "storage.counts",
                                &[
                                    ("installed", &cache.installed.to_string()),
                                    ("unknown", &cache.unknown.to_string()),
                                ],
                            ))
                            .size(10.5)
                            .color(TEXT_DIM()),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                        ui.vertical(|ui| match &cache.disk {
                            Some(disk) => {
                                ui.label(
                                    egui::RichText::new(lapinux_core::i18n::t("storage.disk"))
                                        .size(11.0)
                                        .color(TEXT_DIM()),
                                );
                                ui.label(
                                    egui::RichText::new(lapinux_core::i18n::t_args(
                                        "storage.free",
                                        &[
                                            ("free", &lapinux_core::util::format_bytes(disk.free)),
                                            (
                                                "total",
                                                &lapinux_core::util::format_bytes(disk.total),
                                            ),
                                        ],
                                    ))
                                    .size(12.0)
                                    .strong()
                                    .color(TEXT()),
                                );
                                ui.add_space(7.0);
                                let width = 240.0_f32.min(ui.available_width());
                                let (bar, _) = ui.allocate_exact_size(
                                    egui::vec2(width, 6.0),
                                    egui::Sense::hover(),
                                );
                                ui.painter().rect_filled(
                                    bar,
                                    egui::Rounding::same(3.0),
                                    with_alpha(BORDER(), 0.55),
                                );
                                let used = egui::Rect::from_min_size(
                                    bar.min,
                                    egui::vec2(
                                        bar.width() * disk.used_percent() / 100.0,
                                        bar.height(),
                                    ),
                                );
                                ui.painter()
                                    .rect_filled(used, egui::Rounding::same(3.0), ACCENT());
                                ui.add_space(2.0);
                                ui.label(
                                    egui::RichText::new(lapinux_core::i18n::t_args(
                                        "storage.used",
                                        &[("percent", &format!("{:.0}", disk.used_percent()))],
                                    ))
                                    .size(9.5)
                                    .color(TEXT_DIM()),
                                );
                            }
                            None => muted(
                                ui,
                                lapinux_core::i18n::t("settings.storage.disk_unavailable"),
                            ),
                        });
                    });
                });
                ui.add_space(10.0);
                muted(ui, lapinux_core::i18n::t("settings.storage.not_counted"));
            });

        ui.add_space(14.0);
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(lapinux_core::i18n::t("storage.by_size"))
                    .size(13.0)
                    .strong()
                    .color(TEXT()),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                muted(
                    ui,
                    lapinux_core::i18n::t_args(
                        "storage.installed_only",
                        &[("count", &cache.rows.len().to_string())],
                    ),
                );
            });
        });
        ui.add_space(6.0);

        if cache.rows.is_empty() {
            egui::Frame::none()
                .fill(CARD())
                .stroke(egui::Stroke::new(1.0_f32, BORDER()))
                .rounding(egui::Rounding::same(12.0))
                .inner_margin(16.0)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(
                        egui::RichText::new(lapinux_core::i18n::t("settings.storage.no_games"))
                            .size(13.0)
                            .strong()
                            .color(TEXT()),
                    );
                    ui.add_space(2.0);
                    muted(ui, lapinux_core::i18n::t("settings.storage.no_games_hint"));
                });
            return;
        }

        let max_size = cache
            .rows
            .iter()
            .map(|row| row.size)
            .max()
            .unwrap_or(1)
            .max(1);
        let list_height = ui.available_height().clamp(220.0, 520.0);
        egui::ScrollArea::vertical()
            .id_salt("settings-storage-games")
            .max_height(list_height)
            .auto_shrink([false, false])
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
            .show_rows(ui, 48.0, cache.rows.len(), |ui, range| {
                let width = ui.available_width();
                for index in range {
                    let row = &cache.rows[index];
                    let (rect, response) =
                        ui.allocate_exact_size(egui::vec2(width, 48.0), egui::Sense::hover());
                    let card = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), 44.0));
                    let painter = ui.painter().with_clip_rect(card);
                    painter.rect_filled(
                        card,
                        egui::Rounding::same(9.0),
                        if response.hovered() {
                            CARD_HOVER()
                        } else {
                            CARD()
                        },
                    );
                    if row.size > 0 {
                        let ratio = (row.size as f32 / max_size as f32).clamp(0.0, 1.0);
                        let fill = egui::Rect::from_min_size(
                            card.min,
                            egui::vec2((card.width() * ratio).max(3.0), card.height()),
                        );
                        painter.rect_filled(
                            fill,
                            egui::Rounding::same(9.0),
                            with_alpha(ACCENT(), 0.055),
                        );
                    }
                    painter.text(
                        egui::pos2(card.left() + 14.0, card.center().y),
                        egui::Align2::LEFT_CENTER,
                        format!("{:02}", index + 1),
                        egui::FontId::monospace(10.0),
                        if index < 3 { ACCENT() } else { TEXT_DIM() },
                    );
                    let text_rect = egui::Rect::from_min_max(
                        egui::pos2(card.left() + 48.0, card.top()),
                        egui::pos2(card.right() - 104.0, card.bottom()),
                    );
                    let text = ui.painter().with_clip_rect(text_rect);
                    text.text(
                        egui::pos2(text_rect.left(), card.center().y - 6.0),
                        egui::Align2::LEFT_CENTER,
                        &row.name,
                        egui::FontId::proportional(12.5),
                        TEXT(),
                    );
                    let subtitle = if row.installed {
                        row.store.to_string()
                    } else {
                        lapinux_core::i18n::t_fmt(
                            "settings.storage.not_installed",
                            &[row.store.to_string()],
                        )
                    };
                    text.text(
                        egui::pos2(text_rect.left(), card.center().y + 8.0),
                        egui::Align2::LEFT_CENTER,
                        subtitle,
                        egui::FontId::proportional(9.5),
                        TEXT_DIM(),
                    );
                    let size = if row.size > 0 {
                        lapinux_core::util::format_bytes(row.size)
                    } else {
                        "—".to_string()
                    };
                    painter.text(
                        egui::pos2(card.right() - 14.0, card.center().y),
                        egui::Align2::RIGHT_CENTER,
                        size,
                        egui::FontId::proportional(12.5),
                        if row.size > 0 { TEXT() } else { TEXT_DIM() },
                    );
                    if cache.total > 0 && row.size > 0 {
                        painter.text(
                            egui::pos2(card.right() - 14.0, card.center().y + 12.0),
                            egui::Align2::RIGHT_CENTER,
                            format!("{:.1} %", row.size as f64 / cache.total as f64 * 100.0),
                            egui::FontId::proportional(9.0),
                            TEXT_DIM(),
                        );
                    }
                }
            });
        ui.add_space(8.0);
        muted(ui, lapinux_core::i18n::t("settings.storage.caches_hint"));
    }

    pub(super) fn settings_library_tab(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        let mut config = self
            .settings_draft
            .clone()
            .unwrap_or_else(|| self.config.clone());
        let syncing = self
            .catalog
            .as_ref()
            .map(|catalog| catalog.syncing)
            .unwrap_or(false);
        let connected = self
            .steam_status
            .as_ref()
            .map(|status| status.connected)
            .unwrap_or(false);

        egui::Frame::none()
            .fill(CARD())
            .stroke(egui::Stroke::new(1.0_f32, BORDER()))
            .rounding(egui::Rounding::same(12.0))
            .inner_margin(16.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    egui::RichText::new(lapinux_core::i18n::t("settings.library.display"))
                        .size(13.0)
                        .strong()
                        .color(TEXT()),
                );
                ui.add_space(3.0);
                muted(ui, lapinux_core::i18n::t("settings.library.display_hint"));
                ui.add_space(12.0);
                egui::Grid::new("library-settings")
                    .num_columns(2)
                    .spacing([18.0, 10.0])
                    .show(ui, |ui| {
                        ui.label(lapinux_core::i18n::t("settings.library.root"));
                        ui.add(
                            egui::TextEdit::singleline(&mut config.general.library_root)
                                .desired_width(380.0),
                        );
                        ui.end_row();

                        ui.label(lapinux_core::i18n::t("settings.library.tile_size"));
                        settings_slider(ui, &mut config.general.grid_width, 72..=240, " px");
                        ui.end_row();

                        ui.label(lapinux_core::i18n::t("settings.library.cover_radius"));
                        settings_slider(ui, &mut config.library.cover_radius, 0..=32, " px");
                        ui.end_row();

                        ui.label(lapinux_core::i18n::t("settings.library.spacing"));
                        settings_slider(ui, &mut config.library.grid_spacing, 4..=40, " px");
                        ui.end_row();

                        ui.label(lapinux_core::i18n::t("settings.library.hover_size"));
                        settings_slider(ui, &mut config.library.hover_width, 220..=420, " px");
                        ui.end_row();
                    });
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    switch(ui, &mut config.library.show_badges);
                    ui.label(lapinux_core::i18n::t("settings.library.badges"));
                });
                if config.library.show_badges {
                    ui.add_space(10.0);
                    egui::Grid::new("library-badges")
                        .num_columns(2)
                        .spacing([18.0, 10.0])
                        .show(ui, |ui| {
                            ui.label(lapinux_core::i18n::t("settings.library.badge_size"));
                            settings_slider(ui, &mut config.library.badge_scale, 60..=200, " %");
                            ui.end_row();
                        });
                }
                ui.add_space(3.0);
                muted(ui, lapinux_core::i18n::t("settings.library.badges_hint"));
            });

        ui.add_space(14.0);
        egui::Frame::none()
            .fill(CARD())
            .stroke(egui::Stroke::new(1.0_f32, BORDER()))
            .rounding(egui::Rounding::same(12.0))
            .inner_margin(16.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    egui::RichText::new(lapinux_core::i18n::t("settings.library.hidden_title"))
                        .size(13.0)
                        .strong()
                        .color(TEXT()),
                );
                ui.add_space(3.0);
                muted(ui, lapinux_core::i18n::t("settings.library.hidden_hint"));
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    switch(ui, &mut config.library.hide_tools);
                    ui.label(lapinux_core::i18n::t("settings.library.hide_tools"));
                });
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    switch(ui, &mut config.library.show_hidden);
                    ui.label(lapinux_core::i18n::t("settings.library.show_hidden"));
                });
                if !config.library.hidden.is_empty() {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        muted(
                            ui,
                            &format!(
                                "{} {}",
                                config.library.hidden.len(),
                                lapinux_core::i18n::t("settings.library.hidden_count")
                            ),
                        );
                        if ui
                            .button(lapinux_core::i18n::t("settings.library.unhide_all"))
                            .clicked()
                        {
                            config.library.hidden.clear();
                        }
                    });
                }
                ui.add_space(4.0);
                muted(
                    ui,
                    lapinux_core::i18n::t("settings.library.hide_tools_hint"),
                );
            });

        ui.add_space(14.0);
        egui::Frame::none()
            .fill(CARD())
            .stroke(egui::Stroke::new(1.0_f32, BORDER()))
            .rounding(egui::Rounding::same(12.0))
            .inner_margin(16.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    egui::RichText::new(lapinux_core::i18n::t("settings.library.demos_title"))
                        .size(13.0)
                        .strong()
                        .color(TEXT()),
                );
                ui.add_space(3.0);
                muted(ui, lapinux_core::i18n::t("settings.library.demos_hint"));
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(lapinux_core::i18n::t("settings.library.demos"))
                            .size(12.0)
                            .color(TEXT()),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        switch(ui, &mut config.library.show_demos);
                    });
                });
                ui.add_space(4.0);
                muted(ui, lapinux_core::i18n::t("settings.library.demos_hint2"));
            });

        ui.add_space(14.0);
        egui::Frame::none()
            .fill(CARD())
            .stroke(egui::Stroke::new(1.0_f32, BORDER()))
            .rounding(egui::Rounding::same(12.0))
            .inner_margin(16.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    egui::RichText::new(lapinux_core::i18n::t("settings.library.sync_title"))
                        .size(13.0)
                        .strong()
                        .color(TEXT()),
                );
                ui.add_space(3.0);
                muted(ui, lapinux_core::i18n::t("settings.library.sync_hint"));
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui
                        .button(lapinux_core::i18n::t("settings.library.resync"))
                        .clicked()
                    {
                        self.refresh(true);
                        self.notify(lapinux_core::i18n::t("settings.library.scanned"));
                    }
                    if !connected {
                        ui.add_enabled(
                            false,
                            egui::Button::new(lapinux_core::i18n::t("settings.library.sync_steam")),
                        );
                    } else if ui
                        .add_enabled(
                            !syncing,
                            egui::Button::new(lapinux_core::i18n::t("settings.library.sync_steam")),
                        )
                        .clicked()
                    {
                        self.catalog_notice = true;
                        self.load_catalog(true);
                    }
                    if syncing {
                        crate::section_icons::loading(ui, 18.0);
                        ui.label(
                            egui::RichText::new(lapinux_core::i18n::t("library.sync.in_progress"))
                                .size(11.5)
                                .color(TEXT_DIM()),
                        );
                    }
                });
                if let Some(catalog) = &self.catalog {
                    if catalog.updated > 0 {
                        let age = (lapinux_core::util::now_unix() - catalog.updated).max(0) as u64;
                        ui.add_space(4.0);
                        muted(
                            ui,
                            format!(
                                "{} {}",
                                lapinux_core::i18n::t("settings.library.last_sync"),
                                lapinux_core::util::format_duration(age)
                            ),
                        );
                    }
                }
            });

        self.settings_draft = Some(config);
    }

    /// Onglet Apparence : l'ordre des sections de la barre du haut. Chaque
    /// changement s'applique tout de suite, sans bouton Enregistrer.
    pub(super) fn settings_appearance_tab(&mut self, ui: &mut egui::Ui) {
        let mut config = self
            .settings_draft
            .clone()
            .unwrap_or_else(|| self.config.clone());
        let mut topbar_changed = false;
        let mut activate_theme: Option<String> = None;
        let mut create_theme = false;
        let mut edit_theme: Option<(Option<PathBuf>, String)> = None;
        let mut delete_theme: Option<String> = None;
        let mut create_plugin = false;
        let mut edit_plugin: Option<(Option<PathBuf>, String)> = None;
        let mut delete_plugin: Option<String> = None;
        let mut deleted_plugin: Option<String> = None;
        let mut toggle_plugin: Option<(String, bool)> = None;
        let active_id = config.general.theme.clone();
        let active_plugins = config.general.active_plugins.clone();

        if navigation_settings_card(ui, &mut config) {
            topbar_changed = true;
        }

        let classic_active = theme::CLASSIC_ALIASES
            .iter()
            .any(|alias| active_id.eq_ignore_ascii_case(alias));
        settings_card(
            ui,
            lapinux_core::i18n::t("settings.appearance.themes"),
            lapinux_core::i18n::t("ui.theme_hint"),
            |ui| {
                if let Some(error) = &self.theme_library.error {
                    ui.label(egui::RichText::new(error).size(10.5).color(WARN));
                    ui.add_space(6.0);
                }
                for theme in &self.theme_library.themes {
                    let active = (theme.manifest.id == theme::CLASSIC_ID && classic_active)
                        || (!theme::CLASSIC_ALIASES
                            .iter()
                            .any(|alias| active_id.eq_ignore_ascii_case(alias))
                            && theme.manifest.id.eq_ignore_ascii_case(&active_id));
                    let palette = theme.manifest.palette();
                    ui.horizontal(|ui| {
                        let (swatch, _) =
                            ui.allocate_exact_size(egui::vec2(40.0, 40.0), egui::Sense::hover());
                        let painter = ui.painter();
                        painter.rect_filled(swatch, egui::Rounding::same(8.0), palette.background);
                        painter.circle_filled(
                            swatch.left_bottom() + egui::vec2(10.0, -10.0),
                            4.5,
                            palette.accent,
                        );
                        painter.rect_filled(
                            egui::Rect::from_min_size(
                                swatch.min + egui::vec2(19.0, 7.0),
                                egui::vec2(14.0, 11.0),
                            ),
                            egui::Rounding::same(2.0),
                            palette.card,
                        );
                        painter.rect_filled(
                            egui::Rect::from_min_size(
                                swatch.min + egui::vec2(8.0, 23.0),
                                egui::vec2(14.0, 7.0),
                            ),
                            egui::Rounding::same(2.0),
                            palette.border,
                        );
                        ui.add_space(3.0);
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(&theme.manifest.name)
                                        .size(13.0)
                                        .strong()
                                        .color(TEXT()),
                                );
                                if theme.builtin {
                                    ui.label(
                                        egui::RichText::new(lapinux_core::i18n::t(
                                            "ui.theme_builtin",
                                        ))
                                        .size(9.5)
                                        .color(TEXT_DIM()),
                                    );
                                }
                                if active {
                                    ui.label(
                                        egui::RichText::new(lapinux_core::i18n::t(
                                            "settings.theme.active",
                                        ))
                                        .size(9.5)
                                        .color(OK),
                                    );
                                }
                            });
                            ui.label(
                                egui::RichText::new(&theme.manifest.description)
                                    .size(10.5)
                                    .color(TEXT_DIM()),
                            );
                            ui.label(
                                egui::RichText::new(lapinux_core::i18n::t_fmt(
                                    "settings.theme.colors_count",
                                    &[theme.manifest.layout().label().to_string(), "9".to_string()],
                                ))
                                .size(9.5)
                                .color(TEXT_DIM()),
                            );
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if !active {
                                if ui
                                    .button(lapinux_core::i18n::t("settings.theme.activate"))
                                    .clicked()
                                {
                                    activate_theme = Some(theme.manifest.id.clone());
                                }
                            } else if theme.manifest.id != theme::CLASSIC_ID {
                                if ui
                                    .button(lapinux_core::i18n::t("settings.appearance.disable"))
                                    .clicked()
                                {
                                    activate_theme = Some(theme::CLASSIC_ID.to_string());
                                }
                            }
                            if ui
                                .small_button(lapinux_core::i18n::t("settings.theme.duplicate"))
                                .clicked()
                            {
                                let mut copy = theme.manifest.clone();
                                copy.id = lapinux_core::i18n::t_args(
                                    "ui.theme_copy_id",
                                    &[("id", &copy.id)],
                                );
                                copy.name = lapinux_core::i18n::t_args(
                                    "ui.theme_copy_name",
                                    &[("name", &copy.name)],
                                );
                                edit_theme = Some((None, theme::theme_toml(&copy)));
                            }
                            if !theme.builtin {
                                if ui
                                    .small_button(lapinux_core::i18n::t("settings.theme.edit"))
                                    .clicked()
                                {
                                    edit_theme = Some((
                                        theme.path.clone(),
                                        theme::theme_toml(&theme.manifest),
                                    ));
                                }
                                if ui
                                    .small_button(lapinux_core::i18n::t("settings.theme.delete"))
                                    .clicked()
                                {
                                    delete_theme = Some(theme.manifest.id.clone());
                                }
                            }
                        });
                    });
                    ui.add_space(7.0);
                    let (line, _) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), 1.0),
                        egui::Sense::hover(),
                    );
                    ui.painter().hline(
                        line.x_range(),
                        line.center().y,
                        egui::Stroke::new(1.0_f32, with_alpha(BORDER(), 0.55)),
                    );
                    ui.add_space(7.0);
                }
                ui.horizontal(|ui| {
                    if secondary_button(
                        ui,
                        lapinux_core::i18n::t("settings.appearance.create_theme"),
                    )
                    .clicked()
                    {
                        create_theme = true;
                    }
                    muted(ui, lapinux_core::i18n::t("settings.appearance.theme_path"));
                });
            },
        );

        self.settings_theme_catalog_card(ui);

        settings_card(
            ui,
            lapinux_core::i18n::t("ui.plugins"),
            lapinux_core::i18n::t("ui.plugin_hint"),
            |ui| {
                if let Some(error) = &self.plugin_library.error {
                    ui.label(egui::RichText::new(error).size(10.5).color(WARN));
                    ui.add_space(6.0);
                }
                if self.plugin_library.plugins.is_empty() {
                    muted(ui, lapinux_core::i18n::t("settings.appearance.no_plugins"));
                }
                for plugin in &self.plugin_library.plugins {
                    let mut enabled = active_plugins.iter().any(|id| id == &plugin.manifest.id);
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(&plugin.manifest.name)
                                        .size(12.5)
                                        .strong()
                                        .color(TEXT()),
                                );
                                ui.label(
                                    egui::RichText::new(&plugin.manifest.version)
                                        .size(9.5)
                                        .color(TEXT_DIM()),
                                );
                            });
                            ui.label(
                                egui::RichText::new(&plugin.manifest.description)
                                    .size(10.5)
                                    .color(TEXT_DIM()),
                            );
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if switch(ui, &mut enabled).changed() {
                                toggle_plugin = Some((plugin.manifest.id.clone(), enabled));
                            }
                            if ui
                                .small_button(lapinux_core::i18n::t("settings.theme.edit"))
                                .clicked()
                            {
                                edit_plugin = Some((
                                    plugin.path.clone(),
                                    theme::plugin_toml(&plugin.manifest),
                                ));
                            }
                            if ui
                                .small_button(lapinux_core::i18n::t("settings.theme.delete"))
                                .clicked()
                            {
                                delete_plugin = Some(plugin.manifest.id.clone());
                            }
                        });
                    });
                    ui.add_space(6.0);
                }
                ui.horizontal(|ui| {
                    if secondary_button(
                        ui,
                        lapinux_core::i18n::t("settings.appearance.create_plugin"),
                    )
                    .clicked()
                    {
                        create_plugin = true;
                    }
                    muted(ui, lapinux_core::i18n::t("settings.appearance.plugin_path"));
                });
            },
        );

        // Les actions de fichier sont appliquées après le dessin : les cartes
        // ne modifient jamais la bibliothèque pendant qu'elles la parcourent.
        let mut reload_theme = false;
        if let Some(id) = delete_theme {
            match self.theme_library.delete(&id) {
                Ok(()) => {
                    self.notify(lapinux_core::i18n::t_args(
                        "settings.appearance.theme_removed",
                        &[("id", &id.to_string())],
                    ));
                    reload_theme = true;
                }
                Err(error) => self.notify(error),
            }
        }
        if let Some(id) = delete_plugin {
            match self.plugin_library.delete(&id) {
                Ok(()) => {
                    self.notify(lapinux_core::i18n::t_args(
                        "settings.appearance.plugin_removed",
                        &[("id", &id.to_string())],
                    ));
                    reload_theme = true;
                    deleted_plugin = Some(id);
                }
                Err(error) => self.notify(error),
            }
        }
        let mut target = self.config.clone();
        let mut changed = false;
        // Un plugin supprimé ne doit pas rester coché dans la configuration :
        // sinon un futur plugin du même identifiant se rallumerait tout seul.
        if let Some(id) = deleted_plugin {
            if target
                .general
                .active_plugins
                .iter()
                .any(|active| active == &id)
            {
                target.general.active_plugins.retain(|active| active != &id);
                config.general.active_plugins.retain(|active| active != &id);
                changed = true;
            }
        }
        if let Some(id) = activate_theme {
            target.general.theme = id.clone();
            config.general.theme = id;
            changed = true;
        }
        if let Some((id, enabled)) = toggle_plugin {
            if enabled {
                if !target.general.active_plugins.contains(&id) {
                    target.general.active_plugins.push(id.clone());
                }
            } else {
                target.general.active_plugins.retain(|active| active != &id);
            }
            config.general.active_plugins = target.general.active_plugins.clone();
            changed = true;
        }
        if topbar_changed {
            target.general.topbar_order = config.general.topbar_order.clone();
            target.general.topbar_hidden = config.general.topbar_hidden.clone();
            target.general.startup_page = config.general.startup_page.clone();
            changed = true;
        }
        // Si le thème actif est supprimé, la configuration revient au thème
        // classique même si le fichier a déjà disparu.
        let active_missing = self.theme_library.find(&target.general.theme).is_none()
            && !theme::CLASSIC_ALIASES
                .iter()
                .any(|alias| target.general.theme.eq_ignore_ascii_case(alias));
        if active_missing {
            target.general.theme = theme::CLASSIC_ID.to_string();
            config.general.theme = theme::CLASSIC_ID.to_string();
            changed = true;
        }
        if changed {
            if self.apply_config(target) {
                // Le brouillon garde les retouches des autres onglets tout en
                // portant le thème et les plugins qui viennent d'être choisis.
                self.settings_draft = Some(config);
            }
        } else {
            self.settings_draft = Some(config);
        }
        if reload_theme {
            self.theme_dirty = true;
        }
        if create_theme {
            self.theme_studio = Some(ThemeStudio::new(StudioKind::Theme));
        } else if create_plugin {
            self.theme_studio = Some(ThemeStudio::new(StudioKind::Plugin));
        } else if let Some((path, body)) = edit_theme {
            self.theme_studio = Some(ThemeStudio::edit(StudioKind::Theme, path, body));
        } else if let Some((path, body)) = edit_plugin {
            self.theme_studio = Some(ThemeStudio::edit(StudioKind::Plugin, path, body));
        }
    }

    /// Éditeur TOML des thèmes et plugins. Une erreur de validation laisse la
    /// fenêtre ouverte et le fichier intact, pour ne jamais casser le thème
    /// actif avec une sauvegarde partielle.
    pub(super) fn theme_studio_window(&mut self, ctx: &egui::Context) {
        let Some(mut studio) = self.theme_studio.take() else {
            return;
        };
        let mut open = true;
        let mut save = false;
        let mut cancel = false;
        egui::Window::new(studio.title())
            .id(egui::Id::new("theme-studio"))
            .open(&mut open)
            .resizable(true)
            .collapsible(false)
            .default_width(760.0)
            .default_height(640.0)
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(match studio.kind {
                        StudioKind::Theme => lapinux_core::i18n::t("settings.theme.manifest_hint"),
                        StudioKind::Plugin => lapinux_core::i18n::t("settings.theme.plugin_hint"),
                    })
                    .size(11.0)
                    .color(TEXT_DIM()),
                );
                ui.add_space(6.0);
                egui::ScrollArea::vertical()
                    .max_height(500.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut studio.body)
                                .code_editor()
                                .desired_rows(24)
                                .desired_width(f32::INFINITY),
                        );
                    });
                if let Some(error) = &studio.error {
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new(error).size(11.0).color(WARN));
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .button(lapinux_core::i18n::t("settings.theme.save"))
                        .clicked()
                    {
                        save = true;
                    }
                    if ui
                        .button(lapinux_core::i18n::t("settings.theme.cancel"))
                        .clicked()
                    {
                        cancel = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        muted(ui, lapinux_core::i18n::t("settings.theme.validation"));
                    });
                });
            });

        if save {
            match studio.kind {
                StudioKind::Theme => match theme::parse_theme(&studio.body) {
                    Ok(manifest) => {
                        match self.theme_library.save(&manifest, studio.path.as_deref()) {
                            Ok(_) => {
                                self.notify(lapinux_core::i18n::t_fmt(
                                    "settings.theme.saved",
                                    &[manifest.name.clone()],
                                ));
                                self.theme_dirty = true;
                                cancel = true;
                            }
                            Err(error) => studio.error = Some(error),
                        }
                    }
                    Err(error) => studio.error = Some(error),
                },
                StudioKind::Plugin => match theme::parse_plugin(&studio.body) {
                    Ok(manifest) => {
                        match self.plugin_library.save(&manifest, studio.path.as_deref()) {
                            Ok(_) => {
                                self.notify(lapinux_core::i18n::t_fmt(
                                    "settings.theme.plugin_saved",
                                    &[manifest.name.clone()],
                                ));
                                self.theme_dirty = true;
                                cancel = true;
                            }
                            Err(error) => studio.error = Some(error),
                        }
                    }
                    Err(error) => studio.error = Some(error),
                },
            }
        }

        if open && !cancel {
            self.theme_studio = Some(studio);
        }
    }

    /// Onglet Comptes : les boutiques à gauche, le compte choisi à droite.
    /// Steam garde son panneau ; Epic, GOG et itch.io ont le leur.
    pub(super) fn accounts_tab(&mut self, ui: &mut egui::Ui) -> bool {
        let connected = self
            .steam_status
            .as_ref()
            .map(|status| status.connected)
            .unwrap_or(false);
        let name = self
            .steam_status
            .as_ref()
            .map(|status| status.username.clone())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| self.steam_user.clone());
        self.stores.ensure(&self.client);
        if let Some(message) = self.stores.poll(ui.ctx()) {
            self.notify(message);
        }
        let mut close = false;
        let width = ui.available_width();
        // La hauteur est bornée : dans une zone défilante, `available_height`
        // peut valoir l'infini et le filet vertical partirait avec.
        let height = ui.available_height().clamp(320.0, 760.0);
        let left = 214.0;
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(left, height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(left);
                    let sgdb_ready = self.config.artwork.steamgriddb
                        && !self.config.artwork.steamgriddb_key.trim().is_empty();
                    self.stores.list(ui, connected, &name, sgdb_ready);
                    ui.add_space(14.0);
                    ui.separator();
                    ui.add_space(10.0);
                    // Le slide vaut pour tous les comptes d'un coup : il
                    // masque les jeux-outils, runtimes, Proton et assets de
                    // chaque boutique. Les cases de chaque compte restent
                    // disponibles quand il est coupé.
                    let mut hide_all = self.config.library.hide_all_tools;
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(lapinux_core::i18n::t("settings.tools.hide_all"))
                                .size(12.0)
                                .color(TEXT()),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if switch(ui, &mut hide_all).changed() {
                                let mut config = self.config.clone();
                                config.library.hide_all_tools = hide_all;
                                self.touch_library();
                                self.apply_config(config);
                            }
                        });
                    });
                    ui.add_space(3.0);
                    muted(ui, lapinux_core::i18n::t("settings.tools.hide_all_hint"));
                },
            );
            ui.add_space(10.0);
            let (rule, _) = ui.allocate_exact_size(egui::vec2(1.0, height), egui::Sense::hover());
            ui.painter().vline(
                rule.center().x,
                rule.y_range(),
                egui::Stroke::new(1.0_f32, BORDER()),
            );
            ui.add_space(16.0);
            let content = (width - left - 36.0).max(320.0);
            ui.allocate_ui_with_layout(
                egui::vec2(content, height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(content);
                    if self.stores.steamgriddb_selected {
                        self.steamgriddb_panel(ui);
                    } else {
                        // Même présentation pour tous les comptes : d'abord
                        // le compte, puis sa bibliothèque, puis ses outils.
                        // Steam était le seul à montrer sa bibliothèque avant
                        // sa fiche de compte.
                        if self.stores.selected == lapinux_core::stores::Store::Steam {
                            close = self.connection_tab(ui);
                            self.steam_library_summary(ui);
                        } else if let Some(message) = self.stores.panel(ui, &self.client) {
                            self.notify(message);
                        }
                        // La même carte d'outils pour toutes les boutiques,
                        // dans l'option du compte concerné.
                        ui.add_space(14.0);
                        self.store_tools_card(ui, self.stores.selected);
                    }
                },
            );
        });
        close
    }

    /// Carte « Outils et runtimes » d'un compte : chaque boutique cache ses
    /// outils sans toucher aux autres. Le slide global des comptes reste
    /// prioritaire et la carte le signale.
    fn store_tools_card(&mut self, ui: &mut egui::Ui, store: lapinux_core::stores::Store) {
        let (mut hidden, description) = match store {
            lapinux_core::stores::Store::Steam => (
                self.config.library.hide_tools,
                lapinux_core::i18n::t("ui.tools_steam"),
            ),
            lapinux_core::stores::Store::Epic => (
                self.config.library.hide_epic_tools,
                lapinux_core::i18n::t("ui.tools_epic"),
            ),
            lapinux_core::stores::Store::Gog => (
                self.config.library.hide_gog_tools,
                lapinux_core::i18n::t("settings.tools.gog_tools"),
            ),
            lapinux_core::stores::Store::Itch => (
                self.config.library.hide_itch_tools,
                lapinux_core::i18n::t("settings.tools.itch_assets"),
            ),
        };
        let before = hidden;
        settings_card(
            ui,
            lapinux_core::i18n::t("ui.tools_title"),
            lapinux_core::i18n::t("settings.tools.hidden_hint"),
            |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(if hidden {
                            lapinux_core::i18n::t("settings.tools.hide_account")
                        } else {
                            lapinux_core::i18n::t("settings.tools.show_account")
                        })
                        .size(12.0)
                        .color(TEXT()),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        switch(ui, &mut hidden);
                    });
                });
                ui.add_space(3.0);
                muted(ui, description);
                if self.config.library.hide_all_tools {
                    ui.add_space(4.0);
                    muted(ui, lapinux_core::i18n::t("settings.tools.slide_active"));
                }
            },
        );
        if hidden != before {
            let mut config = self.config.clone();
            match store {
                lapinux_core::stores::Store::Steam => config.library.hide_tools = hidden,
                lapinux_core::stores::Store::Epic => config.library.hide_epic_tools = hidden,
                lapinux_core::stores::Store::Gog => config.library.hide_gog_tools = hidden,
                lapinux_core::stores::Store::Itch => config.library.hide_itch_tools = hidden,
            }
            self.touch_library();
            self.apply_config(config);
        }
        ui.add_space(14.0);
    }

    /// Carte « Bibliothèque » du compte Steam : seulement le nombre de jeux,
    /// comme les autres boutiques, sans dérouler tout le catalogue.
    fn steam_library_summary(&self, ui: &mut egui::Ui) {
        let show_demos = self.config.library.show_demos;
        let catalog = self
            .catalog
            .as_ref()
            .map(|catalog| {
                catalog
                    .entries
                    .iter()
                    .filter(|entry| steam_entry_visible(entry, show_demos))
                    .count()
            })
            .unwrap_or(0);
        let disk = self
            .games
            .iter()
            .filter(|game| game.store == lapinux_core::stores::Store::Steam)
            .filter(|game| !is_steam_tool(&game.name))
            .count();
        let installed = self
            .games
            .iter()
            .filter(|game| game.installed)
            .filter(|game| game.store == lapinux_core::stores::Store::Steam)
            .filter(|game| !is_steam_tool(&game.name))
            .count();
        let total = catalog.max(disk);
        settings_card(
            ui,
            lapinux_core::i18n::t("settings.steam.library"),
            lapinux_core::i18n::t("settings.steam.library_hint"),
            |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(total.to_string())
                            .size(28.0)
                            .strong()
                            .color(TEXT()),
                    );
                    ui.add_space(6.0);
                    ui.vertical(|ui| {
                        ui.add_space(5.0);
                        ui.label(
                            egui::RichText::new(if total == 1 {
                                lapinux_core::i18n::t("settings.tools.game_one")
                            } else {
                                lapinux_core::i18n::t("settings.tools.game_many")
                            })
                            .size(12.0)
                            .color(TEXT_DIM()),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        muted(
                            ui,
                            if installed > 1 {
                                lapinux_core::i18n::t_args(
                                    "settings.steam.installed_many",
                                    &[("installed", &installed.to_string())],
                                )
                            } else {
                                lapinux_core::i18n::t_args(
                                    "settings.steam.installed_one",
                                    &[("installed", &installed.to_string())],
                                )
                            },
                        );
                    });
                });
            },
        );
        ui.add_space(14.0);
    }

    /// Panneau SteamGridDB : sa clé et son activation, à part du compte
    /// Steam. C'est un service d'illustrations communautaires, pas une
    /// boutique : il a sa ligne dans la liste des comptes.
    fn steamgriddb_panel(&mut self, ui: &mut egui::Ui) {
        let mut config = self
            .settings_draft
            .clone()
            .unwrap_or_else(|| self.config.clone());
        let mut save = false;
        let key_present = !config.artwork.steamgriddb_key.trim().is_empty();
        settings_card(
            ui,
            "SteamGridDB",
            lapinux_core::i18n::t("ui.artwork_hint"),
            |ui| {
                ui.horizontal(|ui| {
                    crate::status_dot(ui, if key_present { OK } else { DANGER });
                    ui.add_space(2.0);
                    ui.label(
                        egui::RichText::new(if key_present {
                            lapinux_core::i18n::t("settings.steam.key_saved")
                        } else {
                            lapinux_core::i18n::t("settings.steam.no_key")
                        })
                        .size(12.0)
                        .color(if key_present {
                            TEXT()
                        } else {
                            TEXT_DIM()
                        }),
                    );
                });
                ui.add_space(10.0);
                ui.add(
                    egui::TextEdit::singleline(&mut config.artwork.steamgriddb_key)
                        .password(true)
                        .hint_text(lapinux_core::i18n::t("settings.steam.api_key"))
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(6.0);
                muted(ui, lapinux_core::i18n::t("settings.steam.key_local"));
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if secondary_button(ui, lapinux_core::i18n::t("settings.steam.save_key"))
                        .clicked()
                    {
                        save = true;
                    }
                    ui.hyperlink_to(
                        lapinux_core::i18n::t("settings.steam.get_key"),
                        "https://www.steamgriddb.com/profile/preferences/api",
                    );
                });
                ui.add_space(12.0);
                let mut enabled = config.artwork.steamgriddb;
                let toggled = ui
                    .horizontal(|ui| {
                        let response = switch(ui, &mut enabled);
                        ui.label("Autoriser SteamGridDB");
                        response
                    })
                    .inner
                    .changed();
                if toggled {
                    config.artwork.steamgriddb = enabled;
                    save = true;
                }
            },
        );
        self.settings_draft = Some(config.clone());
        if save {
            let mut target = self.config.clone();
            target.artwork = config.artwork.clone();
            if self.apply_config(target) {
                self.settings_draft = Some(config);
            }
        }
    }

    /// Onglet Compte Steam : une seule fiche, repliée par défaut. L'état de la
    /// session reste visible, les formulaires ne s'ouvrent qu'au besoin.
    fn connection_tab(&mut self, ui: &mut egui::Ui) -> bool {
        let state = self.steam_login_state.clone();
        let status = self.steam_status.clone();
        let connected = status
            .as_ref()
            .map(|status| status.connected)
            .unwrap_or(false);
        let token_saved = status
            .as_ref()
            .map(|status| status.token_saved)
            .unwrap_or(false);
        let username = status
            .as_ref()
            .map(|status| status.username.clone())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| self.steam_user.clone());
        let mut close = false;
        let mut open_library = false;
        let mut submit = false;
        let mut submit_guard = false;
        let mut forget_token = false;
        let mut disconnect = false;
        let mut cancel = false;
        let mut start_qr = false;
        let mut config = self
            .settings_draft
            .clone()
            .unwrap_or_else(|| self.config.clone());

        // Un jeton jamais contrôlé depuis le démarrage est vérifié une fois
        // l'écran ouvert : le daemon rend la main tout de suite et le verdict
        // arrive avec le prochain rafraîchissement.
        let unchecked = status
            .as_ref()
            .and_then(|status| status.token.as_ref())
            .map(|token| token.verdict == SteamTokenVerdict::Unknown)
            .unwrap_or(false);
        let due = self
            .token_check_at
            .map(|at| at.elapsed() > Duration::from_secs(20))
            .unwrap_or(true);
        // Un jeton tout juste obtenu n'a pas besoin d'être revérifié : le
        // logon qui l'a rendu vient d'avoir lieu, et un contrôle immédiat
        // après un refus 84 ne ferait que rallonger la limite de Steam.
        let fresh = status
            .as_ref()
            .and_then(|status| status.token.as_ref())
            .and_then(|token| token.saved_at)
            .map(|stamp| lapinux_core::util::now_unix() - stamp < 30 * 60)
            .unwrap_or(false);
        if unchecked && !state.running && due && !fresh {
            self.token_check_at = Some(Instant::now());
            if let Ok(envelope) = self.client.request(&Req::SteamTokenCheck) {
                if envelope.ok {
                    self.apply_steam_status(envelope.into_data::<SteamStatus>().ok());
                }
            }
        }

        // Fiche unique du compte : identité, état du jeton, puis les trois
        // façons de se connecter, repliées pour ne pas encombrer l'écran.
        egui::Frame::none()
            .fill(CARD())
            .stroke(egui::Stroke::new(1.0_f32, BORDER()))
            .rounding(egui::Rounding::same(14.0))
            .inner_margin(18.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                // Identité : avatar, nom, puis une pastille qui dit l'état en
                // un mot (connecté, en cours, hors ligne).
                ui.horizontal(|ui| {
                    let initial = username
                        .chars()
                        .next()
                        .map(|letter| letter.to_uppercase().to_string())
                        .filter(|letter| !letter.is_empty())
                        .unwrap_or_else(|| "S".to_string());
                    let (state_color, state_line) = if state.running {
                        (WARN, lapinux_core::i18n::t("ui.connection_connecting"))
                    } else if connected {
                        (OK, lapinux_core::i18n::t("settings.connection.connected"))
                    } else {
                        (TEXT_DIM(), "Hors ligne")
                    };
                    let (avatar, _) =
                        ui.allocate_exact_size(egui::vec2(54.0, 54.0), egui::Sense::hover());
                    ui.painter()
                        .circle_filled(avatar.center(), 27.0, CARD_ACTIVE());
                    ui.painter().circle_stroke(
                        avatar.center(),
                        27.0,
                        egui::Stroke::new(1.0_f32, with_alpha(state_color, 0.45)),
                    );
                    ui.painter().text(
                        avatar.center(),
                        egui::Align2::CENTER_CENTER,
                        initial,
                        egui::FontId::proportional(21.0),
                        TEXT(),
                    );
                    // Le point de statut est posé sur le bord de l'avatar.
                    let badge = avatar.center() + egui::vec2(18.0, 18.0);
                    ui.painter().circle_filled(badge, 8.0, CARD());
                    ui.painter().circle_filled(badge, 5.5, state_color);
                    ui.add_space(14.0);
                    // La place du bouton est réservée : un nom long ne doit pas
                    // le pousser hors de la fiche.
                    // Deux gestes tiennent à droite du nom : ouvrir la
                    // bibliothèque et se déconnecter, comme pour les autres
                    // boutiques.
                    let reserve = if connected || token_saved { 310.0 } else { 0.0 };
                    ui.vertical(|ui| {
                        ui.set_max_width((ui.available_width() - reserve).max(160.0));
                        ui.label(
                            egui::RichText::new(if username.trim().is_empty() {
                                lapinux_core::i18n::t("settings.connection.no_steam_account")
                            } else {
                                username.as_str()
                            })
                            .size(17.5)
                            .strong()
                            .color(TEXT()),
                        );
                        ui.add_space(7.0);
                        egui::Frame::none()
                            .fill(with_alpha(state_color, 0.14))
                            .rounding(egui::Rounding::same(9.0))
                            .inner_margin(egui::Margin::symmetric(9.0, 3.0))
                            .show(ui, |ui| {
                                ui.label(
                                    egui::RichText::new(state_line)
                                        .size(10.5)
                                        .strong()
                                        .color(state_color),
                                );
                            });
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if connected
                            && secondary_button(
                                ui,
                                lapinux_core::i18n::t("settings.connection.open_library"),
                            )
                            .clicked()
                        {
                            open_library = true;
                        }
                        if (connected || token_saved)
                            && secondary_button(
                                ui,
                                lapinux_core::i18n::t("settings.connection.disconnect"),
                            )
                            .clicked()
                        {
                            disconnect = true;
                        }
                    });
                });

                ui.add_space(16.0);

                // Trois tuiles : le compte, le verdict du jeton, son âge. Elles
                // remplacent la ligne de texte qui mélangeait les trois.
                let token = status.as_ref().and_then(|status| status.token.clone());
                let (dot, verdict) = match token.as_ref().map(|token| token.verdict) {
                    Some(SteamTokenVerdict::Valid) => (OK, "Valide"),
                    Some(SteamTokenVerdict::Checking) => {
                        (WARN, lapinux_core::i18n::t("settings.connection.checking"))
                    }
                    Some(SteamTokenVerdict::Expired) => {
                        (DANGER, lapinux_core::i18n::t("settings.connection.expired"))
                    }
                    Some(SteamTokenVerdict::RateLimited) => (
                        WARN,
                        lapinux_core::i18n::t("settings.connection.temporarily_blocked"),
                    ),
                    Some(SteamTokenVerdict::Unknown) => (
                        TEXT_DIM(),
                        lapinux_core::i18n::t("settings.connection.unverified"),
                    ),
                    None => (
                        TEXT_DIM(),
                        lapinux_core::i18n::t("settings.connection.no_token"),
                    ),
                };
                let account_line = token
                    .as_ref()
                    .map(|token| token.account_name.clone())
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or_else(|| {
                        if username.trim().is_empty() {
                            "—".to_string()
                        } else {
                            username.clone()
                        }
                    });
                let age = token
                    .as_ref()
                    .and_then(|token| token.saved_at)
                    .map(|stamp| token_age(lapinux_core::util::now_unix() - stamp))
                    .unwrap_or_else(|| {
                        lapinux_core::i18n::t("settings.connection.never_saved").to_string()
                    });
                let tile_gap = 10.0;
                let tile_width = ((ui.available_width() - tile_gap * 2.0) / 3.0).max(110.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = tile_gap;
                    stat_tile(
                        ui,
                        tile_width,
                        lapinux_core::i18n::t("settings.connection.account"),
                        &account_line,
                        TEXT(),
                    );
                    stat_tile(ui, tile_width, "Session", verdict, dot);
                    stat_tile(
                        ui,
                        tile_width,
                        lapinux_core::i18n::t("settings.connection.token_saved"),
                        &age,
                        TEXT_DIM(),
                    );
                });

                // La réponse de Steam n'apprend quelque chose que si le jeton
                // est mort : sinon la tuile suffit.
                let attention = token.as_ref().filter(|token| {
                    matches!(
                        token.verdict,
                        SteamTokenVerdict::Expired | SteamTokenVerdict::RateLimited
                    )
                });
                if let Some(detail) = attention.and_then(|token| token.detail.clone()) {
                    ui.add_space(8.0);
                    muted(ui, sentence(&detail));
                }

                ui.add_space(14.0);
                // Les deux gestes utiles, sans ouvrir de formulaire.
                ui.horizontal(|ui| {
                    if secondary_button(ui, lapinux_core::i18n::t("settings.connection.renew_qr"))
                        .clicked()
                    {
                        start_qr = true;
                    }
                    if token_saved {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .small_button(lapinux_core::i18n::t("ui.forget_token"))
                                .clicked()
                            {
                                forget_token = true;
                            }
                        });
                    }
                });
                ui.add_space(12.0);

                if state.running {
                    let elapsed = self
                        .login_since
                        .map(|start| start.elapsed())
                        .unwrap_or_default();
                    let seconds = elapsed.as_secs();
                    ui.horizontal(|ui| {
                        crate::section_icons::loading(ui, 18.0);
                        ui.label(
                            egui::RichText::new(if state.stage.is_empty() {
                                lapinux_core::i18n::t("settings.connection.connecting").to_string()
                            } else {
                                sentence(&state.stage)
                            })
                            .size(13.5)
                            .strong()
                            .color(TEXT()),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            muted(ui, format!("{seconds} s"));
                        });
                    });
                    ui.add_space(10.0);
                    if let Some(url) = state.qr.clone() {
                        muted(ui, lapinux_core::i18n::t("settings.connection.qr_hint"));
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            ui.add_space((ui.available_width() - 202.0).max(0.0) / 2.0);
                            if let Some(texture) =
                                steam_qr_texture(ui.ctx(), &mut self.steam_qr, &url)
                            {
                                egui::Frame::none()
                                    .fill(egui::Color32::WHITE)
                                    .rounding(egui::Rounding::same(12.0))
                                    .inner_margin(6.0)
                                    .show(ui, |ui| {
                                        ui.add(
                                            egui::Image::from_texture(&texture)
                                                .fit_to_exact_size(egui::vec2(190.0, 190.0)),
                                        );
                                    });
                            } else {
                                muted(
                                    ui,
                                    lapinux_core::i18n::t("settings.connection.qr_unavailable"),
                                );
                            }
                        });
                        ui.add_space(10.0);
                        if secondary_button(
                            ui,
                            lapinux_core::i18n::t("settings.connection.cancel_login"),
                        )
                        .clicked()
                        {
                            cancel = true;
                        }
                    } else if state.waits_for_code() {
                        muted(
                            ui,
                            state.guard_message.clone().unwrap_or_else(|| {
                                lapinux_core::i18n::t("settings.connection.code_hint").into()
                            }),
                        );
                        ui.add_space(8.0);
                        ui.add(
                            egui::TextEdit::singleline(&mut self.steam_guard)
                                .hint_text("Code Steam Guard")
                                .desired_width(240.0),
                        );
                        if state.guard_retry {
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new(lapinux_core::i18n::t(
                                    "settings.connection.code_refused",
                                ))
                                .size(11.0)
                                .color(DANGER),
                            );
                        }
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            if primary_button(
                                ui,
                                lapinux_core::i18n::t("settings.connection.submit_code"),
                            )
                            .clicked()
                            {
                                submit_guard = true;
                            }
                            if secondary_button(
                                ui,
                                lapinux_core::i18n::t("settings.connection.cancel"),
                            )
                            .clicked()
                            {
                                cancel = true;
                            }
                        });
                    } else if state.waits_for_confirmation() {
                        muted(
                            ui,
                            if matches!(state.guard, Some(SteamGuardKind::EmailConfirmation)) {
                                lapinux_core::i18n::t("settings.connection.email_link")
                            } else {
                                lapinux_core::i18n::t("settings.connection.three_minutes")
                            },
                        );
                        ui.add_space(10.0);
                        if !matches!(state.guard, Some(SteamGuardKind::EmailConfirmation)) {
                            steam_step(
                                ui,
                                1,
                                lapinux_core::i18n::t("settings.connection.open_phone"),
                            );
                            steam_step(ui, 2, lapinux_core::i18n::t("ui.confirm_tab"));
                            steam_step(ui, 3, lapinux_core::i18n::t("ui.approve_login"));
                            ui.add_space(10.0);
                        }
                        let remaining = 180u64.saturating_sub(seconds);
                        let progress = (seconds as f32 / 180.0).clamp(0.0, 1.0);
                        ui.add(
                            egui::ProgressBar::new(progress)
                                .desired_height(6.0)
                                .text(""),
                        );
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            muted(
                                ui,
                                lapinux_core::i18n::t_args(
                                    "settings.connection.waiting_since",
                                    &[("seconds", &seconds.to_string())],
                                ),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    muted(
                                        ui,
                                        lapinux_core::i18n::t_fmt(
                                            "ui.time_left",
                                            &[lapinux_core::util::format_duration(remaining)],
                                        ),
                                    );
                                },
                            );
                        });
                        ui.add_space(10.0);
                        if secondary_button(
                            ui,
                            lapinux_core::i18n::t("settings.connection.cancel_login"),
                        )
                        .clicked()
                        {
                            cancel = true;
                        }
                    } else {
                        muted(ui, lapinux_core::i18n::t("settings.connection.no_client"));
                        ui.add_space(8.0);
                        if secondary_button(
                            ui,
                            lapinux_core::i18n::t("settings.connection.cancel_login"),
                        )
                        .clicked()
                        {
                            cancel = true;
                        }
                    }
                } else {
                    if let Some(error) = &state.error {
                        egui::Frame::none()
                            .fill(egui::Color32::from_rgb(44, 29, 29))
                            .stroke(egui::Stroke::new(1.0_f32, with_alpha(DANGER, 0.45)))
                            .rounding(egui::Rounding::same(10.0))
                            .inner_margin(12.0)
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.label(
                                    egui::RichText::new(lapinux_core::i18n::t(
                                        "ui.connection_failed",
                                    ))
                                    .size(12.0)
                                    .strong()
                                    .color(DANGER),
                                );
                                ui.add_space(2.0);
                                muted(ui, error.clone());
                            });
                        ui.add_space(10.0);
                    }

                    // Le QR d'abord : c'est la voie la plus simple et la plus
                    // sûre, aucun mot de passe ne sort de la machine.
                    egui::CollapsingHeader::new(
                        egui::RichText::new(lapinux_core::i18n::t("settings.connection.qr_title"))
                            .size(12.5)
                            .strong()
                            .color(TEXT()),
                    )
                    .id_salt("steam-qr-login")
                    .show(ui, |ui| {
                        muted(ui, lapinux_core::i18n::t("settings.connection.scan"));
                        ui.add_space(10.0);
                        if primary_button(ui, lapinux_core::i18n::t("settings.connection.show_qr"))
                            .clicked()
                        {
                            start_qr = true;
                        }
                    });

                    egui::CollapsingHeader::new(
                        egui::RichText::new(if connected {
                            lapinux_core::i18n::t("settings.connection.switch_account")
                        } else {
                            lapinux_core::i18n::t("settings.connection.password_login")
                        })
                        .size(12.5)
                        .strong()
                        .color(TEXT()),
                    )
                    .id_salt("steam-credentials")
                    .show(ui, |ui| {
                        muted(ui, lapinux_core::i18n::t("settings.connection.no_password"));
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new(lapinux_core::i18n::t(
                                "settings.connection.steam_account",
                            ))
                            .size(11.5)
                            .color(TEXT_DIM()),
                        );
                        ui.add(
                            egui::TextEdit::singleline(&mut self.steam_user)
                                .hint_text(lapinux_core::i18n::t(
                                    "settings.connection.account_name",
                                ))
                                .desired_width(f32::INFINITY),
                        );
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new(lapinux_core::i18n::t("ui.password"))
                                .size(11.5)
                                .color(TEXT_DIM()),
                        );
                        ui.add(
                            egui::TextEdit::singleline(&mut self.steam_password)
                                .password(true)
                                .hint_text(if token_saved {
                                    lapinux_core::i18n::t("settings.connection.optional")
                                } else {
                                    lapinux_core::i18n::t("ui.password_hint")
                                })
                                .desired_width(f32::INFINITY),
                        );
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new(lapinux_core::i18n::t("ui.guard_optional"))
                                .size(11.5)
                                .color(TEXT_DIM()),
                        );
                        ui.add(
                            egui::TextEdit::singleline(&mut self.steam_guard)
                                .hint_text(lapinux_core::i18n::t("settings.connection.if_you_have"))
                                .desired_width(f32::INFINITY),
                        );
                        ui.add_space(10.0);
                        if primary_button(ui, lapinux_core::i18n::t("library.sign_in")).clicked() {
                            submit = true;
                        }
                    });

                    egui::CollapsingHeader::new(
                        egui::RichText::new(lapinux_core::i18n::t("ui.lapinux_token"))
                            .size(12.5)
                            .strong()
                            .color(TEXT()),
                    )
                    .id_salt("steam-token")
                    .show(ui, |ui| {
                        muted(ui, lapinux_core::i18n::t("settings.connection.token_hint"));
                        ui.add_space(6.0);
                        ui.add(
                            egui::TextEdit::singleline(&mut self.steam_token)
                                .hint_text(lapinux_core::i18n::t("ui.refresh_token"))
                                .desired_width(f32::INFINITY),
                        );
                        if token_saved {
                            ui.add_space(8.0);
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(lapinux_core::i18n::t(
                                        "settings.connection.token_available",
                                    ))
                                    .size(10.5)
                                    .color(OK),
                                );
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui.small_button("Oublier").clicked() {
                                            forget_token = true;
                                        }
                                    },
                                );
                            });
                        }
                    });
                }
            });

        ui.add_space(14.0);
        settings_card(
            ui,
            lapinux_core::i18n::t("steam.activity"),
            lapinux_core::i18n::t("steam.activity_desc"),
            |ui| {
                ui.horizontal(|ui| {
                    switch(ui, &mut config.steam.show_playing);
                    ui.label(
                        egui::RichText::new(lapinux_core::i18n::t("steam.show_playing"))
                            .size(12.0)
                            .color(TEXT()),
                    );
                });
                muted(ui, lapinux_core::i18n::t("steam.show_playing_desc"));
            },
        );

        self.settings_draft = Some(config);

        if start_qr {
            self.start_steam_qr();
        }
        if cancel {
            self.cancel_steam_login();
        }
        if forget_token {
            self.forget_steam_token();
        }
        if disconnect {
            self.disconnect_steam();
        }
        if submit {
            self.submit_steam_login();
        }
        if submit_guard {
            self.submit_steam_guard();
        }
        if open_library {
            self.page = Page::Library;
            close = true;
        }
        if self.steam_login_state.running {
            ui.ctx().request_repaint_after(Duration::from_millis(250));
        }
        close
    }

    /// Thèmes publiés dans le catalogue : on les installe dans `themes/`
    /// sans toucher aux jeux, puis l'utilisateur les active comme les autres.
    fn settings_theme_catalog_card(&mut self, ui: &mut egui::Ui) {
        self.poll_theme_install(ui.ctx());
        if self.theme_catalog.is_none()
            && self.theme_catalog_rx.is_none()
            && self
                .theme_catalog_asked
                .map_or(true, |asked| asked.elapsed() > Duration::from_secs(30))
        {
            self.ask_theme_catalog(ui.ctx());
        }
        let mut install: Option<String> = None;
        let mut refresh = false;
        let catalog = self.theme_catalog.clone();
        settings_card(
            ui,
            lapinux_core::i18n::t("settings.appearance.catalog"),
            lapinux_core::i18n::t("settings.appearance.catalog_desc"),
            |ui| {
                if let Some(set) = &catalog {
                    if set.themes.is_empty() {
                        muted(ui, lapinux_core::i18n::t("settings.appearance.catalog_empty"));
                    }
                    for entry in &set.themes {
                        let installed = set.installed.iter().any(|id| id == &entry.id);
                        ui.horizontal_top(|ui| {
                            ui.vertical(|ui| {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new(&entry.title)
                                            .size(12.5)
                                            .strong()
                                            .color(TEXT()),
                                    );
                                    if entry.status == "experimental" {
                                        ui.label(
                                            egui::RichText::new(lapinux_core::i18n::t(
                                                "settings.appearance.catalog_experimental",
                                            ))
                                            .size(9.5)
                                            .color(WARN),
                                        );
                                    }
                                    if installed {
                                        ui.label(
                                            egui::RichText::new(lapinux_core::i18n::t(
                                                "settings.appearance.catalog_installed",
                                            ))
                                            .size(9.5)
                                            .color(OK),
                                        );
                                    }
                                });
                                ui.label(
                                    egui::RichText::new(&entry.description)
                                        .size(10.5)
                                        .color(TEXT_DIM()),
                                );
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{} · {} · {}",
                                        entry.author.name, entry.license, entry.version
                                    ))
                                    .size(9.5)
                                    .color(TEXT_DIM()),
                                );
                            });
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if !installed
                                        && entry.available()
                                        && ui
                                            .button(lapinux_core::i18n::t(
                                                "settings.appearance.catalog_install",
                                            ))
                                            .clicked()
                                    {
                                        install = Some(entry.id.clone());
                                    }
                                },
                            );
                        });
                        ui.add_space(6.0);
                    }
                    if !set.remote {
                        muted(
                            ui,
                            lapinux_core::i18n::t("settings.appearance.catalog_offline"),
                        );
                    }
                } else {
                    muted(
                        ui,
                        lapinux_core::i18n::t("settings.appearance.catalog_loading"),
                    );
                }
                ui.horizontal(|ui| {
                    if secondary_button(
                        ui,
                        lapinux_core::i18n::t("settings.appearance.catalog_refresh"),
                    )
                    .clicked()
                    {
                        refresh = true;
                    }
                });
            },
        );
        if refresh {
            self.theme_catalog = None;
            self.theme_catalog_asked = None;
            self.ask_theme_catalog(ui.ctx());
        }
        if let Some(id) = install {
            self.install_catalog_theme(id, ui.ctx());
        }
    }

    fn ask_theme_catalog(&mut self, ctx: &egui::Context) {
        if self.theme_catalog_rx.is_some() {
            return;
        }
        self.theme_catalog_asked = Some(Instant::now());
        let (tx, rx) = mpsc::channel();
        self.theme_catalog_rx = Some(rx);
        let client = self.client.detached();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let data = client
                .request(&Req::ThemeCatalog)
                .ok()
                .and_then(|envelope| envelope.into_data().ok());
            let _ = tx.send(data);
            ctx.request_repaint();
        });
    }

    fn install_catalog_theme(&mut self, id: String, ctx: &egui::Context) {
        if self.theme_install_rx.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.theme_install_rx = Some((id.clone(), rx));
        let client = self.client.detached();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = client
                .request(&Req::InstallTheme { id })
                .map_err(|error| error.to_string())
                .and_then(|envelope| {
                    if envelope.ok {
                        Ok(())
                    } else {
                        Err(envelope.error.unwrap_or_else(|| {
                            lapinux_core::i18n::t("settings.storage.unknown_error").into()
                        }))
                    }
                });
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }

    fn poll_theme_install(&mut self, ctx: &egui::Context) {
        if let Some(rx) = self.theme_catalog_rx.take() {
            match rx.try_recv() {
                Ok(data) => self.theme_catalog = data,
                Err(mpsc::TryRecvError::Empty) => {
                    self.theme_catalog_rx = Some(rx);
                    ctx.request_repaint_after(Duration::from_millis(200));
                }
                Err(mpsc::TryRecvError::Disconnected) => {}
            }
        }
        if let Some((id, rx)) = self.theme_install_rx.take() {
            match rx.try_recv() {
                Ok(Ok(())) => {
                    self.theme_library = theme::ThemeLibrary::load(self.paths.themes_dir());
                    self.theme_catalog = None;
                    self.theme_catalog_asked = None;
                    self.notify(lapinux_core::i18n::t_args(
                        "settings.appearance.catalog_installed_notice",
                        &[("id", &id)],
                    ));
                }
                Ok(Err(error)) => self.notify(error),
                Err(mpsc::TryRecvError::Empty) => {
                    self.theme_install_rx = Some((id, rx));
                    ctx.request_repaint_after(Duration::from_millis(200));
                }
                Err(mpsc::TryRecvError::Disconnected) => {}
            }
        }
    }

    /// Rapport anonyme prêt à coller : mêmes champs que la page Diagnostic,
    /// plus les faits machine (noyau, bureau, mémoire, processeur, GPU). Aucun
    /// chemin, nom de jeu, compte ni adresse réseau n'y entre.
    fn diagnostic_text(&mut self) -> Option<String> {
        if !self.doctor_requested {
            self.doctor_requested = true;
            self.doctor = self
                .client
                .request_with_timeout(&Req::Doctor, Duration::from_secs(3))
                .ok()
                .and_then(|envelope| envelope.into_data().ok());
        }
        let client = self.client.detached_quiet();
        let daemon = match client.request_with_timeout(&Req::Ping, Duration::from_millis(600)) {
            Ok(envelope) => Some(envelope.ok),
            Err(lapinux_core::Error::IpcTimeout(_)) => None,
            Err(_) => Some(false),
        };
        let jobs: Option<Vec<Job>> = client
            .request_with_timeout(&Req::Jobs, Duration::from_secs(2))
            .ok()
            .and_then(|envelope| envelope.into_data().ok());
        let value = lapinux_core::diagnostic::report(self.doctor.as_ref(), jobs.as_deref(), daemon);
        serde_json::to_string_pretty(&value).ok()
    }

    /// Onglet Journal : d'abord le rapport anonyme à copier pour réparer,
    /// ensuite les fichiers bruts pour qui veut les lire.
    pub(super) fn logs_tab(&mut self, ui: &mut egui::Ui) {
        let mut copy = false;
        egui::Frame::none()
            .fill(CARD())
            .stroke(egui::Stroke::new(1.0_f32, BORDER()))
            .rounding(egui::Rounding::same(12.0))
            .inner_margin(14.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new(lapinux_core::i18n::t(
                                "settings.logs.report_title",
                            ))
                            .size(13.0)
                            .strong()
                            .color(TEXT()),
                        );
                        ui.add_space(2.0);
                        muted(ui, lapinux_core::i18n::t("settings.logs.report_desc"));
                    });
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            if ui
                                .button(lapinux_core::i18n::t("settings.logs.report_copy"))
                                .clicked()
                            {
                                copy = true;
                            }
                        },
                    );
                });
                ui.add_space(10.0);
                ui.label(
                    egui::RichText::new(lapinux_core::i18n::t("settings.logs.report_contents"))
                        .size(10.5)
                        .strong()
                        .color(TEXT()),
                );
                ui.add_space(3.0);
                for line in [
                    lapinux_core::i18n::t("settings.logs.report_line_version"),
                    lapinux_core::i18n::t("settings.logs.report_line_system"),
                    lapinux_core::i18n::t("settings.logs.report_line_hardware"),
                    lapinux_core::i18n::t("settings.logs.report_line_state"),
                    lapinux_core::i18n::t("settings.logs.report_line_none"),
                ] {
                    muted(ui, format!("• {line}"));
                }
            });
        if copy {
            match self.diagnostic_text() {
                Some(text) => {
                    ui.ctx().copy_text(text);
                    self.notify(lapinux_core::i18n::t("settings.logs.report_copied"));
                }
                None => self.notify(lapinux_core::diagnostic::label("failure")),
            }
        }
        ui.add_space(12.0);

        egui::CollapsingHeader::new(
            egui::RichText::new(lapinux_core::i18n::t("settings.logs.advanced"))
                .size(11.0)
                .strong(),
        )
        .default_open(false)
        .show(ui, |ui| {
            let files = self
                .logs
                .as_ref()
                .map(|view| view.files.clone())
                .unwrap_or_default();
            let current = self
                .logs
                .as_ref()
                .map(|view| view.file.clone())
                .unwrap_or_default();
            let size = self
                .logs
                .as_ref()
                .and_then(|view| view.files.iter().find(|file| file.name == current))
                .map(|file| file.size)
                .unwrap_or(0);
            let count = self.logs.as_ref().map(|view| view.lines.len()).unwrap_or(0);
            // Le texte est mis en forme une fois : horodatage discret, niveau
            // coloré, message lisible. Les sorties brutes restent en gris.
            let job = log_layout(
                self.logs
                    .as_ref()
                    .map(|view| view.lines.as_slice())
                    .unwrap_or(&[]),
            );

            let mut pick: Option<String> = None;
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(lapinux_core::i18n::t("settings.logs.file"))
                        .size(11.0)
                        .color(TEXT_DIM()),
                );
                egui::ComboBox::from_id_salt("log-file")
                    .selected_text(if current.is_empty() {
                        lapinux_core::i18n::t("settings.logs.no_file").to_string()
                    } else {
                        current.clone()
                    })
                    .width(220.0)
                    .show_ui(ui, |ui| {
                        for file in &files {
                            let label = format!(
                                "{}  ·  {}",
                                file.name,
                                lapinux_core::util::format_bytes(file.size)
                            );
                            if ui.selectable_label(current == file.name, label).clicked() {
                                pick = Some(file.name.clone());
                            }
                        }
                    });
                if !current.is_empty() {
                    muted(
                        ui,
                        format!(
                            "{} · {} lignes",
                            lapinux_core::util::format_bytes(size),
                            count
                        ),
                    );
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button(lapinux_core::i18n::t("settings.logs.refresh"))
                        .clicked()
                    {
                        self.logs_poll = Instant::now() - Duration::from_secs(5);
                    }
                });
            });
            if let Some(name) = pick {
                self.logs_file = Some(name);
                self.logs_poll = Instant::now() - Duration::from_secs(5);
            }
            if let Some(error) = &self.logs_error {
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(sentence(&error))
                        .size(11.0)
                        .color(DANGER),
                );
            }
            ui.add_space(8.0);
            egui::Frame::none()
                .fill(egui::Color32::from_rgb(16, 17, 18))
                .stroke(egui::Stroke::new(1.0_f32, BORDER()))
                .rounding(egui::Rounding::same(10.0))
                .inner_margin(egui::Margin::symmetric(12.0, 10.0))
                .show(ui, |ui| {
                    egui::ScrollArea::both()
                        .id_salt("log-console")
                        .stick_to_bottom(true)
                        .auto_shrink([false, false])
                        .max_height(400.0)
                        .show(ui, |ui| {
                            if count == 0 {
                                ui.label(
                                    egui::RichText::new(if files.is_empty() {
                                        lapinux_core::i18n::t("settings.logs.empty")
                                    } else {
                                        lapinux_core::i18n::t("settings.logs.empty_file")
                                    })
                                    .size(11.0)
                                    .color(TEXT_DIM()),
                                );
                            } else {
                                // Pas de retour à la ligne : une URL ou une
                                // ligne de jeu long se lit en défilant
                                // horizontalement.
                                let label = egui::Label::new(job)
                                    .selectable(true)
                                    .wrap_mode(egui::TextWrapMode::Extend);
                                ui.add(label);
                            }
                        });
                });
            ui.add_space(6.0);
            muted(ui, lapinux_core::i18n::t("settings.logs.auto_reload"));
        });
    }
}
