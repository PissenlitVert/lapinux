//! Pont Steamworks de Lapinux : `libsteam_api.so` sans client Steam.
//!
//! Les jeux Linux chargent `libsteam_api.so` et appellent ses interfaces.
//! Préchargée sous le même SONAME, cette bibliothèque prend sa place : elle
//! répond partiellement aux appels de succès et de statistiques. Les succès
//! confirmés sont déposés dans la file persistante lue par le daemon, qui les
//! transmet à Steam avec la session du compte.
//!
//! Rien n'est inventé ici : seuls les succès que le jeu a lui-même accordés
//! (`SetAchievement`) et confirmés (`StoreStats`) partent. Un succès déjà
//! débloqué est ignoré, et `ClearAchievement` ne fait que retirer un déblocage
//! encore en attente.
//!
//! Couverture partielle : les statistiques et succès du processus vivent en
//! mémoire ; seuls les succès sont transmis. Pas de lecture des valeurs Steam
//! précédentes ni de callbacks. Plusieurs interfaces ne renvoient que des
//! valeurs neutres ; une méthode inconnue peut avoir une ABI incompatible et
//! provoquer une erreur. Aucune compatibilité universelle n'est garantie.

use std::collections::{HashMap, HashSet};
use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

/// Variable qui remplace l'écriture réseau par un journal, pour les essais.
const DRY_RUN: &str = "LAPINUX_STEAMSHIM_DRY_RUN";
/// Emplacements de chaque table d'interface : au-delà du dernier méthode
/// connue, pour ne jamais lire hors de la table sur une version récente.
const SLOTS: usize = 96;

static APPID: AtomicU32 = AtomicU32::new(0);
static STEAM_ID: AtomicU64 = AtomicU64::new(0);
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// Generation des contextes du SDK C++ (zéro est la valeur non initialisée).
static CONTEXT_GENERATION: AtomicUsize = AtomicUsize::new(1);
/// Succès transmis au magasin pendant un essai sans réseau.
static DRY_JOBS: Mutex<Vec<Job>> = Mutex::new(Vec::new());

#[derive(Clone, Debug, PartialEq, Eq)]
struct Job {
    appid: u32,
    names: Vec<String>,
}

#[derive(Clone, Copy, Debug)]
enum StatValue {
    Int(i32),
    Float(f32),
}

#[derive(Default)]
struct State {
    achievements: HashMap<String, bool>,
    stats: HashMap<String, StatValue>,
    /// Succès accordés depuis le dernier `StoreStats`.
    dirty: HashSet<String>,
    /// Ordre de découverte, pour `GetAchievementName`.
    order: Vec<String>,
    /// Noms prêts pour le C, gardés vivants.
    names: Vec<CString>,
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(State::default()))
}

fn dry_run() -> bool {
    std::env::var_os(DRY_RUN).is_some()
}

fn appid() -> u32 {
    APPID.load(Ordering::Relaxed)
}

fn data_dir() -> std::path::PathBuf {
    std::env::var_os("LAPINUX_STEAMSHIM_DATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| lapinux_core::paths::LapinuxPaths::discover().data_dir)
}

fn appid_from_env() -> Option<u32> {
    std::env::var("SteamAppId")
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .or_else(|| {
            std::fs::read_to_string("steam_appid.txt")
                .ok()
                .and_then(|value| value.trim().parse().ok())
        })
}

/// Dépose le lot avant de rendre la main au jeu : le daemon pourra le relire
/// même si le processus du jeu se ferme immédiatement après StoreStats.
fn queue_job(job: &Job) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
    let dir = data_dir().join("achievements");
    std::fs::create_dir_all(&dir)?;
    let steam_id = STEAM_ID.load(Ordering::Relaxed);
    if steam_id == 0 {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "SteamID absent"));
    }
    let body = serde_json::to_vec(&serde_json::json!({ "appid": job.appid, "names": job.names, "steam_id": steam_id }))
        .map_err(std::io::Error::other)?;
    // Un nom unique évite toute collision avec les autres jeux ou sessions.
    for _ in 0..16 {
        let nonce = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let name = format!("native-{}-{}-{nonce}", std::process::id(), lapinux_core::util::now_unix());
        let temporary = dir.join(format!("{name}.tmp"));
        let final_path = dir.join(format!("{name}.json"));
        let mut file = match std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&temporary) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };
        let result: std::io::Result<()> = (|| {
            file.write_all(&body)?;
            file.sync_all()?;
            // hard_link publie un fichier complet sans écraser un ancien lot.
            std::fs::hard_link(&temporary, &final_path)?;
            Ok(())
        })();
        let _ = std::fs::remove_file(&temporary);
        match result {
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            other => return other,
        }
    }
    Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "aucun nom libre pour le lot Steamworks"))
}

/// Envoie les succès accordés et pas encore transmis.
fn flush() -> bool {
    let job = {
        let mut state = state().lock().expect("état Steamworks");
        if state.dirty.is_empty() {
            return true;
        }
        let mut names: Vec<String> = state
            .dirty
            .iter()
            .filter(|name| state.achievements.get(*name).copied().unwrap_or(false))
            .cloned()
            .collect();
        state.dirty.clear();
        if names.is_empty() {
            return true;
        }
        names.sort();
        Job {
            appid: appid(),
            names,
        }
    };
    if dry_run() {
        DRY_JOBS.lock().expect("journal d'essai").push(job);
        return true;
    }
    match queue_job(&job) {
        Ok(()) => true,
        Err(error) => {
            eprintln!("[lapinux-steamshim] file des succès indisponible ({error}) : à réessayer");
            let mut state = state().lock().expect("état Steamworks");
            state.dirty.extend(job.names);
            false
        }
    }
}

