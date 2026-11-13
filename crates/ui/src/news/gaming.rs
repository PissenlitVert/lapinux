//! Section **Actualités › Actus du gaming** : les titres de la presse jeu
//! vidéo, lus dans une quarantaine de flux RSS publics. Les flux partent en
//! parallèle ; la page garde les titres déjà reçus et les images suivent en
//! tâche de fond.

use super::{
    columns, filter_choice, grid_card, grid_card_height_for, grid_card_width,
    skeleton_content_compact, source_menu, status_card, CardContent, MediaByline, GRID_GAP,
    NEWS_FILTER_WIDTH,
};
use crate::gameinfo::NewsArt;
use crate::{ACCENT, OK};
use eframe::egui;
use std::collections::HashSet;
use std::sync::mpsc;
use std::time::Duration;

/// Les sources proposées dans le menu de l'en-tête. Toutes ont été vérifiées
/// en RSS 2.0 : le parseur de la page n'a pas besoin d'un second format.
const FEEDS: [(&str, &str, &str); 50] = [
    (
        "Jeuxvideo.com",
        "https://www.jeuxvideo.com/rss/rss.xml",
        "https://www.google.com/s2/favicons?sz=64&domain=www.jeuxvideo.com",
    ),
    (
        "PC Gamer",
        "https://www.pcgamer.com/rss/",
        "https://www.google.com/s2/favicons?sz=64&domain=www.pcgamer.com",
    ),
    (
        "Rock Paper Shotgun",
        "https://www.rockpapershotgun.com/feed",
        "https://www.google.com/s2/favicons?sz=64&domain=www.rockpapershotgun.com",
    ),
    (
        "Gamekult",
        "https://www.gamekult.com/feed.xml",
        "https://www.google.com/s2/favicons?sz=64&domain=www.gamekult.com",
    ),
    (
        "NoFrag",
        "https://www.nofrag.com/feed/",
        "https://www.google.com/s2/favicons?sz=64&domain=www.nofrag.com",
    ),
    (
        "Canard PC",
        "https://www.canardpc.com/feed/",
        "https://www.google.com/s2/favicons?sz=64&domain=www.canardpc.com",
    ),
    (
        "VGC",
        "https://www.videogameschronicle.com/feed/",
        "https://www.google.com/s2/favicons?sz=64&domain=www.videogameschronicle.com",
    ),
    (
        "Eurogamer",
        "https://www.eurogamer.net/feed",
        "https://www.google.com/s2/favicons?sz=64&domain=www.eurogamer.net",
    ),
    (
        "Game Developer",
        "https://www.gamedeveloper.com/rss.xml",
        "https://www.google.com/s2/favicons?sz=64&domain=www.gamedeveloper.com",
    ),
    (
        "PCGamesN",
        "https://www.pcgamesn.com/feed",
        "https://www.google.com/s2/favicons?sz=64&domain=www.pcgamesn.com",
    ),
    (
        "Factornews",
        "https://www.factornews.com/rss.xml",
        "https://www.google.com/s2/favicons?sz=64&domain=www.factornews.com",
    ),
    (
        "ActuGaming",
        "https://www.actugaming.net/feed/",
        "https://www.google.com/s2/favicons?sz=64&domain=www.actugaming.net",
    ),
    (
        "IGN",
        "https://feeds.ign.com/ign/all",
        "https://www.google.com/s2/favicons?sz=64&domain=www.ign.com",
    ),
    (
        "Kotaku",
        "https://kotaku.com/rss",
        "https://www.google.com/s2/favicons?sz=64&domain=kotaku.com",
    ),
    (
        "Nintendo Life",
        "https://www.nintendolife.com/feeds/latest",
        "https://www.google.com/s2/favicons?sz=64&domain=www.nintendolife.com",
    ),
    (
        "Push Square",
        "https://www.pushsquare.com/feeds/latest",
        "https://www.google.com/s2/favicons?sz=64&domain=www.pushsquare.com",
    ),
    (
        "Gematsu",
        "https://www.gematsu.com/feed",
        "https://www.google.com/s2/favicons?sz=64&domain=www.gematsu.com",
    ),
    (
        "Game Rant",
        "https://gamerant.com/feed/",
        "https://www.google.com/s2/favicons?sz=64&domain=gamerant.com",
    ),
    (
        "DualShockers",
        "https://www.dualshockers.com/feed/",
        "https://www.google.com/s2/favicons?sz=64&domain=www.dualshockers.com",
    ),
    (
        "Siliconera",
        "https://www.siliconera.com/feed/",
        "https://www.google.com/s2/favicons?sz=64&domain=www.siliconera.com",
    ),
    (
        "VG247",
        "https://www.vg247.com/feed",
        "https://www.google.com/s2/favicons?sz=64&domain=www.vg247.com",
    ),
    (
        "Gameblog",
        "https://www.gameblog.fr/rss",
        "https://www.google.com/s2/favicons?sz=64&domain=www.gameblog.fr",
    ),
    (
        "Xboxygen",
        "https://www.xboxygen.com/feed/",
        "https://www.google.com/s2/favicons?sz=64&domain=www.xboxygen.com",
    ),
    (
        "PlayStation Blog",
        "https://blog.playstation.com/feed/",
        "https://www.google.com/s2/favicons?sz=64&domain=blog.playstation.com",
    ),
    (
        "Xbox Wire",
        "https://news.xbox.com/en-us/feed/",
        "https://www.google.com/s2/favicons?sz=64&domain=news.xbox.com",
    ),
    (
        "GamesRadar+",
        "https://www.gamesradar.com/rss/",
        "https://www.google.com/s2/favicons?sz=64&domain=www.gamesradar.com",
    ),
    (
        "Polygon",
        "https://www.polygon.com/rss/index.xml",
        "https://www.google.com/s2/favicons?sz=64&domain=www.polygon.com",
    ),
    (
        "GameSpot",
        "https://www.gamespot.com/feeds/mashup/",
        "https://www.google.com/s2/favicons?sz=64&domain=www.gamespot.com",
    ),
    (
        "Destructoid",
        "https://www.destructoid.com/feed/",
        "https://www.google.com/s2/favicons?sz=64&domain=www.destructoid.com",
    ),
    (
        "PC Invasion",
        "https://www.pcinvasion.com/feed/",
        "https://www.google.com/s2/favicons?sz=64&domain=www.pcinvasion.com",
    ),
    (
        "TheSixthAxis",
        "https://www.thesixthaxis.com/feed/",
        "https://www.google.com/s2/favicons?sz=64&domain=www.thesixthaxis.com",
    ),
    (
        "Pure Xbox",
        "https://www.purexbox.com/feeds/latest",
        "https://www.google.com/s2/favicons?sz=64&domain=www.purexbox.com",
    ),
    (
        "PSU",
        "https://www.psu.com/feed/",
        "https://www.google.com/s2/favicons?sz=64&domain=www.psu.com",
    ),
    (
        "PlayStation LifeStyle",
        "https://www.playstationlifestyle.net/feed/",
        "https://www.google.com/s2/favicons?sz=64&domain=www.playstationlifestyle.net",
    ),
    (
        "GamingBolt",
        "https://www.gamingbolt.com/feed",
        "https://www.google.com/s2/favicons?sz=64&domain=www.gamingbolt.com",
    ),
    (
        "Game Informer",
        "https://www.gameinformer.com/rss.xml",
        "https://www.google.com/s2/favicons?sz=64&domain=www.gameinformer.com",
    ),
    (
        "MCV/Develop",
        "https://www.mcvuk.com/feed/",
        "https://www.google.com/s2/favicons?sz=64&domain=www.mcvuk.com",
    ),
    (
        "TheGamer",
        "https://www.thegamer.com/feed/",
        "https://www.google.com/s2/favicons?sz=64&domain=www.thegamer.com",
    ),
    (
        "Gamergen",
        "https://www.gamergen.com/rss",
        "https://www.google.com/s2/favicons?sz=64&domain=www.gamergen.com",
    ),
    (
        "4WeAreGamers",
        "https://www.4wearegamers.com/feed/",
        "https://www.google.com/s2/favicons?sz=64&domain=www.4wearegamers.com",
    ),
    (
        "Mondes Persistants",
        "https://www.mondespersistants.com/feed/",
        "https://www.google.com/s2/favicons?sz=64&domain=www.mondespersistants.com",
    ),
    (
        "Jeuxvideo-Live",
        "https://www.jeuxvideo-live.com/feed/",
        "https://www.google.com/s2/favicons?sz=64&domain=www.jeuxvideo-live.com",
    ),
    (
        "Le Monde",
        "https://www.lemonde.fr/jeux-video/rss_full.xml",
        "https://www.google.com/s2/favicons?sz=64&domain=www.lemonde.fr",
    ),
    (
        "Clubic",
        "https://www.clubic.com/feed/news-jeux-video.rss",
        "https://www.google.com/s2/favicons?sz=64&domain=www.clubic.com",
    ),
    (
        "01net",
        "https://www.01net.com/actualites/jeux-video/rss/",
        "https://www.google.com/s2/favicons?sz=64&domain=www.01net.com",
    ),
    (
        "RPGFan",
        "https://www.rpgfan.com/feed/",
        "https://www.google.com/s2/favicons?sz=64&domain=www.rpgfan.com",
    ),
    (
        "The Outerhaven",
        "https://www.theouterhaven.net/feed/",
        "https://www.google.com/s2/favicons?sz=64&domain=www.theouterhaven.net",
    ),
    (
        "TechRadar Gaming",
        "https://www.techradar.com/feeds/tag/gaming",
        "https://www.google.com/s2/favicons?sz=64&domain=www.techradar.com",
    ),
    (
        "Nintendo Everything",
        "https://www.nintendoeverything.com/feed/",
        "https://www.google.com/s2/favicons?sz=64&domain=www.nintendoeverything.com",
    ),
    (
        "XboxEra",
        "https://www.xboxera.com/feed/",
        "https://www.google.com/s2/favicons?sz=64&domain=www.xboxera.com",
    ),
];

