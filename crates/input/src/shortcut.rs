//! Raccourci clavier global du menu rapide, porté par KWin.
//!
//! Ce module possède deux choses, et rien d'autre : le nom D-Bus que le daemon
//! sert (`Toggle`) et le script KWin qui enregistre le raccourci pendant une
//! session. Aucune lecture `/dev/input` n'est nécessaire.

use std::sync::{Mutex, OnceLock};
use zbus::blocking::connection::Builder;
use zbus::blocking::Connection;
use zbus::blocking::Proxy;
use zbus::fdo::{RequestNameFlags, RequestNameReply};

const BUS_NAME: &str = "io.github.lapinux.QuickMenu";
const OBJECT_PATH: &str = "/io/github/lapinux/QuickMenu";
const INTERFACE: &str = "io.github.lapinux.QuickMenu";
const SCRIPT_NAME: &str = "lapinux-quick-menu";
/// Guetteur de la première fenêtre du jeu, chargé le temps d'une session.
const GAME_SCRIPT_NAME: &str = "lapinux-game-window";
/// Script qui garde l'écran d'attente au-dessus de tout le reste.
const SPLASH_SCRIPT_NAME: &str = "lapinux-splash";
const REVEAL_GAME_SCRIPT_NAME: &str = "lapinux-splash-reveal";
/// Script d'un coup : réactive la fenêtre du jeu après l'écran d'attente.
const FOCUS_SCRIPT_NAME: &str = "lapinux-focus-game";
/// Script d'un coup : ramène la fenêtre de Lapinux au premier plan.
const REVEAL_SCRIPT_NAME: &str = "lapinux-reveal";
/// Script d'un coup : sort Lapinux de la réduction sans le ramener au premier
/// plan s'il est déjà visible.
const WAKE_SCRIPT_NAME: &str = "lapinux-wake";
/// Script d'un coup : retire la fenêtre de Lapinux de l'écran et de la barre
/// des tâches pendant qu'elle est rangée dans la zone de notification.
const HIDE_SCRIPT_NAME: &str = "lapinux-hide";
/// Script d'un coup : place Lapinux derrière le jeu sans le réduire.
const BURY_SCRIPT_NAME: &str = "lapinux-bury";
/// Script d'un coup : ferme la fenêtre de Lapinux, tray du daemon excepté.
const CLOSE_SCRIPT_NAME: &str = "lapinux-close";
/// Script d'un coup : relève le rectangle de la fenêtre du jeu.
const GEOMETRY_SCRIPT_NAME: &str = "lapinux-game-geometry";

/// État du raccourci, lisible par le diagnostic sans interpréter de chaîne.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// Service démarré, aucun raccourci chargé (hors session).
    Ready,
    /// Raccourci chargé pour la session en cours.
    Active(String),
}

static SERVICE: OnceLock<Connection> = OnceLock::new();
static STATE: Mutex<State> = Mutex::new(State::Ready);
/// Dernier rectangle relevé par le script de géométrie.
static GEOMETRY: Mutex<Option<GameWindow>> = Mutex::new(None);
/// Le relevé charge et décharge un script KWin : deux appels simultanés
/// (démarrage de capture, fin de session) se voleraient le script et
/// pourraient conclure à tort qu'il n'y a plus de fenêtre.
static GEOMETRY_LOCK: Mutex<()> = Mutex::new(());
/// Identifiant KWin de la même fenêtre, appelé à part : KWin tronque un appel
/// D-Bus trop chargé et perd alors son dernier argument.
static HANDLE: Mutex<Option<String>> = Mutex::new(None);

/// Rectangle de la fenêtre du jeu et de l'écran qui la porte, tels que KWin
/// les voit : c'est ce qui cadre une capture ou une vidéo sur le jeu seul.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameWindow {
    /// Connecteur de l'écran de la fenêtre (`DP-1`) ; vide si KWin ne le dit
    /// pas.
    pub output: String,
    /// Identifiant que KWin accepte pour capturer la fenêtre elle-même
    /// (`org.kde.KWin.ScreenShot2.CaptureWindow`) ; vide s'il n'est pas donné.
    pub handle: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    /// Rectangle de l'écran qui porte la fenêtre, en coordonnées du bureau.
    pub screen_x: i32,
    pub screen_y: i32,
    pub screen_width: i32,
    pub screen_height: i32,
}

/// `None` tant que [`serve`] n'a pas pris le nom D-Bus.
pub fn state() -> Option<State> {
    SERVICE.get()?;
    Some(
        STATE
            .lock()
            .map(|state| state.clone())
            .unwrap_or(State::Ready),
    )
}

