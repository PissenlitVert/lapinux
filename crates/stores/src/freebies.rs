//! Offres gratuites à conserver chez Steam, GOG et itch.io.
//!
//! Les boutiques ne publient pas de flux unique : Steam cache les promotions
//! derrière sa recherche, GOG réserve ses giveaways à sa page d'accueil et
//! itch.io ne distingue pas un jeu gratuit d'une offre à durée limitée. Le flux
//! public de FreeToKeep assemble ces trois sources et ne garde que les offres
//! que l'on peut **conserver** : les free-to-play et les free weekends, eux,
//! n'y figurent jamais. C'est exactement la sélection de la page « Jeux
//! gratuits ».
//!
//! Chaque fiche est ensuite ouverte une fois pour retrouver l'adresse de la
//! boutique : le flux ne donne que sa propre page, et c'est la boutique qui
//! sait terminer la récupération.

use crate::common::{self, Error, Result};
use lapinux_core::stores::{FreeGame, Store};
use serde_json::Value;
use std::sync::Mutex;
use std::time::Duration;

const API: &str = "https://freetokeep.gg/api/v1/offers";
const LIMIT: usize = 40;
const CACHE_SECONDS: i64 = 600;
const BROWSER_UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

/// Les offres changent au plus quelques fois par jour ; la page peut être
/// rouverte sans relancer une quarantaine de requêtes.
static CACHE: Mutex<Option<(String, i64, Vec<FreeGame>)>> = Mutex::new(None);

/// Offres gratuites à conserver, hors Epic (son flux officiel est plus riche).
pub fn free_games(locale: &str) -> Result<Vec<FreeGame>> {
    let now = lapinux_core::util::now_unix();
    if let Ok(cache) = CACHE.lock() {
        if let Some((cached_locale, at, games)) = cache.as_ref() {
            if cached_locale == locale && now - at < CACHE_SECONDS {
                return Ok(games.clone());
            }
        }
    }

    let mut games = Vec::new();
    let mut errors = Vec::new();
    for store in [Store::Steam, Store::Gog, Store::Itch] {
        match fetch_store(store) {
            Ok(mut offers) => games.append(&mut offers),
            Err(error) => errors.push(format!("{} : {error}", store.label())),
        }
    }
    games.sort_by(|left, right| {
        left.upcoming
            .cmp(&right.upcoming)
            .then((left.end == 0).cmp(&(right.end == 0)))
            .then(left.end.cmp(&right.end))
            .then(left.title.cmp(&right.title))
    });
    games.dedup_by(|left, right| left.id == right.id);

    if games.is_empty() && !errors.is_empty() {
        return Err(Error::Other(errors.join(" · ")));
    }
    if let Ok(mut cache) = CACHE.lock() {
        *cache = Some((locale.to_string(), now, games.clone()));
    }
    Ok(games)
}

fn fetch_store(store: Store) -> Result<Vec<FreeGame>> {
    let url = format!("{API}?store={}&limit={LIMIT}", store.key());
    let value = common::get_json_as(&url, None, BROWSER_UA)?;
    let items = value
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::Parse("liste d'offres absente".into()))?;
    let mut games = Vec::new();
    for item in items {
        if let Some(game) = offer_from_api(store, item) {
            games.push(enrich(game));
        }
    }
    Ok(games)
}

