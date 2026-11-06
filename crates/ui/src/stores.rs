//! Réglages › Comptes : la liste des boutiques à gauche, le compte choisi à
//! droite.
//!
//! Steam garde son propre panneau, dessiné par [`crate::LapinuxApp`] ; ce module
//! tient les trois autres (Epic Games, GOG, itch.io) : état du compte, capacités
//! réelles de la boutique, bibliothèque possédée et installation. Aucune des
//! trois n'expose de succès, itch n'a pas de cloud : la carte le dit au lieu de
//! promettre un encart qui resterait vide.

use crate::{action_button, muted, settings_card, status_dot, ActionTone};
use crate::{ACCENT, BACKGROUND, BORDER, CARD, DANGER, OK, PANEL, TEXT, TEXT_DIM, WARN};
use eframe::egui;
use lapinux_core::client::Client;
use lapinux_core::ipc::Req;
use lapinux_core::stores::{Store, StoreAccount, StoreLibrary, StoresView};
use std::collections::HashMap;
use std::sync::mpsc;
use std::time::Duration;

/// Réponse du daemon, avec la boutique concernée pour le message.
type Reply = (Store, Result<StoresView, String>);

/// Délai d'une lecture en cache : la réponse vient de la base, sans réseau.
const STORES_CACHE_TIMEOUT: Duration = Duration::from_secs(90);
/// Délai d'une relecture réseau : les trois boutiques interrogent leurs
/// services, parfois en plusieurs requêtes. Une grande bibliothèque peut
/// dépasser 90 s ; la demande vit dans un fil de fond, elle ne fige pas
/// l'interface et doit avoir le temps d'aboutir.
const STORES_REFRESH_TIMEOUT: Duration = Duration::from_secs(300);

fn store_request_timeout(request: &Req) -> Duration {
    match request {
        Req::Stores { refresh: true, .. } => STORES_REFRESH_TIMEOUT,
        _ => STORES_CACHE_TIMEOUT,
    }
}

pub(crate) struct Stores {
    pub view: StoresView,
    /// Boutique dont le panneau est ouvert ; Steam au départ.
    pub selected: Store,
    /// SteamGridDB a son propre panneau, à part de Steam : c'est un service
    /// d'illustrations, pas un compte de jeu.
    pub steamgriddb_selected: bool,
    /// Saisie par boutique : code d'autorisation, ou clé d'API pour itch.
    token: HashMap<Store, String>,
    /// Dernier refus, par boutique : un code invalide se dit sous le champ.
    error: HashMap<Store, String>,
    /// Échange en cours avec le daemon, boutique concernée comprise ; le
    /// drapeau dit si son succès mérite une notification (la première lecture
    /// silencieuse n'en veut pas).
    rx: Option<(Store, bool, mpsc::Receiver<Reply>)>,
    /// Demande laissée en attente quand l'échange était occupé : une
    /// déconnexion ne se perd pas parce qu'une lecture de bibliothèque traîne.
    queued: Option<(Store, bool, Req, Client)>,
    /// Instant (secondes Unix) de la relecture demandée : la vue est
    /// redemandée jusqu'à ce que chaque boutique ait une lecture plus récente.
    refresh_at: Option<i64>,
    /// Une réponse a remplacé la vue des comptes ou des bibliothèques : la
    /// liste fusionnée doit être reconstruite, même hors de la page
    /// Bibliothèque.
    changed: bool,
    /// Installation en cours de lancement : la lecture du plan passe par le
    /// réseau, elle ne doit pas figer l'écran.
    install: Option<(Store, mpsc::Receiver<std::result::Result<String, String>>)>,
    /// Première lecture demandée : les bibliothèques arrivent ensuite d'elles-mêmes.
    asked: bool,
    /// Relecture des bibliothèques demandée par la page Bibliothèque.
    asked_games: bool,
    /// Version retenue par jeu avant l'installation (`linux`, `windows`…).
    pub(crate) platform: HashMap<u32, String>,
    /// Une bibliothèque se relit en tâche de fond après une connexion : la
    /// vue est relue tant qu'une boutique connectée n'a pas encore rendu ses
    /// jeux (date de lecture à zéro).
    settle: Option<std::time::Instant>,
    /// Nombre de relectures déjà tentées pour ce règlement.
    settle_tries: u32,
    /// Contexte egui retenu à la dernière lecture : chaque fil de fond
    /// réveille l'interface dès qu'il répond, au lieu de la faire repeindre à
    /// intervalle fixe pendant toute la durée de l'attente.
    ctx: Option<egui::Context>,
}