fn init() -> bool {
    if ACTIVE.load(Ordering::Relaxed) {
        return true;
    }
    if dry_run() {
        if appid() == 0 {
            APPID.store(appid_from_env().unwrap_or(480), Ordering::Relaxed);
        }
        ACTIVE.store(true, Ordering::Relaxed);
        return true;
    }
    let Some(appid) = appid_from_env().filter(|id| *id != 0) else {
        eprintln!("[lapinux-steamshim] SteamAppId absent : pont inactif");
        return false;
    };
    let session_path = lapinux_steam::session::path_in(&data_dir());
    let Some(session) = lapinux_steam::session::load_from(&session_path)
        .filter(|session| session.steam_id != 0 && !session.account_name.trim().is_empty() && !session.refresh_token.trim().is_empty()) else {
        eprintln!("[lapinux-steamshim] aucun compte Steam valide : pont inactif");
        return false;
    };
    if STEAM_ID.compare_exchange(0, session.steam_id, Ordering::Relaxed, Ordering::Relaxed)
        .is_err_and(|id| id != session.steam_id) {
        eprintln!("[lapinux-steamshim] compte Steam changé pendant la partie : réinitialisation refusée");
        return false;
    }
    APPID.store(appid, Ordering::Relaxed);
    ACTIVE.store(true, Ordering::Relaxed);
    true
}

unsafe fn text<'a>(value: *const c_char) -> Option<&'a str> {
    if value.is_null() {
        return None;
    }
    CStr::from_ptr(value).to_str().ok()
}

// ---------------------------------------------------------------------------
// Tables d'interface
// ---------------------------------------------------------------------------

/// Objet d'interface : son premier mot est la table des méthodes, comme en C++.
#[repr(C)]
struct Object {
    vtable: *const usize,
}

/// Réponse neutre pour les emplacements non implémentés. Certaines signatures
/// (notamment les retours structurés ou flottants) ne sont pas compatibles.
extern "C" fn stub_zero() -> u64 {
    0
}

/// Adresse d'une méthode, pour une table d'interface : la conversion passe
/// par un pointeur nu, seule forme admise pour un pointeur de fonction.
macro_rules! slot {
    ($function:path) => {
        $function as *const () as usize
    };
}

/// Fabrique une table de `SLOTS` emplacements, dont les `entries` de tête sont
/// les vraies méthodes. La table et l'objet vivent jusqu'à la fin du processus.
fn make_object(entries: &[usize]) -> *mut c_void {
    let mut table = vec![slot!(stub_zero); SLOTS];
    let count = entries.len().min(SLOTS);
    table[..count].copy_from_slice(&entries[..count]);
    let table = Box::leak(table.into_boxed_slice());
    let object = Box::new(Object {
        vtable: table.as_ptr(),
    });
    Box::into_raw(object) as *mut c_void
}

macro_rules! stub_objects {
    ($( $name:ident ),* $(,)?) => {
        $(
            /// Interface neutre, partagée par l'accès plat et la recherche par version.
            fn $name() -> *mut c_void {
                static OBJECT: OnceLock<usize> = OnceLock::new();
                *OBJECT.get_or_init(|| make_object(&[]) as usize) as *mut c_void
            }
        )*
    };
}

stub_objects!(
    remote_storage_object,
    matchmaking_object,
    matchmaking_servers_object,
    networking_object,
    networking_sockets_object,
    networking_utils_object,
    screenshots_object,
    http_object,
    inventory_object,
    ugc_object,
    music_object,
    music_remote_object,
    html_surface_object,
    controller_object,
    input_object,
    video_object,
    app_list_object,
    parental_object,
    game_search_object,
    parties_object,
    timeline_object,
    game_server_object,
    game_server_stats_object,
    game_coordinator_object,
    master_server_updater_object,
);

fn stub_object(version: &str) -> *mut c_void {
    static STUBS: Mutex<Option<HashMap<String, usize>>> = Mutex::new(None);
    let mut cache = STUBS.lock().expect("interfaces neutres");
    let map = cache.get_or_insert_with(HashMap::new);
    if let Some(pointer) = map.get(version) {
        return *pointer as *mut c_void;
    }
    eprintln!("[lapinux-steamshim] interface neutre : {version}");
    let pointer = make_object(&[]) as usize;
    map.insert(version.to_string(), pointer);
    pointer as *mut c_void
}

// ---------------------------------------------------------------------------
// ISteamUserStats
// ---------------------------------------------------------------------------

extern "C" fn us_request_current_stats(_: *mut c_void) -> bool {
    true
}

extern "C" fn us_get_stat_int(_: *mut c_void, name: *const c_char, out: *mut i32) -> bool {
    let Some(name) = (unsafe { text(name) }) else {
        return false;
    };
    let state = state().lock().expect("état Steamworks");
    match state.stats.get(name) {
        Some(StatValue::Int(value)) => {
            if !out.is_null() {
                unsafe { *out = *value };
            }
            true
        }
        _ => false,
    }
}

extern "C" fn us_get_stat_float(_: *mut c_void, name: *const c_char, out: *mut f32) -> bool {
    let Some(name) = (unsafe { text(name) }) else {
        return false;
    };
    let state = state().lock().expect("état Steamworks");
    match state.stats.get(name) {
        Some(StatValue::Float(value)) => {
            if !out.is_null() {
                unsafe { *out = *value };
            }
            true
        }
        _ => false,
    }
}

extern "C" fn us_set_stat_int(_: *mut c_void, name: *const c_char, value: i32) -> bool {
    let Some(name) = (unsafe { text(name) }) else {
        return false;
    };
    state()
        .lock()
        .expect("état Steamworks")
        .stats
        .insert(name.to_string(), StatValue::Int(value));
    true
}

