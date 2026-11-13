//! Section **Boutiques › Jeux gratuits** : les offres à conserver chez Steam,
//! Epic Games, GOG et itch.io, leur échéance, et de quoi les rattacher au
//! compte en un clic. Les free-to-play et les free weekends sont écartés à la
//! source.
//!
//! La liste vient du daemon, qui lit les flux publics des boutiques : elle
//! s'affiche même sans compte connecté. Seule la récupération demande une
//! session Steam ou Epic — les offres sans API de récupération s'ouvrent dans
//! leur carte, une par une, et seulement quand on la clique.

use crate::gameinfo::NewsArt;
use crate::{action_button, ActionTone};
use crate::{ACCENT, BORDER, CARD, CARD_ACTIVE, DANGER, OK, PANEL, TEXT, TEXT_DIM, WARN};
use eframe::egui;
use lapinux_core::client::Client;
use lapinux_core::ipc::Req;
use lapinux_core::paths::LapinuxPaths;
use lapinux_core::stores::{ClaimOutcome, FreeGame, Store};
use std::collections::HashSet;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// Réponse du daemon : la liste, ou la raison du refus.
type Reply = Result<Vec<FreeGame>, String>;
type ClaimReply = Result<Vec<ClaimOutcome>, String>;

/// Le script qui remplit le panier sans fenêtre, embarqué dans le binaire.
const CART_HELPER: &str = include_str!("cart_helper.py");

/// Ce qu'a donné le remplissage du panier, tel que le script le rend.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct CartReport {
    #[serde(default)]
    added: u64,
    #[serde(default)]
    errors: Vec<String>,
    #[serde(default)]
    not_logged_in: bool,
}
type CartReply = Result<CartReport, String>;

#[derive(Default)]
pub(crate) struct FreeGames {
    games: Vec<FreeGame>,
    error: Option<String>,
    /// Une lecture est en cours : la liste garde ses entrées précédentes, le
    /// panneau affiche seulement l'attente.
    loading: bool,
    asked: bool,
    rx: Option<mpsc::Receiver<Reply>>,
    claim: Option<mpsc::Receiver<ClaimReply>>,
    /// Verdict de la dernière récupération, par offre.
    outcomes: Vec<ClaimOutcome>,
    /// Phrase affichée après une récupération.
    message: Option<String>,
    /// Le panier : les offres récupérables, cochées avant de partir.
    cart_open: bool,
    selected: HashSet<String>,
    /// Offres à pousser dans le panier du site Epic, dans l'ordre : l'interface
    /// remplit le panier en tâche de fond, sans ouvrir de fenêtre.
    cart_request: Option<Vec<String>>,
    /// Remplissage en cours : le script rend son verdict.
    cart_fill: Option<mpsc::Receiver<CartReply>>,
    /// Verdict du dernier remplissage, montré dans son propre modal.
    cart_done: Option<String>,
    /// L'adresse du panier ou du checkout Epic à ouvrir, quand elle est prête.
    cart_page: Option<String>,
    cover: NewsArt,
}

impl FreeGames {
    /// Nombre d'offres en cours : la liste de gauche le rappelle sous le nom
    /// de la section, sans avoir à l'ouvrir.
    pub(crate) fn current_count(&self) -> usize {
        self.games.iter().filter(|game| !game.upcoming).count()
    }

    /// Première lecture : elle part à l'ouverture de la page, sans clic.
    fn ensure(&mut self, client: &Client) {
        if self.asked {
            return;
        }
        self.asked = true;
        self.load(client);
    }

    fn load(&mut self, client: &Client) {
        if self.rx.is_some() || self.claim.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        self.loading = true;
        self.error = None;
        let worker = client.detached();
        std::thread::spawn(move || {
            let reply = worker
                .request_with_timeout(&Req::StoreFreeGames, Duration::from_secs(180))
                .map_err(|error| error.to_string())
                .and_then(|envelope| {
                    serde_json::from_value::<Vec<FreeGame>>(envelope.data)
                        .map_err(|error| error.to_string())
                });
            let _ = tx.send(reply);
        });
    }

    /// Ouvre le checkout gratuit Epic avec toutes les offres encore
    /// récupérables, PC, Android et iOS. Les offres déjà possédées sont
    /// écartées avant de construire l'adresse.
    fn epic_cart(&mut self) {
        let Some(url) = self.epic_purchase_url() else {
            self.message = Some(lapinux_core::i18n::t("freegames.all_in_library").to_string());
            return;
        };
        self.cart_open = false;
        self.cart_page = Some(url);
        self.message = None;
    }

