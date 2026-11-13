//! Téléchargement, vérification et extraction des builds Proton.

use crate::{Error, Result};
use sha2::{Digest, Sha512};
use std::io::Read;
use std::path::{Path, PathBuf};

/// Installe un Proton depuis une URL d'archive (nom déduit du fichier).
pub fn install_from_url(
    url: &str,
    checksum_url: Option<&str>,
    dest_root: &Path,
    on_progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<PathBuf> {
    std::fs::create_dir_all(dest_root)?;
    let file_name = url
        .rsplit('/')
        .next()
        .filter(|n| !n.is_empty())
        .ok_or_else(|| Error::Other(format!("URL sans nom de fichier : {url}")))?;
    let staging = dest_root.join(".staging");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    let archive = staging.join(file_name);

    crate::tools::download_to(url, &archive, on_progress)?;

    if let Some(checksum_url) = checksum_url {
        let text = crate::tools::fetch_text(checksum_url)?;
        let expected = parse_checksum(&text)
            .ok_or_else(|| Error::Other(format!("somme illisible : {checksum_url}")))?;
        if !verify_sha512(&archive, &expected)? {
            let _ = std::fs::remove_file(&archive);
            return Err(Error::Other(format!(
                "checksum SHA-512 invalide pour {file_name}"
            )));
        }
    } else {
        tracing::warn!("aucune somme SHA-512 publiée pour {file_name}");
    }

    let extracted = extract(&archive, &staging)?;
    std::fs::remove_file(&archive).ok();
    let target = dest_root.join(
        extracted
            .file_name()
            .ok_or_else(|| Error::Other("archive vide".into()))?,
    );
    if target.exists() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(Error::Other(format!("{} existe déjà", target.display())));
    }
    std::fs::rename(&extracted, &target)?;
    let _ = std::fs::remove_dir_all(&staging);
    validate(&target)?;
    Ok(target)
}

/// Vérifie la présence des fichiers indispensables d'un Proton.
pub fn validate(dir: &Path) -> Result<()> {
    let proton = dir.join("proton");
    let wine = dir.join("files/bin/wine");
    if !proton.is_file() && !wine.is_file() {
        return Err(Error::Other(format!(
            "{} ne ressemble pas à un Proton (ni proton ni files/bin/wine)",
            dir.display()
        )));
    }
    if proton.is_file() {
        crate::tools::chmod_exec(&proton)?;
    }
    Ok(())
}

/// Décompresse une archive `.tar.gz`, `.tar.xz` ou `.tar.zst` dans `dest`.
pub fn extract(archive: &Path, dest: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(dest)?;
    let name = archive
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let file = std::fs::File::open(archive)?;
    if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
        tar.unpack(dest)?;
    } else if name.ends_with(".tar.xz") {
        let mut tar = tar::Archive::new(xz2::read::XzDecoder::new(file));
        tar.unpack(dest)?;
    } else if name.ends_with(".tar.zst") {
        let decoder = ruzstd::StreamingDecoder::new(std::io::BufReader::new(file))
            .map_err(|e| Error::Other(format!("zstd : {e}")))?;
        let mut tar = tar::Archive::new(decoder);
        tar.unpack(dest)?;
    } else {
        return Err(Error::Other(format!(
            "format d'archive non pris en charge : {}",
            archive.display()
        )));
    }
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(dest)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    match dirs.len() {
        0 => Err(Error::Other("archive sans répertoire racine".into())),
        1 => Ok(dirs.remove(0)),
        _ => Err(Error::Other(
            "archive avec plusieurs répertoires racine".into(),
        )),
    }
}

/// Vérifie une somme SHA-512 (hexadécimal) en flux.
pub fn verify_sha512(path: &Path, expected_hex: &str) -> Result<bool> {
    let expected = expected_hex.trim().to_ascii_lowercase();
    if expected.len() != 128 {
        return Err(Error::Other("somme SHA-512 de longueur invalide".into()));
    }
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha512::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let digest = hasher.finalize();
    let actual: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    Ok(actual == expected)
}