extern "C" fn us_set_stat_float(_: *mut c_void, name: *const c_char, value: f32) -> bool {
    let Some(name) = (unsafe { text(name) }) else {
        return false;
    };
    state()
        .lock()
        .expect("état Steamworks")
        .stats
        .insert(name.to_string(), StatValue::Float(value));
    true
}

extern "C" fn us_update_avg_rate_stat(_: *mut c_void, _: *const c_char, _: f32, _: f64) -> bool {
    true
}

extern "C" fn us_get_achievement(_: *mut c_void, name: *const c_char, out: *mut bool) -> bool {
    let Some(name) = (unsafe { text(name) }) else {
        return false;
    };
    let state = state().lock().expect("état Steamworks");
    let unlocked = state.achievements.get(name).copied().unwrap_or(false);
    if !out.is_null() {
        unsafe { *out = unlocked };
    }
    true
}

extern "C" fn us_set_achievement(_: *mut c_void, name: *const c_char) -> bool {
    let Some(name) = (unsafe { text(name) }) else {
        return false;
    };
    let mut state = state().lock().expect("état Steamworks");
    if !state.achievements.contains_key(name) {
        if let Ok(c_name) = CString::new(name) {
            state.names.push(c_name);
            state.order.push(name.to_string());
        }
    }
    state.achievements.insert(name.to_string(), true);
    state.dirty.insert(name.to_string());
    true
}

extern "C" fn us_clear_achievement(_: *mut c_void, name: *const c_char) -> bool {
    let Some(name) = (unsafe { text(name) }) else {
        return false;
    };
    let mut state = state().lock().expect("état Steamworks");
    state.achievements.insert(name.to_string(), false);
    state.dirty.remove(name);
    true
}

extern "C" fn us_get_achievement_and_unlock_time(
    this: *mut c_void,
    name: *const c_char,
    out: *mut bool,
    time: *mut u32,
) -> bool {
    if !time.is_null() {
        unsafe { *time = 0 };
    }
    us_get_achievement(this, name, out)
}

extern "C" fn us_store_stats(_: *mut c_void) -> bool {
    flush()
}

extern "C" fn us_get_achievement_icon(_: *mut c_void, _: *const c_char) -> i32 {
    0
}

extern "C" fn us_get_achievement_display_attribute(
    _: *mut c_void,
    _: *const c_char,
    _: *const c_char,
) -> *const c_char {
    static EMPTY: &[u8] = b"\0";
    EMPTY.as_ptr() as *const c_char
}

extern "C" fn us_indicate_achievement_progress(_: *mut c_void, _: *const c_char, _: u32, _: u32) -> bool {
    true
}

extern "C" fn us_get_num_achievements(_: *mut c_void) -> u32 {
    state()
        .lock()
        .expect("état Steamworks")
        .order
        .len()
        .try_into()
        .unwrap_or(0)
}

extern "C" fn us_get_achievement_name(_: *mut c_void, index: u32) -> *const c_char {
    let state = state().lock().expect("état Steamworks");
    state
        .names
        .get(index as usize)
        .map(|name| name.as_ptr())
        .unwrap_or(std::ptr::null())
}

extern "C" fn us_request_user_stats(_: *mut c_void, _: u64) -> u64 {
    0
}

extern "C" fn us_get_user_stat_int(_: *mut c_void, _: u64, _: *const c_char, _: *mut i32) -> bool {
    false
}

extern "C" fn us_get_user_stat_float(_: *mut c_void, _: u64, _: *const c_char, _: *mut f32) -> bool {
    false
}

extern "C" fn us_get_user_achievement(_: *mut c_void, _: u64, _: *const c_char, _: *mut bool) -> bool {
    false
}

extern "C" fn us_get_user_achievement_and_unlock_time(
    _: *mut c_void,
    _: u64,
    _: *const c_char,
    _: *mut bool,
    _: *mut u32,
) -> bool {
    false
}

extern "C" fn us_reset_all_stats(_: *mut c_void, _: bool) -> bool {
    false
}

extern "C" fn us_find_or_create_leaderboard(_: *mut c_void, _: *const c_char, _: i32) -> u64 {
    0
}

extern "C" fn us_find_leaderboard(_: *mut c_void, _: *const c_char) -> u64 {
    0
}

/// `ISteamUserStats` 012, et 013 qui retire RequestCurrentStats.
fn stats_object() -> *mut c_void { stats_object_version(false) }

fn stats_object_version(v013: bool) -> *mut c_void {
    static OBJECT: OnceLock<usize> = OnceLock::new();
    static OBJECT_013: OnceLock<usize> = OnceLock::new();
    let object = if v013 { &OBJECT_013 } else { &OBJECT };
    *object.get_or_init(|| {
        let entries = [
            slot!(us_request_current_stats),
            slot!(us_get_stat_int),
            slot!(us_get_stat_float),
            slot!(us_set_stat_int),
            slot!(us_set_stat_float),
            slot!(us_update_avg_rate_stat),
            slot!(us_get_achievement),
            slot!(us_set_achievement),
            slot!(us_clear_achievement),
            slot!(us_get_achievement_and_unlock_time),
            slot!(us_store_stats),
            slot!(us_get_achievement_icon),
            slot!(us_get_achievement_display_attribute),
            slot!(us_indicate_achievement_progress),
            slot!(us_get_num_achievements),
            slot!(us_get_achievement_name),
            slot!(us_request_user_stats),
            slot!(us_get_user_stat_int),
            slot!(us_get_user_stat_float),
            slot!(us_get_user_achievement),
            slot!(us_get_user_achievement_and_unlock_time),
            slot!(us_reset_all_stats),
            slot!(us_find_or_create_leaderboard),
            slot!(us_find_leaderboard),
            // GetLeaderboardName et GetLeaderboardEntryCount ne sont pas
            // implémentés : leurs signatures ne sont pas interchangeables.
        ];
        make_object(if v013 { &entries[1..] } else { &entries }) as usize
    }) as *mut c_void
}

