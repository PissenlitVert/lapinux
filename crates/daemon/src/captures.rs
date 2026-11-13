//! Captures : l'enregistreur vidéo de la partie en cours.
//!
//! Rien d'externe : KWin fournit l'image (`org.kde.KWin.ScreenShot2`, zone
//! cadrée sur la fenêtre du jeu) et `ffmpeg` l'encode. Aucun portail, aucun
//! sélecteur de fenêtre, aucune application tierce de capture.
//!
//! KWin n'ouvre cette interface qu'aux applications dont un fichier `.desktop`
//! déclare `X-KDE-DBUS-Restricted-Interfaces=org.kde.KWin.ScreenShot2`.
//! Lapinux écrit le sien à la première capture ; KWin lit ces fichiers à son
//! démarrage, donc la permission prend effet au démarrage suivant du
//! compositeur (déconnexion suivante).

use crate::kwin_shot;
use crate::{Result, Shared};
use lapinux_core::config::CaptureConfig;
use lapinux_core::ipc::Envelope;
use lapinux_core::model::SessionInfo;
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Nom du fichier `.desktop` qui porte l'autorisation KWin. Il ne prend pas le
/// nom de l'application : une entrée écrite par l'utilisateur n'est jamais
/// écrasée.
const PERMISSION_DESKTOP: &str = "lapinux-capture.desktop";
/// Reste à attendre `ffmpeg` après la dernière image.
const STOP_WAIT: Duration = Duration::from_secs(25);
/// Trace de l'enregistrement en cours. Écrite sur disque avant que les images
/// n'arrivent, elle permet à un daemon qui redémarre après un plantage de
/// reprendre l'encodeur qu'il a laissé derrière lui.
pub(crate) const RECORDER_JOURNAL: &str = "recorder.json";
/// Temps laissé à un encodeur orphelin pour terminer son fichier avant qu'on
/// ne le termine de force. Court : la reprise bloque le démarrage du daemon,
/// et l'encodeur d'un daemon mort s'arrête déjà seul avec la fermeture du tube.
const ORPHAN_WAIT: Duration = Duration::from_secs(5);
/// Temps laissé au jeu pour ouvrir son flux audio.
const GAME_AUDIO_WAIT: Duration = Duration::from_secs(60);
/// Temps laissé au jeu pour remplacer sa fenêtre d'attente par la vraie :
/// ``A Short Hike`` ouvre son écran de chargement, puis sa fenêtre de jeu
/// plusieurs secondes plus tard.
const WINDOW_GRACE: Duration = Duration::from_secs(6);
/// Format du son rangé à côté de la vidéo, avant le mixage : PCM signé
/// 16 bits, 48 kHz, deux canaux.
const PCM_RATE: u64 = 48_000;
const PCM_CHANNELS: u64 = 2;
/// Latence demandée au nœud `pw-record`. La valeur par défaut de PipeWire est
/// de 100 ms : elle retardait le son de la capture d'environ un dixième de
/// seconde, même quand le décalage de départ était juste.
const PCM_LATENCY: &str = "20ms";

/// Zone capturée : celle de la fenêtre du jeu, en coordonnées du bureau.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Area {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Ce que l'enregistreur capte à chaque image.
#[derive(Clone, Debug)]
enum Source {
    /// La fenêtre du jeu elle-même : elle ne contient que le jeu, et sa
    /// disparition termine la vidéo — le bureau n'y entre jamais.
    Window(String),
    /// Repli : une zone de l'écran cadrée sur la fenêtre.
    Area(Area),
}

/// Enregistrement en cours : `ffmpeg` reçoit les images brutes par son entrée.
pub struct Recorder {
    /// Partie à laquelle appartient cette capture : un enregistrement laissé
    /// par une partie précédente ne doit pas empêcher la suivante d'être filmée.
    appid: u32,
    child: Child,
    path: PathBuf,
    stop: Arc<AtomicBool>,
    /// Pause effective : ni image ni échantillon ne sont écrits.
    paused: Arc<AtomicBool>,
    /// Pause demandée à la main depuis les Captures : elle survit à
    /// l'ouverture puis à la fermeture du menu rapide.
    manual_paused: Arc<AtomicBool>,
    /// Menu rapide ouvert : la capture est suspendue elle aussi.
    menu_paused: Arc<AtomicBool>,
    /// Images déjà remises à `ffmpeg` : c'est le chronomètre de la vidéo.
    frames: Arc<AtomicU64>,
    fps: u32,
    /// Sources audio captées en parallèle de l'image.
    audio: Vec<AudioTrack>,
    /// Retire le noir de fermeture propre à la scène persistante.
    trim_black: bool,
    /// Début de la capture, pour comparer la durée vidéo au temps réel.
    started: Instant,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// Un capteur audio et le fil qui le lance.
struct AudioTrack {
    /// Les capteurs, une fois lancés : un jeu Proton peut exposer plusieurs
    /// flux, et un flux peut apparaître en retard. Le processus naît donc
    /// dans le fil et non avant lui.
    children: Arc<Mutex<Vec<Child>>>,
    /// Les pistes écrites, avec leur retard sur l'image en millisecondes.
    /// Le repli sur la sortie du bureau est marqué pour n'être gardé que si
    /// aucune piste du jeu n'a de son.
    files: Arc<Mutex<Vec<AudioFile>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// Une piste audio écrite par un capteur.
#[derive(Clone)]
struct AudioFile {
    path: PathBuf,
    offset_ms: u64,
    /// Piste de secours : la sortie du bureau, prise seulement quand les
    /// flux du jeu restent muets.
    fallback: bool,
    /// Piste d'un flux du jeu (par opposition au microphone et au repli).
    game: bool,
}

impl Recorder {
    /// Arrête la capture, ferme l'entrée de `ffmpeg` et attend le fichier.
    fn stop(mut self) -> std::result::Result<PathBuf, String> {
        self.stop.store(true, Ordering::Relaxed);
        // Instant de l'arrêt demandé, avant d'attendre les capteurs : la
        // fermeture de `pw-record` peut prendre du temps et ne doit pas être
        // comptée comme du temps enregistré.
        let capture_ended = Instant::now();
        // Le son s'arrête avant l'image : ses capteurs rendent la main dès
        // que leur tube est fermé, et leurs fichiers sont alors complets.
        let mut tracks = Vec::new();
        for mut track in std::mem::take(&mut self.audio) {
            // Le fil referme lui-même ses capteurs (SIGTERM, attente, puis
            // SIGKILL) : leurs WAV gardent ainsi un en-tête lisible.
            if let Some(thread) = track.thread.take() {
                let _ = thread.join();
            }
            let files = track
                .files
                .lock()
                .map(|files| files.clone())
                .unwrap_or_default();
            for file in files {
                let size = file.path.metadata().map(|meta| meta.len()).unwrap_or(0);
                if size <= 44 {
                    // Le journal du capteur explique souvent pourquoi : cible
                    // refusée, périphérique occupé, format indisponible.
                    let detail = std::fs::read_to_string(file.path.with_extension("err"))
                        .unwrap_or_default();
                    tracing::warn!(
                        "son : fichier audio vide ({}), piste ignorée ({})",
                        file.path.display(),
                        detail.trim()
                    );
                }
                let _ = std::fs::remove_file(file.path.with_extension("err"));
                tracks.push(file);
            }
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        // Le fil possédait l'entrée de ffmpeg : sa fin ferme le flux, et
        // l'encodeur termine son fichier.
        // Une différence entre les deux durées est le signe d'une image
        // perdue : l'audio continue au rythme réel et se retrouve en avance.
        let video_secs = self.frames.load(Ordering::Relaxed) as f64 / f64::from(self.fps.max(1));
        let real_secs = capture_ended.duration_since(self.started).as_secs_f64();
        tracing::info!("capture : {video_secs:.2} s de vidéo pour {real_secs:.2} s réelles");
        let deadline = Instant::now() + STOP_WAIT;
        let clean;
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    clean = status.success();
                    break;
                }
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(80));
                }
                Ok(None) => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    return Err(format!(
                        "ffmpeg ne s'est pas arrêté ; fichier à vérifier : {}",
                        self.path.display()
                    ));
                }
                Err(err) => return Err(format!("ffmpeg : {err}")),
            }
        }
        // Aucune image n'est arrivée : le fichier ne serait qu'un conteneur
        // vide, qu'aucune visionneuse ne sait lire. Mieux vaut ne rien garder
        // et le dire que laisser une vidéo morte dans le dossier.
        // Le journal de l'encodeur n'a d'intérêt qu'en cas d'échec : une
        // capture réussie ne laisse pas de `.err` à côté de la vidéo.
        if clean {
            let _ = std::fs::remove_file(self.path.with_extension("err"));
        }
        if self.frames.load(Ordering::Relaxed) == 0 {
            for file in &tracks {
                let _ = std::fs::remove_file(&file.path);
            }
            let _ = std::fs::remove_file(&self.path);
            let _ = std::fs::remove_file(self.path.with_extension("err"));
            return Err("aucune image captée : la vidéo n'a pas pu être enregistrée".to_string());
        }
        // La scène garde la fenêtre GameScope ouverte après la sortie du jeu :
        // le dernier plan reste noir pendant que Proton rend la main. On coupe
        // cette queue, sans toucher aux scènes noires du jeu lui-même.
        if self.trim_black {
            match trim_trailing_black(&self.path, video_secs) {
                Some(removed) => {
                    tracing::info!("capture : noir de fermeture retiré ({removed:.2} s)")
                }
                None => {}
            }
        }
        // Le son arrive après coup : l'image est recopiée telle quelle, seuls
        // les échantillons sont encodés.
        // Un flux jamais trouvé laisse un fichier absent ou vide : il ne
        // compte pas, sinon le mixage échouerait pour rien. Une piste muette
        // est écartée elle aussi : `amix` normalise par nombre d'entrées, et
        // une piste silencieuse atténuerait les autres.
        let analyzed: Vec<(AudioFile, bool)> = tracks
            .into_iter()
            .map(|file| {
                let audible = file
                    .path
                    .metadata()
                    .map(|meta| meta.len() > 44)
                    .unwrap_or(false)
                    && wav_peak(&file.path).is_some_and(|peak| peak > 8);
                (file, audible)
            })
            .collect();
        let any_game = analyzed.iter().any(|(file, audible)| *audible && file.game);
        let mut kept_fallback = false;
        let mut ready: Vec<(PathBuf, u64)> = Vec::new();
        for (file, audible) in &analyzed {
            if !*audible {
                continue;
            }
            if file.fallback {
                if any_game {
                    continue;
                }
                kept_fallback = true;
            }
            ready.push((file.path.clone(), file.offset_ms));
        }
        if any_game {
            tracing::info!("son : {} piste(s) du jeu mixée(s)", ready.len());
        } else if kept_fallback {
            tracing::warn!("son : flux du jeu muets, repli sur la sortie du bureau");
        } else if !analyzed.is_empty() {
            tracing::warn!("son : aucune piste audible, la vidéo reste sans son");
        }
        if !ready.is_empty() {
            match mux_audio(&self.path, &ready) {
                Ok(()) => {}
                Err(err) => tracing::warn!("son : {err}"),
            }
        }
        for (file, _) in &analyzed {
            let _ = std::fs::remove_file(&file.path);
        }
        Ok(self.path)
    }
}

/// Écrit la trace d'un enregistrement, en une seule écriture renommée : un
/// plantage au mauvais moment laisse soit la trace complète, soit rien.
fn write_journal(log_dir: &Path, recorder: &Recorder) {
    let mut audio_pids = Vec::new();
    for track in &recorder.audio {
        if let Ok(children) = track.children.lock() {
            audio_pids.extend(children.iter().map(|child| child.id()));
        }
    }
    let journal = serde_json::json!({
        "daemon_pid": std::process::id(),
        "ffmpeg_pid": recorder.child.id(),
        "path": recorder.path.to_string_lossy(),
        "appid": recorder.appid,
        "audio_pids": audio_pids,
    });
    let _ = std::fs::create_dir_all(log_dir);
    let path = log_dir.join(RECORDER_JOURNAL);
    let temp = path.with_extension("json.tmp");
    if std::fs::write(&temp, journal.to_string()).is_ok() {
        let _ = std::fs::rename(&temp, &path);
    }
}

