//! Session web de Steam : les cookies du site, tirés du jeton de protocole.
//!
//! Le CM et le site ne reconnaissent pas la même preuve d'identité. Le jeton de
//! rafraîchissement ouvre le protocole (`licenses_print`, PICS, la famille) ;
//! `store.steampowered.com` ne lit, lui, que des cookies — dont
//! `steamLoginSecure`. Steam sait échanger l'un contre l'autre :
//! `jwt/finalizelogin` rend des transferts signés, et chaque domaine du site
//! les échange contre ses propres cookies.
//!
//! Rien de tout cela n'est documenté. La page des licences est du HTML, et
//! c'est la seule de tout Steam qui porte une date d'achat : `GetOwnedGames`
//! n'a que du temps de jeu, `ClientLicenseList` que le `time_created` des
//! paquets — absent pour certains jeux, comme Insurgency (222880).

use std::time::Duration;

/// Échange jeton de rafraîchissement → transferts signés.
const FINALIZE_URL: &str = "https://login.steampowered.com/jwt/finalizelogin";
/// Page des licences du compte, avec sa colonne « Date d'achat ».
pub const LICENSES_URL: &str = "https://store.steampowered.com/account/licenses/?l=french";
/// Où retomber après l'échange : le site communautaire, comme le navigateur.
const REDIRECT: &str = "https://steamcommunity.com/login/home/?goto=";
/// Bornes de pages lues dans les abonnements Workshop : un compte en annonce
/// quelques-unes, la borne évite une boucle si Steam répète sa dernière page.
const WORKSHOP_MAX_PAGES: usize = 40;

/// Cookies du site, tels que le navigateur les porterait.
#[derive(Debug, Default, Clone)]
pub struct WebCookies {
    /// Badge de session du site (`<steamid>||<jeton>`).
    pub steam_login_secure: String,
    /// Identifiant de session, joint aux formulaires du site.
    pub sessionid: String,
    /// Cookies secondaires du domaine (`browserid`, `steamCountry`…).
    pub extra: Vec<(String, String)>,
}

impl WebCookies {
    /// En-tête `Cookie` d'une requête au site.
    pub fn header(&self) -> String {
        let mut parts = Vec::with_capacity(2 + self.extra.len());
        if !self.steam_login_secure.is_empty() {
            parts.push(format!("steamLoginSecure={}", self.steam_login_secure));
        }
        if !self.sessionid.is_empty() {
            parts.push(format!("sessionid={}", self.sessionid));
        }
        for (name, value) in &self.extra {
            parts.push(format!("{name}={value}"));
        }
        parts.join("; ")
    }

    /// La session porte-t-elle le badge qui ouvre les pages du compte ?
    pub fn is_signed_in(&self) -> bool {
        !self.steam_login_secure.is_empty()
    }
}

/// Réponse de la page des licences : le statut et le HTML, sans analyse.
pub struct LicensesPage {
    pub status: u16,
    pub body: String,
}

/// Jeton d'accès Web API du compte, demandé au CM : c'est lui qui sert de badge
/// au site, et le seul chemin qui reste pour une session de client.
pub fn access_token(account: &str, refresh_token: &str, steam_id: u64) -> Result<String, String> {
    crate::shared::block_on(async move {
        let client = crate::shared::session(account, refresh_token)
            .await
            .map_err(|error| format!("session CM : {error}"))?;
        crate::native::web_access_token(&client, steam_id, refresh_token)
            .await
            .ok_or_else(|| "Steam n'a pas rendu de jeton d'accès".to_string())
    })
}

/// Dates d'achat du compte, lues sur la page des licences.
///
/// Rend des couples `(nom publié par Steam, date)`. La page ne porte aucun
/// appid : le rapprochement avec le catalogue se fait par le nom, et c'est à
/// l'appelant de décider comment — les noms du site portent parfois une
/// précision de région (« Insurgency (WW) »).
pub fn license_dates(
    account: &str,
    refresh_token: &str,
    steam_id: u64,
) -> Result<Vec<(String, i64)>, String> {
    // La page refuse parfois un jeton qui vient d'être frappé (403 d'Edge) :
    // chaque essai repart donc d'un jeton neuf.
    // Économe : un essai qui échoue reprend à la page qui l'a arrêté, avec un
    // jeton neuf, plutôt que de relire les pages déjà obtenues. Une lecture
    // partielle rend quand même ses lignes — les jeux qu'elle ne couvre pas
    // gardent la date de la synchronisation précédente.
    let mut rows = Vec::new();
    let mut start: Option<String> = None;
    let mut last = String::new();
    for attempt in 0..4 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(5000));
        }
        let token = access_token(account, refresh_token, steam_id)?;
        let cookies = cookies_from_access_token(steam_id, &token);
        match crate::shared::block_on(license_rows(cookies, start.clone(), &mut rows)) {
            Ok(()) => {
                let dated = rows.iter().filter(|row| row.acquired.is_some()).count();
                tracing::info!(
                    "page des licences : {} ligne(s), {dated} datée(s)",
                    rows.len()
                );
                return Ok(rows
                    .into_iter()
                    .filter_map(|row| row.acquired.map(|stamp| (row.name, stamp)))
                    .collect());
            }
            Err((resume, error)) => {
                last = format!("{error} (essai {})", attempt + 1);
                start = resume.or(start);
            }
        }
    }
    if !rows.is_empty() {
        tracing::info!(
            "page des licences : lecture partielle, {} ligne(s) gardée(s) ({last})",
            rows.len()
        );
        return Ok(rows
            .into_iter()
            .filter_map(|row| row.acquired.map(|stamp| (row.name, stamp)))
            .collect());
    }
    Err(last)
}

/// Page d'historique des transactions du compte : chaque achat porte sa date
/// et les jeux achetés, avec leur appid. C'est une source distincte de la page
/// des licences, dont les lignes ne donnent qu'un nom de paquet.
pub const HISTORY_URL: &str = "https://store.steampowered.com/account/history/?l=french";