// ---------------------------------------------------------------------------
// ISteamUser, ISteamApps, ISteamUtils, ISteamFriends
// ---------------------------------------------------------------------------

extern "C" fn user_h_steam_user(_: *mut c_void) -> i32 {
    1
}
extern "C" fn user_logged_on(_: *mut c_void) -> bool {
    ACTIVE.load(Ordering::Relaxed)
}
extern "C" fn user_steam_id(_: *mut c_void) -> u64 {
    STEAM_ID.load(Ordering::Relaxed)
}

/// `ISteamUser` : seuls les trois premiers appels sont servis (identifiant et
/// état de connexion) ; l'authentification et les billets restent neutres.
extern "C" fn user_object() -> *mut c_void {
    static OBJECT: OnceLock<usize> = OnceLock::new();
    *OBJECT.get_or_init(|| {
        make_object(&[
            slot!(user_h_steam_user),
            slot!(user_logged_on),
            slot!(user_steam_id),
        ]) as usize
    }) as *mut c_void
}

extern "C" fn apps_subscribed(_: *mut c_void) -> bool {
    ACTIVE.load(Ordering::Relaxed)
}
extern "C" fn apps_false(_: *mut c_void) -> bool {
    false
}
extern "C" fn apps_false_app(_: *mut c_void, _: u32) -> bool {
    false
}
extern "C" fn apps_language(_: *mut c_void) -> *const c_char {
    static LANGUAGE: &[u8] = b"french\0";
    LANGUAGE.as_ptr() as *const c_char
}
extern "C" fn apps_subscribed_app(_: *mut c_void, _: u32) -> bool {
    true
}

/// `ISteamApps` : abonnement, langues, DLC et possession d'une application.
extern "C" fn apps_object() -> *mut c_void {
    static OBJECT: OnceLock<usize> = OnceLock::new();
    *OBJECT.get_or_init(|| {
        make_object(&[
            slot!(apps_subscribed),
            slot!(apps_false),
            slot!(apps_false),
            slot!(apps_false),
            slot!(apps_language),
            slot!(apps_language),
            slot!(apps_subscribed_app),
            slot!(apps_false_app),
        ]) as usize
    }) as *mut c_void
}

extern "C" fn utils_seconds(_: *mut c_void) -> u32 {
    0
}
extern "C" fn utils_app_id(_: *mut c_void) -> u32 {
    appid()
}
extern "C" fn utils_calls(_: *mut c_void) -> u32 {
    0
}
extern "C" fn utils_overlay(_: *mut c_void) -> bool {
    false
}
extern "C" fn utils_big_picture(_: *mut c_void) -> bool {
    false
}

/// `ISteamUtils` : appid du jeu, sans overlay Steam ni mode TV.
extern "C" fn utils_object() -> *mut c_void {
    static OBJECT: OnceLock<usize> = OnceLock::new();
    *OBJECT.get_or_init(|| {
        let mut entries = vec![slot!(stub_zero); 27];
        entries[0] = slot!(utils_seconds);
        entries[1] = slot!(utils_seconds);
        entries[9] = slot!(utils_app_id);
        entries[15] = slot!(utils_calls);
        entries[17] = slot!(utils_overlay);
        entries[26] = slot!(utils_big_picture);
        make_object(&entries) as usize
    }) as *mut c_void
}

extern "C" fn friends_name(_: *mut c_void) -> *const c_char {
    static NAME: &[u8] = b"Lapinux\0";
    NAME.as_ptr() as *const c_char
}
extern "C" fn friends_state(_: *mut c_void) -> i32 {
    1
}
extern "C" fn friends_count(_: *mut c_void, _: i32) -> i32 {
    0
}

/// `ISteamFriends` : un nom, un état, aucun ami — le social Steam n'existe pas
/// sans client.
extern "C" fn friends_object() -> *mut c_void {
    static OBJECT: OnceLock<usize> = OnceLock::new();
    *OBJECT.get_or_init(|| {
        make_object(&[
            slot!(friends_name),
            slot!(friends_state),
            slot!(friends_count),
        ]) as usize
    }) as *mut c_void
}

// ---------------------------------------------------------------------------
// API plate
// ---------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn SteamAPI_Init() -> bool {
    init()
}

#[no_mangle]
pub extern "C" fn SteamAPI_InitSafe() -> bool {
    init()
}

#[no_mangle]
pub extern "C" fn SteamAPI_Shutdown() {
    ACTIVE.store(false, Ordering::Relaxed);
    CONTEXT_GENERATION.fetch_add(1, Ordering::Relaxed);
}

#[no_mangle]
pub extern "C" fn SteamAPI_RunCallbacks() {}

#[no_mangle]
pub extern "C" fn SteamAPI_IsSteamRunning() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

#[no_mangle]
pub extern "C" fn SteamAPI_RestartAppIfNecessary(_: u32) -> bool {
    false
}

#[no_mangle]
pub extern "C" fn SteamAPI_GetHSteamUser() -> i32 {
    1
}

#[no_mangle]
pub extern "C" fn SteamAPI_GetHSteamPipe() -> i32 {
    1
}

#[no_mangle]
pub extern "C" fn SteamAPI_GetSteamInstallPath() -> *const c_char {
    std::ptr::null()
}

// InitEx rend ESteamAPIInitResult : zéro est le succès, pas un booléen.
// Les versions d'interface ne sont pas validées ici : voir les limites du pont.
#[no_mangle]
pub extern "C" fn SteamInternal_SteamAPI_Init(_: *const c_char, error: *mut c_char) -> i32 {
    init_flat(error)
}

