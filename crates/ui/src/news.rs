//! Section Actualités : les dernières annonces des jeux de la bibliothèque,
//! Steam comme Epic Games, GOG et itch.io. Les requêtes vivent sur des fils de
//! fond ; la page affiche les cartes au fur et à mesure.

use crate::gameinfo::NewsArt;
use crate::{
    ACCENT, BACKGROUND, BORDER, CARD, CARD_ACTIVE, CARD_HOVER, DANGER, OK, TEXT, TEXT_DIM,
};
use eframe::egui;
use lapinux_core::client::Client;
use lapinux_core::ipc::Req;
use lapinux_core::model::SteamNews;
use lapinux_core::stores::{Store, StoreDetails, StoreNews};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

pub(crate) mod calendar;
pub(crate) mod deals;
pub(crate) mod gaming;

/// Annonces retenues par jeu. La page les ajoute au fil de l'analyse.
const NEWS_PER_GAME: usize = 4;
/// Fils simultanés : la file est maintenant servie par petits lots, la lecture
/// d'une annonce ne dépend plus d'une page HTML. Quatre requêtes suffisent à
/// remplir une page sans reproduire la rafale qui faisait refuser Steam.
const WORKERS: usize = 4;
/// Version du cache des annonces. Une version inconnue est ignorée, jamais
/// interprétée de travers.
const NEWS_CACHE_VERSION: u32 = 1;
/// Résultats traités par image : après un changement d'onglet, le fil UI
/// rattrape son retard sans bloquer une frame sur cent jeux.
const POLL_BUDGET: usize = 48;
/// Fondu d'apparition d'une carte nouvellement reçue.
const CARD_REVEAL: f32 = 0.22;
const EXCERPT_CHARS: usize = 240;
const SCROLLBAR_ROOM: f32 = 12.0;
/// En dessous de cette largeur, le héros passe de deux colonnes à une image
/// au-dessus du texte. L'application impose 980 px : ce repli sert aux tests et
/// aux fenêtres systèmes qui réduiraient encore la surface.
const HERO_SPLIT_MIN: f32 = 860.0;
/// Fondu de contenu au changement d'onglet : assez court pour rester vif.
const TAB_FADE: f32 = 0.18;
/// Glissement de la pastille active, sur la même durée que la barre du haut.
const TAB_SLIDE: f32 = 0.22;

// Une seule échelle pour toute la page : 4, 8, 12, 16, 24, 32. Les cartes, les
// filtres et les en-têtes s'y alignent, ce qui donne le même rythme aux trois
// onglets.
pub(super) const SPACE_2: f32 = 8.0;
pub(super) const SPACE_3: f32 = 12.0;
pub(super) const SPACE_4: f32 = 16.0;
pub(super) const SPACE_5: f32 = 24.0;

/// Cartes d'actualités à angles droits : la page n'arrondit ni les cartes ni
/// leurs pastilles.
pub(super) const CARD_RADIUS: f32 = 0.0;
pub(super) const CARD_PADDING: f32 = 18.0;
pub(super) const GRID_GAP: f32 = 20.0;
/// Les trois sélecteurs de sources des Actualités partagent la même largeur :
/// le bouton fermé et le menu déroulant restent alignés d'un onglet à l'autre.
pub(super) const NEWS_FILTER_WIDTH: f32 = 260.0;
/// Tous les contrôles de la ligne d'en-tête ont cette hauteur extérieure :
/// sélecteur, barre d'onglets et Actualiser restent alignés d'une vue à l'autre.
pub(super) const NEWS_CONTROL_HEIGHT: f32 = 36.0;
/// En dessous de cette largeur, le Calendrier place le bouton Steam et
/// Actualiser sur une seconde ligne : les quatre onglets occupent déjà la
/// largeur restante.
const CALENDAR_HEADER_STACK_WIDTH: f32 = 1120.0;
const TAB_FRAME_RADIUS: f32 = 12.0;
const TAB_FRAME_STROKE: f32 = 1.0;
/// Les trois boutons occupent toute la hauteur de la barre : ils ont donc la
/// même hauteur que le sélecteur de sources et Actualiser, pas seulement le
/// même cadre extérieur.
const TAB_BUTTON_HEIGHT: f32 = NEWS_CONTROL_HEIGHT;
/// Corps de carte : métadonnées, titre sur trois lignes, aperçu sur deux
/// lignes, puis l'action collée en bas. Toutes les cartes ont la même hauteur.
pub(super) const GRID_BODY_INNER: f32 = 166.0;
const GRID_PAD_TOP: f32 = 16.0;
const GRID_PAD_BOTTOM: f32 = 14.0;
pub(super) const GRID_BODY_HEIGHT: f32 = GRID_BODY_INNER + GRID_PAD_TOP + GRID_PAD_BOTTOM;
const TITLE_BLOCK_HEIGHT: f32 = 60.0;
const EXCERPT_BLOCK_HEIGHT: f32 = 34.0;
/// Filet, respiration et libellé de l'action, collés au bas du corps.
const CARD_FOOTER_HEIGHT: f32 = 27.0;
/// Cartes sans action visible : le pied disparaît, le titre tient sur une
/// ligne et l'aperçu sur deux. Le corps suit ce contenu au lieu de réserver
/// l'ancien espace de trois lignes de titre et du lien.
const GRID_BODY_INNER_COMPACT: f32 = 62.0;
const GRID_PAD_TOP_COMPACT: f32 = 12.0;
const GRID_PAD_BOTTOM_COMPACT: f32 = 10.0;
pub(super) const GRID_BODY_HEIGHT_COMPACT: f32 =
    GRID_BODY_INNER_COMPACT + GRID_PAD_TOP_COMPACT + GRID_PAD_BOTTOM_COMPACT;
/// Conservé pour les anciens rendus de test ; les nouvelles cartes utilisent
/// directement la hauteur de leur visuel.
pub(super) const HERO_BODY_HEIGHT: f32 = 150.0;

const FONT_TAB: f32 = 13.0;
const FONT_SECTION: f32 = 15.0;
const FONT_CARD_TITLE: f32 = 16.0;
const FONT_CARD_BODY: f32 = 13.0;
const FONT_META: f32 = 11.0;
const FONT_ACTION: f32 = 12.5;
const FONT_HERO_TITLE_WIDE: f32 = 28.0;
const FONT_HERO_BODY: f32 = 14.0;
const FONT_HERO_BODY_NARROW: f32 = 13.5;
const HERO_TEXT_MAX: f32 = 680.0;

/// Corps de texte un peu plus clair que les métadonnées : à 12,5 px, le gris
/// secondaire du thème manquait de contraste sur les cartes.
fn body_ink() -> egui::Color32 {
    let a = TEXT_DIM();
    let b = TEXT();
    let mix = |x: u8, y: u8| ((x as u16 + y as u16) / 2) as u8;
    egui::Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}

/// Un visuel de carte partage les angles de la carte et rejoint franchement
/// le corps : l'image et le texte forment un seul bloc.
pub(super) fn cover_rounding() -> egui::Rounding {
    egui::Rounding {
        nw: CARD_RADIUS,
        ne: CARD_RADIUS,
        sw: 0.0,
        se: 0.0,
    }
}

/// Visuel du héros : seuls les coins extérieurs gauches suivent la carte.
fn hero_cover_rounding() -> egui::Rounding {
    egui::Rounding {
        nw: CARD_RADIUS,
        ne: 0.0,
        sw: CARD_RADIUS,
        se: 0.0,
    }
}

/// Un jeu à interroger : Steam n'a pas de `store`, les trois autres boutiques
/// passent par leur fiche lue par le daemon.
#[derive(Clone, Debug)]
pub(crate) struct GameSource {
    pub appid: u32,
    pub name: String,
    pub store: Option<Store>,
    /// Icône locale du jeu, quand le cache d'illustrations en connaît une.
    pub icon: Option<PathBuf>,
}

#[derive(Clone, Debug)]
struct Article {
    appid: u32,
    game: String,
    store: Option<Store>,
    icon: Option<PathBuf>,
    news: SteamNews,
    /// Instant d'arrivée : les cartes reçues en cours d'analyse apparaissent
    /// en fondu, sans déplacer celles déjà affichées.
    arrived: Instant,
}

/// Annonce choisie sur la page Actualités, à ouvrir dans le lecteur intégré.
#[derive(Debug)]
pub(crate) struct NewsOpen {
    pub items: Vec<SteamNews>,
    pub at: usize,
}

/// Jeu d'une plateforme, avec le nombre d'annonces reçues.
type UpdateGame = (u32, Store, String, usize, Option<PathBuf>);
/// Une plateforme et ses jeux dans le menu des mises à jour.
type UpdateGroup = (Store, Vec<UpdateGame>);

/// Résumé d'une annonce affichée sur l'accueil ; l'annonce complète reste dans le flux.
#[derive(Clone)]
pub(crate) struct HomeUpdate {
    pub game: String,
    pub title: String,
    pub date: i64,
    pub image: String,
    appid: u32,
    store: Option<Store>,
    url: String,
}

/// État de la page Actualités. Les annonces déjà reçues restent affichées
/// pendant que les jeux suivants répondent.
#[derive(Default)]
pub(crate) struct Feed {
    articles: Vec<Article>,
    art: NewsArt,
    /// Tous les jeux disponibles, dans l'ordre fourni.
    games: Vec<GameSource>,
    /// Signature de cette liste : la page repart quand la bibliothèque change,
    /// sans comparer des milliers d'identifiants à chaque image.
    signature: u64,
    language: String,
    /// Jeux déjà mis en file, en cours ou terminés : un jeu n'est jamais
    /// demandé deux fois, même si la bibliothèque change d'ordre.
    queued: HashSet<(u32, Option<Store>)>,
    /// Cache disque des annonces déjà reçues. Il permet d'ouvrir l'accueil
    /// sans aucun appel Steam ; les lectures suivantes sont explicites.
    cache_path: Option<PathBuf>,
    cache_loaded: bool,
    cache_dirty: bool,
    /// Remet la grille en haut à la première ouverture ou après une
    /// actualisation ; un simple retour d'onglet garde la position.
    scroll_to_top: bool,
    /// Jeux dont la requête est revenue, avec ou sans annonce. C'est la
    /// progression affichée sous la liste.
    analysed: usize,
    loading: bool,
    pending: HashSet<(u32, Option<Store>)>,
    /// Annonces déjà reçues : une réponse peut répéter une adresse, et deux
    /// jeux peuvent partager une annonce sans créer de doublon visuel.
    seen: HashSet<(u32, Option<Store>, String)>,
    rx: Option<mpsc::Receiver<(GameSource, Option<Vec<SteamNews>>)>>,
    /// Icônes de jeu décodées pour les cartes visibles, et jeux déjà essayés :
    /// un fichier absent ne provoque pas une lecture à chaque image.
    icons: HashMap<(u32, Option<Store>), Option<egui::TextureHandle>>,
    icon_attempts: HashSet<(u32, Option<Store>)>,
    /// Décodage des icônes hors du fil UI. Une lecture de fichier et un
    /// redimensionnement PNG ne doivent jamais tomber pendant le défilement.
    icon_rx: Option<mpsc::Receiver<((u32, Option<Store>), Option<egui::ColorImage>)>>,
    icon_tx: Option<mpsc::Sender<((u32, Option<Store>), Option<egui::ColorImage>)>>,
    /// Carte cliquée pendant la peinture, ouverte après la grille.
    open_request: Option<(u32, Option<Store>, String)>,
    /// Jeux et plateformes décochés dans le menu des mises à jour.
    hidden_games: HashSet<(u32, Store)>,
    hidden_stores: HashSet<Store>,
    /// Position de défilement observée par le banc de mesure. Le champ n'existe
    /// que dans les tests : le rendu normal n'a pas besoin de la stocker.
    #[cfg(test)]
    last_scroll: f32,
}

/// Cache disque : uniquement des annonces déjà affichées. Une lecture
/// manuelle le met à jour ; aucun démarrage ne le remplit en douce.
#[derive(Serialize, Deserialize)]
struct NewsCache {
    version: u32,
    language: String,
    articles: Vec<CachedArticle>,
}

#[derive(Serialize, Deserialize)]
struct CachedArticle {
    appid: u32,
    game: String,
    store: Option<Store>,
    icon: Option<PathBuf>,
    news: SteamNews,
}

impl Feed {
    fn update_store(store: Option<Store>) -> Store {
        store.unwrap_or(Store::Steam)
    }

    fn icon_store(store: Store) -> Option<Store> {
        (store != Store::Steam).then_some(store)
    }

    fn update_visible(&self, appid: u32, store: Option<Store>) -> bool {
        let store = Self::update_store(store);
        !self.hidden_stores.contains(&store) && !self.hidden_games.contains(&(appid, store))
    }

    fn visible_articles(&self) -> Vec<usize> {
        self.articles
            .iter()
            .enumerate()
            .filter(|(_, article)| self.update_visible(article.appid, article.store))
            .map(|(index, _)| index)
            .collect()
    }

    /// Regroupe les jeux qui ont reçu des annonces par plateforme. Le menu ne
    /// liste donc pas toute la bibliothèque : seuls les jeux réellement
    /// concernés par les notes de mise à jour sont proposés.
    fn update_groups(&self) -> Vec<UpdateGroup> {
        let mut groups: Vec<UpdateGroup> = Store::ALL
            .into_iter()
            .map(|store| (store, Vec::new()))
            .collect();
        for article in &self.articles {
            let store = Self::update_store(article.store);
            let Some((_, games)) = groups.iter_mut().find(|(known, _)| *known == store) else {
                continue;
            };
            if let Some((_, _, _, count, _)) =
                games.iter_mut().find(|(appid, game_store, _, _, _)| {
                    *appid == article.appid && *game_store == store
                })
            {
                *count += 1;
            } else {
                games.push((
                    article.appid,
                    store,
                    article.game.clone(),
                    1,
                    article.icon.clone(),
                ));
            }
        }
        for (_, games) in &mut groups {
            games.sort_by(|left, right| left.2.to_lowercase().cmp(&right.2.to_lowercase()));
        }
        groups.retain(|(_, games)| !games.is_empty());
        groups
    }

    fn update_filter_label(&self, groups: &[UpdateGroup]) -> String {
        let total: usize = groups.iter().map(|(_, games)| games.len()).sum();
        let selected = groups
            .iter()
            .flat_map(|(_, games)| games)
            .filter(|(appid, store, _, _, _)| self.update_visible(*appid, Some(*store)))
            .count();
        if total == 0 || selected == total {
            lapinux_core::i18n::t("news.updates.all_games").to_string()
        } else if selected == 0 {
            lapinux_core::i18n::t("news.updates.none").to_string()
        } else {
            lapinux_core::i18n::t_args(
                "news.updates.selected_games",
                &[("count", &selected.to_string())],
            )
            .to_string()
        }
    }

    /// Menu de l'en-tête : les lignes barrées masquent une plateforme ou un
    /// jeu. Chaque jeu porte son icône locale, chaque plateforme son favicon.
    pub(super) fn filter_picker(&mut self, ui: &mut egui::Ui) {
        let groups = self.update_groups();
        let selected = self.update_filter_label(&groups);
        let store_icons: Vec<String> = groups
            .iter()
            .map(|(store, _)| store_icon_url(*store).to_string())
            .collect();
        let mut changed = false;
        source_menu(
            ui,
            "news-updates-filter",
            &selected,
            NEWS_FILTER_WIDTH,
            |ui| {
                ui.set_min_width(NEWS_FILTER_WIDTH);
                self.art.ensure(ui.ctx(), &store_icons);
                if store_icons.iter().any(|url| self.art.get(url).is_none()) {
                    ui.ctx().request_repaint_after(Duration::from_millis(150));
                }
                if !self.hidden_games.is_empty() || !self.hidden_stores.is_empty() {
                    if ui.button(lapinux_core::i18n::t("news.show_all")).clicked() {
                        self.hidden_games.clear();
                        self.hidden_stores.clear();
                        changed = true;
                    }
                    ui.separator();
                }
                if groups.is_empty() {
                    ui.label(lapinux_core::i18n::t("news.updates.no_games"));
                    return;
                }
                egui::ScrollArea::vertical()
                    .id_salt("news-updates-filter-list")
                    .max_height(380.0)
                    .show(ui, |ui| {
                        for (store, games) in &groups {
                            for (appid, game_store, _, _, icon) in games {
                                self.ensure_icon(
                                    ui.ctx(),
                                    (*appid, Feed::icon_store(*game_store)),
                                    icon.as_deref(),
                                );
                            }
                            let store_hidden = self.hidden_stores.contains(store);
                            let platform_hidden = store_hidden
                                || games.iter().all(|(appid, game_store, _, _, _)| {
                                    self.hidden_games.contains(&(*appid, *game_store))
                                });
                            let updates: usize =
                                games.iter().map(|(_, _, _, count, _)| count).sum();
                            if filter_choice(
                                ui,
                                self.art.get(store_icon_url(*store)),
                                &store.label().to_uppercase(),
                                updates,
                                platform_hidden,
                                store_color(Some(*store)),
                            ) {
                                changed = true;
                                if platform_hidden {
                                    self.hidden_stores.remove(store);
                                } else {
                                    self.hidden_stores.insert(*store);
                                }
                                for (appid, game_store, _, _, _) in games {
                                    self.hidden_games.remove(&(*appid, *game_store));
                                }
                            }
                            ui.indent(("news-updates-store", store.label()), |ui| {
                                for (appid, game_store, name, count, _) in games {
                                    let key = (*appid, *game_store);
                                    let hidden = store_hidden || self.hidden_games.contains(&key);
                                    let icon = self
                                        .icons
                                        .get(&(*appid, Feed::icon_store(*game_store)))
                                        .and_then(|icon| icon.as_ref());
                                    if icon.is_none() {
                                        ui.ctx().request_repaint_after(Duration::from_millis(150));
                                    }
                                    if filter_choice(
                                        ui,
                                        icon,
                                        name,
                                        *count,
                                        hidden,
                                        store_color(Some(*game_store)),
                                    ) {
                                        changed = true;
                                        if hidden {
                                            if store_hidden {
                                                self.hidden_stores.remove(store);
                                                for (other_appid, other_store, _, _, _) in games {
                                                    let other = (*other_appid, *other_store);
                                                    if other != key {
                                                        self.hidden_games.insert(other);
                                                    }
                                                }
                                            }
                                            self.hidden_games.remove(&key);
                                        } else {
                                            self.hidden_games.insert(key);
                                        }
                                    }
                                }
                            });
                            ui.add_space(4.0);
                        }
                    });
            },
        );
        if changed {
            self.scroll_to_top = true;
        }
    }

    fn prepare(&mut self, ctx: &egui::Context, games: &[GameSource], language: &str) {
        let signature = list_signature(games);
        if language != self.language || self.language.is_empty() {
            self.reset(games, language, signature);
            self.load_cache();
        } else if signature != self.signature {
            self.sync_games(games, signature);
        }
        // Aucun envoi ici : le cache disque est affiché tel quel. Steam n'est
        // réveillé que par « Actualiser » ou « Charger plus ».
        self.poll(ctx);
        if self.loading {
            ctx.request_repaint_after(Duration::from_millis(400));
        }
    }

    /// Relit les annonces déjà reçues lors d'une session précédente. Le cache
    /// ne contient que des cartes affichables : il ne déclenche aucune requête
    /// et ne bloque jamais l'ouverture de la page.
    fn load_cache(&mut self) {
        if self.cache_loaded {
            return;
        }
        self.cache_loaded = true;
        let Some(path) = self.cache_path.as_deref() else {
            return;
        };
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        let Ok(cache) = serde_json::from_str::<NewsCache>(&text) else {
            return;
        };
        if cache.version != NEWS_CACHE_VERSION || cache.language != self.language {
            return;
        }
        let allowed: HashSet<(u32, Option<Store>)> = self
            .games
            .iter()
            .map(|game| (game.appid, game.store))
            .collect();
        for cached in cache
            .articles
            .into_iter()
            .filter(|article| allowed.contains(&(article.appid, article.store)))
        {
            let key = (cached.appid, cached.store, cached.news.url.clone());
            if !self.seen.insert(key) {
                continue;
            }
            self.articles.push(Article {
                appid: cached.appid,
                game: cached.game,
                store: cached.store,
                icon: cached.icon,
                news: cached.news,
                arrived: Instant::now(),
            });
        }
        self.sort_by_date();
    }

    /// Écrit le cache après une lecture terminée. Il garde toutes les annonces
    /// reçues : le prochain démarrage doit pouvoir afficher la bibliothèque
    /// entière sans redemander un seul jeu à Steam.
    fn save_cache(&mut self) {
        if !self.cache_dirty || self.language.is_empty() {
            return;
        }
        let Some(path) = self.cache_path.clone() else {
            self.cache_dirty = false;
            return;
        };
        let articles: Vec<CachedArticle> = self
            .articles
            .iter()
            .map(|article| CachedArticle {
                appid: article.appid,
                game: article.game.clone(),
                store: article.store,
                icon: article.icon.clone(),
                news: article.news.clone(),
            })
            .collect();
        let cache = NewsCache {
            version: NEWS_CACHE_VERSION,
            language: self.language.clone(),
            articles,
        };
        let Ok(data) = serde_json::to_vec(&cache) else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let temporary = path.with_extension("tmp");
        if std::fs::write(&temporary, data).is_ok() && std::fs::rename(&temporary, &path).is_ok() {
            self.cache_dirty = false;
        }
    }

    /// Peint la page et entretient ses lectures en arrière-plan. Une carte
    /// cliquée remonte l'annonce à ouvrir dans le lecteur intégré.
    pub(crate) fn page(
        &mut self,
        ui: &mut egui::Ui,
        games: &[GameSource],
        language: &str,
        client: &Client,
    ) -> Option<NewsOpen> {
        self.prepare(ui.ctx(), games, language);
        // Les visuels et les icônes arrivent par des fils : sans cette lecture,
        // les cartes resteraient sur leur lettre d'attente.
        self.art.poll(ui.ctx());
        self.poll_icons(ui.ctx());
        if games.is_empty() {
            status_card(
                ui,
                lapinux_core::i18n::t("news.empty_state.title"),
                lapinux_core::i18n::t("news.no_games"),
                false,
                None,
            );
            return None;
        }

        let width = (ui.available_width() - SCROLLBAR_ROOM).max(240.0);
        let list_height = ui.available_height().max(180.0);

        if self.loading && self.articles.is_empty() {
            ui.allocate_ui_with_layout(
                egui::vec2(width, list_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    let area = egui::ScrollArea::vertical()
                        .id_salt("news-updates")
                        .auto_shrink([false, false]);
                    let area = if std::mem::take(&mut self.scroll_to_top) {
                        area.vertical_scroll_offset(0.0)
                    } else {
                        area
                    };
                    let card_height = grid_card_height_for(grid_card_width(width), true) + GRID_GAP;
                    let skeleton_rows =
                        ((list_height / card_height).ceil() as usize + 1).clamp(2, 4);
                    area.show(ui, |ui| {
                        ui.set_width(width);
                        skeleton_grid_compact(ui, width, skeleton_rows);
                    });
                },
            );
            ui.ctx().request_repaint_after(Duration::from_millis(200));
            return None;
        }
        if self.articles.is_empty() {
            if status_card(
                ui,
                lapinux_core::i18n::t("news.empty_state.title"),
                lapinux_core::i18n::t("news.updates.empty"),
                false,
                Some(lapinux_core::i18n::t("news.load")),
            ) {
                self.refresh(games, language, ui.ctx(), client);
            }
            return None;
        }

        let visible = self.visible_articles();
        if visible.is_empty() {
            if status_card(
                ui,
                lapinux_core::i18n::t("news.empty_state.title"),
                lapinux_core::i18n::t("news.updates.filter_empty"),
                false,
                Some(lapinux_core::i18n::t("news.show_all")),
            ) {
                self.hidden_games.clear();
                self.hidden_stores.clear();
            }
            return None;
        }

        // Seules les rangées visibles sont mises en page. Une liste de mille
        // annonces coûte alors le même prix qu'une liste de dix.
        let columns = columns(width);
        let card_width = grid_card_width(width);
        let card_height = grid_card_height_for(card_width, true);
        let rows = visible.len().div_ceil(columns);
        ui.allocate_ui_with_layout(
            egui::vec2(width, list_height),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.spacing_mut().item_spacing.y = GRID_GAP;
                let area = egui::ScrollArea::vertical()
                    .id_salt("news-updates")
                    .auto_shrink([false, false]);
                let area = if std::mem::take(&mut self.scroll_to_top) {
                    area.vertical_scroll_offset(0.0)
                } else {
                    area
                };
                let output = area.show_rows(ui, card_height, rows, |ui, range| {
                    ui.set_width(width);
                    self.article_rows(ui, range, columns, card_width, &visible);
                });
                #[cfg(test)]
                {
                    self.last_scroll = output.state.offset.y;
                }
                #[cfg(not(test))]
                let _ = output;
            },
        );

