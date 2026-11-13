//! Amis des boutiques hors Steam, avec leur présence quand la boutique la
//! publie.
//!
//! Epic Games et GOG exposent chacun la liste d'amis de leur compte, par deux
//! services qui ne rendent pas la même forme : ce module les ramène au même
//! [`StoreFriend`]. Les pseudos et les avatars Epic viennent d'appels séparés,
//! et la présence d'un dernier appel groupé : le cercle « friends » du service
//! de présence Epic, et le service de statuts de GOG.
//! Un échec de présence ne fait pas perdre la liste ; la note du tableau dit
//! seulement qu'elle n'a pas pu être lue. itch.io n'a pas de liste d'amis dans
//! son API ; le refus est explicite plutôt qu'une liste vide qui laisserait
//! croire à un compte sans amis.

use crate::common;
use crate::common::{Error, Result};
use lapinux_core::social::{StoreFriend, StoreFriends};
use lapinux_core::stores::{Store, StoreTokens};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// Service d'amis du launcher Epic : le même que le sien, jeton du compte.
const EPIC_FRIENDS: &str = "https://friends-public-service-prod.ol.epicgames.com";
/// Service de comptes publics : il rend le pseudo d'un identifiant, y
/// compris quand la liste d'amis ne le porte pas.
const EPIC_ACCOUNT: &str = "https://account-public-service-prod.ol.epicgames.com";
/// Nombre d'identifiants qu'une recherche de comptes accepte d'un coup.
const EPIC_NAME_BATCH: usize = 100;
/// Service de présence Epic : le cercle « friends » rend tous les amis d'un
/// coup, avec leur état et le jeu en cours.
const EPIC_PRESENCE: &str = "https://presence-public-service-prod.ol.epicgames.com";
/// GraphQL du launcher : le profil public d'un compte y porte son avatar
/// officiel, une lettre colorée ou l'avatar social du compte. La liste
/// d'amis ne le porte pas ; il faut demander un profil par ami.
const EPIC_GQL: &str = "https://launcher.store.epicgames.com/graphql";
/// Agent du launcher : son GraphQL attend le sien, comme la vitrine.
const EPIC_UA: &str = "EpicGamesLauncher/14.0.8-22004686+++Portal+Release-Live";
/// Le GraphQL du launcher n'accepte qu'un profil par requête et limite le
/// nombre de requêtes d'un document ; on borne donc les appels simultanés.
const EPIC_AVATAR_PARALLEL: usize = 6;
/// Service de discussion de GOG Galaxy : il porte la liste d'amis du compte.
const GOG_CHAT: &str = "https://chat.gog.com";
/// Service de statuts GOG : il dit quels comptes sont en ligne maintenant.
const GOG_PRESENCE: &str = "https://presence.gog.com";
/// Le service de statuts GOG accepte une liste d'identifiants ; on la borne
/// pour ne pas dépasser ce qu'une adresse sait porter.
const GOG_STATUS_BATCH: usize = 200;

/// Liste d'amis d'un compte de boutique, avec la présence quand elle est
/// disponible.
pub fn list(tokens: &StoreTokens) -> Result<StoreFriends> {
    let store = tokens.store;
    let friends = match store {
        Store::Epic => epic_list(tokens)?,
        Store::Gog => gog_list(tokens)?,
        Store::Itch => {
            return Err(Error::Unavailable(
                "itch.io ne publie pas de liste d'amis : son API n'en expose aucune".into(),
            ))
        }
        Store::Steam => return Err(Error::Unavailable("Steam a sa propre liste d'amis")),
    };
    let mut board = StoreFriends {
        store,
        connected: true,
        friends,
        note: String::new(),
    };
    match store {
        Store::Epic => {
            if let Err(error) = epic_names_into(&mut board, tokens) {
                add_note(&mut board, format!("pseudos indisponibles : {error}"));
            }
            epic_avatars_into(&mut board, tokens);
            epic_presence_into(&mut board, tokens);
        }
        Store::Gog => gog_presence_into(&mut board, tokens),
        _ => {}
    }
    Ok(board)
}

/// Ajoute une note au tableau, sans écraser celle d'un autre service.
fn add_note(board: &mut StoreFriends, text: String) {
    if board.note.is_empty() {
        board.note = text;
    } else {
        board.note.push_str(" · ");
        board.note.push_str(&text);
    }
}

