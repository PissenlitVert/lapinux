//! Section **Boutiques › Jeux gratuits** : les offres du moment chez Epic
//! Games, leur échéance, et de quoi les rattacher au compte en un clic.
//!
//! La liste vient du daemon, qui lit le flux public de la boutique : elle
//! s'affiche même sans compte connecté. Seule la récupération demande une
//! session Epic — le bouton le dit au lieu d'échouer après coup.

use crate::gameinfo::NewsArt;
use crate::{action_button, ActionTone};
use crate::{ACCENT, BORDER, CARD, CARD_ACTIVE, DANGER, OK, PANEL, TEXT, TEXT_DIM, WARN};
use eframe::egui;
use lapinux_core::client::Client;
use lapinux_core::ipc::Req;
use lapinux_core::paths::LapinuxPaths;
use lapinux_core::stores::{ClaimOutcome, FreeGame};
use std::collections::{HashSet, VecDeque};
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
    /// Offres à terminer sur la boutique, ouvertes une par une : Epic y
    /// renvoie le passage en caisse que l'API ne sait pas clore, et les offres
    /// mobiles n'existent que sur leur page. Chaque adresse n'est proposée
    /// qu'une fois, pour ne pas rouvrir sans fin une offre laissée de côté.
    queue: VecDeque<String>,
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
    /// Le panier du site doit s'ouvrir : demande faite depuis le modal.
    cart_page: bool,
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
                .request_with_timeout(&Req::StoreFreeGames, Duration::from_secs(60))
                .map_err(|error| error.to_string())
                .and_then(|envelope| {
                    serde_json::from_value::<Vec<FreeGame>>(envelope.data)
                        .map_err(|error| error.to_string())
                });
            let _ = tx.send(reply);
        });
    }

    /// Ouvre le panier avec toutes les offres encore récupérables cochées :
    /// PC, Android et iOS dans la même liste.
    fn open_cart(&mut self) {
        self.selected = self
            .games
            .iter()
            .filter(|game| !game.upcoming && !game.owned)
            .map(|game| game.id.clone())
            .collect();
        self.cart_open = !self.selected.is_empty();
        self.message = Some(if self.selected.is_empty() {
            "Tout est déjà dans ta bibliothèque Epic.".into()
        } else {
            // Une carte peut porter plusieurs offres (Android et iOS) : le
            // compte annoncé est celui des offres, pas des cartes.
            format!(
                "{} offre(s) dans le panier : décoche ce que tu ne veux pas.",
                self.selected_products().len()
            )
        });
    }

    /// Offres cochées à pousser dans le panier du site : `"namespace:offre"`,
    /// la forme que la boutique attend.
    fn selected_products(&self) -> Vec<String> {
        self.games
            .iter()
            .filter(|game| {
                !game.upcoming && !game.owned && self.selected.contains(&game.id)
            })
            // Toutes les plateformes d'une carte partent au panier : garder la
            // seule première laissait la version iOS de côté.
            .flat_map(|game| {
                std::iter::once(game.id.clone()).chain(game.offers.iter().cloned())
            })
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
            self.message =
                Some("Panier Epic : script de remplissage non installable.".into());
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
                    "Connecte-toi à Epic dans la fenêtre de boutique, puis relance \
                     l'envoi au panier."
                        .to_string()
                } else if report.added == 0 && report.errors.is_empty() {
                    "Rien n'a pu être ajouté au panier Epic.".to_string()
                } else if !report.errors.is_empty() {
                    format!(
                        "{} offre(s) dans le panier Epic ; {} refusée(s) par la boutique.",
                        report.added,
                        report.errors.len()
                    )
                } else {
                    format!("{} offre(s) dans le panier Epic.", report.added)
                }
            }
            Err(error) => format!("Panier Epic : {error}"),
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
                ui.label(egui::RichText::new(message.clone()).size(11.5).color(TEXT()));
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if action_button(ui, "Voir le panier sur le site", ActionTone::Primary).clicked()
                    {
                        page = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if action_button(ui, "Fermer", ActionTone::Quiet).clicked() {
                            close = true;
                        }
                    });
                });
            });
        if page {
            self.cart_page = true;
            self.cart_done = None;
            return;
        }
        if close || !open {
            self.cart_done = None;
        }
    }

    /// Le panier du site doit-il s'ouvrir ? Demande faite depuis le modal.
    pub(crate) fn take_cart_page(&mut self) -> bool {
        std::mem::take(&mut self.cart_page)
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
        // Les offres mobiles ne passent pas par le panier Epic : leur page est
        // la seule voie, et elle s'ouvre dans la même fenêtre de boutique.
        self.queue.clear();
        for game in self.games.iter().filter(|game| {
            !game.upcoming
                && !game.owned
                && game.browser_only
                && self.selected.contains(&game.id)
        }) {
            self.queue.push_back(game.url.clone());
        }
        if ids.is_empty() {
            self.message = Some(if self.queue.is_empty() {
                "Aucune offre cochée : rien à récupérer.".into()
            } else {
                format!(
                    "{} offre(s) mobile(s) s'ouvrent sur la boutique Epic, une par une.",
                    self.queue.len()
                )
            });
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.claim = Some(rx);
        self.message = None;
        let worker = client.detached();
        std::thread::spawn(move || {
            let reply = worker
                .request_with_timeout(
                    &Req::StoreClaimFreeGames { ids },
                    Duration::from_secs(120),
                )
                .map_err(|error| error.to_string())
                .and_then(|envelope| {
                    serde_json::from_value::<Vec<ClaimOutcome>>(envelope.data)
                        .map_err(|error| error.to_string())
                });
            let _ = tx.send(reply);
        });
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
                let message = Self::cart_message(reply);
                self.message = Some(message.clone());
                self.cart_done = Some(message);
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
                        // Epic a renvoyé l'offre vers son passage en caisse :
                        // sa page rejoint la file et s'ouvre toute seule, une
                        // fenêtre à la fois ; la relecture après chaque
                        // fermeture confirmera le droit obtenu.
                        for outcome in outcomes.iter().filter(|outcome| outcome.needs_checkout) {
                            if let Some(game) =
                                self.games.iter().find(|game| game.id == outcome.id)
                            {
                                if !self.queue.contains(&game.url) {
                                    self.queue.push_back(game.url.clone());
                                }
                            }
                        }
                        self.message = Some(if checkout > 0 {
                            format!(
                                "{ok} offre(s) ajoutée(s) ; {checkout} se termine(nt) sur la \
                                 boutique Epic, qui s'ouvre pour toi."
                            )
                        } else if refused == 0 {
                            format!("{ok} offre(s) ajoutée(s) au compte Epic.")
                        } else {
                            format!(
                                "{ok} offre(s) ajoutée(s), {refused} refusée(s) — le détail \
                                 est sous chaque jeu."
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
        egui::Window::new("Panier — jeux gratuits")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_width(520.0)
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(
                        "Tout ce que ton compte Epic peut encore récupérer cette semaine.",
                    )
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
                        egui::RichText::new("Aucune offre à récupérer.")
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
                    if action_button(ui, "Tout cocher", ActionTone::Quiet).clicked() {
                        all = true;
                    }
                    if action_button(ui, "Tout décocher", ActionTone::Quiet).clicked() {
                        none = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let label = if count > 1 {
                            format!("Envoyer {count} offres au panier Epic")
                        } else {
                            format!("Envoyer {count} offre au panier Epic")
                        };
                        if action_button(ui, &label, ActionTone::Primary).clicked() {
                            cart = true;
                        }
                        // Le rattachement direct reste là pour qui préfère
                        // terminer sans quitter Lapinux.
                        if action_button(ui, "Récupérer ici", ActionTone::Quiet).clicked() {
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
                self.message = Some("Aucune offre cochée : rien à envoyer.".into());
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
    pub(crate) fn page(&mut self, ui: &mut egui::Ui, client: &Client) -> Option<String> {
        self.ensure(client);
        self.poll(ui.ctx());
        self.cover
            .ensure(ui.ctx(), &self.games.iter().map(|game| game.cover.clone()).collect::<Vec<_>>());
        let now = lapinux_core::util::now_unix();
        // Ce qui reste à rattacher : le bouton le dit, et ne promet rien
        // quand tout est déjà au compte.
        let claimable = self
            .games
            .iter()
            .filter(|game| !game.upcoming && !game.owned)
            .count();
        let mut open = None;
        let mut cart = false;
        let mut refresh = false;

        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new("Jeux gratuits")
                        .size(20.0)
                        .strong()
                        .color(TEXT()),
                );
                ui.add_space(3.0);
                ui.label(
                    egui::RichText::new(
                        "Les offres du moment chez Epic Games : jeux PC, puis les \
                         versions Android et iOS de la semaine.",
                    )
                    .size(11.5)
                    .color(TEXT_DIM()),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let label = if claimable > 0 {
                    format!("Panier ({claimable})")
                } else {
                    "Tout est récupéré".to_string()
                };
                if action_button(ui, &label, ActionTone::Primary).clicked() {
                    cart = true;
                }
                ui.add_space(8.0);
                if action_button(ui, "Actualiser", ActionTone::Quiet).clicked() {
                    refresh = true;
                }
            });
        });
        ui.add_space(12.0);

        if let Some(message) = &self.message {
            ui.label(egui::RichText::new(message.clone()).size(11.5).color(OK));
            ui.add_space(8.0);
        }
        if let Some(error) = &self.error {
            ui.label(
                egui::RichText::new(error.clone())
                    .size(11.5)
                    .color(DANGER),
            );
            ui.add_space(8.0);
        }
        if self.loading && self.games.is_empty() {
            ui.label(
                egui::RichText::new("Lecture des offres chez Epic…")
                    .size(12.0)
                    .color(TEXT_DIM()),
            );
        }

        let current: Vec<&FreeGame> = self.games.iter().filter(|game| !game.upcoming).collect();
        let next: Vec<&FreeGame> = self.games.iter().filter(|game| game.upcoming).collect();
        if current.is_empty() && next.is_empty() && !self.loading && self.error.is_none() {
            ui.label(
                egui::RichText::new("Aucune offre gratuite publiée en ce moment.")
                    .size(12.0)
                    .color(TEXT_DIM()),
            );
        }

        if !current.is_empty() {
            section(ui, "Gratuit maintenant");
            for game in &current {
                if let Some(url) = card(
                    ui,
                    game,
                    now,
                    self.outcome(&game.id),
                    self.cover.get(&game.cover),
                ) {
                    open = Some(url);
                }
                ui.add_space(8.0);
            }
        }
        if !next.is_empty() {
            ui.add_space(6.0);
            section(ui, "La semaine prochaine");
            for game in &next {
                if let Some(url) = card(ui, game, now, None, self.cover.get(&game.cover)) {
                    open = Some(url);
                }
                ui.add_space(8.0);
            }
        }

        // Une carte ouverte à la main n'entraîne pas la file : elle s'arrête
        // là où l'utilisateur l'a voulu.
        if open.is_some() {
            self.queue.clear();
        }
        if refresh {
            self.load(client);
        }
        if cart {
            self.open_cart();
        }
        self.cart_window(ui.ctx(), client);
        self.cart_done_window(ui.ctx());
        open
    }

    /// La prochaine offre à terminer sur la boutique, quand la fenêtre s'est
    /// refermée : c'est le passage en caisse Epic ou une offre mobile.
    pub(crate) fn take_next(&mut self) -> Option<String> {
        self.queue.pop_front()
    }

    /// Rien en vol : la file peut avancer sans se marcher dessus.
    pub(crate) fn idle(&self) -> bool {
        !self.loading && self.rx.is_none() && self.claim.is_none()
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

/// Carte d'une offre : visuel, titre, éditeur, valeur, échéance et verdict.
/// Rend l'adresse de l'offre quand la carte est cliquée.
fn card(
    ui: &mut egui::Ui,
    game: &FreeGame,
    now: i64,
    outcome: Option<&ClaimOutcome>,
    cover: Option<&egui::TextureHandle>,
) -> Option<String> {
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
                            "Gratuit",
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
                        facts.push(format!("{} au lieu de {}", "Gratuit", game.value));
                    }
                    ui.label(
                        egui::RichText::new(facts.join(" · "))
                            .size(11.0)
                            .color(TEXT_DIM()),
                    );
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        for platform in &game.platforms {
                            chip(ui, platform_label(platform), OK);
                        }
                        ui.add_space(4.0);
                        if game.upcoming {
                            chip(
                                ui,
                                &format!("à partir du {}", lapinux_core::util::format_date(game.start)),
                                TEXT_DIM(),
                            );
                        } else if game.owned {
                            // Une offre déjà au compte n'a plus d'échéance à
                            // afficher : ce qui compte, c'est qu'elle est là.
                            chip(ui, "Récupéré", OK);
                        } else if game.browser_only {
                            // Offre mobile : la récupération vit sur sa page.
                            chip(ui, "Sur la boutique", ACCENT());
                        } else {
                            chip(ui, &ending_label(game.end, now), ACCENT());
                        }
                    });
                    if game.owned {
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new("Déjà dans ta bibliothèque Epic")
                                .size(11.0)
                                .color(OK),
                        );
                    } else if game.browser_only {
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new(
                                "À récupérer sur la page Epic — clique la carte pour l'ouvrir",
                            )
                            .size(11.0)
                            .color(ACCENT()),
                        );
                    } else if let Some(outcome) = outcome {
                        ui.add_space(6.0);
                        if outcome.ok {
                            ui.label(
                                egui::RichText::new("Ajouté au compte")
                                    .size(11.0)
                                    .color(OK),
                            );
                        } else if outcome.needs_checkout {
                            // Epic ouvre un passage en caisse : la suite se
                            // fait sur la page de l'offre, ouverte au clic.
                            ui.label(
                                egui::RichText::new(
                                    "Epic demande de terminer sur la boutique — clique la \
                                     carte",
                                )
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
    open.then(|| game.url.clone())
}

/// Pastille de plateforme ou d'échéance : un fond discret, un texte lisible.
fn chip(ui: &mut egui::Ui, text: &str, color: egui::Color32) {
    let galley = ui
        .painter()
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

/// « se termine dans 2 j 4 h », « se termine dans 12 min ».
fn ending_label(end: i64, now: i64) -> String {
    // Une offre sans échéance publique (les jeux mobiles) se lit comme
    // l'offre de la semaine, sans inventer de date.
    if end <= 0 {
        return "offre de la semaine".to_string();
    }
    let left = end - now;
    if left <= 0 {
        return "termine aujourd'hui".to_string();
    }
    let days = left / 86_400;
    if days >= 1 {
        let hours = (left % 86_400) / 3_600;
        return if hours > 0 {
            format!("se termine dans {days} j {hours} h")
        } else {
            format!("se termine dans {days} j")
        };
    }
    let hours = left / 3_600;
    if hours >= 1 {
        format!("se termine dans {hours} h")
    } else {
        format!("se termine dans {} min", (left / 60).max(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ending_label_counts_down_in_readable_steps() {
        let now = 1_000_000;
        assert_eq!(ending_label(now + 2 * 86_400 + 4 * 3_600, now), "se termine dans 2 j 4 h");
        assert_eq!(ending_label(now + 5 * 3_600, now), "se termine dans 5 h");
        assert_eq!(ending_label(now + 600, now), "se termine dans 10 min");
        assert_eq!(ending_label(now - 10, now), "termine aujourd'hui");
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
            "2 offre(s) dans le panier Epic."
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
            "1 offre(s) dans le panier Epic ; 1 refusée(s) par la boutique."
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
        assert!(message.starts_with("Connecte-toi à Epic"), "{message}");
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
        assert_eq!(ending_label(0, 1_000_000), "offre de la semaine");
    }
}
