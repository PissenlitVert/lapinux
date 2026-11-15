//! itch.io : la seule des trois boutiques dont l'API est officielle.
//!
//! La connexion se fait par une **clé API personnelle**, créée par
//! l'utilisateur sur son compte : itch n'a pas de flux OAuth que ce launcher
//! puisse ouvrir. La bibliothèque vient de `profile/owned-keys`, les fichiers
//! de `games/{id}/uploads`, et le téléchargement d'un envoi redirige vers son
//! stockage.
//!
//! Le nom d'un jeu acheté n'est pas toujours dans la liste des clés : la fiche
//! du jeu est alors demandée, une fois, quand elle répond.

use crate::common::{self, Error, InstallPlan, Result, Step};
use lapinux_core::i18n::t;
use lapinux_core::stores::{store_appid, Store, StoreDetails, StoreGame, StoreNews, StoreTokens};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

const API: &str = "https://api.itch.io";
/// Ancienne racine, encore la seule à rendre `/me` pour certaines clés.
const LEGACY: &str = "https://itch.io/api/1";
/// Une page de jeu itch répond à l'agent d'un navigateur ; celui du launcher
/// peut être servi comme une requête de robot.
const PAGE_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36";

/// La page où créer la clé : itch n'a pas de flux OAuth qu'un launcher peut
/// ouvrir, la connexion passe par une clé personnelle.
pub fn login_link() -> crate::common::LoginLink {
    crate::common::LoginLink {
        url: "https://itch.io/user/settings/api-keys".into(),
        hint: "itch.io ne se connecte pas par navigateur : créez une clé d'API sur cette page (portée \
               « toutes les applications »), puis collez-la ci-dessous."
            .into(),
    }
}

/// Valide une clé API et enregistre le compte.
pub fn login_with_key(key: &str) -> Result<StoreTokens> {
    let key = key.trim();
    if key.is_empty() {
        return Err(Error::Other("clé API vide".into()));
    }
    let profile = profile(key)?;
    let account = profile_account(&profile).ok_or_else(|| Error::Refused {
        status: 401,
        message: "clé API refusée par itch.io".into(),
    })?;
    Ok(StoreTokens {
        store: Store::Itch,
        account,
        access_token: key.to_string(),
        refresh_token: String::new(),
        expires_at: 0,
        user_id: profile_user_id(&profile).unwrap_or_default(),
    })
}

/// Fiche du compte : `api.itch.io/profile`, puis `/me` en repli.
fn profile(key: &str) -> Result<Value> {
    match common::get_json(&format!("{API}/profile?api_key={key}"), None) {
        Ok(value) => Ok(value),
        Err(Error::Refused { .. }) | Err(Error::Expired) => {
            common::get_json(&format!("{LEGACY}/{key}/me"), None)
        }
        Err(error) => Err(error),
    }
}

/// Nom du compte, quelle que soit la forme de la réponse.
fn profile_account(profile: &Value) -> Option<String> {
    for pointer in [
        "/user/username",
        "/user/display_name",
        "/username",
        "/display_name",
    ] {
        if let Some(name) = profile.pointer(pointer).and_then(Value::as_str) {
            if !name.trim().is_empty() {
                return Some(name.trim().to_string());
            }
        }
    }
    None
}

fn profile_user_id(profile: &Value) -> Option<String> {
    profile
        .pointer("/user/id")
        .and_then(Value::as_i64)
        .map(|id| id.to_string())
}

pub fn account_label(tokens: &StoreTokens) -> Result<String> {
    let profile = profile(&tokens.access_token)?;
    profile_account(&profile).ok_or_else(|| Error::Parse("compte itch.io sans nom".into()))
}

/// Visuels d'une fiche itch : l'API ne publie qu'une couverture, large ; elle
/// sert aussi de jaquette, et la tuile verticale la recadre sans la déformer.
fn cover_formats(game: &Value) -> (String, String, String) {
    let text = |key: &str| {
        game.get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let cover = text("cover_url");
    (text("icon_url"), cover.clone(), cover)
}

/// Ce que la page publique ajoute à la petite fiche de l'API : la présentation
/// entière, la galerie et l'éventuelle vidéo d'accueil.
#[derive(Default)]
struct PageDetails {
    description: String,
    screenshots: Vec<String>,
    trailer: String,
    trailer_poster: String,
}

/// Lit la page publique d'un jeu. L'API itch ne publie que le titre, l'accroche
/// et la couverture : la présentation et les captures sont dans le HTML.
fn page_details(site: &str) -> Option<PageDetails> {
    if site.trim().is_empty() {
        return None;
    }
    let html = common::get_as(site, None, PAGE_AGENT).ok()?;
    let description = div_inner(&html, "class=\"formatted_description")
        .map(html_to_markdown)
        .unwrap_or_default();
    let screenshots = page_screenshots(&html);
    let trailer_poster = screenshots.first().cloned().unwrap_or_default();
    let trailer = page_youtube_trailer(&html).unwrap_or_default();
    if description.trim().is_empty() && screenshots.is_empty() && trailer.is_empty() {
        return None;
    }
    Some(PageDetails {
        description,
        screenshots,
        trailer,
        trailer_poster,
    })
}

/// Contenu d'un `<div>` qui porte `marker` dans ses attributs, en tenant compte
/// des `<div>` imbriqués de la présentation.
fn div_inner<'a>(html: &'a str, marker: &str) -> Option<&'a str> {
    let marker_at = html.find(marker)?;
    let open_start = html[..marker_at].rfind("<div")?;
    let open_end = open_start + html[open_start..].find('>')?;
    let content_start = open_end + 1;
    let mut depth = 1usize;
    let mut cursor = content_start;
    while cursor < html.len() {
        let rest = &html[cursor..];
        if rest.starts_with("<div") && rest[4..].starts_with([' ', '>', '\t', '\n']) {
            depth += 1;
            cursor += 4;
            continue;
        }
        if rest.starts_with("</div") {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return Some(&html[content_start..cursor]);
            }
            cursor += 6;
            continue;
        }
        cursor += rest.chars().next()?.len_utf8();
    }
    None
}

