//! Catalogue Steam, famille et boutique.

use super::*;

/// Jeton d'accès Web API de la session, demandé au CM à partir du jeton de
/// refresh. Steam renvoie parfois un jeton de refresh tourné : il est aussitôt
/// enregistré, sinon la session enregistrée finirait par ne plus s'ouvrir.
pub(crate) async fn web_access_token(
    client: &SteamClient<LoggedIn>,
    steam_id: u64,
    refresh_token: &str,
) -> Option<String> {
    let mut request = Vec::new();
    proto::field_bytes(1, refresh_token.as_bytes(), &mut request);
    proto::field_fixed64(2, steam_id, &mut request);
    // `k_ETokenRenewalType_Allow` : le jeton rendu reste utilisable par un
    // client de bureau.
    proto::field_varint(3, 1, &mut request);
    let response = client
        .call_service_method("Authentication.GenerateAccessTokenForApp#1", &request)
        .await
        .map_err(|error| tracing::debug!("catalogue : jeton d'accès refusé ({error})"))
        .ok()?;
    let mut access = None;
    let mut renewed = None;
    let mut reader = proto::Reader::new(&response.body);
    while let Some((number, field)) = reader.next() {
        if let proto::Field::Bytes(value) = field {
            match number {
                1 => access = String::from_utf8(value.to_vec()).ok(),
                2 => renewed = String::from_utf8(value.to_vec()).ok(),
                _ => {}
            }
        }
    }
    if let Some(renewed) = renewed.filter(|token| !token.trim().is_empty()) {
        if renewed != refresh_token {
            if let Some(mut saved) = crate::session::load() {
                saved.refresh_token = renewed;
                match crate::session::save(&saved) {
                    Ok(path) => tracing::info!("jeton Steam entretenu ({})", path.display()),
                    Err(error) => tracing::warn!("jeton Steam non enregistré : {error}"),
                }
            }
        }
    }
    access.filter(|token| !token.trim().is_empty())
}

/// Identifiant du groupe familial du compte, lu par un appel de service CM.
async fn family_group_id(client: &SteamClient<LoggedIn>, steam_id: u64) -> Option<u64> {
    let mut request = Vec::new();
    proto::field_varint(1, steam_id, &mut request);
    proto::field_varint(2, 1, &mut request);
    let response = client
        .call_service_method("FamilyGroups.GetFamilyGroupForUser#1", &request)
        .await
        .map_err(|error| tracing::debug!("catalogue : groupe familial indisponible ({error})"))
        .ok()?;
    let mut group = None;
    let mut reader = proto::Reader::new(&response.body);
    while let Some((number, field)) = reader.next() {
        if let (1, proto::Field::Varint(value)) = (number, field) {
            group = Some(value);
            break;
        }
    }
    group.filter(|group| *group > 0)
}

/// Jeux que le groupe familial partage, appid, nom et date d'obtention
/// (`rt_time_acquired`) compris.
///
/// La liste de licences ne suffit pas : certains jeux du groupe n'y figurent
/// jamais, et ils manquaient donc entièrement à la bibliothèque. Ce service
/// rend tout le groupe ; les fiches (poids, build) se demandent ensuite à PICS
/// et à la boutique comme pour les autres jeux.
async fn family_shared_apps(
    client: &SteamClient<LoggedIn>,
    steam_id: u64,
    refresh_token: &str,
) -> (
    Vec<(u32, String, Option<i64>)>,
    Vec<crate::catalog::FamilyDlc>,
) {
    // Le groupe d'abord : sans famille, aucun jeton d'accès n'est demandé au
    // compte, et la synchronisation garde son coût habituel.
    let Some(group) = family_group_id(client, steam_id).await else {
        return (Vec::new(), Vec::new());
    };
    let Some(access) = web_access_token(client, steam_id, refresh_token).await else {
        return (Vec::new(), Vec::new());
    };
    // `include_own` fait entrer le compte connecté dans la réponse ;
    // `include_non_games` y ajoute les contenus additionnels, qui portent eux
    // aussi leurs propriétaires. Sans cette seconde option, la fiche ne
    // connaîtrait que les DLC du compte et pas ceux des autres membres.
    let url = format!(
        "{FAMILY_SHARED_URL}?access_token={access}&family_groupid={group}&include_own=true&include_non_games=true&max_apps=20000"
    );
    let Ok(response) = reqwest::Client::new()
        .get(&url)
        // Sans cette entête, une réponse compressée arriverait telle quelle :
        // le client HTTP ne décompresse que s'il l'a demandée.
        .header("accept-encoding", "identity")
        .timeout(STORE_TIMEOUT)
        .send()
        .await
    else {
        return (Vec::new(), Vec::new());
    };
    let status = response.status();
    let Ok(body) = response.text().await else {
        return (Vec::new(), Vec::new());
    };
    if !status.is_success() {
        tracing::debug!(
            "catalogue : bibliothèque familiale refusée ({status}, {} octets)",
            body.len()
        );
        return (Vec::new(), Vec::new());
    }
    let games = parse_shared_apps(&body);
    let dlcs = parse_shared_dlcs(&body);
    // Ce compte rendu distingue une réponse sans contenu d'un parseur qui les
    // aurait laissés passer : les types viennent directement de Steam.
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&body) {
        let apps = value
            .get("response")
            .and_then(|response| response.get("apps"))
            .and_then(serde_json::Value::as_array);
        if let Some(apps) = apps {
            let mut types: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
            let mut with_owners = 0usize;
            for app in apps {
                let kind = app
                    .get("app_type")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(1);
                *types.entry(kind).or_default() += 1;
                if app.get("owner_steamids").is_some() {
                    with_owners += 1;
                }
            }
            tracing::info!(
                "catalogue : partage familial {} applications, {} jeux, {} DLC, types {:?}, propriétaires {}",
                apps.len(),
                games.len(),
                dlcs.len(),
                types,
                with_owners
            );
        }
    }
    (games, dlcs)
}