/// Vrai quand KWin répond : c'est le seul chemin de raccourci de Lapinux.
pub fn kwin_available() -> bool {
    let Ok(connection) = Connection::session() else {
        return false;
    };
    connection
        .call_method(
            Some("org.kde.KWin"),
            "/KWin",
            Some("org.freedesktop.DBus.Peer"),
            "Ping",
            &(),
        )
        .is_ok()
}

/// Prend le nom D-Bus que le script KWin appelle et libère un raccourci laissé
/// par un daemon précédent ; `on_toggle` est exécuté à chaque pression,
/// `on_game_window` reçoit le pid de chaque fenêtre qui apparaît, et
/// `on_game_window_gone` celui de chaque fenêtre qui disparaît.
pub fn serve(
    on_toggle: impl Fn() + Send + Sync + 'static,
    on_game_window: impl Fn(u32) + Send + Sync + 'static,
    on_game_window_gone: impl Fn(u32) + Send + Sync + 'static,
) -> Result<(), String> {
    let menu = QuickMenu {
        on_toggle: Box::new(on_toggle),
        on_game_window: Box::new(on_game_window),
        on_game_window_gone: Box::new(on_game_window_gone),
    };
    let connection = Builder::session()
        .map_err(|e| format!("D-Bus : {e}"))?
        .serve_at(OBJECT_PATH, menu)
        .map_err(|e| format!("service {INTERFACE} : {e}"))?
        .build()
        .map_err(|e| format!("connexion D-Bus : {e}"))?;
    // Jamais de remplacement : un second daemon ne doit pas priver le premier
    // du raccourci, il repart simplement sans le menu rapide.
    match connection.request_name_with_flags(BUS_NAME, RequestNameFlags::DoNotQueue.into()) {
        Ok(RequestNameReply::PrimaryOwner) | Ok(RequestNameReply::AlreadyOwner) => {}
        Ok(reply) => return Err(format!("nom {BUS_NAME} déjà pris ({reply:?})")),
        Err(err) => return Err(format!("nom {BUS_NAME} : {err}")),
    }
    let _ = SERVICE.set(connection);
    unregister();
    Ok(())
}

/// Enregistre `chord` pour la session en cours, via un script KWin.
pub fn register(chord: &str) -> Result<(), String> {
    let chord = chord.trim();
    if chord.is_empty() {
        return Err("raccourci clavier vide dans les réglages".into());
    }
    // Le raccourci est inséré dans le script KWin : refuser tout ce qui n'est
    // pas une séquence de touches usuelle.
    if !chord
        .chars()
        .all(|c| c.is_alphanumeric() || matches!(c, '+' | '-' | ' '))
    {
        return Err(format!("raccourci clavier invalide : « {chord} »"));
    }
    if SERVICE.get().is_none() {
        return Err("service du raccourci non démarré (daemon relancé depuis ?)".into());
    }
    let dir = lapinux_core::paths::LapinuxPaths::discover().cache_dir;
    std::fs::create_dir_all(&dir).map_err(|e| format!("cache : {e}"))?;
    let path = dir.join("kwin-quick-menu.js");
    let script = format!(
        "registerShortcut(\"{SCRIPT_NAME}\", \"Menu rapide {}\", \"{chord}\", function() {{\n    callDBus(\"{BUS_NAME}\", \"{OBJECT_PATH}\", \"{INTERFACE}\", \"Toggle\");\n}});\n",
        lapinux_core::APP_NAME
    );
    std::fs::write(&path, script).map_err(|e| format!("script KWin : {e}"))?;
    load_script(&path, SCRIPT_NAME)?;
    store(State::Active(chord.to_string()));
    Ok(())
}

/// Charge (ou recharge) un script KWin et le démarre. KWin garde les scripts
/// chargés jusqu'à leur déchargement : le nom sert de clé.
fn load_script(path: &std::path::Path, name: &str) -> Result<(), String> {
    let connection = Connection::session().map_err(|e| format!("D-Bus : {e}"))?;
    let scripting = Proxy::new(
        &connection,
        "org.kde.KWin",
        "/Scripting",
        "org.kde.kwin.Scripting",
    )
    .map_err(|e| format!("KWin Scripting : {e}"))?;
    let _: bool = scripting.call("unloadScript", &(name,)).unwrap_or(false);
    let _: i32 = scripting
        .call("loadScript", &(path.to_string_lossy().to_string(), name))
        .map_err(|e| format!("chargement du script KWin : {e}"))?;
    scripting
        .call::<_, _, ()>("start", &())
        .map_err(|e| format!("démarrage du script KWin : {e}"))?;
    Ok(())
}

