//! Écrans connectés : connecteur, mode courant et fréquence de rafraîchissement.
//!
//! KScreen (Plasma) est interrogé en premier parce qu'il donne directement le
//! mode courant et l'écran principal, y compris en Wayland. `xrandr` sert de
//! repli sur les sessions X11 et les bureaux sans KScreen.

use serde::{Deserialize, Serialize};
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayInfo {
    /// Nom du connecteur (`DP-2`, `HDMI-A-1`, `eDP-1`…), celui qu'attend
    /// gamescope pour `--prefer-output`.
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub refresh_hz: u32,
    pub primary: bool,
}

impl DisplayInfo {
    /// Libellé lisible pour les réglages.
    pub fn label(&self) -> String {
        let mut label = format!(
            "{} · {}×{} · {} Hz",
            self.name, self.width, self.height, self.refresh_hz
        );
        if self.primary {
            label.push_str(" · principal");
        }
        label
    }
}

/// Écrans reliés et actifs, l'écran principal en tête.
pub fn displays() -> Vec<DisplayInfo> {
    let mut list = kscreen().or_else(xrandr).unwrap_or_default();
    // Le principal d'abord : c'est la valeur proposée par défaut aux réglages.
    list.sort_by_key(|display| !display.primary);
    list
}

/// Écran principal, ou à défaut le premier écran actif.
pub fn primary() -> Option<DisplayInfo> {
    let list = displays();
    list.iter()
        .find(|display| display.primary)
        .cloned()
        .or_else(|| list.first().cloned())
}

fn kscreen() -> Option<Vec<DisplayInfo>> {
    let output = Command::new("kscreen-doctor").arg("-j").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let list = parse_kscreen(&String::from_utf8_lossy(&output.stdout))?;
    (!list.is_empty()).then_some(list)
}

fn xrandr() -> Option<Vec<DisplayInfo>> {
    let output = Command::new("xrandr").arg("--query").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let list = parse_xrandr(&String::from_utf8_lossy(&output.stdout));
    (!list.is_empty()).then_some(list)
}

fn id_of(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(id) => Some(id.clone()),
        serde_json::Value::Number(id) => Some(id.to_string()),
        _ => None,
    }
}

