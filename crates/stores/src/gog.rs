//! GOG : connexion OAuth, bibliothèque et installateurs hors ligne.
//!
//! GOG vend ses jeux sans DRM et publie, pour chacun, des installeurs hors
//! ligne par plateforme et par langue. C'est ce que ce module télécharge : un
//! `.sh` (mojosetup) sous Linux, un `.exe` (InnoSetup) pour les jeux Windows,
//! extrait par `innoextract` quand il est installé.
//!
//! Les points d'entrée sont ceux du client Galaxy, non documentés comme API
//! publique : ils peuvent changer sans préavis, et les erreurs le disent.

use crate::achievements::Achievement;
use crate::cloud::RemoteFile;
use crate::common::{self, Error, InstallPlan, LoginLink, Result, Step};
use lapinux_core::stores::{store_appid, Store, StoreDetails, StoreGame, StoreTokens};
use serde_json::Value;
use std::path::Path;

/// Identifiants du client Galaxy, publics et partagés par les launchers
/// libres : ils identifient le client, pas l'utilisateur.
const CLIENT_ID: &str = "46899977096215655";
const CLIENT_SECRET: &str = "9d85c43b1482497dbbce61f6e4aa173a433796eeae2ca8c5f6129f2dc4de46d9";
const REDIRECT: &str = "https://embed.gog.com/on_login_success?origin=client";
const AUTH: &str = "https://auth.gog.com";
const EMBED: &str = "https://embed.gog.com";
const API: &str = "https://api.gog.com";
/// Sauvegardes cloud : le service de Galaxy, avec les identifiants du jeu.
const CLOUD: &str = "https://cloudstorage.gog.com";
/// Métadonnées des builds (dont les identifiants cloud du jeu).
const CONTENT: &str = "https://content-system.gog.com";
/// Succès du compte pour un jeu.
const GAMEPLAY: &str = "https://gameplay.gog.com";
/// En-tête que le client Galaxy envoie sur les sauvegardes.
const SAVES_UA: &str = "GOGGalaxyCommunicationService/2.0.13.27 (Windows_32bit) dont_sync_marker/true installation_source/gog";

/// Adresse de connexion à ouvrir dans le navigateur.
pub fn login_link() -> LoginLink {
    LoginLink {
        url: format!(
            "{AUTH}/auth?client_id={CLIENT_ID}&redirect_uri={}&response_type=code&layout=client2",
            percent_encode(REDIRECT)
        ),
        hint: "Après la connexion, GOG renvoie sur une page `on_login_success` : colle ici son adresse complète (ou son `code=`).".into(),
    }
}

/// Termine la connexion avec le code d'autorisation.
pub fn login_with_code(input: &str) -> Result<StoreTokens> {
    let code = common::extract_code(input)
        .ok_or_else(|| Error::Other("aucun code d'autorisation".into()))?;
    let value = token_request(&[
        ("grant_type", "authorization_code"),
        ("code", &code),
        ("redirect_uri", REDIRECT),
    ])?;
    tokens_from(&value)
}

/// Relit un jeton d'accès expiré.
pub fn refresh(tokens: &StoreTokens) -> Result<StoreTokens> {
    if !tokens.has_refresh() {
        return Err(Error::Expired);
    }
    let value = token_request(&[
        ("grant_type", "refresh_token"),
        ("refresh_token", &tokens.refresh_token),
    ])?;
    let mut renewed = tokens_from(&value)?;
    // Le nom du compte ne revient pas avec un jeton : il est déjà connu.
    if renewed.account.is_empty() {
        renewed.account = tokens.account.clone();
    }
    if renewed.user_id.is_empty() {
        renewed.user_id = tokens.user_id.clone();
    }
    Ok(renewed)
}

/// Appel du point d'échange de jetons.
fn token_request(form: &[(&str, &str)]) -> Result<Value> {
    let mut query: Vec<(&str, &str)> = vec![
        ("client_id", CLIENT_ID),
        ("client_secret", CLIENT_SECRET),
    ];
    query.extend_from_slice(form);
    let url = format!(
        "{AUTH}/token?{}",
        query
            .iter()
            .map(|(key, value)| format!("{key}={}", percent_encode(value)))
            .collect::<Vec<_>>()
            .join("&")
    );
    common::get_json(&url, None)
}

