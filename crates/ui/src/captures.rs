//! Section Captures : les images et les vidéos prises en jeu par l'overlay.
//!
//! L'overlay écrit ses fichiers dans `~/Images`, nommés d'après la partie en
//! cours (`lapinux-<appid>-<jeu>-<unix>.png`). La section relit ce dossier sans
//! rien demander au réseau : ni Steam ni les boutiques ne sont interrogées, et
//! les jaquettes d'icône viennent du cache d'illustrations déjà sur le disque.
//!
//! Une capture s'ouvre **dans l'application** : l'image en grand, ou la vidéo
//! image par image. Rien ne part vers une application tierce, sauf le bouton
//! qui le demande explicitement.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant, UNIX_EPOCH};

use lapinux_core::config::CaptureConfig;
use lapinux_core::model::SteamCatalog;
use lapinux_core::paths::LapinuxPaths;

use super::{
    display_font, load_texture_file, segment_rect, segmented_pill, CARD_HOVER, PANEL, TEXT, TEXT_DIM,
};

/// Taille d'une carte : l'image occupe toute la surface, sans cadre gris.
const CARD_W: f32 = 236.0;
const THUMB_H: f32 = CARD_W * 9.0 / 16.0;
const CARD_H: f32 = THUMB_H;

/// Aperçu d'une vidéo : cinq images prises autour du milieu, changées à ce
/// rythme pendant le survol de la carte.
const PREVIEW_FRAMES: usize = 5;
const PREVIEW_AT: [f64; PREVIEW_FRAMES] = [0.25, 0.375, 0.5, 0.625, 0.75];
const PREVIEW_FRAME_SECONDS: f64 = 0.9;
/// Vidéos dont les cinq aperçus restent en mémoire.
const PREVIEWS_KEPT: usize = 8;

/// Cadence de relecture du dossier : une capture prise en jeu apparaît en
/// cinq secondes, sans scruter le disque à chaque image.
const SCAN: Duration = Duration::from_secs(5);

/// Vignettes gardées en mémoire : au-delà, le cache repart de zéro plutôt que
/// de garder toutes les captures d'une session en textures.
const THUMBS_KEPT: usize = 96;

/// Largeur de la vignette décodée, en pixels. La carte en fait 224 : le double
/// couvre un écran HiDPI sans garder une capture entière en texture.
const THUMB_PX: u32 = 448;

/// Côté de la corbeille posée sur la vignette.
const TRASH: f32 = 22.0;

/// Temps laissé pour confirmer une suppression : passé ce délai, la
/// corbeille redemande un clic au lieu de supprimer.
const CONFIRM: Duration = Duration::from_secs(4);

/// Largeur donnée à une vidéo relue dans l'application : au-delà, décoder
/// chaque image coûterait plus qu'il n'apporte.
const VIEW_MAX_W: u32 = 1280;

/// Fichier d'icône d'un jeu dans le cache local des illustrations, puis dans
/// celui du client Steam. Aucune requête réseau : une icône absente laisse la
/// pastille de repli.
pub(crate) fn icon_file(paths: &LapinuxPaths, appid: u32) -> Option<PathBuf> {
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
    super::steam_cached_icon(appid)
}

/// Ouvre une capture avec la visionneuse du système : l'application montre
/// tout elle-même, ce chemin ne sert qu'au bouton qui le demande.
pub(crate) fn open_external(path: &Path) -> Result<(), String> {
    std::process::Command::new("xdg-open")
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|err| lapinux_core::i18n::t_args("captures.open_error", &[("error", &err.to_string())]))
}

/// Ce que le fichier contient.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Media {
    Image,
    Video,
}

impl Media {
    fn of(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "png" | "jpg" | "jpeg" | "webp" | "gif" => Some(Media::Image),
            "mp4" | "webm" | "mkv" | "mov" | "avi" => Some(Media::Video),
            _ => None,
        }
    }
}

/// Aperçus d'une vidéo : extraction `ffmpeg` en cours, textures prêtes, ou
/// échec (fichier illisible, `ffmpeg` absent).
enum PreviewEntry {
    Loading(Receiver<Option<Vec<PathBuf>>>),
    Frames(Vec<egui::TextureHandle>),
    Failed,
}

/// Filtre du haut : tout, les seules images ou les seules vidéos.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    All,
    Image,
    Video,
}

impl Default for Kind {
    fn default() -> Self {
        Kind::All
    }
}

impl Kind {
    fn keeps(self, media: Media) -> bool {
        match self {
            Kind::All => true,
            Kind::Image => media == Media::Image,
            Kind::Video => media == Media::Video,
        }
    }
}

/// Une capture telle que le disque la décrit.
#[derive(Clone, Debug)]
struct Capture {
    path: PathBuf,
    media: Media,
    /// Nom du jeu, remis en forme depuis le nom du fichier. Vide pour une
    /// capture prise hors partie, qui n'a pas de jeu à montrer.
    game: String,
    appid: Option<u32>,
    /// Secondes Unix du moment de la capture.
    taken: i64,
    /// Poids du fichier en octets, relevé au même moment que son nom.
    size: u64,
}

impl Capture {
    /// Lit un ancien nom technique. Les captures d'une version antérieure
    /// n'ont pas de marqueur de dossier : elles restent reconnues.
    #[cfg(test)]
    fn from_path(path: &Path) -> Option<Self> {
        Self::from_capture_path(path, false)
    }

    /// Lit une capture. `include_new` est vrai pour un dossier marqué par
    /// Lapinux : le nom peut alors suivre le modèle choisi dans les réglages.
    fn from_capture_path(path: &Path, include_new: bool) -> Option<Self> {
        let stem = path.file_stem()?.to_str()?;
        let media = Media::of(path)?;
        if let Some(rest) = stem.strip_prefix("lapinux-") {
            let parts: Vec<&str> = rest.split('-').filter(|part| !part.is_empty()).collect();
            let taken = parts.last()?.parse::<i64>().ok()?;
            // Un ancien nom (`lapinux-<unix>`) n'a ni jeu ni appid : le seul
            // groupe est alors l'heure elle-même.
            let (appid, game) = if parts.len() < 2 {
                (None, String::new())
            } else {
                (
                    parts.first().and_then(|part| part.parse::<u32>().ok()),
                    unslug(&parts[1..parts.len() - 1]),
                )
            };
            return Some(Self {
                path: path.to_path_buf(),
                media,
                game,
                appid,
                taken,
                size: std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0),
            });
        }
        if !include_new {
            return None;
        }
        let taken = lapinux_core::captures::parse_stamp(stem)
            .or_else(|| modified_unix(path))
            .unwrap_or(0);
        let game = lapinux_core::captures::game_from_stem(stem);
        Some(Self {
            path: path.to_path_buf(),
            media,
            game,
            appid: None,
            taken,
            size: std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0),
        })
    }

    /// Titre affiché : le jeu, ou la mention hors partie.
    fn title(&self) -> String {
        if self.game.is_empty() {
            lapinux_core::i18n::t("captures.out_of_game").to_string()
        } else {
            self.game.clone()
        }
    }
}

/// Compare deux titres sans dépendre de la casse, des espaces ou de la
/// ponctuation. Un nom de capture peut perdre le « ™ » du magasin ; un jeu
/// peut aussi être une démo dont le titre garde ce suffixe.
fn same_game_name(left: &str, right: &str) -> bool {
    let left = normalized_game_name(left);
    let right = normalized_game_name(right);
    if left.is_empty() || right.is_empty() {
        return false;
    }
    if left == right {
        return true;
    }
    let pairs = [
        (left.as_str(), right.as_str()),
        (right.as_str(), left.as_str()),
    ];
    let similar = pairs.into_iter().any(|(long, short)| {
        long.strip_prefix(short).is_some_and(|extra| {
            matches!(extra, "demo" | "playtest" | "beta" | "prologue" | "trial")
        })
    });
    similar
}

fn normalized_game_name(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(|character| character.to_lowercase())
        .collect()
}

/// Remet un nom de jeu lisible depuis son passage en nom de fichier :
/// `team-fortress-2` redevient « Team Fortress 2 ».
fn unslug(parts: &[&str]) -> String {
    parts
        .iter()
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Relit le dossier des captures, la plus récente en tête.
fn scan_dir(dir: &Path) -> Vec<Capture> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    // Un dossier marqué est entièrement à Lapinux : les noms personnalisés
    // peuvent ne plus commencer par `lapinux-`. Sans marqueur, seuls les noms
    // historiques sont reconnus, pour ne pas aspirer les vidéos personnelles.
    let marked = dir.join(lapinux_core::captures::MARKER).is_file();
    let mut out: Vec<Capture> = entries
        .flatten()
        .filter(|entry| {
            if marked {
                return true;
            }
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("lapinux-"))
        })
        .filter_map(|entry| Capture::from_capture_path(&entry.path(), marked))
        .collect();
    out.sort_by(|a, b| b.taken.cmp(&a.taken).then_with(|| b.path.cmp(&a.path)));
    out
}