    /// Offres Epic à mettre dans le checkout, sans doublon et sans celles déjà
    /// possédées : une carte mobile peut porter Android et iOS, mais seule
    /// l'offre encore libre doit entrer dans le panier.
    fn epic_products(&self) -> Vec<String> {
        let mut products = Vec::new();
        let mut seen = HashSet::new();
        for game in self
            .games
            .iter()
            .filter(|game| game.store == Store::Epic && !game.upcoming && !game.owned)
        {
            let candidates: Vec<&String> = if game.browser_only {
                std::iter::once(&game.id)
                    .chain(game.offers.iter())
                    .collect()
            } else {
                vec![&game.id]
            };
            for product in candidates {
                if game.owned_offers.contains(product) {
                    continue;
                }
                if seen.insert(product.clone()) {
                    products.push(product.clone());
                }
            }
        }
        products
    }

    /// Adresse `purchase?offers=…-#/free-checkout` : chaque offre Epic y entre
    /// sous la forme `1-<namespace>-<offerId>`, et le checkout à zéro les
    /// réunit.
    fn epic_purchase_url(&self) -> Option<String> {
        Self::purchase_url(&self.epic_products())
    }

    /// Variante pure, pour que le format de l'adresse reste testable sans
    /// construire toute la page.
    fn purchase_url(products: &[String]) -> Option<String> {
        let mut url = String::from("https://store.epicgames.com/purchase?");
        let mut first = true;
        for product in products {
            let Some((namespace, offer)) = product.split_once(':') else {
                continue;
            };
            if namespace.is_empty() || offer.is_empty() {
                continue;
            }
            if !first {
                url.push('&');
            }
            url.push_str("offers=1-");
            url.push_str(namespace);
            url.push('-');
            url.push_str(offer);
            first = false;
        }
        if first {
            return None;
        }
        url.push_str("#/free-checkout");
        Some(url)
    }

    /// Offres cochées à pousser dans le panier du site : `"namespace:offre"`,
    /// la forme que la boutique attend.
    fn selected_products(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        self.games
            .iter()
            .filter(|game| !game.upcoming && !game.owned && self.selected.contains(&game.id))
            // Toutes les plateformes d'une carte partent au panier : garder la
            // seule première laissait la version iOS de côté.
            .flat_map(|game| std::iter::once(game.id.clone()).chain(game.offers.iter().cloned()))
            .filter(|product| seen.insert(product.clone()))
            .collect()
    }

    /// Le prochain envoi au panier demandé par l'interface.
    pub(crate) fn take_cart_request(&mut self) -> Option<Vec<String>> {
        self.cart_request.take()
    }

    /// Remplit le panier Epic en tâche de fond, sans fenêtre.
    ///
    /// Les cookies de la boutique sont déjà écrits sur le disque par sa
    /// fenêtre : le panier répond donc directement, et le verdict s'affiche
    /// dans un modal plutôt que dans une fenêtre de navigateur.
    pub(crate) fn start_cart(&mut self, paths: &LapinuxPaths, products: Vec<String>) {
        if self.cart_fill.is_some() || products.is_empty() {
            return;
        }
        let dir = paths.run_dir.join("cart");
        std::fs::create_dir_all(&dir).ok();
        let script = dir.join("cart.py");
        if std::fs::write(&script, CART_HELPER).is_err() {
            self.message = Some(lapinux_core::i18n::t("freegames.cart_script_error").into());
            return;
        }
        let jar = paths.store_web_dir().join("cookies.sqlite");
        let mut command = Command::new("python3");
        command.arg(&script).arg(&jar);
        for product in products {
            command.arg(product);
        }
        command
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .stdout(Stdio::piped());
        let (tx, rx) = mpsc::channel();
        self.cart_fill = Some(rx);
        self.message = None;
        std::thread::spawn(move || {
            let reply = match command.output() {
                Ok(output) => {
                    let text = String::from_utf8_lossy(&output.stdout);
                    let line = text.lines().last().unwrap_or("").trim();
                    serde_json::from_str::<CartReport>(line)
                        .map_err(|error| format!("verdict illisible ({error})"))
                }
                Err(error) => Err(error.to_string()),
            };
            let _ = tx.send(reply);
        });
    }

