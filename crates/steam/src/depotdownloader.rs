//! Backend de repli : DepotDownloader (outil tiers, opt-in).
//!
//! Utile quand SteamCMD refuse une app ou pour choisir un manifeste précis.
//! L'outil n'est pas officiel : son utilisation est désactivée par défaut
//! dans la configuration (`steam.allow_depotdownloader`).

use crate::steamcmd::{InstallArgs, SteamEvent};
use crate::tools;
use crate::{Error, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

pub const RELEASE_API: &str = "https://api.github.com/repos/SteamRE/DepotDownloader/releases/latest";
pub const ASSET_SUFFIX: &str = "DepotDownloader-linux-x64.zip";

pub struct DepotDownloader {
    pub root: PathBuf,
}

impl DepotDownloader {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            root: data_dir.join("tools/depotdownloader"),
        }
    }

    pub fn binary(&self) -> PathBuf {
        self.root.join("DepotDownloader")
    }

    pub fn available(&self) -> bool {
        ludus_core::paths::is_executable(&self.binary())
    }

    /// Télécharge la dernière version linux-x64 (self-contained, sans .NET).
    pub fn ensure(&self, on_line: &mut dyn FnMut(&str)) -> Result<()> {
        if self.available() {
            return Ok(());
        }
        std::fs::create_dir_all(&self.root)?;
        on_line("recherche de DepotDownloader…");
        let json = tools::fetch_text(RELEASE_API)?;
        let release: serde_json::Value = serde_json::from_str(&json)?;
        let assets = release
            .get("assets")
            .and_then(|a| a.as_array())
            .ok_or_else(|| Error::Other("réponse GitHub inattendue".into()))?;
        let asset = assets
            .iter()
            .find(|a| {
                a.get("name")
                    .and_then(|n| n.as_str())
                    .map(|n| n.ends_with(ASSET_SUFFIX))
                    .unwrap_or(false)
            })
            .ok_or_else(|| Error::Other(format!("asset {ASSET_SUFFIX} introuvable")))?;
        let url = asset
            .get("browser_download_url")
            .and_then(|u| u.as_str())
            .ok_or_else(|| Error::Other("URL d'asset absente".into()))?;
        let archive = self.root.join("depotdownloader.zip");
        on_line("téléchargement de DepotDownloader…");
        tools::download_to(url, &archive, &mut |_, _| {})?;
        on_line("extraction de DepotDownloader…");
        tools::extract_zip_entry(&archive, "DepotDownloader", &self.binary())?;
        std::fs::remove_file(&archive).ok();
        Ok(())
    }

    pub fn command(&self, args: &InstallArgs) -> Command {
        let mut cmd = Command::new(self.binary());
        cmd.arg("-app").arg(args.appid.to_string());
        cmd.arg("-dir").arg(&args.target);
        cmd.arg("-username").arg(&args.username);
        if let Some(password) = &args.password {
            cmd.arg("-password").arg(password);
        }
        if args.validate {
            cmd.arg("-validate");
        }
        cmd.arg("-remember-password");
        cmd
    }

    pub fn install(
        &self,
        args: &InstallArgs,
        on_event: &mut dyn FnMut(SteamEvent) -> bool,
    ) -> Result<()> {
        if !self.available() {
            return Err(Error::Other("DepotDownloader n'est pas installé".into()));
        }
        let mut cmd = self.command(args);
        let mut fatal: Option<String> = None;
        let outcome = ludus_core::proc::run_streaming(&mut cmd, &mut |line| {
            let event = if let Some(percent) = parse_percent(line) {
                SteamEvent::Progress {
                    percent,
                    downloaded: 0,
                    total: 0,
                }
            } else {
                let lower = line.to_ascii_lowercase();
                if lower.contains("error") || lower.contains("failed") {
                    fatal = Some(line.trim().to_string());
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
                "DepotDownloader a échoué (code {})",
                outcome.status.code().unwrap_or(-1)
            )));
        }
        on_event(SteamEvent::Done);
        Ok(())
    }
}

pub fn parse_percent(line: &str) -> Option<f32> {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r"([0-9]+(?:\.[0-9]+)?)\s*%").unwrap());
    re.captures(line)
        .and_then(|caps| caps.get(1))
        .and_then(|m| m.as_str().parse().ok())
        .map(|p: f32| p.clamp(0.0, 100.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_percentages() {
        assert_eq!(parse_percent("Downloading depot 228981 (12.5%)"), Some(12.5));
        assert_eq!(parse_percent("Total downloaded: 100%"), Some(100.0));
        assert_eq!(parse_percent("Starting"), None);
    }

    #[test]
    fn command_has_expected_flags() {
        let tool = DepotDownloader::new(Path::new("/tmp/x"));
        let args = InstallArgs {
            username: "bob".into(),
            password: Some("secret".into()),
            appid: 440,
            target: PathBuf::from("/mnt/jeux"),
            platform: crate::steamcmd::Platform::Auto,
            validate: false,
        };
        let argv: Vec<String> = tool
            .command(&args)
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert_eq!(
            argv,
            vec![
                "-app",
                "440",
                "-dir",
                "/mnt/jeux",
                "-username",
                "bob",
                "-password",
                "secret",
                "-remember-password"
            ]
        );
    }
}
