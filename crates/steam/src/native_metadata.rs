//! Métadonnées PICS et descriptions des applications.

use super::*;

pub(super) fn parse_kv(bytes: &[u8]) -> Option<KeyValue> {
    if bytes.first() == Some(&0) {
        KeyValue::from_binary(bytes).ok()
    } else {
        KeyValue::from_text(&String::from_utf8_lossy(bytes)).ok()
    }
}

/// Les paquets PICS sont préfixés par quatre octets de version.
fn package_body(bytes: &[u8]) -> &[u8] {
    if bytes.len() > 4 && bytes[0] != 0 {
        &bytes[4..]
    } else {
        bytes
    }
}

/// Les métadonnées, `appids` puis `depotids` d'un paquet sont des racines KV
/// consécutives. Le parseur binaire de Steamroom exige d'avoir consommé toute
/// la tranche : on lit donc chaque section elle-même, jusqu'à sa balise de fin.
fn binary_section_ids(data: &[u8], name: &str) -> Option<Vec<u32>> {
    let needle = format!("\0{name}\0");
    let index = data
        .windows(needle.len())
        .position(|w| w == needle.as_bytes())?;
    if data[index] != 0 {
        return None;
    }
    let mut pos = index + 1;
    pos += data[pos..].iter().position(|byte| *byte == 0)? + 1;
    let mut ids = Vec::new();
    while pos < data.len() {
        let tag = data[pos];
        pos += 1;
        if matches!(tag, 8 | 11) {
            return Some(ids);
        }
        if tag != 2 {
            return None;
        }
        pos += data[pos..].iter().position(|byte| *byte == 0)? + 1;
        let value = i32::from_le_bytes(data.get(pos..pos + 4)?.try_into().ok()?);
        pos += 4;
        if value >= 0 {
            ids.push(value as u32);
        }
    }
    Some(ids)
}

fn section_ids(node: &KeyValue, name: &str) -> Vec<u32> {
    let node = if node.key.eq_ignore_ascii_case(name) {
        node
    } else {
        let Some(node) = node.get(name) else {
            return Vec::new();
        };
        node
    };
    let KvValue::Children(children) = &node.value else {
        return Vec::new();
    };
    children
        .values()
        .filter_map(|value| number(Some(value)))
        .filter_map(|value| u32::try_from(value).ok())
        .collect()
}

/// Lit une section d'identifiants (`appids`, `depotids`) d'un paquet PICS.
pub(super) fn package_section_ids(data: &[u8], name: &str) -> Vec<u32> {
    let data = package_body(data);
    let mut ids = if data.first() == Some(&0) {
        binary_section_ids(data, name).unwrap_or_default()
    } else {
        Vec::new()
    };
    if ids.is_empty() {
        if let Some(root) = parse_kv(data) {
            let package = root.get("package").unwrap_or(&root);
            ids = section_ids(package, name);
        }
    }
    ids.sort_unstable();
    ids.dedup();
    ids
}

pub(super) fn package_appids(data: &[u8]) -> Vec<u32> {
    package_section_ids(data, "appids")
}

/// Nombre de dépôts « anonymes » dont la clé est demandée en une
/// synchronisation. Une bibliothèque de famille peut compter des centaines de
/// jeux ; au-delà de ce budget, les jeux restants gardent le poids publié et
/// seront tranchés à la synchronisation suivante.
pub(crate) const DEPOT_KEY_BUDGET: usize = 400;

