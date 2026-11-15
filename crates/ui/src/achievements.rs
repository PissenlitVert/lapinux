//! Succès Steam d'un jeu, lus par la session du compte.
//!
//! Le daemon interroge le protocole : `Player.GetGameAchievements` publie le
//! schéma du jeu (nom, description, icône, succès caché) et `ClientGetUserStats`
//! rend l'état du compte (débloqué, date). Aucune clé Web API n'est nécessaire :
//! la session Steam enregistrée suffit. Les icônes sont ensuite téléchargées en
//! tâche de fond et gardées sous forme de textures.

use super::*;

/// Budget d'une lecture complète (connexion CM, schéma, état du compte).
/// Au-delà, le bloc abandonne et propose de relancer.
const LOAD_BUDGET: Duration = Duration::from_secs(45);
fn timeout_error() -> &'static str {
    lapinux_core::i18n::t("achievements.timeout")
}
/// Un téléchargement d'icône sans réponse au bout de huit secondes est perdu :
/// la case grise reste pour l'instant, puis un nouvel essai part après
/// `ICON_RETRY`.
const ICON_TIMEOUT: Duration = Duration::from_secs(8);
/// Délai avant de retenter une icône qui a échoué : une coupure passagère ne
/// doit pas laisser une case grise jusqu'au redémarrage.
const ICON_RETRY: Duration = Duration::from_secs(30);
/// Icônes téléchargées en parallèle : quatre requêtes remplissent la bande
/// aussi vite qu'une seule en tirerait le premier tiers.
const ICON_WORKERS: usize = 4;
/// Icônes téléchargées au plus par jeu : trois rangées suffisent à remplir la
/// fiche, et cela évite de tirer toute la liste sur un jeu à 200 succès.
const MAX_ICONS: usize = 30;
/// Hauteur d'une ligne de succès mise en avant, sous la bande d'icônes.
const HIGHLIGHT_ROW: f32 = 20.0;
/// Trois lignes au plus : la fiche repliée n'en montre pas davantage sans
/// devenir une liste, et la liste complète s'ouvre d'un clic.
const HIGHLIGHT_ROWS: usize = 3;
/// Hauteur du libellé qui ouvre ces lignes.
const HIGHLIGHT_HEADING: f32 = 14.0;

/// Un succès du jeu, débloqué ou non.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Achievement {
    pub api_name: String,
    pub name: String,
    pub description: String,
    pub icon: String,
    /// Succès caché par le jeu : son nom n'a de sens qu'une fois débloqué.
    pub hidden: bool,
    pub unlocked: bool,
    /// Date de déblocage, en secondes Unix (nulle pour un succès verrouillé ou
    /// débloqué hors ligne par Steam).
    pub unlock_time: Option<i64>,
    /// Part des joueurs qui l'ont obtenu, en pourcentage.
    pub percent: Option<f32>,
}

/// Les succès d'un jeu, dans l'ordre où Steam les publie.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Achievements {
    pub list: Vec<Achievement>,
    /// Minutes jouées par le compte, lues avec les succès.
    pub play_minutes: Option<u64>,
}

impl Achievements {
    pub fn unlocked(&self) -> usize {
        self.list.iter().filter(|entry| entry.unlocked).count()
    }

    pub fn total(&self) -> usize {
        self.list.len()
    }

    /// Taux de complétion du jeu, en pourcentage arrondi.
    pub fn completion(&self) -> f32 {
        self.fraction() * 100.0
    }

    /// Tous les succès sont débloqués : le jeu est « platine ».
    pub fn platinum(&self) -> bool {
        let total = self.list.len();
        total > 0 && self.unlocked() == total
    }

    /// Part des succès débloqués, entre 0 et 1.
    pub fn fraction(&self) -> f32 {
        if self.list.is_empty() {
            0.0
        } else {
            self.unlocked() as f32 / self.list.len() as f32
        }
    }

    /// Débloqués d'abord, du plus récent au plus ancien, puis les verrouillés
    /// dans l'ordre du jeu : la bande d'icônes montre ce qui vient d'arriver.
    ///
    /// L'ordre est calculé une fois au chargement, puis gardé en indices : la
    /// fiche le relit à chaque image sans retrier la liste.
    pub fn order(&self) -> Vec<usize> {
        let mut unlocked: Vec<usize> = (0..self.list.len())
            .filter(|index| self.list[*index].unlocked)
            .collect();
        unlocked.sort_by(|a, b| {
            self.list[*b]
                .unlock_time
                .unwrap_or(0)
                .cmp(&self.list[*a].unlock_time.unwrap_or(0))
        });
        unlocked.extend((0..self.list.len()).filter(|index| !self.list[*index].unlocked));
        unlocked
    }

    #[cfg(test)]
    pub fn ordered(&self) -> Vec<&Achievement> {
        self.order()
            .into_iter()
            .map(|index| &self.list[index])
            .collect()
    }
}

/// Range la lecture du protocole dans l'état de la fiche. L'ordre est celui
/// que Steam publie : le schéma du jeu, déjà trié par identifiant.
fn assemble(read: SteamAchievements) -> Achievements {
    Achievements {
        play_minutes: read.play_minutes,
        list: read
            .items
            .into_iter()
            .map(|item| Achievement {
                api_name: item.api_name,
                name: item.name,
                description: item.description,
                icon: item.icon,
                hidden: item.hidden,
                unlocked: item.achieved,
                unlock_time: item.unlock_time,
                percent: item.percent,
            })
            .collect(),
    }
}

/// Lit les succès d'un jeu par le daemon : la session Steam qu'il détient
/// ouvre le schéma du jeu et l'état du compte, sans clé Web API.
fn fetch(client: &Client, appid: u32) -> Result<Achievements, String> {
    let envelope = client
        .request(&Req::GameAchievements { appid })
        .map_err(|error| error.to_string())?;
    let read: SteamAchievements = envelope.into_data().map_err(|error| error.to_string())?;
    if read.items.is_empty() {
        return Err(empty_message(&read));
    }
    Ok(assemble(read))
}

/// Ce qu'il faut dire quand la lecture n'a rien rendu : la note du protocole
/// (session refusée, schéma absent, jeu sans succès) est plus utile qu'un
/// message générique.
fn empty_message(read: &SteamAchievements) -> String {
    match read.notes.first() {
        Some(note) => note.clone(),
        None => lapinux_core::i18n::t("achievements.none_published").to_string(),
    }
}

enum Msg {
    Loaded(Result<Achievements, String>),
    Icon { url: String, bytes: Vec<u8> },
    IconFailed { url: String },
}

/// État et peinture du bloc « Succès » de la fiche.
pub(crate) struct Sheet {
    /// Jeu dont les succès sont chargés : redemander le même est inutile,
    /// changer de jeu relance la lecture.
    loaded_for: Option<u32>,
    data: Option<Achievements>,
    /// Ordre d'affichage des succès, calculé une fois au chargement.
    order: Vec<usize>,
    error: Option<String>,
    loading: bool,
    /// Début de la lecture en cours : un appel qui traîne ne peut pas figer la
    /// roue de chargement indéfiniment.
    started: Option<Instant>,
    rx: Option<mpsc::Receiver<Msg>>,
    tx: Option<mpsc::Sender<Msg>>,
    icons: HashMap<String, egui::TextureHandle>,
    /// Icônes en cours de téléchargement : une seule demande par adresse.
    pending: HashSet<String>,
    /// Icônes dont le dernier téléchargement a échoué : nouvel essai après
    /// un délai, sans marteler le CDN à chaque image.
    failed: HashMap<String, Instant>,
    /// Nom du jeu lu, pour le titre de la liste complète.
    title: String,
    /// Boutique d'où viennent les succès : Steam, ou celle qui les publie.
    source: String,
    /// Trophée du platine, chargé la première fois qu'un jeu est terminé.
    platinum_art: Option<crate::Logo>,
    /// Jeu et client du panneau : le bouton de test demande un bandeau au
    /// daemon sans passer par la fiche.
    appid: Option<u32>,
    client: Option<Client>,
    /// Réponse du daemon au test, en attente puis affichée quelques secondes.
    toast_rx: Option<mpsc::Receiver<Result<String, String>>>,
    toast_note: Option<(String, bool, Instant)>,
    /// La liste complète est ouverte : la bande n'en montre qu'un extrait.
    pub modal: bool,
}

