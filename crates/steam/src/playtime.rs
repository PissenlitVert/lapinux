//! Temps de jeu et badge d'un jeu, lus par le protocole Steam.
//!
//! `Player.GetOwnedGames#1` rend la bibliothèque du compte en un appel : le
//! temps de jeu total, celui des deux dernières semaines, par système et la
//! date de la dernière partie. `Player.GetGameBadgeLevels#1` rend le niveau du
//! compte et celui du badge du jeu. Les deux passent par la session CM, sans
//! clé Web API.

use crate::cm::proto::{field_varint, Field, Reader};
use crate::Error;
use std::time::Duration;
use steamroom::client::{LoggedIn, SteamClient};

/// Temps de jeu tel que le protocole le publie, en minutes.
const OWNED_METHOD: &str = "Player.GetOwnedGames#1";
/// Niveau du compte et badge d'un jeu.
const BADGE_METHOD: &str = "Player.GetGameBadgeLevels#1";
/// Groupe familial du compte : il ouvre le temps de jeu des jeux prêtés.
const FAMILY_METHOD: &str = "FamilyGroups.GetFamilyGroupForUser#1";
/// Un CM qui ne répond pas en un quart de minute est perdu.
const REPLY_TIMEOUT: Duration = Duration::from_secs(20);

/// Ce qu'un jeu représente dans la bibliothèque du compte.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Playtime {
    pub minutes_total: u64,
    pub minutes_2weeks: u64,
    pub minutes_windows: u64,
    pub minutes_mac: u64,
    pub minutes_linux: u64,
    pub minutes_deck: u64,
    /// Dernière partie, en secondes Unix.
    pub last_played: Option<i64>,
    /// Jeu prêté par un autre membre de la famille Steam.
    pub family_shared: bool,
}

/// Niveau du compte et niveau du badge du jeu.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BadgeLevels {
    pub player_level: Option<u32>,
    pub badge_level: Option<u32>,
}

/// Temps de jeu d'un seul jeu.
///
/// `appids_filter` limite la réponse à ce jeu : l'appel reste léger, même sur
/// un compte à huit cents jeux. `family_group` est le groupe familial du
/// compte : sans lui, Steam omet les jeux prêtés par la famille — leur temps de
/// jeu existe pourtant, il n'est publié qu'avec le groupe (champ 9, vérifié
/// contre le CM).
pub async fn game_playtime(
    client: &SteamClient<LoggedIn>,
    appid: u32,
    steam_id: u64,
    family_group: Option<u64>,
) -> Result<Option<Playtime>, Error> {
    let body = owned_request(steam_id, Some(appid), family_group);
    let answer = tokio::time::timeout(
        REPLY_TIMEOUT,
        client.call_service_method(OWNED_METHOD, &body),
    )
    .await
    .map_err(|_| Error::Other("Steam n'a pas répondu".into()))?
    .map_err(|error| Error::Other(error.to_string()))?;
    Ok(parse_owned(&answer.body)
        .into_iter()
        .find(|(owned, _)| *owned == appid)
        .map(|(_, play)| play))
}

/// Bibliothèque du compte : appid et temps de jeu, dans l'ordre de Steam.
pub async fn owned_games(
    client: &SteamClient<LoggedIn>,
    steam_id: u64,
    family_group: Option<u64>,
) -> Result<Vec<(u32, Playtime)>, Error> {
    let body = owned_request(steam_id, None, family_group);
    let answer = tokio::time::timeout(
        REPLY_TIMEOUT,
        client.call_service_method(OWNED_METHOD, &body),
    )
    .await
    .map_err(|_| Error::Other("Steam n'a pas répondu".into()))?
    .map_err(|error| Error::Other(error.to_string()))?;
    Ok(parse_owned(&answer.body))
}

