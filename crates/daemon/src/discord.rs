//! Rich Presence Discord.
//!
//! Discord expose une petite IPC locale : un socket Unix `discord-ipc-N`,
//! des trames binaires (opcode, longueur, JSON) et une poignée de commandes.
//! Rien de réseau, rien de bloquant : le daemon publie ce que fait Lapinux
//! quand le réglage est actif, et se taît sinon.
//!
//! Discord exige un identifiant d'application pour accepter une activité :
//! c'est lui qui nomme l'application et porte son icône, et les champs de
//! l'activité ne peuvent pas les remplacer. Le **titre principal** d'une
//! présence est donc toujours le nom de l'application dont on tient
//! l'identifiant, jamais un texte libre.
//!
//! En partie et sur une fiche, Lapinux se présente sous l'application Discord
//! officielle du jeu, trouvée dans le catalogue détectable (voir
//! [`super::discord_apps`]) : le titre est alors le jeu. Un jeu absent du
//! catalogue se présente sous l'application Lapinux, avec son nom en tête de
//! la première ligne : Discord ne permet pas de renommer une application à la
//! volée, mais la présence reste publiée. Les pages de l'application, elles,
//! se publient aussi sous Lapinux.

use lapinux_core::config::DiscordConfig;

use lapinux_core::model::{Game, Job};

use lapinux_core::i18n;
use lapinux_core::stores::Store;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::time::Duration;

/// Ce que l'interface montre : la section ouverte et le jeu dont la fiche
/// s'affiche. Le daemon ne peut pas le deviner, l'interface le lui dit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Viewing {
    pub section: String,
    pub game: Option<String>,
    /// Boutique, temps de jeu et installation de la fiche ouverte.
    pub note: Option<String>,
    /// Jaquette du jeu dont la fiche est ouverte : la grande vignette.
    pub cover: Option<String>,
    /// Boutique du jeu ouvert, ou de la page Boutiques : elle nomme la page
    /// et porte sa vignette.
    pub store: Option<Store>,
    /// Identifiant du jeu chez sa boutique, pour retrouver son application
    /// Discord quand ce n'est pas un jeu Steam.
    pub store_id: Option<String>,
    /// Appid du jeu Steam dont la fiche est ouverte : le daemon y retrouve
    /// l'application Discord officielle, qui donne le titre.
    pub appid: Option<u32>,
    /// Appid interne du jeu, toutes boutiques confondues : il nomme l'icône
    /// déjà téléchargée par la fiche, que Discord réutilise telle quelle.
    pub art_appid: Option<u32>,
}

/// Application Discord de Lapinux : elle publie les pages de l'application,
/// dont le titre est forcément son nom.
fn client_id() -> Option<String> {
    let id = lapinux_core::config::DEFAULT_DISCORD_CLIENT_ID;
    (!id.is_empty()).then(|| id.to_string())
}

/// Identifiant demandé par l'application Discord n'est pas unique : Discord
/// accepte aussi un pid pour rattacher l'activité au bon jeu.
fn pid() -> u32 {
    std::process::id()
}

/// Socket de Discord : le premier qui répond, du plus récent au plus ancien.
fn connect_socket() -> Option<UnixStream> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from);
    let mut roots: Vec<std::path::PathBuf> = Vec::new();
    if let Some(dir) = runtime {
        roots.push(dir);
    }
    roots.push(std::path::PathBuf::from("/tmp"));
    for root in roots {
        for index in 0..10 {
            let path = root.join(format!("discord-ipc-{index}"));
            if let Ok(stream) = UnixStream::connect(&path) {
                return Some(stream);
            }
        }
    }
    None
}

fn write_frame(stream: &mut UnixStream, opcode: u32, payload: &serde_json::Value) -> bool {
    let body = payload.to_string();
    let mut frame = Vec::with_capacity(body.len() + 8);
    frame.extend_from_slice(&opcode.to_le_bytes());
    frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
    frame.extend_from_slice(body.as_bytes());
    stream.write_all(&frame).is_ok()
}

/// Lit une trame de réponse. Utilisé pour la poignée de main seulement : après
/// elle, Discord ne répond aux activités que par une trame de confirmation dont
/// on n'a rien à faire.
fn read_frame(stream: &mut UnixStream) -> Option<serde_json::Value> {
    let mut header = [0u8; 8];
    stream.read_exact(&mut header).ok()?;
    let length = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
    let mut body = vec![0u8; length.min(64 * 1024)];
    stream.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

/// Connexion Discord prête à publier.
pub struct Presence {
    stream: UnixStream,
}

impl Presence {
    /// Ouvre le socket et se présente. `None` quand Discord ne tourne pas.
    fn connect(client_id: &str) -> Option<Self> {
        let mut stream = connect_socket()?;
        stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .ok()?;
        let handshake = serde_json::json!({ "v": 1, "client_id": client_id });
        if !write_frame(&mut stream, 0, &handshake) {
            return None;
        }
        // READY (opcode 1) ou ERREUR (opcode 2) : une erreur ferme la session,
        // un nouvel essai aura lieu plus tard.
        let reply = read_frame(&mut stream)?;
        if reply.get("evt").and_then(|value| value.as_str()) != Some("READY") {
            return None;
        }
        Some(Self { stream })
    }

    /// Chaque commande reçoit exactement une réponse : la lire tout de suite
    /// évite que les réponses s'accumulent dans le tampon du socket jusqu'à le
    /// bloquer, et révèle un refus (`ERROR`) que l'écriture seule ne montre pas.
    fn confirm(&mut self) -> bool {
        match read_frame(&mut self.stream) {
            Some(reply) => reply.get("evt").and_then(|value| value.as_str()) != Some("ERROR"),
            None => false,
        }
    }

    fn set_activity(&mut self, activity: &serde_json::Value) -> bool {
        let payload = serde_json::json!({
            "cmd": "SET_ACTIVITY",
            "args": { "pid": pid(), "activity": activity },
            "nonce": format!("lapinux-{}", lapinux_core::util::now_unix()),
        });
        write_frame(&mut self.stream, 1, &payload) && self.confirm()
    }

    fn clear(&mut self) {
        let payload = serde_json::json!({
            "cmd": "SET_ACTIVITY",
            "args": { "pid": pid() },
            "nonce": format!("lapinux-clear-{}", lapinux_core::util::now_unix()),
        });
        if write_frame(&mut self.stream, 1, &payload) {
            self.confirm();
        }
    }
}

/// Phrase de la page ouverte, telle qu'elle se lit sous le titre :
/// « Parcourt sa bibliothèque », « Parcourt la boutique GOG »…
fn section_title(section: &str, store: Option<Store>) -> String {
    match section {
        "library" => i18n::t("discord.page_library").to_string(),
        "downloads" => i18n::t("discord.page_downloads").to_string(),
        "settings" => i18n::t("discord.page_settings").to_string(),
        "controllers" => i18n::t("discord.page_controllers").to_string(),
        "profile" => i18n::t("discord.page_profile").to_string(),
        "captures" => i18n::t("discord.page_captures").to_string(),
        "boutiques" => match store {
            Some(store) => i18n::t_args("discord.page_stores", &[("store", store.label())]),
            None => i18n::t("discord.page_stores_all").to_string(),
        },
        _ => i18n::t("discord.page_default").to_string(),
    }
}

/// Petite vignette de la page Boutiques : la marque de la boutique, publiée
/// sous l'application Lapinux.
fn store_asset(store: Store) -> &'static str {
    match store {
        Store::Steam => "steam",
        Store::Epic => "epic",
        Store::Gog => "gog",
        Store::Itch => "itch",
    }
}