        if self.loading {
            ui.ctx().request_repaint_after(Duration::from_millis(200));
        }
        let clicked = self.open_request.take();
        clicked.and_then(|(appid, store, url)| self.reader_for(appid, store, &url))
    }

    /// Annonces d'un jeu, dans l'ordre du lecteur, avec celle qui a été
    /// cliquée. Une seule annonce suffit quand la page n'en connaît qu'une.
    fn reader_for(&self, appid: u32, store: Option<Store>, url: &str) -> Option<NewsOpen> {
        let mut items = Vec::new();
        let mut seen = HashSet::new();
        for article in &self.articles {
            if article.appid == appid
                && article.store == store
                && seen.insert(article.news.url.clone())
            {
                items.push(article.news.clone());
            }
        }
        items.sort_by(|left, right| {
            right
                .date
                .cmp(&left.date)
                .then_with(|| left.title.cmp(&right.title))
                .then_with(|| left.url.cmp(&right.url))
        });
        let at = items.iter().position(|item| item.url == url)?;
        Some(NewsOpen { items, at })
    }

    /// Décode l'icône d'un jeu une seule fois, hors du fil UI. Les icônes sont
    /// minuscules : la texture est bornée à 64 px, soit moins qu'un carré de
    /// carte. Le fil de peinture ne fait plus que téléverser le résultat.
    fn ensure_icon(
        &mut self,
        ctx: &egui::Context,
        key: (u32, Option<Store>),
        path: Option<&std::path::Path>,
    ) {
        if !self.icon_attempts.insert(key) {
            return;
        }
        let Some(path) = path else {
            self.icons.insert(key, None);
            return;
        };
        if self.icon_tx.is_none() {
            let (tx, rx) = mpsc::channel();
            self.icon_tx = Some(tx);
            self.icon_rx = Some(rx);
        }
        let Some(tx) = self.icon_tx.clone() else {
            return;
        };
        let path = path.to_path_buf();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let image = std::fs::read(&path).ok().and_then(|bytes| {
                let image = image::load_from_memory(&bytes).ok()?;
                let image = image.thumbnail(64, 64).to_rgba8();
                let size = [image.width() as usize, image.height() as usize];
                Some(egui::ColorImage::from_rgba_unmultiplied(
                    size,
                    image.as_raw(),
                ))
            });
            if tx.send((key, image)).is_ok() {
                ctx.request_repaint();
            }
        });
    }

    /// Relève les icônes décodées en tâche de fond. Le téléversement d'une
    /// texture de 64 px est négligeable ; le décodage, lui, n'est plus dans la
    /// frame qui fait défiler la liste.
    fn poll_icons(&mut self, ctx: &egui::Context) {
        let mut received = Vec::new();
        if let Some(rx) = &self.icon_rx {
            while let Ok(item) = rx.try_recv() {
                received.push(item);
            }
        }
        for (key, image) in received {
            let texture = image.map(|image| {
                ctx.load_texture(
                    format!("news-game-icon-{}-{:?}", key.0, key.1),
                    image,
                    egui::TextureOptions::LINEAR,
                )
            });
            self.icons.insert(key, texture);
        }
    }

    /// La liste de jeux a changé sans changer de langue : on met à jour la
    /// file, sans toucher aux articles déjà affichés.
    fn sync_games(&mut self, games: &[GameSource], signature: u64) {
        self.games = games.to_vec();
        self.signature = signature;
        // La bibliothèque peut perdre un jeu : son annonce en cache ne doit
        // pas rester visible dans la page Actualités.
        let allowed: HashSet<(u32, Option<Store>)> = self
            .games
            .iter()
            .map(|game| (game.appid, game.store))
            .collect();
        self.articles
            .retain(|article| allowed.contains(&(article.appid, article.store)));
        self.seen
            .retain(|(appid, store, _)| allowed.contains(&(*appid, *store)));
    }

    /// Relance la lecture : le bouton de l'en-tête de la page l'appelle. Une
    /// lecture en cours n'est pas doublée ; elle finira d'elle-même.
    pub(crate) fn refresh(
        &mut self,
        games: &[GameSource],
        language: &str,
        ctx: &egui::Context,
        client: &Client,
    ) {
        if self.loading {
            return;
        }
        if language != self.language || self.language.is_empty() {
            self.reset(games, language, list_signature(games));
            self.load_cache();
        } else {
            self.sync_games(games, list_signature(games));
        }
        // Le cache reste affiché pendant la lecture : les jeux retirés de la
        // bibliothèque ont déjà disparu dans `sync_games`, puis chaque réponse
        // remplace les annonces de son jeu.
        self.queued.clear();
        self.pending.clear();
        self.seen = self
            .articles
            .iter()
            .map(|article| (article.appid, article.store, article.news.url.clone()))
            .collect();
        self.scroll_to_top = true;
        self.analysed = 0;
        // Comme avant : Actualiser relit toute la bibliothèque. Le cache ne
        // change que le démarrage, pas le sens du bouton.
        self.dispatch(ctx, client);
    }

    /// Une rangée de cartes, pour la tranche rendue par `show_rows`.
    fn article_rows(
        &mut self,
        ui: &mut egui::Ui,
        range: std::ops::Range<usize>,
        columns: usize,
        card_width: f32,
        visible: &[usize],
    ) {
        // Les visuels et les icônes sont demandés uniquement pour les rangées
        // visibles : une bibliothèque de plusieurs milliers d'annonces ne
        // télécharge pas 2 752 images d'un coup.
        let first = (range.start * columns).min(visible.len());
        let last = (range.end * columns).min(visible.len());
        let visible_images: Vec<String> = visible[first..last]
            .iter()
            .map(|&index| &self.articles[index])
            .filter(|article| !article.news.image.is_empty())
            .map(|article| article.news.image.clone())
            .collect();
        if !visible_images.is_empty() {
            self.art.ensure(ui.ctx(), &visible_images);
        }
        let visible_icons: Vec<((u32, Option<Store>), Option<PathBuf>)> = visible[first..last]
            .iter()
            .map(|&index| &self.articles[index])
            .map(|article| ((article.appid, article.store), article.icon.clone()))
            .collect();
        for (key, path) in visible_icons {
            self.ensure_icon(ui.ctx(), key, path.as_deref());
        }
        let mut clicked = None;
        for row in range {
            let start = row * columns;
            let end = (start + columns).min(visible.len());
            let cards = end - start;
            // Une dernière rangée incomplète est centrée sous les précédentes :
            // une carte seule ne reste pas collée au bord gauche.
            let used = card_width * cards as f32 + GRID_GAP * cards.saturating_sub(1) as f32;
            let full = card_width * columns as f32 + GRID_GAP * (columns - 1) as f32;
            let offset = ((full - used) * 0.5).max(0.0);
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = GRID_GAP;
                if offset > 0.0 {
                    ui.add_space(offset);
                }
                for &index in &visible[start..end] {
                    let article = &self.articles[index];
                    let date = news_date(article.news.date);
                    let content = update_content(article, &date);
                    let icon = self
                        .icons
                        .get(&(article.appid, article.store))
                        .and_then(|icon| icon.as_ref());
                    let id = ui
                        .id()
                        .with(("news-card", article.appid, article.news.url.as_str()));
                    if grid_card(
                        ui,
                        card_width,
                        self.art.get(&article.news.image),
                        icon,
                        &article.game,
                        None,
                        id,
                        reveal_alpha(article.arrived),
                        &content,
                    ) {
                        clicked = Some((article.appid, article.store, article.news.url.clone()));
                    }
                }
            });
        }
        if clicked.is_some() {
            self.open_request = clicked;
        }
    }

    fn reset(&mut self, games: &[GameSource], language: &str, signature: u64) {
        self.games = games.to_vec();
        self.signature = signature;
        self.language = language.to_string();
        self.articles.clear();
        self.pending.clear();
        self.queued.clear();
        self.seen.clear();
        self.scroll_to_top = true;
        self.analysed = 0;
        self.loading = false;
        self.rx = None;
        self.cache_loaded = false;
        self.cache_dirty = false;
    }

    /// Met tous les jeux pas encore demandés dans la file, puis laisse les fils
    /// les parcourir. Les résultats arrivent au fur et à mesure et la page reste
    /// utilisable pendant l'analyse.
    fn dispatch(&mut self, ctx: &egui::Context, client: &Client) {
        if self.loading {
            return;
        }
        let tasks: Vec<GameSource> = self
            .games
            .iter()
            .filter(|game| !self.queued.contains(&(game.appid, game.store)))
            .cloned()
            .collect();
        if tasks.is_empty() {
            return;
        }
        for game in &tasks {
            let key = (game.appid, game.store);
            self.queued.insert(key);
            self.pending.insert(key);
        }
        self.loading = true;

        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        let queue = Arc::new(Mutex::new(VecDeque::from(tasks)));
        let language = steam_language(&self.language);
        let workers = WORKERS.min(queue.lock().map(|queue| queue.len()).unwrap_or(1));
        for _ in 0..workers {
            let queue = Arc::clone(&queue);
            let tx = tx.clone();
            let client = client.detached();
            let language = language.clone();
            std::thread::spawn(move || loop {
                let task = match queue.lock() {
                    Ok(mut queue) => queue.pop_front(),
                    Err(_) => None,
                };
                let Some(game) = task else { break };
                let news = fetch_game(&game, &language, &client);
                if tx.send((game, news)).is_err() {
                    break;
                }
            });
        }
        ctx.request_repaint();
    }

    /// Classe les annonces de la plus récente à la plus ancienne. Les dates
    /// inconnues passent en fin de liste ; les égalités sont départagées de
    /// façon déterministe pour que deux lectures identiques aient le même ordre.
    fn sort_by_date(&mut self) {
        self.articles.sort_by(|left, right| {
            right
                .news
                .date
                .cmp(&left.news.date)
                .then_with(|| left.game.cmp(&right.game))
                .then_with(|| left.news.title.cmp(&right.news.title))
                .then_with(|| left.news.url.cmp(&right.news.url))
        });
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let Some(rx) = self.rx.as_ref() else {
            return;
        };
        let mut received = Vec::new();
        let mut disconnected = false;
        // Une bibliothèque de cent jeux ne doit pas vider tout son tampon en
        // une frame : on borne le rattrapage et la suite vient à l'image
        // suivante, sans freeze perceptible.
        while received.len() < POLL_BUDGET {
            match rx.try_recv() {
                Ok(item) => received.push(item),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            }
        }
        let had_result = !received.is_empty();
        for (game, news) in received {
            let game_key = (game.appid, game.store);
            self.pending.remove(&game_key);
            self.analysed += 1;
            // Une panne réseau ou un refus de Steam n'est pas une réponse
            // vide : le cache du jeu reste intact pour le prochain démarrage.
            let Some(mut news) = news else {
                continue;
            };
            // Une lecture complète remplace les annonces du jeu : une carte
            // retirée par l'éditeur ne doit pas survivre dans le cache.
            self.articles
                .retain(|article| (article.appid, article.store) != game_key);
            self.seen
                .retain(|(appid, store, _)| (*appid, *store) != game_key);
            // Même une réponse vide doit être enregistrée : elle retire les
            // anciennes annonces du jeu du cache.
            self.cache_dirty = true;
            // Chaque jeu garde ses annonces les plus récentes d'abord ; les
            // jeux, eux, s'ajoutent dans l'ordre de la bibliothèque. On ne
            // retrie jamais les cartes déjà affichées : leur position reste
            // stable pendant que la suite arrive.
            news.sort_by(|left, right| right.date.cmp(&left.date));
            for news in news {
                // Une boutique peut publier une annonce sans page dédiée :
                // la carte reste lisible, elle n'ouvre simplement rien.
                if news.title.trim().is_empty() {
                    continue;
                }
                let key = (game.appid, game.store, news.url.clone());
                if !self.seen.insert(key) {
                    continue;
                }
                self.articles.push(Article {
                    appid: game.appid,
                    game: game.name.clone(),
                    store: game.store,
                    icon: game.icon.clone(),
                    news,
                    arrived: Instant::now(),
                });
                self.cache_dirty = true;
            }
        }
        if disconnected || self.pending.is_empty() {
            self.loading = false;
            self.rx = None;
            // Pendant la collecte, les cartes restent dans l'ordre d'arrivée
            // pour ne pas sauter sous le curseur. Une fois tous les jeux
            // revenus, la lecture devient réellement chronologique : la plus
            // récente annonce d'abord, quel que soit le jeu.
            self.sort_by_date();
            self.save_cache();
        } else if self.loading {
            ctx.request_repaint_after(Duration::from_millis(120));
        }
        if had_result {
            ctx.request_repaint();
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Tab {
    #[default]
    Updates,
    Deals,
    Gaming,
    Calendar,
}

impl Tab {
    fn label(self) -> &'static str {
        match self {
            Self::Updates => lapinux_core::i18n::t("news.tab.updates"),
            Self::Deals => lapinux_core::i18n::t("news.tab.deals"),
            Self::Gaming => lapinux_core::i18n::t("news.tab.gaming"),
            Self::Calendar => lapinux_core::i18n::t("news.tab.calendar"),
        }
    }
}

/// La page Actualités : quatre lectures indépendantes, une seule barre de
/// sélection. Chaque onglet garde son état et ses images, donc revenir dessus
/// est instantané.
#[derive(Default)]
pub(crate) struct Page {
    tab: Tab,
    /// Instant du dernier changement d'onglet : le contenu apparaît en un
    /// fondu très court, sans retarder le clic ni perdre le défilement.
    tab_started: Option<Instant>,
    updates: Feed,
    deals: deals::Deals,
    gaming: gaming::Gaming,
    calendar: calendar::Calendar,
    /// Page demandée à la webview Lapinux depuis une carte.
    webview: Option<(String, String)>,
    #[cfg(test)]
    geometry: HeaderGeometry,
}

/// Géométrie réellement dessinée par la page, relevée uniquement par les
/// tests headless : elle permet de vérifier le centrage sans image.
#[cfg(test)]
#[derive(Clone, Copy, Debug, Default)]
struct HeaderGeometry {
    content: Option<egui::Rect>,
    source: Option<egui::Rect>,
    tab_bar: Option<egui::Rect>,
    tab_buttons: Option<[egui::Rect; 4]>,
    refresh: Option<egui::Rect>,
}

impl Page {
    /// Branche le cache disque des annonces. L'application l'installe au
    /// démarrage ; les tests gardent un cache en mémoire pour ne rien écrire.
    pub(crate) fn with_cache(mut self, path: PathBuf) -> Self {
        self.updates.cache_path = Some(path);
        self
    }

    /// Les annonces proviennent du même flux que l'onglet Actualités et
    /// couvrent la même bibliothèque.
    pub(crate) fn home_updates(
        &mut self,
        ctx: &egui::Context,
        games: &[GameSource],
        language: &str,
    ) -> (Vec<HomeUpdate>, bool) {
        self.updates.prepare(ctx, games, language);
        let mut articles: Vec<_> = self
            .updates
            .articles
            .iter()
            .filter(|article| {
                self.updates.update_visible(article.appid, article.store)
                    && games
                        .iter()
                        .any(|game| game.appid == article.appid && game.store == article.store)
            })
            .collect();
        articles.sort_by(|a, b| {
            b.news
                .date
                .cmp(&a.news.date)
                .then_with(|| a.game.cmp(&b.game))
                .then_with(|| a.news.title.cmp(&b.news.title))
        });
        let updates = articles
            .into_iter()
            .take(4)
            .map(|article| HomeUpdate {
                game: article.game.clone(),
                title: article.news.title.clone(),
                date: article.news.date,
                image: article.news.image.clone(),
                appid: article.appid,
                store: article.store,
                url: article.news.url.clone(),
            })
            .collect();
        (updates, self.updates.loading)
    }

    /// Progression du flux complet de l'accueil : jeux analysés et jeux
    /// demandés. Le compteur reste visible tant que la bibliothèque répond,
    /// même lorsque les premières annonces sont déjà affichées.
    pub(crate) fn home_progress(&self) -> Option<(usize, usize)> {
        if !self.updates.loading {
            return None;
        }
        Some((
            self.updates.analysed.min(self.updates.queued.len()),
            self.updates.queued.len(),
        ))
    }

    /// Visuel d'une annonce de l'accueil. Le cache est partagé avec la page
    /// Actualités ; la texture peut arriver après la première image, d'où le
    /// repli sur l'icône générique dans l'accueil.
    pub(crate) fn home_image(
        &mut self,
        ctx: &egui::Context,
        url: &str,
    ) -> Option<egui::TextureHandle> {
        if url.trim().is_empty() {
            return None;
        }
        let urls = [url.to_string()];
        self.updates.art.ensure_still(ctx, &urls);
        self.updates.art.poll(ctx);
        self.updates.art.get(url).cloned()
    }

    pub(crate) fn open_home_update(&self, update: &HomeUpdate) -> Option<NewsOpen> {
        self.updates
            .reader_for(update.appid, update.store, &update.url)
    }

    /// Actualiser et, pour le Calendrier, l'ouverture du calendrier personnel
    /// Steam. Le second bouton est placé à gauche du premier dans le flux
    /// droite-à-gauche. Pendant l'analyse des mises à jour, le bouton porte
    /// lui-même la progression : la liste garde toute sa hauteur.
    fn right_controls(
        &mut self,
        ui: &mut egui::Ui,
        loading: bool,
        scan: Option<(usize, usize)>,
        refresh: &mut bool,
    ) {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
            let progress = loading.then_some(scan).flatten();
            let label = match progress {
                Some((done, total)) => format!("↻  {done} / {total}"),
                None => format!("↻  {}", lapinux_core::i18n::t("news.refresh")),
            };
            let mut response = crate::action_button_sized(
                ui,
                label,
                crate::ActionTone::Quiet,
                !loading,
                NEWS_CONTROL_HEIGHT,
            );
            #[cfg(test)]
            {
                self.geometry.refresh = Some(response.rect);
            }
            if let Some((done, total)) = scan {
                let done_text = done.to_string();
                let total_text = total.to_string();
                let hint = if loading {
                    lapinux_core::i18n::t_args(
                        "news.scanning",
                        &[("done", &done_text), ("total", &total_text)],
                    )
                } else {
                    lapinux_core::i18n::t_args("news.up_to_date", &[("total", &total_text)])
                };
                response = response.on_hover_text(hint);
                if progress.is_some() {
                    let fraction = if total == 0 {
                        0.0
                    } else {
                        (done as f32 / total as f32).clamp(0.0, 1.0)
                    };
                    let shown = ui.ctx().animate_value_with_time(
                        ui.id().with("news-refresh-progress"),
                        fraction,
                        0.20,
                    );
                    if (shown - fraction).abs() > 0.001 {
                        ui.ctx().request_repaint();
                    }
                    let rect = response.rect;
                    let track = egui::Rect::from_min_size(
                        egui::pos2(rect.left() + 9.0, rect.bottom() - 5.0),
                        egui::vec2((rect.width() - 18.0).max(1.0), 2.0),
                    );
                    ui.painter()
                        .rect_filled(track, 1.0, BORDER().gamma_multiply(0.35));
                    let fill = egui::Rect::from_min_size(
                        track.min,
                        egui::vec2(track.width() * shown, track.height()),
                    );
                    ui.painter().rect_filled(fill, 1.0, ACCENT());
                }
            }
            if response.clicked() {
                *refresh = true;
            }
            if self.tab == Tab::Calendar {
                ui.add_space(SPACE_2);
                calendar::personal_calendar_button(ui);
            }
        });
    }

    pub(crate) fn page(
        &mut self,
        ui: &mut egui::Ui,
        games: &[GameSource],
        language: &str,
        client: &Client,
    ) -> Option<NewsOpen> {
        // Soldes et Actus partent une fois : les onglets sont prêts quand on
        // les ouvre, sans bloquer l'image. Le calendrier, qui interroge Steam,
        // démarre seulement à sa première ouverture.
        self.deals.ensure(client, language);
        self.gaming.ensure(language);

        // Raccourcis de section : ils changent uniquement le contenu affiché,
        // jamais les données. Le cache de chaque onglet reste intact.
        let shortcut = ui.ctx().input_mut(|input| {
            [
                (egui::Key::Num1, Tab::Updates),
                (egui::Key::Num2, Tab::Deals),
                (egui::Key::Num3, Tab::Gaming),
                (egui::Key::Num4, Tab::Calendar),
            ]
            .into_iter()
            .find_map(|(key, tab)| input.consume_key(egui::Modifiers::CTRL, key).then_some(tab))
        });
        if let Some(tab) = shortcut {
            self.select_tab(tab);
        }
        if self.tab == Tab::Calendar {
            self.calendar.ensure(ui.ctx(), language);
        }

        let loading = match self.tab {
            Tab::Updates => self.updates.loading,
            Tab::Deals => self.deals.loading(),
            Tab::Gaming => self.gaming.loading(),
            Tab::Calendar => self.calendar.loading(),
        };
        // L'onglet Annonces affiche sa progression dans le bouton Actualiser :
        // le pied de page n'a plus à réserver de hauteur.
        let scan = (self.tab == Tab::Updates).then(|| {
            (
                self.updates.analysed.min(self.updates.queued.len()),
                self.updates.queued.len(),
            )
        });
        let mut refresh = false;

        #[cfg(test)]
        {
            self.geometry.content = Some(ui.available_rect_before_wrap());
        }

        let page_width = ui.available_width();
        let page_height = ui.available_height();
        let stack_calendar_controls =
            self.tab == Tab::Calendar && page_width < CALENDAR_HEADER_STACK_WIDTH;
        let mut opened = None;
        ui.vertical(|ui| {
            ui.set_width(page_width);
            ui.set_min_height(page_height);

            // Les trois sections exposent leur sélecteur dans le coin
            // supérieur gauche. Les onglets restent centrés et Actualiser
            // reste à droite.
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                let tabs_width = tab_bar_width(ui);
                let tabs_left = ((page_width - tabs_width) * 0.5).max(0.0);
                let page_left = ui.cursor().min.x;
                let source = ui.scope(|ui| match self.tab {
                    Tab::Updates => self.updates.filter_picker(ui),
                    Tab::Deals => self.deals.source_picker(ui),
                    Tab::Gaming => self.gaming.source_picker(ui),
                    Tab::Calendar => self.calendar.source_picker(ui),
                });
                #[cfg(test)]
                {
                    self.geometry.source = Some(source.response.rect);
                }
                #[cfg(not(test))]
                let _ = source;
                let used = (ui.cursor().min.x - page_left).max(0.0);
                ui.add_space((tabs_left - used).max(0.0));
                let (tab, tab_rect, tab_buttons) = tab_bar(ui, self.tab);
                #[cfg(test)]
                {
                    self.geometry.tab_bar = Some(tab_rect);
                    self.geometry.tab_buttons = Some(tab_buttons);
                }
                #[cfg(not(test))]
                let _ = (tab_rect, tab_buttons);
                if let Some(tab) = tab {
                    self.select_tab(tab);
                }
                if !stack_calendar_controls {
                    self.right_controls(ui, loading, scan, &mut refresh);
                }
            });
            if stack_calendar_controls {
                ui.add_space(4.0);
                ui.horizontal(|ui| self.right_controls(ui, loading, scan, &mut refresh));
            }
            ui.add_space(SPACE_2);

            if refresh {
                match self.tab {
                    Tab::Updates => {
                        self.updates.refresh(games, language, ui.ctx(), client);
                    }
                    Tab::Deals => self.deals.refresh(client),
                    Tab::Gaming => self.gaming.refresh(),
                    Tab::Calendar => self.calendar.refresh(ui.ctx(), language),
                }
            }

            let alpha = self.tab_started.map_or(1.0, |started| {
                let progress = (started.elapsed().as_secs_f32() / TAB_FADE).clamp(0.0, 1.0);
                1.0 - (1.0 - progress).powi(3)
            });
            if alpha < 1.0 {
                ui.ctx().request_repaint();
            } else {
                self.tab_started = None;
            }
            let article = ui
                .scope(|ui| {
                    if alpha < 1.0 {
                        ui.multiply_opacity(alpha);
                    }
                    match self.tab {
                        Tab::Updates => self.updates.page(ui, games, language, client),
                        Tab::Deals => {
                            self.deals.page(ui, client, language);
                            None
                        }
                        Tab::Gaming => {
                            self.gaming.page(ui, language);
                            None
                        }
                        Tab::Calendar => {
                            self.calendar.page(ui, language);
                            None
                        }
                    }
                })
                .inner;
            if let Some(article) = article {
                opened = Some(article);
            }
        });
        if let Some((url, title)) = self
            .deals
            .take_webview()
            .or_else(|| self.gaming.take_webview())
            .or_else(|| self.calendar.take_webview())
        {
            self.webview = Some((url, title));
        }
        opened
    }

    /// Adresse à confier à la fenêtre WebKit de Lapinux, après le clic d'une
    /// carte Soldes ou Actus.
    pub(crate) fn take_webview(&mut self) -> Option<(String, String)> {
        self.webview.take()
    }

    fn select_tab(&mut self, tab: Tab) {
        if tab != self.tab {
            self.tab = tab;
            self.tab_started = Some(Instant::now());
        }
    }
}

