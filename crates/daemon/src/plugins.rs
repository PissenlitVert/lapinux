//! Hooks backend des plugins actifs.
//!
//! Un plugin reste un manifeste TOML. En plus de ses contributions visuelles,
//! il peut déclarer des commandes exécutées par le daemon aux moments utiles
//! de la partie :
//!
//! ```toml
//! [[backend_hooks]]
//! event = "game_launch"
//! command = "mon-script"
//! ```
//!
//! Sur `game_launch`, la sortie standard peut renvoyer des variables
//! d'environnement `CLE=valeur`, ajoutées au lancement du jeu. Sur
//! `game_stop`, la commande sert à remettre le système dans son état. Les
//! commandes tournent avec les droits de l'utilisateur : un plugin est du code
//! de confiance, exactement comme un script de démarrage personnel.

use lapinux_core::paths::LapinuxPaths;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::process::{Command, Stdio};

const HOOK_TIMEOUT: &str = "8";

#[derive(Debug, Clone, Default, Deserialize)]
struct PluginFile {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    backend_hooks: Vec<BackendHook>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct BackendHook {
    event: String,
    command: String,
}

/// État des plugins locaux, tel que l'interface et les tests peuvent le lire
/// sans reparcourir les fichiers eux-mêmes.
pub fn installed(paths: &LapinuxPaths, active: &[String]) -> Vec<serde_json::Value> {
    let Ok(entries) = std::fs::read_dir(paths.plugins_dir()) else {
        return Vec::new();
    };
    let mut plugins = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("toml") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(manifest) = toml::from_str::<PluginFile>(&text) else {
            continue;
        };
        if !safe_id(&manifest.id) {
            continue;
        }
        plugins.push(serde_json::json!({
            "id": manifest.id,
            "name": manifest.name,
            "path": path.display().to_string(),
            "active": active.iter().any(|id| id == &manifest.id),
            "hooks": manifest.backend_hooks,
        }));
    }
    plugins.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    plugins
}

/// Variables communes données aux hooks, pour qu'un script sache quelle
/// partie commence ou se termine.
pub fn base_context(event: &str) -> Vec<(String, String)> {
    vec![
        ("LAPINUX_EVENT".into(), event.into()),
        ("LAPINUX_DAEMON_PID".into(), std::process::id().to_string()),
    ]
}

/// Exécute les hooks `game_launch` et renvoie les variables d'environnement
/// qu'ils demandent d'ajouter au jeu.
pub fn launch_environment(
    paths: &LapinuxPaths,
    active: &[String],
    appid: u32,
    game_name: &str,
    store: &str,
) -> Vec<(String, Option<String>)> {
    let mut context = base_context("game_launch");
    context.push(("LAPINUX_APPID".into(), appid.to_string()));
    context.push(("LAPINUX_GAME_NAME".into(), game_name.into()));
    context.push(("LAPINUX_STORE".into(), store.into()));
    let mut environment = Vec::new();
    for hook in hooks(paths, active, "game_launch") {
        environment.extend(run(&hook.command, &context, true));
    }
    environment
}

/// Exécute les hooks d'un événement sans récupérer de variables.
pub fn run_event(
    paths: &LapinuxPaths,
    active: &[String],
    event: &str,
    appid: u32,
    game_name: &str,
) {
    let mut context = base_context(event);
    context.push(("LAPINUX_APPID".into(), appid.to_string()));
    context.push(("LAPINUX_GAME_NAME".into(), game_name.into()));
    for hook in hooks(paths, active, event) {
        let _ = run(&hook.command, &context, false);
    }
}

fn hooks(paths: &LapinuxPaths, active: &[String], event: &str) -> Vec<BackendHook> {
    let directory = paths.plugins_dir();
    let mut found = Vec::new();
    for id in active {
        // Un identifiant de plugin reste un nom de fichier simple : jamais de
        // traversée de dossier depuis la configuration.
        if !safe_id(id) {
            continue;
        }
        let path = directory.join(format!("{id}.toml"));
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(manifest) = toml::from_str::<PluginFile>(&text) else {
            tracing::warn!("plugin {id} : hooks illisibles");
            continue;
        };
        found.extend(
            manifest
                .backend_hooks
                .into_iter()
                .filter(|hook| hook.event == event && !hook.command.trim().is_empty()),
        );
    }
    found
}

fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && !id.contains('/')
        && !id.contains('\\')
        && !id.contains("..")
        && id
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_')
}

fn run(
    command: &str,
    context: &[(String, String)],
    capture: bool,
) -> Vec<(String, Option<String>)> {
    if command.trim().is_empty() {
        return Vec::new();
    }
    let mut process = Command::new("timeout");
    process
        .arg(HOOK_TIMEOUT)
        .arg("sh")
        .arg("-c")
        .arg(command)
        .envs(context.iter().map(|(key, value)| (key, value)))
        .stdin(Stdio::null())
        .stdout(if capture {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stderr(Stdio::piped());
    let output = match process.output() {
        Ok(output) => output,
        Err(error) => {
            // `timeout` vient des coreutils ; sans lui, on exécute quand même
            // la commande plutôt que de désactiver silencieusement le hook.
            if error.kind() == std::io::ErrorKind::NotFound {
                return run_without_timeout(command, context, capture);
            }
            tracing::warn!("plugin : hook impossible à lancer ({error})");
            return Vec::new();
        }
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::warn!(
            "plugin : hook {} sorti avec {} {}",
            command,
            output.status,
            stderr.trim()
        );
    }
    if !capture {
        return Vec::new();
    }
    parse_environment(&String::from_utf8_lossy(&output.stdout))
}

fn run_without_timeout(
    command: &str,
    context: &[(String, String)],
    capture: bool,
) -> Vec<(String, Option<String>)> {
    let output = Command::new("sh")
        .arg("-c")
        .arg(command)
        .envs(context.iter().map(|(key, value)| (key, value)))
        .stdin(Stdio::null())
        .stdout(if capture {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .output();
    match output {
        Ok(output) if capture => parse_environment(&String::from_utf8_lossy(&output.stdout)),
        Ok(_) => Vec::new(),
        Err(error) => {
            tracing::warn!("plugin : hook impossible à lancer ({error})");
            Vec::new()
        }
    }
}

fn parse_environment(stdout: &str) -> Vec<(String, Option<String>)> {
    let mut values = BTreeMap::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if !valid_key(key) {
            continue;
        }
        values.insert(
            key.to_string(),
            (!value.is_empty()).then(|| value.to_string()),
        );
    }
    values.into_iter().collect()
}

fn valid_key(key: &str) -> bool {
    let mut chars = key.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn environment_lines_are_strictly_parsed() {
        let parsed = parse_environment("DXVK_HUD=fps\nBAD KEY=1\nEMPTY=\n# comment\nMULTI=a=b\n");
        assert_eq!(
            parsed,
            vec![
                ("DXVK_HUD".into(), Some("fps".into())),
                ("EMPTY".into(), None),
                ("MULTI".into(), Some("a=b".into())),
            ]
        );
    }

    #[test]
    fn identifiers_cannot_escape_the_plugin_directory() {
        let paths = LapinuxPaths::under(Path::new("/tmp/lapinux-plugin-test"));
        assert!(hooks(&paths, &["../evil".into()], "game_launch").is_empty());
        assert!(hooks(&paths, &["Bad".into()], "game_launch").is_empty());
    }

    #[test]
    fn launch_hooks_can_feed_the_game_environment() {
        let root = tempfile::tempdir().unwrap();
        let paths = LapinuxPaths::under(root.path());
        std::fs::create_dir_all(paths.plugins_dir()).unwrap();
        std::fs::write(
            paths.plugins_dir().join("hook.toml"),
            "id = \"hook\"\n[[backend_hooks]]\nevent = \"game_launch\"\ncommand = \"printf 'PLUGIN_TEST=ok\\n'\"\n",
        )
        .unwrap();
        let environment = launch_environment(&paths, &["hook".into()], 42, "Game", "Steam");
        assert_eq!(environment, vec![("PLUGIN_TEST".into(), Some("ok".into()))]);
    }
}