/// Sites dont la page d'accueil ne pose aucun cookie dans sa réponse HTTP
/// initiale (relevé sans navigateur le 10 novembre 2026). Des scripts peuvent
/// en poser ensuite : ce classement sert à mettre ces sources en tête, pas à
/// garantir leur politique de confidentialité.
const COOKIE_FREE_SOURCES: [&str; 27] = [
    "Gamekult",
    "VGC",
    "Game Developer",
    "PCGamesN",
    "ActuGaming",
    "Kotaku",
    "Gematsu",
    "Siliconera",
    "Gameblog",
    "Xboxygen",
    "PlayStation Blog",
    "Destructoid",
    "PC Invasion",
    "TheSixthAxis",
    "PSU",
    "PlayStation LifeStyle",
    "GamingBolt",
    "Game Informer",
    "MCV/Develop",
    "Mondes Persistants",
    "Jeuxvideo-Live",
    "Clubic",
    "01net",
    "RPGFan",
    "The Outerhaven",
    "Nintendo Everything",
    "XboxEra",
];

fn cookie_free(source: &str) -> bool {
    COOKIE_FREE_SOURCES.contains(&source)
}

/// Flux francophones. L'interface ne propose que le français et l'anglais :
/// le mode français ne charge donc que ces sources, et l'anglais le reste.
const FRENCH_SOURCES: [&str; 15] = [
    "Jeuxvideo.com",
    "Gamekult",
    "NoFrag",
    "Canard PC",
    "Factornews",
    "ActuGaming",
    "Gameblog",
    "Xboxygen",
    "Gamergen",
    "4WeAreGamers",
    "Mondes Persistants",
    "Jeuxvideo-Live",
    "Le Monde",
    "Clubic",
    "01net",
];