fn tokens_from(value: &Value) -> Result<StoreTokens> {
    let access = value
        .get("access_token")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if access.is_empty() {
        return Err(Error::Parse(
            "GOG n'a pas rendu de jeton d'accès".to_string(),
        ));
    }
    let expires_in = value
        .get("expires_in")
        .and_then(Value::as_i64)
        .unwrap_or(3600);
    Ok(StoreTokens {
        store: Store::Gog,
        account: String::new(),
        access_token: access,
        refresh_token: value
            .get("refresh_token")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        expires_at: lapinux_core::util::now_unix() + expires_in,
        user_id: value
            .get("user_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    })
}

/// Nom du compte GOG.
pub fn account_label(tokens: &StoreTokens) -> Result<String> {
    let value = common::get_json(&format!("{EMBED}/userData.json"), Some(&tokens.access_token))?;
    if let Some(name) = value
        .get("username")
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
    {
        return Ok(name.trim().to_string());
    }
    if !tokens.user_id.is_empty() {
        return Ok(format!("compte GOG {}", tokens.user_id));
    }
    Err(Error::Parse("compte GOG sans nom".into()))
}

/// Bibliothèque : les identifiants possédés, puis leurs fiches produit.
pub fn library(tokens: &StoreTokens) -> Result<Vec<StoreGame>> {
    let owned = common::get_json(&format!("{EMBED}/user/data/games"), Some(&tokens.access_token))?;
    let ids: Vec<u64> = owned
        .get("owned")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| match item {
                    Value::Number(number) => number.as_u64(),
                    Value::String(text) => text.parse().ok(),
                    Value::Object(object) => object
                        .get("id")
                        .and_then(Value::as_u64),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    let mut entries = Vec::new();
    for batch in ids.chunks(50) {
        let list = batch
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let Ok(value) = common::get_json(&format!("{API}/products?ids={list}"), None) else {
            // Une fiche muette ne doit pas emporter la bibliothèque.
            continue;
        };
        let Some(products) = value.as_array() else {
            continue;
        };
        for product in products {
            let Some(game) = store_game(product) else {
                continue;
            };
            entries.push(game);
        }
    }
    // La fiche produit ne publie que des visuels larges ; la jaquette verticale
    // est gardée à part par le catalogue Galaxy, une requête par jeu. Sans
    // elle, la tuile rognait un logo de 200 px et n'en montrait que le milieu.
    for entry in entries.iter_mut() {
        let Ok(id) = entry.store_id.parse::<u64>() else {
            continue;
        };
        // Une seule requête par jeu : elle rapportait déjà la jaquette, elle
        // donne aussi l'éditeur et le développeur pour les filtres.
        let (cover, publishers, developers) = galaxy_facts(id);
        if let Some(art) = cover {
            entry.cover = art;
        }
        entry.publishers = publishers;
        entry.developers = developers;
    }
    entries.sort_by(|left, right| left.name.to_lowercase().cmp(&right.name.to_lowercase()));
    entries.dedup_by_key(|entry| entry.appid);
    Ok(entries)
}

/// Jaquette verticale d'un jeu et ses crédits : le catalogue de Galaxy publie
/// les deux dans la même réponse, une requête par jeu. La fiche produit ne
/// connaît que des visuels larges et ne nomme personne.
fn galaxy_facts(id: u64) -> (Option<String>, Vec<String>, Vec<String>) {
    let Ok(value) = common::get_json(&format!("{API}/v2/games/{id}"), None) else {
        return (None, Vec::new(), Vec::new());
    };
    let href = value
        .pointer("/_links/boxArtImage/href")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    // Le catalogue rend tantôt une adresse complète, tantôt une adresse sans
    // schéma : le client HTTP refuse la seconde.
    let cover = match href.strip_prefix("//") {
        Some(rest) => Some(format!("https://{rest}")),
        None if href.is_empty() => None,
        None => Some(href.to_string()),
    };
    let names = |key: &str| -> Vec<String> {
        value
            .pointer(&format!("/_embedded/{key}"))
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(|item| item.get("name").and_then(Value::as_str))
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    };
    (cover, names("publishers"), names("developers"))
}

/// Une fiche produit GOG, telle que la bibliothèque l'affiche.
fn store_game(product: &Value) -> Option<StoreGame> {
    let id = product.get("id").and_then(Value::as_u64)?;
    let name = product.get("title").and_then(Value::as_str)?.trim().to_string();
    if name.is_empty() {
        return None;
    }
    // L'API GOG rend ses adresses sans schéma (`//images-…`) : le client HTTP
    // les refuserait.
    let mut cover = ["logo2x", "logo", "background", "icon"]
        .iter()
        .find_map(|key| product.pointer(&format!("/images/{key}"))?.as_str())
        .unwrap_or_default()
        .to_string();
    if cover.starts_with("//") {
        cover.insert_str(0, "https:");
    }
    // L'API des produits nomme les systèmes `content_system_compatibility`
    // (`windows`, `linux`, `osx`) ; l'ancienne forme `platforms` reste lue
    // pour les fiches qui la portent encore. Sans cela, un jeu GOG n'annonçait
    // aucune plateforme et l'installation partait en Linux sur un jeu Windows.
    let mut platforms = Vec::new();
    for (key, compat, label) in [
        ("windows", "windows", "windows"),
        ("linux", "linux", "linux"),
        ("mac", "osx", "macos"),
    ] {
        let played = product
            .pointer(&format!("/platforms/{key}"))
            .and_then(Value::as_bool)
            .or_else(|| {
                product
                    .pointer(&format!("/content_system_compatibility/{compat}"))
                    .and_then(Value::as_bool)
            })
            .unwrap_or(false);
        if played {
            platforms.push(label.to_string());
        }
    }
    Some(StoreGame {
        acquired: None,
        minutes: None,
        store: Store::Gog,
        store_id: id.to_string(),
        appid: store_appid(Store::Gog, &id.to_string()),
        name,
        cover,
        platforms,
        namespace: String::new(),
        catalog_item: String::new(),
        size: None,
        installed: false,
        tool: false,
        // Remplis plus bas par le catalogue Galaxy, dans la requête qui
        // rapporte déjà la jaquette.
        publishers: Vec::new(),
        developers: Vec::new(),
            dlc: Vec::new(),
    })
}

/// Succès du compte pour un jeu, page après page.
pub fn achievements(tokens: &StoreTokens, store_id: &str) -> Result<Vec<Achievement>> {
    let id: u64 = store_id
        .parse()
        .map_err(|_| Error::Parse(format!("identifiant GOG invalide : {store_id}")))?;
    if tokens.user_id.trim().is_empty() {
        return Err(Error::Expired);
    }
    let mut items = Vec::new();
    let mut page: Option<String> = None;
    for _ in 0..20 {
        let mut url = format!(
            "{GAMEPLAY}/clients/{id}/users/{}/achievements",
            tokens.user_id
        );
        if let Some(page) = &page {
            url.push_str(&format!("?page_token={}", percent_encode(page)));
        }
        let value = common::get_json(&url, Some(&tokens.access_token))?;
        let Some(entries) = value.get("items").and_then(Value::as_array) else {
            break;
        };
        for entry in entries {
            let key = entry
                .get("achievement_id")
                .and_then(Value::as_str)
                .or_else(|| entry.get("achievement_key").and_then(Value::as_str))
                .unwrap_or_default()
                .to_string();
            if key.is_empty() {
                continue;
            }
            let unlocked_at = entry
                .get("date_unlocked")
                .and_then(Value::as_str)
                .filter(|date| !date.trim().is_empty())
                .and_then(crate::cloud::parse_iso8601);
            let unlocked = unlocked_at.is_some();
            let icon = ["image_url_unlocked", "image_url_locked"]
                .iter()
                .filter_map(|key| entry.get(*key).and_then(Value::as_str))
                .find(|url| !url.trim().is_empty())
                .unwrap_or_default()
                .to_string();
            items.push(Achievement {
                key,
                name: entry
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                description: entry
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                icon,
                // `visible: false` est le succès caché de GOG.
                hidden: !entry
                    .get("visible")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
                unlocked,
                unlock_time: unlocked_at,
                percent: entry
                    .pointer("/rarity/percent")
                    .and_then(Value::as_f64)
                    .map(|percent| percent as f32),
            });
        }
        page = value
            .get("page_token")
            .and_then(Value::as_str)
            .filter(|token| !token.trim().is_empty())
            .map(str::to_string);
        if page.is_none() {
            break;
        }
    }
    Ok(items)
}

/// Identifiants cloud du jeu, puis jeton d'accès propre à ces identifiants.
///
/// GOG range les sauvegardes sous un identifiant de client **par jeu**, publié
/// dans les métadonnées de son build : le jeton de connexion Galaxy ne suffit
/// pas, il faut le relire avec ce client-là.
fn cloud_session(
    tokens: &StoreTokens,
    store_id: &str,
    platform: &str,
) -> Result<(String, String, String)> {
    if !tokens.has_refresh() {
        return Err(Error::Expired);
    }
    let os = match platform {
        "windows" => "windows",
        "macos" => "mac",
        _ => "linux",
    };
    let builds = common::get_json(
        &format!("{CONTENT}/products/{store_id}/os/{os}/builds?generation=2"),
        Some(&tokens.access_token),
    )?;
    let link = builds
        .pointer("/items/0/link")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Unavailable("GOG ne publie aucun build pour cette plateforme"))?;
    let raw = crate::cloud::get_bytes(link, Some(&tokens.access_token), &[])?;
    let meta = zlib_json(&raw)?;
    let client_id = meta
        .get("clientId")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Parse("build GOG sans identifiant de client".into()))?
        .to_string();
    let client_secret = meta
        .get("clientSecret")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Parse("build GOG sans secret de client".into()))?
        .to_string();
    let url = format!(
        "{AUTH}/token?client_id={}&client_secret={}&grant_type=refresh_token&refresh_token={}&without_new_session=1",
        percent_encode(&client_id),
        percent_encode(&client_secret),
        percent_encode(&tokens.refresh_token)
    );
    let value = common::get_json(&url, None)?;
    let access = value
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or(Error::Expired)?
        .to_string();
    let user_id = value
        .get("user_id")
        .and_then(Value::as_str)
        .unwrap_or(&tokens.user_id)
        .to_string();
    Ok((client_id, access, user_id))
}

/// Corps zlib d'une réponse GOG (les métadonnées de build sont compressées).
fn zlib_json(raw: &[u8]) -> Result<Value> {
    use flate2::read::ZlibDecoder;
    use std::io::Read;
    let mut text = String::new();
    ZlibDecoder::new(raw)
        .read_to_string(&mut text)
        .map_err(|error| Error::Parse(format!("build GOG illisible : {error}")))?;
    serde_json::from_str(&text).map_err(|error| Error::Parse(error.to_string()))
}

/// Liste les sauvegardes cloud du jeu.
///
/// GOG range les fichiers sous `<dossier du jeu>/<chemin>` ; le dossier est
/// retiré du nom affiché, et remis à l'envoi.
pub fn cloud_list(
    tokens: &StoreTokens,
    store_id: &str,
    platform: &str,
    dir_name: &str,
) -> Result<Vec<RemoteFile>> {
    let (client_id, access, user_id) = cloud_session(tokens, store_id, platform)?;
    let url = format!("{CLOUD}/v1/{user_id}/{client_id}");
    let value = match common::get_json(&url, Some(&access)) {
        Ok(value) => value,
        // Un compte sans sauvegarde répond 404 : ce n'est pas une erreur.
        Err(Error::Refused { status: 404, .. }) => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut files = Vec::new();
    let Some(entries) = value.as_array() else {
        return Ok(files);
    };
    for entry in entries {
        let Some(full_name) = entry.get("name").and_then(Value::as_str) else {
            continue;
        };
        let relative = strip_dir(full_name, dir_name);
        files.push(RemoteFile {
            name: relative,
            size: entry.get("size").and_then(Value::as_u64).unwrap_or(0),
            modified: entry
                .get("last_modified")
                .and_then(Value::as_str)
                .and_then(crate::cloud::parse_iso8601)
                .unwrap_or(0),
            hash: entry
                .get("hash")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            read_url: String::new(),
            write_url: String::new(),
            full_name: full_name.to_string(),
        });
    }
    files.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(files)
}

/// Télécharge un fichier de sauvegarde vers `dest`.
pub fn cloud_download(
    tokens: &StoreTokens,
    store_id: &str,
    platform: &str,
    dir_name: &str,
    file: &RemoteFile,
    dest: &Path,
) -> Result<u64> {
    let (client_id, access, user_id) = cloud_session(tokens, store_id, platform)?;
    let full = if file.full_name.trim().is_empty() {
        join_dir(dir_name, &file.name)
    } else {
        file.full_name.clone()
    };
    let url = format!(
        "{CLOUD}/v1/{user_id}/{client_id}/{}",
        percent_encode_path(&full)
    );
    let bytes = crate::cloud::get_bytes(
        &url,
        Some(&access),
        &[("User-Agent", SAVES_UA), ("Accept", "application/json")],
    )?;
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(dest, &bytes)?;
    Ok(bytes.len() as u64)
}

/// Envoie un fichier local dans le cloud du jeu.
pub fn cloud_upload(
    tokens: &StoreTokens,
    store_id: &str,
    platform: &str,
    dir_name: &str,
    file: &RemoteFile,
    body: Vec<u8>,
    modified: i64,
) -> Result<u64> {
    let (client_id, access, user_id) = cloud_session(tokens, store_id, platform)?;
    let full = join_dir(dir_name, &file.name);
    let url = format!(
        "{CLOUD}/v1/{user_id}/{client_id}/{}",
        percent_encode_path(&full)
    );
    let (compressed, digest) = crate::cloud::gzip_md5(&body)?;
    let stamp = crate::cloud::format_iso8601(modified);
    let len = body.len() as u64;
    crate::cloud::put_bytes(
        &url,
        Some(&access),
        compressed,
        &[
            ("User-Agent", SAVES_UA),
            ("Content-Encoding", "gzip"),
            ("Etag", &digest),
            ("X-Object-Meta-LocalLastModified", &stamp),
        ],
    )?;
    Ok(len)
}

/// Retire le dossier du jeu du nom publié par GOG.
fn strip_dir(full_name: &str, dir_name: &str) -> String {
    if dir_name.trim().is_empty() {
        return full_name.to_string();
    }
    full_name
        .strip_prefix(&format!("{dir_name}/"))
        .unwrap_or(full_name)
        .to_string()
}

/// Remet le dossier du jeu devant un chemin relatif.
fn join_dir(dir_name: &str, name: &str) -> String {
    if dir_name.trim().is_empty() {
        name.to_string()
    } else {
        format!("{dir_name}/{name}")
    }
}

/// Encodage d'un chemin, séparateurs conservés : `a/b c.sav` reste lisible
/// pour le serveur, les espaces deviennent `%20`.
fn percent_encode_path(path: &str) -> String {
    path.split('/')
        .map(percent_encode)
        .collect::<Vec<_>>()
        .join("/")
}

/// Plan d'installation : les installeurs hors ligne de la plateforme.
///
/// La fiche produit est publique : elle se lit sans jeton, un jeton expiré la
/// faisait refuser en 401. Chaque fichier porte son adresse de résolution
/// (`downlink`), qui n'accepte que la session du compte : elle est gardée
/// telle quelle dans le plan et résolue au téléchargement.
pub fn install_plan(tokens: &StoreTokens, store_id: &str, platform: &str) -> Result<InstallPlan> {
    let id: u64 = store_id
        .parse()
        .map_err(|_| Error::Parse(format!("identifiant GOG invalide : {store_id}")))?;
    let url = format!("{API}/products/{id}?expand=downloads&locale=fr-FR");
    let value = common::get_json(&url, None)?;
    let name = value
        .get("title")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("Jeu GOG {id}"));
    let wanted = match platform {
        "windows" => "windows",
        "macos" => "mac",
        _ => "linux",
    };
    // Beaucoup de jeux GOG n'ont pas d'installeur Linux : leur build Windows
    // s'extrait avec `innoextract`, et tourne ensuite par Proton.
    let mut os = wanted;
    let mut installer = pick_installer(&value, os);
    if installer.is_none() && wanted == "linux" {
        os = "windows";
        installer = pick_installer(&value, os);
    }
    let installer =
        installer.ok_or_else(|| Error::Unavailable("aucun installeur pour cette plateforme"))?;
    let files: Vec<(String, u64)> = installer
        .get("files")
        .and_then(Value::as_array)
        .map(|files| {
            files
                .iter()
                .filter_map(|file| {
                    let size = file.get("size").and_then(Value::as_u64).unwrap_or(0);
                    let link = file
                        .get("downlink")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .or_else(|| {
                            let file_id = file.get("id").and_then(Value::as_str)?;
                            Some(format!(
                                "{API}/products/{id}/downlink/installer/{file_id}"
                            ))
                        })?;
                    Some((link, size))
                })
                .collect()
        })
        .unwrap_or_default();
    if files.is_empty() {
        return Err(Error::Unavailable("installeur GOG sans fichier"));
    }
    let installdir = common::install_dir(&name);
    let suffix = if os == "windows" { "exe" } else { "sh" };
    let mut steps = Vec::new();
    let mut size = 0u64;
    for (index, (link, file_size)) in files.iter().enumerate() {
        let file = format!("{installdir}-{index}.{suffix}");
        steps.push(Step::Download {
            url: link.clone(),
            file: file.clone(),
            size: *file_size,
        });
        steps.push(if os == "windows" {
            Step::InnoExtract { file }
        } else {
            Step::RunInstaller { file }
        });
        size += file_size;
    }
    let _ = tokens;
    Ok(InstallPlan {
        store: Store::Gog,
        store_id: store_id.to_string(),
        name,
        installdir,
        size,
        installed: 0,
        platform: os.to_string(),
        build: String::new(),
        steps,
    })
}

/// L'installeur de la plateforme, en français quand GOG le publie.
pub(crate) fn pick_installer<'a>(value: &'a Value, os: &str) -> Option<&'a Value> {
    let installers = value
        .pointer("/downloads/installers")
        .and_then(Value::as_array)?;
    let candidates: Vec<&Value> = installers
        .iter()
        .filter(|installer| {
            installer
                .get("os")
                .and_then(Value::as_str)
                .map(|value| value == os)
                .unwrap_or(false)
        })
        .collect();
    // La langue française d'abord, puis la première proposée : un jeu peut
    // n'exister qu'en anglais.
    candidates
        .iter()
        .find(|installer| {
            installer
                .get("language")
                .and_then(Value::as_str)
                .map(|language| language.starts_with("fr"))
                .unwrap_or(false)
        })
        .copied()
        .or_else(|| candidates.first().copied())
}