/// Reprise d'un enregistrement orphelin : l'encodeur est laissé finir son
/// fichier, ses capteurs audio sont arrêtés et la trace est retirée.
fn recover_orphan_recording_at(log_dir: &Path) {
    let path = log_dir.join(RECORDER_JOURNAL);
    let Ok(raw) = std::fs::read(&path) else {
        return;
    };
    let Ok(journal) = serde_json::from_slice::<serde_json::Value>(&raw) else {
        // Trace illisible : elle ne sert plus à rien.
        let _ = std::fs::remove_file(&path);
        return;
    };
    let previous = journal["daemon_pid"].as_u64().unwrap_or(0) as u32;
    if previous == std::process::id() {
        return;
    }
    // Un autre daemon vit encore (relance, tests) : sa capture ne nous
    // appartient pas.
    if previous != 0 && process_is_alive(previous) {
        return;
    }
    let encoder = journal["ffmpeg_pid"].as_u64().unwrap_or(0) as u32;
    let video = journal["path"].as_str().unwrap_or_default().to_string();
    tracing::warn!(
        "enregistrement orphelin (daemon {previous}, encodeur {encoder}) : reprise"
    );
    let clean = stop_orphan_encoder(encoder);
    for pid in journal["audio_pids"].as_array().cloned().unwrap_or_default() {
        if let Some(pid) = pid.as_u64() {
            stop_audio_sensor(pid as u32);
        }
    }
    let file = PathBuf::from(&video);
    if file.as_os_str().is_empty() {
        tracing::warn!("enregistrement orphelin sans fichier à récupérer");
    } else if clean && file.is_file() {
        tracing::info!("vidéo récupérée après un plantage : {}", file.display());
    } else if file.is_file() {
        tracing::warn!(
            "vidéo laissée par un plantage, à vérifier : {}",
            file.display()
        );
    }
    let _ = std::fs::remove_file(&path);
}

/// Vrai si le processus tourne encore.
///
/// Un daemon tué mais pas récolté par son parent reste joignable (`kill 0`
/// répond) avec l'état `Z` : il n'a plus aucun tube ouvert et ses enfants sont
/// déjà orphelins. Le compter comme vivant empêchait la reprise de s'exécuter
/// après un `SIGKILL`.
pub(crate) fn process_is_alive(pid: u32) -> bool {
    if pid == 0 || unsafe { libc::kill(pid as libc::pid_t, 0) } != 0 {
        return false;
    }
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    // Le nom du processus peut contenir des espaces et des parenthèses :
    // l'état est le premier champ qui suit la dernière parenthèse fermante.
    stat.rsplit(')')
        .next()
        .map(|rest| !rest.trim_start().starts_with('Z'))
        .unwrap_or(false)
}

/// Vrai si ce pid est bien l'encodeur : un pid réutilisé par un autre
/// programme ne doit jamais être arrêté.
fn is_encoder(pid: u32) -> bool {
    let Ok(raw) = std::fs::read(format!("/proc/{pid}/cmdline")) else {
        return false;
    };
    String::from_utf8_lossy(&raw)
        .split('\0')
        .any(|arg| arg.ends_with("ffmpeg"))
}

/// Laisse l'encodeur orphelin terminer son fichier. Son tube est déjà fermé
/// avec le daemon, donc il doit s'arrêter seul ; sinon `SIGTERM` lui fait
/// encore refermer proprement le conteneur, `SIGKILL` est le dernier recours.
/// Rend `true` quand il s'est arrêté sans avoir été tué.
fn stop_orphan_encoder(pid: u32) -> bool {
    if !process_is_alive(pid) || !is_encoder(pid) {
        return true;
    }
    let deadline = Instant::now() + ORPHAN_WAIT;
    while Instant::now() < deadline {
        if !process_is_alive(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    // `SIGTERM` laisse encore ffmpeg refermer son conteneur ; `SIGKILL` est le
    // dernier recours, quand l'encodeur refuse même cela.
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if !process_is_alive(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
    false
}

/// Les capteurs audio d'un enregistrement mort n'ont plus de lecteur.
fn stop_audio_sensor(pid: u32) {
    if !process_is_alive(pid) {
        return;
    }
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
}

/// Vrai quand la capture est possible sur cette machine : `ffmpeg` pour
/// l'encodage et un bus de session pour joindre KWin.
pub fn available() -> bool {
    lapinux_core::paths::which("ffmpeg").is_some() && zbus::blocking::Connection::session().is_ok()
}

/// Fichier `.desktop` qui déclare l'interface KWin à Lapinux. `exe` est le
/// binaire qui capture : KWin compare ce chemin à celui du processus appelant.
fn permission_desktop(exe: &Path) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Lapinux (capture)\n\
         Comment=Autorise Lapinux à capturer la fenêtre du jeu\n\
         Exec={} %U\n\
         Icon=lapinux\n\
         NoDisplay=true\n\
         X-KDE-DBUS-Restricted-Interfaces=org.kde.KWin.ScreenShot2\n",
        exe.display()
    )
}

/// Écrit le fichier d'autorisation, une fois pour toutes. Le fichier n'est
/// réécrit que si le chemin du binaire a changé.
pub(crate) fn ensure_permission_file() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let dir = lapinux_core::paths::home_dir().join(".local/share/applications");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join(PERMISSION_DESKTOP);
    let wanted = permission_desktop(&exe);
    if std::fs::read_to_string(&path).ok().as_deref() != Some(wanted.as_str()) {
        let _ = std::fs::write(&path, wanted);
        tracing::info!("capture : autorisation KWin écrite ({})", path.display());
    }
}

impl Shared {
    /// Relevé CPU / mémoire / carte graphique pour le HUD de l'overlay. Une
    /// lecture par seconde : l'échantillonneur garde les compteurs précédents
    /// pour calculer la charge.
    pub fn sample_system(&self) -> lapinux_core::system::SystemInfo {
        match self.system.lock() {
            Ok(mut sampler) => sampler.sample(),
            Err(_) => lapinux_core::system::SystemInfo::default(),
        }
    }

    /// Trace de l'enregistrement en cours, à côté des journaux du daemon.
    fn recorder_journal(&self) -> PathBuf {
        self.paths.log_dir.join(RECORDER_JOURNAL)
    }

    /// Note sur disque l'enregistrement qui démarre : pid du daemon, pid de
    /// l'encodeur, fichier écrit et capteurs audio. Un daemon tué laisse
    /// cette trace derrière lui, et le suivant s'en sert pour ne pas laisser
    /// un `ffmpeg` tourner jusqu'à la fin des temps.
    fn write_recorder_journal(&self) {
        let Ok(guard) = self.recorder.lock() else {
            return;
        };
        let Some(recorder) = guard.as_ref() else {
            return;
        };
        write_journal(&self.paths.log_dir, recorder);
    }

    /// Reprend la main sur un enregistrement laissé par un daemon mort.
    ///
    /// Un daemon tué ferme son extrémité du tube : `ffmpeg` lit la fin du flux
    /// et referme son fichier. Ce n'est pas garanti (machine saturée,
    /// encodeur bloqué), et même alors les capteurs audio et le badge REC
    /// restent orphelins. La trace écrite au démarrage de l'enregistrement
    /// permet de finir le travail au démarrage suivant.
    pub(crate) fn recover_orphan_recording(&self) {
        recover_orphan_recording_at(&self.paths.log_dir);
    }

    /// Un enregistrement tourne-t-il déjà ?
    pub fn is_recording(&self) -> bool {
        self.recorder
            .lock()
            .map(|guard| guard.is_some())
            .unwrap_or(false)
    }

    /// Capture effective d'un jeu : ses réglages explicites priment sur la
    /// configuration globale, champ par champ.
    fn capture_for(&self, appid: u32, config: &lapinux_core::config::Config) -> CaptureConfig {
        self.store
            .game(appid)
            .ok()
            .flatten()
            .map(|game| {
                // Un jeu protégé par un anti-triche ne s'enregistre pas tout
                // seul : la protection se lit dans la même règle que le
                // lancement.
                let (settings, _) = self.protected_settings(&game);
                settings.effective_capture(&config.capture)
            })
            .unwrap_or_else(|| config.capture.clone())
    }

    /// Fenêtre du jeu : son rectangle, ramené dans son écran (repli), et
    /// l'identifiant KWin qui permet de capturer la fenêtre elle-même.
    fn game_target(&self, info: &SessionInfo) -> Option<(Area, String)> {
        let window = lapinux_input::shortcut::game_window(
            &crate::launch::descendants(info.pid),
            Duration::from_secs(3),
        )?;
        let area = subject_area(
            window.x,
            window.y,
            window.width,
            window.height,
            window.screen_x,
            window.screen_y,
            window.screen_width,
            window.screen_height,
        );
        Some((area, window.handle))
    }

