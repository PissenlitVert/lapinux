//! Section « Détails Steam » : tout ce que Steam publie d'un jeu pour ce compte.
//!
//! Le daemon assemble la fiche en trois lectures séparées — le **protocole**
//! (PICS : build public, branches, dépôts et leur poids, classifications,
//! sauvegardes cloud, contenus additionnels), le **compte** par la session
//! Steam (temps de jeu total, deux semaines et par système, dernière partie,
//! niveau, badge, succès et leur rareté mondiale, statistiques publiées) et la
//! **boutique** (description, captures, exigences, prix, avis) et le nombre
//! de joueurs connectés, seule valeur demandée en HTTP : elle est publique.
//!
//! L'interface ne demande la fiche qu'une fois par ouverture de jeu, la garde
//! tant que le jeu reste affiché et la relit au geste « Relire ». Rien ne
//! bloque la peinture : la lecture vit en tâche de fond et la section montre ce
//! qu'elle a dès l'arrivée de la réponse.

use super::*;
use lapinux_core::model::{
    DescriptionBlock, SteamAccountPlay, SteamFeature, SteamGameInfo, SteamNews, SteamStat,
    SteamStorePage,
};
#[cfg(test)]
use lapinux_core::model::{SteamBranch, SteamDepot, SteamUpdateInfo};

/// Budget d'une lecture complète : PICS, compte et boutique s'enchaînent, et
/// PICS répond parfois lentement. Au-delà, la section abandonne et propose de
/// relancer plutôt que de faire tourner une roue indéfiniment.
const LOAD_BUDGET: Duration = Duration::from_secs(90);
/// Délai avant de redemander une fiche qui vient d'échouer. Sans lui, l'erreur
/// relançait une lecture à chaque image, et l'interface harcelait le daemon
/// alors que Steam venait déjà de refuser.
const RETRY_AFTER_ERROR: Duration = Duration::from_secs(10);
const TIMEOUT_ERROR: &str = "Steam n'a pas répondu : les détails du jeu n'ont pas pu être lus.";
/// Succès les plus rares listés avec leur pourcentage mondial.
const RARE_ROWS: usize = 6;
/// Longueur maximale d'un texte replié (conditions, configuration) : la
/// description, elle, se peint entière.
const TEXT_LIMIT: usize = 4000;
/// Écart entre deux cartes d'actualité.
const NEWS_GAP: f32 = 8.0;
/// Largeur en dessous de laquelle les cartes s'empilent au lieu de tenir sur
/// une ligne.
const NEWS_CARD_MIN: f32 = 190.0;
/// Âge au-delà duquel la fiche servie par le daemon est reprise en silence.
const STALE_SECONDS: i64 = 120;
/// Délai laissé à la relecture du daemon avant de redemander la fiche.
const REFRESH_AFTER: Duration = Duration::from_secs(8);
/// Toutes les annonces d'un jeu : la carte n'en montre que trois, le panneau
/// les lit toutes.
#[derive(Clone, Debug)]
pub(crate) struct NewsFeed {
    pub items: Vec<SteamNews>,
    /// Annonce choisie dans l'encart : le lecteur s'ouvre directement dessus.
    pub at: usize,
}

/// Fiche lue, gardée le temps que le jeu reste affiché.
#[derive(Default)]
pub(crate) struct Shelf {
    /// Annonces à lire dans le panneau.
    news_open: Option<NewsFeed>,
    /// Bande-annonce demandée, avec le nom du jeu : l'application ouvre son
    /// lecteur intégré.
    trailer_want: Option<(String, String)>,
    /// Jeu dont la fiche est demandée : redemander le même serait un aller
    /// retour réseau pour rien, changer de jeu relance la lecture.
    loaded_for: Option<u32>,
    data: Option<SteamGameInfo>,
    error: Option<String>,
    loading: bool,
    /// Début de la lecture en cours, pour le budget d'attente.
    started: Option<Instant>,
    /// Fin de la dernière lecture ratée : elle impose une pause avant de
    /// redemander.
    failed_at: Option<Instant>,
    rx: Option<mpsc::Receiver<Result<SteamGameInfo, String>>>,
    /// Instant de la relecture silencieuse : le daemon a servi sa copie et
    /// relit la fiche derrière, l'écran la reprend sans clignoter.
    refresh_at: Option<Instant>,
    /// Relectures silencieuses déjà demandées pour ce jeu.
    refresh_tries: u8,
    /// Visuels des annonces, gardés d'un jeu à l'autre.
    pub(crate) news_art: NewsArt,
    /// Visuels ouverts en grand : un clic sur une capture l'affiche dans le
    /// panneau fixe, au lieu d'ouvrir le navigateur. La galerie retient les
    /// images de la même série, pour passer de l'une à l'autre.
    photo: Option<Gallery>,
}

/// Une image de la fiche ouverte en grand : son adresse et ce qu'elle montre.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Photo {
    pub url: String,
    pub caption: String,
}

impl Photo {
    fn new(url: &str, caption: &str) -> Self {
        Self {
            url: url.to_string(),
            caption: caption.to_string(),
        }
    }
}

/// Titre d'encart, centré sur la largeur de la carte.
///
/// Le décalage se calcule sur la largeur connue de la carte (`inner`) : dans
/// une ligne horizontale, `available_width` vaudrait toute la fenêtre et le
/// titre partirait au milieu de la fiche au lieu de son encart.
pub(crate) fn centered_title(ui: &mut egui::Ui, inner: f32, text: &str, size: f32) {
    let width = crate::text_size(ui, text, size).x;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.add_space(((inner - width) / 2.0).max(0.0));
        ui.label(egui::RichText::new(text).size(size).strong());
    });
}

/// Une description, centrée dans la colonne de la fiche.
///
/// Chaque ligne est repliée puis peinte à partir de son propre centre : le
/// `LayoutJob` centré plaçait le bloc entier, mais ses lignes restaient
/// alignées à gauche et débordaient dans la colonne des actualités. Les titres
/// Markdown de la vitrine Epic (`#`, `##`…) sont peints comme des titres.
fn centered_description_text(ui: &mut egui::Ui, inner: f32, text: &str) {
    let mut painted = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some((level, heading)) = markdown_heading(line) {
            if painted {
                ui.add_space(8.0);
            }
            centered_description_paragraph(ui, inner, heading, heading_size(level));
            ui.add_space(3.0);
        } else {
            centered_description_paragraph(ui, inner, line, 11.0);
        }
        painted = true;
    }
}

/// Titre Markdown : les dièses ouvrent le titre, ils ne doivent pas rester
/// dans le texte. Un dièse au milieu d'une phrase n'est pas un titre.
fn markdown_heading(line: &str) -> Option<(usize, &str)> {
    let line = line.trim();
    let level = line.bytes().take_while(|byte| *byte == b'#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let rest = line.get(level..)?;
    if !rest.starts_with(' ') && !rest.starts_with('\t') {
        return None;
    }
    let title = rest.trim().trim_end_matches('#').trim();
    (!title.is_empty()).then_some((level, title))
}

/// Taille d'un titre Markdown : le corps reste à 11 points, les titres
/// prennent la place qui les distingue sans casser la colonne de lecture.
fn heading_size(level: usize) -> f32 {
    match level {
        1 => 17.0,
        2 => 15.5,
        3 => 14.0,
        _ => 12.5,
    }
}

/// Un paragraphe (ou un titre) replié puis peint depuis son propre centre.
fn centered_description_paragraph(ui: &mut egui::Ui, inner: f32, text: &str, size: f32) {
    let text = text.trim();
    let font = egui::FontId::proportional(size);
    let mut lines: Vec<String> = Vec::new();
    for paragraph in text.lines() {
        let words: Vec<&str> = paragraph.split_whitespace().collect();
        if words.is_empty() {
            continue;
        }
        let mut line = String::new();
        for word in words {
            let candidate = if line.is_empty() {
                word.to_string()
            } else {
                format!("{line} {word}")
            };
            let width = ui.fonts(|fonts| {
                fonts
                    .layout_no_wrap(candidate.clone(), font.clone(), TEXT())
                    .size()
                    .x
            });
            if line.is_empty() || width <= inner {
                line = candidate;
            } else {
                lines.push(std::mem::take(&mut line));
                line = word.to_string();
            }
        }
        if !line.is_empty() {
            lines.push(line);
        }
    }
    if lines.is_empty() {
        return;
    }
    let line_height = ui
        .fonts(|fonts| fonts.layout_no_wrap("X".into(), font.clone(), TEXT()).size().y)
        .max(1.0);
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(inner, line_height * lines.len() as f32),
        egui::Sense::hover(),
    );
    for (index, line) in lines.into_iter().enumerate() {
        let galley = ui.fonts(|fonts| fonts.layout_no_wrap(line, font.clone(), TEXT()));
        let left = rect.center().x - galley.size().x / 2.0;
        ui.painter().galley(
            egui::pos2(left, rect.top() + line_height * index as f32),
            galley,
            TEXT(),
        );
    }
}

/// Largeur de lecture confortable de la description, même quand la fiche est
/// assez large pour en montrer davantage.
const DESCRIPTION_TEXT_WIDTH: f32 = 860.0;

/// La série des captures d'un jeu, dans l'ordre où la fiche les montre.
fn captures(urls: &[String]) -> Vec<Photo> {
    urls.iter()
        .map(|url| Photo::new(url, "Capture du jeu"))
        .collect()
}

/// Les images d'une même série — les captures d'un jeu, les visuels de sa
/// description, ou la bannière seule — et celle qui est affichée.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Gallery {
    pub photos: Vec<Photo>,
    pub index: usize,
}

/// Ce qu'une fiche montre, d'où qu'elle vienne : Steam la remplit depuis sa
/// section de détails, une boutique depuis sa propre API. Les encarts ne lisent
/// que ce paquet, donc la mise en page est la même partout — mêmes encarts,
/// mêmes places, même comportement.
#[derive(Clone, Default)]
pub(crate) struct SheetContent {
    /// Nom du jeu, pour le lecteur de bande-annonce.
    pub name: String,
    pub short_description: String,
    pub description: String,
    /// La description dans l'ordre où la boutique publie texte et visuels.
    pub description_blocks: Vec<DescriptionBlock>,
    pub header_image: String,
    pub screenshots: Vec<String>,
    /// Visuels intercalés dans la description.
    pub description_images: Vec<String>,
    /// Configurations et mentions repliées : titre, puis texte.
    pub requirements: Vec<(String, String)>,
    pub news: Vec<SteamNews>,
    /// Les annonces ont été reprises de Steam, faute d'annonces de la boutique.
    pub news_from_steam: bool,
    /// Bande-annonce et son affiche, quand la boutique en publie une.
    pub trailer: String,
    pub trailer_poster: String,
}

impl SheetContent {
    /// La fiche Steam telle que les encarts la lisent.
    fn from_steam(store: &SteamStorePage, name: &str) -> Self {
        let mut requirements: Vec<(String, String)> = Vec::new();
        for (title, text) in [
            ("Configuration minimale", &store.requirements_min),
            ("Configuration recommandée", &store.requirements_recommended),
            ("Mentions légales", &store.legal_notice),
            ("DRM", &store.drm_notice),
        ] {
            let text = text.trim();
            if !text.is_empty() {
                requirements.push((title.to_string(), text.to_string()));
            }
        }
        Self {
            name: name.to_string(),
            short_description: store.short_description.clone(),
            description: store.description.clone(),
            description_blocks: store.description_blocks.clone(),
            header_image: store.header_image.clone(),
            screenshots: store.screenshots.clone(),
            description_images: store.description_images.clone(),
            requirements,
            news: store.news.clone(),
            news_from_steam: false,
            trailer: store.trailer.clone(),
            trailer_poster: store.trailer_poster.clone(),
        }
    }

    /// La fiche d'un jeu de boutique : la carte Résumé reçoit l'accroche
    /// publiée par la boutique, et « À propos » garde la présentation longue
    /// entière, introduction comprise.
    pub(crate) fn from_store_details(details: Option<&lapinux_core::stores::StoreDetails>) -> Self {
        let Some(details) = details else {
            return Self::default();
        };
        let description = details.description.trim().to_string();
        let summary = details.summary.trim().to_string();
        let short_description = if summary.is_empty() {
            // Ancienne copie de la fiche sans accroche séparée : la première
            // ligne reste un pis-aller, sans toucher au texte long.
            description
                .lines()
                .next()
                .unwrap_or_default()
                .trim()
                .to_string()
        } else {
            summary
        };
        Self {
            name: String::new(),
            short_description,
            description,
            description_blocks: Vec::new(),
            header_image: details.banner.clone(),
            screenshots: details.screenshots.clone(),
            description_images: Vec::new(),
            // Une boutique ne publie ni configuration ni mentions : la fiche
            // garde les mêmes encarts, sans ces replis-là.
            requirements: Vec::new(),
            news: details.news.iter().map(|news| news.as_steam()).collect(),
            news_from_steam: details.news_from_steam,
            trailer: details.trailer.clone(),
            trailer_poster: details.trailer_poster.clone(),
        }
    }

}

impl Shelf {
    /// Annonces à ouvrir dans le lecteur, avec celle qui a été cliquée.
    pub(crate) fn take_news(&mut self) -> Option<NewsFeed> {
        self.news_open.take()
    }

    /// Bande-annonce cliquée, à confier à la fenêtre vidéo.
    pub(crate) fn take_trailer(&mut self) -> Option<(String, String)> {
        self.trailer_want.take()
    }

    /// Fiche gardée du jeu affiché, si elle est arrivée.
    pub fn info(&self) -> Option<&SteamGameInfo> {
        self.data.as_ref()
    }

    /// Le contenu de la fiche, sous la forme que partagent tous les encarts.
    fn content(&self, appid: u32) -> Option<SheetContent> {
        self.data
            .as_ref()
            .filter(|info| info.appid == appid)
            .map(|info| SheetContent::from_steam(&info.store, &info.name))
    }

    /// La fiche est-elle en train d'arriver ? C'est ce qui met le lapin dans
    /// les encarts vides.
    fn waiting_for(&self, appid: u32) -> bool {
        // Une relecture de fond ne remet pas le lapin sur une fiche déjà
        // remplie : elle continue de s'afficher telle quelle.
        self.loaded_for == Some(appid) && self.loading && self.data.is_none()
    }

    /// Oublie la fiche gardée : la prochaine ouverture du jeu la redemandera.
    pub fn forget(&mut self) {
        self.loaded_for = None;
        self.data = None;
        self.error = None;
        self.loading = false;
        self.started = None;
        self.failed_at = None;
        self.rx = None;
        self.photo = None;
        self.refresh_at = None;
        self.refresh_tries = 0;
    }

    /// Ouvre une image dans le panneau fixe : c'est la fiche qui garde ce qui
    /// est affiché, et la série d'images d'où elle vient.
    fn show_photo(&mut self, photos: Vec<Photo>, index: usize) {
        if photos.is_empty() {
            return;
        }
        self.photo = Some(Gallery {
            index: index.min(photos.len() - 1),
            photos,
        });
    }

