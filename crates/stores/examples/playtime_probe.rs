//! Sonde : ce que la bibliothèque d'Epic publie comme temps de jeu.
//!
//! Lit les jetons que l'application garde pour le compte connecté, appelle le
//! service `playtime` de la bibliothèque et imprime la charge utile brute : on
//! voit enfin les noms de champs réels, au lieu de les supposer.

use std::fs;

fn main() {
    let home = std::env::var("HOME").unwrap_or_default();
    let path = format!("{home}/.local/share/lapinux/stores/epic.json");
    let raw = fs::read_to_string(&path).expect("jetons Epic illisibles");
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
        let keys: Vec<String> = value
            .as_object()
            .map(|o| o.keys().cloned().collect())
            .unwrap_or_default();
        println!("pas de jeton ; clés du fichier : {keys:?}");
        return;
    };
    let Some(user) = find("user_id") else {
        println!("pas d'identifiant de compte");
        return;
    };
    let url = format!(
        "https://library-service.live.use1a.on.epicgames.com/library/api/public/playtime/account/{user}/all"
    );
    let response = ureq::get(&url)
        .set("User-Agent", "UELauncher/14.0.0")
        .set("Authorization", &format!("Bearer {token}"))
        .timeout(std::time::Duration::from_secs(20))
        .call();
    match response {
        Ok(response) => {
            let text = response.into_string().unwrap_or_default();
            println!("200 · {} octets", text.len());
            println!("{text}");
        }
        Err(ureq::Error::Status(code, response)) => {
            println!(
                "HTTP {code} · {}",
                response.into_string().unwrap_or_default()
            );
        }
        Err(error) => println!("erreur réseau : {error}"),
    }
}
