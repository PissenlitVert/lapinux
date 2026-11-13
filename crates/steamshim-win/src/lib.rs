//! Pont `steam_api64.dll` pour les jeux Windows lances sous Proton.
//!
//! Le DLL n'ouvre aucune connexion : il retient les succes que le jeu accorde
//! lui-meme (`SetAchievement`) et, au `StoreStats`, les depose dans une file
//! (un fichier JSON par lot). Le daemon Lapinux vide cette file et ecrit avec
//! sa session Steam partagee, sans client Steam.
//!
//! Aucun succes n'est invente : seuls les noms passes par le jeu sont ecrits.

use std::collections::HashSet;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_void};
use std::path::PathBuf;
#[cfg(windows)]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

const SLOTS: usize = 104;

#[derive(Default)]
struct State {
    appid: u32,
    pending: Vec<String>,
    unlocked: HashSet<String>,
    flushed: u64,
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(State::default()))
}

fn appid() -> u32 {
    let mut s = state().lock().unwrap();
    if s.appid == 0 {
        s.appid = std::env::var("SteamAppId")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0);
    }
    s.appid
}

fn spool_dir() -> Option<PathBuf> {
    std::env::var_os("LAPINUX_ACHIEVEMENT_SPOOL").map(PathBuf::from)
}

/// Journal de diagnostic du pont, dans le dossier du spool. Il permet de
/// distinguer un jeu qui n'appelle pas Steam d'un jeu dont l'appel echoue.
fn debug_log(message: &str) {
    let Some(dir) = spool_dir() else { return };
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    // À côté du spool, jamais dedans : le daemon et les tests comptent les
    // fichiers du spool comme des lots d'achievements.
    let path = dir
        .parent()
        .map(|parent| parent.join("shim.log"))
        .unwrap_or_else(|| dir.join("shim.log"));
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        use std::io::Write;
        let _ = writeln!(file, "{message}");
    }
}

fn debug_once(tag: &'static str, message: &str) {
    static SEEN: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let seen = SEEN.get_or_init(|| Mutex::new(HashSet::new()));
    let mut seen = seen.lock().unwrap();
    if seen.insert(tag) {
        drop(seen);
        debug_log(message);
    }
}

/// Journalise la première utilisation d'un emplacement avec le nom de son
/// interface. Sans ce nom, « slot 41 » ne dit pas quel contrat est enfreint.
fn debug_slot(interface: usize, slot: usize) {
    static SEEN: OnceLock<Vec<std::sync::atomic::AtomicBool>> = OnceLock::new();
    let seen = SEEN.get_or_init(|| {
        (0..INTERFACE_LABELS.len() * SLOTS)
            .map(|_| false.into())
            .collect()
    });
    trace_push(interface, slot, 0);
    if let Some(flag) = seen.get(interface * SLOTS + slot) {
        if !flag.swap(true, Ordering::Relaxed) {
            if let Some(label) = INTERFACE_LABELS.get(interface) {
                debug_log(&format!("{label} slot {slot}"));
            }
        }
    }
}

/// Indices des interfaces dans `INTERFACE_LABELS` (ordre de `build.rs`).
const IFACE_USER: usize = 0;
const IFACE_FRIENDS: usize = 1;
const IFACE_UTILS: usize = 2;
const IFACE_STATS: usize = 3;
const IFACE_MATCHMAKING: usize = 4;
const IFACE_APPS: usize = 5;
const IFACE_UGC: usize = 7;

/// Les 256 derniers appels d'interface, gardés en mémoire. Un crash ne laisse
/// pas le temps d'écrire un journal : ce tampon est vidé par le gestionnaire
/// d'exceptions installé au démarrage. Les tables d'interface étant remplies
/// au premier accès, ce journal dit exactement quel contrat le jeu attendait.
const TRACE_LEN: usize = 256;

static TRACE_TAGS: [AtomicU32; TRACE_LEN] = [const { AtomicU32::new(u32::MAX) }; TRACE_LEN];
static TRACE_VALUES: [AtomicU64; TRACE_LEN] = [const { AtomicU64::new(0) }; TRACE_LEN];
static TRACE_SEQ: AtomicUsize = AtomicUsize::new(0);

fn trace_push(interface: usize, slot: usize, value: u64) {
    let index = TRACE_SEQ.fetch_add(1, Ordering::Relaxed) % TRACE_LEN;
    TRACE_VALUES[index].store(value, Ordering::Relaxed);
    TRACE_TAGS[index].store(
        ((interface as u32) << 8) | (slot as u32 & 0xff),
        Ordering::Relaxed,
    );
}

/// Enregistre un appel réel avec sa valeur de retour. Le dernier enregistrement
/// avant le crash désigne l'appel fautif.
fn trace_value(interface: usize, slot: usize, value: u64) {
    trace_push(interface, slot, value);
}

#[cfg(windows)]
#[repr(C)]
struct ExceptionRecord {
    code: u32,
    flags: u32,
    previous: *mut ExceptionRecord,
    address: *mut c_void,
    parameters: u32,
    alignment: u32,
    information: [usize; 15],
}

#[cfg(windows)]
#[repr(C)]
struct ExceptionPointers {
    record: *mut ExceptionRecord,
    context: *mut c_void,
}

/// Écrit le tampon avant que le jeu ne meure. Rend `CONTINUE_SEARCH` pour que
/// le rapport de crash d'Unreal se produise normalement.
#[cfg(windows)]
extern "system" fn trace_crash(info: *mut c_void) -> i32 {
    const ACCESS_VIOLATION: u32 = 0xC000_0005;
    if info.is_null() {
        return 0;
    }
    let pointers = unsafe { &*(info.cast::<ExceptionPointers>()) };
    if pointers.record.is_null() {
        return 0;
    }
    let record = unsafe { &*pointers.record };
    if record.code != ACCESS_VIOLATION {
        return 0;
    }
    static DUMPED: AtomicBool = AtomicBool::new(false);
    if DUMPED.swap(true, Ordering::Relaxed) {
        return 0;
    }
    let fault = record.information[1];
    let rip = if pointers.context.is_null() {
        0
    } else {
        unsafe { *pointers.context.cast::<u8>().add(0xf8).cast::<u64>() }
    };
    debug_log(&format!(
        "=== crash : code {:#x}, adresse {fault:#x}, rip {rip:#x} ===",
        record.code
    ));
    let start = TRACE_SEQ.load(Ordering::Relaxed);
    for offset in 0..TRACE_LEN {
        let index = (start + offset) % TRACE_LEN;
        let tag = TRACE_TAGS[index].load(Ordering::Relaxed);
        if tag == u32::MAX {
            continue;
        }
        let interface = (tag >> 8) as usize;
        let slot = (tag & 0xff) as usize;
        let label = INTERFACE_LABELS.get(interface).copied().unwrap_or("?");
        debug_log(&format!(
            "  {label} slot {slot} -> {:#x}",
            TRACE_VALUES[index].load(Ordering::Relaxed)
        ));
    }
    debug_log("=== fin du journal de crash ===");
    0
}

#[cfg(windows)]
fn install_crash_trace() {
    static INSTALLED: AtomicBool = AtomicBool::new(false);
    if INSTALLED.swap(true, Ordering::Relaxed) {
        return;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn AddVectoredExceptionHandler(
            first: u32,
            handler: extern "system" fn(*mut c_void) -> i32,
        ) -> *mut c_void;
    }
    unsafe {
        AddVectoredExceptionHandler(1, trace_crash);
    }
}

/// Les tests tournent sur l'hôte Linux : pas de gestionnaire vecteur à y
/// installer, la trace reste consultable par les tests unitaires.
#[cfg(not(windows))]
fn install_crash_trace() {}

/// Chaîne vide valide. Un jeu Unreal construit un `FString` à partir du
/// pointeur rendu : rendre zéro provoque une lecture à l'adresse -1.
fn empty_c_string() -> *const c_char {
    b"\0".as_ptr().cast()
}

include!(concat!(env!("OUT_DIR"), "/generic_stubs.rs"));

fn json_escape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Depose le lot en attente dans la file. Retourne false si la file n'est pas
/// configuree ou ne peut pas etre ecrite (le lot reste alors en attente).
fn flush_pending() -> bool {
    let Some(dir) = spool_dir() else { return false };
    if std::fs::create_dir_all(&dir).is_err() {
        return false;
    }
    let mut s = state().lock().unwrap();
    if s.pending.is_empty() {
        return true;
    }
    let appid = s.appid;
    let names: Vec<String> = s.pending.drain(..).collect();
    let body = format!(
        "{{\"appid\":{},\"names\":[{}]}}\n",
        appid,
        names
            .iter()
            .map(|n| format!("\"{}\"", json_escape(n)))
            .collect::<Vec<_>>()
            .join(",")
    );
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let file = dir.join(format!(
        "lot-{}-{}.json",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    if std::fs::write(&file, body).is_err() {
        s.pending = names;
        return false;
    }
    for n in &names {
        s.unlocked.insert(n.clone());
    }
    s.flushed += 1;
    true
}

fn set_achievement(name: &CStr) {
    let Ok(text) = name.to_str() else { return };
    let mut s = state().lock().unwrap();
    if s.unlocked.contains(text) || s.pending.iter().any(|p| p == text) {
        return;
    }
    s.pending.push(text.to_string());
}

fn clear_achievement(name: &CStr) {
    let Ok(text) = name.to_str() else { return };
    state().lock().unwrap().pending.retain(|p| p != text);
}

fn is_unlocked(name: &CStr) -> bool {
    name.to_str()
        .map(|t| state().lock().unwrap().unlocked.contains(t))
        .unwrap_or(false)
}

/// 0 = `RequestCurrentStats` : appele avec le seul `this` juste apres
/// l'initialisation. Le brancher sur `GetStat` faisait ecrire le pont a travers
/// un troisieme argument inexistant (0x7fffffff) et tuait le jeu au demarrage.
extern "system" fn stats_request_current_stats(_this: *mut c_void) -> bool {
    trace_value(IFACE_STATS, 0, 1);
    true
}

extern "system" fn stats_get_stat_int(
    _this: *mut c_void,
    _name: *const c_char,
    value: *mut i32,
) -> bool {
    if !value.is_null() {
        unsafe { *value = 0 };
    }
    false
}

extern "system" fn stats_get_stat_float(
    _this: *mut c_void,
    _name: *const c_char,
    value: *mut f32,
) -> bool {
    if !value.is_null() {
        unsafe { *value = 0.0 };
    }
    false
}

extern "system" fn stats_set_stat_int(
    _this: *mut c_void,
    _name: *const c_char,
    _value: i32,
) -> bool {
    true
}

extern "system" fn stats_set_stat_float(
    _this: *mut c_void,
    _name: *const c_char,
    _value: f32,
) -> bool {
    true
}

extern "system" fn stats_update_avg_rate(
    _this: *mut c_void,
    _name: *const c_char,
    _session: f32,
    _seconds: f64,
) -> bool {
    true
}

extern "system" fn stats_get_achievement(
    _this: *mut c_void,
    name: *const c_char,
    achieved: *mut bool,
) -> bool {
    trace_value(IFACE_STATS, 5, 0);
    if name.is_null() || achieved.is_null() {
        return false;
    }
    let value = is_unlocked(unsafe { CStr::from_ptr(name) });
    unsafe { *achieved = value };
    true
}

extern "system" fn stats_set_achievement(_this: *mut c_void, name: *const c_char) -> bool {
    trace_value(IFACE_STATS, 6, 0);
    if name.is_null() {
        return false;
    }
    set_achievement(unsafe { CStr::from_ptr(name) });
    true
}

extern "system" fn stats_clear_achievement(_this: *mut c_void, name: *const c_char) -> bool {
    trace_value(IFACE_STATS, 7, 0);
    if name.is_null() {
        return false;
    }
    clear_achievement(unsafe { CStr::from_ptr(name) });
    true
}

extern "system" fn stats_achievement_unlock_time(
    _this: *mut c_void,
    _name: *const c_char,
    achieved: *mut bool,
    timestamp: *mut u32,
) -> bool {
    if !achieved.is_null() {
        unsafe { *achieved = false };
    }
    if !timestamp.is_null() {
        unsafe { *timestamp = 0 };
    }
    false
}

extern "system" fn stats_store(_this: *mut c_void) -> bool {
    trace_value(IFACE_STATS, 9, 0);
    flush_pending()
}

extern "system" fn stats_achievement_icon(_this: *mut c_void, _name: *const c_char) -> i32 {
    0
}

/// 11 = `GetAchievementDisplayAttribute` : le jeu affiche la valeur telle
/// quelle, un pointeur nul y ferait lire une chaîne invalide.
extern "system" fn stats_display_attribute(
    _this: *mut c_void,
    _name: *const c_char,
    _key: *const c_char,
) -> *const c_char {
    empty_c_string()
}

extern "system" fn stats_indicate_progress(
    _this: *mut c_void,
    _name: *const c_char,
    _current: u32,
    _maximum: u32,
) -> bool {
    false
}

extern "system" fn stats_num_achievements(_this: *mut c_void) -> u32 {
    0
}

extern "system" fn stats_achievement_name(_this: *mut c_void, _index: u32) -> *const c_char {
    empty_c_string()
}

/// 24 = `GetLeaderboardName` : rend une chaîne, jamais un pointeur nul.
extern "system" fn stats_leaderboard_name(_this: *mut c_void, _handle: u64) -> *const c_char {
    empty_c_string()
}

extern "system" fn stats_request(_this: *mut c_void) -> bool {
    true
}

fn table() -> *const usize {
    static TABLE: OnceLock<Box<[usize; SLOTS]>> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut slots = stubs_stats();
        // `ISteamUserStats012/013` : `RequestCurrentStats` ouvre la table, puis
        // viennent les deux `GetStat`. Tout decalage ici fait appeler un
        // accesseur avec les arguments du voisin (ecriture sauvage).
        slots[0] = stats_request_current_stats as usize;
        slots[1] = stats_get_stat_int as usize;
        slots[2] = stats_get_stat_float as usize;
        slots[3] = stats_set_stat_int as usize;
        slots[4] = stats_set_stat_float as usize;
        slots[5] = stats_update_avg_rate as usize;
        slots[6] = stats_get_achievement as usize;
        slots[7] = stats_set_achievement as usize;
        slots[8] = stats_clear_achievement as usize;
        slots[9] = stats_achievement_unlock_time as usize;
        slots[10] = stats_store as usize;
        slots[11] = stats_achievement_icon as usize;
        slots[12] = stats_display_attribute as usize;
        slots[13] = stats_indicate_progress as usize;
        slots[14] = stats_num_achievements as usize;
        slots[15] = stats_achievement_name as usize;
        slots[24] = stats_leaderboard_name as usize;
        Box::new(slots)
    });
    table.as_ptr()
}

