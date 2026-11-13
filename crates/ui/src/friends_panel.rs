//! Panneau des amis et présences.

use super::*;

/// Petite icône d'information, dessinée pour ne pas dépendre d'un glyphe
/// absent de la police système.
fn info_icon(ui: &mut egui::Ui, hint: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.circle_stroke(rect.center(), 6.5, egui::Stroke::new(1.0_f32, TEXT_DIM()));
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "i",
        egui::FontId::proportional(9.0),
        TEXT_DIM(),
    );
    response.on_hover_text(hint)
}

/// Ce qu'un clic sur une ligne d'ami déclenche.
enum FriendRowAction {
    None,
    Profile,
    Chat,
}

impl LapinuxApp {
    /// Ouvre ou referme le panneau Amis, et relit les listes des boutiques.
    pub(super) fn toggle_friends(&mut self) {
        self.friends_open = !self.friends_open;
        if self.friends_open {
            self.load_store_friends();
        } else {
            self.friends_selected = None;
            self.friends_chat = None;
            self.friends_message.clear();
            self.friends_profile_top = false;
            self.friends_history_top_armed = false;
            self.friends_emotes_open = false;
            self.friends_import_open = false;
            self.friends_import_note = None;
        }
    }

    /// Rafraîchit les présences de l'accueil sans interroger les boutiques à chaque image.
    pub(super) fn ensure_home_friends(&mut self, ctx: &egui::Context) {
        if self
            .friends_last_refresh
            .map_or(true, |at| at.elapsed() >= Duration::from_secs(120))
        {
            self.load_store_friends();
        }
        if self.friends_loading {
            ctx.request_repaint_after(Duration::from_secs(1));
        } else if let Some(at) = self.friends_last_refresh {
            ctx.request_repaint_after(Duration::from_secs(120).saturating_sub(at.elapsed()));
        }
    }

    /// Demande au daemon les listes d'amis Epic et GOG, hors du fil de peinture :
    /// une boutique lente ne fige pas la fenêtre.
    fn load_store_friends(&mut self) {
        if self.friends_rx.is_some() {
            return;
        }
        self.friends_last_refresh = Some(Instant::now());
        let client = self.client.clone();
        let (tx, rx) = mpsc::channel();
        self.friends_rx = Some(rx);
        self.friends_loading = true;
        std::thread::spawn(move || {
            let boards = client
                .request_with_timeout(&Req::StoreFriends, Duration::from_secs(30))
                .ok()
                .filter(|envelope| envelope.ok)
                .and_then(|envelope| envelope.into_data::<Vec<StoreFriends>>().ok());
            let _ = tx.send(boards);
        });
    }

    /// Relit la réponse des boutiques et les photos arrivées.
    fn poll_store_friends(&mut self, ctx: &egui::Context) {
        let received = self.friends_rx.as_ref().map(|rx| rx.try_recv());
        match received {
            Some(Ok(boards)) => {
                // Un échec de la requête ne doit pas effacer une liste déjà
                // affichée : elle reste, seule la relecture a raté.
                self.friends_store_available = boards.is_some();
                if let Some(boards) = boards {
                    self.store_friends = boards;
                }
                self.friends_rx = None;
                self.friends_loading = false;
                ctx.request_repaint();
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.friends_store_available = false;
                self.friends_rx = None;
                self.friends_loading = false;
            }
            Some(Err(mpsc::TryRecvError::Empty)) => {}
            None => {}
        }
        loop {
            let received = match self.friends_avatar_rx.as_ref() {
                Some(rx) => rx.try_recv(),
                None => break,
            };
            match received {
                Ok((key, bytes)) => {
                    let texture = image::load_from_memory(&bytes)
                        .ok()
                        .map(|image| texture_from_image(ctx, image, &format!("friend-{key}")));
                    self.friends_avatar_pending.remove(&key);
                    self.friends_avatars.insert(key, texture);
                    ctx.request_repaint();
                }
                Err(_) => break,
            }
        }
        loop {
            let received = match self.emote_rx.as_ref() {
                Some(rx) => rx.try_recv(),
                None => break,
            };
            match received {
                Ok((name, bytes)) => {
                    let texture = image::load_from_memory(&bytes)
                        .ok()
                        .map(|image| texture_from_image(ctx, image, &format!("emote-{name}")));
                    self.emote_textures.insert(name, texture);
                    ctx.request_repaint();
                }
                Err(_) => break,
            }
        }
        loop {
            let received = match self.sticker_rx.as_ref() {
                Some(rx) => rx.try_recv(),
                None => break,
            };
            match received {
                Ok((name, frames)) => {
                    let animation =
                        frames.and_then(|frames| StickerAnimation::new(ctx, &name, frames));
                    self.sticker_textures.insert(name, animation);
                    ctx.request_repaint();
                }
                Err(_) => break,
            }
        }
    }