#[no_mangle]
pub extern "C" fn SteamAPI_InitFlat(error: *mut c_char) -> i32 {
    init_flat(error)
}

fn init_flat(error: *mut c_char) -> i32 {
    let ok = init();
    if !error.is_null() {
        // SteamErrMsg contient 1024 octets ; la chaîne ci-dessous est bornée.
        let message: &[u8] = if ok { b"\0" } else { b"Lapinux Steamworks: session Steam ou AppId absent\0" };
        unsafe { std::ptr::copy_nonoverlapping(message.as_ptr(), error.cast::<u8>(), message.len()); }
    }
    if ok { 0 } else { 1 }
}

#[no_mangle]
pub extern "C" fn SteamInternal_SteamAPI_Shutdown() {
    SteamAPI_Shutdown();
}

#[no_mangle]
pub extern "C" fn SteamInternal_SteamAPI_RunCallbacks() {}

#[no_mangle]
pub extern "C" fn SteamInternal_SteamAPI_IsSteamRunning() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

#[no_mangle]
pub extern "C" fn SteamInternal_SteamAPI_RestartAppIfNecessary(_: u32) -> bool {
    false
}

#[no_mangle]
pub extern "C" fn SteamInternal_SteamAPI_GetHSteamUser() -> i32 {
    1
}

#[no_mangle]
pub extern "C" fn SteamInternal_SteamAPI_GetHSteamPipe() -> i32 {
    1
}

#[no_mangle]
pub extern "C" fn SteamInternal_SteamAPI_GetSteamInstallPath() -> *const c_char {
    std::ptr::null()
}

#[no_mangle]
pub extern "C" fn SteamInternal_SteamAPI_RegisterCallback(_: *mut c_void, _: i32) {}

#[no_mangle]
pub extern "C" fn SteamInternal_SteamAPI_UnregisterCallback(_: *mut c_void) {}

#[no_mangle]
pub extern "C" fn SteamInternal_SteamAPI_RegisterCallResult(_: *mut c_void, _: u64) {}

#[no_mangle]
pub extern "C" fn SteamInternal_SteamAPI_UnregisterCallResult(_: *mut c_void, _: u64) {}

macro_rules! flat_accessors {
    ($( $name:ident => $object:expr ),* $(,)?) => {
        $(
            #[no_mangle]
            pub extern "C" fn $name(_: i32, _: i32) -> *mut c_void {
                $object
            }
        )*
    };
}

flat_accessors!(
    SteamAPI_ISteamUserStats => stats_object(),
    SteamAPI_ISteamUser => user_object(),
    SteamAPI_ISteamApps => apps_object(),
    SteamAPI_ISteamUtils => utils_object(),
    SteamAPI_ISteamFriends => friends_object(),
    SteamAPI_ISteamRemoteStorage => remote_storage_object(),
    SteamAPI_ISteamMatchmaking => matchmaking_object(),
    SteamAPI_ISteamMatchmakingServers => matchmaking_servers_object(),
    SteamAPI_ISteamNetworking => networking_object(),
    SteamAPI_ISteamNetworkingSockets => networking_sockets_object(),
    SteamAPI_ISteamNetworkingUtils => networking_utils_object(),
    SteamAPI_ISteamScreenshots => screenshots_object(),
    SteamAPI_ISteamHTTP => http_object(),
    SteamAPI_ISteamInventory => inventory_object(),
    SteamAPI_ISteamUGC => ugc_object(),
    SteamAPI_ISteamMusic => music_object(),
    SteamAPI_ISteamMusicRemote => music_remote_object(),
    SteamAPI_ISteamHTMLSurface => html_surface_object(),
    SteamAPI_ISteamController => controller_object(),
    SteamAPI_ISteamInput => input_object(),
    SteamAPI_ISteamVideo => video_object(),
    SteamAPI_ISteamAppList => app_list_object(),
    SteamAPI_ISteamParentalSettings => parental_object(),
    SteamAPI_ISteamGameSearch => game_search_object(),
    SteamAPI_ISteamParties => parties_object(),
    SteamAPI_ISteamTimeline => timeline_object(),
    SteamAPI_ISteamGameServer => game_server_object(),
    SteamAPI_ISteamGameServerStats => game_server_stats_object(),
    SteamAPI_ISteamGameCoordinator => game_coordinator_object(),
    SteamAPI_ISteamMasterServerUpdater => master_server_updater_object(),
);

/// Accès de la forme `SteamAPI_SteamUserStats_v012()`, laissés par les jeux
/// compilés avec l'enveloppe C++ de Valve.
macro_rules! versioned_accessors {
    ($( $name:ident => $object:expr ),* $(,)?) => {
        $(
            #[no_mangle]
            pub extern "C" fn $name() -> *mut c_void {
                $object
            }
        )*
    };
}

