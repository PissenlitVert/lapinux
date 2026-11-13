//! Atelier Steam : abonnements installés, poids et fichiers réels dans
//! `<LapinuxLibrary>/Mods/<jeu> (<appid>)/SteamWorkshop`.
//!
//! Les fichiers de Steam ne sont jamais déplacés : le dossier `Mods` contient
//! une arborescence de vrais fichiers, créée par liens physiques quand le
//! système le permet et par copie sinon. Un rangement suivant retire les
//! entrées qu'il a posées et qui ne correspondent plus à un abonnement, jamais
//! un fichier ou un dossier de l'utilisateur.

use lapinux_core::i18n::{t, t_args};
use crate::{depot, session, vdf, Library};
use lapinux_core::model::{
    WorkshopArrangeReport, WorkshopDownloadReport, WorkshopItem, WorkshopManageReport,
    WorkshopReport,
};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

/// Métadonnées publiques des contenus : Steam n'écrit pas les titres dans le
/// manifeste local, ils ne se lisent que par cette API.
const DETAILS_URL: &str =
    "https://api.steampowered.com/ISteamRemoteStorage/GetPublishedFileDetails/v1/";
const DETAILS_TIMEOUT: Duration = Duration::from_secs(6);
/// Un téléchargement d'atelier peut être long ; chaque contenu est toutefois
/// borné pour ne pas bloquer le daemon indéfiniment.
const WORKSHOP_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// Plafond de parcours pour un contenu sans manifeste.
const MAX_FILES: usize = 20_000;
/// Marqueur caché des dossiers matérialisés : il permet de reconnaître ce que
/// Lapinux a créé, sans jamais confondre un dossier de l'utilisateur avec un
/// rangement géré.
const MATERIALIZED_MARKER: &str = ".lapinux-workshop";

/// Page de l'atelier d'un jeu.
pub fn workshop_url(appid: u32) -> String {
    format!("https://steamcommunity.com/app/{appid}/workshop/")
}

/// Page d'un contenu de l'atelier.
pub fn item_url(id: u64) -> String {
    format!("https://steamcommunity.com/sharedfiles/filedetails/?id={id}")
}

/// Nom de dossier ou de lien acceptable : pas de séparateur, pas de caractère
/// de contrôle, une longueur raisonnable.
fn safe_name(name: &str, fallback: &str) -> String {
    let mut out = String::new();
    for ch in name.trim().chars() {
        if out.chars().count() >= 80 {
            break;
        }
        if ch == '/' || ch == '\\' || ch == ':' || ch.is_control() {
            out.push('-');
        } else {
            out.push(ch);
        }
    }
    let out = out.trim().trim_matches('.').trim();
    if out.is_empty() {
        fallback.to_string()
    } else {
        out.to_string()
    }
}

/// Bibliothèque qui porte l'atelier du jeu : celle où Steam a écrit son
/// manifeste ou son contenu ; à défaut la première bibliothèque connue.
fn workshop_library(libraries: &[Library], appid: u32) -> Option<usize> {
    libraries
        .iter()
        .position(|library| {
            library
                .steamapps()
                .join("workshop")
                .join(format!("appworkshop_{appid}.acf"))
                .is_file()
                || library
                    .steamapps()
                    .join("workshop/content")
                    .join(appid.to_string())
                    .is_dir()
        })
        .or(if libraries.is_empty() { None } else { Some(0) })
}

/// Nom du dossier d'un jeu : le titre lisible et l'appid stable.
fn game_folder(appid: u32, name: &str) -> String {
    format!("{} ({appid})", safe_name(name, &format!("Jeu {appid}")))
}

/// Racine commune des rangements : la bibliothèque gérée par Lapinux
/// (`LapinuxLibrary`), sinon la première bibliothèque connue.
fn mods_root(libraries: &[Library]) -> PathBuf {
    libraries
        .iter()
        .find(|library| library.managed)
        .or_else(|| libraries.first())
        .map(|library| library.root.clone())
        .unwrap_or_default()
}

/// État local du rangement : les contenus désactivés restent téléchargés,
/// mais aucun lien actif ne les présente au jeu. Le fichier vit hors du
/// dossier `SteamWorkshop` pour que le jeu ne le voie jamais.
fn state_path(libraries: &[Library], appid: u32) -> Option<PathBuf> {
    let root = mods_root(libraries);
    if root.as_os_str().is_empty() {
        return None;
    }
    Some(
        root.join(".lapinux")
            .join("workshop")
            .join(format!("{appid}.json")),
    )
}

fn read_disabled(path: Option<&Path>) -> HashSet<u64> {
    let Some(path) = path else {
        return HashSet::new();
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return HashSet::new();
    };
    serde_json::from_str::<Vec<u64>>(&text)
        .map(|ids| ids.into_iter().collect())
        .unwrap_or_default()
}

fn write_disabled(path: &Path, disabled: &HashSet<u64>) -> crate::Result<()> {
    let mut ids: Vec<u64> = disabled.iter().copied().collect();
    ids.sort_unstable();
    let text = serde_json::to_vec_pretty(&ids)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, text)?;
    std::fs::rename(&temporary, path)?;
    Ok(())
}

/// Dossier final : `<LapinuxLibrary>/Mods/<jeu> (<appid>)/SteamWorkshop`.
fn mods_dir(libraries: &[Library], appid: u32, name: &str) -> PathBuf {
    mods_root(libraries)
        .join("Mods")
        .join(game_folder(appid, name))
        .join("SteamWorkshop")
}

/// Identifiant porté par un dossier matérialisé, ou `None` pour un dossier
/// qui n'appartient pas à Lapinux.
fn marker_id(dir: &Path) -> Option<u64> {
    let text = std::fs::read_to_string(dir.join(MATERIALIZED_MARKER)).ok()?;
    text.trim().parse().ok()
}

fn is_managed_dir(path: &Path) -> bool {
    path.is_dir() && marker_id(path).is_some()
}

/// Copie une arborescence en vrais fichiers. Un lien physique garde le même
/// contenu sans doubler l'espace lorsqu'il est possible ; sinon on copie.
fn materialize_tree(
    source: &Path,
    destination: &Path,
    depth: usize,
    files: &mut usize,
) -> std::io::Result<()> {
    if depth > 32 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            t("workshop.err_tree_deep"),
        ));
    }
    std::fs::create_dir_all(destination)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let path = entry.path();
        let target = destination.join(entry.file_name());
        if file_type.is_dir() {
            materialize_tree(&path, &target, depth + 1, files)?;
        } else if file_type.is_file() {
            *files += 1;
            if *files > MAX_FILES {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    t("workshop.err_too_many"),
                ));
            }
            if std::fs::hard_link(&path, &target).is_err() {
                std::fs::copy(&path, &target)?;
                if let Ok(meta) = std::fs::metadata(&path) {
                    let _ = std::fs::set_permissions(&target, meta.permissions());
                }
            }
        }
    }
    Ok(())
}

