//! Liste et sélection des Proton publiés (GE, CachyOS, DW).
//!
//! Deux hébergeurs, une même forme de réponse JSON : GitHub pour GE-Proton et
//! Proton-CachyOS, l'instance Forgejo de Dawn Winery pour DW-Proton — son dépôt
//! GitHub n'existe plus. Chaque source porte donc l'adresse exacte de ses
//! versions, ce qui évite de deviner l'hébergeur à partir du nom du dépôt.

use crate::{Error, Result};
use lapinux_core::model::ProtonRelease;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtonSource {
    pub repo: &'static str,
    pub label: &'static str,
    /// Adresse de l'API qui liste les versions publiées.
    pub releases_api: &'static str,
}

pub const SOURCES: [ProtonSource; 3] = [
    ProtonSource {
        repo: "GloriousEggroll/proton-ge-custom",
        label: "GE-Proton",
        releases_api: "https://api.github.com/repos/GloriousEggroll/proton-ge-custom/releases?per_page=20",
    },
    ProtonSource {
        repo: "CachyOS/proton-cachyos",
        label: "Proton-CachyOS",
        releases_api: "https://api.github.com/repos/CachyOS/proton-cachyos/releases?per_page=20",
    },
    ProtonSource {
        // Dawn Winery héberge ses builds sur son propre Forgejo : l'API y suit
        // la même forme que celle de GitHub (tag, date, fichiers, sommes).
        repo: "dawn-winery/dwproton",
        label: "DW-Proton",
        releases_api: "https://dawn.wine/api/v1/repos/dawn-winery/dwproton/releases?limit=20",
    },
];

pub fn source_label(repo: &str) -> &'static str {
    SOURCES
        .iter()
        .find(|s| s.repo == repo)
        .map(|s| s.label)
        .unwrap_or("Proton")
}

const ARCHIVE_SUFFIXES: [&str; 3] = [".tar.gz", ".tar.xz", ".tar.zst"];

pub fn is_asset_archive(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    ARCHIVE_SUFFIXES.iter().any(|suffix| lower.ends_with(suffix))
}

fn is_x86_64(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    (lower.contains("x86_64") || lower.contains("x64"))
        && !lower.contains("arm")
        && !lower.contains("aarch")
}

/// Convertit la réponse JSON de l'API GitHub en releases utilisables.
pub fn parse_releases(json: &str, repo: &str) -> Result<Vec<ProtonRelease>> {
    let value: serde_json::Value = serde_json::from_str(json)?;
    let array = value
        .as_array()
        .ok_or_else(|| Error::Other("réponse GitHub inattendue".into()))?;
    let mut out = Vec::new();
    for release in array {
        let tag = release
            .get("tag_name")
            .and_then(|t| t.as_str())
            .unwrap_or_default()
            .to_string();
        let published = release
            .get("published_at")
            .and_then(|p| p.as_str())
            .unwrap_or_default()
            .to_string();
        let prerelease = release
            .get("prerelease")
            .and_then(|p| p.as_bool())
            .unwrap_or(false);
        let Some(assets) = release.get("assets").and_then(|a| a.as_array()) else {
            continue;
        };
        let names: Vec<&str> = assets
            .iter()
            .filter_map(|a| a.get("name").and_then(|n| n.as_str()))
            .collect();
        for asset in assets {
            let Some(name) = asset.get("name").and_then(|n| n.as_str()) else {
                continue;
            };
            if !is_asset_archive(name) || !is_x86_64(name) {
                continue;
            }
            let Some(url) = asset.get("browser_download_url").and_then(|u| u.as_str()) else {
                continue;
            };
            let stem = ARCHIVE_SUFFIXES
                .iter()
                .find_map(|suffix| name.strip_suffix(suffix))
                .unwrap_or(name);
            let checksum_url = names
                .iter()
                .find(|candidate| {
                    candidate
                        .strip_suffix(".sha512sum")
                        .map(|base| base == stem)
                        .unwrap_or(false)
                })
                .and_then(|checksum_name| {
                    assets
                        .iter()
                        .find(|a| a.get("name").and_then(|n| n.as_str()) == Some(*checksum_name))
                        .and_then(|a| a.get("browser_download_url").and_then(|u| u.as_str()))
                })
                .map(|s| s.to_string());
            out.push(ProtonRelease {
                repo: repo.to_string(),
                tag: tag.clone(),
                asset: name.to_string(),
                asset_url: url.to_string(),
                size: asset.get("size").and_then(|s| s.as_u64()).unwrap_or(0),
                checksum_url,
                published: published.clone(),
                prerelease,
            });
        }
    }
    Ok(out)
}

