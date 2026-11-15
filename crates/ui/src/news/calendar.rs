//! Section **Actualités › Calendrier** : les sorties du jour et les prochaines
//! sorties PC. Steam, Epic Games, GOG, les nouveautés itch.io et les jeux
//! upcoming sans date sont lus ensemble ; chaque ligne garde sa boutique et la
//! liste fusionnée se complète par paliers. itch.io ne publie pas de calendrier
//! public de dates à venir : son flux ne permet d'ajouter que les sorties du
//! jour et une sélection de jeux marqués upcoming, classée sans date.
//!
//! Steam ne renvoie pas de JSON de sorties : la recherche « infinite » rend un
//! fragment HTML. On y lit l'identifiant, le titre, la date et la capsule, puis
//! on ne garde que les dates complètes. Les dates partielles (« novembre
//! 2026 », « Prochainement ») ne peuvent pas être classées dans un jour.

use super::gaming::{clean_text, days_from_civil, parse_rss_date, tag_text};
use super::{
    filter_choice, source_menu, status_card, store_color, store_logo, NEWS_FILTER_WIDTH,
    SCROLLBAR_ROOM,
};
use crate::gameinfo::NewsArt;
use crate::{ACCENT, BORDER, CARD_ACTIVE, CARD_HOVER, DANGER, TEXT, TEXT_DIM};
use eframe::egui;
use lapinux_core::stores::Store;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

const BROWSER_UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36";
const SEARCH_URL: &str = "https://store.steampowered.com/search/results/?query&start={start}&count={count}&infinite=1&cc=fr&l={language}&filter={filter}&sort_by={sort}";
/// Le GraphQL public d'Epic refuse l'agent d'un navigateur et attend celui du
/// launcher. La période de sortie est incluse dans la requête, ce qui évite de
/// parcourir les quelque 29 000 fiches du catalogue.
const EPIC_UA: &str = "EpicGamesLauncher/14.0.8-22004686+++Portal+Release-Live";
const EPIC_GQL: &str = "https://store.epicgames.com/graphql";
const EPIC_CALENDAR_QUERY: &str = r#"query searchStoreQuery($country: String!, $locale: String, $count: Int, $start: Int, $releaseDate: String, $sortBy: String, $sortDir: String) {
  Catalog {
    searchStore(country: $country, locale: $locale, count: $count, start: $start, releaseDate: $releaseDate, sortBy: $sortBy, sortDir: $sortDir) {
      elements {
        title
        productSlug
        urlSlug
        releaseDate
        keyImages { type url }
        categories { path }
      }
      paging { count total }
    }
  }
}"#;
/// Repli public quand Steam refuse la connexion (Akamai répond parfois 403
/// à l'adresse de la machine). Les catalogues Epic et GOG ne demandent aucune clé.
const GOG_CATALOG: &str = "https://catalog.gog.com/v1/catalog?limit=48&page={page}&order={order}&productType=in:game&countryCode=FR&locale={locale}&currencyCode=EUR&releaseStatuses=in:{status}";
const PAGE_SIZE: usize = 100;
const GOG_PAGE_SIZE: usize = 48;
const EPIC_PAGE_SIZE: usize = 40;
/// Les dates au-delà de dix ans sont des placeholders de catalogue (« 2099 »)
/// et non des sorties annoncées.
const EPIC_HORIZON_DAYS: i64 = 3650;
const ITCH_NEWEST: &str = "https://itch.io/games/newest.xml";
const ITCH_UPCOMING: &str = "https://itch.io/games/tag-upcoming.xml";
/// Les jeux upcoming n'ont pas de date de sortie : on les range après toutes
/// les dates connues, sous un en-tête dédié.
const UNDATED_ORDINAL: i64 = i64::MAX;
/// Les tags itch.io sont peu modérés : au-delà de quatre ans, les pages du flux
/// « top upcoming » sont le plus souvent des prototypes abandonnés.
const ITCH_UPCOMING_MAX_AGE_DAYS: i64 = 4 * 365;
/// Les premières pages sont lues à l'ouverture. Steam annonce près de 18 000
/// sorties : on n'attend pas la totalité pour afficher les plus proches.
const INITIAL_PAGES: usize = 3;
/// En mode Top, la première lecture GOG s'arrête après deux pages ;
/// « Charger plus » poursuit le classement par popularité.
const GOG_TOP_PAGES: usize = 2;
/// La recherche Steam est bloquée depuis certaines adresses. Le classement
/// public des wishlists fournit les appids, puis l'API Store officielle donne
/// les dates, les visuels et les fiches sans passer par la recherche.
const STEAM_TOP_WISHLIST: &str = "https://games-popularity.com/steam/top-wishlist?page={page}";
const STEAM_STORE_ITEMS: &str = "https://api.steampowered.com/IStoreBrowseService/GetItems/v1/";
const STEAM_TOP_PAGES: usize = 8;
/// Le calendrier personnel n'est jamais lu par Lapinux : Steam le réserve au
/// compte connecté. Le bouton l'ouvre simplement dans le navigateur du bureau.
const STEAM_PERSONAL_CALENDAR: &str = "https://store.steampowered.com/personalcalendar";
/// Classement public des progressions de wishlists sur 30 jours. Une seule
/// page suffit : il complète le Top principal avec les jeux qui montent, sans
/// toucher aux pages du Store Steam.
const STEAM_TOP_TRENDING: &str = "https://games-popularity.com/steam/top-wishlist/month/desc/1";
const STEAM_TOP_TRENDING_LIMIT: usize = 25;
/// Le Top est une lecture réseau coûteuse. Une réponse complète est gardée six
/// heures sur disque ; une réponse en échec réutilise la dernière liste connue
/// plutôt que de relancer Steam ou le classement public.
const TOP_CACHE_VERSION: u32 = 1;
const TOP_CACHE_TTL: i64 = 6 * 60 * 60;
/// Les sorties du jour sont en tête de la recherche « released » ; la lecture
/// s'arrête dès qu'une page atteint la veille, avec une borne pour les journées
/// exceptionnellement chargées.
const TODAY_PAGES: usize = 10;

type Reply = Result<PageReply, String>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    Steam,
    Epic,
    Gog,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Cursor {
    source: Source,
    next_start: usize,
    total: usize,
}

