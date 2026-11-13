//! Profils Steam Input lus sur le disque : les configurations que Steam garde
//! pour le compte, jeu par jeu et type de manette par type de manette.
//!
//! Lapinux peut lire les profils Steam existants et conserver ses propres
//! sélections pour les convertir lors du lancement direct, sans client Steam.

use crate::{session, vdf, Library};
use lapinux_core::i18n::{t, t_args};
use lapinux_core::model::{
    steam_input_kind, steam_input_label, DeviceInfo, SteamInputCommunityProfile,
    SteamInputCommunityReport, SteamInputController, SteamInputProfile, SteamInputReport,
    SteamInputSaved,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

/// Budget de parcours : un compte très fourni ne doit pas retenir la fiche.
const MAX_FILES: usize = 800;
const MAX_DEPTH: usize = 7;

/// Catalogue public des configurations Steam Input. SteamInputDB utilise les
/// mêmes données publiées que Steam et les expose en JSON, sans clé Web API.
const COMMUNITY_URL: &str = "https://www.steaminputdb.com/app";
/// Recherche transversale : SteamInputDB-Buddy permet d'appliquer une
/// configuration d'un jeu à un autre, utile quand le jeu n'en a aucune.
const COMMUNITY_SEARCH_URL: &str = "https://www.steaminputdb.com/config/search";
/// Délai d'une recherche dans le catalogue communautaire.
const COMMUNITY_TIMEOUT: Duration = Duration::from_secs(15);
/// Un profil Steam Input pèse quelques dizaines de kilo-octets ; au-delà, ce
/// n'est pas un fichier de configuration et il ne faut pas l'écrire sur disque.
const COMMUNITY_MAX_FILE: usize = 2 * 1024 * 1024;
/// Les pages du catalogue contiennent vingt configurations.
const COMMUNITY_PAGE_SIZE: usize = 20;

/// Type Steam Input d'une manette, d'après son identifiant matériel.
pub fn controller_type(vendor: u16, product: u16) -> &'static str {
    // Steam Deck : le pad interne est un Valve, mais Steam le nomme
    // « neptune » et ses profils sont distincts de ceux du Steam Controller.
    steam_input_kind(vendor, product)
}

/// Manettes branchées, ramenées au type Steam Input qu'elles prennent.
///
/// Le pont `steam-puck-bridge` annonce un pad Xbox 360 : son drapeau le rend à
/// la Steam Controller 2026, sinon les profils proposés seraient ceux d'une
/// manette qui n'est pas là. La 2026 est le type `controller_triton` de Steam ;
/// `controller_steamcontroller` est l'ancien nom, que SteamInputDB ne filtre
/// pas comme une manette Triton.
pub fn controllers(devices: &[DeviceInfo]) -> Vec<SteamInputController> {
    let mut out: Vec<SteamInputController> = Vec::new();
    for device in devices
        .iter()
        .filter(|device| device.gamepad && device.connected)
    {
        let kind = if device.steam_bridge {
            "controller_triton"
        } else {
            controller_type(device.vendor, device.product)
        };
        if out
            .iter()
            .any(|known| known.kind == kind && known.product == device.product)
        {
            continue;
        }
        out.push(SteamInputController {
            kind: kind.to_string(),
            label: steam_input_label(kind).to_string(),
            name: device.name.clone(),
            vendor: device.vendor,
            product: device.product,
            connected: true,
        });
    }
    out
}

/// Dossiers où Steam range les configurations : les partagées du compte et
/// celles de chaque jeu. Les deux arborescences existent selon la version du
/// client, d'où le parcours des deux.
fn profile_roots(roots: &[PathBuf], libraries: &[Library]) -> Vec<PathBuf> {
    let mut bases: Vec<PathBuf> = roots.to_vec();
    for library in libraries {
        bases.push(library.root.clone());
    }
    bases.sort();
    bases.dedup();

    let mut out: Vec<PathBuf> = Vec::new();
    let mut push = |path: PathBuf| {
        if path.is_dir() && !out.iter().any(|known| known == &path) {
            out.push(path);
        }
    };
    for base in &bases {
        push(base.join("steamapps/common/Steam Controller Configs"));
        let userdata = base.join("userdata");
        let Ok(entries) = std::fs::read_dir(&userdata) else {
            continue;
        };
        for entry in entries.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            // Les profils téléchargés vivent directement sous `remote/`
            // (`<id>_controller_config.vdf`), pas dans un sous-dossier.
            push(entry.path().join("241100/remote"));
            push(entry.path().join("241100/remote/controller_config"));
            push(entry.path().join("config/controller_configs"));
        }
    }
    out
}

/// Rassemble les `.vdf` d'un dossier, sans suivre les liens symboliques : un
/// dossier de profils peut en contenir, et une boucle ne doit pas bloquer la
/// fiche.
fn collect(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > MAX_DEPTH || out.len() >= MAX_FILES {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if out.len() >= MAX_FILES {
            return;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_dir() {
            collect(&path, depth + 1, out);
        } else if kind.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("vdf") {
            out.push(path);
        }
    }
}

/// Appid lu dans le chemin : Steam range les profils sous un dossier nommé par
/// l'identifiant du jeu (`…/config/<appid>/…`). Le dossier du compte
/// (`Steam Controller Configs/311993954/config/`) n'est jamais pris pour un
/// appid : seul le composant qui suit un marqueur de configuration compte.
fn appid_of(path: &Path) -> u32 {
    let parts: Vec<&str> = path
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect();
    for (index, part) in parts.iter().enumerate() {
        if matches!(*part, "config" | "controller_config" | "controller_configs") {
            if let Some(appid) = parts
                .get(index + 1)
                .and_then(|next| next.parse::<u32>().ok())
            {
                if appid > 0 {
                    return appid;
                }
            }
        }
    }
    0
}

/// Identifiant d'atelier lu dans un nom de profil téléchargé
/// (`1481807135_controller_config.vdf`).
fn workshop_id_of(path: &Path) -> Option<u64> {
    let stem = path.file_stem()?.to_str()?;
    if !stem.contains("controller_config") {
        return None;
    }
    let digits: String = stem.chars().take_while(|ch| ch.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse().ok()
}

/// Jeu de chaque contenu d'atelier, lu dans les index `configset_*` : ces
/// fichiers associent un appid (ou un nom de raccourci) à l'identifiant du
/// profil choisi pour une manette.
fn workshop_appids(files: &[PathBuf]) -> HashMap<u64, u32> {
    let mut map = HashMap::new();
    for file in files {
        let Some(name) = file.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !name.starts_with("configset_") || !name.ends_with(".vdf") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(file) else {
            continue;
        };
        // `parse_root` enlève déjà l'enveloppe `controller_config`.
        let Ok(config) = vdf::parse_root(&text) else {
            continue;
        };
        for (key, entry) in config.objects() {
            let Ok(appid) = key.parse::<u32>() else {
                continue;
            };
            let Some(workshop) = entry.str("workshop") else {
                continue;
            };
            if let Ok(id) = workshop.parse::<u64>() {
                map.entry(id).or_insert(appid);
            }
        }
    }
    map
}

/// Provenance d'un profil, pour que la fiche distingue une configuration du
/// compte d'une configuration partagée par un jeu.
fn origin_of(path: &Path) -> String {
    let text = path.to_string_lossy();
    if text.contains("Steam Controller Configs") {
        "jeu"
    } else if text.contains("controller_config") {
        "compte"
    } else {
        "partagé"
    }
    .to_string()
}

/// Racine des mappings d'un profil : `controller_mappings` quand elle existe,
/// le document lui-même pour les fichiers qui l'enveloppent autrement.
fn mapping_of(root: &vdf::Obj) -> &vdf::Obj {
    root.obj("controller_mappings").unwrap_or(root)
}

fn profile(path: &Path, appid: u32) -> Option<SteamInputProfile> {
    let text = std::fs::read_to_string(path).ok()?;
    profile_from_text(path, appid, &text)
}

fn profile_from_text(path: &Path, appid: u32, text: &str) -> Option<SteamInputProfile> {
    let root = vdf::parse_root(text).ok()?;
    // La racine habituelle est `controller_mappings` ; certains fichiers
    // l'enveloppent sous une clé de plus.
    let mapping = mapping_of(&root);
    let controller_type = mapping.str("controller_type")?.trim();
    if controller_type.is_empty() {
        return None;
    }
    let title = mapping.str("title").unwrap_or("").trim();
    let description = mapping.str("description").unwrap_or("").trim();
    let creator = mapping.str("creator").unwrap_or("").trim();
    let modified = std::fs::metadata(path)
        .ok()
        .and_then(|meta| meta.modified().ok())
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0);
    Some(SteamInputProfile {
        appid,
        title: if title.is_empty() {
            // Un profil sans titre est illisible dans une liste : le nom du
            // fichier reste mieux qu'une ligne vide.
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or_else(|| t("steaminput.untitled"))
                .to_string()
        } else {
            title.to_string()
        },
        description: description.to_string(),
        controller_type: controller_type.to_string(),
        creator: creator.to_string(),
        path: path.to_path_buf(),
        workshop_id: profile_workshop_id(path, mapping).unwrap_or(0),
        modified,
        origin: origin_of(path),
        active: false,
        ..Default::default()
    })
}

/// Identifiants matériels connus pour un type Steam Input. Steam nomme ses
/// configsets par manette physique (`configset_54c-ce6-…`) : sans eux, une
/// sélection resterait dans le fichier générique et la manette branchée ne la
/// verrait pas.
fn hardware_ids(kind: &str) -> &'static [(u16, u16)] {
    match kind {
        "controller_ps5" | "controller_ps5_edge" => &[(0x054c, 0x0ce6), (0x054c, 0x0df2)],
        "controller_ps4" => &[(0x054c, 0x05c4), (0x054c, 0x09cc), (0x054c, 0x0ba0)],
        "controller_xboxone" => &[
            (0x045e, 0x0b12),
            (0x045e, 0x0b13),
            (0x045e, 0x0b20),
            (0x045e, 0x02fd),
            (0x045e, 0x02e0),
        ],
        "controller_xbox360" => &[
            (0x045e, 0x028e),
            (0x045e, 0x0291),
            (0x045e, 0x02d1),
            (0x045e, 0x0719),
            (0x045e, 0x0867),
        ],
        "controller_switch_pro" => &[(0x057e, 0x2009)],
        "controller_switch_joycon_pair" => &[(0x057e, 0x2006), (0x057e, 0x2007)],
        "controller_steamcontroller" => &[(0x28de, 0x1142), (0x28de, 0x1205), (0x28de, 0x1206)],
        "controller_triton" => &[
            (0x28de, 0x1302),
            (0x28de, 0x1303),
            (0x28de, 0x1304),
            (0x28de, 0x1305),
        ],
        "controller_neptune" => &[(0x28de, 0x1205), (0x28de, 0x1206)],
        _ => &[],
    }
}

