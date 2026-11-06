//! Masque temporairement les panneaux Plasma pendant la partie ou l'overlay.
//!
//! En jeu, les panneaux des autres écrans restent visibles et cassent
//! l'impression de ne voir que le jeu et son overlay. Le daemon garde le
//! masquage pendant toute la session, pour ne pas faire clignoter la barre à
//! chaque ouverture du menu ; l'overlay ne le pose que s'il tourne seul. Le
//! mode de masquage est mémorisé sur disque : si le processus est tué
//! brutalement, le prochain démarrage restaure les panneaux au lieu de
//! laisser l'autohide installé.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use zbus::blocking::{Connection, Proxy};

const SERVICE: &str = "org.kde.plasmashell";
const OBJECT_PATH: &str = "/PlasmaShell";
const INTERFACE: &str = "org.kde.PlasmaShell";
/// Modes acceptés par le scripting Plasma ; tout autre nom est ignoré.
const MODES: [&str; 4] = ["none", "autohide", "dodgewindows", "windowsgobelow"];
const STATE_FILE: &str = "panel-hiding";

#[derive(Debug, Clone, PartialEq, Eq)]
struct SavedPanel {
    id: i32,
    hiding: String,
}

/// Masque les panneaux tant qu'il vit et les remet dans leur état d'origine
/// au `Drop`. `None` quand Plasma n'est pas joignable ou qu'il n'y a aucun
/// panneau : l'overlay continue alors sans toucher au bureau.
#[derive(Debug)]
pub struct PanelGuard {
    state_path: PathBuf,
    panels: Vec<SavedPanel>,
}

impl PanelGuard {
    pub fn hide(data_dir: &Path) -> Option<Self> {
        let state_path = data_dir.join(STATE_FILE);
        // Un autre overlay déjà ouvert possède l'état : ne pas le lui voler.
        if let Some(owner) = owner_pid(&state_path) {
            if is_alive(owner) {
                return Some(Self {
                    state_path,
                    panels: Vec::new(),
                });
            }
        }
        // Un état laissé par un overlay tué trop tôt est réparé d'abord : on
        // ne veut jamais empiler deux autohide et perdre le mode d'origine.
        restore_stale(data_dir);
        let panels = query()?;
        if panels.is_empty() {
            return None;
        }
        if let Err(err) = evaluate(&hide_script()) {
            tracing::warn!("panneaux Plasma : masquage impossible ({err})");
            return None;
        }
        let state = format!("pid={}\n{}", std::process::id(), state_text(&panels));
        if let Err(err) = std::fs::write(&state_path, state) {
            tracing::warn!("panneaux Plasma : état non enregistré ({err})");
        }
        Some(Self { state_path, panels })
    }

    fn restore(&self) {
        if self.panels.is_empty() {
            return;
        }
        match evaluate(&restore_script(&self.panels)) {
            Ok(_) => {
                let _ = std::fs::remove_file(&self.state_path);
            }
            // Le fichier reste : le prochain démarrage retentera la
            // restauration, même si Plasma a redémarré entre-temps.
            Err(err) => tracing::warn!("panneaux Plasma : restauration impossible ({err})"),
        }
    }
}

impl Drop for PanelGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

/// Répare un état laissé par une session précédente. Appelé au démarrage du
/// daemon et avant chaque masquage.
pub fn restore_stale(data_dir: &Path) {
    let path = data_dir.join(STATE_FILE);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    if let Some(owner) = owner_pid(&path) {
        if is_alive(owner) {
            return;
        }
    }
    let panels = parse_state(&text);
    if panels.is_empty() {
        let _ = std::fs::remove_file(&path);
        return;
    }
    match evaluate(&restore_script(&panels)) {
        Ok(_) => {
            let _ = std::fs::remove_file(&path);
        }
        Err(err) => tracing::debug!("panneaux Plasma : restauration différée ({err})"),
    }
}