    /// Visionneuse de captures : la même surface que la bande-annonce, un fond
    /// noir pur, l'image entière et les autres images en vignettes dessous.
    pub fn photo_panel(&mut self, ctx: &egui::Context, screen: egui::Rect) {
        let Some(gallery) = self.photo.clone() else {
            return;
        };
        let total = gallery.photos.len().max(1);
        // Les flèches du clavier sont consommées ici, même quand l'image est
        // seule : derrière la visionneuse, la bibliothèque ne doit pas changer
        // de jeu dans le même geste.
        let mut keyboard_step = 0i32;
        ctx.input_mut(|input| {
            let left = input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft);
            let right = input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight);
            if total > 1 {
                keyboard_step -= i32::from(left);
                keyboard_step += i32::from(right);
            }
        });
        let mut close = ctx.input_mut(|input| {
            input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)
        });
        let index = (gallery.index.min(total - 1) as i32 + keyboard_step)
            .rem_euclid(total as i32) as usize;
        let photo = gallery.photos[index].clone();
        let urls: Vec<String> = gallery
            .photos
            .iter()
            .map(|photo| photo.url.clone())
            .collect();
        self.news_art.ensure(ctx, &urls);
        let textures: Vec<Option<egui::TextureHandle>> = gallery
            .photos
            .iter()
            .map(|photo| self.news_art.get(&photo.url).cloned())
            .collect();
        let texture = textures[index].clone();
        let subtitle = if total > 1 {
            format!("{} · {} / {}", photo.caption, index + 1, total)
        } else {
            photo.caption.clone()
        };
        let header_height = 64.0;
        let footer_height = if total > 1 { 88.0 } else { 24.0 };
        let side = 24.0;
        let image_area = egui::Rect::from_min_max(
            egui::pos2(screen.left() + side, screen.top() + header_height),
            egui::pos2(
                screen.right() - side,
                (screen.bottom() - footer_height).max(screen.top() + header_height + 80.0),
            ),
        );
        let strip_rect = egui::Rect::from_min_max(
            egui::pos2(screen.left() + side, image_area.bottom()),
            egui::pos2(screen.right() - side, screen.bottom() - 12.0),
        );
        let top_rect = egui::Rect::from_min_max(
            egui::pos2(screen.left() + side, screen.top() + 12.0),
            egui::pos2(screen.right() - side, screen.top() + 56.0),
        );
        let mut click_step = 0i32;
        let mut chosen = None;
        egui::Area::new(egui::Id::new("photo-cinema"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                ui.painter().rect_filled(screen, 0.0, egui::Color32::BLACK);
                // La surface opaque absorbe les clics : sans elle, un clic
                // dans le noir atteindrait la fiche ou la bibliothèque
                // derrière la visionneuse.
                let _surface = ui.interact(
                    screen,
                    ui.id().with("photo-surface"),
                    egui::Sense::click(),
                );

                ui.scope_builder(egui::UiBuilder::new().max_rect(top_rect), |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        let text_width = (top_rect.width() - 34.0).max(120.0);
                        ui.allocate_ui_with_layout(
                            egui::vec2(text_width, top_rect.height()),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                ui.label(
                                    egui::RichText::new("Capture")
                                        .size(18.0)
                                        .strong()
                                        .color(TEXT()),
                                );
                                ui.add_space(1.0);
                                ui.label(
                                    egui::RichText::new(&subtitle)
                                        .size(11.0)
                                        .color(TEXT_DIM()),
                                );
                            },
                        );
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                let button = egui::Button::new(
                                    egui::RichText::new("×").size(20.0).color(TEXT_DIM()),
                                )
                                .frame(false)
                                .min_size(egui::vec2(30.0, 30.0));
                                if ui
                                    .add(button)
                                    .on_hover_text("Fermer (Échap)")
                                    .clicked()
                                {
                                    close = true;
                                }
                            },
                        );
                    });
                });

                // L'image entière, jamais rognée : les bandes qui restent
                // autour d'elle sont du noir pur, comme dans le lecteur.
                let natural = texture
                    .as_ref()
                    .map(|texture| texture.size_vec2())
                    .unwrap_or(egui::vec2(16.0, 9.0));
                let shown = fit_rect(image_area, natural);
                match &texture {
                    Some(texture) => paint_fit(ui, image_area, texture),
                    None => {
                        ui.painter().rect_filled(shown, 0.0, CARD_HOVER());
                    }
                }

                if total > 1 {
                    for (_side, delta) in [(-1.0_f32, -1i32), (1.0, 1)] {
                        let center = egui::pos2(
                            if delta < 0 {
                                shown.left() + 24.0
                            } else {
                                shown.right() - 24.0
                            },
                            shown.center().y,
                        );
                        let hit = egui::Rect::from_center_size(center, egui::vec2(36.0, 36.0));
                        let response = ui.interact(
                            hit,
                            ui.id().with(("photo-arrow", delta)),
                            egui::Sense::click(),
                        );
                        ui.painter().circle_filled(
                            center,
                            18.0,
                            egui::Color32::from_black_alpha(if response.hovered() {
                                210
                            } else {
                                150
                            }),
                        );
                        let tip = 6.0;
                        let points = if delta < 0 {
                            vec![
                                center + egui::vec2(-tip, 0.0),
                                center + egui::vec2(tip * 0.7, -tip),
                                center + egui::vec2(tip * 0.7, tip),
                            ]
                        } else {
                            vec![
                                center + egui::vec2(tip, 0.0),
                                center + egui::vec2(-tip * 0.7, -tip),
                                center + egui::vec2(-tip * 0.7, tip),
                            ]
                        };
                        ui.painter().add(egui::Shape::convex_polygon(
                            points,
                            egui::Color32::from_rgb(248, 250, 252),
                            egui::Stroke::NONE,
                        ));
                        if response.hovered() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        let response = response.on_hover_text(if delta < 0 {
                            "Image précédente (←)"
                        } else {
                            "Image suivante (→)"
                        });
                        if response.clicked() {
                            click_step += delta;
                        }
                    }
                }

                if total > 1 {
                    // Toutes les images de la série restent sous l'image
                    // ouverte : un clic sur une vignette la remplace.
                    let gap = 8.0;
                    let count = total as f32;
                    let thumb_w = ((strip_rect.width() - 8.0 - gap * (count - 1.0)) / count)
                        .clamp(40.0, 96.0);
                    let thumb_h = thumb_w * 9.0 / 16.0;
                    let strip_width = thumb_w * count + gap * (count - 1.0);
                    let left = strip_rect.center().x - strip_width / 2.0;
                    let top = strip_rect.center().y - thumb_h / 2.0;
                    for (slot, texture) in textures.iter().enumerate() {
                        let rect = egui::Rect::from_min_size(
                            egui::pos2(left + slot as f32 * (thumb_w + gap), top),
                            egui::vec2(thumb_w, thumb_h),
                        );
                        let response = ui.interact(
                            rect,
                            ui.id().with(("photo-thumb", slot)),
                            egui::Sense::click(),
                        );
                        let inner = rect.shrink(2.0);
                        match texture {
                            Some(texture) => {
                                ui.painter()
                                    .rect_filled(inner, 3.0, egui::Color32::BLACK);
                                paint_fit(ui, inner, texture);
                            }
                            None => {
                                ui.painter().rect_filled(inner, 3.0, CARD_HOVER());
                            }
                        }
                        let stroke = if slot == index {
                            egui::Stroke::new(2.0_f32, egui::Color32::WHITE)
                        } else if response.hovered() {
                            egui::Stroke::new(1.0_f32, egui::Color32::from_white_alpha(110))
                        } else {
                            egui::Stroke::new(1.0_f32, egui::Color32::from_white_alpha(38))
                        };
                        ui.painter().rect_stroke(rect, 4.0, stroke);
                        if response.hovered() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        if response
                            .on_hover_text(format!("Image {} sur {total}", slot + 1))
                            .clicked()
                        {
                            chosen = Some(slot);
                        }
                    }
                }
            });
        if close {
            self.photo = None;
            return;
        }
        // La vignette cliquée prend la main sur les flèches, puis la galerie
        // garde la nouvelle image pour l'image suivante.
        let next = chosen
            .unwrap_or_else(|| (index as i32 + click_step).rem_euclid(total as i32) as usize);
        self.photo = Some(Gallery {
            photos: gallery.photos,
            index: next,
        });
    }

    /// Demande la fiche du jeu si elle n'est pas déjà là (ou en route).
    pub fn ensure(&mut self, ctx: &egui::Context, client: &Client, appid: u32) {
        if self.loaded_for == Some(appid) {
            // La relecture silencieuse attend son tour : le daemon relisait
            // déjà la fiche en tâche de fond quand il a répondu.
            if let Some(at) = self.refresh_at {
                if !self.loading && Instant::now() >= at {
                    self.refresh_at = None;
                    self.start(ctx, client, appid);
                    return;
                }
            }
            if self.data.is_some() || self.loading {
                return;
            }
            // Une lecture qui vient d'échouer attend son tour : « Relire »
            // reste la façon de réessayer tout de suite.
            if self
                .failed_at
                .is_some_and(|at| at.elapsed() < RETRY_AFTER_ERROR)
            {
                return;
            }
        }
        self.start(ctx, client, appid);
    }

    /// Relit la fiche, même quand elle est déjà affichée.
    pub fn reload(&mut self, ctx: &egui::Context, client: &Client, appid: u32) {
        self.start(ctx, client, appid);
    }

    fn start(&mut self, ctx: &egui::Context, client: &Client, appid: u32) {
        // Changer de jeu jette l'ancien contenu ; relire le même le garde
        // affiché pendant que la nouvelle réponse arrive.
        if self.loaded_for != Some(appid) {
            self.forget();
        }
        self.loaded_for = Some(appid);
        self.loading = true;
        self.started = Some(Instant::now());
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        // Le client est cloné : la tâche de fond ne touche pas l'interface.
        let client = client.detached();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = client
                .request(&Req::GameInfo { appid })
                .and_then(|envelope| envelope.into_data::<SteamGameInfo>())
                .map_err(|error| error.to_string());
            if tx.send(result).is_ok() {
                ctx.request_repaint();
            }
        });
    }

    /// Relève la réponse en attente. Appelé à chaque image, même quand la fiche
    /// est refermée : une lecture lancée ne doit pas rester en suspens.
    pub fn poll(&mut self, ctx: &egui::Context) {
        self.news_art.poll(ctx);
        if self.loading && self.started.is_some_and(|started| started.elapsed() > LOAD_BUDGET) {
            self.loading = false;
            self.error = Some(TIMEOUT_ERROR.to_string());
            self.failed_at = Some(Instant::now());
        }
        let Some(rx) = &self.rx else {
            return;
        };
        while let Ok(result) = rx.try_recv() {
            self.loading = false;
            match result {
                // Une réponse pour un autre jeu ne doit pas écraser la fiche
                // affichée : le jeu a changé entre-temps.
                Ok(info) if Some(info.appid) == self.loaded_for => {
                    self.error = None;
                    // Copie servie par le daemon alors qu'elle avait dépassé
                    // sa fenêtre : sa relecture tourne déjà, la fiche la
                    // reprendra d'elle-même dans quelques secondes.
                    let age = lapinux_core::util::now_unix().saturating_sub(info.fetched);
                    let stale = age > STALE_SECONDS;
                    self.data = Some(info);
                    if stale && self.refresh_tries < 2 {
                        self.refresh_tries += 1;
                        self.refresh_at = Some(Instant::now() + REFRESH_AFTER);
                    } else {
                        self.refresh_at = None;
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    self.error = Some(error);
                    self.failed_at = Some(Instant::now());
                    self.refresh_at = None;
                }
            }
        }
        if self.loading {
            ctx.request_repaint_after(Duration::from_millis(120));
        }
    }

    /// Bloc « Détails Steam » de la fiche dépliée : la lecture démarre ici,
    /// quand l'utilisateur ouvre le jeu.
    pub fn block(&mut self, ui: &mut egui::Ui, client: &Client, appid: u32) {
        let ctx = ui.ctx().clone();
        self.ensure(&ctx, client, appid);
        let mut reload = false;
        egui::CollapsingHeader::new(egui::RichText::new("Détails Steam").size(12.5).strong())
            .id_salt(("game-info-steam", appid))
            .default_open(true)
            .show(ui, |ui| {
                reload = status_row(ui, self.data.as_ref(), self.error.as_deref(), self.loading);
                body(ui, self.data.as_ref(), appid);
            });
        if reload {
            self.reload(&ctx, client, appid);
        }
    }

    /// Encart « Actualités » : les annonces de l'éditeur — notes de mise à jour
    /// et actus — en cartes côte à côte, sous le bloc des succès. Rien tant que
    /// la boutique n'a pas répondu, ou quand le jeu ne publie aucune annonce.
    pub fn news_embed(
        &mut self,
        ui: &mut egui::Ui,
        appid: u32,
        bunny: Option<&crate::launch::Bunny>,
    ) -> bool {
        let content = self.content(appid);
        let waiting = self.waiting_for(appid);
        self.news_page(ui, content.as_ref(), waiting, bunny)
    }

    /// Encart « Actualités », quelle que soit l'origine de la fiche : les
    /// trois dernières annonces y sont des cartes, le panneau les lit toutes.
    pub(crate) fn news_page(
        &mut self,
        ui: &mut egui::Ui,
        content: Option<&SheetContent>,
        waiting: bool,
        bunny: Option<&crate::launch::Bunny>,
    ) -> bool {
        // La liste est clonée : les visuels se téléchargent dans le même geste,
        // et le cache de textures ne peut pas être emprunté en même temps que
        // la fiche.
        let from_steam = content
            .map(|content| content.news_from_steam)
            .unwrap_or(false);
        let news: Vec<SteamNews> = content
            .map(|content| content.news.clone())
            .unwrap_or_default();
        if news.is_empty() {
            // La carte existe pendant la lecture : le lapin y attend, comme
            // dans le résumé et les succès.
            if waiting {
                let inner = (ui.available_width() - 20.0).max(120.0);
                egui::Frame::none()
                    .fill(CARD())
                    .rounding(egui::Rounding::same(10.0))
                    .inner_margin(egui::Margin::same(10.0))
                    .show(ui, |ui| {
                        ui.vertical(|ui| {
                            ui.set_width(inner);
                            ui.set_height(96.0);
                            centered_title(ui, inner, "Actualités", 12.5);
                            let rest = ui.available_rect_before_wrap();
                            match bunny {
                                Some(bunny) => crate::launch::waiting_in(
                                    ui,
                                    rest,
                                    bunny,
                                    "Lecture des actualités…",
                                ),
                                None => {
                                    ui.add_space(18.0);
                                    muted(ui, "Lecture des actualités…");
                                }
                            }
                        });
                    });
                return true;
            }
            return false;
        }
        // La carte garde les trois dernières ; le lecteur, ouvert par une
        // carte ou par le bouton, reçoit toute la liste.
        let total = news.len();
        let shown: Vec<&SteamNews> = news.iter().take(3).collect();
        let images: Vec<String> = shown.iter().map(|item| item.image.clone()).collect();
        self.news_art.ensure(ui.ctx(), &images);
        let inner = (ui.available_width() - 20.0).max(120.0);
        egui::Frame::none()
            .fill(CARD())
            .rounding(egui::Rounding::same(10.0))
            .inner_margin(egui::Margin::same(10.0))
            .show(ui, |ui| {
                ui.set_width(inner);
                // Le compte rejoint le titre : centré, il se lit sans
                // chercher le bord droit.
                let count = if total > 1 {
                    format!(" · {total} annonces")
                } else {
                    " · 1 annonce".to_string()
                };
                {
                    let title_width = crate::text_size(ui, "Actualités", 12.5).x;
                    let count_width = crate::text_size(ui, &count, 10.5).x;
                    let info_width = if from_steam { 12.0 } else { 0.0 };
                    let gaps = if from_steam { 8.0 } else { 4.0 };
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        ui.add_space(
                            ((inner - title_width - count_width - info_width - gaps) / 2.0)
                                .max(0.0),
                        );
                        ui.label(egui::RichText::new("Actualités").size(12.5).strong());
                        ui.label(egui::RichText::new(&count).size(10.5).color(TEXT_DIM()));
                        if from_steam {
                            news_source_info(ui);
                        }
                    });
                }
                ui.add_space(8.0);
                // Les cartes se partagent la largeur : trois annonces tiennent
                // sur une ligne, une seule prend la place entière.
                // Côte à côte quand la colonne est large, empilées dans une
                // colonne étroite : trois cartes de 110 px ne montreraient rien.
                let gaps = NEWS_GAP * shown.len().saturating_sub(1) as f32;
                let side_by_side = (inner - gaps) / shown.len() as f32 >= NEWS_CARD_MIN;
                let width = if side_by_side {
                    (inner - gaps) / shown.len() as f32
                } else {
                    inner
                };
                if side_by_side {
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = NEWS_GAP;
                        for (index, item) in shown.iter().copied().enumerate() {
                            if news_card(ui, item, width, self.news_art.get(&item.image)) {
                                self.news_open = Some(NewsFeed {
                                    items: news.clone(),
                                    at: index,
                                });
                            }
                        }
                    });
                } else {
                    for (index, item) in shown.iter().copied().enumerate() {
                        if news_card(ui, item, width, self.news_art.get(&item.image)) {
                            self.news_open = Some(NewsFeed {
                                items: news.clone(),
                                at: index,
                            });
                        }
                        ui.add_space(NEWS_GAP);
                    }
                }
                if total > 3 {
                    ui.add_space(NEWS_GAP);
                    if crate::link_text(ui, &format!("Voir les {total} annonces")).clicked() {
                        self.news_open = Some(NewsFeed {
                            items: news.clone(),
                            at: 0,
                        });
                    }
                }
            });
        true
    }

    /// Encart « Résumé » de la fiche repliée : le texte court de la boutique,
    /// puis trois captures en vignettes dans la place qui reste. `height` est
    /// la hauteur **totale** de la carte : la ligne compacte lui passe celle de
    /// l'encart d'identité, pour que les deux cartes finissent sur la même
    /// ligne.
    ///
    /// La carte existe même avant la réponse : le lapin y tourne pendant la
    /// lecture, pour que la fiche ouvre ses trois encarts d'un coup.
    pub fn summary_embed(
        &mut self,
        ui: &mut egui::Ui,
        appid: u32,
        width: f32,
        height: f32,
        bunny: Option<&crate::launch::Bunny>,
    ) {
        let content = self.content(appid);
        let waiting = self.waiting_for(appid);
        let error = self.error.clone();
        self.summary_page(
            ui,
            content.as_ref(),
            waiting,
            error.as_deref(),
            width,
            height,
            bunny,
        );
    }

    /// Encart « Résumé », le même pour Steam et pour les boutiques : la
    /// présentation courte, puis trois captures.
    pub(crate) fn summary_page(
        &mut self,
        ui: &mut egui::Ui,
        content: Option<&SheetContent>,
        waiting: bool,
        error: Option<&str>,
        width: f32,
        height: f32,
        bunny: Option<&crate::launch::Bunny>,
    ) {
        let Some(content) = content else {
            // La carte existe aussi quand la lecture échoue : la fiche garde
            // ses trois encarts, et l'encart dit pourquoi il est vide.
            if waiting {
                self.waiting_card(ui, width, height, bunny, "Lecture des détails du jeu…");
            } else if let Some(error) = error {
                self.waiting_card(ui, width, height, None, error);
            }
            return;
        };
        let summary = content.short_description.trim().to_string();
        let shots: Vec<String> = content.screenshots.iter().take(3).cloned().collect();
        if summary.is_empty() && shots.is_empty() {
            // Un jeu sans présentation garde son encart : la carte dit
            // « Aucun résumé » plutôt que de disparaître de la ligne.
            self.empty_card(ui, width, height, "Résumé", "Aucun résumé");
            return;
        }
        self.news_art.ensure(ui.ctx(), &shots);
        let inner = (width - 20.0).max(120.0);
        let card = (height - 20.0).max(80.0);
        egui::Frame::none()
            .fill(CARD())
            .rounding(egui::Rounding::same(10.0))
            .inner_margin(egui::Margin::same(10.0))
            .show(ui, |ui| {
                // Le cadre hérite de la disposition de son parent, et la ligne
                // compacte est horizontale : sans colonne explicite, le titre
                // et le texte se peignaient l'un sur l'autre.
                ui.vertical(|ui| {
                    ui.set_width(inner);
                    ui.set_height(card);
                    ui.spacing_mut().item_spacing.y = 6.0;
                    let top = ui.cursor().top();
                    centered_title(ui, inner, "Résumé", 12.5);
                    // La place des vignettes se réserve avant de couper le
                    // texte : les captures tiennent toujours sous lui.
                    let thumb = ((inner - 16.0) / 3.0).max(40.0);
                    let thumb_height = thumb * 9.0 / 16.0;
                    let reserved = if shots.is_empty() {
                        0.0
                    } else {
                        thumb_height + 8.0
                    };
                    // La hauteur occupée se lit sur la réponse du texte :
                    // `set_height` fixe un minimum, pas la place utilisée.
                    let mut used = ui.cursor().top() - top;
                    if !summary.is_empty() {
                        let lines = ((card - 40.0 - reserved) / 14.0).max(1.0) as usize;
                        let text = ui.add(
                            egui::Label::new(
                                egui::RichText::new(clamp_words(&summary, (lines * 62).max(120)))
                                    .size(10.5)
                                    .color(TEXT_DIM()),
                            )
                            .wrap(),
                        );
                        used = text.rect.bottom() - top;
                    }
                    if shots.is_empty() {
                        return;
                    }
                    // Sous le texte, la place qui reste porte trois captures,
                    // à la taille où elles tiennent sans déborder.
                    let leftover = card - used - 8.0;
                    let height = thumb_height.min(leftover);
                    if height < 34.0 {
                        return;
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        for url in &shots {
                            let (rect, response) = ui.allocate_exact_size(
                                egui::vec2(thumb, height),
                                egui::Sense::click(),
                            );
                            match self.news_art.get(url) {
                                Some(texture) => crate::paint_cover(
                                    ui,
                                    rect,
                                    texture,
                                    egui::Rounding::same(6.0),
                                ),
                                None => {
                                    ui.painter().rect_filled(
                                        rect,
                                        egui::Rounding::same(6.0),
                                        CARD_HOVER(),
                                    );
                                }
                            }
                            if response.hovered() {
                                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                            }
                            if response
                                .on_hover_text("Ouvrir la capture en grand")
                                .clicked()
                            {
                                let index = shots.iter().position(|shot| shot == url).unwrap_or(0);
                                self.show_photo(captures(&shots), index);
                            }
                        }
                    });
                });
            });
    }

    /// Encart sans contenu : le cadre des autres cartes, son titre, et son
    /// message au centre. Une carte ne disparaît plus quand la source se tait.
    fn empty_card(&self, ui: &mut egui::Ui, width: f32, height: f32, title: &str, label: &str) {
        let inner = (width - 20.0).max(120.0);
        let card = (height - 20.0).max(80.0);
        egui::Frame::none()
            .fill(CARD())
            .rounding(egui::Rounding::same(10.0))
            .inner_margin(egui::Margin::same(10.0))
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.set_width(inner);
                    ui.set_height(card);
                    centered_title(ui, inner, title, 12.5);
                    let rest = ui.available_rect_before_wrap();
                    ui.vertical_centered(|ui| {
                        ui.add_space((rest.height() / 2.0 - 10.0).max(0.0));
                        ui.label(egui::RichText::new(label).size(11.0).color(TEXT_DIM()));
                    });
                });
            });
    }

    /// Carte d'attente : le cadre des autres encarts, rempli de la forme du
    /// contenu qui arrive — trois lignes de texte et trois vignettes — avec le
    /// lapin qui tourne au centre. La carte ne reste donc pas vide au milieu
    /// de la fiche.
    fn waiting_card(
        &self,
        ui: &mut egui::Ui,
        width: f32,
        height: f32,
        bunny: Option<&crate::launch::Bunny>,
        label: &str,
    ) {
        let inner = (width - 20.0).max(120.0);
        let card = (height - 20.0).max(80.0);
        egui::Frame::none()
            .fill(CARD())
            .rounding(egui::Rounding::same(10.0))
            .inner_margin(egui::Margin::same(10.0))
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.set_width(inner);
                    ui.set_height(card);
                    ui.spacing_mut().item_spacing.y = 6.0;
                    centered_title(ui, inner, "Résumé", 12.5);
                    // La place qui reste porte le lapin, **au centre** de
                    // l'encart : c'est lui qui dit que la lecture travaille,
                    // et l'attente se lit sans chercher un coin de la carte.
                    let rest = ui.available_rect_before_wrap();
                    let Some(bunny) = bunny else {
                        // Lecture échouée : la carte garde son message, centré.
                        ui.vertical_centered(|ui| {
                            ui.add_space((rest.height() / 2.0 - 12.0).max(0.0));
                            ui.label(
                                egui::RichText::new(label)
                                    .size(crate::launch::WAITING_LABEL_FONT)
                                    .color(TEXT_DIM()),
                            );
                        });
                        return;
                    };
                    crate::launch::waiting_in(ui, rest, bunny, label);
                });
            });
    }

    /// Colonne média de la fiche dépliée : la **bande-annonce** en tête, puis
    /// les captures du jeu. La bande-annonce se lit dans le lecteur intégré ;
    /// chaque capture s'ouvre en grand dans le panneau de l'application.
    pub fn media_embed(&mut self, ui: &mut egui::Ui, appid: u32, width: f32) {
        let content = self.content(appid);
        let target = format!("https://store.steampowered.com/app/{appid}");
        self.media_page(
            ui,
            content.as_ref(),
            &target,
            "Voir la bande-annonce sur Steam",
            width,
        );
    }

    /// Colonne média de la fiche, la même pour Steam et pour une boutique :
    /// l'affiche puis les captures, dans un seul encart.
    pub(crate) fn media_page(
        &mut self,
        ui: &mut egui::Ui,
        content: Option<&SheetContent>,
        target: &str,
        hint: &str,
        width: f32,
    ) {
        let Some(content) = content else {
            return;
        };
        let poster = if content.trailer_poster.trim().is_empty() {
            content
                .screenshots
                .first()
                .cloned()
                .unwrap_or_else(|| content.header_image.clone())
        } else {
            content.trailer_poster.clone()
        };
        let shots: Vec<String> = content
            .screenshots
            .iter()
            .skip(1)
            .take(5)
            .cloned()
            .collect();
        if poster.is_empty() && shots.is_empty() {
            return;
        }
        let mut urls: Vec<String> = Vec::new();
        if !poster.is_empty() {
            urls.push(poster.clone());
        }
        urls.extend(shots.iter().cloned());
        // Même cache que les annonces : le téléchargement vit sur son fil.
        self.news_art.ensure(ui.ctx(), &urls);
        let inner = (width - 20.0).max(120.0);
        egui::Frame::none()
            .fill(CARD())
            .rounding(egui::Rounding::same(10.0))
            .inner_margin(egui::Margin::same(10.0))
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.set_width(inner);
                    ui.spacing_mut().item_spacing.y = 7.0;
                    centered_title(ui, inner, "Bande-annonce", 12.5);
                    let height = (inner * 9.0 / 16.0).clamp(90.0, 200.0);
                    let (rect, response) =
                        ui.allocate_exact_size(egui::vec2(inner, height), egui::Sense::click());
                    match self.news_art.get(&poster) {
                        Some(texture) => {
                            crate::paint_cover(ui, rect, texture, egui::Rounding::same(8.0))
                        }
                        None => {
                            ui.painter()
                                .rect_filled(rect, egui::Rounding::same(8.0), CARD_HOVER());
                        }
                    }
                    let trailer = !content.trailer.trim().is_empty();
                    if trailer {
                        // Le bouton de lecture, au centre de l'affiche.
                        ui.painter().circle_filled(
                            rect.center(),
                            20.0,
                            egui::Color32::from_black_alpha(150),
                        );
                        let tip = rect.center() + egui::vec2(2.0, 0.0);
                        ui.painter().add(egui::Shape::convex_polygon(
                            vec![
                                tip + egui::vec2(-5.0, -8.0),
                                tip + egui::vec2(-5.0, 8.0),
                                tip + egui::vec2(8.0, 0.0),
                            ],
                            egui::Color32::from_rgb(248, 250, 252),
                            egui::Stroke::NONE,
                        ));
                    }
                    if response.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    let hint = if trailer {
                        "Lire la bande-annonce dans Lapinux"
                    } else if content.name.is_empty() {
                        hint
                    } else {
                        "Ouvrir la capture en grand"
                    };
                    let response = response.on_hover_text(hint);
                    // Une bande-annonce se joue dans le lecteur intégré ;
                    // sans elle, l'affiche est une capture et s'ouvre en grand.
                    // Une boutique sans page de jeu n'ouvre rien.
                    if response.clicked() {
                        if trailer {
                            self.trailer_want =
                                Some((content.trailer.clone(), content.name.clone()));
                        } else if content.name.is_empty() && !target.is_empty() {
                            ui.ctx()
                                .open_url(egui::OpenUrl::new_tab(target.to_string()));
                        } else {
                            let index =
                                urls.iter().position(|seen| seen == &poster).unwrap_or(0);
                            self.show_photo(captures(&urls), index);
                        }
                    }
                    if shots.is_empty() {
                        return;
                    }
                    ui.add_space(2.0);
                    centered_title(ui, inner, "Captures", 12.5);
                    for url in &shots {
                        let height = (inner * 9.0 / 16.0).clamp(80.0, 170.0);
                        let (rect, response) = ui
                            .allocate_exact_size(egui::vec2(inner, height), egui::Sense::click());
                        match self.news_art.get(url) {
                            Some(texture) => {
                                crate::paint_cover(ui, rect, texture, egui::Rounding::same(8.0))
                            }
                            None => {
                                ui.painter()
                                    .rect_filled(rect, egui::Rounding::same(8.0), CARD_HOVER());
                            }
                        }
                        if response.hovered() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        if response
                            .on_hover_text("Ouvrir la capture en grand")
                            .clicked()
                        {
                            let index = urls.iter().position(|seen| seen == url).unwrap_or(0);
                            self.show_photo(captures(&urls), index);
                        }
                    }
                });
            });
    }

    /// Encart « Description » : le résumé de la boutique, le texte long, puis
    /// les configurations et les mentions, repliées. La fiche le pose sous la
    /// bannière, au centre.
    pub fn description(&mut self, ui: &mut egui::Ui, appid: u32) -> bool {
        let content = self.content(appid);
        self.description_page(ui, content.as_ref(), ("sheet", appid))
    }

    /// Configurations minimale et recommandée, mentions et DRM : repliées,
    /// dans l'onglet Détails de la fiche.
    pub fn requirements(&mut self, ui: &mut egui::Ui, appid: u32) -> bool {
        let Some(content) = self.content(appid) else {
            return false;
        };
        let mut shown = false;
        for (title, text) in &content.requirements {
            collapse_text(ui, title, (title.as_str(), appid), text);
            shown = true;
        }
        shown
    }

    /// Peint un visuel de description, avec ses proportions naturelles.
    fn description_media(
        &mut self,
        ui: &mut egui::Ui,
        url: &str,
        images: &[String],
        inner: f32,
    ) {
        let animated = self.news_art.is_animated(url) || animated_description_image(url);
        let texture = self.news_art.animated(ui.ctx(), url).cloned();
        let natural = texture
            .as_ref()
            .map(|texture| texture.size_vec2())
            .unwrap_or_else(|| {
                if animated {
                    egui::vec2(160.0, 120.0)
                } else {
                    egui::vec2(inner, (inner * 9.0 / 16.0).min(240.0))
                }
            });
        let natural_width = natural.x.max(1.0);
        let natural_height = natural.y.max(1.0);
        // Aucun rognage : une affiche AVIF large ou un petit GIF garde ses
        // proportions. Le plafond de hauteur évite qu'une image verticale
        // prenne toute la fiche.
        let scale = (inner / natural_width)
            .min(240.0 / natural_height)
            .min(1.0);
        let width = natural_width * scale;
        let height = natural_height * scale;
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(inner, height.max(24.0)),
            egui::Sense::click(),
        );
        let image_rect = egui::Rect::from_center_size(
            rect.center(),
            egui::vec2(width.max(24.0), height.max(24.0)),
        );
        match texture.as_ref() {
            Some(texture) => paint_fit(ui, image_rect, texture),
            None => {
                ui.painter()
                    .rect_filled(image_rect, egui::Rounding::same(8.0), CARD_HOVER());
            }
        }
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if response.on_hover_text("Ouvrir l'image en grand").clicked() {
            let index = images.iter().position(|seen| seen == url).unwrap_or(0);
            self.show_photo(captures(images), index);
        }
        ui.add_space(8.0);
    }

    /// Description longue, la même pour Steam et pour une boutique : le texte
    /// et les visuels dans l'ordre de la page.
    pub(crate) fn description_page(
        &mut self,
        ui: &mut egui::Ui,
        content: Option<&SheetContent>,
        _salt: (&str, u32),
    ) -> bool {
        let Some(content) = content else {
            return false;
        };
        let empty = content.description.trim().is_empty()
            && content.description_images.is_empty()
            && content.description_blocks.is_empty();
        if empty {
            return false;
        }
        let available = ui.available_width().max(120.0);
        let inner = available.min(DESCRIPTION_TEXT_WIDTH).max(120.0);
        let left = ((available - inner) / 2.0).max(0.0);
        // Les visuels que la boutique place dans le texte se téléchargent
        // comme ceux des annonces, une fois par adresse.
        let images = content.description_images.clone();
        self.news_art.ensure(ui.ctx(), &images);
        // Pas de cadre : la description se lit directement sous la bannière,
        // centrée sur la largeur de la fiche.
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.add_space(left);
            ui.vertical(|ui| {
                ui.set_width(inner);
            // Dès qu'on connaît le découpage, le texte et les visuels
            // reprennent l'ordre de la page Steam : image, paragraphe, image.
            if !content.description_blocks.is_empty() {
                for block in &content.description_blocks {
                    match block {
                        DescriptionBlock::Text(text) => {
                            let text = text.trim();
                            if text.is_empty() {
                                continue;
                            }
                            ui.add_space(6.0);
                            centered_description_text(ui, inner, text);
                        }
                        DescriptionBlock::Media(url) => {
                            ui.add_space(8.0);
                            self.description_media(ui, url, &images, inner);
                        }
                    }
                }
            }
            // Les fiches plus anciennes n'ont pas encore ce découpage : elles
            // gardent leur texte puis leurs visuels.
            if content.description_blocks.is_empty() && !content.description.trim().is_empty() {
                ui.add_space(6.0);
                centered_description_text(ui, inner, &content.description);
            }
            if content.description_blocks.is_empty() && !images.is_empty() {
                ui.add_space(10.0);
                for url in &images {
                    self.description_media(ui, url, &images, inner);
                }
            }
            });
        });
        true
    }

}

