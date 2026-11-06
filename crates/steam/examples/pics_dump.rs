//! Affiche la structure PICS d'une ou plusieurs applications (diagnostic).

use lapinux_steam::session;
use steamroom::apps::AccessToken;
use steamroom::depot::AppId;
use steamroom::types::key_value::{KvValue, KeyValue};
use steamroom_client::login::LoginBuilder;

fn walk(node: &KeyValue, prefix: &str, depth: usize) {
    if depth > 7 {
        return;
    }
    let path = if prefix.is_empty() {
        node.key.clone()
    } else {
        format!("{prefix}.{}", node.key)
    };
    match &node.value {
        KvValue::Children(children) => {
            for child in children.values() {
                walk(child, &path, depth + 1);
            }
        }
        KvValue::String(text) => {
            let shown: String = text.chars().take(90).collect();
            println!("{path} = {shown}");
        }
        other => println!("{path} = {other:?}"),
    }
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
            .device_name("Lapinux (dump PICS)")
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
        let infos = client.pics_get_product_info(&tokens).await.unwrap();
        for info in infos {
            let Some(bytes) = info.kv_data else {
                continue;
            };
            let value = if bytes.first() == Some(&0) {
                KeyValue::from_binary(&bytes).ok()
            } else {
                KeyValue::from_text(&String::from_utf8_lossy(&bytes)).ok()
            };
            if let Some(value) = value {
                walk(&value, "", 0);
            }
        }
    });
}