/// Remplace un ancien lien symbolique (ou un dossier déjà matérialisé) par
/// une arborescence de vrais fichiers. Le staging évite de laisser un dossier
/// à moitié rempli si la copie échoue.
fn materialize_entry(source: &Path, destination: &Path, id: u64) -> std::io::Result<()> {
    let parent = destination.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            t("workshop.err_dest_no_parent"),
        )
    })?;
    let staging = parent.join(format!(".lapinux-{id}.part"));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    let mut files = 0;
    let filled = materialize_tree(source, &staging, 0, &mut files)
        .and_then(|_| std::fs::write(staging.join(MATERIALIZED_MARKER), id.to_string()));
    if let Err(error) = filled {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(error);
    }

    match std::fs::symlink_metadata(destination) {
        Ok(meta) if meta.file_type().is_symlink() => std::fs::remove_file(destination)?,
        Ok(meta) if meta.is_dir() && marker_id(destination).is_some() => {
            std::fs::remove_dir_all(destination)?
        }
        Ok(_) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                t("workshop.err_name_taken"),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(error);
        }
    }

    if let Err(error) = std::fs::rename(&staging, destination) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(error);
    }
    Ok(())
}

fn remove_managed_entry(path: &Path) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if meta.file_type().is_symlink() {
        std::fs::remove_file(path).is_ok()
    } else if meta.is_dir() && is_managed_dir(path) {
        std::fs::remove_dir_all(path).is_ok()
    } else {
        false
    }
}

/// Ancien emplacement, juste sous `Mods/<jeu> (<appid>)`, pour migrer les
/// liens d'une version précédente sans toucher au contenu Steam.
fn legacy_mods_dir(library: &Library, appid: u32, name: &str) -> PathBuf {
    library.root.join("Mods").join(game_folder(appid, name))
}

/// Déplace uniquement les liens symboliques qui pointent vers le contenu de
/// cet atelier. Un fichier ou un dossier de l'utilisateur au même niveau est
/// laissé en place.
fn migrate_legacy_links(
    libraries: &[Library],
    appid: u32,
    name: &str,
    new_dir: &Path,
) -> std::io::Result<()> {
    let content_roots: Vec<PathBuf> = libraries
        .iter()
        .map(|library| {
            library
                .steamapps()
                .join("workshop/content")
                .join(appid.to_string())
        })
        .collect();
    for library in libraries {
        let legacy = legacy_mods_dir(library, appid, name);
        let Ok(entries) = std::fs::read_dir(&legacy) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if !meta.file_type().is_symlink() {
                continue;
            }
            let Ok(target) = std::fs::read_link(&path) else {
                continue;
            };
            if !content_roots.iter().any(|root| target.starts_with(root)) {
                continue;
            }
            let link = new_dir.join(entry.file_name());
            if let Ok(existing) = std::fs::symlink_metadata(&link) {
                if existing.file_type().is_symlink()
                    && std::fs::read_link(&link).ok().as_deref() == Some(target.as_path())
                {
                    let _ = std::fs::remove_file(&path);
                }
                continue;
            }
            std::os::unix::fs::symlink(&target, &link)?;
            std::fs::remove_file(&path)?;
        }
        if std::fs::read_dir(&legacy)
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(false)
        {
            let _ = std::fs::remove_dir(&legacy);
        }
    }
    Ok(())
}

#[derive(Default)]
struct Acf {
    items: Vec<AcfItem>,
}

struct AcfItem {
    id: u64,
    size: u64,
    time_updated: i64,
    manifest: String,
}

fn read_acf(path: &Path) -> Option<Acf> {
    let text = std::fs::read_to_string(path).ok()?;
    let root = vdf::parse_root(&text).ok()?;
    let installed = root.obj("WorkshopItemsInstalled")?;
    let details = root.obj("WorkshopItemDetails");
    let mut items = Vec::new();
    for (key, entry) in installed.objects() {
        let Ok(id) = key.parse::<u64>() else {
            continue;
        };
        let detail = details.and_then(|details| details.obj(key));
        let time_updated = entry
            .i64("timeupdated")
            .max(detail.map(|detail| detail.i64("timeupdated")).unwrap_or(0));
        let manifest = entry
            .str("manifest")
            .or_else(|| detail.and_then(|detail| detail.str("manifest")))
            .unwrap_or("")
            .to_string();
        items.push(AcfItem {
            id,
            size: entry.u64("size"),
            time_updated,
            manifest,
        });
    }
    items.sort_by(|left, right| {
        right
            .time_updated
            .cmp(&left.time_updated)
            .then(left.id.cmp(&right.id))
    });
    Some(Acf { items })
}

/// Compte Steam courant : l'identifiant de compte est la partie basse du
/// SteamID64. Le fichier de session est facultatif ; sans lui, le fichier
/// d'abonnements le plus récent est retenu.
fn current_account() -> Option<u32> {
    session::load()
        .map(|saved| (saved.steam_id & 0xFFFF_FFFF) as u32)
        .filter(|id| *id != 0)
}

/// Fichier des abonnements que Steam garde dans `userdata` même quand le jeu
/// n'est pas installé. Sans lui, une fiche de catalogue paraîtrait sans
/// atelier alors que le compte a des abonnements.
fn subscriptions_file(libraries: &[Library], appid: u32) -> Option<PathBuf> {
    let account = current_account();
    let mut roots: Vec<PathBuf> = libraries
        .iter()
        .map(|library| library.root.clone())
        .collect();
    roots.extend(lapinux_core::paths::steam_roots());
    roots.sort();
    roots.dedup();

    let mut newest: Option<(SystemTime, PathBuf)> = None;
    for root in roots {
        let userdata = root.join("userdata");
        if let Some(account) = account {
            let path = userdata
                .join(account.to_string())
                .join("ugc")
                .join(format!("{appid}_subscriptions.vdf"));
            if path.is_file() {
                return Some(path);
            }
            // Ne jamais lire les abonnements d'un autre compte présent sur
            // la machine quand la session dit lequel est le nôtre.
            continue;
        }
        let Ok(accounts) = std::fs::read_dir(&userdata) else {
            continue;
        };
        for account in accounts.flatten() {
            let path = account
                .path()
                .join("ugc")
                .join(format!("{appid}_subscriptions.vdf"));
            if !path.is_file() {
                continue;
            }
            let modified = std::fs::metadata(&path)
                .and_then(|meta| meta.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            let newer = newest
                .as_ref()
                .map(|(seen, _)| modified > *seen)
                .unwrap_or(true);
            if newer {
                newest = Some((modified, path));
            }
        }
    }
    newest.map(|(_, path)| path)
}

/// Lit les identifiants publiés du fichier `*_subscriptions.vdf`.
fn read_subscriptions(path: &Path) -> Vec<u64> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(root) = vdf::parse_root(&text) else {
        return Vec::new();
    };
    let mut ids = Vec::new();
    for (_, entry) in root.objects() {
        let Some(id) = entry
            .str("publishedfileid")
            .and_then(|value| value.trim().parse::<u64>().ok())
        else {
            continue;
        };
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids
}

/// Poids d'un dossier, borné : un contenu sans manifeste ne doit pas faire
/// parcourir une arborescence entière sans fin.
fn dir_size(path: &Path) -> u64 {
    let mut total = 0;
    let mut seen = 0;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if seen >= MAX_FILES {
                return total;
            }
            seen += 1;
            let path = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.is_dir() {
                stack.push(path);
            } else if meta.is_file() {
                total += meta.len();
            }
        }
    }
    total
}

