//! Epic Games Store : connexion OAuth et bibliothèque du compte.
//!
//! Les points d'entrée sont ceux du launcher Epic, utilisés par Legendary et
//! Heroic : ils ne sont pas documentés comme API publique. La connexion passe
//! par le navigateur ; la page de retour affiche un `authorizationCode` que
//! l'utilisateur colle dans Lapinux.
//!
//! L'installation, elle, passe par des manifestes binaires et des chunks CDN :
//! elle n'est pas encore branchée, et [`Store::Epic`] l'annonce comme telle —
//! la bibliothèque s'affiche, le bouton d'installation dit pourquoi il n'est
//! pas là.

use crate::achievements::Achievement;
use crate::cloud::RemoteFile;
use crate::common::{self, Error, InstallPlan, LoginLink, Result};
use lapinux_core::stores::{
    store_appid, ClaimOutcome, FreeGame, Store, StoreDetails, StoreGame, StoreTokens,
};
use serde_json::Value;

/// Client OAuth du launcher Epic, public et partagé par les launchers libres.
const CLIENT_ID: &str = "34a02cf8f4414e29b15921876da36f9a";
const CLIENT_SECRET: &str = "daafbccc737745039dffe53d94fc76cf";
const ACCOUNT: &str = "https://account-public-service-prod03.ol.epicgames.com";
const LIBRARY: &str = "https://library-service.live.use1a.on.epicgames.com";
/// API du launcher : résolution des builds et de leurs manifestes.
const LAUNCHER: &str = "https://launcher-public-service-prod06.ol.epicgames.com";
const CATALOG: &str = "https://catalog-public-service-prod06.ol.epicgames.com";
/// Sauvegardes cloud du launcher : `savesync` liste et signe les fichiers.
const DATASTORAGE: &str = "https://datastorage-public-service-liveegs.live.use1a.on.epicgames.com";
/// GraphQL de la boutique : définitions et progression des succès.
const STORE_GQL: &str = "https://launcher.store.epicgames.com/graphql";
/// GraphQL public de la vitrine : la configuration d'accueil d'un produit s'y
/// lit sans jeton de compte.
const PUBLIC_GQL: &str = "https://store.epicgames.com/graphql";
const STORE_UA: &str = "EpicGamesLauncher/14.0.8-22004686+++Portal+Release-Live";
const REDIRECT: &str = "https://www.epicgames.com/id/api/redirect";
/// Offres gratuites publiées par la boutique : la même liste que la page
/// « Jeux gratuits » du site, sans jeton.
const FREE_GAMES: &str = "https://store-site-backend-static.ak.epicgames.com/freeGamesPromotions";
/// Offres mobiles : le flux PC ne les publie pas. C'est le service de
/// l'application Android/iOS qui les liste, un appel par plateforme, et les
/// offres s'y terminent sur la page de la boutique.
const MOBILE_DISCOVER: &str =
    "https://egs-platform-service.store.epicgames.com/api/v2/public/discover/home";
/// Service des droits du compte : la source de vérité de ce qui est possédé.
const ENTITLEMENT: &str = "https://entitlement-public-service-prod08.ol.epicgames.com";
/// Service des commandes : une offre gratuite se rattache par une commande à
/// zéro (`quickPurchase`), pas par le service `ecommerce` — celui-ci ne
/// connaît plus l'offre et répond 404 « resource not found ».
const ORDERS: &str = "https://orderprocessor-public-service-ecomprod01.ol.epicgames.com";

/// Requête des définitions de succès, celle du launcher Epic (reprise par
/// Legendary) : noms débloqué/verrouillé, descriptions, icônes et rareté.
const ACHIEVEMENTS_QUERY: &str = r#"query Achievement($sandboxId: String!, $locale: String!) {
Achievement {
productAchievementsRecordBySandbox(sandboxId: $sandboxId, locale: $locale) {
totalAchievements
achievements {
achievement {
name
hidden
isBase
unlockedDisplayName
lockedDisplayName
unlockedDescription
lockedDescription
unlockedIconLink
lockedIconLink
rarity {
percent
}
}
}
}
}
}"#;

/// Requête de la progression du compte pour ces mêmes succès.
const PLAYER_QUERY: &str = r#"query PlayerAchievement($epicAccountId: String!, $sandboxId: String!) {
PlayerAchievement {
playerAchievementGameRecordsBySandbox(epicAccountId: $epicAccountId, sandboxId: $sandboxId) {
records {
playerAchievements {
playerAchievement {
achievementName
unlocked
unlockDate
}
}
}
}
}
}"#;

/// Adresse de connexion à ouvrir dans le navigateur.
pub fn login_link() -> LoginLink {
    let redirect = format!("{REDIRECT}?clientId={CLIENT_ID}&responseType=code");
    LoginLink {
        url: format!(
            "https://www.epicgames.com/id/login?redirectUrl={}",
            crate::gog::percent_encode(&redirect)
        ),
        hint: "Après la connexion, la page affiche `{\"authorizationCode\": \"…\"}` : colle ici le \
               code — Epic ne le garde que quelques instants, garde donc cette fenêtre ouverte."
            .into(),
    }
}

/// Termine la connexion avec le code d'autorisation.
pub fn login_with_code(input: &str) -> Result<StoreTokens> {
    let code = common::extract_code(input)
        .ok_or_else(|| Error::Other("aucun code d'autorisation".into()))?;
    let value = common::post_form(
        &format!("{ACCOUNT}/account/api/oauth/token"),
        Some((CLIENT_ID, CLIENT_SECRET)),
        &[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("token_type", "eg1"),
        ],
    )
    .map_err(explain_login_refusal)?;
    tokens_from(&value)
}

/// Traduit un refus de connexion en ce qu'il faut faire.
///
/// Epic répond `authorization_code_not_found` pour un code déjà utilisé ou
/// périmé — sa page de redirection n'en donne un que pour une minute. Sans
/// cette traduction, l'interface affichait le JSON du service, qui ne dit pas
/// à l'utilisateur de recommencer.
fn explain_login_refusal(error: Error) -> Error {
    let Error::Refused { message, .. } = &error else {
        return error;
    };
    if message.contains("authorization_code_not_found")
        || message.contains("authorization code you supplied was not found")
    {
        return Error::Other(
            "Ce code a expiré ou a déjà servi : Epic ne le garde que quelques instants. Rouvre la \
             page de connexion, autorise Lapinux, et colle le code qui s'affiche tout de suite."
                .into(),
        );
    }
    if message.contains("invalid_grant") {
        return Error::Other(
            "Epic a refusé ce code : il ne correspond pas à cette demande de connexion. Rouvre la \
             page et recommence."
                .into(),
        );
    }
    error
}

/// Relit un jeton d'accès expiré.
pub fn refresh(tokens: &StoreTokens) -> Result<StoreTokens> {
    if !tokens.has_refresh() {
        return Err(Error::Expired);
    }
    let value = common::post_form(
        &format!("{ACCOUNT}/account/api/oauth/token"),
        Some((CLIENT_ID, CLIENT_SECRET)),
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", &tokens.refresh_token),
            ("token_type", "eg1"),
        ],
    )?;
    let mut renewed = tokens_from(&value)?;
    if renewed.account.is_empty() {
        renewed.account = tokens.account.clone();
    }
    Ok(renewed)
}