    /// Photo d'un ami : le fichier déjà téléchargé par le daemon pour Steam,
    /// un téléchargement unique pour les boutiques.
    pub(super) fn friend_avatar(
        &mut self,
        ctx: &egui::Context,
        key: &str,
        url: &str,
    ) -> Option<egui::TextureHandle> {
        if let Some(texture) = self.friends_avatars.get(key) {
            return texture.clone();
        }
        if let Some(steam_id) = key.strip_prefix("steam:") {
            let path = self
                .paths
                .cache_dir
                .join("steam-avatars")
                .join(format!("{steam_id}.jpg"));
            if let Some(texture) = load_texture_file(ctx, &path, &format!("friend-{key}")) {
                let texture = Some(texture);
                self.friends_avatars
                    .insert(key.to_string(), texture.clone());
                return texture;
            }
            // Le daemon n'a pas encore posé la photo : son URL, quand Steam
            // la publie, la télécharge comme celle d'un ami de boutique.
            if url.trim().is_empty() {
                self.friends_avatars.insert(key.to_string(), None);
                return None;
            }
        }
        if url.trim().is_empty() || !self.friends_avatar_pending.insert(key.to_string()) {
            return None;
        }
        if self.friends_avatar_tx.is_none() {
            let (tx, rx) = mpsc::channel();
            self.friends_avatar_tx = Some(tx);
            self.friends_avatar_rx = Some(rx);
        }
        if let Some(tx) = self.friends_avatar_tx.clone() {
            let key = key.to_string();
            let url = url.to_string();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                if let Some(bytes) = achievements::http_bytes(&url) {
                    let _ = tx.send((key, bytes));
                }
                // Le panneau ne se repeint pas tout seul : sans ce réveil, la
                // photo restait dans le canal jusqu'au prochain mouvement.
                ctx.request_repaint();
            });
        }
        None
    }

    /// Panneau Amis : Steam, Epic Games et GOG dans une seule liste, ouverte
    /// depuis l'icône de la barre du haut.
    pub(super) fn friends_panel(&mut self, ctx: &egui::Context) {
        // Les photos arrivent même quand le panneau est fermé : la fiche d'un
        // jeu partagé demande celle du propriétaire familial.
        self.poll_store_friends(ctx);
        if !self.friends_open {
            return;
        }
        let mut rows: Vec<FriendRow> = Vec::new();
        if let Some(social) = self.social.as_ref() {
            for friend in &social.friends {
                // La liste du CM peut porter le compte lui-même : il n'est pas
                // son propre ami.
                if friend.steam_id == 0 || friend.steam_id == social.steam_id {
                    continue;
                }
                rows.push(FriendRow {
                    store: lapinux_core::stores::Store::Steam,
                    key: format!("steam:{}", friend.steam_id),
                    name: friend.name.clone(),
                    state: friend.state_line(),
                    avatar: friend.avatar.clone(),
                    online: friend.presence.online(),
                    in_game: friend.in_game(),
                    away: matches!(
                        friend.presence,
                        lapinux_core::social::SteamPresence::Away
                            | lapinux_core::social::SteamPresence::Snooze
                    ),
                    last_seen: friend.last_logoff,
                    game: friend.game.clone(),
                    game_appid: friend.game_appid,
                    last_logon: friend.last_logon,
                });
            }
        }
        for board in &self.store_friends {
            for friend in &board.friends {
                // Le nom d'amitié choisi par le joueur prime, comme dans le
                // client de la boutique.
                let name = if friend.alias.trim().is_empty() {
                    friend.name.clone()
                } else {
                    friend.alias.clone()
                };
                rows.push(FriendRow {
                    store: friend.store,
                    key: format!("{}:{}", friend.store.key(), friend.id),
                    name,
                    state: friend.state_line(),
                    avatar: friend.avatar.clone(),
                    online: friend.online(),
                    in_game: friend.status == "in-game" || !friend.game.trim().is_empty(),
                    away: matches!(friend.status.as_str(), "away" | "snooze"),
                    last_seen: friend.last_seen,
                    game: friend.game.clone(),
                    game_appid: 0,
                    last_logon: 0,
                });
            }
        }
        let total = rows.len();
        let available = rows.iter().filter(|row| row.in_game || row.online).count();
        let subtitle = if self.friends_loading && total == 0 {
            lapinux_core::i18n::t("friends.loading_lists").to_string()
        } else if total == 0 {
            lapinux_core::i18n::t("friends.subtitle_empty").to_string()
        } else {
            lapinux_core::i18n::t_args(
                "friends.summary",
                &[
                    ("total", &total.to_string()),
                    ("available", &available.to_string()),
                ],
            )
        };
        let selected_profile = self
            .friends_selected
            .as_ref()
            .and_then(|key| rows.iter().find(|row| &row.key == key));
        let selected_chat = self
            .friends_chat
            .as_ref()
            .and_then(|key| rows.iter().find(|row| &row.key == key));
        let chat_open = selected_chat.is_some();
        if self.friends_import_open && !chat_open {
            self.friends_import_open = false;
            self.friends_import_note = None;
        }
        let selected = selected_chat.or(selected_profile);
        if self.friends_chat.is_some() && !chat_open {
            self.friends_chat = None;
        }
        if selected.is_none() {
            // La liste a pu être relue : une fiche ou une conversation dont
            // l'ami a disparu ne reste pas ouverte sur une vue vide.
            self.friends_selected = None;
            self.friends_chat = None;
            self.friends_message.clear();
            self.friends_profile_top = false;
            self.friends_history_top_armed = false;
        }
        let panel_title = if chat_open {
            lapinux_core::i18n::t("friends.conversation")
        } else {
            lapinux_core::i18n::t("friends.title")
        };
        let panel_subtitle = if chat_open {
            selected.map(|row| row.name.as_str()).unwrap_or("")
        } else {
            subtitle.as_str()
        };
        let close = fixed_panel_with_scroll(
            ctx,
            "friends-panel",
            panel_title,
            panel_subtitle,
            egui::vec2(620.0, 660.0),
            !chat_open,
            true,
            196,
            |ui| {
                if let Some(row) = selected {
                    // On revient en haut de la fiche avant de peindre les
                    // onglets : viser le début du contenu de la conversation
                    // les faisait disparaître sous le défilement du panneau.
                    if self.friends_profile_top {
                        ui.scroll_to_cursor(Some(egui::Align::TOP));
                        self.friends_profile_top = false;
                    }
                    let mut show_chat = chat_open;
                    let bar = friend_top_bar(
                        ui,
                        row.store == lapinux_core::stores::Store::Steam,
                        chat_open,
                    );
                    if bar == FriendTopAction::Back {
                        self.friends_selected = None;
                        self.friends_chat = None;
                        self.friends_message.clear();
                        self.friends_profile_top = false;
                        self.friends_history_top_armed = false;
                        self.friends_chat_autoscroll = false;
                        self.friends_emotes_open = false;
                        self.friends_import_open = false;
                        self.friends_import_note = None;
                        return;
                    }
                    show_chat = match bar {
                        FriendTopAction::Profile => false,
                        FriendTopAction::Messages => true,
                        _ => show_chat,
                    };
                    ui.add_space(8.0);
                    if show_chat != chat_open {
                        if show_chat {
                            self.friends_chat = Some(row.key.clone());
                            self.friends_selected = None;
                            self.friends_message.clear();
                            self.friends_profile_top = true;
                            self.friends_history_top_armed = true;
                            self.friends_chat_autoscroll = true;
                            self.friends_chat_scroll_to_bottom = true;
                            self.request_steam_history(&row.key, false);
                        } else {
                            self.friends_selected = Some(row.key.clone());
                            self.friends_chat = None;
                            self.friends_message.clear();
                            self.friends_profile_top = true;
                            self.friends_history_top_armed = false;
                            self.friends_chat_autoscroll = false;
                        }
                    }
                    if show_chat {
                        self.friend_chat_panel(ui, ctx, row);
                    } else {
                        self.friend_profile(ui, ctx, row);
                    }
                    return;
                }
                let mut notes = Vec::new();
                let mut epic_presence = false;
                for board in &self.store_friends {
                    if board.note.is_empty() {
                        continue;
                    }
                    // La présence Epic manque souvent avec le jeton du launcher :
                    // ce n'est pas une déconnexion, la note reste donc derrière
                    // une icône au lieu d'occuper deux lignes du panneau.
                    if board.store == lapinux_core::stores::Store::Epic
                        && is_presence_permission_note(&board.note)
                    {
                        epic_presence = true;
                    } else {
                        notes.push(format!("{} : {}", board.store.label(), board.note));
                    }
                }
                ui.horizontal(|ui| {
                    // Un seul cadre, des segments de même largeur : la
                    // sélection ressemble à un sélecteur, pas à quatre
                    // boutons isolés. Le trait coloré marque la boutique
                    // active.
                    let reserved = if epic_presence { 24.0 } else { 0.0 };
                    // 8 px de marges internes et trois gouttières de 2 px :
                    // les quatre segments remplissent exactement la rangée.
                    let width = ((ui.available_width() - reserved - 14.0) / 4.0).max(80.0);
                    egui::Frame::none()
                        .fill(CARD())
                        .stroke(egui::Stroke::new(1.0_f32, BORDER()))
                        .rounding(egui::Rounding::same(12.0))
                        .inner_margin(egui::Margin::symmetric(4.0, 3.0))
                        .show(ui, |ui| {
                            ui.set_width(width * 4.0 + 6.0);
                            ui.spacing_mut().item_spacing.x = 2.0;
                            for (store, label) in [
                                (None, lapinux_core::i18n::t("library.all")),
                                (Some(lapinux_core::stores::Store::Steam), "Steam"),
                                (Some(lapinux_core::stores::Store::Epic), "Epic Games"),
                                (Some(lapinux_core::stores::Store::Gog), "GOG"),
                            ] {
                                let count = rows
                                    .iter()
                                    .filter(|row| store.map_or(true, |store| row.store == store))
                                    .count();
                                if friend_chip(
                                    ui,
                                    &format!("{label} · {count}"),
                                    self.friends_store == store,
                                    store,
                                    width,
                                )
                                .clicked()
                                {
                                    self.friends_store = store;
                                }
                            }
                        });
                    if epic_presence {
                        // La note de permission Epic reste alignée à droite
                        // du sélecteur, sans lui voler sa largeur.
                        ui.add_space((ui.available_width() - 16.0).max(4.0));
                        info_icon(ui, lapinux_core::i18n::t("friends.epic_presence_note"));
                    }
                });
                ui.add_space(8.0);
                ui.add(
                    egui::TextEdit::singleline(&mut self.friends_search)
                        .hint_text(lapinux_core::i18n::t("friends.search_placeholder"))
                        .desired_width(f32::INFINITY),
                );
                if !notes.is_empty() {
                    ui.add_space(8.0);
                    for note in &notes {
                        ui.label(egui::RichText::new(note).size(11.0).color(TEXT_DIM()));
                    }
                }
                let needle = self.friends_search.trim().to_lowercase();
                let mut visible: Vec<&FriendRow> = rows
                    .iter()
                    .filter(|row| self.friends_store.map_or(true, |store| row.store == store))
                    .filter(|row| needle.is_empty() || row.name.to_lowercase().contains(&needle))
                    .collect();
                // Steam compte un absent comme « en ligne ». Le rang explicite
                // le remet sous les amis réellement disponibles.
                let presence_rank = |row: &FriendRow| {
                    if row.in_game {
                        0
                    } else if row.away {
                        2
                    } else if row.online {
                        1
                    } else {
                        3
                    }
                };
                visible.sort_by(|left, right| {
                    store_rank(left.store)
                        .cmp(&store_rank(right.store))
                        .then_with(|| presence_rank(left).cmp(&presence_rank(right)))
                        .then(left.name.to_lowercase().cmp(&right.name.to_lowercase()))
                });
                if visible.is_empty() {
                    ui.add_space(14.0);
                    ui.label(
                        egui::RichText::new(if total == 0 {
                            lapinux_core::i18n::t("friends.none")
                        } else {
                            lapinux_core::i18n::t("friends.no_name")
                        })
                        .size(12.0)
                        .color(TEXT_DIM()),
                    );
                    return;
                }
                // Une section par boutique : la présence n'est pas publiée
                // partout, les mélanger laisserait croire à des absents.
                let mut current: Option<lapinux_core::stores::Store> = None;
                for row in &visible {
                    if current != Some(row.store) {
                        current = Some(row.store);
                        ui.add_space(12.0);
                        let count = visible
                            .iter()
                            .filter(|other| other.store == row.store)
                            .count();
                        ui.label(
                            egui::RichText::new(format!("{} · {count}", row.store.label()))
                                .size(12.0)
                                .strong()
                                .color(ACCENT()),
                        );
                        ui.add_space(2.0);
                    }
                    match self.friend_row(ui, ctx, row) {
                        FriendRowAction::Profile => {
                            self.friends_selected = Some(row.key.clone());
                            self.friends_chat = None;
                            self.friends_message.clear();
                            self.friends_profile_top = true;
                            self.friends_history_top_armed = false;
                            self.friends_chat_autoscroll = false;
                        }
                        FriendRowAction::Chat => {
                            self.friends_chat = Some(row.key.clone());
                            self.friends_selected = None;
                            self.friends_message.clear();
                            self.friends_profile_top = true;
                            self.friends_history_top_armed = true;
                            self.friends_chat_autoscroll = true;
                            self.friends_chat_scroll_to_bottom = true;
                            self.request_steam_history(&row.key, false);
                        }
                        FriendRowAction::None => {}
                    }
                }
            },
        );
        if close {
            self.friends_open = false;
            self.friends_selected = None;
            self.friends_chat = None;
            self.friends_message.clear();
            self.friends_profile_top = false;
            self.friends_history_top_armed = false;
            self.friends_emotes_open = false;
            self.friends_import_open = false;
            self.friends_import_note = None;
        }
        if self.friends_import_open {
            let target = selected_chat.and_then(|row| {
                row.key
                    .strip_prefix("steam:")
                    .and_then(|id| id.parse::<u64>().ok())
                    .map(|id| (id, row.name.as_str()))
            });
            if let Some((steam_id, name)) = target {
                let close_import = fixed_panel(
                    ctx,
                    "friends-import",
                    lapinux_core::i18n::t("friends.import_history"),
                    name,
                    egui::vec2(520.0, 300.0),
                    |ui| self.import_history_body(ui, steam_id),
                );
                if close_import {
                    self.friends_import_open = false;
                }
            } else {
                self.friends_import_open = false;
            }
        }
    }

    /// Une ligne d'ami : photo, nom, état et provenance. La bulle de
    /// conversation ouvre la fiche en mode messages ; le reste ouvre la fiche.
    fn friend_row(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        row: &FriendRow,
    ) -> FriendRowAction {
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 44.0), egui::Sense::hover());
        let message_rect = (row.store == lapinux_core::stores::Store::Steam).then(|| {
            egui::Rect::from_center_size(
                egui::pos2(rect.right() - 24.0, rect.center().y),
                egui::vec2(30.0, 30.0),
            )
        });
        let profile_rect = match message_rect {
            Some(message) => {
                egui::Rect::from_min_max(rect.min, egui::pos2(message.left() - 6.0, rect.max.y))
            }
            None => rect,
        };
        let profile_id = ui.id().with(("friend-open", row.key.as_str()));
        let message_id = ui.id().with(("friend-message", row.key.as_str()));
        let profile = ui
            .interact(profile_rect, profile_id, egui::Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        let message = message_rect.map(|rect| {
            ui.interact(rect, message_id, egui::Sense::click())
                .on_hover_cursor(egui::CursorIcon::PointingHand)
        });
        let message_hovered = message
            .as_ref()
            .map_or(false, |response| response.hovered());
        let painter = ui.painter().clone();
        if profile.hovered() || message_hovered {
            painter.rect_filled(
                rect.shrink2(egui::vec2(2.0, 1.0)),
                egui::Rounding::same(8.0),
                CARD_HOVER(),
            );
        }
        let avatar = egui::Rect::from_center_size(
            egui::pos2(rect.left() + 24.0, rect.center().y),
            egui::vec2(32.0, 32.0),
        );
        // Fond de la pastille : les icônes de profil Epic sont transparentes.
        painter.circle_filled(avatar.center(), 16.0, CARD_ACTIVE());
        match self.friend_avatar(ctx, &row.key, &row.avatar) {
            Some(texture) => {
                egui::Image::new((texture.id(), avatar.size()))
                    .rounding(egui::Rounding::same(16.0))
                    .paint_at(ui, avatar);
            }
            None => {
                painter.text(
                    avatar.center(),
                    egui::Align2::CENTER_CENTER,
                    row.name
                        .chars()
                        .next()
                        .unwrap_or('?')
                        .to_uppercase()
                        .to_string(),
                    egui::FontId::proportional(13.0),
                    TEXT(),
                );
            }
        }
        // Pastille de présence au bord de la photo, comme la liste Steam.
        // Un ami absent reste « en ligne » pour Steam, mais sa pastille orange
        // le distingue d'un ami réellement disponible.
        let dot = friend_presence_color(row);
        painter.circle_filled(avatar.center() + egui::vec2(12.0, 12.0), 4.0, dot);
        let text_left = avatar.right() + 12.0;
        let text_right = message_rect.map_or(rect.right() - 8.0, |message| message.left() - 8.0);
        let text = painter.with_clip_rect(egui::Rect::from_min_max(
            egui::pos2(text_left, rect.top()),
            egui::pos2(text_right.max(text_left + 1.0), rect.bottom()),
        ));
        text.text(
            egui::pos2(text_left, rect.center().y - 8.0),
            egui::Align2::LEFT_CENTER,
            lapinux_core::util::ellipsize(&row.name, 34),
            egui::FontId::proportional(13.0),
            TEXT(),
        );
        let state_line = if !row.online && row.last_seen > 0 {
            let date = lapinux_core::util::format_datetime(row.last_seen);
            lapinux_core::i18n::t_args("friends.last_seen_at", &[("date", date.as_str())])
        } else {
            row.state.clone()
        };
        if !state_line.trim().is_empty() {
            text.text(
                egui::pos2(text_left, rect.center().y + 8.0),
                egui::Align2::LEFT_CENTER,
                lapinux_core::util::ellipsize(&state_line, 44),
                egui::FontId::proportional(11.0),
                TEXT_DIM(),
            );
        }
        if let (Some(rect), Some(response)) = (message_rect, &message) {
            let hovered = response.hovered();
            let pressed = response.is_pointer_button_down_on();
            let fill = if pressed {
                mix(CARD_ACTIVE(), BACKGROUND(), 0.22)
            } else if hovered {
                CARD_HOVER()
            } else {
                CARD_ACTIVE()
            };
            painter.rect_filled(rect, egui::Rounding::same(8.0), fill);
            painter.rect_stroke(
                rect,
                egui::Rounding::same(8.0),
                egui::Stroke::new(1.0_f32, BORDER()),
            );
            paint_message_icon(
                &painter,
                rect.center(),
                if hovered || pressed {
                    ACCENT()
                } else {
                    TEXT_DIM()
                },
            );
        }
        let message_clicked = message
            .as_ref()
            .map_or(false, |response| response.clicked());
        if let Some(response) = message {
            response.on_hover_text(lapinux_core::i18n::t("friends.message"));
        }
        if message_clicked {
            FriendRowAction::Chat
        } else if profile.clicked() {
            FriendRowAction::Profile
        } else {
            FriendRowAction::None
        }
    }

    /// Demande à Steam les derniers messages échangés avec cet ami. La
    /// réponse arrive par la relève sociale et rejoint la conversation.
    fn request_steam_history(&mut self, key: &str, older: bool) {
        let Some(steam_id) = key
            .strip_prefix("steam:")
            .and_then(|id| id.parse::<u64>().ok())
        else {
            return;
        };
        if older
            && !self
                .social
                .as_ref()
                .and_then(|social| social.history.get(&steam_id))
                .is_some_and(|history| history.more_available)
        {
            return;
        }
        // L'indicateur apparaît dès le clic, sans attendre la relève sociale.
        if let Some(social) = self.social.as_mut() {
            let history = social.history.entry(steam_id).or_default();
            history.loading = true;
            history.error = None;
        }
        match self.client.request_with_timeout(
            &Req::SteamChatHistory { steam_id, older },
            Duration::from_secs(3),
        ) {
            Ok(envelope) if envelope.ok => {
                // La lecture sociale est relancée tout de suite, puis la
                // relève périodique rapporte les messages arrivés entre-temps.
                self.social_poll = std::time::Instant::now() - Duration::from_secs(10);
            }
            Ok(envelope) => {
                let error = envelope
                    .error
                    .unwrap_or_else(|| lapinux_core::i18n::t("friends.history_error").to_string());
                if let Some(history) = self
                    .social
                    .as_mut()
                    .and_then(|social| social.history.get_mut(&steam_id))
                {
                    history.loading = false;
                    history.error = Some(error.clone());
                }
                self.notify(error);
            }
            Err(err) => {
                let error = format!("{err}");
                if let Some(history) = self
                    .social
                    .as_mut()
                    .and_then(|social| social.history.get_mut(&steam_id))
                {
                    history.loading = false;
                    history.error = Some(error.clone());
                }
                self.notify(error);
            }
        }
    }

    /// Envoie le brouillon de la fiche à un ami Steam.
    fn send_steam_message(&mut self, steam_id: u64) {
        let text = self.friends_message.trim().to_string();
        if text.is_empty() {
            return;
        }
        if text.len() > 1024 {
            self.notify("Message trop long (1024 octets maximum).".to_string());
            return;
        }
        match self.client.request_with_timeout(
            &lapinux_core::ipc::Req::SteamSocialMessage { steam_id, text },
            Duration::from_secs(5),
        ) {
            Ok(envelope) if envelope.ok => {
                // Le daemon ajoute la copie locale une fois le message accepté
                // par le CM ; la relève sociale la fera apparaître ici.
                self.friends_message.clear();
                self.friends_chat_autoscroll = true;
                self.friends_chat_scroll_to_bottom = true;
                self.social_poll = std::time::Instant::now() - Duration::from_secs(10);
            }
            Ok(envelope) => self.notify(
                envelope
                    .error
                    .unwrap_or_else(|| lapinux_core::i18n::t("friends.send_error").to_string()),
            ),
            Err(err) => self.notify(format!("{err}")),
        }
    }

    /// Envoie un autocollant choisi dans le sélecteur. Le daemon ajoute la
    /// copie locale sous forme de balise, que la bulle rend en image.
    fn send_steam_sticker(&mut self, steam_id: u64, name: &str) {
        match self.client.request_with_timeout(
            &lapinux_core::ipc::Req::SteamSocialSticker {
                steam_id,
                name: name.to_string(),
            },
            Duration::from_secs(5),
        ) {
            Ok(envelope) if envelope.ok => {
                self.friends_chat_autoscroll = true;
                self.friends_chat_scroll_to_bottom = true;
                self.social_poll = std::time::Instant::now() - Duration::from_secs(10);
            }
            Ok(envelope) => self.notify(
                envelope
                    .error
                    .unwrap_or_else(|| lapinux_core::i18n::t("friends.send_error").to_string()),
            ),
            Err(err) => self.notify(format!("{err}")),
        }
    }

    /// Le petit formulaire d'import d'un export JSON, ouvert depuis l'en-tête
    /// de la conversation. Le chemin peut être saisi ou déposé sur le panneau.
    fn import_history_body(&mut self, ui: &mut egui::Ui, steam_id: u64) {
        ui.label(
            egui::RichText::new(lapinux_core::i18n::t("friends.import_history_hint"))
                .size(11.0)
                .color(TEXT_DIM()),
        );
        ui.add_space(8.0);
        ui.add(
            egui::TextEdit::singleline(&mut self.friends_import_path)
                .hint_text(lapinux_core::i18n::t("friends.import_history_path"))
                .desired_width(f32::INFINITY),
        );
        if let Some(path) = ui.ctx().input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .find_map(|file| file.path.clone())
        }) {
            self.friends_import_path = path.to_string_lossy().into_owned();
        }
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(lapinux_core::i18n::t("friends.import_history_format"))
                .size(10.0)
                .color(TEXT_DIM()),
        );
        ui.add_space(10.0);
        let can_import = !self.friends_import_path.trim().is_empty();
        if ui
            .add_enabled(
                can_import,
                egui::Button::new(
                    egui::RichText::new(lapinux_core::i18n::t("friends.import_history_action"))
                        .size(11.5)
                        .color(TEXT()),
                )
                .fill(CARD_ACTIVE())
                .stroke(egui::Stroke::new(1.0_f32, BORDER()))
                .rounding(egui::Rounding::same(8.0))
                .min_size(egui::vec2(90.0, 28.0)),
            )
            .clicked()
        {
            self.import_chat_history(steam_id);
        }
        if let Some(note) = &self.friends_import_note {
            ui.add_space(8.0);
            ui.label(egui::RichText::new(note).size(10.5).color(TEXT_DIM()));
        }
    }

    /// Envoie l'import au daemon, qui relit le fichier et fusionne ses messages
    /// dans l'archive locale. Aucun message n'est envoyé à Steam.
    fn import_chat_history(&mut self, steam_id: u64) {
        let path = self.friends_import_path.trim().to_string();
        if path.is_empty() {
            return;
        }
        match self.client.request_with_timeout(
            &Req::SteamChatImport { steam_id, path },
            Duration::from_secs(15),
        ) {
            Ok(envelope) if envelope.ok => {
                let count = envelope
                    .data
                    .get("imported")
                    .and_then(|value| value.as_u64())
                    .unwrap_or(0);
                let count = count.to_string();
                self.friends_import_note = Some(lapinux_core::i18n::t_args(
                    "friends.import_history_done",
                    &[("count", count.as_str())],
                ));
                self.social_poll = Instant::now() - Duration::from_secs(10);
                self.friends_chat_autoscroll = false;
            }
            Ok(envelope) => {
                let error = envelope.error.unwrap_or_else(|| {
                    lapinux_core::i18n::t("friends.import_history_error_generic").to_string()
                });
                self.friends_import_note = Some(lapinux_core::i18n::t_args(
                    "friends.import_history_error",
                    &[("error", error.as_str())],
                ));
            }
            Err(error) => {
                let error = error.to_string();
                self.friends_import_note = Some(lapinux_core::i18n::t_args(
                    "friends.import_history_error",
                    &[("error", error.as_str())],
                ));
            }
        }
    }

    /// Télécharge en fond les images d'emotes encore inconnues. Un nom n'est
    /// demandé qu'une fois : une image illisible laisse une entrée vide au
    /// lieu de relancer la requête à chaque image.
    fn ensure_emotes(&mut self, ctx: &egui::Context, names: Vec<String>) {
        if names.is_empty() {
            return;
        }
        if self.emote_tx.is_none() {
            let (tx, rx) = mpsc::channel();
            self.emote_tx = Some(tx);
            self.emote_rx = Some(rx);
        }
        for name in names {
            if name.is_empty()
                || self.emote_textures.contains_key(&name)
                || !self.emote_pending.insert(name.clone())
            {
                continue;
            }
            let Some(tx) = self.emote_tx.clone() else {
                break;
            };
            let url = lapinux_core::social::steam_emoticon_url(&name);
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                if let Some(bytes) = achievements::http_bytes(&url) {
                    let _ = tx.send((name, bytes));
                }
                // Le panneau ne se repeint pas tout seul : sans ce réveil,
                // l'image resterait dans le canal jusqu'au prochain mouvement.
                ctx.request_repaint();
            });
        }
    }

    /// Télécharge en fond les images d'autocollants encore inconnues. Même
    /// principe que les emotes : un nom n'est demandé qu'une fois.
    fn ensure_stickers(&mut self, ctx: &egui::Context, names: Vec<String>) {
        if names.is_empty() {
            return;
        }
        if self.sticker_tx.is_none() {
            let (tx, rx) = mpsc::channel();
            self.sticker_tx = Some(tx);
            self.sticker_rx = Some(rx);
        }
        for name in names {
            if name.is_empty()
                || self.sticker_textures.contains_key(&name)
                || !self.sticker_pending.insert(name.clone())
            {
                continue;
            }
            let Some(tx) = self.sticker_tx.clone() else {
                break;
            };
            let url = lapinux_core::social::steam_sticker_url(&name);
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let frames =
                    achievements::http_bytes(&url).and_then(|bytes| decode_sticker_frames(&bytes));
                let _ = tx.send((name, frames));
                ctx.request_repaint();
            });
        }
    }

    /// Sélecteur d'emotes et d'autocollants : deux onglets dans une grille
    /// compacte au-dessus du compositeur. Un clic sur une emote insère son
    /// shortcode ; un clic sur un autocollant l'envoie immédiatement.
    /// Retourne le nom de l'autocollant choisi, le cas échéant.
    fn emote_picker(&mut self, ui: &mut egui::Ui, width: f32) -> Option<String> {
        let mut emoticons = self
            .social
            .as_ref()
            .map(|social| social.emoticons.clone())
            .unwrap_or_default();
        let mut stickers = self
            .social
            .as_ref()
            .map(|social| social.stickers.clone())
            .unwrap_or_default();
        if emoticons.is_empty() && stickers.is_empty() {
            muted(ui, lapinux_core::i18n::t("friends.emotes_empty"));
            ui.add_space(4.0);
            return None;
        }
        emoticons.sort_by(|left, right| left.name.cmp(&right.name));
        stickers.sort_by(|left, right| left.name.cmp(&right.name));
        let textures = &self.emote_textures;
        let sticker_textures = &self.sticker_textures;
        let time = ui.input(|input| input.time) as f32;
        let mut tab_stickers = self.friends_picker_stickers;
        let mut chosen_emote: Option<String> = None;
        let mut chosen_sticker: Option<String> = None;
        // Le panneau est peint sur un rectangle de largeur imposée : son
        // cadre ne peut plus se rétracter sur la largeur naturelle de la
        // grille et rester plus étroit que le compositeur.
        let panel_rect = ui
            .allocate_exact_size(egui::vec2(width, PICKER_HEIGHT - 4.0), egui::Sense::hover())
            .0;
        // La découpe de l'historique peut être plus étroite que la carte : le
        // panneau complet doit rester peint, y compris ses quatre bords.
        let previous_clip = ui.clip_rect();
        ui.set_clip_rect(previous_clip.union(panel_rect.expand(4.0)));
        let painter = ui.painter();
        painter.rect_filled(
            panel_rect.translate(egui::vec2(0.0, 4.0)).expand(3.0),
            egui::Rounding::same(16.0),
            egui::Color32::from_black_alpha(55),
        );
        painter.rect_filled(panel_rect, egui::Rounding::same(14.0), CARD());
        painter.rect_stroke(
            panel_rect,
            egui::Rounding::same(14.0),
            egui::Stroke::new(1.0_f32, with_alpha(BORDER(), 0.75)),
        );
        let content_rect = panel_rect.shrink2(egui::vec2(10.0, 8.0));
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(content_rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
            |ui| {
                ui.set_clip_rect(content_rect);
                ui.set_width(content_rect.width());
                ui.horizontal(|ui| {
                    ui.spacing_mut().button_padding = egui::vec2(9.0, 4.0);
                    egui::Frame::none()
                        .fill(PANEL())
                        .rounding(egui::Rounding::same(9.0))
                        .inner_margin(egui::Margin::same(2.0))
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.x = 2.0;
                            if picker_tab(
                                ui,
                                !tab_stickers,
                                lapinux_core::i18n::t("friends.emotes_tab"),
                            )
                            .clicked()
                            {
                                tab_stickers = false;
                            }
                            if picker_tab(
                                ui,
                                tab_stickers,
                                lapinux_core::i18n::t("friends.stickers_tab"),
                            )
                            .clicked()
                            {
                                tab_stickers = true;
                            }
                        });
                    let total = if tab_stickers {
                        stickers.len()
                    } else {
                        emoticons.len()
                    };
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(total.to_string())
                                .size(10.0)
                                .color(TEXT_DIM()),
                        );
                    });
                });
                ui.add_space(6.0);
                let (rule, _) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), 1.0),
                    egui::Sense::hover(),
                );
                ui.painter()
                    .rect_filled(rule, 0.0, with_alpha(BORDER(), 0.55));
                ui.add_space(6.0);
                let picker_scroll = egui::ScrollArea::vertical()
                    .id_salt(if tab_stickers {
                        "friend-stickers"
                    } else {
                        "friend-emotes"
                    })
                    .max_height(PICKER_GRID_H)
                    .auto_shrink([false, false])
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        if tab_stickers {
                            if stickers.is_empty() {
                                muted(ui, lapinux_core::i18n::t("friends.stickers_empty"));
                                return;
                            }
                            let grid_width = ui.available_width();
                            let columns = (grid_width / PICKER_MIN_CELL).floor().max(1.0);
                            let cell = ((grid_width - 0.5) / columns).max(PICKER_ROW_H);
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing = egui::vec2(0.0, 4.0);
                                for sticker in &stickers {
                                    let image = sticker_textures
                                        .get(&sticker.name)
                                        .and_then(|animation| animation.as_ref())
                                        .map(|animation| {
                                            let (texture, uv, _) = animation.frame_at(time);
                                            (texture, uv)
                                        });
                                    if media_tile(
                                        ui,
                                        cell,
                                        PICKER_ROW_H,
                                        image,
                                        sticker.name.clone(),
                                    )
                                    .clicked()
                                    {
                                        chosen_sticker = Some(sticker.name.clone());
                                    }
                                }
                            });
                        } else if emoticons.is_empty() {
                            muted(ui, lapinux_core::i18n::t("friends.emotes_empty"));
                        } else {
                            let grid_width = ui.available_width();
                            let columns = (grid_width / PICKER_MIN_CELL).floor().max(1.0);
                            let cell = ((grid_width - 0.5) / columns).max(PICKER_ROW_H);
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing = egui::vec2(0.0, 4.0);
                                for emote in &emoticons {
                                    let image = textures
                                        .get(&emote.name)
                                        .and_then(|texture| texture.as_ref())
                                        .map(|texture| {
                                            (
                                                texture.id(),
                                                egui::Rect::from_min_max(
                                                    egui::Pos2::ZERO,
                                                    egui::pos2(1.0, 1.0),
                                                ),
                                            )
                                        });
                                    if media_tile(
                                        ui,
                                        cell,
                                        PICKER_ROW_H,
                                        image,
                                        format!(":{}:", emote.name),
                                    )
                                    .clicked()
                                    {
                                        chosen_emote = Some(emote.name.clone());
                                    }
                                }
                            });
                        }
                    });
                // La grille garde trois rangées visibles, même quand un onglet
                // contient peu d'images : le compositeur ne remonte pas.
                let filler = (PICKER_GRID_H - picker_scroll.inner_rect.height()).max(0.0);
                ui.add_space(filler);
            },
        );
        ui.set_clip_rect(previous_clip);
        self.friends_picker_stickers = tab_stickers;
        if let Some(name) = chosen_emote {
            // Le brouillon garde un shortcode lisible `:nom:` ; la session
            // Steam le traduit en emoticon à l'envoi. L'emote s'insère au
            // curseur, le sélecteur se referme dans la même action et le
            // caret se place juste après le shortcode.
            apply_emote_choice(
                ui,
                &mut self.friends_message,
                &mut self.friends_emotes_open,
                &name,
            );
        } else if chosen_sticker.is_some() {
            self.friends_emotes_open = false;
        }
        if tab_stickers
            && stickers.iter().any(|sticker| {
                sticker_textures
                    .get(&sticker.name)
                    .and_then(|animation| animation.as_ref())
                    .is_some_and(StickerAnimation::animated)
            })
        {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(16));
        }
        ui.add_space(4.0);
        chosen_sticker
    }

    /// Fiche d'un ami : sa grande photo, sa présence et ce que la boutique
    /// publie. Steam ouvre aussi son profil public dans la webview.
    pub(super) fn friend_profile(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        row: &FriendRow,
    ) {
        let steam_id = row
            .key
            .strip_prefix("steam:")
            .and_then(|id| id.parse::<u64>().ok());
        // Toutes les informations que la présence sociale expose pour cet ami.
        let steam_friend = steam_id.and_then(|id| {
            self.social
                .as_ref()
                .and_then(|social| social.friends.iter().find(|friend| friend.steam_id == id))
                .cloned()
        });
        let presence = steam_friend
            .as_ref()
            .map(|friend| steam_presence_label(friend.presence))
            .unwrap_or_else(|| {
                if row.away {
                    lapinux_core::i18n::t("profile.away").to_string()
                } else if row.online {
                    lapinux_core::i18n::t("profile.online").to_string()
                } else {
                    lapinux_core::i18n::t("profile.offline").to_string()
                }
            });
        let game = steam_friend
            .as_ref()
            .map(|friend| friend.game.clone())
            .unwrap_or_else(|| row.game.clone());
        let game_appid = steam_friend
            .as_ref()
            .map(|friend| friend.game_appid)
            .unwrap_or(row.game_appid);
        let last_logon = steam_friend
            .as_ref()
            .map(|friend| friend.last_logon)
            .unwrap_or(0);
        let last_logoff = steam_friend
            .as_ref()
            .map(|friend| friend.last_logoff)
            .unwrap_or(row.last_seen);
        let (message_count, last_message_at) = steam_id
            .and_then(|id| {
                self.social
                    .as_ref()
                    .and_then(|social| social.chats.get(&id))
            })
            .map(|messages| {
                (
                    messages.len(),
                    messages.iter().map(|message| message.at).max().unwrap_or(0),
                )
            })
            .unwrap_or((0, 0));
        let recent_activity = steam_id
            .and_then(|id| {
                self.social.as_ref().and_then(|social| {
                    social
                        .friend_events
                        .iter()
                        .find(|event| event.steam_id == id)
                })
            })
            .map(|event| {
                use lapinux_core::social::SteamFriendEventKind;
                let game = if event.game.trim().is_empty() {
                    lapinux_core::i18n::t("notifications_panel.a_game").to_string()
                } else {
                    event.game.clone()
                };
                let caption = match event.kind {
                    SteamFriendEventKind::Online => {
                        lapinux_core::i18n::t("notifications_panel.friend_activity_online")
                            .to_string()
                    }
                    SteamFriendEventKind::Offline => {
                        lapinux_core::i18n::t("notifications_panel.friend_activity_offline")
                            .to_string()
                    }
                    SteamFriendEventKind::GameStarted => lapinux_core::i18n::t_args(
                        "notifications_panel.friend_activity_started",
                        &[("game", game.as_str())],
                    ),
                    SteamFriendEventKind::GameStopped => lapinux_core::i18n::t_args(
                        "notifications_panel.friend_activity_stopped",
                        &[("game", game.as_str())],
                    ),
                };
                format!(
                    "{caption} · {}",
                    lapinux_core::util::format_datetime(event.at)
                )
            });
        ui.add_space(2.0);
        egui::Frame::none()
            .fill(CARD())
            .stroke(egui::Stroke::new(1.0_f32, BORDER()))
            .rounding(egui::Rounding::same(14.0))
            .inner_margin(egui::Margin::same(18.0))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.vertical_centered(|ui| {
                    let size = 88.0;
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
                    let painter = ui.painter().clone();
                    painter.circle_filled(rect.center(), size / 2.0, CARD_ACTIVE());
                    painter.circle_stroke(
                        rect.center(),
                        size / 2.0 - 1.0,
                        egui::Stroke::new(2.0_f32, store_tint(row.store)),
                    );
                    match self.friend_avatar(ctx, &row.key, &row.avatar) {
                        Some(texture) => {
                            let image = rect.shrink(4.0);
                            egui::Image::new((texture.id(), image.size()))
                                .rounding(egui::Rounding::same((size - 8.0) / 2.0))
                                .paint_at(ui, image);
                        }
                        None => {
                            painter.text(
                                rect.center(),
                                egui::Align2::CENTER_CENTER,
                                row.name
                                    .chars()
                                    .next()
                                    .unwrap_or('?')
                                    .to_uppercase()
                                    .to_string(),
                                egui::FontId::proportional(30.0),
                                TEXT(),
                            );
                        }
                    }
                    ui.add_space(10.0);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&row.name)
                                .size(22.0)
                                .strong()
                                .color(TEXT()),
                        )
                        .truncate(),
                    );
                    ui.add_space(4.0);
                    friend_store_badge(ui, row.store);
                    ui.add_space(8.0);
                    friend_presence_line(ui, row);
                    if row.in_game && !row.state.trim().is_empty() {
                        ui.add_space(7.0);
                        egui::Frame::none()
                            .fill(CARD_ACTIVE())
                            .stroke(egui::Stroke::new(1.0_f32, BORDER()))
                            .rounding(egui::Rounding::same(8.0))
                            .inner_margin(egui::Margin::symmetric(9.0, 4.0))
                            .show(ui, |ui| {
                                ui.label(
                                    egui::RichText::new(lapinux_core::i18n::t_args(
                                        "friends.playing",
                                        &[("title", &row.state)],
                                    ))
                                    .size(11.5)
                                    .color(TEXT()),
                                );
                            });
                    }
                });
                ui.add_space(16.0);
                ui.separator();
                ui.add_space(12.0);
                ui.label(
                    egui::RichText::new(lapinux_core::i18n::t("friends.informations"))
                        .size(12.0)
                        .strong()
                        .color(TEXT()),
                );
                ui.add_space(8.0);
                egui::Frame::none()
                    .fill(PANEL())
                    .rounding(egui::Rounding::same(10.0))
                    .inner_margin(egui::Margin::symmetric(12.0, 10.0))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        let id = row
                            .key
                            .split_once(':')
                            .map(|(_, id)| id)
                            .unwrap_or(row.key.as_str());
                        friend_detail_row(
                            ui,
                            lapinux_core::i18n::t("friends.store"),
                            row.store.label(),
                        );
                        ui.add_space(5.0);
                        friend_detail_row(ui, lapinux_core::i18n::t("friends.identifier"), id);
                        ui.add_space(5.0);
                        friend_detail_row(ui, lapinux_core::i18n::t("friends.presence"), &presence);
                        if !game.trim().is_empty() {
                            ui.add_space(5.0);
                            friend_detail_row(
                                ui,
                                lapinux_core::i18n::t("friends.current_game"),
                                &game,
                            );
                        }
                        if game_appid != 0 {
                            ui.add_space(5.0);
                            friend_detail_row(
                                ui,
                                lapinux_core::i18n::t("friends.app_id"),
                                &game_appid.to_string(),
                            );
                        }
                        let last_connection = if last_logon > 0 {
                            lapinux_core::util::format_datetime(last_logon)
                        } else if steam_id.is_none() && row.last_seen > 0 {
                            lapinux_core::util::format_datetime(row.last_seen)
                        } else {
                            lapinux_core::i18n::t("friends.unknown").to_string()
                        };
                        ui.add_space(5.0);
                        friend_detail_row(
                            ui,
                            lapinux_core::i18n::t("friends.last_connection"),
                            &last_connection,
                        );
                        if steam_id.is_some() && last_logoff > 0 {
                            ui.add_space(5.0);
                            friend_detail_row(
                                ui,
                                lapinux_core::i18n::t("friends.last_disconnection"),
                                &lapinux_core::util::format_datetime(last_logoff),
                            );
                        }
                        if steam_id.is_some() {
                            ui.add_space(5.0);
                            friend_detail_row(
                                ui,
                                lapinux_core::i18n::t("friends.messages_count"),
                                &message_count.to_string(),
                            );
                            if last_message_at > 0 {
                                ui.add_space(5.0);
                                friend_detail_row(
                                    ui,
                                    lapinux_core::i18n::t("friends.last_message"),
                                    &lapinux_core::util::format_datetime(last_message_at),
                                );
                            }
                        }
                        if let Some(activity) = &recent_activity {
                            ui.add_space(5.0);
                            friend_detail_row(
                                ui,
                                lapinux_core::i18n::t("friends.recent_activity"),
                                activity,
                            );
                        }
                    });
                ui.add_space(4.0);
                if let Some(steam_id) = steam_id {
                    self.steam_profile_button(ui, row, steam_id);
                } else {
                    ui.add_space(14.0);
                    muted(ui, lapinux_core::i18n::t("friends.steam_only"));
                }
            });
    }

    /// Fenêtre de conversation : un en-tête compact, l'historique défilant et
    /// le compositeur, sans les détails de la fiche complète.
    fn friend_chat_panel(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, row: &FriendRow) {
        let steam_id = row
            .key
            .strip_prefix("steam:")
            .and_then(|id| id.parse::<u64>().ok());
        ui.add_space(2.0);
        // L'en-tête tient sur deux lignes : le nom, puis la boutique et la
        // présence. La photo de 40 px laisse le reste de la hauteur aux
        // messages, comme dans le croquis.
        egui::Frame::none()
            .fill(CARD())
            .stroke(egui::Stroke::new(1.0_f32, BORDER()))
            .rounding(egui::Rounding::same(12.0))
            .inner_margin(egui::Margin::symmetric(10.0, 8.0))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    let size = 40.0;
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
                    let painter = ui.painter().clone();
                    painter.circle_filled(rect.center(), size / 2.0, CARD_ACTIVE());
                    painter.circle_stroke(
                        rect.center(),
                        size / 2.0 - 1.0,
                        egui::Stroke::new(2.0_f32, store_tint(row.store)),
                    );
                    match self.friend_avatar(ctx, &row.key, &row.avatar) {
                        Some(texture) => {
                            let image = rect.shrink(2.0);
                            egui::Image::new((texture.id(), image.size()))
                                .rounding(egui::Rounding::same((size - 4.0) / 2.0))
                                .paint_at(ui, image);
                        }
                        None => {
                            painter.text(
                                rect.center(),
                                egui::Align2::CENTER_CENTER,
                                row.name
                                    .chars()
                                    .next()
                                    .unwrap_or('?')
                                    .to_uppercase()
                                    .to_string(),
                                egui::FontId::proportional(14.0),
                                TEXT(),
                            );
                        }
                    }
                    ui.add_space(10.0);
                    ui.vertical(|ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(&row.name)
                                    .size(15.5)
                                    .strong()
                                    .color(TEXT()),
                            )
                            .truncate(),
                        );
                        ui.add_space(2.0);
                        ui.horizontal(|ui| {
                            friend_store_badge(ui, row.store);
                            ui.add_space(6.0);
                            friend_presence_line(ui, row);
                            if !row.online {
                                ui.add_space(2.0);
                                ui.label(egui::RichText::new("·").size(10.0).color(TEXT_DIM()));
                                ui.add_space(2.0);
                                let last_seen = if row.last_seen > 0 {
                                    let date = lapinux_core::util::format_datetime(row.last_seen);
                                    lapinux_core::i18n::t_args(
                                        "friends.last_seen_at",
                                        &[("date", date.as_str())],
                                    )
                                } else {
                                    lapinux_core::i18n::t("friends.last_seen_unknown").to_string()
                                };
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(last_seen).size(10.0).color(TEXT_DIM()),
                                    )
                                    .truncate(),
                                );
                            }
                        });
                    });
                });
            });
        ui.add_space(6.0);
        if let Some(steam_id) = steam_id {
            // Hauteur visible restante dans le modal : la conversation la
            // remplit exactement, compositeur collé au bas.
            let available = (ui.clip_rect().bottom() - ui.cursor().top()).max(260.0);
            self.friend_conversation(ui, steam_id, available);
        } else {
            muted(ui, lapinux_core::i18n::t("friends.steam_only"));
        }
    }

    /// Le corps de la conversation : l'historique défile, le compositeur reste
    /// sous la main.
    fn friend_conversation(&mut self, ui: &mut egui::Ui, steam_id: u64, available: f32) {
        let mut messages = self
            .social
            .as_ref()
            .and_then(|social| social.chats.get(&steam_id))
            .cloned()
            .unwrap_or_default();
        // Un message sans texte n'a rien à montrer dans la chronologie.
        messages.retain(|message| !message.text.trim().is_empty());
        let history = self
            .social
            .as_ref()
            .and_then(|social| social.history.get(&steam_id))
            .cloned();
        let history_known = history.is_some();
        let loading = history.as_ref().is_some_and(|history| history.loading);
        let more_available = history
            .as_ref()
            .is_some_and(|history| history.more_available);
        let history_error = history.as_ref().and_then(|history| history.error.clone());
        // Toute fenêtre que Steam accepte encore de rendre est chargée
        // d'affilée : les pages rejoignent la même chronologie au lieu
        // d'attendre un défilement vers le haut.
        let can_extend = history.as_ref().is_some_and(|history| {
            history.more_available && !history.loading && history.error.is_none()
        });
        if can_extend {
            self.request_steam_history(&format!("steam:{steam_id}"), true);
        }
        let mut load_older = false;
        let mut send = false;
        let mut sticker_to_send: Option<String> = None;
        // Les images manquantes se téléchargent en fond : celles de la
        // conversation, et celles de l'onglet ouvert du sélecteur.
        let mut wanted: Vec<String> = Vec::new();
        let mut wanted_stickers: Vec<String> = Vec::new();
        for message in &messages {
            for token in chat_tokens(&message.text) {
                match token {
                    ChatToken::Emote(name) => wanted.push(name.to_string()),
                    ChatToken::Sticker(name) => wanted_stickers.push(name.to_string()),
                    ChatToken::Word(_) => {}
                }
            }
        }
        wanted.extend(chat_tokens(&self.friends_message).into_iter().filter_map(
            |token| match token {
                ChatToken::Emote(name) => Some(name.to_string()),
                ChatToken::Sticker(_) | ChatToken::Word(_) => None,
            },
        ));
        if self.friends_emotes_open {
            if let Some(social) = self.social.as_ref() {
                if self.friends_picker_stickers {
                    wanted_stickers
                        .extend(social.stickers.iter().map(|sticker| sticker.name.clone()));
                } else {
                    wanted.extend(social.emoticons.iter().map(|emote| emote.name.clone()));
                }
            }
        }
        self.ensure_emotes(ui.ctx(), wanted);
        self.ensure_stickers(ui.ctx(), wanted_stickers);
        let emoticons_available = self
            .social
            .as_ref()
            .is_some_and(|social| !social.emoticons.is_empty() || !social.stickers.is_empty());
        // La carte de conversation remplit la hauteur restante du modal. Le
        // viewport de l'historique a une hauteur exacte : la carte ne peut pas
        // déborder sous le compositeur, et un contenu court ne crée pas de
        // zone défilante.
        let layout = conversation_layout(available, self.friends_emotes_open);
        let picker_height = layout.picker_height;
        let composer_height = layout.composer_height;
        let inner_margin = layout.inner_margin;
        let card_height = layout.card_height;
        let history_height = layout.history_height;
        let gap = layout.gap;
        // Largeur intérieure réelle de la carte, mesurée avant que le moindre
        // enfant (historique, sélecteur) ne modifie l'espace disponible.
        let card_width = (ui.available_width() - 20.0).max(120.0);
        egui::Frame::none()
            .fill(CARD())
            .stroke(egui::Stroke::new(1.0_f32, BORDER()))
            .rounding(egui::Rounding::same(14.0))
            // 10 en haut, 6 en bas : le compositeur reste près du bord.
            .inner_margin(egui::Margin {
                left: 10.0,
                right: 10.0,
                top: 10.0,
                bottom: 6.0,
            })
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                // La hauteur de la carte est calculée au pixel : aucun
                // espacement implicite ne doit s'ajouter sous le compositeur.
                ui.spacing_mut().item_spacing.y = 0.0;
                let scroll_to_bottom = std::mem::take(&mut self.friends_chat_scroll_to_bottom);
                // Une hauteur exacte pour l'historique : la carte ne peut plus
                // dépasser sous le compositeur, et un contenu court ne crée
                // pas de zone défilante.
                let history_rect = egui::Rect::from_min_size(
                    ui.cursor().min,
                    egui::vec2(ui.available_width(), history_height),
                );
                let _ = ui.allocate_exact_size(history_rect.size(), egui::Sense::hover());
                let mut scroll_output = None;
                ui.scope_builder(egui::UiBuilder::new().max_rect(history_rect), |ui| {
                    // Une barre fine, opaque et placée dans une gouttière :
                    // elle ne recouvre plus les bulles ni l'autocollant.
                    ui.style_mut().spacing.scroll = egui::style::ScrollStyle::solid();
                    ui.style_mut().spacing.scroll.bar_width = 4.0;
                    ui.style_mut().spacing.scroll.bar_inner_margin = 5.0;
                    ui.style_mut().spacing.scroll.bar_outer_margin = 1.0;
                    ui.style_mut().visuals.widgets.inactive.bg_fill = with_alpha(TEXT_DIM(), 0.28);
                    ui.style_mut().visuals.widgets.hovered.bg_fill = with_alpha(TEXT_DIM(), 0.50);
                    ui.style_mut().visuals.widgets.active.bg_fill = with_alpha(ACCENT(), 0.65);
                    scroll_output = Some(
                        egui::ScrollArea::vertical()
                            .id_salt(("friend-chat-history", steam_id))
                            .max_height(history_height)
                            .auto_shrink([false, false])
                            .scroll_bar_visibility(
                                egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded,
                            )
                            .stick_to_bottom(self.friends_chat_autoscroll)
                            .show(ui, |ui| {
                                // Les messages s'arrêtent avant la gouttière :
                                // la barre reste à l'extérieur de leur zone.
                                ui.set_width((ui.available_width() - 14.0).max(120.0));
                                // Les bulles et les séparateurs gèrent déjà
                                // leur espacement : pas de double marge.
                                ui.spacing_mut().item_spacing.y = 0.0;
                                if messages.is_empty() {
                                    if loading {
                                        ui.add_space(6.0);
                                        ui.horizontal(|ui| {
                                            crate::section_icons::loading(ui, 18.0);
                                            muted(
                                                ui,
                                                lapinux_core::i18n::t("friends.history_loading"),
                                            );
                                        });
                                    } else if let Some(error) = &history_error {
                                        ui.add_space(6.0);
                                        ui.label(egui::RichText::new(error).size(10.5).color(WARN));
                                    } else if history_known {
                                        muted(ui, lapinux_core::i18n::t("friends.history_empty"));
                                        ui.add_space(4.0);
                                        muted(ui, lapinux_core::i18n::t("friends.history_hint"));
                                    } else {
                                        muted(ui, lapinux_core::i18n::t("friends.no_messages"));
                                    }
                                } else {
                                    if let Some(error) = &history_error {
                                        ui.label(egui::RichText::new(error).size(10.5).color(WARN));
                                        ui.add_space(4.0);
                                    }
                                    if more_available {
                                        // Le chargement automatique s'arme en remontant,
                                        // mais ce bouton reste utile quand le contenu
                                        // n'est pas encore assez haut pour défiler.
                                        ui.horizontal(|ui| {
                                            let response = ui.add(
                                                egui::Button::new(
                                                    egui::RichText::new(lapinux_core::i18n::t(
                                                        "friends.load_older",
                                                    ))
                                                    .size(10.5)
                                                    .color(TEXT_DIM()),
                                                )
                                                .fill(egui::Color32::TRANSPARENT)
                                                .stroke(egui::Stroke::new(1.0_f32, BORDER()))
                                                .rounding(egui::Rounding::same(7.0))
                                                .min_size(egui::vec2(0.0, 24.0)),
                                            );
                                            if response.clicked() && !loading {
                                                load_older = true;
                                            }
                                            if loading {
                                                ui.add_space(4.0);
                                                crate::section_icons::loading(ui, 18.0);
                                            }
                                        });
                                        ui.add_space(4.0);
                                    } else if history
                                        .as_ref()
                                        .is_some_and(|history| history.complete())
                                    {
                                        // Le haut de la chronologie est aussi la limite
                                        // de ce que Steam accepte encore de rendre.
                                        ui.add_space(4.0);
                                        egui::Frame::none()
                                            .fill(CARD_ACTIVE())
                                            .stroke(egui::Stroke::new(1.0_f32, BORDER()))
                                            .rounding(egui::Rounding::same(9.0))
                                            .inner_margin(egui::Margin::symmetric(10.0, 7.0))
                                            .show(ui, |ui| {
                                                ui.set_width(ui.available_width());
                                                ui.vertical_centered(|ui| {
                                                    ui.label(
                                                        egui::RichText::new(lapinux_core::i18n::t(
                                                            "friends.history_complete",
                                                        ))
                                                        .size(10.5)
                                                        .strong()
                                                        .color(TEXT_DIM()),
                                                    );
                                                    ui.add_space(2.0);
                                                    ui.label(
                                                        egui::RichText::new(lapinux_core::i18n::t(
                                                            "friends.history_limit",
                                                        ))
                                                        .size(9.5)
                                                        .color(TEXT_DIM()),
                                                    );
                                                });
                                            });
                                        ui.add_space(6.0);
                                    }
                                    let mut current_date: Option<String> = None;
                                    for message in &messages {
                                        if message.at > 0 {
                                            let date = lapinux_core::util::format_date(message.at);
                                            if current_date.as_deref() != Some(date.as_str()) {
                                                friend_date_separator(ui, &date);
                                                current_date = Some(date);
                                            }
                                        }
                                        let _ = friend_chat_bubble(
                                            ui,
                                            message,
                                            &self.emote_textures,
                                            &self.sticker_textures,
                                        );
                                    }
                                }
                                if scroll_to_bottom {
                                    ui.scroll_to_cursor(Some(egui::Align::BOTTOM));
                                }
                            }),
                    );
                });
                let scroll = match scroll_output {
                    Some(scroll) => scroll,
                    None => return,
                };
                let at_bottom = scroll.state.offset.y + scroll.inner_rect.height() + 8.0
                    >= scroll.content_size.y;
                if std::env::var_os("LAPINUX_LAYOUT_DEBUG").is_some() {
                    eprintln!(
                        "LAPINUX_LAYOUT available={available:.1} card={card_height:.1} \
                         history={history_height:.1} scroll_view={:.1} content={:.1} \
                         picker={picker_height:.1} composer={composer_height:.1} \
                         inner_margin={inner_margin:.1}",
                        scroll.inner_rect.height(),
                        scroll.content_size.y
                    );
                }
                let needs_jump = chat_needs_jump(
                    scroll.state.offset.y,
                    scroll.inner_rect.height(),
                    scroll.content_size.y,
                );
                self.friends_chat_autoscroll = at_bottom;
                if scroll.state.offset.y > 8.0 {
                    self.friends_history_top_armed = true;
                }
                if load_older {
                    self.friends_history_top_armed = false;
                    self.request_steam_history(&format!("steam:{steam_id}"), true);
                } else if scroll.state.offset.y <= 2.0
                    && !loading
                    && more_available
                    && self.friends_history_top_armed
                {
                    self.friends_history_top_armed = false;
                    self.request_steam_history(&format!("steam:{steam_id}"), true);
                }
                // Le bouton flotte au-dessus du bas de l'historique. Il ne
                // prend aucune place dans la conversation et n'apparaît que
                // lorsque des messages sont réellement cachés.
                if needs_jump {
                    let rect = egui::Rect::from_center_size(
                        egui::pos2(
                            scroll.inner_rect.center().x,
                            scroll.inner_rect.bottom() - 22.0,
                        ),
                        egui::vec2(32.0, 32.0),
                    );
                    // Peint à la main : `ui.put` peut faire grandir la carte
                    // d'une frame et décoller le compositeur du bas.
                    let jump = ui.interact(
                        rect,
                        ui.id().with(("friend-chat-jump", steam_id)),
                        egui::Sense::click(),
                    );
                    let hovered = jump.hovered();
                    let painter = ui.painter();
                    painter.rect_filled(
                        rect,
                        egui::Rounding::same(16.0),
                        if hovered { CARD_HOVER() } else { CARD_ACTIVE() },
                    );
                    painter.rect_stroke(
                        rect,
                        egui::Rounding::same(16.0),
                        egui::Stroke::new(1.0_f32, BORDER()),
                    );
                    painter.text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "↓",
                        egui::FontId::proportional(15.0),
                        TEXT(),
                    );
                    let jump = jump
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_text(lapinux_core::i18n::t("friends.scroll_to_bottom"));
                    if jump.clicked() {
                        self.friends_chat_autoscroll = true;
                        self.friends_chat_scroll_to_bottom = true;
                        ui.ctx().request_repaint();
                    }
                }
                ui.add_space(gap);
                // Un seul cadre : l'emote, le champ et l'envoi vivent dans la
                // même barre, et le compositeur ne bouge pas quand le
                // sélecteur s'ouvre.
                let composer = egui::Frame::none()
                    .fill(CARD_HOVER())
                    .stroke(egui::Stroke::new(1.0_f32, BORDER()))
                    .rounding(egui::Rounding::same(10.0))
                    .inner_margin(egui::Margin::symmetric(8.0, 5.0))
                    .show(ui, |ui| {
                        ui.set_width((card_width - 16.0).max(1.0));
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 6.0;
                                if emote_toggle_button(
                                    ui,
                                    self.friends_emotes_open,
                                    emoticons_available,
                                    24.0,
                                )
                                .on_hover_text(lapinux_core::i18n::t("friends.emotes"))
                                .clicked()
                                {
                                    self.friends_emotes_open = !self.friends_emotes_open;
                                }
                                let send_side = 24.0;
                                let input_width =
                                    (ui.available_width() - send_side - 6.0).max(60.0);
                                let input = draft_field(
                                    ui,
                                    &mut self.friends_message,
                                    &self.emote_textures,
                                    input_width,
                                )
                                .response;
                                if (input.has_focus() || input.lost_focus())
                                    && ui.input(|i| i.key_pressed(egui::Key::Enter))
                                {
                                    send = true;
                                }
                                if send_icon_button(
                                    ui,
                                    !self.friends_message.trim().is_empty(),
                                    send_side,
                                )
                                .on_hover_text(lapinux_core::i18n::t("friends.send"))
                                .clicked()
                                {
                                    send = true;
                                }
                            });
                        });
                    });
                // Le sélecteur flotte au-dessus du compositeur : il emprunte
                // exactement sa largeur et ne réserve aucune hauteur dans la
                // carte, donc la zone des messages ne remonte plus.
                let composer_rect = composer.response.rect;
                if self.friends_emotes_open {
                    let overlay_width = composer_rect.width();
                    let mut picked = None;
                    let ctx = ui.ctx().clone();
                    egui::Area::new(egui::Id::new(("friend-emote-picker", steam_id)))
                        .order(egui::Order::Foreground)
                        .fixed_pos(egui::pos2(
                            composer_rect.left(),
                            composer_rect.top() - PICKER_HEIGHT - 2.0,
                        ))
                        .show(&ctx, |ui| {
                            ui.set_min_width(overlay_width);
                            ui.set_max_width(overlay_width);
                            picked = self.emote_picker(ui, overlay_width);
                        });
                    if picked.is_some() {
                        sticker_to_send = picked;
                    }
                }
            });
        if let Some(name) = sticker_to_send {
            self.send_steam_sticker(steam_id, &name);
        }
        if send {
            self.send_steam_message(steam_id);
        }
    }

    /// Bouton d'ouverture du profil public Steam.
    fn steam_profile_button(&mut self, ui: &mut egui::Ui, row: &FriendRow, steam_id: u64) {
        ui.add_space(14.0);
        ui.vertical_centered(|ui| {
            let texture = store_mark_texture(
                ui.ctx(),
                lapinux_core::stores::Store::Steam,
                mark_pixels(ui.ctx(), 15.0),
            );
            let source = texture.size_vec2();
            let scale = 15.0 / source.x.max(source.y).max(1.0);
            let image = egui::Image::new((texture.id(), source * scale))
                .tint(store_tint(lapinux_core::stores::Store::Steam));
            let label = egui::RichText::new(lapinux_core::i18n::t("friends.open_profile"))
                .size(11.5)
                .color(TEXT());
            if ui
                .add(
                    egui::Button::image_and_text(image, label)
                        .fill(CARD_ACTIVE())
                        .stroke(egui::Stroke::new(1.0_f32, BORDER()))
                        .rounding(egui::Rounding::same(8.0))
                        .min_size(egui::vec2(0.0, 30.0)),
                )
                .clicked()
            {
                let url = format!("https://steamcommunity.com/profiles/{steam_id}");
                self.webview.open(
                    &self.paths,
                    &url,
                    &lapinux_core::i18n::t_args("friends.profile_window", &[("name", &row.name)]),
                );
            }
        });
    }
}