/// Demande à KWin de signaler chaque fenêtre visible qui apparaît pendant la
/// session. Le script ne filtre pas sur le pid du jeu : un jeu Proton élève
/// sa fenêtre depuis un processus enfant de Wine, et c'est le daemon qui
/// vérifie la parenté. Rend une erreur quand KWin n'est pas joignable, pour
/// que l'appelant retombe sur l'ancien comportement.
pub fn watch_game_window() -> Result<(), String> {
    let dir = lapinux_core::paths::LapinuxPaths::discover().cache_dir;
    std::fs::create_dir_all(&dir).map_err(|e| format!("cache : {e}"))?;
    let path = dir.join("kwin-game-window.js");
    // KWin 6 n'expose pas de propriété `visible` : une fenêtre qui vient
    // d'être créée est `hidden` jusqu'à son premier affichage, et les signaux
    // `windowAdded` / `windowActivated` couvrent son apparition. On ne se
    // branche pas sur `windowShown` par fenêtre : l'objet ne l'expose pas
    // toujours, et l'exception interrompait tout le script.
    let script = format!(
        "function report(win) {{\n    if (win && win.pid > 0 && !win.hidden && !win.minimized) {{\n        callDBus(\"{BUS_NAME}\", \"{OBJECT_PATH}\", \"{INTERFACE}\", \"GameWindow\", win.pid);\n    }}\n}}\nfunction gone(win) {{\n    if (win && win.pid > 0) {{\n        callDBus(\"{BUS_NAME}\", \"{OBJECT_PATH}\", \"{INTERFACE}\", \"GameWindowGone\", win.pid);\n    }}\n}}\nworkspace.windowAdded.connect(report);\nworkspace.windowActivated.connect(report);\nworkspace.windowRemoved.connect(gone);\nvar windows = workspace.windowList();\nfor (var i = 0; i < windows.length; ++i) {{\n    report(windows[i]);\n}}\n"
    );
    std::fs::write(&path, script).map_err(|e| format!("script KWin : {e}"))?;
    load_script(&path, GAME_SCRIPT_NAME)
}

/// Garde la fenêtre d'attente au-dessus de toutes les autres, y compris la
/// fenêtre plein écran d'un jeu.
///
/// Sous Wayland, winit ignore tout simplement le niveau de fenêtre : c'est
/// KWin qui doit placer la fenêtre dans sa couche « au-dessus » et sur
/// l'écran du jeu. Le script reconnaît l'écran d'attente à son titre unique.
///
/// `keepAbove` seul ne suffit pas face à un jeu plein écran : KWin réserve
/// une couche supérieure aux fenêtres plein écran, qui passeraient devant
/// l'écran d'attente. Tant qu'il est là, ces fenêtres descendent donc dans la
/// couche « en dessous » (`keepBelow`). Elles sont les seules concernées, et
/// la propriété est rendue dès que l'écran disparaît : le fondu révèle le jeu
/// sans laisser le bureau réorganisé.
///
/// Sans écran cible, winit ouvre le viewport plein écran sur la sortie par
/// défaut (`DP-1` ici), alors que le jeu tourne sur `DP-2` : l'écran de fin
/// semblait arriver en retard parce qu'il s'affichait sur l'autre moniteur.
pub fn keep_splash_above(output: &str) -> Result<(), String> {
    let dir = lapinux_core::paths::LapinuxPaths::discover().cache_dir;
    std::fs::create_dir_all(&dir).map_err(|e| format!("cache : {e}"))?;
    let path = dir.join("kwin-splash.js");
    let title = lapinux_core::SPLASH_TITLE
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    // Un jeu plein écran occupe une couche supérieure à celle d'une fenêtre
    // « toujours au-dessus » : l'écran d'attente se ferait couvrir. Tant qu'il
    // existe, les fenêtres plein écran descendent donc dans la couche
    // inférieure — et seulement elles : les autres fenêtres du bureau ne
    // changent pas d'état, et tout est restauré dès que l'écran disparaît.
    let output = output.trim().replace('\\', "\\\\").replace('"', "\\\"");
    let script = r#"var splashTitle = "__TITLE__";
var targetOutput = "__OUTPUT__";
var lowered = [];
function isSplash(win) { return win && win.caption === splashTitle; }
function splashWindow() {
    var list = workspace.windowList();
    for (var i = 0; i < list.length; ++i) { if (isSplash(list[i])) return list[i]; }
    return null;
}
function placeOnGameScreen(win) {
    if (!win || targetOutput === "") return;
    if (win.output && win.output.name === targetOutput) return;
    var screens = workspace.screens;
    for (var i = 0; i < screens.length; ++i) {
        if (screens[i].name !== targetOutput) continue;
        try { workspace.sendClientToScreen(win, screens[i]); } catch (e) {}
        return;
    }
}
function lower(win) {
    if (!win || isSplash(win) || !win.normalWindow || !win.fullScreen) return;
    // Sous Wayland, `--prefer-output` de GameScope ne déplace pas la fenêtre
    // imbriquée : sans ce placement, le jeu naissait sur DP-1 alors que le
    // splash et l'écran de fin sont sur DP-2. Pendant le fondu, DP-2 montrait
    // donc le bureau avant que le jeu n'arrive. On place le jeu avant de
    // l'abaisser sous le splash : il est déjà prêt derrière le voile.
    placeOnGameScreen(win);
    try {
        if (!win.keepBelow) { win.keepBelow = true; lowered.push(win); }
    } catch (e) {}
}
function restore() {
    for (var i = 0; i < lowered.length; ++i) {
        try { if (lowered[i]) lowered[i].keepBelow = false; } catch (e) {}
    }
    lowered = [];
}
function pin() {
    var win = splashWindow();
    if (!win) { restore(); return; }
    placeOnGameScreen(win);
    try { win.keepAbove = true; win.skipTaskbar = true; win.skipSwitcher = true; } catch (e) {}
    if (workspace.activeWindow !== win) {
        try { workspace.activeWindow = win; } catch (e) {}
    }
    var list = workspace.windowList();
    for (var i = 0; i < list.length; ++i) { lower(list[i]); }
}
workspace.windowAdded.connect(function(win) { if (isSplash(win)) pin(); else { lower(win); pin(); } });
workspace.windowActivated.connect(function(win) { if (!isSplash(win) && splashWindow()) pin(); });
workspace.windowRemoved.connect(function(win) { if (isSplash(win)) restore(); });
pin();
"#
    .replace("__TITLE__", &title)
    .replace("__OUTPUT__", &output);
    std::fs::write(&path, script).map_err(|e| format!("script KWin : {e}"))?;
    load_script(&path, SPLASH_SCRIPT_NAME)
}

