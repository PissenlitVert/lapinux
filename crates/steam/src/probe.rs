//! Lapinux — hôte natif expérimental autour de `steamclient.so`.
//!
//! Lapinux charge la bibliothèque du client Steam de Valve — celle que
//! Steam installe déjà — dans son propre processus, obtient ses interfaces,
//! puis ouvre une session locale sans interface graphique. Il répond à une
//! question mesurable : jusqu'où l'ABI de Valve coopère-t-elle hors du client ?
//!
//! Le module porte la sonde (lecture seule), l'hôte (session) et le backend
//! `steamclient` de la commande `lapinux-probe account` ; l'authentification réelle
//! est assurée par le client CM voisin ([`crate::cm`]).
//!
//! Rien n'est contourné : la bibliothèque n'est pas modifiée, aucun mot de
//! passe n'est lu et rien n'est distribué.

use crate::account::{Account, Backend};
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::path::{Path, PathBuf};

/// Nom de la commande et du processus hôte.
pub const COMMAND: &str = "lapinux-probe";
/// Nom du processus enfant d'exploration (visible dans `ps`).
pub const HOST_PROCESS: &str = "lapinux-host";

/// Versions d'interface essayées, de la plus récente à la plus ancienne.
/// Valve incrémente ce numéro à chaque rupture d'ABI : en interroger plusieurs
/// évite de dépendre d'une version codée en dur dans le binaire.
pub const STEAMCLIENT_VERSIONS: &[&str] = &[
    "SteamClient023",
    "SteamClient022",
    "SteamClient021",
    "SteamClient020",
    "SteamClient019",
    "SteamClient018",
    "SteamClient017",
];

/// Types de compte essayés pour une session locale, du plus complet au plus
/// léger : individu (1), utilisateur anonyme (10), serveur anonyme (4).
///
/// Le type 1 est le seul à exposer `ISteamUser` (donc SteamID et état de
/// connexion). Il initialise la pile vidéo : il ne tient que si le runtime de
/// Valve est complet (voir `compat_dir` et `zlib_preload`). Les types anonymes
/// servent de repli quand ce runtime manque.
pub const LOCAL_ACCOUNT_TYPES: [u32; 3] = [1, 10, 4];

/// Versions d'interface utilisateur essayées, de la plus récente à la plus
/// ancienne.
pub const STEAMUSER_VERSIONS: &[&str] = &["SteamUser023", "SteamUser022", "SteamUser021"];
/// Version de l'interface applications (souscription, DLC, dossier installé).
pub const STEAMAPPS_VERSION: &str = "STEAMAPPS_INTERFACE_VERSION008";

/// Index de `ISteamClient::CreateSteamPipe` dans la table virtuelle
/// (`isteamclient.h`, première méthode de l'interface).
const VT_CREATE_STEAM_PIPE: usize = 0;
/// Index de `ISteamClient::ConnectToGlobalUser`.
const VT_CONNECT_TO_GLOBAL_USER: usize = 2;
/// Index de `ISteamClient::CreateLocalUser`.
const VT_CREATE_LOCAL_USER: usize = 3;
/// Index de `ISteamClient::GetISteamUser`.
const VT_GET_STEAM_USER: usize = 5;
/// Index de `ISteamClient::GetISteamGenericInterface`.
const VT_GENERIC_INTERFACE: usize = 12;

#[derive(Debug, Clone)]
pub struct InterfaceHit {
    pub version: String,
    pub pointer: usize,
    /// Résultat de `CreateSteamPipe` sur cet objet (0 = refusé).
    pub pipe: Option<u32>,
}

#[derive(Debug, Clone, Default)]
pub struct Report {
    pub library: Option<PathBuf>,
    pub size: Option<u64>,
    pub loaded: bool,
    pub error: Option<String>,
    pub interfaces: Vec<InterfaceHit>,
    pub pipe: Option<u32>,
    pub global_user: Option<u32>,
    pub local_user: Option<u32>,
    /// Type de compte ayant permis la session locale.
    pub account: Option<u32>,
    /// `ISteamUser` obtenu pour cette session, s'il existe.
    pub user_interface: Option<String>,
    /// SteamID rendu par le moteur (`None` si l'interface est absente).
    pub steam_id: Option<u64>,
    /// `BLoggedOn` : un compte est-il réellement authentifié ?
    pub logged_on: Option<bool>,
    /// `BIsSubscribed` (`ISteamApps`) : le compte est-il abonné à quelque chose ?
    pub subscribed: Option<bool>,
    pub runtime: Vec<(String, bool)>,
    pub notes: Vec<String>,
}

impl Report {
    /// Version d'interface la plus récente qui a répondu.
    pub fn interface(&self) -> Option<&InterfaceHit> {
        self.interfaces.first()
    }

    /// Première interface ayant accepté d'ouvrir un pipe.
    pub fn piped(&self) -> Option<&InterfaceHit> {
        self.interfaces.iter().find(|hit| hit.pipe.is_some_and(|p| p != 0))
    }

    /// Étape atteinte, pour un affichage lisible.
    pub fn stage(&self) -> &'static str {
        if self.error.is_some() {
            "bibliothèque impossible à charger"
        } else if self.global_user.is_some() {
            "session globale connectée"
        } else if self.local_user.is_some() {
            "session locale ouverte, pas de compte connecté"
        } else if self.pipe.is_some() {
            "pipe ouvert, utilisateur global absent"
        } else if !self.interfaces.is_empty() {
            "interfaces obtenues, pipe refusé"
        } else if self.loaded {
            "bibliothèque chargée, aucune interface reconnue"
        } else {
            "non testée"
        }
    }
}