#[derive(Default)]
struct PublishedDetails {
    title: String,
    size: u64,
    preview_url: String,
}

/// Titres et poids publiés par Steam, et la raison de leur absence le cas
/// échéant. L'API accepte au plus 100 contenus par requête : les ateliers
/// fournis en demandent plusieurs, sinon les derniers resteraient incomplets.
fn fetch_details(ids: &[u64]) -> (HashMap<u64, PublishedDetails>, Option<String>) {
    if ids.is_empty() {
        return (HashMap::new(), None);
    }
    let agent = ureq::AgentBuilder::new().timeout(DETAILS_TIMEOUT).build();
    let mut found = HashMap::new();
    let mut note = None;
    for chunk in ids.chunks(100) {
        let mut form = format!("itemcount={}", chunk.len());
        for (index, id) in chunk.iter().enumerate() {
            form.push_str(&format!("&publishedfileids[{index}]={id}"));
        }
        let response = match agent
            .post(DETAILS_URL)
            .set("Content-Type", "application/x-www-form-urlencoded")
            .send_string(&form)
        {
            Ok(response) => response,
            Err(error) => {
                note = Some(t_args("workshop.note_details_unavailable", &[("error", &error.to_string())]));
                continue;
            }
        };
        let body = match response.into_string() {
            Ok(body) => body,
            Err(error) => {
                note = Some(t_args("workshop.note_details_unreadable", &[("error", &error.to_string())]));
                continue;
            }
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&body) else {
            note = Some(t("workshop.note_details_unreadable_short").into());
            continue;
        };
        if let Some(details) = value
            .get("response")
            .and_then(|response| response.get("publishedfiledetails"))
            .and_then(|details| details.as_array())
        {
            for (id, detail) in parse_published_details(details) {
                found.insert(id, detail);
            }
        }
    }
    (found, note)
}

/// Détails utiles d'une réponse de Steam : identifiant, titre, poids et
/// vignette. La vignette est une URL publique du CDN communautaire ; elle est
/// conservée telle quelle pour que l'interface puisse la charger à son rythme.
fn parse_published_details(details: &[serde_json::Value]) -> Vec<(u64, PublishedDetails)> {
    let mut parsed = Vec::new();
    for detail in details {
        let id = detail
            .get("publishedfileid")
            .and_then(|value| value.as_str())
            .and_then(|value| value.parse::<u64>().ok())
            .or_else(|| {
                detail
                    .get("publishedfileid")
                    .and_then(|value| value.as_u64())
            });
        let title = detail
            .get("title")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let size = detail
            .get("file_size")
            .and_then(|value| {
                value
                    .as_str()
                    .and_then(|text| text.parse::<u64>().ok())
                    .or_else(|| value.as_u64())
            })
            .unwrap_or(0);
        let preview_url = detail
            .get("preview_url")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if let Some(id) = id {
            parsed.push((
                id,
                PublishedDetails {
                    title,
                    size,
                    preview_url,
                },
            ));
        }
    }
    parsed
}

/// Items d'un atelier : le manifeste d'abord, le dossier de contenu en secours.
fn scan(
    libraries: &[Library],
    appid: u32,
    subscribed: &[u64],
) -> (Vec<WorkshopItem>, Option<usize>) {
    let Some(index) = workshop_library(libraries, appid) else {
        return (Vec::new(), None);
    };
    let library = &libraries[index];
    let content = library
        .steamapps()
        .join("workshop/content")
        .join(appid.to_string());
    let mut items = Vec::new();
    let mut seen = HashSet::new();
    if let Some(acf) = read_acf(
        &library
            .steamapps()
            .join("workshop")
            .join(format!("appworkshop_{appid}.acf")),
    ) {
        for entry in acf.items {
            let path = content.join(entry.id.to_string());
            let installed = path.is_dir();
            seen.insert(entry.id);
            items.push(WorkshopItem {
                published_file_id: entry.id,
                title: String::new(),
                preview_url: String::new(),
                size: entry.size,
                time_updated: entry.time_updated,
                manifest: entry.manifest,
                path,
                installed,
                disabled: false,
                url: item_url(entry.id),
            });
        }
    }
    // Un contenu téléchargé sans Steam peut ne pas encore figurer dans le
    // manifeste : le dossier de contenu reste la source de vérité.
    if let Ok(entries) = std::fs::read_dir(&content) {
        for entry in entries.flatten() {
            let Ok(id) = entry.file_name().to_string_lossy().parse::<u64>() else {
                continue;
            };
            if !seen.insert(id) {
                continue;
            }
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            items.push(WorkshopItem {
                published_file_id: id,
                title: String::new(),
                preview_url: String::new(),
                size: dir_size(&path),
                time_updated: 0,
                manifest: String::new(),
                path,
                installed: true,
                disabled: false,
                url: item_url(id),
            });
        }
    }

    // Les abonnements au compte sont connus même sans téléchargement : le
    // fichier `ugc` les garde, et l'API fournira leur poids et leur titre.
    let known: HashSet<u64> = items.iter().map(|item| item.published_file_id).collect();
    for id in subscribed {
        if known.contains(id) {
            continue;
        }
        let path = content.join(id.to_string());
        let installed = path.is_dir();
        items.push(WorkshopItem {
            published_file_id: *id,
            title: String::new(),
            preview_url: String::new(),
            size: if installed { dir_size(&path) } else { 0 },
            time_updated: 0,
            manifest: String::new(),
            path,
            installed,
            disabled: false,
            url: item_url(*id),
        });
    }
    items.sort_by(|left, right| {
        right
            .installed
            .cmp(&left.installed)
            .then(right.time_updated.cmp(&left.time_updated))
            .then(left.published_file_id.cmp(&right.published_file_id))
    });
    (items, Some(index))
}