fn feed_in_language(source: &str, language: &str) -> bool {
    let french = FRENCH_SOURCES.contains(&source);
    if language == "fr" {
        french
    } else {
        !french
    }
}

/// Un site ne figure dans le menu que s'il a réellement fourni des articles.
/// Sources dont le flux ne publie aucune image mais dont la page article en
/// contient une. Le chargement se fait en parallèle : le flux n'attend pas
/// dix pages l'une après l'autre.
const PAGE_IMAGE_SOURCES: [&str; 1] = ["Canard PC"];

fn loaded_sources(stories: &[Story]) -> Vec<(&'static str, &'static str, usize)> {
    FEEDS
        .iter()
        .filter_map(|(source, _, icon)| {
            let count = stories
                .iter()
                .filter(|story| story.source == *source)
                .count();
            (count > 0).then_some((*source, *icon, count))
        })
        .collect()
}

const STORIES_PER_FEED: usize = 10;
const STORY_LIMIT: usize = 240;
const EXCERPT_CHARS: usize = 190;

#[derive(Clone, Debug)]
struct Story {
    source: &'static str,
    title: String,
    excerpt: String,
    url: String,
    image: String,
    /// Publication en secondes Unix ; 0 quand le flux ne la donne pas.
    date: i64,
}

#[derive(Default)]
pub(crate) struct Gaming {
    stories: Vec<Story>,
    art: NewsArt,
    loading: bool,
    asked: bool,
    /// Flux encore attendus : la page se termine quand ce compteur tombe à 0.
    pending: usize,
    rx: Option<mpsc::Receiver<(&'static str, Vec<Story>)>>,
    /// Sites décochés dans le menu. Vide = tous les sites.
    hidden_sources: HashSet<&'static str>,
    language: String,
    /// Adresse à ouvrir dans la webview Lapinux après un clic sur une carte.
    webview: Option<(String, String)>,
    error: Option<String>,
}

impl Gaming {
    pub(crate) fn ensure(&mut self, language: &str) {
        if self.asked && self.language == language {
            return;
        }
        self.asked = true;
        self.language = language.to_string();
        self.stories.clear();
        self.hidden_sources.clear();
        self.error = None;
        self.rx = None;
        self.loading = false;
        self.pending = 0;
        self.load();
    }

    pub(crate) fn refresh(&mut self) {
        if self.loading {
            return;
        }
        self.load();
    }

    pub(crate) fn loading(&self) -> bool {
        self.loading
    }

