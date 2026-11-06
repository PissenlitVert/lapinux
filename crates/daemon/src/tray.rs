//! Icône de notification (StatusNotifierItem) portée par le daemon.
//!
//! L'interface peut être fermée pour rendre sa mémoire : le tray reste, avec
//! le visuel du jeu en cours ou le logo Lapinux, et la relance depuis son menu.
//! Un clic gauche affiche l'interface, un clic droit ouvre les jeux récents,
//! « Afficher/Masquer Lapinux » et « Quitter Lapinux ».

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::Shared;
use lapinux_core::paths::LapinuxPaths;
use zbus::blocking::connection::Builder;
use zbus::blocking::Proxy;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

const OBJECT_PATH: &str = "/StatusNotifierItem";
const MENU_PATH: &str = "/MenuBar";
const INTERFACE: &str = "org.kde.StatusNotifierItem";
const MENU_INTERFACE: &str = "com.canonical.dbusmenu";
const WATCHER_NAME: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";
const WATCHER_INTERFACE: &str = "org.kde.StatusNotifierWatcher";
/// Taille de l'icône envoyée : 64 px couvre un écran HiDPI sans transporter
/// une image entière dans une propriété D-Bus.
const ICON_PX: u32 = 64;
/// Côté de l'icône PNG posée devant chaque jeu du menu clic droit.
const MENU_ICON_PX: u32 = 32;
/// Le tray est un élément de fond : un réveil toutes les trois secondes
/// suffit à suivre une partie qui démarre ou se termine, sans tenir un
/// cœur éveillé en permanence.
const TICK: Duration = Duration::from_secs(3);
/// Le menu des jeux récents n'a pas besoin de suivre chaque scan : il est
/// relu une fois par minute, et toutes les trente secondes tant qu'il est vide.
const RECENT_REFRESH: Duration = Duration::from_secs(60);
const RECENT_EMPTY_RETRY: Duration = Duration::from_secs(30);
/// Premier identifiant des jeux récents dans le menu. Les identifiants 0 à 2
/// sont réservés au menu lui-même.
const MENU_TOGGLE: i32 = 1;
const MENU_SEPARATOR: i32 = 2;
const MENU_QUIT: i32 = 3;
const RECENT_BASE: i32 = 100;
const TOGGLE_ICON: &str = "window-minimize";
const SHOW_ICON: &str = "window-restore";
const QUIT_ICON: &str = "application-exit";

/// Ce qu'un clic sur l'icône ou dans son menu demande au daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    /// Afficher l'interface : la relancer, ou la ramener si elle tourne déjà.
    Show,
    /// Lancer un jeu directement depuis le menu.
    Launch(u32),
    /// Fermer Lapinux.
    Quit,
}

/// Ce que le fil D-Bus partage avec l'interface : le visuel, le nom et
/// l'infobulle, modifiables pendant que le service est publié.
#[derive(Default)]
struct Art {
    pixmap: Mutex<Vec<(i32, i32, Vec<u8>)>>,
    title: Mutex<String>,
}

/// Un jeu récent du menu, avec son icône PNG déjà prête pour D-Bus.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct RecentGame {
    pub appid: u32,
    pub name: String,
    /// Vide quand le cache local n'a pas encore d'icône pour ce jeu.
    pub icon: Vec<u8>,
}

/// Le menu D-Bus et sa liste de jeux récents, partagés entre le service et
/// le fil qui reçoit les mises à jour de l'interface.
struct MenuData {
    recent: Mutex<Vec<RecentGame>>,
    visible: AtomicBool,
    revision: AtomicU32,
}

impl Default for MenuData {
    fn default() -> Self {
        Self {
            recent: Mutex::new(Vec::new()),
            // Le daemon peut démarrer sans interface ; le premier relevé du
            // marqueur confirme l'état réel.
            visible: AtomicBool::new(false),
            revision: AtomicU32::new(0),
        }
    }
}

/// Ordre venu de l'interface : la nouvelle icône et son nom, ou la nouvelle
/// liste de jeux récents.
enum Command {
    Art {
        pixmap: Vec<(i32, i32, Vec<u8>)>,
        title: String,
    },
    Recent(Vec<RecentGame>),
    WindowVisible(bool),
}

/// Publie l'icône et son menu. Rend la main quand le bus de session ou le
/// guetteur d'icônes ne répond pas : le daemon marche sans, sans erreur.
pub fn start(shared: Arc<Shared>) {
    std::thread::spawn(move || {
        let (commands, inbox) = mpsc::channel();
        let (action_tx, actions) = mpsc::channel();
        let (ready_tx, ready) = mpsc::channel();
        std::thread::spawn(move || {
            if let Err(err) = serve(inbox, action_tx, ready_tx) {
                tracing::debug!("icône de notification : {err}");
            }
        });
        if ready.recv_timeout(Duration::from_secs(4)) != Ok(true) {
            return;
        }
        let monitor_shared = Arc::clone(&shared);
        std::thread::spawn(move || monitor(monitor_shared, commands));
        actions_loop(shared, actions);
    });
}

