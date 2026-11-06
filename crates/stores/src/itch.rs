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
use lapinux_core::stores::{store_appid, Store, StoreDetails, StoreGame, StoreTokens};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

const API: &str = "https://api.itch.io";
/// Ancienne racine, encore la seule à rendre `/me` pour certaines clés.
const LEGACY: &str = "https://itch.io/api/1";

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
    let account = profile_account(&profile)
        .ok_or_else(|| Error::Refused {
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
    for pointer in ["/user/username", "/user/display_name", "/username", "/display_name"] {
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
    profile_account(&profile)
        .ok_or_else(|| Error::Parse("compte itch.io sans nom".into()))
}

/// Bibliothèque : les clés de téléchargement du compte, une par jeu acheté.
/// Fiche du jeu chez itch : le texte de présentation, ses images et son
/// auteur. L'API répond à la clé du compte, comme pour la bibliothèque.
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
    // présentation longue, les captures, la vidéo et les annonces vivent sur
    // la page publique ; on les reprend de la fiche Steam exacte quand elle
    // existe, plutôt que de laisser la fiche vide.
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
    let mut news = Vec::new();
    let mut news_from_steam = false;
    if !title.trim().is_empty() {
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
        summary = description.lines().next().unwrap_or_default().trim().to_string();
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
    Ok(StoreDetails {
        store: Store::Itch,
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
        site: text("url"),
        // Les annonces viennent de Steam quand le même jeu y est publié :
        // l'API itch n'expose aucun journal par jeu.
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
        None => format!("{API}/games/{game_id}/uploads?api_key={}", tokens.access_token),
    };
    let value = common::get_json(&url, None)?;
    let uploads = value
        .get("uploads")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::Parse("aucun fichier publié".into()))?;
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
    let file = format!("{installdir}.zip");
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
            Step::Unzip { file },
        ],
    })
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
        .or_else(|| uploads.iter().find(|upload| {
            let filename = upload
                .get("filename")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_lowercase();
            filename.ends_with(".zip") || filename.ends_with(".sh") || filename.ends_with(".appimage")
        }))
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
                let done = common::download(url, None, &path, *size, &mut |received, announced| {
                    let announced = announced.max(total);
                    progress(completed + received, announced)
                })?;
                completed += done;
            }
            Step::Unzip { file } => {
                common::unzip(&cache.join(file), dest)?;
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
        assert_eq!(items[0].pointer("/game/title").and_then(Value::as_str), Some("Jeu A"));
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
    fn a_profile_names_its_account() {
        let value = json!({"user": {"id": 42, "username": "Nathounette"}});
        assert_eq!(profile_account(&value).as_deref(), Some("Nathounette"));
        assert_eq!(profile_user_id(&value).as_deref(), Some("42"));
        let legacy = json!({"username": "old"});
        assert_eq!(profile_account(&legacy).as_deref(), Some("old"));
        assert!(profile_account(&json!({})).is_none());
    }
}