/// Bibliothèques candidates, dans l'ordre de préférence : celles d'un client
/// Steam installé sur la machine, jamais un outil téléchargé par Lapinux.
pub fn candidates(_data_dir: &Path, home: &Path) -> Vec<PathBuf> {
    let mut list = Vec::new();
    // Surcharge d'expérimentation : indispensable pour comparer deux arbres
    // sans recompiler.
    if let Some(custom) = lapinux_core::env_var_os("STEAMCLIENT") {
        list.push(PathBuf::from(custom));
    }
    list.extend([
        home.join(".local/share/Steam/steamrt64/steamclient.so"),
        home.join(".local/share/Steam/linux64/steamclient.so"),
        home.join(".steam/steam/steamrt64/steamclient.so"),
    ]);
    list
}

/// Bribes de runtime que le moteur cherche à côté de la bibliothèque.
/// Sans elles il s'initialise à moitié — et le fait savoir très brutalement.
pub const RUNTIME_PIECES: &[&str] = &[
    "ubuntu12_64/gldriverquery",
    "ubuntu12_64/vulkandriverquery",
    "ubuntu12_32/gldriverquery",
    "ubuntu12_32/vulkandriverquery",
];

/// Vérifie la présence du runtime attendu par le moteur, relativement à la
/// racine de la bibliothèque (`linux64/..`).
pub fn runtime_pieces(library: &Path) -> Vec<(String, bool)> {
    let root = library
        .parent()
        .and_then(Path::parent)
        .unwrap_or(Path::new("."));
    RUNTIME_PIECES
        .iter()
        .map(|piece| ((*piece).to_string(), root.join(piece).exists()))
        .collect()
}

/// Vrai si le runtime minimal est présent.
pub fn runtime_complete(library: &Path) -> bool {
    runtime_pieces(library).iter().all(|(_, ok)| *ok)
}

/// Répertoires de runtime d'un client Steam installé, utilisés pour donner au
/// moteur ses propres `libav*` (les symboles `av_mallocz_tracked` n'existent
/// que dans les builds de Valve).
pub fn runtime_dirs(home: &Path) -> Vec<PathBuf> {
    [
        home.join(".local/share/Steam/ubuntu12_64"),
        home.join(".local/share/Steam/steamrt64"),
    ]
    .into_iter()
    .filter(|path| path.is_dir())
    .collect()
}

/// Noms de bibliothèques que la distribution ne fournit pas sous le nom
/// attendu par le runtime de Valve (`libvideo.so` réclame `libbz2.so.1.0`,
/// Fedora n'a que `libbz2.so.1`). On pointe vers la bibliothèque système.
pub const COMPAT_LIBS: &[(&str, &[&str])] = &[(
    "libbz2.so.1.0",
    &[
        "/lib64/libbz2.so.1",
        "/usr/lib64/libbz2.so.1",
        "/lib/x86_64-linux-gnu/libbz2.so.1",
        "/usr/lib/libbz2.so.1",
        "/lib/libbz2.so.1",
    ],
)];

/// Prépare un répertoire de compatibilité et renvoie son chemin s'il a servi.
/// Rien n'est fait si un runtime Valve fournit déjà le nom attendu.
pub fn compat_dir(data_dir: &Path, runtime: &[PathBuf]) -> Option<PathBuf> {
    let missing: Vec<(&str, &[&str])> = COMPAT_LIBS
        .iter()
        .filter(|(name, _)| !runtime.iter().any(|dir| dir.join(name).exists()))
        .copied()
        .collect();
    if missing.is_empty() {
        return None;
    }
    let dir = data_dir.join("compat");
    std::fs::create_dir_all(&dir).ok()?;
    let mut useful = false;
    for (name, candidates) in missing {
        let link = dir.join(name);
        if link.exists() {
            // Déjà préparé lors d'un lancement précédent : il faut malgré tout
            // le remettre en tête du chemin de recherche.
            useful = true;
            continue;
        }
        let Some(target) = candidates.iter().map(Path::new).find(|path| path.exists()) else {
            continue;
        };
        if std::os::unix::fs::symlink(target, &link).is_ok() {
            useful = true;
        }
    }
    useful.then_some(dir)
}

/// zlib doit être global : le `libavfilter` de Valve appelle `inflate` sans le
/// déclarer dans ses dépendances dynamiques. Sans préchargement, `libvideo.so`
/// ne s'initialise jamais et le moteur part en assertion.
pub fn zlib_preload() -> Option<PathBuf> {
    [
        "/lib64/libz.so.1",
        "/usr/lib64/libz.so.1",
        "/lib/x86_64-linux-gnu/libz.so.1",
        "/usr/lib/libz.so.1",
        "/lib/libz.so.1",
    ]
    .iter()
    .map(PathBuf::from)
    .find(|path| path.exists())
}

type CreateInterfaceFn = unsafe extern "C" fn(*const c_char, *mut c_int) -> *mut c_void;