    fn load(&mut self) {
        if self.rx.is_some() {
            return;
        }
        let language = self.language.clone();
        let feeds: Vec<_> = FEEDS
            .iter()
            .filter(|(source, _, _)| feed_in_language(source, &language))
            .collect();
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        self.loading = true;
        self.pending = feeds.len();
        self.error = None;
        for (source, url, _) in feeds {
            let tx = tx.clone();
            let source = *source;
            let url = *url;
            std::thread::spawn(move || {
                let stories = fetch_feed(source, url);
                let _ = tx.send((source, stories));
            });
        }
        // Le dernier émetteur tombe avec ce fil : la réception se ferme quand
        // tous les flux ont répondu, même si l'un d'eux a échoué.
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let mut received = Vec::new();
        let mut disconnected = false;
        if let Some(rx) = &self.rx {
            loop {
                match rx.try_recv() {
                    Ok(item) => received.push(item),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }
        let got = !received.is_empty();
        for (_source, stories) in received {
            self.pending = self.pending.saturating_sub(1);
            self.stories.extend(stories);
        }
        if disconnected || (self.rx.is_some() && self.pending == 0) {
            self.stories
                .sort_by(|left, right| right.date.cmp(&left.date));
            let mut seen = std::collections::HashSet::new();
            self.stories.retain(|story| seen.insert(story.url.clone()));
            self.stories.truncate(STORY_LIMIT);
            self.error = if self.stories.is_empty() {
                Some(lapinux_core::i18n::t("news.gaming.error").to_string())
            } else {
                None
            };
            self.rx = None;
            self.loading = false;
        }
        if got {
            ctx.request_repaint();
        } else if self.loading {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }

    pub(crate) fn page(&mut self, ui: &mut egui::Ui, language: &str) {
        self.ensure(language);
        self.poll(ui.ctx());
        self.art.poll(ui.ctx());

        let loading = self.loading && self.stories.is_empty();
        let filtered: Vec<usize> = self
            .stories
            .iter()
            .enumerate()
            .filter(|(_, story)| !self.hidden_sources.contains(story.source))
            .map(|(index, _)| index)
            .collect();
        let width = (ui.available_width() - 12.0).max(240.0);
        let columns = columns(width).max(1);
        let card_width = grid_card_width(width);
        let card_height = grid_card_height_for(card_width, true);
        let grid = !loading && !self.stories.is_empty() && !filtered.is_empty();
        let rows = if grid {
            filtered.len().div_ceil(columns)
        } else {
            1
        };
        let row_height = if grid {
            card_height + GRID_GAP
        } else {
            ui.available_height().max(320.0)
        };
        let mut retry = false;
        let mut show_all = false;
        ui.add_space(8.0);
        // Les favicons des cartes suivent leur propre téléchargement : le
        // premier rendu garde une pastille de secours, puis l'image arrive.
        let source_icons: Vec<String> = filtered
            .iter()
            .filter_map(|&index| {
                let icon = source_icon(self.stories[index].source);
                (!icon.is_empty()).then(|| icon.to_string())
            })
            .collect();
        self.art.ensure(ui.ctx(), &source_icons);
        egui::ScrollArea::vertical()
            .id_salt("news-gaming")
            .auto_shrink([false, false])
            .show_rows(ui, row_height, rows, |ui, range| {
                ui.set_width(width);
                ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
                if loading {
                    skeleton_content_compact(ui, width);
                    return;
                }
                if self.stories.is_empty() {
                    let error = self.error.as_deref();
                    let title = if error.is_some() {
                        lapinux_core::i18n::t("news.error.title")
                    } else {
                        lapinux_core::i18n::t("news.empty_state.title")
                    };
                    let message =
                        error.unwrap_or_else(|| lapinux_core::i18n::t("news.gaming.empty"));
                    retry = status_card(
                        ui,
                        title,
                        message,
                        error.is_some(),
                        Some(lapinux_core::i18n::t("news.retry")),
                    );
                    return;
                }
                if filtered.is_empty() {
                    show_all = status_card(
                        ui,
                        lapinux_core::i18n::t("news.empty_state.title"),
                        lapinux_core::i18n::t("news.gaming.filter_empty"),
                        false,
                        Some(lapinux_core::i18n::t("news.show_all")),
                    );
                    return;
                }
                for row_index in range {
                    let start = row_index * columns;
                    let end = (start + columns).min(filtered.len());
                    let row = &filtered[start..end];
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = GRID_GAP;
                        for &index in row {
                            let story = self.stories[index].clone();
                            let date = story_date(story.date);
                            let id = ui.id().with(("news-story", story.url.as_str()));
                            if !story.image.is_empty() {
                                self.art
                                    .ensure(ui.ctx(), std::slice::from_ref(&story.image));
                            }
                            let icon = self.art.get(source_icon(story.source));
                            let content = story_content(&story, icon, "", &date);
                            if grid_card(
                                ui,
                                card_width,
                                self.art.get(&story.image),
                                None,
                                &story.title,
                                None,
                                id,
                                1.0,
                                &content,
                            ) {
                                self.webview = Some((
                                    story.url.clone(),
                                    format!("{} · {} · Lapinux", story.title, story.source),
                                ));
                            }
                        }
                    });
                    ui.add_space(GRID_GAP);
                }
            });

        if retry {
            self.refresh();
        }
        if show_all {
            self.hidden_sources.clear();
        }
    }

    /// Adresse à confier à la fenêtre WebKit de Lapinux, après un clic.
    pub(crate) fn take_webview(&mut self) -> Option<(String, String)> {
        self.webview.take()
    }

    /// Sélecteur de sites de l'en-tête. Chaque ligne barrée masque le site ;
    /// l'icône vient du favicon, avec une pastille de secours hors ligne.
    pub(super) fn source_picker(&mut self, ui: &mut egui::Ui) {
        let mut available = loaded_sources(&self.stories);
        // Les sites sans cookie passent en tête du menu ; l'ordre interne des
        // deux groupes reste celui des flux configurés.
        available.sort_by_key(|(source, _, _)| !cookie_free(source));
        let visible_sources = available
            .iter()
            .filter(|(source, _, _)| !self.hidden_sources.contains(source))
            .count();
        let selected = if available.is_empty() || visible_sources == available.len() {
            lapinux_core::i18n::t("news.gaming.all").to_string()
        } else if visible_sources == 0 {
            lapinux_core::i18n::t("news.gaming.none").to_string()
        } else if visible_sources == 1 {
            available
                .iter()
                .find(|(source, _, _)| !self.hidden_sources.contains(source))
                .map(|(source, _, _)| (*source).to_string())
                .unwrap_or_else(|| lapinux_core::i18n::t("news.gaming.none").to_string())
        } else {
            lapinux_core::i18n::t_args(
                "news.gaming.selected_sites",
                &[("count", &visible_sources.to_string())],
            )
            .to_string()
        };
        let icon_urls: Vec<String> = available
            .iter()
            .map(|(_, icon, _)| (*icon).to_string())
            .collect();
        let cookie_free_count = available
            .iter()
            .filter(|(source, _, _)| cookie_free(source))
            .count();
        let any_hidden = available
            .iter()
            .any(|(source, _, _)| self.hidden_sources.contains(source));
        let loading = self.loading;
        source_menu(
            ui,
            "news-gaming-source",
            &selected,
            NEWS_FILTER_WIDTH,
            |ui| {
                ui.set_min_width(NEWS_FILTER_WIDTH);
                self.art.ensure(ui.ctx(), &icon_urls);
                if icon_urls.iter().any(|url| self.art.get(url).is_none()) {
                    ui.ctx().request_repaint_after(Duration::from_millis(150));
                }
                if any_hidden {
                    if ui.button(lapinux_core::i18n::t("news.show_all")).clicked() {
                        self.hidden_sources.clear();
                    }
                    ui.separator();
                }
                if available.is_empty() {
                    let message = if loading {
                        lapinux_core::i18n::t("news.gaming.loading")
                    } else {
                        lapinux_core::i18n::t("news.gaming.no_articles")
                    };
                    ui.label(message);
                    return;
                }
                egui::ScrollArea::vertical()
                    .id_salt("news-gaming-feed-list")
                    .max_height(360.0)
                    .show(ui, |ui| {
                        let mut cookie_header = false;
                        let mut other_header = false;
                        for (source, icon, count) in &available {
                            let is_cookie_free = cookie_free(source);
                            if is_cookie_free && !cookie_header {
                                ui.label(
                                    egui::RichText::new(lapinux_core::i18n::t(
                                        "news.gaming.no_cookies",
                                    ))
                                    .size(10.5)
                                    .strong()
                                    .color(OK),
                                );
                                cookie_header = true;
                            } else if !is_cookie_free && cookie_free_count > 0 && !other_header {
                                ui.add_space(4.0);
                                ui.separator();
                                ui.label(
                                    egui::RichText::new(lapinux_core::i18n::t(
                                        "news.gaming.other_sites",
                                    ))
                                    .size(10.5)
                                    .color(crate::TEXT_DIM()),
                                );
                                other_header = true;
                            }
                            let hidden = self.hidden_sources.contains(source);
                            if filter_choice(
                                ui,
                                self.art.get(icon),
                                source,
                                *count,
                                hidden,
                                source_color(source),
                            ) {
                                if hidden {
                                    self.hidden_sources.remove(source);
                                } else {
                                    self.hidden_sources.insert(source);
                                }
                            }
                        }
                    });
            },
        );
    }
}

fn story_date(date: i64) -> String {
    if date > 0 {
        lapinux_core::util::format_date(date)
    } else {
        String::new()
    }
}

fn story_content<'a>(
    story: &'a Story,
    icon: Option<&'a egui::TextureHandle>,
    action: &'a str,
    date: &'a str,
) -> CardContent<'a> {
    let color = source_color(story.source);
    CardContent {
        label: "",
        label_color: color,
        store: None,
        meta_right: "",
        media_label: "",
        media_byline: Some(MediaByline {
            label: &story.source,
            color,
            icon,
            right: date,
        }),
        title: &story.title,
        excerpt: &story.excerpt,
        price: None,
        action,
    }
}

