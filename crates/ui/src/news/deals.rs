//! Section **Actualités › Soldes** : les promotions du moment chez Steam,
//! GOG, Epic et itch.io. Les sources sont lues en tâche de fond ; la page garde
//! les promotions déjà reçues pendant une relecture.

use super::gaming::{clean_text, days_from_civil, decode_entities, parse_rss_date, tag_text};
use super::{
    card_title_chars, columns, cover_slot, filter_choice, grid_card, grid_card_height_for,
    grid_card_width, grid_image_height, hero_image_height, overlay_badge, pill, price_cents,
    skeleton_content_compact, source_menu, status_card, store_color, store_logo, CardContent,
    CardPrice, CARD_PADDING, CARD_RADIUS, GRID_BODY_HEIGHT, GRID_GAP, HERO_BODY_HEIGHT,
    NEWS_FILTER_WIDTH,
};
use crate::gameinfo::NewsArt;
use crate::{ACCENT, CARD, DANGER, OK, TEXT, TEXT_DIM};
use eframe::egui;
use lapinux_core::client::Client;
use lapinux_core::ipc::Req;
use lapinux_core::stores::{FreeGame, Store};
use std::collections::HashSet;
use std::io::Read;
use std::sync::mpsc;
use std::time::Duration;

type Reply = Result<Vec<Deal>, String>;

/// Agent d'un navigateur : Steam et itch refusent les requêtes nues.
const BROWSER_UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36";
/// Le GraphQL public de la vitrine refuse l'agent d'un navigateur (403) et
/// répond à celui du launcher ; c'est aussi celui qu'utilise la fiche Epic.
const EPIC_UA: &str = "EpicGamesLauncher/14.0.8-22004686+++Portal+Release-Live";
const STEAM_FEATURED: &str =
    "https://store.steampowered.com/api/featuredcategories?cc=fr&l={language}";
const STEAM_SEARCH: &str = "https://store.steampowered.com/search/results/?query&start={start}&count=50&specials=1&infinite=1&cc=fr&l={language}";
const GOG_SALES: &str = "https://catalog.gog.com/v1/catalog?limit=120&order=desc:trending&productType=in:game&price=discounted&countryCode=FR&locale={locale}&currencyCode=EUR";
const ITCH_SALES: &str = "https://itch.io/games/{language}/on-sale.xml";
const EPIC_GQL: &str = "https://store.epicgames.com/graphql";
const EPIC_SALES_QUERY: &str = r#"query searchStoreQuery($country: String!, $locale: String, $count: Int, $start: Int, $onSale: Boolean) {
  Catalog {
    searchStore(country: $country, locale: $locale, count: $count, start: $start, onSale: $onSale) {
      elements {
        title
        urlSlug
        price(country: $country) {
          totalPrice {
            originalPrice
            discountPrice
            fmtPrice(locale: $locale) {
              originalPrice
              discountPrice
            }
          }
        }
        keyImages { type url }
        promotions {
          promotionalOffers {
            promotionalOffers {
              endDate
            }
          }
        }
      }
    }
  }
}"#;

/// Une promotion, quelle que soit la boutique. Les prix sont déjà formatés :
/// Steam les donne en centimes, GOG et Epic en chaînes localisées.
#[derive(Clone, Debug)]
struct Deal {
    store: Store,
    title: String,
    url: String,
    image: String,
    /// Pourcentage de réduction ; 100 pour un jeu offert.
    discount: u32,
    old_price: String,
    new_price: String,
    /// Fin de la promotion (secondes Unix), 0 quand la boutique ne la publie pas.
    ends: i64,
    free: bool,
}

#[derive(Default)]
pub(crate) struct Deals {
    items: Vec<Deal>,
    art: NewsArt,
    loading: bool,
    asked: bool,
    /// Boutiques encore attendues : les premières promotions s'affichent
    /// sans attendre les deux autres.
    pending: usize,
    rx: Option<mpsc::Receiver<Reply>>,
    error: Option<String>,
    /// Boutiques décochées dans le menu. Vide = toutes les boutiques.
    hidden_stores: HashSet<Store>,
    language: String,
    /// Adresse à ouvrir dans la webview Lapinux après un clic sur une carte.
    webview: Option<(String, String)>,
}

impl Deals {
    /// Prépare la première lecture sans bloquer l'image. Le cache de la page
    /// survit aux changements d'onglet.
    pub(crate) fn ensure(&mut self, client: &Client, language: &str) {
        if self.asked && self.language == language {
            return;
        }
        self.asked = true;
        self.language = language.to_string();
        self.items.clear();
        self.hidden_stores.clear();
        self.error = None;
        self.rx = None;
        self.loading = false;
        self.pending = 0;
        self.load(client);
    }

    pub(crate) fn refresh(&mut self, client: &Client) {
        if self.loading {
            return;
        }
        self.load(client);
    }

    pub(crate) fn loading(&self) -> bool {
        self.loading
    }

