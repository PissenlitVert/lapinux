//! Contrôle du GameScope installé : version et options réellement acceptées.
//!
//! GameScope porte le HUD, l'overlay et la scène de lancement et de fin. Un
//! binaire trop ancien, ou qui ne connaît pas une option de la commande
//! construite par [`crate::gamescope::args`], ne doit pas faire échouer le
//! lancement : le contrôle rend un verdict, le lancement retombe alors sur le
//! mode direct et le motif accompagne la session.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Version minimale. La scène persistante, `-T` et le plafond réglable à
/// chaud datent de cette série ; en dessous, la limite de cadence resterait
/// figée au lancement.
pub const MINIMUM: (u32, u32) = (3, 16);

/// Options que la commande peut passer. GameScope sort sur un argument
/// inconnu : mieux vaut le lancement direct qu'une session morte.
pub const REQUIRED: &[&str] = &[
    "--framerate-limit",
    "--adaptive-sync",
    "--hdr-enabled",
    "--prefer-output",
    "--sharpness",
    "--force-grab-cursor",
];

/// Délai laissé à `--version` et `--help` : un binaire bloqué ne doit pas
/// retenir le lancement du jeu.
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GamescopeVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl GamescopeVersion {
    /// Première version rencontrée dans le texte. `gamescope --version` écrit
    /// `gamescope version 3.17.0` ; d'autres constructions ajoutent des
    /// suffixes (`3.17.0-1`, `v3.17.0`), que la lecture ignore.
    pub fn parse(text: &str) -> Option<Self> {
        let bytes = text.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            if !bytes[index].is_ascii_digit() {
                index += 1;
                continue;
            }
            let start = index;
            let mut parts = [0_u32; 3];
            let mut count = 0;
            loop {
                let digits = index;
                while index < bytes.len() && bytes[index].is_ascii_digit() {
                    index += 1;
                }
                if digits == index || count == 3 {
                    break;
                }
                parts[count] = text[digits..index].parse().unwrap_or(0);
                count += 1;
                if index < bytes.len() && bytes[index] == b'.' && count < 3 {
                    index += 1;
                    continue;
                }
                break;
            }
            // Un nombre isolé (« 1 ») n'est pas une version : il faut au
            // moins majeur et mineur.
            if count >= 2 {
                return Some(Self {
                    major: parts[0],
                    minor: parts[1],
                    patch: parts[2],
                });
            }
            if index == start {
                index += 1;
            }
        }
        None
    }

    pub fn at_least(&self, major: u32, minor: u32) -> bool {
        (self.major, self.minor) >= (major, minor)
    }
}

impl std::fmt::Display for GamescopeVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GamescopeProbe {
    /// Le binaire s'est-il seulement exécuté ? Un script d'enveloppe posé
    /// devant GameScope existe sur certaines distributions : il répond, même
    /// s'il ne sait pas rendre `--version`, et reste utilisable.
    pub responded: bool,
    pub version: Option<GamescopeVersion>,
    /// Options de [`REQUIRED`] absentes de l'aide du binaire.
    pub missing: Vec<&'static str>,
    /// Diagnostic quand la version n'a pas pu être lue.
    pub error: Option<String>,
}

impl GamescopeProbe {
    /// Motif du refus, `None` quand la session peut passer par GameScope.
    ///
    /// Une version illisible ne fait pas refuser le compositeur : beaucoup de
    /// distributions l'enveloppent d'un script qui ne répond pas à
    /// `--version`. Seules une version *connue* et trop ancienne, ou une
    /// option absente de l'aide, écartent GameScope — dans ces deux cas la
    /// commande échouerait au lancement.
    pub fn problem(&self) -> Option<String> {
        if !self.responded {
            return Some(format!(
                "GameScope ne répond pas ({}) : lancement direct",
                self.error.as_deref().unwrap_or("aucune réponse")
            ));
        }
        if let Some(version) = self.version {
            if !version.at_least(MINIMUM.0, MINIMUM.1) {
                return Some(format!(
                    "GameScope {version} est trop ancien (minimum {}.{}) : lancement direct, \
                     sans HUD ni écran de lancement",
                    MINIMUM.0, MINIMUM.1
                ));
            }
            if !self.missing.is_empty() {
                return Some(format!(
                    "GameScope {version} ne connaît pas {} : lancement direct, sans HUD ni \
                     écran de lancement",
                    self.missing.join(", ")
                ));
            }
        }
        None
    }