fn count_entries(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|entry| {
            let path = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                return false;
            };
            meta.file_type().is_symlink() || (meta.is_dir() && is_managed_dir(&path))
        })
        .count()
}

/// Contenus d'atelier réellement installés et actifs, dans l'ordre où le jeu
/// doit les découvrir. Le fichier `Mods` peut désactiver un contenu sans le
/// supprimer : celui-ci ne doit alors pas être présenté au jeu.
pub fn installed(libraries: &[Library], appid: u32) -> Vec<WorkshopItem> {
    let (items, _) = scan(libraries, appid, &[]);
    let disabled = read_disabled(state_path(libraries, appid).as_deref());
    items
        .into_iter()
        .filter(|item| item.installed && !disabled.contains(&item.published_file_id))
        .collect()
}

/// Abonnements d'un jeu, leur poids et le dossier `Mods` correspondant.
pub fn report(libraries: &[Library], appid: u32, name: &str) -> WorkshopReport {
    report_with_subscriptions(libraries, appid, name, &[])
}

/// Même rapport, avec les abonnements relus en ligne.
///
/// Le fichier `userdata` de Steam ne connaît pas toujours un article suivi
/// depuis le site ou la fenêtre web de Lapinux : les identifiants en ligne
/// complètent la lecture locale, sans jamais en retirer.
pub fn report_with_subscriptions(
    libraries: &[Library],
    appid: u32,
    name: &str,
    online: &[u64],
) -> WorkshopReport {
    let subscription_path = subscriptions_file(libraries, appid);
    let mut subscribed = subscription_path
        .as_deref()
        .map(read_subscriptions)
        .unwrap_or_default();
    for id in online {
        if !subscribed.contains(id) {
            subscribed.push(*id);
        }
    }
    // Même sans contenu local, le fichier d'abonnements prouve que le compte
    // suit cet atelier : la fiche ne doit pas le présenter comme absent.
    let supported = !online.is_empty()
        || subscription_path.is_some()
        || libraries.iter().any(|library| {
            library
                .steamapps()
                .join("workshop")
                .join(format!("appworkshop_{appid}.acf"))
                .is_file()
                || library
                    .steamapps()
                    .join("workshop/content")
                    .join(appid.to_string())
                    .is_dir()
        });
    let (mut items, index) = scan(libraries, appid, &subscribed);
    let disabled = read_disabled(state_path(libraries, appid).as_deref());
    let ids: Vec<u64> = items.iter().map(|item| item.published_file_id).collect();
    let (details, details_note) = fetch_details(&ids);
    for item in &mut items {
        if let Some(detail) = details.get(&item.published_file_id) {
            if !detail.title.trim().is_empty() {
                item.title = detail.title.clone();
            }
            if item.size == 0 {
                item.size = detail.size;
            }
            if !detail.preview_url.is_empty() {
                item.preview_url = detail.preview_url.clone();
            }
        }
        item.disabled = disabled.contains(&item.published_file_id);
    }
    let mut report = WorkshopReport {
        appid,
        name: name.to_string(),
        supported,
        total_size: items.iter().map(|item| item.size).sum(),
        installed_size: items
            .iter()
            .filter(|item| item.installed)
            .map(|item| item.size)
            .sum(),
        workshop_url: workshop_url(appid),
        items,
        ..Default::default()
    };
    if index.is_some() {
        report.mods_dir = mods_dir(libraries, appid, name);
        report.links = count_entries(&report.mods_dir);
    }
    if let Some(note) = details_note {
        report.notes.push(note);
    }
    if !supported {
        report
            .notes
            .push(t("workshop.note_not_supported").into());
    } else if report.items.is_empty() {
        report
            .notes
            .push(t("workshop.note_no_subscription").into());
    } else if report.installed_size == 0 {
        report.notes.push(
            t("workshop.note_not_downloaded").into(),
        );
    }
    report
}

/// Range les abonnements installés dans
/// `LapinuxLibrary/Mods/<jeu> (<appid>)/SteamWorkshop`, en vrais fichiers.
/// Rend le bilan du rangement.
pub fn arrange(
    libraries: &[Library],
    appid: u32,
    name: &str,
) -> crate::Result<WorkshopArrangeReport> {
    let disabled = read_disabled(state_path(libraries, appid).as_deref());
    arrange_with_state(libraries, appid, name, &disabled)
}

fn arrange_with_state(
    libraries: &[Library],
    appid: u32,
    name: &str,
    disabled: &HashSet<u64>,
) -> crate::Result<WorkshopArrangeReport> {
    let (items, index) = scan(libraries, appid, &[]);
    index.ok_or_else(|| crate::Error::Other(t("workshop.err_no_library").into()))?;
    let ids: Vec<u64> = items
        .iter()
        .filter(|item| item.installed && !disabled.contains(&item.published_file_id))
        .map(|item| item.published_file_id)
        .collect();
    let (details, _) = fetch_details(&ids);
    let titles: HashMap<u64, String> = details
        .into_iter()
        .map(|(id, detail)| (id, detail.title))
        .collect();
    arrange_items(&items, libraries, appid, name, &titles, disabled)
}

/// Active ou désactive des contenus dans le rangement `Mods`. Le contenu
/// Steam n'est pas déplacé : seul l'état local et les dossiers matérialisés
/// changent.
pub fn set_enabled(
    libraries: &[Library],
    appid: u32,
    name: &str,
    ids: &[u64],
    enabled: bool,
) -> crate::Result<WorkshopManageReport> {
    let path = state_path(libraries, appid)
        .ok_or_else(|| crate::Error::Other(t("workshop.err_no_library").into()))?;
    let (items, index) = scan(libraries, appid, &[]);
    index.ok_or_else(|| crate::Error::Other(t("workshop.err_no_library").into()))?;
    let targets: Vec<u64> = if ids.is_empty() {
        items
            .iter()
            .filter(|item| item.installed)
            .map(|item| item.published_file_id)
            .collect()
    } else {
        ids.to_vec()
    };
    let mut disabled = read_disabled(Some(&path));
    for id in targets {
        if enabled {
            disabled.remove(&id);
        } else {
            disabled.insert(id);
        }
    }
    write_disabled(&path, &disabled)?;
    let arranged = arrange_with_state(libraries, appid, name, &disabled)?;
    Ok(WorkshopManageReport {
        links: arranged.links,
        removed_links: arranged.removed,
        removed_items: 0,
        freed_bytes: 0,
        errors: Vec::new(),
    })
}

