//! Fenêtre de jeu et gestion du cycle de vie.

use super::*;

impl LapinuxApp {
    /// Écran de fin : exactement le vocabulaire de l'écran de démarrage, dans
    /// une fenêtre plein écran séparée. Il couvre le bureau dès la fermeture du
    /// jeu, le temps que Lapinux revienne derrière — sans laisser voir le
    /// passage d'une fenêtre à l'autre.
    pub(super) fn game_end_overlay(&mut self, ctx: &egui::Context) {
        /// Durées calquées sur l'écran d'attente : apparition immédiate,
        /// disparition lente.
        const FADE_OUT: f32 = 0.9;
        const TOTAL: f32 = 4.0;
        let viewport = egui::ViewportId::from_hash_of("lapinux-end-screen");
        if !self.config.general.end_screen {
            if self.game_end.is_some() || self.game_end_open {
                self.game_end = None;
                self.game_end_open = false;
                self.game_end_timer_started = false;
                ctx.send_viewport_cmd_to(viewport, egui::ViewportCommand::Close);
                self.restore_window(ctx);
            }
            return;
        }
        let Some((name, mut until)) = self.game_end.clone() else {
            if self.game_end_open {
                self.game_end_open = false;
                ctx.send_viewport_cmd_to(viewport, egui::ViewportCommand::Close);
            }
            return;
        };
        if !self.game_end_open {
            // La fenêtre principale est restée mappée derrière le jeu : le
            // viewport de fin est une fenêtre séparée, toujours au-dessus. On
            // ne ramène pas la bibliothèque ici, sinon elle apparaît sur
            // l'autre écran avant l'écran de fin.
            // Le minuteur ne part qu'à la première image réellement peinte :
            // une fenêtre restée endormie ne doit pas consommer les quatre
            // secondes de l'écran avant de l'avoir montré. Le premier passage
            // ci-dessous arme la valeur provisoire ; la première image la
            // repousse.
            until = Instant::now() + Duration::from_secs_f32(TOTAL);
            self.game_end = Some((name.clone(), until));
            self.game_end_timer_started = false;
            tracing::info!(
                "{}",
                lapinux_core::i18n::t_args("window.end_open", &[("name", &name.to_string())])
            );
        }
        let now = Instant::now();
        if now >= until {
            self.game_end = None;
            self.game_end_open = false;
            self.game_end_timer_started = false;
            ctx.send_viewport_cmd_to(viewport, egui::ViewportCommand::Close);
            tracing::info!("{}", lapinux_core::i18n::t("window.end_close"));
            // L'écran de fin avait pris le focus : Lapinux le reprend pour
            // que le bureau ne reste pas sur une fenêtre morte.
            self.restore_window(ctx);
            return;
        }
        let left = (until - now).as_secs_f32();
        // L'animation du lapin part de l'ouverture de l'écran ; l'opacité, elle,
        // est complète dès la première image.
        let elapsed = (TOTAL - left).max(0.0);
        let alpha = (left / FADE_OUT).clamp(0.0, 1.0);
        let appid = self.last_session.as_ref().map(|(appid, _)| *appid);
        let texture = appid.and_then(|appid| {
            let (store, remote) = match self.stores.cover_of(appid) {
                Some((store, cover)) => (Some(store), cover),
                None => (None, String::new()),
            };
            self.covers.get(
                ctx,
                &self.paths,
                appid,
                &name,
                store,
                &remote,
                CoverTier::Sheet,
            )
        });
        let bunny = self.bunny.clone();
        let style = match launch::Style::from_config(&self.config.general.launch_screen) {
            launch::Style::Bunny => launch::Style::Bunny,
            // L'écran de fin garde toujours une image : « aucun » ne vaut que
            // pour l'attente du lancement.
            _ => launch::Style::Capsule,
        };
        let mut dismiss = false;
        let mut painted = false;
        // Même titre que l'écran d'attente : KWin le reconnaît et le garde
        // au-dessus des fenêtres plein écran, comme pendant le lancement.
        let builder = egui::ViewportBuilder::default()
            .with_title(lapinux_core::SPLASH_TITLE)
            .with_decorations(false)
            .with_resizable(false)
            .with_inner_size([1280.0, 800.0])
            .with_fullscreen(true)
            .with_always_on_top()
            .with_transparent(true)
            .with_icon(window_icon());
        let veil = egui::Color32::from_rgba_unmultiplied(
            BACKGROUND().r(),
            BACKGROUND().g(),
            BACKGROUND().b(),
            (alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
        );
        ctx.show_viewport_immediate(viewport, builder, |ctx, _class| {
            // Le simple passage dans le viewport est la première peinture :
            // `inner_rect` peut rester vide pour un viewport immédiat, même
            // lorsque la fenêtre est déjà à l'écran. Le minuteur part donc
            // d'ici, et non de l'ordre d'ouverture.
            painted = true;
            egui::CentralPanel::default()
                .frame(egui::Frame::none().fill(veil))
                .show(ctx, |ui| {
                    let rect = ctx.screen_rect();
                    match style {
                        launch::Style::Bunny => {
                            if let Some(bunny) = bunny.as_ref() {
                                launch::paint(ui.painter(), rect, bunny, &name, elapsed, alpha);
                            }
                        }
                        _ => launch::paint_capsule(
                            ui.painter(),
                            rect,
                            texture.as_ref(),
                            &name,
                            elapsed,
                            alpha,
                        ),
                    }
                    ui.painter().text(
                        rect.center_bottom() - egui::vec2(0.0, 26.0),
                        egui::Align2::CENTER_BOTTOM,
                        lapinux_core::i18n::t("launch.over"),
                        egui::FontId::proportional(11.0),
                        egui::Color32::from_rgba_unmultiplied(
                            TEXT_DIM().r(),
                            TEXT_DIM().g(),
                            TEXT_DIM().b(),
                            (alpha.clamp(0.0, 1.0) * 150.0) as u8,
                        ),
                    );
                });
            if ctx.input(|input| input.pointer.any_click()) {
                dismiss = true;
            }
            if ctx.input(|input| input.viewport().close_requested()) {
                dismiss = true;
            }
            ctx.request_repaint_after(Duration::from_millis(16));
        });
        self.game_end_open = true;
        if !self.game_end_timer_started && painted {
            // Première image mappée : les quatre secondes complètes partent
            // maintenant, même si KWin a mis du temps à présenter la fenêtre.
            self.game_end_timer_started = true;
            let fresh = Instant::now() + Duration::from_secs_f32(TOTAL);
            self.game_end = Some((name.clone(), fresh));
            tracing::info!("{}", lapinux_core::i18n::t("window.end_first_frame"));
        }
        if dismiss {
            self.game_end = None;
        }
        if self.game_end.is_none() {
            ctx.send_viewport_cmd_to(viewport, egui::ViewportCommand::Close);
            self.game_end_open = false;
            self.game_end_timer_started = false;
            self.restore_window(ctx);
        } else {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
    }

    /// Suit la partie en cours : Lapinux s'efface quand le jeu a pris l'écran
    /// et revient quand il se ferme. Pendant la partie, l'icône de
    /// notification reste le seul accès.
    pub(super) fn follow_game_window(&mut self, ctx: &egui::Context) {
        if let Some(session) = &self.session {
            self.last_session = Some((session.appid, session.name.clone()));
            self.last_session_scene = session.scene;
        }
        // La fenêtre du jeu peut se fermer avant que son processus ne sorte :
        // l'écran de fin se pose dès la fermeture, sans attendre Proton.
        let playing = self
            .session
            .as_ref()
            .is_some_and(|session| !session.window_gone);
        let scene_session = self.session.as_ref().is_some_and(|session| session.scene);
        if playing {
            self.playing_before = true;
        } else if self.playing_before {
            self.playing_before = false;
            // Ne pas révéler la bibliothèque ici : elle apparaîtrait sur
            // l'autre écran avant que l'écran de fin ne soit posé. Lapinux
            // reste rangé derrière ; l'écran de fin est une fenêtre séparée,
            // déjà toujours au-dessus. La bibliothèque ne revient qu'à sa
            // fermeture.
            if !self.last_session_scene && self.config.general.end_screen {
                if let Some((_, name)) = self.last_session.clone() {
                    self.game_end = Some((name, Instant::now() + Duration::from_secs(4)));
                }
            }
        }
        if playing
            && !scene_session
            && !self.hidden_for_game
            && self.launching.is_none()
            && self.launch_hold.is_none()
            && self.launch_fade.is_none()
        {
            self.hidden_for_game = true;
            // Ne pas réduire : une fenêtre réduite ne mappe pas ses viewports
            // enfants, et l'écran de fin restait invisible jusqu'à sa
            // fermeture. On garde la fenêtre mappée derrière le jeu, hors de
            // la barre des tâches ; le jeu plein écran la couvre.
            let _ = lapinux_input::shortcut::bury_lapinux();
        }
        if !playing && self.hidden_for_game {
            self.hidden_for_game = false;
            self.recording = false;
            // L'écran de fin couvre le retour : la fenêtre principale reste
            // rangée jusqu'à ce qu'il se ferme, sinon le bureau apparaît une
            // image avant le voile et la transition se voit.
            if self.game_end.is_none() {
                self.restore_window(ctx);
            }
        }
        // Le réveil a rempli son rôle dès que la session a changé d'état.
        if self.session.is_none() || self.game_end.is_some() {
            let _ = std::fs::remove_file(self.paths.end_request());
        }
    }

    /// Ramène la fenêtre de Lapinux au premier plan : depuis l'icône de
    /// notification, ou à la sortie d'un jeu.
    pub(super) fn restore_window(&mut self, ctx: &egui::Context) {
        // KWin remet aussi `skipTaskbar` à faux : la fenêtre avait été retirée
        // de la barre des tâches quand elle a été rangée.
        let _ = lapinux_input::shortcut::reveal_lapinux();
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    /// Ferme la fenêtre et rend sa mémoire. Le tray du daemon reste en place ;
    /// c'est lui qui relancera l'interface avec « Afficher Lapinux ».
    fn close_window(&mut self, ctx: &egui::Context) {
        self.idle_since = None;
        self.suspend_input();
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    /// Range la fenêtre dans la zone de notification quand elle n'est plus
    /// active depuis le délai réglé : une fenêtre ouverte mais inutilisée
    /// repeint l'interface pour rien, processeur et carte graphique compris.
    /// Chronomètre d'image sous `LAPINUX_FRAME_LOG` : les sondages d'un côté,
    /// la peinture de l'autre, pour savoir lequel pèse au repos.
    pub(super) fn frame_timer_new() -> Option<std::time::Instant> {
        let now = std::time::Instant::now();
        if let Ok(mut mark) = FRAME_MARK.lock() {
            *mark = Some(now);
        }
        if let Ok(mut mark) = STAGE_MARK.lock() {
            *mark = Some(now);
        }
        if !trace_enabled() && std::env::var_os("LAPINUX_FRAME_LOG").is_none() {
            return None;
        }
        Some(now)
    }

    /// Jalon de mesure sous `LAPINUX_FRAME_LOG` : coût de l'étape qui vient de
    /// s'écouler, imprimé seulement s'il pèse plus de quatre-vingts millisecondes.
    pub(super) fn frame_stage(name: &str) {
        if std::env::var_os("LAPINUX_FRAME_LOG").is_none() {
            return;
        }
        let Ok(mut mark) = STAGE_MARK.lock() else {
            return;
        };
        let Some(start) = *mark else { return };
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        if ms > 80.0 {
            eprintln!("jalon {name} : {ms:.0} ms");
        }
        *mark = Some(std::time::Instant::now());
    }

    pub(super) fn frame_timer_split() {
        if std::env::var_os("LAPINUX_FRAME_LOG").is_none() {
            return;
        }
        if let Ok(mark) = FRAME_MARK.lock() {
            if let Some(start) = *mark {
                eprintln!(
                    "sondages : {:.1} ms",
                    start.elapsed().as_secs_f64() * 1000.0
                );
            }
        }
    }

    /// Compte les images quand `LAPINUX_FPS_LOG` est posé : une interface au
    /// repos qui tourne à soixante images par seconde se repère là, sans
    /// profileur.
    pub(super) fn count_frames() {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::sync::Mutex;
        static FRAMES: AtomicU64 = AtomicU64::new(0);
        static LAST: Mutex<Option<std::time::Instant>> = Mutex::new(None);
        if std::env::var_os("LAPINUX_FPS_LOG").is_none() {
            FRAMES.store(0, Ordering::Relaxed);
            return;
        }
        FRAMES.fetch_add(1, Ordering::Relaxed);
        let now = std::time::Instant::now();
        let Ok(mut last) = LAST.lock() else { return };
        let Some(since) = *last else {
            *last = Some(now);
            return;
        };
        if now.duration_since(since).as_secs_f32() >= 2.0 {
            let frames = FRAMES.swap(0, Ordering::Relaxed);
            eprintln!(
                "images : {:.1}/s",
                frames as f32 / now.duration_since(since).as_secs_f32()
            );
            *last = Some(now);
        }
    }

    /// Rangée : plus aucune lecture de manette. Deux fils qui interrogeaient
    /// `/dev/input` huit fois par seconde pesaient plus lourd que l'interface
    /// elle-même une fois la fenêtre fermée.
    fn suspend_input(&mut self) {
        if let Some(mut monitor) = self.pad_monitor.take() {
            monitor.stop();
        }
        if let Some(mut nav) = self.nav.take() {
            nav.stop();
        }
    }

    pub(super) fn idle_tick(&mut self, ctx: &egui::Context) {
        // Crochet de mesure : `LAPINUX_TRAY_NOW=1` ferme la fenêtre tout de
        // suite, pour vérifier que sa mémoire est bien rendue.
        if std::env::var_os("LAPINUX_TRAY_NOW").is_some() {
            self.close_window(ctx);
            return;
        }
        let minutes = self.config.general.tray_idle_minutes;
        // Une partie en cours a sa propre règle : Lapinux s'efface derrière
        // elle et revient à sa sortie. `0` désactive le rangement.
        if minutes == 0 || self.session.is_some() {
            self.idle_since = None;
            return;
        }
        if ctx.input(|input| input.focused) {
            self.idle_since = None;
            return;
        }
        let since = *self.idle_since.get_or_insert_with(Instant::now);
        if since.elapsed() >= Duration::from_secs(u64::from(minutes) * 60) {
            self.close_window(ctx);
            tracing::info!(
                "{}",
                lapinux_core::i18n::t_args(
                    "window.closed_idle",
                    &[("minutes", &minutes.to_string())]
                )
            );
        }
    }
}