    pub fn usable(&self) -> bool {
        self.problem().is_none()
    }
}

/// Interroge un binaire. Le résultat est retenu par chemin et par date de
/// modification : relancer dix jeux ne relance pas vingt processus.
pub fn check(binary: &Path) -> GamescopeProbe {
    type Cache = Mutex<Vec<(PathBuf, Option<std::time::SystemTime>, GamescopeProbe)>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let modified = std::fs::metadata(binary).and_then(|meta| meta.modified()).ok();
    let cache = CACHE.get_or_init(|| Mutex::new(Vec::new()));
    if let Ok(guard) = cache.lock() {
        if let Some((_, _, probe)) = guard
            .iter()
            .find(|(path, stamp, _)| path == binary && *stamp == modified)
        {
            return probe.clone();
        }
    }
    let probe = inspect(binary);
    if let Ok(mut guard) = cache.lock() {
        guard.retain(|(path, _, _)| path != binary);
        guard.push((binary.to_path_buf(), modified, probe.clone()));
    }
    probe
}

/// Exécute le binaire et lit ce qu'il écrit, sans attendre indéfiniment.
fn run(binary: &Path, arg: &str) -> std::io::Result<String> {
    let mut child = Command::new(binary)
        .arg(arg)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    // Les deux tuyaux sont lus en parallèle : un binaire bavard sur l'un ne
    // bloque pas l'autre.
    let (sender, receiver) = mpsc::channel::<String>();
    if let Some(pipe) = child.stdout.take() {
        drain(pipe, sender.clone());
    }
    if let Some(pipe) = child.stderr.take() {
        drain(pipe, sender.clone());
    }
    drop(sender);
    let deadline = Instant::now() + PROBE_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(15));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
        }
    }
    let mut text = String::new();
    while let Ok(part) = receiver.recv_timeout(Duration::from_millis(200)) {
        text.push_str(&part);
    }
    Ok(text)
}

/// Lit un tuyau jusqu'à sa fermeture, sans jamais bloquer l'appelant : un
/// processus qui garde son tuyau ouvert ne retient pas le lancement.
fn drain<R: Read + Send + 'static>(mut pipe: R, sender: mpsc::Sender<String>) {
    std::thread::spawn(move || {
        let mut text = String::new();
        let _ = pipe.read_to_string(&mut text);
        let _ = sender.send(text);
    });
}