/// Prépare la révélation du jeu : place sa fenêtre plein écran sur l'écran
/// cible et la sort de la couche « en dessous » alors que le splash est encore
/// affiché au-dessus. Le fondu découvre ainsi le jeu déjà en place, jamais le
/// bureau. Sans cela, `--prefer-output` de GameScope ne suffit pas en mode
/// imbriqué et la fenêtre pouvait naître sur un autre moniteur.
pub fn prepare_splash_reveal(output: &str) -> Result<(), String> {
    let dir = lapinux_core::paths::LapinuxPaths::discover().cache_dir;
    std::fs::create_dir_all(&dir).map_err(|e| format!("cache : {e}"))?;
    let path = dir.join("kwin-splash-reveal.js");
    let title = lapinux_core::SPLASH_TITLE
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    let output = output.trim().replace('\\', "\\\\").replace('"', "\\\"");
    let script = r#"var splashTitle = "__TITLE__";
var targetOutput = "__OUTPUT__";
function isSplash(win) { return win && win.caption === splashTitle; }
function place(win) {
    if (!win || targetOutput === "") return;
    try {
        if (win.output && win.output.name === targetOutput) return;
        var screens = workspace.screens;
        for (var i = 0; i < screens.length; ++i) {
            if (screens[i].name !== targetOutput) continue;
            workspace.sendClientToScreen(win, screens[i]);
            return;
        }
    } catch (e) {}
}
function prepare() {
    var list = workspace.windowList();
    for (var i = 0; i < list.length; ++i) {
        var win = list[i];
        if (!win || isSplash(win) || !win.normalWindow || !win.fullScreen) continue;
        place(win);
        // Le splash reste dans la couche « au-dessus » : retirer `keepBelow`
        // ne le découvre pas encore, mais le jeu est prêt dessous.
        try { win.keepBelow = false; } catch (e) {}
    }
}
prepare();
workspace.windowAdded.connect(function(win) { if (!isSplash(win)) prepare(); });
"#
    .replace("__TITLE__", &title)
    .replace("__OUTPUT__", &output);
    std::fs::write(&path, script).map_err(|e| format!("script KWin : {e}"))?;
    load_script(&path, REVEAL_GAME_SCRIPT_NAME)?;
    // Le script n'a qu'un rôle de transition : on le retire pour que KWin ne
    // garde pas une règle de placement sur les fenêtres suivantes.
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(8));
        unload_script(REVEAL_GAME_SCRIPT_NAME);
    });
    Ok(())
}