/// Relit les deux dossiers de captures — vidéos et photos vivent séparément —
/// et rend la plus récente en tête.
fn scan_dirs(dirs: &[&Path]) -> Vec<Capture> {
    let mut out: Vec<Capture> = Vec::new();
    for dir in dirs {
        out.extend(scan_dir(dir));
    }
    out.sort_by(|a, b| b.taken.cmp(&a.taken).then_with(|| b.path.cmp(&a.path)));
    out
}

/// État de la section : ce que le disque contenait à la dernière relecture,
/// les vignettes déjà décodées et la capture ouverte en grand.
#[derive(Default)]
pub(crate) struct Gallery {
    entries: Vec<Capture>,
    filter: Kind,
    scanned: Option<Instant>,
    /// Vignettes : un fil de fond travaille encore, ou l'image est prête.
    thumbs: HashMap<PathBuf, ThumbEntry>,
    icons: HashMap<u32, Option<egui::TextureHandle>>,
    /// Cinq images d'aperçu par vidéo, extraites à la demande.
    preview_cache: HashMap<PathBuf, PreviewEntry>,
    /// Début du survol de chaque vidéo : l'aperçu repart de sa première image.
    preview_started: HashMap<PathBuf, f64>,
    /// Capture ouverte dans la visionneuse : elle prend la place de la grille.
    viewing: Option<Viewer>,
    /// Fichier dont la corbeille a été cliquée une fois, avec l'instant du
    /// clic : le second clic supprime, le premier ne fait que demander
    /// confirmation.
    pending_delete: Option<(PathBuf, Instant)>,
    /// Dossiers de la dernière lecture : un changement dans les réglages
    /// relance la galerie sans attendre les cinq secondes habituelles.
    scanned_dirs: Option<(PathBuf, PathBuf)>,
}

impl Gallery {
    /// Nombre de captures retenues par le filtre, pour la ligne Discord.
    pub(crate) fn count(&self) -> usize {
        self.entries
            .iter()
            .filter(|capture| self.filter.keeps(capture.media))
            .count()
    }

    /// Dessine la section : la grille des captures, ou celle qui est ouverte.
    pub(crate) fn page(
        &mut self,
        ui: &mut egui::Ui,
        paths: &LapinuxPaths,
        config: &CaptureConfig,
        games: &[lapinux_core::model::Game],
        catalog: Option<&SteamCatalog>,
    ) {
        let video_dir = config.videos_dir();
        let image_dir = config.images_dir();
        self.refresh(&video_dir, &image_dir);

        if self.viewing.is_some() {
            let mut close = false;
            if let Some(viewer) = self.viewing.as_mut() {
                // Le fichier a pu disparaître sous la visionneuse : retour
                // direct à la grille.
                close = !viewer.path.exists() || viewer.show(ui);
            }
            if close {
                self.viewing = None;
            }
            if self.viewing.is_some() {
                return;
            }
        }

        let images = self
            .entries
            .iter()
            .filter(|capture| capture.media == Media::Image)
            .count();
        let videos = self.entries.len() - images;

        ui.horizontal(|ui| {
            if let Some(kind) = filter_group(ui, self.filter, images, videos) {
                self.filter = kind;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(lapinux_core::i18n::t_args(
                        "captures.paths",
                        &[
                            ("videos", &video_dir.display().to_string()),
                            ("images", &image_dir.display().to_string()),
                        ],
                    ))
                    .size(11.0)
                    .color(TEXT_DIM()),
                )
                .on_hover_text(
                    "L'overlay range ses captures ici ; la section relit les dossiers toutes \
                     les cinq secondes.",
                );
            });
        });
        ui.add_space(14.0);

        let shown: Vec<usize> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, capture)| self.filter.keeps(capture.media))
            .map(|(index, _)| index)
            .collect();

        if shown.is_empty() {
            empty_state(ui, &video_dir);
            return;
        }

        let mut picked = None;
        let mut asked = None;
        let mut removed = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(14.0, 14.0);
                    for index in shown {
                        let capture = self.entries[index].clone();
                        let (rect, response) = ui
                            .allocate_exact_size(egui::vec2(CARD_W, CARD_H), egui::Sense::click());
                        // Une carte hors écran n'est pas décodée : faire
                        // défiler la grille ne coûte que ce qui se voit.
                        let visible = ui.is_rect_visible(rect);
                        // Rien n'est décodé hors écran, et ce qui se voit est
                        // décodé en fond : la grille s'affiche tout de suite.
                        let (thumb, duration) = if visible {
                            self.thumb(ui.ctx(), &capture)
                        } else {
                            (None, None)
                        };
                        // Le nouveau nom ne porte plus l'appid : le jeu est
                        // retrouvé par son titre dans la bibliothèque quand
                        // elle le connaît encore.
                        let appid = capture.appid.or_else(|| {
                            games
                                .iter()
                                .find(|game| same_game_name(&game.name, &capture.game))
                                .map(|game| game.appid)
                        }).or_else(|| {
                            catalog.and_then(|catalog| {
                                catalog
                                    .entries
                                    .iter()
                                    .find(|entry| same_game_name(&entry.name, &capture.game))
                                    .map(|entry| entry.appid)
                            })
                        });
                        let icon = self.icon(ui.ctx(), paths, appid);
                        // L'aperçu animé ne se fabrique qu'au survol : les
                        // vidéos hors écran ne lancent aucun ffmpeg.
                        let previews = if capture.media == Media::Video
                            && visible
                            && response.hovered()
                        {
                            self.previews(ui.ctx(), &capture)
                        } else {
                            None
                        };
                        let preview = previews.as_ref().and_then(|frames| {
                            if frames.is_empty() {
                                return None;
                            }
                            let now = ui.ctx().input(|input| input.time);
                            let started =
                                *self.preview_started.entry(capture.path.clone()).or_insert(now);
                            if frames.len() > 1 {
                                ui.ctx().request_repaint_after(Duration::from_millis(120));
                            }
                            let step = ((now - started).max(0.0) / PREVIEW_FRAME_SECONDS) as usize;
                            frames.get(step % frames.len())
                        });
                        if !response.hovered() {
                            self.preview_started.remove(&capture.path);
                        }
                        let trash = trash_rect(rect);
                        let folder = folder_rect(rect);
                        let pending = self
                            .pending_delete
                            .as_ref()
                            .is_some_and(|(path, at)| path == &capture.path && at.elapsed() < CONFIRM);
                        let over_trash = visible && ui.rect_contains_pointer(trash);
                        let over_folder = visible && ui.rect_contains_pointer(folder);
                        paint_card(
                            ui,
                            rect,
                            &capture,
                            thumb.as_ref(),
                            preview,
                            icon.as_ref(),
                            duration,
                            response.hovered(),
                            over_trash,
                            over_folder,
                            pending,
                        );
                        let response = response
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .on_hover_text(hint(&capture, pending));
                        if response.clicked() {
                            let at = response.interact_pointer_pos();
                            if at.is_some_and(|at| folder.contains(at)) {
                                // Le fichier reste dans la galerie : ici on
                                // ouvre le dossier qui le contient.
                                if let Some(dir) = capture.path.parent() {
                                    if let Err(err) = open_external(dir) {
                                        tracing::warn!("ouverture du dossier : {err}");
                                    }
                                }
                            } else if at.is_some_and(|at| trash.contains(at)) {
                                if pending {
                                    removed = Some(capture.path.clone());
                                } else {
                                    asked = Some(capture.path.clone());
                                }
                            } else {
                                picked = Some(capture.path.clone());
                            }
                        }
                    }
                });
            });

        if let Some(path) = removed {
            self.pending_delete = None;
            match std::fs::remove_file(&path) {
                Ok(()) => {
                    self.thumbs.remove(&path);
                    self.preview_cache.remove(&path);
                    self.preview_started.remove(&path);
                    // La grille se met à jour tout de suite : attendre la
                    // relecture donnerait l'impression d'un clic perdu.
                    self.entries.retain(|capture| capture.path != path);
                    self.scanned = None;
                }
                Err(err) => {
                    tracing::warn!("suppression de {} : {err}", path.display());
                    self.scanned = None;
                }
            }
        } else if let Some(path) = asked {
            self.pending_delete = Some((path, Instant::now()));
        } else if let Some(path) = picked {
            self.pending_delete = None;
            if let Some(entry) = self.entries.iter().find(|entry| entry.path == path) {
                self.viewing = Some(Viewer::open(ui.ctx(), entry));
            }
        }
    }

    /// Relit les dossiers, au plus toutes les cinq secondes. Un changement de
    /// dossier dans les réglages force la relecture immédiatement.
    fn refresh(&mut self, video_dir: &Path, image_dir: &Path) {
        let dirs = (video_dir.to_path_buf(), image_dir.to_path_buf());
        if self.scanned_dirs.as_ref() != Some(&dirs) {
            self.scanned = None;
            self.scanned_dirs = Some(dirs.clone());
        }
        if !self.scanned.map_or(true, |at| at.elapsed() >= SCAN) {
            return;
        }
        self.scanned = Some(Instant::now());
        self.entries = scan_dirs(&[&dirs.0, &dirs.1]);
        let present: HashSet<&PathBuf> = self
            .entries
            .iter()
            .map(|capture| &capture.path)
            .collect();
        self.thumbs.retain(|path, _| present.contains(path));
        self.preview_cache.retain(|path, _| present.contains(path));
        self.preview_started.retain(|path, _| present.contains(path));
        if self.thumbs.len() > THUMBS_KEPT {
            self.thumbs.clear();
        }
        if let Some((pending, _)) = self.pending_delete.as_ref() {
            if !present.contains(pending) {
                self.pending_delete = None;
            }
        }
    }

    /// Vignette et durée d'une capture. Le fichier est lu, décodé et, pour une
    /// vidéo, interrogé par `ffprobe` puis découpé par `ffmpeg` dans un fil de
    /// fond : la page s'ouvre sans attendre ces secondes-là, et les cartes se
    /// remplissent au fil de l'eau.
    fn thumb(
        &mut self,
        ctx: &egui::Context,
        capture: &Capture,
    ) -> (Option<egui::TextureHandle>, Option<f64>) {
        let path = capture.path.clone();
        if let Some(entry) = self.thumbs.get_mut(&path) {
            match entry {
                ThumbEntry::Ready(thumb) => return (thumb.image.clone(), thumb.duration),
                ThumbEntry::Loading(receiver) => match receiver.try_recv() {
                    Ok(loaded) => {
                        let name = format!("capture-{:016x}", path_id(&path));
                        let image = loaded
                            .image
                            .map(|image| texture_from_image(ctx, image, &name, THUMB_PX));
                        *entry = ThumbEntry::Ready(ReadyThumb {
                            image: image.clone(),
                            duration: loaded.duration,
                        });
                        return (image, loaded.duration);
                    }
                    Err(mpsc::TryRecvError::Empty) => {
                        ctx.request_repaint_after(Duration::from_millis(120));
                        return (None, None);
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        *entry = ThumbEntry::Ready(ReadyThumb {
                            image: None,
                            duration: None,
                        });
                        return (None, None);
                    }
                },
            }
        }
        let (tx, receiver) = mpsc::channel();
        let thread_capture = capture.clone();
        std::thread::spawn(move || {
            let _ = tx.send(load_thumb(&thread_capture));
        });
        self.thumbs.insert(path, ThumbEntry::Loading(receiver));
        ctx.request_repaint_after(Duration::from_millis(120));
        (None, None)
    }

    /// Icône du jeu, lue dans le cache local des illustrations. Aucune
    /// requête réseau : une icône absente laisse la pastille de repli.
    fn icon(
        &mut self,
        ctx: &egui::Context,
        paths: &LapinuxPaths,
        appid: Option<u32>,
    ) -> Option<egui::TextureHandle> {
        let appid = appid?;
        if let Some(cached) = self.icons.get(&appid) {
            return cached.clone();
        }
        let found = icon_file(paths, appid)
            .and_then(|path| load_texture_file(ctx, &path, &format!("capture-icon-{appid}")));
        self.icons.insert(appid, found.clone());
        found
    }

    /// Cinq images d'une vidéo, extraites en tâche de fond au premier survol.
    /// Le dessin de la grille n'attend donc pas ffmpeg : la vignette fixe
    /// reste affichée jusqu'à ce que les textures soient prêtes.
    fn previews(
        &mut self,
        ctx: &egui::Context,
        capture: &Capture,
    ) -> Option<Vec<egui::TextureHandle>> {
        if capture.media != Media::Video {
            return None;
        }
        let path = capture.path.clone();
        if let Some(entry) = self.preview_cache.get_mut(&path) {
            match entry {
                PreviewEntry::Frames(frames) => return Some(frames.clone()),
                PreviewEntry::Failed => return None,
                PreviewEntry::Loading(receiver) => match receiver.try_recv() {
                    Ok(Some(files)) => {
                        let frames = load_preview_textures(ctx, &path, &files);
                        if frames.is_empty() {
                            *entry = PreviewEntry::Failed;
                            return None;
                        }
                        *entry = PreviewEntry::Frames(frames.clone());
                        return Some(frames);
                    }
                    Ok(None) | Err(mpsc::TryRecvError::Disconnected) => {
                        *entry = PreviewEntry::Failed;
                        return None;
                    }
                    Err(mpsc::TryRecvError::Empty) => {
                        ctx.request_repaint_after(Duration::from_millis(180));
                        return None;
                    }
                },
            }
        }
        let (tx, receiver) = mpsc::channel();
        let thread_path = path.clone();
        std::thread::spawn(move || {
            let _ = tx.send(video_preview_files(&thread_path));
        });
        self.preview_cache
            .insert(path, PreviewEntry::Loading(receiver));
        if self.preview_cache.len() > PREVIEWS_KEPT {
            if let Some(stale) = self
                .preview_cache
                .keys()
                .find(|cached| cached.as_path() != capture.path.as_path())
                .cloned()
            {
                self.preview_cache.remove(&stale);
            }
        }
        ctx.request_repaint_after(Duration::from_millis(220));
        None
    }
}