impl Cursor {
    fn has_more(self) -> bool {
        self.next_start < self.total
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Release {
    appid: u32,
    store: Store,
    title: String,
    image: String,
    url: String,
    ordinal: i64,
    date: String,
    /// Rang dans le classement public, quand la ligne vient du Top Steam.
    #[serde(default)]
    rank: Option<u32>,
}

#[derive(Debug, Default)]
struct SearchPage {
    releases: Vec<Release>,
    /// Nombre de lignes renvoyées par Steam, filtres exclus : c'est ce pas
    /// qu'il faut ajouter à `start` pour lire la page suivante.
    count: usize,
    total: usize,
}

#[derive(Debug, Default)]
struct EpicPage {
    releases: Vec<Release>,
    /// Nombre de fiches renvoyées, filtres exclus : c'est le pas de pagination
    /// à ajouter à `start` pour lire la suite.
    count: usize,
    total: usize,
}

#[derive(Debug, Default)]
struct PageReply {
    releases: Vec<Release>,
    cursors: Vec<Cursor>,
    error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct TopCache {
    version: u32,
    language: String,
    saved_at: i64,
    releases: Vec<Release>,
}

#[derive(Debug, Default)]
struct GogPage {
    releases: Vec<Release>,
    pages: usize,
}

#[derive(Default)]
pub(super) struct Calendar {
    releases: Vec<Release>,
    hidden_stores: Vec<Store>,
    top_only: bool,
    art: NewsArt,
    loading: bool,
    loading_more: bool,
    asked: bool,
    append: bool,
    language: String,
    cursors: Vec<Cursor>,
    error: Option<String>,
    rx: Option<mpsc::Receiver<Reply>>,
    webview: Option<(String, String)>,
}

impl Calendar {
    /// Démarre la première lecture quand l'utilisateur ouvre l'onglet. Les
    /// autres onglets ne lancent aucune requête Calendrier.
    pub(super) fn ensure(&mut self, ctx: &egui::Context, language: &str) {
        if self.asked && self.language == language {
            return;
        }
        let first = !self.asked;
        if first {
            self.top_only = true;
        }
        self.asked = true;
        self.language = language.to_string();
        if !first {
            self.releases.clear();
            self.cursors.clear();
        }
        self.start_initial(ctx, language, false);
    }

    pub(super) fn loading(&self) -> bool {
        self.loading || self.loading_more
    }

    /// Relit les prochaines sorties et les sorties du jour. La liste actuelle
    /// reste visible jusqu'à la réponse, ce qui évite un saut de page.
    pub(super) fn refresh(&mut self, ctx: &egui::Context, language: &str) {
        if self.loading() {
            return;
        }
        self.language = language.to_string();
        self.error = None;
        self.start_initial(ctx, language, true);
    }

    fn start_initial(&mut self, ctx: &egui::Context, language: &str, force: bool) {
        let language = language.to_string();
        let top = self.top_only;
        let ctx = ctx.clone();
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        self.loading = true;
        self.loading_more = false;
        self.append = false;
        self.error = None;
        std::thread::spawn(move || {
            let reply = initial_reply(&language, top, force);
            if tx.send(reply).is_ok() {
                ctx.request_repaint();
            }
        });
    }

    fn load_more(&mut self, ctx: &egui::Context) {
        if self.loading() || !self.cursors.iter().any(|cursor| cursor.has_more()) {
            return;
        }
        let language = self.language.clone();
        let top = self.top_only;
        let cursors = self.cursors.clone();
        let ctx = ctx.clone();
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        self.loading_more = true;
        self.append = true;
        self.error = None;
        std::thread::spawn(move || {
            let reply = more_reply(&language, cursors, top);
            if tx.send(reply).is_ok() {
                ctx.request_repaint();
            }
        });
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let mut message = None;
        if let Some(rx) = self.rx.as_ref() {
            match rx.try_recv() {
                Ok(reply) => message = Some(reply),
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    message = Some(Err(
                        lapinux_core::i18n::t("news.calendar.worker_lost").to_string()
                    ));
                }
            }
        }
        let Some(message) = message else {
            if self.loading() {
                ctx.request_repaint_after(Duration::from_millis(180));
            }
            return;
        };
        self.rx = None;
        self.loading = false;
        self.loading_more = false;
        let append = std::mem::take(&mut self.append);
        match message {
            Ok(reply) => {
                if append {
                    self.releases.extend(reply.releases);
                } else {
                    self.releases = reply.releases;
                }
                merge_releases(&mut self.releases);
                self.cursors = reply.cursors;
                self.error = reply.error;
            }
            Err(error) => {
                // Une page « charger plus » en échec ne doit pas effacer les
                // sorties déjà lisibles ; la première lecture, elle, affiche
                // l'état d'erreur de la page.
                if !append {
                    self.releases.clear();
                    self.cursors.clear();
                }
                self.error = Some(error);
            }
        }
        ctx.request_repaint();
    }

    /// Sélecteur réel des boutiques du calendrier. Le popup reste ouvert quand
    /// on clique une ligne, ce qui permet d'en masquer plusieurs d'affilée.
    pub(super) fn source_picker(&mut self, ui: &mut egui::Ui) {
        const STORES: [Store; 4] = [Store::Steam, Store::Epic, Store::Gog, Store::Itch];
        let mut counts = [0usize; 4];
        for release in &self.releases {
            if let Some(index) = STORES.iter().position(|store| *store == release.store) {
                counts[index] += 1;
            }
        }
        let visible_stores: Vec<Store> = STORES
            .iter()
            .copied()
            .filter(|store| !self.hidden_stores.contains(store))
            .collect();
        let mut label = if visible_stores.is_empty() {
            lapinux_core::i18n::t("news.calendar.none").to_string()
        } else if visible_stores.len() == 1 {
            visible_stores[0].label().to_string()
        } else if visible_stores.len() == STORES.len() {
            lapinux_core::i18n::t("news.calendar.all_stores").to_string()
        } else {
            let count = visible_stores.len().to_string();
            lapinux_core::i18n::t_args("news.calendar.selected_stores", &[("count", &count)])
                .to_string()
        };
        if self.top_only {
            label.push_str(" · ");
            label.push_str(lapinux_core::i18n::t("news.calendar.top_badge"));
        }
        let mut hidden = self.hidden_stores.clone();
        let mut top_only = self.top_only;
        let can_toggle_top = !self.loading();
        source_menu(
            ui,
            "news-calendar-source",
            &label,
            NEWS_FILTER_WIDTH,
            |ui| {
                for (index, store) in STORES.iter().enumerate() {
                    let is_hidden = hidden.contains(store);
                    let logo = store_logo(ui.ctx(), *store);
                    if filter_choice(
                        ui,
                        logo.as_ref(),
                        store.label(),
                        counts[index],
                        is_hidden,
                        store_color(Some(*store)),
                    ) {
                        if is_hidden {
                            hidden.retain(|candidate| candidate != store);
                        } else {
                            hidden.push(*store);
                        }
                    }
                }
                ui.add_space(4.0);
                ui.separator();
                ui.add_enabled(
                    can_toggle_top,
                    egui::Checkbox::new(
                        &mut top_only,
                        lapinux_core::i18n::t("news.calendar.top_only"),
                    ),
                );
                ui.label(
                    egui::RichText::new(lapinux_core::i18n::t("news.calendar.top_scope"))
                        .size(10.0)
                        .color(TEXT_DIM()),
                );
                if !hidden.is_empty() {
                    ui.add_space(3.0);
                    if ui
                        .button(lapinux_core::i18n::t("news.calendar.show_all"))
                        .clicked()
                    {
                        hidden.clear();
                    }
                }
            },
        );
        self.hidden_stores = hidden;
        let changed = top_only != self.top_only;
        self.top_only = top_only;
        if changed {
            // Basculer le Top ne doit pas contourner le cache : seule
            // l'action « Actualiser » force une nouvelle lecture réseau.
            let language = self.language.clone();
            self.error = None;
            self.start_initial(ui.ctx(), &language, false);
        }
    }

    pub(super) fn page(&mut self, ui: &mut egui::Ui, language: &str) {
        // Un changement d'onglet est appliqué par l'en-tête avant le contenu :
        // la première frame du calendrier doit déjà lancer sa lecture.
        self.ensure(ui.ctx(), language);
        self.poll(ui.ctx());
        self.art.poll(ui.ctx());
        if self.loading && self.releases.is_empty() {
            ui.add_space(22.0);
            ui.vertical_centered(|ui| {
                crate::section_icons::loading(ui, 18.0);
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(lapinux_core::i18n::t("news.calendar.loading"))
                        .size(12.0)
                        .color(TEXT_DIM()),
                );
            });
            ui.ctx().request_repaint_after(Duration::from_millis(180));
            return;
        }
        if self.releases.is_empty() {
            let failed = self.error.is_some();
            if status_card(
                ui,
                if failed {
                    lapinux_core::i18n::t("news.error.title")
                } else {
                    lapinux_core::i18n::t("news.empty_state.title")
                },
                self.error
                    .as_deref()
                    .unwrap_or(lapinux_core::i18n::t("news.calendar.empty")),
                failed,
                Some(lapinux_core::i18n::t("news.retry")),
            ) {
                self.refresh(ui.ctx(), language);
            }
            return;
        }

        let visible: Vec<Release> = self
            .releases
            .iter()
            .filter(|release| !self.hidden_stores.contains(&release.store))
            .cloned()
            .collect();
        if visible.is_empty() {
            if status_card(
                ui,
                lapinux_core::i18n::t("news.empty_state.title"),
                lapinux_core::i18n::t("news.calendar.filter_empty"),
                false,
                Some(lapinux_core::i18n::t("news.calendar.show_all")),
            ) {
                self.hidden_stores.clear();
            }
            return;
        }
        let width = (ui.available_width() - SCROLLBAR_ROOM).max(320.0);
        let today = today_ordinal();
        let today_label = lapinux_core::util::format_date(lapinux_core::util::now_unix());
        let groups = grouped(&visible, today);
        let mut open = None;
        let mut more = false;
        let images: Vec<String> = visible
            .iter()
            .map(|release| release.image.clone())
            .filter(|image| !image.is_empty())
            .collect();
        self.art.ensure(ui.ctx(), &images);
        if images.iter().any(|image| self.art.get(image).is_none()) {
            ui.ctx().request_repaint_after(Duration::from_millis(180));
        }

        egui::ScrollArea::vertical()
            .id_salt("news-calendar")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_width(width);
                ui.add_space(2.0);
                if groups.first().map(|(ordinal, _)| *ordinal) != Some(today) {
                    group_header(ui, today, today, &today_label, 0);
                    ui.label(
                        egui::RichText::new(lapinux_core::i18n::t("news.calendar.today_empty"))
                            .size(11.5)
                            .color(TEXT_DIM()),
                    );
                    ui.add_space(8.0);
                }
                for (ordinal, indices) in &groups {
                    let date = &visible[indices[0]].date;
                    group_header(ui, *ordinal, today, date, indices.len());
                    for &index in indices {
                        let release = &visible[index];
                        let source = release_source(release, self.top_only);
                        if release_row(ui, width, release, self.art.get(&release.image), &source) {
                            open = Some((
                                release.url.clone(),
                                format!("{} · {} · Lapinux", release.title, release.store.label()),
                            ));
                        }
                    }
                    ui.add_space(8.0);
                }

                if let Some(error) = &self.error {
                    if !error.is_empty() {
                        ui.add_space(2.0);
                        ui.label(egui::RichText::new(error).size(11.0).color(DANGER));
                    }
                }
                ui.add_space(4.0);
                if self.loading_more {
                    ui.vertical_centered(|ui| {
                        crate::section_icons::loading(ui, 14.0);
                        ui.label(
                            egui::RichText::new(lapinux_core::i18n::t(
                                "news.calendar.loading_more",
                            ))
                            .size(11.0)
                            .color(TEXT_DIM()),
                        );
                    });
                } else if self.cursors.iter().any(|cursor| cursor.has_more()) {
                    ui.vertical_centered(|ui| {
                        if crate::action_button_sized(
                            ui,
                            lapinux_core::i18n::t("news.calendar.load_more"),
                            crate::ActionTone::Quiet,
                            true,
                            34.0,
                        )
                        .clicked()
                        {
                            more = true;
                        }
                    });
                } else {
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new(lapinux_core::i18n::t("news.calendar.all_loaded"))
                                .size(11.0)
                                .color(TEXT_DIM()),
                        );
                    });
                }
                ui.add_space(10.0);
            });

        if let Some((url, title)) = open {
            self.webview = Some((url, title));
        }
        if more {
            self.load_more(ui.ctx());
        }
        if self.loading() {
            ui.ctx().request_repaint_after(Duration::from_millis(180));
        }
    }

    /// Adresse à confier à la fenêtre WebKit de Lapinux, après un clic.
    pub(crate) fn take_webview(&mut self) -> Option<(String, String)> {
        self.webview.take()
    }
}