    /// La phrase du verdict : ce qui est entré, ce qu'Epic a refusé, et quoi
    /// faire quand la session du site manque.
    fn cart_message(reply: CartReply) -> String {
        match reply {
            Ok(report) => {
                if report.not_logged_in {
                    lapinux_core::i18n::t("freegames.cart_login").to_string()
                } else if report.added == 0 && report.errors.is_empty() {
                    lapinux_core::i18n::t("freegames.cart_failed").to_string()
                } else if !report.errors.is_empty() {
                    lapinux_core::i18n::t_fmt(
                        "freegames.cart_refused",
                        &[report.added.to_string(), report.errors.len().to_string()],
                    )
                } else {
                    lapinux_core::i18n::t_fmt("freegames.cart_added", &[report.added.to_string()])
                }
            }
            Err(error) => {
                lapinux_core::i18n::t_args("freegames.cart_error", &[("error", &error.to_string())])
            }
        }
    }

    /// Verdict du remplissage, dans un modal : la boutique ne s'ouvre plus
    /// pour remplir le panier, seulement si l'on veut voir ou payer.
    fn cart_done_window(&mut self, ctx: &egui::Context) {
        let Some(message) = self.cart_done.clone() else {
            return;
        };
        let mut open = true;
        let mut page = false;
        let mut close = false;
        egui::Window::new("Panier Epic")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_width(460.0)
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(message.clone())
                        .size(11.5)
                        .color(TEXT()),
                );
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if action_button(
                        ui,
                        lapinux_core::i18n::t("freegames.view_cart_site"),
                        ActionTone::Primary,
                    )
                    .clicked()
                    {
                        page = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if action_button(
                            ui,
                            lapinux_core::i18n::t("freegames.close"),
                            ActionTone::Quiet,
                        )
                        .clicked()
                        {
                            close = true;
                        }
                    });
                });
            });
        if page {
            self.cart_page = Some("https://store.epicgames.com/cart".to_string());
            self.cart_done = None;
            return;
        }
        if close || !open {
            self.cart_done = None;
        }
    }

    /// L'adresse Epic à ouvrir, quand elle est prête.
    pub(crate) fn take_cart_page(&mut self) -> Option<String> {
        self.cart_page.take()
    }

    /// Le checkout Epic part dans le navigateur du bureau : sa protection
    /// Cloudflare ne termine pas dans la fenêtre WebKit intégrée.
    pub(crate) fn checkout_opened_external(&mut self) {
        self.message = Some(lapinux_core::i18n::t("freegames.epic_external").to_string());
    }

    /// Rattache au compte les offres cochées dans le panier. Les offres à
    /// venir sont écartées : la boutique les refuse avant leur date.
    fn claim_selected(&mut self, client: &Client) {
        if self.claim.is_some() || self.rx.is_some() {
            return;
        }
        // Les offres déjà au compte ne repartent pas en commande : la
        // boutique les refuserait et la carte afficherait une erreur pour un
        // jeu qu'on possède.
        let ids: Vec<String> = self
            .games
            .iter()
            .filter(|game| {
                !game.upcoming
                    && !game.owned
                    && !game.browser_only
                    && self.selected.contains(&game.id)
            })
            .map(|game| game.id.clone())
            .collect();
        if ids.is_empty() {
            // Les offres GOG et itch.io, et les offres mobiles Epic, ne
            // passent pas par une commande : leur carte est le seul point
            // d'entrée, et elle n'ouvre que l'offre choisie.
            let browser = self
                .games
                .iter()
                .filter(|game| {
                    !game.upcoming
                        && !game.owned
                        && game.browser_only
                        && self.selected.contains(&game.id)
                })
                .count();
            self.message = Some(if browser > 0 {
                lapinux_core::i18n::t("freegames.claim_on_card").into()
            } else {
                lapinux_core::i18n::t("freegames.nothing_to_fetch").into()
            });
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.claim = Some(rx);
        self.message = None;
        let worker = client.detached();
        std::thread::spawn(move || {
            let reply = worker
                .request_with_timeout(&Req::StoreClaimFreeGames { ids }, Duration::from_secs(180))
                .map_err(|error| error.to_string())
                .and_then(|envelope| {
                    serde_json::from_value::<Vec<ClaimOutcome>>(envelope.data)
                        .map_err(|error| error.to_string())
                });
            let _ = tx.send(reply);
        });
    }

    /// Récupère directement les licences Steam en cours. Les autres boutiques
    /// n'ont pas d'API de récupération : leurs cartes restent cliquables.
    fn claim_steam(&mut self, client: &Client) {
        self.selected = self
            .games
            .iter()
            .filter(|game| game.store == Store::Steam && !game.upcoming && !game.owned)
            .map(|game| game.id.clone())
            .collect();
        self.claim_selected(client);
    }

    /// Relève les réponses et télécharge les visuels manquants.
    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.rx {
            if let Ok(reply) = rx.try_recv() {
                self.rx = None;
                self.loading = false;
                match reply {
                    Ok(games) => {
                        self.games = games;
                        self.outcomes.clear();
                    }
                    Err(error) => self.error = Some(error),
                }
                ctx.request_repaint();
            } else {
                // Le fil qui répond réveille l'écran ; ce délai n'est qu'un
                // filet de sécurité.
                ctx.request_repaint_after(Duration::from_secs(1));
            }
        }
        if let Some(rx) = &self.cart_fill {
            if let Ok(reply) = rx.try_recv() {
                self.cart_fill = None;
                let opened =
                    matches!(&reply, Ok(report) if report.added > 0 && report.errors.is_empty());
                let message = Self::cart_message(reply);
                self.message = Some(message.clone());
                if opened {
                    // Tout est entré : le panier Epic s'ouvre directement,
                    // sans fenêtre intermédiaire.
                    self.cart_page = Some("https://store.epicgames.com/cart".to_string());
                    self.cart_done = None;
                } else {
                    self.cart_done = Some(message);
                }
                ctx.request_repaint();
            } else {
                // Le script tourne dans son propre processus : l'écran se
                // réveille de lui-même à son verdict.
                ctx.request_repaint_after(Duration::from_millis(400));
            }
        }
        if let Some(rx) = &self.claim {
            if let Ok(reply) = rx.try_recv() {
                self.claim = None;
                match reply {
                    Ok(outcomes) => {
                        // Une offre acceptée est désormais au compte : la carte
                        // le dit tout de suite, sans relire la boutique.
                        for outcome in &outcomes {
                            if outcome.ok {
                                if let Some(game) =
                                    self.games.iter_mut().find(|game| game.id == outcome.id)
                                {
                                    game.owned = true;
                                }
                            }
                        }
                        let ok = outcomes.iter().filter(|outcome| outcome.ok).count();
                        let checkout = outcomes
                            .iter()
                            .filter(|outcome| outcome.needs_checkout)
                            .count();
                        let refused = outcomes.len() - ok - checkout;
                        self.message = Some(if checkout > 0 {
                            lapinux_core::i18n::t_args(
                                "ui.freegames_checkout_result",
                                &[("ok", &ok.to_string()), ("checkout", &checkout.to_string())],
                            )
                        } else if refused == 0 {
                            lapinux_core::i18n::t_args(
                                "freegames.claimed_result",
                                &[("ok", &ok.to_string())],
                            )
                        } else {
                            lapinux_core::i18n::t_args(
                                "ui.freegames_refused_result",
                                &[("ok", &ok.to_string()), ("refused", &refused.to_string())],
                            )
                        });
                        self.outcomes = outcomes;
                    }
                    Err(error) => self.message = Some(error),
                }
                ctx.request_repaint();
            } else {
                ctx.request_repaint_after(Duration::from_secs(1));
            }
        }
        self.cover.poll(ctx);
    }

    /// Verdict pour une offre donnée, quand la récupération a eu lieu.
    fn outcome(&self, id: &str) -> Option<&ClaimOutcome> {
        self.outcomes.iter().find(|outcome| outcome.id == id)
    }

    /// Le panier : les offres encore récupérables, PC, Android et iOS dans la
    /// même liste, cochées par défaut. Valider les rattache au compte ; les
    /// offres mobiles s'ouvrent ensuite dans la boutique, une par une.
    fn cart_window(&mut self, ctx: &egui::Context, client: &Client) {
        if !self.cart_open {
            return;
        }
        // Instantané de la liste : le panier ne retient que le nécessaire, et
        // la boucle peut cocher sans emprunter les jeux en même temps.
        let rows: Vec<(String, String, String)> = self
            .games
            .iter()
            .filter(|game| !game.upcoming && !game.owned)
            .map(|game| {
                (
                    game.id.clone(),
                    game.title.clone(),
                    game.platforms
                        .iter()
                        .map(|platform| platform_label(platform))
                        .collect::<Vec<_>>()
                        .join(" · "),
                )
            })
            .collect();
        let mut open = self.cart_open;
        let mut claim = false;
        let mut cart = false;
        let mut all = false;
        let mut none = false;
        egui::Window::new(lapinux_core::i18n::t("freegames.cart_title"))
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_width(520.0)
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(lapinux_core::i18n::t("freegames.subtitle"))
                        .size(11.0)
                        .color(TEXT_DIM()),
                );
                ui.add_space(8.0);
                for (id, title, platforms) in &rows {
                    let mut checked = self.selected.contains(id);
                    let label = if platforms.is_empty() {
                        title.clone()
                    } else {
                        format!("{title}  ·  {platforms}")
                    };
                    if ui.checkbox(&mut checked, label).changed() {
                        if checked {
                            self.selected.insert(id.clone());
                        } else {
                            self.selected.remove(id);
                        }
                    }
                }
                if rows.is_empty() {
                    ui.label(
                        egui::RichText::new(lapinux_core::i18n::t("freegames.none_to_fetch"))
                            .size(11.5)
                            .color(TEXT_DIM()),
                    );
                }
                ui.add_space(10.0);
                let count = rows
                    .iter()
                    .filter(|(id, _, _)| self.selected.contains(id))
                    .count();
                ui.horizontal(|ui| {
                    if action_button(
                        ui,
                        lapinux_core::i18n::t("freegames.check_all"),
                        ActionTone::Quiet,
                    )
                    .clicked()
                    {
                        all = true;
                    }
                    if action_button(
                        ui,
                        lapinux_core::i18n::t("freegames.untick_all"),
                        ActionTone::Quiet,
                    )
                    .clicked()
                    {
                        none = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let label = if count > 1 {
                            lapinux_core::i18n::t_args(
                                "ui.freegames_send_cart_many",
                                &[("count", &count.to_string())],
                            )
                        } else {
                            lapinux_core::i18n::t_args(
                                "ui.freegames_send_cart_one",
                                &[("count", &count.to_string())],
                            )
                        };
                        if action_button(ui, &label, ActionTone::Primary).clicked() {
                            cart = true;
                        }
                        // Le rattachement direct reste là pour qui préfère
                        // terminer sans quitter Lapinux.
                        if action_button(
                            ui,
                            lapinux_core::i18n::t("freegames.fetch_here"),
                            ActionTone::Quiet,
                        )
                        .clicked()
                        {
                            claim = true;
                        }
                    });
                });
            });
        if all {
            self.selected = rows.iter().map(|(id, _, _)| id.clone()).collect();
        }
        if none {
            self.selected.clear();
        }
        self.cart_open = open;
        if cart {
            // Le panier du site est le seul chemin qui laisse Epic terminer
            // lui-même : les offres y entrent cochées, prêtes à valider.
            let products = self.selected_products();
            if products.is_empty() {
                self.message = Some(lapinux_core::i18n::t("freegames.nothing_to_send").into());
            } else {
                self.cart_open = false;
                self.cart_request = Some(products);
            }
        }
        if claim {
            self.cart_open = false;
            self.claim_selected(client);
        }
    }

    /// La page, sur toute la largeur du panneau. Rend l'adresse à ouvrir dans
    /// la fenêtre de boutique quand une carte est cliquée.
    pub(crate) fn page(&mut self, ui: &mut egui::Ui, client: &Client) -> Option<(String, String)> {
        self.ensure(client);
        self.poll(ui.ctx());
        self.cover.ensure(
            ui.ctx(),
            &self
                .games
                .iter()
                .map(|game| game.cover.clone())
                .collect::<Vec<_>>(),
        );
        // Les actions directes : le panier Epic (PC, Android et iOS) et les
        // licences Steam. GOG et itch.io n'ouvrent que la carte cliquée.
        let epic = self.epic_products().len();
        let steam = self
            .games
            .iter()
            .filter(|game| game.store == Store::Steam && !game.upcoming && !game.owned)
            .count();
        let mut open = None;
        let mut epic_cart = false;
        let mut steam_claim = false;
        let mut refresh = false;

        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new(lapinux_core::i18n::t("freegames.title"))
                        .size(20.0)
                        .strong()
                        .color(TEXT()),
                );
                ui.add_space(3.0);
                ui.label(
                    egui::RichText::new(lapinux_core::i18n::t("ui.freegames_intro"))
                        .size(11.5)
                        .color(TEXT_DIM()),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if epic > 0 {
                    let label = lapinux_core::i18n::t_args(
                        "freegames.epic_cart",
                        &[("count", &epic.to_string())],
                    );
                    if action_button(ui, &label, ActionTone::Primary).clicked() {
                        epic_cart = true;
                    }
                    ui.add_space(8.0);
                }
                if steam > 0 {
                    let label = lapinux_core::i18n::t_args(
                        "freegames.steam_claim",
                        &[("count", &steam.to_string())],
                    );
                    let tone = if epic > 0 {
                        ActionTone::Quiet
                    } else {
                        ActionTone::Primary
                    };
                    if action_button(ui, &label, tone).clicked() {
                        steam_claim = true;
                    }
                    ui.add_space(8.0);
                }
                if action_button(ui, "Actualiser", ActionTone::Quiet).clicked() {
                    refresh = true;
                }
            });
        });
        ui.add_space(12.0);

        // Les quatre boutiques en un regard : une section absente ne doit pas
        // faire croire que la boutique n'a pas été interrogée.
        ui.horizontal_wrapped(|ui| {
            for store in lapinux_core::stores::Store::ALL {
                let count = self
                    .games
                    .iter()
                    .filter(|game| game.store == store && !game.upcoming && !game.owned)
                    .count();
                chip(
                    ui,
                    &format!("{} · {count}", store.label()),
                    if count > 0 { ACCENT() } else { TEXT_DIM() },
                );
                ui.add_space(4.0);
            }
        });
        ui.add_space(8.0);

        if let Some(message) = &self.message {
            ui.label(egui::RichText::new(message.clone()).size(11.5).color(OK));
            ui.add_space(8.0);
        }
        if let Some(error) = &self.error {
            ui.label(egui::RichText::new(error.clone()).size(11.5).color(DANGER));
            ui.add_space(8.0);
        }
        if self.loading && self.games.is_empty() {
            ui.label(
                egui::RichText::new(lapinux_core::i18n::t("freegames.loading"))
                    .size(12.0)
                    .color(TEXT_DIM()),
            );
        }

        let current: Vec<&FreeGame> = self.games.iter().filter(|game| !game.upcoming).collect();
        let next: Vec<&FreeGame> = self.games.iter().filter(|game| game.upcoming).collect();
        if current.is_empty() && next.is_empty() && !self.loading && self.error.is_none() {
            ui.label(
                egui::RichText::new(lapinux_core::i18n::t("freegames.none_published"))
                    .size(12.0)
                    .color(TEXT_DIM()),
            );
        }

        if !current.is_empty() {
            section(ui, lapinux_core::i18n::t("freegames.free_now"));
            for store in lapinux_core::stores::Store::ALL {
                let offers: Vec<_> = current
                    .iter()
                    .copied()
                    .filter(|game| game.store == store)
                    .collect();
                if offers.is_empty() {
                    continue;
                }
                section(ui, &format!("{} · {}", store.label(), offers.len()));
                for game in offers {
                    if let Some((url, title)) = card(
                        ui,
                        game,
                        self.outcome(&game.id),
                        self.cover.get(&game.cover),
                    ) {
                        open = Some((url, title));
                    }
                    ui.add_space(8.0);
                }
            }
        }
        if !next.is_empty() {
            ui.add_space(6.0);
            section(ui, lapinux_core::i18n::t("freegames.next_week"));
            for store in lapinux_core::stores::Store::ALL {
                let offers: Vec<_> = next
                    .iter()
                    .copied()
                    .filter(|game| game.store == store)
                    .collect();
                if offers.is_empty() {
                    continue;
                }
                section(ui, &format!("{} · {}", store.label(), offers.len()));
                for game in offers {
                    if let Some((url, title)) = card(ui, game, None, self.cover.get(&game.cover)) {
                        open = Some((url, title));
                    }
                    ui.add_space(8.0);
                }
            }
        }

        if refresh {
            self.load(client);
        }
        if epic_cart {
            self.epic_cart();
        }
        if steam_claim {
            self.claim_steam(client);
        }
        self.cart_window(ui.ctx(), client);
        self.cart_done_window(ui.ctx());
        open
    }

    /// Relit la liste : après un passage sur la boutique, les droits ont pu
    /// changer et les offres récupérées s'affichent comme telles.
    pub(crate) fn refresh(&mut self, client: &Client) {
        self.load(client);
    }
}