fn tokens_from(value: &Value) -> Result<StoreTokens> {
    let access = value
        .get("access_token")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if access.is_empty() {
        return Err(Error::Parse("Epic n'a pas rendu de jeton d'accès".into()));
    }
    let expires_in = value
        .get("expires_in")
        .and_then(Value::as_i64)
        .unwrap_or(3600);
    Ok(StoreTokens {
        store: Store::Epic,
        account: value
            .get("displayName")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        access_token: access,
        refresh_token: value
            .get("refresh_token")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        expires_at: lapinux_core::util::now_unix() + expires_in,
        user_id: value
            .get("account_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    })
}

/// Nom du compte Epic.
pub fn account_label(tokens: &StoreTokens) -> Result<String> {
    let value = common::get_json(
        &format!("{ACCOUNT}/account/api/oauth/verify"),
        Some(&tokens.access_token),
    )?;
    if let Some(name) = value
        .get("displayName")
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
    {
        return Ok(name.trim().to_string());
    }
    if !tokens.user_id.is_empty() {
        return Ok(format!("compte Epic {}", tokens.user_id));
    }
    Err(Error::Parse("compte Epic sans nom".into()))
}

/// Bibliothèque : les enregistrements de la bibliothèque du compte, paginés.
pub fn library(tokens: &StoreTokens) -> Result<Vec<StoreGame>> {
    let records = library_records(tokens)?;
    let mut entries = Vec::new();
    // Les titres viennent du catalogue, une requête par namespace et par lot.
    // Chaque élément dit aussi s'il s'agit d'un jeu ou d'un outil.
    let mut titles: std::collections::HashMap<
        String,
        (String, String, bool, Vec<String>, Vec<String>, Vec<String>),
    > = std::collections::HashMap::new();
    let mut grouped: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    // Contenus additionnels, rangés sous l'identifiant de leur jeu de base :
    // la boutique les publie comme des fiches à part, qui nomment leur jeu.
    let mut dlcs: std::collections::HashMap<String, Vec<lapinux_core::stores::StoreDlc>> =
        std::collections::HashMap::new();
    for record in &records {
        let Some(namespace) = record.get("namespace").and_then(Value::as_str) else {
            continue;
        };
        if let Some(item) = record.get("catalogItemId").and_then(Value::as_str) {
            grouped
                .entry(namespace.to_string())
                .or_default()
                .push(item.to_string());
        }
    }
    for (namespace, items) in grouped {
        for batch in items.chunks(20) {
            let query = batch
                .iter()
                .map(|item| format!("id={item}"))
                .collect::<Vec<_>>()
                .join("&");
            let url = format!(
                "{CATALOG}/catalog/api/shared/namespace/{namespace}/bulk/items?{query}&includeMainGameDetails=true&country=FR&locale=fr"
            );
            let Ok(value) = common::get_json(&url, Some(&tokens.access_token)) else {
                continue;
            };
            if let Some(map) = value.as_object() {
                for (id, item) in map {
                    let title = item
                        .get("title")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    // Une jaquette portrait, pas la première image venue : la
                    // tuile de la bibliothèque est verticale, et le logo de
                    // l'offre ne dit rien du jeu.
                    let cover = images(
                        &item,
                        &[
                            "DieselGameBoxTall",
                            "OfferImageTall",
                            "DieselStoreFrontTall",
                            "Thumbnail",
                            "DieselGameBox",
                            "OfferImageWide",
                            "DieselStoreFrontWide",
                        ],
                    )
                    .into_iter()
                    .next()
                    .unwrap_or_default();
                    let platforms = platforms_of(&item);
                    // Un contenu additionnel porte son jeu de base dans sa
                    // fiche : il rejoint la sienne au lieu de rester une ligne
                    // cachée de la bibliothèque.
                    if let Some(base) = item
                        .get("mainGameItem")
                        .and_then(|base| base.get("id"))
                        .and_then(Value::as_str)
                        .filter(|base| !base.trim().is_empty())
                    {
                        // Le contenu se télécharge par son nom d'application,
                        // publié dans ses versions : sans lui, ni poids ni
                        // installation séparée.
                        let app_name = item
                            .get("releaseInfo")
                            .and_then(Value::as_array)
                            .and_then(|releases| {
                                releases.iter().find_map(|release| {
                                    release
                                        .get("appId")
                                        .and_then(Value::as_str)
                                        .filter(|app| !app.trim().is_empty())
                                        .map(str::to_string)
                                })
                            })
                            .unwrap_or_default();
                        let list = dlcs.entry(base.to_string()).or_default();
                        if !list.iter().any(|content| content.name == title) {
                            list.push(lapinux_core::stores::StoreDlc {
                                store_id: id.clone(),
                                name: title.clone(),
                                app_name,
                                namespace: namespace.clone(),
                            });
                        }
                    }
                    // La même fiche dit qui publie et qui développe : la
                    // bibliothèque les garde sans requête de plus.
                    titles.insert(
                        id.clone(),
                        (
                            title,
                            cover,
                            is_tool(&item),
                            platforms,
                            catalog_credit(&item, "publisher"),
                            catalog_credit(&item, "developer"),
                        ),
                    );
                }
            }
        }
    }
    for record in &records {
        let Some(app_name) = record.get("appName").and_then(Value::as_str) else {
            continue;
        };
        if app_name.trim().is_empty() {
            continue;
        }
        let item = record
            .get("catalogItemId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let (title, cover, catalog_tool, platforms, publishers, developers) =
            titles.get(&item).cloned().unwrap_or_else(|| {
            // Sans fiche de catalogue, l'enregistrement de la bibliothèque
            // porte lui-même les plateformes (`platform: ["Windows"]`).
            let platforms = record
                .get("platform")
                .and_then(Value::as_array)
                .map(|list| {
                    let mut platforms = Vec::new();
                    for value in list {
                        let Some(label) = value.as_str().and_then(platform_label) else {
                            continue;
                        };
                        if !platforms.contains(&label.to_string()) {
                            platforms.push(label.to_string());
                        }
                    }
                    platforms
                })
                .unwrap_or_default();
            (
                humanize(app_name),
                String::new(),
                false,
                platforms,
                Vec::new(),
                Vec::new(),
            )
            });
        if title.trim().is_empty() {
            continue;
        }
        // Le catalogue ne dit pas toujours ce qu'est une entrée : le sable de
        // l'enregistrement, lui, ne ment pas — les assets du marketplace y
        // vivent sous « UE Marketplace » et ceux de Fab sous leur vitrine.
        let tool = catalog_tool || record_is_tool(record);
        // Les contenus additionnels se lisent avant que l'identifiant de la
        // fiche ne parte dans l'entrée.
        let contents = dlcs.get(&item).cloned().unwrap_or_default();
        entries.push(StoreGame {
            store: Store::Epic,
            store_id: app_name.to_string(),
            appid: store_appid(Store::Epic, app_name),
            name: title,
            cover,
            platforms,
            namespace: record
                .get("namespace")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            catalog_item: item,
            size: None,
            installed: false,
            tool,
            // Le catalogue Epic nomme très souvent un développeur sans
            // éditeur distinct : le développeur tient alors les deux rôles,
            // comme sur itch, au lieu de laisser le jeu sans éditeur.
            publishers: if publishers.is_empty() {
                developers.clone()
            } else {
                publishers
            },
            developers,
            // Le droit porte la date à laquelle Epic l'a accordé : c'est la
            // date d'obtention, gratuite comme payante.
            acquired: acquired_stamp(record),
            minutes: None,
            dlc: contents,
        });
    }
    // Le temps de jeu est un service à part, mais il vit sur la même bibliothèque :
    // un appel rend les minutes du compte, par article (`artifactId` = appName).
    let minutes = playtimes(tokens);
    if !minutes.is_empty() {
        for entry in &mut entries {
            // Epic range ses minutes par article : tantôt l'appName du jeu,
            // tantôt l'identifiant de catalogue. Les deux sont essayés.
            entry.minutes = minutes
                .get(&entry.store_id)
                .or_else(|| minutes.get(&entry.catalog_item))
                .copied();
        }
    }
    entries.sort_by(|left, right| left.name.to_lowercase().cmp(&right.name.to_lowercase()));
    entries.dedup_by_key(|entry| entry.appid);
    // Un même jeu revient plusieurs fois : la bibliothèque Epic liste chaque
    // offre et chaque exemplaire d'un lot, sous des identifiants différents et
    // souvent sous le même titre. Une ligne par titre, celle qui porte une
    // jaquette de préférence.
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut unique: Vec<StoreGame> = Vec::new();
    for entry in entries {
        let key = entry.name.trim().to_lowercase();
        match seen.get(&key) {
            Some(&index) => {
                if unique[index].cover.is_empty() && !entry.cover.is_empty() {
                    unique[index].cover = entry.cover;
                }
                if unique[index].catalog_item.is_empty() && !entry.catalog_item.is_empty() {
                    unique[index].catalog_item = entry.catalog_item;
                }
                if unique[index].platforms.is_empty() && !entry.platforms.is_empty() {
                    unique[index].platforms = entry.platforms;
                }
                // Les doublons se complètent aussi sur les crédits : l'offre
                // qui les nomme les donne au titre retenu.
                if unique[index].publishers.is_empty() && !entry.publishers.is_empty() {
                    unique[index].publishers = entry.publishers;
                }
                if unique[index].developers.is_empty() && !entry.developers.is_empty() {
                    unique[index].developers = entry.developers;
                }
                // Un jeu l'emporte sur un outil de même titre.
                if unique[index].tool && !entry.tool {
                    unique[index].tool = false;
                }
            }
            None => {
                seen.insert(key, unique.len());
                unique.push(entry);
            }
        }
    }
    Ok(unique)
}

/// Plateformes publiées par une fiche de catalogue (`releaseInfo[].platform`).
///
/// Sans elles, un jeu Epic n'annonçait aucune version et l'installation
/// demandait un build Linux à un jeu qui n'existe que sous Windows.
fn platforms_of(item: &Value) -> Vec<String> {
    let mut platforms: Vec<String> = Vec::new();
    for info in item
        .get("releaseInfo")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        for value in info
            .get("platform")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(label) = value.as_str().and_then(platform_label) else {
                continue;
            };
            if !platforms.iter().any(|shown| shown == label) {
                platforms.push(label.to_string());
            }
        }
    }
    platforms
}

/// Nom de plateforme Epic (`Windows`, `Mac`, `Win32`…) dans le vocabulaire de
/// la bibliothèque (`windows`, `macos`, `linux`).
fn platform_label(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "windows" | "win32" | "win64" => Some("windows"),
        "mac" | "macos" | "osx" => Some("macos"),
        "linux" => Some("linux"),
        _ => None,
    }
}

/// Le jeu annonce-t-il des sauvegardes cloud ? Epic le dit parfois par un
/// attribut du catalogue (`CloudSaveEnabled`). Son absence ne prouve pas
/// l'absence du service : la fiche reste alors muette au lieu d'affirmer.
fn cloud_evidence(item: &Value) -> Option<bool> {
    let attrs = item.get("customAttributes")?.as_object()?;
    for (key, value) in attrs {
        let key = key.to_lowercase();
        if key.contains("cloud") && key.contains("save") {
            return Some(match value {
                Value::Bool(flag) => *flag,
                Value::String(text) => !matches!(
                    text.trim().to_lowercase().as_str(),
                    "" | "false" | "0" | "disabled" | "off" | "no"
                ),
                Value::Number(number) => number.as_i64().unwrap_or(0) != 0,
                _ => true,
            });
        }
    }
    None
}

/// Vrai pour un enregistrement déposé par une vitrine d'assets plutôt que par
/// le magasin de jeux : des centaines de greffons, moteurs et projets arrivent
/// ainsi dans la même bibliothèque que les jeux possédés.
fn record_is_tool(record: &Value) -> bool {
    let sandbox = record
        .get("sandboxName")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_lowercase();
    let namespace = record
        .get("namespace")
        .and_then(Value::as_str)
        .unwrap_or_default();
    namespace == "ue"
        || sandbox == "ue marketplace"
        || sandbox.contains("fab")
        || sandbox.contains("marketplace")
}

/// Un jeu, ou un outil ? Le marketplace Unreal dépose ses assets, greffons,
/// moteurs et projets dans la même bibliothèque que les jeux ; leurs
/// catégories le disent. Un titre qui porte « games » reste un jeu, et une
/// entrée sans catégories n'est jamais prise pour un outil.
fn is_tool(item: &Value) -> bool {
    let Some(categories) = item.get("categories").and_then(Value::as_array) else {
        return false;
    };
    let paths: Vec<&str> = categories
        .iter()
        .filter_map(|category| category.get("path").and_then(Value::as_str))
        .collect();
    if paths.iter().any(|path| *path == "games") {
        return false;
    }
    paths.iter().any(|path| {
        matches!(
            path.split('/').next().unwrap_or_default(),
            "assets" | "plugins" | "engines" | "projects" | "asset-format" | "software" | "hidden" | "addons"
        )
    })
}

/// Ce que la boutique publie du jeu : description, captures, éditeur et
/// date de sortie. Lu quand la fiche se déplie, pas à la lecture de la
/// bibliothèque.
///
/// Le catalogue de la bibliothèque ne garde que le strict nécessaire : sa
/// « description » ne fait souvent que répéter le titre et ses images sont des
/// jaquettes. La vitrine du Store, elle, publie la présentation rédigée et la
/// galerie de captures ; elle se lit par son GraphQL, sans jeton de compte.
pub fn details(
    tokens: &StoreTokens,
    namespace: &str,
    catalog_item: &str,
    locale: &str,
) -> Result<StoreDetails> {
    if namespace.trim().is_empty() || catalog_item.trim().is_empty() {
        return Err(Error::Other(
            "jeu Epic sans identifiant de catalogue : sa fiche ne se lit pas".into(),
        ));
    }
    let (country, language) = locale_choice(locale);
    let url = format!(
        "{CATALOG}/catalog/api/shared/namespace/{namespace}/bulk/items?id={catalog_item}&includeMainGameDetails=true&country={country}&locale={language}"
    );
    let value = common::get_json(&url, Some(&tokens.access_token))?;
    let item = value.get(catalog_item).cloned().unwrap_or_default();
    let title = item
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    // Le catalogue Epic répète souvent le titre comme description : une fiche
    // qui ne dit que son nom n'apprend rien, elle reste vide.
    let mut description = common::plain_text(
        item.get("description")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    );
    if lapinux_core::stores::title_key(&description) == lapinux_core::stores::title_key(&title) {
        description.clear();
    }
    // L'accroche du catalogue et celle de la recherche sont courtes ; la
    // présentation longue ne doit pas servir de résumé au début de la fiche.
    let mut summary = description.clone();
    let mut screenshots = images(
        &item,
        &[
            "DieselStoreFrontWide",
            "OfferImageWide",
            "Screenshot",
            "Thumbnail",
        ],
    );
    let mut released = item
        .get("releaseInfo")
        .and_then(Value::as_array)
        .and_then(|info| info.first())
        .and_then(|info| info.get("dateAdded"))
        .and_then(Value::as_str)
        .map(|date| date.chars().take(10).collect::<String>())
        .unwrap_or_default();
    let mut developer = item
        .get("developer")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let mut publisher = item
        .get("publisher")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    // La vitrine complète ce que le catalogue tait : présentation rédigée,
    // galerie de captures, et les noms d'éditeur que le catalogue laisse vides.
    if let Some(showcase) = showcase(tokens, &title, namespace, locale) {
        let text = common::plain_text(
            showcase
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        );
        if !text.trim().is_empty()
            && lapinux_core::stores::title_key(&text) != lapinux_core::stores::title_key(&title)
        {
            summary = text.clone();
            if text.chars().count() > description.chars().count() {
                description = text;
            }
        }
        let gallery = images(&showcase, &["GalleryImage", "Screenshot", "featuredMedia"]);
        let gallery = if gallery.is_empty() {
            images(&showcase, &["OfferImageWide", "DieselStoreFrontWide"])
        } else {
            gallery
        };
        if !gallery.is_empty() {
            screenshots = gallery;
        }
        if let Some(date) = showcase.get("releaseDate").and_then(Value::as_str) {
            let date = date.chars().take(10).collect::<String>();
            if date.len() == 10 {
                released = date;
            }
        }
        if let Some(name) = showcase
            .get("developerDisplayName")
            .and_then(Value::as_str)
            .filter(|name| !name.trim().is_empty())
        {
            developer = name.to_string();
        }
        if let Some(name) = showcase
            .get("publisherDisplayName")
            .and_then(Value::as_str)
            .filter(|name| !name.trim().is_empty())
        {
            publisher = name.to_string();
        }
    }
    // La recherche ne rend qu'une accroche, et l'ancienne page produit manque
    // pour les sorties récentes. La configuration d'accueil du produit, lue
    // par le GraphQL public, porte la présentation entière de la fiche.
    let full = product_home_about(namespace, language).filter(|text| {
        !text.trim().is_empty()
            && lapinux_core::stores::title_key(text) != lapinux_core::stores::title_key(&title)
    });
    if let Some(text) = full {
        description = text;
    } else if let Some(slug) = product_slug(tokens, &title, namespace, locale) {
        // Repli pour les fiches encore servies par l'ancienne page produit.
        if let Some(text) = product_about(&slug, language) {
            if text.chars().count() > description.chars().count() {
                description = text;
            }
        }
    }
    screenshots.truncate(6);
    // Epic ne tient pas d'annonces par jeu. Beaucoup de titres Epic existent
    // aussi sur Steam : on y reprend les annonces et la bande-annonce du même
    // jeu, après une correspondance exacte du titre.
    let steam_app = find_steam_appid(&title, language);
    let mut store_news = news(&title, language);
    let mut news_from_steam = false;
    if store_news.is_empty() {
        if let Some(appid) = steam_app {
            store_news = steam_news(appid, language);
            news_from_steam = !store_news.is_empty();
        }
    }
    let (trailer, trailer_poster) = steam_app
        .and_then(|appid| steam_trailer(appid, language))
        .unwrap_or_default();
    // La bannière : le visuel large du catalogue, sinon la première capture.
    let banner = images(
        &item,
        &[
            "DieselStoreFrontWide",
            "OfferImageWide",
            "DieselGameBoxWide",
            "Thumbnail",
        ],
    )
    .into_iter()
    .next()
    .or_else(|| screenshots.first().cloned())
    .unwrap_or_default();
    Ok(StoreDetails {
        store: Store::Epic,
        summary,
        description,
        screenshots,
        banner,
        developer,
        publisher,
        released,
        genres: Vec::new(),
        site: String::new(),
        news: store_news,
        news_from_steam,
        trailer,
        trailer_poster,
        // Le catalogue ne publie pas toujours l'information : sans attribut,
        // la fiche ne doit pas annoncer un cloud qui n'existe pas.
        cloud: cloud_evidence(&item),
        fetched: lapinux_core::util::now_unix(),
    })
}

/// Identifiant d'URL de la page produit, demandé à la recherche de la
/// boutique. `productSlug` manque à certains jeux ; `urlSlug` les couvre.
fn product_slug(tokens: &StoreTokens, title: &str, namespace: &str, locale: &str) -> Option<String> {
    let (country, language) = locale_choice(locale);
    let body = serde_json::json!({
        "query": SLUG_QUERY,
        "variables": {
            "country": country,
            "locale": language,
            "keywords": title,
            "count": 20,
        },
    });
    let value =
        crate::cloud::post_json(STORE_GQL, Some(&tokens.access_token), &body, Some(STORE_UA)).ok()?;
    let elements = value
        .pointer("/data/Catalog/searchStore/elements")?
        .as_array()?;
    let wanted = lapinux_core::stores::title_key(title);
    let mut homonym = None;
    for element in elements {
        let same_title = lapinux_core::stores::title_key(
            element.get("title").and_then(Value::as_str).unwrap_or_default(),
        ) == wanted;
        if !same_title {
            continue;
        }
        let slug = element
            .get("productSlug")
            .and_then(Value::as_str)
            .filter(|slug| !slug.trim().is_empty())
            .or_else(|| {
                element
                    .get("urlSlug")
                    .and_then(Value::as_str)
                    .filter(|slug| !slug.trim().is_empty())
            });
        let Some(slug) = slug else {
            continue;
        };
        if element.get("namespace").and_then(Value::as_str) == Some(namespace) {
            return Some(slug.to_string());
        }
        if homonym.is_none() {
            homonym = Some(slug.to_string());
        }
    }
    homonym
}

const SLUG_QUERY: &str = "
query slugQuery($country: String!, $locale: String, $keywords: String, $count: Int) {
  Catalog {
    searchStore(country: $country, locale: $locale, keywords: $keywords, count: $count) {
      elements { title namespace productSlug urlSlug }
    }
  }
}";

/// Configuration d'accueil publique d'un produit. La réponse est une liste de
/// configurations (boutique, accueil) ; `HomeConfiguration` porte le texte de
/// la section « À propos », bien plus complet que l'accroche du catalogue.
const PRODUCT_HOME_QUERY: &str = r#"
query getProductHomeConfig($locale: String!, $sandboxId: String!) {
  Product {
    sandbox(sandboxId: $sandboxId) {
      configuration(locale: $locale) {
        ... on HomeConfiguration {
          configs { longDescription }
        }
      }
    }
  }
}
"#;

/// Présentation entière lue dans la configuration d'accueil du produit. Le
/// catalogue et la recherche ne gardent que l'accroche ; cette configuration
/// est aussi la source qui reste disponible quand l'ancienne page
/// `store-content` répond 404 pour une sortie récente.
fn product_home_about(namespace: &str, language: &str) -> Option<String> {
    if namespace.trim().is_empty() {
        return None;
    }
    let body = serde_json::json!({
        "query": PRODUCT_HOME_QUERY,
        "variables": { "locale": language, "sandboxId": namespace },
    });
    let value = crate::cloud::post_json(PUBLIC_GQL, None, &body, Some(STORE_UA)).ok()?;
    product_home_description(&value)
}

/// Texte de la configuration d'accueil, balises retirées. Le serveur rend une
/// liste : la configuration de la boutique n'a pas de description longue, et
/// celle de l'accueil peut en publier plusieurs. La plus longue gagne.
fn product_home_description(value: &Value) -> Option<String> {
    let configuration = value.pointer("/data/Product/sandbox/configuration")?;
    let configurations: Vec<&Value> = match configuration {
        Value::Array(items) => items.iter().collect(),
        Value::Object(_) => vec![configuration],
        _ => return None,
    };
    let mut best = String::new();
    for configuration in configurations {
        let Some(text) = configuration
            .pointer("/configs/longDescription")
            .and_then(Value::as_str)
        else {
            continue;
        };
        let text = common::plain_text(text);
        if text.chars().count() > best.chars().count() {
            best = text;
        }
    }
    (!best.trim().is_empty()).then_some(best)
}

/// Présentation rédigée de la page produit Epic, quand la recherche ne rend
/// que son accroche. La page publique se lit sans jeton de compte.
fn product_about(slug: &str, language: &str) -> Option<String> {
    if slug.trim().is_empty() {
        return None;
    }
    let url = format!(
        "https://store-content.ak.epicgames.com/api/{language}/content/products/{slug}"
    );
    let value = common::get_json(&url, None).ok()?;
    about_description(&value)
}

/// Texte de la page d'accueil d'un produit, balises retirées. Une page peut
/// empiler plusieurs sections ; la plus longue est celle qui présente le jeu.
fn about_description(value: &Value) -> Option<String> {
    let pages = value.get("pages")?.as_array()?;
    let mut best = String::new();
    for page in pages {
        let Some(text) = page
            .pointer("/data/about/description")
            .and_then(Value::as_str)
        else {
            continue;
        };
        let text = common::plain_text(text);
        if text.chars().count() > best.chars().count() {
            best = text;
        }
    }
    (!best.trim().is_empty()).then_some(best)
}

/// Annonces publiées par Epic à propos du jeu. Epic ne tient pas d'annonces
/// par jeu : son blog public est celui de la boutique, toutes sorties
/// confondues. Les billets récents sont lus puis filtrés sur le titre du jeu —
/// une actualité qui ne parle pas du jeu n'a rien à faire dans sa fiche.
fn news(title: &str, language: &str) -> Vec<lapinux_core::stores::StoreNews> {
    let wanted = lapinux_core::stores::title_key(title);
    // Un titre trop court (« Control », « Hades ») filtrerait n'importe quoi.
    if wanted.chars().count() < 6 {
        return Vec::new();
    }
    let url = format!(
        "https://store-content-ipv4.ak.epicgames.com/api/{language}/content/blog-posts?count=40"
    );
    let Ok(value) = common::get_json(&url, None) else {
        return Vec::new();
    };
    let Some(posts) = value.get("elements").and_then(Value::as_array) else {
        return Vec::new();
    };
    // Le titre cherché doit être un bloc de mots entier : « Control » ne doit
    // pas se reconnaître au milieu d'un autre mot.
    let needle = format!(" {wanted} ");
    let short_title = wanted.chars().count() < 10;
    let mut items = Vec::new();
    for post in posts {
        let post_title = post.get("title").and_then(Value::as_str).unwrap_or_default();
        let text = common::plain_text(&format!(
            "{post_title} {}",
            post.get("content").and_then(Value::as_str).unwrap_or_default()
        ));
        // La comparaison se fait sur la même forme que le titre : sans
        // majuscules ni ponctuation, avec un espace autour.
        let body = format!(" {} ", lapinux_core::stores::title_key(&text));
        let in_title = format!(" {} ", lapinux_core::stores::title_key(post_title)).contains(&needle);
        // Une simple mention au détour d'un paragraphe ne fait pas une
        // actualité du jeu : il faut le titre du billet, ou le jeu cité deux
        // fois dans le texte.
        if !in_title && body.matches(&needle).count() < 2 {
            continue;
        }
        // Un titre court (« Control », « Among Us ») se retrouve dans le nom
        // d'autres jeux : il ne compte que si l'annonce le porte en titre.
        if short_title && !in_title {
            continue;
        }
        let image = post
            .get("trendingImage")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                post.get("_images_")
                    .and_then(Value::as_array)
                    .and_then(|images| images.first())
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_default();
        let path = post.get("url").and_then(Value::as_str).unwrap_or_default();
        items.push(lapinux_core::stores::StoreNews {
                body: String::new(),
            title: post
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            excerpt: lapinux_core::util::ellipsize(&text.replace('\n', " "), 320),
            date: post
                .get("date")
                .and_then(Value::as_str)
                .and_then(common::iso_seconds)
                .unwrap_or_default(),
            image,
            url: if path.is_empty() {
                String::new()
            } else {
                format!("https://store.epicgames.com{path}")
            },
        });
        if items.len() == 3 {
            break;
        }
    }
    items
}

/// Retrouve le jeu correspondant chez Steam. La recherche de la boutique est
/// publique et rend souvent des suites ou des DLC : seul le titre exact compte,
/// et le résultat doit être une application.
fn find_steam_appid(title: &str, language: &str) -> Option<u32> {
    if title.trim().is_empty() {
        return None;
    }
    let url = format!(
        "https://store.steampowered.com/api/storesearch/?term={}&cc=fr&l={language}",
        crate::gog::percent_encode(title)
    );
    let value = common::get_json(&url, None).ok()?;
    steam_search_appid(&value, title)
}

fn steam_search_appid(value: &Value, title: &str) -> Option<u32> {
    let wanted = lapinux_core::stores::title_key(title);
    value
        .get("items")
        .and_then(Value::as_array)?
        .iter()
        .find_map(|item| {
            let same_title = item
                .get("name")
                .and_then(Value::as_str)
                .map(|name| lapinux_core::stores::title_key(name) == wanted)
                .unwrap_or(false);
            if !same_title || item.get("type").and_then(Value::as_str) != Some("app") {
                return None;
            }
            item.get("id")
                .and_then(Value::as_u64)
                .and_then(|id| u32::try_from(id).ok())
        })
}

fn steam_language(language: &str) -> &'static str {
    if language.to_ascii_lowercase().starts_with("fr") {
        "french"
    } else {
        "english"
    }
}