/// Bandeau du haut : les trois vues, comptées comme les filtres de la
/// Bibliothèque.
fn filter_group(ui: &mut egui::Ui, current: Kind, images: usize, videos: usize) -> Option<Kind> {
    let font = egui::FontId::proportional(11.5);
    let items = [
        (Kind::All, format!("Tout · {}", images + videos)),
        (Kind::Image, format!("Images · {images}")),
        (Kind::Video, format!("Vidéos · {videos}")),
    ];
    let widths: Vec<f32> = items
        .iter()
        .map(|(_, text)| {
            ui.fonts(|fonts| fonts.layout_no_wrap(text.clone(), font.clone(), TEXT()).size().x)
                + 26.0
        })
        .collect();
    let active: Vec<bool> = items
        .iter()
        .map(|(kind, _)| current == *kind)
        .collect();
    let (rect, responses) = segmented_pill(ui, &widths, &active, "captures");
    let painter = ui.painter();
    let mut chosen = None;
    for (index, (kind, text)) in items.iter().enumerate() {
        let segment = segment_rect(rect, &widths, index);
        let ink = if current == *kind || responses[index].hovered() {
            TEXT()
        } else {
            TEXT_DIM()
        };
        painter.text(
            segment.center(),
            egui::Align2::CENTER_CENTER,
            text,
            font.clone(),
            ink,
        );
        if responses[index].clicked() {
            chosen = Some(*kind);
        }
    }
    chosen
}

/// Message d'attente : la section ne fabrique rien elle-même, elle montre ce
/// que l'overlay a déjà écrit.
fn empty_state(ui: &mut egui::Ui, dir: &Path) {
    ui.add_space((ui.available_height() * 0.28).max(24.0));
    ui.vertical_centered(|ui| {
        ui.label(
            egui::RichText::new("Aucune capture pour l'instant")
                .size(15.0)
                .color(TEXT()),
        );
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(
                "En partie, la ligne « Capture d'écran » de l'overlay range l'image dans ce \
                 dossier : elle apparaît ici quelques secondes plus tard.",
            )
            .size(12.0)
            .color(TEXT_DIM()),
        );
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(dir.display().to_string())
                .size(11.0)
                .color(TEXT_DIM()),
        );
    });
}

/// Actions de la carte : posées en bas à droite de l'image, visibles au
/// survol. La corbeille reste à l'extrême droite, le dossier juste avant.
fn trash_rect(card: egui::Rect) -> egui::Rect {
    let margin = 8.0;
    let max = egui::pos2(card.right() - margin, card.bottom() - margin);
    egui::Rect::from_min_max(
        egui::pos2(max.x - TRASH, max.y - TRASH),
        max,
    )
}