/// Transforme le HTML d'itch en texte lisible. Les images sont conservées sous
/// la syntaxe Markdown que la fiche sait déjà peindre dans « À propos ».
fn html_to_markdown(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut cursor = 0usize;
    while cursor < html.len() {
        let rest = &html[cursor..];
        if rest.starts_with('<') {
            let Some(end) = rest.find('>') else {
                break;
            };
            let tag = rest[1..end].trim();
            let lower = tag.to_ascii_lowercase();
            if lower.starts_with("img") {
                if let Some(src) = attribute(tag, "src").and_then(|url| absolute_image(&url)) {
                    let alt = attribute(tag, "alt").unwrap_or_default();
                    out.push_str("\n![");
                    out.push_str(&alt.replace(['[', ']'], ""));
                    out.push_str("](");
                    out.push_str(&src);
                    out.push_str(")\n");
                }
            } else if lower.starts_with("br")
                || lower.starts_with("/p")
                || lower.starts_with("/div")
                || lower.starts_with("/section")
                || lower.starts_with("/h")
                || lower.starts_with("h1")
                || lower.starts_with("h2")
                || lower.starts_with("h3")
                || lower.starts_with("h4")
            {
                out.push('\n');
            } else if lower.starts_with("li") {
                out.push_str("\n- ");
            }
            cursor += end + 1;
            continue;
        }
        if rest.starts_with('&') {
            if let Some((decoded, used)) = decode_entity(rest) {
                out.push_str(&decoded);
                cursor += used;
                continue;
            }
        }
        let Some(ch) = rest.chars().next() else {
            break;
        };
        out.push(ch);
        cursor += ch.len_utf8();
    }
    compact_page_text(&out)
}

/// Galerie de la page : les liens de la visionneuse d'images, qui pointent
/// vers les fichiers « original » plutôt que leurs vignettes.
fn page_screenshots(html: &str) -> Vec<String> {
    let mut screenshots: Vec<String> = Vec::new();
    let Some(start) = html.find("class=\"screenshot_list\"") else {
        return screenshots;
    };
    let list = &html[start..];
    let end = list.find("</div>").unwrap_or(list.len());
    for anchor in list[..end].split("<a ").skip(1) {
        let Some(url) = attribute(anchor, "href").and_then(|url| absolute_image(&url)) else {
            continue;
        };
        if !screenshots.iter().any(|seen| seen == &url) {
            screenshots.push(url);
        }
        if screenshots.len() == 6 {
            break;
        }
    }
    screenshots
}

fn page_youtube_trailer(html: &str) -> Option<String> {
    // La page contient aussi des vidéos dans les commentaires de la
    // communauté. Seul le lecteur de la fiche, marqué `video_embed_widget`,
    // porte la bande-annonce du jeu.
    let widget = html.find("video_embed_widget")?;
    let start = widget.saturating_sub(1200);
    let end = (widget + 12_000).min(html.len());
    let section = &html[start..end];
    for marker in [
        "youtube.com/embed/",
        "youtube-nocookie.com/embed/",
        "youtu.be/",
    ] {
        let Some(at) = section.find(marker) else {
            continue;
        };
        let rest = &section[at + marker.len()..];
        let id: String = rest
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
            .collect();
        if !id.is_empty() {
            return Some(format!("https://www.youtube.com/watch?v={id}"));
        }
    }
    None
}

fn attribute(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let at = lower.find(&format!("{name}="))?;
    let rest = &tag[at + name.len() + 1..];
    let quote = rest.chars().next()?;
    let (rest, closing) = if matches!(quote, '"' | '\'') {
        (&rest[1..], Some(quote))
    } else {
        (rest, None)
    };
    let end = match closing {
        Some(closing) => rest.find(closing)?,
        None => rest.find(|ch: char| ch.is_whitespace() || ch == '>')?,
    };
    Some(rest[..end].to_string())
}

fn absolute_image(url: &str) -> Option<String> {
    let url = url.trim();
    if url.starts_with("https://") {
        Some(url.to_string())
    } else if url.starts_with("//") {
        Some(format!("https:{url}"))
    } else {
        None
    }
}