/// Ce qu'un clic sur la barre supérieure de la fiche demande.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FriendTopAction {
    None,
    Back,
    Profile,
    Messages,
}

/// Barre supérieure de la fiche : le retour aux amis collé à gauche, les
/// onglets Profil/Messages centrés sur toute la largeur du panneau.
fn friend_top_bar(ui: &mut egui::Ui, is_steam: bool, chat_open: bool) -> FriendTopAction {
    let height = 30.0;
    let (row, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::hover(),
    );
    let mut action = FriendTopAction::None;
    let back_label = lapinux_core::i18n::t("friends.back");
    let galley = ui.painter().layout_no_wrap(
        back_label.to_string(),
        egui::FontId::proportional(10.5),
        TEXT_DIM(),
    );
    let back_rect = egui::Rect::from_min_size(row.min, egui::vec2(galley.size().x + 20.0, height));
    if ui
        .put(
            back_rect,
            egui::Button::new(egui::RichText::new(back_label).size(10.5).color(TEXT_DIM()))
                .fill(egui::Color32::TRANSPARENT)
                .stroke(egui::Stroke::NONE)
                .rounding(egui::Rounding::same(7.0))
                .min_size(egui::vec2(galley.size().x + 20.0, height)),
        )
        .clicked()
    {
        action = FriendTopAction::Back;
    }
    if !is_steam {
        return action;
    }
    let tab_width = 96.0;
    let gap = 4.0;
    let group = tab_width * 2.0 + gap;
    let left = row.center().x - group / 2.0;
    let top = row.center().y - 14.0;
    for (index, (label, selected, next)) in [
        (
            lapinux_core::i18n::t("friends.profile_tab"),
            !chat_open,
            FriendTopAction::Profile,
        ),
        (
            lapinux_core::i18n::t("friends.messages_tab"),
            chat_open,
            FriendTopAction::Messages,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let rect = egui::Rect::from_min_size(
            egui::pos2(left + index as f32 * (tab_width + gap), top),
            egui::vec2(tab_width, 28.0),
        );
        let response = ui.put(
            rect,
            egui::Button::new(
                egui::RichText::new(label)
                    .size(12.0)
                    .strong()
                    .color(if selected { TEXT() } else { TEXT_DIM() }),
            )
            .fill(if selected {
                CARD_ACTIVE()
            } else {
                egui::Color32::TRANSPARENT
            })
            .stroke(if selected {
                egui::Stroke::new(1.0_f32, BORDER())
            } else {
                egui::Stroke::NONE
            })
            .rounding(egui::Rounding::same(8.0))
            .min_size(egui::vec2(tab_width, 28.0)),
        );
        if response.clicked() {
            action = next;
        }
    }
    action
}

/// Une note de la boutique qui explique l'absence de présence : le jeton du
/// launcher n'a pas la permission. Les anciennes notes françaises restent
/// reconnues, comme les nouvelles formulations anglaises.
fn is_presence_permission_note(note: &str) -> bool {
    let note = note.to_lowercase();
    note.contains("permission de présence")
        || note.contains("presence permission")
        || note.contains("presence not published")
}

/// Une bulle de conversation dessinée au trait, pour le bouton message
/// d'une ligne d'ami.
fn paint_message_icon(painter: &egui::Painter, center: egui::Pos2, ink: egui::Color32) {
    let stroke = egui::Stroke::new(1.2_f32, ink);
    let bubble =
        egui::Rect::from_center_size(center + egui::vec2(0.0, -1.0), egui::vec2(13.0, 10.0));
    painter.rect_stroke(bubble, egui::Rounding::same(3.2), stroke);
    painter.line_segment(
        [
            egui::pos2(center.x - 2.0, bubble.bottom() - 0.5),
            egui::pos2(center.x - 1.0, bubble.bottom() + 3.0),
        ],
        stroke,
    );
    painter.line_segment(
        [
            egui::pos2(center.x - 1.0, bubble.bottom() + 3.0),
            egui::pos2(center.x + 2.5, bubble.bottom() - 0.5),
        ],
        stroke,
    );
    for offset in [-3.0_f32, 0.0, 3.0] {
        painter.circle_filled(center + egui::vec2(offset, -1.0), 0.9, ink);
    }
}

/// Vrai quand des messages dépassent sous la vue : le bouton de retour au bas
/// n'a de sens que dans ce cas.
fn chat_needs_jump(offset_y: f32, viewport: f32, content: f32) -> bool {
    content > viewport + 1.0 && offset_y + viewport + 8.0 < content
}

/// Un séparateur de date dans la conversation : une pastille compacte
/// centrée, pour situer les messages sans répéter la date dans chaque bulle.
fn friend_date_separator(ui: &mut egui::Ui, date: &str) {
    ui.add_space(6.0);
    let galley = ui.painter().layout_no_wrap(
        date.to_string(),
        egui::FontId::proportional(10.0),
        TEXT_DIM(),
    );
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 20.0), egui::Sense::hover());
    let pill = egui::Rect::from_center_size(rect.center(), galley.size() + egui::vec2(20.0, 7.0));
    let painter = ui.painter();
    painter.rect_filled(pill, egui::Rounding::same(9.0), CARD_HOVER());
    painter.rect_stroke(
        pill,
        egui::Rounding::same(9.0),
        egui::Stroke::new(1.0_f32, BORDER()),
    );
    painter.galley(pill.center() - galley.size() / 2.0, galley, TEXT_DIM());
    ui.add_space(6.0);
}

