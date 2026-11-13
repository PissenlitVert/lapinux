//! Fiche complète d'un jeu, relue à la demande.
//!
//! Trois sources, tentées séparément — une source muette laisse une note et
//! n'empêche pas les autres d'être lues :
//!
//! - le **protocole** : PICS, par la session CM Lapinux (build public,
//!   branches, dépôts et leur poids, classifications, sauvegardes cloud, noms
//!   des contenus additionnels), puis le compte (temps de jeu total, deux
//!   semaines, par système, dernière partie, niveau, badge du jeu, succès et
//!   leur rareté mondiale) ;
//! - la **boutique** : la page publique `appdetails` (description, captures,
//!   exigences, prix, support), les avis de la communauté et le nombre de
//!   joueurs connectés — deux adresses publiques, sans clé ni compte.
//!
//! Aucune clé Web API n'est demandée à l'utilisateur : côté compte, tout passe
//! par la session enregistrée, et les valeurs qu'elle ne publie pas sont
//! simplement absentes.

use crate::native;
use crate::Error;
use crate::Result;
use lapinux_core::model::{
    DescriptionBlock, SteamAchievements, SteamBranch, SteamDepot, SteamDlcRef, SteamFeature,
    SteamGameInfo, SteamNews, SteamRarity, SteamStorePage, SteamUpdateInfo,
};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::time::Duration;
use steamroom::apps::{AccessToken, AppInfo};
use steamroom::client::{LoggedIn, SteamClient};
use steamroom::depot::AppId;
use steamroom::types::key_value::{KeyValue, KvValue};

/// Adresse publique du compteur de joueurs connectés.
const WEB_API: &str = "https://api.steampowered.com";
/// Page publique d'une application. Ni clé ni compte, et en français.
const STORE_APP: &str = "https://store.steampowered.com/api/appdetails?appids=";
/// Annonces publiques d'une application. Le flux `GetNewsForApp/v2` mêle les
/// articles de presse (flux externe, toutes langues) aux publications de
/// l'éditeur : seules ces dernières sont retenues, sous le flux
/// `steam_community_announcements`.
const NEWS_APP: &str = "https://api.steampowered.com/ISteamNews/GetNewsForApp/v2/?appid=";
/// Annonces demandées : le flux dédié de l'éditeur en renvoie jusqu'à 100.
const NEWS_SCAN: usize = 100;
/// Annonces gardées : la fiche en montre trois, le lecteur les lit toutes.
const NEWS_KEPT: usize = NEWS_SCAN;
/// Caractères d'une annonce lus pour son aperçu. La carte en montre ce qui
/// tient dans sa largeur, la coupe fine se fait à l'écran.
const NEWS_EXCERPT: usize = 240;
/// Délai d'une page d'annonce, le temps d'en tirer son visuel.
const IMAGE_TIMEOUT: Duration = Duration::from_secs(6);
/// Délai maximal d'une requête HTTP de la fiche.
const HTTP_TIMEOUT: Duration = Duration::from_secs(12);
/// Succès dont la rareté est retenue, les plus rares d'abord.
const RAREST: usize = 10;
/// Nombre de contenus additionnels dont on demande le nom en une fois.
const DLC_LIMIT: usize = 40;
/// Nombre de captures d'écran conservées.
const SCREENSHOTS: usize = 12;
/// Visuels retenus dans la description : la boutique en intercale parfois une
/// dizaine, la fiche en montre jusqu'à douze.
const DESCRIPTION_IMAGES: usize = 12;

/// Retire les images BBCode du texte : leurs adresses sont rendues à part,
/// avec les autres visuels de la description.
fn strip_image_tags(html: &str) -> String {
    let mut text = String::new();
    let mut rest = html;
    while let Some(index) = rest.find("[img]") {
        text.push_str(&rest[..index]);
        let after = &rest[index + 5..];
        match after.find("[/img]") {
            Some(end) => rest = &after[end + 6..],
            None => {
                rest = "";
            }
        }
    }
    text.push_str(rest);
    text
}

/// Le fichier est-il un média que la fiche sait lire ?
///
/// Les images AVIF et les petites vidéos WebM/MP4 de la description passent
/// par ffmpeg du côté de l'interface ; les formats classiques sont décodés
/// directement.
fn supported_media(url: &str) -> bool {
    let path = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .to_ascii_lowercase();
    [
        ".png", ".jpg", ".jpeg", ".webp", ".avif", ".gif", ".webm", ".mp4",
    ]
    .iter()
    .any(|extension| path.ends_with(extension))
}

/// Complète une adresse relative publiée par Steam.
fn absolute_media_url(url: &str) -> String {
    let url = url.trim();
    if let Some(path) = url.strip_prefix("{STEAM_CLAN_IMAGE}") {
        format!("https://clan.cloudflare.steamstatic.com/images{path}")
    } else {
        url.to_string()
    }
}

/// Valeur d'un attribut `name="…"` dans un fragment HTML.
fn attribute<'a>(html: &'a str, name: &str) -> Option<&'a str> {
    // L'espace évite de confondre `src` avec le `src` de `data-big-src`.
    let marker = format!(" {name}=\"");
    let start = html.find(&marker)? + marker.len();
    let end = html[start..].find('"')?;
    Some(&html[start..start + end])
}

/// Les blocs `<video>` et l'adresse à retenir : la première source WebM/MP4,
/// et l'affiche seulement quand aucune source n'est publiée.
fn video_blocks(html: &str) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    let mut rest = html;
    let mut base = 0;
    while let Some(open) = rest.find("<video") {
        let start = base + open;
        let after = &html[start..];
        let Some(close) = after.find("</video>") else {
            break;
        };
        let end = start + close + "</video>".len();
        let block = &html[start..end];
        let source = block
            .find("<source")
            .and_then(|at| attribute(&block[at..], "src"))
            .or_else(|| attribute(block, "src"));
        let media = source.or_else(|| attribute(block, "poster"));
        if let Some(media) = media {
            out.push((start, end, media.to_string()));
        }
        base = end;
        rest = &html[base..];
    }
    out
}

/// Blocs `<img>` et BBCode `[img]` de la description, avec leur étendue dans
/// le document pour pouvoir remettre le texte autour dans l'ordre.
fn image_blocks(html: &str) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    let mut rest = html;
    let mut base = 0;
    while let Some(open) = rest.find("<img") {
        let start = base + open;
        let Some(close) = html[start..].find('>') else {
            break;
        };
        let end = start + close + 1;
        let tag = &html[start..end];
        if let Some(src) = attribute(tag, "src") {
            out.push((start, end, src.to_string()));
        }
        base = end;
        rest = &html[base..];
    }

    let mut rest = html;
    let mut base = 0;
    while let Some(open) = rest.find("[img]") {
        let start = base + open;
        let content = start + 5;
        let after = &html[content..];
        let Some(close) = after.find("[/img]") else {
            break;
        };
        let end = content + close + 6;
        out.push((start, end, after[..close].to_string()));
        base = end;
        rest = &html[base..];
    }
    out
}

/// Médias que la boutique place **dans** le texte de description, avec leurs
/// positions. Les `<img>`, les `<video>` (source WebM/MP4, ou affiche AVIF en
/// repli) et le BBCode `[img]…[/img]` sont réunis dans l'ordre du texte. Les
/// adresses relatives de Steam sont complétées et les doublons retirés.
fn description_media(html: &str) -> Vec<(usize, usize, String)> {
    let mut found = image_blocks(html);
    found.extend(video_blocks(html));
    found.sort_by_key(|(start, _, _)| *start);
    let mut media = Vec::new();
    for (start, end, url) in found {
        if let Some((_, previous_end, _)) = media.last() {
            if start < *previous_end {
                continue;
            }
        }
        let url = absolute_media_url(&url);
        if (url.starts_with("https://") || url.starts_with("http://"))
            && supported_media(&url)
            && !media.iter().any(|(_, _, seen)| seen == &url)
            && media.len() < DESCRIPTION_IMAGES
        {
            media.push((start, end, url));
        }
    }
    media
}

/// Médias de description, dans l'ordre du texte. La galerie et les blocs de
/// lecture partagent cette liste.
fn description_images(html: &str) -> Vec<String> {
    description_media(html)
        .into_iter()
        .map(|(_, _, url)| url)
        .collect()
}

/// Les titres que Steam donne à la présentation du jeu elle-même, par opposition
/// aux nouvelles, mises à jour et contenus additionnels qui la précèdent souvent.
const ABOUT_MARKERS: [&str; 3] = ["à propos du jeu", "about this game", "about the game"];

/// Sépare le texte avant le titre « À propos » de celui qui le suit, sans
/// garder le titre lui-même : la fiche affiche déjà son propre titre « À propos ».
fn split_about_section(text: &str) -> Option<(String, String)> {
    let mut before = Vec::new();
    let mut after = Vec::new();
    let mut found = false;
    for line in text.lines() {
        let heading = line.trim().to_lowercase();
        if !found
            && ABOUT_MARKERS
                .iter()
                .any(|marker| heading.starts_with(marker))
        {
            found = true;
            continue;
        }
        if found {
            after.push(line);
        } else {
            before.push(line);
        }
    }
    found.then(|| {
        (
            before.join("\n").trim().to_string(),
            after.join("\n").trim().to_string(),
        )
    })
}

/// La présentation du jeu passe avant les nouvelles et les contenus additionnels
/// que Steam publie au début de la description. Sans ce déplacement, « À propos »
/// arrive sous l'explication des DLC, alors qu'il décrit le jeu lui-même.
fn about_first(mut blocks: Vec<DescriptionBlock>) -> Vec<DescriptionBlock> {
    let Some((index, before, after)) = blocks.iter().enumerate().find_map(|(index, block)| {
        let DescriptionBlock::Text(text) = block else {
            return None;
        };
        split_about_section(text).map(|(before, after)| (index, before, after))
    }) else {
        return blocks;
    };
    let mut tail = blocks.split_off(index);
    tail.remove(0);
    let mut reordered = Vec::with_capacity(blocks.len() + tail.len() + 2);
    if !after.is_empty() {
        reordered.push(DescriptionBlock::Text(after));
    }
    reordered.extend(tail);
    let mut pre = blocks;
    if !before.is_empty() {
        pre.push(DescriptionBlock::Text(before));
    }
    reordered.extend(pre);
    reordered
}