/// Petit « i » de provenance : les annonces viennent de Steam parce qu'Epic
/// n'en publie pas pour ce jeu. Le survol explique la limite.
fn news_source_info(ui: &mut egui::Ui) {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
    let center = rect.center();
    let painter = ui.painter();
    painter.circle_stroke(center, 5.0, egui::Stroke::new(1.0_f32, TEXT_DIM()));
    painter.text(
        center + egui::vec2(0.0, 0.5),
        egui::Align2::CENTER_CENTER,
        "i",
        egui::FontId::proportional(8.5),
        TEXT_DIM(),
    );
    let _ = response.on_hover_text(
        "Actualités reprises de la fiche Steam du même jeu : Epic Games n'en publie pas pour celui-ci. Ces informations peuvent être incomplètes ou inexactes.",
    );
}

/// Une carte d'actualité : le visuel de l'annonce quand Steam le publie, puis
/// le titre, le début du texte et la date. La carte entière ouvre le lecteur
/// d'annonces, avec toute la liste.
fn news_card(
    ui: &mut egui::Ui,
    news: &SteamNews,
    width: f32,
    image: Option<&egui::TextureHandle>,
) -> bool {
    let inner = (width - 16.0).max(60.0);
    // L'aperçu suit la largeur de sa carte : quatre lignes, quelle que soit la
    // place, pour que trois annonces ne fassent pas trois hauteurs.
    let per_line = (inner / 5.2).max(12.0);
    let excerpt = clamp_words(&news.excerpt, (per_line * 4.0) as usize);
    let frame = egui::Frame::none()
        .fill(CARD_HOVER())
        .rounding(egui::Rounding::same(8.0))
        .inner_margin(egui::Margin::same(8.0))
        .show(ui, |ui| {
            // La carte vit dans une rangée horizontale : sans mise en page
            // verticale explicite, ses trois textes se peignaient côte à
            // côte, sur une seule ligne qui débordait de la fiche.
            ui.vertical(|ui| {
                ui.set_width(inner);
                if let Some(texture) = image {
                    // Le visuel d'une annonce Steam est une image 16/9 : il
                    // prend la largeur de la carte, en bandeau.
                    let height = (inner * 9.0 / 16.0).clamp(60.0, 150.0);
                    let (rect, _) = ui.allocate_exact_size(
                        egui::vec2(inner, height),
                        egui::Sense::hover(),
                    );
                    crate::paint_cover(ui, rect, texture, egui::Rounding::same(6.0));
                    ui.add_space(7.0);
                }
                ui.label(
                    egui::RichText::new(lapinux_core::util::ellipsize(&news.title, 72))
                        .size(11.5)
                        .strong(),
                );
                if !excerpt.is_empty() {
                    ui.add_space(3.0);
                    ui.label(egui::RichText::new(&excerpt).size(10.0).color(TEXT_DIM()));
                }
                ui.add_space(4.0);
                let mut line = lapinux_core::util::format_date(news.date);
                if !news.author.is_empty() {
                    line.push_str(&format!(" · {}", news.author));
                }
                ui.label(egui::RichText::new(line).size(9.5).color(TEXT_DIM()));
            });
        });
    let response = ui.interact(
        frame.response.rect,
        ui.id().with(("news-card", news.date, news.title.as_str())),
        egui::Sense::click(),
    );
    let response = response.on_hover_text(format!("{} — lire l'annonce", news.title));
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.clicked()
}

