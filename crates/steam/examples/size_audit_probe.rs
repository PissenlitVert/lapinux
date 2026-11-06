//! Diagnostic : poids annoncé par PICS face au poids réellement installé.
//!
//! Pour chaque appid, la sonde lit le protocole (jeton d'accès puis fiche
//! PICS), additionne les manifestes publics de ses dépôts, puis compare au
//! `appmanifest_<appid>.acf` du client Steam local : `SizeOnDisk`,
//! `BytesToDownload`, dépôts réellement montés et taille du dossier sur le
//! disque. Les écarts disent si la fiche annonce trop, et pourquoi.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use lapinux_steam::session;
use lapinux_steam::vdf;
use steamroom::apps::AccessToken;
use steamroom::depot::{AppId, DepotId, PackageId};
use steamroom::types::key_value::{KvValue, KeyValue};
use steamroom_client::login::LoginBuilder;

fn number(value: Option<&KeyValue>) -> Option<u64> {
    match value.map(|value| &value.value) {
        Some(KvValue::String(value)) => value.parse().ok(),
        Some(KvValue::UInt64(value)) => Some(*value),
        Some(KvValue::Int64(value)) => u64::try_from(*value).ok(),
        Some(KvValue::Int32(value)) => u64::try_from(*value).ok(),
        _ => None,
    }
}

fn text(value: Option<&KeyValue>) -> String {
    value
        .and_then(KeyValue::as_str)
        .unwrap_or_default()
        .to_string()
}

fn gib(bytes: u64) -> f64 {
    bytes as f64 / 1e9
}

/// Racines de bibliothèques Steam connues du client local.
fn library_roots() -> Vec<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let main = Path::new(&home).join(".local/share/Steam");
    let mut roots = vec![main.clone()];
    let file = main.join("steamapps/libraryfolders.vdf");
    if let Ok(text) = std::fs::read_to_string(&file) {
        if let Ok(root) = vdf::parse(&text) {
            if let Some(libraries) = root.obj("libraryfolders") {
                for (_, entry) in libraries.objects() {
                    if let Some(path) = entry.str("path") {
                        roots.push(PathBuf::from(path));
                    }
                }
            }
        }
    }
    // Bibliothèque gérée par Lapinux, lue dans sa configuration.
    if let Ok(text) = std::fs::read_to_string(Path::new(&home).join(".config/lapinux/config.toml")) {
        if let Some(line) = text
            .lines()
            .find(|line| line.trim_start().starts_with("library_root"))
        {
            if let Some(path) = line.split('"').nth(1) {
                roots.push(PathBuf::from(path));
            }
        }
    }
    roots
}