    /// Démarre l'enregistrement de la partie en cours : la fenêtre du jeu est
    /// capturée par KWin et l'image part directement dans `ffmpeg`.
    pub(crate) fn record_start(&self) -> std::result::Result<PathBuf, String> {
        // Deux appels rapprochés (fenêtre du jeu, minuteur de session) ne
        // doivent pas lancer deux encodeurs : le lancement est pris une fois,
        // et le second appel voit l'enregistrement déjà en place.
        let _starting = self
            .recording_start
            .lock()
            .map_err(|_| "enregistrement : verrou indisponible".to_string())?;
        let info = self
            .session_info()
            .ok_or_else(|| "aucune partie en cours à enregistrer".to_string())?;
        if self.is_recording() {
            // Un enregistrement d'une partie précédente n'est pas celui-ci :
            // sans cet arrêt, le badge REC continuerait de tourner et cette
            // partie ne serait jamais filmée.
            let previous = self
                .recorder
                .lock()
                .ok()
                .and_then(|guard| guard.as_ref().map(|recorder| recorder.appid));
            if previous == Some(info.appid) {
                return Err("un enregistrement tourne déjà".into());
            }
            self.record_stop();
        }
        let ffmpeg = lapinux_core::paths::which("ffmpeg")
            .ok_or_else(|| "ffmpeg absent : installez-le pour enregistrer".to_string())?;
        ensure_permission_file();
        let (area, handle) = self
            .game_target(&info)
            .ok_or_else(|| "fenêtre du jeu introuvable pour le cadrage".to_string())?;
        let config = self.config_snapshot();
        let capture = self.capture_for(info.appid, &config);
        // La cadence choisie dans les réglages est celle de la vidéo. KWin
        // peut répéter une image quand il n'arrive pas à suivre, mais ffmpeg
        // reçoit bien le rythme demandé : le réglage n'est plus décoratif.
        let fps = capture.video_fps.clamp(5, 240);
        let dir = capture.videos_dir();
        std::fs::create_dir_all(&dir).map_err(|e| format!("dossier des captures : {e}"))?;
        // Le marqueur dit à la galerie que ce dossier lui appartient : elle
        // peut alors lire les noms personnalisés sans ramasser les vidéos
        // personnelles posées à côté.
        let _ = std::fs::write(dir.join(lapinux_core::captures::MARKER), b"");
        let now = lapinux_core::util::now_unix();
        let base_stem = capture.render_stem(&info.name, info.appid, fps, now);
        let (stem, path) = unique_capture_path(&dir, &base_stem, "mp4");
        // Sonde d'autorisation et de taille : la première image donne les
        // dimensions que ffmpeg attendra pour tout le flux. La fenêtre du jeu
        // est préférée à la zone de l'écran — une zone montre le bureau avant
        // la première image du jeu et après sa fermeture, une fenêtre jamais.
        let (source, probe) = if handle.is_empty() {
            (Source::Area(area), probe_area(area)?)
        } else {
            match kwin_shot::capture_window(&handle) {
                Ok(shot) => {
                    tracing::info!("capture : la fenêtre du jeu elle-même ({handle})");
                    (Source::Window(handle), shot)
                }
                Err(err) => {
                    tracing::warn!(
                        "capture : fenêtre non capturable ({err}) ; repli sur la zone de l'écran"
                    );
                    (Source::Area(area), probe_area(area)?)
                }
            }
        };
        let (width, height) = (probe.width, probe.height);
        if width == 0 || height == 0 {
            return Err(format!("capture vide ({width}x{height})"));
        }
        // La capture KWin arrive à la taille de la fenêtre ; le redimensionnement
        // éventuel se fait dans ffmpeg avant l'encodeur. Une vidéo 720p ou 1080p
        // demande beaucoup moins de mémoire et de calcul qu'une fenêtre 4K.
        let target = capture_size(width, height, &capture.video_resolution);
        let filter = video_filter(
            width,
            height,
            target,
            VideoProfile::from_config(&capture.video_profile),
        );
        // Sans filtre, NVENC reçoit directement le BGRA de KWin et fait la
        // conversion 4:2:0 sur le GPU. Avec un redimensionnement, le filtre
        // `scale` rend déjà du YUV 4:2:0 : le forcer en BGRA coûterait une
        // conversion CPU supplémentaire.
        let native_input = filter.is_none();
        let mut command = Command::new(&ffmpeg);
        command
            .args(["-hide_banner", "-loglevel", "error", "-f", "rawvideo"])
            .args(["-pix_fmt", "bgra", "-s"])
            .arg(format!("{width}x{height}"))
            .args(["-r", &fps.to_string(), "-i", "-"]);
        // Un encodeur 4:2:0 refuse une dimension impaire, et le profil peut
        // demander une hauteur maximale plus basse que la fenêtre.
        if let Some(filter) = filter {
            command.args(["-vf", &filter]);
        }
        // Le journal de l'encodeur est gardé à côté de la vidéo : sans lui, un
        // refus de ffmpeg était invisible et la vidéo manquait sans
        // explication.
        let err_path = path.with_extension("err");
        command
            .args(video_encoder_args(&ffmpeg, &capture, native_input))
            .args(["-movflags", "+faststart"])
            .arg(&path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::from(
                std::fs::File::create(&err_path)
                    .unwrap_or_else(|_| std::fs::File::open("/dev/null").expect("/dev/null")),
            ));
        // Pas de `PR_SET_PDEATHSIG` ici : ce signal suit le thread qui a lancé
        // le processus, et ffmpeg était tué dès la fin de la requête — la
        // capture s'arrêtait après une image. Le tube suffit : quand le daemon
        // meurt, son extrémité d'écriture se ferme, ffmpeg lit la fin du flux
        // et referme le fichier proprement.
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(err) => {
                // Le journal a été créé pour ce lancement : sans ffmpeg, il
                // resterait un fichier vide dans le dossier des captures.
                let _ = std::fs::remove_file(&err_path);
                return Err(format!("lancement de ffmpeg : {err}"));
            }
        };
        let stdin = child.stdin.take();
        let stop = Arc::new(AtomicBool::new(false));
        let paused = Arc::new(AtomicBool::new(false));
        let manual_paused = Arc::new(AtomicBool::new(false));
        let menu_paused = Arc::new(AtomicBool::new(false));
        let frames = Arc::new(AtomicU64::new(0));
        // L'image part en premier. Le son est lancé quand la première image
        // est déjà dans la vidéo : son décalage est alors positif et mesurable
        // au lieu de le faire démarrer avant l'image et de le laisser avancer.
        let thread = capture_frames(
            stdin,
            source,
            (width, height),
            area,
            info.pid,
            fps,
            Arc::clone(&stop),
            Arc::clone(&paused),
            Arc::clone(&frames),
        );
        let first_frame = Instant::now() + Duration::from_secs(2);
        while frames.load(Ordering::Relaxed) == 0
            && !stop.load(Ordering::Relaxed)
            && Instant::now() < first_frame
        {
            std::thread::sleep(Duration::from_millis(5));
        }
        // La première image est le zéro de la vidéo : c'est à partir de cet
        // instant que la durée réelle doit être comparée au nombre d'images.
        let capture_started = Instant::now();
        let audio = self.start_audio(&info, &capture, &dir, &stem, &stop, &frames, fps);
        if let Ok(mut guard) = self.recorder.lock() {
            *guard = Some(Recorder {
                appid: info.appid,
                child,
                path: path.clone(),
                stop,
                paused,
                manual_paused,
                menu_paused,
                frames,
                fps,
                audio,
                trim_black: info.scene,
                started: capture_started,
                thread: Some(thread),
            });
        }
        // La trace part avant la première image : même un plantage immédiat
        // laisse de quoi retrouver cet encodeur au démarrage suivant.
        self.write_recorder_journal();
        tracing::info!(
            "enregistrement de {} : {} ({width}x{height} → {}x{}, {fps} i/s, profil {})",
            info.name,
            path.display(),
            target.0,
            target.1,
            capture.video_profile
        );
        Ok(path)
    }

    /// Prépare les capteurs audio demandés par les réglages. Le son du jeu
    /// n'est pas celui du bureau : on ne prend que le flux audio du jeu. Le
    /// micro n'est pris que si sa case est cochée.
    fn start_audio(
        &self,
        info: &SessionInfo,
        capture: &CaptureConfig,
        dir: &Path,
        stem: &str,
        stop: &Arc<AtomicBool>,
        frames: &Arc<AtomicU64>,
        fps: u32,
    ) -> Vec<AudioTrack> {
        if !capture.record_audio && !capture.record_microphone {
            return Vec::new();
        }
        let Some(record) = lapinux_core::paths::which("pw-record") else {
            tracing::warn!("son : pw-record absent (paquet pipewire-utils), vidéo sans audio");
            return Vec::new();
        };
        let mut tracks = Vec::new();
        if capture.record_audio {
            tracks.push(start_game_audio(
                &record,
                dir,
                stem,
                crate::launch::descendants(info.pid),
                Arc::clone(frames),
                fps,
                Arc::clone(stop),
            ));
        }
        if capture.record_microphone {
            tracks.push(start_microphone_audio(
                &record,
                dir.join(format!("{stem}.micro.pcm")),
                Arc::clone(frames),
                fps,
                Arc::clone(stop),
            ));
        }
        tracks
    }

    /// Suspend la capture : le menu rapide est ouvert, plus une image ni un
    /// échantillon n'est écrit.
    pub fn record_pause(&self) {
        self.set_menu_paused(true);
    }

    /// Reprend la capture après la fermeture du menu, sauf si l'utilisateur
    /// l'avait mise en pause à la main depuis les Captures.
    pub fn record_resume(&self) {
        self.set_menu_paused(false);
    }

    fn set_menu_paused(&self, paused: bool) {
        if let Ok(guard) = self.recorder.lock() {
            if let Some(recorder) = guard.as_ref() {
                recorder.menu_paused.store(paused, Ordering::Relaxed);
                let effective = paused || recorder.manual_paused.load(Ordering::Relaxed);
                Self::apply_pause(recorder, effective);
            }
        }
    }

    /// Pause ou reprise demandée depuis la page Captures.
    pub fn recording_pause(&self, paused: bool) -> Result<Envelope> {
        let mut found = false;
        if let Ok(guard) = self.recorder.lock() {
            if let Some(recorder) = guard.as_ref() {
                found = true;
                recorder.manual_paused.store(paused, Ordering::Relaxed);
                let effective = paused || recorder.menu_paused.load(Ordering::Relaxed);
                Self::apply_pause(recorder, effective);
            }
        }
        if !found {
            return Ok(Envelope::err("aucun enregistrement en cours".to_string()));
        }
        Ok(Envelope::ok(&serde_json::json!({ "paused": paused })))
    }

    fn apply_pause(recorder: &Recorder, paused: bool) {
        if recorder.paused.swap(paused, Ordering::Relaxed) == paused {
            return;
        }
        // L'image s'arrête d'elle-même ; le son doit être suspendu, sinon la
        // pause reviendrait dans la vidéo.
        let signal = if paused { libc::SIGSTOP } else { libc::SIGCONT };
        for track in &recorder.audio {
            if let Ok(children) = track.children.lock() {
                for child in children.iter() {
                    unsafe {
                        libc::kill(child.id() as libc::pid_t, signal);
                    }
                }
            }
        }
        tracing::info!(
            "enregistrement {}",
            if paused { "en pause" } else { "repris" }
        );
    }

    /// Ce que l'overlay affiche : pause demandée par l'utilisateur et
    /// secondes déjà captées. La suspension automatique du menu rapide n'est
    /// pas comptée : sinon « Reprendre » semblerait sans effet tant que
    /// l'overlay est ouvert, puisque le menu remet lui-même la capture en
    /// pause.
    pub fn recording_state(&self) -> Option<lapinux_core::model::RecordingState> {
        let guard = self.recorder.lock().ok()?;
        let recorder = guard.as_ref()?;
        Some(lapinux_core::model::RecordingState {
            paused: recorder.manual_paused.load(Ordering::Relaxed),
            secs: recorder.frames.load(Ordering::Relaxed) / u64::from(recorder.fps.max(1)),
        })
    }

    /// Arrête l'enregistrement et rend le fichier gardé.
    pub fn record_stop(&self) -> Option<std::result::Result<PathBuf, String>> {
        let recorder = self
            .recorder
            .lock()
            .ok()
            .and_then(|mut guard| guard.take())?;
        let stopped = recorder.stop();
        // L'enregistrement est fini : la trace ne sert plus qu'à un daemon qui
        // reprendrait après un plantage, elle disparaît avec lui.
        let _ = std::fs::remove_file(self.recorder_journal());
        Some(stopped.map(|path| {
            tracing::info!("vidéo enregistrée : {}", path.display());
            path
        }))
    }

    /// Démarre l'enregistrement si le réglage le demande, une fois par
    /// partie : la fenêtre du jeu doit exister pour être cadrée. Le mode
    /// performance du jeu coupe cet enregistrement automatique ; l'appel
    /// manuel à `record_start` reste disponible.
    pub(crate) fn start_recording_if_wanted(self: &std::sync::Arc<Self>) {
        let Some(info) = self.session_info() else {
            return;
        };
        let config = self.config_snapshot();
        if info.performance || !self.capture_for(info.appid, &config).record_on_launch {
            return;
        }
        if let Err(err) = self.record_start() {
            tracing::warn!("enregistrement : {err}");
            // KWin peut refuser la toute première capture le temps de lire
            // son autorisation, et une fenêtre de jeu peut apparaître une
            // seconde après la première image. Un second essai unique évite
            // de perdre toute la partie pour ce faux départ.
            let shared = std::sync::Arc::clone(self);
            let appid = info.appid;
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(1500));
                let config = shared.config_snapshot();
                if shared.is_recording() || !shared.capture_for(appid, &config).record_on_launch {
                    return;
                }
                if let Err(err) = shared.record_start() {
                    tracing::warn!("enregistrement (second essai) : {err}");
                }
            });
        }
    }

    /// Arrête l'enregistrement à la sortie du jeu quand le réglage le
    /// demande ; sinon il continue, et les Captures proposent de l'arrêter.
    ///
    /// L'identifiant vient de l'appelant : `end_session` tourne après
    /// `take_session`, quand `session_info()` est déjà vide. S'appuyer dessus
    /// laissait l'enregistrement continuer après la partie, badge REC compris.
    pub(crate) fn stop_recording_on_exit(&self, appid: u32) {
        let config = self.config_snapshot();
        if !self.capture_for(appid, &config).stop_on_exit {
            return;
        }
        match self.record_stop() {
            Some(Ok(_)) => {}
            Some(Err(err)) => tracing::warn!("enregistrement : {err}"),
            None => {}
        }
    }

    /// Entrée IPC : lance ou arrête l'enregistrement à la demande.
    pub fn recording(&self, start: bool) -> Result<Envelope> {
        if start {
            if self.is_recording() {
                return Ok(Envelope::err("un enregistrement tourne déjà".to_string()));
            }
            return Ok(match self.record_start() {
                Ok(path) => Envelope::ok(&serde_json::json!({
                    "message": format!("enregistrement : {}", path.display()),
                    "path": path.to_string_lossy(),
                })),
                Err(err) => Envelope::err(err),
            });
        }
        Ok(match self.record_stop() {
            Some(Ok(path)) => Envelope::ok(&serde_json::json!({
                "message": format!("vidéo enregistrée : {}", path.display()),
                "path": path.to_string_lossy(),
            })),
            Some(Err(err)) => Envelope::err(err),
            None => Envelope::err("aucun enregistrement en cours".to_string()),
        })
    }
}

/// Évite d'écraser une capture commencée dans la même seconde : le nom
/// lisible reste intact, un suffixe ` (2)` distingue seulement le doublon.
fn unique_capture_path(dir: &Path, base: &str, extension: &str) -> (String, PathBuf) {
    let mut stem = base.to_string();
    let mut path = dir.join(format!("{stem}.{extension}"));
    let mut suffix = 2;
    while path.exists() {
        stem = format!("{base} ({suffix})");
        path = dir.join(format!("{stem}.{extension}"));
        suffix += 1;
    }
    (stem, path)
}