/// Les quatre segments ont exactement la même largeur : le groupe reste
/// aligné même quand un libellé est plus long que les autres.
fn tab_button_width(ui: &egui::Ui) -> f32 {
    let widest = [Tab::Updates, Tab::Deals, Tab::Gaming, Tab::Calendar]
        .into_iter()
        .map(|tab| {
            ui.painter()
                .layout_no_wrap(
                    tab.label().to_string(),
                    egui::FontId::proportional(FONT_TAB),
                    TEXT(),
                )
                .size()
                .x
        })
        .fold(0.0_f32, f32::max);
    widest + 32.0
}

/// Largeur de la barre : quatre segments égaux, sans marge ni gouttière.
fn tab_bar_width(ui: &egui::Ui) -> f32 {
    tab_button_width(ui) * 4.0
}

/// Arrondi d'un segment : seuls les bords extérieurs du cadre sont arrondis.
/// Les frontières entre deux boutons restent droites, pour que la sélection
/// remplisse toute la forme du segment au lieu de laisser des creux sombres.
fn tab_segment_rounding(slot: usize) -> egui::Rounding {
    let radius = TAB_FRAME_RADIUS;
    egui::Rounding {
        nw: if slot == 0 { radius } else { 0.0 },
        sw: if slot == 0 { radius } else { 0.0 },
        ne: if slot == 3 { radius } else { 0.0 },
        se: if slot == 3 { radius } else { 0.0 },
    }
}

/// Ordonnée du bord supérieur du cadre arrondi à l'abscisse `x`. La sélection
/// est découpée dans cette forme : elle ne peut donc jamais déborder des coins
/// du cadre, même pendant son glissement.
fn tab_frame_top(rect: egui::Rect, x: f32) -> f32 {
    let radius = TAB_FRAME_RADIUS;
    let left = rect.left();
    let right = rect.right();
    if x < left + radius {
        let offset = x - (left + radius);
        rect.top() + radius - (radius * radius - offset * offset).max(0.0).sqrt()
    } else if x > right - radius {
        let offset = x - (right - radius);
        rect.top() + radius - (radius * radius - offset * offset).max(0.0).sqrt()
    } else {
        rect.top()
    }
}

/// Contour d'une tranche verticale du cadre, entre `x0` et `x1`. Le polygone
/// suit le haut et le bas arrondis du cadre, puis ferme par des bords droits :
/// c'est exactement la part de la sélection pour ce segment.
fn tab_slice(rect: egui::Rect, x0: f32, x1: f32) -> Vec<egui::Pos2> {
    let x0 = x0.clamp(rect.left(), rect.right());
    let x1 = x1.clamp(x0, rect.right());
    let steps = (((x1 - x0) / 2.0).ceil() as usize).max(1);
    let mut points = Vec::with_capacity((steps + 1) * 2);
    for index in 0..=steps {
        let x = x0 + (x1 - x0) * index as f32 / steps as f32;
        points.push(egui::pos2(x, tab_frame_top(rect, x)));
    }
    for index in (0..=steps).rev() {
        let x = x0 + (x1 - x0) * index as f32 / steps as f32;
        let top = tab_frame_top(rect, x);
        points.push(egui::pos2(x, rect.bottom() - (top - rect.top())));
    }
    points
}

fn paint_tab_slice(
    painter: &egui::Painter,
    rect: egui::Rect,
    x0: f32,
    x1: f32,
    color: egui::Color32,
) {
    painter.add(egui::Shape::convex_polygon(
        tab_slice(rect, x0, x1),
        color,
        egui::Stroke::NONE,
    ));
}

/// Barre d'onglets segmentée : un seul fond, une tranche claire pour la
/// section ouverte. Le groupe garde une largeur naturelle et se place au
/// centre du haut de la page, encadré par les marges de celle-ci.
///
/// Le rectangle du cadre est rendu avec le choix pour que le test headless
/// puisse vérifier sa position réelle, sans passer par une capture.
fn tab_bar(ui: &mut egui::Ui, active: Tab) -> (Option<Tab>, egui::Rect, [egui::Rect; 4]) {
    let width = tab_bar_width(ui);
    let button_width = tab_button_width(ui);
    let tabs = [Tab::Updates, Tab::Deals, Tab::Gaming, Tab::Calendar];
    // Même animation que les onglets de la barre supérieure : le poids de
    // chaque section suit la courbe cubic_out, et la pastille glisse de
    // l'ancienne sélection vers la nouvelle au lieu de sauter.
    let mut selection = [0.0_f32; 4];
    for (slot, tab) in tabs.iter().copied().enumerate() {
        selection[slot] = ui.ctx().animate_bool_with_time_and_easing(
            ui.id().with(("news-tab-selected", tab as u8)),
            tab == active,
            TAB_SLIDE,
            egui::emath::easing::cubic_out,
        );
    }
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(width, NEWS_CONTROL_HEIGHT), egui::Sense::hover());
    let border = egui::Stroke::new(TAB_FRAME_STROKE, BORDER().gamma_multiply(0.55));
    ui.painter()
        .rect(rect, egui::Rounding::same(TAB_FRAME_RADIUS), CARD(), border);
    let mut chosen = None;
    let mut buttons = [egui::Rect::NOTHING; 4];
    let mut responses = Vec::with_capacity(4);
    // Les quatre boutons sont posés aux coordonnées du cadre, sans passage par
    // une ligne de mise en page intermédiaire : leur hauteur et leur axe sont
    // exactement ceux du sélecteur de sources et d'Actualiser.
    let mut left = rect.left();
    for (slot, tab) in tabs.iter().copied().enumerate() {
        let button = egui::Rect::from_min_size(
            egui::pos2(left, rect.top()),
            egui::vec2(button_width, TAB_BUTTON_HEIGHT),
        );
        buttons[slot] = button;
        let response = ui.interact(
            button,
            ui.id().with(("news-tab", slot)),
            egui::Sense::click(),
        );
        if active != tab {
            if response.is_pointer_button_down_on() {
                paint_tab_slice(
                    ui.painter(),
                    rect,
                    button.left(),
                    button.right(),
                    CARD_ACTIVE(),
                );
            } else if response.hovered() {
                paint_tab_slice(
                    ui.painter(),
                    rect,
                    button.left(),
                    button.right(),
                    CARD_HOVER(),
                );
            }
        }
        if response.clicked() {
            chosen = Some(tab);
        }
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        responses.push(response);
        left += button_width;
    }

    // La sélection est une tranche du cadre : elle traverse la barre pendant
    // le changement d'onglet, mais suit toujours les coins extérieurs.
    let total: f32 = selection.iter().sum();
    let active_slot = tabs.iter().position(|tab| *tab == active).unwrap_or(0);
    let center_x = if total > 0.001 {
        buttons
            .iter()
            .zip(selection.iter())
            .map(|(button, weight)| button.center().x * weight)
            .sum::<f32>()
            / total
    } else {
        buttons[active_slot].center().x
    };
    let current = egui::Rect::from_center_size(
        egui::pos2(center_x, rect.center().y),
        egui::vec2(button_width, TAB_BUTTON_HEIGHT),
    );
    paint_tab_slice(
        ui.painter(),
        rect,
        current.left(),
        current.right(),
        ACCENT(),
    );

    for ((slot, tab), response) in tabs.iter().copied().enumerate().zip(responses.iter()) {
        let selected = tab == active;
        if !selected && response.hovered() {
            ui.painter().rect_stroke(
                buttons[slot],
                tab_segment_rounding(slot),
                egui::Stroke::new(1.0_f32, BORDER()),
            );
        }
        if response.has_focus() {
            ui.painter().rect_stroke(
                buttons[slot],
                tab_segment_rounding(slot),
                egui::Stroke::new(1.5_f32, ACCENT()),
            );
        }
        let ink = blend(TEXT_DIM(), BACKGROUND(), selection[slot]);
        let galley = ui.painter().layout_no_wrap(
            tab.label().to_string(),
            egui::FontId::proportional(FONT_TAB),
            ink,
        );
        ui.painter()
            .galley(buttons[slot].center() - galley.size() / 2.0, galley, ink);
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, tab.label())
        });
    }
    // La bordure est repeinte après la sélection, qui touche maintenant les
    // bords haut, bas et extérieurs du cadre.
    ui.painter()
        .rect_stroke(rect, egui::Rounding::same(TAB_FRAME_RADIUS), border);
    (chosen, rect, buttons)
}

/// Contenu commun d'une carte, quel que soit l'onglet. Les trois sections
/// partagent ainsi le même rythme : provenance, date, titre, texte, prix
/// éventuel, puis action.
#[derive(Clone, Copy)]
pub(super) struct CardContent<'a> {
    pub label: &'a str,
    pub label_color: egui::Color32,
    /// Boutique de l'annonce : son logo accompagne le libellé de la pastille.
    pub store: Option<Store>,
    pub meta_right: &'a str,
    /// Nom du jeu, posé sur le visuel. Vide pour les cartes dont le titre
    /// porte déjà le nom du jeu (soldes, actus).
    pub media_label: &'a str,
    /// Provenance et date posées sur le visuel. Réservé aux actus, qui ne
    /// répètent donc ni le site ni la date dans le corps de la carte.
    pub media_byline: Option<MediaByline<'a>>,
    pub title: &'a str,
    pub excerpt: &'a str,
    pub price: Option<CardPrice<'a>>,
    pub action: &'a str,
}

#[derive(Clone, Copy)]
pub(super) struct MediaByline<'a> {
    pub label: &'a str,
    pub color: egui::Color32,
    pub icon: Option<&'a egui::TextureHandle>,
    pub right: &'a str,
}

#[derive(Clone, Copy)]
pub(super) struct CardPrice<'a> {
    pub current: &'a str,
    pub old: &'a str,
    pub free: bool,
}

fn news_date(date: i64) -> String {
    if date > 0 {
        lapinux_core::util::format_date(date)
    } else {
        String::new()
    }
}

fn update_content<'a>(article: &'a Article, date: &'a str) -> CardContent<'a> {
    CardContent {
        label: article.store.map_or("Steam", Store::label),
        label_color: store_color(article.store),
        store: article.store,
        meta_right: date,
        media_label: &article.game,
        media_byline: None,
        title: &article.news.title,
        excerpt: &article.news.excerpt,
        price: None,
        // La carte entière ouvre déjà l'annonce : pas de doublon textuel.
        action: "",
    }
}

fn hero_title_size(text_width: f32) -> f32 {
    if text_width < 520.0 {
        // À 980 px, le corps du héros fait moins de 390 px : 23 px ne
        // laissent pas tenir les titres réels en trois lignes.
        22.0
    } else {
        FONT_HERO_TITLE_WIDE
    }
}

/// Mélange deux teintes : `t` = 0 rend `a`, `t` = 1 rend `b`. Les états de
/// survol restent dans la palette du thème, clair comme sombre.
pub(super) fn blend(a: egui::Color32, b: egui::Color32, t: f32) -> egui::Color32 {
    let t = t.clamp(0.0, 1.0);
    let channel = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    egui::Color32::from_rgb(
        channel(a.r(), b.r()),
        channel(a.g(), b.g()),
        channel(a.b(), b.b()),
    )
}

/// Coupe un texte à la dernière frontière de mot avant `max` caractères, puis
/// pose une unique ellipsis. Couper au caractère près donnait des aperçus du
/// genre « détail co… » : une carte doit finir sur un mot.
fn clip_words(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let budget = max.saturating_sub(1);
    let mut used = 0usize;
    let mut boundary = 0usize;
    for (index, character) in text.char_indices() {
        if used >= budget {
            break;
        }
        if character.is_whitespace() && used > 0 {
            boundary = index;
        }
        used += 1;
    }
    let cut = if boundary == 0 {
        text.char_indices()
            .nth(budget)
            .map(|(index, _)| index)
            .unwrap_or(text.len())
    } else {
        boundary
    };
    // La ponctuation finale appartient à la phrase coupée, pas à l'ellipsis :
    // sans ce nettoyage, un aperçu finissait par « secondaires.... ».
    let mut out = text[..cut]
        .trim_end_matches([' ', '\t', '.', ',', ';', ':', '!', '?', '—', '–', '-'])
        .to_string();
    out.push('…');
    out
}

/// Corps du héros : le rythme est libre, l'image donnant la hauteur du bloc.
fn render_hero_body(ui: &mut egui::Ui, content: &CardContent<'_>, text_width: f32) {
    // Le héros vit dans une rangée horizontale dont l'espacement est nul ;
    // le corps doit retrouver son propre rythme, notamment entre deux prix.
    ui.spacing_mut().item_spacing.x = SPACE_2;
    // Une promotion porte déjà sa boutique sur le visuel : le corps ne la
    // répète pas en pastille.
    let store_on_media = content.store.is_some() && content.media_label.trim().is_empty();
    if content.media_byline.is_none() {
        ui.horizontal(|ui| {
            if !store_on_media {
                pill(ui, content.label, content.label_color);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if !content.meta_right.is_empty() {
                    ui.label(
                        egui::RichText::new(content.meta_right)
                            .size(FONT_META)
                            .color(TEXT_DIM()),
                    );
                }
            });
        });
        ui.add_space(SPACE_2);
    }

    let title = clip_words(content.title, card_title_chars(text_width, true));
    ui.add(
        egui::Label::new(
            egui::RichText::new(title)
                .size(hero_title_size(text_width))
                .strong()
                .color(TEXT()),
        )
        .wrap(),
    );

    if !content.excerpt.is_empty() {
        ui.add_space(SPACE_2);
        let excerpt = clip_words(content.excerpt, card_excerpt_chars(text_width, true));
        ui.add(
            egui::Label::new(
                egui::RichText::new(excerpt)
                    .size(if text_width < 520.0 {
                        FONT_HERO_BODY_NARROW
                    } else {
                        FONT_HERO_BODY
                    })
                    .color(body_ink()),
            )
            .wrap(),
        );
    }

    if let Some(price) = content.price {
        ui.add_space(SPACE_3);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = SPACE_2;
            ui.label(
                egui::RichText::new(price.current)
                    .size(24.0)
                    .strong()
                    .color(if price.free { OK } else { ACCENT() }),
            );
            if !price.old.is_empty() {
                ui.label(
                    egui::RichText::new(price.old)
                        .size(12.5)
                        .strikethrough()
                        .color(TEXT_DIM()),
                );
            }
        });
    }

    ui.add_space(SPACE_3);
    ui.label(
        egui::RichText::new(format!("{}  →", content.action))
            .size(FONT_ACTION)
            .strong()
            .color(ACCENT()),
    );
}

/// Corps de carte de grille : métadonnées, titre et aperçu occupent des blocs
/// fixes, puis l'action est collée en bas. D'une carte à l'autre, les titres,
/// les aperçus et les liens tombent donc exactement sur les mêmes lignes.
fn render_grid_body(ui: &mut egui::Ui, content: &CardContent<'_>, text_width: f32) {
    let body_top = ui.cursor().top();
    let compact = content.action.is_empty();
    let store_on_media = content.store.is_some() && content.media_label.trim().is_empty();
    ui.spacing_mut().item_spacing = egui::vec2(SPACE_2, 0.0);
    // Les cartes d'actualité portent déjà boutique, jeu et date sur leur
    // visuel : le corps commence directement par le titre de l'annonce. Une
    // offre non plus n'a pas de ligne d'échéance ici : la date rejoint le prix,
    // sous le titre, sans laisser de vide entre l'image et le titre. Les actus
    // déplacent leur site et leur date dans `media_byline`.
    if content.media_byline.is_none() && content.media_label.trim().is_empty() && !store_on_media {
        ui.horizontal(|ui| {
            pill(ui, content.label, content.label_color);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if !content.meta_right.is_empty() {
                    ui.label(
                        egui::RichText::new(content.meta_right)
                            .size(FONT_META)
                            .color(TEXT_DIM()),
                    );
                }
            });
        });
        ui.add_space(SPACE_2);
    }

    if compact {
        // Le titre est une ligne de repère : la carte reste compacte et
        // l'annonce complète s'ouvre au clic.
        ui.add(
            egui::Label::new(
                egui::RichText::new(content.title)
                    .size(FONT_CARD_TITLE)
                    .strong()
                    .color(TEXT()),
            )
            .truncate(),
        );
    } else {
        let title = clip_words(content.title, card_title_chars(text_width, false));
        ui.scope(|ui| {
            ui.set_min_height(TITLE_BLOCK_HEIGHT);
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.add(
                egui::Label::new(
                    egui::RichText::new(title)
                        .size(FONT_CARD_TITLE)
                        .strong()
                        .color(TEXT()),
                )
                .wrap(),
            );
        });
    }
    ui.add_space(6.0);

    if !content.excerpt.is_empty() {
        let excerpt = clip_words(content.excerpt, card_excerpt_chars(text_width, false));
        let excerpt_widget = || {
            egui::Label::new(
                egui::RichText::new(excerpt.clone())
                    .size(FONT_CARD_BODY)
                    .color(body_ink()),
            )
            .wrap()
        };
        if compact {
            ui.add(excerpt_widget());
        } else {
            ui.scope(|ui| {
                ui.set_min_height(EXCERPT_BLOCK_HEIGHT);
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.add(excerpt_widget());
            });
        }
    }

    if let Some(price) = content.price {
        ui.add_space(SPACE_2);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = SPACE_2;
            ui.label(
                egui::RichText::new(price.current)
                    .size(18.0)
                    .strong()
                    .color(if price.free { OK } else { ACCENT() }),
            );
            if !price.old.is_empty() {
                ui.label(
                    egui::RichText::new(price.old)
                        .size(11.0)
                        .strikethrough()
                        .color(TEXT_DIM()),
                );
            }
            if store_on_media && !content.meta_right.is_empty() {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Une seule ligne : la hauteur de la carte reste fixe, ce
                    // dont la grille virtuelle a besoin pour ses rangées.
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(content.meta_right)
                                .size(FONT_META)
                                .color(TEXT_DIM()),
                        )
                        .truncate(),
                    );
                });
            }
        });
    }

    if content.action.is_empty() {
        return;
    }

    // Le pied est peint à la même hauteur partout, même quand le texte est
    // court : la grille garde une ligne d'action parfaitement alignée.
    let used = ui.cursor().top() - body_top;
    let remaining = (GRID_BODY_INNER - used - CARD_FOOTER_HEIGHT).max(0.0);
    ui.add_space(remaining);
    let (rule, _) = ui.allocate_exact_size(egui::vec2(text_width, 1.0), egui::Sense::hover());
    ui.painter()
        .rect_filled(rule, 0.5, BORDER().gamma_multiply(0.35));
    ui.add_space(SPACE_2);
    ui.label(
        egui::RichText::new(format!("{}  →", content.action))
            .size(FONT_ACTION)
            .strong()
            .color(ACCENT()),
    );
}

/// Carte mise en avant : image 16:9 à gauche, texte à droite. L'image garde
/// son ratio ; elle n'est plus écrasée en bandeau sur toute la largeur.
pub(super) fn hero_card(
    ui: &mut egui::Ui,
    width: f32,
    image: Option<&egui::TextureHandle>,
    fallback: &str,
    badge: Option<(&str, egui::Color32)>,
    id: egui::Id,
    content: &CardContent<'_>,
) -> bool {
    let split = width >= HERO_SPLIT_MIN;
    let height = if split {
        hero_media_height(width)
    } else {
        grid_image_height(width).max(180.0) + 248.0
    };
    let origin = ui.cursor().min;
    let rect = egui::Rect::from_min_size(origin, egui::vec2(width, height));
    let hovered = ui.rect_contains_pointer(rect) && ui.is_enabled();
    let hover = ui
        .ctx()
        .animate_bool_with_time(id.with("hover"), hovered, 0.14);
    let pressed = hovered && ui.input(|input| input.pointer.primary_down());
    let fill = if pressed {
        blend(CARD_HOVER(), CARD_ACTIVE(), 0.7)
    } else {
        blend(CARD(), CARD_HOVER(), hover * 0.9)
    };
    let border = blend(BORDER().gamma_multiply(0.55), BORDER(), hover);
    let frame = egui::Frame::none()
        .fill(fill)
        .stroke(egui::Stroke::new(1.0_f32, border))
        .rounding(egui::Rounding::same(CARD_RADIUS))
        .show(ui, |ui| {
            // Un cadre hérite de la disposition de son parent : sans ce
            // `vertical`, le texte se placerait à côté de l'image.
            ui.vertical(|ui| {
                ui.set_width(width);
                if split {
                    let media_height = hero_media_height(width);
                    let media_width = (media_height * 16.0 / 9.0).min(width * 0.62);
                    let body_width = (width - media_width).max(220.0);
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        let media = cover_slot_with(
                            ui,
                            media_width,
                            media_height,
                            image,
                            fallback,
                            hero_cover_rounding(),
                        );
                        paint_media_overlay(ui, media, content, badge, None);
                        egui::Frame::none()
                            .inner_margin(egui::Margin::symmetric(SPACE_5, SPACE_5))
                            .show(ui, |ui| {
                                ui.vertical(|ui| {
                                    let inner = (body_width - SPACE_5 * 2.0).max(120.0);
                                    ui.set_width(inner);
                                    ui.set_min_height((media_height - SPACE_5 * 2.0).max(120.0));
                                    let text_width = inner.min(HERO_TEXT_MAX);
                                    ui.scope(|ui| {
                                        ui.set_width(text_width);
                                        render_hero_body(ui, content, text_width);
                                    });
                                });
                            });
                    });
                } else {
                    let media_height = grid_image_height(width).max(180.0);
                    let media =
                        cover_slot_with(ui, width, media_height, image, fallback, cover_rounding());
                    if let Some((text, color)) = badge {
                        overlay_badge(ui, media, text, color);
                    }
                    egui::Frame::none()
                        .inner_margin(egui::Margin::symmetric(CARD_PADDING, SPACE_4))
                        .show(ui, |ui| {
                            ui.vertical(|ui| {
                                let inner = (width - CARD_PADDING * 2.0).max(160.0);
                                ui.set_width(inner);
                                ui.set_min_height(212.0);
                                render_hero_body(ui, content, inner.min(HERO_TEXT_MAX));
                            });
                        });
                }
            });
        });
    let rect = frame.response.rect;
    let response = ui.interact(rect, id, egui::Sense::click());
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect.shrink(1.5),
            egui::Rounding::same((CARD_RADIUS - 1.5).max(0.0)),
            egui::Stroke::new(1.5_f32, ACCENT()),
        );
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.clicked()
}

