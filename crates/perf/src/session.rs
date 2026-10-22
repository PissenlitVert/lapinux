//! Composition de la commande finale et supervision de la session de jeu.

use crate::gamemode::{self, PerfCaps};
use crate::gamescope::{self, GamescopeSettings};
use crate::mangohud::{self, HudSettings};
use crate::{Error, Result};
use ludus_proton::LaunchPlan as Plan;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq)]
pub struct PerfConfig {
    pub gamescope: GamescopeSettings,
    pub hud: HudSettings,
    pub gamemode: bool,
}

impl Default for PerfConfig {
    fn default() -> Self {
        Self {
            gamescope: GamescopeSettings::default(),
            hud: HudSettings::default(),
            gamemode: true,
        }
    }
}

pub struct PreparedLaunch {
    pub command: Command,
    pub log_path: PathBuf,
    pub gamescope: bool,
    pub mangohud: bool,
    pub gamemode: bool,
}

pub struct Session {
    pub appid: u32,
    pub pid: u32,
    pub log_path: PathBuf,
    pub started: Instant,
    child: std::process::Child,
}

pub struct SessionOutcome {
    pub code: Option<i32>,
    pub duration: Duration,
}

pub fn prepare(
    plan: &Plan,
    appid: u32,
    config: &PerfConfig,
    caps: &PerfCaps,
    config_dir: &std::path::Path,
    log_dir: &std::path::Path,
) -> Result<PreparedLaunch> {
    let mut env = plan.env.clone();
    let mut program = plan.program.clone();
    let mut args = plan.args.clone();
    let mut use_gamescope = false;
    let mut use_mangohud = false;

    let gamescope_available = caps.gamescope.clone();
    let hud_available = caps.mangohud.is_some();

    if config.gamescope.enabled {
        if let Some(gamescope_bin) = gamescope_available {
            use_gamescope = true;
            let mut settings = config.gamescope.clone();
            if settings.mangoapp && !hud_available {
                settings.mangoapp = false;
            }
            let mut scope_args = gamescope::args(&settings);
            scope_args.push("--".into());
            scope_args.push(program.to_string_lossy().to_string());
            scope_args.extend(args.iter().cloned());
            program = gamescope_bin;
            args = scope_args;
            if settings.mangoapp {
                use_mangohud = true;
            }
        } else {
            tracing::warn!("gamescope demandé mais introuvable : lancement direct");
        }
    }

    if !use_gamescope && config.hud.enabled && hud_available {
        match mangohud::write_config(config_dir, appid, &config.hud) {
            Ok(path) => {
                env.extend(mangohud::env(&path));
                use_mangohud = true;
            }
            Err(err) => tracing::warn!("configuration MangoHud : {err}"),
        }
    }

    let wrapped = gamemode::wrap(&program, &args, caps, config.gamemode);
    program = wrapped.program;
    args = wrapped.args;

    let mut command = Command::new(&program);
    command.args(&args);
    for (key, value) in &env {
        command.env(key, value);
    }
    command.env("LUDUS_APPID", appid.to_string());
    if let Some(cwd) = &plan.cwd {
        command.current_dir(cwd);
    }
    std::fs::create_dir_all(log_dir).map_err(Error::Io)?;
    let log_path = log_dir.join(format!("{appid}-{}.log", ludus_core::util::now_unix()));
    command.stdout(Stdio::null()).stderr(Stdio::null());

    Ok(PreparedLaunch {
        command,
        log_path,
        gamescope: use_gamescope,
        mangohud: use_mangohud,
        gamemode: wrapped.gamemode,
    })
}

impl Session {
    pub fn spawn(appid: u32, mut prepared: PreparedLaunch) -> Result<Self> {
        use std::os::unix::process::CommandExt;
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&prepared.log_path)
            .map_err(Error::Io)?;
        let log_err = log.try_clone().map_err(Error::Io)?;
        prepared.command.stdout(Stdio::from(log));
        prepared.command.stderr(Stdio::from(log_err));
        unsafe {
            prepared.command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = prepared.command.spawn().map_err(Error::Io)?;
        let pid = child.id();
        Ok(Self {
            appid,
            pid,
            log_path: prepared.log_path,
            started: Instant::now(),
            child,
        })
    }

    pub fn try_wait(&mut self) -> Result<Option<SessionOutcome>> {
        match self.child.try_wait().map_err(Error::Io)? {
            Some(status) => Ok(Some(SessionOutcome {
                code: status.code(),
                duration: self.started.elapsed(),
            })),
            None => Ok(None),
        }
    }

    pub fn wait(&mut self) -> Result<SessionOutcome> {
        let status = self.child.wait().map_err(Error::Io)?;
        Ok(SessionOutcome {
            code: status.code(),
            duration: self.started.elapsed(),
        })
    }

    /// Envoie SIGTERM au groupe de processus puis SIGKILL après un délai.
    pub fn terminate(&mut self) {
        let pid = self.pid as i32;
        unsafe {
            libc::kill(-pid, libc::SIGTERM);
        }
        for _ in 0..20 {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }
}

/// Représentation lisible de la commande préparée (dry-run, journaux).
pub fn render_prepared(prepared: &PreparedLaunch) -> String {
    let mut out = String::new();
    for (key, value) in prepared.command.get_envs() {
        if let Some(value) = value {
            out.push_str(&format!(
                "{}={}\n",
                key.to_string_lossy(),
                value.to_string_lossy()
            ));
        }
    }
    out.push_str(&prepared.command.get_program().to_string_lossy());
    for arg in prepared.command.get_args() {
        out.push(' ');
        out.push_str(&arg.to_string_lossy());
    }
    out
}

/// Résumé lisible des couches activées, pour l'UI et les journaux.
pub fn layers_summary(gamescope: bool, mangohud: bool, gamemode: bool) -> String {
    let mut parts = Vec::new();
    if gamescope {
        parts.push("gamescope");
    }
    if mangohud {
        parts.push("MangoHud");
    }
    if gamemode {
        parts.push("GameMode");
    }
    if parts.is_empty() {
        "lancement direct".into()
    } else {
        parts.join(" + ")
    }
}