    fn load(&mut self, client: &Client) {
        if self.rx.is_some() {
            return;
        }
        let language = self.language.clone();
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        self.loading = true;
        self.pending = 4;
        self.error = None;
        let worker = client.detached();
        let tx_steam = tx.clone();
        let steam_language = language.clone();
        std::thread::spawn(move || {
            let _ = tx_steam.send(fetch_steam(&steam_language));
        });
        let tx_gog = tx.clone();
        let gog_language = language.clone();
        std::thread::spawn(move || {
            let _ = tx_gog.send(fetch_gog(&gog_language));
        });
        let tx_itch = tx.clone();
        let itch_language = language.clone();
        std::thread::spawn(move || {
            let _ = tx_itch.send(fetch_itch(&itch_language));
        });
        std::thread::spawn(move || {
            let _ = tx.send(fetch_epic(&worker, &language));
        });
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let mut received = Vec::new();
        let mut disconnected = false;
        if let Some(rx) = &self.rx {
            loop {
                match rx.try_recv() {
                    Ok(reply) => received.push(reply),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }
        let got = !received.is_empty();
        let mut errors = Vec::new();
        for reply in received {
            self.pending = self.pending.saturating_sub(1);
            match reply {
                Ok(mut items) => {
                    self.items.append(&mut items);
                    self.settle();
                }
                Err(error) => errors.push(error),
            }
        }
        if self.rx.is_some() && (disconnected || self.pending == 0) {
            self.settle();
            self.rx = None;
            self.loading = false;
            self.error = if self.items.is_empty() {
                Some(if errors.is_empty() {
                    lapinux_core::i18n::t("news.deals.empty").to_string()
                } else {
                    errors.join(" · ")
                })
            } else {
                None
            };
        }
        if got {
            ctx.request_repaint();
        } else if self.loading {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }

    /// Garde l'ordre de lecture et retire les doublons qui peuvent arriver
    /// après une seconde lecture d'une même boutique.
    fn settle(&mut self) {
        self.items.sort_by(|left, right| {
            right
                .free
                .cmp(&left.free)
                .then_with(|| right.discount.cmp(&left.discount))
                .then_with(|| left.title.to_lowercase().cmp(&right.title.to_lowercase()))
        });
        let mut seen = HashSet::new();
        self.items
            .retain(|deal| seen.insert((deal.store, deal.title.to_lowercase())));
    }

    pub(crate) fn page(&mut self, ui: &mut egui::Ui, client: &Client, language: &str) {
        self.ensure(client, language);
        self.poll(ui.ctx());
        self.art.poll(ui.ctx());

        let loading = self.loading && self.items.is_empty();
        let mut retry = false;
        let mut show_all = false;
        ui.add_space(8.0);

        let width = (ui.available_width() - 12.0).max(240.0);
        let filtered: Vec<&Deal> = self
            .items
            .iter()
            .filter(|deal| !self.hidden_stores.contains(&deal.store))
            .collect();
        let columns = columns(width).max(1);
        let card_width = grid_card_width(width);
        let card_height = grid_card_height_for(card_width, true);
        let grid = !loading && !self.items.is_empty() && !filtered.is_empty();
        let rows = if grid {
            filtered.len().div_ceil(columns)
        } else {
            1
        };
        // Une rangée vaut la carte et l'écart vertical : `show_rows` ne met en
        // page que la fenêtre visible, même avec plusieurs centaines d'offres.
        let row_height = if grid {
            card_height + GRID_GAP
        } else {
            ui.available_height().max(320.0)
        };
        egui::ScrollArea::vertical()
            .id_salt("news-deals")
            .auto_shrink([false, false])
            .show_rows(ui, row_height, rows, |ui, range| {
                ui.set_width(width);
                // La rangée porte elle-même son écart : `show_rows` connaît
                // ainsi exactement la hauteur qui sépare deux rangées.
                ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
                if loading {
                    skeleton_content_compact(ui, width);
                    return;
                }
                if self.items.is_empty() {
                    let error = self.error.as_deref();
                    let title = if error.is_some() {
                        lapinux_core::i18n::t("news.error.title")
                    } else {
                        lapinux_core::i18n::t("news.empty_state.title")
                    };
                    let message =
                        error.unwrap_or_else(|| lapinux_core::i18n::t("news.deals.empty"));
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
                        lapinux_core::i18n::t("news.deals.filter_empty"),
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
                        for deal in row {
                            let badge = deal_badge(deal);
                            let until = deal_until(deal);
                            // Comme les annonces, les cartes de la grille
                            // restent compactes : la carte entière ouvre l'offre.
                            let content = deal_content(
                                deal,
                                "",
                                &until,
                                lapinux_core::i18n::t("news.deals.free"),
                            );
                            let id = ui.id().with(("news-deal", deal.url.as_str()));
                            // Le visuel n'est demandé que pour les rangées
                            // visibles : des centaines d'offres ne remplissent
                            // pas le cache pour des images jamais regardées.
                            let card_rect = egui::Rect::from_min_size(
                                ui.cursor().min,
                                egui::vec2(card_width, card_height),
                            );
                            if ui.is_rect_visible(card_rect) && !deal.image.is_empty() {
                                self.art
                                    .ensure_still(ui.ctx(), std::slice::from_ref(&deal.image));
                            }
                            if grid_card(
                                ui,
                                card_width,
                                self.art.get(&deal.image),
                                None,
                                &deal.title,
                                Some((badge.0.as_str(), badge.1)),
                                id,
                                1.0,
                                &content,
                            ) {
                                self.webview = Some((
                                    deal.url.clone(),
                                    format!("{} · {} · Lapinux", deal.title, deal.store.label()),
                                ));
                            }
                        }
                    });
                    ui.add_space(GRID_GAP);
                }
            });

        if retry {
            self.refresh(client);
        }
        if show_all {
            self.hidden_stores.clear();
        }
    }

    /// Adresse à confier à la fenêtre WebKit de Lapinux, après un clic.
    pub(crate) fn take_webview(&mut self) -> Option<(String, String)> {
        self.webview.take()
    }

    /// Sélecteur de boutiques de l'en-tête. Les lignes barrées masquent une
    /// boutique ; leur logo vient du favicon, avec une pastille de secours.
    pub(super) fn source_picker(&mut self, ui: &mut egui::Ui) {
        let stores = [Store::Steam, Store::Gog, Store::Epic, Store::Itch];
        let counts =
            stores.map(|store| self.items.iter().filter(|deal| deal.store == store).count());
        let visible_stores = stores
            .iter()
            .filter(|store| !self.hidden_stores.contains(store))
            .count();
        let selected = if visible_stores == stores.len() {
            lapinux_core::i18n::t("news.deals.all_stores").to_string()
        } else if visible_stores == 0 {
            lapinux_core::i18n::t("news.deals.none").to_string()
        } else if visible_stores == 1 {
            stores
                .iter()
                .find(|store| !self.hidden_stores.contains(store))
                .map(|store| store.label().to_string())
                .unwrap_or_else(|| lapinux_core::i18n::t("news.deals.none").to_string())
        } else {
            lapinux_core::i18n::t_args(
                "news.deals.selected_stores",
                &[("count", &visible_stores.to_string())],
            )
            .to_string()
        };
        source_menu(
            ui,
            "news-deals-source",
            &selected,
            NEWS_FILTER_WIDTH,
            |ui| {
                ui.set_min_width(NEWS_FILTER_WIDTH);
                if !self.hidden_stores.is_empty() {
                    if ui.button(lapinux_core::i18n::t("news.show_all")).clicked() {
                        self.hidden_stores.clear();
                    }
                    ui.separator();
                }
                for (store, count) in stores.into_iter().zip(counts) {
                    let hidden = self.hidden_stores.contains(&store);
                    let logo = store_logo(ui.ctx(), store);
                    if filter_choice(
                        ui,
                        logo.as_ref(),
                        store.label(),
                        count,
                        hidden,
                        store_color(Some(store)),
                    ) {
                        if hidden {
                            self.hidden_stores.remove(&store);
                        } else {
                            self.hidden_stores.insert(store);
                        }
                    }
                }
            },
        );
    }
}

fn deal_badge(deal: &Deal) -> (String, egui::Color32) {
    if deal.free {
        (lapinux_core::i18n::t("news.deals.free").to_string(), OK)
    } else {
        (format!("-{} %", deal.discount), DANGER)
    }
}

fn deal_until(deal: &Deal) -> String {
    if deal.ends <= 0 {
        return String::new();
    }
    lapinux_core::i18n::t_args(
        "news.deals.until",
        &[("date", &lapinux_core::util::format_date(deal.ends))],
    )
    .to_string()
}

/// Échéance ISO 8601 d'Epic (`2026-11-17T17:00:00.000Z`) en secondes Unix.
/// Epic publie l'heure UTC ; un éventuel décalage n'est pas utilisé ici.
fn parse_iso_seconds(text: &str) -> i64 {
    let text = text.trim();
    if text.len() < 19 {
        return 0;
    }
    let year = text
        .get(0..4)
        .and_then(|part| part.parse::<i32>().ok())
        .unwrap_or(0);
    let month = text
        .get(5..7)
        .and_then(|part| part.parse::<u32>().ok())
        .unwrap_or(0);
    let day = text
        .get(8..10)
        .and_then(|part| part.parse::<u32>().ok())
        .unwrap_or(0);
    let hour = text
        .get(11..13)
        .and_then(|part| part.parse::<i64>().ok())
        .unwrap_or(0);
    let minute = text
        .get(14..16)
        .and_then(|part| part.parse::<i64>().ok())
        .unwrap_or(0);
    let second = text
        .get(17..19)
        .and_then(|part| part.parse::<i64>().ok())
        .unwrap_or(0);
    if year == 0 || month == 0 || day == 0 {
        return 0;
    }
    days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second
}

fn deal_content<'a>(
    deal: &'a Deal,
    action: &'a str,
    until: &'a str,
    free_label: &'a str,
) -> CardContent<'a> {
    CardContent {
        label: deal.store.label(),
        label_color: store_color(Some(deal.store)),
        store: Some(deal.store),
        meta_right: until,
        media_label: "",
        media_byline: None,
        title: &deal.title,
        excerpt: "",
        price: Some(CardPrice {
            current: if deal.free {
                free_label
            } else {
                &deal.new_price
            },
            old: &deal.old_price,
            free: deal.free,
        }),
        action,
    }
}