/// Un morceau de message : un mot affiché tel quel, une emote reconnue ou un
/// autocollant.
enum ChatToken<'a> {
    Word(&'a str),
    Emote(&'a str),
    Sticker(&'a str),
}

/// Découpe un message en mots, shortcodes `:nom:`/`ːnomː`, balises
/// `[emoticon]nom[/emoticon]` et `[sticker type="nom"][/sticker]` que Steam
/// renvoie dans les messages et les historiques.
fn chat_tokens(text: &str) -> Vec<ChatToken<'_>> {
    let mut tokens = Vec::new();
    let mut rest = text;
    loop {
        let emote = rest.find("[emoticon]").map(|start| (start, false));
        let sticker = rest.find("[sticker ").map(|start| (start, true));
        let Some((start, sticker_tag)) = (match (emote, sticker) {
            (Some(emote), Some(sticker)) => {
                Some(if emote.0 <= sticker.0 { emote } else { sticker })
            }
            (Some(emote), None) => Some(emote),
            (None, Some(sticker)) => Some(sticker),
            (None, None) => None,
        }) else {
            push_chat_words(rest, &mut tokens);
            break;
        };
        push_chat_words(&rest[..start], &mut tokens);
        if sticker_tag {
            let Some(open_end) = rest[start..].find(']').map(|end| start + end + 1) else {
                push_chat_words(&rest[start..], &mut tokens);
                break;
            };
            let tag = &rest[start..open_end];
            let after = &rest[open_end..];
            if let Some(end) = after.find("[/sticker]") {
                if let Some(name) = sticker_name_from_tag(tag) {
                    tokens.push(ChatToken::Sticker(name));
                    rest = &after[end + "[/sticker]".len()..];
                    continue;
                }
            }
            // Balise incomplète ou nom invalide : elle reste du texte.
            push_chat_words(tag, &mut tokens);
            rest = after;
            continue;
        }
        let after = &rest[start + "[emoticon]".len()..];
        let Some(end) = after.find("[/emoticon]") else {
            push_chat_words(&rest[start..], &mut tokens);
            break;
        };
        let name = &after[..end];
        if is_emote_name(name) {
            tokens.push(ChatToken::Emote(name));
        } else {
            push_chat_words(
                &rest[start..start + "[emoticon]".len() + end + "[/emoticon]".len()],
                &mut tokens,
            );
        }
        rest = &after[end + "[/emoticon]".len()..];
    }
    tokens
}

/// Nom porté par l'attribut `type` d'une balise `[sticker]`.
fn sticker_name_from_tag(tag: &str) -> Option<&str> {
    let start = tag.find("type=\"")? + "type=\"".len();
    let end = tag[start..].find('"')?;
    let name = &tag[start..start + end];
    is_sticker_name(name).then_some(name)
}

fn push_chat_words<'a>(text: &'a str, tokens: &mut Vec<ChatToken<'a>>) {
    let punctuation = [',', '.', '!', '?', ';', ')', '(', '"', '\''];
    for word in text.split_whitespace() {
        let trimmed = word.trim_matches(|character: char| punctuation.contains(&character));
        match emote_shortcode(trimmed) {
            Some(name) => {
                let start = word.find(trimmed).unwrap_or(0);
                let (prefix, rest) = word.split_at(start);
                if !prefix.is_empty() {
                    tokens.push(ChatToken::Word(prefix));
                }
                tokens.push(ChatToken::Emote(name));
                let suffix = &rest[trimmed.len()..];
                if !suffix.is_empty() {
                    tokens.push(ChatToken::Word(suffix));
                }
            }
            None => tokens.push(ChatToken::Word(word)),
        }
    }
}

fn is_emote_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 48
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
}

fn is_sticker_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name
            .chars()
            .any(|character| character.is_control() || matches!(character, '[' | ']' | '"'))
}

/// Taille d'une image dans l'atlas d'un autocollant. Le sticker est peint à
/// 112 px dans une bulle et à 42 px dans le sélecteur ; 80 px suffisent à
/// garder un contour net sans stocker 65 images en 150×150.
const STICKER_ATLAS_FRAME: u32 = 80;

/// Décode toutes les images d'un autocollant. Steam les publie en APNG
/// animé ; GIF et WebP animés passent par le même chemin. Une image fixe
/// donne un atlas d'une seule image.
fn decode_sticker_frames(bytes: &[u8]) -> Option<StickerFrames> {
    use image::AnimationDecoder;
    use std::io::Cursor;

    let mut decoded: Vec<(image::RgbaImage, f32)> = Vec::new();
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        if let Ok(decoder) = image::codecs::png::PngDecoder::new(Cursor::new(bytes)) {
            if decoder.is_apng().unwrap_or(false) {
                if let Ok(apng) = decoder.apng() {
                    if let Ok(frames) = apng.into_frames().collect_frames() {
                        decoded = sticker_frame_images(frames);
                    }
                }
            }
        }
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        if let Ok(decoder) = image::codecs::gif::GifDecoder::new(Cursor::new(bytes)) {
            if let Ok(frames) = decoder.into_frames().collect_frames() {
                decoded = sticker_frame_images(frames);
            }
        }
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        if let Ok(decoder) = image::codecs::webp::WebPDecoder::new(Cursor::new(bytes)) {
            if decoder.has_animation() {
                if let Ok(frames) = decoder.into_frames().collect_frames() {
                    decoded = sticker_frame_images(frames);
                }
            }
        }
    }
    if decoded.is_empty() {
        let image = image::load_from_memory(bytes).ok()?.to_rgba8();
        decoded.push((image, 0.0));
    }
    sticker_atlas(decoded)
}

