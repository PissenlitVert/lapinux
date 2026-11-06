//! Lecture des `appmanifest_<appid>.acf` de Steam.

use crate::vdf;
use std::path::{Path, PathBuf};

pub const STATE_UNINSTALLED: u32 = 1;
pub const STATE_UPDATE_REQUIRED: u32 = 2;
pub const STATE_FULLY_INSTALLED: u32 = 4;

#[derive(Debug, Clone, PartialEq)]
pub struct AppManifest {
    pub appid: u32,
    pub name: String,
    pub installdir: String,
    pub state_flags: u32,
    pub size_on_disk: u64,
    pub last_updated: i64,
    pub last_played: i64,
    pub build_id: String,
    /// Manifeste écrit par Lapinux (par opposition à un manifeste Steam).
    pub lapinux_managed: bool,
}

impl AppManifest {
    pub fn parse(text: &str, fallback_appid: u32) -> Result<Self, crate::Error> {
        let root = vdf::parse_root(text).map_err(|e| crate::Error::Vdf(e.to_string()))?;
        let appid = root
            .str("appid")
            .and_then(|s| s.trim().parse::<u32>().ok())
            .unwrap_or(fallback_appid);
        let name = root
            .str("name")
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("App {appid}"));
        let installdir = root.str("installdir").unwrap_or_default().to_string();
        Ok(Self {
            appid,
            name,
            installdir,
            state_flags: root.u64("StateFlags") as u32,
            size_on_disk: root.u64("SizeOnDisk"),
            last_updated: root.i64("LastUpdated"),
            last_played: root.i64("LastPlayed"),
            build_id: root.str("buildid").unwrap_or_default().to_string(),
            lapinux_managed: root.u64("LapinuxManaged") == 1,
        })
    }

    pub fn read(path: &Path) -> Result<Self, crate::Error> {
        let text = std::fs::read_to_string(path)?;
        let fallback = appid_from_filename(path).unwrap_or(0);
        Self::parse(&text, fallback)
    }

    pub fn installed(&self) -> bool {
        self.state_flags & STATE_FULLY_INSTALLED != 0
    }
}

/// Écrit un `appmanifest_<appid>.acf` minimal pour une installation gérée par Lapinux.
/// Steam le reconnaît comme installé ; Lapinux s'en sert pour sa bibliothèque.
pub fn write_manifest(
    library_root: &Path,
    appid: u32,
    name: &str,
    installdir: &str,
    size_on_disk: u64,
    managed: bool,
) -> Result<std::path::PathBuf, crate::Error> {
    let steamapps = manifest_dir(library_root, managed);
    std::fs::create_dir_all(&steamapps)?;
    let escaped_name = name.replace('"', "'");
    let escaped_dir = installdir.replace('"', "'");
    let text = format!(
        "\"AppState\"\n{{\n\t\"appid\"\t\t\"{appid}\"\n\t\"name\"\t\t\"{escaped_name}\"\n\t\"StateFlags\"\t\t\"4\"\n\t\"installdir\"\t\t\"{escaped_dir}\"\n\t\"SizeOnDisk\"\t\t\"{size_on_disk}\"\n\t\"LastUpdated\"\t\t\"{}\"\n\t\"LapinuxManaged\"\t\t\"1\"\n}}\n",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    );
    let path = steamapps.join(format!("appmanifest_{appid}.acf"));
    std::fs::write(&path, text)?;
    Ok(path)
}

/// Dossier des manifestes : `Steam` dans une bibliothèque gérée (les jeux y
/// vivent sous le même nom), `steamapps` dans une vraie bibliothèque Steam.
fn manifest_dir(library_root: &Path, managed: bool) -> PathBuf {
    if managed {
        library_root.join("Steam")
    } else {
        library_root.join("steamapps")
    }
}

/// Marque un manifeste Steam existant comme géré par Lapinux sans écraser ses
/// champs (buildid, taille, états), pour garder les mises à jour incrémentales.
pub fn mark_managed(
    library_root: &Path,
    appid: u32,
    name: &str,
    installdir: &str,
    size_on_disk: u64,
    managed: bool,
) -> Result<std::path::PathBuf, crate::Error> {
    let steamapps = manifest_dir(library_root, managed);
    let path = steamapps.join(format!("appmanifest_{appid}.acf"));
    if path.is_file() {
        let text = std::fs::read_to_string(&path)?;
        if !text.contains("LapinuxManaged") {
            let marker = "\t\"LapinuxManaged\"\t\t\"1\"\n";
            let updated = match text.rfind('}') {
                Some(index) => {
                    let (head, tail) = text.split_at(index);
                    format!("{head}{marker}{tail}")
                }
                None => {
                    return Err(crate::Error::Vdf(format!(
                        "manifeste sans accolade fermante : {}",
                        path.display()
                    )))
                }
            };
            std::fs::write(&path, updated)?;
        }
        return Ok(path);
    }
    write_manifest(
        library_root,
        appid,
        name,
        installdir,
        size_on_disk,
        managed,
    )
}