#[no_mangle]
pub extern "system" fn SteamAPI_ISteamUserStats(_this: *mut c_void) -> *const usize {
    stats_object() as *const usize
}

#[no_mangle]
pub extern "system" fn SteamAPI_ISteamUserStats_v012(_this: *mut c_void) -> *const usize {
    stats_object() as *const usize
}

#[no_mangle]
pub extern "system" fn SteamAPI_ISteamUserStats_v013(_this: *mut c_void) -> *const usize {
    stats_object() as *const usize
}

#[no_mangle]
pub extern "system" fn SteamAPI_ISteamUserStats_v014(_this: *mut c_void) -> *const usize {
    stats_object() as *const usize
}

#[no_mangle]
pub extern "system" fn SteamAPI_ISteamUserStats_v015(_this: *mut c_void) -> *const usize {
    stats_object() as *const usize
}

#[no_mangle]
pub extern "system" fn SteamAPI_ISteamUserStats_SetAchievement(
    this: *mut c_void,
    name: *const c_char,
) -> bool {
    stats_set_achievement(this, name)
}

#[no_mangle]
pub extern "system" fn SteamAPI_ISteamUserStats_ClearAchievement(
    this: *mut c_void,
    name: *const c_char,
) -> bool {
    stats_clear_achievement(this, name)
}

#[no_mangle]
pub extern "system" fn SteamAPI_ISteamUserStats_StoreStats(this: *mut c_void) -> bool {
    stats_store(this)
}

#[no_mangle]
pub extern "system" fn SteamAPI_ISteamUserStats_GetAchievement(
    this: *mut c_void,
    name: *const c_char,
    achieved: *mut bool,
) -> bool {
    stats_get_achievement(this, name, achieved)
}

#[no_mangle]
pub extern "system" fn SteamAPI_ISteamUserStats_RequestCurrentStats(this: *mut c_void) -> bool {
    stats_request(this)
}

#[no_mangle]
pub extern "system" fn SteamAPI_Init() -> bool {
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    let call = CALLS.fetch_add(1, Ordering::Relaxed) + 1;
    debug_log(&format!("SteamAPI_Init #{call} -> true"));
    install_crash_trace();
    appid();
    true
}

#[no_mangle]
pub extern "system" fn SteamAPI_InitSafe() -> bool {
    SteamAPI_Init()
}

/// Le jeu enregistre ses callbacks avant tout appel aux statistiques. Sans
/// ces exports, Windows refuse de charger le DLL (point d'entrée absent).
fn callbacks() -> &'static Mutex<Vec<(i32, usize)>> {
    static CALLBACKS: OnceLock<Mutex<Vec<(i32, usize)>>> = OnceLock::new();
    CALLBACKS.get_or_init(|| Mutex::new(Vec::new()))
}

fn results() -> &'static Mutex<Vec<(i32, [u64; 3])>> {
    static RESULTS: OnceLock<Mutex<Vec<(i32, [u64; 3])>>> = OnceLock::new();
    RESULTS.get_or_init(|| Mutex::new(Vec::new()))
}

#[no_mangle]
pub extern "system" fn SteamAPI_RegisterCallback(callback: *mut c_void, id: i32) {
    if !callback.is_null() {
        callbacks().lock().unwrap().push((id, callback as usize));
    }
}

#[no_mangle]
pub extern "system" fn SteamAPI_UnregisterCallback(callback: *mut c_void) {
    callbacks()
        .lock()
        .unwrap()
        .retain(|(_, pointer)| *pointer != callback as usize);
}

#[no_mangle]
pub extern "system" fn SteamAPI_RegisterCallResult(_callback: *mut c_void, _handle: u64) {}

#[no_mangle]
pub extern "system" fn SteamAPI_UnregisterCallResult(_callback: *mut c_void, _handle: u64) {}

#[no_mangle]
pub extern "system" fn SteamAPI_Shutdown() {}

#[no_mangle]
pub extern "system" fn SteamAPI_RunCallbacks() {
    let pending: Vec<(i32, [u64; 3])> = std::mem::take(&mut *results().lock().unwrap());
    for (id, mut payload) in pending {
        let registered = callbacks().lock().unwrap().clone();
        for (callback_id, pointer) in registered {
            if callback_id != id {
                continue;
            }
            // `SteamAPI_RegisterCallback` reçoit un objet `CCallbackBase` dont
            // la première entrée de vtable est `Run(void*)`. Appeler l'objet
            // lui-même exécutait ses données comme du code.
            let vtable = unsafe { *(pointer as *const *const usize) };
            if vtable.is_null() {
                continue;
            }
            let run = unsafe { *vtable };
            if run == 0 {
                continue;
            }
            let callback: extern "system" fn(*mut c_void, *mut c_void) =
                unsafe { std::mem::transmute(run) };
            callback(pointer as *mut c_void, payload.as_mut_ptr().cast());
        }
    }
}

/// Dispatch manuel utilisé par Steamworks.NET depuis la v20 : le contexte
/// appelle `ManualDispatch_Init` juste après `SteamAPI_Init`. Ces exports
/// n'existent pas dans l'ancien pont ; leur absence faisait échouer
/// l'initialisation Steam du jeu avec `EntryPointNotFoundException`.
#[no_mangle]
pub extern "system" fn SteamAPI_ManualDispatch_Init() {}

#[no_mangle]
pub extern "system" fn SteamAPI_ManualDispatch_RunFrame(_pipe: i32) {
    SteamAPI_RunCallbacks();
}

#[no_mangle]
pub extern "system" fn SteamAPI_ManualDispatch_GetNextCallback(
    _pipe: i32,
    _message: *mut c_void,
) -> bool {
    false
}

#[no_mangle]
pub extern "system" fn SteamAPI_ManualDispatch_FreeLastCallback(_pipe: i32) {}

#[no_mangle]
pub extern "system" fn SteamAPI_ManualDispatch_GetAPICallResult(
    _pipe: i32,
    _call: u64,
    _callback: *mut c_void,
    _size: i32,
    _expected: i32,
    _io_failure: *mut bool,
    _result: *mut u64,
) -> bool {
    false
}

#[no_mangle]
pub extern "system" fn SteamAPI_IsSteamRunning() -> bool {
    debug_log("SteamAPI_IsSteamRunning -> true");
    true
}

#[no_mangle]
pub extern "system" fn SteamAPI_RestartAppIfNecessary(app_id: u32) -> bool {
    debug_log(&format!("SteamAPI_RestartAppIfNecessary({app_id}) -> false"));
    false
}

#[no_mangle]
pub extern "system" fn SteamAPI_GetHSteamUser() -> i32 {
    debug_once("huser", "SteamAPI_GetHSteamUser -> 1");
    1
}

#[no_mangle]
pub extern "system" fn SteamAPI_GetHSteamPipe() -> i32 {
    1
}

// Le jeu initialise aussi le contexte GameServer via SteamInternal_ContextInit.
// Ces trois exports sont delay-chargés depuis steam_api64.dll ; leur absence
// fait lever PROC_NOT_FOUND (0xC06D007F) pendant le callback de contexte.
#[no_mangle]
pub extern "system" fn SteamGameServer_GetHSteamUser() -> i32 {
    1
}

#[no_mangle]
pub extern "system" fn SteamGameServer_GetHSteamPipe() -> i32 {
    1
}

#[no_mangle]
pub extern "system" fn SteamGameServer_Shutdown() {}

#[no_mangle]
pub extern "system" fn SteamGameServer_RunCallbacks() {
    SteamAPI_RunCallbacks()
}

/// Contexte rendu par l'initialisation Steamworks. Le jeu passe un tableau
/// `[initialiseur, compteur, contexte…]` et attend que l'initialiseur soit
/// appelé une fois avec le contexte, puis que ce contexte soit renvoyé.
/// Sans cet appel, les accesseurs `SteamUser()`, `SteamFriends()`… restent
/// nuls et le jeu croit que Steam est absent.
#[no_mangle]
pub extern "system" fn SteamInternal_ContextInit(data: *mut c_void) -> *mut c_void {
    if data.is_null() {
        return std::ptr::null_mut();
    }
    let base = data.cast::<usize>();
    let init = unsafe { *base };
    let context = unsafe { base.add(2) }.cast::<c_void>();
    if init == 0 {
        return context;
    }
    // Le compteur est le second pointeur du tableau. On le réserve avant
    // l'appel : l'initialiseur peut lui-même demander une autre interface et
    // rappeler ce point d'entrée, sans interblocage.
    let counter = unsafe { &*(base.add(1) as *const std::sync::atomic::AtomicUsize) };
    if counter
        .compare_exchange(
            0,
            1,
            std::sync::atomic::Ordering::AcqRel,
            std::sync::atomic::Ordering::Acquire,
        )
        .is_ok()
    {
        let callback: extern "system" fn(*mut c_void) = unsafe { std::mem::transmute(init) };
        callback(context);
    }
    // Un contexte est interroge a chaque acces d'interface : ne journaliser
    // qu'une fois par contexte, sinon le journal grossit sans fin en jeu.
    static SEEN_CONTEXTS: OnceLock<Mutex<HashSet<usize>>> = OnceLock::new();
    let seen = SEEN_CONTEXTS.get_or_init(|| Mutex::new(HashSet::new()));
    if seen.lock().unwrap().insert(context as usize) {
        debug_log(&format!(
            "SteamInternal_ContextInit ctx={context:p} init={init:#x} -> {:p}",
            unsafe { *(context as *const *mut c_void) }
        ));
    }
    context
}

#[no_mangle]
pub extern "system" fn SteamInternal_CreateInterface(version: *const c_char) -> *mut c_void {
    if version.is_null() {
        return std::ptr::null_mut();
    }
    match unsafe { CStr::from_ptr(version) }.to_str() {
        Ok(text) => {
            debug_log(&format!("SteamInternal_CreateInterface {text}"));
            interface_for(text)
        }
        Err(_) => std::ptr::null_mut(),
    }
}

#[no_mangle]
pub extern "system" fn SteamInternal_SteamAPI_Init(
    _path: *const c_char,
    _params: *mut c_void,
) -> bool {
    SteamAPI_Init()
}