/// Dépôts que le protocole refuse au compte, pour un jeu dont PICS ne décrit
/// pas le paquet.
///
/// Un jeu prêté par la famille Steam arrive sans liste de dépôts : sa fiche
/// publie donc le contenu de *toutes* les éditions, et le poids annoncé double
/// quand l'éditeur en publie deux (Resident Evil Requiem : 169,7 Go annoncés
/// pour 83,4 Go réels). La seule autorité est le protocole : Steam ne rend la
/// clé de déchiffrement d'un dépôt qu'aux licences qui le couvrent. Seuls les
/// dépôts **anonymes** sont interrogés — ni plateforme, ni langue, ni DLC —
/// car eux seuls peuvent être comptés en double ; les autres sont déjà triés
/// par [`platform_size`].
///
/// `None` quand rien n'a pu être tranché : la fiche garde alors le poids
/// publié plutôt que d'annoncer zéro octet. C'est aussi le cas d'un jeu dont
/// **aucun** dépôt ne répond : les outils gratuits (Proton, Steam Linux
/// Runtime) refusent la clé de tous leurs dépôts alors qu'ils s'installent
/// très bien — sans dépôt autorisé, le refus ne prouve rien.
pub(crate) async fn refused_depots(
    client: &SteamClient<LoggedIn>,
    appid: u32,
    root: &KeyValue,
    budget: &mut usize,
) -> Option<HashSet<u32>> {
    let depots = app_section(root).get("depots")?;
    let KvValue::Children(entries) = &depots.value else {
        return None;
    };
    let mut candidates: Vec<(u32, u32)> = Vec::new();
    for (key, depot) in entries {
        let Ok(depot_id) = key.parse::<u32>() else {
            continue;
        };
        if depot_id == 0
            || depot.get("sharedinstall").is_some()
            || depot.get("dlcappid").is_some()
            || depot
                .get("manifests")
                .and_then(|all| all.get("public"))
                .is_none()
        {
            continue;
        }
        let config = depot.get("config");
        let anonymous = config
            .and_then(|config| config.get("oslist"))
            .and_then(KeyValue::as_str)
            .is_none()
            && config
                .and_then(|config| config.get("language"))
                .and_then(KeyValue::as_str)
                .is_none();
        if anonymous {
            // La clé se demande à l'application qui détient le dépôt, comme
            // à l'installation : un dépôt d'un moteur commun appartient à
            // l'application qui le publie, pas au jeu qui l'emprunte.
            let owner = depot
                .get("depotfromapp")
                .and_then(|entry| number(Some(entry)))
                .or_else(|| {
                    depot
                        .get("config")
                        .and_then(|config| config.get("depotfromapp"))
                        .and_then(|entry| number(Some(entry)))
                })
                .and_then(|value| u32::try_from(value).ok())
                .unwrap_or(appid);
            candidates.push((depot_id, owner));
        }
    }
    // Un seul dépôt anonyme : il ne peut pas y avoir de doublon d'édition.
    if candidates.len() < 2 || candidates.len() > *budget {
        return None;
    }
    let mut refused = HashSet::new();
    let mut granted = 0usize;
    for (depot_id, owner) in candidates {
        *budget = budget.saturating_sub(1);
        match client
            .get_depot_decryption_key(DepotId(depot_id), AppId(owner))
            .await
        {
            Ok(_) => granted += 1,
            // Refus explicite du protocole : le compte n'a pas cette édition.
            Err(steamroom::Error::Connection(
                steamroom::error::ConnectionError::DepotAccessDenied(_),
            )) => {
                refused.insert(depot_id);
            }
            // Toute autre erreur ne prouve rien : le dépôt reste compté.
            Err(error) => {
                tracing::debug!("dépôt {depot_id} de {appid} : clé non vérifiée ({error})");
            }
        }
    }
    if refused.is_empty() {
        tracing::debug!("fiche {appid} : tous les dépôts anonymes sont autorisés");
        return None;
    }
    if granted == 0 {
        tracing::debug!(
            "fiche {appid} : aucun dépôt anonyme autorisé, refus {refused:?} sans conclusion"
        );
        return None;
    }
    tracing::info!(
        "fiche {appid} : dépôts d'une autre édition écartés du poids : {:?}",
        {
            let mut refused: Vec<u32> = refused.iter().copied().collect();
            refused.sort_unstable();
            refused
        }
    );
    Some(refused)
}

pub(super) fn app_section(root: &KeyValue) -> &KeyValue {
    root.get("appinfo").unwrap_or(root)
}

pub(super) fn text(value: Option<&KeyValue>) -> String {
    value
        .and_then(KeyValue::as_str)
        .unwrap_or_default()
        .to_string()
}

pub(super) fn number(value: Option<&KeyValue>) -> Option<u64> {
    match value.map(|value| &value.value) {
        Some(KvValue::String(value)) => value.parse().ok(),
        Some(KvValue::UInt64(value)) => Some(*value),
        Some(KvValue::Int64(value)) => u64::try_from(*value).ok(),
        Some(KvValue::Int32(value)) => u64::try_from(*value).ok(),
        _ => None,
    }
}

