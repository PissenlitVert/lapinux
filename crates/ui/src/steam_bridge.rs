//! Contrôle du service utilisateur `steam-puck-bridge`.
//!
//! Le pont n'est pas lancé par Lapinux : il vit dans un service systemd
//! utilisateur, car il doit garder le hidraw du Puck pendant toute la session.
//! Cette page se contente de lire son état, de le relancer et d'ouvrir son
//! fichier de service, sans jamais bloquer la peinture egui.

use eframe::egui;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

const UNIT: &str = "steam-puck-bridge.service";
const POLL: Duration = Duration::from_secs(5);

/// Ce que systemd dit du service. Un service absent n'est pas une erreur de
/// commande : `LoadState=not-found` suffit à le distinguer d'un service
/// installé mais arrêté.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServiceState {
    pub loaded: bool,
    pub enabled: bool,
    pub active: bool,
    pub unit_path: Option<PathBuf>,
    pub error: Option<String>,
}

impl ServiceState {
    pub fn installed(&self) -> bool {
        self.loaded && self.unit_path.is_some()
    }

    /// Interroge systemd. Appelé depuis un fil de fond : la commande peut
    /// attendre le bus utilisateur sans figer l'interface.
    pub fn load() -> Self {
        let output = Command::new("systemctl")
            .args([
                "--user",
                "show",
                UNIT,
                "--property=LoadState",
                "--property=UnitFileState",
                "--property=ActiveState",
                "--property=FragmentPath",
            ])
            .stdin(Stdio::null())
            .output();
        match output {
            Ok(output) => {
                let mut state = parse_show(&String::from_utf8_lossy(&output.stdout));
                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    let stderr = stderr.trim();
                    state.error = Some(if stderr.is_empty() {
                        format!("systemctl a échoué ({})", output.status)
                    } else {
                        stderr.to_string()
                    });
                }
                state
            }
            Err(error) => ServiceState {
                error: Some(format!("systemctl : {error}")),
                ..ServiceState::default()
            },
        }
    }

    /// Redémarre le service puis relit son état. `restart` démarre aussi un
    /// service arrêté, ce qui permet au même bouton de servir de « Démarrer ».
    pub fn restart() -> Result<Self, String> {
        let output = Command::new("systemctl")
            .args(["--user", "restart", UNIT])
            .stdin(Stdio::null())
            .output()
            .map_err(|error| format!("systemctl : {error}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stderr = stderr.trim();
            return Err(if stderr.is_empty() {
                format!("systemctl a échoué ({})", output.status)
            } else {
                stderr.to_string()
            });
        }
        Ok(Self::load())
    }
}

fn parse_show(text: &str) -> ServiceState {
    let mut state = ServiceState::default();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "LoadState" => state.loaded = value == "loaded",
            "UnitFileState" => {
                state.enabled = matches!(value, "enabled" | "enabled-runtime");
            }
            "ActiveState" => {
                state.active = matches!(value, "active" | "activating" | "reloading");
            }
            "FragmentPath" if !value.is_empty() => {
                state.unit_path = Some(PathBuf::from(value));
            }
            _ => {}
        }
    }
    state
}

/// État de la carte et opérations en vol. Le champ `note` garde le dernier
/// verdict sous la carte, même quand le toast a disparu.
pub struct SteamBridge {
    pub service: Option<ServiceState>,
    pub details: bool,
    pub busy: bool,
    pub note: Option<(String, bool)>,
    state_rx: Option<Receiver<ServiceState>>,
    action_rx: Option<Receiver<Result<ServiceState, String>>>,
    last_poll: Instant,
}

impl SteamBridge {
    pub fn new() -> Self {
        Self {
            service: None,
            details: false,
            busy: false,
            note: None,
            state_rx: None,
            action_rx: None,
            last_poll: Instant::now(),
        }
    }

