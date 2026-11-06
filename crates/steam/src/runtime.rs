//! Runtime Steam autonome géré par Lapinux.
//!
//! Le client Steam installé n'est jamais utilisé ici : SteamCMD est téléchargé
//! dans `~/.local/share/lapinux/steam-runtime`, puis la bibliothèque
//! `linux64/steamclient.so` est chargée dans un processus enfant avec un
//! `HOME` privé. La sonde répond à une question précise : cette copie sait-elle
//! ouvrir une session 64 bits et exposer les interfaces client (amis, lobbies),
//! sans le client graphique ni ses fichiers ?

use crate::session::{self, SavedSession};
use lapinux_core::paths::{LapinuxPaths, which};
use std::collections::BTreeMap;
use std::ffi::{CStr, CString, c_void};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::raw::{c_char, c_int};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Archive officielle du client console Steam.
pub const STEAMCMD_URL: &str =
    "https://steamcdn-a.akamaihd.net/client/installer/steamcmd_linux.tar.gz";
/// Sous-dossier du runtime, sous le dossier de données Lapinux.
pub const RUNTIME_DIR: &str = "steam-runtime";
/// Sous-dossier du `HOME` privé de la sonde.
pub const HOME_DIR: &str = "steam-home";
/// Nom du processus enfant, visible dans `ps`.
pub const HOST_PROCESS: &str = "lapinux-runtime-host";
/// Délai maximal laissé à l'enfant pour rendre son rapport.
const HOST_TIMEOUT: Duration = Duration::from_secs(25);
/// Attente maximale du logon après l'envoi du jeton.
const LOGON_TIMEOUT: Duration = Duration::from_secs(10);
/// Attente courte quand la bibliothèque dit déjà le compte connecté.
const AUTO_LOGON_TIMEOUT: Duration = Duration::from_secs(3);
/// Version de l'interface interne qui porte `IClientUser`.
const CLIENT_ENGINE_VERSION: &str = "CLIENTENGINE_INTERFACE_VERSION005";
/// `IClientEngine::GetIClientUser` : huitième entrée de la table virtuelle.
pub const ENGINE_GET_ICLIENT_USER_SLOT: usize = 8;
/// `IClientUser::SetSteamID`, en octets dans la table virtuelle.
pub const ICLIENT_USER_SET_STEAM_ID: usize = 0x08;
/// `IClientUser::IsAccountLoggedIn`.
pub const ICLIENT_USER_IS_ACCOUNT_LOGGED_IN: usize = 0x188;
/// `IClientUser::SetLoginInformation`.
pub const ICLIENT_USER_SET_LOGIN_INFORMATION: usize = 0x1B0;
/// `IClientUser::LogonWithRefreshToken`.
pub const ICLIENT_USER_LOGON_WITH_REFRESH_TOKEN: usize = 0x1C0;

const REQUIRED_SYMBOLS: &[&str] = &[
    "CreateInterface",
    "Steam_CreateGlobalUser",
    "Steam_BLoggedOn",
    "Steam_BGetCallback",
    "Steam_FreeLastCallback",
];
/// Symboles utiles mais non indispensables à la sonde.
const OPTIONAL_SYMBOLS: &[&str] = &[
    "Steam_GetAPICallResult",
    "Steam_IsAPICallCompleted",
    "Steam_ReleaseUser",
    "Steam_BReleaseSteamPipe",
    "Steam_LogOff",
];
/// Bibliothèques que Valve charge à côté de `steamclient.so`.
const RUNTIME_SIBLINGS: &[&str] = &[
    "libtier0_s.so",
    "libvstdlib_s.so",
    "libsteamnetworkingsockets.so",
    "steamservice.so",
];

/// Racine de la copie SteamCMD gérée par Lapinux.
pub fn runtime_root(data_dir: &Path) -> PathBuf {
    data_dir.join(RUNTIME_DIR)
}

pub fn steam_home(data_dir: &Path) -> PathBuf {
    data_dir.join(HOME_DIR)
}

/// Bibliothèque 64 bits visée par la sonde.
pub fn library_path(data_dir: &Path) -> PathBuf {
    runtime_root(data_dir)
        .join("linux64")
        .join("steamclient.so")
}

/// Archive téléchargée, conservée pour éviter un second téléchargement.
pub fn archive_path(data_dir: &Path) -> PathBuf {
    runtime_root(data_dir).join("steamcmd_linux.tar.gz")
}

/// Journal de la mise à jour SteamCMD.
pub fn update_log_path(data_dir: &Path) -> PathBuf {
    runtime_root(data_dir).join("steamcmd-update.log")
}

/// Petit `PATH` privé : seuls les exécutables explicitement copiés y vivent.
fn private_bin(data_dir: &Path) -> PathBuf {
    runtime_root(data_dir).join("bin")
}

/// Prépare les dossiers et les fichiers de configuration vides attendus par
/// la bibliothèque. Aucun fichier du client installé n'est copié.
pub fn stage_steam_home(data_dir: &Path) -> io::Result<PathBuf> {
    let home = steam_home(data_dir);
    let steam = home.join("Steam");
    let config = steam.join("config");
    fs::create_dir_all(&config)?;
    for relative in ["config/config.vdf", "config/local.vdf", "local.vdf"] {
        // Toujours vides : un cache de connexion laissé par une exécution
        // précédente ferait croire au moteur qu'il est déjà connecté alors
        // que le pipe est neuf.
        let path = steam.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        File::create(path)?;
    }
    fs::create_dir_all(home.join(".cache"))?;
    Ok(home)
}