fn source_icon(source: &str) -> &'static str {
    FEEDS
        .iter()
        .find(|(name, _, _)| *name == source)
        .map(|(_, _, icon)| *icon)
        .unwrap_or("")
}

fn source_color(source: &str) -> egui::Color32 {
    match source {
        "Jeuxvideo.com" => egui::Color32::from_rgb(245, 197, 66),
        "PC Gamer" => egui::Color32::from_rgb(102, 192, 244),
        "Rock Paper Shotgun" => egui::Color32::from_rgb(242, 140, 40),
        "Gamekult" => egui::Color32::from_rgb(139, 212, 80),
        _ => ACCENT(),
    }
}

fn fetch_feed(source: &'static str, url: &str) -> Vec<Story> {
    let Some(bytes) = crate::achievements::http_bytes(url) else {
        return Vec::new();
    };
    let xml = String::from_utf8_lossy(&bytes);
    let mut stories: Vec<Story> = rss_items(&xml)
        .into_iter()
        .filter_map(|item| parse_item(source, item))
        .take(STORIES_PER_FEED)
        .collect();
    if PAGE_IMAGE_SOURCES.contains(&source) {
        fill_page_images(&mut stories);
    }
    stories
}

/// Complète les visuels absents du flux en lisant `og:image` sur la page de
/// chaque article. Les requêtes partent ensemble pour ne pas allonger la
/// lecture du flux de dix allers-retours successifs.
fn fill_page_images(stories: &mut [Story]) {
    let targets: Vec<(usize, String)> = stories
        .iter()
        .enumerate()
        .filter(|(_, story)| story.image.is_empty())
        .map(|(index, story)| (index, story.url.clone()))
        .collect();
    if targets.is_empty() {
        return;
    }
    let images = std::thread::scope(|scope| {
        let handles: Vec<_> = targets
            .into_iter()
            .map(|(index, url)| scope.spawn(move || (index, article_image(&url))))
            .collect();
        handles
            .into_iter()
            .filter_map(|handle| handle.join().ok())
            .collect::<Vec<_>>()
    });
    for (index, image) in images {
        if let Some(image) = image {
            if let Some(story) = stories.get_mut(index) {
                story.image = image;
            }
        }
    }
}

fn article_image(url: &str) -> Option<String> {
    let bytes = crate::achievements::http_bytes(url)?;
    let html = String::from_utf8_lossy(&bytes);
    meta_content(&html, "og:image")
}

/// Valeur `content` de la première balise meta qui porte la propriété donnée,
/// quel que soit l'ordre des attributs.
fn meta_content(html: &str, property: &str) -> Option<String> {
    let mut rest = html;
    while let Some(start) = rest.find("<meta") {
        let after = &rest[start..];
        let Some(end) = after.find('>') else {
            return None;
        };
        let head = &after[..end];
        if head.contains(property) {
            let content = head.find("content=\"")? + "content=\"".len();
            let after = &head[content..];
            let close = after.find('"')?;
            let value = decode_entities(&after[..close]).replace("&amp;", "&");
            if value.starts_with("http") {
                return Some(value);
            }
        }
        rest = &after[end..];
    }
    None
}

/// Découpe un flux RSS 2.0 en ses `<item>`. Les sources retenues partagent
/// ce format ; un flux Atom n'aurait pas ces balises.
fn rss_items(xml: &str) -> Vec<&str> {
    xml.split("<item>")
        .skip(1)
        .filter_map(|chunk| chunk.split("</item>").next())
        .collect()
}