versioned_accessors!(
    SteamAPI_SteamUserStats_v012 => stats_object(),
    SteamAPI_SteamUserStats_v013 => stats_object_version(true),
    SteamAPI_SteamUser_v023 => user_object(),
    SteamAPI_SteamApps_v008 => apps_object(),
    SteamAPI_SteamUtils_v010 => utils_object(),
    SteamAPI_SteamFriends_v018 => friends_object(),
    SteamAPI_SteamFriends_v017 => friends_object(),
    SteamAPI_SteamRemoteStorage_v016 => remote_storage_object(),
    SteamAPI_SteamMatchmaking_v009 => matchmaking_object(),
    SteamAPI_SteamMatchmakingServers_v002 => matchmaking_servers_object(),
    SteamAPI_SteamNetworking_v006 => networking_object(),
    SteamAPI_SteamNetworkingSockets_v012 => networking_sockets_object(),
    SteamAPI_SteamNetworkingUtils_v004 => networking_utils_object(),
    SteamAPI_SteamScreenshots_v003 => screenshots_object(),
    SteamAPI_SteamHTTP_v003 => http_object(),
    SteamAPI_SteamInventory_v001 => inventory_object(),
    SteamAPI_SteamUGC_v021 => ugc_object(),
    SteamAPI_SteamMusic_v001 => music_object(),
    SteamAPI_SteamMusicRemote_v001 => music_remote_object(),
    SteamAPI_SteamHTMLSurface_v005 => html_surface_object(),
    SteamAPI_SteamController_v006 => controller_object(),
    SteamAPI_SteamInput_v006 => input_object(),
    SteamAPI_SteamVideo_v002 => video_object(),
    SteamAPI_SteamAppList_v001 => app_list_object(),
    SteamAPI_SteamParentalSettings_v001 => parental_object(),
    SteamAPI_SteamGameSearch_v001 => game_search_object(),
    SteamAPI_SteamParties_v002 => parties_object(),
    SteamAPI_SteamTimeline_v001 => timeline_object(),
    SteamAPI_SteamGameServer_v015 => game_server_object(),
    SteamAPI_SteamGameServerStats_v001 => game_server_stats_object(),
    SteamAPI_SteamGameCoordinator_v001 => game_coordinator_object(),
);

/// Méthodes plates de `ISteamUserStats` : les jeux récents appellent
/// directement ces symboles plutôt que par la table.
macro_rules! stats_flats {
    ($( $name:ident ( $($arg:ident : $ty:ty),* ) -> $ret:ty { $body:expr } ),* $(,)?) => {
        $(
            #[no_mangle]
            pub extern "C" fn $name($($arg: $ty),*) -> $ret {
                $body
            }
        )*
    };
}

stats_flats!(
    SteamAPI_ISteamUserStats_RequestCurrentStats(this: *mut c_void) -> bool {
        us_request_current_stats(this)
    },
    SteamAPI_ISteamUserStats_GetStatInt32(
        this: *mut c_void,
        name: *const c_char,
        out: *mut i32
    ) -> bool {
        us_get_stat_int(this, name, out)
    },
    SteamAPI_ISteamUserStats_GetStatFloat(
        this: *mut c_void,
        name: *const c_char,
        out: *mut f32
    ) -> bool {
        us_get_stat_float(this, name, out)
    },
    SteamAPI_ISteamUserStats_SetStatInt32(this: *mut c_void, name: *const c_char, value: i32) -> bool {
        us_set_stat_int(this, name, value)
    },
    SteamAPI_ISteamUserStats_SetStatFloat(this: *mut c_void, name: *const c_char, value: f32) -> bool {
        us_set_stat_float(this, name, value)
    },
    SteamAPI_ISteamUserStats_UpdateAvgRateStat(
        this: *mut c_void,
        name: *const c_char,
        count: f32,
        length: f64
    ) -> bool {
        us_update_avg_rate_stat(this, name, count, length)
    },
    SteamAPI_ISteamUserStats_GetAchievement(
        this: *mut c_void,
        name: *const c_char,
        out: *mut bool
    ) -> bool {
        us_get_achievement(this, name, out)
    },
    SteamAPI_ISteamUserStats_SetAchievement(this: *mut c_void, name: *const c_char) -> bool {
        us_set_achievement(this, name)
    },
    SteamAPI_ISteamUserStats_ClearAchievement(this: *mut c_void, name: *const c_char) -> bool {
        us_clear_achievement(this, name)
    },
    SteamAPI_ISteamUserStats_GetAchievementAndUnlockTime(
        this: *mut c_void,
        name: *const c_char,
        out: *mut bool,
        time: *mut u32
    ) -> bool {
        us_get_achievement_and_unlock_time(this, name, out, time)
    },
    SteamAPI_ISteamUserStats_StoreStats(this: *mut c_void) -> bool {
        us_store_stats(this)
    },
    SteamAPI_ISteamUserStats_GetAchievementIcon(this: *mut c_void, name: *const c_char) -> i32 {
        us_get_achievement_icon(this, name)
    },
    SteamAPI_ISteamUserStats_GetAchievementDisplayAttribute(
        this: *mut c_void,
        name: *const c_char,
        key: *const c_char
    ) -> *const c_char {
        us_get_achievement_display_attribute(this, name, key)
    },
    SteamAPI_ISteamUserStats_IndicateAchievementProgress(
        this: *mut c_void,
        name: *const c_char,
        current: u32,
        max: u32
    ) -> bool {
        us_indicate_achievement_progress(this, name, current, max)
    },
    SteamAPI_ISteamUserStats_GetNumAchievements(this: *mut c_void) -> u32 {
        us_get_num_achievements(this)
    },
    SteamAPI_ISteamUserStats_GetAchievementName(this: *mut c_void, index: u32) -> *const c_char {
        us_get_achievement_name(this, index)
    },
    SteamAPI_ISteamUserStats_ResetAllStats(this: *mut c_void, wipe: bool) -> bool {
        us_reset_all_stats(this, wipe)
    },
);

#[no_mangle]
pub extern "C" fn SteamAPI_ISteamUser_BLoggedOn(this: *mut c_void) -> bool {
    user_logged_on(this)
}

#[no_mangle]
pub extern "C" fn SteamAPI_ISteamUser_GetSteamID(this: *mut c_void) -> u64 {
    user_steam_id(this)
}

#[no_mangle]
pub extern "C" fn SteamAPI_ISteamApps_GetCurrentGameLanguage(this: *mut c_void) -> *const c_char {
    apps_language(this)
}