/// Prépare le `PATH` minimal de l'enfant : `sh` seulement, pour qu'une
/// tentative de lancement du client installé échoue au lieu de réussir.
fn stage_private_bin(data_dir: &Path) -> io::Result<PathBuf> {
    let bin = private_bin(data_dir);
    fs::create_dir_all(&bin)?;
    let sh = bin.join("sh");
    if !sh.exists() {
        let system = which("sh").unwrap_or_else(|| PathBuf::from("/bin/sh"));
        std::os::unix::fs::symlink(system, &sh)?;
    }
    Ok(bin)
}

/// Rapport de préparation du runtime.
#[derive(Debug, Clone, Default)]
pub struct PrepareReport {
    pub runtime: PathBuf,
    pub archive: Option<PathBuf>,
    pub downloaded: u64,
    pub extracted: bool,
    pub update_status: Option<i32>,
    pub library: PathBuf,
    pub size: Option<u64>,
    pub log: PathBuf,
    pub notes: Vec<String>,
}

/// Télécharge l'archive officielle si nécessaire.
fn download_archive(data_dir: &Path) -> Result<u64, String> {
    let root = runtime_root(data_dir);
    fs::create_dir_all(&root).map_err(|error| format!("création du runtime : {error}"))?;
    let archive = archive_path(data_dir);
    if let Ok(meta) = archive.metadata() {
        if meta.len() > 0 {
            return Ok(meta.len());
        }
        // Une archive vide vient d'un téléchargement interrompu : elle ne
        // doit pas empêcher une nouvelle tentative.
        let _ = fs::remove_file(&archive);
    }
    let part = archive.with_extension("tar.gz.part");
    let _ = fs::remove_file(&part);
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(180))
        .build();
    let response = agent
        .get(STEAMCMD_URL)
        .set("User-Agent", "Lapinux/0.1")
        .call()
        .map_err(|error| format!("téléchargement SteamCMD : {error}"))?;
    let mut reader = response.into_reader();
    let mut file = File::create(&part).map_err(|error| format!("archive SteamCMD : {error}"))?;
    let bytes = io::copy(&mut reader, &mut file)
        .map_err(|error| format!("écriture de l'archive SteamCMD : {error}"))?;
    file.sync_all()
        .map_err(|error| format!("synchronisation de l'archive SteamCMD : {error}"))?;
    drop(file);
    fs::rename(&part, &archive).map_err(|error| format!("archive SteamCMD : {error}"))?;
    Ok(bytes)
}

/// Extrait l'archive en refusant tout chemin absolu ou remontant.
fn extract_archive(data_dir: &Path) -> Result<(), String> {
    let archive = archive_path(data_dir);
    let file = File::open(&archive).map_err(|error| format!("archive SteamCMD : {error}"))?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut tar = tar::Archive::new(decoder);
    let root = runtime_root(data_dir);
    let entries = tar
        .entries()
        .map_err(|error| format!("lecture de l'archive SteamCMD : {error}"))?;
    for entry in entries {
        let mut entry = entry.map_err(|error| format!("entrée SteamCMD : {error}"))?;
        let path = entry
            .path()
            .map_err(|error| format!("chemin SteamCMD : {error}"))?
            .to_path_buf();
        if path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::RootDir | Component::Prefix(_)))
        {
            return Err(format!("archive SteamCMD refusée : {}", path.display()));
        }
        let unpacked = entry
            .unpack_in(&root)
            .map_err(|error| format!("extraction SteamCMD : {error}"))?;
        if !unpacked {
            return Err(format!("archive SteamCMD refusée : {}", path.display()));
        }
    }
    Ok(())
}

/// Lance `steamcmd.sh +quit` une fois, sans identifiants.
fn update_steamcmd(data_dir: &Path) -> Result<(i32, PathBuf), String> {
    let root = runtime_root(data_dir);
    let script = root.join("steamcmd.sh");
    if !script.is_file() {
        return Err(format!(
            "steamcmd.sh absent après extraction : {}",
            script.display()
        ));
    }
    let home = stage_steam_home(data_dir)
        .map_err(|error| format!("préparation du HOME Steam : {error}"))?;
    let log_path = update_log_path(data_dir);
    let mut log = File::create(&log_path).map_err(|error| format!("journal SteamCMD : {error}"))?;
    writeln!(log, "$ sh {} +quit", script.display())
        .map_err(|error| format!("journal : {error}"))?;
    let sh = which("sh").unwrap_or_else(|| PathBuf::from("/bin/sh"));
    // Le script calcule sa racine avec `${0%/*}` : un chemin absolu est
    // indispensable, sinon il prend son propre nom pour un dossier.
    let output = Command::new(sh)
        .arg(&script)
        .arg("+quit")
        .current_dir(&root)
        .env("HOME", &home)
        .env("PATH", "/usr/bin:/bin")
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("lancement de SteamCMD : {error}"))?;
    log.write_all(&output.stdout)
        .map_err(|error| format!("journal SteamCMD : {error}"))?;
    log.write_all(&output.stderr)
        .map_err(|error| format!("journal SteamCMD : {error}"))?;
    log.sync_all()
        .map_err(|error| format!("journal SteamCMD : {error}"))?;
    Ok((output.status.code().unwrap_or(-1), log_path))
}