fn decode_entity(text: &str) -> Option<(String, usize)> {
    let end = text.find(';')?;
    if end > 12 {
        return None;
    }
    let entity = &text[1..end];
    let decoded = match entity {
        "nbsp" => " ".to_string(),
        "amp" => "&".to_string(),
        "quot" => "\"".to_string(),
        "apos" | "#39" => "'".to_string(),
        "lt" => "<".to_string(),
        "gt" => ">".to_string(),
        "eacute" => "é".to_string(),
        "egrave" => "è".to_string(),
        "agrave" => "à".to_string(),
        "ccedil" => "ç".to_string(),
        "rsquo" => "’".to_string(),
        "lsquo" => "‘".to_string(),
        "ldquo" => "“".to_string(),
        "rdquo" => "”".to_string(),
        "mdash" => "—".to_string(),
        "ndash" => "–".to_string(),
        "hellip" => "…".to_string(),
        _ => {
            let number = entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|dec| dec.parse().ok()))?;
            char::from_u32(number)?.to_string()
        }
    };
    Some((decoded, end + 1))
}

fn compact_page_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() {
            if !out.is_empty() && !out.ends_with("\n") {
                out.push('\n');
            }
            continue;
        }
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&line);
    }
    out.trim().to_string()
}

/// Bibliothèque : les clés de téléchargement du compte, une par jeu acheté.
/// Fiche du jeu chez itch : le texte de présentation, ses images et son
/// auteur. L'API répond à la clé du compte, comme pour la bibliothèque.
/// Les notes de version d'un jeu itch vivent dans le journal du projet. Le
/// flux RSS du journal est public, y compris pour un jeu acheté avec une clé :
/// c'est la seule source d'annonces propre à itch, son API ne les expose pas.
fn devlog_news(site: &str) -> Vec<StoreNews> {
    let site = site
        .trim()
        .split(['?', '#'])
        .next()
        .unwrap_or_default()
        .trim_end_matches('/');
    if site.is_empty() {
        return Vec::new();
    }
    let url = format!("{site}/devlog.rss");
    let Ok(xml) = common::get_as(&url, None, PAGE_AGENT) else {
        return Vec::new();
    };
    let mut items = parse_devlog_feed(&xml);
    // Le flux ne porte qu'un début de texte, tronqué par itch. La page de la
    // note contient le corps entier ; on la lit pour que le lecteur affiche
    // toute la note de version, pas seulement ses premières lignes.
    for item in &mut items {
        if let Some((body, image)) = devlog_post(&item.url) {
            item.body = body;
            item.excerpt = lapinux_core::util::ellipsize(&item.body.replace('\n', " "), 320);
            if item.image.is_empty() {
                item.image = image;
            }
        }
    }
    items
}

/// Corps complet d'une note de version, lu sur sa page publique, et son image
/// de tête éventuelle. Le texte est converti en Markdown simple ; les images
/// sont retirées du corps et servent de visuel d'annonce.
fn devlog_post(url: &str) -> Option<(String, String)> {
    if url.trim().is_empty() {
        return None;
    }
    let html = common::get_as(url, None, PAGE_AGENT).ok()?;
    let section = post_body_html(&html)?;
    let body = strip_markdown_images(&html_to_markdown(section));
    let image = first_image(section);
    (!body.trim().is_empty()).then_some((body, image))
}

/// Le corps de la note est la `<section>` qui porte `post_body` ; les autres
/// occurrences de cette classe sont les commentaires, plus bas dans la page.
fn post_body_html(html: &str) -> Option<&str> {
    let marker = "object_text_widget_widget base_widget user_formatted post_body";
    let at = html.find(marker)?;
    let open_start = html[..at].rfind("<section")?;
    let open_end = open_start + html[open_start..].find('>')?;
    let content_start = open_end + 1;
    let end = html[content_start..].find("</section>")?;
    Some(&html[content_start..content_start + end])
}

/// Retire la syntaxe Markdown des images : le lecteur ne peint pas encore les
/// images en ligne, et afficher leur adresse brute coupait le texte.
fn strip_markdown_images(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("![") {
        out.push_str(&rest[..start]);
        let after_alt = &rest[start + 2..];
        let Some(close_alt) = after_alt.find("](") else {
            out.push_str(&rest[start..]);
            return out;
        };
        let after_url = &after_alt[close_alt + 2..];
        let Some(close_url) = after_url.find(')') else {
            out.push_str(&rest[start..]);
            return out;
        };
        rest = &after_url[close_url + 1..];
    }
    out.push_str(rest);
    out
}

/// Un flux RSS itch : un `<item>` par note de version, avec son titre, sa
/// date et le début du texte. Le corps complet n'est pas dans le flux ; la
/// fiche affiche donc l'accroche publiée, comme pour une annonce Steam courte.
fn parse_devlog_feed(xml: &str) -> Vec<StoreNews> {
    let mut items = Vec::new();
    let mut cursor = 0usize;
    while items.len() < 8 {
        let Some(open) = xml[cursor..].find("<item>") else {
            break;
        };
        let start = cursor + open + "<item>".len();
        let Some(close) = xml[start..].find("</item>") else {
            break;
        };
        let block = &xml[start..start + close];
        cursor = start + close + "</item>".len();
        let Some(title) = xml_text(block, "title") else {
            continue;
        };
        let title = compact_page_text(&html_to_markdown(&title)).replace('\n', " ");
        let raw_body = xml_value(block, "content:encoded")
            .or_else(|| xml_value(block, "description"))
            .unwrap_or_default();
        let body = strip_markdown_images(&html_to_markdown(&raw_body));
        if title.trim().is_empty() && body.trim().is_empty() {
            continue;
        }
        let image = first_image(&raw_body);
        let excerpt = lapinux_core::util::ellipsize(&body.replace('\n', " "), 320);
        items.push(StoreNews {
            title,
            excerpt,
            body,
            date: xml_text(block, "pubDate")
                .and_then(|date| rfc2822_seconds(&date))
                .unwrap_or_default(),
            image,
            url: xml_text(block, "link")
                .map(|link| lapinux_core::util::ellipsize(&link, 2048))
                .unwrap_or_default(),
        });
    }
    items
}

