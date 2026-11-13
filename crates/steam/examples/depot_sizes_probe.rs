//! Diagnostic : dépôts d'un jeu et poids de leurs manifestes publics.

use lapinux_steam::session;
use steamroom::apps::AccessToken;
use steamroom::depot::AppId;
use steamroom::types::key_value::{KeyValue, KvValue};
use steamroom_client::login::LoginBuilder;

fn field<'a>(node: &'a KeyValue, key: &str) -> Option<&'a KeyValue> {
    node.get(key)
}

fn text(node: &KeyValue, key: &str) -> String {
    field(node, key)
        .and_then(KeyValue::as_str)
        .unwrap_or_default()
        .to_string()
}

fn number(node: &KeyValue, key: &str) -> u64 {
    field(node, key)
        .and_then(KeyValue::as_str)
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

fn main() {
    let appids: Vec<u32> = std::env::args()
        .skip(1)
        .filter_map(|arg| arg.parse().ok())
        .collect();
    let saved = session::load().expect("aucune session Steam enregistrée");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async move {
        let client = LoginBuilder::new()
            .device_name("Lapinux (poids des dépôts)")
            .with_refresh_token(&saved.account_name, &saved.refresh_token)
            .login()
            .await
            .unwrap();
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
            let Some(bytes) = info.kv_data else { continue };
            let value = if bytes.first() == Some(&0) {
                KeyValue::from_binary(&bytes).ok()
            } else {
                KeyValue::from_text(&String::from_utf8_lossy(&bytes)).ok()
            };
            let Some(value) = value else { continue };
            let app = value.get("appinfo").unwrap_or(&value);
            let Some(depots) = app.get("depots") else { continue };
            let KvValue::Children(entries) = &depots.value else {
                continue;
            };
            let mut total = 0u64;
            for depot in entries.values() {
                let key = depot.key.clone();
                if key.parse::<u32>().is_err() {
                    continue;
                }
                let KvValue::Children(_) = &depot.value else {
                    continue;
                };
                let size = field(depot, "manifests")
                    .and_then(|manifests| field(manifests, "public"))
                    .map(|public| number(public, "size"))
                    .unwrap_or(0);
                total += size;
                println!(
                    "dépôt {key:<9} os={:<10} langue={:<10} dlc={:<8} partagé={} depuis={:<8} taille={:>7} Go",
                    text(
                        field(depot, "config").unwrap_or(depot),
                        "oslist"
                    ),
                    text(field(depot, "config").unwrap_or(depot), "language"),
                    text(depot, "dlcappid"),
                    if field(depot, "sharedinstall").is_some() {
                        "oui"
                    } else {
                        "non"
                    },
                    text(depot, "depotfromapp"),
                    size as f64 / 1e9,
                );
            }
            println!("somme brute : {:.1} Go", total as f64 / 1e9);
        }
    });
}