/// Le calendrier personnel Steam reste sur le site : ce bouton ne récupère
/// rien et n'ajoute donc aucun appel réseau au calendrier. Il vit dans
/// l'en-tête, à gauche d'Actualiser.
pub(super) fn personal_calendar_button(ui: &mut egui::Ui) {
    let label = format!(
        "{}  ↗",
        lapinux_core::i18n::t("news.calendar.personal_steam")
    );
    let response = crate::action_button_sized(
        ui,
        label,
        crate::ActionTone::Quiet,
        true,
        super::NEWS_CONTROL_HEIGHT,
    );
    if response
        .on_hover_text(lapinux_core::i18n::t("news.calendar.personal_steam_hint"))
        .clicked()
    {
        crate::open_url(STEAM_PERSONAL_CALENDAR);
    }
}

fn release_source(release: &Release, top: bool) -> String {
    if top && release.store == Store::Steam {
        if let Some(rank) = release.rank {
            let rank = rank.to_string();
            let label = lapinux_core::i18n::t_args("news.calendar.top_rank", &[("rank", &rank)]);
            return format!("{} · {label}", release.store.label());
        }
    }
    release.store.label().to_string()
}

fn grouped(releases: &[Release], today: i64) -> Vec<(i64, Vec<usize>)> {
    let mut groups: Vec<(i64, Vec<usize>)> = Vec::new();
    for (index, release) in releases.iter().enumerate() {
        if release.ordinal < today {
            continue;
        }
        match groups.last_mut() {
            Some((ordinal, indices)) if *ordinal == release.ordinal => indices.push(index),
            _ => groups.push((release.ordinal, vec![index])),
        }
    }
    groups
}

fn group_header(ui: &mut egui::Ui, ordinal: i64, today: i64, date: &str, count: usize) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let label = if ordinal == UNDATED_ORDINAL {
            lapinux_core::i18n::t("news.calendar.undated").to_string()
        } else if ordinal == today {
            format!("{} · {date}", lapinux_core::i18n::t("news.calendar.today"))
        } else if ordinal == today + 1 {
            format!(
                "{} · {date}",
                lapinux_core::i18n::t("news.calendar.tomorrow")
            )
        } else {
            date.to_string()
        };
        ui.label(
            egui::RichText::new(label)
                .size(14.5)
                .strong()
                .color(if ordinal == today { ACCENT() } else { TEXT() }),
        );
        if count > 0 {
            let count = count.to_string();
            ui.label(
                egui::RichText::new(lapinux_core::i18n::t_args(
                    "news.calendar.release_count",
                    &[("count", &count)],
                ))
                .size(11.0)
                .color(TEXT_DIM()),
            );
        }
    });
    ui.add_space(3.0);
}

fn release_row(
    ui: &mut egui::Ui,
    width: f32,
    release: &Release,
    texture: Option<&egui::TextureHandle>,
    source: &str,
) -> bool {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 58.0), egui::Sense::click());
    let painter = ui.painter();
    if response.hovered() {
        painter.rect_filled(rect, egui::Rounding::same(0.0), CARD_HOVER());
    }
    if response.has_focus() {
        painter.rect_stroke(
            rect.shrink(1.0),
            egui::Rounding::same(0.0),
            egui::Stroke::new(1.5_f32, ACCENT()),
        );
    }

    let image_rect = egui::Rect::from_min_size(
        egui::pos2(rect.left() + 8.0, rect.center().y - 16.5),
        egui::vec2(88.0, 33.0),
    );
    if let Some(texture) = texture {
        painter.image(
            texture.id(),
            image_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    } else {
        painter.rect_filled(image_rect, egui::Rounding::same(0.0), CARD_ACTIVE());
        let initial = release
            .title
            .chars()
            .next()
            .map(String::from)
            .unwrap_or_else(|| "?".to_string());
        painter.text(
            image_rect.center(),
            egui::Align2::CENTER_CENTER,
            initial,
            egui::FontId::proportional(17.0),
            TEXT_DIM(),
        );
    }

    let text_left = image_rect.right() + 12.0;
    let text_room = (rect.right() - 34.0 - text_left).max(40.0);
    let title_galley = super::fitted_text(
        painter,
        &release.title,
        egui::FontId::proportional(14.0),
        TEXT(),
        text_room,
    );
    painter.galley(
        egui::pos2(text_left, rect.top() + 12.0),
        title_galley,
        TEXT(),
    );
    painter.text(
        egui::pos2(text_left, rect.top() + 35.0),
        egui::Align2::LEFT_TOP,
        source,
        egui::FontId::proportional(10.5),
        TEXT_DIM(),
    );
    painter.text(
        egui::pos2(rect.right() - 15.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        "›",
        egui::FontId::proportional(18.0),
        TEXT_DIM(),
    );
    painter.hline(
        rect.x_range(),
        rect.bottom(),
        egui::Stroke::new(1.0_f32, BORDER().gamma_multiply(0.28)),
    );
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.clicked()
}

fn today_ordinal() -> i64 {
    ordinal_from_date(&lapinux_core::util::format_date(
        lapinux_core::util::now_unix(),
    ))
    .unwrap_or(0)
}

fn ordinal_from_date(text: &str) -> Option<i64> {
    let mut parts = text.split('/');
    let day = parts.next().and_then(|value| value.parse::<u32>().ok());
    let month = parts.next().and_then(|value| value.parse::<u32>().ok());
    let year = parts.next().and_then(|value| value.parse::<i32>().ok());
    match (day, month, year) {
        (Some(day), Some(month), Some(year))
            if (1..=31).contains(&day) && (1..=12).contains(&month) =>
        {
            Some(days_from_civil(year, month, day))
        }
        _ => None,
    }
}

fn date_from_stamp(stamp: i64) -> Option<(i64, String)> {
    if stamp <= 0 {
        return None;
    }
    let date = lapinux_core::util::format_date(stamp);
    ordinal_from_date(&date).map(|ordinal| (ordinal, date))
}

fn parse_release_date(text: &str) -> Option<(i64, String)> {
    let text = clean_text(text).replace('.', " ");
    let words: Vec<&str> = text
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect();
    let year = words.iter().find_map(|word| {
        if word.len() != 4 {
            return None;
        }
        let year = word.parse::<i32>().ok()?;
        (1970..=9999).contains(&year).then_some(year)
    })?;
    let month = words.iter().find_map(|word| month_number(word))?;
    let day = words.iter().find_map(|word| {
        let day = word.parse::<u32>().ok()?;
        (1..=31).contains(&day).then_some(day)
    })?;
    Some((
        days_from_civil(year, month, day),
        format!("{day:02}/{month:02}/{year:04}"),
    ))
}

fn month_number(word: &str) -> Option<u32> {
    match fold_accents(&word.to_lowercase()).as_str() {
        "jan" | "janv" | "janvier" | "january" => Some(1),
        "feb" | "fev" | "fevr" | "fevrier" | "february" => Some(2),
        "mar" | "mars" | "march" => Some(3),
        "apr" | "avr" | "avril" | "april" => Some(4),
        "may" | "mai" => Some(5),
        "jun" | "juin" | "june" => Some(6),
        "jul" | "juil" | "juillet" | "july" => Some(7),
        "aug" | "aout" | "august" => Some(8),
        "sep" | "sept" | "septembre" | "september" => Some(9),
        "oct" | "octobre" | "october" => Some(10),
        "nov" | "novembre" | "november" => Some(11),
        "dec" | "decembre" | "december" => Some(12),
        _ => None,
    }
}

fn is_game_like(title: &str) -> bool {
    let title = fold_accents(&title.to_lowercase());
    let words: Vec<&str> = title
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect();
    let blocked = ["demo", "playtest", "soundtrack", "artbook"];
    !words.iter().any(|word| blocked.contains(word))
        && !title.contains("supporter pack")
        && !title.contains("season pass")
}

/// Les dates Steam suivent la langue de la boutique. On replie les accents
/// pour que les noms de mois français restent des constantes ASCII, sans
/// littéraux d'interface à traduire.
fn fold_accents(word: &str) -> String {
    word.chars()
        .map(|character| match character {
            'à' | 'á' | 'â' | 'ä' => 'a',
            'ç' => 'c',
            'è' | 'é' | 'ê' | 'ë' => 'e',
            'ì' | 'í' | 'î' | 'ï' => 'i',
            'ñ' => 'n',
            'ò' | 'ó' | 'ô' | 'ö' => 'o',
            'ù' | 'ú' | 'û' | 'ü' => 'u',
            'ý' | 'ÿ' => 'y',
            other => other,
        })
        .collect()
}

fn parse_search_page(bytes: &[u8]) -> Result<SearchPage, String> {
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|error| {
        let detail = error.to_string();
        lapinux_core::i18n::t_args("news.calendar.invalid_response", &[("detail", &detail)])
    })?;
    if value.get("success").and_then(serde_json::Value::as_bool) == Some(false) {
        return Err(lapinux_core::i18n::t("news.calendar.refused").to_string());
    }
    let html = value
        .get("results_html")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| lapinux_core::i18n::t("news.calendar.no_results").to_string())?;
    let total = value
        .get("total_count")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as usize;
    let mut releases = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut count = 0;
    for row in html.split("data-ds-appid=\"").skip(1) {
        let Some(end) = row.find('"') else {
            continue;
        };
        let Ok(appid) = row[..end].parse::<u32>() else {
            continue;
        };
        count += 1;
        if !seen.insert(appid) {
            continue;
        }
        let row = &row[end..];
        let title = html_between(row, "class=\"title\">", "</span>")
            .map(clean_text)
            .unwrap_or_default();
        if title.is_empty() || !is_game_like(&title) {
            continue;
        }
        let date_text = released_text(row).unwrap_or_default();
        let Some((ordinal, date)) = parse_release_date(&date_text) else {
            continue;
        };
        let image = html_attr(row, "<img src=\"").unwrap_or_default();
        releases.push(Release {
            appid,
            store: Store::Steam,
            title,
            image: image.to_string(),
            url: format!("https://store.steampowered.com/app/{appid}"),
            ordinal,
            date,
            rank: None,
        });
    }
    Ok(SearchPage {
        releases,
        count,
        total,
    })
}