#[no_mangle]
pub extern "system" fn SteamInternal_SteamAPI_Shutdown() {
    SteamAPI_Shutdown()
}

#[no_mangle]
pub extern "system" fn SteamInternal_SteamAPI_RunCallbacks() {
    SteamAPI_RunCallbacks()
}

/// L'API C++ de Steamworks ne passe pas par les accesseurs `SteamAPI_*` :
/// elle demande son interface par version (`SteamUser023`, `SteamFriends018`,
/// `SteamMatchMaking009`…). Sans cette fabrique, le jeu n'obtient aucune
/// interface et croit que Steam est absent.
#[no_mangle]
pub extern "system" fn SteamInternal_FindOrCreateUserInterface(
    _user: i32,
    version: *const c_char,
) -> *mut c_void {
    if version.is_null() {
        return std::ptr::null_mut();
    }
    match unsafe { CStr::from_ptr(version) }.to_str() {
        Ok(text) => {
            let interface = interface_for(text);
            debug_log(&format!(
                "SteamInternal_FindOrCreateUserInterface {text} -> {interface:p}"
            ));
            interface
        }
        Err(_) => std::ptr::null_mut(),
    }
}

#[no_mangle]
pub extern "system" fn SteamInternal_FindOrCreateGameServerInterface(
    _server: i32,
    _version: *const c_char,
) -> *mut c_void {
    generic_object()
}

#[no_mangle]
pub extern "system" fn SteamInternal_GameServer_Init_V2(
    _ip: u32,
    _game_port: u16,
    _query_port: u16,
    _mode: i32,
    _version: *const c_char,
    _dll_path: *const c_char,
) -> bool {
    false
}

/// `LobbyCreated_t` : identifiant de callback Steamworks.
const LOBBY_CREATED_CALLBACK: i32 = 513;

/// `LobbyEnter_t` : réponse à `JoinLobby`, livrée au prochain RunCallbacks.
const LOBBY_ENTER_CALLBACK: i32 = 504;

/// Adresse de la partie à rejoindre, posée au lancement par le daemon
/// (`LAPINUX_JOIN_CONNECT=ip:port`) quand une invitation a été acceptée.
fn join_connect() -> Option<String> {
    std::env::var("LAPINUX_JOIN_CONNECT")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Lobby rejoint et adresse de son serveur : servis aux accesseurs que le jeu
/// interroge après `LobbyEnter_t`.
static JOINED: Mutex<Option<(u64, std::ffi::CString)>> = Mutex::new(None);

/// 12 = `GetLobbyByIndex` : rend un `CSteamID` par pointeur caché.
extern "system" fn matchmaking_lobby_by_index(
    _this: *mut c_void,
    out: *mut u64,
    _index: i32,
) -> u64 {
    trace_value(IFACE_MATCHMAKING, 12, 0);
    steam_id_result(out, 0)
}

extern "system" fn matchmaking_join_lobby(_this: *mut c_void, lobby: u64) -> u64 {
    let lobby = steam_id_value(lobby);
    static CALLS: AtomicU64 = AtomicU64::new(0);
    // Sans adresse, le lobby existe mais rien ne permet de rejoindre la
    // partie : le jeu reçoit un refus explicite au lieu d'un voyage à vide.
    let (response, connect) = match join_connect() {
        Some(address) => {
            debug_log(&format!("JoinLobby {lobby} -> {address}"));
            (1u64, std::ffi::CString::new(address).ok())
        }
        None => {
            debug_log(&format!("JoinLobby {lobby} -> aucune adresse"));
            (2u64, None)
        }
    };
    if let Ok(mut slot) = JOINED.lock() {
        *slot = connect.map(|text| (lobby, text));
    }
    // `LobbyEnter_t` : lobby (8), permissions (4), bloqué (1) et remplissage,
    // puis la réponse d'entrée (4) — trois mots de huit octets.
    results()
        .lock()
        .unwrap()
        .push((LOBBY_ENTER_CALLBACK, [lobby, 1, response]));
    CALLS.fetch_add(1, Ordering::Relaxed) + 1
}

/// Emplacement de `CreateLobby` dans la table virtuelle
/// `ISteamMatchmaking009` : `GetLobbyByIndex` occupe l'indice 12.
const MATCHMAKING_CREATE_LOBBY: usize = 13;

fn lobby_timeout_ms() -> u64 {
    std::env::var("LAPINUX_LOBBY_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(5_000)
}

/// Extrait un nombre après `"clé":` dans une réponse JSON plate.
fn json_number(text: &str, key: &str) -> Option<u64> {
    let pattern = format!("\"{key}\":");
    let rest = &text[text.find(&pattern)? + pattern.len()..];
    let digits: String = rest
        .chars()
        .skip_while(|c| c.is_whitespace())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

/// Demande un lobby au daemon par la file partagée, et attend sa réponse.
fn request_lobby(appid: u32, max_members: i32, lobby_type: i32) -> Result<u64, i32> {
    let dir = spool_dir().ok_or(2)?;
    if std::fs::create_dir_all(&dir).is_err() {
        return Err(2);
    }
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let name = format!(
        "lobby-{}-{}.json",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    );
    let request = dir.join(&name);
    let reply = dir.join(format!("{name}.reply"));
    let body = format!(
        "{{\"lobby_request\":true,\"appid\":{},\"max_members\":{},\"lobby_type\":{}}}\n",
        appid, max_members, lobby_type
    );
    if std::fs::write(&request, body).is_err() {
        return Err(2);
    }
    let deadline = std::time::Instant::now() + Duration::from_millis(lobby_timeout_ms());
    while std::time::Instant::now() < deadline {
        if let Ok(text) = std::fs::read_to_string(&reply) {
            let _ = std::fs::remove_file(&reply);
            let _ = std::fs::remove_file(&request);
            if text.contains("\"ok\":true") {
                if let Some(lobby) = json_number(&text, "lobby") {
                    return Ok(lobby);
                }
            }
            return Err(json_number(&text, "eresult").unwrap_or(2) as i32);
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    let _ = std::fs::remove_file(&request);
    Err(3)
}

extern "system" fn matchmaking_create_lobby(
    _this: *mut c_void,
    lobby_type: i32,
    max_members: i32,
) -> u64 {
    static CALLS: AtomicU64 = AtomicU64::new(0);
    debug_log(&format!(
        "SteamMatchmaking::CreateLobby type={lobby_type} max={max_members}"
    ));
    let call = CALLS.fetch_add(1, Ordering::Relaxed) + 1;
    let payload = match request_lobby(appid(), max_members, lobby_type) {
        Ok(lobby) => {
            debug_log(&format!("CreateLobby -> lobby {lobby}"));
            [1u64, lobby, 0]
        }
        Err(eresult) => {
            debug_log(&format!("CreateLobby -> echec eresult {eresult}"));
            [u64::from(eresult.unsigned_abs()), 0, 0]
        }
    };
    // Le résultat part au prochain `SteamAPI_RunCallbacks`, comme pour tout
    // appel asynchrone Steamworks.
    results()
        .lock()
        .unwrap()
        .push((LOBBY_CREATED_CALLBACK, payload));
    call
}

extern "system" fn matchmaking_invite_user(_this: *mut c_void, _lobby: u64, _invited: u64) -> bool {
    // L'invitation réelle part par la session partagée du daemon ; refuser ici
    // ferait croire au jeu que l'invitation est impossible.
    true
}

extern "system" fn matchmaking_num_members(_this: *mut c_void, _lobby: u64) -> i32 {
    1
}

extern "system" fn matchmaking_member_by_index(
    _this: *mut c_void,
    out: *mut u64,
    _lobby: u64,
    _index: i32,
) -> u64 {
    let id = local_steam_id();
    trace_value(IFACE_MATCHMAKING, 18, id);
    steam_id_result(out, id)
}

extern "system" fn matchmaking_lobby_data(
    _this: *mut c_void,
    lobby: u64,
    key: *const c_char,
) -> *const c_char {
    trace_value(IFACE_MATCHMAKING, 19, 0);
    let lobby = steam_id_value(lobby);
    let key = if key.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(key) }
            .to_string_lossy()
            .to_ascii_lowercase()
    };
    // Les jeux nomment l'adresse différemment ; toutes ces clés la désignent.
    let wants_connect = matches!(
        key.as_str(),
        "connect" | "connectstring" | "server" | "gameserver" | "listen"
    );
    if !wants_connect {
        return empty_c_string();
    }
    JOINED
        .lock()
        .ok()
        .and_then(|slot| {
            slot.as_ref()
                .filter(|entry| entry.0 == lobby)
                .map(|entry| entry.1.as_ptr())
        })
        .unwrap_or_else(empty_c_string)
}

extern "system" fn matchmaking_lobby_game_server(
    _this: *mut c_void,
    lobby: u64,
    ip: *mut u32,
    port: *mut u16,
    server: *mut u64,
) -> bool {
    trace_value(IFACE_MATCHMAKING, 30, 0);
    let lobby = steam_id_value(lobby);
    let Ok(joined) = JOINED.lock() else {
        return false;
    };
    let Some(entry) = joined.as_ref().filter(|entry| entry.0 == lobby) else {
        return false;
    };
    let address = &entry.1;
    let Some((host, port_text)) = address.to_str().ok().and_then(|text| text.rsplit_once(':'))
    else {
        return false;
    };
    let (Ok(addr), Ok(port_value)) = (host.parse::<std::net::Ipv4Addr>(), port_text.parse::<u16>())
    else {
        return false;
    };
    if !ip.is_null() {
        unsafe { *ip = u32::from_be_bytes(addr.octets()) };
    }
    if !port.is_null() {
        unsafe { *port = port_value };
    }
    if !server.is_null() {
        unsafe { *server = 0 };
    }
    true
}

extern "system" fn matchmaking_set_lobby_data(
    _this: *mut c_void,
    _lobby: u64,
    _key: *const c_char,
    _value: *const c_char,
) -> bool {
    true
}

extern "system" fn matchmaking_lobby_data_count(_this: *mut c_void, _lobby: u64) -> i32 {
    0
}

extern "system" fn matchmaking_member_data(
    _this: *mut c_void,
    _lobby: u64,
    _member: u64,
    _key: *const c_char,
) -> *const c_char {
    empty_c_string()
}

extern "system" fn matchmaking_lobby_owner(_this: *mut c_void, out: *mut u64, _lobby: u64) -> u64 {
    let owner = local_steam_id();
    trace_value(IFACE_MATCHMAKING, 35, owner);
    steam_id_result(out, owner)
}

/// 32 = `GetLobbyMemberLimit`. La table précédente y plaçait `GetLobbyOwner` :
/// le jeu lisait un `CSteamID` (notre identifiant factice) comme un entier.
extern "system" fn matchmaking_member_limit(_this: *mut c_void, _lobby: u64) -> i32 {
    4
}

extern "system" fn matchmaking_request_lobby_data(_this: *mut c_void, _lobby: u64) -> bool {
    true
}

extern "system" fn matchmaking_set_joinable(
    _this: *mut c_void,
    _lobby: u64,
    _joinable: bool,
) -> bool {
    trace_value(IFACE_MATCHMAKING, 34, 0);
    true
}

extern "system" fn matchmaking_send_chat(
    _this: *mut c_void,
    _lobby: u64,
    _data: *const c_void,
    _size: u32,
) -> bool {
    true
}

fn matchmaking_table() -> *const usize {
    static TABLE: OnceLock<Box<[usize; SLOTS]>> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut slots = stubs_matchmaking();
        // Indices d'`ISteamMatchMaking009`. `SetLobbyJoinable` et
        // `GetLobbyOwner` sont en 34 et 35 ; l'ancienne table les plaçait en
        // 31 et 32, où le jeu attend `SetLobbyMemberLimit` et
        // `GetLobbyMemberLimit`.
        slots[12] = matchmaking_lobby_by_index as usize;
        slots[MATCHMAKING_CREATE_LOBBY] = matchmaking_create_lobby as usize;
        slots[14] = matchmaking_join_lobby as usize;
        slots[16] = matchmaking_invite_user as usize;
        slots[17] = matchmaking_num_members as usize;
        slots[18] = matchmaking_member_by_index as usize;
        slots[19] = matchmaking_lobby_data as usize;
        slots[20] = matchmaking_set_lobby_data as usize;
        slots[21] = matchmaking_lobby_data_count as usize;
        slots[24] = matchmaking_member_data as usize;
        slots[26] = matchmaking_send_chat as usize;
        slots[28] = matchmaking_request_lobby_data as usize;
        slots[30] = matchmaking_lobby_game_server as usize;
        slots[32] = matchmaking_member_limit as usize;
        slots[34] = matchmaking_set_joinable as usize;
        slots[35] = matchmaking_lobby_owner as usize;
        Box::new(slots)
    });
    table.as_ptr()
}

fn generic_table() -> *const usize {
    static TABLE: OnceLock<Box<[usize; SLOTS]>> = OnceLock::new();
    TABLE.get_or_init(|| Box::new(stubs_generic())).as_ptr()
}

extern "system" fn apps_subscribed(_this: *mut c_void) -> bool {
    trace_value(IFACE_APPS, 0, 0);
    true
}

/// 6 = `BIsSubscribedApp`. `HasValidGameLicense` (Townfall) l'appelle avec son
/// propre appid : sans cette reponse le jeu conclut a l'absence de licence.
/// Seul l'appid lance est possede : les DLC gardent leur propre reponse.
extern "system" fn apps_subscribed_app(_this: *mut c_void, app_id: u32) -> bool {
    let owned = app_id == appid();
    trace_value(IFACE_APPS, 6, owned as u64);
    owned
}

/// Langue du jeu, telle que Steam la nomme (`french`, `schinese`…).
///
/// Un jeu demande sa langue à Steam (`GetCurrentGameLanguage`), pas à `LANG` :
/// c'est donc le pont qui doit répondre la langue choisie dans Lapinux, sinon
/// le jeu reste en anglais quel que soit le réglage. Le daemon la pose dans
/// l'environnement du jeu (`LAPINUX_STEAM_LANGUAGE`) ; à défaut la locale POSIX
/// est traduite ; à défaut l'anglais.
fn game_language() -> *const c_char {
    static NAME: OnceLock<CString> = OnceLock::new();
    NAME.get_or_init(|| {
        let code = std::env::var("LAPINUX_STEAM_LANGUAGE")
            .ok()
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty())
            .or_else(|| {
                let locale = std::env::var("LANGUAGE")
                    .or_else(|_| std::env::var("LANG"))
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                language_from_locale(&locale).map(str::to_string)
            })
            .unwrap_or_else(|| "english".to_string());
        CString::new(code).unwrap_or_else(|_| CString::new("english").unwrap())
    })
    .as_ptr()
}

