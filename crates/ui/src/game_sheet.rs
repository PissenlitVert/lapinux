//! Fiches de jeu et sauvegardes cloud.

use super::*;
use lapinux_core::i18n::{t, t_args, t_fmt};

/// Une bibliothèque détectée indique une possibilité, pas une garantie que
/// le matériel ou le jeu activera DLSS.
fn show_ngx_option(
    game: &Game,
    tech: Option<&Result<lapinux_core::model::GameTech, String>>,
) -> bool {
    game.installed && !game.native && matches!(tech, Some(Ok(found)) if !found.dlss.is_empty())
}

#[cfg(test)]
mod capability_tests {
    use super::*;

    #[test]
    fn ngx_is_shown_only_for_installed_windows_games_with_detected_dlss() {
        let mut game = Game {
            installed: true,
            ..Default::default()
        };
        let found = Ok(lapinux_core::model::GameTech {
            dlss: lapinux_core::model::DlssInfo {
                dlss: true,
                ..Default::default()
            },
            ..Default::default()
        });
        let empty = Ok(lapinux_core::model::GameTech::default());
        let error = Err("scan failed".to_string());
        assert!(show_ngx_option(&game, Some(&found)));
        assert!(!show_ngx_option(&game, None));
        assert!(!show_ngx_option(&game, Some(&empty)));
        assert!(!show_ngx_option(&game, Some(&error)));
        game.native = true;
        assert!(!show_ngx_option(&game, Some(&found)));
        game.native = false;
        game.installed = false;
        assert!(!show_ngx_option(&game, Some(&found)));
    }
}

/// Petit repère « i » : le survol explique la technologie sans alourdir la
/// ligne de réglage. Le tracé suit les autres pictogrammes maison.
fn info_mark(ui: &mut egui::Ui, text: &str) {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
    let response = response.on_hover_cursor(egui::CursorIcon::Help);
    ui.painter()
        .circle_stroke(rect.center(), 6.0, egui::Stroke::new(1.0_f32, TEXT_DIM()));
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "i",
        egui::FontId::proportional(9.0),
        TEXT_DIM(),
    );
    response.on_hover_text(text);
}

/// Ligne d'information de l'onglet Informations : un libellé discret et une
/// valeur qui reste lisible même quand le chemin est long.
fn info_line(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.add_sized(
            [118.0, 19.0],
            egui::Label::new(egui::RichText::new(label).size(11.0).color(TEXT_DIM())),
        );
        ui.add(egui::Label::new(egui::RichText::new(value).size(11.5).color(TEXT())).truncate())
            .on_hover_text(value);
    });
}

fn bool_setting_label(value: bool) -> String {
    if value {
        t("graphics.enabled").to_string()
    } else {
        t("graphics.off").to_string()
    }
}

fn inherited_bool(value: bool) -> String {
    t_args(
        "game_settings.default_global",
        &[("value", &bool_setting_label(value))],
    )
}

/// Interrupteur par jeu : sans valeur explicite il affiche et applique le
/// réglage global ; le premier clic crée l'override, le bouton le retire.
fn game_override_bool(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut Option<bool>,
    global: bool,
) -> bool {
    let mut changed = false;
    settings_row(ui, label, "", |ui| {
        let mut current = value.unwrap_or(global);
        if switch(ui, &mut current).changed() {
            *value = Some(current);
            changed = true;
        }
        if value.is_some()
            && ui
                .small_button(t("game_settings.reset_default"))
                .clicked()
        {
            *value = None;
            changed = true;
        }
    });
    muted(ui, inherited_bool(global));
    changed
}

/// Boutons de la carte DLSS : remplacer les DLL quand une version plus
/// récente existe ailleurs, revenir aux originales à tout moment. `hint`
/// détaille les remplacements au survol.
fn dlss_actions(
    ui: &mut egui::Ui,
    configurations: &mut configurations::Shelf,
    appid: u32,
    state: &lapinux_core::model::DlssState,
    hint: &str,
) {
    ui.horizontal(|ui| {
        let hint = if state.blocked {
            t("graphics.dlss_blocked")
        } else {
            hint
        };
        let update = ui
            .add_enabled(
                !state.available.is_empty() && !state.blocked,
                egui::Button::new(t("graphics.dlss_update")),
            )
            .on_hover_text(hint);
        if update.clicked() {
            configurations.dlss_update(ui.ctx(), appid);
        }
        if ui
            .add_enabled(
                state.updated,
                egui::Button::new(t("graphics.dlss_restore")),
            )
            .clicked()
        {
            configurations.dlss_restore(ui.ctx(), appid);
        }
    });
}

/// Avertissement affiché quand un outil coupé par un anti-triche est sur le
/// point d'être rallumé : le second clic vaut confirmation.
fn protected_confirm(ui: &mut egui::Ui, reason: &str, armed: &mut Option<String>) {
    ui.horizontal_wrapped(|ui| {
        ui.label(
            egui::RichText::new(t_args(
                "game_settings.anticheat_confirm",
                &[("check", reason)],
            ))
            .size(11.0)
            .color(WARN),
        );
        if ui
            .small_button(t("game_settings.anticheat_cancel"))
            .clicked()
        {
            *armed = None;
        }
    });
}

/// Un outil coupé par un anti-triche ne se rallume pas d'un clic : le premier
/// arme l'avertissement, le second applique. Rend vrai quand le changement
/// peut s'appliquer.
fn allow_protected_change(
    protected_by: Option<&str>,
    turning_on: bool,
    armed: &mut Option<String>,
    tag: &str,
) -> bool {
    if !turning_on || protected_by.is_none() || armed.as_deref() == Some(tag) {
        return true;
    }
    *armed = Some(tag.to_string());
    false
}

/// Interrupteur par jeu dont le défaut (`default`) vient du cœur : il vaut
/// déjà « coupé » sur un jeu protégé. Seul l'avertissement avant réactivation
/// est ajouté ici.
fn protected_bool(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut Option<bool>,
    default: bool,
    protected_by: Option<&str>,
    armed: &mut Option<String>,
    tag: &str,
) -> bool {
    let mut changed = false;
    settings_row(ui, label, "", |ui| {
        let mut current = value.unwrap_or(default);
        if switch(ui, &mut current).changed()
            && allow_protected_change(protected_by, current, armed, tag)
        {
            *value = Some(current);
            *armed = None;
            changed = true;
        }
        if value.is_some()
            && ui
                .small_button(t("game_settings.reset_default"))
                .clicked()
        {
            *value = None;
            *armed = None;
            changed = true;
        }
    });
    match protected_by {
        Some(reason) if armed.as_deref() == Some(tag) => {
            protected_confirm(ui, reason, armed);
        }
        Some(reason) if value.unwrap_or(default) => {
            ui.label(
                egui::RichText::new(t_args(
                    "game_settings.anticheat_enabled",
                    &[("check", reason)],
                ))
                .size(11.0)
                .color(WARN),
            );
        }
        Some(reason) => muted(
            ui,
            t_args(
                "game_settings.anticheat_default_off",
                &[("check", reason)],
            ),
        ),
        None => muted(ui, inherited_bool(default)),
    }
    changed
}

/// Curseur par jeu avec valeur héritée nommée et retour au défaut.
fn game_override_slider<T: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut Option<T>,
    global: T,
    range: std::ops::RangeInclusive<T>,
    suffix: &str,
) -> bool {
    let mut changed = false;
    let mut current = value.unwrap_or(global);
    settings_row(ui, label, "", |ui| {
        if settings_slider(ui, &mut current, range, suffix).changed() {
            *value = Some(current);
            changed = true;
        }
        if value.is_some()
            && ui
                .small_button(t("game_settings.reset_default"))
                .clicked()
        {
            *value = None;
            changed = true;
        }
    });
    muted(
        ui,
        t_args(
            "game_settings.default_global",
            &[(
                "value",
                &format!("{}{suffix}", settings_slider_text(global)),
            )],
        ),
    );
    changed
}

fn settings_slider_text<T: egui::emath::Numeric>(value: T) -> String {
    let value = value.to_f64();
    if (value - value.round()).abs() < 0.005 {
        format!("{}", value.round() as i64)
    } else {
        format!("{value:.2}")
    }
}

/// Choix textuel par jeu (profil, résolution, encodeur, codec) : le libellé
/// hérité rappelle la valeur globale, et une entrée dédiée la rétablit.
fn game_override_choice(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash,
    label: &str,
    value: &mut Option<String>,
    global_label: &str,
    choices: &[(&str, &str)],
) -> bool {
    let mut changed = false;
    let inherited = format!(
        "{} · {}",
        t("graphics.inherit"),
        global_label
    );
    let selected = value
        .as_deref()
        .and_then(|current| {
            choices
                .iter()
                .find(|(key, _)| *key == current)
                .map(|(_, label)| (*label).to_string())
        })
        .unwrap_or_else(|| inherited.clone());
    settings_row(ui, label, "", |ui| {
        egui::ComboBox::from_id_salt(id)
            .width(180.0)
            .selected_text(selected.clone())
            .show_ui(ui, |ui| {
                changed |= ui
                    .selectable_value(value, None, inherited.clone())
                    .changed();
                for (key, label) in choices {
                    changed |= ui
                        .selectable_value(value, Some((*key).to_string()), *label)
                        .changed();
                }
            });
        if value.is_some()
            && ui
                .small_button(t("game_settings.reset_default"))
                .clicked()
        {
            *value = None;
            changed = true;
        }
    });
    muted(
        ui,
        t_args("game_settings.default_global", &[("value", global_label)]),
    );
    changed
}

/// Versions mobiles d'un jeu de boutique, dans l'ordre publié par la
/// boutique. Elles servent au menu et aux badges, jamais à l'installation PC.
fn store_mobile_versions(game: &lapinux_core::stores::StoreGame) -> Vec<GameVersion> {
    let mut versions: Vec<GameVersion> = Vec::new();
    for platform in &game.mobile_platforms {
        let (id, label) = if platform.eq_ignore_ascii_case("android") {
            ("android", "Android")
        } else if platform.eq_ignore_ascii_case("ios") {
            ("ios", "iOS")
        } else if platform.eq_ignore_ascii_case("mobile") {
            ("mobile", "Mobile")
        } else {
            continue;
        };
        if versions.iter().any(|version| version.id == id) {
            continue;
        }
        versions.push(GameVersion {
            id,
            label,
            size: None,
        });
    }
    versions
}

impl LapinuxApp {
    /// Résout les photos des membres de la famille au moment de dessiner la
    /// fiche : le bouton et son menu partagent la même texture.
    fn load_family_avatars(&mut self, ctx: &egui::Context, buttons: &mut CardButtons) {
        let CardButtons::Download { family, .. } = buttons else {
            return;
        };
        for member in family.iter_mut() {
            if member.texture.is_some() {
                continue;
            }
            // La fiche publique du membre, lue quand la session ne donne que
            // son identifiant, fournit sa photo comme son nom.
            member.texture = self.profile.family_avatar(member.steam_id);
            if member.texture.is_none() {
                let key = format!("steam:{}", member.steam_id);
                member.texture = self.friend_avatar(ctx, &key, &member.avatar);
            }
        }
    }