pub fn inspect(binary: &Path) -> GamescopeProbe {
    let version_text = match run(binary, "--version") {
        Ok(text) => text,
        Err(error) => {
            return GamescopeProbe {
                responded: false,
                version: None,
                missing: Vec::new(),
                error: Some(format!("exécution impossible : {error}")),
            };
        }
    };
    let version = GamescopeVersion::parse(&version_text);
    let error = version.is_none().then(|| {
        let first = version_text
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .to_string();
        if first.is_empty() {
            "aucune réponse à --version".to_string()
        } else {
            format!("version illisible « {first} »")
        }
    });
    // Sans aide lisible, la liste des options ne peut pas être vérifiée : on
    // garde la version comme seule preuve.
    let help = run(binary, "--help").unwrap_or_default();
    let missing = if help.trim().is_empty() {
        Vec::new()
    } else {
        REQUIRED
            .iter()
            .copied()
            .filter(|option| !help.contains(option))
            .collect()
    };
    GamescopeProbe {
        responded: true,
        version,
        missing,
        error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_version_se_lit_dans_les_formats_connus() {
        let version = GamescopeVersion::parse("gamescope version 3.17.0").unwrap();
        assert_eq!((version.major, version.minor, version.patch), (3, 17, 0));
        let version = GamescopeVersion::parse("gamescope 3.16.2-1 (gcc 14)").unwrap();
        assert_eq!((version.major, version.minor, version.patch), (3, 16, 2));
        let version = GamescopeVersion::parse("v3.15.14").unwrap();
        assert_eq!((version.major, version.minor, version.patch), (3, 15, 14));
        // Un nombre isolé n'est pas une version.
        assert!(GamescopeVersion::parse("gamescope").is_none());
        assert!(GamescopeVersion::parse("version 3").is_none());
    }

    #[test]
    fn la_comparaison_porte_sur_majeur_et_mineur() {
        let old = GamescopeVersion {
            major: 3,
            minor: 15,
            patch: 14,
        };
        let good = GamescopeVersion {
            major: 3,
            minor: 16,
            patch: 0,
        };
        assert!(!old.at_least(MINIMUM.0, MINIMUM.1));
        assert!(good.at_least(MINIMUM.0, MINIMUM.1));
        assert!(good.at_least(3, 17) == false);
    }

    #[test]
    fn un_binaire_qui_ne_s_execute_pas_est_refuse() {
        let probe = GamescopeProbe {
            responded: false,
            version: None,
            missing: Vec::new(),
            error: Some("exécution impossible : introuvable".into()),
        };
        assert!(!probe.usable());
        let problem = probe.problem().unwrap();
        assert!(problem.contains("lancement direct"), "{problem}");
    }

    #[test]
    fn une_version_illisible_n_ecarte_pas_le_compositeur() {
        // Les distributions enveloppent parfois GameScope d'un script qui ne
        // répond pas à `--version` : le compositeur reste utilisable.
        let probe = GamescopeProbe {
            responded: true,
            version: None,
            missing: Vec::new(),
            error: Some("aucune réponse à --version".into()),
        };
        assert!(probe.usable());
        assert_eq!(probe.problem(), None);
    }

    #[test]
    fn une_version_trop_ancienne_est_refusee() {
        let probe = GamescopeProbe {
            responded: true,
            version: Some(GamescopeVersion {
                major: 3,
                minor: 14,
                patch: 2,
            }),
            missing: Vec::new(),
            error: None,
        };
        assert!(!probe.usable());
        let problem = probe.problem().unwrap();
        assert!(problem.contains("3.14.2"), "{problem}");
        assert!(problem.contains("minimum 3.16"), "{problem}");
    }

    #[test]
    fn une_option_absente_est_nommee() {
        let probe = GamescopeProbe {
            responded: true,
            version: Some(GamescopeVersion {
                major: 3,
                minor: 17,
                patch: 0,
            }),
            missing: vec!["--hdr-enabled"],
            error: None,
        };
        let problem = probe.problem().unwrap();
        assert!(problem.contains("--hdr-enabled"), "{problem}");
    }

    #[test]
    fn un_binaire_conforme_passe() {
        let probe = GamescopeProbe {
            responded: true,
            version: Some(GamescopeVersion {
                major: 3,
                minor: 17,
                patch: 0,
            }),
            missing: Vec::new(),
            error: None,
        };
        assert!(probe.usable());
        assert_eq!(probe.problem(), None);
    }

    /// Script de test qui répond comme GameScope.
    #[cfg(unix)]
    fn script(dir: &Path, version: &str, help: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("gamescope");
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\ncase \"$1\" in\n--version) echo \"gamescope version {version}\" ;;\n\
                 --help) echo \"{help}\" ;;\nesac\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[cfg(unix)]
    #[test]
    fn inspecte_un_binaire_reel() {
        let dir = tempfile::tempdir().unwrap();
        let full = REQUIRED.join(" ");
        let binary = script(dir.path(), "3.17.0", &full);
        let probe = inspect(&binary);
        assert_eq!(
            probe.version,
            Some(GamescopeVersion {
                major: 3,
                minor: 17,
                patch: 0
            })
        );
        assert!(probe.missing.is_empty(), "{:?}", probe.missing);
        assert!(probe.usable());

        let binary = script(dir.path(), "3.15.1", &full);
        assert!(!inspect(&binary).usable());

        let binary = script(dir.path(), "3.17.0", "--framerate-limit");
        let probe = inspect(&binary);
        assert!(probe.missing.contains(&"--hdr-enabled"), "{:?}", probe.missing);
    }

    #[cfg(unix)]
    #[test]
    fn un_binaire_absent_ne_panique_pas() {
        let probe = inspect(Path::new("/aucun/gamescope/ici"));
        assert!(!probe.usable());
        assert!(probe.error.is_some());
    }
}