/// Tient le visuel et le menu à jour : jeu en cours, jeux récents et état de
/// l'interface. Aucun appel à l'interface n'est nécessaire.
fn monitor(shared: Arc<Shared>, commands: Sender<Command>) {
    let mut visible = None;
    let mut art = None;
    let mut recent = Vec::new();
    let mut recent_at = Instant::now() - RECENT_REFRESH;
    // Les icônes du menu sont décodées une fois par jeu : relire et
    // redimensionner six PNG à chaque relevé ne servait à rien.
    let mut icons = HashMap::new();
    loop {
        let running = ui_running(&shared.paths);
        if visible != Some(running) {
            visible = Some(running);
            let _ = commands.send(Command::WindowVisible(running));
        }

        let session = shared.session_info();
        let appid = session.as_ref().map(|session| session.appid);
        let title = session
            .as_ref()
            .map(|session| session.name.clone())
            .unwrap_or_else(|| lapinux_core::APP_NAME.to_string());
        if art != Some(appid) {
            art = Some(appid);
            let pixmap = pixmap(&shared.paths, appid);
            let _ = commands.send(Command::Art { pixmap, title });
        }

        let delay = if recent.is_empty() {
            RECENT_EMPTY_RETRY
        } else {
            RECENT_REFRESH
        };
        if recent_at.elapsed() >= delay {
            let list = recent_games(&shared, &mut icons);
            if list != recent {
                recent = list.clone();
                let _ = commands.send(Command::Recent(list));
            }
            recent_at = Instant::now();
        }
        std::thread::sleep(TICK);
    }
}

fn recent_games(shared: &Shared, icons: &mut HashMap<u32, Vec<u8>>) -> Vec<RecentGame> {
    let mut recent: Vec<(u32, String, i64)> = shared
        .store
        .load_games()
        .unwrap_or_default()
        .into_iter()
        .filter(|game| game.installed)
        .map(|game| (game.appid, game.name, game.last_played.unwrap_or(0)))
        .collect();
    recent.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.1.cmp(&b.1)));
    recent.truncate(6);

    // Une icône manquante n'est pas gardée : le prochain relevé la retentera,
    // au cas où la jaquette vient d'être téléchargée.
    let live: std::collections::HashSet<u32> =
        recent.iter().map(|(appid, _, _)| *appid).collect();
    icons.retain(|appid, _| live.contains(appid));
    recent
        .into_iter()
        .map(|(appid, name, _)| {
            let icon = match icons.get(&appid) {
                Some(icon) if !icon.is_empty() => icon.clone(),
                _ => {
                    let icon = menu_icon(&shared.paths, appid);
                    if !icon.is_empty() {
                        icons.insert(appid, icon.clone());
                    }
                    icon
                }
            };
            RecentGame { appid, name, icon }
        })
        .collect()
}

fn actions_loop(shared: Arc<Shared>, actions: Receiver<TrayAction>) {
    let mut last_show: Option<Instant> = None;
    while let Ok(action) = actions.recv() {
        match action {
            // Afficher ne ferme jamais : le menu porte un vrai bouton
            // « Afficher Lapinux ». Un double événement D-Bus ne doit pas
            // relancer une seconde interface pendant que la première
            // démarre, d'où la courte fenêtre de garde.
            TrayAction::Show => {
                if last_show
                    .map(|at| at.elapsed() >= Duration::from_secs(2))
                    .unwrap_or(true)
                {
                    last_show = Some(Instant::now());
                    show_ui(&shared);
                } else if ui_running(&shared.paths) {
                    let _ = lapinux_input::shortcut::reveal_lapinux();
                }
            }
            TrayAction::Launch(appid) => launch_from_tray(&shared, appid),
            TrayAction::Quit => quit(&shared),
        }
    }
}

/// Le marqueur est réécrit toutes les deux secondes tant que l'interface
/// tourne. La grâce couvre une fermeture brutale sans attendre une éternité.
fn ui_running(paths: &LapinuxPaths) -> bool {
    let fresh = std::fs::metadata(paths.ui_marker())
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.elapsed().ok())
        .map(|age| age <= Duration::from_secs(4))
        .unwrap_or(false);
    if !fresh {
        return false;
    }
    // Un marqueur frais ne suffit pas : le processus peut être un zombie
    // laissé par une fermeture, ou avoir disparu juste après sa dernière
    // écriture. Sans ce contrôle, « Afficher Lapinux » croyait l'interface
    // vivante et se contentait de la révéler — donc de ne rien faire.
    match read_pid(paths) {
        Some(pid) => process_alive(pid),
        // Le pid est écrit juste après le marqueur : son absence passagère
        // ne doit pas passer pour une interface morte.
        None => true,
    }
}

/// Vrai quand le processus existe encore et n'est pas un zombie.
fn process_alive(pid: u32) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    // Le troisième champ est l'état, après le nom entre parenthèses (qui
    // peut lui-même contenir des espaces et des parenthèses).
    let state = stat
        .rsplit_once(')')
        .and_then(|(_, rest)| rest.split_whitespace().next())
        .unwrap_or("Z");
    state != "Z"
}