fn evaluate(script: &str) -> Result<String, String> {
    let connection = Connection::session().map_err(|e| format!("D-Bus : {e}"))?;
    let plasmashell = Proxy::new(&connection, SERVICE, OBJECT_PATH, INTERFACE)
        .map_err(|e| format!("PlasmaShell : {e}"))?;
    plasmashell
        .call("evaluateScript", &(script,))
        .map_err(|e| format!("évaluation Plasma : {e}"))
}

fn query() -> Option<Vec<SavedPanel>> {
    let script = "var o=[]; var p=panels(); for(var i=0;i<p.length;i++){o.push(p[i].id+\":\"+p[i].hiding);} print(o.join(\";\"));";
    let output = evaluate(script).ok()?;
    let mut panels = Vec::new();
    for entry in output.trim().trim_matches('\0').split(';') {
        let Some((id, hiding)) = entry.split_once(':') else {
            continue;
        };
        let Ok(id) = id.trim().parse::<i32>() else {
            continue;
        };
        let hiding = hiding.trim().to_ascii_lowercase();
        let hiding = if MODES.contains(&hiding.as_str()) {
            hiding
        } else {
            "none".to_string()
        };
        panels.push(SavedPanel { id, hiding });
    }
    Some(panels)
}

fn hide_script() -> String {
    "var p=panels(); for(var i=0;i<p.length;i++){p[i].hiding=\"autohide\";}".to_string()
}

fn restore_script(panels: &[SavedPanel]) -> String {
    let mut script = String::new();
    for panel in panels {
        // `panelById` renvoie undefined pour un panneau supprimé depuis : le
        // test évite une exception qui annulerait toute la restauration.
        let _ = writeln!(
            script,
            "var p=panelById({id}); if(p){{p.hiding=\"{mode}\";}}",
            id = panel.id,
            mode = panel.hiding
        );
    }
    script
}

fn state_text(panels: &[SavedPanel]) -> String {
    let mut text = String::new();
    for panel in panels {
        let _ = writeln!(text, "{}={}", panel.id, panel.hiding);
    }
    text
}

fn owner_pid(path: &Path) -> Option<u32> {
    let text = std::fs::read_to_string(path).ok()?;
    for line in text.lines() {
        let Some(pid) = line.trim().strip_prefix("pid=") else {
            continue;
        };
        return pid.parse().ok();
    }
    None
}

fn is_alive(pid: u32) -> bool {
    // `kill(pid, 0)` ne tue rien : il vérifie seulement que le processus
    // existe. EPERM signifie qu'il existe mais nous est inaccessible.
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    let result = unsafe { libc::kill(pid, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

fn parse_state(text: &str) -> Vec<SavedPanel> {
    let mut panels = Vec::new();
    for line in text.lines() {
        let Some((id, hiding)) = line.split_once('=') else {
            continue;
        };
        let Ok(id) = id.trim().parse::<i32>() else {
            continue;
        };
        let hiding = hiding.trim().to_ascii_lowercase();
        if MODES.contains(&hiding.as_str()) {
            panels.push(SavedPanel { id, hiding });
        }
    }
    panels
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_round_trips() {
        let panels = vec![
            SavedPanel {
                id: 4,
                hiding: "none".into(),
            },
            SavedPanel {
                id: 7,
                hiding: "dodgewindows".into(),
            },
        ];
        assert_eq!(parse_state(&state_text(&panels)), panels);
    }

    #[test]
    fn unknown_modes_are_not_replayed() {
        let parsed = parse_state("pid=4242\n4=evil\");\n1=autohide\n");
        assert_eq!(
            parsed,
            vec![SavedPanel {
                id: 1,
                hiding: "autohide".into()
            }]
        );
    }

    #[test]
    fn owner_pid_is_read_from_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(STATE_FILE);
        std::fs::write(&path, "pid=4242\n4=none\n").unwrap();
        assert_eq!(owner_pid(&path), Some(4242));
    }

    #[test]
    fn restore_script_skips_missing_panels() {
        let script = restore_script(&[SavedPanel {
            id: 4,
            hiding: "windowsgobelow".into(),
        }]);
        assert!(script.contains("if(p)"));
        assert!(script.contains("windowsgobelow"));
    }
}