/// Charge la première bibliothèque disponible et déroule la sonde.
///
/// `session` tente en plus `CreateLocalUser` ; `account` restreint l'essai à un
/// seul type de compte (None = tous, dans l'ordre de `LOCAL_ACCOUNT_TYPES`).
pub fn probe(candidates: &[PathBuf], session: bool, account: Option<u32>) -> Report {
    let mut report = Report::default();

    let library = candidates.iter().find(|path| path.is_file());
    let Some(library) = library else {
        report.error = Some("aucun steamclient.so trouvé (client Steam absent)".into());
        return report;
    };
    report.library = Some(library.clone());
    report.size = std::fs::metadata(library).ok().map(|m| m.len());
    report.runtime = runtime_pieces(library);
    let missing: Vec<&str> = report
        .runtime
        .iter()
        .filter(|(_, ok)| !ok)
        .map(|(name, _)| name.as_str())
        .collect();
    if !missing.is_empty() {
        report.notes.push(format!(
            "runtime incomplet : {} absent(s)",
            missing.join(", ")
        ));
    }

    // Le moteur de Valve charge ses voisines (crashhandler.so, libtier0_s.so)
    // depuis son propre répertoire : on le signale, la sonde n'y touche pas.
    let parent = library.parent().unwrap_or(Path::new("."));
    report
        .notes
        .push(format!("répertoire de la bibliothèque : {}", parent.display()));

    let handle = match unsafe { dlopen(library) } {
        Ok(handle) => handle,
        Err(message) => {
            report.error = Some(message);
            return report;
        }
    };
    report.loaded = true;

    let symbol = match unsafe { dlsym(handle, b"CreateInterface\0") } {
        Ok(symbol) => symbol,
        Err(message) => {
            report.error = Some(message);
            return report;
        }
    };
    // SAFETY : `CreateInterface` a la signature C documentée par Valve
    // (`const char*`, `int*`) et reste valide tant que `handle` est ouvert.
    let create_interface: CreateInterfaceFn = unsafe { std::mem::transmute(symbol) };

    for version in STEAMCLIENT_VERSIONS {
        let Ok(name) = CString::new(*version) else {
            continue;
        };
        let mut code: c_int = 0;
        let pointer = unsafe { create_interface(name.as_ptr(), &mut code) };
        if !pointer.is_null() {
            // Chaque version peut exposer une table virtuelle différente :
            // on tente le pipe sur chacune plutôt que sur la plus récente.
            let pipe = unsafe { call_no_arg(pointer, VT_CREATE_STEAM_PIPE) };
            report.interfaces.push(InterfaceHit {
                version: (*version).to_string(),
                pointer: pointer as usize,
                pipe: pipe.filter(|p| *p != 0),
            });
        } else {
            report
                .notes
                .push(format!("{version} : absent (code {code})"));
        }
    }

    let Some(hit) = report.piped().cloned() else {
        report
            .notes
            .push("CreateSteamPipe a renvoyé 0 sur toutes les versions".into());
        if session {
            try_local_user(&mut report, account);
        }
        return report;
    };
    let object = hit.pointer as *mut c_void;
    let Some(pipe) = hit.pipe else {
        return report;
    };
    report.pipe = Some(pipe);

    // `ConnectToGlobalUser` exige un client Steam vivant : son échec est
    // l'information la plus utile de la sonde.
    match unsafe { call_pipe(object, VT_CONNECT_TO_GLOBAL_USER, pipe) } {
        Some(user) if user != 0 => report.global_user = Some(user),
        _ => report
            .notes
            .push("ConnectToGlobalUser a refusé : aucune session globale".into()),
    }

    report
}

/// Tente d'ouvrir une session locale : pipe + utilisateur créés sans client.
fn try_local_user(report: &mut Report, account: Option<u32>) {
    let Some(hit) = report.interface().cloned() else {
        return;
    };
    let object = hit.pointer as *mut c_void;
    let accounts: Vec<u32> = match account {
        Some(account) => vec![account],
        None => LOCAL_ACCOUNT_TYPES.to_vec(),
    };
    for account in accounts {
        let (user, pipe) = unsafe { call_create_local_user(object, VT_CREATE_LOCAL_USER, account) };
        if pipe != 0 && user != 0 {
            report.pipe = Some(pipe);
            report.local_user = Some(user);
            report.account = Some(account);
            report.notes.push(format!(
                "session locale obtenue (type de compte {account}, pipe {pipe})"
            ));
            probe_account(report, object, pipe, user);
            return;
        }
        report.notes.push(format!(
            "CreateLocalUser(type {account}) : user={user}, pipe={pipe}"
        ));
    }
}

/// Interroge l'état du compte sur une session locale : `ISteamUser` donne le
/// SteamID et `BLoggedOn`, `ISteamApps` donne la souscription. Ces deux
/// interfaces sont publiques ; elles ne portent aucune méthode de connexion.
fn probe_account(report: &mut Report, object: *mut c_void, pipe: u32, user: u32) {
    let Some((version, interface)) =
        (unsafe { get_interface(object, VT_GET_STEAM_USER, user, pipe, STEAMUSER_VERSIONS) })
    else {
        report
            .notes
            .push("ISteamUser absent pour ce type de compte".into());
        return;
    };
    report.user_interface = Some(version.to_string());
    // `ISteamUser` : BLoggedOn (index 1), GetSteamID (index 2).
    report.logged_on = unsafe { call_bool(interface, 1) };
    report.steam_id = unsafe { call_u64(interface, 2) };

    if let Some(apps) = unsafe {
        get_generic_interface(object, VT_GENERIC_INTERFACE, user, pipe, STEAMAPPS_VERSION)
    } {
        // `ISteamApps` : BIsSubscribed (index 0).
        report.subscribed = unsafe { call_bool(apps, 0) };
        report.notes.push("ISteamApps obtenu (souscription lisible)".into());
    } else {
        report.notes.push(format!("{STEAMAPPS_VERSION} absent"));
    }
    report.notes.push(
        "aucune interface de connexion : IClientUser n'est pas exposé (CreateInterface et \
         GetISteamGenericInterface renvoient null, journal : « Missing interface adapter »)"
            .into(),
    );
}

/// Obtient une interface par `GetISteamUser`, en essayant les versions connues.
unsafe fn get_interface(
    object: *mut c_void,
    index: usize,
    user: u32,
    pipe: u32,
    versions: &'static [&'static str],
) -> Option<(&'static str, *mut c_void)> {
    let entry = vtable_entry(object, index)?;
    let method: unsafe extern "C" fn(*mut c_void, u32, u32, *const c_char) -> *mut c_void =
        std::mem::transmute(entry);
    for version in versions {
        let Ok(name) = CString::new(*version) else {
            continue;
        };
        let interface = method(object, user, pipe, name.as_ptr());
        if !interface.is_null() {
            return Some((version, interface));
        }
    }
    None
}

/// Obtient une interface arbitraire par `GetISteamGenericInterface`.
unsafe fn get_generic_interface(
    object: *mut c_void,
    index: usize,
    user: u32,
    pipe: u32,
    version: &str,
) -> Option<*mut c_void> {
    let entry = vtable_entry(object, index)?;
    let name = CString::new(version).ok()?;
    let method: unsafe extern "C" fn(*mut c_void, u32, u32, *const c_char) -> *mut c_void =
        std::mem::transmute(entry);
    let interface = method(object, user, pipe, name.as_ptr());
    (!interface.is_null()).then_some(interface)
}