/// Une transaction de l'historique : sa date et les jeux qu'elle a accordés.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Purchase {
    pub acquired: i64,
    pub appids: Vec<u32>,
}

/// Dates d'achat par appid, lues dans l'historique des transactions.
///
/// Rend des couples `(appid, date)`, la plus ancienne quand un jeu apparaît
/// dans plusieurs transactions. Une lecture qui échoue ne rend rien : c'est à
/// l'appelant de garder les dates qu'il possède déjà.
pub fn purchase_dates(
    account: &str,
    refresh_token: &str,
    steam_id: u64,
) -> Result<Vec<(u32, i64)>, String> {
    let mut last = String::new();
    for attempt in 0..3 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(4000));
        }
        let token = access_token(account, refresh_token, steam_id)?;
        let cookies = cookies_from_access_token(steam_id, &token);
        match crate::shared::block_on(async move {
            // La première page ne porte que les achats récents : l'historique
            // se parcourt en entier, sinon les vieux jeux restent sans date.
            let mut purchases = Vec::new();
            let mut seen = std::collections::HashSet::new();
            let mut url = HISTORY_URL.to_string();
            while seen.insert(url.clone()) && seen.len() <= HISTORY_MAX_PAGES {
                let body = fetch_page(&cookies, &url).await?;
                let rows = parse_history(&body);
                if rows.is_empty() {
                    break;
                }
                purchases.extend(rows);
                match next_page(&body) {
                    Some(next) => url = history_page_url(&next),
                    None => break,
                }
                tokio::time::sleep(Duration::from_millis(400)).await;
            }
            Ok::<_, String>(purchases)
        }) {
            Ok(purchases) => {
                let named = purchases
                    .iter()
                    .filter(|row| !row.appids.is_empty())
                    .count();
                tracing::info!(
                    "historique : {} transaction(s), {named} avec appid(s)",
                    purchases.len()
                );
                let mut dates: std::collections::HashMap<u32, i64> =
                    std::collections::HashMap::new();
                for purchase in purchases {
                    for appid in purchase.appids {
                        let kept = dates.entry(appid).or_insert(purchase.acquired);
                        *kept = (*kept).min(purchase.acquired);
                    }
                }
                let mut sorted: Vec<(u32, i64)> = dates.into_iter().collect();
                sorted.sort_unstable();
                return Ok(sorted);
            }
            Err(error) => last = format!("{error} (essai {})", attempt + 1),
        }
    }
    Err(last)
}

/// Lit les transactions de l'historique : une ligne porte sa date et les liens
/// vers les jeux achetés. La mise en page change avec la langue, donc la date
/// est cherchée dans les cellules plutôt qu'à un rang fixe.
pub fn parse_history(body: &str) -> Vec<Purchase> {
    let mut purchases = Vec::new();
    for chunk in body.split("<tr").skip(1) {
        let cells: Vec<String> = chunk.split("<td").skip(1).map(cell_text).collect();
        let Some(acquired) = cells.iter().find_map(|cell| parse_date(cell)) else {
            continue;
        };
        let mut appids = Vec::new();
        for (at, _) in chunk.match_indices("/app/") {
            let rest = &chunk[at + 5..];
            let digits: String = rest
                .chars()
                .take_while(|character| character.is_ascii_digit())
                .collect();
            if let Ok(appid) = digits.parse::<u32>() {
                if appid > 0 && !appids.contains(&appid) {
                    appids.push(appid);
                }
            }
        }
        purchases.push(Purchase { acquired, appids });
    }
    purchases
}

/// Toutes les pages de licences, lues d'affilée.
/// Lit les pages de licences à partir de `start` (rien = première page) et
/// ajoute leurs lignes à `rows`.
///
/// En cas d'échec, rend la page où reprendre : les pages déjà lues ne sont pas
/// jetées, et l'essai suivant repart de là au lieu de tout relire.
async fn license_rows(
    cookies: WebCookies,
    start: Option<String>,
    rows: &mut Vec<LicenseRow>,
) -> std::result::Result<(), (Option<String>, String)> {
    // La page annonce son découpage (« 1-100 sur 1991 ») et un lien
    // « Suivante » : la parcourir entièrement est le seul moyen de voir les
    // licences au-delà des cent premières — dont les clés revendeur.
    let mut seen = std::collections::HashSet::new();
    let mut url = start.unwrap_or_else(|| LICENSES_URL.to_string());
    let mut pages = 0usize;
    while pages < max_pages() && seen.insert(url.clone()) {
        let body = match fetch_page(&cookies, &url).await {
            Ok(body) => body,
            Err(error) => return Err((Some(url), format!("page {pages} : {error}"))),
        };
        pages += 1;
        let page_rows = parse_licenses(&body);
        if page_rows.is_empty() {
            break;
        }
        rows.extend(page_rows);
        // Une rafale de pages fait refuser la suivante par Edge. Une seconde
        // entre deux pages tient le seuil, au prix de vingt secondes.
        tokio::time::sleep(Duration::from_millis(1000)).await;
        match next_page(&body).map(|next| license_page_url(&next)) {
            Some(next) => url = next,
            None => break,
        }
    }
    tracing::info!("{pages} page(s) lues dans cette passe");
    Ok(())
}

/// Bornes de pages lues dans l'historique des achats : un compte en annonce
/// quelques-unes, la borne évite une boucle si le lien « Suivante » se répète.
const HISTORY_MAX_PAGES: usize = 40;

/// Nombre maximal de pages de licences lues. Un compte en annonce une
/// vingtaine ; la borne évite une boucle si le lien « Suivante » se répète.
const LICENSES_MAX_PAGES: usize = 80;

/// Plafond de la passe en cours. `LAPINUX_LICENSES_PAGES=3` borne un essai à
/// quelques requêtes, le temps de voir si Steam répond encore ; sans la
/// variable, la lecture va au bout des pages annoncées.
fn max_pages() -> usize {
    std::env::var("LAPINUX_LICENSES_PAGES")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|pages| *pages > 0)
        .map(|pages| pages.min(LICENSES_MAX_PAGES))
        .unwrap_or(LICENSES_MAX_PAGES)
}

