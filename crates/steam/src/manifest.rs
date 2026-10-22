//! Lecture des `appmanifest_<appid>.acf` de Steam.

use crate::vdf;
use std::path::Path;

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

/// Écrit un `appmanifest_<appid>.acf` minimal pour une installation gérée par Ludus.
/// Steam le reconnaît comme installé ; Ludus s'en sert pour sa bibliothèque.
pub fn write_manifest(
    library_root: &Path,
    appid: u32,
    name: &str,
    installdir: &str,
    size_on_disk: u64,
) -> Result<std::path::PathBuf, crate::Error> {
    let steamapps = library_root.join("steamapps");
    std::fs::create_dir_all(&steamapps)?;
    let escaped_name = name.replace('"', "'");
    let escaped_dir = installdir.replace('"', "'");
    let text = format!(
        "\"AppState\"\n{{\n\t\"appid\"\t\t\"{appid}\"\n\t\"name\"\t\t\"{escaped_name}\"\n\t\"StateFlags\"\t\t\"4\"\n\t\"installdir\"\t\t\"{escaped_dir}\"\n\t\"SizeOnDisk\"\t\t\"{size_on_disk}\"\n\t\"LastUpdated\"\t\t\"{}\"\n}}\n",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    );
    let path = steamapps.join(format!("appmanifest_{appid}.acf"));
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
        assert_eq!(m.size_on_disk, 25769803776);
        assert_eq!(m.build_id, "12345678");
        assert_eq!(m.installdir, "Team Fortress 2");
    }

    #[test]
    fn written_manifest_is_readable() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_manifest(dir.path(), 440, "Team \"Fortress\" 2", "Team Fortress 2", 42).unwrap();
        assert!(path.ends_with("steamapps/appmanifest_440.acf"));
        let manifest = AppManifest::read(&path).unwrap();
        assert_eq!(manifest.appid, 440);
        assert_eq!(manifest.installdir, "Team Fortress 2");
        assert!(manifest.installed());
        assert_eq!(manifest.size_on_disk, 42);
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