/// Poids maximal des visuels d'annonces gardés ouverts : une capture de presse
/// pèse plusieurs mégaoctets en pixels, et la fiche change de jeu souvent.
const NEWS_ART_BUDGET: usize = 256 * 1024 * 1024;
/// Téléchargements d'images simultanés : précharger une bibliothèque ne doit
/// pas ouvrir cinquante connexions d'un coup.
const NEWS_ART_FETCHES: usize = 8;

/// Décode un visuel à une taille raisonnable : les images de Steam font
/// 1920 px de large, et une centaine d'entre elles saturent vite la mémoire.
/// Le lecteur d'annonces n'en affiche jamais plus de 720 px de large.
fn news_texture_from_bytes(
    ctx: &egui::Context,
    name: &str,
    bytes: &[u8],
) -> Option<egui::TextureHandle> {
    let image = image::load_from_memory(bytes).ok()?;
    let image = image.thumbnail(800, 450).to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    if size[0] == 0 || size[1] == 0 {
        return None;
    }
    let color = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
    Some(ctx.load_texture(name, color, egui::TextureOptions::LINEAR))
}

/// Visuels des annonces : téléchargés une fois par adresse, puis gardés en
/// texture. Le téléchargement vit sur son propre fil, la fiche ne l'attend pas.
pub(crate) struct NewsArt {
    textures: std::collections::HashMap<String, egui::TextureHandle>,
    /// Petits GIF de description, joués seulement quand ils sont affichés.
    gifs: std::collections::HashMap<String, crate::GifPlayer>,
    /// Budget des images décodées : les plus anciennes sortent quand la fiche a
    /// parcouru trop de jeux.
    budget: crate::TextureBudget<String>,
    pending: std::collections::HashSet<String>,
    rx: Option<mpsc::Receiver<(String, Vec<u8>)>>,
    tx: Option<mpsc::Sender<(String, Vec<u8>)>>,
}

impl Default for NewsArt {
    fn default() -> Self {
        Self {
            textures: std::collections::HashMap::new(),
            gifs: std::collections::HashMap::new(),
            budget: crate::TextureBudget::new(NEWS_ART_BUDGET),
            pending: std::collections::HashSet::new(),
            rx: None,
            tx: None,
        }
    }
}

impl NewsArt {
    pub(crate) fn ensure(&mut self, ctx: &egui::Context, urls: &[String]) {
        if self.pending.len() >= NEWS_ART_FETCHES {
            return;
        }
        if self.tx.is_none() {
            let (tx, rx) = mpsc::channel();
            self.tx = Some(tx);
            self.rx = Some(rx);
        }
        let Some(tx) = self.tx.clone() else {
            return;
        };
        for url in urls {
            if url.is_empty()
                || self.textures.contains_key(url)
                || self.pending.contains(url)
            {
                continue;
            }
            if self.pending.len() >= NEWS_ART_FETCHES {
                break;
            }
            self.pending.insert(url.clone());
            let (url, tx, ctx) = (url.clone(), tx.clone(), ctx.clone());
            std::thread::spawn(move || {
                if let Some(bytes) = crate::achievements::http_bytes(&url) {
                    if let Some(bytes) = prepare_media(&url, &bytes) {
                        if tx.send((url, bytes)).is_ok() {
                            ctx.request_repaint();
                        }
                    }
                }
            });
        }
    }

    /// Relève les visuels arrivés. Appelé à chaque image, fiche affichée ou
    /// non : un téléchargement lancé ne doit pas rester en suspens.
    pub(crate) fn poll(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.rx else {
            return;
        };
        let mut received = Vec::new();
        while let Ok(message) = rx.try_recv() {
            received.push(message);
        }
        for (url, bytes) in received {
            self.pending.remove(&url);
            if bytes.starts_with(b"GIF8") {
                if let Some(player) = crate::GifPlayer::load(ctx, &bytes, &url, [800, 450])
                {
                    let texture = player.first_texture();
                    self.budget.note(url.clone(), &texture);
                    self.textures.insert(url.clone(), texture);
                    self.gifs.insert(url, player);
                    self.trim();
                    continue;
                }
            }
            if let Some(texture) = news_texture_from_bytes(ctx, &url, &bytes) {
                self.budget.note(url.clone(), &texture);
                self.textures.insert(url, texture);
                self.trim();
            }
        }
    }

    /// Libère les visuels sortis du budget, frames animées comprises.
    fn trim(&mut self) {
        for key in self.budget.trim() {
            self.textures.remove(&key);
            self.gifs.remove(&key);
        }
    }

    pub(crate) fn get(&self, url: &str) -> Option<&egui::TextureHandle> {
        self.textures.get(url)
    }

    /// Texture de la frame courante d'un GIF, ou image fixe sinon.
    pub(crate) fn animated(
        &mut self,
        ctx: &egui::Context,
        url: &str,
    ) -> Option<&egui::TextureHandle> {
        if let Some(player) = self.gifs.get_mut(url) {
            Some(player.texture(ctx))
        } else {
            self.textures.get(url)
        }
    }

    /// Vrai dès qu'un média animé a été décodé, même sans extension visible.
    pub(crate) fn is_animated(&self, url: &str) -> bool {
        self.gifs.contains_key(url)
    }
}

fn media_path(url: &str) -> String {
    url.split(['?', '#'])
        .next()
        .unwrap_or(url)
        .to_ascii_lowercase()
}

/// Les petits GIF et les boucles vidéo de description sont gardés à leur
/// taille naturelle : les étirer à la largeur d'une capture les rendrait
/// énormes.
fn animated_description_image(url: &str) -> bool {
    let path = media_path(url);
    path.ends_with(".gif") || path.ends_with(".webm") || path.ends_with(".mp4")
}

/// Prépare un média de description pour le cache de textures : ffmpeg
/// convertit les AVIF en PNG et les petites boucles WebM/MP4 en GIF animé.
/// Les formats déjà lus par le crate `image` passent tels quels.
fn prepare_media(url: &str, bytes: &[u8]) -> Option<Vec<u8>> {
    let path = media_path(url);
    if path.ends_with(".avif") {
        return ffmpeg_media(bytes, "png", &["-frames:v", "1"]);
    }
    if path.ends_with(".webm") || path.ends_with(".mp4") {
        return ffmpeg_media(
            bytes,
            "gif",
            &[
                "-t",
                "3",
                "-vf",
                "fps=8,scale=640:-2:flags=lanczos",
                "-loop",
                "0",
            ],
        );
    }
    Some(bytes.to_vec())
}

/// Lance ffmpeg sur des octets déjà téléchargés, par deux fichiers temporaires
/// uniques. Le résultat est lu puis les deux fichiers sont retirés.
fn ffmpeg_media(bytes: &[u8], extension: &str, args: &[&str]) -> Option<Vec<u8>> {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let stem = format!("lapinux-media-{}-{seq}", std::process::id());
    let input = std::env::temp_dir().join(format!("{stem}.bin"));
    let output = std::env::temp_dir().join(format!("{stem}.{extension}"));
    std::fs::write(&input, bytes).ok()?;
    let status = std::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(&input)
        .args(args)
        .arg(&output)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    let data = status
        .ok()
        .filter(|status| status.success())
        .and_then(|_| std::fs::read(&output).ok());
    let _ = std::fs::remove_file(&input);
    let _ = std::fs::remove_file(&output);
    data
}

/// Rectangle de l'image entière, centrée dans la place libre sans être
/// rognée. Les bandes qui restent reviennent à l'appelant.
fn fit_rect(area: egui::Rect, size: egui::Vec2) -> egui::Rect {
    if size.x <= 0.0 || size.y <= 0.0 {
        return area;
    }
    let scale = (area.width() / size.x).min(area.height() / size.y);
    egui::Rect::from_center_size(area.center(), size * scale)
}

/// Peint la texture entière dans le rectangle, à la bonne proportion : rien
/// n'est rogné, l'image est centrée dans la place libre.
pub(crate) fn paint_fit(ui: &egui::Ui, rect: egui::Rect, texture: &egui::TextureHandle) {
    let size = texture.size_vec2();
    if size.x <= 0.0 || size.y <= 0.0 {
        return;
    }
    let shown = fit_rect(rect, size);
    ui.painter().image(
        texture.id(),
        shown,
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );
}

/// Coupe un texte au nombre de caractères demandé, sans casser un mot.
pub(crate) fn clamp_words(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(max).collect();
    if let Some(index) = cut.rfind(' ') {
        cut.truncate(index);
    }
    cut.push('…');
    cut
}

/// Ligne d'état du bloc : ce qui se lit, ce qui a échoué, et la relecture.
/// Rend vrai quand « Relire » a été demandé.
fn status_row(
    ui: &mut egui::Ui,
    info: Option<&SteamGameInfo>,
    error: Option<&str>,
    loading: bool,
) -> bool {
    let mut reload = false;
    ui.horizontal(|ui| {
        if loading {
            ui.spinner();
            ui.label(
                egui::RichText::new("Lecture des détails Steam…")
                    .size(10.5)
                    .color(TEXT_DIM()),
            );
        } else if let Some(error) = error {
            ui.label(egui::RichText::new(error).size(10.5).color(DANGER));
        } else if let Some(info) = info {
            let mut text = match info.fetched {
                0 => "Lu à l'instant".to_string(),
                fetched => format!("Relu le {}", lapinux_core::util::format_datetime(fetched)),
            };
            if !info.sources.is_empty() {
                text.push_str(&format!(" · {}", info.sources.join(" · ")));
            }
            ui.label(egui::RichText::new(text).size(10.5).color(TEXT_DIM()));
        } else {
            ui.label(
                egui::RichText::new("Aucune lecture demandée")
                    .size(10.5)
                    .color(TEXT_DIM()),
            );
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let hint = "Relit le compte, le protocole et la boutique";
            if ui
                .add(egui::Button::new(egui::RichText::new("Relire").size(10.5)))
                .on_hover_text(hint)
                .clicked()
            {
                reload = true;
            }
        });
    });
    reload
}

