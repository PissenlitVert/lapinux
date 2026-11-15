//! Première image réelle d'un jeu GameScope sans marqueur de swapchain.
//!
//! Les jeux Vulkan journalisent `Created swapchain` dans le journal de
//! gamescope ; les jeux OpenGL, eux, n'ont aucun marqueur texte. Leur fenêtre
//! X11, en revanche, signale chaque dessin par l'extension XDamage : la
//! première zone abîmée est la première image que le jeu montre vraiment.
//!
//! Le guetteur se branche sur l'affichage Xwayland privé de gamescope (le
//! numéro est écrit dans son journal), cherche les fenêtres du processus lancé
//! et de ses enfants, puis attend un événement `DamageNotify`. Rien n'est lu
//! dans les pixels : seul le signal du serveur X est écouté.

use std::collections::HashSet;
use std::time::{Duration, Instant};
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::damage;
use x11rb::protocol::xproto::{
    AtomEnum, ConnectionExt as _, ImageFormat, MapState, Window, WindowClass,
};
use x11rb::protocol::Event;

/// Temps laissé au jeu pour créer sa fenêtre X11. Au-delà, le guetteur rend la
/// main : l'écran d'attente garde son filet de sécurité.
const WINDOW_WAIT: Duration = Duration::from_secs(15);
/// Rythme des relevés : assez rapide pour ne pas rater le premier dessin,
/// assez lent pour ne pas marteler le serveur X pendant un long chargement.
const POLL: Duration = Duration::from_millis(120);
/// Lecture du contenu : un dessin noir (le nettoyage de la fenêtre, la
/// swapchain vide) n'est pas une image de jeu. On échantillonne la fenêtre à
/// ce rythme pour ne relever que la première image réellement visible.
const SAMPLE: Duration = Duration::from_millis(200);
/// Côté des tuiles lues dans la fenêtre et nombre de tuiles par axe : une
/// grille de 7×7 couvre l'écran sans transférer l'image entière.
const TILE: u16 = 32;
const GRID: u16 = 7;
/// Un pixel est « allumé » au-dessus de ce seuil : un noir de nettoyage
/// (0-8) ou un gris très sombre ne compte pas, un logo ou un décor si.
const LIT: u8 = 18;
/// Pixels allumés nécessaires dans une tuile : une poussière ou un curseur
/// isolé ne suffit pas, un élément d'interface ou un décor oui.
const LIT_MIN: usize = 24;

