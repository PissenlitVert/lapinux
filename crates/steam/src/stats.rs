//! Statistiques publiées d'un jeu, lues par le protocole Steam.
//!
//! `Player.GetUserStats#1` rend, pour un jeu et un compte : le schéma binaire
//! (une entrée par statistique, avec son nom interne et son type) et les
//! valeurs du compte. Les deux tiennent dans une seule réponse, sans clé Web
//! API — le schéma KV de Steam est le même que celui de PICS, à quelques
//! balises près, d'où le lecteur tolérant ci-dessous.

use crate::cm::proto::{field_varint, Field, Reader};
use crate::Error;
use lapinux_core::model::SteamStat;
use std::collections::HashMap;
use std::time::Duration;
use steamroom::client::{LoggedIn, SteamClient};

/// Statistiques du compte pour un jeu : schéma et valeurs.
const STATS_METHOD: &str = "Player.GetUserStats#1";
/// Un CM qui ne répond pas en un quart de minute est perdu.
const REPLY_TIMEOUT: Duration = Duration::from_secs(20);
/// Statistiques retenues : au-delà, la fiche serait un mur de chiffres.
const STATS_LIMIT: usize = 60;

/// Une statistique déclarée par le jeu.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatDef {
    /// Nom interne (`GAME.stat_player_steps`), le seul libellé que beaucoup de
    /// jeux publient.
    pub name: String,
    /// Nom localisé, quand le jeu en donne un.
    pub display: String,
    /// Valeur flottante : les bits de l'entier sont alors un `f32`.
    pub float: bool,
}

/// Statistiques publiées du compte, triées par identifiant.
pub async fn game_stats(
    client: &SteamClient<LoggedIn>,
    appid: u32,
    steam_id: u64,
) -> Result<Vec<SteamStat>, Error> {
    let mut body = Vec::new();
    field_varint(1, steam_id, &mut body);
    field_varint(2, u64::from(appid), &mut body);
    let answer = tokio::time::timeout(
        REPLY_TIMEOUT,
        client.call_service_method(STATS_METHOD, &body),
    )
    .await
    .map_err(|_| Error::Other("Steam n'a pas répondu".into()))?
    .map_err(|error| Error::Other(error.to_string()))?;
    Ok(assemble(&answer.body))
}

/// Schéma et valeurs d'une réponse `Player.GetUserStats#1`.
fn assemble(body: &[u8]) -> Vec<SteamStat> {
    let mut schema = None;
    let mut values: Vec<(u32, u32)> = Vec::new();
    let mut reader = Reader::new(body);
    while let Some((number, field)) = reader.next() {
        match (number, field) {
            (3, Field::Bytes(bytes)) => schema = Some(stats_schema(bytes)),
            (4, Field::Bytes(entry)) => {
                let mut stat = 0u32;
                let mut value = 0u32;
                let mut reader = Reader::new(entry);
                while let Some((number, field)) = reader.next() {
                    if let Field::Varint(raw) = field {
                        match number {
                            1 => stat = u32::try_from(raw).unwrap_or_default(),
                            2 => value = u32::try_from(raw).unwrap_or_default(),
                            _ => {}
                        }
                    }
                }
                values.push((stat, value));
            }
            _ => {}
        }
    }
    let schema = schema.unwrap_or_default();
    let mut stats: Vec<SteamStat> = Vec::new();
    for (id, value) in values {
        let Some(definition) = schema.get(&id) else {
            continue;
        };
        // Une statistique à zéro n'apprend rien : la fiche ne montre que ce
        // que le compte a réellement fait.
        if value == 0 || definition.name.is_empty() {
            continue;
        }
        let value = if definition.float {
            f32::from_bits(value).to_string()
        } else {
            // Le protocole transporte les statistiques entières en 32 bits non
            // signés, le schéma les déclare `INT` : la valeur signée est celle
            // que le jeu affiche.
            i32::from_ne_bytes(value.to_ne_bytes()).to_string()
        };
        stats.push(SteamStat {
            display: if definition.display.is_empty() {
                definition.name.clone()
            } else {
                definition.display.clone()
            },
            name: definition.name.clone(),
            value,
        });
    }
    stats.truncate(STATS_LIMIT);
    stats
}

/// Section `stats` du schéma KV : identifiant → définition.
///
/// La racine est un nœud dont la clé est l'appid et dont les enfants portent
/// `gamename`, `groups`, `stats` et `version`.
fn stats_schema(bytes: &[u8]) -> HashMap<u32, StatDef> {
    let mut cursor = Cursor {
        data: bytes,
        pos: 0,
    };
    let mut stats = HashMap::new();
    let Some((_, Value::Children(app))) = cursor.node() else {
        return stats;
    };
    let Some(Value::Children(entries)) = app.get("stats") else {
        return stats;
    };
    for (id, node) in entries {
        let Ok(id) = id.parse::<u32>() else {
            continue;
        };
        let Value::Children(fields) = node else {
            continue;
        };
        let mut definition = StatDef::default();
        if let Some(Value::String(name)) = fields.get("name") {
            definition.name = name.clone();
        }
        if let Some(Value::String(kind)) = fields.get("type") {
            definition.float = kind.eq_ignore_ascii_case("FLOAT");
        }
        if let Some(Value::Children(display)) = fields.get("display") {
            if let Some(Value::String(name)) = display.get("name") {
                definition.display = name.clone();
            }
        }
        stats.insert(id, definition);
    }
    stats
}

/// Valeur d'un nœud KV. Seuls les types que le schéma des statistiques emploie
/// sont retenus ; les autres sont sautés proprement.
#[derive(Debug, Clone)]
enum Value {
    Children(HashMap<String, Value>),
    String(String),
}