/// Une carte de promotion. Le bandeau porte la réduction ; le texte garde le
/// titre, la boutique et les deux prix. Cliquer ouvre la page de l'offre.
fn deal_card(
    ui: &mut egui::Ui,
    deal: &Deal,
    width: f32,
    image: Option<&egui::TextureHandle>,
    featured: bool,
) -> bool {
    let image_height = if featured {
        hero_image_height(width)
    } else {
        grid_image_height(width)
    };
    let body_height = if featured {
        HERO_BODY_HEIGHT
    } else {
        GRID_BODY_HEIGHT
    };
    let text_width = (width - CARD_PADDING * 2.0).max(80.0);
    let title = lapinux_core::util::ellipsize(&deal.title, card_title_chars(text_width, featured));
    let discount_color = if deal.free { OK } else { DANGER };
    let frame = egui::Frame::none()
        .fill(CARD())
        .stroke(egui::Stroke::new(
            1.0_f32,
            crate::BORDER().gamma_multiply(0.45),
        ))
        .rounding(egui::Rounding::same(CARD_RADIUS))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.set_width(width);
                let rect = cover_slot(ui, width, image_height, image, &deal.title);
                let badge = if deal.free {
                    lapinux_core::i18n::t("news.deals.free").to_string()
                } else {
                    format!("-{} %", deal.discount)
                };
                overlay_badge(ui, rect, &badge, discount_color);

                egui::Frame::none()
                    .inner_margin(egui::Margin::symmetric(CARD_PADDING, 10.0))
                    .show(ui, |ui| {
                        ui.set_width(text_width);
                        ui.set_min_height(body_height - 20.0);
                        ui.horizontal(|ui| {
                            pill(ui, deal.store.label(), store_color(Some(deal.store)));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if deal.ends > 0 {
                                        ui.label(
                                            egui::RichText::new(lapinux_core::i18n::t_args(
                                                "news.deals.until",
                                                &[(
                                                    "date",
                                                    &lapinux_core::util::format_date(deal.ends),
                                                )],
                                            ))
                                            .size(9.5)
                                            .color(TEXT_DIM()),
                                        );
                                    }
                                },
                            );
                        });
                        ui.add_space(6.0);
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(title)
                                    .size(if featured { 21.0 } else { 13.5 })
                                    .strong()
                                    .color(TEXT()),
                            )
                            .wrap(),
                        );
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            let price = if deal.free {
                                lapinux_core::i18n::t("news.deals.free").to_string()
                            } else {
                                deal.new_price.clone()
                            };
                            ui.label(
                                egui::RichText::new(price)
                                    .size(if featured { 20.0 } else { 15.0 })
                                    .strong()
                                    .color(if deal.free { OK } else { ACCENT() }),
                            );
                            if !deal.old_price.is_empty() {
                                ui.label(
                                    egui::RichText::new(&deal.old_price)
                                        .size(10.5)
                                        .strikethrough()
                                        .color(TEXT_DIM()),
                                );
                            }
                            if featured {
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "{}  →",
                                                lapinux_core::i18n::t("news.deals.view")
                                            ))
                                            .size(10.0)
                                            .color(ACCENT()),
                                        );
                                    },
                                );
                            }
                        });
                    });
            });
        });
    let rect = frame.response.rect;
    let response = ui.interact(
        rect,
        ui.id().with(("news-deal", deal.url.as_str())),
        egui::Sense::click(),
    );
    if response.hovered() {
        ui.painter().rect_stroke(
            rect,
            egui::Rounding::same(CARD_RADIUS),
            egui::Stroke::new(1.0_f32, crate::BORDER()),
        );
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.clicked()
}