fn folder_rect(rect: egui::Rect) -> egui::Rect {
    let trash = trash_rect(rect);
    let gap = 6.0;
    egui::Rect::from_min_max(
        egui::pos2(trash.left() - gap - TRASH, trash.top()),
        egui::pos2(trash.left() - gap, trash.bottom()),
    )
}

/// Une carte : l'image occupe toute la surface. Le jeu se lit dans un dégradé
/// en haut à gauche, le type en haut à droite, l'heure en bas ; les actions
/// apparaissent au survol.
fn paint_card(
    ui: &egui::Ui,
    rect: egui::Rect,
    capture: &Capture,
    thumb: Option<&egui::TextureHandle>,
    preview: Option<&egui::TextureHandle>,
    icon: Option<&egui::TextureHandle>,
    duration: Option<f64>,
    hovered: bool,
    over_trash: bool,
    over_folder: bool,
    pending_delete: bool,
) {
    let painter = ui.painter();
    let thumb_rect = rect;
    painter.rect_filled(thumb_rect, 0.0, PANEL());
    let frame = preview.or(thumb);
    match (capture.media, frame) {
        (_, Some(texture)) => {
            painter.image(texture.id(), thumb_rect, uv_full(), egui::Color32::WHITE);
        }
        (Media::Image, None) => paint_camera_glyph(painter, thumb_rect.center(), 22.0, TEXT_DIM()),
        (Media::Video, None) => paint_video_glyph(painter, thumb_rect.center(), 28.0, TEXT_DIM()),
    }

    // Deux dégradés rendent le jeu et l'heure lisibles sans poser de bandeau
    // opaque sur l'image.
    paint_thumb_scrim(painter, thumb_rect);

    let icon_rect = egui::Rect::from_min_size(
        thumb_rect.min + egui::vec2(10.0, 10.0),
        egui::vec2(20.0, 20.0),
    );
    match icon {
        Some(texture) => {
            painter.rect_filled(icon_rect, egui::Rounding::same(4.0), PANEL());
            painter.image(texture.id(), icon_rect, uv_full(), egui::Color32::WHITE);
        }
        None => paint_initial_badge(painter, icon_rect, &capture.game),
    }

    let duration_font = egui::FontId::proportional(9.5);
    let duration_label = duration.map(clock);
    let duration_width = duration_label.as_ref().map(|label| {
        ui.fonts(|fonts| {
            fonts
                .layout_no_wrap(label.clone(), duration_font.clone(), TEXT())
                .size()
                .x
        })
    });
    let badge = media_badge_rect(thumb_rect, capture.media, duration_width);
    paint_media_badge(painter, badge, capture.media, duration_label.as_deref());

    let text_left = icon_rect.right() + 8.0;
    let text_width = (badge.left() - 8.0 - text_left).max(24.0);
    let title_font = display_font(12.5);
    painter.text(
        egui::pos2(text_left, icon_rect.center().y),
        egui::Align2::LEFT_CENTER,
        elide(ui, &capture.title(), &title_font, text_width),
        title_font,
        TEXT(),
    );

    let action_width = if hovered || pending_delete {
        TRASH * 2.0 + 18.0
    } else {
        0.0
    };
    let meta_width = (thumb_rect.width() - 20.0 - action_width).max(60.0);
    let date_font = egui::FontId::proportional(10.0);
    let when = format!(
        "{} · {}",
        lapinux_core::util::format_datetime(capture.taken),
        lapinux_core::util::format_bytes(capture.size)
    );
    painter.text(
        egui::pos2(thumb_rect.left() + 10.0, thumb_rect.bottom() - 8.0),
        egui::Align2::LEFT_BOTTOM,
        elide(ui, &when, &date_font, meta_width),
        date_font,
        egui::Color32::from_rgb(220, 222, 225),
    );

    if hovered || pending_delete {
        let trash = trash_rect(rect);
        let ink = if pending_delete {
            egui::Color32::from_rgb(231, 148, 148)
        } else {
            TEXT()
        };
        painter.rect_filled(
            trash.expand(3.0),
            egui::Rounding::same(6.0),
            egui::Color32::from_black_alpha(if over_trash { 190 } else { 130 }),
        );
        paint_trash_glyph(painter, trash.center(), 13.0, ink);
        let folder = folder_rect(rect);
        painter.rect_filled(
            folder.expand(3.0),
            egui::Rounding::same(6.0),
            egui::Color32::from_black_alpha(if over_folder { 190 } else { 130 }),
        );
        paint_folder_glyph(painter, folder.center(), 13.0, TEXT());
    }
}

/// Pastille du type de fichier, en haut à droite de la vignette. La largeur
/// suit la durée réellement affichée : pas de place réservée dans le vide.
fn media_badge_rect(thumb: egui::Rect, media: Media, duration_width: Option<f32>) -> egui::Rect {
    let width = match (media, duration_width) {
        (Media::Video, Some(text)) => 8.0 + text + 4.0 + 13.0 + 8.0,
        _ => 28.0,
    };
    let right = thumb.right() - 10.0;
    let top = thumb.top() + 10.0;
    egui::Rect::from_min_size(
        egui::pos2(right - width, top),
        egui::vec2(width, 22.0),
    )
}

fn paint_media_badge(
    painter: &egui::Painter,
    rect: egui::Rect,
    media: Media,
    duration: Option<&str>,
) {
    painter.rect_filled(
        rect,
        egui::Rounding::same(6.0),
        egui::Color32::from_black_alpha(155),
    );
    match media {
        Media::Image => paint_camera_glyph(painter, rect.center(), 14.0, TEXT()),
        Media::Video => {
            if let Some(duration) = duration {
                painter.text(
                    egui::pos2(rect.left() + 8.0, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    duration,
                    egui::FontId::proportional(9.5),
                    TEXT(),
                );
                paint_video_glyph(
                    painter,
                    egui::pos2(rect.right() - 14.5, rect.center().y),
                    13.0,
                    TEXT(),
                );
            } else {
                paint_video_glyph(painter, rect.center(), 15.0, TEXT());
            }
        }
    }
}

/// Dégradé haut pour le jeu, dégradé bas pour la date : l'image reste visible
/// entre les deux.
fn paint_thumb_scrim(painter: &egui::Painter, rect: egui::Rect) {
    const STEPS: usize = 12;
    let top_h = 44.0_f32.min(rect.height() * 0.5);
    for index in 0..STEPS {
        let t = index as f32 / STEPS as f32;
        let next = (index + 1) as f32 / STEPS as f32;
        let alpha = (120.0 * (1.0 - t)).round() as u8;
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(rect.left(), rect.top() + top_h * t),
                egui::pos2(rect.right(), rect.top() + top_h * next),
            ),
            0.0,
            egui::Color32::from_black_alpha(alpha),
        );
    }
    let bottom_h = 32.0_f32.min(rect.height() * 0.4);
    for index in 0..STEPS {
        let t = index as f32 / STEPS as f32;
        let next = (index + 1) as f32 / STEPS as f32;
        let alpha = (145.0 * next).round() as u8;
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(rect.left(), rect.bottom() - bottom_h + bottom_h * t),
                egui::pos2(rect.right(), rect.bottom() - bottom_h + bottom_h * next),
            ),
            0.0,
            egui::Color32::from_black_alpha(alpha),
        );
    }
}

/// Caméscope au trait, distinct de l'appareil photo des images.
fn paint_video_glyph(
    painter: &egui::Painter,
    center: egui::Pos2,
    size: f32,
    ink: egui::Color32,
) {
    let scale = size / 18.0;
    let stroke = egui::Stroke::new(1.4_f32 * scale, ink);
    let body = egui::Rect::from_center_size(
        center + egui::vec2(-1.5 * scale, 0.0),
        egui::vec2(11.0, 9.0) * scale,
    );
    painter.rect_stroke(body, egui::Rounding::same(2.0 * scale), stroke);
    painter.add(egui::Shape::convex_polygon(
        vec![
            center + egui::vec2(3.8 * scale, -2.8 * scale),
            center + egui::vec2(7.8 * scale, -4.4 * scale),
            center + egui::vec2(7.8 * scale, 4.4 * scale),
            center + egui::vec2(3.8 * scale, 2.8 * scale),
        ],
        ink,
        egui::Stroke::NONE,
    ));
}

