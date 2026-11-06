//! Succès d'un jeu lus par la session Steam, sans clé Web API.
//!
//! Deux appels de service sur la même connexion CM suffisent :
//!
//! - `Player.GetGameAchievements#1` publie le schéma du jeu — nom interne, nom
//!   localisé, description, icône, succès caché, identifiant interne — et la
//!   part des joueurs qui possèdent chaque succès (`player_percent_unlocked`) ;
//! - `Player.GetUserAchievements#1` rend l'état du compte : pour chaque
//!   identifiant interne, débloqué ou non et la date.
//!
//! Le rapprochement se fait par identifiant interne, pas par position : c'est
//! ce que tient le client de Valve, et ce qui laisse la lecture correcte quand
//! Steam omet les succès verrouillés d'une réponse.

use crate::cm::proto::{field_bytes, field_varint, Field, Reader};
use crate::gamedata::Sources;
use crate::Error;
use lapinux_core::model::{SteamAchievement, SteamAchievements};
use std::collections::HashMap;
use std::time::Duration;
use steamroom::client::{LoggedIn, SteamClient};

/// Schéma des succès d'un jeu, localisé.
const SCHEMA_METHOD: &str = "Player.GetGameAchievements#1";
/// État des succès du compte, par identifiant interne.
const USER_METHOD: &str = "Player.GetUserAchievements#1";
/// Icônes officielles : le protocole ne publie que le nom du fichier.
const ICON_BASE: &str =
    "https://cdn.cloudflare.steamstatic.com/steamcommunity/public/images/apps";
/// Langue demandée au schéma : les noms et descriptions s'affichent en français.
const LANGUAGE: &str = "french";
/// Un CM qui ne répond pas en un quart de minute est perdu.
const REPLY_TIMEOUT: Duration = Duration::from_secs(20);

/// Lit les succès du jeu pour le compte de la session enregistrée.
///
/// Ne rend jamais d'erreur : ce qui n'a pas pu être lu laisse une note, comme
/// la fiche complète du jeu.
pub fn game_achievements(appid: u32, sources: &Sources) -> SteamAchievements {
    let mut read = SteamAchievements {
        appid,
        fetched: lapinux_core::util::now_unix(),
        ..Default::default()
    };
    if !sources.has_session() {
        read.notes
            .push("compte Steam non connecté : les succès se lisent avec la session".into());
        return read;
    }
    // Session partagée : la fiche redemande les succès souvent, chaque demande
    // ne doit pas coûter une connexion. Elle vit sur le moteur commun, sans
    // quoi la connexion mourrait avec le runtime de cette lecture.
    let client = crate::shared::block_on(crate::shared::session(
        &sources.account,
        &sources.refresh_token,
    ));
    match client {
        Ok(client) => crate::shared::block_on(read_with(&client, appid, sources.steam_id)),
        Err(error) => {
            read.notes.push(format!("session Steam : {error}"));
            read
        }
    }
}

/// Schéma et état du compte sur une connexion déjà ouverte : la fiche complète
/// du jeu réutilise sa session au lieu d'en ouvrir une seconde. Ne rend jamais
/// d'erreur, chaque étape manquée laisse une note.
pub async fn read_with(
    client: &SteamClient<LoggedIn>,
    appid: u32,
    steam_id: u64,
) -> SteamAchievements {
    let mut read = SteamAchievements {
        appid,
        fetched: lapinux_core::util::now_unix(),
        ..Default::default()
    };
    let entries = match schema(client, appid).await {
        Ok(entries) => entries,
        Err(error) => {
            read.notes.push(format!("schéma des succès : {error}"));
            return read;
        }
    };
    if entries.is_empty() {
        read.notes.push("ce jeu ne publie pas de succès".into());
        return read;
    }
    match unlocks(client, appid, steam_id).await {
        Ok(times) => read.items = merge_unlocks(entries, &times),
        Err(error) => {
            read.notes.push(format!("déblocage : {error}"));
            read.items = entries
                .into_iter()
                .map(|(_, achievement)| achievement)
                .collect();
        }
    }
    // Le temps de jeu suit la même session : l'embed du jeu peut l'afficher
    // sans attendre la fiche complète. Son échec ne compromet pas la lecture.
    // Un jeu prêté par la famille n'apparaît qu'avec l'identifiant du groupe :
    // il n'est demandé que si la première réponse l'ignore.
    let mut play = crate::playtime::game_playtime(client, appid, steam_id, None)
        .await
        .ok()
        .flatten();
    if play.is_none() {
        if let Some(group) = crate::playtime::family_group_id(client, steam_id).await {
            play = crate::playtime::game_playtime(client, appid, steam_id, Some(group))
                .await
                .ok()
                .flatten();
        }
    }
    if let Some(play) = play {
        read.play_minutes = Some(play.minutes_total);
    }
    read
}