/// Un titre de bloc, souligné d'une ligne discrète.
fn section(ui: &mut egui::Ui, title: &str) {
    ui.label(
        egui::RichText::new(title)
            .size(12.0)
            .strong()
            .color(TEXT_DIM()),
    );
    ui.add_space(6.0);
}

/// Carte d'une offre : visuel, titre, éditeur, valeur, date de fin et verdict.
/// Rend l'adresse de l'offre et le titre de sa fenêtre quand la carte est
/// cliquée.
fn card(
    ui: &mut egui::Ui,
    game: &FreeGame,
    outcome: Option<&ClaimOutcome>,
    cover: Option<&egui::TextureHandle>,
) -> Option<(String, String)> {
    let mut open = false;
    let height = 108.0;
    egui::Frame::none()
        .fill(CARD())
        .stroke(egui::Stroke::new(1.0_f32, BORDER()))
        .rounding(egui::Rounding::same(12.0))
        .inner_margin(egui::Margin::same(10.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (rect, response) =
                    ui.allocate_exact_size(egui::vec2(192.0, height), egui::Sense::click());
                ui.painter()
                    .rect_filled(rect, egui::Rounding::same(8.0), PANEL());
                match cover {
                    Some(texture) => {
                        // Le visuel large remplit son cadre sans déformer.
                        let size = texture.size_vec2();
                        if size.x > 0.0 && size.y > 0.0 {
                            let scale = (rect.width() / size.x).max(rect.height() / size.y);
                            let shown = egui::Rect::from_center_size(rect.center(), size * scale);
                            ui.painter().with_clip_rect(rect).image(
                                texture.id(),
                                shown,
                                egui::Rect::from_min_max(
                                    egui::pos2(0.0, 0.0),
                                    egui::pos2(1.0, 1.0),
                                ),
                                egui::Color32::WHITE,
                            );
                        }
                    }
                    None => {
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            lapinux_core::i18n::t("downloads.free"),
                            egui::FontId::proportional(12.0),
                            TEXT_DIM(),
                        );
                    }
                }
                if response.clicked() {
                    open = true;
                }
                if response.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                ui.add_space(12.0);
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(game.title.clone())
                            .size(14.0)
                            .strong()
                            .color(TEXT()),
                    );
                    ui.add_space(2.0);
                    let mut facts = vec![game.seller.clone()];
                    if !game.value.trim().is_empty() {
                        facts.push(lapinux_core::i18n::t_fmt(
                            "ui.instead_of",
                            &[
                                lapinux_core::i18n::t("news.deals.free").to_string(),
                                game.value.clone(),
                            ],
                        ));
                    }
                    ui.label(
                        egui::RichText::new(facts.join(" · "))
                            .size(11.0)
                            .color(TEXT_DIM()),
                    );
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        chip(ui, game.store.label(), ACCENT());
                        ui.add_space(4.0);
                        for platform in &game.platforms {
                            chip(ui, platform_label(platform), OK);
                        }
                        ui.add_space(4.0);
                        if game.upcoming {
                            chip(
                                ui,
                                &lapinux_core::i18n::t_fmt(
                                    "freegames.from_date",
                                    &[lapinux_core::util::format_date(game.start).to_string()],
                                ),
                                TEXT_DIM(),
                            );
                            ui.add_space(4.0);
                        }
                        // La date de fin reste visible même sur une offre
                        // mobile ou déjà récupérée : c'est elle qui dit
                        // combien de temps la promotion court.
                        chip(ui, &deadline_label(game.end), WARN);
                        ui.add_space(4.0);
                        if game.owned {
                            chip(ui, lapinux_core::i18n::t("freegames.fetched"), OK);
                        } else if game.browser_only {
                            // GOG, itch.io et mobile : la carte est le seul
                            // chemin vers cette offre.
                            chip(ui, lapinux_core::i18n::t("freegames.on_store"), ACCENT());
                        }
                    });
                    if game.owned {
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new(lapinux_core::i18n::t(
                                "freegames.already_in_library",
                            ))
                            .size(11.0)
                            .color(OK),
                        );
                    } else if game.browser_only {
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new(lapinux_core::i18n::t("freegames.fetch_on_store"))
                                .size(11.0)
                                .color(ACCENT()),
                        );
                    } else if let Some(outcome) = outcome {
                        ui.add_space(6.0);
                        if outcome.ok {
                            ui.label(
                                egui::RichText::new(lapinux_core::i18n::t("freegames.added"))
                                    .size(11.0)
                                    .color(OK),
                            );
                        } else if outcome.needs_checkout {
                            // Epic ouvre un passage en caisse : la suite se
                            // fait sur la page de l'offre, ouverte au clic.
                            ui.label(
                                egui::RichText::new(lapinux_core::i18n::t(
                                    "ui.freegames_checkout_hint",
                                ))
                                .size(11.0)
                                .color(WARN),
                            );
                        } else {
                            ui.label(
                                egui::RichText::new(outcome.error.clone())
                                    .size(11.0)
                                    .color(DANGER),
                            );
                        }
                    }
                });
            });
        });
    // Cliquer la carte ouvre l'offre ; la récupération vit dans l'en-tête de
    // la page, pour ne pas multiplier les boutons.
    open.then(|| {
        (
            game.url.clone(),
            format!("{} · {} · Lapinux", game.title, game.store.label()),
        )
    })
}