/// Nom Steam d'une langue à partir d'une locale POSIX (`fr_FR.UTF-8`).
fn language_from_locale(locale: &str) -> Option<&'static str> {
    let tag = locale.split(['_', '-', '.', ':']).next().unwrap_or_default();
    Some(match tag {
        "en" => "english",
        "fr" => "french",
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
        "nl" => "dutch",
        "sv" => "swedish",
        "da" => "danish",
        "fi" => "finnish",
        "nb" | "no" => "norwegian",
        "cs" => "czech",
        "hu" => "hungarian",
        "ro" => "romanian",
        "el" => "greek",
        "bg" => "bulgarian",
        "th" => "thai",
        "vi" => "vietnamese",
        "id" => "indonesian",
        _ => return None,
    })
}

extern "system" fn apps_current_language(_this: *mut c_void) -> *const c_char {
    let name = game_language();
    if !name.is_null() {
        // SAFETY: `name` pointe vers la `CString` statique ci-dessus.
        let text = unsafe { CStr::from_ptr(name) }.to_string_lossy().into_owned();
        debug_once(
            "current_language",
            &format!("SteamApps::GetCurrentGameLanguage -> {text}"),
        );
    }
    name
}

extern "system" fn apps_available_languages(_this: *mut c_void) -> *const c_char {
    b"english,french\0".as_ptr().cast()
}

/// 18 = `GetAppInstallDir` : remplit un tampon. Rendre 0 sans l'écrire
/// laisserait le jeu lire les octets déjà présents sur sa pile.
extern "system" fn apps_install_dir(
    _this: *mut c_void,
    _appid: u32,
    folder: *mut c_char,
    size: u32,
) -> u32 {
    if !folder.is_null() && size > 0 {
        unsafe { *folder = 0 };
    }
    0
}

extern "system" fn apps_owner(_this: *mut c_void, out: *mut u64) -> u64 {
    let owner = local_steam_id();
    trace_value(IFACE_APPS, 20, owner);
    steam_id_result(out, owner)
}

/// 21 = `GetLaunchQueryParam` : rend une chaîne, jamais un pointeur nul.
extern "system" fn apps_launch_query_param(
    _this: *mut c_void,
    _key: *const c_char,
) -> *const c_char {
    empty_c_string()
}

fn apps_table() -> *const usize {
    static TABLE: OnceLock<Box<[usize; SLOTS]>> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut slots = stubs_apps();
        slots[0] = apps_subscribed as usize;
        slots[6] = apps_subscribed_app as usize;
        slots[4] = apps_current_language as usize;
        slots[5] = apps_available_languages as usize;
        slots[18] = apps_install_dir as usize;
        slots[20] = apps_owner as usize;
        slots[21] = apps_launch_query_param as usize;
        Box::new(slots)
    });
    table.as_ptr()
}

/// Contenu d'atelier tel que le jeu le voit : l'identifiant publié, son poids
/// et le dossier Windows qui le contient. Le chemin est déjà converti par le
/// daemon (`Z:\...`), car le jeu tourne sous Wine.
#[derive(Debug)]
struct UgcItem {
    id: u64,
    size: u64,
    folder: CString,
}

/// Lit le manifeste écrit par le daemon avant le lancement. Une ligne est
/// `identifiant<TAB>poids<TAB>chemin` ; les chemins Windows peuvent contenir
/// des espaces, jamais de tabulation.
fn parse_workshop_manifest(text: &str) -> Vec<UgcItem> {
    let mut items: Vec<UgcItem> = Vec::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let mut fields = line.splitn(3, '\t');
        let (Some(id), Some(size), Some(folder)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let (Ok(id), Ok(size)) = (id.trim().parse::<u64>(), size.trim().parse::<u64>()) else {
            continue;
        };
        if id == 0 || items.iter().any(|item| item.id == id) {
            continue;
        }
        let Ok(folder) = CString::new(folder) else {
            continue;
        };
        if folder.as_bytes().is_empty() {
            continue;
        }
        items.push(UgcItem { id, size, folder });
    }
    items
}

fn ugc_items() -> &'static [UgcItem] {
    static ITEMS: OnceLock<Vec<UgcItem>> = OnceLock::new();
    ITEMS.get_or_init(|| {
        let Some(path) = std::env::var_os("LAPINUX_WORKSHOP_MANIFEST").map(PathBuf::from) else {
            debug_once("ugc_manifest_absent", "atelier : aucun manifeste fourni");
            return Vec::new();
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            debug_log(&format!("atelier : manifeste illisible {}", path.display()));
            return Vec::new();
        };
        let items = parse_workshop_manifest(&text);
        debug_log(&format!(
            "atelier : {} contenu(s) depuis {}",
            items.len(),
            path.display()
        ));
        items
    })
}

fn ugc_find(id: u64) -> Option<&'static UgcItem> {
    ugc_items().iter().find(|item| item.id == id)
}

extern "system" fn ugc_num_subscribed(_this: *mut c_void, _include_locally_disabled: bool) -> u32 {
    let count = ugc_items().len().min(u32::MAX as usize) as u32;
    trace_value(IFACE_UGC, 74, count as u64);
    count
}

extern "system" fn ugc_subscribed_items(
    _this: *mut c_void,
    ids: *mut u64,
    max_entries: u32,
    _include_locally_disabled: bool,
) -> u32 {
    trace_value(IFACE_UGC, 75, 0);
    if ids.is_null() || max_entries == 0 {
        return 0;
    }
    let count = ugc_items()
        .iter()
        .take(max_entries as usize)
        .enumerate()
        .map(|(index, item)| {
            unsafe { *ids.add(index) = item.id };
            index
        })
        .count();
    count.min(u32::MAX as usize) as u32
}

extern "system" fn ugc_item_state(_this: *mut c_void, id: u64) -> u32 {
    // k_EItemStateSubscribed (1) | k_EItemStateInstalled (4).
    let state = if ugc_find(id).is_some() { 5 } else { 0 };
    trace_value(IFACE_UGC, 76, state as u64);
    state
}

extern "system" fn ugc_install_info(
    _this: *mut c_void,
    id: u64,
    size_on_disk: *mut u64,
    folder: *mut c_char,
    folder_size: u32,
    timestamp: *mut u32,
) -> bool {
    trace_value(IFACE_UGC, 77, 0);
    let Some(item) = ugc_find(id) else {
        return false;
    };
    if !size_on_disk.is_null() {
        unsafe { *size_on_disk = item.size };
    }
    if !timestamp.is_null() {
        unsafe { *timestamp = 0 };
    }
    if folder.is_null() || folder_size == 0 {
        return false;
    }
    let bytes = item.folder.as_bytes_with_nul();
    if bytes.len() > folder_size as usize {
        return false;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), folder.cast::<u8>(), bytes.len());
    }
    true
}

extern "system" fn ugc_download_info(
    _this: *mut c_void,
    _id: u64,
    downloaded: *mut u64,
    total: *mut u64,
) -> bool {
    trace_value(IFACE_UGC, 78, 0);
    if !downloaded.is_null() {
        unsafe { *downloaded = 0 };
    }
    if !total.is_null() {
        unsafe { *total = 0 };
    }
    false
}

extern "system" fn ugc_download_item(_this: *mut c_void, _id: u64, _high_priority: bool) -> bool {
    false
}

extern "system" fn ugc_binit_workshop(
    _this: *mut c_void,
    _depot: u32,
    _folder: *const c_char,
) -> bool {
    false
}

extern "system" fn ugc_suspend_downloads(_this: *mut c_void, _suspend: bool) {}

extern "system" fn ugc_item_update_progress(
    _this: *mut c_void,
    _handle: u64,
    processed: *mut u64,
    total: *mut u64,
) -> i32 {
    if !processed.is_null() {
        unsafe { *processed = 0 };
    }
    if !total.is_null() {
        unsafe { *total = 0 };
    }
    0
}

extern "system" fn ugc_set_items_disabled(
    _this: *mut c_void,
    _ids: *mut u64,
    _count: u32,
    _disabled: bool,
) -> bool {
    false
}

extern "system" fn ugc_set_load_order(_this: *mut c_void, _ids: *mut u64, _count: u32) -> bool {
    false
}

fn remote_table() -> *const usize {
    static TABLE: OnceLock<Box<[usize; SLOTS]>> = OnceLock::new();
    TABLE.get_or_init(|| Box::new(stubs_remote())).as_ptr()
}

fn ugc_table() -> *const usize {
    static TABLE: OnceLock<Box<[usize; SLOTS]>> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut slots = stubs_ugc();
        // Indices de `ISteamUGC021` : les requêtes (0 à 73) restent des
        // stubs neutres, puis l'atelier installé répond aux méthodes de
        // consommation. Un décalage ici ferait appeler le stub voisin avec
        // les mauvais arguments, donc ces indices sont testés plus bas.
        slots[67] = ugc_item_update_progress as usize;
        slots[74] = ugc_num_subscribed as usize;
        slots[75] = ugc_subscribed_items as usize;
        slots[76] = ugc_item_state as usize;
        slots[77] = ugc_install_info as usize;
        slots[78] = ugc_download_info as usize;
        slots[79] = ugc_download_item as usize;
        slots[80] = ugc_binit_workshop as usize;
        slots[81] = ugc_suspend_downloads as usize;
        slots[94] = ugc_set_items_disabled as usize;
        slots[95] = ugc_set_load_order as usize;
        Box::new(slots)
    });
    table.as_ptr()
}

fn input_table() -> *const usize {
    static TABLE: OnceLock<Box<[usize; SLOTS]>> = OnceLock::new();
    TABLE.get_or_init(|| Box::new(stubs_input())).as_ptr()
}

fn net_table() -> *const usize {
    static TABLE: OnceLock<Box<[usize; SLOTS]>> = OnceLock::new();
    TABLE.get_or_init(|| Box::new(stubs_net())).as_ptr()
}

fn servers_table() -> *const usize {
    static TABLE: OnceLock<Box<[usize; SLOTS]>> = OnceLock::new();
    TABLE.get_or_init(|| Box::new(stubs_servers())).as_ptr()
}

extern "system" fn user_hsteam_user() -> i32 {
    1
}

extern "system" fn user_logged_on() -> bool {
    debug_once("logged_on", "SteamUser::BLoggedOn -> true");
    true
}