impl Default for Sheet {
    fn default() -> Self {
        Self {
            loaded_for: None,
            data: None,
            order: Vec::new(),
            error: None,
            loading: false,
            started: None,
            rx: None,
            tx: None,
            icons: HashMap::new(),
            pending: HashSet::new(),
            failed: HashMap::new(),
            title: String::new(),
            source: "Steam".into(),
            modal: false,
            platinum_art: None,
            appid: None,
            client: None,
            toast_rx: None,
            toast_note: None,
        }
    }
}

impl Sheet {
    /// Minutes jouées du jeu affiché, lues avec ses succès : l'embed du jeu
    /// montre le temps de jeu du compte, même pour un jeu non installé.
    pub fn play_minutes(&self) -> Option<u64> {
        self.data
            .as_ref()
            .and_then(|data| data.play_minutes)
            .filter(|minutes| *minutes > 0)
    }

    /// Oublie le jeu lu : la prochaine fiche redemande ses succès. Les icônes
    /// déjà téléchargées restent en mémoire.
    pub fn forget(&mut self) {
        self.reset();
    }

    fn reset(&mut self) {
        self.loaded_for = None;
        self.data = None;
        self.order.clear();
        self.error = None;
        self.loading = false;
        self.started = None;
        self.rx = None;
        self.tx = None;
        // Les tâches en cours s'arrêtent avec le canal : leurs icônes ne
        // seront plus jamais livrées, il faut redemander celles qui manquent.
        self.pending.clear();
        self.failed.clear();
        // Les textures restent : un jeu revu ne re-télécharge pas ses icônes.
    }