/// Adresse de la page suivante, lue dans le lien « Suivante » du paginateur.
/// Steam l'écrit en HTML (`&amp;offset=100`) et donne un jeton `x` qui fait
/// partie du découpage : l'adresse est reprise telle quelle.
fn next_page(body: &str) -> Option<String> {
    // Les deux pages du compte nomment leur paginateur `<…>_paginator_next` :
    // le suffixe commun les couvre, licences comme historique.
    let marker = body.find("paginator_next")?;
    let rest = &body[marker..];
    let start = rest.find("href=\"")? + 6;
    let tail = &rest[start..];
    let end = tail.find('"')?;
    let href = tail[..end].replace("&amp;", "&");
    let href = href.trim();
    (!href.is_empty()).then(|| href.to_string())
}

/// Adresse absolue d'une page de licences : Steam la donne relative
/// (`?x=…&offset=100`), la page d'origine est le dossier des licences.
fn license_page_url(href: &str) -> String {
    if href.starts_with("http") {
        href.to_string()
    } else {
        format!("https://store.steampowered.com/account/licenses/{href}")
    }
}

/// Adresse absolue d'une page d'historique : Steam la donne relative
/// (`?p=2`, `?x=…&offset=100`), la page d'origine est celle des achats.
fn history_page_url(href: &str) -> String {
    if href.starts_with("http") {
        href.to_string()
    } else {
        format!("https://store.steampowered.com/account/history/{href}")
    }
}

/// Lit une page du compte avec réessai : Edge refuse par moments la première
/// demande (403) et la suivante passe.
async fn fetch_page(cookies: &WebCookies, url: &str) -> Result<String, String> {
    let mut last = String::new();
    for attempt in 0..3 {
        // La boutique se repose : inutile d'insister, chaque refus ne fait
        // qu'allonger la fermeture.
        if crate::storegate::resting() {
            return Err("boutique au repos après des refus (403/429)".to_string());
        }
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(2500)).await;
        }
        match fetch_account_page(cookies, url).await {
            Ok(page) if page.status == 200 => return Ok(page.body),
            Ok(page) => {
                last = format!(
                    "page des licences : {} ({} octets)",
                    page.status,
                    page.body.len()
                )
            }
            Err(error) => last = error,
        }
    }
    Err(last)
}

/// Rapproche un nom de licence du site et un nom de jeu du catalogue : mêmes
/// mots, à la ponctuation près, une fois la région retirée (« Insurgency » et
/// « Insurgency (WW) »). L'égalité seule : « Portal 2 » n'est pas « Portal ».
pub fn same_title(license: &str, game: &str) -> bool {
    let left = normalized_title(&core_title(license));
    let right = normalized_title(&core_title(game));
    !left.is_empty() && left == right
}

/// Nom sans ses mentions de vente : « Insurgency (WW) » → « Insurgency »,
/// « Among the Sleep (Global) - Retail » → « Among the Sleep ». La page des
/// licences nomme des paquets, qui portent ces précisions ; le jeu, non.
fn core_title(text: &str) -> String {
    let mut title = text.trim().to_string();
    loop {
        // Un suffixe « - Retail », « - Global » : le tiret d'un titre comme
        // « Half-Life 2 » ne porte pas d'étiquette de vente, il est gardé.
        if let Some((head, tail)) = title.rsplit_once('-') {
            if !head.trim().is_empty() && is_sale_tag(tail) {
                title = head.trim().to_string();
                continue;
            }
        }
        // Une mention entre parenthèses en fin de nom : « (WW) », « (Global) ».
        if title.ends_with(')') {
            if let Some(open) = title.rfind('(') {
                if title.len() - open <= 14 && is_sale_tag(&title[open + 1..title.len() - 1]) {
                    title = title[..open].trim().to_string();
                    continue;
                }
            }
        }
        return title;
    }
}

/// Étiquette de vente accolée à un nom de paquet, jamais au nom du jeu.
fn is_sale_tag(tag: &str) -> bool {
    matches!(
        tag.trim().to_ascii_lowercase().as_str(),
        "ww" | "ww2"
            | "global"
            | "worldwide"
            | "row"
            | "retail"
            | "steam"
            | "gift"
            | "preorder"
            | "eu"
            | "us"
            | "na"
            | "ru"
            | "cis"
            | "latam"
            | "br"
            | "kr"
            | "jp"
            | "cn"
            | "asia"
            | "tr"
            | "de"
            | "fr"
            | "uk"
            | "pl"
            | "au"
    )
}

fn normalized_title(text: &str) -> String {
    text.chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Cookies du site pour une session de client : le jeton d'accès *est* le badge.
///
/// Steam ne remet plus de cookies web à un jeton de client (`AccessDenied`
/// depuis le 30 avril 2025) : pour ce type de session, `steamLoginSecure` vaut
/// `<steamid>||<jeton d'accès>`, pourcentage-encodé. Le jeton s'obtient par le
/// CM — c'est `access_token` — et se présente tel quel au site.
pub fn cookies_from_access_token(steam_id: u64, access_token: &str) -> WebCookies {
    let mut value = String::new();
    percent_encode(&format!("{steam_id}||{access_token}"), &mut value);
    WebCookies {
        steam_login_secure: value,
        sessionid: session_id(),
        extra: Vec::new(),
    }
}

/// Résultat d'une récupération de licence gratuite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LicenseClaim {
    /// La licence vient d'être ajoutée au compte.
    Added,
    /// Steam répond qu'elle y était déjà.
    AlreadyOwned,
    /// La page de l'offre ne publie pas de paquet gratuit : il faut la
    /// terminer dans la boutique.
    NeedsPage(String),
}