fn disk_bytes(path: &Path) -> Option<u64> {
    let output = std::process::Command::new("du")
        .arg("-sb")
        .arg(path)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    text.split_whitespace().next()?.parse().ok()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let appids: Vec<u32> = args
        .iter()
        .filter_map(|arg| arg.parse::<u32>().ok())
        .collect();
    let packages: Vec<u32> = args
        .iter()
        .filter_map(|arg| arg.strip_prefix("pkg:"))
        .filter_map(|id| id.parse().ok())
        .collect();
    let roots = library_roots();
    let saved = session::load().expect("aucune session Steam enregistrée");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async move {
        let client = LoginBuilder::new()
            .device_name("Lapinux (audit des tailles)")
            .with_refresh_token(&saved.account_name, &saved.refresh_token)
            .login()
            .await
            .unwrap();
        if !packages.is_empty() {
            let ids: Vec<PackageId> = packages.iter().copied().map(PackageId).collect();
            let infos = client.pics_get_package_info(&ids).await.unwrap();
            for info in infos {
                let Some(package_id) = info.package_id.map(|id| id.0) else {
                    continue;
                };
                let Some(bytes) = info.kv_data else {
                    println!("paquet {package_id} : aucune fiche");
                    continue;
                };
                let root = if bytes.first() == Some(&0) {
                    KeyValue::from_binary(&bytes).ok()
                } else {
                    KeyValue::from_text(&String::from_utf8_lossy(&bytes)).ok()
                };
                let Some(root) = root else {
                    println!("paquet {package_id} : fiche illisible");
                    continue;
                };
                let package = root.get("package").unwrap_or(&root);
                let depots: Vec<u32> = package
                    .get("depotids")
                    .map(|node| {
                        if let KvValue::Children(children) = &node.value {
                            children
                                .values()
                                .filter_map(|value| number(Some(value)))
                                .filter_map(|value| u32::try_from(value).ok())
                                .collect()
                        } else {
                            Vec::new()
                        }
                    })
                    .unwrap_or_default();
                println!(
                    "paquet {package_id} : {} dépôt(s) {:?}",
                    depots.len(),
                    depots
                );
            }
        }
        if appids.is_empty() {
            return;
        }
        let ids: Vec<AppId> = appids.iter().copied().map(AppId).collect();
        let tokens = client.pics_get_access_tokens(&ids).await.unwrap();
        let tokens: Vec<AccessToken> = ids
            .iter()
            .map(|id| AccessToken {
                app_id: *id,
                token: tokens
                    .iter()
                    .find(|token| token.app_id == *id)
                    .map(|token| token.token)
                    .unwrap_or(0),
            })
            .collect();
        for info in client.pics_get_product_info(&tokens).await.unwrap() {
            let Some(appid) = info.app_id.map(|id| id.0) else {
                continue;
            };
            let Some(bytes) = info.kv_data else {
                println!("{appid} : aucune fiche PICS (jeu de la famille ?)");
                continue;
            };
            let root = if bytes.first() == Some(&0) {
                KeyValue::from_binary(&bytes).ok()
            } else {
                KeyValue::from_text(&String::from_utf8_lossy(&bytes)).ok()
            };
            let Some(root) = root else { continue };
            let app = root.get("appinfo").unwrap_or(&root);
            let name = text(app.get("common").and_then(|common| common.get("name")));
            let installdir = text(
                app.get("config")
                    .and_then(|config| config.get("installdir")),
            );
            let Some(depots) = app.get("depots") else {
                continue;
            };
            let KvValue::Children(entries) = &depots.value else {
                continue;
            };
            println!("\n=== {appid} · {name} · {installdir} ===");
            let mut total_install = 0u64;
            let mut total_download = 0u64;
            let mut counted: HashSet<u32> = HashSet::new();
            for (key, depot) in entries {
                let Ok(depot_id) = key.parse::<u32>() else {
                    continue;
                };
                if depot.get("sharedinstall").is_some() || depot.get("dlcappid").is_some() {
                    continue;
                }
                let Some(manifest) = depot.get("manifests").and_then(|all| all.get("public")) else {
                    continue;
                };
                let install = number(manifest.get("size")).unwrap_or(0);
                let download = number(manifest.get("download")).unwrap_or(0);
                if install == 0 && download == 0 {
                    continue;
                }
                total_install += install;
                total_download += download;
                counted.insert(depot_id);
                println!(
                    "  dépôt {depot_id:<9} os={:<8} langue={:<8} install={:>7.2} Go download={:>7.2} Go",
                    text(depot.get("config").and_then(|c| c.get("oslist"))),
                    text(depot.get("config").and_then(|c| c.get("language"))),
                    gib(install),
                    gib(download),
                );
            }
            println!(
                "  somme annoncée : install {:.2} Go · download {:.2} Go ({})",
                gib(total_install),
                gib(total_download),
                counted.len()
            );
            // Ce que le protocole autorise réellement : Steam ne rend la clé
            // d'un dépôt qu'aux licences qui le couvrent.
            let mut entitled_install = 0u64;
            let mut entitled_download = 0u64;
            let mut refused: Vec<u32> = Vec::new();
            for depot_id in &counted {
                match client
                    .get_depot_decryption_key(DepotId(*depot_id), AppId(appid))
                    .await
                {
                    Ok(_) => {
                        let depot = entries
                            .iter()
                            .find(|(key, _)| key.parse::<u32>() == Ok(*depot_id))
                            .map(|(_, depot)| depot);
                        let manifest = depot
                            .and_then(|depot| depot.get("manifests"))
                            .and_then(|all| all.get("public"));
                        entitled_install += manifest
                            .and_then(|manifest| number(manifest.get("size")))
                            .unwrap_or(0);
                        entitled_download += manifest
                            .and_then(|manifest| number(manifest.get("download")))
                            .unwrap_or(0);
                    }
                    Err(_) => refused.push(*depot_id),
                }
            }
            println!(
                "  autorisé par le protocole : install {:.2} Go · download {:.2} Go{}",
                gib(entitled_install),
                gib(entitled_download),
                if refused.is_empty() {
                    String::new()
                } else {
                    format!(" (refusés : {refused:?})")
                }
            );
            for root in &roots {
                let acf = root.join(format!("steamapps/appmanifest_{appid}.acf"));
                let Ok(text) = std::fs::read_to_string(&acf) else {
                    continue;
                };
                let Ok(parsed) = vdf::parse_root(&text) else {
                    continue;
                };
                let state = parsed.obj("AppState").unwrap_or(&parsed);
                let mounted: Vec<u32> = state
                    .obj("MountedDepots")
                    .map(|depots| {
                        depots
                            .strings()
                            .filter_map(|(key, _)| key.parse().ok())
                            .collect()
                    })
                    .unwrap_or_default();
                let unknown: Vec<u32> = mounted
                    .iter()
                    .copied()
                    .filter(|depot| !counted.contains(depot))
                    .collect();
                let missing: Vec<u32> = counted
                    .iter()
                    .copied()
                    .filter(|depot| !mounted.contains(depot))
                    .collect();
                let dir = root.join("steamapps/common").join(&installdir);
                println!(
                    "  installé {} : SizeOnDisk {:.2} Go · BytesToDownload {:.2} Go · {} dépôt(s) monté(s)",
                    root.display(),
                    gib(state.u64("SizeOnDisk")),
                    gib(state.u64("BytesToDownload")),
                    mounted.len(),
                );
                if !unknown.is_empty() {
                    println!("    montés mais jamais comptés : {unknown:?}");
                }
                if !missing.is_empty() {
                    println!("    comptés mais jamais montés : {missing:?}");
                }
                if let Some(bytes) = disk_bytes(&dir) {
                    println!("    disque réel : {:.2} Go", gib(bytes));
                }
            }
        }
    });
}