/// Corps du bloc : les groupes lus, puis ce qui n'a pas pu l'être.
///
/// Les mises à jour et la description ont leurs propres encarts dans la
/// fiche : le bloc ne les reprend pas, sinon la même information s'y lit deux
/// fois.
fn body(ui: &mut egui::Ui, info: Option<&SteamGameInfo>, appid: u32) {
    let Some(info) = info else {
        ui.add_space(4.0);
        muted(
            ui,
            "Le compte, le protocole et la boutique se lisent en tâche de fond : la fiche s'affiche dès que Steam répond.",
        );
        return;
    };
    let account = account_rows(&info.account);
    let store = store_rows(&info.store, appid);
    if account.is_empty() && store.is_empty() && info.stats.is_empty() {
        ui.add_space(4.0);
        muted(
            ui,
            "Aucune source n'a répondu pour ce jeu : connectez le compte Steam depuis les réglages.",
        );
    }
    group(ui, "Mon compte", ("game-info-account", appid), &account);
    group(ui, "Boutique", ("game-info-store", appid), &store);
    stats_group(ui, &info.stats, appid);
    if !info.notes.is_empty() {
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new("Sources muettes")
                .size(11.0)
                .strong()
                .color(TEXT_DIM()),
        );
        for note in &info.notes {
            ui.label(egui::RichText::new(note).size(10.0).color(TEXT_DIM()));
        }
    }
}

/// Groupe de faits : un titre discret, puis un tableau libellé / valeur.
fn group(ui: &mut egui::Ui, title: &str, salt: (&str, u32), rows: &[(&str, String)]) {
    if rows.is_empty() {
        return;
    }
    ui.add_space(8.0);
    ui.label(
        egui::RichText::new(title)
            .size(11.0)
            .strong()
            .color(TEXT_DIM()),
    );
    ui.add_space(2.0);
    egui::Grid::new(salt)
        .num_columns(2)
        .spacing([10.0, 4.0])
        .show(ui, |ui| {
            for (label, value) in rows {
                ui.label(egui::RichText::new(*label).size(10.5).color(TEXT_DIM()));
                if value.starts_with("http") {
                    ui.hyperlink_to(egui::RichText::new(value.as_str()).size(10.5), value);
                } else {
                    // La grille ne contraint pas la largeur d'une cellule : sans
                    // ce plafond, les longues listes débordent du panneau.
                    let width = ui.available_width();
                    ui.allocate_ui_with_layout(
                        egui::vec2(width, 0.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(value.as_str()).size(10.5),
                                )
                                .wrap(),
                            );
                        },
                    );
                }
                ui.end_row();
            }
        });
}

/// Ce que le compte a fait du jeu.
fn account_rows(play: &SteamAccountPlay) -> Vec<(&'static str, String)> {
    let read = play.owned
        || play.minutes_total.is_some()
        || play.level.is_some()
        || play.players_now.is_some();
    if !read {
        return Vec::new();
    }
    let duration =
        |minutes: u64| lapinux_core::util::format_duration(minutes.saturating_mul(60));
    let mut rows: Vec<(&'static str, String)> = Vec::new();
    rows.push((
        "Possédé",
        if play.owned {
            "Oui, sur ce compte".into()
        } else {
            "Non : prêté par la famille ou hors du compte".into()
        },
    ));
    if let Some(minutes) = play.minutes_total {
        rows.push(("Temps de jeu", duration(minutes)));
    }
    if let Some(minutes) = play.minutes_2weeks {
        rows.push(("Deux dernières semaines", duration(minutes)));
    }
    for (label, minutes) in [
        ("Windows", play.minutes_windows),
        ("Linux", play.minutes_linux),
        ("macOS", play.minutes_macos),
        ("Steam Deck", play.minutes_deck),
    ] {
        if let Some(minutes) = minutes.filter(|minutes| *minutes > 0) {
            rows.push((label, duration(minutes)));
        }
    }
    if let Some(last) = play.last_played.filter(|last| *last > 0) {
        rows.push(("Dernière partie", lapinux_core::util::format_datetime(last)));
    }
    if let Some(level) = play.level {
        rows.push(("Niveau Steam", level.to_string()));
    }
    if let Some(level) = play.badge_level {
        rows.push(("Badge du jeu", format!("niveau {level}")));
    }
    if let Some(players) = play.players_now {
        rows.push(("Joueurs en ligne", thousands(players)));
    }
    if let Some(total) = play.achievements_total.filter(|total| *total > 0) {
        let unlocked = play.achievements_unlocked.unwrap_or(0);
        let share = f64::from(unlocked) / f64::from(total) * 100.0;
        rows.push((
            "Succès",
            format!("{unlocked} / {total} · {} %", percent(share)),
        ));
    }
    if !play.last_achievement.is_empty() {
        let value = match play.last_achievement_time.filter(|time| *time > 0) {
            Some(time) => format!(
                "{} · {}",
                play.last_achievement,
                lapinux_core::util::format_date(time)
            ),
            None => play.last_achievement.clone(),
        };
        rows.push(("Dernier succès", value));
    }
    if !play.rarest.is_empty() {
        let lines: Vec<String> = play
            .rarest
            .iter()
            .take(RARE_ROWS)
            .map(|rare| {
                let state = if rare.unlocked {
                    "débloqué"
                } else {
                    "manquant"
                };
                format!("{} · {} % · {state}", rare.name, percent(f64::from(rare.percent)))
            })
            .collect();
        rows.push(("Succès les plus rares", lines.join("\n")));
    }
    rows
}

/// Ce que la page publique de la boutique raconte du jeu.
fn store_rows(store: &SteamStorePage, appid: u32) -> Vec<(&'static str, String)> {
    let mut rows: Vec<(&'static str, String)> = Vec::new();
    if !store.price.is_empty() {
        rows.push(("Prix", store.price.clone()));
    }
    // Le résumé des avis vient de la page communautaire, que Steam ne sert
    // plus sans cookie de session ; son compte, lui, vient d'`appdetails`.
    let votes = store.review_count.or(store.recommendations);
    if !store.review_summary.is_empty() || votes.is_some() {
        let mut parts: Vec<String> = Vec::new();
        if !store.review_summary.is_empty() {
            parts.push(store.review_summary.clone());
        }
        if let Some(count) = votes {
            parts.push(format!("{} avis", thousands(count)));
        }
        rows.push(("Avis des joueurs", parts.join(" · ")));
    }
    if !store.release.is_empty() {
        rows.push(("Sortie", store.release.clone()));
    }
    if !store.genres.is_empty() {
        rows.push(("Genres", store.genres.join(" · ")));
    }
    if !store.categories.is_empty() {
        rows.push(("Fonctionnalités", store.categories.join(" · ")));
    }
    if !store.descriptors.is_empty() {
        rows.push(("Contenu", store.descriptors.join(" · ")));
    }
    if !store.short_description.is_empty() {
        rows.push(("Résumé", store.short_description.clone()));
    }
    if !store.screenshots.is_empty() {
        rows.push((
            "Captures",
            plural(store.screenshots.len(), "publiée", "publiées"),
        ));
    }
    if !store.website.is_empty() {
        rows.push(("Site officiel", store.website.clone()));
    }
    if !store.support_url.is_empty() {
        rows.push(("Assistance", store.support_url.clone()));
    } else if !store.support_email.is_empty() {
        rows.push(("Assistance", store.support_email.clone()));
    }
    rows.push((
        "Page boutique",
        format!("https://store.steampowered.com/app/{appid}"),
    ));
    if rows.len() <= 1 {
        return Vec::new();
    }
    rows
}

fn collapse_text(ui: &mut egui::Ui, title: &str, salt: (&str, u32), text: &str) {
    egui::CollapsingHeader::new(egui::RichText::new(title).size(11.5))
        .id_salt(salt)
        .default_open(false)
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt((salt.0, salt.1, "scroll"))
                .max_height(200.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(ellipsize_text(text, TEXT_LIMIT)).size(10.5),
                        )
                        .wrap(),
                    );
                });
        });
}

/// Statistiques publiées par le jeu, avec la valeur du compte.
fn stats_group(ui: &mut egui::Ui, stats: &[SteamStat], appid: u32) {
    if stats.is_empty() {
        return;
    }
    ui.add_space(8.0);
    egui::CollapsingHeader::new(
        egui::RichText::new(format!("Statistiques du compte ({})", stats.len())).size(11.5),
    )
    .id_salt(("game-info-stats", appid))
    .default_open(false)
    .show(ui, |ui| {
        egui::ScrollArea::vertical()
            .id_salt(("game-info-stats-scroll", appid))
            .max_height(220.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                egui::Grid::new(("game-info-stats-grid", appid))
                    .num_columns(2)
                    .spacing([10.0, 4.0])
                    .show(ui, |ui| {
                        for stat in stats {
                            let label = if stat.display.trim().is_empty() {
                                stat.name.clone()
                            } else {
                                stat.display.clone()
                            };
                            ui.label(
                                egui::RichText::new(label)
                                    .size(crate::launch::WAITING_LABEL_FONT)
                                    .color(TEXT_DIM()),
                            );
                            ui.label(egui::RichText::new(&stat.value).size(10.5));
                            ui.end_row();
                        }
                    });
            });
    });
}

/// Accord du nombre : « 1 publié », « 3 publiés ».
fn plural(value: usize, singular: &str, many: &str) -> String {
    if value > 1 {
        format!("{value} {many}")
    } else {
        format!("{value} {singular}")
    }
}

/// Milliers séparés : « 12 345 joueurs ».
fn thousands(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(' ');
        }
        out.push(digit);
    }
    out
}

/// Pourcentage à la virgule décimale de l'interface.
fn percent(value: f64) -> String {
    format!("{value:.1}").replace('.', ",")
}


/// Pictogramme d'une fonctionnalité. Les traits sont composés comme le reste
/// des marques de l'application : le sous-ensemble Remix embarqué ne contient
/// que les glyphes de la navigation, et une icône de plus dans la police
/// coûterait un fichier de fonte entier pour quinze traits.
///
/// L'encart des fonctionnalités n'existe plus dans la fiche, mais la marque du
/// nuage sert encore : c'est elle qui signale les sauvegardes cloud dans
/// l'encart d'un jeu.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeatureIcon {
    Solo,
    /// Plusieurs joueurs : multijoueur, PvP, coopération.
    Duo,
    Cloud,
    Trophy,
    /// Atelier, éditeur de niveau : des outils.
    Wrench,
    Cards,
    Pad,
    /// Remote Play, écran partagé.
    Screen,
    Vr,
    /// Partage familial : la maison.
    Family,
    Shield,
    Cart,
    Stats,
    /// Sous-titres, son, couleurs, difficulté : des réglages.
    Options,
    Generic,
}

#[allow(dead_code)]
impl FeatureIcon {
    /// L'identifiant de la catégorie Steam tranche ; le libellé prend le
    /// relais pour les catégories que la boutique ajoute au fil des ans.
    fn of(feature: &SteamFeature) -> Self {
        match feature.id {
            2 => FeatureIcon::Solo,
            // Multijoueur, coopération, JcJ, PvP, LAN.
            1 | 9 | 36 | 38 | 39 | 48 | 49 => FeatureIcon::Duo,
            8 => FeatureIcon::Shield,
            // Manettes : partielle, complète, DUALSHOCK, DualSense, Steam Input.
            18 | 28 | 52 | 55 | 56 | 57 | 58 | 59 => FeatureIcon::Pad,
            22 => FeatureIcon::Trophy,
            23 => FeatureIcon::Cloud,
            24 => FeatureIcon::Screen,
            29 => FeatureIcon::Cards,
            17 | 30 | 51 => FeatureIcon::Wrench,
            31 | 40 | 54 => FeatureIcon::Vr,
            35 => FeatureIcon::Cart,
            41 | 42 | 43 | 44 => FeatureIcon::Screen,
            15 => FeatureIcon::Stats,
            62 => FeatureIcon::Family,
            // Sous-titres, commentaires, confort de vue, son, difficulté.
            13 | 14 | 61 | 63..=79 => FeatureIcon::Options,
            _ => FeatureIcon::from_label(&feature.label),
        }
    }

    fn from_label(label: &str) -> Self {
        let label = label.to_lowercase();
        let has = |words: &[&str]| words.iter().any(|word| label.contains(word));
        match () {
            _ if has(&["solo"]) => FeatureIcon::Solo,
            _ if has(&["multi", "coop", "pvp", "jcj", "joueur"]) => FeatureIcon::Duo,
            _ if has(&["cloud", "nuage"]) => FeatureIcon::Cloud,
            _ if has(&["succès", "troph"]) => FeatureIcon::Trophy,
            _ if has(&["atelier", "workshop", "éditeur", "editeur"]) => FeatureIcon::Wrench,
            _ if has(&["carte"]) => FeatureIcon::Cards,
            _ if has(&["manette", "contrôleur", "controleur"]) => FeatureIcon::Pad,
            _ if has(&["remote", "écran", "ecran"]) => FeatureIcon::Screen,
            _ if has(&["vr", "casque"]) => FeatureIcon::Vr,
            _ if has(&["famille", "partage"]) => FeatureIcon::Family,
            _ if has(&["antitriche", "anti-triche"]) => FeatureIcon::Shield,
            _ if has(&["achat"]) => FeatureIcon::Cart,
            _ if has(&["classement", "statistique"]) => FeatureIcon::Stats,
            _ => FeatureIcon::Generic,
        }
    }
}

