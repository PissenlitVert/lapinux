//! Réglages « En jeu » et « Notifications ».
//!
//! Ces deux pages vivaient dans `settings_tabs.rs` ; elles sont isolées ici
//! pour garder les réglages HUD globaux et leurs aperçus au même endroit.

use super::*;
use lapinux_core::i18n::t;

impl LapinuxApp {
    /// Onglet En jeu : raccourcis de l'overlay et réglages du HUD.
    pub(super) fn settings_in_game_tab(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        let mut config = self
            .settings_draft
            .clone()
            .unwrap_or_else(|| self.config.clone());

        settings_card(
            ui,
            t("settings.in_game.shortcuts"),
            t("settings.in_game.shortcuts_desc"),
            |ui| {
                settings_row(
                    ui,
                    t("settings.general.quickmenu_shortcut"),
                    t("settings.general.quickmenu_shortcut"),
                    |ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut config.overlay.hotkey)
                                .desired_width(190.0),
                        );
                    },
                );
                settings_row(
                    ui,
                    t("settings.general.controller"),
                    t("settings.general.guide_shortcut"),
                    |ui| {
                        super::settings_tabs::pad_hotkey_picker(ui, &mut config.overlay.pad_hotkey);
                    },
                );
            },
        );

        ui.add_space(14.0);
        // La capture des manettes appartient à la partie en cours : elle vit
        // avec les raccourcis et le HUD de l'onglet En jeu, pas dans Général.
        settings_card(
            ui,
            t("settings.general.controllers_overlay"),
            t("settings.general.input_desc"),
            |ui| {
                settings_row(
                    ui,
                    t("settings.general.controllers"),
                    t("settings.general.input_isolation_desc"),
                    |ui| {
                        switch(ui, &mut config.input.grab_devices);
                    },
                );
            },
        );

        ui.add_space(14.0);
        super::settings_hud::hud_settings_card(ui, &mut config);
        ui.add_space(2.0);
        super::settings_hud::record_hud_settings_card(ui, &mut config);
        self.settings_draft = Some(config);
    }

    /// Onglet Notifications : interrupteur global, catégories, amis,
    /// démonstration de l'animation et durée de conservation.
    pub(super) fn settings_notifications_tab(&mut self, ui: &mut egui::Ui) {
        ui.add_space(2.0);
        let mut config = self
            .settings_draft
            .clone()
            .unwrap_or_else(|| self.config.clone());
        let notifications_enabled = config.notifications.enabled;
        let mut test_animation = false;

        settings_card(
            ui,
            t("settings.notifications.categories_card"),
            t("settings.notifications.categories_desc"),
            |ui| {
                settings_row(
                    ui,
                    t("settings.notifications.enabled"),
                    t("settings.notifications.enabled_desc"),
                    |ui| {
                        switch(ui, &mut config.notifications.enabled);
                    },
                );
                ui.add_space(4.0);
                for (enabled, label, hint) in [
                    (
                        &mut config.notifications.wishlist_promotions,
                        "settings.notifications.wishlist_promotions",
                        "settings.notifications.wishlist_promotions_desc",
                    ),
                    (
                        &mut config.notifications.steam_other,
                        "settings.notifications.steam_other",
                        "settings.notifications.steam_other_desc",
                    ),
                    (
                        &mut config.notifications.store_notifications,
                        "settings.notifications.stores",
                        "settings.notifications.stores_desc",
                    ),
                ] {
                    settings_row(
                        ui,
                        t(label),
                        t(hint),
                        |ui| {
                            ui.add_enabled_ui(notifications_enabled, |ui| {
                                switch(ui, enabled);
                            });
                        },
                    );
                }
            },
        );

        ui.add_space(10.0);
        settings_card(
            ui,
            t("settings.notifications.friends_card"),
            t("settings.notifications.friends_desc"),
            |ui| {
                for (enabled, label, hint) in [
                    (
                        &mut config.notifications.friend_online,
                        "settings.notifications.online",
                        "settings.notifications.online_desc",
                    ),
                    (
                        &mut config.notifications.friend_offline,
                        "settings.notifications.offline",
                        "settings.notifications.offline_desc",
                    ),
                    (
                        &mut config.notifications.friend_game_started,
                        "settings.notifications.game_started",
                        "settings.notifications.game_started_desc",
                    ),
                    (
                        &mut config.notifications.friend_game_stopped,
                        "settings.notifications.game_stopped",
                        "settings.notifications.game_stopped_desc",
                    ),
                    (
                        &mut config.notifications.friend_messages,
                        "settings.notifications.messages",
                        "settings.notifications.messages_desc",
                    ),
                ] {
                    settings_row(
                        ui,
                        t(label),
                        t(hint),
                        |ui| {
                            ui.add_enabled_ui(notifications_enabled, |ui| {
                                switch(ui, enabled);
                            });
                        },
                    );
                }
            },
        );

        ui.add_space(10.0);
        settings_card(
            ui,
            t("settings.notifications.test_animation"),
            t("settings.notifications.test_animation_desc"),
            |ui| {
                if ui
                    .button(t(
                        "settings.notifications.test_animation_button",
                    ))
                    .clicked()
                {
                    test_animation = true;
                }
                muted(
                    ui,
                    t("settings.notifications.test_animation_caption"),
                );
            },
        );

        ui.add_space(10.0);
        settings_card(
            ui,
            t("settings.notifications.retention_card"),
            t("settings.notifications.retention_desc"),
            |ui| {
                settings_row(
                    ui,
                    t("settings.notifications.retention_days"),
                    t("settings.notifications.retention_days_desc"),
                    |ui| {
                        let suffix = format!(
                            " {}",
                            t("settings.notifications.retention_unit")
                        );
                        settings_slider(
                            ui,
                            &mut config.notifications.retention_days,
                            1..=90,
                            &suffix,
                        );
                    },
                );
            },
        );

        self.settings_draft = Some(config.clone());
        if test_animation {
            self.apply_config(config);
            self.start_notification_animation_test();
            self.close_settings();
        }
    }
}