fn sticker_frame_images(frames: Vec<image::Frame>) -> Vec<(image::RgbaImage, f32)> {
    frames
        .into_iter()
        .map(|frame| {
            let (numerator, denominator) = frame.delay().numer_denom_ms();
            let delay = if denominator == 0 {
                0.1
            } else {
                numerator as f32 / denominator as f32 / 1000.0
            };
            (frame.into_buffer(), delay.clamp(0.02, 5.0))
        })
        .collect()
}

fn sticker_atlas(frames: Vec<(image::RgbaImage, f32)>) -> Option<StickerFrames> {
    if frames.is_empty() {
        return None;
    }
    let count = u32::try_from(frames.len()).ok()?;
    let columns = count.min(8);
    let rows = (count + columns - 1) / columns;
    let size = STICKER_ATLAS_FRAME;
    let width = columns * size;
    let height = rows * size;
    let mut pixels = vec![0_u8; width as usize * height as usize * 4];
    let mut delays = Vec::with_capacity(frames.len());
    for (index, (frame, delay)) in frames.iter().enumerate() {
        let scaled = image::imageops::thumbnail(frame, size, size);
        let tile_x = (index as u32 % columns) * size;
        let tile_y = (index as u32 / columns) * size;
        for (x, y, pixel) in scaled.enumerate_pixels() {
            let offset = ((tile_y + y) as usize * width as usize + (tile_x + x) as usize) * 4;
            pixels[offset..offset + 4].copy_from_slice(&pixel.0);
        }
        delays.push(*delay);
    }
    Some(StickerFrames {
        width,
        height,
        columns,
        rows,
        pixels,
        delays,
    })
}

/// Le nom d'une emote si le mot est un shortcode complet. Le sélecteur envoie
/// la forme ASCII `:nom:` ; les anciens messages et les balises BBCode
/// peuvent aussi porter le deux-points modificatif `ː` (U+02D0). Les noms
/// Steam sont des identifiants ASCII, ce qui laisse les heures et la
/// ponctuation ordinaires intactes.
fn emote_shortcode(word: &str) -> Option<&str> {
    let name = word.strip_prefix(':').or_else(|| word.strip_prefix('ː'))?;
    let name = name.strip_suffix(':').or_else(|| name.strip_suffix('ː'))?;
    is_emote_name(name).then_some(name)
}

/// Hauteur du compositeur : une seule rangée, même quand le brouillon
/// contient une emote.
const COMPOSER_BASE_HEIGHT: f32 = 34.0;
/// Hauteur du champ de saisie, alignée sur les boutons de la barre.
const COMPOSER_FIELD_HEIGHT: f32 = 24.0;
/// Côté de l'image d'emote peinte dans le champ, comme dans les bulles.
const DRAFT_EMOTE_SIDE: f32 = 20.0;

/// Shortcodes d'emote du brouillon, avec leur plage d'octets dans le texte.
fn draft_emote_ranges(text: &str) -> Vec<(std::ops::Range<usize>, &str)> {
    let punctuation = [',', '.', '!', '?', ';', ')', '(', '"', '\''];
    let mut ranges = Vec::new();
    let mut start = None;
    for (index, character) in text.char_indices() {
        if character.is_whitespace() {
            if let Some(word_start) = start.take() {
                push_draft_emote(text, word_start..index, &punctuation, &mut ranges);
            }
        } else if start.is_none() {
            start = Some(index);
        }
    }
    if let Some(word_start) = start {
        push_draft_emote(text, word_start..text.len(), &punctuation, &mut ranges);
    }
    ranges
}

fn push_draft_emote<'a>(
    text: &'a str,
    word: std::ops::Range<usize>,
    punctuation: &[char],
    ranges: &mut Vec<(std::ops::Range<usize>, &'a str)>,
) {
    let raw = &text[word.clone()];
    let trimmed = raw.trim_matches(|character: char| punctuation.contains(&character));
    if let Some(name) = emote_shortcode(trimmed) {
        let offset = word.start + raw.find(trimmed).unwrap_or(0);
        ranges.push((offset..offset + trimmed.len(), name));
    }
}

/// Le texte du champ, avec les shortcodes d'emotes déjà chargées rendus
/// transparents : l'image est peinte par-dessus, à leur place exacte. Le
/// texte réel du champ garde le shortcode, donc le curseur, la sélection et
/// l'envoi restent ceux d'un champ ordinaire.
fn draft_layout_job(
    text: &str,
    emotes: &std::collections::HashMap<String, Option<egui::TextureHandle>>,
    font: egui::FontId,
    measure: &dyn Fn(&str) -> f32,
) -> egui::text::LayoutJob {
    let normal = egui::TextFormat {
        font_id: font.clone(),
        color: egui::Color32::PLACEHOLDER,
        ..Default::default()
    };
    let hidden = egui::TextFormat {
        font_id: font,
        color: egui::Color32::TRANSPARENT,
        ..Default::default()
    };
    let single_line = !text.contains('\n');
    let mut job = egui::text::LayoutJob::default();
    let mut last = 0;
    for (range, name) in draft_emote_ranges(text) {
        if range.start > last {
            job.append(&text[last..range.start], 0.0, normal.clone());
        }
        let run = &text[range.clone()];
        let loaded = single_line && emotes.get(name).and_then(Option::as_ref).is_some();
        let format = if loaded {
            // La chasse du shortcode est ramenée à la largeur de l'image :
            // sans cela, l'emote flotte au milieu d'un vide de la taille du
            // texte masqué.
            let mut format = hidden.clone();
            let characters = run.chars().count();
            if characters > 1 {
                let natural = measure(run);
                format.extra_letter_spacing =
                    (DRAFT_EMOTE_SIDE - natural) / (characters - 1) as f32;
            }
            format
        } else {
            normal.clone()
        };
        job.append(run, 0.0, format);
        last = range.end;
    }
    if last < text.len() {
        job.append(&text[last..], 0.0, normal.clone());
    }
    if job.sections.is_empty() {
        // Une section vide réserve la hauteur d'une ligne : sans elle, la
        // galley vide n'a aucune ligne et le caret partait du milieu du
        // champ, une demi-ligne trop bas.
        job.append("", 0.0, normal);
    }
    job
}

/// Peint chaque emote chargée sur son shortcode masqué, sur la ligne du
/// champ. Les images sont découpées au rectangle visible du champ pour
/// suivre son défilement horizontal.
fn paint_draft_emotes(
    ui: &egui::Ui,
    galley: &egui::Galley,
    galley_pos: egui::Pos2,
    clip: egui::Rect,
    emotes: &std::collections::HashMap<String, Option<egui::TextureHandle>>,
) {
    let text = galley.text();
    if text.contains('\n') {
        return;
    }
    let Some(row) = galley.rows.first() else {
        return;
    };
    let mut xs: Vec<f32> = row.glyphs.iter().map(|glyph| glyph.pos.x).collect();
    let end_x = galley.size().x;
    xs.push(end_x);
    let painter = ui.painter().with_clip_rect(clip);
    let center_y = galley_pos.y + row.rect.center().y;
    for (range, name) in draft_emote_ranges(text) {
        let Some(texture) = emotes.get(name).and_then(Option::as_ref) else {
            continue;
        };
        let start = text[..range.start].chars().count();
        let end = text[..range.end].chars().count();
        let x0 = xs.get(start).copied().unwrap_or(end_x);
        let x1 = xs.get(end).copied().unwrap_or(end_x);
        if x1 - x0 < 2.0 {
            continue;
        }
        let side = (x1 - x0).clamp(10.0, DRAFT_EMOTE_SIDE + 2.0);
        let rect = egui::Rect::from_center_size(
            egui::pos2(galley_pos.x + (x0 + x1) / 2.0, center_y),
            egui::vec2(side, side),
        );
        painter.image(
            texture.id(),
            rect,
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    }
}

/// Identifiant stable du champ de saisie : le sélecteur d'emotes y replace
/// le curseur après avoir inséré un shortcode.
fn message_field_id() -> egui::Id {
    egui::Id::new("friends-message-field")
}

/// Convertit un index de caractère en index d'octet dans `text`.
fn char_to_byte(text: &str, character: usize) -> usize {
    text.char_indices()
        .nth(character)
        .map_or(text.len(), |(byte, _)| byte)
}

/// Insère le shortcode d'une emote au curseur du champ (ou remplace la
/// sélection), puis replace le curseur juste après l'insertion et rend le
/// focus au champ. Renvoie l'index du caret après insertion.
fn insert_emote_at_cursor(ui: &egui::Ui, text: &mut String, name: &str) -> usize {
    let id = message_field_id();
    let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
    let characters = text.chars().count();
    let range = state
        .cursor
        .char_range()
        .unwrap_or_else(|| egui::text::CCursorRange::one(egui::text::CCursor::new(characters)));
    let start = range
        .primary
        .index
        .min(range.secondary.index)
        .min(characters);
    let end = range
        .primary
        .index
        .max(range.secondary.index)
        .min(characters);
    let mut insertion = String::new();
    let before = &text[..char_to_byte(text, start)];
    if start > 0
        && before
            .chars()
            .last()
            .is_some_and(|character| !character.is_whitespace())
    {
        insertion.push(' ');
    }
    insertion.push_str(&format!(":{name}: "));
    let caret = start + insertion.chars().count();
    text.replace_range(
        char_to_byte(text, start)..char_to_byte(text, end),
        &insertion,
    );
    state
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::one(
            egui::text::CCursor::new(caret),
        )));
    state.store(ui.ctx(), id);
    ui.memory_mut(|memory| memory.request_focus(id));
    ui.ctx().request_repaint();
    caret
}

/// Un choix dans le sélecteur : il se referme dans la même action, l'emote
/// s'insère au curseur et le caret reste juste après le shortcode.
fn apply_emote_choice(
    ui: &egui::Ui,
    text: &mut String,
    picker_open: &mut bool,
    name: &str,
) -> usize {
    *picker_open = false;
    insert_emote_at_cursor(ui, text, name)
}

/// Géométrie rendue par le champ : le caret que le widget dessine et le point
/// d'insertion calculé sur la même galley. Les tests headless comparent les
/// deux sans passer par une capture.
#[cfg_attr(not(test), allow(dead_code))]
struct DraftFieldOutput {
    response: egui::Response,
    caret: Option<egui::Rect>,
    insertion: Option<egui::Rect>,
    galley_pos: egui::Pos2,
    row_height: f32,
}

/// Champ de saisie du chat : le texte est centré dans la rangée et les emotes
/// chargées remplacent leur shortcode, sans aperçu séparé ni hauteur en plus.
fn draft_field(
    ui: &mut egui::Ui,
    text: &mut String,
    emotes: &std::collections::HashMap<String, Option<egui::TextureHandle>>,
    width: f32,
) -> DraftFieldOutput {
    let font = egui::TextStyle::Body.resolve(ui.style());
    // Le placeholder disparaît dès que le champ a le focus : le curseur ne
    // clignote plus par-dessus le texte d'aide.
    let focused = ui.memory(|memory| memory.has_focus(message_field_id()));
    let hint = if focused {
        String::new()
    } else {
        lapinux_core::i18n::t("friends.message_placeholder").to_string()
    };
    let row_height = ui.fonts(|fonts| fonts.row_height(&font));
    let pad = ((COMPOSER_FIELD_HEIGHT - row_height) / 2.0).max(0.0);
    let mut layouter = |ui: &egui::Ui, text: &str, _wrap_width: f32| {
        let measure = |run: &str| {
            ui.fonts(|fonts| {
                fonts
                    .layout_no_wrap(run.to_string(), font.clone(), egui::Color32::WHITE)
                    .size()
                    .x
            })
        };
        let mut job = draft_layout_job(text, emotes, font.clone(), &measure);
        // Champ mono-ligne : le texte ne se replie jamais, il défile
        // horizontalement. Sans cela, un brouillon long se replierait dans le
        // champ et le caret suivrait une ligne cachée au lieu de défiler.
        job.wrap.max_width = f32::INFINITY;
        ui.fonts(|fonts| fonts.layout_job(job))
    };
    let output = egui::TextEdit::singleline(text)
        .id(message_field_id())
        .hint_text(hint)
        .frame(false)
        .margin(egui::Margin::symmetric(4.0, pad))
        .vertical_align(egui::Align::Center)
        .desired_width(width)
        .min_size(egui::vec2(width, COMPOSER_FIELD_HEIGHT))
        .layouter(&mut layouter)
        .show(ui);
    paint_draft_emotes(
        ui,
        &output.galley,
        output.galley_pos,
        output.text_clip_rect,
        emotes,
    );
    // Même galley et même position que le widget : ce rectangle est celui que
    // `TextEdit` peint réellement, pas une reconstruction parallèle.
    let cursor = output.cursor_range.map(|range| range.primary);
    let caret = cursor.map(|cursor| {
        egui::text_selection::text_cursor_state::cursor_rect(
            output.galley_pos,
            &output.galley,
            &cursor,
            row_height,
        )
    });
    let insertion = cursor.map(|cursor| {
        output
            .galley
            .pos_from_cursor(&cursor)
            .translate(output.galley_pos.to_vec2())
    });
    DraftFieldOutput {
        response: output.response,
        caret,
        insertion,
        galley_pos: output.galley_pos,
        row_height,
    }
}

const BUBBLE_TEXT_SIZE: f32 = 12.5;
const BUBBLE_TIME_SIZE: f32 = 9.5;
const BUBBLE_PAD_X: f32 = 13.0;
const BUBBLE_PAD_Y: f32 = 10.0;
const BUBBLE_EMOTE: f32 = 28.0;
const BUBBLE_STICKER: f32 = 112.0;
/// Une emote seule est un vrai média, pas un petit caractère dans un cadre.
const BUBBLE_EMOTE_MEDIA: f32 = 44.0;
/// Un autocollant seul flotte dans la conversation, à la manière de Steam.
const BUBBLE_STICKER_MEDIA: f32 = 128.0;
const BUBBLE_LINE_H: f32 = 18.5;
const BUBBLE_LINE_GAP: f32 = 2.5;
const BUBBLE_TOKEN_GAP: f32 = 5.0;
const BUBBLE_TIME_H: f32 = 12.0;
const BUBBLE_RADIUS: f32 = 14.0;

/// Rangée du sélecteur : 40 px de haut pour trois rangées visibles. La
/// largeur des cellules est calculée pour remplir exactement le panneau.
const PICKER_ROW_H: f32 = 40.0;
const PICKER_MIN_CELL: f32 = 46.0;
const PICKER_GRID_H: f32 = 128.0;
/// Hauteur du panneau : en-tête, filet, trois rangées de tuiles, marges et
/// l'espace de 4 px qui le sépare du compositeur.
const PICKER_HEIGHT: f32 = 193.0;

/// Un mot ou une emote déjà mesuré. La bulle est peinte à la main : sa
/// largeur reste celle du contenu, jamais celle de la zone de conversation.
struct BubblePiece {
    galley: Option<std::sync::Arc<egui::Galley>>,
    texture: Option<egui::TextureId>,
    animation: Option<StickerAnimation>,
    width: f32,
    height: f32,
}

struct BubbleLine {
    pieces: Vec<BubblePiece>,
    width: f32,
    /// Hauteur de la ligne : celle du texte, ou celle d'un autocollant plus
    /// grand qui y est posé.
    height: f32,
}

struct BubbleLayout {
    lines: Vec<BubbleLine>,
    width: f32,
    height: f32,
}

impl BubbleLayout {
    /// Délai avant la prochaine image d'un autocollant animé présent dans la
    /// bulle, afin de ne réveiller l'interface que lorsque l'image change.
    fn next_frame_in(&self, time: f32) -> Option<f32> {
        self.lines
            .iter()
            .flat_map(|line| &line.pieces)
            .filter_map(|piece| piece.animation.as_ref())
            .filter(|animation| animation.animated())
            .map(|animation| animation.frame_at(time).2)
            .reduce(f32::min)
    }
}

fn bubble_piece(
    ui: &egui::Ui,
    token: ChatToken<'_>,
    emotes: &std::collections::HashMap<String, Option<egui::TextureHandle>>,
    stickers: &std::collections::HashMap<String, Option<StickerAnimation>>,
    media_only: bool,
) -> BubblePiece {
    let emote_size = if media_only {
        BUBBLE_EMOTE_MEDIA
    } else {
        BUBBLE_EMOTE
    };
    let sticker_size = if media_only {
        BUBBLE_STICKER_MEDIA
    } else {
        BUBBLE_STICKER
    };
    match token {
        ChatToken::Emote(name) => match emotes.get(name).and_then(|texture| texture.as_ref()) {
            Some(texture) => BubblePiece {
                galley: None,
                texture: Some(texture.id()),
                animation: None,
                width: emote_size,
                height: emote_size,
            },
            None => {
                let galley = ui.painter().layout_no_wrap(
                    format!(":{name}:"),
                    egui::FontId::proportional(BUBBLE_TEXT_SIZE),
                    TEXT(),
                );
                BubblePiece {
                    width: galley.size().x,
                    height: galley.size().y,
                    galley: Some(galley),
                    texture: None,
                    animation: None,
                }
            }
        },
        ChatToken::Sticker(name) => {
            match stickers.get(name).and_then(|animation| animation.as_ref()) {
                Some(animation) => BubblePiece {
                    galley: None,
                    texture: Some(animation.texture.id()),
                    animation: Some(animation.clone()),
                    width: sticker_size,
                    height: sticker_size,
                },
                None => {
                    let galley = ui.painter().layout_no_wrap(
                        format!("[{name}]"),
                        egui::FontId::proportional(BUBBLE_TEXT_SIZE),
                        TEXT_DIM(),
                    );
                    BubblePiece {
                        width: galley.size().x,
                        height: galley.size().y,
                        galley: Some(galley),
                        texture: None,
                        animation: None,
                    }
                }
            }
        }
        ChatToken::Word(word) => {
            let galley = ui.painter().layout_no_wrap(
                word.to_string(),
                egui::FontId::proportional(BUBBLE_TEXT_SIZE),
                TEXT(),
            );
            BubblePiece {
                width: galley.size().x,
                height: galley.size().y,
                galley: Some(galley),
                texture: None,
                animation: None,
            }
        }
    }
}

/// Mesure les mots et les emotes en respectant la largeur maximale de la
/// bulle. Une ligne vide garde sa hauteur, comme un paragraphe.
fn bubble_layout(
    ui: &egui::Ui,
    text: &str,
    emotes: &std::collections::HashMap<String, Option<egui::TextureHandle>>,
    stickers: &std::collections::HashMap<String, Option<StickerAnimation>>,
    max_content_width: f32,
    media_only: bool,
) -> BubbleLayout {
    let mut lines: Vec<BubbleLine> = Vec::new();
    for raw_line in text.split('\n') {
        let mut line = BubbleLine {
            pieces: Vec::new(),
            width: 0.0,
            height: BUBBLE_LINE_H,
        };
        for token in chat_tokens(raw_line) {
            let piece = bubble_piece(ui, token, emotes, stickers, media_only);
            if !line.pieces.is_empty()
                && line.width + BUBBLE_TOKEN_GAP + piece.width > max_content_width
            {
                lines.push(line);
                line = BubbleLine {
                    pieces: Vec::new(),
                    width: 0.0,
                    height: BUBBLE_LINE_H,
                };
            }
            if !line.pieces.is_empty() {
                line.width += BUBBLE_TOKEN_GAP;
            }
            line.width += piece.width;
            line.height = line.height.max(piece.height);
            line.pieces.push(piece);
        }
        lines.push(line);
    }
    let width = lines.iter().map(|line| line.width).fold(0.0_f32, f32::max);
    let height = lines.iter().map(|line| line.height).sum::<f32>()
        + lines.len().saturating_sub(1) as f32 * BUBBLE_LINE_GAP;
    BubbleLayout {
        lines,
        width,
        height,
    }
}