/// Identifiant local rendu au jeu. Le vrai compte reste celui du daemon ;
/// cette valeur sert seulement aux vérifications « suis-je connecté ? ».
/// `LAPINUX_STEAM_ID` permet au daemon de transmettre le vrai identifiant.
/// Identifiant local du compte. Tout appel qui répond « qui suis-je »
/// (membre de lobby, propriétaire) doit rendre la même valeur.
fn local_steam_id() -> u64 {
    std::env::var("LAPINUX_STEAM_ID")
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .filter(|id| *id != 0)
        .unwrap_or(76561198000000000)
}

/// Bornes de la pile du thread courant, lues dans le TEB (`gs:[0x10]` pour la
/// limite basse, `gs:[0x08]` pour la base). MSVC dépose le `CSteamID` rendu
/// par une méthode dans la pile de l'appelant : ces bornes distinguent ce
/// pointeur caché d'un argument ordinaire reçu dans RDX.
#[cfg(all(windows, target_arch = "x86_64"))]
fn stack_bounds() -> (usize, usize) {
    let (low, high): (usize, usize);
    unsafe {
        core::arch::asm!(
            "mov {}, gs:[0x10]",
            out(reg) low,
            options(nomem, nostack, preserves_flags)
        );
        core::arch::asm!(
            "mov {}, gs:[0x08]",
            out(reg) high,
            options(nomem, nostack, preserves_flags)
        );
    }
    (low, high)
}

/// Rend un `CSteamID` comme le fait MSVC pour une classe non triviale : le
/// jeu passe l'adresse de destination dans RDX et déréférence le pointeur
/// rendu en RAX. Sans pointeur de pile plausible (appelant qui attendait la
/// valeur), rendre l'identifiant lui-même.
fn steam_id_result(out: *mut u64, id: u64) -> u64 {
    #[cfg(all(windows, target_arch = "x86_64"))]
    {
        let candidate = out as usize;
        let (low, high) = stack_bounds();
        if candidate != 0
            && candidate % 8 == 0
            && candidate >= low
            && candidate < high
            && candidate + 8 <= high
        {
            unsafe { *out = id };
            debug_once(
                "steam_id_sret",
                "CSteamID : retour par pointeur caché (RDX -> RAX)",
            );
            return candidate as u64;
        }
    }
    #[cfg(not(all(windows, target_arch = "x86_64")))]
    let _ = out;
    id
}

/// `CSteamID` n'est pas trivial pour le SDK : MSVC passe aussi les paramètres
/// de ce type par adresse. Un identifiant Steam porte son univers dans l'octet
/// de poids fort (1 pour l'univers public), jamais un pointeur d'espace
/// utilisateur (octet nul) : c'est le discriminant entre la valeur et sa
/// référence.
fn steam_id_value(raw: u64) -> u64 {
    let address = raw as usize;
    if raw >> 56 == 0 && address >= 0x1_0000 && address % 8 == 0 {
        return unsafe { (address as *const u64).read_unaligned() };
    }
    raw
}

/// 24 = `GetFriendFromSourceByIndex`. Ces méthodes rendent un `CSteamID` mais
/// n'ont rien à dire : répondre zéro par valeur ferait déréférencer l'adresse
/// nulle au jeu, d'où le passage par l'ABI à pointeur caché.
extern "system" fn friends_source_by_index(
    _this: *mut c_void,
    out: *mut u64,
    _source: u64,
    _index: i32,
) -> u64 {
    trace_value(IFACE_FRIENDS, 24, 0);
    steam_id_result(out, 0)
}

/// 38 = `GetClanOwner`.
extern "system" fn friends_clan_owner(_this: *mut c_void, out: *mut u64, _clan: u64) -> u64 {
    trace_value(IFACE_FRIENDS, 38, 0);
    steam_id_result(out, 0)
}

/// 40 = `GetClanOfficerByIndex`.
extern "system" fn friends_clan_officer_by_index(
    _this: *mut c_void,
    out: *mut u64,
    _clan: u64,
    _index: i32,
) -> u64 {
    trace_value(IFACE_FRIENDS, 40, 0);
    steam_id_result(out, 0)
}

/// 55 = `GetChatMemberByIndex`.
extern "system" fn friends_chat_member_by_index(
    _this: *mut c_void,
    out: *mut u64,
    _clan: u64,
    _index: i32,
) -> u64 {
    trace_value(IFACE_FRIENDS, 55, 0);
    steam_id_result(out, 0)
}

extern "system" fn user_steam_id(_this: *mut c_void, out: *mut u64) -> u64 {
    debug_once("steam_id", "SteamUser::GetSteamID");
    let id = local_steam_id();
    trace_value(IFACE_USER, 2, id);
    steam_id_result(out, id)
}

/// 6 = `GetUserDataFolder` (sans appid : la signature d'origine n'en a pas).
/// Il remplit un tampon ; rendre `false` sans le terminer laisserait le jeu
/// lire une chaîne non initialisée.
extern "system" fn user_data_folder(_this: *mut c_void, buffer: *mut c_char, size: i32) -> bool {
    if !buffer.is_null() && size > 0 {
        unsafe { *buffer = 0 };
    }
    false
}

extern "system" fn user_noop(_this: *mut c_void) {}

/// 9 = `GetAvailableVoice` : sans micro, rend « pas d'enregistrement » et
/// remplit les tailles à zéro plutôt que de les laisser indéfinies.
extern "system" fn user_available_voice(
    _this: *mut c_void,
    compressed: *mut u32,
    uncompressed: *mut u32,
    _rate: u32,
) -> i32 {
    if !compressed.is_null() {
        unsafe { *compressed = 0 };
    }
    if !uncompressed.is_null() {
        unsafe { *uncompressed = 0 };
    }
    1
}

extern "system" fn user_get_voice(
    _this: *mut c_void,
    _compressed: bool,
    _buffer: *mut c_void,
    _size: u32,
    written: *mut u32,
) -> i32 {
    if !written.is_null() {
        unsafe { *written = 0 };
    }
    1
}

extern "system" fn user_decompress_voice(
    _this: *mut c_void,
    _compressed: *const c_void,
    _compressed_size: u32,
    _destination: *mut c_void,
    _destination_size: u32,
    written: *mut u32,
    _rate: u32,
) -> i32 {
    if !written.is_null() {
        unsafe { *written = 0 };
    }
    1
}

extern "system" fn user_voice_rate(_this: *mut c_void) -> u32 {
    48_000
}

extern "system" fn user_get_auth_session_ticket(
    _this: *mut c_void,
    _ticket: *mut c_void,
    _max_ticket: i32,
    written: *mut u32,
    _identity: *const c_void,
) -> u32 {
    // Aucun jeton sans client Steam : la taille écrite doit être nulle, sinon
    // le jeu lit un tampon jamais rempli.
    if !written.is_null() {
        unsafe { *written = 0 };
    }
    debug_once("auth_session", "SteamUser::GetAuthSessionTicket -> 0");
    0
}

extern "system" fn user_get_auth_ticket_for_webapi(
    _this: *mut c_void,
    _identity: *const c_char,
) -> u32 {
    debug_once("auth_webapi", "SteamUser::GetAuthTicketForWebApi -> 0");
    0
}

extern "system" fn user_begin_auth_session(
    _this: *mut c_void,
    _ticket: *const c_void,
    _size: i32,
    _member: u64,
) -> i32 {
    // 1 = k_EBeginAuthSessionResultOK. Sans serveur d'authentification,
    // refuser ferait tourner le jeu en boucle sur une session invalide.
    1
}

extern "system" fn user_end_auth_session(_this: *mut c_void, _member: u64) {}

extern "system" fn user_cancel_auth_ticket(_this: *mut c_void, _ticket: u32) {}

extern "system" fn user_has_license(_this: *mut c_void, _member: u64, _appid: u32) -> i32 {
    // 0 = k_EUserHasLicenseResultHasLicense.
    0
}

extern "system" fn user_behind_nat(_this: *mut c_void) -> bool {
    false
}

extern "system" fn user_advertise_game(_this: *mut c_void, _server: u64, _ip: u32, _port: u16) {}

extern "system" fn user_request_encrypted_ticket(
    _this: *mut c_void,
    _data: *const c_void,
    _size: i32,
) -> u64 {
    0
}

extern "system" fn user_get_encrypted_ticket(
    _this: *mut c_void,
    _buffer: *mut c_void,
    _max: i32,
    written: *mut u32,
) -> bool {
    if !written.is_null() {
        unsafe { *written = 0 };
    }
    false
}

extern "system" fn user_badge_level(_this: *mut c_void, _series: i32, _foil: bool) -> i32 {
    0
}

extern "system" fn user_level(_this: *mut c_void) -> i32 {
    0
}

extern "system" fn user_store_auth_url(_this: *mut c_void, _url: *const c_char) -> u64 {
    0
}

extern "system" fn user_false(_this: *mut c_void) -> bool {
    false
}

/// Nom affiché du compte, transmis par le daemon dans
/// `LAPINUX_STEAM_PERSONA`. Sans valeur — jeu lancé hors de Lapinux — le pont
/// garde « Joueur » plutôt qu'une chaîne vide.
fn persona_name() -> &'static CStr {
    static NAME: OnceLock<CString> = OnceLock::new();
    NAME.get_or_init(|| {
        let name = std::env::var("LAPINUX_STEAM_PERSONA")
            .ok()
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "Joueur".to_string());
        debug_log(&format!("pont : nom du compte « {name} »"));
        CString::new(name).unwrap_or_else(|_| CString::new("Joueur").expect("nom de repli"))
    })
}

extern "system" fn friends_persona_name() -> *const c_char {
    debug_once("persona_name", "SteamFriends::GetPersonaName");
    persona_name().as_ptr()
}

extern "system" fn friends_persona_state() -> i32 {
    1
}

extern "system" fn utils_connected_universe() -> i32 {
    1
}

extern "system" fn utils_server_time() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as u32)
        .unwrap_or(0)
}

extern "system" fn utils_app_id() -> u32 {
    let id = appid();
    debug_once("app_id", "SteamUtils::GetAppID");
    trace_value(IFACE_UTILS, 9, id as u64);
    id
}

extern "system" fn utils_ui_language() -> *const c_char {
    game_language()
}

/// 4 = `GetIPCountry` : rend une chaîne. Un stub à zéro ferait construire un
/// `FString` sur un pointeur nul.
extern "system" fn utils_ip_country() -> *const c_char {
    b"FR\0".as_ptr().cast()
}

// API plate d'`ISteamUtils` : Steamworks.NET résout ces symboles par leur nom
// au lieu d'appeler la vtable. `GetAppID` est appelé par
// `FurySDK.SteamPlatformSDK.Init` juste après `SteamAPI.Init`.
#[no_mangle]
pub extern "system" fn SteamAPI_ISteamUtils_GetAppID(_this: *mut c_void) -> u32 {
    utils_app_id()
}

#[no_mangle]
pub extern "system" fn SteamAPI_ISteamUtils_GetConnectedUniverse(_this: *mut c_void) -> i32 {
    utils_connected_universe()
}

#[no_mangle]
pub extern "system" fn SteamAPI_ISteamUtils_GetServerRealTime(_this: *mut c_void) -> u32 {
    utils_server_time()
}

#[no_mangle]
pub extern "system" fn SteamAPI_ISteamUtils_GetSteamUILanguage(
    _this: *mut c_void,
) -> *const c_char {
    utils_ui_language()
}

#[no_mangle]
pub extern "system" fn SteamAPI_ISteamUtils_GetIPCountry(_this: *mut c_void) -> *const c_char {
    utils_ip_country()
}

#[no_mangle]
pub extern "system" fn SteamAPI_ISteamUtils_IsSteamRunningOnSteamDeck(_this: *mut c_void) -> bool {
    false
}

#[no_mangle]
pub extern "system" fn SteamAPI_ISteamUtils_IsSteamInBigPictureMode(_this: *mut c_void) -> bool {
    false
}

#[no_mangle]
pub extern "system" fn SteamAPI_ISteamUtils_IsOverlayEnabled(_this: *mut c_void) -> bool {
    false
}

#[no_mangle]
pub extern "system" fn SteamAPI_ISteamUtils_GetOverlayNotificationPosition(
    _this: *mut c_void,
) -> i32 {
    0
}

#[no_mangle]
pub extern "system" fn SteamAPI_ISteamUtils_GetSecondsSinceAppActive(_this: *mut c_void) -> u32 {
    0
}