/// Corps de `Player.GetOwnedGames#1` : `steamid` (1), jeux gratuits joués (3)
/// — sans quoi la réponse omet les jeux auxquels le compte a joué sans les
/// acheter —, filtre d'appid (4) et groupe familial (9).
fn owned_request(steam_id: u64, appid: Option<u32>, family_group: Option<u64>) -> Vec<u8> {
    let mut body = Vec::new();
    field_varint(1, steam_id, &mut body);
    field_varint(3, 1, &mut body);
    if let Some(appid) = appid {
        field_varint(4, u64::from(appid), &mut body);
    }
    if let Some(group) = family_group.filter(|group| *group > 0) {
        field_varint(9, group, &mut body);
    }
    body
}

/// Groupe familial du compte, demandé au CM avec la session déjà ouverte.
///
/// `FamilyGroups.GetFamilyGroupForUser#1` rend l'identifiant du groupe (1) et
/// un drapeau d'appartenance (2, nul quand le compte en est membre). Sans
/// groupe, les jeux prêtés gardent un temps de jeu invisible.
pub async fn family_group_id(client: &SteamClient<LoggedIn>, steam_id: u64) -> Option<u64> {
    let mut body = Vec::new();
    field_varint(1, steam_id, &mut body);
    field_varint(2, 1, &mut body);
    let answer = tokio::time::timeout(
        REPLY_TIMEOUT,
        client.call_service_method(FAMILY_METHOD, &body),
    )
    .await
    .ok()?
    .ok()?;
    let mut group = 0u64;
    let mut reader = Reader::new(&answer.body);
    while let Some((number, field)) = reader.next() {
        if let (1, Field::Varint(value)) = (number, field) {
            group = value;
        }
    }
    (group > 0).then_some(group)
}

/// Niveau du compte et badge du jeu.
pub async fn badge_levels(
    client: &SteamClient<LoggedIn>,
    appid: u32,
) -> Result<BadgeLevels, Error> {
    let mut body = Vec::new();
    field_varint(1, u64::from(appid), &mut body);
    let answer = tokio::time::timeout(
        REPLY_TIMEOUT,
        client.call_service_method(BADGE_METHOD, &body),
    )
    .await
    .map_err(|_| Error::Other("Steam n'a pas répondu".into()))?
    .map_err(|error| Error::Other(error.to_string()))?;
    Ok(parse_badges(&answer.body))
}

/// Entrées `Game` de `Player.GetOwnedGames#1`.
fn parse_owned(body: &[u8]) -> Vec<(u32, Playtime)> {
    let mut games = Vec::new();
    let mut reader = Reader::new(body);
    while let Some((number, field)) = reader.next() {
        let Field::Bytes(entry) = field else {
            continue;
        };
        if number != 2 {
            continue;
        }
        let mut appid = 0u32;
        let mut play = Playtime::default();
        let mut reader = Reader::new(entry);
        while let Some((number, field)) = reader.next() {
            let Field::Varint(value) = field else {
                continue;
            };
            match number {
                1 => appid = u32::try_from(value).unwrap_or_default(),
                3 => play.minutes_2weeks = value,
                4 => play.minutes_total = value,
                8 => play.minutes_windows = value,
                9 => play.minutes_mac = value,
                10 => play.minutes_linux = value,
                11 => play.last_played = (value > 0).then_some(value as i64),
                20 => play.minutes_deck = value,
                21 => play.family_shared = value != 0,
                _ => {}
            }
        }
        if appid != 0 {
            games.push((appid, play));
        }
    }
    games
}