/// Analyse la sortie de `kscreen-doctor -j`. Séparée pour être testable.
fn parse_kscreen(raw: &str) -> Option<Vec<DisplayInfo>> {
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    let outputs = value.get("outputs")?.as_array()?;
    let mut list = Vec::new();
    for output in outputs {
        let connected = output
            .get("connected")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let enabled = output
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        if !connected || !enabled {
            continue;
        }
        let Some(name) = output.get("name").and_then(|v| v.as_str()) else {
            continue;
        };
        let modes = output
            .get("modes")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let current = output.get("currentModeId").and_then(id_of);
        let preferred = output
            .get("preferredModes")
            .and_then(|v| v.as_array())
            .and_then(|ids| ids.first())
            .and_then(id_of);
        let mode = current
            .and_then(|id| modes.iter().find(|mode| mode.get("id").and_then(id_of).as_deref() == Some(id.as_str())))
            .or_else(|| {
                preferred.and_then(|id| {
                    modes
                        .iter()
                        .find(|mode| mode.get("id").and_then(id_of).as_deref() == Some(id.as_str()))
                })
            })
            .or_else(|| modes.first());
        let Some(mode) = mode else {
            continue;
        };
        let width = mode
            .get("size")
            .and_then(|size| size.get("width"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;
        let height = mode
            .get("size")
            .and_then(|size| size.get("height"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;
        if width == 0 || height == 0 {
            continue;
        }
        let refresh_hz = mode
            .get("refreshRate")
            .and_then(|v| v.as_f64())
            .map(|rate| rate.round() as u32)
            .unwrap_or(0);
        // Dans KScreen, la priorité 1 désigne l'écran principal.
        let primary = output.get("priority").and_then(|v| v.as_u64()) == Some(1);
        list.push(DisplayInfo {
            name: name.to_string(),
            width,
            height,
            refresh_hz,
            primary,
        });
    }
    Some(list)
}

fn parse_resolution(text: &str) -> Option<(u32, u32)> {
    let (width, rest) = text.split_once('x')?;
    let height = rest
        .split(|c: char| !c.is_ascii_digit())
        .next()
        .filter(|digits| !digits.is_empty())?;
    Some((width.parse().ok()?, height.parse().ok()?))
}

/// Analyse `xrandr --query`. Une ligne d'en-tête décrit un écran connecté, ses
/// lignes indentées listent les modes ; l'astérisque marque le mode courant.
fn parse_xrandr(raw: &str) -> Vec<DisplayInfo> {
    struct Pending {
        name: String,
        primary: bool,
        width: u32,
        height: u32,
        refresh_hz: u32,
    }

    fn push(list: &mut Vec<DisplayInfo>, pending: Pending) {
        if pending.width == 0 || pending.height == 0 {
            return;
        }
        list.push(DisplayInfo {
            name: pending.name,
            width: pending.width,
            height: pending.height,
            refresh_hz: pending.refresh_hz,
            primary: pending.primary,
        });
    }

    let mut list = Vec::new();
    let mut pending: Option<Pending> = None;
    for line in raw.lines() {
        if line.starts_with(' ') || line.starts_with('\t') {
            let Some(entry) = pending.as_mut() else {
                continue;
            };
            if entry.refresh_hz != 0 {
                continue;
            }
            let tokens: Vec<&str> = line.split_whitespace().collect();
            let Some(mode) = tokens.first() else {
                continue;
            };
            if parse_resolution(mode) != Some((entry.width, entry.height)) {
                continue;
            }
            let starred = tokens.iter().find(|token| token.contains('*'));
            let rate = starred.or_else(|| tokens.get(1));
            entry.refresh_hz = rate
                .map(|token| {
                    token
                        .trim_end_matches(|c: char| !c.is_ascii_digit() && c != '.')
                        .replace(|c: char| !c.is_ascii_digit() && c != '.', "")
                })
                .and_then(|text| text.parse::<f64>().ok())
                .map(|rate| rate.round() as u32)
                .unwrap_or(0);
            continue;
        }
        if let Some(entry) = pending.take() {
            push(&mut list, entry);
        }
        if !line.contains(" connected") {
            continue;
        }
        let Some(name) = line.split_whitespace().next() else {
            continue;
        };
        let geometry = line
            .split_whitespace()
            .find(|token| token.contains('x') && token.contains('+'));
        let Some((width, height)) = geometry.and_then(parse_resolution) else {
            continue;
        };
        pending = Some(Pending {
            name: name.to_string(),
            primary: line.contains(" primary"),
            width,
            height,
            refresh_hz: 0,
        });
    }
    if let Some(entry) = pending {
        push(&mut list, entry);
    }
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kscreen_reads_current_mode_and_primary() {
        let raw = r#"{
            "outputs": [
                {
                    "name": "DP-2", "connected": true, "enabled": true,
                    "priority": 1, "currentModeId": "2",
                    "modes": [
                        {"id": "1", "refreshRate": 60.0, "size": {"width": 1920, "height": 1080}},
                        {"id": "2", "refreshRate": 164.9, "size": {"width": 2560, "height": 1440}}
                    ]
                },
                {
                    "name": "HDMI-A-1", "connected": true, "enabled": true,
                    "priority": 2, "currentModeId": "7",
                    "modes": [{"id": "7", "refreshRate": 60.0, "size": {"width": 1920, "height": 1080}}]
                },
                {
                    "name": "DP-3", "connected": false, "enabled": false,
                    "priority": 3, "modes": []
                }
            ]
        }"#;
        let list = parse_kscreen(raw).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "DP-2");
        assert_eq!((list[0].width, list[0].height), (2560, 1440));
        assert_eq!(list[0].refresh_hz, 165);
        assert!(list[0].primary);
        assert!(!list[1].primary);
        assert_eq!(list[1].refresh_hz, 60);
    }

    #[test]
    fn xrandr_reads_starred_mode() {
        let raw = concat!(
            "Screen 0: minimum 8 x 8, current 3840 x 1080, maximum 32767 x 32767\n",
            "DP-2 connected primary 1920x1080+0+0 (normal left inverted right x axis y axis)\n",
            "   1920x1080     60.00 + 164.90* \n",
            "   1280x720      60.00\n",
            "HDMI-A-1 connected 1920x1080+1920+0 (normal left inverted right x axis y axis)\n",
            "   1920x1080     60.00*+\n",
            "   1024x768      60.00\n",
        );
        let list = parse_xrandr(raw);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "DP-2");
        assert!(list[0].primary);
        assert_eq!((list[0].width, list[0].height), (1920, 1080));
        assert_eq!(list[0].refresh_hz, 165);
        assert_eq!(list[1].name, "HDMI-A-1");
        assert!(!list[1].primary);
        assert_eq!(list[1].refresh_hz, 60);
    }

    #[test]
    fn xrandr_ignores_disconnected_outputs() {
        let raw = concat!(
            "eDP-1 connected 1920x1080+0+0 (normal)\n",
            "   1920x1080     60.00*+\n",
            "DP-4 disconnected (normal left inverted right x axis y axis)\n",
        );
        let list = parse_xrandr(raw);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "eDP-1");
        assert_eq!(list[0].refresh_hz, 60);
    }

    #[test]
    fn label_mentions_primary() {
        let display = DisplayInfo {
            name: "DP-2".into(),
            width: 1920,
            height: 1080,
            refresh_hz: 165,
            primary: true,
        };
        assert_eq!(display.label(), "DP-2 · 1920×1080 · 165 Hz · principal");
    }
}