impl Default for Stores {
    fn default() -> Self {
        Self {
            view: StoresView::default(),
            selected: Store::Steam,
            steamgriddb_selected: false,
            token: HashMap::new(),
            error: HashMap::new(),
            rx: None,
            queued: None,
            refresh_at: None,
            changed: false,
            install: None,
            asked: false,
            asked_games: false,
            platform: HashMap::new(),
            settle: None,
            settle_tries: 0,
            ctx: None,
        }
    }
}

impl Stores {
    /// Compte d'une boutique, réel ou vide.
    fn account(&self, store: Store) -> StoreAccount {
        self.view
            .accounts
            .iter()
            .find(|account| account.store == store)
            .cloned()
            .unwrap_or_else(|| StoreAccount::disconnected(store))
    }

    fn library(&self, store: Store) -> StoreLibrary {
        self.view
            .libraries
            .iter()
            .find(|library| library.store == store)
            .cloned()
            .unwrap_or_else(|| StoreLibrary {
                store,
                ..Default::default()
            })
    }

    /// Jeux publiés par les boutiques connectées : la bibliothèque les
    /// mélange aux jeux Steam, avec leur jaquette d'origine.
    pub(crate) fn owned_games(&self) -> Vec<lapinux_core::stores::StoreGame> {
        let mut games = Vec::new();
        for library in &self.view.libraries {
            // Une boutique non connectée n'a rien à montrer : sa bibliothèque
            // peut être vide ou rester d'une session précédente.
            games.extend(library.entries.iter().cloned());
        }
        games
    }

    /// Boutique et jaquette d'un jeu déjà installé, quand une boutique le
    /// publie : la fiche s'en sert pour l'image de la ligne compacte.
    pub(crate) fn cover_of(
        &self,
        appid: u32,
    ) -> Option<(lapinux_core::stores::Store, String)> {
        self.owned_games()
            .into_iter()
            .find(|game| game.appid == appid)
            .map(|game| (game.store, game.cover))
    }

    /// Une lecture silencieuse (ouverture de page, règlement après connexion)
    /// n'occupe pas la boutique : seules les actions demandées affichent
    /// « Connexion… » et bloquent le bouton.
    fn busy(&self, store: Store) -> bool {
        self.rx
            .as_ref()
            .map(|(busy, quiet, _)| *busy == store && !*quiet)
            .unwrap_or(false)
    }

    /// Demande la vue au daemon à la première ouverture : elle vient de son
    /// cache, sans lecture réseau, donc elle s'affiche tout de suite. L'état du
    /// compte et le lien d'autorisation ne doivent jamais attendre la
    /// relecture d'une bibliothèque de mille cinq cents jeux.
    ///
    /// Tant que la réponse ne couvre pas les trois boutiques (daemon remplacé
    /// après une mise à jour, réponse perdue), la demande repart : c'est une
    /// requête locale.
    pub(crate) fn ensure(&mut self, client: &Client) {
        // Une connexion vient d'aboutir et le daemon relit la bibliothèque en
        // tâche de fond : la vue est redemandée toutes les deux secondes
        // jusqu'à ce que les jeux arrivent, sans jamais bloquer l'écran.
        if self.settle.is_some() {
            let pending = self.view.accounts.iter().any(|account| {
                account.connected
                    && self
                        .view
                        .libraries
                        .iter()
                        .find(|library| library.store == account.store)
                        .map(|library| {
                            (library.fetched == 0 && library.entries.is_empty())
                                || self.refresh_at.map_or(false, |at| library.fetched < at)
                        })
                        .unwrap_or(true)
            });
            let due = self
                .settle
                .map(|since| since.elapsed() >= Duration::from_secs(2))
                .unwrap_or(false);
            if !pending {
                self.settle = None;
                self.settle_tries = 0;
                self.refresh_at = None;
            } else if due && self.rx.is_none() && self.settle_tries < 300 {
                self.settle = Some(std::time::Instant::now());
                self.settle_tries += 1;
                self.spawn(client, self.selected, true, Req::Stores { refresh: false, only: Vec::new() });
            }
        }
        // Une réponse reçue — trois comptes, ou un refus à afficher — clôt la
        // relance : sans cette borne, un daemon qui échoue à chaque coup
        // relancerait une demande à chaque image.
        let settled =
            self.view.accounts.len() >= Store::EXTERNAL.len() || !self.error.is_empty();
        if self.rx.is_some() || (self.asked && settled) {
            return;
        }
        self.asked = true;
        self.spawn(client, self.selected, true, Req::Stores { refresh: false, only: Vec::new() });
    }