/// Dessine le pictogramme d'une fonctionnalité dans un carré de `size` pixels
/// centré sur `center`. Tous les traits sont tracés dans un repère de 18
/// pixels, puis mis à l'échelle.
pub fn paint_feature_icon(
    painter: &egui::Painter,
    center: egui::Pos2,
    size: f32,
    ink: egui::Color32,
    kind: FeatureIcon,
) {
    let scale = size / 18.0;
    let stroke = egui::Stroke::new(1.4_f32 * scale, ink);
    let at = |x: f32, y: f32| center + egui::vec2(x, y) * scale;
    let line = |points: Vec<egui::Pos2>| painter.add(egui::Shape::line(points, stroke));
    let closed = |points: Vec<egui::Pos2>| painter.add(egui::Shape::closed_line(points, stroke));
    let ring = |x: f32, y: f32, radius: f32| {
        painter.circle_stroke(at(x, y), radius * scale, stroke);
    };
    let frame = |width: f32, height: f32, y: f32, radius: f32| {
        painter.rect_stroke(
            egui::Rect::from_center_size(at(0.0, y), egui::vec2(width, height) * scale),
            egui::Rounding::same(radius * scale),
            stroke,
        );
    };
    match kind {
        FeatureIcon::Solo => {
            ring(0.0, -2.8, 3.0);
            line(vec![
                at(-5.6, 6.8),
                at(-4.6, 3.2),
                at(-2.4, 1.9),
                at(2.4, 1.9),
                at(4.6, 3.2),
                at(5.6, 6.8),
            ]);
        }
        FeatureIcon::Duo => {
            ring(-3.6, -3.2, 2.4);
            ring(3.6, -3.2, 2.4);
            line(vec![
                at(-7.6, 6.6),
                at(-6.4, 3.8),
                at(-3.8, 2.6),
                at(-1.6, 2.6),
                at(0.0, 1.8),
                at(1.6, 2.6),
                at(3.8, 2.6),
                at(6.4, 3.8),
                at(7.6, 6.6),
            ]);
        }
        FeatureIcon::Cloud => {
            ring(-3.6, 1.0, 2.9);
            ring(0.4, -2.0, 3.4);
            ring(4.4, 1.0, 2.9);
            line(vec![at(-6.5, 3.9), at(7.3, 3.9)]);
        }
        FeatureIcon::Trophy => {
            line(vec![
                at(-4.6, -5.6),
                at(-3.9, 1.2),
                at(0.0, 3.0),
                at(3.9, 1.2),
                at(4.6, -5.6),
            ]);
            line(vec![at(-4.8, -5.6), at(4.8, -5.6)]);
            line(vec![at(0.0, 3.0), at(0.0, 5.6)]);
            line(vec![at(-3.4, 6.6), at(3.4, 6.6)]);
        }
        FeatureIcon::Wrench => {
            // Un marteau : la tête tient sur l'extrémité du manche.
            painter.rect_stroke(
                egui::Rect::from_center_size(at(2.6, -4.2), egui::vec2(9.4, 4.6) * scale),
                egui::Rounding::same(1.4 * scale),
                stroke,
            );
            line(vec![at(1.6, -1.6), at(-4.8, 6.2)]);
        }
        FeatureIcon::Cards => {
            painter.rect_stroke(
                egui::Rect::from_center_size(at(-1.6, -1.6), egui::vec2(8.6, 11.4) * scale),
                egui::Rounding::same(1.6 * scale),
                stroke,
            );
            painter.rect_stroke(
                egui::Rect::from_center_size(at(2.6, 2.6), egui::vec2(8.6, 11.4) * scale),
                egui::Rounding::same(1.6 * scale),
                stroke,
            );
        }
        FeatureIcon::Pad => {
            frame(15.0, 9.2, 0.4, 4.2);
            ring(-4.2, 0.4, 1.5);
            ring(4.2, 0.4, 1.5);
        }
        FeatureIcon::Screen => {
            frame(15.4, 10.6, -0.8, 1.8);
            closed(vec![at(-1.9, -3.0), at(-1.9, 1.4), at(2.3, -0.8)]);
            line(vec![at(-3.4, 6.6), at(3.4, 6.6)]);
        }
        FeatureIcon::Vr => {
            frame(16.2, 9.0, 0.2, 3.2);
            ring(-3.6, 0.2, 2.0);
            ring(3.6, 0.2, 2.0);
        }
        FeatureIcon::Family => {
            line(vec![at(-6.6, -0.4), at(0.0, -6.4), at(6.6, -0.4)]);
            line(vec![at(-4.9, -1.2), at(-4.9, 6.2), at(4.9, 6.2), at(4.9, -1.2)]);
        }
        FeatureIcon::Shield => {
            closed(vec![
                at(0.0, -6.6),
                at(5.4, -4.2),
                at(5.4, 1.4),
                at(0.0, 6.6),
                at(-5.4, 1.4),
                at(-5.4, -4.2),
            ]);
            line(vec![at(-2.2, 0.2), at(-0.5, 2.2), at(2.6, -1.8)]);
        }
        FeatureIcon::Cart => {
            line(vec![at(-7.0, -5.6), at(-4.4, -5.6), at(-2.4, 2.2)]);
            line(vec![at(-3.6, -2.8), at(6.8, -2.8), at(5.4, 2.2)]);
            ring(-1.4, 5.2, 1.5);
            ring(4.6, 5.2, 1.5);
        }
        FeatureIcon::Stats => {
            line(vec![at(-7.2, 6.2), at(7.2, 6.2)]);
            line(vec![at(-4.6, 6.2), at(-4.6, 0.4)]);
            line(vec![at(0.0, 6.2), at(0.0, -3.6)]);
            line(vec![at(4.6, 6.2), at(4.6, -6.2)]);
        }
        FeatureIcon::Options => {
            for (y, knob) in [(-4.4, 2.6), (0.0, -2.8), (4.4, 1.4)] {
                line(vec![at(-6.6, y), at(6.6, y)]);
                ring(knob, y, 1.6);
            }
        }
        FeatureIcon::Generic => {
            ring(0.0, 0.0, 4.4);
            painter.circle_filled(at(0.0, 0.0), 1.4 * scale, ink);
        }
    }
}