/// Découpe la description aux endroits où un média est publié, pour que la
/// fiche peigne une image entre les paragraphes qui l'entourent au lieu de
/// reléguer tous les visuels à la fin.
fn description_blocks(html: &str) -> Vec<DescriptionBlock> {
    let media = description_media(html);
    let mut blocks = Vec::new();
    let mut cursor = 0;
    for (start, end, url) in media {
        let text = html_text(&strip_image_tags(&html[cursor..start]));
        if !text.is_empty() {
            blocks.push(DescriptionBlock::Text(text));
        }
        blocks.push(DescriptionBlock::Media(url));
        cursor = end;
    }
    let text = html_text(&strip_image_tags(&html[cursor..]));
    if !text.is_empty() {
        blocks.push(DescriptionBlock::Text(text));
    }
    about_first(blocks)
}

/// Session Steam nécessaire à la lecture d'une fiche. Aucune clé Web API :
/// le jeton de la session désigne déjà le compte.
#[derive(Debug, Clone, Default)]
pub struct Sources {
    pub account: String,
    pub refresh_token: String,
    pub steam_id: u64,
    /// Nom Steam de la langue demandée pour la boutique et les annonces
    /// (`french`, `english`…). Vide : le français, la langue de l'interface.
    pub language: String,
}

impl Sources {
    /// Une session Steam est-elle utilisable ?
    pub fn has_session(&self) -> bool {
        !self.refresh_token.trim().is_empty()
    }
}

/// Lit tout ce que Steam publie sur `appid`.
///
/// Ne rend jamais d'erreur : chaque source échoue séparément, la fiche porte
/// les sources lues et une note par source muette. `installed_build` est le
/// build du manifeste installé, pour le comparer au build public.
pub fn game_info(appid: u32, sources: &Sources, installed_build: Option<u64>) -> SteamGameInfo {
    let mut info = SteamGameInfo {
        appid,
        fetched: lapinux_core::util::now_unix(),
        ..Default::default()
    };
    info.update.installed_build = installed_build;
    // Le moteur commun porte la session : la garder sur un runtime jetable la
    // tuait avec lui, et la fiche suivante rouvrait un logon.
    crate::shared::block_on(collect(&mut info, sources));
    info
}

fn http_client() -> Option<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .user_agent(format!("Lapinux/{}", lapinux_core::APP_NAME))
        .build()
        .ok()
}

async fn collect(info: &mut SteamGameInfo, sources: &Sources) {
    // Chaque source est chronométrée : une fiche qui met dix secondes doit
    // dire laquelle les prend (journal au niveau debug).
    let mut mark = std::time::Instant::now();
    macro_rules! lap {
        ($name:expr) => {
            tracing::debug!(
                "fiche {} : {} en {} ms",
                info.appid,
                $name,
                mark.elapsed().as_millis()
            );
            mark = std::time::Instant::now();
        };
    }
    let http = http_client();
    // Les lectures publiques (joueurs, boutique, annonces) ne dépendent pas du
    // protocole : elles partent tout de suite sur leur propre tâche et
    // s'appliquent à la fin. En séquence, elles ajoutaient deux à sept
    // secondes à chaque fiche.
    let public_task = http.as_ref().map(|http| {
        let http = http.clone();
        let appid = info.appid;
        let language = store_language(&sources.language).to_string();
        tokio::spawn(async move {
            let players = players_now(&http, appid).await;
            let store = store_block(&http, appid, &language).await;
            (players, store)
        })
    });
    // 1. Protocole : PICS, puis le compte, sur une seule session.
    let mut client = None;
    if sources.has_session() {
        // Session partagée par toutes les lectures : ouvrir la fiche ne doit
        // pas coûter une connexion de plus (Steam n'en garde qu'une).
        match crate::shared::session(&sources.account, &sources.refresh_token).await {
            Ok(logged) => {
                lap!("session");
                match pics_app(&logged, info.appid).await {
                    Ok(root) => {
                        lap!("PICS");
                        // Un jeu prêté par la famille arrive sans paquet décrit :
                        // le protocole tranche alors les dépôts des autres
                        // éditions, pour que le poids du bloc Détails Steam soit
                        // celui du bouton de téléchargement.
                        let mut budget = native::DEPOT_KEY_BUDGET;
                        let refused =
                            native::refused_depots(&logged, info.appid, &root, &mut budget).await;
                        lap!("dépôts refusés");
                        info.update = merge_installed(
                            &info.update,
                            update_from_pics(&root, refused.as_ref(), native::default_language()),
                        );
                        let dlc: Vec<u32> = info
                            .update
                            .dlc
                            .iter()
                            .map(|content| content.appid)
                            .take(DLC_LIMIT)
                            .collect();
                        if !dlc.is_empty() {
                            // Nom et poids de chaque contenu, en une seule
                            // demande PICS : sans elle la liste affichait des
                            // appids nus et un « poids inconnu ».
                            let details = pics_dlc(&logged, &dlc).await;
                            lap!("contenus PICS");
                            for content in &mut info.update.dlc {
                                if let Some(found) = details.get(&content.appid) {
                                    content.name = found.name.clone();
                                    content.image = found.image.clone();
                                    // Le poids du jeu parent fait foi ; celui
                                    // de la fiche du contenu ne sert que si
                                    // aucun dépôt n'a été trouvé.
                                    if content.download == 0 && content.install == 0 {
                                        content.download = found.download;
                                        content.install = found.install;
                                    }
                                }
                            }
                        }
                        info.sources.push("protocole".into());
                    }
                    Err(error) => info.notes.push(format!("protocole : {error}")),
                }
                client = Some(logged);
            }
            Err(error) => info.notes.push(format!("session Steam : {error}")),
        }
    } else {
        info.notes
            .push("compte Steam non connecté : le protocole n'a pas été lu".into());
    }

    // 2. Compte : le temps de jeu, le badge, les succès et leur rareté, tous
    //    lus par la session — aucune clé à demander.
    if let Some(client) = &client {
        protocol_account(info, client, sources.steam_id).await;
        lap!("compte et succès");
    }

    // 3. Public : joueurs connectés, page de la boutique et annonces. Ni clé
    //    ni compte, ces lectures restent donc valables même déconnecté.
    match public_task {
        Some(task) => match task.await {
            Ok((players, store)) => {
                if let Some(players) = players {
                    info.account.players_now = Some(players);
                    account_source(info);
                }
                match store {
                    Some(page) => {
                        info.store = page;
                        info.sources.push("boutique".into());
                    }
                    None => info.notes.push("boutique : page indisponible".into()),
                }
                lap!("lectures publiques");
            }
            Err(_) => info.notes.push("lectures publiques interrompues".into()),
        },
        None => info.notes.push("moteur HTTP indisponible".into()),
    }
    // Dernière lecture du chronomètre : elle évite un avertissement sur la
    // dernière affectation, inutilisée par construction.
    let _ = mark.elapsed();
    info.notes.dedup();
}

/// Requête Web API : la réponse JSON, ou `None` (Steam répond parfois vide).
async fn web(http: &reqwest::Client, url: &str) -> Option<Value> {
    let response = http.get(url).send().await.ok()?;
    if !response.status().is_success() {
        tracing::debug!("Web API : HTTP {}", response.status());
        return None;
    }
    response.json::<Value>().await.ok()
}

// ---------------------------------------------------------------------------
// Protocole : PICS
// ---------------------------------------------------------------------------

/// Racine PICS d'une application, telle que le CM la rend.
async fn pics_app(client: &SteamClient<LoggedIn>, appid: u32) -> Result<KeyValue> {
    let app_id = AppId(appid);
    let tokens = client
        .pics_get_access_tokens(&[app_id])
        .await
        .map_err(|error| Error::Other(format!("jeton PICS : {error}")))?;
    let token = tokens
        .iter()
        .find(|token| token.app_id == app_id)
        .map(|token| token.token)
        .unwrap_or(0);
    let infos = client
        .pics_get_product_info(&[AccessToken { app_id, token }])
        .await
        .map_err(|error| Error::Other(format!("PICS : {error}")))?;
    let info: AppInfo = infos
        .into_iter()
        .next()
        .ok_or_else(|| Error::Other(format!("PICS ne connaît pas l'application {appid}")))?;
    let bytes = info
        .kv_data
        .ok_or_else(|| Error::Other(format!("PICS sans données pour {appid}")))?;
    parse_kv(&bytes).ok_or_else(|| Error::Other(format!("réponse PICS illisible pour {appid}")))
}

/// Chemin d'une bannière dans les visuels de la boutique.
///
/// PICS publie `common.header_image.<langue>` sous la forme `<hash>/header.jpg`
/// pour les visuels récents ; les anciens n'ont qu'un `header.jpg` à plat, et
/// la réponse est vide.
fn store_image(common: Option<&KeyValue>) -> String {
    let Some(image) = common.and_then(|common| common.get("header_image")) else {
        return String::new();
    };
    if let KvValue::Children(languages) = &image.value {
        for language in ["english", "french"] {
            if let Some(text) = languages.get(language).and_then(KeyValue::as_str) {
                return text.to_string();
            }
        }
        return languages
            .values()
            .find_map(KeyValue::as_str)
            .unwrap_or_default()
            .to_string();
    }
    text_of(Some(image))
}

async fn pics_dlc(client: &SteamClient<LoggedIn>, appids: &[u32]) -> HashMap<u32, SteamDlcRef> {
    let mut found = HashMap::new();
    if appids.is_empty() {
        return found;
    }
    let ids: Vec<AppId> = appids.iter().map(|appid| AppId(*appid)).collect();
    let Ok(tokens) = client.pics_get_access_tokens(&ids).await else {
        return found;
    };
    let requests: Vec<AccessToken> = appids
        .iter()
        .map(|appid| AccessToken {
            app_id: AppId(*appid),
            token: tokens
                .iter()
                .find(|token| token.app_id == AppId(*appid))
                .map(|token| token.token)
                .unwrap_or(0),
        })
        .collect();
    let Ok(infos) = client.pics_get_product_info(&requests).await else {
        return found;
    };
    for info in infos {
        let (Some(app_id), Some(bytes)) = (info.app_id, info.kv_data) else {
            continue;
        };
        let Some(root) = parse_kv(&bytes) else {
            continue;
        };
        let common = app_section(&root).get("common");
        let name = text_of(common.and_then(|common| common.get("name")));
        let size = crate::native::dlc_size(&root);
        found.insert(
            app_id.0,
            SteamDlcRef {
                appid: app_id.0,
                name,
                image: store_image(common),
                owned: false,
                // Repli seulement : la plupart des contenus ne décrivent pas
                // leurs dépôts ici, ils appartiennent au jeu parent (voir
                // `dlc_sizes`). Quelques contenus autonomes font exception.
                download: size.map(|size| size.download).unwrap_or(0),
                install: size.map(|size| size.install).unwrap_or(0),
            },
        );
    }
    found
}