pub(super) fn meta_from_pics(root: &KeyValue) -> AppMeta {
    let app = app_section(root);
    let common = app.get("common");
    let config = app.get("config");
    AppMeta {
        name: text(common.and_then(|common| common.get("name"))),
        installdir: text(config.and_then(|config| config.get("installdir"))),
        kind: text(common.and_then(|common| common.get("type"))),
    }
}

fn push_unique(list: &mut Vec<String>, value: String) {
    let value = value.trim().to_string();
    if !value.is_empty() && !list.iter().any(|entry| entry.eq_ignore_ascii_case(&value)) {
        list.push(value);
    }
}

/// Les valeurs booléennes de PICS sont tantôt `"1"`, tantôt `Int32(1)`.
fn truthy(value: Option<&KeyValue>) -> bool {
    match value.map(|value| &value.value) {
        Some(KvValue::String(text)) => matches!(text.as_str(), "1" | "true" | "True"),
        Some(KvValue::Int32(number)) => *number != 0,
        Some(KvValue::UInt64(number)) => *number != 0,
        Some(KvValue::Int64(number)) => *number != 0,
        _ => false,
    }
}

fn genre_label(id: u32) -> Option<&'static str> {
    Some(match id {
        1 => "Action",
        2 => "Stratégie",
        3 => "RPG",
        4 => "Occasionnel",
        23 => "Indépendant",
        25 => "Aventure",
        28 => "Simulation",
        29 => "Massivement multijoueur",
        37 => "Free to Play",
        _ => return None,
    })
}

fn category_label(id: u32) -> Option<&'static str> {
    Some(match id {
        1 => "Multijoueur",
        2 => "Solo",
        8 => "Valve Anti-Cheat",
        9 => "Coopération",
        13 => "Sous-titres",
        14 => "Commentaires audio",
        15 => "Statistiques",
        16 => "SDK Source",
        17 => "Éditeur de niveaux",
        18 => "Manette partielle",
        20 => "MMO",
        22 => "Succès Steam",
        23 => "Steam Cloud",
        24 => "Écran partagé",
        27 => "Multijoueur multiplateforme",
        28 => "Manette complète",
        29 => "Cartes à collectionner",
        30 => "Atelier Steam",
        35 => "Achats intégrés",
        36 => "PvP en ligne",
        37 => "PvP en écran partagé",
        38 => "Coopération en ligne",
        39 => "Coopération en écran partagé",
        40 => "Objets SteamVR",
        41 => "Remote Play (téléphone)",
        42 => "Remote Play (tablette)",
        43 => "Remote Play (TV)",
        44 => "Remote Play Ensemble",
        47 => "PvP en réseau local",
        48 => "Coopération en réseau local",
        49 => "PvP",
        55 | 56 => "DualShock",
        57 | 58 => "DualSense",
        59 => "Steam Input",
        60 => "Manette recommandée",
        61 => "HDR",
        62 => "Partage familial",
        63 => "Steam Timeline",
        64 => "Taille de texte réglable",
        65 => "Options de sous-titres",
        66 => "Alternatives colorées",
        67 => "Confort caméra",
        68 => "Volume réglable",
        69 => "Son stéréo",
        70 => "Son surround",
        71 => "Menus narrés",
        78 => "Difficulté réglable",
        79 => "Sauvegarde à tout moment",
        _ => return None,
    })
}

pub(super) fn language_label(code: &str) -> String {
    match code {
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
        "norwegian" => "Norvégien",
        "finnish" => "Finnois",
        "czech" => "Tchèque",
        "hungarian" => "Hongrois",
        "romanian" => "Roumain",
        "turkish" => "Turc",
        "greek" => "Grec",
        "bulgarian" => "Bulgare",
        "ukrainian" => "Ukrainien",
        "thai" => "Thaï",
        "vietnamese" => "Vietnamien",
        "arabic" => "Arabe",
        "indonesian" => "Indonésien",
        other => other,
    }
    .to_string()
}

fn platform_label(os: &str) -> Option<&'static str> {
    match os.trim().to_ascii_lowercase().as_str() {
        "windows" => Some("Windows"),
        "macos" => Some("macOS"),
        "linux" => Some("Linux"),
        _ => None,
    }
}

fn deck_label(category: u64) -> &'static str {
    match category {
        1 => "Non pris en charge",
        2 => "Jouable",
        3 => "Vérifié",
        _ => "",
    }
}