/// Numéro d'affichage Xwayland annoncé par gamescope, par exemple `:1`.
pub fn xwayland_display(log: &str) -> Option<String> {
    let marker = "Starting Xwayland on :";
    let index = log.rfind(marker)?;
    let digits: String = log[index + marker.len()..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    (!digits.is_empty()).then(|| format!(":{digits}"))
}

/// Attend le premier dessin d'une fenêtre du jeu sur `display`.
///
/// `belongs` dit si un pid X11 descend du processus lancé, `alive` si la
/// session attend encore, et `attached` reçoit un signal une fois les fenêtres
/// trouvées — l'appelant sait alors que l'absence de marqueur texte n'est plus
/// un motif de repli. Rend `true` quand une image a été dessinée, `false` si la
/// session s'est arrêtée entre-temps.
pub fn watch_first_frame(
    display: &str,
    belongs: &dyn Fn(u32) -> bool,
    alive: &dyn Fn() -> bool,
    attached: std::sync::mpsc::Sender<()>,
) -> Result<bool, String> {
    let (conn, screen_num) =
        x11rb::connect(Some(display)).map_err(|e| format!("connexion X11 {display} : {e}"))?;
    let root = conn
        .setup()
        .roots
        .get(screen_num)
        .ok_or_else(|| format!("écran X11 introuvable sur {display}"))?
        .root;
    if conn
        .extension_information(damage::X11_EXTENSION_NAME)
        .map_err(|e| format!("extension XDamage : {e}"))?
        .is_none()
    {
        return Err("extension XDamage absente de l'affichage du jeu".into());
    }
    // La négociation de version initialise l'extension côté Xwayland : sans
    // elle, la première création de zone de dégâts est refusée.
    damage::query_version(&conn, 1, 1)
        .map_err(|e| format!("version XDamage : {e}"))?
        .reply()
        .map_err(|e| format!("version XDamage : {e}"))?;
    let pid_atom = conn
        .intern_atom(false, b"_NET_WM_PID")
        .map_err(|e| format!("atome _NET_WM_PID : {e}"))?
        .reply()
        .map_err(|e| format!("atome _NET_WM_PID : {e}"))?
        .atom;

    let mut watched: HashSet<Window> = HashSet::new();
    let mut damages: Vec<damage::Damage> = Vec::new();
    let mut announced = false;
    let mut last_scan = Instant::now() - Duration::from_secs(1);
    // Les dégâts signalent qu'il faut relire tout de suite ; ils ne suffisent
    // pas à conclure : le premier événement accompagne souvent un écran noir.
    let mut last_sample = Instant::now() - SAMPLE;
    let mut damaged = false;
    let started = Instant::now();
    loop {
        if !alive() {
            return Ok(false);
        }
        while let Some(event) = conn
            .poll_for_event()
            .map_err(|e| format!("événements X11 : {e}"))?
        {
            if matches!(event, Event::DamageNotify(_)) {
                damaged = true;
            }
        }
        if !damages.is_empty()
            && (damaged || last_sample.elapsed() >= SAMPLE)
            && last_sample.elapsed() >= Duration::from_millis(60)
        {
            damaged = false;
            last_sample = Instant::now();
            for target in &watched {
                match has_content(&conn, *target) {
                    Ok(true) => return Ok(true),
                    // Fenêtre fermée, redirigée ou illisible : une autre
                    // fenêtre du jeu, ou le prochain relevé, décidera.
                    Ok(false) | Err(_) => {}
                }
            }
        }

        if last_scan.elapsed() >= Duration::from_millis(400) {
            last_scan = Instant::now();
            // Certains jeux X11 natifs ne publient pas `_NET_WM_PID`
            // (glxgears, par exemple). Après cinq secondes, on se rabat sur
            // les fenêtres de premier niveau de l'affichage privé de
            // gamescope : il n'y a guère que le jeu et son compositeur.
            let fallback = damages.is_empty() && started.elapsed() > Duration::from_secs(2);
            let targets = game_windows(&conn, root, pid_atom, belongs, fallback)?;
            for target in targets {
                if watched.contains(&target) {
                    continue;
                }
                let Ok(attributes) = conn
                    .get_window_attributes(target)
                    .map_err(|e| format!("attributs X11 : {e}"))?
                    .reply()
                else {
                    continue;
                };
                if attributes.class == WindowClass::INPUT_ONLY {
                    continue;
                }
                let Ok(id) = conn.generate_id() else {
                    continue;
                };
                // Une fenêtre peut disparaître entre l'inventaire et la
                // création : elle n'est marquée suivie qu'une fois le
                // guetteur réellement posé.
                if damage::create(&conn, id, target, damage::ReportLevel::BOUNDING_BOX).is_ok() {
                    watched.insert(target);
                    damages.push(id);
                }
            }
            if !damages.is_empty() && !announced {
                let _ = attached.send(());
                announced = true;
                conn.flush().map_err(|e| format!("envoi X11 : {e}"))?;
            }
        }

        if damages.is_empty() && started.elapsed() > WINDOW_WAIT {
            return Err(format!(
                "aucune fenêtre du jeu trouvée sur {display} après {} s",
                WINDOW_WAIT.as_secs()
            ));
        }
        std::thread::sleep(POLL);
    }
}

/// La fenêtre montre-t-elle autre chose qu'un aplat noir ?
///
/// XDamage dit qu'une zone a été dessinée, pas ce qu'elle contient :
/// gamescope présente souvent une première image entièrement noire pendant
/// que le moteur compile ses shaders. On lit une grille de petites tuiles
/// (sans jamais transférer l'image entière) et on considère qu'une image est
/// réelle dès qu'une tuile contient assez de pixels colorés.
fn has_content(conn: &impl Connection, window: Window) -> Result<bool, String> {
    let attributes = conn
        .get_window_attributes(window)
        .map_err(|e| format!("attributs X11 : {e}"))?
        .reply()
        .map_err(|e| format!("attributs X11 : {e}"))?;
    if attributes.map_state != MapState::VIEWABLE {
        return Ok(false);
    }
    let geometry = conn
        .get_geometry(window)
        .map_err(|e| format!("géométrie X11 : {e}"))?
        .reply()
        .map_err(|e| format!("géométrie X11 : {e}"))?;
    let width = geometry.width;
    let height = geometry.height;
    if width < 8 || height < 8 {
        return Ok(false);
    }
    let columns = GRID.min((width / TILE).max(1));
    let rows = GRID.min((height / TILE).max(1));
    for row in 0..rows {
        for column in 0..columns {
            let x = if columns > 1 {
                (width - TILE) * column / (columns - 1)
            } else {
                0
            };
            let y = if rows > 1 {
                (height - TILE) * row / (rows - 1)
            } else {
                0
            };
            let tile = TILE.min(width - x).min(32);
            let tile_h = TILE.min(height - y).min(32);
            let Ok(reply) = conn
                .get_image(
                    ImageFormat::Z_PIXMAP,
                    window,
                    x as i16,
                    y as i16,
                    tile,
                    tile_h,
                    !0,
                )
                .map_err(|e| format!("lecture X11 : {e}"))?
                .reply()
            else {
                continue;
            };
            if pixels_lit(&reply.data, reply.depth, tile, tile_h) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Compte les pixels éclairés d'une tuile BGRX (24/32 bits).
fn pixels_lit(data: &[u8], depth: u8, width: u16, height: u16) -> bool {
    let pixels = width as usize * height as usize;
    if pixels == 0 {
        return false;
    }
    let stride = if depth >= 24 && data.len() >= pixels * 4 {
        4
    } else if data.len() >= pixels * 3 {
        3
    } else {
        return false;
    };
    let mut lit = 0usize;
    for pixel in data.chunks_exact(stride).take(pixels) {
        let (b, g, r) = match stride {
            4 => (pixel[0], pixel[1], pixel[2]),
            _ => (pixel[0], pixel[1], pixel[2]),
        };
        if b.max(g).max(r) > LIT {
            lit += 1;
        }
    }
    lit >= LIT_MIN
}

/// Fenêtres X11 qui appartiennent au jeu : celles dont `_NET_WM_PID` descend
/// du processus lancé, et tous leurs descendants (Wine dessine souvent dans
/// une fenêtre enfant).
fn game_windows(
    conn: &impl Connection,
    root: Window,
    pid_atom: u32,
    belongs: &dyn Fn(u32) -> bool,
    fallback: bool,
) -> Result<Vec<Window>, String> {
    let mut found = Vec::new();
    let mut loose = Vec::new();
    if fallback {
        let root_tree = conn
            .query_tree(root)
            .map_err(|e| format!("arbre X11 : {e}"))?
            .reply()
            .map_err(|e| format!("arbre X11 : {e}"))?;
        for &window in &root_tree.children {
            let Ok(geometry) = conn
                .get_geometry(window)
                .map_err(|e| format!("géométrie X11 : {e}"))?
                .reply()
            else {
                continue;
            };
            // Le compositeur de gamescope se déclare en 1×1 : ce n'est pas
            // une image de jeu.
            if geometry.width <= 2 || geometry.height <= 2 {
                continue;
            }
            loose.push(window);
        }
    }
    let mut stack = vec![root];
    let mut visited = 0usize;
    while let Some(window) = stack.pop() {
        visited += 1;
        if visited > 8192 {
            break;
        }
        let Ok(tree) = conn
            .query_tree(window)
            .map_err(|e| format!("arbre X11 : {e}"))?
            .reply()
        else {
            continue;
        };
        stack.extend(tree.children.iter().copied());
        if window == root {
            continue;
        }
        let Ok(property) = conn
            .get_property(false, window, pid_atom, AtomEnum::CARDINAL, 0, 1)
            .map_err(|e| format!("pid X11 : {e}"))?
            .reply()
        else {
            continue;
        };
        let Some(pid) = property.value32().and_then(|mut values| values.next()) else {
            continue;
        };
        if !belongs(pid) {
            continue;
        }
        // La fenêtre du jeu peut être un conteneur : le rendu se fait dans un
        // enfant. On prend tout le sous-arbre.
        let mut inner = vec![window];
        while let Some(target) = inner.pop() {
            if !found.contains(&target) {
                found.push(target);
            }
            if let Ok(tree) = conn
                .query_tree(target)
                .map_err(|e| format!("arbre X11 : {e}"))?
                .reply()
            {
                inner.extend(tree.children.iter().copied());
            }
        }
    }
    if found.is_empty() {
        for window in loose {
            let mut inner = vec![window];
            while let Some(target) = inner.pop() {
                if !found.contains(&target) {
                    found.push(target);
                }
                if let Ok(tree) = conn
                    .query_tree(target)
                    .map_err(|e| format!("arbre X11 : {e}"))?
                    .reply()
                {
                    inner.extend(tree.children.iter().copied());
                }
            }
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_gamescope_display() {
        let log = "[gamescope] [Info]  wlserver: [xwayland/server.c:108] Starting Xwayland on :1\n";
        assert_eq!(xwayland_display(log).as_deref(), Some(":1"));
    }

    #[test]
    fn keeps_the_last_display_when_gamescope_restarts_xwayland() {
        let log = "Starting Xwayland on :2\n... Starting Xwayland on :3\n";
        assert_eq!(xwayland_display(log).as_deref(), Some(":3"));
    }

    #[test]
    fn no_display_without_the_gamescope_line() {
        assert_eq!(xwayland_display("[gamescope] rien ici"), None);
        assert_eq!(xwayland_display("Starting Xwayland on :"), None);
    }
}