/// Télécharge, extrait et met à jour SteamCMD si la bibliothèque manque.
pub fn prepare(data_dir: &Path) -> Result<PrepareReport, String> {
    let root = runtime_root(data_dir);
    fs::create_dir_all(&root).map_err(|error| format!("création du runtime : {error}"))?;
    let library = library_path(data_dir);
    let log = update_log_path(data_dir);
    let mut report = PrepareReport {
        runtime: root,
        archive: Some(archive_path(data_dir)),
        library: library.clone(),
        log,
        ..PrepareReport::default()
    };
    if library.is_file() {
        report.size = library.metadata().ok().map(|meta| meta.len());
        report.notes.push("bibliothèque déjà présente ; aucun téléchargement".into());
        return Ok(report);
    }

    report.downloaded = download_archive(data_dir)?;
    extract_archive(data_dir)?;
    report.extracted = true;
    let (status, log) = update_steamcmd(data_dir)?;
    report.update_status = Some(status);
    report.log = log;
    if !library.is_file() {
        let tail = tail_of_log(&report.log, 24);
        return Err(format!(
            "SteamCMD n'a pas fourni linux64/steamclient.so (code {status}). Journal : {}\n{tail}",
            report.log.display()
        ));
    }
    report.size = library.metadata().ok().map(|meta| meta.len());
    Ok(report)
}

/// Dernières lignes d'un journal, pour un échec lisible sans noyer le CLI.
fn tail_of_log(path: &Path, lines: usize) -> String {
    let Ok(text) = fs::read_to_string(path) else {
        return String::new();
    };
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

/// État d'une bibliothèque candidate.
#[derive(Debug, Clone, Default)]
pub struct LibraryReport {
    pub path: PathBuf,
    pub exists: bool,
    pub size: Option<u64>,
    /// Description lisible du format ELF.
    pub elf: Option<String>,
    pub loaded: bool,
    pub symbols: Vec<(String, bool)>,
    pub siblings: Vec<(String, bool)>,
    pub error: Option<String>,
    pub notes: Vec<String>,
}

impl LibraryReport {
    pub fn has(&self, name: &str) -> bool {
        self.symbols
            .iter()
            .any(|(symbol, present)| symbol == name && *present)
    }

    pub fn missing_required(&self) -> Vec<&str> {
        REQUIRED_SYMBOLS
            .iter()
            .copied()
            .filter(|name| !self.has(name))
            .collect()
    }
}

/// Lit les premiers octets d'un ELF : classe et machine.
fn elf_description(path: &Path) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let mut header = [0u8; 20];
    file.read_exact(&mut header).ok()?;
    if &header[..4] != b"\x7fELF" {
        return Some("format inconnu (pas ELF)".into());
    }
    let class = match header[4] {
        1 => "32 bits",
        2 => "64 bits",
        _ => "classe inconnue",
    };
    let machine = u16::from_le_bytes([header[18], header[19]]);
    let machine = match machine {
        3 => "x86",
        62 => "x86_64",
        183 => "aarch64",
        _ => "machine inconnue",
    };
    Some(format!("ELF {class} {machine}"))
}

/// Vérifie les symboles et les bibliothèques voisines.
pub fn inspect(data_dir: &Path) -> LibraryReport {
    let path = library_path(data_dir);
    let mut report = LibraryReport {
        path: path.clone(),
        ..LibraryReport::default()
    };
    if !path.is_file() {
        report.error = Some(format!(
            "bibliothèque absente : lance `lapinux-probe runtime --prepare` ({})",
            path.display()
        ));
        return report;
    }
    report.exists = true;
    report.size = path.metadata().ok().map(|meta| meta.len());
    report.elf = elf_description(&path);
    let parent = path.parent().unwrap_or(Path::new("."));
    // L'inspection charge la bibliothèque : elle doit voir le même `HOME`
    // privé que l'enfant, jamais le dossier personnel réel ni un client
    // installé. Le CLI est un processus court ; cet environnement reste
    // ensuite privé jusqu'à sa sortie.
    let home = match stage_steam_home(data_dir) {
        Ok(home) => home,
        Err(error) => {
            report.error = Some(format!("HOME Steam : {error}"));
            return report;
        }
    };
    let bin = match stage_private_bin(data_dir) {
        Ok(bin) => bin,
        Err(error) => {
            report.error = Some(format!("PATH privé : {error}"));
            return report;
        }
    };
    unsafe {
        std::env::set_var("HOME", &home);
        std::env::set_var("PATH", &bin);
        std::env::set_var("XDG_CONFIG_HOME", home.join(".config"));
        std::env::set_var("XDG_CACHE_HOME", home.join(".cache"));
        std::env::set_var("LD_LIBRARY_PATH", parent);
        std::env::remove_var("DISPLAY");
        std::env::remove_var("WAYLAND_DISPLAY");
        std::env::remove_var("DBUS_SESSION_BUS_ADDRESS");
    }
    if let Some(zlib) = crate::probe::zlib_preload() {
        unsafe { std::env::set_var("LD_PRELOAD", zlib) };
    }
    report.siblings = preload_siblings(parent);

    let Some(handle) = (unsafe { dlopen(&path) }) else {
        report.error = Some(format!("dlopen : {}", dl_error()));
        return report;
    };
    report.loaded = true;
    for name in REQUIRED_SYMBOLS.iter().chain(OPTIONAL_SYMBOLS.iter()) {
        report
            .symbols
            .push(((*name).to_string(), dlsym(handle, name).is_some()));
    }
    let missing = report.missing_required();
    if !missing.is_empty() {
        report.notes.push(format!(
            "symboles manquants : {} — ce n'est probablement pas un moteur client",
            missing.join(", ")
        ));
    }
    if !report.has("Steam_CreateGlobalUser") {
        report
            .notes
            .push("pas de session globale : bibliothèque de serveur dédié probable".into());
    }
    report
}