/// Rend le focus à la fenêtre du jeu : l'écran d'attente l'avait pris, et
/// KWin réactive sinon la fenêtre de Lapinux quand celui-ci disparaît. `pids`
/// rassemble le processus du jeu et ses descendants, dont la fenêtre Wine.
pub fn focus_window(pids: &[u32]) -> Result<(), String> {
    if pids.is_empty() {
        return Err("aucun processus de jeu à activer".into());
    }
    let list = pids
        .iter()
        .map(|pid| pid.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let dir = lapinux_core::paths::LapinuxPaths::discover().cache_dir;
    std::fs::create_dir_all(&dir).map_err(|e| format!("cache : {e}"))?;
    let path = dir.join("kwin-focus-game.js");
    // La fenêtre peut arriver juste après le fondu : un essai immédiat, puis
    // un nouvel essai à chaque fenêtre créée. Le script est déchargé par
    // l'appelant quelques secondes plus tard.
    let script = format!(
        "var pids = [{list}];\n\
         function activate() {{\n\
             var list = workspace.windowList();\n\
             for (var i = 0; i < list.length; ++i) {{\n\
                 var win = list[i];\n\
                 if (!win || !win.normalWindow || win.hidden || win.minimized) continue;\n\
                 if (pids.indexOf(win.pid) < 0) continue;\n\
                 try {{ workspace.raiseWindow(win); workspace.activeWindow = win; }} catch (e) {{}}\n\
                 return true;\n\
             }}\n\
             return false;\n\
         }}\n\
         activate();\n\
         workspace.windowAdded.connect(function() {{ activate(); }});\n"
    );
    std::fs::write(&path, script).map_err(|e| format!("script KWin : {e}"))?;
    load_script(&path, FOCUS_SCRIPT_NAME)?;
    // Le minuteur n'a plus rien à faire au bout de quelques secondes.
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(8));
        unload_script(FOCUS_SCRIPT_NAME);
    });
    Ok(())
}