/// Langue de la vitrine Steam et locale des réponses GOG/Epic. La devise et
/// le pays restent ceux du compte : seule la langue change.
fn deal_steam_language(language: &str) -> &'static str {
    if language == "fr" {
        "french"
    } else {
        "english"
    }
}

fn deal_locale(language: &str) -> &'static str {
    if language == "fr" {
        "fr-FR"
    } else {
        "en-US"
    }
}

fn deal_accept_language(language: &str) -> &'static str {
    if language == "fr" {
        "fr-FR,fr;q=0.9"
    } else {
        "en-US,en;q=0.9"
    }
}

fn epic_store_prefix(language: &str) -> &'static str {
    if language == "fr" {
        "fr"
    } else {
        "en-US"
    }
}

/// itch.io n'a pas de flux traduit : son filtre de langue porte sur les jeux
/// annoncés comme disponibles dans cette langue. C'est la meilleure sélection
/// possible pour les soldes de la boutique.
fn itch_language_path(language: &str) -> &'static str {
    if language == "fr" {
        "lang-fr"
    } else {
        "lang-en"
    }
}

fn fetch_steam(language: &str) -> Result<Vec<Deal>, String> {
    let mut deals = Vec::new();
    let mut errors = Vec::new();
    let steam_language = deal_steam_language(language);
    let featured = STEAM_FEATURED.replace("{language}", steam_language);
    match http_get(&featured) {
        Ok(bytes) => match steam_deals(&bytes) {
            Ok(mut items) => deals.append(&mut items),
            Err(error) => errors.push(format!("Steam : {error}")),
        },
        Err(error) => errors.push(format!("Steam : {error}")),
    }
    // La vitrine ne met en avant qu'une dizaine d'offres. La recherche
    // « specials » en donne cinquante ; une seule requête suffit et évite de
    // déclencher la limitation de Steam, qui refuse alors les suivantes.
    let url = STEAM_SEARCH
        .replace("{start}", "0")
        .replace("{language}", steam_language);
    match http_get_steam(&url, language) {
        Ok(bytes) => match steam_search_deals(&bytes) {
            Ok(mut items) => deals.append(&mut items),
            Err(error) => errors.push(format!("Steam : {error}")),
        },
        Err(error) => errors.push(format!("Steam : {error}")),
    }
    // Les résultats de recherche ne portent pas d'échéance. Quand la vitrine
    // connaît celle d'une même offre, on la reporte sur la ligne de recherche.
    let endings: std::collections::HashMap<String, i64> = deals
        .iter()
        .filter(|deal| deal.ends > 0)
        .map(|deal| (deal.url.clone(), deal.ends))
        .collect();
    for deal in &mut deals {
        if deal.ends == 0 {
            if let Some(end) = endings.get(&deal.url) {
                deal.ends = *end;
            }
        }
    }
    // La fiche ouverte dans la webview garde la langue choisie pour l'interface.
    for deal in &mut deals {
        if deal.store == Store::Steam {
            deal.url
                .push_str(if deal.url.contains('?') { "&" } else { "?" });
            deal.url.push_str(&format!("l={steam_language}"));
        }
    }
    if deals.is_empty() && !errors.is_empty() {
        Err(errors.join(" · "))
    } else {
        Ok(deals)
    }
}

fn fetch_gog(language: &str) -> Result<Vec<Deal>, String> {
    let url = GOG_SALES.replace("{locale}", deal_locale(language));
    let bytes = http_get(&url).map_err(|error| format!("GOG : {error}"))?;
    gog_deals(&bytes).map_err(|error| format!("GOG : {error}"))
}

fn fetch_itch(language: &str) -> Result<Vec<Deal>, String> {
    let url = ITCH_SALES.replace("{language}", itch_language_path(language));
    let bytes = http_get(&url).map_err(|error| format!("itch.io : {error}"))?;
    itch_deals(&bytes).map_err(|error| format!("itch.io : {error}"))
}

fn fetch_epic(client: &Client, language: &str) -> Result<Vec<Deal>, String> {
    let mut deals = Vec::new();
    let mut errors = Vec::new();
    match client.request_with_timeout(&Req::StoreFreeGames, Duration::from_secs(60)) {
        Ok(reply) => match serde_json::from_value::<Vec<FreeGame>>(reply.data) {
            Ok(games) => deals.extend(epic_deals(games)),
            Err(error) => errors.push(format!("Epic : {error}")),
        },
        Err(error) => errors.push(format!("Epic : {error}")),
    }
    match fetch_epic_sales(language) {
        Ok(mut sales) => deals.append(&mut sales),
        Err(error) => errors.push(format!("Epic : {error}")),
    }
    if deals.is_empty() && !errors.is_empty() {
        Err(errors.join(" · "))
    } else {
        Ok(deals)
    }
}

fn fetch_epic_sales(language: &str) -> Result<Vec<Deal>, String> {
    let locale = deal_locale(language);
    let body = serde_json::json!({
        "query": EPIC_SALES_QUERY,
        "variables": {
            "country": "FR",
            "locale": locale,
            "count": 40,
            "start": 0,
            "onSale": true,
        }
    });
    let referer = format!(
        "https://store.epicgames.com/{}/browse?onSale=true",
        epic_store_prefix(language)
    );
    let response = ureq::post(EPIC_GQL)
        .set("User-Agent", EPIC_UA)
        .set("Accept", "application/json")
        .set("Accept-Language", deal_accept_language(language))
        .set("Origin", "https://store.epicgames.com")
        .set("Referer", &referer)
        .set("Content-Type", "application/json")
        .timeout(Duration::from_secs(25))
        .send_string(&body.to_string())
        .map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    epic_sales(&value, language)
}