#[no_mangle]
pub extern "C" fn SteamAPI_ISteamUtils_GetAppID(this: *mut c_void) -> u32 {
    utils_app_id(this)
}

// --- Serveur de jeu : absent, mais les symboles doivent exister ---

#[no_mangle]
pub extern "C" fn SteamGameServer_Init(
    _: u32,
    _: u16,
    _: u16,
    _: u16,
    _: i32,
    _: *const c_char,
) -> bool {
    false
}

#[no_mangle]
pub extern "C" fn SteamGameServer_InitSafe(
    _: u32,
    _: u16,
    _: u16,
    _: u16,
    _: i32,
    _: *const c_char,
) -> bool {
    false
}

#[no_mangle]
pub extern "C" fn SteamInternal_GameServer_Init(
    _: u32,
    _: u16,
    _: u16,
    _: u16,
    _: i32,
    _: *const c_char,
) -> bool {
    false
}

#[no_mangle]
pub extern "C" fn SteamGameServer_Shutdown() {}

#[no_mangle]
pub extern "C" fn SteamGameServer_RunCallbacks() {}

#[no_mangle]
pub extern "C" fn SteamGameServer_GetHSteamPipe() -> i32 {
    1
}

#[no_mangle]
pub extern "C" fn SteamGameServer_GetHSteamUser() -> i32 {
    1
}

// --- Résolution par nom de version ---

#[no_mangle]
pub extern "C" fn SteamInternal_FindOrCreateUserInterface(
    _: i32,
    version: *const c_char,
) -> *mut c_void {
    interface(version)
}

#[no_mangle]
pub extern "C" fn SteamInternal_CreateInterface(version: *const c_char) -> *mut c_void {
    interface(version)
}

#[no_mangle]
pub extern "C" fn SteamInternal_FindOrCreateGameServerInterface(
    _: i32,
    version: *const c_char,
) -> *mut c_void {
    interface(version)
}

/// Layout SDK : pointeur vers l'initialiseur, génération, puis contexte.
/// Le SDK fournit un stockage valide et correctement aligné.
#[no_mangle]
pub unsafe extern "C" fn SteamInternal_ContextInit(context: *mut c_void) -> *mut c_void {
    if context.is_null() { return std::ptr::null_mut(); }
    let words = context.cast::<usize>();
    let data = words.add(2).cast::<c_void>();
    let generation = CONTEXT_GENERATION.load(Ordering::Relaxed);
    if *words.add(1) != generation {
        let callback: unsafe extern "C" fn(*mut c_void) = std::mem::transmute(*words);
        callback(data);
        *words.add(1) = generation;
    }
    data
}

#[no_mangle]
pub extern "C" fn SteamAPI_RegisterCallback(_: *mut c_void, _: i32) {}

#[no_mangle]
pub extern "C" fn SteamAPI_UnregisterCallback(_: *mut c_void) {}

#[no_mangle]
pub extern "C" fn SteamAPI_RegisterCallResult(_: *mut c_void, _: u64) {}

#[no_mangle]
pub extern "C" fn SteamAPI_UnregisterCallResult(_: *mut c_void, _: u64) {}

#[no_mangle]
pub extern "C" fn SteamAPI_ManualDispatch_Init() {}

#[no_mangle]
pub extern "C" fn SteamAPI_ManualDispatch_RunFrame(_: i32) {}

#[no_mangle]
pub extern "C" fn SteamAPI_ManualDispatch_GetNextCallback(_: i32, _: *mut c_void) -> bool {
    false
}

#[no_mangle]
pub extern "C" fn SteamAPI_ManualDispatch_FreeLastCallback(_: i32) {}

#[no_mangle]
pub extern "C" fn SteamAPI_ManualDispatch_GetAPICallResult(
    _: i32,
    _: u64,
    _: *mut c_void,
    _: i32,
    _: i32,
    _: *mut bool,
) -> bool {
    false
}