/// Types Steam Input qu'un nom de configset peut désigner. Un fichier par
/// manette physique peut correspondre à plusieurs types quand le matériel est
/// partagé (Steam Controller et Steam Deck, DualSense et DualSense Edge).
fn kinds_of_configset(name: &str) -> Vec<String> {
    let Some(stem) = name
        .strip_prefix("configset_")
        .and_then(|stem| stem.strip_suffix(".vdf"))
    else {
        return Vec::new();
    };
    if stem.starts_with("controller_") {
        return vec![stem.to_string()];
    }
    let mut kinds = Vec::new();
    for kind in [
        "controller_ps5",
        "controller_ps5_edge",
        "controller_ps4",
        "controller_xboxone",
        "controller_xbox360",
        "controller_switch_pro",
        "controller_switch_joycon_pair",
        "controller_steamcontroller",
        "controller_triton",
        "controller_neptune",
    ] {
        for (vendor, product) in hardware_ids(kind) {
            if stem.starts_with(&format!("{vendor:x}-{product:x}-")) {
                kinds.push(kind.to_string());
            }
        }
    }
    kinds.sort();
    kinds.dedup();
    kinds
}

fn has_configset(dir: &Path) -> bool {
    fs::read_dir(dir).ok().is_some_and(|entries| {
        entries.flatten().any(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            name.starts_with("configset_") && name.ends_with(".vdf")
        })
    })
}

fn push_configset_dir(dir: PathBuf, out: &mut Vec<PathBuf>) {
    if !has_configset(&dir) {
        return;
    }
    let real = fs::canonicalize(&dir).unwrap_or(dir);
    if !out.contains(&real) {
        out.push(real);
    }
}

/// Dossiers qui portent les `configset_*.vdf` du compte.
fn configset_dirs(roots: &[PathBuf], libraries: &[Library]) -> Vec<PathBuf> {
    let mut bases: Vec<PathBuf> = roots.to_vec();
    for library in libraries {
        bases.push(library.root.clone());
    }
    bases.sort();
    bases.dedup();

    let mut out = Vec::new();
    for base in bases {
        let steam_ctrl = base.join("steamapps/common/Steam Controller Configs");
        if let Ok(accounts) = fs::read_dir(&steam_ctrl) {
            for account in accounts.flatten() {
                push_configset_dir(account.path().join("config"), &mut out);
            }
        }
        let userdata = base.join("userdata");
        if let Ok(accounts) = fs::read_dir(&userdata) {
            for account in accounts.flatten() {
                push_configset_dir(account.path().join("config"), &mut out);
            }
        }
    }
    out
}

fn current_account() -> Option<u32> {
    session::load()
        .map(|saved| (saved.steam_id & 0xFFFF_FFFF) as u32)
        .filter(|id| *id != 0)
}

/// Dossier du compte courant. Un seul compte sur la machine suffit ; quand il
/// y en a plusieurs, la session Steam dit lequel ne pas toucher.
fn config_dir(roots: &[PathBuf], libraries: &[Library]) -> Result<PathBuf, String> {
    let dirs = configset_dirs(roots, libraries);
    if dirs.len() == 1 {
        return Ok(dirs[0].clone());
    }
    if dirs.is_empty() {
        return Err(t("steaminput.err_config_dir").to_string());
    }
    if let Some(account) = current_account() {
        let account_name = account.to_string();
        if let Some(dir) = dirs.iter().find(|dir| {
            dir.parent()
                .and_then(|parent| parent.file_name())
                .and_then(|name| name.to_str())
                == Some(account_name.as_str())
        }) {
            return Ok(dir.clone());
        }
    }
    Err(t("steaminput.err_multiple_accounts").to_string())
}

/// Tous les fichiers où Steam peut lire la sélection de cette manette.
fn configset_paths(dir: &Path, controller_type: &str) -> Vec<PathBuf> {
    let mut out = vec![dir.join(format!("configset_{controller_type}.vdf"))];
    let prefixes: Vec<String> = hardware_ids(controller_type)
        .iter()
        .map(|(vendor, product)| format!("configset_{vendor:x}-{product:x}-"))
        .collect();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with("configset_") || !name.ends_with(".vdf") {
                continue;
            }
            if prefixes.iter().any(|prefix| name.starts_with(prefix)) {
                out.push(entry.path());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Ce que les configsets disent du jeu pour un type de manette.
#[derive(Default, Clone, Copy)]
struct Selection {
    workshop: Option<u64>,
    autosave: bool,
}

fn selections_for(dir: &Path, appid: u32) -> HashMap<String, Selection> {
    let mut out: HashMap<String, Selection> = HashMap::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let kinds = kinds_of_configset(&name);
        if kinds.is_empty() {
            continue;
        }
        let Ok(text) = fs::read_to_string(entry.path()) else {
            continue;
        };
        let Ok(root) = vdf::parse(&text) else {
            continue;
        };
        let Some(config) = root.obj("controller_config") else {
            continue;
        };
        let Some(selected) = config.obj(&appid.to_string()) else {
            continue;
        };
        let workshop = selected
            .str("workshop")
            .and_then(|text| text.trim().parse::<u64>().ok());
        let autosave = selected
            .str("autosave")
            .map(|text| text.trim() == "1")
            .unwrap_or(false);
        for kind in kinds {
            let selection = out.entry(kind).or_default();
            if workshop.is_some() {
                selection.workshop = workshop;
            }
            if autosave {
                selection.autosave = true;
            }
        }
    }
    out
}

fn canonical_eq(left: &Path, right: &Path) -> bool {
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

/// Identifiant d'atelier d'un profil téléchargé. Un profil local (autosave)
/// reste local même s'il descend d'un atelier : ses modifications ne doivent
/// pas être perdues en le remplaçant par l'original.
fn profile_workshop_id(path: &Path, mapping: &vdf::Obj) -> Option<u64> {
    let stem = path.file_stem()?.to_str()?;
    let remote = path.to_string_lossy().contains("/241100/remote/");
    if remote || stem.contains("controller_config") {
        let digits: String = stem
            .chars()
            .take_while(|character| character.is_ascii_digit())
            .collect();
        if let Ok(id) = digits.parse::<u64>() {
            return Some(id);
        }
    }
    if remote {
        return mapping
            .str("progenitor")
            .and_then(workshop_link)
            .or_else(|| mapping.str("url").and_then(workshop_link));
    }
    None
}

fn workshop_link(text: &str) -> Option<u64> {
    text.trim().strip_prefix("workshop://")?.parse().ok()
}

fn active_for(
    path: &Path,
    mapping: &vdf::Obj,
    appid: u32,
    dir: &Path,
    selections: &HashMap<String, Selection>,
) -> bool {
    let controller_type = mapping.str("controller_type").unwrap_or("").trim();
    let Some(selection) = selections.get(controller_type) else {
        return false;
    };
    if let Some(id) = profile_workshop_id(path, mapping) {
        return selection.workshop == Some(id);
    }
    if !selection.autosave {
        return false;
    }
    canonical_eq(
        path,
        &dir.join(appid.to_string())
            .join(format!("{controller_type}.vdf")),
    )
}

fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    if fs::read_to_string(path).ok().as_deref() == Some(text) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("{} : {error}", parent.display()))?;
    }
    let temp = path.with_extension("lapinux-tmp");
    fs::write(&temp, text).map_err(|error| format!("{} : {error}", temp.display()))?;
    fs::rename(&temp, path).map_err(|error| format!("{} : {error}", path.display()))?;
    Ok(())
}