/// Boucle de capture : une image KWin par période, rangée sans remplissage
/// puis écrite dans l'entrée de `ffmpeg`.
///
/// En pause, rien n'est capté ni écrit : `ffmpeg` compte ses images au lieu
/// de lire une horloge, donc le temps du menu ouvert ne laisse aucune trace
/// dans la vidéo — elle saute directement de l'image d'avant à celle d'après.
fn capture_frames(
    stdin: Option<ChildStdin>,
    source: Source,
    size: (u32, u32),
    fallback: Area,
    pid: u32,
    fps: u32,
    stop: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    frames: Arc<AtomicU64>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        // 30 i/s vaut 33,333 ms, pas 33 ms : l'arrondi à la milliseconde
        // faisait écrire 30,3 images par seconde, et la vidéo gagnait plus
        // d'une seconde sur une longue partie. La période est exacte à la
        // nanoseconde, donc l'image et le son restent sur la même horloge.
        let period = Duration::from_nanos(1_000_000_000 / u64::from(fps.max(1)));
        // Un seul emplacement pour la dernière image : la capture ne doit
        // jamais attendre que l'encodeur ait fini d'écrire. C'est cette
        // attente qui décalait la capture par rapport à l'horloge réelle.
        // Le tampon est déplacé, jamais recopié : `Arc::from(Vec<u8>)`
        // allouait et copiait encore 8 Mo pour chaque image 1080p.
        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(1);
        // La connexion au bus est réutilisée par toutes les images de la
        // partie : elle ne dépend ni de la fenêtre ni de la géométrie.
        let capturer = kwin_shot::Capturer::new().ok();
        // Une image n'est demandée à KWin que lorsque l'écrivain a de la place
        // pour la recevoir. Sans cette demande, une capture 60 i/s continuait
        // à lire et copier des images 1080p pendant que ffmpeg était déjà en
        // retard, ce qui prenait des ressources au jeu pour rien.
        let demand = Arc::new(AtomicBool::new(true));
        let capture_demand = Arc::clone(&demand);
        let capture_stop = Arc::clone(&stop);
        let capture_paused = Arc::clone(&paused);
        let capture_frames_count = Arc::clone(&frames);
        let capture = std::thread::spawn(move || {
            // Ces deux closures vivent dans le fil de capture : elles peuvent
            // emprunter la connexion persistante sans sortir de son périmètre.
            let shot_window = |handle: &str| match capturer.as_ref() {
                Some(capturer) => capturer.capture_window(handle),
                None => kwin_shot::capture_window(handle),
            };
            let shot_area = |area: &Area| match capturer.as_ref() {
                Some(capturer) => capturer.capture_area(area.x, area.y, area.width, area.height),
                None => kwin_shot::capture_area(area.x, area.y, area.width, area.height),
            };
            let mut source = source;
            let mut misses = 0u32;
            let mut next_capture = Instant::now();
            'capture: while !capture_stop.load(Ordering::Relaxed) {
                if capture_paused.load(Ordering::Relaxed) {
                    std::thread::sleep(period);
                    next_capture = Instant::now();
                    continue;
                }
                if !capture_demand.swap(false, Ordering::Relaxed) {
                    // L'encodeur n'a pas encore consommé l'image précédente.
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                }
                let now = Instant::now();
                if next_capture > now {
                    std::thread::sleep(next_capture - now);
                }
                if capture_stop.load(Ordering::Relaxed) {
                    break 'capture;
                }
                let shot = match &source {
                    Source::Window(handle) => shot_window(handle),
                    Source::Area(area) => shot_area(area),
                };
                match shot {
                    Ok(shot) => {
                        // Une image d'une autre taille casserait tout le flux
                        // déjà encodé : elle compte comme un échec plutôt que
                        // d'être écrite de travers.
                        if (shot.width, shot.height) != size {
                            misses += 1;
                            if misses >= 3 {
                                tracing::warn!(
                                    "capture : image {}x{} au lieu de {}x{}, vidéo terminée",
                                    shot.width,
                                    shot.height,
                                    size.0,
                                    size.1
                                );
                                break 'capture;
                            }
                            std::thread::sleep(period);
                            continue;
                        }
                        misses = 0;
                        // KWin rend du RGB32/ARGB32 (BGRA) ou du
                        // RGBX/RGBA8888 selon le compositeur ; `into_bgra`
                        // ramène les deux familles au format attendu par
                        // ffmpeg. Un format inconnu arrête la capture plutôt
                        // que d'écrire une vidéo aux couleurs fausses.
                        if !kwin_shot::supported_format(shot.format) {
                            tracing::warn!("capture : format d'image inattendu {}", shot.format);
                            break 'capture;
                        }
                        let _ = tx.try_send(shot.into_bgra());
                        next_capture += period;
                        if next_capture < Instant::now() {
                            next_capture = Instant::now();
                        }
                    }
                    Err(err) => {
                        let current = match &source {
                            Source::Window(handle) => Some(handle.clone()),
                            Source::Area(_) => None,
                        };
                        let Some(current) = current else {
                            tracing::warn!("capture : {err}");
                            // Une interface refusée ne se débloque pas en
                            // réessayant trente fois par seconde.
                            std::thread::sleep(Duration::from_millis(500));
                            continue;
                        };
                        misses += 1;
                        if misses < 3 {
                            continue;
                        }
                        misses = 0;
                        // La fenêtre d'attente d'un jeu se ferme parfois
                        // plusieurs secondes avant que la vraie fenêtre
                        // n'apparaisse : on la cherche avant de conclure que
                        // la partie est finie.
                        let deadline = Instant::now() + WINDOW_GRACE;
                        loop {
                            if capture_stop.load(Ordering::Relaxed) {
                                break 'capture;
                            }
                            let next = lapinux_input::shortcut::game_window(
                                &crate::launch::descendants(pid),
                                Duration::from_millis(250),
                            )
                            .filter(|window| !window.handle.is_empty() && window.handle != current)
                            .and_then(|window| {
                                shot_window(&window.handle)
                                    .ok()
                                    .filter(|shot| (shot.width, shot.height) == size)
                                    .map(|shot| (window.handle, shot))
                            });
                            if let Some((handle, shot)) = next {
                                tracing::info!("capture : nouvelle fenêtre du jeu ({handle})");
                                source = Source::Window(handle);
                                misses = 0;
                                let _ = tx.try_send(shot.into_bgra());
                                break;
                            }
                            if Instant::now() >= deadline {
                                // Rien n'a encore été écrit : une zone cadrée
                                // sur le jeu vaut mieux qu'une vidéo vide,
                                // même si le bureau peut s'y glisser.
                                if capture_frames_count.load(Ordering::Relaxed) == 0 {
                                    tracing::warn!(
                                        "capture : fenêtre introuvable ({err}) ; repli sur la zone de l'écran"
                                    );
                                    source = Source::Area(fallback);
                                } else {
                                    tracing::info!(
                                        "capture : fenêtre du jeu fermée ({err}), vidéo terminée"
                                    );
                                    break 'capture;
                                }
                                break;
                            }
                            std::thread::sleep(Duration::from_millis(150));
                        }
                    }
                }
            }
        });

        // Le rythme des images est tenu ici, à la cadence choisie, indépendamment
        // du temps que met KWin à fournir une nouvelle capture. Si le jeu ou la
        // capture ralentit, la dernière image est répétée ; la vidéo ne perd
        // jamais de temps par rapport au son.
        let mut sink = stdin;
        let mut last: Option<Vec<u8>> = None;
        let mut next_at: Option<Instant> = None;
        'writer: while !stop.load(Ordering::Relaxed) {
            if paused.load(Ordering::Relaxed) {
                std::thread::sleep(period);
                // Le temps du menu ouvert ne compte pas : la reprise repart
                // de l'horloge, sans rattraper la pause.
                next_at = None;
                continue;
            }
            // Une seule capture en attente : la suivante n'est demandée
            // qu'après la consommation de celle-ci par l'écrivain.
            demand.store(true, Ordering::Relaxed);
            while let Ok(frame) = rx.try_recv() {
                last = Some(frame);
            }
            let Some(frame) = last.as_deref() else {
                std::thread::sleep(period);
                continue;
            };
            if let Some(at) = next_at {
                let now = Instant::now();
                if at > now {
                    std::thread::sleep(at - now);
                }
            }
            if stop.load(Ordering::Relaxed) || paused.load(Ordering::Relaxed) {
                continue;
            }
            let Some(out) = sink.as_mut() else {
                stop.store(true, Ordering::Relaxed);
                break 'writer;
            };
            if out.write_all(frame).is_err() {
                tracing::warn!("capture : flux d'encodage fermé");
                stop.store(true, Ordering::Relaxed);
                break 'writer;
            }
            frames.fetch_add(1, Ordering::Relaxed);
            let now = Instant::now();
            next_at = Some(match next_at {
                None => now + period,
                Some(at) => {
                    let planned = at + period;
                    // L'encodeur a pris plus d'une période : on repart de
                    // maintenant au lieu d'écrire une rafale de rattrapage.
                    if now > planned + period {
                        now + period
                    } else {
                        planned
                    }
                }
            });
        }
        let _ = capture.join();
    })
}

/// Sonde de capture sur la zone de l'écran, à sa taille réelle : son refus
/// arrive avant que ffmpeg ne crée un fichier vide, et ses dimensions fixent
/// celles du flux.
fn probe_area(area: Area) -> std::result::Result<kwin_shot::Shot, String> {
    kwin_shot::capture_area(area.x, area.y, area.width, area.height)
        .map_err(|err| format!("capture KWin refusée : {err}"))
}

/// Codec vidéo effectivement demandé par les réglages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VideoCodec {
    H264,
    Hevc,
    Av1,
}

impl VideoCodec {
    fn from_config(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "hevc" => Self::Hevc,
            "av1" => Self::Av1,
            _ => Self::H264,
        }
    }

    fn hardware(self) -> &'static str {
        match self {
            Self::H264 => "h264_nvenc",
            Self::Hevc => "hevc_nvenc",
            Self::Av1 => "av1_nvenc",
        }
    }

    fn software(self) -> &'static str {
        match self {
            Self::H264 => "libx264",
            Self::Hevc => "libx265",
            Self::Av1 => "libsvtav1",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::H264 => "H.264",
            Self::Hevc => "H.265",
            Self::Av1 => "AV1",
        }
    }
}

/// Effort demandé à l'encodeur : c'est le réglage qui change le plus la charge
/// pendant une partie. Le profil performance privilégie les images du jeu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VideoProfile {
    Performance,
    Balanced,
    Quality,
}

impl VideoProfile {
    fn from_config(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "performance" => Self::Performance,
            "quality" => Self::Quality,
            _ => Self::Balanced,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Performance => "performance",
            Self::Balanced => "équilibré",
            Self::Quality => "qualité",
        }
    }

    fn nvenc_preset(self) -> &'static str {
        match self {
            Self::Performance => "p1",
            Self::Balanced => "p2",
            Self::Quality => "p4",
        }
    }

    fn nvenc_tune(self) -> &'static str {
        match self {
            Self::Performance => "ll",
            Self::Balanced | Self::Quality => "hq",
        }
    }

    /// Plus le CQ est haut, moins l'encodeur dépense de débit et de calcul.
    fn nvenc_cq(self) -> u32 {
        match self {
            Self::Performance => 30,
            Self::Balanced => 27,
            Self::Quality => 22,
        }
    }

    fn software_preset(self) -> &'static str {
        match self {
            Self::Performance => "ultrafast",
            Self::Balanced => "veryfast",
            Self::Quality => "medium",
        }
    }

    fn svt_av1_preset(self) -> &'static str {
        match self {
            Self::Performance => "10",
            Self::Balanced => "8",
            Self::Quality => "6",
        }
    }

    fn crf(self) -> u32 {
        match self {
            Self::Performance => 30,
            Self::Balanced => 26,
            Self::Quality => 20,
        }
    }

    fn software_threads(self) -> &'static str {
        match self {
            Self::Performance | Self::Balanced => "2",
            Self::Quality => "0",
        }
    }

    fn scaler(self) -> &'static str {
        match self {
            Self::Performance => "fast_bilinear",
            Self::Balanced => "bilinear",
            Self::Quality => "lanczos",
        }
    }
}