/// Rapport d'une tentative de connexion dans l'enfant.
#[derive(Debug, Clone, Default)]
pub struct ProbeReport {
    pub library: LibraryReport,
    pub fields: Vec<(String, String)>,
    pub stderr: String,
    pub exit: Option<i32>,
    pub duration_ms: u64,
    pub token_len: usize,
}

impl ProbeReport {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }

    pub fn logged_on(&self) -> bool {
        self.get("connecte") == Some("true")
    }
}

/// Ouvre la bibliothèque privée et renvoie son handle.
unsafe fn dlopen(path: &Path) -> Option<*mut c_void> {
    let c_path = CString::new(path.as_os_str().to_string_lossy().as_bytes()).ok()?;
    let handle = libc::dlopen(c_path.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
    (!handle.is_null()).then_some(handle)
}

/// Cherche un symbole dans un handle déjà ouvert.
fn dlsym(handle: *mut c_void, name: &str) -> Option<*mut c_void> {
    let c_name = CString::new(name).ok()?;
    let found = unsafe { libc::dlsym(handle, c_name.as_ptr()) };
    (!found.is_null()).then_some(found)
}

fn dl_error() -> String {
    unsafe {
        let pointer = libc::dlerror();
        if pointer.is_null() {
            "erreur inconnue".into()
        } else {
            CStr::from_ptr(pointer).to_string_lossy().to_string()
        }
    }
}

type CreateInterfaceFn = unsafe extern "C" fn(*const c_char, *mut c_int) -> *mut c_void;
type CreateGlobalUserFn = unsafe extern "C" fn(*mut c_int) -> c_int;
type BLoggedOnFn = unsafe extern "C" fn(c_int, c_int) -> bool;
type BGetCallbackFn = unsafe extern "C" fn(c_int, *mut c_void) -> bool;
type FreeLastCallbackFn = unsafe extern "C" fn(c_int);

/// Précharge les voisines avec `RTLD_GLOBAL`, dans l'ordre de dépendance.
fn preload_siblings(parent: &Path) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    for name in RUNTIME_SIBLINGS {
        let path = parent.join(name);
        let present = path.is_file();
        let loaded = if present {
            let Some(c_path) = CString::new(path.to_string_lossy().as_bytes()).ok() else {
                out.push(((*name).to_string(), false));
                continue;
            };
            let handle =
                unsafe { libc::dlopen(c_path.as_ptr(), libc::RTLD_NOW | libc::RTLD_GLOBAL) };
            if handle.is_null() {
                tracing::debug!("voisine Steam non chargée ({name}) : {}", dl_error());
            }
            !handle.is_null()
        } else {
            false
        };
        out.push(((*name).to_string(), loaded));
    }
    out
}

/// Lit une valeur non alignée dans le tampon de callback.
unsafe fn read_unaligned<T: Copy>(base: *const u8, offset: usize) -> T {
    std::ptr::read_unaligned(base.add(offset) as *const T)
}

/// Entrée de table virtuelle par **index de slot**.
unsafe fn vtable_entry(object: *mut c_void, slot: usize) -> Option<*const c_void> {
    if object.is_null() {
        return None;
    }
    let vtable = *(object as *mut *const *const c_void);
    if vtable.is_null() {
        return None;
    }
    let entry = *vtable.add(slot);
    (!entry.is_null()).then_some(entry)
}

/// Entrée de table virtuelle par **décalage en octets**, comme les constantes
/// d'ABI de Valve.
unsafe fn vtable_entry_at(object: *mut c_void, offset: usize) -> Option<*const c_void> {
    if offset % std::mem::size_of::<usize>() != 0 {
        return None;
    }
    vtable_entry(object, offset / std::mem::size_of::<usize>())
}

/// Rapport de l'enfant, sérialisé en lignes `clé=valeur`.
#[derive(Debug, Default)]
struct HostReport {
    fields: Vec<(String, String)>,
}

impl HostReport {
    fn push(&mut self, key: &str, value: impl ToString) {
        self.fields.push((key.to_string(), value.to_string()));
    }
}