/// Rectangle de la fenêtre du jeu, relevé dans KWin. `wait` borne l'attente :
/// KWin répond en quelques dizaines de millisecondes, et une session sans
/// fenêtre ne doit pas retarder le lancement de la vidéo.
pub fn game_window(pids: &[u32], wait: std::time::Duration) -> Option<GameWindow> {
    if pids.is_empty() || SERVICE.get().is_none() {
        return None;
    }
    let _geometry = GEOMETRY_LOCK.lock().ok()?;
    let list = pids
        .iter()
        .map(|pid| pid.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let dir = lapinux_core::paths::LapinuxPaths::discover().cache_dir;
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("kwin-game-geometry.js");
    // Le script rend aussi l'écran de la fenêtre : ses coordonnées servent à
    // ramener le rectangle de la fenêtre à celui du moniteur, que
    // l'enregistreur attend en décalage.
    let script = r#"var pids = [__PIDS__];
function report() {
    var list = workspace.windowList();
    for (var i = 0; i < list.length; ++i) {
        var win = list[i];
        if (!win || !win.normalWindow || win.hidden || win.minimized) continue;
        if (pids.indexOf(win.pid) < 0) continue;
        var g = win.frameGeometry;
        var handle = "";
        try { if (win.internalId) handle = String(win.internalId); } catch (e) {}
        var name = "";
        var ox = 0; var oy = 0; var ow = 0; var oh = 0;
        try {
            var out = win.output;
            if (out) {
                name = out.name;
                var og = out.geometry;
                ox = og.x; oy = og.y; ow = og.width; oh = og.height;
            }
        } catch (e) {}
        callDBus("__BUS__", "__PATH__", "__IFACE__", "WindowGeometry",
            name, g.x, g.y, g.width, g.height, ox, oy, ow, oh);
        // L'identifiant part à part : KWin tronque les appels trop chargés en
        // perdant leur dernier argument, sans le dire.
        callDBus("__BUS__", "__PATH__", "__IFACE__", "WindowHandle", handle);
        return true;
    }
    return false;
}
report();
workspace.windowAdded.connect(function() { report(); });
"#
    .replace("__PIDS__", &list)
    .replace("__BUS__", BUS_NAME)
    .replace("__PATH__", OBJECT_PATH)
    .replace("__IFACE__", INTERFACE);
    std::fs::write(&path, script).ok()?;
    if let Ok(mut slot) = GEOMETRY.lock() {
        *slot = None;
    }
    if let Ok(mut slot) = HANDLE.lock() {
        *slot = None;
    }
    load_script(&path, GEOMETRY_SCRIPT_NAME).ok()?;
    let deadline = std::time::Instant::now() + wait;
    loop {
        let found = GEOMETRY.lock().ok().and_then(|slot| slot.clone());
        if let Some(mut found) = found {
            // L'identifiant de fenêtre arrive par un second appel, juste
            // après le rectangle : on lui laisse un instant pour suivre.
            let until = std::time::Instant::now() + std::time::Duration::from_millis(200);
            loop {
                if let Some(handle) = HANDLE.lock().ok().and_then(|slot| slot.clone()) {
                    found.handle = handle;
                    break;
                }
                if std::time::Instant::now() >= until {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            unload_script(GEOMETRY_SCRIPT_NAME);
            return Some(found);
        }
        if std::time::Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(60));
    }
    unload_script(GEOMETRY_SCRIPT_NAME);
    None
}

/// Ramène la fenêtre de Lapinux au premier plan, la sort de la réduction et
/// la réactive. Un script KWin s'en charge : sous Wayland, une fenêtre
/// réduite ne reçoit plus d'images, et l'application ne pourrait pas
/// s'afficher d'elle-même au retour d'une partie.
pub fn reveal_lapinux() -> Result<(), String> {
    let dir = lapinux_core::paths::LapinuxPaths::discover().cache_dir;
    std::fs::create_dir_all(&dir).map_err(|e| format!("cache : {e}"))?;
    let path = dir.join("kwin-reveal.js");
    let title = lapinux_core::APP_NAME
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    let script = format!(
        "var title = \"{title}\";\n\
         function reveal() {{\n\
             var list = workspace.windowList();\n\
             for (var i = 0; i < list.length; ++i) {{\n\
                 var win = list[i];\n\
                 if (!win || win.caption !== title) continue;\n\
                 try {{ if (win.skipTaskbar) win.skipTaskbar = false; }} catch (e) {{}}\n\
                 try {{ if (win.skipPager) win.skipPager = false; }} catch (e) {{}}\n\
                 try {{ if (win.skipSwitcher) win.skipSwitcher = false; }} catch (e) {{}} try {{ if (win.keepBelow) win.keepBelow = false; }} catch (e) {{}}\n\
                 try {{ if (win.minimized) win.minimized = false; }} catch (e) {{}}\n\
                 try {{ workspace.raiseWindow(win); workspace.activeWindow = win; }} catch (e) {{}}\n\
                 return true;\n\
             }}\n\
             return false;\n\
         }}\n\
         reveal();\n\
         workspace.windowAdded.connect(function() {{ reveal(); }});\n"
    );
    std::fs::write(&path, script).map_err(|e| format!("script KWin : {e}"))?;
    load_script(&path, REVEAL_SCRIPT_NAME)?;
    // Le guetteur ne sert qu'au retour : il repart après quelques secondes.
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(6));
        unload_script(REVEAL_SCRIPT_NAME);
    });
    Ok(())
}

/// Réveille Lapinux s'il est réduit, sans le faire passer devant. La fenêtre
/// reste mappée (donc ses viewports peignent), mais elle est envoyée sous les
/// autres fenêtres et retirée de la barre des tâches. Le lancement depuis le
/// tray s'en sert : le splash peut être créé immédiatement, sans que la
/// bibliothèque ne s'affiche avant lui.
pub fn wake_lapinux() -> Result<(), String> {
    let dir = lapinux_core::paths::LapinuxPaths::discover().cache_dir;
    std::fs::create_dir_all(&dir).map_err(|e| format!("cache : {e}"))?;
    let path = dir.join("kwin-wake.js");
    let title = lapinux_core::APP_NAME
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    let script = format!(
        "var title = \"{title}\";\n\
         function wake() {{\n\
             var list = workspace.windowList();\n\
             for (var i = 0; i < list.length; ++i) {{\n\
                 var win = list[i];\n\
                 if (!win || win.caption !== title) continue;\n\
                 try {{ win.skipTaskbar = true; }} catch (e) {{}}\n\
                 try {{ win.skipPager = true; }} catch (e) {{}}\n\
                 try {{ win.skipSwitcher = true; }} catch (e) {{}}\n\
                 try {{ win.keepBelow = true; }} catch (e) {{}}\n\
                 try {{ if (win.minimized) win.minimized = false; }} catch (e) {{}}\n\
                 return true;\n\
             }}\n\
             return false;\n\
         }}\n\
         wake();\n\
         workspace.windowAdded.connect(function() {{ wake(); }});\n"
    );
    std::fs::write(&path, script).map_err(|e| format!("script KWin : {e}"))?;
    load_script(&path, WAKE_SCRIPT_NAME)?;
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(6));
        unload_script(WAKE_SCRIPT_NAME);
    });
    Ok(())
}