/// Annonces Steam du même jeu, quand Epic n'en publie aucune. Steam les expose
/// publiquement pour une application identifiée par son appid.
fn steam_news(appid: u32, language: &str) -> Vec<lapinux_core::stores::StoreNews> {
    let url = format!(
        "https://api.steampowered.com/ISteamNews/GetNewsForApp/v2/?appid={appid}&count=8&maxlength=1200&format=json&l={}",
        steam_language(language)
    );
    let Ok(value) = common::get_json(&url, None) else {
        return Vec::new();
    };
    steam_news_items(&value)
}

fn steam_news_items(value: &Value) -> Vec<lapinux_core::stores::StoreNews> {
    let Some(items) = value
        .pointer("/appnews/newsitems")
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let title = item.get("title").and_then(Value::as_str)?.trim();
            if title.is_empty() {
                return None;
            }
            let body = common::plain_text(
                item.get("contents")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            );
            Some(lapinux_core::stores::StoreNews {
                title: title.to_string(),
                excerpt: lapinux_core::util::ellipsize(&body.replace('\n', " "), 320),
                body,
                date: item.get("date").and_then(Value::as_i64).unwrap_or_default(),
                image: String::new(),
                url: item
                    .get("url")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            })
        })
        .collect()
}

/// Bande-annonce Steam du même jeu : HLS est le format que le lecteur intégré
/// décode déjà pour les fiches Steam.
fn steam_trailer(appid: u32, language: &str) -> Option<(String, String)> {
    let url = format!(
        "https://store.steampowered.com/api/appdetails?appids={appid}&l={}",
        steam_language(language)
    );
    let value = common::get_json(&url, None).ok()?;
    steam_trailer_from_value(&value, appid)
}

fn steam_trailer_from_value(value: &Value, appid: u32) -> Option<(String, String)> {
    let data = value.get(appid.to_string())?.get("data")?;
    let movie = data
        .get("movies")?
        .as_array()?
        .iter()
        .find(|movie| {
            movie.get("hls_h264").is_some()
                || movie.get("dash_h264").is_some()
                || movie.pointer("/mp4/max").is_some()
                || movie.pointer("/webm/max").is_some()
        })?;
    let trailer = movie
        .get("hls_h264")
        .or_else(|| movie.get("dash_h264"))
        .or_else(|| movie.pointer("/mp4/max"))
        .or_else(|| movie.pointer("/webm/max"))
        .and_then(Value::as_str)?.to_string();
    let poster = movie
        .get("thumbnail")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    Some((trailer, poster))
}

/// Fiche Steam du même jeu, pour les boutiques qui ne publient pas certains
/// champs. Le titre doit correspondre exactement : une suite ou un DLC ne doit
/// pas prendre la place du jeu demandé.
#[derive(Clone, Default)]
pub(crate) struct SteamSheet {
    pub(crate) summary: String,
    pub(crate) description: String,
    pub(crate) screenshots: Vec<String>,
    pub(crate) trailer: String,
    pub(crate) trailer_poster: String,
    pub(crate) news: Vec<lapinux_core::stores::StoreNews>,
}

pub(crate) fn steam_sheet(title: &str, language: &str) -> Option<SteamSheet> {
    let appid = find_steam_appid(title, language)?;
    let url = format!(
        "https://store.steampowered.com/api/appdetails?appids={appid}&l={}&cc=fr",
        steam_language(language)
    );
    let value = common::get_json(&url, None).ok()?;
    let mut sheet = steam_sheet_from_value(&value, appid)?;
    sheet.news = steam_news(appid, language);
    Some(sheet)
}

fn steam_sheet_from_value(value: &Value, appid: u32) -> Option<SteamSheet> {
    let data = value.get(appid.to_string())?.get("data")?;
    let field = |key: &str| {
        data.get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let detailed = common::plain_text(&field("detailed_description"));
    let description = if detailed.trim().is_empty() {
        common::plain_text(&field("about_the_game"))
    } else {
        detailed
    };
    let screenshots = data
        .get("screenshots")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("path_full").and_then(Value::as_str))
                .filter(|url| !url.trim().is_empty())
                .take(6)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let (trailer, trailer_poster) =
        steam_trailer_from_value(value, appid).unwrap_or_default();
    Some(SteamSheet {
        summary: common::plain_text(&field("short_description")),
        description,
        screenshots,
        trailer,
        trailer_poster,
        news: Vec::new(),
    })
}

/// Recherche de la vitrine : la présentation et la galerie d'un jeu, à partir
/// de son titre. La réponse porte aussi des offres homonymes ; celle de
/// l'espace de noms du jeu possédé gagne.
const SHOWCASE_QUERY: &str = "
query searchStoreQuery($country: String!, $locale: String, $keywords: String, $count: Int) {
  Catalog {
    searchStore(country: $country, locale: $locale, keywords: $keywords, count: $count) {
      elements {
        title
        namespace
        description
        releaseDate
        developerDisplayName
        publisherDisplayName
        keyImages { type url }
      }
    }
  }
}";