/// Descripteurs de contenu mature publiés par Steam.
fn descriptor_label(id: u32) -> Option<&'static str> {
    Some(match id {
        1 => "Nudité ou contenu sexuel",
        2 => "Violence ou gore fréquents",
        3 => "Contenu sexuel réservé aux adultes",
        4 => "Nudité ou contenu sexuel fréquent",
        5 => "Contenu mature général",
        _ => return None,
    })
}

/// Somme des manifestes publics des dépôts d'une plateforme. Seuls les
/// dépôts de la licence comptent : un jeu vendu en plusieurs éditions
/// régionales déclare tous leurs dépôts, mais le paquet n'en donne qu'un.
/// Les dépôts partagés (`sharedinstall`), ceux d'un DLC (`dlcappid`) et les
/// doublages autres que la langue retenue sont écartés. `refused` porte les
/// dépôts dont le protocole a refusé la clé : Steam les publie pour une autre
/// édition, le compte ne peut pas les installer.
pub(super) fn platform_size(
    app: &KeyValue,
    platform: &str,
    owned_depots: Option<&HashSet<u32>>,
    refused: Option<&HashSet<u32>>,
    language: Option<&str>,
    dlc: bool,
) -> Option<lapinux_core::model::SteamSize> {
    use lapinux_core::model::SteamSize;
    let depots = app.get("depots")?;
    let KvValue::Children(depots) = &depots.value else {
        return None;
    };
    struct Candidate<'a> {
        language: Option<&'a str>,
        install: u64,
        download: u64,
    }
    let mut candidates: Vec<Candidate> = Vec::new();
    for (key, depot) in depots {
        let Ok(depot_id) = key.parse::<u32>() else {
            continue;
        };
        // Les dépôts connus du compte limitent le compte aux éditions
        // achetées. Sans liste pour ce jeu — cas d'un jeu prêté par la
        // famille, dont PICS ne décrit pas le paquet — on compte tout : sans
        // quoi la fiche n'annonçait aucun poids, donc aucun bouton de taille.
        if let Some(owned) = owned_depots.filter(|owned| !owned.is_empty()) {
            if !owned.contains(&depot_id) {
                continue;
            }
        }
        if refused.is_some_and(|refused| refused.contains(&depot_id)) {
            continue;
        }
        // Les dépôts d'un DLC appartiennent au DLC : ils ne comptent dans le
        // poids du jeu de base que lorsqu'on lit la fiche de ce DLC.
        if depot.get("sharedinstall").is_some() || (!dlc && depot.get("dlcappid").is_some()) {
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
                .any(|os| os.eq_ignore_ascii_case(platform));
            if !wanted {
                continue;
            }
        }
        let Some(manifest) = depot.get("manifests").and_then(|all| all.get("public")) else {
            continue;
        };
        let install = number(manifest.get("size")).unwrap_or(0);
        let download = number(manifest.get("download")).unwrap_or(0);
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
    // Un seul doublage est installé, celui que Steam prendrait : la langue du
    // client quand le jeu la publie, sinon l'anglais, sinon la première par
    // ordre alphabétique pour rester déterministe.
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
    let mut size = SteamSize::default();
    let mut found = false;
    for candidate in candidates {
        if let Some(language) = candidate.language {
            if Some(language) != preferred {
                continue;
            }
        }
        size.install += candidate.install;
        size.download += candidate.download;
        found = true;
    }
    found.then_some(size)
}