/// Ajoute au compte la licence gratuite d'une offre Steam à conserver.
///
/// La page de l'application publie les paquets ; celui qui est marqué gratuit
/// est celui de la promotion. Steam limite les ajouts en rafale : l'appelant
/// espace les offres.
pub fn claim_free_license(cookies: &WebCookies, appid: u32) -> Result<LicenseClaim, String> {
    let details =
        format!("https://store.steampowered.com/api/appdetails?appids={appid}&cc=fr&l=french");
    let referer = format!("https://store.steampowered.com/app/{appid}/");
    let value = store_get_json(cookies, &details, &referer)?;
    let data = value
        .get(appid.to_string().as_str())
        .and_then(|item| item.get("data"));
    let Some(data) = data else {
        return Ok(LicenseClaim::NeedsPage("fiche Steam illisible".into()));
    };
    let package = free_package(data);
    let Some(package) = package else {
        // Un jeu déjà permanentment gratuit n'a pas de paquet à réclamer.
        if data.get("is_free").and_then(serde_json::Value::as_bool) == Some(true) {
            return Ok(LicenseClaim::AlreadyOwned);
        }
        return Ok(LicenseClaim::NeedsPage(
            "aucun paquet gratuit publié".into(),
        ));
    };
    let url = format!("https://store.steampowered.com/checkout/addfreelicense/{package}");
    let response = ureq::post(&url)
        .set("User-Agent", STORE_UA)
        .set("Accept", "application/json, text/javascript, */*; q=0.01")
        .set("Accept-Language", "fr-FR,fr;q=0.9,en;q=0.8")
        .set("X-Requested-With", "XMLHttpRequest")
        .set("Referer", &referer)
        .set("Cookie", &cookies.header())
        .set("Content-Type", "application/x-www-form-urlencoded")
        .timeout(Duration::from_secs(20))
        .send_form(&[("ajax", "1"), ("sessionid", cookies.sessionid.as_str())])
        .map_err(|error| error.to_string())?;
    let text = response.into_string().map_err(|error| error.to_string())?;
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed == "[]" {
        return Ok(LicenseClaim::Added);
    }
    let value: serde_json::Value = serde_json::from_str(trimmed).map_err(|_| {
        "Steam a renvoyé une page au lieu d'une réponse : la session web a peut-être expiré"
            .to_string()
    })?;
    if value.is_null() {
        return Err("Steam n'a pas confirmé la licence".to_string());
    }
    match value
        .get("purchaseresultdetail")
        .and_then(serde_json::Value::as_i64)
    {
        Some(9) => Ok(LicenseClaim::AlreadyOwned),
        Some(0) | None => Ok(LicenseClaim::Added),
        Some(code) => Err(format!("Steam a refusé la licence ({code})")),
    }
}

/// Le paquet de promotion gratuit publié par la fiche d'une application.
fn free_package(data: &serde_json::Value) -> Option<u64> {
    data.get("package_groups")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|group| group.get("subs").and_then(serde_json::Value::as_array))
        .flatten()
        .find(|sub| {
            sub.get("is_free_license")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
                || sub
                    .get("price_in_cents_with_discount")
                    .and_then(serde_json::Value::as_i64)
                    == Some(0)
        })
        .and_then(|sub| sub.get("packageid"))
        .and_then(serde_json::Value::as_u64)
}

const STORE_UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

fn store_get_json(
    cookies: &WebCookies,
    url: &str,
    referer: &str,
) -> Result<serde_json::Value, String> {
    let response = ureq::get(url)
        .set("User-Agent", STORE_UA)
        .set("Accept", "application/json, text/javascript, */*; q=0.01")
        .set("Accept-Language", "fr-FR,fr;q=0.9,en;q=0.8")
        .set("X-Requested-With", "XMLHttpRequest")
        .set("Referer", referer)
        .set("Cookie", &cookies.header())
        .timeout(Duration::from_secs(20))
        .call()
        .map_err(|error| error.to_string())?;
    let text = response.into_string().map_err(|error| error.to_string())?;
    serde_json::from_str(&text).map_err(|error| error.to_string())
}

/// Échange le jeton de rafraîchissement contre les cookies du site.
///
/// `nonce` est le jeton lui-même : Steam le reconnaît et rend, pour chaque
/// domaine (boutique, communauté, aide), une adresse et ses paramètres signés.
/// Chaque transfert est posté sans suivre la redirection : ce sont les en-têtes
/// `Set-Cookie` de la réponse qui comptent.
pub fn web_cookies(refresh_token: &str) -> Result<WebCookies, String> {
    web_cookies_traced(refresh_token).map(|(cookies, _)| cookies)
}

/// Même échange, avec le journal des étapes : ce qui a répondu quoi, et quels
/// cookies chaque domaine a posés. Sert à diagnostiquer un refus que Steam ne
/// motive pas.
pub fn web_cookies_traced(refresh_token: &str) -> Result<(WebCookies, Vec<String>), String> {
    if refresh_token.trim().is_empty() {
        return Err("jeton de rafraîchissement vide".to_string());
    }
    crate::shared::block_on(exchange(refresh_token))
}

/// Ouvre la page des licences du compte : les cookies d'abord, puis le HTML.
pub fn licenses_page(refresh_token: &str) -> Result<(WebCookies, LicensesPage), String> {
    crate::shared::block_on(async move {
        let (cookies, steps) = exchange(refresh_token).await?;
        if !cookies.is_signed_in() {
            return Err(format!("session web refusée ({})", steps.join(" ; ")));
        }
        let page = fetch_account_page(&cookies, LICENSES_URL).await?;
        Ok((cookies, page))
    })
}

/// Lit la page des licences avec une session déjà ouverte.
pub fn licenses(cookies: &WebCookies) -> Result<LicensesPage, String> {
    crate::shared::block_on(fetch_account_page(cookies, LICENSES_URL))
}

/// Adresse de la page des abonnements Workshop du compte pour un jeu.
fn workshop_subscriptions_url(steam_id: u64, appid: u32, page: usize) -> String {
    format!(
        "https://steamcommunity.com/profiles/{steam_id}/myworkshopfiles/?appid={appid}&browsefilter=mysubscriptions&browsesort=mysubscriptions&numperpage=30&p={page}"
    )
}