/// Réécrit un champ du manifeste, ou l'insère avant l'accolade fermante.
fn set_field(text: &str, key: &str, value: &str) -> String {
    let prefix = format!("\"{key}\"");
    let mut found = false;
    let mut lines: Vec<String> = text
        .lines()
        .map(|line| {
            if line.trim_start().starts_with(&prefix) {
                found = true;
                format!("\t\"{key}\"\t\t\"{value}\"")
            } else {
                line.to_string()
            }
        })
        .collect();
    if !found {
        let marker = format!("\t\"{key}\"\t\t\"{value}\"");
        match lines.iter().rposition(|line| line.trim() == "}") {
            Some(index) => lines.insert(index, marker),
            None => lines.push(marker),
        }
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// Inscrit le build réellement installé dans le manifeste, sans changer son
/// origine : un jeu Steam mis à jour par Lapinux reste un jeu Steam (même
/// préfixe Proton, même `compatdata`) et ne devient pas « géré par Lapinux ».
pub fn record_build(
    library_root: &Path,
    appid: u32,
    name: &str,
    installdir: &str,
    size_on_disk: u64,
    build_id: Option<u64>,
    managed: bool,
) -> Result<std::path::PathBuf, crate::Error> {
    let path = manifest_dir(library_root, managed)
        .join(format!("appmanifest_{appid}.acf"));
    if !path.is_file() {
        write_manifest(
            library_root,
            appid,
            name,
            installdir,
            size_on_disk,
            managed,
        )?;
    }
    let mut text = std::fs::read_to_string(&path)?;
    text = set_field(&text, "SizeOnDisk", &size_on_disk.to_string());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    text = set_field(&text, "LastUpdated", &now.to_string());
    if let Some(build_id) = build_id {
        text = set_field(&text, "buildid", &build_id.to_string());
    }
    std::fs::write(&path, text)?;
    Ok(path)
}

pub fn appid_from_filename(path: &Path) -> Option<u32> {
    let name = path.file_name()?.to_string_lossy();
    let rest = name.strip_prefix("appmanifest_")?;
    let digits = rest.strip_suffix(".acf")?;
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TF2: &str = r#"
"AppState"
{
	"appid"		"440"
	"name"		"Team Fortress 2"
	"StateFlags"		"4"
	"installdir"		"Team Fortress 2"
	"LastUpdated"		"1712345678"
	"LastPlayed"		"0"
	"SizeOnDisk"		"25769803776"
	"buildid"		"12345678"
}
"#;

    #[test]
    fn parses_install_manifest() {
        let m = AppManifest::parse(TF2, 0).unwrap();
        assert_eq!(m.appid, 440);
        assert_eq!(m.name, "Team Fortress 2");
        assert!(m.installed());
        assert!(!m.lapinux_managed);
        assert_eq!(m.size_on_disk, 25769803776);
        assert_eq!(m.build_id, "12345678");
        assert_eq!(m.installdir, "Team Fortress 2");
    }

    #[test]
    fn recording_a_build_keeps_steam_origin_and_other_fields() {
        let dir = tempfile::tempdir().unwrap();
        let steamapps = dir.path().join("steamapps");
        std::fs::create_dir_all(&steamapps).unwrap();
        let path = steamapps.join("appmanifest_440.acf");
        std::fs::write(&path, TF2).unwrap();

        record_build(
            dir.path(),
            440,
            "Team Fortress 2",
            "Team Fortress 2",
            1234,
            Some(987654),
            false,
        )
        .unwrap();

        let manifest = AppManifest::read(&path).unwrap();
        assert_eq!(manifest.build_id, "987654");
        assert_eq!(manifest.size_on_disk, 1234);
        assert!(!manifest.lapinux_managed);
        assert_eq!(manifest.name, "Team Fortress 2");
        assert_eq!(manifest.state_flags, STATE_FULLY_INSTALLED);
    }

    #[test]
    fn recording_a_build_creates_a_lapinux_manifest_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        record_build(dir.path(), 620, "Portal 2", "Portal 2", 42, Some(7), false).unwrap();
        let manifest = AppManifest::read(&dir.path().join("steamapps/appmanifest_620.acf")).unwrap();
        assert_eq!(manifest.build_id, "7");
        assert!(manifest.lapinux_managed);
        assert!(manifest.installed());
    }

    #[test]
    fn written_manifest_is_readable() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_manifest(
            dir.path(),
            440,
            "Team \"Fortress\" 2",
            "Team Fortress 2",
            42,
            false,
        )
        .unwrap();
        assert!(path.ends_with("steamapps/appmanifest_440.acf"));
        let manifest = AppManifest::read(&path).unwrap();
        assert_eq!(manifest.appid, 440);
        assert_eq!(manifest.installdir, "Team Fortress 2");
        assert!(manifest.installed());
        assert!(manifest.lapinux_managed);
        assert_eq!(manifest.size_on_disk, 42);
    }

    #[test]
    fn marking_keeps_the_original_build_id() {
        let dir = tempfile::tempdir().unwrap();
        let steamapps = dir.path().join("steamapps");
        std::fs::create_dir_all(&steamapps).unwrap();
        let path = steamapps.join("appmanifest_440.acf");
        std::fs::write(&path, TF2).unwrap();
        let marked = mark_managed(
            dir.path(),
            440,
            "Team Fortress 2",
            "Team Fortress 2",
            1,
            false,
        )
        .unwrap();
        assert_eq!(marked, path);
        let manifest = AppManifest::read(&path).unwrap();
        assert!(manifest.lapinux_managed);
        assert_eq!(manifest.build_id, "12345678");
        assert_eq!(manifest.size_on_disk, 25769803776);
    }

    #[test]
    fn appid_from_path() {
        let p = Path::new("/tmp/steamapps/appmanifest_228980.acf");
        assert_eq!(appid_from_filename(p), Some(228980));
        assert_eq!(appid_from_filename(Path::new("/tmp/nope.acf")), None);
    }

    #[test]
    fn missing_fields_have_defaults() {
        let m = AppManifest::parse("\"AppState\" { \"appid\" \"10\" }", 10).unwrap();
        assert_eq!(m.name, "App 10");
        assert!(!m.installed());
        assert_eq!(m.size_on_disk, 0);
    }

    #[test]
    fn invalid_vdf_is_error() {
        assert!(AppManifest::parse("\"AppState\" {", 1).is_err());
    }
}