/// Fondu d'apparition d'une carte : court, et calculé depuis l'instant où
/// l'annonce est arrivée. Une carte déjà affichée ne rejoue jamais le fondu.
fn reveal_alpha(arrived: Instant) -> f32 {
    (arrived.elapsed().as_secs_f32() / CARD_REVEAL).clamp(0.0, 1.0)
}

/// Hauteur totale d'une carte de grille : le visuel 16:9 et un corps fixe.
/// La grille virtuelle s'appuie sur cette valeur pour ne mettre en page que
/// les rangées visibles.
pub(super) fn grid_card_height_for(width: f32, compact: bool) -> f32 {
    let image = if compact {
        grid_image_height_compact(width)
    } else {
        grid_image_height(width)
    };
    let body = if compact {
        GRID_BODY_HEIGHT_COMPACT
    } else {
        GRID_BODY_HEIGHT
    };
    image + body
}

/// Carte de grille : image 16:9, puis un corps de hauteur fixe. Le fond, le
/// liseré et l'action réagissent au survol ; le clavier obtient un anneau de
/// focus, sans déplacer la carte d'un pixel.
pub(super) fn grid_card(
    ui: &mut egui::Ui,
    width: f32,
    image: Option<&egui::TextureHandle>,
    game_icon: Option<&egui::TextureHandle>,
    fallback: &str,
    badge: Option<(&str, egui::Color32)>,
    id: egui::Id,
    appear: f32,
    content: &CardContent<'_>,
) -> bool {
    let appear = appear.clamp(0.0, 1.0);
    let compact = content.action.is_empty();
    let body_inner = if compact {
        GRID_BODY_INNER_COMPACT
    } else {
        GRID_BODY_INNER
    };
    let image_height = if compact {
        grid_image_height_compact(width)
    } else {
        grid_image_height(width)
    };
    let height = grid_card_height_for(width, compact);
    let origin = ui.cursor().min;
    let rect = egui::Rect::from_min_size(origin, egui::vec2(width, height));
    let hovered = ui.rect_contains_pointer(rect) && ui.is_enabled();
    let hover = ui
        .ctx()
        .animate_bool_with_time(id.with("hover"), hovered, 0.14);
    let pressed = hovered && ui.input(|input| input.pointer.primary_down());
    let fill = if pressed {
        blend(CARD_HOVER(), CARD_ACTIVE(), 0.7)
    } else {
        blend(CARD(), CARD_HOVER(), hover * 0.9)
    };
    let border = blend(BORDER().gamma_multiply(0.55), BORDER(), hover);
    let frame = ui
        .scope(|ui| {
            ui.multiply_opacity(appear);
            egui::Frame::none()
                .fill(fill)
                .stroke(egui::Stroke::new(1.0_f32, border))
                .rounding(egui::Rounding::same(CARD_RADIUS))
                .show(ui, |ui| {
                    ui.vertical(|ui| {
                        ui.set_width(width);
                        // La carte est un bloc : l'image et son corps se touchent.
                        // Sans cela, l'espacement vertical de la grille (20 px)
                        // s'insère aussi entre le visuel et le titre.
                        ui.spacing_mut().item_spacing.y = 0.0;
                        let media = cover_slot_with(
                            ui,
                            width,
                            image_height,
                            image,
                            fallback,
                            cover_rounding(),
                        );
                        paint_media_overlay(ui, media, content, badge, game_icon);
                        egui::Frame::none()
                            .inner_margin(egui::Margin {
                                left: CARD_PADDING,
                                right: CARD_PADDING,
                                top: if compact {
                                    GRID_PAD_TOP_COMPACT
                                } else {
                                    GRID_PAD_TOP
                                },
                                bottom: if compact {
                                    GRID_PAD_BOTTOM_COMPACT
                                } else {
                                    GRID_PAD_BOTTOM
                                },
                            })
                            .show(ui, |ui| {
                                ui.vertical(|ui| {
                                    let inner = (width - CARD_PADDING * 2.0).max(120.0);
                                    ui.set_width(inner);
                                    ui.set_min_height(body_inner);
                                    render_grid_body(ui, content, inner);
                                });
                            });
                    });
                })
        })
        .inner;
    if appear < 1.0 {
        ui.ctx().request_repaint();
    }
    let rect = frame.response.rect;
    let response = ui.interact(rect, id, egui::Sense::click());
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect.shrink(1.5),
            egui::Rounding::same((CARD_RADIUS - 1.5).max(0.0)),
            egui::Stroke::new(1.5_f32, ACCENT()),
        );
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), content.title)
    });
    response.clicked()
}

fn hero_media_height(width: f32) -> f32 {
    (width * 0.30).clamp(280.0, 320.0)
}

/// Bandeau d'une carte. Le visuel garde le même rayon que le reste du bloc ;
/// sans image, un monogramme sur fond de carte évite le trou noir et garde
/// exactement la même hauteur.
pub(super) fn cover_slot(
    ui: &mut egui::Ui,
    width: f32,
    height: f32,
    image: Option<&egui::TextureHandle>,
    game: &str,
) -> egui::Rect {
    cover_slot_with(ui, width, height, image, game, cover_rounding())
}

fn cover_slot_with(
    ui: &mut egui::Ui,
    width: f32,
    height: f32,
    image: Option<&egui::TextureHandle>,
    game: &str,
    rounding: egui::Rounding,
) -> egui::Rect {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    if let Some(texture) = image {
        crate::paint_cover(ui, rect, texture, rounding);
        return rect;
    }
    let painter = ui.painter();
    painter.rect_filled(rect, rounding, CARD_HOVER());
    painter.rect_stroke(
        rect,
        rounding,
        egui::Stroke::new(1.0_f32, BORDER().gamma_multiply(0.7)),
    );
    let initial = game
        .chars()
        .find(|character| character.is_alphanumeric())
        .map(|character| character.to_uppercase().to_string())
        .unwrap_or_else(|| "?".to_string());
    let radius = (height * 0.16).clamp(22.0, 42.0);
    painter.circle_filled(rect.center(), radius, CARD());
    painter.circle_stroke(rect.center(), radius, egui::Stroke::new(1.0_f32, BORDER()));
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        initial,
        egui::FontId::proportional(radius * 0.9),
        TEXT_DIM(),
    );
    rect
}

pub(super) fn columns(width: f32) -> usize {
    if width >= 1600.0 {
        5
    } else if width >= 1360.0 {
        4
    } else if width >= 1000.0 {
        3
    } else if width >= 680.0 {
        2
    } else {
        1
    }
}

pub(super) fn grid_card_width(width: f32) -> f32 {
    let columns = columns(width) as f32;
    ((width - GRID_GAP * (columns - 1.0)) / columns).max(220.0)
}

/// Ancienne hauteur de bandeau, conservée pour le rendu historique. Les
/// nouvelles cartes utilisent `hero_media_height`.
pub(super) fn hero_image_height(width: f32) -> f32 {
    hero_media_height(width)
}

/// Vignette : 16/9, ratio des captures Steam et des visuels de presse.
pub(super) fn grid_image_height(width: f32) -> f32 {
    (width * 0.5625).min(240.0)
}

/// Visuel des annonces compactes : moins haut que la vignette 16/9, pour
/// laisser la place au texte sans étirer la carte.
pub(super) fn grid_image_height_compact(width: f32) -> f32 {
    (width * 0.42).clamp(120.0, 160.0)
}

/// Coupe un titre à la longueur qui tient en deux lignes dans la largeur de
/// texte donnée. La coupe est faite avant la mise en page : aucune ligne ne
/// dépasse ni ne fait grandir la carte.
pub(super) fn card_title_chars(width: f32, hero: bool) -> usize {
    let font = if hero {
        hero_title_size(width)
    } else {
        FONT_CARD_TITLE
    };
    // Trois lignes en large, quatre en étroit. En grille, on vise un peu moins
    // de trois lignes : un mot long ne doit jamais faire déborder la carte
    // dont la hauteur est fixe.
    let lines = if hero && width < 520.0 {
        4.0
    } else if hero {
        3.0
    } else {
        2.55
    };
    let count = (width / (font * 0.52) * lines) as usize;
    if hero {
        count.clamp(70, 160)
    } else {
        count.clamp(48, 112)
    }
}

pub(super) fn card_excerpt_chars(width: f32, hero: bool) -> usize {
    let font = if hero {
        if width < 520.0 {
            FONT_HERO_BODY_NARROW
        } else {
            FONT_HERO_BODY
        }
    } else {
        FONT_CARD_BODY
    };
    let count = (width / (font * 0.52) * if hero { 2.0 } else { 1.8 }) as usize;
    if hero {
        count.clamp(80, 220)
    } else {
        count.clamp(38, 96)
    }
}

/// Favicon d'une boutique, chargé dans le cache d'images des actualités.
pub(super) fn store_icon_url(store: Store) -> &'static str {
    match store {
        Store::Steam => "https://www.google.com/s2/favicons?sz=64&domain=store.steampowered.com",
        Store::Epic => "https://www.google.com/s2/favicons?sz=64&domain=store.epicgames.com",
        Store::Gog => "https://www.google.com/s2/favicons?sz=64&domain=www.gog.com",
        Store::Itch => "https://www.google.com/s2/favicons?sz=64&domain=itch.io",
    }
}

/// Logo local d'une boutique pour les menus de filtre. Les favicons distants
/// ne sont pas toujours disponibles et donnaient des carrés vides ; ces images
/// sont embarquées et décodées une seule fois par contexte.
pub(super) fn store_logo(ctx: &egui::Context, store: Store) -> Option<egui::TextureHandle> {
    let key = egui::Id::new(("news-store-logo", store.label()));
    if let Some(texture) = ctx.data(|data| data.get_temp::<egui::TextureHandle>(key)) {
        return Some(texture);
    }
    let bytes: &[u8] = match store {
        Store::Steam => include_bytes!("../../../assets/steam.png"),
        Store::Epic => include_bytes!("../../../assets/stores/epic.png"),
        Store::Gog => include_bytes!("../../../assets/stores/gog.png"),
        Store::Itch => include_bytes!("../../../assets/stores/itch.png"),
    };
    let image = image::load_from_memory(bytes).ok()?.to_rgba8();
    let thumbnail = image::imageops::thumbnail(&image, 64, 64);
    let color = egui::ColorImage::from_rgba_unmultiplied(
        [thumbnail.width() as usize, thumbnail.height() as usize],
        thumbnail.as_raw(),
    );
    let texture = ctx.load_texture(
        format!("news-store-logo-{}", store.label()),
        color,
        egui::TextureOptions::LINEAR,
    );
    ctx.data_mut(|data| data.insert_temp(key, texture.clone()));
    Some(texture)
}

/// Une ligne de filtre : icône, libellé, compteur. Un élément masqué est
/// barré au lieu d'être coché ; cliquer la ligne bascule son état.
pub(super) fn filter_choice(
    ui: &mut egui::Ui,
    icon: Option<&egui::TextureHandle>,
    label: &str,
    count: usize,
    hidden: bool,
    tint: egui::Color32,
) -> bool {
    let width = ui.available_width().max(190.0);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 26.0), egui::Sense::click());
    let center = rect.center().y;
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, egui::Rounding::same(3.0), CARD_HOVER());
    }
    let badge = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 16.0, center),
        egui::vec2(22.0, 22.0),
    );
    ui.painter()
        .rect_filled(badge, egui::Rounding::same(4.0), tint.gamma_multiply(0.14));
    ui.painter().rect_stroke(
        badge,
        egui::Rounding::same(4.0),
        egui::Stroke::new(1.0_f32, tint.gamma_multiply(0.38)),
    );
    let icon_rect = badge.shrink(3.0);
    if let Some(icon) = icon {
        let source = icon.size_vec2();
        if source.x > 0.0 && source.y > 0.0 {
            let scale = (icon_rect.width() / source.x).min(icon_rect.height() / source.y);
            let target = egui::Rect::from_center_size(icon_rect.center(), source * scale);
            ui.painter().image(
                icon.id(),
                target,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
    } else {
        ui.painter().text(
            badge.center(),
            egui::Align2::CENTER_CENTER,
            label
                .chars()
                .next()
                .unwrap_or('?')
                .to_uppercase()
                .to_string(),
            egui::FontId::proportional(9.0),
            egui::Color32::WHITE,
        );
    }
    let count_galley = ui.painter().layout_no_wrap(
        count.to_string(),
        egui::FontId::proportional(9.5),
        TEXT_DIM(),
    );
    let count_left = rect.right() - 8.0 - count_galley.size().x;
    ui.painter().galley(
        egui::pos2(count_left, center - count_galley.size().y / 2.0),
        count_galley.clone(),
        TEXT_DIM(),
    );
    let color = if hidden { TEXT_DIM() } else { TEXT() };
    let label_galley =
        ui.painter()
            .layout_no_wrap(label.to_string(), egui::FontId::proportional(12.0), color);
    let text_left = rect.left() + 36.0;
    let text_right = (count_left - 8.0).max(text_left + 10.0);
    let text_clip = egui::Rect::from_min_max(
        egui::pos2(text_left, rect.top()),
        egui::pos2(text_right, rect.bottom()),
    );
    let text_painter = ui.painter().with_clip_rect(text_clip);
    text_painter.galley(
        egui::pos2(text_left, center - label_galley.size().y / 2.0),
        label_galley.clone(),
        color,
    );
    if hidden {
        let end = (text_left + label_galley.size().x).min(text_right);
        text_painter.line_segment(
            [egui::pos2(text_left, center), egui::pos2(end, center)],
            egui::Stroke::new(1.0_f32, color),
        );
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.clicked()
}

/// Bouton de filtre et sa liste de choix. Le popup reste ouvert quand on
/// clique une ligne : on peut donc en barrer plusieurs d'affilée.
pub(super) fn source_menu(
    ui: &mut egui::Ui,
    id_salt: impl std::hash::Hash,
    label: &str,
    width: f32,
    add: impl FnOnce(&mut egui::Ui),
) {
    let id = ui.id().with(id_salt);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, NEWS_CONTROL_HEIGHT), egui::Sense::click());
    let fill = if response.is_pointer_button_down_on() {
        CARD_ACTIVE()
    } else if response.hovered() {
        CARD_HOVER()
    } else {
        CARD()
    };
    ui.painter().rect(
        rect,
        egui::Rounding::same(2.0),
        fill,
        egui::Stroke::new(1.0_f32, BORDER().gamma_multiply(0.55)),
    );
    let text_clip =
        egui::Rect::from_min_max(rect.min, egui::pos2(rect.right() - 24.0, rect.bottom()));
    ui.painter().with_clip_rect(text_clip).text(
        egui::pos2(rect.left() + 10.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(14.0),
        TEXT(),
    );
    ui.painter().text(
        egui::pos2(rect.right() - 10.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        "▾",
        egui::FontId::proportional(12.0),
        TEXT_DIM(),
    );
    if response.clicked() {
        ui.memory_mut(|memory| memory.toggle_popup(id));
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    // Le panneau ouvert reprend les angles droits des cartes Actualités. Ce
    // réglage ne touche que la durée du popup : le bouton fermé et le reste de
    // la page gardent leur style.
    let previous_style = ui.ctx().style();
    let previous_menu = ui.style().visuals.menu_rounding;
    let previous_window = ui.style().visuals.window_rounding;
    let square = egui::Rounding::same(0.0);
    ui.ctx().style_mut(|style| {
        style.visuals.menu_rounding = square;
        style.visuals.window_rounding = square;
    });
    ui.style_mut().visuals.menu_rounding = square;
    ui.style_mut().visuals.window_rounding = square;
    egui::popup_above_or_below_widget(
        ui,
        id,
        &response,
        egui::AboveOrBelow::Below,
        egui::PopupCloseBehavior::CloseOnClickOutside,
        |ui| {
            ui.set_min_width(width);
            add(ui);
        },
    );
    ui.ctx().set_style(previous_style);
    ui.style_mut().visuals.menu_rounding = previous_menu;
    ui.style_mut().visuals.window_rounding = previous_window;
}

/// Titre de section et compteur, sur une seule ligne. Les filtres vivent sur
/// leur propre ligne en dessous : ils ne compriment plus le titre et ne
/// changent jamais la hauteur du bloc quand le nombre de boutiques varie.
pub(super) fn section_title(ui: &mut egui::Ui, title: &str, count: Option<usize>) {
    ui.horizontal(|ui| {
        ui.set_min_height(22.0);
        ui.spacing_mut().item_spacing.x = SPACE_2;
        ui.label(
            egui::RichText::new(title)
                .size(FONT_SECTION)
                .strong()
                .color(TEXT()),
        );
        if let Some(count) = count {
            // Compteur discret, posé dans un liseré : il complète le titre au
            // lieu de rivaliser avec lui.
            let galley = ui.painter().layout_no_wrap(
                count.to_string(),
                egui::FontId::proportional(10.5),
                TEXT_DIM(),
            );
            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(galley.size().x + 14.0, 19.0),
                egui::Sense::hover(),
            );
            ui.painter().rect(
                rect,
                egui::Rounding::same(6.0),
                CARD(),
                egui::Stroke::new(1.0_f32, BORDER().gamma_multiply(0.55)),
            );
            ui.painter()
                .galley(rect.center() - galley.size() / 2.0, galley, TEXT_DIM());
        }
    });
}

pub(super) fn filter_row(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
        add(ui);
    });
}

/// Puce de filtre : le libellé et le compteur ont chacun leur poids visuel,
/// au lieu d'une chaîne « texte  nombre » collée.
pub(super) fn filter_chip(ui: &mut egui::Ui, label: &str, count: usize, selected: bool) -> bool {
    let label_ink = if selected { BACKGROUND() } else { TEXT() };
    let count_ink = if selected { BACKGROUND() } else { TEXT_DIM() };
    let label_galley = ui.painter().layout_no_wrap(
        label.to_string(),
        egui::FontId::proportional(12.0),
        label_ink,
    );
    let count_galley = ui.painter().layout_no_wrap(
        count.to_string(),
        egui::FontId::proportional(10.0),
        count_ink,
    );
    let padding = 12.0;
    let gap = 7.0;
    let count_padding = 5.0;
    let width =
        padding * 2.0 + label_galley.size().x + gap + count_galley.size().x + count_padding * 2.0;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 30.0), egui::Sense::click());
    let fill = if selected {
        ACCENT()
    } else if response.hovered() {
        CARD_ACTIVE()
    } else {
        CARD()
    };
    let painter = ui.painter();
    painter.rect_filled(rect, egui::Rounding::same(0.0), fill);
    if !selected {
        painter.rect_stroke(
            rect,
            egui::Rounding::same(0.0),
            egui::Stroke::new(
                1.0_f32,
                if response.hovered() {
                    BORDER()
                } else {
                    BORDER().gamma_multiply(0.55)
                },
            ),
        );
    }
    painter.galley(
        egui::pos2(
            rect.left() + padding,
            rect.center().y - label_galley.size().y / 2.0,
        ),
        label_galley,
        label_ink,
    );
    let badge = egui::Rect::from_min_size(
        egui::pos2(
            rect.right() - padding - count_galley.size().x - count_padding * 2.0,
            rect.center().y - 9.0,
        ),
        egui::vec2(count_galley.size().x + count_padding * 2.0, 18.0),
    );
    painter.rect_filled(
        badge,
        egui::Rounding::same(9.0),
        if selected {
            BACKGROUND().gamma_multiply(0.16)
        } else {
            CARD_ACTIVE()
        },
    );
    painter.galley(
        egui::pos2(
            badge.center().x - count_galley.size().x / 2.0,
            badge.center().y - count_galley.size().y / 2.0,
        ),
        count_galley,
        count_ink,
    );
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.clicked()
}

pub(super) fn skeleton_chips(ui: &mut egui::Ui, count: usize) {
    for width in [64.0, 88.0, 76.0, 96.0].iter().take(count) {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(*width, 30.0), egui::Sense::hover());
        ui.painter()
            .rect_filled(rect, egui::Rounding::same(0.0), CARD_ACTIVE());
    }
}

fn skeleton_bar(ui: &mut egui::Ui, width: f32, height: f32) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(width.max(12.0), height), egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, egui::Rounding::same(height / 2.0), CARD_ACTIVE());
}

fn skeleton_rect(ui: &mut egui::Ui, width: f32, height: f32, rounding: egui::Rounding) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    ui.painter().rect_filled(rect, rounding, CARD_HOVER());
}

fn skeleton_body(ui: &mut egui::Ui, width: f32, hero: bool) {
    skeleton_bar(ui, (width * 0.24).min(86.0), if hero { 20.0 } else { 16.0 });
    ui.add_space(if hero { 14.0 } else { 10.0 });
    skeleton_bar(ui, width * 0.84, if hero { 22.0 } else { 17.0 });
    ui.add_space(6.0);
    skeleton_bar(ui, width * 0.58, if hero { 22.0 } else { 17.0 });
    ui.add_space(if hero { 12.0 } else { 9.0 });
    skeleton_bar(ui, width * 0.94, 9.0);
    ui.add_space(6.0);
    skeleton_bar(ui, width * 0.72, 9.0);
    ui.add_space(if hero { 14.0 } else { 10.0 });
    skeleton_bar(ui, (width * 0.20).min(72.0), if hero { 20.0 } else { 16.0 });
}

/// Carte d'attente aux dimensions exactes d'une vraie carte : le passage du
/// squelette aux données ne déplace ni les colonnes ni le bandeau.
pub(super) fn skeleton_card(ui: &mut egui::Ui, width: f32, hero: bool) {
    skeleton_card_with(ui, width, hero, false);
}

fn skeleton_card_with(ui: &mut egui::Ui, width: f32, hero: bool, compact: bool) {
    let split = hero && width >= HERO_SPLIT_MIN;
    egui::Frame::none()
        .fill(CARD())
        .stroke(egui::Stroke::new(1.0_f32, BORDER().gamma_multiply(0.45)))
        .rounding(egui::Rounding::same(CARD_RADIUS))
        .show(ui, |ui| {
            ui.set_width(width);
            // Le squelette doit coller son visuel à son corps comme une vraie
            // carte, sans hériter de l'espacement des rangées.
            ui.spacing_mut().item_spacing.y = 0.0;
            if split {
                let media_height = hero_media_height(width);
                let media_width = (media_height * 16.0 / 9.0).min(width * 0.62);
                let body_width = (width - media_width).max(220.0);
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    skeleton_rect(ui, media_width, media_height, hero_cover_rounding());
                    egui::Frame::none()
                        .inner_margin(egui::Margin::symmetric(24.0, 22.0))
                        .show(ui, |ui| {
                            ui.vertical(|ui| {
                                let inner = (body_width - 48.0).max(120.0);
                                ui.set_width(inner);
                                ui.set_min_height((media_height - 44.0).max(120.0));
                                let text_width = inner.min(HERO_TEXT_MAX);
                                ui.scope(|ui| {
                                    ui.set_width(text_width);
                                    skeleton_body(ui, text_width, true);
                                });
                            });
                        });
                });
            } else if hero {
                let image_height = grid_image_height(width).max(180.0);
                skeleton_rect(ui, width, image_height, cover_rounding());
                egui::Frame::none()
                    .inner_margin(egui::Margin::symmetric(20.0, 18.0))
                    .show(ui, |ui| {
                        let inner = (width - 40.0).max(160.0);
                        ui.set_width(inner);
                        ui.set_min_height(212.0);
                        skeleton_body(ui, inner.min(HERO_TEXT_MAX), true);
                    });
            } else {
                let image_height = if compact {
                    grid_image_height_compact(width)
                } else {
                    grid_image_height(width)
                };
                skeleton_rect(ui, width, image_height, cover_rounding());
                egui::Frame::none()
                    .inner_margin(egui::Margin {
                        left: CARD_PADDING,
                        right: CARD_PADDING,
                        top: if compact {
                            GRID_PAD_TOP_COMPACT
                        } else {
                            GRID_PAD_TOP
                        },
                        bottom: if compact {
                            GRID_PAD_BOTTOM_COMPACT
                        } else {
                            GRID_PAD_BOTTOM
                        },
                    })
                    .show(ui, |ui| {
                        let inner = (width - CARD_PADDING * 2.0).max(120.0);
                        ui.set_width(inner);
                        ui.set_min_height(if compact {
                            GRID_BODY_INNER_COMPACT
                        } else {
                            GRID_BODY_INNER
                        });
                        skeleton_grid_body(ui, inner, compact);
                    });
            }
        });
}

