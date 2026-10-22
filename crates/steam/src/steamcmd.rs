//! Intégration de SteamCMD : installation de l'outil, connexion, app_update.

use crate::meta::{self, AppInfo};
use crate::tools;
use crate::{Error, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

pub const STEAMCMD_URL: &str =
    "https://steamcdn-a.akamaihd.net/client/installer/steamcmd_linux.tar.gz";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    /// Télécharge le dépôt Windows (utile avec Proton) si disponible.
    Windows,
    Linux,
    Auto,
}

impl Platform {
    pub fn arg(self) -> Option<&'static str> {
        match self {
            Platform::Windows => Some("windows"),
            Platform::Linux => Some("linux"),
            Platform::Auto => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct InstallArgs {
    pub username: String,
    pub password: Option<String>,
    pub appid: u32,
    pub target: PathBuf,
    pub platform: Platform,
    pub validate: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SteamEvent {
    Line(String),
    Progress { percent: f32, downloaded: u64, total: u64 },
    Done,
    Fatal(String),
}

pub struct SteamCmd {
    pub root: PathBuf,
}

impl SteamCmd {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            root: data_dir.join("tools/steamcmd"),
        }
    }

    pub fn binary(&self) -> PathBuf {
        self.root.join("steamcmd.sh")
    }

    pub fn available(&self) -> bool {
        ludus_core::paths::is_executable(&self.binary())
    }

    /// Télécharge et initialise SteamCMD si nécessaire.
    pub fn ensure(&self, on_line: &mut dyn FnMut(&str)) -> Result<()> {
        if self.available() {
            return Ok(());
        }
        std::fs::create_dir_all(&self.root)?;
        let archive = self.root.join("steamcmd_linux.tar.gz");
        on_line("téléchargement de SteamCMD…");
        tools::download_to(STEAMCMD_URL, &archive, &mut |done, total| {
            if let Some(total) = total {
                if done == total {
                    tracing::debug!("SteamCMD téléchargé : {done}/{total}");
                }
            }
        })?;
        on_line("extraction de SteamCMD…");
        tools::extract_tar_gz(&archive, &self.root)?;
        std::fs::remove_file(&archive).ok();
        if !self.available() {
            return Err(Error::Other(format!(
                "SteamCMD extrait mais {} est absent",
                self.binary().display()
            )));
        }
        tools::chmod_exec(&self.binary())?;
        self.bootstrap(on_line)?;
        Ok(())
    }

    /// Premier lancement (`+quit`) qui télécharge le client complet.
    pub fn bootstrap(&self, on_line: &mut dyn FnMut(&str)) -> Result<()> {
        let mut cmd = Command::new(self.binary());
        cmd.arg("+quit");
        let outcome = ludus_core::proc::run_streaming(&mut cmd, &mut |line| {
            on_line(line);
            true
        })?;
        if !outcome.success() {
            return Err(Error::Other(format!(
                "initialisation de SteamCMD échouée ({})",
                outcome.status
            )));
        }
        Ok(())
    }

    /// Connexion interactive dans le terminal courant (mot de passe et 2FA
    /// saisis par SteamCMD lui-même, jamais stockés par Ludus).
    pub fn login_interactive(&self, username: &str) -> Result<()> {
        if !self.available() {
            return Err(Error::Other(
                "SteamCMD n'est pas installé : lancez « ludus cli tools install steamcmd »".into(),
            ));
        }
        let status = Command::new(self.binary())
            .arg("+login")
            .arg(username)
            .arg("+quit")
            .status()?;
        if !status.success() {
            return Err(Error::Other(format!(
                "connexion Steam échouée (code {})",
                status.code().unwrap_or(-1)
            )));
        }
        Ok(())
    }

    pub fn command(&self, args: &InstallArgs) -> Command {
        let mut cmd = Command::new(self.binary());
        cmd.arg("+force_install_dir").arg(&args.target);
        if let Some(platform) = args.platform.arg() {
            cmd.arg("+@sSteamCmdForcePlatformType").arg(platform);
        }
        cmd.arg("+login").arg(&args.username);
        cmd.arg("+app_update").arg(args.appid.to_string());
        if args.validate {
            cmd.arg("validate");
        }
        cmd.arg("+quit");
        cmd
    }

    /// Installe/mets à jour une app ; renvoie `Ok` si SteamCMD réussit.
    pub fn install(
        &self,
        args: &InstallArgs,
        on_event: &mut dyn FnMut(SteamEvent) -> bool,
    ) -> Result<()> {
        if !self.available() {
            return Err(Error::Other("SteamCMD n'est pas installé".into()));
        }
        let mut cmd = self.command(args);
        let mut fatal: Option<String> = None;
        let outcome = ludus_core::proc::run_streaming(&mut cmd, &mut |line| {
            let event = if let Some((percent, done, total)) = parse_progress(line) {
                SteamEvent::Progress {
                    percent,
                    downloaded: done,
                    total,
                }
            } else {
                if let Some(message) = fatal_error(line) {
                    fatal = Some(message.clone());
                }
                SteamEvent::Line(line.to_string())
            };
            on_event(event)
        })?;
        if let Some(message) = fatal {
            on_event(SteamEvent::Fatal(message.clone()));
            return Err(Error::Other(message));
        }
        if !outcome.success() {
            return Err(Error::Other(format!(
                "SteamCMD s'est arrêté avec le code {}",
                outcome.status.code().unwrap_or(-1)
            )));
        }
        on_event(SteamEvent::Done);
        Ok(())
    }

    /// Récupère les métadonnées publiques d'une app (login anonyme).
    pub fn app_info(&self, appid: u32) -> Result<AppInfo> {
        if !self.available() {
            return Err(Error::Other("SteamCMD n'est pas installé".into()));
        }
        let mut cmd = Command::new(self.binary());
        cmd.arg("+login")
            .arg("anonymous")
            .arg("+app_info_print")
            .arg(appid.to_string())
            .arg("+quit");
        let (status, stdout, stderr) = ludus_core::proc::run_capture(&mut cmd)?;
        if !status.success() {
            return Err(Error::Other(format!(
                "app_info_print {appid} a échoué : {}",
                stderr.lines().last().unwrap_or("")
            )));
        }
        meta::parse_app_info(&stdout)
            .ok_or_else(|| Error::Other(format!("métadonnées illisibles pour {appid}")))
    }
}

/// `Update state (0x61) downloading, progress: 12.34 (1234 / 10000)`
pub fn parse_progress(line: &str) -> Option<(f32, u64, u64)> {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(r"progress:\s*([0-9]+(?:\.[0-9]+)?)\s*\((\d+)\s*/\s*(\d+)\)").unwrap()
    });
    let caps = re.captures(line)?;
    let percent: f32 = caps.get(1)?.as_str().parse().ok()?;
    let downloaded: u64 = caps.get(2)?.as_str().parse().ok()?;
    let total: u64 = caps.get(3)?.as_str().parse().ok()?;
    Some((percent, downloaded, total))
}