/// Niveau du compte et premier badge de la réponse.
fn parse_badges(body: &[u8]) -> BadgeLevels {
    let mut levels = BadgeLevels::default();
    let mut reader = Reader::new(body);
    while let Some((number, field)) = reader.next() {
        match (number, field) {
            (1, Field::Varint(value)) => {
                levels.player_level = u32::try_from(value).ok().filter(|level| *level > 0)
            }
            (2, Field::Bytes(entry)) => {
                let mut badge = 0u32;
                let mut reader = Reader::new(entry);
                while let Some((number, field)) = reader.next() {
                    if let (1, Field::Varint(value)) = (number, field) {
                        badge = u32::try_from(value).unwrap_or_default();
                    }
                }
                levels.badge_level = levels.badge_level.or(Some(badge).filter(|level| *level > 0));
            }
            _ => {}
        }
    }
    levels
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cm::proto::field_bytes;

    fn game(appid: u64, fields: &[(u32, u64)]) -> Vec<u8> {
        let mut entry = Vec::new();
        field_varint(1, appid, &mut entry);
        for (number, value) in fields {
            field_varint(*number, *value, &mut entry);
        }
        entry
    }

    #[test]
    fn the_owned_request_carries_the_family_group() {
        let body = owned_request(7, Some(620), Some(6_457_711));
        let mut reader = Reader::new(&body);
        let mut fields = Vec::new();
        while let Some((number, field)) = reader.next() {
            fields.push((number, field));
        }
        assert!(fields.contains(&(1, Field::Varint(7))));
        assert!(fields.contains(&(3, Field::Varint(1))));
        assert!(fields.contains(&(4, Field::Varint(620))));
        assert!(fields.contains(&(9, Field::Varint(6_457_711))));
        // Sans groupe connu, le champ 9 ne part pas : Steam rend alors la
        // bibliothèque du seul compte.
        let body = owned_request(7, None, Some(0));
        let mut reader = Reader::new(&body);
        let mut seen = Vec::new();
        while let Some((number, _)) = reader.next() {
            seen.push(number);
        }
        assert_eq!(seen, vec![1, 3]);
    }

    #[test]
    fn owned_games_carry_every_playtime_and_the_last_session() {
        let mut body = Vec::new();
        field_varint(1, 2, &mut body);
        field_bytes(
            2,
            &game(
                620,
                &[
                    (3, 45),
                    (4, 1627),
                    (8, 1005),
                    (9, 12),
                    (10, 610),
                    (11, 1_748_268_660),
                    (20, 7),
                ],
            ),
            &mut body,
        );
        field_bytes(2, &game(4000, &[(4, 3)]), &mut body);

        let games = parse_owned(&body);
        assert_eq!(games.len(), 2);
        assert_eq!(games[0].0, 620);
        let play = &games[0].1;
        assert_eq!(play.minutes_total, 1627);
        assert_eq!(play.minutes_2weeks, 45);
        assert_eq!(play.minutes_windows, 1005);
        assert_eq!(play.minutes_mac, 12);
        assert_eq!(play.minutes_linux, 610);
        assert_eq!(play.minutes_deck, 7);
        assert_eq!(play.last_played, Some(1_748_268_660));
        assert!(!play.family_shared);
        // Un jeu jamais lancé garde une date nulle, pas une date de 1970.
        assert_eq!(games[1].1.last_played, None);
    }

    #[test]
    fn a_family_loan_is_flagged() {
        let mut body = Vec::new();
        field_bytes(2, &game(620, &[(4, 10), (21, 1)]), &mut body);
        let games = parse_owned(&body);
        assert!(games[0].1.family_shared);
    }

    #[test]
    fn the_account_level_and_badge_are_read() {
        let mut badge = Vec::new();
        field_varint(1, 3, &mut badge);
        field_varint(2, 1, &mut badge);
        let mut body = Vec::new();
        field_varint(1, 23, &mut body);
        field_bytes(2, &badge, &mut body);
        let levels = parse_badges(&body);
        assert_eq!(levels.player_level, Some(23));
        assert_eq!(levels.badge_level, Some(3));
    }

    #[test]
    fn a_game_without_a_badge_leaves_it_empty() {
        let mut body = Vec::new();
        field_varint(1, 23, &mut body);
        let levels = parse_badges(&body);
        assert_eq!(levels.player_level, Some(23));
        assert_eq!(levels.badge_level, None);
    }
}