fn skeleton_grid_body(ui: &mut egui::Ui, width: f32, compact: bool) {
    if !compact {
        skeleton_bar(ui, (width * 0.26).min(88.0), 16.0);
        ui.add_space(SPACE_2);
    }
    skeleton_bar(ui, width * 0.88, 15.0);
    if !compact {
        ui.add_space(6.0);
        skeleton_bar(ui, width * 0.62, 15.0);
    }
    ui.add_space(SPACE_2);
    skeleton_bar(ui, width * 0.94, 9.0);
    ui.add_space(6.0);
    skeleton_bar(ui, width * 0.70, 9.0);
    if !compact {
        ui.add_space(SPACE_3);
        skeleton_bar(ui, 68.0, 12.0);
    }
}

/// Squelettes des cartes sans action : même corps compact que les cartes
/// réelles, pour que l'arrivée des données ne fasse pas sauter la grille.
pub(super) fn skeleton_grid_compact(ui: &mut egui::Ui, width: f32, rows: usize) {
    skeleton_grid_with(ui, width, rows, true);
}

fn skeleton_grid_with(ui: &mut egui::Ui, width: f32, rows: usize, compact: bool) {
    let columns = columns(width);
    let card_width = grid_card_width(width);
    for row in 0..rows {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = GRID_GAP;
            for _ in 0..columns {
                // La rangée est horizontale ; la carte elle-même doit rester
                // verticale pour que son visuel soit au-dessus de son corps.
                ui.vertical(|ui| {
                    skeleton_card_with(ui, card_width, false, compact);
                });
            }
        });
        if row + 1 < rows {
            ui.add_space(GRID_GAP);
        }
    }
}

/// Même disposition d'attente que les annonces : un héros, puis les cartes
/// compactes qui vont réellement le suivre. Le passage aux données ne fait
/// donc pas sauter la grille des soldes ni des actus.
pub(super) fn skeleton_content_compact(ui: &mut egui::Ui, width: f32) {
    skeleton_card(ui, width, true);
    ui.add_space(GRID_GAP);
    skeleton_grid_compact(ui, width, 2);
}

/// État vide ou en erreur : une carte courte, sans filtres vides, avec une
/// action unique. Le bouton remonte au module qui sait relancer la lecture.
pub(super) fn status_card(
    ui: &mut egui::Ui,
    title: &str,
    message: &str,
    error: bool,
    action: Option<&str>,
) -> bool {
    let mut clicked = false;
    ui.add_space(18.0);
    ui.vertical_centered(|ui| {
        let card_width = ui.available_width().min(560.0).max(280.0);
        ui.set_width(card_width);
        egui::Frame::none()
            .fill(CARD())
            .stroke(if error {
                egui::Stroke::new(1.0_f32, DANGER.gamma_multiply(0.55))
            } else {
                egui::Stroke::new(1.0_f32, BORDER().gamma_multiply(0.45))
            })
            .rounding(egui::Rounding::same(CARD_RADIUS))
            .inner_margin(egui::Margin::symmetric(26.0, 22.0))
            .show(ui, |ui| {
                ui.set_width((card_width - 52.0).max(200.0));
                ui.vertical_centered(|ui| {
                    let color = if error { DANGER } else { ACCENT() };
                    let radius = 18.0;
                    let (rect, _) = ui.allocate_exact_size(
                        egui::vec2(radius * 2.0, radius * 2.0),
                        egui::Sense::hover(),
                    );
                    ui.painter()
                        .circle_filled(rect.center(), radius, color.gamma_multiply(0.16));
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        if error { "!" } else { "–" },
                        egui::FontId::proportional(16.5),
                        color,
                    );
                    ui.add_space(10.0);
                    ui.label(egui::RichText::new(title).size(16.0).strong().color(TEXT()));
                    ui.add_space(5.0);
                    ui.add(
                        egui::Label::new(egui::RichText::new(message).size(12.5).color(body_ink()))
                            .wrap(),
                    );
                    if let Some(label) = action {
                        ui.add_space(14.0);
                        if crate::action_button(ui, label, crate::ActionTone::Quiet).clicked() {
                            clicked = true;
                        }
                    }
                });
            });
    });
    clicked
}

pub(super) fn pill(ui: &mut egui::Ui, text: &str, color: egui::Color32) {
    // La teinte de la boutique reste reconnaissable, mais se fond dans la
    // carte : un liseré discret plutôt qu'une pastille saturée.
    let ink = blend(color, TEXT(), 0.30);
    egui::Frame::none()
        .fill(blend(CARD(), color, 0.12))
        .rounding(egui::Rounding::same(0.0))
        .inner_margin(egui::Margin::symmetric(7.0, 3.0))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).size(10.5).strong().color(ink));
        });
}