fn released_text(row: &str) -> Option<String> {
    let marker = "class=\"search_released";
    let start = row.find(marker)? + marker.len();
    let tail = &row[start..];
    let start = tail.find('>')? + 1;
    let tail = &tail[start..];
    let end = tail.find("</div>")?;
    Some(tail[..end].to_string())
}

fn html_between<'a>(text: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let start = text.find(start)? + start.len();
    let tail = &text[start..];
    let end = tail.find(end)?;
    Some(&tail[..end])
}

fn html_attr<'a>(text: &'a str, marker: &str) -> Option<&'a str> {
    let start = text.find(marker)? + marker.len();
    let tail = &text[start..];
    let end = tail.find('"')?;
    Some(&tail[..end])
}

fn initial_reply(language: &str, top: bool, force: bool) -> Reply {
    let today = today_ordinal();
    let now = lapinux_core::util::now_unix();
    let path = top_cache_path();
    if top && !force {
        if let Some(releases) = load_top_cache(&path, language, today, now, false) {
            return Ok(PageReply {
                releases,
                cursors: Vec::new(),
                error: None,
            });
        }
    }
    let reply = fetch_initial_reply(language, top);
    if top {
        match &reply {
            Ok(page) if page.error.is_none() && !page.releases.is_empty() => {
                let _ = save_top_cache(&path, language, &page.releases, now);
            }
            _ => {
                if let Some(releases) = load_top_cache(&path, language, today, now, true) {
                    return Ok(PageReply {
                        releases,
                        cursors: Vec::new(),
                        error: Some(lapinux_core::i18n::t("news.calendar.top_cached").to_string()),
                    });
                }
            }
        }
    }
    reply
}

fn fetch_initial_reply(language: &str, top: bool) -> Reply {
    let today = today_ordinal();
    let mut releases = Vec::new();
    let mut cursors = Vec::new();
    let mut errors = Vec::new();

    let (mut steam, steam_cursor, steam_errors) = steam_initial(language, today, top);
    releases.append(&mut steam);
    if let Some(cursor) = steam_cursor {
        cursors.push(cursor);
    }
    errors.extend(steam_errors);

    let (mut epic, epic_cursor, epic_errors) = epic_initial(language, today);
    releases.append(&mut epic);
    if let Some(cursor) = epic_cursor {
        cursors.push(cursor);
    }
    errors.extend(epic_errors);

    let (mut gog, gog_cursor, gog_errors) = gog_initial(language, today, top);
    releases.append(&mut gog);
    if let Some(cursor) = gog_cursor {
        cursors.push(cursor);
    }
    errors.extend(gog_errors);

    let (mut itch, itch_errors) = itch_initial(language);
    releases.append(&mut itch);
    errors.extend(itch_errors);

    merge_releases(&mut releases);
    if releases.is_empty() {
        if errors.is_empty() {
            errors.push(lapinux_core::i18n::t("news.calendar.empty").to_string());
        }
        return Err(errors.join(" · "));
    }
    let error = (!errors.is_empty()).then(|| errors.join(" · "));
    Ok(PageReply {
        releases,
        cursors,
        error,
    })
}

fn more_reply(language: &str, cursors: Vec<Cursor>, top: bool) -> Reply {
    let today = today_ordinal();
    let mut releases = Vec::new();
    let mut next_cursors = Vec::new();
    let mut errors = Vec::new();
    for cursor in cursors {
        if !cursor.has_more() {
            next_cursors.push(cursor);
            continue;
        }
        match fetch_more(cursor, language, today, top) {
            Ok((mut items, next)) => {
                releases.append(&mut items);
                next_cursors.push(next);
            }
            Err(error) => {
                errors.push(error);
                next_cursors.push(cursor);
            }
        }
    }
    merge_releases(&mut releases);
    if releases.is_empty() && !errors.is_empty() {
        return Err(errors.join(" · "));
    }
    let error = (!errors.is_empty()).then(|| errors.join(" · "));
    Ok(PageReply {
        releases,
        cursors: next_cursors,
        error,
    })
}

fn fetch_more(
    cursor: Cursor,
    language: &str,
    today: i64,
    top: bool,
) -> Result<(Vec<Release>, Cursor), String> {
    match cursor.source {
        Source::Steam => {
            let page = fetch_search(
                cursor.next_start,
                steam_upcoming_filter(top),
                steam_upcoming_sort(top),
                language,
            )?;
            let count = page.count;
            let total = page.total;
            let mut releases = page.releases;
            releases.retain(|release| release.ordinal >= today);
            Ok((
                releases,
                Cursor {
                    source: Source::Steam,
                    next_start: if count == 0 {
                        total
                    } else {
                        (cursor.next_start + count).min(total)
                    },
                    total,
                },
            ))
        }
        Source::Epic => {
            let page = fetch_epic_page(cursor.next_start, language)?;
            let count = page.count;
            let total = page.total;
            let mut releases = page.releases;
            releases.retain(|release| {
                release.ordinal >= today && release.ordinal <= today + EPIC_HORIZON_DAYS
            });
            Ok((
                releases,
                Cursor {
                    source: Source::Epic,
                    next_start: if count == 0 {
                        total
                    } else {
                        (cursor.next_start + count).min(total)
                    },
                    total,
                },
            ))
        }
        Source::Gog => {
            let page = fetch_gog_page(
                cursor.next_start,
                "upcoming",
                gog_upcoming_order(top),
                language,
            )?;
            let total = page.pages;
            let mut releases = page.releases;
            releases.retain(|release| release.ordinal >= today);
            Ok((
                releases,
                Cursor {
                    source: Source::Gog,
                    next_start: if cursor.next_start >= total {
                        total
                    } else {
                        cursor.next_start + 1
                    },
                    total,
                },
            ))
        }
    }
}

fn steam_upcoming_filter(top: bool) -> &'static str {
    if top {
        "popularcomingsoon"
    } else {
        "comingsoon"
    }
}

fn steam_upcoming_sort(top: bool) -> &'static str {
    if top {
        "Relevance"
    } else {
        "Released_ASC"
    }
}

fn gog_upcoming_order(top: bool) -> &'static str {
    if top {
        "desc:popularity"
    } else {
        "asc:releaseDate"
    }
}

fn steam_initial(
    language: &str,
    today: i64,
    top: bool,
) -> (Vec<Release>, Option<Cursor>, Vec<String>) {
    if top {
        return match steam_top_initial(language, today) {
            Ok(releases) => (releases, None, Vec::new()),
            Err(error) => (Vec::new(), None, vec![error]),
        };
    }

    let mut releases = Vec::new();
    let mut errors = Vec::new();
    match fetch_today(language, today) {
        Ok(today) => releases.extend(today),
        Err(error) => errors.push(error),
    }

    let mut next_start = 0;
    let mut total = 0;
    for _ in 0..INITIAL_PAGES {
        match fetch_search(
            next_start,
            steam_upcoming_filter(top),
            steam_upcoming_sort(top),
            language,
        ) {
            Ok(page) => {
                total = page.total;
                let count = page.count;
                releases.extend(
                    page.releases
                        .into_iter()
                        .filter(|release| release.ordinal >= today),
                );
                next_start += count;
                if count == 0 {
                    next_start = total;
                    break;
                }
                if count < PAGE_SIZE {
                    break;
                }
            }
            Err(error) => {
                errors.push(error);
                break;
            }
        }
    }
    let cursor = (next_start < total).then_some(Cursor {
        source: Source::Steam,
        next_start,
        total,
    });
    (releases, cursor, errors)
}

fn steam_top_initial(language: &str, today: i64) -> Result<Vec<Release>, String> {
    let appids = fetch_steam_top_appids()?;
    if appids.is_empty() {
        return Err(lapinux_core::i18n::t("news.calendar.steam_top_empty").to_string());
    }
    fetch_steam_store_items(&appids, language, today)
}

fn fetch_steam_top_appids() -> Result<Vec<u32>, String> {
    let mut appids = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for page in 1..=STEAM_TOP_PAGES {
        let url = STEAM_TOP_WISHLIST.replace("{page}", &page.to_string());
        let html = match fetch_steam_top_text(&url) {
            Ok(html) => html,
            Err(_) if page > 1 => break,
            Err(error) => return Err(error),
        };
        let page_ids = parse_steam_top_appids(&html);
        if page_ids.is_empty() && page == 1 {
            return Err(lapinux_core::i18n::t("news.calendar.steam_top_empty").to_string());
        }
        for appid in page_ids {
            if seen.insert(appid) {
                appids.push(appid);
            }
        }
    }
    // Le classement des progressions complète le Top sans multiplier les
    // requêtes : une seule page tierce, et les appids déjà présents sont
    // ignorés. Une panne de cette source ne fait pas échouer le Top principal.
    if let Ok(html) = fetch_steam_top_text(STEAM_TOP_TRENDING) {
        for appid in parse_steam_top_appids(&html)
            .into_iter()
            .take(STEAM_TOP_TRENDING_LIMIT)
        {
            if seen.insert(appid) {
                appids.push(appid);
            }
        }
    }
    Ok(appids)
}