fn read_pid(paths: &LapinuxPaths) -> Option<u32> {
    std::fs::read_to_string(paths.ui_pid())
        .ok()
        .and_then(|text| text.trim().parse().ok())
}

/// Le tray ne peut pas peindre l'écran d'attente : il dépose une demande
/// que l'interface lit, puis la relance si elle était fermée. Le lancement
/// direct reste le filet de sécurité si l'interface ne répond pas.
fn launch_from_tray(shared: &Arc<Shared>, appid: u32) {
    if let Err(err) = request_launch(&shared.paths, appid) {
        tracing::warn!("lancement depuis le tray : {err}");
        launch_direct(shared, appid);
        return;
    }
    // Ne pas réveiller la fenêtre principale : le splash est une fenêtre
    // séparée, créée par le fil de l'interface. L'ancien `show_ui` faisait
    // apparaître la bibliothèque avant l'écran d'attente.
    if !ui_running(&shared.paths) {
        start_ui(shared);
    }
    // Une fenêtre réduite ne peint plus : sans ce réveil, la demande reste
    // dans le vide et le splash n'est jamais créé. On la réveille **derrière**
    // les autres fenêtres : le script attend aussi la fenêtre quand elle vient
    // d'être lancée, et le splash peut apparaître sans que la bibliothèque ne
    // passe devant lui.
    let _ = lapinux_input::shortcut::wake_lapinux();
    let shared = Arc::clone(shared);
    std::thread::spawn(move || {
        // L'interface peut être rangée derrière une partie précédente : son
        // fil de peinture est à l'arrêt et ne lirait jamais la demande. On la
        // réveille tant que le fichier est là, en laissant à un démarrage à
        // froid le temps de se poser.
        for _ in 0..25 {
            let path = shared.paths.launch_request();
            if !path.is_file() {
                return;
            }
            if shared.session_info().is_some() {
                let _ = std::fs::remove_file(&path);
                return;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
        let path = shared.paths.launch_request();
        if !path.is_file() {
            return;
        }
        // L'interface n'a pas lu la demande : elle a sans doute échoué à
        // démarrer. Le jeu part quand même, sans écran d'attente.
        tracing::warn!("lancement depuis le tray : l'interface n'a pas lu la demande");
        let _ = std::fs::remove_file(&path);
        if shared.session_info().is_none() {
            launch_direct(&shared, appid);
        }
    });
}

fn request_launch(paths: &LapinuxPaths, appid: u32) -> std::io::Result<()> {
    let path = paths.launch_request();
    let temporary = path.with_extension("tmp");
    std::fs::create_dir_all(&paths.run_dir)?;
    std::fs::write(&temporary, appid.to_string())?;
    // Renommage atomique : l'interface ne lit jamais un identifiant tronqué.
    std::fs::rename(&temporary, &path)
}

fn launch_direct(shared: &Arc<Shared>, appid: u32) {
    let shared = Arc::clone(shared);
    std::thread::spawn(move || {
        if let Err(err) = shared.launch(appid, false) {
            tracing::warn!("lancement depuis le tray : {err}");
        }
    });
}

fn show_ui(shared: &Shared) {
    if ui_running(&shared.paths) {
        let _ = lapinux_input::shortcut::reveal_lapinux();
        // L'interface peut être en train de se fermer : son marqueur reste
        // frais quelques secondes. On lui laisse le temps de disparaître,
        // puis on relance une vraie fenêtre si elle n'existe plus.
        std::thread::sleep(Duration::from_millis(500));
        if ui_running(&shared.paths) {
            return;
        }
    }
    start_ui(shared);
}

/// Démarre l'interface sans chercher à ramener une fenêtre existante. Le
/// lancement depuis le tray s'en sert : c'est le splash, pas la bibliothèque,
/// qui doit apparaître.
fn start_ui(shared: &Shared) {
    cleanup_ui_markers(&shared.paths);
    let exe = match lapinux_core::client::daemon_binary() {
        Ok(exe) => exe,
        Err(err) => {
            tracing::warn!("relance de l'interface : {err}");
            return;
        }
    };
    let ui_log = shared.paths.log_dir.join("ui.log");
    lapinux_core::util::rotate_log(&ui_log);
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&ui_log);
    let mut command = ProcessCommand::new(exe);
    command.stdin(Stdio::null());
    if let Ok(log) = log {
        if let Ok(err) = log.try_clone() {
            command.stdout(Stdio::from(log)).stderr(Stdio::from(err));
        }
    } else {
        command.stdout(Stdio::null()).stderr(Stdio::null());
    }
    match command.spawn() {
        Ok(mut child) => {
            // Sans `wait`, chaque interface fermée restait un zombie dans le
            // processus du daemon ; `ui_running` la croyait alors vivante.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(err) => tracing::warn!("relance de l'interface : {err}"),
    }
}

/// Ferme l'interface pour libérer sa mémoire, puis nettoie les marqueurs si
/// elle ne répond pas. KWin ferme proprement ; le PID sert de secours.
fn close_ui(shared: &Shared) {
    if !ui_running(&shared.paths) {
        cleanup_ui_markers(&shared.paths);
        return;
    }
    let pid = read_pid(&shared.paths);
    if lapinux_input::shortcut::close_lapinux().is_err() {
        signal_ui(pid);
        cleanup_ui_markers(&shared.paths);
        return;
    }
    // La fermeture KWin est asynchrone : laisser à l'interface le temps de
    // rendre la main et de retirer son marqueur. Elle n'a rien à sauver —
    // les marqueurs et Discord se règlent sans elle — donc l'attente reste
    // courte ; au-delà, le PID fait le travail. Attendre une seconde et demie
    // donnait l'impression que « Quitter » ne répondait pas.
    for _ in 0..5 {
        if !ui_running(&shared.paths) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    signal_ui(pid);
    cleanup_ui_markers(&shared.paths);
}

fn signal_ui(pid: Option<u32>) {
    let Some(pid) = pid else { return };
    // SAFETY: un signal à un PID lu dans le fichier de l'interface ; l'échec
    // (processus déjà parti) est sans conséquence.
    unsafe {
        libc::kill(pid as i32, libc::SIGTERM);
    }
}

fn cleanup_ui_markers(paths: &LapinuxPaths) {
    let _ = std::fs::remove_file(paths.ui_marker());
    let _ = std::fs::remove_file(paths.ui_pid());
}

fn quit(shared: &Arc<Shared>) {
    close_ui(shared);
    let _ = shared.stop_session();
    let _ = std::fs::remove_file(&shared.paths.socket);
    cleanup_ui_markers(&shared.paths);
    // Si l'interface a été interrompue avant son propre `on_exit`, ce marqueur
    // la ferait passer par Mesa pendant sept jours au prochain lancement.
    let _ = std::fs::remove_file(shared.paths.data_dir.join("gui-wayland-crashed"));
    std::process::exit(0);
}

/// Sert l'icône et le menu sur le bus de session, et suit les demandes.
fn serve(
    commands: Receiver<Command>,
    on_action: Sender<TrayAction>,
    ready: Sender<bool>,
) -> Result<(), String> {
    let art = Arc::new(Art::default());
    let menu_data = Arc::new(MenuData::default());
    let item = Item {
        art: Arc::clone(&art),
        on_action: on_action.clone(),
    };
    let menu = Menu {
        data: Arc::clone(&menu_data),
        on_action,
    };
    let bus = format!("org.kde.StatusNotifierItem-{}-1", std::process::id());
    let connection = match Builder::session()
        .map_err(|err| format!("bus de session : {err}"))?
        .name(bus.clone())
        .map_err(|err| format!("nom {bus} : {err}"))?
        .serve_at(OBJECT_PATH, item)
        .map_err(|err| format!("service {INTERFACE} : {err}"))?
        .serve_at(MENU_PATH, menu)
        .map_err(|err| format!("service {MENU_INTERFACE} : {err}"))?
        .build()
    {
        Ok(connection) => connection,
        Err(err) => {
            let _ = ready.send(false);
            return Err(format!("connexion D-Bus : {err}"));
        }
    };
    // Le guetteur peut manquer (session sans barre Plasma) : l'icône est
    // simplement invisible, le service reste sans effet.
    if let Ok(watcher) = Proxy::new(&connection, WATCHER_NAME, WATCHER_PATH, WATCHER_INTERFACE) {
        let _: std::result::Result<(), _> =
            watcher.call("RegisterStatusNotifierItem", &(bus.as_str(),));
    }
    let _ = ready.send(true);
    while let Ok(command) = commands.recv() {
        match command {
            Command::Art { pixmap, title } => {
                if let Ok(mut slot) = art.pixmap.lock() {
                    *slot = pixmap;
                }
                if let Ok(mut slot) = art.title.lock() {
                    *slot = title;
                }
                // Plasma relit les propriétés sur ces signaux : sans eux,
                // l'icône garderait le visuel du premier jeu de la session.
                let _ = connection.emit_signal(
                    None::<&str>,
                    OBJECT_PATH,
                    INTERFACE,
                    "NewIcon",
                    &(),
                );
                let _ = connection.emit_signal(
                    None::<&str>,
                    OBJECT_PATH,
                    INTERFACE,
                    "NewToolTip",
                    &(),
                );
                let _ = connection.emit_signal(
                    None::<&str>,
                    OBJECT_PATH,
                    INTERFACE,
                    "NewTitle",
                    &(),
                );
            }
            Command::Recent(recent) => {
                if let Ok(mut slot) = menu_data.recent.lock() {
                    *slot = recent;
                }
                let revision = menu_data.revision.fetch_add(1, Ordering::Relaxed) + 1;
                // Le menu doit être relu : sans ce signal, un clic droit
                // garderait la liste de jeux de l'ouverture précédente.
                let _ = connection.emit_signal(
                    None::<&str>,
                    MENU_PATH,
                    MENU_INTERFACE,
                    "LayoutUpdated",
                    &(revision, 0i32),
                );
            }
            Command::WindowVisible(visible) => {
                menu_data.visible.store(visible, Ordering::Relaxed);
                let revision = menu_data.revision.fetch_add(1, Ordering::Relaxed) + 1;
                let _ = connection.emit_signal(
                    None::<&str>,
                    MENU_PATH,
                    MENU_INTERFACE,
                    "LayoutUpdated",
                    &(revision, 0i32),
                );
            }
        }
    }
    Ok(())
}

/// L'élément publié : des propriétés lues par la barre des tâches, et deux
/// appels qui reviennent à un clic.
struct Item {
    art: Arc<Art>,
    on_action: Sender<TrayAction>,
}

#[zbus::interface(name = "org.kde.StatusNotifierItem")]
impl Item {
    #[zbus(property)]
    fn category(&self) -> String {
        "ApplicationStatus".into()
    }

    #[zbus(property)]
    fn id(&self) -> String {
        "lapinux".into()
    }

    #[zbus(property)]
    fn title(&self) -> String {
        self.art
            .title
            .lock()
            .map(|title| title.clone())
            .unwrap_or_else(|_| lapinux_core::APP_NAME.to_string())
    }

    #[zbus(property)]
    fn status(&self) -> String {
        "Active".into()
    }

    #[zbus(property)]
    fn window_id(&self) -> i32 {
        0
    }

    /// Un nom vide : c'est le visuel transporté qui s'affiche, jamais une
    /// icône cherchée dans le thème.
    #[zbus(property)]
    fn icon_name(&self) -> String {
        String::new()
    }

    #[zbus(property)]
    fn icon_pixmap(&self) -> Vec<(i32, i32, Vec<u8>)> {
        self.art
            .pixmap
            .lock()
            .map(|pixmap| pixmap.clone())
            .unwrap_or_default()
    }

    #[zbus(property)]
    fn item_is_menu(&self) -> bool {
        // Faux : le clic gauche ramène Lapinux. Le menu clic droit reste
        // servi par la propriété `Menu`, sans transformer l'icône en menu.
        false
    }

    #[zbus(property)]
    fn menu(&self) -> OwnedObjectPath {
        zbus::zvariant::ObjectPath::try_from(MENU_PATH)
            .expect("chemin D-Bus constant")
            .into()
    }

    /// Infobulle : le nom affiché, rien d'autre. Le visuel y est vide pour ne
    /// pas doubler les octets envoyés à chaque changement.
    #[zbus(property)]
    fn tool_tip(&self) -> (Vec<(i32, i32, Vec<u8>)>, String, String, String) {
        let title = self
            .art
            .title
            .lock()
            .map(|title| title.clone())
            .unwrap_or_default();
        (Vec::new(), String::new(), title, String::new())
    }

    /// Clic gauche : Lapinux revient au premier plan. Le script KWin s'en
    /// charge aussi, car une fenêtre réduite ne peint plus : l'interface ne
    /// pourrait pas se rétablir d'elle-même.
    fn activate(&self, _x: i32, _y: i32) {
        let _ = self.on_action.send(TrayAction::Show);
    }

    /// Clic du milieu : même effet.
    fn secondary_activate(&self, _x: i32, _y: i32) {
        let _ = self.on_action.send(TrayAction::Show);
    }

    /// Le menu est servi par D-Bus ; ce rappel ne fait rien de plus.
    fn context_menu(&self, _x: i32, _y: i32) {}

    fn scroll(&self, _delta: i32, _orientation: &str) {}
}

/// Menu clic droit au format `com.canonical.dbusmenu`, compris par Plasma,
/// GNOME (extension) et les barres AppIndicator.
struct Menu {
    data: Arc<MenuData>,
    on_action: Sender<TrayAction>,
}

type Props = HashMap<String, OwnedValue>;

impl Menu {
    fn entries(&self) -> Vec<(i32, String, &'static str, Vec<u8>, Option<&'static str>)> {
        let visible = self.data.visible.load(Ordering::Relaxed);
        // L'entrée affiche toujours Lapinux : c'est un bouton « Afficher »,
        // jamais un « Masquer » qui fermerait la fenêtre que l'utilisateur
        // vient d'ouvrir. L'icône suit seulement l'état de la fenêtre.
        let label = lapinux_core::i18n::t("tray.show");
        let icon_name = if visible { TOGGLE_ICON } else { SHOW_ICON };
        let mut entries = Vec::new();
        if let Ok(recent) = self.data.recent.lock() {
            for (index, game) in recent.iter().enumerate() {
                entries.push((
                    RECENT_BASE + index as i32,
                    game.name.clone(),
                    "standard",
                    game.icon.clone(),
                    None,
                ));
            }
            if !recent.is_empty() {
                entries.push((
                    MENU_SEPARATOR,
                    String::new(),
                    "separator",
                    Vec::new(),
                    None,
                ));
            }
        }
        entries.push((
            MENU_TOGGLE,
            label.to_string(),
            "standard",
            Vec::new(),
            Some(icon_name),
        ));
        entries.push((
            MENU_QUIT,
            lapinux_core::i18n::t("tray.quit").to_string(),
            "standard",
            Vec::new(),
            Some(QUIT_ICON),
        ));
        entries
    }

    fn properties(&self, id: i32) -> Props {
        let mut props = Props::new();
        if id == 0 {
            insert(&mut props, "children-display", "submenu");
            return props;
        }
        if let Some((_, label, kind, icon, icon_name)) = self
            .entries()
            .into_iter()
            .find(|(entry, _, _, _, _)| *entry == id)
        {
            insert(&mut props, "label", label);
            insert(&mut props, "type", kind);
            insert(&mut props, "enabled", true);
            insert(&mut props, "visible", true);
            if !icon.is_empty() {
                insert(&mut props, "icon-data", icon);
            }
            if let Some(icon_name) = icon_name {
                insert(&mut props, "icon-name", icon_name);
            }
        }
        props
    }

    fn node(
        &self,
        id: i32,
        children: Vec<OwnedValue>,
    ) -> (i32, Props, Vec<OwnedValue>) {
        (id, self.properties(id), children)
    }
}

fn insert<T: Into<Value<'static>>>(props: &mut Props, key: &str, value: T) {
    if let Ok(value) = OwnedValue::try_from(value.into()) {
        props.insert(key.to_string(), value);
    }
}

fn child(node: (i32, Props, Vec<OwnedValue>)) -> OwnedValue {
    OwnedValue::try_from(Value::new(node)).unwrap_or_else(|_| {
        OwnedValue::try_from(Value::from(String::new())).expect("valeur de repli")
    })
}

#[zbus::interface(name = "com.canonical.dbusmenu")]
impl Menu {
    #[zbus(property, name = "Version")]
    fn version(&self) -> u32 {
        3
    }

    #[zbus(property, name = "Status")]
    fn status(&self) -> String {
        "normal".into()
    }

    #[zbus(property, name = "TextDirection")]
    fn text_direction(&self) -> String {
        "ltr".into()
    }

    fn get_layout(
        &self,
        parent_id: i32,
        _recursion_depth: i32,
        _property_names: Vec<String>,
    ) -> (u32, (i32, Props, Vec<OwnedValue>)) {
        let revision = self.data.revision.load(Ordering::Relaxed).max(1);
        let children = self
            .entries()
            .into_iter()
            .map(|(id, _, _, _, _)| child(self.node(id, Vec::new())))
            .collect();
        (revision, self.node(parent_id, children))
    }

    fn get_group_properties(
        &self,
        ids: Vec<i32>,
        _property_names: Vec<String>,
    ) -> Vec<(i32, Props)> {
        ids.into_iter()
            .map(|id| (id, self.properties(id)))
            .collect()
    }

    fn get_property(&self, id: i32, name: String) -> OwnedValue {
        self.properties(id)
            .remove(&name)
            .or_else(|| Props::new().remove(""))
            .unwrap_or_else(|| {
                OwnedValue::try_from(Value::from(String::new())).expect("valeur de repli")
            })
    }

    fn event(&self, id: i32, event_id: String, _data: OwnedValue, _timestamp: u32) {
        if event_id != "clicked" {
            return;
        }
        if id == MENU_TOGGLE {
            let _ = self.on_action.send(TrayAction::Show);
        } else if id == MENU_QUIT {
            let _ = self.on_action.send(TrayAction::Quit);
        } else if id >= RECENT_BASE {
            let index = (id - RECENT_BASE) as usize;
            if let Ok(recent) = self.data.recent.lock() {
                if let Some(game) = recent.get(index) {
                    let _ = self.on_action.send(TrayAction::Launch(game.appid));
                }
            }
        }
    }

    fn event_group(
        &self,
        events: Vec<(i32, String, OwnedValue, u32)>,
    ) -> Vec<bool> {
        for (id, event, data, timestamp) in events {
            self.event(id, event, data, timestamp);
        }
        vec![true]
    }

    fn about_to_show(&self, _id: i32) -> bool {
        false
    }

    fn about_to_show_group(&self, _ids: Vec<i32>) -> (Vec<i32>, Vec<i32>) {
        (Vec::new(), Vec::new())
    }
}

/// Visuel de l'icône : celui du jeu, ou le logo de Lapinux.
pub(crate) fn pixmap(paths: &LapinuxPaths, appid: Option<u32>) -> Vec<(i32, i32, Vec<u8>)> {
    let file = appid.and_then(|appid| icon_file(paths, appid));
    match file.as_deref().and_then(read_icon) {
        Some(pixmap) => pixmap,
        None => logo_pixmap(),
    }
}

/// Icône PNG d'une entrée de jeu du menu. Une jaquette absente ou illisible
/// rend un vecteur vide : l'entrée reste alors en texte seul.
pub(crate) fn menu_icon(paths: &LapinuxPaths, appid: u32) -> Vec<u8> {
    let Some(path) = menu_icon_file(paths, appid) else {
        return Vec::new();
    };
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    let Ok(image) = image::load_from_memory(&bytes) else {
        return Vec::new();
    };
    let small = image
        .resize_to_fill(
            MENU_ICON_PX,
            MENU_ICON_PX,
            image::imageops::FilterType::Triangle,
        )
        .to_rgba8();
    let mut png = Vec::new();
    if image::DynamicImage::ImageRgba8(small)
        .write_to(
            &mut std::io::Cursor::new(&mut png),
            image::ImageFormat::Png,
        )
        .is_err()
    {
        return Vec::new();
    }
    png
}

/// Fichier d'icône d'un jeu dans le cache local des illustrations. Aucune
/// requête réseau : une icône absente laisse le menu en texte seul.
fn icon_file(paths: &LapinuxPaths, appid: u32) -> Option<PathBuf> {
    let dir = paths.art_dir();
    for stem in [
        format!("steam-icon-{appid}"),
        format!("sgdb-icon-{appid}"),
        format!("icon-{appid}"),
    ] {
        for ext in ["jpg", "png"] {
            let path = dir.join(format!("{stem}.{ext}"));
            if path.is_file() {
                return Some(path);
            }
        }
    }
    None
}

/// Fichier le plus proche d'une icône carrée : cache Lapinux, client Steam,
/// puis jaquettes locales. L'appelant rogne l'image au carré.
pub(crate) fn menu_icon_file(paths: &LapinuxPaths, appid: u32) -> Option<PathBuf> {
    if let Some(path) = icon_file(paths, appid) {
        return Some(path);
    }
    let files = steam_librarycache_files(appid);
    if let Some(path) = file_named(&files, "icon.jpg") {
        return Some(path.clone());
    }
    if let Some(path) = files.iter().find(|path| is_icon_file(path)) {
        return Some(path.clone());
    }
    let covers = paths.covers_dir();
    for name in [
        format!("custom-{appid}.jpg"),
        format!("store-{appid}.jpg"),
        format!("{appid}.jpg"),
        format!("sgdb-{appid}.jpg"),
    ] {
        let path = covers.join(name);
        if path.is_file() {
            return Some(path);
        }
    }
    if let Some(path) = steam_cached_cover(appid) {
        return Some(path);
    }
    let art = paths.art_dir();
    for name in ["steam-capsule", "sgdb-capsule", "steam-hero", "sgdb-hero"] {
        let path = art.join(format!("{name}-{appid}.jpg"));
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

fn steam_librarycache_files(appid: u32) -> Vec<PathBuf> {
    for root in lapinux_core::paths::steam_roots() {
        let dir = root
            .join("appcache")
            .join("librarycache")
            .join(appid.to_string());
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut files = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                files.push(path);
            } else if path.is_dir() {
                if let Ok(children) = std::fs::read_dir(&path) {
                    files.extend(
                        children
                            .flatten()
                            .map(|child| child.path())
                            .filter(|path| path.is_file()),
                    );
                }
            }
        }
        if !files.is_empty() {
            return files;
        }
    }
    Vec::new()
}

fn file_named<'a>(files: &'a [PathBuf], name: &str) -> Option<&'a PathBuf> {
    files.iter().find(|path| {
        path.file_name()
            .and_then(|candidate| candidate.to_str())
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(name))
    })
}