/// Amis Epic, avant la présence : une entrée par compte.
fn epic_list(tokens: &StoreTokens) -> Result<Vec<StoreFriend>> {
    let user_id = tokens.user_id.trim();
    if user_id.is_empty() {
        return Err(Error::Expired);
    }
    let url = format!("{EPIC_FRIENDS}/friends/api/v1/{user_id}/friends?displayNames=true");
    Ok(epic_friends(&common::get_json(
        &url,
        Some(&tokens.access_token),
    )?))
}

/// Amis GOG, avant la présence : une entrée par compte.
fn gog_list(tokens: &StoreTokens) -> Result<Vec<StoreFriend>> {
    if tokens.user_id.trim().is_empty() {
        return Err(Error::Expired);
    }
    let url = format!("{GOG_CHAT}/users/{}/friends", tokens.user_id.trim());
    Ok(gog_friends(&common::get_json(
        &url,
        Some(&tokens.access_token),
    )?))
}

/// Complète les pseudos Epic : la liste d'amis ne les porte pas toujours,
/// même avec `displayNames=true`. Le service de comptes publics les rend par
/// lots de cent identifiants.
fn epic_names_into(board: &mut StoreFriends, tokens: &StoreTokens) -> Result<()> {
    let missing: Vec<String> = board
        .friends
        .iter()
        .filter(|friend| friend.name.trim().is_empty() || friend.name == friend.id)
        .map(|friend| friend.id.clone())
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    let mut names = BTreeMap::new();
    for chunk in missing.chunks(EPIC_NAME_BATCH) {
        let mut url = format!("{EPIC_ACCOUNT}/account/api/public/account");
        for (index, id) in chunk.iter().enumerate() {
            url.push(if index == 0 { '?' } else { '&' });
            url.push_str("accountId=");
            url.push_str(id);
        }
        let value = common::get_json(&url, Some(&tokens.access_token))?;
        names.extend(epic_account_names(&value));
    }
    for friend in &mut board.friends {
        if let Some(name) = names.get(&friend.id) {
            friend.name = name.clone();
        }
        // Dernier recours : un identifiant de 32 caractères n'est pas un nom.
        if friend.name.trim().is_empty() || friend.name == friend.id {
            friend.name = short_account(&friend.id);
        }
    }
    Ok(())
}

/// Pseudos d'une réponse de comptes publics, indexés par identifiant.
fn epic_account_names(value: &Value) -> BTreeMap<String, String> {
    let mut names = BTreeMap::new();
    for account in value.as_array().into_iter().flatten() {
        let Some(id) = account.get("id").and_then(Value::as_str) else {
            continue;
        };
        let name = account
            .get("displayName")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if !name.is_empty() {
            names.insert(id.to_string(), name.to_string());
        }
    }
    names
}

/// Nom de repli quand le service ne rend pas le pseudo : la fin de
/// l'identifiant distingue deux comptes sans afficher trente-deux caractères.
fn short_account(id: &str) -> String {
    let suffix: String = id
        .chars()
        .rev()
        .take(4)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    if suffix.is_empty() {
        "Compte Epic".to_string()
    } else {
        format!("Compte Epic {suffix}")
    }
}
/// Complète les avatars Epic : le GraphQL du launcher rend l'icône officielle
/// du profil, une lettre colorée ou l'avatar social du compte. La liste
/// d'amis ne porte pas cette image et le resolver n'accepte qu'un profil par
/// requête : on interroge les amis par petits groupes. Un profil privé ou un
/// échec garde l'initiale dessinée par l'interface.
fn epic_avatars_into(board: &mut StoreFriends, tokens: &StoreTokens) {
    if board.friends.is_empty() {
        return;
    }
    let token = &tokens.access_token;
    let avatars: std::sync::Mutex<BTreeMap<String, String>> =
        std::sync::Mutex::new(BTreeMap::new());
    for chunk in board.friends.chunks(EPIC_AVATAR_PARALLEL) {
        std::thread::scope(|scope| {
            for friend in chunk {
                if !epic_account_id(&friend.id) {
                    continue;
                }
                let avatars = &avatars;
                scope.spawn(move || {
                    if let Some(url) = epic_avatar(&friend.id, token) {
                        if let Ok(mut avatars) = avatars.lock() {
                            avatars.insert(friend.id.clone(), url);
                        }
                    }
                });
            }
        });
    }
    let avatars = avatars.into_inner().unwrap_or_default();
    for friend in &mut board.friends {
        if let Some(avatar) = avatars.get(&friend.id) {
            friend.avatar = avatar.clone();
        }
    }
}

