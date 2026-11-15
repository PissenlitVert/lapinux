//! Contrôle du service utilisateur `steam-puck-bridge`.
//!
//! Le pont n'est pas lancé par Lapinux : il vit dans un service systemd
//! utilisateur, car il doit garder le hidraw du Puck pendant toute la session.
//! Cette page lit son état, permet de l'arrêter ou de le relancer et ouvre son
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
    pub failed: bool,
    pub state_known: bool,
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
        Self::load_with(std::ffi::OsStr::new("systemctl"))
    }

    fn load_with(program: &std::ffi::OsStr) -> Self {
        let output = Command::new(program)
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
                        lapinux_core::i18n::t_fmt(
                            "steam_bridge.systemctl_failed",
                            &[output.status.to_string()],
                        )
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BridgeAction {
    Restart,
    Stop,
}

impl BridgeAction {
    fn command(self) -> &'static str {
        match self {
            Self::Restart => "restart",
            Self::Stop => "stop",
        }
    }

    fn success_key(self) -> &'static str {
        match self {
            Self::Restart => "controllers.steam.bridge_restarted",
            Self::Stop => "controllers.steam.bridge_stop_success",
        }
    }

    fn failure_key(self) -> &'static str {
        match self {
            Self::Restart => "controllers.steam.bridge_restart_failed",
            Self::Stop => "controllers.steam.bridge_stop_failed",
        }
    }
}

/// Ne touche qu'au service utilisateur nommé ; un arrêt ne change pas son
/// activation au démarrage. L'état est relu avant d'annoncer la réussite.
pub(super) fn control_with(
    action: BridgeAction,
    run: impl FnOnce(&str) -> Result<(), String>,
    load: impl FnOnce() -> ServiceState,
) -> Result<ServiceState, String> {
    run(action.command())?;
    let state = load();
    if let Some(error) = &state.error {
        return Err(error.clone());
    }
    if !state.installed() {
        return Err(lapinux_core::i18n::t("controllers.steam.bridge_service_missing").to_string());
    }
    if !state.state_known || state.failed || state.active != (action == BridgeAction::Restart) {
        return Err(
            lapinux_core::i18n::t("controllers.steam.bridge_state_unconfirmed").to_string(),
        );
    }
    Ok(state)
}

fn control(action: BridgeAction) -> Result<ServiceState, String> {
    control_with_program(action, std::ffi::OsStr::new("systemctl"))
}

fn control_with_program(
    action: BridgeAction,
    program: &std::ffi::OsStr,
) -> Result<ServiceState, String> {
    control_with(
        action,
        |verb| {
            let output = Command::new(program)
                .args(["--user", verb, UNIT])
                .stdin(Stdio::null())
                .output()
                .map_err(|error| format!("systemctl : {error}"))?;
            if output.status.success() {
                Ok(())
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let stderr = stderr.trim();
                Err(if stderr.is_empty() {
                    lapinux_core::i18n::t_fmt(
                        "steam_bridge.systemctl_failed",
                        &[output.status.to_string()],
                    )
                } else {
                    stderr.to_string()
                })
            }
        },
        || ServiceState::load_with(program),
    )
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
                state.active = value == "active";
                state.failed = value == "failed";
                state.state_known = matches!(value, "active" | "inactive" | "failed");
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
    action_rx: Option<Receiver<(BridgeAction, Result<ServiceState, String>)>>,
    last_poll: Instant,
    #[cfg(test)]
    pub(super) test_backend: Option<TestBackend>,
}