/// Pose l'icône du jeu comme visuel principal de l'activité. Discord n'accepte
/// qu'une URL : une icône locale ne peut donc pas lui être envoyée.
fn add_game_icon(activity: &mut serde_json::Value, icon: &str) {
    let icon = icon.trim();
    if !icon.starts_with("https://") {
        return;
    }
    let name = activity
        .get("name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    if !activity
        .get("assets")
        .map(serde_json::Value::is_object)
        .unwrap_or(false)
    {
        activity["assets"] = serde_json::json!({});
    }
    // L'icône occupe la grande vignette : c'est elle qu'on veut voir, pas la
    // jaquette ni le logo de la boutique. Une petite vignette en plus ne
    // ferait que répéter la même image.
    activity["assets"]["large_image"] = serde_json::json!(icon);
    activity["assets"]["large_text"] = serde_json::json!(clamp(&name));
    if let Some(assets) = activity["assets"].as_object_mut() {
        assets.remove("small_image");
        assets.remove("small_text");
    }
}

/// Icône publique du jeu pour Discord. Une icône de jeu vient de Steam ou de
/// SteamGridDB. Ni la jaquette ni le logo de la boutique ne la remplacent :
/// sans icône publiée, l'activité garde simplement l'application du jeu.
fn presence_icon(
    shared: &super::Shared,
    store: Option<Store>,
    appid: Option<u32>,
    art_appid: Option<u32>,
    name: &str,
) -> Option<String> {
    let store = store
        .or_else(|| {
            appid.and_then(|appid| shared.cached_store_entry(appid).map(|entry| entry.store))
        })
        .or_else(|| appid.map(|_| Store::Steam));
    let key = format!(
        "{}:{}:{}",
        store.map(Store::key).unwrap_or_default(),
        appid.unwrap_or(0),
        name.trim().to_lowercase()
    );
    let art_dir = shared.paths.art_dir();
    let art_id = art_appid.or(appid);
    // L'icône que la fiche a déjà choisie et téléchargée fait foi : on relit
    // son adresse avant le cache mémoire, sinon une résolution antérieure
    // garderait la main pendant dix minutes et Discord divergerait encore.
    if let Some(appid) = art_id {
        if let Some(url) = cached_sgdb_icon_url(&art_dir, appid) {
            return Some(url);
        }
    }
    // Une résolution coûte un appel réseau : le résultat, même vide, reste
    // dix minutes en mémoire. La boucle Discord repasse toutes les cinq
    // secondes, elle ne doit pas interroger SteamGridDB à chaque tour.
    if let Ok(cache) = presence_icons().lock() {
        if let Some((url, at)) = cache.get(&key) {
            if at.elapsed() < Duration::from_secs(600) {
                return url.clone();
            }
        }
    }
    // À défaut d'adresse laissée par la fiche, la dernière résolution du
    // daemon évite de redemander SteamGridDB au redémarrage.
    let mut url = art_id.and_then(|appid| cached_presence_icon_url(&art_dir, appid));
    if url.is_none() && store == Some(Store::Steam) {
        if let Some(appid) = appid {
            let catalog = lapinux_steam::catalog::Catalog::load(&shared.catalog_path());
            url = catalog
                .get(appid)
                .map(|entry| entry.details.icon_image.trim().to_string())
                .filter(|icon| icon.starts_with("https://"));
        }
    }
    if url.is_none() {
        let config = shared.config_snapshot().artwork;
        if config.steamgriddb && !config.steamgriddb_key.trim().is_empty() {
            url = sgdb_icon_url(appid, name, config.steamgriddb_key.trim());
        }
    }
    if let Ok(mut cache) = presence_icons().lock() {
        cache.insert(key, (url.clone(), std::time::Instant::now()));
    }
    if let (Some(appid), Some(url)) = (art_id, url.as_deref()) {
        remember_presence_icon_url(&art_dir, appid, url);
    }
    url
}

/// Adresse publique gardée à côté de l'icône SteamGridDB téléchargée par la
/// fiche. Le format du nom est celui du cache d'illustrations de l'interface.
fn cached_sgdb_icon_url(dir: &std::path::Path, appid: u32) -> Option<String> {
    let path = dir.join(format!("sgdb-icon-{appid}.url"));
    let url = std::fs::read_to_string(path).ok()?.trim().to_string();
    url.starts_with("https://").then_some(url)
}

/// Dernière adresse résolue par le daemon quand la fiche n'en a pas encore
/// laissé : elle évite un nouvel appel à SteamGridDB après un redémarrage.
fn cached_presence_icon_url(dir: &std::path::Path, appid: u32) -> Option<String> {
    let path = dir.join(format!("discord-icon-{appid}.url"));
    let url = std::fs::read_to_string(path).ok()?.trim().to_string();
    url.starts_with("https://").then_some(url)
}

fn remember_presence_icon_url(dir: &std::path::Path, appid: u32, url: &str) {
    if !url.starts_with("https://") {
        return;
    }
    std::fs::create_dir_all(dir).ok();
    let path = dir.join(format!("discord-icon-{appid}.url"));
    let temporary = path.with_extension("tmp");
    if std::fs::write(&temporary, url).is_ok() {
        let _ = std::fs::rename(&temporary, &path);
    }
}

fn presence_icons() -> &'static std::sync::Mutex<
    std::collections::HashMap<String, (Option<String>, std::time::Instant)>,
> {
    static CACHE: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, (Option<String>, std::time::Instant)>>,
    > = std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Icône carrée choisie sur SteamGridDB. L'API accepte le numéro Steam quand
/// on l'a, sinon le nom du jeu : les jeux Epic, GOG et itch passent donc par
/// la même recherche que les jeux Steam sans appid connu.
fn sgdb_icon_url(appid: Option<u32>, name: &str, key: &str) -> Option<String> {
    const BASE: &str = "https://www.steamgriddb.com/api/v2";
    let game = appid
        .and_then(|appid| sgdb_json(&format!("{BASE}/games/steam/{appid}"), key))
        .and_then(|value| {
            value
                .get("data")
                .and_then(|data| data.get("id"))
                .and_then(serde_json::Value::as_u64)
        });
    let game = game.or_else(|| {
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        let search = sgdb_json(
            &format!("{BASE}/search/autocomplete/{}", url_escape(name)),
            key,
        )?;
        let wanted = name.to_lowercase();
        search
            .get("data")
            .and_then(serde_json::Value::as_array)?
            .iter()
            .find(|item| {
                item.get("name")
                    .and_then(serde_json::Value::as_str)
                    .map(|found| found.to_lowercase() == wanted)
                    .unwrap_or(false)
            })
            .or_else(|| {
                search
                    .get("data")
                    .and_then(serde_json::Value::as_array)
                    .and_then(|items| items.first())
            })
            .and_then(|item| item.get("id"))
            .and_then(serde_json::Value::as_u64)
    })?;
    let icons = sgdb_json(&format!("{BASE}/icons/game/{game}"), key)?;
    sgdb_icon_pick(&icons)
}

fn sgdb_json(url: &str, key: &str) -> Option<serde_json::Value> {
    let response = ureq::get(url)
        .set("Authorization", &format!("Bearer {key}"))
        .set("User-Agent", "Lapinux/1.0")
        .timeout(Duration::from_secs(4))
        .call()
        .ok()?;
    serde_json::from_str(&response.into_string().ok()?).ok()
}

fn sgdb_icon_pick(value: &serde_json::Value) -> Option<String> {
    let mut best: Option<(f64, String)> = None;
    for asset in value.get("data")?.as_array()? {
        if asset
            .get("nsfw")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
            || asset
                .get("humor")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
            || asset.get("style").and_then(serde_json::Value::as_str) == Some("blurred")
        {
            continue;
        }
        let Some(url) = asset.get("url").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let width = asset
            .get("width")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as f64;
        let height = asset
            .get("height")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as f64;
        let png = asset
            .get("mime")
            .and_then(serde_json::Value::as_str)
            .map(|mime| mime.contains("png"))
            .unwrap_or_else(|| url.split('?').next().unwrap_or(url).ends_with(".png"));
        // L'icône officielle du jeu passe avant une création de la
        // communauté, même plus grande : Discord doit montrer le jeu, pas
        // l'interprétation qu'en a faite un internaute.
        let official = asset.get("style").and_then(serde_json::Value::as_str) == Some("official");
        // SteamGridDB omet parfois les dimensions d'une icône PNG. La refuser
        // laissait la présence sans icône alors que la fiche, elle, savait la
        // télécharger : un PNG sans taille connue reste donc un candidat.
        let score = if width > 0.0 && height > 0.0 {
            let ratio = width / height;
            if !(0.5..=2.0).contains(&ratio) {
                continue;
            }
            width.min(height) - (ratio - 1.0).abs() * 2000.0
        } else {
            if !png {
                continue;
            }
            1000.0
        } + if png { 500.0 } else { 0.0 }
            + if official { 1000.0 } else { 0.0 };
        if best.as_ref().map(|(top, _)| score > *top).unwrap_or(true) {
            best = Some((score, url.to_string()));
        }
    }
    best.map(|(_, url)| url)
}

/// Encode un nom de jeu pour un segment d'URL (`Portal 2` → `Portal%202`).
fn url_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'_' | b'.' | b'~') {
            out.push(*byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Boutique d'une tâche de téléchargement : le moteur Lapinux pose les jeux
/// Steam, les boutiques écrivent leur nom en clair dans le backend.
fn job_store(job: &Job) -> Option<Store> {
    let backend = job.backend.trim();
    if backend.eq_ignore_ascii_case("Lapinux") || backend.eq_ignore_ascii_case("Steam") {
        Some(Store::Steam)
    } else if backend.eq_ignore_ascii_case("Epic Games") || backend.eq_ignore_ascii_case("Epic") {
        Some(Store::Epic)
    } else if backend.eq_ignore_ascii_case("GOG") {
        Some(Store::Gog)
    } else if backend.eq_ignore_ascii_case("itch.io") || backend.eq_ignore_ascii_case("itch") {
        Some(Store::Itch)
    } else {
        None
    }
}

/// L'activité à publier, ou `None` quand rien ne doit l'être.
///
/// Chez Discord, une activité se lit sur trois lignes : le nom de
/// l'application, puis `details` et `state`. Le titre vient de l'application
/// utilisée — celle du jeu — donc les phrases et les chiffres vont dans les
/// deux lignes suivantes : `name` est ignoré pour une activité publiée par
/// IPC.
pub fn activity(
    config: &DiscordConfig,
    viewing: &Viewing,
    session: Option<&lapinux_core::model::SessionInfo>,
    job: Option<&Job>,
    speed_bps: u64,
    now: i64,
) -> Option<serde_json::Value> {
    if !config.enabled {
        return None;
    }
    // La partie en cours prime : c'est ce que les amis veulent voir.
    if config.show_game {
        if let Some(session) = session {
            // La cadence du moment, et la moyenne quand elle dit autre chose :
            // une chute passagère ne se lit pas dans une seule mesure.
            let details = match session.fps.filter(|stats| stats.current > 0) {
                Some(stats) if stats.avg > 0 && stats.avg != stats.current => i18n::t_fmt(
                    "discord.playing_fps_avg",
                    &[stats.current.to_string(), stats.avg.to_string()],
                ),
                Some(stats) => i18n::t_fmt("discord.playing_fps", &[stats.current.to_string()]),
                None => i18n::t("discord.playing").to_string(),
            };
            // La boutique d'abord, puis la couche de compatibilité, et
            // Lapinux en dernier : une info à part au bas de l'activité.
            let state = i18n::t_args(
                "discord.game_state",
                &[
                    ("store", session.store.label()),
                    ("runtime", runtime_label(&session.proton).as_str()),
                    ("mention", i18n::t("profile_page.on_lapinux")),
                ],
            );
            // Le nom du jeu est le titre de l'activité : Discord le prend de
            // l'application du jeu, sous laquelle on se présente.
            let mut activity = serde_json::json!({
                "name": clamp(&session.name),
                "details": clamp(&details),
                "state": clamp(&state),
                "instance": true,
            });
            // Le temps écoulé depuis le lancement : Discord l'affiche sous
            // les deux lignes, sans prendre la place d'une information.
            if let Some(start) = elapsed_start(session.started, now) {
                activity["timestamps"] = serde_json::json!({ "start": start });
            }
            return Some(activity);
        }
    }
    if config.show_downloads {
        if let Some(job) = job {
            let title = if job.title.trim().is_empty() {
                job.label.trim()
            } else {
                job.title.trim()
            };
            let percent = (job.progress.clamp(0.0, 1.0) * 100.0).round() as u32;
            // La boutique d'abord, les chiffres ensuite, Lapinux en dernier.
            let mut parts: Vec<String> = Vec::new();
            if let Some(store) = job_store(job) {
                parts.push(store.label().to_string());
            }
            parts.push(format!("{percent} %"));
            if job.total > 0 {
                parts.push(format!(
                    "{} / {}",
                    lapinux_core::util::format_bytes(job.downloaded),
                    lapinux_core::util::format_bytes(job.total)
                ));
            } else if job.downloaded > 0 {
                parts.push(lapinux_core::util::format_bytes(job.downloaded));
            }
            if speed_bps > 0 {
                parts.push(format!("{}/s", lapinux_core::util::format_bytes(speed_bps)));
            }
            parts.push(i18n::t("profile_page.on_lapinux").to_string());
            let state = parts.join(" · ");
            // Une mise à jour et une première installation ne se disent pas
            // de la même façon.
            let mut activity = serde_json::json!({
                "name": clamp(title),
                "details": clamp(&i18n::t_args(
                    if job.updating {
                        "discord.update"
                    } else {
                        "discord.download"
                    },
                    &[("name", title)],
                )),
                "state": clamp(&state),
            });
            if let Some(start) = elapsed_start(job.started, now) {
                activity["timestamps"] = serde_json::json!({ "start": start });
            }
            // La jaquette du jeu, quand elle se compose depuis l'appid : le
            // téléchargement montre l'affiche plutôt qu'une vignette seule.
            if let Some(store) = job_store(job) {
                let cover = (store == Store::Steam)
                    .then_some(job.appid)
                    .flatten()
                    .map(|appid| {
                        format!(
                            "https://cdn.cloudflare.steamstatic.com/steam/apps/{appid}/header.jpg"
                        )
                    });
                if let Some(cover) = cover {
                    activity["assets"] = serde_json::json!({
                        "large_image": cover,
                        "large_text": clamp(title),
                    });
                }
            }
            return Some(activity);
        }
    }
    if config.show_viewing && viewing.game.is_some() {
        // La fiche ouverte : la phrase, puis la boutique, le temps de jeu et
        // l'installation. La jaquette du jeu remplace toute icône de Lapinux.
        let title = viewing.game.as_deref().unwrap_or_default();
        // « Regarde » plutôt que « Joue » : la fiche est ouverte, pas la
        // partie. Le type reste Playing : c'est le seul sous lequel Discord
        // affiche toute la présence (jaquette, chrono, deux lignes), le type
        // Watching en masquant une partie.
        let mut activity = serde_json::json!({
            "name": clamp(title),
            "details": clamp(&i18n::t_args("discord.viewing", &[("name", title)])),
        });
        if let Some(note) = viewing
            .note
            .as_deref()
            .map(str::trim)
            .filter(|note| !note.is_empty())
        {
            activity["state"] = serde_json::json!(clamp(note));
        }
        return Some(activity);
    }
    // Les pages de l'application : leur titre est « Lapinux », le nom de
    // l'application qui les publie. La phrase et le contenu de la page vont
    // dessous, avec la marque de la boutique ouverte.
    if config.show_library && viewing.game.is_none() && !viewing.section.is_empty() {
        let mut activity = serde_json::json!({
            "name": "Lapinux",
            "details": clamp(&section_title(&viewing.section, viewing.store)),
        });
        if let Some(note) = viewing
            .note
            .as_deref()
            .map(str::trim)
            .filter(|note| !note.is_empty())
        {
            activity["state"] = serde_json::json!(clamp(note));
        }
        if viewing.section == "boutiques" {
            if let Some(store) = viewing.store {
                activity["assets"] = serde_json::json!({
                    "small_image": store_asset(store),
                    "small_text": store.label(),
                });
            }
        }
        return Some(activity);
    }
    None
}

/// Nom du jeu quand l'activité se publie sous l'application Lapinux : le
/// titre Discord est alors « Lapinux », et le nom du jeu doit rester lisible
/// dans la première ligne. Les jeux reconnus gardent leur application et
/// n'ont donc pas besoin de ce préfixe.
fn prefix_game_name(activity: &mut serde_json::Value, name: &str) {
    let name = clamp(name);
    if name.is_empty() {
        return;
    }
    let details = activity
        .get("details")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    activity["details"] = serde_json::Value::String(clamp(&if details.is_empty() {
        name
    } else {
        format!("{name} · {details}")
    }));
}

/// Application sous laquelle publier en partie : celle du jeu.
///
/// Lancée avant la fin du téléchargement du catalogue Discord, une partie a
/// gardé un `discord_app` vide : la boutique du jeu permet alors de refaire la
/// correspondance, sans relancer le jeu.
fn session_application(
    shared: &super::Shared,
    session: &lapinux_core::model::SessionInfo,
) -> Option<String> {
    let app = session.discord_app.trim();
    if !app.is_empty() {
        return Some(app.to_string());
    }
    let game = if session.store == Store::Steam {
        Game {
            appid: session.appid,
            store: Store::Steam,
            ..Default::default()
        }
    } else {
        let entry = shared.cached_store_entry(session.appid)?;
        Game {
            appid: entry.appid,
            name: entry.name,
            store: entry.store,
            store_id: entry.store_id,
            ..Default::default()
        }
    };
    app_for_game(shared, &game)
}

fn viewing_application(shared: &super::Shared, viewing: &Viewing) -> Option<String> {
    let game = Game {
        appid: viewing.appid.unwrap_or(0),
        name: viewing.game.clone().unwrap_or_default(),
        store: viewing.store.unwrap_or(Store::Steam),
        store_id: viewing.store_id.clone().unwrap_or_default(),
        ..Default::default()
    };
    app_for_game(shared, &game)
}

fn job_application(shared: &super::Shared, job: &Job) -> Option<String> {
    if job.kind != lapinux_core::model::JobKind::InstallGame {
        return None;
    }
    let appid = job.appid?;
    // Le moteur Lapinux installe les jeux Steam et porte leur appid ; les
    // boutiques ont leurs propres identifiants, relus dans leur bibliothèque.
    if job.backend.eq_ignore_ascii_case("Lapinux") || job.backend.eq_ignore_ascii_case("Steam") {
        let game = Game {
            appid,
            store: Store::Steam,
            ..Default::default()
        };
        return app_for_game(shared, &game);
    }
    let entry = shared.cached_store_entry(appid)?;
    let game = Game {
        appid: entry.appid,
        name: entry.name,
        store: entry.store,
        store_id: entry.store_id,
        ..Default::default()
    };
    app_for_game(shared, &game)
}

/// Application Discord officielle d'un jeu, Steam ou de boutique.
fn app_for_game(shared: &super::Shared, game: &Game) -> Option<String> {
    shared
        .discord_apps
        .lock()
        .ok()
        .and_then(|index| index.app_for(game).map(|app| app.id.clone()))
}

/// Débit du téléchargement entre deux relevés de la boucle de publication.
/// Le premier tour n'a pas de mesure ; Discord reste alors sans vitesse.
fn download_speed(sample: &mut Option<(i64, u64, String)>, job: Option<&Job>, now: i64) -> u64 {
    let Some(job) = job else {
        *sample = None;
        return 0;
    };
    let previous = sample.take();
    *sample = Some((now, job.downloaded, job.id.clone()));
    match previous {
        Some((at, bytes, id)) if id == job.id && now > at => {
            let delta = job.downloaded.saturating_sub(bytes) as f64;
            (delta / (now - at) as f64).round() as u64
        }
        _ => 0,
    }
}

/// Une ligne d'activité, bornée à ce que Discord accepte.
fn clamp(text: &str) -> String {
    const MAX: usize = 128;
    let text = text.trim();
    if text.chars().count() <= MAX {
        return text.to_string();
    }
    let mut out: String = text.chars().take(MAX - 1).collect();
    out.push('…');
    out
}

/// Début de la partie ou du téléchargement, en millisecondes Unix : Discord
/// affiche alors « il y a … ». Un instant absent ou à venir ne s'affiche pas.
fn elapsed_start(started: i64, now: i64) -> Option<i64> {
    (started > 0 && started <= now).then(|| started * 1000)
}

/// Couche de compatibilité affichée sous le jeu : « natif » quand le jeu
/// s'exécute sans Proton.
fn runtime_label(proton: &str) -> String {
    let proton = proton.trim();
    if proton.is_empty() || proton.eq_ignore_ascii_case("natif") {
        i18n::t("discord.native").to_string()
    } else {
        proton.to_string()
    }
}

impl super::Shared {
    /// Lance la publication Discord en tâche de fond : elle attend un client
    /// Discord et un identifiant, sans jamais bloquer le démarrage.
    pub fn start_discord_in_background(self: &Arc<Self>) {
        let shared = Arc::clone(self);
        std::thread::spawn(move || run(shared));
    }
}

/// L'interface a-t-elle tourné récemment ? Elle réécrit son marqueur toutes
/// les deux secondes ; passé la grâce, elle a disparu — tuée ou fermée sans
/// requête — et la présence n'a plus lieu d'être.
fn ui_recently_alive(paths: &lapinux_core::paths::LapinuxPaths) -> bool {
    const GRACE: Duration = Duration::from_secs(20);
    std::fs::metadata(paths.ui_marker())
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.elapsed().ok())
        .map(|age| age <= GRACE)
        .unwrap_or(false)
}

/// Attente de la boucle, réveillée dès que la présence n'a plus lieu d'être :
/// sans cela, l'activité survivrait cinq secondes de plus après la fermeture.
fn idle(shared: &super::Shared, total: Duration) {
    const STEP: Duration = Duration::from_millis(250);
    let mut left = total;
    while left > Duration::ZERO {
        if !shared
            .discord_active
            .load(std::sync::atomic::Ordering::Relaxed)
            || !ui_recently_alive(&shared.paths)
        {
            return;
        }
        let nap = left.min(STEP);
        std::thread::sleep(nap);
        left = left.saturating_sub(nap);
    }
}

/// Boucle de publication : une activité toutes les cinq secondes, seulement si
/// elle a changé, et une reconnexion douce quand Discord disparaît.
pub fn run(shared: Arc<super::Shared>) {
    let mut client: Option<Presence> = None;
    let mut published: Option<serde_json::Value> = None;
    // Application sous laquelle la connexion courante est ouverte.
    let mut connected: Option<String> = None;
    let mut warned = false;
    // Relevé précédent du téléchargement : le débit se lit entre deux tours.
    let mut download_sample: Option<(i64, u64, String)> = None;
    loop {
        // L'interface coupée (fermeture de l'application) : l'activité est
        // retirée, et rien n'est republié tant qu'elle ne revient pas. Le
        // marqueur de vie couvre aussi le cas d'une application tuée sans
        // message de fermeture.
        let active = shared
            .discord_active
            .load(std::sync::atomic::Ordering::Relaxed);
        if !active || !ui_recently_alive(&shared.paths) {
            if let Some(mut client) = client.take() {
                client.clear();
                tracing::info!("présence Discord retirée");
            }
            connected = None;
            published = None;
            warned = false;
            std::thread::sleep(Duration::from_secs(1));
            continue;
        }
        let config = shared.config_snapshot().discord;
        if !config.enabled {
            if let Some(client) = client.as_mut() {
                client.clear();
            }
            client = None;
            connected = None;
            published = None;
            warned = false;
            idle(&shared, Duration::from_secs(5));
            continue;
        }
        let viewing = shared
            .discord_view
            .lock()
            .map(|view| view.clone())
            .unwrap_or_default();
        let session = shared.session_info();
        let job = shared.jobs.lock().ok().and_then(|jobs| {
            jobs.iter()
                .find(|job| job.state == lapinux_core::model::JobState::Running)
                .cloned()
        });
        let now = lapinux_core::util::now_unix();
        let speed = download_speed(&mut download_sample, job.as_ref(), now);
        let mut activity = activity(
            &config,
            &viewing,
            session.as_ref(),
            job.as_ref(),
            speed,
            now,
        );
        // L'icône du jeu remplace la marque de la boutique : Discord la place
        // en petit sur la jaquette, ou en grand quand il n'y a pas de jaquette.
        let icon = if config.show_game && session.is_some() {
            session.as_ref().and_then(|session| {
                presence_icon(
                    &shared,
                    Some(session.store),
                    (session.store == Store::Steam).then_some(session.appid),
                    Some(session.appid),
                    &session.name,
                )
            })
        } else if config.show_downloads && job.is_some() {
            job.as_ref().and_then(|job| {
                let title = if job.title.trim().is_empty() {
                    job.label.trim()
                } else {
                    job.title.trim()
                };
                let store = job_store(job);
                let appid = (store == Some(Store::Steam)).then_some(job.appid).flatten();
                presence_icon(&shared, store, appid, job.appid, title)
            })
        } else if config.show_viewing {
            viewing.game.as_deref().and_then(|name| {
                presence_icon(
                    &shared,
                    viewing.store,
                    viewing.appid,
                    viewing.art_appid,
                    name,
                )
            })
        } else {
            None
        };
        if let (Some(activity), Some(icon)) = (activity.as_mut(), icon) {
            add_game_icon(activity, &icon);
        }
        // Le titre d'une activité Discord est le nom de l'application qui la
        // publie. En partie, Lapinux utilise l'application du jeu quand il la
        // connaît ; sinon il publie sous la sienne et met le nom du jeu dans
        // la première ligne. Un téléchargement ou une page gardent aussi son
        // nom.
        let application = if config.show_game && session.is_some() {
            session
                .as_ref()
                .and_then(|session| session_application(&shared, session))
                .or_else(client_id)
        } else if config.show_downloads && job.is_some() {
            job.as_ref()
                .and_then(|job| job_application(&shared, job))
                .or_else(client_id)
        } else if config.show_viewing && viewing.game.is_some() {
            viewing_application(&shared, &viewing).or_else(client_id)
        } else if config.show_library && viewing.game.is_none() && !viewing.section.is_empty() {
            client_id()
        } else {
            None
        };
        let Some(application) = application else {
            if let Some(mut client) = client.take() {
                client.clear();
            }
            connected = None;
            published = None;
            idle(&shared, Duration::from_secs(5));
            continue;
        };
        if config.show_game
            && session.is_some()
            && client_id().as_deref() == Some(application.as_str())
        {
            if let (Some(activity), Some(session)) = (activity.as_mut(), session.as_ref()) {
                prefix_game_name(activity, &session.name);
            }
        }
        if client.is_some() && connected.as_deref() != Some(application.as_str()) {
            if let Some(client) = client.as_mut() {
                client.clear();
            }
            client = None;
            connected = None;
            published = None;
        }
        if client.is_none() {
            match Presence::connect(&application) {
                Some(found) => {
                    tracing::info!("Rich Presence : application Discord {application}");
                    client = Some(found);
                    connected = Some(application);
                    published = None;
                    warned = false;
                }
                None => {
                    if !warned {
                        tracing::info!("Discord injoignable : présence en attente");
                        warned = true;
                    }
                    idle(&shared, Duration::from_secs(5));
                    continue;
                }
            }
        }
        // Rien de neuf : pas de commande, mais un sommeil malgré tout — sans
        // lui, la boucle tournerait à plein cœur.
        if activity == published {
            idle(&shared, Duration::from_secs(5));
            continue;
        }
        let delivered = {
            let Some(client) = client.as_mut() else {
                idle(&shared, Duration::from_secs(5));
                continue;
            };
            match activity.clone() {
                Some(value) => client.set_activity(&value),
                None => {
                    client.clear();
                    true
                }
            }
        };
        if delivered {
            published = activity;
        } else {
            // Connexion perdue ou activité refusée : on repart de zéro au
            // prochain tour, avec une poignée de main neuve.
            client = None;
            published = None;
        }
        idle(&shared, Duration::from_secs(5));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lapinux_core::model::{Job, SessionInfo};

    const NOW: i64 = 1_700_000_100;

    fn session() -> SessionInfo {
        serde_json::from_value(serde_json::json!({
            "appid": 2830030,
            "name": "MOTORSLICE",
            "pid": 42,
            "started": 1_700_000_000,
            "proton": "GE-Proton",
            "fps_cap": 165,
            "overlay_open": false,
            "input_profile": "defaut",
            "log_path": "/tmp/session.log",
            "cover": "https://cdn.example/cover.jpg",
            "fps": { "current": 158, "min": 45, "avg": 78, "max": 120 },
            "store": "steam"
        }))
        .unwrap()
    }

    fn download() -> Job {
        serde_json::from_value(serde_json::json!({
            "id": "telechargement-1",
            "kind": "InstallGame",
            "label": "MOTORSLICE (2830030)",
            "title": "MOTORSLICE",
            "appid": 2830030,
            "state": "Running",
            "progress": 0.45,
            "downloaded": 1_200_000_000u64,
            "total": 2_600_000_000u64,
            "message": "",
            "backend": "Lapinux",
            "started": 1_700_000_000
        }))
        .unwrap()
    }

    const CONFIG: DiscordConfig = DiscordConfig {
        enabled: true,
        show_library: true,
        show_game: true,
        show_downloads: true,
        show_viewing: true,
    };

    /// Les textes attendus se composent depuis les catalogues : les tests
    /// valident l'assemblage, pas une langue figée.
    fn playing_details(fps: Option<(u32, u32)>) -> String {
        match fps {
            Some((current, avg)) if avg > 0 && avg != current => i18n::t_fmt(
                "discord.playing_fps_avg",
                &[current.to_string(), avg.to_string()],
            ),
            Some((current, _)) => i18n::t_fmt("discord.playing_fps", &[current.to_string()]),
            None => i18n::t("discord.playing").to_string(),
        }
    }

    fn game_state(runtime: &str) -> String {
        i18n::t_args(
            "discord.game_state",
            &[
                ("store", "Steam"),
                ("runtime", runtime),
                ("mention", i18n::t("profile_page.on_lapinux")),
            ],
        )
    }

    fn download_state(percent: u32, downloaded: u64, total: u64, speed: u64) -> String {
        let mut parts = vec![
            "Steam".to_string(),
            format!("{percent} %"),
            format!(
                "{} / {}",
                lapinux_core::util::format_bytes(downloaded),
                lapinux_core::util::format_bytes(total)
            ),
        ];
        if speed > 0 {
            parts.push(format!("{}/s", lapinux_core::util::format_bytes(speed)));
        }
        parts.push(i18n::t("profile_page.on_lapinux").to_string());
        parts.join(" · ")
    }

    #[test]
    fn presence_shows_fps_runtime_and_elapsed_time() {
        let activity = activity(&CONFIG, &Viewing::default(), Some(&session()), None, 0, NOW)
            .expect("activité attendue");
        assert_eq!(activity["name"], "MOTORSLICE");
        assert_eq!(activity["details"], playing_details(Some((158, 78))));
        assert_eq!(activity["state"], game_state("GE-Proton"));
        assert_eq!(activity["timestamps"]["start"], 1_700_000_000_000i64);
    }

    #[test]
    fn presence_waits_for_the_game_icon() {
        let activity = activity(&CONFIG, &Viewing::default(), Some(&session()), None, 0, NOW)
            .expect("activité attendue");
        assert!(activity.get("assets").is_none());
    }

    #[test]
    fn a_game_outside_discords_catalog_keeps_its_name_under_lapinux() {
        let mut value = activity(&CONFIG, &Viewing::default(), Some(&session()), None, 0, NOW)
            .expect("activité attendue");
        prefix_game_name(&mut value, "LIMINAL STATE");
        assert_eq!(
            value["details"],
            format!("LIMINAL STATE · {}", playing_details(Some((158, 78))))
        );
    }

    #[test]
    fn presence_without_measurement_stays_sober() {
        let mut info = session();
        info.fps = None;
        let activity = activity(&CONFIG, &Viewing::default(), Some(&info), None, 0, NOW)
            .expect("activité attendue");
        assert_eq!(activity["details"], playing_details(None));
        assert_eq!(activity["state"], game_state("GE-Proton"));
    }

    #[test]
    fn a_native_game_says_natif_and_skips_a_future_start() {
        let mut info = session();
        info.proton = String::new();
        info.started = NOW + 60;
        let activity = activity(&CONFIG, &Viewing::default(), Some(&info), None, 0, NOW)
            .expect("activité attendue");
        assert_eq!(activity["state"], game_state(&i18n::t("discord.native")));
        assert!(activity.get("timestamps").is_none());
    }

    #[test]
    fn a_download_reports_progress_speed_and_time() {
        let activity = activity(
            &CONFIG,
            &Viewing::default(),
            None,
            Some(&download()),
            24_000_000,
            NOW,
        )
        .expect("activité attendue");
        assert_eq!(
            activity["details"],
            i18n::t_args("discord.download", &[("name", "MOTORSLICE")])
        );
        assert_eq!(
            activity["state"],
            download_state(45, 1_200_000_000, 2_600_000_000, 24_000_000)
        );
        assert_eq!(activity["timestamps"]["start"], 1_700_000_000_000i64);
        assert_eq!(
            activity["assets"]["large_image"],
            "https://cdn.cloudflare.steamstatic.com/steam/apps/2830030/header.jpg"
        );
        assert!(activity["assets"].get("small_image").is_none());
    }

    #[test]
    fn a_store_session_never_borrows_the_store_badge() {
        for store in [Store::Epic, Store::Gog, Store::Itch] {
            let mut info = session();
            info.store = store;
            info.cover = String::new();
            let activity = activity(&CONFIG, &Viewing::default(), Some(&info), None, 0, NOW)
                .expect("activité attendue");
            assert!(activity.get("assets").is_none());
        }
    }

    #[test]
    fn a_store_download_has_no_store_badge() {
        // La marque de la boutique n'est jamais la vignette d'un jeu : c'est
        // l'icône du jeu que la boucle ajoute.
        let mut job = download();
        job.backend = "GOG".into();
        let activity = activity(&CONFIG, &Viewing::default(), None, Some(&job), 0, NOW)
            .expect("activité attendue");
        assert!(activity.get("assets").is_none());
    }

    #[test]
    fn the_game_icon_is_the_large_image() {
        let mut presence = activity(&CONFIG, &Viewing::default(), Some(&session()), None, 0, NOW)
            .expect("activité attendue");
        add_game_icon(&mut presence, "https://cdn.example/icon.png");
        assert_eq!(
            presence["assets"]["large_image"],
            "https://cdn.example/icon.png"
        );
        assert_eq!(presence["assets"]["large_text"], "MOTORSLICE");
        assert!(presence["assets"].get("small_image").is_none());
    }

    #[test]
    fn sgdb_icon_picker_prefers_the_official_square_png() {
        let value = serde_json::json!({
            "data": [
                {"url": "https://cdn.example/wide.png", "width": 512, "height": 256, "mime": "image/png"},
                {"url": "https://cdn.example/fan.png", "width": 512, "height": 512, "mime": "image/png", "style": "custom"},
                {"url": "https://cdn.example/official.png", "width": 128, "height": 128, "mime": "image/png", "style": "official"},
                {"url": "https://cdn.example/nsfw.png", "width": 512, "height": 512, "mime": "image/png", "nsfw": true}
            ]
        });
        assert_eq!(
            sgdb_icon_pick(&value).as_deref(),
            Some("https://cdn.example/official.png")
        );
    }

    #[test]
    fn the_cached_sgdb_icon_url_is_reused() {
        let dir = std::env::temp_dir().join(format!(
            "lapinux-icon-url-{}-{}",
            std::process::id(),
            line!()
        ));
        std::fs::create_dir_all(&dir).expect("dossier temporaire");
        assert_eq!(cached_sgdb_icon_url(&dir, 42), None);
        std::fs::write(
            dir.join("sgdb-icon-42.url"),
            "https://cdn.example/game.png\n",
        )
        .expect("adresse écrite");
        assert_eq!(
            cached_sgdb_icon_url(&dir, 42).as_deref(),
            Some("https://cdn.example/game.png")
        );
        // Une adresse non HTTPS ne peut pas être envoyée à Discord.
        std::fs::write(dir.join("sgdb-icon-43.url"), "http://cdn.example/x.png")
            .expect("adresse écrite");
        assert_eq!(cached_sgdb_icon_url(&dir, 43), None);
        // Le daemon garde aussi sa dernière résolution, séparément de la fiche.
        assert_eq!(cached_presence_icon_url(&dir, 44), None);
        remember_presence_icon_url(&dir, 44, "https://cdn.example/other.png");
        assert_eq!(
            cached_presence_icon_url(&dir, 44).as_deref(),
            Some("https://cdn.example/other.png")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_game_without_a_cover_keeps_its_icon() {
        let mut info = session();
        info.cover = String::new();
        let activity = activity(&CONFIG, &Viewing::default(), Some(&info), None, 0, NOW)
            .expect("activité attendue");
        assert!(activity.get("assets").is_none());
    }

    #[test]
    fn an_update_reads_as_an_update() {
        let mut job = download();
        job.updating = true;
        let activity = activity(&CONFIG, &Viewing::default(), None, Some(&job), 0, NOW)
            .expect("activité attendue");
        assert_eq!(
            activity["details"],
            i18n::t_args("discord.update", &[("name", "MOTORSLICE")])
        );
        assert_eq!(
            activity["state"],
            download_state(45, 1_200_000_000, 2_600_000_000, 0)
        );
    }

    #[test]
    fn a_store_game_sheet_has_no_store_badge() {
        let viewing = Viewing {
            section: "library".into(),
            game: Some("MOTORSLICE".into()),
            store: Some(Store::Epic),
            ..Viewing::default()
        };
        let activity = activity(&CONFIG, &viewing, None, None, 0, NOW).expect("activité attendue");
        assert!(activity.get("assets").is_none());
    }

    #[test]
    fn a_game_sheet_shows_its_note_without_the_cover() {
        let viewing = Viewing {
            section: "library".into(),
            game: Some("MOTORSLICE".into()),
            note: Some("Steam · 12 h de jeu · installé · sur Lapinux".into()),
            cover: Some("https://cdn.example/cover.jpg".into()),
            appid: Some(2830030),
            ..Viewing::default()
        };
        let activity = activity(&CONFIG, &viewing, None, None, 0, NOW).expect("activité attendue");
        assert_eq!(activity["name"], "MOTORSLICE");
        assert_eq!(
            activity["details"],
            i18n::t_args("discord.viewing", &[("name", "MOTORSLICE")])
        );
        assert_eq!(
            activity["state"],
            "Steam · 12 h de jeu · installé · sur Lapinux"
        );
        assert!(activity.get("assets").is_none());
    }

    #[test]
    fn a_page_publishes_under_the_lapinux_app() {
        let viewing = Viewing {
            section: "library".into(),
            note: Some("42 jeux · 12 installés".into()),
            ..Viewing::default()
        };
        let activity = activity(&CONFIG, &viewing, None, None, 0, NOW).expect("activité attendue");
        assert_eq!(activity["name"], "Lapinux");
        assert_eq!(activity["details"], i18n::t("discord.page_library"));
        assert_eq!(activity["state"], "42 jeux · 12 installés");
        assert!(activity.get("assets").is_none());
    }

    #[test]
    fn a_store_page_carries_its_badge() {
        let viewing = Viewing {
            section: "boutiques".into(),
            store: Some(Store::Gog),
            ..Viewing::default()
        };
        let activity = activity(&CONFIG, &viewing, None, None, 0, NOW).expect("activité attendue");
        assert_eq!(
            activity["details"],
            i18n::t_args("discord.page_stores", &[("store", "GOG")])
        );
        assert_eq!(activity["assets"]["small_image"], "gog");
    }

    #[test]
    fn presence_wording_comes_from_the_catalogs() {
        assert_eq!(i18n::t_in("fr", "discord.playing"), "En jeu");
        assert_eq!(i18n::t_in("en", "discord.playing"), "Playing");
        assert_eq!(i18n::t_in("en", "profile_page.on_lapinux"), "on Lapinux");
        assert_eq!(i18n::t_in("en", "discord.viewing"), "Viewing {name}");
        assert_eq!(
            i18n::t_in("en", "discord.page_library"),
            "Browsing their library"
        );
        assert_eq!(i18n::t_in("en", "discord.native"), "native");
    }

    #[test]
    fn a_long_line_stays_within_discord_limit() {
        let long = "x".repeat(200);
        assert_eq!(clamp(&long).chars().count(), 128);
        assert!(clamp(&long).ends_with('…'));
        assert_eq!(clamp("  court  "), "court");
    }
}