/// Schéma du jeu : une entrée par succès, avec son identifiant interne.
async fn schema(
    client: &SteamClient<LoggedIn>,
    appid: u32,
) -> Result<Vec<(u64, SteamAchievement)>, Error> {
    let mut body = Vec::new();
    field_varint(1, u64::from(appid), &mut body);
    field_bytes(2, LANGUAGE.as_bytes(), &mut body);
    let answer = tokio::time::timeout(
        REPLY_TIMEOUT,
        client.call_service_method(SCHEMA_METHOD, &body),
    )
    .await
    .map_err(|_| Error::Other("Steam n'a pas répondu".into()))?
    .map_err(|error| Error::Other(error.to_string()))?;
    Ok(parse_schema(&answer.body, appid))
}

/// Dates de déblocage du compte, rangées par identifiant interne.
async fn unlocks(
    client: &SteamClient<LoggedIn>,
    appid: u32,
    steam_id: u64,
) -> Result<HashMap<u64, u32>, Error> {
    let mut body = Vec::new();
    field_varint(1, steam_id, &mut body);
    field_varint(2, u64::from(appid), &mut body);
    let answer = tokio::time::timeout(REPLY_TIMEOUT, client.call_service_method(USER_METHOD, &body))
        .await
        .map_err(|_| Error::Other("Steam n'a pas répondu".into()))?
        .map_err(|error| Error::Other(error.to_string()))?;
    Ok(parse_unlocks(&answer.body))
}

/// Entrées du schéma, triées par identifiant interne.
fn parse_schema(body: &[u8], appid: u32) -> Vec<(u64, SteamAchievement)> {
    let mut entries: Vec<(u64, SteamAchievement)> = Vec::new();
    let mut reader = Reader::new(body);
    while let Some((number, field)) = reader.next() {
        let Field::Bytes(entry) = field else {
            continue;
        };
        if number != 1 {
            continue;
        }
        let mut achievement = SteamAchievement::default();
        let mut key = 0u64;
        let mut icon = String::new();
        let mut reader = Reader::new(entry);
        while let Some((number, field)) = reader.next() {
            match (number, field) {
                (1, Field::Bytes(value)) => achievement.api_name = text(value),
                (2, Field::Bytes(value)) => achievement.name = text(value),
                (3, Field::Bytes(value)) => achievement.description = text(value),
                (4, Field::Bytes(value)) => icon = text(value),
                // Rareté mondiale, publiée en chaîne (« 74.0 »).
                (7, Field::Bytes(value)) => achievement.percent = text(value).trim().parse().ok(),
                (6, Field::Varint(value)) => achievement.hidden = value != 0,
                (8, Field::Varint(value)) => key = value,
                _ => {}
            }
        }
        if achievement.api_name.is_empty() {
            continue;
        }
        // Un succès sans nom localisé garde son nom technique : mieux vaut le
        // montrer que laisser une case vide.
        if achievement.name.trim().is_empty() {
            achievement.name = achievement.api_name.clone();
        }
        if !icon.is_empty() {
            achievement.icon = format!("{ICON_BASE}/{appid}/{icon}");
        }
        entries.push((key, achievement));
    }
    entries.sort_by_key(|(key, _)| *key);
    entries
}

/// Succès débloqués du compte : identifiant interne → date de déblocage.
///
/// Steam n'envoie que les succès obtenus ; les autres restent absents, ce qui
/// les laisse verrouillés.
fn parse_unlocks(body: &[u8]) -> HashMap<u64, u32> {
    let mut times = HashMap::new();
    let mut reader = Reader::new(body);
    while let Some((number, field)) = reader.next() {
        let Field::Bytes(entry) = field else {
            continue;
        };
        if number != 1 {
            continue;
        }
        let mut key = 0u64;
        let mut unlocked = false;
        let mut time = 0u32;
        let mut reader = Reader::new(entry);
        while let Some((number, field)) = reader.next() {
            match (number, field) {
                (1, Field::Varint(value)) => key = value,
                (2, Field::Varint(value)) => unlocked = value != 0,
                (3, Field::Fixed32(value)) => time = value,
                _ => {}
            }
        }
        if unlocked {
            times.insert(key, time);
        }
    }
    times
}

/// Pose l'état du compte sur le schéma, par identifiant interne.
fn merge_unlocks(
    entries: Vec<(u64, SteamAchievement)>,
    times: &HashMap<u64, u32>,
) -> Vec<SteamAchievement> {
    entries
        .into_iter()
        .map(|(key, mut achievement)| {
            // Une date nulle laisse le succès verrouillé : Steam l'écrit ainsi
            // pour un déblocage hors ligne qu'il n'a pas encore daté.
            if let Some(time) = times.get(&key).filter(|time| **time > 0) {
                achievement.achieved = true;
                achievement.unlock_time = Some(i64::from(*time));
            }
            achievement
        })
        .collect()
}