/// Bouton du sélecteur d'emotes, dessiné au trait pour ne pas dépendre d'un
/// glyphe absent de la police. Sa taille s'adapte à la barre de saisie.
/// Un onglet du sélecteur, en contrôle segmenté : le choix actif porte le fond
/// et le liseré d'accent, l'autre reste discret.
fn picker_tab(ui: &mut egui::Ui, selected: bool, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(label).size(11.0).color(if selected {
            TEXT()
        } else {
            TEXT_DIM()
        }))
        .fill(if selected {
            CARD_ACTIVE()
        } else {
            egui::Color32::TRANSPARENT
        })
        .stroke(if selected {
            egui::Stroke::new(1.0_f32, with_alpha(ACCENT(), 0.32))
        } else {
            egui::Stroke::NONE
        })
        .rounding(egui::Rounding::same(7.0))
        .min_size(egui::vec2(0.0, 24.0)),
    )
}

/// Une tuile du sélecteur, dessinée sans le padding des boutons egui : sa
/// largeur est calculée pour remplir exactement le panneau.
fn media_tile(
    ui: &mut egui::Ui,
    width: f32,
    height: f32,
    image: Option<(egui::TextureId, egui::Rect)>,
    tooltip: String,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
    let painter = ui.painter();
    // Le fond de survol garde une petite respiration entre deux cellules.
    let background = rect.shrink2(egui::vec2(2.0, 0.0));
    if response.hovered() {
        painter.rect_filled(
            background,
            egui::Rounding::same(8.0),
            with_alpha(ACCENT(), 0.16),
        );
        painter.rect_stroke(
            background,
            egui::Rounding::same(8.0),
            egui::Stroke::new(1.0_f32, with_alpha(ACCENT(), 0.5)),
        );
    }
    match image {
        Some((texture, uv)) => {
            let image =
                egui::Rect::from_center_size(rect.center(), egui::vec2(height - 6.0, height - 6.0));
            painter.image(texture, image, uv, egui::Color32::WHITE);
        }
        None => {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "…",
                egui::FontId::proportional(13.0),
                TEXT_DIM(),
            );
        }
    }
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(tooltip)
}

fn emote_toggle_button(ui: &mut egui::Ui, open: bool, enabled: bool, size: f32) -> egui::Response {
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), sense);
    let painter = ui.painter();
    let ink = if !enabled {
        TEXT_DIM().gamma_multiply(0.55)
    } else if open || response.hovered() {
        ACCENT()
    } else {
        TEXT_DIM()
    };
    if open {
        painter.rect_filled(rect, egui::Rounding::same(8.0), CARD_ACTIVE());
        painter.rect_stroke(
            rect,
            egui::Rounding::same(8.0),
            egui::Stroke::new(1.0_f32, BORDER()),
        );
    }
    let center = rect.center();
    let stroke = (size / 24.0).max(0.75);
    painter.circle_stroke(center, size * 0.27, egui::Stroke::new(stroke, ink));
    let eye = size * 0.035;
    painter.circle_filled(center + egui::vec2(-size * 0.093, -size * 0.067), eye, ink);
    painter.circle_filled(center + egui::vec2(size * 0.093, -size * 0.067), eye, ink);
    let smile: Vec<egui::Pos2> = (0..=8)
        .map(|step| {
            let angle = std::f32::consts::PI * (0.18 + 0.64 * step as f32 / 8.0);
            center + egui::vec2(angle.cos(), angle.sin()) * (size * 0.147)
        })
        .collect();
    painter.add(egui::Shape::line(smile, egui::Stroke::new(stroke, ink)));
    response
}

/// Icône d'envoi : un petit avion de papier, dessiné pour rester lisible dans
/// la barre de saisie et ne pas dépendre d'un glyphe.
fn send_icon_button(ui: &mut egui::Ui, enabled: bool, size: f32) -> egui::Response {
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), sense);
    let ink = if !enabled {
        TEXT_DIM().gamma_multiply(0.5)
    } else if response.hovered() {
        TEXT()
    } else {
        ACCENT()
    };
    let painter = ui.painter();
    let s = rect.width().min(rect.height());
    let origin = rect.min;
    let tip = origin + egui::vec2(s * 0.82, s * 0.18);
    let tail = origin + egui::vec2(s * 0.16, s * 0.50);
    let fold = origin + egui::vec2(s * 0.52, s * 0.52);
    let bottom = origin + egui::vec2(s * 0.58, s * 0.84);
    painter.add(egui::Shape::convex_polygon(
        vec![tail, tip, bottom],
        ink,
        egui::Stroke::NONE,
    ));
    painter.line_segment([tail, fold], egui::Stroke::new(1.0_f32, CARD_HOVER()));
    painter.line_segment([fold, tip], egui::Stroke::new(1.0_f32, CARD_HOVER()));
    response
}

/// Géométrie de la carte de conversation. Le sélecteur flotte au-dessus du
/// compositeur et ne réserve aucune hauteur dans la carte.
struct ConversationLayout {
    card_height: f32,
    history_height: f32,
    picker_height: f32,
    composer_height: f32,
    gap: f32,
    inner_margin: f32,
}

fn conversation_layout(available: f32, picker_open: bool) -> ConversationLayout {
    // Onglets + filet + trois rangées + marges : le sélecteur garde une
    // hauteur fixe pour que le compositeur ne bouge pas quand on change
    // d'onglet.
    let picker_height = if picker_open { PICKER_HEIGHT } else { 0.0 };
    let composer_height = COMPOSER_BASE_HEIGHT;
    let gap = 6.0;
    // Marge de la Frame : 10 en haut, 6 en bas.
    let inner_margin = 16.0;
    let card_height = (available - 2.0).max(0.0);
    let history_height = (card_height - inner_margin - gap - composer_height).max(0.0);
    ConversationLayout {
        card_height,
        history_height,
        picker_height,
        composer_height,
        gap,
        inner_margin,
    }
}

/// Peint le contenu mesuré d'une bulle : mots et emotes, ligne par ligne.
fn paint_chat_content(
    painter: &egui::Painter,
    layout: &BubbleLayout,
    origin: egui::Pos2,
    time: f32,
) {
    let mut y = origin.y;
    for line in &layout.lines {
        let mut x = origin.x;
        for piece in &line.pieces {
            let (texture, uv) = match &piece.animation {
                Some(animation) => {
                    let (texture, uv, _) = animation.frame_at(time);
                    (Some(texture), uv)
                }
                None => (
                    piece.texture,
                    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                ),
            };
            if let Some(texture) = texture {
                let rect = egui::Rect::from_min_size(
                    egui::pos2(x, y + (line.height - piece.height) / 2.0),
                    egui::vec2(piece.width, piece.height),
                );
                painter.image(texture, rect, uv, egui::Color32::WHITE);
            } else if let Some(galley) = &piece.galley {
                painter.galley(
                    egui::pos2(x, y + (line.height - galley.size().y) / 2.0),
                    galley.clone(),
                    TEXT(),
                );
            }
            x += piece.width + BUBBLE_TOKEN_GAP;
        }
        y += line.height + BUBBLE_LINE_GAP;
    }
}

/// Une bulle de conversation : les messages envoyés s'alignent à droite, avec
/// l'heure sous le texte et les emotes rendues en image.
/// Toute la bulle est peinte dans un rectangle mesuré, sans passer par une
/// Frame qui prendrait la
/// largeur du panneau.
fn friend_chat_bubble(
    ui: &mut egui::Ui,
    message: &lapinux_core::social::SteamChatMessage,
    emotes: &std::collections::HashMap<String, Option<egui::TextureHandle>>,
    stickers: &std::collections::HashMap<String, Option<StickerAnimation>>,
) -> Option<egui::Rect> {
    if message.text.trim().is_empty() {
        return None;
    }
    let now = ui.input(|input| input.time) as f32;
    let row_width = ui.available_width().max(1.0);
    // Une suite d'emotes ou un autocollant seul n'a pas besoin d'un cadre :
    // le média flotte, comme dans le client Steam, et l'heure passe dessous.
    let tokens = chat_tokens(&message.text);
    let media_only = !tokens.is_empty()
        && tokens.iter().all(|token| match token {
            ChatToken::Emote(name) => emotes
                .get(*name)
                .and_then(|texture| texture.as_ref())
                .is_some(),
            ChatToken::Sticker(name) => stickers
                .get(*name)
                .and_then(|animation| animation.as_ref())
                .is_some(),
            ChatToken::Word(_) => false,
        });
    let max_bubble_width = if media_only {
        row_width
    } else {
        (row_width * 0.72).max(140.0).min(row_width)
    };
    let max_content_width = if media_only {
        row_width
    } else {
        (max_bubble_width - 2.0 * BUBBLE_PAD_X).max(40.0)
    };
    let layout = bubble_layout(
        ui,
        &message.text,
        emotes,
        stickers,
        max_content_width,
        media_only,
    );
    let time = if message.at > 0 {
        Some(ui.painter().layout_no_wrap(
            lapinux_core::util::format_time(message.at),
            egui::FontId::proportional(BUBBLE_TIME_SIZE),
            TEXT_DIM(),
        ))
    } else {
        None
    };
    let time_width = time.as_ref().map_or(0.0, |galley| galley.size().x);
    let bubble_width = if media_only {
        layout.width.max(4.0).min(row_width)
    } else {
        (layout.width + 2.0 * BUBBLE_PAD_X)
            .min(max_bubble_width)
            .max(2.0 * BUBBLE_PAD_X + 4.0)
    };
    let bubble_height = if media_only {
        layout.height.max(4.0)
    } else {
        2.0 * BUBBLE_PAD_Y + layout.height
    };
    let below = time.as_ref().map_or(0.0, |_| 3.0 + BUBBLE_TIME_H);
    let (row, _) = ui.allocate_exact_size(
        egui::vec2(row_width, bubble_height + below),
        egui::Sense::hover(),
    );
    let left = if message.outgoing {
        row.right() - bubble_width
    } else {
        row.left()
    };
    let bubble = egui::Rect::from_min_size(
        egui::pos2(left, row.top()),
        egui::vec2(bubble_width, bubble_height),
    );
    let rounding = egui::Rounding {
        nw: BUBBLE_RADIUS,
        ne: BUBBLE_RADIUS,
        sw: if message.outgoing { BUBBLE_RADIUS } else { 5.0 },
        se: if message.outgoing { 5.0 } else { BUBBLE_RADIUS },
    };
    let painter = ui.painter();
    if !media_only {
        painter.rect_filled(
            bubble,
            rounding,
            if message.outgoing {
                blend_colors(CARD_ACTIVE(), ACCENT(), 0.22)
            } else {
                CARD_HOVER()
            },
        );
        painter.rect_stroke(
            bubble,
            rounding,
            egui::Stroke::new(
                1.0_f32,
                if message.outgoing {
                    with_alpha(ACCENT(), 0.38)
                } else {
                    with_alpha(BORDER(), 0.8)
                },
            ),
        );
    }
    let content = painter.with_clip_rect(ui.clip_rect().intersect(bubble.expand(1.0)));
    let origin = if media_only {
        bubble.min
    } else {
        bubble.min + egui::vec2(BUBBLE_PAD_X, BUBBLE_PAD_Y)
    };
    paint_chat_content(&content, &layout, origin, now);
    if ui.is_rect_visible(bubble) {
        if let Some(delay) = layout.next_frame_in(now) {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs_f32(delay));
        }
    }
    if let Some(galley) = time {
        let time_x = if message.outgoing {
            bubble.right() - time_width
        } else {
            bubble.left()
        };
        painter.galley(
            egui::pos2(time_x, bubble.bottom() + 3.0),
            galley,
            TEXT_DIM(),
        );
    }
    ui.add_space(6.0);
    Some(bubble)
}

/// Couleur de la pastille de présence d'un ami.
fn friend_presence_color(row: &FriendRow) -> egui::Color32 {
    let state = row.state.trim().to_lowercase();
    if row.in_game {
        OK
    } else if state.starts_with("absent") || state.starts_with("away") {
        WARN
    } else if row.online {
        OK
    } else {
        TEXT_DIM()
    }
}

/// La marque et le nom de la boutique, centrés sous le nom de l'ami.
fn friend_store_badge(ui: &mut egui::Ui, store: lapinux_core::stores::Store) {
    let galley = ui.painter().layout_no_wrap(
        store.label().to_string(),
        egui::FontId::proportional(11.0),
        TEXT_DIM(),
    );
    let mark = 15.0;
    let gap = 6.0;
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(mark + gap + galley.size().x, 18.0),
        egui::Sense::hover(),
    );
    let painter = ui.painter();
    paint_store_mark_in(
        painter,
        store,
        egui::Rect::from_center_size(
            egui::pos2(rect.left() + mark / 2.0, rect.center().y),
            egui::vec2(mark, mark),
        ),
        store_tint(store),
    );
    painter.galley(
        egui::pos2(
            rect.left() + mark + gap,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        TEXT_DIM(),
    );
}

/// La pastille de présence et son libellé, centrés dans la fiche.
fn friend_presence_line(ui: &mut egui::Ui, row: &FriendRow) {
    let color = if row.online { TEXT() } else { TEXT_DIM() };
    let state = if row.in_game {
        lapinux_core::i18n::t("friends.in_game").to_string()
    } else if row.state.trim().is_empty() {
        lapinux_core::i18n::t("friends.presence_hidden").to_string()
    } else {
        row.state.clone()
    };
    let galley = ui
        .painter()
        .layout_no_wrap(state, egui::FontId::proportional(12.0), color);
    let dot = 9.0;
    let gap = 6.0;
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(dot + gap + galley.size().x, 18.0),
        egui::Sense::hover(),
    );
    let painter = ui.painter();
    painter.circle_filled(
        egui::pos2(rect.left() + dot / 2.0, rect.center().y),
        4.0,
        friend_presence_color(row),
    );
    painter.galley(
        egui::pos2(
            rect.left() + dot + gap,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        color,
    );
}

/// Libellé localisé d'un état de présence Steam.
fn steam_presence_label(presence: lapinux_core::social::SteamPresence) -> String {
    use lapinux_core::social::SteamPresence;
    match presence {
        SteamPresence::Offline => lapinux_core::i18n::t("friends.presence_offline"),
        SteamPresence::Online => lapinux_core::i18n::t("friends.presence_online"),
        SteamPresence::Busy => lapinux_core::i18n::t("friends.presence_busy"),
        SteamPresence::Away => lapinux_core::i18n::t("friends.presence_away"),
        SteamPresence::Snooze => lapinux_core::i18n::t("friends.presence_snooze"),
        SteamPresence::Trade => lapinux_core::i18n::t("friends.presence_trade"),
        SteamPresence::Play => lapinux_core::i18n::t("friends.presence_play"),
        SteamPresence::Invisible => lapinux_core::i18n::t("friends.presence_invisible"),
    }
    .to_string()
}

/// Une ligne d'information de la fiche : libellé à gauche, valeur à droite.
fn friend_detail_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).size(10.5).color(TEXT_DIM()));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add(
                egui::Label::new(egui::RichText::new(value).size(10.5).color(TEXT())).truncate(),
            );
        });
    });
    ui.add_space(3.0);
}

#[cfg(test)]
mod presence_note_tests {
    use super::*;

    #[test]
    fn une_balise_emoticon_de_l_historique_devient_une_emote() {
        let tokens = chat_tokens("salut [emoticon]steamhappy[/emoticon] !");
        assert_eq!(tokens.len(), 3);
        assert!(matches!(tokens[0], ChatToken::Word("salut")));
        assert!(matches!(tokens[1], ChatToken::Emote("steamhappy")));
        assert!(matches!(tokens[2], ChatToken::Word("!")));
    }

    #[test]
    fn une_balise_sticker_devient_un_autocollant() {
        let tokens = chat_tokens("salut [sticker type=\"sticker_1\" limit=\"0\"][/sticker] !");
        assert_eq!(tokens.len(), 3);
        assert!(matches!(tokens[0], ChatToken::Word("salut")));
        assert!(matches!(tokens[1], ChatToken::Sticker("sticker_1")));
        assert!(matches!(tokens[2], ChatToken::Word("!")));
    }

    #[test]
    fn un_autocollant_dont_le_nom_contient_des_espaces_est_reconnu() {
        let tokens = chat_tokens("[sticker type=\"Three Piece\" limit=\"0\"][/sticker]");
        assert_eq!(tokens.len(), 1);
        assert!(matches!(tokens[0], ChatToken::Sticker("Three Piece")));
    }

    #[test]
    fn les_notes_de_presence_anciennes_et_neutres_sont_reconnues() {
        assert!(is_presence_permission_note(
            "Le jeton du launcher n'a pas la permission de présence."
        ));
        assert!(is_presence_permission_note("Missing presence permission"));
        assert!(is_presence_permission_note("Presence not published"));
        assert!(!is_presence_permission_note("Hors ligne depuis 2 h"));
    }

    #[test]
    fn le_retour_en_bas_n_apparait_que_si_des_messages_sont_caches() {
        assert!(!chat_needs_jump(0.0, 300.0, 280.0));
        assert!(chat_needs_jump(60.0, 300.0, 600.0));
        assert!(!chat_needs_jump(300.0, 300.0, 600.0));
    }

    #[test]
    fn les_shortcodes_d_emotes_sont_reconnus_sans_casser_les_heures() {
        let tokens = chat_tokens("salut ːsteamboredː :steamsad: à 12:30, ça va ?");
        let emotes: Vec<&str> = tokens
            .iter()
            .filter_map(|token| match token {
                ChatToken::Emote(name) => Some(*name),
                ChatToken::Sticker(_) | ChatToken::Word(_) => None,
            })
            .collect();
        assert_eq!(emotes, vec!["steambored", "steamsad"]);
        assert!(tokens
            .iter()
            .any(|token| matches!(token, ChatToken::Word("12:30,"))));
    }

