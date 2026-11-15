//! Notifications personnelles des boutiques hors Steam.
//!
//! Epic Games publie les demandes d'ami entrantes sur son service d'amis ;
//! GOG les expose sur le service de discussion de Galaxy. Les deux réponses
//! sont ramenées au même [`StoreNotification`]. itch.io n'expose aucune
//! notification personnelle dans son API : le refus est explicite, pour que
//! l'interface ne fasse pas croire à une boîte vide.

use crate::common;
use crate::common::{Error, Result};
use lapinux_core::stores::{Store, StoreNotification, StoreNotifications, StoreTokens};
use serde_json::Value;

/// Service d'amis d'Epic, le même que celui de sa liste d'amis.
const EPIC_FRIENDS: &str = "https://friends-public-service-prod.ol.epicgames.com";
/// Service de discussion de GOG Galaxy : il porte les invitations d'ami.
const GOG_CHAT: &str = "https://chat.gog.com";

/// Notifications personnelles d'un compte de boutique.
pub fn list(tokens: &StoreTokens) -> Result<StoreNotifications> {
    let store = tokens.store;
    let notifications = match store {
        Store::Epic => epic_incoming(tokens)?,
        Store::Gog => gog_invitations(tokens)?,
        Store::Itch => {
            return Err(Error::Unavailable(
                "itch.io n'expose pas de notifications personnelles dans son API",
            ))
        }
        Store::Steam => return Err(Error::Unavailable("Steam a sa propre lecture")),
    };
    Ok(StoreNotifications {
        store,
        connected: true,
        notifications,
        note: String::new(),
    })
}

/// Demandes d'ami reçues chez Epic.
fn epic_incoming(tokens: &StoreTokens) -> Result<Vec<StoreNotification>> {
    let user_id = tokens.user_id.trim();
    if user_id.is_empty() {
        return Err(Error::Expired);
    }
    let url = format!("{EPIC_FRIENDS}/friends/api/v1/{user_id}/incoming?displayNames=true");
    let value = common::get_json(&url, Some(&tokens.access_token))?;
    Ok(epic_notifications(&value))
}

/// Invitations d'ami reçues chez GOG.
fn gog_invitations(tokens: &StoreTokens) -> Result<Vec<StoreNotification>> {
    let user_id = tokens.user_id.trim();
    if user_id.is_empty() {
        return Err(Error::Expired);
    }
    let url = format!("{GOG_CHAT}/users/{user_id}/invitations");
    let value = common::get_json(&url, Some(&tokens.access_token))?;
    Ok(gog_notifications(&value))
}

/// Cartes des invitations Epic. La réponse du service est une liste, mais
/// certains proxys l'enveloppent dans `incoming` ou `friends`.
fn epic_notifications(value: &Value) -> Vec<StoreNotification> {
    let mut out = Vec::new();
    for entry in entries(value, &["incoming", "friends"]) {
        if entry.as_object().is_some_and(|entry| entry.is_empty()) {
            continue;
        }
        let name = text(entry.get("displayName"))
            .or_else(|| text(entry.get("alias")))
            .or_else(|| text(entry.get("name")))
            .unwrap_or_else(|| "Compte Epic".to_string());
        out.push(StoreNotification {
            store: Store::Epic,
            kind: "friend_request".into(),
            title: name,
            detail: "Demande d'ami en attente.".into(),
        });
    }
    out
}

/// Cartes des invitations GOG. Les comptes peuvent être à la racine de
/// l'entrée ou sous `user`, `inviter` ou `sender` selon la version du service.
fn gog_notifications(value: &Value) -> Vec<StoreNotification> {
    let mut out = Vec::new();
    for entry in entries(value, &["items", "invitations"]) {
        if entry.as_object().is_some_and(|entry| entry.is_empty()) {
            continue;
        }
        let user = entry
            .get("user")
            .or_else(|| entry.get("inviter"))
            .or_else(|| entry.get("sender"))
            .unwrap_or(entry);
        let name = text(user.get("username"))
            .or_else(|| text(entry.get("username")))
            .or_else(|| text(user.get("name")))
            .or_else(|| text(entry.get("name")))
            .unwrap_or_else(|| "Compte GOG".to_string());
        out.push(StoreNotification {
            store: Store::Gog,
            kind: "friend_request".into(),
            title: name,
            detail: "Demande d'ami en attente.".into(),
        });
    }
    out
}

/// Liste d'objets d'une réponse, qu'elle soit directe ou sous l'une des clés
/// d'enveloppe connues.
fn entries<'a>(value: &'a Value, keys: &[&str]) -> &'a [Value] {
    if let Some(items) = value.as_array() {
        return items;
    }
    for key in keys {
        if let Some(items) = value.get(*key).and_then(Value::as_array) {
            return items;
        }
    }
    &[]
}

/// Texte non vide d'un champ JSON.
fn text(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn epic_incoming_keeps_one_card_per_request() {
        let value = json!([
            { "accountId": "a", "displayName": "Alice" },
            { "accountId": "b", "alias": "Bob" },
            { "accountId": "c" }
        ]);
        let rows = epic_notifications(&value);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].store, Store::Epic);
        assert_eq!(rows[0].kind, "friend_request");
        assert_eq!(rows[0].title, "Alice");
        assert_eq!(rows[1].title, "Bob");
        assert_eq!(rows[2].title, "Compte Epic");
    }

    #[test]
    fn gog_invitations_accept_the_wrapped_and_nested_shapes() {
        let value = json!({
            "items": [
                { "user_id": "1", "username": "Alice" },
                { "inviter": { "username": "Bob" } }
            ]
        });
        let rows = gog_notifications(&value);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].store, Store::Gog);
        assert_eq!(rows[0].title, "Alice");
        assert_eq!(rows[1].title, "Bob");

        let bare = json!([{ "username": "Carol" }]);
        assert_eq!(gog_notifications(&bare)[0].title, "Carol");
    }

    #[test]
    fn an_empty_or_foreign_response_has_no_notification() {
        assert!(epic_notifications(&json!({})).is_empty());
        assert!(gog_notifications(&json!({ "error": "refusé" })).is_empty());
        assert!(entries(&json!({}), &["items"]).is_empty());
    }

    #[test]
    fn itch_has_no_personal_notification_api() {
        let tokens = StoreTokens {
            store: Store::Itch,
            access_token: "jeton".into(),
            ..StoreTokens::default()
        };
        let error = list(&tokens).expect_err("itch.io n'a pas de notifications");
        assert!(error.to_string().contains("itch.io"));
    }
}