fn parse_item(source: &'static str, item: &str) -> Option<Story> {
    let title = clean_text(&tag_text(item, "title"));
    let mut url = tag_text(item, "link").trim().to_string();
    if url.is_empty() {
        url = tag_attr(item, "link", "href").unwrap_or_default();
    }
    url = decode_entities(&url).trim().to_string();
    if title.is_empty() || !url.starts_with("http") {
        return None;
    }
    let description = tag_text(item, "description");
    let content = tag_text(item, "content:encoded");
    let excerpt_text = if description.trim().is_empty() {
        clean_text(&content)
    } else {
        clean_text(&description)
    };
    let excerpt = lapinux_core::util::ellipsize(&excerpt_text, EXCERPT_CHARS);
    let image = tag_attr(item, "media:content", "url")
        .or_else(|| tag_attr(item, "media:thumbnail", "url"))
        .or_else(|| tag_attr(item, "enclosure", "url"))
        .or_else(|| first_src(&description))
        .or_else(|| first_src(&content))
        .map(|url| decode_entities(&url).replace("&amp;", "&"))
        .filter(|url| url.starts_with("http"))
        .unwrap_or_default();
    let date = parse_rss_date(&tag_text(item, "pubDate"));
    Some(Story {
        source,
        title,
        excerpt,
        url,
        image,
        date,
    })
}

/// Contenu textuel d'une balise, sans ses attributs. Les flux enveloppent le
/// titre et le texte dans `CDATA`, que `clean_text` retire ensuite.
pub(super) fn tag_text(block: &str, tag: &str) -> String {
    let open = format!("<{tag}");
    let Some(start) = block.find(&open) else {
        return String::new();
    };
    let after = &block[start..];
    let Some(gt) = after.find('>') else {
        return String::new();
    };
    let body_start = start + gt + 1;
    let rest = &block[body_start..];
    let close = format!("</{tag}>");
    let Some(end) = rest.find(&close) else {
        return String::new();
    };
    rest[..end].to_string()
}