/// Supprime les fichiers locaux de contenus choisis. La souscription Steam
/// reste en place : Steam peut les retélécharger lors d'une vérification.
/// Seuls les dossiers de contenu de cet appid sont supprimés, jamais une entrée
/// hors du rangement connu.
pub fn uninstall(
    libraries: &[Library],
    appid: u32,
    name: &str,
    ids: &[u64],
) -> crate::Result<WorkshopManageReport> {
    let (items, index) = scan(libraries, appid, &[]);
    let index =
        index.ok_or_else(|| crate::Error::Other(t("workshop.err_no_library").into()))?;
    let expected_root = libraries[index]
        .steamapps()
        .join("workshop/content")
        .join(appid.to_string());
    let dir = mods_dir(libraries, appid, name);
    let requested: HashSet<u64> = if ids.is_empty() {
        items
            .iter()
            .filter(|item| item.installed)
            .map(|item| item.published_file_id)
            .collect()
    } else {
        ids.iter().copied().collect()
    };
    let mut disabled = read_disabled(state_path(libraries, appid).as_deref());
    let mut report = WorkshopManageReport::default();
    let mut state_changed = false;

    for item in &items {
        if !item.installed || !requested.contains(&item.published_file_id) {
            continue;
        }
        let expected = expected_root.join(item.published_file_id.to_string());
        if item.path != expected {
            report.errors.push(t_args(
                "workshop.err_unexpected_path",
                &[("id", &item.published_file_id.to_string())],
            ));
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(&expected) else {
            continue;
        };
        if !meta.file_type().is_dir() {
            report.errors.push(t_args(
                "workshop.err_not_folder",
                &[("id", &item.published_file_id.to_string())],
            ));
            continue;
        }
        let bytes = dir_size(&expected);
        if let Err(error) = std::fs::remove_dir_all(&expected) {
            report.errors.push(t_args(
                "workshop.err_remove_failed",
                &[
                    ("id", &item.published_file_id.to_string()),
                    ("error", &error.to_string()),
                ],
            ));
            continue;
        }
        report.removed_items += 1;
        report.freed_bytes += bytes;
        report.removed_links += remove_entries_for(&dir, item.published_file_id);
        if disabled.remove(&item.published_file_id) {
            state_changed = true;
        }
    }

    if state_changed {
        if let Some(path) = state_path(libraries, appid) {
            if let Err(error) = write_disabled(&path, &disabled) {
                report.errors.push(t_args(
                    "workshop.err_state_save",
                    &[("error", &error.to_string())],
                ));
            }
        }
    }
    report.links = count_entries(&dir);
    Ok(report)
}

/// Télécharge des contenus de l'Atelier sans client Steam.
/// DepotDownloader réutilise la session Lapinux enregistrée quand elle existe
/// (sans redemander de mot de passe), sinon il passe en anonyme ; chaque contenu
/// est récupéré séparément, placé dans la bibliothèque Steam/Workshop, puis
/// matérialisé en vrais fichiers dans `Mods`. Aucun fichier du client Steam
/// n'est manipulé.
pub fn download_without_steam(
    data_dir: &Path,
    libraries: &[Library],
    appid: u32,
    name: &str,
    ids: &[u64],
) -> crate::Result<WorkshopDownloadReport> {
    let mut ids: Vec<u64> = ids.iter().copied().filter(|id| *id != 0).collect();
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
        return Err(crate::Error::Other(t("workshop.err_nothing_to_download").into()));
    }

    let index = download_library(libraries, appid)
        .ok_or_else(|| crate::Error::Other(t("workshop.err_no_library").into()))?;
    let content_root = libraries[index]
        .steamapps()
        .join("workshop/content")
        .join(appid.to_string());
    std::fs::create_dir_all(&content_root)?;

    let binary = depot::ensure(data_dir).map_err(crate::Error::Other)?;
    let depot_root = depot::root(data_dir);
    let log_path = depot::log_path(data_dir, appid);
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let work_root = depot::work(data_dir).join(appid.to_string());
    let home = work_root.join("home");
    let (account, auth_error) = match depot::prepare_home(&binary, data_dir, &home) {
        Ok(account) => (account, None),
        Err(error) => (None, Some(t_args("workshop.err_session", &[("error", &error.to_string())]))),
    };

    let mut report = WorkshopDownloadReport {
        appid,
        log: log_path.clone(),
        ..Default::default()
    };
    for id in &ids {
        let item_work = work_root.join(id.to_string());
        let stage = item_work.join("content");
        let _ = std::fs::remove_dir_all(&item_work);
        if let Err(error) = std::fs::create_dir_all(&stage) {
            report
                .errors
                .push(t_args("workshop.err_prepare", &[("id", &id.to_string()), ("error", &error.to_string())]));
            continue;
        }
        let log_file = match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
        {
            Ok(file) => file,
            Err(error) => {
                report
                    .errors
                    .push(t_args("workshop.err_log", &[("id", &id.to_string()), ("error", &error.to_string())]));
                let _ = std::fs::remove_dir_all(&item_work);
                continue;
            }
        };
        let log_err = match log_file.try_clone() {
            Ok(file) => file,
            Err(error) => {
                report
                    .errors
                    .push(t_args("workshop.err_log", &[("id", &id.to_string()), ("error", &error.to_string())]));
                let _ = std::fs::remove_dir_all(&item_work);
                continue;
            }
        };

        let mut command = Command::new(&binary);
        command
            .arg("-app")
            .arg(appid.to_string())
            .arg("-pubfile")
            .arg(id.to_string());
        if let Some(account) = &account {
            command
                .arg("-username")
                .arg(account)
                .arg("-remember-password");
        }
        command
            .arg("-dir")
            .arg(&stage)
            .current_dir(&depot_root)
            .env("HOME", &home)
            .env("XDG_DATA_HOME", home.join(".local/share"))
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("PATH", "/usr/bin:/bin")
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("DBUS_SESSION_BUS_ADDRESS")
            .stdin(Stdio::null())
            .stdout(Stdio::from(log_file))
            .stderr(Stdio::from(log_err));

        match command.spawn() {
            Ok(mut child) => match wait_for_child(&mut child, WORKSHOP_DOWNLOAD_TIMEOUT) {
                Ok(Some(status)) if status.success() => {}
                Ok(Some(status)) => report.errors.push(t_args("workshop.err_depot_code", &[("id", &id.to_string()), ("code", &status.code().unwrap_or(-1).to_string())])),
                Ok(None) => report.errors.push(t_args("workshop.err_depot_timeout", &[("id", &id.to_string()), ("minutes", &(WORKSHOP_DOWNLOAD_TIMEOUT.as_secs() / 60).to_string())])),
                Err(error) => report
                    .errors
                    .push(t_args("workshop.err_depot_interrupted", &[("id", &id.to_string()), ("error", &error.to_string())])),
            },
            Err(error) => report.errors.push(t_args("workshop.err_depot_launch", &[("id", &id.to_string()), ("error", &error.to_string())])),
        }

        let _ = std::fs::remove_dir_all(stage.join(".DepotDownloader"));
        if !has_payload(&stage) {
            report.errors.push(t_args("workshop.err_depot_empty", &[("id", &id.to_string())]));
            let _ = std::fs::remove_dir_all(&item_work);
            continue;
        }
        let target = content_root.join(id.to_string());
        match place_downloaded(&stage, &target) {
            Ok(()) => {
                report.downloaded.push(*id);
                report.bytes += dir_size(&target);
            }
            Err(error) => report
                .errors
                .push(t_args("workshop.err_integrate", &[("id", &id.to_string()), ("error", &error.to_string())])),
        }
        let _ = std::fs::remove_dir_all(&item_work);
    }
    // Le compte.config contient une copie du jeton : il ne doit pas survivre
    // au téléchargement, même quand celui-ci a échoué.
    let _ = std::fs::remove_dir_all(&home);

    if !report.downloaded.is_empty() {
        let disabled = read_disabled(state_path(libraries, appid).as_deref());
        let (items, _) = scan(libraries, appid, &[]);
        let (details, _) = fetch_details(&report.downloaded);
        let titles: HashMap<u64, String> = details
            .into_iter()
            .map(|(id, detail)| (id, detail.title))
            .collect();
        match arrange_items(&items, libraries, appid, name, &titles, &disabled) {
            Ok(arranged) => report.links = arranged.links,
            Err(error) => report
                .errors
                .push(t_args("workshop.err_arrange", &[("error", &error.to_string())])),
        }
    }
    if report.downloaded.is_empty() && report.errors.is_empty() {
        report
            .errors
            .push(t("workshop.err_depot_empty_short").into());
    }
    if !report.errors.is_empty() {
        if let Some(error) = auth_error {
            report.errors.push(error);
        }
        let tail = log_tail(&log_path);
        if !tail.is_empty() {
            report.errors.push(tail);
        }
    }
    Ok(report)
}