/// En-tête posé sur le visuel d'une annonce : nom du jeu et son icône en haut
/// au centre, puis boutique et date juste en dessous. Le groupe icône + nom et
/// les pastilles reposent sur un noir translucide, pour rester lisibles sur les
/// visuels clairs sans voiler toute l'image. Toutes les positions sont peintes
/// dans le rectangle du visuel, donc aucun texte ne peut déborder de la carte.
fn paint_media_header(
    ui: &egui::Ui,
    rect: egui::Rect,
    content: &CardContent<'_>,
    icon: Option<&egui::TextureHandle>,
) {
    if content.media_label.trim().is_empty() {
        return;
    }
    let painter = ui.painter().with_clip_rect(rect);
    let top = rect.top() + 12.0;
    let icon_side = 22.0;
    let chip_height = 24.0;
    // La pastille est un peu plus haute que l'icône : on aligne leurs centres
    // pour que les trois éléments forment une seule ligne de tête.
    let chip_top = top - (chip_height - icon_side) / 2.0;
    let chip_fill = egui::Color32::from_black_alpha(118);
    let chip_stroke = egui::Stroke::new(1.0_f32, egui::Color32::from_white_alpha(28));
    let white = egui::Color32::from_white_alpha(238);

    // Plateforme : marque de la boutique puis libellé. Les dimensions sont
    // calculées avant la peinture pour réserver la place du titre au centre.
    let store = content.store.unwrap_or(Store::Steam);
    let label_galley = painter.layout_no_wrap(
        content.label.to_string(),
        egui::FontId::proportional(10.5),
        white,
    );
    let logo = 13.0;
    let pad = 9.0;
    let chip_width = pad * 2.0 + logo + 6.0 + label_galley.size().x;
    let store_left = rect.left() + 12.0;
    let store_chip = egui::Rect::from_min_size(
        egui::pos2(store_left, chip_top),
        egui::vec2(chip_width, chip_height),
    );

    // Date : même hauteur de pastille, alignée au bord droit.
    let date = if content.meta_right.is_empty() {
        None
    } else {
        let date_galley = painter.layout_no_wrap(
            content.meta_right.to_string(),
            egui::FontId::proportional(10.5),
            white,
        );
        let date_width = date_galley.size().x + 18.0;
        let date_chip = egui::Rect::from_min_size(
            egui::pos2(rect.right() - 12.0 - date_width, chip_top),
            egui::vec2(date_width, chip_height),
        );
        Some((date_galley, date_chip))
    };

    // La ligne boutique / jeu / date n'est possible que si le titre centré
    // dispose encore d'une vraie largeur entre les deux pastilles. Sur une
    // carte étroite, on revient à deux rangs plutôt que de faire chevaucher.
    let center = rect.center().x;
    let date_left = date
        .as_ref()
        .map_or(rect.right() - 12.0, |(_, chip)| chip.left());
    let side_gap = 10.0;
    let left_room = center - (store_chip.right() + side_gap);
    let right_room = date_left - side_gap - center;
    let centered_room = (2.0 * left_room.min(right_room)).max(80.0);
    let same_row = centered_room >= 100.0;
    let chips_top = if same_row {
        chip_top
    } else {
        top + icon_side + 10.0
    };
    let store_chip =
        egui::Rect::from_min_size(egui::pos2(store_left, chips_top), store_chip.size());
    painter.rect_filled(store_chip, egui::Rounding::same(0.0), chip_fill);
    painter.rect_stroke(store_chip, egui::Rounding::same(0.0), chip_stroke);
    let logo_rect = egui::Rect::from_center_size(
        egui::pos2(store_chip.left() + pad + logo / 2.0, store_chip.center().y),
        egui::vec2(logo, logo),
    );
    crate::paint_store_mark_in(&painter, store, logo_rect, content.label_color);
    painter.galley(
        egui::pos2(
            logo_rect.right() + 6.0,
            store_chip.center().y - label_galley.size().y / 2.0,
        ),
        label_galley,
        white,
    );

    if let Some((date_galley, chip)) = date {
        let date_chip = egui::Rect::from_min_size(egui::pos2(chip.left(), chips_top), chip.size());
        painter.rect_filled(date_chip, egui::Rounding::same(0.0), chip_fill);
        painter.rect_stroke(date_chip, egui::Rounding::same(0.0), chip_stroke);
        painter.galley(
            egui::pos2(
                date_chip.center().x - date_galley.size().x / 2.0,
                date_chip.center().y - date_galley.size().y / 2.0,
            ),
            date_galley,
            white,
        );
    }

    // Titre du jeu : icône puis nom, centrés en haut du visuel.
    let gap = 7.0;
    let max_width = if same_row {
        centered_room
    } else {
        (rect.width() - 32.0).max(80.0)
    };
    let max_text = (max_width - icon_side - gap).max(44.0);
    let title_galley = fitted_title(&painter, content.media_label, max_text);
    let total = icon_side + gap + title_galley.size().x;
    let left = rect.center().x - total / 2.0;
    let icon_rect =
        egui::Rect::from_min_size(egui::pos2(left, top), egui::vec2(icon_side, icon_side));
    // Fond commun derrière l'icône et le nom : sans lui, le titre se perd sur
    // les visuels clairs. Même noir translucide que les pastilles.
    let title_bg = egui::Rect::from_min_max(
        egui::pos2(left - 7.0, top - 4.0),
        egui::pos2(left + total + 7.0, top + icon_side + 4.0),
    );
    painter.rect_filled(title_bg, egui::Rounding::same(0.0), chip_fill);
    if let Some(texture) = icon {
        // L'icône se pose sur le fond du groupe, sans liseré clair autour d'elle.
        painter.image(
            texture.id(),
            icon_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    } else {
        painter.rect_filled(icon_rect, egui::Rounding::same(0.0), chip_fill);
        let initial = content
            .media_label
            .chars()
            .find(|character| character.is_alphanumeric())
            .map(|character| character.to_uppercase().to_string())
            .unwrap_or_else(|| "?".to_string());
        painter.text(
            icon_rect.center(),
            egui::Align2::CENTER_CENTER,
            initial,
            egui::FontId::proportional(11.0),
            white,
        );
    }
    let text_at = egui::pos2(
        icon_rect.right() + gap,
        icon_rect.center().y - title_galley.size().y / 2.0,
    );
    painter.galley(
        text_at + egui::vec2(0.0, 1.0),
        title_galley.clone(),
        egui::Color32::from_black_alpha(170),
    );
    painter.galley(text_at, title_galley, white);
}

/// Réduit un texte à la largeur disponible, en coupant sur un mot quand c'est
/// possible. La mesure est faite avec la vraie police : aucun libellé ne peut
/// donc déborder de son visuel.
fn fitted_text(
    painter: &egui::Painter,
    text: &str,
    font: egui::FontId,
    color: egui::Color32,
    max_width: f32,
) -> std::sync::Arc<egui::Galley> {
    let full = painter.layout_no_wrap(text.to_string(), font.clone(), color);
    if full.size().x <= max_width {
        return full;
    }
    let chars: Vec<char> = text.chars().collect();
    let mut low = 0usize;
    let mut high = chars.len();
    while low < high {
        let mid = (low + high + 1) / 2;
        let mut candidate: String = chars[..mid].iter().collect();
        candidate.truncate(candidate.trim_end().len());
        candidate.push('…');
        let width = painter
            .layout_no_wrap(candidate, font.clone(), color)
            .size()
            .x;
        if width <= max_width {
            low = mid;
        } else {
            high = mid.saturating_sub(1);
        }
    }
    let mut cut = low;
    if let Some(space) = chars[..low]
        .iter()
        .rposition(|character| character.is_whitespace())
    {
        if space > 3 {
            cut = space;
        }
    }
    let mut label: String = chars[..cut].iter().collect();
    label.truncate(label.trim_end().len());
    label.push('…');
    painter.layout_no_wrap(label, font, color)
}

fn fitted_title(
    painter: &egui::Painter,
    text: &str,
    max_width: f32,
) -> std::sync::Arc<egui::Galley> {
    fitted_text(
        painter,
        text,
        egui::FontId::proportional(13.0),
        egui::Color32::from_white_alpha(238),
        max_width,
    )
}

pub(super) fn overlay_badge(ui: &egui::Ui, rect: egui::Rect, text: &str, fill: egui::Color32) {
    let galley = ui.painter().layout_no_wrap(
        text.to_string(),
        egui::FontId::proportional(12.0),
        BACKGROUND(),
    );
    let padding = egui::vec2(10.0, 5.0);
    let badge = egui::Rect::from_min_size(
        rect.left_top() + egui::vec2(12.0, 12.0),
        galley.size() + padding * 2.0,
    );
    ui.painter()
        .rect_filled(badge, egui::Rounding::same(0.0), fill);
    ui.painter()
        .galley(badge.min + padding, galley, BACKGROUND());
}

/// Positions de l'en-tête d'une offre : la boutique à gauche, le tag à
/// droite, tous deux à la même distance du bord haut.
fn deal_header_positions(
    rect: egui::Rect,
    store_size: egui::Vec2,
    tag_size: egui::Vec2,
) -> (egui::Rect, egui::Rect) {
    let margin = 12.0;
    let store = egui::Rect::from_min_size(
        egui::pos2(rect.left() + margin, rect.top() + margin),
        store_size,
    );
    let tag = egui::Rect::from_min_size(
        egui::pos2(rect.right() - margin - tag_size.x, rect.top() + margin),
        tag_size,
    );
    (store, tag)
}

/// En-tête d'une promotion : la plateforme en haut à gauche, le tag de
/// réduction en haut à droite. Le visuel reste lisible entre les deux.
fn paint_deal_header(
    ui: &egui::Ui,
    rect: egui::Rect,
    content: &CardContent<'_>,
    badge: Option<(&str, egui::Color32)>,
) {
    let painter = ui.painter().with_clip_rect(rect);
    let white = egui::Color32::from_white_alpha(238);
    let chip_fill = egui::Color32::from_black_alpha(118);
    let chip_stroke = egui::Stroke::new(1.0_f32, egui::Color32::from_white_alpha(28));
    let chip_height = 24.0;

    // Boutique : sa marque puis son libellé, comme sur les annonces.
    let store = content.store.unwrap_or(Store::Steam);
    let label_galley = painter.layout_no_wrap(
        content.label.to_string(),
        egui::FontId::proportional(10.5),
        white,
    );
    let logo = 13.0;
    let pad = 9.0;
    let chip_width = pad * 2.0 + logo + 6.0 + label_galley.size().x;
    let tag_padding = egui::vec2(10.0, 5.0);
    let tag = badge.map(|(text, _)| {
        painter.layout_no_wrap(
            text.to_string(),
            egui::FontId::proportional(12.0),
            BACKGROUND(),
        )
    });
    let tag_size = tag
        .as_ref()
        .map(|galley| galley.size() + tag_padding * 2.0)
        .unwrap_or(egui::Vec2::ZERO);
    let (store_chip, tag_chip) =
        deal_header_positions(rect, egui::vec2(chip_width, chip_height), tag_size);
    painter.rect_filled(store_chip, egui::Rounding::same(0.0), chip_fill);
    painter.rect_stroke(store_chip, egui::Rounding::same(0.0), chip_stroke);
    let logo_rect = egui::Rect::from_center_size(
        egui::pos2(store_chip.left() + pad + logo / 2.0, store_chip.center().y),
        egui::vec2(logo, logo),
    );
    crate::paint_store_mark_in(&painter, store, logo_rect, content.label_color);
    painter.galley(
        egui::pos2(
            logo_rect.right() + 6.0,
            store_chip.center().y - label_galley.size().y / 2.0,
        ),
        label_galley,
        white,
    );

    if let (Some((_, color)), Some(galley)) = (badge, tag) {
        painter.rect_filled(tag_chip, egui::Rounding::same(0.0), color);
        painter.galley(
            egui::pos2(
                tag_chip.center().x - galley.size().x / 2.0,
                tag_chip.center().y - galley.size().y / 2.0,
            ),
            galley,
            BACKGROUND(),
        );
    }
}

/// En-tête d'une actu du gaming : favicon et site dans le coin haut gauche,
/// date dans le coin haut droit. Les pastilles sombres gardent les deux
/// lisibles sur les photos claires, sans masquer le haut du visuel.
fn paint_media_byline(ui: &egui::Ui, rect: egui::Rect, byline: MediaByline<'_>) {
    let painter = ui.painter().with_clip_rect(rect);
    let margin = 12.0;
    let top = rect.top() + margin;
    let chip_height = 24.0;
    let icon_side = 14.0;
    let padding = 8.0;
    let gap = 6.0;
    let font = egui::FontId::proportional(10.5);
    let white = egui::Color32::from_white_alpha(238);
    let chip_fill = egui::Color32::from_black_alpha(118);
    let chip_stroke = egui::Stroke::new(1.0_f32, egui::Color32::from_white_alpha(28));

    let date = (!byline.right.is_empty())
        .then(|| painter.layout_no_wrap(byline.right.to_string(), font.clone(), white));
    let date_width = date
        .as_ref()
        .map(|galley| galley.size().x + 18.0)
        .unwrap_or(0.0);
    let date_left = rect.right() - margin - date_width;
    let label_room =
        (date_left - 8.0 - (rect.left() + margin) - padding * 2.0 - icon_side - gap).max(28.0);
    let label = fitted_text(&painter, byline.label, font, white, label_room);
    let source_width = padding * 2.0 + icon_side + gap + label.size().x;
    let source = egui::Rect::from_min_size(
        egui::pos2(rect.left() + margin, top),
        egui::vec2(source_width, chip_height),
    );
    painter.rect_filled(source, egui::Rounding::same(0.0), chip_fill);
    painter.rect_stroke(source, egui::Rounding::same(0.0), chip_stroke);

    let icon = egui::Rect::from_center_size(
        egui::pos2(source.left() + padding + icon_side / 2.0, source.center().y),
        egui::vec2(icon_side, icon_side),
    );
    if let Some(texture) = byline.icon {
        painter.image(
            texture.id(),
            icon,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    } else {
        painter.rect_filled(
            icon,
            egui::Rounding::same(3.0),
            byline.color.gamma_multiply(0.7),
        );
        let initial = byline
            .label
            .chars()
            .find(|character| character.is_alphanumeric())
            .map(|character| character.to_uppercase().to_string())
            .unwrap_or_else(|| "?".to_string());
        painter.text(
            icon.center(),
            egui::Align2::CENTER_CENTER,
            initial,
            egui::FontId::proportional(9.0),
            white,
        );
    }
    painter.galley(
        egui::pos2(icon.right() + gap, source.center().y - label.size().y / 2.0),
        label,
        white,
    );

    if let Some(galley) = date {
        let chip = egui::Rect::from_min_size(
            egui::pos2(date_left, top),
            egui::vec2(date_width, chip_height),
        );
        painter.rect_filled(chip, egui::Rounding::same(0.0), chip_fill);
        painter.rect_stroke(chip, egui::Rounding::same(0.0), chip_stroke);
        painter.galley(
            egui::pos2(
                chip.center().x - galley.size().x / 2.0,
                chip.center().y - galley.size().y / 2.0,
            ),
            galley,
            white,
        );
    }
}

/// En-tête posé sur un visuel : actu avec provenance, annonce complète,
/// promotion à deux coins, ou simple badge quand aucune boutique n'est connue.
fn paint_media_overlay(
    ui: &egui::Ui,
    rect: egui::Rect,
    content: &CardContent<'_>,
    badge: Option<(&str, egui::Color32)>,
    icon: Option<&egui::TextureHandle>,
) {
    if let Some(byline) = content.media_byline {
        paint_media_byline(ui, rect, byline);
    } else if !content.media_label.trim().is_empty() {
        paint_media_header(ui, rect, content, icon);
    } else if content.store.is_some() && badge.is_some() {
        paint_deal_header(ui, rect, content, badge);
    } else if let Some((text, color)) = badge {
        overlay_badge(ui, rect, text, color);
    }
}

pub(super) fn price_cents(cents: Option<u64>, currency: &str) -> String {
    let Some(cents) = cents else {
        return String::new();
    };
    let amount = lapinux_core::i18n::decimal(format!("{}.{:02}", cents / 100, cents % 100));
    match currency {
        "EUR" => format!("{amount} €"),
        "USD" => format!("{amount} $"),
        "GBP" => format!("{amount} £"),
        "" => amount,
        other => format!("{amount} {other}"),
    }
}

pub(super) fn store_color(store: Option<Store>) -> egui::Color32 {
    match store.unwrap_or(Store::Steam) {
        Store::Steam => egui::Color32::from_rgb(102, 192, 244),
        Store::Epic => egui::Color32::from_rgb(230, 232, 235),
        Store::Gog => egui::Color32::from_rgb(166, 115, 255),
        Store::Itch => egui::Color32::from_rgb(255, 105, 120),
    }
}

/// Empreinte de la liste demandée : identifiants et boutiques, dans l'ordre.
/// Elle évite de recalculer un vecteur complet à chaque image.
fn list_signature(games: &[GameSource]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    games.len().hash(&mut hasher);
    for game in games {
        game.appid.hash(&mut hasher);
        game.store.hash(&mut hasher);
    }
    hasher.finish()
}

fn steam_language(language: &str) -> String {
    match language {
        "fr" => "french".to_string(),
        _ => "english".to_string(),
    }
}

fn fetch_game(game: &GameSource, language: &str, client: &Client) -> Option<Vec<SteamNews>> {
    if game.store.is_some() {
        fetch_store_news(game.appid, client)
    } else {
        fetch_steam_news(game.appid, language)
    }
}

/// Les fiches de boutique sont déjà tenues par le daemon : son cache évite de
/// redemander la même fiche à chaque ouverture de la page.
fn fetch_store_news(appid: u32, client: &Client) -> Option<Vec<SteamNews>> {
    let details = client
        .request_with_timeout(&Req::StoreDetails { appid }, Duration::from_secs(90))
        .ok()?
        .into_data::<StoreDetails>()
        .ok()?;
    // Une même bannière pour quatre annonces ferait quatre cartes jumelles :
    // les captures de la fiche prennent le relais avant les petits visuels.
    let mut visuals: VecDeque<String> = std::iter::once(details.banner.clone())
        .chain(details.screenshots.iter().cloned())
        .chain([
            details.capsule.clone(),
            details.cover.clone(),
            details.trailer_poster.clone(),
        ])
        .filter(|url| !url.is_empty())
        .collect();
    let fallback = visuals.front().cloned().unwrap_or_default();
    let mut news: Vec<SteamNews> = details
        .news
        .into_iter()
        .take(NEWS_PER_GAME)
        .map(|news| StoreNews::as_steam(&news))
        .collect();
    for item in &mut news {
        if item.image.is_empty() {
            item.image = visuals.pop_front().unwrap_or_else(|| fallback.clone());
        }
    }
    Some(news)
}

fn fetch_steam_news(appid: u32, language: &str) -> Option<Vec<SteamNews>> {
    let url = format!(
        "https://api.steampowered.com/ISteamNews/GetNewsForApp/v2/?appid={appid}\
         &count={NEWS_PER_GAME}&feeds=steam_community_announcements&maxlength=0\
         &format=json&l={language}"
    );
    let bytes = crate::achievements::http_bytes(&url)?;
    let value = serde_json::from_slice::<serde_json::Value>(&bytes).ok()?;
    let mut news = news_from_json(&value);
    // Le flux n'expose pas toujours le visuel. L'en-tête du jeu est stable et
    // déjà servi par le CDN : plus besoin d'ouvrir la page de chaque annonce
    // pour y chercher `og:image`, un aller-retour lent et sérialisé par jeu.
    let fallback = news
        .iter()
        .find(|item| !item.image.is_empty())
        .map(|item| item.image.clone())
        .unwrap_or_else(|| steam_header_image(appid));
    for item in &mut news {
        if item.image.is_empty() {
            item.image = fallback.clone();
        }
    }
    Some(news)
}

fn steam_header_image(appid: u32) -> String {
    format!("https://cdn.cloudflare.steamstatic.com/steam/apps/{appid}/header.jpg")
}

fn json_str(value: Option<&serde_json::Value>) -> String {
    value
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Les publications de l'éditeur uniquement : le flux public mêle la presse
/// externe aux notes de mise à jour et aux actualités du jeu.
fn news_from_json(value: &serde_json::Value) -> Vec<SteamNews> {
    let Some(items) = value
        .pointer("/appnews/newsitems")
        .and_then(serde_json::Value::as_array)
    else {
        return Vec::new();
    };
    items
        .iter()
        .filter(|item| json_str(item.get("feedname")) == "steam_community_announcements")
        .filter(|item| !json_str(item.get("title")).trim().is_empty())
        .take(NEWS_PER_GAME)
        .map(|item| {
            let contents = json_str(item.get("contents"));
            let title = json_str(item.get("title")).trim().to_string();
            // Le flux recopie souvent le titre au début du corps, parfois suivi
            // de l'adresse de l'annonce. La carte ne doit pas répéter ce que le
            // lecteur vient de lire deux lignes plus haut.
            let body = strip_repeated_title(&bbcode_markdown(&contents), &title);
            let plain = strip_repeated_title(&plain_text(&contents), &title);
            SteamNews {
                title,
                excerpt: excerpt(&strip_urls(&plain)),
                body,
                date: item
                    .get("date")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(0),
                author: json_str(item.get("author")),
                url: json_str(item.get("url")),
                image: news_image(&contents).unwrap_or_default(),
            }
        })
        .collect()
}

/// Retire les blocs d'images et les chemins de visuels Steam d'une annonce.
/// Les images sont déjà portées séparément par `news_image`.
fn strip_news_images(contents: &str) -> String {
    let mut text = contents.to_string();
    while let Some(start) = text.find("[img") {
        let Some(open_end) = text[start..].find(']') else {
            break;
        };
        let Some(close) = text[start + open_end + 1..].find("[/img]") else {
            break;
        };
        let end = start + open_end + 1 + close + "[/img]".len();
        text.replace_range(start..end, "");
    }

    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(index) = rest.find("{STEAM_CLAN_IMAGE}") {
        out.push_str(&rest[..index]);
        let after = &rest[index + "{STEAM_CLAN_IMAGE}".len()..];
        let end = after
            .find(|character: char| {
                character.is_whitespace() || matches!(character, '"' | ']' | ')' | '[')
            })
            .unwrap_or(after.len());
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

/// Garantit une fin de ligne, et une ligne vide si `blank` est vrai. Les
/// balises de bloc BBCode se suivent souvent sans espace : sans ce passage,
/// les paragraphes et les listes se recollent en un seul bloc.
fn news_break(out: &mut String, blank: bool) {
    while out.ends_with(' ') || out.ends_with('\t') {
        out.pop();
    }
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    if blank && !out.is_empty() && !out.ends_with("\n\n") {
        out.push('\n');
    }
}

/// Convertit le BBCode des annonces Steam en Markdown simple, celui que
/// `markdown_body` sait déjà peindre : titres, listes, gras, italique, citations
/// et liens. Les annonces sans balise traversent la conversion sans changer.
fn bbcode_markdown(contents: &str) -> String {
    let cleaned = strip_news_images(contents);
    let mut out = String::with_capacity(cleaned.len());
    let mut rest = cleaned.as_str();
    let mut in_list = 0usize;
    let mut in_heading = false;
    let mut urls: Vec<String> = Vec::new();
    while let Some(open) = rest.find('[') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find(']') else {
            out.push('[');
            rest = after;
            continue;
        };
        let tag = &after[..close];
        let trimmed = tag.trim();
        let closing = trimmed.starts_with('/');
        let name = trimmed
            .trim_start_matches('/')
            .split(|character: char| character.is_whitespace() || character == '=')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        match name.as_str() {
            "b" => {
                if !in_heading {
                    out.push_str("**");
                }
            }
            "i" => {
                if !in_heading {
                    out.push('*');
                }
            }
            "u" => {}
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                if closing {
                    in_heading = false;
                    news_break(&mut out, true);
                } else {
                    news_break(&mut out, true);
                    let level = name[1..].bytes().next().unwrap_or(b'3') - b'0';
                    out.push_str(&"#".repeat(level as usize));
                    out.push(' ');
                    in_heading = true;
                }
            }
            "p" => {
                if in_list == 0 {
                    news_break(&mut out, true);
                }
            }
            "br" => out.push('\n'),
            "list" | "olist" => {
                if closing {
                    in_list = in_list.saturating_sub(1);
                } else {
                    in_list += 1;
                }
                news_break(&mut out, false);
            }
            "*" => {
                if closing {
                    news_break(&mut out, false);
                } else {
                    news_break(&mut out, false);
                    for _ in 1..in_list {
                        out.push_str("  ");
                    }
                    out.push_str("- ");
                }
            }
            "quote" => {
                if closing {
                    news_break(&mut out, true);
                } else {
                    news_break(&mut out, false);
                    out.push_str("> ");
                }
            }
            "code" => {
                news_break(&mut out, true);
                out.push_str("```");
                news_break(&mut out, true);
            }
            "hr" => {
                news_break(&mut out, true);
                out.push_str("---");
                news_break(&mut out, true);
            }
            "url" => {
                if closing {
                    let url = urls.pop().unwrap_or_default();
                    out.push(']');
                    if !url.is_empty() {
                        out.push('(');
                        out.push_str(&url);
                        out.push(')');
                    }
                } else {
                    let url = tag
                        .split_once('=')
                        .map(|(_, value)| {
                            value
                                .trim()
                                .trim_matches('"')
                                .trim_matches('\'')
                                .to_string()
                        })
                        .unwrap_or_default();
                    urls.push(url);
                    out.push('[');
                }
            }
            "table" | "tr" => news_break(&mut out, false),
            "td" | "th" => out.push(' '),
            _ => {}
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    normalize_news_lines(&out)
}

/// Nettoie les lignes d'une annonce sans perdre ses blocs : une ligne vide
/// entre deux paragraphes, une seule, et aucun espace en fin de ligne.
fn normalize_news_lines(text: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() {
            if lines.last().is_some_and(|last| !last.is_empty()) {
                lines.push(String::new());
            }
        } else {
            lines.push(line);
        }
    }
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    lines.join("\n").trim().to_string()
}

/// Texte brut d'une annonce : le BBCode est converti en Markdown, puis les
/// marqueurs sont retirés. L'aperçu de carte garde ainsi les mots, sans
/// afficher `**`, `#` ou `-`.
fn plain_text(contents: &str) -> String {
    markdown_to_plain(&bbcode_markdown(contents))
}

fn markdown_to_plain(markdown: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    for line in markdown.lines() {
        let line = line.trim();
        if line.starts_with("```") {
            continue;
        }
        let line = line.trim_start_matches('#').trim_start();
        let line = line.strip_prefix("- ").unwrap_or(line);
        let line = line.strip_prefix("> ").unwrap_or(line);
        let line = line
            .replace("**", "")
            .replace("__", "")
            .replace('`', "")
            .replace('*', "");
        let line = strip_markdown_links(&line);
        if line.is_empty() {
            if lines.last().is_some_and(|last| !last.is_empty()) {
                lines.push(String::new());
            }
        } else {
            lines.push(line);
        }
    }
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    lines.join("\n").trim().to_string()
}

fn strip_markdown_links(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        if let Some(mid) = after.find("](") {
            if let Some(close) = after[mid + 2..].find(')') {
                out.push_str(&after[..mid]);
                rest = &after[mid + 2 + close + 1..];
                continue;
            }
        }
        out.push('[');
        rest = after;
    }
    out.push_str(rest);
    out
}

fn excerpt(text: &str) -> String {
    let mut chars = text.chars();
    let short: String = chars.by_ref().take(EXCERPT_CHARS).collect();
    if chars.next().is_some() {
        format!("{short}…")
    } else {
        short
    }
}

/// Retire un titre recopié au début du corps. La comparaison ignore la casse
/// et la ponctuation de liaison, pas les mots : on ne coupe jamais un texte
/// qui commence simplement par les mêmes lettres.
fn strip_repeated_title(text: &str, title: &str) -> String {
    let text = text.trim();
    let title = title.trim();
    if title.is_empty() {
        return text.to_string();
    }
    // Une annonce formatée commence souvent par `## Titre` ou `**Titre**` :
    // le titre tient alors toute la première ligne, qu'il faut retirer sans
    // emporter le premier titre de section qui la suit.
    let first_line = text.lines().next().unwrap_or("");
    let first_line_title = first_line
        .trim_start_matches(|character: char| matches!(character, '#' | '*' | '>' | ' '))
        .trim_end_matches(|character: char| matches!(character, '*' | '#' | ' '));
    if first_line_title.eq_ignore_ascii_case(title) {
        let rest = text
            .lines()
            .skip(1)
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string();
        if !rest.is_empty() {
            return rest;
        }
    }
    let lead =
        text.trim_start_matches(|character: char| matches!(character, '#' | '*' | '>' | ' '));
    let prefix: String = lead.chars().take(title.chars().count()).collect();
    if prefix.to_lowercase() != title.to_lowercase() {
        return text.to_string();
    }
    let rest: String = lead.chars().skip(title.chars().count()).collect();
    let rest = rest.trim_start_matches('*');
    let rest = rest.trim_start_matches(|character: char| {
        character.is_whitespace()
            || matches!(character, '!' | ':' | '-' | '—' | '–' | '|' | '.' | ',')
    });
    if rest.is_empty() {
        text.to_string()
    } else {
        rest.trim().to_string()
    }
}

/// Une adresse collée à la fin d'un paragraphe n'apporte rien dans un aperçu :
/// la carte entière ouvre déjà la source.
fn strip_urls(text: &str) -> String {
    text.split_whitespace()
        .filter(|word| !word.to_ascii_lowercase().starts_with("http"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Première image intégrée au BBCode d'une annonce, quand Steam en publie une.
fn news_image(contents: &str) -> Option<String> {
    const CLAN: &str = "{STEAM_CLAN_IMAGE}";
    if let Some(index) = contents.find(CLAN) {
        let path = contents[index + CLAN.len()..].trim_start_matches('/');
        let end = path
            .find(|character: char| {
                character.is_whitespace() || matches!(character, '"' | ']' | ')' | '[')
            })
            .unwrap_or(path.len());
        if end > 0 {
            return Some(format!(
                "https://clan.akamai.steamstatic.com/images/{}",
                &path[..end]
            ));
        }
    }
    let start = contents.find("[img")?;
    let rest = &contents[start..];
    let link = rest.find("http")?;
    let rest = &rest[link..];
    let end = rest
        .find(|character: char| {
            character.is_whitespace() || matches!(character, '"' | ']' | ')' | '[')
        })
        .unwrap_or(rest.len());
    let url = &rest[..end];
    url.starts_with("https://").then(|| url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn l_accueil_affiche_les_annonces_recentes_et_ouvre_le_lecteur() {
        let ctx = egui::Context::default();
        let games = vec![GameSource {
            appid: 42,
            name: "Jeu".into(),
            store: None,
            icon: None,
        }];
        let mut page = Page::default();
        page.updates.games = games.clone();
        page.updates.signature = list_signature(&games);
        page.updates.language = "fr".into();
        page.updates.queued.insert((42, None));
        for date in [1, 3, 2] {
            page.updates.articles.push(Article {
                appid: 42,
                game: "Jeu".into(),
                store: None,
                icon: None,
                news: SteamNews {
                    title: format!("Patch {date}"),
                    date,
                    url: format!("https://example.test/{date}"),
                    ..Default::default()
                },
                arrived: Instant::now(),
            });
        }
        // Une publication ordinaire plus récente fait partie des annonces
        // de l'Accueil, au même titre qu'une note de mise à jour.
        page.updates.articles.push(Article {
            appid: 42,
            game: "Jeu".into(),
            store: None,
            icon: None,
            news: SteamNews {
                title: "Dev blog".into(),
                date: 4,
                url: "https://example.test/blog".into(),
                ..Default::default()
            },
            arrived: Instant::now(),
        });
        let (updates, loading) = page.home_updates(&ctx, &games, "fr");
        assert!(!loading);
        assert_eq!(
            updates.iter().map(|item| item.date).collect::<Vec<_>>(),
            vec![4, 3, 2, 1]
        );
        let reader = page.open_home_update(&updates[0]).unwrap();
        assert_eq!(reader.items[reader.at].title, "Dev blog");
    }

    #[test]
    fn le_cache_affiche_les_annonces_sans_requete() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("news-feed.json");
        let games = vec![GameSource {
            appid: 42,
            name: "Jeu".into(),
            store: None,
            icon: None,
        }];
        let mut page = Page::default().with_cache(path.clone());
        page.updates.games = games.clone();
        page.updates.signature = list_signature(&games);
        page.updates.language = "fr".into();
        page.updates.articles.push(Article {
            appid: 42,
            game: "Jeu".into(),
            store: None,
            icon: None,
            news: SteamNews {
                title: "Patch en cache".into(),
                date: 7,
                url: "https://example.test/cache".into(),
                ..Default::default()
            },
            arrived: Instant::now(),
        });
        page.updates.cache_dirty = true;
        page.updates.save_cache();
        assert!(path.exists());

        let mut reloaded = Page::default().with_cache(path);
        let (updates, loading) = reloaded.home_updates(&egui::Context::default(), &games, "fr");
        assert!(!loading);
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].title, "Patch en cache");
    }

    #[test]
    fn only_editor_announcements_are_kept() {
        let value = json!({
            "appnews": {
                "newsitems": [
                    {
                        "feedname": "PlayGround.ru",
                        "title": "Article de presse",
                        "contents": "Un article externe",
                        "date": 1,
                        "url": "https://example.com/press"
                    },
                    {
                        "feedname": "steam_community_announcements",
                        "title": "Mise à jour 1.2",
                        "contents": "Le mode coop arrive.",
                        "date": 2,
                        "url": "https://example.com/news"
                    }
                ]
            }
        });
        let news = news_from_json(&value);
        assert_eq!(news.len(), 1);
        assert_eq!(news[0].title, "Mise à jour 1.2");
    }

    #[test]
    fn bbcode_and_clan_images_are_cleaned() {
        let raw = "[b]Mise à jour[/b] [url=https://example.com]lire[/url]\n\
                   {STEAM_CLAN_IMAGE}/123/photo.jpg [img]https://example.com/ignored.jpg[/img]";
        let formatted = bbcode_markdown(raw);
        assert!(formatted.contains("**Mise à jour**"), "{formatted}");
        assert!(
            formatted.contains("[lire](https://example.com)"),
            "{formatted}"
        );
        assert!(!formatted.contains("[img]"), "{formatted}");
        assert!(!formatted.contains("STEAM_CLAN_IMAGE"), "{formatted}");
        assert_eq!(plain_text(raw), "Mise à jour lire");
        // Le Markdown déjà présent reste lisible, mais l'aperçu n'en garde
        // pas les marqueurs : c'est le lecteur qui les met en forme.
        let notes = plain_text("# Correctifs\n- **Crash** corrigé\n- 60 FPS stables");
        assert_eq!(notes, "Correctifs\nCrash corrigé\n60 FPS stables");
        assert_eq!(
            news_image("{STEAM_CLAN_IMAGE}/123/photo.jpg").as_deref(),
            Some("https://clan.akamai.steamstatic.com/images/123/photo.jpg")
        );
    }

    #[test]
    fn steam_hotfix_bbcode_becomes_formatted_markdown() {
        let raw = "[h2]🔧 Hotfix 19 - 01/30/25[/h2]\
                   [h3][b]New Features[/b] [b]& Updates[/b][/h3]\
                   [list][*][p]German localization is now available![/p][/*]\
                   [*][p]Spanish localization is now available![/p][/*][/list]\
                   [h3][b]Bug Fixes[/b][/h3]\
                   [list][*][p]Fixed the Seasoned Teacher icon.[/p][/*][/list]";
        let markdown = bbcode_markdown(raw);
        assert!(markdown.contains("## 🔧 Hotfix 19"), "{markdown}");
        assert!(
            markdown.contains("\n### New Features & Updates"),
            "{markdown}"
        );
        assert!(markdown.contains("\n- German localization"), "{markdown}");
        assert!(markdown.contains("\n- Spanish localization"), "{markdown}");
        assert!(markdown.contains("\n### Bug Fixes"), "{markdown}");
        assert!(!markdown.contains("[list]"), "{markdown}");
        assert!(!markdown.contains("**"), "{markdown}");
        let plain = plain_text(raw);
        assert!(plain.contains("Hotfix 19"), "{plain}");
        assert!(plain.contains("New Features & Updates"), "{plain}");
        assert!(!plain.contains('#'), "{plain}");
        assert!(!plain.contains('*'), "{plain}");
    }

    #[test]
    fn a_repeated_title_leaves_the_formatted_body() {
        assert_eq!(
            strip_repeated_title(
                "## Mise à jour 1.2\n\n### Nouveautés\n- Le mode coop arrive.",
                "Mise à jour 1.2"
            ),
            "### Nouveautés\n- Le mode coop arrive."
        );
        assert_eq!(
            strip_repeated_title("**Mise à jour 1.2** : correctifs", "Mise à jour 1.2"),
            "correctifs"
        );
    }

    #[test]
    fn the_reader_receives_the_formatted_body() {
        let value = json!({
            "appnews": {
                "newsitems": [{
                    "feedname": "steam_community_announcements",
                    "title": "Mise à jour 1.2",
                    "contents": "[h2]Mise à jour 1.2[/h2][h3][b]Nouveautés[/b][/h3][list][*][p]Le mode coop arrive.[/p][/*][/list]",
                    "date": 2,
                    "author": "Studio",
                    "url": "https://example.com/news"
                }]
            }
        });
        let news = news_from_json(&value);
        assert_eq!(news.len(), 1);
        assert_eq!(
            news[0].body, "### Nouveautés\n\n- Le mode coop arrive.",
            "le lecteur doit recevoir les titres et la liste"
        );
        assert_eq!(news[0].excerpt, "Nouveautés Le mode coop arrive.");
    }

    #[test]
    fn the_excerpt_is_cut_with_an_ellipsis() {
        let text = "a".repeat(EXCERPT_CHARS + 1);
        let short = excerpt(&text);
        assert!(short.ends_with('…'));
        assert_eq!(short.chars().count(), EXCERPT_CHARS + 1);
    }

    #[test]
    fn a_missing_announcement_image_uses_the_steam_header() {
        assert_eq!(
            steam_header_image(620),
            "https://cdn.cloudflare.steamstatic.com/steam/apps/620/header.jpg"
        );
    }

    #[test]
    fn a_clipped_card_text_ends_on_a_word() {
        let text = "Nouvelle passe sur les builds et les temps de recharge";
        let short = clip_words(text, 30);
        assert!(short.ends_with('…'));
        assert!(short.chars().count() <= 30);
        assert!(text.starts_with(short.trim_end_matches('…')));
        assert_eq!(clip_words("Court", 30), "Court");
    }

    #[test]
    fn a_failed_game_does_not_stop_the_analysis() {
        let ctx = egui::Context::default();
        let games = vec![
            GameSource {
                appid: 1,
                name: "Sans nouvelles".into(),
                store: None,
                icon: None,
            },
            GameSource {
                appid: 2,
                name: "Avec nouvelles".into(),
                store: Some(Store::Gog),
                icon: None,
            },
        ];
        let mut feed = Feed {
            games: games.clone(),
            loading: true,
            ..Default::default()
        };
        let (tx, rx) = mpsc::channel();
        feed.rx = Some(rx);
        for game in &games {
            let key = (game.appid, game.store);
            feed.queued.insert(key);
            feed.pending.insert(key);
        }
        let update = SteamNews {
            title: "Mise à jour 1.0".into(),
            excerpt: "Correctifs et équilibrages".into(),
            body: String::new(),
            date: 10,
            author: String::new(),
            url: "https://example.test/update".into(),
            image: String::new(),
        };
        tx.send((games[0].clone(), Some(Vec::new()))).unwrap();
        tx.send((games[1].clone(), Some(vec![update]))).unwrap();
        feed.poll(&ctx);
        assert_eq!(feed.analysed, 2);
        assert!(!feed.loading);
        assert!(feed.rx.is_none());
        assert_eq!(feed.articles.len(), 1);
        assert_eq!(feed.articles[0].game, "Avec nouvelles");
    }

    #[test]
    fn the_finished_feed_is_sorted_by_most_recent_date() {
        let ctx = egui::Context::default();
        let games = vec![
            GameSource {
                appid: 1,
                name: "Jeu ancien".into(),
                store: None,
                icon: None,
            },
            GameSource {
                appid: 2,
                name: "Jeu récent".into(),
                store: Some(Store::Gog),
                icon: None,
            },
        ];
        let mut feed = Feed {
            games: games.clone(),
            loading: true,
            ..Default::default()
        };
        let (tx, rx) = mpsc::channel();
        feed.rx = Some(rx);
        for game in &games {
            let key = (game.appid, game.store);
            feed.queued.insert(key);
            feed.pending.insert(key);
        }
        let old = SteamNews {
            title: "Mise à jour 1.0".into(),
            excerpt: "Correctifs".into(),
            body: String::new(),
            date: 10,
            author: String::new(),
            url: "https://example.test/old".into(),
            image: String::new(),
        };
        let recent = SteamNews {
            title: "Mise à jour 2.0".into(),
            excerpt: "Nouveautés".into(),
            body: String::new(),
            date: 20,
            author: String::new(),
            url: "https://example.test/recent".into(),
            image: String::new(),
        };
        tx.send((games[0].clone(), Some(vec![old]))).unwrap();
        tx.send((games[1].clone(), Some(vec![recent]))).unwrap();
        feed.poll(&ctx);
        assert!(!feed.loading);
        assert_eq!(feed.articles.len(), 2);
        assert_eq!(feed.articles[0].news.date, 20);
        assert_eq!(feed.articles[1].news.date, 10);
        assert_eq!(feed.articles[0].game, "Jeu récent");
    }

    #[test]
    fn the_announcements_feed_keeps_ordinary_posts() {
        let ctx = egui::Context::default();
        let game = GameSource {
            appid: 7,
            name: "Jeu".into(),
            store: None,
            icon: None,
        };
        let mut feed = Feed {
            games: vec![game.clone()],
            loading: true,
            ..Default::default()
        };
        let (tx, rx) = mpsc::channel();
        feed.rx = Some(rx);
        feed.queued.insert((game.appid, game.store));
        feed.pending.insert((game.appid, game.store));
        tx.send((
            game,
            Some(vec![SteamNews {
                title: "Dev blog".into(),
                excerpt: "Les coulisses du studio.".into(),
                date: 4,
                url: "https://example.test/blog".into(),
                ..Default::default()
            }]),
        ))
        .unwrap();
        feed.poll(&ctx);
        assert_eq!(feed.articles.len(), 1);
        assert_eq!(feed.articles[0].news.title, "Dev blog");
    }

    #[test]
    fn a_library_change_keeps_the_cards_already_loaded() {
        let game = GameSource {
            appid: 1,
            name: "Déjà chargé".into(),
            store: None,
            icon: None,
        };
        let mut feed = Feed {
            articles: vec![Article {
                appid: game.appid,
                game: game.name.clone(),
                store: None,
                icon: None,
                news: SteamNews {
                    title: "Mise à jour 1.0".into(),
                    excerpt: "Correctifs".into(),
                    body: String::new(),
                    date: 10,
                    author: String::new(),
                    url: "https://example.test/update".into(),
                    image: String::new(),
                },
                arrived: Instant::now(),
            }],
            games: vec![game.clone()],
            signature: 1,
            language: "fr".into(),
            queued: [(game.appid, game.store)].into_iter().collect(),
            analysed: 1,
            ..Default::default()
        };
        let added = GameSource {
            appid: 2,
            name: "Arrivé après coup".into(),
            store: Some(Store::Epic),
            icon: None,
        };
        feed.sync_games(&[game, added.clone()], 2);
        assert_eq!(feed.articles.len(), 1);
        assert_eq!(feed.analysed, 1);
        assert_eq!(feed.games.len(), 2);
        assert_eq!(feed.signature, 2);
        assert!(feed
            .games
            .iter()
            .any(|game| !feed.queued.contains(&(game.appid, game.store))));
    }

    /// Une offre pose sa plateforme à gauche du visuel et son tag de promo à
    /// droite, sans les empiler ni les coller du même côté.
    #[test]
    fn deal_header_puts_the_store_left_and_the_promotion_right() {
        let rect = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(400.0, 220.0));
        let (store, tag) =
            deal_header_positions(rect, egui::vec2(110.0, 24.0), egui::vec2(60.0, 24.0));
        assert!((store.left() - rect.left() - 12.0).abs() < 0.01);
        assert!((store.top() - rect.top() - 12.0).abs() < 0.01);
        assert!((rect.right() - tag.right() - 12.0).abs() < 0.01);
        assert!((tag.top() - rect.top() - 12.0).abs() < 0.01);
        assert!(
            store.right() < tag.left(),
            "la plateforme et le tag ne se chevauchent pas"
        );
    }

    #[test]
    fn the_game_name_center_store_and_date_share_the_top_row() {
        let ctx = egui::Context::default();
        ctx.set_pixels_per_point(1.0);
        crate::apply_theme(&ctx);
        crate::install_fonts(&ctx);
        // Largeur réelle d'une carte quand la grille passe à cinq colonnes.
        let media = egui::Rect::from_min_size(
            egui::pos2(40.0, 30.0),
            egui::vec2(grid_card_width(1_864.0), 240.0),
        );
        let content = CardContent {
            label: "Epic Games",
            label_color: store_color(Some(Store::Epic)),
            store: Some(Store::Epic),
            meta_right: "05/12/2023",
            media_label: "Annulus",
            media_byline: None,
            title: "Maintenance Update",
            excerpt: "…",
            price: None,
            action: "Lire l'annonce",
        };
        let mut output = None;
        for _ in 0..2 {
            output = Some(ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(520.0, 320.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        paint_media_header(ui, media, &content, None);
                    });
                },
            ));
        }
        let output = output.expect("l'en-tête doit être peint");

        fn collect(
            shape: &egui::Shape,
            texts: &mut Vec<(String, egui::Rect)>,
            rects: &mut Vec<egui::Rect>,
        ) {
            match shape {
                egui::Shape::Text(text) => texts.push((
                    text.galley.text().to_string(),
                    egui::Rect::from_min_size(text.pos, text.galley.size()),
                )),
                egui::Shape::Rect(rect) => rects.push(rect.rect),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        collect(shape, texts, rects);
                    }
                }
                _ => {}
            }
        }
        let mut texts = Vec::new();
        let mut rects = Vec::new();
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut texts, &mut rects);
        }

        let icon = rects
            .iter()
            .copied()
            .find(|rect| {
                (rect.width() - 22.0).abs() < 0.01
                    && (rect.height() - 22.0).abs() < 0.01
                    && (rect.top() - (media.top() + 12.0)).abs() < 0.01
            })
            .expect("l'icône du jeu doit être peinte en haut du visuel");

        let mut title_rects: Vec<egui::Rect> = texts
            .iter()
            .filter(|(text, _)| text == "Annulus")
            .map(|(_, rect)| *rect)
            .collect();
        assert_eq!(title_rects.len(), 2, "le titre et son ombre");
        title_rects.sort_by(|left, right| left.top().total_cmp(&right.top()));
        let title = title_rects[0];
        assert!(
            (title.center().y - icon.center().y).abs() < 0.6,
            "le nom et l'icône ne partagent pas la même ligne"
        );
        let group_center = (icon.left() + title.right()) / 2.0;
        assert!(
            (group_center - media.center().x).abs() < 0.6,
            "le bloc jeu doit être centré : {group_center:.1} au lieu de {:.1}",
            media.center().x
        );

        let mut chips: Vec<egui::Rect> = rects
            .iter()
            .copied()
            .filter(|rect| (rect.height() - 24.0).abs() < 0.01 && rect.width() > 50.0)
            .collect();
        chips.sort_by(|left, right| left.left().total_cmp(&right.left()));
        chips.dedup();
        assert_eq!(chips.len(), 2, "boutique et date : {chips:?}");
        for chip in &chips {
            assert!(
                (chip.center().y - icon.center().y).abs() < 0.6,
                "la boutique, le jeu et la date doivent partager la même ligne : {chip:?}"
            );
        }
        assert!(
            icon.left() >= chips[0].right() + 0.5,
            "le nom du jeu chevauche la boutique"
        );
        assert!(
            title.right() <= chips[1].left() - 0.5,
            "le nom du jeu chevauche la date"
        );
    }

    #[test]
    fn very_wide_screens_get_a_fifth_news_column() {
        assert_eq!(columns(1_599.0), 4);
        assert_eq!(columns(1_600.0), 5);
        assert!(
            grid_card_width(1_864.0) < 400.0,
            "les cartes à cinq colonnes doivent rester compactes"
        );
    }
}

