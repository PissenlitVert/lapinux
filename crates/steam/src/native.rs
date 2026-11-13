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
use bytes::Bytes;
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
use steamroom::types::key_value::{KeyValue, KvValue};
use steamroom_client::download::{BoxError, ChunkFetcher, DepotJob};
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

#[path = "native_metadata.rs"]
mod native_metadata;
use native_metadata::{
    app_section, details_from_pics, language_label, meta_from_pics, number, package_appids,
    package_section_ids, parse_kv, platform_size,
};
pub(crate) use native_metadata::{refused_depots, DEPOT_KEY_BUDGET};
#[path = "native_pics.rs"]
mod native_pics;
use native_pics::{app_infos, package_infos, product};
#[cfg(test)]
use native_pics::{missing_ids, smaller_batch};
#[path = "native_catalog.rs"]
mod native_catalog;
pub use native_catalog::catalog;
pub(crate) use native_catalog::web_access_token;
#[cfg(test)]
use native_catalog::{
    parse_shared_apps, parse_shared_dlcs, parse_store_app, parse_store_package,
    parse_store_package_name, site_package_dates,
};
#[path = "native_depots.rs"]
mod native_depots;
pub use native_depots::{
    default_language, default_language_label, language_override, package_app, search_appid,
    set_language_override, steam_language,
};
use native_depots::{directory_size, dlc_sizes_of, pick_platform, safe_installdir};
pub(crate) use native_depots::{dlc_size, dlc_sizes};
#[cfg(test)]
use native_depots::{language_code, language_from_registry};
#[path = "native_install.rs"]
mod native_install;
pub use native_install::install;
#[cfg(test)]
use native_install::CountingFetcher;
#[path = "native_art.rs"]
mod native_art;
pub use native_art::store_art;

#[cfg(test)]
mod tests {
    use super::native_catalog::family_owners;
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
            (
                "Among the Sleep (Global) - Retail".to_string(),
                1_793_448_000,
            ),
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
            tokio::spawn(async move { fetcher.fetch_chunk(DepotId(1), &ChunkId([7u8; 20])).await })
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
            tokio::spawn(async move { fetcher.fetch_chunk(DepotId(1), &ChunkId([3u8; 20])).await })
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
        let result = fetcher.fetch_chunk(DepotId(1), &ChunkId([9u8; 20])).await;
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
        assert_eq!(
            chunk_downloads(&DownloadLimit::unlimited()),
            CHUNK_DOWNLOADS
        );
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
                  "app_type": 1, "exclude_reason": 0,
                  "rt_time_acquired": 1498357027 },
                { "appid": 220, "name": "Half-Life 2",
                  "app_type": 1, "exclude_reason": 0,
                  "rt_time_acquired": 0 },
                { "appid": 3990820, "name": "Resident Evil Requiem - Deluxe Kit",
                  "app_type": 32, "exclude_reason": 0 },
                { "appid": 620, "name": "Portal 2",
                  "app_type": 1, "exclude_reason": 3 }
            ] }
        }"#;
        assert_eq!(
            parse_shared_apps(body),
            vec![
                (
                    3764200,
                    "Resident Evil Requiem".to_string(),
                    Some(1_498_357_027)
                ),
                (220, "Half-Life 2".to_string(), None)
            ]
        );
        // Le même contenu additionnel garde son propriétaire : la fiche peut
        // afficher les DLC de la famille et non seulement ceux du compte.
        assert_eq!(
            parse_shared_dlcs(body),
            vec![crate::catalog::FamilyDlc {
                appid: 3990820,
                name: "Resident Evil Requiem - Deluxe Kit".to_string(),
                owners: Vec::new(),
            }]
        );
        // Une réponse illisible ou vide n'ajoute rien à la bibliothèque.
        assert!(parse_shared_apps("pas du json").is_empty());
        assert!(parse_shared_apps(r#"{"response":{}}"#).is_empty());
        assert!(parse_shared_apps(r#"{"response":{"apps":[]}}"#).is_empty());
    }

    #[test]
    fn a_shared_dlc_keeps_its_owners() {
        let body = r#"{
            "response": { "apps": [
                { "appid": 3990820, "name": "Deluxe Kit", "app_type": 32,
                  "exclude_reason": 0, "owner_steamids": ["76561198000000001", 76561198000000002] }
            ] }
        }"#;
        assert_eq!(
            parse_shared_dlcs(body),
            vec![crate::catalog::FamilyDlc {
                appid: 3990820,
                name: "Deluxe Kit".to_string(),
                owners: vec![76561198000000001, 76561198000000002],
            }]
        );
        // Le type 4 d'un outil ne doit pas devenir un contenu du jeu.
        let tool = r#"{
            "response": { "apps": [
                { "appid": 563, "name": "Left 4 Dead 2 Authoring Tools", "app_type": 4,
                  "exclude_reason": 0, "owner_steamids": ["76561198000000001"] }
            ] }
        }"#;
        assert!(parse_shared_dlcs(tool).is_empty());
    }

    #[test]
    fn a_family_dlc_is_attributed_to_every_owner_of_its_package() {
        // Un paquet de contenu peut appartenir à deux membres ; un appid qui
        // n'est porté par aucun paquet familial n'a pas de propriétaire.
        let app_packages = std::collections::HashMap::from([
            (3_990_820u32, vec![17_569u32, 88_000]),
            (239_140u32, vec![44_000]),
        ]);
        let family = std::collections::HashMap::from([
            (17_569u32, vec![76_561_197_960_265_728 + 485_264_600u64]),
            (88_000, vec![76_561_197_960_265_728 + 990_333_947u64]),
        ]);
        assert_eq!(
            family_owners(3_990_820, &app_packages, &family),
            vec![76_561_198_445_530_328, 76_561_198_950_599_675]
        );
        assert!(family_owners(239_140, &app_packages, &family).is_empty());
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
                "game".to_string(),
                Vec::new()
            ))
        );
        // La boutique publie aussi les DLC du jeu : pour un jeu que PICS
        // refuse de décrire, c'est la seule liste qui relie les contenus
        // partagés au jeu.
        let with_dlc = r#"{"239140":{"success":true,"data":{"type":"game","name":"Dying Light","dlc":[239150,239160]}}}"#;
        assert_eq!(
            parse_store_app(239140, with_dlc),
            Some((
                "Dying Light".to_string(),
                "game".to_string(),
                vec![239150, 239160]
            ))
        );
        let dlc = r#"{"629330":{"success":true,"data":{"type":"dlc","name":"Fight Songs"}}}"#;
        assert_eq!(
            parse_store_app(629330, dlc),
            Some(("Fight Songs".to_string(), "dlc".to_string(), Vec::new()))
        );
        // Un outil n'est pas au magasin : rien à ajouter, rien à nommer.
        assert_eq!(
            parse_store_app(228980, r#"{"228980":{"success":false}}"#),
            None
        );
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