/// Abonnements de l'Atelier d'un jeu, lus sur le site communautaire.
///
/// Le fichier `userdata/<compte>/ugc/<appid>_subscriptions.vdf` n'est écrit
/// qu'au moment où le client Steam voit l'abonnement. Un article suivi depuis
/// la fenêtre web de Lapinux peut donc manquer au rapport local ; cette page,
/// elle, suit le compte. Les identifiants sont fusionnés par l'appelant, qui
/// garde ainsi ce que la lecture locale connaît déjà.
pub fn workshop_subscriptions(
    account: &str,
    refresh_token: &str,
    steam_id: u64,
    appid: u32,
) -> Result<Vec<u64>, String> {
    let token = access_token(account, refresh_token, steam_id)?;
    let cookies = cookies_from_access_token(steam_id, &token);
    crate::shared::block_on(async move {
        let mut ids: Vec<u64> = Vec::new();
        for page in 1..=WORKSHOP_MAX_PAGES {
            let url = workshop_subscriptions_url(steam_id, appid, page);
            let body = fetch_workshop_page(&cookies, &url).await?;
            let found = parse_workshop_subscriptions(&body);
            if found.is_empty() {
                // Une session que le site ne reconnaît pas rend la page de
                // connexion : le dire vaut mieux qu'un atelier vide.
                if page == 1 && workshop_page_signed_out(&body) {
                    return Err(
                        "le site communautaire n'a pas reconnu la session Steam".to_string()
                    );
                }
                break;
            }
            let before = ids.len();
            for id in found {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
            // Une page sans nouveau contenu est la dernière : Steam la répète
            // quand `p` dépasse le découpage.
            if ids.len() == before {
                break;
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        tracing::info!("atelier {appid} : {} abonnement(s) lus en ligne", ids.len());
        Ok(ids)
    })
}

/// Lit une page communautaire avec réessai : Edge refuse parfois la première
/// demande et la suivante passe.
async fn fetch_workshop_page(cookies: &WebCookies, url: &str) -> Result<String, String> {
    let mut last = String::new();
    for attempt in 0..3 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(1200)).await;
        }
        match fetch_account_page(cookies, url).await {
            Ok(page) if page.status == 200 => return Ok(page.body),
            Ok(page) => last = format!("page des abonnements : {}", page.status),
            Err(error) => last = error,
        }
    }
    Err(last)
}

/// Identifiants des contenus d'une page d'abonnements. Les cartes portent un
/// `id="sharedfile_<id>"` ; le lien de la carte sert de repli quand Steam
/// change l'attribut.
pub fn parse_workshop_subscriptions(body: &str) -> Vec<u64> {
    let cards = ids_after(body, "id=\"sharedfile_");
    if !cards.is_empty() {
        return cards;
    }
    ids_after(body, "filedetails/?id=")
}

fn ids_after(body: &str, marker: &str) -> Vec<u64> {
    let mut ids = Vec::new();
    for (at, _) in body.match_indices(marker) {
        let rest = &body[at + marker.len()..];
        let digits: String = rest
            .chars()
            .take_while(|character| character.is_ascii_digit())
            .collect();
        if let Ok(id) = digits.parse::<u64>() {
            if id > 0 && !ids.contains(&id) {
                ids.push(id);
            }
        }
    }
    ids
}

/// La page est-elle la page de connexion, faute de session reconnue ?
fn workshop_page_signed_out(body: &str) -> bool {
    body.contains("steamcommunity.com/login/home/") || body.contains("g_steamID = false")
}

async fn fetch_account_page(cookies: &WebCookies, url: &str) -> Result<LicensesPage, String> {
    let client = client()?;
    let response = client
        .get(url)
        .header("cookie", cookies.header())
        // Sans cette entête, une réponse compressée arriverait telle
        // quelle : le client HTTP ne décompresse que s'il l'a demandée.
        .header("accept-encoding", "identity")
        .header("accept-language", "fr-FR,fr;q=0.9")
        .header("user-agent", USER_AGENT)
        .send()
        .await
        .map_err(|error| format!("boutique injoignable : {error}"))?;
    let status = response.status().as_u16();
    // Le même hôte que la boutique : un 403 ou un 429 compte pour la porte
    // anti-rafale, et trois refus d'affilée arrêtent aussi la pagination.
    if status == 403 || status == 429 {
        crate::storegate::refused(crate::storegate::retry_after(&response));
    } else if status == 200 {
        crate::storegate::accepted();
    }
    let body = response
        .text()
        .await
        .map_err(|error| format!("page illisible : {error}"))?;
    Ok(LicensesPage { status, body })
}

/// Une ligne de la page des licences : le nom publié par Steam, le type de
/// licence, et la date d'achat quand elle est lisible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LicenseRow {
    pub name: String,
    pub kind: String,
    /// Date d'achat, minuit UTC du jour annoncé (secondes Unix).
    pub acquired: Option<i64>,
}

/// Lit les lignes de la page des licences.
///
/// Le tableau ne porte aucun appid, et l'ordre des colonnes dépend de la
/// langue : la date est donc cherchée dans chaque cellule plutôt que prise à un
/// rang fixe. Une licence gratuite (« Gratuit », « Complimentary ») garde sa
/// date vide.
pub fn parse_licenses(body: &str) -> Vec<LicenseRow> {
    let mut rows = Vec::new();
    for chunk in body.split("<tr").skip(1) {
        let cells: Vec<(String, String)> = chunk
            .split("<td")
            .skip(1)
            .map(|cell| (cell_attributes(cell), cell_text(cell)))
            .filter(|(_, text)| !text.is_empty())
            .collect();
        if cells.is_empty() {
            continue;
        }
        // Les classes du tableau disent à quelle colonne on a affaire ; la date
        // n'est donc pas cherchée à un rang fixe, qui change avec la langue.
        let mut name = String::new();
        let mut kind = String::new();
        let mut acquired = None;
        for (attributes, text) in &cells {
            if attributes.contains("license_date") {
                acquired = parse_date(text);
            } else if attributes.contains("license_acquisition") {
                kind = text.clone();
            } else if name.is_empty() {
                name = text.clone();
            }
        }
        // Sans colonne nommée, la date se reconnaît à son contenu, et le type
        // de licence est la cellule qui reste.
        if acquired.is_none() {
            acquired = cells.iter().find_map(|(_, text)| parse_date(text));
            if kind.is_empty() {
                kind = cells
                    .iter()
                    .map(|(_, text)| text)
                    .find(|text| **text != name && parse_date(text).is_none())
                    .cloned()
                    .unwrap_or_default();
            }
        }
        if name.is_empty() {
            continue;
        }
        // L'en-tête du tableau n'est pas une licence.
        let lowered = name.to_lowercase();
        if lowered.contains("produit") || lowered.contains("product") {
            continue;
        }
        rows.push(LicenseRow {
            name,
            kind,
            acquired,
        });
    }
    rows
}