pub fn fatal_error(line: &str) -> Option<String> {
    let lower = line.to_ascii_lowercase();
    let markers = [
        "error! failed to install",
        "no subscription",
        "invalid platform",
        "login failure",
        "failed to install app",
        "app state is 0x",
    ];
    if markers.iter().any(|m| lower.contains(m)) {
        return Some(line.trim().to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn parses_progress_lines() {
        let line = " Update state (0x61) downloading, progress: 42.13 (4213000 / 10000000)";
        assert_eq!(parse_progress(line), Some((42.13, 4213000, 10000000)));
        assert_eq!(parse_progress("Update state (0x81) preallocate"), None);
        assert_eq!(parse_progress("Nothing to do"), None);
    }

    #[test]
    fn detects_fatal_errors() {
        assert!(fatal_error("ERROR! Failed to install app '440' (No subscription)").is_some());
        assert!(fatal_error("Success! App '440' fully installed.").is_none());
    }

    #[test]
    fn command_includes_platform_and_validate() {
        let cmd = SteamCmd::new(Path::new("/tmp/ludus-test"));
        let args = InstallArgs {
            username: "bob".into(),
            password: None,
            appid: 440,
            target: PathBuf::from("/mnt/jeux"),
            platform: Platform::Windows,
            validate: true,
        };
        let built = cmd.command(&args);
        let argv: Vec<String> = built
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert_eq!(
            argv,
            vec![
                "+force_install_dir",
                "/mnt/jeux",
                "+@sSteamCmdForcePlatformType",
                "windows",
                "+login",
                "bob",
                "+app_update",
                "440",
                "validate",
                "+quit"
            ]
        );
    }

    /// Sérialise write+exec des faux binaires : un fork concurrent peut hériter
    /// du descripteur d'écriture et provoquer ETXTBSY au moment de l'exécution.
    fn exec_lock() -> std::sync::MutexGuard<'static, ()> {
        use std::sync::{Mutex, OnceLock};
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    fn fake_tool(root: &Path, body: &str) -> SteamCmd {
        let steamcmd = SteamCmd {
            root: root.to_path_buf(),
        };
        std::fs::create_dir_all(&steamcmd.root).unwrap();
        let bin = steamcmd.binary();
        // Écriture puis renommage atomique : éviter ETXTBSY quand un autre test
        // exécute un script pendant que celui-ci est encore ouvert en écriture.
        let tmp = bin.with_extension("tmp");
        std::fs::write(&tmp, body).unwrap();
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::rename(&tmp, &bin).unwrap();
        steamcmd
    }

    #[test]
    fn install_streams_progress_and_reports_done() {
        let _guard = exec_lock();
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("lib");
        let script = r#"#!/bin/sh
dir=""
while [ $# -gt 0 ]; do
  if [ "$1" = "+force_install_dir" ]; then dir="$2"; shift 2; continue; fi
  shift
done
echo "Logging in user 'bob' to Steam Public..."
echo " Update state (0x61) downloading, progress: 42.50 (4250 / 10000)"
mkdir -p "$dir/steamapps/common/Team Fortress 2"
echo "Success! App '440' fully installed."
"#;
        let steamcmd = fake_tool(dir.path(), script);
        let args = InstallArgs {
            username: "bob".into(),
            password: None,
            appid: 440,
            target: target.clone(),
            platform: Platform::Auto,
            validate: false,
        };
        let mut progress = None;
        let mut done = false;
        steamcmd
            .install(&args, &mut |event| match event {
                SteamEvent::Progress { percent, .. } => {
                    progress = Some(percent);
                    true
                }
                SteamEvent::Done => {
                    done = true;
                    true
                }
                _ => true,
            })
            .unwrap();
        assert_eq!(progress, Some(42.5));
        assert!(done);
        assert!(target.join("steamapps/common/Team Fortress 2").is_dir());
    }

    #[test]
    fn install_reports_failure() {
        let _guard = exec_lock();
        let dir = tempfile::tempdir().unwrap();
        let script = "#!/bin/sh\necho \"ERROR! Failed to install app '440' (No subscription)\"\nexit 0\n";
        let steamcmd = fake_tool(dir.path(), script);
        let args = InstallArgs {
            username: "bob".into(),
            password: None,
            appid: 440,
            target: dir.path().join("lib"),
            platform: Platform::Auto,
            validate: false,
        };
        let err = steamcmd.install(&args, &mut |_| true).unwrap_err();
        assert!(err.to_string().contains("subscription"));
    }

    #[test]
    fn app_info_parses_fake_output() {
        let _guard = exec_lock();
        let dir = tempfile::tempdir().unwrap();
        let script = r#"#!/bin/sh
echo "Loading Steam API...OK"
echo '"440"'
echo '{'
echo '  "appid" "440"'
echo '  "common" { "name" "Team Fortress 2" }'
echo '  "config" { "installdir" "Team Fortress 2" }'
echo '}'
"#;
        let steamcmd = fake_tool(dir.path(), script);
        let info = steamcmd.app_info(440).unwrap();
        assert_eq!(info.name, "Team Fortress 2");
        assert_eq!(info.installdir, "Team Fortress 2");
    }
}