unsafe fn call_bool(object: *mut c_void, index: usize) -> Option<bool> {
    let entry = vtable_entry(object, index)?;
    let method: unsafe extern "C" fn(*mut c_void) -> bool = std::mem::transmute(entry);
    Some(method(object))
}

unsafe fn call_u64(object: *mut c_void, index: usize) -> Option<u64> {
    let entry = vtable_entry(object, index)?;
    let method: unsafe extern "C" fn(*mut c_void) -> u64 = std::mem::transmute(entry);
    Some(method(object))
}

unsafe fn dlopen(path: &Path) -> Result<*mut c_void, String> {
    let Ok(c_path) = CString::new(path.as_os_str().to_string_lossy().as_bytes()) else {
        return Err("chemin de bibliothèque invalide".into());
    };
    let handle = libc::dlopen(c_path.as_ptr(), libc::RTLD_NOW | libc::RTLD_GLOBAL);
    if handle.is_null() {
        let detail = dl_error();
        return Err(format!("dlopen : {detail}"));
    }
    Ok(handle)
}

unsafe fn dlsym(handle: *mut c_void, symbol: &[u8]) -> Result<*mut c_void, String> {
    let found = libc::dlsym(handle, symbol.as_ptr() as *const c_char);
    if found.is_null() {
        return Err(format!(
            "symbole {} absent : {}",
            String::from_utf8_lossy(&symbol[..symbol.len() - 1]),
            dl_error()
        ));
    }
    Ok(found)
}

fn dl_error() -> String {
    unsafe {
        let pointer = libc::dlerror();
        if pointer.is_null() {
            "erreur inconnue".to_string()
        } else {
            CStr::from_ptr(pointer).to_string_lossy().to_string()
        }
    }
}

/// Appelle une méthode virtuelle sans argument (`this` implicite) et renvoie
/// son résultat 32 bits. `None` si l'emplacement de la table est vide.
unsafe fn call_no_arg(object: *mut c_void, index: usize) -> Option<u32> {
    let entry = vtable_entry(object, index)?;
    let method: unsafe extern "C" fn(*mut c_void) -> u32 = std::mem::transmute(entry);
    Some(method(object))
}

/// Appelle `CreateLocalUser(HSteamPipe*, EAccountType)` et renvoie
/// `(utilisateur, pipe)`.
unsafe fn call_create_local_user(object: *mut c_void, index: usize, account: u32) -> (u32, u32) {
    let Some(entry) = vtable_entry(object, index) else {
        return (0, 0);
    };
    let method: unsafe extern "C" fn(*mut c_void, *mut u32, u32) -> u32 =
        std::mem::transmute(entry);
    let mut pipe: u32 = 0;
    let user = method(object, &mut pipe, account);
    (user, pipe)
}

/// Appelle une méthode virtuelle prenant un handle et renvoyant un handle.
unsafe fn call_pipe(object: *mut c_void, index: usize, pipe: u32) -> Option<u32> {
    let entry = vtable_entry(object, index)?;
    let method: unsafe extern "C" fn(*mut c_void, u32) -> u32 = std::mem::transmute(entry);
    Some(method(object, pipe))
}

unsafe fn vtable_entry(object: *mut c_void, index: usize) -> Option<*const c_void> {
    if object.is_null() {
        return None;
    }
    let vtable = *(object as *mut *const *const c_void);
    if vtable.is_null() {
        return None;
    }
    let entry = *vtable.add(index);
    (!entry.is_null()).then_some(entry)
}

// ---------------------------------------------------------------------------
// Surface en ligne de commande
// ---------------------------------------------------------------------------

/// Point d'entrée commun à `lapinux-probe` et à `lapinux cli steam-probe`.
///
/// Sous-commandes : `probe [--session]` (défaut) et `host [--account N]`
/// (interne, exécuté dans le processus enfant).
/// Backend du moteur de Valve : ouvre une session locale, sans
/// authentification (le moteur ne sait pas s'authentifier hors du client).
pub struct SteamclientBackend {
    libraries: Vec<PathBuf>,
    account: Option<u32>,
}

impl SteamclientBackend {
    pub fn new(libraries: Vec<PathBuf>, account: Option<u32>) -> Self {
        Self {
            libraries,
            account,
        }
    }
}

impl Backend for SteamclientBackend {
    fn name(&self) -> &'static str {
        "steamclient"
    }

    fn sign_in(
        &mut self,
        _prompt: &mut dyn FnMut(crate::account::Prompt, bool) -> Result<String, String>,
    ) -> Result<Account, String> {
        let report = probe(&self.libraries, false, None);
        if report.interfaces.is_empty() {
            return Err(report.error.unwrap_or_else(|| {
                "steamclient.so introuvable ou versions d'interface inconnues".to_string()
            }));
        }
        // Le type 1 est le seul à exposer ISteamUser, donc le SteamID ; il est
        // le candidat par défaut, comme pour la sonde `--session`.
        let account_type = self.account.or(Some(LOCAL_ACCOUNT_TYPES[0]));
        let fields = run_host(&report, account_type)?;
        let get = |key: &str| {
            fields
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.clone())
        };
        if let Some(error) = get("erreur") {
            return Err(format!("le moteur a rendu : {error}"));
        }
        let steam_id = get("steamid").and_then(|value| value.parse().ok());
        if steam_id.is_none() {
            return Err("le moteur n'a pas rendu de SteamID".into());
        }
        let logged_on = get("connecte").as_deref() == Some("true");
        Ok(Account {
            backend: "steamclient",
            steam_id,
            logged_on,
            anonymous: false,
            endpoint: None,
            licenses: None,
            refresh_token: None,
            guard_data: None,
            details: fields,
        })
    }
}

