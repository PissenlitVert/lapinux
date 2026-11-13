//! Mises à jour publiées sur GitHub : lecture d'une release, comparaison de
//! versions et état d'un téléchargement.

use serde::{Deserialize, Serialize};

/// Dépôt interrogé. `LAPINUX_UPDATE_REPO` le remplace pour les essais.
pub const REPOSITORY: &str = "PissenlitVert/lapinux";

/// Date de fabrication du binaire, posée par `build.rs`.
pub const BUILD_DATE: &str = env!("LAPINUX_BUILD_DATE");

/// Dépôt effectif : la variable d'environnement sert aux essais locaux, le
/// dépôt du projet reste le défaut.
pub fn repository() -> String {
    std::env::var("LAPINUX_UPDATE_REPO")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| REPOSITORY.to_string())
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ReleaseAsset {
    pub name: String,
    pub url: String,
    pub size: u64,
    pub digest: Option<String>,
}

impl ReleaseAsset {
    /// Empreinte SHA-256 annoncée par GitHub (`sha256:…`), sans préfixe.
    pub fn sha256(&self) -> Option<&str> {
        self.digest
            .as_deref()
            .and_then(|digest| digest.strip_prefix("sha256:"))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Release {
    pub tag: String,
    pub version: String,
    pub published: String,
    pub notes: String,
    pub asset: Option<ReleaseAsset>,
}

/// Ce que la page de réglages affiche : le binaire courant et la dernière
/// release publiée, quand elle existe.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UpdateStatus {
    pub current: String,
    pub current_date: String,
    pub latest: Option<Release>,
    pub available: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum UpdateStage {
    #[default]
    Downloading,
    Verifying,
    Ready,
    Failed,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UpdateProgress {
    pub tag: String,
    pub stage: UpdateStage,
    pub received: u64,
    pub total: u64,
    pub message: String,
    pub path: Option<String>,
}

impl UpdateProgress {
    pub fn fraction(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            (self.received as f32 / self.total as f32).clamp(0.0, 1.0)
        }
    }
}

/// Version d'un tag : `v0.2.0` → `0.2.0`.
pub fn version_of(tag: &str) -> String {
    tag.trim().trim_start_matches(['v', 'V']).to_string()
}

/// La version publiée est-elle plus récente que celle du binaire ? Les
/// composants numériques sont comparés un à un, un suffixe de pré-version est
/// ignoré : `1.10` vient après `1.9`.
pub fn is_newer(latest: &str, current: &str) -> bool {
    let latest = numbers(latest);
    let current = numbers(current);
    for index in 0..latest.len().max(current.len()) {
        let left = latest.get(index).copied().unwrap_or(0);
        let right = current.get(index).copied().unwrap_or(0);
        if left != right {
            return left > right;
        }
    }
    false
}

fn numbers(version: &str) -> Vec<u64> {
    version_of(version)
        .split('-')
        .next()
        .unwrap_or_default()
        .split('.')
        .map(|part| part.parse::<u64>().unwrap_or(0))
        .collect()
}

pub fn parse_release(json: &str) -> Result<Release, String> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|error| format!("release illisible : {error}"))?;
    let tag = value
        .get("tag_name")
        .and_then(|tag| tag.as_str())
        .filter(|tag| !tag.trim().is_empty())
        .ok_or_else(|| "release sans tag".to_string())?
        .to_string();
    let published = value
        .get("published_at")
        .and_then(|date| date.as_str())
        .unwrap_or_default()
        .chars()
        .take(10)
        .collect();
    let notes = value
        .get("body")
        .and_then(|body| body.as_str())
        .unwrap_or_default()
        .to_string();
    let asset = value
        .get("assets")
        .and_then(|assets| assets.as_array())
        .and_then(|assets| best_asset(assets));
    Ok(Release {
        version: version_of(&tag),
        tag,
        published,
        notes,
        asset,
    })
}

/// L'AppImage x86-64 de la release : un asset `.AppImage` dont le nom cite
/// l'architecture passe avant les autres.
fn best_asset(assets: &[serde_json::Value]) -> Option<ReleaseAsset> {
    let candidates: Vec<ReleaseAsset> = assets
        .iter()
        .filter_map(|asset| {
            let name = asset.get("name")?.as_str()?.to_string();
            if !name.to_ascii_lowercase().ends_with(".appimage") {
                return None;
            }
            Some(ReleaseAsset {
                url: asset.get("browser_download_url")?.as_str()?.to_string(),
                size: asset.get("size").and_then(|size| size.as_u64()).unwrap_or(0),
                digest: asset
                    .get("digest")
                    .and_then(|digest| digest.as_str())
                    .map(|digest| digest.to_string()),
                name,
            })
        })
        .collect();
    candidates
        .iter()
        .find(|asset| {
            let name = asset.name.to_ascii_lowercase();
            name.contains("x86_64") || name.contains("amd64")
        })
        .or_else(|| candidates.first())
        .cloned()
}

pub fn status(current: &str, current_date: &str, latest: Release) -> UpdateStatus {
    UpdateStatus {
        current: version_of(current),
        current_date: current_date.to_string(),
        available: is_newer(&latest.version, current),
        latest: Some(latest),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RELEASE: &str = r#"{
        "tag_name": "v0.2.0",
        "published_at": "2026-11-01T12:34:56Z",
        "body": "Nouveautés",
        "assets": [
            {"name": "Lapinux-0.2.0-aarch64.AppImage", "browser_download_url": "https://example/a", "size": 10, "digest": "sha256:aa"},
            {"name": "Lapinux-0.2.0-x86_64.AppImage", "browser_download_url": "https://example/b", "size": 20, "digest": "sha256:bb"},
            {"name": "Lapinux-0.2.0-x86_64.AppImage.zsync", "browser_download_url": "https://example/c", "size": 30}
        ]
    }"#;

    #[test]
    fn a_release_prefers_the_x86_64_appimage() {
        let release = parse_release(RELEASE).unwrap();
        assert_eq!(release.tag, "v0.2.0");
        assert_eq!(release.version, "0.2.0");
        assert_eq!(release.published, "2026-11-01");
        assert_eq!(release.notes, "Nouveautés");
        let asset = release.asset.unwrap();
        assert_eq!(asset.name, "Lapinux-0.2.0-x86_64.AppImage");
        assert_eq!(asset.size, 20);
        assert_eq!(asset.sha256(), Some("bb"));
    }

    #[test]
    fn a_release_without_appimage_is_usable_but_not_downloadable() {
        let release = parse_release(r#"{"tag_name":"v1.0.0","assets":[]}"#).unwrap();
        assert!(release.asset.is_none());
        assert_eq!(release.published, "");
    }

    #[test]
    fn a_release_without_tag_is_an_error() {
        assert!(parse_release(r#"{"assets":[]}"#).is_err());
    }

    #[test]
    fn versions_compare_by_numbers() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("1.10.0", "1.9.0"));
        assert!(is_newer("v0.1.1", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(!is_newer("1.0.0", "1"));
        assert!(!is_newer("1.0", "1.0.1"));
        assert!(is_newer("1.0.1", "1.0"));
    }

    #[test]
    fn a_build_has_a_date() {
        assert!(!BUILD_DATE.is_empty());
    }

    #[test]
    fn progress_fraction_is_bounded() {
        let progress = UpdateProgress {
            received: 5,
            total: 10,
            ..Default::default()
        };
        assert!((progress.fraction() - 0.5).abs() < f32::EPSILON);
        let empty = UpdateProgress::default();
        assert_eq!(empty.fraction(), 0.0);
    }
}