#[no_mangle]
pub extern "system" fn SteamAPI_ISteamUtils_GetSecondsSinceComputerActive(
    _this: *mut c_void,
) -> u32 {
    0
}

#[no_mangle]
pub extern "system" fn SteamAPI_ISteamUtils_GetCurrentBatteryPower(_this: *mut c_void) -> u8 {
    255
}

#[no_mangle]
pub extern "system" fn SteamAPI_ISteamUtils_IsAPICallCompleted(
    _this: *mut c_void,
    _call: u64,
    _failed: *mut bool,
) -> bool {
    false
}

#[no_mangle]
pub extern "system" fn SteamAPI_ISteamUtils_GetAPICallFailureReason(
    _this: *mut c_void,
    _call: u64,
) -> i32 {
    -1
}

#[no_mangle]
pub extern "system" fn SteamAPI_ISteamUtils_GetAPICallResult(
    _this: *mut c_void,
    _call: u64,
    _callback: *mut c_void,
    _size: i32,
    _expected: i32,
    _failed: *mut bool,
) -> bool {
    false
}

#[no_mangle]
pub extern "system" fn SteamAPI_ISteamApps_BIsSubscribedApp(
    _this: *mut c_void,
    _appid: u32,
) -> bool {
    true
}

#[no_mangle]
pub extern "system" fn SteamAPI_ISteamApps_BIsDlcInstalled(
    _this: *mut c_void,
    _appid: u32,
) -> bool {
    false
}

#[no_mangle]
pub extern "system" fn SteamAPI_ISteamApps_GetAppBuildId(_this: *mut c_void) -> i32 {
    0
}

#[no_mangle]
pub extern "system" fn SteamAPI_ISteamApps_GetAvailableGameLanguages(
    this: *mut c_void,
) -> *const c_char {
    apps_available_languages(this)
}

#[no_mangle]
pub extern "system" fn SteamAPI_ISteamUser_GetSteamID(this: *mut c_void, out: *mut u64) -> u64 {
    user_steam_id(this, out)
}

#[no_mangle]
pub extern "system" fn SteamAPI_ISteamUser_BLoggedOn(_this: *mut c_void) -> bool {
    user_logged_on()
}

#[no_mangle]
pub extern "system" fn SteamAPI_ISteamApps_BIsSubscribed(this: *mut c_void) -> bool {
    apps_subscribed(this)
}

#[no_mangle]
pub extern "system" fn SteamAPI_ISteamApps_GetCurrentGameLanguage(
    this: *mut c_void,
) -> *const c_char {
    apps_current_language(this)
}

fn user_table() -> *const usize {
    static TABLE: OnceLock<Box<[usize; SLOTS]>> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut slots = stubs_user();
        // Indices d'`ISteamUser023`, la version demandée par le jeu. La
        // table précédente visait un SDK plus ancien : elle plaçait les
        // jetons d'authentification en 3, donc `GetUserDataFolder` tombait
        // sur un stub qui ne remplissait pas son tampon.
        slots[0] = user_hsteam_user as usize;
        slots[1] = user_logged_on as usize;
        slots[2] = user_steam_id as usize;
        slots[5] = user_noop as usize;
        slots[6] = user_data_folder as usize;
        slots[7] = user_noop as usize;
        slots[8] = user_noop as usize;
        slots[9] = user_available_voice as usize;
        slots[10] = user_get_voice as usize;
        slots[11] = user_decompress_voice as usize;
        slots[12] = user_voice_rate as usize;
        slots[13] = user_get_auth_session_ticket as usize;
        slots[14] = user_get_auth_ticket_for_webapi as usize;
        slots[15] = user_begin_auth_session as usize;
        slots[16] = user_end_auth_session as usize;
        slots[17] = user_cancel_auth_ticket as usize;
        slots[18] = user_has_license as usize;
        slots[19] = user_behind_nat as usize;
        slots[20] = user_advertise_game as usize;
        slots[21] = user_request_encrypted_ticket as usize;
        slots[22] = user_get_encrypted_ticket as usize;
        slots[23] = user_badge_level as usize;
        slots[24] = user_level as usize;
        slots[25] = user_store_auth_url as usize;
        slots[26] = user_false as usize;
        slots[27] = user_false as usize;
        slots[28] = user_false as usize;
        slots[29] = user_false as usize;
        Box::new(slots)
    });
    table.as_ptr()
}

extern "system" fn friends_count(_this: *mut c_void, _flags: i32) -> i32 {
    0
}

extern "system" fn friends_by_index(
    _this: *mut c_void,
    out: *mut u64,
    _index: i32,
    _flags: i32,
) -> u64 {
    trace_value(IFACE_FRIENDS, 3, 0);
    steam_id_result(out, 0)
}

extern "system" fn friends_relationship(_this: *mut c_void, _friend: u64) -> i32 {
    0
}

extern "system" fn friends_state(_this: *mut c_void, _friend: u64) -> i32 {
    0
}

/// 6 = `GetFriendPersonaName` : rend une chaîne, jamais un entier. L'ancienne
/// table y plaçait `GetFriendPersonaState`, donc le jeu recevait `0` comme
/// `const char*` et construisait un `FString` sur un pointeur nul.
extern "system" fn friends_name(_this: *mut c_void, friend: u64) -> *const c_char {
    trace_value(IFACE_FRIENDS, 6, 0);
    // Le jeu demande souvent son propre nom par cette voie : la même réponse
    // que `GetPersonaName` évite un « Joueur » à côté du vrai nom de compte.
    if friend == local_steam_id() {
        return persona_name().as_ptr();
    }
    empty_c_string()
}

/// API plate historique : certains jeux résolvent ces exports par leur nom
/// (`GetProcAddress`) au lieu d'appeler la vtable de l'interface.
#[no_mangle]
pub extern "system" fn SteamAPI_ISteamFriends_GetPersonaName(_this: *mut c_void) -> *const c_char {
    friends_persona_name()
}

#[no_mangle]
pub extern "system" fn SteamAPI_ISteamFriends_GetFriendPersonaName(
    this: *mut c_void,
    friend: u64,
) -> *const c_char {
    friends_name(this, friend)
}

extern "system" fn friends_game_played(
    _this: *mut c_void,
    _friend: u64,
    _info: *mut c_void,
) -> bool {
    trace_value(IFACE_FRIENDS, 7, 0);
    false
}

extern "system" fn friends_name_history(
    _this: *mut c_void,
    _friend: u64,
    _index: i32,
) -> *const c_char {
    empty_c_string()
}

extern "system" fn friends_level(_this: *mut c_void, _friend: u64) -> i32 {
    0
}

extern "system" fn friends_nickname(_this: *mut c_void, _player: u64) -> *const c_char {
    b"Joueur\0".as_ptr().cast()
}

extern "system" fn friends_group_count(_this: *mut c_void) -> i32 {
    0
}

extern "system" fn friends_group_id(_this: *mut c_void, _index: i32) -> i32 {
    -1
}

extern "system" fn friends_group_name(_this: *mut c_void, _group: i32) -> *const c_char {
    empty_c_string()
}

extern "system" fn friends_group_members(_this: *mut c_void, _group: i32) -> i32 {
    0
}

extern "system" fn friends_has(_this: *mut c_void, _friend: u64, _flags: i32) -> bool {
    false
}

extern "system" fn friends_clan_count(_this: *mut c_void) -> i32 {
    0
}

extern "system" fn friends_clan_by_index(_this: *mut c_void, out: *mut u64, _index: i32) -> u64 {
    trace_value(IFACE_FRIENDS, 18, 0);
    steam_id_result(out, 0)
}

extern "system" fn friends_clan_name(_this: *mut c_void, _clan: u64) -> *const c_char {
    empty_c_string()
}

extern "system" fn friends_clan_tag(_this: *mut c_void, _clan: u64) -> *const c_char {
    empty_c_string()
}

extern "system" fn friends_small_avatar(_this: *mut c_void, _friend: u64) -> i32 {
    0
}

extern "system" fn friends_medium_avatar(_this: *mut c_void, _friend: u64) -> i32 {
    0
}

extern "system" fn friends_large_avatar(_this: *mut c_void, _friend: u64) -> i32 {
    0
}

extern "system" fn friends_request_info(
    _this: *mut c_void,
    _friend: u64,
    _name_only: bool,
) -> bool {
    false
}

extern "system" fn friends_set_rich_presence(
    _this: *mut c_void,
    _key: *const c_char,
    _value: *const c_char,
) -> bool {
    true
}

extern "system" fn friends_clear_rich_presence(_this: *mut c_void) {}

/// 43 = `GetFriendRichPresence` : rend une chaîne, jamais un pointeur nul.
extern "system" fn friends_rich_presence(
    _this: *mut c_void,
    _friend: u64,
    _key: *const c_char,
) -> *const c_char {
    trace_value(IFACE_FRIENDS, 43, 0);
    empty_c_string()
}

extern "system" fn friends_rich_presence_key_count(_this: *mut c_void, _friend: u64) -> i32 {
    0
}

extern "system" fn friends_rich_presence_key(
    _this: *mut c_void,
    _friend: u64,
    _index: i32,
) -> *const c_char {
    empty_c_string()
}

extern "system" fn friends_invite_game(
    _this: *mut c_void,
    _friend: u64,
    _connect: *const c_char,
) -> bool {
    true
}

extern "system" fn friends_coplay_count(_this: *mut c_void) -> i32 {
    0
}

extern "system" fn friends_coplay(_this: *mut c_void, out: *mut u64, _index: i32) -> u64 {
    trace_value(IFACE_FRIENDS, 49, 0);
    steam_id_result(out, 0)
}

extern "system" fn friends_join_chat(_this: *mut c_void, _clan: u64) -> u64 {
    0
}

extern "system" fn friends_profile_item_string(
    _this: *mut c_void,
    _id: u64,
    _item: i32,
    _property: i32,
) -> *const c_char {
    empty_c_string()
}

extern "system" fn friends_follower_count(_this: *mut c_void, _id: u64) -> u64 {
    0
}

extern "system" fn friends_is_following(_this: *mut c_void, _id: u64) -> u64 {
    0
}

fn friends_table() -> *const usize {
    static TABLE: OnceLock<Box<[usize; SLOTS]>> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut slots = stubs_friends();
        // Indices d'`ISteamFriends018` (version demandée dans le journal).
        // `SetPersonaName` n'existe plus : `GetPersonaState` est en 1, donc
        // toute la table ancienne était décalée d'un cran.
        slots[0] = friends_persona_name as usize;
        slots[1] = friends_persona_state as usize;
        slots[2] = friends_count as usize;
        slots[3] = friends_by_index as usize;
        slots[4] = friends_relationship as usize;
        slots[5] = friends_state as usize;
        slots[6] = friends_name as usize;
        slots[7] = friends_game_played as usize;
        slots[8] = friends_name_history as usize;
        slots[9] = friends_level as usize;
        slots[10] = friends_nickname as usize;
        slots[11] = friends_group_count as usize;
        slots[12] = friends_group_id as usize;
        slots[13] = friends_group_name as usize;
        slots[14] = friends_group_members as usize;
        slots[16] = friends_has as usize;
        slots[17] = friends_clan_count as usize;
        slots[18] = friends_clan_by_index as usize;
        slots[19] = friends_clan_name as usize;
        slots[20] = friends_clan_tag as usize;
        slots[24] = friends_source_by_index as usize;
        slots[38] = friends_clan_owner as usize;
        slots[40] = friends_clan_officer_by_index as usize;
        slots[55] = friends_chat_member_by_index as usize;
        slots[33] = friends_small_avatar as usize;
        slots[34] = friends_medium_avatar as usize;
        slots[35] = friends_large_avatar as usize;
        slots[36] = friends_request_info as usize;
        slots[41] = friends_set_rich_presence as usize;
        slots[42] = friends_clear_rich_presence as usize;
        slots[43] = friends_rich_presence as usize;
        slots[44] = friends_rich_presence_key_count as usize;
        slots[45] = friends_rich_presence_key as usize;
        slots[47] = friends_invite_game as usize;
        slots[48] = friends_coplay_count as usize;
        slots[49] = friends_coplay as usize;
        slots[52] = friends_join_chat as usize;
        slots[65] = friends_follower_count as usize;
        slots[66] = friends_is_following as usize;
        slots[76] = friends_profile_item_string as usize;
        Box::new(slots)
    });
    table.as_ptr()
}