/// `lapinux-probe account` : interroge les backends et rapporte le compte obtenu.
///
/// Sans drapeau, les deux chemins sont essayés : le moteur de Valve d'abord
/// (local, hors ligne), puis le protocole CM (authentification anonyme).
fn account_command(libraries: &[PathBuf], args: &[String]) -> i32 {
    let flag = |name: &str| args.iter().any(|arg| arg == name);
    let value = |name: &str| {
        args.iter()
            .position(|arg| arg == name)
            .and_then(|index| args.get(index + 1))
            .cloned()
    };
    let endpoint = value("--endpoint");
    let account_type = value("--account").and_then(|value| value.parse::<u32>().ok());
    let login = value("--login");

    if flag("--login") && login.is_none() {
        eprintln!("usage : lapinux-probe account --login COMPTE");
        return 2;
    }
    if login.is_some() && flag("--local") {
        eprintln!("--login utilise le client CM : --local est incompatible");
        return 2;
    }

    let mut prompt = |kind: crate::account::Prompt, retry: bool| -> Result<String, String> {
        match kind {
            crate::account::Prompt::EmailCode => {
                if retry {
                    println!("code précédent refusé");
                }
                read_code("code Steam Guard (e-mail) : ")
            }
            crate::account::Prompt::TwoFactor => {
                if retry {
                    println!("code précédent refusé");
                }
                read_code("code Steam Guard (application) : ")
            }
            crate::account::Prompt::DeviceConfirmation => {
                println!("confirme la connexion dans l'application Steam (l'attente peut durer)");
                Ok(String::new())
            }
            crate::account::Prompt::EmailConfirmation => {
                println!("confirme la connexion via le lien envoyé par e-mail");
                Ok(String::new())
            }
        }
    };

    if let Some(name) = login.as_deref().map(str::trim) {
        if name.is_empty() {
            eprintln!("--login attend un nom de compte");
            return 2;
        }
        return login_command(name, endpoint, &mut prompt);
    }

    let mut backends: Vec<Box<dyn Backend>> = Vec::new();
    if !flag("--cm") {
        backends.push(Box::new(SteamclientBackend::new(
            libraries.to_vec(),
            account_type,
        )));
    }
    if !flag("--local") {
        let mut config = crate::cm::Config::from_env();
        if let Some(endpoint) = endpoint {
            config.endpoints = vec![endpoint];
        }
        backends.push(Box::new(
            crate::account::CmBackend::anonymous().with_config(config),
        ));
    }

    println!("comptes Steam :");
    let mut authenticated = false;
    for backend in backends.iter_mut() {
        let name = backend.name();
        match backend.sign_in(&mut prompt) {
            Ok(account) => {
                authenticated |= account.logged_on && account.steam_id.is_some();
                print_account(&account);
            }
            Err(reason) => println!("  {name:<12} échec : {reason}"),
        }
    }
    if authenticated {
        0
    } else {
        1
    }
}

/// Connexion nominative : jeton enregistré d'abord, mot de passe seulement
/// s'il n'y en a pas ou si Steam le refuse. C'est le parcours de SteamCMD.
fn login_command(
    name: &str,
    endpoint: Option<String>,
    prompt: &mut dyn FnMut(crate::account::Prompt, bool) -> Result<String, String>,
) -> i32 {
    let mut config = crate::cm::Config::from_env();
    if let Some(endpoint) = endpoint {
        config.endpoints = vec![endpoint];
    }

    if let Some(saved) = crate::session::load()
        .filter(|saved| saved.account_name.eq_ignore_ascii_case(name))
    {
        println!("jeton Steam enregistré pour {name} : reconnexion sans mot de passe");
        let mut backend =
            crate::account::CmBackend::token(name.to_string(), saved.refresh_token)
                .with_config(config.clone());
        match backend.sign_in(&mut *prompt) {
            Ok(account) => {
                println!("comptes Steam :");
                print_account(&account);
                return 0;
            }
            Err(reason) => {
                println!("  cm           jeton refusé : {reason}");
                println!("               mot de passe demandé");
            }
        }
    }

    println!("connexion à {name} (mot de passe et Steam Guard au terminal)");
    let password = match read_secret(&format!("mot de passe pour {name} : ")) {
        Ok(password) if !password.is_empty() => password,
        Ok(_) => {
            eprintln!("mot de passe vide");
            return 2;
        }
        Err(reason) => {
            eprintln!("{reason}");
            return 2;
        }
    };
    let saved_guard = crate::session::load()
        .filter(|saved| saved.account_name.eq_ignore_ascii_case(name))
        .and_then(|saved| saved.guard_data);
    let mut backend = crate::account::CmBackend::account_with_guard(
        name.to_string(),
        password,
        saved_guard,
    )
    .with_config(config);
    match backend.sign_in(&mut *prompt) {
        Ok(account) => {
            println!("comptes Steam :");
            print_account(&account);
            save_session(name, &account);
            0
        }
        Err(reason) => {
            println!("  cm           échec : {reason}");
            1
        }
    }
}

fn save_session(name: &str, account: &Account) {
    let (Some(steam_id), Some(refresh_token)) =
        (account.steam_id, account.refresh_token.clone())
    else {
        return;
    };
    let saved = crate::session::SavedSession {
        account_name: name.to_string(),
        steam_id,
        refresh_token,
        guard_data: account.guard_data.clone(),
        saved_at: None,
    };
    match crate::session::save(&saved) {
        Ok(path) => println!(
            "               session enregistrée dans {} (0600)",
            path.display()
        ),
        Err(error) => eprintln!("session non enregistrée : {error}"),
    }
}