/// Texte de l'infobulle : le jeu, ce que le fichier contient, quand il a été
/// pris, son poids et où il se trouve.
fn hint(capture: &Capture, pending_delete: bool) -> String {
    if pending_delete {
        return "Cliquer à nouveau sur la corbeille supprime ce fichier.".to_string();
    }
    let when = lapinux_core::util::format_datetime(capture.taken);
    let what = match capture.media {
        Media::Image => "Image",
        Media::Video => "Vidéo",
    };
    let head = if capture.game.is_empty() {
        format!("{what} du {when}")
    } else {
        format!("{} · {what}\n{when}", capture.game)
    };
    format!(
        "{head}\n{}\n{}\nCliquer pour ouvrir",
        lapinux_core::util::format_bytes(capture.size),
        capture.path.display()
    )
}

/// Pictogramme de l'appareil photo, au trait comme les autres icônes de
/// navigation : le sous-ensemble Remix embarqué n'en a pas.
pub(crate) fn paint_camera_glyph(
    painter: &egui::Painter,
    center: egui::Pos2,
    size: f32,
    ink: egui::Color32,
) {
    let scale = size / 19.0;
    let stroke = egui::Stroke::new(1.5_f32 * scale, ink);
    let body = egui::Rect::from_center_size(
        center + egui::vec2(0.0, 0.5 * scale),
        egui::vec2(16.0, 11.0) * scale,
    );
    painter.rect_stroke(body, egui::Rounding::same(3.0 * scale), stroke);
    painter.circle_stroke(center + egui::vec2(0.0, 0.5 * scale), 2.7 * scale, stroke);
    painter.rect_filled(
        egui::Rect::from_center_size(
            center + egui::vec2(0.0, -5.4 * scale),
            egui::vec2(6.4, 2.6) * scale,
        ),
        egui::Rounding::same(1.0 * scale),
        ink,
    );
}

/// Bouton de lecture d'une vidéo : un rond et son triangle.
fn paint_play_glyph(painter: &egui::Painter, center: egui::Pos2, ink: egui::Color32) {
    painter.circle_stroke(center, 15.0, egui::Stroke::new(1.4_f32, ink));
    painter.add(egui::Shape::convex_polygon(
        vec![
            center + egui::vec2(-3.5, -6.0),
            center + egui::vec2(6.5, 0.0),
            center + egui::vec2(-3.5, 6.0),
        ],
        ink,
        egui::Stroke::NONE,
    ));
}

/// Corbeille au trait : couvercle, anse et corps.
fn paint_trash_glyph(painter: &egui::Painter, center: egui::Pos2, size: f32, ink: egui::Color32) {
    let scale = size / 13.0;
    let stroke = egui::Stroke::new(1.3_f32 * scale, ink);
    let lid_y = center.y - 4.2 * scale;
    painter.line_segment(
        [
            egui::pos2(center.x - 5.4 * scale, lid_y),
            egui::pos2(center.x + 5.4 * scale, lid_y),
        ],
        stroke,
    );
    painter.line_segment(
        [
            egui::pos2(center.x - 1.9 * scale, lid_y - 2.4 * scale),
            egui::pos2(center.x + 1.9 * scale, lid_y - 2.4 * scale),
        ],
        stroke,
    );
    let body = egui::Rect::from_min_max(
        egui::pos2(center.x - 4.2 * scale, lid_y),
        egui::pos2(center.x + 4.2 * scale, center.y + 5.4 * scale),
    );
    painter.rect_stroke(body, egui::Rounding::same(1.6 * scale), stroke);
    for offset in [-1.8_f32, 1.8] {
        painter.line_segment(
            [
                egui::pos2(center.x + offset * scale, lid_y + 2.0 * scale),
                egui::pos2(center.x + offset * scale, center.y + 3.4 * scale),
            ],
            egui::Stroke::new(1.0_f32 * scale, ink),
        );
    }
}

/// Dossier au trait : le corps, son onglet et le rabat du haut.
fn paint_folder_glyph(painter: &egui::Painter, center: egui::Pos2, size: f32, ink: egui::Color32) {
    let scale = size / 13.0;
    let stroke = egui::Stroke::new(1.3_f32 * scale, ink);
    let body = egui::Rect::from_min_max(
        egui::pos2(center.x - 5.6 * scale, center.y - 3.0 * scale),
        egui::pos2(center.x + 5.6 * scale, center.y + 4.8 * scale),
    );
    painter.rect_stroke(body, egui::Rounding::same(1.4 * scale), stroke);
    // Onglet : un petit trait qui sort du coin haut gauche.
    painter.line_segment(
        [
            egui::pos2(center.x - 5.6 * scale, center.y - 5.4 * scale),
            egui::pos2(center.x - 1.4 * scale, center.y - 5.4 * scale),
        ],
        stroke,
    );
    painter.line_segment(
        [
            egui::pos2(center.x - 1.4 * scale, center.y - 5.4 * scale),
            egui::pos2(center.x - 1.4 * scale, center.y - 3.0 * scale),
        ],
        stroke,
    );
}

/// Pastille de repli quand l'icône du jeu n'est pas en cache : son initiale.
fn paint_initial_badge(painter: &egui::Painter, rect: egui::Rect, game: &str) {
    painter.circle_filled(rect.center(), rect.width() / 2.0, CARD_HOVER());
    let letter = game
        .chars()
        .next()
        .map(|first| first.to_uppercase().to_string())
        .unwrap_or_else(|| "•".into());
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        letter,
        egui::FontId::proportional(10.0),
        TEXT_DIM(),
    );
}

/// Coupe un texte à la largeur disponible, points de suspension compris.
fn elide(ui: &egui::Ui, text: &str, font: &egui::FontId, max: f32) -> String {
    let width = |text: &str| {
        ui.fonts(|fonts| fonts.layout_no_wrap(text.to_owned(), font.clone(), TEXT()).size().x)
    };
    if width(text) <= max {
        return text.to_owned();
    }
    let mut kept: Vec<char> = text.chars().collect();
    while !kept.is_empty() {
        kept.pop();
        let candidate = format!("{}…", kept.iter().collect::<String>().trim_end());
        if width(&candidate) <= max {
            return candidate;
        }
    }
    "…".into()
}

/// Vignette et durée d'une capture, telles qu'un fil de fond les rapporte.
struct Thumb {
    image: Option<image::DynamicImage>,
    duration: Option<f64>,
}

/// Entrée du cache : le fil de fond travaille encore, ou la vignette est prête.
enum ThumbEntry {
    Loading(Receiver<Thumb>),
    Ready(ReadyThumb),
}

/// Vignette déjà téléversée en texture, avec la durée de la vidéo.
struct ReadyThumb {
    image: Option<egui::TextureHandle>,
    duration: Option<f64>,
}

/// Lit, décode et mesure une capture à la taille d'une vignette. Le fichier
/// fait plusieurs milliers de pixels de large : gardé entier en texture, il
/// pèserait huit mégaoctets pour une carte de 224 pixels. Tout ce qui coûte —
/// disque, `ffprobe`, `ffmpeg` — se fait ici, jamais pendant le dessin.
fn load_thumb(capture: &Capture) -> Thumb {
    let duration = match capture.media {
        Media::Video => video_duration(&capture.path)
            .filter(|seconds| seconds.is_finite() && *seconds > 0.0),
        Media::Image => None,
    };
    let source = match capture.media {
        Media::Image => std::fs::read(&capture.path).ok(),
        Media::Video => video_still(&capture.path, duration)
            .and_then(|still| std::fs::read(&still).ok()),
    };
    Thumb {
        image: source.and_then(|bytes| image::load_from_memory(&bytes).ok()),
        duration,
    }
}

/// Image fixe d'une vidéo, prise au milieu : les premières secondes sont
/// souvent noires (logo, fondu), le milieu montre la scène. Le nouveau nom de
/// cache invalide les anciennes images extraites au début.
fn video_still(path: &Path, known_duration: Option<f64>) -> Option<PathBuf> {
    let dir = LapinuxPaths::discover().cache_dir.join("thumbs");
    std::fs::create_dir_all(&dir).ok()?;
    let out = dir.join(format!("{:016x}-middle.png", path_id(path)));
    if preview_file_ok(&out) {
        return Some(out);
    }
    let duration = known_duration
        .or_else(|| video_duration(path))
        .unwrap_or(0.0);
    let at = if duration.is_finite() && duration > 0.0 {
        duration / 2.0
    } else {
        0.5
    };
    if extract_frame(path, at, &out) {
        return Some(out);
    }
    // Une vidéo plus courte que le point visé ne donne rien : on réessaie au
    // tout début plutôt que de laisser une carte vide.
    let _ = std::fs::remove_file(&out);
    extract_frame(path, 0.0, &out).then_some(out)
}