fn has_payload(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(false)
}

/// Bibliothèque où placer le téléchargement : celle qui porte déjà un atelier
/// Steam, sinon la bibliothèque gérée par Lapinux, sinon la première connue.
fn download_library(libraries: &[Library], appid: u32) -> Option<usize> {
    let existing = libraries.iter().position(|library| {
        library
            .steamapps()
            .join("workshop")
            .join(format!("appworkshop_{appid}.acf"))
            .is_file()
            || library
                .steamapps()
                .join("workshop/content")
                .join(appid.to_string())
                .is_dir()
    });
    existing
        .or_else(|| libraries.iter().position(|library| library.managed))
        .or(if libraries.is_empty() { None } else { Some(0) })
}

/// Déplace le dossier téléchargé par DepotDownloader vers son emplacement
/// final. Un téléchargement déjà présent n'est jamais écrasé.
fn place_downloaded(source: &Path, target: &Path) -> std::io::Result<()> {
    if source == target {
        return Ok(());
    }
    if let Ok(meta) = std::fs::symlink_metadata(target) {
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                t_args("workshop.err_target_exists", &[("path", &target.display().to_string())]),
            ));
        }
        let _ = std::fs::remove_dir_all(source);
        return Ok(());
    }
    let parent = target.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            t("workshop.err_target_no_parent"),
        )
    })?;
    std::fs::create_dir_all(parent)?;
    let staging = parent.join(format!(
        ".{}.part",
        target
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("workshop")
    ));
    let _ = std::fs::remove_dir_all(&staging);
    if std::fs::rename(source, &staging).is_err() {
        if let Err(error) = lapinux_core::util::copy_tree(source, &staging) {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(error);
        }
    }
    if let Err(error) = std::fs::rename(&staging, target) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(error);
    }
    let _ = std::fs::remove_dir_all(source);
    Ok(())
}

fn wait_for_child(
    child: &mut std::process::Child,
    timeout: Duration,
) -> std::io::Result<Option<std::process::ExitStatus>> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn log_tail(path: &Path) -> String {
    let Ok(text) = std::fs::read_to_string(path) else {
        return String::new();
    };
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(8)..].join(" | ")
}

fn remove_entries_for(dir: &Path, id: u64) -> usize {
    let suffix = format!("[{id}]");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let target_matches = std::fs::read_link(&path)
            .ok()
            .and_then(|target| target.file_name()?.to_str()?.parse::<u64>().ok())
            == Some(id);
        let managed = marker_id(&path) == Some(id);
        if (name.ends_with(&suffix) || target_matches || managed) && remove_managed_entry(&path) {
            removed += 1;
        }
    }
    removed
}

