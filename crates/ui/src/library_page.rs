//! Bibliothèque et suivi des jeux.

use super::*;

/// Fenêtre de rattrapage après l'ouverture ou l'arrêt du défilement : les
/// jaquettes réseau arrivent après plusieurs centaines de millisecondes, et une
/// rafale d'images immédiates s'épuisait avant leur arrivée. On étale donc les
/// passes dans le temps, sans laisser la fenêtre redemander des rendus sans fin.
const LIBRARY_FILL_WINDOW: std::time::Duration = std::time::Duration::from_secs(6);
/// Intervalle entre deux passes de rattrapage : le budget de décodage garde son
/// rythme de six jaquettes par image, mais laisse aussi les requêtes réseau
/// aboutir entre deux passes.
const LIBRARY_FILL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

impl LapinuxApp {
    /// Bibliothèque unifiée : jeux installés/scannés et catalogue Steam du
    /// compte, sans onglet séparé.
    pub(super) fn library_page(&mut self, ui: &mut egui::Ui) {
        // Pendant un défilement, aucun décodage : la grille ne demande pas
        // toutes les jaquettes traversées d'un coup. Le budget revient dès que
        // le défilement s'arrête et les tuiles visibles se remplissent.
        let scrolling = ui.input(|input| {
            input.raw_scroll_delta.y.abs() > 0.1 || input.smooth_scroll_delta.y.abs() > 0.1
        });
        if scrolling {
            self.library_fill_until = None;
        } else if self.library_was_scrolling {
            self.library_fill_until = Some(Instant::now() + LIBRARY_FILL_WINDOW);
        }
        self.library_was_scrolling = scrolling;
        self.covers.begin_frame(if scrolling { 0 } else { 6 });
        let page_started = std::time::Instant::now();
        if trace_enabled() {
            eprintln!("library page: enter");
        }
        // Les jeux des boutiques viennent du daemon comme le catalogue Steam :
        // la vue est demandée ici sans passer par Réglages › Comptes. Le cache
        // d'abord (instantané), puis sa relecture réseau — qui attend la
        // première réponse pour ne pas occuper l'échange avec le daemon.
        self.stores.ensure(&self.client);
        self.stores.ensure_games(&self.client);
        // La réponse arrive par la même boucle que les autres requêtes : sans
        // cette lecture, les jeux des boutiques n'apparaîtraient qu'après un
        // passage par Réglages › Comptes.
        if let Some(message) = self.stores.poll(ui.ctx()) {
            self.notify(message);
            // Une réponse de boutique peut ajouter ou retirer des jeux.
            self.touch_library();
        }
        // Les jaquettes de boutique partent d'avance : une tuile ne doit pas
        // rester en lettre le temps de son téléchargement au défilement.
        if self.covers.needs_prefetch(
            self.stores
                .view
                .libraries
                .iter()
                .map(|library| library.entries.len())
                .sum(),
        ) && !self.stores.view.libraries.is_empty()
        {
            // Les assets du marketplace que la grille cache n'ont pas de
            // jaquette à précharger : les demander saturerait la file pour des
            // tuiles que personne ne verra.
            let games: Vec<_> = self
                .stores
                .owned_games()
                .into_iter()
                .filter(|game| !game.tool || !self.config.library.store_tools_hidden(game.store))
                .collect();
            self.covers.prefetch_store(&self.paths, &games);
        }
        self.covers.pump_prefetch(ui.ctx(), &self.paths);
        // Les jaquettes sorties de l'écran quittent la mémoire : sans ce
        // passage, un défilement de bibliothèque les gardait toutes.
        self.covers.trim();
        // Les vignettes sont décodées à la taille où la grille les peint : sur
        // un affichage à l'échelle 1, une carte de 168 points n'a pas besoin
        // d'une jaquette de 600 pixels de large.
        self.covers
            .set_tile(self.grid_width, ui.ctx().pixels_per_point());
        let status = self.steam_status.clone();
        let connected = status.as_ref().map(|s| s.connected).unwrap_or(false);

        let list_started = std::time::Instant::now();
        let base = self.library_entries_all();
        if list_started.elapsed() > Duration::from_millis(50) {
            eprintln!("game list: {} ms", list_started.elapsed().as_millis());
        }
        // Ctrl+F ouvre la recherche, comme dans un navigateur : le geste est
        // attendu, et il ne coûte rien quand la barre est déjà ouverte.
        if ui.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::F)) {
            self.search_open = true;
            self.focus_search = true;
        }
        let needle = self.search.trim().to_string();
        let view = self.library_view(&base, needle);
        // Les compteurs de la pastille portent sur la vue avant le filtre
        // d'installation : sinon « Tous » afficherait le nombre de lignes
        // filtrées, et les trois pastilles se contrediraient.
        let total = view.order.len();
        let installed_count = view
            .order
            .iter()
            .filter(|index| base.get(**index).is_some_and(|entry| entry.installed))
            .count();
        let missing_count = total - installed_count;
        // Le filtre d'installation réduit la grille, après les compteurs.
        let visible: Vec<&LibraryEntry> = view
            .order
            .iter()
            .filter_map(|index| base.get(*index))
            .filter(|entry| self.catalog_filter.keeps(entry.installed))
            .collect();
        let disk_count = view.disk;
        let steam_count = view.steam;
        let family_count = view.family;
        let all_steam_count = view.all_steam;
        let epic_count = view.epic;
        let epic_pc_count = view.epic_pc;
        let epic_mobile_count = view.epic_mobile;
        let gog_count = view.gog;
        let itch_count = view.itch;
        if list_started.elapsed() > Duration::from_millis(50) {
            eprintln!(
                "filter and sort: {} ms total",
                list_started.elapsed().as_millis()
            );
        }
        // Sur une petite fenêtre, les trois zones ne tiennent plus sur une
        // ligne : les filtres passent sous la recherche.
        let width = ui.available_width();
        let narrow = width < 760.0;
        let mut shuffle = false;
        let mut close_search = false;
        let mut open_sync = false;
        let mut open_library_settings = false;
        // La synchronisation en cours se lit avant la rangée : la flèche
        // tourne sur place et son infobulle dit où elle en est.
        let syncing = self.library_sync;
        // Ce que fait la flèche, où elle en est, et ce qu'elle a trouvé la
        // dernière fois : l'infobulle porte le détail que le bouton ne peut
        // pas dire en 32 pixels.
        let catalog_info = self.catalog.as_ref().map(|catalog| {
            let age = lapinux_core::util::now_unix().saturating_sub(catalog.updated);
            let when = if catalog.updated <= 0 {
                lapinux_core::i18n::t("library.sync.never").to_string()
            } else if age < 90 {
                lapinux_core::i18n::t("library.sync.just_now").to_string()
            } else if age < 3600 {
                lapinux_core::i18n::t_args(
                    "library.sync.minutes",
                    &[("count", &(age / 60).to_string())],
                )
            } else if age < 86_400 {
                lapinux_core::i18n::t_args(
                    "library.sync.hours",
                    &[("count", &(age / 3600).to_string())],
                )
            } else {
                lapinux_core::i18n::t_args(
                    "library.sync.days",
                    &[("count", &(age / 86_400).to_string())],
                )
            };
            let error = catalog
                .error
                .as_ref()
                .map(|error| {
                    format!(
                        "\n{}",
                        lapinux_core::i18n::t_args("library.sync.error", &[("error", error)]),
                    )
                })
                .unwrap_or_default();
            lapinux_core::i18n::t_args(
                "library.sync.games",
                &[
                    ("when", &when),
                    ("count", &catalog.entries.len().to_string()),
                    ("error", &error),
                ],
            )
        });
        let refresh_hint = match syncing {
            Some(started) => {
                // Les étapes encore en vol, dans l'ordre où elles s'exécutent.
                let mut steps: Vec<&str> = Vec::new();
                if self.refresh_rx.is_some() || self.rescan_after_refresh {
                    steps.push(lapinux_core::i18n::t("library.sync.step_disk"));
                }
                if !self.stores.ready() {
                    steps.push(lapinux_core::i18n::t("library.sync.step_stores"));
                }
                if self
                    .catalog
                    .as_ref()
                    .map(|catalog| catalog.syncing)
                    .unwrap_or(false)
                {
                    steps.push(lapinux_core::i18n::t("library.sync.step_steam"));
                }
                let steps = if steps.is_empty() {
                    lapinux_core::i18n::t("library.sync.step_waiting").to_string()
                } else {
                    steps.join(", ")
                };
                let seconds = started.elapsed().as_secs().to_string();
                lapinux_core::i18n::t_args(
                    "library.sync.running",
                    &[
                        ("seconds", &seconds),
                        ("steps", &steps),
                        (
                            "catalog",
                            catalog_info
                                .as_deref()
                                .unwrap_or(lapinux_core::i18n::t("library.sync.reading")),
                        ),
                    ],
                )
            }
            None => lapinux_core::i18n::t_args(
                "library.sync.refresh",
                &[
                    ("total", &total.to_string()),
                    ("installed", &installed_count.to_string()),
                    ("missing", &missing_count.to_string()),
                    (
                        "catalog",
                        catalog_info
                            .as_deref()
                            .unwrap_or(lapinux_core::i18n::t("library.sync.never")),
                    ),
                ],
            ),
        };
        ui.horizontal(|ui| {
            // Un peu d'air entre les groupes — sources, recherche, filtre —
            // pour que chaque sélection se lise comme son propre bouton.
            ui.spacing_mut().item_spacing.x = 8.0;
            match source_switch(
                ui,
                self.sources,
                disk_count,
                steam_count,
                family_count,
                all_steam_count,
                epic_count,
                epic_pc_count,
                epic_mobile_count,
                gog_count,
                itch_count,
            ) {
                Some(next) => self.sources = next,
                None => self.notify(lapinux_core::i18n::t("library.keep_source")),
            }

            // Le champ de recherche remplace la loupe sur la même ligne : le
            // bloc loupe + dé reste centré, le dé glisse donc vers la droite.
            let spacing = ui.spacing().item_spacing.x;
            // Place réservée au filtre d'installation et au groupe d'icônes
            // sur la ligne.
            let reserve = if narrow { 0.0 } else { 300.0 };
            let free_end = ui.max_rect().right() - reserve;
            let start_x = ui.cursor().min.x;
            let field_width = (free_end - start_x - FILTER_H * 2.0 - 24.0).clamp(160.0, 360.0);
            // La loupe et le dé forment une seule pastille : deux segments
            // accolés, sans espacement entre eux.
            let cluster = if self.search_open {
                // Champ et dé collés : une seule pastille.
                field_width + FILTER_H
            } else {
                FILTER_H * 2.0
            };
            let center = ui.max_rect().center().x;
            // Le filtre est collé à la loupe : une seule pastille de deux
            // segments, sans écart entre eux. C'est le groupe entier — filtre
            // compris — qui se centre, sinon l'ensemble dérive vers la gauche
            // dès que la barre de recherche s'ouvre.
            let group = cluster + FILTER_H;
            let mut cluster_left = center - group / 2.0;
            if cluster_left + group > free_end - 8.0 {
                cluster_left = free_end - 8.0 - group;
            }
            let gap = (cluster_left - start_x - spacing).max(8.0);
            ui.add_space(gap);
            ui.spacing_mut().item_spacing.x = 0.0;
            let options = match (self.sort_menu, self.credit_menu) {
                (true, Some(true)) => self.credit_options(true),
                (true, Some(false)) => self.credit_options(false),
                _ => Vec::new(),
            };
            library_filter_panel(
                ui,
                &mut self.library_sort,
                &mut self.library_descending,
                &mut self.only_updates,
                &mut self.never_played,
                &mut self.sort_menu,
                &mut self.publisher_filter,
                &mut self.developer_filter,
                &mut self.credit_menu,
                &mut self.credit_query,
                &options,
            );
            if self.search_open {
                // Le champ et le dé restent une seule pastille : l'écart est
                // retiré avant de poser le champ, puisque c'est à ce moment
                // qu'egui réserve la place qui suit l'élément.
                ui.spacing_mut().item_spacing.x = 0.0;
                close_search = search_field(
                    ui,
                    &mut self.search,
                    field_width,
                    &mut self.focus_search,
                    true,
                );
            }
            let (open_search, pick_random) = search_random_pill(ui, self.search_open, true);
            ui.spacing_mut().item_spacing.x = spacing;
            if open_search {
                self.search_open = true;
                self.focus_search = true;
            }
            if pick_random {
                shuffle = true;
            }

            if !narrow {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // À droite du filtre : réglages puis relecture, dans la
                    // même pastille segmentée que les sélections voisines.
                    let icons = library_icon_group(
                        ui,
                        syncing,
                        &refresh_hint,
                        lapinux_core::i18n::t("library.settings_tooltip"),
                    );
                    if icons[0].clicked() {
                        open_sync = true;
                    }
                    if icons[1].clicked() {
                        open_library_settings = true;
                    }
                    if let Some(filter) = install_filter_group(
                        ui,
                        self.catalog_filter,
                        total,
                        installed_count,
                        missing_count,
                    ) {
                        self.catalog_filter = filter;
                    }
                });
            }
        });
        if narrow {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let icons = library_icon_group(
                        ui,
                        syncing,
                        &refresh_hint,
                        lapinux_core::i18n::t("library.settings_tooltip"),
                    );
                    if icons[0].clicked() {
                        open_sync = true;
                    }
                    if icons[1].clicked() {
                        open_library_settings = true;
                    }
                    if let Some(filter) = install_filter_group(
                        ui,
                        self.catalog_filter,
                        total,
                        installed_count,
                        missing_count,
                    ) {
                        self.catalog_filter = filter;
                    }
                });
            });
        }
        if open_library_settings {
            self.open_settings(SettingsTab::Library);
        }
        if open_sync {
            // Le bouton de relecture n'entraîne plus tout : il ouvre le choix
            // des sources, chacune avec ce qu'elle contient.
            self.sync_dialog = Some(SyncDialog::new(disk_count));
        }

        if self.search_open
            && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            close_search = true;
        }
        if close_search {
            self.search.clear();
            self.search_open = false;
        }
        ui.add_space(12.0);

        if let Some(error) = self
            .catalog
            .as_ref()
            .and_then(|catalog| catalog.error.clone())
        {
            ui.label(
                egui::RichText::new(sentence(&error))
                    .size(11.0)
                    .color(egui::Color32::from_rgb(230, 140, 140)),
            );
        }
        if !connected {
            let cached = self
                .catalog
                .as_ref()
                .map(|catalog| !catalog.entries.is_empty())
                .unwrap_or(false);
            egui::Frame::none()
                .fill(CARD())
                .rounding(egui::Rounding::same(10.0))
                .inner_margin(12.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(if cached {
                                lapinux_core::i18n::t("library.disconnected")
                            } else {
                                lapinux_core::i18n::t("library.connect_prompt")
                            })
                            .size(11.5)
                            .color(TEXT_DIM()),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .button(lapinux_core::i18n::t("library.sign_in"))
                                .clicked()
                            {
                                self.open_steam_login();
                            }
                        });
                    });
                });
            ui.add_space(8.0);
        }

        // Le cache du compte suffit à afficher les jeux possédés, même quand
        // la session Steam n'est pas active : la bibliothèque reste complète.
        let known_account = !self.steam_user.trim().is_empty()
            || self
                .steam_status
                .as_ref()
                .map(|status| status.token_saved)
                .unwrap_or(false);
        if known_account && !self.catalog_loaded {
            self.load_catalog(false);
        }

        // La grille suit le tri choisi : la liste déjà filtrée et ordonnée
        // plus haut est celle qui se dessine. Auparavant la page redemandait
        // une liste brute, si bien que le menu des filtres ne changeait rien.
        let entries: Vec<&LibraryEntry> = visible;
        // Entrée dans le champ de recherche ouvre le premier résultat : le
        // clavier suffit pour chercher, choisir et lire la fiche.
        if self.search_open
            && !self.search.trim().is_empty()
            && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Enter))
        {
            if let Some(first) = entries.first() {
                self.selected = Some(first.appid);
            }
        }
        if shuffle {
            if entries.is_empty() {
                self.notify(lapinux_core::i18n::t("library.no_shuffle"));
            } else {
                let seed = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|elapsed| elapsed.subsec_nanos() as usize)
                    .unwrap_or(0);
                let mut index = seed % entries.len();
                if entries.len() > 1 && self.selected == Some(entries[index].appid) {
                    index = (index + 1) % entries.len();
                }
                self.selected = Some(entries[index].appid);
            }
        }
        if entries.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(80.0);
                let (title, hint) = if !connected {
                    (
                        lapinux_core::i18n::t("library.empty_title"),
                        lapinux_core::i18n::t("library.empty_hint"),
                    )
                } else if self
                    .catalog
                    .as_ref()
                    .map(|catalog| catalog.entries.is_empty())
                    .unwrap_or(true)
                {
                    (
                        lapinux_core::i18n::t("library.empty_catalog_title"),
                        lapinux_core::i18n::t("library.empty_catalog_hint"),
                    )
                } else {
                    (
                        lapinux_core::i18n::t("library.empty_filter_title"),
                        lapinux_core::i18n::t("library.empty_filter_hint"),
                    )
                };
                ui.label(egui::RichText::new(title).size(16.0).color(TEXT_DIM()));
                ui.label(egui::RichText::new(hint).size(12.0).color(TEXT_DIM()));
            });
            return;
        }

        let ctx = ui.ctx().clone();
        let gap = self.config.library.grid_spacing as f32;
        let card_width = self.grid_width;
        let cover_radius = self.config.library.cover_radius as f32;
        let hover_width = self.config.library.hover_width as f32;
        let columns = grid_columns(ui.available_width(), card_width, gap);
        let mut clicked: Option<u32> = None;
        let mut hovered_card: Option<u32> = None;
        let mut missing_visible = false;
        let mut picker_request: Option<(u32, String)> = None;
        let mut reset_cover: Option<u32> = None;
        let mut open_folder: Option<PathBuf> = None;
        let mut play_request: Option<u32> = None;
        let mut install_request: Option<u32> = None;
        let mut hide_request: Option<u32> = None;
        let mut unhide_request: Option<u32> = None;
        // Les jeux arrivés depuis le dernier lancement portent une pastille :
        // le Profil les listait déjà, la grille les montre aussi.
        let new_ids: HashSet<u32> = self.new_games.iter().map(|entry| entry.appid).collect();
        // Seules les rangées visibles sont construites : avec plus de mille
        // jeux, tout mettre en page d'un coup décodait autant de jaquettes et
        // figeait la fenêtre juste après l'écran de démarrage.
        if trace_enabled() {
            eprintln!(
                "library page: before grid at {} ms",
                page_started.elapsed().as_millis()
            );
        }
        let rows = entries.len().div_ceil(columns);
        let row_height = card_width * 1.5 + gap;
        // Les premières rangées partent avant la peinture de la grille. Au
        // démarrage, ce travail se fait derrière l'écran d'attente : les
        // jaquettes visibles ont déjà eu le temps d'arriver quand il se retire.
        if !self.library_covers_warmed && !scrolling && !entries.is_empty() {
            self.warm_library_covers(&ctx, &entries, columns);
            self.library_covers_warmed = true;
            self.library_fill_until = Some(Instant::now() + LIBRARY_FILL_WINDOW);
        }
        // Flèches et Entrée : la grille se parcourt sans souris. Le jeu choisi
        // défile au centre, même quand sa rangée n'est pas encore construite.
        let mut launch_from_keys: Option<u32> = None;
        if !self.search_open {
            let current = self
                .library_focus
                .and_then(|appid| entries.iter().position(|entry| entry.appid == appid));
            let mut next = current;
            let input = ui.input(|input| {
                (
                    input.key_pressed(egui::Key::ArrowRight),
                    input.key_pressed(egui::Key::ArrowLeft),
                    input.key_pressed(egui::Key::ArrowDown),
                    input.key_pressed(egui::Key::ArrowUp),
                )
            });
            if input.0 {
                next = Some(current.map_or(0, |index| (index + 1).min(entries.len() - 1)));
            }
            if input.1 {
                next = Some(current.map_or(0, |index| index.saturating_sub(1)));
            }
            if input.2 {
                next = Some(current.map_or(0, |index| (index + columns).min(entries.len() - 1)));
            }
            if input.3 {
                next = Some(current.map_or(0, |index| index.saturating_sub(columns)));
            }
            if next != current {
                if let Some(entry) = next.and_then(|index| entries.get(index)) {
                    self.library_focus = Some(entry.appid);
                    self.library_scroll_to = Some(entry.appid);
                }
            }
            if let Some(appid) = self
                .library_focus
                .filter(|_| ui.input(|input| input.key_pressed(egui::Key::Enter)))
            {
                launch_from_keys = Some(appid);
            }
        }
        // Le clavier peut sortir de la fenêtre : la grille se recale sur la
        // rangée du jeu choisi.
        let mut scroll_offset: Option<f32> = None;
        if let Some(appid) = self.library_scroll_to.take() {
            if let Some(index) = entries.iter().position(|entry| entry.appid == appid) {
                let row = index / columns.max(1);
                let viewport = ui.available_height().max(row_height);
                scroll_offset =
                    Some((row as f32 * row_height - (viewport - row_height) * 0.5).max(0.0));
            }
        }
        let mut grid = egui::ScrollArea::vertical()
            .id_salt("library-grid")
            .auto_shrink([false, false]);
        if let Some(offset) = scroll_offset {
            grid = grid.vertical_scroll_offset(offset);
        }
        grid.show_rows(ui, row_height, rows, |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for row in entries.chunks(columns).skip(range.start).take(range.len()) {
                let pad = row_padding(ui.available_width(), row.len(), card_width, gap);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    ui.add_space(pad);
                    for entry in row {
                        let texture = self.covers.get(
                            &ctx,
                            &self.paths,
                            entry.appid,
                            &entry.name,
                            entry.store,
                            &entry.cover,
                            CoverTier::Tile,
                        );
                        if texture.is_none() {
                            missing_visible = true;
                        }
                        // Prépare les visuels des tuiles visibles au repos :
                        // au premier survol, l'icône est déjà chargée et les
                        // captures ont eu le temps d'arriver. Pendant un
                        // défilement, on saute ces lectures : la grille reste
                        // prioritaire et le survol les demandera au besoin.
                        if !scrolling {
                            self.hover_shots
                                .ensure(&ctx, &self.paths, entry.appid, entry.store);
                            let mut prefetched_urls = self.hover_shots.get(entry.appid).to_vec();
                            if prefetched_urls.is_empty() {
                                // Epic, GOG et itch : la fiche lue par la
                                // boutique garde ses captures en mémoire, sans
                                // repasser par le disque.
                                prefetched_urls = self.store_info.screenshots(entry.appid);
                            }
                            if !prefetched_urls.is_empty() {
                                self.gameinfo.news_art.ensure(&ctx, &prefetched_urls);
                            }
                            if self.hover_prefetched.insert(entry.appid) {
                                let details = self
                                    .catalog
                                    .as_ref()
                                    .and_then(|catalog| {
                                        catalog
                                            .entries
                                            .iter()
                                            .find(|game| game.appid == entry.appid)
                                    })
                                    .map(|game| game.details.clone())
                                    .unwrap_or_default();
                                let _ = self.art.get(
                                    &ctx,
                                    &self.paths,
                                    entry.appid,
                                    &details,
                                    SteamArt::Icon,
                                    &entry.name,
                                );
                            }
                        }
                        let has_custom = custom_cover_path(&self.paths, entry.appid).is_file();
                        let focused = self.library_focus == Some(entry.appid);
                        let response = game_card(
                            ui,
                            texture.as_ref(),
                            &entry.name,
                            focused || self.selected == Some(entry.appid),
                            card_width,
                            cover_radius,
                            entry.steam,
                            entry.family,
                            entry.family_available,
                            entry.store,
                            self.config.library.show_badges,
                            self.config.library.badge_scale,
                            entry.update.is_some(),
                            new_ids.contains(&entry.appid),
                        );
                        if response.clicked() {
                            clicked = Some(entry.appid);
                            self.library_focus = Some(entry.appid);
                        }
                        // Un double-clic lance le jeu installé ; sinon il
                        // ouvre sa fiche, là où l'installation se décide.
                        if response.double_clicked() {
                            play_request = Some(entry.appid);
                        }
                        let install_dir = self
                            .games
                            .iter()
                            .find(|game| game.appid == entry.appid && game.installed)
                            .map(Game::install_dir);
                        // L'aperçu garde la capsule large quelques
                        // secondes, puis bascule sur les captures du jeu.
                        // Les adresses viennent de la fiche en cache ;
                        // Steam n'est interrogé qu'en dernier recours.
                        if response.hovered() {
                            hovered_card = Some(entry.appid);
                            let now = Instant::now();
                            let started = match self.hover_since {
                                Some((id, at)) if id == entry.appid => at,
                                _ => {
                                    self.hover_since = Some((entry.appid, now));
                                    now
                                }
                            };
                            self.hover_shots
                                .ensure(&ctx, &self.paths, entry.appid, entry.store);
                            let mut urls = self.hover_shots.get(entry.appid).to_vec();
                            if urls.is_empty() {
                                urls = self.store_info.screenshots(entry.appid);
                            }
                            // Ni fiche sur disque ni fiche déjà lue : la
                            // boutique est interrogée, et le diaporama
                            // arrive dès qu'elle répond.
                            if urls.is_empty() && entry.store.is_some() {
                                self.store_info.ensure(&ctx, &self.client, entry.appid);
                            }
                            let store_banner = self.store_info.banner(entry.appid);
                            if let Some(banner) = &store_banner {
                                self.gameinfo
                                    .news_art
                                    .ensure(&ctx, std::slice::from_ref(banner));
                            }
                            if !urls.is_empty() {
                                self.gameinfo.news_art.ensure(&ctx, &urls);
                            }
                            let shots: Vec<egui::TextureHandle> = urls
                                .iter()
                                .filter_map(|url| self.gameinfo.news_art.get(url).cloned())
                                .collect();
                            // La capsule large de Steam, et non la
                            // jaquette verticale de la grille, ouvre
                            // l'aperçu. Elle est déjà téléchargée pour la
                            // fiche ; on la demande au même cache.
                            let details = self
                                .catalog
                                .as_ref()
                                .and_then(|catalog| {
                                    catalog
                                        .entries
                                        .iter()
                                        .find(|game| game.appid == entry.appid)
                                })
                                .map(|game| game.details.clone())
                                .unwrap_or_default();
                            // La même illustration large que la fiche
                            // Steam, en 616×353. Un jeu de boutique n'a
                            // pas de capsule Steam : on réutilise d'abord
                            // son illustration large déjà en cache (lue
                            // pour sa fiche), puis la bannière publiée par
                            // la boutique, et enfin la jaquette de la
                            // tuile pour ne jamais laisser un vide pendant
                            // le téléchargement.
                            let capsule = if entry.store.is_some() {
                                self.art
                                    .get(
                                        &ctx,
                                        &self.paths,
                                        entry.appid,
                                        &details,
                                        SteamArt::Wide,
                                        &entry.name,
                                    )
                                    .or_else(|| {
                                        store_banner.as_deref().and_then(|url| {
                                            self.gameinfo.news_art.get(url).cloned()
                                        })
                                    })
                                    .or_else(|| texture.clone())
                            } else {
                                self.art.get(
                                    &ctx,
                                    &self.paths,
                                    entry.appid,
                                    &details,
                                    SteamArt::Wide,
                                    &entry.name,
                                )
                            };
                            let game_icon = self
                                .art
                                .get(
                                    &ctx,
                                    &self.paths,
                                    entry.appid,
                                    &details,
                                    SteamArt::Icon,
                                    &entry.name,
                                )
                                .or_else(|| square_icon_from_cover(&ctx, &self.paths, entry.appid));
                            let origin = game_origin_label(
                                entry.store,
                                entry.steam,
                                entry.family,
                                entry.family_available,
                            );
                            let store_mark = game_store_mark(
                                entry.store,
                                entry.steam,
                                entry.family,
                                entry.family_available,
                            );
                            game_hover_card(
                                &ctx,
                                response.rect,
                                entry.appid,
                                &entry.name,
                                capsule.as_ref(),
                                self.art.focus_of(entry.appid, SteamArt::Wide),
                                game_icon.as_ref(),
                                store_mark,
                                &shots,
                                &origin,
                                entry.update.is_some(),
                                now.duration_since(started),
                                hover_width,
                            );
                        }
                        // L'icône carrée n'est lue qu'à l'ouverture du
                        // menu : la chercher pour chaque tuile à chaque
                        // image ferait payer la grille entière.
                        if response.secondary_clicked() {
                            // La fiche du catalogue porte l'adresse de
                            // l'icône carrée ; un jeu du disque la trouve
                            // de toute façon dans le cache du client.
                            let details = self
                                .catalog
                                .as_ref()
                                .and_then(|catalog| {
                                    catalog
                                        .entries
                                        .iter()
                                        .find(|game| game.appid == entry.appid)
                                })
                                .map(|game| game.details.clone())
                                .unwrap_or_default();
                            let icon = {
                                let found = self.art.get(
                                    ui.ctx(),
                                    &self.paths,
                                    entry.appid,
                                    &details,
                                    SteamArt::Icon,
                                    &entry.name,
                                );
                                found.or_else(|| {
                                    square_icon_from_cover(ui.ctx(), &self.paths, entry.appid)
                                })
                            };
                            self.cover_menu_icon = Some((entry.appid, icon));
                        }
                        let icon = self
                            .cover_menu_icon
                            .as_ref()
                            .filter(|(appid, _)| *appid == entry.appid)
                            .and_then(|(_, icon)| icon.as_ref());
                        let hidden = self.config.library.hidden.contains(&entry.appid);
                        let (pick, reset, folder, collection_changes, action) = cover_context_menu(
                            response,
                            entry.appid,
                            &entry.name,
                            icon,
                            has_custom,
                            entry.installed,
                            hidden,
                            install_dir.as_deref(),
                            &self.config.collections.items,
                            None,
                        );
                        match action {
                            Some(GameAction::Play(appid)) => play_request = Some(appid),
                            Some(GameAction::Install(appid)) => install_request = Some(appid),
                            Some(GameAction::Sheet(appid)) => clicked = Some(appid),
                            Some(GameAction::Hide(appid)) => hide_request = Some(appid),
                            Some(GameAction::Unhide(appid)) => unhide_request = Some(appid),
                            None => {}
                        }
                        for (collection_id, member) in collection_changes {
                            self.toggle_collection_game(&collection_id, entry.appid, member);
                            ui.ctx().request_repaint();
                        }
                        if pick.is_some() {
                            picker_request = Some((entry.appid, entry.name.clone()));
                        }
                        reset_cover = reset_cover.or(reset);
                        if folder.is_some() {
                            open_folder = folder;
                        }
                    }
                });
                ui.add_space(gap);
            }
        });
        if trace_enabled() {
            eprintln!(
                "library page: grid done at {} ms",
                page_started.elapsed().as_millis()
            );
        }
        // Une tuile sans texture peut simplement attendre son téléchargement,
        // mais elle peut aussi être sur disque et bloquée par le budget de
        // décodage. Pendant une courte fenêtre, on redemande un rendu à intervalle
        // régulier : les téléchargements Steam ont ainsi le temps d'aboutir sans
        // que la grille ne s'endorme sur des initiales.
        if missing_visible && !scrolling {
            if let Some(until) = self.library_fill_until {
                if Instant::now() < until {
                    ctx.request_repaint_after(LIBRARY_FILL_INTERVAL);
                } else {
                    self.library_fill_until = None;
                }
            }
        }
        if hovered_card.is_none() {
            self.hover_since = None;
        }
        if let Some(appid) = clicked {
            self.selected = Some(appid);
        }
        if let Some(appid) = reset_cover {
            if remove_custom_cover(&self.paths, appid) {
                self.covers.forget(appid);
                self.notify(lapinux_core::i18n::t("library.cover_reset"));
            }
        }
        if let Some((appid, name)) = picker_request {
            self.open_cover_picker(appid, name, &ctx);
        }
        if let Some(path) = open_folder {
            self.open_folder(&path);
        }
        // Actions rapides du menu et du clavier : lancer, installer, masquer.
        if let Some(appid) = play_request.or(launch_from_keys) {
            let installed = base
                .iter()
                .any(|entry| entry.appid == appid && entry.installed);
            if installed {
                self.launch(appid, false);
            } else {
                self.selected = Some(appid);
            }
        }
        if let Some(appid) = install_request {
            self.install_game(appid, Vec::new());
        }
        if let Some(appid) = hide_request {
            if !self.config.library.hidden.contains(&appid) {
                self.config.library.hidden.push(appid);
            }
            self.library_focus = None;
            self.config_dirty_at = Some(Instant::now());
            self.touch_library();
        }
        if let Some(appid) = unhide_request {
            self.config.library.hidden.retain(|id| *id != appid);
            self.config_dirty_at = Some(Instant::now());
            self.touch_library();
        }
    }

    /// Prépare les jaquettes des premières rangées avant que la grille ne les
    /// peigne. Le budget relevé ne vaut que pour cette passe : la priorité va
    /// aux tuiles du haut, puis le rythme normal reprend.
    fn warm_library_covers(
        &mut self,
        ctx: &egui::Context,
        entries: &[&LibraryEntry],
        columns: usize,
    ) {
        const PRELOAD_ROWS: usize = 3;
        const PRELOAD_REQUESTS: usize = 12;
        let count = columns.saturating_mul(PRELOAD_ROWS).min(entries.len());
        if count == 0 {
            return;
        }
        self.covers.boost_requests(PRELOAD_REQUESTS.min(count));
        let first_store: Vec<u32> = entries
            .iter()
            .take(count)
            .filter_map(|entry| entry.store.map(|_| entry.appid))
            .collect();
        if !first_store.is_empty() {
            self.covers.prioritize_prefetch(&first_store);
            self.covers.pump_prefetch(ctx, &self.paths);
        }
        for entry in entries.iter().take(count) {
            // Une jaquette de boutique vient d'être remontée dans sa file de
            // préchargement ; une jaquette Steam présente sur disque sera
            // décodée par la grille sans réseau. Seules les manquantes prennent
            // un créneau.
            if entry.store.is_some()
                || custom_cover_path(&self.paths, entry.appid).is_file()
                || self
                    .paths
                    .covers_dir()
                    .join(format!("{}.jpg", entry.appid))
                    .is_file()
                || self
                    .paths
                    .covers_dir()
                    .join(format!("sgdb-{}.jpg", entry.appid))
                    .is_file()
            {
                continue;
            }
            let _ = self.covers.get(
                ctx,
                &self.paths,
                entry.appid,
                &entry.name,
                entry.store,
                &entry.cover,
                CoverTier::Tile,
            );
        }
    }

    /// Fusionne les jeux locaux et le catalogue Steam, dédupliqués par appid.
    /// Toutes les entrées de la bibliothèque, avant recherche et filtre
    /// installé. Sert aussi aux compteurs des filtres.
    ///
    /// Le résultat est gardé tant que les sources n'ont pas bougé : la page
    /// Bibliothèque et celle des Téléchargements le demandent à chaque image,
    /// et le reconstruire à chaque fois coûtait plus cher que la peinture.
    /// Vue filtrée et triée de la bibliothèque, gardée entre deux images : le
    /// filtre, le tri et les compteurs parcourent toutes les entrées, et les
    /// refaire à chaque image pesait plusieurs millisecondes sur une grande
    /// collection.
    fn library_view(&self, base: &Rc<Vec<LibraryEntry>>, needle: String) -> Rc<LibraryView> {
        let key = LibraryViewKey {
            epoch: self.library_epoch,
            base: Rc::as_ptr(base) as usize,
            needle,
            sources: self.sources,
            catalog_filter: self.catalog_filter,
            sort: self.library_sort,
            descending: self.library_descending,
            only_updates: self.only_updates,
            never_played: self.never_played,
            publisher: self.publisher_filter.clone(),
            developer: self.developer_filter.clone(),
        };
        if let Some(view) = self.library_view_cache.borrow().as_ref() {
            if view.key == key {
                return view.clone();
            }
        }
        // La recherche porte sur le titre et sur les crédits, sans se soucier
        // des accents ni de la casse : chaque mot tapé doit se retrouver, dans
        // n'importe quel ordre.
        let tokens = search_tokens(&key.needle);
        let mut order: Vec<usize> = base
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                key.sources.keeps(entry)
                    && (tokens.is_empty() || matches_tokens(&entry_search_text(entry), &tokens))
                    // Les filtres d'attributs réduisent la vue au même titre que
                    // le filtre d'installation.
                    && (!key.only_updates || entry.update.is_some())
                    && (!key.never_played || entry.playtime == 0)
                    // Les crédits choisis réduisent la vue à leurs jeux.
                    && match key.publisher.as_deref() {
                        Some(name) => credit_matches(entry, true, name),
                        None => true,
                    }
                    && match key.developer.as_deref() {
                        Some(name) => credit_matches(entry, false, name),
                        None => true,
                    }
            })
            .map(|(index, _)| index)
            .collect();
        order.sort_by(|left, right| {
            compare_library(&base[*left], &base[*right], key.sort, key.descending)
        });
        // Compteurs par source : ce que la source contient avec la recherche et
        // le filtre d'installation courants, sans dépendre des autres sources.
        let mut view = LibraryView {
            key,
            order,
            disk: 0,
            steam: 0,
            family: 0,
            all_steam: 0,
            epic: 0,
            epic_pc: 0,
            epic_mobile: 0,
            gog: 0,
            itch: 0,
        };
        for entry in base.iter() {
            if !(tokens.is_empty() || matches_tokens(&entry_search_text(entry), &tokens))
                || !view.key.catalog_filter.keeps(entry.installed)
                || (view.key.only_updates && entry.update.is_none())
                || (view.key.never_played && entry.playtime > 0)
            {
                continue;
            }
            if entry.on_disk {
                view.disk += 1;
            }
            if entry.steam && !entry.family {
                view.steam += 1;
            }
            if entry.family || entry.family_available {
                view.family += 1;
            }
            if entry.steam || entry.family_available {
                view.all_steam += 1;
            }
            match entry.store {
                Some(lapinux_core::stores::Store::Epic) => {
                    view.epic += 1;
                    if lapinux_core::config::EpicPlatform::Pc.keeps(&entry.platforms, entry.mobile)
                    {
                        view.epic_pc += 1;
                    }
                    if lapinux_core::config::EpicPlatform::Mobile
                        .keeps(&entry.platforms, entry.mobile)
                    {
                        view.epic_mobile += 1;
                    }
                }
                Some(lapinux_core::stores::Store::Gog) => view.gog += 1,
                Some(lapinux_core::stores::Store::Itch) => view.itch += 1,
                _ => {}
            }
        }
        let view = Rc::new(view);
        *self.library_view_cache.borrow_mut() = Some(view.clone());
        view
    }

    pub(super) fn library_entries_all(&self) -> Rc<Vec<LibraryEntry>> {
        let key = LibraryKey {
            epoch: self.library_epoch,
            family: family_fingerprint(self.social.as_ref()),
            hide_tools: self.config.library.hide_tools,
            show_demos: self.config.library.show_demos,
            hide_all_tools: self.config.library.hide_all_tools,
            hide_epic_tools: self.config.library.hide_epic_tools,
            hide_gog_tools: self.config.library.hide_gog_tools,
            hide_itch_tools: self.config.library.hide_itch_tools,
            hidden: self.config.library.hidden.clone(),
            show_hidden: self.config.library.show_hidden,
            account: self.steam_user.trim().to_string(),
            connected_as: self
                .steam_status
                .as_ref()
                .map(|status| status.username.clone())
                .unwrap_or_default(),
        };
        if let Some(cache) = self.library_cache.borrow().as_ref() {
            if cache.key == key {
                return cache.entries.clone();
            }
        }
        let entries = Rc::new(self.build_library_entries());
        *self.library_cache.borrow_mut() = Some(LibraryCache {
            key,
            entries: entries.clone(),
        });
        entries
    }

    /// Reconstruit la liste complète : un seul passage par source, avec un
    /// index des appid déjà vus plutôt qu'un parcours de la liste à chaque
    /// entrée (une bibliothèque de quelques centaines de jeux rendait le
    /// second parcours quadratique).
    fn build_library_entries(&self) -> Vec<LibraryEntry> {
        // Compteur de diagnostic : chaque reconstruction alloue la liste entière.
        if trace_enabled() {
            static REBUILDS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let count = REBUILDS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            eprintln!("list rebuild #{count} ({} games)", self.games.len());
        }
        let hide_tools = self
            .config
            .library
            .store_tools_hidden(lapinux_core::stores::Store::Steam);
        let show_demos = self.config.library.show_demos;
        let account = self.steam_user.trim();
        let catalog = self.catalog.as_ref().filter(|catalog| {
            account.is_empty()
                || catalog.account.trim().is_empty()
                || catalog.account.eq_ignore_ascii_case(account)
                || self
                    .steam_status
                    .as_ref()
                    .map(|status| status.username.eq_ignore_ascii_case(account))
                    .unwrap_or(false)
        });
        // Le groupe familial distingue les jeux du compte de ceux des autres
        // membres. Le compte connecté figure dans la même liste : ses appids
        // prouvent une possession personnelle même quand le catalogue le
        // présentait encore comme prêté.
        let (family_self, family_shared): (HashSet<u32>, HashSet<u32>) = self
            .social
            .as_ref()
            .and_then(|social| social.family.as_ref())
            .map(|family| {
                let mut own = HashSet::new();
                let mut shared = HashSet::new();
                for member in &family.members {
                    if member.is_self {
                        own.extend(member.appids.iter().copied());
                    } else {
                        shared.extend(member.appids.iter().copied());
                    }
                }
                (own, shared)
            })
            .unwrap_or_default();
        // Un jeu installé qui figure aussi au catalogue appartient aux deux
        // sources ; `family` ne vaut vrai que s'il n'est pas possédé en propre.
        let catalog_sources: HashMap<u32, bool> = catalog
            .map(|catalog| {
                catalog
                    .entries
                    .iter()
                    .map(|entry| (entry.appid, entry.family))
                    .collect()
            })
            .unwrap_or_default();
        let catalog_builds: HashMap<u32, Option<u64>> = catalog
            .map(|catalog| {
                catalog
                    .entries
                    .iter()
                    .map(|entry| (entry.appid, entry.details.build_id))
                    .collect()
            })
            .unwrap_or_default();
        // Date du build public : elle évite d'annoncer une mise à jour quand
        // le manifeste installé a été écrit après ce build, même si les deux
        // identifiants ne se comparent pas (builds de dépôt, vieux manifeste).
        let catalog_build_times: HashMap<u32, Option<i64>> = catalog
            .map(|catalog| {
                catalog
                    .entries
                    .iter()
                    .map(|entry| (entry.appid, entry.details.build_time))
                    .collect()
            })
            .unwrap_or_default();
        // Crédits : la fiche PICS du catalogue porte l'éditeur et le
        // développeur de chaque jeu possédé. Sans elle, les tris par crédit
        // n'auraient rien à comparer.
        let credits: HashMap<u32, (Vec<String>, Vec<String>)> = catalog
            .map(|catalog| {
                catalog
                    .entries
                    .iter()
                    .map(|item| {
                        (
                            item.appid,
                            (
                                clean_credits(&item.details.publishers),
                                clean_credits(&item.details.developers),
                            ),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        // Temps joué dans le lanceur : ses propres parties, toutes boutiques
        // confondues. Un jeu de boutique sans fiche locale se rabat sur
        // l'historique des parties.
        // Date d'ajout au compte, publiée par la liste de licences de Steam.
        let catalog_acquired: HashMap<u32, Option<i64>> = catalog
            .map(|catalog| {
                catalog
                    .entries
                    .iter()
                    .map(|entry| (entry.appid, entry.acquired))
                    .collect()
            })
            .unwrap_or_default();
        // Une démo peut aussi être installée localement : son appid suffit
        // alors à la reconnaître, même quand le catalogue ne la décrit pas.
        let demo_appids: HashSet<u32> = catalog
            .map(|catalog| {
                catalog
                    .entries
                    .iter()
                    .filter(|entry| is_steam_demo(entry))
                    .map(|entry| entry.appid)
                    .collect()
            })
            .unwrap_or_default();
        // Minutes jouées publiées par Steam, jeu par jeu.
        let catalog_minutes: HashMap<u32, Option<u64>> = catalog
            .map(|catalog| {
                catalog
                    .entries
                    .iter()
                    .map(|entry| (entry.appid, entry.minutes))
                    .collect()
            })
            .unwrap_or_default();
        let mut played: HashMap<u32, u64> = self
            .games
            .iter()
            .map(|game| (game.appid, game.playtime_secs))
            .collect();
        for session in &self.play_history {
            if played.contains_key(&session.appid) {
                continue;
            }
            let seconds = (session.ended - session.started).max(0) as u64;
            *played.entry(session.appid).or_default() += seconds;
        }
        // Le plus grand des deux temps de jeu : celui de la boutique et celui
        // que Lapinux a chronométré. Les additionner compterait deux fois les
        // parties lancées d'ici.
        let played_secs = |appid: u32, store_minutes: Option<u64>| -> u64 {
            let local = played.get(&appid).copied().unwrap_or(0);
            let store = store_minutes
                .map(|minutes| minutes.saturating_mul(60))
                .unwrap_or(0);
            local.max(store)
        };
        // Dernière partie par jeu : le tri « récemment joués » la lit, et le
        // catalogue la reprend pour les jeux qui ne sont pas sur le disque.
        let mut last_played: HashMap<u32, i64> = self
            .games
            .iter()
            .filter_map(|game| game.last_played.map(|at| (game.appid, at)))
            .collect();
        for session in &self.play_history {
            last_played
                .entry(session.appid)
                .and_modify(|at| *at = (*at).max(session.started))
                .or_insert(session.started);
        }
        let mut entries: Vec<LibraryEntry> = Vec::new();
        let mut seen: HashMap<u32, usize> = HashMap::new();
        for game in &self.games {
            if (!show_demos && demo_appids.contains(&game.appid))
                || (hide_tools && is_steam_tool(&game.name))
            {
                continue;
            }
            let catalog_family = catalog_sources.get(&game.appid).copied().unwrap_or(false);
            let self_owns = family_self.contains(&game.appid);
            let family = catalog_family && !self_owns;
            let family_available = catalog_family || family_shared.contains(&game.appid);
            // Un build public plus récent que le manifeste installé signale
            // une mise à jour ; un manifeste absent ou illisible ne conclut rien.
            let installed_build = game.build_id.trim().parse::<u64>().ok();
            let update = if game.installed {
                catalog_builds
                    .get(&game.appid)
                    .copied()
                    .flatten()
                    .filter(|latest| installed_build.map_or(false, |build| *latest > build))
                    .filter(|_| {
                        // Un manifeste plus récent que le build public prouve
                        // que le jeu est déjà à jour : sans cette garde, une
                        // différence d'identifiants faisait proposer un
                        // téléchargement complet pour rien.
                        catalog_build_times
                            .get(&game.appid)
                            .copied()
                            .flatten()
                            .map_or(true, |built| {
                                game.last_updated <= 0 || built > game.last_updated + 120
                            })
                    })
            } else {
                None
            };
            seen.insert(game.appid, entries.len());
            entries.push(LibraryEntry {
                appid: game.appid,
                name: game.name.clone(),
                installed: game.installed,
                on_disk: true,
                steam: catalog_sources.contains_key(&game.appid),
                family,
                family_available,
                update,
                store: None,
                cover: String::new(),
                platforms: Vec::new(),
                mobile: false,
                size: None,
                publishers: Vec::new(),
                developers: Vec::new(),
                genres: Vec::new(),
                playtime: played_secs(
                    game.appid,
                    catalog_minutes.get(&game.appid).copied().flatten(),
                ),
                acquired: catalog_acquired.get(&game.appid).copied().flatten(),
                updated: game.last_updated,
                last_played: last_played.get(&game.appid).copied().unwrap_or(0),
            });
        }
        if let Some(catalog) = catalog {
            for item in &catalog.entries {
                if !steam_entry_visible(item, show_demos)
                    || (hide_tools && is_steam_tool(&item.name))
                    || seen.contains_key(&item.appid)
                {
                    continue;
                }
                let self_owns = family_self.contains(&item.appid) || !item.family;
                let family = item.family && !self_owns;
                let family_available = item.family || family_shared.contains(&item.appid);
                seen.insert(item.appid, entries.len());
                entries.push(LibraryEntry {
                    appid: item.appid,
                    name: item.name.clone(),
                    installed: item.installed,
                    on_disk: false,
                    steam: true,
                    family,
                    family_available,
                    update: None,
                    store: None,
                    cover: String::new(),
                    platforms: Vec::new(),
                    mobile: false,
                    size: None,
                    publishers: Vec::new(),
                    developers: Vec::new(),
                    genres: clean_credits(&item.details.genres),
                    playtime: played_secs(item.appid, item.minutes),
                    acquired: item.acquired,
                    updated: item.details.build_time.unwrap_or(0),
                    last_played: last_played.get(&item.appid).copied().unwrap_or(0),
                });
            }
        }
        // Les boutiques omettent souvent le studio : Epic ne nomme parfois que
        // l'éditeur. Un jeu de boutique qui porte le même titre qu'une fiche
        // Steam emprunte donc ses crédits au catalogue, plutôt que de rester
        // sans développeur.
        // Les genres viennent des mêmes fiches : ils remplissent l'axe
        // « genre » même pour les jeux trouvés sur le disque.
        let catalog_genres: HashMap<u32, Vec<String>> = catalog
            .map(|catalog| {
                catalog
                    .entries
                    .iter()
                    .map(|item| (item.appid, clean_credits(&item.details.genres)))
                    .collect()
            })
            .unwrap_or_default();
        let credits_by_title: HashMap<String, (Vec<String>, Vec<String>)> = catalog
            .map(|catalog| {
                catalog
                    .entries
                    .iter()
                    .filter_map(|item| {
                        let key = lapinux_core::stores::title_key(&item.name);
                        if key.is_empty() {
                            return None;
                        }
                        Some((
                            key,
                            (
                                clean_credits(&item.details.publishers),
                                clean_credits(&item.details.developers),
                            ),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Les jeux des boutiques : leur jaquette et leur poids viennent de la
        // boutique, leur fiche est la même que celle des jeux Steam — Epic,
        // GOG et itch.io compris, chacun avec sa pastille.
        for game in self
            .stores
            .owned_games()
            .into_iter()
            // Les assets et greffons du marketplace ne sont pas des jeux : la
            // grille les cache, sauf si la case de leur boutique les montre.
            .filter(|game| !game.tool || !self.config.library.store_tools_hidden(game.store))
        {
            // Une boutique qui ne nomme que l'éditeur le recopie chez le
            // développeur : ce doublon ne vaut pas un vrai studio, le
            // catalogue Steam prend alors le relais.
            let unnamed = !game.publishers.is_empty() && game.publishers == game.developers;
            let borrowed = credits_by_title
                .get(&lapinux_core::stores::title_key(&game.name))
                .cloned();
            if let Some(position) = seen.get(&game.appid).copied() {
                // Déjà installé et scanné : la ligne du disque gagne, la
                // boutique apporte seulement sa marque et sa jaquette.
                let entry = &mut entries[position];
                entry.store = Some(game.store);
                entry.cover = game.cover.clone();
                entry.platforms = game.platforms.clone();
                entry.mobile = game.mobile;
                entry.size = game.size;
                if entry.publishers.is_empty() && !game.publishers.is_empty() {
                    entry.publishers = game.publishers.clone();
                }
                if entry.developers.is_empty() && !game.developers.is_empty() {
                    entry.developers = game.developers.clone();
                }
                if let Some((publishers, developers)) = borrowed {
                    if entry.publishers.is_empty() {
                        entry.publishers = publishers;
                    }
                    if entry.developers.is_empty() {
                        entry.developers = developers;
                    }
                }
                continue;
            }
            seen.insert(game.appid, entries.len());
            entries.push(LibraryEntry {
                appid: game.appid,
                name: game.name.clone(),
                installed: game.installed,
                on_disk: game.installed,
                steam: false,
                family: false,
                family_available: false,
                update: None,
                store: Some(game.store),
                cover: game.cover.clone(),
                platforms: game.platforms.clone(),
                mobile: game.mobile,
                size: game.size,
                playtime: played_secs(game.appid, game.minutes),
                acquired: game.acquired,
                updated: 0,
                last_played: last_played.get(&game.appid).copied().unwrap_or(0),
                publishers: if game.publishers.is_empty() || unnamed {
                    borrowed
                        .as_ref()
                        .map(|(publishers, _)| publishers.clone())
                        .unwrap_or_default()
                } else {
                    game.publishers.clone()
                },
                developers: if game.developers.is_empty() || unnamed {
                    borrowed
                        .as_ref()
                        .map(|(_, developers)| developers.clone())
                        .unwrap_or_default()
                } else {
                    game.developers.clone()
                },
                genres: Vec::new(),
            });
        }
        // Chaque jeu reçoit les crédits de sa fiche, qu'il vienne du disque,
        // du catalogue ou d'une boutique.
        for entry in entries.iter_mut() {
            if let Some((publishers, developers)) = credits.get(&entry.appid) {
                entry.publishers = publishers.clone();
                entry.developers = developers.clone();
            }
            if entry.genres.is_empty() {
                if let Some(genres) = catalog_genres.get(&entry.appid) {
                    entry.genres = genres.clone();
                }
            }
        }
        // Les jeux masqués à la main sortent de la grille, sauf quand on
        // demande à les revoir.
        if !self.config.library.show_hidden {
            let hidden: HashSet<u32> = self.config.library.hidden.iter().copied().collect();
            entries.retain(|entry| !hidden.contains(&entry.appid));
        }
        // Ordre de base stable : la vue applique ensuite le tri choisi, et les
        // listes qui ne trient pas gardent les installés en tête.
        entries.sort_by_cached_key(|entry| (!entry.installed, entry.name.to_lowercase()));
        entries
    }

    /// Éditeurs ou développeurs les plus fréquents, avec leur nombre de jeux.
    /// Relevés uniquement à l'ouverture du menu, jamais à chaque image.
    fn credit_options(&self, publisher: bool) -> Vec<(String, usize)> {
        let mut counts: HashMap<String, (String, usize)> = HashMap::new();
        let mut unknown = 0usize;
        for entry in self.library_entries_all().iter() {
            let list = if publisher {
                &entry.publishers
            } else {
                &entry.developers
            };
            // Tous les crédits comptent, pas seulement le premier : un jeu
            // co-développé nomme plusieurs studios, et chacun doit pouvoir
            // être choisi dans le menu.
            let mut kept: Vec<String> = Vec::new();
            for name in list {
                // « SEGA of Europe » et « SEGA » comptent pour le même
                // éditeur : la clé est le nom nettoyé.
                let clean = clean_credit(name);
                if clean.is_empty() {
                    continue;
                }
                let key = clean.to_lowercase();
                if kept.contains(&key) {
                    continue;
                }
                kept.push(key.clone());
                counts.entry(key).or_insert_with(|| (clean, 0)).1 += 1;
            }
            // Une fiche muette n'a toujours aucun nom à proposer.
            if kept.is_empty() {
                unknown += 1;
            }
        }
        let mut options: Vec<(String, usize)> = counts.into_values().collect();
        options.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| cmp_lower(&a.0, &b.0)));
        // Les fiches muettes se retrouvent sous un seul nom, en fin de liste.
        if unknown > 0 {
            options.push((UNKNOWN_CREDIT.to_string(), unknown));
        }
        options
    }

    pub(super) fn library_entries(&self) -> Vec<LibraryEntry> {
        let tokens = search_tokens(&self.search);
        self.library_entries_all()
            .iter()
            .filter(|entry| {
                self.sources.keeps(entry)
                    && self.catalog_filter.keeps(entry.installed)
                    && (tokens.is_empty() || matches_tokens(&entry_search_text(entry), &tokens))
            })
            .cloned()
            .collect()
    }

    /// Les données de la bibliothèque ont changé (scan, catalogue, boutiques,
    /// compte) : la liste fusionnée sera reconstruite à la prochaine demande.
    pub(super) fn touch_library(&mut self) {
        self.library_epoch = self.library_epoch.wrapping_add(1);
        self.library_covers_warmed = false;
        self.library_fill_until = None;
    }

    /// Entretient l'état de la synchronisation demandée par l'icône de la
    /// Bibliothèque. La flèche tourne tant qu'une lecture est en vol ; elle
    /// s'arrête d'elle-même une fois les trois sources revenues, avec un temps
    /// minimal pour que le geste reste lisible.
    pub(super) fn poll_library_sync(&mut self, ctx: &egui::Context) {
        let Some(started) = self.library_sync else {
            return;
        };
        if started.elapsed() > Duration::from_secs(90) {
            self.library_sync = None;
            return;
        }
        let busy = self.refresh_rx.is_some()
            || self.rescan_after_refresh
            || self.catalog_want.is_some()
            || self.catalog_rx.is_some()
            || self
                .catalog
                .as_ref()
                .map(|catalog| catalog.syncing)
                .unwrap_or(false)
            || !self.stores.ready();
        if busy || started.elapsed() < Duration::from_millis(900) {
            // La flèche tourne à la cadence de la peinture : 33 ms suffisent
            // pour une rotation fluide sans repeindre la page à 60 images/s.
            ctx.request_repaint_after(Duration::from_millis(33));
        } else {
            self.library_sync = None;
        }
    }

    /// Compare les bibliothèques au lancement précédent, une fois les sources
    /// chargées. Le contrôle attend le premier scan et la vue des comptes ; il
    /// est relancé par l'icône de rafraîchissement de la Bibliothèque.
    pub(super) fn maybe_check_library(&mut self) {
        if self.library_checked {
            return;
        }
        let Some(at) = self.library_check_at else {
            return;
        };
        if Instant::now() < at
            || !self.stores.ready()
            || self.refresh_rx.is_some()
            || self.catalog_want.is_some()
            || self.catalog_rx.is_some()
            || self
                .catalog
                .as_ref()
                .map(|catalog| catalog.syncing)
                .unwrap_or(false)
        {
            return;
        }
        if self.games.is_empty() && self.stores.owned_games().is_empty() {
            // Rien à comparer pour l'instant : le scan suivant donnera la
            // première photographie.
            self.library_check_at = Some(Instant::now() + Duration::from_secs(2));
            return;
        }
        self.check_library_now();
    }

    /// Relève les jeux présents, en déduit ceux qui sont arrivés depuis le
    /// dernier contrôle, puis enregistre la nouvelle photographie.
    fn check_library_now(&mut self) {
        let entries = self.library_entries_all().to_vec();
        if entries.is_empty() {
            return;
        }
        let previous = self.library_seen.clone();
        self.new_games = new_library_entries(&entries, previous.as_ref());
        let seen = SeenLibrary {
            games: entries
                .iter()
                .map(|entry| SeenGame {
                    appid: entry.appid,
                    name: entry.name.clone(),
                })
                .collect(),
            checked: lapinux_core::util::now_unix(),
        };
        let path = library_seen_path(&self.paths);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(text) = serde_json::to_string_pretty(&seen) {
            let _ = std::fs::write(path, text);
        }
        self.library_seen = Some(seen);
        self.library_checked = true;
        self.library_check_at = None;
    }
}
