//! Session de jeu en cours, quel que soit le jeu : entrées, raccourci du menu
//! rapide, overlay, temps de jeu et arrêt. Le parcours Steam qui la prépare vit
//! dans [`crate::steam`].

use crate::{Result, Shared};
use lapinux_core::ipc::Envelope;
use lapinux_core::model::{SaveEvent, SaveEventKind, SessionInfo};
use lapinux_input::Profile;
use lapinux_perf::mangohud;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub struct SessionState {
    pub info: SessionInfo,
    pub session: Arc<Mutex<lapinux_perf::Session>>,
    /// Panneaux Plasma masqués pendant toute la partie, et non à chaque
    /// ouverture de l'overlay : changer leur mode deux fois par ouverture
    /// faisait apparaître puis disparaître la barre des tâches.
    _panels: Option<lapinux_input::panels::PanelGuard>,
}

impl Shared {
    /// Prend en charge une session préparée par le flux Steam : état, entrées,
    /// raccourci du menu rapide et surveillance de fin de processus. Le menu
    /// reste fermé jusqu'à la première pression du raccourci.
    pub(crate) fn adopt_session(
        self: &Arc<Self>,
        mut info: SessionInfo,
        session: lapinux_perf::Session,
        wants_mapping: bool,
    ) {
        // KWin lit son autorisation de capture au démarrage : le fichier doit
        // exister avant la première partie, pas seulement au premier clic sur
        // « Démarrer ».
        crate::captures::ensure_permission_file();
        // Un réveil d'écran de fin laissé par la partie précédente ne doit pas
        // faire tourner l'interface pour rien.
        let _ = std::fs::remove_file(self.paths.end_request());
        // Le tube de cadence naît avec la session et meurt avec elle.
        let stats_path = session.stats_path.clone();
        // La scène dépose ce fichier dès la sortie du jeu, avant son écran de
        // fin : la capture et le HUD doivent s'arrêter à ce moment-là.
        let scene_exit = session.scene_exit_path.clone();
        // Le journal des parties est partagé : la position de départ de cette
        // session évite de relire les marqueurs des parties précédentes.
        let log_offset = session.log_start;
        let config = self.config_snapshot();
        // KWin signale la première fenêtre du jeu. Sans KWin joignable, le
        // drapeau est posé tout de suite : l'écran d'attente s'efface comme
        // avant plutôt que de couvrir l'écran sans savoir quand s'arrêter.
        if let Err(err) = lapinux_input::shortcut::watch_game_window() {
            tracing::warn!("écran d'attente : fenêtre du jeu non suivie ({err})");
            info.window_shown = true;
        }
        // Sans splash d'interface, plus personne ne demande à KWin de placer
        // la fenêtre GameScope sur l'écran de jeu : `--prefer-output` ne suffit
        // pas en mode imbriqué. La scène garde donc ce placement, mais sans
        // voile à révéler.
        if info.scene {
            if let Err(err) =
                lapinux_input::shortcut::prepare_splash_reveal(&config.launch.output)
            {
                tracing::warn!("scène : placement de GameScope ({err})");
            }
        }
        // Les panneaux restent en autohide jusqu'à la fin de la partie :
        // l'overlay n'a plus à les toucher à chaque ouverture.
        let panels = lapinux_input::panels::PanelGuard::hide(&self.paths.data_dir);
        if let Ok(mut guard) = self.session.lock() {
            *guard = Some(SessionState {
                info: info.clone(),
                session: Arc::new(Mutex::new(session)),
                _panels: panels,
            });
        }
        if let Some(path) = scene_exit {
            let shared = Arc::clone(self);
            let started = info.started;
            std::thread::spawn(move || {
                loop {
                    if !shared
                        .session_info()
                        .is_some_and(|current| current.started == started)
                    {
                        let _ = std::fs::remove_file(&path);
                        return;
                    }
                    if path.is_file() {
                        let _ = std::fs::remove_file(&path);
                        tracing::info!("session : jeu terminé dans la scène");
                        shared.kill_overlay();
                        shared.kill_perf_hud();
                        shared.kill_record_hud();
                        shared.stop_recording_on_exit();
                        if let Ok(mut guard) = shared.session.lock() {
                            if let Some(state) = guard.as_mut() {
                                if state.info.started == started {
                                    state.info.window_gone = true;
                                }
                            }
                        }
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            });
        }
        // Un flux PipeWire peut rester muet d'une session précédente, alors
        // que le jeu et son volume sont normaux. On lève seulement la sourdine
        // du flux du jeu, sans toucher au reste du bureau. Un jeu Proton peut
        // ouvrir son flux plus tard : on retente sans retarder le HUD, qui
        // attend désormais la première image réelle du jeu.
        let shared = Arc::clone(self);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(6));
            let audio = Arc::clone(&shared);
            std::thread::spawn(move || {
                for _ in 0..10 {
                    if let Some(info) = audio.session_info() {
                        if crate::captures::ensure_game_audio(&crate::launch::descendants(
                            info.pid,
                        )) {
                            break;
                        }
                    }
                    std::thread::sleep(Duration::from_secs(1));
                }
            });
            // Filet de sécurité pour un jeu hors GameScope dont KWin n'a pas
            // signalé la fenêtre : les fonctions de HUD refusent toujours de
            // se lancer avant la première image.
            let _ = shared.spawn_perf_hud();
            let _ = shared.spawn_record_hud();
        });

        // Sous gamescope, la fenêtre n'est qu'une enveloppe : on guette la
        // première image réellement présentée dans son journal.
        if (info.gamescope || info.scene) && !info.render_started {
            let shared = Arc::clone(self);
            let log = info.log_path.clone();
            let offset = log_offset;
            let started = info.started;
            let pid = info.pid;
            let scene = info.scene;
            std::thread::spawn(move || shared.watch_render(log, offset, started, pid, scene));
        }

        // La cadence des images est relevée pendant toute la partie : elle
        // nourrit la carte d'activité et la Rich Presence. GameScope la
        // connaît, y compris en Vulkan (voir [`crate::fps`]).
        if let Some(path) = stats_path {
            let shared = Arc::clone(self);
            let started = info.started;
            let fps_cap = info.fps_cap;
            std::thread::spawn(move || shared.watch_fps(path, started, fps_cap));
        }

        // Les succès débloqués pendant la partie s'annoncent en haut de
        // l'écran : la surveillance relit le compte sans le cache de la fiche
        // et s'arrête avec la session.
        let shared = Arc::clone(self);
        let watched_appid = info.appid;
        let watched_started = info.started;
        std::thread::spawn(move || {
            shared.watch_achievement_unlocks(watched_appid, watched_started)
        });

        let wants_pad_hotkey = !config.overlay.pad_hotkey.trim().is_empty();
        if wants_mapping {
            self.start_input(&info.input_profile, true);
        } else if wants_pad_hotkey {
            self.start_input(&info.input_profile, false);
        }
        if let Err(err) = lapinux_input::shortcut::register(&config.overlay.hotkey) {
            tracing::warn!("menu rapide : {err}");
        }