/// Résolution de sortie : hauteur maximale, sans jamais agrandir une petite
/// fenêtre. Les deux dimensions sont paires pour les encodeurs 4:2:0.
fn capture_size(width: u32, height: u32, resolution: &str) -> (u32, u32) {
    let limit = match resolution.trim().to_ascii_lowercase().as_str() {
        "720p" => 720,
        "1080p" => 1080,
        "1440p" => 1440,
        _ => height,
    };
    if width == 0 || height == 0 {
        return (width.max(2), height.max(2));
    }
    let target_h = height.min(limit.max(2));
    let target_w = (u64::from(width) * u64::from(target_h) / u64::from(height)).max(2) as u32;
    ((target_w & !1).max(2), (target_h & !1).max(2))
}

fn video_filter(
    width: u32,
    height: u32,
    target: (u32, u32),
    profile: VideoProfile,
) -> Option<String> {
    if target == (width, height) {
        // Même une capture native peut avoir une dimension impaire : ffmpeg
        // la corrige alors sans changer l'échelle.
        return (width % 2 == 1 || height % 2 == 1)
            .then(|| "scale=trunc(iw/2)*2:trunc(ih/2)*2".to_string());
    }
    Some(format!(
        "scale={}:{}:flags={}",
        target.0,
        target.1,
        profile.scaler()
    ))
}

/// Arguments de l'encodeur vidéo, choisis pour chaque enregistrement.
///
/// Le codec et le profil viennent des réglages. NVENC est préféré quand il
/// fonctionne : il prend beaucoup moins de temps processeur que x264 sur une
/// image 1080p, et le profil performance réduit encore son preset et son
/// débit. Un encodeur indisponible retombe sur le suivant, sans empêcher la
/// capture.
fn video_encoder_args(
    ffmpeg: &Path,
    capture: &lapinux_core::config::CaptureConfig,
    native_input: bool,
) -> Vec<String> {
    let codec = VideoCodec::from_config(&capture.video_codec);
    let profile = VideoProfile::from_config(&capture.video_profile);
    let wanted = capture.video_encoder.trim().to_ascii_lowercase();

    let mut candidates: Vec<(&'static str, bool)> = Vec::new();
    if wanted != "software" {
        candidates.push((codec.hardware(), true));
    }
    if wanted != "nvenc" {
        candidates.push((codec.software(), false));
    }
    // Si le codec choisi n'est pas disponible ici, H.264 reste le repli le
    // plus compatible. Il vaut mieux une vidéo qu'un lancement en erreur.
    if codec != VideoCodec::H264 {
        if wanted != "software" {
            candidates.push(("h264_nvenc", true));
        }
        if wanted != "nvenc" {
            candidates.push(("libx264", false));
        }
    }

    let mut selected = None;
    for (encoder, hardware) in candidates {
        if encoder_works(ffmpeg, encoder) {
            selected = Some((encoder, hardware));
            break;
        }
    }
    let (encoder, hardware) = selected.unwrap_or_else(|| {
        if encoder_works(ffmpeg, "h264_nvenc") {
            ("h264_nvenc", true)
        } else {
            ("libx264", false)
        }
    });
    if encoder != codec.hardware() && encoder != codec.software() {
        tracing::warn!(
            "capture : {} indisponible, repli sur {}",
            codec.label(),
            encoder
        );
    }
    tracing::info!(
        "capture : encodeur {encoder} · codec {} · profil {}",
        codec.label(),
        profile.label()
    );

    let mut args = vec!["-c:v".to_string(), encoder.to_string()];
    if hardware {
        args.extend(["-preset".into(), profile.nvenc_preset().into()]);
        args.extend(["-tune".into(), profile.nvenc_tune().into()]);
        args.extend(["-rc".into(), "vbr".into()]);
        args.extend(["-cq".into(), profile.nvenc_cq().to_string()]);
        args.extend(["-b:v".into(), "0".into()]);
        if profile == VideoProfile::Performance {
            // Le mode performance privilégie la réactivité du jeu au débit.
            args.extend(["-zerolatency".into(), "1".into()]);
        }
    } else if encoder == "libsvtav1" {
        args.extend(["-preset".into(), profile.svt_av1_preset().into()]);
        args.extend(["-crf".into(), profile.crf().to_string()]);
        args.extend(["-threads".into(), profile.software_threads().into()]);
    } else {
        args.extend(["-preset".into(), profile.software_preset().into()]);
        args.extend(["-tune".into(), "zerolatency".into()]);
        args.extend(["-crf".into(), profile.crf().to_string()]);
        args.extend(["-threads".into(), profile.software_threads().into()]);
    }
    // NVENC AV1 accepte directement le BGRA compact de KWin et effectue la
    // conversion 4:2:0 sur le GPU. Pour les autres chemins, ou après un
    // redimensionnement déjà en YUV, on garde le format habituel.
    let pix_fmt = if hardware && native_input && encoder == "av1_nvenc" {
        "bgra"
    } else {
        "yuv420p"
    };
    args.extend(["-pix_fmt".into(), pix_fmt.into()]);
    args
}

/// L'encodeur encode-t-il vraiment sur cette machine ? Un pilote absent fait
/// échouer NVENC au premier lancement : mieux vaut le savoir avant la partie.
fn encoder_works(ffmpeg: &Path, encoder: &str) -> bool {
    static CACHE: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();
    if let Ok(cache) = CACHE.get_or_init(|| Mutex::new(HashMap::new())).lock() {
        if let Some(works) = cache.get(encoder) {
            return *works;
        }
    }
    let works = Command::new(ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "nullsrc=s=640x360:d=0.1",
            "-frames:v",
            "1",
            "-c:v",
            encoder,
            "-f",
            "null",
            "-",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if let Ok(mut cache) = CACHE.get_or_init(|| Mutex::new(HashMap::new())).lock() {
        cache.insert(encoder.to_string(), works);
    }
    works
}

/// Le processus meurt avec le daemon : un daemon redémarré ne doit jamais
/// laisser un encodeur filer dans le vide en mangeant les images du jeu.
fn survive_daemon() -> std::io::Result<()> {
    unsafe {
        libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0);
    }
    Ok(())
}

/// Lance un capteur `pw-record` sur une cible donnée (numéro de série d'un
/// nœud PipeWire, ou nom de la sortie du bureau). Le journal du capteur est
/// gardé à côté de la piste : sans lui, un refus de PipeWire serait invisible.
fn start_captor(record: &Path, path: &Path, target: Option<&str>) -> std::io::Result<Child> {
    let err_path = path.with_extension("err");
    let mut command = Command::new(record);
    command
        .arg("--container")
        .arg("wav")
        .arg("--rate")
        .arg(PCM_RATE.to_string())
        .arg("--channels")
        .arg(PCM_CHANNELS.to_string())
        .arg("--latency")
        .arg(PCM_LATENCY);
    if let Some(target) = target {
        command.arg("--target").arg(target);
    }
    command
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(
            std::fs::File::create(&err_path)
                .unwrap_or_else(|_| std::fs::File::open("/dev/null").expect("/dev/null")),
        ));
    unsafe {
        command.pre_exec(survive_daemon);
    }
    match command.spawn() {
        Ok(child) => Ok(child),
        Err(err) => {
            let _ = std::fs::remove_file(&err_path);
            Err(err)
        }
    }
}

/// Ferme tous les capteurs d'une piste : SIGTERM d'abord, pour que
/// `pw-record` termine son WAV proprement, puis SIGKILL au bout d'un instant.
fn stop_captors(children: &Arc<Mutex<Vec<Child>>>) {
    let Ok(mut guard) = children.lock() else {
        return;
    };
    for child in guard.iter() {
        let pid = child.id() as libc::pid_t;
        unsafe {
            libc::kill(pid, libc::SIGCONT);
            libc::kill(pid, libc::SIGTERM);
        }
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    for child in guard.iter_mut() {
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                _ if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break;
                }
                _ => std::thread::sleep(Duration::from_millis(20)),
            }
        }
    }
    guard.clear();
}

/// Décalage d'un capteur sur la vidéo, en millisecondes.
fn audio_offset(frames: &AtomicU64, fps: u32) -> u64 {
    frames.load(Ordering::Relaxed) * 1000 / u64::from(fps.max(1))
}

/// Capte tous les flux audio du jeu.
///
/// Un jeu Proton ouvre souvent deux flux — musique et effets — et les deux
/// doivent être pris : s'en tenir au premier donnait une piste muette quand
/// c'était l'autre qui jouait. Le premier flux trouvé est suivi quelques
/// secondes, le temps que le second apparaisse. Si les flux restent muets,
/// la sortie du bureau prend le relais pour que la vidéo ne soit pas vide.
fn start_game_audio(
    record: &Path,
    dir: &Path,
    stem: &str,
    pids: Vec<u32>,
    frames: Arc<AtomicU64>,
    fps: u32,
    stop: Arc<AtomicBool>,
) -> AudioTrack {
    let children = Arc::new(Mutex::new(Vec::new()));
    let files = Arc::new(Mutex::new(Vec::new()));
    let children_slot = Arc::clone(&children);
    let files_slot = Arc::clone(&files);
    let record = record.to_path_buf();
    let dir = dir.to_path_buf();
    let stem = stem.to_string();
    let thread = std::thread::spawn(move || {
        let deadline = Instant::now() + GAME_AUDIO_WAIT;
        let mut started: Vec<u64> = Vec::new();
        let mut first_seen: Option<Instant> = None;
        loop {
            if stop.load(Ordering::Relaxed) {
                stop_captors(&children_slot);
                return;
            }
            for (node, serial, label) in game_audio_nodes(&pids) {
                if started.contains(&node) {
                    continue;
                }
                let Some(serial) = serial else {
                    tracing::warn!("son du jeu : nœud {node} sans numéro de série, ignoré");
                    continue;
                };
                let path = dir.join(format!("{stem}.jeu-{node}.pcm"));
                match start_captor(&record, &path, Some(&serial.to_string())) {
                    Ok(child) => {
                        let offset = audio_offset(&frames, fps);
                        tracing::info!(
                            "son du jeu : flux « {label} » (nœud {node}) capté à {offset} ms"
                        );
                        if let Ok(mut guard) = children_slot.lock() {
                            guard.push(child);
                        }
                        if let Ok(mut guard) = files_slot.lock() {
                            guard.push(AudioFile {
                                path,
                                offset_ms: offset,
                                fallback: false,
                                game: true,
                            });
                        }
                        started.push(node);
                        first_seen.get_or_insert_with(Instant::now);
                    }
                    Err(err) => tracing::warn!("son du jeu : {err}"),
                }
            }
            if let Some(first) = first_seen {
                // Laisse le temps au second flux du jeu (musique, effets)
                // d'apparaître avant de figer la liste des capteurs.
                if first.elapsed() >= Duration::from_secs(2) {
                    break;
                }
            } else if Instant::now() >= deadline {
                tracing::warn!(
                    "son du jeu : aucun flux audio après {} s (flux présents : {}), vidéo sans son",
                    GAME_AUDIO_WAIT.as_secs(),
                    audio_stream_listing()
                );
                stop_captors(&children_slot);
                return;
            }
            std::thread::sleep(Duration::from_millis(250));
        }

        // Les flux du jeu peuvent exister sans rien jouer (un moteur audio
        // muet, un mauvais flux choisi). Un court relevé du niveau évite de
        // livrer une vidéo muette sans rien dire.
        std::thread::sleep(Duration::from_millis(1200));
        let silent = files_slot
            .lock()
            .map(|files| {
                files
                    .iter()
                    .all(|file| wav_peak(&file.path).map_or(true, |peak| peak <= 8))
            })
            .unwrap_or(true);
        if silent && !stop.load(Ordering::Relaxed) {
            if let Some(target) = default_sink_monitor() {
                let path = dir.join(format!("{stem}.bureau.pcm"));
                match start_captor(&record, &path, Some(&target)) {
                    Ok(child) => {
                        let offset = audio_offset(&frames, fps);
                        tracing::warn!(
                            "son du jeu : flux muets, repli sur la sortie du bureau à {offset} ms"
                        );
                        if let Ok(mut guard) = children_slot.lock() {
                            guard.push(child);
                        }
                        if let Ok(mut guard) = files_slot.lock() {
                            guard.push(AudioFile {
                                path,
                                offset_ms: offset,
                                fallback: true,
                                game: false,
                            });
                        }
                    }
                    Err(err) => tracing::warn!("son : repli sur la sortie du bureau : {err}"),
                }
            } else {
                tracing::warn!("son : flux du jeu muets et sortie du bureau introuvable");
            }
        }

        while !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(150));
        }
        stop_captors(&children_slot);
    });
    AudioTrack {
        children,
        files,
        thread: Some(thread),
    }
}