/// Applique un profil au jeu : Steam lira cette sélection dans ses
/// `configset_*.vdf` au prochain lancement. Le profil local est d'abord copié
/// dans le slot autosave du jeu, pour que Steam n'ait pas à connaître le jeu
/// d'origine. Rend le rapport relu, avec l'état actif à jour.
pub fn apply(
    roots: &[PathBuf],
    libraries: &[Library],
    devices: &[DeviceInfo],
    appid: u32,
    profile_path: &Path,
) -> Result<SteamInputReport, String> {
    if appid == 0 {
        return Err(t("steaminput.err_no_appid").to_string());
    }
    let text =
        fs::read_to_string(profile_path).map_err(|error| t_args("steaminput.err_profile_unreadable", &[("error", &error.to_string())]))?;
    let root = vdf::parse_root(&text).map_err(|error| t_args("steaminput.err_profile_unreadable", &[("error", &error.to_string())]))?;
    let mapping = mapping_of(&root);
    let controller_type = mapping.str("controller_type").unwrap_or("").trim();
    if controller_type.is_empty() {
        return Err(t("steaminput.err_not_controller").to_string());
    }
    let dir = config_dir(roots, libraries)?;
    let workshop = profile_workshop_id(profile_path, mapping);
    if workshop.is_none() {
        let destination = dir
            .join(appid.to_string())
            .join(format!("{controller_type}.vdf"));
        if !canonical_eq(profile_path, &destination) {
            write_atomic(&destination, &text)?;
        }
    }

    let mut written = 0usize;
    for path in configset_paths(&dir, controller_type) {
        let mut document = match fs::read_to_string(&path) {
            Ok(text) => {
                vdf::parse(&text).map_err(|error| format!("{} : {error}", path.display()))?
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => vdf::Obj::default(),
            Err(error) => return Err(format!("{} : {error}", path.display())),
        };
        if !document.0.contains_key("controller_config") {
            document.0.insert(
                "controller_config".to_string(),
                vdf::Value::Obj(vdf::Obj::default()),
            );
        }
        let config = document
            .obj_mut("controller_config")
            .ok_or_else(|| {
                t_args(
                    "steaminput.err_unexpected_root",
                    &[("path", &path.display().to_string())],
                )
            })?;
        if !matches!(config.0.get(&appid.to_string()), Some(vdf::Value::Obj(_))) {
            config
                .0
                .insert(appid.to_string(), vdf::Value::Obj(vdf::Obj::default()));
        }
        let entry = config
            .obj_mut(&appid.to_string())
            .ok_or_else(|| {
                t_args(
                    "steaminput.err_invalid_game_entry",
                    &[("path", &path.display().to_string())],
                )
            })?;
        entry.0.remove("template");
        entry.0.remove("config");
        match workshop {
            Some(id) => {
                entry.0.remove("autosave");
                entry
                    .0
                    .insert("workshop".to_string(), vdf::Value::Str(id.to_string()));
            }
            None => {
                entry.0.remove("workshop");
                entry
                    .0
                    .insert("autosave".to_string(), vdf::Value::Str("1".to_string()));
            }
        }
        write_atomic(&path, &vdf::write(&document))?;
        written += 1;
    }
    tracing::info!(
        "Steam Input : {controller_type} appliqué au jeu {appid} dans {written} configset(s)"
    );
    Ok(report(roots, libraries, devices, appid))
}

/// Chemin de la copie locale d'une configuration communautaire.
///
/// Le nom ne contient volontairement pas `controller_config` : le fichier est
/// une autosave locale, pas un contenu d'Atelier que Steam devrait retrouver
/// dans son cache. Ainsi [`apply`] le copie dans le slot du jeu au lieu
/// d'écrire un identifiant d'atelier que Steam ne posséderait pas.
fn community_cache_path(data_dir: &Path, id: u64) -> PathBuf {
    data_dir
        .join("steaminput")
        .join("community")
        .join(format!("community-{id}.vdf"))
}

/// Dossier des copies que Lapinux conserve, indépendamment du cache CDN.
fn saved_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("steaminput").join("saved")
}

fn saved_path(data_dir: &Path, appid: u32, id: u64) -> PathBuf {
    saved_dir(data_dir)
        .join(appid.to_string())
        .join(format!("community-{id}.vdf"))
}

fn saved_manifest_path(data_dir: &Path, appid: u32, id: u64) -> PathBuf {
    saved_dir(data_dir)
        .join(appid.to_string())
        .join(format!("community-{id}.json"))
}

/// Métadonnées persistantes d'une configuration conservée. Le `.vdf` reste la
/// source du mapping ; ce fichier ne sert qu'à retrouver son jeu et son titre
/// sans relire tout le disque Steam.
#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
struct SavedManifest {
    id: u64,
    appid: u32,
    source_appid: u32,
    title: String,
    description: String,
    controller_type: String,
    creator: String,
    modified: i64,
    saved_at: i64,
}

impl Default for SavedManifest {
    fn default() -> Self {
        Self {
            id: 0,
            appid: 0,
            source_appid: 0,
            title: String::new(),
            description: String::new(),
            controller_type: String::new(),
            creator: String::new(),
            modified: 0,
            saved_at: 0,
        }
    }
}

/// Recopie une configuration communautaire dans le dossier de Lapinux et
/// écrit ses métadonnées. Appelée seulement après une application réussie :
/// avant le clic, rien n'est conservé.
fn keep_community_copy(
    data_dir: &Path,
    appid: u32,
    profile: &SteamInputCommunityProfile,
    text: &str,
) -> Result<(), String> {
    let target = saved_path(data_dir, appid, profile.id);
    write_atomic(&target, text)?;
    let manifest = SavedManifest {
        id: profile.id,
        appid,
        source_appid: profile.appid,
        title: profile.title.clone(),
        description: profile.description.clone(),
        controller_type: profile.controller_type.clone(),
        creator: profile.creator.clone(),
        modified: profile.modified,
        saved_at: lapinux_core::util::now_unix(),
    };
    let json = serde_json::to_string_pretty(&manifest)
        .map_err(|error| {
            t_args(
                "steaminput.err_metadata",
                &[("error", &error.to_string())],
            )
        })?;
    write_atomic(&saved_manifest_path(data_dir, appid, profile.id), &json)
}

/// Configurations conservées dans Lapinux, de la plus récente à la plus
/// ancienne. Un manifeste ou un `.vdf` absent est ignoré : la liste ne doit
/// pas s'arrêter sur une copie abîmée.
pub fn saved(data_dir: &Path) -> Vec<SteamInputSaved> {
    let root = saved_dir(data_dir);
    let Ok(appids) = fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for appid in appids.flatten() {
        let Ok(kind) = appid.file_type() else {
            continue;
        };
        if !kind.is_dir() {
            continue;
        }
        let Ok(entries) = fs::read_dir(appid.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            if entry.path().extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let Ok(json) = fs::read_to_string(entry.path()) else {
                continue;
            };
            let Ok(manifest) = serde_json::from_str::<SavedManifest>(&json) else {
                continue;
            };
            if manifest.id == 0 || manifest.appid == 0 {
                continue;
            }
            let path = saved_path(data_dir, manifest.appid, manifest.id);
            if !path.is_file() {
                continue;
            }
            let (preset, details_loaded, details_error, action_groups, settings) =
                match fs::read_to_string(&path) {
                    Ok(text) => match crate::steam_mapping::profile_details(&text) {
                        Ok(details) => (
                            details.preset,
                            true,
                            String::new(),
                            details.groups,
                            details.settings,
                        ),
                        Err(error) => (String::new(), false, error, Vec::new(), Vec::new()),
                    },
                    Err(error) => (
                        String::new(),
                        false,
                        error.to_string(),
                        Vec::new(),
                        Vec::new(),
                    ),
                };
            out.push(SteamInputSaved {
                id: manifest.id,
                appid: manifest.appid,
                source_appid: manifest.source_appid,
                title: if manifest.title.trim().is_empty() {
                    t("steaminput.untitled").into()
                } else {
                    manifest.title
                },
                description: manifest.description,
                controller_type: manifest.controller_type,
                creator: manifest.creator,
                modified: manifest.modified,
                saved_at: manifest.saved_at,
                path,
                preset,
                details_loaded,
                details_error,
                action_groups,
                settings,
            });
        }
    }
    out.sort_by(|left, right| {
        right
            .saved_at
            .cmp(&left.saved_at)
            .then_with(|| right.id.cmp(&left.id))
    });
    out
}

fn url_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'_' | b'.' | b'~') {
            out.push(*byte as char);
        } else {
            out.push('%');
            out.push_str(&format!("{byte:02X}"));
        }
    }
    out
}