/// Lit un secret sans l'afficher quand l'entrée est un terminal ; sinon lit
/// une ligne, ce qui permet d'automatiser les tests.
fn read_secret(prompt: &str) -> Result<String, String> {
    use std::io::{BufRead, IsTerminal, Write};
    print!("{prompt}");
    std::io::stdout()
        .flush()
        .map_err(|error| format!("terminal : {error}"))?;
    let stdin = std::io::stdin();
    let mut line = String::new();
    if stdin.is_terminal() {
        let fd = libc::STDIN_FILENO;
        let mut original = std::mem::MaybeUninit::<libc::termios>::uninit();
        if unsafe { libc::tcgetattr(fd, original.as_mut_ptr()) } == 0 {
            let original = unsafe { original.assume_init() };
            let mut hidden = original;
            hidden.c_lflag &= !libc::ECHO;
            unsafe { libc::tcsetattr(fd, libc::TCSANOW, &hidden) };
            let result = stdin.lock().read_line(&mut line);
            unsafe { libc::tcsetattr(fd, libc::TCSANOW, &original) };
            println!();
            result.map_err(|error| format!("lecture : {error}"))?;
            return Ok(trim_newline(line));
        }
    }
    stdin
        .lock()
        .read_line(&mut line)
        .map_err(|error| format!("lecture : {error}"))?;
    Ok(trim_newline(line))
}

fn read_code(prompt: &str) -> Result<String, String> {
    use std::io::{BufRead, Write};
    print!("{prompt}");
    std::io::stdout()
        .flush()
        .map_err(|error| format!("terminal : {error}"))?;
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|error| format!("lecture : {error}"))?;
    let code = trim_newline(line);
    if code.is_empty() {
        return Err("code vide".into());
    }
    Ok(code)
}

fn trim_newline(mut line: String) -> String {
    while line.ends_with('\r') || line.ends_with('\n') {
        line.pop();
    }
    line
}

fn print_account(account: &Account) {
    let steam_id = account
        .steam_id
        .map(|id| id.to_string())
        .unwrap_or_else(|| "—".into());
    println!(
        "  {:<12} steamid={steam_id} connecte={}",
        account.backend,
        if account.logged_on { "oui" } else { "non" }
    );
    if account.anonymous {
        println!("               anonyme=oui");
    }
    if let Some(endpoint) = &account.endpoint {
        println!("               endpoint={endpoint}");
    }
    let keys = [
        "interface",
        "compte",
        "authentification",
        "cell",
        "battement",
        "pays",
    ];
    let pairs: Vec<String> = account
        .details
        .iter()
        .filter(|(key, _)| keys.contains(&key.as_str()))
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    if !pairs.is_empty() {
        println!("               {}", pairs.join(" "));
    }
    if let Some(licenses) = &account.licenses {
        let preview: Vec<String> = licenses.iter().take(8).map(u32::to_string).collect();
        println!(
            "               licences={} ({}{})",
            licenses.len(),
            preview.join(", "),
            if licenses.len() > 8 { "…" } else { "" }
        );
    }
}

pub fn cli(args: &[String]) -> i32 {
    // Le module sert deux binaires : `lapinux-probe` (sonde) et `lapinux`
    // relancé comme hôte. La variable d'environnement tranche avant toute
    // analyse d'arguments.
    let forced_host = std::env::var("LAPINUX_MODE").as_deref() == Ok("host");
    let command = if forced_host {
        "host"
    } else {
        match args.first().map(String::as_str) {
            // Un drapeau en tête (`--session`) laisse la sous-commande par défaut.
            None => "probe",
            Some(value) if value.starts_with('-') => "probe",
            Some(value) => value,
        }
    };
    // Les parcours autonomes ne construisent même pas la liste des
    // bibliothèques d'un client installé : ils ne doivent jamais y retomber.
    if command == "runtime" {
        return crate::runtime::cli(&args[1..]);
    }

    let session = args.iter().any(|arg| arg == "--session");
    let account = args
        .iter()
        .position(|arg| arg == "--account")
        .and_then(|index| args.get(index + 1))
        .and_then(|value| value.parse::<u32>().ok());

    let paths = lapinux_core::paths::LapinuxPaths::discover();
    let home = lapinux_core::paths::home_dir();
    let libraries = candidates(&paths.data_dir, &home);

    match command {
        "host" => host(&libraries, account),
        "probe" => probe_command(&libraries, session),
        "account" => account_command(&libraries, args),
        other => {
            eprintln!("commande Lapinux inconnue : {other}");
            eprintln!(
                "usage : {COMMAND} [probe [--session]] | {COMMAND} runtime [--prepare] [--probe] | \
                 {COMMAND} host [--account N] | {COMMAND} account [--login COMPTE] \
                 [--local|--cm] [--endpoint HOST:PORT]"
            );
            2
        }
    }
}

/// Rapport de lecture seule, puis (si demandé) session dans des enfants isolés.
fn probe_command(libraries: &[PathBuf], session: bool) -> i32 {
    let report = probe(libraries, false, None);
    print_report(&report, session);

    if report.interfaces.is_empty() {
        return 1;
    }
    if session {
        println!("\nsession locale (processus isolé) :");
        let mut established = false;
        for account in LOCAL_ACCOUNT_TYPES {
            match run_host(&report, Some(account)) {
                Ok(fields) => {
                    let pairs: Vec<String> = fields
                        .iter()
                        .map(|(key, value)| format!("{key}={value}"))
                        .collect();
                    println!(
                        "  type de compte {account:<2} → {}",
                        if pairs.is_empty() {
                            "rapport vide".to_string()
                        } else {
                            pairs.join(" ")
                        }
                    );
                    established = true;
                    break;
                }
                Err(reason) => println!("  type de compte {account:<2} → {reason}"),
            }
        }
        if !established {
            println!("  aucune session locale n'a survécu");
        }
    }
    0
}