    /// Fiche d'un jeu du disque : la ligne compacte (identité, succès, rail)
    /// puis, si elle est dépliée, le corps détaillé.
    fn detail_sheet_body(&mut self, ui: &mut egui::Ui, appid: u32) {
        // La bande de tags annonce l'état du cloud : sa lecture part d'ici,
        // même fiche repliée.
        self.poll_cloud(ui.ctx(), appid);
        if !self.games.iter().any(|game| game.appid == appid) {
            // Un jeu de boutique absent du disque a sa propre fiche ; le
            // catalogue Steam ne le connaît pas.
            if self.stores.cover_of(appid).is_some() {
                self.store_sheet(ui, appid);
            } else {
                self.catalog_sheet(ui, appid);
            }
            return;
        }
        let Some(index) = self.games.iter().position(|game| game.appid == appid) else {
            self.selected = None;
            return;
        };
        let game = self.games[index].clone();
        if game.store == lapinux_core::stores::Store::Steam && !game.native {
            self.poll_steam_bridge(ui.ctx(), appid);
        }
        if self.sheet_tab_for != Some(appid) {
            self.sheet_tab = SheetTab::General;
            self.sheet_tab_for = Some(appid);
        }
        let steam_entry = self.catalog.as_ref().and_then(|catalog| {
            catalog
                .entries
                .iter()
                .find(|entry| entry.appid == game.appid)
                .cloned()
        });
        let catalog_family = steam_entry.as_ref().map(|entry| entry.family);
        let self_owns = self.family_self_owns(game.appid) || catalog_family == Some(false);
        let family_available =
            self.family_shared_by_other(game.appid) || catalog_family == Some(true);
        // Le catalogue peut encore marquer « famille » un jeu que le compte
        // possède : la liste du groupe tranche en faveur de la licence perso.
        let steam_share = catalog_family.map(|family| family && !self_owns);
        let steam_details = steam_entry
            .as_ref()
            .map(|entry| entry.details.clone())
            .unwrap_or_default();
        let installed_build = game.build_id.trim().parse::<u64>().ok();
        let update_build = steam_details
            .build_id
            .zip(installed_build)
            .filter(|(latest, build)| latest > build)
            .map(|(latest, _)| latest);

        let expanded = self.detail_expanded;
        let dlc = steam_entry
            .as_ref()
            .map(|entry| entry.dlc.clone())
            .unwrap_or_default();
        // Les faits (âge, développeur, éditeur, prix, sortie) forment une
        // bande de tags sous tous les encarts, sur toute la largeur de la
        // fiche : ils ne prennent la place d'aucune colonne.
        let facts = sheet_facts(&steam_details, self.gameinfo.info(), self.cloud.get(&appid));
        let strip = if facts.is_empty() {
            0.0
        } else {
            SHEET_TAGS_HEIGHT + SHEET_TAGS_GAP
        };
        let area = ui.max_rect();
        let content = egui::Rect::from_min_size(
            area.min,
            egui::vec2(area.width(), (area.height() - strip).max(120.0)),
        );
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(content), |ui| {
            ui.set_width(content.width());
            ui.set_clip_rect(content.intersect(ui.clip_rect()));
            self.local_compact_row(
                ui,
                &game,
                steam_share,
                self_owns,
                family_available,
                &steam_details,
                &dlc,
                update_build,
                SHEET_HEADER_HEIGHT,
                expanded,
            );
            if !expanded {
                return;
            }
            ui.add_space(8.0);
            let show_configurations = self.configurations.has_entries(game.appid);
            let steam_game = game.store == lapinux_core::stores::Store::Steam;
            let show_steam_input = steam_game;
            if steam_game {
                self.workshop
                    .ensure(ui.ctx(), &self.client, game.appid, &game.name);
            }
            // Un atelier local peut exister sans que PICS le signale : le
            // dossier de contenu décide, pas seulement le drapeau.
            let show_workshop = steam_game
                && workshop::workshop_tab_visible(
                    steam_details.workshop,
                    &self.workshop,
                    game.appid,
                );
            if !show_configurations && self.sheet_tab == SheetTab::Configurations {
                self.sheet_tab = SheetTab::General;
            }
            if !show_steam_input
                && matches!(self.sheet_tab, SheetTab::SteamInput | SheetTab::Workshop)
            {
                self.sheet_tab = SheetTab::General;
            }
            if !show_workshop && self.sheet_tab == SheetTab::Workshop {
                self.sheet_tab = SheetTab::General;
            }
            if let Some(tab) = sheet_tab_bar(
                ui,
                game.appid,
                self.sheet_tab,
                show_configurations,
                show_steam_input,
                show_workshop,
            ) {
                self.sheet_tab = tab;
            }
            ui.add_space(8.0);
            match self.sheet_tab {
                SheetTab::General => {
                    self.local_sheet_details(ui, &game, &steam_details, update_build);
                }
                SheetTab::Configurations => {
                    let mut changed = false;
                    Self::sheet_tab_column_with(
                        ui,
                        ("sheet-configurations", game.appid),
                        980.0,
                        |ui| {
                            changed = self.configurations.page(ui, &game);
                        },
                    );
                    if changed {
                        self.refresh_async(ui.ctx(), false);
                    }
                }
                SheetTab::Saves => self.saves_tab(ui, &game),
                SheetTab::SteamInput => {
                    Self::sheet_workshop_column(ui, ("sheet-steam-input", game.appid), |ui| {
                        self.steaminput.ensure(ui.ctx(), &self.client, game.appid);
                        self.steaminput.page(ui, game.appid);
                    });
                }
                SheetTab::Workshop => {
                    Self::sheet_workshop_column(ui, ("sheet-workshop", game.appid), |ui| {
                        self.workshop
                            .ensure(ui.ctx(), &self.client, game.appid, &game.name);
                        self.workshop.page_view(ui, game.appid, &game.name);
                        self.take_workshop_webview(ui.ctx());
                    });
                }
                SheetTab::Details => {
                    self.local_sheet_details_tab(ui, &game, &steam_details);
                }
            }
        });
        if !facts.is_empty() {
            let tags = egui::Rect::from_min_size(
                egui::pos2(area.min.x, content.bottom() + SHEET_TAGS_GAP),
                egui::vec2(area.width(), SHEET_TAGS_HEIGHT),
            );
            ui.allocate_new_ui(egui::UiBuilder::new().max_rect(tags), |ui| {
                ui.set_width(tags.width());
                sheet_tags(ui, &facts);
            });
        }
    }

    /// Corps détaillé, sous la ligne compacte : les mises à jour à gauche, la
    /// description au centre, les fonctionnalités et les actualités à droite.
    /// Sous 860 px de fiche, les encarts s'empilent.
    #[allow(clippy::too_many_arguments)]
    fn local_sheet_details(
        &mut self,
        ui: &mut egui::Ui,
        game: &Game,
        steam_details: &SteamDetails,
        _update_build: Option<u64>,
    ) {
        // Un jeu de boutique installé se présente comme dans sa fiche de
        // boutique : même résumé, mêmes annonces, mêmes captures. Sans ce
        // détour, la fiche demandait une fiche Steam à Epic, qui ne la publie
        // pas — et l'encart restait sur un message d'erreur.
        let is_store = game.store != lapinux_core::stores::Store::Steam;
        let mut content = None;
        let mut waiting = false;
        let mut error = None;
        let mut site = String::new();
        if is_store {
            self.store_info.poll(ui.ctx());
            self.store_info.ensure(ui.ctx(), &self.client, game.appid);
            waiting = self.store_info.waiting(game.appid);
            error = self.store_info.error.get(&game.appid).cloned();
            if let Some(details) = self.store_info.details(game.appid) {
                site = details.site.clone();
            }
            content = Some(crate::gameinfo::SheetContent::from_store_details(
                self.store_info.details(game.appid),
            ));
        }
        egui::ScrollArea::vertical()
            .id_salt(("game-sheet", game.appid))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let width = ui.available_width();
                let (actus, center, media) = sheet_body_columns(width);
                if actus <= 0.0 {
                    // Une seule colonne : la fiche et son « À propos » d'abord,
                    // les annonces et leurs versions ensuite, puis les médias.
                    self.local_sheet_main(
                        ui,
                        game,
                        steam_details,
                        content.as_ref(),
                        &site,
                        waiting,
                        error.as_deref(),
                    );
                    ui.add_space(14.0);
                    let news = if is_store {
                        self.gameinfo
                            .news_page(ui, content.as_ref(), waiting, self.bunny.as_ref())
                    } else {
                        self.gameinfo
                            .news_embed(ui, game.appid, self.bunny.as_ref())
                    };
                    if news {
                        ui.add_space(14.0);
                    }
                    if is_store {
                        self.gameinfo.media_page(
                            ui,
                            content.as_ref(),
                            &site,
                            t("game_sheet.open_store_page"),
                            width,
                        );
                    } else {
                        self.gameinfo.media_embed(ui, game.appid, width);
                    }
                    return;
                }
                // Les annonces ouvrent la ligne, à gauche de la fiche ; les
                // images ferment à droite ; l'entretien suit les annonces
                // quand le jeu est installé.
                ui.horizontal_top(|ui| {
                    let gap = sheet_column_gap(ui.spacing().item_spacing.x);
                    ui.allocate_ui_with_layout(
                        egui::vec2(actus, 0.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_width(actus);
                            let news = if is_store {
                                self.gameinfo.news_page(
                                    ui,
                                    content.as_ref(),
                                    waiting,
                                    self.bunny.as_ref(),
                                )
                            } else {
                                self.gameinfo
                                    .news_embed(ui, game.appid, self.bunny.as_ref())
                            };
                            if news {
                                ui.add_space(14.0);
                            }
                        },
                    );
                    ui.add_space(gap);
                    ui.allocate_ui_with_layout(
                        egui::vec2(center, 0.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_width(center);
                            self.local_sheet_main(
                                ui,
                                game,
                                steam_details,
                                content.as_ref(),
                                &site,
                                waiting,
                                error.as_deref(),
                            );
                        },
                    );
                    if media > 0.0 {
                        ui.add_space(gap);
                        ui.allocate_ui_with_layout(
                            egui::vec2(media, 0.0),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                ui.set_width(media);
                                if is_store {
                                    self.gameinfo.media_page(
                                        ui,
                                        content.as_ref(),
                                        &site,
                                        t("game_sheet.open_store_page"),
                                        media,
                                    );
                                } else {
                                    self.gameinfo.media_embed(ui, game.appid, media);
                                }
                            },
                        );
                    }
                });
            });
    }

    /// Boutons de la fiche, en haut à gauche : fermer, puis déplier ou
    /// replier. Ils restent à l'écart du contenu, quel que soit le jeu.
    fn sheet_controls(&mut self, ui: &mut egui::Ui, height: f32, expanded: bool) {
        ui.allocate_ui_with_layout(
            egui::vec2(SHEET_CONTROLS_WIDTH, height),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_width(SHEET_CONTROLS_WIDTH);
                ui.spacing_mut().item_spacing.y = 2.0;
                if sheet_button(ui, SheetButton::Close)
                    .on_hover_text(t("game_sheet.close"))
                    .clicked()
                {
                    self.selected = None;
                }
                let hint = if expanded {
                    t("ui.sheet_collapse")
                } else {
                    t("game_sheet.expand")
                };
                if sheet_button(ui, SheetButton::Fold(!expanded))
                    .on_hover_text(hint)
                    .clicked()
                {
                    self.detail_expanded = !expanded;
                }
            },
        );
    }

    /// Ligne compacte de la fiche : encart d'identité, succès Steam, puis le
    /// rail des informations et de l'entretien.
    #[allow(clippy::too_many_arguments)]
    fn local_compact_row(
        &mut self,
        ui: &mut egui::Ui,
        game: &Game,
        steam_share: Option<bool>,
        owned: bool,
        family_available: bool,
        steam_details: &SteamDetails,
        dlc: &[SteamDlc],
        update_build: Option<u64>,
        height: f32,
        expanded: bool,
    ) {
        // Un jeu de boutique garde la jaquette que la boutique publie.
        let store_cover = self.stores.cover_of(game.appid);
        let texture = self.covers.get(
            ui.ctx(),
            &self.paths,
            game.appid,
            &game.name,
            store_cover.as_ref().map(|(store, _)| *store),
            store_cover
                .as_ref()
                .map(|(_, url)| url.as_str())
                .unwrap_or_default(),
            CoverTier::Sheet,
        );
        let has_custom = custom_cover_path(&self.paths, game.appid).is_file();
        let install_dir = game.installed.then(|| game.install_dir());
        let width = ui.available_width();
        let columns = compact_columns(width);
        let (identity, identity_gap, middle) =
            (columns.identity, columns.identity_gap, columns.middle);
        // Le temps de jeu et le poids sur le disque se lisent dans l'encart,
        // jeu installé ou non : les heures viennent du compte, le poids du
        // manifeste local. La fiche Steam est demandée même repliée, sinon un
        // jeu absent du disque resterait sans aucune heure — sauf pour un jeu
        // de boutique, qui se lit chez sa boutique.
        let is_store = game.store != lapinux_core::stores::Store::Steam;
        if is_store {
            self.store_info.poll(ui.ctx());
            self.store_info.ensure(ui.ctx(), &self.client, game.appid);
        } else {
            self.gameinfo.ensure(ui.ctx(), &self.client, game.appid);
        }
        // Le catalogue de configurations se lit à l'ouverture de la fiche :
        // son onglet propose les correctifs et les traductions du jeu.
        self.configurations.ensure(&self.client, game.appid);
        let played = played_label(
            self.play_minutes(game.appid, game.installed.then_some(game.playtime_secs)),
        );
        let played_hint = playtime_tooltip(
            self.gameinfo
                .info()
                .filter(|info| info.appid == game.appid)
                .map(|info| &info.account),
            game.installed.then_some(game.playtime_secs),
        );
        // Installé : ce que le disque occupe vraiment. Absent : ce que Steam
        // annonce pour la version retenue, c'est-à-dire la place qu'il prendra.
        let versions = game_versions(steam_details);
        let chosen = self.version_index(game.appid, &versions);
        let size = (if game.installed && game.size_on_disk > 0 {
            Some(game.size_on_disk)
        } else {
            versions
                .get(chosen)
                .and_then(|version| published_install_bytes(steam_details, version.id))
        })
        .map(lapinux_core::util::format_bytes)
        .unwrap_or_default();
        // Les succès se lisent par la session Steam enregistrée : c'est elle
        // que le daemon utilise, avec la seule session enregistrée.
        let session = self
            .steam_status
            .as_ref()
            .is_some_and(|status| status.token_saved);
        let show_badges = self.config.library.show_badges;
        let badge_scale = self.config.library.badge_scale;
        let mut choose_cover = false;
        let mut reset_cover = false;
        let mut open_folder: Option<PathBuf> = None;
        // Ce que les icônes ne disent pas : l'infobulle porte l'appid, l'état
        // d'installation et le type de compte.
        let state = if !game.installed {
            t("game_sheet.not_installed")
        } else if game.native {
            t("game_sheet.installed_native")
        } else {
            t("game_sheet.installed_proton")
        };
        let account = match steam_share {
            Some(true) => t("game_sheet.family_shared"),
            Some(false) => t("game_sheet.personal_account"),
            None => t("game_sheet.local_game"),
        };
        let detail = format!("{state} · {account} · appid {}", game.appid);
        let cloud = if is_store {
            // La capacité de la boutique ne dit pas que ce jeu a le cloud :
            // seule sa réponse pour ce titre compte.
            self.store_info
                .details(game.appid)
                .and_then(|details| details.cloud)
                .unwrap_or(false)
        } else {
            cloud_saves(self.gameinfo.info().filter(|info| info.appid == game.appid))
        };
        // La protection ne se lit que sur un jeu installé : hors du disque,
        // la fiche n'a pas de dossier à analyser et n'invente rien.
        let drm_line = if !game.installed {
            None
        } else if is_store {
            let report = install_dir
                .as_deref()
                .and_then(|dir| self.drm_report(game.appid, dir));
            report
                .as_ref()
                .and_then(|report| store_drm_sentence(game.store, Some(report)))
        } else {
            local_drm_sentence(game.drm).map(str::to_string)
        };
        let mut buttons = self.card_buttons(
            game.appid,
            game.installed,
            steam_share,
            owned,
            family_available,
            steam_details,
            dlc,
            update_build.is_some(),
        );
        self.load_family_avatars(ui.ctx(), &mut buttons);

        ui.horizontal_top(|ui| {
            // Sans cet espacement implicite, les largeurs de colonnes
            // calculées plus haut sont exactes : sinon chaque `add_space`
            // ajoutait aussi l'écart par défaut et la ligne débordait à
            // droite, décalant la marge de la fiche.
            ui.spacing_mut().item_spacing.x = 0.0;
            self.sheet_controls(ui, height, expanded);
            ui.add_space(SHEET_CONTROLS_GAP);
            let icon = {
                let found = self.art.get(
                    ui.ctx(),
                    &self.paths,
                    game.appid,
                    steam_details,
                    SteamArt::Icon,
                    &game.name,
                );
                found.or_else(|| square_icon_from_cover(ui.ctx(), &self.paths, game.appid))
            };
            let action = identity_card(
                ui,
                identity,
                height,
                game.appid,
                &game.name,
                &texture,
                &icon,
                has_custom,
                install_dir.as_deref(),
                steam_share,
                family_available,
                // La marque de la boutique ne remplace l'icône du disque que
                // pour un jeu qui n'est pas Steam : un jeu scanné reste lu
                // comme un jeu du disque.
                (game.store != lapinux_core::stores::Store::Steam).then_some(game.store),
                show_badges,
                badge_scale,
                &detail,
                &played,
                &played_hint,
                &size,
                cloud,
                drm_line.as_deref(),
                &buttons,
                self.uninstall_armed(game.appid),
            );
            choose_cover = action.choose_cover;
            reset_cover = action.reset_cover;
            open_folder = action.open_folder;
            if action.play {
                self.launch(game.appid, false);
            }
            if action.install {
                let selected = self.selected_dlc(game.appid);
                self.install_game(game.appid, selected);
            }
            if let Some(version) = action.version {
                self.version_choice.insert(game.appid, version.to_string());
            }
            if let Some(owner) = action.family_owner {
                self.family_choice.insert(game.appid, owner);
            }
            if action.settings {
                self.game_settings = Some(game.appid);
                self.game_settings_tab = GameSettingsTab::Launch;
            }
            if action.uninstall {
                self.uninstall_modal = Some(game.appid);
                self.uninstall_error = None;
            }
            if action.contents {
                self.dlc_modal = Some(game.appid);
            }
            if let Some(job) = action.cancel {
                self.cancel_job(&job);
            }
            // L'espace libre entre l'encart et les succès porte le résumé du
            // jeu et ses captures, dans les deux états de la fiche.
            // La ligne ne change pas quand on déplie : le résumé et les
            // succès restent au-dessus de la ligne du dépliage, identiques.
            // Le même écart de chaque côté du résumé : les trois cartes de la
            // ligne sont espacées pareil.
            let summary = (identity_gap - 36.0).max(0.0);
            if summary >= 240.0 {
                // La hauteur de l'encart d'identité, pas celle de la ligne :
                // les trois cartes finissent sur la même ligne.
                ui.add_space(18.0);
                let bunny = self.bunny.clone();
                if is_store {
                    // Le même encart « Résumé », rempli par la boutique : sa
                    // présentation courte et ses captures.
                    let details = self.store_info.details(game.appid);
                    let content = crate::gameinfo::SheetContent::from_store_details(details);
                    let waiting = self.store_info.waiting(game.appid);
                    let error = self.store_info.error.get(&game.appid).cloned();
                    self.gameinfo.summary_page(
                        ui,
                        details.map(|_| &content),
                        waiting,
                        error.as_deref(),
                        summary,
                        action.height.max(80.0),
                        bunny.as_ref(),
                    );
                } else {
                    self.gameinfo.summary_embed(
                        ui,
                        game.appid,
                        summary,
                        action.height.max(80.0),
                        bunny.as_ref(),
                    );
                }
                ui.add_space(18.0);
            } else {
                ui.add_space(identity_gap);
            }

            ui.allocate_ui_with_layout(
                egui::vec2(middle, height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(middle);
                    // Repliée, la colonne des succès descend jusqu'à la
                    // hauteur des autres cartes de la ligne : la place libre
                    // sous la bande porte les succès mis en avant. Dépliée,
                    // elle se contente de sa bande.
                    let bunny = self.bunny.clone();
                    let accent = self.downloads.accent(ui.ctx(), &self.paths, game.appid);
                    ui.ctx().data_mut(|data| {
                        data.insert_temp(egui::Id::new(("game-accent", game.appid)), accent);
                    });
                    self.achievements.block(
                        ui,
                        &game.name,
                        game.appid,
                        steam_share.is_some(),
                        session,
                        &self.client,
                        middle,
                        action.height.max(80.0),
                        bunny.as_ref(),
                        // Un jeu de boutique lit ses succès chez elle, sans
                        // session Steam.
                        game.store,
                        accent,
                    );
                },
            );
        });

        if reset_cover {
            self.restore_automatic_cover(game.appid);
        }
        if choose_cover {
            let ctx = ui.ctx().clone();
            self.open_cover_picker(game.appid, game.name.clone(), &ctx);
        }
        if let Some(path) = open_folder {
            self.open_folder(&path);
        }
    }

    /// Demande l'état cloud d'un jeu, une fois par fiche : la lecture passe par
    /// le daemon, qui tient la session Steam partagée. Sans session, la réponse
    /// revient vide et la fiche n'affiche pas de ligne.
    pub(super) fn poll_cloud(&mut self, ctx: &egui::Context, appid: u32) {
        // Restauration en cours : son résultat remplace la réponse gardée.
        let restored = self.cloud_restore.as_ref().map(|(_, rx)| rx.try_recv());
        match restored {
            Some(Ok(result)) => {
                let done = self.cloud_restore.take().map(|(id, _)| id);
                if let Some(done) = done {
                    // La copie locale a changé : la carte relira l'état du cloud.
                    self.cloud.remove(&done);
                    self.cloud_asked.remove(&done);
                }
                match result {
                    Ok(message) => self.notify(message),
                    Err(error) => self.notify(t_args(
                        "game_sheet.cloud_saves_error",
                        &[("error", &error)],
                    )),
                }
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                if let Some((id, _)) = self.cloud_restore.take() {
                    self.cloud.remove(&id);
                    self.cloud_asked.remove(&id);
                }
                self.notify(t("game_sheet.cloud_saves_interrupted"));
            }
            Some(Err(mpsc::TryRecvError::Empty)) => {
                // La restauration réveille l'écran de son côté.
                ctx.request_repaint_after(Duration::from_secs(1));
            }
            None => {}
        }
        // Envoi en cours : même bilan, même relecture ensuite.
        let sent = self.cloud_upload.as_ref().map(|(_, rx)| rx.try_recv());
        match sent {
            Some(Ok(result)) => {
                let done = self.cloud_upload.take().map(|(id, _)| id);
                if let Some(done) = done {
                    self.cloud.remove(&done);
                    self.cloud_asked.remove(&done);
                }
                match result {
                    Ok(message) => self.notify(message),
                    Err(error) => self.notify(t_args(
                        "game_sheet.cloud_upload_error",
                        &[("error", &error)],
                    )),
                }
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                if let Some((id, _)) = self.cloud_upload.take() {
                    self.cloud.remove(&id);
                    self.cloud_asked.remove(&id);
                }
                self.notify(t("game_sheet.cloud_upload_interrupted"));
            }
            Some(Err(mpsc::TryRecvError::Empty)) => {
                ctx.request_repaint_after(Duration::from_secs(1));
            }
            None => {}
        }
        let received = self.cloud_fetch.as_ref().map(|rx| rx.try_recv());
        match received {
            Some(Ok((loaded, report))) => {
                self.cloud_fetch = None;
                self.cloud_fetch_appid = None;
                self.cloud.insert(loaded, report);
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.cloud_fetch = None;
                if let Some(id) = self.cloud_fetch_appid.take() {
                    self.cloud.insert(
                        id,
                        lapinux_core::model::CloudReport {
                            error: Some(t("ui.read_interrupted").into()),
                            ..Default::default()
                        },
                    );
                }
            }
            Some(Err(mpsc::TryRecvError::Empty)) => {
                // La lecture peut prendre une seconde : son fil réveille la fiche.
                ctx.request_repaint_after(Duration::from_secs(1));
            }
            None => {}
        }
        if self.cloud.contains_key(&appid)
            || self.cloud_fetch.is_some()
            || !self.cloud_asked.insert(appid)
        {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.cloud_fetch = Some(rx);
        self.cloud_fetch_appid = Some(appid);
        let client = self.client.detached();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let report = client
                .request_with_timeout(&Req::SteamCloudFresh { appid }, Duration::from_secs(45))
                .and_then(|envelope| envelope.into_data::<lapinux_core::model::CloudReport>())
                .unwrap_or_else(|error| lapinux_core::model::CloudReport {
                    error: Some(error.to_string()),
                    ..Default::default()
                });
            if tx.send((appid, report)).is_ok() {
                ctx.request_repaint();
            }
        });
    }

    /// Lit une fois par jeu l'état du pont Steamworks. Une session interrompue
    /// laisse le DLL de Lapinux et sa sauvegarde : la fiche doit pouvoir le
    /// signaler avant que le joueur ne relance le jeu hors de Lapinux.
    fn poll_steam_bridge(&mut self, ctx: &egui::Context, appid: u32) {
        let mut wake = false;
        if self
            .steam_bridge_fetch
            .as_ref()
            .is_some_and(|(id, _)| *id == appid)
        {
            if let Some((_, rx)) = self.steam_bridge_fetch.take() {
                match rx.try_recv() {
                    Ok(Ok(paths)) => {
                        self.steam_bridge_status.insert(appid, paths);
                    }
                    Ok(Err(_)) => {
                        // Un daemon plus ancien ne connaît pas la requête :
                        // l'absence de diagnostic ne doit pas bloquer la fiche.
                        self.steam_bridge_status.entry(appid).or_default();
                    }
                    Err(mpsc::TryRecvError::Empty) => {
                        self.steam_bridge_fetch = Some((appid, rx));
                        ctx.request_repaint_after(Duration::from_secs(1));
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        self.steam_bridge_status.entry(appid).or_default();
                    }
                }
            }
        }
        if self
            .steam_bridge_repair
            .as_ref()
            .is_some_and(|(id, _)| *id == appid)
        {
            if let Some((_, rx)) = self.steam_bridge_repair.take() {
                match rx.try_recv() {
                    Ok(Ok(paths)) => {
                        self.steam_bridge_status.insert(appid, Vec::new());
                        self.notify(t_fmt(
                            "game_sheet.steam_bridge_repaired",
                            &[paths.len().to_string()],
                        ));
                        wake = true;
                    }
                    Ok(Err(error)) => {
                        self.notify(t_args(
                            "game_sheet.steam_bridge_error",
                            &[("error", &error)],
                        ));
                    }
                    Err(mpsc::TryRecvError::Empty) => {
                        self.steam_bridge_repair = Some((appid, rx));
                        ctx.request_repaint_after(Duration::from_millis(250));
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        self.notify(t("game_sheet.steam_bridge_interrupted"));
                    }
                }
            }
        }
        if !self.steam_bridge_status.contains_key(&appid)
            && self.steam_bridge_fetch.is_none()
            && self.steam_bridge_repair.is_none()
        {
            // Marque la demande avant le fil : la fiche est repeinte souvent.
            self.steam_bridge_status.insert(appid, Vec::new());
            let (tx, rx) = mpsc::channel();
            self.steam_bridge_fetch = Some((appid, rx));
            let client = self.client.detached();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let result = client
                    .request_with_timeout(
                        &Req::SteamBridgeStatus { appid },
                        Duration::from_secs(20),
                    )
                    .and_then(|envelope| envelope.into_data::<Vec<String>>())
                    .map_err(|error| error.to_string());
                let _ = tx.send(result);
                ctx.request_repaint();
            });
            wake = true;
        }
        if wake {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }

    fn repair_steam_bridge(&mut self, ctx: &egui::Context, appid: u32) {
        if self.steam_bridge_repair.is_some() {
            return;
        }
        self.steam_bridge_armed = None;
        let (tx, rx) = mpsc::channel();
        self.steam_bridge_repair = Some((appid, rx));
        let client = self.client.detached();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = client
                .request_with_timeout(&Req::RepairSteamBridge { appid }, Duration::from_secs(60))
                .and_then(|envelope| envelope.into_data::<Vec<String>>())
                .map_err(|error| error.to_string());
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }

    pub(super) fn resolve_cloud(
        &mut self,
        ctx: &egui::Context,
        appid: u32,
        choice: lapinux_core::ipc::SteamCloudChoice,
        expected: lapinux_core::model::CloudSaves,
        store_game: bool,
    ) {
        if self.cloud_restore.is_some() || self.cloud_upload.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.cloud_restore = Some((appid, rx));
        let client = self.client.detached();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let request = if store_game {
                Req::ResolveStoreCloud {
                    appid,
                    choice,
                    expected,
                }
            } else {
                Req::ResolveSteamCloud {
                    appid,
                    choice,
                    expected,
                }
            };
            let result = client
                .request_with_timeout(&request, Duration::from_secs(600))
                .and_then(|envelope| envelope.into_data::<serde_json::Value>())
                .map(|value| {
                    t_fmt(
                        "game_sheet.verified_copies",
                        &[value
                            .get("backup")
                            .and_then(|v| v.as_str())
                            .unwrap_or(t("game_sheet.path_unavailable"))
                            .to_string()],
                    )
                })
                .map_err(|error| error.to_string());
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }

    /// Récupère les sauvegardes cloud du jeu : le daemon les télécharge par le
    /// protocole et les écrit dans le préfixe, l'interface attend le bilan.
    pub(super) fn restore_cloud(&mut self, ctx: &egui::Context, appid: u32) {
        if self.cloud_restore.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.cloud_restore = Some((appid, rx));
        self.cloud_restore_armed = None;
        let client = self.client.detached();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = match client
                .request_with_timeout(&Req::CloudRestore { appid }, Duration::from_secs(180))
            {
                Ok(envelope) => match envelope.into_data::<serde_json::Value>() {
                    Ok(value) => {
                        let files = value.get("files").and_then(|v| v.as_u64()).unwrap_or(0);
                        let bytes = value.get("bytes").and_then(|v| v.as_u64()).unwrap_or(0);
                        Ok(t_fmt(
                            "game_sheet.saves_fetched",
                            &[
                                files.to_string(),
                                lapinux_core::util::format_bytes(bytes).to_string(),
                            ],
                        ))
                    }
                    Err(error) => Err(error.to_string()),
                },
                Err(error) => Err(error.to_string()),
            };
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }

    /// Envoie les sauvegardes du jeu vers le cloud du compte : la requête
    /// part en tâche de fond, la carte des réglages attend le bilan.
    pub(super) fn upload_cloud(&mut self, ctx: &egui::Context, appid: u32) {
        if self.cloud_upload.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.cloud_upload = Some((appid, rx));
        self.cloud_upload_armed = None;
        let client = self.client.detached();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = match client
                .request_with_timeout(&Req::CloudUpload { appid }, Duration::from_secs(180))
            {
                Ok(envelope) => match envelope.into_data::<serde_json::Value>() {
                    Ok(value) => {
                        let files = value.get("files").and_then(|v| v.as_u64()).unwrap_or(0);
                        let bytes = value.get("bytes").and_then(|v| v.as_u64()).unwrap_or(0);
                        if files == 0 {
                            Ok(t("game_sheet.no_newer_save").to_string())
                        } else {
                            Ok(t_fmt(
                                "game_sheet.saves_sent",
                                &[
                                    files.to_string(),
                                    lapinux_core::util::format_bytes(bytes).to_string(),
                                ],
                            ))
                        }
                    }
                    Err(error) => Err(error.to_string()),
                },
                Err(error) => Err(error.to_string()),
            };
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }

    /// Corps de la fiche : description et détails techniques.
    #[allow(clippy::too_many_arguments)]
    fn local_sheet_main(
        &mut self,
        ui: &mut egui::Ui,
        game: &Game,
        steam_details: &SteamDetails,
        store: Option<&crate::gameinfo::SheetContent>,
        store_site: &str,
        waiting: bool,
        error: Option<&str>,
    ) {
        // La description ouvre la colonne du milieu, entre les deux colonnes
        // latérales : la bannière d'en-tête ne prend plus de place au-dessus.
        let described = match store {
            Some(content) => {
                self.gameinfo
                    .description_page(ui, Some(content), ("sheet", game.appid))
            }
            None => self.gameinfo.description(ui, game.appid),
        };
        if described {
            ui.add_space(12.0);
        }
        // La boutique n'a pas répondu : l'encart vide dit pourquoi plutôt que
        // de laisser la colonne muette.
        if let Some(store) = store {
            let _ = (store, store_site);
            if waiting {
                muted(ui, t("game_sheet.reading_store"));
                ui.add_space(10.0);
            } else if let Some(error) = error {
                muted(
                    ui,
                    t_args(
                        "game_sheet.unavailable_error",
                        &[("error", &error.to_string())],
                    ),
                );
                ui.add_space(10.0);
            }
        }
        if !game.installed {
            if let Some(bytes) = public_download_bytes(steam_details) {
                muted(
                    ui,
                    t_fmt(
                        "game_sheet.to_download_no_client",
                        &[lapinux_core::util::format_bytes(bytes).to_string()],
                    ),
                );
                ui.add_space(10.0);
            }
        }
    }

    /// Formulaire des réglages d'un jeu : il vit dans la fiche dépliée et dans
    /// la fenêtre qu'ouvre l'engrenage de l'encart. Les réglages sont groupés —
    /// lancement, affichage, langue, environnement — et s'enregistrent dès
    /// qu'un contrôle change.
    pub(super) fn game_settings_form(&mut self, ui: &mut egui::Ui, index: usize) {
        // Chaque onglet relit la copie courante : un changement enregistré
        // dans l'onglet précédent ne doit pas être écrasé par une copie
        // capturée à l'ouverture de la fenêtre.
        let game = self.games[index].clone();
        self.configurations.ensure(&self.client, game.appid);
        let mut settings = game.settings.clone();
        let mut save = false;
        let proton_names: Vec<String> = self.protons.iter().map(|p| p.name.clone()).collect();
        let global_language = self.config.general.game_language.clone();
        let tab = self.game_settings_tab;
        // Le relevé sert toutes les cartes : un anti-triche change les défauts
        // du pont, des injecteurs et de l'enregistrement, quel que soit
        // l'onglet ouvert. La première lecture lance la recherche, les
        // suivantes la relisent.
        let tech = if game.installed && !game.native {
            self.configurations.game_tech(ui.ctx(), &game)
        } else {
            None
        };
        // Un jeu protégé demande un avertissement avant de rallumer un outil
        // qui s'injecte dans son processus.
        let protected_by: Option<String> = tech
            .as_ref()
            .and_then(|result| result.as_ref().ok())
            .filter(|tech| tech.protected())
            .map(|tech| tech.anti_cheat());
        // La règle des défauts est celle du cœur, la même que celle du
        // lancement : sur un jeu protégé, les outils restent coupés. La fiche
        // lit cette copie au lieu de recalculer un défaut de son côté.
        let defaults = {
            let mut effective = settings.clone();
            if protected_by.is_some() {
                effective.apply_protected_defaults(game.native);
            }
            effective
        };
        // Les bibliothèques DLSS se relisent moins souvent que le moteur :
        // elles ne bougent qu'après une mise à jour ou un retour. La fiche
        // graphique les remplace, l'onglet Informations les compare à ce que
        // le préfixe Proton fournit.
        let dlss = if game.installed
            && matches!(tab, GameSettingsTab::Graphics | GameSettingsTab::Info)
        {
            self.configurations.game_dlss(ui.ctx(), &game)
        } else {
            None
        };

        if tab == GameSettingsTab::Launch {
            settings_panel(ui, |ui| {
                if !game.native {
                    field_label(ui, "Proton");
                    let mut proton_choice =
                        settings.proton.clone().unwrap_or_else(|| "auto".into());
                    // Ce que le choix « comme les réglages » donne réellement, sans
                    // le préfixe de la liste.
                    let auto_label_value = match self.config.launch.proton.trim() {
                        "" | "auto" => self.automatic_proton_label(),
                        name => name.to_string(),
                    };
                    // Le choix « par défaut » dit quel Proton sert vraiment : celui
                    // des réglages généraux, nommé, et non un « automatique » muet.
                    let auto_label = t_args(
                        "game_sheet.like_global_value",
                        &[("auto_label_value", &auto_label_value.to_string())],
                    );
                    egui::ComboBox::from_id_salt(("proton_choice", game.appid))
                        .width(ui.available_width())
                        .selected_text(if proton_choice == "auto" {
                            auto_label.clone()
                        } else {
                            proton_choice.clone()
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut proton_choice,
                                "auto".to_string(),
                                auto_label.clone(),
                            );
                            for name in &proton_names {
                                ui.selectable_value(&mut proton_choice, name.clone(), name);
                            }
                        });
                    let chosen = (proton_choice != "auto").then_some(proton_choice);
                    if chosen != settings.proton {
                        settings.proton = chosen;
                        save = true;
                    }
                    ui.add_space(10.0);
                }

                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if switch(ui, &mut settings.performance).changed() {
                        save = true;
                    }
                    ui.label(t("ui.sheet_performance"));
                });
                muted(ui, t("ui.sheet_performance_hint"));
            });
        }

        if tab == GameSettingsTab::InGame {
            settings_panel(ui, |ui| {
                // Profil manettes propre à ce jeu : le daemon l'applique au
                // lancement et active le remapping evdev.
                let pad_profiles = lapinux_input::list_profiles(&self.paths.manettes_dir());
                let global_label = format!("global · {}", self.config.input.default_profile);
                field_label(ui, t("game_sheet.pad_profile"));
                let mut pad_choice = settings
                    .input_profile
                    .clone()
                    .unwrap_or_else(|| global_label.clone());
                egui::ComboBox::from_id_salt(("game_pad_profile", game.appid))
                    .width(ui.available_width())
                    .selected_text(pad_choice.clone())
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut pad_choice, global_label.clone(), &global_label);
                        for profile in &pad_profiles {
                            ui.selectable_value(
                                &mut pad_choice,
                                profile.name.clone(),
                                &profile.name,
                            );
                        }
                    });
                let pad_selection = (pad_choice != global_label).then_some(pad_choice);
                if pad_selection != settings.input_profile {
                    settings.input_profile = pad_selection;
                    save = true;
                }

                // Le catalogue communautaire vit dans l’onglet Steam Input de
                // la fiche : ce raccourci y emmène directement depuis les
                // réglages du jeu, sans chercher l’onglet à côté.
                ui.add_space(6.0);
                if ui
                    .button(t("game_settings.steam_input_choose"))
                    .on_hover_text(t("game_settings.steam_input_hint"))
                    .clicked()
                {
                    self.selected = Some(game.appid);
                    self.sheet_tab = SheetTab::SteamInput;
                    self.sheet_tab_for = Some(game.appid);
                    self.settings_open = false;
                }

                ui.add_space(10.0);
                save |= game_override_bool(
                    ui,
                    t("settings.general.capture_label"),
                    &mut settings.grab_devices,
                    self.config.input.grab_devices,
                );
                ui.add_space(10.0);
                field_label(ui, t("game_settings.hud_details"));
                settings_row(
                    ui,
                    t("game_sheet.hud_custom"),
                    t("game_sheet.hud_custom_desc"),
                    |ui| {
                        let selected = settings
                            .hud_style
                            .as_deref()
                            .filter(|value| !value.trim().is_empty())
                            .and_then(|id| {
                                self.config
                                    .capture
                                    .hud_custom
                                    .iter()
                                    .find(|style| style.id == id)
                            })
                            .map(|style| style.name.clone())
                            .unwrap_or_else(|| {
                                t("graphics.inherit").to_string()
                            });
                        egui::ComboBox::from_id_salt(("game-hud-style", game.appid))
                            .width(250.0)
                            .selected_text(selected)
                            .show_ui(ui, |ui| {
                                if ui
                                    .selectable_value(
                                        &mut settings.hud_style,
                                        None,
                                        t("graphics.inherit"),
                                    )
                                    .changed()
                                {
                                    save = true;
                                }
                                for style in &self.config.capture.hud_custom {
                                    if ui
                                        .selectable_value(
                                            &mut settings.hud_style,
                                            Some(style.id.clone()),
                                            style.name.clone(),
                                        )
                                        .changed()
                                    {
                                        save = true;
                                    }
                                }
                            });
                    },
                );
                ui.add_space(10.0);
                save |= protected_bool(
                    ui,
                    t("settings.in_game.hud_visible"),
                    &mut settings.hud_on_screen,
                    defaults
                        .hud_on_screen
                        .unwrap_or(self.config.capture.hud_on_screen),
                    protected_by.as_deref(),
                    &mut self.protect_armed,
                    &format!("{}:hud", game.appid),
                );
                save |= game_override_bool(
                    ui,
                    t("settings.captures.badge_show"),
                    &mut settings.record_badge,
                    self.config.capture.record_badge,
                );
            });
        }

        if tab == GameSettingsTab::Display {
            settings_panel(ui, |ui| {
                save |= game_override_bool(
                    ui,
                    "GameScope",
                    &mut settings.gamescope,
                    self.config.launch.gamescope,
                );
                ui.add_space(8.0);
                settings_row(
                    ui,
                    t("settings.general.fps_limit"),
                    "",
                    |ui| {
                        let mut fps = settings.fps_cap.unwrap_or(self.config.launch.fps_cap);
                        let old_fps = fps;
                        fps_picker(ui, ("game-fps", game.appid), &mut fps);
                        if fps != old_fps {
                            settings.fps_cap = Some(fps);
                            save = true;
                        }
                        if settings.fps_cap.is_some()
                            && ui
                                .small_button(t("graphics.inherit"))
                                .clicked()
                        {
                            settings.fps_cap = None;
                            save = true;
                        }
                    },
                );
                muted(
                    ui,
                    t_args(
                        "game_settings.default_global",
                        &[("value", &fps_text(self.config.launch.fps_cap))],
                    ),
                );

                ui.add_space(10.0);
                // La limite inscrite par le jeu lui-même se lève dans ses
                // fichiers de réglages : c'est la seule façon de dépasser un
                // plafond interne, GameScope ne sachant que le baisser.
                ui.horizontal(|ui| {
                    let mut unlock = settings.unlock_fps.unwrap_or(false);
                    if switch(ui, &mut unlock).changed() {
                        settings.unlock_fps = Some(unlock);
                        save = true;
                    }
                    ui.label(t("game_sheet.unlock_fps"));
                    info_mark(ui, t("game_sheet.unlock_fps_hint"));
                });

                ui.add_space(10.0);
                if game.store == lapinux_core::stores::Store::Steam {
                    // Le pont natif est préchargé sans modifier le jeu ; le pont
                    // Windows remplace temporairement la DLL.
                    let default = defaults.steam_shim.unwrap_or(
                        !game.native && !lapinux_core::model::steam_shim_excluded(game.appid),
                    );
                    save |= protected_bool(
                        ui,
                        t("ui.steamworks_bridge"),
                        &mut settings.steam_shim,
                        default,
                        protected_by.as_deref(),
                        &mut self.protect_armed,
                        &format!("{}:shim", game.appid),
                    );
                    if game.native {
                        muted(ui, t("game_sheet.native_bridge_hint"));
                    } else if settings.steam_shim.is_some() {
                        muted(ui, t("game_sheet.achievements_hint"));
                    }
                    let orphans = self
                        .steam_bridge_status
                        .get(&game.appid)
                        .map(Vec::len)
                        .unwrap_or(0);
                    if !game.native && orphans > 0 {
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new(t_fmt(
                                "game_sheet.steam_bridge_orphaned",
                                &[orphans.to_string()],
                            ))
                            .size(11.0)
                            .color(WARN),
                        );
                        muted(ui, t("game_sheet.steam_bridge_hint"));
                        let busy = self
                            .steam_bridge_repair
                            .as_ref()
                            .is_some_and(|(id, _)| *id == game.appid);
                        let armed = self.steam_bridge_armed == Some(game.appid);
                        let label = if armed {
                            t("game_sheet.steam_bridge_confirm")
                        } else {
                            t("game_sheet.steam_bridge_repair")
                        };
                        if ui.add_enabled(!busy, egui::Button::new(label)).clicked() {
                            if armed {
                                self.repair_steam_bridge(ui.ctx(), game.appid);
                            } else {
                                self.steam_bridge_armed = Some(game.appid);
                            }
                        }
                        if busy {
                            muted(ui, t("game_sheet.steam_bridge_busy"));
                        }
                    }
                }

                ui.add_space(10.0);
                settings_row(
                    ui,
                    t("settings.general.resolution"),
                    t("settings.general.resolution_desc"),
                    |ui| {
                        let mut width = settings.width.unwrap_or(self.config.launch.width);
                        let mut height = settings.height.unwrap_or(self.config.launch.height);
                        // La disposition va de droite à gauche : la hauteur est
                        // posée d'abord pour que « largeur × hauteur » se lise
                        // dans l'ordre habituel.
                        let height_response = ui.add_sized(
                            [72.0, 24.0],
                            egui::DragValue::new(&mut height).range(240..=4320),
                        );
                        ui.label("×");
                        let width_response = ui.add_sized(
                            [72.0, 24.0],
                            egui::DragValue::new(&mut width).range(320..=7680),
                        );
                        if height_response.changed() || width_response.changed() {
                            settings.width = Some(width);
                            settings.height = Some(height);
                            save = true;
                        }
                        if ui
                            .small_button(t("game_settings.resolution_auto"))
                            .clicked()
                        {
                            settings.width = None;
                            settings.height = None;
                            save = true;
                        }
                    },
                );
                save |= game_override_bool(
                    ui,
                    t("game_settings.hdr"),
                    &mut settings.hdr,
                    self.config.launch.hdr,
                );
                save |= game_override_bool(
                    ui,
                    t("game_settings.adaptive_sync"),
                    &mut settings.adaptive_sync,
                    self.config.launch.adaptive_sync,
                );
            });
        }

        if tab == GameSettingsTab::Graphics {
            // Un jeu protégé se lance sans injection : l'avertissement est
            // posé avant les cartes, pas caché dans une infobulle.
            if let Some(reason) = protected_by.as_deref() {
                ui.label(
                    egui::RichText::new(t_args(
                        "game_settings.anticheat_banner",
                        &[("check", reason)],
                    ))
                    .size(11.0)
                    .color(WARN),
                );
                // Les injecteurs installés (OptiScaler, ReShade, cnc-ddraw) et
                // les DLL DLSS remplacées suivent la même règle que le reste :
                // coupés sauf accord explicite.
                save |= protected_bool(
                    ui,
                    t("graphics.protected_injections"),
                    &mut settings.allow_protected_injections,
                    defaults.allow_protected_injections.unwrap_or(false),
                    protected_by.as_deref(),
                    &mut self.protect_armed,
                    &format!("{}:injections", game.appid),
                );
                muted(ui, t("graphics.protected_injections_hint"));
                ui.add_space(6.0);
            }
            settings_card(
                ui,
                t("graphics.frame_title"),
                t("graphics.frame_hint"),
                |ui| {
                    use lapinux_core::config::FrameGeneration;
                    field_label(ui, t("graphics.frame_title"));
                    // La génération d'images passe par une couche Vulkan
                    // injectée : sur un jeu protégé, elle est coupée par défaut
                    // et se rallume par un avertissement.
                    let afmf_tag = format!("{}:afmf", game.appid);
                    let effective = defaults
                        .frame_generation
                        .unwrap_or(self.config.graphics.frame_generation)
                        .supported();
                    let selected = settings_tabs::frame_generation_label(effective);
                    let mut choice = settings.frame_generation;
                    egui::ComboBox::from_id_salt(("game-frame-generation", game.appid))
                        .width(ui.available_width())
                        .selected_text(selected)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut choice,
                                None,
                                t("graphics.inherit"),
                            );
                            for value in [FrameGeneration::Off, FrameGeneration::AfmfLinux] {
                                ui.selectable_value(
                                    &mut choice,
                                    Some(value),
                                    settings_tabs::frame_generation_label(value),
                                );
                            }
                        });
                    if choice != settings.frame_generation {
                        let turning_on = choice.is_some_and(|value| value != FrameGeneration::Off);
                        if allow_protected_change(
                            protected_by.as_deref(),
                            turning_on,
                            &mut self.protect_armed,
                            &afmf_tag,
                        ) {
                            settings.frame_generation = choice;
                            self.protect_armed = None;
                            save = true;
                        }
                    }
                    if let Some(reason) = protected_by.as_deref() {
                        if self.protect_armed.as_deref() == Some(afmf_tag.as_str()) {
                            protected_confirm(ui, reason, &mut self.protect_armed);
                        }
                    }
                    muted(ui, t("graphics.anticheat"));
                    muted(ui, t("graphics.frame_launch_only"));
                    muted(
                        ui,
                        t_args(
                            "game_settings.effective_engine",
                            &[("engine", settings_tabs::frame_generation_label(effective))],
                        ),
                    );
                    muted(ui, settings_tabs::frame_generation_hint(effective));
                    if effective == FrameGeneration::AfmfLinux {
                        field_label(ui, t("graphics.afmf_version"));
                        let selected = settings
                            .afmf_version
                            .as_deref()
                            .unwrap_or_else(|| t("graphics.inherit"));
                        egui::ComboBox::from_id_salt(("game-afmf-version", game.appid))
                            .width(ui.available_width())
                            .selected_text(selected)
                            .show_ui(ui, |ui| {
                                if ui
                                    .selectable_value(
                                        &mut settings.afmf_version,
                                        None,
                                        t("graphics.inherit"),
                                    )
                                    .changed()
                                {
                                    save = true;
                                }
                                let local = settings_tabs::afmf_local_installed(&self.paths);
                                for version in lapinux_core::config::afmf_versions(local) {
                                    if ui
                                        .selectable_value(
                                            &mut settings.afmf_version,
                                            Some(version.to_string()),
                                            if version == lapinux_core::config::AFMF_LOCAL_VERSION {
                                                t("graphics.afmf_version_local")
                                            } else {
                                                version
                                            },
                                        )
                                        .changed()
                                    {
                                        save = true;
                                    }
                                }
                            });
                        muted(ui, t("graphics.afmf_version_game_hint"));
                        use lapinux_core::config::{AfmfPerformanceMode, AfmfSearchMode};
                        field_label(ui, t("graphics.afmf_performance"));
                        egui::ComboBox::from_id_salt(("game-afmf-performance", game.appid))
                            .width(ui.available_width())
                            .selected_text(
                                settings
                                    .afmf_performance_mode
                                    .map(settings_tabs::afmf_performance_label)
                                    .unwrap_or_else(|| t("graphics.inherit")),
                            )
                            .show_ui(ui, |ui| {
                                save |= ui
                                    .selectable_value(
                                        &mut settings.afmf_performance_mode,
                                        None,
                                        t("graphics.inherit"),
                                    )
                                    .changed();
                                for value in [
                                    AfmfPerformanceMode::Auto,
                                    AfmfPerformanceMode::Quality,
                                    AfmfPerformanceMode::Performance,
                                ] {
                                    save |= ui
                                        .selectable_value(
                                            &mut settings.afmf_performance_mode,
                                            Some(value),
                                            settings_tabs::afmf_performance_label(value),
                                        )
                                        .changed();
                                }
                            });
                        field_label(ui, t("graphics.afmf_search"));
                        egui::ComboBox::from_id_salt(("game-afmf-search", game.appid))
                            .width(ui.available_width())
                            .selected_text(
                                settings
                                    .afmf_search_mode
                                    .map(settings_tabs::afmf_search_label)
                                    .unwrap_or_else(|| t("graphics.inherit")),
                            )
                            .show_ui(ui, |ui| {
                                save |= ui
                                    .selectable_value(
                                        &mut settings.afmf_search_mode,
                                        None,
                                        t("graphics.inherit"),
                                    )
                                    .changed();
                                for value in [
                                    AfmfSearchMode::Auto,
                                    AfmfSearchMode::Standard,
                                    AfmfSearchMode::High,
                                ] {
                                    save |= ui
                                        .selectable_value(
                                            &mut settings.afmf_search_mode,
                                            Some(value),
                                            settings_tabs::afmf_search_label(value),
                                        )
                                        .changed();
                                }
                            });
                        field_label(ui, t("graphics.afmf_multiplier"));
                        egui::ComboBox::from_id_salt(("game-afmf-multiplier", game.appid))
                            .width(ui.available_width())
                            .selected_text(
                                settings
                                    .afmf_multiplier
                                    .map(settings_tabs::afmf_multiplier_label)
                                    .unwrap_or_else(|| {
                                        t("graphics.inherit").to_string()
                                    }),
                            )
                            .show_ui(ui, |ui| {
                                save |= ui
                                    .selectable_value(
                                        &mut settings.afmf_multiplier,
                                        None,
                                        t("graphics.inherit"),
                                    )
                                    .changed();
                                for value in lapinux_core::config::AFMF_MULTIPLIERS {
                                    save |= ui
                                        .selectable_value(
                                            &mut settings.afmf_multiplier,
                                            Some(*value),
                                            settings_tabs::afmf_multiplier_label(*value),
                                        )
                                        .changed();
                                }
                            });
                        muted(ui, t("graphics.afmf_options_hint"));
                    }
                },
            );
            settings_card(
                ui,
                t("graphics.scaling_title"),
                t("graphics.scaling_hint"),
                |ui| {
                    use lapinux_core::config::ScalingFilter;
                    field_label(ui, t("graphics.filter"));
                    let selected = settings
                        .scaling_filter
                        .map(settings_tabs::scaling_filter_label)
                        .unwrap_or_else(|| t("graphics.inherit"));
                    egui::ComboBox::from_id_salt(("game-scaling-filter", game.appid))
                        .width(ui.available_width())
                        .selected_text(selected)
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_value(
                                    &mut settings.scaling_filter,
                                    None,
                                    t("graphics.inherit"),
                                )
                                .changed()
                            {
                                save = true;
                            }
                            for value in [
                                ScalingFilter::Auto,
                                ScalingFilter::Fsr,
                                ScalingFilter::Nis,
                                ScalingFilter::Linear,
                                ScalingFilter::Nearest,
                                ScalingFilter::Pixel,
                            ] {
                                if ui
                                    .selectable_value(
                                        &mut settings.scaling_filter,
                                        Some(value),
                                        settings_tabs::scaling_filter_label(value),
                                    )
                                    .changed()
                                {
                                    save = true;
                                }
                            }
                        });
                    save |= game_override_slider(
                        ui,
                        t("graphics.sharpness"),
                        &mut settings.sharpness,
                        self.config.graphics.sharpness,
                        0..=20,
                        "",
                    );
                },
            );
            settings_card(
                ui,
                t("graphics.effects_title"),
                t("graphics.effects_hint"),
                |ui| {
                    save |= protected_bool(
                        ui,
                        t("graphics.vkbasalt_enable"),
                        &mut settings.vkbasalt,
                        defaults.vkbasalt.unwrap_or(self.config.graphics.vkbasalt),
                        protected_by.as_deref(),
                        &mut self.protect_armed,
                        &format!("{}:vkbasalt", game.appid),
                    );
                    ui.add_space(8.0);
                    field_label(ui, t("graphics.vkbasalt_effect"));
                    let selected = settings
                        .vkbasalt_effect
                        .map(settings_tabs::vkbasalt_effect_label)
                        .unwrap_or_else(|| t("graphics.inherit"));
                    egui::ComboBox::from_id_salt(("game-vkbasalt-effect", game.appid))
                        .width(ui.available_width())
                        .selected_text(selected)
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_value(
                                    &mut settings.vkbasalt_effect,
                                    None,
                                    t("graphics.inherit"),
                                )
                                .changed()
                            {
                                save = true;
                            }
                            use lapinux_core::config::VkBasaltEffect;
                            for value in [
                                VkBasaltEffect::Existing,
                                VkBasaltEffect::Cas,
                                VkBasaltEffect::Fxaa,
                                VkBasaltEffect::Smaa,
                                VkBasaltEffect::Ascii,
                            ] {
                                if ui
                                    .selectable_value(
                                        &mut settings.vkbasalt_effect,
                                        Some(value),
                                        settings_tabs::vkbasalt_effect_label(value),
                                    )
                                    .changed()
                                {
                                    save = true;
                                }
                            }
                        });
                    let about = settings
                        .vkbasalt_effect
                        .or(defaults.vkbasalt_effect)
                        .unwrap_or(self.config.graphics.vkbasalt_effect);
                    muted(
                        ui,
                        format!(
                            "{} {}",
                            t("graphics.vkbasalt_about_title"),
                            settings_tabs::vkbasalt_effect_about(about)
                        ),
                    );
                    muted(ui, t("graphics.vkbasalt_effect_hint"));
                    muted(ui, t("graphics.vkbasalt_runtime"));
                    // Les Protons installés ne remplacent pas les DLL DLSS :
                    // Lapinux le fait lui-même, à partir des bibliothèques
                    // présentes sur la machine, avec retour possible.
                    if show_ngx_option(&game, tech.as_ref()) {
                        ui.add_space(8.0);
                        match dlss.as_ref().map(|result| result.as_ref()) {
                            None => muted(ui, t("graphics.dlss_detecting")),
                            Some(Err(_)) => {
                                muted(ui, t("graphics.dlss_scan_error"))
                            }
                            Some(Ok(state)) => {
                                for library in &state.current {
                                    info_line(ui, &library.name, &library.version_label());
                                }
                                let details: Vec<String> = state
                                    .available
                                    .iter()
                                    .filter_map(|candidate| {
                                        let current = state
                                            .current
                                            .iter()
                                            .find(|library| library.name == candidate.name)?;
                                        Some(format!(
                                            "{} {} → {} ({})",
                                            candidate.name,
                                            current.version_label(),
                                            candidate.version_label(),
                                            candidate.source
                                        ))
                                    })
                                    .collect();
                                dlss_actions(
                                    ui,
                                    &mut self.configurations,
                                    game.appid,
                                    state,
                                    &details.join("\n"),
                                );
                                muted(ui, t("graphics.dlss_hint"));
                            }
                        }
                    } else if game.installed && !game.native {
                        ui.add_space(8.0);
                        let key = match &tech {
                            None => "graphics.dlss_detecting",
                            Some(Err(_)) => "graphics.dlss_scan_error",
                            Some(Ok(_)) => "graphics.dlss_not_detected",
                        };
                        muted(ui, t(key));
                    }
                },
            );
            if game.installed && !game.native {
                settings_card(
                    ui,
                    t("graphics.graphics_tools_title"),
                    t("graphics.graphics_tools_hint"),
                    |ui| {
                        self.configurations.graphics_controls(ui, &game);
                    },
                );
            }
        }

        if tab == GameSettingsTab::Captures {
            settings_card(
                ui,
                t("ui.settings_captures"),
                t("settings.captures.hint_video"),
                |ui| {
                    save |= protected_bool(
                        ui,
                        t("settings.captures.record_on_launch"),
                        &mut settings.record_on_launch,
                        defaults
                            .record_on_launch
                            .unwrap_or(self.config.capture.record_on_launch),
                        protected_by.as_deref(),
                        &mut self.protect_armed,
                        &format!("{}:record", game.appid),
                    );
                    save |= game_override_bool(
                        ui,
                        t("settings.captures.stop_on_close"),
                        &mut settings.stop_on_exit,
                        self.config.capture.stop_on_exit,
                    );
                    ui.add_space(8.0);
                    save |= game_override_slider(
                        ui,
                        t("ui.capture_fps"),
                        &mut settings.video_fps,
                        self.config.capture.video_fps,
                        15..=240,
                        " i/s",
                    );
                    save |= game_override_choice(
                        ui,
                        ("game-video-profile", game.appid),
                        t("settings.captures.profile"),
                        &mut settings.video_profile,
                        profile_label(&self.config.capture.video_profile),
                        &[
                            (
                                "performance",
                                t("settings.captures.profile_performance"),
                            ),
                            (
                                "balanced",
                                t("settings.captures.balanced"),
                            ),
                            (
                                "quality",
                                t("settings.captures.quality"),
                            ),
                        ],
                    );
                    save |= game_override_choice(
                        ui,
                        ("game-video-resolution", game.appid),
                        t("settings.captures.resolution"),
                        &mut settings.video_resolution,
                        resolution_label(&self.config.capture.video_resolution),
                        &[
                            ("720p", t("settings.captures.720p")),
                            ("1080p", t("settings.captures.1080p")),
                            ("1440p", t("settings.captures.1440p")),
                            ("native", t("settings.captures.native")),
                        ],
                    );
                    save |= game_override_choice(
                        ui,
                        ("game-video-encoder", game.appid),
                        t("ui.capture_encoder"),
                        &mut settings.video_encoder,
                        encoder_label(&self.config.capture.video_encoder),
                        &[
                            (
                                "auto",
                                t("settings.captures.encoder_auto"),
                            ),
                            (
                                "nvenc",
                                t("settings.captures.encoder_nvenc"),
                            ),
                            (
                                "software",
                                t("settings.captures.encoder_software"),
                            ),
                        ],
                    );
                    save |= game_override_choice(
                        ui,
                        ("game-video-codec", game.appid),
                        t("ui.capture_codec"),
                        &mut settings.video_codec,
                        codec_label(&self.config.capture.video_codec),
                        &[
                            (
                                "auto",
                                t("settings.captures.codec_auto"),
                            ),
                            ("h264", "H.264"),
                            ("hevc", "H.265 · HEVC"),
                            ("av1", "AV1"),
                        ],
                    );
                    ui.add_space(8.0);
                    save |= game_override_bool(
                        ui,
                        t("settings.captures.record_game_audio"),
                        &mut settings.record_audio,
                        self.config.capture.record_audio,
                    );
                    save |= game_override_bool(
                        ui,
                        t("settings.captures.record_microphone"),
                        &mut settings.record_microphone,
                        self.config.capture.record_microphone,
                    );
                },
            );
        }

        if tab == GameSettingsTab::Saves {
            settings_card(
                ui,
                t("settings.saves.local_copies"),
                t("settings.saves.subtitle"),
                |ui| {
                    save |= game_override_bool(
                        ui,
                        t("settings.saves.take_on_close"),
                        &mut settings.saves_auto,
                        self.config.general.saves_auto,
                    );
                    save |= game_override_bool(
                        ui,
                        t("settings.saves.skip_cloud"),
                        &mut settings.saves_skip_cloud,
                        self.config.general.saves_skip_cloud,
                    );
                    save |= game_override_bool(
                        ui,
                        t("settings.saves.relaunch"),
                        &mut settings.saves_relaunch,
                        self.config.general.saves_relaunch,
                    );
                },
            );
        }

        if tab == GameSettingsTab::Language {
            settings_panel(ui, |ui| {
                field_label(ui, t("game_sheet.game_language"));
                // « Comme les réglages » nomme la langue qui servira vraiment :
                // celle des réglages généraux, sinon celle du client Steam ou
                // du bureau.
                let default_label = match lapinux_core::lang::label(&global_language) {
                    Some(label) => t_args(
                        "game_sheet.like_global_label",
                        &[("label", &label.to_string())],
                    ),
                    None => match self.detected_language() {
                        Some(label) => t_args(
                            "game_sheet.like_global_label",
                            &[("label", &label.to_string())],
                        ),
                        None => t("game_sheet.like_global").to_string(),
                    },
                };
                let selected = settings
                    .language
                    .as_deref()
                    .and_then(lapinux_core::lang::label)
                    .map(str::to_string)
                    .unwrap_or_else(|| default_label.clone());
                let mut choice = settings.language.clone().unwrap_or_default();
                egui::ComboBox::from_id_salt(("game-language", game.appid))
                    .width(ui.available_width())
                    .selected_text(selected)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut choice, String::new(), default_label.clone());
                        ui.separator();
                        for language in lapinux_core::lang::GAME_LANGUAGES {
                            ui.selectable_value(
                                &mut choice,
                                language.code.to_string(),
                                language.label,
                            );
                        }
                    });
                let chosen = (!choice.is_empty()).then_some(choice);
                if chosen != settings.language {
                    settings.language = chosen;
                    save = true;
                }
                muted(ui, t("game_sheet.language_hint"));
            });
        }

        if tab == GameSettingsTab::Info {
            let install_dir = game.install_dir();
            let install_text = install_dir.display().to_string();
            let executable = game
                .exe
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| t("game_settings.info_none").to_string());
            let size = if game.size_on_disk > 0 {
                lapinux_core::util::format_bytes(game.size_on_disk)
            } else {
                t("game_settings.info_none").to_string()
            };
            let playtime = if game.playtime_secs > 0 {
                lapinux_core::util::format_duration(game.playtime_secs)
            } else {
                t("game_settings.info_none").to_string()
            };
            let status = if game.installed {
                t("game_settings.info_installed")
            } else {
                t("game_sheet.not_installed")
            };
            let appid = game.appid.to_string();
            let store = game.store.label();
            let installed = game.installed;

            settings_panel(ui, |ui| {
                info_line(
                    ui,
                    t("game_settings.info_appid"),
                    &appid,
                );
                info_line(
                    ui,
                    t("game_settings.info_status"),
                    status,
                );
                info_line(ui, t("game_settings.info_store"), store);
                info_line(ui, t("game_settings.info_size"), &size);
                info_line(
                    ui,
                    t("game_settings.info_playtime"),
                    &playtime,
                );
                info_line(
                    ui,
                    t("game_settings.info_folder"),
                    &install_text,
                );
                info_line(
                    ui,
                    t("game_settings.info_executable"),
                    &executable,
                );
                // Même principe que la détection DLSS de l'onglet Graphismes :
                // un mot pendant la recherche, un mot discret si elle échoue.
                if game.installed && !game.native {
                    match &tech {
                        None => muted(ui, t("game_settings.info_scanning")),
                        Some(Err(_)) => {
                            muted(ui, t("game_settings.info_scan_error"))
                        }
                        Some(Ok(found)) => {
                            let none = t("game_settings.info_none");
                            info_line(
                                ui,
                                t("game_settings.info_engine"),
                                found.engine.map(|engine| engine.label()).unwrap_or(none),
                            );
                            info_line(
                                ui,
                                t("game_settings.info_upscaling"),
                                &found.upscaling(),
                            );
                            info_line(
                                ui,
                                t("game_settings.info_anti_cheat"),
                                &found.anti_cheat(),
                            );
                        }
                    }
                }

                // Version DLSS du jeu face à celle que Proton lui fournit
                // dans son préfixe, et à celles des autres jeux : une DLL plus
                // ancienne se remplace d'ici, sans Proton ni NGX.
                if game.installed {
                    if let Some(Ok(state)) = dlss.as_ref() {
                        let dlss_version = state
                            .current
                            .iter()
                            .find(|library| library.name == "nvngx_dlss.dll")
                            .or_else(|| state.current.first());
                        if let Some(library) = dlss_version {
                            info_line(
                                ui,
                                t("game_settings.info_dlss"),
                                &library.version_label(),
                            );
                            // La comparaison porte sur la même DLL : une
                            // `dlssd` plus récente ne dit rien du `dlss` du jeu.
                            let candidate = state
                                .available
                                .iter()
                                .find(|other| other.name == library.name);
                            match candidate {
                                Some(candidate) => {
                                    ui.label(
                                        egui::RichText::new(t_args(
                                            "game_settings.dlss_outdated",
                                            &[
                                                ("current", &library.version_label()),
                                                ("latest", &candidate.version_label()),
                                                ("source", &candidate.source),
                                            ],
                                        ))
                                        .size(11.0)
                                        .color(WARN),
                                    );
                                    dlss_actions(
                                        ui,
                                        &mut self.configurations,
                                        game.appid,
                                        state,
                                        &t_args(
                                            "game_settings.dlss_change",
                                            &[
                                                ("current", &library.version_label()),
                                                ("latest", &candidate.version_label()),
                                                ("source", &candidate.source),
                                            ],
                                        ),
                                    );
                                }
                                None => muted(
                                    ui,
                                    t_args(
                                        "game_settings.dlss_up_to_date",
                                        &[("version", &library.version_label())],
                                    ),
                                ),
                            }
                            muted(ui, t("game_settings.dlss_ngx_note"));
                        }
                    }
                }

                ui.add_space(10.0);
                field_label(ui, t("game_settings.info_command"));
                ui.horizontal(|ui| {
                    if ui
                        .button(t("game_settings.info_open_folder"))
                        .clicked()
                    {
                        self.open_folder(&install_dir);
                    }
                    if ui
                        .add_enabled(
                            installed,
                            egui::Button::new(t("game_settings.info_dry_run")),
                        )
                        .clicked()
                    {
                        self.launch(game.appid, true);
                    }
                });
                muted(ui, t("game_settings.info_dry_run_hint"));
                if let Some(plan) = self
                    .dry_run_info
                    .clone()
                    .filter(|plan| plan.appid == game.appid)
                {
                    ui.add_space(10.0);
                    field_label(ui, t("game_settings.dry_run_title"));
                    muted(ui, plan.layers.clone());
                    ui.add_space(4.0);
                    let mut command = plan.command.clone();
                    egui::ScrollArea::vertical()
                        .id_salt(("sheet-dry-run-command", game.appid))
                        .max_height(220.0)
                        .show(ui, |ui| {
                            ui.add(
                                egui::TextEdit::multiline(&mut command)
                                    .desired_rows(6)
                                    .desired_width(f32::INFINITY)
                                    .font(egui::TextStyle::Monospace),
                            );
                        });
                    if ui
                        .button(t("game_settings.dry_run_copy"))
                        .clicked()
                    {
                        ui.output_mut(|out| out.copied_text = plan.command.clone());
                    }
                }
            });
        }

        if tab == GameSettingsTab::Compatibility {
            settings_card(
                ui,
                "Environnement",
                t("game_sheet.launch_env"),
                |ui| {
                    let mut env_text = settings
                        .env
                        .iter()
                        .map(|(key, value)| format!("{key}={value}"))
                        .collect::<Vec<_>>()
                        .join("\n");
                    if ui
                        .add(
                            egui::TextEdit::multiline(&mut env_text)
                                .desired_rows(2)
                                .hint_text("DXVK_HUD=0"),
                        )
                        .changed()
                    {
                        settings.env = env_text
                            .lines()
                            .filter_map(|line| {
                                let (key, value) = line.split_once('=')?;
                                let key = key.trim();
                                if key.is_empty() {
                                    None
                                } else {
                                    Some((key.to_string(), value.trim().to_string()))
                                }
                            })
                            .collect();
                        save = true;
                    }
                    muted(ui, t("game_sheet.env_hint"));
                },
            );

            if !game.native {
                settings_card(
                    ui,
                    t("compatibility.title"),
                    t("compatibility.hint"),
                    |ui| {
                        let c = &mut settings.compatibility;
                        for (field, label, hint) in [
                            (
                                &mut c.wined3d,
                                "compatibility.wined3d",
                                "compatibility.wined3d_hint",
                            ),
                            (
                                &mut c.dxvk_d3d8,
                                "compatibility.d3d8",
                                "compatibility.d3d8_hint",
                            ),
                            (
                                &mut c.old_gl_string,
                                "compatibility.oldgl",
                                "compatibility.oldgl_hint",
                            ),
                            (
                                &mut c.no_fsync,
                                "compatibility.fsync",
                                "compatibility.fsync_hint",
                            ),
                            (
                                &mut c.no_esync,
                                "compatibility.esync",
                                "compatibility.esync_hint",
                            ),
                            (
                                &mut c.no_ntsync,
                                "compatibility.ntsync",
                                "compatibility.ntsync_hint",
                            ),
                            (
                                &mut c.d3d9_mode_count,
                                "compatibility.modes",
                                "compatibility.modes_hint",
                            ),
                            (
                                &mut c.d3d9_cube_depth,
                                "compatibility.cube",
                                "compatibility.cube_hint",
                            ),
                            (
                                &mut c.d3d9_sampler_type,
                                "compatibility.sampler",
                                "compatibility.sampler_hint",
                            ),
                            (
                                &mut c.vkd3d_no_dxr,
                                "compatibility.nodxr",
                                "compatibility.nodxr_hint",
                            ),
                        ] {
                            let changed = ui
                                .horizontal(|ui| {
                                    let response = switch(ui, field);
                                    ui.label(t(label));
                                    response
                                })
                                .inner
                                .changed();
                            if changed {
                                save = true;
                            }
                            muted(ui, t(hint));
                        }
                        if c.wined3d && c.dxvk_d3d8 {
                            ui.colored_label(WARN, t("compatibility.conflict"));
                        }
                        ui.hyperlink_to(t("compatibility.documentation"), "https://github.com/ValveSoftware/Proton/blob/proton_11.0/README.md#runtime-config-options");
                    },
                );
            }

            // Certains moteurs lisent mal la carte qu'on leur annonce et se
            // brident eux-mêmes : DXVK transmet ces valeurs au jeu. Le catalogue
            // de configurations en propose pour les jeux connus ; ces champs
            // restent pour ajuster le reste à la main.
            if !game.native {
                settings_card(
                    ui,
                    "DXVK",
                    t("game_sheet.dxvk_hint"),
                    |ui| {
                        field_label(ui, t("game_sheet.vram_shared"));
                        let mut shared = settings
                            .dxvk
                            .shared_mb
                            .map(|mb| mb.to_string())
                            .unwrap_or_default();
                        if ui
                            .add(
                                egui::TextEdit::singleline(&mut shared)
                                    .hint_text(t("game_sheet.like_gpu"))
                                    .desired_width(120.0),
                            )
                            .changed()
                        {
                            settings.dxvk.shared_mb = memory_from_text(&shared);
                            save = true;
                        }

                        ui.add_space(8.0);
                        field_label(ui, t("game_sheet.vram_dedicated"));
                        let mut dedicated = settings
                            .dxvk
                            .device_mb
                            .map(|mb| mb.to_string())
                            .unwrap_or_default();
                        if ui
                            .add(
                                egui::TextEdit::singleline(&mut dedicated)
                                    .hint_text(t("game_sheet.like_gpu"))
                                    .desired_width(120.0),
                            )
                            .changed()
                        {
                            settings.dxvk.device_mb = memory_from_text(&dedicated);
                            save = true;
                        }

                        ui.add_space(8.0);
                        field_label(ui, t("game_sheet.gpu_name"));
                        let mut device = settings.dxvk.device.clone().unwrap_or_default();
                        if ui
                            .add(
                                egui::TextEdit::singleline(&mut device)
                                    .hint_text(t("game_sheet.like_gpu"))
                                    .desired_width(260.0),
                            )
                            .changed()
                        {
                            settings.dxvk.device =
                                (!device.trim().is_empty()).then(|| device.clone());
                            save = true;
                        }

                        muted(ui, t("game_sheet.dxvk_hint"));

                        if let Some(line) = settings.dxvk.config_line() {
                            ui.add_space(10.0);
                            ui.label(
                                egui::RichText::new(format!("DXVK_CONFIG={line}"))
                                    .size(11.0)
                                    .monospace()
                                    .color(TEXT_DIM()),
                            );
                            let manual = settings.env.iter().any(|(key, _)| {
                                key.eq_ignore_ascii_case("DXVK_CONFIG")
                                    || key.eq_ignore_ascii_case("DXVK_CONFIG_FILE")
                            });
                            if manual {
                                ui.label(
                                    egui::RichText::new(t(
                                        "game_sheet.dxvk_conflict",
                                    ))
                                    .size(11.0)
                                    .color(WARN),
                                );
                            }
                            ui.add_space(6.0);
                            if secondary_button(
                                ui,
                                t("game_sheet.clear_settings"),
                            )
                            .clicked()
                            {
                                settings.dxvk = Default::default();
                                save = true;
                            }
                        }
                    },
                );
            }
        }

        // Une autre langue change les dépôts (donc les poids) : la fiche
        // relira ses détails Steam plutôt que de garder les précédents.
        if settings.language != game.settings.language {
            self.gameinfo.forget();
        }
        if self.configurations.take_reload_request() {
            // Le diagnostic a corrigé des réglages dans le daemon : relire la
            // fiche évite de réécrire la copie affichée, désormais périmée.
            self.refresh(false);
            return;
        }
        if save {
            self.games[index].settings = settings.clone();
            self.save_settings(game.appid, settings);
            // La protection anti-triche dépend désormais d'autres valeurs :
            // l'état des injecteurs et des DLL DLSS se relit.
            self.configurations.forget_protection(game.appid);
        }
    }

    /// Fiche d'un jeu possédé sur Steam mais absent du disque : la même ligne
    /// compacte que les autres, et les mêmes onglets — seuls les réglages
    /// d'installation manquent.
    fn catalog_sheet(&mut self, ui: &mut egui::Ui, appid: u32) {
        self.poll_cloud(ui.ctx(), appid);
        if self.sheet_tab_for != Some(appid) {
            self.sheet_tab = SheetTab::General;
            self.sheet_tab_for = Some(appid);
        }
        let Some(entry) = self.catalog.as_ref().and_then(|catalog| {
            catalog
                .entries
                .iter()
                .find(|entry| entry.appid == appid)
                .cloned()
        }) else {
            self.selected = None;
            return;
        };
        // Les configurations viennent du même catalogue que la fiche : savoir
        // s'il en existe décide de l'onglet, avant de le peindre.
        self.configurations.ensure(&self.client, appid);
        let expanded = self.detail_expanded;
        // La bande de tags ferme la fiche, sous tous les encarts.
        let facts = sheet_facts(&entry.details, self.gameinfo.info(), self.cloud.get(&appid));
        let strip = if facts.is_empty() {
            0.0
        } else {
            SHEET_TAGS_HEIGHT + SHEET_TAGS_GAP
        };
        let area = ui.max_rect();
        let content = egui::Rect::from_min_size(
            area.min,
            egui::vec2(area.width(), (area.height() - strip).max(120.0)),
        );
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(content), |ui| {
            ui.set_width(content.width());
            ui.set_clip_rect(content.intersect(ui.clip_rect()));
            self.catalog_compact_row(ui, &entry, SHEET_HEADER_HEIGHT, expanded);
            if !expanded {
                return;
            }
            ui.add_space(8.0);
            let show_configurations = self.configurations.has_entries(entry.appid);
            // Même un jeu de catalogue non installé peut avoir des
            // abonnements : Steam les garde dans `userdata/.../ugc`.
            self.workshop
                .ensure(ui.ctx(), &self.client, entry.appid, &entry.name);
            let show_workshop =
                workshop::workshop_tab_visible(entry.details.workshop, &self.workshop, entry.appid);
            if !show_configurations && self.sheet_tab == SheetTab::Configurations {
                self.sheet_tab = SheetTab::General;
            }
            if !show_workshop && self.sheet_tab == SheetTab::Workshop {
                self.sheet_tab = SheetTab::General;
            }
            if let Some(tab) = sheet_tab_bar(
                ui,
                entry.appid,
                self.sheet_tab,
                show_configurations,
                true,
                show_workshop,
            ) {
                self.sheet_tab = tab;
            }
            ui.add_space(8.0);
            // Le catalogue ne décrit pas un jeu installé : les réglages du
            // disque restent absents, mais les onglets sont ceux des autres
            // fiches.
            let catalog_game = lapinux_core::Game {
                appid: entry.appid,
                name: entry.name.clone(),
                store: lapinux_core::stores::Store::Steam,
                installed: entry.installed,
                ..Default::default()
            };
            match self.sheet_tab {
                SheetTab::General => self.catalog_sheet_body(ui, &entry),
                SheetTab::Configurations => {
                    let mut changed = false;
                    Self::sheet_tab_column_with(
                        ui,
                        ("sheet-configurations", entry.appid),
                        980.0,
                        |ui| {
                            changed = self.configurations.page(ui, &catalog_game);
                        },
                    );
                    if changed {
                        self.refresh_async(ui.ctx(), false);
                    }
                }
                SheetTab::Saves => self.saves_tab(ui, &catalog_game),
                SheetTab::SteamInput => {
                    Self::sheet_workshop_column(ui, ("sheet-steam-input", entry.appid), |ui| {
                        self.steaminput.ensure(ui.ctx(), &self.client, entry.appid);
                        self.steaminput.page(ui, entry.appid);
                    });
                }
                SheetTab::Workshop => {
                    Self::sheet_workshop_column(ui, ("sheet-workshop", entry.appid), |ui| {
                        self.workshop
                            .ensure(ui.ctx(), &self.client, entry.appid, &entry.name);
                        self.workshop.page_view(ui, entry.appid, &entry.name);
                        self.take_workshop_webview(ui.ctx());
                    });
                }
                SheetTab::Details => {
                    self.gameinfo.ensure(ui.ctx(), &self.client, entry.appid);
                    Self::sheet_tab_column(ui, ("sheet-details", entry.appid), |ui| {
                        steam_details_section(
                            ui,
                            &self.paths,
                            &mut self.art,
                            &mut self.covers,
                            entry.appid,
                            &entry.name,
                            &entry.details,
                            None,
                            self.cloud.get(&entry.appid),
                        );
                        ui.add_space(12.0);
                        self.gameinfo.block(ui, &self.client, entry.appid);
                    });
                }
            }
        });
        if !facts.is_empty() {
            let tags = egui::Rect::from_min_size(
                egui::pos2(area.min.x, content.bottom() + SHEET_TAGS_GAP),
                egui::vec2(area.width(), SHEET_TAGS_HEIGHT),
            );
            ui.allocate_new_ui(egui::UiBuilder::new().max_rect(tags), |ui| {
                ui.set_width(tags.width());
                sheet_tags(ui, &facts);
            });
        }
    }

    /// Ce que le dossier installé montre de sa protection : le scan n'est
    /// gardé qu'une fois, un jeu de 150 Go ne se relit pas à chaque image.
    fn drm_report(&mut self, appid: u32, dir: &Path) -> Option<drm::Report> {
        if let Some((seen, report)) = self.drm_reports.get(&appid) {
            if seen == dir {
                return report.clone();
            }
        }
        let report = drm::scan(dir);
        self.drm_reports
            .insert(appid, (dir.to_path_buf(), report.clone()));
        report
    }

    /// Fiche d'un jeu de boutique : la même que Steam — ligne compacte avec
    /// sa jaquette et ses actions, résumé, succès, dépliage et bande de tags.
    /// Seule l'installation change de camp : elle part de la boutique.
    fn store_sheet(&mut self, ui: &mut egui::Ui, appid: u32) {
        let Some(game) = self
            .stores
            .owned_games()
            .into_iter()
            .find(|game| game.appid == appid)
        else {
            self.selected = None;
            return;
        };
        if self.sheet_tab_for != Some(appid) {
            self.sheet_tab = SheetTab::General;
            self.sheet_tab_for = Some(appid);
        }
        let expanded = self.detail_expanded;
        // La fiche d'un jeu de boutique se lit chez la boutique, repliée comme
        // dépliée : le résumé et la bande de tags en ont besoin. La réponse est
        // gardée, elle n'est demandée qu'une fois.
        self.store_info.poll(ui.ctx());
        self.store_info.ensure(ui.ctx(), &self.client, game.appid);
        // Le catalogue nomme aussi les jeux de boutique : ses entrées se
        // lisent avant la barre d'onglets, pour savoir s'il y en a.
        self.configurations.ensure(&self.client, game.appid);
        self.store_sizes.poll();
        self.store_sizes.ensure(ui.ctx(), &self.client, game.appid);
        let details = self.store_info.details(game.appid).cloned();
        let error = self.store_info.error.get(&game.appid).cloned();
        let waiting = self.store_info.waiting(game.appid);
        // Ce que le téléchargement demande, puis la place qu'il occupera.
        let download = self.store_sizes.get(game.appid).or(game.size);
        let installed = self.store_sizes.installed_of(game.appid);
        // Un jeu sur le disque dit sa protection : le dossier est lu une fois,
        // puis gardé — un jeu de 150 Go ne se relit pas à chaque image.
        let install_dir = self
            .games
            .iter()
            .find(|tracked| tracked.appid == game.appid && tracked.installed)
            .map(Game::install_dir);
        let drm = install_dir
            .as_deref()
            .and_then(|dir| self.drm_report(game.appid, dir));
        // La bande de tags ferme la fiche, sous tous les encarts, comme pour
        // un jeu Steam : développeur, éditeur, sortie, boutique.
        let facts = store_facts(
            &game,
            details.as_ref(),
            installed.or(download),
            self.stores.platform.get(&game.appid).map(String::as_str),
            drm.as_ref(),
        );
        let strip = if facts.is_empty() {
            0.0
        } else {
            SHEET_TAGS_HEIGHT + SHEET_TAGS_GAP
        };
        let area = ui.max_rect();
        let content = egui::Rect::from_min_size(
            area.min,
            egui::vec2(area.width(), (area.height() - strip).max(120.0)),
        );
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(content), |ui| {
            ui.set_width(content.width());
            ui.set_clip_rect(content.intersect(ui.clip_rect()));
            self.store_compact_row(
                ui,
                &game,
                details.as_ref(),
                waiting,
                error.as_deref(),
                SHEET_HEADER_HEIGHT,
                expanded,
                drm.as_ref(),
            );
            if !expanded {
                return;
            }
            ui.add_space(8.0);
            let show_configurations = self.configurations.has_entries(game.appid);
            if !show_configurations && self.sheet_tab == SheetTab::Configurations {
                self.sheet_tab = SheetTab::General;
            }
            if matches!(self.sheet_tab, SheetTab::SteamInput | SheetTab::Workshop) {
                self.sheet_tab = SheetTab::General;
            }
            if let Some(tab) = sheet_tab_bar(
                ui,
                game.appid,
                self.sheet_tab,
                show_configurations,
                false,
                false,
            ) {
                self.sheet_tab = tab;
            }
            ui.add_space(8.0);
            let account = self
                .stores
                .view
                .accounts
                .iter()
                .find(|account| account.store == game.store)
                .cloned();
            let bunny = self.bunny.clone();
            // Le catalogue nomme les jeux par leur identifiant de boutique :
            // la fiche d'un jeu de boutique se décrit avec le même type que
            // celle d'un jeu Steam.
            let catalog_game = lapinux_core::Game {
                appid: game.appid,
                name: game.name.clone(),
                store: game.store,
                store_id: game.store_id.clone(),
                installed: game.installed,
                ..Default::default()
            };
            match self.sheet_tab {
                SheetTab::General => {
                    store_sheet_body(
                        ui,
                        &mut self.gameinfo,
                        bunny.as_ref(),
                        &game,
                        details.as_ref(),
                        waiting,
                    );
                }
                SheetTab::Configurations => {
                    let mut changed = false;
                    Self::sheet_tab_column_with(
                        ui,
                        ("sheet-configurations", game.appid),
                        980.0,
                        |ui| {
                            changed = self.configurations.page(ui, &catalog_game);
                        },
                    );
                    if changed {
                        self.refresh_async(ui.ctx(), false);
                    }
                }
                SheetTab::SteamInput | SheetTab::Workshop => {}
                SheetTab::Saves => {
                    store_sheet_saves_tab(
                        ui,
                        &mut self.saves,
                        &self.client,
                        &game,
                        details.as_ref(),
                        self.config.general.saves_relaunch,
                    );
                }
                SheetTab::Details => {
                    let cover_texture = self.covers.get(
                        ui.ctx(),
                        &self.paths,
                        game.appid,
                        &game.name,
                        Some(game.store),
                        &game.cover,
                        CoverTier::Sheet,
                    );
                    store_sheet_details_tab(
                        ui,
                        &mut self.gameinfo.news_art,
                        &mut self.art,
                        &self.paths,
                        cover_texture,
                        &game,
                        account.as_ref(),
                        details.as_ref(),
                        installed.or(download),
                        self.stores.platform.get(&game.appid).map(String::as_str),
                    );
                }
            }
        });
        if !facts.is_empty() {
            let tags = egui::Rect::from_min_size(
                egui::pos2(area.min.x, content.bottom() + SHEET_TAGS_GAP),
                egui::vec2(area.width(), SHEET_TAGS_HEIGHT),
            );
            ui.allocate_new_ui(egui::UiBuilder::new().max_rect(tags), |ui| {
                ui.set_width(tags.width());
                sheet_tags(ui, &facts);
            });
        }
    }

    /// Ligne compacte d'un jeu de boutique : les mêmes colonnes que la fiche
    /// Steam, avec la marque de la boutique comme provenance et son bouton
    /// d'installation à la place du téléchargement Steam.
    #[allow(clippy::too_many_arguments)]
    fn store_compact_row(
        &mut self,
        ui: &mut egui::Ui,
        game: &lapinux_core::stores::StoreGame,
        details: Option<&lapinux_core::stores::StoreDetails>,
        waiting: bool,
        error: Option<&str>,
        height: f32,
        expanded: bool,
        drm: Option<&drm::Report>,
    ) {
        let texture = self.covers.get(
            ui.ctx(),
            &self.paths,
            game.appid,
            &game.name,
            Some(game.store),
            &game.cover,
            CoverTier::Sheet,
        );
        let width = ui.available_width();
        let columns = compact_columns(width);
        let (identity, identity_gap, middle) =
            (columns.identity, columns.identity_gap, columns.middle);
        let show_badges = self.config.library.show_badges;
        let badge_scale = self.config.library.badge_scale;
        let mobile_versions = store_mobile_versions(game);
        let mobile_only = game.platforms.is_empty() && (game.mobile || !mobile_versions.is_empty());
        let mut install = false;
        // Ce que la ligne dit : la boutique, la version publiée et l'état.
        let version = game
            .platforms
            .first()
            .map(|platform| crate::stores::platform_label(platform))
            .unwrap_or_default();
        let mut detail = game.store.label().to_string();
        if mobile_only {
            detail.push_str(" · ");
            detail.push_str(t("game_sheet.mobile_only"));
            detail.push_str(" · ");
            detail.push_str(t("game_sheet.mobile_not_installable"));
        } else if !version.is_empty() {
            detail.push_str(" · ");
            detail.push_str(version);
        }
        detail.push_str(if game.installed {
            t("game_sheet.installed_suffix")
        } else {
            t("game_sheet.not_installed_suffix")
        });
        detail.push_str(&format!(" · appid {}", game.appid));
        // Deux chiffres, deux places : le bouton annonce ce qu'on télécharge,
        // l'encart la place que le jeu prendra une fois installé. Ils viennent
        // tous deux du manifeste de la boutique ; sans lui, l'encart se rabat
        // sur ce que la bibliothèque publie.
        let download = self.store_sizes.get(game.appid).or(game.size);
        let installed = self.store_sizes.installed_of(game.appid);
        let size = installed
            .or(download)
            .map(lapinux_core::util::format_bytes)
            .unwrap_or_default();
        // Les heures de jeu : celles que la boutique publie (Epic les donne),
        // ou le chronomètre de Lapinux pour un jeu installé. Le plus grand des
        // deux, jamais leur somme — la même règle que la bibliothèque.
        let tracked = self
            .games
            .iter()
            .find(|tracked| tracked.appid == game.appid)
            .map(|tracked| tracked.playtime_secs / 60)
            .filter(|minutes| *minutes > 0);
        let minutes = match (game.minutes.filter(|minutes| *minutes > 0), tracked) {
            (Some(published), Some(tracked)) => Some(published.max(tracked)),
            (published, tracked) => published.or(tracked),
        };
        let played = played_label(minutes);
        let played_hint = match (minutes, game.minutes) {
            (Some(minutes), _) if game.minutes.is_some() => format!(
                "{} selon {}.",
                lapinux_core::util::format_duration(minutes.saturating_mul(60)),
                game.store.label()
            ),
            (Some(_), _) => t("game_sheet.measured_time").to_string(),
            (None, _) => String::new(),
        };
        // Une installation en cours prime sur le bouton : la fiche suit le
        // transfert comme la carte de la bibliothèque, avec sa barre et sa
        // croix. Sans cela, lancer le téléchargement depuis la fiche laissait
        // le bouton « Installer » à sa place, comme si rien ne partait.
        // Les versions publiées de la boutique, dans l'ordre Windows, macOS
        // puis Linux : la fiche peut installer celle qu'on veut, comme Steam.
        let versions: Vec<GameVersion> = [
            ("windows", "Windows"),
            ("macos", "macOS"),
            ("linux", "Linux"),
        ]
        .into_iter()
        .filter(|(key, _)| {
            game.platforms
                .iter()
                .any(|platform| platform.eq_ignore_ascii_case(key))
        })
        .map(|(key, label)| GameVersion {
            id: key,
            label,
            size: None,
        })
        .collect();
        let chosen = self
            .stores
            .platform
            .get(&game.appid)
            .and_then(|platform| versions.iter().position(|version| version.id == platform))
            .unwrap_or(0);
        let buttons = match self.running_job(game.appid) {
            Some((fraction, job)) => CardButtons::Progress {
                fraction,
                job,
                appid: game.appid,
            },
            None if mobile_only => CardButtons::Mobile {
                appid: game.appid,
                versions: mobile_versions.clone(),
            },
            None => CardButtons::Download {
                appid: game.appid,
                // Ce que le téléchargement demandera, comme le launcher Epic :
                // la place sur le disque se lit dans l'encart.
                size: download,
                dlc: game.dlc.len(),
                versions,
                mobile: mobile_versions,
                chosen,
                family: Vec::new(),
                family_chosen: 0,
                dlc_names: Vec::new(),
                steam: false,
            },
        };
        // Dossier du jeu quand il est sur le disque : la fiche gagne le même
        // bouton dossier qu'une fiche Steam.
        let install_dir: Option<std::path::PathBuf> = self
            .games
            .iter()
            .find(|installed| installed.appid == game.appid && installed.installed)
            .map(|installed| installed.install_dir());
        let mut chosen_platform: Option<&'static str> = None;
        // La phrase ne s'affiche que sur un jeu installé : le dossier a été
        // lu, la fiche peut dire ce qu'il contient.
        let drm_line = drm
            .filter(|_| game.installed)
            .and_then(|report| store_drm_sentence(game.store, Some(report)));
        ui.horizontal_top(|ui| {
            // Les colonnes sont calculées sans écart implicite : chaque
            // `add_space` en ajoutait un de plus et la ligne débordait.
            ui.spacing_mut().item_spacing.x = 0.0;
            self.sheet_controls(ui, height, expanded);
            ui.add_space(SHEET_CONTROLS_GAP);
            let action = identity_card(
                ui,
                identity,
                height,
                game.appid,
                &game.name,
                &texture,
                &None,
                false,
                install_dir.as_deref(),
                None,
                false,
                Some(game.store),
                show_badges,
                badge_scale,
                &detail,
                &played,
                &played_hint,
                &size,
                // Le cloud se lit pour **ce jeu** : la capacité de la boutique
                // dit qu'Epic ou GOG en proposent, pas que ce titre en a. Sans
                // réponse de la boutique, rien n'est annoncé.
                details.and_then(|details| details.cloud).unwrap_or(false),
                drm_line.as_deref(),
                &buttons,
                false,
            );
            if action.store_install {
                install = true;
            }
            // Le bouton DLC de la ligne boutique ouvre le même panneau que
            // celui de Steam : sans cette lecture, le clic ne faisait rien —
            // l'action était posée puis jetée avec la ligne.
            if action.contents {
                self.dlc_modal = Some(game.appid);
            }
            if action.version.is_some() {
                chosen_platform = action.version;
            }
            // L'encart du milieu fait la même chose que sur une fiche Steam :
            // le résumé d'abord, les succès ensuite, côte à côte.
            let summary = (identity_gap - 36.0).max(0.0);
            if summary >= 240.0 {
                ui.add_space(18.0);
                let content = crate::gameinfo::SheetContent::from_store_details(details);
                let bunny = self.bunny.clone();
                // Le même encart que la fiche Steam, à la même place : lapin
                // pendant la lecture, captures cliquables, hauteur commune.
                self.gameinfo.summary_page(
                    ui,
                    details.map(|_| &content),
                    waiting,
                    error,
                    summary,
                    action.height.max(80.0),
                    bunny.as_ref(),
                );
                ui.add_space(18.0);
            } else {
                ui.add_space(identity_gap);
            }
            ui.allocate_ui_with_layout(
                egui::vec2(middle, height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(middle);
                    let bunny = self.bunny.clone();
                    let accent = self.downloads.accent(ui.ctx(), &self.paths, game.appid);
                    ui.ctx().data_mut(|data| {
                        data.insert_temp(egui::Id::new(("game-accent", game.appid)), accent);
                    });
                    self.achievements.block(
                        ui,
                        &game.name,
                        game.appid,
                        true,
                        true,
                        &self.client,
                        middle,
                        action.height.max(80.0),
                        bunny.as_ref(),
                        game.store,
                        accent,
                    );
                },
            );
        });
        // La version choisie dans la flèche est retenue pour ce jeu : le
        // bouton et la demande d'installation suivent le même choix.
        if let Some(platform) = chosen_platform {
            self.stores
                .platform
                .insert(game.appid, platform.to_string());
        }
        if install {
            let platform = self.stores.platform.get(&game.appid).cloned();
            if let Some(message) =
                self.stores
                    .start_install(&self.client, game.store, &game.store_id, platform)
            {
                self.notify(message);
            }
        }
    }

    /// Corps d'une fiche Steam absente du disque : actualités, mises à jour et
    /// description, sans réglages ni désinstallation.
    fn catalog_sheet_body(&mut self, ui: &mut egui::Ui, entry: &SteamCatalogGame) {
        egui::ScrollArea::vertical()
            .id_salt(("catalog-sheet", entry.appid))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let width = ui.available_width();
                let (actus, center, media) = sheet_body_columns(width);
                if actus <= 0.0 {
                    if self
                        .gameinfo
                        .news_embed(ui, entry.appid, self.bunny.as_ref())
                    {
                        ui.add_space(14.0);
                    }
                    self.catalog_sheet_main(ui, entry);
                    self.gameinfo.media_embed(ui, entry.appid, width);
                    return;
                }
                // Les annonces à gauche, les images à droite.
                ui.horizontal_top(|ui| {
                    let gap = sheet_column_gap(ui.spacing().item_spacing.x);
                    ui.allocate_ui_with_layout(
                        egui::vec2(actus, 0.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_width(actus);
                            self.gameinfo
                                .news_embed(ui, entry.appid, self.bunny.as_ref());
                        },
                    );
                    ui.add_space(gap);
                    ui.allocate_ui_with_layout(
                        egui::vec2(center, 0.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_width(center);
                            self.catalog_sheet_main(ui, entry);
                        },
                    );
                    if media > 0.0 {
                        ui.add_space(gap);
                        ui.allocate_ui_with_layout(
                            egui::vec2(media, 0.0),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                ui.set_width(media);
                                self.gameinfo.media_embed(ui, entry.appid, media);
                            },
                        );
                    }
                });
            });
    }

    /// Ligne compacte d'un jeu absent du disque : jaquette, identité et
    /// téléchargement, succès Steam, rail et repli.
    fn catalog_compact_row(
        &mut self,
        ui: &mut egui::Ui,
        entry: &SteamCatalogGame,
        height: f32,
        expanded: bool,
    ) {
        let texture = self.covers.get(
            ui.ctx(),
            &self.paths,
            entry.appid,
            &entry.name,
            None,
            "",
            CoverTier::Sheet,
        );
        let has_custom = custom_cover_path(&self.paths, entry.appid).is_file();
        let width = ui.available_width();
        let columns = compact_columns(width);
        let (identity, identity_gap, middle) =
            (columns.identity, columns.identity_gap, columns.middle);
        // Les succès se lisent par la session Steam enregistrée : c'est elle
        // que le daemon utilise, avec la seule session enregistrée.
        let session = self
            .steam_status
            .as_ref()
            .is_some_and(|status| status.token_saved);
        let show_badges = self.config.library.show_badges;
        let badge_scale = self.config.library.badge_scale;
        // Le temps de jeu vient du compte, pas du disque : la fiche est donc
        // demandée même quand la ligne reste repliée.
        self.gameinfo.ensure(ui.ctx(), &self.client, entry.appid);
        let versions = game_versions(&entry.details);
        let chosen = self.version_index(entry.appid, &versions);
        let played = played_label(self.play_minutes(entry.appid, None));
        let played_hint = playtime_tooltip(
            self.gameinfo
                .info()
                .filter(|info| info.appid == entry.appid)
                .map(|info| &info.account),
            None,
        );
        // Sur le disque quand il y est, sinon ce que Steam annonce pour la
        // version retenue : la place qu'il prendra une fois installé.
        let size = self
            .installed_size(entry.appid)
            .or_else(|| {
                versions
                    .get(chosen)
                    .and_then(|version| published_install_bytes(&entry.details, version.id))
            })
            .map(lapinux_core::util::format_bytes)
            .unwrap_or_default();
        let download = public_download_bytes(&entry.details);
        let mut choose_cover = false;
        let mut reset_cover = false;
        let self_owns = self.family_self_owns(entry.appid) || !entry.family;
        let family_available = self.family_shared_by_other(entry.appid) || entry.family;
        let detail = format!(
            "{} · {} · appid {}",
            if entry.installed {
                t("game_sheet.installed_outside_scan")
            } else {
                t("game_sheet.not_installed")
            },
            if entry.family && !self_owns {
                t("game_sheet.family_shared")
            } else if family_available && self_owns {
                t("game_sheet.personal_and_family")
            } else {
                t("game_sheet.personal_account")
            },
            entry.appid
        );
        // Un jeu installé hors du scan n'est pas dans la bibliothèque
        // Lapinux : le relire est ce qui le rend jouable.
        let download = versions
            .get(chosen)
            .and_then(|version| version.size)
            .or(download);
        let cloud = cloud_saves(
            self.gameinfo
                .info()
                .filter(|info| info.appid == entry.appid),
        );
        let mut buttons = match self.running_job(entry.appid) {
            Some((fraction, job)) => CardButtons::Progress {
                fraction,
                job,
                appid: entry.appid,
            },
            None if entry.installed => CardButtons::Rescan,
            None => {
                let mut family = if family_available {
                    self.family_versions(entry.appid, self_owns)
                } else {
                    Vec::new()
                };
                let family_chosen = self
                    .family_choice
                    .get(&entry.appid)
                    .and_then(|steam_id| {
                        family
                            .iter()
                            .position(|member| member.steam_id == *steam_id)
                    })
                    .unwrap_or(0);
                self.fill_family_dlc_names(&mut family, &entry.details, &entry.dlc);
                let version_dlc = self.version_dlc(entry.appid);
                CardButtons::Download {
                    appid: entry.appid,
                    size: download_size(
                        download,
                        &version_dlc,
                        Some(&selected_contents(
                            &self.dlc_selection,
                            entry.appid,
                            &version_dlc,
                        )),
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
        };
        self.load_family_avatars(ui.ctx(), &mut buttons);

        ui.horizontal_top(|ui| {
            // Sans cet espacement implicite, les largeurs de colonnes
            // calculées plus haut sont exactes : sinon chaque `add_space`
            // ajoutait aussi l'écart par défaut et la ligne débordait à
            // droite, décalant la marge de la fiche.
            ui.spacing_mut().item_spacing.x = 0.0;
            self.sheet_controls(ui, height, expanded);
            ui.add_space(SHEET_CONTROLS_GAP);
            let icon = self.art.get(
                ui.ctx(),
                &self.paths,
                entry.appid,
                &entry.details,
                SteamArt::Icon,
                &entry.name,
            );
            let action = identity_card(
                ui,
                identity,
                height,
                entry.appid,
                &entry.name,
                &texture,
                &icon,
                has_custom,
                None,
                Some(entry.family && !self_owns),
                family_available,
                None,
                show_badges,
                badge_scale,
                &detail,
                &played,
                &played_hint,
                &size,
                cloud,
                None,
                &buttons,
                false,
            );
            choose_cover = action.choose_cover;
            reset_cover = action.reset_cover;
            if action.install {
                let selected = self.selected_dlc(entry.appid);
                self.install_game(entry.appid, selected);
            }
            if let Some(version) = action.version {
                self.version_choice.insert(entry.appid, version.to_string());
            }
            if let Some(owner) = action.family_owner {
                self.family_choice.insert(entry.appid, owner);
            }
            if action.rescan {
                self.refresh(true);
                self.notify(t("settings.library.scanned"));
            }
            if action.contents {
                self.dlc_modal = Some(entry.appid);
            }
            if let Some(job) = action.cancel {
                self.cancel_job(&job);
            }
            // La ligne ne change pas quand on déplie : le résumé et les
            // succès restent au-dessus de la ligne du dépliage, identiques.
            // Le même écart de chaque côté du résumé.
            let summary = (identity_gap - 36.0).max(0.0);
            if summary >= 240.0 {
                // La hauteur de l'encart d'identité, pas celle de la ligne.
                ui.add_space(18.0);
                let bunny = self.bunny.clone();
                self.gameinfo.summary_embed(
                    ui,
                    entry.appid,
                    summary,
                    action.height.max(80.0),
                    bunny.as_ref(),
                );
                ui.add_space(18.0);
            } else {
                ui.add_space(identity_gap);
            }

            ui.allocate_ui_with_layout(
                egui::vec2(middle, height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(middle);
                    // Repliée, la colonne des succès descend jusqu'à la
                    // hauteur des autres cartes de la ligne.
                    let bunny = self.bunny.clone();
                    let accent = self.downloads.accent(ui.ctx(), &self.paths, entry.appid);
                    ui.ctx().data_mut(|data| {
                        data.insert_temp(egui::Id::new(("game-accent", entry.appid)), accent);
                    });
                    self.achievements.block(
                        ui,
                        &entry.name,
                        entry.appid,
                        true,
                        session,
                        &self.client,
                        middle,
                        action.height.max(80.0),
                        bunny.as_ref(),
                        lapinux_core::stores::Store::Steam,
                        accent,
                    );
                },
            );
        });

        if reset_cover {
            self.restore_automatic_cover(entry.appid);
        }
        if choose_cover {
            let ctx = ui.ctx().clone();
            self.open_cover_picker(entry.appid, entry.name.clone(), &ctx);
        }
    }

    fn catalog_sheet_main(&mut self, ui: &mut egui::Ui, entry: &SteamCatalogGame) {
        // La description ouvre la colonne du milieu, sans bannière au-dessus.
        if self.gameinfo.description(ui, entry.appid) {
            ui.add_space(12.0);
        }
    }

    /// Fiche du jeu : une feuille qui monte du bas de la fenêtre. Elle garde
    /// son contenu pendant la fermeture, le temps de la glissière.
    pub(super) fn detail_sheet(&mut self, ctx: &egui::Context) {
        if let Some(appid) = self.selected {
            // Une nouvelle fiche s'ouvre toujours repliée : le corps détaillé
            // reste un geste explicite. Ses succès, eux, sont relus : un
            // déblocage en partie ne doit pas attendre le prochain lancement.
            if self.detail_shown != Some(appid) {
                self.detail_expanded = false;
                self.achievements.forget();
                self.gameinfo.forget();
            }
            self.detail_shown = Some(appid);
        }
        let Some(appid) = self.detail_shown else {
            return;
        };
        let open = self.selected.is_some();
        let progress =
            advance_detail_sheet(self.detail_anim, open, ctx.input(|input| input.stable_dt));
        if progress != self.detail_anim {
            self.detail_anim = progress;
            ctx.request_repaint();
        }
        if !open && progress <= 0.0 {
            self.detail_shown = None;
            return;
        }
        show_detail_sheet(ctx, progress, self.detail_expanded, |ui| {
            self.detail_sheet_body(ui, appid);
        });
    }

    /// Ouvre un chemin dans le gestionnaire de fichiers du bureau, sans
    /// bloquer l'interface.
    pub(super) fn open_folder(&mut self, path: &Path) {
        if !path.exists() {
            self.notify(t_fmt(
                "game_sheet.folder_missing",
                &[path.display().to_string()],
            ));
            return;
        }
        match std::process::Command::new("xdg-open")
            .arg(path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(_) => {}
            Err(err) => self.notify(t_args(
                "game_sheet.folder_open_error",
                &[("err", &err.to_string())],
            )),
        }
    }
}
