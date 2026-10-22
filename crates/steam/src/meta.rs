//! Métadonnées d'application issues de `steamcmd +app_info_print`.

use crate::vdf;

#[derive(Debug, Clone, PartialEq)]
pub struct LaunchEntry {
    pub executable: String,
    pub oslist: Vec<String>,
    pub description: String,
    pub arguments: String,
}

impl LaunchEntry {
    pub fn is_windows(&self) -> bool {
        self.oslist.is_empty()
            || self.oslist.iter().any(|os| os.eq_ignore_ascii_case("windows"))
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct AppInfo {
    pub appid: u32,
    pub name: String,
    pub installdir: String,
    pub kind: String,
    pub launch: Vec<LaunchEntry>,
}

/// Extrait le bloc VDF d'une sortie de `app_info_print`.
pub fn extract_vdf_block(output: &str) -> Option<&str> {
    let bytes = output.as_bytes();
    let mut start = None;
    for (index, line) in output.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() > 2 {
            let inner = &trimmed[1..trimmed.len() - 1];
            if inner.chars().all(|c| c.is_ascii_digit()) {
                // position de cette ligne dans la sortie
                let offset: usize = output
                    .lines()
                    .take(index)
                    .map(|l| l.len() + 1)
                    .sum::<usize>()
                    .min(bytes.len());
                start = Some(offset);
                break;
            }
        }
    }
    let start = start?;
    let mut depth = 0i32;
    let mut seen_brace = false;
    let mut end = None;
    for (offset, ch) in output[start..].char_indices() {
        match ch {
            '{' => {
                depth += 1;
                seen_brace = true;
            }
            '}' => {
                depth -= 1;
                if seen_brace && depth == 0 {
                    end = Some(start + offset + 1);
                    break;
                }
            }
            _ => {}
        }
    }
    let end = end?;
    Some(&output[start..end])
}

pub fn parse_app_info(text: &str) -> Option<AppInfo> {
    let block = extract_vdf_block(text)?;
    let root = vdf::parse_root(block).ok()?;
    let common = root.obj("common");
    let config = root.obj("config");
    let name = common
        .and_then(|c| c.str("name"))
        .or_else(|| root.str("name"))
        .unwrap_or_default()
        .to_string();
    let installdir = config
        .and_then(|c| c.str("installdir"))
        .or_else(|| common.and_then(|c| c.str("installdir")))
        .unwrap_or_default()
        .to_string();
    let kind = common
        .and_then(|c| c.str("type"))
        .unwrap_or_default()
        .to_string();
    let mut launch = Vec::new();
    if let Some(entries) = config.and_then(|c| c.obj("launch")) {
        for (_index, entry) in entries.objects() {
            let executable = entry.str("executable").unwrap_or_default().to_string();
            if executable.is_empty() {
                continue;
            }
            let oslist = entry
                .obj("config")
                .and_then(|c| c.str("oslist"))
                .map(|s| {
                    s.split(',')
                        .map(|part| part.trim().to_string())
                        .filter(|p| !p.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            launch.push(LaunchEntry {
                executable,
                oslist,
                description: entry.str("description").unwrap_or_default().to_string(),
                arguments: entry.str("arguments").unwrap_or_default().to_string(),
            });
        }
    }
    Some(AppInfo {
        appid: root
            .str("appid")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0),
        name,
        installdir,
        kind,
        launch,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUTPUT: &str = r#"
Redirecting stderr to '/home/x/.steam/logs/stderr.txt'
Logging in user 'anonymous' to Steam Public...OK
Waiting for user info...OK
"440"
{
	"appid"		"440"
	"common"
	{
		"name"		"Team Fortress 2"
		"type"		"Game"
	}
	"config"
	{
		"installdir"		"Team Fortress 2"
		"launch"
		{
			"0"
			{
				"executable"		"hl2.exe"
				"arguments"		"-steam"
				"config"		{ "oslist"		"windows" }
				"description"		"Jouer"
			}
			"1"
			{
				"executable"		"hl2_linux"
				"config"		{ "oslist"		"linux" }
			}
		}
	}
}
"#;

    #[test]
    fn extracts_block_skipping_preamble() {
        let block = extract_vdf_block(OUTPUT).unwrap();
        assert!(block.starts_with("\"440\""));
        assert!(block.trim_end().ends_with('}'));
    }

    #[test]
    fn parses_name_installdir_launch() {
        let info = parse_app_info(OUTPUT).unwrap();
        assert_eq!(info.name, "Team Fortress 2");
        assert_eq!(info.installdir, "Team Fortress 2");
        assert_eq!(info.kind, "Game");
        assert_eq!(info.launch.len(), 2);
        assert!(info.launch[0].is_windows());
        assert!(!info.launch[1].is_windows());
        assert_eq!(info.launch[0].arguments, "-steam");
    }

    #[test]
    fn no_block_is_none() {
        assert!(parse_app_info("Loading Steam API...OK\n").is_none());
    }
}