/// Processus enfant : charge le moteur, ouvre la session, rend le rapport et
/// termine sans le laisser démonter ce qu'il n'a pas construit.
fn host(libraries: &[PathBuf], account: Option<u32>) -> i32 {
    // Nom lisible dans `ps`/`top`, en plus de l'argv[0] posé par le parent.
    unsafe {
        let name = c"lapinux-host";
        libc::prctl(libc::PR_SET_NAME, name.as_ptr(), 0, 0, 0);
    }
    let report = probe(libraries, true, account);
    println!("etape={}", report.stage());
    if let Some(pipe) = report.pipe {
        println!("pipe={pipe}");
    }
    if let Some(user) = report.local_user {
        println!("utilisateur={user}");
    }
    if let Some(account) = report.account {
        println!("compte={account}");
    }
    if let Some(interface) = &report.user_interface {
        println!("interface={interface}");
    }
    if let Some(steam_id) = report.steam_id {
        println!("steamid={steam_id}");
    }
    if let Some(logged_on) = report.logged_on {
        println!("connecte={logged_on}");
    }
    if let Some(subscribed) = report.subscribed {
        println!("abonne={subscribed}");
    }
    if let Some(error) = &report.error {
        println!("erreur={error}");
    }
    use std::io::Write;
    let _ = std::io::stdout().flush();
    // Le démontage interne du moteur n'est pas sûr hors du client officiel
    // (`tier0/threadtools.cpp`, `tier1/mempool.cpp`) : on rend la main sans
    // atexit.
    unsafe { libc::_exit(0) }
}

/// Lance l'hôte dans un processus séparé, avec un environnement propre.
fn run_host(report: &Report, account: Option<u32>) -> Result<Vec<(String, String)>, String> {
    use std::process::{Command, Stdio};
    let exe = std::env::current_exe().map_err(|e| format!("binaire introuvable : {e}"))?;
    let home = lapinux_core::paths::home_dir();

    let mut command = Command::new(exe);
    // Nom de processus lisible dans `ps`.
    use std::os::unix::process::CommandExt;
    command.arg0(HOST_PROCESS);
    command.env("LAPINUX_MODE", "host");
    if let Some(account) = account {
        command.args(["--account", &account.to_string()]);
    }
    command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());

    // Le moteur doit voir le runtime de Valve, pas les bibliothèques héritées
    // de l'environnement du lanceur (`/tmp/.mount_*`). L'ordre compte : le
    // répertoire de compatibilité d'abord, puis la bibliothèque, puis Valve.
    let runtime = runtime_dirs(&home);
    let paths = lapinux_core::paths::LapinuxPaths::discover();
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(compat) = compat_dir(&paths.data_dir, &runtime) {
        dirs.push(compat);
    }
    if let Some(library) = &report.library {
        if let Some(parent) = library.parent() {
            dirs.push(parent.to_path_buf());
        }
    }
    dirs.extend(runtime);
    match zlib_preload() {
        // `libavfilter` de Valve appelle `inflate` sans déclarer zlib.
        Some(zlib) => command.env("LD_PRELOAD", zlib),
        None => command.env_remove("LD_PRELOAD"),
    };
    let joined = std::env::join_paths(dirs)
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_default();
    if !joined.is_empty() {
        command.env("LD_LIBRARY_PATH", &joined);
    }

    let output = command
        .output()
        .map_err(|e| format!("lancement impossible : {e}"))?;
    // L'hôte sort volontairement par `_exit` : un code 0 ne prouve donc rien
    // si le moteur a déclenché une assertion juste avant. On la cherche dans
    // le journal, faute de quoi un type 1 cassé passerait pour un succès.
    let stderr = String::from_utf8_lossy(&output.stderr);
    let assert_marker = ["steamp2pvoice", "assert_", "tier0/threadtools"]
        .iter()
        .find(|marker| stderr.contains(*marker));
    let text = String::from_utf8_lossy(&output.stdout);
    let fields: Vec<(String, String)> = text
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
    if output.status.success() && assert_marker.is_none() {
        return Ok(fields);
    }
    if let Some(marker) = assert_marker {
        // La ligne qui porte le marqueur est la seule vraiment informative ;
        // les suivantes sont le rapport de minidump de Valve.
        let detail = stderr
            .lines()
            .find(|line| line.contains(marker))
            .unwrap_or_default()
            .trim();
        return Err(format!(
            "moteur en assertion : {detail} [LD_LIBRARY_PATH={joined}]"
        ));
    }
    let last = fields
        .last()
        .map(|(key, value)| format!("{key} = {value}"))
        .unwrap_or_else(|| "aucune information".into());
    Err(format!(
        "moteur interrompu ({}) après : {last}",
        signal_label(&output.status)
    ))
}

/// Résumé lisible d'une fin anormale (signal 11, code de sortie, etc.).
fn signal_label(status: &std::process::ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;
    match (status.code(), status.signal()) {
        (_, Some(signal)) => format!("signal {signal}"),
        (Some(code), None) => format!("code {code}"),
        _ => "cause inconnue".into(),
    }
}

