//! Application Discord officielle d'un jeu, pour le Rich Presence.
//!
//! Discord n'autorise pas une présence à changer le nom de l'application :
//! le titre affiché (« Joue à … ») vient toujours de l'identifiant utilisé à
//! la poignée de main, et les champs `details` / `state` ne font que le texte
//! en dessous. Pour que le titre principal soit le nom du jeu, il faut donc
//! se présenter sous l'application Discord de ce jeu. Discord publie
//! justement son catalogue de jeux détectables, avec l'identifiant
//! d'application, son nom, les identifiants Steam/Epic/GOG et les
//! exécutables : c'est la même source que son client utilise pour reconnaître
//! un jeu lancé.
//!
//! La correspondance par identifiant de boutique est fiable : elle vient de
//! l'éditeur du jeu. Celle par nom d'exécutable est un filet pour les jeux
//! absents du catalogue ; elle est vérifiée par le nom de l'application, sans
//! quoi un binaire partagé pourrait afficher le titre et l'icône d'un autre
//! jeu (le cas typique d'un moteur ou d'un outil).
//!
//! Le catalogue pèse une dizaine de mégaoctets ; il est téléchargé en tâche de
//! fond, réduit à un index de correspondances et gardé une semaine.

use lapinux_core::model::Game;
use lapinux_core::paths::LapinuxPaths;
use lapinux_core::stores::Store;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

/// Catalogue public des applications détectables par Discord.
const SOURCE: &str = "https://discord.com/api/v9/applications/detectable";
/// Une semaine : le catalogue bouge lentement, et pèse lourd à télécharger.
const MAX_AGE: i64 = 7 * 24 * 60 * 60;

/// Application Discord reconnue pour un jeu : son identifiant, qui décide du
/// titre et de l'icône, et son nom, qui sert à écarter une fausse
/// correspondance par exécutable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct App {
    pub id: String,
    #[serde(default)]
    pub name: String,
}

/// Correspondances prêtes à l'emploi : `steam:2830030` → application Discord.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Index {
    #[serde(default)]
    pub apps: HashMap<String, App>,
    /// Date du dernier téléchargement réussi, en secondes Unix.
    #[serde(default)]
    pub fetched: i64,
}

impl Index {
    pub fn path(paths: &LapinuxPaths) -> PathBuf {
        paths.cache_dir.join("discord-games.json")
    }

    /// Relit l'index du disque ; vide s'il est absent ou illisible.
    pub fn load(paths: &LapinuxPaths) -> Self {
        std::fs::read(Self::path(paths))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, paths: &LapinuxPaths) -> Result<(), String> {
        let path = Self::path(paths);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("cache Discord : {e}"))?;
        }
        let body = serde_json::to_vec(self).map_err(|e| format!("index Discord : {e}"))?;
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, body).map_err(|e| format!("index Discord : {e}"))?;
        std::fs::rename(&temporary, &path).map_err(|e| format!("index Discord : {e}"))
    }

    pub fn stale(&self) -> bool {
        self.fetched <= 0 || lapinux_core::util::now_unix() - self.fetched > MAX_AGE
    }

    /// Application Discord du jeu, si elle est reconnue.
    ///
    /// La boutique et son identifiant passent avant l'exécutable : deux jeux
    /// peuvent partager un nom de binaire (moteurs, démos), pas un identifiant
    /// de boutique. Une correspondance par exécutable n'est retenue que si le
    /// nom de l'application ressemble à celui du jeu.
    pub fn app_for(&self, game: &Game) -> Option<&App> {
        let by_store = match game.store {
            Store::Steam => Some(format!("steam:{}", game.appid)),
            Store::Epic => store_key("epic", &game.store_id),
            Store::Gog => store_key("gog", &game.store_id),
            Store::Itch => store_key("itch", &game.store_id),
        };
        if let Some(app) = by_store.and_then(|key| self.apps.get(&key)) {
            return Some(app);
        }
        let by_exe = game
            .exe
            .as_ref()
            .and_then(|exe| exe.file_name())
            .and_then(|name| name.to_str())
            .map(|name| format!("exe:{}", name.to_lowercase()));
        by_exe
            .and_then(|key| self.apps.get(&key))
            .filter(|app| same_game(&app.name, &game.name))
    }
}