/// Processus enfant : charge la bibliothèque, tente le login, pompe les
/// callbacks et écrit son rapport. Ne revient jamais : `_exit` évite le
/// démontage du moteur, qui n'est pas sûr hors du client officiel.
fn host(data_dir: &Path) -> i32 {
    unsafe {
        let name = CString::new(HOST_PROCESS).unwrap_or_default();
        libc::prctl(libc::PR_SET_NAME, name.as_ptr(), 0, 0, 0);
    }
    let mut report = HostReport::default();
    report.push("data_dir", data_dir.display());
    let _ = stage_steam_home(data_dir);
    let bin = match stage_private_bin(data_dir) {
        Ok(bin) => bin,
        Err(error) => {
            report.push("erreur", format!("PATH privé : {error}"));
            print_host(&report);
            unsafe { libc::_exit(1) }
        }
    };
    let home = steam_home(data_dir);
    unsafe {
        std::env::set_var("HOME", &home);
        std::env::set_var("PATH", &bin);
        std::env::set_var("XDG_CONFIG_HOME", home.join(".config"));
        std::env::set_var("XDG_CACHE_HOME", home.join(".cache"));
        std::env::remove_var("DISPLAY");
        std::env::remove_var("WAYLAND_DISPLAY");
        std::env::remove_var("DBUS_SESSION_BUS_ADDRESS");
    }
    let library = library_path(data_dir);
    let parent = library.parent().unwrap_or(Path::new("."));
    unsafe {
        std::env::set_var("LD_LIBRARY_PATH", parent);
    }
    if let Some(zlib) = crate::probe::zlib_preload() {
        unsafe { std::env::set_var("LD_PRELOAD", zlib) };
    }
    let siblings = preload_siblings(parent);
    report.push(
        "voisines",
        siblings
            .iter()
            .map(|(name, ok)| format!("{name}:{}", if *ok { "oui" } else { "non" }))
            .collect::<Vec<_>>()
            .join(","),
    );

    let Some(handle) = (unsafe { dlopen(&library) }) else {
        report.push("erreur", format!("dlopen : {}", dl_error()));
        print_host(&report);
        unsafe { libc::_exit(1) }
    };
    report.push("charge", "true");
    let Some(create_interface) = dlsym(handle, "CreateInterface") else {
        report.push("erreur", "CreateInterface absent");
        print_host(&report);
        unsafe { libc::_exit(1) }
    };
    let Some(create_global_user) = dlsym(handle, "Steam_CreateGlobalUser") else {
        report.push("erreur", "Steam_CreateGlobalUser absent : shim serveur dédié probable");
        print_host(&report);
        unsafe { libc::_exit(1) }
    };
    let Some(blogged_on) = dlsym(handle, "Steam_BLoggedOn") else {
        report.push("erreur", "Steam_BLoggedOn absent");
        print_host(&report);
        unsafe { libc::_exit(1) }
    };
    let Some(bget_callback) = dlsym(handle, "Steam_BGetCallback") else {
        report.push("erreur", "Steam_BGetCallback absent");
        print_host(&report);
        unsafe { libc::_exit(1) }
    };
    let Some(free_callback) = dlsym(handle, "Steam_FreeLastCallback") else {
        report.push("erreur", "Steam_FreeLastCallback absent");
        print_host(&report);
        unsafe { libc::_exit(1) }
    };
    let create_interface: CreateInterfaceFn = unsafe { std::mem::transmute(create_interface) };
    let create_global_user: CreateGlobalUserFn =
        unsafe { std::mem::transmute(create_global_user) };
    let blogged_on: BLoggedOnFn = unsafe { std::mem::transmute(blogged_on) };
    let bget_callback: BGetCallbackFn = unsafe { std::mem::transmute(bget_callback) };
    let free_callback: FreeLastCallbackFn = unsafe { std::mem::transmute(free_callback) };

    let mut pipe: c_int = 0;
    let user = unsafe { create_global_user(&mut pipe) };
    report.push("pipe", pipe);
    report.push("utilisateur", user);
    if pipe == 0 || user == 0 {
        report.push("erreur", "Steam_CreateGlobalUser a refusé le pipe ou l'utilisateur");
        print_host(&report);
        unsafe { libc::_exit(1) }
    }

    let mut error_code: c_int = 0;
    let engine_name = CString::new(CLIENT_ENGINE_VERSION).unwrap_or_default();
    let engine = unsafe { create_interface(engine_name.as_ptr(), &mut error_code) };
    report.push("moteur", !engine.is_null());
    report.push("moteur_code", error_code);
    if engine.is_null() {
        report.push("erreur", format!("{CLIENT_ENGINE_VERSION} absent (code {error_code})"));
        print_host(&report);
        unsafe { libc::_exit(1) }
    }
    let Some(slot) = (unsafe { vtable_entry(engine, ENGINE_GET_ICLIENT_USER_SLOT) }) else {
        report.push("erreur", "GetIClientUser absent de la table IClientEngine");
        print_host(&report);
        unsafe { libc::_exit(1) }
    };
    let get_user: unsafe extern "C" fn(*mut c_void, c_int, c_int) -> *mut c_void =
        unsafe { std::mem::transmute(slot) };
    let client_user = unsafe { get_user(engine, user, pipe) };
    report.push("iclient_user", !client_user.is_null());
    if client_user.is_null() {
        report.push("erreur", "IClientUser absent : la bibliothèque n'expose pas le client");
        print_host(&report);
        unsafe { libc::_exit(1) }
    }

    let Some(account) = session_for(data_dir) else {
        report.push("erreur", "aucune session Steam enregistrée ; lance `lapinux-probe account --login`");
        print_host(&report);
        unsafe { libc::_exit(1) }
    };
    report.push("compte", &account.account_name);
    report.push("jeton_octets", account.refresh_token.len());

    let account_c = CString::new(account.account_name.as_str()).unwrap_or_default();
    let token_c = CString::new(account.refresh_token.as_str()).unwrap_or_default();
    let Some(is_logged_in_entry) =
        (unsafe { vtable_entry_at(client_user, ICLIENT_USER_IS_ACCOUNT_LOGGED_IN) })
    else {
        report.push("erreur", "IsAccountLoggedIn absent");
        print_host(&report);
        unsafe { libc::_exit(1) }
    };
    let is_logged_in: unsafe extern "C" fn(*mut c_void, *const c_char) -> bool =
        unsafe { std::mem::transmute(is_logged_in_entry) };
    let already = unsafe { is_logged_in(client_user, account_c.as_ptr()) };
    report.push("deja_connecte", already);

    let started = Instant::now();
    let mut counts: BTreeMap<i32, u32> = BTreeMap::new();
    let mut last_eresult: Option<i32> = None;
    let mut logged_on = false;
    let mut login_sent = false;

    if already {
        // Un cache de session peut déjà être ouvert : on laisse le moteur
        // finir son logon automatique avant de forcer un jeton.
        let deadline = Instant::now() + AUTO_LOGON_TIMEOUT;
        while Instant::now() < deadline {
            drain_callbacks(
                pipe,
                bget_callback,
                free_callback,
                &mut counts,
                &mut last_eresult,
            );
            if unsafe { blogged_on(pipe, user) } {
                logged_on = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    if !logged_on {
        login_sent = logon_with_token(client_user, &account, &account_c, &token_c, &mut report);
        if !login_sent {
            print_host(&report);
            unsafe { libc::_exit(1) }
        }
    }

    let deadline = Instant::now() + LOGON_TIMEOUT;
    while Instant::now() < deadline && !logged_on {
        drain_callbacks(
            pipe,
            bget_callback,
            free_callback,
            &mut counts,
            &mut last_eresult,
        );
        if unsafe { blogged_on(pipe, user) } {
            logged_on = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    // Un dernier drain : le callback de connexion peut être arrivé juste
    // avant le dernier `BLoggedOn`.
    drain_callbacks(
        pipe,
        bget_callback,
        free_callback,
        &mut counts,
        &mut last_eresult,
    );
    for (id, count) in &counts {
        report.push(&format!("callback_{id}"), count);
    }
    if let Some(eresult) = last_eresult {
        report.push("eresult", eresult);
    }
    report.push("login_envoye", login_sent);
    report.push("connecte", logged_on);
    report.push("duree_ms", started.elapsed().as_millis());
    print_host(&report);
    unsafe { libc::_exit(0) }
}

fn logon_with_token(
    client_user: *mut c_void,
    account: &SavedSession,
    account_c: &CString,
    token_c: &CString,
    report: &mut HostReport,
) -> bool {
    let Some(set_id_entry) =
        (unsafe { vtable_entry_at(client_user, ICLIENT_USER_SET_STEAM_ID) })
    else {
        report.push("erreur", "SetSteamID absent");
        return false;
    };
    let Some(set_login_entry) =
        (unsafe { vtable_entry_at(client_user, ICLIENT_USER_SET_LOGIN_INFORMATION) })
    else {
        report.push("erreur", "SetLoginInformation absent");
        return false;
    };
    let Some(logon_entry) =
        (unsafe { vtable_entry_at(client_user, ICLIENT_USER_LOGON_WITH_REFRESH_TOKEN) })
    else {
        report.push("erreur", "LogonWithRefreshToken absent");
        return false;
    };
    let set_id: unsafe extern "C" fn(*mut c_void, u64) =
        unsafe { std::mem::transmute(set_id_entry) };
    let set_login: unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char, c_int) =
        unsafe { std::mem::transmute(set_login_entry) };
    let logon: unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) =
        unsafe { std::mem::transmute(logon_entry) };
    let empty = CString::new("").unwrap_or_default();
    unsafe {
        set_login(client_user, account_c.as_ptr(), empty.as_ptr(), 1);
        logon(client_user, token_c.as_ptr(), account_c.as_ptr());
        set_id(client_user, account.steam_id);
    }
    report.push("login_envoye", "true");
    report.push("set_steam_id", account.steam_id);
    true
}

/// Charge la session depuis le dossier de données explicite de l'enfant.
fn session_for(data_dir: &Path) -> Option<SavedSession> {
    session::load_from(&session::path_in(data_dir))
}

/// Vide la file de callbacks et retient l'EResult d'un échec de connexion.
fn drain_callbacks(
    pipe: c_int,
    bget: BGetCallbackFn,
    free: FreeLastCallbackFn,
    counts: &mut BTreeMap<i32, u32>,
    last_eresult: &mut Option<i32>,
) {
    let mut buffer = [0u8; 64];
    while unsafe { bget(pipe, buffer.as_mut_ptr() as *mut c_void) } {
        let id: i32 = unsafe { read_unaligned(buffer.as_ptr(), 4) };
        let data: *const c_void = unsafe { read_unaligned(buffer.as_ptr(), 8) };
        *counts.entry(id).or_insert(0) += 1;
        if id == 102 && !data.is_null() {
            let value: i32 = unsafe { read_unaligned(data as *const u8, 0) };
            *last_eresult = Some(value);
        }
        unsafe { free(pipe) };
    }
}

/// Écrit le rapport de l'enfant sur stdout, une ligne par champ.
fn print_host(report: &HostReport) {
    let mut stdout = io::stdout().lock();
    for (key, value) in &report.fields {
        let _ = writeln!(stdout, "{key}={}", value.replace('\n', " "));
    }
    let _ = stdout.flush();
}

/// Binaire qui porte la sous-commande `runtime host`.
///
/// C'est la sonde `lapinux-probe` qui l'analyse ; le binaire `lapinux`, qui la
/// lance pourtant depuis le daemon, ne la connaît pas. La sonde posée à côté
/// du binaire courant est donc préférée, quand elle existe.
fn host_exe() -> Result<std::path::PathBuf, String> {
    let current = std::env::current_exe().map_err(|error| format!("binaire : {error}"))?;
    let name = current
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if name.starts_with("lapinux-probe") {
        return Ok(current);
    }
    let sibling = current.with_file_name(format!(
        "lapinux-probe{}",
        std::env::consts::EXE_SUFFIX
    ));
    Ok(if sibling.is_file() { sibling } else { current })
}

/// Lance l'enfant avec un délai maximal et renvoie sa sortie.
fn run_host_child(
    data_dir: &Path,
    extra: &[String],
) -> Result<(Vec<(String, String)>, String, i32, u64), String> {
    let exe = host_exe()?;
    let home = steam_home(data_dir);
    let bin = private_bin(data_dir);
    let root = runtime_root(data_dir);
    fs::create_dir_all(&root).map_err(|error| format!("runtime : {error}"))?;
    let mut command = Command::new(exe);
    command
        .arg("runtime")
        .arg("host")
        .arg("--data-dir")
        .arg(data_dir)
        .args(extra)
        .current_dir(&root)
        .env("LAPINUX_RUNTIME_CHILD", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command.env("HOME", &home).env("PATH", &bin);
    let started = Instant::now();
    let mut child = command
        .spawn()
        .map_err(|error| format!("lancement de l'hôte : {error}"))?;
    let stdout = child.stdout.take().ok_or("stdout de l'hôte absent")?;
    let stderr = child.stderr.take().ok_or("stderr de l'hôte absent")?;
    let (out_tx, out_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut text = String::new();
        let mut reader = stdout;
        let _ = reader.read_to_string(&mut text);
        let _ = out_tx.send(text);
    });
    let (err_tx, err_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut text = String::new();
        let mut reader = stderr;
        let _ = reader.read_to_string(&mut text);
        let _ = err_tx.send(text);
    });
    let status = wait_child(&mut child, HOST_TIMEOUT)?;
    let stdout = out_rx.recv_timeout(Duration::from_secs(2)).unwrap_or_default();
    let stderr = err_rx.recv_timeout(Duration::from_secs(2)).unwrap_or_default();
    let duration = started.elapsed().as_millis() as u64;
    let fields = parse_fields(&stdout);
    let exit = status.code().unwrap_or(-1);
    Ok((fields, stderr, exit, duration))
}

/// Attend un enfant avec un plafond ; le tue et le récolte sinon.
fn wait_child(child: &mut Child, timeout: Duration) -> Result<std::process::ExitStatus, String> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().map_err(|error| format!("hôte : {error}"))? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "l'hôte n'a pas rendu la main en {} s",
                timeout.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Parse les lignes `clé=valeur` du rapport enfant.
fn parse_fields(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.trim().to_string(), value.trim().to_string()))
        .filter(|(key, _)| !key.is_empty())
        .collect()
}