fn utils_table() -> *const usize {
    static TABLE: OnceLock<Box<[usize; SLOTS]>> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut slots = stubs_utils();
        // `ISteamUtils010` garde deux méthodes privées (`GetCSERIPPort` en 7
        // et `RunFrame` en 14) : `GetAppID` est donc bien en 9, et
        // `GetSteamUILanguage` en 23.
        slots[2] = utils_connected_universe as usize;
        slots[3] = utils_server_time as usize;
        slots[4] = utils_ip_country as usize;
        slots[9] = utils_app_id as usize;
        slots[23] = utils_ui_language as usize;
        Box::new(slots)
    });
    table.as_ptr()
}

/// Une interface Steamworks est un objet C++ dont le premier champ est le
/// pointeur de vtable. Renvoyer directement la table de fonctions fait lire
/// le premier pointeur de fonction comme une vtable, puis appeler un octet
/// au hasard : c'est exactement le crash `call *0x38(%rax)` observé.
#[repr(C)]
struct InterfaceObject {
    vtable: usize,
}

fn interface_object(table: *const usize, cell: &'static OnceLock<InterfaceObject>) -> *mut c_void {
    let object = cell.get_or_init(|| InterfaceObject {
        vtable: table as usize,
    });
    (object as *const InterfaceObject).cast_mut().cast()
}

static USER_OBJECT: OnceLock<InterfaceObject> = OnceLock::new();
static FRIENDS_OBJECT: OnceLock<InterfaceObject> = OnceLock::new();
static UTILS_OBJECT: OnceLock<InterfaceObject> = OnceLock::new();
static STATS_OBJECT: OnceLock<InterfaceObject> = OnceLock::new();
static MATCHMAKING_OBJECT: OnceLock<InterfaceObject> = OnceLock::new();
static APPS_OBJECT: OnceLock<InterfaceObject> = OnceLock::new();
static REMOTE_OBJECT: OnceLock<InterfaceObject> = OnceLock::new();
static UGC_OBJECT: OnceLock<InterfaceObject> = OnceLock::new();
static INPUT_OBJECT: OnceLock<InterfaceObject> = OnceLock::new();
static NET_OBJECT: OnceLock<InterfaceObject> = OnceLock::new();
static SERVERS_OBJECT: OnceLock<InterfaceObject> = OnceLock::new();
static GENERIC_OBJECT: OnceLock<InterfaceObject> = OnceLock::new();

fn user_object() -> *mut c_void {
    interface_object(user_table(), &USER_OBJECT)
}

fn friends_object() -> *mut c_void {
    interface_object(friends_table(), &FRIENDS_OBJECT)
}

fn utils_object() -> *mut c_void {
    interface_object(utils_table(), &UTILS_OBJECT)
}

fn stats_object() -> *mut c_void {
    interface_object(table(), &STATS_OBJECT)
}

fn matchmaking_object() -> *mut c_void {
    interface_object(matchmaking_table(), &MATCHMAKING_OBJECT)
}

fn apps_object() -> *mut c_void {
    interface_object(apps_table(), &APPS_OBJECT)
}

fn remote_object() -> *mut c_void {
    interface_object(remote_table(), &REMOTE_OBJECT)
}

fn ugc_object() -> *mut c_void {
    interface_object(ugc_table(), &UGC_OBJECT)
}

fn input_object() -> *mut c_void {
    interface_object(input_table(), &INPUT_OBJECT)
}

fn net_object() -> *mut c_void {
    interface_object(net_table(), &NET_OBJECT)
}

fn servers_object() -> *mut c_void {
    interface_object(servers_table(), &SERVERS_OBJECT)
}

fn generic_object() -> *mut c_void {
    interface_object(generic_table(), &GENERIC_OBJECT)
}

fn interface_for(version: &str) -> *mut c_void {
    // Les versions constantes du SDK sont parfois en majuscules
    // (`STEAMUSERSTATS_INTERFACE_VERSION013`) : ne pas les rater.
    let version = version.to_ascii_lowercase();
    if version.starts_with("steammatchmakingservers") {
        servers_object()
    } else if version.starts_with("steammatchmaking") {
        matchmaking_object()
    } else if version.starts_with("steamuserstats") {
        stats_object()
    } else if version.starts_with("steamuser") {
        user_object()
    } else if version.starts_with("steamfriends") {
        friends_object()
    } else if version.starts_with("steamutils") {
        utils_object()
    } else if version.starts_with("steamapps") {
        apps_object()
    } else if version.starts_with("steamremotestorage") {
        remote_object()
    } else if version.starts_with("steamugc") {
        ugc_object()
    } else if version.starts_with("steaminput") {
        input_object()
    } else if version.starts_with("steamnetworking") {
        net_object()
    } else {
        generic_object()
    }
}

#[no_mangle]
pub extern "system" fn SteamAPI_ISteamMatchmaking(_this: *mut c_void) -> *const usize {
    matchmaking_object() as *const usize
}

#[no_mangle]
pub extern "system" fn SteamAPI_SteamUGC_v021() -> *const usize {
    debug_once("flat_ugc", "SteamAPI_SteamUGC_v021");
    ugc_object() as *const usize
}

#[no_mangle]
pub extern "system" fn SteamAPI_SteamGameServerUGC_v021() -> *const usize {
    ugc_object() as *const usize
}

#[no_mangle]
pub extern "system" fn SteamAPI_ISteamUGC(_this: *mut c_void) -> *const usize {
    ugc_object() as *const usize
}

#[no_mangle]
pub extern "system" fn SteamAPI_ISteamUGC_GetNumSubscribedItems(
    this: *mut c_void,
    include_locally_disabled: bool,
) -> u32 {
    ugc_num_subscribed(this, include_locally_disabled)
}

#[no_mangle]
pub extern "system" fn SteamAPI_ISteamUGC_GetSubscribedItems(
    this: *mut c_void,
    ids: *mut u64,
    max_entries: u32,
    include_locally_disabled: bool,
) -> u32 {
    ugc_subscribed_items(this, ids, max_entries, include_locally_disabled)
}

#[no_mangle]
pub extern "system" fn SteamAPI_ISteamUGC_GetItemState(this: *mut c_void, id: u64) -> u32 {
    ugc_item_state(this, id)
}

#[no_mangle]
pub extern "system" fn SteamAPI_ISteamUGC_GetItemInstallInfo(
    this: *mut c_void,
    id: u64,
    size_on_disk: *mut u64,
    folder: *mut c_char,
    folder_size: u32,
    timestamp: *mut u32,
) -> bool {
    ugc_install_info(this, id, size_on_disk, folder, folder_size, timestamp)
}

#[no_mangle]
pub extern "system" fn SteamAPI_ISteamUGC_GetItemDownloadInfo(
    this: *mut c_void,
    id: u64,
    downloaded: *mut u64,
    total: *mut u64,
) -> bool {
    ugc_download_info(this, id, downloaded, total)
}

#[no_mangle]
pub extern "system" fn SteamAPI_ISteamUGC_DownloadItem(
    this: *mut c_void,
    id: u64,
    high_priority: bool,
) -> bool {
    ugc_download_item(this, id, high_priority)
}

/// Accesseurs plats `ISteamClient` utilisés par `com.rlabrecque.steamworks.net`.
/// Le contexte .NET demande ses interfaces au client Steam au lieu des
/// accesseurs `SteamAPI_Steam*_vNNN` ; sans ces exports, `SteamAPI.Init`
/// lève `EntryPointNotFoundException` et le jeu retombe sur NoopPlatformSDK.
macro_rules! client_getters {
    ($($name:ident => $object:ident),* $(,)?) => {
        $(
            #[no_mangle]
            pub extern "system" fn $name(
                _client: *mut c_void,
                _user: i32,
                _pipe: i32,
                _version: *const c_char,
            ) -> *mut c_void {
                $object()
            }
        )*
    };
}

client_getters! {
    SteamAPI_ISteamClient_GetISteamUser => user_object,
    SteamAPI_ISteamClient_GetISteamFriends => friends_object,
    SteamAPI_ISteamClient_GetISteamUtils => utils_object,
    SteamAPI_ISteamClient_GetISteamUserStats => stats_object,
    SteamAPI_ISteamClient_GetISteamApps => apps_object,
    SteamAPI_ISteamClient_GetISteamMatchmaking => matchmaking_object,
    SteamAPI_ISteamClient_GetISteamMatchmakingServers => servers_object,
    SteamAPI_ISteamClient_GetISteamRemoteStorage => remote_object,
    SteamAPI_ISteamClient_GetISteamUGC => ugc_object,
    SteamAPI_ISteamClient_GetISteamInput => input_object,
    SteamAPI_ISteamClient_GetISteamController => input_object,
    SteamAPI_ISteamClient_GetISteamNetworking => net_object,
    SteamAPI_ISteamClient_GetISteamGenericInterface => generic_object,
    SteamAPI_ISteamClient_GetISteamAppList => generic_object,
    SteamAPI_ISteamClient_GetISteamGameSearch => generic_object,
    SteamAPI_ISteamClient_GetISteamGameServer => generic_object,
    SteamAPI_ISteamClient_GetISteamGameServerStats => generic_object,
    SteamAPI_ISteamClient_GetISteamHTMLSurface => generic_object,
    SteamAPI_ISteamClient_GetISteamHTTP => generic_object,
    SteamAPI_ISteamClient_GetISteamInventory => generic_object,
    SteamAPI_ISteamClient_GetISteamMusic => generic_object,
    SteamAPI_ISteamClient_GetISteamMusicRemote => generic_object,
    SteamAPI_ISteamClient_GetISteamParentalSettings => generic_object,
    SteamAPI_ISteamClient_GetISteamParties => generic_object,
    SteamAPI_ISteamClient_GetISteamRemotePlay => generic_object,
    SteamAPI_ISteamClient_GetISteamScreenshots => generic_object,
    SteamAPI_ISteamClient_GetISteamVideo => generic_object,
}

#[no_mangle]
pub extern "system" fn SteamAPI_SteamMatchmaking_v009(_this: *mut c_void) -> *const usize {
    debug_once("flat_matchmaking", "SteamAPI_SteamMatchmaking_v009");
    matchmaking_object() as *const usize
}

#[no_mangle]
pub extern "system" fn SteamAPI_SteamUser_v023() -> *const usize {
    debug_once("flat_user", "SteamAPI_SteamUser_v023");
    user_object() as *const usize
}

#[no_mangle]
pub extern "system" fn SteamAPI_SteamFriends_v018() -> *const usize {
    debug_once("flat_friends", "SteamAPI_SteamFriends_v018");
    friends_object() as *const usize
}

#[no_mangle]
pub extern "system" fn SteamAPI_SteamUtils_v010() -> *const usize {
    debug_once("flat_utils", "SteamAPI_SteamUtils_v010");
    utils_object() as *const usize
}

#[no_mangle]
pub extern "system" fn SteamAPI_SteamInput_v006() -> *const usize {
    input_object() as *const usize
}

#[no_mangle]
pub extern "system" fn SteamAPI_SteamMatchmakingServers_v002() -> *const usize {
    servers_object() as *const usize
}

#[no_mangle]
pub extern "system" fn SteamAPI_ISteamMatchmaking_CreateLobby(
    this: *mut c_void,
    lobby_type: i32,
    max_members: i32,
) -> u64 {
    matchmaking_create_lobby(this, lobby_type, max_members)
}

#[cfg(test)]
mod tests {
    use super::*;

    static CAPTURED: Mutex<Option<(u64, u64)>> = Mutex::new(None);