/// Contenu brut d'une balise, CDATA défait. Les flux itch échappent le HTML
/// dans `<description>` ou l'enveloppent dans CDATA selon le champ.
fn xml_value(block: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = block.find(&open)? + open.len();
    let rest = &block[start..];
    let end = rest.find(&close)?;
    let raw = rest[..end].trim();
    let raw = raw
        .strip_prefix("<![CDATA[")
        .and_then(|raw| raw.strip_suffix("]]>"))
        .unwrap_or(raw);
    Some(raw.to_string())
}

fn xml_text(block: &str, tag: &str) -> Option<String> {
    xml_value(block, tag).map(|raw| compact_page_text(&html_to_markdown(&raw)).replace('\n', " "))
}

fn first_image(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let Some(start) = lower.find("<img") else {
        return String::new();
    };
    let Some(end) = html[start..].find('>') else {
        return String::new();
    };
    let tag = &html[start..start + end];
    attribute(tag, "src")
        .and_then(|url| absolute_image(&url))
        .unwrap_or_default()
}

/// Date RFC 2822 d'un flux RSS (`Mon, 02 Nov 2026 22:16:08 GMT`) en secondes
/// Unix. Le jour, le mois abrégé et le fuseau sont lus sans dépendre du fuseau
/// de la machine.
fn rfc2822_seconds(text: &str) -> Option<i64> {
    let text = text.trim();
    let rest = text.split_once(',').map(|(_, rest)| rest).unwrap_or(text);
    let mut parts = rest.split_whitespace();
    let day: i64 = parts.next()?.parse().ok()?;
    let month = match parts.next()? {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    };
    let year: i64 = parts.next()?.parse().ok()?;
    let mut clock = parts.next()?.split(':');
    let hour: i64 = clock.next()?.parse().ok()?;
    let minute: i64 = clock.next()?.parse().ok()?;
    let second: i64 = clock.next().unwrap_or_default().parse().unwrap_or_default();
    let zone = parts.next().map(zone_offset).unwrap_or_default();
    let noon = common::civil_seconds(year, month, day)?;
    Some(noon + hour * 3600 + minute * 60 + second - 12 * 3600 - zone)
}

/// Décalage d'un fuseau RFC 2822, en secondes à l'est de Greenwich.
fn zone_offset(zone: &str) -> i64 {
    let bytes = zone.as_bytes();
    if bytes.len() != 5 || !matches!(bytes[0], b'+' | b'-') {
        return 0;
    }
    let hours: i64 = zone[1..3].parse().unwrap_or_default();
    let minutes: i64 = zone[3..5].parse().unwrap_or_default();
    let seconds = hours * 3600 + minutes * 60;
    if bytes[0] == b'-' {
        -seconds
    } else {
        seconds
    }
}