/// Le cache ne contient que des données déjà affichées. Il est écrit après une
/// lecture complète et relu avant toute requête : ouvrir le calendrier ne
/// réveille donc pas Steam si une liste récente existe.
fn top_cache_path() -> std::path::PathBuf {
    lapinux_core::paths::LapinuxPaths::discover()
        .cache_dir
        .join("news-calendar-top.json")
}

fn load_top_cache(
    path: &Path,
    language: &str,
    today: i64,
    now: i64,
    allow_stale: bool,
) -> Option<Vec<Release>> {
    let text = std::fs::read_to_string(path).ok()?;
    let cache: TopCache = serde_json::from_str(&text).ok()?;
    if cache.version != TOP_CACHE_VERSION || cache.language != language || cache.releases.is_empty()
    {
        return None;
    }
    let age = now.saturating_sub(cache.saved_at);
    if !allow_stale && age > TOP_CACHE_TTL {
        return None;
    }
    let mut releases = cache.releases;
    releases.retain(|release| release.ordinal >= today || release.ordinal == UNDATED_ORDINAL);
    (!releases.is_empty()).then_some(releases)
}

fn save_top_cache(path: &Path, language: &str, releases: &[Release], now: i64) -> Option<()> {
    let cache = TopCache {
        version: TOP_CACHE_VERSION,
        language: language.to_string(),
        saved_at: now,
        releases: releases.to_vec(),
    };
    let data = serde_json::to_vec(&cache).ok()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok()?;
    }
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, data).ok()?;
    std::fs::rename(&temporary, path).ok()?;
    Some(())
}

fn fetch_steam_top_text(url: &str) -> Result<String, String> {
    let response = ureq::get(url)
        .set("User-Agent", BROWSER_UA)
        .set(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .set("Accept-Language", "fr-FR,fr;q=0.9,en;q=0.8")
        .timeout(Duration::from_secs(25))
        .call()
        .map_err(|error| steam_error(&error.to_string()))?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(8 * 1024 * 1024)
        .read_to_end(&mut bytes)
        .map_err(|error| steam_error(&error.to_string()))?;
    Ok(String::from_utf8_lossy(&bytes).to_string())
}

fn parse_steam_top_appids(html: &str) -> Vec<u32> {
    let marker = "/store_item_assets/steam/apps/";
    let mut appids = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for chunk in html.split(marker).skip(1) {
        let digits: String = chunk
            .chars()
            .take_while(|character| character.is_ascii_digit())
            .collect();
        let Ok(appid) = digits.parse::<u32>() else {
            continue;
        };
        if appid > 0 && seen.insert(appid) {
            appids.push(appid);
        }
    }
    appids
}

fn fetch_steam_store_items(
    appids: &[u32],
    language: &str,
    today: i64,
) -> Result<Vec<Release>, String> {
    let ids: Vec<serde_json::Value> = appids
        .iter()
        .map(|appid| serde_json::json!({ "appid": appid }))
        .collect();
    let context_language = if language.starts_with("fr") {
        "french"
    } else {
        "english"
    };
    let input = serde_json::json!({
        "ids": ids,
        "context": {
            "language": context_language,
            "country_code": "FR",
            "steam_realm": 1
        },
        "data_request": {
            "include_assets": true,
            "include_basic_info": true,
            "include_release": true
        }
    });
    let query = percent_encode(&input.to_string());
    let url = format!(
        "{STEAM_STORE_ITEMS}?origin=https%3A%2F%2Fstore.steampowered.com&input_json={query}"
    );
    let response = ureq::get(&url)
        .set("User-Agent", BROWSER_UA)
        .set("Accept", "application/json")
        .timeout(Duration::from_secs(25))
        .call()
        .map_err(|error| steam_error(&error.to_string()))?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(8 * 1024 * 1024)
        .read_to_end(&mut bytes)
        .map_err(|error| steam_error(&error.to_string()))?;
    parse_steam_top_items(&bytes, today, appids)
}

fn parse_steam_top_items(bytes: &[u8], today: i64, appids: &[u32]) -> Result<Vec<Release>, String> {
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|error| {
        let detail = error.to_string();
        lapinux_core::i18n::t_args("news.calendar.invalid_response", &[("detail", &detail)])
    })?;
    let items = value
        .pointer("/response/store_items")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| lapinux_core::i18n::t("news.calendar.no_results").to_string())?;
    let mut releases = Vec::new();
    for item in items {
        let Some(appid) = item.get("appid").and_then(serde_json::Value::as_u64) else {
            continue;
        };
        let name = item
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        if appid == 0 || name.is_empty() || !is_game_like(name) {
            continue;
        }
        let release = item.get("release");
        let coming = item
            .get("is_coming_soon")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
            || release
                .and_then(|value| value.get("is_coming_soon"))
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
        let dated = release
            .and_then(|value| value.get("steam_release_date"))
            .and_then(serde_json::Value::as_i64)
            .and_then(date_from_stamp);
        let (ordinal, date) = match dated {
            Some((ordinal, date)) if ordinal >= today => (ordinal, date),
            Some(_) => continue,
            None if coming => (UNDATED_ORDINAL, String::new()),
            None => continue,
        };
        let image = steam_top_image(item, appid as u32);
        let url = item
            .get("store_url_path")
            .and_then(serde_json::Value::as_str)
            .filter(|path| !path.is_empty())
            .map(|path| format!("https://store.steampowered.com/{path}"))
            .unwrap_or_else(|| format!("https://store.steampowered.com/app/{appid}"));
        let rank = appids
            .iter()
            .position(|candidate| *candidate == appid as u32)
            .map(|index| index as u32 + 1);
        releases.push(Release {
            appid: appid as u32,
            store: Store::Steam,
            title: name.to_string(),
            image,
            url,
            ordinal,
            date,
            rank,
        });
    }
    Ok(releases)
}

fn steam_top_image(item: &serde_json::Value, appid: u32) -> String {
    let Some(assets) = item.get("assets") else {
        return format!("https://cdn.akamai.steamstatic.com/steam/apps/{appid}/header.jpg");
    };
    let format = assets
        .get("asset_url_format")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let file = assets
        .get("header")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            assets
                .get("small_capsule")
                .and_then(serde_json::Value::as_str)
        });
    match (format.is_empty(), file) {
        (false, Some(file)) => format!(
            "https://shared.akamai.steamstatic.com/store_item_assets/{}",
            format.replace("${FILENAME}", file)
        ),
        _ => format!("https://cdn.akamai.steamstatic.com/steam/apps/{appid}/header.jpg"),
    }
}

fn steam_error(detail: &str) -> String {
    lapinux_core::i18n::t_args("news.calendar.steam_error", &[("detail", detail)])
}

fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 2);
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn fetch_today(language: &str, today: i64) -> Result<Vec<Release>, String> {
    let mut releases = Vec::new();
    let mut start = 0;
    for _ in 0..TODAY_PAGES {
        let page = fetch_search(start, "released", "Released_DESC", language)?;
        let count = page.count;
        let last = page.releases.last().map(|release| release.ordinal);
        releases.extend(
            page.releases
                .into_iter()
                .filter(|release| release.ordinal == today),
        );
        start += count;
        if count < PAGE_SIZE || last.is_some_and(|ordinal| ordinal < today) {
            break;
        }
    }
    Ok(releases)
}

/// Premières pages Epic : la requête est déjà bornée à partir d'aujourd'hui,
/// donc les fiches arrivent dans l'ordre des sorties les plus proches.
fn epic_initial(language: &str, today: i64) -> (Vec<Release>, Option<Cursor>, Vec<String>) {
    let mut releases = Vec::new();
    let mut errors = Vec::new();
    let mut next_start = 0;
    let mut total = 0;
    for _ in 0..INITIAL_PAGES {
        match fetch_epic_page(next_start, language) {
            Ok(page) => {
                total = page.total;
                let count = page.count;
                releases.extend(page.releases);
                next_start += count;
                if count == 0 {
                    next_start = total;
                    break;
                }
                if next_start >= total {
                    break;
                }
            }
            Err(error) => {
                errors.push(error);
                break;
            }
        }
    }
    releases
        .retain(|release| release.ordinal >= today && release.ordinal <= today + EPIC_HORIZON_DAYS);
    let cursor = (next_start < total).then_some(Cursor {
        source: Source::Epic,
        next_start,
        total,
    });
    (releases, cursor, errors)
}

fn gog_initial(
    language: &str,
    today: i64,
    top: bool,
) -> (Vec<Release>, Option<Cursor>, Vec<String>) {
    let mut releases = Vec::new();
    let mut errors = Vec::new();

    match fetch_gog_today(language, today) {
        Ok(today) => releases.extend(today),
        Err(error) => errors.push(error),
    }

    let mut pages = 0;
    let mut next_start;
    let mut page = 1;
    loop {
        match fetch_gog_page(page, "upcoming", gog_upcoming_order(top), language) {
            Ok(result) => {
                pages = result.pages;
                releases.extend(
                    result
                        .releases
                        .into_iter()
                        .filter(|release| release.ordinal >= today),
                );
                if page >= pages {
                    next_start = pages;
                    break;
                }
                if top && page >= GOG_TOP_PAGES {
                    next_start = page + 1;
                    break;
                }
                page += 1;
                next_start = page;
                if page > 20 {
                    break;
                }
            }
            Err(error) => {
                errors.push(error);
                next_start = page;
                break;
            }
        }
    }

    let cursor = (next_start < pages).then_some(Cursor {
        source: Source::Gog,
        next_start,
        total: pages,
    });
    (releases, cursor, errors)
}