    /// Prépare les succès du jeu affiché. Sans session Steam, ou pour un jeu
    /// hors du compte, rien n'est demandé au daemon.
    pub fn ensure(
        &mut self,
        ctx: &egui::Context,
        client: &Client,
        appid: u32,
        owned: bool,
        session: bool,
    ) {
        if !owned || !session {
            if self.loaded_for.is_some() {
                self.reset();
            }
            return;
        }
        if self.loaded_for == Some(appid) {
            return;
        }
        self.reset();
        self.loaded_for = Some(appid);
        self.appid = Some(appid);
        // Le client est gardé pour le bouton de test : il ne dépend pas de la
        // lecture en cours et survit à un changement de jeu.
        self.client = Some(client.detached());
        self.loading = true;
        self.started = Some(Instant::now());
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        self.tx = Some(tx.clone());
        // Le client est cloné : la tâche de fond ne touche pas l'interface.
        let client = client.detached();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = fetch(&client, appid);
            if tx.send(Msg::Loaded(result)).is_ok() {
                ctx.request_repaint();
            }
        });
    }

    /// Relève les réponses des tâches de fond : jeu chargé, puis icônes. Une
    /// lecture qui dépasse son budget est abandonnée, avec un message clair.
    pub fn poll(&mut self, ctx: &egui::Context) {
        if self.loading
            && self
                .started
                .is_some_and(|started| started.elapsed() > LOAD_BUDGET)
        {
            self.loading = false;
            self.error = Some(timeout_error().to_string());
        }
        while let Some(rx) = &self.rx {
            let Ok(message) = rx.try_recv() else {
                break;
            };
            match message {
                Msg::Loaded(Ok(data)) => {
                    self.loading = false;
                    self.error = None;
                    self.order = data.order();
                    self.data = Some(data);
                }
                Msg::Loaded(Err(error)) => {
                    self.loading = false;
                    self.error = Some(error);
                }
                Msg::Icon { url, bytes } => {
                    self.pending.remove(&url);
                    if let Some(texture) = texture_from_bytes(ctx, &url, &bytes) {
                        self.failed.remove(&url);
                        self.icons.insert(url, texture);
                    } else {
                        self.failed.insert(url, Instant::now());
                        ctx.request_repaint_after(ICON_RETRY);
                    }
                }
                Msg::IconFailed { url } => {
                    self.pending.remove(&url);
                    self.failed.insert(url, Instant::now());
                    ctx.request_repaint_after(ICON_RETRY);
                }
            }
        }
    }

    /// Télécharge les icônes demandées en quelques tâches parallèles, dans
    /// l'ordre d'affichage : la bande se remplit quatre fois plus vite.
    fn start_icons(&mut self, urls: Vec<String>, ctx: &egui::Context) {
        let now = Instant::now();
        self.failed
            .retain(|_, failed_at| now.duration_since(*failed_at) < ICON_RETRY);
        let wanted: Vec<String> = urls
            .into_iter()
            .filter(|url| self.icon_wanted(url, now))
            .take(MAX_ICONS)
            .collect();
        if wanted.is_empty() {
            return;
        }
        let Some(tx) = self.tx.clone() else {
            return;
        };
        for url in &wanted {
            self.pending.insert(url.clone());
        }
        let queue = Arc::new(std::sync::Mutex::new(wanted));
        let workers = ICON_WORKERS.min(queue.lock().map(|queue| queue.len()).unwrap_or(0));
        for _ in 0..workers {
            let queue = Arc::clone(&queue);
            let tx = tx.clone();
            let ctx = ctx.clone();
            std::thread::spawn(move || loop {
                let next = match queue.lock() {
                    Ok(mut queue) => queue.pop(),
                    // Un verrou empoisonné arrête la tâche : les autres
                    // continuent sur les icônes qui restent.
                    Err(_) => return,
                };
                let Some(url) = next else {
                    return;
                };
                let Some(bytes) = http_bytes(&url) else {
                    if tx.send(Msg::IconFailed { url }).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                    continue;
                };
                if tx.send(Msg::Icon { url, bytes }).is_err() {
                    return;
                }
                ctx.request_repaint();
            });
        }
    }

    /// Une icône absente, pas déjà en téléchargement et dont le dernier échec
    /// date de plus que `ICON_RETRY` peut être redemandée.
    fn icon_wanted(&self, url: &str, now: Instant) -> bool {
        !self.icons.contains_key(url)
            && !self.pending.contains(url)
            && !self
                .failed
                .get(url)
                .is_some_and(|failed_at| now.duration_since(*failed_at) < ICON_RETRY)
    }

    /// Liste complète des succès, ouverte par la tuile « + N » : les débloqués
    /// d'abord, du plus récent au plus ancien, puis tout ce qui reste. Rien
    /// n'est tronqué ici, contrairement à la bande de la fiche. La liste vit
    /// dans le panneau fixe de l'application, comme les Réglages.
    pub fn panel(&mut self, ctx: &egui::Context) {
        if !self.modal {
            return;
        }
        // La réponse du test arrive d'un fil : elle s'affiche quelques
        // secondes à côté du bouton, sans bloquer la liste.
        if let Some(rx) = &self.toast_rx {
            if let Ok(note) = rx.try_recv() {
                self.toast_rx = None;
                self.toast_note = Some(match note {
                    Ok(text) => (text, true, Instant::now()),
                    Err(error) => (error, false, Instant::now()),
                });
            }
        }
        if self
            .toast_note
            .as_ref()
            .is_some_and(|(_, _, at)| at.elapsed() > Duration::from_secs(5))
        {
            self.toast_note = None;
        }
        // Les icônes de la liste entière : la bande n'en avait demandé qu'une
        // poignée. Elles arrivent quatre par quatre, la liste se remplit seule.
        let urls: Vec<String> = match &self.data {
            Some(data) => self
                .order
                .iter()
                .filter_map(|index| data.list.get(*index))
                .map(|achievement| achievement.icon.clone())
                .filter(|url| !url.is_empty())
                .collect(),
            None => {
                self.modal = false;
                return;
            }
        };
        self.start_icons(urls, ctx);
        let Some(data) = &self.data else {
            self.modal = false;
            return;
        };
        let subtitle = lapinux_core::i18n::t_args(
            "achievements.subtitle",
            &[
                ("title", &self.title),
                ("unlocked", &data.unlocked().to_string()),
                ("total", &data.total().to_string()),
                ("percent", &share(data.completion())),
                (
                    "platinum",
                    if data.platinum() {
                        lapinux_core::i18n::t("achievements.platinum_suffix")
                    } else {
                        ""
                    },
                ),
            ],
        );
        let mut reload = false;
        let mut test = false;
        let close = crate::fixed_panel(
            ctx,
            "achievements-panel",
            lapinux_core::i18n::t("achievements.title"),
            &subtitle,
            egui::vec2(660.0, 680.0),
            |ui| {
                ui.horizontal(|ui| {
                    if crate::link_text(
                        ui,
                        &lapinux_core::i18n::t_args(
                            "achievements.reload_from",
                            &[("source", &self.source)],
                        ),
                    )
                    .clicked()
                    {
                        reload = true;
                    }
                    ui.add_space(12.0);
                    if crate::link_text(ui, lapinux_core::i18n::t("achievements.test_notification"))
                        .clicked()
                    {
                        test = true;
                    }
                    if let Some((note, ok, _)) = &self.toast_note {
                        ui.add_space(10.0);
                        ui.label(egui::RichText::new(note).size(10.5).color(if *ok {
                            OK
                        } else {
                            TEXT_DIM()
                        }));
                    }
                });
                ui.add_space(8.0);
                for index in &self.order {
                    let Some(achievement) = data.list.get(*index) else {
                        continue;
                    };
                    ui.horizontal_top(|ui| {
                        let (rect, response) =
                            ui.allocate_exact_size(egui::vec2(34.0, 34.0), egui::Sense::hover());
                        match self.icons.get(&achievement.icon) {
                            Some(texture) => {
                                let tint = if achievement.unlocked {
                                    egui::Color32::WHITE
                                } else {
                                    egui::Color32::from_gray(96)
                                };
                                egui::Image::new((texture.id(), rect.size()))
                                    .rounding(egui::Rounding::same(6.0))
                                    .tint(tint)
                                    .paint_at(ui, rect);
                            }
                            None => {
                                ui.painter().rect_filled(
                                    rect,
                                    egui::Rounding::same(6.0),
                                    CARD_HOVER(),
                                );
                            }
                        }
                        ui.add_space(2.0);
                        let width = (ui.available_width() - 96.0).max(120.0);
                        ui.allocate_ui_with_layout(
                            egui::vec2(width, 34.0),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                ui.set_width(width);
                                ui.label(
                                    egui::RichText::new(&achievement.name)
                                        .size(12.5)
                                        .strong()
                                        .color(if achievement.unlocked {
                                            TEXT()
                                        } else {
                                            TEXT_DIM()
                                        }),
                                );
                                if !achievement.description.is_empty() {
                                    ui.label(
                                        egui::RichText::new(&achievement.description)
                                            .size(11.0)
                                            .color(TEXT_DIM()),
                                    );
                                }
                            },
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                            let (label, color) =
                                match (achievement.unlocked, achievement.unlock_time) {
                                    // L'heure compte : un succès obtenu hier soir
                                    // se lit autrement qu'un succès de 2017.
                                    (true, Some(time)) => {
                                        (lapinux_core::util::format_datetime(time), OK)
                                    }
                                    (true, None) => (
                                        lapinux_core::i18n::t("achievements.unlocked").to_string(),
                                        OK,
                                    ),
                                    (false, _) => (
                                        lapinux_core::i18n::t("achievements.locked").to_string(),
                                        TEXT_DIM(),
                                    ),
                                };
                            ui.label(egui::RichText::new(label).size(11.0).color(color));
                            if let Some(percent) = achievement.percent {
                                ui.label(
                                    egui::RichText::new(format!("{} %", share(percent)))
                                        .size(11.0)
                                        .color(TEXT_DIM()),
                                );
                            }
                        });
                        response.on_hover_text(achievement_tooltip(achievement));
                    });
                    ui.add_space(4.0);
                }
            },
        );
        if close {
            self.modal = false;
        }
        if reload {
            self.forget();
        }
        if test {
            self.test_toast(ctx);
        }
    }

    /// Demande au daemon le bandeau d'un succès du jeu affiché : le dernier
    /// débloqué, avec sa vraie icône et sa vraie rareté. La requête part en
    /// tâche de fond, la liste continue de se peindre.
    fn test_toast(&mut self, ctx: &egui::Context) {
        let (Some(appid), Some(client)) = (self.appid, self.client.as_ref()) else {
            return;
        };
        let client = client.detached();
        let (tx, rx) = mpsc::channel();
        self.toast_rx = Some(rx);
        self.toast_note = Some((
            lapinux_core::i18n::t("achievements.toast_pending").into(),
            true,
            Instant::now(),
        ));
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = match client.request(&Req::AchievementToast {
                appid,
                api_name: String::new(),
            }) {
                Ok(envelope) if envelope.ok => {
                    Ok(lapinux_core::i18n::t("achievements.toast_shown").to_string())
                }
                Ok(envelope) => Err(envelope
                    .error
                    .unwrap_or_else(|| lapinux_core::i18n::t("achievements.toast_refused").into())),
                Err(error) => Err(error.to_string()),
            };
            if tx.send(result).is_ok() {
                ctx.request_repaint();
            }
        });
    }

    /// Bloc « Succès » : le mot, la bande d'icônes à sa droite et le compte au
    /// bord, puis la barre de progression en dessous. Tout tient sur deux
    /// lignes : la bande ne descend plus sous la barre.
    #[allow(clippy::too_many_arguments)]
    pub fn block(
        &mut self,
        ui: &mut egui::Ui,
        name: &str,
        appid: u32,
        owned: bool,
        session: bool,
        client: &Client,
        width: f32,
        // Hauteur que l'encart peut occuper, ou zéro quand la fiche ne lui en
        // réserve aucune : la bande et sa barre restent seules.
        height: f32,
        // Lapin d'attente : la carte se remplit pendant la lecture au lieu de
        // laisser un simple point qui tourne.
        bunny: Option<&crate::launch::Bunny>,
        // Boutique du jeu : Epic et GOG publient aussi leurs succès, sans
        // session Steam ; itch.io n'en a aucun.
        store: lapinux_core::stores::Store,
        // Teinte dominante du jeu : la barre la reprend, comme celle du
        // téléchargement, au lieu d'un vert qui jure avec la jaquette.
        accent: egui::Color32,
    ) {
        let steam = store == lapinux_core::stores::Store::Steam;
        self.source = store.label().to_string();
        // Un jeu de boutique n'a pas besoin de la session Steam : sa propre
        // boutique répond, et le daemon dit ce qui manque le cas échéant.
        self.ensure(ui.ctx(), client, appid, owned || !steam, session || !steam);
        if self.title != name {
            self.title = name.to_string();
        }
        let inner = (width - 20.0).max(80.0);
        egui::Frame::none()
            .fill(CARD())
            .rounding(egui::Rounding::same(10.0))
            .inner_margin(egui::Margin::same(10.0))
            .show(ui, |ui| {
                ui.set_width(inner);
                if height > 0.0 {
                    // L'encart descend jusqu'à la hauteur des autres cartes de
                    // la ligne : la place libre porte les succès mis en avant.
                    ui.set_height((height - 20.0).max(0.0));
                }
                // Repère de la carte : ce qui reste à peindre se mesure à
                // partir d'ici, pas à partir d'une hauteur théorique.
                let top = ui.cursor().top();
                // L'attente occupe la carte entière, comme celle du résumé :
                // titre centré, puis le lapin au milieu de la place qui reste.
                // La bande de succès n'a rien à montrer pendant la lecture, et
                // la garder réservée réduisait le lapin d'une taille à l'autre.
                if self.loading {
                    if let Some(bunny) = bunny {
                        let title_size = crate::text_size(
                            ui,
                            lapinux_core::i18n::t("achievements.title"),
                            TITLE,
                        );
                        let left = ui.available_rect_before_wrap().center().x - title_size.x / 2.0;
                        crate::painted_text_at(
                            ui,
                            egui::pos2(left, top),
                            lapinux_core::i18n::t("achievements.title"),
                            TITLE,
                            TEXT(),
                        );
                        let mut rest = ui
                            .available_rect_before_wrap()
                            .with_min_y(top + title_size.y + 6.0);
                        if height > 0.0 {
                            let bottom = top + (height - 20.0).max(0.0) - 2.0;
                            rest.set_bottom(rest.bottom().max(bottom).max(rest.min.y + 24.0));
                        }
                        crate::launch::waiting_in(
                            ui,
                            rest,
                            bunny,
                            lapinux_core::i18n::t("achievements.loading"),
                        );
                        return;
                    }
                }
                let count = self
                    .data
                    .as_ref()
                    .filter(|data| !data.list.is_empty())
                    .map(|data| {
                        format!(
                            "{} / {} · {} %",
                            data.unlocked(),
                            data.total(),
                            share(data.completion())
                        )
                    })
                    .unwrap_or_default();
                // Les icônes tiennent sur la ligne du mot : elles y sont un
                // peu plus petites que dans la fiche dépliée.
                const SIZE: f32 = 30.0;
                const GAP: f32 = 5.0;
                const TITLE: f32 = 12.5;
                let count_size = if count.is_empty() {
                    egui::Vec2::ZERO
                } else {
                    crate::text_size(ui, &count, 11.0)
                };
                let ready = owned
                    && self.loaded_for.is_some()
                    && !self.loading
                    && self.error.is_none()
                    && self.data.as_ref().is_some_and(|data| !data.list.is_empty());
                let (row, _) =
                    ui.allocate_exact_size(egui::vec2(inner, SIZE + 2.0), egui::Sense::hover());
                // Sans bande d'icônes (lecture, jeu sans succès), le titre se
                // centre sur la carte ; avec la bande, il reste au bord pour
                // lui laisser la largeur nécessaire.
                let title_width =
                    crate::text_size(ui, lapinux_core::i18n::t("achievements.title"), TITLE).x;
                let title_left = if ready {
                    row.left()
                } else {
                    row.center().x - title_width / 2.0
                };
                let title = crate::painted_text_at(
                    ui,
                    egui::pos2(
                        title_left,
                        row.center().y
                            - crate::text_size(
                                ui,
                                lapinux_core::i18n::t("achievements.title"),
                                TITLE,
                            )
                            .y / 2.0,
                    ),
                    lapinux_core::i18n::t("achievements.title"),
                    TITLE,
                    TEXT(),
                );
                // Le trophée du platine se place juste avant le compte : un
                // jeu terminé à 100 % le porte sur sa bande de succès.
                const TROPHY: f32 = 22.0;
                let platinum = self.data.as_ref().is_some_and(Achievements::platinum);
                let mut count_left = row.right();
                let mut band_right = row.right();
                if !count.is_empty() {
                    crate::painted_text_at(
                        ui,
                        egui::pos2(
                            row.right() - count_size.x,
                            row.center().y - count_size.y / 2.0,
                        ),
                        &count,
                        11.0,
                        if platinum { ACCENT() } else { TEXT_DIM() },
                    );
                    count_left = row.right() - count_size.x - 10.0;
                    band_right = row.right() - count_size.x - 6.0;
                }
                if platinum {
                    if self.platinum_art.is_none() {
                        self.platinum_art = crate::Logo::load_bytes(
                            ui.ctx(),
                            "lapinux-platinum",
                            include_bytes!("../../../assets/platinum.png"),
                        );
                    }
                    if let Some(art) = &self.platinum_art {
                        let width = TROPHY * art.aspect;
                        let rect = egui::Rect::from_min_size(
                            egui::pos2(band_right - width, row.center().y - TROPHY / 2.0),
                            egui::vec2(width, TROPHY),
                        );
                        art.draw(ui.painter(), rect.center(), TROPHY, 1.0);
                        ui.interact(
                            rect,
                            ui.id().with("achievements-platinum"),
                            egui::Sense::hover(),
                        )
                        .on_hover_text(lapinux_core::i18n::t("achievements.platinum_tooltip"));
                        count_left = rect.left() - 8.0;
                    }
                }
                // Place de la bande : du mot jusqu'au compte.
                let first = title.right() + 10.0;
                let per_row = (((count_left - first + GAP) / (SIZE + GAP)).floor() as usize).max(0);
                let mut shown = 0usize;
                if ready {
                    if let Some(data) = &self.data {
                        shown = data.list.len().min(per_row);
                    }
                    // La tuile « +N » prend une place quand la bande ne montre
                    // pas tout : le reste s'ouvre par un clic.
                    let total = self.data.as_ref().map_or(0, |data| data.list.len());
                    if total > shown {
                        shown = shown.saturating_sub(1);
                    }
                    let urls: Vec<String> = self
                        .order
                        .iter()
                        .take(shown)
                        .filter_map(|index| self.data.as_ref()?.list.get(*index))
                        .map(|achievement| achievement.icon.clone())
                        .filter(|url| !url.is_empty())
                        .collect();
                    self.start_icons(urls, ui.ctx());
                }
                let mut open_list = false;
                if ready {
                    if let Some(data) = &self.data {
                        let top = row.center().y - SIZE / 2.0;
                        let mut x = first;
                        for index in self.order.iter().take(shown) {
                            let Some(achievement) = data.list.get(*index) else {
                                continue;
                            };
                            let icon = egui::Rect::from_min_size(
                                egui::pos2(x, top),
                                egui::vec2(SIZE, SIZE),
                            );
                            match self.icons.get(&achievement.icon) {
                                Some(texture) => {
                                    let tint = if achievement.unlocked {
                                        egui::Color32::WHITE
                                    } else {
                                        egui::Color32::from_gray(96)
                                    };
                                    egui::Image::new((texture.id(), icon.size()))
                                        .rounding(egui::Rounding::same(6.0))
                                        .tint(tint)
                                        .paint_at(ui, icon);
                                }
                                None => {
                                    ui.painter().rect_filled(
                                        icon,
                                        egui::Rounding::same(6.0),
                                        CARD_HOVER(),
                                    );
                                }
                            }
                            let id = ui.id().with(("achievement", *index));
                            ui.interact(icon, id, egui::Sense::hover())
                                .on_hover_text(achievement_tooltip(achievement));
                            x += SIZE + GAP;
                        }
                        let rest = data.list.len().saturating_sub(shown);
                        if rest > 0 && x + SIZE <= count_left + 0.5 {
                            let tile = egui::Rect::from_min_size(
                                egui::pos2(x, top),
                                egui::vec2(SIZE, SIZE),
                            );
                            let response = ui.interact(
                                tile,
                                ui.id().with("achievement-rest"),
                                egui::Sense::click(),
                            );
                            ui.painter().rect_filled(
                                tile,
                                egui::Rounding::same(6.0),
                                if response.hovered() {
                                    CARD_ACTIVE()
                                } else {
                                    CARD_HOVER()
                                },
                            );
                            ui.painter().text(
                                tile.center(),
                                egui::Align2::CENTER_CENTER,
                                format!("+{rest}"),
                                egui::FontId::new(12.0, egui::FontFamily::Proportional),
                                TEXT(),
                            );
                            if response
                                .on_hover_text(lapinux_core::i18n::t_args(
                                    "achievements.see_all",
                                    &[("count", &data.list.len().to_string())],
                                ))
                                .clicked()
                            {
                                open_list = true;
                            }
                        }
                    }
                }
                ui.add_space(6.0);
                if !owned && steam {
                    centered_notice(ui, lapinux_core::i18n::t("achievements.empty"));
                    return;
                }
                if self.loaded_for.is_none() {
                    centered_notice(ui, lapinux_core::i18n::t("achievements.empty"));
                    return;
                }
                if self.loading {
                    // Sans planche, la carte garde sa ligne d'attente : même
                    // libellé, même taille que sous le lapin.
                    ui.horizontal(|ui| {
                        crate::section_icons::loading(ui, 18.0);
                        ui.label(
                            egui::RichText::new(lapinux_core::i18n::t("achievements.loading"))
                                .size(crate::launch::WAITING_LABEL_FONT)
                                .color(TEXT_DIM()),
                        );
                    });
                    return;
                }
                if self.error.is_some() {
                    // Rien à montrer : la carte le dit au milieu d'elle-même,
                    // sans détailler une panne que les réglages expliquent.
                    centered_notice(ui, lapinux_core::i18n::t("achievements.empty"));
                    return;
                }
                let fraction = match &self.data {
                    Some(data) if !data.list.is_empty() => data.fraction(),
                    Some(_) => {
                        // Un jeu sans succès le dit au milieu de son encart.
                        centered_notice(ui, lapinux_core::i18n::t("achievements.empty"));
                        return;
                    }
                    None => return,
                };
                ui.add(
                    egui::ProgressBar::new(fraction)
                        .desired_height(5.0)
                        .desired_width(inner)
                        .fill(accent)
                        .rounding(egui::Rounding::same(3.0)),
                );
                // La place libre sous la barre, dans la fiche repliée : les
                // succès qui viennent d'être obtenus, ou à défaut les plus
                // rares encore à décrocher.
                // Ce qui reste, écarts compris : une ligne coûte sa hauteur
                // plus l'écart qui la sépare de la précédente. Sans ce compte,
                // deux lignes de trop faisaient grandir la carte au-delà de
                // celle des autres encarts.
                let free = (height - 20.0) - (ui.cursor().top() - top);
                let gap = ui.spacing().item_spacing.y;
                let room = ((free - HIGHLIGHT_HEADING - 8.0) / (HIGHLIGHT_ROW + gap))
                    .floor()
                    .max(0.0) as usize;
                if room > 0 {
                    if let Some((heading, picks)) = self.picks(room) {
                        let urls: Vec<String> = picks
                            .iter()
                            .map(|entry| entry.icon.clone())
                            .filter(|url| !url.is_empty())
                            .collect();
                        self.start_icons(urls, ui.ctx());
                        open_list |= self.highlights(ui, heading, &picks, inner);
                    }
                }
                if open_list {
                    self.modal = true;
                }
            });
    }

    /// Les succès mis en avant dans la place libre de l'encart, avec le libellé
    /// de leur section. Rien quand le jeu n'a aucun succès lisible.
    fn picks(&self, room: usize) -> Option<(&'static str, Vec<Achievement>)> {
        let data = self.data.as_ref()?;
        if data.list.is_empty() {
            return None;
        }
        let (heading, indices) = highlights(data, &self.order, room.min(HIGHLIGHT_ROWS));
        if indices.is_empty() {
            return None;
        }
        let picks = indices
            .iter()
            .filter_map(|index| data.list.get(*index).cloned())
            .collect();
        Some((heading, picks))
    }

    /// Lignes de succès sous la bande : l'icône, le nom, puis la date de
    /// déblocage ou la rareté. Chacune ouvre la liste complète, comme la tuile
    /// « + N » — la fiche repliée n'a pas la place de tout montrer.
    /// Rend vrai quand le clic demande cette liste.
    fn highlights(
        &self,
        ui: &mut egui::Ui,
        heading: &str,
        picks: &[Achievement],
        inner: f32,
    ) -> bool {
        let mut open = false;
        ui.add_space(8.0);
        let (line, _) =
            ui.allocate_exact_size(egui::vec2(inner, HIGHLIGHT_HEADING), egui::Sense::hover());
        crate::painted_text_at(ui, line.min, heading, 10.0, TEXT_DIM());
        for achievement in picks {
            let (row, response) =
                ui.allocate_exact_size(egui::vec2(inner, HIGHLIGHT_ROW), egui::Sense::click());
            let icon = egui::Rect::from_center_size(
                egui::pos2(row.left() + 7.0, row.center().y),
                egui::vec2(14.0, 14.0),
            );
            match self.icons.get(&achievement.icon) {
                Some(texture) => {
                    let tint = if achievement.unlocked {
                        egui::Color32::WHITE
                    } else {
                        egui::Color32::from_gray(96)
                    };
                    egui::Image::new((texture.id(), icon.size()))
                        .rounding(egui::Rounding::same(3.0))
                        .tint(tint)
                        .paint_at(ui, icon);
                }
                None => {
                    ui.painter()
                        .rect_filled(icon, egui::Rounding::same(3.0), CARD_HOVER());
                }
            }
            let detail = highlight_detail(achievement);
            let detail_size = crate::text_size(ui, &detail, 10.0);
            let room = (inner - 22.0 - detail_size.x - 8.0).max(16.0);
            let name = elide_to_width(
                ui.painter(),
                &achievement.name,
                egui::FontId::proportional(11.0),
                room,
            );
            let name_size = crate::text_size(ui, &name, 11.0);
            crate::painted_text_at(
                ui,
                egui::pos2(row.left() + 22.0, row.center().y - name_size.y / 2.0),
                &name,
                11.0,
                if achievement.unlocked {
                    TEXT()
                } else {
                    TEXT_DIM()
                },
            );
            crate::painted_text_at(
                ui,
                egui::pos2(
                    row.right() - detail_size.x,
                    row.center().y - detail_size.y / 2.0,
                ),
                &detail,
                10.0,
                TEXT_DIM(),
            );
            if response
                .on_hover_text(achievement_tooltip(achievement))
                .clicked()
            {
                open = true;
            }
        }
        open
    }
}