fn http_get(url: &str) -> Result<Vec<u8>, String> {
    let response = ureq::get(url)
        .set("User-Agent", BROWSER_UA)
        .set("Accept", "*/*")
        .timeout(Duration::from_secs(25))
        .call()
        .map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    Ok(bytes)
}

fn http_get_steam(url: &str, language: &str) -> Result<Vec<u8>, String> {
    let response = ureq::get(url)
        .set("User-Agent", BROWSER_UA)
        .set("Accept", "application/json, text/javascript, */*; q=0.01")
        .set("Accept-Language", deal_accept_language(language))
        .set(
            "Referer",
            "https://store.steampowered.com/search/?specials=1",
        )
        .timeout(Duration::from_secs(25))
        .call()
        .map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    Ok(bytes)
}

fn steam_deals(bytes: &[u8]) -> Result<Vec<Deal>, String> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    let Some(items) = value
        .pointer("/specials/items")
        .and_then(serde_json::Value::as_array)
    else {
        return Err(lapinux_core::i18n::t("news.deals.unexpected").to_string());
    };
    let mut deals = Vec::new();
    for item in items {
        let title = json_string(item.get("name"));
        let discount = item
            .get("discount_percent")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as u32;
        let id = item
            .get("id")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        if title.is_empty() || discount == 0 || id == 0 {
            continue;
        }
        let currency = json_string(item.get("currency"));
        let image = ["header_image", "large_capsule_image", "small_capsule_image"]
            .iter()
            .map(|key| json_string(item.get(*key)))
            .find(|url| url.starts_with("https://"))
            .unwrap_or_default();
        deals.push(Deal {
            store: Store::Steam,
            title,
            url: format!("https://store.steampowered.com/app/{id}"),
            image,
            discount,
            old_price: price_cents(
                item.get("original_price")
                    .and_then(serde_json::Value::as_u64),
                &currency,
            ),
            new_price: price_cents(
                item.get("final_price").and_then(serde_json::Value::as_u64),
                &currency,
            ),
            ends: item
                .get("discount_expiration")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0),
            free: false,
        });
    }
    Ok(deals)
}

/// La recherche du magasin Steam enveloppe le HTML des résultats dans du JSON.
/// Chaque ligne porte l'appid, le titre, les deux prix, la remise et la
/// capsule ; c'est la source qui donne le plus d'offres d'un coup.
fn steam_search_deals(bytes: &[u8]) -> Result<Vec<Deal>, String> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    let html = value
        .get("results_html")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if html.is_empty() {
        return Err(lapinux_core::i18n::t("news.deals.unexpected").to_string());
    }
    let mut deals = Vec::new();
    for block in html.split("data-ds-appid=\"").skip(1) {
        let Some(quote) = block.find('"') else {
            continue;
        };
        let Ok(id) = block[..quote].parse::<u64>() else {
            continue;
        };
        if id == 0 {
            continue;
        }
        let block = &block[quote..];
        let title = block
            .find("class=\"title\">")
            .and_then(|index| {
                let rest = &block[index + "class=\"title\">".len()..];
                let end = rest.find("</span>")?;
                Some(clean_text(&rest[..end]))
            })
            .unwrap_or_default();
        let discount = block
            .find("data-discount=\"")
            .and_then(|index| {
                let rest = &block[index + "data-discount=\"".len()..];
                let end = rest.find('"')?;
                rest[..end].parse::<u32>().ok()
            })
            .or_else(|| {
                block.find("search_discount").and_then(|index| {
                    let rest = &block[index..];
                    let percent = rest.find('%')?;
                    let digits: String = rest[..percent]
                        .chars()
                        .rev()
                        .take_while(|character| character.is_ascii_digit())
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect();
                    digits.parse::<u32>().ok()
                })
            })
            .unwrap_or(0);
        if title.is_empty() || discount == 0 {
            continue;
        }
        // La vitrine actuelle sépare les deux prix par des classes dédiées
        // (`discount_original_price` / `discount_final_price`). Les anciennes
        // pages utilisaient un `<strike>` : on garde ce repli.
        let price_text = |class: &str| -> String {
            block
                .find(class)
                .and_then(|index| {
                    let rest = &block[index + class.len()..];
                    let start = rest.find('>')? + 1;
                    let rest = &rest[start..];
                    let end = rest.find("</div>")?;
                    Some(clean_text(&rest[..end]))
                })
                .unwrap_or_default()
        };
        let fallback_price = block
            .find("search_price")
            .and_then(|index| {
                let rest = &block[index..];
                let start = rest.find('>')? + 1;
                let rest = &rest[start..];
                let end = rest.find("</div>").unwrap_or(rest.len());
                Some(&rest[..end])
            })
            .unwrap_or("");
        let old_price = {
            let dedicated = price_text("discount_original_price");
            if !dedicated.is_empty() {
                dedicated
            } else {
                fallback_price
                    .find("<strike>")
                    .and_then(|index| {
                        let rest = &fallback_price[index + "<strike>".len()..];
                        let end = rest.find("</strike>")?;
                        Some(clean_text(&rest[..end]))
                    })
                    .unwrap_or_default()
            }
        };
        let new_price = {
            let dedicated = price_text("discount_final_price");
            if !dedicated.is_empty() {
                dedicated
            } else if let Some(index) = fallback_price.find("</strike>") {
                let rest = fallback_price[index + "</strike>".len()..].trim_start();
                let rest = rest.strip_prefix("</span>").unwrap_or(rest);
                let end = rest.find("</div>").unwrap_or(rest.len());
                clean_text(&rest[..end])
            } else {
                clean_text(fallback_price)
            }
        };
        let image = block
            .find("<img src=\"")
            .and_then(|index| {
                let rest = &block[index + "<img src=\"".len()..];
                let end = rest.find('"')?;
                Some(decode_entities(&rest[..end]))
            })
            .unwrap_or_default();
        deals.push(Deal {
            store: Store::Steam,
            title,
            url: format!("https://store.steampowered.com/app/{id}"),
            image,
            discount,
            old_price,
            new_price: new_price.clone(),
            ends: 0,
            free: discount >= 100
                || new_price.eq_ignore_ascii_case("gratuit")
                || new_price.eq_ignore_ascii_case("free"),
        });
    }
    Ok(deals)
}