/// Avatar officiel d'un compte Epic, ou `None` si le profil est privé, absent
/// ou si le service ne répond pas.
fn epic_avatar(id: &str, token: &str) -> Option<String> {
    if !epic_account_id(id) {
        return None;
    }
    let query = format!(
        "query {{ PlayerProfile {{ playerProfile(epicAccountId: \"{id}\") {{ avatar {{ medium }} }} }} }}"
    );
    let body = serde_json::json!({ "query": query });
    let value = crate::cloud::post_json(EPIC_GQL, Some(token), &body, Some(EPIC_UA)).ok()?;
    epic_avatar_url(&value)
}

/// Adresse de l'avatar dans une réponse `PlayerProfile`.
fn epic_avatar_url(value: &Value) -> Option<String> {
    value
        .pointer("/data/PlayerProfile/playerProfile/avatar/medium")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(str::to_string)
}

/// Un identifiant de compte Epic est hexadécimal sur 32 caractères ; on le
/// vérifie avant de l'insérer dans la requête GraphQL.
fn epic_account_id(id: &str) -> bool {
    id.len() == 32 && id.chars().all(|c| c.is_ascii_hexdigit())
}

fn epic_presence_into(board: &mut StoreFriends, tokens: &StoreTokens) {
    if board.friends.is_empty() {
        return;
    }
    let user_id = tokens.user_id.trim();
    let url = format!("{EPIC_PRESENCE}/presence/api/v1/_/{user_id}/presence?circle=friends");
    let value = match common::get_json(&url, Some(&tokens.access_token)) {
        Ok(value) => value,
        Err(error) => {
            add_note(board, presence_note(&error));
            return;
        }
    };
    let Some(presences) = epic_presence(&value) else {
        return;
    };
    for friend in &mut board.friends {
        match presences.get(&friend.id) {
            Some((status, game)) => {
                friend.status = status.clone();
                friend.game = game.clone();
            }
            // Le cercle ne porte que les comptes connectés : un ami absent de
            // la réponse est hors ligne, pas « état inconnu ».
            None => friend.status = "offline".into(),
        }
    }
}

/// Note de présence Epic : un 403 est un refus de permission, pas une session
/// à renouveler ; le 401 garde le message de reconnexion.
fn presence_note(error: &Error) -> String {
    match error {
        Error::Refused { status: 403, .. } => {
            "présence indisponible : le jeton du launcher n'a pas la permission de présence".into()
        }
        other => format!("présence indisponible : {other}"),
    }
}

/// Complète la liste GOG avec le service de statuts, par lots d'identifiants.
fn gog_presence_into(board: &mut StoreFriends, tokens: &StoreTokens) {
    if board.friends.is_empty() {
        return;
    }
    let ids: Vec<String> = board
        .friends
        .iter()
        .map(|friend| friend.id.clone())
        .collect();
    let online = match gog_statuses(&ids, &tokens.access_token) {
        Ok(online) => online,
        Err(error) => {
            add_note(board, format!("présence indisponible : {error}"));
            return;
        }
    };
    for friend in &mut board.friends {
        friend.status = if online.contains(&friend.id) {
            "online"
        } else {
            "offline"
        }
        .into();
    }
}

/// Identifiants en ligne chez GOG, tous lots confondus.
fn gog_statuses(ids: &[String], token: &str) -> Result<BTreeSet<String>> {
    let mut online = BTreeSet::new();
    for chunk in ids.chunks(GOG_STATUS_BATCH) {
        let url = format!("{GOG_PRESENCE}/statuses?user_id={}", chunk.join(","));
        let value = common::get_json(&url, Some(token))?;
        online.extend(gog_online(&value)?);
    }
    Ok(online)
}

/// Comptes en ligne d'une réponse de `presence.gog.com/statuses`.
fn gog_online(value: &Value) -> Result<BTreeSet<String>> {
    let items = value
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::Parse("statuts GOG sans liste".into()))?;
    Ok(items
        .iter()
        .filter_map(|item| {
            item.get("user_id").and_then(|id| {
                id.as_str()
                    .map(str::to_string)
                    .or_else(|| id.as_u64().map(|id| id.to_string()))
            })
        })
        .collect())
}