/// Pastille de plateforme ou d'échéance : un fond discret, un texte lisible.
fn chip(ui: &mut egui::Ui, text: &str, color: egui::Color32) {
    let galley =
        ui.painter()
            .layout_no_wrap(text.to_string(), egui::FontId::proportional(10.5), color);
    let size = egui::vec2(galley.size().x + 14.0, 18.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, egui::Rounding::same(9.0), CARD_ACTIVE());
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, color);
}

fn platform_label(platform: &str) -> &str {
    match platform {
        "mobile" => "Mobile",
        "android" => "Android",
        "ios" => "iOS",
        _ => "PC",
    }
}

/// « Fin le 13/11/2026 à 17:00 » : la date locale de fin de promotion.
fn deadline_label(end: i64) -> String {
    // Une offre sans échéance publique se lit comme l'offre de la semaine,
    // sans inventer de date.
    if end <= 0 {
        return lapinux_core::i18n::t("freegames.weekly_offer").to_string();
    }
    lapinux_core::i18n::t_args(
        "freegames.ends_on",
        &[("date", &lapinux_core::util::format_datetime(end))],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_deadline_is_a_local_date_and_time() {
        let text = deadline_label(1_700_000_000);
        assert!(text.contains('/'), "{text}");
        assert!(text.contains(':'), "{text}");
        assert_eq!(
            deadline_label(0),
            lapinux_core::i18n::t("freegames.weekly_offer")
        );
    }

    #[test]
    fn epic_checkout_joins_every_offer() {
        let url = FreeGames::purchase_url(&[
            "27e33143fc9e4de8bbf76782292933d4:3be0a0d8479946d78f60d90c9442a0b4".into(),
            "ns2:offer2".into(),
        ])
        .unwrap();
        assert_eq!(
            url,
            "https://store.epicgames.com/purchase?offers=1-27e33143fc9e4de8bbf76782292933d4-3be0a0d8479946d78f60d90c9442a0b4&offers=1-ns2-offer2#/free-checkout"
        );
        assert!(FreeGames::purchase_url(&[]).is_none());
    }

    #[test]
    fn epic_checkout_skips_owned_offers_but_keeps_every_platform() {
        let game =
            |id: &str, browser_only: bool, platforms: &[&str], offers: &[&str], held: &[&str]| {
                FreeGame {
                    store: Store::Epic,
                    id: id.into(),
                    browser_only,
                    platforms: platforms
                        .iter()
                        .map(|platform| platform.to_string())
                        .collect(),
                    offers: offers.iter().map(|offer| offer.to_string()).collect(),
                    owned_offers: held.iter().map(|offer| offer.to_string()).collect(),
                    ..Default::default()
                }
            };
        let free = FreeGames {
            games: vec![
                game(
                    "ns:chants-android",
                    true,
                    &["android", "ios"],
                    &["ns:chants-ios"],
                    &[],
                ),
                game("ns:flotsam", false, &["pc"], &[], &[]),
                game(
                    "ns:idle-android",
                    true,
                    &["android", "ios"],
                    &["ns:idle-ios"],
                    &["ns:idle-android"],
                ),
            ],
            ..Default::default()
        };
        assert_eq!(
            free.epic_products(),
            vec![
                "ns:chants-android".to_string(),
                "ns:chants-ios".to_string(),
                "ns:flotsam".to_string(),
                "ns:idle-ios".to_string(),
            ]
        );
    }

    #[test]
    fn the_cart_verdict_names_what_went_in() {
        let report = CartReport {
            added: 2,
            errors: Vec::new(),
            not_logged_in: false,
        };
        assert_eq!(
            FreeGames::cart_message(Ok(report)),
            lapinux_core::i18n::t_fmt("freegames.cart_added", &["2".to_string()])
        );
    }

    #[test]
    fn a_refused_offer_is_counted_apart() {
        let report = CartReport {
            added: 1,
            errors: vec!["27889a97:1c55aa0a".into()],
            not_logged_in: false,
        };
        assert_eq!(
            FreeGames::cart_message(Ok(report)),
            lapinux_core::i18n::t_fmt(
                "freegames.cart_refused",
                &["1".to_string(), "1".to_string()]
            )
        );
    }

    #[test]
    fn a_missing_session_asks_for_a_login() {
        let report = CartReport {
            added: 0,
            errors: vec!["x:y".into()],
            not_logged_in: true,
        };
        let message = FreeGames::cart_message(Ok(report));
        assert_eq!(message, lapinux_core::i18n::t("freegames.cart_login"));
    }

    #[test]
    fn platform_labels_are_french() {
        assert_eq!(platform_label("pc"), "PC");
        assert_eq!(platform_label("mobile"), "Mobile");
        assert_eq!(platform_label("android"), "Android");
        assert_eq!(platform_label("ios"), "iOS");
    }

    #[test]
    fn an_offer_without_deadline_reads_as_the_weekly_offer() {
        assert_eq!(
            deadline_label(0),
            lapinux_core::i18n::t("freegames.weekly_offer")
        );
    }
}