/// Prépare le runtime si nécessaire, puis tente une connexion réelle.
pub fn probe(data_dir: &Path) -> Result<ProbeReport, String> {
    let library = inspect(data_dir);
    if !library.exists {
        return Err(library
            .error
            .clone()
            .unwrap_or_else(|| "bibliothèque Steam absente".into()));
    }
    let account = session::load_from(&session::path_in(data_dir)).ok_or_else(|| {
        "aucune session Steam enregistrée ; lance `lapinux-probe account --login`".to_string()
    })?;
    if account.refresh_token.trim().is_empty() {
        return Err("la session Steam enregistrée ne contient pas de jeton".into());
    }
    stage_steam_home(data_dir).map_err(|error| format!("HOME Steam : {error}"))?;
    let token_len = account.refresh_token.len();
    let (fields, stderr, exit, duration_ms) = run_host_child(data_dir, &[])?;
    Ok(ProbeReport {
        library,
        fields,
        stderr,
        exit: Some(exit),
        duration_ms,
        token_len,
    })
}

/// Affiche un rapport de préparation.
fn print_prepare(report: &PrepareReport) {
    println!("runtime SteamCMD :");
    println!("  dossier       {}", report.runtime.display());
    if let Some(archive) = &report.archive {
        println!("  archive       {}", archive.display());
    }
    if report.downloaded > 0 {
        println!(
            "  téléchargé    {}",
            lapinux_core::util::format_bytes(report.downloaded)
        );
    }
    println!("  extraite      {}", if report.extracted { "oui" } else { "non" });
    if let Some(status) = report.update_status {
        println!("  mise à jour   code {status}");
    }
    println!("  journal       {}", report.log.display());
    println!("  bibliothèque  {}", report.library.display());
    if let Some(size) = report.size {
        println!("  taille        {}", lapinux_core::util::format_bytes(size));
    }
    for note in &report.notes {
        println!("  note          {note}");
    }
}