/// Attributs d'une cellule : le fragment qui précède son `>` d'ouverture.
fn cell_attributes(cell: &str) -> String {
    cell.split('>')
        .next()
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// Texte d'une cellule : attributs écartés, balises retirées, entités défaites,
/// espaces serrés.
fn cell_text(cell: &str) -> String {
    // La découpe sur `<td` a déjà mangé le chevron ouvrant : les attributs sont
    // donc en tête de la cellule, jusqu'au premier `>`.
    let start = cell.find('>').map(|at| at + 1).unwrap_or(0);
    let rest = &cell[start..];
    let end = rest.find("</td>").unwrap_or(rest.len());
    let mut out = String::new();
    let mut inside = false;
    for character in rest[..end].chars() {
        match character {
            '<' => inside = true,
            '>' => inside = false,
            other if !inside => out.push(other),
            _ => {}
        }
    }
    let out = out
        .replace("&amp;", "&")
        .replace("&#39;", "'")
        .replace("&quot;", "\"")
        .replace("&nbsp;", " ");
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Date d'une cellule : « 22 nov. 2014 », « 22 novembre 2014 »,
/// « Nov 22, 2014 ». Rend midi UTC, pour qu'aucun fuseau ne la déplace d'un jour.
fn parse_date(cell: &str) -> Option<i64> {
    let mut day = None;
    let mut month = None;
    let mut year = None;
    for word in cell.split(|character: char| !character.is_alphanumeric()) {
        if word.is_empty() {
            continue;
        }
        if word.len() == 4 && word.chars().all(|character| character.is_ascii_digit()) {
            let value: i64 = word.parse().ok()?;
            if (1990..=2100).contains(&value) {
                year = Some(value);
                continue;
            }
        }
        if month.is_none() {
            if let Some(found) = month_number(word) {
                month = Some(found);
                continue;
            }
        }
        if day.is_none() && word.chars().all(|character| character.is_ascii_digit()) {
            let value: i64 = word.parse().ok()?;
            if (1..=31).contains(&value) {
                day = Some(value);
            }
        }
    }
    let (day, month, year) = (day?, month?, year?);
    Some((days_from_civil(year, month, day) * 86_400) + 43_200)
}

/// Numéro du mois, en français ou en anglais, abrégé ou entier.
fn month_number(word: &str) -> Option<i64> {
    let lowered: String = word
        .chars()
        .map(|character| match character {
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'û' | 'ù' | 'ü' => 'u',
            'î' | 'ï' => 'i',
            'ô' | 'ö' => 'o',
            'à' | 'â' | 'ä' => 'a',
            'ç' => 'c',
            other => other.to_ascii_lowercase(),
        })
        .collect();
    // Français d'abord : « août » devient « aout », qui ne commence pas par le
    // « aug » anglais.
    const FRENCH: [&str; 12] = [
        "janv", "fevr", "mars", "avr", "mai", "juin", "juil", "aout", "sept", "oct", "nov", "dec",
    ];
    const ENGLISH: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    for months in [&FRENCH, &ENGLISH] {
        for (index, prefix) in months.iter().enumerate() {
            if lowered.starts_with(prefix) {
                return Some(index as i64 + 1);
            }
        }
    }
    None
}

/// Jours depuis l'origine Unix, calendrier grégorien (algorithme civil).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let shifted_month = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Un client HTTP pour le site : les redirections sont suivies ici, sauf pour
/// les transferts de cookies, qui doivent être lus sur place.
fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|error| format!("client HTTP : {error}"))
}

const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) Lapinux";

async fn exchange(refresh_token: &str) -> Result<(WebCookies, Vec<String>), String> {
    let client = client()?;
    let sessionid = session_id();
    let mut steps = Vec::new();
    let response = client
        .post(FINALIZE_URL)
        .header("accept-encoding", "identity")
        .header("user-agent", USER_AGENT)
        // Le site associe le transfert à cette session : sans ce cookie, la
        // réponse ne pose rien.
        .header("cookie", format!("sessionid={sessionid}"))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(form_encode(&[
            ("nonce".to_string(), refresh_token.to_string()),
            ("sessionid".to_string(), sessionid.clone()),
            ("redir".to_string(), REDIRECT.to_string()),
        ]))
        .send()
        .await
        .map_err(|error| format!("login.steampowered.com injoignable : {error}"))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| format!("réponse illisible : {error}"))?;
    steps.push(format!("finalizelogin → {status}"));
    if !status.is_success() {
        return Err(format!("finalizelogin a répondu {status}"));
    }
    let value: serde_json::Value = serde_json::from_str(&body)
        .map_err(|_| "réponse de finalizelogin illisible".to_string())?;
    let transfers = value
        .get("transfer_info")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    if value.get("success").and_then(serde_json::Value::as_bool) == Some(false) {
        steps.push("Steam a refusé l'échange".to_string());
    }
    steps.push(format!("transferts : {}", transfers.len()));
    if transfers.is_empty() {
        return Err("Steam n'a rendu aucun transfert".to_string());
    }
    let mut cookies = WebCookies::default();
    for transfer in transfers {
        let Some(url) = transfer.get("url").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let form = transfer_form(transfer.get("params"));
        let Ok(answer) = client
            .post(url)
            .header("accept-encoding", "identity")
            .header("user-agent", USER_AGENT)
            .header("cookie", format!("sessionid={sessionid}"))
            .header("content-type", "application/x-www-form-urlencoded")
            .body(form_encode(&form))
            .send()
            .await
        else {
            steps.push(format!("{} → injoignable", host_of(url)));
            continue;
        };
        let status = answer.status();
        let mut names = Vec::new();
        for header in answer.headers().get_all(reqwest::header::SET_COOKIE).iter() {
            if let Ok(text) = header.to_str() {
                if let Some((name, _)) = text.split_once('=') {
                    names.push(name.trim().to_string());
                }
                collect_cookie(&mut cookies, text);
            }
        }
        let keys: Vec<&str> = form.iter().map(|(name, _)| name.as_str()).collect();
        let received = if names.is_empty() {
            "aucun".to_string()
        } else {
            names.join(", ")
        };
        steps.push(format!(
            "{} → {status}, {} paramètre(s) : {}, cookie(s) : {received}",
            host_of(url),
            form.len(),
            keys.join(", "),
        ));
    }
    Ok((cookies, steps))
}