/// Lecteur du format KV binaire de Steam (`tag | clé\0 | valeur`).
struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn byte(&mut self) -> Option<u8> {
        let byte = *self.data.get(self.pos)?;
        self.pos += 1;
        Some(byte)
    }

    fn cstring(&mut self) -> Option<String> {
        let start = self.pos;
        while *self.data.get(self.pos)? != 0 {
            self.pos += 1;
        }
        let text = String::from_utf8_lossy(&self.data[start..self.pos]).into_owned();
        self.pos += 1;
        Some(text)
    }

    fn skip(&mut self, count: usize) -> Option<()> {
        if self.pos + count > self.data.len() {
            return None;
        }
        self.pos += count;
        Some(())
    }

    /// Une entrée : sa balise, sa clé et sa valeur (ou ses enfants).
    fn node(&mut self) -> Option<(String, Value)> {
        let tag = self.byte()?;
        let key = self.cstring()?;
        let value = match tag {
            0 => Value::Children(self.children()?),
            1 => Value::String(self.cstring()?),
            // Entier 32, flottant 32, pointeur, couleur.
            2 | 3 | 4 | 6 => {
                self.skip(4)?;
                Value::String(String::new())
            }
            5 => Value::String(self.wide()?),
            7 | 10 => {
                self.skip(8)?;
                Value::String(String::new())
            }
            // Une balise inconnue arrête la lecture : mieux vaut un schéma
            // partiel qu'un décalage qui lirait n'importe quoi.
            _ => return None,
        };
        Some((key, value))
    }

    /// Chaîne UTF-16, terminée par deux octets nuls.
    fn wide(&mut self) -> Option<String> {
        let start = self.pos;
        while self.pos + 1 < self.data.len()
            && !(self.data[self.pos] == 0 && self.data[self.pos + 1] == 0)
        {
            self.pos += 2;
        }
        let slice = self.data.get(start..self.pos)?;
        self.pos += 2;
        Some(String::from_utf16_lossy(
            &slice
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect::<Vec<_>>(),
        ))
    }

    /// Enfants d'un nœud, jusqu'à sa balise de fin (8 ou 11).
    fn children(&mut self) -> Option<HashMap<String, Value>> {
        let mut map = HashMap::new();
        loop {
            let tag = *self.data.get(self.pos)?;
            if tag == 8 || tag == 11 {
                self.pos += 1;
                return Some(map);
            }
            let (key, value) = self.node()?;
            map.insert(key, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encode un nœud KV de test.
    fn node(tag: u8, key: &str, value: &[u8], out: &mut Vec<u8>) {
        out.push(tag);
        out.extend_from_slice(key.as_bytes());
        out.push(0);
        out.extend_from_slice(value);
    }

    fn string(key: &str, value: &str, out: &mut Vec<u8>) {
        node(1, key, format!("{value}\0").as_bytes(), out);
    }

    fn children(key: &str, body: &[u8], out: &mut Vec<u8>) {
        node(0, key, body, out);
        out.extend_from_slice(&[8]);
    }

    fn schema() -> Vec<u8> {
        let mut steps = Vec::new();
        string("name", "GAME.stat_player_steps", &mut steps);
        string("type", "INT", &mut steps);

        let mut ratio = Vec::new();
        string("name", "GAME.ratio", &mut ratio);
        string("type", "FLOAT", &mut ratio);
        let mut display = Vec::new();
        string("name", "Ratio de victoires", &mut display);
        children("display", &display, &mut ratio);

        let mut stats = Vec::new();
        children("6200", &steps, &mut stats);
        children("6201", &ratio, &mut stats);

        let mut app = Vec::new();
        children("stats", &stats, &mut app);
        let mut root = Vec::new();
        children("620", &app, &mut root);
        root
    }

    fn response(values: &[(u32, u32)]) -> Vec<u8> {
        let mut body = Vec::new();
        crate::cm::proto::field_bytes(3, &schema(), &mut body);
        for (id, value) in values {
            let mut entry = Vec::new();
            field_varint(1, u64::from(*id), &mut entry);
            field_varint(2, u64::from(*value), &mut entry);
            crate::cm::proto::field_bytes(4, &entry, &mut body);
        }
        body
    }

    #[test]
    fn the_schema_gives_names_display_names_and_types() {
        let schema = stats_schema(&schema());
        assert_eq!(schema.len(), 2);
        assert_eq!(schema[&6200].name, "GAME.stat_player_steps");
        assert!(!schema[&6200].float);
        assert_eq!(schema[&6201].display, "Ratio de victoires");
        assert!(schema[&6201].float);
    }

    #[test]
    fn values_are_matched_to_their_schema_entry() {
        let stats = assemble(&response(&[(6200, 12_345), (6201, 0.5f32.to_bits())]));
        assert_eq!(stats.len(), 2);
        assert_eq!(stats[0].name, "GAME.stat_player_steps");
        assert_eq!(stats[0].value, "12345");
        assert_eq!(stats[0].display, "GAME.stat_player_steps");
        assert_eq!(stats[1].display, "Ratio de victoires");
        assert_eq!(stats[1].value, "0.5");
    }

    #[test]
    fn a_zero_stat_is_left_out() {
        let stats = assemble(&response(&[(6200, 0), (6201, 0)]));
        assert!(stats.is_empty());
    }

    #[test]
    fn an_unknown_stat_id_is_ignored() {
        let stats = assemble(&response(&[(6200, 7), (9999, 3)]));
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].name, "GAME.stat_player_steps");
    }

    #[test]
    fn a_broken_schema_leaves_the_values_out_instead_of_panicking() {
        let mut body = Vec::new();
        crate::cm::proto::field_bytes(3, b"\x00\x01hors", &mut body);
        assert!(assemble(&body).is_empty());
    }
}