fn extract_frame(path: &Path, at: f64, out: &Path) -> bool {
    let Some(ffmpeg) = lapinux_core::paths::which("ffmpeg") else {
        return false;
    };
    let mut command = Command::new(ffmpeg);
    command.args(["-hide_banner", "-loglevel", "error", "-y"]);
    if at > 0.0 {
        command.arg("-ss").arg(format!("{at:.3}"));
    }
    command
        .arg("-i")
        .arg(path)
        .args(["-frames:v", "1", "-vf", &format!("scale={THUMB_PX}:-2")])
        .arg(out)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
        && preview_file_ok(out)
}

/// Cinq images réparties dans la vidéo. Le cache est relu avant de lancer
/// `ffmpeg` : un survol ultérieur ne redécode rien.
fn video_preview_files(path: &Path) -> Option<Vec<PathBuf>> {
    let dir = LapinuxPaths::discover().cache_dir.join("previews");
    std::fs::create_dir_all(&dir).ok()?;
    let outputs: Vec<PathBuf> = (0..PREVIEW_FRAMES)
        .map(|index| dir.join(format!("{:016x}-mid-{index}.png", path_id(path))))
        .collect();
    if outputs.iter().all(|out| preview_file_ok(out)) {
        return Some(outputs);
    }
    let duration = video_duration(path).filter(|duration| duration.is_finite() && *duration > 0.0)?;
    let mut frames = Vec::with_capacity(PREVIEW_FRAMES);
    for (index, at) in preview_times(duration).into_iter().enumerate() {
        let out = &outputs[index];
        if !preview_file_ok(out) {
            if !extract_frame(path, at, out) {
                let _ = std::fs::remove_file(out);
                continue;
            }
        }
        frames.push(out.clone());
    }
    (frames.len() >= 2).then_some(frames)
}

fn preview_times(duration: f64) -> Vec<f64> {
    PREVIEW_AT
        .iter()
        .map(|fraction| (duration * fraction).max(0.0))
        .collect()
}

fn preview_file_ok(path: &Path) -> bool {
    path.metadata()
        .map(|meta| meta.is_file() && meta.len() > 0)
        .unwrap_or(false)
}

fn load_preview_textures(
    ctx: &egui::Context,
    video: &Path,
    files: &[PathBuf],
) -> Vec<egui::TextureHandle> {
    let hash = path_id(video);
    files
        .iter()
        .enumerate()
        .filter_map(|(index, file)| {
            let bytes = std::fs::read(file).ok()?;
            let image = image::load_from_memory(&bytes).ok()?;
            Some(texture_from_image(
                ctx,
                image,
                &format!("capture-preview-{hash:016x}-{index}"),
                THUMB_PX,
            ))
        })
        .collect()
}

/// Rédimensionne une image décodée et la confie à `egui`.
fn texture_from_image(
    ctx: &egui::Context,
    image: image::DynamicImage,
    name: &str,
    max_width: u32,
) -> egui::TextureHandle {
    let source = image.width().max(1);
    let width = max_width.min(source);
    let height = ((image.height() as f32) * (width as f32 / source as f32))
        .round()
        .max(1.0) as u32;
    let small = image.thumbnail(width, height).to_rgba8();
    let size = [small.width() as usize, small.height() as usize];
    let pixels = egui::ColorImage::from_rgba_unmultiplied(size, small.as_raw());
    ctx.load_texture(name, pixels, egui::TextureOptions::LINEAR)
}

/// Une capture ouverte en grand : elle prend la place de la grille et de son
/// entête, comme demandé, au lieu d'ouvrir une autre application.
struct Viewer {
    path: PathBuf,
    media: Media,
    title: String,
    taken: i64,
    size: u64,
    /// Image fixe, ou dernière image décodée d'une vidéo.
    frame: Option<egui::TextureHandle>,
    player: Option<Player>,
    error: Option<String>,
    /// Durée de la vidéo, quand `ffprobe` la donne.
    duration: Option<f64>,
}

impl Viewer {
    fn open(ctx: &egui::Context, capture: &Capture) -> Self {
        let mut viewer = Self {
            path: capture.path.clone(),
            media: capture.media,
            title: capture.title(),
            taken: capture.taken,
            size: capture.size,
            frame: None,
            player: None,
            error: None,
            duration: None,
        };
        let name = format!("view-{:016x}", path_id(&capture.path));
        match capture.media {
            Media::Image => match std::fs::read(&capture.path)
                .ok()
                .and_then(|bytes| image::load_from_memory(&bytes).ok())
            {
                Some(image) => {
                    viewer.frame = Some(texture_from_image(ctx, image, &name, VIEW_MAX_W));
                }
                None => viewer.error = Some("image illisible".into()),
            },
            Media::Video => {
                viewer.duration = video_duration(&capture.path);
                viewer.start_playback(ctx);
            }
        }
        viewer
    }

    /// Lance la relecture : `ffmpeg` décode au rythme de la vidéo, chaque
    /// image devient une texture.
    fn start_playback(&mut self, ctx: &egui::Context) {
        if self.media != Media::Video {
            return;
        }
        self.player = None;
        // Un fichier sans piste vidéo (capture ratée) n'a rien à décoder :
        // le dire vaut mieux qu'une zone noire.
        if video_size(&self.path).is_none() {
            self.error = Some(
                "Cette capture ne contient aucune image : le fichier est illisible.".to_string(),
            );
            return;
        }
        match Player::start(&self.path) {
            Some(player) => {
                self.error = None;
                self.player = Some(player);
            }
            None => self.error = Some("lecture impossible (ffmpeg)".into()),
        }
        ctx.request_repaint();
    }

    /// Dessine la visionneuse. Rend `true` quand il faut revenir à la grille.
    fn show(&mut self, ui: &mut egui::Ui) -> bool {
        let ctx = ui.ctx().clone();
        let mut closed = false;

        ui.horizontal(|ui| {
            if ui
                .button(egui::RichText::new("‹  Retour").size(12.0))
                .clicked()
            {
                closed = true;
            }
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(elide(ui, &self.title, &display_font(14.0), 320.0))
                    .size(14.0)
                    .color(TEXT()),
            );
            ui.label(
                egui::RichText::new(format!(
                    "{} · {}",
                    lapinux_core::util::format_datetime(self.taken),
                    lapinux_core::util::format_bytes(self.size)
                ))
                .size(11.0)
                .color(TEXT_DIM()),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(egui::RichText::new("Ouvrir ailleurs").size(11.5)).clicked() {
                    if let Err(err) = open_external(&self.path) {
                        self.error = Some(err);
                    }
                }
                if self.media == Media::Video
                    && ui.button(egui::RichText::new("Recommencer").size(11.5)).clicked()
                {
                    self.start_playback(&ctx);
                }
                if self.media == Media::Video {
                    let (label, paused) = match self.player.as_ref() {
                        Some(player) => {
                            if player.paused {
                                ("Lire", true)
                            } else {
                                ("Pause", false)
                            }
                        }
                        None => ("Lire", false),
                    };
                    let _ = paused;
                    if ui.button(egui::RichText::new(label).size(11.5)).clicked() {
                        match self.player.as_mut() {
                            Some(player) => player.toggle(),
                            None => self.start_playback(&ctx),
                        }
                    }
                }
            });
        });
        ui.add_space(10.0);

        if let Some(err) = self.error.as_ref() {
            ui.label(egui::RichText::new(err).size(12.0).color(TEXT_DIM()));
        }

        // Zone de l'image : tout ce qui reste de la page.
        let area = ui.available_rect_before_wrap();
        let area = egui::Rect::from_min_max(area.min, egui::pos2(area.max.x, area.max.y - 26.0));
        let painter = ui.painter();
        painter.rect_filled(area, egui::Rounding::same(10.0), egui::Color32::from_rgb(10, 11, 13));

        let mut seek_to: Option<f64> = None;
        if let Some(player) = self.player.as_mut() {
            let path = self.path.clone();
            player.pump(&ctx, &path);
            if let Some(texture) = player.texture.as_ref() {
                paint_fit(painter, texture, area);
            }
            // Progression : le temps écoulé et la durée connue de la vidéo.
            let elapsed = player.elapsed();
            let total = self.duration;
            let label = match total {
                Some(total) if total > 0.0 => {
                    format!("{} / {}", clock(elapsed), clock(total))
                }
                _ => clock(elapsed),
            };
            painter.text(
                egui::pos2(area.left() + 12.0, area.bottom() - 4.0),
                egui::Align2::LEFT_CENTER,
                label,
                egui::FontId::proportional(11.0),
                TEXT_DIM(),
            );
            // Barre de lecture : la piste se clique et se fait glisser, et le
            // décodeur repart de l'instant visé.
            if let Some(total) = total.filter(|total| *total > 0.0) {
                let bar = egui::Rect::from_min_max(
                    egui::pos2(area.left() + 12.0, area.bottom() - 32.0),
                    egui::pos2(area.right() - 12.0, area.bottom() - 24.0),
                );
                let response = ui.interact(
                    bar.expand(5.0),
                    ui.id().with(("seek", &self.path)),
                    egui::Sense::click_and_drag(),
                );
                painter.rect_filled(bar, egui::Rounding::same(3.0), CARD_HOVER());
                let fraction = (elapsed / total).clamp(0.0, 1.0) as f32;
                let filled = egui::Rect::from_min_max(
                    bar.min,
                    egui::pos2(bar.left() + bar.width() * fraction, bar.max.y),
                );
                painter.rect_filled(filled, egui::Rounding::same(3.0), TEXT());
                painter.circle_filled(
                    egui::pos2(bar.left() + bar.width() * fraction, bar.center().y),
                    4.0,
                    TEXT(),
                );
                if response.hovered() || response.dragged() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                if response.clicked() || response.dragged() {
                    if let Some(at) = response.interact_pointer_pos() {
                        let ratio = f64::from(((at.x - bar.left()) / bar.width()).clamp(0.0, 1.0));
                        seek_to = Some(ratio * total);
                    }
                }
            }
        } else if let Some(texture) = self.frame.as_ref() {
            paint_fit(painter, texture, area);
            if self.media == Media::Video {
                // Image d'attente : le bouton de lecture du bandeau relance.
                paint_play_glyph(painter, area.center(), TEXT());
            }
        }

        if let Some(target) = seek_to {
            self.seek(&ctx, target);
        }

        ctx.request_repaint_after(Duration::from_millis(if self.player.is_some() {
            16
        } else {
            400
        }));
        closed
    }

    /// Reprend la lecture à un instant donné : `ffmpeg` ne sait pas se
    /// déplacer dans un flux lu à la volée, le décodeur et le son repartent
    /// donc de là.
    fn seek(&mut self, ctx: &egui::Context, seconds: f64) {
        if self.media != Media::Video {
            return;
        }
        let Some(total) = self.duration.filter(|total| *total > 0.0) else {
            return;
        };
        let at = seconds.clamp(0.0, (total - 0.05).max(0.0));
        match Player::start_at(&self.path, at) {
            Some(player) => {
                self.error = None;
                self.player = Some(player);
            }
            None => self.error = Some("lecture impossible (ffmpeg)".into()),
        }
        ctx.request_repaint();
    }
}