#[cfg(test)]
#[derive(Clone)]
pub(super) struct TestBackend {
    pub load: std::sync::Arc<dyn Fn() -> ServiceState + Send + Sync>,
    pub control: std::sync::Arc<dyn Fn(BridgeAction) -> Result<ServiceState, String> + Send + Sync>,
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
            #[cfg(test)]
            test_backend: None,
        }
    }

    pub fn visible(&self, puck_present: bool, bridge_running: bool) -> bool {
        (puck_present && bridge_running)
            || self
                .service
                .as_ref()
                .is_some_and(|service| service.installed() || service.error.is_some())
            || self.note.is_some()
    }

    /// Rend vrai quand une action a abouti et que la liste des manettes doit
    /// être relue : le pont peut créer ou retirer un périphérique virtuel.
    pub fn poll(&mut self, ctx: &egui::Context) -> bool {
        let mut action_finished = false;
        let mut action_succeeded = false;
        let received_action = self.action_rx.as_ref().map(|rx| rx.try_recv());
        match received_action {
            Some(Ok((action, Ok(state)))) => {
                self.service = Some(state);
                self.note = Some((
                    lapinux_core::i18n::t(action.success_key()).to_string(),
                    true,
                ));
                action_finished = true;
                action_succeeded = true;
            }
            Some(Ok((action, Err(error)))) => {
                self.note = Some((
                    lapinux_core::i18n::t_args(action.failure_key(), &[("error", &error)]),
                    false,
                ));
                // La commande peut avoir changé l'état malgré une erreur : le
                // prochain sondage le relira, sans annoncer de réussite.
                self.service = None;
                action_finished = true;
            }
            Some(Err(TryRecvError::Disconnected)) => {
                self.note = Some((
                    lapinux_core::i18n::t("controllers.steam.bridge_service_silent").to_string(),
                    false,
                ));
                self.service = None;
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
            #[cfg(test)]
            let backend = self.test_backend.clone();
            std::thread::spawn(move || {
                #[cfg(test)]
                let state = match backend {
                    Some(backend) => (backend.load)(),
                    None => ServiceState::load(),
                };
                #[cfg(not(test))]
                let state = ServiceState::load();
                let _ = tx.send(state);
                ctx.request_repaint();
            });
        }

        if self.busy || self.state_rx.is_some() || self.action_rx.is_some() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        action_succeeded
    }

    pub fn restart(&mut self, ctx: &egui::Context) {
        self.start_action(ctx, BridgeAction::Restart);
    }

    pub fn stop(&mut self, ctx: &egui::Context) {
        self.start_action(ctx, BridgeAction::Stop);
    }

    fn start_action(&mut self, ctx: &egui::Context, action: BridgeAction) {
        #[cfg(test)]
        if let Some(backend) = self.test_backend.clone() {
            self.start_action_with(ctx, action, move || (backend.control)(action));
            return;
        }
        self.start_action_with(ctx, action, move || control(action));
    }

    fn start_action_with(
        &mut self,
        ctx: &egui::Context,
        action: BridgeAction,
        execute: impl FnOnce() -> Result<ServiceState, String> + Send + 'static,
    ) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.note = None;
        // Une lecture demandée avant l'action ne doit pas écraser son résultat.
        self.state_rx = None;
        let (tx, rx) = mpsc::channel();
        self.action_rx = Some(rx);
        let thread_ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send((action, execute()));
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
            .ok_or_else(|| lapinux_core::i18n::t("steam_bridge.service_missing").to_string())?;
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
    use super::{control_with, parse_show, BridgeAction};

    #[derive(Clone)]
    struct SimulatedService {
        state: super::ServiceState,
        commands: Vec<String>,
        command_error: bool,
        read_error: bool,
        unchanged: bool,
    }

    fn simulated_bridge() -> (
        super::SteamBridge,
        std::sync::Arc<std::sync::Mutex<SimulatedService>>,
    ) {
        use std::sync::{Arc, Mutex};
        let mock = Arc::new(Mutex::new(SimulatedService {
            state: parse_show("LoadState=loaded\nUnitFileState=enabled\nActiveState=active\nFragmentPath=/tmp/fake-unit\n"),
            commands: Vec::new(),
            command_error: false,
            read_error: false,
            unchanged: false,
        }));
        let loader = Arc::clone(&mock);
        let controller = Arc::clone(&mock);
        let mut bridge = super::SteamBridge::new();
        bridge.test_backend = Some(super::TestBackend {
            load: Arc::new(move || {
                let mock = loader.lock().unwrap();
                if mock.read_error {
                    super::ServiceState {
                        error: Some("lecture simulée impossible".into()),
                        ..Default::default()
                    }
                } else {
                    mock.state.clone()
                }
            }),
            control: Arc::new(move |action| {
                let run_mock = Arc::clone(&controller);
                let load_mock = Arc::clone(&controller);
                control_with(
                    action,
                    move |verb| {
                        let mut mock = run_mock.lock().unwrap();
                        mock.commands.push(verb.to_string());
                        if mock.command_error {
                            return Err("commande simulée refusée".into());
                        }
                        if !mock.unchanged {
                            mock.state.active = verb == "restart";
                            mock.state.failed = false;
                        }
                        Ok(())
                    },
                    move || {
                        let mock = load_mock.lock().unwrap();
                        if mock.read_error {
                            super::ServiceState {
                                error: Some("lecture simulée impossible".into()),
                                ..Default::default()
                            }
                        } else {
                            mock.state.clone()
                        }
                    },
                )
            }),
        });
        (bridge, mock)
    }

    fn frame(
        ctx: &eframe::egui::Context,
        bridge: &mut super::SteamBridge,
        events: Vec<eframe::egui::Event>,
        time: &mut f64,
    ) -> eframe::egui::FullOutput {
        use eframe::egui;
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(356.0, 900.0),
            )),
            events,
            time: Some(*time),
            ..Default::default()
        };
        *time += 1.0 / 60.0;
        ctx.run(input, |ctx| {
            bridge.poll(ctx);
            egui::CentralPanel::default().show(ctx, |ui| {
                if bridge.visible(false, false) {
                    crate::LapinuxApp::steam_bridge_card(ui, bridge);
                }
            });
        })
    }

    fn painted_text(output: &eframe::egui::FullOutput, label: &str) -> Option<eframe::egui::Rect> {
        fn find(shape: &eframe::egui::Shape, label: &str) -> Option<eframe::egui::Rect> {
            match shape {
                eframe::egui::Shape::Text(text) if text.galley.text() == label => Some(
                    eframe::egui::Rect::from_min_size(text.pos, text.galley.size()),
                ),
                eframe::egui::Shape::Vec(shapes) => {
                    shapes.iter().find_map(|shape| find(shape, label))
                }
                _ => None,
            }
        }
        output.shapes.iter().find_map(|clipped| {
            find(&clipped.shape, label).map(|rect| {
                assert!(
                    clipped.clip_rect.contains_rect(rect),
                    "texte rogné : {label}"
                );
                rect
            })
        })
    }

    fn click(
        ctx: &eframe::egui::Context,
        bridge: &mut super::SteamBridge,
        label: &str,
        time: &mut f64,
    ) {
        use eframe::egui::{Event, Modifiers, PointerButton};
        let output = frame(ctx, bridge, Vec::new(), time);
        let pos = painted_text(&output, label)
            .unwrap_or_else(|| panic!("bouton absent : {label}"))
            .center();
        frame(ctx, bridge, vec![Event::PointerMoved(pos)], time);
        for pressed in [true, false] {
            frame(
                ctx,
                bridge,
                vec![Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::default(),
                }],
                time,
            );
        }
    }

    fn wait_for(
        ctx: &eframe::egui::Context,
        bridge: &mut super::SteamBridge,
        time: &mut f64,
        done: impl Fn(&super::SteamBridge) -> bool,
    ) -> eframe::egui::FullOutput {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let output = frame(ctx, bridge, Vec::new(), time);
            if done(bridge) {
                return output;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "attente du service simulé expirée"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    #[test]
    fn interface_stops_and_restarts_simulated_service_without_receiver() {
        let (mut bridge, mock) = simulated_bridge();
        let ctx = eframe::egui::Context::default();
        crate::apply_theme(&ctx);
        crate::install_fonts(&ctx);
        let mut time = 0.0;
        let output = wait_for(&ctx, &mut bridge, &mut time, |bridge| {
            bridge.service.is_some()
        });
        assert!(bridge.visible(false, false));
        assert!(painted_text(
            &output,
            lapinux_core::i18n::t("controllers.steam.bridge_stop_hint")
        )
        .is_some());
        click(
            &ctx,
            &mut bridge,
            lapinux_core::i18n::t("controllers.steam.bridge_stop"),
            &mut time,
        );
        let output = wait_for(&ctx, &mut bridge, &mut time, |bridge| bridge.note.is_some());
        assert!(bridge.note.as_ref().unwrap().1);
        assert!(painted_text(
            &output,
            lapinux_core::i18n::t("controllers.steam.bridge_stopped")
        )
        .is_some());
        assert!(bridge.service.as_ref().unwrap().enabled);
        click(
            &ctx,
            &mut bridge,
            lapinux_core::i18n::t("controllers.steam.bridge_start"),
            &mut time,
        );
        let output = wait_for(&ctx, &mut bridge, &mut time, |bridge| {
            bridge.note.is_some() && !bridge.busy
        });
        assert!(bridge.note.as_ref().unwrap().1);
        assert!(painted_text(
            &output,
            lapinux_core::i18n::t("controllers.steam.bridge_running")
        )
        .is_some());
        click(
            &ctx,
            &mut bridge,
            lapinux_core::i18n::t("controllers.steam.bridge_restart"),
            &mut time,
        );
        wait_for(&ctx, &mut bridge, &mut time, |bridge| {
            bridge.note.is_some() && !bridge.busy
        });
        assert!(bridge.note.as_ref().unwrap().1);
        assert_eq!(
            mock.lock().unwrap().commands,
            ["stop", "restart", "restart"]
        );
        assert!(mock.lock().unwrap().state.enabled);
    }

    #[test]
    fn interface_reports_simulated_errors_and_unconfirmed_results() {
        for (mode, restart) in [
            ("command", false),
            ("unchanged", false),
            ("read", false),
            ("command", true),
            ("unchanged", true),
            ("read", true),
        ] {
            let (mut bridge, mock) = simulated_bridge();
            if restart {
                mock.lock().unwrap().state.active = false;
            }
            let ctx = eframe::egui::Context::default();
            crate::apply_theme(&ctx);
            crate::install_fonts(&ctx);
            let mut time = 0.0;
            wait_for(&ctx, &mut bridge, &mut time, |bridge| {
                bridge.service.is_some()
            });
            {
                let mut mock = mock.lock().unwrap();
                mock.command_error = mode == "command";
                mock.unchanged = mode == "unchanged";
                mock.read_error = mode == "read";
            }
            let button = if restart {
                "controllers.steam.bridge_start"
            } else {
                "controllers.steam.bridge_stop"
            };
            click(&ctx, &mut bridge, lapinux_core::i18n::t(button), &mut time);
            wait_for(&ctx, &mut bridge, &mut time, |bridge| {
                bridge.note.is_some() && !bridge.busy
            });
            let (message, ok) = bridge.note.as_ref().unwrap();
            assert!(!ok);
            let expected = match mode {
                "command" => "commande simulée refusée",
                "read" => "lecture simulée impossible",
                _ => lapinux_core::i18n::t("controllers.steam.bridge_state_unconfirmed"),
            };
            assert!(message.contains(expected), "{message}");
            let message = message.clone();
            let output = wait_for(&ctx, &mut bridge, &mut time, |bridge| {
                bridge.service.is_some()
            });
            assert!(painted_text(&output, &message).is_some());
            if mode == "read" {
                assert!(painted_text(
                    &output,
                    lapinux_core::i18n::t("controllers.steam.bridge_unknown")
                )
                .is_some());
                assert!(painted_text(
                    &output,
                    lapinux_core::i18n::t("controllers.steam.bridge_stopped")
                )
                .is_none());
                click(
                    &ctx,
                    &mut bridge,
                    lapinux_core::i18n::t("controllers.steam.bridge_start"),
                    &mut time,
                );
            } else {
                let status = if restart {
                    "controllers.steam.bridge_stopped"
                } else {
                    "controllers.steam.bridge_running"
                };
                assert!(painted_text(&output, lapinux_core::i18n::t(status)).is_some());
            }
            assert_eq!(
                mock.lock().unwrap().commands,
                [if restart { "restart" } else { "stop" }]
            );
        }
    }

    #[test]
    fn interface_does_not_send_a_second_command_while_busy() {
        let (mut bridge, mock) = simulated_bridge();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let release_rx = std::sync::Mutex::new(release_rx);
        let backend = bridge.test_backend.as_mut().unwrap();
        backend.control = std::sync::Arc::new(move |action| {
            started_tx.send(action).unwrap();
            release_rx
                .lock()
                .unwrap()
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap();
            Ok(parse_show("LoadState=loaded\nUnitFileState=enabled\nActiveState=inactive\nFragmentPath=/tmp/unit\n"))
        });
        let ctx = eframe::egui::Context::default();
        crate::apply_theme(&ctx);
        crate::install_fonts(&ctx);
        let mut time = 0.0;
        wait_for(&ctx, &mut bridge, &mut time, |bridge| {
            bridge.service.is_some()
        });
        click(
            &ctx,
            &mut bridge,
            lapinux_core::i18n::t("controllers.steam.bridge_stop"),
            &mut time,
        );
        assert_eq!(
            started_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap(),
            BridgeAction::Stop
        );
        assert!(bridge.busy);
        click(
            &ctx,
            &mut bridge,
            lapinux_core::i18n::t("controllers.steam.bridge_stop"),
            &mut time,
        );
        assert!(started_rx.try_recv().is_err());
        assert!(mock.lock().unwrap().commands.is_empty());
        release_tx.send(()).unwrap();
        wait_for(&ctx, &mut bridge, &mut time, |bridge| {
            bridge.note.is_some() && !bridge.busy
        });
        assert!(bridge.note.as_ref().unwrap().1);
    }

    #[test]
    fn interface_runs_fake_systemctl_with_user_unit_only() {
        use std::os::unix::fs::PermissionsExt;
        let directory =
            tempfile::tempdir_in(concat!(env!("CARGO_MANIFEST_DIR"), "/../../target")).unwrap();
        let executable = directory.path().join("systemctl");
        let state = directory.path().join("state");
        let log = directory.path().join("commands");
        std::fs::write(&state, "active").unwrap();
        let script = format!(
            r#"#!/bin/sh
printf '%s\n' "$*" >> '{log}'
case "$1 $2 $3" in
  '--user stop steam-puck-bridge.service') printf inactive > '{state}' ;;
  '--user restart steam-puck-bridge.service') printf active > '{state}' ;;
  '--user show steam-puck-bridge.service')
    printf 'LoadState=loaded\nUnitFileState=enabled\nActiveState=%s\nFragmentPath=/tmp/fake-unit\n' "$(cat '{state}')" ;;
  *) exit 2 ;;