    /// Demande en plus la relecture des bibliothèques : la page Bibliothèque en
    /// a besoin pour montrer les jeux possédés, et le daemon ne sert ensuite
    /// que son cache. Une seule lecture réseau par session.
    pub(crate) fn ensure_games(&mut self, client: &Client) {
        // La relecture réseau attend que la vue du cache soit arrivée : elle
        // occupe l'unique échange avec le daemon, et sur une grande
        // bibliothèque elle prend des minutes — le panneau des comptes
        // resterait alors sans état ni lien d'autorisation.
        if self.asked_games
            || self.rx.is_some()
            || self.view.accounts.len() < Store::EXTERNAL.len()
        {
            return;
        }
        self.asked_games = true;
        self.spawn(client, self.selected, true, Req::Stores { refresh: true, only: Vec::new() });
    }

    /// La première vue des comptes est arrivée, leur relecture réseau est
    /// terminée et aucune lecture n'est en vol : le contrôle des nouveaux
    /// jeux peut comparer les bibliothèques au dernier lancement.
    pub(crate) fn ready(&self) -> bool {
        self.asked
            && self.asked_games
            && self.rx.is_none()
            && self.view.accounts.len() >= Store::EXTERNAL.len()
    }

    /// Vrai une seule fois quand une réponse a remplacé la vue. L'appelant
    /// invalide alors la liste fusionnée, quelle que soit la page ouverte.
    pub(crate) fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }

    /// Le daemon a été remplacé : sa vue peut venir d'un binaire précédent, on
    /// la redemande au suivant.
    pub(crate) fn forget(&mut self) {
        self.asked = false;
        self.asked_games = false;
    }

    fn spawn(&mut self, client: &Client, store: Store, quiet: bool, request: Req) {
        if self.rx.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.rx = Some((store, quiet, rx));
        let worker = client.detached();
        let timeout = store_request_timeout(&request);
        // Le fil qui répond réveille l'interface : la peinture n'a plus besoin
        // d'être relancée toutes les 250 ms pendant une lecture qui dure.
        let wake = self.ctx.clone();
        std::thread::spawn(move || {
            let reply = worker
                .request_with_timeout(&request, timeout)
                .map_err(|error| error.to_string())
                .and_then(|envelope| {
                    if envelope.ok {
                        envelope
                            .into_data::<StoresView>()
                            .map_err(|error| error.to_string())
                    } else {
                        Err(envelope
                            .error
                            .unwrap_or_else(|| lapinux_core::i18n::t("stores.unknown_error").into()))
                    }
                });
            let _ = tx.send((store, reply));
            if let Some(ctx) = &wake {
                ctx.request_repaint();
            }
        });
    }

    /// Relecture choisie dans la fenêtre de synchronisation : seules les
    /// boutiques cochées sont relues, les autres gardent leur cache.
    pub(crate) fn sync_stores(&mut self, client: &Client, only: Vec<Store>) {
        if only.is_empty() {
            return;
        }
        self.asked_games = true;
        self.refresh_at = Some(lapinux_core::util::now_unix());
        self.spawn_user(client, self.selected, false, Req::Stores { refresh: true, only });
    }

    /// Une demande venue d'un geste de l'utilisateur : si l'échange est pris
    /// par une lecture qui peut durer des minutes, elle attend son tour au
    /// lieu d'être jetée en silence. Les sondages de fond, eux, gardent
    /// `spawn` : ils ne doivent jamais déloger une déconnexion en attente.
    fn spawn_user(&mut self, client: &Client, store: Store, quiet: bool, request: Req) {
        if self.rx.is_some() {
            self.queued = Some((store, quiet, request, client.clone()));
            return;
        }
        self.spawn(client, store, quiet, request);
    }

    /// Lit la réponse en cours ; rend le message à notifier, s'il y en a un.
    pub(crate) fn poll(&mut self, ctx: &egui::Context) -> Option<String> {
        self.ctx = Some(ctx.clone());
        // Une demande laissée en attente part dès que l'échange est libre.
        if self.rx.is_none() {
            if let Some((store, quiet, request, client)) = self.queued.take() {
                self.spawn_user(&client, store, quiet, request);
            }
        }
        if self.settle.is_some() {
            // La relecture est relancée par `ensure` à son rythme : une image
            // par seconde suffit à laisser passer ce rendez-vous.
            ctx.request_repaint_after(Duration::from_secs(1));
        }
        let started = self.install.as_ref().map(|(_, rx)| rx.try_recv());
        match started {
            Some(Ok(result)) => {
                self.install = None;
                return Some(match result {
                    Ok(message) => message,
                    Err(error) => error,
                });
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => self.install = None,
            Some(Err(mpsc::TryRecvError::Empty)) => {
                // Filet de sécurité : le fil de fond réveille normalement
                // l'interface dès sa réponse.
                ctx.request_repaint_after(Duration::from_secs(1));
            }
            None => {}
        }
        let received = self.rx.as_ref().map(|(_, quiet, rx)| (*quiet, rx.try_recv()));
        match received {
            Some((quiet, Ok((store, Ok(view))))) => {
                self.view = view;
                self.rx = None;
                // Même silencieuse, une nouvelle vue peut contenir des jeux
                // ajoutés : la liste fusionnée ne doit pas rester en cache.
                // `library_checked` est relancé par le contrôle de nouveaux
                // jeux, pas ici, pour ne pas réécrire la photographie à chaque
                // lecture de la page Comptes.
                self.changed = true;
                self.error.remove(&store);
                // Une boutique connectée sans bibliothèque lue : sa relecture
                // tourne en tâche de fond côté daemon, la vue suivra.
                let pending = self.view.accounts.iter().any(|account| {
                    account.connected
                        && self
                            .view
                            .libraries
                            .iter()
                            .find(|library| library.store == account.store)
                            .map(|library| {
                            (library.fetched == 0 && library.entries.is_empty())
                                || self.refresh_at.map_or(false, |at| library.fetched < at)
                        })
                            .unwrap_or(true)
                });
                if pending {
                    if self.settle.is_none() {
                        self.settle_tries = 0;
                    }
                    self.settle = Some(std::time::Instant::now());
                } else {
                    self.settle = None;
                    self.settle_tries = 0;
                }
                (!quiet).then(|| lapinux_core::i18n::t_args("stores.up_to_date", &[("store", store.label())]))
            }
            Some((_, Ok((store, Err(error))))) => {
                self.rx = None;
                self.error.insert(store, error.clone());
                Some(lapinux_core::i18n::t_args("stores.error", &[("store", store.label()), ("error", &error.to_string())]))
            }
            Some((_, Err(mpsc::TryRecvError::Disconnected))) => {
                self.rx = None;
                None
            }
            Some((_, Err(mpsc::TryRecvError::Empty))) => {
                // La lecture des bibliothèques passe par le réseau : le fil qui
                // répond réveille l'écran, ce délai n'est qu'un filet.
                ctx.request_repaint_after(Duration::from_secs(1));
                None
            }
            None => None,
        }
    }

    /// Liste des boutiques, à gauche : l'état du compte se lit au point.
    pub(crate) fn list(
        &mut self,
        ui: &mut egui::Ui,
        steam_connected: bool,
        steam_name: &str,
        steamgriddb_ready: bool,
    ) {
        let width = ui.available_width();
        ui.set_width(width);
        for store in Store::ALL {
            let account = self.account(store);
            let connected = if store == Store::Steam {
                steam_connected
            } else {
                account.connected
            };
            let _ = steam_name;
            let selected = self.selected == store && !self.steamgriddb_selected;
            // Les lignes respirent : une liste de boutiques collée ne se lit
            // pas, et la marque a besoin d'air autour d'elle.
            ui.add_space(8.0);
            let inner = egui::Frame::none()
                .fill(if selected { CARD() } else { PANEL() })
                .stroke(egui::Stroke::new(
                    1.0_f32,
                    if selected { BORDER() } else { PANEL() },
                ))
                .rounding(egui::Rounding::same(10.0))
                .inner_margin(egui::Margin::symmetric(13.0, 12.0))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        // Les marques restent blanches : c'est la pastille du
                        // statut qui dit si le compte est connecté. Un logo
                        // éteint se lisait comme un logo cassé.
                        let ink = if selected || connected { TEXT() } else { ACCENT() };
                        // La marque garde son lettrage : à cette taille, « EPIC
                        // GAMES » et « gog.com » se lisent, là où une
                        // silhouette réduite ne disait plus rien. En dessous de
                        // quarante-huit pixels, les mots du logo deviennent
                        // une bouillie grise.
                        crate::paint_store_mark(ui, store, 48.0, ink);
                        ui.add_space(10.0);
                        ui.label(
                            egui::RichText::new(store.label())
                                .size(12.5)
                                .color(if selected { TEXT() } else { TEXT_DIM() }),
                        );
                        // Le nom du compte reste hors de la liste : un point
                        // vert dit « connecté », un point rouge « non ».
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                crate::status_dot(
                                    ui,
                                    if connected { OK } else { DANGER },
                                );
                            },
                        );
                    });
                });
            let response = ui.interact(
                inner.response.rect,
                ui.id().with(("store-row", store.key())),
                egui::Sense::click(),
            );
            if response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if response.clicked() {
                self.selected = store;
                self.steamgriddb_selected = false;
            }
        }
        self.steamgriddb_row(ui, steamgriddb_ready);
    }

    /// Ligne SteamGridDB, après les boutiques : un service d'illustrations,
    /// avec sa propre clé.
    fn steamgriddb_row(&mut self, ui: &mut egui::Ui, ready: bool) {
        let selected = self.steamgriddb_selected;
        ui.add_space(8.0);
        let inner = egui::Frame::none()
            .fill(if selected { CARD() } else { PANEL() })
            .stroke(egui::Stroke::new(
                1.0_f32,
                if selected { BORDER() } else { PANEL() },
            ))
            .rounding(egui::Rounding::same(10.0))
            .inner_margin(egui::Margin::symmetric(13.0, 12.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    // Pas de marque de boutique : un carré arrondi portant
                    // « SGDB », de la même taille que les marques voisines
                    // pour que toutes les lignes de comptes s'alignent.
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(48.0, 48.0), egui::Sense::hover());
                    ui.painter().rect_filled(
                        rect,
                        egui::Rounding::same(8.0),
                        egui::Color32::from_white_alpha(18),
                    );
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "SGDB",
                        egui::FontId::proportional(12.0),
                        if selected || ready { TEXT() } else { ACCENT() },
                    );
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new("SteamGridDB")
                            .size(12.5)
                            .color(if selected { TEXT() } else { TEXT_DIM() }),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        crate::status_dot(ui, if ready { OK } else { DANGER });
                    });
                });
            });
        let response = ui.interact(
            inner.response.rect,
            ui.id().with("store-row-steamgriddb"),
            egui::Sense::click(),
        );
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if response.clicked() {
            self.steamgriddb_selected = true;
        }
    }

    /// Panneau de la boutique choisie : compte puis bibliothèque.
    pub(crate) fn panel(&mut self, ui: &mut egui::Ui, client: &Client) -> Option<String> {
        let store = self.selected;
        if store == Store::Steam {
            return None;
        }
        let mut message = None;
        message = message.or(self.account_card(ui, client, store));
        message = message.or(self.library_card(ui, client, store));
        message
    }

    fn account_card(&mut self, ui: &mut egui::Ui, client: &Client, store: Store) -> Option<String> {
        let account = self.account(store);
        let busy = self.busy(store);
        let caps = store.capabilities();
        let subtitle = match store {
            Store::Epic => {
                lapinux_core::i18n::t("stores.subtitle.epic")
            }
            Store::Gog => {
                lapinux_core::i18n::t("stores.subtitle.gog")
            }
            Store::Itch => lapinux_core::i18n::t("stores.subtitle.itch"),
            Store::Steam => lapinux_core::i18n::t("stores.subtitle.steam"),
        };
        let mut refresh = false;
        let mut reload = false;
        let mut logout = false;
        let mut connect = false;
        let connected = account.connected;
        let account_name = account.account.clone();
        let note = account.note.clone();
        let hint = account.login_hint.clone();
        let url = account.login_url.clone();
        let error = self.error.get(&store).cloned();
        let is_itch = store == Store::Itch;
        settings_card(ui, store.label(), subtitle, |ui| {
            ui.horizontal(|ui| {
                // Même règle que la liste : la marque garde son blanc, l'état
                // du compte se lit sur le point et le libellé.
                crate::paint_store_mark(ui, store, 42.0, TEXT());
                ui.add_space(8.0);
                status_dot(ui, if connected { OK } else { TEXT_DIM() });
                ui.add_space(2.0);
                ui.label(
                    egui::RichText::new(if connected {
                        if account_name.trim().is_empty() {
                            lapinux_core::i18n::t("stores.connected").to_string()
                        } else {
                            account_name.clone()
                        }
                    } else {
                        lapinux_core::i18n::t("stores.disconnected").to_string()
                    })
                    .size(13.0)
                    .strong(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if connected {
                        if action_button(ui, lapinux_core::i18n::t("stores.logout"), ActionTone::Quiet).clicked() {
                            logout = true;
                        }
                        if action_button(ui, lapinux_core::i18n::t("stores.reload"), ActionTone::Quiet).clicked() {
                            refresh = true;
                        }
                    } else if action_button(ui, lapinux_core::i18n::t("stores.refresh"), ActionTone::Quiet).clicked() {
                        // Une réponse perdue (daemon remplacé, délai) laissait
                        // le panneau sans lien d'autorisation : ce bouton
                        // redemande l'état, sans toucher au réseau.
                        reload = true;
                    }
                });
            });
            if !note.trim().is_empty() {
                ui.add_space(2.0);
                muted(ui, note.clone());
            }
            ui.add_space(12.0);
            // Ce qui manque s'explique au survol, boutique par boutique.
            let missing = match store {
                Store::Itch => lapinux_core::i18n::t("stores.missing.itch"),
                Store::Epic => {
                    lapinux_core::i18n::t("stores.missing.epic")
                }
                _ => lapinux_core::i18n::t("stores.missing.unsupported"),
            };
            ui.horizontal_wrapped(|ui| {
                capability_chip(ui, lapinux_core::i18n::t("stores.capability.library"), caps.library, missing);
                capability_chip(ui, lapinux_core::i18n::t("stores.capability.install"), caps.install, missing);
                capability_chip(ui, lapinux_core::i18n::t("stores.capability.cloud"), caps.cloud, missing);
                capability_chip(ui, lapinux_core::i18n::t("stores.capability.achievements"), caps.achievements, missing);
            });
            if !connected {
                ui.add_space(12.0);
                if !hint.trim().is_empty() {
                    muted(ui, hint.clone());
                }
                if !url.trim().is_empty() {
                    ui.add_space(6.0);
                    // Un bouton plutôt qu'un lien : c'est le seul chemin vers
                    // l'autorisation, il ne doit pas se perdre dans le texte.
                    if action_button(
                        ui,
                        if is_itch {
                            lapinux_core::i18n::t("stores.button.itch_key").to_string()
                        } else {
                            lapinux_core::i18n::t_args("stores.button.login", &[("store", store.label())])
                        },
                        ActionTone::Light,
                    )
                    .clicked()
                    {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(url.clone()));
                    }
                    ui.add_space(4.0);
                    muted(
                        ui,
                        if is_itch {
                            lapinux_core::i18n::t("stores.hint.itch")
                        } else {
                            lapinux_core::i18n::t("stores.hint.login")
                        },
                    );
                }
                ui.add_space(8.0);
                let field = self.token.entry(store).or_default();
                ui.add(
                    egui::TextEdit::singleline(field)
                        .hint_text(if is_itch {
                            lapinux_core::i18n::t("stores.placeholder.itch")
                        } else {
                            lapinux_core::i18n::t("stores.placeholder.code")
                        })
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let ready = self
                        .token
                        .get(&store)
                        .map(|token| !token.trim().is_empty())
                        .unwrap_or(false);
                    if ui
                        .add_enabled(
                            ready && !busy,
                            egui::Button::new(
                                egui::RichText::new(if busy {
                                    lapinux_core::i18n::t("stores.connecting")
                                } else {
                                    lapinux_core::i18n::t("stores.connect")
                                })
                                .size(12.5)
                                .strong()
                                .color(BACKGROUND()),
                            )
                            .fill(TEXT())
                            .rounding(egui::Rounding::same(8.0))
                            .min_size(egui::vec2(0.0, 32.0)),
                        )
                        .clicked()
                    {
                        connect = true;
                    }
                    if busy {
                        ui.spinner();
                    }
                });
            }
            if let Some(error) = &error {
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(error)
                        .size(11.0)
                        .color(DANGER),
                );
            }
        });
        if reload {
            self.forget();
            self.spawn_user(client, store, true, Req::Stores { refresh: false, only: Vec::new() });
            return None;
        }
        if logout {
            self.spawn_user(client, store, false, Req::StoreLogout { store });
            return Some(lapinux_core::i18n::t_args("stores.account_removed", &[("store", store.label())]));
        }
        if refresh {
            self.refresh_at = Some(lapinux_core::util::now_unix());
            self.spawn_user(client, store, false, Req::Stores { refresh: true, only: Vec::new() });
            return Some(lapinux_core::i18n::t_args("stores.reloading", &[("store", store.label())]));
        }
        if connect {
            let token = self.token.get(&store).cloned().unwrap_or_default();
            let account = if connected { account_name } else { String::new() };
            self.spawn_user(
                client,
                store,
                false,
                Req::StoreLogin {
                    store,
                    token,
                    username: account,
                },
            );
        }
        None
    }

    fn library_card(&mut self, ui: &mut egui::Ui, _client: &Client, store: Store) -> Option<String> {
        let library = self.library(store);
        let account = self.account(store);
        let count = library.entries.len();
        let installed = library
            .entries
            .iter()
            .filter(|entry| entry.installed)
            .count();
        let subtitle = if account.connected {
            if library.fetched > 0 {
                lapinux_core::i18n::t_args(
                    "stores.library.reloaded",
                    &[("date", &lapinux_core::util::format_datetime(library.fetched))],
                )
            } else {
                lapinux_core::i18n::t("stores.library.never").to_string()
            }
        } else {
            lapinux_core::i18n::t("stores.library.connect").to_string()
        };
        settings_card(ui, lapinux_core::i18n::t("stores.library.title"), &subtitle, |ui| {
            if let Some(error) = &library.error {
                ui.label(egui::RichText::new(error).size(11.0).color(WARN));
                ui.add_space(6.0);
            }
            // Seulement le nombre de jeux : la liste complète vit dans la
            // Bibliothèque principale, pas dans les réglages du compte.
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(count.to_string())
                        .size(28.0)
                        .strong()
                        .color(TEXT()),
                );
                ui.add_space(6.0);
                ui.vertical(|ui| {
                    ui.add_space(5.0);
                    ui.label(
                        egui::RichText::new(if count == 1 {
                            lapinux_core::i18n::t("stores.library.games_one")
                        } else {
                            lapinux_core::i18n::t("stores.library.games_many")
                        })
                            .size(12.0)
                            .color(TEXT_DIM()),
                    );
                });
                ui.with_layout(
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        muted(
                            ui,
                            if installed > 1 {
                                lapinux_core::i18n::t_args(
                                    "stores.library.installed_many",
                                    &[("count", &installed.to_string())],
                                )
                            } else {
                                lapinux_core::i18n::t_args(
                                    "stores.library.installed_one",
                                    &[("count", &installed.to_string())],
                                )
                            },
                        );
                    },
                );
            });
            if !account.connected {
                ui.add_space(4.0);
                muted(ui, lapinux_core::i18n::t("stores.library.empty"));
            } else if count == 0 && library.fetched == 0 {
                ui.add_space(4.0);
                muted(
                    ui,
                    lapinux_core::i18n::t("stores.library.unread"),
                );
            }
        });
        None
    }

    /// Lance l'installation d'un jeu de boutique : le daemon prépare le plan,
    /// le téléchargement vit ensuite dans la page Téléchargements.
    pub(crate) fn start_install(
        &mut self,
        client: &Client,
        store: Store,
        store_id: &str,
        platform: Option<String>,
    ) -> Option<String> {
        if self.install.is_some() {
            return Some(lapinux_core::i18n::t_args("stores.install.preparing", &[("store", store.label())]));
        }
        let (tx, rx) = mpsc::channel();
        self.install = Some((store, rx));
        let worker = client.detached();
        let store_id = store_id.to_string();
        let label = platform
            .as_deref()
            .map(platform_label)
            .unwrap_or(lapinux_core::i18n::t("stores.install.published_version"))
            .to_string();
        std::thread::spawn(move || {
            let result = worker
                .request_with_timeout(
                    &Req::StoreInstall {
                        store,
                        store_id,
                        platform,
                    },
                    Duration::from_secs(120),
                )
                .map_err(|error| lapinux_core::i18n::t_args("stores.error", &[("store", store.label()), ("error", &error.to_string())]))
                .and_then(|envelope| {
                    if envelope.ok {
                        Ok(lapinux_core::i18n::t_args("stores.install.started", &[("platform", &label)]))
                    } else {
                        Err(format!(
                            "{} : {}",
                            store.label(),
                            envelope
                                .error
                                .unwrap_or_else(|| lapinux_core::i18n::t("stores.install.refused").into())
                        ))
                    }
                });
            let _ = tx.send(result);
        });
        Some(lapinux_core::i18n::t_args(
            "stores.install.preparing_download",
            &[("store", store.label())],
        ))
    }
}