/// Capte le microphone par défaut du bureau.
fn start_microphone_audio(
    record: &Path,
    path: PathBuf,
    frames: Arc<AtomicU64>,
    fps: u32,
    stop: Arc<AtomicBool>,
) -> AudioTrack {
    let children = Arc::new(Mutex::new(Vec::new()));
    let files = Arc::new(Mutex::new(Vec::new()));
    let children_slot = Arc::clone(&children);
    let files_slot = Arc::clone(&files);
    let record = record.to_path_buf();
    let thread = std::thread::spawn(move || {
        match start_captor(&record, &path, None) {
            Ok(child) => {
                let offset = audio_offset(&frames, fps);
                tracing::info!("son : microphone capté à {offset} ms");
                if let Ok(mut guard) = children_slot.lock() {
                    guard.push(child);
                }
                if let Ok(mut guard) = files_slot.lock() {
                    guard.push(AudioFile {
                        path,
                        offset_ms: offset,
                        fallback: false,
                        game: false,
                    });
                }
            }
            Err(err) => tracing::warn!("capture audio : {err}"),
        }
        while !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(150));
        }
        stop_captors(&children_slot);
    });
    AudioTrack {
        children,
        files,
        thread: Some(thread),
    }
}

/// Sortie par défaut du bureau, sous la forme d'une source `.monitor` que
/// `pw-record` sait ouvrir. Sert uniquement de repli quand les flux du jeu
/// restent muets.
fn default_sink_monitor() -> Option<String> {
    if let Some(tool) = lapinux_core::paths::which("pactl") {
        if let Ok(output) = Command::new(tool).arg("get-default-sink").output() {
            if output.status.success() {
                let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !name.is_empty() {
                    return Some(format!("{name}.monitor"));
                }
            }
        }
    }
    let tool = lapinux_core::paths::which("wpctl")?;
    let output = Command::new(tool)
        .args(["inspect", "@DEFAULT_AUDIO_SINK@"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let name = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("node.name = "))
        .map(|value| value.trim().trim_matches('"'))
        .filter(|value| !value.is_empty())?;
    Some(format!("{name}.monitor"))
}

/// Niveau maximal d'un WAV, en valeur absolue d'un échantillon 16 bits.
/// Sert à écarter une piste muette avant le mixage : `amix` normalise par
/// nombre d'entrées, et une piste silencieuse atténuerait les autres.
fn wav_peak(path: &Path) -> Option<u32> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut header = [0u8; 4096];
    let read = file.read(&mut header).ok()?;
    let data = header[..read]
        .windows(4)
        .position(|window| window == b"data")?
        + 8;
    if data > read {
        return Some(0);
    }
    file.seek(SeekFrom::Start(data as u64)).ok()?;
    let mut buffer = [0u8; 64 * 1024];
    let mut peak = 0u32;
    loop {
        let read = file.read(&mut buffer).ok()?;
        if read == 0 {
            break;
        }
        let mut index = 0;
        while index + 1 < read {
            let sample = i16::from_le_bytes([buffer[index], buffer[index + 1]]);
            peak = peak.max(u32::from(sample.unsigned_abs()));
            index += 2;
        }
    }
    Some(peak)
}

/// Arguments de `ffmpeg` qui posent le son sur la vidéo : l'image est
/// recopiée, seuls les échantillons sont encodés. Deux sources (jeu et micro)
/// sont mélangées à parts égales.
fn mux_args(video: &Path, tracks: &[(PathBuf, u64)], out: &Path) -> Vec<std::ffi::OsString> {
    use std::ffi::OsString;
    let mut args: Vec<OsString> = ["-hide_banner", "-loglevel", "error", "-y", "-i"]
        .iter()
        .map(|arg| OsString::from(*arg))
        .collect();
    args.push(video.as_os_str().to_os_string());
    for (path, _) in tracks {
        args.push(OsString::from("-i"));
        args.push(path.as_os_str().to_os_string());
    }
    // Le décalage s'applique aux échantillons, après le décodage. `-itsoffset`
    // posé devant l'entrée était perdu par le filtre `aresample` : le son
    // gardait seulement l'avance de l'encodeur AAC, pas les 66 ms mesurés au
    // lancement du capteur. `adelay` est exact et garde le début à zéro.
    let filter = if tracks.len() > 1 {
        let mut parts = Vec::new();
        for (index, (_, offset)) in tracks.iter().enumerate() {
            parts.push(format!(
                "[{}:a]adelay={offset}:all=1[a{}]",
                index + 1,
                index + 1
            ));
        }
        let labels: String = (1..=tracks.len())
            .map(|index| format!("[a{index}]"))
            .collect();
        // `apad` complète en silence : une sortie muette un moment ne doit pas
        // raccourcir la vidéo au moment du mixage.
        // `aresample=async` absorbe la dérive entre l'horloge de l'image et
        // celle de PipeWire : sans elle, le son décroche lentement même quand
        // le départ est juste.
        parts.push(format!(
            "{labels}amix=inputs={}:duration=longest,apad,aresample=async=1[a]",
            tracks.len()
        ));
        parts.join(";")
    } else {
        let delay = tracks.first().map(|(_, offset)| *offset).unwrap_or(0);
        if delay > 0 {
            format!("adelay={delay}:all=1,apad,aresample=async=1")
        } else {
            "apad,aresample=async=1".to_string()
        }
    };
    if tracks.len() > 1 {
        args.push(OsString::from("-filter_complex"));
        args.push(OsString::from(filter));
        for arg in ["-map", "0:v", "-map", "[a]"] {
            args.push(OsString::from(arg));
        }
    } else {
        for arg in ["-map", "0:v", "-map", "1:a", "-af"] {
            args.push(OsString::from(arg));
        }
        args.push(OsString::from(filter));
    }
    for arg in [
        "-c:v",
        "copy",
        "-c:a",
        "aac",
        "-b:a",
        "192k",
        "-shortest",
        "-movflags",
        "+faststart",
    ] {
        args.push(OsString::from(arg));
    }
    args.push(out.as_os_str().to_os_string());
    args
}

/// Ajoute le son à la vidéo muette. En cas d'échec la vidéo reste en place,
/// sans audio : une capture sans son vaut mieux qu'une capture perdue.
fn mux_audio(video: &Path, tracks: &[(PathBuf, u64)]) -> std::result::Result<(), String> {
    let Some(ffmpeg) = lapinux_core::paths::which("ffmpeg") else {
        return Err("ffmpeg absent : la vidéo reste sans son".into());
    };
    let stem = video
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_default();
    let mixed = video.with_file_name(format!("{stem}.son.mp4"));
    let status = Command::new(ffmpeg)
        .args(mux_args(video, tracks, &mixed))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|err| format!("ffmpeg (son) : {err}"))?;
    if !status.success() {
        let _ = std::fs::remove_file(&mixed);
        return Err(format!("ffmpeg (son) : sortie {status}"));
    }
    std::fs::rename(&mixed, video).map_err(|err| format!("vidéo sonorisée : {err}"))
}