    /// Les tests qui posent `LAPINUX_ACHIEVEMENT_SPOOL` partagent
    /// l'environnement du processus : sans verrou, l'un lit le dossier que
    /// l'autre vient de choisir.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    extern "system" fn capture(payload: *mut c_void) {
        if payload.is_null() {
            return;
        }
        let words = unsafe { std::slice::from_raw_parts(payload.cast::<u64>(), 2) };
        *CAPTURED.lock().unwrap() = Some((words[0], words[1]));
    }

    static CONTEXT_CALLS: AtomicU64 = AtomicU64::new(0);

    extern "system" fn init_context(context: *mut c_void) {
        CONTEXT_CALLS.fetch_add(1, Ordering::Relaxed);
        unsafe { *(context as *mut usize) = 0x1234 };
    }

    #[test]
    fn context_init_appelle_l_initialiseur_une_fois() {
        CONTEXT_CALLS.store(0, Ordering::Relaxed);
        let mut data: [usize; 3] = [init_context as usize, 0, 0];
        let context = SteamInternal_ContextInit(data.as_mut_ptr().cast());
        assert_eq!(context as *mut usize, unsafe { data.as_mut_ptr().add(2) });
        assert_eq!(data[2], 0x1234);
        assert_eq!(data[1], 1);
        let _ = SteamInternal_ContextInit(data.as_mut_ptr().cast());
        assert_eq!(CONTEXT_CALLS.load(Ordering::Relaxed), 1);
    }

    fn vtable_of(interface: *mut c_void) -> *const usize {
        unsafe { *(interface as *const *const usize) }
    }

    #[test]
    fn les_versions_steam_rendent_les_bonnes_interfaces() {
        let matchmaking = vtable_of(interface_for("SteamMatchMaking009"));
        assert_eq!(
            unsafe { *matchmaking.add(MATCHMAKING_CREATE_LOBBY) },
            matchmaking_create_lobby as usize
        );
        let user = vtable_of(interface_for("SteamUser023"));
        assert_eq!(unsafe { *user.add(1) }, user_logged_on as usize);
        assert_eq!(vtable_of(interface_for("SteamUserStats015")), table());
        assert_eq!(
            vtable_of(interface_for("STEAMUSERSTATS_INTERFACE_VERSION013")),
            table()
        );
        assert_eq!(vtable_of(interface_for("SteamFriends018")), friends_table());
        assert_eq!(vtable_of(interface_for("SteamUtils010")), utils_table());
        assert_eq!(vtable_of(interface_for("SteamInput006")), input_table());
        assert_eq!(
            vtable_of(interface_for("SteamMatchMakingServers002")),
            servers_table()
        );
        assert_eq!(
            vtable_of(interface_for("STEAMAPPS_INTERFACE_VERSION009")),
            apps_table()
        );
        // `GetFriendPersonaName` (6) et `GetFriendRichPresence` (43) rendent
        // des chaînes : un `const char*` nul ferait lire un `FString` invalide.
        let friends = friends_table();
        assert_eq!(unsafe { *friends.add(1) }, friends_persona_state as usize);
        assert_eq!(unsafe { *friends.add(2) }, friends_count as usize);
        assert_eq!(unsafe { *friends.add(6) }, friends_name as usize);
        assert_eq!(unsafe { *friends.add(43) }, friends_rich_presence as usize);
        // `ISteamUser023` : les jetons d'authentification sont en 13 et 14,
        // `GetUserDataFolder` en 6.
        assert_eq!(unsafe { *user.add(6) }, user_data_folder as usize);
        assert_eq!(
            unsafe { *user.add(13) },
            user_get_auth_session_ticket as usize
        );
        assert_eq!(
            unsafe { *user.add(14) },
            user_get_auth_ticket_for_webapi as usize
        );
        // `STEAMUSERSTATS_INTERFACE_VERSION013` : `GetAchievement` en 5,
        // `SetAchievement` en 6, `StoreStats` en 9.
        let stats = table();
        assert_eq!(unsafe { *stats.add(5) }, stats_get_achievement as usize);
        assert_eq!(unsafe { *stats.add(6) }, stats_set_achievement as usize);
        assert_eq!(unsafe { *stats.add(9) }, stats_store as usize);
        // `SteamMatchMaking009` : membre 32, joignabilité 34, propriétaire 35.
        let matchmaking_slots = matchmaking_table();
        assert_eq!(
            unsafe { *matchmaking_slots.add(32) },
            matchmaking_member_limit as usize
        );
        assert_eq!(
            unsafe { *matchmaking_slots.add(34) },
            matchmaking_set_joinable as usize
        );
        assert_eq!(
            unsafe { *matchmaking_slots.add(35) },
            matchmaking_lobby_owner as usize
        );

        let version = CString::new("SteamMatchMaking009").unwrap();
        let pointer = SteamInternal_FindOrCreateUserInterface(1, version.as_ptr());
        assert_eq!(vtable_of(pointer), matchmaking_table());
    }

    #[test]
    fn create_lobby_est_a_l_emplacement_de_l_interface() {
        let table = matchmaking_table();
        assert_eq!(
            unsafe { *table.add(MATCHMAKING_CREATE_LOBBY) },
            matchmaking_create_lobby as usize
        );
        // `GetLobbyByIndex` reste un stub : un jeu qui parcourt la liste des
        // lobbies ne doit pas déclencher une création par erreur.
        assert_ne!(unsafe { *table.add(12) }, matchmaking_create_lobby as usize);
    }

    #[test]
    fn create_lobby_livre_le_vrai_identifiant_puis_echoue_proprement() {
        let _env = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = std::env::temp_dir().join(format!("lapinux-lobby-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("LAPINUX_ACHIEVEMENT_SPOOL", &dir);
        std::env::set_var("LAPINUX_LOBBY_TIMEOUT_MS", "250");
        std::env::set_var("SteamAppId", "480");
        callbacks().lock().unwrap().clear();
        results().lock().unwrap().clear();
        // `SteamAPI_RegisterCallback` reçoit un objet `CCallbackBase` : sa
        // première entrée de vtable est `Run(void*)`, appelée par RunCallbacks.
        #[repr(C)]
        struct TestCallback {
            vtable: *const usize,
        }

        extern "system" fn run_capture(_object: *mut c_void, payload: *mut c_void) {
            capture(payload);
        }

        static CAPTURE_VTABLE: OnceLock<[usize; 3]> = OnceLock::new();
        let vtable = CAPTURE_VTABLE.get_or_init(|| [run_capture as usize, 0, 0]);
        let mut callback = TestCallback {
            vtable: vtable.as_ptr(),
        };
        SteamAPI_RegisterCallback(
            (&mut callback as *mut TestCallback).cast(),
            LOBBY_CREATED_CALLBACK,
        );

        // Un faux daemon répond dès que la demande apparaît.
        let watch = dir.clone();
        let responder = std::thread::spawn(move || {
            for _ in 0..200 {
                if let Ok(entries) = std::fs::read_dir(&watch) {
                    for entry in entries.flatten() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        if name.starts_with("lobby-") && name.ends_with(".json") {
                            let reply = watch.join(format!("{name}.reply"));
                            let _ =
                                std::fs::write(reply, r#"{"ok":true,"lobby":109775240000000001}"#);
                            return;
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });

        // Chemin succès : appel par la table, comme le fait un jeu.
        let table = matchmaking_table();
        let create: extern "system" fn(*mut c_void, i32, i32) -> u64 =
            unsafe { std::mem::transmute(*table.add(MATCHMAKING_CREATE_LOBBY)) };
        let call = create(std::ptr::null_mut(), 2, 4);
        assert!(call > 0);
        responder.join().unwrap();
        SteamAPI_RunCallbacks();
        assert_eq!(
            *CAPTURED.lock().unwrap(),
            Some((1, 109775240000000001)),
            "LobbyCreated_t doit porter eresult 1 et le vrai lobby"
        );

        // Chemin refus : le daemon transmet l'eresult rendu par le CM.
        *CAPTURED.lock().unwrap() = None;
        let watch = dir.clone();
        let refuser = std::thread::spawn(move || {
            for _ in 0..200 {
                if let Ok(entries) = std::fs::read_dir(&watch) {
                    for entry in entries.flatten() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        if name.starts_with("lobby-") && name.ends_with(".json") {
                            let reply = watch.join(format!("{name}.reply"));
                            let _ = std::fs::write(reply, r#"{"ok":false,"eresult":25}"#);
                            return;
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        let call = create(std::ptr::null_mut(), 2, 4);
        assert!(call > 0);
        refuser.join().unwrap();
        SteamAPI_RunCallbacks();
        assert_eq!(
            *CAPTURED.lock().unwrap(),
            Some((25, 0)),
            "LobbyCreated_t doit porter l'eresult du refus, lobby nul"
        );

        // Chemin échec : aucun daemon ne répond.
        *CAPTURED.lock().unwrap() = None;
        let call = create(std::ptr::null_mut(), 2, 4);
        assert!(call > 0);
        SteamAPI_RunCallbacks();
        let (eresult, lobby) = CAPTURED.lock().unwrap().unwrap();
        assert_ne!(eresult, 1);
        assert_eq!(lobby, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    use std::ffi::CString;

    fn reset(dir: &std::path::Path) {
        std::env::set_var("LAPINUX_ACHIEVEMENT_SPOOL", dir);
        let mut s = state().lock().unwrap();
        s.pending.clear();
        s.unlocked.clear();
        s.appid = 480;
    }

    #[test]
    fn join_lobby_entre_et_sert_l_adresse() {
        std::env::set_var("LAPINUX_JOIN_CONNECT", "192.168.1.50:7777");
        results().lock().unwrap().clear();
        let table = matchmaking_table();
        let join: extern "system" fn(*mut c_void, u64) -> u64 =
            unsafe { std::mem::transmute(*table.add(14)) };
        assert!(join(std::ptr::null_mut(), 42) > 0);
        let pending = results().lock().unwrap().clone();
        let (id, payload) = pending.last().expect("LobbyEnter en attente");
        assert_eq!(*id, LOBBY_ENTER_CALLBACK);
        assert_eq!(payload[0], 42);
        assert_eq!(payload[2], 1, "entrée acceptée");

        let data = matchmaking_lobby_data(std::ptr::null_mut(), 42, b"connect\0".as_ptr().cast());
        assert_eq!(
            unsafe { CStr::from_ptr(data) }.to_str().unwrap(),
            "192.168.1.50:7777"
        );
        let mut ip = 0u32;
        let mut port = 0u16;
        assert!(matchmaking_lobby_game_server(
            std::ptr::null_mut(),
            42,
            &mut ip,
            &mut port,
            std::ptr::null_mut(),
        ));
        assert_eq!(ip, u32::from_be_bytes([192, 168, 1, 50]));
        assert_eq!(port, 7777);
        // Un autre lobby ne reçoit pas cette adresse.
        let other = matchmaking_lobby_data(std::ptr::null_mut(), 43, b"connect\0".as_ptr().cast());
        assert!(unsafe { CStr::from_ptr(other) }.to_bytes().is_empty());
    }

    #[test]
    fn capture_seulement_ce_que_le_jeu_accorde() {
        let _env = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = std::env::temp_dir().join(format!("lapinux-win-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        reset(&dir);

        let meat = CString::new("achievement_meat").unwrap();
        let sleep = CString::new("achievement_sleepy").unwrap();
        set_achievement(&meat);
        set_achievement(&sleep);
        clear_achievement(&sleep);
        assert!(flush_pending());

        let files: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert_eq!(files.len(), 1);
        let body = std::fs::read_to_string(&files[0]).unwrap();
        assert!(body.contains("\"achievement_meat\""));
        assert!(!body.contains("achievement_sleepy"));
        assert!(body.contains("\"appid\":480"));

        // Deja debloque : pas de nouvelle ecriture.
        set_achievement(&meat);
        assert!(flush_pending());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn manifest_atelier_lit_ids_poids_et_chemins() {
        let text = "3812321265\t6447548\tZ:\\var\\mnt\\Jeux\\content\\3812321265\r\n\
                    3815663735\t12\tZ:\\var\\mnt\\Jeux\\content\\3815663735\n\
                    ligne-invalide\n";
        let items = parse_workshop_manifest(text);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, 3812321265);
        assert_eq!(items[0].size, 6_447_548);
        assert_eq!(
            items[0].folder.to_str().unwrap(),
            r"Z:\var\mnt\Jeux\content\3812321265"
        );
        assert_eq!(items[1].id, 3815663735);
    }

    #[test]
    fn la_table_ugc_place_les_methodes_atelier() {
        let table = ugc_table();
        assert_eq!(unsafe { *table.add(74) }, ugc_num_subscribed as usize);
        assert_eq!(unsafe { *table.add(75) }, ugc_subscribed_items as usize);
        assert_eq!(unsafe { *table.add(76) }, ugc_item_state as usize);
        assert_eq!(unsafe { *table.add(77) }, ugc_install_info as usize);
        assert_eq!(unsafe { *table.add(78) }, ugc_download_info as usize);
        assert_eq!(unsafe { *table.add(79) }, ugc_download_item as usize);
        // `ISteamUGC021` s'arrête à 95 (`SetSubscriptionsLoadOrder`) : les
        // anciens slots 96 à 98 n'existent pas dans la vtable.
        assert_eq!(unsafe { *table.add(94) }, ugc_set_items_disabled as usize);
        assert_eq!(unsafe { *table.add(95) }, ugc_set_load_order as usize);
    }
}