esac
"#,
            log = log.display(),
            state = state.display()
        );
        std::fs::write(&executable, script).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let program = std::sync::Arc::new(executable.into_os_string());
        let load_program = std::sync::Arc::clone(&program);
        let control_program = std::sync::Arc::clone(&program);
        let mut bridge = super::SteamBridge::new();
        bridge.test_backend = Some(super::TestBackend {
            load: std::sync::Arc::new(move || super::ServiceState::load_with(&load_program)),
            control: std::sync::Arc::new(move |action| {
                super::control_with_program(action, &control_program)
            }),
        });
        let ctx = eframe::egui::Context::default();
        crate::apply_theme(&ctx);
        crate::install_fonts(&ctx);
        let mut time = 0.0;
        wait_for(&ctx, &mut bridge, &mut time, |bridge| {
            bridge.service.is_some()
        });
        click(
            &ctx,
            &mut bridge,
            lapinux_core::i18n::t("controllers.steam.bridge_stop"),
            &mut time,
        );
        wait_for(&ctx, &mut bridge, &mut time, |bridge| {
            bridge.note.is_some() && !bridge.busy
        });
        assert!(bridge.note.as_ref().unwrap().1);
        click(
            &ctx,
            &mut bridge,
            lapinux_core::i18n::t("controllers.steam.bridge_start"),
            &mut time,
        );
        wait_for(&ctx, &mut bridge, &mut time, |bridge| {
            bridge.note.is_some() && !bridge.busy
        });
        assert!(bridge.note.as_ref().unwrap().1);
        let lines = std::fs::read_to_string(log).unwrap();
        assert_eq!(lines.lines().collect::<Vec<_>>(), [
            "--user show steam-puck-bridge.service --property=LoadState --property=UnitFileState --property=ActiveState --property=FragmentPath",
            "--user stop steam-puck-bridge.service",
            "--user show steam-puck-bridge.service --property=LoadState --property=UnitFileState --property=ActiveState --property=FragmentPath",
            "--user restart steam-puck-bridge.service",
            "--user show steam-puck-bridge.service --property=LoadState --property=UnitFileState --property=ActiveState --property=FragmentPath",
        ]);
        assert!(bridge.service.as_ref().unwrap().enabled);
    }

    #[test]
    fn interface_displays_failed_state_and_allows_recovery() {
        let (mut bridge, mock) = simulated_bridge();
        mock.lock().unwrap().state = parse_show(
            "LoadState=loaded\nUnitFileState=enabled\nActiveState=failed\nFragmentPath=/tmp/unit\n",
        );
        let ctx = eframe::egui::Context::default();
        crate::apply_theme(&ctx);
        crate::install_fonts(&ctx);
        let mut time = 0.0;
        let output = wait_for(&ctx, &mut bridge, &mut time, |bridge| {
            bridge.service.is_some()
        });
        assert!(painted_text(
            &output,
            lapinux_core::i18n::t("controllers.steam.bridge_failed")
        )
        .is_some());
        assert!(painted_text(
            &output,
            lapinux_core::i18n::t("controllers.steam.bridge_stopped")
        )
        .is_none());
        click(
            &ctx,
            &mut bridge,
            lapinux_core::i18n::t("controllers.steam.bridge_start"),
            &mut time,
        );
        let output = wait_for(&ctx, &mut bridge, &mut time, |bridge| {
            bridge.note.is_some() && !bridge.busy
        });
        assert!(bridge.note.as_ref().unwrap().1);
        assert!(painted_text(
            &output,
            lapinux_core::i18n::t("controllers.steam.bridge_running")
        )
        .is_some());
        assert_eq!(mock.lock().unwrap().commands, ["restart"]);
    }

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
    fn stop_uses_only_the_stop_action_and_confirms_inactive() {
        let stopped = parse_show("LoadState=loaded\nUnitFileState=enabled\nActiveState=inactive\nFragmentPath=/tmp/unit\n");
        let state = control_with(
            BridgeAction::Stop,
            |verb| {
                assert_eq!(verb, "stop");
                Ok(())
            },
            || stopped,
        )
        .unwrap();
        assert!(!state.active);
        assert!(
            state.enabled,
            "l'arrêt ne désactive pas le démarrage automatique"
        );
    }

    #[test]
    fn stop_reports_command_failure_or_unconfirmed_state() {
        let failed = control_with(
            BridgeAction::Stop,
            |_| Err("refus".into()),
            || panic!("la commande a échoué"),
        );
        assert_eq!(failed.unwrap_err(), "refus");
        let running = parse_show("LoadState=loaded\nActiveState=active\nFragmentPath=/tmp/unit\n");
        assert!(control_with(BridgeAction::Stop, |_| Ok(()), || running).is_err());
        let failed_read = super::ServiceState {
            error: Some("indisponible".into()),
            ..Default::default()
        };
        assert_eq!(
            control_with(BridgeAction::Stop, |_| Ok(()), || failed_read).unwrap_err(),
            "indisponible"
        );
        let missing = parse_show("LoadState=not-found\nActiveState=inactive\nFragmentPath=\n");
        assert!(control_with(BridgeAction::Stop, |_| Ok(()), || missing).is_err());
        for status in ["", "deactivating", "failed", "unexpected"] {
            let state = parse_show(&format!(
                "LoadState=loaded\nActiveState={status}\nFragmentPath=/tmp/unit\n"
            ));
            assert!(control_with(BridgeAction::Stop, |_| Ok(()), || state).is_err());
        }
    }

    #[test]
    fn stale_status_cannot_overwrite_stop_result() {
        let mut bridge = super::SteamBridge::new();
        let ctx = eframe::egui::Context::default();
        let (old_tx, old_rx) = std::sync::mpsc::channel();
        old_tx
            .send(parse_show(
                "LoadState=loaded\nActiveState=active\nFragmentPath=/tmp/unit\n",
            ))
            .unwrap();
        bridge.state_rx = Some(old_rx);
        bridge.start_action_with(&ctx, BridgeAction::Stop, || {
            Ok(parse_show(
                "LoadState=loaded\nActiveState=inactive\nFragmentPath=/tmp/unit\n",
            ))
        });
        let result = bridge
            .action_rx
            .as_ref()
            .unwrap()
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(result).unwrap();
        bridge.action_rx = Some(rx);
        assert!(bridge.poll(&ctx));
        assert!(!bridge.service.as_ref().unwrap().active);
        assert!(!bridge.busy);
    }

    #[test]
    fn restart_requires_a_confirmed_active_service() {
        for status in ["inactive", "activating", "failed"] {
            let state = parse_show(&format!(
                "LoadState=loaded\nActiveState={status}\nFragmentPath=/tmp/unit\n"
            ));
            assert!(control_with(BridgeAction::Restart, |_| Ok(()), || state).is_err());
        }
        let running = parse_show("LoadState=loaded\nActiveState=active\nFragmentPath=/tmp/unit\n");
        assert!(control_with(
            BridgeAction::Restart,
            |verb| {
                assert_eq!(verb, "restart");
                Ok(())
            },
            || running
        )
        .is_ok());
    }

    #[test]
    fn an_activating_unit_is_not_confirmed_running() {
        let state = parse_show(
            "LoadState=loaded\nUnitFileState=disabled\nActiveState=activating\nFragmentPath=/tmp/unit\n",
        );
        assert!(!state.active);
        assert!(!state.state_known);
        assert!(!state.enabled);
        assert!(state.installed());
    }
}