/// Les succès à mettre sous la bande : les derniers obtenus, ou, tant qu'aucun
/// ne l'est, les plus rares encore à décrocher. Les succès cachés gardent leur
/// nom secret : ils ne sont jamais mis en avant avant d'être obtenus.
fn highlights(data: &Achievements, order: &[usize], limit: usize) -> (&'static str, Vec<usize>) {
    let mut picked: Vec<usize> = order
        .iter()
        .copied()
        .filter(|index| data.list.get(*index).is_some_and(|entry| entry.unlocked))
        .collect();
    if !picked.is_empty() {
        picked.truncate(limit);
        return (lapinux_core::i18n::t("achievements.recent"), picked);
    }
    let mut locked: Vec<usize> = (0..data.list.len())
        .filter(|index| {
            let entry = &data.list[*index];
            !entry.hidden && entry.percent.is_some()
        })
        .collect();
    locked.sort_by(|a, b| {
        data.list[*a]
            .percent
            .unwrap_or(100.0)
            .total_cmp(&data.list[*b].percent.unwrap_or(100.0))
    });
    locked.truncate(limit);
    (lapinux_core::i18n::t("achievements.rarest"), locked)
}

/// Ce qui se lit à droite d'une ligne : la date du déblocage, ou la part des
/// joueurs qui possèdent le succès tant qu'il reste à décrocher.
fn highlight_detail(achievement: &Achievement) -> String {
    match (achievement.unlocked, achievement.unlock_time) {
        (true, Some(time)) => lapinux_core::util::format_date(time),
        (true, None) => lapinux_core::i18n::t("achievements.unlocked").to_string(),
        (false, _) => achievement
            .percent
            .map(|percent| format!("{} %", share(percent)))
            .unwrap_or_default(),
    }
}