/// Amis d'une réponse Epic : une entrée par compte, le nom d'affichage quand
/// la requête l'a demandé, sinon l'alias et à défaut l'identifiant.
fn epic_friends(value: &Value) -> Vec<StoreFriend> {
    let mut friends: Vec<StoreFriend> = value
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|entry| {
            let id = entry
                .get("accountId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_string();
            if id.is_empty() {
                return None;
            }
            let alias = entry
                .get("alias")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_string();
            let display = entry
                .get("displayName")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_string();
            let name = if !display.is_empty() {
                display
            } else if !alias.is_empty() {
                alias.clone()
            } else {
                id.clone()
            };
            Some(StoreFriend {
                store: Store::Epic,
                id,
                name,
                alias,
                ..StoreFriend::default()
            })
        })
        .collect();
    friends.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then(left.id.cmp(&right.id))
    });
    friends
}

/// Amis d'une réponse GOG : `items` porte les comptes, avec leur photo moyenne.
fn gog_friends(value: &Value) -> Vec<StoreFriend> {
    let entries = value
        .get("items")
        .and_then(Value::as_array)
        .or_else(|| value.as_array());
    let mut friends: Vec<StoreFriend> = entries
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|entry| {
            let id = entry
                .get("user_id")
                .and_then(|id| {
                    id.as_str()
                        .map(str::to_string)
                        .or_else(|| id.as_u64().map(|id| id.to_string()))
                })
                .unwrap_or_default();
            if id.is_empty() {
                return None;
            }
            let name = entry
                .get("username")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_string();
            let avatar = entry
                .pointer("/images/medium")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            Some(StoreFriend {
                store: Store::Gog,
                id,
                name,
                avatar,
                ..StoreFriend::default()
            })
        })
        .collect();
    friends.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then(left.id.cmp(&right.id))
    });
    friends
}

/// Présences Epic, indexées par compte : état et jeu en cours.
///
/// `None` quand la réponse ne porte pas le cercle « friends » : l'appelant
/// laisse alors les états vides plutôt que de déclarer tout le monde absent.
fn epic_presence(value: &Value) -> Option<BTreeMap<String, (String, String)>> {
    let presences = epic_presence_list(value)?;
    let mut out = BTreeMap::new();
    for presence in presences {
        let account = presence
            .get("accountId")
            .and_then(Value::as_str)
            .or_else(|| presence.pointer("/account/id").and_then(Value::as_str))
            .unwrap_or_default()
            .trim();
        if account.is_empty() {
            continue;
        }
        let raw = presence
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase();
        let status = match raw.as_str() {
            "online" => "online",
            "away" | "xa" | "dnd" | "busy" => "away",
            "offline" => "offline",
            _ => "",
        };
        let game = if status == "offline" {
            String::new()
        } else {
            epic_game(presence)
        };
        let status = if status.is_empty() {
            if game.is_empty() {
                continue;
            }
            "in-game"
        } else if game.is_empty() {
            status
        } else {
            "in-game"
        };
        out.insert(account.to_string(), (status.to_string(), game));
    }
    Some(out)
}

/// Liste des présences d'une réponse Epic, quelle que soit son enveloppe :
/// cercles (`circle: friends`), liste directe, ou objet `friends`.
fn epic_presence_list(value: &Value) -> Option<&[Value]> {
    if let Some(circles) = value.as_array() {
        if let Some(friends) = circles
            .iter()
            .find(|circle| circle.get("circle").and_then(Value::as_str) == Some("friends"))
        {
            if let Some(presences) = friends.get("presences").and_then(Value::as_array) {
                return Some(presences);
            }
        }
        // Réponse déjà réduite à la liste des présences.
        if circles.iter().any(|entry| entry.get("accountId").is_some()) {
            return Some(circles);
        }
        for circle in circles {
            if let Some(presences) = circle.get("presences").and_then(Value::as_array) {
                return Some(presences);
            }
        }
    }
    for key in ["presences", "friends"] {
        if let Some(presences) = value.get(key).and_then(Value::as_array) {
            return Some(presences);
        }
    }
    None
}