fn parse_kv(bytes: &[u8]) -> Option<KeyValue> {
    if bytes.first() == Some(&0) {
        KeyValue::from_binary(bytes).ok()
    } else {
        KeyValue::from_text(&String::from_utf8_lossy(bytes)).ok()
    }
}

/// Le protocole remplace toute la section « mise à jour » : le build installé,
/// lu sur le disque et posé avant la lecture, doit y survivre — c'est lui que
/// la fiche compare au build public pour dire si une mise à jour attend.
fn merge_installed(previous: &SteamUpdateInfo, mut fresh: SteamUpdateInfo) -> SteamUpdateInfo {
    fresh.installed_build = previous.installed_build;
    fresh.installed_build_time = previous.installed_build_time;
    fresh
}

/// Ce que le protocole publie du build : branches, dépôts, classifications.
pub fn update_from_pics(
    root: &KeyValue,
    refused: Option<&HashSet<u32>>,
    language: Option<&str>,
) -> SteamUpdateInfo {
    let app = app_section(root);
    let common = app.get("common");
    // PICS écrit ces deux champs tantôt en minuscules, tantôt avec une
    // majuscule (`game`, `Game`) : la fiche les compare à des libellés
    // canoniques, l'écriture est donc ramenée à une seule forme.
    let mut update = SteamUpdateInfo {
        app_type: text_of(common.and_then(|common| common.get("type"))).to_ascii_lowercase(),
        release_state: text_of(common.and_then(|common| common.get("release_state")))
            .to_ascii_lowercase(),
        ..Default::default()
    };
    if let Some(ufs) = app.get("ufs") {
        update.cloud_quota = number_of(ufs.get("quota")).filter(|quota| *quota > 0);
        update.cloud_files = number_of(ufs.get("maxnumfiles"))
            .and_then(|files| u32::try_from(files).ok())
            .filter(|files| *files > 0);
    }
    update.ratings = ratings_of(common);
    if let Some(depots) = app.get("depots") {
        if let Some(branches) = depots.get("branches") {
            if let KvValue::Children(entries) = &branches.value {
                for (name, branch) in entries {
                    let build = number_of(branch.get("buildid"));
                    let time = number_of(branch.get("timebuildupdated"))
                        .filter(|time| *time > 0)
                        .map(|time| time as i64);
                    if name == "public" {
                        update.public_build = build;
                        update.public_build_time = time;
                    }
                    update.branches.push(SteamBranch {
                        name: name.clone(),
                        build,
                        time,
                        description: text_of(branch.get("description")),
                    });
                }
            }
        }
        update.depots = depot_list(depots);
    }
    let (download, install) = platform_size(root, "linux", refused, language);
    update.download = download;
    update.install = install;
    // Le poids de chaque contenu vient des dépôts du jeu qui portent son
    // `dlcappid` : la fiche d'un contenu ne les publie pas.
    let sizes = crate::native::dlc_sizes(app);
    update.dlc = dlc_ids(app)
        .into_iter()
        .map(|appid| {
            let size = sizes.get(&appid).copied().unwrap_or_default();
            SteamDlcRef {
                appid,
                download: size.download,
                install: size.install,
                ..Default::default()
            }
        })
        .collect();
    update
}