fn text(value: &[u8]) -> String {
    String::from_utf8_lossy(value).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encodage d'un champ imbriqué, pour fabriquer des réponses d'essai.
    fn message(field: u32, body: &[u8], out: &mut Vec<u8>) {
        field_bytes(field, body, out);
    }

    fn schema_entry(
        key: u64,
        api_name: &str,
        name: &str,
        hidden: bool,
        percent: Option<&str>,
    ) -> Vec<u8> {
        let mut entry = Vec::new();
        field_bytes(1, api_name.as_bytes(), &mut entry);
        field_bytes(2, name.as_bytes(), &mut entry);
        field_bytes(3, "Description".as_bytes(), &mut entry);
        field_bytes(4, "icone.jpg".as_bytes(), &mut entry);
        field_varint(6, u64::from(hidden), &mut entry);
        if let Some(percent) = percent {
            field_bytes(7, percent.as_bytes(), &mut entry);
        }
        field_varint(8, key, &mut entry);
        entry
    }

    #[test]
    fn the_schema_is_sorted_and_carries_rarity_and_icon_urls() {
        let mut body = Vec::new();
        let second = schema_entry(513, "SECOND", "Deuxième", true, Some("12.5"));
        let first = schema_entry(512, "PREMIER", "Premier", false, Some("74.0"));
        message(1, &second, &mut body);
        message(1, &first, &mut body);

        let entries = parse_schema(&body, 4000);
        assert_eq!(entries.len(), 2);
        // Trié par identifiant interne, pas par ordre d'arrivée.
        assert_eq!(entries[0].0, 512);
        assert_eq!(entries[0].1.api_name, "PREMIER");
        assert_eq!(entries[1].1.api_name, "SECOND");
        assert!(entries[1].1.hidden);
        assert_eq!(entries[0].1.percent, Some(74.0));
        assert_eq!(entries[1].1.percent, Some(12.5));
        assert_eq!(
            entries[0].1.icon,
            "https://cdn.cloudflare.steamstatic.com/steamcommunity/public/images/apps/4000/icone.jpg"
        );
        assert!(!entries[0].1.achieved);
    }

    #[test]
    fn a_success_without_published_rarity_stays_unrated() {
        // Le champ 7 absent est le cas des vieux jeux : aucun pourcentage.
        let mut body = Vec::new();
        message(1, &schema_entry(1, "SECRET", "Secret", true, None), &mut body);
        let entries = parse_schema(&body, 4000);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].1.percent, None);
    }

    /// Réponse d'essai de `Player.GetUserAchievements#1`.
    fn user_body(entries: &[(u64, bool, u32)]) -> Vec<u8> {
        let mut body = Vec::new();
        for (key, unlocked, time) in entries {
            let mut entry = Vec::new();
            field_varint(1, *key, &mut entry);
            field_varint(2, u64::from(*unlocked), &mut entry);
            crate::cm::proto::varint(u64::from(3u32 << 3 | 5), &mut entry);
            entry.extend_from_slice(&time.to_le_bytes());
            message(1, &entry, &mut body);
        }
        body
    }

    #[test]
    fn unlocks_are_matched_by_internal_key_not_by_position() {
        let mut body = Vec::new();
        // Le schéma arrive dans cet ordre ; les dates dans un autre.
        let first = schema_entry(7, "A", "A", false, Some("50.0"));
        let second = schema_entry(9, "B", "B", false, Some("10.0"));
        message(1, &first, &mut body);
        message(1, &second, &mut body);
        let entries = parse_schema(&body, 4000);

        let times = parse_unlocks(&user_body(&[(9, true, 1_700_000_000), (7, false, 0)]));
        let items = merge_unlocks(entries, &times);
        assert!(!items[0].achieved, "A est resté verrouillé");
        assert!(items[1].achieved);
        assert_eq!(items[1].unlock_time, Some(1_700_000_000));
    }

    #[test]
    fn an_unlock_without_a_date_is_not_marked_achieved() {
        let mut body = Vec::new();
        message(1, &schema_entry(1, "A", "A", false, Some("50.0")), &mut body);
        let entries = parse_schema(&body, 4000);
        // Succès obtenu hors ligne, pas encore daté par Steam.
        let times = parse_unlocks(&user_body(&[(1, true, 0)]));
        let items = merge_unlocks(entries, &times);
        assert!(!items[0].achieved);
        assert_eq!(items[0].unlock_time, None);
    }

    #[test]
    fn a_locked_success_is_absent_from_the_account_answer() {
        // Steam n'envoie que les succès obtenus : la réponse vide ne débloque
        // rien.
        assert!(parse_unlocks(&[]).is_empty());
        let mut body = Vec::new();
        message(1, &schema_entry(4, "A", "A", false, Some("50.0")), &mut body);
        let items = merge_unlocks(parse_schema(&body, 4000), &parse_unlocks(&[]));
        assert!(!items[0].achieved);
    }

    #[test]
    fn a_game_without_a_session_says_so() {
        let read = game_achievements(4000, &Sources::default());
        assert!(read.items.is_empty());
        assert!(read
            .notes
            .iter()
            .any(|note| note.contains("non connecté")));
        assert_eq!(read.unlocked(), 0);
    }
}