fn offer_from_api(store: Store, item: &Value) -> Option<FreeGame> {
    let title = item
        .get("title")
        .and_then(Value::as_str)?
        .trim()
        .to_string();
    let page = item.get("url").and_then(Value::as_str)?.trim().to_string();
    if title.is_empty() || page.is_empty() {
        return None;
    }
    let end = item
        .get("endsAt")
        .and_then(Value::as_str)
        .and_then(parse_timestamp)
        .unwrap_or(0);
    let start = item
        .get("startsAt")
        .and_then(Value::as_str)
        .and_then(parse_timestamp)
        .or_else(|| {
            item.get("publishedAt")
                .and_then(Value::as_str)
                .and_then(parse_timestamp)
        })
        .unwrap_or(0);
    let value = item
        .get("originalPrice")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let image = item
        .get("imageUrl")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let tags = item
        .get("tags")
        .and_then(Value::as_array)
        .map(|tags| {
            tags.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let suffix = item
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or(&page)
        .to_string();
    Some(FreeGame {
        store,
        id: format!("{store}:{suffix}", store = store.key()),
        title,
        description: tags.join(" · "),
        cover: image,
        url: page.clone(),
        seller: store.label().to_string(),
        value,
        start,
        end,
        upcoming: start > lapinux_core::util::now_unix() && start > 0,
        platforms: vec!["pc".into()],
        owned: false,
        // La boutique termine elle-même l'offre : sa page est le seul chemin
        // commun aux trois enseignes.
        // Steam se récupère par licence, mais GOG et itch.io n'ont pas d'API
        // de récupération : leur page est le seul chemin.
        browser_only: store != Store::Steam,
        offers: Vec::new(),
        owned_offers: Vec::new(),
    })
}

/// Remplace l'adresse du flux par celle de la boutique, et récupère le visuel
/// quand le flux n'en publie pas.
fn enrich(mut game: FreeGame) -> FreeGame {
    let page = game.url.clone();
    let Some(html) = get_page(&page) else {
        game.browser_only = true;
        return game;
    };
    if let Some(url) = store_url(&html, game.store) {
        game.id = offer_id(game.store, &url).unwrap_or(game.id);
        game.url = url;
    } else {
        game.browser_only = true;
    }
    // Le flux laisse souvent le visuel itch.io vide, et sa propre page
    // OpenGraph est une bannière générique FreeToKeep. Les captures de la
    // fiche agrégée, elles, pointent déjà vers l'art du jeu ; on les préfère
    // à cette bannière.
    if placeholder_cover(&game.cover) {
        let image = if game.store == Store::Itch {
            first_itch_image(&html).or_else(|| meta_content(&html, "og:image"))
        } else {
            meta_content(&html, "og:image")
        };
        if let Some(image) = image {
            game.cover = image;
        }
    }
    let description = meta_content(&html, "description").unwrap_or_default();
    if !description.is_empty() {
        game.description = description;
    }
    game
}

fn get_page(url: &str) -> Option<String> {
    let response = ureq::get(url)
        .set("User-Agent", BROWSER_UA)
        .set(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .set("Accept-Language", "fr-FR,fr;q=0.9,en;q=0.8")
        .timeout(Duration::from_secs(12))
        .call()
        .ok()?;
    response.into_string().ok()
}

/// Première adresse de boutique trouvée dans la page du flux.
fn store_url(html: &str, store: Store) -> Option<String> {
    for href in hrefs(html) {
        let href = href.replace("&amp;", "&");
        let candidate = match store {
            Store::Steam => href
                .starts_with("https://store.steampowered.com/app/")
                .then_some(href),
            Store::Gog => (href.contains("gog.com/") && href.contains("/game/")).then_some(href),
            Store::Itch => {
                (href.contains(".itch.io/") && !href.contains("itch.io/t/")).then_some(href)
            }
            Store::Epic => None,
        };
        if let Some(candidate) = candidate {
            return Some(
                candidate
                    .split('?')
                    .next()
                    .unwrap_or(&candidate)
                    .to_string(),
            );
        }
    }
    None
}

fn hrefs(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(index) = rest.find("href=\"") {
        rest = &rest[index + 6..];
        let Some(end) = rest.find('"') else {
            break;
        };
        out.push(rest[..end].to_string());
        rest = &rest[end + 1..];
    }
    out
}

fn meta_content(html: &str, name: &str) -> Option<String> {
    let markers = [format!("property=\"{name}\""), format!("name=\"{name}\"")];
    for tag in html.split("<meta").skip(1) {
        let Some(end) = tag.find('>') else {
            continue;
        };
        let tag = &tag[..end];
        if !markers.iter().any(|marker| tag.contains(marker)) {
            continue;
        }
        let Some(content) = attribute(tag, "content") else {
            continue;
        };
        return Some(content.replace("&amp;", "&").replace("&#39;", "'"));
    }
    None
}

/// Valeur d'un attribut HTML, dans un tag déjà délimité.
fn attribute(tag: &str, name: &str) -> Option<String> {
    let marker = format!("{name}=\"");
    let start = tag.find(&marker)? + marker.len();
    let end = tag[start..].find('"')?;
    let value = tag[start..start + end].trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// Une jaquette absente ou la bannière générique de l'agrégateur.
fn placeholder_cover(cover: &str) -> bool {
    let cover = cover.trim();
    cover.is_empty() || cover.contains("freetokeep.gg")
}

/// Première image de jeu trouvée dans une fiche agrégée : les captures
/// itch.io pointent vers `img.itch.zone` et montrent le vrai jeu.
fn first_itch_image(html: &str) -> Option<String> {
    for marker in ["https://img.itch.zone/", "//img.itch.zone/"] {
        let Some(index) = html.find(marker) else {
            continue;
        };
        let rest = &html[index..];
        let end = rest
            .find(|character: char| {
                character == '"'
                    || character == '\''
                    || character == '\\'
                    || character == '<'
                    || character.is_whitespace()
            })
            .unwrap_or(rest.len());
        let image = rest[..end].trim();
        if image.len() > marker.len() {
            return Some(if image.starts_with("//") {
                format!("https:{image}")
            } else {
                image.to_string()
            });
        }
    }
    None
}

fn offer_id(store: Store, url: &str) -> Option<String> {
    let path = url.split('?').next()?;
    let id = match store {
        Store::Steam => path.split("/app/").nth(1)?.split('/').next()?.to_string(),
        Store::Gog => path.split("/game/").nth(1)?.trim_matches('/').to_string(),
        Store::Itch => {
            let rest = path.split("://").nth(1)?;
            rest.trim_matches('/').to_string()
        }
        Store::Epic => return None,
    };
    (!id.is_empty()).then(|| format!("{store}:{id}", store = store.key()))
}

/// Date RFC 3339 du flux (`2026-11-12T16:00:00.000Z`) en secondes Unix.
fn parse_timestamp(text: &str) -> Option<i64> {
    let text = text.trim();
    let date = text.get(0..10)?;
    let mut parts = date.split('-');
    let year = parts.next()?.parse::<i32>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let day = parts.next()?.parse::<u32>().ok()?;
    if !(1970..=9999).contains(&year) || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let time = text.get(11..19).unwrap_or("00:00:00");
    let mut parts = time.split(':');
    let hour = parts.next()?.parse::<i64>().ok()?;
    let minute = parts.next()?.parse::<i64>().ok()?;
    let second = parts.next()?.parse::<i64>().ok()?;
    if !(0..24).contains(&hour) || !(0..60).contains(&minute) || !(0..60).contains(&second) {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second)
}

/// Jours depuis l'époque grégorienne, sans dépendance de calendrier.
fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month = month as i32;
    let day_of_year =
        (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day as i32 - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    (era * 146_097 + day_of_era - 719_468) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_follow_the_api_format() {
        assert_eq!(parse_timestamp("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(
            parse_timestamp("2026-11-12T16:00:00.000Z"),
            Some(1_794_499_200)
        );
        assert_eq!(parse_timestamp("pas une date"), None);
    }

    #[test]
    fn the_store_link_wins_over_the_aggregator_page() {
        let html = r#"
            <a href="https://freetokeep.gg/game/penance">Penance</a>
            <a href="https://pehriska.itch.io/penance">Claim Game</a>
            <meta property="og:image" content="https://img.example/penance.png">
        "#;
        assert_eq!(
            store_url(html, Store::Itch).as_deref(),
            Some("https://pehriska.itch.io/penance")
        );
        assert_eq!(
            meta_content(html, "og:image").as_deref(),
            Some("https://img.example/penance.png")
        );
    }

    #[test]
    fn steam_urls_keep_the_appid() {
        let html =
            r#"<a href="https://store.steampowered.com/app/730/CounterStrike_2/?snr=1">Steam</a>"#;
        let url = store_url(html, Store::Steam).unwrap();
        assert_eq!(
            url,
            "https://store.steampowered.com/app/730/CounterStrike_2/"
        );
        assert_eq!(offer_id(Store::Steam, &url).as_deref(), Some("steam:730"));
    }

    #[test]
    fn itch_images_beat_the_aggregator_banner() {
        let html = r#"
            <meta property="og:image" content="https://freetokeep.gg/game/x/opengraph-image">
            <img src="https://img.itch.zone/abc/794x1000/art.png" alt="screenshot">
        "#;
        assert!(placeholder_cover(
            "https://freetokeep.gg/game/x/opengraph-image"
        ));
        assert_eq!(
            first_itch_image(html).as_deref(),
            Some("https://img.itch.zone/abc/794x1000/art.png")
        );
    }

    #[test]
    fn meta_tags_work_in_either_attribute_order() {
        let html = r#"<meta content="https://img.itch.zone/cover.gif" property="og:image">"#;
        assert_eq!(
            meta_content(html, "og:image").as_deref(),
            Some("https://img.itch.zone/cover.gif")
        );
    }
}