fn itch_initial(language: &str) -> (Vec<Release>, Vec<String>) {
    let mut errors = Vec::new();
    let mut releases = Vec::new();
    for (url, dated) in [(ITCH_NEWEST, true), (ITCH_UPCOMING, false)] {
        match fetch_itch_feed(url, language) {
            Ok(bytes) => {
                let parsed = if dated {
                    parse_itch_feed(&bytes)
                } else {
                    parse_itch_upcoming(&bytes)
                };
                match parsed {
                    Ok(items) => {
                        if dated {
                            let today = today_ordinal();
                            releases.extend(
                                items.into_iter().filter(|release| release.ordinal == today),
                            );
                        } else {
                            releases.extend(items);
                        }
                    }
                    Err(error) => errors.push(error),
                }
            }
            Err(error) => errors.push(error),
        }
    }
    (releases, errors)
}

fn fetch_itch_feed(url: &str, language: &str) -> Result<Vec<u8>, String> {
    let response = ureq::get(url)
        .set("User-Agent", BROWSER_UA)
        .set(
            "Accept",
            "application/rss+xml, application/xml, text/xml, */*",
        )
        .set(
            "Accept-Language",
            if language.starts_with("fr") {
                "fr-FR,fr;q=0.9,en;q=0.8"
            } else {
                "en-US,en;q=0.9"
            },
        )
        .timeout(Duration::from_secs(25))
        .call()
        .map_err(|error| format!("{} : {error}", Store::Itch.label()))?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(8 * 1024 * 1024)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("{} : {error}", Store::Itch.label()))?;
    Ok(bytes)
}

/// Le flux public des nouveaux jeux itch.io donne une date de publication,
/// mais aucune date de sortie future. On ne peut donc y lire que le jour même.
fn parse_itch_feed(bytes: &[u8]) -> Result<Vec<Release>, String> {
    let text = String::from_utf8_lossy(bytes);
    if !text.contains("<item>") {
        return Err(lapinux_core::i18n::t("news.calendar.itch_no_results").to_string());
    }
    let mut releases = Vec::new();
    for item in text.split("<item>").skip(1) {
        let mut title = clean_text(&tag_text(item, "plainTitle"));
        if title.is_empty() {
            title = clean_text(&tag_text(item, "title"));
        }
        let url = clean_text(&tag_text(item, "link"));
        if title.is_empty() || !is_game_like(&title) || !url.starts_with("https://") {
            continue;
        }
        let Some((ordinal, date)) = date_from_stamp(parse_rss_date(&tag_text(item, "pubDate")))
        else {
            continue;
        };
        releases.push(Release {
            appid: lapinux_core::stores::store_appid(Store::Itch, &url),
            store: Store::Itch,
            title,
            image: clean_text(&tag_text(item, "imageurl")),
            url,
            ordinal,
            date,
            rank: None,
        });
    }
    Ok(releases)
}

/// Le tag `upcoming` d'itch.io est le seul signal public de jeux à venir. Son
/// flux « top » est classé par popularité, mais contient aussi des tests et des
/// pages très anciennes : on ne garde que les jeux avec visuel, sans mention de
/// test/démo, et publiés récemment. Ces jeux n'ont pas de date de sortie.
fn parse_itch_upcoming(bytes: &[u8]) -> Result<Vec<Release>, String> {
    let text = String::from_utf8_lossy(bytes);
    if !text.contains("<item>") {
        return Err(lapinux_core::i18n::t("news.calendar.itch_no_results").to_string());
    }
    let now = lapinux_core::util::now_unix();
    let mut releases = Vec::new();
    for item in text.split("<item>").skip(1) {
        let mut title = clean_text(&tag_text(item, "plainTitle"));
        if title.is_empty() {
            title = clean_text(&tag_text(item, "title"));
        }
        let description = clean_text(&tag_text(item, "description"));
        let url = clean_text(&tag_text(item, "link"));
        let image = clean_text(&tag_text(item, "imageurl"));
        if title.is_empty()
            || image.is_empty()
            || !is_game_like(&title)
            || !url.starts_with("https://")
            || !itch_upcoming_is_quality(
                &title,
                &description,
                parse_rss_date(&tag_text(item, "pubDate")),
                now,
            )
        {
            continue;
        }
        releases.push(Release {
            appid: lapinux_core::stores::store_appid(Store::Itch, &url),
            store: Store::Itch,
            title,
            image,
            url,
            ordinal: UNDATED_ORDINAL,
            date: String::new(),
            rank: None,
        });
    }
    Ok(releases)
}

fn itch_upcoming_is_quality(title: &str, description: &str, published: i64, now: i64) -> bool {
    let text = format!("{title} {description}").to_lowercase();
    const NOISE: [&str; 6] = [
        "playtest",
        "play test",
        "prototype",
        "demo",
        "test build",
        "placeholder",
    ];
    if NOISE.iter().any(|word| text.contains(word)) {
        return false;
    }
    if published > 0 && now.saturating_sub(published) / 86_400 > ITCH_UPCOMING_MAX_AGE_DAYS {
        return false;
    }
    true
}

fn fetch_epic_page(start: usize, language: &str) -> Result<EpicPage, String> {
    let locale = epic_locale(language);
    let release_date = format!("[{},2099-12-31]", today_iso());
    let body = serde_json::json!({
        "query": EPIC_CALENDAR_QUERY,
        "variables": {
            "country": "FR",
            "locale": locale,
            "count": EPIC_PAGE_SIZE,
            "start": start,
            "releaseDate": release_date,
            "sortBy": "releaseDate",
            "sortDir": "ASC",
        }
    });
    let referer = format!(
        "https://store.epicgames.com/{}/browse?onSale=false",
        epic_store_prefix(language)
    );
    let response = ureq::post(EPIC_GQL)
        .set("User-Agent", EPIC_UA)
        .set("Accept", "application/json")
        .set("Accept-Language", epic_accept_language(language))
        .set("Origin", "https://store.epicgames.com")
        .set("Referer", &referer)
        .set("Content-Type", "application/json")
        .timeout(Duration::from_secs(25))
        .send_string(&body.to_string())
        .map_err(|error| epic_error(&error.to_string()))?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(16 * 1024 * 1024)
        .read_to_end(&mut bytes)
        .map_err(|error| epic_error(&error.to_string()))?;
    parse_epic_page(&bytes, language)
}

fn parse_epic_page(bytes: &[u8], language: &str) -> Result<EpicPage, String> {
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|error| {
        let detail = error.to_string();
        lapinux_core::i18n::t_args(
            "news.calendar.epic_invalid_response",
            &[("detail", &detail)],
        )
    })?;
    if let Some(error) = value
        .get("errors")
        .and_then(serde_json::Value::as_array)
        .and_then(|errors| errors.first())
    {
        let detail = error
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown error");
        return Err(epic_error(detail));
    }
    let store = value
        .pointer("/data/Catalog/searchStore")
        .ok_or_else(|| lapinux_core::i18n::t("news.calendar.epic_no_results").to_string())?;
    let total = store
        .pointer("/paging/total")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as usize;
    let elements = store
        .get("elements")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| lapinux_core::i18n::t("news.calendar.epic_no_results").to_string())?;
    let prefix = epic_store_prefix(language);
    let mut releases = Vec::new();
    for element in elements {
        let title = element
            .get("title")
            .and_then(serde_json::Value::as_str)
            .map(clean_text)
            .unwrap_or_default();
        if title.is_empty() || !is_game_like(&title) || !epic_is_game(element) {
            continue;
        }
        let Some((ordinal, date)) = element
            .get("releaseDate")
            .and_then(serde_json::Value::as_str)
            .and_then(parse_epic_date)
        else {
            continue;
        };
        let slug = element
            .get("productSlug")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|slug| !slug.is_empty())
            .or_else(|| {
                element
                    .get("urlSlug")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|slug| !slug.is_empty())
            });
        let Some(slug) = slug else {
            continue;
        };
        let url = format!("https://store.epicgames.com/{prefix}/p/{slug}");
        releases.push(Release {
            appid: lapinux_core::stores::store_appid(Store::Epic, slug),
            store: Store::Epic,
            title,
            image: epic_image(element),
            url,
            ordinal,
            date,
            rank: None,
        });
    }
    Ok(EpicPage {
        releases,
        count: elements.len(),
        total,
    })
}

fn epic_is_game(element: &serde_json::Value) -> bool {
    let Some(categories) = element
        .get("categories")
        .and_then(serde_json::Value::as_array)
    else {
        return true;
    };
    let paths: Vec<&str> = categories
        .iter()
        .filter_map(|category| category.get("path").and_then(serde_json::Value::as_str))
        .collect();
    if paths.is_empty() {
        return true;
    }
    if paths
        .iter()
        .any(|path| path.starts_with("addons") || path.starts_with("digitalextras"))
    {
        return false;
    }
    paths.iter().any(|path| {
        *path == "games" || path.starts_with("games/") || path.starts_with("bundles/games")
    })
}

fn epic_image(element: &serde_json::Value) -> String {
    let Some(images) = element
        .get("keyImages")
        .and_then(serde_json::Value::as_array)
    else {
        return String::new();
    };
    for kind in ["OfferImageWide", "Thumbnail", "featuredMedia"] {
        if let Some(url) = images
            .iter()
            .find(|image| image.get("type").and_then(serde_json::Value::as_str) == Some(kind))
            .and_then(|image| image.get("url"))
            .and_then(serde_json::Value::as_str)
            .filter(|url| url.starts_with("https://"))
        {
            return url.to_string();
        }
    }
    String::new()
}

