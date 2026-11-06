//! La boutique d'un magasin, ouverte dans sa propre fenêtre WebKit.
//!
//! egui ne sait pas afficher de page web, et une vue rendue hors écran ne
//! reçoit aucun rafraîchissement : les animations n'y avancent pas et chaque
//! geste devait passer par une capture d'écran — un défilement saccadé, un
//! texte mou, et un coût de processeur permanent. La page est donc confiée à
//! **WebKitGTK**, le moteur de GNOME Web, dans une fenêtre à part : défilement,
//! animations, clavier et formulaires s'y comportent comme dans un navigateur,
//! parce que c'en est un.
//!
//! Lapinux garde la main sur cette fenêtre sans la dessiner : le processus hôte
//! (écrit à côté du binaire, en Python) lui parle par lignes JSON. Le fermer
//! arrête le processus ; l'interface le voit et remet son bouton à zéro.
//!
//! Rien n'oblige le bureau à passer par XWayland : la fenêtre est une fenêtre
//! Wayland comme une autre, et l'application reste native.

use eframe::egui;
use lapinux_core::paths::LapinuxPaths;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// L'hôte WebKit, embarqué dans le binaire et écrit dans le cache.
const HELPER: &str = include_str!("webview_helper.py");

enum Event {
    Loading(bool),
    Navigated(String),
    Closed,
}

struct Session {
    child: Child,
    stdin: ChildStdin,
    rx: mpsc::Receiver<Event>,
}

impl Session {
    fn send(&mut self, value: serde_json::Value) {
        let _ = writeln!(self.stdin, "{value}");
        let _ = self.stdin.flush();
    }
}

/// La fenêtre de boutique : au plus une à la fois, réutilisée d'un magasin à
/// l'autre (changer de magasin change la page, pas la fenêtre).
pub(crate) struct Webview {
    session: Option<Session>,
    url: String,
    title: String,
    loading: bool,
    error: Option<String>,
}

impl Default for Webview {
    fn default() -> Self {
        Self {
            session: None,
            url: String::new(),
            title: String::new(),
            loading: false,
            error: None,
        }
    }
}

impl Drop for Webview {
    fn drop(&mut self) {
        self.close();
    }
}

impl Webview {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Une fenêtre est-elle ouverte ?
    pub(crate) fn is_open(&self) -> bool {
        self.session.is_some()
    }

    /// L'adresse que la fenêtre affiche, quand elle est ouverte.
    pub(crate) fn url(&self) -> &str {
        &self.url
    }

    /// Ouvre le site d'un magasin — ou y amène la fenêtre déjà ouverte, ce qui
    /// évite d'accumuler les fenêtres au fil des clics.
    pub(crate) fn open(&mut self, paths: &LapinuxPaths, url: &str, title: &str) {
        self.error = None;
        if let Some(session) = self.session.as_mut() {
            session.send(serde_json::json!({
                "cmd": "load",
                "url": url,
                "title": title,
            }));
            self.url = url.to_string();
            self.title = title.to_string();
            return;
        }
        match spawn(paths, url, title) {
            Ok(session) => {
                self.session = Some(session);
                self.url = url.to_string();
                self.title = title.to_string();
                self.loading = true;
            }
            Err(message) => self.error = Some(message),
        }
    }

    /// Ramène la fenêtre au premier plan, quand elle est déjà ouverte.
    pub(crate) fn present(&mut self) {
        if let Some(session) = self.session.as_mut() {
            session.send(serde_json::json!({ "cmd": "present" }));
        }
    }

    /// Ferme la fenêtre et son processus.
    pub(crate) fn close(&mut self) {
        if let Some(mut session) = self.session.take() {
            session.send(serde_json::json!({ "cmd": "quit" }));
            // Le processus écrit ses cookies sur le disque en s'arrêtant : le
            // tuer aussitôt perdait la connexion à la boutique. On lui laisse
            // le temps de sortir proprement, et on le termine sinon.
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                match session.child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) => std::thread::sleep(Duration::from_millis(40)),
                    Err(_) => break,
                }
            }
            let _ = session.child.kill();
            let _ = session.child.wait();
        }
        self.url.clear();
        self.title.clear();
        self.loading = false;
    }

    /// Ce que l'interface montre de la fenêtre : ouverte sur telle adresse,
    /// en cours de chargement, ou la raison de l'échec.
    pub(crate) fn status(&mut self, ctx: &egui::Context) -> Option<String> {
        self.poll(ctx);
        if let Some(error) = self.error.clone() {
            return Some(error);
        }
        if self.session.is_none() {
            return None;
        }
        Some(if self.loading {
            "Chargement de la page…".to_string()
        } else {
            format!("Fenêtre ouverte · {}", host_of(&self.url))
        })
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let mut closed = false;
        let mut changed = false;
        loop {
            match session.rx.try_recv() {
                Ok(Event::Loading(value)) => {
                    self.loading = value;
                    changed = true;
                }
                Ok(Event::Navigated(url)) => {
                    if !url.is_empty() {
                        self.url = url;
                    }
                    changed = true;
                }
                Ok(Event::Closed) => {
                    closed = true;
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    closed = true;
                    break;
                }
            }
        }
        if closed {
            self.session = None;
            self.url.clear();
            self.title.clear();
            self.loading = false;
            ctx.request_repaint();
            return;
        }
        if changed {
            ctx.request_repaint();
        }
    }
}