/// Télécharge les installeurs puis les exécute dans le dossier du jeu.
///
/// L'adresse d'un installeur se résout avec la session du compte : elle
/// renvoie une adresse CDN signée, et c'est son **dernier segment** qui donne
/// le vrai nom du fichier. Ce nom compte : Inno Setup retrouve ses tranches
/// (`setup…-1.bin`) à côté de son `.exe`, un nom inventé les lui cache.
pub fn install(
    tokens: &StoreTokens,
    plan: &InstallPlan,
    cache: &Path,
    dest: &Path,
    progress: &mut dyn FnMut(u64, u64) -> bool,
) -> Result<()> {
    let total = plan.download_size();
    // Fichier du plan → adresse CDN et nom réel, résolus une fois.
    let mut resolved: std::collections::HashMap<String, (String, std::path::PathBuf)> =
        std::collections::HashMap::new();
    for step in &plan.steps {
        let Step::Download { url, file, .. } = step else {
            continue;
        };
        let link = if url.starts_with(API) {
            let value = common::get_json(url, Some(&tokens.access_token))?;
            let link = value
                .get("downlink")
                .and_then(Value::as_str)
                .ok_or_else(|| Error::Parse("GOG n'a pas donné d'adresse de téléchargement".into()))?;
            link.to_string()
        } else {
            url.clone()
        };
        let name = link
            .rsplit('/')
            .next()
            .map(percent_decode)
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| file.clone());
        resolved.insert(file.clone(), (link, cache.join(name)));
    }

    // Toutes les tranches d'abord : `innoextract` a besoin de l'ensemble
    // (`setup.exe` et ses `-1.bin`) avant de lire quoi que ce soit.
    let mut completed = 0u64;
    for step in &plan.steps {
        let Step::Download { file, size, .. } = step else {
            continue;
        };
        let (link, path) = resolved
            .get(file)
            .ok_or_else(|| Error::Other("installeur GOG non résolu".into()))?;
        let done = common::download(link, None, path, *size, &mut |received, announced| {
            progress(completed + received, announced.max(total))
        })?;
        completed += done;
    }
    // Puis l'extraction, une seule fois : les fichiers suivants ne sont que
    // des tranches du même installeur.
    for step in &plan.steps {
        match step {
            Step::RunInstaller { file } => {
                let path = resolved
                    .get(file)
                    .map(|(_, path)| path.clone())
                    .unwrap_or_else(|| cache.join(file));
                common::run_gog_installer(&path, dest)?;
                break;
            }
            Step::InnoExtract { file } => {
                let path = resolved
                    .get(file)
                    .map(|(_, path)| path.clone())
                    .unwrap_or_else(|| cache.join(file));
                common::run_innoextract(&path, dest)?;
                break;
            }
            Step::Unzip { file } => {
                common::unzip(&cache.join(file), dest)?;
                break;
            }
            _ => {}
        }
    }
    common::hoist_single_directory(dest);
    Ok(())
}