/// Applications d'une réponse `GetSharedLibraryApps` : `appid`, nom, et date
/// d'obtention en secondes Unix quand Steam la publie.
///
/// `app_type` 1 désigne un jeu ; les démos, outils et contenus additionnels du
/// groupe n'ont pas leur place dans la bibliothèque. `exclude_reason` non nul
/// signale une application que le partage familial écarte : elle n'est pas
/// accessible au compte.
pub(super) fn parse_shared_apps(text: &str) -> Vec<(u32, String, Option<i64>)> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return Vec::new();
    };
    let Some(apps) = value
        .get("response")
        .and_then(|response| response.get("apps"))
        .and_then(serde_json::Value::as_array)
    else {
        return Vec::new();
    };
    apps.iter()
        .filter_map(|app| {
            let appid = app.get("appid").and_then(serde_json::Value::as_u64)? as u32;
            let kind = app
                .get("app_type")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(1);
            if kind != 1 {
                return None;
            }
            let excluded = app
                .get("exclude_reason")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            if excluded != 0 {
                return None;
            }
            let name = app
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_string();
            // La réponse famille date chaque application à l'achat, ce que la
            // liste de licences ne fait que pour les jeux du compte.
            let acquired = app
                .get("rt_time_acquired")
                .and_then(serde_json::Value::as_i64)
                .filter(|stamp| *stamp > 0);
            Some((appid, name, acquired))
        })
        .collect()
}

/// Contenus additionnels d'une réponse `GetSharedLibraryApps`. Steam ne les
/// publie pas aujourd'hui (le drapeau `EAppType` d'un DLC vaut 32, absent des
/// réponses) ; ce parseur reste pour le jour où le service les rendrait, avec
/// la même liste de propriétaires que les jeux.
pub(super) fn parse_shared_dlcs(text: &str) -> Vec<crate::catalog::FamilyDlc> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return Vec::new();
    };
    let Some(apps) = value
        .get("response")
        .and_then(|response| response.get("apps"))
        .and_then(serde_json::Value::as_array)
    else {
        return Vec::new();
    };
    let id = |value: &serde_json::Value| {
        value
            .as_u64()
            .or_else(|| value.as_str().and_then(|id| id.parse().ok()))
    };
    apps.iter()
        .filter_map(|app| {
            let appid = app.get("appid").and_then(serde_json::Value::as_u64)? as u32;
            // `EAppType` est un jeu de drapeaux : 32 désigne un contenu
            // additionnel. Le type 4 recouvre aussi des outils (Authoring
            // Tools, SDK) : les accepter ferait passer de faux contenus pour
            // des DLC de la famille.
            if app
                .get("app_type")
                .and_then(serde_json::Value::as_u64)
                .map(|kind| kind & 32 == 0)
                .unwrap_or(true)
            {
                return None;
            }
            if app
                .get("exclude_reason")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0)
                != 0
            {
                return None;
            }
            let name = app
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_string();
            if name.is_empty() {
                return None;
            }
            let owners = app
                .get("owner_steamids")
                .and_then(serde_json::Value::as_array)
                .map(|ids| ids.iter().filter_map(id).collect())
                .unwrap_or_default();
            Some(crate::catalog::FamilyDlc {
                appid,
                name,
                owners,
            })
        })
        .collect()
}

/// Membres de la famille qui possèdent une application, d'après les paquets de
/// licence qui la portent. Un paquet partagé par deux membres cite les deux ;
/// l'appelant écarte les applications que le compte possède en propre.
pub(super) fn family_owners(
    appid: u32,
    app_packages: &HashMap<u32, Vec<u32>>,
    family_packages: &HashMap<u32, Vec<u64>>,
) -> Vec<u64> {
    let mut owners: Vec<u64> = app_packages
        .get(&appid)
        .into_iter()
        .flatten()
        .filter_map(|package| family_packages.get(package))
        .flatten()
        .copied()
        .collect();
    owners.sort_unstable();
    owners.dedup();
    owners
}

/// Classe un jeu rendu par `owned_games` : le service dit s'il est possédé ou
/// seulement prêté. Un appid déjà connu par un paquet familial peut ainsi
/// rejoindre les jeux possédés, et la fiche cesse de l'annoncer comme prêté.
fn note_owned_game(
    appid: u32,
    family_shared: bool,
    owned_appids: &mut HashSet<u32>,
    family_appids: &mut HashSet<u32>,
) {
    if family_shared {
        family_appids.insert(appid);
    } else {
        owned_appids.insert(appid);
    }
}