fn print_report(report: &Report, session: bool) {
    println!("Lapinux — hôte natif Steam (expérimental)\n");
    match (&report.library, report.size) {
        (Some(path), Some(size)) =>        println!(
            "bibliothèque  {}\n              {}",
            path.display(),
            lapinux_core::util::format_bytes(size)
        ),
        (Some(path), None) => println!("bibliothèque  {}", path.display()),
        (None, _) => println!("bibliothèque  aucune"),
    }
    println!("chargée       {}", if report.loaded { "oui" } else { "non" });
    if !report.runtime.is_empty() {
        let present = report.runtime.iter().filter(|(_, ok)| *ok).count();
        println!("runtime       {present}/{} pièces", report.runtime.len());
    }
    if report.interfaces.is_empty() {
        println!("interfaces    aucune");
    } else {
        println!(
            "interfaces    {} version(s) reconnue(s)",
            report.interfaces.len()
        );
        for hit in &report.interfaces {
            println!(
                "  {:<14} pipe {}",
                hit.version,
                hit.pipe
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "—".into())
            );
        }
    }
    println!(
        "pipe          {}",
        report
            .pipe
            .map(|p| p.to_string())
            .unwrap_or_else(|| "—".into())
    );
    println!(
        "session       {}",
        match (report.local_user, report.account) {
            (Some(user), Some(account)) => format!("locale (type {account}, utilisateur {user})"),
            (Some(user), None) => format!("locale (utilisateur {user})"),
            (None, _) if report.global_user.is_some() => {
                format!("globale (utilisateur {})", report.global_user.unwrap_or(0))
            }
            _ => "aucune".to_string(),
        }
    );
    if let Some(steam_id) = report.steam_id {
        println!(
            "compte        SteamID {steam_id} — connecté : {}",
            match report.logged_on {
                Some(true) => "oui",
                Some(false) => "non",
                None => "inconnu",
            }
        );
    }
    println!("étape         {}", report.stage());
    if let Some(error) = &report.error {
        println!("erreur        {error}");
    }
    if !report.notes.is_empty() {
        println!("\nnotes :");
        for note in &report.notes {
            println!("  - {note}");
        }
    }
    if !session {
        println!("\nAstuce : --session ouvre une session locale dans un processus isolé.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_descending() {
        let numbers: Vec<u32> = STEAMCLIENT_VERSIONS
            .iter()
            .filter_map(|v| v.trim_start_matches("SteamClient").parse().ok())
            .collect();
        assert_eq!(numbers.len(), STEAMCLIENT_VERSIONS.len());
        assert!(numbers.windows(2).all(|pair| pair[0] > pair[1]));
    }

    #[test]
    fn account_types_try_the_user_first() {
        // Le type 1 est le seul à exposer ISteamUser : il doit passer d'abord,
        // quitte à retomber sur les types anonymes si le runtime manque.
        assert_eq!(LOCAL_ACCOUNT_TYPES[0], 1);
        assert!(LOCAL_ACCOUNT_TYPES.contains(&10));
        assert_eq!(LOCAL_ACCOUNT_TYPES.last(), Some(&4));
    }

    #[test]
    fn compat_is_skipped_when_the_runtime_provides_the_name() {
        let data = tempfile::tempdir().unwrap();
        let runtime = tempfile::tempdir().unwrap();
        std::fs::write(runtime.path().join("libbz2.so.1.0"), b"").unwrap();
        assert!(compat_dir(data.path(), &[runtime.path().to_path_buf()]).is_none());
    }

    #[test]
    fn compat_dir_stays_useful_on_second_run() {
        let data = tempfile::tempdir().unwrap();
        let runtime = tempfile::tempdir().unwrap();
        let first = compat_dir(data.path(), &[runtime.path().to_path_buf()]);
        let second = compat_dir(data.path(), &[runtime.path().to_path_buf()]);
        assert_eq!(first.is_some(), second.is_some());
        if let Some(dir) = second {
            assert!(dir.join("libbz2.so.1.0").exists());
        }
    }

    #[test]
    fn preloaded_zlib_exists_when_found() {
        if let Some(path) = zlib_preload() {
            assert!(path.is_file(), "{} doit exister", path.display());
        }
    }

    #[test]
    fn candidates_only_look_at_an_installed_steam_client() {
        let list = candidates(Path::new("/data"), Path::new("/home/moi"));
        assert_eq!(
            list[0],
            PathBuf::from("/home/moi/.local/share/Steam/steamrt64/steamclient.so")
        );
        assert!(list.len() >= 3);
        assert!(!list.iter().any(|path| path.to_string_lossy().contains("steamcmd")));
    }

    #[test]
    fn runtime_is_looked_up_next_to_the_library() {
        let dir = tempfile::tempdir().unwrap();
        let linux64 = dir.path().join("linux64");
        std::fs::create_dir_all(&linux64).unwrap();
        let library = linux64.join("steamclient.so");
        std::fs::write(&library, b"x").unwrap();
        assert!(!runtime_complete(&library));

        std::fs::create_dir_all(dir.path().join("ubuntu12_64")).unwrap();
        std::fs::write(dir.path().join("ubuntu12_64/gldriverquery"), b"").unwrap();
        std::fs::write(dir.path().join("ubuntu12_64/vulkandriverquery"), b"").unwrap();
        std::fs::create_dir_all(dir.path().join("ubuntu12_32")).unwrap();
        std::fs::write(dir.path().join("ubuntu12_32/gldriverquery"), b"").unwrap();
        assert!(!runtime_complete(&library));
        std::fs::write(dir.path().join("ubuntu12_32/vulkandriverquery"), b"").unwrap();
        assert!(runtime_complete(&library));
        let pieces = runtime_pieces(&library);
        assert!(pieces.iter().all(|(_, ok)| *ok));
    }

    #[test]
    fn runtime_dirs_only_keep_existing_ones() {
        let home = tempfile::tempdir().unwrap();
        assert!(runtime_dirs(home.path()).is_empty());
        let steam = home.path().join(".local/share/Steam/ubuntu12_64");
        std::fs::create_dir_all(&steam).unwrap();
        let dirs = runtime_dirs(home.path());
        assert_eq!(dirs, vec![steam]);
    }

    #[test]
    fn missing_library_is_reported() {
        let report = probe(&[PathBuf::from("/inexistant/steamclient.so")], false, None);
        assert!(!report.loaded);
        assert!(report
            .error
            .as_ref()
            .is_some_and(|e| e.contains("aucun steamclient.so")));
        assert_eq!(report.stage(), "bibliothèque impossible à charger");
    }

    #[test]
    fn stage_progression() {
        let mut report = Report {
            loaded: true,
            ..Default::default()
        };
        assert_eq!(
            report.stage(),
            "bibliothèque chargée, aucune interface reconnue"
        );
        report.interfaces.push(InterfaceHit {
            version: "SteamClient021".into(),
            pointer: 1,
            pipe: None,
        });
        assert_eq!(report.stage(), "interfaces obtenues, pipe refusé");
        report.interfaces[0].pipe = Some(4);
        report.pipe = Some(7);
        assert_eq!(report.stage(), "pipe ouvert, utilisateur global absent");
        report.local_user = Some(9);
        assert_eq!(
            report.stage(),
            "session locale ouverte, pas de compte connecté"
        );
        report.global_user = Some(3);
        assert_eq!(report.stage(), "session globale connectée");
    }
}