fn community_url(appid: u32, query: &str, controller: Option<&str>, page: usize) -> String {
    let mut url = format!(
        "{COMMUNITY_URL}/{appid}/__data.json?sort-by=vote&page={}",
        page.max(1)
    );
    append_community_filters(&mut url, query, controller);
    url
}

fn community_search_url(query: &str, controller: Option<&str>, page: usize) -> String {
    let mut url = format!(
        "{COMMUNITY_SEARCH_URL}/__data.json?sort-by=vote&page={}",
        page.max(1)
    );
    append_community_filters(&mut url, query, controller);
    url
}

fn append_community_filters(url: &mut String, query: &str, controller: Option<&str>) {
    let query = query.trim();
    if !query.is_empty() {
        url.push_str("&searchtext=");
        url.push_str(&url_encode(query));
    }
    if let Some(controller) = controller.map(str::trim).filter(|kind| !kind.is_empty()) {
        url.push_str("&controller_type=");
        url.push_str(&url_encode(controller));
    }
}

fn fetch_community_value(url: &str) -> Result<Value, String> {
    let agent = ureq::AgentBuilder::new()
        .user_agent(crate::tools::USER_AGENT)
        .timeout(COMMUNITY_TIMEOUT)
        .build();
    let response = agent
        .get(url)
        // Sans cette entête, une réponse compressée arriverait telle quelle.
        .set("accept-encoding", "identity")
        .call()
        .map_err(|error| t_args("steaminput.err_catalog", &[("error", &error.to_string())]))?;
    let body = response
        .into_string()
        .map_err(|error| t_args("steaminput.err_catalog_unreadable", &[("error", &error.to_string())]))?;
    if body.len() > 4 * 1024 * 1024 {
        return Err(t("steaminput.err_catalog_too_large").into());
    }
    serde_json::from_str(&body).map_err(|error| t_args("steaminput.err_catalog", &[("error", &error.to_string())]))
}

/// Une valeur du tableau `data` peut être une référence vers une autre entrée.
/// On ne résout qu'un niveau : les nombres littéraux du catalogue (taille,
/// score, identifiant…) ont la même représentation et une résolution récursive
/// les confondrait avec des index. Le graphe SvelteKit contient en plus des
/// références cycliques, qu'il ne faut jamais parcourir en entier.
fn dereference<'a>(value: &'a Value, data: &'a [Value]) -> &'a Value {
    let Some(index) = value.as_u64().map(|index| index as usize) else {
        return value;
    };
    data.get(index).unwrap_or(value)
}

/// Champ d'un objet, avec sa référence SvelteKit résolue.
fn field<'a>(object: &'a Value, name: &str, data: &'a [Value]) -> Option<&'a Value> {
    object.get(name).map(|value| dereference(value, data))
}