/// Coupe le noir qui suit la dernière image du jeu. La scène persistante
/// laisse GameScope afficher une surface noire pendant que Proton se ferme :
/// sans ce nettoyage, la vidéo garde une à trois secondes de noir à la fin.
/// La coupe est faite par réencodage (2 à 3 s sur 1080p) parce qu'une copie
/// de flux s'arrêterait à l'image clé précédente, donc encore dans le noir.
fn trim_trailing_black(path: &Path, duration: f64) -> Option<f64> {
    let ffmpeg = lapinux_core::paths::which("ffmpeg")?;
    let output = Command::new(&ffmpeg)
        .args(["-hide_banner", "-i"])
        .arg(path)
        .args([
            "-vf",
            "blackdetect=d=0.5:pix_th=0.10",
            "-an",
            "-f",
            "null",
            "-",
        ])
        .output()
        .ok()?;
    let report = String::from_utf8_lossy(&output.stderr);
    let mut black_start = None;
    for line in report.lines() {
        let (Some(start), Some(end)) = (
            number_after(line, "black_start:"),
            number_after(line, "black_end:"),
        ) else {
            continue;
        };
        if end >= duration - 0.30 {
            black_start = Some(start);
        }
    }
    let start = black_start?;
    let removed = duration - start;
    // Une queue courte est un écran de fermeture. Une longue séquence noire
    // peut être un choix du jeu : on ne la coupe pas.
    if !(0.8..=4.0).contains(&removed) {
        return None;
    }
    let cut = (start - 0.08).clamp(0.5, duration);
    let stem = path.file_stem()?.to_string_lossy();
    let temporary = path.with_file_name(format!("{stem}.coupe.mp4"));
    let status = Command::new(&ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(path)
        .args([
            "-t",
            &format!("{cut:.3}"),
            "-an",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-crf",
            "20",
            "-pix_fmt",
            "yuv420p",
            "-movflags",
            "+faststart",
        ])
        .arg(&temporary)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok()?;
    if !status.success() {
        let _ = std::fs::remove_file(&temporary);
        return None;
    }
    if std::fs::rename(&temporary, path).is_err() {
        let _ = std::fs::remove_file(&temporary);
        return None;
    }
    Some(removed)
}

/// Lit `clé:nombre` dans une ligne de `blackdetect`.
fn number_after(line: &str, key: &str) -> Option<f64> {
    let start = line.find(key)? + key.len();
    line[start..]
        .split_whitespace()
        .next()?
        .trim_end_matches(',')
        .parse()
        .ok()
}

/// Nœuds PipeWire qui portent le son du jeu. Un jeu Proton en ouvre souvent
/// deux — effets et musique — et les deux doivent être réglés : n'en garder
/// qu'un laissait le son muet quand le second était le bon.
pub(crate) fn game_audio_nodes(pids: &[u32]) -> Vec<(u64, Option<u64>, String)> {
    let Some(dump_tool) = lapinux_core::paths::which("pw-dump") else {
        return Vec::new();
    };
    let Ok(output) = Command::new(dump_tool).output() else {
        return Vec::new();
    };
    let Ok(dump) = serde_json::from_slice(&output.stdout) else {
        return Vec::new();
    };
    audio_nodes_from_dump(&dump, pids)
}

/// Croise les objets PipeWire : un flux audio de sortie appartient à un
/// *client*, et c'est le client qui porte le pid du processus. On relie donc
/// les clients du jeu à leurs flux de sortie.
fn audio_nodes_from_dump(
    dump: &serde_json::Value,
    pids: &[u32],
) -> Vec<(u64, Option<u64>, String)> {
    let Some(objects) = dump.as_array() else {
        return Vec::new();
    };
    // `pipewire.sec.pid` est le pid vu par le serveur : c'est le seul qui
    // reste juste quand le jeu tourne dans un bac à sable (Steam Linux
    // Runtime), où `application.process.id` est un pid de son propre espace
    // de noms. Le second sert de repli pour les clients sans sécurité
    // PipeWire.
    let belongs = |props: &serde_json::Value| {
        ["pipewire.sec.pid", "application.process.id"]
            .iter()
            .filter_map(|key| props.get(*key).and_then(|value| value.as_u64()))
            .any(|pid| pids.contains(&(pid as u32)))
    };
    let mut clients: Vec<u64> = Vec::new();
    for object in objects {
        if object.get("type").and_then(|value| value.as_str()) != Some("PipeWire:Interface:Client")
        {
            continue;
        }
        let Some(props) = object.pointer("/info/props") else {
            continue;
        };
        if belongs(props) {
            if let Some(id) = object.get("id").and_then(|value| value.as_u64()) {
                clients.push(id);
            }
        }
    }
    // Un jeu Proton ouvre parfois deux flux : celui qui joue vraiment est
    // `running`, l'autre reste `suspended` et ne livre que du silence.
    // Prendre le premier trouvé donnait une piste muette sur MOTORSLICE :
    // tous les candidats sont collectés, et l'état vivant passe devant.
    let mut candidates: Vec<(bool, String, u64, Option<u64>, String)> = Vec::new();
    for object in objects {
        let Some(props) = object.pointer("/info/props") else {
            continue;
        };
        if props.get("media.class").and_then(|value| value.as_str()) != Some("Stream/Output/Audio")
        {
            continue;
        }
        let own = belongs(props);
        let by_client = props
            .get("client.id")
            .and_then(|value| value.as_u64())
            .map(|id| clients.contains(&id))
            .unwrap_or(false);
        if !(own || by_client) {
            continue;
        }
        let Some(id) = object.get("id").and_then(|value| value.as_u64()) else {
            continue;
        };
        let label = [
            "application.process.binary",
            "application.name",
            "media.name",
        ]
        .iter()
        .find_map(|key| props.get(*key).and_then(|value| value.as_str()))
        .unwrap_or("?")
        .to_string();
        let serial = props.get("object.serial").and_then(|value| value.as_u64());
        let state = object
            .pointer("/info/state")
            .and_then(|value| value.as_str())
            .unwrap_or("inconnu")
            .to_string();
        candidates.push((state == "running", label, id, serial, state));
    }
    if candidates.len() > 1 {
        let listing = candidates
            .iter()
            .map(|candidate| format!("{} {} ({})", candidate.2, candidate.1, candidate.4))
            .collect::<Vec<_>>()
            .join(", ");
        // Niveau debug : le relevé du volume passe ici chaque seconde, et
        // deux flux sont la norme pour un jeu Proton.
        tracing::debug!(
            "son du jeu : {} flux candidats : {listing}",
            candidates.len()
        );
    }
    // Les flux vivants d'abord ; `sort_by_key` est stable, l'ordre des
    // candidats égaux ne change pas.
    candidates.sort_by_key(|candidate| !candidate.0);
    candidates
        .into_iter()
        .map(|(_, label, id, serial, _)| (id, serial, label))
        .collect()
}

/// Nœuds PipeWire retenus le temps d'un glissement : `pw-dump` est bien plus
/// coûteux qu'un `wpctl set-volume`, et les nœuds ne changent pas au fil des
/// pas.
const AUDIO_CACHE_TTL: Duration = Duration::from_millis(750);

struct CachedAudioNodes {
    pids: Vec<u32>,
    nodes: Vec<(u64, Option<u64>, String)>,
    checked: Instant,
}

static AUDIO_CACHE: OnceLock<Mutex<Option<CachedAudioNodes>>> = OnceLock::new();

fn cached_game_audio_nodes(pids: &[u32]) -> Vec<(u64, Option<u64>, String)> {
    let cache = AUDIO_CACHE.get_or_init(|| Mutex::new(None));
    if let Ok(guard) = cache.lock() {
        if let Some(entry) = guard.as_ref() {
            if entry.pids.as_slice() == pids && entry.checked.elapsed() < AUDIO_CACHE_TTL {
                return entry.nodes.clone();
            }
        }
    }
    let nodes = game_audio_nodes(pids);
    if let Ok(mut guard) = cache.lock() {
        *guard = Some(CachedAudioNodes {
            pids: pids.to_vec(),
            nodes: nodes.clone(),
            checked: Instant::now(),
        });
    }
    nodes
}

/// Volume effectif le plus haut des flux du jeu. `None` quand aucun flux ne
/// lui appartient encore ou quand `wpctl` ne répond pas. Un jeu Proton peut
/// exposer deux flux : le plus haut est celui que l'utilisateur entend.
pub(crate) fn game_volume(pids: &[u32]) -> Option<f32> {
    let mut loudest: Option<f32> = None;
    for (node, _, _) in cached_game_audio_nodes(pids) {
        if let Some(volume) = read_wpctl_volume(node) {
            loudest = Some(loudest.map_or(volume, |current| current.max(volume)));
        }
    }
    loudest
}

/// Applique le volume à **tous** les flux du jeu et renvoie la valeur
/// effective. `wpctl` adresse directement chaque nœud PipeWire : les autres
/// applications ne changent pas de volume.
pub(crate) fn set_game_volume(pids: &[u32], volume: f32) -> std::result::Result<f32, String> {
    let nodes = cached_game_audio_nodes(pids);
    if nodes.is_empty() {
        return Err("aucun flux audio du jeu".into());
    }
    let Some(tool) = lapinux_core::paths::which("wpctl") else {
        return Err("wpctl introuvable".into());
    };
    let value = volume.clamp(0.0, 1.0);
    let mut applied = false;
    let mut last_error = String::new();
    for (node, _, label) in &nodes {
        let node = node.to_string();
        match Command::new(&tool)
            .args(["set-volume", &node, &format!("{value:.2}")])
            .output()
        {
            Ok(output) if output.status.success() => {
                applied = true;
                // Un flux peut rester muet alors que son volume est non nul :
                // sans ce rétablissement, le curseur du menu semblait agir
                // sans rendre le son.
                if value > 0.0 {
                    let _ = Command::new(&tool).args(["set-mute", &node, "0"]).output();
                }
            }
            Ok(output) => {
                let error = String::from_utf8_lossy(&output.stderr);
                last_error = format!("wpctl ({label}) : {}", error.trim());
            }
            Err(error) => last_error = format!("wpctl : {error}"),
        }
    }
    if applied {
        Ok(value)
    } else {
        Err(last_error)
    }
}

/// Rétablit le son de **tous** les flux du jeu restés muets d'une session
/// précédente. Un flux dont le volume stocké vaut zéro reste silencieux même
/// une fois démuté : c'est le cas d'un jeu Proton dont PipeWire a gardé un
/// ancien 0 %. La sourdine est levée et le volume remis à 100 % uniquement
/// dans ce cas ; un volume choisi par l'utilisateur est conservé.
pub(crate) fn ensure_game_audio(pids: &[u32]) -> bool {
    let nodes = cached_game_audio_nodes(pids);
    if nodes.is_empty() {
        return false;
    }
    let Some(tool) = lapinux_core::paths::which("wpctl") else {
        return false;
    };
    let mut restored = false;
    for (node, _, label) in &nodes {
        let node_text = node.to_string();
        match Command::new(&tool)
            .args(["set-mute", &node_text, "0"])
            .output()
        {
            Ok(output) if output.status.success() => {
                if read_wpctl_volume(*node).is_some_and(|volume| volume <= 0.001) {
                    let _ = Command::new(&tool)
                        .args(["set-volume", &node_text, "1.00"])
                        .output();
                    let _ = Command::new(&tool)
                        .args(["set-mute", &node_text, "0"])
                        .output();
                    tracing::info!("son du jeu : flux « {label} » remis à 100 %");
                } else {
                    tracing::info!("son du jeu : flux « {label} » rétabli");
                }
                restored = true;
            }
            Ok(output) => {
                let error = String::from_utf8_lossy(&output.stderr);
                tracing::debug!("son du jeu : sourdine non levée ({})", error.trim());
            }
            Err(error) => {
                tracing::debug!("son du jeu : wpctl indisponible ({error})");
            }
        }
    }
    restored
}

fn read_wpctl_volume(node: u64) -> Option<f32> {
    let tool = lapinux_core::paths::which("wpctl")?;
    let output = Command::new(tool)
        .args(["get-volume", &node.to_string()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_wpctl_volume(&String::from_utf8_lossy(&output.stdout))
}

/// `Volume: 0.50 [MUTED]` -> `0.0`, `Volume: 0.50` -> `0.5`. Le volume
/// affiché est le volume effectif : un flux muet ne doit pas paraître à
/// moitié sonore dans le menu.
fn parse_wpctl_volume(text: &str) -> Option<f32> {
    let value = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("Volume:"))?
        .split_whitespace()
        .next()?
        .parse::<f32>()
        .ok()?;
    if text.contains("[MUTED]") {
        return Some(0.0);
    }
    Some(value.clamp(0.0, 1.0))
}

/// Flux audio du bureau, résumés pour le journal : quand aucun ne correspond
/// au jeu, c'est cette liste qui dit pourquoi.
fn audio_stream_listing() -> String {
    let Some(tool) = lapinux_core::paths::which("pw-dump") else {
        return "pw-dump absent".into();
    };
    let Ok(output) = Command::new(tool).output() else {
        return "pw-dump illisible".into();
    };
    let Ok(dump) = serde_json::from_slice::<serde_json::Value>(&output.stdout) else {
        return "pw-dump illisible".into();
    };
    let mut names = Vec::new();
    for object in dump.as_array().into_iter().flatten() {
        let Some(props) = object.pointer("/info/props") else {
            continue;
        };
        if props.get("media.class").and_then(|value| value.as_str()) != Some("Stream/Output/Audio")
        {
            continue;
        }
        let name = [
            "application.process.binary",
            "application.name",
            "media.name",
        ]
        .iter()
        .find_map(|key| props.get(*key).and_then(|value| value.as_str()))
        .unwrap_or("?");
        names.push(name.to_string());
    }
    if names.is_empty() {
        "aucun".into()
    } else {
        names.join(", ")
    }
}

/// Ramène la zone de la fenêtre dans son écran : KWin peut rapporter une
/// fenêtre qui déborde légèrement, et une capture hors écran échouerait.
fn subject_area(
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    screen_x: i32,
    screen_y: i32,
    screen_width: i32,
    screen_height: i32,
) -> Area {
    let mut left = x;
    let mut top = y;
    let mut right = x + width.max(16);
    let mut bottom = y + height.max(16);
    if screen_width > 0 && screen_height > 0 {
        left = left.max(screen_x);
        top = top.max(screen_y);
        right = right.min(screen_x + screen_width);
        bottom = bottom.min(screen_y + screen_height);
    }
    Area {
        x: left,
        y: top,
        width: (right - left).max(16) as u32,
        height: (bottom - top).max(16) as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_capture_du_jeu_prime_et_revient_au_global_au_defaut() {
        let dir = tempfile::tempdir().unwrap();
        let paths = lapinux_core::paths::LapinuxPaths::under(dir.path());
        let shared = Shared::new(&paths).unwrap();
        let mut config = lapinux_core::Config::default();
        config.capture.video_fps = 30;
        config.capture.video_profile = "balanced".into();
        config.capture.record_audio = true;

        let mut game = lapinux_core::model::Game::default();
        game.appid = 4242;
        game.settings.video_fps = Some(60);
        game.settings.video_profile = Some("quality".into());
        game.settings.record_audio = Some(false);
        shared.store.save_game(&game).unwrap();

        let capture = shared.capture_for(4242, &config);
        assert_eq!(capture.video_fps, 60);
        assert_eq!(capture.video_profile, "quality");
        assert!(!capture.record_audio);

        game.settings.video_fps = None;
        game.settings.video_profile = None;
        game.settings.record_audio = None;
        shared.store.save_game(&game).unwrap();
        let inherited = shared.capture_for(4242, &config);
        assert_eq!(inherited.video_fps, 30);
        assert_eq!(inherited.video_profile, "balanced");
        assert!(inherited.record_audio);
    }

    #[test]
    fn la_resolution_de_sortie_ne_grandit_jamais_la_fenetre() {
        assert_eq!(capture_size(1920, 1080, "720p"), (1280, 720));
        assert_eq!(capture_size(2560, 1440, "1080p"), (1920, 1080));
        assert_eq!(capture_size(2560, 1440, "1440p"), (2560, 1440));
        assert_eq!(capture_size(1920, 1080, "1440p"), (1920, 1080));
        assert_eq!(capture_size(1280, 720, "1080p"), (1280, 720));
        assert_eq!(capture_size(1921, 1081, "native"), (1920, 1080));
        assert_eq!(capture_size(1920, 1080, "native"), (1920, 1080));
    }

    #[test]
    fn le_filtre_de_redimensionnement_suit_le_profil() {
        let target = capture_size(1920, 1080, "720p");
        let fast = video_filter(1920, 1080, target, VideoProfile::Performance).unwrap();
        assert!(fast.contains("scale=1280:720"));
        assert!(fast.contains("fast_bilinear"));
        assert!(video_filter(1920, 1080, (1920, 1080), VideoProfile::Balanced).is_none());
        let odd = video_filter(1919, 1079, (1919 & !1, 1079 & !1), VideoProfile::Balanced);
        assert!(odd.is_some());
    }

    #[test]
    fn une_fenetre_plein_ecran_garde_l_ecran_entier() {
        let area = subject_area(1920, 0, 1920, 1080, 1920, 0, 1920, 1080);
        assert_eq!(
            area,
            Area {
                x: 1920,
                y: 0,
                width: 1920,
                height: 1080
            }
        );
    }

    #[test]
    fn une_fenetre_plus_petite_garde_son_rectangle() {
        let area = subject_area(2120, 150, 1280, 720, 1920, 0, 1920, 1080);
        assert_eq!(
            area,
            Area {
                x: 2120,
                y: 150,
                width: 1280,
                height: 720
            }
        );
    }

    #[test]
    fn une_fenetre_debordante_reste_dans_son_ecran() {
        let area = subject_area(2020, 0, 1920, 1080, 1920, 0, 1920, 1080);
        assert_eq!(area.x, 2020);
        assert_eq!(area.width, 1820);
        assert_eq!(area.height, 1080);
    }

    #[test]
    fn le_son_du_jeu_se_choisit_sur_son_client() {
        let dump: serde_json::Value = serde_json::from_str(
            r#"[
                {"id": 5, "type": "PipeWire:Interface:Client", "info": {"props": {"application.process.id": 42}}},
                {"id": 6, "type": "PipeWire:Interface:Client", "info": {"props": {"application.process.id": 7}}},
                {"id": 9, "type": "PipeWire:Interface:Node", "info": {"props": {"media.class": "Stream/Output/Audio", "client.id": 5}}},
                {"id": 11, "type": "PipeWire:Interface:Node", "info": {"props": {"media.class": "Stream/Output/Audio", "client.id": 6}}},
                {"id": 13, "type": "PipeWire:Interface:Node", "info": {"props": {"media.class": "Audio/Sink", "client.id": 5}}}
            ]"#,
        )
        .unwrap();
        // Le jeu est le pid 42 : son flux, et lui seul.
        assert_eq!(
            audio_nodes_from_dump(&dump, &[1, 42])
                .into_iter()
                .map(|(node, _, _)| node)
                .collect::<Vec<_>>(),
            vec![9]
        );
        // Un autre jeu ne prend pas le flux d'un voisin.
        assert!(audio_nodes_from_dump(&dump, &[1, 2]).is_empty());
        // Ni une sortie matérielle, même portée par le même client.
        assert!(!audio_nodes_from_dump(&dump, &[42])
            .into_iter()
            .any(|(node, _, _)| node == 13));
    }

    #[test]
    fn le_pid_du_bac_a_sable_ne_perd_pas_le_son() {
        // Jeu dans un conteneur Steam : son pid vu de l'intérieur n'existe pas
        // sur la machine, seul `pipewire.sec.pid` le retrouve.
        let dump: serde_json::Value = serde_json::from_str(
            r#"[
                {"id": 3, "type": "PipeWire:Interface:Client", "info": {"props": {"application.process.id": 7, "pipewire.sec.pid": 4242}}},
                {"id": 4, "type": "PipeWire:Interface:Node", "info": {"props": {"media.class": "Stream/Output/Audio", "client.id": 3}}}
            ]"#,
        )
        .unwrap();
        assert_eq!(
            audio_nodes_from_dump(&dump, &[4242])
                .first()
                .map(|(node, _, _)| *node),
            Some(4)
        );
    }

    #[test]
    fn le_son_s_ajoute_sans_reecoder_l_image() {
        let video = Path::new("/tmp/lapinux/mp4/partie.mp4");
        let son = Path::new("/tmp/lapinux/mp4/partie.son.pcm").to_path_buf();
        let micro = Path::new("/tmp/lapinux/mp4/partie.micro.pcm").to_path_buf();
        let out = Path::new("/tmp/lapinux/mp4/partie.son.mp4");
        let seul = mux_args(video, &[(son.clone(), 0)], out)
            .iter()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        assert!(seul.windows(2).any(|pair| pair == ["-c:v", "copy"]));
        assert!(seul.windows(2).any(|pair| pair == ["-map", "1:a"]));
        assert!(!seul.iter().any(|arg| arg == "-filter_complex"));
        // Le format vient du WAV, il n'est plus supposé.
        assert!(!seul.iter().any(|arg| arg == "-f"));
        // Deux sources : le micro et le jeu sont mélangés, l'image copiée.
        // Décalages en millisecondes : 3000 = trois secondes de retard.
        let deux = mux_args(video, &[(son.clone(), 3000), (micro, 0)], out)
            .iter()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        assert!(deux.iter().any(|arg| arg
            == "[1:a]adelay=3000:all=1[a1];[2:a]adelay=0:all=1[a2];[a1][a2]amix=inputs=2:duration=longest,apad,aresample=async=1[a]"));
        // Le silence comble l'écart : sans lui, `-shortest` couperait l'image.
        // `first_pts=0` est interdit : il collait un son tardif au début de la
        // vidéo et annulait le décalage calculé par le capteur.
        assert!(seul
            .windows(2)
            .any(|pair| pair == ["-af", "apad,aresample=async=1"]));
        assert!(!seul.iter().any(|arg| arg.contains("first_pts=0")));
        assert!(!deux.iter().any(|arg| arg.contains("first_pts=0")));
        // Un flux en retard est décalé par `adelay`, pas collé au début. Un
        // `-itsoffset` serait mangé par `aresample` et laisserait le son en
        // avance de la valeur calculée.
        assert!(!deux.iter().any(|arg| arg == "-itsoffset"));
        let un_seul_retard = mux_args(video, &[(son, 1500)], out)
            .iter()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        assert!(un_seul_retard
            .windows(2)
            .any(|pair| pair == ["-af", "adelay=1500:all=1,apad,aresample=async=1"]));
        assert!(deux.windows(2).any(|pair| pair == ["-c:v", "copy"]));
        assert_eq!(deux.first().map(String::as_str), Some("-hide_banner"));
        assert_eq!(
            deux.last().map(String::as_str),
            Some(out.to_string_lossy().as_ref())
        );
    }

    #[test]
    fn le_fichier_d_autorisation_declare_l_interface_kwin() {
        let desktop = permission_desktop(Path::new("/home/joueur/lapinux"));
        assert!(desktop.contains("X-KDE-DBUS-Restricted-Interfaces=org.kde.KWin.ScreenShot2"));
        assert!(desktop.contains("Exec=/home/joueur/lapinux"));
    }

    #[test]
    fn le_flux_audio_actif_passe_devant_le_flux_endormi() {
        // MOTORSLICE exposait deux flux `wine64-preloader` : le premier de la
        // liste ne jouait rien, et la vidéo sortait muette.
        let dump = serde_json::json!([
            {
                "id": 10,
                "type": "PipeWire:Interface:Stream",
                "info": {
                    "state": "suspended",
                    "props": {
                        "media.class": "Stream/Output/Audio",
                        "pipewire.sec.pid": 4242,
                        "object.serial": 111,
                        "application.process.binary": "wine64-preloader"
                    }
                }
            },
            {
                "id": 11,
                "type": "PipeWire:Interface:Stream",
                "info": {
                    "state": "running",
                    "props": {
                        "media.class": "Stream/Output/Audio",
                        "pipewire.sec.pid": 4242,
                        "object.serial": 222,
                        "application.process.binary": "wine64-preloader"
                    }
                }
            }
        ]);
        let nodes = audio_nodes_from_dump(&dump, &[4242]);
        // Les deux flux du jeu sont conservés : c'est le premier, le vivant,
        // qui doit passer devant. N'en garder qu'un laissait le son muet
        // quand le second portait la musique et les effets.
        assert_eq!(nodes.len(), 2);
        assert_eq!(
            nodes.first().cloned(),
            Some((11, Some(222), "wine64-preloader".to_string()))
        );
    }

    #[test]
    fn un_seul_flux_endormi_reste_utilisable() {
        let dump = serde_json::json!([{
            "id": 12,
            "type": "PipeWire:Interface:Stream",
            "info": {
                "state": "suspended",
                "props": {
                    "media.class": "Stream/Output/Audio",
                    "pipewire.sec.pid": 4242,
                    "object.serial": 333,
                    "application.process.binary": "wine64-preloader"
                }
            }
        }]);
        assert_eq!(
            audio_nodes_from_dump(&dump, &[4242]).first().cloned(),
            Some((12, Some(333), "wine64-preloader".to_string()))
        );
    }

    #[test]
    fn une_piste_wav_muette_est_reconnue() {
        let dir = tempfile::tempdir().unwrap();
        let silent = dir.path().join("silence.wav");
        std::fs::write(&silent, wav_for_test(&[0, 1, -2, 0])).unwrap();
        assert_eq!(wav_peak(&silent), Some(2));
        let audible = dir.path().join("son.wav");
        std::fs::write(&audible, wav_for_test(&[0, 1000, -2000, 300])).unwrap();
        assert_eq!(wav_peak(&audible), Some(2000));
        // Un WAV tronqué ne fait pas échouer le mixage : la piste est
        // simplement considérée comme vide.
        let broken = dir.path().join("court.wav");
        std::fs::write(&broken, b"RIFF").unwrap();
        assert_eq!(wav_peak(&broken), None);
    }

    #[test]
    fn le_noir_final_est_coupe() {
        let Some(ffmpeg) = lapinux_core::paths::which("ffmpeg") else {
            return;
        };
        let Some(ffprobe) = lapinux_core::paths::which("ffprobe") else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let video = dir.path().join("partie.mp4");
        let status = Command::new(&ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "color=c=red:s=320x180:d=1:r=30",
                "-f",
                "lavfi",
                "-i",
                "color=c=black:s=320x180:d=1:r=30",
                "-filter_complex",
                "[0:v][1:v]concat=n=2:v=1:a=0[v]",
                "-map",
                "[v]",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&video)
            .status()
            .unwrap();
        assert!(status.success());
        let removed = trim_trailing_black(&video, 2.0).expect("la queue noire doit être détectée");
        assert!((removed - 1.0).abs() < 0.2, "queue retirée : {removed}");
        let output = Command::new(ffprobe)
            .args([
                "-v",
                "error",
                "-show_entries",
                "format=duration",
                "-of",
                "default=nw=1:nk=1",
            ])
            .arg(&video)
            .output()
            .unwrap();
        let duration: f64 = String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse()
            .unwrap();
        assert!(duration < 1.5, "la vidéo garde du noir : {duration}");
    }

    fn wav_for_test(samples: &[i16]) -> Vec<u8> {
        let data_len = (samples.len() * 2) as u32;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&48_000u32.to_le_bytes());
        bytes.extend_from_slice(&96_000u32.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_len.to_le_bytes());
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn le_volume_wpctl_est_lu() {
        assert_eq!(parse_wpctl_volume("Volume: 0.50 [MUTED]\n"), Some(0.0));
        assert_eq!(parse_wpctl_volume("Volume: 1.00\n"), Some(1.0));
        assert_eq!(parse_wpctl_volume("Volume: 0.00\n"), Some(0.0));
        assert_eq!(parse_wpctl_volume("aucun volume"), None);
    }

    #[test]
    fn un_enregistrement_orphelin_est_close_et_la_trace_disparait() {
        let dir = tempfile::tempdir().unwrap();
        let video = dir.path().join("partie.mp4");
        std::fs::write(&video, b"moov").unwrap();
        let journal = serde_json::json!({
            "daemon_pid": 0,
            "ffmpeg_pid": 0,
            "path": video.to_string_lossy(),
            "appid": 1636440,
            "audio_pids": [0],
        });
        std::fs::write(dir.path().join(RECORDER_JOURNAL), journal.to_string()).unwrap();
        recover_orphan_recording_at(dir.path());
        // La vidéo reste en place : c'est elle qu'on récupère.
        assert!(video.is_file());
        assert!(
            !dir.path().join(RECORDER_JOURNAL).exists(),
            "la trace d'un enregistrement mort est retirée"
        );
    }

    #[test]
    fn la_trace_d_un_daemon_vivant_est_laissee() {
        let dir = tempfile::tempdir().unwrap();
        let journal = serde_json::json!({
            "daemon_pid": std::process::id(),
            "ffmpeg_pid": 0,
            "path": "",
            "appid": 0,
            "audio_pids": [],
        });
        let path = dir.path().join(RECORDER_JOURNAL);
        std::fs::write(&path, journal.to_string()).unwrap();
        recover_orphan_recording_at(dir.path());
        assert!(path.is_file(), "un daemon vivant garde sa trace");
    }
}
