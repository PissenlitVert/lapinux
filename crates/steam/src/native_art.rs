//! Illustrations depuis la boutique Steam.

use super::*;

/// Présentation et illustrations d'un jeu, lues sur la boutique publique : les
/// images du CDN se déduisent de l'appid, l'icône carrée vient du `clienticon`
/// que la boutique publie. Une clé activée n'a pas encore de fiche PICS ;
/// c'est ce qui lui donne son icône et ses images sans attendre une relève.
pub fn store_art(appid: u32) -> Option<lapinux_core::model::SteamDetails> {
    let client = reqwest::Client::builder()
        .user_agent(crate::tools::USER_AGENT)
        .timeout(STORE_TIMEOUT)
        .build()
        .ok()?;
    // Sans filtre : la boutique ne publie le hachage de l'icône carrée et les
    // genres qu'avec la fiche complète.
    let url = format!("{STORE_APP_URL}{appid}&l=french&cc=FR");
    let value: serde_json::Value = crate::shared::block_on(async {
        let response = client
            .get(&url)
            .header("accept-encoding", "identity")
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        response.json::<serde_json::Value>().await.ok()
    })?;
    let data = value.get(appid.to_string())?.get("data")?;
    let text = |key: &str| -> String {
        data.get(key)
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string()
    };
    let listed = |key: &str| -> Vec<String> {
        data.get(key)
            .and_then(|value| value.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        item.get("description")
                            .and_then(|found| found.as_str())
                            .or_else(|| item.as_str())
                    })
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut platforms = Vec::new();
    if let Some(flags) = data.get("platforms") {
        for (key, label) in [("windows", "Windows"), ("mac", "macOS"), ("linux", "Linux")] {
            if flags.get(key).and_then(|value| value.as_bool()) == Some(true) {
                platforms.push(label.to_string());
            }
        }
    }
    let cdn = "https://cdn.cloudflare.steamstatic.com/steam/apps";
    let kind = text("type");
    let mut details = lapinux_core::model::SteamDetails {
        header_image: format!("{cdn}/{appid}/header.jpg"),
        library_image: format!("{cdn}/{appid}/library_600x900.jpg"),
        hero_image: format!("{cdn}/{appid}/library_hero.jpg"),
        logo_image: format!("{cdn}/{appid}/logo.png"),
        capsule_image: format!("{cdn}/{appid}/capsule_231x87.jpg"),
        background_image: format!("{cdn}/{appid}/library_hero_blur.jpg"),
        app_type: if kind.eq_ignore_ascii_case("game") {
            "Game".to_string()
        } else if kind.is_empty() {
            String::new()
        } else {
            let mut letters = kind.chars();
            match letters.next() {
                Some(first) => first.to_uppercase().collect::<String>() + letters.as_str(),
                None => String::new(),
            }
        },
        developers: listed("developers"),
        publishers: listed("publishers"),
        genres: listed("genres"),
        categories: listed("categories"),
        platforms,
        homepage: text("website"),
        free: data
            .get("is_free")
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
        ..Default::default()
    };
    // Les jeux récents ne sont plus servis au chemin historique : la boutique
    // donne l'adresse hachée de leur bannière et de leur capsule.
    let header = text("header_image");
    if !header.is_empty() {
        details.header_image = header;
    }
    let capsule = text("capsule_image");
    let capsule = if capsule.is_empty() {
        text("capsule_imagev5")
    } else {
        capsule
    };
    if !capsule.is_empty() {
        details.capsule_image = capsule;
    }
    let icon = text("clienticon");
    if !icon.is_empty() {
        details.icon_image = format!(
            "https://cdn.cloudflare.steamstatic.com/steamcommunity/public/images/apps/{appid}/{icon}.jpg"
        );
    }
    Some(details)
}
