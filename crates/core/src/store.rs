//! Persistance locale (redb, pur Rust).

use crate::error::{Error, Result};
use crate::model::Game;
use redb::{Database, ReadableTable, TableDefinition};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::path::Path;

const GAMES: TableDefinition<u32, &[u8]> = TableDefinition::new("games");
const KV: TableDefinition<&str, &[u8]> = TableDefinition::new("kv");

pub struct Store {
    db: Database,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let db = Database::create(path)?;
        Ok(Self { db })
    }

    pub fn save_game(&self, game: &Game) -> Result<()> {
        let bytes = serde_json::to_vec(game)?;
        let txn = self.db.begin_write()?;
        {
            let mut table = txn.open_table(GAMES)?;
            table.insert(game.appid, bytes.as_slice())?;
        }
        txn.commit()?;
        Ok(())
    }

    pub fn save_games(&self, games: &[Game]) -> Result<()> {
        let txn = self.db.begin_write()?;
        {
            let mut table = txn.open_table(GAMES)?;
            for game in games {
                let bytes = serde_json::to_vec(game)?;
                table.insert(game.appid, bytes.as_slice())?;
            }
        }
        txn.commit()?;
        Ok(())
    }

    pub fn load_games(&self) -> Result<Vec<Game>> {
        let txn = self.db.begin_read()?;
        let table = match txn.open_table(GAMES) {
            Ok(t) => t,
            Err(_) => return Ok(Vec::new()),
        };
        let mut out = Vec::new();
        for entry in table.iter()? {
            let (_, value) = entry?;
            match serde_json::from_slice::<Game>(value.value()) {
                Ok(game) => out.push(game),
                Err(err) => tracing::warn!("entrée de jeu illisible : {err}"),
            }
        }
        out.sort_by_key(|g| g.name.to_lowercase());
        Ok(out)
    }

    pub fn game(&self, appid: u32) -> Result<Option<Game>> {
        let txn = self.db.begin_read()?;
        let table = match txn.open_table(GAMES) {
            Ok(t) => t,
            Err(_) => return Ok(None),
        };
        match table.get(appid)? {
            Some(value) => Ok(serde_json::from_slice(value.value()).ok()),
            None => Ok(None),
        }
    }

    pub fn remove_game(&self, appid: u32) -> Result<()> {
        let txn = self.db.begin_write()?;
        {
            let mut table = txn.open_table(GAMES)?;
            table.remove(appid)?;
        }
        txn.commit()?;
        Ok(())
    }

    pub fn get_json<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let txn = self.db.begin_read()?;
        let table = match txn.open_table(KV) {
            Ok(t) => t,
            Err(_) => return Ok(None),
        };
        match table.get(key)? {
            Some(value) => Ok(serde_json::from_slice(value.value()).ok()),
            None => Ok(None),
        }
    }

    pub fn set_json<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        let bytes = serde_json::to_vec(value)?;
        let txn = self.db.begin_write()?;
        {
            let mut table = txn.open_table(KV)?;
            table.insert(key, bytes.as_slice())?;
        }
        txn.commit()?;
        Ok(())
    }

    pub fn get_string(&self, key: &str) -> Result<Option<String>> {
        self.get_json::<String>(key)
    }

    pub fn set_string(&self, key: &str, value: &str) -> Result<()> {
        self.set_json(key, &value.to_string())
    }

    /// Enregistre un échec et le remonte.
    pub fn record_error(context: &str, err: impl std::fmt::Display) -> Error {
        tracing::error!("{context} : {err}");
        Error::Other(format!("{context} : {err}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Drm, Game};

    fn sample(appid: u32) -> Game {
        Game {
            appid,
            name: format!("Jeu {appid}"),
            drm: Drm::None,
            installed: true,
            ..Default::default()
        }
    }

    #[test]
    fn store_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        {
            let store = Store::open(&dir.path().join("db.redb")).unwrap();
            store.save_games(&[sample(10), sample(20)]).unwrap();
            assert_eq!(store.load_games().unwrap().len(), 2);
            assert_eq!(store.game(10).unwrap().unwrap().name, "Jeu 10");
            store.remove_game(10).unwrap();
            assert_eq!(store.load_games().unwrap().len(), 1);
            store.set_json("stats", &42u32).unwrap();
            assert_eq!(store.get_json::<u32>("stats").unwrap(), Some(42));
        }
        let store = Store::open(&dir.path().join("db.redb")).unwrap();
        assert_eq!(store.load_games().unwrap().len(), 1);
    }

    #[test]
    fn missing_table_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("empty.redb")).unwrap();
        assert!(store.load_games().unwrap().is_empty());
        assert!(store.game(1).unwrap().is_none());
        assert!(store.get_json::<String>("nope").unwrap().is_none());
    }
}