/// Captures hors écran de la vraie page Actualités.
///
/// Le banc ne dessine pas une maquette : il appelle `Page::page`, avec les
/// mêmes polices, les mêmes marges et les mêmes cartes que l'application. Les
/// images sont des textures déterministes chargées dans le même cache que les
/// visuels réseau ; aucun test ne dépend donc d'Internet.
#[cfg(test)]
mod visual_snapshots {
    use super::*;
    use eframe::egui::{self, Pos2, Rect, Vec2};
    use egui_wgpu::{wgpu, ScreenDescriptor};
    use std::future::Future;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::task::{Context as TaskContext, Poll, Wake, Waker};

    const LARGE_WIDTH: u32 = 1180;
    const WIDE_WIDTH: u32 = 1440;
    const NARROW_WIDTH: u32 = 760;

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum State {
        Ready,
        /// Analyse en cours : des résultats déjà là et une progression visible.
        Scanning,
        Loading,
        Error,
    }

    struct Case {
        name: &'static str,
        width: u32,
        height: u32,
        tab: Tab,
        state: State,
        /// Position de la souris pendant la capture : laisse les animations de
        /// survol se stabiliser avant le rendu.
        pointer: Option<Pos2>,
    }

    fn cases() -> Vec<Case> {
        vec![
            Case {
                name: "01-mises-a-jour-large.png",
                width: LARGE_WIDTH,
                height: 1160,
                tab: Tab::Updates,
                state: State::Ready,
                pointer: None,
            },
            Case {
                name: "02-soldes-large.png",
                width: LARGE_WIDTH,
                height: 1160,
                tab: Tab::Deals,
                state: State::Ready,
                pointer: None,
            },
            Case {
                name: "03-actus-large.png",
                width: LARGE_WIDTH,
                height: 1160,
                tab: Tab::Gaming,
                state: State::Ready,
                pointer: None,
            },
            Case {
                name: "04-mises-a-jour-etroit.png",
                width: NARROW_WIDTH,
                height: 1100,
                tab: Tab::Updates,
                state: State::Ready,
                pointer: None,
            },
            Case {
                name: "05-soldes-etroit.png",
                width: NARROW_WIDTH,
                height: 1100,
                tab: Tab::Deals,
                state: State::Ready,
                pointer: None,
            },
            Case {
                name: "06-actus-etroit.png",
                width: NARROW_WIDTH,
                height: 1100,
                tab: Tab::Gaming,
                state: State::Ready,
                pointer: None,
            },
            Case {
                name: "07-chargement-soldes.png",
                width: LARGE_WIDTH,
                height: 900,
                tab: Tab::Deals,
                state: State::Loading,
                pointer: None,
            },
            Case {
                name: "08-erreur-actus.png",
                width: LARGE_WIDTH,
                height: 780,
                tab: Tab::Gaming,
                state: State::Error,
                pointer: None,
            },
            Case {
                name: "09-vide-mises-a-jour.png",
                width: LARGE_WIDTH,
                height: 780,
                tab: Tab::Updates,
                state: State::Error,
                pointer: None,
            },
            Case {
                name: "10-mises-a-jour-1440.png",
                width: WIDE_WIDTH,
                height: 1160,
                tab: Tab::Updates,
                state: State::Ready,
                pointer: None,
            },
            Case {
                name: "11-survol-mises-a-jour.png",
                width: WIDE_WIDTH,
                height: 900,
                tab: Tab::Updates,
                state: State::Ready,
                pointer: Some(Pos2::new(540.0, 400.0)),
            },
            Case {
                name: "12-progression-mises-a-jour.png",
                width: LARGE_WIDTH,
                height: 900,
                tab: Tab::Updates,
                state: State::Scanning,
                pointer: None,
            },
            Case {
                name: "13-chargement-mises-a-jour.png",
                width: LARGE_WIDTH,
                height: 900,
                tab: Tab::Updates,
                state: State::Loading,
                pointer: None,
            },
        ]
    }

    fn scale_games(count: usize) -> Vec<GameSource> {
        (0..count)
            .map(|index| GameSource {
                appid: (index as u32).saturating_add(1),
                name: format!("Jeu de la bibliothèque {}", index + 1),
                store: match index % 4 {
                    1 => Some(Store::Gog),
                    2 => Some(Store::Epic),
                    3 => Some(Store::Itch),
                    _ => None,
                },
                icon: None,
            })
            .collect()
    }

    fn scale_feed(games: &[GameSource], article_count: usize) -> Feed {
        let titles = [
            "Mise à jour 1.3.2 — équilibrages et correctifs",
            "Hotfix 2.1.4 : sauvegardes et performances",
            "Notes de la mise à jour 4.0 : le mode coop arrive",
            "Correctif 1.0.7 — audio et manettes",
            "Patch 1.6 : refonte de l'interface",
            "Changelog 3.2.1",
            "Mise à jour de contenu : saison 2",
            "Correctif de lancement",
            "Version 5.0 : le nouveau cycle",
        ];
        let excerpts = [
            "Nouvelle passe sur les builds, les temps de recharge et quelques quêtes secondaires. Le détail complet est dans les notes.",
            "Correction d'un plantage rare lors des sauvegardes manuelles et gain de fluidité dans les zones les plus chargées.",
            "Jouez toute la campagne à deux, avec une progression partagée et de nouveaux défis dédiés.",
        ];
        let mut articles = Vec::with_capacity(article_count);
        for index in 0..article_count {
            let game = &games[index % games.len()];
            articles.push(Article {
                appid: game.appid,
                game: game.name.clone(),
                store: game.store,
                icon: None,
                news: SteamNews {
                    title: titles[index % titles.len()].to_string(),
                    excerpt: excerpts[index % excerpts.len()].to_string(),
                    body: String::new(),
                    date: 1_791_590_400 - index as i64 * 3_600,
                    author: String::new(),
                    url: format!("https://example.com/scale/{index}"),
                    image: String::new(),
                },
                arrived: Instant::now() - Duration::from_secs(2),
            });
        }
        Feed {
            articles,
            games: games.to_vec(),
            signature: list_signature(games),
            language: "fr".to_string(),
            queued: games.iter().map(|game| (game.appid, game.store)).collect(),
            analysed: games.len(),
            loading: false,
            ..Default::default()
        }
    }