fn gog_deals(bytes: &[u8]) -> Result<Vec<Deal>, String> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    let Some(items) = value.get("products").and_then(serde_json::Value::as_array) else {
        return Err(lapinux_core::i18n::t("news.deals.unexpected").to_string());
    };
    let mut deals = Vec::new();
    for item in items {
        let title = json_string(item.get("title"));
        let price = item.get("price");
        let discount = price
            .and_then(|price| price.get("discount"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim_start_matches('-')
            .trim_end_matches('%')
            .parse::<u32>()
            .unwrap_or(0);
        let url = json_string(item.get("storeLink"));
        if title.is_empty() || discount == 0 || !url.starts_with("https://") {
            continue;
        }
        let Some(price) = price else { continue };
        let old_price = json_string(price.get("base")).replace('\u{a0}', " ");
        let new_price = json_string(price.get("final")).replace('\u{a0}', " ");
        deals.push(Deal {
            store: Store::Gog,
            title,
            url,
            image: json_string(item.get("coverHorizontal")),
            discount,
            old_price,
            new_price,
            ends: 0,
            free: false,
        });
    }
    Ok(deals)
}

fn epic_deals(games: Vec<FreeGame>) -> Vec<Deal> {
    games
        .into_iter()
        .filter(|game| {
            !game.upcoming
                && !game.owned
                && !game.title.trim().is_empty()
                && !game.url.trim().is_empty()
        })
        .map(|game| Deal {
            store: Store::Epic,
            title: game.title,
            url: game.url,
            image: game.cover,
            discount: 100,
            old_price: game.value,
            new_price: String::new(),
            ends: game.end,
            free: true,
        })
        .collect()
}

/// Le flux RSS officiel des jeux en promotion : chaque item donne le titre
/// nu, la remise, les deux prix, l'image et la fin de la solde.
fn itch_deals(bytes: &[u8]) -> Result<Vec<Deal>, String> {
    let text = String::from_utf8_lossy(bytes);
    if !text.contains("<item>") {
        return Err(lapinux_core::i18n::t("news.deals.unexpected").to_string());
    }
    let mut deals = Vec::new();
    for item in text.split("<item>").skip(1) {
        let title = clean_text(&tag_text(item, "plainTitle"));
        let url = clean_text(&tag_text(item, "link"));
        let discount = tag_text(item, "discountpercent")
            .trim()
            .parse::<u32>()
            .unwrap_or(0);
        if title.is_empty() || discount == 0 || !url.starts_with("https://") {
            continue;
        }
        deals.push(Deal {
            store: Store::Itch,
            title,
            url,
            image: clean_text(&tag_text(item, "imageurl")),
            discount,
            old_price: clean_text(&tag_text(item, "fullPrice")),
            new_price: clean_text(&tag_text(item, "price")),
            ends: parse_rss_date(&tag_text(item, "saleends")),
            free: discount >= 100,
        });
    }
    Ok(deals)
}

/// Promotions Epic : le GraphQL public de la vitrine répond sans jeton de
/// compte. Le pourcentage n'est pas publié, il se calcule sur les centimes.
fn epic_sales(value: &serde_json::Value, language: &str) -> Result<Vec<Deal>, String> {
    let Some(elements) = value
        .pointer("/data/Catalog/searchStore/elements")
        .and_then(serde_json::Value::as_array)
    else {
        return Err(lapinux_core::i18n::t("news.deals.unexpected").to_string());
    };
    let mut deals = Vec::new();
    for element in elements {
        let title = json_string(element.get("title"));
        let slug = json_string(element.get("urlSlug"));
        let total = element
            .get("price")
            .and_then(|price| price.get("totalPrice"));
        let original = total
            .and_then(|price| price.get("originalPrice"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let current = total
            .and_then(|price| price.get("discountPrice"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(original);
        if title.is_empty() || slug.is_empty() || original == 0 || current >= original {
            continue;
        }
        let discount = (((original - current) * 100) / original) as u32;
        if discount == 0 {
            continue;
        }
        let fmt = total.and_then(|price| price.get("fmtPrice"));
        let ends = element
            .pointer("/promotions/promotionalOffers/0/promotionalOffers/0/endDate")
            .and_then(serde_json::Value::as_str)
            .map(parse_iso_seconds)
            .unwrap_or(0);
        let image = element
            .get("keyImages")
            .and_then(serde_json::Value::as_array)
            .and_then(|images| {
                ["OfferImageWide", "featuredMedia", "Thumbnail"]
                    .iter()
                    .find_map(|kind| {
                        images
                            .iter()
                            .find(|image| {
                                image.get("type").and_then(serde_json::Value::as_str) == Some(*kind)
                            })
                            .and_then(|image| image.get("url"))
                            .and_then(serde_json::Value::as_str)
                            .filter(|url| url.starts_with("https://"))
                    })
            })
            .unwrap_or_default()
            .to_string();
        deals.push(Deal {
            store: Store::Epic,
            title,
            url: format!(
                "https://store.epicgames.com/{}/p/{slug}",
                epic_store_prefix(language)
            ),
            image,
            discount,
            old_price: json_string(fmt.and_then(|price| price.get("originalPrice"))),
            new_price: json_string(fmt.and_then(|price| price.get("discountPrice"))),
            ends,
            free: current == 0,
        });
    }
    Ok(deals)
}

fn json_string(value: Option<&serde_json::Value>) -> String {
    value
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Jeu de données déterministe pour les captures hors écran : les visuels
/// couvrent les trois boutiques, un prix barré, une date, un gratuit et une
/// vignette absente.
#[cfg(test)]
pub(super) fn snapshot_deals(ctx: &egui::Context) -> Deals {
    const COLORS: [(egui::Color32, egui::Color32); 8] = [
        (
            egui::Color32::from_rgb(36, 42, 88),
            egui::Color32::from_rgb(134, 62, 96),
        ),
        (
            egui::Color32::from_rgb(28, 72, 80),
            egui::Color32::from_rgb(68, 140, 110),
        ),
        (
            egui::Color32::from_rgb(92, 46, 32),
            egui::Color32::from_rgb(206, 132, 56),
        ),
        (
            egui::Color32::from_rgb(56, 36, 82),
            egui::Color32::from_rgb(158, 76, 190),
        ),
        (
            egui::Color32::from_rgb(32, 62, 92),
            egui::Color32::from_rgb(74, 152, 192),
        ),
        (
            egui::Color32::from_rgb(70, 42, 30),
            egui::Color32::from_rgb(174, 84, 64),
        ),
        (
            egui::Color32::from_rgb(46, 58, 34),
            egui::Color32::from_rgb(134, 176, 76),
        ),
        (
            egui::Color32::from_rgb(64, 38, 58),
            egui::Color32::from_rgb(196, 88, 132),
        ),
    ];
    let samples: [(&str, Store, u32, &str, &str, i64, bool); 8] = [
        (
            "Cyberpunk 2077: Ultimate Edition",
            Store::Steam,
            60,
            "35,99 €",
            "89,99 €",
            1_800_000_000,
            false,
        ),
        (
            "Baldur's Gate 3",
            Store::Gog,
            30,
            "41,99 €",
            "59,99 €",
            0,
            false,
        ),
        (
            "Hades II",
            Store::Steam,
            25,
            "22,49 €",
            "29,99 €",
            1_800_000_000,
            false,
        ),
        (
            "Alan Wake 2",
            Store::Epic,
            50,
            "24,99 €",
            "49,99 €",
            1_800_000_000,
            false,
        ),
        (
            "Disco Elysium: The Final Cut",
            Store::Gog,
            75,
            "9,99 €",
            "39,99 €",
            0,
            false,
        ),
        (
            "Control Ultimate Edition",
            Store::Epic,
            100,
            "",
            "29,99 €",
            1_800_000_000,
            true,
        ),
        (
            "Sea of Stars",
            Store::Steam,
            40,
            "20,39 €",
            "33,99 €",
            1_800_000_000,
            false,
        ),
        ("Tunic", Store::Gog, 50, "14,99 €", "29,99 €", 0, false),
    ];
    let mut items = Vec::new();
    let mut art = NewsArt::default();
    for (index, (title, store, discount, new_price, old_price, ends, free)) in
        samples.iter().enumerate()
    {
        let image = if index == 2 {
            String::new()
        } else {
            format!("snapshot://deal-{index}")
        };
        if !image.is_empty() {
            let (top, bottom) = COLORS[index];
            art.seed_cover(ctx, &image, top, bottom);
        }
        items.push(Deal {
            store: *store,
            title: (*title).to_string(),
            url: format!("https://example.com/deal/{index}"),
            image,
            discount: *discount,
            old_price: (*old_price).to_string(),
            new_price: (*new_price).to_string(),
            ends: *ends,
            free: *free,
        });
    }
    Deals {
        items,
        art,
        loading: false,
        asked: true,
        pending: 0,
        rx: None,
        error: None,
        hidden_stores: HashSet::new(),
        language: "fr".to_string(),
        webview: None,
    }
}

#[cfg(test)]
pub(super) fn snapshot_deals_loading() -> Deals {
    Deals {
        loading: true,
        asked: true,
        language: "fr".to_string(),
        ..Default::default()
    }
}

#[cfg(test)]
pub(super) fn snapshot_deals_error() -> Deals {
    Deals {
        asked: true,
        error: Some(lapinux_core::i18n::t("news.deals.empty").to_string()),
        language: "fr".to_string(),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_steam_promo_keeps_its_prices_and_badge() {
        let body = json!({
            "specials": {
                "items": [{
                    "id": 620,
                    "name": "Portal 2",
                    "discount_percent": 75,
                    "original_price": 999,
                    "final_price": 249,
                    "currency": "EUR",
                    "header_image": "https://cdn.example/portal.jpg",
                    "discount_expiration": 1_800_000_000
                }]
            }
        });
        let deals = steam_deals(body.to_string().as_bytes()).unwrap();
        assert_eq!(deals.len(), 1);
        assert_eq!(deals[0].store, Store::Steam);
        assert_eq!(deals[0].discount, 75);
        assert!(deals[0].new_price.contains('9') || deals[0].new_price.contains('4'));
        assert_eq!(deals[0].ends, 1_800_000_000);
    }

    #[test]
    fn a_gog_promo_reads_its_percentage() {
        let body = json!({
            "products": [{
                "title": "Disco Elysium",
                "storeLink": "https://www.gog.com/fr/game/disco_elysium",
                "coverHorizontal": "https://cdn.example/disco.png",
                "price": {
                    "final": "9,99\u{a0}€",
                    "base": "39,99\u{a0}€",
                    "discount": "-75%"
                }
            }]
        });
        let deals = gog_deals(body.to_string().as_bytes()).unwrap();
        assert_eq!(deals.len(), 1);
        assert_eq!(deals[0].discount, 75);
        assert_eq!(deals[0].new_price, "9,99 €");
        assert_eq!(deals[0].old_price, "39,99 €");
    }

    #[test]
    fn epic_freebies_are_free_deals() {
        let deals = epic_deals(vec![FreeGame {
            store: Store::Epic,
            id: "ns:offer".into(),
            title: "Un jeu offert".into(),
            description: String::new(),
            cover: "https://cdn.example/free.jpg".into(),
            url: "https://store.epicgames.com/fr/p/offer".into(),
            seller: String::new(),
            value: "19,99 €".into(),
            start: 0,
            end: 1_800_000_000,
            upcoming: false,
            platforms: Vec::new(),
            owned: false,
            browser_only: false,
            offers: Vec::new(),
            owned_offers: Vec::new(),
        }]);
        assert_eq!(deals.len(), 1);
        assert!(deals[0].free);
        assert_eq!(deals[0].discount, 100);
    }

    #[test]
    fn a_steam_search_row_becomes_a_deal() {
        let html = r#"<a href="https://store.steampowered.com/app/620/Portal_2/" data-ds-appid="620" data-ds-itemkey="App_620"><div class="search_capsule"><img src="https://cdn.example/portal.jpg"></div><div class="search_name"><span class="title">Portal 2</span></div><div class="search_price_discount_combined"><div class="search_discount_and_price"><div class="discount_block search_discount_block" data-discount="75"><div class="discount_pct">-75%</div><div class="discount_prices"><div class="discount_original_price">9,99€</div><div class="discount_final_price">2,49€</div></div></div></div></div></a>"#;
        let body = json!({ "results_html": html });
        let deals = steam_search_deals(body.to_string().as_bytes()).unwrap();
        assert_eq!(deals.len(), 1);
        assert_eq!(deals[0].title, "Portal 2");
        assert_eq!(deals[0].discount, 75);
        assert_eq!(deals[0].old_price, "9,99€");
        assert_eq!(deals[0].new_price, "2,49€");
        assert_eq!(deals[0].image, "https://cdn.example/portal.jpg");
    }

    #[test]
    fn an_itch_sale_becomes_a_deal() {
        let xml = br#"<rss><channel><item><plainTitle>Goodboy Galaxy (for GBA)</plainTitle><imageurl>https://img.itch.zone/cover.png</imageurl><price>$14.00</price><discountpercent>30</discountpercent><fullPrice>$20.00</fullPrice><saleends>Wed, 18 Nov 2026 22:59:00 GMT</saleends><link>https://goodboygalaxy.itch.io/goodboy-galaxy-gba</link></item></channel></rss>"#;
        let deals = itch_deals(xml).unwrap();
        assert_eq!(deals.len(), 1);
        assert_eq!(deals[0].store, Store::Itch);
        assert_eq!(deals[0].discount, 30);
        assert_eq!(deals[0].new_price, "$14.00");
        assert!(deals[0].ends > 0);
    }

    #[test]
    fn an_iso_sale_end_becomes_unix_seconds() {
        assert_eq!(parse_iso_seconds("2026-11-17T17:00:00.000Z"), 1_794_934_800);
        assert_eq!(parse_iso_seconds("pas une date"), 0);
    }

    #[test]
    fn an_epic_sale_reads_its_discount_and_wide_image() {
        let body = json!({
            "data": {"Catalog": {"searchStore": {"elements": [{
                "title": "Shrek 2 Remastered",
                "urlSlug": "shrek-2-remastered-01",
                "price": {"totalPrice": {
                    "originalPrice": 2999,
                    "discountPrice": 2699,
                    "fmtPrice": {"originalPrice": "29,99 €", "discountPrice": "26,99 €"}
                }},
                "keyImages": [
                    {"type": "Thumbnail", "url": "https://cdn.example/thumb.png"},
                    {"type": "OfferImageWide", "url": "https://cdn.example/wide.jpg"}
                ],
                "promotions": {"promotionalOffers": [{"promotionalOffers": [{
                    "endDate": "2026-11-17T17:00:00.000Z"
                }]}]}
            }]}}}
        });
        let deals = epic_sales(&body, "fr").unwrap();
        assert_eq!(deals.len(), 1);
        assert_eq!(deals[0].store, Store::Epic);
        assert_eq!(deals[0].discount, 10);
        assert_eq!(deals[0].new_price, "26,99 €");
        assert_eq!(deals[0].ends, 1_794_934_800);
        assert_eq!(deals[0].image, "https://cdn.example/wide.jpg");
        assert_eq!(
            deals[0].url,
            "https://store.epicgames.com/fr/p/shrek-2-remastered-01"
        );
        let english = epic_sales(&body, "en").unwrap();
        assert_eq!(
            english[0].url,
            "https://store.epicgames.com/en-US/p/shrek-2-remastered-01"
        );
    }

    #[test]
    fn deal_endpoints_follow_the_interface_language() {
        assert_eq!(deal_steam_language("fr"), "french");
        assert_eq!(deal_steam_language("en"), "english");
        assert_eq!(deal_locale("fr"), "fr-FR");
        assert_eq!(deal_locale("en"), "en-US");
        assert_eq!(epic_store_prefix("fr"), "fr");
        assert_eq!(epic_store_prefix("en"), "en-US");
        assert_eq!(itch_language_path("fr"), "lang-fr");
        assert_eq!(itch_language_path("en"), "lang-en");
    }

    fn test_deal(store: Store, title: &str, discount: u32) -> Deal {
        Deal {
            store,
            title: title.to_string(),
            url: format!("https://example.com/{title}"),
            image: String::new(),
            discount,
            old_price: "29,99 €".to_string(),
            new_price: "9,99 €".to_string(),
            ends: 0,
            free: false,
        }
    }

    #[test]
    fn promotions_show_up_before_every_store_has_answered() {
        let ctx = egui::Context::default();
        let (tx, rx) = mpsc::channel();
        let mut deals = Deals {
            rx: Some(rx),
            loading: true,
            pending: 3,
            ..Default::default()
        };

        tx.send(Ok(vec![test_deal(Store::Steam, "Portal 2", 75)]))
            .unwrap();
        deals.poll(&ctx);
        assert_eq!(deals.items.len(), 1);
        assert!(deals.loading);
        assert_eq!(deals.pending, 2);

        tx.send(Ok(vec![test_deal(Store::Gog, "Disco Elysium", 70)]))
            .unwrap();
        tx.send(Err("Epic indisponible".to_string())).unwrap();
        drop(tx);
        deals.poll(&ctx);

        assert!(!deals.loading);
        assert_eq!(deals.items.len(), 2);
        assert_eq!(deals.error, None);
    }

    #[test]
    fn an_empty_steam_answer_is_an_error() {
        assert!(steam_deals(b"{}").is_err());
    }
}