fn showcase(tokens: &StoreTokens, title: &str, namespace: &str, locale: &str) -> Option<Value> {
    if title.trim().is_empty() {
        return None;
    }
    let (country, language) = locale_choice(locale);
    let body = serde_json::json!({
        "query": SHOWCASE_QUERY,
        "variables": {
            "country": country,
            "locale": language,
            "keywords": title,
            "count": 20,
        },
    });
    let value =
        crate::cloud::post_json(STORE_GQL, Some(&tokens.access_token), &body, Some(STORE_UA)).ok()?;
    let elements = value
        .pointer("/data/Catalog/searchStore/elements")?
        .as_array()?;
    let wanted = lapinux_core::stores::title_key(title);
    let mut homonym = None;
    for element in elements {
        let same_title = lapinux_core::stores::title_key(
            element.get("title").and_then(Value::as_str).unwrap_or_default(),
        ) == wanted;
        if !same_title {
            continue;
        }
        // Une offre qui ne publie que son titre n'apprend rien : la fiche
        // garde alors ce que le catalogue a dit.
        let said = lapinux_core::stores::title_key(
            element.get("description").and_then(Value::as_str).unwrap_or_default(),
        );
        if said.is_empty() || said == wanted {
            continue;
        }
        if element.get("namespace").and_then(Value::as_str) == Some(namespace) {
            return Some(element.clone());
        }
        if homonym.is_none() {
            homonym = Some(element.clone());
        }
    }
    homonym
}

/// Adresses des images d'un objet de boutique, dans l'ordre des types demandés
/// et sans doublon.
fn images(value: &Value, wanted: &[&str]) -> Vec<String> {
    let mut urls: Vec<String> = Vec::new();
    let Some(images) = value.get("keyImages").and_then(Value::as_array) else {
        return urls;
    };
    for kind in wanted {
        for image in images {
            if image.get("type").and_then(Value::as_str) != Some(*kind) {
                continue;
            }
            let url = image.get("url").and_then(Value::as_str).unwrap_or_default();
            if url.is_empty() || urls.iter().any(|seen| seen.as_str() == url) {
                continue;
            }
            urls.push(url.to_string());
        }
    }
    urls
}

/// Pays et langue des API de boutique, depuis la langue de Lapinux.
fn locale_choice(locale: &str) -> (&'static str, &'static str) {
    if locale.to_lowercase().starts_with("en") {
        ("US", "en-US")
    } else {
        ("FR", "fr")
    }
}

/// Enregistrements de la bibliothèque, page après page.
fn library_records(tokens: &StoreTokens) -> Result<Vec<Value>> {
    let mut records = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..20 {
        let mut url = format!("{LIBRARY}/library/api/public/items?includeMetadata=true");
        if let Some(cursor) = &cursor {
            url.push_str(&format!("&cursor={cursor}"));
        }
        let value = common::get_json(&url, Some(&tokens.access_token))?;
        if let Some(items) = value.get("records").and_then(Value::as_array) {
            records.extend(items.iter().cloned());
        }
        cursor = value
            .pointer("/responseMetadata/nextCursor")
            .and_then(Value::as_str)
            .filter(|cursor| !cursor.is_empty())
            .map(str::to_string);
        if cursor.is_none() {
            break;
        }
    }
    Ok(records)
}

/// Nom lisible à défaut de catalogue : `Sugar` reste `Sugar`, `abc123def`
/// devient l'identifiant tel quel.
fn humanize(app_name: &str) -> String {
    if app_name.chars().any(|c| c.is_whitespace()) || app_name.chars().any(|c| c.is_lowercase()) {
        app_name.to_string()
    } else {
        app_name.to_string()
    }
}

/// Succès du jeu : les définitions, puis la progression du compte, rapprochées
/// par nom interne. `namespace` est le « sandbox » du jeu, publié par la
/// bibliothèque du compte.
pub fn achievements(
    tokens: &StoreTokens,
    namespace: &str,
    locale: &str,
) -> Result<Vec<Achievement>> {
    if namespace.trim().is_empty() {
        return Err(Error::Other(
            "succès Epic : espace de noms inconnu, relisez la bibliothèque".into(),
        ));
    }
    let definitions = crate::cloud::post_json(
        STORE_GQL,
        Some(&tokens.access_token),
        &serde_json::json!({
            "query": ACHIEVEMENTS_QUERY,
            "variables": { "sandboxId": namespace, "locale": locale },
        }),
        Some(STORE_UA),
    )?;
    let record = definitions
        .pointer("/data/Achievement/productAchievementsRecordBySandbox")
        .cloned()
        .unwrap_or(Value::Null);
    let unlocked = if tokens.user_id.trim().is_empty() {
        Vec::new()
    } else {
        crate::cloud::post_json(
            STORE_GQL,
            Some(&tokens.access_token),
            &serde_json::json!({
                "query": PLAYER_QUERY,
                "variables": {
                    "sandboxId": namespace,
                    "epicAccountId": tokens.user_id,
                },
            }),
            Some(STORE_UA),
        )
        .map(|value| player_state(&value))
        .unwrap_or_default()
    };
    let mut items = Vec::new();
    let Some(list) = record.get("achievements").and_then(Value::as_array) else {
        return Ok(items);
    };
    for entry in list {
        let Some(achievement) = entry.get("achievement") else {
            continue;
        };
        let key = achievement
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if key.is_empty() {
            continue;
        }
        let (is_unlocked, unlock_time) = unlocked
            .iter()
            .find(|(name, _, _)| name == &key)
            .map(|(_, state, date)| (*state, *date))
            .unwrap_or((false, None));
        let name = text(achievement, if is_unlocked {
            "unlockedDisplayName"
        } else {
            "lockedDisplayName"
        })
        .or_else(|| text(achievement, "unlockedDisplayName"))
        .unwrap_or_default();
        let description = text(achievement, if is_unlocked {
            "unlockedDescription"
        } else {
            "lockedDescription"
        })
        .or_else(|| text(achievement, "lockedDescription"))
        .unwrap_or_default();
        let icon = text(achievement, if is_unlocked {
            "unlockedIconLink"
        } else {
            "lockedIconLink"
        })
        .or_else(|| text(achievement, "unlockedIconLink"))
        .unwrap_or_default();
        let percent = achievement
            .pointer("/rarity/percent")
            .and_then(Value::as_f64)
            .map(|percent| percent as f32);
        items.push(Achievement {
            key,
            name,
            description,
            icon,
            hidden: achievement
                .get("hidden")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            unlocked: is_unlocked,
            unlock_time,
            percent,
        });
    }
    Ok(items)
}

/// Progression du compte : nom interne, débloqué, date ISO de déblocage.
fn player_state(value: &Value) -> Vec<(String, bool, Option<i64>)> {
    let Some(records) = value
        .pointer("/data/PlayerAchievement/playerAchievementGameRecordsBySandbox/records")
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for record in records {
        let Some(list) = record
            .get("playerAchievements")
            .and_then(Value::as_array)
        else {
            continue;
        };
        for entry in list {
            let Some(state) = entry.get("playerAchievement") else {
                continue;
            };
            let Some(name) = state.get("achievementName").and_then(Value::as_str) else {
                continue;
            };
            let unlocked = state
                .get("unlocked")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let date = state
                .get("unlockDate")
                .and_then(Value::as_str)
                .and_then(crate::cloud::parse_iso8601);
            out.push((name.to_string(), unlocked, if unlocked { date } else { None }));
        }
    }
    out
}

fn text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// Liste les sauvegardes cloud du jeu.
///
/// Epic rend un objet dont `files` associe chaque nom à son empreinte, sa date
/// et ses liens signés ; les formes anciennes listaient un tableau, les deux
/// sont lues.
pub fn cloud_list(tokens: &StoreTokens, app_name: &str) -> Result<Vec<RemoteFile>> {
    if tokens.user_id.trim().is_empty() {
        return Err(Error::Expired);
    }
    let url = format!(
        "{DATASTORAGE}/api/v1/access/egstore/savesync/{}/{app_name}/",
        tokens.user_id
    );
    let value = common::get_json(&url, Some(&tokens.access_token))?;
    Ok(files_from(&value))
}

/// Télécharge un fichier de sauvegarde vers `dest`, et rend son poids.
pub fn cloud_download(
    tokens: &StoreTokens,
    app_name: &str,
    file: &RemoteFile,
    dest: &std::path::Path,
) -> Result<u64> {
    let bytes = if !file.read_url.trim().is_empty() {
        crate::cloud::get_bytes(&file.read_url, None, &[])?
    } else {
        let url = format!(
            "{DATASTORAGE}/api/v1/access/egstore/savesync/{}/{app_name}/",
            tokens.user_id
        );
        let value = common::get_json(&url, Some(&tokens.access_token))?;
        let listed = files_from(&value)
            .into_iter()
            .find(|entry| entry.name == file.name)
            .ok_or_else(|| Error::Other(format!("sauvegarde introuvable : {}", file.name)))?;
        if listed.read_url.trim().is_empty() {
            return Err(Error::Other(
                "Epic n'a pas publié de lien de lecture pour ce fichier".into(),
            ));
        }
        crate::cloud::get_bytes(&listed.read_url, None, &[])?
    };
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(dest, &bytes)?;
    Ok(bytes.len() as u64)
}

/// Envoie un fichier local dans le cloud.
///
/// Epic délivre un lien d'écriture pour les noms annoncés : le nom est d'abord
/// déclaré, puis le contenu part sur le lien signé.
pub fn cloud_upload(
    tokens: &StoreTokens,
    app_name: &str,
    file: &RemoteFile,
    body: Vec<u8>,
) -> Result<u64> {
    let url = format!(
        "{DATASTORAGE}/api/v1/access/egstore/savesync/{}/{app_name}/",
        tokens.user_id
    );
    let declared = crate::cloud::post_json(
        &url,
        Some(&tokens.access_token),
        &serde_json::json!({ "files": [file.name] }),
        None,
    )?;
    let write = files_from(&declared)
        .into_iter()
        .find(|entry| entry.name == file.name)
        .map(|entry| entry.write_url)
        .filter(|link| !link.trim().is_empty())
        .ok_or_else(|| {
            Error::Other("Epic n'a pas publié de lien d'écriture pour ce fichier".into())
        })?;
    let len = body.len() as u64;
    crate::cloud::put_bytes(
        &write,
        None,
        body,
        &[("Content-Type", "application/octet-stream")],
    )?;
    Ok(len)
}