    /// Temps CPU consommé par le fil courant. Contrairement au temps mural,
    /// il ne compte pas les interruptions de l'ordonnanceur quand la machine
    /// est chargée : le budget de frame reste reproductible sous charge.
    fn thread_cpu_time() -> Duration {
        let mut time = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: `clock_gettime` remplit une `timespec` valide et
        // `CLOCK_THREAD_CPUTIME_ID` est fourni par le noyau Linux.
        let result = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut time) };
        assert_eq!(result, 0, "horloge CPU du fil indisponible");
        Duration::new(time.tv_sec as u64, time.tv_nsec as u32)
    }

    /// Le vrai budget de la page, avec la taille de la dernière bibliothèque
    /// réelle : 2 322 jeux et 2 752 annonces. On mesure la mise en page et la
    /// tessellation de plusieurs positions de défilement, pas seulement la
    /// première image. Le temps CPU, et non le temps mural, garde le test
    /// déterministe sous charge tout en détectant une régression réelle.
    #[test]
    fn news_scroll_at_library_scale_stays_under_a_frame() {
        let ctx = visual_context();
        let games = scale_games(2_322);
        let mut page = Page {
            tab: Tab::Updates,
            updates: scale_feed(&games, 2_752),
            deals: deals::snapshot_deals_loading(),
            gaming: gaming::snapshot_gaming_loading(),
            ..Default::default()
        };
        let client = offline_client();
        let width = 1440;
        let height = 1000;
        let pointer = Some(Pos2::new(720.0, 620.0));
        let mut samples = Vec::new();
        for frame in 0..14 {
            let wheel = egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: Vec2::new(0.0, if frame == 0 { 0.0 } else { -640.0 }),
                modifiers: egui::Modifiers::NONE,
            };
            let events = if frame == 0 { Vec::new() } else { vec![wheel] };
            let started = thread_cpu_time();
            let output = page_frame_events(
                &ctx,
                &mut page,
                &games,
                &client,
                width,
                height,
                frame as f64 / 60.0,
                pointer,
                &events,
            );
            let _ = ctx.tessellate(output.shapes, output.pixels_per_point);
            let elapsed = thread_cpu_time().saturating_sub(started).as_secs_f64() * 1_000.0;
            if frame >= 2 {
                samples.push(elapsed);
            }
        }
        samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let p95 = samples[(samples.len() * 95 / 100).min(samples.len() - 1)];
        let median = samples[samples.len() / 2];
        eprintln!(
            "Actualités à l'échelle : 2 752 annonces, layout+tessellation p50={median:.2} ms, p95={p95:.2} ms, scroll={:.1} px",
            page.updates.last_scroll
        );
        assert!(
            page.updates.last_scroll > 0.0,
            "le banc doit réellement faire défiler la liste"
        );
        assert!(
            p95 < 16.7,
            "la page Actualités dépasse le budget d'une frame à 2 752 annonces : p95={p95:.2} ms"
        );

        // Le défilement d'egui est animé : on laisse d'abord la position se
        // stabiliser pour ne pas confondre la fin de l'animation avec un saut
        // provoqué par l'arrivée des cartes.
        for settle in 0..12 {
            let _ = page_frame(
                &ctx,
                &mut page,
                &games,
                &client,
                width,
                height,
                0.5 + settle as f64 / 60.0,
                pointer,
            );
        }
        let before = page.updates.last_scroll;

        // Une vague de résultats ne doit ni remonter la liste ni déplacer la
        // position courante : on ajoute des cartes derrière celle qui est lue.
        for index in 0..24 {
            let game = &games[index % games.len()];
            page.updates.articles.push(Article {
                appid: game.appid,
                game: game.name.clone(),
                store: game.store,
                icon: None,
                news: SteamNews {
                    title: format!("Mise à jour tardive {}", index + 1),
                    excerpt: "Une vague de résultats arrive pendant la lecture.".to_string(),
                    body: String::new(),
                    date: 1_791_590_400,
                    author: String::new(),
                    url: format!("https://example.com/late/{index}"),
                    image: String::new(),
                },
                arrived: Instant::now(),
            });
        }
        let _ = page_frame(
            &ctx, &mut page, &games, &client, width, height, 1.0, pointer,
        );
        assert!(
            (page.updates.last_scroll - before).abs() < 4.0,
            "l'arrivée de nouveaux lots a déplacé le défilement : {before:.1} -> {:.1}",
            page.updates.last_scroll
        );

        // Un aller-retour d'onglet garde la même position, sans relancer ni
        // vider la lecture en cours.
        page.tab = Tab::Deals;
        let _ = page_frame(
            &ctx, &mut page, &games, &client, width, height, 1.1, pointer,
        );
        page.tab = Tab::Updates;
        let _ = page_frame(
            &ctx, &mut page, &games, &client, width, height, 1.2, pointer,
        );
        assert!(
            (page.updates.last_scroll - before).abs() < 4.0,
            "le retour sur l'onglet Actualités a perdu la position de défilement : {before:.1} -> {:.1}",
            page.updates.last_scroll
        );
    }

    #[test]
    fn the_news_page_uses_the_whole_wide_window() {
        let ctx = visual_context();
        let games = scale_games(8);
        let mut page = Page {
            tab: Tab::Updates,
            updates: scale_feed(&games, 8),
            deals: deals::snapshot_deals_loading(),
            gaming: gaming::snapshot_gaming_loading(),
            ..Default::default()
        };
        let client = offline_client();
        let width = 1920;
        let output = page_frame(&ctx, &mut page, &games, &client, width, 1000, 0.0, None);

        fn collect_cards(shape: &egui::Shape, cards: &mut Vec<egui::Rect>) {
            match shape {
                egui::Shape::Rect(rect) if rect.fill == CARD() && rect.rect.height() > 180.0 => {
                    cards.push(rect.rect)
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        collect_cards(shape, cards);
                    }
                }
                _ => {}
            }
        }
        let mut cards = Vec::new();
        for clipped in &output.shapes {
            collect_cards(&clipped.shape, &mut cards);
        }
        assert!(
            cards.len() >= 5,
            "la première rangée doit contenir cinq cartes"
        );
        let widest = cards
            .iter()
            .map(|rect| rect.width())
            .fold(0.0_f32, f32::max);
        assert!(
            widest < 400.0,
            "les cartes restent trop larges : {widest:.1} px"
        );
        let tallest = cards
            .iter()
            .map(|rect| rect.height())
            .fold(0.0_f32, f32::max);
        assert!(
            tallest < 260.0,
            "les annonces gardent trop de hauteur vide : {tallest:.1} px"
        );
        let left = cards
            .iter()
            .map(|rect| rect.left())
            .fold(f32::INFINITY, f32::min);
        let right = cards
            .iter()
            .map(|rect| rect.right())
            .fold(f32::NEG_INFINITY, f32::max);
        assert!(
            (left - 22.0).abs() < 1.0,
            "marge gauche inattendue : {left:.1}"
        );
        assert!(
            right > width as f32 - 50.0,
            "la page garde une marge droite de {:.0} px",
            width as f32 - right
        );
    }

    /// Vérifie la géométrie réelle de l'en-tête, sans GPU ni capture : le
    /// titre est en haut à gauche, la barre reste centrée et Actualiser reste
    /// à droite sans influer sur ce centre. Les cartes ne répètent plus
    /// « Lire l'annonce ».
    #[test]
    fn the_news_header_puts_the_section_title_top_left_and_centers_tabs() {
        let ctx = visual_context();
        let games = scale_games(8);
        let mut page = Page {
            tab: Tab::Updates,
            updates: scale_feed(&games, 8),
            deals: deals::snapshot_deals_loading(),
            gaming: gaming::snapshot_gaming_loading(),
            ..Default::default()
        };
        let client = offline_client();
        let output = page_frame(&ctx, &mut page, &games, &client, 1180, 900, 0.0, None);

        let content = page
            .geometry
            .content
            .expect("la page doit publier la zone de contenu");
        let tabs = page
            .geometry
            .tab_bar
            .expect("la page doit publier le rectangle de la barre");
        let refresh = page
            .geometry
            .refresh
            .expect("la page doit publier le rectangle d'Actualiser");

        let left = tabs.left() - content.left();
        let right = content.right() - tabs.right();
        assert!(
            (left - right).abs() <= 1.0,
            "barre non centrée : marge gauche {left:.2} px, marge droite {right:.2} px"
        );
        assert!(
            (tabs.top() - content.top()).abs() <= 3.5,
            "la barre doit rester au haut du contenu : barre {:.2} px, contenu {:.2} px",
            tabs.top(),
            content.top()
        );
        assert!(
            (refresh.right() - content.right()).abs() <= 1.0,
            "Actualiser doit rester à droite : bouton {:.2} px, contenu {:.2} px",
            refresh.right(),
            content.right()
        );
        assert!(
            refresh.left() > tabs.right(),
            "Actualiser ne doit pas chevaucher la barre de sélection"
        );

        fn collect_text(shape: &egui::Shape, texts: &mut Vec<(String, egui::Rect)>) {
            match shape {
                egui::Shape::Text(text) => texts.push((
                    text.galley.text().to_string(),
                    egui::Rect::from_min_size(text.pos, text.galley.size()),
                )),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        collect_text(shape, texts);
                    }
                }
                _ => {}
            }
        }
        let mut texts = Vec::new();
        for clipped in &output.shapes {
            collect_text(&clipped.shape, &mut texts);
        }
        assert!(
            texts.iter().any(|(text, _)| text == lapinux_core::i18n::t("news.tab.updates")),
            "le test doit voir le libellé de l'onglet pour ne pas être vide : {texts:?}"
        );
        let heading = texts
            .iter()
            .find(|(text, _)| text == lapinux_core::i18n::t("news.updates.all_games"))
            .expect("le sélecteur de jeux doit être dessiné");
        assert!(
            heading.1.left() <= content.left() + 10.0,
            "le sélecteur doit rester dans le coin gauche : sélecteur {:.2} px, contenu {:.2} px",
            heading.1.left(),
            content.left()
        );
        assert!(
            heading.1.center().y <= content.top() + 42.0,
            "le titre doit rester dans la ligne d'en-tête : centre {:.2} px, contenu {:.2} px",
            heading.1.center().y,
            content.top()
        );

        let card_title = texts
            .iter()
            .find(|(text, _)| text.starts_with("Mise à jour 1.3.2"))
            .unwrap_or_else(|| {
                panic!(
                    "un titre d'annonce doit être dessiné ; articles={} visibles={} textes={texts:?}",
                    page.updates.articles.len(),
                    page.updates.visible_articles().len()
                )
            });
        let card_excerpt = texts
            .iter()
            .find(|(text, _)| text.starts_with("Nouvelle passe sur les builds"))
            .expect("un aperçu d'annonce doit être dessiné");
        let gap = card_excerpt.1.top() - card_title.1.bottom();
        assert!(gap <= 12.0, "l'aperçu est trop loin du titre : {gap:.2} px");

        fn collect_cards(shape: &egui::Shape, cards: &mut Vec<egui::Rect>) {
            match shape {
                egui::Shape::Rect(rect) if rect.fill == CARD() && rect.rect.height() > 180.0 => {
                    cards.push(rect.rect)
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        collect_cards(shape, cards);
                    }
                }
                _ => {}
            }
        }
        let mut cards = Vec::new();
        for clipped in &output.shapes {
            collect_cards(&clipped.shape, &mut cards);
        }
        let card = cards
            .iter()
            .find(|card| card.contains(card_title.1.center()))
            .expect("la carte du titre doit être trouvée");
        let image_bottom = card.top() + grid_image_height_compact(card.width());
        let image_gap = card_title.1.top() - image_bottom;
        assert!(
            image_gap <= 20.0,
            "le titre est trop loin de l'image : {image_gap:.2} px"
        );
        let bottom_gap = card.bottom() - card_excerpt.1.bottom();
        assert!(
            bottom_gap <= 24.0,
            "le bas de carte est trop loin du texte : {bottom_gap:.2} px"
        );
        assert!(
            !texts.iter().any(|(text, _)| text == "Lire l'annonce"),
            "l'action redondante est encore dessinée : {texts:?}"
        );
        assert!(
            !texts.iter().any(|(text, _)| text == "Actualités"),
            "l'ancien titre de page est encore dessiné : {texts:?}"
        );
        assert!(
            !texts
                .iter()
                .any(|(text, _)| text == "Mises à jour, soldes et actus du jeu vidéo"),
            "l'ancien sous-titre de page est encore dessiné : {texts:?}"
        );
    }

    #[test]
    fn header_controls_share_the_same_height_on_every_tab() {
        let ctx = visual_context();
        let games = scale_games(8);
        let client = offline_client();
        for tab in [Tab::Updates, Tab::Deals, Tab::Gaming, Tab::Calendar] {
            let mut page = sample_page(&ctx, tab, State::Ready, &games);
            let _ = page_frame(&ctx, &mut page, &games, &client, 1180, 900, 0.0, None);
            let source = page
                .geometry
                .source
                .expect("la page doit publier le rectangle du sélecteur");
            let tabs = page
                .geometry
                .tab_bar
                .expect("la page doit publier le rectangle de la barre");
            let buttons = page
                .geometry
                .tab_buttons
                .expect("la page doit publier les quatre segments");
            let widest = buttons
                .iter()
                .map(|button| button.width())
                .fold(0.0_f32, f32::max);
            let narrowest = buttons
                .iter()
                .map(|button| button.width())
                .fold(f32::INFINITY, f32::min);
            assert!(
                (widest - narrowest).abs() <= 0.5,
                "les quatre sélections doivent avoir la même largeur : {widest:.2} / {narrowest:.2}"
            );
            for button in buttons {
                assert!(
                    (button.height() - NEWS_CONTROL_HEIGHT).abs() <= 0.5,
                    "un bouton d'onglet doit faire {NEWS_CONTROL_HEIGHT} px de haut : {:.2} px",
                    button.height()
                );
                assert!(
                    (button.center().y - source.center().y).abs() <= 0.5,
                    "un bouton d'onglet doit être aligné avec le sélecteur : {:.2} px contre {:.2} px",
                    button.center().y,
                    source.center().y
                );
            }
            let refresh = page
                .geometry
                .refresh
                .expect("la page doit publier le rectangle d'Actualiser");
            for (name, rect) in [
                ("sélecteur", source),
                ("onglets", tabs),
                ("actualiser", refresh),
            ] {
                assert!(
                    (rect.height() - NEWS_CONTROL_HEIGHT).abs() <= 0.5,
                    "{name} doit faire {NEWS_CONTROL_HEIGHT} px de haut : {:.2} px",
                    rect.height()
                );
                assert!(
                    (rect.center().y - source.center().y).abs() <= 0.5,
                    "{name} doit être aligné au centre du sélecteur : {:.2} px contre {:.2} px",
                    rect.center().y,
                    source.center().y
                );
            }
        }
    }

    #[test]
    fn gaming_byline_sits_in_the_top_corners_of_the_image() {
        let ctx = visual_context();
        let image = egui::Rect::from_min_size(egui::pos2(40.0, 30.0), egui::vec2(420.0, 220.0));
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(520.0, 300.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    paint_media_byline(
                        ui,
                        image,
                        MediaByline {
                            label: "PC Gamer",
                            color: ACCENT(),
                            icon: None,
                            right: "10/11/2026",
                        },
                    );
                });
            },
        );

        fn collect(shape: &egui::Shape, texts: &mut Vec<(String, egui::Rect)>) {
            match shape {
                egui::Shape::Text(text) => texts.push((
                    text.galley.text().to_string(),
                    egui::Rect::from_min_size(text.pos, text.galley.size()),
                )),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        collect(shape, texts);
                    }
                }
                _ => {}
            }
        }
        let mut texts = Vec::new();
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut texts);
        }
        let source = texts
            .iter()
            .find(|(text, _)| text == "PC Gamer")
            .expect("le site doit être peint sur le visuel");
        let date = texts
            .iter()
            .find(|(text, _)| text == "10/11/2026")
            .expect("la date doit être peinte sur le visuel");
        assert!(
            source.1.left() >= image.left() + 12.0 && source.1.top() <= image.top() + 45.0,
            "le site doit rester en haut à gauche : {:?} dans {:?}",
            source.1,
            image
        );
        assert!(
            date.1.right() <= image.right() - 11.0
                && date.1.right() >= image.right() - 100.0
                && date.1.top() <= image.top() + 45.0,
            "la date doit rester en haut à droite : {:?} dans {:?}",
            date.1,
            image
        );
        assert!(
            (source.1.center().y - date.1.center().y).abs() <= 1.0,
            "le site et la date ne partagent pas la même ligne : {:?} / {:?}",
            source.1,
            date.1
        );
    }

    #[test]
    fn update_filter_hides_a_game_and_its_platform() {
        let ctx = visual_context();
        let games = scale_games(8);
        let feed = sample_updates(&ctx, &games, State::Ready);
        let total = feed.visible_articles().len();
        assert!(total > 1);
        let first = feed.articles.first().expect("des annonces sont attendues");
        let key = (first.appid, Feed::update_store(first.store));
        let store = key.1;
        let game_count = feed
            .articles
            .iter()
            .filter(|article| (article.appid, Feed::update_store(article.store)) == key)
            .count();
        let mut feed = feed;
        feed.hidden_games.insert(key);
        assert_eq!(feed.visible_articles().len(), total - game_count);
        feed.hidden_games.clear();
        feed.hidden_stores.insert(store);
        let visible = feed.visible_articles();
        let store_count = feed
            .articles
            .iter()
            .filter(|article| Feed::update_store(article.store) == store)
            .count();
        assert_eq!(visible.len(), total - store_count);
        assert!(visible
            .iter()
            .all(|&index| Feed::update_store(feed.articles[index].store) != store));
    }

    #[test]
    fn source_menus_replace_the_section_titles() {
        let ctx = visual_context();
        let games = scale_games(8);
        let client = offline_client();
        for (tab, expected) in [
            (Tab::Updates, lapinux_core::i18n::t("news.updates.all_games")),
            (Tab::Deals, lapinux_core::i18n::t("news.deals.all_stores")),
            (Tab::Gaming, lapinux_core::i18n::t("news.gaming.all")),
        ] {
            let mut page = sample_page(&ctx, tab, State::Ready, &games);
            let output = page_frame(&ctx, &mut page, &games, &client, 1180, 900, 0.0, None);
            let mut texts = Vec::new();
            fn collect_text(shape: &egui::Shape, texts: &mut Vec<String>) {
                match shape {
                    egui::Shape::Text(text) => texts.push(text.galley.text().to_string()),
                    egui::Shape::Vec(shapes) => {
                        for shape in shapes {
                            collect_text(shape, texts);
                        }
                    }
                    _ => {}
                }
            }
            for clipped in &output.shapes {
                collect_text(&clipped.shape, &mut texts);
            }
            assert!(
                texts.iter().any(|text| text == expected),
                "le sélecteur de source doit être dessiné : {texts:?}"
            );
            assert!(
                !texts.iter().any(|text| {
                    text == "Notes de mise à jour"
                        || text == "Promotions en cours"
                        || text == "À la une"
                }),
                "l'ancien titre de section est encore dessiné : {texts:?}"
            );
        }
    }

    #[test]
    fn personal_calendar_button_sits_left_of_refresh() {
        let ctx = visual_context();
        let games = scale_games(8);
        let client = offline_client();

        fn collect_text(shape: &egui::Shape, texts: &mut Vec<(String, egui::Rect)>) {
            match shape {
                egui::Shape::Text(text) => texts.push((
                    text.galley.text().to_string(),
                    egui::Rect::from_min_size(text.pos, text.galley.size()),
                )),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        collect_text(shape, texts);
                    }
                }
                _ => {}
            }
        }

        for width in [980, 1180] {
            let mut page = sample_page(&ctx, Tab::Calendar, State::Ready, &games);
            let output = page_frame(&ctx, &mut page, &games, &client, width, 900, 0.0, None);
            let tabs = page
                .geometry
                .tab_bar
                .expect("la page doit publier la barre des onglets");
            let mut texts = Vec::new();
            for clipped in &output.shapes {
                collect_text(&clipped.shape, &mut texts);
            }
            let personal = texts
                .iter()
                .find(|(text, _)| text.starts_with(lapinux_core::i18n::t("news.calendar.personal_steam")))
                .expect("le bouton du calendrier personnel doit être peint");
            let refresh = texts
                .iter()
                .find(|(text, _)| text.contains(lapinux_core::i18n::t("news.refresh")))
                .expect("le bouton Actualiser doit être peint");

            assert!(
                personal.1.right() <= refresh.1.left(),
                "le bouton du calendrier doit rester à gauche d'Actualiser : {:?} / {:?}",
                personal.1,
                refresh.1
            );
            assert!(
                !personal.1.intersects(tabs),
                "le bouton du calendrier chevauche les onglets à {width} px : {:?} / {:?}",
                personal.1,
                tabs
            );
            assert!(
                (personal.1.center().y - refresh.1.center().y).abs() <= 1.0,
                "le bouton du calendrier n'est pas aligné avec Actualiser : {:?} / {:?}",
                personal.1,
                refresh.1
            );
        }
    }

    #[test]
    fn news_visual_states() {
        let snapshots = snapshot_dir();
        std::fs::create_dir_all(&snapshots)
            .expect("le dossier des captures Actualités doit être créable");
        let (_gpu, _instance, device, queue) = gpu();
        let mut renderer =
            egui_wgpu::Renderer::new(&device, wgpu::TextureFormat::Rgba8Unorm, None, 1, false);

        for case in cases() {
            let rgba = render_case(&device, &queue, &mut renderer, &case);
            check_snapshot(case.name, case.width, case.height, &rgba, &snapshots);
        }
    }

    fn sample_games() -> Vec<GameSource> {
        vec![
            GameSource {
                appid: 1_091_500,
                name: "Cyberpunk 2077".into(),
                store: None,
                icon: None,
            },
            GameSource {
                appid: 1_084_940,
                name: "Baldur's Gate 3".into(),
                store: Some(Store::Gog),
                icon: None,
            },
            GameSource {
                appid: 1_146_540,
                name: "Hades II".into(),
                store: None,
                icon: None,
            },
            GameSource {
                appid: 1_541_540,
                name: "Alan Wake 2".into(),
                store: Some(Store::Epic),
                icon: None,
            },
            GameSource {
                appid: 632_470,
                name: "Disco Elysium".into(),
                store: Some(Store::Gog),
                icon: None,
            },
            GameSource {
                appid: 1_247_720,
                name: "Sea of Stars".into(),
                store: None,
                icon: None,
            },
            GameSource {
                appid: 870_780,
                name: "Control Ultimate Edition".into(),
                store: Some(Store::Epic),
                icon: None,
            },
            GameSource {
                appid: 553_420,
                name: "Tunic".into(),
                store: Some(Store::Gog),
                icon: None,
            },
        ]
    }

    fn sample_updates(ctx: &egui::Context, games: &[GameSource], state: State) -> Feed {
        const SAMPLES: [(&str, &str); 9] = [
            (
                "Mise à jour 1.3.2 — équilibrages et correctifs",
                "Nouvelle passe sur les builds, les temps de recharge et quelques quêtes secondaires. Le détail complet est dans les notes.",
            ),
            (
                "Hotfix 2.1.4 : sauvegardes et performances",
                "Correction d'un plantage rare lors des sauvegardes manuelles et gain de fluidité dans les zones les plus chargées.",
            ),
            (
                "Notes de la mise à jour 4.0 : le mode coop arrive",
                "Jouez toute la campagne à deux, avec une progression partagée et de nouveaux défis dédiés.",
            ),
            (
                "Correctif 1.0.7 — audio et manettes",
                "Le mixage des dialogues est rééquilibré et la détection des manettes Bluetooth est plus fiable.",
            ),
            (
                "Patch 1.6 : refonte de l'interface",
                "Menus plus lisibles, navigation repensée et raccourcis clavier configurables.",
            ),
            (
                "Changelog 3.2.1",
                "Les correctifs de stabilité de la semaine, avec une nouvelle passe sur les succès.",
            ),
            (
                "Mise à jour de contenu : saison 2",
                "Une nouvelle région, deux donjons et un événement de fin de saison arrivent cette semaine.",
            ),
            (
                "Correctif de lancement",
                "Réglages de derniers instants pour les écrans ultra-larges et les cartes graphiques récentes.",
            ),
            (
                "Version 5.0 : le nouveau cycle",
                "Une refonte des récompenses et un nouveau mode de difficulté pour les joueurs vétérans.",
            ),
        ];
        const COLORS: [(egui::Color32, egui::Color32); 9] = [
            (
                egui::Color32::from_rgb(34, 40, 78),
                egui::Color32::from_rgb(135, 62, 98),
            ),
            (
                egui::Color32::from_rgb(28, 68, 76),
                egui::Color32::from_rgb(72, 144, 112),
            ),
            (
                egui::Color32::from_rgb(86, 46, 32),
                egui::Color32::from_rgb(202, 128, 56),
            ),
            (
                egui::Color32::from_rgb(50, 38, 84),
                egui::Color32::from_rgb(146, 84, 184),
            ),
            (
                egui::Color32::from_rgb(32, 58, 88),
                egui::Color32::from_rgb(76, 146, 190),
            ),
            (
                egui::Color32::from_rgb(72, 42, 32),
                egui::Color32::from_rgb(178, 88, 64),
            ),
            (
                egui::Color32::from_rgb(44, 56, 34),
                egui::Color32::from_rgb(132, 172, 76),
            ),
            (
                egui::Color32::from_rgb(62, 38, 56),
                egui::Color32::from_rgb(190, 88, 136),
            ),
            (
                egui::Color32::from_rgb(36, 46, 66),
                egui::Color32::from_rgb(88, 122, 168),
            ),
        ];

        let mut articles = Vec::new();
        let mut art = NewsArt::default();
        if matches!(state, State::Ready | State::Scanning) && !games.is_empty() {
            for (index, (title, excerpt)) in SAMPLES.iter().enumerate() {
                let game = &games[index % games.len()];
                let image = if index == 6 {
                    String::new()
                } else {
                    format!("snapshot://update-{index}")
                };
                if !image.is_empty() {
                    let (top, bottom) = COLORS[index];
                    art.seed_cover(ctx, &image, top, bottom);
                }
                articles.push(Article {
                    appid: game.appid,
                    game: game.name.clone(),
                    store: game.store,
                    icon: None,
                    news: SteamNews {
                        title: (*title).to_string(),
                        excerpt: (*excerpt).to_string(),
                        body: String::new(),
                        date: 1_791_590_400 - index as i64 * 3_600,
                        author: "Studio".to_string(),
                        url: format!("https://example.com/update/{index}"),
                        image,
                    },
                    arrived: Instant::now() - Duration::from_secs(1),
                });
            }
        }
        Feed {
            articles,
            art,
            games: games.to_vec(),
            signature: list_signature(games),
            language: "fr".to_string(),
            queued: games.iter().map(|game| (game.appid, game.store)).collect(),
            analysed: match state {
                State::Ready | State::Error => games.len(),
                State::Scanning => games.len().min(3),
                State::Loading => 0,
            },
            loading: matches!(state, State::Loading | State::Scanning),
            ..Default::default()
        }
    }

    fn sample_page(ctx: &egui::Context, tab: Tab, state: State, games: &[GameSource]) -> Page {
        let updates = if tab == Tab::Updates {
            sample_updates(ctx, games, state)
        } else {
            Feed::default()
        };
        let deals = if tab == Tab::Deals {
            match state {
                State::Ready | State::Scanning => deals::snapshot_deals(ctx),
                State::Loading => deals::snapshot_deals_loading(),
                State::Error => deals::snapshot_deals_error(),
            }
        } else {
            deals::snapshot_deals_loading()
        };
        let gaming = if tab == Tab::Gaming {
            match state {
                State::Ready | State::Scanning => gaming::snapshot_gaming(ctx),
                State::Loading => gaming::snapshot_gaming_loading(),
                State::Error => gaming::snapshot_gaming_error(),
            }
        } else {
            gaming::snapshot_gaming_loading()
        };
        let calendar = if tab == Tab::Calendar {
            calendar::snapshot_calendar()
        } else {
            calendar::Calendar::default()
        };
        Page {
            tab,
            tab_started: None,
            updates,
            deals,
            gaming,
            calendar,
            ..Default::default()
        }
    }

    fn visual_context() -> egui::Context {
        let ctx = egui::Context::default();
        ctx.set_pixels_per_point(1.0);
        crate::apply_theme(&ctx);
        crate::install_fonts(&ctx);
        ctx
    }

    fn page_frame(
        ctx: &egui::Context,
        page: &mut Page,
        games: &[GameSource],
        client: &Client,
        width: u32,
        height: u32,
        time: f64,
        pointer: Option<Pos2>,
    ) -> egui::FullOutput {
        page_frame_events(ctx, page, games, client, width, height, time, pointer, &[])
    }

    fn page_frame_events(
        ctx: &egui::Context,
        page: &mut Page,
        games: &[GameSource],
        client: &Client,
        width: u32,
        height: u32,
        time: f64,
        pointer: Option<Pos2>,
        events: &[egui::Event],
    ) -> egui::FullOutput {
        let mut input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                Pos2::ZERO,
                Vec2::new(width as f32, height as f32),
            )),
            focused: true,
            ..Default::default()
        };
        input.time = Some(time);
        if let Some(pointer) = pointer {
            input.events.push(egui::Event::PointerMoved(pointer));
        }
        input.events.extend_from_slice(events);
        ctx.run(input, |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::none().fill(BACKGROUND()))
                .show(ctx, |ui| {
                    egui::Frame::none()
                        .inner_margin(egui::Margin::symmetric(22.0, 18.0))
                        .show(ui, |ui| {
                            page.page(ui, games, "fr", client);
                        });
                });
        })
    }

    fn render_case(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        renderer: &mut egui_wgpu::Renderer,
        case: &Case,
    ) -> Vec<u8> {
        let ctx = visual_context();
        let games = sample_games();
        let mut page = sample_page(&ctx, case.tab, case.state, &games);
        let client = offline_client();

        let mut shapes = None;
        let mut pixels_per_point = 1.0;
        let frames = if case.pointer.is_some() { 15 } else { 3 };
        for frame in 0..frames {
            let output = page_frame(
                &ctx,
                &mut page,
                &games,
                &client,
                case.width,
                case.height,
                frame as f64 / 60.0,
                case.pointer,
            );
            for (id, delta) in &output.textures_delta.set {
                renderer.update_texture(device, queue, *id, delta);
            }
            for id in &output.textures_delta.free {
                renderer.free_texture(id);
            }
            if frame + 1 == frames {
                pixels_per_point = output.pixels_per_point;
                shapes = Some(output.shapes);
            }
        }

        let paint_jobs = ctx.tessellate(
            shapes.expect("la dernière frame Actualités doit produire des formes"),
            pixels_per_point,
        );
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("news-snapshot"),
            size: wgpu::Extent3d {
                width: case.width,
                height: case.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let screen = ScreenDescriptor {
            size_in_pixels: [case.width, case.height],
            pixels_per_point,
        };
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("news-snapshot"),
        });
        renderer.update_buffers(device, queue, &mut encoder, &paint_jobs, &screen);
        {
            let background = BACKGROUND();
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("news-snapshot"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: background.r() as f64 / 255.0,
                            g: background.g() as f64 / 255.0,
                            b: background.b() as f64 / 255.0,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            renderer.render(&mut pass.forget_lifetime(), &paint_jobs, &screen);
        }

        let bytes_per_row = (case.width * 4).div_ceil(256) * 256;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("news-snapshot-readback"),
            size: (bytes_per_row * case.height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            wgpu::ImageCopyTexture {
                texture: &target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::ImageCopyBuffer {
                buffer: &readback,
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(case.height),
                },
            },
            wgpu::Extent3d {
                width: case.width,
                height: case.height,
                depth_or_array_layers: 1,
            },
        );
        queue.submit(Some(encoder.finish()));

        let slice = readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        let _ = device.poll(wgpu::Maintain::Wait);
        rx.recv()
            .expect("le readback wgpu doit répondre")
            .expect("le readback wgpu doit réussir");

        let mapped = slice.get_mapped_range();
        let mut rgba = vec![0u8; (case.width * case.height * 4) as usize];
        for row in 0..case.height {
            let source = &mapped
                [(row * bytes_per_row) as usize..(row * bytes_per_row + case.width * 4) as usize];
            let destination =
                &mut rgba[(row * case.width * 4) as usize..((row + 1) * case.width * 4) as usize];
            destination.copy_from_slice(source);
        }
        drop(mapped);
        readback.unmap();
        rgba
    }

    fn offline_client() -> Client {
        Client {
            socket: std::env::temp_dir().join("lapinux-news-snapshot.sock"),
            auto_spawn: false,
            log_file: std::env::temp_dir().join("lapinux-news-snapshot.log"),
        }
    }

    fn snapshot_dir() -> PathBuf {
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots/news");
        if lapinux_core::i18n::french() {
            base
        } else {
            base.join(lapinux_core::i18n::active())
        }
    }

    fn check_snapshot(name: &str, width: u32, height: u32, rgba: &[u8], dir: &Path) {
        let path = dir.join(name);
        if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
            save_png(&path, width, height, rgba);
            return;
        }
        let expected = match image::open(&path) {
            Ok(image) => image.to_rgba8(),
            Err(error) => {
                let new_path = path.with_extension("new.png");
                save_png(&new_path, width, height, rgba);
                panic!(
                    "capture absente : {}\nrendu écrit dans {} ({error})",
                    path.display(),
                    new_path.display()
                );
            }
        };
        assert_eq!(
            expected.dimensions(),
            (width, height),
            "taille de capture inattendue pour {}",
            path.display()
        );
        let expected = expected.as_raw();
        let mut differing = 0usize;
        let mut max_delta = 0u8;
        for (left, right) in expected.chunks_exact(4).zip(rgba.chunks_exact(4)) {
            let delta = left
                .iter()
                .zip(right)
                .map(|(a, b)| a.abs_diff(*b))
                .max()
                .unwrap_or(0);
            max_delta = max_delta.max(delta);
            if delta > 3 {
                differing += 1;
            }
        }
        let ratio = differing as f64 / (width * height) as f64;
        if ratio > 0.002 {
            let new_path = path.with_extension("new.png");
            save_png(&new_path, width, height, rgba);
            panic!(
                "capture différente pour {} : {:.3} % de pixels, écart max {}\nrendu écrit dans {}",
                path.display(),
                ratio * 100.0,
                max_delta,
                new_path.display()
            );
        }
    }

    fn save_png(path: &Path, width: u32, height: u32, rgba: &[u8]) {
        image::save_buffer(path, rgba, width, height, image::ExtendedColorType::Rgba8)
            .unwrap_or_else(|error| panic!("écriture de {} impossible : {error}", path.display()));
    }

    fn gpu() -> (
        crate::snapshot_gpu::Guard,
        wgpu::Instance,
        wgpu::Device,
        wgpu::Queue,
    ) {
        // Le chargeur Vulkan n'est pas réentrant : une capture à la fois.
        let guard = crate::snapshot_gpu::serialized();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });
        let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .expect("aucun adaptateur GPU pour les captures Actualités");
        let (device, queue) = block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("news-snapshot"),
                ..Default::default()
            },
            None,
        ))
        .expect("création du device GPU impossible");
        (guard, instance, device, queue)
    }

    fn block_on<F: Future>(future: F) -> F::Output {
        struct ThreadWake(std::thread::Thread);
        impl Wake for ThreadWake {
            fn wake(self: Arc<Self>) {
                self.0.unpark();
            }
            fn wake_by_ref(self: &Arc<Self>) {
                self.0.unpark();
            }
        }

        let mut future = Box::pin(future);
        let waker = Waker::from(Arc::new(ThreadWake(std::thread::current())));
        let mut context = TaskContext::from_waker(&waker);
        loop {
            match future.as_mut().poll(&mut context) {
                Poll::Ready(output) => return output,
                Poll::Pending => std::thread::park(),
            }
        }
    }
}