/// Réduit un texte très long : la description d'un jeu peut faire des dizaines
/// de milliers de caractères, la section n'a pas à en peindre autant.
fn ellipsize_text(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use lapinux_core::model::{SteamDlcRef, SteamRarity};

    fn shelf_status(
        info: Option<&SteamGameInfo>,
        error: Option<&str>,
        loading: bool,
    ) -> Vec<String> {
        let ctx = egui::Context::default();
        crate::install_fonts(&ctx);
        let mut texts = Vec::new();
        let output = ctx.run(
            egui::RawInput {
                // Une fenêtre haute : la section est longue, et egui écarte
                // du dessin ce qui tombe hors du cadre.
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(700.0, 3000.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    status_row(ui, info, error, loading);
                    body(ui, info, 1017900);
                });
            },
        );
        fn walk(shape: &egui::Shape, texts: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(text) => texts.push(text.galley.text().to_string()),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, texts);
                    }
                }
                _ => {}
            }
        }
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut texts);
        }
        texts
    }

    /// Fiche complète d'essai : un jeu possédé, avec une mise à jour en attente.
    fn sample() -> SteamGameInfo {
        SteamGameInfo {
            appid: 1017900,
            name: "APICO".into(),
            fetched: 1_700_000_000,
            sources: vec!["protocole".into(), "compte".into(), "boutique".into()],
            notes: vec!["boutique : page indisponible".into()],
            account: SteamAccountPlay {
                owned: true,
                minutes_total: Some(600),
                minutes_2weeks: Some(45),
                minutes_linux: Some(600),
                last_played: Some(1_700_000_000),
                level: Some(42),
                badge_level: Some(5),
                players_now: Some(1234),
                achievements_unlocked: Some(3),
                achievements_total: Some(4),
                last_achievement: "Première récolte".into(),
                last_achievement_time: Some(1_699_000_000),
                rarest: vec![SteamRarity {
                    name: "Perfection".into(),
                    percent: 1.2,
                    unlocked: false,
                }],
                ..Default::default()
            },
            update: SteamUpdateInfo {
                app_type: "game".into(),
                release_state: "released".into(),
                public_build: Some(1600),
                public_build_time: Some(1_700_000_000),
                installed_build: Some(1500),
                installed_build_time: Some(1_690_000_000),
                branches: vec![SteamBranch {
                    name: "public".into(),
                    build: Some(1600),
                    time: Some(1_700_000_000),
                    description: String::new(),
                }],
                depots: vec![SteamDepot {
                    id: 228990,
                    os: vec!["Linux".into()],
                    language: "Anglais".into(),
                    download: 900_000_000,
                    install: 1_200_000_000,
                    shared: false,
                    dlc_appid: None,
                }],
                download: 900_000_000,
                install: 1_200_000_000,
                cloud_quota: Some(95_367_168),
                cloud_files: Some(1000),
                ratings: vec!["PEGI 3".into()],
                dlc: vec![
                    SteamDlcRef {
                        appid: 1017901,
                        name: "APICO Soundtrack".into(),
                        owned: true,
                        download: 120_000_000,
                        install: 150_000_000,
                        image: String::new(),
                    },
                    SteamDlcRef {
                        appid: 1017902,
                        name: "APICO Artbook".into(),
                        download: 40_000_000,
                        install: 45_000_000,
                        ..Default::default()
                    },
                ],
                ..Default::default()
            },
            store: SteamStorePage {
                short_description: "Un jeu de gestion d'abeilles.".into(),
                description: "Posez des ruches et cultivez vos fleurs.".into(),
                price: "16,79 €".into(),
                review_summary: "Très positive".into(),
                review_count: Some(2_432),
                recommendations: Some(3_000),
                screenshots: vec!["https://cdn.example/1.jpg".into()],
                website: "https://apico.example".into(),
                requirements_min: "Système : Linux".into(),
                ..Default::default()
            },
            stats: vec![SteamStat {
                name: "total_flowers".into(),
                display: "Fleurs plantées".into(),
                value: "1204".into(),
            }],
        }
    }

    #[test]
    fn the_shelf_forgets_between_games() {
        let mut shelf = Shelf::default();
        shelf.loaded_for = Some(400);
        shelf.data = Some(sample());
        shelf.loading = true;
        shelf.forget();
        assert!(shelf.loaded_for.is_none());
        assert!(shelf.data.is_none());
        assert!(!shelf.loading);
        assert!(shelf.data.is_none(), "la fiche suivante sera relue");
    }

    #[test]
    fn a_loaded_shelf_keeps_the_game_it_was_asked_for() {
        let mut shelf = Shelf::default();
        shelf.loaded_for = Some(1017900);
        shelf.data = Some(sample());
        assert_eq!(shelf.data.as_ref().map(|info| info.appid), Some(1017900));
        // L'interface ne redemande pas ce qu'elle a déjà : `ensure` sort ici
        // sans rien relancer, ce que la garde de `start` vérifie.
        assert!(shelf.data.is_some() && shelf.loaded_for == Some(1017900));
    }

    #[test]
    fn the_empty_section_explains_the_background_read() {
        let texts = shelf_status(None, None, true);
        assert!(
            texts.iter().any(|text| text.contains("Lecture des détails Steam")),
            "{texts:?}"
        );
    }

    #[test]
    fn a_failed_read_shows_its_reason_and_the_reload_button() {
        let texts = shelf_status(None, Some("session Steam expirée"), false);
        assert!(texts.iter().any(|text| text == "session Steam expirée"), "{texts:?}");
        assert!(texts.iter().any(|text| text == "Relire"), "{texts:?}");
    }

    #[test]
    fn the_account_group_paints_playtime_badge_and_rarest_success() {
        let info = sample();
        let texts = shelf_status(Some(&info), None, false);
        let joined = texts.join("\n");
        assert!(joined.contains("Mon compte"), "{joined}");
        assert!(joined.contains("Temps de jeu"), "{joined}");
        assert!(joined.contains("10 h 00"), "600 minutes : {joined}");
        assert!(joined.contains("Niveau Steam\n42"), "niveau : {joined}");
        assert!(joined.contains("niveau 5"), "badge du jeu : {joined}");
        assert!(joined.contains("1 234"), "joueurs en ligne : {joined}");
        assert!(joined.contains("3 / 4 · 75,0 %"), "succès du compte : {joined}");
        assert!(
            joined.contains("Perfection · 1,2 % · manquant"),
            "succès rare : {joined}"
        );
    }

    /// Rectangles d'une teinte donnée qu'un encart peint : les cartes (`CARD()`)
    /// ou les images en attente (`CARD_HOVER()`).
    fn embeds_rects_of(
        info: SteamGameInfo,
        color: egui::Color32,
        draw: impl Fn(&mut Shelf, &mut egui::Ui),
    ) -> Vec<egui::Rect> {
        embeds_rects_of_width(560.0, info, color, draw)
    }

    fn embeds_rects_of_width(
        width: f32,
        info: SteamGameInfo,
        color: egui::Color32,
        draw: impl Fn(&mut Shelf, &mut egui::Ui),
    ) -> Vec<egui::Rect> {
        let ctx = egui::Context::default();
        crate::install_fonts(&ctx);
        let mut shelf = Shelf {
            loaded_for: Some(info.appid),
            data: Some(info),
            ..Default::default()
        };
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 900.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(width, 0.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_width(width);
                            draw(&mut shelf, ui);
                        },
                    );
                });
            },
        );
        fn walk(shape: &egui::Shape, color: egui::Color32, frames: &mut Vec<egui::Rect>) {
            match shape {
                egui::Shape::Rect(rect) if rect.fill == color => frames.push(rect.rect),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, color, frames);
                    }
                }
                _ => {}
            }
        }
        let mut frames = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, color, &mut frames);
        }
        frames
    }

    /// La description du jeu n'est pas un encart : elle se lit à même la fiche,
    /// sans titre et sans cadre. Les configurations ont leur onglet.
    #[test]
    fn the_description_reads_without_a_title_or_a_frame() {
        let info = sample();
        let joined = embeds(560.0, info.clone(), |shelf, ui| {
            shelf.description(ui, 1017900);
        })
        .join("\n");
        assert!(
            !joined.contains("À propos"),
            "le titre ne doit plus être peint : {joined}"
        );
        assert!(
            joined.contains("Posez des ruches et cultivez vos fleurs."),
            "{joined}"
        );
        // Le résumé court vit dans son propre encart, au-dessus : il n'est
        // pas répété ici.
        assert!(
            !joined.contains("Un jeu de gestion d'abeilles."),
            "le résumé est répété : {joined}"
        );
        assert!(
            embeds_rects_of(info, CARD(), |shelf, ui| {
                shelf.description(ui, 1017900);
            })
            .is_empty(),
            "la description ne doit plus porter de cadre"
        );
    }

    /// Les configurations minimale et recommandée vivent dans l'onglet
    /// Détails, pas sous la description.
    #[test]
    fn the_requirements_live_in_the_details_tab() {
        let info = sample();
        let joined = embeds(560.0, info, |shelf, ui| {
            assert!(shelf.requirements(ui, 1017900));
        })
        .join("\n");
        // Le titre du repli suffit : son contenu ne se peint qu'une fois
        // ouvert, comme dans la fiche.
        assert!(joined.contains("Configuration minimale"), "{joined}");
    }

    #[test]
    fn animated_description_media_is_recognized() {
        assert!(animated_description_image("https://cdn/a.gif?t=1"));
        assert!(animated_description_image("https://cdn/a.GIF"));
        assert!(animated_description_image("https://cdn/a.webm"));
        assert!(animated_description_image("https://cdn/a.mp4?t=1"));
        assert!(!animated_description_image("https://cdn/a.webp"));
        assert!(!animated_description_image("https://cdn/a.avif"));
    }

    /// Les visuels que la boutique place dans la description suivent le texte,
    /// dans la même colonne : une image par adresse, à la largeur du texte.
    #[test]
    fn the_description_paints_the_store_images_after_the_text() {
        let mut info = sample();
        info.store.description_images = vec![
            "https://cdn.example/desc-1.jpg".to_string(),
            "https://cdn.example/desc-2.jpg".to_string(),
        ];
        let rects = embeds_rects_of(info.clone(), CARD_HOVER(), |shelf, ui| {
            shelf.description(ui, 1017900);
        });
        assert_eq!(rects.len(), 2, "deux images attendues : {rects:?}");
        let text = embeds(560.0, info, |shelf, ui| {
            shelf.description(ui, 1017900);
        });
        assert!(
            text.join("\n")
                .contains("Posez des ruches et cultivez vos fleurs."),
            "le texte de la description est peint : {text:?}"
        );
        // Pile dans l'ordre de la liste, à la largeur du texte, et sans
        // déborder de la colonne.
        assert!(
            rects[0].bottom() <= rects[1].top(),
            "images à la suite : {rects:?}"
        );
        for rect in &rects {
            assert!((rect.width() - 560.0).abs() < 1.0, "largeur : {rect:?}");
        }
    }

    /// Une description découpée peint chaque texte avant le visuel qui le
    /// suit et après celui qui le précède, au lieu de les empiler à la fin.
    #[test]
    fn the_description_keeps_text_between_its_media() {
        let mut info = sample();
        let first = "https://cdn.example/desc-1.jpg".to_string();
        let second = "https://cdn.example/desc-2.jpg".to_string();
        info.store.description.clear();
        info.store.description_images = vec![first.clone(), second.clone()];
        info.store.description_blocks = vec![
            DescriptionBlock::Text("Avant l'image.".into()),
            DescriptionBlock::Media(first),
            DescriptionBlock::Text("Entre les images.".into()),
            DescriptionBlock::Media(second),
            DescriptionBlock::Text("Après la seconde.".into()),
        ];
        let rects = embeds_rects_of(info.clone(), CARD_HOVER(), |shelf, ui| {
            shelf.description(ui, 1017900);
        });
        assert_eq!(rects.len(), 2, "deux visuels attendus : {rects:?}");
        let texts = embeds_rects(560.0, info, |shelf, ui| {
            shelf.description(ui, 1017900);
        });
        let at = |needle: &str| {
            texts
                .iter()
                .find(|(text, _)| text.starts_with(needle))
                .map(|(_, rect)| *rect)
                .unwrap_or_else(|| panic!("{needle} manque : {texts:?}"))
        };
        let before = at("Avant l'image.");
        let between = at("Entre les images.");
        let after = at("Après la seconde.");
        assert!(before.bottom() <= rects[0].top(), "ordre avant : {before:?} / {rects:?}");
        assert!(rects[0].bottom() <= between.top(), "ordre milieu : {rects:?} / {between:?}");
        assert!(between.bottom() <= rects[1].top(), "ordre entre : {between:?} / {rects:?}");
        assert!(rects[1].bottom() <= after.top(), "ordre après : {rects:?} / {after:?}");
        let column_center = rects[0].center().x;
        for text in [before, between, after] {
            assert!(
                (text.center().x - column_center).abs() < 2.0,
                "paragraphe non centré dans la colonne : {text:?} / {column_center}"
            );
        }
    }

    /// La description ne s'étire pas sur toute une fiche large : elle garde
    /// une largeur de lecture et reste centrée, texte comme visuels.
    #[test]
    fn the_description_is_centered_on_a_wide_sheet() {
        let mut info = sample();
        let image = "https://cdn.example/wide.jpg".to_string();
        info.store.description.clear();
        info.store.description_images = vec![image.clone()];
        info.store.description_blocks = vec![
            DescriptionBlock::Text("Un paragraphe centré sur la fiche.".into()),
            DescriptionBlock::Media(image),
        ];
        let rects = embeds_rects_of_width(1188.0, info.clone(), CARD_HOVER(), |shelf, ui| {
            shelf.description(ui, 1017900);
        });
        assert_eq!(rects.len(), 1, "un visuel attendu : {rects:?}");
        assert!(
            (rects[0].width() - 860.0).abs() < 1.0,
            "largeur de lecture : {rects:?}"
        );
        let texts = embeds_rects(1188.0, info, |shelf, ui| {
            shelf.description(ui, 1017900);
        });
        let text = texts
            .iter()
            .find(|(text, _)| text.starts_with("Un paragraphe"))
            .map(|(_, rect)| *rect)
            .expect("paragraphe");
        assert!(
            (text.center().x - rects[0].center().x).abs() < 2.0,
            "texte non centré : {text:?} / {rects:?}"
        );
    }

    /// Un texte long se replie en plusieurs lignes, et chaque ligne garde le
    /// même centre : c'est ce qui évite qu'un paragraphe parte dans la colonne
    /// des actualités tout en ayant un titre centré.
    #[test]
    fn the_description_centres_each_wrapped_line() {
        let mut info = sample();
        let image = "https://cdn.example/wrapped.jpg".to_string();
        info.store.description.clear();
        info.store.description_images = vec![image.clone()];
        info.store.description_blocks = vec![
            DescriptionBlock::Text(
                "Une phrase assez longue pour se replier plusieurs fois dans la colonne centrale de la fiche. "
                    .repeat(4),
            ),
            DescriptionBlock::Media(image),
        ];
        let images = embeds_rects_of_width(660.0, info.clone(), CARD_HOVER(), |shelf, ui| {
            shelf.description(ui, 1017900);
        });
        assert_eq!(images.len(), 1, "un visuel attendu : {images:?}");
        let lines = embeds_rects(660.0, info, |shelf, ui| {
            shelf.description(ui, 1017900);
        });
        let body: Vec<_> = lines
            .into_iter()
            .filter(|(text, _)| !text.starts_with("À propos"))
            .collect();
        assert!(body.len() > 1, "texte replié attendu : {body:?}");
        let center = images[0].center().x;
        for (text, rect) in body {
            assert!(
                (rect.center().x - center).abs() < 2.0,
                "ligne non centrée ({text}) : {rect:?} / {center}"
            );
        }
    }

    #[test]
    fn the_store_group_paints_price_reviews_and_requirements() {
        let info = sample();
        let joined = shelf_status(Some(&info), None, false).join("\n");
        assert!(joined.contains("Boutique"), "{joined}");
        assert!(joined.contains("16,79 €"), "{joined}");
        assert!(joined.contains("Très positive · 2 432 avis"), "{joined}");
        assert!(joined.contains("Un jeu de gestion d'abeilles."), "{joined}");
        assert!(
            joined.contains("https://store.steampowered.com/app/1017900"),
            "{joined}"
        );
    }

    #[test]
    fn the_review_count_falls_back_to_the_recommendations_total() {
        let mut info = sample();
        info.store.review_count = None;
        info.store.review_summary.clear();
        info.store.recommendations = Some(14_410);
        let joined = shelf_status(Some(&info), None, false).join("\n");
        assert!(joined.contains("14 410 avis"), "{joined}");
    }

    #[test]
    fn the_mute_sources_are_listed() {
        let info = sample();
        let joined = shelf_status(Some(&info), None, false).join("\n");
        assert!(joined.contains("Sources muettes"), "{joined}");
        assert!(joined.contains("boutique : page indisponible"), "{joined}");
    }

    #[test]
    fn a_game_without_a_synthetic_section_says_so() {
        let mut info = sample();
        info.sources.clear();
        info.account = SteamAccountPlay::default();
        info.update = SteamUpdateInfo::default();
        info.store = SteamStorePage::default();
        info.stats.clear();
        info.notes.clear();
        let joined = shelf_status(Some(&info), None, false).join("\n");
        assert!(joined.contains("Aucune source n'a répondu"), "{joined}");
    }

    #[test]
    fn numbers_and_percentages_read_french() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_234), "1 234");
        assert_eq!(thousands(12_345_678), "12 345 678");
        assert_eq!(percent(75.0), "75,0");
        assert_eq!(percent(1.26), "1,3");
        assert_eq!(plural(0, "publié", "publiés"), "0 publié");
        assert_eq!(plural(1, "publié", "publiés"), "1 publié");
        assert_eq!(plural(2, "publié", "publiés"), "2 publiés");
    }

    #[test]
    fn a_huge_description_is_cut_before_painting() {
        let text = "a".repeat(5000);
        let cut = ellipsize_text(&text, 100);
        assert_eq!(cut.chars().count(), 101, "cent caractères et l'ellipse");
        assert!(cut.ends_with('…'));
        assert_eq!(ellipsize_text("court", 100), "court");
    }

    /// La description n'est pas un texte replié : sa fin se lit aussi, même
    /// au-delà de l'ancienne limite de 4 000 caractères.
    #[test]
    fn the_whole_description_is_painted() {
        let mut info = sample();
        info.store.description = format!(
            "Début de la présentation. {} Fin de la présentation.",
            "mot ".repeat(1200)
        );
        let joined = embeds(560.0, info, |shelf, ui| {
            shelf.description(ui, 1017900);
        })
        .join("\n");
        assert!(
            joined.contains("Fin de la présentation."),
            "la fin de la description manque"
        );
    }

    /// Les titres Markdown de la vitrine Epic ne gardent pas leurs dièses et
    /// sont peints plus grands que le corps de la description.
    #[test]
    fn markdown_headings_are_painted_as_headings() {
        let mut info = sample();
        info.store.description = "# Danger has escalated\nUn paragraphe.".into();
        info.store.description_blocks.clear();
        info.store.description_images.clear();
        let rects = embeds_rects(560.0, info, |shelf, ui| {
            shelf.description(ui, 1017900);
        });
        let heading = rects
            .iter()
            .find(|(text, _)| text == "Danger has escalated")
            .map(|(_, rect)| *rect)
            .expect("titre peint");
        let body = rects
            .iter()
            .find(|(text, _)| text == "Un paragraphe.")
            .map(|(_, rect)| *rect)
            .expect("paragraphe peint");
        assert!(
            heading.height() > body.height(),
            "titre {heading:?}, corps {body:?}"
        );
        assert!(
            rects.iter().all(|(text, _)| !text.contains('#')),
            "dièses encore peints : {rects:?}"
        );
    }

    /// Peint un encart seul, dans une colonne de la largeur demandée, et rend
    /// les textes réellement dessinés.
    fn embeds(
        width: f32,
        info: SteamGameInfo,
        draw: impl Fn(&mut Shelf, &mut egui::Ui),
    ) -> Vec<String> {
        embeds_rects(width, info, draw)
            .into_iter()
            .map(|(text, _)| text)
            .collect()
    }

    /// Les mêmes textes, avec le rectangle où chacun a été peint : la place
    /// des cartes se vérifie là, pas dans leur contenu.
    fn embeds_rects(
        width: f32,
        info: SteamGameInfo,
        draw: impl Fn(&mut Shelf, &mut egui::Ui),
    ) -> Vec<(String, egui::Rect)> {
        let ctx = egui::Context::default();
        crate::install_fonts(&ctx);
        let mut shelf = Shelf {
            loaded_for: Some(info.appid),
            data: Some(info),
            ..Default::default()
        };
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width + 100.0, 1400.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(width, 0.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_width(width);
                            draw(&mut shelf, ui);
                        },
                    );
                });
            },
        );
        let mut texts = Vec::new();
        fn walk(shape: &egui::Shape, texts: &mut Vec<(String, egui::Rect)>) {
            match shape {
                egui::Shape::Text(text) => texts.push((
                    text.galley.text().to_string(),
                    egui::Rect::from_min_size(text.pos, text.galley.size()),
                )),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, texts);
                    }
                }
                _ => {}
            }
        }
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut texts);
        }
        texts
    }

    /// L'encart des actualités porte le titre, le début du texte et la date de
    /// chaque annonce de l'éditeur.
    #[test]
    fn the_news_embed_paints_the_editor_announcements() {
        let mut info = sample();
        info.store.news = vec![
            SteamNews {
                body: String::new(),
                title: "Mise à jour 1.2".into(),
                excerpt: "Le mode coop arrive.".into(),
                date: 1_793_032_250,
                author: "studio".into(),
                url: "https://exemple/1".into(),
                image: String::new(),
            },
            SteamNews {
                body: String::new(),
                title: "Nouvelle carte".into(),
                excerpt: "Une carte offerte.".into(),
                date: 1_792_000_000,
                author: String::new(),
                url: "https://exemple/2".into(),
                image: String::new(),
            },
        ];
        let texts = embeds(760.0, info, |shelf, ui| {
            assert!(shelf.news_embed(ui, 1017900, None), "l'encart doit se peindre");
        });
        let joined = texts.join("\n");
        assert!(joined.contains("Actualités"), "{joined}");
        assert!(joined.contains("2 annonces"), "{joined}");
        assert!(joined.contains("Mise à jour 1.2"), "{joined}");
        assert!(joined.contains("Le mode coop arrive."), "{joined}");
        assert!(joined.contains("Nouvelle carte"), "{joined}");
        assert!(
            joined.contains(&lapinux_core::util::format_date(1_793_032_250)),
            "la date de publication manque : {joined}"
        );
        // L'auteur accompagne la date quand Steam le publie.
        assert!(joined.contains("· studio"), "{joined}");
    }

    #[test]
    fn a_game_without_announcements_shows_no_news_embed() {
        let info = sample();
        assert!(info.store.news.is_empty(), "la fiche d'essai n'en publie pas");
        let texts = embeds(760.0, info, |shelf, ui| {
            assert!(!shelf.news_embed(ui, 1017900, None), "rien à peindre");
        });
        assert!(texts.is_empty(), "{texts:?}");
    }

    /// L'encart des fonctionnalités liste tout ce que la boutique annonce.
    /// La colonne de la fiche ne fait que la hauteur de la fiche : les lignes
    /// qui n'y tiennent pas sont annoncées, pas coupées sous le bord.
    /// La hauteur de la colonne décide du nombre de lignes : cinq dans la
    /// fiche repliée, toutes dans la dépliée.
    /// L'encart du résumé porte le texte court de la boutique ; sans texte ni
    /// capture, la carte reste et annonce « Aucun résumé ».
    #[test]
    fn the_summary_embed_reads_the_store_blurb() {
        let mut info = sample();
        info.store.short_description = "Un jeu de gestion d'abeilles.".into();
        info.store.screenshots.clear();
        let texts = embeds(320.0, info.clone(), |shelf, ui| {
            shelf.summary_embed(ui, 1017900, 320.0, 200.0, None);
        });
        let joined = texts.join("\n");
        assert!(joined.contains("Résumé"), "{joined}");
        assert!(joined.contains("gestion d'abeilles"), "{joined}");
        // Une fiche sans résumé ni capture garde son encart et le dit : la
        // carte ne disparaît pas de la ligne des trois encarts.
        info.store.short_description.clear();
        let texts = embeds(320.0, info, |shelf, ui| {
            shelf.summary_embed(ui, 1017900, 320.0, 200.0, None);
        });
        let joined = texts.join("\n");
        assert!(joined.contains("Aucun résumé"), "{joined}");
    }

    /// Un clic sur une capture l'ouvre dans le panneau fixe de l'application,
    /// et Échap le referme : le navigateur n'a plus à s'ouvrir pour une image.
    #[test]
    fn a_capture_opens_in_the_fixed_panel() {
        let ctx = egui::Context::default();
        crate::install_fonts(&ctx);
        let mut shelf = Shelf {
            loaded_for: Some(1017900),
            data: Some(sample()),
            ..Default::default()
        };
        shelf.show_photo(
            vec![
                Photo::new("https://cdn/shot-1.jpg", "Capture du jeu"),
                Photo::new("https://cdn/shot-2.jpg", "Capture du jeu"),
            ],
            0,
        );
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        fn arrow(key: egui::Key) -> egui::Event {
            egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }
        }
        fn walk(shape: &egui::Shape, out: &mut Vec<String>, images: &mut usize) {
            match shape {
                egui::Shape::Text(text) => out.push(text.galley.text().to_string()),
                egui::Shape::Mesh(mesh) if mesh.texture_id != egui::TextureId::Managed(0) => {
                    *images += 1;
                }
                // La grande image, que la texture soit arrivée ou non. Le fond
                // plein écran est aussi un rectangle : il est plus large que
                // l'image et ne doit pas être compté comme elle.
                egui::Shape::Rect(rect)
                    if rect.rect.width() > 900.0
                        && rect.rect.width() < 1180.0
                        && rect.rect.height() > 400.0 =>
                {
                    *images += 1;
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, out, images);
                    }
                }
                _ => {}
            }
        }
        fn run(
            ctx: &egui::Context,
            shelf: &mut Shelf,
            screen: egui::Rect,
            events: Vec<egui::Event>,
        ) -> (Vec<String>, usize) {
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..Default::default()
                },
                |ctx| shelf.photo_panel(ctx, screen),
            );
            let (mut texts, mut images) = (Vec::new(), 0);
            for clipped in &output.shapes {
                walk(&clipped.shape, &mut texts, &mut images);
            }
            (texts, images)
        }
        run(&ctx, &mut shelf, screen, Vec::new());
        let (texts, images) = run(&ctx, &mut shelf, screen, Vec::new());
        assert!(texts.iter().any(|text| text == "Capture"), "{texts:?}");
        assert!(
            texts.iter().any(|text| text == "Capture du jeu · 1 / 2"),
            "{texts:?}"
        );
        assert_eq!(images, 1, "l'image occupe le panneau");
        let shown = |shelf: &Shelf| shelf.photo.as_ref().map(|gallery| gallery.index);
        assert_eq!(shown(&shelf), Some(0));
        // La flèche de droite passe à la suivante, et la série boucle.
        let (texts, _) = run(&ctx, &mut shelf, screen, vec![arrow(egui::Key::ArrowRight)]);
        assert_eq!(shown(&shelf), Some(1));
        assert!(
            texts.iter().any(|text| text == "Capture du jeu · 2 / 2"),
            "{texts:?}"
        );
        let (texts, _) = run(&ctx, &mut shelf, screen, vec![arrow(egui::Key::ArrowRight)]);
        assert_eq!(shown(&shelf), Some(0), "la série boucle");
        assert!(
            texts.iter().any(|text| text == "Capture du jeu · 1 / 2"),
            "{texts:?}"
        );
        // La flèche de gauche recule, depuis le début.
        run(&ctx, &mut shelf, screen, vec![arrow(egui::Key::ArrowLeft)]);
        assert_eq!(shown(&shelf), Some(1));
        run(&ctx, &mut shelf, screen, vec![arrow(egui::Key::Escape)]);
        assert!(shelf.photo.is_none(), "Échap referme l'image");
    }

    /// Le résumé se range sous son titre, même peint dans la ligne compacte
    /// qui est horizontale : le défaut corrigé posait le texte sur le titre.
    #[test]
    fn the_summary_stacks_under_its_title_inside_the_compact_row() {
        let ctx = egui::Context::default();
        crate::install_fonts(&ctx);
        let mut shelf = Shelf {
            loaded_for: Some(1017900),
            data: Some(sample()),
            ..Default::default()
        };
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 600.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    // La ligne compacte est horizontale : c'est là que
                    // l'encart se peint, et c'est ce qui le cassait.
                    ui.horizontal_top(|ui| {
                        shelf.summary_embed(ui, 1017900, 320.0, 192.0, None);
                    });
                });
            },
        );
        let mut painted: Vec<(String, egui::Rect)> = Vec::new();
        fn walk(shape: &egui::Shape, painted: &mut Vec<(String, egui::Rect)>) {
            match shape {
                egui::Shape::Text(text) => painted.push((
                    text.galley.text().to_string(),
                    egui::Rect::from_min_size(text.pos, text.galley.size()),
                )),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, painted);
                    }
                }
                _ => {}
            }
        }
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut painted);
        }
        let rect_of = |needle: &str| {
            painted
                .iter()
                .find(|(text, _)| text.contains(needle))
                .map(|(_, rect)| *rect)
                .unwrap_or_else(|| panic!("{needle} manque : {painted:?}"))
        };
        let title = rect_of("Résumé");
        let body = rect_of("gestion d'abeilles");
        assert!(
            body.top() >= title.bottom(),
            "le texte passe sur le titre : {body:?} / {title:?}"
        );
        // Le titre est centré sur l'encart ; le texte, lui, garde la largeur
        // de la colonne. Le centre se calcule sur la même marge intérieure que
        // le texte : 20 px de cadre retirés aux 320 px de la carte.
        let card_center = body.left() + (320.0 - 20.0) / 2.0;
        assert!(
            (title.center().x - card_center).abs() < 12.0,
            "le titre est centré sur l'encart : {title:?} / {card_center}"
        );
        assert!(body.right() <= 320.0, "le texte déborde : {body:?}");
        // Le texte reste dans la hauteur de l'encart : un débordement
        // poussait la colonne des succès vers le bas.
        assert!(body.bottom() <= 192.0, "le texte déborde en bas : {body:?}");
    }

    /// L'encart du résumé occupe exactement la hauteur qu'on lui donne : la
    /// ligne compacte lui passe celle de la carte du jeu, donc les deux cartes
    /// finissent ensemble au lieu que le résumé touche la bande de tags.
    #[test]
    fn the_summary_card_fills_the_height_it_is_given() {
        for height in [170.0_f32, 192.0] {
            let ctx = egui::Context::default();
            crate::install_fonts(&ctx);
            let mut shelf = Shelf {
                loaded_for: Some(1017900),
                data: Some(sample()),
                ..Default::default()
            };
            let mut painted = 0.0_f32;
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900.0, 700.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.horizontal_top(|ui| {
                            let top = ui.cursor().top();
                            shelf.summary_embed(ui, 1017900, 320.0, height, None);
                            painted = ui.min_rect().bottom() - top;
                        });
                    });
                },
            );
            assert!(
                (painted - height).abs() < 1.0,
                "l'encart fait {painted} px au lieu de {height}"
            );
        }
    }

    /// Le petit « i » n'apparaît que lorsque les annonces viennent de Steam.
    #[test]
    fn a_steam_news_origin_is_kept_for_the_info_badge() {
        let details = lapinux_core::stores::StoreDetails {
            news_from_steam: true,
            ..Default::default()
        };
        let content = SheetContent::from_store_details(Some(&details));
        assert!(content.news_from_steam);
    }

    /// L'accroche de la boutique et le début d'« À propos » sont deux textes :
    /// le résumé ne doit pas voler l'introduction de la présentation longue.
    #[test]
    fn a_store_summary_and_the_about_keep_their_own_text() {
        let details = lapinux_core::stores::StoreDetails {
            store: lapinux_core::stores::Store::Epic,
            summary: "Accroche courte de la boutique.".into(),
            description: "Début de la présentation longue.\n# Titre\nCorps.".into(),
            ..Default::default()
        };
        let content = SheetContent::from_store_details(Some(&details));
        assert_eq!(content.short_description, "Accroche courte de la boutique.");
        assert_eq!(
            content.description,
            "Début de la présentation longue.\n# Titre\nCorps."
        );
    }

    /// Une présentation de boutique passe par le même encart que Steam :
    /// c'est ce qui rend les fiches identiques d'une boutique à l'autre.
    #[test]
    fn a_store_summary_paints_with_the_steam_card() {
        let details = lapinux_core::stores::StoreDetails {
            store: lapinux_core::stores::Store::Gog,
            description: "Un jeu de survie coopératif où quatre survivants affrontent des hordes."
                .into(),
            screenshots: vec!["https://cdn.example/1.jpg".into(), "https://cdn.example/2.jpg".into()],
            news: vec![lapinux_core::stores::StoreNews {
                title: "Patch 2.5".into(),
                excerpt: "Notes de mise à jour.".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let content = SheetContent::from_store_details(Some(&details));
        // Sans accroche séparée (ancienne copie), la première ligne sert de
        // résumé et la présentation reste entière sous « À propos ».
        assert!(content.short_description.starts_with("Un jeu de survie"));
        assert_eq!(content.description, details.description);
        assert_eq!(content.news.len(), 1);
        let ctx = egui::Context::default();
        crate::install_fonts(&ctx);
        let mut shelf = Shelf::default();
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 600.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.horizontal_top(|ui| {
                        shelf.summary_page(ui, Some(&content), false, None, 320.0, 170.0, None);
                    });
                });
            },
        );
        let texts: Vec<String> = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) => Some(text.galley.job.text.to_string()),
                _ => None,
            })
            .collect();
        assert!(
            texts.iter().any(|text| text.starts_with("Un jeu de survie")),
            "résumé de la boutique absent : {texts:?}"
        );
        assert!(texts.iter().any(|text| text == "Résumé"), "{texts:?}");
    }

    /// Peint l'encart du résumé dans une ligne horizontale, comme la fiche
    /// repliée, et rend ses textes, ses rectangles de vignette et le nombre
    /// d'images peintes.
    fn summary_paint(
        info: Option<SteamGameInfo>,
        loading: bool,
        with_bunny: bool,
        height: f32,
    ) -> (Vec<(String, egui::Rect)>, Vec<egui::Rect>, usize) {
        let ctx = egui::Context::default();
        crate::install_fonts(&ctx);
        let bunny = with_bunny
            .then(|| crate::launch::Bunny::load(&ctx))
            .flatten();
        let mut shelf = Shelf {
            loaded_for: Some(1017900),
            loading,
            data: info,
            ..Default::default()
        };
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 600.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.horizontal_top(|ui| {
                        shelf.summary_embed(ui, 1017900, 320.0, height, bunny.as_ref());
                    });
                });
            },
        );
        fn walk(
            shape: &egui::Shape,
            texts: &mut Vec<(String, egui::Rect)>,
            rects: &mut Vec<egui::Rect>,
            images: &mut usize,
        ) {
            match shape {
                egui::Shape::Text(text) => texts.push((
                    text.galley.text().to_string(),
                    egui::Rect::from_min_size(text.pos, text.galley.size()),
                )),
                egui::Shape::Rect(rect) if rect.fill == CARD_HOVER() => rects.push(rect.rect),
                egui::Shape::Mesh(mesh) if mesh.texture_id != egui::TextureId::Managed(0) => {
                    *images += 1
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, texts, rects, images);
                    }
                }
                _ => {}
            }
        }
        let mut texts = Vec::new();
        let mut rects = Vec::new();
        let mut images = 0usize;
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut texts, &mut rects, &mut images);
        }
        (texts, rects, images)
    }

    /// Sous le texte du résumé, trois captures prennent la place qui reste :
    /// côte à côte, de même largeur, dans la hauteur de l'encart.
    #[test]
    fn the_summary_keeps_three_captures_below_its_text() {
        let mut info = sample();
        info.store.short_description =
            "Un jeu de survie coopératif où quatre survivants affrontent des hordes.".into();
        info.store.screenshots = vec![
            "https://cdn.example/1.jpg".into(),
            "https://cdn.example/2.jpg".into(),
            "https://cdn.example/3.jpg".into(),
        ];
        let (texts, rects, _) = summary_paint(Some(info), false, false, 170.0);
        assert_eq!(rects.len(), 3, "trois captures attendues : {rects:?}");
        let body = texts
            .iter()
            .find(|(text, _)| text.starts_with("Un jeu de survie"))
            .map(|(_, rect)| *rect)
            .expect("texte du résumé");
        let first = rects[0];
        assert!(
            first.top() > body.bottom(),
            "les captures passent sous le texte ({first:?} / {body:?})"
        );
        assert!(
            rects.iter()
                .all(|rect| (rect.width() - first.width()).abs() < 0.5),
            "vignettes de largeurs différentes : {rects:?}"
        );
        assert!(rects[1].left() >= rects[0].right(), "vignettes collées");
        assert!(rects[2].left() >= rects[1].right(), "vignettes collées");
        let bottom = rects.iter().map(|rect| rect.bottom()).fold(0.0_f32, f32::max);
        assert!(bottom <= 170.0, "les vignettes débordent : {bottom}");
    }

    /// Tant que la fiche n'est pas arrivée, l'encart existe quand même : le
    /// lapin y tourne, et la fiche ouvre bien ses trois cartes ensemble.
    #[test]
    fn the_empty_summary_card_waits_with_the_bunny() {
        let (texts, rects, images) = summary_paint(None, true, true, 170.0);
        assert_eq!(images, 1, "le lapin s'anime dans la carte");
        let label = texts
            .iter()
            .find(|(text, _)| text.contains("Lecture des détails du jeu"))
            .map(|(_, rect)| *rect)
            .expect("libellé d'attente");
        // Au centre de la carte (320 px de large, à 8 px du bord), pas dans un
        // coin : le lapin occupe le milieu et son libellé le suit.
        let card_center = 8.0 + 320.0 / 2.0;
        assert!(
            (label.center().x - card_center).abs() < 30.0,
            "libellé décalé : {label:?}"
        );
        // Sous le lapin, dans la hauteur de la carte.
        assert!(label.top() > 50.0 && label.bottom() <= 180.0, "{label:?}");
        // Plus de squelette : le centre de la carte porte le lapin.
        assert!(rects.is_empty(), "rectangles inattendus : {rects:?}");
        // Une fiche qui n'attend plus rien ne laisse pas une carte vide.
        let (texts, _, images) = summary_paint(None, false, false, 170.0);
        assert!(texts.is_empty() && images == 0, "{texts:?}");
    }

    /// Un libellé plus large que la colonne est coupé, jamais débordant.
    /// Les trois cartes se rangent côte à côte, chacune dans sa colonne : le
    /// défaut corrigé les peignait sur une seule ligne, débordante.
    #[test]
    fn the_news_cards_sit_side_by_side_inside_their_column() {
        let mut info = sample();
        info.store.news = (1..=3)
            .map(|index| SteamNews {
                body: String::new(),
                title: format!("Annonce {index}"),
                excerpt: "Un texte d'annonce assez long pour tenir sur quelques lignes de sa carte."
                    .into(),
                date: 1_793_000_000 + index,
                author: String::new(),
                url: format!("https://exemple/{index}"),
                image: String::new(),
            })
            .collect();
        let painted = embeds_rects(760.0, info, |shelf, ui| {
            shelf.news_embed(ui, 1017900, None);
        });
        let rect_of = |text: &str| {
            painted
                .iter()
                .find(|(painted, _)| painted == text)
                .map(|(_, rect)| *rect)
                .unwrap_or_else(|| panic!("{text} manque : {painted:?}"))
        };
        let (first, second, third) = (
            rect_of("Annonce 1"),
            rect_of("Annonce 2"),
            rect_of("Annonce 3"),
        );
        assert!(first.right() <= second.left(), "les cartes se chevauchent");
        assert!(second.right() <= third.left(), "les cartes se chevauchent");
        assert!((first.top() - third.top()).abs() < 1.0, "les cartes sont sur la même ligne");
        assert!(third.right() <= 760.0, "la carte déborde de la colonne : {third:?}");
        // L'aperçu descend sous le titre de sa carte au lieu de le suivre sur
        // la même ligne.
        let excerpt = painted
            .iter()
            .find(|(text, _)| text.starts_with("Un texte"))
            .map(|(_, rect)| *rect)
            .expect("l'aperçu manque");
        assert!(
            excerpt.top() >= first.bottom(),
            "l'aperçu passe sous le titre : {excerpt:?} / {first:?}"
        );
        assert!(excerpt.right() <= 760.0, "l'aperçu déborde : {excerpt:?}");
        // Une ligne de 10 points fait 13 pixels : deux lignes prouvent que le
        // texte revient à la ligne dans sa carte au lieu de s'étaler.
        assert!(
            excerpt.height() > 20.0,
            "l'aperçu doit revenir à la ligne : {excerpt:?}"
        );
    }

    /// L'aperçu d'une annonce se coupe sur un mot, à la place que lui laisse
    /// sa carte.
    #[test]
    fn an_excerpt_is_cut_to_its_four_lines() {
        assert_eq!(clamp_words("un deux trois", 7), "un…");
        assert_eq!(clamp_words("court", 20), "court");
        assert_eq!(clamp_words("Mise à jour", 11), "Mise à jour");
        assert!(clamp_words(&"mot ".repeat(80), 60).ends_with('…'));
    }

    /// Le pictogramme suit l'identifiant Steam, et le libellé pour les
    /// catégories que la boutique ajoute.
    #[test]
    fn the_feature_icons_follow_the_steam_category() {
        let icon = |id: u32, label: &str| {
            FeatureIcon::of(&SteamFeature {
                id,
                label: label.to_string(),
            })
        };
        assert_eq!(icon(2, "Solo"), FeatureIcon::Solo);
        assert_eq!(icon(1, "Multijoueur"), FeatureIcon::Duo);
        assert_eq!(icon(9, "Coopération"), FeatureIcon::Duo);
        assert_eq!(icon(49, "PvP"), FeatureIcon::Duo);
        assert_eq!(icon(23, "Steam Cloud"), FeatureIcon::Cloud);
        assert_eq!(icon(22, "Succès Steam"), FeatureIcon::Trophy);
        assert_eq!(icon(30, "Workshop Steam"), FeatureIcon::Wrench);
        assert_eq!(icon(28, "Compat. contrôleurs complète"), FeatureIcon::Pad);
        assert_eq!(icon(44, "Remote Play Together"), FeatureIcon::Screen);
        assert_eq!(icon(24, "Écran partagé"), FeatureIcon::Screen);
        assert_eq!(icon(62, "Partage familial"), FeatureIcon::Family);
        assert_eq!(icon(8, "Antitriche Valve activée"), FeatureIcon::Shield);
        assert_eq!(icon(35, "Achats en jeu"), FeatureIcon::Cart);
        assert_eq!(icon(65, "Options de sous-titres"), FeatureIcon::Options);
        // Une catégorie inconnue se lit dans son libellé.
        assert_eq!(icon(999, "Steam Cloud"), FeatureIcon::Cloud);
        assert_eq!(icon(999, "Atelier de niveaux"), FeatureIcon::Wrench);
        assert_eq!(icon(999, "Quelque chose"), FeatureIcon::Generic);
    }
}