fn text_field<'a>(object: &'a Value, name: &str, data: &'a [Value]) -> &'a str {
    field(object, name, data)
        .and_then(Value::as_str)
        .unwrap_or("")
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Date ISO 8601 de Steam (`2022-11-09T16:29:03Z`) en secondes Unix.
fn iso_to_unix(text: &str) -> i64 {
    let Some(year) = text.get(0..4).and_then(|part| part.parse::<i64>().ok()) else {
        return 0;
    };
    let Some(month) = text.get(5..7).and_then(|part| part.parse::<i64>().ok()) else {
        return 0;
    };
    let Some(day) = text.get(8..10).and_then(|part| part.parse::<i64>().ok()) else {
        return 0;
    };
    let hour = text
        .get(11..13)
        .and_then(|part| part.parse::<i64>().ok())
        .unwrap_or(0);
    let minute = text
        .get(14..16)
        .and_then(|part| part.parse::<i64>().ok())
        .unwrap_or(0);
    let second = text
        .get(17..19)
        .and_then(|part| part.parse::<i64>().ok())
        .unwrap_or(0);
    days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second
}

fn number_f64(value: Option<&Value>) -> f64 {
    value
        .and_then(|value| value.as_f64().or_else(|| value.as_i64().map(|n| n as f64)))
        .unwrap_or(0.0)
}

fn number_u64(value: Option<&Value>) -> u64 {
    value
        .and_then(|value| value.as_u64().or_else(|| value.as_f64().map(|n| n as u64)))
        .unwrap_or(0)
}

fn is_steam_cdn_url(url: &str) -> bool {
    url.starts_with("https://cdn.steamusercontent.com/")
        && url.len() < 2048
        && !url.chars().any(char::is_whitespace)
}

fn parse_community_report(
    configs: &Value,
    data: &[Value],
    target_appid: Option<u32>,
    data_dir: &Path,
    page: usize,
) -> Result<SteamInputCommunityReport, String> {
    let total = number_u64(configs.get("total").map(|value| dereference(value, data))) as usize;
    let items = configs
        .get("items")
        .map(|value| dereference(value, data))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut profiles = Vec::new();
    for item in items {
        let item = dereference(&item, data);
        if !item.is_object() {
            continue;
        }
        let id = number_u64(field(item, "file_id", data));
        if id == 0 {
            continue;
        }
        let item_appid = number_u64(field(item, "app_id", data)) as u32;
        if let Some(target) = target_appid {
            if item_appid != 0 && item_appid != target {
                continue;
            }
        }
        let controller_type = text_field(item, "controller_type", data).trim();
        if controller_type.is_empty() {
            continue;
        }
        let download_url = text_field(item, "file_url", data).trim();
        // Sans URL téléchargeable, la carte ne pourrait pas être appliquée.
        if !is_steam_cdn_url(download_url) {
            continue;
        }
        let title = text_field(item, "title", data).trim();
        let description = text_field(item, "description", data).trim();
        let creator = field(item, "creator_id", data)
            .and_then(|value| {
                value
                    .as_str()
                    .map(str::to_string)
                    .or_else(|| value.as_u64().map(|id| id.to_string()))
            })
            .unwrap_or_default();
        let votes = field(item, "votes", data);
        profiles.push(SteamInputCommunityProfile {
            id,
            appid: if item_appid == 0 {
                target_appid.unwrap_or(0)
            } else {
                item_appid
            },
            title: if title.is_empty() {
                t("steaminput.untitled").into()
            } else {
                title.to_string()
            },
            description: description.to_string(),
            controller_type: controller_type.to_string(),
            creator,
            download_url: download_url.to_string(),
            file_size: number_u64(field(item, "file_size", data)),
            modified: iso_to_unix(text_field(item, "time_updated", data)),
            rating: number_f64(votes.and_then(|votes| field(votes, "score", data))) as f32,
            votes_up: number_u64(votes.and_then(|votes| field(votes, "up", data))),
            votes_down: number_u64(votes.and_then(|votes| field(votes, "down", data))),
            subscriptions: number_u64(field(item, "subscriptions", data)),
            installed: community_cache_path(data_dir, id).is_file(),
        });
    }
    let mut notes = Vec::new();
    if profiles.is_empty() {
        notes.push(t("steaminput.note_no_match").into());
    }
    Ok(SteamInputCommunityReport {
        profiles,
        total,
        page: page.max(1),
        per_page: COMMUNITY_PAGE_SIZE,
        fallback: false,
        notes,
    })
}

/// Lit les configurations d'une réponse d'une page de jeu.
fn parse_community_value(
    value: &Value,
    appid: u32,
    data_dir: &Path,
    page: usize,
) -> Result<SteamInputCommunityReport, String> {
    let nodes = value
        .get("nodes")
        .and_then(Value::as_array)
        .ok_or_else(|| t("steaminput.err_catalog_no_data").to_string())?;
    let data = nodes
        .iter()
        .rev()
        .find_map(|node| node.get("data").and_then(Value::as_array))
        .ok_or_else(|| t("steaminput.err_catalog_no_data").to_string())?;
    // Le nœud de page expose `{ appInfo, configs }` ; `configs` est lui-même
    // une référence vers `{ total, items }`.
    let configs = data
        .iter()
        .find_map(|node| node.get("configs"))
        .map(|value| dereference(value, data))
        .filter(|value| value.is_object())
        .ok_or_else(|| t("steaminput.err_catalog_no_configs").to_string())?;
    parse_community_report(configs, data, Some(appid), data_dir, page)
}

/// Lit une réponse de la recherche transversale.
fn parse_community_search_value(
    value: &Value,
    data_dir: &Path,
    page: usize,
) -> Result<SteamInputCommunityReport, String> {
    let nodes = value
        .get("nodes")
        .and_then(Value::as_array)
        .ok_or_else(|| t("steaminput.err_catalog_no_data").to_string())?;
    let data = nodes
        .iter()
        .rev()
        .find_map(|node| node.get("data").and_then(Value::as_array))
        .ok_or_else(|| t("steaminput.err_catalog_no_data").to_string())?;
    let results = data
        .iter()
        .find_map(|node| node.get("results"))
        .map(|value| dereference(value, data))
        .filter(|value| value.is_object())
        .ok_or_else(|| t("steaminput.err_catalog_no_search").to_string())?;
    parse_community_report(results, data, None, data_dir, page)
}

fn community_search(
    data_dir: &Path,
    query: &str,
    controller: Option<&str>,
    page: usize,
) -> Result<SteamInputCommunityReport, String> {
    let page = page.max(1).min(1000);
    let value = fetch_community_value(&community_search_url(query, controller, page))?;
    parse_community_search_value(&value, data_dir, page)
}

/// Recherche les configurations publiées dans le catalogue communautaire
/// Steam. Aucun fichier Steam n'est modifié ici : la liste est seulement lue.
pub fn community(
    data_dir: &Path,
    appid: u32,
    query: &str,
    controller: Option<&str>,
    page: usize,
) -> Result<SteamInputCommunityReport, String> {
    if appid == 0 {
        return Err(t("steaminput.err_no_appid").into());
    }
    let page = page.max(1).min(1000);
    let value = fetch_community_value(&community_url(appid, query, controller, page))?;
    let report = parse_community_value(&value, appid, data_dir, page)?;
    // Le filtre par manette peut vider un jeu qui a des configurations pour
    // d'autres manettes : dans ce cas aussi, le repli transversal est utile.
    if report.profiles.is_empty() {
        let controller = controller.map(str::trim).filter(|kind| !kind.is_empty());
        if let Some(controller) = controller {
            if let Ok(mut other) = community_search(data_dir, query, Some(controller), page) {
                if !other.profiles.is_empty() {
                    other.fallback = true;
                    other.notes = vec![t("steaminput.note_other_games").into()];
                    return Ok(other);
                }
            }
        }
    }
    Ok(report)
}

/// Télécharge une configuration communautaire dans le dossier de données de
/// Lapinux, puis l'applique avec le même chemin que les profils locaux.
pub fn apply_community(
    roots: &[PathBuf],
    libraries: &[Library],
    devices: &[DeviceInfo],
    appid: u32,
    data_dir: &Path,
    profile: &SteamInputCommunityProfile,
) -> Result<SteamInputReport, String> {
    if appid == 0 {
        return Err(t("steaminput.err_no_appid").into());
    }
    if profile.id == 0 {
        return Err(t("steaminput.err_community_no_id").into());
    }
    // Une configuration d'un autre jeu est acceptée : SteamInputDB-Buddy
    // permet ce transfert, et le `controller_type` du fichier est vérifié
    // juste après le téléchargement.
    let cached = community_cache_path(data_dir, profile.id);
    let text = if cached.is_file() {
        fs::read_to_string(&cached)
            .map_err(|error| t_args("steaminput.err_local_copy", &[("error", &error.to_string())]))?
    } else {
        if !is_steam_cdn_url(&profile.download_url) {
            return Err(t("steaminput.err_no_download").into());
        }
        let text = crate::tools::fetch_text(&profile.download_url)
            .map_err(|error| t_args("steaminput.err_download", &[("error", &error.to_string())]))?;
        if text.len() > COMMUNITY_MAX_FILE {
            return Err(t("steaminput.err_file_too_large").into());
        }
        write_atomic(&cached, &text)?;
        text
    };
    let root = vdf::parse_root(&text)
        .map_err(|error| t_args("steaminput.err_community_unreadable", &[("error", &error.to_string())]))?;
    let mapping = mapping_of(&root);
    let controller_type = mapping.str("controller_type").unwrap_or("").trim();
    if controller_type.is_empty() {
        return Err(t("steaminput.err_community_not_controller").into());
    }
    if !profile.controller_type.trim().is_empty()
        && profile.controller_type.trim() != controller_type
    {
        return Err(t_args(
            "steaminput.err_wrong_controller",
            &[
                ("expected", steam_input_label(controller_type)),
                ("actual", steam_input_label(profile.controller_type.trim())),
            ],
        ));
    }
    let mut report = apply_lapinux(roots, libraries, devices, appid, data_dir, &cached)?;
    // La configuration est déjà appliquée : une copie Lapinux impossible ne
    // doit pas faire croire que Steam n'a rien reçu. Le rapport porte la note.
    if let Err(error) = keep_community_copy(data_dir, appid, profile, &text) {
        report.notes.push(t_args(
            "steaminput.note_copy_failed",
            &[("error", &error.to_string())],
        ));
    }
    Ok(report)
}

/// Profils proposés pour les manettes branchées, le jeu ouvert d'abord.
pub fn report(
    roots: &[PathBuf],
    libraries: &[Library],
    devices: &[DeviceInfo],
    appid: u32,
) -> SteamInputReport {
    let mut report = SteamInputReport {
        controllers: controllers(devices),
        ..Default::default()
    };
    let scan_roots = profile_roots(roots, libraries);
    report.available = !scan_roots.is_empty();
    if !report.available {
        report.notes.push(
            t("steaminput.note_no_local").into(),
        );
        return report;
    }

    let mut files = Vec::new();
    for root in &scan_roots {
        collect(root, 0, &mut files);
    }
    let workshop_appids = workshop_appids(&files);
    let config_dirs = configset_dirs(roots, libraries);
    let selections: Vec<(PathBuf, HashMap<String, Selection>)> = config_dirs
        .iter()
        .map(|dir| (dir.clone(), selections_for(dir, appid)))
        .collect();
    let mut seen = HashSet::new();
    for file in files {
        let Ok(canon) = std::fs::canonicalize(&file) else {
            continue;
        };
        if !seen.insert(canon) {
            continue;
        }
        // Un profil téléchargé porte l'identifiant de son contenu d'atelier :
        // les index `configset_*` disent à quel jeu il appartient.
        let profile_appid = workshop_id_of(&file)
            .and_then(|id| workshop_appids.get(&id).copied())
            .unwrap_or_else(|| appid_of(&file));
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        if let Some(mut profile) = profile_from_text(&file, profile_appid, &text) {
            if let Ok(root) = vdf::parse_root(&text) {
                let mapping = mapping_of(&root);
                profile.active = selections
                    .iter()
                    .any(|(dir, selections)| active_for(&file, mapping, appid, dir, selections));
            }
            // Les actions ne sont chargées que pour le jeu ouvert et le profil
            // actif : le rapport peut parcourir des centaines de fichiers, et
            // envoyer tous leurs bindings à l'interface serait inutilement
            // lourd. Les autres profils restent sélectionnables, leur détail
            // est signalé comme non chargé.
            if profile.active || profile.appid == appid {
                match crate::steam_mapping::profile_details(&text) {
                    Ok(details) => {
                        profile.preset = details.preset;
                        profile.action_groups = details.groups;
                        profile.settings = details.settings;
                        profile.details_loaded = true;
                    }
                    Err(error) => profile.details_error = error,
                }
            }
            report.profiles.push(profile);
        }
    }

    // Le jeu ouvert d'abord, puis les profils de la manette branchée : c'est
    // ce que la fiche met en avant. Le reste demeure consultable.
    let kinds: Vec<&str> = report
        .controllers
        .iter()
        .map(|controller| controller.kind.as_str())
        .collect();
    report.profiles.sort_by(|left, right| {
        let left_game = left.appid == appid;
        let right_game = right.appid == appid;
        let left_pad = kinds.contains(&left.controller_type.as_str());
        let right_pad = kinds.contains(&right.controller_type.as_str());
        right_game
            .cmp(&left_game)
            .then_with(|| right_pad.cmp(&left_pad))
            .then_with(|| left.controller_type.cmp(&right.controller_type))
            .then_with(|| left.title.to_lowercase().cmp(&right.title.to_lowercase()))
    });

    if report.profiles.is_empty() {
        report
            .notes
            .push(lapinux_core::i18n::t("steaminput.no_profile_account").into());
    }
    if report.controllers.is_empty() {
        report
            .notes
            .push(t("steaminput.note_no_controller").into());
    }
    report
}

/// Chemin d'une sélection propre à Lapinux, isolée par jeu et type de manette.
fn lapinux_selection_path(data_dir: &Path, appid: u32, kind: &str) -> Option<PathBuf> {
    if appid == 0
        || !kind.starts_with("controller_")
        || !kind
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    {
        return None;
    }
    Some(
        data_dir
            .join("steaminput")
            .join("active")
            .join(appid.to_string())
            .join(format!("{kind}.vdf")),
    )
}

/// Sélectionne une copie locale sans modifier les fichiers du client Steam.
pub fn apply_lapinux(
    roots: &[PathBuf],
    libraries: &[Library],
    devices: &[DeviceInfo],
    appid: u32,
    data_dir: &Path,
    source: &Path,
) -> Result<SteamInputReport, String> {
    let text = fs::read_to_string(source).map_err(|error| t_args("steaminput.err_profile_unreadable", &[("error", &error.to_string())]))?;
    let root = vdf::parse_root(&text).map_err(|error| t_args("steaminput.err_profile_unreadable", &[("error", &error.to_string())]))?;
    let kind = mapping_of(&root)
        .str("controller_type")
        .unwrap_or("")
        .trim();
    let destination = lapinux_selection_path(data_dir, appid, kind)
        .ok_or_else(|| t("steaminput.err_invalid_target").to_string())?;
    write_atomic(&destination, &text)?;
    Ok(report_lapinux(roots, libraries, devices, appid, data_dir))
}

/// Fusionne les profils existants et les sélections Lapinux, prioritaires par type.
pub fn report_lapinux(
    roots: &[PathBuf],
    libraries: &[Library],
    devices: &[DeviceInfo],
    appid: u32,
    data_dir: &Path,
) -> SteamInputReport {
    let mut result = report(roots, libraries, devices, appid);
    let dir = data_dir
        .join("steaminput")
        .join("active")
        .join(appid.to_string());
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("vdf") {
                continue;
            }
            let Some(mut selected) = profile(&path, appid) else {
                continue;
            };
            if lapinux_selection_path(data_dir, appid, &selected.controller_type).as_deref()
                != Some(path.as_path())
            {
                continue;
            }
            for other in &mut result.profiles {
                if other.controller_type == selected.controller_type {
                    other.active = false;
                }
            }
            selected.active = true;
            selected.origin = "Lapinux".into();
            result.profiles.push(selected);
            result.available = true;
        }
    }
    if result.available {
        result.notes.retain(|note| {
            !note.starts_with(lapinux_core::i18n::t("steaminput.no_profile_local"))
                && !note.starts_with(lapinux_core::i18n::t("steaminput.no_profile_found"))
        });
    }
    result
}