/// Place Lapinux derrière le jeu **sans la réduire**. Une fenêtre réduite ne
/// mappe plus ses viewports enfants : l'écran de fin, pourtant créé, restait
/// invisible jusqu'à sa fermeture. La fenêtre reste donc mappée, sous le jeu
/// plein écran, et disparaît de la barre des tâches.
pub fn bury_lapinux() -> Result<(), String> {
    let dir = lapinux_core::paths::LapinuxPaths::discover().cache_dir;
    std::fs::create_dir_all(&dir).map_err(|e| format!("cache : {e}"))?;
    let path = dir.join("kwin-bury.js");
    let title = lapinux_core::APP_NAME
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    let script = format!(
        "var title = \"{title}\";\n\
         function bury() {{\n\
             var list = workspace.windowList();\n\
             for (var i = 0; i < list.length; ++i) {{\n\
                 var win = list[i];\n\
                 if (!win || win.caption !== title) continue;\n\
                 try {{ win.skipTaskbar = true; }} catch (e) {{}}\n\
                 try {{ win.skipPager = true; }} catch (e) {{}}\n\
                 try {{ win.skipSwitcher = true; }} catch (e) {{}}\n\
                 try {{ win.keepBelow = true; }} catch (e) {{}}\n\
                 return true;\n\
             }}\n\
             return false;\n\
         }}\n\
         bury();\n"
    );
    std::fs::write(&path, script).map_err(|e| format!("script KWin : {e}"))?;
    load_script(&path, BURY_SCRIPT_NAME)?;
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(10));
        unload_script(BURY_SCRIPT_NAME);
    });
    Ok(())
}

/// Retire complètement la fenêtre de Lapinux de l'écran et de la barre des
/// tâches. KWin impose `skipTaskbar` et la réduction : sous Wayland, winit
/// laisse sinon une entrée de fenêtre derrière l'icône de notification.
pub fn hide_lapinux() -> Result<(), String> {
    let dir = lapinux_core::paths::LapinuxPaths::discover().cache_dir;
    std::fs::create_dir_all(&dir).map_err(|e| format!("cache : {e}"))?;
    let path = dir.join("kwin-hide.js");
    let title = lapinux_core::APP_NAME
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    let script = format!(
        "var title = \"{title}\";\n\
         function hide(win) {{\n\
             if (!win || win.caption !== title) return false;\n\
             try {{ win.minimized = true; }} catch (e) {{}}\n\
             try {{ win.skipTaskbar = true; }} catch (e) {{}}\n\
             try {{ win.skipPager = true; }} catch (e) {{}}\n\
             try {{ win.skipSwitcher = true; }} catch (e) {{}}\n\
             return true;\n\
         }}\n\
         function hideAll() {{\n\
             var list = workspace.windowList();\n\
             for (var i = 0; i < list.length; ++i) {{\n\
                 if (hide(list[i])) return;\n\
             }}\n\
         }}\n\
         hideAll();\n\
         workspace.windowAdded.connect(function(win) {{ hide(win); }});\n"
    );
    std::fs::write(&path, script).map_err(|e| format!("script KWin : {e}"))?;
    load_script(&path, HIDE_SCRIPT_NAME)?;
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(6));
        unload_script(HIDE_SCRIPT_NAME);
    });
    Ok(())
}

/// Ferme la fenêtre de Lapinux, quel que soit son état (visible, réduite ou
/// rangée). Le tray appartient au daemon : la fenêtre peut donc disparaître
/// complètement, et sa mémoire avec elle. `win.close()` déclenche la fermeture
/// normale de l'application, donc son `on_exit` et le retrait de ses marqueurs.
pub fn close_lapinux() -> Result<(), String> {
    let dir = lapinux_core::paths::LapinuxPaths::discover().cache_dir;
    std::fs::create_dir_all(&dir).map_err(|e| format!("cache : {e}"))?;
    let path = dir.join("kwin-close.js");
    let title = lapinux_core::APP_NAME
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    let script = format!(
        "var title = \"{title}\";\n\
         function closeLapinux() {{\n\
             var list = workspace.windowList();\n\
             for (var i = 0; i < list.length; ++i) {{\n\
                 var win = list[i];\n\
                 if (!win || win.caption !== title) continue;\n\
                 try {{ win.close(); }} catch (e) {{}}\n\
                 return true;\n\
             }}\n\
             return false;\n\
         }}\n\
         closeLapinux();\n"
    );
    std::fs::write(&path, script).map_err(|e| format!("script KWin : {e}"))?;
    load_script(&path, CLOSE_SCRIPT_NAME)?;
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(3));
        unload_script(CLOSE_SCRIPT_NAME);
    });
    Ok(())
}