pub fn details(tokens: &StoreTokens, game_id: &str, language: &str) -> Result<StoreDetails> {
    let url = format!("{LEGACY}/{}/game/{game_id}", tokens.access_token);
    let value = common::get_json(&url, None)?;
    let game = value.get("game").cloned().unwrap_or_default();
    let text = |key: &str| {
        game.get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    // L'API itch ne rend que le titre, l'accroche et la couverture. La
    // présentation longue, les captures et la vidéo vivent sur la page
    // publique ; les annonces, dans le flux du journal du projet. La fiche
    // Steam du même titre ne sert qu'en repli quand itch n'en publie aucune.
    let title = text("title");
    let mut summary = text("short_text");
    let mut description = common::plain_text(&text("description"));
    let mut screenshots: Vec<String> = Vec::new();
    if let Some(images) = game.get("screenshots").and_then(Value::as_array) {
        for image in images {
            // L'API rend tantôt l'adresse seule, tantôt l'objet qui la porte.
            let url = image
                .as_str()
                .or_else(|| image.get("url").and_then(Value::as_str))
                .unwrap_or_default();
            if !url.is_empty() {
                screenshots.push(url.to_string());
            }
        }
    }
    screenshots.truncate(6);
    let mut trailer = game
        .get("trailer_url")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let mut trailer_poster = game
        .get("trailer_poster")
        .or_else(|| game.get("trailer_thumbnail"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let site = text("url");
    // L'API legacy ne rend que l'accroche. La page publique porte la vraie
    // présentation, ses captures et parfois une vidéo d'accueil.
    let page = page_details(&site);
    if let Some(page) = page.as_ref() {
        if !page.description.trim().is_empty() {
            description = page.description.clone();
        }
        if !page.screenshots.is_empty() {
            screenshots = page.screenshots.clone();
        }
    }
    let mut news = devlog_news(&site);
    let mut news_from_steam = false;
    if news.is_empty() && !title.trim().is_empty() {
        if let Some(steam) = crate::epic::steam_sheet(&title, language) {
            if !steam.summary.trim().is_empty() {
                summary = steam.summary.clone();
            }
            if description.trim().is_empty() {
                description = steam.description.clone();
            }
            if screenshots.is_empty() {
                screenshots = steam.screenshots.clone();
            }
            if trailer.is_empty() {
                trailer = steam.trailer.clone();
                trailer_poster = steam.trailer_poster.clone();
            }
            news = steam.news;
            news_from_steam = !news.is_empty();
        }
    }
    if description.trim().is_empty() {
        description = summary.clone();
    }
    if summary.trim().is_empty() {
        summary = description
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .to_string();
    }
    if trailer.trim().is_empty() {
        if let Some(page) = page.as_ref().filter(|page| !page.trailer.trim().is_empty()) {
            trailer = page.trailer.clone();
            trailer_poster = page.trailer_poster.clone();
        }
    }
    let author = game
        .get("user")
        .and_then(|user| user.get("username"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    // itch publie une couverture large, à défaut la première capture : la
    // fiche a besoin d'une bannière, comme celle de Steam.
    let banner = game
        .get("cover_url")
        .and_then(Value::as_str)
        .filter(|url| !url.trim().is_empty())
        .map(str::to_string)
        .or_else(|| screenshots.first().cloned())
        .unwrap_or_default();
    let (icon, capsule, cover) = cover_formats(&game);
    Ok(StoreDetails {
        store: Store::Itch,
        icon,
        capsule,
        cover,
        summary,
        description,
        trailer,
        trailer_poster,
        screenshots,
        banner,
        developer: author,
        publisher: String::new(),
        released: text("published_at").chars().take(10).collect(),
        genres: game
            .get("genre")
            .and_then(Value::as_str)
            .filter(|genre| !genre.trim().is_empty())
            .map(|genre| vec![genre.to_string()])
            .unwrap_or_default(),
        site,
        // Le journal du projet est propre à itch ; Steam ne sert que de
        // repli quand ce jeu n'a rien publié sur itch.
        news,
        news_from_steam,
        // L'API d'itch n'expose aucune sauvegarde cloud : c'est une réponse,
        // pas une information manquante.
        cloud: Some(false),
        fetched: lapinux_core::util::now_unix(),
    })
}

pub fn library(tokens: &StoreTokens) -> Result<Vec<StoreGame>> {
    let keys = owned_keys(tokens)?;
    let mut entries = Vec::new();
    let mut names: HashMap<u64, String> = HashMap::new();
    for (game_id, title, cover, classification, author, created) in &keys {
        let title = match title {
            Some(title) => title.clone(),
            None => names
                .entry(*game_id)
                .or_insert_with(|| game_title(tokens, *game_id).unwrap_or_default())
                .clone(),
        };
        if title.is_empty() {
            continue;
        }
        entries.push(StoreGame {
            acquired: *created,
            minutes: None,
            store: Store::Itch,
            store_id: game_id.to_string(),
            appid: store_appid(Store::Itch, &game_id.to_string()),
            name: title,
            // La clé du jeu porte déjà sa couverture : sans elle, la tuile
            // restait une lettre au milieu des jaquettes des autres boutiques.
            cover: cover.clone(),
            platforms: Vec::new(),
            mobile: false,
            mobile_platforms: Vec::new(),
            namespace: String::new(),
            catalog_item: String::new(),
            size: None,
            installed: false,
            // Un asset, une bande-son ou un outil ne se joue pas : il suit le
            // même réglage « Outils et assets » que le marketplace Epic.
            tool: !matches!(classification.as_str(), "game" | "game_mod"),
            // itch ne sépare pas éditeur et développeur : l'auteur du jeu est
            // les deux, et c'est lui que la boutique nomme.
            publishers: if author.is_empty() {
                Vec::new()
            } else {
                vec![author.clone()]
            },
            developers: if author.is_empty() {
                Vec::new()
            } else {
                vec![author.clone()]
            },
            dlc: Vec::new(),
        });
    }
    entries.sort_by(|left, right| left.name.to_lowercase().cmp(&right.name.to_lowercase()));
    entries.dedup_by_key(|entry| entry.appid);
    Ok(entries)
}

/// Clés de téléchargement du compte : `(game_id, titre, couverture et
/// classification s'ils sont donnés)`.
fn owned_keys(
    tokens: &StoreTokens,
) -> Result<Vec<(u64, Option<String>, String, String, String, Option<i64>)>> {
    let key = &tokens.access_token;
    let mut keys = Vec::new();
    for page in 1..=20 {
        let url = format!("{API}/profile/owned-keys?api_key={key}&page={page}");
        let value = common::get_json(&url, None)?;
        let Some(items) = value.get("owned_keys").and_then(Value::as_array) else {
            // Une page sans clé termine la liste : la dernière est vide.
            break;
        };
        if items.is_empty() {
            break;
        }
        for item in items {
            let game_id = item
                .get("game_id")
                .or_else(|| item.pointer("/game/id"))
                .and_then(Value::as_i64);
            let Some(game_id) = game_id else {
                continue;
            };
            let title = item
                .pointer("/game/title")
                .and_then(Value::as_str)
                .map(str::to_string);
            // La couverture est servie en 315 × 250 et le CDN ne connaît que
            // cette taille : demander plus grand rend un 404.
            let cover = item
                .pointer("/game/cover_url")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let classification = item
                .pointer("/game/classification")
                .and_then(Value::as_str)
                .unwrap_or("game")
                .to_string();
            let author = item
                .pointer("/game/user/username")
                .or_else(|| item.pointer("/game/user/display_name"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let created = item
                .get("created_at")
                .and_then(Value::as_str)
                .and_then(lapinux_core::stores::parse_stamp);
            keys.push((
                game_id as u64,
                title,
                cover,
                classification,
                author,
                created,
            ));
        }
        if items.len() < 2 {
            break;
        }
    }
    Ok(keys)
}

/// Titre d'un jeu, quand la fiche du jeu l'accepte.
fn game_title(tokens: &StoreTokens, game_id: u64) -> Option<String> {
    let url = format!("{API}/games/{game_id}?api_key={}", tokens.access_token);
    let value = common::get_json(&url, None).ok()?;
    value
        .pointer("/game/title")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Plan d'installation : le fichier publié pour la plateforme demandée.
pub fn install_plan(tokens: &StoreTokens, store_id: &str, platform: &str) -> Result<InstallPlan> {
    let game_id: u64 = store_id
        .parse()
        .map_err(|_| Error::Parse(format!("identifiant itch invalide : {store_id}")))?;
    let key_id = download_key_id(tokens, game_id)?;
    let url = match key_id {
        Some(key_id) => format!(
            "{API}/games/{game_id}/uploads?api_key={}&download_key_id={key_id}",
            tokens.access_token
        ),
        None => format!(
            "{API}/games/{game_id}/uploads?api_key={}",
            tokens.access_token
        ),
    };
    let value = common::get_json(&url, None)?;
    let uploads = value
        .get("uploads")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::Parse(t("downloads.store_no_file").into()))?;
    let name = value
        .pointer("/game/title")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| game_title(tokens, game_id))
        .unwrap_or_else(|| format!("Jeu itch {game_id}"));
    let upload = choose_upload(uploads, platform)
        .ok_or_else(|| Error::Unavailable("aucun fichier pour cette plateforme"))?;
    let upload_id = upload
        .get("id")
        .and_then(Value::as_i64)
        .ok_or_else(|| Error::Parse("envoi sans identifiant".into()))?;
    let size = upload.get("size").and_then(Value::as_u64).unwrap_or(0);
    let published = upload
        .get("filename")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let download = match key_id {
        Some(key_id) => format!(
            "{API}/uploads/{upload_id}/download?api_key={}&download_key_id={key_id}",
            tokens.access_token
        ),
        None => format!(
            "{API}/uploads/{upload_id}/download?api_key={}",
            tokens.access_token
        ),
    };
    let installdir = common::install_dir(&name);
    // Le nom publié décide de l'extraction : un jeu itch.io peut être un rar,
    // un 7z ou un tar, pas seulement un zip. On garde aussi son extension
    // pour que le cache ne fasse pas passer un rar pour un zip.
    let file = upload_cache_name(&installdir, upload_id, published);
    let step = if has_archive_suffix(published) {
        Step::Extract { file: file.clone() }
    } else {
        Step::InstallFile { file: file.clone() }
    };
    Ok(InstallPlan {
        store: Store::Itch,
        store_id: store_id.to_string(),
        name,
        installdir,
        size,
        installed: 0,
        platform: platform.to_string(),
        build: String::new(),
        steps: vec![
            Step::Download {
                url: download,
                file: file.clone(),
                size,
            },
            step,
        ],
    })
}

/// Nom du fichier dans le cache : le nom publié, débarrassé de ses dossiers
/// et préfixé par le jeu et l'envoi pour éviter qu'un autre jeu du même nom
/// l'écrase. L'extension reste la dernière partie du nom.
fn upload_cache_name(installdir: &str, upload_id: i64, published: &str) -> String {
    let base = published
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .trim()
        .trim_matches('.');
    let base: String = base
        .chars()
        .map(|ch| if ch.is_control() { '_' } else { ch })
        .collect();
    if base.is_empty() {
        format!("{installdir}-{upload_id}.zip")
    } else {
        format!("{installdir}-{upload_id}-{base}")
    }
}

/// Les formats que `common::extract_archive` sait ouvrir. Un fichier isolé
/// (`.sh`, `.AppImage`, `.exe`, `.html`) est copié tel quel.
fn has_archive_suffix(filename: &str) -> bool {
    let filename = filename.to_ascii_lowercase();
    [
        ".zip", ".rar", ".7z", ".tar", ".tar.gz", ".tgz", ".tar.xz", ".txz", ".tar.bz2", ".tbz2",
        ".tar.zst",
    ]
    .iter()
    .any(|suffix| filename.ends_with(suffix))
}

/// Clé de téléchargement du jeu, s'il en faut une : les jeux gratuits n'en
/// demandent pas, les jeux achetés oui.
fn download_key_id(tokens: &StoreTokens, game_id: u64) -> Result<Option<u64>> {
    let key = &tokens.access_token;
    for page in 1..=20 {
        let url = format!("{API}/profile/owned-keys?api_key={key}&page={page}");
        let value = common::get_json(&url, None)?;
        let Some(items) = value.get("owned_keys").and_then(Value::as_array) else {
            break;
        };
        if items.is_empty() {
            break;
        }
        for item in items {
            let id = item.get("id").and_then(Value::as_i64);
            let id_game = item
                .get("game_id")
                .or_else(|| item.pointer("/game/id"))
                .and_then(Value::as_i64);
            if id_game == Some(game_id as i64) {
                return Ok(id.map(|id| id as u64));
            }
        }
    }
    Ok(None)
}

/// Le fichier qui correspond à la plateforme : son nom ou son envoi le dit.
fn choose_upload<'a>(uploads: &'a [Value], platform: &str) -> Option<&'a Value> {
    let wanted: &[&str] = match platform {
        "linux" => &["linux", ".sh", "x86_64", "amd64", "appimage"],
        "windows" => &["windows", ".exe", "win"],
        "macos" => &["mac", "osx", ".dmg", ".app"],
        _ => &[],
    };
    let candidate = |upload: &&Value| {
        let filename = upload
            .get("filename")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_lowercase();
        let platform_field = upload
            .get("platform")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_lowercase();
        wanted
            .iter()
            .any(|needle| filename.contains(needle) || platform_field.contains(needle))
    };
    uploads
        .iter()
        .find(candidate)
        .or_else(|| {
            uploads.iter().find(|upload| {
                let filename = upload
                    .get("filename")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_lowercase();
                filename.ends_with(".zip")
                    || filename.ends_with(".rar")
                    || filename.ends_with(".7z")
                    || filename.ends_with(".tar")
                    || filename.ends_with(".tar.gz")
                    || filename.ends_with(".tgz")
                    || filename.ends_with(".sh")
                    || filename.ends_with(".appimage")
            })
        })
        .or_else(|| uploads.first())
}

/// Télécharge le fichier puis l'extrait dans le dossier du jeu.
pub fn install(
    _tokens: &StoreTokens,
    plan: &InstallPlan,
    cache: &Path,
    dest: &Path,
    progress: &mut dyn FnMut(u64, u64) -> bool,
) -> Result<()> {
    let mut completed = 0u64;
    let mut total = 0u64;
    for step in &plan.steps {
        if let Step::Download { size, .. } = step {
            total += size;
        }
    }
    for step in &plan.steps {
        match step {
            Step::Download { url, file, size } => {
                let path = cache.join(file);
                let done =
                    common::download(url, None, &path, *size, &mut |received, announced| {
                        let announced = announced.max(total);
                        progress(completed + received, announced)
                    })?;
                completed += done;
            }
            Step::Extract { file } => {
                common::extract_archive(&cache.join(file), dest)?;
            }
            Step::InstallFile { file } => {
                common::install_file(&cache.join(file), dest)?;
            }
            Step::RunInstaller { file } => {
                common::run_gog_installer(&cache.join(file), dest)?;
            }
            Step::InnoExtract { file } => {
                common::run_innoextract(&cache.join(file), dest)?;
            }
            // Un plan itch n'en porte pas : c'est celui d'Epic.
            Step::EpicBuild { .. } => {}
        }
    }
    common::hoist_single_directory(dest);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn owned_keys_accept_both_shapes() {
        let value: Value = json!({
            "owned_keys": [
                {"id": 11, "game_id": 220, "game": {"title": "Jeu A"}},
                {"id": 12, "game": {"id": 330, "title": "Jeu B"}},
            ]
        });
        let items = value["owned_keys"].as_array().unwrap();
        let game_id = items[1]
            .get("game_id")
            .or_else(|| items[1].pointer("/game/id"))
            .and_then(Value::as_i64);
        assert_eq!(game_id, Some(330));
        assert_eq!(
            items[0].pointer("/game/title").and_then(Value::as_str),
            Some("Jeu A")
        );
    }

    #[test]
    fn the_upload_matches_the_platform() {
        let uploads = vec![
            json!({"id": 1, "filename": "jeu-windows.zip", "size": 10}),
            json!({"id": 2, "filename": "jeu-linux.zip", "size": 20}),
            json!({"id": 3, "filename": "jeu.appimage", "size": 30}),
        ];
        assert_eq!(
            choose_upload(&uploads, "linux").and_then(|u| u.get("id")),
            Some(&json!(2))
        );
        assert_eq!(
            choose_upload(&uploads, "windows").and_then(|u| u.get("id")),
            Some(&json!(1))
        );
        // Sans correspondance, un fichier installable quand même.
        assert!(choose_upload(&uploads, "macos").is_some());
        assert!(choose_upload(&[], "linux").is_none());
    }

    #[test]
    fn a_rar_upload_is_kept_whole() {
        let uploads = vec![json!({
            "id": 7,
            "filename": "LIMINAL STATE.rar",
            "size": 1_057_365_869u64
        })];
        let upload = choose_upload(&uploads, "linux").expect("archive rar");
        assert_eq!(upload.get("id"), Some(&json!(7)));
        assert!(has_archive_suffix("LIMINAL STATE.rar"));
        assert_eq!(
            upload_cache_name("LIMINAL STATE", 7, "dossier/LIMINAL STATE.rar"),
            "LIMINAL STATE-7-LIMINAL STATE.rar"
        );
        assert!(!has_archive_suffix("jeu-linux.sh"));
    }

    #[test]
    fn a_single_itch_cover_also_fills_the_cover_slot() {
        let game = json!({
            "cover_url": "https://example.test/cover",
            "icon_url": ""
        });
        assert_eq!(
            cover_formats(&game),
            (
                String::new(),
                "https://example.test/cover".into(),
                "https://example.test/cover".into()
            )
        );
    }

    #[test]
    fn a_profile_names_its_account() {
        let value = json!({"user": {"id": 42, "username": "JoueurExemple"}});
        assert_eq!(profile_account(&value).as_deref(), Some("JoueurExemple"));
        assert_eq!(profile_user_id(&value).as_deref(), Some("42"));
        let legacy = json!({"username": "old"});
        assert_eq!(profile_account(&legacy).as_deref(), Some("old"));
        assert!(profile_account(&json!({})).is_none());
    }

    #[test]
    fn the_public_page_supplies_the_full_about_and_its_images() {
        let html = r#"
            <div class="formatted_description user_formatted">
              <p>Une vraie présentation.<br>Deuxième ligne.</p>
              <div><img src="https://img.itch.zone/a.png" alt="Plan"></div>
              <p>Et la suite.</p>
            </div>
            <div class="screenshot_list">
              <a href="https://img.itch.zone/a/original/x.png" data-image_lightbox="true">
                <img src="https://img.itch.zone/a/347x500/x.png">
              </a>
              <a href="https://img.itch.zone/b/original/y.png" data-image_lightbox="true">
                <img src="https://img.itch.zone/b/347x500/y.png">
              </a>
            </div>
        "#;
        let inner = div_inner(html, "class=\"formatted_description").expect("présentation");
        let description = html_to_markdown(inner);
        assert!(description.contains("Une vraie présentation.\nDeuxième ligne."));
        assert!(description.contains("![Plan](https://img.itch.zone/a.png)"));
        assert!(description.contains("Et la suite."));
        assert_eq!(
            page_screenshots(html),
            vec![
                "https://img.itch.zone/a/original/x.png".to_string(),
                "https://img.itch.zone/b/original/y.png".to_string()
            ]
        );
    }

    #[test]
    fn a_devlog_feed_becomes_the_projects_news() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8" ?><rss version="2.0"><channel>
            <title>Devlog - itch.io</title>
            <item>
              <title>Patch &amp; notes</title>
              <link>https://exemple.itch.io/jeu/devlog/42/patch-notes</link>
              <pubDate>Mon, 02 Nov 2026 22:16:08 GMT</pubDate>
              <description><![CDATA[<p>Corrige &amp; ajoute des choses.</p><img src="https://img.itch.zone/a.png"/><p>La suite.</p>]]></description>
            </item>
            <item>
              <title>Deuxième note</title>
              <link>https://exemple.itch.io/jeu/devlog/41/deuxieme</link>
              <pubDate>Tue, 03 Nov 2026 10:00:00 +0200</pubDate>
              <description>Un texte simple.</description>
            </item>
        </channel></rss>"#;
        let news = parse_devlog_feed(xml);
        assert_eq!(news.len(), 2);
        assert_eq!(news[0].title, "Patch & notes");
        assert_eq!(
            news[0].url,
            "https://exemple.itch.io/jeu/devlog/42/patch-notes"
        );
        assert!(news[0].body.contains("Corrige & ajoute des choses."));
        assert!(!news[0].body.contains("img.itch.zone"));
        assert_eq!(news[0].image, "https://img.itch.zone/a.png");
        let expected = common::civil_seconds(2026, 11, 2).unwrap() + 10 * 3600 + 16 * 60 + 8;
        assert_eq!(news[0].date, expected);
        // Le fuseau du second article est retiré de l'heure locale.
        let second = common::civil_seconds(2026, 11, 3).unwrap() + 10 * 3600 - 2 * 3600 - 12 * 3600;
        assert_eq!(news[1].date, second);
        assert_eq!(news[1].body, "Un texte simple.");
    }

    #[test]
    fn a_post_page_supplies_its_whole_body_without_the_header_image() {
        let html = r#"
            <section class="object_text_widget_widget base_widget user_formatted post_body" id="object_text_widget_1">
              <p>Première correction complète.</p>
              <p>Deuxième paragraphe, sans coupure.</p>
              <img src="https://img.itch.zone/header.png"/>
            </section>
            <section class="post_body user_formatted">Commentaire à ignorer.</section>
        "#;
        let section = post_body_html(html).expect("corps de la note");
        assert!(!section.contains("Commentaire"));
        let body = strip_markdown_images(&html_to_markdown(section));
        assert!(body.contains("Première correction complète."));
        assert!(body.contains("Deuxième paragraphe, sans coupure."));
        assert!(!body.contains("img.itch.zone"));
        assert_eq!(first_image(section), "https://img.itch.zone/header.png");
    }

    #[test]
    fn a_devlog_feed_without_item_has_no_news() {
        let xml = r#"<rss><channel><title>Journal vide</title></channel></rss>"#;
        assert!(parse_devlog_feed(xml).is_empty());
    }

    #[test]
    fn a_public_page_youtube_embed_becomes_a_watch_link() {
        let html = r#"<div class="video_embed_widget"><iframe src="//www.youtube.com/embed/mQ8EP--q9Ek"></iframe></div>"#;
        assert_eq!(
            page_youtube_trailer(html).as_deref(),
            Some("https://www.youtube.com/watch?v=mQ8EP--q9Ek")
        );
        // Une vidéo de commentaire ne doit pas devenir la bande-annonce.
        let comment = r#"<div class="community_post"><iframe src="//www.youtube.com/embed/YZ7Vz4OH3f4"></iframe></div>"#;
        assert!(page_youtube_trailer(comment).is_none());
    }
}