/// Décode un nom de fichier encodé pour une adresse (`%28` → `(`).
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = &text[index + 1..index + 3];
            if let Ok(value) = u8::from_str_radix(hex, 16) {
                out.push(value);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Encodage des paramètres d'adresse (RFC 3986, sans dépendance).
/// Fiche publique du jeu : description, captures, éditeur et date de sortie.
/// L'API des jeux est publique — aucun jeton n'y est nécessaire — et sert les
/// mêmes données que la boutique.
pub fn details(tokens: &StoreTokens, product_id: &str, language: &str) -> Result<StoreDetails> {
    let _ = tokens;
    let value = common::get_json(&format!("https://api.gog.com/v2/games/{product_id}"), None)?;
    // La fiche v1 expose la description en deux parties, les captures et les
    // vidéos ; la v2 donne les crédits, les étiquettes et les fonctions. Une
    // panne de la première ne doit pas emporter la seconde.
    let expanded = common::get_json(
        &format!("{API}/products/{product_id}?expand=description,screenshots,videos,changelog"),
        None,
    )
    .ok();
    let embedded = value
        .get("_embedded")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let text = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let title = expanded
        .as_ref()
        .and_then(|value| value.get("title"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            embedded
                .pointer("/product/title")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default();
    let lead = expanded
        .as_ref()
        .and_then(|value| value.pointer("/description/lead"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let full = expanded
        .as_ref()
        .and_then(|value| value.pointer("/description/full"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    // `lead` est l'accroche de la fiche, `full` la présentation entière. La v2
    // répète souvent la même introduction dans `overview` : elle ne doit pas
    // devenir le résumé.
    let v2_description = text("description");
    let description_source = if !full.trim().is_empty() {
        full.as_str()
    } else if !lead.trim().is_empty() {
        lead.as_str()
    } else {
        v2_description.as_str()
    };
    let mut description = common::plain_text(description_source);
    if description.trim().is_empty() {
        description = common::plain_text(&text("overview"));
    }
    let mut summary = if !lead.trim().is_empty() {
        brief(&lead)
    } else {
        brief(&text("overview"))
    };
    if summary.trim().is_empty() {
        summary = description
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .to_string();
    }
    let mut screenshots: Vec<String> = expanded
        .as_ref()
        .and_then(|value| value.get("screenshots"))
        .and_then(Value::as_array)
        .map(|images| images.iter().filter_map(screenshot_url).collect())
        .unwrap_or_default();
    if screenshots.is_empty() {
        // Repli sur la fiche v2 quand la v1 ne rend pas la galerie.
        if let Some(images) = embedded.get("screenshots").and_then(Value::as_array) {
            for image in images {
                if let Some(template) = image.pointer("/_links/self/href").and_then(Value::as_str) {
                    screenshots.push(template.replace("{formatter}", "1600"));
                }
            }
        }
    }
    screenshots.truncate(6);
    let names = |key: &str| {
        embedded
            .get(key)
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(|item| item.get("name").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    let developers = names("developers");
    let publishers = names("publishers");
    let released = embedded
        .pointer("/product/globalReleaseDate")
        .and_then(Value::as_str)
        .map(|date| date.chars().take(10).collect::<String>())
        .unwrap_or_default();
    let slug = embedded
        .pointer("/product/slug")
        .and_then(Value::as_str)
        .unwrap_or_default();
    // Le visuel large du jeu sert de bannière, à la place de celle de Steam.
    let banner = ["backgroundImage", "galaxyBackgroundImage", "image"]
        .iter()
        .find_map(|key| value.pointer(&format!("/_links/{key}/href"))?.as_str())
        .map(str::to_string)
        .unwrap_or_default();
    let site = if slug.is_empty() {
        format!("https://www.gog.com/game/{product_id}")
    } else {
        format!("https://www.gog.com/game/{slug}")
    };
    // Une vidéo directe serait jouable par le lecteur intégré ; les vidéos
    // YouTube/Wistia de GOG sont des pages embarquées, pas des flux. Dans ce
    // cas, on reprend la bande-annonce Steam du même jeu.
    let mut trailer = String::new();
    let mut trailer_poster = String::new();
    if let Some(videos) = expanded
        .as_ref()
        .and_then(|value| value.get("videos"))
        .and_then(Value::as_array)
    {
        if let Some((url, poster)) = direct_video(videos) {
            trailer = url;
            trailer_poster = poster;
        }
    }
    let mut news = expanded
        .as_ref()
        .and_then(|value| value.get("changelog"))
        .and_then(Value::as_str)
        .map(|changelog| changelog_news(changelog, &site))
        .unwrap_or_default();
    // Un seul repli Steam sert aux deux champs manquants.
    let steam = if (news.is_empty() || trailer.is_empty()) && !title.trim().is_empty() {
        crate::epic::steam_sheet(&title, language)
    } else {
        None
    };
    let mut news_from_steam = false;
    if news.is_empty() {
        if let Some(steam) = &steam {
            news = steam.news.clone();
            news_from_steam = !news.is_empty();
        }
    }
    if trailer.is_empty() {
        if let Some(steam) = &steam {
            trailer = steam.trailer.clone();
            trailer_poster = steam.trailer_poster.clone();
        }
    }
    // GOG publie la liste des fonctions du jeu ; « Cloud saves » y figure
    // seulement pour les jeux qui l'utilisent.
    let cloud = embedded
        .get("features")
        .and_then(Value::as_array)
        .map(|features| {
            features.iter().any(|feature| {
                feature
                    .get("name")
                    .and_then(Value::as_str)
                    .map(|name| name.to_lowercase().contains("cloud save"))
                    .unwrap_or(false)
            })
        });
    Ok(StoreDetails {
        store: Store::Gog,
        summary,
        description,
        trailer,
        trailer_poster,
        screenshots,
        banner,
        developer: developers.first().cloned().unwrap_or_default(),
        publisher: publishers.first().cloned().unwrap_or_default(),
        released,
        // Les étiquettes GOG sont classées : les genres d'abord, puis les
        // thèmes. Quatre suffisent à la bande de la fiche.
        genres: embedded
            .get("tags")
            .and_then(Value::as_array)
            .map(|tags| {
                tags.iter()
                    .filter_map(|tag| tag.get("name").and_then(Value::as_str))
                    .map(str::to_string)
                    .take(4)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
        site,
        news,
        news_from_steam,
        cloud,
        fetched: lapinux_core::util::now_unix(),
    })
}

/// Accroche courte : le début du `lead` GOG, sans balises ni figure, coupé
/// sur un mot. La fiche v2 répète souvent toute la présentation dans
/// `overview` ; elle ne doit pas servir de résumé.
fn brief(html: &str) -> String {
    let plain = common::plain_text(html);
    let compact = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    const LIMIT: usize = 280;
    let Some((limit, _)) = compact.char_indices().nth(LIMIT) else {
        return compact;
    };
    let end = compact[..limit]
        .rfind(char::is_whitespace)
        .map(|index| index + 1)
        .unwrap_or(limit);
    let mut short = compact[..end]
        .trim_end_matches([' ', ',', ';', ':', '-'])
        .to_string();
    if !short.is_empty() {
        short.push('…');
    }
    short
}

/// Adresse d'une capture dans la galerie v1 de GOG.
fn screenshot_url(image: &Value) -> Option<String> {
    if let Some(template) = image.get("formatter_template_url").and_then(Value::as_str) {
        return Some(template.replace("{formatter}", "1600"));
    }
    if let Some(formats) = image.get("formatted_images").and_then(Value::as_array) {
        for format in formats {
            let name = format
                .get("formatter_name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if name == "1600" || name.starts_with("ggvgl") {
                if let Some(url) = format.get("image_url").and_then(Value::as_str) {
                    return Some(url.to_string());
                }
            }
        }
    }
    image
        .pointer("/_links/self/href")
        .and_then(Value::as_str)
        .map(|template| template.replace("{formatter}", "1600"))
}

/// Première vidéo directement lisible par GStreamer ; les embeds YouTube et
/// Wistia ne sont pas des flux et ne doivent pas être donnés au lecteur.
fn direct_video(videos: &[Value]) -> Option<(String, String)> {
    for video in videos {
        let Some(url) = video.get("video_url").and_then(Value::as_str) else {
            continue;
        };
        let lower = url.to_ascii_lowercase();
        if lower.contains(".mp4") || lower.contains(".m3u8") || lower.contains(".webm") {
            let poster = video
                .get("thumbnail_url")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            return Some((url.to_string(), poster));
        }
    }
    None
}

/// Découpe le journal HTML en annonces : chaque titre (`<h2>` à `<h4>`) ouvre
/// une note, le texte qui le suit la remplit. Les titres portent souvent la
/// date en fin de ligne (« Patch 2.5 Oct 22nd 2026 ») : elle quitte le titre
/// pour dater l'annonce.
fn changelog_news(changelog: &str, site: &str) -> Vec<lapinux_core::stores::StoreNews> {
    // Les titres sont repérés d'abord, leur texte ensuite : sans cette passe,
    // un titre inclus dans un paragraphe couperait la note en deux.
    let lower = changelog.to_ascii_lowercase();
    let mut heads: Vec<(usize, usize, usize)> = Vec::new();
    for level in 2..=4 {
        let open = format!("<h{level}");
        let close = format!("</h{level}>");
        let mut from = 0;
        while let Some(offset) = lower[from..].find(&open) {
            let start = from + offset;
            let Some(gt) = lower[start..].find('>') else {
                break;
            };
            let open_end = start + gt + 1;
            let Some(end) = lower[open_end..].find(&close) else {
                break;
            };
            let text_end = open_end + end;
            // Le texte de la note commence après la balise fermante du titre.
            heads.push((start, text_end + close.len(), text_end));
            from = text_end + close.len();
        }
    }
    heads.sort_by_key(|(start, _, _)| *start);
    let mut items = Vec::new();
    for (position, (start, body_start, text_end)) in heads.iter().enumerate() {
        let body_end = heads
            .get(position + 1)
            .map(|(next, _, _)| *next)
            .unwrap_or(changelog.len());
        let heading = common::plain_text(&changelog[*start..*text_end]);
        let body = common::plain_text(&changelog[*body_start..body_end.max(*body_start)]);
        let (title, date) = split_date(&heading);
        if title.trim().is_empty() && body.trim().is_empty() {
            continue;
        }
        items.push(lapinux_core::stores::StoreNews {
                body: String::new(),
            title: if title.trim().is_empty() {
                "Note de mise à jour".to_string()
            } else {
                title
            },
            excerpt: lapinux_core::util::ellipsize(&body.replace('\n', " "), 320),
            date: date.unwrap_or_default(),
            // Le journal ne porte pas d'image : la carte s'en passe.
            image: String::new(),
            url: site.to_string(),
        });
    }
    // Le lecteur montre toutes les notes ; l'encart n'en affiche que trois.
    items.truncate(24);
    items
}

/// Sépare la date de fin d'un titre de note. GOG l'écrit en anglais
/// (« Oct 22nd 2026 », « 30 March 2018 ») ou en ISO ; sans date lisible, le
/// titre reste entier.
fn split_date(heading: &str) -> (String, Option<i64>) {
    let words: Vec<&str> = heading.split_whitespace().collect();
    // Une date ISO se lit d'un seul mot.
    for (index, word) in words.iter().enumerate() {
        let cleaned = word.trim_matches(|ch: char| !ch.is_ascii_digit() && ch != '-');
        if cleaned.len() == 10 && cleaned.matches('-').count() == 2 {
            if let Some(date) = common::iso_seconds(cleaned) {
                let rest: Vec<&str> = words
                    .iter()
                    .enumerate()
                    .filter(|(position, _)| *position != index)
                    .map(|(_, word)| *word)
                    .collect();
                return (rest.join(" ").trim().to_string(), Some(date));
            }
        }
    }
    // Un mois, un jour et une année : GOG écrit tantôt « Oct 22nd 2026 »,
    // tantôt « 30 March 2018 ». Le jour est le nombre voisin du mois.
    let Some(month_pos) = words.iter().position(|word| month_number(word).is_some()) else {
        return (clean_title(heading), None);
    };
    let month = month_number(words[month_pos]).unwrap_or(1);
    let day_pos = [month_pos.checked_sub(1), Some(month_pos + 1)]
        .into_iter()
        .flatten()
        .find(|index| words.get(*index).and_then(|word| day_number(word)).is_some())
        .or_else(|| {
            words
                .iter()
                .enumerate()
                .find(|(index, word)| *index != month_pos && day_number(word).is_some())
                .map(|(index, _)| index)
        });
    let year_pos = words.iter().enumerate().find_map(|(index, word)| {
        (index != month_pos && Some(index) != day_pos && year_number(word).is_some()).then_some(index)
    });
    let (Some(day_pos), Some(year_pos)) = (day_pos, year_pos) else {
        return (clean_title(heading), None);
    };
    let (Some(day), Some(year)) = (day_number(words[day_pos]), year_number(words[year_pos])) else {
        return (clean_title(heading), None);
    };
    let Some(date) = common::civil_seconds(year, month, day) else {
        return (clean_title(heading), None);
    };
    let rest: Vec<&str> = words
        .iter()
        .enumerate()
        .filter(|(position, _)| *position != month_pos && *position != day_pos && *position != year_pos)
        .map(|(_, word)| *word)
        .collect();
    (clean_title(&rest.join(" ")), Some(date))
}

/// Titre allégé de la date : GOG la met entre parenthèses (« Internal Update
/// (21 March 2019) »), qui resteraient vides une fois la date retirée.
fn clean_title(title: &str) -> String {
    let mut cleaned = title.to_string();
    for empty in ["()", "[]", "( )", "[ ]"] {
        while cleaned.contains(empty) {
            cleaned = cleaned.replace(empty, "");
        }
    }
    cleaned
        .trim()
        .trim_end_matches(['-', '–', ':', ',', '.', '(', '[', ')', ']'])
        .trim()
        .to_string()
}

/// Jour d'un jeton (« 22 », « 22nd », « 3rd ») : un nombre, sans décimale ni
/// suffixe alphabétique, dans les bornes d'un jour.
fn day_number(word: &str) -> Option<i64> {
    let word = word.trim_matches(|ch: char| !ch.is_alphanumeric());
    let digits = word.trim_end_matches(|ch: char| ch.is_ascii_alphabetic());
    if digits.is_empty() || !digits.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    let day: i64 = digits.parse().ok()?;
    (1..=31).contains(&day).then_some(day)
}

/// Année d'un jeton : quatre chiffres, dans les bornes d'une date publiée.
fn year_number(word: &str) -> Option<i64> {
    let digits: String = word.chars().filter(char::is_ascii_digit).collect();
    let year: i64 = digits.parse().ok()?;
    (1970..=9999).contains(&year).then_some(year)
}

/// Numéro du mois à partir de son nom anglais, abrégé ou non.
fn month_number(word: &str) -> Option<i64> {
    const MONTHS: [&str; 12] = [
        "january",
        "february",
        "march",
        "april",
        "may",
        "june",
        "july",
        "august",
        "september",
        "october",
        "november",
        "december",
    ];
    let word = word
        .trim_matches(|ch: char| !ch.is_ascii_alphabetic())
        .to_ascii_lowercase();
    if word.len() < 3 {
        return None;
    }
    MONTHS
        .iter()
        .position(|month| month.starts_with(&word) || word.starts_with(month))
        .map(|index| index as i64 + 1)
}

pub(crate) fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_login_link_carries_the_galaxy_client() {
        let link = login_link();
        assert!(link.url.starts_with("https://auth.gog.com/auth?"));
        assert!(link.url.contains(CLIENT_ID));
        // L'adresse de retour est encodée, sinon les `?` se mélangent.
        assert!(link.url.contains("redirect_uri=https%3A%2F%2Fembed.gog.com"));
        assert!(link.hint.contains("on_login_success"));
    }

    #[test]
    fn a_product_becomes_a_library_entry() {
        let product = json!({
            "id": 1207658691,
            "title": "Songs of Silence",
            "images": {"logo2x": "https://images.gog.com/logo.png"},
            "platforms": {"windows": true, "linux": true, "mac": false}
        });
        let game = store_game(&product).expect("fiche");
        assert_eq!(game.name, "Songs of Silence");
        assert_eq!(game.store, Store::Gog);
        assert_eq!(game.store_id, "1207658691");
        assert_eq!(game.appid, store_appid(Store::Gog, "1207658691"));
        assert_eq!(game.platforms, vec!["windows".to_string(), "linux".to_string()]);
        assert!(game.cover.ends_with("logo.png"));
        // Un produit sans titre ne fait pas une entrée.
        assert!(store_game(&json!({"id": 1})).is_none());
    }

    #[test]
    fn the_installer_follows_the_platform_and_the_language() {
        let value = json!({"downloads": {"installers": [
            {"os": "windows", "language": "english", "files": [{"id": 1, "size": 10}]},
            {"os": "linux", "language": "english", "files": [{"id": 2, "size": 20}]},
            {"os": "linux", "language": "français", "files": [{"id": 3, "size": 30}]}
        ]}});
        let chosen = pick_installer(&value, "linux").expect("linux");
        assert_eq!(chosen["language"], json!("français"));
        let window = pick_installer(&value, "windows").expect("windows");
        assert_eq!(window["language"], json!("english"));
        assert!(pick_installer(&value, "mac").is_none());
    }

    #[test]
    fn cloud_names_keep_the_game_folder_out_of_the_way() {
        // GOG range chaque jeu sous un dossier ; la fiche n'affiche que le
        // chemin relatif.
        assert_eq!(strip_dir("Songs of Silence/save1.sav", "Songs of Silence"), "save1.sav");
        assert_eq!(
            strip_dir("autre/save1.sav", "Songs of Silence"),
            "autre/save1.sav"
        );
        // À l'envoi, le dossier est remis devant.
        assert_eq!(
            join_dir("Songs of Silence", "slot1/save.sav"),
            "Songs of Silence/slot1/save.sav"
        );
        assert_eq!(join_dir("", "save.sav"), "save.sav");
        // Les séparateurs restent lisibles, les espaces sont encodés.
        assert_eq!(percent_encode_path("a b/c d.sav"), "a%20b/c%20d.sav");
    }

    #[test]
    fn percent_encoding_keeps_the_url_readable() {
        assert_eq!(percent_encode("a:b/c?d=e"), "a%3Ab%2Fc%3Fd%3De");
        assert_eq!(percent_encode("Songs of Silence"), "Songs%20of%20Silence");
    }

    #[test]
    fn the_changelog_becomes_one_announcement_per_heading() {
        let html = "<h4>Patch 2.5 Oct 22nd 2026</h4><p>Première note.</p>\
                    <h4>Hotfix 2.4</h4><ul><li>Deuxième note.</li></ul>";
        let news = changelog_news(html, "https://www.gog.com/game/x");
        assert_eq!(news.len(), 2);
        assert_eq!(news[0].title, "Patch 2.5");
        assert_eq!(news[0].excerpt, "Première note.");
        assert_eq!(
            news[0].date,
            common::civil_seconds(2026, 10, 22).expect("date")
        );
        // Sans date dans le titre, aucune date n'est inventée.
        assert_eq!(news[1].title, "Hotfix 2.4");
        assert_eq!(news[1].date, 0);
        assert_eq!(news[1].url, "https://www.gog.com/game/x");
    }

    #[test]
    fn a_title_date_reads_in_english_or_iso() {
        let (title, date) = split_date("Update 30 March 2018");
        assert_eq!(title, "Update");
        assert_eq!(date, common::civil_seconds(2018, 3, 30));
        let (title, date) = split_date("Patch 2024-01-09");
        assert_eq!(title, "Patch");
        assert_eq!(date, common::civil_seconds(2024, 1, 9));
        // GOG met parfois la date entre parenthèses, après le titre.
        let (title, date) = split_date("Internal Update (21 March 2019)");
        assert_eq!(title, "Internal Update");
        assert_eq!(date, common::civil_seconds(2019, 3, 21));
        // Un titre sans date reste entier.
        assert_eq!(split_date("Correctifs divers"), ("Correctifs divers".into(), None));
        assert_eq!(
            split_date("Patch January 2023)"),
            ("Patch January 2023".into(), None)
        );
    }

    #[test]
    fn the_lead_becomes_a_short_summary_and_screenshots_keep_their_size() {
        let lead = "<p>Première phrase de présentation.</p><p>Deuxième paragraphe.</p>";
        assert_eq!(brief(lead), "Première phrase de présentation. Deuxième paragraphe.");
        let image = json!({
            "formatter_template_url": "https://cdn.example/{formatter}.png"
        });
        assert_eq!(
            screenshot_url(&image),
            Some("https://cdn.example/1600.png".into())
        );
    }
}