fn store_key(distributor: &str, id: &str) -> Option<String> {
    let id = id.trim();
    (!id.is_empty()).then(|| format!("{distributor}:{id}"))
}

/// Nom comparable : minuscules, sans ponctuation ni espaces. « MOTORSLICE »,
/// « Motorslice™ » et « motorslice » se rejoignent.
fn normalized(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Vrai quand le nom de l'application Discord désigne bien ce jeu. Un
/// catalogue imparfait peut associer un exécutable partagé à un autre jeu ;
/// faute de nom commun, mieux vaut ne rien afficher que le titre d'un autre.
fn same_game(app: &str, game: &str) -> bool {
    let app = normalized(app);
    let game = normalized(game);
    if app.is_empty() || game.is_empty() {
        return true;
    }
    app == game
        || app.contains(&game)
        || game.contains(&app)
        || (app.len() >= 4 && game.len() >= 4 && app[..4] == game[..4])
}

// Le catalogue contient des valeurs nulles au milieu d'entrées valides :
// chaque champ utile est donc facultatif plutôt que de faire échouer toute la
// lecture sur une seule ligne incomplète.
#[derive(Debug, Deserialize)]
struct RawApp {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    third_party_skus: Vec<RawSku>,
    #[serde(default)]
    executables: Vec<RawExecutable>,
}

#[derive(Debug, Deserialize)]
struct RawSku {
    #[serde(default)]
    distributor: Option<String>,
    #[serde(default)]
    id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawExecutable {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    is_launcher: Option<bool>,
}

/// Réduit le catalogue à ses seules correspondances utiles.
fn build(raw: &[RawApp]) -> HashMap<String, App> {
    let mut apps = HashMap::new();
    for app in raw {
        let Some(id) = app.id.as_deref().map(str::trim).filter(|id| !id.is_empty()) else {
            continue;
        };
        let entry = App {
            id: id.to_string(),
            name: app.name.as_deref().unwrap_or_default().trim().to_string(),
        };
        for sku in &app.third_party_skus {
            let distributor = match sku.distributor.as_deref() {
                Some("steam") => "steam",
                Some("epic") => "epic",
                Some("gog") => "gog",
                Some("itch") | Some("itch.io") => "itch",
                _ => continue,
            };
            let Some(key) = store_key(distributor, sku.id.as_deref().unwrap_or_default()) else {
                continue;
            };
            apps.entry(key).or_insert_with(|| entry.clone());
        }
        for executable in &app.executables {
            if executable.is_launcher.unwrap_or(false) {
                continue;
            }
            let Some(name) = executable
                .name
                .as_deref()
                .and_then(|name| name.rsplit(['/', '\\']).next())
            else {
                continue;
            };
            let name = name.trim().to_lowercase();
            if name.is_empty() {
                continue;
            }
            apps.entry(format!("exe:{name}"))
                .or_insert_with(|| entry.clone());
        }
    }
    apps
}

/// Télécharge le catalogue et remplace l'index sur disque.
pub fn refresh(paths: &LapinuxPaths) -> Result<Index, String> {
    let response = ureq::get(SOURCE)
        .set("User-Agent", "Lapinux")
        .timeout(Duration::from_secs(90))
        .call()
        .map_err(|e| format!("catalogue Discord : {e}"))?;
    // `into_string` refuse les réponses de plus de dix mégaoctets : le
    // catalogue les dépasse. La lecture directe n'a pas cette limite.
    let mut body = String::new();
    std::io::Read::read_to_string(
        &mut response.into_reader().take(64 * 1024 * 1024),
        &mut body,
    )
    .map_err(|e| format!("catalogue Discord : {e}"))?;
    let raw: Vec<RawApp> =
        serde_json::from_str(&body).map_err(|e| format!("catalogue Discord : {e}"))?;
    let index = Index {
        apps: build(&raw),
        fetched: lapinux_core::util::now_unix(),
    };
    index.save(paths)?;
    Ok(index)
}

impl super::Shared {
    /// Relit ou rafraîchit le catalogue Discord en tâche de fond : le daemon
    /// démarre sans attendre le réseau.
    pub fn start_discord_apps_in_background(self: &std::sync::Arc<Self>) {
        let shared = std::sync::Arc::clone(self);
        std::thread::spawn(move || {
            let stale = shared
                .discord_apps
                .lock()
                .map(|index| index.stale())
                .unwrap_or(true);
            if !stale {
                return;
            }
            match refresh(&shared.paths) {
                Ok(index) => {
                    let count = index.apps.len();
                    if let Ok(mut current) = shared.discord_apps.lock() {
                        *current = index;
                    }
                    tracing::info!("catalogue Discord : {count} correspondances de jeux");
                }
                Err(err) => tracing::debug!("catalogue Discord indisponible : {err}"),
            }
        });
    }

    /// Identifiant Discord officiel du jeu, vide quand il n'est pas reconnu.
    pub fn discord_app_for(&self, game: &Game) -> String {
        self.discord_apps
            .lock()
            .ok()
            .and_then(|index| index.app_for(game).map(|app| app.id.clone()))
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(name: &str, id: &str, skus: &[(&str, &str)], exes: &[&str]) -> RawApp {
        RawApp {
            id: Some(id.into()),
            name: Some(name.into()),
            third_party_skus: skus
                .iter()
                .map(|(distributor, id)| RawSku {
                    distributor: Some((*distributor).into()),
                    id: Some((*id).into()),
                })
                .collect(),
            executables: exes
                .iter()
                .map(|name| RawExecutable {
                    name: Some((*name).into()),
                    is_launcher: Some(false),
                })
                .collect(),
        }
    }

    #[test]
    fn a_steam_appid_finds_the_official_discord_app() {
        let apps = build(&[raw(
            "MOTORSLICE",
            "1442721146587054180",
            &[("steam", "2830030"), ("xbox", "9P0073G9FW5Z")],
            &[],
        )]);
        let game = Game {
            appid: 2830030,
            store: Store::Steam,
            ..Default::default()
        };
        assert_eq!(
            Index {
                apps,
                fetched: 1
            }
            .app_for(&game)
            .map(|app| app.id.as_str()),
            Some("1442721146587054180")
        );
    }

    #[test]
    fn the_executable_is_the_fallback_when_the_store_id_is_unknown() {
        let apps = build(&[raw(
            "MyGame",
            "42",
            &[("gog", "")],
            &["binaries/win64/MyGame-Win64-Shipping.exe"],
        )]);
        let game = Game {
            appid: 7,
            store: Store::Gog,
            store_id: "".into(),
            exe: Some("/jeux/MyGame/binaries/win64/MyGame-Win64-Shipping.exe".into()),
            ..Default::default()
        };
        assert_eq!(
            Index {
                apps,
                fetched: 1
            }
            .app_for(&game)
            .map(|app| app.id.as_str()),
            Some("42")
        );
    }

    #[test]
    fn launchers_and_empty_identifiers_are_not_mapped() {
        let mut app = raw("Launcher", "9", &[("steam", "")], &[]);
        app.executables.push(RawExecutable {
            name: Some("launcher.exe".into()),
            is_launcher: Some(true),
        });
        let apps = build(&[app]);
        assert!(apps.is_empty());
    }

    #[test]
    fn an_executable_collision_is_rejected() {
        // Un binaire partagé ne doit pas faire apparaître le titre et l'icône
        // d'un autre jeu dans la présence.
        let apps = build(&[raw(
            "osu!",
            "367827983903490050",
            &[],
            &["shared-engine.exe"],
        )]);
        let game = Game {
            appid: 7,
            store: Store::Gog,
            store_id: "".into(),
            name: "Motorslice".into(),
            exe: Some("/jeux/Motorslice/shared-engine.exe".into()),
            ..Default::default()
        };
        assert!(Index {
            apps,
            fetched: 1
        }
        .app_for(&game)
        .is_none());
    }

    #[test]
    fn a_store_identifier_always_wins_over_the_name() {
        let apps = build(&[raw("MOTORSLICE", "1442", &[("steam", "2830030")], &[])]);
        let game = Game {
            appid: 2830030,
            store: Store::Steam,
            name: "Un autre nom".into(),
            ..Default::default()
        };
        assert_eq!(
            Index {
                apps,
                fetched: 1
            }
            .app_for(&game)
            .map(|app| app.id.as_str()),
            Some("1442")
        );
    }
}