/// Le nom de domaine, pour une ligne d'état courte.
fn host_of(url: &str) -> String {
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or(url)
        .trim_start_matches("www.")
        .to_string()
}

/// Lance la fenêtre WebKit : elle vit sa vie, Lapinux ne fait que lui parler.
fn spawn(paths: &LapinuxPaths, url: &str, title: &str) -> Result<Session, String> {
    let dir = paths.run_dir.join("webview");
    std::fs::create_dir_all(&dir).unwrap_or(());
    // Le script est embarqué : il est écrit là où le système le lira, même si
    // le cache de Lapinux n'est pas accessible en écriture.
    let script: PathBuf = dir.join("window.py");
    if std::fs::write(&script, HELPER).is_err() {
        let fallback = std::env::temp_dir().join("lapinux-webview.py");
        std::fs::write(&fallback, HELPER)
            .map_err(|error| format!("hôte WebKit non installable ({error})"))?;
        return spawn_with(&fallback, url, title, paths);
    }
    spawn_with(&script, url, title, paths)
}

/// Le pilote propriétaire NVIDIA expose ce fichier ; Mesa ne le crée pas.
fn nvidia_gpu() -> bool {
    Path::new("/proc/driver/nvidia/version").is_file()
}

fn spawn_with(
    script: &Path,
    url: &str,
    title: &str,
    paths: &LapinuxPaths,
) -> Result<Session, String> {
    let mut command = Command::new("python3");
    command
        .arg(script)
        .arg(title)
        .arg(url)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        // Steam sert ses bandes-annonces en DASH/HLS. WebKitGTK désactive
        // ces deux formats par défaut ; les greffons GStreamer du système
        // (hlsdemux, dashdemux) suffisent une fois ces variables posées.
        .env("WEBKIT_GST_ENABLE_HLS_SUPPORT", "1")
        .env("WEBKIT_GST_ENABLE_DASH_SUPPORT", "1");
    // Sur NVIDIA, les sinks vidéo DMA-BUF et GL de WebKit peignent tous deux
    // des images vertes. Seul le sink logiciel rend les bonnes couleurs ; on
    // ne touche pas à Mesa, où le zéro-copie fonctionne.
    if nvidia_gpu() {
        if std::env::var_os("WEBKIT_GST_DMABUF_SINK_DISABLED").is_none() {
            command.env("WEBKIT_GST_DMABUF_SINK_DISABLED", "1");
        }
        if std::env::var_os("WEBKIT_GST_DISABLE_GL_SINK").is_none() {
            command.env("WEBKIT_GST_DISABLE_GL_SINK", "1");
        }
    }
    // Les erreurs de WebKit et de GStreamer n'ont nulle part où aller : sans
    // ce journal, une vidéo qui reste noire ne laisse aucune trace.
    let _ = std::fs::create_dir_all(&paths.log_dir);
    // WebKit écrit dans le journal de l'interface : un fichier de moins à
    // consulter, et les erreurs de rendu y sont datées comme le reste.
    let ui_log = paths.log_dir.join("ui.log");
    lapinux_core::util::rotate_log(&ui_log);
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&ui_log)
    {
        Ok(log) => command.stderr(Stdio::from(log)),
        Err(_) => command.stderr(Stdio::null()),
    };
    // Le repli Mesa de l'interface ne doit pas descendre jusqu'à WebKit.
    // Sinon la page est rendue par llvmpipe, sur le processeur, même quand le
    // pilote graphique est disponible : c'est la cause des pages qui rament.
    // Le jeu a déjà le même traitement dans `lapinux-perf`.
    for key in [
        "__EGL_VENDOR_LIBRARY_FILENAMES",
        "__GLX_VENDOR_LIBRARY_NAME",
        "LIBGL_ALWAYS_SOFTWARE",
        "GALLIUM_DRIVER",
    ] {
        command.env_remove(key);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("python3 introuvable ({error})"))?;
    let Some(stdin) = child.stdin.take() else {
        let _ = child.kill();
        return Err("entrée de la fenêtre indisponible".into());
    };
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        return Err("sortie de la fenêtre indisponible".into());
    };
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            let event = match value.get("event").and_then(|event| event.as_str()) {
                Some("loading") => Event::Loading(
                    value
                        .get("value")
                        .and_then(|value| value.as_bool())
                        .unwrap_or(false),
                ),
                Some("url") => Event::Navigated(
                    value
                        .get("url")
                        .and_then(|url| url.as_str())
                        .unwrap_or_default()
                        .to_string(),
                ),
                Some("closed") => Event::Closed,
                _ => continue,
            };
            if tx.send(event).is_err() {
                break;
            }
        }
        let _ = tx.send(Event::Closed);
    });
    Ok(Session { child, stdin, rx })
}