/// Fiche complète d'une application, telle que PICS la publie.
pub(super) fn details_from_pics(
    root: &KeyValue,
    appid: u32,
    owned_depots: Option<&HashSet<u32>>,
    refused: Option<&HashSet<u32>>,
    dlc: bool,
) -> lapinux_core::model::SteamDetails {
    use lapinux_core::model::SteamDetails;
    let app = app_section(root);
    let common = app.get("common");
    let extended = app.get("extended");
    let ufs = app.get("ufs");
    let mut details = SteamDetails::default();

    // Développeurs et éditeurs : la liste `associations` fait foi, sinon les
    // champs de la section `extended`.
    if let Some(associations) = common.and_then(|common| common.get("associations")) {
        if let KvValue::Children(entries) = &associations.value {
            for entry in entries.values() {
                let name = text(entry.get("name"));
                match text(entry.get("type")).as_str() {
                    "developer" => push_unique(&mut details.developers, name),
                    "publisher" => push_unique(&mut details.publishers, name),
                    _ => {}
                }
            }
        }
    }
    for (value, list) in [
        (
            text(extended.and_then(|extended| extended.get("developer"))),
            &mut details.developers,
        ),
        (
            text(extended.and_then(|extended| extended.get("publisher"))),
            &mut details.publishers,
        ),
    ] {
        for name in value.split(',') {
            push_unique(list, name.to_string());
        }
    }

    if let Some(genres) = common.and_then(|common| common.get("genres")) {
        if let KvValue::Children(entries) = &genres.value {
            let mut ids: Vec<u32> = entries
                .values()
                .filter_map(|value| number(Some(value)))
                .filter_map(|value| u32::try_from(value).ok())
                .collect();
            ids.sort_unstable();
            for id in ids {
                if let Some(label) = genre_label(id) {
                    push_unique(&mut details.genres, label.to_string());
                }
            }
        }
    }

    if let Some(categories) = common.and_then(|common| common.get("category")) {
        if let KvValue::Children(entries) = &categories.value {
            let mut ids: Vec<u32> = entries
                .keys()
                .filter_map(|key| key.strip_prefix("category_")?.parse().ok())
                .collect();
            ids.sort_unstable();
            ids.dedup();
            for id in ids {
                if let Some(label) = category_label(id) {
                    push_unique(&mut details.categories, label.to_string());
                }
            }
        }
    }

    if let Some(languages) = common.and_then(|common| common.get("supported_languages")) {
        if let KvValue::Children(entries) = &languages.value {
            for (code, entry) in entries {
                if truthy(entry.get("supported")) {
                    push_unique(&mut details.languages, language_label(code));
                }
                if truthy(entry.get("full_audio")) {
                    push_unique(&mut details.full_audio, language_label(code));
                }
            }
        }
    }
    details.languages.sort();
    details.full_audio.sort();

    if let Some(oslist) = common
        .and_then(|common| common.get("oslist"))
        .or_else(|| extended.and_then(|extended| extended.get("validoslist")))
        .and_then(KeyValue::as_str)
    {
        for os in oslist.split(',') {
            if let Some(label) = platform_label(os) {
                push_unique(&mut details.platforms, label.to_string());
            }
        }
    }

    details.release_date = common
        .and_then(|common| common.get("steam_release_date"))
        .or_else(|| common.and_then(|common| common.get("original_release_date")))
        .and_then(|value| number(Some(value)))
        .filter(|value| *value > 0)
        .map(|value| value as i64);
    details.controller =
        match text(common.and_then(|common| common.get("controller_support"))).as_str() {
            "full" => "Complète".into(),
            "partial" => "Partielle".into(),
            _ => String::new(),
        };
    details.metacritic = common
        .and_then(|common| common.get("metacritic_score"))
        .and_then(|value| number(Some(value)))
        .and_then(|value| u32::try_from(value).ok());
    details.metacritic_url = text(common.and_then(|common| common.get("metacritic_fullurl")));
    details.review_percent = common
        .and_then(|common| common.get("review_percentage"))
        .and_then(|value| number(Some(value)))
        .and_then(|value| u32::try_from(value).ok());
    details.review_score = common
        .and_then(|common| common.get("review_score"))
        .and_then(|value| number(Some(value)))
        .and_then(|value| u32::try_from(value).ok());
    details.free = truthy(extended.and_then(|extended| extended.get("isfreeapp")))
        || truthy(common.and_then(|common| common.get("isfreeapp")));
    details.deck = common
        .and_then(|common| common.get("steam_deck_compatibility"))
        .and_then(|deck| deck.get("category"))
        .and_then(|value| number(Some(value)))
        .map(deck_label)
        .unwrap_or_default()
        .to_string();
    details.dlc_count = text(extended.and_then(|extended| extended.get("listofdlc")))
        .split(',')
        .filter(|id| !id.trim().is_empty())
        .count();
    details.original_release_date = common
        .and_then(|common| common.get("original_release_date"))
        .and_then(|value| number(Some(value)))
        .filter(|value| *value > 0)
        .map(|value| value as i64);
    details.metacritic_name = text(common.and_then(|common| common.get("metacritic_name")));
    details.primary_genre = common
        .and_then(|common| common.get("primary_genre"))
        .and_then(|value| number(Some(value)))
        .and_then(|value| u32::try_from(value).ok())
        .and_then(genre_label)
        .unwrap_or_default()
        .to_string();
    details.os_arch = text(common.and_then(|common| common.get("osarch")));
    details.cloud_quota = ufs
        .and_then(|ufs| ufs.get("quota"))
        .and_then(|value| number(Some(value)))
        .filter(|value| *value > 0);
    details.cloud_files = ufs
        .and_then(|ufs| ufs.get("maxnumfiles"))
        .and_then(|value| number(Some(value)))
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value > 0);
    details.homepage = text(extended.and_then(|extended| extended.get("homepage")));
    details.app_type = text(common.and_then(|common| common.get("type")));
    details.workshop = truthy(common.and_then(|common| common.get("workshop_visible")));

    if let Some(descriptors) = common
        .and_then(|common| common.get("content_descriptors"))
        .and_then(|descriptors| match &descriptors.value {
            KvValue::Children(entries) => Some(entries),
            _ => None,
        })
    {
        let mut ids: Vec<u32> = descriptors
            .values()
            .filter_map(|value| number(Some(value)))
            .filter_map(|value| u32::try_from(value).ok())
            .collect();
        ids.sort_unstable();
        ids.dedup();
        for id in ids {
            if let Some(label) = descriptor_label(id) {
                push_unique(&mut details.descriptors, label.to_string());
            }
        }
    }

    details.dlc_ids = text(extended.and_then(|extended| extended.get("listofdlc")))
        .split(',')
        .filter_map(|id| id.trim().parse().ok())
        .collect();
    details.dlc_count = details.dlc_ids.len();

    // Dépôts : build public, branches et poids par plateforme. Les tailles
    // viennent des manifestes publics, sans télécharger un seul octet de jeu.
    if let Some(depots) = app.get("depots") {
        if let KvValue::Children(entries) = &depots.value {
            details.depot_count = entries
                .keys()
                .filter(|key| key.parse::<u32>().is_ok())
                .count();
        }
        if let Some(branches) = depots.get("branches") {
            if let KvValue::Children(entries) = &branches.value {
                details.branches = entries.keys().cloned().collect();
                if let Some(public) = entries.get("public") {
                    details.build_id = public.get("buildid").and_then(|value| number(Some(value)));
                    details.build_time = public
                        .get("timebuildupdated")
                        .and_then(|value| number(Some(value)))
                        .filter(|value| *value > 0)
                        .map(|value| value as i64);
                }
            }
        }
    }
    let supports = |platform: &str| {
        details
            .platforms
            .iter()
            .any(|entry| entry.eq_ignore_ascii_case(platform))
    };
    let language = default_language();
    details.default_language = language.map(language_label).unwrap_or_default();
    if supports("Windows") {
        details.size_windows = platform_size(app, "windows", owned_depots, refused, language, dlc);
    }
    if supports("Linux") {
        details.size_linux = platform_size(app, "linux", owned_depots, refused, language, dlc);
    }
    if supports("macOS") {
        details.size_macos = platform_size(app, "macos", owned_depots, refused, language, dlc);
    }
    if details.size_windows.is_none()
        && details.size_linux.is_none()
        && details.size_macos.is_none()
    {
        tracing::debug!("fiche {appid} : aucun poids publié par PICS");
    }

    // Illustrations officielles du CDN Steam : aucune clé d'API ni service
    // tiers, l'appid suffit.
    let cdn = "https://cdn.cloudflare.steamstatic.com/steam/apps";
    details.header_image = format!("{cdn}/{appid}/header.jpg");
    details.library_image = format!("{cdn}/{appid}/library_600x900.jpg");
    details.hero_image = format!("{cdn}/{appid}/library_hero.jpg");
    details.logo_image = format!("{cdn}/{appid}/logo.png");
    details.capsule_image = format!("{cdn}/{appid}/capsule_231x87.jpg");
    details.background_image = format!("{cdn}/{appid}/library_hero_blur.jpg");
    let clienticon = text(common.and_then(|common| common.get("clienticon")));
    if !clienticon.is_empty() {
        details.icon_image = format!(
            "https://cdn.cloudflare.steamstatic.com/steamcommunity/public/images/apps/{appid}/{clienticon}.jpg"
        );
    }
    details
}