/// Nom du jeu d'une présence Epic : application lancée par le launcher,
/// produit d'un espace de noms de jeu, puis activité publiée par le jeu.
fn epic_game(presence: &Value) -> String {
    let Some(namespaces) = presence.get("perNs").and_then(Value::as_array) else {
        return String::new();
    };
    let mut running = String::new();
    let mut product = String::new();
    let mut activity = String::new();
    for entry in namespaces {
        if running.is_empty() {
            if let Some(connections) = entry.get("conns").and_then(Value::as_array) {
                for connection in connections {
                    running = json_text(connection.pointer("/runningApp/displayName"));
                    if !running.is_empty() {
                        break;
                    }
                }
            }
        }
        let props = entry.get("props");
        if running.is_empty() {
            running = json_text(props.and_then(|props| props.get("running_app_name")));
        }
        // L'espace de noms « _ » est celui du launcher : son activité est un
        // statut libre, pas un jeu.
        if entry.get("ns").and_then(Value::as_str).unwrap_or("_") == "_" {
            continue;
        }
        if product.is_empty() {
            for key in ["X_NonEOS_ProductName", "EOS_ProductName"] {
                product = json_text(props.and_then(|props| props.get(key)));
                if !product.is_empty() {
                    break;
                }
            }
        }
        if activity.is_empty() {
            activity = json_text(entry.pointer("/activity/value"));
        }
    }
    if !running.is_empty() {
        running
    } else if !product.is_empty() {
        product
    } else {
        activity
    }
}