/// Table servie pour un nom de version : les cinq interfaces réelles, les
/// autres en neutre, et jamais de pointeur nul pour une version inconnue.
fn interface(version: *const c_char) -> *mut c_void {
    let Some(version) = (unsafe { text(version) }) else {
        return std::ptr::null_mut();
    };
    let object = if version == "STEAMUSERSTATS_INTERFACE_VERSION013" {
        stats_object_version(true)
    } else if version == "STEAMUSERSTATS_INTERFACE_VERSION012" {
        stats_object()
    } else if version.starts_with("STEAMUSER_INTERFACE_VERSION") || version == "SteamUser023" {
        user_object()
    } else if version.starts_with("STEAMAPPS_INTERFACE_VERSION") {
        apps_object()
    } else if version.starts_with("STEAMUTILS_INTERFACE_VERSION") || version == "SteamUtils010" {
        utils_object()
    } else if version.starts_with("STEAMFRIENDS_INTERFACE_VERSION") || version == "SteamFriends017" {
        friends_object()
    } else if version.starts_with("STEAMREMOTESTORAGE_INTERFACE_VERSION") {
        remote_storage_object()
    } else if version.starts_with("STEAMMATCHMAKING_INTERFACE_VERSION") {
        matchmaking_object()
    } else if version.starts_with("STEAMMATCHMAKINGSERVERS_INTERFACE_VERSION") {
        matchmaking_servers_object()
    } else if version.starts_with("STEAMNETWORKINGSOCKETS_INTERFACE_VERSION") {
        networking_sockets_object()
    } else if version.starts_with("STEAMNETWORKINGUTILS_INTERFACE_VERSION") {
        networking_utils_object()
    } else if version.starts_with("STEAMNETWORKING_INTERFACE_VERSION") {
        networking_object()
    } else if version.starts_with("STEAMSCREENSHOTS_INTERFACE_VERSION") {
        screenshots_object()
    } else if version.starts_with("STEAMHTTP_INTERFACE_VERSION") {
        http_object()
    } else if version.starts_with("STEAMINVENTORY_INTERFACE_VERSION") {
        inventory_object()
    } else if version.starts_with("STEAMUGC_INTERFACE_VERSION") {
        ugc_object()
    } else if version.starts_with("STEAMMUSICREMOTE_INTERFACE_VERSION") {
        music_remote_object()
    } else if version.starts_with("STEAMMUSIC_INTERFACE_VERSION") {
        music_object()
    } else if version.starts_with("STEAMHTMLSURFACE_INTERFACE_VERSION") {
        html_surface_object()
    } else if version.starts_with("STEAMCONTROLLER_INTERFACE_VERSION") {
        controller_object()
    } else if version.starts_with("STEAMINPUT_INTERFACE_VERSION") {
        input_object()
    } else if version.starts_with("STEAMVIDEO_INTERFACE_VERSION") {
        video_object()
    } else if version.starts_with("STEAMAPPLIST_INTERFACE_VERSION") {
        app_list_object()
    } else if version.starts_with("STEAMPARENTALSETTINGS_INTERFACE_VERSION") {
        parental_object()
    } else if version.starts_with("STEAMGAMESEARCH_INTERFACE_VERSION") {
        game_search_object()
    } else if version.starts_with("STEAMPARTIES_INTERFACE_VERSION") {
        parties_object()
    } else if version.starts_with("STEAMTIMELINE_INTERFACE_VERSION") {
        timeline_object()
    } else if version.starts_with("STEAMGAMESERVERSTATS_INTERFACE_VERSION") {
        game_server_stats_object()
    } else if version.starts_with("STEAMGAMESERVER_INTERFACE_VERSION") {
        game_server_object()
    } else if version.starts_with("STEAMGAMECOORDINATOR_INTERFACE_VERSION") {
        game_coordinator_object()
    } else if version.starts_with("STEAMMASTERSERVERUPDATER_INTERFACE_VERSION") {
        master_server_updater_object()
    } else {
        stub_object(version)
    };
    object
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(value: &str) -> CString {
        CString::new(value).unwrap()
    }

    #[test]
    fn the_shim_captures_only_what_the_game_unlocks() {
        // Essai sans réseau : les lots partent dans le journal.
        std::env::set_var(DRY_RUN, "1");
        std::env::set_var("SteamAppId", "480");
        assert!(SteamAPI_Init());
        assert!(SteamAPI_IsSteamRunning());

        let stats = stats_object();
        let win = name("ACH_WIN");
        assert!(us_set_achievement(stats, win.as_ptr()));
        assert!(us_store_stats(stats));

        {
            let jobs = DRY_JOBS.lock().unwrap();
            assert_eq!(jobs.len(), 1);
            assert_eq!(jobs[0].appid, 480);
            assert_eq!(jobs[0].names, vec!["ACH_WIN".to_string()]);
        }

        // Rien de neuf : aucun second envoi.
        assert!(us_store_stats(stats));
        assert_eq!(DRY_JOBS.lock().unwrap().len(), 1);

        // Un succès effacé avant l'envoi ne part pas.
        assert!(us_clear_achievement(stats, win.as_ptr()));
        assert!(us_store_stats(stats));
        assert_eq!(DRY_JOBS.lock().unwrap().len(), 1);

        // L'état reste lisible pour le jeu.
        let mut unlocked = false;
        assert!(us_get_achievement(stats, win.as_ptr(), &mut unlocked));
        assert!(!unlocked);
        assert_eq!(us_get_num_achievements(stats), 1);
    }

    #[test]
    fn every_interface_version_gets_a_callable_table() {
        let stats = name("STEAMUSERSTATS_INTERFACE_VERSION012");
        assert!(!SteamInternal_FindOrCreateUserInterface(0, stats.as_ptr()).is_null());
        let apps = name("STEAMAPPS_INTERFACE_VERSION008");
        assert!(!SteamInternal_CreateInterface(apps.as_ptr()).is_null());
        // Une interface non servie rend une table neutre. Ses signatures
        // n'étant pas toutes connues, elle ne garantit pas l'absence de crash.
        let unknown = name("STEAMREMOTESTORAGE_INTERFACE_VERSION016");
        assert!(!SteamInternal_CreateInterface(unknown.as_ptr()).is_null());
        let invented = name("STEAMINVENTED_INTERFACE_VERSION999");
        assert!(!SteamInternal_CreateInterface(invented.as_ptr()).is_null());
        // Seul un nom absent n'a pas d'interface.
        assert!(SteamInternal_CreateInterface(std::ptr::null()).is_null());
    }

    #[test]
    fn the_neutral_table_answers_zero() {
        let unknown = name("STEAMMACHIN_INTERFACE_VERSION001");
        let object = SteamInternal_CreateInterface(unknown.as_ptr());
        assert!(!object.is_null());
        // Premier emplacement de la table : la réponse neutre.
        let vtable = unsafe { *(object as *const *const usize) };
        assert!(!vtable.is_null());
        let first: extern "C" fn() -> u64 = unsafe { std::mem::transmute(*vtable) };
        assert_eq!(first(), 0);
    }

    #[test]
    fn the_flat_and_versioned_accessors_are_the_same_objects() {
        let flat = SteamAPI_ISteamUserStats(0, 0);
        let versioned = SteamAPI_SteamUserStats_v012();
        assert_eq!(flat, versioned);
        assert_eq!(SteamAPI_ISteamUserStats(0, 0), flat);
        assert!(!SteamAPI_ISteamRemoteStorage(0, 0).is_null());
        assert!(!SteamAPI_SteamUGC_v021().is_null());
    }

}