/// Dépôts publiés, du plus petit identifiant au plus grand.
fn depot_list(depots: &KeyValue) -> Vec<SteamDepot> {
    let KvValue::Children(entries) = &depots.value else {
        return Vec::new();
    };
    let mut list: Vec<SteamDepot> = entries
        .iter()
        .filter_map(|(key, depot)| {
            let id: u32 = key.parse().ok()?;
            let config = depot.get("config");
            let mut os: Vec<String> = config
                .and_then(|config| config.get("oslist"))
                .and_then(KeyValue::as_str)
                .map(|list| {
                    list.split(',')
                        .filter_map(|system| {
                            Some(match system.trim().to_ascii_lowercase().as_str() {
                                "windows" => "Windows".to_string(),
                                "macos" => "macOS".to_string(),
                                "linux" => "Linux".to_string(),
                                _ => return None,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            os.sort();
            os.dedup();
            let manifest = depot.get("manifests").and_then(|all| all.get("public"));
            Some(SteamDepot {
                id,
                os,
                language: config
                    .and_then(|config| config.get("language"))
                    .and_then(KeyValue::as_str)
                    .map(language_label)
                    .unwrap_or_default(),
                download: number_of(manifest.and_then(|manifest| manifest.get("download")))
                    .unwrap_or(0),
                install: number_of(manifest.and_then(|manifest| manifest.get("size"))).unwrap_or(0),
                shared: depot.get("sharedinstall").is_some(),
                dlc_appid: number_of(depot.get("dlcappid")).and_then(|id| u32::try_from(id).ok()),
            })
        })
        .collect();
    list.sort_by_key(|depot| depot.id);
    list
}

/// Somme des manifestes publics des dépôts d'une plateforme : ce qu'un
/// téléchargement pèse, et ce qu'il occupe une fois installé.
///
/// Les dépôts partagés et ceux d'un contenu additionnel sont écartés, et un
/// seul doublage compte (l'anglais quand il existe) : sans quoi un jeu vendu en
/// plusieurs éditions régionales annoncerait plusieurs fois le même poids.
pub fn platform_size(
    root: &KeyValue,
    platform: &str,
    refused: Option<&HashSet<u32>>,
    language: Option<&str>,
) -> (u64, u64) {
    let app = app_section(root);
    let Some(depots) = app.get("depots") else {
        return (0, 0);
    };
    let KvValue::Children(entries) = &depots.value else {
        return (0, 0);
    };
    struct Candidate<'a> {
        language: Option<&'a str>,
        install: u64,
        download: u64,
    }
    let mut candidates: Vec<Candidate> = Vec::new();
    for (key, depot) in entries {
        if key.parse::<u32>().is_err() {
            continue;
        }
        if depot.get("sharedinstall").is_some() || depot.get("dlcappid").is_some() {
            continue;
        }
        // Dépôt qu'une autre édition publie : le protocole en a refusé la clé.
        if refused.is_some_and(|refused| refused.contains(&key.parse::<u32>().unwrap_or_default()))
        {
            continue;
        }
        if let Some(oslist) = depot
            .get("config")
            .and_then(|config| config.get("oslist"))
            .and_then(KeyValue::as_str)
        {
            let wanted = oslist
                .split(',')
                .map(str::trim)
                .any(|system| system.eq_ignore_ascii_case(platform));
            if !wanted {
                continue;
            }
        }
        let Some(manifest) = depot.get("manifests").and_then(|all| all.get("public")) else {
            continue;
        };
        let install = number_of(manifest.get("size")).unwrap_or(0);
        let download = number_of(manifest.get("download")).unwrap_or(0);
        if install == 0 && download == 0 {
            continue;
        }
        candidates.push(Candidate {
            language: depot
                .get("config")
                .and_then(|config| config.get("language"))
                .and_then(KeyValue::as_str),
            install,
            download,
        });
    }
    // Doublage retenu : celui que Steam prendrait — la langue du client quand
    // le jeu la publie, sinon l'anglais, sinon la première par ordre
    // alphabétique.
    let available: Vec<&str> = candidates
        .iter()
        .filter_map(|candidate| candidate.language)
        .collect();
    let preferred = language
        .filter(|language| {
            available
                .iter()
                .any(|value| value.eq_ignore_ascii_case(language))
        })
        .or_else(|| {
            available
                .iter()
                .copied()
                .find(|value| value.eq_ignore_ascii_case("english"))
        })
        .or_else(|| available.iter().copied().min());
    let mut install = 0;
    let mut download = 0;
    for candidate in candidates {
        if let Some(language) = candidate.language {
            if Some(language) != preferred {
                continue;
            }
        }
        install += candidate.install;
        download += candidate.download;
    }
    (download, install)
}

/// Contenus additionnels publiés par le jeu (`extended.listofdlc`).
fn dlc_ids(app: &KeyValue) -> Vec<u32> {
    text_of(
        app.get("extended")
            .and_then(|extended| extended.get("listofdlc")),
    )
    .split(',')
    .filter_map(|id| id.trim().parse().ok())
    .collect()
}

/// Classifications officielles publiées par PICS (PEGI, ESRB, USK…).
fn ratings_of(common: Option<&KeyValue>) -> Vec<String> {
    let Some(ratings) = common.and_then(|common| common.get("ratings")) else {
        return Vec::new();
    };
    let KvValue::Children(entries) = &ratings.value else {
        return Vec::new();
    };
    let mut list = Vec::new();
    for (board, entry) in entries {
        let rating = text_of(entry.get("rating"));
        if rating.is_empty() {
            continue;
        }
        let label = match board.as_str() {
            "pegi" => "PEGI".to_string(),
            "esrb" => "ESRB".to_string(),
            "usk" => "USK".to_string(),
            "cero" => "CERO".to_string(),
            "oflc" => "OFLC".to_string(),
            "acb" => "ACB".to_string(),
            "russia" => "RARS".to_string(),
            other => other.to_uppercase(),
        };
        let value = match board.as_str() {
            "esrb" => match rating.to_ascii_lowercase().as_str() {
                "e" => "Tous publics".to_string(),
                "e10" => "10 ans et plus".to_string(),
                "t" => "Adolescents".to_string(),
                "m" => "Matures 17 ans et plus".to_string(),
                "ao" => "Adultes seulement".to_string(),
                other => other.to_uppercase(),
            },
            _ => rating,
        };
        list.push(format!("{label} {value}"));
    }
    list.sort();
    list.dedup();
    list
}

fn language_label(code: &str) -> String {
    match code.trim().to_ascii_lowercase().as_str() {
        "english" => "Anglais",
        "french" => "Français",
        "german" => "Allemand",
        "spanish" => "Espagnol",
        "latam" => "Espagnol (Amérique latine)",
        "italian" => "Italien",
        "portuguese" => "Portugais",
        "brazilian" => "Portugais (Brésil)",
        "russian" => "Russe",
        "japanese" => "Japonais",
        "koreana" => "Coréen",
        "schinese" => "Chinois simplifié",
        "tchinese" => "Chinois traditionnel",
        "polish" => "Polonais",
        "dutch" => "Néerlandais",
        "danish" => "Danois",
        "swedish" => "Suédois",
        "turkish" => "Turc",
        "ukrainian" => "Ukrainien",
        "czech" => "Tchèque",
        "hungarian" => "Hongrois",
        "romanian" => "Roumain",
        "thai" => "Thaï",
        "vietnamese" => "Vietnamien",
        "arabic" => "Arabe",
        other => other,
    }
    .to_string()
}

/// Reporte sur la fiche le temps de jeu lu par le protocole.
fn apply_playtime(info: &mut SteamGameInfo, play: &crate::playtime::Playtime) {
    info.account.owned = !play.family_shared;
    info.account.minutes_total = Some(play.minutes_total);
    info.account.minutes_2weeks = Some(play.minutes_2weeks).filter(|minutes| *minutes > 0);
    info.account.minutes_windows = Some(play.minutes_windows).filter(|minutes| *minutes > 0);
    info.account.minutes_linux = Some(play.minutes_linux).filter(|minutes| *minutes > 0);
    info.account.minutes_macos = Some(play.minutes_mac).filter(|minutes| *minutes > 0);
    info.account.minutes_deck = Some(play.minutes_deck).filter(|minutes| *minutes > 0);
    info.account.last_played = play.last_played;
}

fn app_section(root: &KeyValue) -> &KeyValue {
    root.get("appinfo").unwrap_or(root)
}

fn text_of(value: Option<&KeyValue>) -> String {
    value
        .and_then(KeyValue::as_str)
        .unwrap_or_default()
        .to_string()
}

fn number_of(value: Option<&KeyValue>) -> Option<u64> {
    match value.map(|value| &value.value) {
        Some(KvValue::String(value)) => value.parse().ok(),
        Some(KvValue::UInt64(value)) => Some(*value),
        Some(KvValue::Int64(value)) => u64::try_from(*value).ok(),
        Some(KvValue::Int32(value)) => u64::try_from(*value).ok(),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Joueurs connectés : adresse publique
// ---------------------------------------------------------------------------

/// Joueurs connectés au jeu, lus sur l'adresse publique de Steam.
///
/// C'est la seule valeur que la fiche demande encore en HTTP : aucun message
/// du protocole ne la publie, et cette méthode ne veut ni clé ni jeton — elle
/// répond pour n'importe quel jeu, compte connecté ou non.
async fn players_now(http: &reqwest::Client, appid: u32) -> Option<u64> {
    let url = format!("{WEB_API}/ISteamUserStats/GetNumberOfCurrentPlayers/v1/?appid={appid}");
    let value = web(http, &url).await?;
    json_u64(value.pointer("/response/player_count"))
}

/// Compte lu par le protocole : la bibliothèque donne le temps de jeu, le
/// service de badge le niveau, et le schéma des succès leur progression et
/// leur rareté. Tout passe par la session, sans clé Web API.
async fn protocol_account(info: &mut SteamGameInfo, client: &SteamClient<LoggedIn>, steam_id: u64) {
    // Le groupe familial ouvre le temps de jeu des jeux prêtés ; sans lui, la
    // fiche d'un jeu de la famille n'en montrait aucun.
    let family = crate::playtime::family_group_id(client, steam_id).await;
    match crate::playtime::owned_games(client, steam_id, family).await {
        Ok(games) => {
            let found = games
                .into_iter()
                .find(|(appid, _)| *appid == info.appid)
                .map(|(_, play)| play);
            // La bibliothèque entière omet parfois un jeu : la réponse peut
            // être tronquée sur un gros compte, et un jeu activé à l'instant
            // n'y est pas encore. Le même appel, filtré sur l'appid, le rend
            // seul — c'est lui qui remplissait les fiches vides.
            let found = match found {
                Some(play) => Some(play),
                None => crate::playtime::game_playtime(client, info.appid, steam_id, family)
                    .await
                    .unwrap_or(None),
            };
            match found {
                Some(play) => {
                    apply_playtime(info, &play);
                    account_source(info);
                }
                None => info
                    .notes
                    .push("compte : ce jeu ne figure pas dans la bibliothèque Steam".into()),
            }
        }
        Err(error) => info
            .notes
            .push(format!("compte : temps de jeu par le protocole — {error}")),
    }

    match crate::playtime::badge_levels(client, info.appid).await {
        Ok(levels) => {
            info.account.level = levels.player_level;
            info.account.badge_level = levels.badge_level;
        }
        Err(error) => info
            .notes
            .push(format!("compte : badge par le protocole — {error}")),
    }

    let read = crate::achievements::read_with(client, info.appid, steam_id).await;
    if read.items.is_empty() {
        // « Pas de succès » n'est pas une panne : seule une vraie erreur se dit.
        for note in &read.notes {
            if !note.contains("ne publie pas de succès") {
                info.notes.push(format!("succès : {note}"));
            }
        }
        return;
    }
    info.account.achievements_total = u32::try_from(read.items.len()).ok();
    info.account.achievements_unlocked = u32::try_from(read.unlocked()).ok();
    if let Some(last) = read
        .items
        .iter()
        .filter(|item| item.achieved)
        .max_by_key(|item| item.unlock_time.unwrap_or(0))
    {
        info.account.last_achievement = last.name.clone();
        info.account.last_achievement_time = last.unlock_time;
    }
    info.account.rarest = rarest(&read);
    account_source(info);

    match crate::stats::game_stats(client, info.appid, steam_id).await {
        Ok(stats) if !stats.is_empty() => {
            info.stats = stats;
            account_source(info);
        }
        Ok(_) => {}
        Err(error) => info.notes.push(format!("statistiques publiées : {error}")),
    }
}

/// Marque la source « compte » une seule fois, que la lecture vienne du
/// protocole ou de la Web API.
fn account_source(info: &mut SteamGameInfo) {
    if !info.sources.iter().any(|source| source == "compte") {
        info.sources.push("compte".into());
    }
}

/// Succès les plus rares d'un jeu, les moins obtenus d'abord, avec la part de
/// joueurs qui les possèdent et s'ils sont débloqués sur ce compte. La rareté
/// vient du schéma du protocole (`player_percent_unlocked`).
fn rarest(read: &SteamAchievements) -> Vec<SteamRarity> {
    let mut rarest: Vec<SteamRarity> = read
        .items
        .iter()
        .filter_map(|item| {
            item.percent.map(|percent| SteamRarity {
                name: item.name.clone(),
                percent,
                unlocked: item.achieved,
            })
        })
        .collect();
    rarest.sort_by(|left, right| left.percent.total_cmp(&right.percent));
    rarest.truncate(RAREST);
    rarest
}

fn json_u64(value: Option<&Value>) -> Option<u64> {
    match value {
        Some(Value::Number(number)) => number.as_u64(),
        Some(Value::String(text)) => text.parse().ok(),
        _ => None,
    }
}

fn json_str(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

// ---------------------------------------------------------------------------
// Boutique : page publique et avis
// ---------------------------------------------------------------------------

/// Nom Steam de la langue demandée : celui du réglage quand il est connu,
/// le français sinon.
fn store_language(code: &str) -> &'static str {
    let code = code.trim();
    if code.is_empty() {
        return "french";
    }
    lapinux_core::lang::find(code)
        .map(|language| language.code)
        .unwrap_or("french")
}

/// Page publique de la boutique, dans la langue du réglage : Steam renvoie le
/// texte d'origine quand le jeu n'a pas de page traduite.
async fn store_block(http: &reqwest::Client, appid: u32, language: &str) -> Option<SteamStorePage> {
    let url = format!("{STORE_APP}{appid}&l={language}&cc=FR");
    let started = std::time::Instant::now();
    let value = web(http, &url).await?;
    let mut page = store_from_json(&value, appid)?;
    tracing::debug!(
        "fiche {appid} : page de boutique en {} ms",
        started.elapsed().as_millis()
    );
    // Les annonces vivent sur une autre adresse, elle aussi publique. Une
    // source muette ne doit pas emporter la page : la liste reste vide.
    let started = std::time::Instant::now();
    page.news = news_block(http, appid, language).await;
    tracing::debug!(
        "fiche {appid} : annonces en {} ms",
        started.elapsed().as_millis()
    );
    // La page des avis de la boutique redirige désormais vers la vitrine pour
    // un client sans cookie de session : le compte d'avis ne peut plus venir
    // que de `appdetails` (`recommendations.total`).
    Some(page)
}

/// Annonces de l'éditeur : notes de mise à jour et actualités du jeu.
async fn news_block(http: &reqwest::Client, appid: u32, language: &str) -> Vec<SteamNews> {
    let url = format!(
        "{NEWS_APP}{appid}&count={NEWS_SCAN}&feeds=steam_community_announcements&maxlength=0&format=json&l={language}"
    );
    let Some(value) = web(http, &url).await else {
        return Vec::new();
    };
    let mut news = news_from_json(&value);
    // Les annonces récentes portent déjà leur visuel dans le BBCode. Seules
    // celles qui n'en ont pas paient une lecture de page `og:image` : sans
    // cela, quatre-vingt-dix-huit requêtes allongeaient la fiche.
    let mut reads = Vec::new();
    for (index, item) in news.iter().enumerate() {
        if !item.image.is_empty() {
            continue;
        }
        let http = http.clone();
        let url = item.url.clone();
        reads.push((
            index,
            tokio::spawn(async move { announcement_image(&http, &url).await }),
        ));
    }
    for (index, read) in reads {
        if let Ok(Some(image)) = read.await {
            if news[index].image.is_empty() {
                news[index].image = image;
            }
        }
    }
    news
}

/// Visuel d'une annonce : la page communautaire le publie en `og:image`. Une
/// annonce sans page lisible garde la carte sans image, elle reste entière.
async fn announcement_image(http: &reqwest::Client, url: &str) -> Option<String> {
    if !url.starts_with("http") {
        return None;
    }
    let response = tokio::time::timeout(IMAGE_TIMEOUT, http.get(url).send())
        .await
        .ok()?
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let html = tokio::time::timeout(IMAGE_TIMEOUT, response.text())
        .await
        .ok()?
        .ok()?;
    og_image(&html)
}

/// Contenu de la balise `og:image` d'une page, quand elle en a une.
pub fn og_image(html: &str) -> Option<String> {
    let start = html.find("og:image")?;
    // La balise entière tient sur une ligne : chercher `content` dans tout le
    // document attraperait une autre balise.
    let tag = &html[start..html.len().min(start + 400)];
    let content = tag.find("content=\"")? + "content=\"".len();
    let rest = &tag[content..];
    let end = rest.find('"')?;
    let url = rest[..end].trim();
    url.starts_with("https://").then(|| url.to_string())
}

/// Lit les annonces d'une réponse `GetNewsForApp` : le flux garde les
/// publications de l'éditeur, la presse externe est écartée.
pub fn news_from_json(value: &Value) -> Vec<SteamNews> {
    let Some(items) = value
        .pointer("/appnews/newsitems")
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    items
        .iter()
        .filter(|item| json_str(item.get("feedname")) == "steam_community_announcements")
        .filter(|item| !json_str(item.get("title")).trim().is_empty())
        .take(NEWS_KEPT)
        .map(|item| {
            let contents = json_str(item.get("contents"));
            SteamNews {
                title: json_str(item.get("title")).trim().to_string(),
                excerpt: news_excerpt(&contents),
                body: news_body(&contents),
                date: item.get("date").and_then(Value::as_i64).unwrap_or(0),
                author: json_str(item.get("author")),
                url: json_str(item.get("url")),
                image: news_image(&contents).unwrap_or_default(),
            }
        })
        .collect()
}

/// Première image intégrée au BBCode d'une annonce.
///
/// Le flux n'expose pas de champ image, mais les publications récentes
/// contiennent `{STEAM_CLAN_IMAGE}` ou une adresse dans `[img]`. Lire cette
/// adresse évite une requête de page par annonce.
fn news_image(contents: &str) -> Option<String> {
    const CLAN: &str = "{STEAM_CLAN_IMAGE}";
    if let Some(index) = contents.find(CLAN) {
        let path = contents[index + CLAN.len()..].trim_start_matches('/');
        let end = path
            .find(|character: char| {
                character.is_whitespace() || matches!(character, '"' | ']' | ')' | '[')
            })
            .unwrap_or(path.len());
        if end > 0 {
            return Some(format!(
                "https://clan.akamai.steamstatic.com/images/{}",
                &path[..end]
            ));
        }
    }
    let start = contents.find("[img")?;
    let rest = &contents[start..];
    let link = rest.find("http")?;
    let rest = &rest[link..];
    let end = rest
        .find(|character: char| {
            character.is_whitespace() || matches!(character, '"' | ']' | ')' | '[')
        })
        .unwrap_or(rest.len());
    let url = &rest[..end];
    url.starts_with("https://").then(|| url.to_string())
}

/// Rend l'espace que le flux a perdu entre deux phrases.
///
/// Steam retire les blocs HTML des annonces sans laisser d'espace : « Cast
/// your vote now!https://… » et « Light!We´re honoured » arrivent collés. Une
/// majuscule après un point, un point d'exclamation ou d'interrogation rouvre
/// donc un espace.
fn breathe(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut previous = ' ';
    for (index, character) in chars.iter().enumerate() {
        // Une phrase se rouvre sur une majuscule, un lien sur son adresse.
        let link = chars[index..]
            .iter()
            .take(8)
            .collect::<String>()
            .starts_with("http");
        let sentence = character.is_uppercase() && matches!(previous, '.' | '!' | '?');
        if previous != ' ' && (sentence || (link && *character == 'h')) {
            out.push(' ');
        }
        out.push(*character);
        previous = *character;
    }
    out
}

/// Texte lisible d'une annonce : le balisage de Steam (`[img]`, `[url]`,
/// `[b]`) disparaît, `{STEAM_CLAN_IMAGE}` emporte le chemin du visuel qui le
/// suit, et les paragraphes restent séparés.
pub fn news_body(contents: &str) -> String {
    let mut text = String::with_capacity(contents.len());
    let mut closing: Option<char> = None;
    let mut tag = String::new();
    // Le chemin qui suit `{STEAM_CLAN_IMAGE}` n'a de sens qu'avec l'accolade :
    // il part avec elle, jusqu'à l'espace suivant.
    let mut path = false;
    for character in contents.chars() {
        if path {
            if character.is_whitespace() {
                path = false;
                text.push(' ');
            }
            continue;
        }
        match closing {
            Some(end) => {
                if character == end {
                    if end == ']' {
                        // Les blocs BBCode séparent les paragraphes ; les
                        // balises inline (gras, italique, lien) non.
                        let name = tag
                            .trim()
                            .trim_start_matches('/')
                            .split_whitespace()
                            .next()
                            .unwrap_or("");
                        if matches!(
                            name,
                            "p" | "h1"
                                | "h2"
                                | "h3"
                                | "h4"
                                | "h5"
                                | "h6"
                                | "li"
                                | "ul"
                                | "ol"
                                | "list"
                                | "quote"
                                | "code"
                                | "br"
                        ) {
                            text.push('\n');
                        }
                    } else {
                        path = true;
                    }
                    closing = None;
                    tag.clear();
                } else {
                    tag.push(character);
                }
            }
            None => match character {
                '[' => {
                    closing = Some(']');
                    tag.clear();
                }
                '{' => {
                    closing = Some('}');
                    tag.clear();
                }
                _ => text.push(character),
            },
        }
    }
    let mut lines: Vec<String> = Vec::new();
    for line in decode_entities(&text).lines() {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() {
            if lines.last().is_some_and(|last| !last.is_empty()) {
                lines.push(String::new());
            }
        } else {
            lines.push(line);
        }
    }
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    breathe(&lines.join("\n"))
}

/// Début lisible d'une annonce, coupé sur un mot entier.
pub fn news_excerpt(contents: &str) -> String {
    let flat = news_body(contents);
    if flat.chars().count() <= NEWS_EXCERPT {
        return flat;
    }
    let mut cut: String = flat.chars().take(NEWS_EXCERPT).collect();
    if let Some(index) = cut.rfind(' ') {
        cut.truncate(index);
    }
    cut.push('…');
    cut
}

/// Lit la réponse d'`appdetails` : c'est elle qui porte le texte, les captures
/// et les exigences. Tout est optionnel, la page est rendue telle quelle.
pub fn store_from_json(value: &Value, appid: u32) -> Option<SteamStorePage> {
    let data = value.get(appid.to_string())?.get("data")?;
    let detailed = json_str(data.get("detailed_description"));
    let mut page = SteamStorePage {
        short_description: json_str(data.get("short_description")),
        description: html_text(&strip_image_tags(&detailed)),
        description_blocks: description_blocks(&detailed),
        description_images: description_images(&detailed),
        header_image: json_str(data.get("header_image")),
        discount: json_u64(data.pointer("/price_overview/discount_percent"))
            .and_then(|discount| u32::try_from(discount).ok())
            .unwrap_or(0),
        recommendations: json_u64(data.pointer("/recommendations/total")),
        website: json_str(data.get("website")),
        support_url: json_str(data.pointer("/support_info/url")),
        support_email: json_str(data.pointer("/support_info/email")),
        legal_notice: html_text(&json_str(data.get("legal_notice"))),
        drm_notice: html_text(&json_str(data.get("drm_notice"))),
        requirements_min: requirements(data, "minimum"),
        requirements_recommended: requirements(data, "recommended"),
        release: json_str(data.pointer("/release_date/date")),
        ..Default::default()
    };
    page.price = match data.get("price_overview") {
        Some(price) => {
            let final_price = json_str(price.get("final_formatted"));
            if page.discount > 0 {
                format!(
                    "{final_price} · -{} % (au lieu de {})",
                    page.discount,
                    json_str(price.get("initial_formatted"))
                )
            } else {
                final_price
            }
        }
        None if data
            .get("is_free")
            .and_then(Value::as_bool)
            .unwrap_or(false) =>
        {
            "Gratuit".into()
        }
        None => String::new(),
    };
    if let Some(list) = data.get("screenshots").and_then(Value::as_array) {
        for shot in list.iter().take(SCREENSHOTS) {
            let path = json_str(shot.get("path_full"));
            if !path.is_empty() {
                page.screenshots.push(path);
            }
        }
    }
    // La bande-annonce : Steam publie plusieurs formats, le plus large
    // d'abord. Le lecteur de l'application ne lit que le fichier, pas la page.
    if let Some(movie) = data
        .get("movies")
        .and_then(Value::as_array)
        .and_then(|list| list.first())
    {
        page.trailer_poster = json_str(movie.get("thumbnail"));
        // Steam ne sert plus de fichier progressif : les manifestes DASH et
        // HLS portent la vidéo, et WebKit les lit avec GStreamer. L'ancien
        // couple mp4/webm reste en dernier recours pour les vieux jeux.
        for key in ["dash_h264", "hls_h264", "dash_av1"] {
            let url = json_str(movie.get(key));
            if !url.is_empty() {
                page.trailer = url;
                break;
            }
        }
        if page.trailer.is_empty() {
            for format in ["mp4", "webm"] {
                for key in ["max", "480"] {
                    let url = json_str(movie.pointer(&format!("/{format}/{key}")));
                    if !url.is_empty() {
                        page.trailer = url;
                        break;
                    }
                }
                if !page.trailer.is_empty() {
                    break;
                }
            }
        }
    }
    for (list, key) in [
        (&mut page.genres, "genres"),
        (&mut page.categories, "categories"),
    ] {
        if let Some(entries) = data.get(key).and_then(Value::as_array) {
            for entry in entries {
                let label = json_str(entry.get("description"));
                if !label.is_empty() && !list.contains(&label) {
                    list.push(label);
                }
            }
        }
    }
    // Les fonctionnalités gardent leur identifiant : c'est lui qui choisit le
    // pictogramme de l'encart, quel que soit le libellé traduit. Deux
    // identifiants peuvent décrire la même chose (les manettes DualSense) : le
    // libellé dédoublonne.
    if let Some(entries) = data.get("categories").and_then(Value::as_array) {
        for entry in entries {
            let label = json_str(entry.get("description"));
            let id = json_u64(entry.get("id"))
                .and_then(|id| u32::try_from(id).ok())
                .unwrap_or(0);
            if label.is_empty() || page.features.iter().any(|feature| feature.label == label) {
                continue;
            }
            page.features.push(SteamFeature { id, label });
        }
    }
    if let Some(descriptors) = data
        .pointer("/content_descriptors/ids")
        .and_then(Value::as_array)
    {
        for id in descriptors {
            let label = match id.as_u64() {
                Some(1) => "Nudité ou contenu sexuel",
                Some(2) => "Violence ou gore fréquents",
                Some(3) => "Contenu sexuel réservé aux adultes",
                Some(4) => "Nudité ou contenu sexuel fréquent",
                Some(5) => "Contenu mature général",
                _ => continue,
            };
            page.descriptors.push(label.to_string());
        }
    }
    Some(page)
}

/// La configuration matérielle que la boutique publie : celle de Linux quand
/// elle est publiée, celle de Windows sinon.
///
/// Steam publie pour les jeux sans version Linux un `linux_requirements`
/// réduit à son titre — « Minimale : » puis une liste vide. Cette section ne
/// dit rien : prendre la configuration Windows, c'est celle que Proton met en
/// œuvre.
fn requirements(data: &Value, kind: &str) -> String {
    fn read(data: &Value, section: &str, kind: &str) -> Option<String> {
        let entry = data.get(section)?;
        let text = match entry.get(kind) {
            Some(Value::String(text)) => html_text(text),
            Some(Value::Array(parts)) => html_text(
                &parts
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            _ => String::new(),
        };
        (!text.trim().is_empty()).then_some(text)
    }
    // Un titre suivi de rien (« Minimale : », puis une liste vide) ne
    // renseigne personne : la section n'a pas de corps.
    fn has_body(text: &str) -> bool {
        let mut lines = text.lines();
        let title = lines.next().unwrap_or_default().trim();
        if title.ends_with(':') {
            return lines.any(|line| !line.trim().is_empty());
        }
        true
    }
    match (
        read(data, "linux_requirements", kind),
        read(data, "pc_requirements", kind),
    ) {
        (Some(linux), Some(_windows)) if has_body(&linux) => linux,
        (Some(_), Some(windows)) => windows,
        (Some(linux), None) => linux,
        (None, Some(windows)) => windows,
        (None, None) => String::new(),
    }
}

/// Déballe le balisage d'un texte de boutique en texte lisible : les balises
/// disparaissent, les blocs se séparent par un saut de ligne, les entités sont
/// rendues, les lignes vides sont resserrées.
pub fn html_text(html: &str) -> String {
    let mut text = String::new();
    let mut name = String::new();
    let mut in_tag = false;
    for character in html.chars() {
        match character {
            '<' => {
                in_tag = true;
                name.clear();
            }
            '>' => {
                in_tag = false;
                let trimmed = name.trim();
                let closing = trimmed.starts_with('/');
                let tag = trimmed
                    .trim_start_matches('/')
                    .split(|c: char| c.is_whitespace() || c == '/')
                    .next()
                    .unwrap_or("")
                    .to_ascii_lowercase();
                let block = matches!(
                    tag.as_str(),
                    "p" | "li" | "ul" | "ol" | "div" | "h1" | "h2" | "h3" | "h4" | "tr"
                );
                if tag == "br" || (closing && block) {
                    text.push('\n');
                }
                name.clear();
            }
            _ if in_tag => name.push(character),
            _ => text.push(character),
        }
    }
    let decoded = decode_entities(&text);
    let mut lines: Vec<String> = Vec::new();
    for line in decoded.lines() {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if !line.is_empty() {
            lines.push(line);
        }
    }
    lines.join("\n")
}

/// Entités HTML courantes et références numériques.
fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(index) = rest.find('&') {
        out.push_str(&rest[..index]);
        let tail = &rest[index..];
        // La limite porte sur les caractères, pas sur les octets : couper
        // au douzième octet tombait au milieu d'un espace insécable et
        // faisait paniquer tout le daemon.
        let Some((end, _)) = tail
            .char_indices()
            .take(12)
            .find(|(_, character)| *character == ';')
        else {
            out.push('&');
            rest = &rest[index + 1..];
            continue;
        };
        let entity = &tail[1..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" => Some(' '),
            "mdash" => Some('—'),
            "ndash" => Some('–'),
            "hellip" => Some('…'),
            "deg" => Some('°'),
            "euro" => Some('€'),
            "times" => Some('×'),
            other => other
                .strip_prefix('#')
                .and_then(|number| {
                    let number = number.strip_prefix(['x', 'X']).map_or_else(
                        || number.parse::<u32>().ok(),
                        |hex| u32::from_str_radix(hex, 16).ok(),
                    );
                    number.and_then(char::from_u32)
                })
                .or_else(|| Some(' ').filter(|_| other == "shy")),
        };
        match decoded {
            Some(character) => out.push(character),
            None => out.push_str(&tail[..=end]),
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_borrowed_game_keeps_its_playtime_without_ownership() {
        let mut info = SteamGameInfo {
            appid: 620,
            ..Default::default()
        };
        let borrowed = crate::playtime::Playtime {
            minutes_total: 75,
            last_played: Some(1_700_000_000),
            family_shared: true,
            ..Default::default()
        };
        apply_playtime(&mut info, &borrowed);
        assert!(!info.account.owned);
        assert_eq!(info.account.minutes_total, Some(75));
        assert_eq!(info.account.last_played, Some(1_700_000_000));

        // Un jeu possédé et jamais lancé garde zéro minute, et reste possédé.
        let never = crate::playtime::Playtime::default();
        apply_playtime(&mut info, &never);
        assert!(info.account.owned);
        assert_eq!(info.account.minutes_total, Some(0));
        assert_eq!(info.account.minutes_2weeks, None);
        assert_eq!(info.account.last_played, None);
    }
    use lapinux_core::model::SteamAchievement;
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn the_store_asks_for_the_chosen_language() {
        // Sans réglage, la fiche parle français ; un nom Steam valide passe
        // tel quel ; une valeur inconnue ne casse pas la demande.
        assert_eq!(store_language(""), "french");
        assert_eq!(store_language("french"), "french");
        assert_eq!(store_language("english"), "english");
        assert_eq!(store_language("schinese"), "schinese");
        assert_eq!(store_language("klingon"), "french");
    }

    fn children(entries: Vec<(&str, KeyValue)>) -> KvValue {
        KvValue::Children(
            entries
                .into_iter()
                .map(|(key, value)| (key.to_string(), value))
                .collect::<BTreeMap<_, _>>(),
        )
    }

    fn kv(key: &str, value: KvValue) -> KeyValue {
        KeyValue {
            key: key.to_string(),
            value,
        }
    }

    fn text(key: &str, value: &str) -> KeyValue {
        kv(key, KvValue::String(value.to_string()))
    }

    /// Appinfo réduit d'un jeu : branches, dépôts, classifications.
    fn appinfo() -> KeyValue {
        let public = kv(
            "public",
            children(vec![
                ("buildid", kv("buildid", KvValue::String("16000000".into()))),
                (
                    "timebuildupdated",
                    kv("timebuildupdated", KvValue::String("1700000000".into())),
                ),
            ]),
        );
        let beta = kv(
            "beta",
            children(vec![
                ("buildid", kv("buildid", KvValue::String("15999999".into()))),
                ("description", text("description", "branche de test")),
            ]),
        );
        let depot = |id: &str, os: &str, language: &str, size: &str, download: &str| {
            kv(
                id,
                children(vec![
                    (
                        "config",
                        kv(
                            "config",
                            children(vec![
                                ("oslist", text("oslist", os)),
                                ("language", text("language", language)),
                            ]),
                        ),
                    ),
                    (
                        "manifests",
                        kv(
                            "manifests",
                            children(vec![(
                                "public",
                                kv(
                                    "public",
                                    children(vec![
                                        ("size", text("size", size)),
                                        ("download", text("download", download)),
                                    ]),
                                ),
                            )]),
                        ),
                    ),
                ]),
            )
        };
        let app = kv(
            "appinfo",
            children(vec![
                (
                    "common",
                    kv(
                        "common",
                        children(vec![
                            ("type", text("type", "game")),
                            ("release_state", text("release_state", "released")),
                            (
                                "ratings",
                                kv(
                                    "ratings",
                                    children(vec![(
                                        "pegi",
                                        kv(
                                            "pegi",
                                            children(vec![("rating", text("rating", "18"))]),
                                        ),
                                    )]),
                                ),
                            ),
                        ]),
                    ),
                ),
                (
                    "extended",
                    kv(
                        "extended",
                        children(vec![("listofdlc", text("listofdlc", "1017900,1017901"))]),
                    ),
                ),
                (
                    "ufs",
                    kv(
                        "ufs",
                        children(vec![
                            ("quota", text("quota", "1073741824")),
                            ("maxnumfiles", text("maxnumfiles", "128")),
                        ]),
                    ),
                ),
                (
                    "depots",
                    kv(
                        "depots",
                        children(vec![
                            (
                                "branches",
                                kv(
                                    "branches",
                                    children(vec![("public", public), ("beta", beta)]),
                                ),
                            ),
                            (
                                "1001",
                                depot("1001", "windows,linux", "english", "1000", "500"),
                            ),
                            ("1002", depot("1002", "linux", "french", "400", "200")),
                            ("1003", depot("1003", "linux", "english", "300", "150")),
                            ("baselanguages", text("baselanguages", "english,french")),
                        ]),
                    ),
                ),
            ]),
        );
        app
    }

    #[test]
    fn the_update_block_reads_branches_depots_and_sizes() {
        let update = update_from_pics(&appinfo(), None, Some("english"));
        assert_eq!(update.app_type, "game");
        assert_eq!(update.public_build, Some(16_000_000));
        assert_eq!(update.public_build_time, Some(1_700_000_000));
        assert_eq!(update.ratings, vec!["PEGI 18".to_string()]);
        assert_eq!(update.cloud_quota, Some(1_073_741_824));
        assert_eq!(update.cloud_files, Some(128));
        assert_eq!(update.dlc.len(), 2);
        assert_eq!(update.dlc[0].appid, 1_017_900);
        // Deux branches, la publique comprise.
        assert_eq!(update.branches.len(), 2);
        assert!(update
            .branches
            .iter()
            .any(|branch| branch.name == "beta" && branch.description == "branche de test"));
        // Les clés non numériques de `depots` ne sont pas des dépôts.
        assert_eq!(update.depots.len(), 3);
        // Le poids Linux écarte le doublage français et ne garde que l'anglais.
        assert_eq!(update.download, 650);
        assert_eq!(update.install, 1300);
        assert_eq!(update.depots[0].language, "Anglais");
        assert_eq!(update.depots[1].os, vec!["Linux".to_string()]);
    }
    #[test]
    fn a_depot_without_manifest_does_not_count() {
        let depot = kv(
            "5",
            children(vec![(
                "config",
                kv(
                    "config",
                    children(vec![("oslist", text("oslist", "linux"))]),
                ),
            )]),
        );
        let depots = kv("depots", children(vec![("5", depot)]));
        let root = kv("appinfo", children(vec![("depots", depots)]));
        assert_eq!(platform_size(&root, "linux", None, None), (0, 0));
        let depots = app_section(&root).get("depots").expect("dépôts");
        assert!(depot_list(depots).iter().all(|depot| depot.install == 0));
    }

    /// Le filtre des visuels de la description : AVIF, GIF et petites vidéos
    /// passent, les formats que la fiche ne sait pas lire restent dehors.
    #[test]
    fn the_description_keeps_only_supported_media() {
        assert!(supported_media("https://cdn/a.jpg"));
        assert!(supported_media("https://cdn/a.JPG?t=1790715525"));
        assert!(supported_media("https://cdn/a.png"));
        assert!(supported_media("https://cdn/a.webp?t=1"));
        assert!(supported_media("https://cdn/a.gif?t=1"));
        assert!(supported_media("https://cdn/a.avif?t=1"));
        assert!(supported_media("https://cdn/a.webm"));
        assert!(supported_media("https://cdn/a.mp4?t=1"));
        assert!(!supported_media("https://cdn/a.poster.mkv?t=1"));
        assert!(!supported_media("https://cdn/rien"));
    }

    /// Les petits GIF animés de la description passent le filtre comme les
    /// autres médias.
    #[test]
    fn the_description_collects_animated_gifs() {
        let html = r#"<p>Texte</p><img src="https://cdn/a.gif?t=1"><img src="https://cdn/b.gif">"#;
        assert_eq!(
            description_images(html),
            vec![
                "https://cdn/a.gif?t=1".to_string(),
                "https://cdn/b.gif".to_string(),
            ]
        );
    }

    /// `data-big-src` n'est pas une seconde image : seule l'adresse `src`
    /// compte, sinon chaque visuel apparaîtrait deux fois.
    #[test]
    fn the_description_ignores_the_alternate_image_attribute() {
        let html =
            r#"<img class="bb_img" src="https://cdn/a.avif" data-big-src="https://cdn/b.webp">"#;
        assert_eq!(
            description_images(html),
            vec!["https://cdn/a.avif".to_string()]
        );
    }

    /// Le texte et les médias doivent rester dans l'ordre de la page, pas
    /// former un bloc de texte suivi d'une pile d'images.
    #[test]
    fn the_description_keeps_text_and_media_in_order() {
        let html = r#"<p>Avant</p><img src="https://cdn/a.avif"><h2>Milieu</h2><video poster="https://cdn/poster.avif"><source src="https://cdn/clip.webm"></video><p>Après</p>"#;
        assert_eq!(
            description_blocks(html),
            vec![
                DescriptionBlock::Text("Avant".into()),
                DescriptionBlock::Media("https://cdn/a.avif".into()),
                DescriptionBlock::Text("Milieu".into()),
                DescriptionBlock::Media("https://cdn/clip.webm".into()),
                DescriptionBlock::Text("Après".into()),
            ]
        );
    }

    /// La présentation du jeu passe avant les nouvelles et les contenus
    /// additionnels, même lorsque Steam les publie en tête de description.
    #[test]
    fn the_about_section_moves_before_the_update_and_dlc() {
        let html = r#"<p>Mise à jour</p><img src="https://cdn/dlc.avif"><p>Contenu DLC</p><p>À propos du jeu</p><p>L'histoire</p><img src="https://cdn/histoire.avif"><p>Fin</p>"#;
        assert_eq!(
            description_blocks(html),
            vec![
                DescriptionBlock::Text("L'histoire".into()),
                DescriptionBlock::Media("https://cdn/histoire.avif".into()),
                DescriptionBlock::Text("Fin".into()),
                DescriptionBlock::Text("Mise à jour".into()),
                DescriptionBlock::Media("https://cdn/dlc.avif".into()),
                DescriptionBlock::Text("Contenu DLC".into()),
            ]
        );
    }

    /// Une boucle vidéo garde sa première source, pas l'affiche AVIF qui la
    /// double, et une vidéo sans source retombe sur son affiche.
    #[test]
    fn the_description_reads_video_blocks_and_their_fallback() {
        let html = r#"<video poster="https://cdn/poster.avif"><source src="https://cdn/clip.webm"><source src="https://cdn/clip.mp4"></video><video poster="https://cdn/fallback.avif"></video>"#;
        assert_eq!(
            description_images(html),
            vec![
                "https://cdn/clip.webm".to_string(),
                "https://cdn/fallback.avif".to_string(),
            ]
        );
    }

    #[test]
    fn the_store_page_is_read_with_its_text_and_requirements() {
        let value = json!({
            "440": {
                "success": true,
                "data": {
                    "type": "game",
                    "name": "Team Fortress 2",
                    "short_description": "Neuf classes.",
                    "detailed_description": "<h1>Titre</h1><p>Un jeu <strong>d'équipe</strong>.</p><video autoplay poster=\"https://cdn/affiche.avif\"><source src=\"https://cdn/clip.webm\"></video><img src=\"https://cdn/desc-1.jpg\"><br>Deuxième<br/>ligne[img]https://cdn/desc-2.jpg[/img]",
                    "header_image": "https://cdn/header.jpg",
                    "website": "https://teamfortress.com",
                    "screenshots": [
                        { "path_full": "https://cdn/1.jpg" },
                        { "path_full": "https://cdn/2.jpg" }
                    ],
                    "genres": [{"id": 1, "description": "Action"}],
                    "categories": [{"id": 9, "description": "Coopération"}],
                    "release_date": { "coming_soon": false, "date": "10 oct. 2007" },
                    "price_overview": {
                        "currency": "EUR",
                        "initial_formatted": "9,99€",
                        "final_formatted": "4,99€",
                        "discount_percent": 50
                    },
                    "recommendations": { "total": 1234567 },
                    "support_info": { "url": "https://support", "email": "aide@valve.com" },
                    "content_descriptors": { "ids": [2] },
                    "linux_requirements": { "minimum": "<strong>Processeur :</strong> 2 cœurs" },
                    "pc_requirements": { "minimum": "Windows seulement" }
                }
            }
        });
        let page = store_from_json(&value, 440).expect("page lue");
        assert_eq!(page.short_description, "Neuf classes.");
        assert_eq!(page.description, "Titre\nUn jeu d'équipe.\nDeuxième\nligne");
        assert_eq!(page.screenshots.len(), 2);
        // Les visuels placés dans le texte sont retenus, dans l'ordre, et le
        // texte reste lisible sans eux. Une vidéo garde sa première source,
        // `.webm`, plutôt que son affiche `.avif`.
        assert_eq!(
            page.description_images,
            vec![
                "https://cdn/clip.webm".to_string(),
                "https://cdn/desc-1.jpg".to_string(),
                "https://cdn/desc-2.jpg".to_string()
            ]
        );
        assert_eq!(
            page.description_blocks,
            vec![
                DescriptionBlock::Text("Titre\nUn jeu d'équipe.".into()),
                DescriptionBlock::Media("https://cdn/clip.webm".into()),
                DescriptionBlock::Media("https://cdn/desc-1.jpg".into()),
                DescriptionBlock::Text("Deuxième\nligne".into()),
                DescriptionBlock::Media("https://cdn/desc-2.jpg".into()),
            ]
        );
        assert_eq!(page.genres, vec!["Action".to_string()]);
        // Le prix porte la remise et l'ancien prix.
        assert_eq!(page.price, "4,99€ · -50 % (au lieu de 9,99€)");
        assert_eq!(page.discount, 50);
        assert_eq!(page.recommendations, Some(1_234_567));
        assert_eq!(
            page.descriptors,
            vec!["Violence ou gore fréquents".to_string()]
        );
        // Linux passe devant Windows quand la boutique le publie.
        assert_eq!(page.requirements_min, "Processeur : 2 cœurs");
        assert_eq!(page.support_email, "aide@valve.com");
        assert_eq!(page.release, "10 oct. 2007");
    }

    #[test]
    fn an_empty_linux_requirement_leaves_the_windows_one() {
        // Ce que Steam publie pour un jeu Windows sans version Linux : un
        // titre et une liste vide. La fiche doit montrer l'exigence réelle.
        let value = json!({
            "440": {
                "success": true,
                "data": {
                    "linux_requirements": {
                        "minimum": "<strong>Minimale\u{a0}:</strong><br><ul class=\"bb_ul\"></ul>"
                    },
                    "pc_requirements": {
                        "minimum": "<strong>Minimale\u{a0}:</strong><br><ul class=\"bb_ul\"><li>Processeur\u{a0}: 1.8 Ghz</li><li>Mémoire vive\u{a0}: 4\u{a0}GB</li></ul>"
                    }
                }
            }
        });
        let page = store_from_json(&value, 440).expect("page lue");
        assert!(
            page.requirements_min.contains("Processeur"),
            "{}",
            page.requirements_min
        );
        assert!(
            page.requirements_min.contains("Mémoire vive"),
            "{}",
            page.requirements_min
        );
    }

    #[test]
    fn a_real_linux_requirement_wins_over_windows() {
        let value = json!({
            "440": {
                "success": true,
                "data": {
                    "linux_requirements": { "minimum": "<ul><li>Processeur : x86 64 bits</li><li>Mémoire : 8 Go</li></ul>" },
                    "pc_requirements": { "minimum": "<ul><li>Windows seulement</li></ul>" }
                }
            }
        });
        let page = store_from_json(&value, 440).expect("page lue");
        assert!(
            page.requirements_min.contains("64 bits"),
            "{}",
            page.requirements_min
        );
    }

    #[test]
    fn a_store_answer_without_data_gives_nothing() {
        assert!(store_from_json(&json!({ "440": { "success": false } }), 440).is_none());
        assert!(store_from_json(&json!({}), 440).is_none());
    }

    /// Les fonctionnalités gardent l'identifiant Steam : c'est lui qui choisit
    /// le pictogramme. Deux identifiants pour la même chose ne font qu'une
    /// ligne.
    #[test]
    fn the_store_features_keep_their_identifier() {
        let value = json!({
            "440": {
                "success": true,
                "data": {
                    "categories": [
                        { "id": 2, "description": "Solo" },
                        { "id": 23, "description": "Steam Cloud" },
                        { "id": 57, "description": "Manettes DualSense" },
                        { "id": 58, "description": "Manettes DualSense" }
                    ]
                }
            }
        });
        let page = store_from_json(&value, 440).expect("page lue");
        assert_eq!(
            page.features,
            vec![
                SteamFeature {
                    id: 2,
                    label: "Solo".into()
                },
                SteamFeature {
                    id: 23,
                    label: "Steam Cloud".into()
                },
                SteamFeature {
                    id: 57,
                    label: "Manettes DualSense".into()
                },
            ]
        );
    }

    /// Le flux public mêle la presse et l'éditeur : seule la seconde est une
    /// actualité du jeu.
    #[test]
    fn news_keep_only_the_editor_announcements() {
        let value = json!({
            "appnews": {
                "newsitems": [
                    {
                        "feedname": "PlayGround.ru - официальная группа",
                        "title": "Article de presse",
                        "contents": "Саундтреки номинированы",
                        "date": 1793003505
                    },
                    {
                        "feedname": "steam_community_announcements",
                        "title": "Mise à jour 1.2",
                        "contents": "[b]Nouveautés[/b]\n\n[img]{STEAM_CLAN_IMAGE}/36260.jpg[/img] Le mode [url=https://exemple]coop[/url] arrive.",
                        "date": 1793032250,
                        "author": "studio",
                        "url": "https://steamcommunity.com/games/440/announcements/1"
                    }
                ]
            }
        });
        let news = news_from_json(&value);
        assert_eq!(news.len(), 1, "la presse doit être écartée");
        assert_eq!(news[0].title, "Mise à jour 1.2");
        assert_eq!(news[0].date, 1_793_032_250);
        assert_eq!(news[0].author, "studio");
        assert!(news[0].url.ends_with("/announcements/1"));
        // Le texte brut sépare déjà les deux paragraphes : ils restent
        // séparés au lieu d'être recollés en une seule ligne.
        assert_eq!(news[0].excerpt, "Nouveautés\n\nLe mode coop arrive.");
    }

    /// Le visuel récent est déjà dans le BBCode ; on ne va chercher
    /// `og:image` que si l'annonce n'en porte aucun.
    #[test]
    fn an_announcement_image_can_come_from_bbcode() {
        assert_eq!(
            news_image("[img src=\"{STEAM_CLAN_IMAGE}/456/abc.jpg\"] La suite").as_deref(),
            Some("https://clan.akamai.steamstatic.com/images/456/abc.jpg")
        );
        assert_eq!(
            news_image("[img]https://exemple.fr/x.png[/img]").as_deref(),
            Some("https://exemple.fr/x.png")
        );
        assert!(news_image("aucune image ici").is_none());
    }

    /// Le visuel d'une annonce se lit dans sa balise `og:image`.
    #[test]
    fn the_announcement_image_comes_from_the_open_graph_tag() {
        let html = r#"<html><head><meta property="og:image" content="https://clan.akamai.steamstatic.com/images/45647043/abc.jpg"><meta name="twitter:image" content="https://autre"></head></html>"#;
        assert_eq!(
            og_image(html).as_deref(),
            Some("https://clan.akamai.steamstatic.com/images/45647043/abc.jpg")
        );
        assert!(og_image("<html><head></head></html>").is_none());
        // Une image relative ne sert à rien à l'interface.
        assert!(og_image("<meta property=\"og:image\" content=\"/images/x.jpg\">").is_none());
    }

    /// Deux phrases que Steam a collées se séparent dans l'aperçu.
    #[test]
    fn a_merged_sentence_regains_its_space() {
        assert_eq!(
            news_excerpt("Cast your vote now!https://exemple.fr"),
            "Cast your vote now! https://exemple.fr"
        );
        assert_eq!(
            news_excerpt("Light!We are honoured."),
            "Light! We are honoured."
        );
        // Un sigle reste tel quel : aucune majuscule ne suit un point ici.
        assert_eq!(news_excerpt("Sortie le 1.2Beta"), "Sortie le 1.2Beta");
    }

    /// Une annonce longue se coupe sur un mot : pas de moitié de mot dans
    /// l'encart.
    #[test]
    fn a_long_announcement_is_cut_on_a_word() {
        let contents = "mot ".repeat(200);
        let excerpt = news_excerpt(&contents);
        assert!(excerpt.ends_with('…'), "{excerpt}");
        assert!(!excerpt.contains("mo…"), "{excerpt}");
        assert!(excerpt.chars().count() <= NEWS_EXCERPT + 1, "{excerpt}");
        // Sans un seul espace, la coupe reste au nombre de caractères.
        assert_eq!(
            news_excerpt(&"a".repeat(400)).chars().count(),
            NEWS_EXCERPT + 1
        );
    }

    /// La fenêtre de recherche d'une entité se compte en caractères : un
    /// espace insécable ne doit jamais être coupé au milieu de son UTF-8.
    #[test]
    fn an_entity_window_never_splits_a_character() {
        let text = "&abcdefghij\u{a0};";
        assert_eq!(decode_entities(text), text);
    }

    /// Les balises de bloc BBCode deviennent de vrais paragraphes.
    #[test]
    fn announcement_paragraphs_keep_their_breaks() {
        let text = news_body("[p]Premier[/p][p align=\"start\"]Deuxième[/p]");
        let visible: Vec<&str> = text.lines().filter(|line| !line.is_empty()).collect();
        assert_eq!(visible, ["Premier", "Deuxième"]);
        assert!(text.contains("\n\n"), "{text}");
    }

    /// Configuration minimale telle que Steam la publie pour Age of Empires:
    /// Definitive Edition : des listes imbriquées et des espaces
    /// insécables. La fiche ne doit pas s'arrêter au titre.
    #[test]
    fn a_requirement_list_is_flattened_whole() {
        let raw = "<strong>Minimale\u{a0}:</strong><br><ul class=\"bb_ul\"><li>Système d'exploitation et \
                   processeur 64 bits nécessaires<br></li><li><strong>Processeur\u{a0}:</strong> 1.8 Ghz+ Dual Core\
                   </li></ul>";
        let text = html_text(raw);
        assert!(text.contains("Minimale"), "{text}");
        assert!(
            text.contains("Système d'exploitation et processeur 64 bits"),
            "{text}"
        );
        assert!(text.contains("Processeur : 1.8 Ghz+ Dual Core"), "{text}");
        assert!(text.lines().count() >= 3, "{text}");
    }

    #[test]
    fn html_is_flattened_and_entities_are_decoded() {
        assert_eq!(html_text("<p>A &amp; B</p>"), "A & B");
        assert_eq!(html_text("Un&#39;appart&eacute;"), "Un'appart&eacute;");
        assert_eq!(html_text("Ligne<br>suivante"), "Ligne\nsuivante");
        assert_eq!(html_text("<ul><li>Un</li><li>Deux</li></ul>"), "Un\nDeux");
        assert_eq!(html_text("  trop   d'espaces  "), "trop d'espaces");
        assert_eq!(html_text("&#8212;tiret"), "—tiret");
    }

    #[test]
    fn the_installed_build_survives_the_protocol_read() {
        let installed = SteamUpdateInfo {
            installed_build: Some(15_000_000),
            installed_build_time: Some(1_690_000_000),
            ..Default::default()
        };
        let merged = merge_installed(
            &installed,
            update_from_pics(&appinfo(), None, Some("english")),
        );
        assert_eq!(merged.public_build, Some(16_000_000));
        assert_eq!(merged.installed_build, Some(15_000_000));
        assert_eq!(merged.installed_build_time, Some(1_690_000_000));
    }

    fn achievement(name: &str, percent: Option<f32>, achieved: bool) -> SteamAchievement {
        SteamAchievement {
            name: name.into(),
            percent,
            achieved,
            ..Default::default()
        }
    }

    #[test]
    fn the_rarest_successes_come_from_the_protocol_schema() {
        let read = SteamAchievements {
            appid: 4000,
            items: vec![
                achievement("Courant", Some(84.3), false),
                achievement("Rare", Some(1.5), true),
                achievement("Sans rareté", None, false),
            ],
            ..Default::default()
        };
        let list = rarest(&read);
        // Les moins obtenus d'abord, et seulement ceux que Steam a notés.
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "Rare");
        assert!((list[0].percent - 1.5).abs() < 0.01);
        assert!(list[0].unlocked);
        assert_eq!(list[1].name, "Courant");
        assert!(!list[1].unlocked);
        assert!(rarest(&SteamAchievements::default()).is_empty());
    }

    #[test]
    fn the_dlc_banner_path_comes_from_the_appinfo() {
        // Forme réelle de `common.header_image` : un chemin par langue, dont on
        // ne garde qu'un seul exemplaire.
        let header = kv(
            "header_image",
            children(vec![
                (
                    "english",
                    text(
                        "english",
                        "9f05c4af67da067bfa4b0ccbceef1e20613a616e/header.jpg",
                    ),
                ),
                ("french", text("french", "abcdef/header_french.jpg")),
            ]),
        );
        let common = kv("common", children(vec![("header_image", header)]));
        assert_eq!(
            store_image(Some(&common)),
            "9f05c4af67da067bfa4b0ccbceef1e20613a616e/header.jpg"
        );
        // Ancien appinfo : un simple `header.jpg` sans chemin.
        let flat = kv(
            "common",
            children(vec![("header_image", text("header_image", "header.jpg"))]),
        );
        assert_eq!(store_image(Some(&flat)), "header.jpg");
        // Rien de publié : la vignette se rabat sur l'adresse à plat.
        let empty = kv("common", children(Vec::new()));
        assert!(store_image(Some(&empty)).is_empty());
        assert!(store_image(None).is_empty());
    }

    #[test]
    fn a_fiche_needs_a_session_and_nothing_else() {
        // Aucune clé à demander : la session enregistrée suffit, et sans elle
        // la fiche se lit quand même côté boutique.
        let mut sources = Sources::default();
        assert!(!sources.has_session());
        sources.refresh_token = "refresh".into();
        assert!(sources.has_session());
    }
}