fn parse_epic_date(text: &str) -> Option<(i64, String)> {
    let head = text.get(..10)?;
    let mut parts = head.split('-');
    let year = parts.next()?.parse::<i32>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let day = parts.next()?.parse::<u32>().ok()?;
    if !(1970..=9999).contains(&year)
        || !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || parts.next().is_some()
    {
        return None;
    }
    Some((
        days_from_civil(year, month, day),
        format!("{day:02}/{month:02}/{year:04}"),
    ))
}

fn today_iso() -> String {
    let text = lapinux_core::util::format_date(lapinux_core::util::now_unix());
    let mut parts = text.split('/');
    let day = parts.next().and_then(|value| value.parse::<u32>().ok());
    let month = parts.next().and_then(|value| value.parse::<u32>().ok());
    let year = parts.next().and_then(|value| value.parse::<i32>().ok());
    match (day, month, year) {
        (Some(day), Some(month), Some(year)) => {
            format!("{year:04}-{month:02}-{day:02}")
        }
        _ => "1970-01-01".to_string(),
    }
}

fn epic_store_prefix(language: &str) -> &'static str {
    if language.starts_with("fr") {
        "fr"
    } else {
        "en-US"
    }
}

fn epic_locale(language: &str) -> &'static str {
    if language.starts_with("fr") {
        "fr-FR"
    } else {
        "en-US"
    }
}

fn epic_accept_language(language: &str) -> &'static str {
    if language.starts_with("fr") {
        "fr-FR,fr;q=0.9,en;q=0.8"
    } else {
        "en-US,en;q=0.9"
    }
}

fn epic_error(detail: &str) -> String {
    lapinux_core::i18n::t_args("news.calendar.epic_error", &[("detail", detail)])
}

fn fetch_gog_today(language: &str, today: i64) -> Result<Vec<Release>, String> {
    let mut releases = Vec::new();
    for page in 1..=2 {
        let result = fetch_gog_page(page, "new-arrival", "desc:releaseDate", language)?;
        let last = result.releases.last().map(|release| release.ordinal);
        let count = result.releases.len();
        releases.extend(
            result
                .releases
                .into_iter()
                .filter(|release| release.ordinal == today),
        );
        if result.pages <= page
            || count < GOG_PAGE_SIZE
            || last.is_some_and(|ordinal| ordinal < today)
        {
            break;
        }
    }
    Ok(releases)
}

fn fetch_gog_page(
    page: usize,
    status: &str,
    order: &str,
    language: &str,
) -> Result<GogPage, String> {
    let locale = if language.starts_with("fr") {
        "fr-FR"
    } else {
        "en-US"
    };
    let url = GOG_CATALOG
        .replace("{page}", &page.to_string())
        .replace("{order}", order)
        .replace("{locale}", locale)
        .replace("{status}", status);
    let response = ureq::get(&url)
        .set("User-Agent", BROWSER_UA)
        .set("Accept", "application/json")
        .set(
            "Accept-Language",
            if language.starts_with("fr") {
                "fr-FR,fr;q=0.9,en;q=0.8"
            } else {
                "en-US,en;q=0.9"
            },
        )
        .timeout(Duration::from_secs(25))
        .call()
        .map_err(|error| format!("{} : {error}", Store::Gog.label()))?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(16 * 1024 * 1024)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("{} : {error}", Store::Gog.label()))?;
    parse_gog_page(&bytes)
}

fn parse_gog_page(bytes: &[u8]) -> Result<GogPage, String> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|error| format!("{} : {error}", Store::Gog.label()))?;
    let pages = value
        .get("pages")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as usize;
    let products = value
        .get("products")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| format!("{} : unexpected response", Store::Gog.label()))?;
    let mut releases = Vec::new();
    for product in products {
        let title = product
            .get("title")
            .and_then(serde_json::Value::as_str)
            .map(clean_text)
            .unwrap_or_default();
        if title.is_empty() || !is_game_like(&title) {
            continue;
        }
        let Some((ordinal, date)) = product
            .get("releaseDate")
            .and_then(serde_json::Value::as_str)
            .and_then(parse_gog_date)
        else {
            continue;
        };
        let Some(store_id) = product.get("id").and_then(|value| {
            value
                .as_str()
                .map(str::to_string)
                .or_else(|| value.as_u64().map(|id| id.to_string()))
        }) else {
            continue;
        };
        if store_id.is_empty() {
            continue;
        }
        let appid = lapinux_core::stores::store_appid(Store::Gog, &store_id);
        let image = product
            .get("coverHorizontal")
            .and_then(serde_json::Value::as_str)
            .or_else(|| {
                product
                    .get("coverVertical")
                    .and_then(serde_json::Value::as_str)
            })
            .unwrap_or_default()
            .to_string();
        let url = product
            .get("storeLink")
            .and_then(serde_json::Value::as_str)
            .filter(|url| !url.is_empty())
            .map(str::to_string)
            .or_else(|| {
                product
                    .get("slug")
                    .and_then(serde_json::Value::as_str)
                    .map(|slug| format!("https://www.gog.com/game/{slug}"))
            });
        let Some(url) = url else {
            continue;
        };
        releases.push(Release {
            appid,
            store: Store::Gog,
            title,
            image,
            url,
            ordinal,
            date,
            rank: None,
        });
    }
    Ok(GogPage { releases, pages })
}

fn parse_gog_date(text: &str) -> Option<(i64, String)> {
    let mut parts = text.split('.');
    let year = parts.next()?.parse::<i32>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let day = parts.next()?.parse::<u32>().ok()?;
    if !(1970..=9999).contains(&year)
        || !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || parts.next().is_some()
    {
        return None;
    }
    Some((
        days_from_civil(year, month, day),
        format!("{day:02}/{month:02}/{year:04}"),
    ))
}

fn fetch_search(
    start: usize,
    filter: &str,
    sort: &str,
    language: &str,
) -> Result<SearchPage, String> {
    let language = if language.starts_with("fr") {
        "french"
    } else {
        "english"
    };
    let url = SEARCH_URL
        .replace("{start}", &start.to_string())
        .replace("{count}", &PAGE_SIZE.to_string())
        .replace("{language}", language)
        .replace("{filter}", filter)
        .replace("{sort}", sort);
    let response = ureq::get(&url)
        .set("User-Agent", BROWSER_UA)
        .set("Accept", "application/json, text/javascript, */*; q=0.01")
        .set(
            "Accept-Language",
            if language == "french" {
                "fr-FR,fr;q=0.9,en;q=0.8"
            } else {
                "en-US,en;q=0.9"
            },
        )
        .set(
            "Referer",
            if filter == "released" {
                "https://store.steampowered.com/search/?filter=released"
            } else {
                "https://store.steampowered.com/search/?filter=comingsoon"
            },
        )
        .timeout(Duration::from_secs(25))
        .call()
        .map_err(|error| {
            let detail = error.to_string();
            lapinux_core::i18n::t_args("news.calendar.steam_error", &[("detail", &detail)])
        })?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(8 * 1024 * 1024)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            let detail = error.to_string();
            lapinux_core::i18n::t_args("news.calendar.steam_error", &[("detail", &detail)])
        })?;
    parse_search_page(&bytes)
}

fn merge_releases(releases: &mut Vec<Release>) {
    releases.sort_by(|left, right| {
        left.ordinal
            .cmp(&right.ordinal)
            .then_with(|| left.title.to_lowercase().cmp(&right.title.to_lowercase()))
            .then_with(|| left.appid.cmp(&right.appid))
    });
    let mut seen = std::collections::HashSet::new();
    releases.retain(|release| seen.insert(release.appid));
}