/// Message centré au milieu de l'encart : le centre retenu est celui de la
/// carte (le même que le titre), pas celui de la fenêtre, et la hauteur est
/// celle de la place restée libre sous la bande.
fn centered_notice(ui: &mut egui::Ui, label: &str) {
    let rest = ui.available_rect_before_wrap();
    let center = egui::pos2(ui.min_rect().center().x, rest.center().y);
    ui.painter().text(
        center,
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(11.0),
        crate::TEXT_DIM(),
    );
}

fn share(value: f32) -> String {
    lapinux_core::i18n::decimal(format!("{value:.1}"))
}

fn achievement_tooltip(achievement: &Achievement) -> String {
    let mut text = achievement.name.clone();
    if achievement.hidden && !achievement.unlocked {
        text.push_str(lapinux_core::i18n::t("achievements.hidden_suffix"));
    }
    if !achievement.description.is_empty() {
        text.push('\n');
        text.push_str(&achievement.description);
    }
    if let Some(percent) = achievement.percent {
        text.push_str("\n");
        text.push_str(&lapinux_core::i18n::t_args(
            "achievements.players_percent",
            &[("percent", &share(percent))],
        ));
    }
    text.push_str("\n\n");
    match (achievement.unlocked, achievement.unlock_time) {
        (true, Some(time)) => {
            text.push_str(&lapinux_core::i18n::t_args(
                "achievements.unlocked_on",
                &[("date", &lapinux_core::util::format_datetime(time))],
            ));
        }
        (true, None) => text.push_str(lapinux_core::i18n::t("achievements.unlocked")),
        (false, _) => text.push_str(lapinux_core::i18n::t("achievements.not_unlocked")),
    }
    text
}

/// Adresse de secours pour les visuels communautaires Steam : les anciens
/// caches peuvent encore porter l'ancien CDN Cloudflare, et quelques jeux
/// n'existent que sur l'un des deux.
fn alternate_steam_community_url(url: &str) -> Option<String> {
    const CURRENT: &str = "https://shared.fastly.steamstatic.com/community_assets/images/apps/";
    const LEGACY: &str =
        "https://cdn.cloudflare.steamstatic.com/steamcommunity/public/images/apps/";
    if let Some(rest) = url.strip_prefix(CURRENT) {
        Some(format!("{LEGACY}{rest}"))
    } else if let Some((_, rest)) = url.split_once("/steamcommunity/public/images/apps/") {
        Some(format!("{CURRENT}{rest}"))
    } else {
        None
    }
}