fn unload_script(name: &str) {
    if let Ok(connection) = Connection::session() {
        if let Ok(scripting) = Proxy::new(
            &connection,
            "org.kde.KWin",
            "/Scripting",
            "org.kde.kwin.Scripting",
        ) {
            let _: bool = scripting.call("unloadScript", &(name,)).unwrap_or(false);
        }
    }
}

/// Décharge le guetteur de fenêtre : il ne sert qu'à la session en cours.
pub fn unwatch_game_window() {
    if let Ok(connection) = Connection::session() {
        if let Ok(scripting) = Proxy::new(
            &connection,
            "org.kde.KWin",
            "/Scripting",
            "org.kde.kwin.Scripting",
        ) {
            let _: bool = scripting
                .call("unloadScript", &(GAME_SCRIPT_NAME,))
                .unwrap_or(false);
        }
    }
}

/// Rend le raccourci au système à la fin de la session.
pub fn unregister() {
    if let Ok(connection) = Connection::session() {
        if let Ok(scripting) = Proxy::new(
            &connection,
            "org.kde.KWin",
            "/Scripting",
            "org.kde.kwin.Scripting",
        ) {
            let _: bool = scripting
                .call("unloadScript", &(SCRIPT_NAME,))
                .unwrap_or(false);
        }
        // KWin garde l'entrée dans kglobalaccel après unload : sans cleanUp,
        // le raccourci reste capturé par un script qui n'existe plus.
        if let Ok(component) = Proxy::new(
            &connection,
            "org.kde.kglobalaccel",
            "/component/kwin",
            "org.kde.kglobalaccel.Component",
        ) {
            let _: bool = component.call("cleanUp", &()).unwrap_or(false);
        }
    }
    store(State::Ready);
}

fn store(state: State) {
    if let Ok(mut current) = STATE.lock() {
        *current = state;
    }
}

struct QuickMenu {
    on_toggle: Box<dyn Fn() + Send + Sync>,
    on_game_window: Box<dyn Fn(u32) + Send + Sync>,
    on_game_window_gone: Box<dyn Fn(u32) + Send + Sync>,
}

#[zbus::interface(name = "io.github.lapinux.QuickMenu")]
impl QuickMenu {
    /// Appelée par KWin quand le raccourci est pressé.
    fn toggle(&self) {
        (self.on_toggle)();
    }

    /// Appelée par le guetteur pour chaque fenêtre visible : le daemon décide
    /// si elle appartient au processus du jeu. KWin envoie les nombres du
    /// script en `int32` : une signature `u` serait refusée par le bus.
    fn game_window(&self, pid: i32) {
        if pid > 0 {
            (self.on_game_window)(pid as u32);
        }
    }

    /// Appelée par le guetteur quand une fenêtre disparaît. Le daemon vérifie
    /// si elle appartenait au jeu et si aucune autre fenêtre du jeu ne reste.
    fn game_window_gone(&self, pid: i32) {
        if pid > 0 {
            (self.on_game_window_gone)(pid as u32);
        }
    }

    /// Rectangle de la fenêtre du jeu, envoyé par le script de géométrie.
    /// KWin passe ses nombres en `int32` et son nom d'écran en chaîne.
    fn window_geometry(
        &self,
        output: String,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        screen_x: i32,
        screen_y: i32,
        screen_width: i32,
        screen_height: i32,
    ) {
        if let Ok(mut slot) = GEOMETRY.lock() {
            *slot = Some(GameWindow {
                output,
                handle: String::new(),
                x,
                y,
                width,
                height,
                screen_x,
                screen_y,
                screen_width,
                screen_height,
            });
        }
    }

    /// Identifiant de la fenêtre du jeu, envoyé par le même script que le
    /// rectangle mais dans un appel séparé : KWin tronque un appel D-Bus trop
    /// chargé en perdant son dernier argument, sans renvoyer d'erreur.
    fn window_handle(&self, handle: String) {
        if let Ok(mut slot) = HANDLE.lock() {
            *slot = Some(handle);
        }
    }
}