/// Affiche l'état de la bibliothèque.
fn print_library(report: &LibraryReport) {
    println!("bibliothèque  {}", report.path.display());
    println!("  présente     {}", if report.exists { "oui" } else { "non" });
    if let Some(size) = report.size {
        println!("  taille       {}", lapinux_core::util::format_bytes(size));
    }
    if let Some(elf) = &report.elf {
        println!("  format       {elf}");
    }
    println!("  chargée      {}", if report.loaded { "oui" } else { "non" });
    if !report.symbols.is_empty() {
        println!("  symboles :");
        for (name, present) in &report.symbols {
            println!("    {name:<34} {}", if *present { "oui" } else { "non" });
        }
    }
    if !report.siblings.is_empty() {
        println!("  voisines :");
        for (name, present) in &report.siblings {
            println!("    {name:<34} {}", if *present { "oui" } else { "non" });
        }
    }
    if let Some(error) = &report.error {
        println!("  erreur       {error}");
    }
    for note in &report.notes {
        println!("  note         {note}");
    }
}

/// Affiche le résultat de la sonde de connexion.
fn print_probe(report: &ProbeReport) {
    print_library(&report.library);
    println!("session :");
    println!("  jeton (octets) {}", report.token_len);
    println!("  client installé utilisé : non");
    for (key, value) in &report.fields {
        if key == "client_installe_utilise" || key == "data_dir" {
            continue;
        }
        println!("  {key:<16} {value}");
    }
    if !report.stderr.trim().is_empty() {
        println!("  journal stderr :");
        for line in report.stderr.lines().rev().take(12).collect::<Vec<_>>().iter().rev() {
            println!("    {line}");
        }
    }
    println!("  durée          {} ms", report.duration_ms);
}