/// Rangement déjà scanné : les tests le pilotent sans réseau.
fn arrange_items(
    items: &[WorkshopItem],
    libraries: &[Library],
    appid: u32,
    name: &str,
    titles: &HashMap<u64, String>,
    disabled: &HashSet<u64>,
) -> crate::Result<WorkshopArrangeReport> {
    let dir = mods_dir(libraries, appid, name);
    std::fs::create_dir_all(&dir)?;
    migrate_legacy_links(libraries, appid, name, &dir)?;

    // Le titre quand Steam le publie, sinon l'identifiant. Le suffixe
    // d'identifiant évite qu'un abonnement en écrase un autre et reconnaît un
    // contenu renommé d'un rangement à l'autre.
    let mut desired: Vec<(String, PathBuf, u64)> = Vec::new();
    for item in items
        .iter()
        .filter(|item| item.installed && !disabled.contains(&item.published_file_id))
    {
        let title = titles
            .get(&item.published_file_id)
            .map(String::as_str)
            .unwrap_or("");
        let base = if title.trim().is_empty() {
            format!("Atelier {}", item.published_file_id)
        } else {
            safe_name(title, "Atelier")
        };
        desired.push((
            format!("{base} [{}]", item.published_file_id),
            item.path.clone(),
            item.published_file_id,
        ));
    }

    // Les entrées d'un rangement précédent qui ne correspondent plus à un
    // abonnement partent : uniquement les liens et dossiers marqués par
    // Lapinux, jamais un fichier ou un dossier de l'utilisateur.
    let wanted: HashSet<&str> = desired.iter().map(|(name, _, _)| name.as_str()).collect();
    let mut removed = 0;
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let file_name = entry.file_name();
            let Some(file_name) = file_name.to_str() else {
                continue;
            };
            if !wanted.contains(file_name) && remove_managed_entry(&path) {
                removed += 1;
            }
        }
    }

    for (entry_name, target, id) in desired {
        if !target.is_dir() {
            continue;
        }
        let destination = dir.join(&entry_name);
        let _ = materialize_entry(&target, &destination, id);
    }

    let links = count_entries(&dir);
    let bytes = items
        .iter()
        .filter(|item| item.installed && !disabled.contains(&item.published_file_id))
        .map(|item| item.size)
        .sum();
    Ok(WorkshopArrangeReport {
        dir,
        links,
        removed,
        bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library_with_workshop() -> (tempfile::TempDir, Library) {
        let dir = tempfile::tempdir().unwrap();
        let library = Library::new(dir.path());
        let workshop = library.steamapps().join("workshop");
        std::fs::create_dir_all(workshop.join("content/4000/111")).unwrap();
        std::fs::create_dir_all(workshop.join("content/4000/222")).unwrap();
        std::fs::write(workshop.join("content/4000/111/mod.txt"), "contenu 111").unwrap();
        std::fs::write(workshop.join("content/4000/222/mod.txt"), "contenu 222").unwrap();
        std::fs::write(
            workshop.join("appworkshop_4000.acf"),
            r#"
"AppWorkshop"
{
    "appid" "4000"
    "WorkshopItemsInstalled"
    {
        "111"
        {
            "size" "1500"
            "timeupdated" "1700000000"
            "manifest" "m1"
        }
        "222"
        {
            "size" "2500"
            "timeupdated" "1700000100"
            "manifest" "m2"
        }
    }
}
"#,
        )
        .unwrap();
        (dir, library)
    }

    #[test]
    fn les_abonnements_se_lisent_dans_le_manifeste() {
        let (_dir, library) = library_with_workshop();
        let acf = read_acf(&library.steamapps().join("workshop/appworkshop_4000.acf"))
            .expect("manifeste lu");
        assert_eq!(acf.items.len(), 2);
        assert_eq!(acf.items[0].id, 222);
        assert_eq!(acf.items[0].size, 2500);
        assert!(acf.items[0].time_updated > acf.items[1].time_updated);
    }

    #[test]
    fn les_abonnements_non_telecharges_sont_lus_dans_ugc() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("4670920_subscriptions.vdf");
        std::fs::write(
            &path,
            r#""subscribedfiles"
{
    "appid" "4670920"
    "time_last_updated" "1792171459"
    "0"
    {
        "publishedfileid" "3812321265"
        "time_subscribed" "1792180814"
        "disabled_locally" "0"
    }
    "1"
    {
        "publishedfileid" "3815663735"
        "time_subscribed" "1792180762"
        "disabled_locally" "0"
    }
}
"#,
        )
        .unwrap();
        assert_eq!(read_subscriptions(&path), vec![3812321265, 3815663735]);

        let (_library_dir, library) = library_with_workshop();
        let (items, index) = scan(
            std::slice::from_ref(&library),
            4000,
            &[3812321265, 3815663735, 111],
        );
        assert!(index.is_some());
        assert_eq!(items.len(), 4, "deux contenus locaux, trois abonnements");
        assert_eq!(
            items
                .iter()
                .filter(|item| item.published_file_id == 111)
                .count(),
            1,
            "un contenu déjà installé n'est pas dupliqué"
        );
        let remote = items
            .iter()
            .find(|item| item.published_file_id == 3812321265)
            .expect("abonnement distant");
        assert!(!remote.installed);
        assert_eq!(remote.size, 0);
    }

    #[test]
    fn les_details_publies_gardent_le_visuel_et_le_poids() {
        let details = serde_json::json!([
            {
                "publishedfileid": "3812321265",
                "title": "Backrooms",
                "file_size": "32616323",
                "preview_url": "https://images.example/backrooms.jpg"
            },
            {
                "publishedfileid": 42,
                "title": "Sans visuel",
                "file_size": 12,
                "preview_url": ""
            }
        ]);
        let parsed = parse_published_details(details.as_array().unwrap());
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].1.title, "Backrooms");
        assert_eq!(parsed[0].1.size, 32_616_323);
        assert_eq!(
            parsed[0].1.preview_url,
            "https://images.example/backrooms.jpg"
        );
        assert_eq!(parsed[1].0, 42);
        assert!(parsed[1].1.preview_url.is_empty());
    }

    #[test]
    fn le_rangement_cree_de_vrais_fichiers_et_ne_touche_pas_a_la_source() {
        let (_dir, library) = library_with_workshop();
        let (items, _) = scan(std::slice::from_ref(&library), 4000, &[]);
        let report = arrange_items(
            &items,
            std::slice::from_ref(&library),
            4000,
            "Garry's Mod",
            &HashMap::new(),
            &HashSet::new(),
        )
        .unwrap();
        assert_eq!(report.links, 2);
        assert!(report
            .dir
            .ends_with("Mods/Garry's Mod (4000)/SteamWorkshop"));
        for entry in std::fs::read_dir(&report.dir).unwrap().flatten() {
            let meta = std::fs::symlink_metadata(entry.path()).unwrap();
            assert!(meta.is_dir(), "le contenu doit être un vrai dossier");
            assert!(
                !meta.file_type().is_symlink(),
                "le dossier ne doit plus être un lien symbolique"
            );
        }
        assert_eq!(
            std::fs::read_to_string(report.dir.join("Atelier 111 [111]/mod.txt")).unwrap(),
            "contenu 111"
        );
        assert!(library
            .steamapps()
            .join("workshop/content/4000/111/mod.txt")
            .is_file());

        // Un fichier de l'utilisateur au même nom n'est jamais écrasé, et une
        // entrée obsolète part au rangement suivant.
        let keep = report.dir.join("garde.txt");
        std::fs::write(&keep, "à moi").unwrap();
        let stale = report.dir.join("vieux [999]");
        std::os::unix::fs::symlink(
            library.steamapps().join("workshop/content/4000/111"),
            &stale,
        )
        .unwrap();
        let again = arrange_items(
            &items,
            std::slice::from_ref(&library),
            4000,
            "Garry's Mod",
            &HashMap::new(),
            &HashSet::new(),
        )
        .unwrap();
        assert_eq!(again.removed, 1);
        assert_eq!(std::fs::read_to_string(&keep).unwrap(), "à moi");
        assert!(!stale.exists());
    }

    #[test]
    fn l_etat_desactive_est_persiste_hors_du_dossier_du_jeu() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".lapinux/workshop/4000.json");
        let disabled = HashSet::from([111_u64, 222]);
        write_disabled(&path, &disabled).unwrap();
        assert_eq!(read_disabled(Some(&path)), disabled);
        assert!(path.is_file());
    }

    #[test]
    fn un_contenu_desactive_perd_son_dossier_mais_garde_ses_fichiers() {
        let (_dir, library) = library_with_workshop();
        let (items, _) = scan(std::slice::from_ref(&library), 4000, &[]);
        let disabled = HashSet::from([111_u64]);
        let report = arrange_items(
            &items,
            std::slice::from_ref(&library),
            4000,
            "Garry's Mod",
            &HashMap::new(),
            &disabled,
        )
        .unwrap();
        assert_eq!(report.links, 1);
        assert!(!report.dir.join("Atelier 111 [111]").exists());
        assert!(report.dir.join("Atelier 222 [222]").is_dir());
        assert!(library
            .steamapps()
            .join("workshop/content/4000/111")
            .is_dir());
        assert!(
            std::fs::symlink_metadata(report.dir.join("Atelier 222 [222]"))
                .unwrap()
                .is_dir()
        );
    }

    #[test]
    fn la_desinstallation_locale_retire_le_contenu_et_son_dossier() {
        let (_dir, library) = library_with_workshop();
        std::fs::write(
            library
                .steamapps()
                .join("workshop/content/4000/111/mod.txt"),
            "contenu",
        )
        .unwrap();
        let (items, _) = scan(std::slice::from_ref(&library), 4000, &[]);
        let arranged = arrange_items(
            &items,
            std::slice::from_ref(&library),
            4000,
            "Garry's Mod",
            &HashMap::new(),
            &HashSet::new(),
        )
        .unwrap();
        let removed =
            uninstall(std::slice::from_ref(&library), 4000, "Garry's Mod", &[111]).unwrap();
        assert_eq!(removed.removed_items, 1);
        assert!(removed.freed_bytes > 0);
        assert!(removed.errors.is_empty());
        assert!(!library
            .steamapps()
            .join("workshop/content/4000/111")
            .exists());
        assert!(!arranged.dir.join("Atelier 111 [111]").exists());
        assert!(library
            .steamapps()
            .join("workshop/content/4000/222")
            .is_dir());
        assert!(arranged.dir.join("Atelier 222 [222]").is_dir());
        assert_eq!(removed.links, 1);
    }

    #[test]
    fn le_rangement_va_dans_la_bibliotheque_geree() {
        let (dir, library) = library_with_workshop();
        let managed_root = dir.path().join("LapinuxLibrary");
        std::fs::create_dir_all(&managed_root).unwrap();
        let managed = Library::managed(managed_root.clone());
        let libraries = [library, managed];
        let (items, _) = scan(&libraries, 4000, &[]);
        let report = arrange_items(
            &items,
            &libraries,
            4000,
            "Garry's Mod",
            &HashMap::new(),
            &HashSet::new(),
        )
        .unwrap();
        assert_eq!(
            report.dir,
            managed_root.join("Mods/Garry's Mod (4000)/SteamWorkshop")
        );
        assert_eq!(report.links, 2);
        assert!(report.dir.is_dir());
    }

    #[test]
    fn les_anciens_liens_directs_migrent_dans_steamworkshop() {
        let (_dir, library) = library_with_workshop();
        let legacy = library.root.join("Mods/Garry's Mod (4000)");
        std::fs::create_dir_all(&legacy).unwrap();
        for id in [111_u64, 222] {
            std::os::unix::fs::symlink(
                library
                    .steamapps()
                    .join(format!("workshop/content/4000/{id}")),
                legacy.join(format!("Atelier {id} [{id}]")),
            )
            .unwrap();
        }
        let (items, _) = scan(std::slice::from_ref(&library), 4000, &[]);
        let report = arrange_items(
            &items,
            std::slice::from_ref(&library),
            4000,
            "Garry's Mod",
            &HashMap::new(),
            &HashSet::new(),
        )
        .unwrap();
        assert_eq!(report.dir, legacy.join("SteamWorkshop"));
        assert_eq!(report.links, 2);
        for id in [111_u64, 222] {
            let old = legacy.join(format!("Atelier {id} [{id}]"));
            assert!(!old.exists(), "l'ancien lien direct a été retiré");
            let new = report.dir.join(format!("Atelier {id} [{id}]"));
            let meta = std::fs::symlink_metadata(&new).unwrap();
            assert!(meta.is_dir());
            assert!(!meta.file_type().is_symlink());
        }
        assert!(legacy.is_dir(), "le parent garde le dossier SteamWorkshop");
        assert!(library
            .steamapps()
            .join("workshop/content/4000/111")
            .is_dir());
    }

    #[test]
    fn un_contenu_hors_manifeste_reste_visible() {
        let (_dir, library) = library_with_workshop();
        let downloaded = library.steamapps().join("workshop/content/4000/333");
        std::fs::create_dir_all(&downloaded).unwrap();
        std::fs::write(downloaded.join("mod.txt"), "SteamCMD").unwrap();

        let (items, _) = scan(std::slice::from_ref(&library), 4000, &[]);
        assert_eq!(
            items.len(),
            3,
            "deux manifestes et un téléchargement direct"
        );
        let item = items
            .iter()
            .find(|item| item.published_file_id == 333)
            .expect("contenu téléchargé hors manifeste");
        assert!(item.installed);
        assert_eq!(item.size, 8);
        assert_eq!(
            items
                .iter()
                .filter(|item| item.published_file_id == 333)
                .count(),
            1
        );
    }

    #[test]
    fn le_contenu_steamcmd_est_deplace_sans_ecraser_l_existant() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir
            .path()
            .join("runtime/steamapps/workshop/content/4000/444");
        let target = dir
            .path()
            .join("library/steamapps/workshop/content/4000/444");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("mod.txt"), "nouveau").unwrap();

        place_downloaded(&source, &target).unwrap();
        assert_eq!(
            std::fs::read_to_string(target.join("mod.txt")).unwrap(),
            "nouveau"
        );
        assert!(!source.exists());

        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("mod.txt"), "encore").unwrap();
        place_downloaded(&source, &target).unwrap();
        assert_eq!(
            std::fs::read_to_string(target.join("mod.txt")).unwrap(),
            "nouveau",
            "un téléchargement existant n'est jamais écrasé"
        );
        assert!(!source.exists());
    }

    #[test]
    fn le_telechargement_choisit_la_bibliotheque_geree_sans_atelier_existant() {
        let dir = tempfile::tempdir().unwrap();
        let library = Library::new(dir.path().join("Steam"));
        let managed = Library::managed(dir.path().join("LapinuxLibrary"));
        let libraries = [library, managed];
        assert_eq!(download_library(&libraries, 4000), Some(1));
    }
}