fn fetch_bytes(url: &str) -> Option<Vec<u8>> {
    let response = ureq::get(url)
        .set("User-Agent", "Lapinux/0.1")
        .timeout(ICON_TIMEOUT)
        .call()
        .ok()?;
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut response.into_reader(), &mut bytes).ok()?;
    Some(bytes)
}

/// Télécharge une image. Pour les visuels communautaires Steam, un ancien
/// chemin disparu est retenté sur le CDN actuel, et inversement : les caches
/// écrits avant la migration continuent de fonctionner.
pub(crate) fn http_bytes(url: &str) -> Option<Vec<u8>> {
    fetch_bytes(url).or_else(|| {
        alternate_steam_community_url(url).and_then(|alternate| fetch_bytes(&alternate))
    })
}

pub(crate) fn texture_from_bytes(
    ctx: &egui::Context,
    name: &str,
    bytes: &[u8],
) -> Option<egui::TextureHandle> {
    let image = image::load_from_memory(bytes).ok()?.to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    if size[0] == 0 || size[1] == 0 {
        return None;
    }
    let color = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
    Some(ctx.load_texture(name, color, egui::TextureOptions::LINEAR))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lapinux_core::model::SteamAchievement;

    /// Client pointé sur un socket absent, sans démarrage du daemon : les tests
    /// peignent des succès déjà chargés et ne doivent rien demander.
    fn offline() -> Client {
        Client {
            socket: std::path::PathBuf::from("/tmp/lapinux-tests-sans-daemon.sock"),
            auto_spawn: false,
            log_file: std::path::PathBuf::from("/tmp/lapinux-tests-sans-daemon.log"),
        }
    }

    fn sample(unlocked: usize, total: usize) -> Achievements {
        let mut list = Vec::new();
        for index in 0..total {
            list.push(Achievement {
                api_name: format!("A{index}"),
                name: format!("Succès {index}"),
                description: String::new(),
                icon: String::new(),
                hidden: false,
                unlocked: index < unlocked,
                unlock_time: (index < unlocked).then_some(1_700_000_000 + index as i64),
                percent: None,
            });
        }
        Achievements {
            list,
            play_minutes: None,
        }
    }

    /// Textes peints par le bloc, avec leur position.
    fn painted_texts(output: &egui::FullOutput) -> Vec<(String, egui::Rect)> {
        fn walk(shape: &egui::Shape, out: &mut Vec<(String, egui::Rect)>) {
            match shape {
                egui::Shape::Text(text) => out.push((
                    text.galley.text().to_string(),
                    egui::Rect::from_min_size(text.pos, text.galley.rect.size()),
                )),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut out);
        }
        out
    }

    /// Rectangles peints d'une couleur donnée : les tuiles d'icônes et la
    /// barre se reconnaissent à leur teinte.
    fn painted_rects(output: &egui::FullOutput, fill: egui::Color32) -> Vec<egui::Rect> {
        fn walk(shape: &egui::Shape, fill: egui::Color32, out: &mut Vec<egui::Rect>) {
            match shape {
                egui::Shape::Rect(rect) if rect.fill == fill => out.push(rect.rect),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, fill, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, fill, &mut out);
        }
        out
    }

    #[test]
    fn the_block_paints_a_count_and_a_strip_of_icons() {
        let ctx = egui::Context::default();
        crate::install_fonts(&ctx);
        let mut sheet = Sheet::default();
        // Succès déjà chargés : le bloc ne doit rien demander au réseau.
        sheet.loaded_for = Some(10);
        sheet.data = Some(sample(2, 5));
        sheet.order = sheet
            .data
            .as_ref()
            .map(|data| data.order())
            .unwrap_or_default();
        let region = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(700.0, 220.0));
        let mut output = None;
        for _ in 0..2 {
            output = Some(ctx.run(
                egui::RawInput {
                    screen_rect: Some(region),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        sheet.block(
                            ui,
                            "Jeu test",
                            10,
                            true,
                            true,
                            &offline(),
                            660.0,
                            0.0,
                            None,
                            lapinux_core::stores::Store::Steam,
                            egui::Color32::from_rgb(0xE0, 0x9A, 0x40),
                        );
                    });
                },
            ));
        }
        let output = output.expect("une image peinte");
        let texts = painted_texts(&output);
        let title = texts
            .iter()
            .find(|(text, _)| text == lapinux_core::i18n::t("achievements.title"))
            .map(|(_, rect)| *rect)
            .expect("titre du bloc");
        let count = texts
            .iter()
            .find(|(text, _)| text.starts_with("2 / 5"))
            .map(|(_, rect)| *rect)
            .unwrap_or_else(|| {
                panic!(
                    "compte des succès : {:?}",
                    texts
                        .iter()
                        .map(|(text, _)| text.as_str())
                        .collect::<Vec<_>>()
                )
            });
        // La bande se range à droite du mot, sur sa ligne, et s'arrête avant
        // le compte.
        let icons = painted_rects(&output, CARD_HOVER());
        assert_eq!(icons.len(), 5, "une tuile par succès chargé");
        let first = icons[0];
        assert!(
            first.left() > title.right(),
            "icône à droite du mot ({:?} / {:?})",
            first,
            title
        );
        assert!(
            first.center().y > title.top() && first.center().y < title.bottom(),
            "icône sur la ligne du mot (y = {})",
            first.center().y
        );
        assert!(
            icons.iter().all(|icon| icon.right() < count.left()),
            "la bande ne touche pas le compte"
        );
        // La barre passe sous la bande, et rien ne déborde de la colonne.
        // Elle porte la teinte du jeu (`accent`), plus le vert fixe d'avant.
        let bars = painted_rects(&output, egui::Color32::from_rgb(0xE0, 0x9A, 0x40));
        assert!(
            bars.iter().any(|bar| bar.top() > first.bottom()),
            "barre sous la bande"
        );
        for (text, rect) in &texts {
            assert!(
                rect.right() <= region.right(),
                "« {text} » déborde à droite"
            );
            assert!(
                rect.bottom() <= region.bottom(),
                "« {text} » déborde en bas"
            );
        }
    }

    /// Peint le bloc à une hauteur donnée et rend ses textes peints.
    fn block_in(height: f32, data: Achievements) -> egui::FullOutput {
        let ctx = egui::Context::default();
        crate::install_fonts(&ctx);
        let mut sheet = Sheet::default();
        sheet.loaded_for = Some(10);
        sheet.order = data.order();
        sheet.data = Some(data);
        let region = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(700.0, 340.0));
        let mut output = None;
        for _ in 0..2 {
            output = Some(ctx.run(
                egui::RawInput {
                    screen_rect: Some(region),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        sheet.block(
                            ui,
                            "Jeu test",
                            10,
                            true,
                            true,
                            &offline(),
                            660.0,
                            height,
                            None,
                            lapinux_core::stores::Store::Steam,
                            egui::Color32::from_rgb(0xE0, 0x9A, 0x40),
                        );
                    });
                },
            ));
        }
        output.expect("une image peinte")
    }

    /// Un jeu sans succès le dit au milieu de son encart, en toutes lettres :
    /// le trophée barré ne parlait pas assez clair.
    #[test]
    fn a_game_without_achievements_says_so_in_the_middle() {
        let output = block_in(170.0, sample(0, 0));
        let texts = painted_texts(&output);
        let (_, rect) = texts
            .iter()
            .find(|(text, _)| text == lapinux_core::i18n::t("achievements.empty"))
            .expect("le message « Aucun succès »");
        let (_, title) = texts
            .iter()
            .find(|(text, _)| text == lapinux_core::i18n::t("achievements.title"))
            .expect("le titre de l'encart");
        // Centré sur le titre de l'encart, donc au milieu de la carte, et sous
        // lui : le message occupe la place libre, pas le coin.
        assert!(
            (rect.center().x - title.center().x).abs() < 1.0,
            "message décalé : {rect:?} contre {title:?}"
        );
        assert!(
            rect.center().y > title.bottom(),
            "message trop haut : {rect:?}"
        );
    }

    /// La place libre sous la barre porte les derniers succès obtenus, avec
    /// leur date ; sans place réservée, le bloc s'en tient à sa bande.
    #[test]
    fn the_free_space_under_the_band_lists_the_last_unlocks() {
        let output = block_in(170.0, sample(2, 5));
        let texts = painted_texts(&output);
        let rect_of = |needle: &str| {
            texts
                .iter()
                .find(|(text, _)| text == needle)
                .map(|(_, rect)| *rect)
                .unwrap_or_else(|| {
                    panic!(
                        "{needle} manque : {:?}",
                        texts
                            .iter()
                            .map(|(text, _)| text.as_str())
                            .collect::<Vec<_>>()
                    )
                })
        };
        assert!(
            rect_of(lapinux_core::i18n::t("achievements.recent")).is_positive(),
            "section des derniers succès"
        );
        let last = rect_of("Succès 1");
        let before = rect_of("Succès 0");
        assert!(
            last.top() < before.top(),
            "le déblocage le plus récent passe en premier ({last:?} / {before:?})"
        );
        // La date du déblocage ferme la ligne, la plus récente en tête.
        assert!(
            texts
                .iter()
                .any(|(text, _)| text == &lapinux_core::util::format_date(1_700_000_001)),
            "date du dernier déblocage : {:?}",
            texts
                .iter()
                .map(|(text, _)| text.as_str())
                .collect::<Vec<_>>()
        );
        // L'encart descend jusqu'à la hauteur demandée : il finit avec les
        // autres cartes de la ligne.
        let card = painted_rects(&output, CARD())
            .into_iter()
            .max_by(|left, right| left.width().total_cmp(&right.width()))
            .expect("encart peint");
        assert!(
            (card.height() - 170.0).abs() < 1.0,
            "encart de {}",
            card.height()
        );
        for (text, rect) in &texts {
            assert!(
                card.contains_rect(*rect),
                "« {text} » sort de l'encart {card:?} / {rect:?}"
            );
        }
        // Sans place réservée, rien sous la barre.
        let output = block_in(0.0, sample(2, 5));
        let texts = painted_texts(&output);
        assert!(
            !texts
                .iter()
                .any(|(text, _)| text == lapinux_core::i18n::t("achievements.recent")),
            "pas de ligne sans place"
        );
    }

    /// Tant qu'aucun succès n'est obtenu, la place libre montre les plus rares
    /// encore à décrocher — jamais un succès caché, dont le nom est secret.
    #[test]
    fn the_free_space_shows_the_rarest_while_nothing_is_unlocked() {
        let mut data = sample(0, 4);
        for (index, percent) in [1.2_f32, 40.0, 12.0, 0.5].into_iter().enumerate() {
            data.list[index].percent = Some(percent);
        }
        data.list[0].hidden = true;
        let output = block_in(170.0, data);
        let texts = painted_texts(&output);
        let names: Vec<&str> = texts.iter().map(|(text, _)| text.as_str()).collect();
        assert!(
            names.contains(&lapinux_core::i18n::t("achievements.rarest")),
            "{names:?}"
        );
        let rarest = lapinux_core::i18n::decimal("0.5 %");
        assert!(
            names.contains(&rarest.as_str()),
            "la rareté se lit : {names:?}"
        );
        assert!(
            names.contains(&"Succès 3"),
            "le plus rare d'abord : {names:?}"
        );
        assert!(names.contains(&"Succès 2"), "{names:?}");
        assert!(
            !names.contains(&"Succès 0"),
            "un succès caché se tait : {names:?}"
        );
    }

    /// Images peintes par le bloc : leur texture et leur cadre.
    fn painted_images(output: &egui::FullOutput) -> Vec<egui::Rect> {
        fn walk(shape: &egui::Shape, out: &mut Vec<egui::Rect>) {
            match shape {
                // La police est le maillage 0 ; tout le reste est une image.
                egui::Shape::Mesh(mesh) if mesh.texture_id != egui::TextureId::Managed(0) => {
                    out.push(mesh.calc_bounds());
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut out);
        }
        out
    }

    /// Peint le bloc avec des succès déjà chargés et rend ce qu'il a dessiné.
    fn block_with(unlocked: usize, total: usize) -> (Sheet, egui::FullOutput) {
        let ctx = egui::Context::default();
        crate::install_fonts(&ctx);
        let mut sheet = Sheet::default();
        sheet.loaded_for = Some(10);
        sheet.data = Some(sample(unlocked, total));
        sheet.order = sheet
            .data
            .as_ref()
            .map(|data| data.order())
            .unwrap_or_default();
        let region = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(700.0, 220.0));
        let mut output = None;
        for _ in 0..2 {
            output = Some(ctx.run(
                egui::RawInput {
                    screen_rect: Some(region),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        sheet.block(
                            ui,
                            "Jeu test",
                            10,
                            true,
                            true,
                            &offline(),
                            660.0,
                            0.0,
                            None,
                            lapinux_core::stores::Store::Steam,
                            egui::Color32::from_rgb(0xE0, 0x9A, 0x40),
                        );
                    });
                },
            ));
        }
        (sheet, output.expect("une image peinte"))
    }

    #[test]
    fn a_finished_game_wears_the_platinum_trophy_before_its_count() {
        let (_sheet, output) = block_with(5, 5);
        let texts = painted_texts(&output);
        let count = texts
            .iter()
            .find(|(text, _)| text.starts_with("5 / 5"))
            .map(|(_, rect)| *rect)
            .expect("compte des succès");
        let title = texts
            .iter()
            .find(|(text, _)| text == lapinux_core::i18n::t("achievements.title"))
            .map(|(_, rect)| *rect)
            .expect("titre du bloc");
        let images = painted_images(&output);
        assert_eq!(images.len(), 1, "le trophée, et lui seul");
        let trophy = images[0];
        assert!(
            trophy.right() <= count.left() && trophy.left() > title.right(),
            "trophée entre les succès et le compte ({trophy:?}, compte {count:?})"
        );
        assert!(
            trophy.center().y > title.top() && trophy.center().y < title.bottom(),
            "trophée sur la ligne du mot"
        );
    }

    #[test]
    fn a_game_in_progress_has_no_platinum_trophy() {
        let (_sheet, output) = block_with(2, 5);
        assert!(
            painted_images(&output).is_empty(),
            "pas de trophée avant la fin"
        );
    }

    #[test]
    fn the_block_says_no_achievements_without_asking_the_network() {
        let ctx = egui::Context::default();
        crate::install_fonts(&ctx);
        let mut sheet = Sheet::default();
        let region = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(700.0, 220.0));
        let mut output = None;
        for _ in 0..2 {
            output = Some(ctx.run(
                egui::RawInput {
                    screen_rect: Some(region),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        sheet.block(
                            ui,
                            "Jeu test",
                            10,
                            true,
                            false,
                            &offline(),
                            660.0,
                            0.0,
                            None,
                            lapinux_core::stores::Store::Steam,
                            egui::Color32::from_rgb(0xE0, 0x9A, 0x40),
                        );
                    });
                },
            ));
        }
        let texts = painted_texts(&output.expect("une image peinte"));
        assert!(
            texts
                .iter()
                .any(|(text, _)| text == lapinux_core::i18n::t("achievements.empty")),
            "la carte le dit sans détailler : {:?}",
            texts
                .iter()
                .map(|(text, _)| text.as_str())
                .collect::<Vec<_>>()
        );
        assert!(sheet.loaded_for.is_none());
    }

    #[test]
    fn a_long_list_announces_what_the_strip_leaves_out() {
        let ctx = egui::Context::default();
        crate::install_fonts(&ctx);
        let mut sheet = Sheet::default();
        sheet.loaded_for = Some(10);
        sheet.data = Some(sample(2, 60));
        let region = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(700.0, 260.0));
        let mut output = None;
        for _ in 0..2 {
            output = Some(ctx.run(
                egui::RawInput {
                    screen_rect: Some(region),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        sheet.block(
                            ui,
                            "Jeu test",
                            10,
                            true,
                            true,
                            &offline(),
                            660.0,
                            0.0,
                            None,
                            lapinux_core::stores::Store::Steam,
                            egui::Color32::from_rgb(0xE0, 0x9A, 0x40),
                        );
                    });
                },
            ));
        }
        let texts = painted_texts(&output.expect("une image peinte"));
        assert!(
            texts.iter().any(|(text, _)| text.starts_with('+')),
            "la tuile du reste doit être peinte : {:?}",
            texts
                .iter()
                .map(|(text, _)| text.as_str())
                .collect::<Vec<_>>()
        );
        assert!(!sheet.modal, "la fenêtre ne s'ouvre qu'au clic");
    }

    #[test]
    fn the_full_list_window_shows_the_running_successes_first() {
        let ctx = egui::Context::default();
        crate::install_fonts(&ctx);
        let mut sheet = Sheet::default();
        sheet.loaded_for = Some(10);
        let data = sample(2, 60);
        sheet.order = data.order();
        sheet.data = Some(data);
        sheet.title = "Jeu test".into();
        sheet.modal = true;
        let region = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 700.0));
        let mut output = None;
        for _ in 0..2 {
            output = Some(ctx.run(
                egui::RawInput {
                    screen_rect: Some(region),
                    ..Default::default()
                },
                |ctx| sheet.panel(ctx),
            ));
        }
        let texts = painted_texts(&output.expect("une image peinte"));
        // Le panneau fixe porte le titre, le jeu et le compte en sous-titre.
        let subtitle = lapinux_core::i18n::t_args(
            "achievements.subtitle",
            &[
                ("title", "Jeu test"),
                ("unlocked", "2"),
                ("total", "60"),
                ("percent", &lapinux_core::i18n::decimal("3.3")),
                ("platinum", ""),
            ],
        );
        for expected in [
            lapinux_core::i18n::t("achievements.title"),
            subtitle.as_str(),
            "Succès 1",
            "Succès 0",
        ] {
            assert!(
                texts.iter().any(|(text, _)| text == expected),
                "« {expected} » manque : {:?}",
                texts
                    .iter()
                    .map(|(text, _)| text.as_str())
                    .collect::<Vec<_>>()
            );
        }
        assert!(texts
            .iter()
            .any(|(text, _)| text == lapinux_core::i18n::t("achievements.locked")));
        assert!(sheet.modal);
    }

    /// Lecture du protocole telle que le daemon la rend : le schéma et l'état
    /// du compte déjà assemblés.
    fn read(entries: &[(&str, &str, &str, bool, bool, Option<i64>)]) -> SteamAchievements {
        SteamAchievements {
            appid: 4000,
            fetched: 0,
            play_minutes: None,
            items: entries
                .iter()
                .map(
                    |(api_name, name, description, hidden, achieved, time)| SteamAchievement {
                        api_name: (*api_name).to_string(),
                        name: (*name).to_string(),
                        description: (*description).to_string(),
                        icon: format!("https://cdn.example/{api_name}.jpg"),
                        hidden: *hidden,
                        achieved: *achieved,
                        unlock_time: *time,
                        percent: None,
                    },
                )
                .collect(),
            notes: Vec::new(),
        }
    }

    #[test]
    fn a_success_is_assembled_from_the_protocol_read() {
        let read = read(&[
            (
                "NEW_DAY",
                "Nouveau jour",
                "Termine le premier jour",
                false,
                false,
                None,
            ),
            (
                "BOSS",
                "Boss final",
                "Vaincs le boss",
                false,
                true,
                Some(1_700_000_000),
            ),
        ]);
        let merged = assemble(read);
        assert_eq!(merged.total(), 2);
        assert_eq!(merged.unlocked(), 1);
        assert_eq!(merged.fraction(), 0.5);
        assert_eq!(merged.list[0].name, "Nouveau jour");
        assert_eq!(merged.list[0].description, "Termine le premier jour");
        assert!(!merged.list[0].unlocked);
        assert_eq!(merged.list[1].unlock_time, Some(1_700_000_000));
    }

    #[test]
    fn unlocked_successes_come_first_most_recent_first() {
        let merged = assemble(read(&[
            ("A", "A", "", false, true, Some(1_600_000_000)),
            ("B", "B", "", false, true, Some(1_700_000_000)),
            ("C", "C", "", false, false, None),
        ]));
        let names: Vec<String> = merged
            .ordered()
            .into_iter()
            .map(|entry| entry.name.clone())
            .collect();
        assert_eq!(names, vec!["B", "A", "C"]);
    }

    #[test]
    fn a_hidden_success_keeps_the_name_the_protocol_published() {
        // Le schéma du protocole donne le nom et la description d'un succès
        // caché : le drapeau ne sert qu'à l'annoncer dans l'infobulle.
        let locked = assemble(read(&[(
            "SECRET",
            "Le grand secret",
            "Tu ne dois pas le savoir",
            true,
            false,
            None,
        )]));
        assert_eq!(locked.list[0].name, "Le grand secret");
        assert_eq!(locked.list[0].description, "Tu ne dois pas le savoir");
        assert!(locked.list[0].hidden);
        assert!(!locked.list[0].unlocked);
        assert!(achievement_tooltip(&locked.list[0])
            .contains(lapinux_core::i18n::t("achievements.hidden_suffix")));

        let unlocked = assemble(read(&[(
            "SECRET",
            "Le grand secret",
            "Tu ne dois pas le savoir",
            true,
            true,
            Some(1_700_000_000),
        )]));
        assert_eq!(unlocked.list[0].name, "Le grand secret");
        assert!(!achievement_tooltip(&unlocked.list[0]).contains("succès caché"));
    }

    #[test]
    fn a_stuck_read_gives_up_and_can_be_relaunched() {
        let mut sheet = Sheet::default();
        sheet.loaded_for = Some(10);
        sheet.loading = true;
        // Lecture engagée il y a plus que le budget : le bloc le dit.
        sheet.started = Some(Instant::now() - LOAD_BUDGET - Duration::from_secs(1));
        let ctx = egui::Context::default();
        sheet.poll(&ctx);
        assert!(!sheet.loading);
        assert_eq!(sheet.error.as_deref(), Some(timeout_error()));

        // « Réessayer » vide la lecture : la fiche repart sur une demande nette.
        sheet.forget();
        assert!(sheet.loaded_for.is_none());
        assert!(sheet.error.is_none());
        assert!(!sheet.loading);
    }

    #[test]
    fn an_empty_read_says_why_instead_of_staying_blank() {
        // Le protocole explique l'échec dans ses notes : la fiche les reprend.
        let mut refused = read(&[]);
        refused
            .notes
            .push("compte Steam non connecté : les succès se lisent avec la session".into());
        assert_eq!(
            empty_message(&refused),
            "compte Steam non connecté : les succès se lisent avec la session"
        );

        // Aucune note : la lecture a abouti, le jeu ne publie rien.
        assert_eq!(
            empty_message(&read(&[])),
            lapinux_core::i18n::t("achievements.none_published")
        );
    }

    #[test]
    fn old_steam_community_icons_fall_back_to_the_new_cdn() {
        let old = "https://cdn.cloudflare.steamstatic.com/steamcommunity/public/images/apps/4670920/a.jpg";
        let new =
            "https://shared.fastly.steamstatic.com/community_assets/images/apps/4670920/a.jpg";
        assert_eq!(alternate_steam_community_url(old).as_deref(), Some(new));
        assert_eq!(alternate_steam_community_url(new).as_deref(), Some(old));
        assert_eq!(
            alternate_steam_community_url("https://example.com/a.jpg"),
            None
        );
    }

    #[test]
    fn a_failed_icon_is_retried_after_the_delay() {
        let url = "https://shared.fastly.steamstatic.com/community_assets/images/apps/1/a.jpg";
        let now = Instant::now();
        let mut sheet = Sheet::default();
        assert!(sheet.icon_wanted(url, now));
        sheet.failed.insert(url.to_string(), now);
        assert!(!sheet.icon_wanted(url, now));
        assert!(sheet.icon_wanted(url, now + ICON_RETRY));
    }

    #[test]
    fn icon_requests_are_capped() {
        let urls: Vec<String> = (0..40)
            .map(|index| format!("https://cdn/{index}.jpg"))
            .collect();
        assert!(urls.len() > MAX_ICONS);
        // Une seule icône par adresse : la garde est un ensemble d'attente.
        let mut pending = HashSet::new();
        for url in urls.iter().take(MAX_ICONS) {
            assert!(pending.insert(url.clone()));
        }
        assert!(!pending.insert(urls[0].clone()));
        assert_eq!(pending.len(), MAX_ICONS);
    }

    #[test]
    fn the_display_order_is_computed_once() {
        let merged = assemble(read(&[
            ("A", "A", "", false, true, Some(1_600_000_000)),
            ("B", "B", "", false, true, Some(1_700_000_000)),
            ("C", "C", "", false, false, None),
        ]));
        // Indices : le débloqué le plus récent, puis l'autre, puis le reste.
        assert_eq!(merged.order(), vec![1, 0, 2]);
    }
}