/// Jeu de données déterministe pour la géométrie de l'en-tête. La page ne
/// lance ici aucune requête : les deux groupes sont déjà prêts.
#[cfg(test)]
pub(super) fn snapshot_calendar() -> Calendar {
    let today = today_ordinal();
    let today_date = lapinux_core::util::format_date(lapinux_core::util::now_unix());
    let tomorrow_date = lapinux_core::util::format_date(lapinux_core::util::now_unix() + 86_400);
    let releases = vec![
        Release {
            appid: 1,
            store: Store::Steam,
            title: "Sortie du jour".to_string(),
            image: String::new(),
            url: "https://store.steampowered.com/app/1".to_string(),
            ordinal: today,
            date: today_date,
            rank: None,
        },
        Release {
            appid: 2,
            store: Store::Steam,
            title: "Sortie de demain".to_string(),
            image: String::new(),
            url: "https://store.steampowered.com/app/2".to_string(),
            ordinal: today + 1,
            date: tomorrow_date,
            rank: None,
        },
    ];
    Calendar {
        releases,
        asked: true,
        loading: false,
        language: "fr".to_string(),
        cursors: vec![Cursor {
            source: Source::Steam,
            next_start: 300,
            total: 300,
        }],
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn french_and_english_dates_are_classified() {
        let (ordinal, label) = parse_release_date("10 nov. 2026").expect("date française");
        assert_eq!(label, "10/11/2026");
        assert_eq!(ordinal, days_from_civil(2026, 11, 10));
        let (ordinal, label) = parse_release_date("November 10, 2026").expect("date anglaise");
        assert_eq!(label, "10/11/2026");
        assert_eq!(ordinal, days_from_civil(2026, 11, 10));
        assert!(parse_release_date("Prochainement").is_none());
        assert!(parse_release_date("novembre 2026").is_none());
    }

    #[test]
    fn steam_fragment_keeps_dated_games_and_skips_demos() {
        let html = r#"{"success":true,"total_count":42,"results_html":"<a data-ds-appid=\"10\" href=\"https://store.steampowered.com/app/10\"><div class=\"search_capsule\"><img src=\"https://cdn.example/10.jpg\"></div><span class=\"title\">Un jeu — démo</span><div class=\"search_released responsive_secondrow\">\n 10 nov. 2026 </div></a><a data-ds-appid=\"11\"><span class=\"title\">Un jeu &amp; une démo</span><div class=\"search_released\">11 nov. 2026</div></a><a data-ds-appid=\"12\"><span class=\"title\">Jeu prochainement</span><div class=\"search_released\">Prochainement</div></a>"}"#;
        let page = parse_search_page(html.as_bytes()).expect("fragment Steam");
        assert_eq!(page.total, 42);
        assert_eq!(
            page.releases.len(),
            0,
            "les démos et dates imprécises sont écartées"
        );
        let html = html.replace("Un jeu — démo", "Un jeu daté");
        let page = parse_search_page(html.as_bytes()).expect("fragment Steam");
        assert_eq!(page.releases.len(), 1);
        assert_eq!(page.releases[0].title, "Un jeu daté");
        assert_eq!(page.releases[0].date, "10/11/2026");
        assert_eq!(page.releases[0].appid, 10);
        assert_eq!(page.releases[0].store, Store::Steam);
    }

    #[test]
    fn full_mode_uses_dated_steam_and_gog() {
        assert_eq!(steam_upcoming_filter(false), "comingsoon");
        assert_eq!(steam_upcoming_sort(false), "Released_ASC");
        assert_eq!(gog_upcoming_order(false), "asc:releaseDate");
    }

    #[test]
    fn steam_top_ranking_reads_steam_appids_in_order() {
        let html = r#"<img src="https://shared.akamai.steamstatic.com/store_item_assets/steam/apps/1422450/deadlock.jpg"><img src="https://shared.akamai.steamstatic.com/store_item_assets/steam/apps/1422450/deadlock2.jpg"><img src="https://shared.akamai.steamstatic.com/store_item_assets/steam/apps/2769570/fable.jpg">"#;
        assert_eq!(parse_steam_top_appids(html), vec![1422450, 2769570]);
    }

    #[test]
    fn steam_top_items_keep_upcoming_and_tba_games() {
        let json = r#"{"response":{"store_items":[
            {"appid":1,"success":1,"visible":true,"name":"Fable","store_url_path":"app/1/Fable","release":{"steam_release_date":1803398400,"is_coming_soon":true},"assets":{"asset_url_format":"steam/apps/1/${FILENAME}?t=1","header":"header.jpg"}},
            {"appid":2,"success":1,"visible":true,"name":"TBA","store_url_path":"app/2/TBA","release":{"is_coming_soon":true,"custom_release_date_message":"À déterminer"}},
            {"appid":3,"success":1,"visible":true,"name":"Déjà sorti","release":{"steam_release_date":1600000000}}
        ]}}"#;
        let releases =
            parse_steam_top_items(json.as_bytes(), days_from_civil(2026, 11, 10), &[1, 2])
                .expect("fiches Steam");
        assert_eq!(releases.len(), 2);
        assert_eq!(releases[0].title, "Fable");
        assert_eq!(releases[0].rank, Some(1));
        assert_eq!(releases[1].rank, Some(2));
        assert!(releases[0].ordinal >= days_from_civil(2026, 11, 10));
        assert_eq!(releases[1].ordinal, UNDATED_ORDINAL);
        assert!(releases[0].image.contains("header.jpg"));
    }

    #[test]
    fn top_cache_expires_after_six_hours_but_keeps_a_stale_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("news-calendar-top.json");
        let today = days_from_civil(2026, 11, 10);
        let releases = vec![Release {
            appid: 1,
            store: Store::Steam,
            title: "Jeu en cache".into(),
            image: String::new(),
            url: String::new(),
            ordinal: today + 2,
            date: "12/11/2026".into(),
            rank: Some(1),
        }];
        save_top_cache(&path, "fr", &releases, 1_000).unwrap();
        assert!(load_top_cache(&path, "fr", today, 1_000 + TOP_CACHE_TTL, false).is_some());
        assert!(load_top_cache(&path, "fr", today, 1_000 + TOP_CACHE_TTL + 1, false).is_none());
        assert!(load_top_cache(&path, "fr", today, 1_000 + TOP_CACHE_TTL + 1, true).is_some());
        assert!(load_top_cache(&path, "en", today, 1_000, false).is_none());
    }

    #[test]
    fn releases_are_sorted_by_day_and_title() {
        let mut releases = vec![
            Release {
                appid: 2,
                store: Store::Steam,
                title: "B".into(),
                image: String::new(),
                url: String::new(),
                ordinal: days_from_civil(2026, 11, 12),
                date: "12/11/2026".into(),
                rank: None,
            },
            Release {
                appid: 1,
                store: Store::Epic,
                title: "A".into(),
                image: String::new(),
                url: String::new(),
                ordinal: days_from_civil(2026, 11, 10),
                date: "10/11/2026".into(),
                rank: None,
            },
        ];
        merge_releases(&mut releases);
        assert_eq!(releases[0].title, "A");
        assert_eq!(releases[1].title, "B");
    }

    #[test]
    fn gog_products_are_read_with_dates_and_pages() {
        let json = r#"{"pages":7,"products":[{"id":"42","title":"Une sortie GOG","releaseDate":"2026.11.10","coverHorizontal":"https://images.example/cover.png","storeLink":"https://www.gog.com/fr/game/une_sortie"}]}"#;
        let page = parse_gog_page(json.as_bytes()).expect("réponse GOG");
        assert_eq!(page.pages, 7);
        assert_eq!(page.releases.len(), 1);
        assert_eq!(
            page.releases[0].appid,
            lapinux_core::stores::store_appid(Store::Gog, "42")
        );
        assert_eq!(page.releases[0].date, "10/11/2026");
        assert_eq!(page.releases[0].image, "https://images.example/cover.png");
        assert_eq!(page.releases[0].store, Store::Gog);
    }

    #[test]
    fn epic_calendar_products_are_read_with_dates_and_slugs() {
        let json = r#"{"data":{"Catalog":{"searchStore":{"elements":[{"title":"Une sortie Epic","productSlug":"une-sortie/home","urlSlug":"abc","releaseDate":"2026-11-10T01:26:03.000Z","keyImages":[{"type":"OfferImageWide","url":"https://cdn.example/epic.jpg"}],"categories":[{"path":"games/edition/base"}]}],"paging":{"count":1,"total":42}}}}}"#;
        let page = parse_epic_page(json.as_bytes(), "fr").expect("réponse Epic");
        assert_eq!(page.total, 42);
        assert_eq!(page.count, 1);
        assert_eq!(page.releases.len(), 1);
        assert_eq!(page.releases[0].title, "Une sortie Epic");
        assert_eq!(page.releases[0].date, "10/11/2026");
        assert_eq!(
            page.releases[0].url,
            "https://store.epicgames.com/fr/p/une-sortie/home"
        );
        assert_eq!(page.releases[0].image, "https://cdn.example/epic.jpg");
        assert_ne!(page.releases[0].appid & 0x8000_0000, 0);
        assert_eq!(page.releases[0].store, Store::Epic);
    }

    #[test]
    fn itch_newest_feed_reads_publication_dates() {
        let pub_date = "Tue, 10 Nov 2026 22:12:29 GMT";
        let expected = date_from_stamp(parse_rss_date(pub_date)).expect("date itch");
        let xml = format!(
            r#"<?xml version="1.0"?><rss><channel><item><plainTitle>Un jeu itch</plainTitle><link>https://studio.itch.io/jeu</link><imageurl>https://img.itch.zone/cover.png</imageurl><pubDate>{pub_date}</pubDate></item></channel></rss>"#
        );
        let releases = parse_itch_feed(xml.as_bytes()).expect("flux itch");
        assert_eq!(releases.len(), 1);
        assert_eq!(releases[0].title, "Un jeu itch");
        assert_eq!(releases[0].store, Store::Itch);
        assert_eq!(
            releases[0].appid,
            lapinux_core::stores::store_appid(Store::Itch, "https://studio.itch.io/jeu")
        );
        assert_eq!(releases[0].ordinal, expected.0);
        assert_eq!(releases[0].date, expected.1);
        assert_eq!(releases[0].image, "https://img.itch.zone/cover.png");
    }

    #[test]
    fn itch_upcoming_feed_is_undated_and_filters_noise() {
        let xml = r#"<?xml version="1.0"?><rss><channel>
            <item><plainTitle>Projet soigné</plainTitle><link>https://studio.itch.io/projet</link><imageurl>https://img.itch.zone/projet.png</imageurl><description>Un vrai jeu à suivre</description></item>
            <item><plainTitle>Play Test</plainTitle><link>https://studio.itch.io/test</link><imageurl>https://img.itch.zone/test.png</imageurl><description>Une version de test</description></item>
            <item><plainTitle>Vieux jeu</plainTitle><link>https://studio.itch.io/vieux</link><imageurl>https://img.itch.zone/vieux.png</imageurl><description>Ancien prototype</description><pubDate>Fri, 01 Jan 2010 00:00:00 GMT</pubDate></item>
        </channel></rss>"#;
        let releases = parse_itch_upcoming(xml.as_bytes()).expect("flux upcoming");
        assert_eq!(releases.len(), 1);
        assert_eq!(releases[0].title, "Projet soigné");
        assert_eq!(releases[0].store, Store::Itch);
        assert_eq!(releases[0].ordinal, UNDATED_ORDINAL);
        assert!(releases[0].date.is_empty());
        assert_eq!(releases[0].image, "https://img.itch.zone/projet.png");
        assert_eq!(
            grouped(&releases, today_ordinal())
                .last()
                .map(|(ordinal, _)| *ordinal),
            Some(UNDATED_ORDINAL)
        );
    }
}
