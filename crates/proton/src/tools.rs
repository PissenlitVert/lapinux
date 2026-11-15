//! Téléchargements HTTP pour les builds Proton.

use crate::{Error, Result};
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

pub const USER_AGENT: &str = concat!("lapinux/", env!("CARGO_PKG_VERSION"));

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .user_agent(USER_AGENT)
        .timeout_connect(std::time::Duration::from_secs(20))
        .timeout_read(std::time::Duration::from_secs(120))
        .build()
}

pub fn download_to(
    url: &str,
    dest: &Path,
    on_progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let response = agent().get(url).call().map_err(|err| match err {
        ureq::Error::Status(code, _) => Error::Http(format!("HTTP {code} pour {url}")),
        ureq::Error::Transport(t) => Error::Http(format!("{t}")),
    })?;
    let total = response
        .header("Content-Length")
        .and_then(|v| v.parse::<u64>().ok());
    let mut reader = response.into_reader();
    let mut file = File::create(dest)?;
    let mut buffer = [0u8; 64 * 1024];
    let mut received = 0u64;
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read])?;
        received += read as u64;
        on_progress(received, total);
    }
    file.flush()?;
    Ok(())
}

pub fn fetch_text(url: &str) -> Result<String> {
    let response = agent().get(url).call().map_err(|err| match err {
        ureq::Error::Status(code, _) => Error::Http(format!("HTTP {code} pour {url}")),
        ureq::Error::Transport(t) => Error::Http(format!("{t}")),
    })?;
    let mut body = String::new();
    response
        .into_reader()
        .take(8 * 1024 * 1024)
        .read_to_string(&mut body)?;
    Ok(body)
}

pub fn chmod_exec(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(perms.mode() | 0o755);
    std::fs::set_permissions(path, perms)?;
    Ok(())
}
