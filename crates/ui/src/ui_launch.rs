//! Lancement des jeux et écran de transition.

use super::*;

impl LapinuxApp {
    /// Lance un jeu sans bloquer l'interface : la requête part en tâche de
    /// fond, l'écran d'attente peut donc animer le lapin pendant que Steam
    /// prépare Proton et le préfixe.
    pub(super) fn launch(&mut self, appid: u32, dry_run: bool) {
        // Un nouveau lancement efface l'écran de fin de la partie précédente.
        self.game_end = None;
        // Un lancement à la fois : un second clic ne doit pas doubler la
        // session déjà en préparation.
        if self.launching.is_some() {
            return;
        }
        let name = self.game_name(appid);
        // La scène n'est utilisée que lorsque GameScope l'est aussi : sans
        // lui, le daemon garde le lancement direct et son écran d'attente.
        let scene = !dry_run && self.config.launch.scene && self.config.launch.gamescope;
        let client = self.client.detached();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = client
                .request(&Req::Launch { appid, dry_run })
                .map_err(|error| error.to_string());
            tx.send(result).ok();
        });
        // Un nouvel écran remplace le fondu du lancement précédent : deux
        // voiles superposés se verraient.
        self.launch_fade = None;
        self.launch_hold = None;
        self.splash_poll = None;
        self.launching = Some(Launching {
            appid,
            name,
            dry_run,
            scene,
            from_tray: false,
            started: Instant::now(),
            rx,
        });
    }

    /// Nom affiché sur l'écran d'attente, qu'il vienne du disque ou du catalogue.
    pub(super) fn game_name(&self, appid: u32) -> String {
        self.games
            .iter()
            .find(|game| game.appid == appid)
            .map(|game| game.name.clone())
            .or_else(|| {
                self.catalog
                    .as_ref()
                    .and_then(|catalog| catalog.entries.iter().find(|entry| entry.appid == appid))
                    .map(|entry| entry.name.clone())
            })
            .unwrap_or_else(|| format!("App {appid}"))
    }

    /// Relève la réponse du lancement : l'attente s'efface et la notification
    /// arrive, ou la commande préparée pour un essai à blanc.
    /// Lit la demande déposée par le tray. L'écran d'attente ne peut être
    /// peint que par l'interface : le tray se contente de demander le
    /// lancement, puis c'est ce chemin qui prépare la session.
    pub(super) fn poll_launch_request(&mut self) {
        if self.launch_request_at.elapsed() < Duration::from_millis(250) {
            return;
        }
        self.launch_request_at = Instant::now();
        let path = self.paths.launch_request();
        let Ok(text) = std::fs::read_to_string(&path) else {
            return;
        };
        // La demande est consommée avant de lancer : un échec ne doit pas
        // relancer le jeu à chaque image.
        let _ = std::fs::remove_file(&path);
        let Ok(appid) = text.trim().parse::<u32>() else {
            return;
        };
        if self.launching.is_some() || self.session.is_some() {
            eprintln!("{}", lapinux_core::i18n::t("launch.tray_ignored"));
            return;
        }
        eprintln!(
            "{}",
            lapinux_core::i18n::t_args("launch.tray_requested", &[("appid", &appid.to_string())])
        );
        self.launch(appid, false);
        if let Some(wait) = self.launching.as_mut() {
            wait.from_tray = true;
        }
    }

    pub(super) fn poll_launch(&mut self, ctx: &egui::Context) {
        let Some(wait) = self.launching.as_ref() else {
            return;
        };
        let Ok(result) = wait.rx.try_recv() else {
            // Le lapin tourne : une image toutes les 100 ms suffit.
            ctx.request_repaint_after(Duration::from_millis(100));
            return;
        };
        let elapsed = wait.started.elapsed().as_secs_f32();
        let from_tray = wait.from_tray;
        let wait = self.launching.take().expect("launch task missing");
        match result {
            Ok(envelope) if envelope.ok => {
                if wait.dry_run {
                    let layers = envelope.data["layers"].as_str().unwrap_or("?");
                    let command = envelope.data["command"].as_str().unwrap_or("");
                    if self.selected == Some(wait.appid) {
                        // La fiche du jeu est ouverte : la commande s'ancre en
                        // bas d'Informations, là où le bouton a été cliqué.
                        self.dry_run_info = Some(DryRunPlan {
                            appid: wait.appid,
                            layers: layers.to_string(),
                            command: command.to_string(),
                        });
                    } else {
                        self.dry_run = Some(format!("couches : {layers}\n\n{command}"));
                    }
                } else {
                    // Le daemon repris les sauvegardes du compte avant de
                    // démarrer : la fiche relit leur état.
                    self.cloud.remove(&wait.appid);
                    self.cloud_asked.remove(&wait.appid);
                    self.notify(lapinux_core::i18n::t("launch.session_started").to_string());
                    // La commande est prête, mais le jeu n'a encore rien
                    // affiché : l'écran d'attente reste jusqu'à sa première
                    // image, sinon il disparaît sur un bureau vide. Il est
                    // posé même quand le daemon a répondu vite (jeu de
                    // boutique) : c'est le jeu qui décide, pas la durée de
                    // préparation. Avec la scène, il ne couvre que
                    // l'initialisation de GameScope : la scène prend ensuite
                    // le relais avec le même visuel large.
                    self.launch_hold = Some(LaunchHold {
                        appid: wait.appid,
                        name: wait.name,
                        scene: wait.scene,
                        elapsed: if from_tray {
                            elapsed.max(launch::DELAY)
                        } else {
                            elapsed
                        },
                        since: Instant::now(),
                        seen_session: false,
                        render_seen_at: None,
                        fps_seen_at: None,
                        from_tray,
                    });
                }
            }
            Ok(envelope) => {
                // Un échec ne laisse pas de voile : la fenêtre s'efface tout
                // de suite si elle était déjà visible.
                if from_tray {
                    self.restore_window(ctx);
                }
                // Un échec doit aussi retirer le relais de scène, sinon il
                // resterait ouvert indéfiniment au-dessus du bureau.
                if wait.scene {
                    ctx.send_viewport_cmd_to(
                        egui::ViewportId::from_hash_of("lapinux-launch-screen"),
                        egui::ViewportCommand::Close,
                    );
                }
                if !wait.dry_run && !wait.scene && launch::visible(elapsed) {
                    self.launch_fade = Some(LaunchFade {
                        appid: wait.appid,
                        name: wait.name,
                        elapsed,
                        at: Instant::now(),
                    });
                }
                self.notify(
                    envelope
                        .error
                        .unwrap_or_else(|| lapinux_core::i18n::t("downloads.failed").into()),
                );
            }
            Err(err) => {
                if from_tray {
                    self.restore_window(ctx);
                }
                // Un échec doit aussi retirer le relais de scène, sinon il
                // resterait ouvert indéfiniment au-dessus du bureau.
                if wait.scene {
                    ctx.send_viewport_cmd_to(
                        egui::ViewportId::from_hash_of("lapinux-launch-screen"),
                        egui::ViewportCommand::Close,
                    );
                }
                if !wait.dry_run && !wait.scene && launch::visible(elapsed) {
                    self.launch_fade = Some(LaunchFade {
                        appid: wait.appid,
                        name: wait.name,
                        elapsed,
                        at: Instant::now(),
                    });
                }
                self.notify(err);
            }
        }
    }

    /// Écran d'attente : une fenêtre plein écran **séparée** couvre le bureau
    /// pendant la préparation, et reste ouverte jusqu'à la première image du
    /// jeu — pas jusqu'à la simple réponse du daemon. Lapinux, lui, garde sa
    /// fenêtre et son état derrière.
    pub(super) fn launch_screen(&mut self, ctx: &egui::Context) {
        /// Apparition rapide ; disparition lente, pour que le jeu se révèle
        /// progressivement au lieu d'apparaître d'un coup.
        const FADE_IN: f32 = 0.24;
        const FADE_OUT: f32 = 0.9;
        /// Filet de sécurité : si ni la fenêtre ni la première image ne sont
        /// signalées, l'écran ne doit pas couvrir un jeu déjà jouable. Un jeu
        /// Proton lent peut dépasser trente secondes avant sa première image.
        const MAX_WAIT: f32 = 60.0;
        /// La swapchain est créée avant la première image utile : sans cette
        /// grâce, le splash découvrait un écran noir ou un menu encore en
        /// chargement. La cadence réelle, quand elle arrive, permet de rendre
        /// la main plus tôt ; le filet reste plus long pour les jeux qui ne la
        /// publient pas.
        const RENDER_GRACE: Duration = Duration::from_millis(3000);
        const FPS_GRACE: Duration = Duration::from_millis(1500);
        let viewport = egui::ViewportId::from_hash_of("lapinux-launch-screen");
        let style = launch::Style::from_config(&self.config.general.launch_screen);
        if style == launch::Style::Off {
            self.launch_fade = None;
            self.launch_hold = None;
            return;
        }

        // Première phase : la requête de lancement est encore en vol.
        if let Some(wait) = self.launching.as_ref() {
            if wait.dry_run {
                return;
            }
            // Une demande du tray vient d'être lue : le splash doit couvrir
            // l'interface tout de suite, sans le délai d'apparition normal.
            let elapsed = wait.started.elapsed().as_secs_f32();
            let elapsed = if wait.from_tray {
                elapsed.max(launch::DELAY)
            } else {
                elapsed
            };
            // La scène ne peut pas peindre avant que GameScope ait créé sa
            // surface. Son visuel est donc repris tout de suite par cette
            // fenêtre, puis la scène réelle prend le relais.
            let scene = wait.scene;
            if !scene && !launch::visible(elapsed) {
                return;
            }
            let alpha = if scene {
                1.0
            } else {
                (((elapsed - launch::DELAY) / FADE_IN).clamp(0.0, 1.0)).clamp(0.0, 1.0)
            };
            let (appid, name) = (wait.appid, wait.name.clone());
            self.show_splash(ctx, appid, name, elapsed, alpha, style);
            ctx.request_repaint_after(Duration::from_millis(16));
            return;
        }

        // Seconde phase : la commande est partie, on attend que le jeu peigne
        // vraiment sa première image. La fenêtre seule ne suffit pas : sous
        // gamescope, son enveloppe Wayland apparaît alors que le jeu charge
        // encore et que l'écran est noir.
        if self.launch_hold.is_some() {
            self.follow_splash_session();
        }
        let dismissed = if let Some(hold) = self.launch_hold.as_mut() {
            if self
                .session
                .as_ref()
                .is_some_and(|session| session.appid == hold.appid)
            {
                hold.seen_session = true;
            }
            // La swapchain est le premier signe de rendu ; la cadence réelle
            // de GameScope est plus proche de l'image présentée. On garde les
            // deux dates séparées : la seconde permet de raccourcir l'attente
            // sans revenir au signal trop précoce de la swapchain.
            let (render_started, drawing) =
                self.session.as_ref().map_or((false, false), |session| {
                    (
                        session.appid == hold.appid && session.render_started,
                        session.appid == hold.appid
                            && session.fps.as_ref().is_some_and(|fps| fps.current > 0),
                    )
                });
            if render_started {
                if hold.render_seen_at.is_none() {
                    tracing::info!("waiting screen: render detected for {}", hold.name);
                }
                hold.render_seen_at.get_or_insert(Instant::now());
            }
            if drawing && hold.fps_seen_at.is_none() {
                hold.fps_seen_at = Some(Instant::now());
                tracing::info!("waiting screen: refresh rate detected for {}", hold.name);
            }
            let render_settled = hold
                .render_seen_at
                .is_some_and(|seen| seen.elapsed() >= RENDER_GRACE);
            let fps_settled = hold
                .fps_seen_at
                .is_some_and(|seen| seen.elapsed() >= FPS_GRACE);
            let since = hold.since.elapsed().as_secs_f32();
            // Avec la scène, le jeu peut mettre plusieurs secondes à dessiner
            // sa première image, mais GameScope a déjà sa fenêtre : c'est elle
            // qui marque la fin du relais de l'interface, pas le rendu du jeu.
            let scene_window = hold.scene
                && self
                    .session
                    .as_ref()
                    .is_some_and(|session| session.appid == hold.appid && session.window_shown);
            let ready = if hold.scene {
                scene_window || (hold.seen_session && self.session.is_none()) || since >= MAX_WAIT
            } else {
                render_settled
                    || fps_settled
                    || (hold.seen_session && self.session.is_none())
                    || since >= MAX_WAIT
            };
            let was_visible = hold.scene || launch::hold_visible(hold.elapsed, since);
            ready.then(|| {
                (
                    hold.appid,
                    hold.name.clone(),
                    hold.elapsed + since,
                    was_visible,
                    hold.scene,
                )
            })
        } else {
            None
        };
        if let Some((appid, name, elapsed, was_visible, scene)) = dismissed {
            tracing::info!(
                "{}",
                lapinux_core::i18n::t_args("launch.splash_close", &[("name", &name.to_string())])
            );
            // Le jeu peut avoir ouvert sa fenêtre sur un autre moniteur : le
            // fondu doit révéler le jeu, pas le bureau de DP-2. KWin place donc
            // la fenêtre plein écran sur l'écran cible et la sort de la couche
            // inférieure avant que le voile ne commence à s'effacer.
            if let Err(err) =
                lapinux_input::shortcut::prepare_splash_reveal(&self.config.launch.output)
            {
                tracing::warn!(
                    "{}",
                    lapinux_core::i18n::t_args(
                        "launch.splash_preparing",
                        &[("err", &err.to_string())]
                    )
                );
            }
            self.launch_hold = None;
            self.splash_poll = None;
            if scene {
                // La scène est déjà dessinée derrière : inutile de la
                // recouvrir pendant un fondu. On ferme le relais dès que
                // GameScope est visible, sans attendre la première image du
                // jeu.
                ctx.send_viewport_cmd_to(viewport, egui::ViewportCommand::Close);
                let paths = self.paths.clone();
                std::thread::spawn(move || {
                    let client = Client::new(&paths).without_spawn();
                    let _ = client
                        .request_with_timeout(&Req::FocusGame { appid }, Duration::from_secs(3));
                });
                return;
            }
            // Un écran jamais peint ne s'efface pas : sinon un voile
            // apparaîtrait uniquement pour disparaître, alors que le jeu a
            // déjà pris la main.
            self.launch_fade = was_visible.then(|| LaunchFade {
                appid,
                name,
                elapsed,
                at: Instant::now(),
            });
        }

        if let Some(hold) = self.launch_hold.as_ref() {
            let since = hold.since.elapsed().as_secs_f32();
            // Grâce : le jeu a le temps de peindre sa première image sans
            // qu'un écran d'attente clignote pour 100 ms. Avec la scène, ce
            // relais doit au contraire apparaître tout de suite.
            if !hold.scene && !launch::hold_visible(hold.elapsed, since) {
                ctx.request_repaint_after(Duration::from_millis(16));
                return;
            }
            let elapsed = hold.elapsed + since;
            let alpha = if hold.scene {
                1.0
            } else if hold.elapsed >= launch::DELAY {
                1.0
            } else {
                ((since - launch::HOLD_GRACE) / FADE_IN).clamp(0.0, 1.0)
            };
            let (appid, name) = (hold.appid, hold.name.clone());
            self.show_splash(ctx, appid, name, elapsed, alpha, style);
            ctx.request_repaint_after(Duration::from_millis(16));
            return;
        }

        // Fondu de sortie : la fenêtre du jeu a pris la main, l'écran s'efface
        // au lieu de disparaître d'une image à l'autre.
        if let Some(fade) = self.launch_fade.as_ref() {
            let gone = fade.at.elapsed().as_secs_f32();
            let (appid, name, elapsed) = (fade.appid, fade.name.clone(), fade.elapsed);
            if gone < FADE_OUT {
                self.show_splash(ctx, appid, name, elapsed, 1.0 - gone / FADE_OUT, style);
                ctx.request_repaint_after(Duration::from_millis(16));
                return;
            }
        }
        if let Some(fade) = self.launch_fade.take() {
            // La fenêtre du fondu ne doit pas rester : transparente, elle
            // garderait le niveau « au-dessus » et capterait les clics sur
            // toute la surface. On la ferme pour de bon une fois effacée.
            ctx.send_viewport_cmd_to(viewport, egui::ViewportCommand::Close);
            // L'écran d'attente avait pris le focus : sans cette demande, KWin
            // rendait la main à Lapinux et il fallait cliquer sur le jeu.
            let paths = self.paths.clone();
            let appid = fade.appid;
            std::thread::spawn(move || {
                let client = Client::new(&paths).without_spawn();
                let _ =
                    client.request_with_timeout(&Req::FocusGame { appid }, Duration::from_secs(3));
            });
        }
    }

    /// Relit la session pendant l'attente de la première image, à part du
    /// rafraîchissement général (3 s) : c'est ce qui retire l'écran d'attente
    /// dans la seconde qui suit le vrai début du rendu, et non trois secondes
    /// plus tard. Une requête à la fois, jamais sur le fil de l'interface.
    fn follow_splash_session(&mut self) {
        if self.launch_hold.is_none() {
            self.splash_poll = None;
            return;
        }
        let mut fresh = None;
        if let Some(rx) = self.splash_poll.as_ref() {
            match rx.try_recv() {
                Ok(info) => fresh = Some(info),
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => self.splash_poll = None,
            }
        }
        if let Some(info) = fresh {
            self.session = info;
            self.splash_poll = None;
        }
        if self.splash_poll.is_none() && self.splash_poll_at.elapsed() >= Duration::from_millis(300)
        {
            self.splash_poll_at = Instant::now();
            let client = self.client.detached();
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let info = client
                    .request_with_timeout(&Req::Session, Duration::from_secs(5))
                    .ok()
                    .and_then(|envelope| envelope.into_data::<Option<SessionInfo>>().ok())
                    .flatten();
                let _ = tx.send(info);
            });
            self.splash_poll = Some(rx);
        }
    }

    /// Visuel large de la scène pour le relais d'attente de l'interface.
    /// Chargé une seule fois ; un fichier absent n'est pas mémorisé comme un
    /// échec, car le daemon peut le télécharger pendant le lancement.
    fn launch_backdrop(&mut self, ctx: &egui::Context, appid: u32) -> Option<egui::TextureHandle> {
        if let Some(cached) = self.launch_backdrops.get(&appid) {
            if cached.is_some() {
                return cached.clone();
            }
        }
        let path = self.paths.art_dir().join(format!("steam-shot-{appid}.jpg"));
        if !path.is_file() {
            return None;
        }
        let texture = load_texture_file(ctx, &path, &format!("launch-backdrop-{appid}"))?;
        self.launch_backdrops.insert(appid, Some(texture.clone()));
        Some(texture)
    }

    /// Peint l'écran d'attente dans sa propre fenêtre plein écran. La fenêtre
    /// principale de Lapinux reste intacte derrière : rien n'est remplacé, et
    /// un clic sur l'écran (une fois le jeu parti) le masque.
    fn show_splash(
        &mut self,
        ctx: &egui::Context,
        appid: u32,
        name: String,
        elapsed: f32,
        alpha: f32,
        style: launch::Style,
    ) {
        let scene = self.launching.as_ref().is_some_and(|wait| wait.scene)
            || self.launch_hold.as_ref().is_some_and(|hold| hold.scene);
        // Chargé avant les autres emprunts de `self` : la texture est ensuite
        // prêtée au peintre pendant toute la durée de la fenêtre.
        let backdrop = if scene {
            self.launch_backdrop(ctx, appid)
        } else {
            None
        };
        // Jaquette du jeu : celle de sa boutique quand il en vient une, sinon
        // celle du cache Steam. Elle arrive en tâche de fond.
        let (store, remote) = match self.stores.cover_of(appid) {
            Some((store, cover)) => (Some(store), cover),
            None => (None, String::new()),
        };
        let cover = self.covers.get(
            ctx,
            &self.paths,
            appid,
            &name,
            store,
            &remote,
            CoverTier::Sheet,
        );
        let bunny = self.bunny.clone();
        let cancellable = self.launching.is_none() && !scene;
        // Une demande du tray ne doit pas laisser apparaître la bibliothèque
        // pendant les premières images du fondu : le splash est opaque tout de
        // suite, même si sa fenêtre met une image à être mappée.
        let from_tray = self.launching.as_ref().is_some_and(|wait| wait.from_tray)
            || self.launch_hold.as_ref().is_some_and(|hold| hold.from_tray);
        let alpha = if from_tray { 1.0 } else { alpha };
        let mut dismiss = false;
        let viewport = egui::ViewportId::from_hash_of("lapinux-launch-screen");
        let builder = egui::ViewportBuilder::default()
            .with_title(lapinux_core::SPLASH_TITLE)
            .with_decorations(false)
            .with_resizable(false)
            .with_inner_size([1280.0, 800.0])
            .with_fullscreen(true)
            // Au-dessus de la fenêtre du jeu, y compris quand elle passe en
            // plein écran : rien ne doit apparaître sur l'écran couvert tant
            // que l'écran d'attente est là.
            .with_always_on_top()
            // Le voile lui-même s'efface à la fin : sans transparence, le fond
            // noir resterait opaque et la fenêtre disparaîtrait d'un coup.
            .with_transparent(true)
            .with_icon(window_icon());
        let veil = egui::Color32::from_rgba_unmultiplied(
            BACKGROUND().r(),
            BACKGROUND().g(),
            BACKGROUND().b(),
            (alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
        );
        ctx.show_viewport_immediate(viewport, builder, |ctx, _class| {
            egui::CentralPanel::default()
                .frame(egui::Frame::none().fill(veil))
                .show(ctx, |ui| {
                    let rect = ctx.screen_rect();
                    if scene {
                        // Même composition que la scène GameScope : le relais
                        // d'attente ne change donc pas de visuel à l'écran.
                        paint_scene(
                            ui.painter(),
                            rect,
                            backdrop.as_ref(),
                            cover.as_ref(),
                            bunny.as_ref(),
                            &name,
                            "Lancement…",
                            false,
                            None,
                            elapsed,
                        );
                    } else {
                        match style {
                            launch::Style::Bunny => {
                                if let Some(bunny) = bunny.as_ref() {
                                    launch::paint(ui.painter(), rect, bunny, &name, elapsed, alpha);
                                }
                            }
                            _ => launch::paint_capsule(
                                ui.painter(),
                                rect,
                                cover.as_ref(),
                                &name,
                                elapsed,
                                alpha,
                            ),
                        }
                    }
                    if cancellable {
                        ui.painter().text(
                            rect.center_bottom() - egui::vec2(0.0, 26.0),
                            egui::Align2::CENTER_BOTTOM,
                            lapinux_core::i18n::t("launch.click_to_hide"),
                            egui::FontId::proportional(11.0),
                            egui::Color32::from_rgba_unmultiplied(
                                TEXT_DIM().r(),
                                TEXT_DIM().g(),
                                TEXT_DIM().b(),
                                (alpha.clamp(0.0, 1.0) * 150.0) as u8,
                            ),
                        );
                    }
                });
            if cancellable && ctx.input(|input| input.pointer.any_click()) {
                dismiss = true;
            }
            if ctx.input(|input| input.viewport().close_requested()) {
                dismiss = true;
            }
            ctx.request_repaint_after(Duration::from_millis(16));
        });
        if dismiss {
            self.splash_poll = None;
            if let Some(hold) = self.launch_hold.take() {
                if hold.scene {
                    // Le relais de scène ne se fond pas : la scène GameScope
                    // est déjà derrière, et le fondu ne doit pas retarder sa
                    // prise de main.
                    ctx.send_viewport_cmd_to(viewport, egui::ViewportCommand::Close);
                    self.launch_fade = None;
                } else {
                    self.launch_fade = Some(LaunchFade {
                        appid: hold.appid,
                        name: hold.name,
                        elapsed: hold.elapsed + hold.since.elapsed().as_secs_f32(),
                        at: Instant::now(),
                    });
                }
            } else {
                self.launch_fade = None;
            }
        }
    }
}