/// Valeur d'un attribut dans l'ouverture d'une balise (`url="…"`).
fn tag_attr(block: &str, tag: &str, attr: &str) -> Option<String> {
    let start = block.find(&format!("<{tag}"))?;
    let head_end = block[start..].find('>')? + start;
    let head = &block[start..head_end];
    let key = format!("{attr}=\"");
    let value_start = head.find(&key)? + key.len();
    let rest = &head[value_start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Première image intégrée au HTML d'un résumé (RPS en met une dans son
/// `description`, les autres plutôt dans `media:content`).
fn first_src(html: &str) -> Option<String> {
    let start = html.find("<img")?;
    let head = &html[start..];
    let end = head.find('>')?;
    let head = &head[..end];
    let src = head.find("src=\"")? + "src=\"".len();
    let rest = &head[src..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Texte lisible : CDATA, balises HTML, entités et espaces multiples retirés.
pub(super) fn clean_text(raw: &str) -> String {
    let raw = raw.trim();
    let raw = raw.strip_prefix("<![CDATA[").unwrap_or(raw);
    let raw = raw.strip_suffix("]]>").unwrap_or(raw);
    let text = strip_html(raw);
    decode_entities(&text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for character in html.chars() {
        match character {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(character),
            _ => {}
        }
    }
    out
}

pub(super) fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(index) = rest.find('&') {
        out.push_str(&rest[..index]);
        let tail = &rest[index..];
        let Some(end) = tail.find(';') else {
            out.push_str(tail);
            return out;
        };
        if let Some(character) = entity_char(&tail[1..end]) {
            out.push(character);
            rest = &tail[end + 1..];
        } else {
            out.push('&');
            rest = &tail[1..];
        }
    }
    out.push_str(rest);
    out
}

fn entity_char(entity: &str) -> Option<char> {
    if let Some(number) = entity.strip_prefix('#') {
        let code = if let Some(hex) = number
            .strip_prefix('x')
            .or_else(|| number.strip_prefix('X'))
        {
            u32::from_str_radix(hex, 16).ok()?
        } else {
            number.parse::<u32>().ok()?
        };
        return char::from_u32(code);
    }
    Some(match entity {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => ' ',
        "eacute" => 'é',
        "egrave" => 'è',
        "agrave" => 'à',
        "ccedil" => 'ç',
        "ecirc" => 'ê',
        "icirc" => 'î',
        "ocirc" => 'ô',
        "ucirc" => 'û',
        "ugrave" => 'ù',
        "Eacute" => 'É',
        "laquo" => '«',
        "raquo" => '»',
        _ => return None,
    })
}

pub(super) fn parse_rss_date(text: &str) -> i64 {
    let text = text.trim();
    if text.is_empty() {
        return 0;
    }
    let parts: Vec<&str> = text.split_whitespace().collect();
    let Some(time_index) = parts.iter().position(|part| part.matches(':').count() >= 2) else {
        return 0;
    };
    let hms: Vec<u32> = parts[time_index]
        .split(':')
        .filter_map(|part| part.parse().ok())
        .collect();
    let (Some(hour), Some(minute), Some(second)) = (hms.first(), hms.get(1), hms.get(2)) else {
        return 0;
    };
    let mut day = 0;
    let mut month = 0;
    let mut year = 0;
    for part in parts.iter().take(time_index) {
        let part = part.trim_end_matches(',');
        if let Some(number) = month_number(part) {
            month = number;
        } else if part.len() == 4 {
            if let Ok(value) = part.parse::<i32>() {
                year = value;
            }
        } else if let Ok(value) = part.parse::<u32>() {
            if (1..=31).contains(&value) {
                day = value;
            }
        }
    }
    if day == 0 || month == 0 || year == 0 {
        return 0;
    }
    let offset = parts
        .get(time_index + 1)
        .map(|zone| zone_offset(zone))
        .unwrap_or(0);
    days_from_civil(year, month, day) * 86_400
        + i64::from(*hour) * 3_600
        + i64::from(*minute) * 60
        + i64::from(*second)
        - i64::from(offset)
}

fn month_number(month: &str) -> Option<u32> {
    let month = month.get(..3)?.to_ascii_lowercase();
    Some(match month.as_str() {
        "jan" => 1,
        "feb" => 2,
        "mar" => 3,
        "apr" => 4,
        "may" => 5,
        "jun" => 6,
        "jul" => 7,
        "aug" => 8,
        "sep" => 9,
        "oct" => 10,
        "nov" => 11,
        "dec" => 12,
        _ => return None,
    })
}

fn zone_offset(zone: &str) -> i32 {
    let zone = zone.trim();
    if zone.eq_ignore_ascii_case("gmt") || zone.eq_ignore_ascii_case("utc") || zone == "Z" {
        return 0;
    }
    let sign = if zone.starts_with('-') { -1 } else { 1 };
    let digits: String = zone.chars().filter(char::is_ascii_digit).take(4).collect();
    if digits.len() < 4 {
        return 0;
    }
    let hours = digits[..2].parse::<i32>().unwrap_or(0);
    let minutes = digits[2..].parse::<i32>().unwrap_or(0);
    sign * (hours * 3_600 + minutes * 60)
}

/// Jours depuis le 1er janvier 1970 (algorithme civil de Howard Hinnant).
pub(super) fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = year - i32::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month = i32::try_from(month).unwrap_or(1);
    let month_prime = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * month_prime + 2) / 5 + i32::try_from(day).unwrap_or(1) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    i64::from(era * 146_097 + day_of_era - 719_468)
}

/// Jeu de données déterministe pour les captures hors écran. Les quatre
/// sources, les dates et une vignette absente sont représentées.
#[cfg(test)]
pub(super) fn snapshot_gaming(ctx: &egui::Context) -> Gaming {
    const COLORS: [(egui::Color32, egui::Color32); 8] = [
        (
            egui::Color32::from_rgb(30, 40, 70),
            egui::Color32::from_rgb(78, 112, 176),
        ),
        (
            egui::Color32::from_rgb(58, 32, 68),
            egui::Color32::from_rgb(176, 82, 132),
        ),
        (
            egui::Color32::from_rgb(28, 52, 44),
            egui::Color32::from_rgb(92, 156, 104),
        ),
        (
            egui::Color32::from_rgb(78, 46, 28),
            egui::Color32::from_rgb(206, 138, 62),
        ),
        (
            egui::Color32::from_rgb(34, 54, 74),
            egui::Color32::from_rgb(84, 154, 186),
        ),
        (
            egui::Color32::from_rgb(70, 34, 36),
            egui::Color32::from_rgb(184, 82, 78),
        ),
        (
            egui::Color32::from_rgb(46, 48, 30),
            egui::Color32::from_rgb(154, 160, 70),
        ),
        (
            egui::Color32::from_rgb(52, 34, 72),
            egui::Color32::from_rgb(138, 96, 198),
        ),
    ];
    let samples: [(&'static str, &str, &str, i64); 8] = [
        (
            "PC Gamer",
            "Les 25 meilleurs jeux PC de 2026",
            "Notre sélection annuelle, mise à jour après une année particulièrement dense en sorties indépendantes.",
            1_791_590_400,
        ),
        (
            "Jeuxvideo.com",
            "Hollow Knight: Silksong — notre test",
            "La suite tant attendue tient-elle ses promesses ? Réponse dans ce test complet.",
            1_791_583_200,
        ),
        (
            "Rock Paper Shotgun",
            "Ce que les soldes d'automne changent pour votre backlog",
            "Les promotions sont là, et avec elles la tentation de repousser encore un peu plus cette liste de jeux.",
            1_791_576_000,
        ),
        (
            "Gamekult",
            "Les sorties de la semaine",
            "RPG, plateforme et expériences indépendantes : le programme des prochains jours.",
            1_791_568_800,
        ),
        (
            "Jeuxvideo.com",
            "Cyberpunk 2077 : le mode multijoueur se précise",
            "CD Projekt donne enfin des nouvelles de son projet multijoueur dans l'univers de Night City.",
            1_791_561_600,
        ),
        (
            "PC Gamer",
            "Un nouveau patch pour Baldur's Gate 3",
            "Larian corrige plusieurs quêtes et améliore la stabilité du mode coopératif.",
            1_791_554_400,
        ),
        (
            "Gamekult",
            "Indie : cinq pépites à ne pas manquer",
            "Des jeux courts, étranges et marquants, repérés dans les nouvelles sorties de la semaine.",
            1_791_547_200,
        ),
        (
            "Rock Paper Shotgun",
            "Hades II quitte l'accès anticipé",
            "Supergiant Games détaille la version 1.0, ses ajouts et ce qui attend les joueurs déjà familiers du jeu.",
            1_791_540_000,
        ),
    ];
    let mut stories = Vec::new();
    let mut art = NewsArt::default();
    for (index, (source, title, excerpt, date)) in samples.iter().enumerate() {
        let image = if index == 3 {
            String::new()
        } else {
            format!("snapshot://story-{index}")
        };
        if !image.is_empty() {
            let (top, bottom) = COLORS[index];
            art.seed_cover(ctx, &image, top, bottom);
        }
        stories.push(Story {
            source,
            title: (*title).to_string(),
            excerpt: (*excerpt).to_string(),
            url: format!("https://example.com/story/{index}"),
            image,
            date: *date,
        });
    }
    Gaming {
        stories,
        art,
        loading: false,
        asked: true,
        pending: 0,
        rx: None,
        hidden_sources: HashSet::new(),
        language: "fr".to_string(),
        webview: None,
        error: None,
    }
}

#[cfg(test)]
pub(super) fn snapshot_gaming_loading() -> Gaming {
    Gaming {
        loading: true,
        asked: true,
        language: "fr".to_string(),
        ..Default::default()
    }
}

#[cfg(test)]
pub(super) fn snapshot_gaming_error() -> Gaming {
    Gaming {
        asked: true,
        error: Some(lapinux_core::i18n::t("news.gaming.error").to_string()),
        language: "fr".to_string(),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_rss_item_yields_title_link_image_and_date() {
        let xml = r#"
            <item>
              <title><![CDATA[Un titre &amp; une suite]]></title>
              <link>https://example.com/article</link>
              <pubDate>Mon, 09 Nov 2026 17:30:10 +0100</pubDate>
              <description><![CDATA[<p>Un résumé <b>lisible</b>.</p>]]></description>
              <enclosure url="https://cdn.example/image.jpg" type="image/jpeg" />
            </item>
        "#;
        let items = rss_items(xml);
        assert_eq!(items.len(), 1);
        let story = parse_item("Test", items[0]).unwrap();
        assert_eq!(story.title, "Un titre & une suite");
        assert_eq!(story.url, "https://example.com/article");
        assert_eq!(story.image, "https://cdn.example/image.jpg");
        assert_eq!(story.excerpt, "Un résumé lisible.");
        assert!(story.date > 0);
    }

    #[test]
    fn rfc_dates_with_different_offsets_are_the_same_instant() {
        let paris = parse_rss_date("Mon, 09 Nov 2026 17:30:10 +0100");
        let utc = parse_rss_date("Mon, 09 Nov 2026 16:30:10 +0000");
        assert_eq!(paris, utc);
        assert!(paris > 0);
    }

    #[test]
    fn html_and_entities_are_cleaned() {
        assert_eq!(
            clean_text("<![CDATA[<p>L&eacute;gende &amp; suite</p>]]>"),
            "Légende & suite"
        );
    }

    #[test]
    fn the_civil_epoch_starts_at_zero() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(1970, 1, 2), 1);
    }

    #[test]
    fn a_source_without_a_link_is_ignored() {
        let item = "<title>Sans lien</title><link></link>";
        assert!(parse_item("Test", item).is_none());
    }

    #[test]
    fn every_configured_feed_has_a_unique_name_and_address() {
        assert!(FEEDS.len() > 20);
        let mut names = std::collections::HashSet::new();
        let mut urls = std::collections::HashSet::new();
        for (name, url, icon) in FEEDS {
            assert!(names.insert(name), "source dupliquée : {name}");
            assert!(urls.insert(url), "flux dupliqué : {url}");
            assert!(icon.starts_with("https://"), "icône absente pour {name}");
        }
    }

    #[test]
    fn only_sources_with_stories_are_listed() {
        assert!(loaded_sources(&[]).is_empty());
        let story = Story {
            source: "Gamekult",
            title: "Un titre".into(),
            excerpt: String::new(),
            url: "https://example.com/article".into(),
            image: String::new(),
            date: 0,
        };
        let sources = loaded_sources(std::slice::from_ref(&story));
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].0, "Gamekult");
        assert_eq!(sources[0].2, 1);
    }
    #[test]
    fn story_content_moves_source_and_date_onto_the_image() {
        let story = Story {
            source: "PC Gamer",
            title: "Un titre".into(),
            excerpt: String::new(),
            url: "https://example.com/article".into(),
            image: String::new(),
            date: 0,
        };
        let content = story_content(&story, None, "", "10/11/2026");
        assert!(content.label.is_empty());
        assert!(content.meta_right.is_empty());
        let byline = content
            .media_byline
            .expect("la provenance doit être sur le visuel");
        assert_eq!(byline.label, "PC Gamer");
        assert_eq!(byline.right, "10/11/2026");
        assert!(byline.icon.is_none());
        assert!(source_icon("PC Gamer").contains("pcgamer.com"));
    }

    #[test]
    fn cookie_free_sites_are_known_and_unique() {
        let configured: std::collections::HashSet<&str> =
            FEEDS.iter().map(|(name, _, _)| *name).collect();
        let mut unique = std::collections::HashSet::new();
        for name in COOKIE_FREE_SOURCES {
            assert!(
                configured.contains(name),
                "site sans cookie inconnu : {name}"
            );
            assert!(unique.insert(name), "site sans cookie dupliqué : {name}");
        }
        assert!(cookie_free("Gamekult"));
        assert!(!cookie_free("PC Gamer"));
    }

    #[test]
    fn an_article_page_gives_its_og_image() {
        let html = r#"<html><head><meta property="description" content="x"><meta content="https://cdn.example/cover.jpg" property="og:image"></head></html>"#;
        assert_eq!(
            meta_content(html, "og:image").as_deref(),
            Some("https://cdn.example/cover.jpg")
        );
    }

    #[test]
    fn feeds_follow_the_interface_language() {
        assert!(feed_in_language("Jeuxvideo.com", "fr"));
        assert!(feed_in_language("Gamergen", "fr"));
        assert!(!feed_in_language("Polygon", "fr"));
        assert!(feed_in_language("Polygon", "en"));
        assert!(!feed_in_language("Gamekult", "en"));
        let configured: std::collections::HashSet<&str> =
            FEEDS.iter().map(|(name, _, _)| *name).collect();
        for name in FRENCH_SOURCES {
            assert!(configured.contains(name), "flux français inconnu : {name}");
        }
    }
}
