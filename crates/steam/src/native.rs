//! Moteur Lapinux natif : catalogue du compte et téléchargements de dépôts.
//!
//! Rien ici n'appelle SteamCMD ni un client Steam : la session CM est ouverte
//! avec le jeton de refresh Lapinux, les métadonnées viennent de PICS, les
//! manifestes et les chunks directement des CDN de Valve.

use crate::catalog::Entry;
use crate::cdn_stream::StreamingFetcher;
use crate::cm::proto;
use crate::download_log::{self, DownloadLog};
use crate::throttle::DownloadLimit;
use crate::Error;
use crate::Result;
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use steamroom::apps::{AccessToken, AppInfo, PackageInfo};
use steamroom::cdn::CdnClient;
use steamroom::cdn::CdnServerPool;
use steamroom::client::{LoggedIn, SteamClient};
use steamroom::depot::manifest::DepotManifest;
use steamroom::depot::{AppId, CellId, ChunkId, DepotId, DepotKey, ManifestId, PackageId};
use steamroom::types::key_value::{KvValue, KeyValue};
use steamroom_client::download::{BoxError, ChunkFetcher, DepotJob};
use bytes::Bytes;
use steamroom_client::event::DownloadEvent;
use steamroom_client::login::LoginBuilder;

/// Nombre d'applications demandées en une fois à PICS.
const PICS_BATCH: usize = 50;

/// Nombre de fiches demandées en une fois à PICS. Une fiche pèse bien plus
/// qu'un appid : au-delà d'une vingtaine le CM découpe sa réponse et le client
/// n'en rend que le premier message (27 fiches mesurées pour une demande de
/// 50), d'où deux lots de tailles différentes.
const PICS_INFO_BATCH: usize = 25;

/// Nombre de paquets de licences que la boutique est autorisée à décrire en
/// une synchronisation. Ces demandes ne servent qu'aux licences que PICS n'a
/// pas su décrire : elles restent rares, et un plafond évite qu'une
/// bibliothèque exotique ne transforme la synchronisation en longue série
/// d'appels HTTP.
const STORE_PACKAGE_LOOKUPS: usize = 1200;
/// Nombre d'applications dont la boutique peut donner la fiche, et budget de
/// temps de ce passage. PICS se tait parfois sur presque toutes les
/// applications d'un compte (session CM qui répond à vide) : le plafond doit
/// alors couvrir une bibliothèque entière. À quatre-vingt-seize, chaque
/// relance n'en récupérait qu'une poignée et la grille restait amputée.
const STORE_APP_LOOKUPS: usize = 4000;
const STORE_APP_BUDGET: Duration = Duration::from_secs(180);
/// Nombre de jeux que la bibliothèque de la famille peut ajouter à la liste
/// des licences. Le service de famille rend tout le groupe : la réserve évite
/// qu'un groupe très fourni ne transforme la synchronisation en longue série
/// de fiches PICS.
const FAMILY_SHARED_LIMIT: usize = 400;
/// Requêtes de paquets menées de front : la boutique répond vite, mais elle
/// ferme la porte à une rafale.
const STORE_LOOKUP_CONCURRENCY: usize = 2;
/// Budget accordé à la description des paquets muets. Une bibliothèque
/// exotique ne doit pas transformer la synchronisation en série sans fin :
/// au-delà, les paquets restants attendent la prochaine fois.
const STORE_LOOKUP_BUDGET: Duration = Duration::from_secs(40);
/// Budget accordé aux noms des paquets sans date : la page des licences ne les
/// nomme pas, la boutique si, et c'est ce nom qui les date.
const STORE_NAME_BUDGET: Duration = Duration::from_secs(60);

/// Délai maximal d'une requête à la boutique publique de Steam.
const STORE_TIMEOUT: Duration = Duration::from_secs(8);
/// Endpoint public des fiches d'application, sans clé ni compte.
const STORE_APP_URL: &str = "https://store.steampowered.com/api/appdetails?appids=";
/// Endpoint public des fiches de paquet : il donne les appids d'un paquet et
/// leurs noms, ce que PICS refuse pour certaines licences partagées.
const STORE_PACKAGE_URL: &str = "https://store.steampowered.com/api/packagedetails?packageids=";
/// Bibliothèque partagée du groupe familial, lue avec le jeton d'accès de la
/// session. C'est la seule source qui connaisse *tous* les jeux du groupe : la
/// liste de licences n'en porte qu'une partie, Steam n'y ajoutant une licence
/// partagée que lorsqu'il juge le jeu emprunté.
const FAMILY_SHARED_URL: &str =
    "https://api.steampowered.com/IFamilyGroupsService/GetSharedLibraryApps/v1/";

/// Métadonnées d'une application lues dans PICS.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppMeta {
    pub name: String,
    pub installdir: String,
    pub kind: String,
}

/// Progression d'une installation native.
#[derive(Debug, Clone)]
pub enum InstallEvent {
    /// Nom et répertoire réels, lus avant le premier octet.
    Metadata { meta: AppMeta },
    /// Étape sans progression en octets (manifestes, vérification).
    Status { message: String },
    /// Avancement global de l'installation, tous dépôts confondus.
    Progress {
        /// Dépôt courant (0-based) et nombre de dépôts retenus.
        depot: usize,
        depots: usize,
        /// Part du build déjà traitée, `downloaded / total`.
        fraction: f32,
        /// Octets du build déjà lus ou écrits (fichiers vérifiés inclus).
        downloaded: u64,
        /// Poids total du build, tous dépôts confondus.
        total: u64,
        /// Octets reçus du réseau depuis le premier octet de l'installation.
        network: u64,
        /// Octets écrits sur le disque depuis le premier octet.
        written: u64,
        /// Hôte CDN qui sert les chunks.
        server: String,
        label: String,
    },
}

/// Résultat d'une installation.
#[derive(Debug, Clone)]
pub struct InstallOutcome {
    pub meta: AppMeta,
    pub target: PathBuf,
    pub bytes: u64,
    pub files: u64,
    pub size: u64,
    /// Build public réellement installé, lu dans PICS au moment du téléchargement.
    pub build_id: Option<u64>,
    /// Applications dont les dépôts ont réellement été installés : le jeu de
    /// base, puis chaque contenu additionnel coché qui a abouti. Un contenu
    /// dont la clé de dépôt est refusée n'y figure pas.
    pub apps: Vec<u32>,
}

/// Build de la branche `public` d'un appinfo PICS.
fn public_build_id(root: &KeyValue) -> Option<u64> {
    root.get("depots")?
        .get("branches")?
        .get("public")?
        .get("buildid")
        .and_then(|value| number(Some(value)))
}

fn steam_error(error: impl std::fmt::Display) -> Error {
    Error::Other(format!("Lapinux : {error}"))
}

fn runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .map_err(steam_error)
}

/// Chunks téléchargés en parallèle pour un dépôt. La bibliothèque en fait 16
/// par défaut ; les CDN lents plafonnent alors le débit, surtout quand un jeu
/// n'a qu'un seul dépôt à télécharger.
const CHUNK_DOWNLOADS: usize = 32;

/// Profondeur du pipeline de chunks, ajustée au plafond de débit.
///
/// Avec un plafond bas et trente-deux chunks en vol, chaque chunk met une
/// minute à se terminer et la barre de progression reste immobile. On garde
/// environ deux secondes de données en vol : les chunks se terminent à un
/// rythme régulier, tout en gardant assez de connexions pour remplir le débit.
/// Sans plafond, on garde la parallélisation maximale.
fn chunk_downloads(limit: &DownloadLimit) -> usize {
    let rate = [limit.global.as_ref(), limit.store.as_ref()]
        .into_iter()
        .flatten()
        .map(|limiter| limiter.rate())
        .filter(|rate| *rate > 0)
        .min()
        .unwrap_or(0);
    if rate == 0 {
        return CHUNK_DOWNLOADS;
    }
    ((rate as usize * 2) / (1024 * 1024)).clamp(1, CHUNK_DOWNLOADS)
}

/// Ouvre une session CM à partir du jeton enregistré.
///
/// Publique pour les sondes de diagnostic (`examples/`) : le daemon passe par
/// les fonctions de plus haut niveau.
pub async fn login(account: &str, refresh_token: &str) -> Result<SteamClient<LoggedIn>> {
    LoginBuilder::new()
        .device_name(lapinux_core::APP_NAME)
        .with_refresh_token(account, refresh_token)
        .login()
        .await
        .map_err(steam_error)
}