    fn collect_text(shape: &egui::Shape, texts: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(text) => texts.push(text.galley.text().to_string()),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_text(shape, texts);
                }
            }
            _ => {}
        }
    }

    fn contient_un_maillage(shape: &egui::Shape) -> bool {
        match shape {
            egui::Shape::Mesh(_) => true,
            egui::Shape::Vec(shapes) => shapes.iter().any(contient_un_maillage),
            _ => false,
        }
    }

    fn rendre_bulle(
        message: &lapinux_core::social::SteamChatMessage,
        emotes: &std::collections::HashMap<String, Option<egui::TextureHandle>>,
    ) -> (egui::Rect, Vec<String>, Vec<egui::Shape>) {
        let ctx = egui::Context::default();
        crate::apply_theme(&ctx);
        crate::install_fonts(&ctx);
        let mut bubble = None;
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 240.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(180.0)
                        .show(ui, |ui| {
                            bubble = friend_chat_bubble(
                                ui,
                                message,
                                emotes,
                                &std::collections::HashMap::new(),
                            );
                        });
                });
            },
        );
        let mut texts = Vec::new();
        let mut shapes = Vec::new();
        for clipped in &output.shapes {
            collect_text(&clipped.shape, &mut texts);
            shapes.push(clipped.shape.clone());
        }
        (bubble.expect("bulle rendue"), texts, shapes)
    }

    #[test]
    fn une_bulle_est_compacte_et_garde_son_heure_en_bas() {
        let message = lapinux_core::social::SteamChatMessage {
            from: 1,
            text: "gros gay".to_string(),
            at: 1_793_646_552,
            outgoing: false,
            received_live: false,
        };
        let (rect, texts, _) = rendre_bulle(&message, &std::collections::HashMap::new());
        assert!(
            rect.width() < 180.0,
            "largeur de la bulle : {}",
            rect.width()
        );
        assert!(
            rect.height() < 80.0,
            "hauteur de la bulle : {}",
            rect.height()
        );
        assert!(rect.left() < 50.0, "la bulle entrante doit rester à gauche");
        assert!(
            texts.iter().any(|text| text == "gros") && texts.iter().any(|text| text == "gay"),
            "textes : {texts:?}"
        );
        let time = lapinux_core::util::format_time(message.at);
        assert!(texts.iter().any(|text| text == &time), "textes : {texts:?}");
    }

    #[test]
    fn un_message_envoye_est_compact_et_a_droite() {
        let message = lapinux_core::social::SteamChatMessage {
            from: 1,
            text: "dfd".to_string(),
            at: 1_793_646_552,
            outgoing: true,
            received_live: false,
        };
        let (rect, _, _) = rendre_bulle(&message, &std::collections::HashMap::new());
        assert!(
            rect.width() < 120.0,
            "largeur de la bulle : {}",
            rect.width()
        );
        assert!(
            rect.height() < 80.0,
            "hauteur de la bulle : {}",
            rect.height()
        );
        assert!(
            rect.right() > 300.0,
            "la bulle envoyée doit rester à droite"
        );
    }

    #[test]
    fn une_emote_est_peinte_comme_une_image() {
        let ctx = egui::Context::default();
        crate::apply_theme(&ctx);
        crate::install_fonts(&ctx);
        let texture = ctx.load_texture(
            "emote-test",
            egui::ColorImage::new([4, 4], egui::Color32::WHITE),
            egui::TextureOptions::LINEAR,
        );
        let mut emotes = std::collections::HashMap::new();
        emotes.insert("steambored".to_string(), Some(texture));
        let message = lapinux_core::social::SteamChatMessage {
            from: 1,
            text: "ːsteamboredː".to_string(),
            at: 1_793_646_552,
            outgoing: false,
            received_live: false,
        };
        let mut bubble = None;
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 240.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(180.0)
                        .show(ui, |ui| {
                            bubble = friend_chat_bubble(
                                ui,
                                &message,
                                &emotes,
                                &std::collections::HashMap::new(),
                            );
                        });
                });
            },
        );
        assert!(bubble.is_some(), "la bulle doit être rendue");
        assert!(
            output
                .shapes
                .iter()
                .any(|clipped| contient_un_maillage(&clipped.shape)),
            "l'emote doit produire un maillage d'image"
        );
    }

    #[test]
    fn le_champ_masque_le_shortcode_et_peint_l_emote_a_sa_place() {
        let ctx = egui::Context::default();
        crate::apply_theme(&ctx);
        crate::install_fonts(&ctx);
        let texture = ctx.load_texture(
            "emote-champ-test",
            egui::ColorImage::new([4, 4], egui::Color32::WHITE),
            egui::TextureOptions::LINEAR,
        );
        let emote_texture = texture.id();
        let mut emotes = std::collections::HashMap::new();
        emotes.insert("steambored".to_string(), Some(texture));
        // Les polices n'existent qu'après la première frame du contexte.
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 80.0),
                )),
                ..Default::default()
            },
            |_| {},
        );
        let font = egui::TextStyle::Body.resolve(&ctx.style());
        let measure = |run: &str| {
            ctx.fonts(|fonts| {
                fonts
                    .layout_no_wrap(run.to_string(), font.clone(), egui::Color32::WHITE)
                    .size()
                    .x
            })
        };
        let job = draft_layout_job("salut ːsteamboredː", &emotes, font.clone(), &measure);
        assert_eq!(
            job.text, "salut ːsteamboredː",
            "le texte du champ reste intact"
        );
        assert!(
            job.sections
                .iter()
                .any(|section| section.format.color == egui::Color32::TRANSPARENT),
            "le shortcode chargé doit être transparent"
        );
        let ranges = draft_emote_ranges(&job.text);
        let (range, _) = &ranges[0];
        let start = job.text[..range.start].chars().count();
        let end = job.text[..range.end].chars().count();
        let galley = ctx.fonts(|fonts| fonts.layout_job(job.clone()));
        let row = galley.rows.first().expect("une ligne de texte");
        let x0 = row.glyphs[start].pos.x;
        let x1 = row
            .glyphs
            .get(end)
            .map_or(galley.size().x, |glyph| glyph.pos.x);
        assert!(
            (12.0..21.0).contains(&(x1 - x0)),
            "la place du shortcode doit tenir dans l'image : {}",
            x1 - x0
        );

        let mut text = "salut ːsteamboredː".to_string();
        let mut field = egui::Rect::NOTHING;
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 80.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    field = draft_field(ui, &mut text, &emotes, 320.0).response.rect;
                });
            },
        );
        assert_eq!(text, "salut ːsteamboredː", "le champ garde le shortcode");
        let meshes: Vec<(egui::TextureId, egui::Rect)> = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Mesh(mesh) => Some((mesh.texture_id, mesh.calc_bounds())),
                _ => None,
            })
            .collect();
        let images: Vec<egui::Rect> = meshes
            .iter()
            .filter(|(texture, _)| *texture == emote_texture)
            .map(|(_, bounds)| *bounds)
            .collect();
        assert_eq!(images.len(), 1, "meshes : {meshes:?}");
        assert!(
            images[0].top() >= field.top() - 5.0 && images[0].bottom() <= field.bottom() + 5.0,
            "l'emote reste dans la rangée : image {:?} / champ {:?}",
            images[0],
            field
        );
        assert!(
            (images[0].center().y - field.center().y).abs() < 1.0,
            "l'emote est centrée verticalement : {:?} / {:?}",
            images[0],
            field
        );
        assert!(
            images[0].left() > field.left() + 20.0,
            "l'emote est peinte après le texte : image {:?} / champ {:?}",
            images[0],
            field
        );
        let text_bounds = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) => {
                    Some(egui::Rect::from_min_size(text.pos, text.galley.size()))
                }
                _ => None,
            })
            .find(|bounds| bounds.width() > 40.0)
            .expect("le texte doit être peint");
        assert!(
            (text_bounds.center().y - field.center().y).abs() < 1.0,
            "le texte est centré verticalement : {:?} / {:?}",
            text_bounds,
            field
        );
    }

    #[test]
    fn une_emote_sans_image_reste_ecrite_dans_le_champ() {
        let ctx = egui::Context::default();
        crate::apply_theme(&ctx);
        crate::install_fonts(&ctx);
        let font = egui::TextStyle::Body.resolve(&ctx.style());
        let mut emotes = std::collections::HashMap::new();
        emotes.insert("absente".to_string(), None);
        let measure = |run: &str| {
            ctx.fonts(|fonts| {
                fonts
                    .layout_no_wrap(run.to_string(), font.clone(), egui::Color32::WHITE)
                    .size()
                    .x
            })
        };
        let job = draft_layout_job(":absente: mais 12:30", &emotes, font.clone(), &measure);
        assert_eq!(job.text, ":absente: mais 12:30");
        assert!(
            job.sections
                .iter()
                .all(|section| section.format.color != egui::Color32::TRANSPARENT),
            "sans image, le shortcode reste lisible"
        );
        assert_eq!(draft_emote_ranges(":absente: mais 12:30").len(), 1);
    }

    /// Rend le champ avec un curseur donné et renvoie sa géométrie plus les
    /// formes réellement peintes (dont le trait du caret).
    fn rendre_le_champ(
        ctx: &egui::Context,
        texte: &mut String,
        curseur: usize,
        emotes: &std::collections::HashMap<String, Option<egui::TextureHandle>>,
    ) -> (DraftFieldOutput, Vec<egui::Shape>) {
        let mut state = egui::TextEdit::load_state(ctx, message_field_id()).unwrap_or_default();
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::one(
                egui::text::CCursor::new(curseur),
            )));
        state.store(ctx, message_field_id());
        let mut champ = None;
        let output = ctx.run(
            egui::RawInput {
                time: Some(0.0),
                focused: true,
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 80.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.memory_mut(|memory| memory.request_focus(message_field_id()));
                    champ = Some(draft_field(ui, texte, emotes, 320.0));
                });
            },
        );
        let shapes = output
            .shapes
            .iter()
            .map(|clipped| clipped.shape.clone())
            .collect();
        (champ.expect("le champ doit être rendu"), shapes)
    }

    /// Largeur du texte avant le caret, mesurée avec le même découpage
    /// (shortcodes compressés compris) mais sans réutiliser la géométrie du
    /// caret : c'est le point d'insertion attendu.
    fn largeur_avant_caret(
        ctx: &egui::Context,
        texte: &str,
        curseur: usize,
        emotes: &std::collections::HashMap<String, Option<egui::TextureHandle>>,
    ) -> f32 {
        let font = egui::TextStyle::Body.resolve(&ctx.style());
        let measure = |run: &str| {
            ctx.fonts(|fonts| {
                fonts
                    .layout_no_wrap(run.to_string(), font.clone(), egui::Color32::WHITE)
                    .size()
                    .x
            })
        };
        let prefix: String = texte.chars().take(curseur).collect();
        let mut job = draft_layout_job(&prefix, emotes, font.clone(), &measure);
        job.wrap.max_width = f32::INFINITY;
        ctx.fonts(|fonts| fonts.layout_job(job)).size().x
    }

    /// Le trait vertical réellement peint par le champ, sous forme de rect.
    fn caret_peint(shapes: &[egui::Shape], champ: egui::Rect) -> Option<egui::Rect> {
        shapes.iter().find_map(|shape| match shape {
            egui::Shape::LineSegment { points, .. }
                if (points[0].x - points[1].x).abs() < 0.01
                    && points[0].y < points[1].y
                    && champ.expand(2.0).contains(points[0])
                    && champ.expand(2.0).contains(points[1]) =>
            {
                Some(egui::Rect::from_min_max(points[0], points[1]))
            }
            _ => None,
        })
    }

    /// Compare le caret exposé, le point d'insertion de la galley, la largeur
    /// du préfixe et le trait effectivement peint. Tolérance : 0,75 px
    /// logique, soit l'arrondi d'un demi-pixel de egui sur chaque bord.
    fn verifier_caret(expected_x: f32, champ: &DraftFieldOutput, peint: Option<egui::Rect>) {
        let insertion = champ.insertion.expect("le point d'insertion doit exister");
        let caret = champ.caret.expect("le caret doit être exposé");
        let tolerance = 0.75;
        assert!(
            (caret.center().x - expected_x).abs() <= tolerance,
            "caret x={} attendu={expected_x}",
            caret.center().x
        );
        assert!(
            (insertion.center().x - expected_x).abs() <= tolerance,
            "insertion x={} attendu={expected_x}",
            insertion.center().x
        );
        assert!(
            (caret.top() + 1.5 - insertion.top()).abs() <= tolerance
                && (caret.bottom() - 1.5 - insertion.bottom()).abs() <= tolerance,
            "caret {caret:?} ne coïncide pas avec l'insertion {insertion:?}"
        );
        assert!(
            (caret.center().y - insertion.center().y).abs() <= tolerance,
            "centre vertical du caret {} / insertion {}",
            caret.center().y,
            insertion.center().y
        );
        assert!(
            (insertion.height() - champ.row_height).abs() <= tolerance,
            "hauteur d'insertion {} pour la ligne {}",
            insertion.height(),
            champ.row_height
        );
        let peint = peint.expect("le trait du caret doit être peint");
        assert!(
            (peint.center().x - insertion.center().x).abs() <= tolerance,
            "trait peint x={} / insertion x={}",
            peint.center().x,
            insertion.center().x
        );
        // egui épaissit le trait de 1,5 px autour de la ligne : le trait peint
        // couvre le rectangle du caret, dont le centre est le point d'insertion.
        assert!(
            (peint.top() - caret.top()).abs() <= tolerance
                && (peint.bottom() - caret.bottom()).abs() <= tolerance,
            "trait peint {peint:?} / caret {caret:?}"
        );
        assert!(
            (peint.center().y - insertion.center().y).abs() <= tolerance,
            "centre du trait peint {} / insertion {}",
            peint.center().y,
            insertion.center().y
        );
        eprintln!(
            "caret attendu x={expected_x:.2} | caret x={:.2} y={:.2}..{:.2} | insertion x={:.2} y={:.2}..{:.2} | peint x={:.2} y={:.2}..{:.2} | ligne {:.2} | tol {tolerance}",
            caret.center().x,
            caret.top(),
            caret.bottom(),
            insertion.center().x,
            insertion.top(),
            insertion.bottom(),
            peint.center().x,
            peint.top(),
            peint.bottom(),
            champ.row_height,
        );
    }

    #[test]
    fn le_caret_du_champ_vide_est_sur_la_ligne() {
        let ctx = egui::Context::default();
        crate::apply_theme(&ctx);
        crate::install_fonts(&ctx);
        let mut texte = String::new();
        let emotes = std::collections::HashMap::new();
        let (champ, shapes) = rendre_le_champ(&ctx, &mut texte, 0, &emotes);
        let expected = champ.galley_pos.x + largeur_avant_caret(&ctx, &texte, 0, &emotes);
        let peint = caret_peint(&shapes, champ.response.rect);
        verifier_caret(expected, &champ, peint);
        let insertion = champ.insertion.expect("insertion");
        // Avant la correction, une galley vide n'avait aucune ligne : le caret
        // partait du milieu du champ. Il doit maintenant occuper la ligne.
        assert!(
            (insertion.top() - champ.galley_pos.y).abs() <= 0.75,
            "insertion {insertion:?} / haut de galley {}",
            champ.galley_pos.y
        );
        assert!(
            (insertion.height() - champ.row_height).abs() <= 0.75,
            "hauteur {} pour la ligne {}",
            insertion.height(),
            champ.row_height
        );
        // Ancrages indépendants de la galley : `response.rect` est le
        // rectangle interne (marges exclues), donc le texte commence à sa
        // gauche et la ligne est centrée dedans. Sans la section vide, la
        // galley vide est centrée puis le caret descend d'une demi-ligne : ces
        // deux mesures le détectent.
        assert!(
            (champ.galley_pos.x - champ.response.rect.left()).abs() <= 0.75,
            "origine du texte {} / gauche interne {}",
            champ.galley_pos.x,
            champ.response.rect.left()
        );
        assert!(
            (insertion.center().y - champ.response.rect.center().y).abs() <= 0.75,
            "ligne centrée en {} / champ centré en {}",
            insertion.center().y,
            champ.response.rect.center().y
        );
    }

    #[test]
    fn le_caret_suit_le_texte_saisi() {
        let ctx = egui::Context::default();
        crate::apply_theme(&ctx);
        crate::install_fonts(&ctx);
        let mut texte = "bonjour".to_string();
        let emotes = std::collections::HashMap::new();
        let curseur = texte.chars().count();
        let (champ, shapes) = rendre_le_champ(&ctx, &mut texte, curseur, &emotes);
        let expected = champ.galley_pos.x + largeur_avant_caret(&ctx, &texte, curseur, &emotes);
        let peint = caret_peint(&shapes, champ.response.rect);
        verifier_caret(expected, &champ, peint);
        let insertion = champ.insertion.expect("insertion");
        assert!(
            (insertion.top() - champ.galley_pos.y).abs() <= 0.75,
            "insertion {insertion:?} / haut de galley {}",
            champ.galley_pos.y
        );
        assert!(
            (insertion.center().y - champ.response.rect.center().y).abs() <= 0.75,
            "ligne centrée en {} / champ centré en {}",
            insertion.center().y,
            champ.response.rect.center().y
        );
    }

    #[test]
    fn choisir_une_emote_ferme_le_selecteur_et_place_le_caret_apres_le_shortcode() {
        let ctx = egui::Context::default();
        crate::apply_theme(&ctx);
        crate::install_fonts(&ctx);
        let texture = ctx.load_texture(
            "emote-insertion-test",
            egui::ColorImage::from_rgba_unmultiplied([4, 4], &[255; 64]),
            egui::TextureOptions::LINEAR,
        );
        let mut emotes = std::collections::HashMap::new();
        emotes.insert("steambored".to_string(), Some(texture));

        // Insertion au milieu puis en fin : le shortcode doit atterrir au
        // curseur, pas toujours à la fin, et le caret suivre l'insertion.
        for (mut texte, curseur, attendu, caret_attendu) in [
            ("salut la".to_string(), 6, "salut :steambored: la", 6 + 13),
            ("salut".to_string(), 5, "salut :steambored: ", 5 + 14),
        ] {
            let mut open = true;
            let mut state =
                egui::TextEdit::load_state(&ctx, message_field_id()).unwrap_or_default();
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::one(
                    egui::text::CCursor::new(curseur),
                )));
            state.store(&ctx, message_field_id());
            let mut champ = None;
            let mut caret_apres = 0;
            let output = ctx.run(
                egui::RawInput {
                    time: Some(0.0),
                    focused: true,
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(400.0, 80.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.memory_mut(|memory| memory.request_focus(message_field_id()));
                        caret_apres = apply_emote_choice(ui, &mut texte, &mut open, "steambored");
                        champ = Some(draft_field(ui, &mut texte, &emotes, 320.0));
                    });
                },
            );
            assert!(!open, "le sélecteur doit être fermé dans la même action");
            let ouvert = conversation_layout(600.0, true);
            let ferme = conversation_layout(600.0, false);
            assert_eq!(ferme.picker_height, 0.0, "plus aucune hauteur de sélecteur");
            assert!(
                ouvert.picker_height > 0.0,
                "le sélecteur ouvert occupe une hauteur"
            );
            assert_eq!(
                ferme.composer_height, ouvert.composer_height,
                "le compositeur reprend sa hauteur"
            );
            assert_eq!(ferme.composer_height, 34.0, "hauteur du compositeur");
            assert!(
                ctx.memory(|memory| memory.has_focus(message_field_id())),
                "le champ garde le focus après le choix"
            );
            eprintln!(
                "emote curseur={curseur} texte={texte:?} caret={caret_apres} selecteur_ouvert={open} picker={} compositeur={} focus=true",
                ferme.picker_height, ferme.composer_height,
            );
            assert_eq!(texte, attendu, "l'emote s'insère au curseur");
            assert_eq!(caret_apres, caret_attendu, "caret juste après le shortcode");
            let champ = champ.expect("le champ doit être rendu");
            let expected =
                champ.galley_pos.x + largeur_avant_caret(&ctx, &texte, caret_apres, &emotes);
            let shapes: Vec<egui::Shape> = output
                .shapes
                .iter()
                .map(|clipped| clipped.shape.clone())
                .collect();
            let peint = caret_peint(&shapes, champ.response.rect);
            verifier_caret(expected, &champ, peint);
        }
    }

    /// Un brouillon nettement plus large que le champ, avec des espaces pour
    /// vérifier qu'il ne se replie pas dans le champ mono-ligne.
    fn brouillon_long() -> String {
        "un long brouillon avec plusieurs mots pour depasser la largeur visible du champ ".repeat(3)
    }

    #[test]
    fn le_caret_suit_le_defilement_horizontal() {
        let texte_long = brouillon_long();
        let total = texte_long.chars().count();
        let emotes = std::collections::HashMap::new();
        // Curseur au milieu puis en fin : dans les deux cas egui doit décaler
        // la galley pour garder le caret dans la fenêtre visible.
        for (ou, curseur) in [("milieu", total / 2), ("fin", total)] {
            let ctx = egui::Context::default();
            crate::apply_theme(&ctx);
            crate::install_fonts(&ctx);
            let mut texte = texte_long.clone();
            let (champ, shapes) = rendre_le_champ(&ctx, &mut texte, curseur, &emotes);
            let largeur_totale = largeur_avant_caret(&ctx, &texte, total, &emotes);
            let largeur_prefixe = largeur_avant_caret(&ctx, &texte, curseur, &emotes);
            let visible = champ.response.rect.width();
            assert!(
                largeur_totale > visible + 50.0,
                "{ou}: brouillon {largeur_totale:.2} / largeur visible {visible:.2}"
            );
            assert!(
                (champ.response.rect.height() - champ.row_height).abs() <= 0.75,
                "{ou}: champ replié sur plusieurs lignes (hauteur {} / ligne {})",
                champ.response.rect.height(),
                champ.row_height
            );
            if curseur < total {
                assert!(
                    largeur_prefixe > visible + 10.0,
                    "{ou}: le milieu doit être hors de la première fenêtre ({largeur_prefixe:.2} / {visible:.2})"
                );
            }
            // Défilement minimal pour rendre le caret visible, depuis un état
            // neuf (singleline_offset = 0), borné à la largeur du texte.
            let attendu_offset =
                (largeur_prefixe - visible).clamp(0.0, (largeur_totale - visible).max(0.0));
            let offset = champ.response.rect.left() - champ.galley_pos.x;
            assert!(
                (offset - attendu_offset).abs() <= 0.75,
                "{ou}: défilement attendu {attendu_offset:.2}, obtenu {offset:.2}"
            );
            let attendu_x = champ.response.rect.left() + largeur_prefixe - attendu_offset;
            eprintln!(
                "scroll {ou} curseur={curseur} total={largeur_totale:.2} prefixe={largeur_prefixe:.2} visible={visible:.2} offset attendu={attendu_offset:.2} obtenu={offset:.2} x attendu={attendu_x:.2} galley_x={:.2}",
                champ.galley_pos.x
            );
            verifier_caret(attendu_x, &champ, caret_peint(&shapes, champ.response.rect));
            let insertion = champ.insertion.expect("insertion");
            assert!(
                champ
                    .response
                    .rect
                    .expand(0.75)
                    .contains(insertion.center()),
                "{ou}: insertion {insertion:?} hors du champ visible {:?}",
                champ.response.rect
            );
            assert!(
                (insertion.center().x - champ.response.rect.right()).abs() <= 0.75,
                "{ou}: insertion x={} / bord droit visible={}",
                insertion.center().x,
                champ.response.rect.right()
            );
        }
    }

    #[test]
    fn choisir_une_emote_dans_un_brouillon_long_garde_le_caret_visible() {
        let texte_long = brouillon_long();
        let total = texte_long.chars().count();
        for (ou, curseur) in [("milieu", total / 2), ("fin", total)] {
            let ctx = egui::Context::default();
            crate::apply_theme(&ctx);
            crate::install_fonts(&ctx);
            let texture = ctx.load_texture(
                "emote-long-test",
                egui::ColorImage::from_rgba_unmultiplied([4, 4], &[255; 64]),
                egui::TextureOptions::LINEAR,
            );
            let mut emotes = std::collections::HashMap::new();
            emotes.insert("steambored".to_string(), Some(texture));
            let mut texte = texte_long.clone();
            let mut open = true;
            let mut state =
                egui::TextEdit::load_state(&ctx, message_field_id()).unwrap_or_default();
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::one(
                    egui::text::CCursor::new(curseur),
                )));
            state.store(&ctx, message_field_id());
            let mut champ = None;
            let mut caret_apres = 0;
            let output = ctx.run(
                egui::RawInput {
                    time: Some(0.0),
                    focused: true,
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(400.0, 80.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.memory_mut(|memory| memory.request_focus(message_field_id()));
                        caret_apres = apply_emote_choice(ui, &mut texte, &mut open, "steambored");
                        champ = Some(draft_field(ui, &mut texte, &emotes, 320.0));
                    });
                },
            );
            assert!(!open, "{ou}: sélecteur fermé dans la même action");
            let ouvert = conversation_layout(600.0, true);
            let ferme = conversation_layout(600.0, false);
            assert_eq!(ferme.picker_height, 0.0, "{ou}: sélecteur replié");
            assert_eq!(ferme.composer_height, 34.0, "{ou}: compositeur");
            assert_eq!(
                ouvert.composer_height, ferme.composer_height,
                "{ou}: compositeur inchangé"
            );
            assert!(ouvert.picker_height > 0.0, "{ou}: sélecteur ouvert");
            assert!(
                ctx.memory(|memory| memory.has_focus(message_field_id())),
                "{ou}: le champ garde le focus"
            );
            let champ = champ.expect("champ rendu");
            assert!(
                (champ.response.rect.height() - champ.row_height).abs() <= 0.75,
                "{ou}: champ replié sur plusieurs lignes (hauteur {} / ligne {})",
                champ.response.rect.height(),
                champ.row_height
            );
            let avant: String = texte.chars().take(caret_apres).collect();
            assert!(
                avant.ends_with(":steambored: "),
                "{ou}: le caret doit suivre le shortcode, avant={avant:?}"
            );
            if curseur < total {
                assert!(
                    texte.chars().count() - caret_apres > 20,
                    "{ou}: l'emote doit rester au milieu du brouillon"
                );
            }
            let largeur_totale = largeur_avant_caret(&ctx, &texte, texte.chars().count(), &emotes);
            let largeur_prefixe = largeur_avant_caret(&ctx, &texte, caret_apres, &emotes);
            let visible = champ.response.rect.width();
            assert!(
                largeur_totale > visible + 50.0,
                "{ou}: brouillon {largeur_totale:.2} / largeur visible {visible:.2}"
            );
            assert!(
                largeur_prefixe > visible + 10.0,
                "{ou}: le caret doit être hors de la première fenêtre ({largeur_prefixe:.2} / {visible:.2})"
            );
            let attendu_offset =
                (largeur_prefixe - visible).clamp(0.0, (largeur_totale - visible).max(0.0));
            let offset = champ.response.rect.left() - champ.galley_pos.x;
            assert!(
                (offset - attendu_offset).abs() <= 0.75,
                "{ou}: défilement attendu {attendu_offset:.2}, obtenu {offset:.2}"
            );
            let attendu_x = champ.response.rect.left() + largeur_prefixe - attendu_offset;
            eprintln!(
                "emote long {ou} curseur initial={curseur} caret={caret_apres} len={} total={largeur_totale:.2} prefixe={largeur_prefixe:.2} visible={visible:.2} offset attendu={attendu_offset:.2} obtenu={offset:.2} x attendu={attendu_x:.2}",
                texte.chars().count()
            );
            let shapes: Vec<egui::Shape> = output
                .shapes
                .iter()
                .map(|clipped| clipped.shape.clone())
                .collect();
            verifier_caret(attendu_x, &champ, caret_peint(&shapes, champ.response.rect));
            let insertion = champ.insertion.expect("insertion");
            assert!(
                champ
                    .response
                    .rect
                    .expand(0.75)
                    .contains(insertion.center()),
                "{ou}: insertion {insertion:?} hors du champ visible {:?}",
                champ.response.rect
            );
            assert!(
                (insertion.center().x - champ.response.rect.right()).abs() <= 0.75,
                "{ou}: insertion x={} / bord droit visible={}",
                insertion.center().x,
                champ.response.rect.right()
            );
        }
    }

    #[test]
    fn la_carte_et_le_viewport_ne_depassent_pas_la_hauteur_disponible() {
        for (available, picker_open) in [(600.0, false), (600.0, true), (420.0, true)] {
            let layout = conversation_layout(available, picker_open);
            let ctx = egui::Context::default();
            crate::apply_theme(&ctx);
            crate::install_fonts(&ctx);
            let mut scroll_view = 0.0;
            let mut scroll_content = 0.0;
            let mut card_height = 0.0;
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(400.0, 700.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let card = egui::Frame::none()
                            .inner_margin(egui::Margin {
                                left: 10.0,
                                right: 10.0,
                                top: 10.0,
                                bottom: 6.0,
                            })
                            .show(ui, |ui| {
                                ui.set_width(360.0);
                                ui.spacing_mut().item_spacing.y = 0.0;
                                let history_rect = egui::Rect::from_min_size(
                                    ui.cursor().min,
                                    egui::vec2(ui.available_width(), layout.history_height),
                                );
                                let _ = ui
                                    .allocate_exact_size(history_rect.size(), egui::Sense::hover());
                                ui.scope_builder(
                                    egui::UiBuilder::new().max_rect(history_rect),
                                    |ui| {
                                        let scroll = egui::ScrollArea::vertical()
                                            .max_height(layout.history_height)
                                            .auto_shrink([false, false])
                                            .show(ui, |ui| {
                                                ui.label("bonjour");
                                            });
                                        scroll_view = scroll.inner_rect.height();
                                        scroll_content = scroll.content_size.y;
                                    },
                                );
                                ui.add_space(layout.gap);
                                let _ = ui.allocate_exact_size(
                                    egui::vec2(ui.available_width(), layout.composer_height),
                                    egui::Sense::hover(),
                                );
                            });
                        card_height = card.response.rect.height();
                    });
                },
            );
            assert!(
                (scroll_view - layout.history_height).abs() < 1.0,
                "viewport {} pour {}",
                scroll_view,
                layout.history_height
            );
            assert!(
                scroll_content <= scroll_view + 1.0,
                "contenu {} / viewport {}",
                scroll_content,
                scroll_view
            );
            assert!(
                (card_height - layout.card_height).abs() < 2.0,
                "carte {} pour {}",
                card_height,
                layout.card_height
            );
            assert!(card_height <= available);
        }
    }

    #[test]
    fn le_compositeur_ne_bouge_pas_avec_le_defilement() {
        let layout = conversation_layout(600.0, false);
        let mut states = Vec::new();
        for (count, offset) in [(1, 0.0), (80, 0.0), (80, 40.0), (80, 200.0)] {
            let ctx = egui::Context::default();
            crate::apply_theme(&ctx);
            crate::install_fonts(&ctx);
            let mut composer_top = 0.0;
            let mut card_height = 0.0;
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(400.0, 700.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let card = egui::Frame::none()
                            .inner_margin(egui::Margin {
                                left: 10.0,
                                right: 10.0,
                                top: 10.0,
                                bottom: 6.0,
                            })
                            .show(ui, |ui| {
                                ui.set_width(360.0);
                                ui.spacing_mut().item_spacing.y = 0.0;
                                let history_rect = egui::Rect::from_min_size(
                                    ui.cursor().min,
                                    egui::vec2(ui.available_width(), layout.history_height),
                                );
                                let _ = ui
                                    .allocate_exact_size(history_rect.size(), egui::Sense::hover());
                                ui.scope_builder(
                                    egui::UiBuilder::new().max_rect(history_rect),
                                    |ui| {
                                        egui::ScrollArea::vertical()
                                            .id_salt(("long-chat", count, offset as i32))
                                            .max_height(layout.history_height)
                                            .auto_shrink([false, false])
                                            .vertical_scroll_offset(offset)
                                            .show(ui, |ui| {
                                                for index in 0..count {
                                                    ui.label(format!("message {index}"));
                                                }
                                            });
                                    },
                                );
                                ui.add_space(layout.gap);
                                let (rect, _) = ui.allocate_exact_size(
                                    egui::vec2(ui.available_width(), layout.composer_height),
                                    egui::Sense::hover(),
                                );
                                composer_top = rect.top();
                            });
                        card_height = card.response.rect.height();
                    });
                },
            );
            states.push((composer_top, card_height));
        }
        let reference = states[0];
        for (index, state) in states.iter().enumerate() {
            assert!(
                (state.0 - reference.0).abs() < 1.0,
                "compositeur {index} décalé : {:?} / {:?}",
                state,
                reference
            );
            assert!(
                (state.1 - reference.1).abs() < 2.0,
                "carte {index} redimensionnée : {:?} / {:?}",
                state,
                reference
            );
        }
    }

    #[test]
    fn la_carte_remplit_le_modal_sans_bouger_le_compositeur() {
        for available in [260.0, 420.0, 600.0] {
            let closed = conversation_layout(available, false);
            let open = conversation_layout(available, true);
            assert!(closed.card_height <= available);
            assert!((closed.card_height - open.card_height).abs() < f32::EPSILON);
            assert!((closed.composer_height - COMPOSER_BASE_HEIGHT).abs() < f32::EPSILON);
            assert!((closed.composer_height - open.composer_height).abs() < f32::EPSILON);
            assert!(
                (closed.inner_margin + closed.history_height + closed.gap + closed.composer_height
                    - closed.card_height)
                    .abs()
                    < 0.01
            );
            assert!(
                (open.inner_margin + open.history_height + open.gap + open.composer_height
                    - open.card_height)
                    .abs()
                    < 0.01
            );
            assert!((open.history_height - closed.history_height).abs() < 0.01);
        }
    }

    fn collect_rect_geometry(shape: &egui::Shape, rects: &mut Vec<(egui::Rect, egui::Color32)>) {
        match shape {
            egui::Shape::Rect(rect) => rects.push((rect.rect, rect.fill)),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_rect_geometry(shape, rects);
                }
            }
            _ => {}
        }
    }

    fn collect_text_geometry(
        shape: &egui::Shape,
        texts: &mut Vec<(String, egui::Pos2, egui::Vec2)>,
    ) {
        match shape {
            egui::Shape::Text(text) => {
                texts.push((text.galley.text().to_string(), text.pos, text.galley.size()))
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_text_geometry(shape, texts);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn les_segments_du_selecteur_centrent_leur_contenu() {
        use lapinux_core::stores::Store;

        let labels = [
            (None, "Tous · 36"),
            (Some(Store::Steam), "Steam · 24"),
            (Some(Store::Epic), "Epic Games · 12"),
            (Some(Store::Gog), "GOG · 0"),
        ];
        let width = 140.0;
        let ctx = egui::Context::default();
        crate::apply_theme(&ctx);
        crate::install_fonts(&ctx);
        let mut segments = Vec::new();
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(700.0, 80.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        for (store, label) in labels {
                            let response = crate::friend_chip(
                                ui,
                                label,
                                store == Some(Store::Steam),
                                store,
                                width,
                            );
                            segments.push((label, store, response.rect));
                        }
                    });
                });
            },
        );
        let mut texts = Vec::new();
        for clipped in &output.shapes {
            collect_text_geometry(&clipped.shape, &mut texts);
        }
        for (label, store, rect) in segments {
            assert!(
                (rect.width() - width).abs() <= 0.01,
                "{label}: largeur du segment {} pour {width}",
                rect.width()
            );
            let (_, pos, size) = texts
                .iter()
                .find(|(text, _, _)| text == label)
                .unwrap_or_else(|| panic!("{label}: texte non peint, textes={texts:?}"));
            let text_center = pos.x + size.x / 2.0;
            // La marque garde son rapport : sa largeur réelle n'est pas
            // toujours 15 px. Le groupe va du bord gauche de la marque au bord
            // droit du texte, avec 7 px entre les deux.
            let icon_width = store.map(|store| {
                let texture =
                    crate::store_mark_texture(&ctx, store, crate::mark_pixels(&ctx, 15.0));
                let source = texture.size_vec2();
                15.0 * source.x / source.x.max(source.y).max(1.0)
            });
            let group_center = text_center - icon_width.map_or(0.0, |width| (width + 7.0) / 2.0);
            eprintln!(
                "{label}: segment x={:.2}..{:.2} centre={:.2} texte centre={text_center:.2} icône={icon_width:?} groupe centre={group_center:.2} décalage={:.2}",
                rect.left(),
                rect.right(),
                rect.center().x,
                group_center - rect.center().x
            );
            assert!(
                (group_center - rect.center().x).abs() < 0.5,
                "{label}: groupe décalé de {:.2} px dans son segment",
                group_center - rect.center().x
            );
            assert!(
                (pos.y + size.y / 2.0 - rect.center().y).abs() < 0.5,
                "{label}: texte décalé verticalement de {:.2} px",
                pos.y + size.y / 2.0 - rect.center().y
            );
        }
    }

    #[test]
    fn les_indicateurs_du_selecteur_sont_centres_sur_leur_segment() {
        use lapinux_core::stores::Store;

        let labels = [
            (None, "Tous · 36"),
            (Some(Store::Steam), "Steam · 24"),
            (Some(Store::Epic), "Epic Games · 12"),
            (Some(Store::Gog), "GOG · 0"),
        ];
        let width = 140.0;
        for (active_index, (active_store, active_label)) in labels.iter().enumerate() {
            let ctx = egui::Context::default();
            crate::apply_theme(&ctx);
            crate::install_fonts(&ctx);
            let mut segments = Vec::new();
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(700.0, 80.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 2.0;
                            for (store, label) in labels {
                                let response = crate::friend_chip(
                                    ui,
                                    label,
                                    store == *active_store,
                                    store,
                                    width,
                                );
                                segments.push((store, label, response.rect));
                            }
                        });
                    });
                },
            );
            assert_eq!(segments.len(), 4);
            let mut rects = Vec::new();
            for clipped in &output.shapes {
                collect_rect_geometry(&clipped.shape, &mut rects);
            }
            let (_, _, segment) = segments[active_index];
            let pill = rects
                .iter()
                .find(|(rect, fill)| {
                    *fill != egui::Color32::TRANSPARENT
                        && (rect.width() - segment.width()).abs() <= 0.75
                        && (rect.height() - segment.height()).abs() <= 0.75
                })
                .unwrap_or_else(|| {
                    panic!("{active_label}: pilule active absente, rects={rects:?}")
                });
            let line = rects
                .iter()
                .find(|(rect, fill)| {
                    *fill != egui::Color32::TRANSPARENT
                        && rect.height() <= 3.0
                        && rect.width() > segment.width() / 2.0
                        && rect.width() < segment.width()
                })
                .unwrap_or_else(|| panic!("{active_label}: trait actif absent, rects={rects:?}"));
            eprintln!(
                "actif {active_label}: segment centre=({:.2},{:.2}) taille=({:.2},{:.2}); pilule centre=({:.2},{:.2}) taille=({:.2},{:.2}); trait centre_x={:.2} haut={:.2} bas={:.2} hauteur={:.2}",
                segment.center().x,
                segment.center().y,
                segment.width(),
                segment.height(),
                pill.0.center().x,
                pill.0.center().y,
                pill.0.width(),
                pill.0.height(),
                line.0.center().x,
                line.0.top(),
                line.0.bottom(),
                line.0.height()
            );
            assert!(
                (pill.0.center().x - segment.center().x).abs() < 0.5,
                "{active_label}: pilule décalée horizontalement de {:.2} px",
                pill.0.center().x - segment.center().x
            );
            assert!(
                (pill.0.center().y - segment.center().y).abs() < 0.5,
                "{active_label}: pilule décalée verticalement de {:.2} px",
                pill.0.center().y - segment.center().y
            );
            assert!(
                (pill.0.width() - segment.width()).abs() < 0.5,
                "{active_label}: pilule large de {} pour un segment de {}",
                pill.0.width(),
                segment.width()
            );
            assert!(
                (pill.0.height() - segment.height()).abs() < 0.5,
                "{active_label}: pilule haute de {} pour un segment de {}",
                pill.0.height(),
                segment.height()
            );
            assert!(
                (line.0.center().x - segment.center().x).abs() < 0.5,
                "{active_label}: trait décalé horizontalement de {:.2} px",
                line.0.center().x - segment.center().x
            );
            assert!(
                (line.0.bottom() - segment.bottom()).abs() < 0.5,
                "{active_label}: trait à {:.2} px du bas du segment",
                segment.bottom() - line.0.bottom()
            );
            assert!(
                (line.0.height() - 2.0).abs() < 0.5,
                "{active_label}: trait haut de {:.2} px",
                line.0.height()
            );
            let left_inset = line.0.left() - segment.left();
            let right_inset = segment.right() - line.0.right();
            assert!(
                (left_inset - right_inset).abs() < 0.5,
                "{active_label}: trait asymétrique ({left_inset:.2} px à gauche, {right_inset:.2} px à droite)"
            );
            assert!(
                line.0.left() >= segment.left() - 0.01 && line.0.right() <= segment.right() + 0.01,
                "{active_label}: trait débordant du segment"
            );
        }
    }
}