/// Peint une texture dans une zone en gardant ses proportions.
fn paint_fit(painter: &egui::Painter, texture: &egui::TextureHandle, area: egui::Rect) {
    let size = texture.size_vec2();
    if size.x <= 0.0 || size.y <= 0.0 {
        return;
    }
    let scale = (area.width() / size.x).min(area.height() / size.y);
    let target = egui::Rect::from_center_size(area.center(), size * scale);
    painter.image(texture.id(), target, uv_full(), egui::Color32::WHITE);
}

/// `mm:ss`, ou `h:mm:ss` au-delà de l'heure.
fn clock(secs: f64) -> String {
    let total = secs.max(0.0) as u64;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Relecture d'une vidéo : `ffmpeg` décode au rythme réel et le fil de lecture
/// pose chaque image dans une texture.
struct Player {
    child: Child,
    /// Piste audio : le décodeur et le lecteur qui consomme son tube.
    audio: Option<Audio>,
    frames: Receiver<Playback>,
    size: (u32, u32),
    texture: Option<egui::TextureHandle>,
    /// Position de départ du décodage, en secondes : un saut relance `ffmpeg`
    /// depuis ce point.
    offset: f64,
    paused: bool,
    started: Instant,
    /// Temps passé en pause, retiré du temps écoulé affiché.
    paused_total: Duration,
    paused_at: Option<Instant>,
    ended: bool,
}

enum Playback {
    Frame(Vec<u8>),
    End,
}

/// Chaîne audio de la relecture : `ffmpeg` décode la piste en PCM, `pacat` la
/// joue. `pacat` ouvre la sortie avant les premiers échantillons, donc le son
/// démarre avec l'image au lieu de plusieurs centaines de millisecondes après
/// elle — c'était le décalage de la visionneuse.
struct Audio {
    player: Child,
    /// Décodeur alimentant le lecteur ; absent du repli `ffplay`.
    decoder: Option<Child>,
}

impl Player {
    fn start(path: &Path) -> Option<Self> {
        Self::start_at(path, 0.0)
    }

    /// Reprend la lecture à partir de `offset` secondes. `ffmpeg` ne sait pas
    /// sauter sans repartir : le décodeur est relancé à cet instant, et le son
    /// avec lui.
    fn start_at(path: &Path, offset: f64) -> Option<Self> {
        let ffmpeg = lapinux_core::paths::which("ffmpeg")?;
        let (source_w, source_h) = video_size(path)?;
        let width = source_w.min(VIEW_MAX_W).max(2) & !1;
        let height = ((source_h as f32) * (width as f32 / source_w as f32)).round().max(2.0) as u32;
        let mut command = Command::new(ffmpeg);
        command.args(["-hide_banner", "-loglevel", "error"]);
        if offset > 0.0 {
            command.arg("-ss").arg(format!("{offset:.3}"));
        }
        // La vidéo part en premier. `ffmpeg` met environ 200 ms à rendre sa
        // première image, tandis que `pacat` ouvre sa sortie en 130 ms :
        // lancer le son avant la vidéo le faisait entendre ~130 ms trop tôt.
        let mut child = command
            .arg("-re")
            .arg("-i")
            .arg(path)
            .args(["-an", "-vf", &format!("scale={width}:{height}")])
            .args(["-f", "rawvideo", "-pix_fmt", "rgba", "-"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let mut stdout = child.stdout.take()?;
        let (tx, frames) = mpsc::channel();
        let frame_len = width as usize * height as usize * 4;
        std::thread::spawn(move || {
            loop {
                let mut buffer = vec![0u8; frame_len];
                if stdout.read_exact(&mut buffer).is_err() {
                    break;
                }
                if tx.send(Playback::Frame(buffer)).is_err() {
                    return;
                }
            }
            let _ = tx.send(Playback::End);
        });
        // 60 ms est l'écart mesuré entre les deux démarrages (200 - 130 ms).
        // Le son est créé après la vidéo, mais avant que sa première image
        // n'ait eu le temps d'être affichée par `pump`.
        std::thread::sleep(Duration::from_millis(60));
        let audio = start_audio(path, offset);
        Some(Self {
            child,
            audio,
            frames,
            size: (width, height),
            texture: None,
            offset,
            paused: false,
            started: Instant::now(),
            paused_total: Duration::ZERO,
            paused_at: None,
            ended: false,
        })
    }

    fn toggle(&mut self) {
        if self.ended {
            return;
        }
        if self.paused {
            self.send_signal("-CONT");
            self.paused = false;
            if let Some(at) = self.paused_at.take() {
                self.paused_total += at.elapsed();
            }
        } else {
            self.send_signal("-STOP");
            self.paused = true;
            self.paused_at = Some(Instant::now());
        }
    }

    /// `kill` du système : inutile d'ajouter une dépendance pour deux signaux.
    /// Le son suit l'image : mettre l'image en pause sans la piste laisserait
    /// le son continuer seul.
    fn send_signal(&self, signal: &str) {
        signal_pid(self.child.id(), signal);
        if let Some(audio) = self.audio.as_ref() {
            signal_pid(audio.player.id(), signal);
            if let Some(decoder) = audio.decoder.as_ref() {
                signal_pid(decoder.id(), signal);
            }
        }
    }

    fn elapsed(&self) -> f64 {
        let paused = self
            .paused_at
            .map(|at| at.elapsed())
            .unwrap_or(Duration::ZERO);
        self.offset
            + (self.started.elapsed().saturating_sub(self.paused_total + paused)).as_secs_f64()
    }

    /// Vide les images arrivées depuis la dernière frame et garde la dernière.
    fn pump(&mut self, ctx: &egui::Context, path: &Path) {
        let mut latest: Option<Vec<u8>> = None;
        loop {
            match self.frames.try_recv() {
                Ok(Playback::Frame(buffer)) => latest = Some(buffer),
                Ok(Playback::End) => {
                    self.ended = true;
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.ended = true;
                    break;
                }
            }
        }
        if let Some(buffer) = latest {
            let size = [self.size.0 as usize, self.size.1 as usize];
            let pixels = egui::ColorImage::from_rgba_unmultiplied(size, &buffer);
            let name = format!("frame-{:016x}", path_id(path));
            self.texture = Some(ctx.load_texture(name, pixels, egui::TextureOptions::LINEAR));
        }
    }
}

/// Envoie un signal à un processus suivi par son pid.
fn signal_pid(pid: u32, signal: &str) {
    let _ = Command::new("kill")
        .arg(signal)
        .arg(pid.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(mut audio) = self.audio.take() {
            let _ = audio.player.kill();
            let _ = audio.player.wait();
            if let Some(mut decoder) = audio.decoder.take() {
                let _ = decoder.kill();
                let _ = decoder.wait();
            }
        }
    }
}

/// Ouvre la piste audio de la relecture. Le décodeur PCM et `pacat` sont deux
/// processus, mais le lecteur ouvre la sortie avant que les échantillons
/// n'arrivent : contrairement à `ffplay`, son démarrage n'ajoute pas de
/// silence au début.
fn start_audio(path: &Path, offset: f64) -> Option<Audio> {
    let ffmpeg = lapinux_core::paths::which("ffmpeg")?;
    if let Some(pacat) = lapinux_core::paths::which("pacat") {
        let mut decode = Command::new(&ffmpeg);
        decode.args(["-hide_banner", "-loglevel", "error"]);
        if offset > 0.0 {
            decode.arg("-ss").arg(format!("{offset:.3}"));
        }
        let mut decoder = decode
            .arg("-i")
            .arg(path)
            .args(["-vn", "-f", "s16le", "-ar", "48000", "-ac", "2", "-"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let pcm = decoder.stdout.take()?;
        let player = Command::new(pacat)
            .args([
                "--playback",
                "--raw",
                "--format=s16le",
                "--rate=48000",
                "--channels=2",
                "--latency-msec=40",
            ])
            .stdin(Stdio::from(pcm))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        return Some(Audio {
            player,
            decoder: Some(decoder),
        });
    }
    // Repli sans `pacat` : `ffplay` seul, avec son démarrage plus lent.
    let ffplay = lapinux_core::paths::which("ffplay")?;
    let mut command = Command::new(ffplay);
    command.args(["-nodisp", "-autoexit", "-loglevel", "quiet"]);
    if offset > 0.0 {
        command.arg("-ss").arg(format!("{offset:.3}"));
    }
    let player = command
        .arg("-i")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    Some(Audio {
        player,
        decoder: None,
    })
}

/// Dimensions de l'image d'une vidéo, lues par `ffprobe`.
fn video_size(path: &Path) -> Option<(u32, u32)> {
    let ffprobe = lapinux_core::paths::which("ffprobe")?;
    let output = Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-of",
            "csv=p=0:s=x",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let (width, height) = text.trim().split_once('x')?;
    Some((width.parse().ok()?, height.parse().ok()?))
}

/// Durée d'une vidéo en secondes, quand `ffprobe` la connaît.
fn video_duration(path: &Path) -> Option<f64> {
    let ffprobe = lapinux_core::paths::which("ffprobe")?;
    let output = Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "csv=p=0",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

fn uv_full() -> egui::Rect {
    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0))
}

fn modified_unix(path: &Path) -> Option<i64> {
    std::fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|age| age.as_secs() as i64)
}

fn path_id(path: &Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capture(name: &str) -> Option<Capture> {
        Capture::from_path(Path::new(name))
    }

    #[test]
    fn a_capture_keeps_its_game_its_appid_and_its_time() {
        let parsed = capture("/tmp/Images/lapinux-440-team-fortress-2-1793000000.png").unwrap();
        assert_eq!(parsed.appid, Some(440));
        assert_eq!(parsed.game, "Team Fortress 2");
        assert_eq!(parsed.taken, 1_793_000_000);
        assert_eq!(parsed.media, Media::Image);
    }

    #[test]
    fn an_older_capture_only_has_its_time() {
        let parsed = capture("/tmp/Images/lapinux-1793000000.png").unwrap();
        assert_eq!(parsed.appid, None);
        assert_eq!(parsed.game, "");
        assert_eq!(parsed.taken, 1_793_000_000);
        assert_eq!(parsed.title(), "Hors partie");
    }

    #[test]
    fn a_renamed_video_is_read_in_a_marked_folder() {
        let root = std::env::temp_dir().join(format!("lapinux-new-captures-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join(lapinux_core::captures::MARKER), b"").unwrap();
        std::fs::write(
            root.join("2026-11-02 13-14-48 - MotorSlice - 30 FPS.mp4"),
            b"x",
        )
        .unwrap();
        let entries = scan_dir(&root);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].game, "MotorSlice");
        assert_eq!(entries[0].media, Media::Video);
        assert!(entries[0].taken > 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_video_is_recognised_and_a_foreign_file_is_ignored() {
        assert_eq!(
            capture("lapinux-440-elden-ring-1793000000.mp4")
                .unwrap()
                .media,
            Media::Video
        );
        assert!(capture("IMG_20240101.png").is_none());
        assert!(capture("lapinux-440-elden-ring.png").is_none());
    }

    #[test]
    fn the_filter_splits_images_from_videos() {
        assert!(Kind::All.keeps(Media::Image));
        assert!(Kind::All.keeps(Media::Video));
        assert!(Kind::Image.keeps(Media::Image));
        assert!(!Kind::Image.keeps(Media::Video));
        assert!(Kind::Video.keeps(Media::Video));
        assert!(!Kind::Video.keeps(Media::Image));
    }

    #[test]
    fn the_folder_lists_only_lapinux_captures_newest_first() {
        let root = std::env::temp_dir().join(format!("lapinux-captures-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("lapinux-440-dota-2-1793000000.png"), b"x").unwrap();
        std::fs::write(root.join("lapinux-1793000500.png"), b"x").unwrap();
        std::fs::write(root.join("photo.png"), b"x").unwrap();
        let entries = scan_dir(&root);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].taken, 1_793_000_500);
        assert_eq!(entries[1].game, "Dota 2");
        // Le poids vient du disque, pas du nom.
        assert_eq!(entries[0].size, 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_thumbnail_is_decoded_before_reaching_the_drawing_thread() {
        let root = std::env::temp_dir().join(format!("lapinux-thumb-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("lapinux-440-dota-2-1793000000.png");
        let pixels = image::RgbaImage::from_pixel(64, 36, image::Rgba([20, 30, 40, 255]));
        pixels.save(&path).unwrap();
        let parsed = Capture::from_path(&path).unwrap();
        let thumb = load_thumb(&parsed);
        assert!(thumb.image.is_some());
        assert_eq!(thumb.duration, None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_clock_reads_like_a_player() {
        assert_eq!(clock(0.0), "0:00");
        assert_eq!(clock(9.4), "0:09");
        assert_eq!(clock(61.0), "1:01");
        assert_eq!(clock(3_723.0), "1:02:03");
        assert_eq!(clock(-5.0), "0:00");
    }

    #[test]
    fn the_actions_sit_in_the_bottom_corner_of_the_thumbnail() {
        let card = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(CARD_W, CARD_H));
        let trash = trash_rect(card);
        let folder = folder_rect(card);
        assert!(trash.width() == TRASH && trash.height() == TRASH);
        assert!(card.contains_rect(trash));
        assert!(card.contains_rect(folder));
        assert!(trash.left() > folder.right());
        assert!(trash.right() < card.right());
    }

    #[test]
    fn the_five_video_previews_are_centered_on_the_file() {
        let times = preview_times(100.0);
        assert_eq!(times.len(), PREVIEW_FRAMES);
        assert!((times[0] - 25.0).abs() < 0.001);
        assert!((times[2] - 50.0).abs() < 0.001);
        assert!((times[times.len() - 1] - 75.0).abs() < 0.001);
        assert!(times.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn a_video_badge_makes_room_for_its_duration() {
        let thumb = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(CARD_W, THUMB_H));
        let image = media_badge_rect(thumb, Media::Image, None);
        let video = media_badge_rect(thumb, Media::Video, Some(24.0));
        assert!(video.width() > image.width());
        assert_eq!(video.width(), 57.0);
        assert!(thumb.contains_rect(video));
    }

    #[test]
    fn a_capture_game_matches_its_steam_title_without_punctuation() {
        assert!(same_game_name("A Short Hike", "A Short Hike™"));
        assert!(same_game_name("MOTORSLICE", "MotorSlice Demo"));
        assert!(!same_game_name("Melatonin", "MOTORSLICE"));
    }
}