/// Première somme hexadécimale d'un fichier `.sha512sum`.
pub fn parse_checksum(text: &str) -> Option<String> {
    for line in text.lines() {
        let token = line.split_whitespace().next()?;
        let cleaned = token.trim().to_ascii_lowercase();
        if cleaned.len() == 128 && cleaned.chars().all(|c| c.is_ascii_hexdigit()) {
            return Some(cleaned);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn make_tar_gz(path: &Path, entries: &[(&str, &[u8])]) {
        let file = std::fs::File::create(path).unwrap();
        let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
        let mut builder = tar::Builder::new(encoder);
        for (name, content) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o755);
            header.set_path(name).unwrap();
            header.set_cksum();
            builder.append(&header, &content[..]).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap();
    }

    #[test]
    fn extracts_single_root_archive() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("proton.tar.gz");
        make_tar_gz(
            &archive,
            &[
                ("GE-Proton11-7/proton", b"#!/bin/sh\n"),
                ("GE-Proton11-7/version", b"11-7\n"),
            ],
        );
        let dest = dir.path().join("out");
        let root = extract(&archive, &dest).unwrap();
        assert_eq!(root.file_name().unwrap(), "GE-Proton11-7");
        assert!(root.join("proton").is_file());
        assert!(validate(&root).is_ok());
    }

    #[test]
    fn installs_from_archive_and_validates() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("GE-Proton11-7-x86_64.tar.gz");
        make_tar_gz(&archive, &[("GE-Proton11-7-x86_64/proton", b"#!/bin/sh\n")]);
        // Simule l'étape de téléchargement en copiant dans le staging.
        let dest_root = dir.path().join("compatibilitytools.d");
        std::fs::create_dir_all(dest_root.join(".staging")).unwrap();
        let staged = dest_root.join(".staging/GE-Proton11-7-x86_64.tar.gz");
        std::fs::copy(&archive, &staged).unwrap();
        let extracted = extract(&staged, &dest_root.join(".staging")).unwrap();
        let target = dest_root.join(extracted.file_name().unwrap());
        std::fs::rename(&extracted, &target).unwrap();
        validate(&target).unwrap();
        assert!(target.join("proton").is_file());
    }

    #[test]
    fn checksum_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f.bin");
        std::fs::write(&file, b"contenu").unwrap();
        // SHA-512 de "contenu"
        let expected = "1c0c0a2f8f9d4c0a8ab83c6370a0bb2f9b7b1d1e0f0b9c6d2e5a1c3f5d8b9a7e6c4f2a1b3d5e7f9a0c2b4d6e8f0a1c3b5d7e9f1a2c4b6d8e0f2a4c6b8d0e2f4a6c8b0d2e4f6a8c0b2d4e6f8a0c2b4d6e8f0a1b3c5d7e9f1a3b5c7d9e1f3a5b7c9d1e3f5a7b9c1d3e5f7a9b1c3d5e7f9a";
        // La constante ci-dessus est volontairement fausse : on vérifie le rejet.
        let result = verify_sha512(&file, expected);
        assert!(matches!(result, Ok(false) | Err(_)));
        assert!(parse_checksum("pas une somme").is_none());
        let real = {
            let mut hasher = Sha512::new();
            hasher.update(b"contenu");
            hasher
                .finalize()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        };
        assert!(verify_sha512(&file, &real).unwrap());
        assert_eq!(
            parse_checksum(&format!("{real}  f.bin\n")),
            Some(real.clone())
        );
    }

    #[test]
    fn rejects_bad_archive() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("x.tar.gz");
        let mut f = std::fs::File::create(&archive).unwrap();
        f.write_all(b"pas une archive").unwrap();
        assert!(extract(&archive, &dir.path().join("out")).is_err());
    }
}