/// Texte non vide d'une valeur JSON, sans espaces superflus.
fn json_text(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_forbidden_presence_is_not_an_expired_session() {
        let note = presence_note(&Error::Refused {
            status: 403,
            message: "Forbidden".into(),
        });
        assert!(note.contains("permission"), "{note}");
        assert!(!note.contains("session expirée"), "{note}");
        let note = presence_note(&Error::Expired);
        assert!(note.contains("session expirée"), "{note}");
    }

    #[test]
    fn epic_friends_keep_the_displayed_name_and_the_alias() {
        let value = serde_json::json!([
            { "accountId": "abc", "displayName": "Alice", "alias": "Lili" },
            { "accountId": "def", "alias": "Bob" },
            { "accountId": "" }
        ]);
        let friends = epic_friends(&value);
        assert_eq!(friends.len(), 2);
        assert_eq!(friends[0].name, "Alice");
        assert_eq!(friends[0].alias, "Lili");
        assert_eq!(friends[1].name, "Bob");
        assert_eq!(friends[0].store, Store::Epic);
    }

    #[test]
    fn gog_friends_come_from_items_with_their_avatar() {
        let value = serde_json::json!({
            "items": [
                {
                    "user_id": "46988961654682898",
                    "username": "adaliabooks",
                    "images": { "medium": "https://images.gog.com/abc_avm.jpg" }
                },
                { "user_id": 42, "username": "Zoe" }
            ]
        });
        let friends = gog_friends(&value);
        assert_eq!(friends.len(), 2);
        assert_eq!(friends[0].name, "adaliabooks");
        assert_eq!(friends[0].id, "46988961654682898");
        assert_eq!(friends[0].avatar, "https://images.gog.com/abc_avm.jpg");
        assert_eq!(friends[1].id, "42");
        assert_eq!(friends[0].store, Store::Gog);
    }

    #[test]
    fn epic_presence_reads_the_friends_circle_and_the_running_game() {
        let value = serde_json::json!([
            {
                "circle": "self",
                "presences": [{ "accountId": "moi", "status": "online" }]
            },
            {
                "circle": "friends",
                "presences": [
                    {
                        "accountId": "a",
                        "status": "online",
                        "perNs": [{
                            "ns": "_",
                            "props": {},
                            "conns": [{ "runningApp": { "displayName": "Genshin Impact" } }]
                        }]
                    },
                    {
                        "accountId": "b",
                        "status": "xa",
                        "perNs": [{
                            "ns": "62a9473a2dca46b29ccf17577fcf42d7",
                            "activity": { "value": "Twine Peaks (140)" },
                            "props": { "EOS_ProductName": "Fortnite" }
                        }]
                    },
                    {
                        "accountId": "c",
                        "status": "offline",
                        "perNs": [{
                            "ns": "62a9473a2dca46b29ccf17577fcf42d7",
                            "activity": { "value": "vieux statut" },
                            "props": { "EOS_ProductName": "Fortnite" }
                        }]
                    },
                    {
                        "accountId": "d",
                        "status": "online",
                        "perNs": [{ "ns": "62a9473a2dca46b29ccf17577fcf42d7", "activity": { "value": "Lobby - 1 / 16" } }]
                    }
                ]
            }
        ]);
        let presences = epic_presence(&value).expect("cercle des amis");
        // Une application lancée par le launcher vaut un jeu en cours.
        assert_eq!(
            presences.get("a"),
            Some(&("in-game".to_string(), "Genshin Impact".to_string()))
        );
        assert_eq!(
            presences.get("b"),
            Some(&("in-game".to_string(), "Fortnite".to_string()))
        );
        // Un ami hors ligne garde son état, jamais un jeu périmé.
        assert_eq!(
            presences.get("c"),
            Some(&("offline".to_string(), String::new()))
        );
        assert_eq!(
            presences.get("d"),
            Some(&("in-game".to_string(), "Lobby - 1 / 16".to_string()))
        );
        assert!(!presences.contains_key("moi"));
        assert!(epic_presence(&serde_json::json!({})).is_none());
    }

    #[test]
    fn epic_presence_accepts_other_envelopes() {
        let bare = serde_json::json!([{ "accountId": "a", "status": "online" }]);
        let presences = epic_presence(&bare).expect("liste directe");
        assert_eq!(
            presences.get("a"),
            Some(&("online".to_string(), String::new()))
        );

        let wrapped = serde_json::json!({
            "friends": [{ "account": { "id": "b" }, "status": "away" }]
        });
        let presences = epic_presence(&wrapped).expect("objet friends");
        assert_eq!(
            presences.get("b"),
            Some(&("away".to_string(), String::new()))
        );
    }

    #[test]
    fn epic_accounts_give_their_display_names() {
        let value = serde_json::json!([
            { "id": "a", "displayName": "Alice" },
            { "id": "b", "displayName": "  " },
            { "id": "c" }
        ]);
        let names = epic_account_names(&value);
        assert_eq!(names.get("a").map(String::as_str), Some("Alice"));
        assert!(!names.contains_key("b"));
        assert!(!names.contains_key("c"));
    }

    #[test]
    fn an_unresolved_account_gets_a_short_label() {
        assert_eq!(
            short_account("0123456789abcdef0123456789abcdef"),
            "Compte Epic cdef"
        );
        assert_eq!(short_account(""), "Compte Epic");
    }

    #[test]
    fn gog_statuses_list_only_the_accounts_online() {
        let value = serde_json::json!({
            "total_count": 2,
            "items": [
                { "user_id": "46988961654682898", "client_id": "1", "data": {} },
                { "user_id": 47510856674996320u64, "client_id": "1", "data": {} }
            ]
        });
        let online = gog_online(&value).expect("statuts");
        assert!(online.contains("46988961654682898"));
        assert!(online.contains("47510856674996320"));
        assert!(!online.contains("47510856674996321"));
        assert!(gog_online(&serde_json::json!({})).is_err());
    }

    #[test]
    fn an_epic_profile_gives_its_official_avatar() {
        let value = serde_json::json!({
            "data": { "PlayerProfile": { "playerProfile": {
                "avatar": {
                    "medium": "https://shared-static-prod.epicgames.com/epic-profile-icon/7B009C/N/icon.png?size=128"
                }
            }}}
        });
        assert_eq!(
            epic_avatar_url(&value).as_deref(),
            Some(
                "https://shared-static-prod.epicgames.com/epic-profile-icon/\
                 7B009C/N/icon.png?size=128"
            )
        );
        assert!(epic_avatar_url(&serde_json::json!({
            "data": { "PlayerProfile": { "playerProfile": null } }
        }))
        .is_none());
        assert!(epic_avatar_url(&serde_json::json!({})).is_none());
    }

    #[test]
    fn only_epic_account_ids_are_queried() {
        assert!(epic_account_id("0123456789abcdef0123456789abcdef"));
        assert!(epic_account_id("0123456789ABCDEF0123456789ABCDEF"));
        assert!(!epic_account_id(""));
        assert!(!epic_account_id("0123456789abcdef"));
        assert!(!epic_account_id("0123456789abcdef0123456789abcde\""));
        assert!(!epic_account_id("0123456789abcdef0123456789abcdeg"));
    }

    #[test]
    fn an_empty_response_lists_no_friend() {
        assert!(epic_friends(&serde_json::json!({})).is_empty());
        assert!(gog_friends(&serde_json::json!({})).is_empty());
    }
}