fn is_icon_file(path: &Path) -> bool {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
        return false;
    };
    matches!(extension.as_str(), "jpg" | "png")
        && stem.len() >= 32
        && stem.chars().all(|character| character.is_ascii_hexdigit())
}

fn steam_cached_cover(appid: u32) -> Option<PathBuf> {
    let files = steam_librarycache_files(appid);
    ["library_600x900.jpg", "library_capsule.jpg"]
        .iter()
        .find_map(|name| file_named(&files, name).cloned())
}

fn read_icon(path: &Path) -> Option<Vec<(i32, i32, Vec<u8>)>> {
    let bytes = std::fs::read(path).ok()?;
    let image = image::load_from_memory(&bytes).ok()?;
    Some(vec![argb(image)])
}

fn logo_pixmap() -> Vec<(i32, i32, Vec<u8>)> {
    match image::load_from_memory(include_bytes!("../../../assets/lapinux-logo.png")) {
        Ok(image) => vec![argb(image)],
        Err(_) => Vec::new(),
    }
}

/// Convertit une image en `ARGB32` tel que le protocole l'exige : les octets
/// sont dans l'ordre d'un entier 32 bits, donc alpha en tête.
fn argb(image: image::DynamicImage) -> (i32, i32, Vec<u8>) {
    let small = image
        .resize_exact(ICON_PX, ICON_PX, image::imageops::FilterType::Triangle)
        .to_rgba8();
    let mut data = Vec::with_capacity((ICON_PX * ICON_PX * 4) as usize);
    for pixel in small.pixels() {
        let [r, g, b, a] = pixel.0;
        data.extend_from_slice(&[a, r, g, b]);
    }
    (ICON_PX as i32, ICON_PX as i32, data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_visuel_est_en_argb() {
        let image = image::RgbaImage::from_pixel(2, 2, image::Rgba([10, 20, 30, 255]));
        let (width, height, data) = argb(image::DynamicImage::ImageRgba8(image));
        assert_eq!((width, height), (ICON_PX as i32, ICON_PX as i32));
        assert_eq!(data.len(), (ICON_PX * ICON_PX * 4) as usize);
        assert_eq!(&data[0..4], &[255, 10, 20, 30]);
    }

    #[test]
    fn sans_jeu_c_est_le_logo_qui_sert() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        paths.ensure().unwrap();
        let pixmap = pixmap(&paths, Some(480));
        assert_eq!(pixmap.len(), 1);
        assert_eq!(pixmap[0].0 as u32, ICON_PX);
        assert_eq!(pixmap[0].2.len(), (ICON_PX * ICON_PX * 4) as usize);
    }

    #[test]
    fn une_icone_de_jeu_devient_un_png() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        paths.ensure().unwrap();
        std::fs::create_dir_all(paths.art_dir()).unwrap();
        image::RgbaImage::from_pixel(80, 80, image::Rgba([200, 40, 20, 255]))
            .save(paths.art_dir().join("steam-icon-42.png"))
            .unwrap();

        let icon = menu_icon(&paths, 42);
        assert!(icon.starts_with(b"\x89PNG\r\n\x1a\n"));
        let decoded = image::load_from_memory(&icon).unwrap();
        assert_eq!(
            (decoded.width(), decoded.height()),
            (MENU_ICON_PX, MENU_ICON_PX)
        );
    }

    #[test]
    fn une_jaquette_sert_d_icone_quand_l_icone_manque() {
        let dir = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(dir.path());
        paths.ensure().unwrap();
        std::fs::create_dir_all(paths.covers_dir()).unwrap();
        image::RgbImage::from_pixel(60, 90, image::Rgb([20, 90, 160]))
            .save(paths.covers_dir().join("42424242.jpg"))
            .unwrap();

        let icon = menu_icon(&paths, 42_424_242);
        assert!(icon.starts_with(b"\x89PNG\r\n\x1a\n"));
        let decoded = image::load_from_memory(&icon).unwrap();
        assert_eq!(
            (decoded.width(), decoded.height()),
            (MENU_ICON_PX, MENU_ICON_PX)
        );
    }

    #[test]
    fn le_menu_porte_l_icone_du_jeu() {
        let (on_action, _actions) = std::sync::mpsc::channel();
        let menu = Menu {
            data: Arc::new(MenuData::default()),
            on_action,
        };
        if let Ok(mut recent) = menu.data.recent.lock() {
            recent.push(RecentGame {
                appid: 42,
                name: "Avec icône".into(),
                icon: b"\x89PNG\r\n\x1a\n".to_vec(),
            });
            recent.push(RecentGame {
                appid: 43,
                name: "Sans icône".into(),
                icon: Vec::new(),
            });
        }

        let avec = menu.properties(RECENT_BASE);
        assert!(avec.contains_key("icon-data"));
        assert_eq!(avec["icon-data"].value_signature().to_string(), "ay");
        let sans = menu.properties(RECENT_BASE + 1);
        assert!(!sans.contains_key("icon-data"));
    }

    #[test]
    fn le_menu_propose_toujours_d_afficher_lapinux() {
        let (on_action, _actions) = std::sync::mpsc::channel();
        let menu = Menu {
            data: Arc::new(MenuData::default()),
            on_action,
        };
        let expected = lapinux_core::i18n::t("tray.show");
        assert_eq!(menu.entries()[0].1, expected);
        // Même quand l'interface tourne, l'entrée ne la ferme pas.
        menu.data
            .visible
            .store(true, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(menu.entries()[0].1, expected);
    }

    #[test]
    fn le_menu_range_les_jeux_puis_les_commandes() {
        let (on_action, _actions) = std::sync::mpsc::channel();
        let menu = Menu {
            data: Arc::new(MenuData::default()),
            on_action,
        };
        menu.data
            .visible
            .store(true, std::sync::atomic::Ordering::Relaxed);
        if let Ok(mut recent) = menu.data.recent.lock() {
            recent.push(RecentGame {
                appid: 42,
                name: "MOTORSLICE".into(),
                icon: Vec::new(),
            });
        }

        let entries = menu.entries();
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].1, "MOTORSLICE");
        assert_eq!(entries[1].2, "separator");
        assert_eq!(entries[2].0, MENU_TOGGLE);
        assert_eq!(entries[3].0, MENU_QUIT);
        assert_eq!(entries[2].4, Some(TOGGLE_ICON));
        assert_eq!(entries[3].4, Some(QUIT_ICON));
        assert_eq!(
            entries[3].1,
            lapinux_core::i18n::t("tray.quit")
        );
    }
}