/// Fichiers d'une réponse `savesync`, objet ou tableau.
fn files_from(value: &Value) -> Vec<RemoteFile> {
    let mut files = Vec::new();
    let push = |name: &str, entry: &Value, files: &mut Vec<RemoteFile>| {
        let modified = entry
            .get("lastModified")
            .or_else(|| entry.get("last_modified"))
            .and_then(|value| match value {
                Value::String(text) => crate::cloud::parse_iso8601(text),
                Value::Number(number) => number.as_i64(),
                _ => None,
            })
            .unwrap_or(0);
        files.push(RemoteFile {
            name: name.to_string(),
            size: entry
                .get("size")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            modified,
            hash: entry
                .get("hash")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            read_url: entry
                .get("readLink")
                .or_else(|| entry.get("read_link"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            write_url: entry
                .get("writeLink")
                .or_else(|| entry.get("write_link"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            full_name: name.to_string(),
        });
    };
    match value.get("files") {
        Some(Value::Object(map)) => {
            for (name, entry) in map {
                push(name, entry, &mut files);
            }
        }
        Some(Value::Array(items)) => {
            for entry in items {
                let name = entry
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !name.is_empty() {
                    push(name, entry, &mut files);
                }
            }
        }
        _ => {}
    }
    files.sort_by(|left, right| left.name.cmp(&right.name));
    files
}

/// Adresse du manifeste d'un build, telle que le launcher la publie.
///
/// Le jeu se désigne par trois choses : son espace de noms, son identifiant de
/// catalogue et son nom d'application. La réponse range souvent plusieurs CDN
/// pour le même build ; le premier suffit, et ses `queryParams` portent le
/// jeton du CDN.
fn build_manifest_urls(
    tokens: &StoreTokens,
    namespace: &str,
    catalog_item: &str,
    app_name: &str,
    platform: &str,
    preferred: &str,
) -> Result<Vec<String>> {
    // Le service des assets répond 404 « download_info_not_found » quand on
    // lui demande une combinaison de plateforme et d'étiquette qu'il ne
    // publie pas : les vieux jeux Epic vivent sous `Win32`, d'autres builds
    // sous `Production`, et quelques-uns n'ont pas d'étiquette du tout. Les
    // combinaisons sont donc essayées dans l'ordre, et la première publiée
    // l'emporte.
    let platforms: &[&str] = match platform {
        "windows" => &["Windows", "Win32"],
        "macos" => &["Mac"],
        _ => &["Linux", "Windows", "Win32"],
    };
    let mut value = None;
    let mut refus: Option<Error> = None;
    'candidates: for os in platforms {
        for label in [Some("Live"), Some("Production"), None] {
            let url = match label {
                Some(label) => format!(
                    "{LAUNCHER}/launcher/api/public/assets/v2/platform/{os}/namespace/{namespace}/catalogItem/{catalog_item}/app/{app_name}/label/{label}"
                ),
                None => format!(
                    "{LAUNCHER}/launcher/api/public/assets/v2/platform/{os}/namespace/{namespace}/catalogItem/{catalog_item}/app/{app_name}"
                ),
            };
            match common::get_json(&url, Some(&tokens.access_token)) {
                Ok(json) => {
                    value = Some(json);
                    break 'candidates;
                }
                Err(error) => {
                    tracing::info!("assets Epic {os} {label:?} : {error}");
                    refus = Some(error)
                }
            }
        }
    }
    // Derniers recours : l'ancien service d'assets (v1), puis le jeu nommé
    // par son identifiant de catalogue — certains billets ne portent pas de
    // nom d'application exploitable.
    if value.is_none() {
        let os = platforms[0];
        let essais = [
            format!("{LAUNCHER}/launcher/api/public/assets/{os}/Live?appName={app_name}"),
            format!(
                "{LAUNCHER}/launcher/api/public/assets/v2/platform/{os}/namespace/{namespace}/catalogItem/{catalog_item}/app/{catalog_item}/label/Live"
            ),
        ];
        for url in essais {
            match common::get_json(&url, Some(&tokens.access_token)) {
                Ok(json) => {
                    tracing::info!("assets Epic (repli) : {url}");
                    value = Some(json);
                    break;
                }
                Err(error) => tracing::info!("assets Epic (repli) {url} : {error}"),
            }
        }
    }
    let value = value.ok_or_else(|| {
        refus.unwrap_or_else(|| Error::Unavailable("Epic ne publie aucun build de ce jeu".into()))
    })?;
    // Les réponses récentes rangent les builds sous `elements` ; `items` reste
    // accepté pour les anciennes.
    let manifests = value
        .pointer("/elements/0/manifests")
        .or_else(|| value.pointer("/items/0/manifests"))
        .and_then(Value::as_array)
        .filter(|manifests| !manifests.is_empty())
        .ok_or_else(|| {
            Error::Unavailable("Epic ne publie aucun build de ce jeu pour cette plateforme")
        })?;
    // Le même build est publié sur plusieurs CDN : ils sont tous essayés, dans
    // l'ordre de la réponse, sauf celui que l'utilisateur a mis en tête.
    // Cloudflare est écarté — il refuse les chunks sans jeton et répond 403 au
    // hasard.
    let mut urls = Vec::new();
    let mut preferred_urls = Vec::new();
    for manifest in manifests {
        let Some(uri) = manifest.get("uri").and_then(Value::as_str) else {
            continue;
        };
        if uri.contains("cloudflare.epicgamescdn.com") {
            continue;
        }
        let mut url = uri.to_string();
        if let Some(params) = manifest.get("queryParams").and_then(Value::as_array) {
            let query = params
                .iter()
                .filter_map(|param| {
                    let name = param.get("name")?.as_str()?;
                    let value = param.get("value")?.as_str()?;
                    Some(format!("{name}={}", crate::gog::percent_encode(value)))
                })
                .collect::<Vec<_>>()
                .join("&");
            if !query.is_empty() {
                url.push(if url.contains('?') { '&' } else { '?' });
                url.push_str(&query);
            }
        }
        if !preferred.trim().is_empty() && cdn_matches(&url, preferred) {
            preferred_urls.push(url);
        } else {
            urls.push(url);
        }
    }
    // Le CDN choisi passe devant ; les autres restent en repli, pour qu'un CDN
    // en panne ne bloque pas l'installation.
    preferred_urls.extend(urls);
    let urls = preferred_urls;
    if urls.is_empty() {
        return Err(Error::Unavailable(
            "aucun CDN Epic ne publie ce build".into(),
        ));
    }
    Ok(urls)
}

/// Vrai si l'adresse appartient à la famille de CDN demandée.
fn cdn_matches(url: &str, family: &str) -> bool {
    match family {
        "cloudfront" => url.contains("cloudfront"),
        "akamai" => url.contains("akamai"),
        "fastly" => url.contains("fastly"),
        _ => false,
    }
}

/// Espace de noms et identifiant de catalogue d'un jeu : sans les deux, Epic
/// ne sait pas de quel build on parle.
fn asset_identity(tokens: &StoreTokens, app_name: &str) -> Result<(String, String)> {
    for record in library_records(tokens)? {
        if record.get("appName").and_then(Value::as_str) != Some(app_name) {
            continue;
        }
        let text = |key: &str| -> String {
            record
                .get(key)
                .and_then(|value| value.as_str())
                .unwrap_or("-")
                .to_string()
        };
        tracing::info!(
            "billet {app_name} : namespace={} item={} sandbox={} record={}",
            text("namespace"),
            text("catalogItemId"),
            text("sandboxName"),
            text("recordId"),
        );
        let namespace = record
            .get("namespace")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let item = record
            .get("catalogItemId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !namespace.is_empty() && !item.is_empty() {
            return Ok((namespace.to_string(), item.to_string()));
        }
    }
    Err(Error::Unavailable(
        "ce jeu n'est pas dans la bibliothèque du compte Epic",
    ))
}

/// Nom d'application publié par le catalogue pour cet article : `releaseInfo`
/// porte l'`appId` que le service des assets attend. Certains billets de
/// bibliothèque n'ont pas ce nom (le leur est un identifiant), et la demande
/// d'installation finissait en 404 « download_info_not_found ».
/// Un crédit tel que la fiche Epic le nomme (`developer`, `publisher`).
fn catalog_credit(item: &Value, key: &str) -> Vec<String> {
    item.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(|name| vec![name.to_string()])
        .unwrap_or_default()
}

fn catalog_app_id(tokens: &StoreTokens, namespace: &str, item: &str) -> Option<String> {
    let url = format!(
        "{CATALOG}/catalog/api/shared/namespace/{namespace}/bulk/items?id={item}&includeMainGameDetails=true&country=FR&locale=fr"
    );
    let value = common::get_json(&url, Some(&tokens.access_token)).ok()?;
    let entry = value.get(item)?;
    // L'`applicationId` de l'article est le nom d'application des jeux
    // récents ; `releaseInfo` garde celui des plus anciens.
    let from_item = entry
        .get("applicationId")
        .and_then(|app| app.as_str())
        .map(str::trim)
        .filter(|app| !app.is_empty())
        .map(str::to_string);
    if from_item.is_some() {
        return from_item;
    }
    entry
        .get("releaseInfo")?
        .as_array()?
        .iter()
        .filter_map(|release| release.get("appId"))
        .filter_map(|app| app.as_str())
        .map(str::trim)
        .find(|app| !app.is_empty())
        .map(str::to_string)
}

/// Plateformes publiées par le catalogue pour cet article : Epic y range
/// aussi les versions de téléphone, et un billet Android n'a aucun build PC.
/// Sans cette lecture, l'installation finissait en 404 et la fiche proposait
/// d'installer un jeu qui n'existe pas sur ordinateur.
fn catalog_platforms(tokens: &StoreTokens, namespace: &str, item: &str) -> Vec<String> {
    let url = format!(
        "{CATALOG}/catalog/api/shared/namespace/{namespace}/bulk/items?id={item}&country=FR&locale=fr"
    );
    let Some(value) = common::get_json(&url, Some(&tokens.access_token)).ok() else {
        return Vec::new();
    };
    let Some(entry) = value.get(item) else {
        return Vec::new();
    };
    let mut platforms = Vec::new();
    if let Some(releases) = entry.get("releaseInfo").and_then(|releases| releases.as_array()) {
        for release in releases {
            if let Some(list) = release.get("platform").and_then(|list| list.as_array()) {
                for platform in list {
                    if let Some(platform) = platform.as_str() {
                        platforms.push(platform.to_ascii_lowercase());
                    }
                }
            }
        }
    }
    platforms
}

/// Titre publié par le catalogue, à défaut un nom lisible tiré de l'appName.
fn catalog_title(tokens: &StoreTokens, namespace: &str, item: &str) -> String {
    let url = format!(
        "{CATALOG}/catalog/api/shared/namespace/{namespace}/bulk/items?id={item}&includeMainGameDetails=true&country=FR&locale=fr"
    );
    common::get_json(&url, Some(&tokens.access_token))
        .ok()
        .and_then(|value| {
            value
                .pointer(&format!("/{item}/title"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| humanize(item))
}

/// Poids d'un contenu additionnel : ce que son téléchargement demande et ce
/// qu'il occupe une fois installé. `None` quand la boutique ne publie pas de
/// build pour ce contenu sur cette plateforme.
pub fn dlc_build(
    tokens: &StoreTokens,
    namespace: &str,
    catalog_item: &str,
    app_name: &str,
    platform: &str,
) -> Option<(String, u64, u64)> {
    if namespace.trim().is_empty() || catalog_item.trim().is_empty() {
        return None;
    }
    // Une vue de bibliothèque ancienne peut ne pas porter le nom
    // d'application : la fiche de catalogue le publie, on le lit ici plutôt
    // que de laisser le contenu sans poids.
    let resolved = match app_name.trim() {
        "" => catalog_app_name(tokens, namespace, catalog_item)?,
        name => name.to_string(),
    };
    let urls = build_manifest_urls(tokens, namespace, catalog_item, &resolved, platform, "").ok()?;
    for url in urls {
        let Ok(bytes) = crate::cloud::get_bytes(&url, None, &[]) else {
            continue;
        };
        if let Ok(manifest) = crate::epic_manifest::parse(&bytes) {
            return Some((resolved, manifest.download_size(), manifest.install_size()));
        }
    }
    None
}

/// Nom d'application publié par la fiche de catalogue d'un article.
fn catalog_app_name(tokens: &StoreTokens, namespace: &str, item: &str) -> Option<String> {
    let url = format!(
        "{CATALOG}/catalog/api/shared/namespace/{namespace}/bulk/items?id={item}&includeMainGameDetails=true&country=FR&locale=fr"
    );
    let value = common::get_json(&url, Some(&tokens.access_token)).ok()?;
    let entry = value.get(item)?;
    entry
        .get("releaseInfo")
        .and_then(Value::as_array)
        .and_then(|releases| {
            releases.iter().find_map(|release| {
                release
                    .get("appId")
                    .and_then(Value::as_str)
                    .filter(|app| !app.trim().is_empty())
                    .map(str::to_string)
            })
        })
}

/// Lit le manifeste du build, en repliant sur Windows quand la plateforme
/// demandée n'est pas publiée.
///
/// Beaucoup de jeux Epic n'existent que pour Windows sous Linux : ils tournent
/// alors par Proton, et c'est ce build-là que la boutique donne.
fn read_manifest(
    tokens: &StoreTokens,
    store_id: &str,
    platform: &str,
    preferred_cdn: &str,
) -> Result<(crate::epic_manifest::Manifest, String, String, String, String)> {
    let (namespace, item) = asset_identity(tokens, store_id)?;
    // Le service des assets attend le nom d'application publié par le
    // catalogue, que certains billets ne portent pas : environ un jeu sur cinq
    // répondait 404 « download_info_not_found » sur le billet seul.
    let mut apps: Vec<String> = vec![store_id.to_string()];
    if let Some(app) = catalog_app_id(tokens, &namespace, &item) {
        if app != store_id {
            apps.push(app);
        }
    }
    let mut platforms: Vec<&str> = vec![platform];
    if platform == "linux" {
        platforms.push("windows");
    }
    let mut last: Option<Error> = None;
    for os in platforms {
        let mut found: Option<Vec<String>> = None;
        for app in &apps {
            match build_manifest_urls(tokens, &namespace, &item, app, os, preferred_cdn) {
                Ok(urls) => {
                    found = Some(urls);
                    break;
                }
                Err(error) => last = Some(error),
            }
        }
        let Some(urls) = found else {
            continue;
        };
        for url in urls {
            // Le manifeste vit sur le CDN, avec son propre jeton dans
            // l'adresse : il ne veut pas du jeton OAuth.
            let bytes = match crate::cloud::get_bytes(&url, None, &[]) {
                Ok(bytes) => bytes,
                Err(error) => {
                    last = Some(error);
                    continue;
                }
            };
            match crate::epic_manifest::parse(&bytes) {
                Ok(manifest) => {
                    return Ok((manifest, url, os.to_string(), namespace.clone(), item.clone()))
                }
                Err(error) => last = Some(error),
            }
        }
    }
    // Aucun build : le billet est-il seulement mobile ? La raison se dit
    // alors clairement, au lieu du 404 du service des assets.
    let published = catalog_platforms(tokens, &namespace, &item);
    if !published.is_empty()
        && !published.iter().any(|platform| {
            matches!(platform.as_str(), "windows" | "macos" | "linux")
        })
    {
        return Err(Error::Unavailable(
            "ce jeu n'existe chez Epic que pour téléphone : aucun build PC à installer".into(),
        ));
    }
    Err(last.unwrap_or_else(|| Error::Unavailable("aucun build Epic publié")))
}

/// Identité stable d'un build Epic : version du manifeste et empreintes des
/// chunks, jamais l'adresse CDN. Pour un même build, Epic change le miroir et
/// le jeton de l'adresse à chaque lecture ; l'adresse ne peut donc pas servir
/// à décider qu'une mise à jour existe.
fn manifest_build(
    manifest: &crate::epic_manifest::Manifest,
    store_id: &str,
    platform: &str,
) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for text in [
        store_id,
        platform,
        manifest.app_name.as_str(),
        manifest.build_version.as_str(),
    ] {
        for byte in text.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    for chunk in &manifest.chunks {
        for byte in chunk.hash.to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    format!("{hash:016x}")
}

/// Plan d'installation : le manifeste du build et le poids de ses chunks.
pub fn install_plan(
    tokens: &StoreTokens,
    store_id: &str,
    platform: &str,
    cdn: &str,
) -> Result<InstallPlan> {
    let (manifest, url, os, namespace, item) = read_manifest(tokens, store_id, platform, cdn)?;
    let name = catalog_title(tokens, &namespace, &item);
    let size = manifest.download_size();
    let installed = manifest.install_size();
    let build = manifest_build(&manifest, store_id, &os);
    Ok(InstallPlan {
        store: Store::Epic,
        store_id: store_id.to_string(),
        name: name.clone(),
        installdir: common::install_dir(&name),
        size,
        installed,
        platform: os.clone(),
        build,
        steps: vec![crate::common::Step::EpicBuild { url, size }],
    })
}

/// Télécharge les chunks du build puis recopie chaque tranche à sa place.
///
/// Le manifeste donne, pour chaque fichier, la liste de ses tranches : chunk,
/// décalage dans le chunk, taille. Un chunk peut servir à plusieurs fichiers ;
/// il n'est donc lu qu'une fois, et effacé du cache dès qu'il a servi — un jeu
/// de cent gigaoctets ne doit pas laisser un cache aussi gros.
pub fn install(
    _tokens: &StoreTokens,
    plan: &InstallPlan,
    cache: &std::path::Path,
    dest: &std::path::Path,
    progress: &mut dyn FnMut(u64, u64) -> bool,
) -> Result<()> {
    let url = plan
        .steps
        .iter()
        .find_map(|step| match step {
            crate::common::Step::EpicBuild { url, .. } => Some(url.clone()),
            _ => None,
        })
        .ok_or_else(|| Error::Other("plan d'installation Epic sans manifeste".into()))?;
    let bytes = crate::cloud::get_bytes(&url, None, &[])?;
    let manifest = crate::epic_manifest::parse(&bytes)?;
    download_build(&manifest, &url, cache, dest, progress)
}

/// Décalage d'une tranche dans son fichier, fichier par fichier.
struct Target {
    path: std::path::PathBuf,
    size: u64,
    executable: bool,
}

fn download_build(
    manifest: &crate::epic_manifest::Manifest,
    manifest_url: &str,
    cache: &std::path::Path,
    dest: &std::path::Path,
    progress: &mut dyn FnMut(u64, u64) -> bool,
) -> Result<()> {
    // Le dossier du manifeste sur le CDN est la base des chunks.
    let clean = manifest_url.split('?').next().unwrap_or(manifest_url);
    let base = &clean[..clean.rfind('/').map(|index| index + 1).unwrap_or(0)];
    let total = manifest.download_size();

    std::fs::create_dir_all(dest)?;
    let mut targets: Vec<Target> = Vec::with_capacity(manifest.files.len());
    // Par chunk : (fichier, décalage dans le fichier, décalage dans le chunk,
    // taille de la tranche).
    let mut wanted: std::collections::HashMap<[u32; 4], Vec<(usize, u64, u64, u64)>> =
        std::collections::HashMap::new();
    for (index, file) in manifest.files.iter().enumerate() {
        let path = common::join_rel(dest, &file.path)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let size = file.size();
        // Le fichier n'est pas tronqué s'il existe déjà : une installation
        // relancée par-dessus, puis annulée, laissait sinon tous les fichiers
        // pleins de zéros — taille juste, contenu perdu. Seule la taille est
        // ajustée, et les tranches écrivent par-dessus l'ancien contenu.
        let handle = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;
        if handle.metadata()?.len() != size {
            handle.set_len(size)?;
        }
        drop(handle);
        let mut offset = 0u64;
        for part in &file.parts {
            wanted.entry(part.chunk).or_default().push((
                index,
                offset,
                part.offset as u64,
                part.size as u64,
            ));
            offset += part.size as u64;
        }
        targets.push(Target {
            path,
            size,
            executable: file.executable(),
        });
    }

    let chunk_dir = cache.join("epic");
    std::fs::create_dir_all(&chunk_dir)?;
    let mut done = 0u64;
    for chunk in &manifest.chunks {
        let Some(parts) = wanted.get(&chunk.guid) else {
            continue;
        };
        let expected = chunk.download_size();
        // Un chunk sans données (taille négative dans le manifeste) se contente
        // de laisser des zéros : rien à télécharger.
        if expected == 0 {
            continue;
        }
        let name = sanitize(&manifest.chunk_path(chunk));
        let file = chunk_dir.join(&name);
        let present = std::fs::metadata(&file)
            .map(|meta| meta.len() > 0 && meta.len() == expected)
            .unwrap_or(false);
        if !present {
            // Le manifeste vit à la racine du build ; les chunks sont rangés
            // sous leur dossier de version (`ChunksV4`, `ChunksV5`…), qu'il
            // faut remettre dans l'adresse — sans lui, le CDN refuse.
            let url = format!(
                "{base}{}/{}",
                manifest.chunk_dir(),
                manifest.chunk_path(chunk)
            );
            common::download(&url, None, &file, expected, &mut |received, _| {
                progress(done + received, total)
            })?;
        }
        done += expected;
        if !progress(done, total) {
            let _ = std::fs::remove_file(&file);
            return Err(Error::Other("téléchargement annulé".into()));
        }

        // Le chunk arrive avec son en-tête CDN, souvent compressé : ses
        // données utiles se lisent après décompression, sinon les fichiers
        // recoivent des octets d'en-tête.
        let raw = std::fs::read(&file)?;
        let data = crate::epic_manifest::chunk_payload(&raw)?;
        let mut source = std::io::Cursor::new(data);
        let mut buffer = vec![0u8; 1024 * 1024];
        place_parts(&mut source, parts, &targets, &mut buffer).map_err(|error| match error {
            Error::Parse(_) => Error::Parse(format!(
                "chunk {} plus court que son manifeste",
                manifest.chunk_path(chunk)
            )),
            other => other,
        })?;
        // Le chunk a servi : il quitte le cache pour ne pas doubler le jeu.
        let _ = std::fs::remove_file(&file);
    }

    for target in &targets {
        if target.executable {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(
                    &target.path,
                    std::fs::Permissions::from_mode(0o755),
                );
            }
        }
        if target.size == 0 {
            let _ = std::fs::File::create(&target.path);
        }
    }
    Ok(())
}

/// Recopie les tranches d'un chunk dans leurs fichiers respectifs.
///
/// `parts` porte, pour chaque tranche : index du fichier, décalage dans le
/// fichier, décalage dans le chunk, taille. L'écriture se fait à la position
/// exacte, les fichiers ayant été dimensionnés d'avance.
fn place_parts(
    source: &mut (impl std::io::Read + std::io::Seek),
    parts: &[(usize, u64, u64, u64)],
    targets: &[Target],
    buffer: &mut [u8],
) -> Result<()> {
    use std::io::{Seek, SeekFrom, Write};
    for (index, file_offset, part_offset, part_size) in parts {
        let target = targets
            .get(*index)
            .ok_or_else(|| Error::Other("manifeste incohérent".into()))?;
        let mut out = std::fs::OpenOptions::new()
            .write(true)
            .open(&target.path)?;
        out.seek(SeekFrom::Start(*file_offset))?;
        source.seek(SeekFrom::Start(*part_offset))?;
        let mut left = *part_size;
        while left > 0 {
            let want = (left as usize).min(buffer.len());
            let read = source.read(&mut buffer[..want])?;
            if read == 0 {
                return Err(Error::Parse("chunk trop court".into()));
            }
            out.write_all(&buffer[..read])?;
            left -= read as u64;
        }
        out.flush()?;
    }
    Ok(())
}

/// Nom de fichier sûr pour le cache : les séparateurs du chemin CDN deviennent
/// des tirets, un nom venu du réseau ne peut pas sortir du dossier.
fn sanitize(path: &str) -> String {
    let name: String = path
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '-' | '_' => c,
            _ => '-',
        })
        .collect();
    // Un nom fait uniquement de points resterait `.` ou `..` : il désignerait
    // le dossier parent une fois posé sur le disque.
    if name.trim_matches('.').is_empty() {
        return "chunk".to_string();
    }
    name
}

/// Date ISO 8601 de la boutique en secondes Unix. Epic écrit
/// `2026-10-19T15:00:00.000Z` ; la conversion passe par l'algorithme civil de
/// Hinnant, le même que `util::format_date` en sens inverse.
fn unix_of(text: &str) -> i64 {
    if text.len() < 19 {
        return 0;
    }
    let number = |range: std::ops::Range<usize>| -> i64 {
        text.get(range)
            .and_then(|part| part.parse::<i64>().ok())
            .unwrap_or(0)
    };
    let (year, month, day) = (number(0..4), number(5..7), number(8..10));
    let (hour, minute, second) = (number(11..13), number(14..16), number(17..19));
    if month == 0 || day == 0 {
        return 0;
    }
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let shifted_month = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era =
        year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    days * 86_400 + hour * 3_600 + minute * 60 + second
}

/// Fenêtre d'une offre à 0 % dans une des listes de promotions d'Epic.
fn zero_offer(groups: Option<&Value>) -> Option<(i64, i64)> {
    for group in groups?.as_array()? {
        for offer in group.get("promotionalOffers")?.as_array()? {
            let percentage = offer
                .pointer("/discountSetting/discountPercentage")
                .and_then(Value::as_i64)
                .unwrap_or(-1);
            if percentage != 0 {
                continue;
            }
            let start = offer
                .get("startDate")
                .and_then(Value::as_str)
                .map(unix_of)
                .unwrap_or(0);
            let end = offer
                .get("endDate")
                .and_then(Value::as_str)
                .map(unix_of)
                .unwrap_or(0);
            if end > 0 {
                return Some((start, end));
            }
        }
    }
    None
}

/// Jeux gratuits d'Epic : ceux en cours et ceux de la semaine prochaine.
///
/// Le flux ne dit pas sur quelle plateforme un jeu est publié ; il porte la
/// fenêtre de l'offre, sa valeur, son visuel et son identifiant d'offre — ce
/// qu'il faut pour l'afficher et la rattacher au compte. L'offre vaut pour le
/// compte, donc pour toutes les plateformes où le jeu existe.
pub fn free_games(locale: &str) -> Result<Vec<FreeGame>> {
    let (country, language) = locale_choice(locale);
    let language = if language == "fr" { "fr-FR" } else { language };
    let url = format!(
        "{FREE_GAMES}?locale={language}&country={country}&allowCountries={country}"
    );
    let value = common::get_json(&url, None)?;
    let elements = value
        .pointer("/data/Catalog/searchStore/elements")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::Parse("Epic n'a pas publié la liste des jeux gratuits".into()))?;
    let now = lapinux_core::util::now_unix();
    let mut games = Vec::new();
    for element in elements {
        let promotions = element.get("promotions");
        let current = zero_offer(promotions.and_then(|value| value.get("promotionalOffers")))
            .filter(|(start, end)| now >= *start && now < *end);
        let next = zero_offer(
            promotions.and_then(|value| value.get("upcomingPromotionalOffers")),
        );
        let (start, end, upcoming) = match (current, next) {
            (Some((start, end)), _) => (start, end, false),
            (None, Some((start, end))) => (start, end, true),
            (None, None) => continue,
        };
        let namespace = element.get("namespace").and_then(Value::as_str).unwrap_or_default();
        let offer = element.get("id").and_then(Value::as_str).unwrap_or_default();
        if namespace.is_empty() || offer.is_empty() {
            continue;
        }
        let slug = element
            .pointer("/offerMappings/0/pageSlug")
            .and_then(Value::as_str)
            .or_else(|| element.get("urlSlug").and_then(Value::as_str))
            .unwrap_or_default();
        let cover = ["OfferImageWide", "OfferImageTall", "Thumbnail"]
            .iter()
            .find_map(|wanted| {
                element
                    .get("keyImages")
                    .and_then(Value::as_array)?
                    .iter()
                    .find(|image| image.get("type").and_then(Value::as_str) == Some(wanted))
                    .and_then(|image| image.get("url"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_default();
        let title = element
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if title.is_empty() {
            continue;
        }
        games.push(FreeGame {
            store: Store::Epic,
            // L'appartenance se vérifie avec le compte, une fois la liste
            // lue : le flux public ne sait pas ce qu'on possède.
            owned: false,
            id: format!("{namespace}:{offer}"),
            description: common::plain_text(
                element
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            ),
            cover,
            url: format!(
                "https://store.epicgames.com/{}/p/{slug}",
                if language.starts_with("fr") { "fr" } else { "en-US" }
            ),
            seller: element
                .pointer("/seller/name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            value: element
                .pointer("/price/totalPrice/fmtPrice/originalPrice")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            start,
            end,
            upcoming,
            // Le flux ne dit pas sur quelles plateformes le jeu est publié :
            // l'offre, elle, est rattachée au compte et vaut donc partout où
            // le jeu existe, y compris dans l'application mobile.
            platforms: vec!["pc".to_string()],
            browser_only: false,
            offers: Vec::new(),
            title,
        });
    }
    games.sort_by(|left, right| left.end.cmp(&right.end).then(left.title.cmp(&right.title)));
    Ok(games)
}

/// Offres déjà rattachées au compte, sous la forme `<espace>:<identifiant>`.
///
/// Le service des droits rend ce que le compte possède réellement, y compris
/// les offres récupérées gratuitement : c'est lui qui décide si une offre
/// gratuite est « déjà récupérée ». Les droits révoqués sont ignorés.
pub fn owned_offers(tokens: &StoreTokens) -> Result<std::collections::HashSet<String>> {
    let mut owned = std::collections::HashSet::new();
    if tokens.user_id.trim().is_empty() {
        return Ok(owned);
    }
    let mut start = 0usize;
    for _ in 0..20 {
        let url = format!(
            "{ENTITLEMENT}/entitlement/api/account/{}/entitlements?start={start}&count=1000",
            tokens.user_id
        );
        let value = common::get_json(&url, Some(&tokens.access_token))?;
        let Some(items) = value.as_array() else {
            break;
        };
        for item in items {
            if item.get("status").and_then(Value::as_str) == Some("REVOKED") {
                continue;
            }
            let Some(namespace) = item.get("namespace").and_then(Value::as_str) else {
                continue;
            };
            if namespace.is_empty() {
                continue;
            }
            // Un droit porte l'offre achetée (`offerId`) et l'article qu'elle
            // contient (`catalogItemId`) : les offres gratuites publiées par la
            // boutique s'identifient tantôt par l'un, tantôt par l'autre.
            for key in ["offerId", "catalogItemId"] {
                if let Some(id) = item.get(key).and_then(Value::as_str) {
                    if !id.trim().is_empty() {
                        owned.insert(format!("{namespace}:{id}"));
                    }
                }
            }
        }
        if items.len() < 1000 {
            break;
        }
        start += items.len();
    }
    Ok(owned)
}

/// Jeux gratuits mobiles (Android et iOS).
///
/// L'offre est celle de l'application Epic Games : elle porte déjà son prix
/// nul (`purchaseType: "Claim"`), son visuel, son identifiant d'offre et son
/// espace de noms, et le lien de sa page. La récupération se fait sur cette
/// page — l'API du launcher ne gère pas ces offres —, d'où `browser_only`.
pub fn mobile_free_games(locale: &str) -> Result<Vec<FreeGame>> {
    let (country, language) = locale_choice(locale);
    let lang = if language == "fr" { "fr" } else { "en" };
    let mut games: Vec<FreeGame> = Vec::new();
    for platform in ["android", "ios"] {
        let url = format!(
            "{MOBILE_DISCOVER}?count=10&country={country}&locale={lang}&platform={platform}&start=0&store=EGS"
        );
        // Le service de l'application mobile refuse tout autre agent (403) :
        // sous l'identité du launcher, il répond comme à l'application.
        let Ok(value) = common::get_json_as(&url, None, STORE_UA) else {
            continue;
        };
        let Some(modules) = value.get("data").and_then(Value::as_array) else {
            continue;
        };
        for module in modules {
            // Seul le module `freeGame` porte l'offre de la semaine ; ses
            // voisins s'appellent aussi « Jeux gratuits » mais listent des
            // titres gratuits en permanence, sans rien à récupérer.
            if module.get("type").and_then(Value::as_str) != Some("freeGame") {
                continue;
            }
            for offer in module
                .get("offers")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let Some(content) = offer.get("content") else {
                    continue;
                };
                let Some(claim) = content
                    .get("purchase")
                    .and_then(Value::as_array)
                    .and_then(|purchases| {
                        purchases.iter().find(|purchase| {
                            purchase.get("purchaseType").and_then(Value::as_str)
                                == Some("Claim")
                        })
                    })
                else {
                    continue;
                };
                let (Some(sandbox), Some(offer_id)) = (
                    offer.get("sandboxId").and_then(Value::as_str),
                    offer.get("offerId").and_then(Value::as_str),
                ) else {
                    continue;
                };
                let title = content
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if title.is_empty() {
                    continue;
                }
                let slug = content
                    .pointer("/mapping/slug")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let cover = ["card16x9", "card3x4", "coverImage", "appIcon"]
                    .iter()
                    .find_map(|wanted| {
                        content
                            .pointer(&format!("/media/{wanted}/imageSrc"))
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    })
                    .unwrap_or_default();
                let end = claim
                    .pointer("/discount/discountEndDate")
                    .and_then(Value::as_str)
                    .and_then(crate::cloud::parse_iso8601)
                    .unwrap_or(0);
                let value = claim
                    .pointer("/discount/originalPriceDisplay")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                // La même offre existe par plateforme : une seule carte, qui
                // porte Android et iOS, plutôt que deux cartes jumelles.
                if let Some(existing) = games
                    .iter_mut()
                    .find(|game| game.title.eq_ignore_ascii_case(&title) && game.end == end)
                {
                    let label = platform.to_string();
                    if !existing.platforms.contains(&label) {
                        existing.platforms.push(label);
                    }
                    // Android et iOS portent chacun leur offre : sans la
                    // seconde, le panier ne recevait qu'une plateforme.
                    let other = format!("{sandbox}:{offer_id}");
                    if existing.id != other && !existing.offers.contains(&other) {
                        existing.offers.push(other);
                    }
                    continue;
                }
                games.push(FreeGame {
                    store: Store::Epic,
                    id: format!("{sandbox}:{offer_id}"),
                    title,
                    description: content
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    cover,
                    url: format!("https://store.epicgames.com/{lang}/p/{slug}"),
                    seller: content
                        .get("publisherDisplayName")
                        .and_then(Value::as_str)
                        .unwrap_or("Epic Games")
                        .to_string(),
                    value,
                    start: 0,
                    end,
                    upcoming: false,
                    platforms: vec![platform.to_string()],
                    owned: false,
                    browser_only: true,
                    offers: Vec::new(),
                });
            }
        }
    }
    Ok(games)
}

/// Marque les offres gratuites que le compte possède déjà.
///
/// Un droit d'Epic porte l'article (`catalogItemId`), pas l'offre : c'est
/// l'offre du catalogue qui donne ses articles. On lit donc les offres par
/// paquet d'espace de noms, puis on compare aux droits du compte — sinon une
/// offre déjà récupérée serait proposée à nouveau, et la boutique refuserait
/// la commande.
pub fn mark_owned(tokens: &StoreTokens, games: &mut [FreeGame]) -> Result<()> {
    if games.is_empty() {
        return Ok(());
    }
    let owned = owned_offers(tokens)?;
    if owned.is_empty() {
        return Ok(());
    }
    let mut by_namespace: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for game in games.iter() {
        for id in std::iter::once(&game.id).chain(game.offers.iter()) {
            if let Some((namespace, offer)) = id.split_once(':') {
                by_namespace
                    .entry(namespace.to_string())
                    .or_default()
                    .push(offer.to_string());
            }
        }
    }
    let mut held: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (namespace, offers) in by_namespace {
        let query = offers
            .iter()
            .map(|offer| format!("id={offer}"))
            .collect::<Vec<_>>()
            .join("&");
        let url = format!(
            "{CATALOG}/catalog/api/shared/namespace/{namespace}/bulk/offers?{query}"
        );
        let Ok(value) = common::get_json(&url, Some(&tokens.access_token)) else {
            continue;
        };
        let Some(map) = value.as_object() else {
            continue;
        };
        for (offer, detail) in map {
            let items = detail
                .get("items")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.get("id").and_then(Value::as_str))
                        .any(|item| owned.contains(&format!("{namespace}:{item}")))
                })
                .unwrap_or(false);
            if items {
                held.insert(format!("{namespace}:{offer}"));
            }
        }
    }
    for game in games.iter_mut() {
        // Une carte ne vaut pour acquise que si **toutes** ses plateformes le
        // sont : sinon l'offre encore libre (iOS, par exemple) ne serait plus
        // jamais proposée.
        let ids: Vec<&String> = std::iter::once(&game.id).chain(game.offers.iter()).collect();
        if !ids.is_empty() && ids.iter().all(|id| held.contains(*id)) {
            game.owned = true;
        }
    }
    Ok(())
}

/// Rattache au compte les offres gratuites désignées. Chacune est une commande
/// à zéro : Epic peut en refuser une (déjà possédée, région, vérification)
/// sans que les autres échouent.
pub fn claim_free(
    tokens: &StoreTokens,
    games: &[FreeGame],
    locale: &str,
) -> Result<Vec<ClaimOutcome>> {
    if tokens.user_id.trim().is_empty() {
        return Err(Error::Unavailable(
            "compte Epic sans identifiant : reconnecte-le dans Réglages → Comptes".into(),
        ));
    }
    let (country, language) = locale_choice(locale);
    let url = format!(
        "{ORDERS}/orderprocessor/api/shared/accounts/{}/orders/quickPurchase?country={country}&locale={language}",
        tokens.user_id
    );
    let mut outcomes = Vec::new();
    // Les offres dont la commande est partie : Epic crée le droit juste
    // après, il faut le voir apparaître pour dire que c'est récupéré.
    let mut claimed: Vec<FreeGame> = Vec::new();
    for game in games {
        if game.owned {
            outcomes.push(ClaimOutcome {
                id: game.id.clone(),
                title: game.title.clone(),
                ok: true,
                error: String::new(),
                needs_checkout: false,
            });
            continue;
        }
        let Some((namespace, offer)) = game.id.split_once(':') else {
            outcomes.push(ClaimOutcome {
                id: game.id.clone(),
                title: game.title.clone(),
                ok: false,
                error: "offre sans identifiant exploitable".into(),
                needs_checkout: false,
            });
            continue;
        };
        // La commande du launcher : le canal et la source sont ceux d'un
        // achat depuis le client, la ligne porte l'offre et son espace.
        let body = serde_json::json!({
            "salesChannel": "Launcher-purchase-client",
            "entitlementSource": "Launcher-purchase-client",
            "returnSplitPaymentItems": false,
            "lineOffers": [{ "offerId": offer, "quantity": 1, "namespace": namespace }],
        });
        let mut reply = common::post_json_bearer(&url, &tokens.access_token, &body);
        // Une rafale d'offres peut être limitée un instant : un seul nouvel
        // essai, après une pause, plutôt qu'un échec définitif.
        if let Err(error) = &reply {
            if error.to_string().contains("(429)") {
                std::thread::sleep(std::time::Duration::from_secs(5));
                reply = common::post_json_bearer(&url, &tokens.access_token, &body);
            }
        }
        let outcome = match reply {
            Ok(value) => {
                // Epic répond « CHECKOUT » quand il attend un passage en caisse
                // au lieu de créer le droit : la commande ne peut pas être
                // finalisée depuis l'API, la page de l'offre le fait.
                if value.get("quickPurchaseStatus").and_then(Value::as_str)
                    == Some("CHECKOUT")
                {
                    ClaimOutcome {
                        id: game.id.clone(),
                        title: game.title.clone(),
                        ok: false,
                        error: "Epic demande de terminer sur la boutique".into(),
                        needs_checkout: true,
                    }
                } else {
                    claimed.push(game.clone());
                    ClaimOutcome {
                        id: game.id.clone(),
                        title: game.title.clone(),
                        ok: true,
                        error: String::new(),
                        needs_checkout: false,
                    }
                }
            }
            Err(error) => {
                // Epic répond parfois « déjà possédé » au lieu d'accepter une
                // commande à zéro : ce n'est pas un échec pour l'utilisateur.
                let message = error.to_string();
                let already = message.to_lowercase();
                let known = already.contains("already own")
                    || already.contains("already_owned")
                    || already.contains("already have");
                ClaimOutcome {
                    id: game.id.clone(),
                    title: game.title.clone(),
                    ok: known,
                    error: if known { String::new() } else { message },
                    needs_checkout: false,
                }
            }
        };
        outcomes.push(outcome);
    }
    // Vérification : la commande ne vaut que si le droit est là. Epic le crée
    // avec un court délai, d'où quelques relevés espacés plutôt qu'un seul.
    for attempt in 0..5 {
        if claimed.iter().all(|game| game.owned) {
            break;
        }
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(2500));
        }
        let _ = mark_owned(tokens, &mut claimed);
    }
    for outcome in &mut outcomes {
        if !outcome.ok {
            continue;
        }
        if let Some(game) = claimed.iter().find(|game| game.id == outcome.id) {
            if !game.owned {
                outcome.ok = false;
                outcome.error =
                    "Epic n'a pas confirmé la commande : rouvre la page dans un instant".into();
            }
        }
    }
    Ok(outcomes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_login_link_asks_for_a_code() {
        let link = login_link();
        assert!(link.url.starts_with("https://www.epicgames.com/id/login?redirectUrl="));
        assert!(link.url.contains("clientId%3D"));
        assert!(link.url.contains("responseType%3Dcode"));
        assert!(link.hint.contains("authorizationCode"));
    }

    #[test]
    fn token_responses_keep_the_account_and_the_delay() {
        let value = json!({
            "access_token": "access",
            "refresh_token": "refresh",
            "expires_in": 7200,
            "account_id": "abc",
            "displayName": "Nathounette"
        });
        let tokens = tokens_from(&value).expect("jetons");
        assert_eq!(tokens.store, Store::Epic);
        assert_eq!(tokens.account, "Nathounette");
        assert_eq!(tokens.user_id, "abc");
        assert_eq!(tokens.expires_at, lapinux_core::util::now_unix() + 7200);
        assert!(tokens_from(&json!({})).is_err());
    }

    #[test]
    fn savesync_files_are_read_in_both_shapes() {
        // Forme actuelle : un objet par nom, avec ses liens signés.
        let object = serde_json::json!({"files": {
            "slot1.sav": {
                "hash": "abc",
                "lastModified": "2026-06-12T09:17:00Z",
                "readLink": "https://cdn/slot1",
                "writeLink": "https://cdn/slot1?write"
            }
        }});
        let files = files_from(&object);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].name, "slot1.sav");
        assert_eq!(files[0].modified, 1_781_255_820);
        assert!(files[0].read_url.ends_with("slot1"));
        assert!(files[0].write_url.ends_with("slot1?write"));
        // Forme en tableau, plus ancienne.
        let array = serde_json::json!({"files": [
            {"name": "slot2.sav", "size": 2048, "lastModified": 1_781_255_820}
        ]});
        let files = files_from(&array);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].size, 2048);
        assert_eq!(files[0].modified, 1_781_255_820);
        // Une réponse vide ne fait pas d'erreur.
        assert!(files_from(&serde_json::json!({})).is_empty());
    }

    #[test]
    fn the_player_progress_marks_what_is_won() {
        let value = serde_json::json!({"data": {"PlayerAchievement": {
            "playerAchievementGameRecordsBySandbox": {"records": [
                {"playerAchievements": [
                    {"playerAchievement": {
                        "achievementName": "ACH_ONE",
                        "unlocked": true,
                        "unlockDate": "2026-06-12T09:17:00.000Z"
                    }},
                    {"playerAchievement": {"achievementName": "ACH_TWO", "unlocked": false}}
                ]}
            ]}
        }}});
        let state = player_state(&value);
        assert_eq!(state.len(), 2);
        assert_eq!(state[0].0, "ACH_ONE");
        assert!(state[0].1);
        assert_eq!(state[0].2, Some(1_781_255_820));
        // Un succès verrouillé n'a pas de date, même si le champ traîne.
        assert_eq!(state[1].2, None);
    }

    /// Le recollage : chaque tranche tombe à sa place, dans le fichier qui
    /// l'attend, y compris quand plusieurs fichiers partagent un chunk.
    #[test]
    fn chunk_parts_are_glued_back_at_their_offsets() {
        use std::io::Write;
        let root = std::env::temp_dir().join(format!(
            "lapinux-epic-parts-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // Un chunk de 16 octets, deux fichiers qui s'y taillent une part.
        let chunk = root.join("chunk");
        let mut file = std::fs::File::create(&chunk).unwrap();
        file.write_all(b"0123456789abcdef").unwrap();
        file.flush().unwrap();

        let first = root.join("premier.bin");
        let second = root.join("second.bin");
        for (path, size) in [(&first, 6u64), (&second, 10u64)] {
            let handle = std::fs::File::create(path).unwrap();
            handle.set_len(size).unwrap();
        }
        let targets = vec![
            Target {
                path: first.clone(),
                size: 6,
                executable: false,
            },
            Target {
                path: second.clone(),
                size: 10,
                executable: false,
            },
        ];
        let mut source = std::fs::File::open(&chunk).unwrap();
        let mut buffer = vec![0u8; 4];
        place_parts(
            &mut source,
            &[(0, 0, 10, 6), (1, 0, 0, 10)],
            &targets,
            &mut buffer,
        )
        .expect("recollage");
        assert_eq!(std::fs::read(&first).unwrap(), b"abcdef");
        assert_eq!(std::fs::read(&second).unwrap(), b"0123456789");

        // Un chunk plus court que la tranche annoncée est une erreur, pas un
        // fichier silencieusement troué.
        let mut source = std::fs::File::open(&chunk).unwrap();
        assert!(place_parts(&mut source, &[(1, 0, 0, 40)], &targets, &mut buffer).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_expired_code_says_to_start_over() {
        let refused = Error::Refused {
            status: 400,
            message: r#"{"errorCode":"errors.com.epicgames.account.oauth.authorization_code_not_found","numericErrorCode":18059}"#
                .into(),
        };
        let text = explain_login_refusal(refused).to_string();
        assert!(text.contains("expiré"), "{text}");
        assert!(text.contains("quelques instants"), "{text}");
        assert!(!text.contains("errorCode"), "le JSON brut ne s'affiche plus");

        // Les autres refus gardent leur message : ils apprennent quelque chose.
        let other = Error::Refused {
            status: 500,
            message: "service indisponible".into(),
        };
        assert!(explain_login_refusal(other).to_string().contains("indisponible"));
    }

    #[test]
    fn cache_names_cannot_leave_their_folder() {
        assert_eq!(sanitize("03/hash_guid.chunk"), "03-hash_guid.chunk");
        // Les séparateurs deviennent des tirets : plus de dossier à remonter.
        let escaped = sanitize("../../evil");
        assert!(!escaped.contains('/') && !escaped.contains('\\'), "{escaped}");
        assert_eq!(escaped, "..-..-evil");
        assert_eq!(sanitize(".."), "chunk");
    }

    #[test]
    fn library_records_give_stable_identifiers() {
        // Un enregistrement sans appName est écarté : il ne nomme aucun jeu.
        let record = json!({"appName": "Sugar", "namespace": "ns", "catalogItemId": "item"});
        let app_name = record["appName"].as_str().unwrap();
        assert_eq!(store_appid(Store::Epic, app_name), store_appid(Store::Epic, "Sugar"));
        assert_eq!(humanize("Sugar"), "Sugar");
    }

    #[test]
    fn marketplace_records_are_tools() {
        // Les assets d'Unreal et de Fab vivent dans la bibliothèque du compte,
        // sous leurs propres sables : ce ne sont pas des jeux.
        let asset = json!({
            "appName": "UE_4.27Chaos",
            "namespace": "ue",
            "sandboxName": "UE Marketplace"
        });
        assert!(record_is_tool(&asset));
        let fab = json!({
            "appName": "2bda08f9230144a19e98373cc4a6ac2d",
            "namespace": "89efe5924d3d467c839449ab6ab52e7f",
            "sandboxName": "fab-listing-live"
        });
        assert!(record_is_tool(&fab));
        // Un jeu du magasin garde sa place dans la grille.
        let game = json!({
            "appName": "Sugar",
            "namespace": "f3720f53cccb4a20841a392a8ea0457f",
            "sandboxName": "AER: Memories of Old"
        });
        assert!(!record_is_tool(&game));
    }
}

/// Date d'obtention publiée par l'enregistrement de bibliothèque.
///
/// Les services d'Epic ne nomment pas ce champ de la même façon : la liste du
/// launcher le tait parfois, les métadonnées de l'enregistrement le portent.
fn acquired_stamp(record: &Value) -> Option<i64> {
    const KEYS: [&str; 6] = [
        "acquisitionDate",
        "grantedDate",
        "creationDate",
        "createdAt",
        "acquiredDate",
        "creation_time",
    ];
    let mut places = vec![record];
    if let Some(metadata) = record.get("metadata") {
        places.push(metadata);
    }
    for place in places {
        for key in KEYS {
            if let Some(stamp) = place
                .get(key)
                .and_then(Value::as_str)
                .and_then(lapinux_core::stores::parse_stamp)
            {
                return Some(stamp);
            }
        }
    }
    None
}

/// Minutes jouées publiées par la bibliothèque d'Epic, par article.
///
/// Le service rend un tableau plat `{accountId, artifactId, totalTime}`, la
/// durée étant en secondes. `artifactId` est l'`appName` du jeu — celui que la
/// bibliothèque garde comme identifiant de boutique —, l'identifiant de
/// catalogue servant de repli : le rapprochement est direct.
fn playtimes(tokens: &StoreTokens) -> std::collections::HashMap<String, u64> {
    let mut minutes = std::collections::HashMap::new();
    if tokens.user_id.trim().is_empty() {
        return minutes;
    }
    // Trois chemins connus du service de bibliothèque : le premier accepte le
    // jeton du launcher, les autres servent de repli si Epic déplace le sien.
    let candidates = [
        format!("{LIBRARY}/library/api/public/playtime/account/{}/all", tokens.user_id),
        format!("{LIBRARY}/library/api/public/playtime/account/{}", tokens.user_id),
        format!("{LIBRARY}/library/api/v1/playtime/account/{}/all", tokens.user_id),
    ];
    for url in candidates {
        match common::get_json(&url, Some(&tokens.access_token)) {
            Ok(value) => {
                let items = value
                    .as_array()
                    .cloned()
                    .or_else(|| value.get("playtimes").and_then(Value::as_array).cloned())
                    .unwrap_or_default();
                tracing::info!("temps de jeu Epic : {} entrée(s) via {url}", items.len());
                for item in items {
                    let Some(artifact) = item.get("artifactId").and_then(Value::as_str) else {
                        continue;
                    };
                    let Some(total) = item
                        .get("totalTime")
                        .or_else(|| item.get("totalMinutes"))
                        .and_then(Value::as_u64)
                    else {
                        continue;
                    };
                    // `totalTime` est en secondes, comme le champ `Playtime` de
                    // Playnite : la bibliothèque garde des minutes.
                    if total >= 60 {
                        minutes.insert(artifact.to_string(), total / 60);
                    }
                }
                if !minutes.is_empty() {
                    return minutes;
                }
            }
            Err(error) => tracing::warn!("temps de jeu Epic : {url} a échoué ({error})"),
        }
    }
    minutes
}

#[cfg(test)]
mod about_tests {
    use super::*;

    #[test]
    fn the_product_page_description_is_read_and_cleaned() {
        let value = serde_json::json!({
            "pages": [
                { "_title": "home", "data": { "about": { "description": "<p>Une longue <b>présentation</b>.</p>" } } },
                { "_title": "offer", "data": { "about": { "description": "" } } }
            ]
        });
        let text = about_description(&value).expect("présentation lue");
        assert!(text.contains("Une longue") && text.contains("présentation"));
        assert!(!text.contains('<'));
    }

    #[test]
    fn a_product_page_without_a_description_adds_nothing() {
        let value = serde_json::json!({ "pages": [{ "_title": "home", "data": {} }] });
        assert!(about_description(&value).is_none());
        assert!(about_description(&serde_json::json!({})).is_none());
    }

    #[test]
    fn the_product_home_configuration_carries_the_whole_presentation() {
        let value = serde_json::json!({
            "data": {
                "Product": {
                    "sandbox": {
                        "configuration": [
                            { "configs": { "shortDescription": "Accroche" } },
                            {
                                "__typename": "HomeConfiguration",
                                "configs": {
                                    "longDescription": "<!--textBlock-->\n<!--title-->\n# Danger has escalated\n<!--text-->\nUne longue présentation, avec ses détails."
                                }
                            }
                        ]
                    }
                }
            }
        });
        let text = product_home_description(&value).expect("présentation lue");
        assert!(text.contains("# Danger has escalated"), "{text}");
        assert!(
            text.contains("Une longue présentation, avec ses détails."),
            "{text}"
        );
        assert!(!text.contains("Accroche"), "{text}");
        assert!(!text.contains("<!--"), "{text}");
    }

    #[test]
    fn a_product_home_without_a_presentation_adds_nothing() {
        assert!(product_home_description(&serde_json::json!({})).is_none());
        assert!(product_home_description(&serde_json::json!({
            "data": { "Product": { "sandbox": { "configuration": [] } } }
        }))
        .is_none());
    }

    #[test]
    fn a_steam_search_result_requires_the_exact_title() {
        let value = serde_json::json!({
            "items": [
                { "type": "app", "id": 1300851, "name": "112 Operator - Facilities" },
                { "type": "app", "id": 793460, "name": "112 Operator" }
            ]
        });
        assert_eq!(steam_search_appid(&value, "112 Operator"), Some(793460));
        assert_eq!(steam_search_appid(&value, "112"), None);
    }

    #[test]
    fn steam_news_becomes_announcements() {
        let value = serde_json::json!({
            "appnews": {
                "newsitems": [{
                    "title": "Patch 1.2",
                    "contents": "<p>Corps de l'annonce.</p>",
                    "date": 123,
                    "url": "https://example.test/news"
                }]
            }
        });
        let news = steam_news_items(&value);
        assert_eq!(news.len(), 1);
        assert_eq!(news[0].title, "Patch 1.2");
        assert!(news[0].excerpt.contains("Corps de l'annonce"));
        assert_eq!(news[0].date, 123);
    }

    #[test]
    fn steam_trailer_keeps_hls_and_its_poster() {
        let value = serde_json::json!({
            "793460": {
                "success": true,
                "data": {
                    "movies": [{
                        "hls_h264": "https://cdn.example/master.m3u8",
                        "thumbnail": "https://cdn.example/poster.jpg"
                    }]
                }
            }
        });
        let (trailer, poster) =
            steam_trailer_from_value(&value, 793460).expect("bande-annonce lue");
        assert_eq!(trailer, "https://cdn.example/master.m3u8");
        assert_eq!(poster, "https://cdn.example/poster.jpg");
    }

    #[test]
    fn a_steam_sheet_carries_the_presentation_and_gallery() {
        let value = serde_json::json!({
            "42": {
                "success": true,
                "data": {
                    "short_description": "Une accroche courte.",
                    "detailed_description": "<p>La présentation longue.</p>",
                    "screenshots": [
                        { "path_full": "https://cdn.example/1.jpg" },
                        { "path_full": "https://cdn.example/2.jpg" }
                    ],
                    "movies": [{
                        "hls_h264": "https://cdn.example/master.m3u8",
                        "thumbnail": "https://cdn.example/poster.jpg"
                    }]
                }
            }
        });
        let sheet = steam_sheet_from_value(&value, 42).expect("fiche Steam");
        assert_eq!(sheet.summary, "Une accroche courte.");
        assert_eq!(sheet.description, "La présentation longue.");
        assert_eq!(sheet.screenshots.len(), 2);
        assert_eq!(sheet.trailer, "https://cdn.example/master.m3u8");
    }
}