    pub fn poll(&mut self, ctx: &egui::Context) {
        let mut action_finished = false;
        let received_action = self.action_rx.as_ref().map(|rx| rx.try_recv());
        match received_action {
            Some(Ok(Ok(state))) => {
                self.service = Some(state);
                self.note = Some((
                    lapinux_core::i18n::t("controllers.steam.bridge_restarted").to_string(),
                    true,
                ));
                action_finished = true;
            }
            Some(Ok(Err(error))) => {
                self.note = Some((
                    lapinux_core::i18n::t_args(
                        "controllers.steam.bridge_restart_failed",
                        &[("error", &error)],
                    ),
                    false,
                ));
                action_finished = true;
            }
            Some(Err(TryRecvError::Disconnected)) => {
                self.note = Some((
                    lapinux_core::i18n::t("controllers.steam.bridge_service_silent").to_string(),
                    false,
                ));
                action_finished = true;
            }
            Some(Err(TryRecvError::Empty)) | None => {}
        }
        if action_finished {
            self.action_rx = None;
            self.busy = false;
            self.last_poll = Instant::now();
        }

        let received_state = self.state_rx.as_ref().map(|rx| rx.try_recv());
        match received_state {
            Some(Ok(state)) => {
                self.service = Some(state);
                self.state_rx = None;
                self.last_poll = Instant::now();
            }
            Some(Err(TryRecvError::Disconnected)) => {
                self.state_rx = None;
                self.last_poll = Instant::now();
            }
            Some(Err(TryRecvError::Empty)) | None => {}
        }

        if self.state_rx.is_none()
            && self.action_rx.is_none()
            && (self.service.is_none() || self.last_poll.elapsed() >= POLL)
        {
            let (tx, rx) = mpsc::channel();
            self.state_rx = Some(rx);
            self.last_poll = Instant::now();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(ServiceState::load());
                ctx.request_repaint();
            });
        }

        if self.busy || self.state_rx.is_some() || self.action_rx.is_some() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }

    pub fn restart(&mut self, ctx: &egui::Context) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.note = None;
        let (tx, rx) = mpsc::channel();
        self.action_rx = Some(rx);
        let thread_ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(ServiceState::restart());
            thread_ctx.request_repaint();
        });
        ctx.request_repaint();
    }

    /// Ouvre le fichier `.service` dans l'application par défaut du bureau.
    /// C'est l'accès direct au service, sans terminal ni commande à retenir.
    pub fn open_unit(&self) -> Result<(), String> {
        let path = self
            .service
            .as_ref()
            .and_then(|service| service.unit_path.as_ref())
            .ok_or_else(|| "fichier du service introuvable".to_string())?;
        Command::new("xdg-open")
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::parse_show;

    #[test]
    fn an_enabled_active_unit_is_installed() {
        let state = parse_show(
            "LoadState=loaded\nUnitFileState=enabled\nActiveState=active\nFragmentPath=/home/test/.config/systemd/user/steam-puck-bridge.service\n",
        );
        assert!(state.installed());
        assert!(state.enabled);
        assert!(state.active);
        assert_eq!(
            state.unit_path.as_deref(),
            Some(std::path::Path::new(
                "/home/test/.config/systemd/user/steam-puck-bridge.service"
            ))
        );
    }

    #[test]
    fn a_missing_unit_is_not_reported_as_installed() {
        let state = parse_show(
            "LoadState=not-found\nUnitFileState=\nActiveState=inactive\nFragmentPath=\n",
        );
        assert!(!state.installed());
        assert!(!state.enabled);
        assert!(!state.active);
        assert!(state.unit_path.is_none());
    }

    #[test]
    fn an_activating_unit_counts_as_running() {
        let state = parse_show(
            "LoadState=loaded\nUnitFileState=disabled\nActiveState=activating\nFragmentPath=/tmp/unit\n",
        );
        assert!(state.active);
        assert!(!state.enabled);
        assert!(state.installed());
    }
}