/// Lit les noms et types des applications possédées. `packages` vient de la
/// liste de licences rendue par la session CM Lapinux : chaque paquet est
/// étendu en appids par PICS, puis les applications sont nommées. Les paquets
/// de `family_packages` appartiennent à un autre membre de la famille Steam.
/// Aucun SteamCMD n'intervient.
///
/// PICS ne décrit pas tous les paquets : ceux d'un jeu partagé par une famille
/// Steam répondent souvent par une fiche vide, et l'application elle-même peut
/// refuser sa fiche au compte qui ne la possède pas. La boutique publique de
/// Steam, qui ne demande ni clé ni compte, complète alors la liste : sans elle
/// ces jeux n'apparaissaient jamais dans la bibliothèque.
pub fn catalog(
    account: &str,
    refresh_token: &str,
    steam_id: u64,
    packages: &[u32],
    family_packages: &HashMap<u32, Vec<u64>>,
    package_dates: &HashMap<u32, i64>,
    site_dates: &[(String, i64)],
) -> Result<(
    Vec<Entry>,
    Vec<crate::catalog::DlcEntry>,
    HashMap<u32, u32>,
    Vec<u32>,
    Vec<crate::catalog::FamilyDlc>,
)> {
    if packages.is_empty() {
        return Ok((
            Vec::new(),
            Vec::new(),
            HashMap::new(),
            Vec::new(),
            Vec::new(),
        ));
    }
    // Moteur commun : la synchronisation emprunte la même session que les
    // fiches et les succès, et cette session lui survit.
    crate::shared::block_on(async move {
        // Session partagée : la synchronisation ne doit pas ouvrir la sienne
        // pendant qu'une autre lecture tient celle du compte.
        let client = crate::shared::session(account, refresh_token).await?;
        let mut appids = Vec::new();
        let mut family_appids: HashSet<u32> = HashSet::new();
        let mut owned_appids: HashSet<u32> = HashSet::new();
        // Dépôts réellement couverts par les licences du compte, jeu par jeu :
        // les tailles PICS ne comptent pas les éditions régionales non
        // achetées. La table est indexée par appid, faute de quoi un jeu prêté
        // par la famille — dont le paquet n'est pas décrit par PICS — perdait
        // tous ses dépôts et n'annonçait aucun poids.
        let mut owned_depots: HashMap<u32, HashSet<u32>> = HashMap::new();
        // Budget de clés de dépôt demandées à cette synchronisation.
        let mut depot_budget = DEPOT_KEY_BUDGET;
        // Poids des contenus additionnels, lu dans les dépôts du jeu parent :
        // la fiche d'un contenu ne publie pas les siens, et sa taille restait
        // donc à zéro partout dans la bibliothèque.
        let mut dlc_sizes: HashMap<u32, lapinux_core::model::SteamSize> = HashMap::new();
        // Paquets de licences dont PICS n'a rien voulu dire. Les doublons sont
        // écartés : une même licence peut apparaître plusieurs fois.
        let mut undescribed: Vec<u32> = Vec::new();
        let mut described: HashSet<u32> = HashSet::new();
        // Jeu principal de chaque paquet de licence : c'est par lui que
        // l'historique des achats nomme une licence datée.
        let mut package_main: HashMap<u32, u32> = HashMap::new();
        // Paquets qui portent chaque application : un jeu acheté dans un lot
        // vient d'un paquet dont il n'est pas le premier appid, et sa date se
        // lit donc dans *toutes* ses licences, pas dans la principale.
        let mut app_packages: HashMap<u32, Vec<u32>> = HashMap::new();
        for info in package_infos(&client, packages).await? {
            let Some(package_id) = info.package_id.map(|id| id.0) else {
                continue;
            };
            let Some(bytes) = info.kv_data else {
                continue;
            };
            let package_apps = package_appids(&bytes);
            if package_apps.is_empty() {
                // Une fiche PICS sans aucun appid ne décrit rien : c'est le cas
                // des licences partagées par la famille. Le paquet repart donc
                // vers la boutique publique, seule à savoir quels jeux il porte.
                continue;
            }
            described.insert(package_id);
            if let Some(appid) = package_apps.first() {
                package_main.entry(package_id).or_insert(*appid);
            }
            let package_depots = package_section_ids(&bytes, "depotids");
            for appid in &package_apps {
                owned_depots
                    .entry(*appid)
                    .or_default()
                    .extend(package_depots.iter().copied());
                app_packages.entry(*appid).or_default().push(package_id);
            }
            if family_packages.contains_key(&package_id) {
                family_appids.extend(package_apps.iter().copied());
            } else {
                owned_appids.extend(package_apps.iter().copied());
            }
            appids.extend(package_apps);
        }
        for package in packages.iter().copied() {
            if !described.contains(&package) && !undescribed.contains(&package) {
                undescribed.push(package);
            }
        }
        // Les paquets de la famille passent devant : PICS les décrit le plus
        // mal, et un jeu partagé absent ne se voit pas dans la bibliothèque.
        undescribed.sort_by_key(|package| !family_packages.contains_key(package));
        let silent = undescribed.len();
        undescribed.truncate(STORE_PACKAGE_LOOKUPS);
        if !undescribed.is_empty() {
            tracing::info!(
                "catalogue : {} paquet(s) de licences sans fiche PICS, {} lus sur la boutique",
                silent,
                undescribed.len()
            );
            for (package, apps) in store_package_apps(&undescribed).await {
                let family = family_packages.contains_key(&package);
                if let Some((appid, _)) = apps.first() {
                    package_main.entry(package).or_insert(*appid);
                }
                for (appid, _) in apps {
                    app_packages.entry(appid).or_default().push(package);
                    // Un appid découvert par la boutique ne dit pas encore s'il
                    // s'agit d'un jeu ou d'un outil : son type se demande juste
                    // après, avec son nom.
                    if family {
                        family_appids.insert(appid);
                    } else {
                        owned_appids.insert(appid);
                    }
                    appids.push(appid);
                }
            }
        }
        // La page des licences du compte date des paquets, pas des jeux :
        // « Insurgency (WW) », « Among the Sleep (Global) - Retail ». PICS ne
        // nomme pas un paquet, la boutique publique le fait : c'est ce nom qui
        // relie une ligne datée du site aux jeux que son paquet porte. Sans
        // cela, un jeu acheté dans un lot restait sans date.
        let undated: Vec<u32> = packages
            .iter()
            .copied()
            .filter(|package| {
                !package_dates.contains_key(package) && !family_packages.contains_key(package)
            })
            .collect();
        let mut dates = package_dates.clone();
        if !site_dates.is_empty() && !undated.is_empty() {
            let names: HashMap<u32, String> =
                store_package_names(&undated).await.into_iter().collect();
            let dated = site_package_dates(site_dates, &names, package_dates);
            tracing::info!(
                "catalogue : {} paquet(s) sans date, {} nommé(s) par la boutique, {} daté(s) par la page des licences",
                undated.len(),
                names.len(),
                dated.len()
            );
            dates.extend(dated);
        }
        // Une licence datée date le jeu principal de son paquet : c'est la date
        // à laquelle Steam a ajouté le jeu au compte. Un jeu peut venir de
        // plusieurs paquets, le plus ancien gagne.
        let mut app_dates: HashMap<u32, i64> = HashMap::new();
        for (appid, packages) in &app_packages {
            let mut oldest = 0i64;
            for package in packages {
                let Some(date) = dates.get(package).copied() else {
                    continue;
                };
                if date > 0 && (oldest == 0 || date < oldest) {
                    oldest = date;
                }
            }
            if oldest > 0 {
                app_dates.insert(*appid, oldest);
            }
        }
        appids.sort_unstable();
        appids.dedup();

        // Les jeux du groupe familial que la liste de licences ne porte pas :
        // sans eux, un jeu prêté par la famille n'apparaissait pas du tout.
        // Leurs noms sont gardés de côté, au cas où ni PICS ni la boutique ne
        // sauraient les décrire.
        let mut family_names: HashMap<u32, String> = HashMap::new();
        let mut family_dates: HashMap<u32, i64> = HashMap::new();
        let (shared, mut family_dlcs) = family_shared_apps(&client, steam_id, refresh_token).await;
        if !shared.is_empty() {
            let known: HashSet<u32> = appids.iter().copied().collect();
            let mut added = 0usize;
            for (appid, name, acquired) in shared {
                // La date vaut aussi pour les jeux déjà venus par une licence
                // familiale : sans elle, ils restaient sans date d'obtention.
                if let Some(stamp) = acquired {
                    family_dates.entry(appid).or_insert(stamp);
                }
                if known.contains(&appid) || family_appids.contains(&appid) {
                    continue;
                }
                if added >= FAMILY_SHARED_LIMIT {
                    continue;
                }
                added += 1;
                family_appids.insert(appid);
                family_names.insert(appid, name);
                appids.push(appid);
            }
            tracing::info!("catalogue : {added} jeu(x) de la famille en plus des licences");
        }
        // Le service des jeux possédés, enfin : la liste de licences ne couvre
        // pas tout ce que le compte possède. Insurgency (222880) appartient au
        // compte, mais aucun de ses paquets ne sort de `licenses_print` — il
        // manquait donc à la bibliothèque, quel que soit le nombre de
        // synchronisations. Ce service nomme les appids sans les décrire ; les
        // fiches se demandent ensuite à PICS et à la boutique comme les autres.
        let group = crate::playtime::family_group_id(&client, steam_id).await;
        let mut from_account = 0usize;
        if let Ok(games) = crate::playtime::owned_games(&client, steam_id, group).await {
            let mut known: HashSet<u32> = appids.iter().copied().collect();
            known.extend(family_names.keys().copied());
            for (appid, play) in games {
                if appid == 0 {
                    continue;
                }
                // Même un appid déjà vu dans un paquet familial doit être
                // reclassé : `owned_games` sait s'il est possédé ou prêté, et
                // c'est cette réponse qui décide de la fiche.
                note_owned_game(
                    appid,
                    play.family_shared,
                    &mut owned_appids,
                    &mut family_appids,
                );
                if !known.insert(appid) {
                    continue;
                }
                appids.push(appid);
                from_account += 1;
            }
        }
        if from_account > 0 {
            tracing::info!(
                "catalogue : {from_account} jeu(x) du compte absent(s) des licences, ajoutés"
            );
        }
        // Le temps de jeu vient du protocole, pas du disque : un appel rend la
        // bibliothèque entière avec ses minutes, jeux jamais lancés compris.
        let mut playtimes: HashMap<u32, u64> = HashMap::new();
        if let Ok(owned) = crate::playtime::owned_games(&client, steam_id, None).await {
            for (appid, play) in owned {
                if play.minutes_total > 0 {
                    playtimes.insert(appid, play.minutes_total);
                }
            }
        }
        appids.sort_unstable();
        appids.dedup();

        // Un jeu prêté n'a pas de licence personnelle : Steam publie sa date
        // dans le service de famille, mais pas dans la liste de licences.
        let family_date = |appid: u32| -> Option<i64> {
            if family_appids.contains(&appid) && !owned_appids.contains(&appid) {
                family_dates.get(&appid).copied()
            } else {
                None
            }
        };
        let mut entries = Vec::new();
        // Applications dont PICS a réellement rendu une fiche : le journal
        // distingue une bibliothèque vide d'une réponse tronquée.
        let mut described = 0usize;
        // Contenus additionnels possédés : le catalogue les garde à part, pour
        // que la fiche d'un jeu sache ce qu'elle peut proposer d'installer.
        let mut dlcs = Vec::new();
        // Appids dont la fiche PICS est arrivée sans nom, et ceux dont aucune
        // fiche n'est revenue : à trancher par la boutique.
        let mut unresolved: Vec<u32> = Vec::new();
        // Appids pour lesquels PICS a répondu quelque chose, même un outil
        // qu'on écarte : eux n'ont plus besoin de la boutique.
        let mut returned: HashSet<u32> = HashSet::new();
        for chunk in appids.chunks(PICS_BATCH) {
            let ids: Vec<AppId> = chunk.iter().copied().map(AppId).collect();
            let tokens = client
                .pics_get_access_tokens(&ids)
                .await
                .map_err(steam_error)?;
            let tokens: Vec<AccessToken> = ids
                .iter()
                .map(|app_id| AccessToken {
                    app_id: *app_id,
                    token: tokens
                        .iter()
                        .find(|token| token.app_id == *app_id)
                        .map(|token| token.token)
                        .unwrap_or(0),
                })
                .collect();
            for info in app_infos(&client, &tokens).await? {
                let Some(appid) = info.app_id.map(|app_id| app_id.0) else {
                    continue;
                };
                let Some(bytes) = info.kv_data else {
                    unresolved.push(appid);
                    continue;
                };
                described += 1;
                let Some(root) = parse_kv(&bytes) else {
                    unresolved.push(appid);
                    continue;
                };
                returned.insert(appid);
                let meta = meta_from_pics(&root);
                if meta.name.trim().is_empty() {
                    // Fiche sans nom : PICS ne dit pas non plus de quel type
                    // d'application il s'agit. La boutique tranchera plus bas.
                    unresolved.push(appid);
                    continue;
                }
                if meta.kind.eq_ignore_ascii_case("game") {
                    dlc_sizes.extend(dlc_sizes_of(&root));
                    // Sans paquet décrit par PICS — jeu prêté par la famille —
                    // les éditions se départagent par le protocole : Steam ne
                    // rend la clé d'un dépôt qu'aux licences qui le couvrent.
                    let described = owned_depots
                        .get(&appid)
                        .is_some_and(|depots| !depots.is_empty());
                    let refused = if described {
                        None
                    } else {
                        refused_depots(&client, appid, &root, &mut depot_budget).await
                    };
                    entries.push(Entry {
                        appid,
                        name: meta.name,
                        kind: "Game".to_string(),
                        // Un jeu possédé directement n'est jamais marqué
                        // familial, même s'il figure aussi dans un paquet partagé.
                        family: family_appids.contains(&appid) && !owned_appids.contains(&appid),
                        details: details_from_pics(
                            &root,
                            appid,
                            owned_depots.get(&appid),
                            refused.as_ref(),
                            false,
                        ),
                        acquired: app_dates
                            .get(&appid)
                            .copied()
                            .or_else(|| family_date(appid)),
                        minutes: playtimes.get(&appid).copied(),
                    });
                } else if meta.kind.eq_ignore_ascii_case("demo") {
                    // Une démo est une application à part entière, avec son
                    // propre appid et ses propres dépôts. Elle reste dans le
                    // catalogue pour que l'interface puisse l'afficher ; le
                    // réglage de la bibliothèque décide ensuite de la voir.
                    entries.push(Entry {
                        appid,
                        name: meta.name,
                        kind: "Demo".to_string(),
                        family: family_appids.contains(&appid) && !owned_appids.contains(&appid),
                        details: details_from_pics(
                            &root,
                            appid,
                            owned_depots.get(&appid),
                            None,
                            false,
                        ),
                        acquired: app_dates
                            .get(&appid)
                            .copied()
                            .or_else(|| family_date(appid)),
                        minutes: playtimes.get(&appid).copied(),
                    });
                } else if meta.kind.eq_ignore_ascii_case("dlc") {
                    // Le contenu autonome (rare) publie ses dépôts ; les autres
                    // prennent leur poids dans ceux du jeu parent, ajouté plus
                    // bas quand la table est complète.
                    let size = dlc_size(&root);
                    let owners = family_owners(appid, &app_packages, family_packages);
                    if !owners.is_empty() && !owned_appids.contains(&appid) {
                        // Le paquet vient d'un autre membre : ce contenu se
                        // joue avec sa version du jeu, pas avec la nôtre.
                        family_dlcs.push(crate::catalog::FamilyDlc {
                            appid,
                            name: meta.name,
                            owners,
                        });
                    } else {
                        dlcs.push(crate::catalog::DlcEntry {
                            appid,
                            name: meta.name,
                            install: size.map(|size| size.install).unwrap_or(0),
                            download: size.map(|size| size.download).unwrap_or(0),
                        });
                    }
                }
            }
        }

        // Applications découvertes par les licences dont PICS n'a rien dit, ou
        // rien de nommable : la boutique publique les décrit, y compris celles
        // qu'un compte ne peut pas lire dans PICS — les jeux prêtés par la
        // famille Steam, par exemple. Elle seule sait si l'appid est un jeu,
        // un contenu additionnel ou un outil.
        for appid in appids.iter().copied() {
            if !returned.contains(&appid) {
                unresolved.push(appid);
            }
        }
        let mut missing = unresolved;
        missing.sort_unstable();
        missing.dedup();
        // Les applications partagées par la famille d'abord : ce sont celles
        // que PICS refuse le plus souvent de décrire.
        missing.sort_by_key(|appid| !family_appids.contains(appid));
        if missing.len() > STORE_APP_LOOKUPS {
            tracing::info!(
                "catalogue : {} applications sans fiche PICS, {} au plus seront demandées à la boutique",
                missing.len(),
                STORE_APP_LOOKUPS
            );
            missing.truncate(STORE_APP_LOOKUPS);
        }
        if !missing.is_empty() {
            for (appid, name, kind, dlc_ids) in store_app_details_all(&missing).await {
                if kind.eq_ignore_ascii_case("game") && !name.trim().is_empty() {
                    entries.push(Entry {
                        appid,
                        name,
                        kind: "Game".to_string(),
                        family: family_appids.contains(&appid) && !owned_appids.contains(&appid),
                        // Sans fiche PICS, ni poids, ni build, ni illustrations :
                        // la bibliothèque les affichera avec ce qu'elle sait
                        // déjà de l'appid. La boutique, elle, publie les DLC :
                        // cette liste relie les contenus d'un membre au jeu.
                        details: lapinux_core::model::SteamDetails {
                            dlc_count: dlc_ids.len(),
                            dlc_ids,
                            ..Default::default()
                        },
                        acquired: app_dates
                            .get(&appid)
                            .copied()
                            .or_else(|| family_date(appid)),
                        minutes: playtimes.get(&appid).copied(),
                    });
                } else if kind.eq_ignore_ascii_case("demo") && !name.trim().is_empty() {
                    entries.push(Entry {
                        appid,
                        name,
                        kind: "Demo".to_string(),
                        family: family_appids.contains(&appid) && !owned_appids.contains(&appid),
                        // La boutique ne publie pas la fiche complète ; le type
                        // suffit pour que l'interface reconnaisse la démo, et
                        // ses DLC publiés restent disponibles.
                        details: lapinux_core::model::SteamDetails {
                            app_type: "demo".to_string(),
                            dlc_count: dlc_ids.len(),
                            dlc_ids,
                            ..Default::default()
                        },
                        acquired: app_dates
                            .get(&appid)
                            .copied()
                            .or_else(|| family_date(appid)),
                        minutes: playtimes.get(&appid).copied(),
                    });
                } else if kind.eq_ignore_ascii_case("dlc") && !name.trim().is_empty() {
                    let owners = family_owners(appid, &app_packages, family_packages);
                    if !owners.is_empty() && !owned_appids.contains(&appid) {
                        family_dlcs.push(crate::catalog::FamilyDlc {
                            appid,
                            name,
                            owners,
                        });
                    } else {
                        dlcs.push(crate::catalog::DlcEntry {
                            appid,
                            name,
                            install: 0,
                            download: 0,
                        });
                    }
                }
            }
        }
        // Dernier recours : le nom que le service de famille a donné. Un jeu
        // partagé que ni PICS ni la boutique ne décrivent garde au moins son
        // nom, au lieu de disparaître de la bibliothèque.
        if !family_names.is_empty() {
            let known: HashSet<u32> = entries
                .iter()
                .map(|entry| entry.appid)
                .chain(dlcs.iter().map(|entry| entry.appid))
                .collect();
            for (appid, name) in &family_names {
                if known.contains(appid) || name.trim().is_empty() {
                    continue;
                }
                entries.push(Entry {
                    appid: *appid,
                    name: name.clone(),
                    kind: "Game".to_string(),
                    family: true,
                    // Sans fiche PICS, ni poids, ni build : la bibliothèque
                    // l'affiche avec ce que le service de famille a donné.
                    details: Default::default(),
                    acquired: app_dates
                        .get(appid)
                        .copied()
                        .or_else(|| family_date(*appid)),
                    minutes: playtimes.get(appid).copied(),
                });
            }
        }
        tracing::info!(
            "catalogue : {described} fiche(s) PICS lues sur {} applications",
            appids.len()
        );
        // Le poids d'un contenu vient des dépôts du jeu parent : la table est
        // complète maintenant que toutes les fiches ont été lues. Sans ce
        // passage, la liste des contenus affichait « poids inconnu ».
        for dlc in &mut dlcs {
            if dlc.download == 0 && dlc.install == 0 {
                if let Some(size) = dlc_sizes.get(&dlc.appid) {
                    dlc.install = size.install;
                    dlc.download = size.download;
                }
            }
        }
        entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        entries.dedup_by_key(|entry| entry.appid);
        dlcs.sort_by_key(|entry| entry.appid);
        dlcs.dedup_by_key(|entry| entry.appid);
        // Ce qui reste sans nom ni fiche est écarté : le journal dit lequel,
        // pour qu'un jeu manquant se retrouve au lieu de disparaître en
        // silence.
        let named: HashSet<u32> = entries
            .iter()
            .map(|entry| entry.appid)
            .chain(dlcs.iter().map(|entry| entry.appid))
            .collect();
        let dropped: Vec<u32> = appids
            .iter()
            .copied()
            .filter(|appid| !named.contains(appid))
            .collect();
        if !dropped.is_empty() {
            tracing::info!(
                "catalogue : {} appid(s) sans nom ni fiche, écartés : {:?}{}",
                dropped.len(),
                &dropped[..dropped.len().min(24)],
                if dropped.len() > 24 { "…" } else { "" }
            );
        }
        family_dlcs.sort_by_key(|dlc| dlc.appid);
        family_dlcs.dedup_by_key(|dlc| dlc.appid);
        Ok((entries, dlcs, package_main, appids, family_dlcs))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_owned_game_already_seen_in_a_family_package_becomes_personal() {
        let mut owned = HashSet::new();
        let mut family = HashSet::from([3_712_150]);
        // L'appid est déjà connu par un paquet familial : la classification
        // doit quand même être corrigée par la réponse de `owned_games`.
        note_owned_game(3_712_150, false, &mut owned, &mut family);
        assert!(owned.contains(&3_712_150));
        assert!(!(family.contains(&3_712_150) && !owned.contains(&3_712_150)));
    }

    #[test]
    fn a_borrowed_game_stays_family() {
        let mut owned = HashSet::new();
        let mut family = HashSet::new();
        note_owned_game(3_712_150, true, &mut owned, &mut family);
        assert!(!owned.contains(&3_712_150));
        assert!(family.contains(&3_712_150));
    }
}

/// Applications d'un paquet, lues dans une réponse de la boutique publique.
///
/// La forme est `{ "<paquet>": { "success": true, "data": { "apps": [ { "id":
/// 440, "name": "Team Fortress 2" } ] } } }` ; un paquet absent du magasin
/// répond `success: false` et n'apporte rien.
pub(super) fn parse_store_package(package: u32, text: &str) -> Vec<(u32, String)> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return Vec::new();
    };
    let Some(entry) = value.get(package.to_string().as_str()) else {
        return Vec::new();
    };
    if entry.get("success").and_then(serde_json::Value::as_bool) != Some(true) {
        return Vec::new();
    }
    entry
        .get("data")
        .and_then(|data| data.get("apps"))
        .and_then(serde_json::Value::as_array)
        .map(|apps| {
            apps.iter()
                .filter_map(|app| {
                    let appid = app.get("id").and_then(serde_json::Value::as_u64)? as u32;
                    let name = app
                        .get("name")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    Some((appid, name))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Nom, type et DLC publiés d'une application, lus dans une réponse de la
/// boutique. Le type (`game`, `dlc`, `tool`, `music`…) décide si l'appid a sa
/// place dans la bibliothèque ; `dlc` donne les contenus du jeu, la seule
/// liste dont dispose un jeu que PICS refuse de décrire.
pub(super) fn parse_store_app(appid: u32, text: &str) -> Option<(String, String, Vec<u32>)> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let entry = value.get(appid.to_string().as_str())?;
    if entry.get("success").and_then(serde_json::Value::as_bool) != Some(true) {
        return None;
    }
    let data = entry.get("data")?;
    let name = data
        .get("name")
        .and_then(serde_json::Value::as_str)?
        .to_string();
    let kind = data
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    let dlc = data
        .get("dlc")
        .and_then(serde_json::Value::as_array)
        .map(|ids| {
            ids.iter()
                .filter_map(serde_json::Value::as_u64)
                .filter_map(|id| u32::try_from(id).ok())
                .collect()
        })
        .unwrap_or_default();
    Some((name, kind, dlc))
}

/// Nom publié d'un paquet par la boutique publique. PICS ne le donne pas ; la
/// boutique le donne sans clé, et c'est ce libellé que la page des licences du
/// compte reprend pour dater un achat.
pub(super) fn parse_store_package_name(package: u32, body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let entry = value.get(package.to_string().as_str())?;
    if entry.get("success").and_then(serde_json::Value::as_bool) != Some(true) {
        return None;
    }
    let name = entry.get("data")?.get("name")?.as_str()?.trim().to_string();
    (!name.is_empty()).then_some(name)
}

/// Noms des paquets demandés à la boutique publique, quatre de front et sous
/// budget : une licence sans date peut être un lot, et dater le lot date tous
/// les jeux qu'il porte.
async fn store_package_names(packages: &[u32]) -> Vec<(u32, String)> {
    let client = reqwest::Client::new();
    let deadline = std::time::Instant::now() + STORE_NAME_BUDGET;
    let mut found = Vec::new();
    let mut next = 0usize;
    let mut tasks = tokio::task::JoinSet::new();
    loop {
        while tasks.len() < STORE_LOOKUP_CONCURRENCY
            && next < packages.len()
            && std::time::Instant::now() < deadline
        {
            let package = packages[next];
            next += 1;
            let client = client.clone();
            tasks.spawn(async move {
                // Le nom d'un paquet ne change pas : le cache évite de
                // redemander chaque synchronisation les mêmes paquets.
                if let Some(cached) = crate::storecache::fresh_package(package) {
                    if let Some(name) = cached.name {
                        crate::storecache::note_package_hit();
                        return (package, Some(name));
                    }
                }
                if matches!(
                    crate::storegate::permit().await,
                    crate::storegate::Verdict::Resting
                ) {
                    return (
                        package,
                        crate::storecache::package(package).and_then(|entry| entry.name),
                    );
                }
                crate::storecache::note_package_fetch();
                let url = format!("{STORE_PACKAGE_URL}{package}");
                let name = match client
                    .get(&url)
                    .header("accept-encoding", "identity")
                    .timeout(STORE_TIMEOUT)
                    .send()
                    .await
                {
                    Ok(response) => {
                        let status = response.status().as_u16();
                        if status == 403 || status == 429 {
                            crate::storegate::refused(crate::storegate::retry_after(&response));
                            return (
                                package,
                                crate::storecache::package(package).and_then(|entry| entry.name),
                            );
                        }
                        crate::storegate::accepted();
                        response
                            .text()
                            .await
                            .ok()
                            .and_then(|body| parse_store_package_name(package, &body))
                    }
                    Err(_) => crate::storecache::package(package).and_then(|entry| entry.name),
                };
                if let Some(name) = name.as_deref() {
                    crate::storecache::save_package_name(package, name);
                }
                (package, name)
            });
        }
        let Some(joined) = tasks.join_next().await else {
            break;
        };
        if let Ok((package, Some(name))) = joined {
            found.push((package, name));
        }
    }
    if next < packages.len() {
        tracing::info!(
            "catalogue : {} paquet(s) sans nom de boutique, repris à la prochaine synchronisation",
            packages.len() - next
        );
    }
    let (hits, fetches) = crate::storecache::take_package_stats();
    if hits > 0 || fetches > 0 {
        tracing::info!(
            "boutique : {fetches} paquet(s) relus sur le réseau, {hits} servi(s) par le cache"
        );
    }
    found
}

/// Dates que la page des licences donne aux paquets du compte.
///
/// La page ne porte aucun identifiant : le rapprochement se fait sur le nom du
/// paquet, et seulement pour ceux dont la liste de licences n'a publié aucun
/// `time_created` — une date connue du protocole n'est jamais remplacée.
pub(super) fn site_package_dates(
    site: &[(String, i64)],
    names: &HashMap<u32, String>,
    known: &HashMap<u32, i64>,
) -> HashMap<u32, i64> {
    let mut dates: HashMap<u32, i64> = HashMap::new();
    for (package, name) in names {
        if known.contains_key(package) {
            continue;
        }
        let oldest = site
            .iter()
            .filter(|(license, stamp)| *stamp > 0 && crate::web::same_title(license, name))
            .map(|(_, stamp)| *stamp)
            .min();
        if let Some(oldest) = oldest {
            dates.insert(*package, oldest);
        }
    }
    dates
}

/// Applications des paquets que PICS n'a pas décrits, demandées à la boutique
/// publique : elle ne veut ni clé ni compte et connaît les licences partagées.
/// Un paquet dont la réponse est vide est simplement absent du magasin.
async fn store_package_apps(packages: &[u32]) -> Vec<(u32, Vec<(u32, String)>)> {
    let client = reqwest::Client::new();
    let deadline = std::time::Instant::now() + STORE_LOOKUP_BUDGET;
    let mut found = Vec::new();
    let mut next = 0usize;
    let mut tasks = tokio::task::JoinSet::new();
    loop {
        while tasks.len() < STORE_LOOKUP_CONCURRENCY
            && next < packages.len()
            && std::time::Instant::now() < deadline
        {
            let package = packages[next];
            next += 1;
            let client = client.clone();
            tasks.spawn(async move { (package, fetch_store_package(&client, package).await) });
        }
        let Some(joined) = tasks.join_next().await else {
            break;
        };
        if let Ok((package, apps)) = joined {
            if !apps.is_empty() {
                found.push((package, apps));
            }
        }
    }
    if next < packages.len() {
        tracing::info!(
            "catalogue : {} paquet(s) restent sans fiche PICS, repris à la prochaine synchronisation",
            packages.len() - next
        );
    }
    let (hits, fetches) = crate::storecache::take_package_stats();
    if hits > 0 || fetches > 0 {
        tracing::info!(
            "boutique : {fetches} paquet(s) relus sur le réseau, {hits} servi(s) par le cache"
        );
    }
    found
}

/// Applications d'un paquet, demandées seules à la boutique publique.
async fn fetch_store_package(client: &reqwest::Client, package: u32) -> Vec<(u32, String)> {
    if let Some(cached) = crate::storecache::fresh_package(package) {
        if !cached.apps.is_empty() {
            crate::storecache::note_package_hit();
            return cached.apps;
        }
    }
    if matches!(
        crate::storegate::permit().await,
        crate::storegate::Verdict::Resting
    ) {
        return crate::storecache::package(package)
            .map(|entry| entry.apps)
            .unwrap_or_default();
    }
    crate::storecache::note_package_fetch();
    let url = format!("{STORE_PACKAGE_URL}{package}");
    let Ok(response) = client
        .get(&url)
        // Sans cette entête, une réponse compressée arriverait telle quelle :
        // le client HTTP ne décompresse que s'il l'a demandée.
        .header("accept-encoding", "identity")
        .timeout(STORE_TIMEOUT)
        .send()
        .await
    else {
        return crate::storecache::package(package)
            .map(|entry| entry.apps)
            .unwrap_or_default();
    };
    let status = response.status().as_u16();
    if status == 403 || status == 429 {
        crate::storegate::refused(crate::storegate::retry_after(&response));
        return crate::storecache::package(package)
            .map(|entry| entry.apps)
            .unwrap_or_default();
    }
    crate::storegate::accepted();
    let Ok(body) = response.text().await else {
        return crate::storecache::package(package)
            .map(|entry| entry.apps)
            .unwrap_or_default();
    };
    let apps = parse_store_package(package, &body);
    if !apps.is_empty() {
        crate::storecache::save_package_apps(package, apps.clone());
    }
    apps
}

/// Fiches de plusieurs applications, demandées à la boutique publique quatre
/// de front et sous budget. Une synchronisation où PICS s'est tu doit retrouver
/// la bibliothèque entière d'un coup, sans bloquer la suivante pour autant.
async fn store_app_details_all(appids: &[u32]) -> Vec<(u32, String, String, Vec<u32>)> {
    let client = reqwest::Client::new();
    let deadline = std::time::Instant::now() + STORE_APP_BUDGET;
    let mut found = Vec::new();
    let mut next = 0usize;
    let mut tasks = tokio::task::JoinSet::new();
    loop {
        while tasks.len() < STORE_LOOKUP_CONCURRENCY
            && next < appids.len()
            && std::time::Instant::now() < deadline
        {
            let appid = appids[next];
            next += 1;
            let client = client.clone();
            tasks.spawn(async move { (appid, store_app_details(&client, appid).await) });
        }
        let Some(joined) = tasks.join_next().await else {
            break;
        };
        if let Ok((appid, Some((name, kind, dlc_ids)))) = joined {
            found.push((appid, name, kind, dlc_ids));
        }
    }
    if next < appids.len() {
        tracing::info!(
            "catalogue : {} application(s) restent sans fiche, reprises à la prochaine synchronisation",
            appids.len() - next
        );
    }
    let (hits, fetches) = crate::storecache::take_app_stats();
    if hits > 0 || fetches > 0 {
        tracing::info!(
            "boutique : {fetches} fiche(s) relues sur le réseau, {hits} servie(s) par le cache"
        );
    }
    found
}

/// Fiche d'une application demandée à la boutique publique : son nom, son
/// type et ses DLC publiés. `None` quand le magasin ne la connaît pas (outil,
/// redistribution).
async fn store_app_details(
    client: &reqwest::Client,
    appid: u32,
) -> Option<(String, String, Vec<u32>)> {
    if let Some(cached) = crate::storecache::fresh_app(appid) {
        // Une entrée écrite avant le relevé des DLC n'en porte pas : elle est
        // relue une fois pour la compléter, sans perdre son nom en cas de
        // panne (les secours plus bas la reprennent).
        if let Some(dlc_ids) = cached.dlc_ids.clone() {
            crate::storecache::note_app_hit();
            return Some((cached.name, cached.kind, dlc_ids));
        }
    }
    if matches!(
        crate::storegate::permit().await,
        crate::storegate::Verdict::Resting
    ) {
        return crate::storecache::app(appid)
            .map(|entry| (entry.name, entry.kind, entry.dlc_ids.unwrap_or_default()));
    }
    crate::storecache::note_app_fetch();
    let url = format!("{STORE_APP_URL}{appid}");
    let Some(body) = async {
        let response = client
            .get(&url)
            .header("accept-encoding", "identity")
            .timeout(STORE_TIMEOUT)
            .send()
            .await
            .ok()?;
        let status = response.status().as_u16();
        if status == 403 || status == 429 {
            crate::storegate::refused(crate::storegate::retry_after(&response));
            return None;
        }
        crate::storegate::accepted();
        response.text().await.ok()
    }
    .await
    else {
        return crate::storecache::app(appid)
            .map(|entry| (entry.name, entry.kind, entry.dlc_ids.unwrap_or_default()));
    };
    let parsed = parse_store_app(appid, &body);
    if let Some((name, kind, dlc_ids)) = parsed.as_ref() {
        crate::storecache::save_app(appid, name, kind, dlc_ids);
    }
    parsed.or_else(|| {
        crate::storecache::app(appid)
            .map(|entry| (entry.name, entry.kind, entry.dlc_ids.unwrap_or_default()))
    })
}
