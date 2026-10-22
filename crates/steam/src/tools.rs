//! Téléchargements et extraction d'archives pour les outils externes.

use crate::{Error, Result};
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

pub const USER_AGENT: &str = concat!("ludus/", env!("CARGO_PKG_VERSION"));

pub fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .user_agent(USER_AGENT)
        .timeout_connect(Duration::from_secs(20))
        .timeout_read(Duration::from_secs(60))
        .build()
}

pub fn http_error(err: ureq::Error) -> Error {
    match err {
        ureq::Error::Status(code, resp) => {
            Error::Http(format!("HTTP {code} pour {}", resp.get_url()))
        }
        ureq::Error::Transport(t) => Error::Http(format!("réseau : {t}")),
    }
}

/// Télécharge `url` vers `dest`, en rapportant (octets reçus, taille totale).
pub fn download_to(
    url: &str,
    dest: &Path,
    on_progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let resp = agent().get(url).call().map_err(http_error)?;
    let total = resp
        .header("Content-Length")
        .and_then(|v| v.parse::<u64>().ok())
        .or_else(|| {
            resp.header("Content-Range")
                .and_then(|v| v.rsplit('/').next().and_then(|n| n.parse::<u64>().ok()))
        });
    let mut reader = resp.into_reader();
    let mut file = File::create(dest)?;
    let mut buffer = [0u8; 64 * 1024];
    let mut received: u64 = 0;
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
    let resp = agent().get(url).call().map_err(http_error)?;
    let mut body = String::new();
    resp.into_reader()
        .take(8 * 1024 * 1024)
        .read_to_string(&mut body)?;
    Ok(body)
}

pub fn extract_tar_gz(archive: &Path, dest: &Path) -> Result<()> {
    let file = File::open(archive)?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut tar = tar::Archive::new(decoder);
    std::fs::create_dir_all(dest)?;
    tar.unpack(dest)?;
    Ok(())
}

pub fn chmod_exec(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(perms.mode() | 0o755);
    std::fs::set_permissions(path, perms)?;
    Ok(())
}

/// Extrait une entrée précise d'un zip (recherche par suffixe).
pub fn extract_zip_entry(archive: &Path, entry_suffix: &str, dest: &Path) -> Result<()> {
    let file = File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| Error::Other(format!("zip : {e}")))?;
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|e| Error::Other(format!("zip : {e}")))?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().to_string();
        if name.ends_with(entry_suffix) {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut out = File::create(dest)?;
            std::io::copy(&mut entry, &mut out)?;
            chmod_exec(dest)?;
            return Ok(());
        }
    }
    Err(Error::Other(format!(
        "entrée {entry_suffix} absente de {}",
        archive.display()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn extracts_zip_entry() {
        let dir = tempfile::tempdir().unwrap();
        let archive_path = dir.path().join("a.zip");
        {
            let file = File::create(&archive_path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default();
            zip.start_file("bin/tool", options).unwrap();
            zip.write_all(b"#!/bin/sh\n").unwrap();
            zip.finish().unwrap();
        }
        let out = dir.path().join("tool");
        extract_zip_entry(&archive_path, "bin/tool", &out).unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"#!/bin/sh\n");
    }

    #[test]
    fn extracts_tar_gz() {
        let dir = tempfile::tempdir().unwrap();
        let archive_path = dir.path().join("a.tar.gz");
        {
            let file = File::create(&archive_path).unwrap();
            let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
            let mut builder = tar::Builder::new(encoder);
            let mut header = tar::Header::new_gnu();
            header.set_path("hello.txt").unwrap();
            header.set_size(5);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append(&header, &b"salut"[..]).unwrap();
            builder.into_inner().unwrap().finish().unwrap();
        }
        let dest = dir.path().join("out");
        extract_tar_gz(&archive_path, &dest).unwrap();
        assert_eq!(std::fs::read_to_string(dest.join("hello.txt")).unwrap(), "salut");
    }
}