/// Nom d'hôte d'une adresse, pour un journal lisible.
fn host_of(url: &str) -> &str {
    let rest = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    rest.split('/').next().unwrap_or(rest)
}

/// Paramètres signés d'un transfert, mis à plat pour un formulaire.
fn transfer_form(params: Option<&serde_json::Value>) -> Vec<(String, String)> {
    let Some(map) = params.and_then(serde_json::Value::as_object) else {
        return Vec::new();
    };
    map.iter()
        .filter_map(|(key, value)| {
            let text = match value {
                serde_json::Value::String(text) => text.clone(),
                serde_json::Value::Number(number) => number.to_string(),
                serde_json::Value::Bool(flag) => flag.to_string(),
                _ => return None,
            };
            Some((key.clone(), text))
        })
        .collect()
}

/// Un en-tête `Set-Cookie` : `nom=valeur; chemin; attributs`.
fn collect_cookie(cookies: &mut WebCookies, header: &str) {
    let Some(pair) = header.split(';').next() else {
        return;
    };
    let Some((name, value)) = pair.split_once('=') else {
        return;
    };
    let name = name.trim();
    let value = value.trim().to_string();
    if name.is_empty() || value.is_empty() {
        return;
    }
    match name {
        "steamLoginSecure" => cookies.steam_login_secure = value,
        "sessionid" => cookies.sessionid = value,
        "browserid"
        | "steamCountry"
        | "timezoneOffset"
        | "steamRememberLogin"
        | "mobileClientVersion"
        | "mobileClient" => {
            cookies.extra.push((name.to_string(), value));
        }
        _ => {}
    }
}

/// Corps `application/x-www-form-urlencoded`. Le client HTTP n'embarque pas le
/// sérialiseur de formulaires : l'encodage est fait ici, à la main.
fn form_encode(fields: &[(String, String)]) -> String {
    let mut out = String::new();
    for (name, value) in fields {
        if !out.is_empty() {
            out.push('&');
        }
        percent_encode(name, &mut out);
        out.push('=');
        percent_encode(value, &mut out);
    }
    out
}

fn percent_encode(value: &str, out: &mut String) {
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
}

