//! Coque, navigation et affichage principal.

use super::*;

/// Un jeu d'une notification wishlist. Chaque notification garde sa propre
/// carte compacte : les icônes et les détails ne sont jamais mélangés avec
/// ceux d'une autre notification.
struct WishlistCardEntry {
    deal: lapinux_core::social::SteamWishlistDeal,
    icon: Option<egui::TextureHandle>,
}

impl LapinuxApp {
    pub(super) fn startup_screen(&mut self, ctx: &egui::Context) -> bool {
        if self.startup.done {
            return false;
        }
        let elapsed = self.startup.started.elapsed().as_secs_f32();
        let skip =
            ctx.input(|input| input.pointer.any_click() || input.key_pressed(egui::Key::Escape));
        if skip {
            self.startup.done = true;
            return false;
        }
        // La lecture initiale tourne hors du fil d'affichage : l'écran de
        // démarrage couvre l'attente au lieu de figer sur une fenêtre vide.
        if elapsed >= 0.35
            && self.startup.ready
            && self.startup.painted
            && self.startup.closing.is_none()
        {
            self.startup.closing = Some(Instant::now());
        }
        if let Some(closing) = self.startup.closing {
            if closing.elapsed() > Duration::from_millis(220) {
                self.startup.done = true;
                return false;
            }
        }
        // Filet de sécurité : le daemon ne répond pas, on montre la fenêtre.
        if elapsed > 6.0 {
            self.startup.done = true;
            return false;
        }

        let rect = ctx.screen_rect();
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("lapinux-startup"),
        ));
        painter.rect_filled(rect, 0.0, BACKGROUND());

        let fade_in = (elapsed / 0.18).clamp(0.0, 1.0);
        let fade_out = self
            .startup
            .closing
            .map(|closing| (1.0 - closing.elapsed().as_secs_f32() / 0.22).clamp(0.0, 1.0))
            .unwrap_or(1.0);
        let alpha = fade_in.min(fade_out);
        let hop = (elapsed * 5.5).sin().abs() * 12.0;
        let center = rect.center() + egui::vec2(0.0, -18.0 - hop);
        if let Some(logo) = &self.logo {
            logo.draw(&painter, center, 150.0, alpha);
        }

        // Le nom écrit à la main, le même que dans la barre de fenêtre ; le
        // texte reste en secours si l'image manque.
        let name_at = rect.center() + egui::vec2(0.0, 122.0);
        match &self.wordmark {
            Some(wordmark) => wordmark.draw(&painter, name_at, 32.0, alpha),
            None => {
                painter.text(
                    name_at,
                    egui::Align2::CENTER_CENTER,
                    lapinux_core::APP_NAME,
                    egui::FontId::proportional(27.0),
                    with_alpha(TEXT(), alpha),
                );
            }
        }
        painter.text(
            rect.center() + egui::vec2(0.0, 154.0),
            egui::Align2::CENTER_CENTER,
            lapinux_core::i18n::t("shell.library_title"),
            egui::FontId::proportional(13.0),
            with_alpha(TEXT_DIM(), alpha),
        );
        ctx.request_repaint_after(Duration::from_millis(16));
        true
    }

    pub(super) fn handle_gamepad(&mut self, ctx: &egui::Context) {
        // Pendant une partie, les manettes appartiennent au jeu. Cette garde
        // couvre aussi l'image où le lancement vient de partir, avant que la
        // session ne soit publiée par le daemon.
        if self.session.is_some() || self.launching.is_some() {
            return;
        }
        let Some(nav) = self.nav.as_ref() else {
            return;
        };
        let events: Vec<lapinux_input::Nav> = nav.events.try_iter().collect();
        if events.is_empty() {
            return;
        }
        // Sur la page Manettes, les entrées servent uniquement au test :
        // aucun bouton ne doit lancer un jeu ni ouvrir le quick menu.
        if self.page == Page::Controllers {
            return;
        }
        for event in events {
            match event {
                // Les boutons de la manette ne pilotent la bibliothèque que
                // lorsqu'elle est visible : sur la page Manettes, ils servent
                // au test et ne doivent ni lancer un jeu ni ouvrir un overlay.
                lapinux_input::Nav::Confirm => {
                    if self.page == Page::Library {
                        if let Some(appid) = self.selected {
                            self.launch(appid, false);
                        }
                    }
                }
                lapinux_input::Nav::Back => {
                    if self.page == Page::Library {
                        self.selected = None;
                    } else if self.page == Page::Collections {
                        if self.collection_detail_open && self.selected.is_some() {
                            self.selected = None;
                        } else {
                            self.back_to_collections();
                        }
                    }
                }
                lapinux_input::Nav::Menu => {
                    let _ = self.client.request(&Req::OverlayToggle);
                }
                lapinux_input::Nav::Left | lapinux_input::Nav::Right => {
                    if self.page == Page::Library {
                        let step = if event == lapinux_input::Nav::Left {
                            -1
                        } else {
                            1
                        };
                        self.move_selection(step, ctx);
                    }
                }
                lapinux_input::Nav::Up | lapinux_input::Nav::Down => {
                    if self.page == Page::Library {
                        let step = if event == lapinux_input::Nav::Up {
                            -4
                        } else {
                            4
                        };
                        self.move_selection(step, ctx);
                    }
                }
            }
        }
    }

    /// Pendant une partie, l'interface ne doit ouvrir aucun nœud evdev : les
    /// manettes sont capturées par le service d'entrée et destinées au jeu.
    /// Relâcher les lecteurs ferme aussi leurs descripteurs, au lieu de les
    /// laisser ouverts sans recevoir d'événements à cause du grab.
    pub(super) fn sync_gamepad_input(&mut self) {
        if self.session.is_some() || self.launching.is_some() {
            self.nav = None;
            self.pad_monitor = None;
            return;
        }
        if self.nav.is_none() {
            self.nav = lapinux_input::NavReader::start().ok();
        }
        if self.pad_monitor.is_none() {
            self.pad_monitor = lapinux_input::PadMonitor::start().ok();
        }
    }

    fn move_selection(&mut self, delta: i32, _ctx: &egui::Context) {
        let visible: Vec<u32> = self
            .library_entries()
            .iter()
            .map(|entry| entry.appid)
            .collect();
        if visible.is_empty() {
            return;
        }
        let current = self
            .selected
            .and_then(|appid| visible.iter().position(|id| *id == appid))
            .unwrap_or(0) as i32;
        let next = (current + delta).clamp(0, visible.len() as i32 - 1) as usize;
        self.selected = Some(visible[next]);
    }

    fn toggle_maximized(&mut self, ctx: &egui::Context) {
        let maximized = !self.maximized(ctx);
        self.maximized = maximized;
        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(maximized));
    }

    pub(super) fn maximized(&self, ctx: &egui::Context) -> bool {
        ctx.input(|input| input.viewport().maximized)
            .unwrap_or(self.maximized)
    }

    /// Barre de fenêtre dessinée par l'application : le compositeur ne décore
    /// plus la fenêtre, donc le nom et les commandes vivent ici.
    pub(super) fn window_bar(&mut self, ctx: &egui::Context) -> Option<egui::Rect> {
        if self.big_picture {
            return None;
        }
        // En NX, la barre de fenêtre se fond dans le fond de la console :
        // seulement les boutons système de la fenêtre.
        let nx = self.active_theme.layout == theme::ThemeLayout::Nx;
        let panel = egui::TopBottomPanel::top("windowbar")
            .exact_height(if nx { 36.0 } else { 42.0 })
            .show_separator_line(false)
            .frame(
                egui::Frame::none()
                    .fill(if nx {
                        self.active_theme.colors.background
                    } else {
                        PANEL()
                    })
                    .inner_margin(egui::Margin::symmetric(12.0, 0.0)),
            )
            .show(ctx, |ui| {
                let bar = ui.available_rect_before_wrap();
                // Toute la barre déplace la fenêtre ; les boutons ajoutés
                // ensuite reprennent la main sur leur propre zone.
                let drag = ui.interact(
                    bar,
                    ui.id().with("windowbar-drag"),
                    egui::Sense::click_and_drag(),
                );
                if drag.double_clicked() {
                    self.toggle_maximized(ctx);
                }
                if drag.drag_started() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                    release_pointer_after_window_command(ctx);
                }
                ui.horizontal_centered(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if window_button(ui, WindowButton::Close).clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        let restore = self.maximized(ctx);
                        let kind = if restore {
                            WindowButton::Restore
                        } else {
                            WindowButton::Maximize
                        };
                        if window_button(ui, kind).clicked() {
                            self.toggle_maximized(ctx);
                        }
                        if window_button(ui, WindowButton::Minimize).clicked() {
                            // Réduction dans la barre des tâches, pas une
                            // fermeture : le tray garde déjà le rôle de
                            // masquer l'interface.
                            self.idle_since = None;
                            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                        }
                    });
                });
            });
        let rect = panel.response.rect;
        // Le nom et le filet vivent dans le calque de fond de la fenêtre, pas
        // au premier plan : un panneau superposé (Réglages, succès, contenus)
        // doit les couvrir, sinon le filet passe par-dessus son contenu.
        if !nx {
            let layer = ctx.layer_painter(egui::LayerId::background());
            // Le nom vit au milieu de la barre : il se centre sur la fenêtre
            // entière, comme un titre.
            if let Some(wordmark) = &self.wordmark {
                wordmark.draw(
                    &layer,
                    egui::pos2(rect.center().x, rect.center().y),
                    21.0,
                    1.0,
                );
            }
            layer.line_segment(
                [
                    egui::pos2(rect.left(), rect.bottom() - 0.5),
                    egui::pos2(rect.right(), rect.bottom() - 0.5),
                ],
                egui::Stroke::new(1.0_f32, BORDER()),
            );
        }
        Some(rect)
    }

    /// Bords redimensionnables : sans décoration système, la fenêtre n'a plus
    /// de poignées fournies par le compositeur.
    pub(super) fn resize_grips(&self, ctx: &egui::Context) {
        if self.maximized(ctx) || self.big_picture {
            return;
        }
        const T: f32 = 5.0;
        const C: f32 = 14.0;
        use egui::ResizeDirection as Dir;
        let screen = ctx.screen_rect();
        let corners = [
            (
                egui::Rect::from_min_size(screen.min, egui::vec2(C, C)),
                Dir::NorthWest,
                egui::CursorIcon::ResizeNorthWest,
            ),
            (
                egui::Rect::from_min_size(
                    egui::pos2(screen.right() - C, screen.top()),
                    egui::vec2(C, C),
                ),
                Dir::NorthEast,
                egui::CursorIcon::ResizeNorthEast,
            ),
            (
                egui::Rect::from_min_size(
                    egui::pos2(screen.left(), screen.bottom() - C),
                    egui::vec2(C, C),
                ),
                Dir::SouthWest,
                egui::CursorIcon::ResizeSouthWest,
            ),
            (
                egui::Rect::from_min_size(
                    egui::pos2(screen.right() - C, screen.bottom() - C),
                    egui::vec2(C, C),
                ),
                Dir::SouthEast,
                egui::CursorIcon::ResizeSouthEast,
            ),
        ];
        let edges = [
            (
                egui::Rect::from_min_max(screen.min, egui::pos2(screen.right(), screen.top() + T)),
                Dir::North,
                egui::CursorIcon::ResizeNorth,
            ),
            (
                egui::Rect::from_min_max(
                    egui::pos2(screen.left(), screen.bottom() - T),
                    screen.max,
                ),
                Dir::South,
                egui::CursorIcon::ResizeSouth,
            ),
            (
                egui::Rect::from_min_max(
                    screen.min,
                    egui::pos2(screen.left() + T, screen.bottom()),
                ),
                Dir::West,
                egui::CursorIcon::ResizeWest,
            ),
            (
                egui::Rect::from_min_max(egui::pos2(screen.right() - T, screen.top()), screen.max),
                Dir::East,
                egui::CursorIcon::ResizeEast,
            ),
        ];
        // Les coins passent en dernier pour primer sur les bords.
        for (index, (rect, direction, cursor)) in edges.into_iter().chain(corners).enumerate() {
            let response = egui::Area::new(egui::Id::new(("lapinux-resize", index)))
                .order(egui::Order::Foreground)
                .fixed_pos(rect.min)
                .show(ctx, |ui| {
                    ui.allocate_exact_size(rect.size(), egui::Sense::drag()).1
                })
                .inner;
            if response.hovered() || response.dragged() {
                ctx.set_cursor_icon(cursor);
            }
            if response.drag_started() {
                ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(direction));
                release_pointer_after_window_command(ctx);
            }
        }
    }

    /// Une clé activée fait entrer un jeu : la bibliothèque est relue en
    /// tâche de fond et la jaquette part vers l'onglet Bibliothèque.
    /// Retrouve le jeu d'une activation que Steam n'a pas nommée : le
    /// catalogue rafraîchi porte son appid, donc sa jaquette, son icône et son
    /// nom. Sans réponse au bout d'un quart d'heure, l'attente est abandonnée.
    pub(super) fn resolve_pending_reveal(&mut self) {
        if self.game_reveal.is_some() {
            return;
        }
        let Some(pending) = self.reveal_pending.clone() else {
            return;
        };
        if pending.since.elapsed() > Duration::from_secs(900) {
            self.reveal_pending = None;
            return;
        }
        let Some(catalog) = self.catalog.as_ref() else {
            return;
        };
        // Les jeux qui viennent d'entrer, puis celui que le reçu nommait.
        let arrived: Vec<&lapinux_core::model::SteamCatalogGame> = catalog
            .entries
            .iter()
            .filter(|entry| !pending.known.contains(&entry.appid))
            .collect();
        let found = pending
            .name
            .as_deref()
            .and_then(|name| {
                arrived
                    .iter()
                    .find(|entry| entry.name.eq_ignore_ascii_case(name))
                    .copied()
            })
            .or_else(|| arrived.iter().copied().max_by_key(|entry| entry.acquired));
        let Some(entry) = found else {
            return;
        };
        let appid = entry.appid;
        let name = if entry.name.trim().is_empty() {
            lapinux_core::i18n::t("shell.new_game").to_string()
        } else {
            entry.name.clone()
        };
        self.reveal_pending = None;
        // Le bandeau tourne déjà, lancé sans nom ni appid : la fiche qui arrive
        // lui donne son titre, son icône et sa jaquette sans relancer
        // l'animation. Une célébration déjà finie ne se rejoue pas.
        if let Some(reveal) = self.game_reveal.as_mut() {
            if reveal.appid == 0 || reveal.appid == appid {
                reveal.appid = appid;
                reveal.name = name;
            } else {
                // Deux clés coup sur coup : la seconde célébration attend que
                // la première ait fini de s'effacer.
                self.reveal_queue = Some((appid, name));
            }
            return;
        }
        // La jaquette n'est pas encore là : la célébration l'attend, sinon
        // le bandeau s'ouvre sur l'initiale et l'image arrive après lui.
        self.reveal_wait = Some((appid, name, Instant::now()));
    }

    pub(super) fn note_redeem(&mut self, ctx: &egui::Context) {
        // Une activation qui repart remet le repère à zéro : deux clés peuvent
        // rendre un reçu mot pour mot identique, et la seconde était prise
        // pour la première.
        if self
            .social
            .as_ref()
            .and_then(|social| social.redeem.as_ref())
            .map(|redeem| redeem.state != "ok")
            .unwrap_or(false)
        {
            self.redeem_seen = None;
        }
        let stamp = self
            .social
            .as_ref()
            .and_then(|social| social.redeem.as_ref())
            .map(|redeem| {
                // Deux clés peuvent rendre le même message : l'empreinte
                // porte aussi les jeux et les appids du reçu, sinon la
                // deuxième activation passait pour la première.
                (
                    redeem.state.clone(),
                    redeem.message.clone(),
                    redeem.games.join("|"),
                    redeem
                        .appids
                        .iter()
                        .map(|appid| appid.to_string())
                        .collect::<Vec<_>>()
                        .join("|"),
                )
            });
        if self.redeem_seen == stamp {
            return;
        }
        let revealed = self
            .social
            .as_ref()
            .and_then(|social| social.redeem.as_ref())
            .filter(|redeem| redeem.state == "ok")
            .map(|redeem| {
                // Steam ne joint pas toujours les lignes du reçu : le nom et
                // l'appid se retrouvent alors dans la bibliothèque déjà lue.
                let name = redeem.games.first().cloned();
                let appid = redeem.appids.first().copied().unwrap_or_else(|| {
                    name.as_deref()
                        .and_then(|name| {
                            self.library_entries_all()
                                .iter()
                                .find(|entry| entry.name.eq_ignore_ascii_case(name))
                                .map(|entry| entry.appid)
                        })
                        .unwrap_or(0)
                });
                (appid, name)
            });
        self.redeem_seen = stamp;
        if let Some((appid, name)) = revealed.clone() {
            // Le jeu vient d'entrer dans le compte : le reçu le nomme, le
            // daemon le range au catalogue avec une fiche minimale, et la
            // célébration part dès qu'elle arrive — sans relire 1470 jeux.
            self.refresh_async(ctx, true);
            // Même un reçu muet déclenche la demande : le daemon retrouve la
            // licence de la clé et son paquet, puis écrit la fiche.
            self.catalog_patch = Some((appid, name.unwrap_or_default()));
            self.load_catalog(false);
        }
        // La célébration part tout de suite, avec ce que le reçu donne : le
        // catalogue peut arriver après, il ne la retient plus. Sans nom, le
        // bandeau s'ouvre sur « Nouveau jeu » plutôt que de ne rien montrer.
        if let Some((appid, name)) = revealed {
            let name = name
                .filter(|name| !name.trim().is_empty())
                .or_else(|| {
                    self.library_entries_all()
                        .iter()
                        .find(|entry| appid != 0 && entry.appid == appid)
                        .map(|entry| entry.name.clone())
                })
                .unwrap_or_else(|| lapinux_core::i18n::t("shell.new_game").to_string());
            // La fiche du jeu arrive une à deux secondes après : le catalogue
            // est relu tant qu'elle manque, sinon la bibliothèque restait sans
            // le jeu activé jusqu'à la synchronisation suivante.
            self.reveal_pending = Some(PendingReveal {
                name: (name != lapinux_core::i18n::t("shell.new_game")).then(|| name.clone()),
                since: Instant::now(),
                known: self
                    .library_entries_all()
                    .iter()
                    .map(|entry| entry.appid)
                    .collect(),
            });
            // Pas de bandeau tant que la fiche manque : il partirait sans nom
            // ni jaquette et une seconde célébration suivrait. Une seule
            // animation, la bonne, dès que le jeu est identifié.
        }
    }

    /// Icône carrée d'un jeu pour la célébration : celle déjà rangée pour la
    /// fiche, sinon celle du client Steam. Aucun téléchargement ici — la
    /// célébration ne doit pas attendre le réseau.
    fn reveal_icon_texture(
        &mut self,
        ctx: &egui::Context,
        appid: u32,
    ) -> Option<egui::TextureHandle> {
        if let Some((known, texture)) = &self.reveal_icon {
            if *known == appid {
                return texture.clone();
            }
        }
        let dir = self.paths.art_dir();
        let name = format!("reveal-icon-{appid}");
        let mut found = [ArtSource::Steamgriddb, ArtSource::Steam]
            .into_iter()
            .find_map(|source| {
                let path = art_source_path(&dir, appid, SteamArt::Icon, source);
                load_texture_file(ctx, &path, &name)
            });
        if found.is_none() {
            let legacy = dir.join(format!("icon-{appid}.jpg"));
            found = load_texture_file(ctx, &legacy, &name);
        }
        if found.is_none() {
            found = steam_cached_icon(appid).and_then(|path| load_texture_file(ctx, &path, &name));
        }
        if found.is_none() {
            // Aucune icône publiée : le centre de la jaquette, recadré.
            found = square_icon_from_cover(ctx, &self.paths, appid);
        }
        self.reveal_icon = Some((appid, found.clone()));
        found
    }

    /// Jaquette de la célébration : le fichier du cache, lu directement.
    /// Passer par le cache de la grille laissait le bandeau sans image quand
    /// ce cache refusait de décoder (budget par image, sources épuisées).
    pub(super) fn reveal_cover_texture(
        &mut self,
        ctx: &egui::Context,
        appid: u32,
    ) -> Option<egui::TextureHandle> {
        if let Some((known, texture)) = &self.reveal_cover {
            if *known == appid {
                return Some(texture.clone());
            }
        }
        let dir = self.paths.covers_dir();
        let name = format!("reveal-cover-{appid}");
        let found = ["", "sgdb-"].into_iter().find_map(|prefix| {
            let path = dir.join(format!("{prefix}{appid}.jpg"));
            load_texture_file(ctx, &path, &name)
        })?;
        self.reveal_cover = Some((appid, found.clone()));
        Some(found)
    }

    /// Lance la célébration d'une clé activée.
    pub(super) fn start_game_reveal(&mut self, appid: u32, name: impl Into<String>) {
        self.game_reveal = Some(GameReveal {
            appid,
            name: name.into(),
            started: Instant::now(),
        });
    }

    /// Célébration d'une activation, dessinée comme le bandeau de succès de
    /// l'overlay : l'icône tombe du haut, le panneau glisse derrière elle et
    /// les écritures montent l'une après l'autre. La jaquette rejoint ensuite
    /// le logo de la bibliothèque.
    pub(super) fn paint_game_reveal(&mut self, ctx: &egui::Context) {
        const ENTER: f32 = 0.88;
        const HOLD: f32 = 2.4;
        const EXIT: f32 = 1.05;
        const TOTAL: f32 = ENTER + HOLD + EXIT;
        // Mesures du bandeau de succès, à l'identique.
        const CARD_W: f32 = 680.0;
        const CARD_H: f32 = 148.0;
        const ICON: f32 = 148.0;
        // Dégradé du panneau et filet d'accent de la boutique.
        const TOP: egui::Color32 = egui::Color32::from_rgb(44, 82, 112);
        const BOTTOM: egui::Color32 = egui::Color32::from_rgb(18, 34, 47);
        const REVEAL_ACCENT: egui::Color32 = egui::Color32::from_rgb(102, 192, 244);
        let Some(mut reveal) = self.game_reveal.clone() else {
            return;
        };
        // La fiche du jeu n'arrive qu'une à deux secondes après le début du
        // bandeau : tant qu'il tourne sans nom, le catalogue le complète ici.
        // Sans cela, la célébration restait sur « Nouveau jeu », sans icône ni
        // jaquette, alors que le jeu était déjà dans la bibliothèque.
        if reveal.appid == 0 || reveal.name == lapinux_core::i18n::t("shell.new_game") {
            let young =
                lapinux_core::util::now_unix() - reveal.started.elapsed().as_secs() as i64 - 120;
            let found = self
                .catalog
                .as_ref()
                .and_then(|catalog| {
                    catalog
                        .entries
                        .iter()
                        .filter(|entry| entry.acquired.is_some_and(|date| date >= young))
                        .max_by_key(|entry| entry.acquired)
                })
                .map(|entry| (entry.appid, entry.name.clone()));
            if let Some((appid, name)) = found {
                reveal.appid = appid;
                if !name.trim().is_empty() {
                    reveal.name = name;
                }
                if let Some(slot) = self.game_reveal.as_mut() {
                    slot.appid = reveal.appid;
                    slot.name = reveal.name.clone();
                }
            }
        }
        let elapsed = reveal.started.elapsed().as_secs_f32();
        if elapsed >= TOTAL {
            self.game_reveal = None;
            return;
        }
        // Une image par rafraîchissement pendant le mouvement : un minuteur de
        // 16 ms ajoutait son retard à chaque image et saccadait le bandeau.
        // Pendant la tenue, rien ne bouge : on réveille l'interface pile à la
        // fin, pour que la sortie démarre sans temps mort.
        let moving = elapsed < 1.0 || elapsed > ENTER + HOLD;
        if moving {
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(Duration::from_secs_f32((ENTER + HOLD - elapsed).max(0.0)));
        }
        // Une clé dont Steam ne nomme pas le jeu n'a pas d'appid : la
        // célébration se fait alors avec l'initiale, sans jaquette.
        let texture = if reveal.appid == 0 {
            None
        } else {
            self.reveal_cover_texture(ctx, reveal.appid)
        };
        let screen = ctx.screen_rect();
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("redeem-celebration"),
        ));
        // La jaquette garde son format : sa case prend sa largeur et vient
        // buter contre le filet du panneau, sans jeu entre les deux.
        let cover_ratio = texture
            .as_ref()
            .map(|texture| {
                let size = texture.size_vec2();
                size.x.max(1.0) / size.y.max(1.0)
            })
            .unwrap_or(0.7);
        let art_w = (ICON * cover_ratio).clamp(ICON * 0.55, ICON * 1.5);
        let card_w = art_w + (CARD_W - ICON);
        // Le bandeau garde ses proportions : il rétrécit sur un petit écran.
        let k = ((screen.width() - 64.0) / card_w)
            .min((screen.height() - 64.0) / CARD_H)
            .clamp(0.5, 1.0);
        let card = egui::Rect::from_center_size(
            screen.center() - egui::vec2(0.0, screen.height() * 0.06),
            egui::vec2(card_w, CARD_H) * k,
        );
        let ease = |value: f32| {
            let p = value.clamp(0.0, 1.0);
            1.0 - (1.0 - p) * (1.0 - p) * (1.0 - p)
        };
        // Les temps du bandeau : l'icône tombe (0,34 s), glisse à gauche
        // (0,40 s) pendant que le panneau s'ouvre (0,26 s), puis les écritures
        // arrivent l'une après l'autre.
        let drop = ease((elapsed - 0.03) / 0.34);
        let shift = ease((elapsed - 0.03 - 0.32) / 0.40);
        let panel_in = ease((elapsed - 0.03 - 0.48) / 0.26);
        let header_in = ease((elapsed - 0.46) / 0.22);
        let title_in = ease((elapsed - 0.54) / 0.24);
        let line_in = ease((elapsed - 0.62) / 0.24);
        // Sortie : les écritures s'effacent (0,30 s), puis la jaquette part
        // vers l'onglet Bibliothèque.
        let exit = (elapsed - ENTER - HOLD).max(0.0);
        let text_out = ease(exit / 0.30);
        let fly = ease((exit - 0.30) / 0.75);
        let fade = (1.0 - text_out).clamp(0.0, 1.0);
        // Le voile couvre l'application entière : la célébration se passe ici.
        let veil = (panel_in * (1.0 - ease(exit / 0.95))).clamp(0.0, 1.0);
        if veil > 0.0 {
            painter.rect_filled(
                screen,
                0.0,
                egui::Color32::from_black_alpha((veil * 132.0) as u8),
            );
        }
        // L'icône tombe au centre de l'écran, puis glisse à gauche pour
        // découvrir le panneau ; elle finit dans le logo de la bibliothèque.
        let home = egui::Rect::from_min_size(card.min, egui::vec2(art_w, CARD_H) * k);
        let centre_x = screen.center().x - art_w * k / 2.0;
        let icon_x = centre_x + (home.left() - centre_x) * shift;
        let icon_y = -ICON * k + (home.top() + ICON * k) * drop;
        let target = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new(("nav-tab-rect", Page::Library as u8)))
            })
            .map(|tab| egui::Rect::from_center_size(tab.center(), egui::vec2(18.0, 18.0)))
            .unwrap_or_else(|| {
                egui::Rect::from_center_size(
                    egui::pos2(screen.left() + 96.0, screen.top() + 28.0),
                    egui::vec2(18.0, 18.0),
                )
            });
        let icon = if fly > 0.0 {
            egui::Rect::from_center_size(
                egui::pos2(icon_x, icon_y) + (target.center() - egui::pos2(icon_x, icon_y)) * fly,
                home.size() + (target.size() - home.size()) * fly,
            )
        } else {
            egui::Rect::from_min_size(egui::pos2(icon_x, icon_y), home.size())
        };
        // Le panneau : un rectangle à plat, teinté par la boutique. Pas de
        // carte arrondie ni d'ombre — le bandeau de succès n'en a pas.
        let panel = egui::Rect::from_min_size(
            egui::pos2(card.left() + art_w * k, card.top()),
            egui::vec2((card_w - art_w) * k, card.height()),
        );
        let panel_alpha = (panel_in * fade).clamp(0.0, 1.0);
        if panel_alpha > 0.002 {
            let mut mesh = egui::epaint::Mesh::default();
            mesh.colored_vertex(panel.left_top(), with_alpha(TOP, panel_alpha * 0.94));
            mesh.colored_vertex(panel.right_top(), with_alpha(TOP, panel_alpha * 0.94));
            mesh.colored_vertex(panel.left_bottom(), with_alpha(BOTTOM, panel_alpha * 0.94));
            mesh.colored_vertex(panel.right_bottom(), with_alpha(BOTTOM, panel_alpha * 0.94));
            mesh.add_triangle(0, 1, 2);
            mesh.add_triangle(1, 2, 3);
            painter.add(egui::Shape::mesh(mesh));
            // Filet de la boutique au bord du panneau : la seule couleur du
            // fond.
            painter.rect_filled(
                egui::Rect::from_min_size(panel.min, egui::vec2(3.0 * k, panel.height())),
                0.0,
                with_alpha(REVEAL_ACCENT, panel_alpha * 0.9),
            );
        }
        // La jaquette entière, à plat dans sa case : elle touche le filet du
        // panneau. L'icône carrée ne prend sa place que si la jaquette manque.
        let icon_alpha = (drop * (1.0 - 0.2 * fly)).clamp(0.0, 1.0);
        let initial: String = reveal
            .name
            .chars()
            .next()
            .map(|letter| letter.to_uppercase().to_string())
            .unwrap_or_default();
        let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        let tint = egui::Color32::from_white_alpha((255.0 * icon_alpha) as u8);
        let game_icon = if reveal.appid == 0 {
            None
        } else {
            self.reveal_icon_texture(ctx, reveal.appid)
        };
        let mut art = icon;
        match &texture {
            Some(texture) => {
                art = contain_fit_rect(icon, texture.size_vec2());
                painter.image(texture.id(), art, uv, tint);
            }
            None => match &game_icon {
                Some(game_icon) => {
                    painter.rect_filled(icon, 0.0, with_alpha(BOTTOM, icon_alpha * 0.95));
                    art = contain_fit_rect(icon, game_icon.size_vec2());
                    painter.image(game_icon.id(), art, uv, tint);
                }
                None => {
                    painter.rect_filled(icon, 0.0, with_alpha(BOTTOM, icon_alpha * 0.95));
                    painter.text(
                        icon.center(),
                        egui::Align2::CENTER_CENTER,
                        &initial,
                        egui::FontId::proportional(52.0 * k),
                        with_alpha(TEXT(), icon_alpha),
                    );
                }
            },
        }
        // Pastille de déblocage au coin de l'icône : un disque de la couleur
        // du panneau la sépare du cadre, puis la coche dit le succès obtenu.
        let badge_alpha = (icon_alpha * (1.0 - fly)).clamp(0.0, 1.0);
        if badge_alpha > 0.01 {
            let badge = egui::pos2(art.right() - 5.0 * k, art.bottom() - 5.0 * k);
            painter.circle_filled(badge, 12.0 * k, with_alpha(BOTTOM, badge_alpha * 0.95));
            painter.circle_filled(badge, 9.5 * k, with_alpha(REVEAL_ACCENT, badge_alpha));
            painter.add(egui::Shape::line(
                vec![
                    badge + egui::vec2(-4.0, 0.2) * k,
                    badge + egui::vec2(-1.2, 3.0) * k,
                    badge + egui::vec2(4.2, -3.0) * k,
                ],
                egui::Stroke::new(2.0 * k, with_alpha(BOTTOM, badge_alpha * 0.95)),
            ));
        }
        if panel_alpha <= 0.002 {
            return;
        }
        let text_x = panel.left() + 20.0 * k;
        let right = panel.right() - 20.0 * k;
        // À la sortie, tout monte pour partir : un fondu sur place laisserait
        // les écritures flotter au bord du panneau.
        let lift = text_out * 10.0 * k;
        let ink = |alpha: f32, color: egui::Color32| {
            with_alpha(color, (panel_alpha * alpha).clamp(0.0, 1.0))
        };
        let added_in = ease((elapsed - 0.70) / 0.24);
        // Tout tient haut dans le panneau : l'icône, le nom du jeu et la
        // boutique sur une seule ligne, puis la clé et son effet dessous.
        let name_y = panel.top() + 56.0 * k + (1.0 - title_in) * 12.0 * k - lift;
        let label = painter.text(
            egui::pos2(right, name_y - 2.0 * k),
            egui::Align2::RIGHT_BOTTOM,
            "STEAM",
            egui::FontId::proportional(12.0 * k),
            ink(0.85 * header_in, egui::Color32::from_rgb(232, 235, 240)),
        );
        let mark = store_mark_texture(
            ctx,
            lapinux_core::stores::Store::Steam,
            mark_pixels(ctx, 15.0 * k),
        );
        let source = mark.size_vec2();
        let ratio = if source.y > 0.0 {
            source.x / source.y
        } else {
            1.0
        };
        let mark_h = 15.0 * k;
        let mark_w = mark_h * ratio;
        let mark_rect = egui::Rect::from_min_size(
            egui::pos2(
                label.left() - 7.0 * k - mark_w,
                name_y - 2.0 * k - mark_h * 0.86,
            ),
            egui::vec2(mark_w, mark_h),
        );
        painter.image(
            mark.id(),
            mark_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::from_white_alpha((255.0 * panel_alpha * 0.9 * header_in) as u8),
        );
        // L'icône carrée à gauche du nom, sur la même ligne.
        let side = 32.0 * k;
        if let Some(thumb) = game_icon.as_ref().or(texture.as_ref()) {
            let tile = egui::Rect::from_center_size(
                egui::pos2(text_x + side / 2.0, name_y - side * 0.30),
                egui::vec2(side, side),
            );
            painter.image(
                thumb.id(),
                contain_fit_rect(tile, thumb.size_vec2()),
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::from_white_alpha((255.0 * panel_alpha * 0.95 * title_in) as u8),
            );
        }
        let name_x = text_x + side + 10.0 * k;
        let room = (label.left() - 10.0 * k - name_x).max(60.0 * k);
        let mut name_size = 26.0 * k;
        let measured = painter
            .layout_no_wrap(
                reveal.name.clone(),
                egui::FontId::proportional(name_size),
                TEXT(),
            )
            .size()
            .x;
        if measured > room {
            name_size = (name_size * room / measured).max(16.0 * k);
        }
        painter.text(
            egui::pos2(name_x, name_y),
            egui::Align2::LEFT_BOTTOM,
            &reveal.name,
            egui::FontId::proportional(name_size),
            ink(title_in, TEXT()),
        );
        // La clé juste sous le nom, puis ce qu'elle a produit.
        let key_y = panel.top() + 86.0 * k + (1.0 - line_in) * 8.0 * k - lift;
        painter.text(
            egui::pos2(name_x, key_y),
            egui::Align2::LEFT_BOTTOM,
            lapinux_core::i18n::t("shell.key_activated"),
            egui::FontId::proportional(15.0 * k),
            ink(0.95 * line_in, REVEAL_ACCENT),
        );
        let added_y = panel.top() + 111.0 * k + (1.0 - added_in) * 6.0 * k - lift;
        painter.text(
            egui::pos2(name_x, added_y),
            egui::Align2::LEFT_BOTTOM,
            lapinux_core::i18n::t("shell.added"),
            egui::FontId::proportional(13.5 * k),
            ink(0.9 * added_in, TEXT_DIM()),
        );
    }

    /// Relit les manifestes actifs et réapplique leur style egui. Appelé au
    /// démarrage et après chaque activation de thème ou de plugin.
    pub(super) fn refresh_theme(&mut self, ctx: &egui::Context) {
        let plugins = self
            .plugin_library
            .active(&self.config.general.active_plugins);
        let active = self
            .theme_library
            .resolve(&self.config.general.theme, &plugins);
        apply_active_theme(ctx, &active);
        self.active_theme = active;
        self.theme_dirty = false;
    }

    /// Accueil NX : avatars, horloge, carrousel de jaquettes et dock. Le
    /// carrousel se pilote aux flèches, le bouton d'action lance le jeu
    /// sélectionné. Les pages autres que la bibliothèque gardent leur contenu
    /// Lapinux, sous un bandeau NX compact.
    pub(super) fn nx_home(&mut self, ctx: &egui::Context) {
        // Le cache décode par petits lots : l'accueil NX doit ouvrir son
        // budget comme la bibliothèque, sinon les cartes restent en lettre.
        self.covers.begin_frame(6);
        let base = self.library_entries_all();
        let mut recent: HashMap<u32, i64> = HashMap::new();
        for game in &self.games {
            if let Some(at) = game.last_played {
                recent.insert(game.appid, at);
            }
        }
        let mut order: Vec<usize> = (0..base.len()).collect();
        order.sort_by(|left, right| {
            let left = &base[*left];
            let right = &base[*right];
            recent
                .get(&right.appid)
                .copied()
                .unwrap_or(0)
                .cmp(&recent.get(&left.appid).copied().unwrap_or(0))
                .then_with(|| right.installed.cmp(&left.installed))
                .then_with(|| cmp_lower(&left.name, &right.name))
        });
        let colors = self.active_theme.colors;
        let now = lapinux_core::util::now_unix();
        let battery = self
            .devices
            .iter()
            .filter(|device| device.connected)
            .filter_map(|device| device.battery)
            .max();
        let dock_entries = self.active_theme.dock.clone();
        let mut hidden = self.active_theme.hidden_pages.clone();
        hidden.extend(self.config.general.topbar_hidden.iter().cloned());

        if order.is_empty() {
            egui::CentralPanel::default()
                .frame(egui::Frame::none().fill(colors.background))
                .show(ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(ui.available_height() * 0.28);
                        ui.label(
                            egui::RichText::new(lapinux_core::i18n::t("shell.empty_library"))
                                .size(22.0)
                                .strong()
                                .color(colors.text),
                        );
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new(lapinux_core::i18n::t("shell.nx_empty_hint"))
                                .size(12.5)
                                .color(colors.text_dim),
                        );
                        ui.add_space(18.0);
                        if ui
                            .button(lapinux_core::i18n::t("shell.open_settings"))
                            .clicked()
                        {
                            self.open_settings(SettingsTab::General);
                        }
                    });
                });
            return;
        }

        let mut focus = self
            .nx_focus
            .and_then(|appid| order.iter().position(|index| base[*index].appid == appid))
            .unwrap_or(0)
            .min(order.len() - 1);
        let previous = focus;
        if ctx.input(|input| input.key_pressed(egui::Key::ArrowRight)) {
            focus = (focus + 1).min(order.len() - 1);
        }
        if ctx.input(|input| input.key_pressed(egui::Key::ArrowLeft)) {
            focus = focus.saturating_sub(1);
        }
        if focus != previous {
            self.nx_focus = Some(base[order[focus]].appid);
            ctx.request_repaint();
        }
        // Les couleurs de la jaquette sélectionnée colorent le halo du fond
        // et le panneau du dock, comme la teinte du jeu sur la console.
        let (tone, tone_deep) = {
            let entry = &base[order[focus]];
            self.covers.tone(&self.paths, entry.appid, entry.store)
        };
        let mut seen_controllers = HashSet::new();
        let connected_controllers: Vec<String> = self
            .devices
            .iter()
            .filter(|device| device.connected && device.gamepad)
            .filter(|device| seen_controllers.insert(controller_key(device)))
            .map(|device| device.name.clone())
            .collect();
        let mut launch: Option<u32> = None;
        if ctx.input(|input| input.key_pressed(egui::Key::Enter)) {
            let entry = &base[order[focus]];
            if entry.installed {
                launch = Some(entry.appid);
            } else {
                self.selected = Some(entry.appid);
            }
        }
        let mut detail: Option<u32> = None;
        let mut nav: Option<Page> = None;
        let mut open_settings = false;
        let mut close_app = false;
        let mut open_link: Option<String> = None;
        let mut cover_pending = false;

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(colors.background))
            .show(ctx, |ui| {
                let full = ui.max_rect();
                ui.painter().rect_filled(full, 0.0, colors.background);
                paint_nx_ambient(ui.painter(), full, tone, colors.background);
                let margin = 34.0;

                // Bandeau utilisateurs : des icônes rondes, sans libellé de
                // page. L'utilisateur actif porte le liseré blanc de la Switch.
                let avatar_y = full.top() + 34.0;
                nx_mii_avatar(
                    ui.painter(),
                    egui::pos2(full.left() + margin + 23.0, avatar_y),
                    23.0,
                    egui::Color32::from_rgb(224, 122, 47),
                    true,
                    colors.text,
                );
                nx_mii_avatar(
                    ui.painter(),
                    egui::pos2(full.left() + margin + 76.0, avatar_y),
                    19.0,
                    egui::Color32::from_rgb(47, 127, 193),
                    false,
                    colors.text,
                );

                // Heure, Wi-Fi et batterie, avec la typographie du bandeau
                // supérieur de la console.
                let status = egui::Rect::from_min_max(
                    egui::pos2(full.right() - margin - 240.0, full.top() + 16.0),
                    egui::pos2(full.right() - margin, full.top() + 52.0),
                );
                paint_nx_status(
                    ui.painter(),
                    status,
                    now,
                    battery,
                    24.0,
                    colors.text,
                    colors.text_dim,
                );

                // Le carrousel occupe le centre de l'écran. Le jeu choisi est
                // toujours au milieu ; ses voisins se répartissent de part et
                // d'autre, et son titre est posé juste au-dessus de la jaquette.
                let footer_baseline = full.bottom() - 20.0;
                let dock_center_y = full.bottom() - 56.0;
                let available_w = (full.width() - margin * 2.0).max(280.0);
                let gap = 16.0;
                let max_card = ((available_w - gap * 4.0) / 5.0).min(300.0);
                let row_area_top = full.top() + 92.0;
                let row_area_bottom = (dock_center_y - 50.0).max(row_area_top + 120.0);
                let vertical = (row_area_bottom - row_area_top - 30.0).max(120.0);
                let card_h = max_card.min(vertical * 0.82).clamp(120.0, 300.0);
                let card_w = card_h;
                let slots = {
                    let fit = (((available_w + gap) / (card_w + gap)).floor() as usize).clamp(1, 5);
                    if fit % 2 == 0 {
                        (fit - 1).max(1)
                    } else {
                        fit
                    }
                };
                let (start, shown) = if order.len() <= slots {
                    (0, order.len())
                } else {
                    (
                        focus.saturating_sub(slots / 2).min(order.len() - slots),
                        slots,
                    )
                };
                let row_center_y = (row_area_top + row_area_bottom) / 2.0;
                let card_top = row_center_y - card_h / 2.0;
                let radius = self.active_theme.rounding.clamp(0.0, 6.0);
                let rounding = egui::Rounding::same(radius);
                let row_width = shown as f32 * card_w + shown.saturating_sub(1) as f32 * gap;
                let row_left = full.center().x - row_width / 2.0;
                let mut selected_rect = None;
                let mut selected_texture = None;

                for slot in 0..shown {
                    let index = start + slot;
                    let entry = &base[order[index]];
                    let rect = egui::Rect::from_min_size(
                        egui::pos2(row_left + slot as f32 * (card_w + gap), card_top),
                        egui::vec2(card_w, card_h),
                    );
                    if index == focus {
                        selected_rect = Some(rect);
                    }
                    let response = ui.interact(
                        rect,
                        egui::Id::new(("nx-game", entry.appid)),
                        egui::Sense::click(),
                    );
                    let texture = self.covers.get(
                        ui.ctx(),
                        &self.paths,
                        entry.appid,
                        &entry.name,
                        entry.store,
                        &entry.cover,
                        CoverTier::Sheet,
                    );
                    if index == focus {
                        selected_texture = texture.clone();
                    }
                    match texture.as_ref() {
                        Some(texture) => {
                            paint_cover_at(ui, rect, texture, rounding, egui::vec2(0.5, 0.5));
                        }
                        None => {
                            ui.painter().rect_filled(rect, rounding, colors.card);
                            ui.painter().text(
                                rect.center(),
                                egui::Align2::CENTER_CENTER,
                                entry.name.chars().next().unwrap_or('?'),
                                display_font(34.0),
                                colors.text_dim,
                            );
                            cover_pending = true;
                        }
                    }
                    if response.hovered() {
                        ui.painter().rect_stroke(
                            rect.shrink(1.0),
                            rounding,
                            egui::Stroke::new(2.0_f32, colors.text),
                        );
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    if response.clicked() {
                        if index == focus {
                            if entry.installed {
                                launch = Some(entry.appid);
                            } else {
                                detail = Some(entry.appid);
                            }
                        } else {
                            focus = index;
                        }
                    }
                    if response.double_clicked() {
                        if entry.installed {
                            launch = Some(entry.appid);
                        } else {
                            detail = Some(entry.appid);
                        }
                    }
                }

                if let Some(rect) = selected_rect {
                    // Liseré blanc de sélection, et titre au-dessus de la
                    // jaquette : la signature de l'accueil Switch.
                    ui.painter().rect_stroke(
                        rect.expand(3.0),
                        egui::Rounding::same(radius + 3.0),
                        egui::Stroke::new(3.0_f32, colors.text),
                    );
                    let entry = &base[order[focus]];
                    // La petite vignette devant le titre est la jaquette du
                    // jeu, pas une cartouche générique.
                    let title = lapinux_core::util::ellipsize(&entry.name, 42);
                    let galley = ui.painter().layout_no_wrap(
                        title,
                        egui::FontId::proportional(18.0),
                        colors.text,
                    );
                    let icon = 24.0;
                    let gap = 8.0;
                    let total = icon + gap + galley.size().x;
                    let left = (rect.center().x - total / 2.0).clamp(
                        full.left() + margin,
                        (full.right() - margin - total).max(full.left() + margin),
                    );
                    let title_top = rect.top() - 8.0 - galley.size().y;
                    let icon_rect = egui::Rect::from_min_size(
                        egui::pos2(left, title_top + (galley.size().y - icon) / 2.0),
                        egui::vec2(icon, icon),
                    );
                    match selected_texture.as_ref() {
                        Some(texture) => {
                            paint_cover_at(
                                ui,
                                icon_rect,
                                texture,
                                egui::Rounding::same(5.0),
                                egui::vec2(0.5, 0.5),
                            );
                            ui.painter().rect_stroke(
                                icon_rect,
                                egui::Rounding::same(5.0),
                                egui::Stroke::new(1.0_f32, tone_deep),
                            );
                        }
                        None => {
                            ui.painter().rect_filled(
                                icon_rect,
                                egui::Rounding::same(5.0),
                                colors.card,
                            );
                            ui.painter().text(
                                icon_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                entry.name.chars().next().unwrap_or('?'),
                                display_font(12.0),
                                colors.text_dim,
                            );
                        }
                    }
                    ui.painter().galley(
                        egui::pos2(left + icon + gap, title_top),
                        galley,
                        colors.text,
                    );
                    let playing = self
                        .session
                        .as_ref()
                        .is_some_and(|session| session.appid == entry.appid);
                    let label = if entry.installed {
                        if playing {
                            lapinux_core::i18n::t("shell.game_running")
                        } else {
                            lapinux_core::i18n::t("library.action.play")
                        }
                    } else {
                        lapinux_core::i18n::t("shell.view_page")
                    };
                    let label_w = label.chars().count() as f32 * 7.2;
                    let pill = egui::Rect::from_center_size(
                        egui::pos2(rect.center().x, rect.bottom() - 26.0),
                        egui::vec2(label_w + 46.0, 30.0),
                    );
                    ui.painter().rect(
                        pill,
                        egui::Rounding::same(15.0),
                        egui::Color32::from_rgb(248, 248, 248),
                        egui::Stroke::new(1.0_f32, egui::Color32::from_black_alpha(30)),
                    );
                    if entry.installed {
                        let c = egui::pos2(pill.center().x - label_w / 2.0 - 10.0, pill.center().y);
                        ui.painter().add(egui::Shape::convex_polygon(
                            vec![
                                c + egui::vec2(-3.5, -4.5),
                                c + egui::vec2(-3.5, 4.5),
                                c + egui::vec2(4.5, 0.0),
                            ],
                            egui::Color32::from_rgb(28, 28, 28),
                            egui::Stroke::NONE,
                        ));
                    }
                    ui.painter().text(
                        egui::pos2(pill.center().x + 4.0, pill.center().y),
                        egui::Align2::CENTER_CENTER,
                        label,
                        display_font(13.5),
                        egui::Color32::from_rgb(25, 25, 25),
                    );
                    // Le clic sur la capsule lance sans viser la jaquette.
                    let play = ui.interact(
                        pill,
                        egui::Id::new(("nx-play", entry.appid)),
                        egui::Sense::click(),
                    );
                    if play.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    if play.clicked() {
                        if entry.installed {
                            launch = Some(entry.appid);
                        } else {
                            detail = Some(entry.appid);
                        }
                    }
                }

                // Dock : les icônes système sont posées à même le fond, sans
                // pastille. L'ordre suit celui de la Switch (News, eShop,
                // Album, etc.), avec les pages propres à Lapinux.
                let dock_pages: Vec<Page> = [
                    Page::Home,
                    Page::News,
                    Page::Downloads,
                    Page::Boutiques,
                    Page::Captures,
                    Page::Collections,
                    Page::Controllers,
                    Page::Profile,
                ]
                .into_iter()
                .filter(|page| !hidden.iter().any(|key| key == page.key()))
                .collect();
                let item_w = 58.0;
                let item_gap = 12.0;
                let plugin_count = dock_entries.len();
                let item_count = dock_pages.len() + plugin_count + 2;
                let dock_w =
                    item_count as f32 * item_w + item_count.saturating_sub(1) as f32 * item_gap;
                let mut item_center = full.center().x - dock_w / 2.0 + item_w / 2.0;
                // Un panneau translucide teinté par le jeu, avec des icônes
                // sombres : le dock ressemble à une barre de console, pas à
                // une rangée d'icônes flottantes.
                let panel_rgb = blend_colors(egui::Color32::from_rgb(248, 248, 248), tone, 0.42);
                let panel_fill = egui::Color32::from_rgba_unmultiplied(
                    panel_rgb.r(),
                    panel_rgb.g(),
                    panel_rgb.b(),
                    238,
                );
                let panel_ink = blend_colors(tone, egui::Color32::from_rgb(18, 18, 20), 0.72);
                let dock_panel = egui::Rect::from_center_size(
                    egui::pos2(full.center().x, dock_center_y),
                    egui::vec2(dock_w + 44.0, 70.0),
                );
                ui.painter().rect_filled(
                    dock_panel.expand(3.0),
                    egui::Rounding::same(38.0),
                    egui::Color32::from_black_alpha(30),
                );
                ui.painter().rect(
                    dock_panel,
                    egui::Rounding::same(35.0),
                    panel_fill,
                    egui::Stroke::new(1.0_f32, egui::Color32::from_white_alpha(120)),
                );

                for page in &dock_pages {
                    let rect = egui::Rect::from_center_size(
                        egui::pos2(item_center, dock_center_y),
                        egui::vec2(item_w, item_w),
                    );
                    let response = ui.interact(
                        rect,
                        egui::Id::new(("nx-dock", page.key())),
                        egui::Sense::click(),
                    );
                    if response.hovered() {
                        ui.painter().circle_filled(
                            rect.center(),
                            27.0,
                            egui::Color32::from_black_alpha(22),
                        );
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    let progress = icon_hover_progress(
                        ui,
                        ui.id().with(("nx-page-icon-motion", page.key())),
                        response.hovered() || response.has_focus(),
                    );
                    section_icons::page_animated(
                        ui.painter(),
                        *page,
                        rect.center(),
                        26.0,
                        panel_ink,
                        progress,
                    );
                    let response = response
                        .on_hover_text(page_label(*page))
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                    if response.clicked() {
                        nav = Some(*page);
                    }
                    item_center += item_w + item_gap;
                }

                for shortcut in &dock_entries {
                    let rect = egui::Rect::from_center_size(
                        egui::pos2(item_center, dock_center_y),
                        egui::vec2(item_w, item_w),
                    );
                    let response = ui.interact(
                        rect,
                        egui::Id::new(("nx-dock-plugin", shortcut.label.as_str())),
                        egui::Sense::click(),
                    );
                    if response.hovered() {
                        ui.painter().circle_filled(
                            rect.center(),
                            27.0,
                            egui::Color32::from_black_alpha(22),
                        );
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        shortcut
                            .icon
                            .chars()
                            .next()
                            .unwrap_or_else(|| shortcut.label.chars().next().unwrap_or('•')),
                        display_font(23.0),
                        panel_ink,
                    );
                    let response = response.on_hover_text(&shortcut.label);
                    if response.clicked() {
                        if let Some(page) = shortcut.page.as_deref().and_then(Page::from_key) {
                            nav = Some(page);
                        } else if let Some(url) = &shortcut.url {
                            open_link = Some(url.clone());
                        }
                    }
                    item_center += item_w + item_gap;
                }

                // Réglages.
                let gear_rect = egui::Rect::from_center_size(
                    egui::pos2(item_center, dock_center_y),
                    egui::vec2(item_w, item_w),
                );
                let gear = ui.interact(
                    gear_rect,
                    egui::Id::new("nx-dock-settings"),
                    egui::Sense::click(),
                );
                if gear.hovered() {
                    ui.painter().circle_filled(
                        gear_rect.center(),
                        27.0,
                        egui::Color32::from_black_alpha(22),
                    );
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                let progress = icon_hover_progress(
                    ui,
                    ui.id().with("nx-dock-gear-motion"),
                    gear.hovered() || gear.has_focus(),
                );
                section_icons::gear(ui.painter(), gear_rect.center(), 26.0, panel_ink, progress);
                let gear = gear.on_hover_text(lapinux_core::i18n::t("shell.settings"));
                if gear.clicked() {
                    open_settings = true;
                }
                item_center += item_w + item_gap;

                // Extinction.
                let power_rect = egui::Rect::from_center_size(
                    egui::pos2(item_center, dock_center_y),
                    egui::vec2(item_w, item_w),
                );
                let power = ui.interact(
                    power_rect,
                    egui::Id::new("nx-dock-power"),
                    egui::Sense::click(),
                );
                if power.hovered() {
                    ui.painter().circle_filled(
                        power_rect.center(),
                        27.0,
                        egui::Color32::from_black_alpha(22),
                    );
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                ui.painter().text(
                    power_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    ICON_POWER.to_string(),
                    icon_font(24.0),
                    panel_ink,
                );
                let power = power.on_hover_text(lapinux_core::i18n::t("shell.close_app"));
                if power.clicked() {
                    close_app = true;
                }

                // En bas à gauche, la manette active ; à droite, les boutons
                // de la console avec leurs pastilles, comme sur la Switch.
                let controllers_x = full.left() + margin + 12.0;
                if connected_controllers.is_empty() {
                    let controller_rect = egui::Rect::from_center_size(
                        egui::pos2(controllers_x, dock_center_y),
                        egui::vec2(38.0, 38.0),
                    );
                    let controller = ui.interact(
                        controller_rect,
                        egui::Id::new("nx-controller"),
                        egui::Sense::hover(),
                    );
                    paint_nx_controller(ui.painter(), controller_rect.center());
                    let _ = controller.on_hover_text(lapinux_core::i18n::t("profile_page.no_pad"));
                } else {
                    for (index, name) in connected_controllers.iter().take(3).enumerate() {
                        let center = egui::pos2(controllers_x + index as f32 * 40.0, dock_center_y);
                        let controller_rect =
                            egui::Rect::from_center_size(center, egui::vec2(38.0, 38.0));
                        let controller = ui.interact(
                            controller_rect,
                            egui::Id::new(("nx-controller", index)),
                            egui::Sense::hover(),
                        );
                        paint_nx_controller(ui.painter(), center);
                        ui.painter().circle_filled(
                            egui::pos2(center.x - 12.0, center.y - 12.0),
                            3.0,
                            egui::Color32::from_rgb(94, 210, 110),
                        );
                        let _ = controller.on_hover_text(name);
                    }
                }
                if full.width() > 980.0 {
                    let mut x = full.right() - margin;
                    x = nx_key_hint(
                        ui.painter(),
                        x,
                        footer_baseline,
                        "+",
                        "Options",
                        colors.text,
                        colors.text_dim,
                        colors.background,
                    ) - 18.0;
                    x = nx_key_hint(
                        ui.painter(),
                        x,
                        footer_baseline,
                        "X",
                        lapinux_core::i18n::t("shell.close"),
                        colors.text,
                        colors.text_dim,
                        colors.background,
                    ) - 18.0;
                    let _ = nx_key_hint(
                        ui.painter(),
                        x,
                        footer_baseline,
                        "A",
                        lapinux_core::i18n::t("shell.open"),
                        colors.text,
                        colors.text_dim,
                        colors.background,
                    );
                }
            });

        if cover_pending {
            // Le cache décode quelques jaquettes par image : sans cette
            // relance, les cartes hors budget resteraient en placeholder
            // jusqu'au prochain mouvement de souris.
            ctx.request_repaint_after(Duration::from_millis(200));
        }
        if focus != previous || self.nx_focus.is_none() {
            self.nx_focus = Some(base[order[focus]].appid);
        }
        if let Some(appid) = detail {
            self.selected = Some(appid);
        }
        if let Some(page) = nav {
            self.page = page;
            self.collection_detail_open = false;
            self.selected = None;
        }
        if open_settings {
            self.open_settings(SettingsTab::General);
        }
        if close_app {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if let Some(url) = open_link {
            open_url(&url);
        }
        if let Some(appid) = launch {
            self.launch(appid, false);
        }
    }

    /// Bandeau NX des pages autres que l'accueil : un retour, le nom de la
    /// section et l'état de la machine. Les onglets classiques sont remplacés
    /// par le dock de l'accueil, qui reste à un clic.
    pub(super) fn nx_page_bar(&mut self, ctx: &egui::Context) {
        let colors = self.active_theme.colors;
        let now = lapinux_core::util::now_unix();
        let battery = self
            .devices
            .iter()
            .filter(|device| device.connected)
            .filter_map(|device| device.battery)
            .max();
        let title = page_label(self.page).to_string();
        let initial = self
            .steam_user
            .trim()
            .chars()
            .next()
            .map(|ch| ch.to_uppercase().to_string())
            .unwrap_or_else(|| "J".to_string());
        let mut back = false;
        let mut settings = false;
        let panel = egui::TopBottomPanel::top("nx-pagebar")
            .exact_height(58.0)
            .show_separator_line(false)
            .frame(
                egui::Frame::none()
                    .fill(colors.panel)
                    .inner_margin(egui::Margin::symmetric(18.0, 4.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    let (back_rect, back_response) =
                        ui.allocate_exact_size(egui::vec2(32.0, 32.0), egui::Sense::click());
                    let c = back_rect.center();
                    ui.painter().line_segment(
                        [c + egui::vec2(5.0, -7.0), c + egui::vec2(-3.0, 0.0)],
                        egui::Stroke::new(1.7_f32, colors.text),
                    );
                    ui.painter().line_segment(
                        [c + egui::vec2(-3.0, 0.0), c + egui::vec2(5.0, 7.0)],
                        egui::Stroke::new(1.7_f32, colors.text),
                    );
                    if back_response.hovered() {
                        ui.painter().circle_filled(c, 14.0, colors.card_hover);
                    }
                    if back_response.clicked() {
                        back = true;
                    }
                    back_response.on_hover_text(lapinux_core::i18n::t("page.home"));
                    ui.add_space(4.0);
                    let avatar_center = ui.cursor().left_top() + egui::vec2(15.0, 16.0);
                    nx_avatar(
                        ui.painter(),
                        avatar_center,
                        13.0,
                        &initial,
                        colors.accent,
                        egui::Color32::from_rgb(40, 107, 161),
                        colors.text,
                    );
                    ui.add_space(38.0);
                    ui.label(
                        egui::RichText::new(title)
                            .size(15.0)
                            .strong()
                            .color(colors.text),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let (gear_rect, gear_response) =
                            ui.allocate_exact_size(egui::vec2(34.0, 34.0), egui::Sense::click());
                        section_icons::gear(
                            ui.painter(),
                            gear_rect.center(),
                            section_icons::INTERFACE_ICON_SIZE,
                            if gear_response.hovered() {
                                colors.text
                            } else {
                                colors.text_dim
                            },
                            icon_hover_progress(
                                ui,
                                ui.id().with("nx-page-gear-motion"),
                                gear_response.hovered() || gear_response.has_focus(),
                            ),
                        );
                        let gear_response = gear_response
                            .on_hover_text(lapinux_core::i18n::t("shell.settings_shortcut"));
                        if gear_response.clicked() {
                            settings = true;
                        }
                        let status = egui::Rect::from_min_size(
                            egui::pos2(ui.cursor().left() - 218.0, ui.cursor().top()),
                            egui::vec2(218.0, 34.0),
                        );
                        paint_nx_status(
                            ui.painter(),
                            status,
                            now,
                            battery,
                            18.0,
                            colors.text,
                            colors.text_dim,
                        );
                        ui.add_space(224.0);
                    });
                });
            });
        ctx.layer_painter(egui::LayerId::background()).line_segment(
            [
                egui::pos2(
                    panel.response.rect.left(),
                    panel.response.rect.bottom() - 0.5,
                ),
                egui::pos2(
                    panel.response.rect.right(),
                    panel.response.rect.bottom() - 0.5,
                ),
            ],
            egui::Stroke::new(1.0_f32, colors.border),
        );
        if back {
            self.page = Page::Library;
            self.selected = None;
            self.collection_detail_open = false;
        }
        if settings {
            if self.settings_open {
                self.close_settings();
            } else {
                self.open_settings(SettingsTab::General);
            }
        }
    }

    /// Notifications Steam et activité des amis, dans un panneau fixe. Les
    /// commentaires du profil n'y figurent pas : ce sont des messages de la
    /// page communautaire, pas des notifications du lanceur.
    pub(super) fn notifications_panel(&mut self, ctx: &egui::Context) {
        if !self.notifications_open {
            return;
        }
        let notification_prefs = self.config.notifications.clone();
        let notification_since = notification_cutoff(&notification_prefs, notification_now());
        fn notification_card(
            ui: &mut egui::Ui,
            key: &str,
            title: &str,
            count: Option<u32>,
            detail: &str,
            at: i64,
        ) -> bool {
            let mut deleted = false;
            egui::Frame::none()
                .fill(CARD())
                .stroke(egui::Stroke::new(1.0_f32, BORDER()))
                .rounding(egui::Rounding::same(10.0))
                .inner_margin(egui::Margin::symmetric(14.0, 12.0))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(title).size(13.0).strong().color(TEXT()));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if notification_delete_button(ui, key) {
                                deleted = true;
                            }
                            let time = notification_time(at);
                            if !time.is_empty() {
                                ui.label(egui::RichText::new(time).size(10.0).color(TEXT_DIM()));
                            }
                            if let Some(count) = count {
                                ui.label(
                                    egui::RichText::new(format!("×{count}"))
                                        .size(13.0)
                                        .strong()
                                        .color(TEXT()),
                                );
                            }
                        });
                    });
                    if !detail.trim().is_empty() {
                        ui.add_space(4.0);
                        ui.label(egui::RichText::new(detail).size(11.5).color(TEXT_DIM()));
                    }
                });
            ui.add_space(8.0);
            deleted
        }
        fn notification_time(at: i64) -> String {
            if at > 0 {
                lapinux_core::util::format_datetime(at)
            } else {
                String::new()
            }
        }
        /// Corbeille d'une carte : la même icône au trait que la galerie,
        /// avec un fond discret au survol et une infobulle explicite.
        fn notification_delete_button(ui: &mut egui::Ui, key: &str) -> bool {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::hover());
            let response = ui.interact(
                rect,
                ui.id().with(("notification-delete", key)),
                egui::Sense::click(),
            );
            let hovered = response.hovered();
            if hovered {
                ui.painter()
                    .rect_filled(rect, egui::Rounding::same(5.0), CARD_ACTIVE());
            }
            crate::captures::paint_trash_glyph(
                ui.painter(),
                rect.center(),
                12.0,
                if hovered { DANGER } else { TEXT_DIM() },
            );
            response
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text(lapinux_core::i18n::t("notifications_panel.delete"))
                .clicked()
        }
        /// Les événements d'amis arrivent parfois dans la même minute : la
        /// seconde évite de confondre une connexion et un lancement de jeu.
        fn friend_event_time(at: i64) -> String {
            if at > 0 {
                lapinux_core::util::format_datetime_seconds(at)
            } else {
                String::new()
            }
        }
        fn friend_event_avatar(
            ui: &mut egui::Ui,
            avatar: Option<&egui::TextureHandle>,
            name: &str,
            size: f32,
        ) {
            match avatar {
                Some(texture) => {
                    ui.add(
                        egui::Image::new((texture.id(), egui::vec2(size, size)))
                            .rounding(egui::Rounding::same(size / 2.0)),
                    );
                }
                None => {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
                    ui.painter()
                        .circle_filled(rect.center(), size / 2.0, CARD_ACTIVE());
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        name.trim()
                            .chars()
                            .next()
                            .map(|letter| letter.to_uppercase().to_string())
                            .unwrap_or_else(|| "?".to_string()),
                        egui::FontId::proportional(size * 0.48),
                        TEXT(),
                    );
                }
            }
        }
        fn friend_game_icon(ui: &mut egui::Ui, icon: Option<&egui::TextureHandle>, size: f32) {
            let size = egui::vec2(size, size);
            match icon {
                Some(texture) => {
                    ui.add(
                        egui::Image::new((texture.id(), size)).rounding(egui::Rounding::same(4.0)),
                    );
                }
                None => {
                    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                    ui.painter()
                        .rect_filled(rect, egui::Rounding::same(4.0), PANEL());
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        nav_glyph(Page::Library),
                        egui::FontId::proportional(11.0),
                        TEXT_DIM(),
                    );
                }
            }
        }
        fn steam_logo(ui: &mut egui::Ui, size: f32) {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
            // `paint_steam_icon` raisonne en multiples de 11,4 px : le logo
            // garde ainsi la même taille optique que les autres marques.
            paint_steam_icon(ui.painter(), rect.center(), TEXT_DIM(), size / 11.4);
        }
        fn friend_event_card(
            ui: &mut egui::Ui,
            key: &str,
            event: &SteamFriendEvent,
            icon: Option<&egui::TextureHandle>,
            avatar: Option<&egui::TextureHandle>,
        ) -> bool {
            use lapinux_core::social::SteamFriendEventKind;
            let game_event = matches!(
                event.kind,
                SteamFriendEventKind::GameStarted | SteamFriendEventKind::GameStopped
            );
            let name = event.name.trim();
            let name = if name.is_empty() {
                lapinux_core::i18n::t("notifications_panel.a_friend")
            } else {
                name
            };
            let time = friend_event_time(event.at);
            let mut deleted = false;
            egui::Frame::none()
                .fill(CARD())
                .stroke(egui::Stroke::new(1.0_f32, BORDER()))
                .rounding(egui::Rounding::same(10.0))
                .inner_margin(egui::Margin::symmetric(14.0, 12.0))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        friend_event_avatar(ui, avatar, name, 24.0);
                        if game_event {
                            let verb = match event.kind {
                                SteamFriendEventKind::GameStarted => lapinux_core::i18n::t(
                                    "notifications_panel.friend_game_started_verb",
                                ),
                                _ => lapinux_core::i18n::t(
                                    "notifications_panel.friend_game_stopped_verb",
                                ),
                            };
                            let game = event.game.trim();
                            let game = if game.is_empty() {
                                lapinux_core::i18n::t("notifications_panel.a_game")
                            } else {
                                game
                            };
                            ui.label(egui::RichText::new(name).size(12.5).strong().color(TEXT()));
                            ui.label(egui::RichText::new(verb).size(12.5).color(TEXT_DIM()));
                            friend_game_icon(ui, icon, 20.0);
                            ui.label(egui::RichText::new(game).size(12.5).strong().color(TEXT()));
                        } else {
                            let verb = match event.kind {
                                SteamFriendEventKind::Online => {
                                    lapinux_core::i18n::t("notifications_panel.friend_online_verb")
                                }
                                _ => {
                                    lapinux_core::i18n::t("notifications_panel.friend_offline_verb")
                                }
                            };
                            ui.label(egui::RichText::new(name).size(12.5).strong().color(TEXT()));
                            ui.label(egui::RichText::new(verb).size(12.5).color(TEXT_DIM()));
                            steam_logo(ui, 18.0);
                            ui.label(
                                egui::RichText::new("Steam")
                                    .size(12.5)
                                    .strong()
                                    .color(TEXT()),
                            );
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if notification_delete_button(ui, key) {
                                deleted = true;
                            }
                            if !time.is_empty() {
                                ui.label(egui::RichText::new(time).size(10.0).color(TEXT_DIM()));
                            }
                        });
                    });
                });
            ui.add_space(6.0);
            deleted
        }
        fn web_notification_detail(
            notification: &lapinux_core::social::SteamWebNotification,
        ) -> String {
            let title = notification.title.trim();
            let body = notification.body.trim();
            match (title.is_empty(), body.is_empty()) {
                (false, false) => format!("{title} — {body}"),
                (false, true) => title.to_string(),
                (true, false) => body.to_string(),
                (true, true) => String::new(),
            }
        }
        fn web_notification_card(
            ui: &mut egui::Ui,
            key: &str,
            notification: &lapinux_core::social::SteamWebNotification,
        ) -> bool {
            let title = notification.label();
            let mut deleted = false;
            egui::Frame::none()
                .fill(CARD())
                .stroke(egui::Stroke::new(1.0_f32, BORDER()))
                .rounding(egui::Rounding::same(10.0))
                .inner_margin(egui::Margin::symmetric(14.0, 12.0))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let url = notification.url.trim();
                    let time = notification_time(notification.at);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(title).size(13.0).strong().color(TEXT()));
                        if !url.is_empty() {
                            ui.hyperlink_to(
                                lapinux_core::i18n::t("notifications_panel.open_link"),
                                url,
                            );
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if notification_delete_button(ui, key) {
                                deleted = true;
                            }
                            if !time.is_empty() {
                                ui.label(egui::RichText::new(time).size(10.0).color(TEXT_DIM()));
                            }
                        });
                    });
                    let detail = web_notification_detail(notification);
                    if !detail.is_empty() {
                        ui.add_space(4.0);
                        ui.label(egui::RichText::new(detail).size(11.5).color(TEXT_DIM()));
                    }
                });
            ui.add_space(6.0);
            deleted
        }
        fn wishlist_price(deal: &lapinux_core::social::SteamWishlistDeal) -> String {
            let price = deal.price.trim();
            if deal.discount > 0 {
                if price.is_empty() {
                    format!("-{} %", deal.discount)
                } else {
                    format!("-{} % · {}", deal.discount, price)
                }
            } else {
                price.to_string()
            }
        }
        fn wishlist_icon(
            ui: &mut egui::Ui,
            icon: Option<&egui::TextureHandle>,
            size: f32,
        ) -> egui::Response {
            let size = egui::vec2(size, size);
            match icon {
                Some(texture) => ui.add(
                    egui::Image::new((texture.id(), size)).rounding(egui::Rounding::same(6.0)),
                ),
                None => {
                    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
                    ui.painter()
                        .rect_filled(rect, egui::Rounding::same(6.0), PANEL());
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        nav_glyph(Page::Library),
                        egui::FontId::proportional((size.y * 0.38).max(9.0)),
                        TEXT_DIM(),
                    );
                    response
                }
            }
        }
        fn wishlist_card(
            ui: &mut egui::Ui,
            notification: &lapinux_core::social::SteamWebNotification,
            entries: &[WishlistCardEntry],
            expanded: &mut std::collections::HashSet<String>,
            open_web: &mut Option<(String, String)>,
        ) -> bool {
            let title = lapinux_core::i18n::t("notifications_panel.wishlist_promotions");
            let time = notification_time(notification.at);
            let url = notification.url.trim();
            let key = format!(
                "{}:{}:{}",
                notification.at, notification.kind, notification.url
            );
            let mut is_open = expanded.contains(&key);
            let mut deleted = false;
            egui::Frame::none()
                .fill(CARD())
                .stroke(egui::Stroke::new(1.0_f32, BORDER()))
                .rounding(egui::Rounding::same(10.0))
                .inner_margin(egui::Margin::symmetric(14.0, 12.0))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.spacing_mut().item_spacing.x = 6.0;
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(title).size(13.0).strong().color(TEXT()));
                        ui.label(
                            egui::RichText::new(format!("×{}", entries.len()))
                                .size(11.0)
                                .color(TEXT_DIM()),
                        );
                        let toggle = if is_open {
                            lapinux_core::i18n::t("notifications_panel.wishlist_hide_details")
                        } else {
                            lapinux_core::i18n::t("notifications_panel.wishlist_show_details")
                        };
                        if ui.link(egui::RichText::new(toggle).size(10.5)).clicked() {
                            is_open = !is_open;
                            if is_open {
                                expanded.insert(key.clone());
                            } else {
                                expanded.remove(&key);
                            }
                        }
                        if !url.is_empty()
                            && ui
                                .link(
                                    egui::RichText::new(lapinux_core::i18n::t(
                                        "notifications_panel.open_link",
                                    ))
                                    .size(10.5),
                                )
                                .clicked()
                        {
                            *open_web = Some((url.to_string(), format!("{title} · Lapinux")));
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if notification_delete_button(ui, &key) {
                                deleted = true;
                            }
                            if !time.is_empty() {
                                ui.label(egui::RichText::new(time).size(10.0).color(TEXT_DIM()));
                            }
                        });
                    });
                    ui.add_space(8.0);
                    egui::ScrollArea::horizontal()
                        .id_salt(("notifications-wishlist-icons", key.as_str()))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 5.0;
                                for entry in entries {
                                    let response = wishlist_icon(ui, entry.icon.as_ref(), 36.0);
                                    let price = wishlist_price(&entry.deal);
                                    let hint = if price.is_empty() {
                                        entry.deal.name.clone()
                                    } else {
                                        format!("{}\n{}", entry.deal.name, price)
                                    };
                                    response.on_hover_text(hint);
                                }
                            });
                        });
                    if is_open {
                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(4.0);
                        // Pas de scroll interne : les détails prennent toute
                        // la largeur de la carte et le panneau gère la suite.
                        ui.set_width(ui.available_width());
                        for entry in entries {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 6.0;
                                wishlist_icon(ui, entry.icon.as_ref(), 26.0);
                                ui.vertical(|ui| {
                                    ui.set_width(ui.available_width());
                                    ui.spacing_mut().item_spacing.y = 1.0;
                                    ui.label(
                                        egui::RichText::new(&entry.deal.name)
                                            .size(11.5)
                                            .color(TEXT()),
                                    );
                                    let price = wishlist_price(&entry.deal);
                                    if !price.is_empty() {
                                        let tone = if entry.deal.discount > 0 {
                                            ACCENT()
                                        } else {
                                            TEXT_DIM()
                                        };
                                        ui.label(egui::RichText::new(price).size(10.0).color(tone));
                                    }
                                });
                            });
                            ui.add_space(4.0);
                        }
                    }
                });
            ui.add_space(6.0);
            deleted
        }
        fn steam_notification_detail(kind: u32) -> &'static str {
            match kind {
                1 => lapinux_core::i18n::t("notifications_panel.items"),
                2 => lapinux_core::i18n::t("notifications_panel.gifts"),
                3 => lapinux_core::i18n::t("notifications_panel.offline_messages"),
                4 => lapinux_core::i18n::t("notifications_panel.friend_requests"),
                5 => lapinux_core::i18n::t("notifications_panel.trades"),
                _ => lapinux_core::i18n::t("notifications_panel.generic"),
            }
        }

        let (live, steam_connecting, all_notifications, all_web_notifications) = self
            .social
            .as_ref()
            .map_or((false, true, Vec::new(), Vec::new()), |social| {
                (
                    social.state == "live",
                    social.state == "connecting",
                    social.notifications.clone(),
                    social.web_notifications.clone(),
                )
            });
        let notifications: Vec<lapinux_core::social::SteamNotification> = if notification_prefs
            .enabled
            && notification_prefs.steam_other
        {
            all_notifications
                .into_iter()
                .filter(|notification| notification.count > 0)
                .filter(|notification| notification_is_recent(notification.at, notification_since))
                .filter(|notification| {
                    !notification_prefs
                        .dismissed_notifications
                        .contains(&notification_counter_key(notification))
                })
                .collect()
        } else {
            Vec::new()
        };
        let mut web_notifications: Vec<lapinux_core::social::SteamWebNotification> =
            if notification_prefs.enabled {
                all_web_notifications
                    .into_iter()
                    .filter(|notification| {
                        notification_prefs.web_notification_enabled(notification.kind)
                    })
                    .filter(|notification| {
                        notification_is_recent(notification.at, notification_since)
                    })
                    .filter(|notification| {
                        !notification_prefs
                            .dismissed_notifications
                            .contains(&notification_web_key(notification))
                    })
                    .collect()
            } else {
                Vec::new()
            };
        web_notifications.sort_by(|left, right| right.at.cmp(&left.at));
        let (wishlist_notifications, other_web_notifications): (Vec<_>, Vec<_>) = web_notifications
            .into_iter()
            .partition(|notification| notification.kind == 8);
        let mut wishlist_entries: Vec<Vec<WishlistCardEntry>> =
            Vec::with_capacity(wishlist_notifications.len());
        for notification in &wishlist_notifications {
            let mut seen = std::collections::HashSet::new();
            let mut entries = Vec::new();
            for deal in &notification.deals {
                // Un même jeu ne doit pas apparaître deux fois dans la même
                // notification.
                if deal.appid != 0 && !seen.insert(deal.appid) {
                    continue;
                }
                entries.push(WishlistCardEntry {
                    deal: deal.clone(),
                    icon: self.art.game_icon(ctx, &self.paths, deal.appid),
                });
            }
            wishlist_entries.push(entries);
        }
        // Le daemon complète le nom des jeux d'amis ; le catalogue et la
        // boutique prennent le relais pour les événements plus anciens, que
        // Steam n'a pas nommés.
        let mut game_names: std::collections::HashMap<u32, String> =
            std::collections::HashMap::new();
        let mut avatar_urls: std::collections::HashMap<u64, String> =
            std::collections::HashMap::new();
        if let Some(social) = self.social.as_ref() {
            for friend in &social.friends {
                if friend.game_appid != 0 && !friend.game.trim().is_empty() {
                    game_names.insert(friend.game_appid, friend.game.clone());
                }
                if !friend.avatar.trim().is_empty() {
                    avatar_urls.insert(friend.steam_id, friend.avatar.clone());
                }
            }
            for event in &social.friend_events {
                if event.appid != 0 && !event.game.trim().is_empty() {
                    game_names.insert(event.appid, event.game.clone());
                }
                if !event.avatar.trim().is_empty() {
                    avatar_urls
                        .entry(event.steam_id)
                        .or_insert_with(|| event.avatar.clone());
                }
            }
        }
        if let Some(catalog) = self.catalog.as_ref() {
            for entry in &catalog.entries {
                if !entry.name.trim().is_empty() {
                    game_names
                        .entry(entry.appid)
                        .or_insert_with(|| entry.name.clone());
                }
            }
        }
        let mut friend_events: Vec<SteamFriendEvent> = self
            .friend_events_history
            .iter()
            .filter(|event| {
                !notification_prefs
                    .dismissed_notifications
                    .contains(&friend_notification_key(event))
            })
            .filter(|event| notification_prefs.friend_event_enabled(event.kind))
            .filter(|event| notification_is_recent(event.at, notification_since))
            .cloned()
            .collect();
        friend_events.sort_by(|left, right| right.at.cmp(&left.at));
        friend_events.truncate(20);
        for event in &mut friend_events {
            if event.game.trim().is_empty() {
                if let Some((appid, name)) = self.friend_games.get(&event.steam_id) {
                    if event.appid == 0 || event.appid == *appid {
                        event.game = name.clone();
                        if event.appid == 0 {
                            event.appid = *appid;
                        }
                    }
                }
            }
            if event.game.trim().is_empty() && event.appid != 0 {
                if let Some(name) = game_names.get(&event.appid) {
                    event.game = name.clone();
                } else if let Some(name) = crate::steam_app_name_async(ctx, event.appid) {
                    event.game = name;
                }
            }
            if event.avatar.trim().is_empty() {
                if let Some(avatar) = avatar_urls.get(&event.steam_id) {
                    event.avatar = avatar.clone();
                }
            }
        }
        let friend_icons: Vec<Option<egui::TextureHandle>> = friend_events
            .iter()
            .map(|event| {
                let game_event = matches!(
                    event.kind,
                    lapinux_core::social::SteamFriendEventKind::GameStarted
                        | lapinux_core::social::SteamFriendEventKind::GameStopped
                );
                if game_event && event.appid != 0 {
                    self.art.game_icon(ctx, &self.paths, event.appid)
                } else {
                    None
                }
            })
            .collect();
        let friend_avatars: Vec<Option<egui::TextureHandle>> = friend_events
            .iter()
            .map(|event| {
                self.friend_avatar(
                    ctx,
                    &format!("steam:{}", event.steam_id),
                    event.avatar.as_str(),
                )
            })
            .collect();
        // Un seul flux : les compteurs Steam, les notifications Web et
        // l'activité des amis sont triés ensemble par heure décroissante.
        // Chaque notification wishlist reste une carte distincte.
        enum FeedItem {
            Counter(usize),
            Web(usize),
            Friend(usize),
            Wishlist(usize),
        }
        enum DismissRequest {
            Counter(String),
            Web(String),
            Friend(String),
            All,
        }
        let mut dismiss_request: Option<DismissRequest> = None;
        let mut feed: Vec<(i64, FeedItem)> = Vec::new();
        for index in 0..notifications.len() {
            feed.push((notifications[index].at, FeedItem::Counter(index)));
        }
        for index in 0..other_web_notifications.len() {
            feed.push((other_web_notifications[index].at, FeedItem::Web(index)));
        }
        for index in 0..friend_events.len() {
            feed.push((friend_events[index].at, FeedItem::Friend(index)));
        }
        for index in 0..wishlist_notifications.len() {
            if !wishlist_entries[index].is_empty() {
                feed.push((wishlist_notifications[index].at, FeedItem::Wishlist(index)));
            }
        }
        feed.sort_by(|left, right| right.0.cmp(&left.0));
        // Un lien de promotion wishlist n'ouvre pas le navigateur externe :
        // la page part dans la fenêtre WebKit de Lapinux.
        let mut open_web: Option<(String, String)> = None;
        let mut wishlist_open = self.notifications_wishlist_open.clone();
        let (close, clear_all) = fixed_panel_with_scroll_action(
            ctx,
            "notifications-modal",
            lapinux_core::i18n::t("notifications_panel.title"),
            lapinux_core::i18n::t("notifications_panel.subtitle"),
            egui::vec2(760.0, 620.0),
            true,
            true,
            196,
            (!feed.is_empty()).then_some(lapinux_core::i18n::t("notifications_panel.clear_all")),
            |ui| {
                if feed.is_empty() {
                    let message = if live {
                        lapinux_core::i18n::t("notifications_panel.steam_empty")
                    } else if steam_connecting {
                        lapinux_core::i18n::t("notifications_panel.steam_connecting")
                    } else {
                        lapinux_core::i18n::t("notifications_panel.steam_unavailable")
                    };
                    muted(ui, message);
                } else {
                    if !live {
                        muted(
                            ui,
                            lapinux_core::i18n::t("notifications_panel.steam_cached"),
                        );
                        ui.add_space(6.0);
                    }
                    for (_, item) in &feed {
                        match item {
                            FeedItem::Counter(index) => {
                                let notification = &notifications[*index];
                                let key = notification_counter_key(notification);
                                if notification_card(
                                    ui,
                                    &key,
                                    notification.label(),
                                    Some(notification.count),
                                    steam_notification_detail(notification.kind),
                                    notification.at,
                                ) {
                                    dismiss_request = Some(DismissRequest::Counter(key));
                                }
                            }
                            FeedItem::Web(index) => {
                                let notification = &other_web_notifications[*index];
                                let key = notification_web_key(notification);
                                if web_notification_card(ui, &key, notification) {
                                    dismiss_request = Some(DismissRequest::Web(key));
                                }
                            }
                            FeedItem::Friend(index) => {
                                let key = friend_notification_key(&friend_events[*index]);
                                if friend_event_card(
                                    ui,
                                    &key,
                                    &friend_events[*index],
                                    friend_icons[*index].as_ref(),
                                    friend_avatars[*index].as_ref(),
                                ) {
                                    dismiss_request = Some(DismissRequest::Friend(key));
                                }
                            }
                            FeedItem::Wishlist(index) => {
                                if wishlist_card(
                                    ui,
                                    &wishlist_notifications[*index],
                                    &wishlist_entries[*index],
                                    &mut wishlist_open,
                                    &mut open_web,
                                ) {
                                    dismiss_request = Some(DismissRequest::Web(
                                        notification_web_key(&wishlist_notifications[*index]),
                                    ));
                                }
                            }
                        }
                    }
                }
            },
        );
        self.notifications_wishlist_open = wishlist_open;
        if clear_all {
            dismiss_request = Some(DismissRequest::All);
        }
        if let Some(request) = dismiss_request {
            match request {
                DismissRequest::Counter(key) | DismissRequest::Web(key) => {
                    self.remember_dismissed_notification(key);
                }
                DismissRequest::Friend(key) => self.dismiss_friend_event(&key),
                DismissRequest::All => self.dismiss_all_notifications(),
            }
        }
        if close {
            self.notifications_open = false;
        }
        if let Some((url, title)) = open_web {
            self.webview.open(&self.paths, &url, &title);
            self.webview.present();
        }
    }

    /// Retient une notification supprimée. Le compteur Steam continue
    /// d'arriver à chaque instantané : cette clé le garde hors du panneau et
    /// de la pastille jusqu'à ce qu'une nouvelle notification le remplace.
    fn remember_dismissed_notification(&mut self, key: String) {
        const MAX_DISMISSED: usize = 400;
        let dismissed = &mut self.config.notifications.dismissed_notifications;
        if !dismissed.iter().any(|item| item == &key) {
            dismissed.push(key);
        }
        let excess = dismissed.len().saturating_sub(MAX_DISMISSED);
        if excess > 0 {
            dismissed.drain(0..excess);
        }
        self.config_dirty_at = Some(Instant::now());
    }

    /// Supprime un événement d'ami de l'historique local. La clé est aussi
    /// retenue : un redémarrage ne doit pas le faire revenir avec le prochain
    /// instantané Steam.
    fn dismiss_friend_event(&mut self, key: &str) {
        self.remember_dismissed_notification(key.to_string());
        let raw_key = key.strip_prefix("friend:").unwrap_or(key);
        let before = self.friend_events_history.len();
        self.friend_events_history
            .retain(|event| friend_event_key(event) != raw_key);
        if self.friend_events_history.len() != before {
            friend_events_save(&self.paths, &self.friend_events_history);
            self.friend_events_unread = self.friend_events_unread.saturating_sub(1);
        }
    }

    /// Vide le panneau : l'historique local part, et tous les compteurs et
    /// notifications Web actuellement publiés par Steam sont masqués.
    fn dismiss_all_notifications(&mut self) {
        let mut keys = Vec::new();
        if let Some(social) = self.social.as_ref() {
            keys.extend(social.notifications.iter().map(notification_counter_key));
            keys.extend(social.web_notifications.iter().map(notification_web_key));
            keys.extend(social.friend_events.iter().map(friend_notification_key));
        }
        keys.extend(
            self.friend_events_history
                .iter()
                .map(friend_notification_key),
        );
        for key in keys {
            self.remember_dismissed_notification(key);
        }
        self.friend_events_history.clear();
        friend_events_save(&self.paths, &self.friend_events_history);
        self.friend_events_unread = 0;
        self.steam_notifications_seen = 0;
        self.notifications_wishlist_open.clear();
    }

    /// Total Steam filtré porté par la cloche. Il sert aussi de référence
    /// après l'ouverture du panneau : les notifications déjà chargées sont
    /// alors considérées comme vues. Les cartes supprimées ne comptent plus.
    fn steam_notification_badge(&self) -> u32 {
        if !self.config.notifications.enabled {
            return 0;
        }
        let prefs = &self.config.notifications;
        let since = notification_cutoff(prefs, notification_now());
        self.social.as_ref().map_or(0, |social| {
            let dismissed = &prefs.dismissed_notifications;
            let counters: u32 = if prefs.steam_other {
                social
                    .notifications
                    .iter()
                    .filter(|notification| {
                        notification_is_recent(notification.at, since)
                            && !dismissed.contains(&notification_counter_key(notification))
                    })
                    .map(|notification| notification.count)
                    .sum()
            } else {
                0
            };
            let web = social
                .web_notifications
                .iter()
                .filter(|notification| prefs.web_notification_enabled(notification.kind))
                .filter(|notification| notification_is_recent(notification.at, since))
                .filter(|notification| !dismissed.contains(&notification_web_key(notification)))
                .count() as u32;
            counters + web
        })
    }

    pub(super) fn topbar(&mut self, ctx: &egui::Context) {
        if self.friend_activity.as_ref().is_some_and(|activity| {
            activity.started.elapsed().as_secs_f32() >= FRIEND_ACTIVITY_FLASH_SECS
        }) {
            self.friend_activity = None;
        }
        let friend_activity = self.friend_activity.clone();
        let friend_activity_avatar = friend_activity.as_ref().and_then(|activity| {
            self.friend_avatar(
                ctx,
                &format!("steam:{}", activity.steam_id),
                &activity.avatar,
            )
        });
        let friend_activity_game_icon = friend_activity.as_ref().and_then(|activity| {
            if activity.appid == 0 {
                return None;
            }
            self.art.game_icon(ctx, &self.paths, activity.appid)
        });
        if friend_activity.is_some() {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
        // Un modal assombrit déjà l'écran : la bulle d'activité ne doit pas
        // rester la seule surface lumineuse au-dessus de son voile.
        let notification_dimmed = self.settings_open
            || self.notifications_open
            || self.friends_open
            || self.activity_log.is_some()
            || self.game_settings.is_some()
            || self.dlc_modal.is_some()
            || self.uninstall_modal.is_some();
        let friend_flash = friend_activity.as_ref().map(|activity| TopbarFriendFlash {
            texture: friend_activity_avatar.as_ref(),
            game_icon: friend_activity_game_icon.as_ref(),
            name: &activity.name,
            caption: &activity.caption,
            game_event: activity.appid != 0 || !activity.game.trim().is_empty(),
            progress: (activity.started.elapsed().as_secs_f32() / FRIEND_ACTIVITY_FLASH_SECS)
                .clamp(0.0, 1.0),
            opacity: if notification_dimmed { 0.45 } else { 1.0 },
        });
        let panel = egui::TopBottomPanel::top("topbar")
            .frame(
                egui::Frame::none()
                    .fill(PANEL())
                    .inner_margin(egui::Margin::symmetric(18.0, 10.0)),
            )
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.x = 7.0;
                let nav_badge = self.downloads.nav_badge(ctx, &self.paths, &self.jobs);
                ui.horizontal(|ui| {
                    let bar_left = ui.cursor().left();
                    let bar_width = ui.available_width();
                    let pages = visible_topbar_pages(
                        &self.config.general.topbar_order,
                        &self.config.general.topbar_hidden,
                    );
                    if let Some(page) = main_tabs(ui, self.page, nav_badge.as_ref(), &pages) {
                        // Cliquer sur Manettes rouvre toujours la liste des
                        // appareils ; Général reste accessible par son onglet.
                        if page == Page::Controllers {
                            self.controllers_tab = ControllersTab::Devices;
                        }
                        if page != self.page {
                            // La fiche d'un jeu ne suit pas d'une page à
                            // l'autre : seule la page qui l'a ouverte la peint.
                            self.collection_detail_open = false;
                        }
                        self.page = page;
                        if self.settings_open {
                            self.close_settings();
                        }
                    }
                    // Le jeu en cours vit à droite des onglets, séparé d'un
                    // filet : une pastille ronde avec sa jaquette, son nom et
                    // son bouton d'arrêt.
                    self.topbar_game_chip(ui);
                    // L'heure se cale sur le milieu de la barre entière, pas
                    // sur l'espace libre laissé à droite des onglets.
                    let unix = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|elapsed| elapsed.as_secs() as i64)
                        .unwrap_or(0);
                    let clock_left = bar_left + (bar_width - TOPBAR_CLOCK_WIDTH) / 2.0;
                    let gap = clock_left - ui.cursor().left();
                    if gap > 0.0 {
                        ui.add_space(gap);
                    }
                    topbar_clock(ui, unix, self.system.uptime_secs);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let friend_unread = if self.config.notifications.enabled {
                            self.friend_events_unread
                        } else {
                            0
                        };
                        let steam_badge = self.steam_notification_badge();
                        // La pastille ne suit pas le total Steam : elle ne
                        // s'allume que lorsqu'une notification nouvelle le fait
                        // monter. Une baisse de total (lue ailleurs) abaisse la
                        // référence sans allumer la cloche.
                        if steam_badge < self.steam_notifications_seen {
                            self.steam_notifications_seen = steam_badge;
                        }
                        let notification_unread =
                            friend_unread > 0 || steam_badge > self.steam_notifications_seen;
                        let (
                            settings_clicked,
                            screen_clicked,
                            friends_clicked,
                            notifications_clicked,
                        ) = topbar_controls(
                            ui,
                            self.settings_open,
                            self.friends_open,
                            self.notifications_open,
                            self.big_picture,
                            notification_unread,
                            friend_flash,
                        );
                        if notifications_clicked {
                            self.notifications_open = !self.notifications_open;
                            if self.notifications_open {
                                self.social_poll = Instant::now();
                                self.poll_social(false);
                                // Le panneau est ouvert : tout ce que la lecture
                                // vient de rapporter est déjà visible. Seule une
                                // notification ultérieure rallumera la pastille.
                                self.friend_events_unread = 0;
                                self.steam_notifications_seen = self.steam_notification_badge();
                            }
                        }
                        if settings_clicked {
                            if self.settings_open {
                                self.close_settings();
                            } else {
                                self.open_settings(SettingsTab::General);
                            }
                        }
                        if friends_clicked {
                            self.toggle_friends();
                        }
                        if screen_clicked {
                            self.big_picture = !self.big_picture;
                            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(
                                self.big_picture,
                            ));
                        }
                        performance_panel(ui, &self.system);
                        ui.add_space(6.0);
                        let pads = self
                            .pad_monitor
                            .as_ref()
                            .map(|monitor| monitor.snapshot())
                            .unwrap_or_default();
                        topbar_controller_indicators(
                            ui,
                            &self.devices,
                            &self.config.input.pad_names,
                            &pads,
                        );
                    });
                });
            });
        // Filet discret sous toute la barre, pour la séparer du contenu.
        // Même calque que la barre : un panneau superposé le recouvre.
        if !self.settings_open {
            let rect = panel.response.rect;
            ctx.layer_painter(egui::LayerId::background()).line_segment(
                [
                    egui::pos2(rect.left(), rect.bottom() - 0.5),
                    egui::pos2(rect.right(), rect.bottom() - 0.5),
                ],
                egui::Stroke::new(1.0_f32, BORDER()),
            );
        }
    }

    /// Pastille du jeu en cours dans la barre supérieure : sa jaquette, son
    /// nom et un bouton rond pour l'arrêter. Elle prend la même hauteur que
    /// les onglets et se place juste après eux, séparée d'un filet.
    fn topbar_game_chip(&mut self, ui: &mut egui::Ui) {
        let Some(session) = self.session.clone() else {
            return;
        };
        ui.add_space(12.0);
        let (rule, _) = ui.allocate_exact_size(egui::vec2(1.0, 20.0), egui::Sense::hover());
        ui.painter().rect_filled(rule, 0.0, BORDER());
        ui.add_space(12.0);

        let (store, remote) = self.activity_cover(session.appid);
        let texture = self.covers.get(
            ui.ctx(),
            &self.paths,
            session.appid,
            &session.name,
            store,
            &remote,
            CoverTier::Sheet,
        );
        let galley = ui.painter().layout_no_wrap(
            lapinux_core::util::ellipsize(&session.name, 22),
            egui::FontId::proportional(12.5),
            TEXT(),
        );
        let name_width = galley.size().x;
        // La cadence du moment à côté du nom : verte tant que la session tient
        // sa limite, rouge quand elle décroche, comme l'étiquette de l'overlay.
        let fps = session
            .fps
            .as_ref()
            .filter(|stats| stats.current > 0)
            .map(|stats| {
                let color =
                    if lapinux_core::util::fps_is_comfortable(stats.current, session.fps_cap) {
                        OK
                    } else {
                        DANGER
                    };
                (
                    ui.painter().layout_no_wrap(
                        format!("{} FPS", stats.current),
                        egui::FontId::proportional(11.0),
                        color,
                    ),
                    color,
                )
            });
        const HEIGHT: f32 = 34.0;
        const ICON: f32 = 24.0;
        const PAD: f32 = 7.0;
        let fps_width = fps
            .as_ref()
            .map(|(galley, _)| 8.0 + galley.size().x)
            .unwrap_or(0.0);
        let width = PAD + ICON + 8.0 + name_width + fps_width + 10.0 + 24.0 + PAD;
        // Toute la pastille ouvre la fiche du jeu ; le bouton d'arrêt, posé
        // par-dessus, garde son propre clic.
        let (rect, pill) = ui.allocate_exact_size(egui::vec2(width, HEIGHT), egui::Sense::click());
        let painter = ui.painter().clone();
        painter.rect(
            rect,
            egui::Rounding::same(HEIGHT / 2.0),
            if pill.hovered() { CARD_HOVER() } else { CARD() },
            egui::Stroke::new(1.0_f32, BORDER()),
        );
        let icon_rect = egui::Rect::from_center_size(
            egui::pos2(rect.left() + PAD + ICON / 2.0, rect.center().y),
            egui::vec2(ICON, ICON),
        );
        painter.rect_filled(icon_rect, egui::Rounding::same(6.0), PANEL());
        match &texture {
            Some(texture) => {
                let size = texture.size_vec2();
                if size.x > 0.0 && size.y > 0.0 {
                    let scale = (ICON / size.x).max(ICON / size.y);
                    let shown = egui::Rect::from_center_size(icon_rect.center(), size * scale);
                    painter.with_clip_rect(icon_rect).image(
                        texture.id(),
                        shown,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                }
            }
            None => {
                let initial = session
                    .name
                    .chars()
                    .next()
                    .map(|ch| ch.to_uppercase().to_string())
                    .unwrap_or_default();
                painter.text(
                    icon_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    initial,
                    egui::FontId::proportional(12.0),
                    TEXT_DIM(),
                );
            }
        }
        painter.galley(
            egui::pos2(
                icon_rect.right() + 8.0,
                rect.center().y - galley.size().y / 2.0,
            ),
            galley,
            TEXT(),
        );
        if let Some((fps_galley, color)) = fps {
            painter.galley(
                egui::pos2(
                    icon_rect.right() + 8.0 + name_width + 8.0,
                    rect.center().y - fps_galley.size().y / 2.0,
                ),
                fps_galley,
                color,
            );
        }

        let stop_rect = egui::Rect::from_center_size(
            egui::pos2(rect.right() - PAD - 12.0, rect.center().y),
            egui::vec2(24.0, 24.0),
        );
        let response = ui.interact(
            stop_rect,
            ui.id().with("topbar-game-stop"),
            egui::Sense::click(),
        );
        let hovered = response.hovered();
        if hovered {
            painter.circle_filled(stop_rect.center(), 12.0, CARD_HOVER());
        }
        painter.text(
            stop_rect.center(),
            egui::Align2::CENTER_CENTER,
            ICON_POWER.to_string(),
            icon_font(13.0),
            if hovered { DANGER } else { TEXT_DIM() },
        );
        let clicked = response.clicked();
        let over_stop = response.hovered();
        let response = response.on_hover_text(lapinux_core::i18n::t_fmt(
            "shell.stop",
            &[session.name.to_string()],
        ));
        response.on_hover_cursor(egui::CursorIcon::PointingHand);
        if clicked {
            let _ = self.client.request(&Req::Stop);
            self.notify(lapinux_core::i18n::t("shell.game_stopped"));
            return;
        }
        if pill.clicked() && !over_stop {
            self.selected = Some(session.appid);
        }
        if pill.hovered() && !over_stop {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            pill.on_hover_text(lapinux_core::i18n::t_fmt(
                "shell.view_page_of",
                &[session.name.clone()],
            ));
        }
    }
}