pub(crate) fn platform_label(platform: &str) -> &str {
    match platform {
        "linux" => "Linux",
        "windows" => "Windows",
        "macos" => "macOS",
        other => other,
    }
}

/// Ce que la boutique sait faire, en clair : vert quand la fonction existe,
/// gris quand elle n'est pas exposée.
/// Pastille d'une capacité. Éteinte, elle dit **pourquoi** au survol : une
/// pastille grise sans raison laisse croire à une panne plutôt qu'à une
/// fonction qui n'existe pas encore.
pub(crate) fn capability_chip(ui: &mut egui::Ui, text: &str, available: bool, missing: &str) {
    let (fill, ink) = if available {
        (egui::Color32::from_rgb(36, 58, 44), OK)
    } else {
        (PANEL(), TEXT_DIM())
    };
    let response = egui::Frame::none()
        .fill(fill)
        .rounding(egui::Rounding::same(6.0))
        .inner_margin(egui::Margin::symmetric(8.0, 3.0))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).size(10.5).color(ink));
        })
        .response;
    if !available {
        response.on_hover_text(missing);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_store_refresh_gets_time_to_finish() {
        let refresh = Req::Stores {
            refresh: true,
            only: Vec::new(),
        };
        assert_eq!(store_request_timeout(&refresh), STORES_REFRESH_TIMEOUT);
        assert!(STORES_REFRESH_TIMEOUT >= Duration::from_secs(300));

        let cached = Req::Stores {
            refresh: false,
            only: Vec::new(),
        };
        assert_eq!(store_request_timeout(&cached), STORES_CACHE_TIMEOUT);
    }
}