/// Télécharge la liste des dernières releases d'un dépôt.
///
/// Une source connue fournit l'adresse de son API ; un dépôt quelconque est
/// supposé GitHub, ce qui laisse la porte ouverte aux builds saisis à la main.
pub fn fetch_releases(repo: &str) -> Result<Vec<ProtonRelease>> {
    let url = SOURCES
        .iter()
        .find(|source| source.repo == repo)
        .map(|source| source.releases_api.to_string())
        .unwrap_or_else(|| format!("https://api.github.com/repos/{repo}/releases?per_page=20"));
    let text = crate::tools::fetch_text(&url)?;
    parse_releases(&text, repo)
}

pub fn all_releases(repos: &[&str]) -> Vec<ProtonRelease> {
    let mut out = Vec::new();
    for repo in repos {
        match fetch_releases(repo) {
            Ok(mut releases) => out.append(&mut releases),
            Err(err) => tracing::warn!("releases {repo} : {err}"),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const JSON: &str = r#"[
      {
        "tag_name": "GE-Proton11-11",
        "published_at": "2026-10-01T12:00:00Z",
        "prerelease": false,
        "assets": [
          {"name": "GE-Proton11-11-x86_64.tar.gz", "size": 555, "browser_download_url": "https://x/ge.tar.gz"},
          {"name": "GE-Proton11-11-x86_64.sha512sum", "size": 10, "browser_download_url": "https://x/ge.sha512sum"},
          {"name": "GE-Proton11-11-aarch64.tar.gz", "size": 555, "browser_download_url": "https://x/ge-arm.tar.gz"}
        ]
      },
      {
        "tag_name": "cachyos-11.0-20261015-slr",
        "published_at": "2026-10-15T12:00:00Z",
        "prerelease": false,
        "assets": [
          {"name": "proton-cachyos-11.0-20261015-slr-x86_64.tar.xz", "size": 777, "browser_download_url": "https://x/cachy.tar.xz"},
          {"name": "proton-cachyos-11.0-20261015-slr-x86_64_v3.tar.xz", "size": 777, "browser_download_url": "https://x/cachy-v3.tar.xz"},
          {"name": "proton-cachyos-11.0-20261015-slr-x86_64.sha512sum", "size": 10, "browser_download_url": "https://x/cachy.sha512sum"}
        ]
      }
    ]"#;

    /// Chaque source déclare l'adresse qui liste ses versions : DW-Proton vit
    /// sur le Forgejo de Dawn Winery, pas sur GitHub.
    #[test]
    fn each_source_declares_its_releases_api() {
        for source in SOURCES {
            assert!(
                source.releases_api.contains(source.repo),
                "{} ne pointe pas vers son dépôt",
                source.label
            );
            assert!(source.releases_api.starts_with("https://"));
        }
        let dw = SOURCES
            .iter()
            .find(|source| source.label == "DW-Proton")
            .expect("DW-Proton listé");
        assert!(dw.releases_api.contains("dawn.wine"), "{}", dw.releases_api);
    }

    #[test]
    fn keeps_x86_64_archives_only() {
        let releases = parse_releases(JSON, "test/repo").unwrap();
        assert_eq!(releases.len(), 3);
        assert!(releases.iter().all(|r| !r.asset.contains("aarch64")));
        assert!(releases.iter().all(|r| !r.asset.contains("arm")));
    }

    #[test]
    fn finds_checksum_sidecar() {
        let releases = parse_releases(JSON, "test/repo").unwrap();
        let ge = releases
            .iter()
            .find(|r| r.asset == "GE-Proton11-11-x86_64.tar.gz")
            .unwrap();
        assert_eq!(ge.checksum_url.as_deref(), Some("https://x/ge.sha512sum"));
        assert_eq!(ge.tag, "GE-Proton11-11");
        assert_eq!(ge.size, 555);
        let cachy = releases
            .iter()
            .find(|r| r.asset.ends_with("_v3.tar.xz"))
            .unwrap();
        // Le sidecar ne correspond pas exactement à la variante _v3 : aucune somme
        // ne doit être associée par erreur.
        assert!(cachy.checksum_url.is_none());
        let plain = releases
            .iter()
            .find(|r| r.asset == "proton-cachyos-11.0-20261015-slr-x86_64.tar.xz")
            .unwrap();
        assert_eq!(plain.checksum_url.as_deref(), Some("https://x/cachy.sha512sum"));
    }

    #[test]
    fn bad_json_is_error() {
        assert!(parse_releases("{}", "x").is_err());
    }

    #[test]
    fn labels_resolve() {
        assert_eq!(source_label("GloriousEggroll/proton-ge-custom"), "GE-Proton");
        assert_eq!(source_label("inconnu"), "Proton");
    }
}