fn parse_kv(bytes: &[u8]) -> Option<KeyValue> {
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
    let index = data.windows(needle.len()).position(|w| w == needle.as_bytes())?;
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
fn package_section_ids(data: &[u8], name: &str) -> Vec<u32> {
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

fn package_appids(data: &[u8]) -> Vec<u32> {
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
            || depot.get("manifests").and_then(|all| all.get("public")).is_none()
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
            Err(steamroom::Error::Connection(steamroom::error::ConnectionError::DepotAccessDenied(_))) => {
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

fn app_section(root: &KeyValue) -> &KeyValue {
    root.get("appinfo").unwrap_or(root)
}

fn text(value: Option<&KeyValue>) -> String {
    value
        .and_then(KeyValue::as_str)
        .unwrap_or_default()
        .to_string()
}

fn number(value: Option<&KeyValue>) -> Option<u64> {
    match value.map(|value| &value.value) {
        Some(KvValue::String(value)) => value.parse().ok(),
        Some(KvValue::UInt64(value)) => Some(*value),
        Some(KvValue::Int64(value)) => u64::try_from(*value).ok(),
        Some(KvValue::Int32(value)) => u64::try_from(*value).ok(),
        _ => None,
    }
}

fn meta_from_pics(root: &KeyValue) -> AppMeta {
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

fn language_label(code: &str) -> String {
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
fn platform_size(
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
        .filter(|language| available.iter().any(|value| value.eq_ignore_ascii_case(language)))
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
fn details_from_pics(
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
    details.controller = match text(common.and_then(|common| common.get("controller_support")))
        .as_str()
    {
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
            details.depot_count = entries.keys().filter(|key| key.parse::<u32>().is_ok()).count();
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
    if details.size_windows.is_none() && details.size_linux.is_none() && details.size_macos.is_none()
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

async fn product(root_appid: u32, client: &SteamClient<LoggedIn>) -> Result<(AppMeta, KeyValue)> {
    let app_id = AppId(root_appid);
    let tokens = client
        .pics_get_access_tokens(&[app_id])
        .await
        .map_err(steam_error)?;
    let token = tokens
        .iter()
        .find(|token| token.app_id == app_id)
        .map(|token| token.token)
        .unwrap_or(0);
    let infos = client
        .pics_get_product_info(&[AccessToken {
            app_id,
            token,
        }])
        .await
        .map_err(steam_error)?;
    let info: AppInfo = infos
        .into_iter()
        .next()
        .ok_or_else(|| Error::Other(format!("PICS ne connaît pas l'application {root_appid}")))?;
    let bytes = info
        .kv_data
        .ok_or_else(|| Error::Other(format!("PICS sans données pour {root_appid}")))?;
    let root = parse_kv(&bytes)
        .ok_or_else(|| Error::Other(format!("réponse PICS illisible pour {root_appid}")))?;
    let meta = meta_from_pics(&root);
    Ok((meta, root))
}

/// Identifiants demandés que la réponse n'a pas rendus.
fn missing_ids(requested: &[u32], returned: &[u32]) -> Vec<u32> {
    requested
        .iter()
        .filter(|id| !returned.contains(id))
        .copied()
        .collect()
}

/// Lot suivant pour les identifiants que Steam n'a pas rendus : on resserre de
/// moitié, jusqu'à l'appel unitaire. `None` quand l'appel unitaire lui-même n'a
/// rien rendu : l'identifiant est alors abandonné, sans boucle sans fin.
fn smaller_batch(batch: usize) -> Option<usize> {
    (batch > 1).then(|| (batch / 2).max(1))
}

/// Fiches de paquets, sans perte de lot.
///
/// Le CM découpe ses réponses en plusieurs messages et `steamroom` ne rend que
/// le premier : la fin d'un gros lot disparaît sans erreur. Les paquets absents
/// sont redemandés par lots plus petits, jusqu'à l'appel unitaire.
async fn package_infos(
    client: &SteamClient<LoggedIn>,
    packages: &[u32],
) -> Result<Vec<PackageInfo>> {
    let mut wanted = packages.to_vec();
    let mut batch = PICS_BATCH;
    let mut infos = Vec::new();
    while !wanted.is_empty() {
        let mut missing = Vec::new();
        for chunk in wanted.chunks(batch) {
            let ids: Vec<PackageId> = chunk.iter().copied().map(PackageId).collect();
            let found = client
                .pics_get_package_info(&ids)
                .await
                .map_err(steam_error)?;
            let returned: Vec<u32> = found
                .iter()
                .filter_map(|info| info.package_id.map(|id| id.0))
                .collect();
            missing.extend(missing_ids(chunk, &returned));
            infos.extend(found);
        }
        if missing.len() == wanted.len() {
            match smaller_batch(batch) {
                Some(smaller) => batch = smaller,
                None => break,
            }
        }
        wanted = missing;
    }
    Ok(infos)
}

/// Fiches d'applications, sans perte de lot (même découpage que
/// [`package_infos`]).
async fn app_infos(
    client: &SteamClient<LoggedIn>,
    tokens: &[AccessToken],
) -> Result<Vec<AppInfo>> {
    let mut wanted = tokens.to_vec();
    let mut batch = PICS_INFO_BATCH;
    let mut infos = Vec::new();
    while !wanted.is_empty() {
        let mut missing: Vec<AccessToken> = Vec::new();
        for chunk in wanted.chunks(batch) {
            let found = client
                .pics_get_product_info(chunk)
                .await
                .map_err(steam_error)?;
            let returned: Vec<u32> = found
                .iter()
                .filter_map(|info| info.app_id.map(|id| id.0))
                .collect();
            missing.extend(
                chunk
                    .iter()
                    .filter(|token| !returned.contains(&token.app_id.0))
                    .cloned(),
            );
            infos.extend(found);
        }
        if missing.len() == wanted.len() {
            match smaller_batch(batch) {
                Some(smaller) => batch = smaller,
                None => break,
            }
        }
        wanted = missing;
    }
    Ok(infos)
}

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

/// Jeux que le groupe familial partage, appid et nom compris.
///
/// La liste de licences ne suffit pas : certains jeux du groupe n'y figurent
/// jamais, et ils manquaient donc entièrement à la bibliothèque. Ce service
/// rend tout le groupe ; les fiches (poids, build) se demandent ensuite à PICS
/// et à la boutique comme pour les autres jeux.
async fn family_shared_apps(
    client: &SteamClient<LoggedIn>,
    steam_id: u64,
    refresh_token: &str,
) -> Vec<(u32, String)> {
    // Le groupe d'abord : sans famille, aucun jeton d'accès n'est demandé au
    // compte, et la synchronisation garde son coût habituel.
    let Some(group) = family_group_id(client, steam_id).await else {
        return Vec::new();
    };
    let Some(access) = web_access_token(client, steam_id, refresh_token).await else {
        return Vec::new();
    };
    let url = format!(
        "{FAMILY_SHARED_URL}?access_token={access}&family_groupid={group}"
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
        return Vec::new();
    };
    let status = response.status();
    let Ok(body) = response.text().await else {
        return Vec::new();
    };
    if !status.is_success() {
        tracing::debug!(
            "catalogue : bibliothèque familiale refusée ({status}, {} octets)",
            body.len()
        );
        return Vec::new();
    }
    parse_shared_apps(&body)
}

/// Applications d'une réponse `GetSharedLibraryApps` : `appid`, nom, et rien
/// que la grille puisse afficher.
///
/// `app_type` 1 désigne un jeu ; les démos, outils et contenus additionnels du
/// groupe n'ont pas leur place dans la bibliothèque. `exclude_reason` non nul
/// signale une application que le partage familial écarte : elle n'est pas
/// accessible au compte.
fn parse_shared_apps(text: &str) -> Vec<(u32, String)> {
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
            Some((appid, name))
        })
        .collect()
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
    family_packages: &HashSet<u32>,
    package_dates: &HashMap<u32, i64>,
    site_dates: &[(String, i64)],
) -> Result<(
    Vec<Entry>,
    Vec<crate::catalog::DlcEntry>,
    HashMap<u32, u32>,
    Vec<u32>,
)> {
    if packages.is_empty() {
        return Ok((Vec::new(), Vec::new(), HashMap::new(), Vec::new()));
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
            if family_packages.contains(&package_id) {
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
        undescribed.sort_by_key(|package| !family_packages.contains(package));
        let silent = undescribed.len();
        undescribed.truncate(STORE_PACKAGE_LOOKUPS);
        if !undescribed.is_empty() {
            tracing::info!(
                "catalogue : {} paquet(s) de licences sans fiche PICS, {} lus sur la boutique",
                silent,
                undescribed.len()
            );
            for (package, apps) in store_package_apps(&undescribed).await {
                let family = family_packages.contains(&package);
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
                !package_dates.contains_key(package) && !family_packages.contains(package)
            })
            .collect();
        let mut dates = package_dates.clone();
        if !site_dates.is_empty() && !undated.is_empty() {
            let names: HashMap<u32, String> = store_package_names(&undated).await.into_iter().collect();
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
        let shared = family_shared_apps(&client, steam_id, refresh_token).await;
        if !shared.is_empty() {
            let known: HashSet<u32> = appids.iter().copied().collect();
            let mut added = 0usize;
            for (appid, name) in shared {
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
            tracing::info!(
                "catalogue : {added} jeu(x) de la famille en plus des licences"
            );
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
                if appid == 0 || !known.insert(appid) {
                    continue;
                }
                if play.family_shared {
                    family_appids.insert(appid);
                } else {
                    owned_appids.insert(appid);
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
                    let described = owned_depots.get(&appid).is_some_and(|depots| !depots.is_empty());
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
                        acquired: app_dates.get(&appid).copied(),
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
                        acquired: app_dates.get(&appid).copied(),
                        minutes: playtimes.get(&appid).copied(),
                    });
                } else if meta.kind.eq_ignore_ascii_case("dlc") {
                    // Le contenu autonome (rare) publie ses dépôts ; les autres
                    // prennent leur poids dans ceux du jeu parent, ajouté plus
                    // bas quand la table est complète.
                    let size = dlc_size(&root);
                    dlcs.push(crate::catalog::DlcEntry {
                        appid,
                        name: meta.name,
                        install: size.map(|size| size.install).unwrap_or(0),
                        download: size.map(|size| size.download).unwrap_or(0),
                    });
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
            for (appid, name, kind) in store_app_details_all(&missing).await {
                if kind.eq_ignore_ascii_case("game") && !name.trim().is_empty() {
                    entries.push(Entry {
                        appid,
                        name,
                        kind: "Game".to_string(),
                        family: family_appids.contains(&appid) && !owned_appids.contains(&appid),
                        // Sans fiche PICS, ni poids, ni build, ni illustrations :
                        // la bibliothèque les affichera avec ce qu'elle sait
                        // déjà de l'appid.
                        details: Default::default(),
                        acquired: app_dates.get(&appid).copied(),
                        minutes: playtimes.get(&appid).copied(),
                    });
                } else if kind.eq_ignore_ascii_case("demo") && !name.trim().is_empty() {
                    entries.push(Entry {
                        appid,
                        name,
                        kind: "Demo".to_string(),
                        family: family_appids.contains(&appid) && !owned_appids.contains(&appid),
                        // La boutique ne publie pas la fiche complète ; le type
                        // suffit pour que l'interface reconnaisse la démo.
                        details: lapinux_core::model::SteamDetails {
                            app_type: "demo".to_string(),
                            ..Default::default()
                        },
                        acquired: app_dates.get(&appid).copied(),
                        minutes: playtimes.get(&appid).copied(),
                    });
                } else if kind.eq_ignore_ascii_case("dlc") && !name.trim().is_empty() {
                    dlcs.push(crate::catalog::DlcEntry {
                        appid,
                        name,
                        install: 0,
                        download: 0,
                    });
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
                    acquired: app_dates.get(appid).copied(),
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
        Ok((entries, dlcs, package_main, appids))
    })
}

/// Applications d'un paquet, lues dans une réponse de la boutique publique.
///
/// La forme est `{ "<paquet>": { "success": true, "data": { "apps": [ { "id":
/// 440, "name": "Team Fortress 2" } ] } } }` ; un paquet absent du magasin
/// répond `success: false` et n'apporte rien.
fn parse_store_package(package: u32, text: &str) -> Vec<(u32, String)> {
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

/// Nom et type d'une application, lus dans une réponse de la boutique.
/// Le type (`game`, `dlc`, `tool`, `music`…) décide si l'appid a sa place dans
/// la bibliothèque.
fn parse_store_app(appid: u32, text: &str) -> Option<(String, String)> {
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
    Some((name, kind))
}

/// Nom publié d'un paquet par la boutique publique. PICS ne le donne pas ; la
/// boutique le donne sans clé, et c'est ce libellé que la page des licences du
/// compte reprend pour dater un achat.
fn parse_store_package_name(package: u32, body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let entry = value.get(package.to_string().as_str())?;
    if entry.get("success").and_then(serde_json::Value::as_bool) != Some(true) {
        return None;
    }
    let name = entry
        .get("data")?
        .get("name")?
        .as_str()?
        .trim()
        .to_string();
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
                                crate::storecache::package(package)
                                    .and_then(|entry| entry.name),
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
        tracing::info!("boutique : {fetches} paquet(s) relus sur le réseau, {hits} servi(s) par le cache");
    }
    found
}

/// Dates que la page des licences donne aux paquets du compte.
///
/// La page ne porte aucun identifiant : le rapprochement se fait sur le nom du
/// paquet, et seulement pour ceux dont la liste de licences n'a publié aucun
/// `time_created` — une date connue du protocole n'est jamais remplacée.
fn site_package_dates(
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
        tracing::info!("boutique : {fetches} paquet(s) relus sur le réseau, {hits} servi(s) par le cache");
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
async fn store_app_details_all(appids: &[u32]) -> Vec<(u32, String, String)> {
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
        if let Ok((appid, Some((name, kind)))) = joined {
            found.push((appid, name, kind));
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
        tracing::info!("boutique : {fetches} fiche(s) relues sur le réseau, {hits} servie(s) par le cache");
    }
    found
}

/// Fiche d'une application demandée à la boutique publique : son nom et son
/// type. `None` quand le magasin ne la connaît pas (outil, redistribution).
async fn store_app_details(client: &reqwest::Client, appid: u32) -> Option<(String, String)> {
    if let Some(cached) = crate::storecache::fresh_app(appid) {
        crate::storecache::note_app_hit();
        return Some((cached.name, cached.kind));
    }
    if matches!(
        crate::storegate::permit().await,
        crate::storegate::Verdict::Resting
    ) {
        return crate::storecache::app(appid).map(|entry| (entry.name, entry.kind));
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
        return crate::storecache::app(appid).map(|entry| (entry.name, entry.kind));
    };
    let parsed = parse_store_app(appid, &body);
    if let Some((name, kind)) = parsed.as_ref() {
        crate::storecache::save_app(appid, name, kind);
    }
    parsed.or_else(|| crate::storecache::app(appid).map(|entry| (entry.name, entry.kind)))
}

/// Poids publié d'un contenu additionnel : ses propres dépôts, que la lecture
/// d'un jeu de base écarte. La plateforme suit celle de l'installation.
/// Poids de chaque contenu additionnel, lu dans les dépôts du **jeu parent**.
///
/// PICS ne décrit pas les dépôts d'un contenu dans sa propre fiche — elle ne
/// porte que son nom : les dépôts d'un contenu vivent dans la fiche du jeu, avec
/// un champ `dlcappid`. C'est de là que venait le « poids inconnu » : on
/// demandait les dépôts au mauvais endroit.
/// Poids des contenus d'une fiche de jeu, prêts à rejoindre la table du
/// catalogue.
fn dlc_sizes_of(root: &KeyValue) -> HashMap<u32, lapinux_core::model::SteamSize> {
    dlc_sizes(app_section(root))
}

pub(crate) fn dlc_sizes(app: &KeyValue) -> HashMap<u32, lapinux_core::model::SteamSize> {
    let mut sizes: HashMap<u32, lapinux_core::model::SteamSize> = HashMap::new();
    // Une préférence par système, comme pour le jeu de base : Linux d'abord,
    // puis Windows et macOS pour les contenus qui n'en publient qu'un.
    for platform in ["linux", "windows", "macos"] {
        for (id, size) in dlc_sizes_on(app, platform) {
            sizes.entry(id).or_insert(size);
        }
    }
    sizes
}

fn dlc_sizes_on(
    app: &KeyValue,
    platform: &str,
) -> HashMap<u32, lapinux_core::model::SteamSize> {
    let mut sizes: HashMap<u32, lapinux_core::model::SteamSize> = HashMap::new();
    let Some(depots) = app.get("depots") else {
        return sizes;
    };
    let KvValue::Children(depots) = &depots.value else {
        return sizes;
    };
    for (key, depot) in depots {
        let Some(dlc) = number(depot.get("dlcappid")) else {
            continue;
        };
        if key.parse::<u32>().is_err() || depot.get("sharedinstall").is_some() {
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
        let entry = sizes
            .entry(u32::try_from(dlc).unwrap_or_default())
            .or_default();
        entry.install += install;
        entry.download += download;
    }
    sizes
}

pub(crate) fn dlc_size(root: &KeyValue) -> Option<lapinux_core::model::SteamSize> {
    // (la fiche du DLC ne porte pas de dépôts d'édition : rien à départager)
    let app = app_section(root);
    let language = default_language();
    platform_size(app, "windows", None, None, language, true)
        .or_else(|| platform_size(app, "linux", None, None, language, true))
        .or_else(|| platform_size(app, "macos", None, None, language, true))
}

#[derive(Debug, Clone)]
struct DepotPlan {
    depot_id: u32,
    manifest_id: u64,
    from_app: Option<u32>,
}

/// Langue de doublage que Steam utiliserait, lue dans le registre du client.
///
/// Le protocole publie l'inventaire des langues d'un jeu (dépôts de doublage,
/// `supported_languages`, `baselanguages`) mais **pas** celle qui sera
/// retenue : ce choix appartient au client, qui l'envoie lui-même au CM dans
/// `CMsgClientLogon.client_language`. On lit donc le réglage que ce client
/// garde (`HKCU/Software/Valve/Steam/language` du `registry.vdf`) : c'est la
/// langue dans laquelle Steam lance réellement les jeux, pas celle du bureau.
fn language_from_registry(text: &str) -> Option<&'static str> {
    let root = crate::vdf::parse(text).ok()?;
    let registry = root.obj("Registry")?;
    let machine = registry.obj("HKCU")?;
    let valve = machine.obj("Software")?.obj("Valve")?;
    let steam = valve.obj("Steam")?;
    language_code(steam.str("language")?)
}

/// Langue choisie par le client Steam, s'il est installé dans une racine
/// connue. `None` sur une machine sans client Steam.
/// Échappement d'un terme pour l'URL de recherche : lettres, chiffres et
/// ponctuation sûre passent tels quels, le reste devient `%XX`.
fn query_escape(term: &str) -> String {
    let mut out = String::with_capacity(term.len());
    for byte in term.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            b' ' => out.push_str("%20"),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Premier jeu d'un paquet de licences, demandé à la boutique : une clé
/// activée ne donne ni appid ni nom, mais sa licence porte son paquet.
pub fn package_app(package_id: u32) -> Option<(u32, String)> {
    crate::shared::block_on(async move {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(8))
            .build()
            .ok()?;
        let url = format!(
            "https://store.steampowered.com/api/packagedetails/?packageids={package_id}&l=french&cc=FR"
        );
        let response = client.get(url).send().await.ok()?;
        let value: serde_json::Value = response.json().await.ok()?;
        let apps = value.get(package_id.to_string())?.get("data")?.get("apps")?.as_array()?;
        for app in apps {
            let Some(id) = app.get("id").and_then(|id| id.as_u64()) else {
                continue;
            };
            let Some(name) = app.get("name").and_then(|name| name.as_str()) else {
                continue;
            };
            return Some((id as u32, name.to_string()));
        }
        None
    })
}

/// Appid du jeu qui porte ce nom, demandé à la recherche de la boutique
/// Steam : une clé activée ne joint pas toujours l'appid de son produit, et le
/// catalogue a besoin de lui pour ranger la fiche.
pub fn search_appid(name: &str) -> Option<(u32, String)> {
    let term = name.trim().to_string();
    if term.is_empty() {
        return None;
    }
    crate::shared::block_on(async move {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(8))
            .build()
            .ok()?;
        // Le client de ce module n'expose pas les paramètres d'URL : la
        // recherche part avec un terme encodé à la main.
        let url = format!(
            "https://store.steampowered.com/api/storesearch/?term={}&l=french&cc=FR",
            query_escape(&term)
        );
        let response = client.get(url).send().await.ok()?;
        let value: serde_json::Value = response.json().await.ok()?;
        let items = value.get("items")?.as_array()?;
        let mut fallback: Option<(u32, String)> = None;
        for item in items {
            let Some(appid) = item.get("id").and_then(|id| id.as_u64()) else {
                continue;
            };
            let found = item
                .get("name")
                .and_then(|name| name.as_str())
                .unwrap_or_default();
            if found.eq_ignore_ascii_case(&term) {
                return Some((appid as u32, found.to_string()));
            }
            if fallback.is_none() {
                fallback = Some((appid as u32, found.to_string()));
            }
        }
        fallback
    })
}

pub fn steam_language() -> Option<&'static str> {
    let mut candidates: Vec<std::path::PathBuf> = lapinux_core::paths::steam_roots()
        .into_iter()
        .map(|root| root.join("registry.vdf"))
        .collect();
    candidates.push(lapinux_core::paths::home_dir().join(".steam/registry.vdf"));
    candidates
        .into_iter()
        .find_map(|path| language_from_registry(&std::fs::read_to_string(path).ok()?))
}

/// Langue choisie dans les réglages de Lapinux (index dans
/// [`lapinux_core::lang::GAME_LANGUAGES`], `usize::MAX` pour « comme Steam »).
///
/// Elle prime sur celle du client : c'est elle qui décide des dépôts de
/// doublage téléchargés et du poids annoncé, donc de ce que le disque reçoit
/// vraiment.
static OVERRIDE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(usize::MAX);

/// Impose une langue de jeu à tout le moteur ; `None` rend la main au client
/// Steam puis au bureau. Appelée par le daemon à chaque configuration.
pub fn set_language_override(code: Option<&str>) {
    use std::sync::atomic::Ordering;
    let index = code
        .and_then(|code| {
            let code = code.trim();
            lapinux_core::lang::GAME_LANGUAGES
                .iter()
                .position(|language| language.code.eq_ignore_ascii_case(code))
        })
        .unwrap_or(usize::MAX);
    OVERRIDE.store(index, Ordering::Relaxed);
}

/// Langue imposée par les réglages, s'il y en a une.
pub fn language_override() -> Option<&'static str> {
    use std::sync::atomic::Ordering;
    let index = OVERRIDE.load(Ordering::Relaxed);
    lapinux_core::lang::GAME_LANGUAGES
        .get(index)
        .map(|language| language.code)
}

/// Langue de jeu par défaut : celle des réglages si elle est fixée, sinon
/// celle du client Steam, sinon celle du bureau. La lecture du client est
/// mémorisée : la fiche de chaque jeu la demande.
pub fn default_language() -> Option<&'static str> {
    if let Some(code) = language_override() {
        return Some(code);
    }
    use std::sync::OnceLock;
    static LANGUAGE: OnceLock<Option<&'static str>> = OnceLock::new();
    *LANGUAGE.get_or_init(|| steam_language().or_else(system_language))
}

/// Libellé français de cette langue, pour la fiche du jeu.
pub fn default_language_label() -> Option<String> {
    default_language().map(|code| language_label(code))
}

/// Code de dépôt Steam d'une langue, accepté en locale (`fr_FR.UTF-8`, `fr`)
/// comme en nom Steam (`french`, `koreana`).
fn language_code(raw: &str) -> Option<&'static str> {
    let code = raw.trim();
    // Un nom Steam passe tel quel ; une locale se réduit à ses deux lettres.
    let lowered = code.to_ascii_lowercase();
    if let Some(found) = language_name(&lowered) {
        return Some(found);
    }
    let short = lowered
        .split(['_', '-', '.', '@'])
        .next()
        .unwrap_or_default();
    language_name(short)
}

/// Nom du bureau, dans le vocabulaire des dépôts Steam.
fn system_language() -> Option<&'static str> {
    let raw = std::env::var("LC_ALL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| std::env::var("LC_MESSAGES").ok())
        .or_else(|| std::env::var("LANG").ok())?;
    language_code(&raw)
}

/// Table locale courte → code de dépôt Steam, puis noms Steam tels quels.
fn language_name(code: &str) -> Option<&'static str> {
    Some(match code {
        "fr" => "french",
        "en" => "english",
        "de" => "german",
        "es" => "spanish",
        "it" => "italian",
        "pt" => "portuguese",
        "ru" => "russian",
        "pl" => "polish",
        "ja" => "japanese",
        "ko" => "koreana",
        "zh" => "schinese",
        "tr" => "turkish",
        "uk" => "ukrainian",
        "cs" => "czech",
        "da" => "danish",
        "nl" => "dutch",
        "fi" => "finnish",
        "no" => "norwegian",
        "sv" => "swedish",
        "hu" => "hungarian",
        "ro" => "romanian",
        "th" => "thai",
        "vi" => "vietnamese",
        "el" => "greek",
        "bg" => "bulgarian",
        // Noms déjà au vocabulaire Steam.
        "english" => "english",
        "french" => "french",
        "german" => "german",
        "spanish" => "spanish",
        "latam" => "latam",
        "italian" => "italian",
        "portuguese" => "portuguese",
        "brazilian" => "brazilian",
        "russian" => "russian",
        "polish" => "polish",
        "japanese" => "japanese",
        "koreana" => "koreana",
        "schinese" => "schinese",
        "tchinese" => "tchinese",
        "turkish" => "turkish",
        "ukrainian" => "ukrainian",
        "czech" => "czech",
        "danish" => "danish",
        "dutch" => "dutch",
        "finnish" => "finnish",
        "norwegian" => "norwegian",
        "swedish" => "swedish",
        "hungarian" => "hungarian",
        "romanian" => "romanian",
        "thai" => "thai",
        "vietnamese" => "vietnamese",
        "greek" => "greek",
        "bulgarian" => "bulgarian",
        _ => return None,
    })
}

/// Dépôts à installer : une seule langue de doublage, jamais les dépôts de
/// DLC (Steam les installe séparément, avec leur propre appid) ni les dépôts
/// partagés. Sans ce filtre, un jeu comme Cyberpunk annonçait 11 dépôts et
/// téléchargeait dix langues inutiles, soit ~50 Go de trop.
fn depot_plans(app: &KeyValue, platform: &str, language: Option<&str>, dlc: bool) -> Vec<DepotPlan> {
    let candidates = depot_candidates(app, platform, dlc);

    let available: Vec<&str> = candidates
        .iter()
        .filter_map(|(_, language)| language.as_deref())
        .collect();
    // Langue du client Steam si le jeu la propose, sinon l'anglais, sinon la
    // première par ordre alphabétique : le choix reste déterministe.
    let preferred = language
        .filter(|wanted| available.iter().any(|value| value.eq_ignore_ascii_case(wanted)))
        .map(str::to_string)
        .or_else(|| {
            available
                .iter()
                .find(|value| value.eq_ignore_ascii_case("english"))
                .map(|value| value.to_string())
        })
        .or_else(|| available.iter().min().map(|value| value.to_string()));

    candidates
        .into_iter()
        .filter(|(_, language)| {
            language.is_none() || language.as_deref() == preferred.as_deref()
        })
        .map(|(plan, _)| plan)
        .collect()
}

/// Tous les dépôts de contenu du jeu, avec leur langue éventuelle, avant le
/// choix de la langue de doublage.
fn depot_candidates(app: &KeyValue, platform: &str, dlc: bool) -> Vec<(DepotPlan, Option<String>)> {
    let Some(depots) = app.get("depots") else {
        return Vec::new();
    };
    let KvValue::Children(depots) = &depots.value else {
        return Vec::new();
    };
    let mut candidates: Vec<(DepotPlan, Option<String>)> = Vec::new();
    for (key, depot) in depots {
        let Ok(depot_id) = key.parse::<u32>() else {
            continue;
        };
        if depot_id == 0 {
            continue;
        }
        if depot.get("sharedinstall").is_some() || (!dlc && depot.get("dlcappid").is_some()) {
            continue;
        }
        if let Some(config) = depot.get("config") {
            if let Some(oslist) = config.get("oslist").and_then(KeyValue::as_str) {
                let wanted = oslist
                    .split(',')
                    .map(str::trim)
                    .any(|os| os.eq_ignore_ascii_case(platform));
                if !wanted {
                    continue;
                }
            }
        }
        let Some(manifests) = depot.get("manifests") else {
            continue;
        };
        let manifest = manifests
            .get("public")
            .map(|entry| entry.get("gid").unwrap_or(entry));
        let Some(manifest_id) = manifest.and_then(|entry| number(Some(entry))) else {
            continue;
        };
        let from_app = depot
            .get("depotfromapp")
            .and_then(|entry| number(Some(entry)))
            .or_else(|| {
                depot
                    .get("config")
                    .and_then(|config| config.get("depotfromapp"))
                    .and_then(|entry| number(Some(entry)))
            })
            .and_then(|value| u32::try_from(value).ok());
        let language = depot
            .get("config")
            .and_then(|config| config.get("language"))
            .and_then(KeyValue::as_str)
            .map(str::to_string);
        candidates.push((
            DepotPlan {
                depot_id,
                manifest_id,
                from_app,
            },
            language,
        ));
    }
    candidates.sort_by_key(|(plan, _)| plan.depot_id);
    candidates.dedup_by_key(|(plan, _)| plan.depot_id);
    candidates
}

fn pick_platform(
    app: &KeyValue,
    language: Option<&str>,
    dlc: bool,
    preferred: Option<&str>,
) -> (&'static str, Vec<DepotPlan>) {
    // La version demandée passe d'abord ; sans choix, Lapinux lance les jeux
    // Windows avec Proton et ne prend Linux que si l'application n'a rien
    // pour Windows.
    if let Some(preferred) = preferred {
        let plans = depot_plans(app, preferred, language, dlc);
        if !plans.is_empty() {
            return (
                match preferred {
                    "linux" => "linux",
                    "macos" => "macos",
                    _ => "windows",
                },
                plans,
            );
        }
    }
    let windows = depot_plans(app, "windows", language, dlc);
    if !windows.is_empty() {
        return ("windows", windows);
    }
    let linux = depot_plans(app, "linux", language, dlc);
    if !linux.is_empty() {
        return ("linux", linux);
    }
    ("macos", depot_plans(app, "macos", language, dlc))
}

fn safe_installdir(meta: &AppMeta, appid: u32) -> String {
    let raw = meta.installdir.trim();
    let clean = Path::new(raw)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .trim()
        .to_string();
    if !clean.is_empty() && clean != "." && clean != ".." {
        return clean;
    }
    let fallback = if meta.name.trim().is_empty() {
        format!("app-{appid}")
    } else {
        meta.name.clone()
    };
    let slug = lapinux_core::util::slugify(&fallback);
    if slug.is_empty() {
        format!("app-{appid}")
    } else {
        slug
    }
}

fn directory_size(path: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| {
            let path = entry.path();
            if path.is_dir() {
                directory_size(&path)
            } else {
                entry.metadata().map(|meta| meta.len()).unwrap_or(0)
            }
        })
        .sum()
}

fn decompress_manifest(data: &[u8]) -> Result<Vec<u8>> {
    if data.len() > 2 && data[0] == 0x50 && data[1] == 0x4b {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(data))
            .map_err(|error| Error::Other(format!("manifeste ZIP : {error}")))?;
        let mut file = archive
            .by_index(0)
            .map_err(|error| Error::Other(format!("manifeste ZIP vide : {error}")))?;
        let mut out = Vec::new();
        file.read_to_end(&mut out)?;
        Ok(out)
    } else {
        Ok(data.to_vec())
    }
}

/// Dépôt prêt à télécharger : manifeste déjà lu, donc son poids exact est
/// connu avant le premier octet.
struct PreparedDepot {
    index: usize,
    depot_id: DepotId,
    key: DepotKey,
    manifest: DepotManifest,
    total: u64,
    cdn_auth_token: Option<String>,
    /// Application dont ce dépôt vient : le jeu de base, ou le contenu
    /// additionnel coché qui l'apporte.
    from_app: Option<u32>,
}

/// Fetcher de suivi : compte les requêtes en vol, journalise chaque chunk et
/// coupe une requête dès la pause ou l'annulation. Les octets réseau sont
/// comptés par le fetcher CDN au fil de la lecture, pas ici : un chunk bridé
/// peut durer longtemps et la jauge doit avancer avant sa fin.
struct CountingFetcher<F> {
    inner: F,
    /// Compteur réseau partagé, lu pour le cumul du journal ; il est mis à
    /// jour par le fetcher CDN.
    fetched: Arc<AtomicU64>,
    in_flight: Arc<AtomicUsize>,
    log: DownloadLog,
    depot: u32,
    cancel: Arc<AtomicBool>,
    pause: Arc<AtomicBool>,
}

impl<F> CountingFetcher<F> {
    #[allow(clippy::too_many_arguments)]
    fn new(
        inner: F,
        fetched: Arc<AtomicU64>,
        in_flight: Arc<AtomicUsize>,
        log: DownloadLog,
        depot: u32,
        cancel: Arc<AtomicBool>,
        pause: Arc<AtomicBool>,
    ) -> Self {
        Self {
            inner,
            fetched,
            in_flight,
            log,
            depot,
            cancel,
            pause,
        }
    }
}

/// Début du hachage d'un chunk, assez pour le retrouver dans un journal sans
/// écrire quarante caractères hexadécimaux par ligne.
fn chunk_tag(chunk_id: &ChunkId) -> String {
    chunk_id.0[..6]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Attend qu'une pause soit levée ou qu'une annulation arrive. Sert à
/// interrompre une requête déjà en cours au lieu de la laisser finir : sans
/// cela, une pause attendait le dernier chunk en vol, parfois vingt secondes.
async fn wait_until_resumed(cancel: &AtomicBool, pause: &AtomicBool) {
    loop {
        if cancel.load(Ordering::Relaxed) || pause.load(Ordering::Relaxed) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

impl<F: ChunkFetcher> ChunkFetcher for CountingFetcher<F> {
    async fn fetch_chunk(
        &self,
        depot_id: DepotId,
        chunk_id: &ChunkId,
    ) -> std::result::Result<Bytes, BoxError> {
        // Chaque chunk demandé au CDN laisse une trace datée : c'est ce qui
        // permet de vérifier, horodatage en main, qu'aucune requête ne part
        // après une pause ou une annulation.
        let tag = chunk_tag(chunk_id);
        let mut attempt = 0u32;
        loop {
            // La bibliothèque lance les chunks d'un fichier dans des tâches
            // détachées : arrêter de sonder la progression ne les arrête pas.
            // Chaque tâche attend ici avant de toucher au réseau.
            let mut waiting = false;
            loop {
                if self.cancel.load(Ordering::Relaxed) {
                    self.log.write(format!(
                        "chunk ✗ dépôt {} · {tag} abandonné · annulation demandée",
                        self.depot
                    ));
                    return Err("téléchargement annulé".into());
                }
                if !self.pause.load(Ordering::Relaxed) {
                    break;
                }
                if !waiting {
                    waiting = true;
                    self.log.write(format!(
                        "chunk ⋯ dépôt {} · {tag} · pause active, requête retenue",
                        self.depot
                    ));
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }

            attempt += 1;
            let running = self.in_flight.fetch_add(1, Ordering::Relaxed) + 1;
            let started = Instant::now();
            self.log.write(format!(
                "chunk → dépôt {} · {tag} · essai {attempt} · {running} en vol",
                self.depot
            ));
            // Pause ou annulation coupe la requête en cours dans les 50 ms :
            // le corps déjà reçu est perdu, mais plus rien n'est téléchargé.
            let outcome: Option<std::result::Result<Bytes, BoxError>> = tokio::select! {
                biased;
                result = self.inner.fetch_chunk(depot_id, chunk_id) => Some(result),
                _ = wait_until_resumed(&self.cancel, &self.pause) => None,
            };
            self.in_flight.fetch_sub(1, Ordering::Relaxed);
            let elapsed = started.elapsed().as_millis() as u64;
            match outcome {
                Some(result) => {
                    match &result {
                        Ok(data) => {
                            let bytes = data.len() as u64;
                            // Le compteur est tenu par le fetcher, au fil des
                            // octets reçus : ne pas l'incrémenter une seconde
                            // fois ici, le débit serait doublé.
                            let total = self.fetched.load(Ordering::Relaxed);
                            self.log.write(format!(
                                "chunk ← dépôt {} · {tag} · {} Kio en {elapsed} ms ({:.2} Mio/s) · cumul {} o",
                                self.depot,
                                bytes / 1024,
                                download_log::rate_mib(bytes, elapsed),
                                total
                            ));
                        }
                        Err(error) => {
                            self.log.write(format!(
                                "chunk ✗ dépôt {} · {tag} · échec après {elapsed} ms · {error}",
                                self.depot
                            ));
                        }
                    }
                    return result;
                }
                None => {
                    if self.cancel.load(Ordering::Relaxed) {
                        self.log.write(format!(
                            "chunk ✗ dépôt {} · {tag} · interrompu après {elapsed} ms par l'annulation",
                            self.depot
                        ));
                        return Err("téléchargement annulé".into());
                    }
                    self.log.write(format!(
                        "chunk ⏸ dépôt {} · {tag} · interrompu après {elapsed} ms par la pause",
                        self.depot
                    ));
                }
            }
        }
    }
}

/// Télécharge une application dans une bibliothèque Lapinux.
///
/// Le jeton de refresh Lapinux remplace totalement SteamCMD : authentification
/// CM, clés de dépôts, manifestes et chunks passent par notre client. La
/// progression rapportée est globale au build : les compteurs ne repartent
/// jamais de zéro d'un dépôt à l'autre.
pub fn install(
    account: &str,
    refresh_token: &str,
    appid: u32,
    dlc: &[u32],
    // Version demandée (`linux`, `windows`, `macos`), ou `None` pour la
    // préférence de Lapinux (Windows sous Proton, repli Linux).
    platform_choice: Option<&str>,
    // Langue de doublage demandée pour ce jeu ; `None` suit les réglages ou le
    // client Steam.
    preferred_language: Option<&str>,
    library_root: &Path,
    // Dossier qui reçoit les jeux : `Steam` dans une bibliothèque gérée, ou
    // `steamapps/common` pour une vraie bibliothèque Steam.
    games_root: &Path,
    cancel: Arc<AtomicBool>,
    pause: Arc<AtomicBool>,
    log: &DownloadLog,
    limit: DownloadLimit,
    // Serveur CDN préféré (hôte de l'annuaire Steam) ; `None` laisse Steam
    // ordonner ses serveurs, comme son client.
    preferred_server: Option<&str>,
    report: &mut dyn FnMut(InstallEvent),
) -> Result<InstallOutcome> {
    // Le journal complet (voir `download_log`) : une ligne par chunk, un point
    // par seconde avec le débit moteur et celui de la machine.
    let log = log.clone();
    // Profondeur fixée au démarrage : elle dépend du plafond choisi, pas de
    // la tâche, et reste stable pendant tout le transfert.
    let concurrency = chunk_downloads(&limit);
    log.write(format!(
        "install appid={appid} compte={account} bibliothèque={} · {} téléchargements simultanés",
        library_root.display(),
        concurrency
    ));
    let mbit = |rate: u64| rate as f64 * 8.0 / 1_000_000.0;
    log.write(format!(
        "plafonds · global {} · store {}",
        limit
            .global
            .as_ref()
            .map(|limiter| format!("{:.1} Mb/s", mbit(limiter.rate())))
            .unwrap_or_else(|| "illimité".into()),
        limit
            .store
            .as_ref()
            .map(|limiter| format!("{:.1} Mb/s", mbit(limiter.rate())))
            .unwrap_or_else(|| "illimité".into())
    ));
    let started = Instant::now();
    let log_job = log.clone();
    let rt = runtime()?;
    // Première étape signalée tout de suite : l'interface affiche la vraie
    // phase pendant la poignée de main CM, au lieu d'un 0 % immobile.
    report(InstallEvent::Status {
        message: "connexion à Steam".into(),
    });
    let result = rt.block_on(async move {
        // Une seule connexion Steam à la fois : les lectures de fiches
        // attendent la fin du téléchargement au lieu de rouvrir une session
        // qui ferait fermer celle-ci par Steam (erreur « disconnected »).
        let _install = crate::shared::begin_install().await;
        let client = login(account, refresh_token).await?;
        log_job.write("CM Steam connecté, lecture de l'appinfo");
        report(InstallEvent::Status {
            message: "lecture de l'appinfo".into(),
        });
        let (mut meta, root) = product(appid, &client).await?;
        let build_id = public_build_id(&root);
        let installdir = safe_installdir(&meta, appid);
        meta.installdir = installdir.clone();
        if meta.name.trim().is_empty() {
            meta.name = format!("App {appid}");
        }
        if meta.kind.eq_ignore_ascii_case("dlc")
            || meta.kind.eq_ignore_ascii_case("tool")
            || meta.kind.eq_ignore_ascii_case("demo")
        {
            return Err(Error::Other(format!(
                "{} n'est pas un jeu installable",
                meta.name
            )));
        }
        report(InstallEvent::Metadata { meta: meta.clone() });

        let target = games_root.join(&installdir);
        std::fs::create_dir_all(&target)?;
        log_job.write(format!(
            "jeu « {} » · build {} · dossier {}",
            meta.name,
            build_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "inconnu".into()),
            target.display()
        ));
        // La langue demandée pour ce jeu prime sur celle des réglages, qui
        // prime elle-même sur le client Steam et le bureau. Le nom Steam est
        // ramené à la table partagée : un code inconnu n'impose rien.
        let language: Option<&'static str> = preferred_language
            .and_then(lapinux_core::lang::find)
            .map(|language| language.code)
            .or_else(default_language);
        let (platform, mut plans) = pick_platform(
            app_section(&root),
            language,
            false,
            platform_choice,
        );
        if plans.is_empty() {
            return Err(Error::Other(format!(
                "aucun dépôt téléchargeable pour {}",
                meta.name
            )));
        }
        // Contenus additionnels demandés : leurs dépôts rejoignent ceux du jeu
        // et s'installent dans le même dossier, puisque c'est là que Steam les
        // attend. Un DLC non possédé refuse sa clé de dépôt et se saute plus
        // loin, sans interrompre le reste.
        let mut extras: Vec<u32> = Vec::new();
        for id in dlc.iter().copied().filter(|id| *id != appid) {
            report(InstallEvent::Status {
                message: format!("lecture du contenu {id}"),
            });
            let (dlc_meta, dlc_root) = match product(id, &client).await {
                Ok(found) => found,
                Err(error) => {
                    log_job.write(format!("contenu {id} ignoré : {error}"));
                    continue;
                }
            };
            let (dlc_platform, dlc_plans) = pick_platform(
                app_section(&dlc_root),
                language,
                true,
                Some(platform),
            );
            if dlc_plans.is_empty() {
                log_job.write(format!(
                    "contenu « {} » ({id}) sans dépôt {} à télécharger",
                    dlc_meta.name, dlc_platform
                ));
                continue;
            }
            log_job.write(format!(
                "contenu « {} » ({id}) · {dlc_platform} · {} partie(s) : {:?}",
                dlc_meta.name,
                dlc_plans.len(),
                dlc_plans.iter().map(|depot| depot.depot_id).collect::<Vec<_>>()
            ));
            extras.push(id);
            plans.extend(dlc_plans);
        }
        plans.sort_by_key(|plan| plan.depot_id);
        plans.dedup_by_key(|plan| plan.depot_id);
        log_job.write(format!(
            "plateforme {platform} · {} partie(s) retenue(s) : {:?} · doublage {} · {} contenu(s) additionnel(s)",
            plans.len(),
            plans.iter().map(|depot| depot.depot_id).collect::<Vec<_>>(),
            language.unwrap_or("aucun"),
            extras.len()
        ));

        let mut cdn_servers = client
            .get_cdn_servers(CellId(0), Some(20))
            .await
            .map_err(steam_error)?;
        if cdn_servers.is_empty() {
            return Err(Error::Other("aucun serveur CDN Steam disponible".into()));
        }
        // Le serveur choisi passe en tête ; les autres restent en repli, pour
        // qu'un cache régional en panne n'arrête pas le téléchargement.
        if let Some(wanted) = preferred_server.filter(|host| !host.trim().is_empty()) {
            if let Some(index) = cdn_servers.iter().position(|server| server.host == wanted) {
                let chosen = cdn_servers.remove(index);
                cdn_servers.insert(0, chosen);
                log_job.write(format!("serveur CDN demandé : {wanted}"));
            } else {
                log_job.write(format!(
                    "serveur CDN {wanted} absent de la liste : ordre de Steam conservé"
                ));
            }
        }
        log_job.write(format!(
            "{} serveur(s) CDN, premier : {}",
            cdn_servers.len(),
            cdn_servers
                .first()
                .map(|candidate| candidate.host.as_str())
                .unwrap_or("aucun")
        ));

        // Premier serveur retenu : c'est lui que l'interface affiche à côté de
        // la vitesse réseau. Les chunks restent répartis sur le pool.
        let server = cdn_servers
            .first()
            .map(|candidate| candidate.host.clone())
            .unwrap_or_default();
        let cdn = CdnClient::new().map_err(steam_error)?;

        // Première passe : lire tous les manifestes avant de télécharger. Elle
        // donne le poids exact du build et permet une progression globale dès
        // le premier chunk, au lieu de compteurs qui repartent de zéro à
        // chaque dépôt.
        report(InstallEvent::Status {
            message: "lecture des manifestes".into(),
        });
        let mut prepared: Vec<PreparedDepot> = Vec::new();
        let mut last_error = None;
        for (index, depot) in plans.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Other("installation annulée".into()));
            }
            let depot_id = DepotId(depot.depot_id);
            let key_app = AppId(depot.from_app.unwrap_or(appid));
            let key: DepotKey = match client
                .get_depot_decryption_key(depot_id, key_app)
                .await
            {
                Ok(key) => key,
                Err(error) => {
                    // Un dépôt DLC non possédé répond « accès refusé » : ce
                    // n'est pas une raison pour annuler le reste du jeu.
                    tracing::debug!("dépôt {} ignoré : {error}", depot.depot_id);
                    last_error = Some(format!("dépôt {} : {error}", depot.depot_id));
                    continue;
                }
            };
            report(InstallEvent::Status {
                message: format!("manifeste {}/{}", index + 1, plans.len()),
            });
            let manifest_id = ManifestId(depot.manifest_id);
            let request_code = client
                .get_manifest_request_code(key_app, depot_id, manifest_id, Some("public"), None)
                .await
                .ok()
                .flatten()
                .unwrap_or(0);
            let cdn_auth_token = client
                .get_cdn_auth_token(key_app, depot_id, &cdn_servers[0].host)
                .await
                .ok()
                .and_then(|token| token.token);
            let pool = CdnServerPool::new(cdn_servers.clone());
            let raw = cdn
                .download_manifest_pooled(
                    &pool,
                    depot_id,
                    manifest_id,
                    request_code,
                    cdn_auth_token.as_deref(),
                )
                .await
                .map_err(steam_error)?;
            let manifest_bytes = decompress_manifest(&raw)?;
            let mut manifest = DepotManifest::parse(&manifest_bytes).map_err(steam_error)?;
            if manifest.filenames_encrypted {
                let _ = manifest.decrypt_filenames(&key);
            }
            let total = manifest.files.iter().map(|file| file.size).sum();
            log_job.write(format!(
                "dépôt {} · manifeste {} · {} Kio compressés · {} fichiers · {} o",
                depot.depot_id,
                depot.manifest_id,
                raw.len() / 1024,
                manifest.files.len(),
                total
            ));
            // Numérotation des dépôts réellement préparés : un dépôt sans
            // clé (DLC non possédé) est écarté, et l'index du plan affichait
            // alors « dépôt 2/1 ».
            prepared.push(PreparedDepot {
                index: prepared.len(),
                depot_id,
                key,
                manifest,
                total,
                cdn_auth_token,
                from_app: depot.from_app,
            });
        }

        if prepared.is_empty() {
            return Err(Error::Other(format!(
                "aucun dépôt de {} n'a pu être préparé{}",
                meta.name,
                last_error
                    .map(|error| format!(" ({error})"))
                    .unwrap_or_default()
            )));
        }
        let depot_count = prepared.len();
        let grand_total: u64 = prepared.iter().map(|depot| depot.total).sum();
        log_job.write(format!(
            "préparation terminée : {} dépôt(s), {} o à télécharger",
            depot_count, grand_total
        ));
        let network_counter = Arc::new(AtomicU64::new(0));
        let mut bytes = 0u64;
        let mut files = 0u64;
        let mut downloaded = 0u64;
        let mut downloaded_depots = 0usize;
        // Applications dont les dépôts sont allés au bout : le jeu de base et
        // les contenus additionnels réellement installés.
        let mut installed_apps: HashSet<u32> = HashSet::new();
        let mut last_report = Instant::now() - Duration::from_secs(1);
        report(InstallEvent::Progress {
            depot: 0,
            depots: depot_count,
            fraction: 0.0,
            downloaded: 0,
            total: grand_total,
            network: 0,
            written: 0,
            server: server.clone(),
            label: "démarrage".into(),
        });

        for depot in prepared {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Other("installation annulée".into()));
            }
            let (events, mut progress) = tokio::sync::mpsc::unbounded_channel();
            let job = DepotJob::builder()
                .depot_id(depot.depot_id)
                .depot_key(depot.key)
                .install_dir(target.clone())
                .verify(true)
                .max_downloads(concurrency)
                .event_sender(events)
                .build()
                .map_err(steam_error)?;
            let in_flight = Arc::new(AtomicUsize::new(0));
            let fetcher = CountingFetcher::new(
                StreamingFetcher::new(
                    CdnServerPool::new(cdn_servers.clone()),
                    depot.cdn_auth_token,
                    limit.clone(),
                    Arc::clone(&network_counter),
                ),
                Arc::clone(&network_counter),
                Arc::clone(&in_flight),
                log_job.clone(),
                depot.depot_id.0,
                Arc::clone(&cancel),
                Arc::clone(&pause),
            );
            // Taille de chaque fichier du manifeste : les fichiers déjà
            // présents que la vérification saute comptent quand même dans
            // l'avancement, sinon la mise à jour reste à 0 % pendant tout le
            // hachage de l'installation existante.
            let file_sizes: std::collections::HashMap<&str, u64> = depot
                .manifest
                .files
                .iter()
                .map(|file| (file.filename.as_str(), file.size))
                .collect();
            report(InstallEvent::Status {
                message: format!(
                    "partie {}/{} · vérification des fichiers",
                    depot.index + 1,
                    depot_count
                ),
            });
            log_job.write(format!(
                "dépôt {} · démarrage · {} fichiers · {} o à écrire",
                depot.depot_id,
                depot.manifest.files.len(),
                depot.total
            ));
            let network_before = network_counter.load(Ordering::Relaxed);
            let download = job.download(&depot.manifest, Arc::new(fetcher));
            tokio::pin!(download);
            let mut depot_written = 0u64;
            let mut depot_total = depot.total;
            let mut verified = 0u64;
            let mut ticker = tokio::time::interval(Duration::from_millis(150));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            // Échantillonnage : le débit vu par le moteur (chunks qu'il a
            // demandés) et celui reçu par la machine, pour distinguer un
            // téléchargement d'un autre trafic.
            let mut last_paused = pause.load(Ordering::Relaxed);
            let mut last_sample = Instant::now();
            let mut sampled_network = network_counter.load(Ordering::Relaxed);
            let mut sampled_rx = download_log::system_rx_bytes();
            let mut last_progress_at = Instant::now();
            let mut last_stall_log = Instant::now();
            loop {
                let paused = pause.load(Ordering::Relaxed);
                tokio::select! {
                    biased;
                    _ = ticker.tick() => {
                        if cancel.load(Ordering::Relaxed) {
                            log_job.write(format!(
                                "annulation lue par le moteur · dépôt {} · {} chunk(s) en vol · {} o reçus",
                                depot.depot_id,
                                in_flight.load(Ordering::Relaxed),
                                network_counter.load(Ordering::Relaxed)
                            ));
                            return Err(Error::Other("installation annulée".into()));
                        }
                        let paused_now = pause.load(Ordering::Relaxed);
                        if paused_now != last_paused {
                            log_job.write(format!(
                                "pause {} · {} chunk(s) déjà en vol se terminent · {} o reçus",
                                if paused_now { "activée" } else { "levée" },
                                in_flight.load(Ordering::Relaxed),
                                network_counter.load(Ordering::Relaxed)
                            ));
                            last_paused = paused_now;
                            last_sample = Instant::now();
                            sampled_network = network_counter.load(Ordering::Relaxed);
                            sampled_rx = download_log::system_rx_bytes();
                        }
                        if last_sample.elapsed() >= Duration::from_secs(1) {
                            let elapsed_ms = last_sample.elapsed().as_millis() as u64;
                            let network = network_counter.load(Ordering::Relaxed);
                            let rx = download_log::system_rx_bytes();
                            let motor_delta = network.saturating_sub(sampled_network);
                            let rx_delta = match (sampled_rx, rx) {
                                (Some(before), Some(after)) => Some(after.saturating_sub(before)),
                                _ => None,
                            };
                            log_job.write(format!(
                                "tick · dépôt {}/{} · {} · en vol {} · moteur +{} o ({:.2} Mio/s){}",
                                depot.index + 1,
                                depot_count,
                                if paused_now { "EN PAUSE" } else { "téléchargement" },
                                in_flight.load(Ordering::Relaxed),
                                motor_delta,
                                download_log::rate_mib(motor_delta, elapsed_ms),
                                match rx_delta {
                                    Some(delta) => format!(
                                        " · machine +{} o ({:.2} MiB/s)",
                                        delta,
                                        download_log::rate_mib(delta, elapsed_ms)
                                    ),
                                    None => String::new(),
                                }
                            ));
                            if motor_delta > 0 {
                                last_progress_at = Instant::now();
                            }
                            last_sample = Instant::now();
                            sampled_network = network;
                            sampled_rx = rx;
                        }
                        // Un moteur qui ne demande plus rien alors qu'il n'est
                        // pas en pause est le signal d'un blocage local.
                        let silent = !paused_now
                            && in_flight.load(Ordering::Relaxed) == 0
                            && last_progress_at.elapsed() >= Duration::from_secs(4);
                        if silent && last_stall_log.elapsed() >= Duration::from_secs(10) {
                            last_stall_log = Instant::now();
                            log_job.write(format!(
                                "aucun chunk depuis {} s (dépôt {}) : vérification locale ou attente réseau",
                                last_progress_at.elapsed().as_secs(),
                                depot.depot_id
                            ));
                        }
                    }
                    result = &mut download, if !paused => {
                        let stats = result.map_err(steam_error)?;
                        let network_used = network_counter
                            .load(Ordering::Relaxed)
                            .saturating_sub(network_before);
                        log_job.write(format!(
                            "dépôt {} terminé · {} o récupérés sur le réseau · {} o traités (réutilisés compris) · {} fichiers · {} en vol",
                            depot.depot_id,
                            network_used,
                            stats.bytes_downloaded,
                            stats.files_completed,
                            in_flight.load(Ordering::Relaxed)
                        ));
                        bytes += stats.bytes_downloaded;
                        files += stats.files_completed;
                        downloaded = downloaded.saturating_add(depot.total).min(grand_total);
                        downloaded_depots += 1;
                        installed_apps.insert(depot.from_app.unwrap_or(appid));
                        let fraction = if grand_total == 0 {
                            1.0
                        } else {
                            downloaded as f32 / grand_total as f32
                        };
                        report(InstallEvent::Progress {
                            depot: depot.index,
                            depots: depot_count,
                            fraction: fraction.clamp(0.0, 1.0),
                            downloaded,
                            total: grand_total,
                            network: network_counter.load(Ordering::Relaxed),
                            written: downloaded,
                            server: server.clone(),
                            label: format!("partie {} terminée", depot.index + 1),
                        });
                        break;
                    }
                    Some(event) = progress.recv() => {
                        match event {
                            DownloadEvent::DownloadStarted { total_bytes, .. } => {
                                depot_total = total_bytes;
                            }
                            // Les chunks font avancer la tuile en continu : la
                            // bibliothèque ne signale sinon qu'à la fin de
                            // chaque fichier, ce qui paraissait bloqué.
                            DownloadEvent::ChunkCompleted { bytes: chunk } => {
                                depot_written = depot_written.saturating_add(chunk);
                                // Pendant une pause, l'interface garde « en pause » :
                                // les chunks déjà en vol ne doivent pas réécrire
                                // « téléchargement » ni relancer l'avancement.
                                if !pause.load(Ordering::Relaxed)
                                    && last_report.elapsed() >= Duration::from_millis(120)
                                {
                                    last_report = Instant::now();
                                    let done = downloaded
                                        .saturating_add(depot_written.min(depot_total))
                                        .min(grand_total);
                                    let fraction = if grand_total == 0 {
                                        0.0
                                    } else {
                                        (done as f32 / grand_total as f32).clamp(0.0, 1.0)
                                    };
                                    report(InstallEvent::Progress {
                                        depot: depot.index,
                                        depots: depot_count,
                                        fraction,
                                        downloaded: done,
                                        total: grand_total,
                                        network: network_counter.load(Ordering::Relaxed),
                                        written: done,
                                        server: server.clone(),
                                        label: format!("partie {}/{} · téléchargement", depot.index + 1, depot_count),
                                    });
                                }
                            }
                            DownloadEvent::DepotProgress {
                                completed_bytes,
                                total_bytes,
                            } => {
                                depot_total = total_bytes;
                                depot_written = completed_bytes;
                            }
                            DownloadEvent::FileSkipped { filename } => {
                                verified += 1;
                                depot_written = depot_written.saturating_add(
                                    file_sizes.get(filename.as_str()).copied().unwrap_or(0),
                                );
                                if last_report.elapsed() >= Duration::from_millis(200) {
                                    last_report = Instant::now();
                                    let done = downloaded
                                        .saturating_add(depot_written.min(depot_total))
                                        .min(grand_total);
                                    let fraction = if grand_total == 0 {
                                        0.0
                                    } else {
                                        (done as f32 / grand_total as f32).clamp(0.0, 1.0)
                                    };
                                    report(InstallEvent::Progress {
                                        depot: depot.index,
                                        depots: depot_count,
                                        fraction,
                                        downloaded: done,
                                        total: grand_total,
                                        network: network_counter.load(Ordering::Relaxed),
                                        written: done,
                                        server: server.clone(),
                                        label: format!(
                                            "vérification · {} fichiers déjà présents",
                                            verified
                                        ),
                                    });
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        if downloaded_depots == 0 {
            return Err(Error::Other(format!(
                "aucun dépôt de {} n'a pu être téléchargé{}",
                meta.name,
                last_error
                    .map(|error| format!(" ({error})"))
                    .unwrap_or_default()
            )));
        }
        let size = directory_size(&target);
        log_job.write(format!(
            "tous les dépôts terminés · {} o réseau · {} fichiers · {} o sur disque",
            bytes, files, size
        ));
        let mut apps: Vec<u32> = installed_apps.into_iter().collect();
        apps.sort_unstable();
        Ok(InstallOutcome {
            meta,
            target,
            bytes,
            files,
            size,
            build_id,
            apps,
        })
    });
    match &result {
        Ok(outcome) => log.write(format!(
            "installation appid={appid} réussie en {} s · {} o réseau · {} fichiers",
            started.elapsed().as_secs(),
            outcome.bytes,
            outcome.files
        )),
        Err(error) => log.write(format!(
            "installation appid={appid} interrompue après {} s · {error}",
            started.elapsed().as_secs()
        )),
    }
    drop(rt);
    log.write("runtime tokio libéré : connexions CDN fermées, plus aucune requête en cours");
    result
}

/// Présentation et illustrations d'un jeu, lues sur la boutique publique : les
/// images du CDN se déduisent de l'appid, l'icône carrée vient du `clienticon`
/// que la boutique publie. Une clé activée n'a pas encore de fiche PICS ;
/// c'est ce qui lui donne son icône et ses images sans attendre une relève.
pub fn store_art(appid: u32) -> Option<lapinux_core::model::SteamDetails> {
    let client = reqwest::Client::builder()
        .user_agent(crate::tools::USER_AGENT)
        .timeout(STORE_TIMEOUT)
        .build()
        .ok()?;
    // Sans filtre : la boutique ne publie le hachage de l'icône carrée et les
    // genres qu'avec la fiche complète.
    let url = format!("{STORE_APP_URL}{appid}&l=french&cc=FR");
    let value: serde_json::Value = crate::shared::block_on(async {
        let response = client
            .get(&url)
            .header("accept-encoding", "identity")
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        response.json::<serde_json::Value>().await.ok()
    })?;
    let data = value.get(appid.to_string())?.get("data")?;
    let text = |key: &str| -> String {
        data.get(key)
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string()
    };
    let listed = |key: &str| -> Vec<String> {
        data.get(key)
            .and_then(|value| value.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        item.get("description")
                            .and_then(|found| found.as_str())
                            .or_else(|| item.as_str())
                    })
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut platforms = Vec::new();
    if let Some(flags) = data.get("platforms") {
        for (key, label) in [("windows", "Windows"), ("mac", "macOS"), ("linux", "Linux")] {
            if flags.get(key).and_then(|value| value.as_bool()) == Some(true) {
                platforms.push(label.to_string());
            }
        }
    }
    let cdn = "https://cdn.cloudflare.steamstatic.com/steam/apps";
    let kind = text("type");
    let mut details = lapinux_core::model::SteamDetails {
        header_image: format!("{cdn}/{appid}/header.jpg"),
        library_image: format!("{cdn}/{appid}/library_600x900.jpg"),
        hero_image: format!("{cdn}/{appid}/library_hero.jpg"),
        logo_image: format!("{cdn}/{appid}/logo.png"),
        capsule_image: format!("{cdn}/{appid}/capsule_231x87.jpg"),
        background_image: format!("{cdn}/{appid}/library_hero_blur.jpg"),
        app_type: if kind.eq_ignore_ascii_case("game") {
            "Game".to_string()
        } else if kind.is_empty() {
            String::new()
        } else {
            let mut letters = kind.chars();
            match letters.next() {
                Some(first) => first.to_uppercase().collect::<String>() + letters.as_str(),
                None => String::new(),
            }
        },
        developers: listed("developers"),
        publishers: listed("publishers"),
        genres: listed("genres"),
        categories: listed("categories"),
        platforms,
        homepage: text("website"),
        free: data
            .get("is_free")
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
        ..Default::default()
    };
    // Les jeux récents ne sont plus servis au chemin historique : la boutique
    // donne l'adresse hachée de leur bannière et de leur capsule.
    let header = text("header_image");
    if !header.is_empty() {
        details.header_image = header;
    }
    let capsule = text("capsule_image");
    let capsule = if capsule.is_empty() {
        text("capsule_imagev5")
    } else {
        capsule
    };
    if !capsule.is_empty() {
        details.capsule_image = capsule;
    }
    let icon = text("clienticon");
    if !icon.is_empty() {
        details.icon_image = format!(
            "https://cdn.cloudflare.steamstatic.com/steamcommunity/public/images/apps/{appid}/{icon}.jpg"
        );
    }
    Some(details)
}


#[cfg(test)]
mod tests {
    use super::*;

    /// La boutique publique nomme un paquet : c'est ce libellé que la page des
    /// licences reprend, et il relie la date au paquet.
    #[test]
    fn a_store_package_publishes_its_name() {
        let body = r#"{"256340":{"success":true,"data":{"name":"Insurgency (WW)","apps":[{"id":222880,"name":"Insurgency"}]}}}"#;
        assert_eq!(
            parse_store_package_name(256340, body).as_deref(),
            Some("Insurgency (WW)")
        );
        // Un paquet absent ou refusé n'a pas de nom.
        assert_eq!(parse_store_package_name(1, body), None);
        assert_eq!(
            parse_store_package_name(1, r#"{"1":{"success":false}}"#),
            None
        );
    }

    /// Une ligne datée du site date son paquet, et seulement lui : une date
    /// déjà connue du protocole n'est jamais remplacée.
    #[test]
    fn site_dates_reach_the_package_they_name() {
        let site = vec![
            ("Insurgency".to_string(), 1_793_448_000i64),
            ("Among the Sleep (Global) - Retail".to_string(), 1_793_448_000),
            ("Jeu inconnu du compte".to_string(), 1_300_000_000),
        ];
        let names: HashMap<u32, String> = [
            (100, "Insurgency (WW)".to_string()),
            (200, "Among the Sleep".to_string()),
            (300, "Half-Life 2".to_string()),
        ]
        .into_iter()
        .collect();
        let known: HashMap<u32, i64> = [(300, 999_999_999)].into_iter().collect();
        let dates = site_package_dates(&site, &names, &known);
        assert_eq!(dates.get(&100), Some(&1_793_448_000));
        assert_eq!(dates.get(&200), Some(&1_793_448_000));
        assert_eq!(dates.get(&300), None, "la date du protocole est gardée");
        assert_eq!(dates.len(), 2);
        assert!(site_package_dates(&[], &names, &known).is_empty());
    }

    /// La langue imposée par les réglages passe par la table partagée : un nom
    /// Steam connu est retenu, un nom inconnu rend la main au client.
    #[test]
    fn the_language_override_keeps_known_steam_names() {
        set_language_override(Some("french"));
        assert_eq!(language_override(), Some("french"));
        assert_eq!(default_language(), Some("french"));

        set_language_override(Some("klingon"));
        assert_eq!(language_override(), None);

        set_language_override(None);
        assert_eq!(language_override(), None);
    }

    /// Fetcher de test : compte les appels au réseau et rend un contenu fixe.
    struct FakeFetcher(Arc<AtomicUsize>);

    impl ChunkFetcher for FakeFetcher {
        async fn fetch_chunk(
            &self,
            _depot_id: DepotId,
            _chunk_id: &ChunkId,
        ) -> std::result::Result<Bytes, BoxError> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(Bytes::from_static(b"contenu"))
        }
    }

    fn counting(
        calls: Arc<AtomicUsize>,
        cancel: Arc<AtomicBool>,
        pause: Arc<AtomicBool>,
    ) -> CountingFetcher<FakeFetcher> {
        CountingFetcher::new(
            FakeFetcher(calls),
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicUsize::new(0)),
            DownloadLog::sink(),
            1,
            cancel,
            pause,
        )
    }

    #[tokio::test]
    async fn paused_fetcher_does_not_touch_the_network() {
        let calls = Arc::new(AtomicUsize::new(0));
        let pause = Arc::new(AtomicBool::new(true));
        let fetcher = Arc::new(counting(
            Arc::clone(&calls),
            Arc::new(AtomicBool::new(false)),
            Arc::clone(&pause),
        ));
        let running = {
            let fetcher = Arc::clone(&fetcher);
            tokio::spawn(async move {
                fetcher
                    .fetch_chunk(DepotId(1), &ChunkId([7u8; 20]))
                    .await
            })
        };
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(
            calls.load(Ordering::Relaxed),
            0,
            "aucune requête ne doit partir pendant la pause"
        );

        pause.store(false, Ordering::Relaxed);
        let data = tokio::time::timeout(Duration::from_secs(2), running)
            .await
            .expect("la reprise débloque la requête")
            .expect("tâche terminée")
            .expect("chunk récupéré");
        assert_eq!(&data[..], b"contenu");
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    /// Fetcher volontairement lent : sert à vérifier qu'une pause coupe la
    /// requête en cours au lieu d'attendre sa fin.
    struct SlowFetcher(Arc<AtomicUsize>);

    impl ChunkFetcher for SlowFetcher {
        async fn fetch_chunk(
            &self,
            _depot_id: DepotId,
            _chunk_id: &ChunkId,
        ) -> std::result::Result<Bytes, BoxError> {
            self.0.fetch_add(1, Ordering::Relaxed);
            tokio::time::sleep(Duration::from_secs(5)).await;
            Ok(Bytes::from_static(b"lent"))
        }
    }

    #[tokio::test]
    async fn pause_interrupts_an_in_flight_request() {
        let calls = Arc::new(AtomicUsize::new(0));
        let pause = Arc::new(AtomicBool::new(false));
        let fetcher = Arc::new(CountingFetcher::new(
            SlowFetcher(Arc::clone(&calls)),
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicUsize::new(0)),
            DownloadLog::sink(),
            1,
            Arc::new(AtomicBool::new(false)),
            Arc::clone(&pause),
        ));
        let running = {
            let fetcher = Arc::clone(&fetcher);
            tokio::spawn(async move {
                fetcher
                    .fetch_chunk(DepotId(1), &ChunkId([3u8; 20]))
                    .await
            })
        };
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(calls.load(Ordering::Relaxed), 1);

        pause.store(true, Ordering::Relaxed);
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert_eq!(
            calls.load(Ordering::Relaxed),
            1,
            "la requête en cours est abandonnée, pas relancée pendant la pause"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert_eq!(calls.load(Ordering::Relaxed), 1);

        pause.store(false, Ordering::Relaxed);
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(
            calls.load(Ordering::Relaxed),
            2,
            "la reprise redemande le chunk"
        );
        running.abort();
    }

    #[tokio::test]
    async fn cancelled_fetcher_fails_without_a_request() {
        let calls = Arc::new(AtomicUsize::new(0));
        let fetcher = counting(
            Arc::clone(&calls),
            Arc::new(AtomicBool::new(true)),
            Arc::new(AtomicBool::new(false)),
        );
        let result = fetcher
            .fetch_chunk(DepotId(1), &ChunkId([9u8; 20]))
            .await;
        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::Relaxed), 0);
    }

    const APP: &str = r#"
"appinfo"
{
    "appid" "440"
    "common"
    {
        "name" "Team Fortress 2"
        "type" "Game"
    }
    "config"
    {
        "installdir" "Team Fortress 2"
    }
    "depots"
    {
        "441"
        {
            "config" { "oslist" "windows" }
            "manifests" { "public" { "gid" "123456" } }
        }
        "442"
        {
            "config" { "oslist" "linux" }
            "manifests" { "public" { "gid" "654321" } }
        }
        "443"
        {
            "config" { "oslist" "windows" }
            "manifests" { "public" { "gid" "999" } }
            "sharedinstall" "1"
        }
    }
}
"#;

    #[test]
    fn reads_name_installdir_and_windows_depots() {
        let root = parse_kv(APP.as_bytes()).unwrap();
        let meta = meta_from_pics(&root);
        assert_eq!(meta.name, "Team Fortress 2");
        assert_eq!(meta.installdir, "Team Fortress 2");
        assert_eq!(meta.kind, "Game");
        let (platform, plans) = pick_platform(app_section(&root), None, false, None);
        assert_eq!(platform, "windows");
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].depot_id, 441);
        assert_eq!(plans[0].manifest_id, 123456);
    }

    #[test]
    fn falls_back_to_linux_when_windows_is_absent() {
        let root = parse_kv(
            r#"
"appinfo" {
    "depots" {
        "442" {
            "config" { "oslist" "linux" }
            "manifests" { "public" "654321" }
        }
    }
}"#
            .as_bytes(),
        )
        .unwrap();
        let (platform, plans) = pick_platform(app_section(&root), None, false, None);
        assert_eq!(platform, "linux");
        assert_eq!(plans[0].manifest_id, 654321);
    }

    #[test]
    fn the_chosen_version_wins_over_the_default() {
        // Le jeu publie Windows et Linux : la demande prime, et sans demande
        // Windows reste le choix de Proton.
        let root = parse_kv(
            br#"
"appinfo" {
    "depots" {
        "441" {
            "config" { "oslist" "windows" }
            "manifests" { "public" "654320" }
        }
        "442" {
            "config" { "oslist" "linux" }
            "manifests" { "public" "654321" }
        }
    }
}"#,
        )
        .unwrap();
        let (platform, plans) = pick_platform(app_section(&root), None, false, Some("linux"));
        assert_eq!(platform, "linux");
        assert_eq!(plans[0].manifest_id, 654321);
        let (platform, _) = pick_platform(app_section(&root), None, false, Some("windows"));
        assert_eq!(platform, "windows");
        let (platform, _) = pick_platform(app_section(&root), None, false, None);
        assert_eq!(platform, "windows");
    }

    #[test]
    fn a_version_the_game_does_not_publish_falls_back() {
        let root = parse_kv(
            br#"
"appinfo" {
    "depots" {
        "441" {
            "config" { "oslist" "windows" }
            "manifests" { "public" "654320" }
        }
    }
}"#,
        )
        .unwrap();
        // Linux demandé, mais le jeu n'en a pas : le dépôt Windows reste le
        // seul téléchargeable, la tâche ne doit pas échouer pour autant.
        let (platform, plans) = pick_platform(app_section(&root), None, false, Some("linux"));
        assert_eq!(platform, "windows");
        assert!(!plans.is_empty());
    }

    #[test]
    fn install_plan_keeps_one_language_and_skips_dlc() {
        let root = parse_kv(
            br#"
"appinfo" {
    "depots" {
        "1" {
            "config" { "oslist" "windows" }
            "manifests" { "public" { "gid" "10" } }
        }
        "2" {
            "config" { "oslist" "windows" "language" "english" }
            "manifests" { "public" { "gid" "11" } }
        }
        "3" {
            "config" { "oslist" "windows" "language" "french" }
            "manifests" { "public" { "gid" "12" } }
        }
        "4" {
            "dlcappid" "99"
            "manifests" { "public" { "gid" "13" } }
        }
    }
}"#,
        )
        .unwrap();
        let (_, plans) = pick_platform(app_section(&root), Some("french"), false, None);
        let ids: Vec<u32> = plans.iter().map(|plan| plan.depot_id).collect();
        assert_eq!(ids, vec![1, 3], "contenu de base + doublage français");

        // Langue absente du jeu : repli sur l'anglais, toujours sans DLC.
        let (_, plans) = pick_platform(app_section(&root), Some("japanese"), false, None);
        let ids: Vec<u32> = plans.iter().map(|plan| plan.depot_id).collect();
        assert_eq!(ids, vec![1, 2]);
    }

    #[test]
    fn a_dlc_depot_belongs_to_its_own_content() {
        let root = parse_kv(
            br#"
"appinfo" {
    "depots" {
        "1" {
            "config" { "oslist" "windows" }
            "manifests" { "public" { "gid" "10" "size" "1000" "download" "400" } }
        }
        "2" {
            "dlcappid" "99"
            "config" { "oslist" "windows" }
            "manifests" { "public" { "gid" "11" "size" "500" "download" "200" } }
        }
    }
}
"#,
        )
        .unwrap();
        let app = app_section(&root);
        // Le poids du jeu de base ignore ce qui appartient au contenu.
        let base = platform_size(app, "windows", None, None, None, false).unwrap();
        assert_eq!(base.download, 400);
        assert_eq!(base.install, 1000);
        // Lu comme un contenu, ses dépôts comptent.
        let content = platform_size(app, "windows", None, None, None, true).unwrap();
        assert_eq!(content.download, 600);
        assert_eq!(content.install, 1500);
        // Et ils rejoignent la liste à télécharger.
        let (_, plans) = pick_platform(app, None, true, None);
        assert_eq!(
            plans.iter().map(|plan| plan.depot_id).collect::<Vec<_>>(),
            vec![1, 2]
        );
    }

    #[test]
    fn the_parent_game_publishes_each_contents_weight() {
        // Fiche réelle d'un jeu, réduite : les dépôts d'un contenu vivent chez
        // le jeu parent, avec son `dlcappid`. La fiche du contenu, elle, ne les
        // décrit pas — d'où le « poids inconnu ».
        let root = parse_kv(
            br#"
"appinfo" {
    "depots" {
        "1" {
            "config" { "oslist" "linux" }
            "manifests" { "public" { "gid" "10" "size" "8000" "download" "4000" } }
        }
        "2" {
            "dlcappid" "99"
            "manifests" { "public" { "gid" "11" "size" "500" "download" "200" } }
        }
        "3" {
            "dlcappid" "98"
            "manifests" { "public" { "gid" "12" "size" "70" "download" "30" } }
        }
        "4" {
            "dlcappid" "98"
            "config" { "oslist" "windows" }
            "manifests" { "public" { "gid" "13" "size" "9" "download" "9" } }
        }
    }
}
"#,
        )
        .unwrap();
        let sizes = dlc_sizes(app_section(&root));
        assert_eq!(sizes.get(&99).map(|size| size.install), Some(500));
        assert_eq!(sizes.get(&99).map(|size| size.download), Some(200));
        // Un contenu dont un seul dépôt porte le système retenu garde son
        // poids : le dépôt Windows ne compte pas dans la version Linux.
        assert_eq!(sizes.get(&98).map(|size| size.install), Some(70));
        // Le jeu de base n'est pas un contenu.
        assert!(!sizes.contains_key(&1));
    }

    #[test]
    fn a_dlc_publishes_its_own_weight() {
        let root = parse_kv(
            br#"
"appinfo" {
    "common" { "name" "Extension" "type" "DLC" "oslist" "windows" }
    "depots" {
        "7" {
            "dlcappid" "99"
            "config" { "oslist" "windows" }
            "manifests" { "public" { "gid" "5" "size" "2048" "download" "1024" } }
        }
    }
}
"#,
        )
        .unwrap();
        assert_eq!(meta_from_pics(&root).kind, "DLC");
        let size = dlc_size(&root).expect("un poids publié");
        assert_eq!(size.install, 2048);
        assert_eq!(size.download, 1024);
    }

    #[test]
    fn chunk_pipeline_depth_follows_the_cap() {
        let limited = |mbit: u64| DownloadLimit {
            global: Some(crate::throttle::RateLimiter::new(mbit * 1_000_000 / 8)),
            store: None,
        };
        // Sans plafond, le maximum de parallélisme.
        assert_eq!(chunk_downloads(&DownloadLimit::unlimited()), CHUNK_DOWNLOADS);
        // Environ deux secondes de chunks : 10 Mb/s → 2, 20 Mb/s → 4.
        assert_eq!(chunk_downloads(&limited(10)), 2);
        assert_eq!(chunk_downloads(&limited(20)), 4);
        assert_eq!(chunk_downloads(&limited(1000)), CHUNK_DOWNLOADS);
        // Le plafond le plus strict décide de la profondeur.
        let both = DownloadLimit {
            global: Some(crate::throttle::RateLimiter::new(100_000_000 / 8)),
            store: Some(crate::throttle::RateLimiter::new(20_000_000 / 8)),
        };
        assert_eq!(chunk_downloads(&both), 4);
    }

    #[test]
    fn public_build_is_read_from_the_public_branch() {
        let root = parse_kv(
            br#"
"appinfo" {
    "depots" {
        "branches" {
            "public" { "buildid" "21616108" "timebuildupdated" "1700000000" }
            "beta" { "buildid" "1" }
        }
    }
}"#,
        )
        .unwrap();
        assert_eq!(public_build_id(&root), Some(21616108));
    }

    #[test]
    fn package_appids_are_read_and_deduplicated() {
        let data = br#"
"package"
{
    "packageid" "33530"
    "appids"
    {
        "0" "440"
        "1" "730"
        "2" "440"
        "3" "228980"
    }
}
"#;
        assert_eq!(package_appids(data), vec![440, 730, 228980]);
    }

    #[test]
    fn the_family_service_names_the_games_missing_from_the_licenses() {
        // Réponse réelle de `GetSharedLibraryApps`, réduite à trois entrées :
        // un jeu partagé, un contenu additionnel et une application que le
        // partage familial écarte.
        let body = r#"{
            "response": { "apps": [
                { "appid": 3764200, "name": "Resident Evil Requiem",
                  "app_type": 1, "exclude_reason": 0 },
                { "appid": 3990820, "name": "Resident Evil Requiem - Deluxe Kit",
                  "app_type": 6, "exclude_reason": 0 },
                { "appid": 620, "name": "Portal 2",
                  "app_type": 1, "exclude_reason": 3 }
            ] }
        }"#;
        assert_eq!(
            parse_shared_apps(body),
            vec![(3764200, "Resident Evil Requiem".to_string())]
        );
        // Une réponse illisible ou vide n'ajoute rien à la bibliothèque.
        assert!(parse_shared_apps("pas du json").is_empty());
        assert!(parse_shared_apps(r#"{"response":{}}"#).is_empty());
        assert!(parse_shared_apps(r#"{"response":{"apps":[]}}"#).is_empty());
    }

    #[test]
    fn the_store_fills_what_pics_keeps_quiet() {
        // Réponses réelles de la boutique publique, sans clé ni compte :
        // `packagedetails` rend les appids d'un paquet et leurs noms.
        let packages = r#"{
            "469": { "success": true, "data": { "apps": [
                { "id": 220, "name": "Half-Life 2" },
                { "id": 440, "name": "Team Fortress 2" }
            ] } },
            "330198": { "success": false }
        }"#;
        assert_eq!(
            parse_store_package(469, packages),
            vec![
                (220, "Half-Life 2".to_string()),
                (440, "Team Fortress 2".to_string())
            ]
        );
        // Un paquet absent du magasin, ou une réponse illisible, n'apporte rien.
        assert!(parse_store_package(330198, packages).is_empty());
        assert!(parse_store_package(999, packages).is_empty());
        assert!(parse_store_package(469, "pas du json").is_empty());

        // `appdetails` donne le type, qui décide de la place d'un appid :
        // seuls les jeux entrent dans la bibliothèque.
        let game = r#"{"1017900":{"success":true,"data":{"type":"game","name":"Age of Empires: Definitive Edition"}}}"#;
        assert_eq!(
            parse_store_app(1017900, game),
            Some((
                "Age of Empires: Definitive Edition".to_string(),
                "game".to_string()
            ))
        );
        let dlc = r#"{"629330":{"success":true,"data":{"type":"dlc","name":"Fight Songs"}}}"#;
        assert_eq!(
            parse_store_app(629330, dlc),
            Some(("Fight Songs".to_string(), "dlc".to_string()))
        );
        // Un outil n'est pas au magasin : rien à ajouter, rien à nommer.
        assert_eq!(parse_store_app(228980, r#"{"228980":{"success":false}}"#), None);
    }

    #[test]
    fn truncated_batches_are_requested_again_in_smaller_lots() {
        // La réponse du CM est découpée : ce qu'elle n'a pas rendu est
        // identifié précisément, puis redemandé par lots plus petits.
        assert_eq!(missing_ids(&[10, 20, 30], &[10, 30]), vec![20]);
        assert_eq!(missing_ids(&[10, 20], &[10, 20]), Vec::<u32>::new());
        assert!(PICS_INFO_BATCH < PICS_BATCH);
        assert_eq!(smaller_batch(PICS_INFO_BATCH), Some(12));
        assert_eq!(smaller_batch(3), Some(1));
        // L'appel unitaire a déjà été tenté : on abandonne au lieu de boucler.
        assert_eq!(smaller_batch(1), None);
    }

    #[test]
    fn binary_package_reads_the_second_root() {
        // Même disposition que les paquets PICS réels : en-tête, racine
        // « 0 » de métadonnées, fin, puis racine « appids ».
        let mut data = Vec::new();
        data.extend_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(&[0]);
        data.extend_from_slice(b"0\0");
        data.push(2);
        data.extend_from_slice(b"packageid\0");
        data.extend_from_slice(&33530i32.to_le_bytes());
        data.push(8);
        data.push(0);
        data.extend_from_slice(b"appids\0");
        for (index, appid) in [440u32, 730, 440].iter().enumerate() {
            data.push(2);
            data.extend_from_slice(format!("{index}\0").as_bytes());
            data.extend_from_slice(&(*appid as i32).to_le_bytes());
        }
        data.push(8);
        assert_eq!(package_appids(&data), vec![440, 730]);
    }

    #[test]
    fn package_without_appids_is_empty() {
        assert!(package_appids(br#""package" { "packageid" "1" }"#).is_empty());
        assert!(package_appids(&[1, 0, 0, 0, 0, b'x', 0, 8]).is_empty());
    }

    #[test]
    fn installdir_stays_inside_the_library() {
        let meta = AppMeta {
            name: "Portal 2".into(),
            installdir: "../../etc".into(),
            kind: "Game".into(),
        };
        assert_eq!(safe_installdir(&meta, 620), "etc");
        let meta = AppMeta {
            name: "Portal 2".into(),
            installdir: String::new(),
            kind: "Game".into(),
        };
        assert_eq!(safe_installdir(&meta, 620), "portal-2");
    }

    const PORTAL: &str = r#"
"appinfo" {
    "common" {
        "name" "Portal 2"
        "type" "game"
        "oslist" "windows,macos,linux"
        "controller_support" "full"
        "metacritic_score" "95"
        "metacritic_fullurl" "https://www.metacritic.com/game/pc/portal-2"
        "review_percentage" "98"
        "review_score" "9"
        "steam_release_date" "1301358000"
        "original_release_date" "1301358000"
        "metacritic_name" "Portal 2"
        "primary_genre" "1"
        "osarch" "64"
        "clienticon" "2c9f9c1f6b7a4a1e9f4e2b7d9a1c3e5f7a9b0c1d"
        "content_descriptors" { "0" "2" "1" "5" }
        "workshop_visible" "1"
        "supported_languages" {
            "english" { "supported" "true" "full_audio" "true" }
            "french" { "supported" "true" }
            "german" { "supported" "false" }
        }
        "genres" { "0" "1" "1" "25" }
        "category" { "category_2" "1" "category_22" "1" "category_999" "1" }
        "steam_deck_compatibility" { "category" "3" }
        "associations" {
            "0" { "name" "Valve" "type" "developer" }
            "1" { "name" "Valve" "type" "publisher" }
        }
    }
    "extended" {
        "developer" "Valve"
        "publisher" "Valve, Electronic Arts"
        "homepage" "http://www.thinkwithportals.com/"
        "isfreeapp" "0"
        "listofdlc" "1,2,3"
    }
    "depots" {
        "branches" {
            "beta" { "buildid" "42" "timebuildupdated" "1700000000" }
            "public" { "buildid" "99" "timebuildupdated" "1750000000" }
        }
        "621" {
            "config" { "oslist" "windows" }
            "manifests" { "public" { "gid" "1" "size" "1000" "download" "400" } }
        }
        "622" {
            "config" { "oslist" "linux" }
            "manifests" { "public" { "gid" "2" "size" "900" "download" "350" } }
        }
        "623" {
            "manifests" { "public" { "gid" "3" "size" "100" "download" "50" } }
        }
        "624" {
            "dlcappid" "700"
            "manifests" { "public" { "gid" "4" "size" "999999" "download" "999999" } }
        }
        "228988" {
            "config" { "oslist" "windows" }
            "sharedinstall" "1"
            "manifests" { "public" { "gid" "5" "size" "888888" "download" "888888" } }
        }
    }
    "ufs" { "quota" "1073741824" "maxnumfiles" "1000" }
}
"#;

    #[test]
    fn owned_depots_limit_the_size_and_their_absence_counts_everything() {
        let root = parse_kv(PORTAL.as_bytes()).unwrap();
        // Le compte ne possède que le dépôt Windows : c'est son poids qui
        // s'affiche, et macOS n'annonce plus rien.
        let mut owned = HashSet::new();
        owned.insert(621);
        let details = details_from_pics(&root, 620, Some(&owned), None, false);
        assert_eq!(details.size_windows.unwrap().install, 1000);
        assert!(details.size_macos.is_none());
        // Sans liste connue — jeu prêté par la famille — tout le contenu
        // public compte, sinon la fiche n'affiche aucun bouton de taille.
        let details = details_from_pics(&root, 620, None, None, false);
        assert_eq!(details.size_windows.unwrap().install, 1100);
    }

    #[test]
    fn language_codes_accept_locales_and_steam_names() {
        assert_eq!(language_code("fr_FR.UTF-8"), Some("french"));
        assert_eq!(language_code("fr"), Some("french"));
        assert_eq!(language_code("french"), Some("french"));
        assert_eq!(language_code("pt-BR"), Some("portuguese"));
        assert_eq!(language_code("schinese"), Some("schinese"));
        assert_eq!(language_code("klingon"), None);
    }

    #[test]
    fn the_steam_registry_gives_the_language_steam_would_use() {
        let registry = r#"
"Registry"
{
    "HKCU"
    {
        "Software"
        {
            "Valve"
            {
                "Steam" { "language" "french" "AutoLoginUser" "bob" }
            }
        }
    }
}
"#;
        assert_eq!(language_from_registry(registry), Some("french"));
        // Un registre sans réglage de langue ne tranche rien.
        assert_eq!(language_from_registry("\"Registry\" { }"), None);
    }

    #[test]
    fn the_announced_weight_follows_the_players_language() {
        // Deux doublages, l'anglais et le français : seul celui du joueur
        // compte, et l'anglais sert de repli.
        let root = parse_kv(
            br#"
"appinfo"
{
    "common" { "name" "Double doublage" "oslist" "windows" }
    "depots" {
        "public" { "buildid" "7" }
        "910" { "manifests" { "public" { "gid" "1" "size" "1000" "download" "900" } } }
        "911" {
            "config" { "language" "english" }
            "manifests" { "public" { "gid" "2" "size" "2000" "download" "1800" } }
        }
        "912" {
            "config" { "language" "french" }
            "manifests" { "public" { "gid" "3" "size" "2500" "download" "2300" } }
        }
    }
}
"#,
        )
        .unwrap();
        let app = app_section(&root);
        // Langue du client publiée par le jeu : c'est elle qui pèse.
        let size = platform_size(app, "windows", None, None, Some("french"), false).unwrap();
        assert_eq!(size.install, 3500);
        assert_eq!(size.download, 3200);
        // Langue absente du jeu : repli sur l'anglais.
        let size = platform_size(app, "windows", None, None, Some("japanese"), false).unwrap();
        assert_eq!(size.install, 3000);
        // Sans langue connue, même repli.
        let size = platform_size(app, "windows", None, None, None, false).unwrap();
        assert_eq!(size.install, 3000);
    }

    #[test]
    fn refused_depots_leave_the_size_of_the_owned_edition() {
        // Deux dépôts sans plateforme ni langue, comme une édition publiée en
        // double : le protocole refuse la clé du second, qui ne compte plus.
        let root = parse_kv(
            br#"
"appinfo"
{
    "appid" "900"
    "common" { "name" "Double edition" "oslist" "windows" }
    "depots" {
        "public" { "buildid" "7" }
        "901" {
            "manifests" { "public" { "gid" "1" "size" "80000" "download" "70000" } }
        }
        "902" {
            "manifests" { "public" { "gid" "2" "size" "81000" "download" "71000" } }
        }
    }
}
"#,
        )
        .unwrap();
        // Sans refus connu, la fiche additionne les deux éditions.
        let details = details_from_pics(&root, 900, None, None, false);
        assert_eq!(details.size_windows.unwrap().install, 161_000);
        // Avec le refus du dépôt 902, elle annonce le poids de la seule
        // édition que le compte peut installer.
        let mut refused = HashSet::new();
        refused.insert(902);
        let details = details_from_pics(&root, 900, None, Some(&refused), false);
        assert_eq!(details.size_windows.unwrap().install, 80_000);
        assert_eq!(details.size_windows.unwrap().download, 70_000);
    }

    #[test]
    fn details_cover_every_pics_field() {
        let root = parse_kv(PORTAL.as_bytes()).unwrap();
        let details = details_from_pics(&root, 620, None, None, false);
        assert_eq!(details.developers, vec!["Valve"]);
        assert_eq!(details.publishers, vec!["Valve", "Electronic Arts"]);
        assert_eq!(details.genres, vec!["Action", "Aventure"]);
        assert_eq!(details.categories, vec!["Solo", "Succès Steam"]);
        assert_eq!(details.platforms, vec!["Windows", "macOS", "Linux"]);
        assert_eq!(details.release_date, Some(1_301_358_000));
        assert_eq!(details.controller, "Complète");
        assert_eq!(details.metacritic, Some(95));
        assert_eq!(details.review_percent, Some(98));
        assert_eq!(details.review_score, Some(9));
        assert_eq!(details.languages, vec!["Anglais", "Français"]);
        assert_eq!(details.full_audio, vec!["Anglais"]);
        assert_eq!(details.deck, "Vérifié");
        assert!(details.workshop);
        assert!(!details.free);
        assert_eq!(details.dlc_count, 3);
        assert_eq!(details.dlc_ids, vec![1, 2, 3]);
        assert_eq!(details.cloud_quota, Some(1_073_741_824));
        assert_eq!(details.cloud_files, Some(1000));
        assert_eq!(details.homepage, "http://www.thinkwithportals.com/");
        assert_eq!(details.app_type, "game");
        assert_eq!(details.os_arch, "64");
        assert_eq!(details.original_release_date, Some(1_301_358_000));
        assert_eq!(details.primary_genre, "Action");
        assert_eq!(details.metacritic_name, "Portal 2");
        assert_eq!(
            details.descriptors,
            vec!["Violence ou gore fréquents", "Contenu mature général"]
        );
        assert_eq!(details.build_id, Some(99));
        assert_eq!(details.build_time, Some(1_750_000_000));
        assert_eq!(details.branches, vec!["beta", "public"]);
        assert_eq!(details.depot_count, 5);
        // 1000 + 100 de contenu commun : le DLC et le dépôt partagé sont exclus.
        assert_eq!(details.size_windows.unwrap().install, 1100);
        assert_eq!(details.size_windows.unwrap().download, 450);
        assert_eq!(details.size_linux.unwrap().install, 1000);
        assert_eq!(details.size_linux.unwrap().download, 400);
        // Aucun dépôt propre à macOS : seul le contenu commun compte.
        assert_eq!(details.size_macos.unwrap().install, 100);
        assert_eq!(details.size_macos.unwrap().download, 50);
        assert_eq!(
            details.header_image,
            "https://cdn.cloudflare.steamstatic.com/steam/apps/620/header.jpg"
        );
        assert_eq!(
            details.capsule_image,
            "https://cdn.cloudflare.steamstatic.com/steam/apps/620/capsule_231x87.jpg"
        );
        assert!(details.icon_image.ends_with(
            "/steamcommunity/public/images/apps/620/2c9f9c1f6b7a4a1e9f4e2b7d9a1c3e5f7a9b0c1d.jpg"
        ));
        assert!(!details.is_empty());
    }
}
