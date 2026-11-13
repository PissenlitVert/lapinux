//! Sélection de dossier par les outils du bureau.

use std::path::{Path, PathBuf};

/// Sans zenity ni kdialog, le champ texte reste utilisable.
pub(super) fn pick_folder(current: &Path, title: &str) -> Option<PathBuf> {
    let current = if current.exists() {
        current.to_path_buf()
    } else {
        lapinux_core::paths::home_dir()
    };
    if let Some(zenity) = lapinux_core::paths::which("zenity") {
        let mut command = std::process::Command::new(zenity);
        command
            .arg("--file-selection")
            .arg("--directory")
            .arg(format!("--title={title}"))
            .arg(format!("--filename={}/", current.display()))
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        if let Ok(output) = command.output() {
            if output.status.success() {
                let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !path.is_empty() {
                    return Some(PathBuf::from(path));
                }
            }
        }
    }
    if let Some(kdialog) = lapinux_core::paths::which("kdialog") {
        let output = std::process::Command::new(kdialog)
            .arg("--getexistingdirectory")
            .arg(&current)
            .arg("--title")
            .arg(title)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return Some(PathBuf::from(path));
            }
        }
    }
    None
}