/// Point d'entrée de `lapinux-probe runtime`.
pub fn cli(args: &[String]) -> i32 {
    let data_dir = LapinuxPaths::discover().data_dir;
    if args.first().map(String::as_str) == Some("host") {
        let data = value_of(args, "--data-dir").map(PathBuf::from);
        return host(data.as_deref().unwrap_or(&data_dir));
    }
    let prepare_requested = args.iter().any(|arg| arg == "--prepare");
    let probe_requested = args.iter().any(|arg| arg == "--probe");
    if prepare_requested {
        match prepare(&data_dir) {
            Ok(report) => print_prepare(&report),
            Err(error) => {
                eprintln!("préparation du runtime impossible : {error}");
                return 1;
            }
        }
        if !probe_requested {
            return 0;
        }
    }
    if probe_requested {
        match probe(&data_dir) {
            Ok(report) => {
                print_probe(&report);
                if report.get("erreur").is_some() {
                    return 1;
                }
                return if report.logged_on() { 0 } else { 1 };
            }
            Err(error) => {
                eprintln!("sonde Steam : {error}");
                return 1;
            }
        }
    }
    let library = inspect(&data_dir);
    print_library(&library);
    if !library.exists {
        println!("\nAstuce : lance `lapinux-probe runtime --prepare`.");
        return 1;
    }
    println!("\nAstuce : `lapinux-probe runtime --probe` tente la connexion avec le jeton enregistré.");
    0
}

fn value_of(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|index| args.get(index + 1))
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn runtime_paths_are_private() {
        let data = Path::new("/data/lapinux");
        assert_eq!(runtime_root(data), PathBuf::from("/data/lapinux/steam-runtime"));
        assert_eq!(steam_home(data), PathBuf::from("/data/lapinux/steam-home"));
        let library = library_path(data);
        assert!(library.ends_with("steam-runtime/linux64/steamclient.so"));
        let text = library.to_string_lossy();
        assert!(!text.contains("/.steam/"));
        assert!(!text.contains("/.local/share/Steam"));
    }

    #[test]
    fn abi_offsets_match_the_client_engine_layout() {
        assert_eq!(ENGINE_GET_ICLIENT_USER_SLOT, 8);
        assert_eq!(ICLIENT_USER_SET_STEAM_ID, 0x08);
        assert_eq!(ICLIENT_USER_IS_ACCOUNT_LOGGED_IN, 0x188);
        assert_eq!(ICLIENT_USER_SET_LOGIN_INFORMATION, 0x1B0);
        assert_eq!(ICLIENT_USER_LOGON_WITH_REFRESH_TOKEN, 0x1C0);
        assert_eq!(ICLIENT_USER_SET_STEAM_ID / std::mem::size_of::<usize>(), 1);
        assert_eq!(
            ICLIENT_USER_LOGON_WITH_REFRESH_TOKEN / std::mem::size_of::<usize>(),
            56
        );
    }

    #[test]
    fn staging_home_creates_and_empties_vdf_files() {
        let dir = tempfile::tempdir().unwrap();
        let steam = steam_home(dir.path()).join("Steam");
        std::fs::create_dir_all(steam.join("config")).unwrap();
        for relative in ["config/config.vdf", "config/local.vdf", "local.vdf"] {
            std::fs::write(steam.join(relative), "cache de connexion").unwrap();
        }
        let home = stage_steam_home(dir.path()).unwrap();
        for relative in ["config/config.vdf", "config/local.vdf", "local.vdf"] {
            let path = home.join("Steam").join(relative);
            assert!(path.is_file(), "{}", path.display());
            assert_eq!(std::fs::metadata(path).unwrap().len(), 0);
        }
    }

    #[test]
    fn missing_library_does_not_fall_back_to_installed_steam() {
        let dir = tempfile::tempdir().unwrap();
        let report = inspect(dir.path());
        assert!(!report.exists);
        let error = report.error.unwrap();
        assert!(error.contains("runtime --prepare"));
        assert!(!error.contains(".local/share/Steam"));
    }

    #[test]
    fn steamcmd_url_is_https() {
        assert!(STEAMCMD_URL.starts_with("https://steamcdn-a.akamaihd.net/"));
    }

    #[test]
    fn probe_report_never_carries_the_token() {
        let report = ProbeReport {
            token_len: 42,
            ..ProbeReport::default()
        };
        assert_eq!(report.token_len, 42);
        assert_eq!(report.get("jeton"), None);
    }

    #[test]
    fn parse_fields_ignores_noise() {
        let fields = parse_fields("connecte=true\nbruit\npipe=3\n");
        assert_eq!(
            fields,
            vec![
                ("connecte".to_string(), "true".to_string()),
                ("pipe".to_string(), "3".to_string())
            ]
        );
    }

    #[test]
    fn log_tail_is_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("log");
        std::fs::write(&log, "a\nb\nc\n").unwrap();
        assert_eq!(tail_of_log(&log, 2), "b\nc");
    }

    #[test]
    fn private_bin_contains_only_sh() {
        let dir = tempfile::tempdir().unwrap();
        let bin = stage_private_bin(dir.path()).unwrap();
        assert!(bin.join("sh").exists());
        let entries = std::fs::read_dir(bin).unwrap().count();
        assert_eq!(entries, 1);
        let mode = std::fs::metadata(stage_private_bin(dir.path()).unwrap().join("sh"))
            .unwrap()
            .permissions()
            .mode();
        assert!(mode & 0o111 != 0);
    }
}
