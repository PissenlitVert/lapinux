//! Sonde : le catalogue Epic dit-il quels contenus additionnels vont avec un
//! jeu ?
//!
//! Lit les jetons du compte, puis les enregistrements de la bibliothèque, et
//! imprime pour chaque fiche de catalogue ses catégories et les clés qui
//! pourraient relier un contenu à son jeu de base. On voit ainsi si un jeu peut
//! annoncer ses DLC sans deviner.

use std::collections::BTreeSet;
use std::fs;

const LIBRARY: &str = "https://library-service.live.use1a.on.epicgames.com";
const CATALOG: &str = "https://catalog-public-service-prod06.ol.epicgames.com";

fn get(url: &str, token: &str) -> Option<serde_json::Value> {
    let response = ureq::get(url)
        .set("User-Agent", "UELauncher/14.0.0")
        .set("Authorization", &format!("Bearer {token}"))
        .timeout(std::time::Duration::from_secs(30))
        .call();
    match response {
        Ok(response) => response
            .into_string()
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok()),
        Err(ureq::Error::Status(code, _)) => {
            println!("HTTP {code} · {url}");
            None
        }
        Err(error) => {
            println!("réseau : {error}");
            None
        }
    }
}

fn main() {
    let home = std::env::var("HOME").unwrap_or_default();
    let raw = fs::read_to_string(format!("{home}/.local/share/lapinux/stores/epic.json"))
        .expect("jetons Epic illisibles");
    let value: serde_json::Value = serde_json::from_str(&raw).expect("json illisible");
    let find = |key: &str| -> Option<String> {
        value
            .get(key)
            .and_then(|v| v.as_str())
            .or_else(|| {
                value
                    .get("tokens")
                    .and_then(|t| t.get(key))
                    .and_then(|v| v.as_str())
            })
            .map(str::to_string)
    };
    let Some(token) = find("access_token") else {
        println!("pas de jeton");
        return;
    };
    let mut records = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..5 {
        let mut url = format!("{LIBRARY}/library/api/public/items?includeMetadata=true");
        if let Some(cursor) = &cursor {
            url.push_str(&format!("&cursor={cursor}"));
        }
        let Some(page) = get(&url, &token) else {
            return;
        };
        if let Some(items) = page.get("records").and_then(|v| v.as_array()) {
            records.extend(items.iter().cloned());
        }
        cursor = page
            .pointer("/responseMetadata/nextCursor")
            .and_then(|v| v.as_str())
            .filter(|cursor| !cursor.is_empty())
            .map(str::to_string);
        if cursor.is_none() || records.len() >= 200 {
            break;
        }
    }
    println!("{} enregistrements de bibliothèque", records.len());
    let mut grouped: std::collections::HashMap<String, BTreeSet<String>> = Default::default();
    for record in &records {
        let (Some(namespace), Some(item)) = (
            record.get("namespace").and_then(|v| v.as_str()),
            record.get("catalogItemId").and_then(|v| v.as_str()),
        ) else {
            continue;
        };
        grouped
            .entry(namespace.to_string())
            .or_default()
            .insert(item.to_string());
    }
    for (namespace, items) in grouped {
        let items: Vec<String> = items.into_iter().take(60).collect();
        for batch in items.chunks(20) {
            let query = batch
                .iter()
                .map(|item| format!("id={item}"))
                .collect::<Vec<_>>()
                .join("&");
            let url = format!(
                "{CATALOG}/catalog/api/shared/namespace/{namespace}/bulk/items?{query}&includeMainGameDetails=true&country=FR&locale=fr"
            );
            let Some(page) = get(&url, &token) else {
                continue;
            };
            let Some(map) = page.as_object() else {
                continue;
            };
            for (id, item) in map {
                let title = item.get("title").and_then(|v| v.as_str()).unwrap_or("");
                let categories: Vec<&str> = item
                    .get("categories")
                    .and_then(|v| v.as_array())
                    .into_iter()
                    .flatten()
                    .filter_map(|category| category.get("path").and_then(|v| v.as_str()))
                    .collect();
                let main = item
                    .get("mainGameItem")
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".into());
                let mut extra: Vec<String> = Vec::new();
                if let Some(object) = item.as_object() {
                    for key in object.keys() {
                        let lower = key.to_lowercase();
                        if lower.contains("dlc") || lower.contains("addon") || lower.contains("main") {
                            extra.push(key.clone());
                        }
                    }
                }
                println!(
                    "{namespace} · {id} · {title}\n   catégories : {}\n   mainGameItem : {}\n   clés : {:?}",
                    categories.join(" | "),
                    main,
                    extra
                );
            }
        }
    }
}