/// Identifiant de session du site : vingt-quatre caractères hexadécimaux, comme
/// le navigateur les fabrique. Il n'a pas besoin d'être cryptographique — Steam
/// le relit seulement dans le formulaire et le cookie.
fn session_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos() as u64)
        .unwrap_or(0x9E37_79B9_7F4A_7C15);
    let mut seed = nanos ^ (u64::from(std::process::id()) << 32);
    let mut out = String::with_capacity(24);
    for _ in 0..24 {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        out.push(char::from_digit((seed % 16) as u32, 16).unwrap_or('0'));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cookie_header_carries_the_badge_and_the_session() {
        let cookies = WebCookies {
            steam_login_secure: "7656119||jeton".into(),
            sessionid: "abc".into(),
            extra: vec![("browserid".into(), "42".into())],
        };
        assert!(cookies.is_signed_in());
        assert_eq!(
            cookies.header(),
            "steamLoginSecure=7656119||jeton; sessionid=abc; browserid=42"
        );
    }

    #[test]
    fn cookies_are_read_from_set_cookie_headers() {
        let mut cookies = WebCookies::default();
        collect_cookie(
            &mut cookies,
            "steamLoginSecure=1||x; Path=/; Secure; HttpOnly",
        );
        collect_cookie(&mut cookies, "sessionid=deadbeef; Path=/");
        collect_cookie(
            &mut cookies,
            "browserid=123; expires=Sat, 01 Nov 2026 00:00:00 GMT",
        );
        collect_cookie(&mut cookies, "garbage");
        assert_eq!(cookies.steam_login_secure, "1||x");
        assert_eq!(cookies.sessionid, "deadbeef");
        assert_eq!(
            cookies.extra,
            vec![("browserid".to_string(), "123".to_string())]
        );
        assert!(cookies.is_signed_in());
    }

    #[test]
    fn a_session_id_looks_like_the_browsers_one() {
        let id = session_id();
        assert_eq!(id.len(), 24);
        assert!(id.chars().all(|character| character.is_ascii_hexdigit()));
        assert_ne!(id, session_id());
    }

    #[test]
    fn a_promotion_package_is_the_free_one() {
        let data = serde_json::json!({
            "is_free": false,
            "package_groups": [{
                "subs": [
                    {"packageid": 123, "is_free_license": false, "price_in_cents_with_discount": 1999},
                    {"packageid": 456, "is_free_license": true, "price_in_cents_with_discount": 0}
                ]
            }]
        });
        assert_eq!(free_package(&data), Some(456));
        assert_eq!(free_package(&serde_json::json!({})), None);
    }

    #[test]
    fn a_licence_row_carries_its_date() {
        let body = r#"
            <table><tr><td>Nom du produit</td><td>Type</td><td>Date d'achat</td></tr>
            <tr><td><a href="/x">Insurgency</a></td><td>Suite</td><td>22 nov. 2014</td></tr>
            <tr><td>Free Game</td><td>Gratuit</td><td>Gratuit</td></tr>
            </table>"#;
        let rows = parse_licenses(body);
        assert_eq!(rows.len(), 2, "l'en-tête ne compte pas");
        assert_eq!(rows[0].name, "Insurgency");
        assert_eq!(rows[0].kind, "Suite");
        assert_eq!(rows[0].acquired, parse_date("22 nov. 2014"));
        assert!(rows[0].acquired.is_some());
        assert_eq!(rows[1].acquired, None);
    }

    #[test]
    fn a_region_suffix_still_matches_the_game() {
        assert!(same_title("Insurgency (WW)", "Insurgency"));
        assert!(same_title("Portal 2", "Portal 2"));
        assert!(!same_title("Portal 2", "Portal"));
        assert!(!same_title("Portal", "Portal 2"));
        assert!(!same_title("", "Portal"));
    }

    #[test]
    fn a_retail_suffix_still_matches_the_game() {
        // Les paquets de clés revendeur portent une région *et* une mention de
        // vente : le jeu, lui, n'a ni l'une ni l'autre.
        assert!(same_title(
            "Among the Sleep (Global) - Retail",
            "Among the Sleep"
        ));
        assert!(same_title("Insurgency (WW) - Retail", "Insurgency"));
        assert!(same_title("Darkwater (Global)", "Darkwater"));
        // Un tiret qui appartient au titre n'est pas une étiquette.
        assert!(same_title("Half-Life 2", "Half-Life 2"));
        assert!(!same_title("Half-Life 2 - Retail", "Half-Life"));
        assert!(!same_title("Portal 2 (WW) - Retail", "Portal"));
    }

    #[test]
    fn dates_are_read_in_french_and_english() {
        let french = parse_date("1 août 2019").unwrap();
        let english = parse_date("Aug 1, 2019").unwrap();
        assert_eq!(french, english);
        assert_eq!(french, days_from_civil(2019, 8, 1) * 86_400 + 43_200);
        // Un nom de jeu n'est pas une date, et une date sans année n'en est pas
        // une non plus.
        assert_eq!(parse_date("Football Manager 2019"), None);
        assert_eq!(parse_date("22 nov."), None);
    }

    #[test]
    fn the_history_reads_a_transaction_date_and_its_appids() {
        let body = r#"<table><tr>
            <td class="history_date">22 nov. 2014</td>
            <td><a href="https://store.steampowered.com/app/220/">Half-Life 2</a>
                <a href="https://store.steampowered.com/app/320/">Portal</a></td>
            <td>Achat</td>
        </tr></table>"#;
        let purchases = parse_history(body);
        assert_eq!(purchases.len(), 1);
        assert_eq!(purchases[0].appids, vec![220, 320]);
        assert_eq!(purchases[0].acquired, parse_date("22 nov. 2014").unwrap());
    }

    #[test]
    fn a_wallet_transaction_names_no_appid() {
        let body = "<tr><td>3 janv. 2015</td><td>Portefeuille</td><td>+5,00 €</td></tr>";
        let purchases = parse_history(body);
        assert_eq!(purchases.len(), 1);
        assert!(purchases[0].appids.is_empty());
    }

    #[test]
    fn the_next_page_comes_from_the_paginator() {
        // Le lien réel, tel que Steam l'écrit : HTML échappé, jeton `x` inclus.
        let body = r#"<div class="license_paginator"><span>1-100 sur 1991</span>
            <a class="license_paginator_next" href="?x=1789465284:388027149365842018&amp;offset=100">Suivante</a></div>"#;
        assert_eq!(
            next_page(body).as_deref(),
            Some("?x=1789465284:388027149365842018&offset=100")
        );
        assert_eq!(
            license_page_url(&next_page(body).unwrap()),
            "https://store.steampowered.com/account/licenses/?x=1789465284:388027149365842018&offset=100"
        );
        // La dernière page n'a pas de lien : la lecture s'arrête là.
        assert_eq!(
            next_page(r#"<div class="license_paginator">1845-1991 sur 1991</div>"#),
            None
        );
    }

    #[test]
    fn a_form_body_encodes_what_the_site_sends() {
        let body = form_encode(&[
            ("nonce".into(), "1||a+b".into()),
            (
                "redir".into(),
                "https://steamcommunity.com/login/home/?goto=".into(),
            ),
        ]);
        assert_eq!(
            body,
            "nonce=1%7C%7Ca%2Bb&redir=https%3A%2F%2Fsteamcommunity.com%2Flogin%2Fhome%2F%3Fgoto%3D"
        );
        assert_eq!(form_encode(&[("q".into(), "a b".into())]), "q=a+b");
    }

    #[test]
    fn transfer_params_flatten_strings_and_numbers() {
        let params = serde_json::json!({ "nonce": "n", "auth": "a", "steamID": 7656119u64 });
        let form = transfer_form(Some(&params));
        assert!(form.contains(&("nonce".to_string(), "n".to_string())));
        assert!(form.contains(&("steamID".to_string(), "7656119".to_string())));
        assert!(transfer_form(None).is_empty());
    }

    #[test]
    fn workshop_cards_give_their_published_ids() {
        let body = r#"<div id="sharedfile_3767639280" class="workshopItemPreviewHolder">
            <a href="https://steamcommunity.com/sharedfiles/filedetails/?id=3767639280">x</a>
            </div>
            <div id="sharedfile_3812321265" class="workshopItemPreviewHolder"></div>
            <div id="sharedfile_3767639280"></div>"#;
        assert_eq!(
            parse_workshop_subscriptions(body),
            vec![3767639280, 3812321265]
        );
        // Repli : plus de cartes nommées, seuls les liens restent.
        let links = r#"<a href="https://steamcommunity.com/sharedfiles/filedetails/?id=42">x</a>"#;
        assert_eq!(parse_workshop_subscriptions(links), vec![42]);
        assert!(parse_workshop_subscriptions("<p>aucun abonnement</p>").is_empty());
    }
}