        let shared = Arc::clone(self);
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_millis(500));
                // Un overlay tué brutalement ne doit pas laisser le service
                // d'entrée sans grab pour le reste de la partie.
                shared.reap_overlay();
                let finished = {
                    let guard = shared.session.lock().ok();
                    match guard.as_ref().and_then(|g| g.as_ref()) {
                        Some(state) => match state.session.lock() {
                            Ok(mut session) => matches!(session.try_wait(), Ok(Some(_))),
                            Err(_) => true,
                        },
                        None => true,
                    }
                };
                if finished {
                    break;
                }
            }
            shared.finish_session();
        });
    }

    /// KWin vient de montrer une fenêtre : si elle appartient à l'arbre du
    /// processus lancé (le jeu natif, ou son enfant Wine sous Proton), le jeu
    /// a envoyé sa première image et l'écran d'attente peut s'effacer.
    pub(crate) fn note_game_window(self: &Arc<Self>, pid: u32) {
        let mut shown = None;
        if let Ok(mut guard) = self.session.lock() {
            let Some(state) = guard.as_mut() else {
                return;
            };
            if state.info.window_shown {
                return;
            }
            if process_belongs_to(pid, state.info.pid) {
                state.info.window_shown = true;
                shown = Some(state.info.name.clone());
            }
        }
        let Some(name) = shown else {
            return;
        };
        tracing::info!("écran d'attente : première fenêtre de {name} (pid {pid})");
        // L'écran d'attente de Lapinux reste affiché jusqu'à la première image
        // du jeu : démarrer ici ferait commencer la vidéo par un logo de
        // chargement. L'enregistrement attend donc cette première image, que
        // `mark_render_started` signale (GameScope la connaît vraiment).
        let gamescope = self.session_info().is_some_and(|info| info.gamescope);
        if gamescope {
            return;
        }
        // Hors GameScope, la fenêtre est celle du jeu : un court délai laisse
        // passer un panneau de chargement avant de cadrer.
        let shared = Arc::clone(self);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(1500));
            shared.start_recording_if_wanted();
            let _ = shared.spawn_perf_hud();
            let _ = shared.spawn_record_hud();
        });
    }

    /// KWin vient de fermer une fenêtre. Si elle appartenait au jeu, et si
    /// aucune autre fenêtre du jeu ne reste, la partie est finie : l'écran de
    /// fin doit couvrir le bureau tout de suite, sans attendre que Proton ait
    /// fini de rendre la main.
    pub(crate) fn note_game_window_gone(self: &Arc<Self>, pid: u32) {
        let Some(info) = self.session_info() else {
            return;
        };
        if info.scene
            || info.window_gone
            || !info.window_shown
            || !process_belongs_to(pid, info.pid)
        {
            return;
        }
        // Au démarrage, un lanceur peut fermer sa fenêtre avant que le jeu
        // n'ouvre la sienne : on ne conclut pas pendant les dix premières
        // secondes. Un jeu qui se ferme vraiment passe ensuite par ce chemin.
        if !info.render_started
            && lapinux_core::util::now_unix() - info.started < 10
        {
            return;
        }
        let shared = Arc::clone(self);
        let started = info.started;
        // Après la première image, une fermeture est presque toujours
        // définitive : la grâce courte rend l'écran de fin immédiat. Avant
        // elle, on garde de la marge pour la fenêtre de chargement qui cède
        // sa place au jeu.
        let grace = if info.render_started {
            Duration::from_millis(350)
        } else {
            Duration::from_millis(1200)
        };
        std::thread::spawn(move || {
            std::thread::sleep(grace);
            let Some(current) = shared.session_info() else {
                return;
            };
            if current.started != started {
                return;
            }
            let pids = descendants(current.pid);
            if lapinux_input::shortcut::game_window(&pids, Duration::from_millis(300)).is_some() {
                return;
            }
            tracing::info!("session : fenêtre du jeu fermée, écran de fin");
            // Les surfaces de jeu disparaissent d'abord : l'écran de fin ne
            // doit pas hériter du HUD. Chaque attente est courte, sinon elle
            // retarderait le retour de l'interface.
            shared.kill_overlay();
            shared.kill_perf_hud();
            shared.kill_record_hud();
            // L'écran de fin se pose tout de suite. La session, elle, reste
            // ouverte jusqu'à la sortie réelle du processus : les sauvegardes
            // et l'envoi cloud ne doivent pas partir pendant que le jeu écrit
            // encore ses fichiers de fermeture.
            if let Ok(mut guard) = shared.session.lock() {
                let Some(state) = guard.as_mut() else {
                    return;
                };
                if state.info.started != started {
                    return;
                }
                state.info.window_gone = true;
            }
            // Réveil explicite : le fil de peinture d'eframe peut rester
            // endormi. L'interface lit ce fichier dans son fil de surveillance
            // et redemande une image. On ne ramène pas la fenêtre principale :
            // l'écran de fin est une fenêtre séparée, et la bibliothèque ne
            // doit pas apparaître sur l'autre écran avant lui.
            let _ = std::fs::write(shared.paths.end_request(), b"1");
            // La vidéo s'arrête avec la fenêtre du jeu ; elle est finalisée
            // après le réveil de l'interface, qui n'attend pas le mixage.
            shared.stop_recording_on_exit();
        });
    }

    /// Redonne le focus à la fenêtre du jeu, une fois l'écran d'attente
    /// disparu : sinon KWin réactive la fenêtre de Lapinux et il faut cliquer
    /// sur le jeu pour commencer à jouer.
    pub fn focus_game(&self, appid: u32) -> Result<Envelope> {
        let Some(info) = self.session_info() else {
            return Ok(Envelope::ok(
                &serde_json::json!({ "message": "aucune session" }),
            ));
        };
        if info.appid != appid {
            return Ok(Envelope::ok(
                &serde_json::json!({ "message": "session terminée" }),
            ));
        }
        let pids = descendants(info.pid);
        match lapinux_input::shortcut::focus_window(&pids) {
            Ok(()) => Ok(Envelope::ok(
                &serde_json::json!({ "message": "focus rendu au jeu" }),
            )),
            Err(err) => Ok(Envelope::err(format!("focus du jeu : {err}"))),
        }
    }

    pub fn stop_session(self: &Arc<Self>) -> Result<Envelope> {
        // Un jeu gelé par l'overlay doit pouvoir lire son SIGTERM : sinon il
        // resterait figé jusqu'au SIGKILL de secours.
        self.set_game_frozen(false);
        let Some(state) = self.take_session() else {
            return Ok(Envelope::ok(&serde_json::json!({ "message": "aucune session" })));
        };
        if let Ok(mut session) = state.session.lock() {
            session.terminate();
        }
        self.end_session(&state);
        Ok(Envelope::ok(&serde_json::json!({ "message": "session arrêtée" })))
    }

    fn take_session(&self) -> Option<SessionState> {
        self.session.lock().ok().and_then(|mut guard| guard.take())
    }

    /// Sortie de session, quel que soit le chemin (arrêt demandé ou jeu terminé).
    /// Attend la première image du jeu dans le journal de la session.
    ///
    /// Gamescope ouvre sa fenêtre avant que le jeu ne dessine : un écran noir
    /// occupe l'écran pendant que le moteur charge. Sa couche Vulkan (WSI)
    /// journalise la création de la swapchain au moment où le jeu commence
    /// vraiment à rendre ; c'est ce marqueur qui lève l'attente. Les jeux
    /// OpenGL, qui ne passent pas par cette couche, sont relâchés après un
    /// repli borné.
    fn watch_render(
        self: &Arc<Self>,
        log_path: PathBuf,
        log_offset: u64,
        started_at: i64,
        root_pid: u32,
        scene: bool,
    ) {
        use std::io::{Read, Seek, SeekFrom};
        let watch_started = std::time::Instant::now();
        let mut offset = log_offset;
        let mut text = String::new();
        let mut saw_gamescope = false;
        // Sous scène, la première swapchain du journal peut être celle du
        // splash eframe, pas celle du jeu. On ne commence à regarder qu'après
        // la ligne que la scène écrit en lançant le jeu.
        let mut scene_game_from: Option<usize> = None;
        // Guetteur XDamage des jeux OpenGL, et son signal de branchement.
        let mut xdamage: Option<std::sync::mpsc::Receiver<()>> = None;
        let mut xdamage_attached = false;
        loop {
            match self.session_info() {
                // Le journal est partagé par toutes les parties : c'est
                // l'heure de départ de la session qui l'identifie.
                Some(info) if info.started == started_at => {
                    if info.render_started {
                        return;
                    }
                }
                _ => return,
            }
            if let Ok(mut file) = std::fs::File::open(&log_path) {
                let length = file.metadata().map(|meta| meta.len()).unwrap_or(0);
                if length > offset && file.seek(SeekFrom::Start(offset)).is_ok() {
                    let mut chunk = String::new();
                    if file.by_ref().take(length - offset).read_to_string(&mut chunk).is_ok() {
                        text.push_str(&chunk);
                        offset = length;
                    }
                }
            }
            if !saw_gamescope && text.contains("[gamescope]") {
                saw_gamescope = true;
            }
            if scene && scene_game_from.is_none() {
                if let Some(position) = text.rfind("[scene] jeu lancé") {
                    scene_game_from = Some(position + "[scene] jeu lancé".len());
                }
            }
            let game_text = match scene_game_from {
                Some(position) => &text[position..],
                None if scene => "",
                None => text.as_str(),
            };
            // Pas de gamescope dans les premières secondes : la fenêtre du
            // jeu est la sienne, KWin suffit à décider.
            if !saw_gamescope && !scene && watch_started.elapsed() > Duration::from_secs(2) {
                self.mark_render_started(started_at);
                return;
            }
            // `Created swapchain` est écrit par la couche WSI de gamescope au
            // premier rendu Vulkan du jeu.
            if game_text.contains("Created swapchain")
                || game_text.contains("Creating swapchain")
            {
                self.mark_render_started(started_at);
                return;
            }
            // Les jeux OpenGL n'écrivent pas de swapchain : leur première
            // image se lit sur l'affichage Xwayland de gamescope, via XDamage.
            if xdamage.is_none() && (!scene || scene_game_from.is_some()) {
                if let Some(display) = crate::frames::xwayland_display(&text) {
                    let (tx, rx) = std::sync::mpsc::channel();
                    let shared = Arc::clone(self);
                    std::thread::spawn(move || {
                        let belongs = |pid: u32| process_belongs_to(pid, root_pid);
                        let alive = || {
                            shared.session_info().is_some_and(|info| {
                                info.started == started_at && !info.render_started
                            })
                        };
                        match crate::frames::watch_first_frame(&display, &belongs, &alive, tx)
                        {
                            Ok(true) => shared.mark_render_started(started_at),
                            Ok(false) => {}
                            Err(err) => tracing::debug!("première image X11 : {err}"),
                        }
                    });
                    xdamage = Some(rx);
                }
            }
            if let Some(rx) = xdamage.as_ref() {
                if rx.try_recv().is_ok() {
                    xdamage_attached = true;
                }
            }
            // Branchement réussi : le guetteur X11 décide seul, sans repli
            // arbitraire qui couperait un long chargement. Sinon, l'ancien
            // filet de sécurité reste court.
            let timeout = if xdamage_attached {
                Duration::from_secs(120)
            } else {
                Duration::from_secs(25)
            };
            if watch_started.elapsed() > timeout {
                self.mark_render_started(started_at);
                return;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    /// Tient les bornes de cadence de la session à jour pendant la partie.
    fn watch_fps(self: &Arc<Self>, stats_path: PathBuf, started: i64, fps_cap: u32) {
        let alive_owner = Arc::clone(self);
        let alive = || alive_owner.session_info().is_some_and(|info| info.started == started);
        let report = |stats: lapinux_core::model::FpsStats, series: &[u32], from: i64, to: i64| {
            if let Ok(mut guard) = self.session.lock() {
                if let Some(state) = guard.as_mut() {
                    if state.info.started == started {
                        state.info.fps = Some(stats);
                        state.info.fps_series = series.to_vec();
                        state.info.fps_series_start = from;
                        state.info.fps_series_end = to;
                    }
                }
            }
        };
        crate::fps::watch_stats(&stats_path, &alive, &report, fps_cap);
    }

    /// Taille de l'image du jeu pour placer le HUD : la résolution de
    /// lancement, celle que GameScope compose.
    fn screen_size(&self) -> (u32, u32) {
        let config = self.config_snapshot();
        if config.launch.width >= 320 && config.launch.height >= 240 {
            (config.launch.width, config.launch.height)
        } else {
            (1920, 1080)
        }
    }

    fn mark_render_started(self: &Arc<Self>, started_at: i64) {
        // La plaque n'entre qu'une fois le jeu à l'écran : au lancement, elle
        // apparaissait par-dessus l'écran d'attente.
        let shared = Arc::clone(self);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(700));
            let _ = shared.spawn_perf_hud();
            let _ = shared.spawn_record_hud();
        });
        if let Ok(mut guard) = self.session.lock() {
            let Some(state) = guard.as_mut() else {
                return;
            };
            if state.info.started != started_at || state.info.render_started {
                return;
            }
            state.info.render_started = true;
            tracing::info!(
                "écran d'attente : première image de {} présentée",
                state.info.name
            );
        }
        // Filet de sécurité : certains jeux n'annoncent aucune fenêtre à KWin.
        // La première image est alors le seul signal de départ pour
        // l'enregistrement, qui ne part qu'une fois par partie.
        self.start_recording_if_wanted();
        // La limite choisie est reposée dès que GameScope répond : l'argument
        // `--framerate-limit` divise la cadence du compositeur et peut rester
        // sans effet selon la version et l'écran.
        if let Some(info) = self.session_info() {
            self.apply_launch_fps_limit(&info);
        }
    }

    /// Applique la limite de FPS de la session une fois GameScope en place.
    ///
    /// C'est le même chemin que le panneau Performances : `debug_set_fps_limit`
    /// agit sur le compositeur déjà lancé, sans dépendre du calcul de `-r` ni
    /// de la façon dont GameScope divise sa cadence au démarrage.
    fn apply_launch_fps_limit(self: &Arc<Self>, info: &SessionInfo) {
        if !info.gamescope || info.fps_cap == 0 {
            return;
        }
        let target = info.clone();
        let shared = Arc::clone(self);
        std::thread::spawn(move || {
            for attempt in 0..12 {
                std::thread::sleep(Duration::from_millis(if attempt == 0 { 400 } else { 300 }));
                let alive = shared
                    .session_info()
                    .is_some_and(|current| current.started == target.started);
                if !alive {
                    return;
                }
                match set_gamescope_fps_limit(&target, target.fps_cap) {
                    Ok(()) => {
                        tracing::info!(
                            "limite FPS {} : appliquée au lancement",
                            target.fps_cap
                        );
                        return;
                    }
                    Err(err) if attempt == 11 => tracing::warn!(
                        "limite FPS {} au lancement : {err}",
                        target.fps_cap
                    ),
                    Err(_) => {}
                }
            }
        });
    }

    fn end_session(self: &Arc<Self>, state: &SessionState) {
        // Le jeu n'est plus en cours : l'état « en jeu » quitte Steam avant
        // les opérations de sauvegarde, qui peuvent prendre plusieurs secondes.
        self.set_steam_playing(None);
        // Wine laisse `wineserver` et ses `winedevice.exe` dans le préfixe, et
        // GameScope son `gamescopereaper` : détachés de la session, ils
        // survivent à la partie et le lancement suivant peut rester noir.
        if let Ok(session) = state.session.lock() {
            session.reap_leftovers();
        }
        // Même sans signal KWin (processus tué, fenêtre déjà disparue), le fil
        // de l'interface doit se réveiller pour poser l'écran de fin.
        if !state.info.scene {
            let _ = std::fs::write(self.paths.end_request(), b"1");
        }
        // Les surfaces de jeu disparaissent avec lui : on les retire avant la
        // finalisation de la vidéo, qui peut prendre plusieurs secondes. Leur
        // laisser la vie sauve jusqu'au mixage donnait le HUD encore affiché
        // après le retour au bureau.
        self.kill_overlay();
        self.kill_perf_hud();
        self.kill_record_hud();
        // La bibliothèque reste rangée : l'écran de fin est une fenêtre
        // séparée, et la ramener ici la ferait apparaître sur DP-1 avant que
        // le voile de fin ne soit posé. L'interface la révélera elle-même à la
        // fermeture de l'écran de fin.
        // La vidéo se ferme avec le jeu : le fichier est gardé tel quel.
        self.stop_recording_on_exit();
        lapinux_input::shortcut::unwatch_game_window();
        lapinux_input::shortcut::unregister();
        self.record_playtime(&state.info);
        // Le cloud de la boutique garde-t-il cette partie ? Réponse donnée
        // par la synchronisation ci-dessous, ou par le relevé du lancement.
        let mut cloud_saves = false;
        // Les sauvegardes de la partie rejoignent le cloud du compte : sans
        // client Steam, personne d'autre ne le ferait. Le relevé pris au
        // lancement sert de témoin — si une autre machine a sauvegardé
        // entre-temps, rien n'est envoyé.
        if let Ok(Some(game)) = self.store.game(state.info.appid) {
            if !game.settings.cloud_sync_enabled() {
                tracing::debug!(
                    "cloud : synchronisation automatique désactivée pour {}",
                    state.info.appid
                );
            } else if game.store == lapinux_core::stores::Store::Steam {
                cloud_saves = self
                    .cloud_before
                    .lock()
                    .ok()
                    .and_then(|slot| {
                        slot.as_ref()
                            .filter(|(appid, _)| *appid == state.info.appid)
                            .map(|(_, saves)| !saves.files.is_empty())
                    })
                    .unwrap_or(false);
                match self.sync_cloud_up(&game, state.info.appid) {
                Ok((files, bytes, diff, present)) => {
                    cloud_saves |= present;
                    if let Some(diff) = diff {
                        if let Ok(mut slot) = self.cloud_last.lock() {
                            *slot = Some((state.info.appid, diff));
                        }
                    }
                    if let Ok(mut cache) = self.game_reads.lock() {
                        cache.forget("cloud", state.info.appid);
                    }
                    self.forget_remote_sheet("cloud", state.info.appid);
                    if files > 0 {
                        tracing::info!(
                            "cloud : {files} sauvegarde(s) envoyée(s) après la partie de {} ({bytes} octets)",
                            state.info.appid
                        );
                    }
                }
                    Err(error) => tracing::warn!(
                        "cloud : envoi après la partie de {} ({error})",
                        state.info.appid
                    ),
                }
            } else if game.settings.save_dir.is_some() {
                // Jeu de boutique : seul ce que la partie a modifié repart,
                // vers le cloud de sa boutique et non vers Steam.
                match self.store_cloud_push_auto(state.info.appid) {
                    Ok((files, bytes, present)) => {
                        cloud_saves |= present;
                        if files > 0 {
                            tracing::info!(
                                "cloud {} : {files} sauvegarde(s) envoyée(s) après la partie de {} ({bytes} octets)",
                                game.store.label(),
                                state.info.appid
                            );
                        }
                    }
                    Err(error) => tracing::warn!(
                        "cloud {} : envoi après la partie de {} ({error})",
                        game.store.label(),
                        state.info.appid
                    ),
                }
            }
        }
        // Les sauvegardes locales se prennent après l'envoi cloud, pour
        // copier l'état final : c'est le filet des jeux sans cloud. Un jeu
        // déjà sauvegardé en ligne n'en a pas besoin, sauf si le réglage
        // demande une copie locale pour tous.
        if let Ok(Some(game)) = self.store.game(state.info.appid) {
            let config = self.config_snapshot();
            if config.general.saves_auto
                && config.general.saves_skip_cloud
                && cloud_saves
            {
                tracing::info!(
                    "sauvegardes de {} : déjà dans le cloud, copie locale ignorée",
                    state.info.name
                );
            } else if config.general.saves_auto {
                match crate::saves::backup(&game, &self.paths, &config) {
                    Ok(Some(snapshot)) => {
                        // Le journal garde la copie automatique : la fiche
                        // montre ce qui a été fait sans attendre un clic.
                        crate::saves::note(
                            &self.store,
                            state.info.appid,
                            SaveEvent {
                                at: lapinux_core::util::now_unix(),
                                kind: SaveEventKind::Backup,
                                snapshot: snapshot.id.clone(),
                                files: snapshot.files,
                                bytes: snapshot.bytes,
                                detail: snapshot.source.clone(),
                                auto: true,
                                imported: false,
                            },
                        );
                        tracing::info!(
                            "sauvegardes de {} copiées localement ({} fichiers, copie {})",
                            state.info.name,
                            snapshot.files,
                            snapshot.id
                        );
                    }
                    Ok(None) => {}
                    Err(error) => tracing::warn!(
                        "copie locale des sauvegardes de {} ({error})",
                        state.info.name
                    ),
                }
            }
        }
        // Le cloud a pu changer pendant la partie : la fiche le relira et
        // comparera avec le relevé pris au lancement.
        if let Ok(mut cache) = self.game_reads.lock() {
            cache.forget("cloud", state.info.appid);
        }
        self.forget_remote_sheet("cloud", state.info.appid);
    }

    /// Retire un overlay terminé et rend les manettes au service d'entrée.
    fn reap_overlay(&self) {
        let mut ended = false;
        let mut present = false;
        if let Ok(mut guard) = self.overlay.lock() {
            if let Some(child) = guard.as_mut() {
                present = true;
                match child.try_wait() {
                    Ok(Some(_)) | Err(_) => {
                        *guard = None;
                        ended = true;
                    }
                    Ok(None) => {}
                }
            }
        }
        if ended || !present {
            lapinux_input::service::set_overlay_navigation(false);
            // L'overlay a disparu : le jeu doit repartir, même s'il a été tué
            // par un crash et non par le raccourci.
            self.set_game_frozen(false);
            self.close_nav_tube();
            // Le menu s'est refermé de lui-même (« Reprendre ») : la capture
            // repart sans attendre un nouveau raccourci.
            self.record_resume();
        }
        if ended {
            if let Ok(mut guard) = self.session.lock() {
                if let Some(state) = guard.as_mut() {
                    state.info.overlay_open = false;
                }
            }
        }
    }

    pub(crate) fn finish_session(self: &Arc<Self>) {
        self.set_game_frozen(false);
        if let Some(state) = self.take_session() {
            self.end_session(&state);
        }
    }

    /// Gèle ou relance le groupe de processus du jeu. L'overlay est un
    /// processus séparé : il garde la main sur l'écran et la manette pendant
    /// que le jeu ne reçoit plus aucune image ni aucun événement.
    fn set_game_frozen(&self, frozen: bool) {
        let Some(info) = self.session_info() else {
            return;
        };
        if info.pid <= 1 {
            return;
        }
        let signal = if frozen { libc::SIGSTOP } else { libc::SIGCONT };
        unsafe {
            libc::kill(-(info.pid as libc::pid_t), signal);
        }
    }

    fn record_playtime(&self, info: &SessionInfo) {
        let ended = lapinux_core::util::now_unix();
        let played = (ended - info.started).max(0) as u64;
        // L'historique garde la partie elle-même : la page Activité montre les
        // parties terminées en cartes, pas seulement un total par jeu.
        if let Ok(mut stored) = self
            .store
            .get_json::<Vec<lapinux_core::model::PlaySession>>("sessions")
        {
            let mut sessions = stored.take().unwrap_or_default();
            sessions.push(lapinux_core::model::PlaySession {
                appid: info.appid,
                name: info.name.clone(),
                started: info.started,
                ended,
                seconds: played,
                // Le journal de la session reste sur le disque : la carte
                // d'activité le déplie au lieu de le perdre.
                log: info.log_path.to_string_lossy().to_string(),
                // Les bornes de cadence relevées pendant la partie : une
                // partie antérieure à la mesure n'en a pas.
                fps: info.fps.filter(|stats| !stats.is_empty()),
                fps_series: info.fps_series.clone(),
                fps_series_start: info.fps_series_start,
                fps_series_end: info.fps_series_end,
            });
            // Les plus anciennes sortent : la page n'a pas à porter des années
            // de parties, et le fichier garde une taille bornée.
            if sessions.len() > 200 {
                let excess = sessions.len() - 200;
                sessions.drain(0..excess);
            }
            let _ = self.store.set_json("sessions", &sessions);
        }
        if let Ok(Some(mut game)) = self.store.game(info.appid) {
            game.playtime_secs += played;
            game.last_played = Some(lapinux_core::util::now_unix());
            let _ = self.store.save_game(&game);
        }
        if let Ok(mut stats) = self.store.get_json::<lapinux_core::model::Stats>("stats") {
            let mut stats = stats.take().unwrap_or_default();
            stats.launches += 1;
            stats.total_seconds += played;
            let _ = self.store.set_json("stats", &stats);
        }
    }

    pub fn spawn_overlay(self: &Arc<Self>) -> Result<()> {
        // Un overlay précédent a pu être tué sans rendre les panneaux.
        lapinux_input::panels::restore_stale(&self.paths.data_dir);
        let Some(info) = self.session_info() else {
            return Ok(());
        };
        if let Ok(mut guard) = self.overlay.lock() {
            if let Some(child) = guard.as_mut() {
                if matches!(child.try_wait(), Ok(None)) {
                    return Ok(());
                }
            }
        }
        let exe = overlay_exe()?;
        let config = self.config_snapshot();
        let mut command = Command::new(exe);
        command.arg("overlay");
        // La plaque de performance a son propre processus : le menu ne la
        // redessine pas, sinon deux plaques se superposeraient.
        command
            .arg("--game")
            .arg(&info.name)
            .arg("--fps")
            .arg(info.fps_cap.to_string())
            .arg("--profile")
            .arg(&info.input_profile)
            .arg("--profiles")
            .arg(
                lapinux_input::list_profiles(&self.paths.data_dir)
                    .into_iter()
                    .map(|profile| profile.name)
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
            .arg("--proton")
            .arg(&info.proton)
            .arg("--width")
            .arg(config.launch.width.to_string())
            .arg("--height")
            .arg(config.launch.height.to_string())
            .arg("--perf-hud-scale")
            .arg(config.capture.hud_scale.to_string())
            .arg("--hud-engine")
            // Le moteur affiché est celui de la session en cours, pas le
            // réglage global : un jeu qui a choisi la plaque native ne doit
            // pas annoncer MangoHud parce qu'un autre jeu l'utilise.
            .arg(if info.mangohud { "mangohud" } else { "native" })
            .arg("--hud")
            .arg(if info.hud_preset.is_empty() {
                "off"
            } else {
                info.hud_preset.as_str()
            });
        // Le profil peut rester enregistré quand MangoHud est coupé : le
        // drapeau dit l'état réel, indépendamment du nom du profil.
        if info.mangohud {
            command.arg("--hud-on");
        }
        // La plaque native est un autre processus, mais le panneau doit
        // annoncer son état réel : sans ce drapeau, l'interrupteur paraissait
        // coupé alors que la plaque était bien affichée depuis le lancement.
        // Le drapeau ne fait que refléter l'état : `hud_only` empêche ce
        // processus de redessiner la plaque par-dessus celle du HUD.
        if config.capture.hud_on_screen && !info.mangohud {
            command.arg("--perf-hud");
        }
        // La manette passe par le service d'entrée, qui connaît déjà ses
        // boutons : l'overlay se contente de lire ce tube.
        let nav_path = self.create_nav_tube(info.appid);
        if let Some(path) = nav_path.as_ref() {
            command.arg("--nav-fifo").arg(path);
        }
        command
            .arg("--hud-position")
            .arg(if info.hud_position.is_empty() {
                "top-left"
            } else {
                info.hud_position.as_str()
            });
        command
            .arg("--hud-layout")
            .arg(if info.hud_layout.is_empty() {
                "block"
            } else {
                info.hud_layout.as_str()
            });
        if info.hud_graph {
            command.arg("--hud-graph");
        }
        if info.hud_cores {
            command.arg("--hud-cores");
        }
        command
            .arg("--record-badge-position")
            .arg(&config.capture.record_badge_position)
            .arg("--record-badge-scale")
            .arg(config.capture.record_badge_scale.to_string());
        if config.capture.record_badge {
            command.arg("--record-badge");
        }
        command
            .arg("--appid")
            .arg(info.appid.to_string())
            .arg("--captures")
            .arg(config.capture.videos_dir());
        if !config.launch.output.trim().is_empty() {
            command
                .arg("--output")
                .arg(config.launch.output.trim());
        }
        // Visuel de tête du panneau : la jaquette déjà en cache, sans réseau.
        let art = self.paths.art_dir();
        let covers = self.paths.covers_dir();
        // La bannière large passe en premier : la petite capsule 231 px
        // pixeliserait une fois agrandie dans la carte de tête.
        let capsule = [
            covers.join(format!("custom-{}.jpg", info.appid)),
            art.join(format!("sgdb-hero-{}.jpg", info.appid)),
            art.join(format!("hero-{}.jpg", info.appid)),
            art.join(format!("steam-capsule-{}.jpg", info.appid)),
            art.join(format!("capsule-{}.jpg", info.appid)),
            covers.join(format!("{}.jpg", info.appid)),
            covers.join(format!("store-{}.jpg", info.appid)),
        ]
        .into_iter()
        .find(|path| path.is_file());
        if let Some(path) = capsule {
            command.arg("--capsule").arg(path);
        }
        // Icône carrée du jeu : elle remplace le mot « PERFORMANCE » dans
        // l'en-tête de la plaque. Le cache d'illustrations la garde déjà.
        let icon = [
            art.join(format!("steam-icon-{}.jpg", info.appid)),
            art.join(format!("icon-{}.jpg", info.appid)),
            art.join(format!("steam-icon-{}.png", info.appid)),
        ]
        .into_iter()
        .find(|path| path.is_file());
        if let Some(path) = icon {
            command.arg("--icon").arg(path);
        }
        let log = crate::logs::open_shared(&self.paths, "overlay.log", "menu rapide")
            .map_err(lapinux_core::Error::Io)?;
        let log_err = log.try_clone().map_err(lapinux_core::Error::Io)?;
        let child = command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::from(log))
            .stderr(std::process::Stdio::from(log_err))
            .spawn()
            .map_err(|e| lapinux_core::Error::Other(format!("overlay : {e}")))?;
        if let Ok(mut guard) = self.overlay.lock() {
            *guard = Some(child);
        }
        // Le tube n'est relié qu'une fois l'enfant publié : le veilleur
        // peut passer entre-temps, il fermerait un tube encore sans lecteur.
        if let Some(path) = nav_path.as_ref() {
            self.attach_nav_tube(path);
        }
        // L'overlay lit la manette pour sa navigation : le service d'entrée
        // relâche ses grabs pendant ce temps. Après avoir publié l'enfant,
        // pour que le veilleur ne prenne pas ce court instant pour une sortie.
        lapinux_input::service::set_overlay_navigation(true);
        if let Ok(mut guard) = self.session.lock() {
            if let Some(state) = guard.as_mut() {
                state.info.overlay_open = true;
            }
        }
        // Le jeu est figé tant que le menu est ouvert : plus d'images, plus
        // d'entrées, plus de calcul. L'overlay, lui, vit dans son propre
        // groupe de processus.
        self.set_game_frozen(true);
        // Menu ouvert : la capture s'arrête. Rien n'est écrit pendant ce
        // temps, donc la pause ne laisse pas de trou dans la vidéo.
        self.record_pause();
        Ok(())
    }

    /// Plaque de performance persistante : un overlay sans menu, sans clavier
    /// ni souris, qui vit toute la partie. Sans lui, la plaque n'existait que
    /// pendant l'ouverture du menu rapide — c'est-à-dire jamais à l'écran.
    fn spawn_perf_hud(self: &Arc<Self>) -> Result<()> {
        let Some(info) = self.session_info() else {
            return Ok(());
        };
        let config = self.config_snapshot();
        // Ce processus ne dessine que la plaque de chiffres. Le badge
        // d'enregistrement a le sien (`spawn_record_hud`) : couper l'un ne
        // touche pas l'autre. Un jeu qui a MangoHud ne reçoit jamais la plaque
        // native en plus, même si l'interrupteur global est resté allumé.
        // Le mode performance coupe aussi cette plaque, sans toucher aux
        // réglages globaux.
        if info.performance || !config.capture.hud_on_screen || info.mangohud {
            return Ok(());
        }
        // La plaque ne doit apparaître qu'une fois le jeu réellement à
        // l'écran : sous GameScope, sa fenêtre existe bien avant la première
        // image et la plaque surgissait par-dessus l'écran d'attente. On se
        // fie au rendu, pas à la fenêtre : KWin peut manquer le signal.
        if !info.render_started || (!info.gamescope && !info.window_shown) {
            return Ok(());
        }
        if let Ok(mut guard) = self.perf_hud.lock() {
            if let Some(child) = guard.as_mut() {
                if matches!(child.try_wait(), Ok(None)) {
                    return Ok(());
                }
            }
        }
        let exe = overlay_exe()?;
        let log = crate::logs::open_shared(&self.paths, "overlay.log", "plaque de performance")
            .map_err(lapinux_core::Error::Io)?;
        // La plaque reprend les réglages du panneau Performances : profil,
        // ancrage, disposition, courbe et barres par cœur. Ils arrivent aussi
        // à chaud par la session, une fois la partie lancée.
        let mut command = Command::new(exe);
        command.arg("overlay").arg("--hud-only").arg("--perf-hud");
        command
            .arg("--game")
            .arg(&info.name)
            .arg("--fps")
            .arg(info.fps_cap.to_string())
            .arg("--width")
            .arg(config.launch.width.to_string())
            .arg("--height")
            .arg(config.launch.height.to_string())
            .arg("--perf-hud-scale")
            .arg(config.capture.hud_scale.to_string())
            .arg("--hud")
            .arg(if info.hud_preset.is_empty() {
                "essential"
            } else {
                info.hud_preset.as_str()
            })
            .arg("--hud-position")
            .arg(if info.hud_position.is_empty() {
                "top-left"
            } else {
                info.hud_position.as_str()
            })
            .arg("--hud-layout")
            .arg(if info.hud_layout.is_empty() {
                "block"
            } else {
                info.hud_layout.as_str()
            })
            .arg("--appid")
            .arg(info.appid.to_string());
        if let Some(output) = hud_output(&info, &config.launch.output) {
            command.arg("--output").arg(output);
        }
        if info.hud_graph {
            command.arg("--hud-graph");
        }
        if info.hud_cores {
            command.arg("--hud-cores");
        }
        let child = command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::from(log))
            .spawn()
            .map_err(|e| lapinux_core::Error::Other(format!("plaque de perf : {e}")))?;
        if let Ok(mut guard) = self.perf_hud.lock() {
            *guard = Some(child);
        }
        tracing::info!("HUD hors du jeu : plaque affichée");
        Ok(())
    }

    /// Badge d'enregistrement : sa propre surface, son propre processus. Il
    /// ne partage ni la vie ni les réglages de la plaque de performance.
    fn spawn_record_hud(self: &Arc<Self>) -> Result<()> {
        let Some(info) = self.session_info() else {
            return Ok(());
        };
        let config = self.config_snapshot();
        if !config.capture.record_badge {
            return Ok(());
        }
        if !info.render_started || (!info.gamescope && !info.window_shown) {
            return Ok(());
        }
        if let Ok(mut guard) = self.record_hud.lock() {
            if let Some(child) = guard.as_mut() {
                if matches!(child.try_wait(), Ok(None)) {
                    return Ok(());
                }
            }
        }
        let exe = overlay_exe()?;
        let log = crate::logs::open_shared(&self.paths, "overlay.log", "badge d'enregistrement")
            .map_err(lapinux_core::Error::Io)?;
        let mut command = Command::new(exe);
        command
            .arg("overlay")
            .arg("--hud-only")
            .arg("--record-badge")
            .arg("--record-badge-position")
            .arg(&config.capture.record_badge_position)
            .arg("--record-badge-scale")
            .arg(config.capture.record_badge_scale.to_string())
            .arg("--game")
            .arg(&info.name)
            .arg("--width")
            .arg(config.launch.width.to_string())
            .arg("--height")
            .arg(config.launch.height.to_string())
            .arg("--appid")
            .arg(info.appid.to_string());
        if let Some(output) = hud_output(&info, &config.launch.output) {
            command.arg("--output").arg(output);
        }
        let child = command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::from(log))
            .spawn()
            .map_err(|e| lapinux_core::Error::Other(format!("HUD d'enregistrement : {e}")))?;
        if let Ok(mut guard) = self.record_hud.lock() {
            *guard = Some(child);
        }
        tracing::info!("HUD d'enregistrement : badge REC affiché");
        Ok(())
    }

    /// Ferme le badge d'enregistrement sans toucher à la plaque de chiffres.
    fn kill_record_hud(&self) {
        if let Ok(mut guard) = self.record_hud.lock() {
            if let Some(mut child) = guard.take() {
                unsafe {
                    libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
                }
                let deadline = std::time::Instant::now() + Duration::from_millis(120);
                loop {
                    match child.try_wait() {
                        Ok(Some(_)) => break,
                        _ if std::time::Instant::now() >= deadline => {
                            let _ = child.kill();
                            let _ = child.wait();
                            break;
                        }
                        _ => std::thread::sleep(Duration::from_millis(10)),
                    }
                }
            }
        }
    }

    /// Arrête les surfaces HUD laissées par un daemon précédent.
    ///
    /// Un enfant survit à la mort de son parent : après un plantage ou un
    /// redémarrage, la plaque de performance et le badge REC continuaient de
    /// se dessiner sur le bureau, sans jeu et sans moyen de les fermer depuis
    /// l'overlay. Au démarrage, aucune partie n'est en cours : tout processus
    /// `--hud-only` encore vivant est un reste.
    pub(crate) fn reap_stale_hud() {
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return;
        };
        let me = std::process::id();
        for entry in entries.flatten() {
            let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
                continue;
            };
            if pid == me {
                continue;
            }
            let Ok(raw) = std::fs::read(entry.path().join("cmdline")) else {
                continue;
            };
            let args: Vec<String> = raw
                .split(|byte| *byte == 0)
                .filter(|part| !part.is_empty())
                .map(|part| String::from_utf8_lossy(part).into_owned())
                .collect();
            // Le binaire doit être le nôtre : un autre outil qui parle de
            // `--hud-only` ne doit pas être touché.
            let ours = args.first().is_some_and(|arg| {
                arg == "lapinux" || arg.ends_with("/lapinux") || arg.ends_with("\\lapinux")
            });
            let hud = args.iter().any(|arg| arg == "overlay")
                && args.iter().any(|arg| arg == "--hud-only");
            if ours && hud {
                unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
                tracing::info!("HUD orphelin arrêté ({pid})");
            }
        }
    }

    /// Ferme la plaque de performance : le jeu repart sans elle.
    fn kill_perf_hud(&self) {
        if let Ok(mut guard) = self.perf_hud.lock() {
            if let Some(mut child) = guard.take() {
                unsafe {
                    libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
                }
                let deadline = std::time::Instant::now() + Duration::from_millis(120);
                loop {
                    match child.try_wait() {
                        Ok(Some(_)) => break,
                        _ if std::time::Instant::now() >= deadline => {
                            let _ = child.kill();
                            let _ = child.wait();
                            break;
                        }
                        _ => std::thread::sleep(Duration::from_millis(20)),
                    }
                }
            }
        }
    }

    /// Crée le tube par lequel le service d'entrée poussera les appuis de la
    /// manette à l'overlay. Il est relié après le lancement de l'overlay.
    fn create_nav_tube(&self, appid: u32) -> Option<PathBuf> {
        self.close_nav_tube();
        let dir = &self.paths.log_dir;
        std::fs::create_dir_all(dir).ok()?;
        let path = dir.join(format!("nav-{appid}.fifo"));
        let _ = std::fs::remove_file(&path);
        let c_path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).ok()?;
        if unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) } != 0 {
            tracing::warn!("tube manette : {}", std::io::Error::last_os_error());
            return None;
        }
        Some(path)
    }

    /// Ouvre l'extrémité d'écriture du tube, en lecture *et* écriture : elle
    /// ne bloque jamais, même si l'overlay n'a pas encore ouvert la sienne.
    fn attach_nav_tube(&self, path: &std::path::Path) {
        use std::os::unix::fs::OpenOptionsExt;
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)
        {
            Ok(file) => {
                if let Ok(mut guard) = self.nav_out.lock() {
                    *guard = Some(file);
                }
            }
            Err(err) => tracing::warn!("tube manette : {err}"),
        }
    }

    fn close_nav_tube(&self) {
        if let Ok(mut guard) = self.nav_out.lock() {
            guard.take();
        }
    }

    /// Transmet un appui au menu ouvert. Sans lecteur, l'écriture dans le tube
    /// est simplement perdue : le jeu n'en dépend pas.
    fn send_overlay_nav(&self, nav: lapinux_input::Nav) {
        use std::io::Write;
        if let Ok(mut guard) = self.nav_out.lock() {
            if let Some(file) = guard.as_mut() {
                let _ = file.write_all(&[nav.wire_byte()]);
            }
        }
    }

    fn kill_overlay(&self) {
        if let Ok(mut guard) = self.overlay.lock() {
            if let Some(mut child) = guard.take() {
                // SIGTERM laisse l'overlay jouer sa sortie animée ; SIGKILL
                // seulement s'il ne répond pas, pour ne jamais bloquer le
                // raccourci du menu.
                unsafe {
                    libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
                }
                let deadline = std::time::Instant::now() + Duration::from_millis(120);
                loop {
                    match child.try_wait() {
                        Ok(Some(_)) => break,
                        _ if std::time::Instant::now() >= deadline => {
                            let _ = child.kill();
                            let _ = child.wait();
                            break;
                        }
                        _ => std::thread::sleep(Duration::from_millis(10)),
                    }
                }
            }
        }
        if let Ok(mut guard) = self.session.lock() {
            if let Some(state) = guard.as_mut() {
                state.info.overlay_open = false;
            }
        }
        // Le service d'entrée reprend les manettes et leur remapping, et le
        // jeu repart exactement où il s'était arrêté.
        lapinux_input::service::set_overlay_navigation(false);
        self.set_game_frozen(false);
        self.close_nav_tube();
        // Menu refermé : l'enregistrement reprend là où il s'était arrêté.
        self.record_resume();
        // L'overlay a pu être tué par le raccourci : ses panneaux Plasma sont
        // alors restés masqués. Le fichier d'état rend le mode d'origine.
        lapinux_input::panels::restore_stale(&self.paths.data_dir);
    }

    pub fn toggle_overlay(self: &Arc<Self>) -> Result<Envelope> {
        let running = {
            let mut guard = self.overlay.lock().map_err(|_| {
                lapinux_core::Error::Other("verrou overlay indisponible".into())
            })?;
            match guard.as_mut() {
                Some(child) => match child.try_wait() {
                    Ok(None) => true,
                    _ => {
                        *guard = None;
                        false
                    }
                },
                None => false,
            }
        };
        if running {
            self.kill_overlay();
            Ok(Envelope::ok(&serde_json::json!({ "overlay": "fermé" })))
        } else {
            self.spawn_overlay()?;
            Ok(Envelope::ok(&serde_json::json!({ "overlay": "ouvert" })))
        }
    }

    fn start_input(self: &Arc<Self>, profile_name: &str, remap: bool) {
        let config = self.config_snapshot();
        let mut profile = lapinux_input::load_profile(&self.paths.data_dir, profile_name)
            .unwrap_or_default();
        profile.deadzone = config.input.deadzone;
        profile.curve = config.input.stick_curve;
        profile.hotkey_chord = split_chord(&config.overlay.pad_hotkey);
        let started = if remap {
            lapinux_input::InputService::start(profile, config.input.grab_devices)
        } else {
            lapinux_input::InputService::start_hotkeys(profile)
        };
        let Ok(mut service) = started else {
            return;
        };
        // Le service vit avec la session : le thread s'arrête quand elle se termine
        // et libère immédiatement le grab des manettes.
        let shared = Arc::clone(self);
        std::thread::spawn(move || {
            loop {
                match service.events.recv_timeout(Duration::from_millis(200)) {
                    Ok(lapinux_input::InputMsg::Hotkey) => {
                        let _ = shared.toggle_overlay();
                    }
                    Ok(lapinux_input::InputMsg::Nav(nav)) => {
                        shared.send_overlay_nav(nav);
                    }
                    Ok(_) => {}
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        if shared.session_info().is_none() {
                            break;
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
            service.stop();
        });
    }

    /// Masque ou remontre le HUD du jeu en réécrivant sa configuration
    /// MangoHud puis en la faisant recharger.
    ///
    /// `mangohudctl` ne joint pas toujours un HUD lancé dans un bac à sable :
    /// ce fichier-ci est celui que MangoHud relit à chaud, et c'est donc le
    /// seul chemin fiable pour retirer le HUD de la vidéo.
    pub(crate) fn set_game_hud(&self, visible: bool) -> bool {
        let Some(info) = self.session_info() else {
            return false;
        };
        let profile = if info.hud_preset.trim().is_empty() {
            "full".to_string()
        } else {
            info.hud_preset.clone()
        };
        let mut settings = mangohud::preset_settings_at(
            &profile,
            Some(info.fps_cap),
            &info.hud_position,
        );
        settings.enabled = visible && info.mangohud;
        settings.horizontal = layout_is_inline(&info.hud_layout);
        settings.frame_timing = info.hud_graph;
        settings.cpu_cores = info.hud_cores;
        settings.screen = Some(self.screen_size());
        if mangohud::write_config(&self.paths.config_dir, info.appid, &settings).is_err() {
            return false;
        }
        if let Some(ctl) = lapinux_core::paths::which("mangohudctl") {
            let _ = Command::new(ctl)
                .args(["set", "reload_config", "true"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
        true
    }

    /// Relit ou applique le volume du flux audio de la partie en cours.
    /// L'overlay s'en sert pour son curseur ; le menu rapide pour le geste
    /// immédiat.
    pub fn game_volume(&self, volume: Option<f32>) -> Result<Envelope> {
        let Some(info) = self.session_info() else {
            return Ok(Envelope::err("aucun jeu en cours"));
        };
        let pids = descendants(info.pid);
        match volume {
            Some(volume) => match crate::captures::set_game_volume(&pids, volume) {
                Ok(value) => Ok(Envelope::ok(&serde_json::json!({ "volume": value }))),
                Err(error) => Ok(Envelope::err(error)),
            },
            None => Ok(Envelope::ok(&serde_json::json!({
                "volume": crate::captures::game_volume(&pids),
            }))),
        }
    }

    pub fn overlay_action(self: &Arc<Self>, action: &str) -> Result<Envelope> {
        let mut message = String::new();
        // Le moteur du HUD se choisit depuis Performances. La plaque native
        // peut être relancée tout de suite ; MangoHud, lui, est injecté au
        // lancement du jeu et attend la partie suivante.
        if let Some(engine) = action.strip_prefix("hud_engine:") {
            let mangohud = engine.trim() == "mangohud";
            let appid = self.session_info().map(|state| state.appid);
            let mut config = self.config_snapshot();
            config.capture.hud_on_screen = !mangohud;
            config.launch.mangohud = mangohud;
            let _ = self.save_config(config);
            if let Some(appid) = appid {
                if let Ok(Some(mut game)) = self.store.game(appid) {
                    // Le choix vaut aussi pour la prochaine partie : sans ce
                    // drapeau, un ancien réglage MangoHud du jeu reprenait le
                    // dessus sur le moteur natif choisi ici.
                    game.settings.mangohud = Some(mangohud);
                    let _ = self.store.save_game(&game);
                }
            }
            if mangohud {
                // La plaque native ne doit pas rester en place pendant que le
                // jeu annonce MangoHud ; celui-ci prendra la main au prochain
                // lancement, comme le dit le message.
                self.kill_perf_hud();
                if let Ok(mut guard) = self.session.lock() {
                    if let Some(state) = guard.as_mut() {
                        state.info.mangohud = true;
                    }
                }
                message = "MangoHud : actif au prochain lancement".into();
            } else {
                // Retire MangoHud de la partie en cours avant de remettre la
                // plaque native : les deux ne doivent jamais coexister.
                let _ = self.set_game_hud(false);
                let _ = lapinux_perf::mangohud::set_display(false);
                if let Ok(mut guard) = self.session.lock() {
                    if let Some(state) = guard.as_mut() {
                        state.info.mangohud = false;
                    }
                }
                self.kill_perf_hud();
                let _ = self.spawn_perf_hud();
                message = "plaque native affichée".into();
            }
            return Ok(Envelope::ok(&serde_json::json!({ "message": message })));
        }
        // La taille de la plaque native s'applique à chaud : le processus de
        // HUD est relancé avec la nouvelle échelle.
        // Volume du flux du jeu, depuis le menu rapide. Le changement est
        // appliqué à chaud et ne touche aucun autre son du bureau.
        if let Some(value) = action.strip_prefix("game_volume:") {
            let Ok(volume) = value.trim().parse::<f32>() else {
                return Ok(Envelope::err("volume invalide"));
            };
            let Some(info) = self.session_info() else {
                return Ok(Envelope::err("aucun jeu en cours"));
            };
            let pids = descendants(info.pid);
            return match crate::captures::set_game_volume(&pids, volume) {
                Ok(value) => {
                    message = format!("volume du jeu : {:.0} %", value * 100.0);
                    Ok(Envelope::ok(&serde_json::json!({ "message": message })))
                }
                Err(error) => Ok(Envelope::err(error)),
            };
        }
        // Message envoyé depuis la conversation d'un ami. Le texte peut
        // contenir des deux-points : seule la première séparation compte.
        if let Some(body) = action.strip_prefix("message:") {
            let Some((steam_id, text)) = body.split_once(':') else {
                return Ok(Envelope::err("message Steam invalide"));
            };
            let Ok(steam_id) = steam_id.trim().parse::<u64>() else {
                return Ok(Envelope::err("SteamID d'ami invalide"));
            };
            return self.steam_social_message(steam_id, text);
        }
        if let Some(scale) = action.strip_prefix("hud_scale:") {
            if let Ok(value) = scale.trim().parse::<f32>() {
                let value = value.clamp(0.75, 2.0);
                let mut config = self.config_snapshot();
                config.capture.hud_scale = value;
                let _ = self.save_config(config);
                self.kill_perf_hud();
                let _ = self.spawn_perf_hud();
                return Ok(Envelope::ok(&serde_json::json!({
                    "message": "taille du HUD enregistrée",
                    "scale": value,
                })));
            }
        }
        // Interrupteur de la page Performances : la plaque native seulement.
        // Le badge d'enregistrement a son propre processus et reste intact.
        if let Some(show) = action.strip_prefix("hud_visible:") {
            let show = show.trim() != "0";
            let appid = self.session_info().map(|state| state.appid);
            let mut config = self.config_snapshot();
            config.capture.hud_on_screen = show;
            // La plaque native est le moteur par défaut quand on la rallume.
            config.launch.mangohud = false;
            let _ = self.save_config(config);
            if let Some(appid) = appid {
                if let Ok(Some(mut game)) = self.store.game(appid) {
                    // Un réglage MangoHud propre au jeu ne doit pas ressurgir
                    // au prochain lancement après avoir choisi la plaque.
                    game.settings.mangohud = Some(false);
                    let _ = self.store.save_game(&game);
                }
            }
            // L'interrupteur est global : MangoHud et la plaque sont coupés
            // ensemble, puis seule la plaque est éventuellement rallumée.
            let _ = self.set_game_hud(false);
            let _ = lapinux_perf::mangohud::set_display(false);
            if let Ok(mut guard) = self.session.lock() {
                if let Some(state) = guard.as_mut() {
                    state.info.mangohud = false;
                }
            }
            self.kill_perf_hud();
            if show {
                let _ = self.spawn_perf_hud();
            }
            return Ok(Envelope::ok(&serde_json::json!({
                "message": if show {
                    "HUD de performance affiché"
                } else {
                    "HUD de performance retiré"
                },
            })));
        }
        // Réglages du HUD d'enregistrement : le daemon relance uniquement son
        // processus de badge, jamais celui de la plaque de performance.
        if let Some(rest) = action.strip_prefix("record_badge:") {
            let mut parts = rest.splitn(3, ':');
            let enabled = parts.next().is_some_and(|value| value == "1");
            let position = parts.next().unwrap_or("bottom-left").to_string();
            let scale = parts
                .next()
                .and_then(|value| value.parse::<f32>().ok())
                .unwrap_or(1.0)
                .clamp(0.65, 2.0);
            let mut config = self.config_snapshot();
            config.capture.record_badge = enabled;
            config.capture.record_badge_position = position;
            config.capture.record_badge_scale = scale;
            let _ = self.save_config(config);
            self.kill_record_hud();
            if enabled {
                let _ = self.spawn_record_hud();
            }
            return Ok(Envelope::ok(&serde_json::json!({
                "message": "HUD d'enregistrement mis à jour",
            })));
        }
        // Commandes de la page Enregistrements de l'overlay. La pause est
        // immédiate ; l'arrêt ferme l'encodeur dans un fil séparé pour que
        // l'overlay et le menu restent réactifs pendant la finalisation.
        if let Some(paused) = action.strip_prefix("recording_pause:") {
            return self.recording_pause(paused.trim() != "0");
        }
        if action == "recording_start" {
            let shared = Arc::clone(self);
            std::thread::spawn(move || {
                let _ = shared.recording(true);
            });
            return Ok(Envelope::ok(&serde_json::json!({
                "message": "démarrage de l'enregistrement demandé",
            })));
        }
        if action == "recording_stop" {
            let shared = Arc::clone(self);
            std::thread::spawn(move || {
                let _ = shared.recording(false);
            });
            return Ok(Envelope::ok(&serde_json::json!({
                "message": "arrêt de l'enregistrement demandé",
            })));
        }
        if action.starts_with("fps_cap:") {
            if let Ok(value) = action.trim_start_matches("fps_cap:").parse::<u32>() {
                let mut target = None;
                if let Ok(mut guard) = self.session.lock() {
                    if let Some(state) = guard.as_mut() {
                        state.info.fps_cap = value;
                        // Changer la limite ne doit pas perdre le profil,
                        // l'ancrage ni le graphique choisis dans l'onglet
                        // Performances.
                        let profile = if state.info.hud_preset.trim().is_empty() {
                            "full".to_string()
                        } else {
                            state.info.hud_preset.clone()
                        };
                        let mut settings = mangohud::preset_settings_at(
                            &profile,
                            Some(value),
                            &state.info.hud_position,
                        );
                        settings.enabled = state.info.mangohud;
                        // La couche corrigée de Lapinux place les neuf
                        // ancrages en ligne comme en bloc.
                        settings.horizontal = layout_is_inline(&state.info.hud_layout);
                        settings.frame_timing = state.info.hud_graph;
                        settings.cpu_cores = state.info.hud_cores;
                        settings.screen = Some(self.screen_size());
                        let _ = mangohud::write_config(
                            &self.paths.config_dir,
                            state.info.appid,
                            &settings,
                        );
                        // La limite suit aussi le jeu : le prochain lancement
                        // repart de cette valeur au lieu de celle des réglages.
                        if let Ok(Some(mut game)) = self.store.game(state.info.appid) {
                            game.settings.fps_cap = Some(value);
                            let _ = self.store.save_game(&game);
                        }
                        target = Some(state.info.clone());
                    }
                }
                // GameScope peut changer sa limite à chaud via gamescopectl ;
                // sinon la valeur reste enregistrée pour le prochain lancement.
                // La commande attend une cadence, pas un diviseur : la valeur
                // demandée est transmise telle quelle, GameScope choisit seul
                // le mode de rafraîchissement qui la porte.
                let live = match target.as_ref() {
                    Some(info) if info.gamescope => {
                        match set_gamescope_fps_limit(info, value) {
                            Ok(()) => true,
                            Err(err) => {
                                tracing::warn!("limite FPS à chaud : {err}");
                                false
                            }
                        }
                    }
                    _ => false,
                };
                message = if !live {
                    format!("limite FPS {value} : prochain lancement")
                } else {
                    format!("limite FPS {value} appliquée")
                };
            }
        } else if let Some(spec) = action.strip_prefix("hud:") {
            // Format complet : `hud:<0|1>:<profil>:<position>:<disposition>:<graphique>`.
            // L'ancien `hud:<profil>` reste accepté pour une instance déjà
            // lancée, et garde alors l'ancrage et le graphique courants.
            let mut applied = String::new();
            let mut stop_native = false;
            let mut drop_native_config = false;
            if let Ok(mut guard) = self.session.lock() {
                if let Some(state) = guard.as_mut() {
                    if let Some((enabled, profile, position, layout, graph, cores)) =
                        parse_hud_spec(
                            spec,
                            &state.info.hud_preset,
                            &state.info.hud_position,
                            layout_is_inline(&state.info.hud_layout),
                            state.info.hud_graph,
                            state.info.hud_cores,
                        )
                    {
                        let mut settings = mangohud::preset_settings_at(
                            &profile,
                            Some(state.info.fps_cap),
                            &position,
                        );
                        settings.enabled = enabled;
                        settings.horizontal = layout;
                        settings.frame_timing = graph;
                        settings.cpu_cores = cores;
                        settings.screen = Some(self.screen_size());
                        state.info.hud_preset = profile.clone();
                        state.info.hud_position = position.clone();
                        state.info.hud_layout = if settings.horizontal {
                            "inline"
                        } else {
                            "block"
                        }
                        .to_string();
                        state.info.hud_graph = graph;
                        state.info.hud_cores = cores;
                        let was_mangohud = state.info.mangohud;
                        state.info.mangohud = enabled;
                        stop_native = enabled;
                        // En activant MangoHud, la plaque native s'efface. En
                        // le coupant, elle ne doit pas réapparaître toute
                        // seule alors que l'utilisateur a demandé « coupé ».
                        drop_native_config = enabled || was_mangohud;
                        let _ = mangohud::write_config(
                            &self.paths.config_dir,
                            state.info.appid,
                            &settings,
                        );
                        // Le profil, l'ancrage et la disposition suivent le
                        // jeu : le prochain lancement écrit la même
                        // configuration MangoHud.
                        if let Ok(Some(mut game)) = self.store.game(state.info.appid) {
                            game.settings.hud_preset = Some(profile.clone());
                            game.settings.hud_position = Some(position.clone());
                            game.settings.hud_layout = Some(state.info.hud_layout.clone());
                            game.settings.hud_graph = Some(graph);
                            game.settings.hud_cores = Some(cores);
                            game.settings.mangohud = Some(enabled);
                            let _ = self.store.save_game(&game);
                        }
                        applied = format!(
                            "MangoHud {} · {} · {} · {} · graphique {} · cœurs {}",
                            if enabled { "activé" } else { "coupé" },
                            profile_label(&profile),
                            position_label(&position),
                            layout_label(layout),
                            if graph { "activé" } else { "coupé" },
                            if cores { "activés" } else { "coupés" }
                        );
                    }
                }
            }
            if drop_native_config {
                let mut config = self.config_snapshot();
                config.capture.hud_on_screen = false;
                let _ = self.save_config(config);
            }
            if stop_native {
                self.kill_perf_hud();
            }
            message = if applied.is_empty() {
                "MangoHud : aucun jeu en cours".to_string()
            } else {
                // MangoHud relit le fichier à chaud ; le réglage est aussi
                // enregistré pour le prochain lancement.
                format!("{applied} : appliqué")
            };
        } else if action == "hud_cut" {
            // Le bouton « tout couper » : MangoHud s'éteint, la plaque est
            // retirée, sans toucher au reste de la configuration.
            let appid = self.session_info().map(|state| state.appid);
            let hidden = self.set_game_hud(false);
            let _ = lapinux_perf::mangohud::set_display(false);
            let mut config = self.config_snapshot();
            config.capture.hud_on_screen = false;
            config.launch.mangohud = false;
            let _ = self.save_config(config);
            if let Some(appid) = appid {
                if let Ok(Some(mut game)) = self.store.game(appid) {
                    game.settings.mangohud = Some(false);
                    let _ = self.store.save_game(&game);
                }
            }
            if let Ok(mut guard) = self.session.lock() {
                if let Some(state) = guard.as_mut() {
                    state.info.mangohud = false;
                }
            }
            self.kill_perf_hud();
            self.kill_record_hud();
            message = if hidden {
                "HUD coupés".into()
            } else {
                "plaque coupée".to_string()
            };
        } else if action == "profile_next" || action.starts_with("profile_select:") {
            let profiles = lapinux_input::list_profiles(&self.paths.data_dir);
            if !profiles.is_empty() {
                let session = self.session_info();
                let current = session
                    .as_ref()
                    .map(|state| state.input_profile.clone())
                    .unwrap_or_default();
                let appid = session.as_ref().map(|state| state.appid);
                // Un choix exact vient de la liste ; le cycle garde son
                // comportement historique pour les raccourcis.
                let wanted = action
                    .strip_prefix("profile_select:")
                    .map(str::trim)
                    .filter(|name| !name.is_empty());
                let next = match wanted {
                    Some(name) => profiles
                        .iter()
                        .find(|profile| profile.name == name)
                        .map(|profile| profile.name.clone()),
                    None => {
                        let index = profiles.iter().position(|p| p.name == current).unwrap_or(0);
                        Some(profiles[(index + 1) % profiles.len()].name.clone())
                    }
                };
                if let Some(next) = next {
                    if let Ok(mut guard) = self.session.lock() {
                        if let Some(state) = guard.as_mut() {
                            state.info.input_profile = next.clone();
                        }
                    }
                    self.start_input(&next, true);
                    // Le profil choisi dans l'overlay suit le jeu : la
                    // prochaine partie repart de ce choix au lieu de revenir
                    // au défaut.
                    if let Some(appid) = appid {
                        if let Ok(Some(mut game)) = self.store.game(appid) {
                            game.settings.input_profile = Some(next.clone());
                            let _ = self.store.save_game(&game);
                        }
                    }
                    message = format!("profil manettes : {next}");
                } else {
                    message = "profil manettes introuvable".into();
                }
            }
        } else if action == "proton_next" {
            let protons = self.protons_cached();
            if !protons.is_empty() {
                let session = self.session_info();
                let current = session
                    .as_ref()
                    .map(|state| state.proton.clone())
                    .unwrap_or_default();
                let appid = session.as_ref().map(|state| state.appid);
                let index = protons.iter().position(|p| p.name == current).unwrap_or(0);
                let next = protons[(index + 1) % protons.len()].name.clone();
                if let Ok(mut guard) = self.session.lock() {
                    if let Some(state) = guard.as_mut() {
                        state.info.proton = next.clone();
                    }
                }
                // Le jeu tourne déjà avec l'ancien Proton : le choix ne peut
                // pas s'appliquer à chaud, mais il doit au moins survivre à
                // la session et être utilisé au prochain lancement.
                if let Some(appid) = appid {
                    if let Ok(Some(mut game)) = self.store.game(appid) {
                        game.settings.proton = Some(next.clone());
                        let _ = self.store.save_game(&game);
                    }
                }
                message = format!("Proton {next} : prochain lancement");
            }
        } else if action == "quit_game" {
            self.stop_session()?;
            message = "jeu arrêté".into();
        } else if action == "screenshot" {
            let capture = self.config_snapshot().capture;
            message = take_screenshot(self.session_info().as_ref(), &capture);
        } else {
            message = "action ignorée".into();
        }
        Ok(Envelope::ok(&serde_json::json!({ "message": message })))
    }
}

/// Lit une action MangoHud de l'overlay :
/// `enabled:profil:position:disposition:graphique`. Sans profil, l'ancien
/// format `hud:<profil>` garde le profil, l'ancrage, la disposition et le
/// graphique de la session ; `off` coupe sans oublier le choix courant.
fn parse_hud_spec(
    spec: &str,
    current_profile: &str,
    current_position: &str,
    current_layout: bool,
    current_graph: bool,
    current_cores: bool,
) -> Option<(bool, String, String, bool, bool, bool)> {
    let parts: Vec<&str> = spec.split(':').filter(|part| !part.is_empty()).collect();
    match parts.as_slice() {
        [flag, profile, position, layout, graph, cores] => Some((
            matches!(*flag, "1" | "on" | "true"),
            normalize_profile(profile),
            mangohud::normalize_position(position),
            layout_is_inline(layout),
            matches!(*graph, "1" | "on" | "true"),
            matches!(*cores, "1" | "on" | "true"),
        )),
        // Format d'avant la ligne des cœurs : on garde le choix courant.
        [flag, profile, position, layout, graph] => Some((
            matches!(*flag, "1" | "on" | "true"),
            normalize_profile(profile),
            mangohud::normalize_position(position),
            layout_is_inline(layout),
            matches!(*graph, "1" | "on" | "true"),
            current_cores,
        )),
        // Format d'avant le graphique : on garde le choix courant.
        [flag, profile, position, layout] => Some((
            matches!(*flag, "1" | "on" | "true"),
            normalize_profile(profile),
            mangohud::normalize_position(position),
            layout_is_inline(layout),
            current_graph,
            current_cores,
        )),
        // Ancien format sans disposition : on garde les choix courants.
        [flag, profile, position] => Some((
            matches!(*flag, "1" | "on" | "true"),
            normalize_profile(profile),
            mangohud::normalize_position(position),
            current_layout,
            current_graph,
            current_cores,
        )),
        [profile] => {
            let position = mangohud::normalize_position(current_position);
            if profile.eq_ignore_ascii_case("off") {
                Some((
                    false,
                    normalize_profile(current_profile),
                    position,
                    current_layout,
                    current_graph,
                    current_cores,
                ))
            } else {
                Some((
                    true,
                    normalize_profile(profile),
                    position,
                    current_layout,
                    current_graph,
                    current_cores,
                ))
            }
        }
        _ => None,
    }
}

/// Change la limite de FPS d'un GameScope déjà lancé. `gamescopectl` parle
/// au compositeur par son socket Wayland ; la commande debug applique la
/// nouvelle limite et le cycle de rafraîchissement sans relancer le jeu.
/// Binaire à relancer pour l'overlay. Après une reconstruction, le démon déjà
/// lancé voit son propre exécutable comme supprimé (« … (deleted) ») : ce
/// chemin-là n'existe plus, et l'overlay refusait alors de démarrer. Le binaire
/// neuf est au même endroit, sans le suffixe.
pub(crate) fn overlay_exe() -> Result<std::path::PathBuf> {
    let exe = std::env::current_exe()?;
    if exe.exists() {
        return Ok(exe);
    }
    let text = exe.to_string_lossy().to_string();
    if let Some(clean) = text.strip_suffix(" (deleted)") {
        let path = std::path::PathBuf::from(clean);
        if path.exists() {
            return Ok(path);
        }
    }
    lapinux_core::paths::which("lapinux")
        .ok_or_else(|| lapinux_core::Error::Other(format!("binaire de l'overlay introuvable ({text})")))
}

fn set_gamescope_fps_limit(info: &SessionInfo, value: u32) -> std::result::Result<(), String> {
    let ctl = lapinux_core::paths::which("gamescopectl")
        .ok_or_else(|| "gamescopectl introuvable".to_string())?;
    let display = gamescope_display(info).unwrap_or_else(|| "gamescope-0".into());
    let output = Command::new(ctl)
        .env("GAMESCOPE_WAYLAND_DISPLAY", &display)
        .arg("debug_set_fps_limit")
        .arg(value.to_string())
        .output()
        .map_err(|e| format!("gamescopectl : {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let error = String::from_utf8_lossy(&output.stderr);
        Err(format!("gamescopectl : {}", error.trim()))
    }
}

/// Nom du socket Wayland de GameScope, lu dans l'environnement d'un de ses
/// descendants (le jeu). `gamescope-0` reste le repli quand `/proc` ne le
/// donne pas.
fn gamescope_display(info: &SessionInfo) -> Option<String> {
    let mut pids = descendants(info.pid);
    pids.insert(0, info.pid);
    for pid in pids {
        let Ok(raw) = std::fs::read(format!("/proc/{pid}/environ")) else {
            continue;
        };
        let text = String::from_utf8_lossy(&raw);
        for entry in text.split('\0') {
            if let Some(value) = entry.strip_prefix("GAMESCOPE_WAYLAND_DISPLAY=") {
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
    }
    None
}

fn layout_is_inline(text: &str) -> bool {
    matches!(
        text.trim().to_ascii_lowercase().as_str(),
        "inline" | "line" | "horizontal" | "ligne"
    )
}

fn layout_label(inline: bool) -> &'static str {
    if inline {
        "En ligne"
    } else {
        "En bloc"
    }
}

fn normalize_profile(profile: &str) -> String {
    match profile.trim().to_ascii_lowercase().as_str() {
        "simple" => "simple".into(),
        "full" | "detailed" => "full".into(),
        _ => "essential".into(),
    }
}

fn profile_label(profile: &str) -> &'static str {
    match profile {
        "simple" => "Simple",
        "full" => "Détaillée",
        _ => "Basique",
    }
}

fn position_label(position: &str) -> &'static str {
    match position {
        "top-left" => "haut gauche",
        "top-center" => "haut centre",
        "top-right" => "haut droite",
        "middle-left" => "milieu gauche",
        "middle-center" => "centre",
        "middle-right" => "milieu droite",
        "bottom-left" => "bas gauche",
        "bottom-center" => "bas centre",
        "bottom-right" => "bas droite",
        _ => "haut gauche",
    }
}

/// Le processus `root` et tous ses descendants, lus dans `/proc`. La fenêtre
/// du jeu appartient souvent à un enfant de Wine, pas au processus lancé.
/// Sortie Wayland du HUD. Le réglage de lancement est prioritaire : c'est lui
/// que GameScope reçoit (`--prefer-output`), et donc l'écran où le jeu
/// s'affiche. S'il est automatique, on demande au guetteur KWin l'écran de la
/// fenêtre du jeu ; sans réponse, le compositeur garde son choix par défaut.
pub(crate) fn hud_output(info: &SessionInfo, configured: &str) -> Option<String> {
    let configured = configured.trim();
    if !configured.is_empty() {
        return Some(configured.to_string());
    }
    lapinux_input::shortcut::game_window(&descendants(info.pid), Duration::from_millis(300))
        .map(|window| window.output)
        .filter(|output| !output.is_empty())
}

pub(crate) fn descendants(root: u32) -> Vec<u32> {
    let mut parents: Vec<(u32, u32)> = Vec::new();
    if let Ok(entries) = std::fs::read_dir("/proc") {
        for entry in entries.flatten() {
            let name = entry.file_name();
            if !name.to_string_lossy().chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            let Ok(pid) = name.to_string_lossy().parse::<u32>() else {
                continue;
            };
            let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
                continue;
            };
            let Some(after) = stat.rfind(')').map(|index| &stat[index + 1..]) else {
                continue;
            };
            let Some(ppid) = after.split_whitespace().nth(1).and_then(|p| p.parse().ok()) else {
                continue;
            };
            parents.push((pid, ppid));
        }
    }
    let mut out = vec![root];
    let mut index = 0;
    while index < out.len() {
        let current = out[index];
        index += 1;
        for (pid, ppid) in &parents {
            if *ppid == current && !out.contains(pid) {
                out.push(*pid);
            }
        }
    }
    out
}

/// Ce pid descend-il du processus lancé ? KWin rapporte le pid de la fenêtre,
/// qui n'est pas celui d'`umu-run` : sous Proton, la fenêtre du jeu appartient
/// à un enfant de Wine. On remonte `/proc` jusqu'à la racine.
pub(crate) fn process_belongs_to(mut pid: u32, root: u32) -> bool {
    for _ in 0..64 {
        if pid == root {
            return true;
        }
        if pid <= 1 {
            return false;
        }
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            return false;
        };
        // `pid (nom) état ppid…` : le nom peut contenir espaces et
        // parenthèses, le parent se lit donc après la dernière ')'.
        let Some(after) = stat.rfind(')').map(|index| &stat[index + 1..]) else {
            return false;
        };
        let Some(parent) = after.split_whitespace().nth(1) else {
            return false;
        };
        let Ok(parent) = parent.parse::<u32>() else {
            return false;
        };
        pid = parent;
    }
    false
}

fn split_chord(text: &str) -> Vec<String> {
    text.split('+')
        .map(|part| part.trim().to_string())
        .filter(|part| !part.is_empty())
        .collect()
}

/// Dossier où l'overlay range ses captures. La section Captures de
/// l'interface relit ce même dossier : les deux chemins doivent rester
/// d'accord.
pub fn captures_dir() -> std::path::PathBuf {
    lapinux_core::paths::videos_dir().join("Lapinux")
}

/// Dossier des photos de jeu : le dossier XDG des images, sous-dossier
/// Lapinux. Une photo n'est pas une vidéo, et les deux dossiers du bureau
/// servent à les retrouver.
pub fn screenshots_dir() -> std::path::PathBuf {
    lapinux_core::paths::pictures_dir().join("Lapinux")
}

/// Déplace les captures laissées directement dans `~/Images` par une version
/// antérieure : sans cela, la galerie ne les verrait plus après le changement
/// de dossier.
pub fn migrate_legacy_captures() {
    let legacy = lapinux_core::paths::home_dir().join("Images");
    let Ok(entries) = std::fs::read_dir(&legacy) else {
        return;
    };
    let videos = captures_dir();
    let photos = screenshots_dir();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(text) = name.to_str() else {
            continue;
        };
        if !text.starts_with("lapinux-") {
            continue;
        }
        let target_dir = match entry.path().extension().and_then(|ext| ext.to_str()) {
            Some("mp4") | Some("pcm") | Some("err") => &videos,
            Some("png") | Some("jpg") | Some("jpeg") => &photos,
            _ => continue,
        };
        let target = target_dir.join(&name);
        if target.exists() {
            continue;
        }
        if std::fs::create_dir_all(target_dir).is_err() {
            continue;
        }
        let _ = std::fs::rename(entry.path(), &target);
    }
}

/// Réduit un nom de jeu à ce qui passe dans un nom de fichier : minuscules,
/// sans espace ni accent. Les caractères inconnus deviennent un tiret, jamais
/// deux de suite.
pub(crate) fn capture_slug(name: &str) -> String {
    let mut slug = String::new();
    for c in name.chars() {
        let folded = match c {
            'à' | 'á' | 'â' | 'ä' | 'ã' | 'å' => 'a',
            'ç' => 'c',
            'è' | 'é' | 'ê' | 'ë' => 'e',
            'ì' | 'í' | 'î' | 'ï' => 'i',
            'ñ' => 'n',
            'ò' | 'ó' | 'ô' | 'ö' | 'õ' | 'ø' => 'o',
            'ù' | 'ú' | 'û' | 'ü' => 'u',
            'ý' | 'ÿ' => 'y',
            other => other,
        };
        if folded.is_ascii_alphanumeric() {
            slug.push(folded.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.trim_matches('-').chars().take(40).collect()
}

/// Capture nommée d'après la partie en cours : la section Captures retrouve
/// ainsi le jeu et son icône sans rien redemander à Steam.
fn take_screenshot(
    session: Option<&lapinux_core::model::SessionInfo>,
    capture: &lapinux_core::config::CaptureConfig,
) -> String {
    // La fenêtre du jeu prend le focus avant la photo : sans cela, la capture
    // de la fenêtre active prendrait le menu rapide ou le bureau.
    if let Some(info) = session {
        let _ = lapinux_input::shortcut::focus_window(&descendants(info.pid));
    }    let dir = capture.images_dir();
    std::fs::create_dir_all(&dir).ok();
    let _ = std::fs::write(dir.join(lapinux_core::captures::MARKER), b"");

    let now = lapinux_core::util::now_unix();
    let path = match session {
        Some(info) => {
            let slug = capture_slug(&info.name);
            let stem = if slug.is_empty() {
                format!("lapinux-{}-{now}", info.appid)
            } else {
                format!("lapinux-{}-{slug}-{now}", info.appid)
            };
            dir.join(format!("{stem}.png"))
        }
        None => dir.join(format!("lapinux-{now}.png")),
    };
    // La photo prend la fenêtre du jeu, pas le bureau : KWin reçoit d'abord
    // l'activation (`-a` capture la fenêtre active), le temps que la
    // demande traverse son cycle.
    if session.is_some() {
        std::thread::sleep(Duration::from_millis(250));
    }
    if let Some(spectacle) = lapinux_core::paths::which("spectacle") {
        for window in [true, false] {
            let mut command = Command::new(&spectacle);
            if window {
                command.arg("-a");
            }
            let status = command
                .arg("-b")
                .arg("-n")
                .arg("-o")
                .arg(&path)
                .status();
            if matches!(status, Ok(s) if s.success()) {
                return format!(
                    "capture {} : {}",
                    if window { "du jeu" } else { "de l'écran" },
                    path.display()
                );
            }
        }
    }
    if let Some(grim) = lapinux_core::paths::which("grim") {
        let status = Command::new(grim).arg(&path).status();
        if matches!(status, Ok(s) if s.success()) {
            return format!("capture : {}", path.display());
        }
    }
    "aucun outil de capture disponible (installez spectacle ou grim)".into()
}

/// Profil par défaut du jeu (utilisé par la CLI).
pub fn default_profile(paths: &lapinux_core::LapinuxPaths, name: &str) -> Profile {
    lapinux_input::load_profile(&paths.data_dir, name).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hud_spec_reads_graph_and_keeps_old_formats() {
        let full = parse_hud_spec(
            "1:full:top-center:inline:1:1",
            "essential",
            "top-left",
            false,
            false,
            false,
        )
        .unwrap();
        assert_eq!(full.0, true);
        assert_eq!(full.1, "full");
        assert_eq!(full.2, "top-center");
        assert!(full.3);
        assert!(full.4);
        assert!(full.5);

        let old = parse_hud_spec(
            "1:simple:bottom-right",
            "full",
            "top-left",
            false,
            true,
            true,
        )
        .unwrap();
        assert_eq!(old.1, "simple");
        assert_eq!(old.2, "bottom-right");
        assert!(!old.3);
        // L'ancien format sans graphique conserve le réglage courant.
        assert!(old.4);
        assert!(old.5);

        let legacy = parse_hud_spec("off", "full", "top-left", true, true, true).unwrap();
        assert!(!legacy.0);
        assert_eq!(legacy.1, "full");
        assert_eq!(legacy.3, true);
        assert_eq!(legacy.4, true);
        assert_eq!(legacy.5, true);
    }
}

pub fn overlay_log_path(paths: &lapinux_core::LapinuxPaths, appid: u32) -> PathBuf {
    paths.log_dir.join(format!("overlay-{appid}.log"))
}
