//! Modèles partagés entre le daemon, l'interface, l'overlay et la CLI.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Drm {
    /// Aucun DRM Steam détecté.
    None,
    /// Utilise steam_api (statuts) mais devrait démarrer sans Steam.
    Steamworks,
    /// SteamStub/CEG : nécessite le client Steam.
    SteamStub,
    /// Non encore analysé.
    Unknown,
}

impl Drm {
    pub fn label(self) -> &'static str {
        match self {
            Drm::None => "sans DRM",
            Drm::Steamworks => "Steamworks",
            Drm::SteamStub => "DRM Steam",
            Drm::Unknown => "inconnu",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct GameSettings {
    /// Nom du Proton à utiliser, sinon celui de la config.
    pub proton: Option<String>,
    pub gamescope: Option<bool>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps_cap: Option<u32>,
    pub mangohud: Option<bool>,
    pub hdr: Option<bool>,
    pub adaptive_sync: Option<bool>,
    /// Variables d'environnement additionnelles.
    pub env: Vec<(String, String)>,
    /// Arguments ajoutés après le chemin de l'exécutable.
    pub args: Vec<String>,
    /// Profil manettes associé.
    pub input_profile: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Game {
    pub appid: u32,
    pub name: String,
    pub installdir: String,
    pub library: PathBuf,
    pub size_on_disk: u64,
    pub state_flags: u32,
    pub last_updated: i64,
    pub build_id: String,
    pub installed: bool,
    pub prefix: Option<PathBuf>,
    pub exe: Option<PathBuf>,
    /// Jeu Linux natif (pas besoin de Proton).
    pub native: bool,
    pub drm: Drm,
    pub settings: GameSettings,
    pub playtime_secs: u64,
    pub last_played: Option<i64>,
    pub favorite: bool,
}

impl Default for Game {
    fn default() -> Self {
        Self {
            appid: 0,
            name: String::new(),
            installdir: String::new(),
            library: PathBuf::new(),
            size_on_disk: 0,
            state_flags: 0,
            last_updated: 0,
            build_id: String::new(),
            installed: false,
            prefix: None,
            exe: None,
            native: false,
            drm: Drm::Unknown,
            settings: GameSettings::default(),
            playtime_secs: 0,
            last_played: None,
            favorite: false,
        }
    }
}

impl Game {
    pub fn install_dir(&self) -> PathBuf {
        self.library
            .join("steamapps")
            .join("common")
            .join(&self.installdir)
    }

    /// Lancement bloqué par un DRM nécessitant le client Steam.
    pub fn blocked_by_drm(&self) -> bool {
        self.drm == Drm::SteamStub
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProtonInfo {
    pub name: String,
    pub path: PathBuf,
    /// "steam", "compatibilitytools", "ludus" ou "umu".
    pub source: String,
    pub version: String,
    /// Exécutable `proton` présent.
    pub runnable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProtonRelease {
    pub repo: String,
    pub tag: String,
    pub asset: String,
    pub asset_url: String,
    pub size: u64,
    pub checksum_url: Option<String>,
    pub published: String,
    pub prerelease: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobKind {
    InstallGame,
    InstallProton,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobState {
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
}

impl JobState {
    pub fn label(self) -> &'static str {
        match self {
            JobState::Queued => "en attente",
            JobState::Running => "en cours",
            JobState::Done => "terminé",
            JobState::Failed => "échec",
            JobState::Cancelled => "annulé",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Job {
    pub id: String,
    pub kind: JobKind,
    pub label: String,
    pub appid: Option<u32>,
    pub state: JobState,
    /// 0.0 à 1.0 (peut rester à 0 si la progression est inconnue).
    pub progress: f32,
    pub downloaded: u64,
    pub total: u64,
    pub message: String,
    pub backend: String,
    pub started: i64,
    pub finished: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionInfo {
    pub appid: u32,
    pub name: String,
    pub pid: u32,
    pub started: i64,
    pub proton: String,
    pub fps_cap: u32,
    pub mangohud: bool,
    pub overlay_open: bool,
    pub input_profile: String,
    pub log_path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheckStatus {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Check {
    pub name: String,
    pub status: CheckStatus,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct DoctorReport {
    pub checks: Vec<Check>,
}

impl DoctorReport {
    pub fn worst(&self) -> CheckStatus {
        if self.checks.iter().any(|c| c.status == CheckStatus::Fail) {
            CheckStatus::Fail
        } else if self.checks.iter().any(|c| c.status == CheckStatus::Warn) {
            CheckStatus::Warn
        } else {
            CheckStatus::Ok
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeviceInfo {
    pub path: PathBuf,
    pub name: String,
    pub vendor: u16,
    pub product: u16,
    pub gamepad: bool,
    pub buttons: usize,
    pub axes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProfileSummary {
    pub name: String,
    pub path: PathBuf,
    pub is_default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Stats {
    pub launches: u64,
    pub total_seconds: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn game_install_dir() {
        let mut g = Game::default();
        g.library = PathBuf::from("/mnt/Jeux/Steam");
        g.installdir = "Team Fortress 2".into();
        assert_eq!(
            g.install_dir(),
            PathBuf::from("/mnt/Jeux/Steam/steamapps/common/Team Fortress 2")
        );
    }

    #[test]
    fn drm_blocks_steamstub_only() {
        let mut g = Game::default();
        g.drm = Drm::Steamworks;
        assert!(!g.blocked_by_drm());
        g.drm = Drm::SteamStub;
        assert!(g.blocked_by_drm());
    }

    #[test]
    fn game_serde_defaults() {
        let g: Game = serde_json::from_str("{\"appid\":10,\"name\":\"X\"}").unwrap();
        assert_eq!(g.drm, Drm::Unknown);
        assert_eq!(g.settings.env.len(), 0);
    }
}