/// Choisit de façon déterministe un profil actif compatible, en privilégiant
/// les sélections explicites de Lapinux sur les configurations importées.
pub fn active_lapinux_profile_path(
    roots: &[PathBuf],
    libraries: &[Library],
    devices: &[DeviceInfo],
    appid: u32,
    data_dir: &Path,
) -> Option<PathBuf> {
    let report = report_lapinux(roots, libraries, devices, appid, data_dir);
    report
        .profiles
        .iter()
        .filter(|profile| {
            profile.active
                && (report.controllers.is_empty()
                    || report
                        .controllers
                        .iter()
                        .any(|pad| pad.kind == profile.controller_type))
        })
        .min_by(|a, b| {
            (a.origin != "Lapinux", &a.controller_type, &a.path).cmp(&(
                b.origin != "Lapinux",
                &b.controller_type,
                &b.path,
            ))
        })
        .map(|profile| profile.path.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lapinux_core::model::DeviceLink;

    fn mapping(title: &str, kind: &str) -> String {
        format!(
            "\"controller_mappings\"\n{{\n\t\"title\"\t\t\"{title}\"\n\t\"description\"\t\"Essai\"\n\t\"controller_type\"\t\"{kind}\"\n\t\"creator\"\t\t\"7656119\"\n}}\n"
        )
    }

    fn detailed_mapping(title: &str, kind: &str) -> String {
        format!(
            r#"
            "controller_mappings"
            {{
                "title" "{title}"
                "description" "Essai détaillé"
                "controller_type" "{kind}"
                "creator" "7656119"
                "preset"
                {{
                    "id" "0"
                    "name" "Default"
                    "group_source_bindings"
                    {{
                        "0" "button_diamond active"
                        "7" "switch active"
                    }}
                }}
                "group"
                {{
                    "id" "0"
                    "mode" "four_buttons"
                    "inputs"
                    {{
                        "button_a" {{ "activators" {{ "Full_Press" {{ "bindings" {{ "binding" "xinput_button A, , " }} }} }} }}
                    }}
                }}
                "group"
                {{
                    "id" "7"
                    "mode" "switches"
                    "inputs"
                    {{
                        "button_back_left" {{ "activators" {{ "Full_Press" {{ "bindings" {{ "binding" "xinput_button B, , " }} }} }} }}
                    }}
                }}
                "settings" {{ "left_stick_deadzone" "0.15" }}
            }}
            "#
        )
    }

    fn pad(name: &str, vendor: u16, product: u16) -> DeviceInfo {
        DeviceInfo {
            path: PathBuf::new(),
            name: name.into(),
            vendor,
            product,
            gamepad: true,
            buttons: 12,
            button_names: Vec::new(),
            axes: Vec::new(),
            force_feedback: true,
            touchpad: false,
            connection: DeviceLink::Usb,
            battery: None,
            battery_status: None,
            address: None,
            connected: true,
            steam_bridge: false,
            udev: Default::default(),
        }
    }

    #[test]
    fn les_identifiants_materiels_donnent_le_type_steam() {
        assert_eq!(controller_type(0x28de, 0x1304), "controller_triton");
        assert_eq!(controller_type(0x054c, 0x0ce6), "controller_ps5");
        assert_eq!(controller_type(0x054c, 0x0df2), "controller_ps5_edge");
        assert_eq!(controller_type(0x054c, 0x05c4), "controller_ps4");
        assert_eq!(controller_type(0x28de, 0x1205), "controller_neptune");
        assert_eq!(controller_type(0x045e, 0x0b13), "controller_xboxone");
        assert_eq!(controller_type(0x045e, 0x028e), "controller_xbox360");
        assert_eq!(controller_type(0x057e, 0x2009), "controller_switch_pro");
        assert_eq!(controller_type(0x1234, 0x5678), "controller_generic");
    }

    #[test]
    fn la_manette_native_est_identifiee_sans_pont() {
        for product in 0x1302..=0x1305 {
            let found = controllers(&[pad("Steam Controller", 0x28de, product)]);
            assert_eq!(found.len(), 1);
            assert_eq!(found[0].kind, "controller_triton");
            assert_eq!(found[0].label, "Steam Controller 2026");
        }
    }

    #[test]
    fn le_pont_du_puck_reste_une_steam_controller_triton() {
        let mut device = pad("Microsoft X-Box 360 pad", 0x045e, 0x028e);
        device.steam_bridge = true;
        let found = controllers(&[device]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, "controller_triton");
        assert_eq!(found[0].label, "Steam Controller 2026");
    }

    #[test]
    fn un_profil_lit_son_titre_son_type_et_son_appid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("Steam Controller Configs/42/config/620/ps5.vdf");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, mapping("Mon layout", "controller_ps5")).unwrap();
        let profile = profile(&path, appid_of(&path)).expect("profil lu");
        assert_eq!(profile.title, "Mon layout");
        assert_eq!(profile.controller_type, "controller_ps5");
        assert_eq!(profile.appid, 620);
        assert_eq!(profile.creator, "7656119");
        assert_eq!(profile.origin, "jeu");
    }

    #[test]
    fn un_profil_d_atelier_reprend_l_appid_du_configset() {
        let dir = tempfile::tempdir().unwrap();
        let index = dir
            .path()
            .join("Steam Controller Configs/42/config/configset_controller_ps4.vdf");
        std::fs::create_dir_all(index.parent().unwrap()).unwrap();
        std::fs::write(
            &index,
            "\"controller_config\"\n{\n\t\"443510\"\n\t{\n\t\t\"workshop\"\t\t\"1481807135\"\n\t}\n}\n",
        )
        .unwrap();
        let remote = dir
            .path()
            .join("userdata/42/241100/remote/1481807135_controller_config.vdf");
        std::fs::create_dir_all(remote.parent().unwrap()).unwrap();
        std::fs::write(&remote, mapping("BLACK OPS2", "controller_ps4")).unwrap();
        let files = vec![index, remote.clone()];
        let map = workshop_appids(&files);
        assert_eq!(map.get(&1481807135), Some(&443510));
        let appid = workshop_id_of(&remote)
            .and_then(|id| map.get(&id).copied())
            .unwrap_or(0);
        assert_eq!(appid, 443510);
        let profile = profile(&remote, appid).expect("profil lu");
        assert_eq!(profile.appid, 443510);
    }

    #[test]
    fn le_rapport_garde_les_profils_de_la_manette_branchee() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir
            .path()
            .join("steamapps/common/Steam Controller Configs/42/config");
        for (appid, kind) in [(620u32, "controller_ps5"), (4000, "controller_xboxone")] {
            let path = base.join(appid.to_string()).join("profil.vdf");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, mapping("Layout", kind)).unwrap();
        }
        let library = Library::new(dir.path());
        let devices = vec![pad("DualSense", 0x054c, 0x0ce6)];
        let report = report(
            &[dir.path().to_path_buf()],
            std::slice::from_ref(&library),
            &devices,
            620,
        );
        assert_eq!(report.controllers.len(), 1);
        assert_eq!(report.profiles.len(), 2);
        // Le profil du jeu ouvert passe devant celui de la manette, puis le
        // filtre ne garde que les types branchés.
        let offered = report.for_connected();
        assert_eq!(offered.len(), 1);
        assert_eq!(offered[0].controller_type, "controller_ps5");
    }

    #[test]
    fn le_rapport_charge_les_actions_du_jeu_et_pas_celles_des_autres_jeux() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir
            .path()
            .join("steamapps/common/Steam Controller Configs/42/config");
        let current = base.join("620/controller_ps5.vdf");
        let other = base.join("4000/controller_ps5.vdf");
        std::fs::create_dir_all(current.parent().unwrap()).unwrap();
        std::fs::create_dir_all(other.parent().unwrap()).unwrap();
        std::fs::write(&current, detailed_mapping("Portal", "controller_ps5")).unwrap();
        std::fs::write(&other, detailed_mapping("Autre jeu", "controller_ps5")).unwrap();
        let library = Library::new(dir.path());
        let report = report(
            &[dir.path().to_path_buf()],
            std::slice::from_ref(&library),
            &[pad("DualSense", 0x054c, 0x0ce6)],
            620,
        );
        let profile = report
            .profiles
            .iter()
            .find(|profile| profile.path == current)
            .expect("profil du jeu");
        assert!(profile.details_loaded);
        assert_eq!(profile.preset, "Default");
        assert!(profile.action_groups.iter().any(|group| {
            group.source == "button_diamond"
                && group
                    .actions
                    .iter()
                    .any(|action| action.binding.contains("xinput_button A"))
        }));
        assert!(profile
            .settings
            .iter()
            .any(|setting| setting.key == "left_stick_deadzone" && setting.value == "0.15"));
        let other = report
            .profiles
            .iter()
            .find(|profile| profile.path == other)
            .expect("profil d'un autre jeu");
        assert!(
            !other.details_loaded,
            "le détail des autres jeux n'est pas chargé"
        );
    }

    #[test]
    fn une_copie_conservee_expose_ses_actions_sans_relire_steam() {
        let data = tempfile::tempdir().unwrap();
        let appid = 620u32;
        let id = 42u64;
        let dir = saved_dir(data.path()).join(appid.to_string());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("community-{id}.vdf")),
            detailed_mapping("Copie Portal", "controller_ps5"),
        )
        .unwrap();
        std::fs::write(
            dir.join(format!("community-{id}.json")),
            serde_json::json!({
                "id": id,
                "appid": appid,
                "source_appid": appid,
                "title": "Copie Portal",
                "description": "Essai",
                "controller_type": "controller_ps5",
                "creator": "7656119",
                "modified": 1_700_000_000,
                "saved_at": 1_700_000_100
            })
            .to_string(),
        )
        .unwrap();
        let saved = saved(data.path());
        assert_eq!(saved.len(), 1);
        assert!(saved[0].details_loaded);
        assert_eq!(saved[0].preset, "Default");
        assert_eq!(
            saved[0]
                .action_groups
                .iter()
                .map(|group| group.actions.len())
                .sum::<usize>(),
            2
        );
    }

    #[test]
    fn une_selection_atelier_est_ecrite_et_relue() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir
            .path()
            .join("steamapps/common/Steam Controller Configs/42/config");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(
            base.join("configset_controller_ps5.vdf"),
            "\"controller_config\"\n{\n}\n",
        )
        .unwrap();
        std::fs::write(
            base.join("configset_54c-ce6-1298a662.vdf"),
            "\"controller_config\"\n{\n}\n",
        )
        .unwrap();
        let remote = dir
            .path()
            .join("userdata/42/241100/remote/1481807135_controller_config.vdf");
        std::fs::create_dir_all(remote.parent().unwrap()).unwrap();
        std::fs::write(&remote, mapping("BLACK OPS2", "controller_ps5")).unwrap();
        let library = Library::new(dir.path());
        let devices = vec![pad("DualSense", 0x054c, 0x0ce6)];
        let report = apply(
            &[dir.path().to_path_buf()],
            std::slice::from_ref(&library),
            &devices,
            620,
            &remote,
        )
        .expect("profil appliqué");
        for path in [
            base.join("configset_controller_ps5.vdf"),
            base.join("configset_54c-ce6-1298a662.vdf"),
        ] {
            let text = std::fs::read_to_string(&path).unwrap();
            assert!(text.contains("\"620\""), "{text}");
            assert!(text.contains("\"workshop\"\t\t\"1481807135\""), "{text}");
        }
        let applied = report
            .profiles
            .iter()
            .find(|profile| profile.path == remote)
            .expect("profil relu");
        assert!(applied.active);
    }

    #[test]
    fn appliquer_preserve_les_autres_entrees_du_configset() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir
            .path()
            .join("steamapps/common/Steam Controller Configs/42/config");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(
            base.join("configset_controller_ps5.vdf"),
            "\"controller_config\"\n{\n\t\"a short hike\"\n\t{\n\t\t\"template\"\t\t\"controller_ps3_gamepad_joystick.vdf\"\n\t}\n\t\"620\"\n\t{\n\t\t\"template\"\t\t\"controller_ps5_gamepad.vdf\"\n\t}\n}\n",
        )
        .unwrap();
        let remote = dir
            .path()
            .join("userdata/42/241100/remote/1481807135_controller_config.vdf");
        std::fs::create_dir_all(remote.parent().unwrap()).unwrap();
        std::fs::write(&remote, mapping("BLACK OPS2", "controller_ps5")).unwrap();
        let library = Library::new(dir.path());
        let devices = vec![pad("DualSense", 0x054c, 0x0ce6)];
        apply(
            &[dir.path().to_path_buf()],
            std::slice::from_ref(&library),
            &devices,
            620,
            &remote,
        )
        .expect("profil appliqué");
        let path = base.join("configset_controller_ps5.vdf");
        let root = vdf::parse_root(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            root.obj("a short hike").unwrap().str("template"),
            Some("controller_ps3_gamepad_joystick.vdf")
        );
        let selected = root.obj("620").expect("entrée du jeu");
        assert_eq!(selected.str("workshop"), Some("1481807135"));
        assert_eq!(selected.str("template"), None);
    }

    #[test]
    fn une_selection_locale_copie_l_autosave() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir
            .path()
            .join("steamapps/common/Steam Controller Configs/42/config");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(
            base.join("configset_controller_ps5.vdf"),
            "\"controller_config\"\n{\n}\n",
        )
        .unwrap();
        let source = base.join("443510/controller_ps5.vdf");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(&source, mapping("Mon layout", "controller_ps5")).unwrap();
        let library = Library::new(dir.path());
        let devices = vec![pad("DualSense", 0x054c, 0x0ce6)];
        let report = apply(
            &[dir.path().to_path_buf()],
            std::slice::from_ref(&library),
            &devices,
            620,
            &source,
        )
        .expect("profil appliqué");
        let destination = base.join("620/controller_ps5.vdf");
        assert_eq!(
            std::fs::read_to_string(&destination).unwrap(),
            std::fs::read_to_string(&source).unwrap()
        );
        let text = std::fs::read_to_string(base.join("configset_controller_ps5.vdf")).unwrap();
        assert!(text.contains("\"autosave\"\t\t\"1\""), "{text}");
        let applied = report
            .profiles
            .iter()
            .find(|profile| profile.path == destination)
            .expect("copie relue");
        assert!(applied.active);
    }

    #[test]
    fn le_catalogue_communautaire_lit_les_references_sveltekit() {
        // Structure réelle de `__data.json` : le nœud de page porte
        // `{ appInfo, configs }`, puis toutes les valeurs sont des références
        // vers le tableau plat. Les index sont volontairement dans l'ordre
        // exact où SteamInputDB les publie.
        let value = serde_json::json!({
            "nodes": [{
                "type": "data",
                "data": [
                    { "appInfo": 1, "configs": 2 },
                    { "app_id": 3 },
                    { "total": 4, "items": 5 },
                    620,
                    1,
                    [6],
                    {
                        "title": 7,
                        "description": 8,
                        "app_id": 3,
                        "file_id": 9,
                        "file_url": 10,
                        "file_size": 11,
                        "creator_id": 12,
                        "controller_type": 13,
                        "time_updated": 14,
                        "subscriptions": 15,
                        "votes": 16
                    },
                    "Portal 2: Dualshock4 controller",
                    "Gyro et gâchettes",
                    791932662u64,
                    "https://cdn.steamusercontent.com/ugc/397834284244938964/01DF0758BFB02CDFC4D21C244D01BE0A363A1946/",
                    12949,
                    "76561198000000000",
                    "controller_ps4",
                    "2022-11-09T16:29:03Z",
                    20521,
                    { "score": 17, "up": 18, "down": 19 },
                    0.94,
                    120,
                    8
                ]
            }]
        });
        let dir = tempfile::tempdir().unwrap();
        let report = parse_community_value(&value, 620, dir.path(), 2).expect("catalogue lu");
        assert_eq!(report.total, 1);
        assert_eq!(report.page, 2);
        assert_eq!(report.per_page, COMMUNITY_PAGE_SIZE);
        let profile = report.profiles.first().expect("configuration");
        assert_eq!(profile.id, 791932662);
        assert_eq!(profile.appid, 620);
        assert_eq!(profile.title, "Portal 2: Dualshock4 controller");
        assert_eq!(profile.controller_type, "controller_ps4");
        assert_eq!(profile.file_size, 12949);
        assert!((profile.rating - 0.94).abs() < 0.001);
        assert_eq!(profile.votes_up, 120);
        assert_eq!(profile.votes_down, 8);
        assert!(profile.modified > 1_600_000_000, "date ISO convertie");
        assert!(!profile.installed);
    }

    #[test]
    fn l_url_du_catalogue_encode_la_recherche_et_la_manette() {
        assert_eq!(
            community_url(620, "Portal 2 & gyro", Some("controller_ps4"), 2),
            "https://www.steaminputdb.com/app/620/__data.json?sort-by=vote&page=2&searchtext=Portal%202%20%26%20gyro&controller_type=controller_ps4"
        );
        assert_eq!(iso_to_unix("1970-01-01T00:00:00Z"), 0);
    }

    #[test]
    fn une_configuration_communautaire_en_cache_est_appliquee_sans_reseau() {
        let dir = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let base = dir
            .path()
            .join("steamapps/common/Steam Controller Configs/42/config");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(
            base.join("configset_controller_ps5.vdf"),
            "\"controller_config\"\n{\n}\n",
        )
        .unwrap();
        let mut profile = SteamInputCommunityProfile {
            id: 42,
            appid: 620,
            title: "Portal 2 communautaire".into(),
            controller_type: "controller_ps5".into(),
            installed: true,
            ..Default::default()
        };
        let cached = community_cache_path(data.path(), profile.id);
        std::fs::create_dir_all(cached.parent().unwrap()).unwrap();
        std::fs::write(&cached, mapping("Portal 2 communautaire", "controller_ps5")).unwrap();
        let library = Library::new(dir.path());
        let devices = vec![pad("DualSense", 0x054c, 0x0ce6)];
        let report = apply_community(
            &[dir.path().to_path_buf()],
            std::slice::from_ref(&library),
            &devices,
            620,
            data.path(),
            &profile,
        )
        .expect("configuration communautaire appliquée sans réseau");
        let destination = lapinux_selection_path(data.path(), 620, "controller_ps5").unwrap();
        assert_eq!(
            std::fs::read_to_string(&destination).unwrap(),
            std::fs::read_to_string(&cached).unwrap()
        );
        let text = std::fs::read_to_string(base.join("configset_controller_ps5.vdf")).unwrap();
        assert!(
            !text.contains("\"620\""),
            "Steam ne doit pas être modifié : {text}"
        );
        assert!(
            report
                .profiles
                .iter()
                .any(|found| found.path == destination && found.active),
            "la copie locale doit être relue comme active"
        );
        let kept = saved(data.path());
        assert_eq!(kept.len(), 1, "la configuration doit rester dans Lapinux");
        assert_eq!(kept[0].appid, 620);
        assert_eq!(kept[0].source_appid, 620);
        assert_eq!(kept[0].title, "Portal 2 communautaire");
        assert_eq!(kept[0].controller_type, "controller_ps5");
        assert!(kept[0].path.is_file());
        assert_eq!(
            std::fs::read_to_string(&kept[0].path).unwrap(),
            std::fs::read_to_string(&cached).unwrap()
        );

        profile.controller_type = "controller_ps4".into();
        let error = apply_community(
            &[dir.path().to_path_buf()],
            std::slice::from_ref(&library),
            &devices,
            620,
            data.path(),
            &profile,
        )
        .expect_err("un type de manette différent doit être refusé");
        assert!(error.contains("prévue pour"), "{error}");
    }

    #[test]
    fn selection_lapinux_sans_client_steam_et_par_manette() {
        let data = tempfile::tempdir().unwrap();
        let source = data.path().join("profil.vdf");
        std::fs::write(&source, mapping("DualSense", "controller_ps5")).unwrap();
        let devices = vec![pad("DualSense", 0x054c, 0x0ce6)];
        let report = apply_lapinux(&[], &[], &devices, 620, data.path(), &source).unwrap();
        assert!(report.available);
        assert!(report
            .profiles
            .iter()
            .any(|p| p.active && p.title == "DualSense" && p.origin == "Lapinux"));
        let reloaded = report_lapinux(&[], &[], &devices, 620, data.path());
        assert_eq!(reloaded.profiles.iter().filter(|p| p.active).count(), 1);
        assert_eq!(
            active_lapinux_profile_path(&[], &[], &devices, 620, data.path()),
            lapinux_selection_path(data.path(), 620, "controller_ps5")
        );
        assert!(report_lapinux(&[], &[], &devices, 621, data.path())
            .profiles
            .is_empty());
        std::fs::write(&source, mapping("Xbox", "controller_xboxone")).unwrap();
        let report = apply_lapinux(&[], &[], &devices, 620, data.path(), &source).unwrap();
        assert_eq!(report.profiles.iter().filter(|p| p.active).count(), 2);
        assert_eq!(
            active_lapinux_profile_path(&[], &[], &devices, 620, data.path()),
            lapinux_selection_path(data.path(), 620, "controller_ps5")
        );
        std::fs::write(&source, mapping("mauvais", "controller_../../etc")).unwrap();
        assert!(apply_lapinux(&[], &[], &devices, 620, data.path(), &source).is_err());
    }

    /// Vérification manuelle du vrai catalogue : `cargo test -p lapinux-steam
    /// le_catalogue_public_reel -- --ignored`.
    #[test]
    #[ignore = "accès réseau au catalogue public SteamInputDB"]
    fn le_catalogue_public_reel_se_parse() {
        let dir = tempfile::tempdir().unwrap();
        let report = community(dir.path(), 620, "", Some("controller_ps4"), 1)
            .expect("le catalogue public doit répondre");
        assert!(report.total > 0);
        assert!(!report.profiles.is_empty());
        assert!(report.profiles.iter().all(|profile| profile.appid == 620));
        assert!(report
            .profiles
            .iter()
            .all(|profile| profile.controller_type == "controller_ps4"));
        assert!(report
            .profiles
            .iter()
            .any(|profile| profile.id == 791932662));
    }

    /// Vérifie le chemin complet catalogue → CDN → cache → configset, sans
    /// jamais toucher aux dossiers Steam de l'utilisateur.
    #[test]
    #[ignore = "téléchargement réel depuis le CDN Steam"]
    fn une_configuration_publique_reelle_est_telechargee_et_appliquee() {
        let data = tempfile::tempdir().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let base = dir
            .path()
            .join("steamapps/common/Steam Controller Configs/42/config");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(
            base.join("configset_controller_ps4.vdf"),
            "\"controller_config\"\n{\n}\n",
        )
        .unwrap();
        let catalog = community(data.path(), 620, "", Some("controller_ps4"), 1)
            .expect("le catalogue public doit répondre");
        let profile = catalog
            .profiles
            .iter()
            .find(|profile| profile.id == 791932662)
            .expect("configuration de référence du catalogue");
        let library = Library::new(dir.path());
        let report = apply_community(
            &[dir.path().to_path_buf()],
            std::slice::from_ref(&library),
            &[],
            620,
            data.path(),
            profile,
        )
        .expect("téléchargement et application depuis le CDN Steam");
        assert!(community_cache_path(data.path(), profile.id).is_file());
        let destination = lapinux_selection_path(data.path(), 620, "controller_ps4").unwrap();
        assert!(destination.is_file());
        let text = std::fs::read_to_string(base.join("configset_controller_ps4.vdf")).unwrap();
        assert!(
            !text.contains("\"620\""),
            "Steam ne doit pas être modifié : {text}"
        );
        assert!(
            report
                .profiles
                .iter()
                .any(|found| found.path == destination && found.active),
            "la configuration téléchargée doit être active"
        );
    }

    /// Diagnostic manuel du catalogue d'un jeu : `cargo test -p
    /// lapinux-steam motorslice -- --ignored --nocapture`.
    #[test]
    #[ignore = "accès réseau au catalogue public SteamInputDB"]
    fn motorslice_diagnostic_par_manette() {
        let dir = tempfile::tempdir().unwrap();
        for controller in [None, Some("controller_triton")] {
            let report = community(dir.path(), 2_830_030, "", controller, 1)
                .expect("le catalogue public doit répondre");
            eprintln!(
                "MOTORSLICE manette={controller:?} total={} profils={} fallback={} types={:?}",
                report.total,
                report.profiles.len(),
                report.fallback,
                report
                    .profiles
                    .iter()
                    .take(10)
                    .map(|profile| profile.controller_type.as_str())
                    .collect::<Vec<_>>()
            );
        }
    }

    /// Pizza Possum n'a aucune configuration communautaire propre : la vue
    /// doit basculer sur les configurations populaires de la manette, sans
    /// jamais les présenter comme des configurations du jeu.
    #[test]
    #[ignore = "accès réseau au catalogue public SteamInputDB"]
    fn pizza_possum_recoit_des_configurations_d_autres_jeux() {
        let dir = tempfile::tempdir().unwrap();
        let report = community(dir.path(), 1951230, "", Some("controller_triton"), 1)
            .expect("le catalogue public doit répondre");
        assert!(report.fallback, "le rapport doit signaler le repli");
        assert!(!report.profiles.is_empty());
        assert!(report.total > 0);
        assert!(report
            .profiles
            .iter()
            .all(|profile| profile.appid != 1951230));
        assert!(report
            .profiles
            .iter()
            .all(|profile| profile.controller_type == "controller_triton"));
    }
}
